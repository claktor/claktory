//! Signing in to model providers from the app.
//!
//! A provider's sign-in talks to an `AuthInteraction`: it publishes a URL or a
//! device code and asks questions. Here those become `Workspace.login`, which
//! every client shows, and `AnswerLogin` requests answer the questions. So one
//! flow covers browser sign-in with its redirect, a pasted code or redirect
//! address, device codes and API keys, for every provider.

use std::sync::Arc;

use factory_agent::Error;
use factory_agent::auth::{AuthEvent, AuthFuture, AuthInteraction, AuthPrompt};
use factory_protocol::v1::{self, LoginPromptKind};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::factory::{Factory, Inner, RequestError};

pub(crate) struct Flow {
    /// Tells a finished sign-in apart from a newer one that replaced it.
    generation: u64,
    pub(crate) view: v1::LoginFlow,
    answers: mpsc::UnboundedSender<String>,
    cancel: CancellationToken,
}

struct Interaction {
    factory: Arc<Factory>,
    generation: u64,
    answers: tokio::sync::Mutex<mpsc::UnboundedReceiver<String>>,
    cancel: CancellationToken,
}

impl Factory {
    /// Starts signing in, replacing any sign-in in progress.
    pub async fn start_login(self: &Arc<Self>, provider_id: &str, method_id: &str) -> Result<(), RequestError> {
        let providers = self.lock().providers.clone();
        let Some(provider) = providers.iter().find(|provider| provider.id == provider_id) else {
            return Err(RequestError::UnknownProvider(provider_id.to_owned()));
        };
        if !provider.methods.iter().any(|method| method.id == method_id) {
            return Err(RequestError::UnknownLoginMethod { provider: provider.name.clone(), method: method_id.to_owned() });
        }
        let (answers, receiver) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let generation = {
            let mut inner = self.lock();
            if let Some(previous) = inner.login.take() {
                previous.cancel.cancel();
            }
            inner.login_generation += 1;
            let generation = inner.login_generation;
            let view = v1::LoginFlow { provider_id: provider_id.to_owned(), method_id: method_id.to_owned(), ..Default::default() };
            inner.login = Some(Flow { generation, view, answers, cancel: cancel.clone() });
            self.broadcast_workspace(&inner);
            generation
        };
        let interaction = Interaction { factory: self.clone(), generation, answers: tokio::sync::Mutex::new(receiver), cancel };
        let factory = self.clone();
        let (provider_id, method_id) = (provider_id.to_owned(), method_id.to_owned());
        tokio::spawn(async move {
            let outcome = factory.models.login(&provider_id, &method_id, &interaction).await;
            factory.finish_login(generation, outcome).await;
        });
        Ok(())
    }

    pub fn answer_login(&self, text: &str) -> Result<(), RequestError> {
        let inner = self.lock();
        let Some(flow) = inner.login.as_ref().filter(|flow| flow.view.prompt.is_some()) else {
            return Err(RequestError::NoLoginQuestion);
        };
        // The receiver lives as long as the sign-in; a closed one means it just ended.
        flow.answers.send(text.trim().to_owned()).map_err(|_| RequestError::NoLoginQuestion)
    }

    /// Cancels the sign-in in progress, or dismisses a failed one.
    pub fn cancel_login(&self) {
        let mut inner = self.lock();
        if let Some(flow) = inner.login.take() {
            flow.cancel.cancel();
            self.broadcast_workspace(&inner);
        }
    }

    pub async fn sign_out(&self, provider_id: &str) -> Result<(), RequestError> {
        if !self.lock().providers.iter().any(|provider| provider.id == provider_id) {
            return Err(RequestError::UnknownProvider(provider_id.to_owned()));
        }
        self.models.logout(provider_id).await?;
        self.refresh_models().await;
        Ok(())
    }

    async fn finish_login(&self, generation: u64, outcome: Result<(), Error>) {
        match outcome {
            Ok(()) => {
                // The new credential changes which providers and models are available.
                self.refresh_models().await;
                let mut inner = self.lock();
                if inner.login.as_ref().is_some_and(|flow| flow.generation == generation) {
                    inner.login = None;
                    self.broadcast_workspace(&inner);
                }
            }
            // Cancelled or replaced: whoever did that already updated the flow.
            Err(Error::Aborted) => {}
            Err(error) => {
                tracing::warn!(%error, "sign-in failed");
                self.update_flow(generation, |view| {
                    view.prompt = None;
                    view.error = error.to_string();
                });
            }
        }
    }

    fn update_flow(&self, generation: u64, change: impl FnOnce(&mut v1::LoginFlow)) {
        let mut inner = self.lock();
        let Inner { login: Some(flow), .. } = &mut *inner else { return };
        if flow.generation != generation {
            return;
        }
        change(&mut flow.view);
        self.broadcast_workspace(&inner);
    }
}

impl AuthInteraction for Interaction {
    fn prompt<'a>(&'a self, prompt: AuthPrompt) -> AuthFuture<'a, Result<String, Error>> {
        Box::pin(async move {
            let question = question(prompt);
            self.factory.update_flow(self.generation, |view| view.prompt = Some(question));
            let mut answers = self.answers.lock().await;
            let answer = tokio::select! {
                answer = answers.recv() => answer,
                () = self.cancel.cancelled() => None,
            };
            self.factory.update_flow(self.generation, |view| view.prompt = None);
            answer.ok_or(Error::Aborted)
        })
    }

    fn notify(&self, event: AuthEvent) {
        self.factory.update_flow(self.generation, |view| match event {
            AuthEvent::AuthUrl { url, instructions } => {
                view.url = url;
                view.instructions = instructions.unwrap_or_default();
            }
            AuthEvent::DeviceCode { user_code, verification_uri, .. } => {
                view.user_code = user_code;
                view.verification_url = verification_uri;
            }
            AuthEvent::Info { message, links } => {
                let mut text = message;
                for link in links {
                    text.push('\n');
                    text.push_str(&link.url);
                }
                view.progress = text;
            }
            AuthEvent::Progress { message } => view.progress = message,
        });
    }

    fn cancel(&self) -> CancellationToken {
        self.cancel.clone()
    }
}

fn question(prompt: AuthPrompt) -> v1::LoginPrompt {
    let (kind, message, placeholder, choices) = match prompt {
        AuthPrompt::Text { message, placeholder } => (LoginPromptKind::Text, message, placeholder, Vec::new()),
        AuthPrompt::Secret { message, placeholder } => (LoginPromptKind::Secret, message, placeholder, Vec::new()),
        AuthPrompt::ManualCode { message, placeholder } => (LoginPromptKind::Code, message, placeholder, Vec::new()),
        AuthPrompt::Select { message, options } => (LoginPromptKind::Select, message, None, options),
    };
    let mut options = Vec::new();
    for choice in choices {
        options.push(v1::LoginOption { id: choice.id, label: choice.label });
    }
    v1::LoginPrompt { kind: kind.into(), message, placeholder: placeholder.unwrap_or_default(), options }
}
