//! Websites shared over pipes (pipe-port-sharing.md, the revised plan).
//!
//! The person shares a port on this computer (Share a website…) with
//! chosen pipes, or with a pipe group, which it then follows: a pipe moved
//! into the group gets it, one moved out stops getting it. Each pipe's
//! other side is offered it with a `share` frame, in order with the pipe's
//! controls (`pipe_link::share`), and told of its end with `unshare`; it
//! keeps a record of its own ("theirs") and serves it in its person's
//! browser (`share_listener`, the receiving side).
//!
//! The receiving side opens one stream per browser connection on the
//! connection it dialed for the pipe (`stream`), below the pipe's frames;
//! this side takes it in `serve`, told apart from a frame by its first
//! byte. Every stream is checked afresh, nothing cached: the share is this
//! side's and still reaches that pipe, the pipe is open with no account
//! change held, and the stream comes from the key pinned for it when it
//! was offered. Then it connects only to 127.0.0.1 or [::1] at the share's
//! port, never one of Claktory's own, and copies bytes both ways at a rate
//! per share and peer (`DIRECT_RATE`, `RELAYED_RATE`, `MOST_STREAMS` at
//! once), checked again every `RECHECK` and cut at once by Stop, a pause or
//! a close. A paused pipe's share waits and works again on resume, with
//! the same pinned key (Fugo 30940); a pipe closed, another key pinned, or
//! another account on the other side ends it on that pipe for good.
//!
//! Priority orders a share's streams below the pipe's frames only on the
//! connection the receiving side dialed; the sharer's own messages travel
//! on the connection it dialed, so the rate is what keeps them flowing.
//! Only the person shares, stops, opens or hides a share: the requests
//! come from their app, never an agent's tool. Logged with share, peer,
//! port and bytes, never what was carried.

use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use factory_protocol::v1;
use iroh::EndpointId;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use rusqlite::{OptionalExtension, params};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

use factory_agent::text::hidden;
use crate::locking::lock;
use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;

pub(crate) const SCHEMA: &str = "
-- Websites shared over pipes (pipes/shares.rs). Mine: a port on this
-- computer the person shares, with chosen pipes or a group's (grp). Theirs:
-- one another Claktory shares with this one over `pipe`, by its id there
-- (`remote`), from the key pinned for the pipe when it came (`peer`).
CREATE TABLE pipe_shares (
    id TEXT PRIMARY KEY,
    mine INTEGER NOT NULL,
    name TEXT NOT NULL,
    port INTEGER NOT NULL,
    grp TEXT NOT NULL DEFAULT '',
    created INTEGER NOT NULL,
    pipe TEXT NOT NULL DEFAULT '',
    remote TEXT NOT NULL DEFAULT '',
    peer TEXT NOT NULL DEFAULT ''
);
-- Each pipe a share of mine was offered to, with the key pinned for it
-- then; ended: ended there for good (another key or account, or a
-- Claktory too old), never offered there again.
CREATE TABLE pipe_share_pipes (
    share TEXT NOT NULL,
    pipe TEXT NOT NULL,
    peer TEXT NOT NULL,
    ended INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (share, pipe)
);
";

/// Connections at once per share and peer.
const MOST_STREAMS: usize = 64;
/// Bytes a second per share and peer, each way, over a direct path and a
/// relay: above most home uplinks, so a dashboard never notices, and a
/// looping one never swamps the pipe.
const DIRECT_RATE: f64 = 25.0 * 1024.0 * 1024.0;
const RELAYED_RATE: f64 = 5.0 * 1024.0 * 1024.0;
/// How often a live stream checks again that it may go on.
const RECHECK: Duration = Duration::from_secs(2);
/// How long connecting to the port may take.
const CONNECT: Duration = Duration::from_secs(3);
/// How long the other side may take to say which share, and to answer.
const HEADER: Duration = Duration::from_secs(10);
/// The longest header: "share " and an id.
const HEADER_MAX: usize = 80;
/// Claktory's own ports, the real one's and the dev one's, whatever this
/// one listens on.
const OWN_PORTS: [u16; 2] = [7420, 7421];
/// A share's name, at most.
const NAME_MAX: usize = 80;
/// Websites another Claktory may share over one pipe: past it, more are
/// dropped (Fugo 31072).
const MOST_THEIRS: usize = 32;

/// What sharing keeps while Factory runs: the streams it serves, and each
/// share and peer's rate.
#[derive(Default)]
pub(crate) struct State {
    live: Mutex<HashMap<u64, Live>>,
    next: AtomicU64,
    gauges: Mutex<HashMap<(String, String), Arc<Gauge>>>,
}

/// A stream this side serves, cut at once when its share or pipe stops.
struct Live {
    share: String,
    pipe: String,
    cut: CancellationToken,
}

/// One share and peer's connections and rate.
struct Gauge {
    streams: Arc<tokio::sync::Semaphore>,
    up: Bucket,
    down: Bucket,
}

