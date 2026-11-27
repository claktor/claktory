//! Claktory's browser (#435 P4): the person opens it, watches it, types in
//! it and stops it, through the Mac dev stand-in over this Mac's Chrome.
//! The live view goes only to the connection watching: another one taking
//! the view tells the first, whose input then counts no more.
//!
//! A file of its own: it turns Claktory's browser on for the whole
//! process, which also makes errands part of every Claktory started after.
//!
//! Claktory's browser isn't on Windows.
#![cfg(unix)]

#[path = "common/mod.rs"]
mod common;

use std::time::Duration;

use factory_protocol::v1::{self, AgentBrowserState as State, client_message::Body as Request, server_message::Body};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use common::{Client, MODEL, View, apply_request, start, temp_dir};

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

/// Its live view on, as QA and debugging turn it on (#435: off in the
/// product, where agents use the browser on their own).
fn live_view_on(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("home")).expect("home");
    std::fs::write(dir.join("home/config.json"), r#"{"browser": {"live_view": true}}"#).expect("config");
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
    live_view_on(&dir);
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
    other.ok(Request::StopAgentBrowser(v1::StopAgentBrowser::default())).await;
    tokio::time::timeout(Duration::from_secs(30), view.follow(&mut client, |view| state(view) == Some(State::Off)))
        .await
        .expect("it stops");
    assert!(refused(other.request(watch(true)).await.body).contains("isn't open"));

    // Its data deleted while it's off: the kept profile goes.
    let profile = dir.join("home/browser-standin");
    assert!(profile.is_dir(), "the profile is kept between uses");
    other.ok(Request::DeleteAgentBrowserData(v1::DeleteAgentBrowserData::default())).await;
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
    live_view_on(&dir);
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
    std::fs::create_dir_all(dir.join("home")).expect("home");
    let config = serde_json::json!({"agents": [{"id": "runner", "name": "Runner", "model": MODEL, "effort": "off"}]});
    std::fs::write(dir.join("home/config.json"), config.to_string()).expect("config");
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert!(factoryd::browser_host::land_for_test(&factory, "errand-1", "runner", "x.txt", b"x").await.is_err(), "no browser without it");
    browser_on(&mut client, &mut view, "runner").await;
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
    factoryd::browser_host::end_agent_for_test(&factory, "runner", "errand-1").expect("ended");
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
    live_view_on(&dir);
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
    client.ok(Request::StopAgentBrowser(v1::StopAgentBrowser::default())).await;
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
    client.ok(Request::StopAgentBrowser(v1::StopAgentBrowser::default())).await;
    drop(held);
}

/// Off, as in the product (#435, Matt's card 222): no device watches,
/// opens, types in, takes over or hands back Claktory's browser; stopping
/// it and deleting its data aren't held back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_its_live_view_no_device_watches_or_drives_claktorys_browser() {
    factoryd::browser_host::enable_for_test();
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let no_view = "has no live view";
    let requests = [
        Request::WatchAgentBrowser(v1::WatchAgentBrowser { on: true, ..Default::default() }),
        Request::OpenAgentBrowser(v1::OpenAgentBrowser { url: "https://example.com/".into(), ..Default::default() }),
        Request::AgentBrowserInput(v1::AgentBrowserInput {
            events: vec![v1::AgentBrowserEvent { event: Some(v1::agent_browser_event::Event::Text("hi".into())) }],
        }),
        Request::TakeOverAgentBrowser(v1::TakeOverAgentBrowser {}),
        Request::HandBackAgentBrowser(v1::HandBackAgentBrowser::default()),
    ];
    for request in requests {
        let message = refused(client.request(request.clone()).await.body);
        assert!(message.contains(no_view), "{request:?}: {message}");
    }
    client.ok(Request::WatchAgentBrowser(v1::WatchAgentBrowser { on: false, ..Default::default() })).await;
    // Stopping isn't gated (here it's refused only because it isn't open).
    let stop = client.request(Request::StopAgentBrowser(v1::StopAgentBrowser::default())).await.body;
    assert!(refused(stop).contains("isn't open"));
    client.ok(Request::DeleteAgentBrowserData(v1::DeleteAgentBrowserData::default())).await;
}

fn chrome_here() -> bool {
    let here = std::path::Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome").is_file();
    if !here {
        eprintln!("no Chrome on this Mac: skipped");
    }
    here
}

