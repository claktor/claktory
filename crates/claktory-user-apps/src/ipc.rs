//! The channel between the account service (the root) and the user-apps
//! worker: one request per connection over a local (Unix) socket, which only
//! the root may connect to.
//!
//! The root authenticates every HTTP call under `/v1/apps/` (a Claktory's
//! backend-signed request, or a person's device-signed one) and sends the
//! worker what it verified, a typed [`Caller`], with the call's method, path,
//! query and bytes; it parses nothing of the app's. The worker answers with a
//! status and a JSON body, which the root checks ([`acceptable`]) and signs
//! as it signs every answer. The worker holds none of the root's keys, so a
//! bug in an app can't sign in a device, release a vault share or sign
//! anything but its own answers' bytes, through the root, for the request
//! they answer.
//!
//! A frame is a 4-byte big-endian length and that many bytes of JSON (the
//! head), then a 4-byte length and the body's bytes.

use std::io;
use std::path::Path;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The channel's version: a head of another is refused.
pub const VERSION: u32 = 1;
/// The largest head.
pub const HEAD_LIMIT: usize = 64 * 1024;
/// The largest body, either way: the root reads no more of a request.
pub const BODY_LIMIT: usize = 1024 * 1024;
/// How long the root waits for an answer: a long poll's 25 seconds, with
/// a busy worker's wait for a turn (5 seconds) before and after, and some.
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(40);

/// Who a call is from, as the root verified it. Nothing here comes from the
/// caller unchecked except an agent's claims, which its backend signed.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Caller {
    /// A person's device, signed in to the account with its key.
    Device {
        account_id: String,
        /// The device's account key (base64url), which signed the call.
        device_key: String,
        /// The account's Claktories signed in now, each with its
        /// registration's epoch: a grant made under another epoch is dead.
        registrations: Vec<Registration>,
    },
    /// The person, signed in at the account page (claktory.com/account)
    /// with an emailed code, on no device of the account's.
    Account {
        account_id: String,
        /// As for a device.
        registrations: Vec<Registration>,
    },
    /// A Claktory's backend (its machine token and backend key), at the
    /// epoch it's registered under now, for itself or for one of its agents.
    Backend {
        account_id: String,
        claktory_id: String,
        epoch: u64,
        /// The agent it acts for (`?agent=&incarnation=` on the call, which
        /// the backend's signature covers); None for the backend itself.
        agent: Option<Actor>,
    },
}

impl Caller {
    pub fn account_id(&self) -> &str {
        match self {
            Caller::Device { account_id, .. } | Caller::Account { account_id, .. } | Caller::Backend { account_id, .. } => account_id,
        }
    }
}

/// A Claktory signed in to the account, and its registration's epoch.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    pub claktory_id: String,
    pub epoch: u64,
}

/// An agent as its Claktory names it: its id and its incarnation, which a
/// new agent of the same id doesn't share.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Actor {
    pub id: String,
    pub incarnation: String,
}

/// What the root asks the worker.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    /// An HTTP call under `/v1/apps/`, authenticated; its body follows.
    Call {
        method: String,
        /// The whole path, `/v1/apps/…`.
        path: String,
        /// The query, without `agent` and `incarnation`, which are in the
        /// caller.
        query: String,
        /// The root's time, unix seconds: the worker keeps no clock of its own.
        time: i64,
        caller: Caller,
    },
    /// Whether the worker is up: 200 when it is.
    Health,
    /// Forget everything of an account, which was deleted (its calls no
    /// longer authenticate, so this may come late).
    PurgeAccount { account_id: String, time: i64 },
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct RequestHead {
    v: u32,
    request: Request,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct AnswerHead {
    v: u32,
    status: u16,
}

/// The statuses an answer may have; any other is the worker's failure.
/// 401 isn't one: only the root says who's signed in.
const STATUSES: &[u16] = &[200, 201, 202, 204, 400, 403, 404, 409, 410, 413, 422, 429, 503];

/// Whether the root may pass on an answer: a status of `STATUSES` and a JSON
/// body (empty only with 204), so an app can't make the service serve a
/// page, a script or a redirect.
pub fn acceptable(status: u16, body: &[u8]) -> bool {
    if !STATUSES.contains(&status) {
        return false;
    }
    if status == 204 {
        return body.is_empty();
    }
    serde_json::from_slice::<serde::de::IgnoredAny>(body).is_ok()
}

#[derive(Debug)]
pub enum IpcError {
    /// No worker listens (not running, or not configured).
    Unavailable(io::Error),
    /// It took longer than `ANSWER_TIMEOUT`.
    Timeout,
    /// What came back isn't a frame of this version.
    Broken(String),
    /// What listens on the socket runs as another user than the worker's:
    /// it gets nothing.
    WrongPeer(u32),
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IpcError::Unavailable(error) => write!(f, "the user-apps worker isn't reachable: {error}"),
            IpcError::Timeout => write!(f, "the user-apps worker didn't answer in time"),
            IpcError::Broken(why) => write!(f, "the user-apps worker answered something else: {why}"),
            IpcError::WrongPeer(uid) => write!(f, "what listens on the user-apps socket runs as uid {uid}, not the worker's: nothing sent"),
        }
    }
}

impl std::error::Error for IpcError {}

