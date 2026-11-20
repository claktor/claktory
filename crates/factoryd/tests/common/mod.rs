#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use factory_agent::auth::{AuthEvent, AuthInteraction, AuthPrompt};
use factory_agent::stream::event_stream;
use factory_agent::types::ModelCost;
use factory_agent::{
    AssistantMessage, AssistantPart, Error, Message, Model, StopReason, StreamEvent, StreamFn, Usage, UserContent, UserPart,
};
use factory_protocol::PROTOCOL_VERSION;
use factory_protocol::v1::{self, ClientMessage, Hello, ServerMessage, client_message, server_message::Body};
use factoryd::Factory;
use factoryd::home::Home;
use factoryd::models::{API_KEY, Models, OAUTH};
use futures_util::future::BoxFuture;
use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message as Frame;
use tokio_tungstenite::{WebSocketStream, client_async};

pub const MODEL: &str = "scripted/echo";
/// Another model, which works like `MODEL`.
pub const OTHER_MODEL: &str = "scripted/parrot";
/// How long a test waits for the server before calling it a hang. Never a
/// measure of speed: a test process starved of the processor (agents' builds
/// and tests run beside it) can stall for many seconds, and only a real hang
/// should fail.
const WAIT: Duration = Duration::from_secs(60);

/// A fresh directory for one test.
/// Lets this process open as many files as the tests need: every test of
/// the crate runs in it (tests/all), each with a Factory of its own, and
/// macOS starts a process at 256.
pub fn raise_open_files() {
    static RAISED: std::sync::Once = std::sync::Once::new();
    RAISED.call_once(|| {
        let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        // SAFETY: getrlimit and setrlimit only read and write `limit`.
        unsafe {
            if libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut limit) == 0 {
                // macOS refuses more than OPEN_MAX (10240) for the soft limit.
                limit.rlim_cur = limit.rlim_max.min(10_240);
                libc::setrlimit(libc::RLIMIT_NOFILE, &raw const limit);
            }
        }
    });
}

