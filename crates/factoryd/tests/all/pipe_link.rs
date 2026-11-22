//! The pipes' transport (#196): two Factories, an invite and a join, the
//! same four digits on both sides, approval from both people before
//! anything flows, signed messages with files delivered in order and
//! acked, refusals heard back, the queue while the peer is away, close.

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use factory_protocol::v1::{self, client_message::Body as Request};
use factoryd::Factory;
use factoryd::pipe_link::{self, Incoming, LinkStatus, OutFile, Outgoing, Peer, PipeError, Side, Via};

use crate::common::{Client, View, apply_request, temp_dir};

/// What the transport told the store, tagged with the Factory's home so
/// every test (and both Factories) can share one registered store.
#[derive(Clone, Debug)]
enum Event {
    Knocked { pipe: String, peer: Peer, digits: String },
    PeerApproved { pipe: String },
    Received { pipe: String, message: Incoming },
    Delivered { pipe: String, msg_id: String },
    Refused { pipe: String, msg_id: String, reason: String },
    PeerClosed { pipe: String },
}

fn events() -> &'static Mutex<Vec<(String, Event)>> {
    static EVENTS: OnceLock<Mutex<Vec<(String, Event)>>> = OnceLock::new();
    EVENTS.get_or_init(Mutex::default)
}

fn record(factory: &Arc<Factory>, event: Event) {
    events().lock().expect("events").push((factory.home().root().display().to_string(), event));
}

struct TestStore;

impl pipe_link::Store for TestStore {
    fn knocked(&self, factory: &Arc<Factory>, pipe: &str, peer: Peer, digits: String) {
        record(factory, Event::Knocked { pipe: pipe.to_owned(), peer, digits });
    }
    fn peer_approved(&self, factory: &Arc<Factory>, pipe: &str) {
        record(factory, Event::PeerApproved { pipe: pipe.to_owned() });
    }
    fn received(&self, factory: &Arc<Factory>, pipe: &str, message: Incoming) -> Result<(), String> {
        if message.text.starts_with("refuse:") {
            return Err("no thanks".to_owned());
        }
        record(factory, Event::Received { pipe: pipe.to_owned(), message });
        Ok(())
    }
    fn delivered(&self, factory: &Arc<Factory>, pipe: &str, msg_id: &str) {
        record(factory, Event::Delivered { pipe: pipe.to_owned(), msg_id: msg_id.to_owned() });
    }
    fn refused(&self, factory: &Arc<Factory>, pipe: &str, msg_id: &str, reason: &str) {
        record(factory, Event::Refused { pipe: pipe.to_owned(), msg_id: msg_id.to_owned(), reason: reason.to_owned() });
    }
    fn peer_paused(&self, _factory: &Arc<Factory>, _pipe: &str) {}
    fn peer_resumed(&self, _factory: &Arc<Factory>, _pipe: &str) {}
    fn peer_closed(&self, factory: &Arc<Factory>, pipe: &str) {
        record(factory, Event::PeerClosed { pipe: pipe.to_owned() });
    }
    fn link_changed(&self, _factory: &Arc<Factory>, _pipe: &str, _status: LinkStatus) {}
}

