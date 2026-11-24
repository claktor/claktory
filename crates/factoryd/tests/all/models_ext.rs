//! The Models extension: an admin agent finds a model's exact id on every
//! signed-in provider (models_list), with each provider's state, and puts an
//! agent on it, with its effort (models_set), checked against what's
//! available; never after reading a pipe, or others' words in Claktory's
//! records.

use std::sync::Arc;

use factory_agent::auth::AuthInteraction;
use factory_agent::{Error, Model, StreamFn};
use factory_protocol::v1::{self, AgentStatus, client_message::Body as Request, entry::Kind, server_message::Body};
use factoryd::models::Models;
use futures_util::future::BoxFuture;

use crate::common::{
    Client, MODEL, Request as Seen, Scripted, View, apply_request, create_agent, home, send_message, start_with_models, temp_dir,
};

/// The scripted provider Rivet runs on, plus Anthropic (one of its two
/// subscriptions at its limit), OpenRouter (its only key out of credit) and
/// OpenAI, not signed in.
struct Catalog(Scripted);

const HOUR: i64 = 3_600_000;

fn model(id: &str, name: &str, efforts: &[&str], context_window: u64) -> v1::Model {
    let efforts = efforts.iter().map(|effort| (*effort).to_owned()).collect();
    v1::Model { id: id.into(), name: name.into(), context_window, efforts, image_input: false, added: false }
}

impl Models for Catalog {
    fn list(&self) -> BoxFuture<'_, Vec<v1::Model>> {
        Box::pin(async move {
            let mut models = self.0.list().await;
            models.extend([
                model("anthropic/claude-opus-4-8", "Claude Opus 4.8", &["low", "medium", "high", "max"], 1_000_000),
                model("anthropic/claude-sonnet-4-6", "Claude Sonnet 4.6", &["low", "medium", "high"], 200_000),
                model(
                    "openrouter/anthropic/claude-opus-4.8",
                    "Anthropic: Claude Opus 4.8",
                    &["minimal", "low", "medium", "high"],
                    1_000_000,
                ),
                model("openrouter/anthropic/claude-opus-4.8:batch", "Anthropic: Claude Opus 4.8 (batch)", &["low", "high"], 1_000_000),
                model("openrouter/openai/gpt-5.5", "OpenAI: GPT-5.5", &["low", "high"], 400_000),
            ]);
            models
        })
    }

    fn providers(&self) -> BoxFuture<'_, Vec<v1::Provider>> {
        Box::pin(async move {
            let now = i64::try_from(factory_agent::clock::now_ms()).expect("now");
            let account = |id: &str, label: &str| v1::Account { id: id.into(), label: label.into(), ..Default::default() };
            let mut providers = self.0.providers().await;
            providers.extend([
                v1::Provider {
                    id: "anthropic".into(),
                    name: "Anthropic".into(),
                    signed_in_with: "Anthropic (Claude Pro/Max)".into(),
                    accounts: vec![
                        v1::Account { exhausted_until: now + 2 * HOUR, ..account("1", "ada@example.test") },
                        // A subscription whose limits were read.
                        v1::Account {
                            plan: "Claude Max 20x".into(),
                            usage: Some(v1::ProviderUsage {
                                windows: vec![
                                    v1::UsageWindow {
                                        id: "five_hour".into(),
                                        label: "5-hour".into(),
                                        used_percent: 82.4,
                                        resets_at: now + 2 * HOUR,
                                    },
                                    v1::UsageWindow {
                                        id: "weekly".into(),
                                        label: "Weekly".into(),
                                        used_percent: 40.0,
                                        resets_at: now + 50 * HOUR,
                                    },
                                ],
                                updated: now,
                                ..Default::default()
                            }),
                            ..account("2", "bo@example.test")
                        },
                    ],
                    ..Default::default()
                },
                v1::Provider {
                    id: "openrouter".into(),
                    name: "OpenRouter".into(),
                    signed_in_with: "OpenRouter API key".into(),
                    accounts: vec![v1::Account { out_of_credit: true, ..account("1", "sk-…a1b2") }],
                    ..Default::default()
                },
                v1::Provider { id: "openai".into(), name: "OpenAI".into(), ..Default::default() },
            ]);
            providers
        })
    }

    fn resolve(&self, id: &str) -> Option<Model> {
        self.0.resolve(id)
    }

    fn stream(&self) -> StreamFn {
        self.0.stream()
    }

    fn login<'a>(
        &'a self,
        provider: &'a str,
        method: &'a str,
        interaction: &'a dyn AuthInteraction,
        add: bool,
    ) -> BoxFuture<'a, Result<(), Error>> {
        self.0.login(provider, method, interaction, add)
    }

    fn logout<'a>(&'a self, provider: &'a str, account: &'a str) -> BoxFuture<'a, Result<(), Error>> {
        self.0.logout(provider, account)
    }
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

