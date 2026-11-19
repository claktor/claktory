use std::sync::Arc;

use factory_agent::auth::{ApiKeyAuth, Env, EnvApiKeyAuth};
use factory_agent::credentials::Credential;

fn env(pairs: &[(&str, &str)]) -> Env {
    let pairs: Vec<(String, String)> = pairs.iter().map(|(name, value)| ((*name).to_owned(), (*value).to_owned())).collect();
    Arc::new(move |name| pairs.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone()))
}

#[test]
fn the_base_url_variable_replaces_the_providers_url_for_either_key() {
    let auth = EnvApiKeyAuth { name: "Relay key", env_vars: &["RELAY_API_KEY"], base_url_var: Some("RELAY_BASE_URL") };
    let relayed = env(&[("RELAY_API_KEY", "placeholder"), ("RELAY_BASE_URL", "http://10.231.0.1:8080")]);
    let result = auth.resolve(&relayed, None).unwrap().unwrap();
    assert_eq!(result.auth.api_key.as_deref(), Some("placeholder"));
    assert_eq!(result.auth.base_url.as_deref(), Some("http://10.231.0.1:8080"));
    let stored = Credential::ApiKey { key: "stored".into(), extra: Default::default() };
    let result = auth.resolve(&relayed, Some(&stored)).unwrap().unwrap();
    assert_eq!((result.auth.api_key.as_deref(), result.auth.base_url.as_deref()), (Some("stored"), Some("http://10.231.0.1:8080")));
    let result = auth.resolve(&env(&[("RELAY_API_KEY", "key")]), None).unwrap().unwrap();
    assert_eq!(result.auth.base_url, None);
}
