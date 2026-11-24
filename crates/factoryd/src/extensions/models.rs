//! The Models extension: what an admin agent, such as Rivet, needs to put an
//! agent on a model in one step ("set Rivet to Opus on OpenRouter").
//! `models_list` lists the models each signed-in provider can run now, the
//! same list as the app's model picker (`Inner::models`), with each
//! provider's state: signed in or not, and whether its accounts can take
//! requests now. `models_set` changes an agent's model and effort as the
//! app's agent settings do (`Factory::update_agent`), only to a model and an
//! effort that list has.
//!
//! An agent that has read a stranger's words can't change models: what it
//! read may be someone else's instructions, and a model is where an agent's
//! work goes and what it costs. Since #346 that's only a web page it read
//! through browser control (`Pipes::tainted`); pipes and direct messages
//! are friends' words and mark no one.

use std::fmt::Write as _;
use std::sync::Arc;

use factory_agent::{Tool, ToolResult};
use factory_protocol::v1;
use serde::Deserialize;
use serde_json::json;

use super::{Extension, Manifest, OptionKind, OptionSpec, PromptSection, async_tool};
use crate::config::{AgentConfig, Config};
use crate::factory::{AgentChange, Factory, Inner};

pub(crate) const ID: &str = "models";
/// The option that gives it models_set; off, it only lists (friend bug #8).
pub(crate) const CAN_SET: &str = "can_set";

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Models",
    icon: "sparkles",
    description: "Lets the agent see which models your signed-in providers can run now, and put an agent on one of them, with its effort.",
    default_enabled: false,
    options: &[OptionSpec {
        id: CAN_SET,
        label: "Can change models",
        description: "It can put agents, itself included, on another model or effort. Off, it only sees the models and how much of your subscriptions' limits is used.",
        kind: OptionKind::Switch,
        default_on: true,
        includes_project: false,
    }],
};

const LIST: &str = "The models agents can run on now, by provider, each with its exact id (what an agent's model setting takes), its name, its efforts and its context size, and each provider's state: signed in or not, and whether it can take requests now (an account at its usage limit, out of credit or needing a new sign-in can't), and each subscription's limits: how much of each is used and when it resets. query: words that must all be in the id or the name, such as \"opus\" or \"sonnet 4.6\" (\"-\", \".\" and \"_\" alike). provider: one provider, by id or name, such as \"openrouter\".";
const SET: &str = "Put an agent on a model, an effort, or both, from its next step; Claktory tells it. agent: its id or name (you included). model: an id exactly as models_list gives it. effort: one the model lists; left out, the agent keeps its effort if the model takes it, else gets \"high\" or the model's strongest. reason: why, in a sentence for the person, who sees every change in their History and can undo it. Refused for a model that isn't available now, saying which are close.";
/// The longest reason kept.
const MOST_REASON: usize = 500;

/// The most models one list shows.
const MOST: usize = 60;

pub(crate) struct Models;

impl Extension for Models {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn prompt(&self, _factory: &Factory, inner: &Inner, agent: &AgentConfig) -> Vec<PromptSection> {
        let person = inner.config.person();
        if !super::switch(agent, ID, CAN_SET) {
            let text = format!(
                "## Models\n\nmodels_list shows the models agents can run on now, by provider, and how much of each of {person}'s subscriptions' limits is used, with when each resets. You can't change an agent's model: {person} does that in the agent's settings.\n"
            );
            return vec![PromptSection::static_text(text)];
        }
        let text = format!(
            "## Models\n\nTo put an agent on a model, find its exact id with models_list (a few words of its name in query, such as \"opus\", and the provider if {person} named one), then set it with models_set; don't edit the settings file for it, or guess an id. An id is \"<provider>/<the provider's own id>\", so one model reads differently on each provider: anthropic/claude-opus-4-8 on Anthropic, openrouter/anthropic/claude-opus-4.8 on OpenRouter. Only {person} signs in to a provider, in Settings → Providers. Changing a model costs a full write of the agent's prompt cache on its next step, so change models only when asked.\n"
        );
        vec![PromptSection::static_text(text)]
    }

