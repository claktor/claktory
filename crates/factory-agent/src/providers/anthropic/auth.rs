use crate::auth::{ApiKeyAuth, AuthFuture, AuthInteraction, AuthResult, Env, RequestAuth, secret_login};
use crate::credentials::Credential;
use crate::errors::Error;

/// A stored key, else `ANTHROPIC_AUTH_TOKEN` as a bearer header, else `ANTHROPIC_OAUTH_TOKEN` or `ANTHROPIC_API_KEY` as the key.
pub struct AnthropicApiKeyAuth;

impl ApiKeyAuth for AnthropicApiKeyAuth {
    fn name(&self) -> &str {
        "Anthropic API key"
    }

    fn login<'a>(&'a self, interaction: &'a dyn AuthInteraction) -> AuthFuture<'a, Result<Credential, Error>> {
        secret_login(interaction, "Enter Anthropic API key".into())
    }

    fn resolve(&self, env: &Env, credential: Option<&Credential>) -> Result<Option<AuthResult>, Error> {
        if let Some(Credential::ApiKey { key, .. }) = credential
            && !key.is_empty()
        {
            return Ok(Some(AuthResult {
                auth: RequestAuth { api_key: Some(key.clone()), ..RequestAuth::default() },
                source: "stored credential".into(),
            }));
        }
        if let Some(token) = env("ANTHROPIC_AUTH_TOKEN") {
            let auth = RequestAuth { headers: vec![("Authorization".into(), format!("Bearer {token}"))], ..RequestAuth::default() };
            return Ok(Some(AuthResult { auth, source: "ANTHROPIC_AUTH_TOKEN".into() }));
        }
        for name in ["ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"] {
            if let Some(key) = env(name) {
                return Ok(Some(AuthResult { auth: RequestAuth { api_key: Some(key), ..RequestAuth::default() }, source: name.into() }));
            }
        }
        Ok(None)
    }
}
