//! #546 (Matt's card 237): what's the person's alone stays theirs. No agent,
//! on any side, sees or handles a pairing code, a connect code or the four
//! digits; agents only raise cards. Fugo's plan is
//! coordination/security/agents-and-pairing.md; the map of which test proves
//! which path is coordination/security/agent-safety.md (Gyro).
//!
//! Each test acts as an agent would: through the tool token's connection
//! (what an agent's CLI or its own script gets on the loopback port), or
//! through `factoryd` run as a command in its shell. They fail until their
//! block lands, ignored with the block's name ("#546 A", "#546 B", "#546
//! moves", "D", "G8"); its owner removes the ignore when it goes in.
//!
//! Every test checks first that the tool connection is held to Tool
//! authority (`gated`), so run against a backend without the gate it stops
//! there, sending nothing that would act.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::common::{Client, MODEL, View, apply_patch, apply_request, home, start, temp_dir};
use factory_protocol::PROTOCOL_VERSION;
use factory_protocol::v1::{self, ClientMessage, ErrorCode, ServerMessage, client_message, entry::Kind, server_message::Body};
use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as Frame;

const WAIT: Duration = Duration::from_secs(60);

// A's interface (Giorno 42047, 42182; Fugo 42067, 42185). Until A lands
// these stand in for it; when it does, each becomes what its comment says.

/// ERROR_CODE_PERSON_ONLY. A: `ErrorCode::PersonOnly as i32`.
const PERSON_ONLY: i32 = 18;

/// The gate: whether a Tool connection may send `body`; everything else is
/// refused with PERSON_ONLY. A: `factoryd::authority::tool_allowed(body)`.
fn tool_allowed(body: &client_message::Body) -> bool {
    match body {
        client_message::Body::UpdateSettings(update) => settings_allowed(update),
        _ => TOOL_ALLOWED.contains(&name(body).as_str()),
    }
}

/// Where the Mac app's socket is. A: `factoryd::authority::app_socket_path(&factory)`.
fn app_socket(dir: &Path) -> PathBuf {
    home(dir).root().join("app.sock")
}

/// What a Tool connection may send (the allowlist, Fugo 42185, Giorno
/// 42182): Hello, the settings `factoryd settings set` changes, Add a device
/// without a code (B's RequestAddDevice) and calling off a move. Nothing
/// goes on it without a reason sent to Fugo first.
const TOOL_ALLOWED: &[&str] = &["Hello", "UpdateSettings", "RequestAddDevice", "CancelMove"];

/// The fields of UpdateSettings a Tool connection may set: what `factoryd
/// settings set` takes (Giorno 42182). Any other is refused.
const SETTINGS_ALLOWED: &[&str] = &["user_name", "name", "color", "default_model", "default_effort"];

fn settings_allowed(update: &v1::UpdateSettings) -> bool {
    let set = fields_set(&update.encode_to_vec());
    let allowed: Vec<u32> = proto()
        .fields("UpdateSettings")
        .iter()
        .filter(|field| SETTINGS_ALLOWED.contains(&field.name.as_str()))
        .map(|field| field.number)
        .collect();
    set.iter().all(|number| allowed.contains(number))
}

/// Never for a Tool connection, whatever the allowlist becomes: codes, the
/// digits, the account, signing in, pipe connects, the vault's values and
/// recovery key, updates, the person's answers, and what only the person's
/// own devices do (G2, G3, G4 in agent-safety.md).
const NEVER_FOR_TOOLS: &[&str] = &[
    "ShowPairingCode",
    "CreatePairing",
    "ApproveDevice",
    "DenyDevice",
    "ConfirmDevice",
    "VouchDevice",
    "DismissAllowNotice",
    "UnpairDevice",
    "CancelPairing",
    "AccountSignIn",
    "AccountVerify",
    "AccountResend",
    "AccountSignInCancel",
    "PublishRecoveryKey",
    "UseRecoveryKey",
    "AccountSignOut",
    "SkipAccountSignIn",
    "SetAccountService",
    "SetRemoteAccess",
    "StartLogin",
    "AnswerLogin",
    "CancelLogin",
    "UpdateAccount",
    "CreatePipeInvite",
    "AcceptPipeInvite",
    "ApprovePipe",
    "AcceptPipeIdentity",
    "RevealSecret",
    "ShowRecoveryKey",
    "NewRecoveryKey",
    "SetSecretCard",
    "SetSecretSites",
    "SetSecretAskEachTime",
    "InstallUpdate",
    "StartMove",
    "ConfirmMove",
    "DecideProposal",
    "AnswerAsk",
    "DecideSecretRequest",
    "DecideSecretApproval",
    "DmLeafOffer",
    "DmEnrollLeaf",
    "BrowserCapability",
    "BrowserGrant",
    "RegisterNotifications",
    // Not in the protocol yet (Fugo 42660): D's switch, and the account key's removal.
    "SetCommandContainment",
    "UnpairAccountKey",
];

/// Requests on these lists that aren't in the protocol on main yet (S3/S4,
/// #435 W9, B): checked once they are.
const NOT_YET: &[&str] = &[
    "VouchDevice",
    "DismissAllowNotice",
    "SetSecretCard",
    "SetSecretSites",
    "RequestAddDevice",
    "SetCommandContainment",
    "UnpairAccountKey",
];

/// Requests that act on the process itself: only the gate's answer for them
/// is checked, so a gate that let one through can't stop the test's
/// process.
const NEVER_SENT: &[&str] = &["StopEngine", "RestoreBackendService"];

/// A request's name, as the protocol has it: "ApproveDevice".
fn name(body: &client_message::Body) -> String {
    let debug = format!("{body:?}");
    debug.split('(').next().unwrap_or_default().to_owned()
}

/// Every request the protocol has: each oneof field, decoded from an empty
/// message under its number (as rivet_reach's unit test lists them).
fn every_request() -> Vec<(u32, client_message::Body)> {
    (2..1000)
        .filter_map(|number: u32| {
            let mut bytes = Vec::new();
            prost::encoding::encode_key(number, prost::encoding::WireType::LengthDelimited, &mut bytes);
            bytes.push(0);
            ClientMessage::decode(bytes.as_slice()).ok()?.body.map(|body| (number, body))
        })
        .collect()
}

/// The tool token: what every agent's command can read (`<home>/tool-token`).
fn tool_token(dir: &Path) -> String {
    let path = home(dir).root().join("tool-token");
    let token = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("no tool token at {}: {error}", path.display()));
    token.trim().to_owned()
}

/// What an agent's command makes of the loopback port with a token: its own
/// WebSocket client, following the Workspace it's given.
struct Agent {
    socket: WebSocketStream<TcpStream>,
    next_id: u64,
    device_id: String,
    workspace: v1::Workspace,
    /// Every Workspace it was given, patches applied, in order.
    seen: Vec<v1::Workspace>,
    /// Every reply it got, for what a reply mustn't carry.
    replies: Vec<Body>,
}

impl Agent {
    /// Says hello with `token`: the agent once welcomed, or what it got
    /// instead (None: closed).
    async fn hello(addr: SocketAddr, token: &str, patches: bool) -> Result<Agent, Option<Body>> {
        let tcp = TcpStream::connect(addr).await.expect("connect");
        let (mut socket, _) = tokio_tungstenite::client_async("ws://factory/", tcp).await.expect("websocket");
        let hello = v1::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_version: "agent".into(),
            token: token.into(),
            workspace_patches: patches,
            ..Default::default()
        };
        let message = ClientMessage { id: 1, body: Some(client_message::Body::Hello(hello)) };
        socket.send(Frame::Binary(message.encode_to_vec().into())).await.expect("send");
        let welcome = match receive(&mut socket).await.and_then(|message| message.body) {
            Some(Body::Welcome(welcome)) => welcome,
            other => return Err(other),
        };
        let mut agent = Agent {
            socket,
            next_id: 2,
            device_id: welcome.device_id,
            workspace: Default::default(),
            seen: Vec::new(),
            replies: Vec::new(),
        };
        agent.follow(|_| true).await;
        Ok(agent)
    }

    /// The tool connection, welcomed.
    async fn tool(addr: SocketAddr, dir: &Path, patches: bool) -> Agent {
        match Agent::hello(addr, &tool_token(dir), patches).await {
            Ok(agent) => agent,
            Err(other) => panic!("the tool token wasn't welcomed on the loopback port: {other:?}"),
        }
    }

    fn take(&mut self, body: &Body) {
        match body {
            Body::Workspace(workspace) => self.workspace = workspace.clone(),
            Body::WorkspacePatch(patch) => self.workspace = apply_patch(&self.workspace, patch),
            _ => return,
        }
        self.seen.push(self.workspace.clone());
    }

    /// Reads until a Workspace for which `done` holds (the first one, for
    /// `|_| true`).
    async fn follow(&mut self, mut done: impl FnMut(&v1::Workspace) -> bool) {
        let waited = tokio::time::timeout(WAIT, async {
            loop {
                let message = receive(&mut self.socket).await.expect("the connection stays open");
                let Some(body) = message.body else { continue };
                let workspace = matches!(body, Body::Workspace(_) | Body::WorkspacePatch(_));
                self.take(&body);
                if workspace && done(&self.workspace) {
                    return;
                }
            }
        })
        .await;
        assert!(waited.is_ok(), "the agent's Workspace never got there");
    }

    async fn request(&mut self, body: client_message::Body) -> Body {
        let id = self.next_id;
        self.next_id += 1;
        let message = ClientMessage { id, body: Some(body) };
        self.socket.send(Frame::Binary(message.encode_to_vec().into())).await.expect("send");
        loop {
            let message = receive(&mut self.socket).await.expect("an answer");
            let Some(body) = message.body else { continue };
            if message.reply_to == id {
                self.replies.push(body.clone());
                return body;
            }
            self.take(&body);
        }
    }
}

async fn receive<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(socket: &mut WebSocketStream<S>) -> Option<ServerMessage> {
    loop {
        match tokio::time::timeout(WAIT, socket.next()).await.expect("the server answers in time") {
            Some(Ok(Frame::Binary(bytes))) => return Some(ServerMessage::decode(bytes).expect("decode")),
            Some(Ok(Frame::Close(_)) | Err(_)) | None => return None,
            Some(Ok(_)) => {}
        }
    }
}

fn person_only(body: &Body) -> bool {
    matches!(body, Body::Error(error) if error.code == PERSON_ONLY)
}

/// The tool connection is held to Tool authority: a person request is
/// refused as one. Checked before anything else is sent.
async fn gated(agent: &mut Agent) {
    assert_ne!(agent.device_id, factoryd::remote::LOCAL_DEVICE, "the tool connection is this Mac's own app");
    let probe = client_message::Body::ApproveDevice(v1::ApproveDevice::default());
    let answer = agent.request(probe).await;
    assert!(person_only(&answer), "the tool connection isn't held to Tool authority: ApproveDevice was answered {answer:?}");
}

/// The person on this Mac (common's Client::connect, the person's in tests),
/// with remote access through a relay that leads nowhere, so codes can be
/// made without the internet.
async fn person(addr: SocketAddr, factory: &factoryd::Factory) -> (Client, View) {
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let relays = v1::SetRemoteAccess { relays: vec!["http://127.0.0.1:9/".into()], set_relays: true, ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::SetRemoteAccess(relays)).await;
    (client, view)
}

fn pending(view: &View) -> Vec<v1::PendingDevice> {
    view.workspace.remote.as_ref().map(|remote| remote.pending_devices.clone()).unwrap_or_default()
}

/// Paths 1, 4, 6, 9 (A), G2-G4: over the loopback port, the tool token is
/// welcomed as something other than this Mac's app, and every request off
/// the Tool allowlist is refused as the person's. Each is sent empty, so the
/// refusal has to come before the request is handled: PERSON_ONLY, never
/// INVALID_REQUEST or NOT_FOUND. NEVER_FOR_TOOLS stays off the allowlist
/// whatever it becomes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "#546 A"]
async fn a_tool_connection_is_refused_everything_off_its_allowlist() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let mut agent = Agent::tool(addr, &dir, false).await;
    gated(&mut agent).await;

    let requests = every_request();
    assert!(requests.len() > 200, "{} requests", requests.len());
    let names: Vec<String> = requests.iter().map(|(_, body)| name(body)).collect();
    let listed = NEVER_FOR_TOOLS.iter().chain(TOOL_ALLOWED).chain(NEVER_SENT);
    let unknown: Vec<&&str> = listed.filter(|listed| !names.iter().any(|name| name == *listed) && !NOT_YET.contains(listed)).collect();
    assert!(unknown.is_empty(), "not requests of the protocol: {unknown:?}");

    let mut wrong = Vec::new();
    let mut refused = 0;
    for (number, body) in requests {
        let name = name(&body);
        let never = NEVER_FOR_TOOLS.contains(&name.as_str()) || NEVER_SENT.contains(&name.as_str());
        if never && tool_allowed(&body) {
            wrong.push(format!("{number} {name}: on the Tool allowlist"));
        }
        if name == "Hello" || (TOOL_ALLOWED.contains(&name.as_str()) && !never) || NEVER_SENT.contains(&name.as_str()) {
            continue;
        }
        let answer = agent.request(body).await;
        if person_only(&answer) {
            refused += 1;
        } else {
            wrong.push(format!("{number} {name}: answered {answer:?}"));
        }
    }
    assert!(wrong.is_empty(), "a tool connection did what only the person does:\n{}", wrong.join("\n"));
    assert!(refused > 200, "{refused} refused");
}

/// A: UpdateSettings from a Tool connection changes only what `factoryd
/// settings set` takes. Each other field, set alone, is refused and changes
/// nothing; so are the account service and the relays, which are requests
/// of their own (Fugo 42185). The test reads UpdateSettings' fields from
/// factory.proto, so a new one is refused unless it's added to
/// SETTINGS_ALLOWED.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "#546 A"]
async fn a_tool_connection_changes_only_the_settings_the_cli_sets() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let mut agent = Agent::tool(addr, &dir, false).await;
    gated(&mut agent).await;
    let before = agent.workspace.settings.clone();
    let mut wrong = Vec::new();
    for field in proto().fields("UpdateSettings") {
        if SETTINGS_ALLOWED.contains(&field.name.as_str()) {
            continue;
        }
        let bytes = field.sample();
        let update = v1::UpdateSettings::decode(bytes.as_slice()).unwrap_or_else(|error| panic!("{}: {error}", field.name));
        assert!(!tool_allowed(&client_message::Body::UpdateSettings(update.clone())), "{} on the allowlist", field.name);
        let answer = agent.request(client_message::Body::UpdateSettings(update)).await;
        if !person_only(&answer) {
            wrong.push(format!("{}: answered {answer:?}", field.name));
        }
    }
    let service = client_message::Body::SetAccountService(v1::SetAccountService { url: "https://account.example".into() });
    let relays = client_message::Body::SetRemoteAccess(v1::SetRemoteAccess {
        relays: vec!["https://relay.example/".into()],
        set_relays: true,
        ..Default::default()
    });
    for request in [service, relays] {
        let name = name(&request);
        let answer = agent.request(request).await;
        if !person_only(&answer) {
            wrong.push(format!("{name}: answered {answer:?}"));
        }
    }
    assert!(wrong.is_empty(), "a tool connection changed what only the person does:\n{}", wrong.join("\n"));
    agent.request(client_message::Body::UpdateSettings(v1::UpdateSettings::default())).await;
    assert_eq!(agent.workspace.settings, before, "the settings changed");
    // What `factoryd settings set` changes still works.
    let named = client_message::Body::UpdateSettings(v1::UpdateSettings { name: Some("Studio".into()), ..Default::default() });
    assert!(matches!(agent.request(named).await, Body::Ack(_)));
}

