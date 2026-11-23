//! What an app reads and who uses it, as it changed (#339): every change,
//! whoever made it (Rivet with apps_set_access, the person in their apps or
//! by approving Rivet's card, or undoing another), is kept here, never
//! changed afterwards but for being marked undone. Needs you's History,
//! Marketplace → Apps ("Given by Rivet · today") and Undo read it.
//!
//! A change touches parts: a source (on or off) or an agent (its scopes,
//! and the app's events it hears). Each keeps the touched parts as they
//! were before and after. Undo (only ever the person's, from their device)
//! puts each part still as the change left it back as it was before, and is
//! a change of its own.

use std::collections::BTreeMap;
use std::sync::Arc;

use factory_protocol::v1;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::AppConfig;
use super::bundle::Manifest;
use crate::config::Config;
use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;

pub(crate) const SCHEMA: &str = "
-- Every change to what an installed app reads, which agents use it and
-- what they hear of it (apps::changes): never changed afterwards, but for
-- undone_by.
CREATE TABLE app_access_changes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    app TEXT NOT NULL,
    -- '' for the person, else the admin agent that applied it (Rivet).
    by TEXT NOT NULL,
    -- rivet: Rivet applied it; card: the person approved Rivet's card;
    -- you: the person changed it in their apps; undo: the person undid
    -- another.
    via TEXT NOT NULL,
    -- card: the proposal; undo: the change undone.
    ref TEXT NOT NULL DEFAULT '',
    -- Rivet's words: why (apps_set_access's reason).
    reason TEXT NOT NULL DEFAULT '',
    -- JSON: Claktory's sentences of why it needed a card.
    rules TEXT NOT NULL DEFAULT '[]',
    -- JSON (Access): the touched parts before and after.
    before TEXT NOT NULL,
    after TEXT NOT NULL,
    -- JSON (Labels): names for the person, as they were then.
    labels TEXT NOT NULL,
    at INTEGER NOT NULL,
    -- The change that undid it.
    undone_by INTEGER
);
CREATE INDEX app_access_changes_by_app ON app_access_changes (app, at);
CREATE INDEX app_access_changes_at ON app_access_changes (at);
";

/// Parts of an app's access: sources on or off, and agents' scopes and
/// the events they hear.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Access {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sources: BTreeMap<String, bool>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<String, Held>,
}

/// What an agent holds of an app: no scopes, nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Held {
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub hears: Vec<String>,
}

impl Access {
    pub(crate) fn is_empty(&self) -> bool {
        self.sources.is_empty() && self.agents.is_empty()
    }
}

/// Names for the person, as they were when it changed.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Labels {
    pub app: String,
    #[serde(default)]
    pub sources: BTreeMap<String, String>,
    #[serde(default)]
    pub scopes: BTreeMap<String, String>,
    #[serde(default)]
    pub events: BTreeMap<String, String>,
}

impl Labels {
    pub(crate) fn of(config: &Config, manifest: &Manifest, app: &AppConfig) -> Labels {
        Labels {
            app: manifest.name.clone(),
            sources: super::data::infos(config, manifest, app).into_iter().map(|source| (source.id, source.label)).collect(),
            scopes: manifest.scopes.iter().map(|scope| (scope.id.clone(), scope.label.clone())).collect(),
            events: manifest
                .events
                .iter()
                .map(|event| (event.kind.clone(), if event.label.is_empty() { event.kind.clone() } else { event.label.clone() }))
                .collect(),
        }
    }
}

/// The parts `touched` names, as they are in `app` now.
pub(crate) fn now(app: &AppConfig, touched: &Access) -> Access {
    Access {
        sources: touched.sources.keys().map(|id| (id.clone(), app.sources.contains(id))).collect(),
        agents: touched.agents.keys().map(|agent| (agent.clone(), held(app, agent))).collect(),
    }
}

/// What `agent` holds of `app` now.
pub(crate) fn held(app: &AppConfig, agent: &str) -> Held {
    Held {
        scopes: app.grants.get(agent).cloned().unwrap_or_default(),
        hears: app.subscriptions.iter().filter(|(_, agents)| agents.iter().any(|id| id == agent)).map(|(kind, _)| kind.clone()).collect(),
    }
}

