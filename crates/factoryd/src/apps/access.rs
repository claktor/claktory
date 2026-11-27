//! Rivet sets an app up (#333): what one of the installed apps may read and
//! which agents may use it, for what the person wants (charted, say). It
//! proposes, with `apps_propose_access` (the App grants extension); the
//! person decides on the card it opens in Rivet's conversation
//! (`v1::AppAccessProposal`, DecideAppAccess). Installing an app gives it
//! nothing, and only the person's approval gives anything (Fugo's review):
//!
//! - Only Rivet proposes, never after reading a pipe, and never all of
//!   Claktory's records (only the sources that say what they hold), a
//!   curator's database, or an agent limited to a pipe.
//! - The card shows exactly what an approval gives, and its digest binds
//!   the approval to it: the app's id, version and publisher key, the
//!   sources, and each agent (as the incarnation it was) with its scopes
//!   and whether it has read a web page. The app sends the digest back; any
//!   other is refused.
//! - At approval everything is checked again: the app still installed at
//!   that version from that publisher, each source still one it asks for,
//!   each agent the same agent, none limited to a pipe. One that changed
//!   fails the card ("nothing was given"); an agent that read a web page since
//!   changes the card (and its digest), for the person to look at again.
//! - Approved, the sources are added to what the app reads and each agent
//!   gets its scopes in place of those it had; nothing else changes.

use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use factory_agent::ToolResult;
use factory_protocol::v1::{self, AppGrantState as State};
use prost::Message;
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use super::changes::{self, Access, Held, How, Via};
use crate::factory::{Factory, RequestError, host_event};
use crate::store::Store;
use crate::transcript::now_ms;

pub(crate) const SCHEMA: &str = "
-- Rivet's proposals of what an installed app may read and which agents may
-- use it (apps_propose_access): the card in its conversation.
CREATE TABLE app_access_proposals (
    id TEXT PRIMARY KEY,
    -- Rivet, whose conversation shows it.
    agent TEXT NOT NULL,
    app TEXT NOT NULL,
    -- The app as proposed: its version and publisher key.
    version TEXT NOT NULL,
    key TEXT NOT NULL,
    -- JSON: {\"<agent id>\": \"<incarnation>\"}, each agent as it was.
    incarnations TEXT NOT NULL,
    -- The card as shown (v1.AppAccessProposal, protobuf), pending.
    card BLOB NOT NULL,
    digest TEXT NOT NULL,
    -- pending, granted, declined, failed, or cleared (/clear closed it
    -- unseen; shown as declined)
    state TEXT NOT NULL,
    created INTEGER NOT NULL,
    decided INTEGER,
    comment TEXT NOT NULL DEFAULT '',
    error TEXT NOT NULL DEFAULT ''
);
CREATE INDEX app_access_proposals_by_agent ON app_access_proposals (agent, created);
";

/// The migration that added replies (#370 Submit): 1 when the person
/// answered the card with a comment instead (state 'declined': nothing was
/// given).
pub(crate) const REPLIED: &str = "
ALTER TABLE app_access_proposals ADD COLUMN replied INTEGER NOT NULL DEFAULT 0;
";

/// The tool whose result showed a card, before apps_set_access.
pub(crate) const TOOL: &str = "apps_propose_access";
/// Rivet's tools: what the apps are, and changing what they read and who
/// uses them.
pub(crate) const LIST_TOOL: &str = "apps_list";
pub(crate) const SET_TOOL: &str = "apps_set_access";

pub(crate) const LIST_DESCRIPTION: &str = "The person's installed apps (such as Charts), or one (app, its id): what each can read (sources, each on or off, and whether it's private), whether it reaches the internet, its scopes and the tools they give, its events, and what each agent has of it. Look here before apps_set_access, and tell the person first when what they want needs their approval in a request.";

pub(crate) const SET_DESCRIPTION: &str = "Set up one of the person's installed apps for what they asked (\"have Ingrid keep a chart of her bugs by severity\"): app, its id; sources, each source id turned on (true) or off (false), those left out staying as they are (activity has no message text: prefer it; an app that reads files takes a CSV or TSV as file:<place>:<path>, such as file:web-site:reports/bugs.csv); agents, each {agent, its id or name; scopes, the scopes it has from now on, [] to take the app away; hears, the app's events it hears, each needing its scope, left out to keep those it hears now}; reason, why, in a sentence for the person. Only what you name changes. Taking anything away is applied at once. Giving is applied at once too when it's not private data, to an app that reaches no site, for agents and by you that haven't read a web page; otherwise it goes in a request in your conversation for the person to approve, and the answer says why. Every change shows in the person's History, where they can undo it. Use this, never config.json, to change what apps read and who uses them. proposal: the id of an approved proposal whose grants name it, once you've reviewed it: then it's made without a request when it's within one of them; otherwise the request opens as usual and the answer says why.";

/// The longest reason a card shows, and the most it may name.
const MAX_REASON: usize = 500;
const MAX_SOURCES: usize = 32;
const MAX_AGENTS: usize = 16;
/// Cards Rivet may have waiting at once.
const MAX_PENDING: i64 = 5;

/// A proposal as kept.
struct Row {
    id: String,
    agent: String,
    app: String,
    version: String,
    key: String,
    incarnations: std::collections::BTreeMap<String, String>,
    card: v1::AppAccessProposal,
    state: String,
    decided: Option<i64>,
    comment: String,
    error: String,
    /// Declined by a reply (#370): the comment is what to do instead.
    replied: bool,
}

const COLUMNS: &str = "id, agent, app, version, key, incarnations, card, digest, state, created, decided, comment, error, replied";

impl Row {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
        let card: Vec<u8> = row.get(6)?;
        let incarnations: String = row.get(5)?;
        Ok(Row {
            id: row.get(0)?,
            agent: row.get(1)?,
            app: row.get(2)?,
            version: row.get(3)?,
            key: row.get(4)?,
            incarnations: serde_json::from_str(&incarnations).unwrap_or_default(),
            card: v1::AppAccessProposal::decode(card.as_slice()).unwrap_or_default(),
            state: row.get(8)?,
            decided: row.get(10)?,
            comment: row.get(11)?,
            error: row.get(12)?,
            replied: row.get(13)?,
        })
    }

    fn state(&self) -> State {
        match self.state.as_str() {
            "pending" => State::Pending,
            "granted" => State::Granted,
            "failed" => State::Failed,
            _ => State::Declined,
        }
    }

    /// The card as it is now.
    fn to_proto(&self) -> v1::AppAccessProposal {
        v1::AppAccessProposal {
            state: self.state().into(),
            decided_ms: self.decided.unwrap_or_default(),
            comment: self.comment.clone(),
            error: self.error.clone(),
            replied: self.replied,
            ..self.card.clone()
        }
    }
}

