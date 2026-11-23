//! Continue on another provider (#341): an agent or a chat waiting out its
//! provider's usage limit (or credit) can switch, at the person's tap, to
//! the same model on another signed-in provider with room, or the closest
//! one (OpenRouter carries Anthropic's and OpenAI's models). Never by
//! itself: another provider may cost the person money. Each switch is kept
//! in factory.db, so once the first provider has room again the person can
//! go back with one tap; their own model change forgets it.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};

use factory_protocol::v1;
use rusqlite::params;

use crate::factory::{AgentChange, Factory, Inner, RequestError};

pub(crate) const SCHEMA: &str = "
-- Agents and chats that continued on another provider while theirs was at
-- its usage limit (#341): the model and effort they ran on before, to go
-- back to.
CREATE TABLE usage_switches (
    id TEXT PRIMARY KEY,
    model TEXT NOT NULL,
    effort TEXT NOT NULL,
    at INTEGER NOT NULL
);
";

/// What an agent or chat ran on before it switched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Before {
    pub model: String,
    pub effort: String,
}

/// The switches, as in factory.db.
#[derive(Default)]
pub(crate) struct Switches {
    kept: Mutex<HashMap<String, Before>>,
}

impl Switches {
    pub(crate) fn load(store: &crate::store::Store) -> Switches {
        let db = store.db();
        let read = || -> rusqlite::Result<HashMap<String, Before>> {
            let mut statement = db.prepare("SELECT id, model, effort FROM usage_switches")?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, Before { model: row.get(1)?, effort: row.get(2)? })))?;
            rows.collect()
        };
        let kept = read().unwrap_or_else(|error| {
            tracing::error!(%error, "could not read the agents switched to another provider");
            HashMap::new()
        });
        Switches { kept: Mutex::new(kept) }
    }

    fn kept(&self) -> MutexGuard<'_, HashMap<String, Before>> {
        self.kept.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn get(&self, id: &str) -> Option<Before> {
        self.kept().get(id).cloned()
    }

    fn set(&self, store: &crate::store::Store, id: &str, before: Before) {
        let saved = store.db().execute(
            "INSERT OR REPLACE INTO usage_switches (id, model, effort, at) VALUES (?1, ?2, ?3, ?4)",
            params![id, before.model, before.effort, crate::transcript::now_ms()],
        );
        if let Err(error) = saved {
            tracing::error!(agent = %id, %error, "could not keep the switch to another provider");
        }
        self.kept().insert(id.to_owned(), before);
    }

    /// Forgets the agent's or chat's switch: it went back, the person chose
    /// its model, or it's gone.
    pub(crate) fn forget(&self, store: &crate::store::Store, id: &str) {
        if self.kept().remove(id).is_none() {
            return;
        }
        if let Err(error) = store.db().execute("DELETE FROM usage_switches WHERE id = ?1", [id]) {
            tracing::error!(agent = %id, %error, "could not forget the switch to another provider");
        }
    }
}

// ---- matching models across providers ----

/// Whose model a model id is ("anthropic", "openai", …), and its name there:
/// "openrouter/anthropic/claude-opus-4.8" is Anthropic's "claude-opus-4.8".
fn maker_and_name(model: &str) -> Option<(&str, &str)> {
    let (provider, id) = model.split_once('/')?;
    match provider {
        "openrouter" => id.split_once('/'),
        "openai-codex" => Some(("openai", id)),
        _ => Some((provider, id)),
    }
}

/// A model's family and version, from its name: "claude-opus-4-5-20251101"
/// and "claude-opus-4.5" are both (claude-opus, 4.5). None for variants no
/// agent is switched to (OpenRouter's ":batch" and the like).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Kind {
    maker: String,
    family: Vec<String>,
    version: Vec<u32>,
}

fn kind(model: &str) -> Option<Kind> {
    let (maker, name) = maker_and_name(model)?;
    if name.contains(':') {
        return None;
    }
    let mut family = Vec::new();
    let mut version = Vec::new();
    for token in name.to_ascii_lowercase().split(['-', '.', '_']).filter(|token| !token.is_empty()) {
        if token.bytes().all(|byte| byte.is_ascii_digit()) {
            // A date ("20251101") isn't part of the version.
            if token.len() < 6 {
                version.push(token.parse().ok()?);
            }
        } else {
            family.push(token.to_owned());
        }
    }
    Some(Kind { maker: maker.to_owned(), family, version })
}

fn provider_of(model: &str) -> &str {
    model.split_once('/').map_or(model, |(provider, _)| provider)
}

