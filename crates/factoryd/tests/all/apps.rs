//! Apps (slice 1): an app installed from a .clapp after the person reviews
//! what it can reach, set up with a secret kept in the vault for the app
//! alone, run by the apps host (here a stand-in speaking its protocol),
//! offering its tools to the agents given its scopes and its events to those
//! subscribed, restarted when it crashes, and uninstalled.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signer, SigningKey};
use factory_protocol::apps::{self as wire, Frame, frame::Body as Host};
use factory_protocol::v1::{self, client_message::Body as Request, entry::Kind, server_message::Body};
use factoryd::apps::AppsLink;
use prost::Message;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::common::{Client, Scripted, View, apply_request, create_agent, create_project, describe, send_message, start_with, temp_dir};

const MANIFEST: &str = r#"
id = "telegram"
name = "Telegram"
version = "0.1.0"
world = "claktory:app@1"
publisher = "claktory"
description = "Message your agents from Telegram."
icon = "send"
hosts = ["api.telegram.org"]
instructions = "Answer the person's messages with telegram_send."

[[secrets]]
name = "bot_token"
label = "Bot token"
description = "Make a bot with @BotFather."

[[scopes]]
id = "send"
label = "Send you Telegram messages"
tools = ["telegram_send"]

[[scopes]]
id = "read"
label = "Read your messages to the bot"
tools = ["telegram_history"]

[[events]]
kind = "message"
scope = "read"
label = "A message you send the bot"

[[tools]]
name = "telegram_send"
description = "Send the person a message."
parameters = { type = "object", required = ["text"], properties = { text = { type = "string" } } }

[[tools]]
name = "telegram_history"
description = "The recent messages."
parameters = { type = "object", properties = {} }
"#;

const TOKEN: &str = "123456:bot-token-abcdefghijklmnop";

