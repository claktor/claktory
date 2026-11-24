//! What goes over an app's connection in a typical session, measured: a
//! Claktory like a person's who uses it all day (Rivet, two projects, a
//! dozen agents with long conversations, folders with a real repository),
//! and a session on it (connect, open a few agents, one works for a while,
//! the explorer, idle, reconnect). Prints bytes and counts per message kind
//! and phase, and writes every frame to `$TRAFFIC_DUMP` (each a 4-byte
//! big-endian length, its phase's number, then the frame) for compression
//! estimates. Ignored: run it by hand,
//!
//! ```sh
//! TRAFFIC_DUMP=/tmp/traffic.bin cargo test -p factoryd --test all traffic:: -- --ignored --nocapture
//! ```

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use factory_agent::auth::AuthInteraction;
use factory_agent::stream::event_stream;
use factory_agent::types::ModelCost;
use factory_agent::{AssistantMessage, AssistantPart, Error, Message, Model, StopReason, StreamEvent, StreamFn, Usage};
use factory_protocol::PROTOCOL_VERSION;
use factory_protocol::v1::{self, ClientMessage, ServerMessage, client_message, server_message::Body};
use factoryd::models::Models;
use futures_util::future::BoxFuture;
use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message as Frame;

use crate::common::{apply_patch, home, start_with_models, temp_dir};

const MODEL: &str = "anthropic/claude-opus-5-5";
/// Tool calls in the run the session watches.
const ROUNDS: usize = 10;
/// Time between streamed pieces, and their size: a model streaming about
/// 100 tokens a second.
const DELTA_EVERY: Duration = Duration::from_millis(10);
const DELTA_CHARS: usize = 4;

/// Providers and models as a person signed in to two has them, and a model
/// that thinks, says what it does and runs a command, `ROUNDS` times.
struct Busy;

fn models() -> Vec<v1::Model> {
    let efforts = |list: &[&str]| list.iter().map(|effort| (*effort).to_owned()).collect::<Vec<_>>();
    let mut models = Vec::new();
    for name in [
        "claude-fable-5",
        "claude-haiku-4-5",
        "claude-haiku-4-5-20251001",
        "claude-opus-4-5",
        "claude-opus-4-5-20251101",
        "claude-opus-4-6",
        "claude-opus-4-7",
        "claude-opus-4-8",
        "claude-opus-5",
        "claude-opus-5-5",
        "claude-sonnet-4-5",
        "claude-sonnet-4-5-20250929",
        "claude-sonnet-4-6",
        "claude-sonnet-5",
        "claude-sonnet-5-5",
    ] {
        models.push(v1::Model {
            id: format!("anthropic/{name}"),
            name: format!("Claude {name}"),
            context_window: 200_000,
            efforts: efforts(&["off", "minimal", "low", "medium", "high", "xhigh", "max"]),
            image_input: false,
            added: false,
        });
    }
    for name in ["gpt-5.3-codex-spark", "gpt-5.4", "gpt-5.4-mini", "gpt-5.5", "gpt-5.6-luna", "gpt-5.6-sol", "gpt-5.6-terra", "gpt-6.1-sol"]
    {
        models.push(v1::Model {
            id: format!("openai-codex/{name}"),
            name: format!("GPT {name}"),
            context_window: 400_000,
            efforts: efforts(&["off", "minimal", "low", "medium", "high", "xhigh"]),
            image_input: false,
            added: false,
        });
    }
    models
}

fn provider(id: &str, name: &str, accounts: usize) -> v1::Provider {
    let usage = || {
        Some(v1::ProviderUsage {
            windows: vec![
                v1::UsageWindow { id: "weekly".into(), label: "Weekly".into(), used_percent: 41.0, resets_at: 1_795_554_000_000 },
                v1::UsageWindow { id: "five_hour".into(), label: "5-hour".into(), used_percent: 12.5, resets_at: 1_795_154_000_000 },
            ],
            ..Default::default()
        })
    };
    v1::Provider {
        id: id.into(),
        name: name.into(),
        signed_in_with: "Subscription".into(),
        can_sign_out: true,
        methods: vec![
            v1::LoginMethod { id: "oauth".into(), label: format!("{name} subscription") },
            v1::LoginMethod { id: "api_key".into(), label: "API key".into() },
        ],
        usage: usage(),
        plan: "Max".into(),
        accounts: (0..accounts)
            .map(|n| v1::Account { id: format!("account-{n}"), label: format!("Account {n}"), usage: usage(), ..Default::default() })
            .collect(),
        cache_ttl: String::new(),
        ..Default::default()
    }
}

