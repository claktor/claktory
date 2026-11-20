//! Connecting services (docs/CONNECTORS.md), with stand-in recipes: an
//! agent allowed to suggests a service, the person connects it from the
//! device that started it, the backend keeps the tokens in the vault where
//! no agent gets them, and a secret made from it with narrower access is
//! what an agent is given, renewed before its commands get it.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use factory_protocol::v1::{self, ConnectFlowState, ConnectionStatus, client_message::Body as Request, entry::Kind, server_message::Body};
use factoryd::connectors::{
    AccessSpec, BoxFuture, Choice, Choices, Control, Derive, Derived, Device, ExportMeta, FieldSpec, FlowKind, Http, Inspected, Poll,
    Problem, Recipe, Spec, Start, Variable,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::common::{
    Client, MODEL, Scripted, View, apply_request, create_agent, create_project, describe, send_message, start_with, temp_dir,
};

const LOOPBACK: &str = "http://127.0.0.1:{port}/oauth/demo";

/// A stand-in service: it signs in by browser (or device, `device`),
/// mints tokens on demand, and can be told to stop accepting the
/// connection or to make secrets due for renewal.
#[derive(Default)]
struct Demo {
    spec: Spec,
    /// Counts tokens handed out, so each is new.
    minted: AtomicUsize,
    /// The service no longer accepts the connection.
    revoked: AtomicBool,
    /// Tokens need refreshing on the next use.
    stale: AtomicBool,
    /// Secrets made are due for renewal at once.
    due: AtomicBool,
    /// Device flow: polls before the person approves.
    polls: AtomicUsize,
    /// Minted token ids removed at the service.
    removed: Mutex<Vec<String>>,
}

impl Demo {
    fn browser() -> Demo {
        Demo {
            spec: Spec {
                id: "demo".into(),
                name: "Demo".into(),
                description: "Your Demo account.".into(),
                accesses: vec![AccessSpec {
                    id: "dns".into(),
                    label: "Edit DNS".into(),
                    why: "Change DNS records in the accounts you choose.".into(),
                    scopes: vec!["dns:edit".into(), "tokens:write".into()],
                    preselected: true,
                }],
                flow: FlowKind::Browser,
                redirect_uris: vec![LOOPBACK.into(), "com.claktory.app:/oauth/demo".into()],
                variables: vec!["DEMO_TOKEN".into()],
                ..Spec::default()
            },
            ..Demo::default()
        }
    }

    fn device() -> Demo {
        Demo {
            spec: Spec {
                id: "cloudy".into(),
                name: "Cloudy".into(),
                description: "Your Cloudy roles.".into(),
                accesses: vec![AccessSpec { id: "roles".into(), label: "Roles".into(), preselected: true, ..AccessSpec::default() }],
                fields: vec![FieldSpec { id: "region".into(), label: "Region".into(), ..FieldSpec::default() }],
                flow: FlowKind::Device,
                variables: vec!["CLOUDY_KEY".into()],
                ..Spec::default()
            },
            ..Demo::default()
        }
    }

    fn tokens(&self) -> Control {
        let n = self.minted.fetch_add(1, Ordering::SeqCst);
        Control::from_value(
            &json!({"access_token": format!("demo-access-{n}-abcdefgh"), "refresh_token": format!("demo-refresh-{n}-abcdefgh")}),
        )
    }

    fn made(&self, permissions: Vec<String>, resources: Vec<String>) -> Derived {
        let n = self.minted.fetch_add(1, Ordering::SeqCst);
        let renew_at_ms = if self.due.load(Ordering::SeqCst) { 1 } else { i64::MAX };
        Derived {
            values: Some(vec![Variable { name: "DEMO_TOKEN".into(), value: Zeroizing::new(format!("minted-{n}-0123456789")) }]),
            meta: ExportMeta {
                access: vec!["Edit DNS of acme.com".into()],
                permissions,
                resources,
                expires_ms: i64::MAX,
                renew_at_ms,
                recipe: json!({"token_id": format!("t{n}")}),
                ..ExportMeta::default()
            },
        }
    }
}

impl Recipe for Demo {
    fn spec(&self) -> &Spec {
        &self.spec
    }

    fn authorize_url(&self, start: &Start<'_>) -> Result<String, Problem> {
        Ok(format!(
            "https://demo.example/authorize?scope={}&state={}&code_challenge={}&redirect_uri={}",
            start.access.scopes.join("+"),
            start.state,
            start.challenge,
            start.redirect_uri
        ))
    }

    fn exchange<'a>(
        &'a self,
        _http: &'a Http,
        start: &'a Start<'a>,
        code: &'a str,
        verifier: &'a str,
    ) -> BoxFuture<'a, Result<Control, Problem>> {
        Box::pin(async move {
            // PKCE: the verifier is the one the challenge was made from.
            assert_eq!(URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())), start.challenge);
            match code {
                "good-code" => Ok(self.tokens()),
                _ => Err(Problem::Refused("Demo didn't accept that code.".into())),
            }
        })
    }

    fn start_device<'a>(&'a self, _http: &'a Http, start: &'a Start<'a>) -> BoxFuture<'a, Result<Device, Problem>> {
        let region = start.fields.get("region").cloned().unwrap_or_default();
        Box::pin(async move {
            Ok(Device {
                url: format!("https://{region}.cloudy.example/device?user_code=ABCD-1234"),
                user_code: "ABCD-1234".into(),
                interval: Duration::from_secs(1),
                expires_ms: 0,
                pending: Control::from_value(&json!({"device_code": "dc-1"})),
            })
        })
    }

    fn poll_device<'a>(&'a self, _http: &'a Http, _start: &'a Start<'a>, pending: &'a Control) -> BoxFuture<'a, Result<Poll, Problem>> {
        Box::pin(async move {
            assert!(pending.as_str().contains("dc-1"));
            if self.polls.fetch_add(1, Ordering::SeqCst) == 0 { Ok(Poll::Pending) } else { Ok(Poll::Done(self.tokens())) }
        })
    }

    fn inspect<'a>(&'a self, _http: &'a Http, _control: &'a Control) -> BoxFuture<'a, Result<Inspected, Problem>> {
        Box::pin(async {
            Ok(Inspected {
                subject: "acct-1".into(),
                account: "matt@example.com".into(),
                access: vec!["Edit DNS and make tokens in Acme".into()],
                scopes: vec!["dns:edit".into(), "tokens:write".into()],
                resources: vec!["Demo account Acme".into()],
                expires_ms: 0,
            })
        })
    }

    fn refresh<'a>(&'a self, _http: &'a Http, _control: &'a Control) -> BoxFuture<'a, Result<Option<Control>, Problem>> {
        Box::pin(async {
            if self.revoked.load(Ordering::SeqCst) {
                return Err(Problem::Reconnect("Demo no longer accepts it.".into()));
            }
            Ok(self.stale.swap(false, Ordering::SeqCst).then(|| self.tokens()))
        })
    }

    fn choices<'a>(&'a self, _http: &'a Http, _control: &'a Control) -> BoxFuture<'a, Result<Choices, Problem>> {
        Box::pin(async {
            let choice = |kind: &str, id: &str, parent: &str| Choice {
                kind: kind.into(),
                id: id.into(),
                label: id.to_uppercase(),
                parent: parent.into(),
                why: String::new(),
            };
            Ok(Choices {
                choices: vec![choice("account", "acme", ""), choice("zone", "acme.com", "acme"), choice("permission", "dns-edit", "")],
                rules: "Pick one account and at least one permission.".into(),
            })
        })
    }

    fn derive<'a>(&'a self, _http: &'a Http, _control: &'a Control, ask: &'a Derive) -> BoxFuture<'a, Result<Derived, Problem>> {
        Box::pin(async move {
            if ask.permissions.is_empty() {
                return Err(Problem::Refused("Pick at least one permission.".into()));
            }
            Ok(self.made(ask.permissions.clone(), ask.resources.clone()))
        })
    }

    fn renew<'a>(&'a self, _http: &'a Http, _control: &'a Control, export: &'a ExportMeta) -> BoxFuture<'a, Result<Derived, Problem>> {
        Box::pin(async move { Ok(self.made(export.permissions.clone(), export.resources.clone())) })
    }

    fn remove_exports<'a>(
        &'a self,
        _http: &'a Http,
        _control: &'a Control,
        exports: &'a [ExportMeta],
    ) -> BoxFuture<'a, Result<(), Problem>> {
        Box::pin(async move {
            let ids = exports.iter().filter_map(|export| export.recipe["token_id"].as_str().map(str::to_owned));
            self.removed.lock().expect("lock").extend(ids);
            Ok(())
        })
    }

    fn revoke<'a>(&'a self, _http: &'a Http, _control: &'a Control) -> BoxFuture<'a, Result<(), Problem>> {
        Box::pin(async {
            self.revoked.store(true, Ordering::SeqCst);
            Ok(())
        })
    }
}

