#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use factory_agent::credentials::{Credential, CredentialInfo, FileCredentialStore};
use factory_agent::errors::{AgentFailure, Error};
use serde_json::json;

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).expect("metadata").permissions().mode() & 0o777
}

/// On Windows, a mode as private as 0600 or 0700 is: only this user and
/// SYSTEM.
#[cfg(windows)]
fn mode(path: &Path) -> u32 {
    let private = factory_agent::windows::system::is_private_path(path).expect("security");
    match (private, path.is_dir()) {
        (true, true) => 0o700,
        (true, false) => 0o600,
        (false, _) => 0o644,
    }
}

fn corrupt(error: &Error) -> (&AgentFailure, bool) {
    let Error::Agent(agent) = error else { panic!("{error}") };
    (&agent.failure, agent.cause.is_some())
}

fn api_key(key: &str) -> Credential {
    Credential::ApiKey { key: key.into(), extra: Default::default() }
}

#[tokio::test]
async fn the_first_write_creates_missing_directories_private_and_leaves_no_temp_file() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("factory").join("secrets");
    let auth_file = dir.join("auth.json");
    let store = FileCredentialStore::new(auth_file.clone());
    store.modify("openrouter", |_| async { Ok(Some(api_key("k"))) }).await.unwrap();
    assert_eq!((mode(&dir), mode(&home.path().join("factory")), mode(&auth_file)), (0o700, 0o700, 0o600));
    let names: Vec<String> =
        std::fs::read_dir(&dir).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["auth.json"]);
}

#[cfg(unix)]
#[tokio::test]
async fn a_rewrite_keeps_the_existing_owner_group_and_mode() {
    let home = tempfile::tempdir().unwrap();
    let auth_file = home.path().join("auth.json");
    let store = FileCredentialStore::new(auth_file.clone());
    store.modify("openrouter", |_| async { Ok(Some(api_key("a"))) }).await.unwrap();
    let created = std::fs::metadata(&auth_file).unwrap().gid();
    let mut groups = vec![0; 256];
    let count = unsafe { libc::getgroups(groups.len() as libc::c_int, groups.as_mut_ptr()) };
    if let Some(other) = groups[..usize::try_from(count).unwrap()].iter().copied().find(|gid| *gid != created) {
        std::os::unix::fs::chown(&auth_file, None, Some(other)).unwrap();
    }
    let before = std::fs::metadata(&auth_file).unwrap();
    store.modify("openrouter", |_| async { Ok(Some(api_key("b"))) }).await.unwrap();
    let after = std::fs::metadata(&auth_file).unwrap();
    assert_eq!((after.uid(), after.gid(), mode(&auth_file)), (before.uid(), before.gid(), 0o600));
    assert_eq!(store.read("openrouter").await.unwrap(), Some(api_key("b")));
}

#[tokio::test]
async fn malformed_credential_json_is_a_corrupt_credentials_failure() {
    let home = tempfile::tempdir().unwrap();
    let auth_file = home.path().join("auth.json");
    std::fs::write(&auth_file, "{").unwrap();
    let store = FileCredentialStore::new(auth_file.clone());
    let error = store.list().await.unwrap_err();
    assert_eq!(corrupt(&error), (&AgentFailure::CorruptCredentials { file: auth_file.display().to_string() }, true));
}

