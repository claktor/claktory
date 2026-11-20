//! A claktory:app@1 app for the host's tests. Each tool (`call`) does one
//! thing, with `args` as plain text:
//! - echo: answers "<args> from <caller>", with `args` as its data.
//! - count: counts its calls in memory, so a restart shows.
//! - get <key> / set <key> <value> / delete <key>: its store.
//! - secret <name>: the secret, or "none".
//! - emit <text>: an event of kind "said", scope "chat", deduped by text.
//! - status <text>: its status line.
//! - timer <ms>: on-timer("tick") after ms, which emits "tick".
//! - fetch <url>: GET, answering "<status> <body>" or the error.
//! - spin: never returns. trap: panics. grow <mb>: allocates that much.
//!
//! start() sets its status to "started"; upgrade(from) keeps
//! "upgraded-from" in its store.

use std::cell::Cell;

use wasi::http::outgoing_handler;
use wasi::http::types::{Fields, Method, OutgoingRequest, Scheme};
use wasi::io::streams::StreamError;

wit_bindgen::generate!({
    world: "app",
    path: "../../wit",
});

use claktory::app::host;
use claktory::app::types::Event;

thread_local! {
    static COUNT: Cell<u32> = const { Cell::new(0) };
}

struct TestApp;

fn ok(text: impl Into<String>) -> ToolResult {
    ToolResult { text: text.into(), data: None, is_error: false }
}

fn fail(text: impl Into<String>) -> ToolResult {
    ToolResult { text: text.into(), data: None, is_error: true }
}

impl Guest for TestApp {
    fn start() {
        host::set_status("started");
    }

    fn stop() {
        host::log(host::LogLevel::Info, "stopping");
    }

    fn upgrade(from_version: String) {
        host::kv_set("upgraded-from", from_version.as_bytes()).expect("room for it");
    }

    fn call(tool: String, args: String, caller: String) -> ToolResult {
        let (key, rest) = args.split_once(' ').unwrap_or((&args, ""));
        match tool.as_str() {
            "echo" => ToolResult { text: format!("{args} from {caller}"), data: Some(args.clone()), is_error: false },
            "count" => ok(COUNT
                .with(|count| {
                    count.set(count.get() + 1);
                    count.get()
                })
                .to_string()),
            "get" => ok(host::kv_get(key).map_or("none".to_owned(), |value| String::from_utf8_lossy(&value).into_owned())),
            "set" => match host::kv_set(key, rest.as_bytes()) {
                Ok(()) => ok("kept"),
                Err(error) => fail(error),
            },
            "delete" => {
                host::kv_delete(key);
                ok("deleted")
            }
            "secret" => ok(host::secret(key).unwrap_or_else(|| "none".to_owned())),
            "emit" => {
                host::emit(&Event {
                    kind: "said".into(),
                    text: args.clone(),
                    data: Some(format!("{{\"text\":\"{args}\"}}")),
                    dedupe_key: Some(args.clone()),
                    scope: "chat".into(),
                });
                ok("emitted")
            }
            "status" => {
                host::set_status(&args);
                ok("set")
            }
            "timer" => {
                host::set_timer("tick", args.parse().unwrap_or(0));
                ok("set")
            }
            "fetch" => match fetch(&args) {
                Ok(text) => ok(text),
                Err(error) => fail(error),
            },
            "spin" => {
                let mut n: u64 = 0;
                loop {
                    n = std::hint::black_box(n.wrapping_add(1));
                }
            }
            "trap" => panic!("asked to trap"),
            "grow" => {
                let mb: usize = args.parse().unwrap_or(1);
                let block = std::hint::black_box(vec![1u8; mb << 20]);
                ok(block.len().to_string())
            }
            _ => fail(format!("no tool {tool}")),
        }
    }

    fn on_timer(name: String) {
        host::emit(&Event { kind: "tick".into(), text: name, data: None, dedupe_key: None, scope: "chat".into() });
    }
}

fn fetch(url: &str) -> Result<String, String> {
    let (scheme, rest) = url.split_once("://").ok_or("no scheme")?;
    let (authority, path) = rest.split_once('/').map_or((rest, "/".to_owned()), |(authority, path)| (authority, format!("/{path}")));
    let request = OutgoingRequest::new(Fields::new());
    let scheme = if scheme == "https" { Scheme::Https } else { Scheme::Http };
    request.set_method(&Method::Get).map_err(|()| "method")?;
    request.set_scheme(Some(&scheme)).map_err(|()| "scheme")?;
    request.set_authority(Some(authority)).map_err(|()| "authority")?;
    request.set_path_with_query(Some(&path)).map_err(|()| "path")?;
    let future = outgoing_handler::handle(request, None).map_err(|error| format!("{error:?}"))?;
    future.subscribe().block();
    let response = match future.get() {
        Some(Ok(Ok(response))) => response,
        Some(Ok(Err(error))) => return Err(format!("{error:?}")),
        _ => return Err("no answer".into()),
    };
    let status = response.status();
    let body = response.consume().map_err(|()| "body")?;
    let stream = body.stream().map_err(|()| "stream")?;
    let mut bytes = Vec::new();
    loop {
        match stream.blocking_read(65536) {
            Ok(chunk) => bytes.extend_from_slice(&chunk),
            Err(StreamError::Closed) => break,
            Err(StreamError::LastOperationFailed(error)) => return Err(error.to_debug_string()),
        }
    }
    Ok(format!("{status} {}", String::from_utf8_lossy(&bytes)))
}

export!(TestApp);