fn failure(body: &Body) -> String {
    match body {
        Body::Error(error) => error.message.clone(),
        other => panic!("expected an error, got {other:?}"),
    }
}

fn vault(view: &View) -> v1::VaultState {
    view.workspace.vault.clone().expect("vault state")
}

fn flow(view: &View, id: &str) -> Option<v1::ConnectFlow> {
    vault(view).flows.into_iter().find(|flow| flow.id == id)
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == v1::AgentStatus::Idle && agent.queued_messages == 0
}

/// Sends a request that must succeed, the view following.
async fn ok(client: &mut Client, view: &mut View, body: Request) {
    let reply = apply_request(client, view, body).await;
    assert!(matches!(reply, Body::Ack(_)), "expected ack, got {reply:?}");
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

fn connect_option(agent: &str, on: bool) -> Request {
    let value = v1::OptionValue { on, agent_ids: Vec::new() };
    let change = v1::ExtensionChange { id: "secrets".into(), option_id: "connect".into(), value: Some(value), ..Default::default() };
    Request::UpdateAgent(v1::UpdateAgent { agent_id: agent.into(), extension: Some(change), ..Default::default() })
}

fn query(url: &str, key: &str) -> String {
    let url = reqwest::Url::parse(url).expect("url");
    url.query_pairs().find(|(name, _)| name == key).map(|(_, value)| value.into_owned()).expect(key)
}

#[tokio::test]
async fn an_agent_suggests_a_service_the_person_connects_it_and_only_a_narrower_secret_is_given() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let tools = models.tools.clone();
    let (addr, factory) = start_with(&dir, models).await;
    factory.set_vault_passphrase_cost("argon2id m=1024 t=1 p=1");
    let demo = Arc::new(Demo::browser());
    factory.add_connector(demo.clone(), &[]);
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert_eq!(vault(&view).connectors.iter().map(|connector| connector.id.as_str()).collect::<Vec<_>>(), ["demo"]);
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;
    apply_request(&mut client, &mut view, connect_option("builder", true)).await;
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdateAgent(rivet)).await;

    // Without the option, an agent has no tool to suggest one.
    call(&mut client, &mut view, "writer", "bash", r#"{"command":"true"}"#).await;
    assert!(!tools.lock().expect("lock").contains("secrets_propose_connection"));
    let args = r#"{"service":"Demo","purpose":"Deploy acme.com","access":"DNS edit on acme.com","reason":"The site moves to Demo."}"#;
    let said = call(&mut client, &mut view, "builder", "secrets_propose_connection", args).await;
    assert!(said.starts_with("Proposed connecting Demo;"), "{said}");
    let unknown = call(&mut client, &mut view, "builder", "secrets_propose_connection", &args.replace("Demo\"", "Stripe\"")).await;
    assert!(unknown.contains("Claktory can't connect \"Stripe\". It can connect: demo."), "{unknown}");
    let proposal = vault(&view).proposals[0].clone();
    assert_eq!(
        (proposal.agent_id.as_str(), proposal.connector_id.as_str(), proposal.purpose.as_str()),
        ("builder", "demo", "Deploy acme.com")
    );
    assert!(describe(&view.entries).iter().any(|line| line.starts_with(&format!("connect proposal {} (Pending)", proposal.id))));

    // Started on this connection; only a receiver the service allows.
    let start = |redirect: &str| {
        Request::StartConnect(v1::StartConnect {
            connector_id: "demo".into(),
            proposal_id: proposal.id.clone(),
            redirect_uri: redirect.into(),
            ..Default::default()
        })
    };
    let refused = apply_request(&mut client, &mut view, start("https://evil.example/oauth/demo")).await;
    assert_eq!(failure(&refused), "Demo can't return to https://evil.example/oauth/demo.");
    let redirect = "http://127.0.0.1:53682/oauth/demo";
    let started = apply_request(&mut client, &mut view, start(redirect)).await;
    let Body::ConnectStarted(started) = started else { panic!("expected a start, got {started:?}") };
    assert_eq!(query(&started.url, "redirect_uri"), redirect);
    assert_eq!(query(&started.url, "scope"), "dns:edit tokens:write");
    let state = query(&started.url, "state");
    view.follow(&mut client, |view| flow(view, &started.flow_id).is_some_and(|flow| flow.state() == ConnectFlowState::Waiting)).await;
    assert!(!format!("{:?}", vault(&view).flows).contains(&state), "no device sees the state but the one that started it");

    // Another device can't finish it; a wrong state or address is refused.
    let finish = |url: String| Request::FinishConnect(v1::FinishConnect { flow_id: started.flow_id.clone(), callback_url: url });
    let (mut other, other_workspace, _) = Client::connect(addr).await;
    let mut other_view = View { workspace: other_workspace, ..View::default() };
    let elsewhere = apply_request(&mut other, &mut other_view, finish(format!("{redirect}?code=good-code&state={state}"))).await;
    assert_eq!(failure(&elsewhere), "This connection was started on another device. Finish it there, or start again here.");
    let forged = apply_request(&mut client, &mut view, finish(format!("{redirect}?code=good-code&state=forged"))).await;
    assert_eq!(failure(&forged), "That address doesn't belong to this connection. Start again.");
    let moved = apply_request(&mut client, &mut view, finish(format!("http://127.0.0.1:9/oauth/demo?code=good-code&state={state}"))).await;
    assert_eq!(failure(&moved), "That address doesn't belong to this connection.");
    ok(&mut client, &mut view, finish(format!("{redirect}?code=good-code&state={state}"))).await;
    view.follow(&mut client, |view| flow(view, &started.flow_id).is_some_and(|flow| flow.state() == ConnectFlowState::Connected)).await;
    let again = apply_request(&mut client, &mut view, finish(format!("{redirect}?code=good-code&state={state}"))).await;
    assert_eq!(failure(&again), "This connection is already done.");

    // Kept in the vault, given to no one, its tokens nowhere to be read.
    let state_now = vault(&view);
    let connection = state_now.secrets.iter().find(|secret| secret.connection.is_some()).expect("a connection").clone();
    assert_eq!(connection.name, "Demo (matt@example.com)");
    assert_eq!(flow(&view, &started.flow_id).expect("flow").secret_id, connection.id);
    let info = connection.connection.clone().expect("connection");
    assert_eq!(
        (info.account.as_str(), info.status(), info.proposal_id.as_str()),
        ("matt@example.com", ConnectionStatus::Ready, proposal.id.as_str())
    );
    assert!(connection.variables.is_empty() && connection.grants.is_empty());
    assert!(state_now.proposals.is_empty());
    for file in ["home/vault/vault.db", "home/factory.db", "home/config.json"] {
        let bytes = std::fs::read(dir.join(file)).unwrap_or_default();
        assert!(!bytes.windows(12).any(|window| window == b"demo-access-"), "{file} holds a token");
    }
    let give = |id: &str| Request::GrantSecret(v1::GrantSecret { id: id.into(), agent_id: "builder".into(), proof: None });
    let granted = apply_request(&mut client, &mut view, give(&connection.id)).await;
    assert!(failure(&granted).starts_with("Agents can't be given Demo (matt@example.com) itself."), "{granted:?}");

    // The proposer learns it has nothing yet; Rivet is asked to decide.
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    assert!(describe(&view.entries).iter().any(|line| line.starts_with(&format!("connect proposal {} (Connected)", proposal.id))));
    assert!(
        describe(&view.entries).iter().any(|line| line
            == "event from Claktory: The person connected Demo (matt@example.com) for your proposal. You don't have it: an admin agent decides what you get."),
        "{:?}",
        describe(&view.entries)
    );
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    view.follow(&mut client, |view| describe(&view.entries).iter().any(|line| line.contains("for Builder (builder)'s proposal"))).await;

    // A narrower secret is made from it, given, and used without being seen.
    let choices =
        apply_request(&mut client, &mut view, Request::ListConnectChoices(v1::ListConnectChoices { secret_id: connection.id.clone() }))
            .await;
    let Body::ConnectChoices(choices) = choices else { panic!("expected choices, got {choices:?}") };
    assert_eq!(choices.rules, "Pick one account and at least one permission.");
    let derive = |permissions: Vec<String>| {
        Request::DeriveSecret(v1::DeriveSecret {
            secret_id: connection.id.clone(),
            name: "Demo DNS".into(),
            permissions,
            resources: vec!["acme.com".into()],
            ..Default::default()
        })
    };
    assert_eq!(failure(&apply_request(&mut client, &mut view, derive(Vec::new())).await), "Pick at least one permission.");
    demo.due.store(true, Ordering::SeqCst);
    ok(&mut client, &mut view, derive(vec!["dns-edit".into()])).await;
    let made = vault(&view).secrets.into_iter().find(|secret| secret.name == "Demo DNS").expect("the secret made");
    let export = made.export.clone().expect("export");
    assert_eq!((export.connection_id.as_str(), export.resources.as_slice()), (connection.id.as_str(), ["acme.com".to_owned()].as_slice()));
    assert!(made.grants.is_empty(), "made, given to no one");
    apply_request(&mut client, &mut view, give(&made.id)).await;

    // It's due: renewed before the command gets it, the old value still hidden.
    demo.stale.store(true, Ordering::SeqCst);
    let first = "minted-1-0123456789";
    let command = format!(r#"{{"command":"echo \"$DEMO_TOKEN {first}\"","secrets":["Demo DNS"]}}"#);
    let output = call(&mut client, &mut view, "builder", "bash", &command).await;
    assert_eq!(output.lines().next(), Some("[secret Demo DNS/DEMO_TOKEN] [secret Demo DNS/DEMO_TOKEN]"), "{output}");
    let renewed = vault(&view).secrets.into_iter().find(|secret| secret.id == made.id).expect("still there");
    assert_eq!(renewed.version, 2);

    // Disconnected and revoked: everything made from it goes, at the service too.
    ok(&mut client, &mut view, Request::DisconnectService(v1::DisconnectService { secret_id: connection.id.clone(), revoke: true })).await;
    view.follow(&mut client, |view| vault(view).secrets.is_empty()).await;
    assert!(demo.revoked.load(Ordering::SeqCst));
    // The token renewed away went when it was renewed; the last one now.
    assert_eq!(*demo.removed.lock().expect("lock"), ["t1", "t3"]);
}

#[tokio::test]
async fn a_device_flow_connects_by_itself_and_a_connection_the_service_drops_is_connected_again() {
    let dir = temp_dir();
    let (addr, factory) = start_with(&dir, Scripted::signed_in()).await;
    factory.set_vault_passphrase_cost("argon2id m=1024 t=1 p=1");
    let cloudy = Arc::new(Demo::device());
    factory.add_connector(cloudy.clone(), &[]);
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let start = |region: &str, secret_id: &str| {
        let fields = [("region".to_owned(), region.to_owned())].into_iter().filter(|(_, value)| !value.is_empty()).collect();
        Request::StartConnect(v1::StartConnect { connector_id: "cloudy".into(), fields, secret_id: secret_id.into(), ..Default::default() })
    };
    assert_eq!(failure(&apply_request(&mut client, &mut view, start("", "")).await), "Enter Region to connect Cloudy.");
    let started = apply_request(&mut client, &mut view, start("eu", "")).await;
    let Body::ConnectStarted(started) = started else { panic!("expected a start, got {started:?}") };
    assert_eq!((started.url.as_str(), started.user_code.as_str()), ("https://eu.cloudy.example/device?user_code=ABCD-1234", "ABCD-1234"));
    view.follow(&mut client, |view| flow(view, &started.flow_id).is_some_and(|flow| flow.state() == ConnectFlowState::Connected)).await;
    let connection = vault(&view).secrets[0].clone();
    assert_eq!(connection.name, "Cloudy (matt@example.com)");

    // The service stops accepting it: it says so, and it's connected again in place.
    cloudy.revoked.store(true, Ordering::SeqCst);
    let choices =
        apply_request(&mut client, &mut view, Request::ListConnectChoices(v1::ListConnectChoices { secret_id: connection.id.clone() }))
            .await;
    assert_eq!(failure(&choices), "Cloudy (matt@example.com) needs to be connected again. Demo no longer accepts it.");
    view.follow(&mut client, |view| {
        vault(view).secrets[0].connection.as_ref().is_some_and(|info| info.status() == ConnectionStatus::NeedsReconnect)
    })
    .await;
    cloudy.revoked.store(false, Ordering::SeqCst);
    cloudy.polls.store(0, Ordering::SeqCst);
    let again = apply_request(&mut client, &mut view, start("eu", &connection.id)).await;
    let Body::ConnectStarted(again) = again else { panic!("expected a start, got {again:?}") };
    view.follow(&mut client, |view| flow(view, &again.flow_id).is_some_and(|flow| flow.state() == ConnectFlowState::Connected)).await;
    let secrets = vault(&view).secrets;
    assert_eq!(secrets.len(), 1, "connected again in place");
    assert_eq!(secrets[0].connection.as_ref().map(|info| info.status()), Some(ConnectionStatus::Ready));

    // Cancelled, then taken off the list.
    cloudy.polls.store(0, Ordering::SeqCst);
    let third = apply_request(&mut client, &mut view, start("eu", "")).await;
    let Body::ConnectStarted(third) = third else { panic!("expected a start, got {third:?}") };
    ok(&mut client, &mut view, Request::CancelConnect(v1::CancelConnect { flow_id: third.flow_id.clone() })).await;
    view.follow(&mut client, |view| flow(view, &third.flow_id).is_some_and(|flow| flow.state() == ConnectFlowState::Cancelled)).await;
    ok(&mut client, &mut view, Request::CancelConnect(v1::CancelConnect { flow_id: third.flow_id.clone() })).await;
    view.follow(&mut client, |view| flow(view, &third.flow_id).is_none()).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(vault(&view).secrets.len(), 1, "a cancelled flow connects nothing");
}
