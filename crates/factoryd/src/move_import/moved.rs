//! Moved mode: a Claktory that `factoryd export --final` moved to another
//! computer leaves `<home>/moved.json` behind (docs/MOVE.md). A backend that
//! finds it opens nothing of the Claktory: no agents run, no endpoint
//! starts, nothing is read but that file. It holds its port so the
//! supervisor stays content, and answers every client that it moved.

#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use factory_protocol::v1::{self, ErrorCode, ServerMessage, client_message, server_message::Body};
use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use serde::Deserialize;
use tokio_tungstenite::tungstenite::Message;

use crate::home::Home;

/// The tombstone, as export writes it.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Moved {
    #[serde(default)]
    pub export_id: String,
    #[serde(default)]
    pub at_ms: i64,
    #[serde(default)]
    pub epoch: u64,
    /// The computer it moved from: this one.
    #[serde(default)]
    pub machine: String,
    /// Where it moved: the new computer's name (a move over the network);
    /// empty when the move didn't say (a bundle, moved by hand).
    #[serde(default)]
    pub to: String,
    /// The new computer's system ("macos", "linux"); empty when the move
    /// didn't say.
    #[serde(default)]
    pub to_os: String,
    #[serde(default)]
    pub version: String,
    /// When the copy left here was deleted; 0 while it's kept.
    #[serde(default)]
    pub deleted_ms: i64,
}

fn path(home: &Home) -> PathBuf {
    home.root().join("moved.json")
}

/// Where a backend in moved mode writes its process id, so an import into
/// this home (moving back) can stop it.
fn pid_path(home: &Home) -> PathBuf {
    home.root().join("moved-mode.pid")
}

/// The tombstone, if this Claktory moved away. One that can't be read still
/// counts: it's never started by mistake.
pub fn read(home: &Home) -> Option<Moved> {
    let text = match std::fs::read_to_string(path(home)) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => return Some(Moved::default()),
    };
    Some(serde_json::from_str(&text).unwrap_or_default())
}

/// What clients are told: "This Claktory moved to another computer on 20
/// Nov 2026. It no longer runs here." and how to reach it.
pub fn said(moved: &Moved) -> String {
    let when = jiff::Timestamp::from_millisecond(moved.at_ms)
        .ok()
        .filter(|_| moved.at_ms > 0)
        .map(|at| format!(" on {}", at.to_zoned_datetime(jiff::tz::TimeZone::system()).strftime("%-d %b %Y")))
        .unwrap_or_default();
    let to = if moved.to.trim().is_empty() { "another computer".to_owned() } else { moved.to.trim().to_owned() };
    let back = if moved.deleted_ms == 0 { " To bring it back here instead, run factoryd export --restore on this computer." } else { "" };
    format!("This Claktory moved to {to}{when}. It no longer runs here: connect to it on the computer it moved to.{back}")
}

/// How long a client has to say hello.
const HELLO_WITHIN: Duration = Duration::from_secs(10);
/// How long this computer's own app's connection stays open after it's
/// told, for it to delete the copy left here (DeleteMovedCopy).
const KEPT_OPEN: Duration = Duration::from_secs(600);

/// Serves moved mode on `listener` until stopped: every client's Hello is
/// answered that it moved (ERROR_CODE_MOVED, with where to), and this
/// computer's own app may then delete the copy left here.
pub async fn serve(listener: tokio::net::TcpListener, home: &Home, moved: Moved) {
    tracing::warn!(machine = %moved.machine, epoch = moved.epoch, "this Claktory moved away; serving moved mode only");
    let _ = crate::remote::write_private(&pid_path(home), format!("{}\n", std::process::id()).as_bytes());
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
    // This computer's own app comes on its socket.
    let app = crate::authority::AppSocket::take(home);
    #[cfg(windows)]
    let mut terminate = crate::windows::stop::listen().inspect_err(|error| tracing::warn!(%error, "can't listen for being stopped")).ok();
    loop {
        let app_client = async {
            match &app {
                Some(app) => app.accept().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, peer)) = accepted else { continue };
                let home = home.clone();
                tokio::spawn(async move {
                    let Ok(socket) = tokio_tungstenite::accept_async(stream).await else { return };
                    client(socket, Via::Port { loopback: peer.ip().is_loopback() }, &home).await;
                });
            }
            accepted = app_client => {
                let Some((stream, checked)) = accepted else { continue };
                let home = home.clone();
                tokio::spawn(async move {
                    let Ok(socket) = tokio_tungstenite::accept_async(stream).await else { return };
                    client(socket, Via::App { own: checked.is_ok() }, &home).await;
                });
            }
            () = async { match terminate.as_mut() { Some(terminate) => { terminate.recv().await; } None => std::future::pending().await } } => break,
        }
    }
    let _ = std::fs::remove_file(pid_path(home));
}