/// Path 3 (A): with a device waiting, the tool connection gets nowhere with
/// the right four digits: ApproveDevice and DenyDevice are refused and it
/// still waits. The person's connection then allows it with the same digits,
/// so the refusal wasn't for anything else.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "#546 A"]
async fn a_tool_connection_cant_allow_a_waiting_device_even_with_its_digits() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let mut agent = Agent::tool(addr, &dir, false).await;
    gated(&mut agent).await;
    let (mut client, mut view) = person(addr, &factory).await;

    let (code, id, device, _link, mut socket) = crate::remote::waiting_device(&mut client, &mut view, "Phone").await;
    view.follow(&mut client, |view| pending(view).len() == 1).await;
    let right = crate::remote::number(&code, &device.id().to_string());

    let allow = client_message::Body::ApproveDevice(v1::ApproveDevice { device_id: id.clone(), number: right.clone(), ..Default::default() });
    let deny = client_message::Body::DenyDevice(v1::DenyDevice { device_id: id.clone() });
    for request in [allow.clone(), deny] {
        let answer = agent.request(request).await;
        assert!(person_only(&answer), "{answer:?}");
    }
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    assert_eq!(pending(&view).iter().map(|device| device.device_id.as_str()).collect::<Vec<_>>(), [id.as_str()], "still waiting");

    assert!(matches!(apply_request(&mut client, &mut view, allow).await, Body::Ack(_)));
    match crate::remote::next(&mut socket).await {
        Some(Body::Welcome(welcome)) => assert_eq!(welcome.device_id, id),
        other => panic!("expected the device let in, got {other:?}"),
    }
}

/// Paths 2 and 5 (A): an admin agent's devices_add pairing shows its code
/// only to the person's app. ShowPairingCode for it from the tool
/// connection is refused, and no reply to it ever carries a code; nor does
/// CreatePairing hand it one (G2).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "#546 A"]
async fn a_tool_connection_never_gets_a_pairing_code() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let mut agent = Agent::tool(addr, &dir, false).await;
    gated(&mut agent).await;
    let (mut client, mut view) = person(addr, &factory).await;
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;

    let before = view.entries.len();
    let call = format!("call devices_add {}", serde_json::json!({"name": "Matt's iPhone", "kind": "phone"}));
    apply_request(&mut client, &mut view, crate::common::send_message("rivet", &call)).await;
    view.follow(&mut client, |view| {
        view.entries[before..].iter().any(|entry| matches!(&entry.kind, Some(Kind::DevicePairing(pairing)) if !pairing.id.is_empty()))
    })
    .await;
    let pairing = view.entries[before..]
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::DevicePairing(pairing)) => Some(pairing.id.clone()),
            _ => None,
        })
        .expect("the card");

    let show = client_message::Body::ShowPairingCode(v1::ShowPairingCode { pairing_id: pairing });
    let answer = agent.request(show.clone()).await;
    assert!(person_only(&answer), "{answer:?}");
    let create = client_message::Body::CreatePairing(v1::CreatePairing { name: "Agent's phone".into(), host: String::new() });
    let answer = agent.request(create).await;
    assert!(person_only(&answer), "{answer:?}");
    assert!(!agent.replies.iter().any(|reply| matches!(reply, Body::PairingCode(_))), "a code reached the agent");
    // The person's app still gets it, so the refusal was the tool token's.
    assert!(matches!(apply_request(&mut client, &mut view, show).await, Body::PairingCode(code) if !code.code.is_empty()));
}

/// Code-shaped text in a Workspace an agent was given: a pairing code or
/// its link, a pipe invite, a move's receive code.
fn codes_in(workspace: &v1::Workspace) -> Vec<&'static str> {
    let text = format!("{workspace:?}");
    ["factory://", "claktory://", "claktory-pipe://", "claktory-move://", "/pair#", "?t=", "&t="]
        .into_iter()
        .filter(|shape| text.contains(shape))
        .collect()
}

/// Every place a tool connection's Workspaces could carry the digits or a
/// code: what it found.
fn digits_and_codes(seen: &[v1::Workspace]) -> Vec<String> {
    let mut found = Vec::new();
    for (at, workspace) in seen.iter().enumerate() {
        for device in workspace.remote.iter().flat_map(|remote| &remote.pending_devices) {
            if !device.number.is_empty() {
                found.push(format!("Workspace {at}: a waiting device's digits"));
            }
        }
        for pipe in &workspace.pipes {
            if !pipe.digits.is_empty() {
                found.push(format!("Workspace {at}: pipe {}'s digits", pipe.id));
            }
        }
        if workspace.r#move.as_ref().is_some_and(|seen| !seen.digits.is_empty()) {
            found.push(format!("Workspace {at}: the move's digits"));
        }
        if workspace.login.as_ref().is_some_and(|login| !login.user_code.is_empty()) {
            found.push(format!("Workspace {at}: a provider sign-in's code"));
        }
        for shape in codes_in(workspace) {
            found.push(format!("Workspace {at}: {shape}"));
        }
    }
    found
}

/// Path 3, G1 (A): a waiting device's four digits are sent to no one, the
/// person's own app included (Fugo 42045 (a)), and a tool connection's
/// Workspaces carry no code, whether it takes whole Workspaces or patches.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "#546 A"]
async fn no_connection_is_sent_a_waiting_devices_digits() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let mut whole = Agent::tool(addr, &dir, false).await;
    let mut patched = Agent::tool(addr, &dir, true).await;
    gated(&mut whole).await;
    gated(&mut patched).await;
    let (mut client, mut view) = person(addr, &factory).await;
    let (_code, id, _device, _link, _socket) = crate::remote::waiting_device(&mut client, &mut view, "Phone").await;
    view.follow(&mut client, |view| pending(view).len() == 1).await;
    for agent in [&mut whole, &mut patched] {
        agent
            .follow(|workspace| {
                workspace.remote.as_ref().is_some_and(|remote| remote.pending_devices.iter().any(|device| device.device_id == id))
            })
            .await;
        let found = digits_and_codes(&agent.seen);
        assert!(found.is_empty(), "the agent was given:\n{}", found.join("\n"));
    }
    assert!(pending(&view).iter().all(|device| device.number.is_empty()), "the person's app was sent the digits: {:?}", pending(&view));
}

/// Path 6, G1 (A): a pipe at APPROVAL, on both sides: the person's app
/// shows its digits to compare; a tool connection's Workspaces carry none,
/// whole or patched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "#546 A"]
async fn a_tool_connection_is_never_sent_a_pipes_digits() {
    let (dir_a, dir_b) = (temp_dir(), temp_dir());
    let (addr_a, a) = start(&dir_a).await;
    let (addr_b, b) = start(&dir_b).await;
    let mut agents = Vec::new();
    for (addr, dir) in [(addr_a, &dir_a), (addr_b, &dir_b)] {
        for patches in [false, true] {
            let mut agent = Agent::tool(addr, dir, patches).await;
            gated(&mut agent).await;
            agents.push(agent);
        }
    }
    let (mut alice, mut alice_view) = person(addr_a, &a).await;
    let (mut bob, mut bob_view) = person(addr_b, &b).await;
    apply_request(&mut alice, &mut alice_view, crate::common::create_project("Site", &dir_a.join("site"))).await;
    apply_request(&mut bob, &mut bob_view, crate::common::create_project("Marketing", &dir_b.join("marketing"))).await;
    let project = |view: &View| view.workspace.projects.first().expect("the project").id.clone();
    let invite = v1::CreatePipeInvite {
        project_id: project(&alice_view),
        name: "Site ⇄ Marketing".into(),
        purpose: "testing".into(),
        ..Default::default()
    };
    let code = match apply_request(&mut alice, &mut alice_view, client_message::Body::CreatePipeInvite(invite)).await {
        Body::PipeInvite(invite) => invite.code,
        other => panic!("expected an invite, got {other:?}"),
    };
    let accept = v1::AcceptPipeInvite { code, project_id: project(&bob_view), purpose: "testing".into(), ..Default::default() };
    apply_request(&mut bob, &mut bob_view, client_message::Body::AcceptPipeInvite(accept)).await;
    let approval = |workspace: &v1::Workspace| workspace.pipes.iter().any(|pipe| pipe.state() == v1::PipeState::Approval);
    alice_view.follow(&mut alice, |view| approval(&view.workspace)).await;
    bob_view.follow(&mut bob, |view| approval(&view.workspace)).await;
    for view in [&alice_view, &bob_view] {
        assert!(
            view.workspace.pipes.iter().all(|pipe| pipe.digits.len() == 4),
            "the person compares the digits: {:?}",
            view.workspace.pipes
        );
    }
    for agent in &mut agents {
        agent.follow(approval).await;
        let found = digits_and_codes(&agent.seen);
        assert!(found.is_empty(), "the agent was given:\n{}", found.join("\n"));
    }
}

/// Path 14, G1 (A): a move at CONFIRM: the person's app shows its digits; a
/// tool connection's Workspaces carry none.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "#546 A"]
async fn a_tool_connection_is_never_sent_a_moves_digits() {
    let source = temp_dir();
    let (addr, _factory) = start(&source).await;
    let mut agents = Vec::new();
    for patches in [false, true] {
        let mut agent = Agent::tool(addr, &source, patches).await;
        gated(&mut agent).await;
        agents.push(agent);
    }
    let (mut client, _, _) = Client::connect(addr).await;

    let (code, receiving) = receiving().await;
    client.ok(client_message::Body::StartMove(v1::StartMove { code: code.text() })).await;
    let confirm = |workspace: &v1::Workspace| workspace.r#move.as_ref().is_some_and(|seen| seen.phase() == v1::MovePhase::Confirm);
    let shown = client
        .wait_for(|body| match body {
            Body::Workspace(workspace) if confirm(workspace) => workspace.r#move.clone(),
            _ => None,
        })
        .await;
    assert!(!shown.digits.is_empty(), "the person compares the digits: {shown:?}");
    for agent in &mut agents {
        agent.follow(confirm).await;
        let found = digits_and_codes(&agent.seen);
        assert!(found.is_empty(), "the agent was given:\n{}", found.join("\n"));
    }
    client.ok(client_message::Body::CancelMove(v1::CancelMove::default())).await;
    receiving.abort();
}

/// 64 hex digits standing alone in `bytes`: what a token looks like.
fn hex_tokens(bytes: &[u8]) -> Vec<String> {
    let mut found = Vec::new();
    let mut run = 0;
    for (at, byte) in bytes.iter().chain(std::iter::once(&b' ')).enumerate() {
        if byte.is_ascii_hexdigit() {
            run += 1;
            continue;
        }
        if run == 64 {
            found.push(String::from_utf8_lossy(&bytes[at - 64..at]).into_owned());
        }
        run = 0;
    }
    found
}

/// The t= values of every pairing-code-shaped string in `bytes`
/// (factory://?t=…, claktory://…?t=…, a link's #t=), whatever they look like.
fn code_tokens(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut found = Vec::new();
    for scheme in ["factory://", "claktory://", "/pair#"] {
        for (at, _) in text.match_indices(scheme) {
            let rest = &text[at..];
            let end = rest.find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '\0').unwrap_or(rest.len());
            for part in rest[..end].split(['?', '&', '#']) {
                if let Some(token) = part.strip_prefix("t=").filter(|token| !token.is_empty()) {
                    found.push(token.to_owned());
                }
            }
        }
    }
    found
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => files_under(&path, out),
            Ok(kind) if kind.is_file() => out.push(path),
            _ => {}
        }
    }
}

/// Path 1 (A): once Claktory has started, paired a device, has a pairing
/// waiting and made a pipe invite, no file under its home holds a token
/// that opens a person's connection on the loopback port: every 64-hex
/// string and every pairing code's token in every file (factory.db too) is
/// tried as Hello.token, and any it welcomes must be refused ApproveDevice
/// as a Tool connection is. Today remote/local-pairing's does.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "#546 A"]
async fn no_file_under_home_opens_a_persons_connection() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, mut view) = person(addr, &factory).await;
    // A device paired, one waiting, and a pipe invite out.
    let (code, id, device, _link, mut socket) = crate::remote::waiting_device(&mut client, &mut view, "Phone").await;
    let right = crate::remote::number(&code, &device.id().to_string());
    let allow = client_message::Body::ApproveDevice(v1::ApproveDevice { device_id: id, number: right, ..Default::default() });
    assert!(matches!(apply_request(&mut client, &mut view, allow).await, Body::Ack(_)));
    assert!(matches!(crate::remote::next(&mut socket).await, Some(Body::Welcome(_))));
    let (_code, _id, _device, _link, _waiting) = crate::remote::waiting_device(&mut client, &mut view, "Tablet").await;
    apply_request(&mut client, &mut view, crate::common::create_project("Site", &dir.join("site"))).await;
    let project = view.workspace.projects.first().expect("the project").id.clone();
    let invite = v1::CreatePipeInvite { project_id: project, name: "Out".into(), purpose: "testing".into(), ..Default::default() };
    assert!(matches!(apply_request(&mut client, &mut view, client_message::Body::CreatePipeInvite(invite)).await, Body::PipeInvite(_)));

    let mut files = Vec::new();
    files_under(home(&dir).root(), &mut files);
    let mut tried: Vec<(String, PathBuf)> = Vec::new();
    for file in &files {
        let Ok(bytes) = std::fs::read(file) else { continue };
        for token in hex_tokens(&bytes).into_iter().chain(code_tokens(&bytes)) {
            if !tried.iter().any(|(seen, _)| *seen == token) {
                tried.push((token, file.clone()));
            }
        }
    }
    assert!(!tried.is_empty(), "nothing token-shaped under home: the scan found nothing to try");
    let mut opened = Vec::new();
    for (token, file) in &tried {
        let Ok(mut agent) = Agent::hello(addr, token, false).await else { continue };
        let answer = agent.request(client_message::Body::ApproveDevice(v1::ApproveDevice::default())).await;
        if !person_only(&answer) {
            opened.push(format!("{} (answered {answer:?})", file.strip_prefix(home(&dir).root()).unwrap_or(file).display()));
        }
    }
    assert!(opened.is_empty(), "a file under home opens a person's connection:\n{}", opened.join("\n"));
}

