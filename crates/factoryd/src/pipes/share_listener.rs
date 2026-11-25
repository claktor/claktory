//! Opening a website a pipe's peer shares with this Claktory, in this
//! computer's own browser (pipe-port-sharing.md, revised plan: the
//! receiving side).
//!
//! The first Open starts a listener for the share on 127.0.0.1, at a port
//! that stays the share's for as long as it lasts (the sharer's own number
//! when it's free here), under a hostname of its own, `s-<random>.localhost`,
//! never used for anything else. The browser opens a one-time address
//! (`/__claktory_open?k=…`, single use, for a minute), which the listener
//! trades for the share's key cookie (HttpOnly, SameSite=Strict), and from
//! then on every request, WebSocket upgrades included, must carry it and
//! name the share's exact host. What passes goes to the sharer over the
//! pipe's connection (`shares::stream`), one stream per browser connection,
//! spoken as HTTP/1.1, so keep-alive and upgrades work as they would on the
//! sharer's computer.
//!
//! What this keeps out (Fugo, msg 30760): other users of this computer and
//! web pages, which can all reach 127.0.0.1 with any Host they like. The
//! exact Host check stops DNS rebinding; the cookie stops everything that
//! hasn't been through Open. Safari can't resolve `*.localhost`, so there
//! the share answers at `127.0.0.1:<port>` too, where the browser sends it
//! every cookie of 127.0.0.1, whatever their port: there, only the cookies
//! the shared website set itself go on to it.
//!
//! The share's end (Stop on either side, its pipe paused or closed, the
//! peer's identity changed) closes the listener and every connection at
//! once. Shares that were opened listen again after a restart, at the same
//! address, so a bookmark keeps working.

use std::collections::{BTreeSet, HashMap};
use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use base64::Engine as _;
use http_body_util::{Either, Full};
use hyper::body::{Bytes, Incoming};
use hyper::header::{self, HeaderValue};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rusqlite::{OptionalExtension as _, params};
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::factory::Factory;

pub(crate) const SCHEMA: &str = "
-- Websites pipes' peers share with this Claktory, as it opens them
-- (pipes/share_listener.rs): the hostname and port each answers at, the
-- SHA-256s of its key cookies, the cookies the website set itself, whether
-- the person hid it, when it was first opened and when it ended. Rows stay
-- after the share ends: a hostname is never used again, nor a port for
-- another peer's share.
CREATE TABLE pipe_share_visits (
    share TEXT PRIMARY KEY,
    peer TEXT NOT NULL,
    host TEXT NOT NULL UNIQUE,
    port INTEGER NOT NULL,
    keys TEXT NOT NULL DEFAULT '[]',
    cookies TEXT NOT NULL DEFAULT '[]',
    hidden INTEGER NOT NULL DEFAULT 0,
    opened INTEGER NOT NULL DEFAULT 0,
    ended INTEGER NOT NULL DEFAULT 0
);
";

/// The path the one-time code comes to; never passed on.
const OPEN_PATH: &str = "/__claktory_open";
/// How long a one-time code works.
const CODE_LIFE: Duration = Duration::from_secs(60);
/// The key cookies a share keeps (the latest), one per browser it was opened in.
const KEYS_KEPT: usize = 8;
/// The browser connections a share's listener takes at once.
const CONNECTIONS: usize = 64;
/// How long the key cookie lasts in the browser: the most browsers keep
/// one (400 days). The share's end makes it worthless sooner.
const KEY_AGE: u64 = 400 * 86_400;
/// The cookies a shared website may set that the listener remembers, for
/// Safari's filter.
const COOKIES_KEPT: usize = 200;

/// The listeners running, by share.
#[derive(Default)]
pub(crate) struct Listeners {
    live: Mutex<HashMap<String, Live>>,
}

struct Live {
    gate: Arc<Gate>,
    cancel: CancellationToken,
}

/// What the app shows of a share it receives.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Status {
    /// `http://s-<random>.localhost:<port>/`, the share's own address.
    pub address: String,
    /// `http://127.0.0.1:<port>/`, for Safari.
    pub fallback_address: String,
    /// Whether its listener is up now.
    pub listening: bool,
    /// Why it isn't, when it should be, as the person reads it.
    pub problem: String,
    pub hidden: bool,
}

