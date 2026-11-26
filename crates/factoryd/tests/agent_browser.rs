//! Claktory's browser (#435 P4): the person opens it, watches it, types in
//! it and stops it, through the Mac dev stand-in over this Mac's Chrome.
//! The live view goes only to the connection watching: another one taking
//! the view tells the first, whose input then counts no more.
//!
//! A file of its own: it turns Claktory's browser on for the whole
//! process, which also makes errands part of every Claktory started after.

#[path = "common/mod.rs"]
mod common;

use std::time::Duration;

use factory_protocol::v1::{self, AgentBrowserState as State, client_message::Body as Request, server_message::Body};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use common::{Client, View, apply_request, start, temp_dir};

/// A page with a text field, served on this computer.
async fn page() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else { return };
            tokio::spawn(async move {
                let mut request = [0u8; 4096];
                let _ = stream.read(&mut request).await;
                let body = "<!doctype html><title>Form</title><body style='background:#36c'><input autofocus id=f>";
                let reply = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\nset-cookie: sid=1; Max-Age=3600\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes()).await;
            });
        }
    });
    format!("http://{address}/")
}

fn state(view: &View) -> Option<State> {
    view.workspace.agent_browser.as_ref().map(|browser| browser.state())
}

fn refused(body: Body) -> String {
    match body {
        Body::Error(error) => error.message,
        other => panic!("not refused: {other:?}"),
    }
}

