use std::sync::Arc;

use factory_agent::auth::{ApiKeyAuth, Env, RequestAuth};
use factory_agent::credentials::Credential;
use factory_agent::providers::anthropic::auth::AnthropicApiKeyAuth;

fn env(pairs: &[(&str, &str)]) -> Env {
    let pairs: Vec<(String, String)> = pairs.iter().map(|(name, value)| ((*name).to_owned(), (*value).to_owned())).collect();
    Arc::new(move |name| pairs.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone()))
}

#[test]
fn a_stored_key_wins_then_the_auth_token_as_a_bearer_header_then_the_key_variables() {
    let auth = AnthropicApiKeyAuth;
    assert_eq!(auth.name(), "Anthropic API key");
    let stored = Credential::ApiKey { key: "stored".into(), extra: Default::default() };
    let all = env(&[("ANTHROPIC_AUTH_TOKEN", "tok"), ("ANTHROPIC_OAUTH_TOKEN", "oat"), ("ANTHROPIC_API_KEY", "key")]);
    let result = auth.resolve(&all, Some(&stored)).unwrap().unwrap();
    assert_eq!((result.auth.api_key.as_deref(), result.source.as_str()), (Some("stored"), "stored credential"));
    let result = auth.resolve(&all, None).unwrap().unwrap();
    assert_eq!(result.auth, RequestAuth { api_key: None, headers: vec![("Authorization".into(), "Bearer tok".into())], base_url: None });
    assert_eq!(result.source, "ANTHROPIC_AUTH_TOKEN");
    let result = auth.resolve(&env(&[("ANTHROPIC_OAUTH_TOKEN", "oat"), ("ANTHROPIC_API_KEY", "key")]), None).unwrap().unwrap();
    assert_eq!((result.auth.api_key.as_deref(), result.source.as_str()), (Some("oat"), "ANTHROPIC_OAUTH_TOKEN"));
    let result = auth.resolve(&env(&[("ANTHROPIC_API_KEY", "key")]), None).unwrap().unwrap();
    assert_eq!((result.auth.api_key.as_deref(), result.source.as_str()), (Some("key"), "ANTHROPIC_API_KEY"));
    assert_eq!(auth.resolve(&env(&[]), None).unwrap(), None);
    let empty = Credential::ApiKey { key: String::new(), extra: Default::default() };
    assert_eq!(auth.resolve(&env(&[("ANTHROPIC_API_KEY", "key")]), Some(&empty)).unwrap().unwrap().source, "ANTHROPIC_API_KEY");
}