fn words(chars: usize, seed: &str) -> String {
    let base = format!("{seed} reads the code around the change, checks what calls it and what it returns, then ");
    base.chars().cycle().take(chars).collect()
}

impl Models for Busy {
    fn list(&self) -> BoxFuture<'_, Vec<v1::Model>> {
        Box::pin(async { models() })
    }

    fn providers(&self) -> BoxFuture<'_, Vec<v1::Provider>> {
        Box::pin(async {
            vec![
                provider("anthropic", "Anthropic", 2),
                provider("openai-codex", "ChatGPT", 1),
                v1::Provider { id: "openrouter".into(), name: "OpenRouter".into(), ..Default::default() },
            ]
        })
    }

    fn login<'a>(&'a self, _: &'a str, _: &'a str, _: &'a dyn AuthInteraction, _: bool) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async { Ok(()) })
    }

    fn logout<'a>(&'a self, _: &'a str, _: &'a str) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async { Ok(()) })
    }

    fn usage<'a>(&'a self, _: &'a str, _: &'a str) -> BoxFuture<'a, Result<Option<v1::ProviderUsage>, Error>> {
        Box::pin(async { Ok(None) })
    }

    fn resolve(&self, id: &str) -> Option<Model> {
        let (provider, name) = id.split_once('/')?;
        Some(Model {
            id: name.into(),
            name: name.into(),
            api: "busy".into(),
            provider: provider.into(),
            base_url: String::new(),
            reasoning: true,
            input: vec!["text".into()],
            cost: ModelCost::default(),
            context_window: 200_000,
            max_tokens: 32_000,
            thinking_level_map: None,
            compat: None,
            headers: None,
            extra: Default::default(),
        })
    }

    fn stream(&self) -> StreamFn {
        Arc::new(move |model, context, _options| {
            let done = context.messages.iter().rev().take_while(|message| !matches!(message, Message::User(_))).count() / 2;
            let tokens = 40_000 + 3_000 * context.messages.len() as u64;
            let mut partial = AssistantMessage {
                content: Vec::new(),
                api: model.api.clone(),
                provider: model.provider.clone(),
                model: model.id.clone(),
                usage: Usage { input: 4, output: 300, cache_read: tokens, ..Usage::default() },
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: factoryd::transcript::now_ms() as u64,
                extra: Default::default(),
            };
            let (sink, stream) = event_stream();
            tokio::spawn(async move {
                sink.push(StreamEvent::Start { partial: partial.clone() });
                let thinking = words(320, "It");
                partial.content.push(AssistantPart::Thinking { thinking: String::new(), thinking_signature: None, redacted: None });
                sink.push(StreamEvent::ThinkingStart { content_index: 0, partial: partial.clone() });
                for piece in thinking.as_bytes().chunks(DELTA_CHARS) {
                    let piece = String::from_utf8_lossy(piece).into_owned();
                    if let Some(AssistantPart::Thinking { thinking, .. }) = partial.content.last_mut() {
                        thinking.push_str(&piece);
                    }
                    sink.push(StreamEvent::ThinkingDelta { content_index: 0, delta: piece, partial: partial.clone() });
                    tokio::time::sleep(DELTA_EVERY).await;
                }
                sink.push(StreamEvent::ThinkingEnd { content_index: 0, content: thinking, partial: partial.clone() });
                let text = if done < ROUNDS { words(160, "Next, it") } else { words(700, "Done: it") };
                partial.content.push(AssistantPart::Text { text: String::new(), text_signature: None });
                sink.push(StreamEvent::TextStart { content_index: 1, partial: partial.clone() });
                for piece in text.as_bytes().chunks(DELTA_CHARS) {
                    let piece = String::from_utf8_lossy(piece).into_owned();
                    if let Some(AssistantPart::Text { text, .. }) = partial.content.last_mut() {
                        text.push_str(&piece);
                    }
                    sink.push(StreamEvent::TextDelta { content_index: 1, delta: piece, partial: partial.clone() });
                    tokio::time::sleep(DELTA_EVERY).await;
                }
                sink.push(StreamEvent::TextEnd { content_index: 1, content: text, partial: partial.clone() });
                if done < ROUNDS {
                    let call = AssistantPart::ToolCall {
                        id: format!("call-{done}"),
                        name: "bash".into(),
                        arguments: json!({"command": "seq 1 400 | sed 's/^/line /'", "timeout": null, "run_in_background": null}),
                        thought_signature: None,
                    };
                    partial.content.push(call.clone());
                    sink.push(StreamEvent::ToolcallStart { content_index: 2, partial: partial.clone() });
                    sink.push(StreamEvent::ToolcallEnd { content_index: 2, tool_call: call, partial: partial.clone() });
                    partial.stop_reason = StopReason::ToolUse;
                    sink.push(StreamEvent::Done { reason: StopReason::ToolUse, message: partial });
                } else {
                    sink.push(StreamEvent::Done { reason: StopReason::Stop, message: partial });
                }
            });
            Ok(stream)
        })
    }
}

