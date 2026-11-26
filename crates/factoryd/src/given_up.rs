//! Access agents gave up on their own (#236): a pipe, or only reading or
//! leading one (`pipes::leave`'s pipe_leave), an app (apps_give_up), or
//! messaging an agent of another project (agents_give_up_reach). Giving
//! access up only takes it away, so it asks no one; the person sees it in
//! Needs you's History (NEEDS_YOU_RECORD_KIND_GIVEN_UP), with the agent's
//! reason apart, and getting it back is theirs alone.
//!
//! Each tool acts only on its caller (it has no agent parameter), and only
//! on what it holds when the call runs, checked under the same lock the
//! person's change takes (Fugo 43111, 43129): a frame keeps its tools, so a
//! call may come after the access is gone, and is then refused, changing
//! nothing. Contained agents (a pipe's liaison, one limited to a pipe, a
//! curator, an errand agent) can't change their own access: that's the
//! person's.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use factory_agent::{Tool, ToolResult};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::apps::changes::{self, Access, Held, How, Via};
use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;

pub(crate) const SCHEMA: &str = "
-- Access agents gave up on their own (#236, given_up), for Needs you's
-- history.
CREATE TABLE IF NOT EXISTS access_given_up (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent TEXT NOT NULL,
    -- pipe, pipe_reading, pipe_leading, app or reach.
    what TEXT NOT NULL,
    -- The pipe's id, the app's id, or the agent it no longer messages.
    target TEXT NOT NULL,
    -- Its name for the person then: the pipe's name when this side chose
    -- it (quoted), else 'with <who>' or '', the app's name, the agent's
    -- name.
    label TEXT NOT NULL,
    -- Why, in the agent's words.
    reason TEXT NOT NULL DEFAULT '',
    at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS access_given_up_at ON access_given_up (at);
";

/// The key of a give-up's result details naming its record.
pub(crate) const DETAIL: &str = "access_given_up";
pub(crate) const APPS_TOOL: &str = "apps_give_up";
pub(crate) const REACH_TOOL: &str = "agents_give_up_reach";
/// The most of a reason kept.
const MAX_REASON: usize = 500;

/// What was given up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum What {
    /// The whole pipe.
    Pipe,
    /// Reading the pipe: it only sends to it now.
    PipeReading,
    /// Leading the pipe: it's still one of its agents.
    PipeLeading,
    App,
    /// Messaging an agent of another project.
    Reach,
}

impl What {
    fn word(self) -> &'static str {
        match self {
            What::Pipe => "pipe",
            What::PipeReading => "pipe_reading",
            What::PipeLeading => "pipe_leading",
            What::App => "app",
            What::Reach => "reach",
        }
    }

    fn of(word: &str) -> Option<What> {
        Some(match word {
            "pipe" => What::Pipe,
            "pipe_reading" => What::PipeReading,
            "pipe_leading" => What::PipeLeading,
            "app" => What::App,
            "reach" => What::Reach,
            _ => return None,
        })
    }

    pub(crate) fn pipe(self) -> bool {
        matches!(self, What::Pipe | What::PipeReading | What::PipeLeading)
    }
}

/// Keeps that `agent` gave up `target`: its id; None when it couldn't be
/// kept (logged).
pub(crate) fn record(factory: &Factory, agent: &str, what: What, target: &str, label: &str, reason: &str) -> Option<i64> {
    let db = factory.store.db();
    let kept = db.execute(
        "INSERT INTO access_given_up (agent, what, target, label, reason, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![agent, what.word(), target, label, reason, now_ms()],
    );
    match kept {
        Ok(_) => Some(db.last_insert_rowid()),
        Err(error) => {
            tracing::error!(%error, "couldn't keep access given up for the history");
            None
        }
    }
}

/// Access an agent gave up, as kept.
#[derive(Clone, Debug)]
pub(crate) struct GivenUp {
    pub id: i64,
    pub agent: String,
    pub what: What,
    pub target: String,
    pub label: String,
    pub reason: String,
    pub at: i64,
}