fn row(store: &Store, id: &str) -> Option<Row> {
    store.db().query_row(&format!("SELECT {COLUMNS} FROM app_access_proposals WHERE id = ?1"), [id], Row::read).optional().ok().flatten()
}

/// What an approval binds to: the app as proposed, the sources, and each
/// agent (the incarnation it was) with its scopes and whether it had read a
/// pipe; hex SHA-256.
fn digest(card: &v1::AppAccessProposal, version: &str, key: &str, incarnations: &std::collections::BTreeMap<String, String>) -> String {
    let sources: Vec<&str> = card.sources.iter().map(|source| source.id.as_str()).collect();
    let agents: Vec<serde_json::Value> = card
        .agents
        .iter()
        .map(|grant| {
            json!({
                "agent": grant.agent_id,
                "incarnation": incarnations.get(&grant.agent_id),
                "scopes": grant.scopes,
                "read_pipes": card.agents_read_pipes.contains(&grant.agent_id),
                "reading_pipes": card.agents_reading_pipes.contains(&grant.agent_id),
            })
        })
        .collect();
    let mut bound =
        json!({"v": 1, "id": card.id, "app": card.app_id, "version": version, "key": key, "sources": sources, "agents": agents});
    // What each agent would hear: on cards from apps_set_access.
    if !card.card_rules.is_empty() || !card.subscriptions.is_empty() {
        let hears: Vec<serde_json::Value> = card
            .subscriptions
            .iter()
            .map(|subscription| json!({"kind": subscription.event_kind, "agents": subscription.agent_ids}))
            .collect();
        bound["hears"] = json!(hears);
        bound["rules"] = json!(card.card_rules);
    }
    let hash = Sha256::digest(bound.to_string().as_bytes());
    crate::encoding::hex(&hash)
}

/// The agent named, by id or else by name, of this Claktory's agents.
fn named(factory: &Factory, wanted: &str) -> Result<(String, String), String> {
    let inner = factory.lock();
    let wanted = wanted.trim();
    if crate::chats::is_chat(wanted) {
        return Err("Only an agent can use an app, not a chat.".to_owned());
    }
    if let Some(agent) = inner.config.agent(wanted) {
        return Ok((agent.id.clone(), agent.name.clone()));
    }
    let matching: Vec<_> = inner.config.agents.iter().filter(|agent| agent.name.eq_ignore_ascii_case(wanted)).collect();
    match matching.as_slice() {
        [agent] => Ok((agent.id.clone(), agent.name.clone())),
        [] => Err(format!("There's no agent \"{wanted}\" in this Claktory.")),
        several => {
            let ids: Vec<&str> = several.iter().map(|agent| agent.id.as_str()).collect();
            Err(format!("More than one agent is called {wanted}: name one by its id ({}).", ids.join(", ")))
        }
    }
}

/// What apps_set_access asks for an agent: its scopes, in place of those
/// it has ([] takes the app away), and the events it hears (left out: those
/// it hears now that it keeps the scope for).
#[derive(Deserialize)]
struct SetAgent {
    agent: String,
    #[serde(default)]
    scopes: Vec<String>,
    #[serde(default)]
    hears: Option<Vec<String>>,
}

