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
