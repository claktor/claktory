//! Provider sign-ins kept in the vault (rivet-move.md §3, P2): moved there
//! from `auth.json`, sealed, with no token left in plaintext once no older
//! backend could need one; a sign-in made elsewhere taken in; a locked vault
//! that keeps them while its agents wait, and opens with the recovery key;
//! and a sign-in made while it's locked that works at once.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::accounts::{account, anthropic_with, builder, claude, create_builder, model_requests, order, registry_over};
use crate::common::{Client, View, apply_request, describe, home, send_message, temp_dir};
use factory_agent::clock::now_ms;
use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body};
use factoryd::factory::Factory;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;

/// What the tokens and keys in these tests start with, and the marks in
/// what's made from them: none may be in plaintext.
const SECRETS: [&[u8]; 4] = [b"sk-ant-oat01", b"sk-ant-ort01", b"sk-or-v1", b"acct-mark"];

/// Waits for `done`, a few seconds at most.
async fn until(what: &str, done: impl Fn() -> bool) {
    for _ in 0..200 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting until {what}");
}

fn plain(dir: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("auth.json")).unwrap_or_default()).unwrap_or(Value::Null)
}

fn moved(dir: &Path) -> bool {
    home(dir).root().join("vault/sign-ins.json").exists()
}

/// The files under `dir` that hold a token in plaintext.
fn tokens_in_plaintext(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(path) = pending.pop() {
        let Ok(metadata) = std::fs::symlink_metadata(&path) else { continue };
        if metadata.is_dir() {
            pending.extend(std::fs::read_dir(&path).expect("dir").map(|entry| entry.expect("entry").path()));
        } else if std::fs::read(&path)
            .is_ok_and(|bytes| SECRETS.iter().any(|secret| bytes.windows(secret.len()).any(|window| window == *secret)))
        {
            found.push(path.display().to_string());
        }
    }
    found
}

/// Account `id`, with a refresh token and an account id to look for.
fn signed_in(id: &str) -> Value {
    let mut account = account(id);
    account["refresh"] = json!(format!("sk-ant-ort01-{id}"));
    account["accountId"] = json!(format!("acct-mark-{id}"));
    account
}

/// Anthropic's endpoints, no account at its limit: requests by any account
/// are answered "hi".
async fn anthropic() -> (String, Arc<std::sync::Mutex<Vec<(String, String)>>>) {
    anthropic_with(now_ms() / 1000 + 3 * 3600, Arc::new(std::sync::atomic::AtomicBool::new(false))).await
}

/// A Claktory over the sign-ins in `dir`'s `auth.json`, serving.
struct Open {
    factory: Arc<Factory>,
    registry: Arc<factory_agent::Registry>,
    server: tokio::task::JoinHandle<()>,
}

impl Open {
    /// Stops it as a restart does. What it started runs on in this process,
    /// so its sign-ins are kept elsewhere from now on: it touches none of
    /// the next one's.
    async fn stop(self, dir: &Path, client: Client) {
        self.server.abort();
        drop(client);
        self.registry.set_backing(Arc::new(factory_agent::credentials::FileBacking::new(dir.join("stopped.json"))));
    }
}

async fn open(dir: &Path, base_url: &str) -> (Open, Client, View) {
    let registry = Arc::new(registry_over(dir.join("auth.json"), base_url));
    let factory = Factory::open(home(dir), registry.clone()).await.expect("open");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    crate::common::serves(addr, home(dir));
    let server = tokio::spawn(factoryd::serve(listener, factory.clone()));
    let (client, workspace, transcript) = Client::connect(addr).await;
    let entries = transcript.map(|transcript| transcript.entries).unwrap_or_default();
    (Open { factory, registry, server }, client, View { workspace, entries, ..View::default() })
}

/// Opens `dir`'s Claktory, lets its sign-ins move into the vault and
/// returns the vault's recovery key; then its key on disk is lost, and it
/// opens again: locked, until the recovery key opens it.
async fn locked_after_restart(dir: &Path, base_url: &str) -> (Arc<Factory>, Client, View, String) {
    std::fs::write(dir.join("auth.json"), json!({"anthropic": account("a")}).to_string()).expect("credentials");
    let (running, mut client, _) = open(dir, base_url).await;
    until("the sign-ins move into the vault", || moved(dir) && plain(dir) == json!({})).await;
    let Body::RecoveryKey(recovery) = client.request(Request::ShowRecoveryKey(v1::ShowRecoveryKey {})).await.body else {
        panic!("expected the recovery key")
    };
    running.stop(dir, client).await;
    std::fs::remove_file(home(dir).root().join("vault/auto.key")).expect("the key on disk");
    let (running, client, view) = open(dir, base_url).await;
    (running.factory, client, view, recovery.key)
}