#[derive(Deserialize)]
pub(crate) struct SetArgs {
    app: String,
    /// Each source turned on (true) or off; those left out stay as they are.
    #[serde(default)]
    sources: std::collections::BTreeMap<String, bool>,
    #[serde(default)]
    agents: Vec<SetAgent>,
    reason: String,
    /// An approved proposal that names what it gives
    /// (`crate::proposal_grants`): no card then, when it's covered.
    #[serde(default)]
    proposal: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct ListArgs {
    #[serde(default)]
    app: Option<String>,
}

/// An installed app, its manifest, and the person's name; or why not.
fn installed(factory: &Factory, wanted: &str, nothing: &str) -> Result<(super::AppConfig, Arc<super::bundle::Manifest>), String> {
    let wanted = wanted.trim();
    let app = factory.lock().config.apps.iter().find(|app| app.id == wanted).cloned();
    let Some(app) = app.filter(|_| factory.apps.available()) else {
        return Err(format!("There's no app \"{wanted}\" installed. {nothing}"));
    };
    let Some(manifest) = factory.apps.manifest(&app.id) else {
        return Err(format!("{wanted} can't be read right now. {nothing}"));
    };
    Ok((app, manifest))
}

/// Whether `agent` carries a stranger's words: it read a web page through
/// browser control (#346: reading a pipe, or others' words through an app
/// or Claktory's records, marks no one).
fn tainted(factory: &Factory, agent: &str) -> bool {
    !factory.pipes.tainted(agent).is_empty()
}

/// apps_set_access, for Rivet (the caller checks it's Rivet): what the
/// change takes away is applied at once; what it gives, at once too when
/// none of Fugo's rules needs the person (`card_rules`), and otherwise on
/// one card for all of it. Kept as changes (`super::changes`).
pub(crate) async fn set_access(factory: &Arc<Factory>, rivet: &str, args: SetArgs) -> Result<ToolResult, String> {
    let nothing = "Nothing was changed.";
    let (app, manifest) = installed(factory, &args.app, nothing)?;
    let person = factory.lock().config.person().to_owned();
    let reason = args.reason.trim().to_owned();
    if reason.is_empty() || reason.chars().count() > MAX_REASON {
        return Err(format!("Say why in 1 to {MAX_REASON} characters. {nothing}"));
    }
    if args.sources.len() > MAX_SOURCES || args.agents.len() > MAX_AGENTS {
        return Err(format!("Name {MAX_SOURCES} sources and {MAX_AGENTS} agents at most. {nothing}"));
    }
    if args.sources.is_empty() && args.agents.is_empty() {
        return Err(format!("Name the sources to turn on or off, the agents to give or take it from, or both. {nothing}"));
    }
    let to = wanted(factory, &app, &manifest, &args.sources, &args.agents, nothing)?;
    let before = changes::now(&app, &to);
    if before == to {
        return Ok(ToolResult::text(format!("Nothing to change: {} already has that.", manifest.name), json!({})));
    }
    let (narrow, widen) = split(&manifest, &before, &to);
    let rules = if widen.is_empty() { Vec::new() } else { card_rules(factory, rivet, &manifest, &widen) };
    let names = changes::agent_names(factory);
    let how = How { by: rivet, via: Via::Rivet, reference: "", reason: &reason, rules: &[] };
    if rules.is_empty() {
        let id = changes::apply(factory, &app.id, &to, how).await.map_err(|error| format!("{error} {nothing}"))?;
        let done = id.and_then(|id| changes::find(factory, id));
        let text = match &done {
            Some(change) => format!(
                "Done: {}. It shows in {person}'s History and in {} settings, where they can undo it.",
                yours(change, &names),
                changes::possessive(&manifest.name)
            ),
            None => format!("Nothing to change: {} already has that.", manifest.name),
        };
        return Ok(ToolResult::text(text, json!({"app_change": id.map(|id| id.to_string())})));
    }
    // An approved proposal that names it: made now, with no card.
    let mut text = String::new();
    if let Some(proposal) = args.proposal.as_deref().map(str::trim).filter(|proposal| !proposal.is_empty()) {
        let fits = |grant: &crate::proposal_grants::Grant| crate::proposal_grants::fits_app(factory, grant, &app, &widen);
        match crate::proposal_grants::claim(factory, rivet, proposal, fits) {
            Ok(claimed) => {
                let reference = format!("proposal:{}", claimed.reference());
                let how = How { by: rivet, via: Via::Rivet, reference: &reference, reason: &reason, rules: &[] };
                let id = match changes::apply(factory, &app.id, &to, how).await {
                    Ok(id) => id,
                    Err(error) => {
                        crate::proposal_grants::unclaim(factory, &claimed);
                        return Err(format!("{error} {nothing}"));
                    }
                };
                crate::proposals::refresh(factory, claimed.proposal);
                let done = id.and_then(|id| changes::find(factory, id));
                let text = match &done {
                    Some(change) => format!(
                        "Done: {}. {} They can undo it there and in {} settings.",
                        yours(change, &names),
                        claimed.said(&person),
                        changes::possessive(&manifest.name)
                    ),
                    None => format!("Nothing to change: {} already has that.", manifest.name),
                };
                return Ok(ToolResult::text(
                    text,
                    json!({"app_change": id.map(|id| id.to_string()), "proposal_grant": claimed.reference()}),
                ));
            }
            Err(why) => text.push_str(&format!("Proposal {proposal} doesn't cover it: {why}. ")),
        }
    }
    // What it takes away goes now; what it gives waits for the person.
    let mut details = json!({});
    if !narrow.is_empty() {
        let id = changes::apply(factory, &app.id, &narrow, how).await.map_err(|error| format!("{error} {nothing}"))?;
        if let Some(change) = id.and_then(|id| changes::find(factory, id)) {
            text.push_str(&format!("Done: {}. ", yours(&change, &names)));
            details["app_change"] = json!(change.id.to_string());
        }
    }
    let app = factory.lock().config.apps.iter().find(|installed| installed.id == app.id).cloned().unwrap_or(app);
    let card = file(factory, rivet, &app, &manifest, &widen, &reason, &rules)?;
    text.push_str(&format!(
        "What it gives needs {person}'s approval, so it's in a request in your conversation: {} You'll be told what they decide.",
        rules.join(" ")
    ));
    details["app_access"] = json!(STANDARD.encode(card.encode_to_vec()));
    Ok(ToolResult::text(text, details))
}

/// apps_set_access as a proposal's grant (`crate::proposal_grants`): what
/// it gives, checked as the tool would, for Rivet (`rivet`); taking away
/// needs no approval, so a grant only gives.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantArgs {
    app: String,
    #[serde(default)]
    sources: std::collections::BTreeMap<String, bool>,
    #[serde(default)]
    agents: Vec<SetAgent>,
}

pub(crate) fn resolve_grant(
    factory: &Arc<Factory>,
    rivet: &str,
    args: serde_json::Value,
) -> Result<crate::proposal_grants::Resolved, String> {
    let args: GrantArgs = serde_json::from_value(args).map_err(|error| error.to_string())?;
    let nothing = "Nothing was proposed.";
    let (app, manifest) = installed(factory, &args.app, nothing)?;
    if args.sources.values().any(|on| !on) || args.agents.iter().any(|agent| agent.scopes.is_empty()) {
        return Err(format!("A grant only gives: taking away needs no approval, so leave it out. {nothing}"));
    }
    if args.sources.is_empty() && args.agents.is_empty() {
        return Err(format!("Name the sources it reads, the agents that use it, or both. {nothing}"));
    }
    if args.sources.len() > MAX_SOURCES || args.agents.len() > MAX_AGENTS {
        return Err(format!("Name {MAX_SOURCES} sources and {MAX_AGENTS} agents at most. {nothing}"));
    }
    let to = wanted(factory, &app, &manifest, &args.sources, &args.agents, nothing)?;
    let before = changes::now(&app, &to);
    let (_, widen) = split(&manifest, &before, &to);
    if widen.is_empty() {
        return Err(format!("{} already has that, so there's nothing to give. {nothing}", manifest.name));
    }
    let mut incarnations = std::collections::BTreeMap::new();
    for agent in widen.agents.keys() {
        let incarnation = crate::user_apps::incarnation(&factory.home, agent).map_err(|error| error.to_string())?;
        incarnations.insert(agent.clone(), incarnation);
    }
    let labels = {
        let inner = factory.lock();
        changes::Labels::of(&inner.config, &manifest, &app)
    };
    // History's sentence, as something to do.
    let change = changes::Change {
        id: 0,
        app: app.id.clone(),
        by: rivet.to_owned(),
        via: Via::Rivet,
        reference: String::new(),
        reason: String::new(),
        before: changes::now(&app, &widen),
        after: widen.clone(),
        labels,
        at: 0,
        undone_by: None,
    };
    let summary = change.asked(&changes::agent_names(factory));
    let warnings = card_rules(factory, rivet, &manifest, &widen);
    Ok(crate::proposal_grants::Resolved {
        grant: crate::proposal_grants::Grant::App {
            app: app.id.clone(),
            version: app.version.clone(),
            key: app.key.clone(),
            access: widen,
            incarnations,
        },
        summary,
        warnings,
    })
}