pub fn temp_dir() -> PathBuf {
    raise_open_files();
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!("factoryd-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

pub fn home(dir: &Path) -> Home {
    Home::new(dir.join("home"))
}

/// A Factory whose scripted provider is already signed in.
pub async fn start(dir: &Path) -> (SocketAddr, Arc<Factory>) {
    start_with(dir, Scripted::signed_in()).await
}

pub async fn start_with(dir: &Path, models: Scripted) -> (SocketAddr, Arc<Factory>) {
    raise_open_files();
    // Logs of commands with secrets go through the real `factoryd redact`.
    factoryd::set_filter_executable(env!("CARGO_BIN_EXE_factoryd").into());
    let factory = Factory::open(home(dir), Arc::new(models)).await.expect("open factory");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(factoryd::serve(listener, factory.clone()));
    (addr, factory)
}

/// A provider with one model, which answers "echo: <the prompt>" streamed in
/// two pieces and reports 1200 tokens of context. Asked for a summary, it
/// writes one. A prompt ending in "hang" never gets an answer.
///
/// Sign in through the browser by pasting a redirect address with
/// `code=good`, or with the API key "sk-good".
pub struct Scripted {
    /// How it's signed in; None when it isn't.
    signed_in_with: Mutex<Option<&'static str>>,
    /// The system prompt of the latest request.
    pub system_prompt: Arc<Mutex<String>>,
    /// The tools of the latest request, as JSON.
    pub tools: Arc<Mutex<String>>,
    /// What the latest summary was asked of: the request's text, message by message.
    pub compacted: Arc<Mutex<String>>,
    /// Every request, oldest first.
    pub requests: Arc<Mutex<Vec<Request>>>,
}

/// What a model request was sent with.
#[derive(Clone, Debug)]
pub struct Request {
    pub model: String,
    /// The effort, e.g. "high"; "off" when it doesn't reason.
    pub effort: String,
    pub system_prompt: String,
    /// The tools' names, in order.
    pub tools: Vec<String>,
}

const ACCOUNT: &str = "Scripted account";
const KEY: &str = "Scripted key";

impl Scripted {
    pub fn signed_in() -> Scripted {
        Scripted {
            signed_in_with: Mutex::new(Some(KEY)),
            system_prompt: Arc::default(),
            tools: Arc::default(),
            compacted: Arc::default(),
            requests: Arc::default(),
        }
    }

    /// Signed in with its subscription, whose weekly limit is 40% used.
    pub fn subscribed() -> Scripted {
        Scripted {
            signed_in_with: Mutex::new(Some(ACCOUNT)),
            system_prompt: Arc::default(),
            tools: Arc::default(),
            compacted: Arc::default(),
            requests: Arc::default(),
        }
    }

    pub fn signed_out() -> Scripted {
        Scripted {
            signed_in_with: Mutex::new(None),
            system_prompt: Arc::default(),
            tools: Arc::default(),
            compacted: Arc::default(),
            requests: Arc::default(),
        }
    }

    fn current(&self) -> Option<&'static str> {
        *self.signed_in_with.lock().expect("lock")
    }
}

impl Models for Scripted {
    fn list(&self) -> BoxFuture<'_, Vec<v1::Model>> {
        let models = match self.current() {
            Some(_) => vec![
                v1::Model { id: MODEL.into(), name: "Echo".into(), context_window: 10_000, efforts: vec!["off".into()] },
                v1::Model { id: OTHER_MODEL.into(), name: "Parrot".into(), context_window: 10_000, efforts: vec!["off".into()] },
            ],
            None => Vec::new(),
        };
        Box::pin(async move { models })
    }

    fn providers(&self) -> BoxFuture<'_, Vec<v1::Provider>> {
        let current = self.current();
        Box::pin(async move {
            vec![v1::Provider {
                id: "scripted".into(),
                name: "Scripted".into(),
                signed_in_with: current.unwrap_or_default().into(),
                can_sign_out: current.is_some(),
                methods: vec![
                    v1::LoginMethod { id: OAUTH.into(), label: ACCOUNT.into() },
                    v1::LoginMethod { id: API_KEY.into(), label: KEY.into() },
                ],
                usage: None,
                plan: String::new(),
                accounts: Vec::new(),
            }]
        })
    }

    fn login<'a>(
        &'a self,
        _provider: &'a str,
        method: &'a str,
        interaction: &'a dyn AuthInteraction,
        _add: bool,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let signed_in_with = if method == OAUTH {
                interaction
                    .notify(AuthEvent::AuthUrl { url: "https://scripted.test/authorize".into(), instructions: Some("Approve".into()) });
                let pasted = interaction.prompt(AuthPrompt::ManualCode { message: "Paste the redirect".into(), placeholder: None }).await?;
                if !pasted.contains("code=good") {
                    return Err(Error::Other("the authorization code was rejected".into()));
                }
                ACCOUNT
            } else {
                let key = interaction.prompt(AuthPrompt::Secret { message: "Enter Scripted key".into(), placeholder: None }).await?;
                if key != "sk-good" {
                    return Err(Error::Other("invalid API key".into()));
                }
                KEY
            };
            *self.signed_in_with.lock().expect("lock") = Some(signed_in_with);
            Ok(())
        })
    }

    fn logout<'a>(&'a self, _provider: &'a str, _account: &'a str) -> BoxFuture<'a, Result<(), Error>> {
        *self.signed_in_with.lock().expect("lock") = None;
        Box::pin(async { Ok(()) })
    }

    fn usage<'a>(&'a self, _provider: &'a str, _account: &'a str) -> BoxFuture<'a, Result<Option<Vec<v1::UsageWindow>>, Error>> {
        let subscribed = self.current() == Some(ACCOUNT);
        Box::pin(async move {
            Ok(subscribed.then(|| {
                vec![
                    v1::UsageWindow { id: "weekly".into(), label: "Weekly".into(), used_percent: 40.0, resets_at: 1_795_554_000_000 },
                    v1::UsageWindow { id: "five_hour".into(), label: "5-hour".into(), used_percent: 2.5, resets_at: 0 },
                ]
            }))
        })
    }

    fn resolve(&self, id: &str) -> Option<Model> {
        (id == MODEL || id == OTHER_MODEL).then(|| Model {
            id: if id == OTHER_MODEL { "parrot" } else { "echo" }.into(),
            name: if id == OTHER_MODEL { "Parrot" } else { "Echo" }.into(),
            api: "scripted".into(),
            provider: "scripted".into(),
            base_url: String::new(),
            reasoning: false,
            input: vec!["text".into()],
            cost: ModelCost::default(),
            context_window: 10_000,
            max_tokens: 1_000,
            thinking_level_map: None,
            compat: None,
            headers: None,
            extra: Default::default(),
        })
    }

    fn stream(&self) -> StreamFn {
        let system_prompt = self.system_prompt.clone();
        let tools = self.tools.clone();
        let compacted = self.compacted.clone();
        let requests = self.requests.clone();
        Arc::new(move |model, context, options| {
            requests.lock().expect("lock").push(Request {
                model: model.id.clone(),
                effort: options.reasoning.map_or("off".to_owned(), |level| format!("{level:?}").to_lowercase()),
                system_prompt: context.system_prompt.clone().unwrap_or_default(),
                tools: context.tools.iter().map(|tool| tool.name.to_string()).collect(),
            });
            *system_prompt.lock().expect("lock") = context.system_prompt.clone().unwrap_or_default();
            *tools.lock().expect("lock") = serde_json::to_string(&context.tools).expect("tools");
            let prompt = match context.messages.last() {
                Some(Message::User(user)) => text_of(&user.content),
                Some(Message::ToolResult(result)) => {
                    let mut text = String::from("done: ");
                    for part in &result.content {
                        if let factory_agent::ToolResultPart::Text { text: part } = part {
                            text.push_str(part);
                        }
                    }
                    text
                }
                _ => String::new(),
            };
            // Asked to compact, it summarizes like a real model would.
            let reply = if prompt.contains("<summary>") {
                let texts: Vec<String> = context
                    .messages
                    .iter()
                    .map(|message| match message {
                        Message::User(user) => text_of(&user.content),
                        _ => String::new(),
                    })
                    .collect();
                *compacted.lock().expect("lock") = texts.join("\n");
                format!("<summary>{} earlier messages.</summary>", context.messages.len() - 1)
            } else if prompt.starts_with("done: ") {
                prompt.clone()
            } else {
                format!("echo: {prompt}")
            };
            let (first, second) = reply.split_at(reply.len() / 2);
            let mut partial = AssistantMessage {
                content: Vec::new(),
                api: model.api.clone(),
                provider: model.provider.clone(),
                model: model.id.clone(),
                usage: Usage { input: 1_000, output: 200, ..Usage::default() },
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 1,
                extra: Default::default(),
            };
            let (sink, stream) = event_stream();
            sink.push(StreamEvent::Start { partial: partial.clone() });
            // "slowcall <tool> <arguments>" calls it too, writing the
            // arguments in two pieces a moment apart, as a model streams them.
            if let Some(call) = prompt.strip_prefix("slowcall ")
                && let Some((name, raw)) = call.split_once(' ')
            {
                let (name, raw) = (name.to_owned(), raw.to_owned());
                tokio::spawn(async move {
                    let arguments = serde_json::from_str(&raw).expect("tool arguments");
                    let part =
                        |arguments| AssistantPart::ToolCall { id: "call-1".into(), name: name.clone(), arguments, thought_signature: None };
                    partial.content.push(part(serde_json::json!({})));
                    sink.push(StreamEvent::ToolcallStart { content_index: 0, partial: partial.clone() });
                    let (first, second) = raw.split_at(raw.len() / 2);
                    sink.push(StreamEvent::ToolcallDelta { content_index: 0, delta: first.into(), partial: partial.clone() });
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    sink.push(StreamEvent::ToolcallDelta { content_index: 0, delta: second.into(), partial: partial.clone() });
                    partial.content[0] = part(arguments);
                    sink.push(StreamEvent::ToolcallEnd {
                        content_index: 0,
                        tool_call: partial.content[0].clone(),
                        partial: partial.clone(),
                    });
                    partial.stop_reason = StopReason::ToolUse;
                    sink.push(StreamEvent::Done { reason: StopReason::ToolUse, message: partial });
                });
                return Ok(stream);
            }
            // "call <tool> <arguments>" calls the tool.
            if let Some(call) = prompt.strip_prefix("call ")
                && let Some((name, arguments)) = call.split_once(' ')
            {
                let arguments = serde_json::from_str(arguments).expect("tool arguments");
                partial.content.push(AssistantPart::ToolCall {
                    id: "call-1".into(),
                    name: name.into(),
                    arguments,
                    thought_signature: None,
                });
                partial.stop_reason = StopReason::ToolUse;
                sink.push(StreamEvent::Done { reason: StopReason::ToolUse, message: partial });
                return Ok(stream);
            }
            // "fail" fails for good, as a model that won't answer does.
            if prompt.ends_with("fail") {
                partial.stop_reason = StopReason::Error;
                partial.error_message = Some("scripted failure".into());
                sink.push(StreamEvent::Error { reason: StopReason::Error, error: partial });
                return Ok(stream);
            }
            // "hang" never finishes, like a run a restart cuts off.
            if prompt.ends_with("hang") {
                tokio::spawn(async move {
                    let _open = sink;
                    std::future::pending::<()>().await
                });
                return Ok(stream);
            }
            partial.content.push(AssistantPart::Text { text: String::new(), text_signature: None });
            sink.push(StreamEvent::TextStart { content_index: 0, partial: partial.clone() });
            for piece in [first, second] {
                if let Some(AssistantPart::Text { text, .. }) = partial.content.first_mut() {
                    text.push_str(piece);
                }
                sink.push(StreamEvent::TextDelta { content_index: 0, delta: piece.into(), partial: partial.clone() });
            }
            sink.push(StreamEvent::TextEnd { content_index: 0, content: reply.clone(), partial: partial.clone() });
            sink.push(StreamEvent::Done { reason: StopReason::Stop, message: partial });
            Ok(stream)
        })
    }
}