    fn tools(&self, factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
        let list_parameters = json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "provider": {"type": "string"},
            },
            "additionalProperties": false,
        });
        let set_parameters = json!({
            "type": "object",
            "required": ["agent"],
            "properties": {
                "agent": {"type": "string", "minLength": 1},
                "model": {"type": "string"},
                "effort": {"type": "string"},
                "reason": {"type": "string", "maxLength": MOST_REASON},
            },
            "additionalProperties": false,
        });
        let list = super::tool("models_list", "List models", LIST, list_parameters, super::typed_handler(factory, agent, list));
        if !can_set(factory, agent) {
            return vec![list];
        }
        vec![list, async_tool(factory, agent, "models_set", "Change an agent's model", SET, set_parameters, set)]
    }

    fn admin_default(&self) -> bool {
        true
    }
}

#[derive(Deserialize)]
struct ListArgs {
    #[serde(default)]
    query: String,
    #[serde(default)]
    provider: String,
}

fn list(factory: &Arc<Factory>, agent: &str, args: ListArgs) -> Result<ToolResult, String> {
    // An agent that has read a web page doesn't see whose accounts these
    // are or their usage: a stranger's page could ask for them (Fugo).
    let hidden = (!factory.pipes.tainted(agent).is_empty()).then(|| crate::pipes::agents::what_was_read(factory, agent));
    let inner = factory.lock();
    super::caller(&inner.config, agent)?;
    let person = inner.config.person();
    let providers = chosen(&inner.providers, &args.provider)?;
    let words = words(&args.query);
    let now = factory_agent::clock::now_ms();
    let mut text = String::new();
    let mut shown = 0;
    for provider in providers {
        let _ = writeln!(text, "{} ({}): {}", provider.name, provider.id, state(provider, person, now, hidden.is_some()));
        if !signed_in(provider) {
            continue;
        }
        let mut usage_lines = Vec::new();
        for account in &provider.accounts {
            let key = (provider.id.clone(), account.id.clone());
            let usage = inner.usage.get(&key).or(account.usage.as_ref());
            let plan = inner.plans.get(&key).unwrap_or(&account.plan);
            usage_lines.extend(usage.and_then(|usage| limits(account, plan, usage)));
        }
        match &hidden {
            Some(read) if !usage_lines.is_empty() => {
                let _ = writeln!(text, "- Usage hidden: you've read {read}.");
            }
            _ => usage_lines.iter().for_each(|line| {
                let _ = writeln!(text, "- {line}");
            }),
        }
        let mut models: Vec<&v1::Model> =
            inner.models.iter().filter(|model| provider_of(&model.id) == provider.id && matches(&words, model)).collect();
        models.sort_by(|a, b| a.id.cmp(&b.id));
        if models.is_empty() {
            let _ = writeln!(text, "- {}", if words.is_empty() { "no models" } else { "no model matches" });
        }
        let room = MOST.saturating_sub(shown);
        for model in models.iter().take(room) {
            let _ = writeln!(text, "- {}", line(model));
        }
        shown += models.len().min(room);
        if models.len() > room {
            let _ = writeln!(text, "- … and {} more: narrow it with query", models.len() - room);
        }
    }
    if text.is_empty() {
        text = "No provider is set up in this Claktory.".to_owned();
    }
    Ok(ToolResult::text(text.trim_end().to_owned(), json!({})))
}

#[derive(Deserialize)]
struct SetArgs {
    agent: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    effort: String,
    #[serde(default)]
    reason: String,
}

