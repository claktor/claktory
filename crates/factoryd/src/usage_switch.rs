//! Continue on another provider: an agent or a chat waiting out its
//! provider's usage limit (or credit) can switch, at the person's tap, to
//! the same model on another signed-in provider with room, or the closest
//! one (OpenRouter carries Anthropic's and OpenAI's models). Never by
//! itself: another provider may cost the person money. Each switch is kept
//! in factory.db, and once the first provider has room again it goes back
//! by itself, which only saves money, or with one tap
//! sooner; the person's own model change forgets it.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};

use factory_agent::providers::models::Pool;
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

/// A switch doesn't go back by itself before `hold_until` (ms since 1970):
/// it went back once and reached the limit again.
pub(crate) const HOLD: &str = "ALTER TABLE usage_switches ADD COLUMN hold_until INTEGER NOT NULL DEFAULT 0;";

/// Whether waits on a usage limit continue on another provider by
/// themselves: off unless the person turns it on.
pub(crate) const AUTO: &str = "
CREATE TABLE usage_settings (name TEXT PRIMARY KEY, value INTEGER NOT NULL);
";

/// How soon after going back by itself a limit again counts as bouncing.
const BOUNCE_MS: i64 = 10 * 60 * 1000;

/// What an agent or chat ran on before it switched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Before {
    pub model: String,
    pub effort: String,
}

/// A switch as kept: what it goes back to, and not by itself before when.
#[derive(Clone, Debug)]
struct Kept {
    before: Before,
    hold_until: i64,
}

/// The switches, as in factory.db, and when each agent or chat last went
/// back by itself (for the bounce guard; not kept across restarts).
#[derive(Default)]
pub(crate) struct Switches {
    kept: Mutex<HashMap<String, Kept>>,
    came_back: Mutex<HashMap<String, i64>>,
    /// Tests: switches go back only at the person's tap.
    only_by_tap: std::sync::atomic::AtomicBool,
    /// Workspace.usage_auto_continue.
    auto_continue: std::sync::atomic::AtomicBool,
}

impl Switches {
    pub(crate) fn load(store: &crate::store::Store) -> Switches {
        let db = store.db();
        let read = || -> rusqlite::Result<HashMap<String, Kept>> {
            let mut statement = db.prepare("SELECT id, model, effort, hold_until FROM usage_switches")?;
            let rows = statement.query_map([], |row| {
                Ok((row.get(0)?, Kept { before: Before { model: row.get(1)?, effort: row.get(2)? }, hold_until: row.get(3)? }))
            })?;
            rows.collect()
        };
        let kept = read().unwrap_or_else(|error| {
            tracing::error!(%error, "could not read the agents switched to another provider");
            HashMap::new()
        });
        let auto_continue = db
            .query_row("SELECT value FROM usage_settings WHERE name = 'auto_continue'", [], |row| row.get::<_, i64>(0))
            .map(|value| value != 0)
            .unwrap_or(false);
        Switches { kept: Mutex::new(kept), auto_continue: auto_continue.into(), ..Switches::default() }
    }

