//! The protocol driver: one connection to a sandbox per command, let in as
//! the person on the loopback port (`<home>/qa-person-token`, which factoryd
//! built with its qa-person feature writes; `remote/local-pairing`'s token
//! from a commit before the app's socket), sending any request as JSON and printing what
//! comes back as JSON. Requests and answers go through the protocol's
//! descriptors (`build.rs`), so a new request needs no code here: its name
//! and its fields as the proto file has them.
//!
//! Requests that would reach outside the sandbox are refused before they're
//! sent (`check_request`).

use std::sync::LazyLock;
use std::time::Duration;

use factory_protocol::v1::{self, ClientMessage, ServerMessage, client_message, server_message::Body};
use futures_util::{SinkExt as _, StreamExt as _};
use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, Kind, MessageDescriptor, SerializeOptions};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message as Frame;

use crate::Failure;
use crate::sandbox::Sandbox;

static POOL: LazyLock<DescriptorPool> = LazyLock::new(|| {
    DescriptorPool::decode(include_bytes!(concat!(env!("OUT_DIR"), "/factory.bin")).as_ref()).expect("the protocol's descriptors")
});

fn descriptor(name: &str) -> MessageDescriptor {
    POOL.get_message_by_name(&format!("factory.v1.{name}")).expect("a message of the protocol")
}

/// How messages print: every field, defaults too (so `account.skippable`
/// reads false rather than missing), named as the proto file names them.
fn options() -> SerializeOptions {
    SerializeOptions::new().skip_default_fields(false).use_proto_field_name(true).stringify_64_bit_integers(false)
}

/// A message of the protocol as JSON.
pub fn to_json(name: &str, bytes: &[u8]) -> Value {
    match DynamicMessage::decode(descriptor(name), bytes) {
        Ok(message) => message.serialize_with_options(serde_json::value::Serializer, &options()).unwrap_or(Value::Null),
        Err(error) => json!({ "undecodable": error.to_string() }),
    }
}

pub fn workspace_json(workspace: &v1::Workspace) -> Value {
    to_json("Workspace", &workspace.encode_to_vec())
}

/// The request field of ClientMessage whose message is `name`
/// ("CreatePipeInvite", or its field's name, "create_pipe_invite").
fn request_field(name: &str) -> Option<prost_reflect::FieldDescriptor> {
    let wanted = name.replace(['_', '-'], "").to_lowercase();
    descriptor("ClientMessage").fields().filter(|field| field.containing_oneof().is_some()).find(|field| {
        let message = match field.kind() {
            Kind::Message(message) => message.name().to_lowercase(),
            _ => String::new(),
        };
        message == wanted || field.name().replace('_', "") == wanted
    })
}

/// Every request's name, as the proto file has them.
pub fn requests() -> Vec<String> {
    descriptor("ClientMessage")
        .fields()
        .filter(|field| field.containing_oneof().is_some() && field.name() != "hello")
        .filter_map(|field| match field.kind() {
            Kind::Message(message) => Some(message.name().to_owned()),
            _ => None,
        })
        .collect()
}

/// A request's fields: name, type, and whether it repeats or is optional.
pub fn fields(name: &str) -> Result<Value, Failure> {
    let field = request_field(name).ok_or_else(|| unknown(name))?;
    let Kind::Message(message) = field.kind() else { return Err(unknown(name)) };
    Ok(describe(&message))
}

fn describe(message: &MessageDescriptor) -> Value {
    let fields: Vec<Value> = message
        .fields()
        .map(|field| {
            let kind = match field.kind() {
                Kind::Message(inner) => inner.name().to_owned(),
                Kind::Enum(inner) => {
                    format!("{} ({})", inner.name(), inner.values().map(|value| value.name().to_owned()).collect::<Vec<_>>().join(", "))
                }
                other => format!("{other:?}").to_lowercase(),
            };
            let shape = if field.is_map() {
                "map"
            } else if field.is_list() {
                "repeated"
            } else if field.supports_presence() && !matches!(field.kind(), Kind::Message(_)) {
                "optional"
            } else {
                ""
            };
            json!({ "name": field.name(), "type": kind, "shape": shape })
        })
        .collect();
    json!({ "request": message.name(), "fields": fields })
}

fn unknown(name: &str) -> Failure {
    Failure::usage(format!("There's no request called {name}. `claktory-qa requests` lists them."))
}