fn text_of(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Parts(parts) => {
            let mut text = String::new();
            for part in parts {
                if let UserPart::Text { text: part } = part {
                    text.push_str(part);
                }
            }
            text
        }
    }
}

pub struct Client {
    socket: WebSocketStream<TcpStream>,
    next_id: u64,
}

pub struct Reply {
    /// Events the server sent before answering.
    pub events: Vec<ServerMessage>,
    pub body: Body,
}

impl Client {
    pub async fn open(addr: SocketAddr) -> Client {
        let stream = TcpStream::connect(addr).await.expect("connect");
        let (socket, _) = client_async(format!("ws://{addr}"), stream).await.expect("websocket handshake");
        Client { socket, next_id: 1 }
    }

    /// Connects and completes the handshake. Returns the initial state.
    pub async fn connect(addr: SocketAddr) -> (Client, v1::Workspace, Option<v1::Transcript>) {
        let mut client = Client::open(addr).await;
        let hello = Hello { protocol_version: PROTOCOL_VERSION, client_version: "test".into(), token: String::new() };
        client.send(ClientMessage { id: 1, body: Some(client_message::Body::Hello(hello)) }).await;
        match client.receive().await.body {
            Some(Body::Welcome(_)) => {}
            other => panic!("expected welcome, got {other:?}"),
        }
        let workspace = match client.receive().await.body {
            Some(Body::Workspace(workspace)) => workspace,
            other => panic!("expected workspace, got {other:?}"),
        };
        let transcript = if workspace.selected_agent_id.is_empty() {
            None
        } else {
            match client.receive().await.body {
                Some(Body::Transcript(transcript)) => Some(transcript),
                other => panic!("expected transcript, got {other:?}"),
            }
        };
        client.next_id = 2;
        (client, workspace, transcript)
    }

