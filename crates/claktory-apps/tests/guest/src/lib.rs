//! A claktory:app@1.1 app for the host's tests (with feature `v1_0`, the
//! same app built for 1.0, without views). Each tool (`call`) does one
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
//! - push <surface> <message>: push-view, answering "pushed" or the error.
//! - sources: data-sources, "<id> <label> <granted>" a line.
//! - query <source> <sql>: data-query with params [1], answering its JSON or
//!   the error ("not-granted", "unknown-source", "invalid: …", "failed: …").
//! - whoami: caller-info, "<id> <project or -> <admin> <tainted>" or "none".
//! - emit-to <agent> <text>: emit-to that agent, kind "said", scope "chat".
//!
//! on-timer(name) emits "tick" with the name as its text, and " as <id>"
//! after it should caller-info say anyone (1.1).
//!
//! start() sets its status to "started"; upgrade(from) keeps
//! "upgraded-from" in its store.
//!
//! Views (1.1): view-open("main") pushes a createSurface whose Text shows
//! the params; "refuse" refuses; "invalid" pushes `{"invalid":true}` and
//! refuses with the push's error. view-action pushes an updateDataModel of
//! /last with the message, and keeps it as "last-action". view-closed
//! keeps the surface as "closed" and what pushing to it then answered as
//! "push-after-close".

use std::cell::Cell;

use wasi::http::outgoing_handler;
use wasi::http::types::{Fields, Method, OutgoingRequest, Scheme};
use wasi::io::streams::StreamError;

#[cfg(not(feature = "v1_0"))]
wit_bindgen::generate!({
    world: "app-with-views",
    path: "../../wit",
});

#[cfg(feature = "v1_0")]
wit_bindgen::generate!({
    world: "app",
    path: "wit-1.0",
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
            #[cfg(not(feature = "v1_0"))]
            "push" => match host::push_view(key, rest) {
                Ok(()) => ok("pushed"),
                Err(error) => fail(error),
            },
            #[cfg(not(feature = "v1_0"))]
            "sources" => ok(host::data_sources()
                .iter()
                .map(|source| format!("{} {} {}", source.id, source.label, source.granted))
                .collect::<Vec<_>>()
                .join("\n")),
            #[cfg(not(feature = "v1_0"))]
            "query" => {
                use claktory::app::types::DataError;
                match host::data_query(key, rest, "[1]") {
                    Ok(json) => ok(json),
                    Err(DataError::NotGranted) => fail("not-granted"),
                    Err(DataError::UnknownSource) => fail("unknown-source"),
                    Err(DataError::Invalid(why)) => fail(format!("invalid: {why}")),
                    Err(DataError::Failed(why)) => fail(format!("failed: {why}")),
                }
            }
            #[cfg(not(feature = "v1_0"))]
            "whoami" => ok(whoami()),
            #[cfg(not(feature = "v1_0"))]
            "emit-to" => {
                let event = Event { kind: "said".into(), text: rest.into(), data: None, dedupe_key: None, scope: "chat".into() };
                host::emit_to(key, &event);
                ok("emitted")
            }
            "grow" => {
                let mb: usize = args.parse().unwrap_or(1);
                let block = std::hint::black_box(vec![1u8; mb << 20]);
                ok(block.len().to_string())
            }
            _ => fail(format!("no tool {tool}")),
        }
    }

    fn on_timer(name: String) {
        #[cfg(not(feature = "v1_0"))]
        let name = match host::caller_info() {
            Some(caller) => format!("{name} as {}", caller.id),
            None => name,
        };
        host::emit(&Event { kind: "tick".into(), text: name, data: None, dedupe_key: None, scope: "chat".into() });
    }
}

#[cfg(not(feature = "v1_0"))]
impl exports::claktory::app::views::Guest for TestApp {
    fn view_open(view: String, params: String, surface: String) -> Result<(), String> {
        match view.as_str() {
            "main" => {
                let create = format!(
                    r#"{{"version":"1.0","createSurface":{{"surfaceId":"{surface}","catalogId":"https://claktory.com/a2ui/catalog/v1","components":[{{"id":"root","component":"Text","text":{params:?}}}],"dataModel":{{}}}}}}"#
                );
                host::push_view(&surface, &create)
            }
            "invalid" => Err(host::push_view(&surface, r#"{"invalid":true}"#).err().unwrap_or_else(|| "it was taken".into())),
            _ => Err(format!("no view {view}")),
        }
    }

    fn view_action(surface: String, message: String) {
        host::kv_set("last-action", message.as_bytes()).expect("room for it");
        let update = format!(r#"{{"version":"1.0","updateDataModel":{{"surfaceId":"{surface}","path":"/last","value":{message:?}}}}}"#);
        let _ = host::push_view(&surface, &update);
    }

    fn view_closed(surface: String) {
        host::kv_set("closed", surface.as_bytes()).expect("room for it");
        let pushed = host::push_view(&surface, "{}").err().unwrap_or_else(|| "pushed".into());
        host::kv_set("push-after-close", pushed.as_bytes()).expect("room for it");
    }
}

#[cfg(not(feature = "v1_0"))]
fn whoami() -> String {
    match host::caller_info() {
        Some(caller) => format!("{} {} {} {}", caller.id, caller.project.as_deref().unwrap_or("-"), caller.admin, caller.tainted),
        None => "none".to_owned(),
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