/// What Open gives the app to open in the browser: the one-time address,
/// under the share's hostname or 127.0.0.1 (Safari).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Opened {
    pub url: String,
    pub fallback_url: String,
}

/// A share as this side keeps it.
struct Visit {
    peer: String,
    host: String,
    port: u16,
    keys: Vec<String>,
    cookies: BTreeSet<String>,
    hidden: bool,
    opened: i64,
    ended: i64,
}

fn visit(factory: &Factory, share: &str) -> Option<Visit> {
    let db = factory.store.db();
    let found = db
        .query_row("SELECT peer, host, port, keys, cookies, hidden, opened, ended FROM pipe_share_visits WHERE share = ?1", [share], |row| {
            let keys: String = row.get(3)?;
            let cookies: String = row.get(4)?;
            Ok(Visit {
                peer: row.get(0)?,
                host: row.get(1)?,
                port: row.get::<_, i64>(2)?.try_into().unwrap_or(0),
                keys: serde_json::from_str(&keys).unwrap_or_default(),
                cookies: serde_json::from_str(&cookies).unwrap_or_default(),
                hidden: row.get(5)?,
                opened: row.get(6)?,
                ended: row.get(7)?,
            })
        })
        .optional();
    found.unwrap_or_else(|error| {
        tracing::error!(%error, "couldn't read a shared website's listener");
        None
    })
}

/// The share as this side keeps it, made the first time it's asked for:
/// its hostname, and the port it means to listen at (nothing listens until
/// the person opens it).
fn ensure(factory: &Factory, share: &str, peer: &str, port: u16) -> Option<Visit> {
    if let Some(found) = visit(factory, share) {
        return Some(found);
    }
    let port = choose_port(factory, peer, port).unwrap_or(0);
    let host = format!("s-{}", random_label());
    let added = factory.store.db().execute(
        "INSERT OR IGNORE INTO pipe_share_visits (share, peer, host, port) VALUES (?1, ?2, ?3, ?4)",
        params![share, peer, host, port],
    );
    if let Err(error) = added {
        tracing::error!(%error, "couldn't keep a shared website's address");
    }
    visit(factory, share)
}

/// What the app shows of a share it receives (`share`, this side's id for
/// it, from `peer`, the sharer's `port`), paused or not.
pub(crate) fn status(factory: &Factory, share: &str, peer: &str, port: u16) -> Status {
    let Some(visit) = ensure(factory, share, peer, port) else { return Status::default() };
    let listening = factory.share_listeners.live.lock().unwrap_or_else(std::sync::PoisonError::into_inner).contains_key(share);
    if visit.port == 0 {
        return Status { problem: "No port is free on this computer.".to_owned(), hidden: visit.hidden, ..Status::default() };
    }
    Status {
        address: format!("http://{}.localhost:{}/", visit.host, visit.port),
        fallback_address: format!("http://127.0.0.1:{}/", visit.port),
        listening,
        problem: String::new(),
        hidden: visit.hidden,
    }
}

/// The person hides the share's chip, or shows it again.
pub(crate) fn set_hidden(factory: &Factory, share: &str, hidden: bool) -> Result<(), String> {
    let changed = factory
        .store
        .db()
        .execute("UPDATE pipe_share_visits SET hidden = ?2 WHERE share = ?1", params![share, hidden])
        .map_err(|error| error.to_string())?;
    if changed == 0 {
        return Err("That website isn't shared with you.".to_owned());
    }
    factory.broadcast_workspace(&factory.lock());
    Ok(())
}

/// Whether this computer opens a website shared with it at `port`: such a
/// port can't be shared back.
pub(crate) fn serves(factory: &Factory, port: u16) -> bool {
    let db = factory.store.db();
    db.query_row("SELECT EXISTS (SELECT 1 FROM pipe_share_visits WHERE port = ?1 AND ended = 0)", [port], |row| row.get(0)).unwrap_or(false)
}

