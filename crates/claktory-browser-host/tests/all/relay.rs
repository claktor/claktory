//! One session through the helper, and each way it refuses: the wrong
//! peer, a launch for another caller, an unsafe folder, an oversized frame,
//! the wrong tab.

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::Duration;

use claktory_browser_host::frame::{self, FRAME_LIMIT, Order};
use claktory_browser_host::peer;
use claktory_browser_host::relay::{Ended, Setup, run};
use claktory_browser_host::wire::Refusal;
use serde_json::{Value, json};

const CALLER: &str = "chrome-extension://jpdkjilehjhinhohikojjngehpbakcmo/";

/// A stand-in app: its private folder and listening socket.
struct App {
    _dir: tempfile::TempDir,
    socket: PathBuf,
    listener: UnixListener,
}

fn app() -> App {
    let dir = tempfile::tempdir().expect("a folder");
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).expect("0700");
    let socket = dir.path().join("broker.sock");
    let listener = UnixListener::bind(&socket).expect("a socket");
    App { _dir: dir, socket, listener }
}

/// What this test process meets: its own identifier and cdhash.
fn me() -> String {
    let own = peer::own().expect("the test's own signature");
    format!("identifier \"{}\" and cdhash H\"{}\"", own.identifier, own.cdhash_hex())
}

/// The stand-in Chrome's ends of the helper's stdin and stdout.
struct Chrome {
    to: UnixStream,
    from: UnixStream,
}

impl Chrome {
    fn send(&mut self, message: Value) {
        frame::write(&mut self.to, Order::Native, &message).expect("to the helper");
    }

    fn next(&mut self) -> Option<Value> {
        frame::read(&mut self.from, Order::Native).expect("a frame").map(Value::Object)
    }
}

fn start(app: &App, caller: &str, requirement: String) -> (Chrome, JoinHandle<Ended>) {
    let (to, stdin) = UnixStream::pair().expect("stdin");
    let (stdout, from) = UnixStream::pair().expect("stdout");
    from.set_read_timeout(Some(Duration::from_secs(10))).expect("timeout");
    let setup = Setup { caller: Some(CALLER.into()), socket: app.socket.clone(), requirement, version: "0.1.0".into() };
    let caller = caller.to_owned();
    let helper = std::thread::spawn(move || run(&caller, &setup, stdin, stdout));
    (Chrome { to, from }, helper)
}

fn accept(app: &App) -> UnixStream {
    let (socket, _) = app.listener.accept().expect("the helper connects");
    socket.set_read_timeout(Some(Duration::from_secs(10))).expect("timeout");
    socket
}

fn app_next(socket: &mut UnixStream) -> Option<Value> {
    frame::read(socket, Order::Little).expect("a frame").map(Value::Object)
}

fn app_send(socket: &mut UnixStream, message: Value) {
    frame::write(socket, Order::Little, &message).expect("to the helper");
}

fn hello(tab: u64) -> Value {
    json!({"type": "hello", "v": 1, "tab": tab, "origin": "https://example.com", "title": "Example"})
}

#[test]
fn a_session_passes_through_and_another_tab_ends_it() {
    let app = app();
    let (mut chrome, helper) = start(&app, CALLER, me());
    let mut socket = accept(&app);
    assert_eq!(app_next(&mut socket), Some(json!({"type": "helper", "v": 1, "version": "0.1.0", "caller": CALLER})));

    app_send(&mut socket, json!({"type": "pair", "code": "042917"}));
    assert_eq!(chrome.next(), Some(json!({"type": "pairing", "code": "042917"})));
    chrome.send(hello(7));
    assert_eq!(app_next(&mut socket), Some(hello(7)), "relayed as it is");
    let bound = json!({"type": "bound", "agent": "Rivet", "until_ms": 1_900_000_000_000_u64});
    app_send(&mut socket, bound.clone());
    assert_eq!(chrome.next(), Some(bound));
    app_send(&mut socket, json!({"id": 1, "session": "s-1", "sequence": 1, "cmd": {"op": "read", "limit": 4096}}));
    let command = json!({"type": "command", "id": 1, "session": "s-1", "sequence": 1, "cmd": {"op": "read", "limit": 4096}});
    assert_eq!(chrome.next(), Some(command));
    let result = json!({"type": "result", "tab": 7, "id": 1, "result": {"title": "Example", "text": "Hello"}});
    chrome.send(result.clone());
    assert_eq!(app_next(&mut socket), Some(result));

    // Another tab, in any frame, ends it on both sides.
    chrome.send(json!({"type": "paused", "tab": 8, "reason": "tab_inactive"}));
    assert_eq!(app_next(&mut socket), Some(json!({"type": "ended", "reason": "wrong_tab"})));
    assert_eq!(app_next(&mut socket), None, "then closed");
    assert_eq!(chrome.next(), Some(json!({"type": "ended", "reason": "wrong_tab"})));
    assert_eq!(helper.join().expect("the helper"), Ended::Refused(Refusal::WrongTab));
}