/// Whether a signed-in provider has an account that can take a request now.
fn has_room(provider: &v1::Provider, now: i64) -> bool {
    if provider.signed_in_with.is_empty() {
        return false;
    }
    provider.accounts.is_empty()
        || provider.accounts.iter().any(|account| account.exhausted_until <= now && !account.out_of_credit && !account.sign_in_again)
}

const EFFORTS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// The effort `effort` maps to on a model with `efforts`: the same, or the
/// highest below it, or its lowest.
fn effort_on(efforts: &[String], effort: &str) -> String {
    if efforts.is_empty() || efforts.iter().any(|known| known == effort) {
        return effort.to_owned();
    }
    let rank = |level: &str| EFFORTS.iter().position(|known| *known == level);
    let wanted = rank(effort).unwrap_or(3);
    let mut ranked: Vec<(usize, &String)> = efforts.iter().filter_map(|known| rank(known).map(|at| (at, known))).collect();
    ranked.sort();
    ranked.iter().rev().find(|(at, _)| *at < wanted).or(ranked.first()).map_or_else(|| effort.to_owned(), |(_, known)| (*known).clone())
}

/// A model's name as the person reads it: OpenRouter's "Anthropic: Claude
/// Opus 4.8" without its maker.
fn shown_name(name: &str) -> String {
    name.split_once(": ").map_or(name, |(_, rest)| rest).to_owned()
}

/// How a candidate model ranks, lower first: (not the same model, above
/// it, not the maker's own provider, newest below first, nearest above).
type Rank = (bool, bool, bool, std::cmp::Reverse<Vec<u32>>, Vec<u32>);

/// What `model` at `effort`, waiting on its provider, could continue on:
/// a model of another provider with room, the same one first, the maker's
/// own provider before OpenRouter.
fn alternative(providers: &[v1::Provider], models: &[v1::Model], model: &str, effort: &str, now: i64) -> Option<v1::UsageWaitAlternative> {
    let wanted = kind(model)?;
    let waiting_on = provider_of(model);
    let usable: HashSet<&str> = providers
        .iter()
        .filter(|provider| provider.id != waiting_on && has_room(provider, now))
        .map(|provider| provider.id.as_str())
        .collect();
    let mut best: Option<(Rank, &v1::Model)> = None;
    for candidate in models {
        if !usable.contains(provider_of(&candidate.id)) {
            continue;
        }
        let Some(found) = kind(&candidate.id) else { continue };
        if found.maker != wanted.maker || found.family != wanted.family {
            continue;
        }
        let same = found.version == wanted.version;
        let below = found.version <= wanted.version;
        let own = provider_of(&candidate.id) != "openrouter";
        // Lower sorts first: the same model, then the newest not above it,
        // then the nearest above; the maker's own provider first.
        let rank = (!same, !below, !own, std::cmp::Reverse(if below { found.version.clone() } else { Vec::new() }), found.version.clone());
        if best.as_ref().is_none_or(|(ranked, _)| rank < *ranked) {
            best = Some((rank, candidate));
        }
    }
    let ((not_same, ..), chosen) = best?;
    let provider = providers.iter().find(|provider| provider.id == provider_of(&chosen.id))?;
    let to_effort = effort_on(&chosen.efforts, effort);
    Some(v1::UsageWaitAlternative {
        provider_name: provider.name.clone(),
        model: chosen.id.clone(),
        model_name: shown_name(&chosen.name),
        same_model: !not_same,
        effort_changed: to_effort != effort,
        effort: to_effort,
        others_waiting: 0,
    })
}

// ---- the view ----

/// An agent's or chat's model and effort, by id.
fn model_and_effort(factory: &Factory, inner: &Inner, id: &str) -> Option<(String, String)> {
    match inner.config.agent(id) {
        Some(agent) => Some((agent.model.clone(), agent.effort.clone())),
        None => factory.chats.model_and_effort(id),
    }
}

/// The waits' alternatives and the switches, for the Workspace.
#[derive(Default)]
pub(crate) struct View {
    alternatives: HashMap<String, v1::UsageWaitAlternative>,
    no_other: HashSet<String>,
    switches: HashMap<String, v1::UsageSwitch>,
}

impl View {
    pub(crate) fn alternative(&self, id: &str) -> Option<v1::UsageWaitAlternative> {
        self.alternatives.get(id).cloned()
    }

    pub(crate) fn no_other_provider(&self, id: &str) -> bool {
        self.no_other.contains(id)
    }