/// What a call asks the app to be: each source on or off, each agent's
/// scopes and events, checked against what the app has; or why not, ending
/// with `nothing`.
fn wanted(
    factory: &Arc<Factory>,
    app: &super::AppConfig,
    manifest: &super::bundle::Manifest,
    sources: &std::collections::BTreeMap<String, bool>,
    agents: &[SetAgent],
    nothing: &str,
) -> Result<Access, String> {
    let person = factory.lock().config.person().to_owned();
    let offered = super::data::proposable(factory, manifest);
    let mut to = Access::default();
    for (id, on) in sources {
        let id = id.trim();
        let had = app.sources.iter().any(|source| source == id);
        // A file: one an app may read now, or why not.
        if *on && !had && super::files::is_file(id) {
            super::data::file_offer(factory, manifest, id).map_err(|why| format!("{why} {nothing}"))?;
        } else if *on && !had && !offered.iter().any(|source| source.id == id) {
            if id == super::data::FACTORY_SOURCE {
                return Err(format!(
                    "Everything Claktory keeps (factory-records) can't be given: name activity, conversations or an agent's database. {nothing}"
                ));
            }
            let known: Vec<&str> = offered.iter().map(|source| source.id.as_str()).collect();
            return Err(format!(
                "{} doesn't read \"{id}\" (it can be given: {}). {nothing}",
                manifest.name,
                if known.is_empty() { "nothing".to_owned() } else { known.join(", ") }
            ));
        }
        if !*on && !had && !offered.iter().any(|source| source.id == id) {
            return Err(format!("{} doesn't read \"{id}\". {nothing}", manifest.name));
        }
        to.sources.insert(id.to_owned(), *on);
    }
    for asked in agents {
        let (id, name) = named(factory, &asked.agent).map_err(|why| format!("{why} {nothing}"))?;
        if to.agents.contains_key(&id) {
            return Err(format!("{name} is there twice: name each agent once, with all its scopes. {nothing}"));
        }
        let mut scopes: Vec<String> = Vec::new();
        for scope in &asked.scopes {
            let scope = scope.trim();
            if manifest.scope(scope).is_none() {
                let known: Vec<&str> = manifest.scopes.iter().map(|scope| scope.id.as_str()).collect();
                return Err(format!("{} has no scope \"{scope}\": its scopes are {}. {nothing}", manifest.name, known.join(", ")));
            }
            if !scopes.iter().any(|kept| kept == scope) {
                scopes.push(scope.to_owned());
            }
        }
        scopes.sort_by_key(|scope| manifest.scopes.iter().position(|spec| &spec.id == scope));
        if !scopes.is_empty() && factory.pipes.contained(&id) {
            return Err(format!("{name} is limited to a pipe, so it can't use {person}'s apps. {nothing}"));
        }
        let hears: Vec<String> = match &asked.hears {
            Some(hears) => {
                let mut kept: Vec<String> = Vec::new();
                for kind in hears {
                    let kind = kind.trim();
                    let Some(event) = manifest.event(kind) else {
                        let known: Vec<&str> = manifest.events.iter().map(|event| event.kind.as_str()).collect();
                        return Err(format!(
                            "{} has no event \"{kind}\" (its events: {}). {nothing}",
                            manifest.name,
                            if known.is_empty() { "none".to_owned() } else { known.join(", ") }
                        ));
                    };
                    if !scopes.contains(&event.scope) {
                        return Err(format!("{name} hears \"{kind}\" only with the scope \"{}\". {nothing}", event.scope));
                    }
                    if !kept.iter().any(|known| known == kind) {
                        kept.push(kind.to_owned());
                    }
                }
                kept
            }
            None => changes::held(app, &id)
                .hears
                .into_iter()
                .filter(|kind| manifest.event(kind).is_some_and(|event| scopes.contains(&event.scope)))
                .collect(),
        };
        let mut hears = hears;
        hears.sort();
        to.agents.insert(id, Held { scopes, hears });
    }
    Ok(to)
}

/// What going from `before` to `to` takes away, and what it gives.
fn split(manifest: &super::bundle::Manifest, before: &Access, to: &Access) -> (Access, Access) {
    let mut narrow = Access::default();
    let mut widen = Access::default();
    for (id, on) in &to.sources {
        match (before.sources.get(id).copied().unwrap_or(false), *on) {
            (true, false) => {
                narrow.sources.insert(id.clone(), false);
            }
            (false, true) => {
                widen.sources.insert(id.clone(), true);
            }
            _ => {}
        }
    }
    for (agent, wanted) in &to.agents {
        let had = before.agents.get(agent).cloned().unwrap_or_default();
        let scopes: Vec<String> = had.scopes.iter().filter(|scope| wanted.scopes.contains(scope)).cloned().collect();
        let hears: Vec<String> = had
            .hears
            .iter()
            .filter(|kind| wanted.hears.contains(kind) && manifest.event(kind).is_some_and(|event| scopes.contains(&event.scope)))
            .cloned()
            .collect();
        let kept = Held { scopes, hears };
        if kept != had {
            narrow.agents.insert(agent.clone(), kept.clone());
        }
        if kept != *wanted {
            widen.agents.insert(agent.clone(), wanted.clone());
        }
    }
    (narrow, widen)
}

/// A change's sentence as told to Rivet, who made it: "you gave Ingrid…".
fn yours(change: &changes::Change, names: &std::collections::BTreeMap<String, String>) -> String {
    let mut told = change.clone();
    told.by = String::new();
    let title = told.title(names);
    title.strip_prefix("You ").map_or(title.clone(), |rest| format!("you {rest}"))
}

