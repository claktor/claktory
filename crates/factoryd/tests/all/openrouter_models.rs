//! OpenRouter's model list as last fetched from OpenRouter (U4): kept in
//! the home folder and taken in at start, in place of the bundled one.

use std::sync::Arc;

use crate::common::{Client, temp_dir};
use factory_agent::auth::Env;
use factory_agent::credentials::FileCredentialStore;
use factory_agent::providers::models::{Registry, default_providers};
use factoryd::factory::Factory;
use serde_json::json;
use tokio::net::TcpListener;

/// A listing as OpenRouter sends it: 60 models that run agents, and
/// "vendor/brand-new".
fn listing() -> Vec<u8> {
    let data: Vec<serde_json::Value> = (0..60)
        .map(|index| format!("vendor/model-{index:03}"))
        .chain(["vendor/brand-new".to_owned()])
        .map(|id| {
            json!({
                "id": id,
                "name": format!("Vendor: {id}"),
                "context_length": 128_000,
                "architecture": {"input_modalities": ["text"], "output_modalities": ["text"]},
                "pricing": {"prompt": "0.000001", "completion": "0.000002"},
                "supported_parameters": ["tools"],
            })
        })
        .collect();
    serde_json::to_vec(&json!({"data": data})).expect("json")
}

/// The models a Claktory signed in to OpenRouter lists at start, with
/// `kept` as the list kept from the last fetch.
async fn models_at_start(kept: &[u8]) -> Vec<String> {
    let dir = temp_dir();
    let home = crate::common::home(&dir);
    std::fs::create_dir_all(home.root()).expect("home");
    std::fs::write(home.root().join("openrouter-models.json"), kept).expect("kept list");
    std::fs::write(home.auth_path(), json!({"openrouter": {"type": "api_key", "key": "sk-or-test"}}).to_string()).expect("sign-in");
    let env: Env = Arc::new(|_| None);
    let registry = Registry::with_providers(FileCredentialStore::new(home.auth_path()), env, default_providers());
    let factory = Factory::open(home.clone(), Arc::new(registry)).await.expect("open");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    crate::common::serves(addr, home);
    tokio::spawn(factoryd::serve(listener, factory.clone()));
    let (_client, workspace, _) = Client::connect(addr).await;
    workspace.models.into_iter().map(|model| model.id).collect()
}

#[tokio::test]
async fn openrouters_models_as_last_fetched_are_there_from_the_start() {
    let models = models_at_start(&listing()).await;
    assert!(models.iter().any(|id| id == "openrouter/vendor/brand-new"), "{models:?}");
    assert!(!models.iter().any(|id| id == "openrouter/openai/o1"), "bundled, but no longer listed");
}

#[tokio::test]
async fn a_kept_list_that_fails_its_checks_leaves_the_bundled_one() {
    let models = models_at_start(br#"{"data": []}"#).await;
    assert!(models.iter().any(|id| id == "openrouter/openai/o1"), "{models:?}");
    assert!(!models.iter().any(|id| id == "openrouter/vendor/brand-new"));
}