/// A ustar header and the file, padded.
fn tar_file(name: &str, content: &[u8]) -> Vec<u8> {
    let mut header = [0u8; 512];
    header[..name.len()].copy_from_slice(name.as_bytes());
    header[100..107].copy_from_slice(b"0000644");
    header[108..115].copy_from_slice(b"0000000");
    header[116..123].copy_from_slice(b"0000000");
    header[124..135].copy_from_slice(format!("{:011o}", content.len()).as_bytes());
    header[136..147].copy_from_slice(b"00000000000");
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    header[148..156].copy_from_slice(b"        ");
    let sum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
    header[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
    let mut out = header.to_vec();
    out.extend_from_slice(content);
    out.resize(out.len().div_ceil(512) * 512, 0);
    out
}

/// A .clapp of `manifest` signed with `key`, as build.sh makes one.
pub(crate) fn clapp(manifest: &str, wasm: &[u8], key: &SigningKey) -> Vec<u8> {
    let hex = |bytes: &[u8]| Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    let message = format!("claktory-app-v1\n{}  app.wasm\n{}  manifest.toml\n", hex(wasm), hex(manifest.as_bytes()));
    let signature = key.sign(message.as_bytes());
    let text = format!("key ed25519:{}\nsig {}\n", STANDARD.encode(key.verifying_key().as_bytes()), STANDARD.encode(signature.to_bytes()));
    let mut out = tar_file("manifest.toml", manifest.as_bytes());
    out.extend(tar_file("app.wasm", wasm));
    out.extend(tar_file("signature", text.as_bytes()));
    out.extend([0u8; 1024]);
    out
}

/// What the stand-in host was sent, and a way to send it things.
#[derive(Clone)]
struct Stand {
    log: Arc<Mutex<Vec<String>>>,
    /// Frames the stand-in sends factoryd on its own.
    tell: Arc<Mutex<Option<mpsc::UnboundedSender<Frame>>>>,
    /// Start fails with this, when set.
    fail_start: Arc<Mutex<Option<String>>>,
}

impl Stand {
    fn new() -> Stand {
        Stand { log: Arc::default(), tell: Arc::default(), fail_start: Arc::default() }
    }

    fn log(&self) -> Vec<String> {
        self.log.lock().expect("log").clone()
    }

    fn send(&self, body: Host) {
        let tell = self.tell.lock().expect("tell").clone().expect("the host runs");
        tell.send(Frame { id: 0, reply_to: 0, body: Some(body) }).expect("sent");
    }

    fn launcher(&self) -> factoryd::apps::Launcher {
        let stand = self.clone();
        Arc::new(move || {
            let (ours, theirs) = tokio::io::duplex(1 << 20);
            let (their_read, their_write) = tokio::io::split(theirs);
            tokio::spawn(stand.clone().serve(ours));
            Ok(AppsLink { reader: Box::new(their_read), writer: Box::new(their_write), child: None })
        })
    }

    async fn serve(self, stream: tokio::io::DuplexStream) {
        let (mut read, mut write) = tokio::io::split(stream);
        let (out, mut outgoing) = mpsc::unbounded_channel::<Frame>();
        *self.tell.lock().expect("tell") = Some(out.clone());
        tokio::spawn(async move {
            while let Some(frame) = outgoing.recv().await {
                let bytes = frame.encode_to_vec();
                if write.write_all(&(bytes.len() as u32).to_be_bytes()).await.is_err() || write.write_all(&bytes).await.is_err() {
                    return;
                }
            }
        });
        let mut next = 1_000;
        let mut asked_secret = None;
        loop {
            let mut length = [0u8; 4];
            if read.read_exact(&mut length).await.is_err() {
                return;
            }
            let mut bytes = vec![0; u32::from_be_bytes(length) as usize];
            if read.read_exact(&mut bytes).await.is_err() {
                return;
            }
            let frame = Frame::decode(bytes.as_slice()).expect("a frame");
            let answer = |body: Host| Frame { id: 0, reply_to: frame.id, body: Some(body) };
            let done = |error: &str| answer(Host::Done(wire::Done { error: error.to_owned() }));
            let reply = match frame.body.expect("a body") {
                Host::Hello(_) => {
                    self.log.lock().expect("log").push("hello".into());
                    answer(Host::Ready(wire::Ready { version: "stand-in".into(), worlds: vec!["claktory:app@1".into()] }))
                }
                Host::Load(load) => {
                    let wasm = std::fs::read(&load.component).unwrap_or_default();
                    self.log.lock().expect("log").push(format!(
                        "load {} {} hosts={} wasm={} from={}",
                        load.app_id,
                        load.version,
                        load.hosts.join(","),
                        wasm.len(),
                        load.upgrade_from
                    ));
                    done("")
                }
                Host::Start(start) => {
                    let failed = self.fail_start.lock().expect("fail").clone();
                    self.log.lock().expect("log").push(format!("start {}", start.app_id));
                    if let Some(error) = failed {
                        done(&error)
                    } else {
                        // Asks for its secret, as the real app would.
                        next += 1;
                        asked_secret = Some(next);
                        out.send(Frame {
                            id: next,
                            reply_to: 0,
                            body: Some(Host::GetSecret(wire::GetSecret { app_id: start.app_id.clone(), name: "bot_token".into() })),
                        })
                        .expect("asked");
                        done("")
                    }
                }
                Host::Stop(stop) => {
                    self.log.lock().expect("log").push(format!("stop {}", stop.app_id));
                    done("")
                }
                Host::Unload(unload) => {
                    self.log.lock().expect("log").push(format!("unload {}", unload.app_id));
                    done("")
                }
                Host::Call(call) => {
                    self.log.lock().expect("log").push(format!("call {} {} by {}", call.tool, call.args, call.caller));
                    answer(Host::CallResult(wire::CallResult {
                        text: format!("Sent for {}.", call.caller),
                        data: r#"{"message_id":7}"#.into(),
                        is_error: false,
                    }))
                }
                Host::SecretValue(value) if Some(frame.reply_to) == asked_secret => {
                    self.log.lock().expect("log").push(format!("secret found={} value={}", value.found, value.value));
                    continue;
                }
                other => panic!("the stand-in host got {other:?}"),
            };
            if out.send(reply).is_err() {
                return;
            }
        }
    }
}

fn failure(body: &Body) -> String {
    match body {
        Body::Error(error) => error.message.clone(),
        other => panic!("expected an error, got {other:?}"),
    }
}

fn app(view: &View) -> Option<v1::AppInfo> {
    view.workspace.apps.first().cloned()
}

fn state(view: &View) -> v1::AppState {
    app(view).map_or(v1::AppState::Unspecified, |app| app.state())
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == v1::AgentStatus::Idle && agent.queued_messages == 0
}

/// What `agent`'s call of `tool` with `args` answered.
async fn call(client: &mut Client, view: &mut View, agent: &str, tool: &str, args: &str) -> String {
    apply_request(client, view, Request::SelectAgent(v1::SelectAgent { agent_id: agent.into() })).await;
    let before = view.entries.len();
    apply_request(client, view, send_message(agent, &format!("call {tool} {args}"))).await;
    view.follow(client, |view| view.entries.len() >= before + 3 && idle(view, agent)).await;
    view.entries[before..]
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) => Some(call.output.clone()),
            _ => None,
        })
        .expect("a tool call")
}