/// The person opens a share: its listener starts if it isn't up, and a
/// one-time address is made for the browser.
pub(crate) fn open(factory: &Arc<Factory>, share: &str) -> Result<Opened, String> {
    let Some(received) = super::shares::received(factory, share) else {
        return Err("This website isn't shared with you now.".to_owned());
    };
    ensure(factory, share, &received.peer_key, received.port);
    let gate = start(factory, share)?;
    let code = gate.mint();
    let now = crate::transcript::now_ms();
    let _ = factory.store.db().execute("UPDATE pipe_share_visits SET opened = ?2 WHERE share = ?1 AND opened = 0", params![share, now]);
    factory.broadcast_workspace(&factory.lock());
    Ok(Opened {
        url: format!("http://{}.localhost:{}{OPEN_PATH}?k={code}", gate.host, gate.port),
        fallback_url: format!("http://127.0.0.1:{}{OPEN_PATH}?k={code}", gate.port),
    })
}

/// Its pipe paused: the listener closes, with every connection it has,
/// until `resumed`. Its address and cookies stay.
pub(crate) fn close(factory: &Factory, share: &str) {
    let live = factory.share_listeners.live.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(share);
    if let Some(live) = live {
        live.cancel.cancel();
        tracing::info!(share, port = live.gate.port, "closed a shared website's listener");
    }
}

/// Its pipe resumed, with the same key and no account change (the sharing
/// side checked): it listens again at its address if it had been opened.
/// Called as the resume is recorded, before the pipe shows open again, so
/// it doesn't wait for that: every stream to the sharer is checked anyway.
pub(crate) fn resumed(factory: &Arc<Factory>, share: &str) {
    let opened = visit(factory, share).is_some_and(|visit| visit.opened != 0 && visit.ended == 0);
    if opened && let Err(why) = start(factory, share) {
        tracing::warn!(share, %why, "a shared website couldn't listen again");
    }
}

/// The share ended for good (Stop on either side, its pipe closed, the
/// peer's identity changed): its listener closes, with every connection
/// it has, and its address is never used again.
pub(crate) fn end(factory: &Factory, share: &str) {
    close(factory, share);
    let now = crate::transcript::now_ms();
    let _ = factory.store.db().execute("UPDATE pipe_share_visits SET ended = ?2 WHERE share = ?1 AND ended = 0", params![share, now]);
}

/// At start: the shares that were opened, and are live, listen again at
/// their addresses (a paused one waits for `resumed`).
pub(crate) fn restore(factory: &Arc<Factory>) {
    let opened: Vec<String> = {
        let db = factory.store.db();
        let Ok(mut statement) = db.prepare("SELECT share FROM pipe_share_visits WHERE opened != 0 AND ended = 0") else { return };
        statement.query_map([], |row| row.get(0)).map(|rows| rows.flatten().collect()).unwrap_or_default()
    };
    for share in opened {
        if super::shares::received(factory, &share).is_some() {
            resumed(factory, &share);
        }
    }
}

/// The share's listener, started if it isn't up.
fn start(factory: &Arc<Factory>, share: &str) -> Result<Arc<Gate>, String> {
    if let Some(live) = factory.share_listeners.live.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(share) {
        return Ok(live.gate.clone());
    }
    let Some(visit) = visit(factory, share) else {
        return Err("This website isn't shared with you any more.".to_owned());
    };
    if visit.ended != 0 {
        return Err("This website isn't shared with you any more.".to_owned());
    }
    // Its own port when it can have it; else another, never one another
    // peer's share had.
    let (listener, port) = match bind(visit.port) {
        Some(listener) if visit.port != 0 && !own_port(factory, visit.port) => (listener, visit.port),
        _ => {
            let port = choose_port(factory, &visit.peer, 0).ok_or("No port is free on this computer.")?;
            let listener = bind(port).ok_or("No port is free on this computer.")?;
            let _ = factory.store.db().execute("UPDATE pipe_share_visits SET port = ?2 WHERE share = ?1", params![share, port]);
            if visit.port != 0 {
                tracing::info!(share, was = visit.port, port, "a shared website's port was taken here; it moved");
            }
            (listener, port)
        }
    };
    let gate = Arc::new(Gate {
        share: share.to_owned(),
        host: visit.host,
        port,
        keys: Mutex::new(visit.keys),
        cookies: Mutex::new(visit.cookies),
        codes: Mutex::new(Vec::new()),
        save: Box::new(saver(Arc::downgrade(factory), share.to_owned())),
    });
    let cancel = CancellationToken::new();
    let weak = Arc::downgrade(factory);
    let id = share.to_owned();
    let connect: Connect = Arc::new(move || {
        let weak = weak.clone();
        let id = id.clone();
        Box::pin(async move {
            let Some(factory) = weak.upgrade() else { return Err("Claktory is stopping.".to_owned()) };
            let (send, receive) = super::shares::stream(&factory, &id).await?;
            Ok(Box::new(tokio::io::join(receive, send)) as Box<dyn Io>)
        })
    });
    tokio::spawn(serve(listener, gate.clone(), connect, cancel.clone()));
    tracing::info!(share, port, "a shared website listens");
    factory.share_listeners.live.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(share.to_owned(), Live { gate: gate.clone(), cancel });
    Ok(gate)
}