impl Default for Gauge {
    fn default() -> Gauge {
        Gauge { streams: Arc::new(tokio::sync::Semaphore::new(MOST_STREAMS)), up: Bucket::default(), down: Bucket::default() }
    }
}

/// A token bucket of bytes, holding half a second's worth.
#[derive(Default)]
struct Bucket {
    state: tokio::sync::Mutex<Option<(f64, tokio::time::Instant)>>,
}

impl Bucket {
    /// Waits until `bytes` may go at `rate` bytes a second.
    async fn take(&self, bytes: usize, rate: f64) {
        let mut state = self.state.lock().await;
        let now = tokio::time::Instant::now();
        let cap = rate / 2.0;
        let (tokens, then) = state.unwrap_or((cap, now));
        let tokens = (tokens + now.duration_since(then).as_secs_f64() * rate).min(cap) - bytes as f64;
        if tokens < 0.0 {
            tokio::time::sleep(Duration::from_secs_f64(-tokens / rate)).await;
            *state = Some((0.0, tokio::time::Instant::now()));
        } else {
            *state = Some((tokens, now));
        }
    }
}

/// A share as kept.
#[derive(Clone, Debug, Default)]
struct Share {
    id: String,
    mine: bool,
    name: String,
    port: u16,
    group: String,
    created: i64,
    pipe: String,
    remote: String,
    peer: String,
}

const COLUMNS: &str = "id, mine, name, port, grp, created, pipe, remote, peer";

impl Share {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Share> {
        Ok(Share {
            id: row.get(0)?,
            mine: row.get(1)?,
            name: row.get(2)?,
            port: row.get(3)?,
            group: row.get(4)?,
            created: row.get(5)?,
            pipe: row.get(6)?,
            remote: row.get(7)?,
            peer: row.get(8)?,
        })
    }
}

/// A pipe a share of mine reaches, as offered.
#[derive(Clone, Debug)]
struct Reach {
    pipe: String,
    peer: String,
    ended: bool,
}

fn all(factory: &Factory) -> Vec<Share> {
    let db = factory.store.db();
    let read = || -> rusqlite::Result<Vec<Share>> {
        let mut statement = db.prepare(&format!("SELECT {COLUMNS} FROM pipe_shares ORDER BY created, id"))?;
        statement.query_map([], Share::read)?.collect()
    };
    read().unwrap_or_else(|error| {
        tracing::error!(%error, "could not read the shared websites");
        Vec::new()
    })
}

fn get(factory: &Factory, id: &str) -> Option<Share> {
    let db = factory.store.db();
    db.query_row(&format!("SELECT {COLUMNS} FROM pipe_shares WHERE id = ?1"), [id], Share::read).optional().unwrap_or_else(|error| {
        tracing::error!(%error, "could not read a shared website");
        None
    })
}

fn reaches(factory: &Factory, share: &str) -> Vec<Reach> {
    let db = factory.store.db();
    let read = || -> rusqlite::Result<Vec<Reach>> {
        let mut statement = db.prepare("SELECT pipe, peer, ended FROM pipe_share_pipes WHERE share = ?1 ORDER BY pipe")?;
        statement.query_map([share], |row| Ok(Reach { pipe: row.get(0)?, peer: row.get(1)?, ended: row.get(2)? }))?.collect()
    };
    read().unwrap_or_else(|error| {
        tracing::error!(%error, "could not read where a website is shared");
        Vec::new()
    })
}

fn execute(factory: &Factory, sql: &str, values: impl rusqlite::Params) {
    if let Err(error) = factory.store.db().execute(sql, values) {
        tracing::error!(%error, "could not save a shared website");
    }
}

fn changed(factory: &Factory) {
    factory.broadcast_workspace(&factory.lock());
}

fn random_id() -> String {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).expect("the system's random source");
    crate::encoding::hex(&random)
}

/// Whether `pipe` lets a share through now, toward the key `peer`: open,
/// no account change held, and that key still pinned.
fn pipe_live(factory: &Factory, pipe: &str, peer: &str) -> bool {
    factory.pipes.get(pipe).is_some_and(|record| record.state == super::State::Open)
        && !super::identity::changed_account(factory, pipe)
        && crate::pipe_link::pinned(factory, pipe).is_some_and(|pinned| pinned == peer)
}

/// Who's on `pipe`'s other side, for sentences.
fn who(factory: &Factory, pipe: &str) -> String {
    factory.pipes.get(pipe).map_or_else(|| "The other side".to_owned(), |record| record.peer_who())
}

// ---- Sharing (mine) ----

/// Why `port` can't be shared, in a sentence; None when it can.
fn port_problem(factory: &Factory, port: u32) -> Option<String> {
    let Some(port) = u16::try_from(port).ok().filter(|port| *port != 0) else {
        return Some("Choose a port from 1 to 65535.".to_owned());
    };
    if OWN_PORTS.contains(&port) || factory.remote.listener_port() == Some(port) {
        return Some(format!("Port {port} is Claktory's own."));
    }
    if super::share_listener::serves(factory, port) {
        return Some(format!("Port {port} is where this computer opens a website shared with it."));
    }
    None
}

