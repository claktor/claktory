//! A new Claktory's first run: the steps the app takes the person through,
//! held here so every device continues where another left off (tenet 7).
//!
//! "Connect a model" is needed when a Claktory starts for the first time
//! (no config.json yet, as the account's sign-in step counts it) with no
//! model available. The person connects one, or skips it; the first time a
//! model is available the step is done, and Rivet greets them: one event
//! from Claktory starts it, after moving it to a model that can run if its
//! own can't.
//!
//! "When Claktory quits" (friend bug #27) is needed when a new Claktory
//! runs under its Mac's background service, which the Mac app can stop and
//! start (`crate::service`): the person answers it
//! (UpdateSettings.when_app_quits) or skips it, and it keeps running.
//!
//! The state lives in factory.db's `onboarding`, not config.json, so a
//! backend from before it still reads the settings after a rollback. A
//! Claktory with no row there (one from before it, or whose first run is
//! over) has every step done.

use std::sync::{Arc, Mutex, Weak};

use factory_protocol::v1::{self, OnboardingStep};
use rusqlite::OptionalExtension;

use crate::factory::{Factory, RequestError, host_event};
use crate::locking::lock;
use crate::store::Store;

pub(crate) const SCHEMA: &str = "
-- A new Claktory's first-run steps still to do: 'model' (Connect a model),
-- needed or skipped; 'quit' (When Claktory quits, friend bug #27), needed
-- or skipped. A step done has no row.
CREATE TABLE onboarding (step TEXT PRIMARY KEY, state TEXT NOT NULL);
";

/// The model Rivet moves to when the first model available is a provider's
/// and its own can't run, by provider; otherwise the provider's first.
const PREFERRED: &[&str] =
    &[crate::config::DEFAULT_MODEL, "openai-codex/gpt-6.1-sol", "openai/gpt-5.6-sol", "openrouter/anthropic/claude-opus-5"];

/// What Factory keeps of the first run while it runs.
#[derive(Default)]
pub(crate) struct State {
    model: Mutex<OnboardingStep>,
    quit: Mutex<OnboardingStep>,
    /// Notified when the models available change.
    pub(crate) models_changed: Arc<tokio::sync::Notify>,
}

impl State {
    /// The steps as factory.db has them. A new Claktory (`created`) with no
    /// model available starts with Connect a model needed; one its Mac's
    /// background service runs (`service`), with When Claktory quits.
    pub(crate) fn load(store: &Store, created: bool, models: &[v1::Model], service: bool) -> rusqlite::Result<State> {
        if created && models.is_empty() {
            store.db().execute("INSERT OR REPLACE INTO onboarding (step, state) VALUES ('model', 'needed')", [])?;
        }
        if created && service {
            store.db().execute("INSERT OR REPLACE INTO onboarding (step, state) VALUES ('quit', 'needed')", [])?;
        }
        let step = |name: &str| -> rusqlite::Result<OnboardingStep> {
            let state: Option<String> =
                store.db().query_row("SELECT state FROM onboarding WHERE step = ?1", [name], |row| row.get(0)).optional()?;
            Ok(match state.as_deref() {
                Some("needed") => OnboardingStep::Needed,
                Some("skipped") => OnboardingStep::Skipped,
                _ => OnboardingStep::Done,
            })
        };
        Ok(State { model: Mutex::new(step("model")?), quit: Mutex::new(step("quit")?), models_changed: Arc::default() })
    }

    /// What Settings.onboarding shows.
    pub(crate) fn view(&self) -> v1::Onboarding {
        v1::Onboarding { model: (*lock(&self.model)).into(), when_app_quits: (*lock(&self.quit)).into() }
    }
}

/// The person answered When Claktory quits: done.
pub(crate) fn when_app_quits_answered(factory: &Factory) -> rusqlite::Result<()> {
    if *lock(&factory.onboarding.quit) != OnboardingStep::Done {
        factory.store.db().execute("DELETE FROM onboarding WHERE step = 'quit'", [])?;
        *lock(&factory.onboarding.quit) = OnboardingStep::Done;
    }
    Ok(())
}

impl Factory {
    /// Skips steps of the first run (SetOnboarding).
    pub fn set_onboarding(&self, request: &v1::SetOnboarding) -> Result<(), RequestError> {
        if request.skip_model && *lock(&self.onboarding.model) == OnboardingStep::Needed {
            self.store.db().execute("UPDATE onboarding SET state = 'skipped' WHERE step = 'model'", [])?;
            *lock(&self.onboarding.model) = OnboardingStep::Skipped;
            self.broadcast_workspace(&self.lock());
        }
        if request.skip_when_app_quits && *lock(&self.onboarding.quit) == OnboardingStep::Needed {
            self.store.db().execute("UPDATE onboarding SET state = 'skipped' WHERE step = 'quit'", [])?;
            *lock(&self.onboarding.quit) = OnboardingStep::Skipped;
            self.broadcast_workspace(&self.lock());
        }
        Ok(())
    }
}

impl Factory {
    /// Moves agent `id` to `model`, keeping its effort if the model has it;
    /// the default model too, when it can't run either.
    async fn move_to_model(&self, id: &str, model: &str) -> Result<(), RequestError> {
        let models = self.lock().models.clone();
        let efforts: Vec<String> =
            models.iter().find(|candidate| candidate.id == model).map(|found| found.efforts.clone()).unwrap_or_default();
        let fitting = crate::factory::fitting_effort(&models, model).to_owned();
        self.edit_config(|config| {
            if !models.iter().any(|candidate| candidate.id == config.default_model) {
                config.default_model = model.to_owned();
                if !efforts.contains(&config.default_effort) {
                    config.default_effort.clone_from(&fitting);
                }
            }
            let Some(agent) = config.agents.iter_mut().find(|agent| agent.id == id) else {
                return Err(RequestError::UnknownAgent(id.to_owned()));
            };
            agent.model = model.to_owned();
            if !efforts.contains(&agent.effort) {
                agent.effort.clone_from(&fitting);
            }
            Ok(())
        })
        .await
    }
}

/// Once a model is available, finishes Connect a model and has Rivet greet
/// the person; checked whenever the models available change.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    while let Some(strong) = factory.upgrade() {
        model_connected(&strong).await;
        if *lock(&strong.onboarding.model) == OnboardingStep::Done {
            return;
        }
        let changed = strong.onboarding.models_changed.clone();
        drop(strong);
        changed.notified().await;
    }
}