/// A Claktory with Rivet on the scripted model, selected, and Lea.
async fn setup(dir: &std::path::Path) -> (Arc<factoryd::Factory>, Client, View) {
    let (factory, client, view, _) = setup_seen(dir).await;
    (factory, client, view)
}

/// As `setup`, with the requests the model was sent.
async fn setup_seen(dir: &std::path::Path) -> (Arc<factoryd::Factory>, Client, View, Arc<std::sync::Mutex<Vec<Seen>>>) {
    let scripted = Scripted::signed_in();
    let requests = scripted.requests.clone();
    let (addr, factory) = start_with_models(dir, Arc::new(Catalog(scripted))).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdateAgent(rivet)).await;
    apply_request(&mut client, &mut view, create_agent("", "Lea", "")).await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    (factory, client, view, requests)
}

/// Rivet calls `tool` with `args`; returns what it said.
async fn call(client: &mut Client, view: &mut View, tool: &str, args: serde_json::Value) -> String {
    call_as(client, view, "rivet", tool, args).await
}

/// `agent`, selected, calls `tool` with `args`; returns what it said.
async fn call_as(client: &mut Client, view: &mut View, agent: &str, tool: &str, args: serde_json::Value) -> String {
    let before = view.entries.len();
    apply_request(client, view, send_message(agent, &format!("call {tool} {args}"))).await;
    view.follow(client, |view| view.entries.len() >= before + 3 && idle(view, agent)).await;
    view.entries[before..]
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) if call.name == tool => Some(call.output.clone()),
            _ => None,
        })
        .expect("the call")
}

