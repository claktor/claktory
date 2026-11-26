//! The leaf's loop (dms-e2ee.md §5.2): while this Claktory's leaf is
//! enrolled, it keeps its KeyPackages stocked at the worker, joins the
//! groups it's welcomed to, and processes every group it's in, in order,
//! each message in one step (its MLS state, its history row and the group's
//! position together). MLS state only moves forward through every commit
//! and message, so every group is processed, not only followed ones.
//!
//! - Woken by the worker's feed for this leaf (`GET claktory/events`):
//!   ids only, one long poll at a time; Welcomes and group changes come
//!   that way. A group this Claktory's home holds for the leaf (#451) is
//!   read from the home instead of the worker, woken by what arrives there;
//!   any other still from the worker.
//! - **Paused, never skipping,** while the vault is locked or the person's
//!   device hasn't enrolled it: positions don't move, and the 30-day mailbox
//!   covers the wait.
//! - A group the worker no longer lists for it (its grant ended, the
//!   friendship gone) is forgotten here, history and all.
//! - A conversation it's too far behind in is forgotten too (a device adds
//!   it again); its own channel it rejoins by external commit.
//! - **A commit is never skipped** (R5): one naming a leaf the device list
//!   kept here doesn't (a device the person just added, its list not
//!   fetched yet) is read again once the newest list is fetched; one that
//!   stays refused under it keeps its group waiting there, sending nothing,
//!   and the channel rejoins at the current epoch if every member there
//!   checks out, at most once a round.
//! - **A channel out of step heals:** a commit from an epoch after its own
//!   that it can't take, or the worker's epoch past its own once it has read
//!   everything, means it missed a commit; it rejoins the same way.
//! - A group that reaches a commit under a security key this leaf hasn't
//!   been given (the person's new one, a friend's) waits there, unread,
//!   until a device of the person's gives it. Enrolled under the person's
//!   new key, the leaf moves to it in every group, by an Update each.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use claktory_dm_crypto::certificate::LeafKind;
use claktory_dm_crypto::content::{Content, kind};
use claktory_dm_crypto::envelope::Envelope;
use claktory_dm_crypto::group::{Group, Incoming, LAST_RESORT_RENEW, WINDOW_MOST, Window};
use claktory_dm_crypto::validation::Pins;
use serde::Deserialize;
use serde_json::{Value, json};

use super::home;
use super::leaf::{self, KeptGroup, StepError, Unready};
use super::store;
use crate::account::apps::Problem;
use crate::factory::Factory;
use crate::transcript::now_ms;

/// How long the worker may hold a call for events, and how long this side waits.
const WAIT: u64 = 25;
const CALL: Duration = Duration::from_secs(WAIT + 15);
const FEED_WAIT: Duration = Duration::from_secs(WAIT);
/// Between looks while the leaf can't run.
const IDLE: Duration = Duration::from_secs(60);
/// Between looks while the vault is locked: opening it is all it waits for,
/// and that check is in memory.
const LOCKED: Duration = Duration::from_secs(3);
/// A full round (key packages, welcomes, every group) at least this often.
const ROUND: Duration = Duration::from_secs(600);
/// KeyPackages kept at the worker: topped up to `STOCK` below `LOW`.
const STOCK: i64 = 100;
const LOW: i64 = 30;
/// Messages asked for at a time.
const PAGE: i64 = 100;
/// After rejoining the channel, the least time before it may try again, in
/// milliseconds: a round (tests set less). Doubled after each attempt that
/// fails, up to `REJOIN_MOST`.
pub(crate) static REJOIN_WAIT: AtomicU64 = AtomicU64::new(ROUND.as_millis() as u64);
const REJOIN_MOST: Duration = Duration::from_secs(3600);

/// Something changed here (an enrollment): a full round now, even in the
/// middle of waiting for the feed. A wake while the loop is busy elsewhere
/// is kept for its next wait (`notify_one`), never lost until the long poll
/// ends (row 606: a new home followed its list a minute late).
pub(crate) fn wake(factory: &Factory) {
    factory.dm_leaf.round_now.store(true, std::sync::atomic::Ordering::SeqCst);
    factory.dm_wakes.process.notify_one();
}

#[derive(Debug)]
enum Stop {
    /// The leaf can't run now: wait.
    Unready(Unready),
    /// The worker or the network: try again later.
    Problem(Problem),
    Failed(String),
}

impl From<StepError> for Stop {
    fn from(error: StepError) -> Stop {
        match error {
            StepError::Unready(unready) => Stop::Unready(unready),
            StepError::Failed(error) => Stop::Failed(error),
        }
    }
}

impl From<Problem> for Stop {
    fn from(problem: Problem) -> Stop {
        Stop::Problem(problem)
    }
}

fn unreadable(error: impl std::fmt::Display) -> Stop {
    Stop::Problem(Problem::ForNow(format!("an unreadable answer: {error}")))
}

/// The leaf's call to the worker, as this Claktory (no agent).
async fn call(factory: &Factory, method: &str, route: &str, query: &[(&str, String)], body: Option<Value>) -> Result<Vec<u8>, Problem> {
    factory.account.dms(method, route, query, None, body, None).await
}

async fn get<T: serde::de::DeserializeOwned>(factory: &Factory, route: &str, query: &[(&str, String)]) -> Result<T, Stop> {
    let answer = call(factory, "GET", route, query, None).await?;
    serde_json::from_slice(&answer).map_err(unreadable)
}

