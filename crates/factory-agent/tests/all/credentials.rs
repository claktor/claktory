use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use factory_agent::credentials::{Credential, CredentialInfo, FileCredentialStore};
use factory_agent::errors::{AgentFailure, Error};
use serde_json::json;

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).expect("metadata").permissions().mode() & 0o777
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
