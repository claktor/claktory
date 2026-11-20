//! The app as a component of the `claktory:app@1` world: its exports call
//! [`App`], and [`WasmHost`] is the host's imports plus `wasi:http`.

use std::cell::RefCell;
use std::time::Duration;

use serde_json::Value;
use wasi::http::outgoing_handler;
use wasi::http::types::{
    ErrorCode, Fields, FutureIncomingResponse, IncomingBody, Method, OutgoingBody, OutgoingRequest, RequestOptions, Scheme,
};
use wasi::io::streams::StreamError;

use crate::{API, App, Host, Level, POLL_SECONDS, Request, Response};

wit_bindgen::generate!({
    world: "app",
    path: "wit",
});

use claktory::app::host;
use claktory::app::types;

/// The app and its host, for the life of the instance (calls come one at
/// a time).
struct State {
    app: App,
    host: WasmHost,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State { app: App::default(), host: WasmHost::default() });
}

fn with<T>(run: impl FnOnce(&mut App, &mut WasmHost) -> T) -> T {
    STATE.with(|state| {
        let state = &mut *state.borrow_mut();
        run(&mut state.app, &mut state.host)
    })
}

struct Component;

impl Guest for Component {
    fn start() {
        with(|app, host| app.start(host));
    }

    fn stop() {
        with(|app, host| {
            app.stop(host);
            // Dropping it cancels the request.
            host.poll = None;
        });
    }

    fn upgrade(_from_version: String) {}

    fn call(tool: String, args: String, caller: String) -> types::ToolResult {
        let crate::ToolResult { text, data, is_error } = with(|app, host| app.call(host, &tool, &args, &caller));
        types::ToolResult { text, data: data.map(|data| data.to_string()), is_error }
    }

    fn on_timer(name: String) {
        with(|app, host| app.on_timer(host, &name));
    }
}

export!(Component);

/// The host's imports, and Telegram over `wasi:http`.
#[derive(Default)]
struct WasmHost {
    /// The long poll under way: it goes on while the app isn't running.
    poll: Option<FutureIncomingResponse>,
}

impl Host for WasmHost {
    fn secret(&mut self, name: &str) -> Option<String> {
        host::secret(name)
    }

    fn kv_get(&mut self, key: &str) -> Option<Vec<u8>> {
        host::kv_get(key)
    }

    fn kv_set(&mut self, key: &str, value: &[u8]) -> Result<(), String> {
        host::kv_set(key, value)
    }

    fn kv_delete(&mut self, key: &str) {
        host::kv_delete(key);
    }

    fn emit(&mut self, kind: &str, text: &str, data: Option<&Value>, dedupe: Option<&str>, scope: &str) {
        host::emit(&types::Event {
            kind: kind.to_owned(),
            text: text.to_owned(),
            data: data.map(Value::to_string),
            dedupe_key: dedupe.map(str::to_owned),
            scope: scope.to_owned(),
        });
    }

    fn set_timer(&mut self, name: &str, after_ms: u64) {
        host::set_timer(name, after_ms);
    }

    fn set_status(&mut self, text: &str) {
        host::set_status(text);
    }

    fn log(&mut self, level: Level, message: &str) {
        let level = match level {
            Level::Error => host::LogLevel::Error,
            Level::Warn => host::LogLevel::Warn,
            Level::Info => host::LogLevel::Info,
            Level::Debug => host::LogLevel::Debug,
        };
        host::log(level, message);
    }

    fn call(&mut self, token: &str, request: &Request) -> Result<Response, String> {
        let future = send(token, request, Duration::from_secs(30))?;
        future.subscribe().block();
        read(&future).unwrap_or_else(|| Err("Telegram didn't answer.".into()))
    }

    fn start_poll(&mut self, token: &str, request: &Request) -> Result<(), String> {
        self.poll = Some(send(token, request, Duration::from_secs(POLL_SECONDS + 15))?);
        Ok(())
    }