/// Sets the parts of `to` in the app `id` of `config`, checked against its
/// manifest: each source one it can be given (all of Claktory's records
/// only taken away), each scope and event its own, each event heard with
/// its scope. An agent loses the events it no longer has the scope for.
pub(crate) fn set(config: &mut Config, manifest: &Manifest, id: &str, to: &Access) -> Result<(), RequestError> {
    let no_app = || RequestError::Apps(format!("There's no app \"{id}\" installed."));
    for (agent, held) in &to.agents {
        if config.agent(agent).is_none() {
            return Err(RequestError::UnknownAgent(agent.clone()));
        }
        for scope in &held.scopes {
            if manifest.scope(scope).is_none() {
                return Err(RequestError::Apps(format!("The {} app has no scope \"{scope}\".", manifest.name)));
            }
        }
        for kind in &held.hears {
            let Some(event) = manifest.event(kind) else {
                return Err(RequestError::Apps(format!("The {} app has no event \"{kind}\".", manifest.name)));
            };
            if !held.scopes.contains(&event.scope) {
                return Err(RequestError::Apps(format!("An agent hears \"{kind}\" of {} only with \"{}\".", manifest.name, event.scope)));
            }
        }
    }
    let had = config.apps.iter().find(|app| app.id == id).ok_or_else(no_app)?.sources.clone();
    for (source, on) in &to.sources {
        if *on && !had.contains(source) {
            super::data::can_give(config, manifest, &had, source).map_err(RequestError::Apps)?;
        }
    }
    let app = config.apps.iter_mut().find(|app| app.id == id).ok_or_else(no_app)?;
    for (source, on) in &to.sources {
        app.sources.retain(|kept| kept != source);
        if *on {
            app.sources.push(source.clone());
        }
    }
    app.sources.sort();
    for (agent, held) in &to.agents {
        let mut scopes: Vec<String> = Vec::new();
        for scope in &held.scopes {
            if !scopes.contains(scope) {
                scopes.push(scope.clone());
            }
        }
        // In the manifest's order, so the agent's tools keep theirs.
        scopes.sort_by_key(|scope| manifest.scopes.iter().position(|spec| &spec.id == scope));
        if scopes.is_empty() {
            app.grants.remove(agent);
        } else {
            app.grants.insert(agent.clone(), scopes.clone());
        }
        for kind in manifest.events.iter().map(|event| &event.kind) {
            let hears = held.hears.contains(kind);
            let agents = app.subscriptions.entry(kind.clone()).or_default();
            agents.retain(|id| id != agent);
            if hears {
                agents.push(agent.clone());
            }
        }
        // Events it no longer has the scope for (an older manifest's too).
        for (kind, agents) in &mut app.subscriptions {
            let scope = manifest.event(kind).map(|event| event.scope.as_str());
            if !scope.is_some_and(|scope| scopes.iter().any(|granted| granted == scope)) {
                agents.retain(|id| id != agent);
            }
        }
        app.subscriptions.retain(|_, agents| !agents.is_empty());
    }
    Ok(())
}

/// How a change was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Via {
    Rivet,
    Card,
    You,
    Undo,
}

impl Via {
    fn word(self) -> &'static str {
        match self {
            Via::Rivet => "rivet",
            Via::Card => "card",
            Via::You => "you",
            Via::Undo => "undo",
        }
    }

    fn of(word: &str) -> Via {
        match word {
            "rivet" => Via::Rivet,
            "card" => Via::Card,
            "undo" => Via::Undo,
            _ => Via::You,
        }
    }
}

/// A change as kept.
#[derive(Clone, Debug)]
pub(crate) struct Change {
    pub id: i64,
    pub app: String,
    pub by: String,
    pub via: Via,
    pub reference: String,
    pub reason: String,
    pub before: Access,
    pub after: Access,
    pub labels: Labels,
    pub at: i64,
    pub undone_by: Option<i64>,
}

const COLUMNS: &str = "id, app, by, via, ref, reason, before, after, labels, at, undone_by";

