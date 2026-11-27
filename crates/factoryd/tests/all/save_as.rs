//! bash's save_as (Fugo's save-as-secret.md, Bruno 45874): an agent saves
//! a command's whole output as a new secret, for a key it makes with one
//! it holds. The output reaches the vault alone (never the agent, its
//! conversation, factory.db or a log), the new secret is granted to that
//! agent alone, and History says so. Everything that can refuse it is
//! checked before the command runs, where it can be.

use std::path::Path;
use std::sync::Arc;

use crate::common::{Client, View, apply_request, create_agent, create_project, start, temp_dir};
use crate::pipes::call;
use factory_protocol::v1::{self, client_message::Body as Request, entry::Kind, server_message::Body};
use factoryd::Factory;
use serde_json::json;

const OWNER: &str = "gc-owner-token-abcdef0123";
/// What the command prints, made by it rather than written in it, so
/// nothing but the vault ever holds it.
const MADE: &str = "nk-864192-296295";
const MAKE: &str = "printf 'nk-%s-%s' $((123456*7)) $((98765*3))";

/// A project "Site" with Builder, which holds "Google Cloud".
async fn setup() -> (std::path::PathBuf, Arc<Factory>, Client, View) {
    setup_in(&temp_dir()).await
}

async fn setup_in(dir: &Path) -> (std::path::PathBuf, Arc<Factory>, Client, View) {
    let dir = dir.to_path_buf();
    let (addr, factory) = start(&dir).await;
    factory.set_vault_passphrase_cost("argon2id m=1024 t=1 p=1");
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    add(&mut client, &mut view, "Google Cloud", "GOOGLE_TOKEN", OWNER, &[]).await;
    (dir, factory, client, view)
}

/// Adds a secret given to Builder.
async fn add(client: &mut Client, view: &mut View, name: &str, variable: &str, value: &str, sites: &[&str]) -> String {
    let secret = v1::CreateSecret {
        name: name.into(),
        description: "Given.".into(),
        entries: vec![v1::SecretEntry { variable: variable.into(), value: value.into(), file: false }],
        ask_each_time: false,
        sites: sites.iter().map(|site| (*site).to_owned()).collect(),
    };
    apply_request(client, view, Request::CreateSecret(secret)).await;
    let id = secrets(view).into_iter().find(|secret| secret.name == name).expect("added").id;
    let grant = v1::GrantSecret { id: id.clone(), agent_id: "builder".into(), proof: None, fill_only: false };
    apply_request(client, view, Request::GrantSecret(grant)).await;
    id
}

fn secrets(view: &View) -> Vec<v1::SecretInfo> {
    view.workspace.vault.clone().expect("vault").secrets
}