impl GivenUp {
    /// Claktory's sentence for it, with agents' names now: "Mista left the
    /// pipe "Launch"", "Mista gave up Charts", "Mista gave up messaging Ana".
    pub(crate) fn title(&self, names: &HashMap<String, String>) -> String {
        let agent = names.get(&self.agent).cloned().unwrap_or_else(|| self.agent.clone());
        let pipe = || pipe_called(&self.label);
        match self.what {
            What::Pipe => format!("{agent} left {}", pipe()),
            What::PipeReading => format!("{agent} stopped reading {}: it only sends to it now", pipe()),
            What::PipeLeading => format!("{agent} stopped leading {}", pipe()),
            What::App => format!("{agent} gave up {}", self.label),
            What::Reach => {
                let other = names.get(&self.target).cloned().unwrap_or_else(|| self.label.clone());
                format!("{agent} gave up messaging {other}")
            }
        }
    }
}

/// "the pipe "Launch"", "the pipe with Fred" for one whose name the other
/// side chose, or "a pipe" when who it's with isn't known either
/// (`pipes::leave`'s label).
pub(crate) fn pipe_called(label: &str) -> String {
    if label.starts_with('"') || label.starts_with("with ") { format!("the pipe {label}") } else { "a pipe".to_owned() }
}

/// What was given up from `since` to before `before`, newest first, at
/// most `limit`.
pub(crate) fn between(db: &rusqlite::Connection, since: i64, before: i64, limit: i64) -> rusqlite::Result<Vec<GivenUp>> {
    let mut statement = db.prepare(
        "SELECT id, agent, what, target, label, reason, at FROM access_given_up
         WHERE at >= ?1 AND at < ?2 ORDER BY at DESC, id DESC LIMIT ?3",
    )?;
    let rows = statement.query_map(rusqlite::params![since, before, limit], |row| {
        Ok((
            row.get::<_, String>(2)?,
            GivenUp {
                id: row.get(0)?,
                agent: row.get(1)?,
                what: What::App,
                target: row.get(3)?,
                label: row.get(4)?,
                reason: row.get(5)?,
                at: row.get(6)?,
            },
        ))
    })?;
    let mut given = Vec::new();
    for row in rows {
        let (word, mut found) = row?;
        // A kind a later version wrote is left out.
        let Some(what) = What::of(&word) else { continue };
        found.what = what;
        given.push(found);
    }
    Ok(given)
}

/// The record a give-up call made, from its entry.
pub(crate) fn given_up_of(call: &factory_protocol::v1::ToolCall) -> Option<String> {
    if !matches!(call.name.as_str(), crate::pipes::leave::TOOL | APPS_TOOL | REACH_TOOL | "extension_call") {
        return None;
    }
    let details: Value = serde_json::from_str(&call.details).ok()?;
    details.get(DETAIL)?.as_str().map(str::to_owned)
}

/// The reason given, trimmed: required, and at most `MAX_REASON`
/// characters.
pub(crate) fn reason(given: &str) -> Result<String, String> {
    let reason = given.trim();
    if reason.is_empty() {
        return Err("Say why you're giving it up, in reason: the person reads it in History. Nothing was changed.".to_owned());
    }
    if reason.chars().count() > MAX_REASON {
        return Err(format!("Keep reason under {MAX_REASON} characters. Nothing was changed."));
    }
    Ok(reason.to_owned())
}

/// Refuses a contained agent: what it holds is the person's to change.
pub(crate) fn refuse_contained(factory: &Factory, agent: &str) -> Result<(), String> {
    if factory.pipes.contained(agent) {
        let person = factory.person_capitalized();
        return Err(format!(
            "{person} set what you can use, so only they change it: ask {person} to take it from you. Nothing was changed."
        ));
    }
    Ok(())
}

/// The result of a give-up: what it says, and its record.
fn done(said: String, mut details: Value, kept: Option<i64>) -> ToolResult {
    if let Some(map) = details.as_object_mut() {
        map.insert(DETAIL.to_owned(), kept.map_or(Value::Null, |id| Value::String(id.to_string())));
    }
    ToolResult::text(said, details)
}

// ---- Apps ----