impl Change {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Change> {
        let json = |index: usize| -> rusqlite::Result<String> { row.get(index) };
        Ok(Change {
            id: row.get(0)?,
            app: row.get(1)?,
            by: row.get(2)?,
            via: Via::of(&row.get::<_, String>(3)?),
            reference: row.get(4)?,
            reason: row.get(5)?,
            before: serde_json::from_str(&json(6)?).unwrap_or_default(),
            after: serde_json::from_str(&json(7)?).unwrap_or_default(),
            labels: serde_json::from_str(&json(8)?).unwrap_or_default(),
            at: row.get(9)?,
            undone_by: row.get(10)?,
        })
    }

    /// Claktory's sentence for it: "Rivet gave Ingrid Change dashboards on
    /// Charts", the parts joined. `names`: each agent's name now, by id.
    pub(crate) fn title(&self, names: &BTreeMap<String, String>) -> String {
        let who = if self.by.is_empty() { "You".to_owned() } else { names.get(&self.by).cloned().unwrap_or_else(|| self.by.clone()) };
        let app = &self.labels.app;
        let name = |id: &str| names.get(id).cloned().unwrap_or_else(|| id.to_owned());
        let source = |id: &str| self.labels.sources.get(id).cloned().unwrap_or_else(|| id.to_owned());
        let scope = |id: &str| self.labels.scopes.get(id).cloned().unwrap_or_else(|| id.to_owned());
        let event = |id: &str| super::lower_first(&self.labels.events.get(id).cloned().unwrap_or_else(|| id.to_owned()));
        let mut parts: Vec<String> = Vec::new();
        let on: Vec<String> = self
            .after
            .sources
            .iter()
            .filter(|(id, on)| **on && self.before.sources.get(*id) != Some(&true))
            .map(|(id, _)| source(id))
            .collect();
        let off: Vec<String> = self
            .after
            .sources
            .iter()
            .filter(|(id, on)| !**on && self.before.sources.get(*id) == Some(&true))
            .map(|(id, _)| source(id))
            .collect();
        if !on.is_empty() {
            parts.push(format!("let {app} read {}", and(&on)));
        }
        if !off.is_empty() {
            parts.push(format!("stopped {app} reading {}", and(&off)));
        }
        for (agent, after) in &self.after.agents {
            let before = self.before.agents.get(agent).cloned().unwrap_or_default();
            let agent = name(agent);
            let scopes: Vec<String> = after.scopes.iter().map(|id| scope(id)).collect();
            if after.scopes.is_empty() {
                if !before.scopes.is_empty() {
                    parts.push(format!("stopped {agent} using {app}"));
                }
                continue;
            }
            if before.scopes.is_empty() {
                parts.push(format!("let {agent} use {app}: {}", scopes.join(", ")));
            } else if before.scopes != after.scopes {
                parts.push(format!("changed what {agent} can do in {app}: {}", scopes.join(", ")));
            }
            let heard: Vec<String> = after.hears.iter().filter(|kind| !before.hears.contains(kind)).map(|kind| event(kind)).collect();
            let unheard: Vec<String> = before.hears.iter().filter(|kind| !after.hears.contains(kind)).map(|kind| event(kind)).collect();
            if !heard.is_empty() {
                parts.push(format!("let {agent} hear from {app}: {}", heard.join(", ")));
            }
            if !unheard.is_empty() && !before.scopes.is_empty() {
                parts.push(format!("stopped {agent} hearing from {app}: {}", unheard.join(", ")));
            }
        }
        if parts.is_empty() {
            return format!("{who} changed nothing in {app}");
        }
        format!("{who} {}", parts.join("; "))
    }
}

/// "Lea's", "Charts'".
pub(crate) fn possessive(name: &str) -> String {
    if name.ends_with('s') { format!("{name}'") } else { format!("{name}'s") }
}

/// "A", "A and B", "A, B and C".
pub(crate) fn and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// What a change records.
pub(crate) struct Made<'a> {
    pub app: &'a str,
    pub by: &'a str,
    pub via: Via,
    pub reference: &'a str,
    pub reason: &'a str,
    pub rules: &'a [String],
    pub before: &'a Access,
    pub after: &'a Access,
    pub labels: &'a Labels,
}