/// A conversation of `blocks` tasks, each a request and `ROUNDS` tool calls
/// with their output: about 30 KB a block, as a working agent's.
fn conversation(blocks: usize) -> Vec<Value> {
    let mut messages = Vec::new();
    let mut at = 1_795_000_000_000_u64;
    for block in 0..blocks {
        at += 60_000;
        messages.push(
            json!({"role": "user", "content": [{"type": "text", "text": words(600, &format!("Task {block}: please"))}], "timestamp": at}),
        );
        for round in 0..ROUNDS {
            at += 5_000;
            let id = format!("toolu_{block}_{round}");
            messages.push(json!({
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": words(400, "It"), "thinkingSignature": "Q".repeat(300)},
                    {"type": "text", "text": words(160, "Next, it")},
                    {"type": "toolCall", "id": id, "name": "bash", "arguments": {"command": "rg -n 'fn ' crates/factoryd/src | sed -n 1,60p", "timeout": null, "run_in_background": null}}
                ],
                "api": "anthropic-messages", "provider": "anthropic", "model": "claude-opus-5-5",
                "usage": {"input": 4, "output": 300, "cacheRead": 90000, "cacheWrite": 900, "cacheWrite1h": 0, "reasoning": 0, "totalTokens": 91204,
                          "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}},
                "stopReason": "toolUse", "timestamp": at
            }));
            messages.push(json!({
                "role": "toolResult", "toolCallId": id, "toolName": "bash",
                "content": [{"type": "text", "text": (0..60).map(|n| format!("crates/factoryd/src/factory.rs:{n}:    pub fn something_{n}(&self) -> Result<(), Error> {{\n")).collect::<String>()}],
                "details": {"exit": 0, "ms": 42}, "isError": false, "timestamp": at + 40, "started": at
            }));
        }
        messages.push(json!({
            "role": "assistant", "content": [{"type": "text", "text": words(900, "Done: it")}],
            "api": "anthropic-messages", "provider": "anthropic", "model": "claude-opus-5-5",
            "usage": {"input": 4, "output": 300, "cacheRead": 90000, "cacheWrite": 900, "cacheWrite1h": 0, "reasoning": 0, "totalTokens": 91204,
                      "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}},
            "stopReason": "stop", "timestamp": at + 100
        }));
    }
    messages
}

/// A Claktory like a person's: Rivet, "Claktory" with App (a real
/// repository) and Research, a dozen agents with roles, long prompts and
/// conversations, and "Site" with two more.
fn set_up(dir: &Path) {
    let repo = dir.join("app");
    std::fs::create_dir_all(&repo).expect("app folder");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let archive = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("git -C '{}' archive HEAD | tar -x -C '{}'", root.display(), repo.display()))
        .status()
        .expect("git archive");
    assert!(archive.success());
    std::fs::create_dir_all(dir.join("research")).expect("research");
    std::fs::create_dir_all(dir.join("home/workspace/site")).expect("site");
    let agent = |id: &str, name: &str, role: &str, project: Option<&str>, folder: &str| {
        let mut agent = json!({
            "id": id, "name": name, "role": role, "prompt": words(1_800, name), "model": MODEL, "effort": "high",
            "extensions": {"checklist": {"enabled": true}, "memory": {"enabled": true}},
        });
        if let Some(project) = project {
            agent["project"] = json!(project);
        }
        if !folder.is_empty() {
            agent["folder"] = json!(folder);
        }
        agent
    };
    let mut agents = vec![json!({
        "id": "rivet", "name": "Rivet", "role": "Admin", "model": MODEL, "effort": "high",
        "extensions": {"agents": {"reachable": false, "admin": true}, "highlight": {"enabled": true}},
    })];
    let team = [
        ("mike", "Mike", "Project Manager", ""),
        ("sam", "Sam", "Backend", "app"),
        ("lena", "Lena", "Backend", "app"),
        ("omar", "Omar", "Backend", "app"),
        ("priya", "Priya", "Frontend", "app"),
        ("kenji", "Kenji", "Frontend", "app"),
        ("zoe", "Zoe", "Frontend", "app"),
        ("hana", "Hana", "UX Writer", "app"),
        ("arjun", "Arjun", "DevOps", "app"),
        ("felix", "Felix", "Refactoring", "app"),
        ("nora", "Nora", "QA", "app"),
        ("iris", "Iris", "Researcher", "research"),
        ("theo", "Theo", "Researcher", "research"),
    ];
    for (id, name, role, folder) in team {
        agents.push(agent(id, name, role, Some("factory"), folder));
    }
    agents.push(agent("ana", "Ana", "Frontend", Some("site"), ""));
    agents.push(agent("ben", "Ben", "Writer", Some("site"), ""));
    let config = json!({
        "default_model": MODEL, "default_effort": "high",
        "user": {"name": "Matt"},
        "projects": [
            {"id": "factory", "name": "Claktory", "path": dir.join("home/workspace/factory"),
             "folders": [{"id": "app", "name": "App", "path": repo}, {"id": "research", "name": "Research", "path": dir.join("research")}]},
            {"id": "site", "name": "Site", "path": dir.join("home/workspace/site")},
        ],
        "agents": agents,
    });
    std::fs::create_dir_all(dir.join("home/workspace/factory")).expect("factory");
    std::fs::write(dir.join("home/config.json"), serde_json::to_string_pretty(&config).expect("json")).expect("config");
    // Long conversations: Sam's has been compacted once.
    for (id, blocks) in [("mike", 30), ("sam", 40), ("lena", 25), ("zoe", 25), ("rivet", 10), ("hana", 8)] {
        let folder = dir.join("home/agents").join(id);
        std::fs::create_dir_all(&folder).expect("agent folder");
        let mut segment = json!({"messages": conversation(blocks)});
        if id == "sam" {
            std::fs::write(folder.join("segment-0001.json"), serde_json::to_string(&segment).expect("json")).expect("segment");
            segment = json!({
                "compaction": {"summary": words(12_000, "Summary"), "tokens_before": 400_000},
                "messages": conversation(10),
            });
            std::fs::write(folder.join("segment-0002.json"), serde_json::to_string(&segment).expect("json")).expect("segment");
        } else {
            std::fs::write(folder.join("segment-0001.json"), serde_json::to_string(&segment).expect("json")).expect("segment");
        }
    }
}