/// A5 (A): the Mac app's socket gives person authority only to Claktory's
/// own signed app. This test's binary is signed ad hoc by the linker, so it
/// never gets Welcome there: turned away before it, or closed. (Another
/// user's connection, refused too, is Abbacchio's to try in a sandbox.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "#546 A"]
async fn the_app_socket_turns_away_a_binary_that_isnt_claktorys_app() {
    let dir = temp_dir();
    let (_addr, _factory) = start(&dir).await;
    let path = app_socket(&dir);
    let stream = match tokio::net::UnixStream::connect(&path).await {
        Ok(stream) => stream,
        // No local person authority at all on Linux (agents-and-pairing.md, A).
        Err(_) if cfg!(target_os = "linux") => return,
        Err(error) => panic!("no app socket at {}: {error}", path.display()),
    };
    let Ok((mut socket, _)) = tokio_tungstenite::client_async("ws://factory/", stream).await else { return };
    let hello = v1::Hello { protocol_version: PROTOCOL_VERSION, client_version: "not the app".into(), ..Default::default() };
    let message = ClientMessage { id: 1, body: Some(client_message::Body::Hello(hello)) };
    if socket.send(Frame::Binary(message.encode_to_vec().into())).await.is_err() {
        return;
    }
    match receive(&mut socket).await.and_then(|message| message.body) {
        None => {}
        Some(Body::Error(error)) if error.code() == ErrorCode::Unauthorized => {}
        other => panic!("the app socket let in a binary signed ad hoc: {other:?}"),
    }
}

/// A6 (A): the loopback port turns away, before Welcome, a Hello with no
/// token, a wrong one, and the person token a Claktory before A left in
/// remote/local-pairing; that file is gone once A has started.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "#546 A"]
async fn the_loopback_port_turns_away_no_token_a_wrong_one_and_the_old_person_token() {
    use sha2::Digest as _;
    let dir = temp_dir();
    // What a Claktory before A left: this Mac's app's pairing code and its
    // device.
    let old = "5".repeat(64);
    let hash: String = sha2::Sha256::digest(old.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect();
    let remote = home(&dir).root().join("remote");
    std::fs::create_dir_all(&remote).expect("remote folder");
    let local = serde_json::json!([{"id": "local", "name": "Old Mac", "token_sha256": hash, "created": 1, "local": true}]);
    std::fs::write(remote.join("devices.json"), local.to_string()).expect("devices.json");
    std::fs::write(remote.join("local-pairing"), format!("factory://?t={old}&name=Old%20Mac\n")).expect("local-pairing");

    let (addr, _factory) = start(&dir).await;
    assert!(!remote.join("local-pairing").exists(), "remote/local-pairing is still there");
    for (case, token) in [("no token", String::new()), ("a wrong token", "a".repeat(64)), ("the old person token", old)] {
        match Agent::hello(addr, &token, false).await {
            Err(Some(Body::Error(error))) if error.code() == ErrorCode::Unauthorized => {}
            Err(other) => panic!("{case}: expected UNAUTHORIZED before Welcome, got {other:?}"),
            Ok(agent) => panic!("{case}: welcomed as {}", agent.device_id),
        }
    }
}

/// The `factoryd` binary run as an agent's command, against the test's
/// Claktory: its home and port, never the person's.
fn factoryd_command(dir: &Path, addr: SocketAddr, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_factoryd"))
        .args(args)
        .env("HOME", dir)
        .env("FACTORY_HOME", home(dir).root())
        .env("FACTORY_PORT", addr.port().to_string())
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run factoryd")
}

/// What a command printed that's shaped like a code.
fn printed_codes(output: &std::process::Output) -> Vec<String> {
    let mut text = output.stdout.clone();
    text.extend_from_slice(&output.stderr);
    let mut found: Vec<String> = hex_tokens(&text).into_iter().chain(code_tokens(&text)).collect();
    for shape in ["factory://", "claktory://", "/pair#"] {
        if String::from_utf8_lossy(&text).contains(shape) {
            found.push(shape.to_owned());
        }
    }
    found
}

/// Its JSON line (the last line of stdout).
fn json_line(output: &std::process::Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().rev().find(|line| line.trim_start().starts_with('{')).unwrap_or_else(|| panic!("no JSON line in {stdout:?}"));
    serde_json::from_str(line).unwrap_or_else(|error| panic!("{line}: {error}"))
}

/// Every key anywhere in `value`.
fn keys_in(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                out.push(key.clone());
                keys_in(value, out);
            }
        }
        serde_json::Value::Array(items) => items.iter().for_each(|item| keys_in(item, out)),
        _ => {}
    }
}

/// A command refused as the person's: exit 3, and with --json the error
/// word "person_only" (Mista 42276); never its sentence.
fn refused_as_persons(case: &str, output: &std::process::Output, json: bool) {
    assert_eq!(output.status.code(), Some(3), "{case}: {}", String::from_utf8_lossy(&output.stdout));
    if json {
        assert_eq!(json_line(output)["error"], "person_only", "{case}");
    }
}

/// `factoryd` with a terminal, as an agent makes one with `script`.
fn in_terminal(dir: &Path, addr: SocketAddr, args: &[&str]) -> std::process::Output {
    let factoryd = env!("CARGO_BIN_EXE_factoryd");
    let mut script = std::process::Command::new("script");
    if cfg!(target_os = "macos") {
        script.args(["-q", "/dev/null", factoryd]).args(args);
    } else {
        script.args(["-qc", &format!("{factoryd} {}", args.join(" ")), "/dev/null"]);
    }
    script
        .env("HOME", dir)
        .env("FACTORY_HOME", home(dir).root())
        .env("FACTORY_PORT", addr.port().to_string())
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run script")
}

/// A loopback port nothing listens on.
fn closed_port() -> SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.local_addr().expect("its address")
}

async fn run(dir: &Path, addr: SocketAddr, args: &[&str], terminal: bool) -> std::process::Output {
    let (dir, args) = (dir.to_owned(), args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>());
    tokio::task::spawn_blocking(move || {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        if terminal { in_terminal(&dir, addr, &args) } else { factoryd_command(&dir, addr, &args) }
    })
    .await
    .expect("ran")
}

/// Path 2 (B): `factoryd pair`, in every form an agent can run it, prints no
/// code: it asks the running backend (RequestAddDevice), which raises the
/// Add a device card on the person's devices. Its JSON line has the
/// device's id and when it ends, and no code or link.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "#546 B"]
async fn factoryd_pair_prints_no_code() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let mut agent = Agent::tool(addr, &dir, false).await;
    gated(&mut agent).await;
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    for args in [&["pair", "Phone", "--no-wait", "--json"][..], &["pair", "Phone", "--json"], &["pair", "--no-wait"], &["pair", "Phone"]] {
        let case = format!("factoryd {}", args.join(" "));
        let output = run(&dir, addr, args, false).await;
        let found = printed_codes(&output);
        assert!(found.is_empty(), "{case}: printed {found:?}");
        assert_eq!(output.status.code(), Some(0), "{case}: {}", String::from_utf8_lossy(&output.stdout));
        if args.contains(&"--json") {
            let line = json_line(&output);
            assert!(line["device_id"].as_str().is_some_and(|id| !id.is_empty()) && line["expires_ms"].as_i64().is_some(), "{case}: {line}");
            let mut keys = Vec::new();
            keys_in(&line, &mut keys);
            let shaped: Vec<&String> =
                keys.iter().filter(|key| key.contains("code") || key.contains("link") || key.contains("token")).collect();
            assert!(shaped.is_empty(), "{case}: {shaped:?} in {line}");
        }
    }
    // It never got a code itself either.
    assert!(!agent.replies.iter().any(|reply| matches!(reply, Body::PairingCode(_))));
}

/// Path 2 (B): with no Claktory running, `factoryd pair` makes no code of
/// its own: exit 4 "not_running", nothing printed shaped like a code, and
/// devices.json untouched (the offline path is gone).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "#546 B"]
async fn factoryd_pair_with_no_claktory_running_makes_no_code() {
    let dir = temp_dir();
    let remote = home(&dir).root().join("remote");
    std::fs::create_dir_all(&remote).expect("remote folder");
    let addr = closed_port();
    for args in [&["pair", "Phone"][..], &["pair", "Phone", "--json"]] {
        let case = format!("factoryd {}", args.join(" "));
        let output = run(&dir, addr, args, false).await;
        assert!(printed_codes(&output).is_empty(), "{case}: printed {:?}", printed_codes(&output));
        assert_eq!(output.status.code(), Some(4), "{case}: {}", String::from_utf8_lossy(&output.stdout));
        if args.contains(&"--json") {
            assert_eq!(json_line(&output)["error"], "not_running", "{case}");
        }
        assert!(!remote.join("devices.json").exists(), "{case}: devices.json was written");
    }
}

/// Paths 3 and 5 (B): `allow`, `deny` and `unpair` from an agent's shell
/// change nothing, with a terminal (`script`) or without, with the right
/// digits or not: exit 3 "person_only", the waiting device still waits and
/// the paired one stays. `devices` still lists them, without the digits.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "#546 B"]
async fn factoryd_device_commands_from_an_agent_change_nothing() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let mut agent = Agent::tool(addr, &dir, false).await;
    gated(&mut agent).await;
    let (mut client, mut view) = person(addr, &factory).await;
    let (code, paired, device, _link, mut socket) = crate::remote::waiting_device(&mut client, &mut view, "Phone").await;
    let right = crate::remote::number(&code, &device.id().to_string());
    let allow = client_message::Body::ApproveDevice(v1::ApproveDevice { device_id: paired.clone(), number: right, ..Default::default() });
    assert!(matches!(apply_request(&mut client, &mut view, allow).await, Body::Ack(_)));
    assert!(matches!(crate::remote::next(&mut socket).await, Some(Body::Welcome(_))));
    let (code, waiting, device, _link, _waiting_socket) = crate::remote::waiting_device(&mut client, &mut view, "Tablet").await;
    let right = crate::remote::number(&code, &device.id().to_string());

    let commands: Vec<(Vec<&str>, bool)> = vec![
        (vec!["allow", &waiting, "--number", &right, "--json"], false),
        (vec!["allow", &waiting, "--number", &right], true),
        (vec!["allow", &waiting, "--json"], false),
        (vec!["deny", &waiting, "--json"], false),
        (vec!["deny", &waiting], false),
        (vec!["unpair", &paired, "--json"], false),
        (vec!["unpair", &paired], false),
        (vec!["unpair", &paired], true),
    ];
    for (args, terminal) in &commands {
        let case = format!("factoryd {}{}", args.join(" "), if *terminal { " (in a terminal)" } else { "" });
        let output = run(&dir, addr, args, *terminal).await;
        // `script` may not pass the exit on: its JSON or its effect tells.
        if !*terminal {
            refused_as_persons(&case, &output, args.contains(&"--json"));
        }
    }
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    assert_eq!(pending(&view).iter().map(|device| device.device_id.as_str()).collect::<Vec<_>>(), [waiting.as_str()], "still waiting");
    let devices = view.workspace.remote.as_ref().map(|remote| remote.devices.clone()).unwrap_or_default();
    assert!(devices.iter().any(|device| device.id == paired), "the paired device was removed: {devices:?}");

    for args in [&["devices", "--json"][..], &["devices"]] {
        let case = format!("factoryd {}", args.join(" "));
        let output = run(&dir, addr, args, false).await;
        assert_eq!(output.status.code(), Some(0), "{case}");
        let text = String::from_utf8_lossy(&output.stdout);
        let words: Vec<&str> = text.split(|c: char| !c.is_ascii_digit()).collect();
        assert!(!words.contains(&right.as_str()), "{case}: printed the waiting device's digits: {text}");
        if args.contains(&"--json") {
            let mut keys = Vec::new();
            keys_in(&json_line(&output), &mut keys);
            assert!(!keys.iter().any(|key| key == "number" || key == "digits"), "{case}: {keys:?}");
        }
    }
}

/// Path 5 (B): with no Claktory running, `deny` and `unpair` write nothing
/// themselves: exit 4 "not_running", devices.json as it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "#546 B"]
async fn factoryd_device_commands_with_no_claktory_running_write_nothing() {
    use sha2::Digest as _;
    let dir = temp_dir();
    let remote = home(&dir).root().join("remote");
    std::fs::create_dir_all(&remote).expect("remote folder");
    let hash: String = sha2::Sha256::digest(b"phone").iter().map(|byte| format!("{byte:02x}")).collect();
    let devices = serde_json::json!([{"id": "phone", "name": "Phone", "token_sha256": hash, "created": 1, "endpoint_id": "ab".repeat(32)}]);
    std::fs::write(remote.join("devices.json"), devices.to_string()).expect("devices.json");
    let before = std::fs::read(remote.join("devices.json")).expect("read");
    let addr = closed_port();
    for args in
        [&["deny", "phone"][..], &["deny", "phone", "--json"], &["unpair", "phone"], &["unpair", "phone", "--json"], &["allow", "phone"]]
    {
        let case = format!("factoryd {}", args.join(" "));
        let output = run(&dir, addr, args, false).await;
        assert_eq!(output.status.code(), Some(4), "{case}: {}", String::from_utf8_lossy(&output.stdout));
        if args.contains(&"--json") {
            assert_eq!(json_line(&output)["error"], "not_running", "{case}");
        }
        assert_eq!(std::fs::read(remote.join("devices.json")).expect("read"), before, "{case}: devices.json changed");
    }
}

/// `factoryd receive` on the other computer, in this process, whose person
/// never answers: its first code, and the task to abort.
async fn receiving() -> (factoryd::move_wire::ReceiveCode, tokio::task::JoinHandle<()>) {
    use factoryd::move_import::receive::{Options, Ui};
    let target = temp_dir();
    let there = factoryd::home::Home::new(target.join("home"));
    let (code_tx, mut codes) = tokio::sync::mpsc::unbounded_channel();
    let ui = Ui {
        code: std::sync::Arc::new(move |code| drop(code_tx.send(code.clone()))),
        say: std::sync::Arc::new(|_: &str| {}),
        confirm: std::sync::Arc::new(|_| -> factoryd::move_import::receive::Answer { Box::pin(std::future::pending::<bool>()) }),
    };
    let task = tokio::spawn(async move {
        let _keep = target;
        let options = Options { committed: false, local_only: true };
        let nowhere = SocketAddr::from(([127, 0, 0, 1], 1));
        let _ = factoryd::move_import::receive::receive(&there, nowhere, &options, &ui).await;
    });
    let code = tokio::time::timeout(WAIT, codes.recv()).await.expect("a code in time").expect("a code");
    (code, task)
}