    pub(crate) fn switch(&self, id: &str) -> Option<v1::UsageSwitch> {
        self.switches.get(id).cloned()
    }
}

/// Who waits on which provider with somewhere else to go, and who switched.
pub(crate) fn view(factory: &Factory, inner: &Inner) -> View {
    let now = crate::transcript::now_ms();
    let mut view = View::default();
    let mut waiting_on: HashMap<String, String> = HashMap::new();
    for (id, runtime) in &inner.agents {
        if !runtime.usage_wait.as_ref().is_some_and(|wait| wait.switchable) {
            continue;
        }
        let Some((model, effort)) = model_and_effort(factory, inner, id) else { continue };
        let provider = provider_of(&model).to_owned();
        if !inner.providers.iter().any(|known| known.id != provider && !known.signed_in_with.is_empty()) {
            view.no_other.insert(id.clone());
        }
        if let Some(alternative) = alternative(&inner.providers, &inner.models, &model, &effort, now) {
            view.alternatives.insert(id.clone(), alternative);
            waiting_on.insert(id.clone(), provider);
        }
    }
    for (id, alternative) in &mut view.alternatives {
        let provider = &waiting_on[id];
        alternative.others_waiting = waiting_on.iter().filter(|(other, on)| *other != id && *on == provider).count() as u32;
    }
    let kept: Vec<(String, Before)> = factory.usage_switches.kept().iter().map(|(id, before)| (id.clone(), before.clone())).collect();
    let mut ready_from: HashMap<String, Vec<String>> = HashMap::new();
    for (id, before) in kept {
        if model_and_effort(factory, inner, &id).is_none() {
            continue;
        }
        let provider_id = provider_of(&before.model);
        let provider = inner.providers.iter().find(|known| known.id == provider_id);
        let listed = inner.models.iter().find(|known| known.id == before.model);
        let ready = provider.is_some_and(|provider| has_room(provider, now)) && listed.is_some();
        if ready {
            ready_from.entry(provider_id.to_owned()).or_default().push(id.clone());
        }
        let name = provider.map_or(provider_id, |provider| provider.name.as_str());
        view.switches.insert(
            id,
            v1::UsageSwitch {
                provider_name: factory_agent::accounts::account_noun(provider_id, name),
                model_name: listed.map_or_else(|| before.model.clone(), |model| shown_name(&model.name)),
                model: before.model,
                ready,
                others_ready: 0,
            },
        );
    }
    for switch in view.switches.values_mut().filter(|switch| switch.ready) {
        switch.others_ready = ready_from.get(provider_of(&switch.model)).map_or(0, |ids| ids.len() - 1) as u32;
    }
    view
}

// ---- switching ----

/// Gives an agent or chat a model and effort, as the person changing them
/// would, without forgetting a switch.
async fn run_on(factory: &Arc<Factory>, id: &str, model: &str, effort: &str) -> Result<(), RequestError> {
    if crate::chats::is_chat(id) {
        crate::chats::set_model(factory, id, model, effort)
    } else {
        let change = AgentChange { model: Some(model), effort: Some(effort), ..AgentChange::default() };
        factory.update_agent_keeping_switch(id, change).await
    }
}

fn name_of(factory: &Factory, inner: &Inner, id: &str) -> Option<String> {
    match inner.config.agent(id) {
        Some(agent) => Some(agent.name.clone()),
        None => factory.chats.exists(id).then(|| "The chat".to_owned()),
    }
}

/// With `all`, the others the person saw (`expected`, 0 for unchecked)
/// must be those there are now: none is switched that they didn't count.
fn as_seen(expected: u32, now: usize, what: &str) -> Result<(), RequestError> {
    if expected == 0 || now == expected as usize {
        return Ok(());
    }
    Err(RequestError::Wait(format!("{} {what} now, not {}. Choose again.", now + 1, expected + 1)))
}