/// One frame received: its phase, kind and size, and its size compressed as
/// the tunnel's `claktory/2` would (one zstd stream, flushed per message).
struct Received {
    phase: usize,
    kind: String,
    bytes: usize,
    compressed: usize,
}

#[derive(Default)]
struct Meter {
    phase: usize,
    received: Vec<Received>,
    dump: Vec<u8>,
    /// Each agent's status in the latest Workspace.
    status: BTreeMap<String, (i32, u32)>,
    /// The latest Workspace, patches applied.
    workspace: v1::Workspace,
}

/// Whether the session's app applies WorkspacePatch (TRAFFIC_PATCHES=1).
fn patches() -> bool {
    std::env::var("TRAFFIC_PATCHES").is_ok_and(|value| value == "1")
}

fn kind(body: Option<&Body>) -> String {
    let Some(body) = body else { return "none".into() };
    let debug = format!("{body:?}");
    debug.split('(').next().unwrap_or_default().to_owned()
}

type Socket = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

async fn connect(addr: std::net::SocketAddr) -> Socket {
    let tcp = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let (mut socket, _) = tokio_tungstenite::client_async("ws://factory/", tcp).await.expect("websocket");
    let hello = v1::Hello {
        protocol_version: PROTOCOL_VERSION,
        client_version: "traffic".into(),
        token: crate::common::local_token(addr),
        device_kind: 0,
        workspace_patches: patches(),
        ..Default::default()
    };
    let message = ClientMessage { id: 1, body: Some(client_message::Body::Hello(hello)) };
    socket.send(Frame::Binary(message.encode_to_vec().into())).await.expect("hello");
    socket
}

/// Reads everything that comes, into the meter.
fn read(mut socket: futures_util::stream::SplitStream<Socket>, meter: Arc<Mutex<Meter>>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt as _;
        let mut zstd = async_compression::tokio::write::ZstdEncoder::with_quality(Vec::new(), async_compression::Level::Default);
        while let Some(Ok(frame)) = socket.next().await {
            let Frame::Binary(bytes) = frame else { continue };
            zstd.write_all(&bytes).await.expect("compress");
            zstd.flush().await.expect("flush");
            let compressed = zstd.get_ref().len();
            zstd.get_mut().clear();
            let message = ServerMessage::decode(bytes.clone()).expect("decode");
            let mut meter = meter.lock().expect("lock");
            match &message.body {
                Some(Body::Workspace(workspace)) => meter.workspace = workspace.clone(),
                Some(Body::WorkspacePatch(patch)) => meter.workspace = apply_patch(&meter.workspace, patch),
                _ => {}
            }
            meter.status = meter.workspace.agents.iter().map(|agent| (agent.id.clone(), (agent.status, agent.queued_messages))).collect();
            let phase = meter.phase;
            meter.received.push(Received { phase, kind: kind(message.body.as_ref()), bytes: bytes.len(), compressed });
            meter.dump.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            meter.dump.push(phase as u8);
            meter.dump.extend_from_slice(&bytes);
        }
    })
}