fn unlock(recovery: &str) -> Request {
    Request::UnlockVault(v1::UnlockVault { passphrase: recovery.to_owned() })
}

/// The sign-ins move into the vault: `auth.json` is emptied, and no file
/// holds a token or key in plaintext, the vault's included, even one this
/// Claktory can't read. A sign-in made with no Claktory running (`factoryd
/// login`) is taken in, and the file emptied again.
#[tokio::test]
async fn sign_ins_move_into_the_vault_and_no_token_stays_in_plaintext() {
    let dir = temp_dir();
    let (base_url, seen) = anthropic().await;
    let mut first = signed_in("a");
    first["factoryAccounts"] = json!([signed_in("b")]);
    let credentials = json!({
        "anthropic": first,
        "openrouter": {"type": "api_key", "key": "sk-or-v1-key", "factoryAccount": "k"},
        // Edited by hand, say: unreadable, and moved all the same.
        "openai": {"type": "api", "key": "sk-or-v1-unreadable"},
    });
    std::fs::write(dir.join("auth.json"), credentials.to_string()).expect("credentials");
    let (_factory, mut client, mut view) = open(&dir, &base_url).await;
    until("the sign-ins move into the vault", || moved(&dir) && plain(&dir) == json!({})).await;
    view.follow(&mut client, |view| order(view) == ["a", "b"]).await;
    assert_eq!(tokens_in_plaintext(&dir), Vec::<String>::new());
    let outline = home(&dir).root().join("vault/sign-ins.json");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&outline).expect("outline").permissions().mode() & 0o777, 0o600);

    // Agents run on them.
    create_builder(&mut client, &mut view).await;
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
    view.follow(&mut client, |view| describe(&view.entries).iter().any(|entry| entry == "assistant: hi")).await;
    assert_eq!(model_requests(&seen), 1);

    // Signed in elsewhere: taken in, and not kept in plaintext.
    std::fs::write(dir.join("auth.json"), json!({"anthropic": signed_in("c")}).to_string()).expect("a sign-in");
    view.follow(&mut client, |view| order(view) == ["a", "b", "c"]).await;
    until("auth.json is emptied again", || plain(&dir) == json!({})).await;
    assert_eq!(tokens_in_plaintext(&dir), Vec::<String>::new());
}

/// While the backend the supervisor would go back to reads only
/// `auth.json`, it keeps a full copy, written with every change; once that
/// backend reads the vault too, it's emptied.
#[tokio::test]
async fn auth_json_keeps_a_copy_while_the_backend_before_would_need_it() {
    let dir = temp_dir();
    let (base_url, _) = anthropic().await;
    let older = b"an older backend".to_vec();
    let bin = home(&dir).root().join("bin");
    std::fs::create_dir_all(&bin).expect("bin");
    std::fs::write(bin.join("factoryd.previous"), &older).expect("previous");
    std::fs::write(dir.join("auth.json"), json!({"anthropic": account("a")}).to_string()).expect("credentials");
    {
        let (running, mut client, mut view) = open(&dir, &base_url).await;
        until("the sign-ins move into the vault", || moved(&dir)).await;
        view.follow(&mut client, |view| order(view) == ["a"]).await;
        assert_eq!(plain(&dir)["anthropic"]["access"], "sk-ant-oat01-a", "the copy stays");
        let rename = Request::UpdateAccount(v1::UpdateAccount {
            provider_id: "anthropic".into(),
            account_id: "a".into(),
            label: Some("Team".into()),
            retry: false,
        });
        assert!(matches!(apply_request(&mut client, &mut view, rename).await, Body::Ack(_)));
        until("the copy follows the change", || plain(&dir)["anthropic"]["factoryLabel"] == "Team").await;
        running.stop(&dir, client).await;
    }

    // The backend before is one that reads the vault (an update noted it).
    let noted: String = Sha256::digest(&older).iter().map(|byte| format!("{byte:02x}")).collect();
    std::fs::write(home(&dir).root().join("vault/sign-ins-backends"), format!("{noted}\n")).expect("noted");
    let (_factory, mut client, mut view) = open(&dir, &base_url).await;
    until("auth.json is emptied", || plain(&dir) == json!({})).await;
    view.follow(&mut client, |view| claude(view).accounts.first().is_some_and(|account| account.label == "Team")).await;
    assert_eq!(tokens_in_plaintext(&dir), Vec::<String>::new());
}