#[tokio::test]
async fn rivet_finds_a_models_id_on_each_provider_with_its_state() {
    let dir = temp_dir();
    let (_factory, mut client, mut view) = setup(&dir).await;

    // "Opus on OpenRouter": one call gives the exact ids, and why it can't run now.
    let found = call(&mut client, &mut view, "models_list", serde_json::json!({"provider": "OpenRouter", "query": "opus"})).await;
    assert!(found.starts_with("OpenRouter (openrouter): signed in, but can't take requests now: sk-…a1b2 is out of credit;"), "{found}");
    assert!(
        found.contains(
            "\n- openrouter/anthropic/claude-opus-4.8: Anthropic: Claude Opus 4.8; efforts minimal, low, medium, high; 1M context\n"
        ),
        "{found}"
    );
    assert!(found.contains("\n- openrouter/anthropic/claude-opus-4.8:batch: "), "{found}");
    assert!(!found.contains("gpt-5.5") && !found.contains("Anthropic (anthropic)"), "only OpenRouter's Opus: {found}");

    // Every provider: "-", "." and "_" alike, and the ones not signed in say so.
    let found = call(&mut client, &mut view, "models_list", serde_json::json!({"query": "Opus 4_8"})).await;
    assert!(
        found.contains("Anthropic (anthropic): signed in, and can take requests now (ada@example.test is at its usage limit until "),
        "{found}"
    );
    assert!(found.contains("\n- anthropic/claude-opus-4-8: Claude Opus 4.8; efforts low, medium, high, max; 1M context\n"), "{found}");
    assert!(found.contains("\n- openrouter/anthropic/claude-opus-4.8: "), "{found}");
    assert!(found.contains("OpenAI (openai): not signed in; only the person can sign in, in Settings → Providers."), "{found}");
    assert!(found.contains("Scripted (scripted): signed in, and can take requests now.\n- no model matches"), "{found}");
    assert!(!found.contains("sonnet"), "{found}");

    let wrong = call(&mut client, &mut view, "models_list", serde_json::json!({"provider": "Mistral"})).await;
    assert!(wrong.contains("There's no provider \"Mistral\". Providers: scripted (Scripted), anthropic (Anthropic),"), "{wrong}");

    // Rivet has it without a word in the settings file; Lea doesn't.
    assert!(view.agent("rivet").extensions.iter().any(|extension| extension.id == "models" && extension.enabled));
    assert!(!view.agent("lea").extensions.iter().any(|extension| extension.id == "models" && extension.enabled));
    let config = std::fs::read_to_string(home(&dir).config_path()).expect("config");
    assert!(!config.contains("\"models\""), "{config}");
}

#[tokio::test]
async fn rivet_puts_an_agent_on_a_model_and_effort_only_from_whats_available() {
    let dir = temp_dir();
    let (factory, mut client, mut view) = setup(&dir).await;

    // Lea's effort isn't one the model takes: she gets "high", and the note
    // says her provider can't take requests now.
    let set = serde_json::json!({"agent": "Lea", "model": "openrouter/anthropic/claude-opus-4.8"});
    let done = call(&mut client, &mut view, "models_set", set).await;
    assert!(
        done.starts_with("Lea (lea) now runs on openrouter/anthropic/claude-opus-4.8 (Anthropic: Claude Opus 4.8) at effort high, from its next step. A new model writes its whole prompt cache again"),
        "{done}"
    );
    assert!(done.ends_with("Note: OpenRouter can't take requests now: sk-…a1b2 is out of credit, so Lea waits until it can."), "{done}");
    view.follow(&mut client, |view| view.agent("lea").model == "openrouter/anthropic/claude-opus-4.8").await;
    assert_eq!(view.agent("lea").effort, "high");

    // An effort alone, saying what it costs the cache.
    let done = call(&mut client, &mut view, "models_set", serde_json::json!({"agent": "lea", "effort": "low"})).await;
    assert!(done.contains("at effort low, from its next step. What an effort change costs its prompt cache"), "{done}");
    view.follow(&mut client, |view| view.agent("lea").effort == "low").await;

    // Refused, changing nothing: an effort the model doesn't take, a model
    // that isn't there (with the close ones), a provider not signed in,
    // nothing to change, an agent that isn't there.
    let refused = call(&mut client, &mut view, "models_set", serde_json::json!({"agent": "lea", "effort": "max"})).await;
    assert_eq!(
        refused,
        "openrouter/anthropic/claude-opus-4.8 takes the efforts minimal, low, medium, high, not \"max\". Nothing was changed."
    );
    let refused =
        call(&mut client, &mut view, "models_set", serde_json::json!({"agent": "lea", "model": "openrouter/claude-opus-4-8"})).await;
    assert_eq!(
        refused,
        "No model \"openrouter/claude-opus-4-8\" is available now, so nothing was changed. Close: openrouter/anthropic/claude-opus-4.8, openrouter/anthropic/claude-opus-4.8:batch."
    );
    let refused =
        call(&mut client, &mut view, "models_set", serde_json::json!({"agent": "lea", "model": "anthropic/claude-opus-48"})).await;
    assert_eq!(
        refused,
        "No model \"anthropic/claude-opus-48\" is available now, so nothing was changed. Close: anthropic/claude-opus-4-8, openrouter/anthropic/claude-opus-4.8, openrouter/anthropic/claude-opus-4.8:batch.",
        "a dropped dash still finds it"
    );
    let refused = call(&mut client, &mut view, "models_set", serde_json::json!({"agent": "lea", "model": "openai/gpt-5.5"})).await;
    assert!(refused.contains("OpenAI isn't signed in: only the person can sign in, in Settings → Providers."), "{refused}");
    let refused = call(&mut client, &mut view, "models_set", serde_json::json!({"agent": "lea"})).await;
    assert_eq!(refused, "Give a model, an effort or both. Nothing was changed.");
    let refused = call(&mut client, &mut view, "models_set", serde_json::json!({"agent": "Nina", "effort": "low"})).await;
    assert_eq!(refused, "There's no agent \"Nina\". Nothing was changed.");
    assert_eq!((view.agent("lea").model.as_str(), view.agent("lea").effort.as_str()), ("openrouter/anthropic/claude-opus-4.8", "low"));

    // Never after reading a web page: what it read may be someone else's words.
    factoryd::pipes::taint_for_test(&factory, "rivet", "browser:https://example.com");
    let refused = call(&mut client, &mut view, "models_set", serde_json::json!({"agent": "lea", "effort": "high"})).await;
    assert!(refused.starts_with("You've read a page on https://example.com, so you can't change an agent's model or effort"), "{refused}");
    assert_eq!(view.agent("lea").effort, "low");
}