/// CheckPipeSharePort: whether something answers on the port, and why it
/// can't be shared if it can't.
pub(crate) async fn check_port(factory: &Factory, request: &v1::CheckPipeSharePort) -> v1::PipeSharePort {
    let problem = port_problem(factory, request.port).unwrap_or_default();
    let listening = match u16::try_from(request.port) {
        Ok(port) if port != 0 => connect(port).await.is_some(),
        _ => false,
    };
    v1::PipeSharePort { port: request.port, listening, problem }
}

/// A share's name as both sides show it: spaces, accents and every other
/// letter kept, but no control characters, nor the ones that show nothing or
/// change how what's around them shows (bidi controls, zero widths, fillers,
/// tag characters: hidden text an agent reading the chat line would still
/// read); runs of whitespace become one space.
fn shown(name: &str) -> String {
    let name: String = name.chars().map(|char| if char.is_control() { ' ' } else { char }).filter(|char| !hidden(*char)).collect();
    name.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// SharePipeWebsite: the person shares a port with pipes, or a group's.
pub(crate) fn share(factory: &Arc<Factory>, request: &v1::SharePipeWebsite) -> Result<(), RequestError> {
    let refuse = |why: String| Err(RequestError::Command(why));
    if let Some(why) = port_problem(factory, request.port) {
        return refuse(why);
    }
    let name = shown(&request.name);
    if name.is_empty() {
        return refuse("Give it a name: what the other side sees it called.".to_owned());
    }
    if name.chars().count() > NAME_MAX {
        return refuse(format!("A name can be at most {NAME_MAX} characters."));
    }
    let (group, pipes) = if request.group_id.is_empty() {
        if request.pipe_ids.is_empty() {
            return refuse("Choose the pipes to share it with, or a group.".to_owned());
        }
        let mut pipes = Vec::new();
        for id in &request.pipe_ids {
            let Some(record) = factory.pipes.get(id) else { return refuse("One of those pipes is gone.".to_owned()) };
            if record.state != super::State::Open {
                return refuse(format!("{} isn't open, so it can't get a website.", record.name));
            }
            if !pipes.contains(&record.id) {
                pipes.push(record.id);
            }
        }
        (String::new(), pipes)
    } else {
        let Some(group) = super::groups::get(factory, &request.group_id) else {
            return refuse("That group is gone.".to_owned());
        };
        let members = super::groups::members(factory);
        let pipes = members.into_iter().filter(|(_, group_id)| *group_id == group.id).map(|(pipe, _)| pipe).collect();
        (group.id, pipes)
    };
    let share = Share {
        id: random_id(),
        mine: true,
        name,
        port: u16::try_from(request.port).unwrap_or_default(),
        group,
        created: now_ms(),
        ..Share::default()
    };
    execute(
        factory,
        "INSERT INTO pipe_shares (id, mine, name, port, grp, created) VALUES (?1, 1, ?2, ?3, ?4, ?5)",
        params![share.id, share.name, share.port, share.group, share.created],
    );
    let mut reached = 0;
    for pipe in &pipes {
        reached += usize::from(offer(factory, &share, pipe));
    }
    tracing::info!(share = %share.id, port = share.port, pipes = reached, "shared a website");
    changed(factory);
    Ok(())
}

/// Offers `share` on `pipe`, if that pipe can take it now (open or paused,
/// a key pinned) and it never ended there; tells the pipe's chat.
fn offer(factory: &Arc<Factory>, share: &Share, pipe: &str) -> bool {
    let Some(record) = factory.pipes.get(pipe) else { return false };
    if !matches!(record.state, super::State::Open | super::State::Paused) {
        return false;
    }
    let Some(peer) = crate::pipe_link::pinned(factory, pipe) else { return false };
    let known = factory
        .store
        .db()
        .query_row("SELECT 1 FROM pipe_share_pipes WHERE share = ?1 AND pipe = ?2", params![share.id, pipe], |_| Ok(()))
        .optional()
        .ok()
        .flatten()
        .is_some();
    if known {
        return false;
    }
    if let Err(error) = crate::pipe_link::share(factory, pipe, &share.id, Some((&share.name, share.port))) {
        tracing::error!(%error, "could not offer a website on a pipe");
        return false;
    }
    execute(factory, "INSERT INTO pipe_share_pipes (share, pipe, peer) VALUES (?1, ?2, ?3)", params![share.id, pipe, peer]);
    super::line(factory, pipe, format!("You shared {} (port {}).", share.name, share.port));
    true
}

/// Takes `share` back from `pipe`: the other side is told, its streams
/// end, the pipe's chat says `said`. `for_good` keeps it from being offered
/// there again (`ended`); else its row goes.
fn withdraw(factory: &Arc<Factory>, share: &Share, pipe: &str, said: Option<String>, for_good: bool) {
    if let Err(error) = crate::pipe_link::share(factory, pipe, &share.id, None) {
        tracing::debug!(%error, "could not tell a pipe a website stopped");
    }
    if for_good {
        execute(factory, "UPDATE pipe_share_pipes SET ended = 1 WHERE share = ?1 AND pipe = ?2", params![share.id, pipe]);
    } else {
        execute(factory, "DELETE FROM pipe_share_pipes WHERE share = ?1 AND pipe = ?2", params![share.id, pipe]);
    }
    cut(factory, |live| live.share == share.id && live.pipe == pipe);
    if let Some(said) = said {
        super::line(factory, pipe, said);
    }
}

/// StopPipeShare: the person stops a share of theirs on every pipe.
pub(crate) fn stop(factory: &Arc<Factory>, request: &v1::StopPipeShare) -> Result<(), RequestError> {
    let Some(share) = get(factory, &request.share_id).filter(|share| share.mine) else {
        return Err(RequestError::Command("That website isn't shared any more.".to_owned()));
    };
    for reach in reaches(factory, &share.id) {
        let said = (!reach.ended).then(|| format!("You stopped sharing {}.", share.name));
        withdraw(factory, &share, &reach.pipe, said, false);
    }
    forget(factory, &share.id);
    tracing::info!(share = %share.id, port = share.port, "stopped sharing a website");
    changed(factory);
    Ok(())
}

fn forget(factory: &Factory, share: &str) {
    execute(factory, "DELETE FROM pipe_share_pipes WHERE share = ?1", [share]);
    execute(factory, "DELETE FROM pipe_shares WHERE id = ?1", [share]);
    cut(factory, |live| live.share == share);
}

/// Ends the streams `which` picks, at once.
fn cut(factory: &Factory, which: impl Fn(&Live) -> bool) {
    lock(&factory.pipe_shares.live).retain(|_, live| {
        if which(live) {
            live.cut.cancel();
            false
        } else {
            true
        }
    });
}

/// The groups' shares a pipe gets and stops getting as it moves from
/// `from` to `to` (group ids, empty for none): the person's move (or an
/// agent's they approved on a card).
pub(crate) fn moved(factory: &Arc<Factory>, pipe: &str, from: &str, to: &str) {
    if from == to {
        return;
    }
    let shares = all(factory);
    let mut any = false;
    for share in shares.iter().filter(|share| share.mine && !share.group.is_empty() && share.group == from) {
        let reached = reaches(factory, &share.id).into_iter().any(|reach| reach.pipe == pipe && !reach.ended);
        if reached {
            withdraw(factory, share, pipe, Some(format!("You stopped sharing {} here: the pipe left its group.", share.name)), false);
            any = true;
        }
    }
    for share in shares.iter().filter(|share| share.mine && !share.group.is_empty() && share.group == to) {
        any |= offer(factory, share, pipe);
    }
    if any {
        changed(factory);
    }
}

/// The websites a pipe would get by moving into `group` (id, name), for a
/// card when an agent moves it.
pub(crate) fn of_group(factory: &Factory, group: &str) -> Vec<(String, String)> {
    all(factory)
        .into_iter()
        .filter(|share| share.mine && share.group == group && !group.is_empty())
        .map(|share| (share.id, share.name))
        .collect()
}

/// The names of the websites of this side's that reach `pipe` now.
pub(crate) fn reaching(factory: &Factory, pipe: &str) -> Vec<String> {
    all(factory)
        .into_iter()
        .filter(|share| share.mine && reaches(factory, &share.id).iter().any(|reach| reach.pipe == pipe && !reach.ended))
        .map(|share| share.name)
        .collect()
}

/// A deleted group's shares stay with the pipes they reach now.
pub(crate) fn group_deleted(factory: &Factory, group: &str) {
    if group.is_empty() {
        return;
    }
    execute(factory, "UPDATE pipe_shares SET grp = '' WHERE mine = 1 AND grp = ?1", [group]);
}

// ---- A pipe's life ----

/// A pipe paused (either side): its shares' streams end now; they work
/// again on resume.
pub(crate) fn paused(factory: &Arc<Factory>, pipe: &str) {
    cut(factory, |live| live.pipe == pipe);
    for share in all(factory).into_iter().filter(|share| !share.mine && share.pipe == pipe) {
        // The listener closes, its address, port and cookies kept.
        super::share_listener::close(factory, &share.id);
    }
    changed(factory);
}

/// A pipe resumed: its shares work again, if the key pinned is the one
/// they were offered to and no account change is held; else they end
/// there for good.
pub(crate) fn resumed(factory: &Arc<Factory>, pipe: &str) {
    let pinned = crate::pipe_link::pinned(factory, pipe).unwrap_or_default();
    let held = super::identity::changed_account(factory, pipe);
    for share in all(factory) {
        if share.mine {
            for reach in reaches(factory, &share.id).into_iter().filter(|reach| reach.pipe == pipe && !reach.ended) {
                if held {
                    let said = format!("{} stopped here: this pipe's Claktory now belongs to a different account.", share.name);
                    withdraw(factory, &share, pipe, Some(said), true);
                } else if reach.peer != pinned {
                    let said = format!("{} stopped here: the other side isn't the one it was shared with.", share.name);
                    withdraw(factory, &share, pipe, Some(said), true);
                }
            }
        } else if share.pipe == pipe {
            if held {
                let said = format!("{} stopped here: this pipe's Claktory now belongs to a different account.", share.name);
                end_theirs(factory, &share, Some(said));
            } else if share.peer != pinned {
                end_theirs(factory, &share, Some(format!("{} stopped here: the other side isn't the one that shared it.", share.name)));
            } else {
                // Listening again at the same address, if it had been opened.
                super::share_listener::resumed(factory, &share.id);
            }
        }
    }
    changed(factory);
}

/// A pipe closed (either side) or forgotten: its shares end there.
pub(crate) fn closed(factory: &Arc<Factory>, pipe: &str) {
    let mut any = false;
    for share in all(factory) {
        if share.mine {
            let reached = reaches(factory, &share.id);
            if !reached.iter().any(|reach| reach.pipe == pipe) {
                continue;
            }
            execute(factory, "DELETE FROM pipe_share_pipes WHERE share = ?1 AND pipe = ?2", params![share.id, pipe]);
            cut(factory, |live| live.share == share.id && live.pipe == pipe);
            // A share with chosen pipes and none left is over.
            if share.group.is_empty() && reached.iter().all(|reach| reach.pipe == pipe) {
                forget(factory, &share.id);
            }
            any = true;
        } else if share.pipe == pipe {
            end_theirs(factory, &share, None);
            any = true;
        }
    }
    if any {
        changed(factory);
    }
}

/// What the other side says it is was heard: if it's another account now
/// (held), the shares on that pipe end for good.
pub(crate) fn identity_heard(factory: &Arc<Factory>, pipe: &str) {
    if !super::identity::changed_account(factory, pipe) {
        return;
    }
    let mut any = false;
    for share in all(factory) {
        if share.mine {
            if reaches(factory, &share.id).iter().any(|reach| reach.pipe == pipe && !reach.ended) {
                let said = format!("{} stopped here: this pipe's Claktory now belongs to a different account.", share.name);
                withdraw(factory, &share, pipe, Some(said), true);
                any = true;
            }
        } else if share.pipe == pipe {
            end_theirs(factory, &share, Some(format!("{} stopped here: this pipe's Claktory now belongs to a different account.", share.name)));
            any = true;
        }
    }
    if any {
        changed(factory);
    }
}

/// A Claktory from before took the offer as something it can't read: it
/// ends there, said once.
pub(crate) fn too_old(factory: &Arc<Factory>, pipe: &str, id: &str) {
    let Some(share) = get(factory, id).filter(|share| share.mine) else { return };
    execute(factory, "UPDATE pipe_share_pipes SET ended = 1 WHERE share = ?1 AND pipe = ?2", params![id, pipe]);
    super::line(factory, pipe, format!("Their Claktory is too old to open shared websites, so {} isn't shared here.", share.name));
    changed(factory);
}

// ---- Shared with this side (theirs) ----

/// The other side offered a website over `pipe` (`peer`, its pinned key).
pub(crate) fn offered(factory: &Arc<Factory>, pipe: &str, peer: &str, remote: &str, name: &str, port: u16) {
    if !id_like(remote) {
        return;
    }
    let name: String = shown(name).chars().take(NAME_MAX).collect::<String>().trim_end().to_owned();
    let name = if name.is_empty() { "A website".to_owned() } else { name };
    let theirs: Vec<Share> = all(factory).into_iter().filter(|share| !share.mine && share.pipe == pipe).collect();
    let known = theirs.iter().find(|share| share.remote == remote);
    if known.is_none() && theirs.len() >= MOST_THEIRS {
        tracing::warn!(pipe, "the other side shares more websites than this side keeps; dropped one");
        return;
    }
    if let Some(known) = known {
        execute(factory, "UPDATE pipe_shares SET name = ?2, port = ?3 WHERE id = ?1", params![known.id, name, port]);
    } else {
        execute(
            factory,
            "INSERT INTO pipe_shares (id, mine, name, port, created, pipe, remote, peer) VALUES (?1, 0, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![random_id(), name, port, now_ms(), pipe, remote, peer],
        );
        super::line(factory, pipe, format!("{} shares {name}.", who(factory, pipe)));
    }
    changed(factory);
}

/// The other side stopped sharing a website over `pipe`.
pub(crate) fn withdrawn(factory: &Arc<Factory>, pipe: &str, remote: &str) {
    let Some(share) = all(factory).into_iter().find(|share| !share.mine && share.pipe == pipe && share.remote == remote) else {
        return;
    };
    end_theirs(factory, &share, Some(format!("{} stopped sharing {}.", who(factory, pipe), share.name)));
    changed(factory);
}

/// A share of theirs ends for good: its listener and connections close,
/// its address is never used again, its record goes.
fn end_theirs(factory: &Factory, share: &Share, said: Option<String>) {
    // The listener and every live connection close; the address and port
    // never go to another peer.
    super::share_listener::end(factory, &share.id);
    execute(factory, "DELETE FROM pipe_shares WHERE id = ?1", [&share.id]);
    if let Some(said) = said {
        super::line(factory, &share.pipe, said);
    }
}

fn id_like(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// A website another Claktory shares with this one, as the receiving side
/// (`share_listener`) reads it.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct Received {
    /// This side's id for it.
    pub id: String,
    /// The pipe it came over.
    pub pipe: String,
    /// The key pinned for that pipe when it came.
    pub peer_key: String,
    /// Who shares it, for sentences: "Ana", else their Claktory.
    pub sharer_name: String,
    /// What it's called: the other side's words.
    pub name: String,
    /// The sharer's port, which this side uses for its own when it's free.
    pub port: u16,
}

fn received_of(factory: &Factory, share: Share) -> Received {
    Received { sharer_name: who(factory, &share.pipe), id: share.id, pipe: share.pipe, peer_key: share.peer, name: share.name, port: share.port }
}

/// The share of theirs with this id, while it can be used: its pipe open,
/// with no account change held and the same key pinned.
pub(crate) fn received(factory: &Factory, id: &str) -> Option<Received> {
    let share = get(factory, id).filter(|share| !share.mine)?;
    pipe_live(factory, &share.pipe, &share.peer).then(|| received_of(factory, share))
}

/// Every share of theirs that can be used now.
#[allow(dead_code)]
pub(crate) fn received_all(factory: &Factory) -> Vec<Received> {
    all(factory)
        .into_iter()
        .filter(|share| !share.mine && pipe_live(factory, &share.pipe, &share.peer))
        .map(|share| received_of(factory, share))
        .collect()
}

/// Opens a stream to a website another Claktory shares with this one: on
/// the pipe's connection, below its frames, asked for by the sharer's id.
/// Err says why for the person.
pub(crate) async fn stream(factory: &Arc<Factory>, id: &str) -> Result<(SendStream, RecvStream), String> {
    let share = get(factory, id).filter(|share| !share.mine).ok_or("This website isn't shared with you any more.")?;
    if !pipe_live(factory, &share.pipe, &share.peer) {
        return Err(format!("{} can't be reached now: its pipe isn't open.", share.name));
    }
    ask(factory, &share.pipe, &share.remote).await.map_err(|why| why.unwrap_or_else(|| format!("{} didn't answer.", share.name)))
}

/// Asks the other side of `pipe` for its website `remote`: the stream, or
/// what it said (None: nothing in time).
async fn ask(factory: &Arc<Factory>, pipe: &str, remote: &str) -> Result<(SendStream, RecvStream), Option<String>> {
    let (mut send, mut receive) = crate::pipe_link::share_stream(factory, pipe).await.map_err(Some)?;
    let header = format!("share {remote}\n");
    send.write_all(header.as_bytes()).await.map_err(|error| Some(error.to_string()))?;
    match tokio::time::timeout(HEADER, line(&mut receive)).await {
        Ok(Some(answer)) if answer == "ok" => Ok((send, receive)),
        Ok(Some(answer)) => Err(Some(answer.strip_prefix("no ").unwrap_or("The other side turned it away.").to_owned())),
        _ => Err(None),
    }
}

// ---- Serving (mine) ----

/// A line, read a byte at a time so nothing after it is taken; at most
/// `HEADER_MAX` bytes before the newline, or None.
async fn line(receive: &mut RecvStream) -> Option<String> {
    let mut line = Vec::new();
    loop {
        let byte = receive.read_u8().await.ok()?;
        if byte == b'\n' {
            break;
        }
        line.push(byte);
        if line.len() > HEADER_MAX + 200 {
            return None;
        }
    }
    String::from_utf8(line).ok()
}

/// Where a stream for `id` from `peer` may go now: the share, and the pipe
/// it reaches that key over; else why not, for the other side.
fn allowed(factory: &Factory, id: &str, peer: &str) -> Result<(Share, String), String> {
    let gone = || "This website isn't shared with you now.".to_owned();
    let share = get(factory, id).filter(|share| share.mine).ok_or_else(gone)?;
    let pipe = reaches(factory, &share.id)
        .into_iter()
        .find(|reach| !reach.ended && reach.peer == peer && pipe_live(factory, &reach.pipe, peer))
        .ok_or_else(gone)?
        .pipe;
    if let Some(why) = port_problem(factory, u32::from(share.port)) {
        return Err(why);
    }
    Ok((share, pipe))
}

/// Serves one stream from a pinned peer, its first byte ("s") read
/// already: `hare <id>\n`, then the share's bytes both ways.
pub(crate) async fn serve(factory: Weak<Factory>, connection: &Connection, peer: EndpointId, mut send: SendStream, mut receive: RecvStream) {
    let header = tokio::time::timeout(HEADER, line(&mut receive)).await.ok().flatten();
    let id = header.as_deref().and_then(|header| header.strip_prefix("hare ")).map(str::trim).filter(|id| id.len() <= HEADER_MAX);
    let Some(id) = id.map(str::to_owned) else { return no(send, "That isn't a website's stream.").await };
    let Some(strong) = factory.upgrade() else { return };
    let key = peer.to_string();
    let (share, pipe) = match allowed(&strong, &id, &key) {
        Ok(found) => found,
        Err(why) => {
            tracing::info!(share = %id, endpoint = %key, %why, "refused a shared website's stream");
            return no(send, &why).await;
        }
    };
    let gauge = lock(&strong.pipe_shares.gauges).entry((share.id.clone(), key.clone())).or_default().clone();
    let Ok(_stream) = gauge.streams.clone().try_acquire_owned() else {
        return no(send, &format!("Too many connections to {} at once.", share.name)).await;
    };
    drop(strong);
    let Some(socket) = connect(share.port).await else {
        tracing::info!(share = %share.id, port = share.port, "a shared website: nothing listens there");
        return no(send, &format!("{} isn't answering on the other side.", share.name)).await;
    };
    let _ = send.set_priority(-1);
    if send.write_all(b"ok\n").await.is_err() {
        return;
    }
    let Some(strong) = factory.upgrade() else { return };
    let cut = CancellationToken::new();
    let number = strong.pipe_shares.next.fetch_add(1, Ordering::Relaxed);
    lock(&strong.pipe_shares.live).insert(number, Live { share: share.id.clone(), pipe: pipe.clone(), cut: cut.clone() });
    drop(strong);
    tracing::info!(share = %share.id, endpoint = %key, port = share.port, "serving a shared website");
    let rate = if relayed(connection) { RELAYED_RATE } else { DIRECT_RATE };
    let (mut read, mut write) = socket.into_split();
    let up = async {
        let copied = copy(&mut receive, &mut write, &gauge.up, rate).await;
        let _ = write.shutdown().await;
        copied
    };
    let down = async {
        let copied = copy(&mut read, &mut send, &gauge.down, rate).await;
        let _ = send.finish();
        copied
    };
    // Checked again while it lasts, nothing cached: cut the moment it may
    // not go on.
    let watch = async {
        loop {
            tokio::select! {
                () = cut.cancelled() => return,
                () = tokio::time::sleep(RECHECK) => {}
            }
            let Some(strong) = factory.upgrade() else { return };
            if allowed(&strong, &share.id, &key).is_err() {
                return;
            }
        }
    };
    let (bytes_in, bytes_out) = tokio::select! {
        (up, down) = async { tokio::join!(up, down) } => (up, down),
        () = watch => {
            let _ = send.reset(0u32.into());
            let _ = receive.stop(0u32.into());
            (0, 0)
        }
    };
    if let Some(strong) = factory.upgrade() {
        lock(&strong.pipe_shares.live).remove(&number);
    }
    tracing::info!(share = %share.id, endpoint = %key, port = share.port, bytes_in, bytes_out, "a shared website's stream ended");
}

/// Copies `from` to `to` at `rate` (`bucket`), until `from` ends or either
/// fails; the bytes copied.
async fn copy(from: &mut (impl AsyncReadExt + Unpin), to: &mut (impl AsyncWriteExt + Unpin), bucket: &Bucket, rate: f64) -> u64 {
    let mut buffer = vec![0u8; 16 * 1024];
    let mut copied = 0u64;
    loop {
        let read = match from.read(&mut buffer).await {
            Ok(0) | Err(_) => return copied,
            Ok(read) => read,
        };
        bucket.take(read, rate).await;
        if to.write_all(&buffer[..read]).await.is_err() {
            return copied;
        }
        copied += read as u64;
    }
}

/// Whether the connection travels through a relay now.
fn relayed(connection: &Connection) -> bool {
    let paths = connection.paths();
    let path = paths.iter().find(|path| path.is_selected()).or_else(|| paths.iter().next());
    path.is_some_and(|path| matches!(path.remote_addr(), iroh::TransportAddr::Relay(_)))
}

async fn no(mut send: SendStream, why: &str) {
    let line = format!("no {}\n", why.replace('\n', " "));
    let _ = send.write_all(line.as_bytes()).await;
    let _ = send.finish();
}

/// What listens on `port`, on 127.0.0.1 or else [::1]: never another host.
async fn connect(port: u16) -> Option<TcpStream> {
    for ip in [Ipv4Addr::LOCALHOST.into(), Ipv6Addr::LOCALHOST.into()] {
        if let Ok(Ok(socket)) = tokio::time::timeout(CONNECT, TcpStream::connect(SocketAddr::new(ip, port))).await {
            let _ = socket.set_nodelay(true);
            return Some(socket);
        }
    }
    None
}

// ---- What the app shows ----

/// Workspace.pipe_shares: mine with the pipes each reaches now, and theirs.
pub(crate) fn view(factory: &Factory) -> Vec<v1::PipeShare> {
    all(factory)
        .into_iter()
        .map(|share| {
            if share.mine {
                let reached: Vec<Reach> = reaches(factory, &share.id).into_iter().filter(|reach| !reach.ended).collect();
                let live = reached.iter().any(|reach| pipe_live(factory, &reach.pipe, &reach.peer));
                v1::PipeShare {
                    id: share.id,
                    mine: true,
                    name: share.name,
                    port: u32::from(share.port),
                    pipe_ids: reached.into_iter().map(|reach| reach.pipe).collect(),
                    group_id: share.group,
                    created_ms: share.created,
                    live,
                    ..v1::PipeShare::default()
                }
            } else {
                let live = pipe_live(factory, &share.pipe, &share.peer);
                let status = super::share_listener::status(factory, &share.id, &share.peer, share.port);
                v1::PipeShare {
                    id: share.id,
                    mine: false,
                    name: share.name,
                    port: u32::from(share.port),
                    pipe_ids: vec![share.pipe],
                    created_ms: share.created,
                    live,
                    address: status.address,
                    fallback_address: status.fallback_address,
                    listening: status.listening,
                    listener_problem: status.problem,
                    hidden: status.hidden,
                    ..v1::PipeShare::default()
                }
            }
        })
        .collect()
}

/// At start: shares live in factory.db, so nothing to load; the shares of
/// theirs opened before listen again, so their addresses keep working.
pub(crate) fn start(factory: &Arc<Factory>) {
    super::share_listener::restore(factory);
}

/// Tests: a stream to a website shared with this side, as the receiving
/// side opens one.
#[doc(hidden)]
pub async fn stream_for_test(factory: &Arc<Factory>, id: &str) -> Result<(SendStream, RecvStream), String> {
    stream(factory, id).await
}

/// Tests: asks `pipe`'s other side for its website `remote` (its id there),
/// shared with this side or not; what it answered.
#[doc(hidden)]
pub async fn ask_for_test(factory: &Arc<Factory>, pipe: &str, remote: &str) -> Result<(), String> {
    ask(factory, pipe, remote).await.map(drop).map_err(|why| why.unwrap_or_else(|| "no answer".to_owned()))
}

/// Tests: the other side of `pipe` (pinned to `peer`) offers a website.
#[doc(hidden)]
pub fn offered_for_test(factory: &Arc<Factory>, pipe: &str, peer: &str, remote: &str, name: &str) {
    offered(factory, pipe, peer, remote, name, 3000);
}

/// Tests: what the other side says it is was heard, as the transport
/// tells this module.
#[doc(hidden)]
pub fn identity_heard_for_test(factory: &Arc<Factory>, pipe: &str) {
    identity_heard(factory, pipe);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn a_bucket_holds_bytes_to_its_rate() {
        let bucket = Bucket::default();
        let rate = 1_000_000.0;
        let start = tokio::time::Instant::now();
        // Half a second's worth goes at once; the next second's waits.
        bucket.take(500_000, rate).await;
        assert!(start.elapsed() < Duration::from_millis(10));
        bucket.take(1_000_000, rate).await;
        let took = start.elapsed();
        assert!(took >= Duration::from_millis(990) && took < Duration::from_millis(1100), "{took:?}");
    }

    #[test]
    fn only_a_shares_id_is_taken_from_the_other_side() {
        assert!(id_like("0123abcdef"));
        for id in ["", "../etc", "share 1", &"a".repeat(65)] {
            assert!(!id_like(id), "{id}");
        }
    }

    #[test]
    fn a_shares_name_keeps_its_spaces_and_letters() {
        assert_eq!(shown("QA Vite"), "QA Vite");
        assert_eq!(shown("Café board — Ünïcode ok"), "Café board — Ünïcode ok");
        assert_eq!(shown("  two\n\tlines\u{0}  "), "two lines");
        assert_eq!(shown("\u{202E}etiV\u{2066}x\u{2069}\u{200B}\u{FEFF}"), "etiVx");
        assert_eq!(shown("ab\u{61C}c\u{AD}d\u{115F}\u{3164}e"), "abcde");
        let tagged: String = "Tag".chars().chain("evil".chars().map(|c| char::from_u32(0xE0000 + u32::from(c)).unwrap())).chain("soft".chars()).collect();
        assert_eq!(shown(&tagged), "Tagsoft");
        assert_eq!(shown("日本語 サイト 🚀"), "日本語 サイト 🚀");
    }
}