async fn set(factory: Arc<Factory>, caller: String, args: SetArgs) -> Result<ToolResult, String> {
    super::untainted(
        &factory,
        &caller,
        "change an agent's model or effort",
        "change it in the agent's settings. Nothing was changed.",
    )?;
    let (model, effort) = (args.model.trim(), args.effort.trim());
    if model.is_empty() && effort.is_empty() {
        return Err("Give a model, an effort or both. Nothing was changed.".to_owned());
    }
    let reason = args.reason.trim();
    if reason.chars().count() > MOST_REASON {
        return Err(format!("Say why in {MOST_REASON} characters at most. Nothing was changed."));
    }
    // A model its provider's list doesn't have is checked with the provider
    // and added first (friend bug #14); refused, the provider's word comes
    // before the ids that come close.
    let listed = model.is_empty() || factory.lock().models.iter().any(|known| known.id == model);
    let provider_said = match listed {
        true => None,
        false => {
            crate::catalog::ensure(&factory, model, crate::catalog::Check::WhenItCan).await.err().filter(|why| !why.starts_with("Sign in"))
        }
    };
    let (id, name, before, model, effort, warning) = {
        let inner = factory.lock();
        super::caller(&inner.config, &caller)?;
        let agent = agent(&inner.config, &args.agent)?;
        let model = available(&inner, if model.is_empty() { &agent.model } else { model })
            .map_err(|why| provider_said.as_ref().map_or_else(|| why.clone(), |said| format!("{said} {why}")))?;
        let effort = if !effort.is_empty() {
            if !model.efforts.iter().any(|known| known == effort) {
                return Err(match model.efforts.is_empty() {
                    true => format!("{} has no effort to choose. Nothing was changed.", model.id),
                    false => format!("{} takes the efforts {}, not \"{effort}\". Nothing was changed.", model.id, model.efforts.join(", ")),
                });
            }
            Some(effort.to_owned())
        } else if model.efforts.is_empty() || model.efforts.contains(&agent.effort) {
            None
        } else {
            Some(crate::factory::fitting_effort(&inner.models, &model.id).to_owned())
        };
        let provider = inner.providers.iter().find(|provider| provider.id == provider_of(&model.id));
        let now = factory_agent::clock::now_ms();
        let warning = provider.and_then(|provider| busy(provider, now, false).map(|why| format!("{} {why}", provider.name)));
        let before = (agent.model.clone(), agent.effort.clone());
        (agent.id.clone(), agent.name.clone(), before, model.clone(), effort, warning)
    };
    let change = AgentChange { model: Some(&model.id), effort: effort.as_deref(), ..Default::default() };
    factory.update_agent(&id, change).await.map_err(|error| format!("{error} Nothing was changed."))?;
    let effort = effort.unwrap_or(before.1.clone());
    let made = Made { agent: &id, by: &caller, reason, before: (&before.0, &before.1), after: (&model.id, &effort), name: &model.name };
    let change =
        if (model.id.as_str(), effort.as_str()) == (before.0.as_str(), before.1.as_str()) { None } else { record(&factory, &made) };
    let mut text = if model.efforts.is_empty() {
        format!("{name} ({id}) now runs on {} ({}), from its next step.", model.id, model.name)
    } else {
        format!("{name} ({id}) now runs on {} ({}) at effort {effort}, from its next step.", model.id, model.name)
    };
    text.push(' ');
    text.push_str(if model.id != before.0 {
        "A new model writes its whole prompt cache again on that step, once."
    } else if effort != before.1 {
        "What an effort change costs its prompt cache, once, on its next step: on Anthropic models with adaptive thinking (Sonnet 4.6, Opus 4.6 and later) a full write, system prompt and tools included, and going back to the effort before reads that effort's cache again while it lasts; on Anthropic models with a thinking budget (Sonnet 4.5, Haiku 4.5, Opus 4.5) a write of the conversation only. OpenAI and Codex document their caches as not depending on the effort."
    } else {
        "That's what it ran on already."
    });
    if change.is_some() {
        let person = factory.lock().config.person().to_owned();
        let _ = write!(text, " It shows in {person}'s History, where they can undo it.");
    }
    if let Some(warning) = warning {
        let _ = write!(text, " Note: {warning}, so {name} waits until it can.");
    }
    Ok(ToolResult::text(text, json!({"model_change": change.map(|id| id.to_string())})))
}

/// The table of models_set's changes.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS model_changes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- The agent whose model or effort changed, and the agent that changed it.
    agent TEXT NOT NULL,
    by TEXT NOT NULL,
    -- Why, in the changing agent's words.
    reason TEXT NOT NULL DEFAULT '',
    model_before TEXT NOT NULL,
    effort_before TEXT NOT NULL,
    model_after TEXT NOT NULL,
    effort_after TEXT NOT NULL,
    -- The model's name when it was set (\"Claude Opus 5\").
    name_after TEXT NOT NULL DEFAULT '',
    at INTEGER NOT NULL,
    -- When the person undid it.
    undone INTEGER
);
CREATE INDEX IF NOT EXISTS model_changes_at ON model_changes (at);
";