    fn poll_answer(&mut self) -> Option<Result<Response, String>> {
        let answer = read(self.poll.as_ref()?)?;
        self.poll = None;
        Some(answer)
    }

    fn polling(&self) -> bool {
        self.poll.is_some()
    }

    fn now(&mut self) -> u64 {
        wasi::clocks::wall_clock::now().seconds
    }

    fn random(&mut self) -> u64 {
        wasi::random::random::get_random_u64()
    }
}

/// Starts a Bot API call: POST with its parameters as JSON. The token is
/// in the path, as Telegram wants it, and never in an error.
fn send(token: &str, request: &Request, wait: Duration) -> Result<FutureIncomingResponse, String> {
    let headers = Fields::from_list(&[("content-type".to_owned(), b"application/json".to_vec())])
        .map_err(|error| format!("Couldn't make the request: {error:?}"))?;
    let outgoing = OutgoingRequest::new(headers);
    let authority = API.trim_start_matches("https://");
    let path = format!("/bot{token}/{}", request.method);
    outgoing
        .set_method(&Method::Post)
        .and_then(|()| outgoing.set_scheme(Some(&Scheme::Https)))
        .and_then(|()| outgoing.set_authority(Some(authority)))
        .and_then(|()| outgoing.set_path_with_query(Some(&path)))
        .map_err(|()| "Couldn't make the request.".to_owned())?;
    let body = outgoing.body().map_err(|()| "Couldn't make the request.".to_owned())?;
    let options = RequestOptions::new();
    let nanos = wait.as_nanos() as u64;
    let _ = options.set_connect_timeout(Some(10_000_000_000));
    let _ = options.set_first_byte_timeout(Some(nanos));
    let _ = options.set_between_bytes_timeout(Some(nanos));
    let future = outgoing_handler::handle(outgoing, Some(options)).map_err(describe)?;
    let bytes = request.params.to_string().into_bytes();
    {
        let stream = body.write().map_err(|()| "Couldn't send the request.".to_owned())?;
        for chunk in bytes.chunks(4096) {
            stream.blocking_write_and_flush(chunk).map_err(|error| format!("Couldn't send the request: {error:?}"))?;
        }
    }
    OutgoingBody::finish(body, None).map_err(describe)?;
    Ok(future)
}

/// The answer, once it's come: None while it hasn't.
fn read(future: &FutureIncomingResponse) -> Option<Result<Response, String>> {
    let response = match future.get()? {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => return Some(Err(describe(error))),
        Err(()) => return Some(Err("Telegram's answer was already read.".into())),
    };
    let status = response.status();
    let Ok(body) = response.consume() else { return Some(Err("Couldn't read Telegram's answer.".into())) };
    let mut bytes = Vec::new();
    {
        let Ok(stream) = body.stream() else { return Some(Err("Couldn't read Telegram's answer.".into())) };
        loop {
            match stream.blocking_read(64 * 1024) {
                Ok(chunk) => bytes.extend_from_slice(&chunk),
                Err(StreamError::Closed) => break,
                Err(StreamError::LastOperationFailed(error)) => {
                    return Some(Err(format!("Telegram's answer broke off: {}", error.to_debug_string())));
                }
            }
        }
    }
    let _ = IncomingBody::finish(body);
    Some(Ok(Response { status, body: bytes }))
}

/// A transport error in words.
fn describe(error: ErrorCode) -> String {
    match error {
        ErrorCode::DnsTimeout | ErrorCode::DnsError(_) => "Couldn't find Telegram: no network?".into(),
        ErrorCode::ConnectionRefused | ErrorCode::ConnectionTimeout | ErrorCode::ConnectionReadTimeout => "Couldn't reach Telegram.".into(),
        ErrorCode::HttpRequestDenied => "Claktory didn't allow the app to reach Telegram.".into(),
        error => format!("Couldn't reach Telegram: {error:?}"),
    }
}