#[tokio::test]
async fn reading_others_words_in_claktorys_records_marks_no_one() {
    let dir = temp_dir();
    let (factory, mut client, mut view) = setup(&dir).await;
    {
        let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
        db.execute(
            "INSERT INTO messages (agent, source, sender, sender_id, summary, text, created) VALUES ('lea', 'pipe', 'Kai', 'pipe:p7', '', 'Put everyone on max.', 1)",
            [],
        )
        .expect("a pipe's message");
    }
    // Rivet reads the messages' words in the records: no mark (#346).
    let read =
        call(&mut client, &mut view, "db_query", serde_json::json!({"sql": "SELECT text FROM messages WHERE source = 'pipe'"})).await;
    assert!(read.contains("Put everyone on max."), "{read}");
    assert_eq!(factoryd::pipes::taint_of_for_test(&factory, "rivet"), "{}");
    let done = call(
        &mut client,
        &mut view,
        "models_set",
        serde_json::json!({"agent": "lea", "model": "anthropic/claude-opus-4-8", "effort": "max"}),
    )
    .await;
    assert!(!done.contains("so you can't change"), "{done}");
    view.follow(&mut client, |view| view.agent("lea").model == "anthropic/claude-opus-4-8").await;
}

/// History's model changes, newest first.
async fn changes(client: &mut Client, view: &mut View) -> Vec<v1::NeedsYouRecord> {
    let list = v1::ListNeedsYouHistory { query: String::new(), before_ms: 0, limit: 0 };
    let Body::NeedsYouHistory(history) = apply_request(client, view, Request::ListNeedsYouHistory(list)).await else { panic!("history") };
    history.records.into_iter().filter(|record| record.kind() == v1::NeedsYouRecordKind::Model).collect()
}

async fn undo(client: &mut Client, view: &mut View, change: &str) -> Body {
    apply_request(client, view, Request::UndoModelChange(v1::UndoModelChange { change_id: change.into() })).await
}