/// A change models_set made, to keep.
struct Made<'a> {
    agent: &'a str,
    by: &'a str,
    reason: &'a str,
    before: (&'a str, &'a str),
    after: (&'a str, &'a str),
    name: &'a str,
}

/// Keeps `made`: its id; None when it couldn't be kept (logged).
fn record(factory: &Factory, made: &Made<'_>) -> Option<i64> {
    let db = factory.store.db();
    let kept = db.execute(
        "INSERT INTO model_changes (agent, by, reason, model_before, effort_before, model_after, effort_after, name_after, at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            made.agent,
            made.by,
            made.reason,
            made.before.0,
            made.before.1,
            made.after.0,
            made.after.1,
            made.name,
            factory_agent::clock::now_ms() as i64
        ],
    );
    match kept {
        Ok(_) => Some(db.last_insert_rowid()),
        Err(error) => {
            tracing::error!(%error, "couldn't keep a model change for the history");
            None
        }
    }
}

/// A change models_set made, as kept.
#[derive(Clone, Debug)]
pub(crate) struct ModelChange {
    pub id: i64,
    pub agent: String,
    pub by: String,
    pub reason: String,
    pub model_before: String,
    pub effort_before: String,
    pub model_after: String,
    pub effort_after: String,
    pub name_after: String,
    pub at: i64,
    pub undone: Option<i64>,
}

const COLUMNS: &str = "id, agent, by, reason, model_before, effort_before, model_after, effort_after, name_after, at, undone";

impl ModelChange {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<ModelChange> {
        Ok(ModelChange {
            id: row.get(0)?,
            agent: row.get(1)?,
            by: row.get(2)?,
            reason: row.get(3)?,
            model_before: row.get(4)?,
            effort_before: row.get(5)?,
            model_after: row.get(6)?,
            effort_after: row.get(7)?,
            name_after: row.get(8)?,
            at: row.get(9)?,
            undone: row.get(10)?,
        })
    }

    /// Claktory's sentence for it, with each agent's name now (`names`):
    /// "Rivet put Ingrid on Claude Opus 5 at high effort", "Rivet changed Ingrid's
    /// effort to high".
    pub(crate) fn title(&self, names: &std::collections::HashMap<String, String>) -> String {
        let name = |id: &str| names.get(id).cloned().unwrap_or_else(|| id.to_owned());
        let (by, agent) = (name(&self.by), name(&self.agent));
        let model = if self.name_after.is_empty() { self.model_after.as_str() } else { self.name_after.as_str() };
        if self.model_after == self.model_before {
            return format!("{by} changed {} effort to {}", crate::apps::changes::possessive(&agent), self.effort_after);
        }
        match self.effort_after.as_str() {
            "" => format!("{by} put {agent} on {model}"),
            effort => format!("{by} put {agent} on {model} at {effort} effort"),
        }
    }
}

/// The changes made from `since` to before `before`, newest first, at most
/// `limit`.
pub(crate) fn between(db: &rusqlite::Connection, since: i64, before: i64, limit: i64) -> rusqlite::Result<Vec<ModelChange>> {
    let mut statement =
        db.prepare(&format!("SELECT {COLUMNS} FROM model_changes WHERE at >= ?1 AND at < ?2 ORDER BY at DESC, id DESC LIMIT ?3"))?;
    statement.query_map(rusqlite::params![since, before, limit], ModelChange::read)?.collect()
}

/// The change a models_set call made, from its entry.
pub(crate) fn change_of(call: &v1::ToolCall) -> Option<String> {
    if call.name != "models_set" && call.name != "extension_call" {
        return None;
    }
    let details: serde_json::Value = serde_json::from_str(&call.details).ok()?;
    details.get("model_change")?.as_str().map(str::to_owned)
}