/// How a client came: on the port (from this computer or not), or on the
/// app's socket, `own` if it's Claktory's own app.
enum Via {
    Port { loopback: bool },
    App { own: bool },
}

type Socket<S> = tokio_tungstenite::WebSocketStream<S>;

/// One client: told it moved, then, if it's this computer's own app, its
/// DeleteMovedCopy taken. Everything else is answered that it moved.
async fn client<S>(mut socket: Socket<S>, via: Via, home: &Home)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    // This computer's own app: on its socket, or, in test and QA builds,
    // with the person's token on the loopback port.
    let mut own = false;
    loop {
        // Its first message has 10 s; only this computer's own app is then
        // kept waiting on.
        let wait = if own { KEPT_OPEN } else { HELLO_WITHIN };
        let Ok(Some(Ok(frame))) = tokio::time::timeout(wait, socket.next()).await else { break };
        let message = match frame {
            Message::Binary(bytes) => match v1::ClientMessage::decode(bytes) {
                Ok(message) => message,
                Err(_) => break,
            },
            Message::Close(_) => break,
            _ => continue,
        };
        let mut done = false;
        let body = match message.body {
            // A program on the app's socket that isn't this computer's own
            // app is turned away as in normal mode, so it never offers a
            // Delete it couldn't make.
            Some(client_message::Body::Hello(_)) if matches!(via, Via::App { own: false }) => Body::Error(v1::Error {
                code: ErrorCode::Unauthorized.into(),
                message: crate::authority::not_the_app().to_owned(),
                ..Default::default()
            }),
            Some(client_message::Body::Hello(hello)) => {
                own = match via {
                    Via::App { own } => own,
                    Via::Port { loopback } => loopback && crate::authority::is_person_token(home, &hello.token),
                };
                moved_error(home)
            }
            Some(client_message::Body::DeleteMovedCopy(request)) => {
                let answer = delete(home, own, &request.typed_word).await;
                done = matches!(answer, Body::Ack(_));
                answer
            }
            _ => moved_error(home),
        };
        // Anyone but this computer's own app is answered once and let go.
        if !send(&mut socket, message.id, body).await || done || !own {
            break;
        }
    }
    let _ = socket.close(None).await;
}

async fn send<S>(socket: &mut Socket<S>, reply_to: u64, body: Body) -> bool
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let answer = ServerMessage { reply_to, body: Some(body), ..Default::default() };
    socket.send(Message::Binary(answer.encode_to_vec().into())).await.is_ok()
}

/// The answer to everything: it moved, and where to, as moved.json says now.
fn moved_error(home: &Home) -> Body {
    let moved = read(home).unwrap_or_default();
    let away = v1::MovedAway {
        to: moved.to.clone(),
        to_os: moved.to_os.clone(),
        at_ms: moved.at_ms,
        machine: moved.machine.clone(),
        deleted_ms: moved.deleted_ms,
    };
    Body::Error(v1::Error { code: ErrorCode::Moved.into(), message: said(&moved), moved: Some(away), ..Default::default() })
}