#[test]
fn a_process_that_isnt_the_app_gets_nothing() {
    let app = app();
    let (mut chrome, helper) = start(&app, CALLER, "identifier \"dev.factory.app\"".into());
    let mut socket = accept(&app);
    assert_eq!(app_next(&mut socket), None, "not even the helper's first frame");
    assert_eq!(chrome.next(), Some(json!({"type": "ended", "reason": "app_refused"})));
    assert!(matches!(helper.join().expect("the helper"), Ended::AppRefused(_)));
}

#[test]
fn started_for_another_caller_it_connects_to_nothing() {
    let app = app();
    app.listener.set_nonblocking(true).expect("nonblocking");
    for caller in ["chrome-extension://someoneelse/", "", "chrome-extension://jpdkjilehjhinhohikojjngehpbakcmo"] {
        let (mut chrome, helper) = start(&app, caller, me());
        assert_eq!(helper.join().expect("the helper"), Ended::WrongCaller, "{caller:?}");
        assert_eq!(chrome.next(), None, "{caller:?}: nothing said");
        assert!(app.listener.accept().is_err(), "{caller:?}: never connected");
    }
    // A build that names no extension yet refuses every caller.
    let (stdin, _to) = UnixStream::pair().expect("stdin");
    let (stdout, _from) = UnixStream::pair().expect("stdout");
    let setup = Setup { caller: None, socket: app.socket.clone(), requirement: me(), version: "0.1.0".into() };
    assert_eq!(run(CALLER, &setup, stdin, stdout), Ended::WrongCaller);
    assert!(app.listener.accept().is_err());
}

#[test]
fn a_socket_folder_open_to_others_or_a_link_is_never_used() {
    let app = app();
    app.listener.set_nonblocking(true).expect("nonblocking");
    let dir = app.socket.parent().expect("its folder").to_path_buf();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).expect("0755");
    let (mut chrome, helper) = start(&app, CALLER, me());
    assert!(matches!(helper.join().expect("the helper"), Ended::NoApp(why) if why.contains("open to other users")));
    assert_eq!(chrome.next(), Some(json!({"type": "ended", "reason": "no_app"})));
    assert!(app.listener.accept().is_err());
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).expect("0700");

    let linked = tempfile::tempdir().expect("a folder");
    let link = linked.path().join("browser");
    std::os::unix::fs::symlink(&dir, &link).expect("a link");
    let elsewhere = App { _dir: tempfile::tempdir().expect("a folder"), socket: link.join("broker.sock"), listener: app.listener };
    let (_chrome, helper) = start(&elsewhere, CALLER, me());
    assert!(matches!(helper.join().expect("the helper"), Ended::NoApp(why) if why.contains("isn't a folder")));
}

#[test]
fn an_oversized_frame_is_refused_before_it_arrives() {
    // From Chrome: only the length is sent, past the limit.
    let app = app();
    let (mut chrome, helper) = start(&app, CALLER, me());
    let mut socket = accept(&app);
    app_next(&mut socket);
    std::io::Write::write_all(&mut chrome.to, &(FRAME_LIMIT as u32 + 1).to_ne_bytes()).expect("a length");
    assert_eq!(app_next(&mut socket), Some(json!({"type": "ended", "reason": "bad_frame"})));
    assert_eq!(chrome.next(), Some(json!({"type": "ended", "reason": "bad_frame"})));
    assert!(matches!(helper.join().expect("the helper"), Ended::Refused(Refusal::BadFrame(_))));

    // From the app: the same.
    let app = self::app();
    let (mut chrome, helper) = start(&app, CALLER, me());
    let mut socket = accept(&app);
    app_next(&mut socket);
    std::io::Write::write_all(&mut socket, &u32::MAX.to_le_bytes()).expect("a length");
    assert_eq!(chrome.next(), Some(json!({"type": "ended", "reason": "bad_frame"})));
    assert!(matches!(helper.join().expect("the helper"), Ended::Refused(Refusal::BadFrame(_))));
    let mut rest = Vec::new();
    let _ = socket.read_to_end(&mut rest);
}

#[test]
fn a_command_out_of_turn_ends_it_and_a_stop_reaches_chrome() {
    let app = app();
    let (mut chrome, helper) = start(&app, CALLER, me());
    let mut socket = accept(&app);
    app_next(&mut socket);
    // Before the extension's hello there's no session to command.
    app_send(&mut socket, json!({"id": 1, "session": "s-1", "sequence": 1, "cmd": {"op": "read"}}));
    assert_eq!(chrome.next(), Some(json!({"type": "ended", "reason": "bad_command"})));
    assert_eq!(app_next(&mut socket), Some(json!({"type": "ended", "reason": "bad_command"})));
    assert!(matches!(helper.join().expect("the helper"), Ended::Refused(Refusal::BadCommand(_))));

    let app = self::app();
    let (mut chrome, helper) = start(&app, CALLER, me());
    let mut socket = accept(&app);
    app_next(&mut socket);
    chrome.send(hello(3));
    app_next(&mut socket);
    app_send(&mut socket, json!({"type": "stop"}));
    assert_eq!(chrome.next(), Some(json!({"type": "stop"})));
    assert_eq!(helper.join().expect("the helper"), Ended::Stopped);
    assert_eq!(app_next(&mut socket), None, "closed");
}
