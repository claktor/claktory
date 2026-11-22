//! Advanced diagnostics: off until the person turns them on; then the
//! person's devices send what they recorded, by area, and the backend
//! records its own side, kept a source each, bounded, once per event, and
//! deleted when they're turned off.

use crate::common::{Client, last_workspace, start, temp_dir};
use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body};

fn event(seq: u64, at_ms: i64, kind: &str, detail: &str) -> v1::DiagnosticEvent {
    v1::DiagnosticEvent { at_ms, seq, kind: kind.into(), detail: detail.into(), duration_ms: 812, session: 3, area: "connection".into() }
}

fn send(events: Vec<v1::DiagnosticEvent>) -> Request {
    Request::SendDiagnostics(v1::SendDiagnostics { events })
}

fn set(on: Option<bool>, clear: bool, source: &str, area: &str) -> Request {
    Request::SetDiagnostics(v1::SetDiagnostics { share: on, clear, source_id: source.into(), area: area.into() })
}

/// (source, area, session, seq, kind, detail, duration, app_version)
type Row = (String, String, i64, i64, String, String, i64, String);

fn rows(dir: &std::path::Path) -> Vec<Row> {
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
    let mut statement = db
        .prepare("SELECT source, area, session, seq, kind, detail, duration, app_version FROM diagnostics ORDER BY source, at, id")
        .expect("query");
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?)))
        .expect("rows")
        .collect::<Result<_, _>>()
        .expect("read")
}

fn from(dir: &std::path::Path, source: &str) -> Vec<Row> {
    rows(dir).into_iter().filter(|row| row.0 == source).collect()
}