async fn request(sink: &mut futures_util::stream::SplitSink<Socket, Frame>, id: u64, body: client_message::Body) {
    let message = ClientMessage { id, body: Some(body) };
    sink.send(Frame::Binary(message.encode_to_vec().into())).await.expect("send");
}

/// The agents that work in the session.
const WORKERS: [&str; 4] = ["zoe", "lena", "omar", "kenji"];

const PHASES: [&str; 7] = ["connect", "open 3 agents", "4 agents work", "explorer", "idle 20 s", "reconnect", "reconnect: open agent"];

#[tokio::test]
#[ignore]
async fn measure_a_session() {
    let dir = temp_dir();
    set_up(&dir);
    let (addr, factory) = start_with_models(&dir, Arc::new(Busy)).await;
    let _ = home(&dir);
    let meter = Arc::new(Mutex::new(Meter::default()));
    let (mut sink, stream) = connect(addr).await.split();
    let reader = read(stream, meter.clone());
    let phase = |n: usize| meter.lock().expect("lock").phase = n;
    tokio::time::sleep(Duration::from_secs(2)).await;

    phase(1);
    for (n, agent) in ["mike", "sam", "zoe"].into_iter().enumerate() {
        request(&mut sink, 10 + n as u64, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    phase(2);
    let started = Instant::now();
    // Four agents work at once, as a team does.
    for (n, agent) in WORKERS.into_iter().enumerate() {
        let message =
            v1::SendMessage { agent_id: agent.into(), text: "Please look into the explorer's speed.".into(), ..Default::default() };
        request(&mut sink, 20 + n as u64, client_message::Body::SendMessage(message)).await;
    }
    loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let idle = {
            let meter = meter.lock().expect("lock");
            WORKERS.iter().all(|agent| meter.status.get(*agent).copied() == Some((v1::AgentStatus::Idle as i32, 0)))
        };
        if started.elapsed() > Duration::from_secs(2) && idle {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(300), "the run never ended");
    }
    let worked = started.elapsed();

    phase(3);
    request(&mut sink, 30, client_message::Body::FocusProject(v1::FocusProject { project_id: "factory".into() })).await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    phase(4);
    tokio::time::sleep(Duration::from_secs(20)).await;

    phase(5);
    drop(sink);
    reader.abort();
    let (mut sink, stream) = connect(addr).await.split();
    let reader = read(stream, meter.clone());
    tokio::time::sleep(Duration::from_secs(2)).await;
    phase(6);
    request(&mut sink, 40, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "sam".into() })).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    reader.abort();
    drop(factory);

    let meter = meter.lock().expect("lock");
    println!("\nThe run took {worked:.1?}.");
    for (number, name) in PHASES.iter().enumerate() {
        let mut kinds: BTreeMap<&str, (usize, usize, usize)> = BTreeMap::new();
        for received in meter.received.iter().filter(|received| received.phase == number) {
            let entry = kinds.entry(&received.kind).or_default();
            entry.0 += 1;
            entry.1 += received.bytes;
            entry.2 = entry.2.max(received.bytes);
        }
        let total: usize = kinds.values().map(|(_, bytes, _)| bytes).sum();
        let count: usize = kinds.values().map(|(count, ..)| count).sum();
        let compressed: usize = meter.received.iter().filter(|received| received.phase == number).map(|received| received.compressed).sum();
        println!("\n{name}: {count} messages, {total} bytes ({compressed} compressed)");
        let mut sorted: Vec<_> = kinds.into_iter().collect();
        sorted.sort_by_key(|(_, (_, bytes, _))| std::cmp::Reverse(*bytes));
        for (kind, (count, bytes, max)) in sorted {
            println!("  {kind:<22} {count:>6} × avg {:>8}  = {bytes:>10}  (max {max})", bytes / count.max(1));
        }
    }
    if let Some(path) = std::env::var_os("TRAFFIC_DUMP") {
        std::fs::write(path, &meter.dump).expect("dump");
    }
}