/// Keeps a gate's key cookies and the website's cookie names.
fn saver(factory: Weak<Factory>, share: String) -> impl Fn(&Gate) + Send + Sync {
    move |gate: &Gate| {
        let Some(factory) = factory.upgrade() else { return };
        let keys = serde_json::to_string(&*gate.keys.lock().unwrap_or_else(std::sync::PoisonError::into_inner)).unwrap_or_default();
        let cookies = serde_json::to_string(&*gate.cookies.lock().unwrap_or_else(std::sync::PoisonError::into_inner)).unwrap_or_default();
        let saved =
            factory.store.db().execute("UPDATE pipe_share_visits SET keys = ?2, cookies = ?3 WHERE share = ?1", params![share, keys, cookies]);
        if let Err(error) = saved {
            tracing::error!(%error, "couldn't keep a shared website's cookies");
        }
    }
}

/// Claktory's own ports: its API's, the dev one's, the tunnel's.
fn own_port(factory: &Factory, port: u16) -> bool {
    [Some(7420), Some(7421), factory.remote.listener_port()].contains(&Some(port))
}

/// A port for a share from `peer`: `wanted` when it's free here, else one
/// the system gives; never Claktory's own, nor one another peer's share
/// had, so what a browser keeps for one origin never meets another peer's
/// pages.
fn choose_port(factory: &Factory, peer: &str, wanted: u16) -> Option<u16> {
    let others: BTreeSet<u16> = {
        let db = factory.store.db();
        let mut statement = db.prepare("SELECT port FROM pipe_share_visits WHERE peer != ?1").ok()?;
        statement.query_map([peer], |row| row.get::<_, i64>(0)).ok()?.flatten().filter_map(|port| u16::try_from(port).ok()).collect()
    };
    let usable = |port: u16| port != 0 && !own_port(factory, port) && !others.contains(&port) && bind(port).is_some();
    // The sharer's number only above 1023: a peer never steers this side
    // onto a well-known port (22, 80, 443) to hold it while the person's
    // own service there is down (Fugo, #154).
    if wanted >= 1024 && usable(wanted) {
        return Some(wanted);
    }
    (0..32).find_map(|_| {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok()?;
        let port = listener.local_addr().ok()?.port();
        drop(listener);
        usable(port).then_some(port)
    })
}

/// A listener on 127.0.0.1 at `port`, never any other address.
fn bind(port: u16) -> Option<TcpListener> {
    let listener = std::net::TcpListener::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port)).ok()?;
    listener.set_nonblocking(true).ok()?;
    TcpListener::from_std(listener).ok()
}

/// A random DNS label: 20 lowercase letters and digits (100 bits).
fn random_label() -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut bytes = [0_u8; 20];
    getrandom::fill(&mut bytes).expect("the system's random source");
    bytes.iter().map(|byte| ALPHABET[usize::from(byte % 32)] as char).collect()
}

/// 32 random bytes, base64url.
fn random_token() -> String {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).expect("the system's random source");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Equal, in a time that doesn't depend on where they differ.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0_u8, |diff, (a, b)| diff | (a ^ b)) == 0
}

// ---- The listener ----

/// The sharer's side of one browser connection: bytes both ways.
pub(crate) trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// Opens a stream to the sharer, or says why it can't, for the person.
pub(crate) type Connect = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<Box<dyn Io>, String>> + Send>> + Send + Sync>;