    pub async fn send(&mut self, message: ClientMessage) {
        self.send_bytes(message.encode_to_vec()).await;
    }

    pub async fn send_bytes(&mut self, bytes: Vec<u8>) {
        self.socket.send(Frame::Binary(bytes.into())).await.expect("send");
    }

    pub async fn receive(&mut self) -> ServerMessage {
        let next = async {
            loop {
                match self.socket.next().await.expect("server closed the connection").expect("read") {
                    Frame::Binary(payload) => return ServerMessage::decode(payload).expect("decode"),
                    _ => continue,
                }
            }
        };
        tokio::time::timeout(WAIT, next).await.expect("timed out waiting for the server")
    }

    /// Sends a request and collects everything up to its answer.
    pub async fn request(&mut self, body: client_message::Body) -> Reply {
        let id = self.next_id;
        self.next_id += 1;
        self.send(ClientMessage { id, body: Some(body) }).await;
        let mut events = Vec::new();
        loop {
            let message = self.receive().await;
            if message.reply_to == id {
                return Reply { events, body: message.body.expect("reply body") };
            }
            assert_eq!(message.reply_to, 0, "reply to another request");
            events.push(message);
        }
    }

    /// Sends a request that must succeed.
    pub async fn ok(&mut self, body: client_message::Body) -> Vec<ServerMessage> {
        let reply = self.request(body).await;
        match reply.body {
            Body::Ack(_) => reply.events,
            other => panic!("expected ack, got {other:?}"),
        }
    }