/// Whether a CDP answer is the lease's refusal.
fn no_lease(answer: &serde_json::Value) -> bool {
    answer["error"]["code"] == -32001 && answer["error"]["message"] == "no input lease"
}

/// R11.3 #5 (#530): an errand's Input.* reaches Chrome only under its
/// lease, held by the host as well as the runner (the Mac's stand-in grants
/// every lease, so this is the host's gate): refused at once without one,
/// through with one, refused again once it's dropped, and once the errand
/// stopped driving (the person's turn) however long the old one had left.
/// A message to another target, which could carry Input inside it, never
/// goes.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_errands_input_reaches_the_page_only_under_its_lease() {
    if !chrome_here() {
        return;
    }
    factoryd::browser_host::enable_for_test();
    factoryd::browser_host::standin_for_test();
    let dir = temp_dir();
    let (_addr, factory) = start(&dir).await;
    let mut drive = factoryd::browser_host::drive_for_test(&factory, "errand-1", None).await.expect("driven");
    let page = drive.page().await;
    assert!(!page.is_empty());
    let moved = serde_json::json!({"type": "mouseMoved", "x": 10, "y": 10});

    let refused = drive.call_in(&page, "Input.dispatchMouseEvent", moved.clone()).await;
    assert!(no_lease(&refused), "{refused}");
    assert_eq!(refused["sessionId"], page.as_str(), "answered in its session");
    let lease = drive.lease(30_000).expect("a lease");
    let through = drive.call_in(&page, "Input.dispatchMouseEvent", moved.clone()).await;
    assert!(through.get("result").is_some(), "{through}");
    // Not Input: through either way.
    let other = drive.call("Browser.getVersion", serde_json::json!({})).await;
    assert!(other.get("result").is_some(), "{other}");
    let wrapped = drive
        .call("Target.sendMessageToTarget", serde_json::json!({"message": r#"{"id":1,"method":"Input.insertText","params":{"text":"x"}}"#}))
        .await;
    assert_eq!(wrapped["error"]["code"], -32001, "{wrapped}");
    drop(lease);
    let after = drive.call_in(&page, "Input.dispatchMouseEvent", moved.clone()).await;
    assert!(no_lease(&after), "{after}");

    // The person's turn ends it: back to the errand, the old lease (still
    // kept, with time left) holds nothing.
    let kept = drive.lease(60_000).expect("a lease");
    factoryd::browser_host::turn_for_test(&factory, false).expect("a turn and back");
    drop(drive);
    let mut again = factoryd::browser_host::drive_for_test(&factory, "errand-1", None).await.expect("driven again");
    let page = again.page().await;
    let refused = again.call_in(&page, "Input.dispatchMouseEvent", moved).await;
    assert!(no_lease(&refused), "{refused}");
    drop(kept);
    assert_eq!(again.bootstrapped().await, Ok(()));
    factoryd::browser_host::end_for_test(&factory, "errand-1");
}

/// Ghiaccio 43216: the errand's turn back right after an undriven one, its
/// mode change not reported yet, drives Chrome again: DRIVEN only once the
/// runner says Chrome is, never DRIVEN with Chrome undriven (where the
/// errand's connection would hang, and no lease could be had).
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_back_right_after_an_undriven_one_drives_chrome_again() {
    if !chrome_here() {
        return;
    }
    factoryd::browser_host::enable_for_test();
    factoryd::browser_host::standin_for_test();
    let dir = temp_dir();
    let (_addr, factory) = start(&dir).await;
    let drive = factoryd::browser_host::drive_for_test(&factory, "errand-1", None).await.expect("driven");
    drop(drive);
    // Handed over undriven and back at once, before the runner reports.
    factoryd::browser_host::turn_for_test(&factory, true).expect("a turn and back");
    let mut again = factoryd::browser_host::drive_for_test(&factory, "errand-1", None).await.expect("driven again");
    let version = again.call("Browser.getVersion", serde_json::json!({})).await;
    assert!(version.get("result").is_some(), "{version}");
    // Every report of the two mode changes is in by now.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let driven = State::Driven as i32;
    assert_eq!(factoryd::browser_host::mode_for_test(&factory), (Some("driven"), driven), "DRIVEN with Chrome driven");
    let lease = again.lease(10_000).expect("a lease, Chrome being driven");
    drop(lease);
    factoryd::browser_host::end_for_test(&factory, "errand-1");
}

/// A cookie of `domain` named `name`, for Storage.setCookies, kept an hour.
fn cookie(name: &str, domain: &str) -> serde_json::Value {
    let expires = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + 3600;
    serde_json::json!({"name": name, "value": "v", "domain": domain, "path": "/", "secure": true, "expires": expires})
}

/// The cookies' domains Chrome holds now.
async fn domains(drive: &mut factoryd::browser_host::DriveForTest) -> Vec<String> {
    let jar = drive.call("Storage.getCookies", serde_json::json!({})).await;
    let mut domains: Vec<String> = jar["result"]["cookies"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|cookie| cookie["domain"].as_str().map(str::to_owned))
        .collect();
    domains.sort();
    domains
}

async fn until(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..200 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{what} never happened");
}

/// A sign-out while an errand drives waits (nothing filled on Google
/// meanwhile), is applied when the errand ends, and deletes every cookie
/// Google owns (google.com, google.co.uk, youtube.com) and no other.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sign_out_waits_for_the_errand_and_deletes_only_the_providers_cookies() {
    if !chrome_here() {
        return;
    }
    factoryd::browser_host::enable_for_test();
    factoryd::browser_host::standin_for_test();
    let dir = temp_dir();
    let (_addr, factory) = start(&dir).await;
    let mut drive = factoryd::browser_host::drive_for_test(&factory, "errand-1", None).await.expect("driven");
    // A sign-in page's cookie alone isn't a sign-in.
    let set = drive.call("Storage.setCookies", serde_json::json!({"cookies": [cookie("NID", ".google.com")]})).await;
    assert!(set.get("result").is_some(), "{set}");
    factoryd::browser_host::end_for_test(&factory, "errand-1");

    let mut drive = factoryd::browser_host::drive_for_test(&factory, "errand-2", None).await.expect("driven");
    let cookies =
        [cookie("SID", ".google.com"), cookie("NID", ".google.co.uk"), cookie("VISITOR", ".youtube.com"), cookie("sid", "shop.example")];
    let set = drive.call("Storage.setCookies", serde_json::json!({ "cookies": cookies })).await;
    assert!(set.get("result").is_some(), "{set}");
    drop(drive);
    factoryd::browser_host::end_for_test(&factory, "errand-2");

    let mut drive = factoryd::browser_host::drive_for_test(&factory, "errand-3", None).await.expect("driven");
    assert_eq!(factoryd::browser_host::sign_out_for_test(&factory, "google").await, Ok("pending"));
    assert!(factoryd::browser_host::signing_out_for_test(&factory, "Google"));
    assert!(dir.join("home/browser-sign-outs.json").is_file(), "kept for a restart");
    // Still signed in while the errand drives.
    assert!(domains(&mut drive).await.contains(&".google.com".to_owned()));
    drop(drive);
    factoryd::browser_host::end_for_test(&factory, "errand-3");
    until("the sign-out applied", || !factoryd::browser_host::signing_out_for_test(&factory, "Google")).await;
    assert!(!dir.join("home/browser-sign-outs.json").exists());

    let mut drive = factoryd::browser_host::drive_for_test(&factory, "errand-4", None).await.expect("driven");
    assert_eq!(domains(&mut drive).await, ["shop.example"]);
    // Not signed in: nothing to sign out of waits, and an unknown one is refused.
    assert!(factoryd::browser_host::sign_out_for_test(&factory, "Evil").await.is_err());
    factoryd::browser_host::end_for_test(&factory, "errand-4");
}

/// A sign-out asked while the browser is off waits in its file, and is
/// applied before the next driven start lets the errand in.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_waiting_sign_out_is_applied_before_the_errand_drives() {
    if !chrome_here() {
        return;
    }
    factoryd::browser_host::enable_for_test();
    factoryd::browser_host::standin_for_test();
    let dir = temp_dir();
    let (_addr, factory) = start(&dir).await;
    assert_eq!(factoryd::browser_host::sign_out_for_test(&factory, "Apple").await, Ok("pending"));
    assert!(factoryd::browser_host::signing_out_for_test(&factory, "Apple"));
    let drive = factoryd::browser_host::drive_for_test(&factory, "errand-1", None).await.expect("driven");
    // DRIVEN only once it was applied.
    assert!(!factoryd::browser_host::signing_out_for_test(&factory, "Apple"));
    assert!(!dir.join("home/browser-sign-outs.json").exists());
    assert_eq!(drive.bootstrapped().await, Ok(()));
    factoryd::browser_host::end_for_test(&factory, "errand-1");
}