/// ContinueOnAlternative.
pub(crate) async fn continue_on(factory: &Arc<Factory>, id: &str, all_waiting: bool, expected: u32) -> Result<(), RequestError> {
    let moves: Vec<(String, String, String, Before)> = {
        let inner = factory.lock();
        let Some(name) = name_of(factory, &inner, id) else { return Err(RequestError::UnknownAgent(id.to_owned())) };
        let view = view(factory, &inner);
        let Some(chosen) = view.alternatives.get(id) else {
            let waiting = inner.agents.get(id).is_some_and(|runtime| runtime.usage_wait.is_some());
            return Err(RequestError::Wait(if waiting {
                format!("No other provider can run {name}'s model right now.")
            } else {
                format!("{name} isn't waiting anymore.")
            }));
        };
        let provider = model_and_effort(factory, &inner, id).map(|(model, _)| provider_of(&model).to_owned()).unwrap_or_default();
        let mut ids = vec![id.to_owned()];
        if all_waiting {
            let mut others: Vec<String> = view
                .alternatives
                .keys()
                .filter(|other| *other != id)
                .filter(|other| model_and_effort(factory, &inner, other).is_some_and(|(model, _)| provider_of(&model) == provider))
                .cloned()
                .collect();
            as_seen(expected, others.len(), "are waiting")?;
            others.sort();
            ids.extend(others);
        }
        tracing::info!(agent = %id, to = %chosen.model, count = ids.len(), "continuing on another provider");
        ids.into_iter()
            .filter_map(|id| {
                let alternative = view.alternatives.get(&id)?;
                let (model, effort) = model_and_effort(factory, &inner, &id)?;
                Some((id, alternative.model.clone(), alternative.effort.clone(), Before { model, effort }))
            })
            .collect()
    };
    for (id, model, effort, before) in moves {
        // The first switch is what it goes back to.
        let before = factory.usage_switches.get(&id).unwrap_or(before);
        run_on(factory, &id, &model, &effort).await?;
        factory.usage_switches.set(&factory.store, &id, before);
    }
    factory.broadcast_workspace(&factory.lock());
    factory.capacity_changed();
    Ok(())
}