    /// Reads events until `pick` finds what it's looking for.
    pub async fn wait_for<T>(&mut self, mut pick: impl FnMut(&Body) -> Option<T>) -> T {
        loop {
            let message = self.receive().await;
            if let Some(body) = &message.body
                && let Some(found) = pick(body)
            {
                return found;
            }
        }
    }

    pub async fn closed(&mut self) -> bool {
        let next = tokio::time::timeout(WAIT, self.socket.next()).await.expect("timed out waiting for close");
        matches!(next, None | Some(Err(_)) | Some(Ok(Frame::Close(_))))
    }
}

/// The last workspace among `events`.
pub fn last_workspace(events: &[ServerMessage]) -> Option<&v1::Workspace> {
    let mut found = None;
    for event in events {
        if let Some(Body::Workspace(workspace)) = &event.body {
            found = Some(workspace);
        }
    }
    found
}

pub fn create_project(name: &str, path: &Path) -> client_message::Body {
    client_message::Body::CreateProject(v1::CreateProject { name: name.into(), path: path.display().to_string() })
}

pub fn create_agent(project_id: &str, name: &str, prompt: &str) -> client_message::Body {
    client_message::Body::CreateAgent(v1::CreateAgent {
        project_id: project_id.into(),
        name: name.into(),
        prompt: prompt.into(),
        model: MODEL.into(),
        effort: "off".into(),
        color: String::new(),
        folder_id: String::new(),
        role: String::new(),
    })
}

pub fn send_message(agent_id: &str, text: &str) -> client_message::Body {
    client_message::Body::SendMessage(v1::SendMessage { agent_id: agent_id.into(), text: text.into() })
}

/// A transcript's entries as short strings, for comparing whole conversations.
pub fn describe(entries: &[v1::Entry]) -> Vec<String> {
    use v1::entry::Kind;
    let mut out = Vec::new();
    for entry in entries {
        out.push(match &entry.kind {
            Some(Kind::User(user)) => format!("user: {}", user.text),
            Some(Kind::Assistant(assistant)) => format!("assistant: {}", assistant.text),
            Some(Kind::Thinking(thinking)) => format!("thinking: {}", thinking.text),
            Some(Kind::ToolCall(call)) => format!("tool {}: {}", call.name, call.output),
            Some(Kind::Compaction(compaction)) => format!("compaction: {}", compaction.summary),
            Some(Kind::Notice(notice)) => format!("notice: {}", notice.text),
            Some(Kind::Event(event)) if event.summary.is_empty() => format!("event from {}: {}", event.from, event.text),
            Some(Kind::Event(event)) => format!("event from {}: {}", event.from, event.summary),
            Some(Kind::Proposal(proposal)) => format!("proposal {} ({:?}): {}", proposal.id, proposal.state(), proposal.text),
            Some(Kind::SecretRequest(request)) => format!("secret request {} ({:?}): {}", request.id, request.state(), request.reason),
            None => "empty".into(),
        });
    }
    out
}