/// A driven start restores no pages (Fugo 42798): whatever the errand left
/// open last time, Chrome starts again on blank pages only, so no page runs
/// before the host has set the browser up.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_driven_start_restores_no_page() {
    if !chrome_here() {
        return;
    }
    factoryd::browser_host::enable_for_test();
    factoryd::browser_host::standin_for_test();
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let url = page().await;
    let mut drive = factoryd::browser_host::drive_for_test(&factory, "errand-1", None).await.expect("driven");
    let session = drive.page().await;
    let went = drive.call_in(&session, "Page.navigate", serde_json::json!({ "url": url })).await;
    assert!(went.get("result").is_some(), "{went}");
    tokio::time::sleep(Duration::from_secs(1)).await;
    drop(drive);
    factoryd::browser_host::end_for_test(&factory, "errand-1");
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    client.ok(Request::StopAgentBrowser(v1::StopAgentBrowser::default())).await;
    tokio::time::timeout(Duration::from_secs(30), view.follow(&mut client, |view| state(view) == Some(State::Off)))
        .await
        .expect("it stops");

    let mut drive = factoryd::browser_host::drive_for_test(&factory, "errand-2", None).await.expect("driven again");
    let targets = drive.call("Target.getTargets", serde_json::json!({})).await;
    let pages: Vec<&str> = targets["result"]["targetInfos"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|target| target["type"] == "page")
        .filter_map(|target| target["url"].as_str())
        .collect();
    assert!(!pages.is_empty() && pages.iter().all(|url| *url == "about:blank"), "{pages:?}");
    factoryd::browser_host::end_for_test(&factory, "errand-2");
}