/// Keeps a change; its id.
pub(crate) fn record(factory: &Factory, made: Made<'_>) -> rusqlite::Result<i64> {
    let db = factory.store.db();
    db.execute(
        "INSERT INTO app_access_changes (app, by, via, ref, reason, rules, before, after, labels, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            made.app,
            made.by,
            made.via.word(),
            made.reference,
            made.reason,
            serde_json::to_string(made.rules).unwrap_or_else(|_| "[]".to_owned()),
            serde_json::to_string(made.before).unwrap_or_default(),
            serde_json::to_string(made.after).unwrap_or_default(),
            serde_json::to_string(made.labels).unwrap_or_default(),
            now_ms(),
        ],
    )?;
    Ok(db.last_insert_rowid())
}

/// A change by its id.
pub(crate) fn find(factory: &Factory, id: i64) -> Option<Change> {
    factory
        .store
        .db()
        .query_row(&format!("SELECT {COLUMNS} FROM app_access_changes WHERE id = ?1"), [id], Change::read)
        .optional()
        .ok()
        .flatten()
}

/// The changes made from `from` to before `before` (ms), newest first, at
/// most `limit`; for Needs you's History.
pub(crate) fn between(db: &rusqlite::Connection, from: i64, before: i64, limit: i64) -> rusqlite::Result<Vec<Change>> {
    let mut statement =
        db.prepare(&format!("SELECT {COLUMNS} FROM app_access_changes WHERE at >= ?1 AND at < ?2 ORDER BY at DESC, id DESC LIMIT ?3"))?;
    statement.query_map(params![from, before, limit], Change::read)?.collect()
}

/// Who set each of the app's sources and agents to what it is: the last
/// change since it was installed that left it as it is now.
pub(crate) fn given(factory: &Factory, app: &AppConfig) -> (BTreeMap<String, v1::AppAccessGiven>, BTreeMap<String, v1::AppAccessGiven>) {
    let mut sources = BTreeMap::new();
    let mut agents = BTreeMap::new();
    let changes: Vec<Change> = {
        let db = factory.store.db();
        let Ok(mut statement) =
            db.prepare(&format!("SELECT {COLUMNS} FROM app_access_changes WHERE app = ?1 AND at >= ?2 ORDER BY at, id"))
        else {
            return (sources, agents);
        };
        statement
            .query_map(params![app.id, app.installed_ms], Change::read)
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    };
    for change in changes {
        let given = v1::AppAccessGiven {
            by_agent_id: change.by.clone(),
            at_ms: change.at,
            change_id: change.id.to_string(),
            by_card: change.via == Via::Card,
        };
        for (source, on) in &change.after.sources {
            if *on == app.sources.contains(source) {
                sources.insert(source.clone(), given.clone());
            } else {
                sources.remove(source);
            }
        }
        for (agent, after) in &change.after.agents {
            if *after == held(app, agent) {
                agents.insert(agent.clone(), given.clone());
            } else {
                agents.remove(agent);
            }
        }
    }
    (sources, agents)
}

/// Applies `to` (the touched parts as they should be) to the app and keeps
/// the change; its id, or None when nothing changed. The caller has
/// checked who may.
pub(crate) async fn apply(factory: &Arc<Factory>, app_id: &str, to: &Access, how: How<'_>) -> Result<Option<i64>, RequestError> {
    let manifest = factory.apps.manifest(app_id).ok_or_else(|| RequestError::Apps(format!("There's no app \"{app_id}\" installed.")))?;
    let mut was: Option<(Access, Labels)> = None;
    factory
        .edit_config(|config| {
            let app = config
                .apps
                .iter()
                .find(|app| app.id == app_id)
                .ok_or_else(|| RequestError::Apps(format!("There's no app \"{app_id}\" installed.")))?
                .clone();
            let before = now(&app, to);
            if before == *to {
                return Ok(());
            }
            let mut labels = Labels::of(config, &manifest, &app);
            // A file given now isn't among the app's sources yet.
            for id in to.sources.keys().filter(|id| super::files::is_file(id)) {
                labels.sources.entry(id.clone()).or_insert_with(|| super::files::label(config, id));
            }
            set(config, &manifest, app_id, to)?;
            was = Some((before, labels));
            Ok(())
        })
        .await?;
    let Some((before, labels)) = was else { return Ok(None) };
    // What it is now, as set (scopes in order, events dropped with their
    // scope).
    let after = factory.lock().config.apps.iter().find(|app| app.id == app_id).map(|app| now(app, to)).unwrap_or_else(|| to.clone());
    let id = record(
        factory,
        Made {
            app: app_id,
            by: how.by,
            via: how.via,
            reference: how.reference,
            reason: how.reason,
            rules: how.rules,
            before: &before,
            after: &after,
            labels: &labels,
        },
    )?;
    factory.apps_changed();
    Ok(Some(id))
}