#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_person_opens_watches_types_in_and_stops_claktorys_browser() {
    if !std::path::Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome").is_file() {
        eprintln!("no Chrome on this Mac: skipped");
        return;
    }
    factoryd::browser_host::enable_for_test();
    factoryd::browser_host::standin_for_test();
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert_eq!(state(&view), Some(State::Off), "{:?}", view.workspace.agent_browser);

    // Nothing to watch or type in before it's open.
    let watch = |on: bool| Request::WatchAgentBrowser(v1::WatchAgentBrowser { on, max_width: 640, ..Default::default() });
    assert!(refused(client.request(watch(true)).await.body).contains("isn't open"));
    let typed = Request::AgentBrowserInput(v1::AgentBrowserInput {
        events: vec![v1::AgentBrowserEvent { event: Some(v1::agent_browser_event::Event::Text("hi".into())) }],
    });
    assert!(refused(client.request(typed.clone()).await.body).contains("Watch"));
    // Only web addresses.
    let open = |url: &str| Request::OpenAgentBrowser(v1::OpenAgentBrowser { url: url.to_owned(), ..Default::default() });
    assert!(refused(client.request(open("--remote-debugging-port=9222")).await.body).contains("web addresses"));

    let url = page().await;
    client.ok(open(&url)).await;
    tokio::time::timeout(Duration::from_secs(30), view.follow(&mut client, |view| state(view) == Some(State::Browsing)))
        .await
        .expect("it opens for the person");

    // Watched: frames come one at a time, as big as asked at most.
    client.ok(watch(true)).await;
    let frame = client.wait_for(|body| if let Body::AgentBrowserFrame(frame) = body { Some(frame.clone()) } else { None }).await;
    assert!(frame.width <= 640 && frame.width > 0 && frame.height > 0, "{}x{}", frame.width, frame.height);
    assert_eq!(&frame.jpeg[..2], [0xff, 0xd8]);
    assert!(frame.screen_width >= frame.width);
    client.ok(Request::AgentBrowserFrameAck(v1::AgentBrowserFrameAck { seq: frame.seq })).await;
    // Its turn (it opened it): input counts.
    client.ok(typed.clone()).await;

    // Another connection takes the view: the first hears it, and its input
    // no longer counts.
    let (mut other, _, _) = Client::connect(addr).await;
    other.ok(watch(true)).await;
    let moved = client.wait_for(|body| if let Body::AgentBrowserViewMoved(moved) = body { Some(moved.clone()) } else { None }).await;
    assert!(!moved.device.is_empty());
    assert!(refused(client.request(typed.clone()).await.body).contains("Watch"));
    other.wait_for(|body| matches!(body, Body::AgentBrowserFrame(_)).then_some(())).await;

    // Stopped: off, and nothing to watch.
    other.ok(Request::StopAgentBrowser(v1::StopAgentBrowser {})).await;
    tokio::time::timeout(Duration::from_secs(30), view.follow(&mut client, |view| state(view) == Some(State::Off)))
        .await
        .expect("it stops");
    assert!(refused(other.request(watch(true)).await.body).contains("isn't open"));

    // Its data deleted while it's off: the kept profile goes.
    let profile = dir.join("home/browser-standin");
    assert!(profile.is_dir(), "the profile is kept between uses");
    other.ok(Request::DeleteAgentBrowserData(v1::DeleteAgentBrowserData {})).await;
    tokio::time::timeout(Duration::from_secs(30), async {
        while profile.exists() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the profile is deleted");
    tokio::time::timeout(Duration::from_secs(30), view.follow(&mut client, |view| state(view) == Some(State::Off)))
        .await
        .expect("off again");
}

fn locale(view: &View) -> (String, Vec<String>) {
    let browser = view.workspace.agent_browser.clone().unwrap_or_default();
    (browser.time_zone, browser.languages)
}

/// Its time zone and languages (change 3): the first device's fill them,
/// a later device's never overwrite them, Settings changes them, and only
/// a known zone and well-formed languages are taken.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_browsers_time_zone_and_languages_are_the_persons() {
    factoryd::browser_host::enable_for_test();
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert_eq!(locale(&view), (String::new(), Vec::new()));

    // The zones offered are ones it takes.
    let zones = client.request(Request::ListTimeZones(v1::ListTimeZones {})).await.body;
    let Body::TimeZones(zones) = zones else { panic!("not time zones: {zones:?}") };
    assert!(zones.names.iter().any(|zone| zone == "Europe/London"));
    assert!(!zones.names.iter().any(|zone| zone.starts_with("posix/")));

    // The first device that watches fills both (it isn't open, so the
    // watch itself is refused); what isn't a language is left out.
    let watch = |zone: &str, languages: &[&str]| {
        Request::WatchAgentBrowser(v1::WatchAgentBrowser {
            on: true,
            device_time_zone: zone.to_owned(),
            device_languages: languages.iter().map(|tag| (*tag).to_owned()).collect(),
            ..Default::default()
        })
    };
    client.request(watch("Europe/Rome", &["it-IT", "--lang=x", "it"])).await;
    let rome = ("Europe/Rome".to_owned(), vec!["it-IT".to_owned(), "it".to_owned()]);
    tokio::time::timeout(Duration::from_secs(10), view.follow(&mut client, |view| locale(view) == rome)).await.expect("filled from the device");

    // Another device's never overwrite them.
    let (mut other, _, _) = Client::connect(addr).await;
    other.request(watch("Asia/Tokyo", &["ja"])).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let config = std::fs::read_to_string(dir.join("home/config.json")).expect("config");
    assert!(config.contains("Europe/Rome") && !config.contains("Asia/Tokyo"), "{config}");

    // Settings: only a known zone and languages Chrome takes.
    let set = |zone: &str, languages: &[&str]| {
        Request::SetAgentBrowserLocale(v1::SetAgentBrowserLocale {
            time_zone: zone.to_owned(),
            languages: languages.iter().map(|tag| (*tag).to_owned()).collect(),
        })
    };
    assert!(refused(client.request(set("Europe/Lndon", &["en"])).await.body).contains("doesn't know the time zone “Europe/Lndon”"));
    assert!(refused(client.request(set(":/etc/passwd", &["en"])).await.body).contains("doesn't know the time zone"));
    assert!(refused(client.request(set("Europe/London", &["en", "--lang=x"])).await.body).contains("isn't a language"));
    let nine = ["en", "fr", "de", "it", "es", "pt", "nl", "sv", "da"];
    assert!(refused(client.request(set("Europe/London", &nine)).await.body).contains("at most 8"));
    apply_request(&mut client, &mut view, set("Europe/London", &["en-GB", "en"])).await;
    let london = ("Europe/London".to_owned(), vec!["en-GB".to_owned(), "en".to_owned()]);
    tokio::time::timeout(Duration::from_secs(10), view.follow(&mut client, |view| locale(view) == london)).await.expect("set");

    // Empty: not set again, and the next device fills it.
    apply_request(&mut client, &mut view, set("", &[])).await;
    tokio::time::timeout(Duration::from_secs(10), view.follow(&mut client, |view| locale(view).0.is_empty())).await.expect("unset");
}