/// Path 14 (moves): starting a move and confirming it are the person's: a
/// Tool connection's StartMove (with a live receive code) and ConfirmMove
/// are refused, and so is `factoryd move <code>` run as an agent; no move
/// starts (server-authorize.md, Moves 2).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "#546 moves"]
async fn a_tool_connection_cant_start_or_confirm_a_move() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let mut agent = Agent::tool(addr, &dir, false).await;
    gated(&mut agent).await;
    let (code, task) = receiving().await;
    for request in [
        client_message::Body::StartMove(v1::StartMove { code: code.text() }),
        client_message::Body::ConfirmMove(v1::ConfirmMove::default()),
    ] {
        let name = name(&request);
        assert!(!tool_allowed(&request), "{name}: on the Tool allowlist");
        let answer = agent.request(request).await;
        assert!(person_only(&answer), "{name}: {answer:?}");
    }
    let output = run(&dir, addr, &["move", &code.text(), "--json"], false).await;
    refused_as_persons("factoryd move", &output, true);
    agent.request(client_message::Body::UpdateSettings(v1::UpdateSettings::default())).await;
    assert!(agent.workspace.r#move.is_none(), "a move started: {:?}", agent.workspace.r#move);
    task.abort();
}

/// factory.proto, read as text: each message's fields, and which names are
/// enums. Enough of the language for the sweep: messages, enums, oneofs,
/// maps, labels; comments, options and reserved lines skipped.
struct Proto {
    messages: std::collections::HashMap<String, Vec<Field>>,
    enums: std::collections::HashSet<String>,
}

#[derive(Clone, Debug)]
struct Field {
    message: String,
    name: String,
    number: u32,
    /// Its type's last name: "string", "uint32", "Workspace".
    ty: String,
}

const PROTO: &str = include_str!("../../../../proto/factory/v1/factory.proto");

fn proto() -> &'static Proto {
    static PARSED: std::sync::OnceLock<Proto> = std::sync::OnceLock::new();
    PARSED.get_or_init(|| Proto::parse(PROTO))
}

impl Proto {
    fn parse(text: &str) -> Proto {
        let mut uncommented = String::new();
        for line in text.lines() {
            uncommented.push_str(line.split("//").next().unwrap_or_default());
            uncommented.push('\n');
        }
        let spaced: String = uncommented.chars().flat_map(|c| if "{};=<>,[]".contains(c) { vec![' ', c, ' '] } else { vec![c] }).collect();
        let tokens: Vec<&str> = spaced.split_whitespace().collect();
        let mut proto = Proto { messages: Default::default(), enums: Default::default() };
        // (kind, name) of each open block.
        let mut open: Vec<(&str, String)> = Vec::new();
        let mut at = 0;
        let skip_statement = |at: &mut usize| {
            let mut depth = 0;
            while *at < tokens.len() {
                match tokens[*at] {
                    "[" => depth += 1,
                    "]" => depth -= 1,
                    ";" if depth == 0 => break,
                    _ => {}
                }
                *at += 1;
            }
            *at += 1;
        };
        while at < tokens.len() {
            let token = tokens[at];
            match token {
                "message" | "enum" | "oneof" if tokens.get(at + 2) == Some(&"{") => {
                    let name = tokens[at + 1].to_owned();
                    match token {
                        "message" => {
                            proto.messages.entry(name.clone()).or_default();
                        }
                        "enum" => {
                            proto.enums.insert(name.clone());
                        }
                        _ => {}
                    }
                    open.push((token, name));
                    at += 3;
                }
                "}" => {
                    open.pop();
                    at += 1;
                }
                "syntax" | "package" | "import" | "option" | "reserved" => skip_statement(&mut at),
                _ if open.last().is_some_and(|(kind, _)| *kind == "enum") => skip_statement(&mut at),
                _ => {
                    let message =
                        open.iter().rev().find(|(kind, _)| *kind == "message").map(|(_, name)| name.clone()).expect("a field in a message");
                    let mut start = at;
                    if matches!(token, "repeated" | "optional") {
                        start += 1;
                    }
                    let (ty, name_at) = if tokens[start] == "map" {
                        // map < K , V > name
                        (tokens[start + 4], start + 6)
                    } else {
                        (tokens[start], start + 1)
                    };
                    let name = tokens[name_at].to_owned();
                    assert_eq!(tokens[name_at + 1], "=", "{message}.{name}: a field");
                    let number = tokens[name_at + 2].parse().unwrap_or_else(|_| panic!("{message}.{name}: its number"));
                    let ty = ty.rsplit('.').next().unwrap_or(ty).to_owned();
                    proto.messages.get_mut(&message).expect("its message").push(Field { message, name, number, ty });
                    at = name_at + 3;
                    skip_statement(&mut at);
                }
            }
        }
        proto
    }

    fn fields(&self, message: &str) -> &[Field] {
        self.messages.get(message).map(Vec::as_slice).unwrap_or_else(|| panic!("no message {message} in factory.proto"))
    }

    /// Every string field reachable from `root`, through its messages.
    fn strings_under(&self, root: &str, out: &mut std::collections::BTreeSet<String>, seen: &mut std::collections::HashSet<String>) {
        if !seen.insert(root.to_owned()) {
            return;
        }
        let Some(fields) = self.messages.get(root) else { return };
        for field in fields {
            if field.ty == "string" {
                out.insert(format!("{}.{}", field.message, field.name));
            } else if self.messages.contains_key(&field.ty) {
                self.strings_under(&field.ty, out, seen);
            }
        }
    }
}

impl Field {
    /// Its message, encoded with only this field set to something.
    fn sample(&self) -> Vec<u8> {
        use prost::encoding::{WireType, encode_key};
        let mut bytes = Vec::new();
        match self.ty.as_str() {
            "string" | "bytes" => {
                encode_key(self.number, WireType::LengthDelimited, &mut bytes);
                bytes.extend_from_slice(&[1, b'x']);
            }
            "double" | "fixed64" | "sfixed64" => {
                encode_key(self.number, WireType::SixtyFourBit, &mut bytes);
                bytes.extend_from_slice(&1.0_f64.to_le_bytes());
            }
            "float" | "fixed32" | "sfixed32" => {
                encode_key(self.number, WireType::ThirtyTwoBit, &mut bytes);
                bytes.extend_from_slice(&1.0_f32.to_le_bytes());
            }
            ty if proto().messages.contains_key(ty) => {
                encode_key(self.number, WireType::LengthDelimited, &mut bytes);
                bytes.push(0);
            }
            // bool, the integers and enums.
            _ => {
                encode_key(self.number, WireType::Varint, &mut bytes);
                bytes.push(1);
            }
        }
        bytes
    }
}

/// The numbers of the fields present in an encoded message.
fn fields_set(mut bytes: &[u8]) -> Vec<u32> {
    let mut numbers = Vec::new();
    while !bytes.is_empty() {
        let (number, wire) = prost::encoding::decode_key(&mut bytes).expect("a key");
        prost::encoding::skip_field(wire, number, &mut bytes, prost::encoding::DecodeContext::default()).expect("a field");
        numbers.push(number);
    }
    numbers
}

/// How a string field shaped like a code, a key, digits or a value reaches
/// a connection.
enum How {
    /// Only in replies to these requests, each refused to a Tool connection.
    Reply(&'static [&'static str]),
    /// In the Workspace: left out of a Tool connection's
    /// (`digits_and_codes`), or out of everyone's.
    Stripped,
    /// Not a code: why.
    Not(&'static str),
}

/// Every string field under a ServerMessage body whose name is shaped like
/// a code, and how it's kept from agents (Fugo 42141). A new one fails the
/// sweep until it's added here.
const SHAPED: &[(&str, How)] = &[
    ("PairingCode.code", How::Reply(&["CreatePairing", "ShowPairingCode"])),
    ("PairingCode.link", How::Reply(&["CreatePairing", "ShowPairingCode"])),
    ("PipeInvite.code", How::Reply(&["CreatePipeInvite"])),
    ("RecoveryKey.key", How::Reply(&["ShowRecoveryKey", "NewRecoveryKey"])),
    ("SecretEntry.value", How::Reply(&["RevealSecret"])),
    ("ConnectStarted.url", How::Reply(&["StartConnect"])),
    ("ConnectStarted.user_code", How::Reply(&["StartConnect"])),
    ("PipeShareOpened.url", How::Reply(&["OpenPipeShare"])),
    ("PipeShareOpened.fallback_url", How::Reply(&["OpenPipeShare"])),
    ("PendingDevice.number", How::Stripped),
    ("Pipe.digits", How::Stripped),
    ("Move.digits", How::Stripped),
    ("LoginFlow.user_code", How::Stripped),
    ("PipeInvitePreview.code", How::Not("the code the client itself sent ReadPipeInvite, given back")),
    ("LoginFlow.url", How::Not("the provider's sign-in page")),
    ("LoginFlow.verification_url", How::Not("the provider's page for the code, without it")),
    ("Connector.setup_url", How::Not("a service's page to read")),
    ("Connector.token_url", How::Not("a service's page that makes a token, without one")),
    ("UsageResetAnswer.fallback_url", How::Not("a provider's page")),
    ("CacheTtl.key", How::Not("the name of a cache setting")),
    ("OptionChoice.value", How::Not("an app setting's choice")),
    ("MapBadge.key", How::Not("a map badge's id")),
    ("AppInfo.publisher_key", How::Not("an app publisher's public key")),
    ("AppPermissions.publisher_key", How::Not("an app publisher's public key")),
    ("ClaktoryAccount.code_from", How::Not("the address the emailed code comes from")),
    ("Transcription.error_code", How::Not("why a recording wasn't transcribed, in a word")),
    ("PipeReport.refused_code", How::Not("why a report was refused, in a word")),
    ("SecretRequest.secret_id", How::Not("a secret's id")),
    ("SecretRequest.secret_name", How::Not("a secret's name")),
    ("SecretRequest.given_secret_id", How::Not("a secret's id")),
    ("SecretRequest.given_secret_name", How::Not("a secret's name")),
    ("SecretRequest.fitting_secret_id", How::Not("a secret's id")),
    ("SecretRequest.fitting_secret_name", How::Not("a secret's name")),
    ("SecretRequest.public_key", How::Not("the public half of a generated ssh key")),
    ("SecretRequest.link", How::Not("the service's page for making the secret")),
    ("SecretRequest.link_host", How::Not("that page's host")),
    ("SecretApproval.secret_names", How::Not("secrets' names")),
    ("ConnectProposal.secret_id", How::Not("a secret's id")),
    ("ConnectProposal.secret_name", How::Not("a secret's name")),
    ("ConnectFlow.secret_id", How::Not("a secret's id")),
    ("ConnectChoices.secret_id", How::Not("a secret's id")),
    ("SecretUse.secret_id", How::Not("a secret's id")),
    ("SecretUse.secret_name", How::Not("a secret's name")),
];

/// Whether a field's name is shaped like a code, a key, digits, a token or
/// a value: one of its words, split at _, is.
fn shaped(field: &str) -> bool {
    const WORDS: &[&str] =
        &["code", "token", "digits", "number", "link", "secret", "passphrase", "key", "password", "pairing", "url", "value", "values"];
    let name = field.rsplit('.').next().unwrap_or(field);
    name.split('_').any(|word| WORDS.contains(&word))
}