#[tokio::test]
async fn the_store_serializes_writes_and_keeps_fields_it_does_not_model() {
    let home = tempfile::tempdir().unwrap();
    let auth_file = home.path().join("auth.json");
    let store = FileCredentialStore::new(auth_file.clone());
    assert_eq!(store.read("openai-codex").await.unwrap(), None);
    assert_eq!(store.list().await.unwrap(), vec![]);
    let oauth = Credential::Oauth {
        access: "a".into(),
        refresh: "r".into(),
        expires: 1_789_661_414_272,
        account_id: Some("acct".into()),
        extra: json!({"enterpriseUrl": "https://x"}).as_object().unwrap().clone(),
    };
    let written = store.modify("openai-codex", |current| async move {
        assert_eq!(current, None);
        Ok(Some(oauth))
    });
    assert_eq!(written.await.unwrap().map(|c| c.kind()), Some("oauth"));
    let kept = store.modify("openai-codex", |current| async move {
        assert!(current.is_some());
        Ok(None)
    });
    let access = kept.await.unwrap().and_then(|c| match c {
        Credential::Oauth { access, .. } => Some(access),
        Credential::ApiKey { .. } => None,
    });
    assert_eq!(access, Some("a".into()));
    store.modify("openrouter", |_| async { Ok(Some(Credential::ApiKey { key: "k".into(), extra: Default::default() })) }).await.unwrap();
    let mut on_disk: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&auth_file).unwrap()).unwrap();
    // Each is its provider's first account, with the id Factory gave it.
    for (_, credential) in on_disk.as_object_mut().unwrap() {
        assert!(credential.as_object_mut().unwrap().remove("factoryAccount").unwrap().is_string());
    }
    assert_eq!(
        on_disk,
        json!({"openai-codex": {"type": "oauth", "access": "a", "refresh": "r", "expires": 1_789_661_414_272_u64, "accountId": "acct", "enterpriseUrl": "https://x"}, "openrouter": {"type": "api_key", "key": "k"}})
    );
    assert_eq!(mode(&auth_file), 0o600);
    assert_eq!(
        store.list().await.unwrap(),
        vec![
            CredentialInfo { provider_id: "openai-codex".into(), kind: "oauth".into() },
            CredentialInfo { provider_id: "openrouter".into(), kind: "api_key".into() }
        ]
    );
    store.delete("openai-codex").await.unwrap();
    store.delete("missing").await.unwrap();
    assert_eq!(store.read("openai-codex").await.unwrap(), None);
    assert_eq!(store.read("openrouter").await.unwrap().map(|c| c.kind()), Some("api_key"));
}

/// A credentials document as a vault keeps it: an OAuth sign-in with a
/// second account, and a key.
fn document() -> serde_json::Map<String, serde_json::Value> {
    let second = json!({"type": "oauth", "access": "sk-ant-oat01-second", "refresh": "sk-ant-ort01-second", "expires": 9,
        "factoryAccount": "b", "factoryLabel": "b@x.com", "factoryIdentity": "user-b", "factoryRefusedSince": 5});
    json!({
        "anthropic": {"type": "oauth", "access": "sk-ant-oat01-first", "refresh": "sk-ant-ort01-first", "expires": 7,
            "accountId": "acct-1", "factoryAccount": "a", "factoryLabel": "a@x.com", "factoryIdentity": "user-a",
            "factoryExhaustedUntil": 3, "factoryAccounts": [second]},
        "openrouter": {"type": "api_key", "key": "sk-or-v1-secret", "factoryAccount": "k", "factoryIdentity": "key:0123abcd"},
    })
    .as_object()
    .expect("a document")
    .clone()
}

/// What's known of sign-ins locked away holds no key or token, nor anything
/// made from one (an API key's identity is a hash of it).
#[tokio::test]
async fn an_outline_keeps_the_accounts_and_none_of_their_secrets() {
    let outline = factory_agent::credentials::outline(&document());
    let text = serde_json::Value::Object(outline.clone()).to_string();
    for secret in ["sk-ant-oat01", "sk-ant-ort01", "sk-or-v1", "user-a", "user-b", "key:0123", "acct-1"] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }
    // It reads as the accounts, their kinds, labels and states.
    let dir = tempfile::tempdir().unwrap();
    let auth = dir.path().join("auth.json");
    factory_agent::credentials::write_document(&auth, &outline).unwrap();
    let store = FileCredentialStore::new(auth);
    let accounts = store.accounts("anthropic").await.unwrap();
    let shown: Vec<_> = accounts
        .iter()
        .map(|account| (account.id.as_str(), account.label.as_str(), account.credential.kind(), account.refused_since))
        .collect();
    assert_eq!(shown, [("a", "a@x.com", "oauth", 0), ("b", "b@x.com", "oauth", 5)]);
    assert_eq!(accounts[0].exhausted_until, 3);
    assert_eq!(store.accounts("openrouter").await.unwrap()[0].credential, api_key(""));
}

