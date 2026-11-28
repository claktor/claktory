//! The vault: secrets the person adds in the app, sealed in
//! <home>/vault/vault.db, which Claktory makes by itself and opens with the
//! key on disk. The person's app grants, reveals and deletes with no
//! passphrase; agents use what they're given. Values never reach the
//! workspace, factory.db, the log or a transcript.

use std::path::Path;
use std::sync::Arc;

use crate::common::{
    Client, Scripted, View, apply_request, create_agent, create_project, describe, last_workspace, send_message, start, start_with,
    temp_dir,
};
use factory_protocol::v1::{self, ErrorCode, client_message::Body as Request, entry::Kind, server_message::Body};
use factoryd::Factory;

const PASSPHRASE: &str = "correct horse battery";
const TOKEN: &str = "cf-0123456789abcdef-SECRETVALUE";

async fn open(dir: &Path) -> (std::net::SocketAddr, Arc<Factory>) {
    let (addr, factory) = start(dir).await;
    factory.set_vault_passphrase_cost("argon2id m=1024 t=1 p=1");
    (addr, factory)
}

fn entry(variable: &str, value: &str) -> v1::SecretEntry {
    v1::SecretEntry { variable: variable.into(), value: value.into(), file: false }
}

fn failure(body: &Body) -> (ErrorCode, String) {
    match body {
        Body::Error(error) => (error.code(), error.message.clone()),
        other => panic!("expected an error, got {other:?}"),
    }
}

fn vault(workspace: &v1::Workspace) -> v1::VaultState {
    workspace.vault.clone().expect("vault state")
}

async fn add(client: &mut Client, name: &str, variable: &str, value: &str) -> String {
    let secret = v1::CreateSecret { name: name.into(), entries: vec![entry(variable, value)], ..Default::default() };
    let events = client.ok(Request::CreateSecret(secret)).await;
    let state = vault(last_workspace(&events).expect("workspace"));
    state.secrets.iter().find(|secret| secret.name == name).expect("added").id.clone()
}

