//! The vault (#107): secrets the person adds in the app, sealed in
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
    assert_eq!(preview.warnings, ["PIN is too short to hide in agents' output."]);
    let import = v1::ImportEnv { text: text.into(), name: "Resend".into(), description: "Mail.".into(), one_per_variable: false };
    let events = client.ok(Request::ImportEnv(import)).await;
    let state = vault(last_workspace(&events).expect("workspace"));
    assert_eq!(state.secrets.iter().map(|secret| secret.name.as_str()).collect::<Vec<_>>(), ["Cloudflare", "Resend"]);
    assert_eq!(state.secrets[1].warnings, ["PIN is too short to hide in agents' output."]);

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
    let grant = || Request::GrantSecret(v1::GrantSecret { id: id.clone(), agent_id: "builder".into(), proof: None });
    let reveal = || Request::RevealSecret(v1::RevealSecret { id: id.clone(), proof: None });

    let missing = client.request(Request::GrantSecret(v1::GrantSecret { id: id.clone(), agent_id: "nobody".into(), proof: None })).await;
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
    assert_eq!(kinds, [("revoke", "builder", ""), ("reveal", "", "this computer"), ("grant", "builder", "")]);

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
            std::fs::metadata(dir.join("home/vault/auto.key"))
                .map(|meta| std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o777)
                .ok(),
            Some(0o600)
        );
        // With only the key on disk, locked mode would lock it for good.
        let refused = client.request(Request::SetVaultLockedMode(v1::SetVaultLockedMode { locked_mode: true, proof: None })).await;
        assert!(failure(&refused.body).1.contains("neither"), "{:?}", refused.body);
        // A recovery key first: then it can.
        let key = client.request(Request::NewRecoveryKey(v1::NewRecoveryKey { proof: None })).await;
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
    client.request(Request::GrantSecret(v1::GrantSecret { id, agent_id: "builder".into(), proof: None })).await;
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
    let grant = v1::GrantSecret { id: id.clone(), agent_id: "builder".into(), proof: None };
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

fn requests(view: &View) -> Vec<String> {
    describe(&view.entries).into_iter().filter(|line| line.starts_with("secret request")).collect()
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
    let pending = vault(&view.workspace).requests;
    assert_eq!(pending.len(), 1);
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
    for request in vault(&view.workspace).requests.into_iter().filter(|request| request.id != first.id) {
        apply_request(&mut client, &mut view, Request::DecideSecretRequest(decide("builder", &request.id, false, Vec::new(), ""))).await;
    }

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
    let decided = view.entries.iter().find_map(|entry| match &entry.kind {
        Some(Kind::SecretRequest(request)) if request.id == first.id => Some(request.clone()),
        _ => None,
    });
    let decided = decided.expect("the entry");
    assert_eq!((decided.given_secret_id.as_str(), decided.given_secret_name.as_str()), (cloudflare.as_str(), "Cloudflare"));
    assert!(vault(&view.workspace).requests.is_empty());
    let output =
        call(&mut client, &mut view, "builder", "bash", r#"{"command":"echo $CLOUDFLARE_API_TOKEN","secrets":["Cloudflare"]}"#).await;
    assert!(output.starts_with("[secret Cloudflare/CLOUDFLARE_API_TOKEN]"), "{output}");

    // A new one, from the description: the person names it and fills in its value.
    let new =
        r#"{"name":"Mail","description":"Mail for the site.","variables":[{"name":"RESEND_API_KEY"}],"reason":"To send sign-up mail."}"#;
    call(&mut client, &mut view, "builder", "secrets_request", new).await;
    let request = vault(&view.workspace).requests[0].clone();
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
    let id = vault(&view.workspace).requests[0].id.clone();
    apply_request(&mut client, &mut view, Request::DecideSecretRequest(decide("builder", &id, false, Vec::new(), "Not yet."))).await;
    view.follow(&mut client, |view| {
        events(view).iter().any(|line| line.contains("dismissed your request for Stripe")) && idle(view, "builder")
    })
    .await;
    assert!(
        events(&view).iter().any(|line| line == "event from Claktory: The person dismissed your request for Stripe (comment: Not yet.)."),
        "{:?}",
        events(&view)
    );
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
