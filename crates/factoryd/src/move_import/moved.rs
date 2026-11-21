//! Moved mode: a Claktory that `factoryd export --final` moved to another
//! computer leaves `<home>/moved.json` behind (docs/MOVE.md). A backend that
//! finds it opens nothing of the Claktory: no agents run, no endpoint
//! starts, nothing is read but that file. It holds its port so the
//! supervisor stays content, and answers every client that it moved.

use std::path::{Path, PathBuf};

use factory_protocol::v1::{self, ErrorCode, ServerMessage, server_message::Body};
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
    #[serde(default)]
    pub version: String,
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
    format!(
        "This Claktory moved to {to}{when}. It no longer runs here: connect to it on the computer it moved to. To bring it back here instead, run factoryd export --restore on this computer."
    )
}

/// Serves moved mode on `listener` until stopped: every client's Hello is
/// answered with why, then closed.
pub async fn serve(listener: tokio::net::TcpListener, home: &Home, moved: Moved) {
    let said = said(&moved);
    tracing::warn!(machine = %moved.machine, epoch = moved.epoch, "this Claktory moved away; serving moved mode only");
    let _ = crate::remote::write_private(&pid_path(home), format!("{}\n", std::process::id()).as_bytes());
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            () = async { match terminate.as_mut() { Some(terminate) => { terminate.recv().await; } None => std::future::pending().await } } => break,
        };
        let Ok((stream, _)) = accepted else { continue };
        let said = said.clone();
        tokio::spawn(async move {
            let Ok(mut socket) = tokio_tungstenite::accept_async(stream).await else { return };
            let mut reply_to = 0;
            if let Ok(Some(Ok(Message::Binary(bytes)))) = tokio::time::timeout(std::time::Duration::from_secs(10), socket.next()).await
                && let Ok(message) = v1::ClientMessage::decode(bytes)
            {
                reply_to = message.id;
            }
            let error = v1::Error { code: ErrorCode::Internal.into(), message: said };
            let answer = ServerMessage { reply_to, body: Some(Body::Error(error)) };
            let _ = socket.send(Message::Binary(answer.encode_to_vec().into())).await;
            let _ = socket.close(None).await;
        });
    }
    let _ = std::fs::remove_file(pid_path(home));
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
    unsafe { libc::kill(pid, libc::SIGTERM) == 0 }
}

/// `pid` is a factoryd (not another process that took the number since).
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
        let _ = std::fs::remove_dir_all(dir);
    }
}