async fn wait_for(stand: &Stand, line: &str) {
    for _ in 0..200 {
        if stand.log().iter().any(|logged| logged == line) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the host never got \"{line}\": {:?}", stand.log());
}

#[tokio::test]
async fn an_app_is_reviewed_installed_set_up_given_to_agents_and_heard_from() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let system_prompt = models.system_prompt.clone();
    let (addr, factory) = start_with(&dir, models).await;
    factory.set_vault_passphrase_cost("argon2id m=1024 t=1 p=1");
    factory.set_app_event_window(Duration::from_millis(300));
    let stand = Stand::new();
    factory.set_apps_host(stand.launcher());
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert!(view.workspace.apps_available && view.workspace.apps.is_empty());
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;

    // Reviewed first: what it can reach, and its file's checksum.
    let key = SigningKey::from_bytes(&[7; 32]);
    let wasm = b"\0asm\x01\0\0\0 stand-in".to_vec();
    let bundle = clapp(MANIFEST, &wasm, &key);
    let install = |approve: bool, sha256: &str, content: &[u8]| {
        Request::InstallLocalApp(v1::InstallLocalApp {
            path: String::new(),
            approve,
            sha256: sha256.into(),
            content: content.to_vec(),
            ..Default::default()
        })
    };
    let reviewed = apply_request(&mut client, &mut view, install(false, "", &bundle)).await;
    let Body::AppPermissions(permissions) = reviewed else { panic!("expected permissions, got {reviewed:?}") };
    assert_eq!((permissions.id.as_str(), permissions.name.as_str(), permissions.tier()), ("telegram", "Telegram", v1::AppTier::Local));
    assert_eq!(permissions.hosts, ["api.telegram.org"]);
    assert_eq!(
        permissions.scopes.iter().map(|scope| scope.label.as_str()).collect::<Vec<_>>(),
        ["Send you Telegram messages", "Read your messages to the bot"]
    );
    assert_eq!(permissions.secrets[0].label, "Bot token");
    assert_eq!(permissions.sha256, Sha256::digest(&bundle).iter().map(|byte| format!("{byte:02x}")).collect::<String>());
    assert!(view.workspace.apps.is_empty(), "nothing installed yet");

    // A file changed since it was signed, or not the one reviewed, is refused.
    let mut tampered = bundle.clone();
    let at = tampered.windows(8).position(|window| window == b"stand-in").expect("the component");
    tampered[at] = b'S';
    assert_eq!(
        failure(&apply_request(&mut client, &mut view, install(false, "", &tampered)).await),
        "This app was changed after it was signed, so Claktory won't install it."
    );
    assert_eq!(
        failure(&apply_request(&mut client, &mut view, install(true, "0000", &bundle)).await),
        "This file isn't the one you reviewed. Review it again before installing."
    );

    // Installed: loaded, but waiting for its bot token.
    let reply = apply_request(&mut client, &mut view, install(true, &permissions.sha256, &bundle)).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    view.follow(&mut client, |view| state(view) == v1::AppState::NeedsSetup).await;
    let info = app(&view).expect("installed");
    assert_eq!((info.version.as_str(), info.icon.as_str(), info.setup[0].done), ("0.1.0", "send", false));
    wait_for(&stand, &format!("load telegram 0.1.0 hosts=api.telegram.org wasm={} from=", wasm.len())).await;
    assert!(!stand.log().iter().any(|line| line.starts_with("start")), "{:?}", stand.log());
    assert!(std::fs::read(dir.join("home/config.json")).is_ok_and(|config| String::from_utf8_lossy(&config).contains("\"apps\"")));

    // Its secret: in the vault, given to the app alone; then it starts.
    let secret = v1::SetAppSecret { app_id: "telegram".into(), name: "bot_token".into(), value: format!(" {TOKEN} ") };
    apply_request(&mut client, &mut view, Request::SetAppSecret(secret)).await;
    view.follow(&mut client, |view| state(view) == v1::AppState::Running).await;
    wait_for(&stand, &format!("secret found=true value={TOKEN}")).await;
    let vault = view.workspace.vault.clone().expect("vault");
    let kept = vault.secrets.iter().find(|secret| secret.name == "Telegram bot token").expect("kept in the vault");
    assert_eq!(kept.grants.iter().map(|grant| grant.agent_id.as_str()).collect::<Vec<_>>(), ["app:telegram"]);
    assert!(app(&view).expect("app").setup[0].done);

    // Its status line, as it sets it.
    stand.send(Host::SetStatus(wire::SetStatus { app_id: "telegram".into(), text: "To link your Telegram, send /start 4821.".into() }));
    view.follow(&mut client, |view| app(view).is_some_and(|app| app.status == "To link your Telegram, send /start 4821.")).await;

    // Given to Builder to send: its tool, its instructions.
    let grant = |agent: &str, scopes: &[&str]| {
        Request::GrantApp(v1::GrantApp {
            app_id: "telegram".into(),
            agent_id: agent.into(),
            scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
        })
    };
    apply_request(&mut client, &mut view, grant("builder", &["send"])).await;
    let said = call(&mut client, &mut view, "builder", "telegram_send", r#"{"text":"Build failed"}"#).await;
    assert_eq!(said, "Sent for builder.");
    assert!(stand.log().contains(&r#"call telegram_send {"text":"Build failed"} by builder"#.to_owned()), "{:?}", stand.log());
    let prompt = system_prompt.lock().expect("prompt").clone();
    assert!(prompt.contains("# The Telegram app") && prompt.contains("Answer the person's messages with telegram_send."), "{prompt}");
    // Not what it wasn't given.
    let refused = call(&mut client, &mut view, "builder", "extension_call", r#"{"tool":"telegram_history"}"#).await;
    assert!(refused.contains("No tool \"telegram_history\""), "{refused}");
    let writer = call(&mut client, &mut view, "writer", "extension_call", r#"{"tool":"telegram_send","args":{"text":"hi"}}"#).await;
    assert!(writer.contains("No tool \"telegram_send\""), "{writer}");

    // Hearing its messages needs the scope that reads them.
    let subscribe = |agents: &[&str]| {
        Request::SubscribeApp(v1::SubscribeApp {
            app_id: "telegram".into(),
            event_kind: "message".into(),
            agent_ids: agents.iter().map(|agent| (*agent).to_owned()).collect(),
        })
    };
    assert_eq!(
        failure(&apply_request(&mut client, &mut view, subscribe(&["builder"])).await),
        "Builder can't hear a message you send the bot. Turn on \u{201c}Read your messages to the bot\u{201d} for Builder first."
    );
    apply_request(&mut client, &mut view, grant("builder", &["read", "send"])).await;
    apply_request(&mut client, &mut view, subscribe(&["builder"])).await;
    // Told mid-context, and its new tool reached through extension_call.
    let history = call(&mut client, &mut view, "builder", "extension_call", r#"{"tool":"telegram_history"}"#).await;
    assert_eq!(history, "Sent for builder.");
    assert!(
        describe(&view.entries).iter().any(|line| line.contains("You can now use the apps: Telegram (send, read).")),
        "{:?}",
        describe(&view.entries)
    );
    assert_eq!(app(&view).expect("app").grants, [v1::AppGrant { agent_id: "builder".into(), scopes: vec!["send".into(), "read".into()] }]);
    // On the Map: the app, what Builder was given and hears, and its secret.
    let map = view.workspace.map.clone().expect("map");
    let node = map.nodes.iter().find(|node| node.id == "app:telegram").expect("the app's node");
    assert_eq!((node.kind(), node.label.as_str(), node.icon.as_str()), (v1::MapNodeKind::App, "Telegram", "send"));
    let edge = |kind: v1::MapEdgeKind| map.edges.iter().find(|edge| edge.kind() == kind && edge.from_id == "app:telegram").cloned();
    let given = edge(v1::MapEdgeKind::App).expect("given");
    assert_eq!(
        (given.to_id.as_str(), given.label.as_str()),
        ("agent:builder", "Send you Telegram messages · Read your messages to the bot")
    );
    let hears = edge(v1::MapEdgeKind::Hears).expect("hears");
    assert_eq!((hears.to_id.as_str(), hears.label.as_str()), ("agent:builder", "A message you send the bot"));
    assert!(
        map.edges
            .iter()
            .any(|edge| edge.kind() == v1::MapEdgeKind::Secret && edge.from_id.starts_with("secret:") && edge.to_id == "app:telegram")
    );

    // Its events: the first at once, a burst folded into one, repeats dropped.
    let emit = |text: &str, key: &str| {
        Host::Emit(wire::Emit {
            app_id: "telegram".into(),
            kind: "message".into(),
            scope: "read".into(),
            text: text.into(),
            data: String::new(),
            dedupe_key: key.into(),
        })
    };
    stand.send(emit("Matt: Can you check the deploy?", "update:1"));
    stand.send(emit("Matt: Can you check the deploy?", "update:1"));
    stand.send(emit("Matt: And the DNS.", "update:2"));
    stand.send(emit("Matt: Thanks!", "update:3"));
    let heard = |view: &View| -> Vec<v1::Event> {
        view.entries
            .iter()
            .filter_map(|entry| match &entry.kind {
                Some(Kind::Event(event)) if event.source() == v1::EventSource::App => Some(event.clone()),
                _ => None,
            })
            .collect()
    };
    view.follow(&mut client, |view| heard(view).len() == 2 && idle(view, "builder")).await;
    let events = heard(&view);
    assert_eq!((events[0].from.as_str(), events[0].from_id.as_str()), ("Telegram", "telegram"));
    assert_eq!(events[0].text, "Matt: Can you check the deploy?");
    assert_eq!(events[1].text, "Matt: And the DNS.\nMatt: Thanks!");

    // A crash: it starts again by itself.
    stand.send(Host::Crashed(wire::Crashed { app_id: "telegram".into(), error: "It ran out of memory (32 MB).".into() }));
    view.follow(&mut client, |view| state(view) == v1::AppState::Restarting).await;
    assert_eq!(app(&view).expect("app").state_detail, "It ran out of memory (32 MB).");
    view.follow(&mut client, |view| state(view) == v1::AppState::Running).await;
    assert_eq!(stand.log().iter().filter(|line| *line == "start telegram").count(), 2);

    // Taken back from Builder: its tools and events go too.
    apply_request(&mut client, &mut view, grant("builder", &[])).await;
    let info = app(&view).expect("app");
    assert!(info.grants.is_empty() && info.subscriptions.is_empty(), "{info:?}");
    let gone = call(&mut client, &mut view, "builder", "telegram_send", r#"{"text":"hi"}"#).await;
    assert_eq!(gone, "You no longer have telegram_send of the Telegram app, so it doesn't work any more.");

    // Stopped, then uninstalled: the host forgets it; its secret stays in
    // the vault, no longer given to it.
    apply_request(&mut client, &mut view, Request::SetAppRunning(v1::SetAppRunning { id: "telegram".into(), running: false })).await;
    view.follow(&mut client, |view| state(view) == v1::AppState::Stopped).await;
    wait_for(&stand, "stop telegram").await;
    apply_request(&mut client, &mut view, Request::UninstallApp(v1::UninstallApp { id: "telegram".into(), keep_data: false })).await;
    view.follow(&mut client, |view| view.workspace.apps.is_empty()).await;
    let vault = view.workspace.vault.clone().expect("vault");
    let kept = vault.secrets.iter().find(|secret| secret.name == "Telegram bot token").expect("still in the vault");
    assert!(kept.grants.is_empty());
    assert!(!dir.join("home/apps/telegram").exists());
}

#[tokio::test]
async fn an_app_that_keeps_crashing_is_paused_and_rivet_hears_why() {
    let dir = temp_dir();
    let (addr, factory) = start_with(&dir, Scripted::signed_in()).await;
    factory.set_vault_passphrase_cost("argon2id m=1024 t=1 p=1");
    let stand = Stand::new();
    *stand.fail_start.lock().expect("fail") = Some("It trapped: unreachable.".to_owned());
    factory.set_apps_host(stand.launcher());
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    // No secrets to set up: it starts at once.
    let manifest =
        MANIFEST.split("[[secrets]]").next().expect("head").to_owned() + &MANIFEST[MANIFEST.find("[[scopes]]").expect("scopes")..];
    let bundle = clapp(&manifest, b"\0asm\x01\0\0\0", &SigningKey::from_bytes(&[9; 32]));
    let reviewed = apply_request(
        &mut client,
        &mut view,
        Request::InstallLocalApp(v1::InstallLocalApp { content: bundle.clone(), ..Default::default() }),
    )
    .await;
    let Body::AppPermissions(permissions) = reviewed else { panic!("expected permissions, got {reviewed:?}") };
    let approve = v1::InstallLocalApp { content: bundle.clone(), approve: true, sha256: permissions.sha256.clone(), ..Default::default() };
    apply_request(&mut client, &mut view, Request::InstallLocalApp(approve)).await;
    // Fails to start, waits 1 s, then 5 s, then pauses.
    view.follow(&mut client, |view| state(view) == v1::AppState::PausedCrashed).await;
    assert_eq!(app(&view).expect("app").state_detail, "It trapped: unreachable.");
    assert_eq!(stand.log().iter().filter(|line| *line == "start telegram").count(), 3);
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    let told = view.entries.iter().any(|entry| {
        matches!(&entry.kind, Some(Kind::Event(event)) if event.text.starts_with(
            "The Telegram app crashed 3 times in 10 minutes, so Claktory paused it. The last error: It trapped: unreachable."
        ))
    });
    assert!(told, "{:?}", describe(&view.entries));

    // A different publisher's Telegram can't replace it.
    let other = clapp(&manifest, b"\0asm\x01\0\0\0", &SigningKey::from_bytes(&[3; 32]));
    let refused =
        apply_request(&mut client, &mut view, Request::InstallLocalApp(v1::InstallLocalApp { content: other, ..Default::default() })).await;
    assert_eq!(
        failure(&refused),
        "This Telegram comes from a different publisher than the one installed. To install it, uninstall that one first."
    );

    // Started again by the person, it runs.
    *stand.fail_start.lock().expect("fail") = None;
    apply_request(&mut client, &mut view, Request::SetAppRunning(v1::SetAppRunning { id: "telegram".into(), running: true })).await;
    view.follow(&mut client, |view| state(view) == v1::AppState::Running).await;
    drop(client);
    drop(factory);

    // After a restart it's loaded and started again.
    let stand = Stand::new();
    let (addr, factory) = start_with(&dir, Scripted::signed_in()).await;
    factory.set_apps_host(stand.launcher());
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    view.follow(&mut client, |view| state(view) == v1::AppState::Running).await;
    wait_for(&stand, "start telegram").await;
}

#[tokio::test]
async fn apps_leave_no_trace_where_they_are_not_offered() {
    // Like a Claktory where apps don't ship: not the dev one, no host set.
    let dir = temp_dir();
    let (addr, factory) = start_with(&dir, Scripted::signed_in()).await;
    factory.set_apps_unavailable_for_test();
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert!(!view.workspace.apps_available && view.workspace.apps.is_empty());
    let bundle = clapp(MANIFEST, b"\0asm\x01\0\0\0", &SigningKey::from_bytes(&[7; 32]));
    let install = Request::InstallLocalApp(v1::InstallLocalApp { content: bundle, ..Default::default() });
    assert_eq!(failure(&apply_request(&mut client, &mut view, install).await), "Apps aren't available in this Claktory yet.");
    let running = Request::SetAppRunning(v1::SetAppRunning { id: "telegram".into(), running: true });
    assert_eq!(failure(&apply_request(&mut client, &mut view, running).await), "Apps aren't available in this Claktory yet.");
    let map = view.workspace.map.clone().expect("map");
    assert!(!map.nodes.iter().any(|node| node.kind() == v1::MapNodeKind::App));
}

/// Installs a real .clapp in the dev Claktory (port 7421) and sets it up:
/// `CLAPP=<path> [BOT_TOKEN=…] [GRANT_AGENT=<agent id>] cargo test -p
/// factoryd --test all apps::dev_install -- --ignored --nocapture`. A token
/// only ever comes from the environment (a vault secret's variable), into
/// the dev vault; it's never printed. The same file again changes nothing,
/// so it also sets up an app already installed.
#[tokio::test]
#[ignore = "needs the dev Claktory, its apps host and a .clapp"]
async fn dev_install() {
    let path = std::env::var("CLAPP").expect("CLAPP: the .clapp's path");
    // The loopback port wants this machine's app's token.
    let code = std::fs::read_to_string(std::env::var("HOME").expect("HOME") + "/.factory-dev/remote/local-pairing").expect("pairing code");
    let token = reqwest::Url::parse(code.trim()).expect("code").query_pairs().find(|(key, _)| key == "t").expect("token").1.into_owned();
    let mut client = Client::open("127.0.0.1:7421".parse().expect("address")).await;
    let hello =
        v1::Hello { protocol_version: factory_protocol::PROTOCOL_VERSION, client_version: "test".into(), token, ..Default::default() };
    client.send(v1::ClientMessage { id: 1, body: Some(Request::Hello(hello)) }).await;
    assert!(matches!(client.receive().await.body, Some(Body::Welcome(_))));
    let Some(Body::Workspace(workspace)) = client.receive().await.body else { panic!("expected the workspace") };
    let mut view = View { workspace, ..View::default() };
    let review = Request::InstallLocalApp(v1::InstallLocalApp { path: path.clone(), ..Default::default() });
    let Body::AppPermissions(permissions) = apply_request(&mut client, &mut view, review).await else { panic!("not an app") };
    println!("review: {permissions:#?}");
    let approve = v1::InstallLocalApp { path, approve: true, sha256: permissions.sha256.clone(), ..Default::default() };
    let reply = apply_request(&mut client, &mut view, Request::InstallLocalApp(approve)).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    if let Ok(token) = std::env::var("BOT_TOKEN") {
        for secret in &permissions.secrets {
            let set = v1::SetAppSecret { app_id: permissions.id.clone(), name: secret.name.clone(), value: token.clone() };
            let reply = apply_request(&mut client, &mut view, Request::SetAppSecret(set)).await;
            assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
        }
    }
    // GRANT_AGENT=<agent id>: given every scope, and hearing every event.
    if let Ok(agent) = std::env::var("GRANT_AGENT") {
        let scopes = permissions.scopes.iter().map(|scope| scope.id.clone()).collect();
        let grant = v1::GrantApp { app_id: permissions.id.clone(), agent_id: agent.clone(), scopes };
        let reply = apply_request(&mut client, &mut view, Request::GrantApp(grant)).await;
        assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
        for event in &permissions.events {
            let subscribe =
                v1::SubscribeApp { app_id: permissions.id.clone(), event_kind: event.kind.clone(), agent_ids: vec![agent.clone()] };
            let reply = apply_request(&mut client, &mut view, Request::SubscribeApp(subscribe)).await;
            assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
        }
    }
    let id = permissions.id.clone();
    let found = |view: &View| view.workspace.apps.iter().find(|app| app.id == id).cloned();
    tokio::time::timeout(Duration::from_secs(30), view.follow(&mut client, |view| found(view).is_some_and(|app| !app.status.is_empty())))
        .await
        .ok();
    println!("app: {:#?}", found(&view));
}

/// Claktory's own apps key (#C7) changes no install: an app keeps updating
/// with the key it was installed with, stays as trusted as it was, and no
/// other key replaces it.
#[tokio::test]
async fn an_installed_app_keeps_updating_with_its_own_key_and_no_other() {
    let dir = temp_dir();
    let (addr, factory) = start_with(&dir, Scripted::signed_in()).await;
    factory.set_vault_passphrase_cost("argon2id m=1024 t=1 p=1");
    let stand = Stand::new();
    factory.set_apps_host(stand.launcher());
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let manifest =
        MANIFEST.split("[[secrets]]").next().expect("head").to_owned() + &MANIFEST[MANIFEST.find("[[scopes]]").expect("scopes")..];
    let key = SigningKey::from_bytes(&[9; 32]);
    for version in ["0.1.0", "0.2.0"] {
        let manifest = manifest.replace("version = \"0.1.0\"", &format!("version = \"{version}\""));
        let bundle = clapp(&manifest, b"\0asm\x01\0\0\0", &key);
        let install = v1::InstallLocalApp { content: bundle.clone(), ..Default::default() };
        let reviewed = apply_request(&mut client, &mut view, Request::InstallLocalApp(install)).await;
        let Body::AppPermissions(permissions) = reviewed else { panic!("expected permissions, got {reviewed:?}") };
        assert_eq!(permissions.tier(), v1::AppTier::Local);
        let approve = v1::InstallLocalApp { content: bundle, approve: true, sha256: permissions.sha256.clone(), ..Default::default() };
        apply_request(&mut client, &mut view, Request::InstallLocalApp(approve)).await;
        view.follow(&mut client, |view| app(view).is_some_and(|app| app.version == version)).await;
        assert_eq!(app(&view).expect("app").tier(), v1::AppTier::Local);
    }

    let other = clapp(&manifest.replace("0.1.0", "0.3.0"), b"\0asm\x01\0\0\0", &SigningKey::from_bytes(&[3; 32]));
    let refused =
        apply_request(&mut client, &mut view, Request::InstallLocalApp(v1::InstallLocalApp { content: other, ..Default::default() })).await;
    assert_eq!(
        failure(&refused),
        "This Telegram comes from a different publisher than the one installed. To install it, uninstall that one first."
    );
    assert_eq!(app(&view).expect("app").version, "0.2.0");
}

/// Claktory's own apps ship with the release (#201): the real signed
/// Charts in its folder is offered, reviewed and installed like any file;
/// a file that isn't signed by Claktory, or was changed, isn't offered or
/// installed, even swapped in after it was listed.
#[tokio::test]
async fn claktorys_own_apps_are_offered_checked_and_installed_from_the_release() {
    let dir = temp_dir();
    let (addr, factory) = start_with(&dir, Scripted::signed_in()).await;
    let shipped = dir.join("release-apps");
    std::fs::create_dir_all(&shipped).expect("folder");
    let charts = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../apps/dist/charts.clapp")).expect("Charts, signed");
    std::fs::write(shipped.join("charts.clapp"), &charts).expect("written");
    // Signed, but not by Claktory; and Charts with a byte changed.
    let local = clapp(MANIFEST, b"\0asm\x01\0\0\0", &SigningKey::from_bytes(&[7; 32]));
    std::fs::write(shipped.join("telegram.clapp"), &local).expect("written");
    let mut changed = charts.clone();
    let middle = changed.len() / 2;
    changed[middle] ^= 1;
    std::fs::write(shipped.join("changed.clapp"), &changed).expect("written");
    factoryd::apps::bundled::set_folder_for_test(&factory, shipped.clone());

    // Not offered where apps aren't.
    factory.set_apps_unavailable_for_test();
    let (client, workspace, _) = Client::connect(addr).await;
    assert!(workspace.bundled_apps.is_empty());
    drop(client);

    let stand = Stand::new();
    factory.set_apps_host(stand.launcher());
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let offered: Vec<_> =
        view.workspace.bundled_apps.iter().map(|app| (app.id.as_str(), app.version.as_str(), app.installed_version.as_str())).collect();
    assert_eq!(offered, [("charts", "0.2.1", "")]);
    let listed = view.workspace.bundled_apps[0].clone();
    assert_eq!((listed.name.as_str(), listed.publisher.as_str(), listed.icon.as_str()), ("Charts", "claktory", "chart-column"));

    // Only what's offered installs this way.
    let telegram = v1::InstallLocalApp { bundled: "telegram".into(), ..Default::default() };
    let refused = apply_request(&mut client, &mut view, Request::InstallLocalApp(telegram)).await;
    assert_eq!(failure(&refused), "Claktory doesn't come with an app called telegram.");

    // Reviewed like a file, as Claktory's.
    let review = v1::InstallLocalApp { bundled: "charts".into(), ..Default::default() };
    let Body::AppPermissions(permissions) = apply_request(&mut client, &mut view, Request::InstallLocalApp(review)).await else {
        panic!("expected its permissions")
    };
    assert_eq!((permissions.id.as_str(), permissions.tier(), permissions.path.as_str()), ("charts", v1::AppTier::Claktory, ""));
    let sha256 = Sha256::digest(&charts).iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    assert_eq!(permissions.sha256, sha256);

    // Swapped after it was listed: what's read now is checked again.
    std::fs::write(shipped.join("charts.clapp"), &local).expect("swapped");
    let approve = v1::InstallLocalApp { bundled: "charts".into(), approve: true, sha256: sha256.clone(), ..Default::default() };
    let refused = apply_request(&mut client, &mut view, Request::InstallLocalApp(approve.clone())).await;
    assert!(failure(&refused).contains("isn't signed by Claktory"), "{}", failure(&refused));
    std::fs::write(shipped.join("charts.clapp"), &changed).expect("swapped");
    let refused = apply_request(&mut client, &mut view, Request::InstallLocalApp(approve.clone())).await;
    assert!(failure(&refused).contains("changed after it was signed"), "{}", failure(&refused));

    // The real one installs, as Claktory's, and isn't offered any more.
    std::fs::write(shipped.join("charts.clapp"), &charts).expect("put back");
    let reply = apply_request(&mut client, &mut view, Request::InstallLocalApp(approve)).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    view.follow(&mut client, |view| view.workspace.apps.iter().any(|app| app.id == "charts")).await;
    let installed = view.workspace.apps.iter().find(|app| app.id == "charts").expect("installed").clone();
    assert_eq!((installed.version.as_str(), installed.tier()), ("0.2.1", v1::AppTier::Claktory));
    view.follow(&mut client, |view| view.workspace.bundled_apps.is_empty()).await;
}
