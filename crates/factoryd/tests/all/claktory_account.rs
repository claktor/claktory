//! Signing in to claktory.com (#116), against a local account service
//! (`claktory-account`): the email and its code, the vault protected by the
//! account instead of a key on disk, opened again after a restart with the
//! service's share (by itself once the service is back), check-ins, sign
//! out, and removal at the service.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use claktory_account::server::{self, Config, Secrets, Service, UserApps, mail};
use factory_protocol::v1::{self, ClaktoryAccountState as AccountState, ErrorCode, client_message::Body as Request, server_message::Body};

use crate::common::{Client, create_agent, last_workspace, send_message, start, temp_dir};

/// A local account service, on a port of its own, with mail kept in memory
/// and a clock a test can move on.
pub(crate) struct Instance {
    pub(crate) url: String,
    addr: std::net::SocketAddr,
    db: std::path::PathBuf,
    mail: Arc<mail::Memory>,
    ahead: Arc<AtomicI64>,
    pub(crate) secrets: ([u8; 32], [u8; 32]),
    /// What it serves as its root's word for its key (F-OK1), if anything.
    pub(crate) statement: Option<claktory_account::protocol::SignedStatement>,
    /// The user-apps worker it forwards `/v1/apps/…` to, if any.
    user_apps: Option<UserApps>,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Instance {
    pub(crate) async fn start(dir: &Path) -> Instance {
        Instance::start_with_apps(dir, None).await
    }

    /// One that forwards its apps' calls to the worker at `user_apps`.
    pub(crate) async fn start_with_apps(dir: &Path, user_apps: Option<UserApps>) -> Instance {
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
            statement: None,
            user_apps,
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
        let config = Config { service_key_statement: self.statement.clone(), user_apps: self.user_apps.clone(), ..Config::default() };
        let service = Service::new(&self.db, secrets, config, self.mail.clone(), clock).expect("the service");
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

    /// Its current key, vouched for from `from` by `root` (id "test-root").
    fn vouch(&mut self, root: &ed25519_dalek::SigningKey, from: i64) {
        use base64::Engine;
        use ed25519_dalek::Signer;
        let key = ed25519_dalek::SigningKey::from_bytes(&self.secrets.1).verifying_key();
        let statement = claktory_account::protocol::KeyStatement {
            service: self.url.clone(),
            key: claktory_account::protocol::b64(key.to_bytes()),
            from,
        };
        let text = statement.text();
        let signature = base64::engine::general_purpose::STANDARD.encode(root.sign(text.as_bytes()).to_bytes());
        self.statement = Some(claktory_account::protocol::SignedStatement { text, signature: format!("test-root:{signature}") });
    }

    pub(crate) fn last_code(&self, email: &str) -> String {
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
pub(crate) async fn open(dir: &Path, service: &str) -> (std::net::SocketAddr, Arc<factoryd::Factory>) {
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
pub(crate) async fn sign_in(client: &mut Client, instance: &Instance, email: &str) -> v1::Workspace {
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
        assert_eq!(sent["counts"], serde_json::json!({"agents": 0, "projects": 0, "devices": 0, "pipes": 0, "secrets": 1, "monitors": 0}));
        assert!(sent.get("periods").is_none(), "no whole hour since signing in: {sent}");
        assert!(state.last_contact_ms > 0);
        assert_eq!(instance.count("SELECT COUNT(*) FROM endpoints WHERE kind = 'backend'"), 1, "the backend's endpoint registered");
        // With its name, color and relays, for the person's devices to find it.
        let (name, color, relays): (String, String, String) = rusqlite::Connection::open(&instance.db)
            .expect("the service's database")
            .query_row("SELECT display_name, color, relays FROM claktories", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("its name, color and relays");
        assert!(!name.is_empty() && !color.is_empty(), "{name:?} {color:?}");
        assert!(serde_json::from_str::<Vec<String>>(&relays).is_ok(), "{relays}");

        // The recovery key, to note down: the same each time, and logged.
        let shown = client.request(Request::ShowRecoveryKey(v1::ShowRecoveryKey {})).await;
        let Body::RecoveryKey(key) = shown.body else { panic!("expected the recovery key, got {:?}", shown.body) };
        let again = client.request(Request::ShowRecoveryKey(v1::ShowRecoveryKey {})).await;
        assert!(matches!(again.body, Body::RecoveryKey(ref again) if again.key == key.key));

        // Saved? Needs you asks until a group of it is typed back (R1).
        assert!(workspace.vault.as_ref().is_some_and(|vault| vault.recovery_key_unconfirmed), "signed in, a key to confirm");
        let check = client.request(Request::CheckRecoveryKey(v1::CheckRecoveryKey {})).await;
        let Body::RecoveryKeyCheck(check) = check.body else { panic!("expected a check, got {:?}", check.body) };
        assert_eq!(check.groups, 8);
        let group = key.key.split('-').nth(check.group as usize - 1).expect("the group").to_owned();
        let wrong = if group == "AAAA" { "BBBB" } else { "AAAA" };
        let refused = client
            .request(Request::ConfirmRecoveryKey(v1::ConfirmRecoveryKey { check_id: check.check_id.clone(), typed: wrong.into() }))
            .await;
        let said = format!("That isn't block {} of your recovery key. Check what you saved and try again.", check.group);
        assert_eq!(failure(&refused.body), (ErrorCode::WrongProof, said));
        let stale = client
            .request(Request::ConfirmRecoveryKey(v1::ConfirmRecoveryKey { check_id: "not-this-one".into(), typed: group.clone() }))
            .await;
        assert_eq!(failure(&stale.body), (ErrorCode::InvalidRequest, "This check timed out. Start again.".into()));
        let events =
            client.ok(Request::ConfirmRecoveryKey(v1::ConfirmRecoveryKey { check_id: check.check_id, typed: group.to_lowercase() })).await;
        let workspace = last_workspace(&events).expect("workspace");
        assert!(workspace.vault.as_ref().is_some_and(|vault| !vault.recovery_key_unconfirmed), "confirmed, for every device");
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
    // A minute after signing in, from where it signed in: the person's own
    // unlock, not emailed (#412: only an unusual one is).
    assert!(
        !instance.mail.sent().iter().any(|mail| mail.to == "matt@example.com" && mail.subject.contains("unlocked")),
        "the unlock just after signing in isn't emailed"
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
async fn after_the_services_share_key_changes_the_recovery_key_opens_the_vault_once_and_it_opens_by_itself_after() {
    let dir = temp_dir();
    let mut instance = Instance::start(&dir).await;
    let recovery;
    {
        let (addr, _factory) = open(&dir, &instance.url).await;
        let (mut client, _, _) = Client::connect(addr).await;
        sign_in(&mut client, &instance, "matt@example.com").await;
        let shown = client.request(Request::ShowRecoveryKey(v1::ShowRecoveryKey {})).await;
        let Body::RecoveryKey(key) = shown.body else { panic!("expected the recovery key") };
        recovery = key.key;
    }
    // The service's share key is rotated (a leak, say): its shares change,
    // so the vault no longer opens with the account.
    instance.secrets.0 = *claktory_account::server::Secrets::generate().k_share;
    instance.restart().await;
    instance.advance(61);
    let (addr, _factory) = open(&dir, &instance.url).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let workspace = until(&mut client, workspace, |workspace| !account(workspace).problem.is_empty()).await;
    assert!(!unlocked(&workspace), "locked");
    instance.advance(61);
    let workspace = sign_in(&mut client, &instance, "matt@example.com").await;
    assert_eq!(account(&workspace).problem, "Your secrets stay locked until you open them with the recovery key.");

    // The recovery key opens it, and it's sealed under the new share at
    // once: no check-in, sign-out or second sign-in needed.
    instance.advance(61);
    let opened = client.request(Request::UnlockVault(v1::UnlockVault { passphrase: recovery })).await;
    assert!(matches!(opened.body, Body::VaultSession(_)), "{:?}", opened.body);
    wait_for(&mut client, |workspace| unlocked(workspace) && account(workspace).problem.is_empty()).await;
    drop(client);
    instance.advance(61);
    let (addr, _factory) = open(&dir, &instance.url).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    until(&mut client, workspace, unlocked).await;
}

/// The service's signing key is rotated (KEY-LEAK.md): its answers no longer
/// check against the key pinned at sign-in. The Claktory says so once, rather
/// than spending the one share a minute on retries, and signing in again pins
/// the new key and opens the vault, whose shares didn't change.
#[tokio::test]
async fn after_the_services_signing_key_changes_signing_in_again_opens_the_vault() {
    let dir = temp_dir();
    let mut instance = Instance::start(&dir).await;
    {
        let (addr, _factory) = open(&dir, &instance.url).await;
        let (mut client, _, _) = Client::connect(addr).await;
        sign_in(&mut client, &instance, "matt@example.com").await;
    }
    instance.secrets.1 = claktory_account::server::Secrets::generate().signing_key.to_bytes();
    instance.restart().await;
    instance.advance(61);
    let (addr, _factory) = open(&dir, &instance.url).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let workspace = until(&mut client, workspace, |workspace| !account(workspace).problem.is_empty()).await;
    assert!(!unlocked(&workspace), "locked");
    assert_eq!(account(&workspace).problem, "127.0.0.1 changed its key. Sign in again, and your secrets open by themselves.");
    instance.advance(61);
    sign_in(&mut client, &instance, "matt@example.com").await;
    wait_for(&mut client, |workspace| unlocked(workspace) && account(workspace).problem.is_empty()).await;
    // One share went to the answer that didn't check, one to signing in again.
    assert_eq!(instance.count("SELECT count(*) FROM share_releases"), 2);
}

#[tokio::test]
async fn while_signing_in_is_off_nothing_shows_and_nothing_is_sent() {
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("home")).expect("home");
    std::fs::write(dir.join("home/config.json"), r#"{"account": {"enabled": false}}"#).expect("config");
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

/// Onboarding slice 3: an agent signs a Claktory in from a shell, with the
/// code the person reads in their email.
#[tokio::test]
async fn the_command_line_signs_in_with_the_emailed_code() {
    use factoryd::cli::{Exit, Flags, run};
    let dir = temp_dir();
    let service = Instance::start(&dir).await;
    let (addr, _factory) = open(&dir, &service.url).await;
    let instance = factoryd::home::Instance { home: crate::common::home(&dir), addr };
    let command = |line: String| {
        let args: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
        Flags::parse(&args).expect("a command")
    };
    let asked = run(&instance, &command("account sign-in --email matt@example.com --json".into()), String::new).await;
    assert_eq!(asked.exit, Exit::NeedsHuman, "{asked:?}");
    assert_eq!((asked.json["state"].as_str(), asked.json["sent_to"].as_str()), (Some("awaiting_code"), Some("m…@example.com")));
    assert_eq!(asked.json["sent_from"], "code@auth.claktory.com");
    assert!(asked.json["expires_in"].as_i64().is_some_and(|left| left > 0), "{}", asked.json);
    let wrong = run(&instance, &command("account verify --code 000000 --json".into()), String::new).await;
    assert_eq!((wrong.exit, wrong.json["error"].as_str()), (Exit::Refused, Some("wrong_code")), "{wrong:?}");
    let code = service.last_code("matt@example.com");
    let verified = run(&instance, &command(format!("account verify --code {code} --json")), String::new).await;
    assert_eq!(verified.exit, Exit::Ok, "{verified:?}");
    assert_eq!((verified.json["state"].as_str(), verified.json["email"].as_str()), (Some("signed_in"), Some("matt@example.com")));
    let status = run(&instance, &command("account status --json".into()), String::new).await;
    assert_eq!(status.json["account"]["state"], "signed_in");
}

/// Signing in is on by default, and every Claktory must sign in (#195): a
/// new one, one from before sign-in, and one that skipped the step in an
/// earlier version are all asked, and Not now is refused. The code step
/// says where the code comes from.
#[tokio::test]
async fn every_claktory_is_asked_to_sign_in_and_none_may_skip_it() {
    // New: no config.json yet, so it's made now, with signing in on.
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let state = account(&workspace);
    assert_eq!((state.state(), state.skippable), (AccountState::SignedOut, false));
    let skip = client.request(Request::SkipAccountSignIn(v1::SkipAccountSignIn {})).await;
    assert_eq!(failure(&skip.body), (ErrorCode::InvalidRequest, "Sign in to use this Claktory.".into()));
    let config = std::fs::read_to_string(dir.join("home/config.json")).expect("config.json");
    assert!(!config.contains("\"account\""), "on by default, so not written: {config}");

    // One that skipped the step in an earlier version is asked again.
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    std::fs::create_dir_all(dir.join("home/account")).expect("home");
    std::fs::write(dir.join("home/config.json"), format!(r#"{{"account": {{"service": "{}"}}}}"#, instance.url)).expect("config");
    std::fs::write(dir.join("home/account/account.json"), r#"{"step": "skipped"}"#).expect("account.json");
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let state = account(&workspace);
    assert_eq!((state.state(), state.skippable, state.code_from.as_str()), (AccountState::SignedOut, false, ""));
    let saved = std::fs::read_to_string(dir.join("home/account/account.json")).expect("account.json");
    assert!(saved.contains("\"step\": \"asked\""), "so an earlier version asks too: {saved}");
    let events = client.ok(Request::AccountSignIn(v1::AccountSignIn { email: "matt@example.com".into() })).await;
    let state = account(last_workspace(&events).expect("workspace"));
    assert_eq!((state.state(), state.skippable), (AccountState::AwaitingCode, false));
    assert_eq!(state.code_from, "code@auth.claktory.com", "the service says where codes come from");
    let skip = client.request(Request::SkipAccountSignIn(v1::SkipAccountSignIn {})).await;
    assert_eq!(failure(&skip.body), (ErrorCode::InvalidRequest, "Sign in to use this Claktory.".into()));
    let code = instance.last_code("matt@example.com");
    let events = client.ok(Request::AccountVerify(v1::AccountVerify { code })).await;
    let state = account(last_workspace(&events).expect("workspace"));
    assert_eq!((state.state(), state.skippable), (AccountState::SignedIn, false), "the code still works after a refused Not now");
    let skip = client.request(Request::SkipAccountSignIn(v1::SkipAccountSignIn {})).await;
    assert_eq!(failure(&skip.body), (ErrorCode::InvalidRequest, "This Claktory is already signed in.".into()));
    let events = client.ok(Request::AccountSignOut(v1::AccountSignOut {})).await;
    assert_eq!(account(last_workspace(&events).expect("workspace")).state(), AccountState::SignedOut, "signed out, it asks again");
}

/// Signed out of one account, signing in to another takes one code: the
/// Claktory proves to the service that it's the same one, and moves.
#[tokio::test]
async fn switching_accounts_takes_one_code() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (addr, _factory) = open(&dir, &instance.url).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let first = account(&sign_in(&mut client, &instance, "matt@example.com").await);
    assert_eq!(first.state(), AccountState::SignedIn);
    client.ok(Request::AccountSignOut(v1::AccountSignOut {})).await;

    let workspace = sign_in(&mut client, &instance, "ana@example.com").await;
    let state = account(&workspace);
    assert_eq!((state.state(), state.email.as_str(), state.problem.as_str()), (AccountState::SignedIn, "ana@example.com", ""));
    assert_eq!(instance.count("SELECT COUNT(*) FROM claktories"), 1, "the same Claktory, moved");
    assert_eq!(instance.count("SELECT COUNT(*) FROM accounts"), 2);
}

/// Moved to another computer (#217): at its first start there, a Claktory
/// changes its token and backend key at the service, so the copy it moved
/// from can't use them; its vault still opens with the same share.
#[tokio::test]
async fn a_claktory_that_moved_here_changes_its_credentials_once_and_its_vault_still_opens() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let account_file = |name: &str| std::fs::read_to_string(dir.join("home/account").join(name)).expect("a file");
    let (old_key, old_token) = {
        let (addr, _factory) = open(&dir, &instance.url).await;
        let (mut client, _, _) = Client::connect(addr).await;
        let workspace = sign_in(&mut client, &instance, "matt@example.com").await;
        assert!(unlocked(&workspace));
        let saved: serde_json::Value = serde_json::from_str(&account_file("account.json")).expect("json");
        (account_file("claktory.key"), saved["machine_token"].as_str().expect("a token").to_owned())
    };
    // As import leaves it.
    std::fs::write(dir.join("home/move-epoch"), "1\n").expect("write");
    instance.advance(61);

    let (addr, _factory) = open(&dir, &instance.url).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    until(&mut client, workspace, unlocked).await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let saved: serde_json::Value = serde_json::from_str(&account_file("account.json")).expect("json");
        if saved["move_epoch"] == 1 {
            assert_ne!(saved["machine_token"].as_str(), Some(old_token.as_str()), "a new token");
            break;
        }
        assert!(std::time::Instant::now() < deadline, "the credentials weren't changed: {saved}");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_ne!(account_file("claktory.key"), old_key, "a new backend key");
    assert_eq!(mode(&dir.join("home/account/claktory.key")), 0o600);
    assert!(!dir.join("home/account/moving.json").exists());
    assert!(instance.mail.sent().iter().any(|mail| mail.subject.contains(" moved to ")), "the account is told");

    // The old pair no longer works at the service: the copy left behind is locked out.
    let seed: [u8; 32] = factoryd_unhex(&old_key).try_into().expect("a seed");
    let old = ed25519_dalek::SigningKey::from_bytes(&seed);
    let body = br#"{"version":"v0","platform":"macos-arm64"}"#.to_vec();
    let time = jiff::Timestamp::now().as_second() + 61;
    let signature = claktory_account::protocol::sign_request(&old, "POST", "/v1/check-in", time, &body);
    let answer = reqwest::Client::new()
        .post(format!("{}/v1/check-in", instance.url))
        .header("authorization", format!("Bearer {old_token}"))
        .header(claktory_account::protocol::TIME_HEADER, time.to_string())
        .header(claktory_account::protocol::SIGNATURE_HEADER, signature)
        .body(body)
        .send()
        .await
        .expect("an answer");
    assert_eq!(answer.status().as_u16(), 401);

    // Started again, nothing more is changed.
    let token = account_file("account.json");
    drop(client);
    drop(_factory);
    instance.advance(61);
    let (addr, _factory) = open(&dir, &instance.url).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    until(&mut client, workspace, unlocked).await;
    let saved: serde_json::Value = serde_json::from_str(&account_file("account.json")).expect("json");
    let before: serde_json::Value = serde_json::from_str(&token).expect("json");
    assert_eq!(saved["machine_token"], before["machine_token"]);
}

fn factoryd_unhex(text: &str) -> Vec<u8> {
    let text = text.trim();
    (0..text.len()).step_by(2).map(|at| u8::from_str_radix(&text[at..at + 2], 16).expect("hex")).collect()
}

/// F-OK1: a service anchored to a root (as claktory.com is, once its root's
/// statement is live). No statement, no sign-in; then a key it rotates to is
/// taken by itself on the root's word, so the vault opens with no sign-in;
/// and an older key, even one the root once vouched for, isn't taken back.
#[tokio::test]
async fn an_anchored_service_s_key_is_taken_only_on_its_root_s_word_and_never_rolled_back() {
    use base64::Engine;
    let dir = temp_dir();
    let mut instance = Instance::start(&dir).await;
    let root = ed25519_dalek::SigningKey::from_bytes(&[11; 32]);
    let roots = vec![("test-root".to_owned(), base64::engine::general_purpose::STANDARD.encode(root.verifying_key().to_bytes()))];
    const FLOOR: i64 = 1_795_000_000;
    factoryd::anchor_account_for_tests(&instance.url, roots, FLOOR);
    let saved = |dir: &Path| -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(dir.join("home/account/account.json")).expect("account.json")).expect("json")
    };
    // Nothing vouches for its key, then a statement older than this build's
    // floor: no sign-in. (Each with the Claktory started again, so it
    // reaches the service as it's served now.)
    for from in [None, Some(FLOOR - 1)] {
        if let Some(from) = from {
            instance.vouch(&root, from);
            instance.restart().await;
        }
        let (addr, _factory) = open(&dir, &instance.url).await;
        let (mut client, _, _) = Client::connect(addr).await;
        let refused = client.request(Request::AccountSignIn(v1::AccountSignIn { email: "matt@example.com".into() })).await;
        assert_eq!(
            failure(&refused.body),
            (ErrorCode::Unreachable, "Couldn't confirm this is really 127.0.0.1, so you can't sign in right now. Try again later.".into())
        );
    }
    instance.vouch(&root, FLOOR);
    instance.restart().await;
    {
        let (addr, _factory) = open(&dir, &instance.url).await;
        let (mut client, _, _) = Client::connect(addr).await;
        sign_in(&mut client, &instance, "matt@example.com").await;
        assert_eq!(saved(&dir)["service_key_from"], FLOOR);
    }

    // Rotated, with the root's word for the new key: taken by itself.
    let old = (instance.secrets.1, instance.statement.clone());
    instance.secrets.1 = Secrets::generate().signing_key.to_bytes();
    instance.vouch(&root, FLOOR + 10);
    instance.restart().await;
    instance.advance(61);
    let (addr, _factory) = open(&dir, &instance.url).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let workspace = until(&mut client, workspace, unlocked).await;
    assert_eq!(account(&workspace).problem, "");
    let new_key = claktory_account::protocol::b64(ed25519_dalek::SigningKey::from_bytes(&instance.secrets.1).verifying_key().to_bytes());
    assert_eq!((saved(&dir)["service_key"].as_str(), saved(&dir)["service_key_from"].as_i64()), (Some(new_key.as_str()), Some(FLOOR + 10)));
    drop(client);

    // The old key back, with its old statement (a leak, served by someone
    // else): not taken, and said so.
    instance.secrets.1 = old.0;
    instance.statement = old.1;
    instance.restart().await;
    instance.advance(61);
    let (addr, _factory) = open(&dir, &instance.url).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let workspace = until(&mut client, workspace, |workspace| !account(workspace).problem.is_empty()).await;
    assert!(!unlocked(&workspace));
    assert_eq!(
        account(&workspace).problem,
        "Couldn't confirm this is really 127.0.0.1, so your secrets stay locked for now. Claktory keeps trying."
    );
    assert_eq!(saved(&dir)["service_key"].as_str(), Some(new_key.as_str()), "the pinned key stays");
}

#[tokio::test]
async fn a_check_in_counts_devices_setup_and_how_backend_versions_fared() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let home = dir.join("home");
    {
        let (addr, factory) = open(&dir, &instance.url).await;
        // The backend before stopped without stopping cleanly, and the
        // supervisor rolled an update back: each counted once, however
        // often this backend starts.
        std::fs::write(home.join("backend-running"), "v0.1.14").expect("marker");
        std::fs::write(
            home.join("update-failed.json"),
            r#"{"version": "v0.1.16", "back_to": "v0.1.15", "reason": "It couldn't serve the app."}"#,
        )
        .expect("update-failed.json");
        factoryd::backend_started(&factory);
        assert!(home.join("backend-running").exists(), "this one runs");
        factoryd::backend_stopped(factory.home());
        assert!(!home.join("backend-running").exists(), "stopped cleanly");
        factoryd::backend_started(&factory);
        // An app on a laptop says its system and version; a tool says none.
        let mut app = Client::open(addr).await;
        let hello = v1::Hello {
            protocol_version: factory_protocol::PROTOCOL_VERSION,
            client_version: "v0.1.16".into(),
            platform: "macos".into(),
            token: crate::common::local_token(addr),
            device_kind: v1::DeviceKind::Laptop.into(),
            ..Default::default()
        };
        app.send(v1::ClientMessage { id: 1, body: Some(v1::client_message::Body::Hello(hello)) }).await;
        app.welcomed().await;
        let (mut client, _, _) = Client::connect(addr).await;
        let workspace = sign_in(&mut client, &instance, "matt@example.com").await;
        let workspace = until(&mut client, workspace, |workspace| account(workspace).last_check_in_ms > 0).await;
        let sent: serde_json::Value = serde_json::from_str(&account(&workspace).last_check_in_json).expect("the JSON sent");
        assert_eq!(sent["counts"], serde_json::json!({"agents": 0, "projects": 0, "devices": 0, "pipes": 0, "secrets": 0, "monitors": 0}));
        assert_eq!(sent["setup"], serde_json::json!({"model_connected": true, "first_run": false}));
        assert_eq!(sent["devices"], serde_json::json!([{"platform": "macos", "kind": "laptop", "app_version": "v0.1.16", "count": 1}]));
    }
    // As if the hour before held it all, and the hours since were unsent.
    let db = rusqlite::Connection::open(home.join("factory.db")).expect("factory.db");
    let hour = factoryd::transcript::now_ms() / 1000 / 3600 * 3600;
    for table in ["device_hours", "backend_health"] {
        db.execute(&format!("UPDATE {table} SET hour = ?1"), [hour - 3600]).expect("an hour back");
    }
    // A run that never reached a model (none to run on) isn't a first run.
    db.execute("INSERT INTO runs (agent, version, started, ended, outcome) VALUES ('rivet', 'v0.1.16', ?1, ?1, 'finished')", [hour * 1000]).expect("a run");
    let path = home.join("account/account.json");
    let mut saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("account.json")).expect("json");
    saved["periods_until"] = serde_json::json!(hour - 7200);
    std::fs::write(&path, saved.to_string()).expect("account.json");

    instance.advance(61);
    let (addr, _factory) = open(&dir, &instance.url).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let workspace = until(&mut client, workspace, |workspace| account(workspace).last_check_in_json.contains("periods")).await;
    let sent: serde_json::Value = serde_json::from_str(&account(&workspace).last_check_in_json).expect("the JSON sent");
    let period = &sent["periods"][0];
    assert_eq!(period["devices_connected"], 1, "this computer's app, however often it connected: {sent}");
    assert_eq!(
        period["health"],
        serde_json::json!([
            {"version": "v0.1.14", "rolled_back": 0, "crashes": 1},
            {"version": "v0.1.16", "rolled_back": 1, "crashes": 0},
        ]),
        "{sent}"
    );
    assert_eq!(instance.count("SELECT COALESCE(SUM(crashes), 0) FROM health"), 1, "recorded at the service");
    // A model connected stays reached; no agent has reached a model.
    assert_eq!(sent["setup"], serde_json::json!({"model_connected": true, "first_run": false}));
}