/// Waits for an event of this Factory's passing `pick`; what it picked.
async fn wait_for<T>(factory: &Arc<Factory>, pick: impl Fn(&Event) -> Option<T>) -> T {
    let home = factory.home().root().display().to_string();
    // Generous: riding out a stale connection costs a timeout and a redial.
    for _ in 0..1200 {
        {
            let events = events().lock().expect("events");
            if let Some(found) = events.iter().filter(|(of, _)| *of == home).find_map(|(_, event)| pick(event)) {
                return found;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let seen: Vec<_> = events().lock().expect("events").iter().filter(|(of, _)| *of == home).map(|(_, e)| e.clone()).collect();
    panic!("the event never came; saw {seen:?}");
}

/// A Factory with its tunnel quick and its relay dead, so nothing uses the
/// network, and the test store registered.
async fn claktory(dir: &Path) -> Arc<Factory> {
    pipe_link::set_store(Box::new(TestStore));
    let (addr, factory) = crate::common::start(dir).await;
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let relays = v1::SetRemoteAccess { relays: vec!["http://127.0.0.1:9/".into()], set_relays: true, ..Default::default() };
    apply_request(&mut client, &mut view, Request::SetRemoteAccess(relays)).await;
    factory
}

fn side(name: &str) -> Side {
    Side {
        claktory: format!("{name}'s Claktory"),
        owner_name: name.to_owned(),
        owner_email: format!("{name}@example.com"),
        project: "Site".to_owned(),
        pipe_name: "Site ⇄ Marketing".to_owned(),
        purpose: "copy and launch dates".to_owned(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_claktories_pair_approve_and_talk_through_a_pipe() {
    let (dir_a, dir_b) = (temp_dir(), temp_dir());
    let a = claktory(&dir_a).await;
    let b = claktory(&dir_b).await;

    // Matt invites; the code says who without dialing.
    let (code, expires) = pipe_link::invite(&a, "p-a", &side("Matt")).await.expect("a code");
    assert!(code.starts_with("claktory-pipe://"), "{code}");
    assert!(expires > factoryd::transcript::now_ms());
    assert_eq!(pipe_link::read_invite(&code).expect("readable").owner_name, "Matt");

    // Ana joins with it: both sides now show the same digits.
    let (peer, digits_b) = pipe_link::join(&b, "p-b", &code, &side("Ana")).await.expect("joined");
    assert_eq!((peer.side.owner_name.as_str(), peer.verified), ("Matt", false));
    let (knocked, digits_a) = wait_for(&a, |event| match event {
        Event::Knocked { pipe, peer, digits } if pipe == "p-a" => Some((peer.clone(), digits.clone())),
        _ => None,
    })
    .await;
    assert_eq!(knocked.side.owner_name, "Ana");
    assert_eq!(digits_a, digits_b);
    assert_eq!(digits_a.len(), 4);

    // The code is used up.
    let again = pipe_link::join(&b, "p-b2", &code, &side("Ana")).await;
    assert!(matches!(again, Err(PipeError::Exists)), "{again:?}");
    let dir_c = temp_dir();
    let c = claktory(&dir_c).await;
    let stranger = pipe_link::join(&c, "p-c", &code, &side("Eve")).await;
    assert!(matches!(stranger, Err(PipeError::Used)), "{stranger:?}");

    // Nothing flows until both people approved.
    let note = |id: &str, text: &str| Outgoing {
        msg_id: id.to_owned(),
        via: Via::Person { name: "Matt".into() },
        text: text.to_owned(),
        files: Vec::new(),
    };
    assert!(matches!(pipe_link::send(&a, "p-a", note("m0", "hi")), Err(PipeError::NotOpen)));
    pipe_link::approve(&b, "p-b").expect("Ana approves");
    wait_for(&a, |event| matches!(event, Event::PeerApproved { pipe } if pipe == "p-a").then_some(())).await;
    assert!(matches!(pipe_link::send(&b, "p-b", note("m0", "hi")), Err(PipeError::NotOpen)), "Ana approved alone");
    pipe_link::approve(&a, "p-a").expect("Matt approves");
    wait_for(&b, |event| matches!(event, Event::PeerApproved { pipe } if pipe == "p-b").then_some(())).await;

    // A message with a file, signed, delivered once, acked.
    let report = dir_a.join("report.txt");
    std::fs::write(&report, b"all green").expect("written");
    let message = Outgoing {
        msg_id: "m1".to_owned(),
        via: Via::Agent { name: "Lea".into(), role: "Copywriter".into() },
        text: "the launch report".to_owned(),
        files: vec![OutFile { name: "report.txt".into(), path: report.clone() }],
    };
    assert_eq!(pipe_link::send(&a, "p-a", message).expect("queued"), 1);
    // The queue holds the bytes: the file may go at once.
    std::fs::remove_file(&report).expect("removed");
    let got = wait_for(&b, |event| match event {
        Event::Received { pipe, message } if pipe == "p-b" => Some(message.clone()),
        _ => None,
    })
    .await;
    assert_eq!((got.msg_id.as_str(), got.seq, got.text.as_str()), ("m1", 1, "the launch report"));
    assert_eq!(got.via, Via::Agent { name: "Lea".into(), role: "Copywriter".into() });
    assert_eq!(got.files.len(), 1);
    assert_eq!(std::fs::read(&got.files[0].path).expect("the file"), b"all green");
    wait_for(&a, |event| matches!(event, Event::Delivered { pipe, msg_id } if pipe == "p-a" && msg_id == "m1").then_some(())).await;

    // The other way too, and a refusal is heard back.
    pipe_link::send(&b, "p-b", note("n1", "thanks")).expect("queued");
    wait_for(&a, |event| matches!(event, Event::Received { pipe, message } if pipe == "p-a" && message.msg_id == "n1").then_some(())).await;
    pipe_link::send(&b, "p-b", note("n2", "refuse: this one")).expect("queued");
    let reason = wait_for(&b, |event| match event {
        Event::Refused { pipe, msg_id, reason } if pipe == "p-b" && msg_id == "n2" => Some(reason.clone()),
        _ => None,
    })
    .await;
    assert_eq!(reason, "no thanks");

    // Close: Ana's side hears it, and both links are gone, so a new code
    // for the same pipe works again.
    pipe_link::close(&a, "p-a");
    wait_for(&b, |event| matches!(event, Event::PeerClosed { pipe } if pipe == "p-b").then_some(())).await;
    for _ in 0..100 {
        if pipe_link::invite(&a, "p-a", &side("Matt")).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the closed pipe never made way for a new code");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_waits_out_the_peer_being_away_and_a_wrong_secret_gets_nothing() {
    let (dir_a, dir_b) = (temp_dir(), temp_dir());
    let a = claktory(&dir_a).await;
    let b = claktory(&dir_b).await;
    let (code, _) = pipe_link::invite(&a, "pipe", &side("Matt")).await.expect("a code");

    // A code with the wrong secret is turned away.
    let forged = code.replace("&s=", "&s=00").replacen("s=00", "s=", 1);
    let mut wrong: Vec<char> = code.chars().collect();
    let at = code.find("&s=").expect("the secret") + 3;
    wrong[at] = if wrong[at] == 'a' { 'b' } else { 'a' };
    let wrong: String = wrong.into_iter().collect();
    drop(forged);
    let refused = pipe_link::join(&b, "pipe", &wrong, &side("Eve")).await;
    assert!(matches!(refused, Err(PipeError::BadCode)), "{refused:?}");

    pipe_link::join(&b, "pipe", &code, &side("Ana")).await.expect("joined");
    pipe_link::approve(&a, "pipe").expect("approved");
    pipe_link::approve(&b, "pipe").expect("approved");
    wait_for(&a, |event| matches!(event, Event::PeerApproved { pipe } if pipe == "pipe").then_some(())).await;
    wait_for(&b, |event| matches!(event, Event::PeerApproved { pipe } if pipe == "pipe").then_some(())).await;

    // Ana's Claktory goes away; Matt's message waits in the queue.
    drop(b);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let message =
        Outgoing { msg_id: "m1".to_owned(), via: Via::Person { name: "Matt".into() }, text: "still there?".to_owned(), files: Vec::new() };
    pipe_link::send(&a, "pipe", message).expect("queued");
    for _ in 0..100 {
        if pipe_link::status(&a, "pipe").queued == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(pipe_link::status(&a, "pipe").queued, 1);

    // Back, it arrives by itself.
    let b = claktory(&dir_b).await;
    wait_for(&b, |event| matches!(event, Event::Received { pipe, message } if pipe == "pipe" && message.msg_id == "m1").then_some(()))
        .await;
    wait_for(&a, |event| matches!(event, Event::Delivered { pipe, msg_id } if pipe == "pipe" && msg_id == "m1").then_some(())).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_join_that_cannot_reach_the_inviter_keeps_trying_until_it_can() {
    let (dir_a, dir_b) = (temp_dir(), temp_dir());
    let a = claktory(&dir_a).await;
    // Short dials, or every failed try would wait the whole 15 s.
    pipe_link::set_dial(Duration::from_millis(1500));
    let (code, _) = pipe_link::invite(&a, "p-a", &side("Matt")).await.expect("a code");

    // A gate between the two: closed, the joiner's packets go nowhere, as
    // when macOS's local-network prompt was denied (#246); open, they flow.
    let a_port = code_values(&code, "a")
        .first()
        .and_then(|address| address.parse::<std::net::SocketAddr>().ok())
        .expect("an address in the code")
        .port();
    let a_addr: std::net::SocketAddr = format!("127.0.0.1:{a_port}").parse().expect("the inviter's address");
    let open = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let proxy = tokio::net::UdpSocket::bind("127.0.0.1:0").await.expect("a proxy socket");
    let proxy_port = proxy.local_addr().expect("the proxy's address").port();
    {
        let open = open.clone();
        tokio::spawn(async move {
            let mut joiner: Option<std::net::SocketAddr> = None;
            let mut buf = vec![0u8; 65536];
            loop {
                let Ok((n, from)) = proxy.recv_from(&mut buf).await else { return };
                if !open.load(std::sync::atomic::Ordering::Relaxed) {
                    continue;
                }
                if from == a_addr {
                    if let Some(to) = joiner {
                        let _ = proxy.send_to(&buf[..n], to).await;
                    }
                } else {
                    joiner = Some(from);
                    let _ = proxy.send_to(&buf[..n], a_addr).await;
                }
            }
        });
    }

    // The code the joiner gets points only at the proxy.
    let kept: Vec<&str> = code.split('&').filter(|part| !part.starts_with("a=")).collect();
    let doctored = format!("{}&a=127.0.0.1%3A{proxy_port}", kept.join("&"));

    // The joiner is a full Claktory: the retry under test lives in the
    // pipes layer behind AcceptPipeInvite, above pipe_link::join.
    let (addr_b, _b) = crate::common::start(&dir_b).await;
    _b.set_tunnel_online_wait(Duration::from_millis(100));
    let (mut client, workspace, _) = Client::connect(addr_b).await;
    let mut view = View { workspace, ..View::default() };
    let dead_relay = v1::SetRemoteAccess { relays: vec!["http://127.0.0.1:9/".into()], set_relays: true, ..Default::default() };
    apply_request(&mut client, &mut view, Request::SetRemoteAccess(dead_relay.clone())).await;
    apply_request(&mut client, &mut view, crate::common::create_project("Site", &dir_b.join("site"))).await;
    let project = view.workspace.projects.first().expect("the project").id.clone();
    let accept = v1::AcceptPipeInvite {
        code: doctored,
        project_id: project,
        name: String::new(),
        purpose: "testing".to_owned(),
        ..Default::default()
    };
    apply_request(&mut client, &mut view, Request::AcceptPipeInvite(accept)).await;
    view.follow(&mut client, |view| view.workspace.pipes.iter().any(|pipe| pipe.state() == v1::PipeState::Joining)).await;

    // Dials fail while the gate is closed; that must not close the pipe.
    // Before the fix, the first failed dial closed it for good.
    tokio::time::sleep(Duration::from_secs(4)).await;
    apply_request(&mut client, &mut view, Request::SetRemoteAccess(dead_relay)).await;
    let pipe = view.workspace.pipes.first().expect("the pipe");
    assert_eq!(pipe.state(), v1::PipeState::Joining, "a failed dial should leave the pipe joining and retrying");
    assert!(!pipe.offline_reason.is_empty() && pipe.offline_since_ms > 0, "the pipe says why it can't connect yet: {pipe:?}");

    // The way opens (the person allowed the local network, say): the next
    // retry connects, and both sides reach the digits step.
    open.store(true, std::sync::atomic::Ordering::Relaxed);
    tokio::time::timeout(
        Duration::from_secs(60),
        view.follow(&mut client, |view| view.workspace.pipes.iter().any(|pipe| pipe.state() == v1::PipeState::Approval)),
    )
    .await
    .expect("the retried join should connect once the way is open");
    let pipe = view.workspace.pipes.first().expect("the pipe");
    assert!(pipe.offline_reason.is_empty() && pipe.offline_since_ms == 0, "connected, the reason clears: {pipe:?}");
    wait_for(&a, |event| matches!(event, Event::Knocked { pipe, .. } if pipe == "p-a").then_some(())).await;
}

// -- Hostile-peer regressions: a pipe's other end is an outside party --

/// What a frame's signature covers (pipe_link's LABEL; a change there is
/// a wire change and should fail here loudly).
const LABEL: &[u8] = b"claktory pipe frame v1\n";

fn pct_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%'
            && at + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&text[at + 1..at + 3], 16)
        {
            out.push(byte);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The values of `name` in an invite code's query.
fn code_values(code: &str, name: &str) -> Vec<String> {
    code.trim().split(['?', '&']).filter_map(|part| part.strip_prefix(name)?.strip_prefix('=')).map(pct_decode).collect()
}

fn code_value(code: &str, name: &str) -> String {
    code_values(code, name).into_iter().next().unwrap_or_default()
}

/// A frame as the wire has it: length, payload, signature — and no file
/// bytes, whatever the payload lists.
fn raw_frame(key: &iroh::SecretKey, payload: &serde_json::Value) -> Vec<u8> {
    let body = serde_json::to_vec(payload).expect("payload");
    let signature = key.sign(&[LABEL, body.as_slice()].concat());
    let mut out = u32::try_from(body.len()).expect("length").to_be_bytes().to_vec();
    out.extend_from_slice(&body);
    out.extend_from_slice(&signature.to_bytes());
    out
}

/// Sends a frame and reads the reply, leaving the stream open: a server
/// still waiting for the file bytes the payload lists would answer
/// nothing, and the timeout here would fail the test.
async fn ask(connection: &iroh::endpoint::Connection, frame: &[u8]) -> serde_json::Value {
    use tokio::io::AsyncReadExt;
    let (mut send, mut receive) = connection.open_bi().await.expect("a stream");
    tokio::io::AsyncWriteExt::write_all(&mut send, frame).await.expect("written");
    tokio::time::timeout(Duration::from_secs(10), async {
        let length = receive.read_u32().await.expect("a length") as usize;
        let mut body = vec![0; length];
        receive.read_exact(&mut body).await.expect("a body");
        serde_json::from_slice::<serde_json::Value>(&body).expect("json")
    })
    .await
    .expect("a prompt reply, not a server waiting for file bytes")
}

/// The link's stored relays and addresses, straight from factory.db.
fn stored_route(dir: &Path, pipe: &str) -> (String, String) {
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("db");
    db.query_row("SELECT relays, addresses FROM pipe_links WHERE pipe = ?1", [pipe], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })
    .expect("the link")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stranger_gets_no_buffering_and_a_peer_cannot_point_us_at_ourselves() {
    let dir_a = temp_dir();
    let a = claktory(&dir_a).await;
    let (code, _) = pipe_link::invite(&a, "p-a", &side("Matt")).await.expect("a code");
    let wire = code_value(&code, "w");
    let secret = code_value(&code, "s");

    // A client of our own, the key fresh: admitted only because the
    // invite waits.
    let key = iroh::SecretKey::generate();
    let client = iroh::endpoint::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .secret_key(key.clone())
        .bind()
        .await
        .expect("a client endpoint");
    let mut addr = iroh::EndpointAddr::new(code_value(&code, "e").parse().expect("an endpoint id"));
    for address in code_values(&code, "a") {
        if let Ok(address) = address.parse() {
            addr = addr.with_ip_addr(address);
        }
    }
    let connection = client.connect(addr, pipe_link::ALPN_PIPE).await.expect("connected");

    // Unpinned, a signed message frame listing a 10 MB file is turned
    // away before any file byte is read.
    let big_file = serde_json::json!([{"name": "big.bin", "size": 10 * 1024 * 1024, "sha256": "0".repeat(64)}]);
    let message = serde_json::json!({
        "v": 1, "wire": wire, "kind": "message", "seq": 1, "msg_id": "big", "text": "x", "sent_at": 1, "files": big_file,
    });
    assert_eq!(ask(&connection, &raw_frame(&key, &message)).await["code"], "bad");

    // The same key joins: pinned from here on.
    let join = serde_json::json!({
        "v": 1, "wire": wire, "kind": "join", "secret": secret, "sent_at": 1,
        "side": {"claktory": "Eve's Claktory", "owner_name": "Eve"}, "relays": [], "addresses": [],
    });
    assert_eq!(ask(&connection, &raw_frame(&key, &join)).await["code"], "ok");

    // Pinned, but a frame with files on another pipe's wire is turned
    // away before its files are read too.
    let (code2, _) = pipe_link::invite(&a, "p-a2", &side("Matt")).await.expect("a second code");
    let other = serde_json::json!({
        "v": 1, "wire": code_value(&code2, "w"), "kind": "message", "seq": 1, "msg_id": "big2", "text": "x", "sent_at": 1,
        "files": big_file,
    });
    assert_eq!(ask(&connection, &raw_frame(&key, &other)).await["code"], "bad");

    // A ping's relays and addresses are kept only when this side may
    // dial them: an https relay and a routable address land...
    let good = serde_json::json!({
        "v": 1, "wire": wire, "kind": "ping", "sent_at": 1,
        "relays": ["https://relay.example/"], "addresses": ["203.0.113.5:4433"],
    });
    assert_eq!(ask(&connection, &raw_frame(&key, &good)).await["code"], "ok");
    let (relays, addresses) = stored_route(&dir_a, "p-a");
    assert!(relays.contains("https://relay.example/"), "{relays}");
    assert!(addresses.contains("203.0.113.5:4433"), "{addresses}");

    // ...and an http relay with a loopback address changes nothing, the
    // loopback mapped into IPv6 included.
    let evil = serde_json::json!({
        "v": 1, "wire": wire, "kind": "ping", "sent_at": 1,
        "relays": ["http://127.0.0.1:9/"], "addresses": ["127.0.0.1:22", "[::ffff:127.0.0.1]:22"],
    });
    assert_eq!(ask(&connection, &raw_frame(&key, &evil)).await["code"], "ok");
    assert_eq!(stored_route(&dir_a, "p-a"), (relays, addresses), "the peer's http relay and loopback addresses were ignored");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_name_the_other_side_chose_never_says_claktory_in_claktorys_own_places() {
    let (dir_a, dir_b) = (temp_dir(), temp_dir());
    let a = claktory(&dir_a).await;
    let mut posing = side("Matt");
    posing.pipe_name = "CIaktory Security".to_owned();
    let (code, _) = pipe_link::invite(&a, "p-a", &posing).await.expect("a code");

    // The joiner, accepting without naming it, gets a name in Claktory's
    // words instead, in the preview and on the pipe.
    let (addr_b, b) = crate::common::start(&dir_b).await;
    b.set_tunnel_online_wait(Duration::from_millis(100));
    let (mut client, workspace, _) = Client::connect(addr_b).await;
    let mut view = View { workspace, ..View::default() };
    let dead_relay = v1::SetRemoteAccess { relays: vec!["http://127.0.0.1:9/".into()], set_relays: true, ..Default::default() };
    apply_request(&mut client, &mut view, Request::SetRemoteAccess(dead_relay)).await;
    apply_request(&mut client, &mut view, crate::common::create_project("Site", &dir_b.join("site"))).await;
    let preview = apply_request(&mut client, &mut view, Request::ReadPipeInvite(v1::ReadPipeInvite { code: code.clone() })).await;
    let factory_protocol::v1::server_message::Body::PipeInvitePreview(preview) = preview else { panic!("{preview:?}") };
    assert!(preview.name.starts_with("Pipe joined "), "{}", preview.name);
    let project = view.workspace.projects.first().expect("the project").id.clone();
    let accept = v1::AcceptPipeInvite { code, project_id: project, purpose: "testing".to_owned(), ..Default::default() };
    apply_request(&mut client, &mut view, Request::AcceptPipeInvite(accept)).await;
    view.follow(&mut client, |view| !view.workspace.pipes.is_empty()).await;
    let pipe = view.workspace.pipes.first().expect("the pipe");
    assert!(pipe.name.starts_with("Pipe joined ") && !pipe.name.to_lowercase().contains("aktory"), "{}", pipe.name);
}