const APPS: &str = "Give up an app you were given, on your own, when you no longer need it or decide you shouldn't have it: its tools and events stop reaching you at once, without asking the person, and they see it in History with your reason. app: its name or id; reason: why, for them (at most 500 characters). Getting it back is their choice, in the app's settings.";

#[derive(Deserialize)]
struct AppArgs {
    app: String,
    #[serde(default)]
    reason: String,
}

/// apps_give_up, for an agent given any app.
pub(crate) fn apps_tool(factory: &Arc<Factory>, agent: &str) -> Tool {
    crate::extensions::async_tool(
        factory,
        agent,
        APPS_TOOL,
        "Give up an app",
        APPS,
        json!({
            "type": "object",
            "required": ["app", "reason"],
            "properties": {"app": {"type": "string"}, "reason": {"type": "string"}},
            "additionalProperties": false,
        }),
        give_up_app,
    )
}

/// The apps `agent` holds any scope of, by id, with their names.
fn apps_held(factory: &Factory, agent: &str) -> BTreeMap<String, String> {
    let ids: Vec<String> = factory
        .lock()
        .config
        .apps
        .iter()
        .filter(|app| app.grants.get(agent).is_some_and(|scopes| !scopes.is_empty()))
        .map(|app| app.id.clone())
        .collect();
    ids.into_iter()
        .map(|id| {
            let name = factory.apps.manifest(&id).map_or_else(|| id.clone(), |manifest| manifest.name.clone());
            (id, name)
        })
        .collect()
}

/// apps_give_up (#236): the caller's own scopes of the app, and the events
/// it hears of it, taken as the person's removal takes them
/// (`changes::apply`, kept as the agent's change), with no card.
async fn give_up_app(factory: Arc<Factory>, agent: String, args: AppArgs) -> Result<ToolResult, String> {
    let reason = reason(&args.reason)?;
    refuse_contained(&factory, &agent)?;
    let wanted = args.app.trim();
    let held = apps_held(&factory, &agent);
    let found = held
        .iter()
        .find(|(id, _)| id.as_str() == wanted)
        .or_else(|| held.iter().find(|(_, name)| name.eq_ignore_ascii_case(wanted)))
        .map(|(id, name)| (id.clone(), name.clone()));
    let Some((app, name)) = found else {
        let yours: Vec<&str> = held.values().map(String::as_str).collect();
        let yours = if yours.is_empty() { "You have none.".to_owned() } else { format!("Yours: {}.", yours.join(", ")) };
        return Err(format!("\"{wanted}\" isn't an app you were given. {yours} Nothing was changed."));
    };
    let to = Access { agents: [(agent.clone(), Held::default())].into(), ..Default::default() };
    let how = How { by: &agent, via: Via::Agent, reference: "", reason: &reason, rules: &[] };
    // Checked again as it changes, under the settings' lock: none left
    // means the person (or Rivet) took it meanwhile.
    match changes::apply(&factory, &app, &to, how).await {
        Ok(Some(_)) => {}
        Ok(None) => return Err(format!("You no longer have {name}: it was taken from you already. Nothing was changed.")),
        Err(error) => return Err(format!("{error} Nothing was changed.")),
    }
    let kept = record(&factory, &agent, What::App, &app, &name, &reason);
    let person = factory.person_capitalized();
    Ok(done(
        format!(
            "Gave up {name}: its tools and events no longer reach you, from your next step on. {person} sees this in History, with your reason; getting it back is their choice."
        ),
        json!({"app": app}),
        kept,
    ))
}

// ---- Messaging ----

const REACH: &str = "Give up messaging an agent of another project that you were let message, on your own, when you no longer need to or decide you shouldn't: you can't message it from your next step on, without asking the person, and they see it in History with your reason. It can still message you if it could. to: the agent, by id or name; reason: why, for them (at most 500 characters). Agents in your own project stay reachable. Getting it back is their choice, in your settings.";

#[derive(Deserialize)]
struct ReachArgs {
    to: String,
    #[serde(default)]
    reason: String,
}