async fn model_connected(factory: &Arc<Factory>) {
    let (models, providers) = {
        let inner = factory.lock();
        (inner.models.clone(), inner.providers.clone())
    };
    if models.is_empty() || *lock(&factory.onboarding.model) == OnboardingStep::Done {
        return;
    }
    if let Err(error) = factory.store.db().execute("DELETE FROM onboarding WHERE step = 'model'", []) {
        tracing::error!(%error, "could not record that a model was connected");
        return;
    }
    *lock(&factory.onboarding.model) = OnboardingStep::Done;
    // Rivet greets the person, on a model that can run.
    let Some((rivet, model)) = ({
        let inner = factory.lock();
        crate::proposals::rivet(&inner.config).map(|rivet| (rivet.id.clone(), rivet.model.clone()))
    }) else {
        factory.broadcast_workspace(&factory.lock());
        return;
    };
    let runs = |id: &str| models.iter().any(|candidate| candidate.id == id);
    let moved = if runs(&model) {
        None
    } else {
        let chosen = PREFERRED.iter().copied().find(|id| runs(id)).unwrap_or(models[0].id.as_str()).to_owned();
        match factory.move_to_model(&rivet, &chosen).await {
            Ok(()) => models.iter().find(|candidate| candidate.id == chosen).map(|found| found.name.clone()),
            Err(error) => {
                tracing::error!(%error, model = %chosen, "could not move Rivet to a model that can run");
                None
            }
        }
    };
    factory.broadcast_workspace(&factory.lock());
    let (person, person_capitalized) = {
        let inner = factory.lock();
        (inner.config.person().to_owned(), inner.config.person_capitalized().to_owned())
    };
    let connected: Vec<String> =
        providers.iter().filter(|provider| !provider.signed_in_with.is_empty()).map(|provider| provider.signed_in_with.clone()).collect();
    let connected = if connected.is_empty() { "a model".to_owned() } else { connected.join(" and ") };
    let mut text = format!(
        "{person_capitalized} just set up Claktory and connected {connected}. Greet them in two sentences, and ask what they want their agents to do first."
    );
    if let Some(name) = moved {
        text.push_str(&format!(
            " You run on {name} now, since your model before couldn't run on what {person} connected; {person} can change it in your settings."
        ));
    }
    let summary = format!("{person_capitalized} connected a model");
    let event = host_event(summary);
    factory.deliver_event(&rivet, text, event);
}