/// Why giving `widen` (after `narrow`) needs the person's approval, each
/// one of Claktory's sentences; none when Rivet may apply it (Fugo's
/// ruling, #339). Judged from the manifest, the sources and the agents as
/// they are now, never from what Rivet says.
fn card_rules(factory: &Factory, rivet: &str, manifest: &super::bundle::Manifest, widen: &Access) -> Vec<String> {
    let name = &manifest.name;
    let names = changes::agent_names(factory);
    let agent_name = |id: &str| names.get(id).cloned().unwrap_or_else(|| id.to_owned());
    let mut rules = Vec::new();
    // 4: Rivet has read a stranger's words (a web page, #346).
    if tainted(factory, rivet) {
        rules.push(format!(
            "{} has read {}, which may hold someone else's instructions, so what it gives needs your approval.",
            agent_name(rivet),
            crate::pipes::agents::what_was_read(factory, rivet)
        ));
    }
    // 1: private data.
    for id in widen.sources.keys() {
        let source = super::data::offer(factory, manifest, id);
        let source = source.as_ref();
        let label = source.map_or(id.as_str(), |source| source.label.as_str());
        if source.is_some_and(|source| source.private) || super::data::holds_others_words(id) {
            rules.push(format!("{label} holds what people wrote, so letting {name} read it needs your approval."));
        }
    }
    // 2: the internet.
    if !manifest.hosts.is_empty() {
        rules.push(format!(
            "{name} can reach {} on the internet, so giving it data or agents needs your approval.",
            changes::and(&manifest.hosts)
        ));
    }
    // 3: an agent given it has read a stranger's words (a web page, #346).
    for agent in widen.agents.keys() {
        if tainted(factory, agent) {
            rules.push(format!(
                "{} has read {}, so giving it {name} needs your approval.",
                agent_name(agent),
                crate::pipes::agents::what_was_read(factory, agent)
            ));
        }
    }
    rules.dedup();
    rules
}