/// Waits until `done`: the backend writes its own events after the fact.
async fn until(done: impl Fn() -> bool) {
    for _ in 0..200 {
        if done() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("never happened");
}

fn diagnostics(events: &[v1::ServerMessage]) -> v1::Diagnostics {
    last_workspace(events).expect("a workspace").diagnostics.clone().expect("diagnostics")
}

fn refused(reply: Body) -> String {
    match reply {
        Body::Error(error) => error.message,
        other => panic!("expected an error, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn devices_and_the_backend_record_diagnostics_only_while_the_person_has_them_on() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut mac, workspace, _) = Client::connect_as(addr, v1::DeviceKind::Laptop).await;
    let off = workspace.diagnostics.expect("diagnostics");
    assert_eq!((off.share, off.sources.len(), off.keep_days, off.source_limit_bytes), (false, 0, 14, 4 * 1024 * 1024));

    // Off: nothing is taken, and the backend records nothing.
    let now = factoryd::transcript::now_ms();
    let reply = mac.request(send(vec![event(1, now, "connect", "path=direct")])).await;
    assert_eq!(refused(reply.body), "Diagnostics are off on this Claktory.");
    assert!(rows(&dir).is_empty());

    // On: kept, as one bounded line, from the device the connection is.
    let on = diagnostics(&mac.ok(set(Some(true), false, "", "")).await);
    assert!(on.share && on.sources.is_empty());
    let mut other_area = event(2, now - 1500, "Tap", "screen=settings");
    other_area.area = "Launch Speed".into();
    let mut no_area = event(3, now - 1200, "x", "");
    no_area.area = String::new();
    let events = vec![
        event(1, now - 2000, "Connect", "path=relay\nms=812"),
        other_area,
        no_area,
        event(4, now - 1000, "<b>error</b>", &"x".repeat(900)),
        // Older than 14 days: dropped.
        event(5, now - 15 * 24 * 3600 * 1000, "connect", "stale"),
    ];
    mac.ok(send(events.clone())).await;
    // Sent again after a lost Ack: kept once.
    let shown = diagnostics(&mac.ok(send(events)).await);
    let kept = from(&dir, "device:local");
    assert_eq!(kept.len(), 4, "{kept:?}");
    let row = |area: &str, seq: i64, kind: &str, detail: &str| {
        ("device:local".to_owned(), area.to_owned(), 3, seq, kind.to_owned(), detail.to_owned(), 812, "test".to_owned())
    };
    assert_eq!(kept[0], row("connection", 1, "connect", "path=relay ms=812"));
    assert_eq!(kept[1], row("launchspeed", 2, "tap", "screen=settings"));
    assert_eq!(kept[2], row("general", 3, "x", ""));
    assert_eq!((kept[3].4.as_str(), kept[3].5.chars().count()), ("berrorb", 500));
    let mac_shown = shown.sources.iter().find(|source| source.id == "device:local").expect("this Mac's app");
    assert_eq!((mac_shown.kind(), mac_shown.events), (v1::DeviceKind::Laptop, 4));
    assert_eq!((mac_shown.oldest_ms, mac_shown.newest_ms), (now - 2000, now - 1000));
    assert!(mac_shown.bytes > 500 && mac_shown.received_ms >= now);
    let areas: Vec<(&str, u32)> = mac_shown.areas.iter().map(|area| (area.area.as_str(), area.events)).collect();
    assert_eq!(areas, [("connection", 2), ("general", 1), ("launchspeed", 1)]);

    // Too many at once is refused whole.
    let flood = (0..2001).map(|seq| event(100 + seq, now, "network", "net=wifi")).collect();
    assert_eq!(refused(mac.request(send(flood)).await.body), "Send at most 2000 diagnostic events at a time.");
    assert_eq!(from(&dir, "device:local").len(), 4);

    // A paired device's are its own; it can't pass for another. The backend
    // records it coming in, under its own source.
    let (mut phone, _) = crate::remote::another_device(addr, "Phone").await;
    let shown = diagnostics(&phone.ok(send(vec![event(1, now, "welcome", "path=relay")])).await);
    let phone_source = shown.sources.iter().find(|source| source.name == "Phone").expect("the phone").id.clone();
    assert_eq!(phone_source, "device:phone");
    assert_eq!(from(&dir, "device:phone").len(), 1);
    let came = |row: &Row| row.1 == "connection" && row.4 == "connected" && row.5 == "device=phone via=this_computer app=test";
    until(|| from(&dir, "backend").iter().any(came)).await;
    let shown = diagnostics(&mac.ok(set(None, false, "", "")).await);
    let backend_shown = shown.sources.iter().find(|source| source.id == "backend").expect("the backend");
    assert_eq!((backend_shown.name.as_str(), backend_shown.app_version.as_str()), ("", factoryd::update::VERSION));

    // Clearing one source's area leaves the rest, and the setting.
    let shown = diagnostics(&mac.ok(set(None, true, "device:local", "connection")).await);
    assert!(shown.share);
    assert_eq!(from(&dir, "device:local").iter().map(|row| row.1.as_str()).collect::<Vec<_>>(), ["launchspeed", "general"]);
    let mac_shown = shown.sources.iter().find(|source| source.id == "device:local").expect("still there");
    assert_eq!(mac_shown.events, 2);
    // Clearing a source leaves the others.
    let shown = diagnostics(&mac.ok(set(None, true, "device:phone", "")).await);
    assert!(shown.sources.iter().all(|source| source.id != "device:phone"));
    assert!(from(&dir, "device:phone").is_empty() && !from(&dir, "device:local").is_empty());

    // A disconnection is recorded, with how it ended and how long it lasted.
    let disconnected = || from(&dir, "backend").into_iter().filter(|row| row.4 == "disconnected").count();
    let before = disconnected();
    let (extra, _, _) = Client::connect(addr).await;
    drop(extra);
    until(|| disconnected() > before).await;
    let gone = from(&dir, "backend").into_iter().rfind(|row| row.4 == "disconnected").expect("recorded");
    assert!(gone.5.starts_with("device=local via=port end="), "{gone:?}");
    drop(phone);

    // Off deletes everything, and nothing more is taken or recorded.
    let shown = diagnostics(&mac.ok(set(Some(false), false, "", "")).await);
    assert!(!shown.share && shown.sources.is_empty());
    assert!(rows(&dir).is_empty());
    assert!(refused(mac.request(send(vec![event(9, now, "connect", "")])).await.body).contains("off"));
    let (again, _, _) = Client::connect(addr).await;
    drop(again);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(rows(&dir).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_source_keeps_at_most_four_megabytes_oldest_out_first() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut mac, _, _) = Client::connect(addr).await;
    mac.ok(set(Some(true), false, "", "")).await;
    let now = factoryd::transcript::now_ms();
    let detail = "d".repeat(500);
    // 5 batches of 2000 × ~600 bytes: about 6 MB.
    for batch in 0..5u64 {
        let events = (0..2000u64).map(|n| event(batch * 2000 + n, now - 100_000 + (batch * 2000 + n) as i64, "network", &detail)).collect();
        mac.ok(send(events)).await;
    }
    let kept = from(&dir, "device:local");
    let bytes: usize = kept.iter().map(|row| 64 + row.1.len() + row.4.len() + row.5.len()).sum();
    assert!(bytes <= 4 * 1024 * 1024 && bytes > 4 * 1024 * 1024 - 1000, "{bytes}");
    // The newest stay.
    assert_eq!(kept.last().expect("kept").3, 9999);
    assert_eq!(kept[0].3, 10_000 - kept.len() as i64);
}

/// Turned away is the one thing a stranger can make Claktory record: a
/// flood of knocks is a row per window, counting them, never a write each.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn knocks_turned_away_are_counted_not_written_one_by_one() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut mac, _, _) = Client::connect(addr).await;
    mac.ok(set(Some(true), false, "", "")).await;
    factoryd::diagnostics::set_turned_window_for_test(&factory, std::time::Duration::from_millis(1500));
    let started = std::time::Instant::now();
    for _ in 0..1000 {
        factoryd::diagnostics::turned_away_for_test(&factory, "relay", "unpaired");
    }
    factoryd::diagnostics::turned_away_for_test(&factory, "direct", "sign_in_required");
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    let turned = || from(&dir, "backend").into_iter().filter(|row| row.4 == "turned_away").map(|row| row.5).collect::<Vec<_>>();
    until(|| turned().len() == 3).await;
    tokio::time::sleep(std::time::Duration::from_millis(1700)).await;
    let mut rows = turned();
    rows.sort();
    assert_eq!(rows, ["via=direct why=sign_in_required n=1", "via=relay why=unpaired n=1", "via=relay why=unpaired n=999"]);
}

/// A client on the local port without this computer's token (an app built
/// without it, or with a stale one) is recorded as turned away, counted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_on_the_local_port_without_the_token_is_recorded_turned_away() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut mac, _, _) = Client::connect(addr).await;
    mac.ok(set(Some(true), false, "", "")).await;
    factoryd::diagnostics::set_turned_window_for_test(&factory, std::time::Duration::from_millis(500));
    for token in ["", "stale", "stale"] {
        let mut client = Client::open(addr).await;
        let hello = v1::Hello { protocol_version: factory_protocol::PROTOCOL_VERSION, client_version: "test".into(), token: token.into(), ..Default::default() };
        client.send(v1::ClientMessage { id: 1, body: Some(Request::Hello(hello)) }).await;
        assert!(matches!(client.receive().await.body, Some(Body::Error(_))));
    }
    let turned = || {
        let mut rows = from(&dir, "backend").into_iter().filter(|row| row.4 == "turned_away").map(|row| row.5).collect::<Vec<_>>();
        rows.sort();
        rows
    };
    until(|| turned().len() == 3).await;
    assert_eq!(turned(), ["via=port why=no_token n=1", "via=port why=wrong_token n=1", "via=port why=wrong_token n=1"]);
}