type Body = Either<Incoming, Full<Bytes>>;

/// What a share's requests are checked against.
pub(crate) struct Gate {
    share: String,
    /// `s-<random>`.
    host: String,
    port: u16,
    /// SHA-256s of the key cookies given out, the latest last.
    keys: Mutex<Vec<String>>,
    /// The cookies the shared website set, by name.
    cookies: Mutex<BTreeSet<String>>,
    /// One-time codes not used yet: their SHA-256s and when they stop working.
    codes: Mutex<Vec<(String, Instant)>>,
    save: Box<dyn Fn(&Gate) + Send + Sync>,
}

/// Which of its two hosts a request named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Named {
    /// `s-<random>.localhost:<port>`.
    Own,
    /// `127.0.0.1:<port>`, Safari's.
    Fallback,
}

impl Gate {
    /// A one-time code for Open.
    fn mint(&self) -> String {
        let code = random_token();
        let mut codes = self.codes.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        codes.retain(|(_, until)| *until > now);
        codes.push((sha256_hex(&code), now + CODE_LIFE));
        code
    }

    /// Uses a one-time code up: whether it was one, and still worked.
    fn redeem(&self, code: &str) -> bool {
        let hash = sha256_hex(code);
        let mut codes = self.codes.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        codes.retain(|(_, until)| *until > now);
        let found = codes.iter().position(|(kept, _)| same(kept, &hash));
        found.map(|at| codes.remove(at)).is_some()
    }

    /// A new key cookie's value, kept.
    fn new_key(&self) -> String {
        let key = random_token();
        {
            let mut keys = self.keys.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            keys.push(sha256_hex(&key));
            let over = keys.len().saturating_sub(KEYS_KEPT);
            keys.drain(..over);
        }
        (self.save)(self);
        key
    }

    fn has_key(&self, key: &str) -> bool {
        let hash = sha256_hex(key);
        self.keys.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().fold(false, |found, kept| found | same(kept, &hash))
    }

    /// The key cookie's name: the share's own, since 127.0.0.1's cookies
    /// are the same for every port.
    fn cookie_name(&self) -> String {
        format!("claktory_{}", self.host.replace('-', "_"))
    }

    /// Which of the share's hosts `host` is, exactly; None for any other.
    fn named(&self, host: Option<&str>) -> Option<Named> {
        let host = host?.to_ascii_lowercase();
        if host == format!("{}.localhost:{}", self.host, self.port) {
            Some(Named::Own)
        } else if host == format!("127.0.0.1:{}", self.port) {
            Some(Named::Fallback)
        } else {
            None
        }
    }

    /// The website set these cookies: remembered for Safari's filter.
    fn learn(&self, names: &[String]) {
        let added = {
            let mut cookies = self.cookies.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let before = cookies.len();
            for name in names {
                if cookies.len() < COOKIES_KEPT {
                    cookies.insert(name.clone());
                }
            }
            cookies.len() != before
        };
        if added {
            (self.save)(self);
        }
    }
}

/// Serves a share's listener until the share ends.
pub(crate) async fn serve(listener: TcpListener, gate: Arc<Gate>, connect: Connect, cancel: CancellationToken) {
    let room = Arc::new(tokio::sync::Semaphore::new(CONNECTIONS));
    loop {
        let accepted = tokio::select! {
            () = cancel.cancelled() => return,
            accepted = listener.accept() => accepted,
        };
        let Ok((socket, _)) = accepted else { continue };
        let Ok(permit) = room.clone().try_acquire_owned() else {
            drop(socket);
            continue;
        };
        let (gate, connect, cancel) = (gate.clone(), connect.clone(), cancel.clone());
        tokio::spawn(async move {
            let _permit = permit;
            let upstream: Upstream = Arc::new(tokio::sync::Mutex::new(None));
            let service = {
                let (gate, connect, cancel) = (gate.clone(), connect.clone(), cancel.clone());
                hyper::service::service_fn(move |request| {
                    handle(gate.clone(), connect.clone(), upstream.clone(), cancel.clone(), request)
                })
            };
            let connection = hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket), service).with_upgrades();
            tokio::select! {
                () = cancel.cancelled() => {}
                _ = connection => {}
            }
        });
    }
}