/// Keeps a card for the person to approve `widen` (Rivet's apps_set_access
/// when a rule needs them): the sources it would add, and each agent with
/// the scopes and events it would have.
fn file(
    factory: &Arc<Factory>,
    rivet: &str,
    app: &super::AppConfig,
    manifest: &super::bundle::Manifest,
    widen: &Access,
    reason: &str,
    rules: &[String],
) -> Result<v1::AppAccessProposal, String> {
    let sources: Vec<v1::AppDataSource> = widen.sources.keys().filter_map(|id| super::data::offer(factory, manifest, id)).collect();
    let mut agents: Vec<v1::AppGrant> = Vec::new();
    let mut incarnations = std::collections::BTreeMap::new();
    let mut read_pipes = Vec::new();
    let mut hearing: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
    for (id, held) in &widen.agents {
        let incarnation = crate::user_apps::incarnation(&factory.home, id).map_err(|error| format!("Couldn't ask for it: {error}"))?;
        incarnations.insert(id.clone(), incarnation);
        if tainted(factory, id) {
            read_pipes.push(id.clone());
        }
        for kind in &held.hears {
            hearing.entry(kind.clone()).or_default().push(id.clone());
        }
        agents.push(v1::AppGrant { agent_id: id.clone(), scopes: held.scopes.clone(), given: None });
    }
    let mut card = v1::AppAccessProposal {
        id: format!("x{}", crate::encoding::hex(&crate::vault::crypto::random::<6>())),
        app_id: app.id.clone(),
        app_name: manifest.name.clone(),
        sources,
        agents,
        reason: reason.to_owned(),
        state: State::Pending.into(),
        created_ms: now_ms(),
        agents_read_pipes: read_pipes,
        publisher: manifest.publisher.clone(),
        tier: super::tier(super::bundle::tier_of(&app.key)).into(),
        hosts: manifest.hosts.clone(),
        subscriptions: hearing.into_iter().map(|(event_kind, agent_ids)| v1::AppSubscription { event_kind, agent_ids }).collect(),
        card_rules: rules.to_vec(),
        ..Default::default()
    };
    card.digest = digest(&card, &app.version, &app.key, &incarnations);
    let db = factory.store.db();
    let waiting: i64 = db
        .query_row("SELECT COUNT(*) FROM app_access_proposals WHERE agent = ?1 AND state = 'pending'", [rivet], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if waiting >= MAX_PENDING {
        return Err(format!(
            "{waiting} of your requests already wait for the person. Wait for them to decide before asking for more; what it takes away was applied."
        ));
    }
    db.execute(
        &format!("INSERT INTO app_access_proposals ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', ?9, NULL, '', '', 0)"),
        params![
            card.id,
            rivet,
            app.id,
            app.version,
            app.key,
            serde_json::to_string(&incarnations).unwrap_or_default(),
            card.encode_to_vec(),
            card.digest,
            card.created_ms,
        ],
    )
    .map_err(|error| format!("Couldn't keep the request: {error}"))?;
    Ok(card)
}

/// apps_list: the installed apps (or one), as Claktory judges them: what
/// each can read and whether it's private, whether it reaches the
/// internet, its scopes and events, and what each agent holds; with
/// whether Rivet's own giving would need a card.
pub(crate) fn list(factory: &Arc<Factory>, rivet: &str, args: ListArgs) -> Result<ToolResult, String> {
    let wanted = args.app.as_deref().map(str::trim).filter(|app| !app.is_empty());
    let apps: Vec<super::AppConfig> =
        factory.lock().config.apps.iter().filter(|app| wanted.is_none_or(|wanted| app.id == wanted)).cloned().collect();
    if !factory.apps.available() || apps.is_empty() {
        return Ok(ToolResult::text(
            match wanted {
                Some(wanted) => format!("There's no app \"{wanted}\" installed."),
                None => "No apps are installed.".to_owned(),
            },
            json!({"apps": []}),
        ));
    }
    let names = changes::agent_names(factory);
    let name_of = |id: &str| names.get(id).cloned().unwrap_or_else(|| id.to_owned());
    let mut text = String::new();
    let mut listed = Vec::new();
    if tainted(factory, rivet) {
        text.push_str(&format!(
            "You have read {}, so anything you give with apps_set_access goes in a request for the person; taking away is applied at once.\n\n",
            crate::pipes::agents::what_was_read(factory, rivet)
        ));
    }
    for app in &apps {
        let Some(manifest) = factory.apps.manifest(&app.id) else {
            text.push_str(&format!("{} {} ({}): can't be read right now.\n\n", app.id, app.version, app.id));
            continue;
        };
        let (sources_given, agents_given) = changes::given(factory, app);
        let config = factory.lock().config.clone();
        let mut sources: Vec<v1::AppDataSource> = super::data::infos(&config, &manifest, app);
        // Never a curator's database.
        let offered = super::data::proposable(factory, &manifest);
        sources.retain(|source| source.granted || offered.iter().any(|offer| offer.id == source.id));
        let reach = if manifest.hosts.is_empty() {
            "No internet: it reaches no site.".to_owned()
        } else {
            format!("Reaches the internet: {}. Anything you give it needs the person's approval.", manifest.hosts.join(", "))
        };
        text.push_str(&format!("{} {} (app \"{}\"). {reach}\n", manifest.name, app.version, app.id));
        text.push_str("Sources (turn on or off with apps_set_access's sources):\n");
        let mut source_items = Vec::new();
        for source in &sources {
            let others = super::data::holds_others_words(&source.id);
            let state = if source.granted { "on" } else { "off" };
            let kind = if super::files::is_file(&source.id) {
                "private: a file may hold anything, so giving it needs a request"
            } else if others {
                "private, and holds messages from other Claktories: giving it needs a request"
            } else if source.private {
                "private: giving it needs a request"
            } else {
                "not private"
            };
            let given = sources_given.get(&source.id).map(|given| given_line(given, &name_of)).unwrap_or_default();
            text.push_str(&format!("- {}: {} ({state}{given}; {kind}). {}\n", source.id, source.label, source.holds));
            source_items.push(json!({
                "id": source.id, "label": source.label, "holds": source.holds, "private": source.private,
                "others_words": others, "on": source.granted,
            }));
        }
        let reads_files = manifest.data.iter().any(|data| data.kind == super::bundle::FILE);
        if reads_files {
            text.push_str("- Files: it reads CSV or TSV files you name as file:<place>:<path>, the place being workspace, a project's id or <project>/<folder>, the path in its folder (file:web-site:reports/bugs.csv). Each file is private: giving one needs a request.\n");
        }
        text.push_str("Scopes (give agents with apps_set_access's agents):\n");
        let mut scope_items = Vec::new();
        for scope in &manifest.scopes {
            let reads = super::scope_reads(&manifest, &app.key, scope);
            let tools = scope.tools.join(", ");
            let note = if reads { format!(" Reads what {} reads.", manifest.name) } else { String::new() };
            text.push_str(&format!("- {}: {}. Tools: {}.{note}\n", scope.id, scope.label, if tools.is_empty() { "none" } else { &tools }));
            scope_items.push(json!({"id": scope.id, "label": scope.label, "tools": scope.tools, "reads_data": reads}));
        }
        let mut event_items = Vec::new();
        if !manifest.events.is_empty() {
            text.push_str("Events (an agent hears them with hears; each needs its scope):\n");
            for event in &manifest.events {
                text.push_str(&format!("- {}: {} (needs {}).\n", event.kind, event.label, event.scope));
                event_items.push(json!({"kind": event.kind, "label": event.label, "scope": event.scope}));
            }
        }
        let holders: Vec<&String> = app.grants.keys().filter(|agent| config.agent(agent).is_some()).collect();
        let mut agent_items = Vec::new();
        if holders.is_empty() {
            text.push_str("Agents: none use it.\n");
        } else {
            text.push_str("Agents:\n");
            for agent in holders {
                let held = changes::held(app, agent);
                let hears = if held.hears.is_empty() { String::new() } else { format!("; hears {}", held.hears.join(", ")) };
                let given = agents_given.get(agent).map(|given| given_line(given, &name_of)).unwrap_or_default();
                let read = if tainted(factory, agent) {
                    format!(" Has read {}.", crate::pipes::agents::what_was_read(factory, agent))
                } else {
                    String::new()
                };
                text.push_str(&format!("- {} ({agent}): {}{hears}{given}.{read}\n", name_of(agent), held.scopes.join(", ")));
                agent_items.push(json!({"agent": agent, "name": name_of(agent), "scopes": held.scopes, "hears": held.hears, "read_pipes": tainted(factory, agent)}));
            }
        }
        text.push('\n');
        listed.push(json!({
            "id": app.id, "name": manifest.name, "version": app.version, "hosts": manifest.hosts,
            "sources": source_items, "reads_files": reads_files, "scopes": scope_items, "events": event_items, "agents": agent_items,
        }));
    }
    text.push_str("Giving a source that isn't private, or scopes, to an app that reaches no site is applied at once; taking anything away always is. The rest goes in a request: say so to the person first.");
    Ok(ToolResult::text(text, json!({"apps": listed, "you_read_pipes": tainted(factory, rivet)})))
}

/// ", given by Rivet" or ", given by the person".
fn given_line(given: &v1::AppAccessGiven, name_of: &impl Fn(&str) -> String) -> String {
    if given.by_agent_id.is_empty() { ", set by the person".to_owned() } else { format!(", set by {}", name_of(&given.by_agent_id)) }
}

/// Claktory's sentence for a card (AppAccessProposal), for History: "Let
/// Charts read Conversations; let Ingrid use Charts".
pub(crate) fn title(card: &v1::AppAccessProposal, names: &std::collections::BTreeMap<String, String>) -> String {
    let mut parts = Vec::new();
    if !card.sources.is_empty() {
        let labels: Vec<String> = card.sources.iter().map(|source| source.label.clone()).collect();
        parts.push(format!("let {} read {}", card.app_name, changes::and(&labels)));
    }
    if !card.agents.is_empty() {
        let agents: Vec<String> =
            card.agents.iter().map(|grant| names.get(&grant.agent_id).cloned().unwrap_or_else(|| grant.agent_id.clone())).collect();
        parts.push(format!("let {} use {}", changes::and(&agents), card.app_name));
    }
    let text = match parts.as_slice() {
        [first, second] => format!("{first}, and {}", second.replacen(&format!(" use {}", card.app_name), " use it", 1)),
        _ => parts.join("; "),
    };
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => format!("Set up {}", card.app_name),
    }
}

/// The change an apps_set_access call made, from its entry.
pub(crate) fn change_of(call: &v1::ToolCall) -> Option<String> {
    if call.name != SET_TOOL && call.name != "extension_call" {
        return None;
    }
    let details: serde_json::Value = serde_json::from_str(&call.details).ok()?;
    details.get("app_change")?.as_str().map(str::to_owned)
}

/// The card an apps_propose_access result shows (called directly, or
/// through extension_call), waiting; None for any other result.
pub(crate) fn proposed(tool: &str, is_error: bool, details: &serde_json::Value) -> Option<v1::AppAccessProposal> {
    if !matches!(tool, TOOL | SET_TOOL | "extension_call") || is_error {
        return None;
    }
    let bytes = STANDARD.decode(details.get("app_access")?.as_str()?).ok()?;
    v1::AppAccessProposal::decode(bytes.as_slice()).ok()
}