/// Turns the Browser extension on for `agent`, as the person does.
async fn browser_on(client: &mut Client, view: &mut View, agent: &str) {
    let on = Request::UpdateAgent(v1::UpdateAgent {
        agent_id: agent.into(),
        extension: Some(v1::ExtensionChange { id: "browser".into(), enabled: Some(true), ..Default::default() }),
        ..Default::default()
    });
    assert!(matches!(apply_request(client, view, on).await, Body::Ack(_)), "{agent}'s browser on");
}

/// Each agent has a browser of its own: one agent's cookies are never in
/// another's, the Workspace lists each browsing agent's, Close keeps the
/// profile, and an agent whose Browser extension is turned off has its
/// browser closed and its profile deleted while the other's stays.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_agent_has_its_own_browser_and_its_profile_goes_with_its_browser() {
    if !chrome_here() {
        return;
    }
    factoryd::browser_host::enable_for_test();
    factoryd::browser_host::standin_for_test();
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("home")).expect("home");
    let config = serde_json::json!({
        "agents": [
            {"id": "yukako", "name": "Yukako", "model": MODEL, "effort": "off"},
            {"id": "koichi", "name": "Koichi", "model": MODEL, "effort": "off"},
            {"id": "rohan", "name": "Rohan", "model": MODEL, "effort": "off"},
        ]
    });
    std::fs::write(dir.join("home/config.json"), config.to_string()).expect("config");
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let listed = |view: &View| view.workspace.agent_browsers.iter().map(|browser| browser.agent_id.clone()).collect::<Vec<_>>();
    assert!(listed(&view).is_empty());
    browser_on(&mut client, &mut view, "koichi").await;
    browser_on(&mut client, &mut view, "yukako").await;
    assert_eq!(listed(&view), ["yukako", "koichi"], "only browsing agents, in order");
    assert!(factoryd::browser_host::drive_agent_for_test(&factory, "rohan", "errand-r", None).await.is_err(), "no browser without it");

    let mut yukako = factoryd::browser_host::drive_agent_for_test(&factory, "yukako", "errand-y", None).await.expect("yukako drives");
    let set = yukako.call("Storage.setCookies", serde_json::json!({"cookies": [cookie("sid", "yukako.example")]})).await;
    assert!(set.get("error").is_none(), "{set}");
    let mut koichi = factoryd::browser_host::drive_agent_for_test(&factory, "koichi", "errand-k", None).await.expect("koichi drives");
    assert!(domains(&mut yukako).await.contains(&"yukako.example".to_owned()));
    assert!(!domains(&mut koichi).await.contains(&"yukako.example".to_owned()), "koichi's profile is its own");
    let (yukako_profile, koichi_profile) = (
        factoryd::browser_host::standin_profile_for_test(&factory, "yukako"),
        factoryd::browser_host::standin_profile_for_test(&factory, "koichi"),
    );
    assert!(yukako_profile.is_dir() && koichi_profile.is_dir() && yukako_profile != koichi_profile);
    assert_eq!(factoryd::browser_host::profiles_for_test(&factory), ["koichi", "yukako"]);
    view.follow(&mut client, |view| {
        view.workspace.agent_browsers.iter().all(|browser| browser.state() == State::Driven)
            && view.workspace.agent_browser.as_ref().is_some_and(|browser| browser.state() == State::Warm)
    })
    .await;

    // Close keeps the profile.
    drop(koichi);
    factoryd::browser_host::close_for_test(&factory, "koichi");
    until("koichi's browser closes", || factoryd::browser_host::agent_state_for_test(&factory, "koichi") == State::Off as i32).await;
    assert!(koichi_profile.is_dir());

    // Yukako's browser turned off: closed, and its profile gone.
    drop(yukako);
    let off = Request::UpdateAgent(v1::UpdateAgent {
        agent_id: "yukako".into(),
        extension: Some(v1::ExtensionChange { id: "browser".into(), enabled: Some(false), ..Default::default() }),
        ..Default::default()
    });
    client.ok(off).await;
    until("yukako's profile goes", || !yukako_profile.exists()).await;
    until("yukako is no longer listed", || factoryd::browser_host::profiles_for_test(&factory) == ["koichi"]).await;
    assert!(koichi_profile.is_dir(), "koichi's stays");
    view.follow(&mut client, |view| listed(view) == ["koichi"]).await;

    // Deleting koichi's while it's closed.
    factoryd::browser_host::delete_for_test(&factory, "koichi").expect("deleted");
    until("koichi's profile goes", || !koichi_profile.exists()).await;
    until("no agent is listed", || factoryd::browser_host::profiles_for_test(&factory).is_empty()).await;
}