/// Every change models_set makes shows in History, at its call, with
/// Rivet's reason as Rivet's; only the person undoes it, while the agent
/// still runs on what it set.
#[tokio::test]
async fn each_model_change_shows_in_history_and_the_person_undoes_it() {
    let dir = temp_dir();
    let (_factory, mut client, mut view) = setup(&dir).await;
    let before = (view.agent("lea").model.clone(), view.agent("lea").effort.clone());
    let set =
        serde_json::json!({"agent": "Lea", "model": "anthropic/claude-opus-4-8", "effort": "high", "reason": "Matt wants Lea on Opus."});
    let done = call(&mut client, &mut view, "models_set", set).await;
    assert!(done.contains("It shows in the person's History, where they can undo it."), "{done}");
    view.follow(&mut client, |view| view.agent("lea").effort == "high").await;
    let call_seq = view
        .entries
        .iter()
        .rev()
        .find(|entry| matches!(&entry.kind, Some(Kind::ToolCall(call)) if call.name == "models_set"))
        .expect("the call")
        .seq;
    let records = changes(&mut client, &mut view).await;
    let record = records.first().expect("a record");
    assert_eq!(record.title, "Rivet put Lea on Claude Opus 4.8 at high effort");
    assert_eq!(record.outcome(), v1::NeedsYouOutcome::Changed);
    assert_eq!(
        (record.agent_id.as_str(), record.subject_agent_id.as_str(), record.agent_reason.as_str(), record.seq, record.undone),
        ("rivet", "lea", "Matt wants Lea on Opus.", call_seq, false)
    );
    let first = record.model_change_id.clone();
    assert!(!first.is_empty() && !record.superseded);

    // The effort alone; and nothing to change is no record.
    call(&mut client, &mut view, "models_set", serde_json::json!({"agent": "lea", "effort": "low"})).await;
    view.follow(&mut client, |view| view.agent("lea").effort == "low").await;
    call(&mut client, &mut view, "models_set", serde_json::json!({"agent": "lea", "effort": "low"})).await;
    let records = changes(&mut client, &mut view).await;
    assert_eq!(records.len(), 2);
    assert_eq!((records[0].title.as_str(), records[0].agent_reason.as_str()), ("Rivet changed Lea's effort to low", ""));
    // The first is changed since: Undo would refuse it (Abbacchio's row 684).
    assert_eq!((records[0].superseded, records[1].superseded), (false, true));

    // Undone: back to high; once only.
    let second = records[0].model_change_id.clone();
    assert!(matches!(undo(&mut client, &mut view, &second).await, Body::Ack(_)));
    view.follow(&mut client, |view| view.agent("lea").effort == "high").await;
    let records = changes(&mut client, &mut view).await;
    // Undone isn't changed since; the first is back as it set it.
    assert_eq!((records[0].undone, records[0].superseded, records[1].superseded), (true, false, false));
    assert!(matches!(undo(&mut client, &mut view, &second).await, Body::Error(error) if error.message == "This was undone already."));
    assert!(matches!(undo(&mut client, &mut view, "nope").await, Body::Error(error) if error.message == "This change can't be found."));

    // Changed since by the person: left as it is.
    let medium = v1::UpdateAgent { agent_id: "lea".into(), effort: Some("medium".into()), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdateAgent(medium)).await;
    assert!(
        matches!(undo(&mut client, &mut view, &first).await, Body::Error(error) if error.message == "Lea's model changed since, so it was left as it is.")
    );
    assert_eq!(view.agent("lea").effort, "medium");
    assert!(changes(&mut client, &mut view).await[1].superseded);
    // Back as it set it: undone to what Lea had before.
    let high = v1::UpdateAgent { agent_id: "lea".into(), effort: Some("high".into()), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdateAgent(high)).await;
    assert!(matches!(undo(&mut client, &mut view, &first).await, Body::Ack(_)));
    view.follow(&mut client, |view| (view.agent("lea").model.clone(), view.agent("lea").effort.clone()) == before).await;
}

fn models_option(agent: &str, change: v1::ExtensionChange) -> Request {
    Request::UpdateAgent(v1::UpdateAgent { agent_id: agent.into(), extension: Some(change), ..Default::default() })
}