/// UndoModelChange: the person puts back the model and effort a change
/// replaced, while the agent still runs on what it set.
pub(crate) async fn undo(factory: &Arc<Factory>, request: &v1::UndoModelChange) -> Result<(), crate::factory::RequestError> {
    use crate::factory::RequestError;
    let refused = |text: String| RequestError::Usage(text);
    let gone = || refused("This change can't be found.".to_owned());
    let id: i64 = request.change_id.trim().parse().map_err(|_| gone())?;
    let change = factory
        .store
        .db()
        .query_row(&format!("SELECT {COLUMNS} FROM model_changes WHERE id = ?1"), [id], ModelChange::read)
        .map_err(|_| gone())?;
    if change.undone.is_some() {
        return Err(refused("This was undone already.".to_owned()));
    }
    let now = {
        let inner = factory.lock();
        inner.config.agent(&change.agent).map(|agent| (agent.name.clone(), agent.model.clone(), agent.effort.clone()))
    };
    let Some((name, model, effort)) = now else {
        return Err(refused("That agent was deleted, so there's nothing to put back.".to_owned()));
    };
    if (model.as_str(), effort.as_str()) != (change.model_after.as_str(), change.effort_after.as_str()) {
        return Err(refused(format!("{} model changed since, so it was left as it is.", crate::apps::changes::possessive(&name))));
    }
    let back = AgentChange { model: Some(&change.model_before), effort: Some(&change.effort_before), ..Default::default() };
    factory.update_agent(&change.agent, back).await?;
    factory.store.db().execute(
        "UPDATE model_changes SET undone = ?2 WHERE id = ?1",
        rusqlite::params![change.id, factory_agent::clock::now_ms() as i64],
    )?;
    Ok(())
}

/// The agent may change models (Can change models, on unless turned off).
fn can_set(factory: &Factory, agent: &str) -> bool {
    factory.lock().config.agent(agent).is_some_and(|config| super::switch(config, ID, CAN_SET))
}

/// The agent named by id, or by a name only one agent has.
pub(super) fn agent<'a>(config: &'a Config, named: &str) -> Result<&'a AgentConfig, String> {
    let named = named.trim();
    if let Some(agent) = config.agent(named) {
        return Ok(agent);
    }
    let same: Vec<&AgentConfig> = config.agents.iter().filter(|agent| agent.name.eq_ignore_ascii_case(named)).collect();
    match same.as_slice() {
        [agent] => Ok(agent),
        [] => Err(format!("There's no agent \"{named}\". Nothing was changed.")),
        several => {
            let ids: Vec<&str> = several.iter().map(|agent| agent.id.as_str()).collect();
            Err(format!("Several agents are called {named}: {}. Give its id. Nothing was changed.", ids.join(", ")))
        }
    }
}

/// The model `id` if it's available now; else why not, and close ones.
fn available<'a>(inner: &'a Inner, id: &str) -> Result<&'a v1::Model, String> {
    if let Some(model) = inner.models.iter().find(|model| model.id == id) {
        return Ok(model);
    }
    let mut why = format!("No model \"{id}\" is available now, so nothing was changed.");
    let provider = inner.providers.iter().find(|provider| provider.id == provider_of(id));
    if let Some(provider) = provider.filter(|provider| !signed_in(provider)) {
        let person = inner.config.person();
        let _ = write!(why, " {} isn't signed in: only {person} can sign in, in Settings → Providers.", provider.name);
    }
    let words = words(id);
    let mut close: Vec<&str> = inner.models.iter().filter(|model| matches(&words, model)).map(|model| model.id.as_str()).collect();
    if close.is_empty() {
        // A dropped "-" or "." (claude-opus-48): letters and digits alone.
        let words: Vec<String> = words.iter().map(|word| compact(word)).filter(|word| !word.is_empty()).collect();
        close = inner
            .models
            .iter()
            .filter(|model| !words.is_empty() && words.iter().all(|word| compact(&model.id).contains(word.as_str())))
            .map(|model| model.id.as_str())
            .collect();
    }
    close.sort_unstable();
    match close.len() {
        0 => why.push_str(" Find its id with models_list."),
        n if n <= 10 => {
            let _ = write!(why, " Close: {}.", close.join(", "));
        }
        n => {
            let _ = write!(why, " {n} are close, such as {}; narrow it with models_list.", close[..10].join(", "));
        }
    }
    Err(why)
}

