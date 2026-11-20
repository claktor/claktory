//! The real providers behind the app's sign-in, exercised offline: nothing
//! here reaches the network because every sign-in is cancelled before a code
//! is exchanged.

use std::sync::Mutex;

use factory_agent::Error;
use factory_agent::auth::{AuthEvent, AuthFuture, AuthInteraction, AuthPrompt};
use factoryd::models::{API_KEY, Models, OAUTH, Registry};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Records what a provider shows and asks, and never answers.
struct Recorder {
    seen: Mutex<Vec<String>>,
    asked: mpsc::UnboundedSender<AuthPrompt>,
    cancel: CancellationToken,
}

impl AuthInteraction for Recorder {
    fn prompt<'a>(&'a self, prompt: AuthPrompt) -> AuthFuture<'a, Result<String, Error>> {
        let _ = self.asked.send(prompt);
        Box::pin(async move {
            self.cancel.cancelled().await;
            Err(Error::Aborted)
        })
    }

    fn notify(&self, event: AuthEvent) {
        if let AuthEvent::AuthUrl { url, .. } = event {
            self.seen.lock().expect("lock").push(url);
        }
    }

    fn cancel(&self) -> CancellationToken {
        self.cancel.clone()
    }
}

#[tokio::test]
async fn anthropic_offers_its_subscription_in_the_browser_with_a_paste_fallback() {
    let registry = Registry::new(crate::common::temp_dir().join("auth.json"));
    let providers = Models::providers(&registry).await;
    let anthropic = providers.iter().find(|provider| provider.id == "anthropic").expect("anthropic is offered");
    let mut methods = Vec::new();
    for method in &anthropic.methods {
        methods.push((method.id.as_str(), method.label.as_str()));
    }
    assert_eq!(methods, [(OAUTH, "Anthropic (Claude Pro/Max)"), (API_KEY, "Anthropic API key")]);

    let (asked, mut questions) = mpsc::unbounded_channel();
    let recorder = Recorder { seen: Mutex::new(Vec::new()), asked, cancel: CancellationToken::new() };
    let login = Models::login(&registry, "anthropic", OAUTH, &recorder, false);
    tokio::pin!(login);
    // The browser flow publishes its URL, then waits for the redirect or a pasted code.
    let question = tokio::select! {
        question = questions.recv() => question.expect("a question"),
        outcome = &mut login => panic!("sign-in ended early: {outcome:?}"),
    };
    assert!(matches!(question, AuthPrompt::ManualCode { .. }), "{question:?}");
    let url = recorder.seen.lock().expect("lock").first().cloned().expect("an authorization URL");
    assert!(url.starts_with("https://claude.ai/oauth/authorize?"), "{url}");
    assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A53692%2Fcallback"), "{url}");

    recorder.cancel.cancel();
    assert!(matches!(login.await, Err(Error::Aborted)));
}
