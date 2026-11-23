//! The host runs apps for factoryd: it loads, starts, calls and stops them
//! when asked, gives them their store, secrets, timers and only the hosts
//! they were approved for, and tells factoryd when one crashes, without
//! restarting it by itself.

use std::time::Duration;

use crate::support::{Factoryd, error, quick, temp_dir};
use factory_protocol::apps::frame::Body;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn an_app_starts_answers_tool_calls_and_stops() {
    let dir = temp_dir("lifecycle");
    let mut factoryd = Factoryd::start(quick()).await;
    assert_eq!(factoryd.load("echo", &dir, |_| {}).await, "");
    let before = factoryd.call("echo", "echo", "hi").await;
    assert_eq!(error(before), "echo isn't running.");

    assert_eq!(factoryd.start_app("echo").await, "");
    let status = factoryd.told(|body| matches!(body, Body::SetStatus(status) if status.app_id == "echo").then(|| body.clone())).await;
    let Body::SetStatus(status) = status else { unreachable!() };
    assert_eq!(status.text, "started");
    assert_eq!(factoryd.start_app("echo").await, "", "starting one that runs is a no-op");

    let Body::CallResult(result) = factoryd.call("echo", "echo", "hi").await else { panic!("a result") };
    assert_eq!((result.text.as_str(), result.data.as_str(), result.is_error), ("hi from builder", "hi", false));
    let Body::CallResult(unknown) = factoryd.call("echo", "nope", "").await else { panic!("a result") };
    assert_eq!((unknown.text.as_str(), unknown.is_error), ("no tool nope", true), "the app's own error, not a crash");

    assert_eq!(factoryd.stop_app("echo").await, "");
    assert_eq!(error(factoryd.call("echo", "echo", "hi").await), "echo isn't running.");
    assert_eq!(factoryd.stop_app("echo").await, "", "stopping one that doesn't run is a no-op");

    let unload = factoryd.ask(Body::Unload(factory_protocol::apps::Unload { app_id: "echo".into() })).await;
    assert_eq!(error(unload), "");
    assert_eq!(error(factoryd.call("echo", "echo", "hi").await), "echo isn't loaded.");
    assert_eq!(factoryd.start_app("echo").await, "echo isn't loaded.");
    factoryd.close().await.expect("the host ends when factoryd does");
}

#[tokio::test]
async fn what_isnt_an_app_is_refused_at_load() {
    let dir = temp_dir("refused");
    let mut factoryd = Factoryd::start(quick()).await;
    let junk = dir.join("junk.wasm");
    std::fs::write(&junk, b"not wasm").expect("write");
    let refused = factoryd.load("junk", &dir, |load| load.component = junk.display().to_string()).await;
    assert!(refused.contains("isn't a component this Claktory can run"), "{refused}");
    let world = factoryd.load("future", &dir, |load| load.world = "claktory:app@2".into()).await;
    assert_eq!(world, "This Claktory can't run apps built for claktory:app@2.");
    let host = factoryd.load("hosts", &dir, |load| load.hosts = vec!["a/b".into()]).await;
    assert_eq!(host, "\"a/b\" isn't a host name.");
    assert_eq!(factoryd.start_app("junk").await, "junk isn't loaded.");
}