fn can_set(agent: &str, on: bool) -> Request {
    let value = v1::OptionValue { on, ..Default::default() };
    models_option(agent, v1::ExtensionChange { id: "models".into(), option_id: "can_set".into(), value: Some(value), ..Default::default() })
}

/// Friend bug #8 (A3): models_list says how much of each subscription's
/// limits is used and when each resets, and Can change models (on unless
/// turned off) lets an agent have the list without models_set.
#[tokio::test]
async fn any_agent_can_read_the_limits_without_changing_models() {
    let dir = temp_dir();
    let (factory, mut client, mut view, requests) = setup_seen(&dir).await;

    let found = call(&mut client, &mut view, "models_list", serde_json::json!({"provider": "anthropic"})).await;
    let usage = found.lines().find(|line| line.starts_with("- Usage of ")).expect("a usage line");
    assert!(usage.starts_with("- Usage of bo@example.test (Claude Max 20x): 5-hour 82% (resets "), "{found}");
    assert!(usage.contains("), Weekly 40% (resets ") && usage.contains("; read "), "{usage}");
    assert_eq!(found.matches("- Usage of ").count(), 1, "an account with no limits read has none: {found}");
    assert!(found.contains("\n- anthropic/claude-opus-4-8: "), "the models still follow: {found}");

    // Lea gets the list only: no models_set, and her prompt says so.
    apply_request(
        &mut client,
        &mut view,
        models_option("lea", v1::ExtensionChange { id: "models".into(), enabled: Some(true), ..Default::default() }),
    )
    .await;
    apply_request(&mut client, &mut view, can_set("lea", false)).await;
    let option = |view: &View, agent: &str| {
        let models = view.agent(agent).extensions.iter().find(|extension| extension.id == "models").cloned().expect("models");
        (models.enabled, models.options.get("can_set").map(|value| value.on))
    };
    assert_eq!(option(&view, "lea"), (true, Some(false)));
    assert_eq!(option(&view, "rivet"), (true, Some(true)), "on unless turned off");
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "lea".into() })).await;
    let found = call_as(&mut client, &mut view, "lea", "models_list", serde_json::json!({"provider": "anthropic"})).await;
    assert!(found.contains("- Usage of bo@example.test (Claude Max 20x): 5-hour 82%"), "{found}");
    let seen = requests.lock().expect("requests").iter().rev().find(|request| request.prompt.contains("models_list")).cloned();
    let seen = seen.expect("Lea's request");
    assert!(seen.tools.iter().any(|tool| tool == "models_list") && !seen.tools.iter().any(|tool| tool == "models_set"), "{:?}", seen.tools);
    assert!(seen.system_prompt.contains("You can't change an agent's model: the person does that in the agent's settings."));
    assert!(!seen.system_prompt.contains("then set it with models_set"));

    // Turned off for Rivet after its tools were written: models_set is
    // refused all the same (the frame's tool is gone from its next step).
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    apply_request(&mut client, &mut view, can_set("rivet", false)).await;
    let refused = call(&mut client, &mut view, "models_set", serde_json::json!({"agent": "lea", "effort": "low"})).await;
    assert_eq!(refused, "models_set isn't available with your current settings.");
    let effort = view.agent("lea").effort.clone();
    assert_ne!(effort, "low");

    // After reading a web page, whose accounts they are and their usage are
    // hidden: a stranger's page could ask for them (Fugo).
    factoryd::pipes::taint_for_test(&factory, "rivet", "browser:https://example.com");
    let found = call(&mut client, &mut view, "models_list", serde_json::json!({"provider": "anthropic"})).await;
    assert!(found.contains("\n- Usage hidden: you've read a page on https://example.com.\n"), "{found}");
    assert!(found.contains("can take requests now (an account is at its usage limit until "), "{found}");
    assert!(!found.contains("Usage of") && !found.contains("@example.test") && !found.contains("Max 20x"), "{found}");
}