/// Writes a frame: the head as JSON, then the body.
pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, head: &impl Serialize, body: &[u8]) -> io::Result<()> {
    let head = serde_json::to_vec(head).map_err(io::Error::other)?;
    if head.len() > HEAD_LIMIT || body.len() > BODY_LIMIT {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "too large a frame"));
    }
    let mut frame = Vec::with_capacity(8 + head.len() + body.len());
    frame.extend_from_slice(&(head.len() as u32).to_be_bytes());
    frame.extend_from_slice(&head);
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(body);
    writer.write_all(&frame).await?;
    writer.flush().await
}

async fn read_part<R: AsyncRead + Unpin>(reader: &mut R, limit: usize) -> io::Result<Vec<u8>> {
    let length = reader.read_u32().await? as usize;
    if length > limit {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("a part of {length} bytes, over {limit}")));
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes).await?;
    Ok(bytes)
}

/// Reads a frame: its head, parsed, and its body.
pub async fn read_frame<R: AsyncRead + Unpin, T: DeserializeOwned>(reader: &mut R) -> io::Result<(T, Vec<u8>)> {
    let head = read_part(reader, HEAD_LIMIT).await?;
    let head = serde_json::from_slice(&head).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let body = read_part(reader, BODY_LIMIT).await?;
    Ok((head, body))
}

/// Reads a request, as the worker does.
pub async fn read_request<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<(Request, Vec<u8>)> {
    let (head, body): (RequestHead, _) = read_frame(reader).await?;
    if head.v != VERSION {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("version {}", head.v)));
    }
    Ok((head.request, body))
}

/// Writes an answer, as the worker does.
pub async fn write_answer<W: AsyncWrite + Unpin>(writer: &mut W, status: u16, body: &[u8]) -> io::Result<()> {
    write_frame(writer, &AnswerHead { v: VERSION, status }, body).await
}

/// Asks the worker listening at `socket`, as the root does: its answer's
/// status and body, within `ANSWER_TIMEOUT`. Whatever listens there must
/// run as `worker_uid`, the worker's user, or it's sent nothing: while the
/// worker is down, something else might have put a socket there.
#[cfg(unix)]
pub async fn ask(socket: &Path, worker_uid: u32, request: &Request, body: &[u8]) -> Result<(u16, Vec<u8>), IpcError> {
    ask_within(socket, worker_uid, request, body, ANSWER_TIMEOUT).await
}

#[cfg(unix)]
pub async fn ask_within(
    socket: &Path,
    worker_uid: u32,
    request: &Request,
    body: &[u8],
    timeout: Duration,
) -> Result<(u16, Vec<u8>), IpcError> {
    let exchange = async {
        let mut stream = tokio::net::UnixStream::connect(socket).await.map_err(IpcError::Unavailable)?;
        let peer = stream.peer_cred().map_err(IpcError::Unavailable)?.uid();
        if peer != worker_uid {
            return Err(IpcError::WrongPeer(peer));
        }
        let head = RequestHead { v: VERSION, request: request.clone() };
        write_frame(&mut stream, &head, body).await.map_err(|error| IpcError::Broken(error.to_string()))?;
        let (head, body): (AnswerHead, _) = read_frame(&mut stream).await.map_err(|error| IpcError::Broken(error.to_string()))?;
        if head.v != VERSION {
            return Err(IpcError::Broken(format!("version {}", head.v)));
        }
        Ok((head.status, body))
    };
    tokio::time::timeout(timeout, exchange).await.unwrap_or(Err(IpcError::Timeout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_json_answers_with_known_statuses_pass() {
        assert!(acceptable(200, b"{\"ok\":true}"));
        assert!(acceptable(404, b"{\"error\":\"not_found\"}"));
        assert!(acceptable(204, b""));
        assert!(!acceptable(204, b"{}"), "nothing with 204");
        assert!(!acceptable(200, b"<script>alert(1)</script>"), "not JSON");
        assert!(!acceptable(200, b""), "empty");
        assert!(!acceptable(302, b"{}"), "no redirects");
        assert!(!acceptable(401, b"{}"), "only the root says who's signed in");
        assert!(!acceptable(500, b"{}"));
    }

    #[tokio::test]
    async fn frames_read_back_and_refuse_what_is_too_large_or_of_another_version() {
        let request = Request::Call {
            method: "GET".into(),
            path: "/v1/apps/whoami".into(),
            query: String::new(),
            time: 1,
            caller: Caller::Backend { account_id: "a".into(), claktory_id: "c".into(), epoch: 2, agent: None },
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &RequestHead { v: VERSION, request: request.clone() }, b"body").await.expect("written");
        let (read, body) = read_request(&mut bytes.as_slice()).await.expect("read");
        assert_eq!((read, body.as_slice()), (request.clone(), &b"body"[..]));

        let mut other = Vec::new();
        write_frame(&mut other, &RequestHead { v: 2, request }, b"").await.expect("written");
        assert!(read_request(&mut other.as_slice()).await.is_err(), "another version");

        let mut huge = ((HEAD_LIMIT + 1) as u32).to_be_bytes().to_vec();
        huge.extend(vec![b' '; 16]);
        assert!(read_request(&mut huge.as_slice()).await.is_err(), "too large a head, refused before it's read");

        let unknown = br#"{"v":1,"request":{"type":"health"},"extra":1}"#;
        let mut frame = (unknown.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(unknown);
        frame.extend_from_slice(&0u32.to_be_bytes());
        assert!(read_request(&mut frame.as_slice()).await.is_err(), "unknown fields");
    }
}