/// SwitchBack.
pub(crate) async fn switch_back(factory: &Arc<Factory>, id: &str, all: bool, expected: u32) -> Result<(), RequestError> {
    let backs: Vec<(String, Before)> = {
        let inner = factory.lock();
        let Some(name) = name_of(factory, &inner, id) else { return Err(RequestError::UnknownAgent(id.to_owned())) };
        let Some(before) = factory.usage_switches.get(id) else {
            return Err(RequestError::Wait(format!("{name} didn't switch to another provider.")));
        };
        let mut backs = vec![(id.to_owned(), before.clone())];
        if all {
            let view = view(factory, &inner);
            let mut others: Vec<(String, Before)> = view
                .switches
                .iter()
                .filter(|(other, switch)| *other != id && switch.ready && provider_of(&switch.model) == provider_of(&before.model))
                .filter_map(|(other, _)| Some((other.clone(), factory.usage_switches.get(other)?)))
                .collect();
            as_seen(expected, others.len(), "are ready to go back")?;
            others.sort_by(|a, b| a.0.cmp(&b.0));
            backs.extend(others);
        }
        backs
    };
    for (id, before) in backs {
        run_on(factory, &id, &before.model, &before.effort).await?;
        factory.usage_switches.forget(&factory.store, &id);
    }
    factory.broadcast_workspace(&factory.lock());
    factory.capacity_changed();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, name: &str, efforts: &[&str]) -> v1::Model {
        v1::Model {
            id: id.into(),
            name: name.into(),
            efforts: efforts.iter().map(|effort| (*effort).to_owned()).collect(),
            ..Default::default()
        }
    }

    fn provider(id: &str, name: &str, accounts: Vec<v1::Account>) -> v1::Provider {
        v1::Provider { id: id.into(), name: name.into(), signed_in_with: "key".into(), accounts, ..Default::default() }
    }

    struct Known {
        providers: Vec<v1::Provider>,
        models: Vec<v1::Model>,
    }

    fn inner_with(providers: Vec<v1::Provider>, models: Vec<v1::Model>) -> Known {
        Known { providers, models }
    }

    fn alternative(known: &Known, model: &str, effort: &str, now: i64) -> Option<v1::UsageWaitAlternative> {
        super::alternative(&known.providers, &known.models, model, effort, now)
    }

    #[test]
    fn the_same_model_matches_across_providers_whatever_its_spelling() {
        assert_eq!(kind("anthropic/claude-opus-4-5-20251101"), kind("openrouter/anthropic/claude-opus-4.5"));
        assert_eq!(kind("openai-codex/gpt-5.6-sol"), kind("openrouter/openai/gpt-5.6-sol"));
        assert_eq!(kind("openai-codex/gpt-5.6-sol"), kind("openai/gpt-5.6-sol"));
        assert_ne!(kind("anthropic/claude-opus-4-7"), kind("openrouter/anthropic/claude-opus-4.7-fast"));
        assert_eq!(kind("openrouter/anthropic/claude-opus-4.7:batch"), None);
    }

    #[test]
    fn an_agent_continues_on_the_same_model_or_the_closest_and_keeps_its_effort_where_it_can() {
        let efforts = ["low", "medium", "high"];
        let inner = inner_with(
            vec![provider("anthropic", "Anthropic", Vec::new()), provider("openrouter", "OpenRouter", Vec::new())],
            vec![
                model("anthropic/claude-opus-4-8", "Claude Opus 4.8", &["off", "low", "medium", "high", "xhigh", "max"]),
                model("openrouter/anthropic/claude-opus-4.8", "Anthropic: Claude Opus 4.8", &efforts),
                model("openrouter/anthropic/claude-opus-4.8-fast", "Anthropic: Claude Opus 4.8 Fast", &efforts),
                model("openrouter/anthropic/claude-opus-5", "Anthropic: Claude Opus 5", &efforts),
                model("openrouter/anthropic/claude-opus-4.5", "Anthropic: Claude Opus 4.5", &efforts),
                model("openrouter/anthropic/claude-sonnet-5", "Anthropic: Claude Sonnet 5", &efforts),
            ],
        );
        let same = alternative(&inner, "anthropic/claude-opus-4-8", "xhigh", 0).expect("an alternative");
        assert_eq!(
            (same.model.as_str(), same.model_name.as_str(), same.same_model, same.effort.as_str(), same.effort_changed),
            ("openrouter/anthropic/claude-opus-4.8", "Claude Opus 4.8", true, "high", true)
        );
        assert_eq!(same.provider_name, "OpenRouter");
        // Not there: the newest of its family not above it.
        let closest = alternative(&inner, "anthropic/claude-opus-5-5", "medium", 0).expect("an alternative");
        assert_eq!(
            (closest.model.as_str(), closest.same_model, closest.effort_changed),
            ("openrouter/anthropic/claude-opus-5", false, false)
        );
        // Nothing below it: the nearest above.
        let above = alternative(&inner, "anthropic/claude-opus-4-1", "medium", 0).expect("an alternative");
        assert_eq!(above.model, "openrouter/anthropic/claude-opus-4.5");
        // Never another family.
        assert_eq!(alternative(&inner, "anthropic/claude-haiku-4-5", "low", 0), None);
    }

    #[test]
    fn only_a_signed_in_provider_with_room_carries_it_the_makers_own_first() {
        let spent = v1::Account { exhausted_until: 2_000, ..Default::default() };
        let models = vec![
            model("openai-codex/gpt-5.6-sol", "GPT-5.6 Sol", &[]),
            model("openai/gpt-5.6-sol", "GPT-5.6 Sol", &[]),
            model("openrouter/openai/gpt-5.6-sol", "OpenAI: GPT-5.6 Sol", &[]),
        ];
        let both = vec![
            provider("openai-codex", "OpenAI Codex", Vec::new()),
            provider("openai", "OpenAI", Vec::new()),
            provider("openrouter", "OpenRouter", Vec::new()),
        ];
        let inner = inner_with(both.clone(), models.clone());
        assert_eq!(alternative(&inner, "openai-codex/gpt-5.6-sol", "medium", 1_000).expect("one").model, "openai/gpt-5.6-sol");
        let mut spent_key = both;
        spent_key[1].accounts = vec![spent];
        let inner = inner_with(spent_key.clone(), models.clone());
        assert_eq!(alternative(&inner, "openai-codex/gpt-5.6-sol", "medium", 1_000).expect("one").model, "openrouter/openai/gpt-5.6-sol");
        // Its own provider never counts, and a spent one counts again once reset.
        assert_eq!(alternative(&inner, "openai-codex/gpt-5.6-sol", "medium", 3_000).expect("one").model, "openai/gpt-5.6-sol");
        spent_key[2].signed_in_with.clear();
        let inner = inner_with(spent_key, models);
        assert_eq!(alternative(&inner, "openai-codex/gpt-5.6-sol", "medium", 1_000), None);
    }

    #[test]
    fn an_effort_the_model_lacks_maps_to_the_highest_below_or_its_lowest() {
        let efforts: Vec<String> = ["low", "medium", "high"].iter().map(|effort| (*effort).to_owned()).collect();
        assert_eq!(effort_on(&efforts, "medium"), "medium");
        assert_eq!(effort_on(&efforts, "max"), "high");
        assert_eq!(effort_on(&efforts, "off"), "low");
        assert_eq!(effort_on(&[], "max"), "max");
    }
}