/// An errand's download (R9): read over the runner's channel 5 only once
/// asked for, kept by its hash, named by Claktory, copied into the errand
/// agent's folder afresh and inert, and the runner's copy deleted.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_errands_download_lands_named_by_claktory_in_its_agents_folder() {
    if !std::path::Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome").is_file() {
        eprintln!("no Chrome on this Mac: skipped");
        return;
    }
    factoryd::browser_host::enable_for_test();
    factoryd::browser_host::standin_for_test();
    let dir = temp_dir();
    let (_addr, factory) = start(&dir).await;
    let land = |suggested: &'static str, bytes: Vec<u8>| {
        let factory = factory.clone();
        async move { factoryd::browser_host::land_for_test(&factory, "errand-1", "runner", suggested, &bytes).await }
    };

    let (name, path, flag) = land("../../setup.sh", b"#!/bin/sh\necho hi\n".to_vec()).await.expect("landed");
    assert_eq!((name.as_str(), flag.as_str()), ("setup.sh", "a script"));
    assert_eq!(path, dir.join("home/errands/runner/downloads/errand-1/setup.sh"));
    assert_eq!(std::fs::read(&path).unwrap(), b"#!/bin/sh\necho hi\n");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o644, "never executable");

    // The same name again: a name of its own. A big file comes in pieces.
    let big: Vec<u8> = (0..3_000_000u32).map(|n| n as u8).collect();
    let (again, path, flag) = land("setup.sh", big.clone()).await.expect("landed");
    assert_eq!((again.as_str(), flag.as_str()), ("setup (2).sh", "a script"));
    assert_eq!(std::fs::read(&path).unwrap(), big);
    let (plain, _, flag) = land("report.pdf", b"%PDF-1.7".to_vec()).await.expect("landed");
    assert_eq!((plain.as_str(), flag.as_str()), ("report.pdf", ""));
    factoryd::browser_host::end_for_test(&factory, "errand-1");
}

/// Stopped while an errand's agent is connected to it (Ghiaccio 38462):
/// STOPPING until its runner has really ended, however long the errand
/// holds on, then OFF; and it opens again. On the way, errands learn the
/// sites it's signed in to (Ghiaccio 38597).
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stopped_browser_ends_even_while_an_errand_holds_its_connection() {
    if !std::path::Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome").is_file() {
        eprintln!("no Chrome on this Mac: skipped");
        return;
    }
    factoryd::browser_host::enable_for_test();
    factoryd::browser_host::standin_for_test();
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let url = page().await;
    let held = factoryd::browser_host::connect_for_test(&factory, "errand-1", Some(url.clone())).await.expect("driven");
    // What it's signed in to reaches errands (errand_open says it): read
    // again as the errand ends, once the page has set its cookie.
    tokio::time::sleep(Duration::from_millis(500)).await;
    factoryd::browser_host::end_for_test(&factory, "errand-1");
    tokio::time::timeout(Duration::from_secs(10), async {
        while !factoryd::browser_host::errands_signed_in_for_test(&factory).contains(&"https://127.0.0.1".to_owned()) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("errands know where it's signed in");
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    client.ok(Request::StopAgentBrowser(v1::StopAgentBrowser {})).await;
    // Off well before Claktory would give up waiting for the runner (10 s):
    // it ended, though the errand's connection is still held.
    tokio::time::timeout(Duration::from_secs(6), view.follow(&mut client, |view| state(view) == Some(State::Off)))
        .await
        .expect("its runner ended");
    let open = Request::OpenAgentBrowser(v1::OpenAgentBrowser { url, ..Default::default() });
    client.ok(open).await;
    tokio::time::timeout(Duration::from_secs(30), view.follow(&mut client, |view| state(view) == Some(State::Browsing)))
        .await
        .expect("it opens again");
    client.ok(Request::StopAgentBrowser(v1::StopAgentBrowser {})).await;
    drop(held);
}