/// The provider `id` or `name` names, or every provider.
fn chosen<'a>(providers: &'a [v1::Provider], named: &str) -> Result<Vec<&'a v1::Provider>, String> {
    let named = named.trim();
    if named.is_empty() {
        return Ok(providers.iter().collect());
    }
    let found: Vec<&v1::Provider> =
        providers.iter().filter(|provider| provider.id.eq_ignore_ascii_case(named) || provider.name.eq_ignore_ascii_case(named)).collect();
    if found.is_empty() {
        let all: Vec<String> = providers.iter().map(|provider| format!("{} ({})", provider.id, provider.name)).collect();
        return Err(format!("There's no provider \"{named}\". Providers: {}.", all.join(", ")));
    }
    Ok(found)
}

fn signed_in(provider: &v1::Provider) -> bool {
    !provider.signed_in_with.is_empty() || !provider.accounts.is_empty()
}

/// A provider's state, as models_list says it.
/// `hide`: accounts named only as "an account".
fn state(provider: &v1::Provider, person: &str, now: u64, hide: bool) -> String {
    if !signed_in(provider) {
        return format!("not signed in; only {person} can sign in, in Settings → Providers.");
    }
    match busy(provider, now, hide) {
        Some(why) => format!("signed in, but {why}; agents on its models wait until it can."),
        None => {
            let held: Vec<String> = provider
                .accounts
                .iter()
                .filter_map(|account| held(account, now).map(|why| format!("{} {why}", label(account, hide))))
                .collect();
            if held.is_empty() {
                "signed in, and can take requests now.".to_owned()
            } else {
                format!("signed in, and can take requests now ({}).", held.join("; "))
            }
        }
    }
}

/// Why none of a signed-in provider's accounts can take requests now; None
/// when one can.
fn busy(provider: &v1::Provider, now: u64, hide: bool) -> Option<String> {
    if provider.accounts.is_empty() {
        return None;
    }
    let mut why = Vec::new();
    for account in &provider.accounts {
        why.push(format!("{} {}", label(account, hide), held(account, now)?));
    }
    Some(format!("can't take requests now: {}", why.join("; ")))
}

/// Why an account can't take requests now; None when it can.
fn held(account: &v1::Account, now: u64) -> Option<String> {
    let until = u64::try_from(account.exhausted_until).unwrap_or_default();
    if account.sign_in_again {
        Some("needs signing in again".to_owned())
    } else if account.out_of_credit {
        Some("is out of credit".to_owned())
    } else if until > now {
        Some(format!("is at its usage limit until {}", factory_agent::accounts::when(until)))
    } else {
        None
    }
}

/// A subscription account's limits, as models_list says them: "Usage of
/// ana@example.com (Claude Max 20x): 5-hour 82% (resets Tue 18:00), Weekly
/// 40% (resets Mon 09:00)." None for an account with none to read.
fn limits(account: &v1::Account, plan: &str, usage: &v1::ProviderUsage) -> Option<String> {
    let mut line = format!("Usage of {}", label(account, false));
    if !plan.is_empty() && plan != account.label {
        let _ = write!(line, " ({plan})");
    }
    if usage.windows.is_empty() {
        return (!usage.error.is_empty()).then(|| format!("{line} couldn't be read: {}", usage.error.trim_end_matches('.')));
    }
    let windows: Vec<String> = usage
        .windows
        .iter()
        .map(|window| {
            let name = if window.label.is_empty() { &window.id } else { &window.label };
            let mut said = format!("{name} {}%", window.used_percent.clamp(0.0, 100.0).round());
            if let Ok(at) = u64::try_from(window.resets_at)
                && at > 0
            {
                let _ = write!(said, " (resets {})", reset_time(at));
            }
            said
        })
        .collect();
    let _ = write!(line, ": {}", windows.join(", "));
    if let Ok(read) = u64::try_from(usage.updated)
        && read > 0
    {
        let _ = write!(line, "; read {}", factory_agent::accounts::when(read));
    }
    Some(line)
}