/// agents_give_up_reach, for an agent let message others outside its
/// project (the Messaging extension's `also`).
pub(crate) fn reach_tool(factory: &Arc<Factory>, agent: &str) -> Tool {
    crate::extensions::async_tool(
        factory,
        agent,
        REACH_TOOL,
        "Give up messaging an agent",
        REACH,
        json!({
            "type": "object",
            "required": ["to", "reason"],
            "properties": {"to": {"type": "string"}, "reason": {"type": "string"}},
            "additionalProperties": false,
        }),
        give_up_reach,
    )
}

/// agents_give_up_reach (#236): one entry of the caller's own `also`,
/// taken out under the settings' lock; an entry it doesn't have is
/// refused, whoever it names.
async fn give_up_reach(factory: Arc<Factory>, agent: String, args: ReachArgs) -> Result<ToolResult, String> {
    use crate::extensions::agents::ID;
    let reason = reason(&args.reason)?;
    refuse_contained(&factory, &agent)?;
    let wanted = args.to.trim().to_owned();
    let refused = |text: String| RequestError::GiveUp(format!("{text} Nothing was changed."));
    let gone = factory
        .edit_config(|config| {
            let me = config.agent(&agent).ok_or_else(|| refused("You're no longer in this Claktory.".to_owned()))?;
            let also = crate::extensions::agent_list(me, ID, "also");
            let entry = also
                .iter()
                .find(|id| **id == wanted)
                .or_else(|| also.iter().find(|id| config.agent(id).is_some_and(|other| other.name.eq_ignore_ascii_case(&wanted))))
                .cloned();
            let Some(entry) = entry else {
                let listed: Vec<String> =
                    also.iter().map(|id| config.agent(id).map_or_else(|| id.clone(), |other| other.name.clone())).collect();
                let yours = if listed.is_empty() {
                    "You weren't let message any agent outside your project.".to_owned()
                } else {
                    format!("Those you were let message outside your project: {}.", listed.join(", "))
                };
                return Err(refused(format!("\"{wanted}\" isn't an agent you were let message outside your project. {yours}")));
            };
            let name = config.agent(&entry).map_or_else(|| entry.clone(), |other| other.name.clone());
            let me = config.agents.iter_mut().find(|other| other.id == agent).expect("found above");
            if let Some(list) = me.extensions.get_mut(ID).and_then(|settings| settings.get_mut("also")).and_then(Value::as_array_mut) {
                list.retain(|id| id.as_str() != Some(entry.as_str()));
            }
            Ok((entry, name))
        })
        .await
        .map_err(|error| error.to_string())?;
    let (entry, name) = gone;
    let kept = record(&factory, &agent, What::Reach, &entry, &name, &reason);
    let person = factory.person_capitalized();
    let still = {
        let inner = factory.lock();
        match (inner.config.agent(&agent), inner.config.agent(&entry)) {
            (Some(me), Some(other)) => crate::extensions::agents::can_message(&inner.config, me, other),
            _ => false,
        }
    };
    // An admin agent reaches every agent all the same.
    let note = if still { " You can still message it all the same, as an admin agent." } else { "" };
    Ok(done(
        format!(
            "Gave up messaging {name}: it's no longer on your list.{note} {person} sees this in History, with your reason; getting it back is their choice."
        ),
        json!({"agent": entry}),
        kept,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// History never names a pipe by its id, nor by a name the other side
    /// chose (Trish 45527).
    #[test]
    fn a_pipe_is_named_by_this_sides_name_or_who_its_with() {
        assert_eq!(pipe_called("\"Launch\""), "the pipe \"Launch\"");
        assert_eq!(pipe_called("with Fred"), "the pipe with Fred");
        assert_eq!(pipe_called(""), "a pipe");
        assert_eq!(pipe_called("pipe 3f2a"), "a pipe");
        let given = GivenUp {
            id: 1,
            agent: "mista".into(),
            what: What::PipeReading,
            target: "p".into(),
            label: "with Fred".into(),
            reason: String::new(),
            at: 0,
        };
        let names = HashMap::from([("mista".to_owned(), "Mista".to_owned())]);
        assert_eq!(given.title(&names), "Mista stopped reading the pipe with Fred: it only sends to it now");
    }
}