/// Shows the agent's proposals as they are in its just-loaded conversation.
pub(crate) fn restore(store: &Store, agent: &str, transcript: &mut crate::transcript::Transcript) -> rusqlite::Result<()> {
    let db = store.db();
    let mut statement = db.prepare(&format!("SELECT {COLUMNS} FROM app_access_proposals WHERE agent = ?1 ORDER BY created"))?;
    for row in statement.query_map([agent], Row::read)? {
        transcript.set_app_access(row?.to_proto());
    }
    Ok(())
}

/// Whether a proposal of the agent's waits for the person.
pub(crate) fn pending_of(store: &Store, agent: &str) -> bool {
    store
        .db()
        .query_row("SELECT EXISTS (SELECT 1 FROM app_access_proposals WHERE agent = ?1 AND state = 'pending')", [agent], |row| row.get(0))
        .unwrap_or(false)
}

/// Rivet's conversation was cleared: its waiting proposals can't be seen
/// any more, so they close as 'cleared' (shown as declined), without
/// telling it.
pub(crate) fn cleared(store: &Store, agent: &str) -> rusqlite::Result<()> {
    store.db().execute(
        "UPDATE app_access_proposals SET state = 'cleared', decided = ?2 WHERE agent = ?1 AND state = 'pending'",
        params![agent, now_ms()],
    )?;
    Ok(())
}

/// Rivet's conversation went back to before `since` (`crate::fork`): its
/// proposals made since then close as /clear's do.
pub(crate) fn cleared_since(store: &Store, agent: &str, since: i64) -> rusqlite::Result<()> {
    store.db().execute(
        "UPDATE app_access_proposals SET state = 'cleared', decided = ?3 WHERE agent = ?1 AND state = 'pending' AND created >= ?2",
        params![agent, since, now_ms()],
    )?;
    Ok(())
}

/// Rivet is deleted: its proposals go with its conversation.
pub(crate) fn forget(store: &Store, agent: &str) -> rusqlite::Result<()> {
    store.db().execute("DELETE FROM app_access_proposals WHERE agent = ?1", [agent])?;
    Ok(())
}

/// Shows a proposal as it is now in its agent's conversation.
fn reshow(factory: &Factory, row: &Row) {
    let mut inner = factory.lock();
    let entry = inner
        .agents
        .get_mut(&row.agent)
        .and_then(|runtime| runtime.transcript.as_mut())
        .and_then(|transcript| transcript.set_app_access(row.to_proto()));
    if let Some(entry) = entry {
        factory.send_change(&inner, &row.agent, crate::transcript::Change::Entry(entry));
        // Whether it waits shows with the agent.
        factory.broadcast_workspace(&inner);
    }
}

/// Ends a waiting proposal, shows it as it is now, and returns it; None if
/// it wasn't waiting.
fn end(factory: &Factory, id: &str, state: State, comment: &str, error: &str, replied: bool) -> Option<Row> {
    let word = match state {
        State::Granted => "granted",
        State::Declined => "declined",
        _ => "failed",
    };
    let changed = factory
        .store
        .db()
        .execute(
            "UPDATE app_access_proposals SET state = ?2, decided = ?3, comment = ?4, error = ?5, replied = ?6
             WHERE id = ?1 AND state = 'pending'",
            params![id, word, now_ms(), comment, error, replied],
        )
        .unwrap_or(0);
    if changed == 0 {
        return None;
    }
    let row = row(&factory.store, id)?;
    reshow(factory, &row);
    Some(row)
}

/// Tells Rivet how the person decided, and each agent given the app what
/// it can do now.
fn tell(factory: &Arc<Factory>, row: &Row) {
    let person = factory.person_capitalized();
    let app = &row.card.app_name;
    let note = crate::decisions::note(&person, &row.comment);
    let (summary, text) = match row.state() {
        State::Granted => {
            let summary = format!("{person} approved your proposal for {app}");
            let mut given = Vec::new();
            if !row.card.sources.is_empty() {
                let sources: Vec<&str> = row.card.sources.iter().map(|source| source.label.as_str()).collect();
                given.push(format!("it reads {}", sources.join(", ")));
            }
            for grant in &row.card.agents {
                given.push(format!("{} has {}", agent_name(factory, &grant.agent_id), grant.scopes.join(", ")));
            }
            (summary.clone(), format!("{summary}: {}.{note}", given.join("; ")))
        }
        State::Declined if row.replied => {
            let summary = format!("{person} replied to your proposal for {app}");
            let text = format!(
                "{summary}; nothing was given yet. They say:\n\n{}\n\nIf it still fits, propose it again with that in mind.",
                row.comment
            );
            (summary, text)
        }
        State::Declined => {
            let summary = format!("{person} declined your proposal for {app}");
            (summary.clone(), format!("{summary}. Nothing was given.{note}"))
        }
        _ => {
            let summary = format!("{person} approved your proposal for {app}, but it couldn't be given");
            (summary.clone(), format!("{summary}: {}{note}", row.error))
        }
    };
    factory.deliver_event(&row.agent, text, host_event(summary));
    if row.state() == State::Granted {
        for grant in &row.card.agents {
            if grant.agent_id == row.agent {
                continue;
            }
            let summary = format!("{person} gave you {app}");
            let text = format!("{summary} ({}): its tools are yours from your next step.", grant.scopes.join(", "));
            factory.deliver_event(&grant.agent_id, text, host_event(summary));
        }
    }
}

fn agent_name(factory: &Factory, id: &str) -> String {
    factory.agent_name(id)
}