/// A locked vault keeps the sign-ins: they're shown as they are, and an
/// agent that needs one waits, saying so; after a minute Needs you offers to
/// open the vault. The recovery key opens it, and the agent carries on.
#[tokio::test]
async fn a_locked_vault_keeps_the_sign_ins_and_agents_wait_until_it_opens() {
    let dir = temp_dir();
    let (base_url, seen) = anthropic().await;
    let (factory, mut client, mut view, recovery) = locked_after_restart(&dir, &base_url).await;
    factory.set_locked_wait(Duration::from_millis(300));
    let vault = view.workspace.vault.clone().expect("vault");
    assert!(vault.exists && !vault.unlocked);
    assert_eq!(vault.locked_why, "Unlock them with your recovery key.");
    view.follow(&mut client, |view| order(view) == ["a"]).await;
    assert_eq!(claude(&view).signed_in_with, "Anthropic (Claude Pro/Max)");
    assert!(view.workspace.models.iter().any(|model| model.id.starts_with("anthropic/")), "its models are there");

    create_builder(&mut client, &mut view).await;
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
    view.follow(&mut client, |view| builder(view).usage_wait == "Waiting to use Anthropic until your secrets unlock").await;
    view.follow(&mut client, |view| describe(&view.entries).len() == 2).await;
    assert_eq!(
        describe(&view.entries)[1],
        "notice: Your sign-ins are locked with your secrets. Builder carries on by itself once they're unlocked."
    );
    view.follow(&mut client, |view| view.workspace.vault.as_ref().is_some_and(|vault| vault.agents_waiting)).await;
    assert_eq!(model_requests(&seen), 0);

    // The recovery key opens it; nothing goes to claktory.com.
    assert!(matches!(client.request(unlock(&recovery)).await.body, Body::VaultSession(_)), "the recovery key opens it");
    view.follow(&mut client, |view| describe(&view.entries).iter().any(|entry| entry == "assistant: hi")).await;
    view.follow(&mut client, |view| builder(view).usage_wait.is_empty()).await;
    let vault = view.workspace.vault.clone().expect("vault");
    assert!(vault.unlocked && !vault.agents_waiting && vault.locked_why.is_empty());
    assert_eq!(model_requests(&seen), 1);
    assert_eq!(plain(&dir), json!({}));
}

/// A locked vault never keeps the person from signing in: a sign-in made
/// meanwhile is used at once, and taken into the vault when it opens.
#[tokio::test]
async fn a_sign_in_while_the_vault_is_locked_works_and_goes_into_it_once_it_opens() {
    let dir = temp_dir();
    let (base_url, seen) = anthropic().await;
    let (_factory, mut client, mut view, recovery) = locked_after_restart(&dir, &base_url).await;
    std::fs::write(dir.join("auth.json"), json!({"anthropic": account("b")}).to_string()).expect("a sign-in");
    view.follow(&mut client, |view| order(view) == ["b", "a"]).await;

    create_builder(&mut client, &mut view).await;
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
    view.follow(&mut client, |view| describe(&view.entries).iter().any(|entry| entry == "assistant: hi")).await;
    let asked: Vec<String> =
        seen.lock().expect("lock").iter().filter(|(path, _)| path.ends_with("/v1/messages")).map(|(_, account)| account.clone()).collect();
    assert_eq!(asked, ["b"]);

    assert!(matches!(client.request(unlock(&recovery)).await.body, Body::VaultSession(_)), "the recovery key opens it");
    until("auth.json is emptied", || plain(&dir) == json!({})).await;
    view.follow(&mut client, |view| order(view) == ["a", "b"]).await;
    assert_eq!(tokens_in_plaintext(&dir), Vec::<String>::new());
}