/// DeleteMovedCopy: the copy left here, deleted for good (as `factoryd
/// export --delete-moved`), when this computer's app asks and the person
/// typed the word.
async fn delete(home: &Home, own: bool, typed: &str) -> Body {
    let refused = |code: ErrorCode, why: &str| Body::Error(v1::Error { code: code.into(), message: why.to_owned(), ..Default::default() });
    if !own {
        return refused(ErrorCode::Unauthorized, "Only this computer's own Claktory app can delete the copy left here.");
    }
    if typed != "delete" {
        return refused(ErrorCode::InvalidRequest, "Type delete to confirm.");
    }
    if read(home).is_some_and(|moved| moved.deleted_ms != 0) {
        return refused(ErrorCode::InvalidRequest, "The copy left here was already deleted.");
    }
    let place = crate::move_export::Place {
        home: home.clone(),
        port: 0,
        user_home: std::env::home_dir().unwrap_or_default(),
        dev: false,
        service: crate::move_export::Service::None,
    };
    let deleted = tokio::task::spawn_blocking(move || {
        let deleted = crate::move_export::delete_moved(&place);
        // The deletion takes the process id with the rest; moving back
        // here must still find this backend to stop it.
        let _ = crate::remote::write_private(&pid_path(&place.home), format!("{}\n", std::process::id()).as_bytes());
        deleted
    })
    .await;
    match deleted {
        Ok(Ok(_)) => {
            tracing::warn!("the copy this Claktory left here was deleted from its app");
            Body::Ack(v1::Ack {})
        }
        Ok(Err(refusal)) => refused(ErrorCode::Internal, &refusal.reason),
        Err(error) => refused(ErrorCode::Internal, &error.to_string()),
    }
}

/// A backend in moved mode runs for this home.
pub(crate) fn held(home: &Home) -> bool {
    std::fs::read_to_string(pid_path(home))
        .ok()
        .and_then(|text| text.trim().parse::<i32>().ok())
        .is_some_and(|pid| pid > 0 && is_factoryd(pid))
}

/// Stops this home's backend in moved mode, if one runs: an import is
/// bringing a Claktory back here. Whether one was asked to stop.
pub(crate) fn stop_held(home: &Home) -> bool {
    let Some(pid) = std::fs::read_to_string(pid_path(home)).ok().and_then(|text| text.trim().parse::<i32>().ok()) else { return false };
    if pid <= 0 || !is_factoryd(pid) {
        return false;
    }
    // SAFETY: kill(2) on the process that wrote its id in this home.
    #[cfg(unix)]
    return unsafe { libc::kill(pid, libc::SIGTERM) == 0 };
    #[cfg(windows)]
    return crate::windows::stop::ask_to_stop(pid as u32).is_ok();
}

/// `pid` is a factoryd (not another process that took the number since).
#[cfg(windows)]
fn is_factoryd(pid: i32) -> bool {
    use crate::windows::ports::{Owner, owner};
    matches!(owner(pid as u32), Some(Owner::Mine { image, .. }) if image.eq_ignore_ascii_case("factoryd.exe"))
}

/// `pid` is a factoryd (not another process that took the number since).
#[cfg(unix)]
fn is_factoryd(pid: i32) -> bool {
    let Ok(output) = std::process::Command::new("ps").args(["-o", "command=", "-p", &pid.to_string()]).output() else { return false };
    let command = String::from_utf8_lossy(&output.stdout);
    command.split_whitespace().next().is_some_and(|program| Path::new(program).file_name().is_some_and(|name| name == "factoryd"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tombstone_that_cant_be_read_still_counts() {
        let dir = std::env::temp_dir().join(format!("moved-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let home = Home::new(dir.clone());
        assert!(read(&home).is_none());
        std::fs::write(dir.join("moved.json"), "{not json").expect("write");
        assert!(read(&home).is_some());
        std::fs::write(
            dir.join("moved.json"),
            r#"{"format":1,"export_id":"ab","at_ms":1763735100000,"epoch":2,"machine":"Mac","to":"","version":"v0.1.14","bundle":"/x"}"#,
        )
        .expect("write");
        let moved = read(&home).expect("moved");
        assert_eq!((moved.epoch, moved.machine.as_str()), (2, "Mac"));
        assert!(said(&moved).starts_with("This Claktory moved to another computer on "), "{}", said(&moved));
        let named = Moved { to: "cloud-box".to_owned(), ..moved };
        assert!(said(&named).starts_with("This Claktory moved to cloud-box on "), "{}", said(&named));
        assert!(said(&named).ends_with("run factoryd export --restore on this computer."), "{}", said(&named));
        let deleted = Moved { deleted_ms: 1763735200000, ..named };
        assert!(!said(&deleted).contains("--restore"), "a deleted copy can't come back: {}", said(&deleted));
        let _ = std::fs::remove_dir_all(dir);
    }
}