/// When a limit resets: "Tue 18:00", with the date when that's six days
/// or more away, so a weekly reset doesn't read as today ("Mon 30 Nov 14:50").
fn reset_time(at: u64) -> String {
    let soon = at.saturating_sub(factory_agent::clock::now_ms()) < 6 * 24 * 3_600_000;
    match i64::try_from(at) {
        Ok(ms) if !soon => crate::transcript::local_time(ms, "%a %-d %b %H:%M"),
        _ => factory_agent::accounts::when(at),
    }
}

/// How an account is named; `hide`: as "an account" whatever its label.
fn label(account: &v1::Account, hide: bool) -> &str {
    if account.label.is_empty() || hide { "an account" } else { &account.label }
}

/// "anthropic" of "anthropic/claude-opus-4-8".
fn provider_of(id: &str) -> &str {
    id.split('/').next().unwrap_or_default()
}

/// A model as models_list shows it.
fn line(model: &v1::Model) -> String {
    let mut line = format!("{}: {}", model.id, model.name);
    if model.efforts.is_empty() {
        line.push_str("; no efforts");
    } else {
        let _ = write!(line, "; efforts {}", model.efforts.join(", "));
    }
    if model.context_window > 0 {
        let _ = write!(line, "; {} context", tokens(model.context_window));
    }
    line
}

/// 200000 as "200k", 1000000 as "1M", 1048576 as "1.05M".
fn tokens(count: u64) -> String {
    if count >= 1_000_000 {
        let millions = format!("{:.2}", count as f64 / 1e6);
        format!("{}M", millions.trim_end_matches('0').trim_end_matches('.'))
    } else if count >= 1_000 {
        format!("{}k", (count + 500) / 1_000)
    } else {
        count.to_string()
    }
}

/// Lower case, with "-", "." and "_" alike.
fn normal(text: &str) -> String {
    text.to_lowercase().chars().map(|c| if matches!(c, '.' | '_') { '-' } else { c }).collect()
}

/// Only the letters and digits, lower case.
fn compact(text: &str) -> String {
    text.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
}

/// A query's words: split at spaces, commas and slashes.
fn words(query: &str) -> Vec<String> {
    query.split(|c: char| c.is_whitespace() || c == ',' || c == '/').filter(|word| !word.is_empty()).map(normal).collect()
}

/// Every word is in the model's id or name.
fn matches(words: &[String], model: &v1::Model) -> bool {
    let text = normal(&format!("{} {}", model.id, model.name));
    words.iter().all(|word| text.contains(word.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, name: &str) -> v1::Model {
        v1::Model { id: id.into(), name: name.into(), ..Default::default() }
    }

    #[test]
    fn a_query_matches_loosely_across_providers() {
        let on_anthropic = model("anthropic/claude-opus-4-8", "Claude Opus 4.8");
        let on_openrouter = model("openrouter/anthropic/claude-opus-4.8", "Anthropic: Claude Opus 4.8");
        let sonnet = model("openrouter/anthropic/claude-sonnet-4.6", "Anthropic: Claude Sonnet 4.6");
        for query in ["opus", "Opus 4.8", "opus-4_8", "claude opus, 4.8", "openrouter/anthropic/claude-opus-4-8"] {
            let words = words(query);
            assert!(matches(&words, &on_openrouter), "{query}");
            assert_eq!(matches(&words, &on_anthropic), !query.starts_with("openrouter"), "{query}");
            assert!(!matches(&words, &sonnet), "{query}");
        }
        assert!(matches(&words(""), &sonnet), "no query matches every model");
    }

    #[test]
    fn a_reset_a_week_away_says_its_date() {
        let now = factory_agent::clock::now_ms();
        let week = reset_time(now + 7 * 24 * 3_600_000);
        assert_eq!(week.split(' ').count(), 4, "Mon 30 Nov 14:50: {week}");
        let soon = reset_time(now + 3_600_000);
        assert_eq!(soon.split(' ').count(), 2, "Tue 18:00: {soon}");
    }

    #[test]
    fn context_sizes_read_short() {
        assert_eq!(tokens(200_000), "200k");
        assert_eq!(tokens(1_000_000), "1M");
        assert_eq!(tokens(1_048_576), "1.05M");
        assert_eq!(tokens(128_000), "128k");
        assert_eq!(tokens(400), "400");
    }
}