/// The structural sweep (Fugo 42141), the second check beside A1: every
/// string field any ServerMessage body can carry whose name is shaped like
/// a code is classified in SHAPED, and each one only a reply carries comes
/// only from requests refused to a Tool connection; each one stripped from
/// a Tool connection's Workspace is one `digits_and_codes` looks for. A new
/// field, or a classified one gone, fails it.
#[test]
#[ignore = "#546 A"]
fn every_code_shaped_field_an_agent_could_be_sent_is_accounted_for() {
    let proto = proto();
    let mut found = std::collections::BTreeSet::new();
    let mut seen = std::collections::HashSet::new();
    let bodies: Vec<&Field> = proto.fields("ServerMessage").iter().filter(|field| proto.messages.contains_key(&field.ty)).collect();
    assert!(bodies.len() > 50, "{} ServerMessage bodies", bodies.len());
    for body in bodies {
        proto.strings_under(&body.ty, &mut found, &mut seen);
    }
    let found: Vec<String> = found.into_iter().filter(|field| shaped(field)).collect();
    let unclassified: Vec<&String> = found.iter().filter(|field| !SHAPED.iter().any(|(known, _)| known == field)).collect();
    assert!(unclassified.is_empty(), "string fields shaped like codes that reach connections, not classified in SHAPED: {unclassified:?}");
    let gone: Vec<&str> = SHAPED.iter().map(|(known, _)| *known).filter(|known| !found.iter().any(|field| field == known)).collect();
    assert!(gone.is_empty(), "classified in SHAPED but no longer sent (or renamed): {gone:?}");

    let requests = every_request();
    let mut wrong = Vec::new();
    for (field, how) in SHAPED {
        match how {
            How::Reply(names) => {
                for wanted in *names {
                    match requests.iter().find(|(_, body)| name(body) == *wanted) {
                        Some((_, body)) if tool_allowed(body) => wrong.push(format!("{field}: {wanted} is on the Tool allowlist")),
                        Some(_) => {}
                        None => wrong.push(format!("{field}: {wanted} isn't a request")),
                    }
                }
            }
            How::Stripped => {
                let checked = ["PendingDevice.number", "Pipe.digits", "Move.digits", "LoginFlow.user_code"];
                if !checked.contains(field) {
                    wrong.push(format!("{field}: digits_and_codes doesn't look for it"));
                }
            }
            How::Not(why) => assert!(!why.is_empty()),
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// #546 C: redact and burn (Secco 42356, Mista 42294, Fugo 42291, 42367). A
// code that reaches an agent-visible place is masked there before the
// model or the disk sees it, and one of this Claktory's own live codes is
// dead by the time the masked text appears. The person's typed message is
// masked before the scripted model reads it, so codes reach tools through
// files the test writes, never through the "call …" line.

/// What a mask reads instead of a code (Secco 42356).
const PAIRING_MASK: &str = "[pairing code]";
const PIPE_MASK: &str = "[pipe invite]";
const MOVE_MASK: &str = "[move code]";

/// A project "Site" with Ana and Bo on the scripted model, and the person's
/// connection with remote access through a relay that leads nowhere.
async fn site() -> (PathBuf, SocketAddr, std::sync::Arc<factoryd::Factory>, Client, View) {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, mut view) = person(addr, &factory).await;
    apply_request(&mut client, &mut view, crate::common::create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, crate::common::create_agent("site", "Ana", "")).await;
    apply_request(&mut client, &mut view, crate::common::create_agent("site", "Bo", "")).await;
    (dir, addr, factory, client, view)
}

/// A new pairing code of this Claktory, as the person's app makes one.
async fn pairing(client: &mut Client, view: &mut View, name: &str) -> v1::PairingCode {
    let pairing = client_message::Body::CreatePairing(v1::CreatePairing { name: name.into(), host: String::new() });
    match apply_request(client, view, pairing).await {
        Body::PairingCode(code) => code,
        other => panic!("expected a pairing code, got {other:?}"),
    }
}

/// The ways a code could still be in `text`: the code, its token (in any
/// case), or its token with every space and line break taken out.
fn holds(text: &str, code: &str) -> bool {
    let token = code_tokens(code.as_bytes()).pop().unwrap_or_else(|| code.to_owned());
    let squeezed: String = text.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase();
    text.contains(code) || squeezed.contains(&token.to_lowercase())
}

/// Everything under the agent's folder in Claktory's home (its transcript
/// as written, its task logs; not the person's uploads) and factory.db.
fn agent_files(dir: &Path, agent: &str) -> String {
    let mut files = Vec::new();
    files_under(&home(dir).root().join("agents").join(agent), &mut files);
    // The person's uploads stay exactly as sent (Secco 42356): not agent-visible text.
    let uploads = home(dir).root().join("agents").join(agent).join("uploads");
    files.retain(|file| !file.starts_with(&uploads));
    // factory.db (and its journal), where events and transcripts can live too (Fugo 42603).
    for name in ["factory.db", "factory.db-wal", "factory.db-journal"] {
        files.push(home(dir).root().join(name));
    }
    files
        .iter()
        .filter_map(|file| std::fs::read(file).ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every entry's text, as the person's app gets the transcript.
fn entries_text(view: &View) -> String {
    view.entries.iter().map(|entry| format!("{:?}", entry.kind)).collect::<Vec<_>>().join("\n")
}

/// The code is masked in what the person's app shows, in the agent's files
/// on disk, and in whatever else is given.
fn masked_everywhere(case: &str, dir: &Path, agent: &str, view: &View, code: &str, also: &[&str]) {
    assert!(!holds(&entries_text(view), code), "{case}: the transcript holds the code");
    assert!(!holds(&agent_files(dir, agent), code), "{case}: the agent's files on disk hold the code");
    for text in also {
        assert!(!holds(text, code), "{case}: {text}");
    }
}

/// A code is dead: a device dialing with it is turned away.
async fn dead(case: &str, code: &str) {
    let parsed = crate::remote::parse(code);
    let device = crate::remote::new_device().await;
    match crate::remote::dial(&device, &parsed, &parsed.addresses).await {
        Err(_) => {}
        Ok(connection) => {
            let mut socket = crate::remote::open(&connection).await;
            let answer = crate::remote::hello(&mut socket, &parsed.token).await;
            assert!(!matches!(answer, Body::PairingPending(_) | Body::Welcome(_)), "{case}: the code still works: {answer:?}");
        }
    }
}

/// A code still works: a device using it waits to be allowed.
async fn alive(case: &str, code: &str) {
    let parsed = crate::remote::parse(code);
    let device = crate::remote::new_device().await;
    let connection =
        crate::remote::dial(&device, &parsed, &parsed.addresses).await.unwrap_or_else(|why| panic!("{case}: turned away: {why}"));
    let mut socket = crate::remote::open(&connection).await;
    let answer = crate::remote::hello(&mut socket, &parsed.token).await;
    assert!(matches!(answer, Body::PairingPending(_)), "{case}: {answer:?}");
}

/// The person says `text` to `agent`; what the agent's turn added.
async fn say(client: &mut Client, view: &mut View, agent: &str, text: &str, attachments: Vec<v1::Attachment>) -> String {
    apply_request(client, view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })).await;
    let before = view.entries.len();
    let message = v1::SendMessage { agent_id: agent.into(), text: text.into(), attachments, ..Default::default() };
    apply_request(client, view, client_message::Body::SendMessage(message)).await;
    view.follow(client, |view| {
        view.entries.len() > before + 1
            && view.entries[before..].iter().any(|entry| matches!(entry.kind, Some(Kind::Assistant(_))))
            && crate::pipes::idle(view, agent)
    })
    .await;
    view.entries[before..].iter().map(|entry| format!("{:?}", entry.kind)).collect::<Vec<_>>().join("\n")
}

/// Claktory told the person in the conversation (an event from Claktory
/// since `from`), in whatever words Trish settles.
fn told(view: &View, from: usize) -> bool {
    view.entries[from..].iter().any(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.source() == v1::EventSource::Factory))
}

/// C1, C7: a pairing code the person types to an agent, as the code, its
/// link and the percent-encoded claktory:// form, is masked before the
/// model and the disk, the device waiting with it is turned away, each is
/// dead once the answer shows, and Claktory says so in the conversation.
/// The person's own card still shows a code that works.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "#546 C"]
async fn a_pairing_code_typed_to_an_agent_is_masked_and_dies() {
    let (dir, _addr, _factory, mut client, mut view) = site().await;
    let typed = pairing(&mut client, &mut view, "Phone").await;
    // A device waits with it: once it's seen, that device is turned away.
    let parsed = crate::remote::parse(&typed.code);
    let device = crate::remote::new_device().await;
    let connection = crate::remote::dial(&device, &parsed, &parsed.addresses).await.expect("a code waits: let in");
    let mut socket = crate::remote::open(&connection).await;
    assert!(matches!(crate::remote::hello(&mut socket, &parsed.token).await, Body::PairingPending(_)));

    let from = view.entries.len();
    let turn = say(&mut client, &mut view, "ana", &format!("Here's the code: {}", typed.code), Vec::new()).await;
    assert!(turn.contains(PAIRING_MASK), "the echo shows the mask: {turn}");
    masked_everywhere("typed", &dir, "ana", &view, &typed.code, &[&turn]);
    crate::remote::turned_away(&mut socket, "a device that used a code seen in a conversation").await;
    dead("typed", &typed.code).await;
    assert!(told(&view, from), "Claktory didn't say the code no longer works");

    // Its link, and the claktory:// form with the code percent-encoded.
    let linked = pairing(&mut client, &mut view, "Laptop").await;
    if !linked.link.is_empty() {
        let turn = say(&mut client, &mut view, "ana", &format!("open {}", linked.link), Vec::new()).await;
        masked_everywhere("the link", &dir, "ana", &view, &linked.link, &[&turn]);
        dead("the link", &linked.code).await;
    }
    let encoded = pairing(&mut client, &mut view, "Tablet").await;
    let percent: String = encoded
        .code
        .bytes()
        .map(|byte| if byte.is_ascii_alphanumeric() { (byte as char).to_string() } else { format!("%{byte:02X}") })
        .collect();
    let turn = say(&mut client, &mut view, "ana", &format!("open claktory://add?code={percent}"), Vec::new()).await;
    assert!(!turn.contains(&percent), "the percent-encoded code reached the model: {turn}");
    masked_everywhere("percent-encoded", &dir, "ana", &view, &encoded.code, &[&turn]);
    dead("percent-encoded", &encoded.code).await;

    // C7: the person's own way still works.
    alive("a code made after", &pairing(&mut client, &mut view, "Watch").await.code).await;
}

/// C1: a text file the person attaches holding a code: the file stays as
/// sent, the code is dead once the message is answered, and the agent's
/// read_file of it shows the mask.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "#546 C"]
async fn a_pairing_code_in_an_attached_file_dies_and_reads_masked() {
    let (dir, _addr, _factory, mut client, mut view) = site().await;
    let code = pairing(&mut client, &mut view, "Phone").await.code;
    let sent = format!("pair with {code}\n");
    let attachment = crate::uploads::upload(&mut client, "up-1", "ana", "notes.txt", sent.as_bytes(), "text/plain").await;
    say(&mut client, &mut view, "ana", "see the file", vec![attachment.clone()]).await;
    dead("attached", &code).await;
    let read = crate::pipes::call(
        &mut client,
        &mut view,
        "ana",
        "read_file",
        &serde_json::json!({"path": attachment.path, "offset": null, "limit": null}).to_string(),
    )
    .await;
    assert!(read.contains(PAIRING_MASK), "{read}");
    masked_everywhere("attached", &dir, "ana", &view, &code, &[&read]);
    assert_eq!(std::fs::read_to_string(&attachment.path).expect("the upload"), sent, "the person's file is kept as sent");
}

/// C2, C3: a code a command prints, or a file the agent reads holds, is
/// masked in the tool's result, the transcript and the task log, in the
/// foreground and in a background task, written in two pieces a second
/// apart, as uppercase hex or wrapped across lines; and it's dead.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "#546 C"]
async fn a_pairing_code_a_command_prints_or_a_file_holds_is_masked_and_dies() {
    let (dir, _addr, _factory, mut client, mut view) = site().await;
    let site = dir.join("site");
    let mut cases: Vec<(&str, String, String)> = Vec::new();
    for (case, script) in [
        ("cat", "cat code-1.txt"),
        ("in two pieces", "c=$(cat code-2.txt); printf '%s' \"${c:0:30}\"; sleep 1; printf '%s\\n' \"${c:30}\""),
        ("uppercase token", "c=$(cat code-3.txt); t=${c#*t=}; t=${t%%&*}; echo \"$t\" | tr a-f A-F"),
        ("wrapped token", "c=$(cat code-4.txt); t=${c#*t=}; t=${t%%&*}; printf '%s\\n%s\\n' \"${t:0:32}\" \"${t:32}\""),
    ] {
        let number = cases.len() + 1;
        let code = pairing(&mut client, &mut view, case).await.code;
        std::fs::write(site.join(format!("code-{number}.txt")), &code).expect("the code's file");
        cases.push((case, script.to_owned(), code));
    }
    for (case, script, code) in &cases {
        let output = crate::pipes::call(&mut client, &mut view, "ana", "bash", &serde_json::json!({"command": script}).to_string()).await;
        masked_everywhere(&format!("foreground, {case}"), &dir, "ana", &view, code, &[&output]);
        dead(case, code).await;
    }
    // The same in background tasks: their logs are written by the shell.
    let mut background = Vec::new();
    for (number, (case, script)) in [
        ("cat", "cat code-5.txt"),
        ("in two pieces", "c=$(cat code-6.txt); printf '%s' \"${c:0:30}\"; sleep 1; printf '%s\\n' \"${c:30}\""),
    ]
    .into_iter()
    .enumerate()
    {
        let code = pairing(&mut client, &mut view, case).await.code;
        std::fs::write(site.join(format!("code-{}.txt", number + 5)), &code).expect("the code's file");
        let started = crate::pipes::call(
            &mut client,
            &mut view,
            "ana",
            "bash",
            &serde_json::json!({"command": script, "run_in_background": true}).to_string(),
        )
        .await;
        assert!(started.starts_with("Started task-"), "{started}");
        background.push((case, code));
    }
    view.follow(&mut client, |view| {
        view.entries
            .iter()
            .filter(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.source() == v1::EventSource::Task))
            .count()
            >= 2
            && crate::pipes::idle(view, "ana")
    })
    .await;
    for (case, code) in &background {
        masked_everywhere(&format!("background, {case}"), &dir, "ana", &view, code, &[]);
        dead(case, code).await;
    }
    // C3: read_file.
    let code = pairing(&mut client, &mut view, "read").await.code;
    std::fs::write(site.join("notes.md"), format!("# Notes\n\npair with {code}\n")).expect("notes");
    let read = crate::pipes::call(&mut client, &mut view, "ana", "read_file", r#"{"path":"notes.md","offset":null,"limit":null}"#).await;
    assert!(read.contains(PAIRING_MASK), "{read}");
    masked_everywhere("read_file", &dir, "ana", &view, &code, &[&read]);
    dead("read_file", &code).await;
}

/// C4: a code in an event (a monitor check's output when it fires) is
/// masked in the event and dies.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "#546 C"]
async fn a_pairing_code_in_an_event_is_masked_and_dies() {
    let (dir, _addr, factory, mut client, mut view) = site().await;
    factory.set_monitor_tick(Duration::from_millis(20));
    let code = pairing(&mut client, &mut view, "Phone").await.code;
    std::fs::write(dir.join("site/code.txt"), &code).expect("the code's file");
    let add = serde_json::json!({"monitors": [{
        "name": "code", "prompt": "Look at it.", "max_fires": 1,
        "when": {"check": "cat code.txt", "every": "30s", "fire_on": "success"}
    }]});
    crate::pipes::call(&mut client, &mut view, "ana", "monitors_add", &add.to_string()).await;
    view.follow(&mut client, |view| {
        view.entries.iter().any(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.source() == v1::EventSource::Monitor))
            && crate::pipes::idle(view, "ana")
    })
    .await;
    masked_everywhere("a monitor's event", &dir, "ana", &view, &code, &[]);
    dead("a monitor's event", &code).await;
}