/// Notes each group the home took something for, and wakes the loop.
async fn arrivals(factory: Weak<Factory>) {
    let Some(strong) = factory.upgrade() else { return };
    let mut arrivals = super::home::arrivals(&strong);
    drop(strong);
    loop {
        let arrival = arrivals.recv().await;
        let Some(strong) = factory.upgrade() else { return };
        match arrival {
            Ok(super::home::Arrival::Entry { group_id, .. }) => {
                let mut arrived = strong.dm_leaf.arrived.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if !arrived.contains(&group_id) {
                    arrived.push(group_id);
                }
                drop(arrived);
                strong.dm_wakes.process.notify_one();
            }
            Ok(_) => {}
            // Fell behind: look at everything.
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => wake(&strong),
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// The groups the home took something for, read now.
async fn arrived(factory: &Arc<Factory>) -> Result<(), Stop> {
    let groups = std::mem::take(&mut *factory.dm_leaf.arrived.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
    for group_id in groups {
        let kept = leaf::step(factory, |step| Ok(step.groups()?.into_iter().find(|group| group.group_id == group_id)))?;
        if let Some(group) = kept {
            waiting(catch_up(factory, &group).await)?;
        }
    }
    Ok(())
}

pub(crate) async fn monitor(factory: Weak<Factory>) {
    tokio::spawn(arrivals(factory.clone()));
    crate::network::ready().await;
    let mut cursor: Option<String> = None;
    let mut last_round: Option<tokio::time::Instant> = None;
    let mut backoff = Duration::from_secs(30);
    let mut pace = super::pace::Pace::default();
    loop {
        let Some(strong) = factory.upgrade() else { return };
        let wake = strong.dm_wakes.process.clone();
        let ready = leaf::step(&strong, |step| step.leaf().map(drop));
        let pause = match ready {
            Err(StepError::Unready(Unready::Locked)) => Some(LOCKED),
            Err(StepError::Unready(_)) => Some(IDLE),
            Err(StepError::Failed(error)) => {
                tracing::warn!(%error, "the direct-message leaf can't run");
                Some(IDLE)
            }
            Ok(()) => {
                // The feed's cursor first, so nothing between it and the round is missed.
                let mut outcome = Ok(());
                let mut early = None;
                if strong.dm_leaf.round_now.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    last_round = None;
                }
                if cursor.is_none() {
                    // Without the feed the round still runs; the feed is tried again after it.
                    if let Err(stop) = events(&strong, &mut cursor).await {
                        tracing::debug!(?stop, "the direct-message feed isn't there");
                    }
                    last_round = None;
                }
                if last_round.is_none_or(|at| at.elapsed() >= ROUND) {
                    outcome = full_round(&strong).await;
                    if outcome.is_ok() {
                        last_round = Some(tokio::time::Instant::now());
                    }
                }
                if outcome.is_ok() {
                    outcome = arrived(&strong).await;
                }
                if outcome.is_ok() {
                    // A long poll once there's a cursor.
                    let held = cursor.is_some();
                    let asked = tokio::time::Instant::now();
                    let happened = tokio::select! {
                        happened = events(&strong, &mut cursor) => happened.map(Some),
                        // Woken (an enrollment): the round comes first.
                        () = wake.notified() => Ok(None),
                    };
                    outcome = match happened {
                        Ok(Some(happened)) => {
                            let nothing = held && happened.is_empty();
                            early = pace.after(super::pace::early(FEED_WAIT, asked.elapsed(), nothing), FEED_WAIT);
                            follow(&strong, happened).await
                        }
                        Ok(None) => Ok(()),
                        Err(error) => Err(error),
                    };
                }
                match outcome {
                    Ok(()) => {
                        backoff = Duration::from_secs(30);
                        // Not held at the worker: not again at once.
                        early
                    }
                    Err(Stop::Unready(Unready::Locked)) => Some(LOCKED),
                    Err(Stop::Unready(_)) => Some(IDLE),
                    Err(stop) => {
                        match &stop {
                            Stop::Failed(error) => tracing::warn!(%error, "the direct-message leaf stopped for now"),
                            stop => tracing::debug!(?stop, "the direct-message leaf stopped for now"),
                        }
                        let pause = backoff;
                        backoff = (backoff * 2).min(Duration::from_secs(300));
                        Some(pause)
                    }
                }
            }
        };
        drop(strong);
        if let Some(pause) = pause {
            tokio::select! {
                () = tokio::time::sleep(pause) => {}
                () = wake.notified() => {}
            }
        }
    }
}

#[derive(Deserialize)]
struct Events {
    #[serde(default)]
    events: Vec<Event>,
    next_cursor: String,
}

#[derive(Debug, Deserialize)]
struct Event {
    kind: String,
    #[serde(default)]
    group_id: String,
    /// A `commit`'s: the epoch it was made in.
    #[serde(default)]
    epoch: u64,
}

/// What happened since the cursor: one long poll. Without a cursor, takes
/// one and reports nothing.
async fn events(factory: &Arc<Factory>, cursor: &mut Option<String>) -> Result<Vec<Event>, Stop> {
    let query: Vec<(&str, String)> = match cursor {
        Some(since) => vec![("since", since.clone()), ("wait", WAIT.to_string())],
        None => Vec::new(),
    };
    let answer = factory.account.dms("GET", "claktory/events", &query, None, None, Some(CALL)).await;
    let answer = match answer {
        Err(Problem::Refused(410, _)) => {
            // Too far behind the feed: start again from a full round.
            *cursor = None;
            return Ok(vec![Event { kind: "groups".to_owned(), group_id: String::new(), epoch: 0 }]);
        }
        other => other?,
    };
    let events: Events = serde_json::from_slice(&answer).map_err(unreadable)?;
    let had = cursor.is_some();
    *cursor = Some(events.next_cursor);
    Ok(if had { events.events } else { Vec::new() })
}

/// Acts on the feed's events.
async fn follow(factory: &Arc<Factory>, events: Vec<Event>) -> Result<(), Stop> {
    // A commit in one of the person's groups, which the home may not have
    // carried (a device away from it committed only to claktory.com,
    // Zucchero's row 568): a hint, so the home pulls it from there and every
    // device reading the home gets it.
    if home::is_home(factory) {
        for event in events.iter().filter(|event| event.kind == "commit") {
            let Ok(group_id) = URL_SAFE_NO_PAD.decode(&event.group_id) else { continue };
            if let Err(error) = home::hint(&**factory, &group_id, event.epoch) {
                tracing::debug!(?error, "a commit's hint wasn't kept");
            }
        }
    }
    if events.iter().any(|event| event.kind == "groups") {
        return full_round(factory).await;
    }
    // The person's own signed device list changed: who's the home, and which
    // of their Claktories it serves, follow it now, not at the next round.
    if events.iter().any(|event| event.kind == "device_list") {
        refresh_keys(factory, &[]).await?;
        follow_list(factory)?;
    }
    if events.iter().any(|event| event.kind == "welcome") {
        welcomes(factory).await?;
    }
    let mut seen = Vec::new();
    for event in events.iter().filter(|event| event.kind == "group_message") {
        if seen.contains(&event.group_id) {
            continue;
        }
        seen.push(event.group_id.clone());
        let Ok(group_id) = URL_SAFE_NO_PAD.decode(&event.group_id) else { continue };
        let kept = leaf::step(factory, |step| Ok(step.groups()?.into_iter().find(|group| group.group_id == group_id)))?;
        if let Some(group) = kept {
            waiting(catch_up(factory, &group).await)?;
        }
    }
    Ok(())
}

/// Everything: KeyPackages, Welcomes, which groups, and each one.
async fn full_round(factory: &Arc<Factory>) -> Result<(), Stop> {
    if let Err(stop) = key_packages(factory).await {
        // The person's devices may not list this leaf yet: the rest still runs.
        tracing::debug!(?stop, "this Claktory's KeyPackages weren't topped up");
    }
    let peers: Vec<String> = leaf::step(factory, |step| Ok(step.groups()?.into_iter().map(|group| group.peer).collect()))?;
    refresh_keys(factory, &peers).await?;
    let welcomed = welcomes(factory).await;
    // After the Welcomes, so a group joined now routes to the home too.
    follow_list(factory)?;
    welcomed?;
    let listed = groups(factory).await?;
    for group in leaf::step(factory, |step| step.groups())? {
        if waiting(catch_up(factory, &group).await)? {
            continue;
        }
        if group.channel
            && let Some(at) = listed.iter().find(|listed| listed.id == group.group_id)
            && out_of_step(factory, &group, at)?
        {
            tracing::warn!("this Claktory's channel is out of step (a commit it never took): it rejoins");
            if !rejoin(factory, &group).await {
                continue;
            }
        }
        // Taken out of it earlier, and its rejoin waited (throttled).
        if group.channel && taken_out(factory, &group)? && !rejoin(factory, &group).await {
            continue;
        }
        rotate(factory, &group).await?;
    }
    Ok(())
}

/// Follows the person's own signed device list (#451), as this leaf took
/// it (`refresh_keys`: their pinned identity's, never older than one seen
/// before, Fugo's anti-replay): this Claktory is their home when it names
/// its endpoint; otherwise its leaf's groups route to the home it names, if
/// any. A list under another identity than the one this leaf's certificate
/// is from (the person's new key, not given to this Claktory yet) changes
/// nothing. The home lets go of the person's other Claktories the list no
/// longer names.
fn follow_list(factory: &Arc<Factory>) -> Result<(), Stop> {
    let ours = match crate::remote::secret_key(&factory.home) {
        Ok(key) => *key.public().as_bytes(),
        Err(error) => {
            tracing::debug!(%error, "no endpoint id to place this Claktory by the device list");
            return Ok(());
        }
    };
    let read = leaf::step(factory, |step| {
        let leaf = step.leaf()?;
        let Some(list) = step.own_list()? else { return Ok(None) };
        if list.identity != leaf.me.certificate.identity {
            return Ok(None);
        }
        let groups: Vec<Vec<u8>> = step.groups()?.into_iter().map(|group| group.group_id).collect();
        Ok(Some((list, groups)))
    })?;
    let Some((list, groups)) = read else { return Ok(()) };
    let placed = home::placed(&list, &ours);
    let followed = home::follow_list(factory, &placed, &groups).and_then(|changed| {
        if placed == home::Placed::Home {
            let claktories: Vec<String> =
                list.list.leaves.iter().filter(|listed| listed.kind == LeafKind::Claktory).map(|listed| listed.leaf_id.clone()).collect();
            home::unbind_missing(&**factory, &claktories)?;
        }
        Ok(changed)
    });
    match followed {
        // The links follow the routes and bindings, the gate is_home: each
        // round, since a binding the list dropped may not count as a change.
        Ok(_changed) => crate::dm_link::reload(factory),
        Err(error) => tracing::debug!(?error, "the direct-message home didn't follow the device list"),
    }
    Ok(())
}

/// Whether `group`, read through everything the worker listed, is at an
/// earlier epoch than the worker's: it missed a commit, so nothing it reads
/// from now on opens (an old Claktory skipped one; R5).
fn out_of_step(factory: &Factory, group: &KeptGroup, listed: &ListedGroup) -> Result<bool, Stop> {
    Ok(leaf::step(factory, |step| {
        let leaf = step.leaf()?;
        let read = store::position(step.db(), &group.group_id)?.is_some_and(|position| position >= listed.last_seq);
        let Some(mls) = Group::load(&step.provider()?, step.kind_of(&leaf, group), &group.group_id)? else { return Ok(false) };
        Ok(read && mls.epoch() < listed.epoch)
    })?)
}

/// Whether a commit took this leaf out of `group`, its MLS state kept.
fn taken_out(factory: &Factory, group: &KeptGroup) -> Result<bool, Stop> {
    Ok(leaf::step(factory, |step| {
        let leaf = step.leaf()?;
        Ok(Group::load(&step.provider()?, step.kind_of(&leaf, group), &group.group_id)?.is_some_and(|mls| mls.removed()))
    })?)
}

/// A group whose catching up stopped at a security key this leaf hasn't
/// been given, or at a leaf no list it can fetch names (`process`), or a
/// channel waiting to rejoin, waits there; the others carry on. Whether it
/// waits.
fn waiting(caught: Result<(), Stop>) -> Result<bool, Stop> {
    match caught {
        Ok(()) => Ok(false),
        Err(Stop::Unready(Unready::Rotating | Unready::FriendsNewKey | Unready::Unlisted | Unready::NoChannel)) => Ok(true),
        Err(stop) => Err(stop),
    }
}

/// Moves this leaf's own place in `group` to the certificate it was last
/// enrolled with, if it isn't there yet: after the person made a new
/// security key (dms-e2ee.md §2.1, R2), one Update carrying the new
/// certificate under the same leaf key and nothing else (the one commit a
/// leaf under a replaced key may make). Which groups still need it is what
/// MLS keeps, so a restart carries on where this stopped. `group` is caught
/// up first; a commit that came first (409) is caught up on and it tries
/// again. One sent but never answered stays pending: if the worker took it,
/// catching up merges it (`OwnPendingCommit`), and if not, it goes.
async fn rotate(factory: &Arc<Factory>, group: &KeptGroup) -> Result<(), Stop> {
    let id = URL_SAFE_NO_PAD.encode(&group.group_id);
    for _ in 0..3 {
        let outgoing = leaf::step(factory, |step| {
            let leaf = step.leaf()?;
            let provider = step.provider()?;
            let Some(mut mls) = Group::load(&provider, step.kind_of(&leaf, group), &group.group_id)? else { return Ok(None) };
            // Moved already, or not in it any more (removed, and forgotten
            // at the next round): nothing to do.
            if !matches!(mls.holds(&leaf.me), Ok(false)) {
                return Ok(None);
            }
            // Caught up to the end without it: one sent before wasn't taken.
            mls.clear_pending(&provider)?;
            Ok(Some(mls.update_certificate(&provider, &leaf.me)?))
        })?;
        let Some(outgoing) = outgoing else { return Ok(()) };
        let commit = outgoing.commit.clone();
        let mut body = json!({"commit": Envelope::new(outgoing.commit).map_err(unreadable)?.to_json()});
        if let Some(info) = outgoing.group_info {
            body["group_info"] = Envelope::new(info).map_err(unreadable)?.to_json();
        }
        let pending = |step: &mut leaf::Step<'_>, merge: bool| {
            let leaf = step.leaf()?;
            let provider = step.provider()?;
            if let Some(mut mls) = Group::load(&provider, step.kind_of(&leaf, group), &group.group_id)? {
                if merge { mls.merge_pending(&provider)? } else { mls.clear_pending(&provider)? }
            }
            Ok(())
        };
        match call(factory, "POST", &format!("groups/{id}/commits"), &[], Some(body)).await {
            Ok(_) => {
                leaf::step(factory, |step| pending(step, true))?;
                let _ = to_home(factory, &group.group_id, commit);
                tracing::info!(channel = group.channel, "this Claktory moved to the person's new security key in a group");
                return Ok(());
            }
            Err(Problem::Refused(409, _)) => {
                leaf::step(factory, |step| pending(step, false))?;
                if waiting(catch_up(factory, group).await)? {
                    return Ok(());
                }
            }
            Err(problem) => return Err(problem.into()),
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct Supply {
    remaining: i64,
    #[serde(default)]
    last_resort: bool,
    /// When the last resort's lifetime ends (seconds since the epoch); 0
    /// when there's none or the worker doesn't say.
    #[serde(default)]
    last_resort_ends: i64,
}

/// Keeps the leaf's KeyPackages at the worker topped up: the last-resort
/// one first, so a leaf the worker won't take yet makes one, not a hundred.
/// Each lasts thirty days (Melone's F4): the last resort is replaced a week
/// before it ends, and whenever it makes some, the leaf deletes the private
/// keys of its own whose lifetime is up.
async fn key_packages(factory: &Arc<Factory>) -> Result<(), Stop> {
    let supply: Supply = get(factory, "key-packages", &[]).await?;
    let now = (now_ms() / 1000).max(0);
    let ending = supply.last_resort_ends != 0 && supply.last_resort_ends <= now + LAST_RESORT_RENEW as i64;
    if !supply.last_resort || ending || supply.remaining < LOW {
        let gone = leaf::step(factory, |step| Ok(step.provider()?.sweep_key_packages(now as u64)?))?;
        if gone > 0 {
            tracing::info!(gone, "this Claktory deleted key packages past their lifetime");
        }
    }
    let mut body = json!({"key_packages": []});
    if !supply.last_resort || ending {
        let package = leaf::step(factory, |step| {
            let leaf = step.leaf()?;
            Ok(Envelope::new(leaf.me.last_resort_key_package(&step.provider()?)?)?)
        })?;
        body["last_resort"] = package.to_json();
        call(factory, "POST", "key-packages", &[], Some(body.clone())).await?;
        body = json!({"key_packages": []});
    }
    if supply.remaining >= LOW {
        return Ok(());
    }
    let packages = leaf::step(factory, |step| {
        let leaf = step.leaf()?;
        let provider = step.provider()?;
        (supply.remaining..STOCK).map(|_| Ok(Envelope::new(leaf.me.key_package(&provider)?)?.to_json())).collect::<Result<Vec<_>, StepError>>()
    })?;
    body["key_packages"] = Value::Array(packages);
    call(factory, "POST", "key-packages", &[], Some(body)).await?;
    Ok(())
}

#[derive(Deserialize)]
struct Keys {
    #[serde(default)]
    device_list: String,
}

/// The latest signed device lists of this Claktory's own account and of
/// `accounts`, from the worker: taken only when signed by the identity the
/// person's devices pinned (a key the service swapped is never adopted
/// here; the person's devices tell this Claktory of a real change), and
/// never older than the one kept. Whether any was newer.
async fn refresh_keys(factory: &Arc<Factory>, accounts: &[String]) -> Result<bool, Stop> {
    let own = leaf::step(factory, |step| Ok(step.registration().0.clone()))?;
    let mut wanted: Vec<&str> = std::iter::once(own.as_str()).chain(accounts.iter().map(String::as_str)).filter(|account| !account.is_empty()).collect();
    wanted.sort_unstable();
    wanted.dedup();
    let mut newer = false;
    for account in wanted {
        if !super::valid_id(account) {
            continue;
        }
        let keys: Keys = match get(factory, &format!("keys/{account}"), &[]).await {
            Err(Stop::Problem(Problem::Refused(403 | 404 | 409, _))) => continue,
            other => other?,
        };
        let Ok(list) = URL_SAFE_NO_PAD.decode(&keys.device_list) else { continue };
        let accepted = leaf::step(factory, |step| {
            let before = step.list_version(account)?;
            step.accept_list(account, &list)?;
            Ok(step.list_version(account)? > before)
        });
        match accepted {
            Ok(moved) => newer |= moved,
            Err(error) => tracing::debug!(?error, "a device list this Claktory didn't take"),
        }
    }
    Ok(newer)
}

#[derive(Deserialize)]
struct Welcomes {
    #[serde(default)]
    welcomes: Vec<Welcome>,
}

#[derive(Deserialize)]
struct Welcome {
    id: String,
    group_id: String,
    kind: String,
    #[serde(default)]
    conversation_id: String,
    #[serde(default)]
    contact_generation: i64,
    #[serde(default)]
    claktory_id: String,
    #[serde(default)]
    accounts: Vec<String>,
    welcome: Value,
    /// The commit that added this leaf: it reads on after it.
    #[serde(default)]
    seq: i64,
}

/// Joins the groups this leaf is welcomed to: its channel, and the
/// conversations the person gave its agents.
async fn welcomes(factory: &Arc<Factory>) -> Result<(), Stop> {
    let listed: Welcomes = get(factory, "welcomes", &[]).await?;
    if listed.welcomes.is_empty() {
        return Ok(());
    }
    // A device pins a friend and tells the channel before it adds this leaf
    // to their conversation: read the channel first, or the friend isn't
    // pinned yet and the Welcome is lost.
    if let Err(error) = catch_up_channel(factory).await {
        tracing::debug!(?error, "the channel wasn't read before the Welcomes");
    }
    let peers: Vec<String> = listed.welcomes.iter().flat_map(|welcome| welcome.accounts.clone()).collect();
    if !peers.is_empty() {
        refresh_keys(factory, &peers).await?;
    }
    for welcome in listed.welcomes {
        let joined = leaf::step(factory, |step| join(step, &welcome));
        match joined {
            Ok(()) => {}
            Err(StepError::Unready(unready)) => return Err(Stop::Unready(unready)),
            // One that doesn't join never will: it goes.
            Err(StepError::Failed(error)) => tracing::warn!(%error, kind = %welcome.kind, "a Welcome for this Claktory was refused"),
        }
        match call(factory, "DELETE", &format!("welcomes/{}", welcome.id), &[], None).await {
            Ok(_) | Err(Problem::Refused(404, _)) => {}
            Err(problem) => return Err(problem.into()),
        }
    }
    Ok(())
}

fn join(step: &mut leaf::Step<'_>, welcome: &Welcome) -> Result<(), StepError> {
    let leaf = step.leaf()?;
    let group_id = URL_SAFE_NO_PAD.decode(&welcome.group_id).map_err(|_| StepError::Failed("a group id that doesn't read".to_owned()))?;
    if step.groups()?.iter().any(|group| group.group_id == group_id) {
        return Ok(());
    }
    let kept = match welcome.kind.as_str() {
        "channel" if welcome.claktory_id == leaf.claktory_id => KeptGroup {
            group_id: group_id.clone(),
            channel: true,
            conversation: String::new(),
            generation: 0,
            peer: String::new(),
            position: welcome.seq,
        },
        "conversation" if welcome.accounts.len() == 2 && welcome.accounts.contains(&leaf.account_id) => {
            let peer = welcome.accounts.iter().find(|account| **account != leaf.account_id).cloned().unwrap_or_default();
            if peer.is_empty() {
                return Err(StepError::Failed("a Welcome to a conversation with no friend in it".to_owned()));
            }
            KeptGroup {
                group_id: group_id.clone(),
                channel: false,
                conversation: welcome.conversation_id.clone(),
                generation: welcome.contact_generation,
                peer,
                position: welcome.seq,
            }
        }
        other => return Err(StepError::Failed(format!("a Welcome to a {other} group this leaf isn't for"))),
    };
    if !super::valid_id(&kept.conversation) && !kept.channel {
        return Err(StepError::Failed("a Welcome to a conversation whose id doesn't read".to_owned()));
    }
    let bytes = Envelope::from_json(&welcome.welcome)?;
    let kind = step.kind_of(&leaf, &kept);
    let group = Group::join(&step.provider()?, &leaf.pins, kind, &group_id, bytes.bytes())?;
    // The friend comes from the service's labels: the tree itself has to
    // hold exactly the person and that friend, or a service could swap two
    // granted conversations' labels (Fugo). Refused, the transaction takes
    // the join back.
    if !kept.channel {
        let mut accounts = std::collections::BTreeSet::new();
        // A device the person kept at a new key, not moved yet, counts.
        for member in group.present(&leaf.pins) {
            accounts.insert(member?.account().to_owned());
        }
        let expected = std::collections::BTreeSet::from([leaf.account_id.clone(), kept.peer.clone()]);
        if accounts != expected {
            return Err(StepError::Failed("a Welcome to a conversation whose people aren't the ones the service named".to_owned()));
        }
    }
    step.keep_group(&kept)?;
    Ok(())
}

#[derive(Deserialize)]
struct Groups {
    #[serde(default)]
    groups: Vec<Listed>,
}

#[derive(Deserialize)]
struct Listed {
    group_id: String,
    #[serde(default)]
    epoch: u64,
    #[serde(default)]
    last_seq: i64,
}

/// A group the worker lists for this leaf, where it stands there.
struct ListedGroup {
    id: Vec<u8>,
    epoch: u64,
    last_seq: i64,
}

/// Forgets the groups the worker no longer lists for this leaf; the ones it
/// lists.
async fn groups(factory: &Arc<Factory>) -> Result<Vec<ListedGroup>, Stop> {
    let listed: Groups = get(factory, "groups", &[]).await?;
    let listed: Vec<ListedGroup> = listed
        .groups
        .iter()
        .filter_map(|group| {
            let id = URL_SAFE_NO_PAD.decode(&group.group_id).ok()?;
            Some(ListedGroup { id, epoch: group.epoch, last_seq: group.last_seq })
        })
        .collect();
    let gone: Vec<KeptGroup> = leaf::step(factory, |step| {
        Ok(step.groups()?.into_iter().filter(|group| !listed.iter().any(|listed| listed.id == group.group_id)).collect())
    })?;
    for group in gone {
        tracing::info!(channel = group.channel, "a direct-message group this Claktory can't reach any more is forgotten");
        // It asks to be removed, for the next device commit to take in
        // (the person's device usually has already); then it forgets.
        let leave = leaf::step(factory, |step| {
            let leaf = step.leaf()?;
            let provider = step.provider()?;
            let Some(mut mls) = Group::load(&provider, step.kind_of(&leaf, &group), &group.group_id)? else { return Ok(None) };
            Ok(Some(Envelope::new(mls.leave(&provider, &leaf.me)?)?))
        });
        if let Ok(Some(proposal)) = leave {
            let route = format!("groups/{}/proposals", URL_SAFE_NO_PAD.encode(&group.group_id));
            if let Err(problem) = call(factory, "POST", &route, &[], Some(json!({"proposal": proposal.to_json()}))).await {
                tracing::debug!(?problem, "this Claktory's leaving wasn't taken");
            }
        }
        leaf::step(factory, |step| Ok(store::forget_group(step.db(), &group.group_id)?))?;
    }
    Ok(listed)
}

#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    messages: Vec<Message>,
    #[serde(default)]
    has_more: bool,
}

#[derive(Deserialize, Clone)]
struct Message {
    seq: i64,
    /// "application", "commit" or "proposal", as the service says.
    #[serde(default)]
    kind: String,
    envelope: Value,
    #[serde(default)]
    accepted_ms: i64,
    #[serde(default)]
    epoch: u64,
}

/// What processing one message did that matters after its step.
#[derive(Default)]
struct Done {
    /// A commit in the channel added someone: a device that can't read
    /// what was proposed before it.
    added_in_channel: bool,
    /// A commit or proposal from an epoch after this leaf's, which it
    /// couldn't take: it missed a commit, and is out of step.
    behind: bool,
    /// A commit in the channel took this leaf out: after the person started
    /// over (row 630), their device removes the Claktory's leaf under the
    /// old key along with the rest, and the Claktory, enrolled again under
    /// the new one, rejoins.
    removed: bool,
}

/// Processes `group` from its position to the end of its mailbox.
async fn catch_up(factory: &Arc<Factory>, group: &KeptGroup) -> Result<(), Stop> {
    let id = URL_SAFE_NO_PAD.encode(&group.group_id);
    // A channel kept without its MLS state: its rejoin wasn't taken.
    let stateless = group.channel
        && leaf::step(factory, |step| {
            let leaf = step.leaf()?;
            Ok(Group::load(&step.provider()?, step.kind_of(&leaf, group), &group.group_id)?.is_none())
        })?;
    if stateless && !rejoin(factory, group).await {
        return Err(Stop::Unready(Unready::NoChannel));
    }
    loop {
        let position = leaf::step(factory, |step| Ok(store::position(step.db(), &group.group_id)?))?;
        let Some(position) = position else { return Ok(()) };
        let home = from_home(factory, group, position);
        let read_home = home.is_some();
        let page: Page = match home {
            Some(Ok(page)) => page,
            Some(Err(())) => return behind(factory, group).await,
            None => {
                let route = format!("groups/{id}/messages");
                match get(factory, &route, &[("after", position.to_string()), ("limit", PAGE.to_string())]).await {
                    Err(Stop::Problem(Problem::Refused(410, _))) => return behind(factory, group).await,
                    other => other?,
                }
            }
        };
        let mut added = false;
        let mut behind = false;
        let mut removed = false;
        let mut waits = None;
        let mut waits_at = 0;
        for (at, message) in page.messages.iter().enumerate() {
            match leaf::step(factory, |step| process(step, group, message)) {
                Ok(done) => {
                    added |= done.added_in_channel;
                    behind |= done.behind;
                    removed |= done.removed;
                    if removed {
                        break;
                    }
                }
                Err(StepError::Unready(unready @ (Unready::Rotating | Unready::FriendsNewKey | Unready::Unlisted))) => {
                    waits = Some(unready);
                    waits_at = at;
                    break;
                }
                Err(error) => return Err(error.into()),
            }
        }
        if added {
            super::sends::reposted(factory).await;
        }
        // Out of step, nothing more it reads here opens: the channel
        // rejoins at the current epoch (else the round looks again).
        if behind && group.channel {
            tracing::warn!("this Claktory's channel is out of step (a commit it never took): it rejoins");
            if rejoin(factory, group).await {
                continue;
            }
        }
        // Taken out of its own channel, which the worker still lists for it:
        // it rejoins, once the person's devices list it under their key now
        // (until then the worker refuses, and it tries again later).
        if removed {
            tracing::warn!("this Claktory was taken out of its channel: it rejoins");
            if rejoin(factory, group).await {
                continue;
            }
            return Err(Stop::Unready(Unready::NoChannel));
        }
        if waits == Some(Unready::Unlisted) {
            // Never skipped (R5): a list newer than the one kept may name
            // the leaf (a device just added), so the same commit is read
            // again once it's fetched.
            if refresh_keys(factory, std::slice::from_ref(&group.peer)).await? {
                continue;
            }
            // The friend added a device and took it out again while this
            // leaf was away (row 561): read on past it once their own device
            // took it out, as one that was there did.
            if !group.channel && window(factory, group, &page.messages[waits_at..], page.has_more, read_home).await? {
                continue;
            }
            // No newer list: the channel rejoins at the current epoch, if
            // every member there checks out; a conversation waits.
            if group.channel && rejoin(factory, group).await {
                continue;
            }
            tracing::warn!(channel = group.channel, "a direct-message group waits at a commit naming a leaf no device list here names");
        }
        if group.channel {
            // Nothing is sent in a channel stopped at a commit.
            factory.dm_leaf.set_channel_waits(waits.clone());
        }
        if let Some(unready) = waits {
            return Err(Stop::Unready(unready));
        }
        if read_home {
            // Kept here now: the home may let go of it.
            let leaf_id = own_leaf(factory);
            let read = leaf::step(factory, |step| Ok(store::position(step.db(), &group.group_id)?))?;
            if let (Some(leaf_id), Some(read)) = (leaf_id, read)
                && let Err(error) = super::home::ack(&**factory, &leaf_id, &group.group_id, read)
            {
                tracing::debug!(?error, "the home didn't take this leaf's ack");
            }
        }
        if !page.has_more || page.messages.is_empty() {
            return Ok(());
        }
    }
}

/// This Claktory's own leaf's id, while it's signed in.
fn own_leaf(factory: &Factory) -> Option<String> {
    let config = factory.lock().config.account.clone();
    factory.account.registration(&config).map(|(_, claktory, epoch)| claktory_dm_crypto::certificate::claktory_leaf_id(&claktory, epoch))
}

/// The next page of `group` from this Claktory's home, after `position`,
/// when the home holds the group for this leaf (None: read the worker).
/// A position from the worker (below the home's seqs) reads from the leaf's
/// place at the home, its join there. Err: pruned past it, so it's behind.
fn from_home(factory: &Factory, group: &KeptGroup, position: i64) -> Option<Result<Page, ()>> {
    use super::home;
    let leaf_id = own_leaf(factory)?;
    let place = home::place(factory, &leaf_id, &group.group_id).ok()??;
    let after = if position < home::FIRST_SEQ - 1 { place } else { position };
    match home::page(factory, &leaf_id, &group.group_id, after, PAGE as usize) {
        Ok(Ok(page)) => Some(Ok(Page {
            messages: page
                .entries
                .into_iter()
                .filter_map(|entry| {
                    let envelope = Envelope::new(entry.envelope).ok()?.to_json();
                    Some(Message { seq: entry.seq, kind: entry.kind, envelope, accepted_ms: entry.accepted_ms, epoch: entry.epoch })
                })
                .collect(),
            has_more: page.has_more,
        })),
        Ok(Err(_)) => Some(Err(())),
        Err(error) => {
            tracing::debug!(?error, "the home didn't answer: the worker is read instead");
            None
        }
    }
}

/// One message, in one step: the position moves past it, so one bad
/// message never stops a group, except a commit or proposal that waits for
/// a key or a device list (taken back, unread, to be read again).
fn process(step: &mut leaf::Step<'_>, kept: &KeptGroup, message: &Message) -> Result<Done, StepError> {
    // Read already: the loop and an agent's read may fetch the same page.
    if store::position(step.db(), &kept.group_id)?.is_none_or(|position| message.seq <= position) {
        return Ok(Done::default());
    }
    let leaf = step.leaf()?;
    let kind = step.kind_of(&leaf, kept);
    let provider = step.provider()?;
    let Some(mut group) = Group::load(&provider, kind, &kept.group_id)? else { return Ok(Done::default()) };
    let mut done = Done::default();
    // A commit or proposal from an earlier epoch: from before this leaf
    // joined, or its own commit, already merged. `epoch` is the service's
    // figure, so a lie there can only drop a message, never put one in.
    // Application messages always go to MLS, which reads what's from an
    // epoch it still holds and refuses the rest.
    if message.kind == "application" || message.epoch >= group.epoch() {
        let before = group.members(&leaf.pins).len();
        let bytes = Envelope::from_json(&message.envelope).map(Envelope::into_bytes);
        match bytes.and_then(|bytes| group.process(&provider, &leaf.pins, &bytes)) {
            Ok(Incoming::Application { from, content, .. }) => {
                received(step, &leaf, kept, message, &from, &content, false)?;
            }
            Ok(Incoming::Committed { .. }) => {
                done.added_in_channel = kept.channel && group.members(&leaf.pins).len() > before;
                done.removed = kept.channel && group.removed();
            }
            Ok(Incoming::Proposal) => {}
            // Under a key the person's devices told this leaf was replaced
            // (after more than one new key, row 654): never taken here, so
            // out of step, and the channel rejoins.
            Err(claktory_dm_crypto::Error::Replaced { .. }) if message.kind != "application" => {
                tracing::warn!(seq = message.seq, channel = kept.channel, "a commit under a replaced security key: out of step");
                done.behind = true;
            }
            // A change under a security key this leaf hasn't been given: the
            // person's new one before a device enrolls this Claktory under
            // it, or a friend's before the person's devices pass it on. Left
            // unread (the step is taken back), the group waits here rather
            // than fall out of step with it; the next round tries again.
            Err(claktory_dm_crypto::Error::UnknownIdentity { account }) if message.kind != "application" => {
                tracing::info!(seq = message.seq, channel = kept.channel, "a direct-message group waits for a new security key");
                let own = account == leaf.account_id;
                return Err(StepError::Unready(if own { Unready::Rotating } else { Unready::FriendsNewKey }));
            }
            // A leaf the device list kept here doesn't name, most likely a
            // device just added whose list this leaf hasn't fetched yet:
            // skipped, the group would be an epoch behind for good (R5).
            // Left unread, and read again with the newest list.
            Err(claktory_dm_crypto::Error::Unlisted { .. }) if message.kind != "application" => {
                tracing::info!(seq = message.seq, channel = kept.channel, "a direct-message group waits for a newer device list");
                return Err(StepError::Unready(Unready::Unlisted));
            }
            Err(error) => {
                tracing::warn!(?error, seq = message.seq, channel = kept.channel, "a direct message this Claktory couldn't take");
                done.behind = message.kind != "application" && message.epoch > group.epoch();
            }
        }
    }
    store::set_position(step.db(), &kept.group_id, message.seq)?;
    Ok(done)
}

/// How a window's try ended (`take_window`).
enum WindowEnd {
    /// Taken past the removal: the position moved past it.
    Closed,
    /// It needs the items after these.
    Open,
    /// None to open, or it can't close: the group waits as before.
    No,
}

/// A conversation stopped at a commit bringing in a leaf the friend's
/// newest list doesn't name (row 561): `from[0]` that commit, then what
/// follows it, read on as `Group::process_window` rules, with more pages
/// while the window is open, up to `WINDOW_MOST` items. Whether it moved.
async fn window(factory: &Arc<Factory>, group: &KeptGroup, from: &[Message], has_more: bool, read_home: bool) -> Result<bool, Stop> {
    let mut items: Vec<Message> = from.iter().take(WINDOW_MOST).cloned().collect();
    let mut more = has_more;
    loop {
        match leaf::step(factory, |step| take_window(step, group, &items))? {
            WindowEnd::Closed => {
                tracing::info!(seq = items[0].seq, "a direct-message group read on past a device its person added and took out again");
                return Ok(true);
            }
            WindowEnd::Open if more && items.len() < WINDOW_MOST => {
                let Some(after) = items.last().map(|message| message.seq) else { return Ok(false) };
                let Some(page) = next_page(factory, group, after, read_home).await else { return Ok(false) };
                if page.messages.is_empty() {
                    return Ok(false);
                }
                more = page.has_more;
                let room = WINDOW_MOST - items.len();
                items.extend(page.messages.into_iter().take(room));
            }
            WindowEnd::Open | WindowEnd::No => return Ok(false),
        }
    }
}

/// The page after `after`, from where `catch_up` read the last one; None
/// when it can't be had now.
async fn next_page(factory: &Arc<Factory>, group: &KeptGroup, after: i64, read_home: bool) -> Option<Page> {
    if read_home {
        return match from_home(factory, group, after) {
            Some(Ok(page)) => Some(page),
            _ => None,
        };
    }
    let route = format!("groups/{}/messages", URL_SAFE_NO_PAD.encode(&group.group_id));
    get(factory, &route, &[("after", after.to_string()), ("limit", PAGE.to_string())]).await.ok()
}

/// One try at a window, in a savepoint of the step's own: kept only when
/// it closed, with what was said in it (marked when its leaf said it).
fn take_window(step: &mut leaf::Step<'_>, kept: &KeptGroup, items: &[Message]) -> Result<WindowEnd, StepError> {
    let leaf = step.leaf()?;
    let kind = step.kind_of(&leaf, kept);
    let Ok(envelopes) =
        items.iter().map(|message| Envelope::from_json(&message.envelope).map(Envelope::into_bytes)).collect::<Result<Vec<_>, _>>()
    else {
        return Ok(WindowEnd::No);
    };
    step.db().execute_batch("SAVEPOINT dm_window")?;
    let outcome = (|| {
        let provider = step.provider()?;
        let Some(mut group) = Group::load(&provider, kind, &kept.group_id)? else { return Ok(None) };
        let items: Vec<&[u8]> = envelopes.iter().map(Vec::as_slice).collect();
        Ok::<_, StepError>(Some(group.process_window(&provider, &leaf.pins, &items)))
    })();
    let taken = match outcome {
        Ok(Some(Ok(Window::Closed { taken }))) => taken,
        other => {
            step.db().execute_batch("ROLLBACK TO dm_window; RELEASE dm_window")?;
            return match other {
                Ok(Some(Ok(Window::Open))) => Ok(WindowEnd::Open),
                Ok(Some(Ok(Window::Aborted { why }))) => {
                    tracing::info!(why, seq = items[0].seq, "a direct-message group can't read on past a device its person took out");
                    Ok(WindowEnd::No)
                }
                Ok(Some(Err(error))) => {
                    tracing::debug!(?error, seq = items[0].seq, "no window to read on through");
                    Ok(WindowEnd::No)
                }
                Ok(None) | Ok(Some(Ok(Window::Closed { .. }))) => Ok(WindowEnd::No),
                Err(error) => Err(error),
            };
        }
    };
    for (message, windowed) in items.iter().zip(&taken) {
        if let Ok(Incoming::Application { from, content, .. }) = &windowed.incoming {
            received(step, &leaf, kept, message, from, content, windowed.departed)?;
        }
    }
    let last = items[taken.len() - 1].seq;
    store::set_position(step.db(), &kept.group_id, last)?;
    step.db().execute_batch("RELEASE dm_window")?;
    Ok(WindowEnd::Closed)
}

/// What someone said: kept in the conversation's history, or, in the
/// channel, what the person's devices tell this Claktory.
fn received(
    step: &mut leaf::Step<'_>,
    leaf: &leaf::Leaf,
    kept: &KeptGroup,
    message: &Message,
    from: &claktory_dm_crypto::validation::Member,
    content: &[u8],
    departed: bool,
) -> Result<(), StepError> {
    let content = Content::decode(content)?;
    if kept.channel {
        // Pins only from the person's own devices, merged as at enrollment.
        // Never from a device the person has since taken out (a window's).
        if content.kind == kind::PINS && from.kind() == LeafKind::Device && from.account() == leaf.account_id && !departed {
            let given: Pins = content
                .extra
                .get("pins")
                .cloned()
                .and_then(|pins| serde_json::from_value(pins).ok())
                .ok_or_else(|| StepError::Failed("pins that don't read".to_owned()))?;
            let mut given = given;
            given.reparse()?;
            if let Err(error) = step.import_pins(&given) {
                tracing::warn!(?error, "pins from the person's device disagree with this Claktory's: kept its own");
            }
        }
        return Ok(());
    }
    if content.kind != kind::TEXT {
        return Ok(());
    }
    let mut row = json!({
        "from_account": from.account(),
        "from_leaf": from.leaf_id(),
        "content": content,
    });
    if departed {
        // Said by a device its person has since taken out (row 561).
        row["departed"] = json!(true);
    }
    let place: [&[u8]; 3] = [b"history", &kept.group_id, &message.seq.to_be_bytes()];
    let sealed = step.seal(&place, &serde_json::to_vec(&row).map_err(|error| StepError::Failed(error.to_string()))?)?;
    step.db().execute(
        "INSERT OR REPLACE INTO history (group_id, seq, accepted, sealed) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![kept.group_id, message.seq, message.accepted_ms, sealed],
    )?;
    if from.account() != leaf.account_id && !kept.conversation.is_empty() {
        step.hear(&kept.conversation, message.seq, message.accepted_ms);
    }
    Ok(())
}

/// Too far behind a group's mailbox: its own channel it rejoins by external
/// commit; a conversation is forgotten until a device adds it again.
async fn behind(factory: &Arc<Factory>, group: &KeptGroup) -> Result<(), Stop> {
    if !group.channel {
        leaf::step(factory, |step| Ok(store::forget_group(step.db(), &group.group_id)?))?;
        return Ok(());
    }
    if rejoin(factory, group).await { Ok(()) } else { Err(Stop::Unready(Unready::NoChannel)) }
}

/// Hands a commit of this leaf's that claktory.com took to the person's
/// home (#451): this Claktory's own home.db takes it into the log if it's
/// the home, or passes it on to the home (which takes it only as a hint and
/// looks at claktory.com). Without it the home finds it at its next pull.
/// Its seq in this Claktory's home.db, when it's there.
fn to_home(factory: &Factory, group_id: &[u8], commit: Vec<u8>) -> Option<i64> {
    let leaf_id = match leaf::step(factory, |step| Ok(step.leaf()?.me.certificate.leaf.leaf_id.clone())) {
        Ok(leaf_id) => leaf_id,
        Err(error) => {
            tracing::debug!(?error, "no leaf to hand its commit to the home");
            return None;
        }
    };
    let intake = home::Intake {
        group_id: group_id.to_vec(),
        envelope: commit,
        source: home::Source::Local { leaf_id, client_message_id: String::new() },
    };
    match home::take(factory, intake) {
        Ok(home::Taken::Stored { seq, .. }) => Some(seq),
        Ok(_) => None,
        Err(error) => {
            tracing::debug!(?error, "the home didn't take this Claktory's commit");
            None
        }
    }
}

/// Rejoins this Claktory's channel by external commit at its current epoch,
/// from the GroupInfo the worker keeps: when it's too far behind, or at a
/// commit that stays refused under the newest device lists (R5). Every
/// other member there must check out against the lists kept here
/// (`Group::rejoin`), and the GroupInfo must be this group's; if not,
/// nothing changes and the channel waits. At most one attempt a round,
/// the wait doubling after each that fails (Fugo). Whether it rejoined.
async fn rejoin(factory: &Arc<Factory>, group: &KeptGroup) -> bool {
    let base = Duration::from_millis(REJOIN_WAIT.load(Ordering::SeqCst));
    {
        let throttle = factory.dm_leaf.rejoin();
        if throttle.next.is_some_and(|next| std::time::Instant::now() < next) {
            return false;
        }
    }
    let outcome = rejoin_now(factory, group).await;
    let mut throttle = factory.dm_leaf.rejoin();
    throttle.wait = if outcome.is_ok() || throttle.wait.is_zero() { base } else { (throttle.wait * 2).min(REJOIN_MOST.max(base)) };
    throttle.next = Some(std::time::Instant::now() + throttle.wait);
    match outcome {
        Ok(()) => {
            tracing::info!("this Claktory rejoined its channel");
            true
        }
        Err(stop) => {
            tracing::warn!(?stop, "this Claktory couldn't rejoin its channel: it waits, and tries again later");
            false
        }
    }
}

async fn rejoin_now(factory: &Arc<Factory>, group: &KeptGroup) -> Result<(), Stop> {
    let id = URL_SAFE_NO_PAD.encode(&group.group_id);
    #[derive(Deserialize)]
    struct Info {
        group_info: Value,
    }
    let info: Info = get(factory, &format!("groups/{id}/group-info"), &[]).await?;
    let info = Envelope::from_json(&info.group_info).map_err(unreadable)?;
    // Refused (a member no list here names, another group's GroupInfo), the
    // step is taken back and the channel stays as it was.
    let outgoing = leaf::step(factory, |step| {
        let leaf = step.leaf()?;
        store::forget_group(step.db(), &group.group_id)?;
        let kind = step.kind_of(&leaf, group);
        let floor = claktory_dm_crypto::storage::epoch_floor(step.db(), &group.group_id)?;
        let (_, outgoing) = Group::rejoin(&step.provider()?, &leaf.me, &leaf.pins, kind, &group.group_id, info.bytes(), floor)?;
        Ok(outgoing)
    })?;
    let commit = outgoing.commit.clone();
    let mut body = json!({"commit": Envelope::new(outgoing.commit).map_err(unreadable)?.to_json()});
    if let Some(info) = outgoing.group_info {
        body["group_info"] = Envelope::new(info).map_err(unreadable)?.to_json();
    }
    let taken = call(factory, "POST", &format!("groups/{id}/commits"), &[], Some(body)).await;
    #[derive(Deserialize)]
    struct Taken {
        seq: i64,
    }
    match taken {
        Ok(answer) => {
            let taken: Taken = serde_json::from_slice(&answer).map_err(unreadable)?;
            leaf::step(factory, |step| {
                let leaf = step.leaf()?;
                let provider = step.provider()?;
                let kind = step.kind_of(&leaf, group);
                if let Some(mut rejoined) = Group::load(&provider, kind, &group.group_id)? {
                    rejoined.merge_pending(&provider)?;
                }
                step.keep_group(&KeptGroup { position: taken.seq, ..group.clone() })?;
                Ok(())
            })?;
            // Read from the home, it reads on after the commit's place there.
            if let Some(seq) = to_home(factory, &group.group_id, commit) {
                leaf::step(factory, |step| Ok(store::set_position(step.db(), &group.group_id, seq)?))?;
            }
            Ok(())
        }
        Err(problem) => {
            // Not taken (another commit came first): kept without its MLS
            // state, so it rejoins again, never lost.
            leaf::step(factory, |step| {
                store::forget_group(step.db(), &group.group_id)?;
                step.keep_group(group)
            })?;
            Err(problem.into())
        }
    }
}

/// Processes a conversation's group up to now, before an agent reads it;
/// what fails is left to the loop.
pub(crate) async fn catch_up_conversation(factory: &Arc<Factory>, group_id: &str) {
    let Ok(group_id) = URL_SAFE_NO_PAD.decode(group_id) else { return };
    let kept = leaf::step(factory, |step| Ok(step.groups()?.into_iter().find(|group| group.group_id == group_id)));
    if let Ok(Some(group)) = kept
        && let Err(stop) = catch_up(factory, &group).await
    {
        tracing::debug!(?stop, "a conversation wasn't caught up before reading it");
    }
}

/// Processes this Claktory's channel up to now: for a send the worker
/// refused as made at an old epoch.
pub(crate) async fn catch_up_channel(factory: &Arc<Factory>) -> Result<(), StepError> {
    let channel = leaf::step(factory, |step| Ok(step.groups()?.into_iter().find(|group| group.channel)))?;
    let Some(channel) = channel else { return Err(StepError::Unready(Unready::NoChannel)) };
    match catch_up(factory, &channel).await {
        Ok(()) => Ok(()),
        Err(Stop::Unready(unready)) => Err(StepError::Unready(unready)),
        Err(stop) => Err(StepError::Failed(format!("{stop:?}"))),
    }
}