#[tokio::test]
async fn the_store_lasts_across_restarts_and_updates_but_memory_doesnt() {
    let dir = temp_dir("store");
    let mut factoryd = Factoryd::start(quick()).await;
    factoryd.load("notes", &dir, |_| {}).await;
    factoryd.start_app("notes").await;
    assert_eq!(factoryd.text("notes", "set", "offset 42").await, "kept");
    assert_eq!(factoryd.text("notes", "count", "").await, "1");
    assert_eq!(factoryd.text("notes", "count", "").await, "2");

    factoryd.stop_app("notes").await;
    factoryd.start_app("notes").await;
    assert_eq!(factoryd.text("notes", "get", "offset").await, "42");
    assert_eq!(factoryd.text("notes", "count", "").await, "1", "a new instance");
    assert_eq!(factoryd.text("notes", "get", "upgraded-from").await, "none");

    // An update: Load again, upgrade() runs once before start().
    let updated = factoryd.load("notes", &dir, |load| {
        load.version = "1.1.0".into();
        load.upgrade_from = "1.0.0".into();
    });
    assert_eq!(updated.await, "");
    assert_eq!(error(factoryd.call("notes", "count", "").await), "notes isn't running.", "a new load isn't started");
    factoryd.start_app("notes").await;
    assert_eq!(factoryd.text("notes", "get", "upgraded-from").await, "1.0.0");
    assert_eq!(factoryd.text("notes", "get", "offset").await, "42");
    assert_eq!(factoryd.text("notes", "delete", "offset").await, "deleted");
    assert_eq!(factoryd.text("notes", "get", "offset").await, "none");
    assert!(dir.join("notes/data/kv.sqlite").exists());
}

#[tokio::test]
async fn the_store_holds_to_its_allowance() {
    let dir = temp_dir("allowance");
    let mut factoryd = Factoryd::start(quick()).await;
    factoryd.load("big", &dir, |load| load.storage_mb = 1).await;
    factoryd.start_app("big").await;
    let half = "x".repeat(600 << 10);
    assert_eq!(factoryd.text("big", "set", &format!("a {half}")).await, "kept");
    let Body::CallResult(full) = factoryd.call("big", "set", &format!("b {half}")).await else { panic!("a result") };
    assert_eq!((full.text.as_str(), full.is_error), ("The store would grow past its 1 MB.", true));
    assert_eq!(factoryd.text("big", "set", &format!("a {half}")).await, "kept", "replacing a value counts it once");
}

#[tokio::test]
async fn an_app_gets_its_secrets_from_factoryd() {
    let dir = temp_dir("secrets");
    let mut factoryd = Factoryd::start(quick()).await;
    factoryd.secrets.insert("bot_token".into(), "123:abc".into());
    factoryd.load("bot", &dir, |_| {}).await;
    factoryd.start_app("bot").await;
    assert_eq!(factoryd.text("bot", "secret", "bot_token").await, "123:abc");
    assert_eq!(factoryd.text("bot", "secret", "other").await, "none");
}

#[tokio::test]
async fn events_status_and_timers_reach_factoryd() {
    let dir = temp_dir("events");
    let mut factoryd = Factoryd::start(quick()).await;
    factoryd.load("chat", &dir, |_| {}).await;
    factoryd.start_app("chat").await;
    assert_eq!(factoryd.text("chat", "emit", "hello").await, "emitted");
    let emit = factoryd.told(|body| if let Body::Emit(emit) = body { Some(emit.clone()) } else { None }).await;
    assert_eq!((emit.app_id.as_str(), emit.kind.as_str(), emit.scope.as_str(), emit.text.as_str()), ("chat", "said", "chat", "hello"));
    assert_eq!((emit.data.as_str(), emit.dedupe_key.as_str()), ("{\"text\":\"hello\"}", "hello"));

    factoryd.text("chat", "status", "Give it a bot token in the Marketplace's Apps tab.").await;
    let status = factoryd
        .told(|body| match body {
            Body::SetStatus(status) if status.text != "started" => Some(status.text.clone()),
            _ => None,
        })
        .await;
    assert_eq!(status, "Give it a bot token in the Marketplace's Apps tab.");

    // A timer fires between calls, with nothing else asked.
    factoryd.text("chat", "timer", "50").await;
    let tick = tokio::time::timeout(
        Duration::from_secs(10),
        factoryd.told(|body| match body {
            Body::Emit(emit) if emit.kind == "tick" => Some(emit.text.clone()),
            _ => None,
        }),
    );
    assert_eq!(tick.await.expect("the timer fires"), "tick");

    // Timers go with the instance.
    factoryd.text("chat", "timer", "300").await;
    factoryd.stop_app("chat").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    factoryd.start_app("chat").await;
    factoryd.text("chat", "echo", "flush").await;
    assert!(!factoryd.was_told(|body| matches!(body, Body::Emit(emit) if emit.kind == "tick")), "no tick after a stop");
}