/// An agent gives a page a file from its folders: the runner stages a copy
/// and only that copy's path reaches the page; a file elsewhere, or a path
/// the runner didn't stage, never does. Signing out of a site deletes its
/// cookies (and its names') once the errand driving the browser ends,
/// and leaves other sites' alone.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_page_gets_only_a_staged_copy_and_a_site_signs_out_alone() {
    if !chrome_here() {
        return;
    }
    factoryd::browser_host::enable_for_test();
    factoryd::browser_host::standin_for_test();
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("home")).expect("home");
    let (work, elsewhere) = (dir.join("work"), dir.join("elsewhere"));
    std::fs::create_dir_all(&work).expect("work");
    std::fs::create_dir_all(&elsewhere).expect("elsewhere");
    std::fs::write(work.join("cv.pdf"), b"%PDF-1.7 a CV").expect("cv");
    std::fs::write(elsewhere.join("id_ed25519"), b"a key").expect("key");
    let config = serde_json::json!({"agents": [{"id": "yukako", "name": "Yukako", "model": MODEL, "effort": "off"}]});
    std::fs::write(dir.join("home/config.json"), config.to_string()).expect("config");
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    browser_on(&mut client, &mut View { workspace, ..View::default() }, "yukako").await;
    let roots = [work.clone()];

    let mut drive = factoryd::browser_host::drive_agent_for_test(&factory, "yukako", "errand-u", None).await.expect("yukako drives");
    let staged = drive.upload(&work.join("cv.pdf"), &roots).await.expect("staged");
    assert!(staged.ends_with("/cv.pdf") && !staged.starts_with(work.to_str().unwrap()), "{staged}");
    assert_eq!(std::fs::read(&staged).expect("the copy"), b"%PDF-1.7 a CV");
    assert!(drive.upload(&elsewhere.join("id_ed25519"), &roots).await.is_err(), "outside its folders");

    let session = drive.page().await;
    drive.call_in(&session, "Page.navigate", serde_json::json!({"url": "data:text/html,<input type=file id=f>"})).await;
    let mut input = serde_json::Value::Null;
    for _ in 0..50 {
        input = drive.call_in(&session, "Runtime.evaluate", serde_json::json!({"expression": "document.getElementById('f')"})).await;
        if input["result"]["result"]["objectId"].is_string() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let object = input["result"]["result"]["objectId"].clone();
    assert!(object.is_string(), "{input}");
    let set = |files: Vec<&str>| serde_json::json!({"files": files, "objectId": object});
    let given = drive.call_in(&session, "DOM.setFileInputFiles", set(vec![&staged])).await;
    assert!(given.get("error").is_none(), "{given}");
    let chosen = drive.call_in(&session, "Runtime.evaluate", serde_json::json!({"expression": "(f => f.name + ':' + f.size)(document.getElementById('f').files[0])"})).await;
    assert_eq!(chosen["result"]["result"]["value"], format!("cv.pdf:{}", b"%PDF-1.7 a CV".len()));
    let key = elsewhere.join("id_ed25519");
    for path in [key.to_str().unwrap(), work.join("cv.pdf").to_str().unwrap()] {
        let refused = drive.call_in(&session, "DOM.setFileInputFiles", set(vec![path])).await;
        assert_eq!(refused["error"]["code"], -32001, "{path}: {refused}");
    }

    // A site's sign-out waits for the errand, then takes its cookies only.
    let cookies = [cookie("sid", "x.example"), cookie("ct0", "api.x.example"), cookie("sid", "y.example")];
    let set = drive.call("Storage.setCookies", serde_json::json!({"cookies": cookies})).await;
    assert!(set.get("error").is_none(), "{set}");
    assert_eq!(drive.sign_out_site("https://x.example").await, Ok(false), "pending while the errand drives");
    assert!(drive.sign_out_site("x.example").await.is_err());
    drive.end("errand-u");
    drop(drive);
    let mut again = factoryd::browser_host::drive_agent_for_test(&factory, "yukako", "errand-v", None).await.expect("driven again");
    let left = domains(&mut again).await;
    assert!(!left.iter().any(|domain| domain.ends_with("x.example")) && left.contains(&"y.example".to_owned()), "{left:?}");
    again.end("errand-v");
}