/// One browser connection's stream to the sharer, made at its first request.
type Upstream = Arc<tokio::sync::Mutex<Option<hyper::client::conn::http1::SendRequest<Incoming>>>>;

async fn handle(
    gate: Arc<Gate>,
    connect: Connect,
    upstream: Upstream,
    cancel: CancellationToken,
    mut request: Request<Incoming>,
) -> Result<Response<Body>, Infallible> {
    let host = request.headers().get(header::HOST).and_then(|host| host.to_str().ok());
    let Some(named) = gate.named(host) else {
        return Ok(page(StatusCode::MISDIRECTED_REQUEST, "Open this website from Claktory."));
    };
    if !same_origin(request.headers()) {
        return Ok(page(StatusCode::FORBIDDEN, "Open this website from Claktory."));
    }
    if request.uri().path() == OPEN_PATH {
        let code = request.uri().query().and_then(|query| query.split('&').find_map(|pair| pair.strip_prefix("k="))).unwrap_or("");
        if !gate.redeem(code) {
            return Ok(page(StatusCode::FORBIDDEN, "This link has been used or is too old. Open the website again from Claktory."));
        }
        let cookie = format!("{}={}; HttpOnly; SameSite=Strict; Path=/; Max-Age={KEY_AGE}", gate.cookie_name(), gate.new_key());
        let mut response = Response::new(Either::Right(Full::new(Bytes::new())));
        *response.status_mut() = StatusCode::SEE_OTHER;
        let headers = response.headers_mut();
        headers.insert(header::LOCATION, HeaderValue::from_static("/"));
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
        if let Ok(cookie) = HeaderValue::from_str(&cookie) {
            headers.insert(header::SET_COOKIE, cookie);
        }
        return Ok(response);
    }
    let (has_key, cookie) = cookies(&gate, named, request.headers());
    if !has_key {
        return Ok(page(StatusCode::FORBIDDEN, "Open this website from Claktory."));
    }
    let headers = request.headers_mut();
    headers.remove(header::COOKIE);
    if let Some(cookie) = cookie.and_then(|cookie| HeaderValue::from_str(&cookie).ok()) {
        headers.insert(header::COOKIE, cookie);
    }

    let mut held = upstream.lock().await;
    if held.as_ref().is_none_or(|sender| sender.is_closed()) {
        let io = match connect().await {
            Ok(io) => io,
            Err(why) => {
                tracing::info!(share = %gate.share, %why, "a shared website couldn't be reached");
                return Ok(page(StatusCode::BAD_GATEWAY, &why));
            }
        };
        let (sender, connection) = match hyper::client::conn::http1::handshake(TokioIo::new(io)).await {
            Ok(made) => made,
            Err(_) => return Ok(page(StatusCode::BAD_GATEWAY, "The shared website couldn't be reached.")),
        };
        let cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = cancel.cancelled() => {}
                _ = connection.with_upgrades() => {}
            }
        });
        *held = Some(sender);
    }
    let Some(sender) = held.as_mut() else { return Ok(page(StatusCode::BAD_GATEWAY, "The shared website couldn't be reached.")) };
    let upgrading = request.headers().contains_key(header::UPGRADE);
    let browser = upgrading.then(|| hyper::upgrade::on(&mut request));
    if sender.ready().await.is_err() {
        *held = None;
        return Ok(page(StatusCode::BAD_GATEWAY, "The shared website couldn't be reached."));
    }
    let mut response = match sender.send_request(request).await {
        Ok(response) => response,
        Err(_) => {
            *held = None;
            return Ok(page(StatusCode::BAD_GATEWAY, "The shared website stopped answering."));
        }
    };
    // Its cookies: never one named as Claktory's, and each for the share's
    // own host alone (no Domain, which could reach other shares' hosts
    // under .localhost); remembered for Safari's filter.
    let mut names = Vec::new();
    let set: Vec<HeaderValue> = response.headers().get_all(header::SET_COOKIE).iter().cloned().collect();
    response.headers_mut().remove(header::SET_COOKIE);
    for value in set {
        let Some(cookie) = value.to_str().ok().and_then(host_only) else { continue };
        let Some((name, _)) = cookie.split_once('=') else { continue };
        let name = name.trim().to_owned();
        if name.to_ascii_lowercase().starts_with("claktory_") {
            continue;
        }
        if let Ok(value) = HeaderValue::from_str(&cookie) {
            names.push(name);
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    gate.learn(&names);
    // On 127.0.0.1 it would wipe every local service's cookies and storage.
    response.headers_mut().remove("clear-site-data");
    if response.status() == StatusCode::SWITCHING_PROTOCOLS
        && let Some(browser) = browser
    {
        // A WebSocket, say: the stream is the upgrade's now, both ways.
        let sharer = hyper::upgrade::on(&mut response);
        *held = None;
        tokio::spawn(async move {
            let (Ok(browser), Ok(sharer)) = tokio::join!(browser, sharer) else { return };
            let (mut browser, mut sharer) = (TokioIo::new(browser), TokioIo::new(sharer));
            tokio::select! {
                () = cancel.cancelled() => {}
                _ = tokio::io::copy_bidirectional(&mut browser, &mut sharer) => {}
            }
        });
    }
    Ok(response.map(Either::Left))
}

/// Whether a request comes from the share's own pages, or from the person
/// (Claktory's Open, a bookmark, the address typed): never from another
/// site's (Fugo, #154). Every 127.0.0.1 port is the same site to a browser,
/// so another share's page there, or any page another program serves on
/// it, would otherwise send this share's SameSite=Strict key along. So a
/// request a browser marks same-site or cross-site is refused, and so is
/// one whose Origin isn't the very host it names (browsers send Origin on
/// WebSocket handshakes, POSTs and CORS requests, Safari too); an upgrade
/// must carry one.
fn same_origin(headers: &hyper::HeaderMap) -> bool {
    let site = headers.get("sec-fetch-site").and_then(|site| site.to_str().ok()).map(str::to_ascii_lowercase);
    if matches!(site.as_deref(), Some("same-site" | "cross-site")) {
        return false;
    }
    let host = headers.get(header::HOST).and_then(|host| host.to_str().ok()).unwrap_or("").to_ascii_lowercase();
    match headers.get(header::ORIGIN).map(|origin| origin.to_str().map(str::to_ascii_lowercase)) {
        Some(Ok(origin)) => origin == format!("http://{host}"),
        Some(Err(_)) => false,
        None => !headers.contains_key(header::UPGRADE),
    }
}

/// Whether the request carries the share's key cookie, and the cookies to
/// pass on: never the key; on 127.0.0.1 only those the website set.
fn cookies(gate: &Gate, named: Named, headers: &hyper::HeaderMap) -> (bool, Option<String>) {
    let name = gate.cookie_name();
    let set = gate.cookies.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    let mut has_key = false;
    let mut kept = Vec::new();
    for value in headers.get_all(header::COOKIE) {
        let Ok(value) = value.to_str() else { continue };
        for pair in value.split(';').map(str::trim).filter(|pair| !pair.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            if key == name {
                has_key |= gate.has_key(value);
            } else if key.to_ascii_lowercase().starts_with("claktory_") {
                // Another share's key: never passed on.
            } else if named == Named::Own || set.contains(key) {
                kept.push(pair);
            }
        }
    }
    (has_key, (!kept.is_empty()).then(|| kept.join("; ")))
}

/// A Set-Cookie value without its Domain attribute: the cookie stays the
/// host's that set it.
fn host_only(cookie: &str) -> Option<String> {
    let mut parts = cookie.split(';');
    let first = parts.next()?.trim();
    let rest = parts.map(str::trim).filter(|part| !part.to_ascii_lowercase().starts_with("domain"));
    Some(std::iter::once(first).chain(rest).collect::<Vec<_>>().join("; "))
}

/// A short page of Claktory's own.
fn page(status: StatusCode, text: &str) -> Response<Body> {
    let escaped = text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let html = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Claktory</title>\
         <body style=\"font-family: -apple-system, sans-serif; margin: 4em auto; max-width: 32em\"><p>{escaped}</p></body>"
    );
    let mut response = Response::new(Either::Right(Full::new(Bytes::from(html))));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests;
