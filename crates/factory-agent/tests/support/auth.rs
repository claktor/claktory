use std::sync::Arc;

use factory_agent::auth::{AuthEvent, AuthFuture, AuthInteraction, AuthPrompt, OAuthFlow};
use factory_agent::errors::Error;
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

type Answer = Arc<dyn Fn(&AuthPrompt, &[AuthEvent]) -> Option<String> + Send + Sync>;

/// A scripted [`AuthInteraction`]: answers prompts from a closure that sees the events recorded so far.
pub struct Scripted {
    answer: Answer,
    pub events: Arc<Mutex<Vec<AuthEvent>>>,
    pub prompts: Arc<Mutex<Vec<AuthPrompt>>>,
    pub cancel: CancellationToken,
}

impl AuthInteraction for Scripted {
    fn prompt<'a>(&'a self, prompt: AuthPrompt) -> AuthFuture<'a, Result<String, Error>> {
        self.prompts.lock().push(prompt.clone());
        let answer = (self.answer)(&prompt, &self.events.lock());
        let cancel = self.cancel.clone();
        Box::pin(async move {
            match answer {
                Some(answer) => Ok(answer),
                None => {
                    cancel.cancelled().await;
                    Err(Error::Aborted)
                }
            }
        })
    }

    fn notify(&self, event: AuthEvent) {
        self.events.lock().push(event);
    }

    fn cancel(&self) -> CancellationToken {
        self.cancel.clone()
    }
}

pub fn scripted(answer: impl Fn(&AuthPrompt, &[AuthEvent]) -> Option<String> + Send + Sync + 'static) -> Scripted {
    Scripted {
        answer: Arc::new(answer),
        events: Arc::new(Mutex::new(Vec::new())),
        prompts: Arc::new(Mutex::new(Vec::new())),
        cancel: CancellationToken::new(),
    }
}

/// The error text a login produces, without the denied `unwrap_err`.
pub async fn login_error(flow: &impl OAuthFlow, interaction: &Scripted) -> String {
    flow.login(interaction).await.expect_err("login error").to_string()
}

/// The S256 PKCE challenge for a verifier, as the flows compute it.
pub fn pkce_challenge(text: &str) -> String {
    use base64::Engine;
    use sha2::Digest;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(text.as_bytes()))
}
