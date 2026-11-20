//! Signing in to claktory.com (#116), against a local account service
//! (`claktory-account`): the email and its code, the vault protected by the
//! account instead of a key on disk, opened again after a restart with the
//! service's share (by itself once the service is back), check-ins, sign
//! out, and removal at the service.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use claktory_account::server::{self, Config, Secrets, Service, mail};
use factory_protocol::v1::{self, ClaktoryAccountState as AccountState, ErrorCode, client_message::Body as Request, server_message::Body};

use crate::common::{Client, create_agent, last_workspace, send_message, start, temp_dir};

/// A local account service, on a port of its own, with mail kept in memory
/// and a clock a test can move on.
struct Instance {
    url: String,
    addr: std::net::SocketAddr,
    db: std::path::PathBuf,
    mail: Arc<mail::Memory>,
    ahead: Arc<AtomicI64>,
    secrets: ([u8; 32], [u8; 32]),
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Instance {
    async fn start(dir: &Path) -> Instance {
        let secrets = Secrets::generate();
        let kept = (*secrets.k_share, secrets.signing_key.to_bytes());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a port");
        let addr = listener.local_addr().expect("an address");
        let mut instance = Instance {
            url: format!("http://{addr}"),
            addr,
            db: dir.join("account.db"),
            mail: Arc::new(mail::Memory::default()),
            ahead: Arc::default(),
            secrets: kept,
            server: tokio::spawn(async { Ok(()) }),
        };
        instance.serve(listener);
        instance
    }

    fn serve(&mut self, listener: tokio::net::TcpListener) {
        let secrets = Secrets {
            k_share: zeroize::Zeroizing::new(self.secrets.0),
            signing_key: ed25519_dalek::SigningKey::from_bytes(&self.secrets.1),
            resend_api_key: None,
            cloudflare_api_token: None,
            relay_bearer: None,
        };
        let ahead = self.ahead.clone();
        let clock: server::Clock = Arc::new(move || jiff::Timestamp::now().as_second() + ahead.load(Ordering::SeqCst));
        let service = Service::new(&self.db, secrets, Config::default(), self.mail.clone(), clock).expect("the service");
        self.server = tokio::spawn(server::serve(listener, service));
    }

    /// Stops answering: claktory.com is unreachable.
    fn stop(&self) {
        self.server.abort();
    }

    /// Answers again, at the same address, with the same database and keys.
    async fn restart(&mut self) {
        self.stop();
        let listener = loop {
            match tokio::net::TcpListener::bind(self.addr).await {
                Ok(listener) => break listener,
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
            }
        };
        self.serve(listener);
    }

    /// The service's clock moves on, so its once-a-minute share is given again.
    fn advance(&self, seconds: i64) {
        self.ahead.fetch_add(seconds, Ordering::SeqCst);
    }

    fn last_code(&self, email: &str) -> String {
        let mail = self.mail.sent().into_iter().rev().find(|mail| mail.to == email).expect("an email");
        mail.subject.rsplit(' ').next().expect("a code").to_owned()
    }

    fn count(&self, query: &str) -> i64 {
        rusqlite::Connection::open(&self.db).expect("the service's database").query_row(query, [], |row| row.get(0)).expect("sql")
    }

    fn sql(&self, statement: &str) {
        rusqlite::Connection::open(&self.db).expect("the service's database").execute(statement, []).expect("sql");
    }
}

/// A Claktory with signing in on, at `service`.
async fn open(dir: &Path, service: &str) -> (std::net::SocketAddr, Arc<factoryd::Factory>) {
    let config = dir.join("home/config.json");
    if !config.exists() {
        std::fs::create_dir_all(dir.join("home")).expect("home");
        std::fs::write(&config, format!(r#"{{"account": {{"enabled": true, "service": "{service}"}}}}"#)).expect("config");
    }
    start(dir).await
}

fn account(workspace: &v1::Workspace) -> v1::ClaktoryAccount {
    workspace.account.clone().expect("account state")
}

fn failure(body: &Body) -> (ErrorCode, String) {
    match body {
        Body::Error(error) => (error.code(), error.message.clone()),
        other => panic!("expected an error, got {other:?}"),
    }
}

/// The first workspace, from `first` on, whose account (and vault) passes
/// `done`.
async fn until(client: &mut Client, first: v1::Workspace, done: impl Fn(&v1::Workspace) -> bool) -> v1::Workspace {
    if done(&first) { first } else { wait_for(client, done).await }
}

/// Waits for a workspace whose account (and vault) passes `done`.
async fn wait_for(client: &mut Client, done: impl Fn(&v1::Workspace) -> bool) -> v1::Workspace {
    client
        .wait_for(|body| match body {
            Body::Workspace(workspace) if done(workspace) => Some(workspace.clone()),
            _ => None,
        })
        .await
}

/// Signs in; the workspace after it.
async fn sign_in(client: &mut Client, instance: &Instance, email: &str) -> v1::Workspace {
    client.ok(Request::AccountSignIn(v1::AccountSignIn { email: email.into() })).await;
    let code = instance.last_code(email);
    let events = client.ok(Request::AccountVerify(v1::AccountVerify { code })).await;
    last_workspace(&events).expect("workspace").clone()
}

fn unlocked(workspace: &v1::Workspace) -> bool {
    workspace.vault.as_ref().is_some_and(|vault| vault.unlocked)
}

fn mode(path: &Path) -> u32 {
    std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(path).expect("a file").permissions()) & 0o777
}

#[tokio::test]
async fn signing_in_protects_the_vault_with_the_account_which_opens_it_after_a_restart() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let secret;
    {
        let (addr, _factory) = open(&dir, &instance.url).await;
        let (mut client, workspace, _) = Client::connect(addr).await;
        let state = account(&workspace);
        assert_eq!((state.state(), state.service.as_str(), state.email.as_str()), (AccountState::SignedOut, instance.url.as_str(), ""));
        assert!(!state.name.is_empty(), "the computer's name");
        let added = v1::CreateSecret {
            name: "AWS".into(),
            entries: vec![v1::SecretEntry {
                variable: "AWS_SECRET_ACCESS_KEY".into(),
                value: "wJalrXUtnFEMI-account-test".into(),
                file: false,
            }],
            ..Default::default()
        };
        let events = client.ok(Request::CreateSecret(added)).await;
        secret = last_workspace(&events).and_then(|workspace| workspace.vault.clone()).expect("vault").secrets[0].id.clone();

        let early = client.request(Request::AccountVerify(v1::AccountVerify { code: "123456".into() })).await;
        assert_eq!(failure(&early.body).0, ErrorCode::InvalidRequest, "no code asked for yet");
        let bad = client.request(Request::AccountSignIn(v1::AccountSignIn { email: "matt".into() })).await;
        assert_eq!(failure(&bad.body), (ErrorCode::InvalidRequest, "Enter an email address, like you@example.com.".into()));

        let events = client.ok(Request::AccountSignIn(v1::AccountSignIn { email: " Matt@Example.com ".into() })).await;
        let state = account(last_workspace(&events).expect("workspace"));
        assert_eq!((state.state(), state.email.as_str()), (AccountState::AwaitingCode, "matt@example.com"));
        assert!(state.code_expires_ms > factoryd::transcript::now_ms() + 800_000, "{}", state.code_expires_ms);
        let sent: serde_json::Value = serde_json::from_str(&state.last_sign_in_json).expect("the JSON sent");
        assert_eq!(sent["email"], "matt@example.com");
        assert!(sent["claktory_id"].is_string() && sent["backend_key"].is_string() && sent["platform"].is_string(), "{sent}");
        assert!(state.last_sign_in_ms > 0);
        let moved = client.request(Request::SetAccountService(v1::SetAccountService { url: "https://example.com".into() })).await;
        assert_eq!(failure(&moved.body), (ErrorCode::InvalidRequest, "Sign out first.".into()));

        let code = instance.last_code("matt@example.com");
        let wrong = if code == "000000" { "111111" } else { "000000" };
        let refused = client.request(Request::AccountVerify(v1::AccountVerify { code: wrong.into() })).await;
        assert_eq!(failure(&refused.body), (ErrorCode::WrongProof, "That code isn't right. 4 tries left.".into()));
        let events = client.ok(Request::AccountVerify(v1::AccountVerify { code })).await;
        let workspace = last_workspace(&events).expect("workspace");
        let state = account(workspace);
        assert_eq!((state.state(), state.email.as_str(), state.problem.as_str()), (AccountState::SignedIn, "matt@example.com", ""));
        assert!(workspace.vault.as_ref().is_some_and(|vault| vault.unlocked));
        assert!(!dir.join("home/vault/auto.key").exists(), "the key on disk goes");
        for file in ["account.json", "claktory.key", "share.key"] {
            assert_eq!(mode(&dir.join("home/account").join(file)), 0o600, "{file}");
        }
        assert_eq!(mode(&dir.join("home/account")), 0o700);
        let saved = std::fs::read_to_string(dir.join("home/account/account.json")).expect("account.json");
        assert!(saved.contains("machine_token") && !saved.contains("wJalrXUtnFEMI"), "{saved}");

        // It checks in at once.
        let workspace = until(&mut client, workspace.clone(), |workspace| account(workspace).last_check_in_ms > 0).await;
        let state = account(&workspace);
        let sent: serde_json::Value = serde_json::from_str(&state.last_check_in_json).expect("the JSON sent");
        assert!(sent["version"].is_string() && sent["platform"].is_string(), "{sent}");
        assert_eq!(sent["counts"], serde_json::json!({"agents": 0, "projects": 0, "devices": 0}));
        assert!(sent.get("periods").is_none(), "no whole hour since signing in: {sent}");
        assert!(state.last_contact_ms > 0);
        assert_eq!(instance.count("SELECT COUNT(*) FROM endpoints WHERE kind = 'backend'"), 1, "the backend's endpoint registered");

        // The recovery key, to note down: the same each time, and logged.
        let shown = client.request(Request::ShowRecoveryKey(v1::ShowRecoveryKey {})).await;
        let Body::RecoveryKey(key) = shown.body else { panic!("expected the recovery key, got {:?}", shown.body) };
        let again = client.request(Request::ShowRecoveryKey(v1::ShowRecoveryKey {})).await;
        assert!(matches!(again.body, Body::RecoveryKey(ref again) if again.key == key.key));
    }

    // A model call is counted for the next check-in.
    {
        let (addr, _factory) = open(&dir, &instance.url).await;
        let (mut client, _, _) = Client::connect(addr).await;
        client.ok(create_agent("", "Builder", "")).await;
        client.ok(send_message("builder", "hello")).await;
        let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
        let mut counted = 0i64;
        for _ in 0..100 {
            counted = db.query_row("SELECT COALESCE(SUM(requests), 0) FROM model_usage", [], |row| row.get(0)).expect("model_usage");
            if counted > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert_eq!(counted, 1, "the answer's tokens");
        // As if signed in two hours ago, with a call an hour ago.
        let hour = factoryd::transcript::now_ms() / 1000 / 3600 * 3600;
        db.execute("INSERT INTO model_usage VALUES (?1, 'anthropic', 'claude-opus-4-8', 3, 100, 20, 5000, 300)", [hour - 3600])
            .expect("usage");
        let path = dir.join("home/account/account.json");
        let mut saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("account.json")).expect("json");
        saved["periods_until"] = serde_json::json!(hour - 7200);
        std::fs::write(&path, saved.to_string()).expect("account.json");
    }

    // After a restart the vault opens with the service's share, and the
    // check-in sends the hours since.
    instance.advance(61);
    let (addr, _factory) = open(&dir, &instance.url).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let workspace =
        until(&mut client, workspace, |workspace| unlocked(workspace) && account(workspace).last_check_in_json.contains("periods")).await;
    let sent: serde_json::Value = serde_json::from_str(&account(&workspace).last_check_in_json).expect("the JSON sent");
    assert_eq!(sent["periods"].as_array().map(Vec::len), Some(1), "{sent}");
    let period = &sent["periods"][0];
    assert_eq!(period["to"].as_i64().zip(period["from"].as_i64()).map(|(to, from)| to - from), Some(7200), "the two whole hours");
    assert_eq!(period["usage"][0]["requests"], 3, "{sent}");
    assert_eq!(sent["counts"]["agents"], 1);
    assert_eq!(instance.count("SELECT COUNT(*) FROM periods"), 1, "recorded");
    let saved = std::fs::read_to_string(dir.join("home/account/account.json")).expect("account.json");
    assert!(saved.contains("\"periods\": []"), "dropped once accepted: {saved}");
    let revealed = client.request(Request::RevealSecret(v1::RevealSecret { id: secret.clone(), proof: None })).await;
    let Body::SecretValue(value) = revealed.body else { panic!("expected the value, got {:?}", revealed.body) };
    assert_eq!(value.entries[0].value, "wJalrXUtnFEMI-account-test");
    assert!(
        instance.mail.sent().iter().any(|mail| mail.to == "matt@example.com" && mail.subject.contains("unlocked")),
        "the unlock is emailed"
    );

    // Signing out: the vault opens by itself again, with a key on disk.
    let events = client.ok(Request::AccountSignOut(v1::AccountSignOut {})).await;
    let state = account(last_workspace(&events).expect("workspace"));
    assert_eq!((state.state(), state.email.as_str()), (AccountState::SignedOut, ""));
    assert!(dir.join("home/vault/auto.key").exists());
    client.ok(Request::SetAccountService(v1::SetAccountService { url: "https://account.example.com/".into() })).await;
    let config = std::fs::read_to_string(dir.join("home/config.json")).expect("config");
    assert!(config.contains("\"service\": \"https://account.example.com\""), "{config}");
    let refused = client.request(Request::SetAccountService(v1::SetAccountService { url: "http://example.com".into() })).await;
    assert_eq!(failure(&refused.body).0, ErrorCode::InvalidRequest, "http only on this computer");
    drop(client);
    let (addr, _factory) = open(&dir, &instance.url).await;
    let (_client, workspace, _) = Client::connect(addr).await;
    assert!(workspace.vault.as_ref().is_some_and(|vault| vault.unlocked), "open with no service");
}

#[tokio::test]
async fn while_the_service_is_unreachable_the_vault_waits_and_opens_by_itself() {
    let dir = temp_dir();
    let mut instance = Instance::start(&dir).await;
    {
        let (addr, _factory) = open(&dir, &instance.url).await;
        let (mut client, _, _) = Client::connect(addr).await;
        sign_in(&mut client, &instance, "matt@example.com").await;
    }
    instance.stop();
    let (addr, _factory) = open(&dir, &instance.url).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let workspace = until(&mut client, workspace, |workspace| !account(workspace).problem.is_empty()).await;
    assert!(!workspace.vault.as_ref().is_some_and(|vault| vault.unlocked), "locked");
    assert_eq!(account(&workspace).problem, "Can't reach 127.0.0.1. Claktory keeps working and tries again on its own.");
    assert_eq!(account(&workspace).state(), AccountState::SignedIn);
    let refused = client.request(Request::RevealSecret(v1::RevealSecret { id: "AWS".into(), proof: None })).await;
    assert_eq!(failure(&refused.body).0, ErrorCode::VaultLocked);

    // Back: it opens on the next try, or at once when an app connects.
    instance.restart().await;
    let (mut other, workspace, _) = Client::connect(addr).await;
    let workspace = until(&mut other, workspace, |workspace| unlocked(workspace) && account(workspace).problem.is_empty()).await;
    assert_eq!(account(&workspace).state(), AccountState::SignedIn);
}

#[tokio::test]
async fn a_claktory_removed_at_the_service_opens_only_with_the_recovery_key_until_it_signs_in_again() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let recovery;
    {
        let (addr, _factory) = open(&dir, &instance.url).await;
        let (mut client, _, _) = Client::connect(addr).await;
        sign_in(&mut client, &instance, "matt@example.com").await;
        let shown = client.request(Request::ShowRecoveryKey(v1::ShowRecoveryKey {})).await;
        let Body::RecoveryKey(key) = shown.body else { panic!("expected the recovery key") };
        recovery = key.key;
    }
    instance.sql("UPDATE claktories SET revoked_at = 1");
    let (addr, _factory) = open(&dir, &instance.url).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let workspace = until(&mut client, workspace, |workspace| account(workspace).state() == AccountState::Revoked).await;
    assert!(!workspace.vault.as_ref().is_some_and(|vault| vault.unlocked));