/// Sign-ins made elsewhere come in as signing in again would: the same
/// account (by who it is, else by id) is replaced and keeps its id; another
/// is added after, with an id of its own when the one it has is taken.
#[tokio::test]
async fn sign_ins_made_elsewhere_are_merged_in() {
    use factory_agent::credentials::merge;
    let mut into = document();
    assert!(!merge(&mut into, &document()).unwrap(), "the same sign-ins change nothing");
    let from = json!({"anthropic": {"type": "oauth", "access": "new", "refresh": "new-r", "expires": 8, "factoryAccount": "z",
        "factoryIdentity": "user-a", "factoryAccounts": [
            {"type": "oauth", "access": "c", "refresh": "c-r", "expires": 1, "factoryAccount": "b", "factoryIdentity": "user-c"},
        ]},
        "openai": {"type": "api_key", "key": "sk-new", "factoryAccount": "1"}});
    assert!(merge(&mut into, from.as_object().unwrap()).unwrap());
    let dir = tempfile::tempdir().unwrap();
    let auth = dir.path().join("auth.json");
    factory_agent::credentials::write_document(&auth, &into).unwrap();
    let store = FileCredentialStore::new(auth);
    let accounts = store.accounts("anthropic").await.unwrap();
    let ids: Vec<&str> = accounts.iter().map(|account| account.id.as_str()).collect();
    assert_eq!(&ids[..2], ["a", "b"]);
    assert!(matches!(&accounts[0].credential, Credential::Oauth { access, .. } if access == "new"), "a is signed in again");
    assert_eq!(accounts[1].identity, "user-b", "b keeps its own: c only shares its id");
    assert_eq!((accounts.len(), accounts[2].identity.as_str()), (3, "user-c"));
    assert!(ids[2] != "b" && !ids[2].is_empty(), "c gets an id of its own");
    assert_eq!(store.read("openai").await.unwrap(), Some(api_key("sk-new")));

    // One that can't be read is carried along as it is, never in the way of
    // the others; it never replaces sign-ins that can be.
    let unreadable = json!({"type": "api", "key": "sk-odd"});
    let from = json!({"mistral": unreadable, "anthropic": unreadable, "openrouter": {"type": "api_key", "key": "sk-or-2", "factoryAccount": "k2"}});
    assert!(merge(&mut into, from.as_object().unwrap()).unwrap());
    assert_eq!(into["mistral"], unreadable);
    assert_eq!(into["anthropic"]["access"], "new", "the readable ones stay");
    assert_eq!(into["openrouter"]["factoryAccounts"][0]["key"], "sk-or-2");
}

/// Sign-ins locked away (a backing that has only their outline, and
/// `auth.json` for those made meanwhile).
#[derive(Default)]
struct Locked {
    plain: parking_lot::Mutex<serde_json::Map<String, serde_json::Value>>,
    saved: parking_lot::Mutex<Vec<bool>>,
}

impl factory_agent::credentials::CredentialBacking for Locked {
    fn load(&self) -> Result<factory_agent::credentials::Loaded, Error> {
        Ok(factory_agent::credentials::Loaded {
            all: self.plain.lock().clone(),
            locked: Some(factory_agent::credentials::outline(&document())),
        })
    }

    fn save(&self, all: &serde_json::Map<String, serde_json::Value>, locked: bool) -> Result<(), Error> {
        *self.plain.lock() = all.clone();
        self.saved.lock().push(locked);
        Ok(())
    }
}

/// While they're locked away, a provider's sign-ins can't be used (they
/// wait), only shown; one signed in to meanwhile can, and is saved as made
/// while locked. Removing those locked away waits for them to open.
#[tokio::test]
async fn sign_ins_locked_away_are_shown_and_a_new_one_works_meanwhile() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileCredentialStore::new(dir.path().join("auth.json"));
    let backing = std::sync::Arc::new(Locked::default());
    store.set_backing(backing.clone());
    assert!(store.accounts("anthropic").await.unwrap_err().is_locked());
    assert_eq!(store.accounts("openai").await.unwrap(), [], "a provider with none locked away is just signed out");
    let shown: Vec<String> = store.outlined_accounts("anthropic").await.unwrap().into_iter().map(|account| account.id).collect();
    assert_eq!(shown, ["a", "b"]);
    let mut listed: Vec<String> = store.list().await.unwrap().into_iter().map(|info| info.provider_id).collect();
    listed.sort();
    assert_eq!(listed, ["anthropic", "openrouter"]);

    // Signed in meanwhile: it's used at once.
    store
        .modify_accounts("anthropic", |mut accounts| async move {
            accounts.push(factory_agent::credentials::Account::new(api_key("sk-ant-api-new")));
            Ok(Some(accounts))
        })
        .await
        .unwrap();
    assert_eq!(*backing.saved.lock(), [true]);
    let usable = store.accounts("anthropic").await.unwrap();
    assert_eq!((usable.len(), &usable[0].credential), (1, &api_key("sk-ant-api-new")));
    let shown = store.outlined_accounts("anthropic").await.unwrap();
    assert_eq!(shown.len(), 3, "the new one, then those locked away");
    assert!(store.delete("anthropic").await.unwrap_err().is_locked());
}