    fn kept(&self) -> MutexGuard<'_, HashMap<String, Kept>> {
        self.kept.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn came_back(&self) -> MutexGuard<'_, HashMap<String, i64>> {
        self.came_back.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Workspace.usage_auto_continue.
    pub(crate) fn auto_continue(&self) -> bool {
        self.auto_continue.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn get(&self, id: &str) -> Option<Before> {
        self.kept().get(id).map(|kept| kept.before.clone())
    }

    fn set(&self, store: &crate::store::Store, id: &str, before: Before, hold_until: i64) {
        let saved = store.db().execute(
            "INSERT OR REPLACE INTO usage_switches (id, model, effort, at, hold_until) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, before.model, before.effort, crate::transcript::now_ms(), hold_until],
        );
        if let Err(error) = saved {
            tracing::error!(agent = %id, %error, "could not keep the switch to another provider");
        }
        self.kept().insert(id.to_owned(), Kept { before, hold_until });
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
        // The Grok subscription's models are xAI's, as OpenRouter names them.
        "grok" => Some(("x-ai", id)),
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

/// Which of its provider's limits the model draws on (factory-agent's
/// `Pool`): Fable's, Opus's or Sonnet's own weekly one besides the shared
/// ones, or the shared ones alone.
pub(crate) fn pool(model: &str) -> Pool {
    model.split_once('/').map_or(Pool::Normal, |(provider, id)| Pool::of(provider, id))
}

/// The account's limit mark for the pool: Fable's own, else the shared
/// limits'. An Opus or Sonnet limit of their own shows in the usage
/// windows instead (`own_full`).
fn mark(account: &v1::Account, pool: Pool) -> i64 {
    if pool == Pool::Fable { account.fable_exhausted_until } else { account.exhausted_until }
}

/// The accounts' last usage reads, kept apart from the providers' listing
/// (`Inner::usage`).
type Usages = HashMap<crate::usage::AccountKey, v1::ProviderUsage>;

/// The account's last usage read: the one kept in `Inner::usage`, else the
/// listing's.
fn usage_of<'a>(usages: &'a Usages, provider: &v1::Provider, account: &'a v1::Account) -> Option<&'a v1::ProviderUsage> {
    usages.get(&(provider.id.clone(), account.id.clone())).or(account.usage.as_ref())
}

/// Whether the account's last usage read has the pool's own weekly window
/// at 100%.
fn own_full(usage: Option<&v1::ProviderUsage>, pool: Pool) -> bool {
    usage.is_some_and(|usage| {
        usage.windows.iter().any(|window| Some(window.id.as_str()) == pool.own_window() && window.used_percent >= 100.0)
    })
}

/// Whether a signed-in provider has an account that can take a request for
/// the pool now.
fn has_room(usages: &Usages, provider: &v1::Provider, pool: Pool, now: i64) -> bool {
    if provider.signed_in_with.is_empty() {
        return false;
    }
    provider.accounts.is_empty()
        || provider.accounts.iter().any(|account| {
            mark(account, pool) <= now
                && !account.out_of_credit
                && !account.sign_in_again
                && !own_full(usage_of(usages, provider, account), pool)
        })
}

/// Whether a usage window limits the pool (`Pool::limited_by`).
fn limits(window: &v1::UsageWindow, pool: Pool) -> bool {
    pool.limited_by(&window.id)
}

/// Whether the provider has room for a switch to go back by itself: an
/// account with room whose last usage read has no window of the pool at
/// 100% (another window, weekly say, can still be full when one limit's
/// mark has passed).
fn room_to_go_back(inner: &Inner, provider: &v1::Provider, pool: Pool, now: i64) -> bool {
    if !has_room(&inner.usage, provider, pool, now) {
        return false;
    }
    if provider.accounts.is_empty() {
        return true;
    }
    provider.accounts.iter().any(|account| {
        mark(account, pool) <= now
            && !account.out_of_credit
            && !account.sign_in_again
            && usage_of(&inner.usage, provider, account)
                .is_none_or(|usage| usage.windows.iter().all(|window| !limits(window, pool) || window.used_percent < 100.0))
    })
}

/// When the provider should have room again for the pool (ms since 1970):
/// the earliest, over its accounts, of the later of its limit mark and its
/// full windows' resets. 0 when it has room now or nothing says.
fn room_at(inner: &Inner, provider: &v1::Provider, pool: Pool, now: i64) -> i64 {
    if room_to_go_back(inner, provider, pool, now) {
        return 0;
    }
    provider
        .accounts
        .iter()
        .filter(|account| !account.out_of_credit && !account.sign_in_again)
        .filter_map(|account| {
            let usage = usage_of(&inner.usage, provider, account);
            let full: Vec<i64> = usage
                .map(|usage| {
                    usage
                        .windows
                        .iter()
                        .filter(|window| limits(window, pool) && window.used_percent >= 100.0)
                        .map(|window| window.resets_at)
                        .collect()
                })
                .unwrap_or_default();
            // A full window with no reset time: nothing says when.
            if full.contains(&0) {
                return None;
            }
            let at = full.into_iter().chain([mark(account, pool)]).max().unwrap_or(0);
            (at > now).then_some(at)
        })
        .min()
        .unwrap_or(0)
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
fn alternative(
    usages: &Usages,
    providers: &[v1::Provider],
    models: &[v1::Model],
    model: &str,
    effort: &str,
    now: i64,
) -> Option<v1::UsageWaitAlternative> {
    let wanted = kind(model)?;
    let waiting_on = provider_of(model);
    let mut best: Option<(Rank, &v1::Model)> = None;
    for candidate in models {
        let usable = providers
            .iter()
            .find(|provider| provider.id == provider_of(&candidate.id))
            .is_some_and(|provider| provider.id != waiting_on && has_room(usages, provider, pool(&candidate.id), now));
        if !usable {
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

/// How many agents and chats draw on each pool now, by provider: (provider
/// id, Fable's own pool). An account is shown in use only for these: a
/// provider no agent or chat runs on isn't, whatever has room.
pub(crate) fn pools_in_use(factory: &Factory, inner: &Inner) -> HashMap<(String, bool), u32> {
    let agents = inner.config.agents.iter().map(|agent| agent.model.clone());
    let chats = factory.chats.ids().into_iter().filter_map(|id| factory.chats.model_and_effort(&id).map(|(model, _)| model));
    let mut pools = HashMap::new();
    for model in agents.chain(chats) {
        let Some((provider, id)) = model.split_once('/') else { continue };
        let pool = (provider.to_owned(), factory_agent::providers::models::in_fable_pool(provider, id));
        *pools.entry(pool).or_insert(0) += 1;
    }
    pools
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
        if let Some(alternative) = alternative(&inner.usage, &inner.providers, &inner.models, &model, &effort, now) {
            view.alternatives.insert(id.clone(), alternative);
            waiting_on.insert(id.clone(), provider);
        }
    }
    for (id, alternative) in &mut view.alternatives {
        let provider = &waiting_on[id];
        alternative.others_waiting = waiting_on.iter().filter(|(other, on)| *other != id && *on == provider).count() as u32;
    }
    let kept: Vec<(String, Kept)> = factory.usage_switches.kept().iter().map(|(id, kept)| (id.clone(), kept.clone())).collect();
    let mut ready_from: HashMap<String, Vec<String>> = HashMap::new();
    for (id, Kept { before, hold_until }) in kept {
        if model_and_effort(factory, inner, &id).is_none() {
            continue;
        }
        let provider_id = provider_of(&before.model);
        let provider = inner.providers.iter().find(|known| known.id == provider_id);
        let listed = inner.models.iter().find(|known| known.id == before.model);
        let pool = pool(&before.model);
        let ready = provider.is_some_and(|provider| has_room(&inner.usage, provider, pool, now)) && listed.is_some();
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
                goes_back: hold_until <= now,
                goes_back_at_ms: provider.map_or(0, |provider| room_at(inner, provider, pool, now)),
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
/// would, without forgetting a switch; `why` is what the agent hears of it.
async fn run_on(factory: &Arc<Factory>, id: &str, model: &str, effort: &str, why: &str) -> Result<(), RequestError> {
    if crate::chats::is_chat(id) {
        crate::chats::set_model(factory, id, model, effort)
    } else {
        let change = AgentChange { model: Some(model), effort: Some(effort), why: Some(why), ..AgentChange::default() };
        factory.update_agent_keeping_switch(id, change).await
    }
}

/// What the person did from the device `from`, as an agent hears it: "Matt
/// changed it on Matt's Mac". `person`: `Config::person`.
pub(crate) fn by_person(person: &str, from: &factory_agent::SentFrom, did: &str) -> String {
    match from.device.trim() {
        "" => format!("{person} {did} in the app"),
        device => format!("{person} {did} on {device}"),
    }
}

/// "Anthropic" for an agent's account there: whose usage reached its limit.
fn noun_of(inner: &Inner, provider_id: &str) -> String {
    let name = inner.providers.iter().find(|known| known.id == provider_id).map_or(provider_id, |known| known.name.as_str());
    factory_agent::accounts::account_noun(provider_id, name)
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
/// `by`: the device the person chose it on; None when it's Claktory's own
/// choice (`continue_by_itself`).
pub(crate) async fn continue_on(
    factory: &Arc<Factory>,
    id: &str,
    all_waiting: bool,
    expected: u32,
    by: Option<&factory_agent::SentFrom>,
) -> Result<(), RequestError> {
    let moves: Vec<(String, String, String, Before, i64, String)> = {
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
                let reset = inner.agents.get(&id).and_then(|runtime| runtime.usage_wait.as_ref()).map_or(0, |wait| wait.until_ms);
                let noun = noun_of(&inner, provider_of(&model));
                let to = &alternative.provider_name;
                let why = match by {
                    Some(from) => {
                        format!("{}, while your {noun} usage is at its limit", by_person(inner.config.person(), from, &format!("chose {to}")))
                    }
                    None => format!(
                        "Claktory continued on {to} by itself, since your {noun} usage reached its limit and Continue automatically is on"
                    ),
                };
                Some((id, alternative.model.clone(), alternative.effort.clone(), Before { model, effort }, reset, why))
            })
            .collect()
    };
    for (id, model, effort, before, reset, why) in moves {
        // The first switch is what it goes back to.
        let kept = factory.usage_switches.kept().get(&id).cloned();
        let before = kept.as_ref().map_or(before, |kept| kept.before.clone());
        // Back by itself a moment ago and at the limit again: it stays
        // until this limit resets (or for good, when nothing says when).
        let now = crate::transcript::now_ms();
        let bounced = factory.usage_switches.came_back().get(&id).is_some_and(|at| now - at < BOUNCE_MS);
        let hold_until = if bounced { if reset > now { reset } else { i64::MAX } } else { 0 };
        let hold_until = kept.map_or(hold_until, |kept| kept.hold_until.max(hold_until));
        run_on(factory, &id, &model, &effort, &why).await?;
        factory.usage_switches.set(&factory.store, &id, before, hold_until);
    }
    factory.broadcast_workspace(&factory.lock());
    factory.capacity_changed();
    Ok(())
}

/// SwitchBack.
pub(crate) async fn switch_back(
    factory: &Arc<Factory>,
    id: &str,
    all: bool,
    expected: u32,
    by: &factory_agent::SentFrom,
) -> Result<(), RequestError> {
    let why = by_person(factory.lock().config.person(), by, "switched you back");
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
        run_on(factory, &id, &before.model, &before.effort, &why).await?;
        factory.usage_switches.forget(&factory.store, &id);
    }
    factory.broadcast_workspace(&factory.lock());
    factory.capacity_changed();
    Ok(())
}

impl Factory {
    /// Switches go back only when the person taps Back (SwitchBack), not
    /// by themselves: for tests of the tap.
    pub fn switch_back_only_by_tap(&self) {
        self.usage_switches.only_by_tap.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// U19: what a new agent or chat on `model` (the default one)
/// starts on while others are switched away from its provider and it still
/// has no room: their provider's alternative, as a switch of its own that
/// goes back with them. None when it starts on `model` as it is.
pub(crate) fn start_on(factory: &Factory, inner: &Inner, model: &str, effort: &str) -> Option<(String, String)> {
    let now = crate::transcript::now_ms();
    let provider_id = provider_of(model);
    let away = factory.usage_switches.kept().values().any(|kept| provider_of(&kept.before.model) == provider_id);
    let provider = inner.providers.iter().find(|known| known.id == provider_id)?;
    if !away || room_to_go_back(inner, provider, pool(model), now) {
        return None;
    }
    let alternative = alternative(&inner.usage, &inner.providers, &inner.models, model, effort, now)?;
    Some((alternative.model, alternative.effort))
}

/// Keeps a new agent or chat that started on another provider (`start_on`)
/// as switched from `model` and `effort`.
pub(crate) fn started_switched(factory: &Factory, id: &str, model: &str, effort: &str) {
    tracing::info!(agent = %id, from = %model, "started on another provider while its own has no room");
    factory.usage_switches.set(&factory.store, id, Before { model: model.to_owned(), effort: effort.to_owned() }, 0);
}

/// SetUsageAutoContinue.
pub(crate) fn set_auto_continue(factory: &Arc<Factory>, on: bool) -> Result<(), RequestError> {
    factory.store.db().execute("INSERT OR REPLACE INTO usage_settings (name, value) VALUES ('auto_continue', ?1)", [i64::from(on)])?;
    factory.usage_switches.auto_continue.store(on, std::sync::atomic::Ordering::Relaxed);
    tracing::info!(on, "continue automatically on another provider");
    factory.broadcast_workspace(&factory.lock());
    // Those waiting now continue, as their waits go on.
    factory.capacity_changed();
    Ok(())
}

/// With Continue automatically on, an agent or chat
/// waiting on a usage limit continues on its alternative by itself, if it
/// has one, and its conversation says so. True when it switched.
pub(crate) async fn continue_by_itself(factory: &Arc<Factory>, id: &str) -> bool {
    if !factory.usage_switches.auto_continue() {
        return false;
    }
    let said = {
        let inner = factory.lock();
        let Some((model, _)) = model_and_effort(factory, &inner, id) else { return false };
        let Some(alternative) = alternative(&inner.usage, &inner.providers, &inner.models, &model, "", crate::transcript::now_ms()) else {
            return false;
        };
        let provider_id = provider_of(&model);
        let name = inner.providers.iter().find(|known| known.id == provider_id).map_or(provider_id, |known| known.name.as_str());
        let noun = factory_agent::accounts::account_noun(provider_id, name);
        format!("Continued on {} automatically: your {noun} usage reached its limit.", alternative.provider_name)
    };
    if let Err(error) = continue_on(factory, id, false, 0, None).await {
        tracing::warn!(agent = %id, ?error, "could not continue on another provider by itself");
        return false;
    }
    tracing::info!(agent = %id, "continued on another provider by itself");
    let mut inner = factory.lock();
    if let Some(transcript) = inner.agents.get_mut(id).and_then(|runtime| runtime.transcript.as_mut()) {
        let entry = transcript.notice(said, v1::NoticeLevel::Info);
        factory.send_change(&inner, id, crate::transcript::Change::Entry(entry));
    }
    true
}

/// Every switch whose provider has room again goes back
/// by itself, unless it's held (`HOLD`). The model changes from the agent's
/// next step, as the person's change would; its conversation says so. Run
/// on every usage check, which follows usage reads and resets.
pub(crate) async fn come_back(factory: &Arc<Factory>) {
    if factory.usage_switches.only_by_tap.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let now = crate::transcript::now_ms();
    let backs: Vec<(String, Before, String, String)> = {
        let inner = factory.lock();
        let kept: Vec<(String, Kept)> = factory.usage_switches.kept().iter().map(|(id, kept)| (id.clone(), kept.clone())).collect();
        kept.into_iter()
            .filter(|(id, kept)| kept.hold_until <= now && model_and_effort(factory, &inner, id).is_some())
            .filter_map(|(id, Kept { before, .. })| {
                let provider_id = provider_of(&before.model);
                let provider = inner.providers.iter().find(|known| known.id == provider_id)?;
                let listed = inner.models.iter().find(|known| known.id == before.model)?;
                if !room_to_go_back(&inner, provider, pool(&before.model), now) {
                    return None;
                }
                let noun = factory_agent::accounts::account_noun(provider_id, &provider.name);
                let notice = format!("Back on {}: your {noun} usage has room again.", shown_name(&listed.name));
                let why = format!("Claktory switched you back by itself, since your {noun} usage has room again");
                Some((id, before, notice, why))
            })
            .collect()
    };
    if backs.is_empty() {
        return;
    }
    for (id, before, notice, why) in backs {
        tracing::info!(agent = %id, to = %before.model, "going back by itself: its provider has room again");
        if let Err(error) = run_on(factory, &id, &before.model, &before.effort, &why).await {
            tracing::warn!(agent = %id, ?error, "could not go back to the provider it switched from");
            continue;
        }
        factory.usage_switches.forget(&factory.store, &id);
        factory.usage_switches.came_back().insert(id.clone(), now);
        if let Err(error) = factory.ensure_loaded(&id).await {
            tracing::warn!(agent = %id, %error, "could not say it went back");
            continue;
        }
        let mut inner = factory.lock();
        if let Some(transcript) = inner.agents.get_mut(&id).and_then(|runtime| runtime.transcript.as_mut()) {
            let entry = transcript.notice(notice, v1::NoticeLevel::Info);
            factory.send_change(&inner, &id, crate::transcript::Change::Entry(entry));
        }
    }
    factory.broadcast_workspace(&factory.lock());
    factory.capacity_changed();
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
        super::alternative(&Usages::new(), &known.providers, &known.models, model, effort, now)
    }

    #[test]
    fn the_same_model_matches_across_providers_whatever_its_spelling() {
        assert_eq!(kind("anthropic/claude-opus-4-5-20251101"), kind("openrouter/anthropic/claude-opus-4.5"));
        assert_eq!(kind("openai-codex/gpt-5.6-sol"), kind("openrouter/openai/gpt-5.6-sol"));
        assert_eq!(kind("openai-codex/gpt-5.6-sol"), kind("openai/gpt-5.6-sol"));
        assert_eq!(kind("grok/grok-4.7"), kind("openrouter/x-ai/grok-4.7"));
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

    /// U11: a model's own weekly limit at 100% leaves the provider without
    /// room for that model's pool only. The read is kept apart from the
    /// listing, whose account has only an empty placeholder (models.rs), as
    /// in a running Claktory.
    #[test]
    fn a_full_opus_limit_leaves_room_for_the_other_models() {
        let window = |id: &str, used_percent: f64| v1::UsageWindow { id: id.into(), used_percent, ..Default::default() };
        let usage = v1::ProviderUsage { windows: vec![window("five_hour", 3.0), window("weekly_opus", 100.0)], ..Default::default() };
        let provider = v1::Provider {
            id: "anthropic".into(),
            signed_in_with: "oauth".into(),
            accounts: vec![v1::Account { id: "a".into(), usage: Some(v1::ProviderUsage::default()), ..Default::default() }],
            ..Default::default()
        };
        let usages = Usages::from([(("anthropic".to_owned(), "a".to_owned()), usage)]);
        assert_eq!(pool("anthropic/claude-opus-4-8"), Pool::Opus);
        assert_eq!(pool("anthropic/claude-sonnet-4-6"), Pool::Sonnet);
        assert_eq!(pool("openrouter/anthropic/claude-opus-4.8"), Pool::Normal);
        assert!(has_room(&Usages::new(), &provider, Pool::Opus, 0));
        assert!(!has_room(&usages, &provider, Pool::Opus, 0));
        for room in [Pool::Normal, Pool::Sonnet, Pool::Fable] {
            assert!(has_room(&usages, &provider, room, 0), "{room:?}");
        }
        assert!(limits(&window("weekly_opus", 100.0), Pool::Opus) && !limits(&window("weekly_opus", 100.0), Pool::Sonnet));
        assert!(!limits(&window("weekly_oauth_apps", 100.0), Pool::Fable) && limits(&window("weekly", 100.0), Pool::Fable));
    }
}