/// Each agent browses in its own browser: two agents open theirs at the
/// same time, each begun in its own Chrome, never the person's.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_agents_browse_at_once_each_in_its_own_browser() {
    if !chrome_here() {
        return;
    }
    factoryd::browser_host::enable_for_test();
    factoryd::browser_host::standin_for_test();
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("home")).expect("home");
    let config = serde_json::json!({
        "agents": [
            {"id": "yukako", "name": "Yukako", "model": MODEL, "effort": "off"},
            {"id": "koichi", "name": "Koichi", "model": MODEL, "effort": "off"},
        ]
    });
    std::fs::write(dir.join("home/config.json"), config.to_string()).expect("config");
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    browser_on(&mut client, &mut view, "yukako").await;
    browser_on(&mut client, &mut view, "koichi").await;
    let yukako = factoryd::errands::open_browser_for_test(&factory, "yukako").expect("yukako's");
    let koichi = factoryd::errands::open_browser_for_test(&factory, "koichi").expect("koichi's, at the same time");
    assert_ne!(yukako, koichi);
    view.follow(&mut client, |view| {
        let state = |agent: &str| view.workspace.agent_browsers.iter().find(|browser| browser.agent_id == agent).map(|browser| browser.state());
        state("yukako") == Some(State::Driven)
            && state("koichi") == Some(State::Driven)
            && view.workspace.agent_browser.as_ref().is_none_or(|browser| browser.state() != State::Driven)
    })
    .await;
    assert_eq!(factoryd::browser_host::profiles_for_test(&factory), ["koichi", "yukako"]);
    factoryd::errands::end_errand_for_test(&factory, &yukako).await;
    factoryd::errands::end_errand_for_test(&factory, &koichi).await;
}

/// A browser whose agent left it unused closes then, not kept warm for
/// as long again.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_browser_left_unused_closes_without_waiting_warm() {
    if !chrome_here() {
        return;
    }
    factoryd::browser_host::enable_for_test();
    factoryd::browser_host::standin_for_test();
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("home")).expect("home");
    let config = serde_json::json!({"agents": [{"id": "yukako", "name": "Yukako", "model": MODEL, "effort": "off"}]});
    std::fs::write(dir.join("home/config.json"), config.to_string()).expect("config");
    let (addr, factory) = start(&dir).await;
    factoryd::errands::unused_after_for_test(&factory, Duration::from_secs(2));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    browser_on(&mut client, &mut view, "yukako").await;
    factoryd::errands::open_browser_for_test(&factory, "yukako").expect("yukako's");
    let state = |view: &View| view.workspace.agent_browsers.iter().find(|browser| browser.agent_id == "yukako").map(|browser| browser.state());
    // Off within the test's wait, not after the 10 minutes warm.
    let mut driven = false;
    view.follow(&mut client, |view| {
        driven |= state(view) == Some(State::Driven);
        driven && state(view) == Some(State::Off)
    })
    .await;
}