/// A server answering every request with "200 hello", once per connection.
async fn hello_server() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut request = vec![0; 4096];
                let _ = socket.read(&mut request).await;
                let _ = socket.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello").await;
            });
        }
    });
    port
}

#[tokio::test]
async fn an_app_reaches_only_the_hosts_it_was_approved_for() {
    let dir = temp_dir("http");
    let allowed = hello_server().await;
    let other = hello_server().await;
    let mut factoryd = Factoryd::start(quick()).await;
    let origin = format!("http://127.0.0.1:{allowed}");
    factoryd.load("web", &dir, |load| load.hosts = vec![origin.clone(), "api.telegram.org".into()]).await;
    factoryd.start_app("web").await;

    assert_eq!(factoryd.text("web", "fetch", &format!("{origin}/x")).await, "200 hello");
    for denied in [format!("http://127.0.0.1:{other}/x"), format!("http://localhost:{allowed}/x"), "http://api.telegram.org/x".into()] {
        let Body::CallResult(result) = factoryd.call("web", "fetch", &denied).await else { panic!("a result") };
        assert_eq!((result.text.as_str(), result.is_error), ("ErrorCode::HttpRequestDenied", true), "{denied}");
    }
}

#[tokio::test]
async fn a_crash_ends_the_instance_tells_factoryd_and_waits_to_be_started_again() {
    let dir = temp_dir("crash");
    let mut factoryd = Factoryd::start(quick()).await;
    factoryd.load("fragile", &dir, |load| load.memory_mb = 8).await;
    factoryd.start_app("fragile").await;
    factoryd.text("fragile", "count", "").await;

    let cases = [
        ("trap", "", "It stopped with an error in call: error while executing at wasm backtrace:"),
        ("spin", "", "Its call took longer than 2 seconds."),
        ("grow", "32", "It ran out of memory (8 MB)."),
    ];
    for (tool, args, why) in cases {
        let Body::CallResult(result) = factoryd.call("fragile", tool, args).await else { panic!("a result") };
        assert!(result.is_error && result.text.starts_with(why.trim_end_matches(':')), "{tool}: {}", result.text);
        let crashed = factoryd.told(|body| if let Body::Crashed(crashed) = body { Some(crashed.clone()) } else { None }).await;
        assert_eq!((crashed.app_id.as_str(), crashed.error.as_str()), ("fragile", result.text.as_str()));
        // It isn't restarted until factoryd says so.
        assert_eq!(error(factoryd.call("fragile", "count", "").await), "fragile isn't running.");
        assert_eq!(factoryd.start_app("fragile").await, "");
        assert_eq!(factoryd.text("fragile", "count", "").await, "1", "a new instance after {tool}");
    }
    assert_eq!(factoryd.text("fragile", "grow", "2").await, (2 << 20).to_string(), "within its memory");
}

#[tokio::test]
async fn the_host_binary_speaks_frames_on_stdio_and_ends_with_its_input() {
    use claktory_apps::frames;
    use factory_protocol::apps::{Frame, Hello};
    let mut host = tokio::process::Command::new(env!("CARGO_BIN_EXE_claktory-apps"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn");
    let mut input = host.stdin.take().expect("stdin");
    let mut output = host.stdout.take().expect("stdout");
    let hello = Frame { id: 1, reply_to: 0, body: Some(Body::Hello(Hello { version: "test".into() })) };
    frames::write(&mut input, &hello).await.expect("write");
    let ready = frames::read(&mut output).await.expect("read").expect("a frame");
    assert_eq!(ready.reply_to, 1);
    assert!(matches!(ready.body, Some(Body::Ready(ref ready)) if ready.worlds == ["claktory:app@1", "claktory:app@1.1"]), "{ready:?}");
    drop(input);
    let status = tokio::time::timeout(Duration::from_secs(10), host.wait()).await.expect("it ends").expect("status");
    assert!(status.success(), "{status}");
}