/// C5: a pipe invite code an agent reads is masked as one, and the invite is
/// called off.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "#546 C"]
async fn a_pipe_invite_an_agent_reads_is_masked_and_called_off() {
    let (dir, _addr, _factory, mut client, mut view) = site().await;
    let project = view.workspace.projects.first().expect("the project").id.clone();
    let invite = v1::CreatePipeInvite { project_id: project, name: "Out".into(), purpose: "testing".into(), ..Default::default() };
    let (pipe, code) = match apply_request(&mut client, &mut view, client_message::Body::CreatePipeInvite(invite)).await {
        Body::PipeInvite(invite) => (invite.pipe_id, invite.code),
        other => panic!("expected an invite, got {other:?}"),
    };
    std::fs::write(dir.join("site/invite.txt"), &code).expect("the invite's file");
    let output = crate::pipes::call(&mut client, &mut view, "ana", "bash", r#"{"command":"cat invite.txt"}"#).await;
    assert!(output.contains(PIPE_MASK), "{output}");
    assert!(!output.contains(&code) && !agent_files(&dir, "ana").contains(&code) && !entries_text(&view).contains(&code));
    view.follow(&mut client, |view| view.workspace.pipes.iter().all(|shown| shown.id != pipe || shown.state() != v1::PipeState::Invited))
        .await;
}

/// C6, C8: another Claktory's pairing code, and a move's receive code, are
/// masked as links but nothing is burned: the other Claktory's code still
/// works there, its bare token isn't masked here (only this Claktory's live
/// ones are), and the receive goes on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "#546 C"]
async fn another_claktorys_code_and_a_move_code_are_masked_and_left_alone() {
    let (dir, _addr, _factory, mut client, mut view) = site().await;
    let other_dir = temp_dir();
    let (other_addr, other) = start(&other_dir).await;
    let (mut other_client, mut other_view) = person(other_addr, &other).await;
    let theirs = pairing(&mut other_client, &mut other_view, "Phone").await.code;
    let token = code_tokens(theirs.as_bytes()).pop().expect("a token");
    std::fs::write(dir.join("site/theirs.txt"), &theirs).expect("their code");
    std::fs::write(dir.join("site/token.txt"), &token).expect("their token");
    let link = crate::pipes::call(&mut client, &mut view, "ana", "bash", r#"{"command":"cat theirs.txt"}"#).await;
    assert!(link.contains(PAIRING_MASK) && !link.contains(&theirs), "{link}");
    let bare = crate::pipes::call(&mut client, &mut view, "ana", "bash", r#"{"command":"cat token.txt"}"#).await;
    assert!(bare.contains(&token), "another Claktory's bare token is masked: every sha256 would be: {bare}");
    alive("another Claktory's code", &theirs).await;

    let (code, receiving) = receiving().await;
    std::fs::write(dir.join("site/move.txt"), code.text()).expect("the move code");
    let output = crate::pipes::call(&mut client, &mut view, "ana", "bash", r#"{"command":"cat move.txt"}"#).await;
    assert!(output.contains(MOVE_MASK) && !output.contains(&code.text()), "{output}");
    assert!(!agent_files(&dir, "ana").contains(&code.text()) && !entries_text(&view).contains(&code.text()));
    assert!(!receiving.is_finished(), "the receive stopped");
    receiving.abort();
}

// #546 D: a sandbox for agents' commands (Ghiaccio's d-sandbox.md rev 2,
// interface 42364; Fugo 42004, 42089, 42277, 42370). Every command an agent
// starts, and everything those start, can't read Claktory's keys, the
// clipboard or the app socket, or leave something that runs later outside
// the sandbox. The person's home is a parameter, set to a folder of the
// test's, so a run without D writes nothing to the real one; and every test
// first checks the agent's commands are contained (`contained`), so run
// where D isn't active it stops there.

// D's interface (Ghiaccio 42364). Until D lands each stand-in fails, saying
// what's missing, before anything acts; when it does, each becomes what its
// comment says.

/// D: `factoryd::sandbox::set_user_home_for_test(factory, home)`.
fn set_user_home(_factory: &factoryd::Factory, _home: &Path) {
    panic!("D isn't in yet: no sandbox::set_user_home_for_test");
}

/// D: `factoryd::sandbox::set_executable_for_test(factory, env!("CARGO_BIN_EXE_factoryd"))`.
fn set_sandbox_executable(_factory: &factoryd::Factory) {
    panic!("D isn't in yet: no sandbox::set_executable_for_test");
}

/// D: `factoryd::update::set_own_team_for_test(factory, Some("TEST123"))`.
fn set_own_team(_factory: &factoryd::Factory) {
    panic!("D isn't in yet: no update::set_own_team_for_test");
}

/// Turns "Contain agents' commands" on, from the person's connection.
/// D: `SetCommandContainment { on: true }`, answered with Ack.
async fn contain(_client: &mut Client, _view: &mut View) {
    panic!("D isn't in yet: no SetCommandContainment");
}

/// Claktory's files an agent's command must never read (Fugo 42277: the
/// account folder too), seeded where Claktory hasn't made them.
const DENIED_READS: &[&str] = &[
    "remote/iroh.key",
    "remote/devices.json",
    "account/claktory.key",
    "account/share.key",
    "account/account.json",
    "vault/auto.key",
    "vault/vault.db",
    "auth.json",
    "pipes/key",
];

/// A test Claktory with Site, Ana on the scripted model and the person's
/// connection, its person's home a folder of the test's, and D on.
async fn contained_site() -> (PathBuf, PathBuf, SocketAddr, std::sync::Arc<factoryd::Factory>, Client, View) {
    let (dir, addr, factory, mut client, mut view) = site().await;
    let user = dir.join("user");
    std::fs::create_dir_all(user.join("Library/LaunchAgents")).expect("the person's home");
    set_user_home(&factory, &user);
    set_sandbox_executable(&factory);
    for file in DENIED_READS {
        let path = home(&dir).root().join(file);
        if !path.exists() {
            std::fs::create_dir_all(path.parent().expect("a folder")).expect("its folder");
            std::fs::write(&path, "seeded by the test\n").expect("seed");
        }
    }
    contain(&mut client, &mut view).await;
    contained(&mut client, &mut view, &dir).await;
    (dir, user, addr, factory, client, view)
}

/// Runs `script` as Ana's foreground command; what it printed.
async fn ana_runs(client: &mut Client, view: &mut View, script: &str) -> String {
    crate::pipes::call(client, view, "ana", "bash", &serde_json::json!({"command": script}).to_string()).await
}

/// The agent's commands are contained: reading iroh.key fails. Checked
/// before anything else, so a run where D isn't active stops here.
async fn contained(client: &mut Client, view: &mut View, dir: &Path) {
    let key = home(dir).root().join("remote/iroh.key");
    let output = ana_runs(client, view, &format!("cat '{}' >/dev/null 2>&1 && echo READ-OK || echo READ-DENIED", key.display())).await;
    assert!(output.contains("READ-DENIED") && !output.contains("READ-OK"), "the agent's commands aren't contained: {output}");
}

/// A shell script that tries to read each path and prints "OK <path>" or
/// "DENIED <path>" per line.
fn read_script(paths: &[String]) -> String {
    paths.iter().map(|path| format!("if cat '{path}' >/dev/null 2>&1; then echo 'OK {path}'; else echo 'DENIED {path}'; fi\n")).collect()
}

/// Which of `paths` the result says were read.
fn read_ok(result: &str, paths: &[String]) -> Vec<String> {
    paths.iter().filter(|path| result.lines().any(|line| line == format!("OK {path}"))).cloned().collect()
}

/// D1: no command an agent starts reads Claktory's keys: in the foreground,
/// as a background task, as a monitor's check, double-forked with setsid,
/// or in a pty child; by their path, through /System/Volumes/Data, a
/// symlink or a `../` path. It can't hard-link one or connect to the app
/// socket. The tool token, config.json and its own folder stay readable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "D"]
async fn an_agents_commands_cant_read_claktorys_keys() {
    let (dir, _user, _addr, factory, mut client, mut view) = contained_site().await;
    factory.set_monitor_tick(Duration::from_millis(20));
    let root = home(&dir).root().to_path_buf();
    let mut denied: Vec<String> = DENIED_READS.iter().map(|file| root.join(file).display().to_string()).collect();
    if cfg!(target_os = "macos") {
        // By the firmlink's other name: the canonical path (/private/var/…), under the data volume.
        let real = std::fs::canonicalize(&root).expect("the home's real path");
        denied.extend(DENIED_READS.iter().take(3).map(|file| format!("/System/Volumes/Data{}", real.join(file).display())));
    }
    let link = dir.join("key-link");
    std::os::unix::fs::symlink(root.join("remote/iroh.key"), &link).expect("a symlink");
    denied.push(link.display().to_string());
    denied.push(format!("{}/../{}/remote/iroh.key", root.join("agents").display(), root.file_name().expect("its name").to_string_lossy()));
    let allowed: Vec<String> = ["tool-token", "config.json"]
        .iter()
        .map(|file| root.join(file).display().to_string())
        .chain([dir.join("site/check.sh").display().to_string()])
        .collect();
    let everything: Vec<String> = denied.iter().chain(&allowed).cloned().collect();
    std::fs::write(dir.join("site/check.sh"), read_script(&everything)).expect("the script");

    // Each way a command starts writes what it could read to a file of the
    // agent's own.
    ana_runs(&mut client, &mut view, "sh check.sh > from-foreground.txt").await;
    // A new session, double-forked (macOS has no setsid command).
    let setsid =
        r#"( python3 -c "import os; os.setsid(); os.execvp('sh', ['sh', 'check.sh'])" > from-setsid.txt 2>/dev/null & ) ; sleep 3"#;
    ana_runs(&mut client, &mut view, setsid).await;
    let pty = if cfg!(target_os = "macos") {
        "script -q /dev/null sh check.sh > from-pty.txt"
    } else {
        "script -qc 'sh check.sh' /dev/null > from-pty.txt"
    };
    ana_runs(&mut client, &mut view, pty).await;
    crate::pipes::call(
        &mut client,
        &mut view,
        "ana",
        "bash",
        r#"{"command":"sh check.sh > from-background.txt","run_in_background":true}"#,
    )
    .await;
    let add = serde_json::json!({"monitors": [{"name": "keys", "prompt": "Done.", "max_fires": 1, "when": {"check": "sh check.sh > from-monitor.txt", "every": "30s", "fire_on": "success"}}]});
    crate::pipes::call(&mut client, &mut view, "ana", "monitors_add", &add.to_string()).await;
    let ways = ["foreground", "setsid", "pty", "background", "monitor"];
    let results = |dir: &Path| {
        ways.iter().all(|way| {
            std::fs::read_to_string(dir.join(format!("site/from-{way}.txt"))).is_ok_and(|text| text.lines().count() >= everything.len())
        })
    };
    let waited = tokio::time::timeout(WAIT, async {
        while !results(&dir) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "not every way wrote its results");
    let mut wrong = Vec::new();
    for way in ways {
        let result = std::fs::read_to_string(dir.join(format!("site/from-{way}.txt"))).expect("results");
        for path in read_ok(&result, &denied) {
            wrong.push(format!("{way}: read {path}"));
        }
        for path in allowed.iter().filter(|path| !read_ok(&result, &allowed).contains(path)) {
            wrong.push(format!("{way}: couldn't read {path}, which it may"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));

    // No hard link to a key, and no connection to the app socket.
    let linked = ana_runs(
        &mut client,
        &mut view,
        &format!("ln '{}' /tmp/gyro-d-link-$$ && echo LINK-OK || echo LINK-DENIED", root.join("remote/iroh.key").display()),
    )
    .await;
    assert!(linked.contains("LINK-DENIED"), "{linked}");
    let socket = app_socket(&dir);
    let nc =
        ana_runs(&mut client, &mut view, &format!("printf x | nc -U -w 1 '{}' && echo SOCKET-OK || echo SOCKET-DENIED", socket.display()))
            .await;
    assert!(nc.contains("SOCKET-DENIED"), "{nc}");
}

/// D2: nothing an agent's command writes runs later outside the sandbox:
/// the person's LaunchAgents, shell rc and login files, ssh's files, git's
/// global config, systemd user units; Claktory's own bin folder and
/// factory.db. Its own folder, /tmp and git in its folder still work.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "D"]
async fn an_agents_commands_cant_plant_what_runs_later() {
    let (dir, user, _addr, _factory, mut client, mut view) = contained_site().await;
    let root = home(&dir).root().to_path_buf();
    let mut denied: Vec<PathBuf> = [
        "Library/LaunchAgents/dev.gyro.d-test.plist",
        ".zshrc",
        ".zprofile",
        ".zshenv",
        ".zlogin",
        ".zlogout",
        ".bashrc",
        ".bash_profile",
        ".bash_login",
        ".profile",
        ".config/fish/config.fish",
        ".ssh/authorized_keys",
        ".ssh/authorized_keys2",
        ".ssh/config",
        ".ssh/rc",
        ".ssh/environment",
        ".gitconfig",
        ".config/git/config",
        ".config/systemd/user/gyro-d-test.service",
    ]
    .iter()
    .map(|file| user.join(file))
    .collect();
    denied.push(root.join("bin/gyro-d-test"));
    let allowed = [dir.join("site/written.txt"), PathBuf::from(format!("/tmp/gyro-d-test-{}", std::process::id()))];
    let script: String = denied
        .iter()
        .chain(&allowed)
        .map(|path| {
            let path = path.display();
            format!("mkdir -p \"$(dirname '{path}')\" 2>/dev/null; if printf x >> '{path}' 2>/dev/null; then echo 'OK {path}'; else echo 'DENIED {path}'; fi\n")
        })
        .collect();
    std::fs::write(dir.join("site/write.sh"), script).expect("the script");
    let output = ana_runs(&mut client, &mut view, "sh write.sh").await;
    let mut wrong: Vec<String> = denied
        .iter()
        .filter(|path| output.contains(&format!("OK {}", path.display())) || path.exists())
        .map(|path| format!("wrote {}", path.display()))
        .collect();
    wrong.extend(
        allowed
            .iter()
            .filter(|path| !output.contains(&format!("OK {}", path.display())))
            .map(|path| format!("couldn't write {}, which it may", path.display())),
    );
    let db = ana_runs(
        &mut client,
        &mut view,
        &format!("sqlite3 '{}' 'create table gyro(a)' && echo DB-OK || echo DB-DENIED", root.join("factory.db").display()),
    )
    .await;
    if !db.contains("DB-DENIED") {
        wrong.push(format!("wrote factory.db: {db}"));
    }
    let git = ana_runs(
        &mut client,
        &mut view,
        "git init -q . && git add written.txt && git -c user.name=T -c user.email=t@t commit -qm x && echo GIT-OK",
    )
    .await;
    if !git.contains("GIT-OK") {
        wrong.push(format!("git in its folder failed: {git}"));
    }
    let _ = std::fs::remove_file(&allowed[1]);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// D3 (macOS): an agent's command can't make a launchd job, even with a copy
/// of launchctl, and can't read a pasteboard. Only a named pasteboard
/// ("ruler") is used, written from outside, never the person's clipboard;
/// a job the test finds made is removed at once. `open`, osascript and
/// Apple events stay manual (Abbacchio's VM run): an automated one the
/// sandbox didn't hold would act in the person's session.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "D"]
async fn an_agents_commands_cant_make_launchd_jobs_or_read_the_pasteboard() {
    let (_dir, _user, _addr, _factory, mut client, mut view) = contained_site().await;
    let label = format!("dev.gyro.d-test.{}", std::process::id());
    let submitted = ana_runs(
        &mut client,
        &mut view,
        &format!("launchctl submit -l {label} -- /usr/bin/true && echo JOB-OK; cp /bin/launchctl /tmp/l-$$ && /tmp/l-$$ submit -l {label}.copy -- /usr/bin/true && echo COPY-OK; rm -f /tmp/l-$$"),
    )
    .await;
    let made: Vec<String> = [label.clone(), format!("{label}.copy")]
        .into_iter()
        .filter(|label| std::process::Command::new("launchctl").args(["list", label]).output().is_ok_and(|output| output.status.success()))
        .collect();
    for label in &made {
        let _ = std::process::Command::new("launchctl").args(["remove", label]).status();
    }
    assert!(
        made.is_empty() && !submitted.contains("JOB-OK") && !submitted.contains("COPY-OK"),
        "launchd jobs made from a contained command: {made:?}: {submitted}"
    );

    let marker = format!("gyro-d-pasteboard-{}", std::process::id());
    let mut copy =
        std::process::Command::new("pbcopy").args(["-pboard", "ruler"]).stdin(std::process::Stdio::piped()).spawn().expect("pbcopy");
    std::io::Write::write_all(copy.stdin.as_mut().expect("its stdin"), marker.as_bytes()).expect("write");
    drop(copy.stdin.take());
    copy.wait().expect("pbcopy ran");
    let pasted = ana_runs(&mut client, &mut view, "pbpaste -pboard ruler").await;
    let mut clear =
        std::process::Command::new("pbcopy").args(["-pboard", "ruler"]).stdin(std::process::Stdio::piped()).spawn().expect("pbcopy");
    drop(clear.stdin.take());
    clear.wait().expect("pbcopy ran");
    assert!(!pasted.contains(&marker), "a contained command read the pasteboard");
}

/// D4 (Linux): factoryd's memory and environment are closed to agents'
/// commands (dumpable 0), and the user's systemd bus can't run a command
/// outside the sandbox.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "D"]
async fn an_agents_commands_cant_reach_factoryds_memory_or_the_systemd_bus() {
    let (dir, _user, _addr, _factory, mut client, mut view) = contained_site().await;
    let pid = std::process::id();
    let proc = ana_runs(
        &mut client,
        &mut view,
        &format!(
            "cat /proc/{pid}/environ >/dev/null 2>&1 && echo ENV-OK; head -c1 /proc/{pid}/mem >/dev/null 2>&1 && echo MEM-OK; echo done"
        ),
    )
    .await;
    assert!(!proc.contains("ENV-OK") && !proc.contains("MEM-OK"), "{proc}");
    let key = home(&dir).root().join("remote/iroh.key");
    let bus = ana_runs(
        &mut client,
        &mut view,
        &format!("systemd-run --user --wait --pipe cat '{}' >/dev/null 2>&1 && echo BUS-OK || echo BUS-DENIED", key.display()),
    )
    .await;
    assert!(bus.contains("BUS-DENIED"), "the systemd bus ran a command outside the sandbox: {bus}");
}

/// D5 (Giorno 42266): what an agent plants in its repository's shared .git
/// (hooks, core.fsmonitor, a smudge filter named in .git/config with a
/// .gitattributes) runs, when factoryd's own git runs it on tree_land and
/// on the checkout's refresh, under the agent's sandbox: the key it tries to
/// copy out never arrives.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "D"]
async fn what_an_agent_plants_in_git_runs_contained_when_factoryd_runs_git() {
    let (dir, _user, _addr, _factory, mut client, mut view) = contained_site().await;
    let repo = dir.join("site");
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git").arg("-C").arg(&repo).args(args).output().expect("git");
        assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("lib.rs"), "one\n").expect("lib");
    git(&["add", "."]);
    git(&["-c", "user.name=T", "-c", "user.email=t@t", "commit", "-q", "-m", "start"]);
    let tree_on = v1::ExtensionChange { id: "tree".into(), enabled: Some(true), ..Default::default() };
    let update = v1::UpdateAgent { agent_id: "ana".into(), extension: Some(tree_on), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(update)).await;

    let key = home(&dir).root().join("remote/iroh.key");
    let leak = |what: &str| dir.join(format!("leak-{what}"));
    let copy = |what: &str| format!("cat '{}' > '{}'", key.display(), leak(what).display());
    let mut plant = String::from("g=$(git rev-parse --git-common-dir) && mkdir -p \"$g/hooks\"\n");
    for hook in ["post-checkout", "post-merge", "reference-transaction", "pre-auto-gc", "post-rewrite"] {
        plant.push_str(&format!("printf '#!/bin/sh\\n{}\\n' > \"$g/hooks/{hook}\" && chmod +x \"$g/hooks/{hook}\"\n", copy(hook)));
    }
    plant.push_str(&format!(
        "printf '#!/bin/sh\\n{}\\n' > \"$g/fsmonitor\" && chmod +x \"$g/fsmonitor\" && git config core.fsmonitor \"$g/fsmonitor\"\n",
        copy("fsmonitor")
    ));
    plant.push_str(&format!(
        "git config filter.x.smudge \"sh -c '{}; cat'\" && git config filter.x.clean cat\n",
        copy("filter").replace('\'', "")
    ));
    plant.push_str("echo '*.rs filter=x' > .gitattributes && echo two > lib.rs && git add -A && git -c user.name=T -c user.email=t@t commit -qm planted && echo PLANTED\n");
    let planted = ana_runs(&mut client, &mut view, &plant).await;
    assert!(planted.contains("PLANTED"), "{planted}");
    // factoryd's own git: the landing (its checks and the fast-forward of
    // the main checkout), then the checkout's refresh.
    crate::pipes::call(&mut client, &mut view, "ana", "tree_land", "{}").await;
    crate::pipes::call(&mut client, &mut view, "ana", "tree_land", "{}").await;
    let leaked: Vec<String> =
        ["post-checkout", "post-merge", "reference-transaction", "pre-auto-gc", "post-rewrite", "fsmonitor", "filter"]
            .iter()
            .filter(|what| std::fs::read(leak(what)).is_ok_and(|bytes| !bytes.is_empty()))
            .map(|what| what.to_string())
            .collect();
    assert!(leaked.is_empty(), "factoryd's git ran what the agent planted outside its sandbox: {leaked:?}");
}

/// D6 (Fugo 42089 point 2): a Developer ID backend refuses a local release
/// whose factoryd is signed ad hoc (as the linker leaves this test's), and
/// the Update banner says why; nothing is installed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "D"]
async fn a_release_an_agent_planted_isnt_installed() {
    let (dir, _user, _addr, factory, mut client, mut view) = contained_site().await;
    set_own_team(&factory);
    let release = home(&dir).root().join("releases/v99.0.0");
    std::fs::create_dir_all(&release).expect("the release");
    std::fs::copy(env!("CARGO_BIN_EXE_factoryd"), release.join("factoryd")).expect("an ad hoc factoryd");
    std::fs::write(home(&dir).root().join("releases/latest"), "v99.0.0").expect("latest");
    view.follow(&mut client, |view| view.workspace.latest_release == "v99.0.0").await;
    let install = client_message::Body::InstallUpdate(v1::InstallUpdate { app_path: String::new() });
    match apply_request(&mut client, &mut view, install).await {
        Body::Error(error) => assert!(!error.message.is_empty()),
        other => panic!("an ad hoc release was installed: {other:?}"),
    }
    assert!(!home(&dir).root().join("bin/factoryd").exists(), "it was copied into place");
}

/// D7 (Fugo 42004 Q1): an agent's command that edits config.json can't
/// repoint the account service, turn the account off or change the relays:
/// factoryd applies those keys only from the person. Its other edits apply.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "D"]
async fn an_agents_config_edit_cant_repoint_the_account_or_the_relays() {
    let (dir, _user, _addr, _factory, mut client, mut view) = contained_site().await;
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;
    let service = view.workspace.account.as_ref().map(|account| account.service.clone()).unwrap_or_default();
    let state = view.workspace.account.as_ref().map(|account| account.state());
    let relays = view.workspace.remote.as_ref().map(|remote| remote.relays.clone()).unwrap_or_default();
    let config = home(&dir).root().join("config.json");
    let edit = format!(
        "python3 -c \"import json; p='{}'; c=json.load(open(p)); c['name']='Edited'; c.setdefault('account', {{}}).update(service='https://account.evil.example', enabled=False); c.setdefault('remote', {{}})['relays']=['https://relay.evil.example/']; json.dump(c, open(p, 'w'))\" && echo EDITED",
        config.display()
    );
    let edited = crate::pipes::call(&mut client, &mut view, "rivet", "bash", &serde_json::json!({"command": edit}).to_string()).await;
    assert!(edited.contains("EDITED"), "Rivet may edit config.json: {edited}");
    view.follow(&mut client, |view| view.workspace.settings.as_ref().is_some_and(|settings| settings.name == "Edited")).await;
    assert_eq!(
        view.workspace.account.as_ref().map(|account| account.service.clone()).unwrap_or_default(),
        service,
        "the account service moved"
    );
    assert_eq!(view.workspace.account.as_ref().map(|account| account.state()), state, "the account was turned off");
    assert_eq!(view.workspace.remote.as_ref().map(|remote| remote.relays.clone()).unwrap_or_default(), relays, "the relays changed");
}

/// D9 (Fugo 42277): the in-process file tools refuse Claktory's keys too,
/// even in a project whose folder holds Claktory's home, and a hard link to
/// one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "D"]
async fn the_file_tools_refuse_claktorys_keys() {
    let (dir, _user, _addr, _factory, mut client, mut view) = contained_site().await;
    // A project whose folder is the test's folder, Claktory's home inside it.
    apply_request(&mut client, &mut view, crate::common::create_project("Everything", &dir)).await;
    apply_request(&mut client, &mut view, crate::common::create_agent("everything", "Eve", "")).await;
    let root = home(&dir).root().to_path_buf();
    std::fs::hard_link(root.join("remote/iroh.key"), dir.join("linked-key")).expect("a hard link");
    let mut wrong = Vec::new();
    for path in [root.join("remote/iroh.key"), root.join("account/claktory.key"), dir.join("linked-key")] {
        let path = path.display().to_string();
        let read = crate::pipes::call(
            &mut client,
            &mut view,
            "eve",
            "read_file",
            &serde_json::json!({"path": path, "offset": null, "limit": null}).to_string(),
        )
        .await;
        let key = std::fs::read_to_string(&path).unwrap_or_default();
        if !key.trim().is_empty() && read.contains(key.trim()) {
            wrong.push(format!("read_file read {path}"));
        }
        let written =
            crate::pipes::call(&mut client, &mut view, "eve", "write_file", &serde_json::json!({"path": path, "content": "x"}).to_string())
                .await;
        if std::fs::read_to_string(&path).is_ok_and(|now| now == "x") {
            wrong.push(format!("write_file wrote {path}: {written}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// D8: turning D on rotates what can be rotated as new files (a hard link
/// made before keeps only the dead value), ends the background tasks that
/// ran uncontained and tells their agents.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "D"]
async fn turning_containment_on_rotates_the_tool_token_and_ends_old_tasks() {
    let (dir, addr, factory, mut client, mut view) = site().await;
    set_user_home(&factory, &dir.join("user"));
    set_sandbox_executable(&factory);
    let root = home(&dir).root().to_path_buf();
    let old = std::fs::read_to_string(root.join("tool-token")).expect("the tool token");
    std::fs::hard_link(root.join("tool-token"), dir.join("old-token")).expect("a hard link made before");
    let started = crate::pipes::call(&mut client, &mut view, "ana", "bash", r#"{"command":"sleep 300","run_in_background":true}"#).await;
    assert!(started.starts_with("Started task-"), "{started}");
    contain(&mut client, &mut view).await;
    contained(&mut client, &mut view, &dir).await;
    let new = std::fs::read_to_string(root.join("tool-token")).expect("the tool token");
    assert_ne!(new.trim(), old.trim(), "the tool token wasn't rotated");
    assert_eq!(
        std::fs::read_to_string(dir.join("old-token")).expect("the link").trim(),
        old.trim(),
        "the old link should hold only the dead value"
    );
    assert!(Agent::hello(addr, old.trim(), false).await.is_err(), "the old tool token still opens a connection");
    view.follow(&mut client, |view| {
        view.entries.iter().any(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.source() == v1::EventSource::Task))
            && crate::pipes::idle(view, "ana")
    })
    .await;
    let list = crate::pipes::call(&mut client, &mut view, "ana", "tasks_list", "{}").await;
    assert_eq!(list, "No background tasks are running.");
}

/// D10 (G6, agent-safety.md): `factoryd export` run as an agent's command
/// can't carry Claktory off: a plain export, which would hold the vault,
/// its key on disk, the sign-ins and the tunnel's key under a passphrase it
/// prints, fails and leaves no bundle; `--final` neither stops Claktory nor
/// fences its identity.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "D"]
async fn an_agents_commands_cant_export_claktory() {
    let (dir, user, addr, _factory, mut client, mut view) = contained_site().await;
    let root = home(&dir).root().to_path_buf();
    let env = format!("HOME='{}' FACTORY_HOME='{}' FACTORY_PORT={}", user.display(), root.display(), addr.port());
    let factoryd = env!("CARGO_BIN_EXE_factoryd");
    let bundle = dir.join("site/everything.claktory");
    let plain =
        ana_runs(&mut client, &mut view, &format!("{env} '{factoryd}' export --out '{}' --json; echo EXIT=$?", bundle.display())).await;
    assert!(!plain.contains("EXIT=0"), "export succeeded from an agent's command: {plain}");
    assert!(!bundle.exists(), "a bundle was written");
    let last = ana_runs(
        &mut client,
        &mut view,
        &format!("{env} '{factoryd}' export --final --confirm move --out '{}' --json; echo EXIT=$?", bundle.display()),
    )
    .await;
    assert!(!last.contains("EXIT=0"), "export --final succeeded from an agent's command: {last}");
    // No bundle anywhere it can write, and no passphrase in what it saw (Fugo 42686).
    let mut written = Vec::new();
    files_under(&dir, &mut written);
    if let Ok(entries) = std::fs::read_dir("/tmp") {
        written.extend(entries.flatten().map(|entry| entry.path()).filter(|path| path.is_file()));
    }
    let bundles: Vec<&PathBuf> = written.iter().filter(|path| path.to_string_lossy().contains(".claktory")).collect();
    assert!(bundles.is_empty(), "bundles written: {bundles:?}");
    for output in [&plain, &last] {
        let lines: Vec<serde_json::Value> = output.lines().filter_map(|line| serde_json::from_str(line).ok()).collect();
        assert!(!lines.iter().any(|line| line.get("passphrase").is_some()), "a passphrase reached the agent: {output}");
    }
    for fenced in ["remote", "vault", "account"] {
        assert!(root.join(fenced).exists(), "{fenced} was fenced away");
    }
    assert!(!root.join("moved.json").exists(), "this copy was marked moved");
    // Claktory still answers.
    let ping = client_message::Body::UpdateSettings(v1::UpdateSettings::default());
    assert!(matches!(apply_request(&mut client, &mut view, ping).await, Body::Ack(_)));
}


// G8 (Fugo 46025, 46029; Ghiaccio 46021; Bruno 46017; no cards, per Matt's
// card 283): an agent can't make itself an admin. Under D only an agent
// that's an admin when its command spawns can write this Claktory's own
// config.json, by any path (a rewrite, a rename over it, rm and recreate,
// the temp name factoryd writes it through, a hard link made before); its
// file tools read the same flag per call. Nothing but config.json makes an
// agent an admin. Other homes' config.json (the dev Claktory, qa.sh
// sandboxes) stay writable. A command spawned as an admin keeps the write
// until it ends; tests use a fresh one. Ignored with D.

/// Makes Ana an admin, in config.json's own words (Python on `a`, the agents
/// by id).
const ANA_ADMIN: &str = "a['ana'].setdefault('extensions', {}).setdefault('agents', {})['admin'] = True";

/// A made-up value for the vault's one secret: nothing real.
const MADE_UP: &str = "g8-made-up-value-0123456789abcdef";

/// Puts the vault's one secret in, for an admin to give.
async fn a_secret(client: &mut Client, view: &mut View) {
    let entry = v1::SecretEntry { variable: "CLOUDFLARE_API_TOKEN".into(), value: MADE_UP.into(), file: false };
    let secret = v1::CreateSecret { name: "Cloudflare".into(), entries: vec![entry], ..Default::default() };
    apply_request(client, view, client_message::Body::CreateSecret(secret)).await;
}

/// Rivet on the scripted model, so its commands run.
async fn rivet_runs(client: &mut Client, view: &mut View) {
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(client, view, client_message::Body::UpdateAgent(rivet)).await;
}

/// What `agent` gets trying to give the vault's secret to `to`: only an
/// admin can. Through extension_call, which runs any tool of the extensions
/// the agent has on now, so a change since its context began counts.
async fn gives(client: &mut Client, view: &mut View, agent: &str, to: &str) -> String {
    let args = serde_json::json!({"tool": "secrets_grant", "args": {"secret": "Cloudflare", "agents": [to]}});
    crate::pipes::call(client, view, agent, "extension_call", &args.to_string()).await
}

/// `agent`'s extension `id` as the Workspace shows it.
fn shown(view: &View, agent: &str, id: &str) -> Option<v1::AgentExtension> {
    let agent = view.workspace.agents.iter().find(|shown| shown.id == agent)?;
    agent.extensions.iter().find(|extension| extension.id == id).cloned()
}

fn admin_shown(view: &View, agent: &str) -> bool {
    shown(view, agent, "agents").and_then(|extension| extension.options.get("admin").map(|value| value.on)).unwrap_or(false)
}

/// Has `by`'s command change config.json: `change` is Python on `a`, the
/// agents by id, and `c`, the whole file. A new name rides along; the
/// Workspace showing it is the sign the edit was read.
async fn edit_config(client: &mut Client, view: &mut View, dir: &Path, by: &str, change: &str, mark: &str) {
    let config = home(dir).root().join("config.json");
    let script = format!(
        "python3 - <<'PY'\nimport json\np = '{}'\nc = json.load(open(p))\na = {{x['id']: x for x in c['agents']}}\n{change}\nc['name'] = '{mark}'\njson.dump(c, open(p, 'w'), indent=2)\nprint('EDITED')\nPY",
        config.display()
    );
    let edited = crate::pipes::call(client, view, by, "bash", &serde_json::json!({"command": script}).to_string()).await;
    assert!(edited.contains("EDITED"), "{by} couldn't edit config.json: {edited}");
    view.follow(client, |view| view.workspace.settings.as_ref().is_some_and(|settings| settings.name == mark)).await;
}

/// The person makes `agent` an admin, or not, in the app's agent settings.
fn admin_from_app(agent: &str, on: bool) -> client_message::Body {
    let change = v1::ExtensionChange {
        id: "agents".into(),
        option_id: "admin".into(),
        value: Some(v1::OptionValue { on, ..Default::default() }),
        ..Default::default()
    };
    client_message::Body::UpdateAgent(v1::UpdateAgent { agent_id: agent.into(), extension: Some(change), ..Default::default() })
}

/// Every way a command could write `config` (Fugo 46025, 46029), as one
/// script printing "OK <way>" or "DENIED <way>" per line. A hard link made
/// before containment has a test of its own.
fn config_writes(config: &Path) -> String {
    let config = config.display();
    [
        format!(": >> '{config}'"),
        format!("python3 -c \"import json; p='{config}'; c=json.load(open(p)); c['name']='Rewritten'; json.dump(c, open(p, 'w'))\""),
        format!("cp '{config}' \"$TMPDIR/g8-config\" && mv -f \"$TMPDIR/g8-config\" '{config}'"),
        format!("cp '{config}' '{config}.g8' 2>/dev/null; rm -f '{config}' && cp \"$TMPDIR/g8-keep\" '{config}'"),
        format!(": > '{config}.tmp'"),
        format!("ln -s /tmp/g8-elsewhere '{config}.tmp'"),
        format!("ln '{config}' \"$TMPDIR/g8-link\""),
    ]
    .iter()
    .enumerate()
    .map(|(way, command)| format!("( {command} ) >/dev/null 2>&1 && echo 'OK {way}' || echo 'DENIED {way}'"))
    .collect::<Vec<_>>()
    .join("\n")
}

/// Asserts every way in `output` was denied, and config.json is as it was.
fn none_wrote(who: &str, output: &str, config: &Path, before: &[u8]) {
    let wrote: Vec<&str> = output.lines().filter(|line| line.starts_with("OK ")).collect();
    assert!(wrote.is_empty() && output.lines().filter(|line| line.starts_with("DENIED ")).count() == 7, "{who} wrote config.json: {output}");
    assert_eq!(std::fs::read(config).expect("config.json is there"), before, "{who} changed config.json");
    assert!(!config.with_file_name("config.json.tmp").exists(), "{who} planted config.json.tmp");
}

/// A contained site with the vault's secret.
async fn g8_contained() -> (PathBuf, PathBuf, SocketAddr, std::sync::Arc<factoryd::Factory>, Client, View) {
    let (dir, user, addr, factory, mut client, mut view) = contained_site().await;
    a_secret(&mut client, &mut view).await;
    (dir, user, addr, factory, client, view)
}

/// What a command of Ana's that writes "x" through `link` leaves in
/// config.json, against `before`.
async fn through_link(client: &mut Client, view: &mut View, config: &Path, link: &Path, before: &[u8], when: &str) {
    ana_runs(client, view, &format!("printf x >> '{}'", link.display())).await;
    assert_eq!(std::fs::read(config).expect("config.json"), before, "written through a hard link made {when}");
}

/// G8: an agent that isn't an admin can't write this Claktory's config.json
/// in any way: rewrite, rename over, rm and recreate, factoryd's temp name
/// (empty or a symlink), or a hard link made from inside.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "D"]
async fn a_non_admins_commands_cant_write_config_json() {
    let (dir, _user, _addr, _factory, mut client, mut view) = g8_contained().await;
    let config = home(&dir).root().join("config.json");
    let before = std::fs::read(&config).expect("config.json");
    let output = ana_runs(&mut client, &mut view, &format!("cp '{}' \"$TMPDIR/g8-keep\"\n{}", config.display(), config_writes(&config))).await;
    none_wrote("Ana", &output, &config, &before);
    let given = gives(&mut client, &mut view, "ana", "bo").await;
    assert!(!given.starts_with("Gave"), "{given}");
}

/// G8: nor with its file tools, which read the same flag per call.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "D"]
async fn a_non_admins_file_tools_cant_write_config_json() {
    let (dir, _user, _addr, _factory, mut client, mut view) = g8_contained().await;
    let config = home(&dir).root().join("config.json");
    let before = std::fs::read(&config).expect("config.json");
    let name = view.workspace.settings.as_ref().map(|settings| settings.name.clone()).unwrap_or_default();
    let write = serde_json::json!({"path": config, "content": "{}\n"});
    crate::pipes::call(&mut client, &mut view, "ana", "write_file", &write.to_string()).await;
    let edit = serde_json::json!({"path": config, "edits": [{"oldText": format!("\"{name}\""), "newText": "\"Edited\"", "replaceAll": null}]});
    crate::pipes::call(&mut client, &mut view, "ana", "edit_file", &edit.to_string()).await;
    assert_eq!(std::fs::read(&config).expect("config.json"), before, "Ana's file tools changed config.json");
}

/// G8: holding the Claktory manual doesn't make the file writable (D said
/// "admin and manual agents"; now admins only, Ghiaccio 46021).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "D"]
async fn an_agent_with_the_manual_that_isnt_an_admin_cant_write_config_json() {
    let (dir, _user, _addr, _factory, mut client, mut view) = g8_contained().await;
    let manual = v1::ExtensionChange { id: "memory:factory".into(), enabled: Some(true), ..Default::default() };
    let on = client_message::Body::UpdateAgent(v1::UpdateAgent { agent_id: "ana".into(), extension: Some(manual), ..Default::default() });
    assert!(matches!(apply_request(&mut client, &mut view, on).await, Body::Ack(_)));
    let config = home(&dir).root().join("config.json");
    let before = std::fs::read(&config).expect("config.json");
    let output = ana_runs(&mut client, &mut view, &format!("cp '{}' \"$TMPDIR/g8-keep\"\n{}", config.display(), config_writes(&config))).await;
    none_wrote("Ana, with the manual", &output, &config, &before);
    let write = serde_json::json!({"path": config, "content": "{}\n"});
    crate::pipes::call(&mut client, &mut view, "ana", "write_file", &write.to_string()).await;
    assert_eq!(std::fs::read(&config).expect("config.json"), before, "Ana's write_file changed config.json");
}

/// G8: an admin still sets Claktory up with no card: Rivet's edit makes Ana
/// an admin at once, and she can give.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "D"]
async fn an_admins_edit_still_makes_an_agent_an_admin_at_once() {
    let (dir, _user, _addr, _factory, mut client, mut view) = g8_contained().await;
    rivet_runs(&mut client, &mut view).await;
    edit_config(&mut client, &mut view, &dir, "rivet", ANA_ADMIN, "Set up").await;
    view.follow(&mut client, |view| admin_shown(view, "ana")).await;
    let given = gives(&mut client, &mut view, "ana", "bo").await;
    assert!(given.starts_with("Gave"), "Ana, made an admin by Rivet, can't give: {given}");
}

/// G8: the write follows the agent's admin at spawn: once the person turns
/// Rivet's admin off in the app, its next command can't write.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "D"]
async fn a_demoted_admins_next_command_cant_write_config_json() {
    let (dir, _user, _addr, _factory, mut client, mut view) = g8_contained().await;
    rivet_runs(&mut client, &mut view).await;
    edit_config(&mut client, &mut view, &dir, "rivet", "pass", "Rivet can").await;
    assert!(matches!(apply_request(&mut client, &mut view, admin_from_app("rivet", false)).await, Body::Ack(_)));
    view.follow(&mut client, |view| !admin_shown(view, "rivet")).await;
    let config = home(&dir).root().join("config.json");
    let before = std::fs::read(&config).expect("config.json");
    let script = format!("cp '{}' \"$TMPDIR/g8-keep\"\n{}", config.display(), config_writes(&config));
    let output = crate::pipes::call(&mut client, &mut view, "rivet", "bash", &serde_json::json!({"command": script}).to_string()).await;
    none_wrote("Rivet, no longer an admin", &output, &config, &before);
}

/// G8: the person's own way stays immediate: the app's agent settings make
/// Bo an admin at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "D"]
async fn the_apps_agent_settings_still_make_an_admin_at_once() {
    let (_dir, _user, _addr, _factory, mut client, mut view) = g8_contained().await;
    assert!(matches!(apply_request(&mut client, &mut view, admin_from_app("bo", true)).await, Body::Ack(_)));
    view.follow(&mut client, |view| admin_shown(view, "bo")).await;
    let given = gives(&mut client, &mut view, "bo", "ana").await;
    assert!(given.starts_with("Gave"), "Bo, made an admin in the app, can't give: {given}");
}

/// G8 (Fugo 46025, 3): nothing but config.json makes an agent an admin:
/// uncontained, Ana writes admin into files of her own folder and rows of
/// every settings table in factory.db, and still isn't one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "D"]
async fn nothing_but_config_json_makes_an_agent_an_admin() {
    let (dir, _addr, _factory, mut client, mut view) = site().await;
    a_secret(&mut client, &mut view).await;
    let own = home(&dir).root().join("agents/ana");
    let db = home(&dir).root().join("factory.db");
    let script = format!(
        "mkdir -p '{own}' && printf '{{\"admin\": true, \"extensions\": {{\"agents\": {{\"admin\": true}}}}}}' > '{own}/config.json' && printf 'true' > '{own}/admin'\npython3 - <<'PY'\nimport sqlite3\ndb = sqlite3.connect('{db}')\ntables = [row[0] for row in db.execute(\"SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE '%settings%'\")]\nfor table in tables:\n    cols = [row[1] for row in db.execute(f'PRAGMA table_info({{table}})')]\n    for key in ['admin', 'agents.admin', 'ana.admin', 'agents.ana.admin']:\n        try:\n            db.execute(f'INSERT OR REPLACE INTO {{table}} ({{cols[0]}}, {{cols[1]}}) VALUES (?, ?)', (key, 1))\n        except Exception:\n            pass\ndb.commit()\nprint('PLANTED', len(tables))\nPY",
        own = own.display(),
        db = db.display()
    );
    let planted = ana_runs(&mut client, &mut view, &script).await;
    assert!(planted.contains("PLANTED"), "{planted}");
    let given = gives(&mut client, &mut view, "ana", "bo").await;
    assert!(!given.starts_with("Gave"), "Ana's own files or factory.db made her an admin: {given}");
    assert!(!admin_shown(&view, "ana"));
}

/// G8 (Bruno 46017, Fugo 46029): only this Claktory's own config.json is
/// denied: another home's (a qa.sh sandbox, the dev Claktory) stays
/// writable for an agent that isn't an admin.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "D"]
async fn another_homes_config_json_stays_writable() {
    let (dir, _user, _addr, _factory, mut client, mut view) = g8_contained().await;
    let sandbox = dir.join("site/sandboxes/qa/home/config.json");
    std::fs::create_dir_all(sandbox.parent().expect("its folder")).expect("a sandbox home");
    std::fs::write(&sandbox, "{\"name\": \"Sandbox\"}\n").expect("its config.json");
    let script = format!(
        "python3 -c \"import json; p='{0}'; c=json.load(open(p)); c['name']='Edited'; json.dump(c, open(p, 'w'))\" && echo SANDBOX-OK || echo SANDBOX-DENIED",
        sandbox.display()
    );
    let output = ana_runs(&mut client, &mut view, &script).await;
    assert!(output.contains("SANDBOX-OK"), "a sandbox's config.json isn't writable: {output}");
    assert!(std::fs::read_to_string(&sandbox).expect("read").contains("Edited"));
}

/// G8 (Ghiaccio 46032): a hard link to config.json made before containment
/// writes nothing to it: factoryd gives config.json a new inode when it
/// turns containment on, and at a start that finds it linked (nlink > 1).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "D"]
async fn a_hard_link_made_before_containment_writes_nothing_to_config_json() {
    let dir;
    let user;
    {
        let (made, _addr, factory, mut client, mut view) = site().await;
        dir = made;
        user = dir.join("user");
        std::fs::create_dir_all(user.join("Library/LaunchAgents")).expect("the person's home");
        for file in DENIED_READS {
            let path = home(&dir).root().join(file);
            if !path.exists() {
                std::fs::create_dir_all(path.parent().expect("a folder")).expect("its folder");
                std::fs::write(&path, "seeded by the test\n").expect("seed");
            }
        }
        let config = home(&dir).root().join("config.json");
        // Made while containment was off, as any command could.
        let early = dir.join("site/early-link.json");
        std::fs::hard_link(&config, &early).expect("a hard link");
        set_user_home(&factory, &user);
        set_sandbox_executable(&factory);
        contain(&mut client, &mut view).await;
        contained(&mut client, &mut view, &dir).await;
        let before = std::fs::read(&config).expect("config.json");
        through_link(&mut client, &mut view, &config, &early, &before, "before containment was turned on").await;
    }
    // One made by something outside while Claktory was stopped, found at
    // the next start.
    let config = home(&dir).root().join("config.json");
    let late = dir.join("site/late-link.json");
    std::fs::hard_link(&config, &late).expect("a hard link");
    let (addr, factory) = start(&dir).await;
    set_user_home(&factory, &user);
    set_sandbox_executable(&factory);
    let (mut client, mut view) = person(addr, &factory).await;
    contained(&mut client, &mut view, &dir).await;
    let before = std::fs::read(&config).expect("config.json");
    through_link(&mut client, &mut view, &config, &late, &before, "while Claktory was stopped").await;
}