/// `url` is on this computer (http to localhost or 127.0.0.1), so nothing
/// the sandbox does with it leaves it. Empty is refused too: for the
/// account service it means claktory.com.
pub fn check_local(url: &str, what: &str) -> Result<(), Failure> {
    let rest = url.trim().strip_prefix("http://").unwrap_or("");
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once(':').map_or(host, |(host, _)| host);
    if matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
        Ok(())
    } else {
        Err(Failure::refused(format!(
            "A sandbox's {what} must be on this computer (http://127.0.0.1:PORT), not \"{url}\": nothing in a sandbox may reach claktory.com or the real Claktories."
        )))
    }
}

/// Refuses requests that would reach outside the sandbox.
fn check_request(name: &str, body: &Value) -> Result<(), Failure> {
    let text = |key: &str| body.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
    match name {
        "StartLogin" | "AnswerLogin" => {
            return Err(Failure::refused(
                "A sandbox has no real model: an agent in it would run with this computer's files. Script it instead (Theo's L1 rule).",
            ));
        }
        "StartConnect" | "FinishConnect" => return Err(Failure::refused("A sandbox connects no real services.")),
        "InstallUpdate" => return Err(Failure::refused("A sandbox doesn't install updates: it runs what scripts/qa.sh built.")),
        "SetAccountService" => check_local(&text("url"), "account service")?,
        "SetRemoteAccess" => {
            for relay in body.get("relays").and_then(Value::as_array).into_iter().flatten() {
                check_local(relay.as_str().unwrap_or(""), "relay")?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// A request as wire bytes, from its name and JSON fields.
pub fn encode(name: &str, body: &Value, id: u64) -> Result<Vec<u8>, Failure> {
    let field = request_field(name).ok_or_else(|| unknown(name))?;
    let Kind::Message(message) = field.kind() else { return Err(unknown(name)) };
    check_request(message.name(), body)?;
    let request = DynamicMessage::deserialize(message.clone(), body.clone()).map_err(|error| {
        Failure::usage(format!(
            "{} doesn't take that: {error}. `claktory-qa requests {}` shows its fields.",
            message.name(),
            message.name()
        ))
    })?;
    let mut envelope = DynamicMessage::new(descriptor("ClientMessage"));
    envelope.set_field_by_name("id", prost_reflect::Value::U64(id));
    envelope.set_field(&field, prost_reflect::Value::Message(request));
    Ok(envelope.encode_to_vec())
}

/// One connection to a sandbox, past Hello.
pub struct Connection {
    socket: tokio_tungstenite::WebSocketStream<TcpStream>,
    next: u64,
    /// What came while `request` waited for its answer, for `receive`.
    held: std::collections::VecDeque<ServerMessage>,
    /// The Workspace as the backend last sent it.
    pub workspace: v1::Workspace,
}

/// How long to wait for an answer.
const ANSWER_WAIT: Duration = Duration::from_secs(30);

impl Connection {
    pub async fn open(sandbox: &Sandbox) -> Result<Connection, Failure> {
        let address = format!("127.0.0.1:{}", sandbox.port);
        let down =
            |why: String| Failure::unreachable(format!("The sandbox {} isn't running ({why}): `claktory-qa up` starts one.", sandbox.name));
        let stream = TcpStream::connect(&address).await.map_err(|error| down(error.to_string()))?;
        let (socket, _) =
            tokio_tungstenite::client_async(format!("ws://{address}"), stream).await.map_err(|error| down(error.to_string()))?;
        // The QA person's token (factoryd's qa-person feature), or,
        // from a commit before it, this machine's pairing code's.
        let token = std::fs::read_to_string(sandbox.home().join("qa-person-token"))
            .ok()
            .map(|token| token.trim().to_owned())
            .or_else(|| {
                let code = std::fs::read_to_string(sandbox.home().join("remote/local-pairing")).ok()?;
                code.trim().split(['?', '&']).find_map(|part| part.strip_prefix("t=").map(str::to_owned))
            })
            .unwrap_or_default();
        let mut connection = Connection { socket, next: 1, held: Default::default(), workspace: v1::Workspace::default() };
        let hello = v1::Hello {
            protocol_version: factory_protocol::PROTOCOL_VERSION,
            client_version: format!("claktory-qa {}", env!("CARGO_PKG_VERSION")),
            token,
            ..Default::default()
        };
        connection.send_body(client_message::Body::Hello(hello)).await?;
        let (mut welcomed, mut has_workspace) = (false, false);
        while !(welcomed && has_workspace) {
            let message = connection.receive(ANSWER_WAIT).await?.ok_or_else(|| down("it closed the connection".to_owned()))?;
            match message.body {
                Some(Body::Welcome(_)) => welcomed = true,
                Some(Body::Workspace(_)) => has_workspace = true,
                Some(Body::Error(error)) => {
                    return Err(Failure::refused(format!("The sandbox turned claktory-qa away: {}", error.message)));
                }
                _ => {}
            }
        }
        Ok(connection)
    }

    async fn send_body(&mut self, body: client_message::Body) -> Result<u64, Failure> {
        let id = self.next;
        self.next += 1;
        let bytes = ClientMessage { id, body: Some(body) }.encode_to_vec();
        self.socket.send(Frame::Binary(bytes.into())).await.map_err(|error| Failure::unreachable(format!("Lost the sandbox: {error}")))?;
        Ok(id)
    }

    /// The next message, keeping the Workspace; None when it closed, or
    /// nothing came within `wait`.
    pub async fn receive(&mut self, wait: Duration) -> Result<Option<ServerMessage>, Failure> {
        if let Some(message) = self.held.pop_front() {
            return Ok(Some(message));
        }
        self.next_message(wait).await
    }

    async fn next_message(&mut self, wait: Duration) -> Result<Option<ServerMessage>, Failure> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let frame = match tokio::time::timeout_at(deadline, self.socket.next()).await {
                Err(_) => return Ok(None),
                Ok(None) => return Err(Failure::unreachable("The sandbox closed the connection.")),
                Ok(Some(Err(error))) => return Err(Failure::unreachable(format!("Lost the sandbox: {error}"))),
                Ok(Some(Ok(frame))) => frame,
            };
            let Frame::Binary(bytes) = frame else { continue };
            let Ok(message) = ServerMessage::decode(bytes) else { continue };
            if let Some(Body::Workspace(workspace)) = &message.body {
                self.workspace = workspace.clone();
            }
            return Ok(Some(message));
        }
    }

    /// Sends `name` with `body` and returns its answer: Ok with the answer's
    /// JSON (`{"ack": {}}`, `{"pipe_invite": {…}}`), or the backend's error.
    pub async fn request(&mut self, name: &str, body: &Value) -> Result<Value, Failure> {
        let id = self.next;
        self.next += 1;
        let bytes = encode(name, body, id)?;
        self.socket.send(Frame::Binary(bytes.into())).await.map_err(|error| Failure::unreachable(format!("Lost the sandbox: {error}")))?;
        loop {
            let Some(message) = self.next_message(ANSWER_WAIT).await? else {
                return Err(Failure::failed(format!("No answer to {name} in {} s.", ANSWER_WAIT.as_secs())));
            };
            if message.reply_to != id {
                self.held.push_back(message);
                continue;
            }
            if let Some(Body::Error(error)) = &message.body {
                return Err(Failure::backend(error));
            }
            let answer = to_json("ServerMessage", &message.encode_to_vec());
            return Ok(body_of(answer));
        }
    }
}

/// A ServerMessage's JSON without its envelope (reply_to, the Workspace
/// revisions): only its body, `{"ack": {}}`.
pub fn body_of(message: Value) -> Value {
    let envelope = descriptor("ServerMessage");
    let envelope: Vec<_> = envelope.fields().filter(|field| field.containing_oneof().is_none()).map(|field| field.name().to_owned()).collect();
    match message {
        Value::Object(fields) => Value::Object(
            fields.into_iter().filter(|(key, value)| !envelope.iter().any(|name| name == key) && !value.is_null()).collect(),
        ),
        other => other,
    }
}

/// What a dotted path names in `value`: `account.state`, `agents[0].name`,
/// `agents[name=Lea].status` (the first element whose field is that).
pub fn select<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for step in steps(path)? {
        current = match step {
            Step::Key(key) => current.get(key)?,
            Step::Index(index) => current.get(index)?,
            Step::Match(key, wanted) => {
                current.as_array()?.iter().find(|item| item.get(&key).is_some_and(|value| plain(value) == wanted))?
            }
        };
    }
    Some(current)
}

enum Step {
    Key(String),
    Index(usize),
    Match(String, String),
}

fn steps(path: &str) -> Option<Vec<Step>> {
    let mut steps = Vec::new();
    let mut rest = path.trim();
    while !rest.is_empty() {
        if let Some(inner) = rest.strip_prefix('[') {
            let end = inner.find(']')?;
            let selector = &inner[..end];
            steps.push(match selector.split_once('=') {
                Some((key, value)) => Step::Match(key.trim().to_owned(), value.trim().to_owned()),
                None => Step::Index(selector.trim().parse().ok()?),
            });
            rest = &inner[end + 1..];
        } else {
            let rest_ = rest.strip_prefix('.').unwrap_or(rest);
            let end = rest_.find(['.', '[']).unwrap_or(rest_.len());
            if end == 0 {
                return None;
            }
            steps.push(Step::Key(rest_[..end].to_owned()));
            rest = &rest_[end..];
        }
    }
    Some(steps)
}

/// A value as a person compares it: strings without their quotes.
pub fn plain(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_request_goes_from_json_to_the_wire() {
        let bytes = encode("CreatePipeInvite", &json!({"project_id": "site", "name": "Launch"}), 7).expect("encoded");
        let message = ClientMessage::decode(bytes.as_slice()).expect("decoded");
        assert_eq!(message.id, 7);
        let Some(client_message::Body::CreatePipeInvite(invite)) = message.body else { panic!("{message:?}") };
        assert_eq!((invite.project_id.as_str(), invite.name.as_str()), ("site", "Launch"));
        // The field's name and lowerCamelCase work as well.
        assert!(encode("create_pipe_invite", &json!({"projectId": "site"}), 1).is_ok());
        assert!(encode("NoSuchThing", &json!({}), 1).is_err());
        assert!(encode("CreatePipeInvite", &json!({"nonsense": 1}), 1).is_err());
        assert!(requests().contains(&"SendMessage".to_owned()));
        assert!(!requests().contains(&"Hello".to_owned()));
    }

    #[test]
    fn requests_that_reach_outside_are_refused() {
        for (name, body) in [
            ("SetAccountService", json!({"url": ""})),
            ("SetAccountService", json!({"url": "https://account.claktory.com"})),
            ("SetAccountService", json!({"url": "http://example.com"})),
            ("SetRemoteAccess", json!({"relays": ["https://relay.example.com/"], "set_relays": true})),
            ("StartLogin", json!({"provider_id": "anthropic"})),
            ("InstallUpdate", json!({})),
        ] {
            assert!(encode(name, &body, 1).is_err(), "{name} {body}");
        }
        assert!(encode("SetAccountService", &json!({"url": "http://127.0.0.1:7600"}), 1).is_ok());
        assert!(encode("SetRemoteAccess", &json!({"relays": ["http://127.0.0.1:9/"], "set_relays": true}), 1).is_ok());
    }

    #[test]
    fn paths_pick_values_out() {
        let workspace = json!({"account": {"state": "CLAKTORY_ACCOUNT_STATE_SIGNED_IN"}, "agents": [{"id": "rivet", "name": "Rivet"}, {"id": "lea", "name": "Lea"}]});
        assert_eq!(select(&workspace, "account.state").map(plain).as_deref(), Some("CLAKTORY_ACCOUNT_STATE_SIGNED_IN"));
        assert_eq!(select(&workspace, "agents[1].id").map(plain).as_deref(), Some("lea"));
        assert_eq!(select(&workspace, "agents[name=Lea].id").map(plain).as_deref(), Some("lea"));
        assert_eq!(select(&workspace, "agents[name=Bo].id"), None);
        assert_eq!(select(&workspace, "").map(|value| value == &workspace), Some(true));
    }

    #[test]
    fn answers_print_as_json() {
        let workspace = v1::Workspace {
            account: Some(v1::ClaktoryAccount { email: "qa@example.test".into(), ..Default::default() }),
            ..Default::default()
        };
        let json = workspace_json(&workspace);
        assert_eq!(select(&json, "account.email").map(plain).as_deref(), Some("qa@example.test"));
        assert_eq!(select(&json, "account.skippable"), Some(&Value::Bool(false)));
        let fields = fields("CreatePipeInvite").expect("fields");
        assert_eq!(select(&fields, "fields[name=project_id].type").map(plain).as_deref(), Some("string"));
    }
}