/// What a client shows: the workspace, the selected agent's transcript and
/// the focused project's explorer, kept up to date from events the way the
/// app does it.
#[derive(Default)]
pub struct View {
    pub workspace: v1::Workspace,
    pub entries: Vec<v1::Entry>,
    pub deltas: usize,
    pub explorer: Option<v1::Explorer>,
    /// The open extension panel's content, as last sent.
    pub extension_view: Option<v1::ExtensionView>,
    /// What waits for the selected agent, as last sent.
    pub queued: Option<v1::QueuedMessages>,
    /// Every highlight sent, in order.
    pub highlights: Vec<v1::Highlight>,
}

impl View {
    /// The explorer's rows as indented names: "src/", "  main.rs", and
    /// "(hidden)" after rows .factoryignore hides.
    pub fn tree(&self) -> Vec<String> {
        let mut rows = Vec::new();
        for entry in self.explorer.as_ref().map(|explorer| explorer.entries.as_slice()).unwrap_or_default() {
            let indent = "  ".repeat(entry.depth as usize);
            let slash = if entry.is_dir { "/" } else { "" };
            let hidden = if entry.hidden { " (hidden)" } else { "" };
            rows.push(format!("{indent}{}{slash}{hidden}", entry.name));
        }
        rows
    }
}

impl View {
    pub fn apply(&mut self, body: &Body) {
        match body {
            Body::Workspace(workspace) => self.workspace = workspace.clone(),
            Body::Explorer(explorer) => self.explorer = Some(explorer.clone()),
            Body::ExtensionView(view) => self.extension_view = Some(view.clone()),
            Body::QueuedMessages(queued) => self.queued = Some(queued.clone()),
            Body::Highlight(highlight) => self.highlights.push(highlight.clone()),
            Body::Transcript(transcript) => self.entries = transcript.entries.clone(),
            Body::Entry(update) => {
                let Some(entry) = update.entry.clone() else { return };
                let seq = entry.seq as usize;
                if seq < self.entries.len() {
                    self.entries[seq] = entry;
                } else {
                    assert_eq!(seq, self.entries.len(), "entries arrive in order");
                    self.entries.push(entry);
                }
            }
            Body::Delta(delta) => {
                use v1::entry::Kind;
                self.deltas += 1;
                match self.entries[delta.seq as usize].kind.as_mut() {
                    Some(Kind::Assistant(v1::AssistantText { text })) | Some(Kind::Thinking(v1::Thinking { text })) => {
                        text.push_str(&delta.text)
                    }
                    other => panic!("delta for {other:?}"),
                }
            }
            _ => {}
        }
    }

    pub fn agent(&self, id: &str) -> &v1::Agent {
        self.workspace.agents.iter().find(|agent| agent.id == id).expect("agent in workspace")
    }

    /// Applies events until `done` holds.
    pub async fn follow(&mut self, client: &mut Client, mut done: impl FnMut(&View) -> bool) {
        while !done(self) {
            let message = client.receive().await;
            if let Some(body) = &message.body {
                self.apply(body);
            }
        }
    }
}

/// Applies a request's pre-ack events to `view` and returns the reply.
pub async fn apply_request(client: &mut Client, view: &mut View, body: client_message::Body) -> Body {
    let reply = client.request(body).await;
    for event in &reply.events {
        if let Some(body) = &event.body {
            view.apply(body);
        }
    }
    reply.body
}