/// bash's arguments saving `command`'s output as `name`, run with
/// `secrets`; `more` adds to them (`save_as` merges into its own).
fn save(command: &str, secrets: &[&str], name: &str, more: serde_json::Value) -> String {
    let mut args = serde_json::json!({
        "command": command,
        "secrets": secrets,
        "save_as": {"name": name, "variable": "PLAY_KEY", "description": "The Play deploy key, made with Google Cloud owner."},
    });
    for (key, value) in more.as_object().cloned().unwrap_or_default() {
        match (key.as_str(), value) {
            ("save_as", serde_json::Value::Object(fields)) => {
                for (field, value) in fields {
                    args["save_as"][field] = value;
                }
            }
            (_, value) => args[key] = value,
        }
    }
    args.to_string()
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
async fn an_agent_saves_a_key_it_makes_and_only_the_vault_ever_holds_it() {
    let (dir, _factory, mut client, mut view) = setup().await;
    // What it says on stderr comes back, with the new value hidden too.
    let command = format!("echo \"made $({MAKE})\" >&2; {MAKE}");
    let said = call(&mut client, &mut view, "builder", "bash", &save(&command, &["Google Cloud"], "Play deploy key", json!({}))).await;
    assert!(
        said.starts_with(&format!(
            "Saved the command's output ({} bytes) as the secret Play deploy key, which sets PLAY_KEY; it's granted to you alone.",
            MADE.len()
        )),
        "{said}"
    );
    assert!(said.contains("It said (stderr):\nmade [secret Play deploy key/PLAY_KEY]"), "{said}");
    assert!(said.ends_with("s]") && said.contains("[exit 0, "), "{said}");

    let saved = secrets(&view).into_iter().find(|secret| secret.name == "Play deploy key").expect("saved");
    assert_eq!(saved.owner_agent_id, "builder");
    assert_eq!(saved.description, "The Play deploy key, made with Google Cloud owner.");
    assert_eq!(saved.grants.iter().map(|grant| grant.agent_id.as_str()).collect::<Vec<_>>(), ["builder"]);
    assert!(!saved.ask_each_time);

    // It uses it as any other, hidden in what it prints.
    let used = call(&mut client, &mut view, "builder", "bash", r#"{"command": "echo got $PLAY_KEY", "secrets": ["play deploy key"]}"#).await;
    assert!(used.starts_with("got [secret Play deploy key/PLAY_KEY]"), "{used}");

    // History has it at its call, with its description.
    let list = v1::ListNeedsYouHistory { query: String::new(), before_ms: 0, limit: 0 };
    let Body::NeedsYouHistory(history) = apply_request(&mut client, &mut view, Request::ListNeedsYouHistory(list)).await else {
        panic!("history")
    };
    let record = history.records.iter().find(|record| record.kind() == v1::NeedsYouRecordKind::SecretSaved).expect("a record");
    assert_eq!(
        (record.agent_id.as_str(), record.title.as_str(), record.agent_reason.as_str(), record.outcome()),
        (
            "builder",
            "Builder saved Play deploy key, a new secret made with Google Cloud",
            "The Play deploy key, made with Google Cloud owner.",
            v1::NeedsYouOutcome::Changed
        )
    );
    let at = view.entries.iter().find(|entry| entry.seq == record.seq).and_then(|entry| match &entry.kind {
        Some(Kind::ToolCall(call)) => Some(call.name.clone()),
        _ => None,
    });
    assert_eq!(at.as_deref(), Some("bash"));
    let list = v1::ListNeedsYouHistory { query: "deploy key".into(), before_ms: 0, limit: 0 };
    let Body::NeedsYouHistory(found) = apply_request(&mut client, &mut view, Request::ListNeedsYouHistory(list)).await else {
        panic!("history")
    };
    assert_eq!(found.records.len(), 1, "found by its name and description");

    // No file but the vault's holds the value: not the transcript, factory.db,
    // the tasks folder or the log; and the vault's is sealed.
    assert_eq!(holding(&dir, MADE), Vec::<String>::new());
    assert!(!format!("{:?}{:?}", view.entries, view.workspace).contains(MADE));
}

#[tokio::test]
async fn what_can_refuse_a_save_is_checked_before_the_command_runs() {
    let (dir, factory, mut client, mut view) = setup().await;
    add(&mut client, &mut view, "Shop", "SHOP_PASSWORD", "hunter2-hunter2-0", &["https://shop.example"]).await;
    let ran = dir.join("site/ran");
    let command = format!("touch ran; {MAKE}");
    let cases = [
        (save(&command, &["Google Cloud"], "google cloud", json!({})), "There's a secret named google cloud already: choose another name."),
        (
            save(&command, &["Google Cloud"], "Play deploy key", json!({"run_in_background": true})),
            "save_as keeps a command's output out of every log, so it runs in the foreground only",
        ),
        (save(&command, &["Google Cloud", "Shop"], "Play deploy key", json!({})), "Shop is for filling into its sites in Claktory's browser"),
        (save(&command, &["Stripe"], "Play deploy key", json!({})), "\"Stripe\" isn't one of your secrets."),
        (save(&command, &[], "Play deploy key", json!({"save_as": {"variable": "1KEY"}})), "\"1KEY\" isn't a variable name"),
        (save(&command, &[], "Play deploy key", json!({"save_as": {"description": " "}})), "Describe the secret (description)"),
    ];
    for (args, expected) in cases {
        let said = call(&mut client, &mut view, "builder", "bash", &args).await;
        assert!(said.contains(expected), "{args}\n{said}");
        assert!(!ran.exists(), "{args}: it ran");
    }

    // An agent that read what came from outside saves nothing.
    factoryd::pipes::taint_for_test(&factory, "builder", "browser:https://example.com");
    let said = call(&mut client, &mut view, "builder", "bash", &save(&command, &[], "Play deploy key", json!({}))).await;
    assert!(said.starts_with("You've read what came from outside this Claktory"), "{said}");
    assert!(!ran.exists(), "it ran");
    assert_eq!(secrets(&view).len(), 2, "nothing saved");
}

#[tokio::test]
async fn a_save_is_refused_after_the_command_when_its_output_cant_be_one() {
    let (_dir, _factory, mut client, mut view) = setup().await;
    let nothing = "Nothing was saved: if the command made a key at a service, revoke it there, then run it again.";
    let cases = [
        // Another secret's value: as it is, or in what's printed.
        ("printf %s \"$GOOGLE_TOKEN\"", "That's the value of another secret; ask the person instead. Nothing was saved."),
        ("echo \"Bearer $GOOGLE_TOKEN\"", "That's the value of another secret"),
        ("true", "The command printed nothing to save, so nothing was saved."),
        ("echo '   '", "The command printed nothing to save"),
        ("printf abcdefgh12345; exit 3", "The command failed (exit 3)."),
        ("head -c 70000 /dev/zero | tr '\\0' a", "The command printed more than a secret holds (64 KiB)."),
        ("sleep 5; printf abcdefgh12345", "The command timed out."),
    ];
    for (command, expected) in cases {
        let args = save(command, &["Google Cloud"], "Play deploy key", json!({"timeout": 1}));
        let said = call(&mut client, &mut view, "builder", "bash", &args).await;
        assert!(said.starts_with(expected), "{command}\n{said}");
        if expected.starts_with("The command failed") {
            assert!(said.contains(nothing), "{said}");
        }
        assert!(!said.contains(OWNER), "{said}");
    }
    assert_eq!(secrets(&view).len(), 1, "nothing saved");

    // Output that isn't text is saved only as a file, its bytes as printed.
    let binary = "printf '\\377\\376\\375key-bytes-0123'";
    let said = call(&mut client, &mut view, "builder", "bash", &save(binary, &[], "Play deploy key", json!({}))).await;
    assert!(said.starts_with("The output isn't text, so it can only be saved as a file: set file to true."), "{said}");
    let said = call(&mut client, &mut view, "builder", "bash", &save(binary, &[], "Play deploy key", json!({"save_as": {"file": true}}))).await;
    assert!(said.starts_with("Saved the command's output (17 bytes)"), "{said}");
    let used = call(&mut client, &mut view, "builder", "bash", r#"{"command": "wc -c < \"$PLAY_KEY\" | tr -d ' '", "secrets": ["Play deploy key"]}"#).await;
    assert!(used.starts_with("17\n"), "{used}");
}

#[tokio::test]
async fn an_agent_saves_at_most_5_secrets_an_hour() {
    let (dir, _factory, mut client, mut view) = setup().await;
    for n in 1..=5 {
        let command = format!("printf 'made-key-%s-0000' {n}");
        let said = call(&mut client, &mut view, "builder", "bash", &save(&command, &[], &format!("Key {n}"), json!({}))).await;
        assert!(said.starts_with("Saved the command's output"), "{said}");
    }
    let said = call(&mut client, &mut view, "builder", "bash", &save("touch ran; printf made-key-6-0000", &[], "Key 6", json!({}))).await;
    assert!(said.starts_with("You've saved 5 secrets in the last hour, the most there can be"), "{said}");
    assert!(!dir.join("site/ran").exists(), "it ran");
}

#[tokio::test]
async fn nothing_is_saved_while_the_vault_is_locked_and_the_command_doesnt_run() {
    let dir = temp_dir();
    {
        let (_dir, _factory, mut client, mut view) = setup_in(&dir).await;
        // Its context starts with the vault open, so its bash has save_as,
        // kept with the context after the restart.
        let used = call(&mut client, &mut view, "builder", "bash", r#"{"command": "echo ok", "secrets": ["Google Cloud"]}"#).await;
        assert!(used.starts_with("ok"), "{used}");
        client.request(Request::NewRecoveryKey(v1::NewRecoveryKey { proof: None, ..Default::default() })).await;
        client.request(Request::SetVaultLockedMode(v1::SetVaultLockedMode { locked_mode: true, proof: None })).await;
    }
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert!(!view.workspace.vault.clone().expect("vault").unlocked);
    let said = call(&mut client, &mut view, "builder", "bash", &save("touch ran; printf made-key-0000", &[], "Key", json!({}))).await;
    assert!(said.starts_with("The vault is locked, so nothing can be saved"), "{said}");
    assert!(!dir.join("site/ran").exists(), "it ran");
}

/// A key made with a secret that asks each time asks each time too: a copy
/// never sheds a limit.
#[tokio::test]
async fn a_key_made_with_a_secret_that_asks_each_time_asks_each_time() {
    let (_dir, _factory, mut client, mut view) = setup().await;
    let aws = add(&mut client, &mut view, "AWS", "AWS_SECRET", "wJalrXUtnFEMI-K7MDENG", &[]).await;
    apply_request(&mut client, &mut view, Request::SetSecretAskEachTime(v1::SetSecretAskEachTime { id: aws, ask_each_time: true, proof: None })).await;
    let args = save(MAKE, &["AWS"], "Narrow AWS key", json!({}));
    let asked = call(&mut client, &mut view, "builder", "bash", &args).await;
    assert!(asked.starts_with("Not run: AWS asks"), "{asked}");
    let seq = view
        .entries
        .iter()
        .rev()
        .find_map(|entry| matches!(entry.kind, Some(Kind::SecretApproval(_))).then_some(entry.seq))
        .expect("a card");
    let allow = v1::DecideSecretApproval { agent_id: "builder".into(), seq, allow: true, ..Default::default() };
    apply_request(&mut client, &mut view, Request::DecideSecretApproval(allow)).await;
    view.follow(&mut client, |view| {
        view.entries.iter().any(|entry| {
            entry.seq == seq
                && matches!(&entry.kind, Some(Kind::SecretApproval(approval)) if approval.state() == v1::SecretApprovalState::Allowed)
        })
    })
    .await;
    view.follow(&mut client, |view| crate::pipes::idle(view, "builder")).await;
    let said = call(&mut client, &mut view, "builder", "bash", &args).await;
    assert!(said.starts_with("Saved the command's output"), "{said}");
    let saved = secrets(&view).into_iter().find(|secret| secret.name == "Narrow AWS key").expect("saved");
    assert!(saved.ask_each_time, "{saved:?}");
}