#[tokio::test]
async fn the_vault_is_there_from_the_start_and_the_person_adds_secrets() {
    let dir = temp_dir();
    let (addr, _factory) = open(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let state = vault(&workspace);
    assert!(state.exists && state.unlocked && !state.locked_mode && state.secrets.is_empty(), "{state:?}");
    assert!(dir.join("home/vault/vault.db").exists() && dir.join("home/vault/auto.key").exists());
    let made = client.request(Request::CreateVault(v1::CreateVault { passphrase: PASSPHRASE.into(), locked_mode: false })).await;
    assert_eq!(failure(&made.body).0, ErrorCode::InvalidRequest, "there's one already");

    let secret = v1::CreateSecret {
        name: "Cloudflare".into(),
        description: "The claktory.com zone.".into(),
        entries: vec![entry("CLOUDFLARE_API_TOKEN", TOKEN), entry("ZONE", "claktory")],
        ..Default::default()
    };
    let events = client.ok(Request::CreateSecret(secret)).await;
    let state = vault(last_workspace(&events).expect("workspace"));
    let cloudflare = &state.secrets[0];
    assert_eq!(
        (cloudflare.name.as_str(), cloudflare.description.as_str(), cloudflare.version),
        ("Cloudflare", "The claktory.com zone.", 1)
    );
    let variables: Vec<(&str, &str)> =
        cloudflare.variables.iter().map(|variable| (variable.name.as_str(), variable.hint.as_str())).collect();
    assert_eq!(variables, [("CLOUDFLARE_API_TOKEN", "ALUE"), ("ZONE", "")]);
    assert!(cloudflare.grants.is_empty(), "granted to no one");

    // A .env: previewed without values, then imported as one secret.
    let text = "RESEND_API_KEY=re_abcdefghijklmnop\nPIN=12\n";
    let preview = client.request(Request::PreviewEnv(v1::PreviewEnv { text: text.into() })).await;
    let Body::EnvPreview(preview) = &preview.body else { panic!("expected a preview, got {:?}", preview.body) };
    assert_eq!(preview.variables.iter().map(|variable| variable.name.as_str()).collect::<Vec<_>>(), ["RESEND_API_KEY", "PIN"]);
    assert_eq!(preview.warnings, ["PIN is too short to hide, so agents may see it if a command prints it."]);
    let import = v1::ImportEnv { text: text.into(), name: "Resend".into(), description: "Mail.".into(), one_per_variable: false };
    let events = client.ok(Request::ImportEnv(import)).await;
    let state = vault(last_workspace(&events).expect("workspace"));
    assert_eq!(state.secrets.iter().map(|secret| secret.name.as_str()).collect::<Vec<_>>(), ["Cloudflare", "Resend"]);
    assert_eq!(state.secrets[1].warnings, ["PIN is too short to hide, so agents may see it if a command prints it."]);

    // Renamed, then given a new value: a new version.
    let id = cloudflare.id.clone();
    let rename = v1::UpdateSecret { id: id.clone(), name: "CF".into(), description: "Zone.".into(), entries: Vec::new() };
    client.ok(Request::UpdateSecret(rename)).await;
    let update = v1::UpdateSecret {
        id: id.clone(),
        name: "CF".into(),
        description: "Zone.".into(),
        entries: vec![entry("CLOUDFLARE_API_TOKEN", "cf-new-token-value-0001")],
    };
    let events = client.ok(Request::UpdateSecret(update)).await;
    let state = vault(last_workspace(&events).expect("workspace"));
    let cf = state.secrets.iter().find(|secret| secret.id == id).expect("cf");
    assert_eq!((cf.name.as_str(), cf.version, cf.variables.len()), ("CF", 2, 1));

    // Nothing holds a value in the clear: not the vault, factory.db, the log or the workspace.
    for file in ["home/vault/vault.db", "home/factory.db", "home/config.json"] {
        let bytes = std::fs::read(dir.join(file)).unwrap_or_default();
        for value in [TOKEN, "cf-new-token-value-0001", "re_abcdefghijklmnop"] {
            assert!(!bytes.windows(value.len()).any(|window| window == value.as_bytes()), "{value} in {file}");
        }
    }
    assert!(!format!("{state:?}").contains("cf-new-token"));
}

#[tokio::test]
async fn the_person_grants_reveals_and_deletes_with_no_passphrase() {
    let dir = temp_dir();
    let (addr, _factory) = open(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    client.ok(create_project("Site", &dir.join("site"))).await;
    client.ok(create_agent("site", "Builder", "")).await;
    let id = add(&mut client, "Cloudflare", "TOKEN", TOKEN).await;
    let grant = || Request::GrantSecret(v1::GrantSecret { id: id.clone(), agent_id: "builder".into(), proof: None, fill_only: false });
    let reveal = || Request::RevealSecret(v1::RevealSecret { id: id.clone(), proof: None });

    let missing = client.request(Request::GrantSecret(v1::GrantSecret { id: id.clone(), agent_id: "nobody".into(), proof: None, fill_only: false })).await;
    assert_eq!(failure(&missing.body).0, ErrorCode::NotFound);
    let granted = client.request(grant()).await;
    let state = vault(last_workspace(&granted.events).expect("workspace"));
    let grants: Vec<&str> = state.secrets[0].grants.iter().map(|grant| grant.agent_id.as_str()).collect();
    assert_eq!(grants, ["builder"]);

    let shown = client.request(reveal()).await;
    let Body::SecretValue(shown) = &shown.body else { panic!("expected the value, got {:?}", shown.body) };
    assert_eq!((shown.entries[0].variable.as_str(), shown.entries[0].value.as_str()), ("TOKEN", TOKEN));

    // Taking back; a grant that isn't there says so.
    client.ok(Request::RevokeSecret(v1::RevokeSecret { id: id.clone(), agent_id: "builder".into(), stop_running_tasks: false })).await;
    let again = client
        .request(Request::RevokeSecret(v1::RevokeSecret { id: id.clone(), agent_id: "builder".into(), stop_running_tasks: false }))
        .await;
    assert_eq!(failure(&again.body).0, ErrorCode::NotGranted);

    let uses = client.request(Request::ListSecretUses(v1::ListSecretUses { id: String::new(), limit: 0 })).await;
    let Body::SecretUses(uses) = &uses.body else { panic!("expected uses, got {:?}", uses.body) };
    let kinds: Vec<(&str, &str, &str)> =
        uses.uses.iter().map(|used| (used.kind.as_str(), used.agent_id.as_str(), used.detail.as_str())).collect();
    // Revealed on this computer's app, by its device's name.
    let devices = factoryd::remote::devices(&crate::common::home(&dir)).expect("devices");
    let this_computer = devices.iter().find(|device| device.local).map(|device| device.name.as_str()).expect("this computer");
    assert_eq!(kinds, [("revoke", "builder", ""), ("reveal", "", this_computer), ("grant", "builder", "")]);

    // Deleted; granted again first, the grant goes with it.
    client.request(grant()).await;
    let deleted = client.request(Request::DeleteSecret(v1::DeleteSecret { id: id.clone(), proof: None })).await;
    assert!(vault(last_workspace(&deleted.events).expect("workspace")).secrets.is_empty());
}

#[tokio::test]
async fn a_key_the_person_holds_can_open_the_vault_after_a_restart() {
    let dir = temp_dir();
    let id;
    let recovery;
    {
        let (addr, _factory) = open(&dir).await;
        let (mut client, _, _) = Client::connect(addr).await;
        id = add(&mut client, "AWS", "AWS_SECRET_ACCESS_KEY", TOKEN).await;
        assert_eq!(
            dir.join("home/vault/auto.key").exists().then(|| crate::common::mode(&dir.join("home/vault/auto.key"))),
            Some(0o600)
        );
        // With only the key on disk, locked mode would lock it for good.
        let refused = client.request(Request::SetVaultLockedMode(v1::SetVaultLockedMode { locked_mode: true, proof: None })).await;
        assert!(failure(&refused.body).1.contains("neither"), "{:?}", refused.body);
        // A recovery key first: then it can.
        let key = client.request(Request::NewRecoveryKey(v1::NewRecoveryKey { proof: None, ..Default::default() })).await;
        let Body::RecoveryKey(key) = key.body else { panic!("expected a recovery key") };
        recovery = key.key;
        let locked = client.request(Request::SetVaultLockedMode(v1::SetVaultLockedMode { locked_mode: true, proof: None })).await;
        assert!(vault(last_workspace(&locked.events).expect("workspace")).locked_mode);
        assert!(!dir.join("home/vault/auto.key").exists());
    }

    // After a restart it's locked until the person's key opens it.
    let (addr, _factory) = open(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let state = vault(&workspace);
    assert!(state.exists && !state.unlocked && state.locked_mode);
    let refused = client.request(Request::RevealSecret(v1::RevealSecret { id: id.clone(), proof: None })).await;
    assert_eq!(failure(&refused.body).0, ErrorCode::VaultLocked);
    let wrong = client.request(Request::UnlockVault(v1::UnlockVault { passphrase: PASSPHRASE.into() })).await;
    assert_eq!(failure(&wrong.body), (ErrorCode::WrongProof, "That recovery key isn't right.".to_owned()));
    // The recovery key sets a passphrase, and opens it.
    let changed = client
        .request(Request::ChangePassphrase(v1::ChangePassphrase { old_passphrase: recovery.clone(), new_passphrase: PASSPHRASE.into() }))
        .await;
    assert!(matches!(changed.body, Body::VaultSession(_)), "{:?}", changed.body);
    assert!(matches!(client.request(Request::RevealSecret(v1::RevealSecret { id, proof: None })).await.body, Body::SecretValue(_)));
}

#[tokio::test]
async fn a_deleted_agent_keeps_no_grants() {
    let dir = temp_dir();
    let (addr, _factory) = open(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    client.ok(create_project("Site", &dir.join("site"))).await;
    client.ok(create_agent("site", "Builder", "")).await;
    let id = add(&mut client, "AWS", "KEY", TOKEN).await;
    client.request(Request::GrantSecret(v1::GrantSecret { id, agent_id: "builder".into(), proof: None, fill_only: false })).await;
    let events = client.ok(Request::DeleteAgent(v1::DeleteAgent { agent_id: "builder".into() })).await;
    assert!(vault(last_workspace(&events).expect("workspace")).secrets[0].grants.is_empty());
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

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == v1::AgentStatus::Idle && agent.queued_messages == 0
}

fn events(view: &View) -> Vec<String> {
    describe(&view.entries).into_iter().filter(|line| line.starts_with("event from")).collect()
}

/// Every file under `dir` that holds `value`.
fn holding(dir: &Path, value: &str) -> Vec<String> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(holding(&path, value));
        } else if std::fs::read(&path).is_ok_and(|bytes| bytes.windows(value.len()).any(|window| window == value.as_bytes())) {
            found.push(path.display().to_string());
        }
    }
    found
}

#[tokio::test]
async fn agents_use_the_secrets_they_are_given_without_seeing_them() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let (system_prompt, tools) = (models.system_prompt.clone(), models.tools.clone());
    let (addr, factory) = start_with(&dir, models).await;
    factory.set_vault_passphrase_cost("argon2id m=1024 t=1 p=1");
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;
    for (name, variable, value) in [("Cloudflare", "CLOUDFLARE_API_TOKEN", TOKEN), ("Resend", "RESEND_API_KEY", "re_abcdefghijklmnop")] {
        let secret = v1::CreateSecret { name: name.into(), entries: vec![entry(variable, value)], ..Default::default() };
        apply_request(&mut client, &mut view, Request::CreateSecret(secret)).await;
    }
    let id = vault(&view.workspace).secrets[0].id.clone();

    // Given none, its bash takes no secrets.
    assert!(call(&mut client, &mut view, "builder", "bash", r#"{"command":"echo hello"}"#).await.starts_with("hello\n"));
    assert!(!tools.lock().expect("lock").contains("\"secrets\""));

    // Given Cloudflare: it hears, and its bash runs with it, the value hidden.
    let grant = v1::GrantSecret { id: id.clone(), agent_id: "builder".into(), proof: None, fill_only: false };
    apply_request(&mut client, &mut view, Request::GrantSecret(grant)).await;
    let command = r#"{"command":"echo \"token=$CLOUDFLARE_API_TOKEN\"","secrets":["Cloudflare"]}"#;
    let output = call(&mut client, &mut view, "builder", "bash", command).await;
    assert!(output.starts_with("token=[secret Cloudflare/CLOUDFLARE_API_TOKEN]\n"), "{output}");
    assert!(
        events(&view).contains(
            &"event from Claktory: You can now use the secrets (pass names in bash's `secrets`): Cloudflare (sets CLOUDFLARE_API_TOKEN)."
                .to_owned()
        ),
        "{:?}",
        events(&view)
    );
    assert!(tools.lock().expect("lock").contains("\"secrets\""), "its bash takes secrets now");
    let prompt = system_prompt.lock().expect("lock").clone();
    assert!(prompt.contains("## Secrets") && prompt.contains("- Cloudflare (sets CLOUDFLARE_API_TOKEN)"), "{prompt}");
    assert!(!prompt.contains("Resend"), "nothing of secrets it wasn't given");
    // A secret it wasn't given is refused as one the vault doesn't have.
    let refused = call(&mut client, &mut view, "builder", "bash", r#"{"command":"true","secrets":["Resend"]}"#).await;
    assert!(refused.contains("\"Resend\" isn't one of your secrets. Yours: Cloudflare."), "{refused}");
    let missing = call(&mut client, &mut view, "builder", "bash", r#"{"command":"true","secrets":["Stripe"]}"#).await;
    assert_eq!(missing.replace("Stripe", "Resend"), refused, "the same either way");

    // A task that got it shows in the vault; revoking it can stop the task.
    let command = r#"{"command":"echo $CLOUDFLARE_API_TOKEN; sleep 30","run_in_background":true,"secrets":["Cloudflare"]}"#;
    assert!(call(&mut client, &mut view, "builder", "bash", command).await.starts_with("Started task-1"));
    view.follow(&mut client, |view| vault(&view.workspace).secrets[0].grants.first().is_some_and(|grant| grant.running_tasks == 1)).await;
    assert_eq!(vault(&view.workspace).secrets[0].last_used_by, "builder");
    let open = v1::OpenPanel { extension_id: "tasks".into(), agent_id: "builder".into() };
    apply_request(&mut client, &mut view, Request::OpenPanel(open)).await;
    let tail = |view: &View| match view.extension_view.as_ref().and_then(|view| view.body.clone()) {
        Some(v1::extension_view::Body::Tasks(tasks)) => tasks.tasks.first().map(|task| task.log_tail.clone()).unwrap_or_default(),
        _ => String::new(),
    };
    view.follow(&mut client, |view| !tail(view).is_empty()).await;
    assert_eq!(tail(&view), "[secret Cloudflare/CLOUDFLARE_API_TOKEN]");
    let revoke = v1::RevokeSecret { id: id.clone(), agent_id: "builder".into(), stop_running_tasks: true };
    apply_request(&mut client, &mut view, Request::RevokeSecret(revoke)).await;
    view.follow(&mut client, |view| {
        events(view).iter().any(|line| line.contains("You can no longer use the secrets: Cloudflare.")) && idle(view, "builder")
    })
    .await;
    view.follow(&mut client, |view| events(view).iter().any(|line| line.contains("task-1 finished")) && idle(view, "builder")).await;
    assert!(events(&view).iter().any(|line| line.contains("task-1 finished (stopped by the person, ")), "{:?}", events(&view));
    let refused = call(&mut client, &mut view, "builder", "bash", r#"{"command":"true","secrets":["Cloudflare"]}"#).await;
    assert!(refused.contains("\"Cloudflare\" isn't one of your secrets. You have none yet."), "{refused}");

    // Each use is logged, with the command.
    let uses = client.request(Request::ListSecretUses(v1::ListSecretUses { id: id.clone(), limit: 0 })).await;
    let Body::SecretUses(uses) = &uses.body else { panic!("expected uses, got {:?}", uses.body) };
    let kinds: Vec<(&str, &str, &str)> =
        uses.uses.iter().map(|used| (used.kind.as_str(), used.agent_id.as_str(), used.task_id.as_str())).collect();
    assert_eq!(kinds, [("revoke", "builder", ""), ("use", "builder", "task-1"), ("use", "builder", ""), ("grant", "builder", "")]);

    // No value anywhere on disk: transcripts, factory.db, the vault, and the
    // background task's own log, written through `factoryd redact`.
    assert!(dir.join("home/agents").exists());
    assert_eq!(holding(&dir, TOKEN), Vec::<String>::new());

    // A value in a message between agents is hidden too.
    let message = format!(r#"{{"to":"writer","text":"the token is {TOKEN}"}}"#);
    call(&mut client, &mut view, "builder", "agents_send", &message).await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "writer".into() })).await;
    view.follow(&mut client, |view| !events(view).is_empty() && idle(view, "writer")).await;
    assert_eq!(events(&view), ["event from Builder: the token is [secret Cloudflare/CLOUDFLARE_API_TOKEN]"]);
}

#[tokio::test]
async fn rivet_gives_and_takes_back_secrets_when_the_person_asks() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let tools = models.tools.clone();
    let (addr, _factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let rivet = v1::UpdateAgent {
        agent_id: "rivet".into(),
        model: Some(crate::common::MODEL.into()),
        effort: Some("off".into()),
        ..Default::default()
    };
    apply_request(&mut client, &mut view, Request::UpdateAgent(rivet)).await;
    let id = add(&mut client, "Cloudflare", "CLOUDFLARE_API_TOKEN", TOKEN).await;
    add(&mut client, "Resend", "RESEND_API_KEY", "re_abcdefghijklmnop").await;

    // Rivet sees what each is for and who has it, never a value.
    let listed = call(&mut client, &mut view, "rivet", "secrets_list", "{}").await;
    assert_eq!(listed, "Cloudflare (sets CLOUDFLARE_API_TOKEN) — given to no one\nResend (sets RESEND_API_KEY) — given to no one");
    assert!(tools.lock().expect("lock").contains("secrets_grant"));

    // It gives one, which Builder can use, and the vault logs who gave it.
    let given = call(&mut client, &mut view, "rivet", "secrets_grant", r#"{"secret":"cloudflare","agents":["Builder"]}"#).await;
    assert_eq!(given, "Gave Cloudflare to Builder (builder).");
    let details = view.entries.iter().rev().find_map(|entry| match &entry.kind {
        Some(Kind::ToolCall(call)) if call.name == "secrets_grant" => Some(call.details.clone()),
        _ => None,
    });
    let details: serde_json::Value = serde_json::from_str(&details.expect("the call")).expect("details");
    assert_eq!(details, serde_json::json!({"secret": "Cloudflare", "agents": [{"id": "builder", "name": "Builder"}]}));
    assert!(vault(&view.workspace).secrets[0].grants.iter().any(|grant| grant.agent_id == "builder"));
    let output =
        call(&mut client, &mut view, "builder", "bash", r#"{"command":"echo $CLOUDFLARE_API_TOKEN","secrets":["Cloudflare"]}"#).await;
    assert!(output.starts_with("[secret Cloudflare/CLOUDFLARE_API_TOKEN]"), "{output}");

    // Not to itself; and agents that aren't admins have no such tools.
    let own = call(&mut client, &mut view, "rivet", "secrets_grant", r#"{"secret":"Resend","agents":["rivet"]}"#).await;
    assert!(own.contains("can't give a secret to yourself"), "{own}");
    let refused = call(&mut client, &mut view, "builder", "secrets_grant", r#"{"secret":"Resend","agents":["builder"]}"#).await;
    assert!(!refused.starts_with("Gave"), "{refused}");
    assert!(!tools.lock().expect("lock").contains("secrets_grant"), "Builder's tools");

    // It takes it back, stopping Builder's task that got it.
    let command = r#"{"command":"sleep 30","run_in_background":true,"secrets":["Cloudflare"]}"#;
    assert!(call(&mut client, &mut view, "builder", "bash", command).await.starts_with("Started task-1"));
    let taken =
        call(&mut client, &mut view, "rivet", "secrets_revoke", r#"{"secret":"Cloudflare","agents":["builder"],"stop_tasks":true}"#).await;
    assert_eq!(taken, "Took Cloudflare back from Builder (builder), stopping task-1.");
    assert!(vault(&view.workspace).secrets[0].grants.is_empty());

    let uses = client.request(Request::ListSecretUses(v1::ListSecretUses { id, limit: 0 })).await;
    let Body::SecretUses(uses) = &uses.body else { panic!("expected uses, got {:?}", uses.body) };
    let kinds: Vec<(&str, &str, &str)> =
        uses.uses.iter().map(|used| (used.kind.as_str(), used.agent_id.as_str(), used.detail.as_str())).collect();
    assert_eq!(kinds[0], ("revoke", "builder", "rivet"));
    assert_eq!(kinds.last(), Some(&("grant", "builder", "rivet")));
}

/// An agent gives up a secret on its own, with no card: only its
/// own grant goes, as a revoke takes it, and History says so with its reason.
#[tokio::test]
async fn an_agent_gives_up_its_own_secret_and_history_says_why() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;
    let cloudflare = add(&mut client, "Cloudflare", "CLOUDFLARE_API_TOKEN", TOKEN).await;
    let resend = add(&mut client, "Resend", "RESEND_API_KEY", "re_abcdefghijklmnop").await;
    for (id, agent) in [(&cloudflare, "builder"), (&cloudflare, "writer"), (&resend, "writer")] {
        let grant = v1::GrantSecret { id: id.clone(), agent_id: agent.into(), proof: None, fill_only: false };
        apply_request(&mut client, &mut view, Request::GrantSecret(grant)).await;
    }

    // Only what it holds, said the same whether or not the vault has it;
    // and it says why.
    let others = call(&mut client, &mut view, "builder", "secrets_give_up", r#"{"secret":"Resend","reason":"Not mine."}"#).await;
    assert_eq!(others, "\"Resend\" isn't one of your secrets. Yours: Cloudflare. Nothing was changed.");
    let missing = call(&mut client, &mut view, "builder", "secrets_give_up", r#"{"secret":"Stripe","reason":"Not mine."}"#).await;
    assert_eq!(missing.replace("Stripe", "Resend"), others);
    let why = call(&mut client, &mut view, "builder", "secrets_give_up", r#"{"secret":"Cloudflare","reason":" "}"#).await;
    assert!(why.starts_with("Say why you're giving it up"), "{why}");

    // Given up while a task that got it runs: the task is left, and said so.
    let command = r#"{"command":"sleep 30","run_in_background":true,"secrets":["Cloudflare"]}"#;
    assert!(call(&mut client, &mut view, "builder", "bash", command).await.starts_with("Started task-1"));
    let said =
        call(&mut client, &mut view, "builder", "secrets_give_up", r#"{"secret":"cloudflare","reason":"Done deploying the site."}"#).await;
    assert!(said.starts_with("Gave up Cloudflare: you can't use it from your next command on. The person sees it in History"), "{said}");
    assert!(
        said.contains("keep running with what they got: task-1.") && said.ends_with("To get it back, ask with secrets_request."),
        "{said}"
    );
    let grants = |view: &View, id: &str| -> Vec<String> {
        let secret = vault(&view.workspace).secrets.into_iter().find(|secret| secret.id == id).expect("secret");
        secret.grants.into_iter().map(|grant| grant.agent_id).collect()
    };
    assert_eq!(grants(&view, &cloudflare), ["writer"], "Writer keeps its own");
    let refused = call(&mut client, &mut view, "builder", "bash", r#"{"command":"true","secrets":["Cloudflare"]}"#).await;
    assert!(refused.contains("\"Cloudflare\" isn't one of your secrets."), "{refused}");
    let uses = client.request(Request::ListSecretUses(v1::ListSecretUses { id: cloudflare.clone(), limit: 0 })).await;
    let Body::SecretUses(uses) = &uses.body else { panic!("expected uses, got {:?}", uses.body) };
    assert_eq!(
        (uses.uses[0].kind.as_str(), uses.uses[0].agent_id.as_str(), uses.uses[0].detail.as_str()),
        ("revoke", "builder", "builder")
    );

    // With stop_tasks, its tasks that got it stop.
    let command = r#"{"command":"sleep 30","run_in_background":true,"secrets":["Resend"]}"#;
    assert!(call(&mut client, &mut view, "writer", "bash", command).await.starts_with("Started task-1"));
    let said = call(
        &mut client,
        &mut view,
        "writer",
        "secrets_give_up",
        r#"{"secret":"Resend","reason":"Only Builder sends mail now.","stop_tasks":true}"#,
    )
    .await;
    assert!(said.contains("Stopped your background tasks that got it: task-1."), "{said}");
    assert!(grants(&view, &resend).is_empty() && grants(&view, &cloudflare) == ["writer"]);

    // History has both, newest first, each at its call.
    let list = v1::ListNeedsYouHistory { query: String::new(), before_ms: 0, limit: 0 };
    let Body::NeedsYouHistory(history) = apply_request(&mut client, &mut view, Request::ListNeedsYouHistory(list)).await else {
        panic!("history")
    };
    let given: Vec<(&str, &str, &str, v1::NeedsYouOutcome, bool)> = history
        .records
        .iter()
        .filter(|record| record.kind() == v1::NeedsYouRecordKind::SecretGivenUp)
        .map(|record| (record.agent_id.as_str(), record.title.as_str(), record.agent_reason.as_str(), record.outcome(), record.seq > 0))
        .collect();
    assert_eq!(
        given,
        [
            ("writer", "Writer gave up Resend", "Only Builder sends mail now.", v1::NeedsYouOutcome::Changed, true),
            ("builder", "Builder gave up Cloudflare", "Done deploying the site.", v1::NeedsYouOutcome::Changed, true),
        ]
    );
    let builder = history.records.iter().find(|record| record.agent_id == "builder").expect("builder's");
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    let at = view.entries.iter().find(|entry| entry.seq == builder.seq).and_then(|entry| match &entry.kind {
        Some(Kind::ToolCall(call)) => Some(call.name.clone()),
        _ => None,
    });
    assert_eq!(at.as_deref(), Some("secrets_give_up"));
    // Found by its reason.
    let list = v1::ListNeedsYouHistory { query: "deploying".into(), before_ms: 0, limit: 0 };
    let Body::NeedsYouHistory(found) = apply_request(&mut client, &mut view, Request::ListNeedsYouHistory(list)).await else {
        panic!("history")
    };
    assert_eq!(found.records.iter().map(|record| record.title.as_str()).collect::<Vec<_>>(), ["Builder gave up Cloudflare"]);
}

fn requests(view: &View) -> Vec<String> {
    describe(&view.entries).into_iter().filter(|line| line.starts_with("secret request")).collect()
}

/// The requests waiting in the selected agent's conversation, oldest first.
fn waiting(view: &View) -> Vec<v1::SecretRequest> {
    view.entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::SecretRequest(request)) if request.state() == v1::SecretRequestState::Pending => Some(request.clone()),
            _ => None,
        })
        .collect()
}

fn decide(agent: &str, id: &str, approve: bool, entries: Vec<v1::SecretEntry>, comment: &str) -> v1::DecideSecretRequest {
    v1::DecideSecretRequest {
        agent_id: agent.into(),
        request_id: id.into(),
        approve,
        proof: None,
        entries,
        comment: comment.into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn agents_describe_the_secret_they_need_and_the_person_gives_one_adds_one_or_dismisses() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let system_prompt = models.system_prompt.clone();
    let (addr, _factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let cloudflare = add(&mut client, "Cloudflare", "CLOUDFLARE_API_TOKEN", TOKEN).await;

    // It describes what it needs; the answer is the same whether or not the
    // vault has such a secret.
    let need = r#"{"name":"Cloudflare token","description":"DNS edit on claktory.com","variables":[{"name":"CLOUDFLARE_API_TOKEN"}],"reason":"To deploy the site."}"#;
    let asked = call(&mut client, &mut view, "builder", "secrets_request", need).await;
    assert_eq!(asked, "Asked for Cloudflare token; the person will give it to you or dismiss the request, and you'll be told.");
    let prompt = system_prompt.lock().expect("lock").clone();
    assert!(prompt.contains("secrets_request") && !prompt.contains("Cloudflare (sets"), "{prompt}");
    let pending = waiting(&view);
    assert_eq!(pending.len(), 1);
    assert!(vault(&view.workspace).requests.is_empty(), "requests show only in the conversation");
    let first = pending[0].clone();
    assert_eq!((first.secret_id.as_str(), first.secret_name.as_str()), ("", "Cloudflare token"));
    let wanted = first.new_secret.clone().expect("what it needs");
    assert_eq!((wanted.description.as_str(), wanted.variables[0].name.as_str()), ("DNS edit on claktory.com", "CLOUDFLARE_API_TOKEN"));
    assert_eq!(requests(&view), [format!("secret request {} (Pending): To deploy the site.", first.id)]);
    let again = call(&mut client, &mut view, "builder", "secrets_request", need).await;
    assert!(again.contains("waiting for the person"), "{again}");
    // From an agent whose tools are from before: a name it guessed reads as
    // a description, whether the vault has it or not.
    let named = call(&mut client, &mut view, "builder", "secrets_request", r#"{"secret":"Cloudflare","reason":"Deploys."}"#).await;
    let guessed = call(&mut client, &mut view, "builder", "secrets_request", r#"{"secret":"Stripe","reason":"Deploys."}"#).await;
    assert_eq!(guessed.replace("Stripe", "Cloudflare"), named);
    assert!(named.starts_with("Asked for Cloudflare;"), "{named}");
    for request in waiting(&view).into_iter().filter(|request| request.id != first.id) {
        apply_request(&mut client, &mut view, Request::DecideSecretRequest(decide("builder", &request.id, false, Vec::new(), ""))).await;
    }
    // Dismissed with nothing more, they're told quietly:
    // they wait for Builder's next wake rather than waking it.
    view.follow(&mut client, |view| view.agent("builder").quiet_waiting == 2).await;
    assert!(idle(&view, "builder") && !events(&view).iter().any(|line| line.contains("dismissed")), "{:?}", events(&view));
    let quietly_dismissed = |view: &View| {
        view.entries
            .iter()
            .filter(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.quiet && event.summary.contains("dismissed your request")))
            .count()
    };

    // Given one of the vault's: it hears what it got, and only that.
    let nothing =
        apply_request(&mut client, &mut view, Request::DecideSecretRequest(decide("builder", &first.id, true, Vec::new(), ""))).await;
    assert!(failure(&nothing).1.contains("Choose a secret"), "{nothing:?}");
    let give = v1::DecideSecretRequest { secret_id: cloudflare.clone(), ..decide("builder", &first.id, true, Vec::new(), "") };
    apply_request(&mut client, &mut view, Request::DecideSecretRequest(give)).await;
    view.follow(&mut client, |view| events(view).iter().any(|line| line.contains("gave you Cloudflare")) && idle(view, "builder")).await;
    assert!(
        events(&view).iter().any(|line| line == "event from Claktory: The person gave you Cloudflare: pass it in bash's `secrets`."),
        "{:?}",
        events(&view)
    );
    assert_eq!(requests(&view)[0], format!("secret request {} (Granted): To deploy the site.", first.id));
    assert_eq!(quietly_dismissed(&view), 2, "the dismissals came with the next wake");
    let decided = view.entries.iter().find_map(|entry| match &entry.kind {
        Some(Kind::SecretRequest(request)) if request.id == first.id => Some(request.clone()),
        _ => None,
    });
    let decided = decided.expect("the entry");
    assert_eq!((decided.given_secret_id.as_str(), decided.given_secret_name.as_str()), (cloudflare.as_str(), "Cloudflare"));
    let output =
        call(&mut client, &mut view, "builder", "bash", r#"{"command":"echo $CLOUDFLARE_API_TOKEN","secrets":["Cloudflare"]}"#).await;
    assert!(output.starts_with("[secret Cloudflare/CLOUDFLARE_API_TOKEN]"), "{output}");

    // A new one, from the description: the person names it and fills in its value.
    let new =
        r#"{"name":"Mail","description":"Mail for the site.","variables":[{"name":"RESEND_API_KEY"}],"reason":"To send sign-up mail."}"#;
    call(&mut client, &mut view, "builder", "secrets_request", new).await;
    let request = waiting(&view).pop().expect("the request");
    let add_new = v1::DecideSecretRequest {
        name: "Resend".into(),
        ..decide("builder", &request.id, true, vec![entry("RESEND_API_KEY", "re_abcdefghijklmnop")], "")
    };
    apply_request(&mut client, &mut view, Request::DecideSecretRequest(add_new)).await;
    let resend = vault(&view.workspace).secrets.into_iter().find(|secret| secret.name == "Resend").expect("made");
    assert_eq!((resend.description.as_str(), resend.grants[0].agent_id.as_str()), ("Mail for the site.", "builder"));

    // Dismissed, with a comment it reads.
    let other = r#"{"name":"Stripe","description":"Payments.","reason":"Payments."}"#;
    call(&mut client, &mut view, "builder", "secrets_request", other).await;
    let id = waiting(&view).pop().expect("the request").id;
    apply_request(&mut client, &mut view, Request::DecideSecretRequest(decide("builder", &id, false, Vec::new(), "Not yet."))).await;
    view.follow(&mut client, |view| {
        events(view).iter().any(|line| line.contains("dismissed your request for Stripe")) && idle(view, "builder")
    })
    .await;
    assert!(
        events(&view).iter().any(|line| line == "event from Claktory: The person dismissed your request for Stripe."),
        "{:?}",
        events(&view)
    );
    // The person's words reach it as theirs, on a line of their own,
    // to read rather than take as a no (needs-you-cards §7).
    assert!(view.entries.iter().any(|entry| matches!(&entry.kind,
        Some(Kind::Event(event)) if event.text.starts_with("The person dismissed your request for Stripe and says:\n\nNot yet.\n\nRead what they say. \
            If they ask something, say yes, or say what to ask for instead, ask again with that. Only a plain no ends it.\n\nYou asked because: Payments."))));
    let twice = client.request(Request::DecideSecretRequest(decide("builder", &id, true, Vec::new(), ""))).await;
    assert!(failure(&twice.body).1.contains("decided already"), "{:?}", twice.body);
    drop(client);
    drop(_factory);

    // After a restart, the conversation shows each as decided.
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    let states: Vec<String> = requests(&view).iter().map(|line| line.split(' ').nth(3).unwrap_or_default().to_owned()).collect();
    assert_eq!(states, ["(Granted):", "(Dismissed):", "(Dismissed):", "(Granted):", "(Dismissed):"]);
}

#[tokio::test]
async fn a_request_left_unanswered_expires_and_the_agent_is_told() {
    let dir = temp_dir();
    let (addr, factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let need = r#"{"name":"Stripe","description":"Payments.","variables":[{"name":"STRIPE_KEY"}],"reason":"To take payments."}"#;
    call(&mut client, &mut view, "builder", "secrets_request", need).await;
    let asked = waiting(&view).pop().expect("the request");

    // However busy the agent is, it waits for the person (a busy
    // agent's requests expired before Matt opened its conversation).
    for _ in 0..12 {
        let before = view.entries.len();
        apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
        view.follow(&mut client, |view| view.entries.len() > before + 1 && idle(view, "builder")).await;
    }
    // A minute on (the first check) and 11 hours on, it still waits; a
    // request round trip brings any change the checks made.
    factory.expire_secret_requests_after(60_000);
    factory.expire_secret_requests_after(11 * 3_600_000);
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    assert_eq!(waiting(&view).len(), 1, "waiting after 12 runs and 11 hours");
    assert_eq!(requests(&view), [format!("secret request {} (Pending): To take payments.", asked.id)]);

    // 12 hours on: expired, and the agent is told.
    factory.expire_secret_requests_after(12 * 3_600_000);
    let told = "event from Claktory: Your request for Stripe expired.";
    view.follow(&mut client, |view| events(view).iter().any(|line| line == told) && idle(view, "builder")).await;
    assert_eq!(requests(&view), [format!("secret request {} (Expired): To take payments.", asked.id)]);
    let late = client.request(Request::DecideSecretRequest(decide("builder", &asked.id, false, Vec::new(), ""))).await;
    assert_eq!(failure(&late.body).1, "This request expired.");
}

/// K1: a secret given in Settings → Secrets answers the request it
/// was asked for, as giving it on the card would: the card closes and the
/// agent is told, idle or not. A secret for none of its requests wakes it
/// all the same, and closes nothing.
#[tokio::test]
async fn a_secret_given_in_settings_answers_the_request_waiting_for_it() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let github = r#"{"name":"GitHub token","description":"Push to the repo.","variables":[{"name":"GITHUB_TOKEN"}],"reason":"To push."}"#;
    call(&mut client, &mut view, "builder", "secrets_request", github).await;
    let stripe = r#"{"name":"Stripe","description":"Payments.","variables":[{"name":"STRIPE_KEY"}],"reason":"To take payments."}"#;
    call(&mut client, &mut view, "builder", "secrets_request", stripe).await;
    let asked = waiting(&view);
    assert_eq!(asked.len(), 2);

    // Named otherwise, but it sets what the agent asked for.
    let id = add(&mut client, "GitHub", "GITHUB_TOKEN", TOKEN).await;
    let grant = v1::GrantSecret { id, agent_id: "builder".into(), proof: None, fill_only: false };
    apply_request(&mut client, &mut view, Request::GrantSecret(grant)).await;
    view.follow(&mut client, |view| events(view).iter().any(|line| line.contains("gave you GitHub")) && idle(view, "builder")).await;
    assert_eq!(requests(&view)[0], format!("secret request {} (Granted): To push.", asked[0].id));
    assert_eq!(waiting(&view).iter().map(|request| request.id.clone()).collect::<Vec<_>>(), [asked[1].id.clone()]);

    // Another secret: Stripe's request still waits, and the agent hears.
    let fly = add(&mut client, "Fly", "FLY_API_TOKEN", "fly-test").await;
    let grant = v1::GrantSecret { id: fly, agent_id: "builder".into(), proof: None, fill_only: false };
    apply_request(&mut client, &mut view, Request::GrantSecret(grant)).await;
    view.follow(&mut client, |view| events(view).iter().any(|line| line.contains("gave you Fly")) && idle(view, "builder")).await;
    let told = events(&view).into_iter().find(|line| line.contains("gave you Fly")).expect("told");
    assert_eq!(waiting(&view).len(), 1, "{told}");
    // Each give is said once: the answer, not also the note of what changed.
    let notes: Vec<String> = events(&view).into_iter().filter(|line| line.contains("You can now use the secrets")).collect();
    assert!(notes.is_empty(), "{notes:?}");
}

/// An agent's Secrets panel shows its grants from the vault's
/// state, so every way of giving a secret has to land there, under the
/// agent's id, and stay after a restart.
#[tokio::test]
async fn every_way_of_giving_a_secret_shows_in_the_agents_grants() {
    let dir = temp_dir();
    let (addr, factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Ops", &dir.join("ops"))).await;
    // Named unlike its id, as Arjun (devops) is.
    apply_request(&mut client, &mut view, create_agent("ops", "Devops", "")).await;
    let renamed = v1::UpdateAgent { agent_id: "devops".into(), name: Some("Arjun".into()), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdateAgent(renamed)).await;
    let rivet = v1::UpdateAgent {
        agent_id: "rivet".into(),
        model: Some(crate::common::MODEL.into()),
        effort: Some("off".into()),
        ..Default::default()
    };
    apply_request(&mut client, &mut view, Request::UpdateAgent(rivet)).await;
    let store = add(&mut client, "App Store Connect", "ASC_KEY", "asc-0123456789abcdef").await;
    add(&mut client, "Hetzner", "HCLOUD_TOKEN", "hz-0123456789abcdef").await;
    let dns = add(&mut client, "CF_CLAKTORY_DNS", "CLOUDFLARE_API_TOKEN", TOKEN).await;
    let given = |view: &View| -> Vec<String> {
        let mut names: Vec<String> = vault(&view.workspace)
            .secrets
            .into_iter()
            .filter(|secret| secret.grants.iter().any(|grant| grant.agent_id == "devops"))
            .map(|secret| secret.name)
            .collect();
        names.sort();
        names
    };

    // From Settings → Secrets.
    apply_request(&mut client, &mut view, Request::GrantSecret(v1::GrantSecret { id: store, agent_id: "devops".into(), proof: None, fill_only: false }))
        .await;
    assert_eq!(given(&view), ["App Store Connect"]);
    // By Rivet, by name.
    let said = call(&mut client, &mut view, "rivet", "secrets_grant", r#"{"secret":"Hetzner","agents":["Arjun"]}"#).await;
    assert_eq!(said, "Gave Hetzner to Arjun (devops).");
    view.follow(&mut client, |view| given(view).len() == 2).await;
    // By deciding its request, with one the vault has, and with a new one.
    for need in [
        r#"{"name":"DNS","description":"DNS edit","variables":[{"name":"CLOUDFLARE_API_TOKEN"}],"reason":"Deploys."}"#,
        r#"{"name":"Mail","description":"Mail","variables":[{"name":"RESEND_API_KEY"}],"reason":"Mail."}"#,
    ] {
        call(&mut client, &mut view, "devops", "secrets_request", need).await;
    }
    let asked = waiting(&view);
    let existing = v1::DecideSecretRequest { secret_id: dns, ..decide("devops", &asked[0].id, true, Vec::new(), "") };
    apply_request(&mut client, &mut view, Request::DecideSecretRequest(existing)).await;
    let new = v1::DecideSecretRequest {
        name: "Resend".into(),
        ..decide("devops", &asked[1].id, true, vec![entry("RESEND_API_KEY", "re_abcdefghijklmnop")], "")
    };
    apply_request(&mut client, &mut view, Request::DecideSecretRequest(new)).await;
    view.follow(&mut client, |view| given(view).len() == 4).await;
    assert_eq!(given(&view), ["App Store Connect", "CF_CLAKTORY_DNS", "Hetzner", "Resend"]);
    drop(client);
    drop(factory);

    // Days later, after a restart: still there.
    let (addr, _factory) = start(&dir).await;
    let (_client, workspace, _) = Client::connect(addr).await;
    let view = View { workspace, ..View::default() };
    assert_eq!(given(&view), ["App Store Connect", "CF_CLAKTORY_DNS", "Hetzner", "Resend"]);
}

#[tokio::test]
async fn a_secret_given_for_a_request_also_sets_the_names_the_agent_asked_for() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let dns = add(&mut client, "CF_CLAKTORY_DNS", "CF_CLAKTORY_DNS", TOKEN).await;
    let need = r#"{"name":"Cloudflare","description":"DNS edit on claktory.com","variables":[{"name":"CLOUDFLARE_API_TOKEN"}],"reason":"To deploy."}"#;
    call(&mut client, &mut view, "builder", "secrets_request", need).await;
    let asked = waiting(&view).pop().expect("the request");

    // One name missing, one variable unused: paired by themselves.
    let give = v1::DecideSecretRequest { secret_id: dns.clone(), ..decide("builder", &asked.id, true, Vec::new(), "") };
    apply_request(&mut client, &mut view, Request::DecideSecretRequest(give)).await;
    view.follow(&mut client, |view| events(view).iter().any(|line| line.contains("gave you CF_CLAKTORY_DNS")) && idle(view, "builder"))
        .await;
    let secret = vault(&view.workspace).secrets.into_iter().find(|secret| secret.id == dns).expect("the secret");
    assert_eq!(secret.grants[0].aliases.get("CLOUDFLARE_API_TOKEN").map(String::as_str), Some("CF_CLAKTORY_DNS"));
    let told = view.entries.iter().rev().find_map(|entry| match &entry.kind {
        Some(Kind::Event(event)) if event.text.contains("gave you CF_CLAKTORY_DNS") => Some(event.text.clone()),
        _ => None,
    });
    assert!(told.expect("told").contains("sets CF_CLAKTORY_DNS also as CLOUDFLARE_API_TOKEN"));
    let command = r#"{"command":"echo $CLOUDFLARE_API_TOKEN $CF_CLAKTORY_DNS","secrets":["CF_CLAKTORY_DNS"]}"#;
    let output = call(&mut client, &mut view, "builder", "bash", command).await;
    assert!(output.starts_with("[secret CF_CLAKTORY_DNS/CF_CLAKTORY_DNS] [secret CF_CLAKTORY_DNS/CF_CLAKTORY_DNS]"), "{output}");

    // Two names against two others: the person says which is which, or gives it as it is.
    let pair = add(&mut client, "Pair", "KEY_A", "value-a-0123456789").await;
    let set = v1::CreateSecret {
        name: "Two".into(),
        entries: vec![entry("ONE", "value-one-0123456789"), entry("TWO", "value-two-0123456789")],
        ..Default::default()
    };
    apply_request(&mut client, &mut view, Request::CreateSecret(set)).await;
    let two = vault(&view.workspace).secrets.into_iter().find(|secret| secret.name == "Two").expect("made").id;
    let need = r#"{"name":"Keys","variables":[{"name":"FIRST"},{"name":"SECOND"}],"reason":"Both keys."}"#;
    call(&mut client, &mut view, "builder", "secrets_request", need).await;
    let asked = waiting(&view).pop().expect("the request");
    let give = |secret: &str| v1::DecideSecretRequest { secret_id: secret.into(), ..decide("builder", &asked.id, true, Vec::new(), "") };
    let unclear = apply_request(&mut client, &mut view, Request::DecideSecretRequest(give(&two))).await;
    assert_eq!(
        failure(&unclear).1,
        "Two sets ONE, TWO, not FIRST, SECOND: choose which of its variables each name is, or give it as it is."
    );
    let wrong = v1::DecideSecretRequest { aliases: [("FIRST".to_owned(), "NOPE".to_owned())].into(), ..give(&two) };
    assert_eq!(failure(&apply_request(&mut client, &mut view, Request::DecideSecretRequest(wrong)).await).1, "Two has no variable NOPE.");
    let mapped = v1::DecideSecretRequest {
        aliases: [("FIRST".to_owned(), "ONE".to_owned()), ("SECOND".to_owned(), "TWO".to_owned())].into(),
        ..give(&two)
    };
    apply_request(&mut client, &mut view, Request::DecideSecretRequest(mapped)).await;
    let secret = vault(&view.workspace).secrets.into_iter().find(|secret| secret.id == two).expect("the secret");
    assert_eq!(secret.grants[0].aliases.len(), 2);
    call(&mut client, &mut view, "builder", "secrets_request", r#"{"name":"Again","variables":[{"name":"OTHER"}],"reason":"More."}"#).await;
    let asked = waiting(&view).pop().expect("the request");
    let as_is = v1::DecideSecretRequest { as_is: true, secret_id: pair.clone(), ..decide("builder", &asked.id, true, Vec::new(), "") };
    apply_request(&mut client, &mut view, Request::DecideSecretRequest(as_is)).await;
    let secret = vault(&view.workspace).secrets.into_iter().find(|secret| secret.id == pair).expect("the secret");
    assert!(secret.grants[0].aliases.is_empty());
}

#[tokio::test]
async fn agents_list_change_and_withdraw_their_own_requests() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let need = r#"{"name":"Cloudflare","description":"DNS edit.","variables":[{"name":"CF_TOKEN"}],"reason":"To deploy."}"#;
    call(&mut client, &mut view, "builder", "secrets_request", need).await;
    let asked = waiting(&view).pop().expect("the request");
    let again = call(&mut client, &mut view, "builder", "secrets_request", need).await;
    assert!(again.contains(&format!("({})", asked.id)) && again.contains("secrets_request_update"), "{again}");

    // Changed in place: the same card, its new variables and reason. An id
    // with spaces around it names the same request.
    let change = format!(r#"{{"id":" {} ","variables":[{{"name":"CLOUDFLARE_API_TOKEN"}}],"reason":"To deploy claktory.com."}}"#, asked.id);
    let said = call(&mut client, &mut view, "builder", "secrets_request_update", &change).await;
    assert!(said.starts_with(&format!("Changed your request {}", asked.id)), "{said}");
    let changed = waiting(&view).into_iter().find(|request| request.id == asked.id).expect("still waiting");
    assert_eq!(changed.new_secret.as_ref().map(|wanted| wanted.variables[0].name.as_str()), Some("CLOUDFLARE_API_TOKEN"));
    assert_eq!(
        (changed.reason.as_str(), changed.new_secret.as_ref().map(|wanted| wanted.description.as_str())),
        ("To deploy claktory.com.", Some("DNS edit."))
    );
    assert!(changed.updated_ms > 0);
    let listed = call(&mut client, &mut view, "builder", "secrets_requests", "{}").await;
    assert!(listed.starts_with(&format!("{} Cloudflare (waiting), variables CLOUDFLARE_API_TOKEN: DNS edit.", asked.id)), "{listed}");

    // Withdrawn: shown so, and the person can't answer it any more.
    let cancel = format!(r#"{{"id":" {} ","reason":"Not needed after all."}}"#, asked.id);
    let said = call(&mut client, &mut view, "builder", "secrets_request_cancel", &cancel).await;
    assert_eq!(said, format!("Withdrew your request {} for Cloudflare.", asked.id));
    assert_eq!(requests(&view), [format!("secret request {} (Withdrawn): To deploy claktory.com.", asked.id)]);
    let late = client.request(Request::DecideSecretRequest(decide("builder", &asked.id, false, Vec::new(), ""))).await;
    assert_eq!(failure(&late.body).1, "The agent withdrew this request.");
    let twice = call(&mut client, &mut view, "builder", "secrets_request_cancel", &cancel).await;
    assert!(twice.contains("isn't waiting any more"), "{twice}");
    let others = call(&mut client, &mut view, "builder", "secrets_request_cancel", r#"{"id":"r00000000"}"#).await;
    assert!(others.starts_with("You have no request r00000000."), "{others}");
}

/// The entries of `agent` waiting for the person, as the workspace says.
fn waiting_seqs(view: &View, agent: &str) -> Vec<u64> {
    view.agent(agent).waiting_seqs.clone()
}

/// The seq of the entry showing request or proposal `id`.
fn seq_of(view: &View, id: &str) -> u64 {
    view.entries
        .iter()
        .find(|entry| match &entry.kind {
            Some(Kind::SecretRequest(request)) => request.id == id,
            Some(Kind::Proposal(proposal)) => proposal.id == id,
            _ => false,
        })
        .map(|entry| entry.seq)
        .expect("its entry")
}

/// Every device can mark an agent with something waiting for the
/// person, and go to it, without opening its conversation.
#[tokio::test]
async fn what_waits_for_the_person_shows_with_the_agent() {
    let dir = temp_dir();
    let (first, second, third);
    {
        let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
        let (mut client, workspace, _) = Client::connect(addr).await;
        let mut view = View { workspace, ..View::default() };
        apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
        apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
        apply_request(&mut client, &mut view, create_agent("site", "Other", "")).await;
        let rivet = v1::UpdateAgent {
            agent_id: "rivet".into(),
            model: Some(crate::common::MODEL.into()),
            effort: Some("off".into()),
            ..Default::default()
        };
        apply_request(&mut client, &mut view, Request::UpdateAgent(rivet)).await;
        assert!(waiting_seqs(&view, "builder").is_empty());

        // Two requests and a proposal to Rivet wait, oldest first.
        let need = |name: &str| {
            format!(r#"{{"name":"{name}","description":"Access.","variables":[{{"name":"{name}_KEY"}}],"reason":"To work."}}"#)
        };
        call(&mut client, &mut view, "builder", "secrets_request", &need("STRIPE")).await;
        call(&mut client, &mut view, "builder", "secrets_request", &need("FLY")).await;
        let asked = waiting(&view);
        (first, second) = (seq_of(&view, &asked[0].id), seq_of(&view, &asked[1].id));
        let said = call(&mut client, &mut view, "builder", "propose_to_rivet", r#"{"message":"Add an agent."}"#).await;
        let proposal = view
            .entries
            .iter()
            .find_map(|entry| match &entry.kind {
                Some(Kind::Proposal(proposal)) => Some(proposal.id.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("the proposal: {said}"));
        third = seq_of(&view, &proposal);
        view.follow(&mut client, |view| waiting_seqs(view, "builder") == [first, second, third]).await;

        // Withdrawn, decided: no longer waiting.
        let cancel = format!(r#"{{"id":"{}","reason":"Not needed."}}"#, asked[1].id);
        call(&mut client, &mut view, "builder", "secrets_request_cancel", &cancel).await;
        view.follow(&mut client, |view| waiting_seqs(view, "builder") == [first, third]).await;
        let decided = v1::DecideProposal { agent_id: "builder".into(), proposal_id: proposal, approve: false, ..Default::default() };
        apply_request(&mut client, &mut view, Request::DecideProposal(decided)).await;
        view.follow(&mut client, |view| waiting_seqs(view, "builder") == [first]).await;
        // The app opens on another agent after the restart.
        apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "other".into() })).await;
    }

    // After a restart, read without anyone opening Builder.
    let (addr, factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    view.follow(&mut client, |view| waiting_seqs(view, "builder") == [first]).await;
    // Expired: nothing waits.
    factory.expire_secret_requests_after(12 * 3_600_000);
    view.follow(&mut client, |view| waiting_seqs(view, "builder").is_empty()).await;
}

/// A request entry for `id` in the selected agent's conversation, as shown
/// now.
fn shown_request(view: &View, id: &str) -> v1::SecretRequest {
    view.entries
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            Some(Kind::SecretRequest(request)) if request.id == id => Some(request.clone()),
            _ => None,
        })
        .expect("the request's entry")
}

/// The person approves the need, and the admin agent (Rivet) does
/// the vault work: it answers the request with a matching secret, renamed
/// to the variables asked for, or collects a new one with an add-secret
/// card in its own conversation.
#[tokio::test]
async fn the_person_approves_the_need_and_the_admin_grants_or_collects() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let rivet = v1::UpdateAgent {
        agent_id: "rivet".into(),
        model: Some(crate::common::MODEL.into()),
        effort: Some("off".into()),
        ..Default::default()
    };
    apply_request(&mut client, &mut view, Request::UpdateAgent(rivet)).await;
    // The vault's token sets CF_TOKEN, not the name Builder asks for.
    add(&mut client, "Cloudflare", "CF_TOKEN", TOKEN).await;

    // The request knows which admin agent an approval passes it to.
    let need = r#"{"name":"Cloudflare token","description":"A Cloudflare token that can edit claktory.com's DNS.","variables":[{"name":"CLOUDFLARE_API_TOKEN"}],"reason":"To deploy."}"#;
    call(&mut client, &mut view, "builder", "secrets_request", need).await;
    let asked = waiting(&view).pop().expect("the request");
    assert_eq!(asked.admin_agent_id, "rivet");

    // Approving the need gives nothing by itself: it passes to Rivet, and
    // leaves Needs you, since it now waits on Rivet, not the person.
    let approve = v1::DecideSecretRequest { approve_only: true, ..decide("builder", &asked.id, true, Vec::new(), "Go ahead.") };
    apply_request(&mut client, &mut view, Request::DecideSecretRequest(approve)).await;
    let shown = shown_request(&view, &asked.id);
    assert_eq!(shown.state(), v1::SecretRequestState::Approved);
    assert!(shown.approved_ms > 0 && shown.admin_agent_id == "rivet" && shown.comment == "Go ahead.");
    assert!(waiting_seqs(&view, "builder").is_empty(), "approved waits on Rivet, not in Needs you");
    assert!(vault(&view.workspace).secrets[0].grants.is_empty(), "nothing given yet");
    let twice = client
        .request(Request::DecideSecretRequest(v1::DecideSecretRequest {
            approve_only: true,
            ..decide("builder", &asked.id, true, Vec::new(), "")
        }))
        .await;
    assert!(failure(&twice.body).1.contains("approved already"), "{:?}", twice.body);

    // Rivet hears what was approved, and answers it with the existing
    // secret: the one variable-name mismatch pairs by itself, the grant
    // goes to Builder, and Builder is told what it got.
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    view.follow(&mut client, |view| {
        events(view).iter().any(|line| line.contains("The person approved Builder's request for Cloudflare token")) && idle(view, "rivet")
    })
    .await;
    let give = format!(r#"{{"secret":"Cloudflare","request":"{}","note":"It's the DNS-only token."}}"#, asked.id);
    let given = call(&mut client, &mut view, "rivet", "secrets_grant", &give).await;
    assert!(given.starts_with("Gave Cloudflare to builder"), "{given}");
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    view.follow(&mut client, |view| {
        events(view).iter().any(|line| line.contains("approved your request and Rivet gave you Cloudflare")) && idle(view, "builder")
    })
    .await;
    // Each note reaches it as its writer's: the person's approval comment,
    // and Rivet's, never as the person's.
    assert!(
        view.entries.iter().any(|entry| matches!(&entry.kind,
        Some(Kind::Event(event)) if event.text.contains("\n\nThe person's note: Go ahead.\n\nRivet's note: It's the DNS-only token.\n\n")))
    );
    let decided = shown_request(&view, &asked.id);
    assert_eq!((decided.state(), decided.given_secret_name.as_str()), (v1::SecretRequestState::Granted, "Cloudflare"));
    assert!(waiting_seqs(&view, "builder").is_empty(), "nothing waits any more");
    // History says it was left to Rivet.
    let history = crate::needs_you::list(&mut client, "Cloudflare token", 0, 0).await;
    let record = history.records.first().expect("in History");
    assert_eq!(
        (record.secret_outcome(), record.subject_agent_id.as_str()),
        (v1::NeedsYouSecretOutcome::Admin, "rivet")
    );
    // Builder uses it under the name it asked for.
    let output =
        call(&mut client, &mut view, "builder", "bash", r#"{"command":"echo $CLOUDFLARE_API_TOKEN","secrets":["Cloudflare"]}"#).await;
    assert!(output.starts_with("[secret Cloudflare/"), "{output}");

    // A second need has no match in the vault: Rivet asks the person to
    // add one, with a card in its own conversation.
    let mail = r#"{"name":"Mail key","description":"A Resend key that can send sign-up mail.","variables":[{"name":"RESEND_API_KEY"}],"reason":"Sign-up mail."}"#;
    call(&mut client, &mut view, "builder", "secrets_request", mail).await;
    let second = waiting(&view).pop().expect("the request");
    let approve = v1::DecideSecretRequest { approve_only: true, ..decide("builder", &second.id, true, Vec::new(), "") };
    apply_request(&mut client, &mut view, Request::DecideSecretRequest(approve)).await;
    let opened = call(&mut client, &mut view, "rivet", "secrets_collect", &format!(r#"{{"request":"{}"}}"#, second.id)).await;
    assert!(opened.starts_with("Asked the person to add Mail key"), "{opened}");
    let card = shown_request(&view, &second.id);
    assert!(card.collecting && card.state() == v1::SecretRequestState::Approved, "the request in Rivet's conversation");
    // While collecting, the person's wait is Rivet's card, counted once.
    assert_eq!(waiting_seqs(&view, "rivet").len(), 1, "the collect request is in Needs you");
    assert!(waiting_seqs(&view, "builder").is_empty(), "the asker's request isn't");

    // The person saves the card: the secret is made and goes to Builder,
    // the card shows it granted, and Rivet hears how it ended.
    let save = v1::DecideSecretRequest {
        name: "Resend".into(),
        ..decide("builder", &second.id, true, vec![entry("RESEND_API_KEY", "re_abcdefghijklmnop")], "")
    };
    apply_request(&mut client, &mut view, Request::DecideSecretRequest(save)).await;
    let card = shown_request(&view, &second.id);
    assert!(card.state() == v1::SecretRequestState::Granted && !card.collecting, "Rivet's request follows");
    assert!(waiting_seqs(&view, "rivet").is_empty(), "nothing waits with Rivet any more");
    view.follow(&mut client, |view| {
        events(view).iter().any(|line| line.contains("gave Builder Resend for its request themselves")) && idle(view, "rivet")
    })
    .await;
    let resend = vault(&view.workspace).secrets.into_iter().find(|secret| secret.name == "Resend").expect("made");
    assert_eq!(resend.grants[0].agent_id.as_str(), "builder");

    // After a restart, both conversations show the requests as they ended.
    drop(client);
    drop(_factory);
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    let card = shown_request(&view, &second.id);
    assert!(card.state() == v1::SecretRequestState::Granted && !card.collecting, "Rivet's request after a restart");
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    assert_eq!(shown_request(&view, &asked.id).state(), v1::SecretRequestState::Granted);
}

/// Approval needs an admin agent with Secrets, and falls
/// back to the direct card without one; an approved request can be
/// withdrawn but not changed, and never expires.
#[tokio::test]
async fn approval_needs_an_admin_and_approved_requests_neither_expire_nor_change() {
    let dir = temp_dir();
    let (addr, factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;

    // Approved, the asker can still withdraw it (Rivet's card would close).
    let need = r#"{"name":"Stripe","description":"Payments.","variables":[{"name":"STRIPE_KEY"}],"reason":"Payments."}"#;
    call(&mut client, &mut view, "builder", "secrets_request", need).await;
    let stripe = waiting(&view).pop().expect("the request");
    assert_eq!(stripe.admin_agent_id, "rivet");
    let approve = v1::DecideSecretRequest { approve_only: true, ..decide("builder", &stripe.id, true, Vec::new(), "") };
    apply_request(&mut client, &mut view, Request::DecideSecretRequest(approve)).await;
    let out = call(&mut client, &mut view, "builder", "secrets_request_cancel", &format!(r#"{{"id":"{}"}}"#, stripe.id)).await;
    assert!(out.starts_with("Withdrew"), "{out}");
    assert_eq!(shown_request(&view, &stripe.id).state(), v1::SecretRequestState::Withdrawn);

    // Approved, it can't be changed any more, and it doesn't expire.
    let keys = r#"{"name":"Keys","description":"Keys.","variables":[{"name":"SOME_KEY"}],"reason":"Keys."}"#;
    call(&mut client, &mut view, "builder", "secrets_request", keys).await;
    let keys = waiting(&view).pop().expect("the request");
    let approve = v1::DecideSecretRequest { approve_only: true, ..decide("builder", &keys.id, true, Vec::new(), "") };
    apply_request(&mut client, &mut view, Request::DecideSecretRequest(approve)).await;
    let changed =
        call(&mut client, &mut view, "builder", "secrets_request_update", &format!(r#"{{"id":"{}","reason":"More."}}"#, keys.id)).await;
    assert!(changed.contains("already approved"), "{changed}");
    factory.expire_secret_requests_after(13 * 3_600_000);
    assert_eq!(shown_request(&view, &keys.id).state(), v1::SecretRequestState::Approved, "approved requests don't expire");

    // The admin gone between the request and the approval: approving the
    // need is refused, and the direct card still works.
    let gone = r#"{"name":"Gone","description":"Gone.","variables":[{"name":"GONE_KEY"}],"reason":"Gone."}"#;
    call(&mut client, &mut view, "builder", "secrets_request", gone).await;
    let gone = waiting(&view).pop().expect("the request");
    assert_eq!(gone.admin_agent_id, "rivet");
    apply_request(&mut client, &mut view, Request::DeleteAgent(v1::DeleteAgent { agent_id: "rivet".into() })).await;
    // Deleting the admin hands its requests back: the approved one loses
    // its admin and is the person's (and Needs you's) again.
    let keys = shown_request(&view, &keys.id);
    assert!(keys.state() == v1::SecretRequestState::Approved && keys.admin_agent_id.is_empty(), "handed back");
    let builder_waits = waiting_seqs(&view, "builder");
    assert!(builder_waits.contains(&seq_of(&view, &keys.id)), "it counts again: {builder_waits:?}");
    let refused = client
        .request(Request::DecideSecretRequest(v1::DecideSecretRequest {
            approve_only: true,
            ..decide("builder", &gone.id, true, Vec::new(), "")
        }))
        .await;
    assert!(failure(&refused.body).1.contains("No admin agent"), "{:?}", refused.body);
    apply_request(&mut client, &mut view, Request::DecideSecretRequest(decide("builder", &gone.id, false, Vec::new(), ""))).await;
    assert_eq!(shown_request(&view, &gone.id).state(), v1::SecretRequestState::Dismissed);

    // With no admin agent at all, a new request carries none: the app
    // shows the direct card from the start.
    let last = r#"{"name":"Last","description":"Last.","variables":[{"name":"LAST_KEY"}],"reason":"Last."}"#;
    call(&mut client, &mut view, "builder", "secrets_request", last).await;
    let last = waiting(&view).pop().expect("the request");
    assert_eq!(last.admin_agent_id, "", "no admin agent to pass it to");
}

/// Builder asks for a mail key, the person approves the need, and Rivet
/// opens the add-secret card for it: the request, with Rivet's
/// conversation selected.
async fn collecting(client: &mut Client, view: &mut View, dir: &Path) -> v1::SecretRequest {
    apply_request(client, view, create_project("Site", &dir.join("site"))).await;
    apply_request(client, view, create_agent("site", "Builder", "")).await;
    let rivet = v1::UpdateAgent {
        agent_id: "rivet".into(),
        model: Some(crate::common::MODEL.into()),
        effort: Some("off".into()),
        ..Default::default()
    };
    apply_request(client, view, Request::UpdateAgent(rivet)).await;
    let mail = r#"{"name":"Mail key","description":"A Resend key.","variables":[{"name":"RESEND_API_KEY"}],"reason":"Sign-up mail."}"#;
    call(client, view, "builder", "secrets_request", mail).await;
    let asked = waiting(view).pop().expect("the request");
    let approve = v1::DecideSecretRequest { approve_only: true, ..decide("builder", &asked.id, true, Vec::new(), "") };
    apply_request(client, view, Request::DecideSecretRequest(approve)).await;
    let opened = call(client, view, "rivet", "secrets_collect", &format!(r#"{{"request":"{}"}}"#, asked.id)).await;
    assert!(opened.starts_with("Asked the person to add Mail key"), "{opened}");
    assert!(shown_request(view, &asked.id).collecting);
    assert_eq!(waiting_seqs(view, "rivet").len(), 1, "the collect request is in Needs you");
    asked
}

/// The asker withdraws its request while Rivet's add-secret card is
/// open. The card shows it withdrawn and leaves Needs you at once; a save
/// sent from a card that missed it is refused and shows it again as it
/// ended; secrets_collect cancel on it still works; and after a restart
/// (the state a card stuck before this fix left on disk) it stays closed.
#[tokio::test]
async fn a_request_withdrawn_while_collected_closes_the_admins_card() {
    let dir = temp_dir();
    let id;
    {
        let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
        let (mut client, workspace, _) = Client::connect(addr).await;
        let mut view = View { workspace, ..View::default() };
        let asked = collecting(&mut client, &mut view, &dir).await;
        id = asked.id.clone();
        let out = call(&mut client, &mut view, "builder", "secrets_request_cancel", &format!(r#"{{"id":"{id}"}}"#)).await;
        assert!(out.starts_with("Withdrew"), "{out}");
        view.follow(&mut client, |view| waiting_seqs(view, "rivet").is_empty()).await;
        apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
        let card = shown_request(&view, &id);
        assert!(card.state() == v1::SecretRequestState::Withdrawn && !card.collecting, "Rivet's request follows: {card:?}");

        // The form saved anyway: refused, saying why.
        let save = v1::DecideSecretRequest {
            name: "Resend".into(),
            ..decide("builder", &id, true, vec![entry("RESEND_API_KEY", "re_abcdefghijklmnop")], "")
        };
        let late = client.request(Request::DecideSecretRequest(save)).await;
        assert_eq!(failure(&late.body).1, "The agent withdrew this request.");
        assert!(vault(&view.workspace).secrets.is_empty(), "nothing made");

        // Rivet closing the card after the request closed isn't an error.
        let closed = call(&mut client, &mut view, "rivet", "secrets_collect", &format!(r#"{{"request":"{id}","cancel":true}}"#)).await;
        assert_eq!(closed, format!("{id} is closed already (its agent withdrew it), and so is its request."));
        assert!(waiting_seqs(&view, "rivet").is_empty());
        apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    }

    // Read back after a restart, without opening Rivet, then opened.
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert!(waiting_seqs(&view, "rivet").is_empty(), "nothing waits with Rivet after a restart");
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    let card = shown_request(&view, &id);
    assert!(card.state() == v1::SecretRequestState::Withdrawn && !card.collecting, "{card:?}");
    assert!(waiting_seqs(&view, "rivet").is_empty());
}

/// A card closed and opened again for the same request: only the new one
/// waits, live and after a restart.
#[tokio::test]
async fn a_collect_card_opened_again_replaces_the_first() {
    let dir = temp_dir();
    let id;
    {
        let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
        let (mut client, workspace, _) = Client::connect(addr).await;
        let mut view = View { workspace, ..View::default() };
        id = collecting(&mut client, &mut view, &dir).await.id;
        let closed = call(&mut client, &mut view, "rivet", "secrets_collect", &format!(r#"{{"request":"{id}","cancel":true}}"#)).await;
        assert!(closed.starts_with("Closed the request"), "{closed}");
        assert!(waiting_seqs(&view, "rivet").is_empty());
        call(&mut client, &mut view, "rivet", "secrets_collect", &format!(r#"{{"request":"{id}"}}"#)).await;
        assert_eq!(waiting_seqs(&view, "rivet").len(), 1, "the new request");
    }
    let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    view.follow(&mut client, |view| !waiting_seqs(view, "rivet").is_empty()).await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    let cards: Vec<(u64, bool)> = view
        .entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::SecretRequest(request)) if request.id == id => Some((entry.seq, request.collecting)),
            _ => None,
        })
        .collect();
    assert_eq!(cards.len(), 2, "{cards:?}");
    assert!(!cards[0].1 && cards[1].1, "only the newer request is open: {cards:?}");
    assert_eq!(waiting_seqs(&view, "rivet"), [cards[1].0]);
}

/// The asker deleted while Rivet's card is open: its requests go with it,
/// and the card closes, now and when Rivet's conversation is read back
/// after a restart (as one stuck before this fix does).
#[tokio::test]
async fn deleting_the_asker_closes_the_admins_card() {
    let dir = temp_dir();
    let id;
    {
        let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
        let (mut client, workspace, _) = Client::connect(addr).await;
        let mut view = View { workspace, ..View::default() };
        id = collecting(&mut client, &mut view, &dir).await.id;
        apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
        apply_request(&mut client, &mut view, Request::DeleteAgent(v1::DeleteAgent { agent_id: "builder".into() })).await;
        view.follow(&mut client, |view| waiting_seqs(view, "rivet").is_empty()).await;
        let card = shown_request(&view, &id);
        assert!(card.state() == v1::SecretRequestState::Withdrawn && !card.collecting, "{card:?}");
        assert_eq!(card.comment, "The agent was deleted.");
    }
    let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert!(waiting_seqs(&view, "rivet").is_empty());
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    let card = shown_request(&view, &id);
    assert!(card.state() == v1::SecretRequestState::Withdrawn && !card.collecting, "read back closed: {card:?}");
    assert!(waiting_seqs(&view, "rivet").is_empty());
}

/// /clear of the admin agent: its card went with the conversation, so the
/// requests it held are the person's again, in the asker's conversation.
#[tokio::test]
async fn clearing_the_admin_hands_its_requests_back() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let id = collecting(&mut client, &mut view, &dir).await.id;
    let clear = v1::RunCommand { agent_id: "rivet".into(), name: "clear".into(), args: String::new() };
    apply_request(&mut client, &mut view, Request::RunCommand(clear)).await;
    view.follow(&mut client, |view| waiting_seqs(view, "rivet").is_empty() && waiting_seqs(view, "builder").len() == 1).await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    let card = shown_request(&view, &id);
    assert!(card.state() == v1::SecretRequestState::Approved && card.admin_agent_id.is_empty() && !card.collecting, "{card:?}");
}