/// Who made a change, how, and why.
#[derive(Clone, Copy)]
pub(crate) struct How<'a> {
    pub by: &'a str,
    pub via: Via,
    pub reference: &'a str,
    pub reason: &'a str,
    pub rules: &'a [String],
}

impl How<'static> {
    /// The person, in their apps.
    pub(crate) const YOU: How<'static> = How { by: "", via: Via::You, reference: "", reason: "", rules: &[] };
}

/// UndoAppAccessChange: the person undoes change `id`.
pub(crate) async fn undo(factory: &Arc<Factory>, request: &v1::UndoAppAccessChange) -> Result<v1::AppAccessUndone, RequestError> {
    let refused = |text: &str| RequestError::Apps(text.to_owned());
    let id: i64 = request.change_id.trim().parse().map_err(|_| refused("This change can't be found."))?;
    let _busy = UNDOING.lock().await;
    let Some(change) = find(factory, id) else { return Err(refused("This change can't be found.")) };
    if change.undone_by.is_some() {
        return Err(refused("This was undone already."));
    }
    let app = factory.lock().config.apps.iter().find(|app| app.id == change.app).cloned();
    let Some(app) = app.filter(|app| app.installed_ms <= change.at) else {
        return Err(RequestError::Apps(format!("{} was uninstalled since, so there's nothing to undo.", change.labels.app)));
    };
    let names = agent_names(factory);
    let current = now(&app, &change.after);
    let mut back = Access::default();
    let mut kept = Vec::new();
    for (source, after) in &change.after.sources {
        if current.sources.get(source) == Some(after) {
            back.sources.insert(source.clone(), change.before.sources.get(source).copied().unwrap_or(false));
        } else {
            let label = change.labels.sources.get(source).cloned().unwrap_or_else(|| source.clone());
            kept.push(format!("{} access to {label} changed since, so it was left as it is.", possessive(&change.labels.app)));
        }
    }
    for (agent, after) in &change.after.agents {
        let exists = factory.lock().config.agent(agent).is_some();
        if exists && current.agents.get(agent) == Some(after) {
            back.agents.insert(agent.clone(), change.before.agents.get(agent).cloned().unwrap_or_default());
        } else {
            let name = names.get(agent).cloned().unwrap_or_else(|| agent.clone());
            kept.push(format!("{} access to {} changed since, so it was left as it is.", possessive(&name), change.labels.app));
        }
    }
    let undid = if back.is_empty() {
        None
    } else {
        let reference = change.id.to_string();
        apply(factory, &change.app, &back, How { by: "", via: Via::Undo, reference: &reference, reason: "", rules: &[] }).await?
    };
    if let Some(undid) = undid {
        factory.store.db().execute("UPDATE app_access_changes SET undone_by = ?2 WHERE id = ?1", params![change.id, undid])?;
        // Agents given or losing the app hear it, as when the person
        // changes it in their apps.
        factory.broadcast_workspace(&factory.lock());
    }
    Ok(v1::AppAccessUndone { change_id: undid.map(|id| id.to_string()).unwrap_or_default(), kept })
}

/// One undo at a time.
static UNDOING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Each agent's name now, by id.
pub(crate) fn agent_names(factory: &Factory) -> BTreeMap<String, String> {
    factory.lock().config.agents.iter().map(|agent| (agent.id.clone(), agent.name.clone())).collect()
}