    // Signing in again while it's locked: the last epoch's share opens it,
    // and it's sealed under the new one.
    let workspace = sign_in(&mut client, &instance, "matt@example.com").await;
    assert_eq!(account(&workspace).state(), AccountState::SignedIn);
    assert!(workspace.vault.as_ref().is_some_and(|vault| vault.unlocked), "{:?}", account(&workspace));
    instance.advance(61);
    drop(client);
    let (addr, _factory) = open(&dir, &instance.url).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    until(&mut client, workspace, unlocked).await;

    // The recovery key still opens it, whatever the service says.
    instance.sql("UPDATE claktories SET revoked_at = 1");
    instance.advance(61);
    drop(client);
    let (addr, _factory) = open(&dir, &instance.url).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    until(&mut client, workspace, |workspace| account(workspace).state() == AccountState::Revoked).await;
    let opened = client.request(Request::UnlockVault(v1::UnlockVault { passphrase: recovery })).await;
    assert!(matches!(opened.body, Body::VaultSession(_)), "{:?}", opened.body);
}

#[tokio::test]
async fn while_signing_in_is_off_nothing_shows_and_nothing_is_sent() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    assert_eq!(workspace.account, None);
    let refused = client.request(Request::AccountSignIn(v1::AccountSignIn { email: "matt@example.com".into() })).await;
    assert_eq!(failure(&refused.body).0, ErrorCode::InvalidRequest);
    assert!(!dir.join("home/account").exists(), "nothing made");
    // The recovery key is the vault's, signed in or not.
    let shown = client.request(Request::ShowRecoveryKey(v1::ShowRecoveryKey {})).await;
    assert!(matches!(shown.body, Body::RecoveryKey(ref key) if !key.key.is_empty()), "{:?}", shown.body);
}