/// One decision at a time.
static BUSY: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Why approving `row` would give something other than what it shows now;
/// None when everything still holds.
fn changed(factory: &Factory, row: &Row) -> Option<String> {
    let app = &row.card.app_name;
    let same = factory
        .lock()
        .config
        .apps
        .iter()
        .any(|installed| installed.id == row.app && installed.version == row.version && installed.key == row.key);
    if !same {
        return Some(format!("{app} was uninstalled, updated or replaced since Rivet suggested this, so nothing was given."));
    }
    let Some(manifest) = factory.apps.manifest(&row.app) else {
        return Some(format!("{app} can't be read right now, so nothing was given."));
    };
    let offered = super::data::proposable(factory, &manifest);
    for source in &row.card.sources {
        // A file: walked and checked again now.
        if super::files::is_file(&source.id) {
            if let Err(why) = super::data::file_offer(factory, &manifest, &source.id) {
                return Some(format!("{why} So nothing was given."));
            }
        } else if !offered.iter().any(|now| now.id == source.id) {
            return Some(format!("{} can't be given to {app} any more, so nothing was given.", source.label));
        }
    }
    for grant in &row.card.agents {
        let same = factory.lock().config.agent(&grant.agent_id).is_some()
            && crate::user_apps::current_incarnation(&factory.home, &grant.agent_id).as_ref() == row.incarnations.get(&grant.agent_id);
        if !same {
            return Some(format!("{} was deleted since Rivet suggested this, so nothing was given.", agent_name(factory, &grant.agent_id)));
        }
        if factory.pipes.contained(&grant.agent_id) {
            return Some(format!("{} is limited to a pipe now, so nothing was given.", agent_name(factory, &grant.agent_id)));
        }
        if grant.scopes.iter().any(|scope| manifest.scope(scope).is_none()) {
            return Some(format!("{app} no longer has the scopes proposed, so nothing was given."));
        }
    }
    None
}

/// DecideAppAccess: the person approves Rivet's proposal at `seq`, which
/// gives exactly what it shows, or declines it. Its card shows the
/// decision, and Rivet is told.
pub(crate) async fn decide(factory: &Arc<Factory>, request: &v1::DecideAppAccess) -> Result<(), RequestError> {
    let agent = request.agent_id.as_str();
    let comment = crate::decisions::comment(&request.comment)?;
    if request.reply && comment.is_empty() {
        return Err(RequestError::Proposal("Write a reply, or approve or decline it.".to_owned()));
    }
    factory.known_agent(agent)?;
    factory.ensure_loaded(agent).await?;
    let id = {
        let inner = factory.lock();
        let entry = inner
            .agents
            .get(agent)
            .and_then(|runtime| runtime.transcript.as_ref())
            .and_then(|transcript| transcript.entry(request.seq).cloned());
        match entry.and_then(|entry| entry.kind) {
            Some(v1::entry::Kind::AppAccess(proposal)) => proposal.id,
            _ => return Err(RequestError::Proposal("There's no such proposal in this conversation.".to_owned())),
        }
    };
    let _busy = BUSY.lock().await;
    let Some(mut found) = row(&factory.store, &id).filter(|row| row.agent == agent) else {
        return Err(RequestError::Proposal("There's no such proposal.".to_owned()));
    };
    if found.state() != State::Pending {
        // Decided meanwhile: the card catches up.
        reshow(factory, &found);
        return Ok(());
    }
    // A reply (#370 Submit) gives nothing: Rivet gets the comment.
    if !request.approve || request.reply {
        if let Some(row) = end(factory, &id, State::Declined, comment, "", request.reply) {
            tell(factory, &row);
        }
        return Ok(());
    }
    if request.digest != found.card.digest {
        reshow(factory, &found);
        return Err(RequestError::Proposal("This suggestion changed after you opened it. Look at it again.".to_owned()));
    }
    if let Some(why) = changed(factory, &found) {
        if let Some(row) = end(factory, &id, State::Failed, comment, &why, false) {
            tell(factory, &row);
        }
        return Ok(());
    }
    // An agent that read a web page since it was shown: the card says so
    // now (a new digest), and the person decides again, knowing.
    let read: Vec<String> = found
        .card
        .agents
        .iter()
        .filter(|grant| !factory.pipes.tainted(&grant.agent_id).is_empty())
        .map(|grant| grant.agent_id.clone())
        .collect();
    let newly_read = read.iter().any(|agent| !found.card.agents_read_pipes.contains(agent));
    if newly_read {
        found.card.agents_read_pipes = read;
        found.card.digest = digest(&found.card, &found.version, &found.key, &found.incarnations);
        factory.store.db().execute(
            "UPDATE app_access_proposals SET card = ?2, digest = ?3 WHERE id = ?1 AND state = 'pending'",
            params![id, found.card.encode_to_vec(), found.card.digest],
        )?;
        reshow(factory, &found);
        return Err(RequestError::Proposal("An agent on it has read a web page since Rivet suggested this. Look at it again.".to_owned()));
    }
    let given = give(factory, &found).await;
    if let Err(error) = given {
        let why = format!("It couldn't be given: {error}");
        if let Some(row) = end(factory, &id, State::Failed, comment, &why, false) {
            tell(factory, &row);
        }
        return Ok(());
    }
    if let Some(row) = end(factory, &id, State::Granted, comment, "", false) {
        tell(factory, &row);
    }
    Ok(())
}

/// Gives what `row` shows: its sources to the app, its scopes to each
/// agent, and the events it would hear (a card from apps_set_access; on
/// one from before, each keeps those it still has the scope for); kept as
/// the person's change, by the card.
async fn give(factory: &Arc<Factory>, row: &Row) -> Result<(), RequestError> {
    let manifest = factory.apps.manifest(&row.app).ok_or_else(|| super::no_app(&row.app))?;
    let current = factory.lock().config.apps.iter().find(|app| app.id == row.app).cloned().ok_or_else(|| super::no_app(&row.app))?;
    let hearing = !row.card.card_rules.is_empty() || !row.card.subscriptions.is_empty();
    let mut to = Access::default();
    for source in &row.card.sources {
        to.sources.insert(source.id.clone(), true);
    }
    for grant in &row.card.agents {
        let hears: Vec<String> = if hearing {
            row.card
                .subscriptions
                .iter()
                .filter(|subscription| subscription.agent_ids.contains(&grant.agent_id))
                .map(|subscription| subscription.event_kind.clone())
                .collect()
        } else {
            changes::held(&current, &grant.agent_id)
                .hears
                .into_iter()
                .filter(|kind| manifest.event(kind).is_some_and(|event| grant.scopes.contains(&event.scope)))
                .collect()
        };
        to.agents.insert(grant.agent_id.clone(), Held { scopes: grant.scopes.clone(), hears });
    }
    let how = How { by: "", via: Via::Card, reference: &row.id, reason: &row.card.reason, rules: &row.card.card_rules };
    changes::apply(factory, &row.app, &to, how).await?;
    tracing::info!(app = %row.app, proposal = %row.id, "the person approved what an app reads and who uses it");
    Ok(())
}
