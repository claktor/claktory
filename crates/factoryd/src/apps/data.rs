//! Data apps read: the sources their manifest's `[[data]]` asks for, each
//! read only once the person grants it (`AppConfig.sources`), and checked
//! again on every query. Queries run here, in factoryd, never in the app's
//! sandbox: one read-only SELECT or WITH statement, bound params, the
//! Database extension's authorizer (agents' memory, pipes and comments on
//! files stay unreadable), 10 s, and at most 10,000 rows and 2 MB an answer.
//!
//! The sources:
//! - **Activity** (`activity`): what happened, never what was written: of
//!   factory.db, only the tables and columns `ACTIVITY_READS` lists (ids, agents,
//!   times, states, counts and token use), every other read refused by the
//!   authorizer (default deny: a table or column added later stays out).
//! - **Conversations** (`conversations`): the messages agents got, with
//!   their text (`CONVERSATIONS_READS`), and nothing else of factory.db.
//! - **Schedule** (`schedule`): Activity, and what names it: monitors' names
//!   and when they fire, checklist items' words and when they're looked at
//!   again, and each agent's project (`SCHEDULE_READS`, `SCHEDULE_TABLES`).
//!   Never what a monitor runs or says to its agent.
//! - **An agent's database** (`agent:<id>`), each one the person chooses.
//! - **All of Claktory's records** (`factory-records`): factory.db as the
//!   Database extension reads it. No longer granted: kept, and listed, only
//!   where the person granted it before, so those apps keep working.
//! - **A file** (`file:<place>:<path>`), a CSV or TSV each the person (or
//!   Rivet, on a card) chooses, as the table `rows` (`super::files`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use factory_protocol::apps::{self as wire, DataError};
use factory_protocol::v1;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::types::ValueRef;
use serde_json::{Value, json};

use super::bundle::{ACTIVITY, AGENT_DATABASE, CONVERSATIONS, FACTORY_RECORDS, FILE, Manifest, SCHEDULE};
use super::files;
use super::{AppConfig, no_app};
use crate::config::Config;
use crate::extensions::database::sql::{self, Target};
use crate::factory::{Factory, RequestError};

/// All of Claktory's records' source id: only where it was granted before.
pub(crate) const FACTORY_SOURCE: &str = "factory-records";
pub(crate) const ACTIVITY_SOURCE: &str = "activity";
pub(crate) const CONVERSATIONS_SOURCE: &str = "conversations";
pub(crate) const SCHEDULE_SOURCE: &str = "schedule";
/// An agent's database's source id: this, then the agent's id.
pub(crate) const AGENT_PREFIX: &str = "agent:";
const MAX_ROWS: usize = 10_000;
const MAX_BYTES: usize = 2 << 20;

/// What Activity reads of factory.db: each table, and its columns. Ids,
/// agents, times, states, counts and token use; never text anyone wrote
/// (a message, a task's command or output, a monitor's prompt, a checklist
/// item, an event's summary). Widening it is a reviewed change: a test
/// holds it to this list.
pub(crate) const ACTIVITY_READS: &[(&str, &[&str])] = &[
    ("runs", &["id", "agent", "version", "started", "ended", "outcome", "last_call"]),
    ("agent_usage", &["hour", "provider", "model", "agent", "agent_name", "requests", "input", "output", "cache_read", "cache_write"]),
    ("model_usage", &["hour", "provider", "model", "requests", "input", "output", "cache_read", "cache_write"]),
    ("agent_presence", &["agent", "idle_since", "failed"]),
    ("tasks", &["id", "agent", "task", "started", "ended", "status"]),
    (
        "monitors",
        &[
            "agent",
            "id",
            "kind",
            "until",
            "max_fires",
            "idle_only",
            "created_by",
            "created",
            "state",
            "next_due",
            "last_fired",
            "fires",
            "updated",
        ],
    ),
    ("monitor_events", &["seq", "agent", "monitor", "time", "actor", "kind"]),
    (
        "checklist_items",
        &["agent", "id", "status", "owner", "created", "updated", "blocked_since", "goal", "blocked_kind", "retries", "dismissed_at"],
    ),
    ("checklist_events", &["seq", "agent", "item", "time", "actor", "kind"]),
    ("messages", &["id", "agent", "source", "sender", "sender_id", "created", "delivered", "run", "dropped", "sender_run", "intent"]),
];

/// What Conversations reads of factory.db: the messages agents got, whole.
pub(crate) const CONVERSATIONS_READS: &[(&str, &[&str])] = &[(
    "messages",
    &[
        "id",
        "agent",
        "source",
        "sender",
        "sender_id",
        "summary",
        "text",
        "created",
        "delivered",
        "run",
        "dropped",
        "sent_from",
        "attachments",
        "voice",
        "sender_run",
        "reply_to",
        "intent",
    ],
)];

/// What Schedule reads of factory.db besides Activity's: what monitors and
/// checklist items are called, and when an item is looked at again. A
/// monitor's spec never: it holds a check's or a watch's command.
pub(crate) const SCHEDULE_READS: &[(&str, &[&str])] =
    &[("monitors", &["name"]), ("checklist_items", &["text", "retry_at", "reask_at"])];

/// The tables Schedule makes in each connection's temp database
/// (`schedule_tables`): at, every and cron monitors' specs, as stored
/// (`{"at": …}`, `{"every": …, "start"?: …}`, `{"cron": …}`), and the agents
/// there are now, with their project's id and name (null in the workspace)
/// and whether they're admin agents (0 or 1).
pub(crate) const SCHEDULE_TABLES: &[(&str, &[&str])] =
    &[("monitor_specs", &["agent", "id", "kind", "spec"]), ("agents", &["id", "name", "project", "admin", "project_name"])];

/// What a source may read, past its authorizer: factory.db's tables and
/// columns, and the tables it makes in the connection's temp database.
struct Allowed {
    main: &'static [&'static [(&'static str, &'static [&'static str])]],
    temp: &'static [(&'static str, &'static [&'static str])],
}

const ACTIVITY_ALLOWED: Allowed = Allowed { main: &[ACTIVITY_READS], temp: &[] };
const CONVERSATIONS_ALLOWED: Allowed = Allowed { main: &[CONVERSATIONS_READS], temp: &[] };
const SCHEDULE_ALLOWED: Allowed = Allowed { main: &[ACTIVITY_READS, SCHEDULE_READS], temp: SCHEDULE_TABLES };

/// An agent as Schedule's `agents` table has it.
struct ScheduleAgent {
    id: String,
    name: String,
    project: Option<String>,
    admin: bool,
    project_name: Option<String>,
}

/// Which data a source is.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Reads {
    Activity,
    Conversations,
    Schedule,
    /// All of factory.db, as the Database extension reads it.
    Records,
    Agent(String),
    /// A file: its source id.
    File(String),
}

/// A source the app can be given, as things are now.
struct Source {
    id: String,
    reads: Reads,
    label: String,
    holds: String,
    reason: String,
}

impl Source {
    fn kind(&self) -> v1::AppDataKind {
        match self.reads {
            Reads::Activity => v1::AppDataKind::Activity,
            Reads::Conversations => v1::AppDataKind::Conversations,
            Reads::Schedule => v1::AppDataKind::Unspecified,
            Reads::Records => v1::AppDataKind::FactoryRecords,
            Reads::Agent(_) => v1::AppDataKind::AgentDatabase,
            Reads::File(_) => v1::AppDataKind::File,
        }
    }

    fn agent(&self) -> &str {
        match &self.reads {
            Reads::Agent(agent) => agent,
            _ => "",
        }
    }

    fn info(&self, granted: bool) -> v1::AppDataSource {
        v1::AppDataSource {
            id: self.id.clone(),
            kind: self.kind().into(),
            agent_id: self.agent().to_owned(),
            label: self.label.clone(),
            reason: self.reason.clone(),
            granted,
            holds: self.holds.clone(),
            // Anything people wrote: all but Activity.
            private: self.reads != Reads::Activity,
            given: None,
        }
    }

    fn target(&self) -> Target {
        match &self.reads {
            Reads::Agent(agent) => Target::Own(agent.clone()),
            _ => Target::Factory,
        }
    }

    /// What it may read of its database; None for all of it.
    fn allowed(&self) -> Option<&'static Allowed> {
        match self.reads {
            Reads::Activity => Some(&ACTIVITY_ALLOWED),
            Reads::Conversations => Some(&CONVERSATIONS_ALLOWED),
            Reads::Schedule => Some(&SCHEDULE_ALLOWED),
            Reads::Records | Reads::Agent(_) | Reads::File(_) => None,
        }
    }
}

fn activity(reason: &str) -> Source {
    Source {
        id: ACTIVITY_SOURCE.to_owned(),
        reads: Reads::Activity,
        label: "Activity".to_owned(),
        holds: "When your agents run, their messages, background tasks, monitors and checklist items, and the tokens they use: times, states and counts, never what was written.".to_owned(),
        reason: reason.to_owned(),
    }
}

fn conversations(reason: &str) -> Source {
    Source {
        id: CONVERSATIONS_SOURCE.to_owned(),
        reads: Reads::Conversations,
        label: "Conversations".to_owned(),
        holds: "The messages your agents get, with their text: yours, other agents' and events.".to_owned(),
        reason: reason.to_owned(),
    }
}

fn schedule(reason: &str) -> Source {
    Source {
        id: SCHEDULE_SOURCE.to_owned(),
        reads: Reads::Schedule,
        label: "Schedule".to_owned(),
        holds: "What your agents have scheduled: their monitors' names and when they fire, their checklist items' words and when they're looked at again, and each agent's project. Never what a monitor runs or tells its agent.".to_owned(),
        reason: reason.to_owned(),
    }
}

fn records(reason: &str) -> Source {
    Source {
        id: FACTORY_SOURCE.to_owned(),
        reads: Reads::Records,
        label: "Everything Claktory keeps".to_owned(),
        holds: "Everything Claktory keeps about your agents, including the messages they get, with their text.".to_owned(),
        reason: reason.to_owned(),
    }
}

fn agent_source(config: &Config, agent: &str, reason: &str) -> Option<Source> {
    let found = config.agent(agent)?;
    Some(Source {
        id: format!("{AGENT_PREFIX}{agent}"),
        reads: Reads::Agent(agent.to_owned()),
        label: format!("{}'s database", found.name),
        holds: format!("Whatever {} keeps in its own database.", found.name),
        reason: reason.to_owned(),
    })
}

fn file_source(config: &Config, id: &str, reason: &str) -> Source {
    Source {
        id: id.to_owned(),
        reads: Reads::File(id.to_owned()),
        label: files::label(config, id),
        holds: files::holds(config, id),
        reason: reason.to_owned(),
    }
}

/// The sources `manifest` asks for (one for each agent there is when it
/// asks for any agent's database), the files `granted` has when it asks for
/// files, and all of Claktory's records where `granted` has them from
/// before.
fn sources(config: &Config, manifest: &Manifest, granted: &[String]) -> Vec<Source> {
    let mut sources: Vec<Source> = Vec::new();
    for data in &manifest.data {
        let found: Vec<Source> = match data.kind.as_str() {
            ACTIVITY => vec![activity(&data.reason)],
            CONVERSATIONS => vec![conversations(&data.reason)],
            SCHEDULE => vec![schedule(&data.reason)],
            AGENT_DATABASE if data.agent == "*" => {
                config.agents.iter().filter_map(|agent| agent_source(config, &agent.id, &data.reason)).collect()
            }
            AGENT_DATABASE => agent_source(config, &data.agent, &data.reason).into_iter().collect(),
            _ => Vec::new(),
        };
        for source in found {
            if !sources.iter().any(|known| known.id == source.id) {
                sources.push(source);
            }
        }
    }
    if let Some(data) = manifest.data.iter().find(|data| data.kind == FILE) {
        for id in granted.iter().filter(|id| files::is_file(id) && files::parse(id).is_ok()) {
            if !sources.iter().any(|known| known.id == *id) {
                sources.push(file_source(config, id, &data.reason));
            }
        }
    }
    if granted.iter().any(|source| source == FACTORY_SOURCE) {
        let reason = manifest.data.iter().find(|data| data.kind == FACTORY_RECORDS).map_or("", |data| data.reason.as_str());
        sources.push(records(reason));
    }
    sources
}

/// AppInfo.sources: what it can be given, and what it was.
pub(crate) fn infos(config: &Config, manifest: &Manifest, app: &AppConfig) -> Vec<v1::AppDataSource> {
    sources(config, manifest, &app.sources).iter().map(|source| source.info(app.sources.contains(&source.id))).collect()
}

/// What Rivet may propose the app reads (`super::access`): what it asks
/// for, but never all of Claktory's records,
/// nor a curator's database, which apps never read.
pub(crate) fn proposable(factory: &Factory, manifest: &Manifest) -> Vec<v1::AppDataSource> {
    let asked = sources(&factory.lock().config, manifest, &[]);
    asked
        .iter()
        .filter(|source| !matches!(&source.reads, Reads::Agent(agent) if factory.pipes.curator(agent)))
        .map(|source| source.info(false))
        .collect()
}

/// Whether `id`, a file, can be given to the app now: it asks for files,
/// and the file is one an app may read (`files::check`, done again at every
/// read). The source as the card shows it.
pub(crate) fn file_offer(factory: &Factory, manifest: &Manifest, id: &str) -> Result<v1::AppDataSource, String> {
    let Some(data) = manifest.data.iter().find(|data| data.kind == FILE) else {
        return Err(format!("The {} app doesn't read files.", manifest.name));
    };
    let config = factory.lock().config.clone();
    files::check(&config, &factory.home, id)?;
    Ok(file_source(&config, id, &data.reason).info(false))
}

/// ListAppFiles: the files the app could be given, for "Add a file…".
pub(crate) async fn list_files(factory: &Arc<Factory>, request: &v1::ListAppFiles) -> Result<v1::AppFiles, RequestError> {
    let manifest = factory.apps.manifest(&request.app_id).ok_or_else(|| no_app(&request.app_id))?;
    if !manifest.data.iter().any(|data| data.kind == FILE) {
        return Err(RequestError::Apps(format!("The {} app doesn't read files.", manifest.name)));
    }
    let config = factory.lock().config.clone();
    let home = factory.home.clone();
    let query = request.query.chars().take(200).collect::<String>();
    let (files, truncated) = tokio::task::spawn_blocking(move || files::choices(&config, &home, &query))
        .await
        .map_err(|_| RequestError::Apps("The files couldn't be listed.".to_owned()))?;
    Ok(v1::AppFiles { files, truncated })
}

/// The source `id` as it can be given now: one the app asks for (never a
/// curator's database), or a file it may read.
pub(crate) fn offer(factory: &Factory, manifest: &Manifest, id: &str) -> Option<v1::AppDataSource> {
    if files::is_file(id) {
        return file_offer(factory, manifest, id).ok();
    }
    proposable(factory, manifest).into_iter().find(|source| source.id == id)
}

/// AppPermissions.sources: what it asks for, as its manifest says it.
pub(crate) fn requested(config: &Config, manifest: &Manifest) -> Vec<v1::AppDataSource> {
    manifest
        .data
        .iter()
        .map(|data| match data.kind.as_str() {
            FACTORY_RECORDS => records(&data.reason).info(false),
            ACTIVITY => activity(&data.reason).info(false),
            CONVERSATIONS => conversations(&data.reason).info(false),
            SCHEDULE => schedule(&data.reason).info(false),
            FILE => v1::AppDataSource {
                id: format!("{}*", files::PREFIX),
                kind: v1::AppDataKind::File.into(),
                label: "Files you choose (CSV or TSV)".to_owned(),
                reason: data.reason.clone(),
                holds: "Whatever is in each file you choose, which you or any of your agents may have written.".to_owned(),
                private: true,
                ..Default::default()
            },
            _ if data.agent == "*" => v1::AppDataSource {
                id: format!("{AGENT_PREFIX}*"),
                kind: v1::AppDataKind::AgentDatabase.into(),
                agent_id: "*".to_owned(),
                label: "Agents' own databases (you choose which)".to_owned(),
                reason: data.reason.clone(),
                granted: false,
                holds: "Whatever the agents you choose keep in their own databases.".to_owned(),
                private: true,
                given: None,
            },
            _ => agent_source(config, &data.agent, &data.reason).map_or_else(
                || v1::AppDataSource {
                    id: format!("{AGENT_PREFIX}{}", data.agent),
                    kind: v1::AppDataKind::AgentDatabase.into(),
                    agent_id: data.agent.clone(),
                    label: format!("The database of an agent \"{}\", which isn't here", data.agent),
                    reason: data.reason.clone(),
                    granted: false,
                    holds: String::new(),
                    private: true,
                    given: None,
                },
                |source| source.info(false),
            ),
        })
        .collect()
}

/// What an update asks to read that the installed version didn't, for the
/// person (AppPermissions.added).
pub(crate) fn added(before: &Manifest, after: &Manifest) -> Vec<String> {
    after
        .data
        .iter()
        .filter(|data| !before.data.iter().any(|old| old.kind == data.kind && old.agent == data.agent))
        .map(|data| match (data.kind.as_str(), data.agent.as_str()) {
            (FACTORY_RECORDS, _) => "Ask to read all of Claktory's records".to_owned(),
            (ACTIVITY, _) => "Ask to read your agents' activity".to_owned(),
            (CONVERSATIONS, _) => "Ask to read your agents' conversations".to_owned(),
            (SCHEDULE, _) => "Ask to read your agents' schedule".to_owned(),
            (FILE, _) => "Ask to read files you choose".to_owned(),
            (_, "*") => "Ask to read agents' own databases".to_owned(),
            (_, agent) => format!("Ask to read the database of the agent \"{agent}\""),
        })
        .collect()
}

/// The sources as the app sees them (data-sources).
pub(crate) fn list(factory: &Factory, app: &str) -> wire::DataSourceList {
    let Some(manifest) = factory.apps.manifest(app) else { return wire::DataSourceList::default() };
    let inner = factory.lock();
    let Some(installed) = inner.config.apps.iter().find(|installed| installed.id == app) else { return wire::DataSourceList::default() };
    let sources = sources(&inner.config, &manifest, &installed.sources)
        .into_iter()
        .map(|source| wire::DataSource { granted: installed.sources.contains(&source.id), id: source.id, label: source.label })
        .collect();
    wire::DataSourceList { sources }
}

fn answer(error: DataError, message: impl Into<String>) -> wire::DataAnswer {
    wire::DataAnswer { json: String::new(), error: error.into(), message: message.into() }
}

/// An app's query (data-query): its source looked up and its grant checked
/// now, then run read only.
pub(crate) async fn query(factory: &Arc<Factory>, asked: wire::DataQuery) -> wire::DataAnswer {
    // Model calls counted are read with the rest (`crate::calls`).
    factory.store.write_calls();
    let Some(manifest) = factory.apps.manifest(&asked.app_id) else {
        return answer(DataError::UnknownSource, "");
    };
    let source = {
        let inner = factory.lock();
        let Some(installed) = inner.config.apps.iter().find(|installed| installed.id == asked.app_id) else {
            return answer(DataError::UnknownSource, "");
        };
        let Some(source) = sources(&inner.config, &manifest, &installed.sources).into_iter().find(|source| source.id == asked.source)
        else {
            return answer(DataError::UnknownSource, "");
        };
        if !installed.sources.contains(&source.id) {
            return answer(DataError::NotGranted, "");
        }
        source
    };
    // A curator's database holds friends' words: an app could pass them to
    // an agent untainted. The person reads it in
    // the app instead.
    if let Reads::Agent(agent) = &source.reads
        && factory.pipes.curator(agent)
    {
        return answer(DataError::NotGranted, format!("{} holds reports from other Claktories, so apps can't read it.", source.label));
    }
    if let Reads::File(id) = &source.reads {
        return query_file(factory, id, &asked).await;
    }
    let home = factory.home.clone();
    let agents = if source.reads == Reads::Schedule { schedule_agents(&factory.lock().config) } else { Vec::new() };
    // What an agent's call reads of people's words goes back to it in the
    // call's answer: the agent is marked as having read outside content.
    let watch = !asked.caller.is_empty() && holds_others_words(&source.id);
    let read_from = format!("what the {} app read from {}", manifest.name, source.label);
    let ran = tokio::task::spawn_blocking(move || {
        let target = source.target();
        let db = sql::open_read_only(&home, &target).map_err(|why| (DataError::Failed, why))?;
        let Some(mut db) = db else {
            return Err((DataError::Failed, format!("{} has nothing in it yet.", source.label)));
        };
        if source.reads == Reads::Schedule {
            schedule_tables(&mut db, &agents).map_err(|error| (DataError::Failed, error.to_string()))?;
        }
        if let Some(allowed) = source.allowed() {
            limit(&mut db, allowed).map_err(|error| (DataError::Failed, error.to_string()))?;
        }
        let json = run(&db, &target, &asked.sql, &asked.params)?;
        let words = watch && reads_words(&mut db, &source.reads, &asked.sql);
        Ok((json, words, asked.caller))
    })
    .await;
    match ran {
        Ok(Ok((json, words, caller))) => {
            if words {
                crate::errands::read_outside(factory, &caller, &read_from);
            }
            wire::DataAnswer { json, ..Default::default() }
        }
        Ok(Err((error, message))) => answer(error, message),
        Err(_) => answer(DataError::Failed, "The query stopped."),
    }
}

/// The columns of Conversations' messages that hold what someone wrote.
const MESSAGE_WORDS: &[&str] = &["summary", "text", "attachments"];

/// Whether reading `column` of `table` from `reads` takes what people
/// wrote: Conversations' messages' words, or any column of all of
/// Claktory's records. A read of no column (`count(*)`) never does.
fn words(reads: &Reads, table: &str, column: &str) -> bool {
    match reads {
        Reads::Conversations => table == "messages" && MESSAGE_WORDS.contains(&column),
        Reads::Records => !column.is_empty(),
        _ => false,
    }
}

/// Whether `sql`, which just ran in `db`, read what people wrote from
/// `reads` (`words`): SQLite says which columns it reads as it prepares
/// it again, never running it. One it can't say counts as reading them.
fn reads_words(db: &mut rusqlite::Connection, reads: &Reads, sql: &str) -> bool {
    let found = Arc::new(AtomicBool::new(false));
    let (seen, reads) = (found.clone(), reads.clone());
    let watched = db.authorizer(Some(move |context: AuthContext<'_>| {
        if let AuthAction::Read { table_name, column_name } = context.action
            && words(&reads, table_name, column_name)
        {
            seen.store(true, Ordering::Relaxed);
        }
        Authorization::Allow
    }));
    watched.is_err() || db.prepare(sql).is_err() || found.load(Ordering::Relaxed)
}

/// A query of a file: walked, opened and read again now, as a table.
async fn query_file(factory: &Arc<Factory>, id: &str, asked: &wire::DataQuery) -> wire::DataAnswer {
    let place = files::parse(id).and_then(|file| {
        let (folder, words) = files::place(&factory.lock().config, &factory.home, &file.place)?;
        Ok((file, folder, words))
    });
    let (file, folder, words) = match place {
        Ok(found) => found,
        Err(why) => return answer(DataError::Failed, why),
    };
    let home = factory.home.clone();
    let (sql, params) = (asked.sql.clone(), asked.params.clone());
    let ran = tokio::task::spawn_blocking(move || {
        let shown = format!("{} in {words}", file.path);
        let bytes = files::read(&home, &folder, &file, &words).map_err(|why| (DataError::Failed, why))?;
        let db = files::table(&bytes, &file, &shown).map_err(|why| (DataError::Failed, why))?;
        run(&db, &Target::Own(String::new()), &sql, &params).map_err(|(error, message)| {
            if message.starts_with("That isn't in this source") || message.contains("not authorized") {
                (error, format!("A file has one table, {}: only reading it can be run.", files::TABLE))
            } else {
                (error, message)
            }
        })
    })
    .await;
    match ran {
        Ok(Ok(json)) => wire::DataAnswer { json, ..Default::default() },
        Ok(Err((error, message))) => answer(error, message),
        Err(_) => answer(DataError::Failed, "The query stopped."),
    }
}

/// The statement's first word, past spaces and comments.
fn first_word(sql: &str) -> String {
    let mut rest = sql;
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix("--") {
            rest = after.split_once('\n').map_or("", |(_, next)| next);
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after.split_once("*/").map_or("", |(_, next)| next);
        } else {
            break;
        }
    }
    rest.chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_lowercase()
}

/// A failure that isn't the query's own: what an app may not read.
fn refused(error: &rusqlite::Error, target: &Target) -> (DataError, String) {
    let text = error.to_string();
    if text.contains("not authorized") || text.contains("prohibited") {
        return (
            DataError::Invalid,
            "That isn't in this source: it reads only its own tables and columns. Agents' memory, pipes and comments on files can't be read, nor can other databases be attached.".into(),
        );
    }
    (DataError::Invalid, sql::sql_error(error, target))
}

/// Runs one read-only statement: its rows as JSON.
fn run(db: &rusqlite::Connection, target: &Target, text: &str, params: &str) -> Result<String, (DataError, String)> {
    if !matches!(first_word(text).as_str(), "select" | "with") {
        return Err((DataError::Invalid, "Only a SELECT (or WITH) statement can be run.".into()));
    }
    let params: Option<Value> = if params.trim().is_empty() {
        None
    } else {
        Some(serde_json::from_str(params).map_err(|_| (DataError::Invalid, "params must be a JSON list, e.g. [1, \"a\"].".to_owned()))?)
    };
    let values = sql::params(params.as_ref()).map_err(|why| (DataError::Invalid, why))?;
    let mut statement = db.prepare(text).map_err(|error| match error {
        rusqlite::Error::MultipleStatement => (DataError::Invalid, "One statement at a time.".to_owned()),
        error => refused(&error, target),
    })?;
    if !statement.readonly() {
        return Err((DataError::Invalid, "Only statements that read can be run.".into()));
    }
    let columns: Vec<String> = statement.column_names().map(str::to_owned).collect();
    let width = columns.len();
    let mut rows = statement.query(rusqlite::params_from_iter(values)).map_err(|error| refused(&error, target))?;
    let failed = |error: rusqlite::Error| (DataError::Failed, sql::sql_error(&error, target));
    let (mut out, mut bytes, mut more) = (Vec::new(), 0, false);
    while let Some(row) = rows.next().map_err(failed)? {
        let cells: Vec<Value> = (0..width).map(|index| row.get_ref(index).map(cell).unwrap_or(Value::Null)).collect();
        let row = Value::Array(cells);
        let weight = row.to_string().len() + 1;
        if out.len() == MAX_ROWS || bytes + weight > MAX_BYTES {
            more = true;
            break;
        }
        bytes += weight;
        out.push(row);
    }
    Ok(json!({"columns": columns, "rows": out, "more": more}).to_string())
}

fn schedule_agents(config: &Config) -> Vec<ScheduleAgent> {
    config
        .agents
        .iter()
        .map(|agent| ScheduleAgent {
            id: agent.id.clone(),
            name: agent.name.clone(),
            project: agent.project.clone(),
            admin: crate::proposals::is_admin(agent),
            project_name: agent.project.as_deref().and_then(|id| config.project(id)).map(|project| project.name.clone()),
        })
        .collect()
}

/// Makes Schedule's tables (`SCHEDULE_TABLES`) in `db`'s temp database, a
/// copy for this connection alone: the specs of the monitors that only
/// wait for a time, and `agents`. Copied rather than a view over
/// `monitors`, so no statement of the app's can reach `monitors.spec`
/// itself, whatever it calls its own tables.
fn schedule_tables(db: &mut rusqlite::Connection, agents: &[ScheduleAgent]) -> rusqlite::Result<()> {
    db.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)?;
    db.pragma_update(None, "query_only", false)?;
    let made = (|| {
        db.execute_batch(
            "CREATE TEMP TABLE monitor_specs AS SELECT agent, id, kind, spec FROM main.monitors WHERE kind IN ('at', 'every', 'cron');
             CREATE TEMP TABLE agents (id TEXT NOT NULL, name TEXT NOT NULL, project TEXT, admin INTEGER NOT NULL, project_name TEXT);",
        )?;
        let mut insert = db.prepare("INSERT INTO temp.agents (id, name, project, admin, project_name) VALUES (?1, ?2, ?3, ?4, ?5)")?;
        for agent in agents {
            insert.execute(rusqlite::params![agent.id, agent.name, agent.project, agent.admin, agent.project_name])?;
        }
        Ok(())
    })();
    db.pragma_update(None, "query_only", true)?;
    made
}

/// Holds `db` to `allowed`'s tables and columns: every other read, and
/// anything but reading, is refused (not blanked), through views, CTEs and
/// subqueries alike, since SQLite asks about each column it reads from a
/// table, whatever the statement looks like. The private tables stay
/// refused, as everything unlisted is.
fn limit(db: &mut rusqlite::Connection, allowed: &'static Allowed) -> rusqlite::Result<()> {
    let own: Vec<String> = db.prepare("SELECT name FROM sqlite_schema")?.query_map([], |row| row.get(0))?.collect::<Result<_, _>>()?;
    db.authorizer(Some(move |context: AuthContext<'_>| allows(allowed, &own, context.database_name, &context.action)))
}

/// Whether `table`, in a read of no column, is a WITH clause's or a
/// subquery's result rather than one of the database's own tables (`own`):
/// SQLite asks so when it takes no value from it, as `NOT EXISTS (SELECT 1
/// FROM c)` does from a grouped WITH used twice. What it holds
/// came from reads already checked, so counting its rows shows nothing more.
/// sqlite_ and pragma_ names are never one.
pub(super) fn derived(table: &str, own: &[impl AsRef<str>]) -> bool {
    let lower = table.to_ascii_lowercase();
    !lower.starts_with("sqlite_") && !lower.starts_with("pragma_") && !own.iter().any(|name| name.as_ref().eq_ignore_ascii_case(table))
}

/// Whether `action`, in `database` ("main", "temp"), is one `allowed` lets
/// through.
fn allows(allowed: &Allowed, own: &[String], database: Option<&str>, action: &AuthAction<'_>) -> Authorization {
    match action {
        AuthAction::Read { table_name, column_name: "" } if derived(table_name, own) => Authorization::Allow,
        AuthAction::Select | AuthAction::Recursive => Authorization::Allow,
        AuthAction::Function { function_name } if !function_name.eq_ignore_ascii_case("load_extension") => Authorization::Allow,
        AuthAction::Read { table_name, column_name } => {
            let lists: &[&[(&str, &[&str])]] = if database == Some("temp") { &[allowed.temp] } else { allowed.main };
            // "" is a read of no column (count(*)): only the table's rows.
            let listed = lists.iter().flat_map(|list| list.iter()).any(|(table, columns)| {
                table == table_name && (column_name.is_empty() || columns.contains(column_name))
            });
            if listed { Authorization::Allow } else { Authorization::Deny }
        }
        _ => Authorization::Deny,
    }
}

fn cell(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(integer) => json!(integer),
        ValueRef::Real(real) => serde_json::Number::from_f64(real).map_or(Value::Null, Value::Number),
        ValueRef::Text(text) => Value::String(String::from_utf8_lossy(text).into_owned()),
        ValueRef::Blob(blob) => Value::String(STANDARD.encode(blob)),
    }
}

/// Whether the app may be given source `id` now, `had` being what it
/// reads: one it asks for, and never all of Claktory's records again
/// (Activity and Conversations say what they hold).
pub(crate) fn can_give(config: &Config, manifest: &Manifest, had: &[String], id: &str) -> Result<(), String> {
    // A file: well named, to an app that reads files; who gives it checks
    // the file itself (`file_offer`), and every read does again.
    if files::is_file(id) {
        if !manifest.data.iter().any(|data| data.kind == FILE) {
            return Err(format!("The {} app doesn't read files.", manifest.name));
        }
        return files::parse(id).map(|_| ());
    }
    if !sources(config, manifest, had).iter().any(|source| source.id == id) {
        return Err(format!("The {} app doesn't ask to read \"{id}\".", manifest.name));
    }
    if id == FACTORY_SOURCE && !had.iter().any(|source| source == id) {
        return Err("Everything Claktory keeps can't be given any more: give Activity or Conversations.".to_owned());
    }
    Ok(())
}

impl Factory {
    /// GrantAppSource: the person gives the app a source it asks for, or
    /// takes it back; kept as a change of theirs.
    pub(crate) async fn grant_app_source(self: &Arc<Self>, request: &v1::GrantAppSource) -> Result<(), RequestError> {
        let manifest = self.apps.manifest(&request.app_id).ok_or_else(|| no_app(&request.app_id))?;
        let new_file = {
            let inner = self.lock();
            let app = inner.config.apps.iter().find(|app| app.id == request.app_id).ok_or_else(|| no_app(&request.app_id))?;
            let new_file = files::is_file(&request.source_id) && request.granted && !app.sources.contains(&request.source_id);
            if !new_file && !sources(&inner.config, &manifest, &app.sources).iter().any(|source| source.id == request.source_id) {
                return Err(RequestError::Apps(format!("The {} app doesn't ask to read \"{}\".", manifest.name, request.source_id)));
            }
            new_file
        };
        // A file the person picked: one an app may read.
        if new_file {
            file_offer(self, &manifest, &request.source_id).map_err(RequestError::Apps)?;
        }
        let to = super::changes::Access { sources: [(request.source_id.clone(), request.granted)].into(), ..Default::default() };
        super::changes::apply(self, &request.app_id, &to, super::changes::How::YOU).await?;
        Ok(())
    }
}

/// An agent is gone: no app reads its database any more.
pub(crate) fn forget_agent(app: &mut AppConfig, gone: &str) {
    let id = format!("{AGENT_PREFIX}{gone}");
    app.sources.retain(|source| *source != id);
}

/// Whether source `id` holds what other Claktories wrote: the messages
/// pipes delivered, and what agents that had read them sent. Conversations
/// and all of Claktory's records do; Activity has no text.
pub(crate) fn holds_others_words(id: &str) -> bool {
    id == CONVERSATIONS_SOURCE || id == FACTORY_SOURCE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> rusqlite::Connection {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE runs (day TEXT, n INTEGER, cost REAL, raw BLOB);
             INSERT INTO runs VALUES ('mon', 3, 1.5, x'00ff'), ('tue', 4, NULL, NULL);",
        )
        .unwrap();
        db
    }

    #[test]
    fn what_people_wrote_is_told_from_counts_and_times() {
        let read = |reads: Reads, sql: &str| {
            let mut db = rusqlite::Connection::open_in_memory().unwrap();
            db.execute_batch("CREATE TABLE messages (id INTEGER, created INTEGER, summary TEXT, text TEXT, attachments TEXT)").unwrap();
            reads_words(&mut db, &reads, sql)
        };
        for sql in [
            "SELECT text FROM messages",
            "SELECT id FROM messages WHERE summary LIKE '%key%'",
            "SELECT count(attachments) FROM messages",
            "WITH m AS (SELECT Text AS t FROM messages) SELECT count(*) FROM m",
            "SELECT * FROM messages",
        ] {
            assert!(read(Reads::Conversations, sql), "{sql}");
            assert!(read(Reads::Records, sql), "{sql}");
        }
        for sql in ["SELECT count(*) FROM messages", "SELECT 1", "SELECT id, created FROM messages"] {
            assert!(!read(Reads::Conversations, sql), "{sql}");
        }
        assert!(!read(Reads::Records, "SELECT count(*) FROM messages"));
        assert!(read(Reads::Records, "SELECT created FROM messages"), "all of the records: any column");
        assert!(read(Reads::Conversations, "SELECT nothing FROM nowhere"), "what can't be told counts");
        for reads in [Reads::Activity, Reads::Schedule, Reads::Agent("lea".into())] {
            assert!(!read(reads, "SELECT text FROM messages"));
        }
    }

    #[test]
    fn one_read_only_statement_answers_its_rows_as_json() {
        let db = db();
        let target = Target::Own("lea".into());
        let answer: Value =
            serde_json::from_str(&run(&db, &target, "-- per day\nSELECT day, n, cost, raw FROM runs WHERE n > ?1", "[2]").unwrap())
                .unwrap();
        assert_eq!(answer["columns"], json!(["day", "n", "cost", "raw"]));
        assert_eq!(answer["rows"], json!([["mon", 3, 1.5, "AP8="], ["tue", 4, null, null]]));
        assert_eq!(answer["more"], false);
        let with: Value =
            serde_json::from_str(&run(&db, &target, "WITH d AS (SELECT day FROM runs) SELECT count(*) FROM d", "").unwrap()).unwrap();
        assert_eq!(with["rows"], json!([[2]]));

        for (sql, params, why) in [
            ("DELETE FROM runs", "", "Only a SELECT"),
            ("SELECT 1; DELETE FROM runs", "", "One statement at a time"),
            ("SELECT 1; SELECT 2", "", "One statement at a time"),
            ("PRAGMA table_info(runs)", "", "Only a SELECT"),
            ("WITH d AS (SELECT 1) DELETE FROM runs", "", "Only statements that read"),
            ("SELECT ?1", "{\"a\": 1}", "params must be a list"),
            ("SELECT ?1", "[", "params must be a JSON list"),
        ] {
            let (error, message) = run(&db, &target, sql, params).unwrap_err();
            assert_eq!(error, DataError::Invalid, "{sql}");
            assert!(message.contains(why), "{sql}: {message}");
        }
        let count: i64 = db.query_row("SELECT count(*) FROM runs", [], |row| row.get(0)).unwrap();
        assert_eq!(count, 2, "nothing was deleted");
    }

    /// A database shaped like factory.db's messages and runs, with a view
    /// over the text, as a later schema might have.
    fn records() -> rusqlite::Connection {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE messages (id INTEGER PRIMARY KEY, agent TEXT, text TEXT, created INTEGER);
             INSERT INTO messages VALUES (1, 'lea', 'my bank password is hunter2', 5), (2, 'lea', 'hi', 6);
             CREATE TABLE runs (id INTEGER, agent TEXT, started INTEGER);
             INSERT INTO runs VALUES (1, 'lea', 5);
             CREATE TABLE memory_notes (body TEXT);
             CREATE VIEW said AS SELECT agent, text FROM messages;",
        )
        .unwrap();
        db
    }

    #[test]
    fn activity_reads_only_its_columns_however_the_statement_is_written() {
        let mut db = records();
        limit(&mut db, &ACTIVITY_ALLOWED).unwrap();
        let target = Target::Factory;
        let rows = |sql: &str| -> Value { serde_json::from_str::<Value>(&run(&db, &target, sql, "").unwrap()).unwrap()["rows"].clone() };
        assert_eq!(rows("SELECT agent, COUNT(*) FROM messages GROUP BY agent"), json!([["lea", 2]]));
        assert_eq!(rows("SELECT count(*) FROM messages"), json!([[2]]));
        assert_eq!(rows("SELECT r.agent FROM runs r JOIN messages m ON m.agent = r.agent WHERE m.id = 1"), json!([["lea"]]));
        assert_eq!(rows("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n LIMIT 3) SELECT sum(i) FROM n"), json!([[6]]));
        // A grouped WITH used twice is materialised, and NOT EXISTS reads it
        // with no column.
        let twice = "WITH c AS (SELECT agent, count(*) AS n FROM messages GROUP BY agent) \
                     SELECT agent, n FROM c UNION ALL SELECT 'none', 0 WHERE NOT EXISTS (SELECT 1 FROM c)";
        assert_eq!(rows(twice), json!([["lea", 2]]));
        for sql in [
            "SELECT text FROM messages",
            "SELECT * FROM messages",
            "SELECT length(text) FROM messages",
            "SELECT id FROM messages WHERE text LIKE '%password%'",
            "SELECT text FROM said",
            "SELECT agent FROM said",
            "WITH t AS (SELECT text FROM messages) SELECT * FROM t",
            "SELECT (SELECT text FROM messages LIMIT 1)",
            "SELECT agent FROM (SELECT agent, text FROM messages)",
            "SELECT body FROM memory_notes",
            "SELECT name FROM sqlite_master",
            "SELECT * FROM pragma_table_info('messages')",
            "SELECT count(*) FROM memory_notes",
            "SELECT count(*) FROM said",
            "SELECT count(*) FROM sqlite_schema",
            "SELECT count(*) FROM pragma_table_info('memory_notes')",
            "WITH c AS (SELECT 1 FROM memory_notes) SELECT count(*) FROM c",
            "WITH c AS (SELECT text FROM messages GROUP BY text) SELECT 1 FROM c UNION ALL SELECT 2 WHERE NOT EXISTS (SELECT 1 FROM c)",
        ] {
            let (error, message) = run(&db, &target, sql, "").unwrap_err();
            assert_eq!(error, DataError::Invalid, "{sql}");
            assert!(message.contains("isn't in this source"), "{sql}: {message}");
        }
    }

    #[test]
    fn conversations_read_the_messages_and_nothing_else() {
        let mut db = records();
        limit(&mut db, &CONVERSATIONS_ALLOWED).unwrap();
        let target = Target::Factory;
        let said: Value = serde_json::from_str(&run(&db, &target, "SELECT text FROM messages ORDER BY id", "").unwrap()).unwrap();
        assert_eq!(said["rows"], json!([["my bank password is hunter2"], ["hi"]]));
        for sql in ["SELECT agent FROM runs", "SELECT body FROM memory_notes"] {
            assert_eq!(run(&db, &target, sql, "").unwrap_err().0, DataError::Invalid, "{sql}");
        }
    }

    /// Widening Activity is a reviewed change: this is the list, and no
    /// column of it holds what someone wrote.
    #[test]
    fn activity_is_this_list() {
        let listed: Vec<String> =
            ACTIVITY_READS.iter().flat_map(|(table, columns)| columns.iter().map(move |column| format!("{table}.{column}"))).collect();
        assert_eq!(
            listed.join(" "),
            "runs.id runs.agent runs.version runs.started runs.ended runs.outcome runs.last_call \
             agent_usage.hour agent_usage.provider agent_usage.model agent_usage.agent agent_usage.agent_name agent_usage.requests agent_usage.input agent_usage.output agent_usage.cache_read agent_usage.cache_write \
             model_usage.hour model_usage.provider model_usage.model model_usage.requests model_usage.input model_usage.output model_usage.cache_read model_usage.cache_write \
             agent_presence.agent agent_presence.idle_since agent_presence.failed \
             tasks.id tasks.agent tasks.task tasks.started tasks.ended tasks.status \
             monitors.agent monitors.id monitors.kind monitors.until monitors.max_fires monitors.idle_only monitors.created_by monitors.created monitors.state monitors.next_due monitors.last_fired monitors.fires monitors.updated \
             monitor_events.seq monitor_events.agent monitor_events.monitor monitor_events.time monitor_events.actor monitor_events.kind \
             checklist_items.agent checklist_items.id checklist_items.status checklist_items.owner checklist_items.created checklist_items.updated checklist_items.blocked_since checklist_items.goal checklist_items.blocked_kind checklist_items.retries checklist_items.dismissed_at \
             checklist_events.seq checklist_events.agent checklist_events.item checklist_events.time checklist_events.actor checklist_events.kind \
             messages.id messages.agent messages.source messages.sender messages.sender_id messages.created messages.delivered messages.run messages.dropped messages.sender_run messages.intent"
        );
        for never in [
            "text",
            "summary",
            "command",
            "log",
            "prompt",
            "spec",
            "value",
            "reason",
            "name",
            "account",
            "attachments",
            "voice",
            "blocked_on",
        ] {
            assert!(!listed.iter().any(|column| column.ends_with(&format!(".{never}"))), "{never}");
        }
    }

    /// Every table and column the lists name is in factory.db as Claktory
    /// makes it, so a rename shows here rather than as a chart that fails.
    #[test]
    fn the_lists_name_factory_dbs_own_columns() {
        let dir = std::env::temp_dir().join(format!("factoryd-app-data-{}-{}", std::process::id(), crate::transcript::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = crate::store::Store::open(&dir.join("factory.db")).unwrap();
        let db = store.db();
        for (table, columns) in ACTIVITY_READS.iter().chain(CONVERSATIONS_READS).chain(SCHEDULE_READS) {
            let have: Vec<String> =
                crate::store::query(&db, &format!("SELECT name FROM pragma_table_info('{table}')"), [], |row| row.get(0)).unwrap();
            for column in *columns {
                assert!(have.iter().any(|name| name == column), "{table}.{column} isn't in factory.db: {have:?}");
            }
        }
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Schedule reads what names things and when they're due, and a
    /// monitor's spec only when it waits for a time: never a check's or a
    /// watch's command, nor a monitor's prompt, however the statement asks.
    #[test]
    fn schedule_reads_names_and_times_but_never_a_monitors_command() {
        let dir = std::env::temp_dir().join(format!("factoryd-app-schedule-{}-{}", std::process::id(), crate::transcript::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("factory.db");
        let store = crate::store::Store::open(&path).unwrap();
        store
            .db()
            .execute_batch(
                "INSERT INTO monitors (agent, id, name, prompt, kind, spec, idle_only, created_by, created, state, reason, fires, carry, cursor, updated)
                 VALUES ('lea', 1, 'Logs', 'read the logs', 'every', '{\"every\":\"6h\"}', 1, '', 0, 'active', '', 0, '', '{}', 0),
                        ('lea', 2, 'Disk', 'say if full', 'check', '{\"check\":\"curl -H token:hunter2 x\"}', 1, '', 0, 'active', '', 0, '', '{}', 0),
                        ('ana', 3, 'Release', 'ship', 'at', '{\"at\":1796000000}', 1, '', 0, 'active', '', 0, '', '{}', 0);
                 INSERT INTO checklist_items (agent, id, text, status, owner, created, updated, retry_at) VALUES ('lea', 1, 'Ship v2', 'open', '', 0, 0, 1796000000);",
            )
            .unwrap();
        drop(store);
        let before = std::fs::read(&path).unwrap();
        let mut db = rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        db.pragma_update(None, "query_only", true).unwrap();
        let agents = [
            ScheduleAgent { id: "lea".into(), name: "Lea".into(), project: Some("site".into()), admin: false, project_name: Some("Web Site".into()) },
            ScheduleAgent { id: "rivet".into(), name: "Rivet".into(), project: None, admin: true, project_name: None },
        ];
        schedule_tables(&mut db, &agents).unwrap();
        limit(&mut db, &SCHEDULE_ALLOWED).unwrap();
        let target = Target::Factory;
        let rows = |sql: &str| -> Value { serde_json::from_str::<Value>(&run(&db, &target, sql, "").unwrap()).unwrap()["rows"].clone() };
        assert_eq!(rows("SELECT name, kind, next_due FROM monitors ORDER BY id"), json!([["Logs", "every", null], ["Disk", "check", null], ["Release", "at", null]]));
        assert_eq!(
            rows("SELECT agent, id, kind, spec FROM monitor_specs ORDER BY id"),
            json!([["lea", 1, "every", "{\"every\":\"6h\"}"], ["ana", 3, "at", "{\"at\":1796000000}"]])
        );
        assert_eq!(rows("SELECT text, retry_at, reask_at FROM checklist_items"), json!([["Ship v2", 1796000000, null]]));
        assert_eq!(
            rows("SELECT id, name, project, admin, project_name FROM agents ORDER BY id"),
            json!([["lea", "Lea", "site", 0, "Web Site"], ["rivet", "Rivet", null, 1, null]])
        );
        assert_eq!(
            rows("SELECT a.project, m.name FROM monitor_specs s JOIN monitors m USING (agent, id) JOIN agents a ON a.id = s.agent"),
            json!([["site", "Logs"]])
        );
        for sql in [
            "SELECT spec FROM monitors",
            "SELECT spec FROM main.monitors",
            "SELECT * FROM monitors",
            "SELECT prompt FROM monitors",
            "SELECT carry, cursor, reason FROM monitors",
            "WITH monitor_specs AS (SELECT agent, spec FROM main.monitors) SELECT * FROM monitor_specs",
            "SELECT * FROM (SELECT spec FROM monitors) AS monitor_specs",
            "SELECT id FROM monitors WHERE spec LIKE '%hunter2%'",
            "SELECT text FROM messages",
            "SELECT blocked_on FROM checklist_items",
        ] {
            let (error, message) = run(&db, &target, sql, "").unwrap_err();
            assert_eq!(error, DataError::Invalid, "{sql}");
            assert!(message.contains("isn't in this source"), "{sql}: {message}");
        }
        assert_eq!(run(&db, &target, "DELETE FROM temp.agents", "").unwrap_err().0, DataError::Invalid);
        drop(db);
        assert_eq!(std::fs::read(&path).unwrap(), before, "factory.db as it was");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Widening Schedule is a reviewed change, as Activity's is.
    #[test]
    fn schedule_is_this_list() {
        let listed = |list: &[(&str, &[&str])]| -> String {
            list.iter().flat_map(|(table, columns)| columns.iter().map(move |column| format!("{table}.{column}"))).collect::<Vec<_>>().join(" ")
        };
        assert_eq!(listed(SCHEDULE_READS), "monitors.name checklist_items.text checklist_items.retry_at checklist_items.reask_at");
        assert_eq!(
            listed(SCHEDULE_TABLES),
            "monitor_specs.agent monitor_specs.id monitor_specs.kind monitor_specs.spec agents.id agents.name agents.project agents.admin agents.project_name"
        );
    }

    #[test]
    fn answers_stop_at_their_caps() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        let target = Target::Factory;
        let many: Value = serde_json::from_str(
            &run(&db, &target, "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n LIMIT 10001) SELECT i FROM n", "").unwrap(),
        )
        .unwrap();
        assert_eq!((many["rows"].as_array().unwrap().len(), &many["more"]), (MAX_ROWS, &json!(true)));
        let big: Value = serde_json::from_str(
            &run(
                &db,
                &target,
                "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n LIMIT 100) SELECT zeroblob(100000) FROM n",
                "",
            )
            .unwrap(),
        )
        .unwrap();
        assert!(big["rows"].as_array().unwrap().len() < 100 && big["more"] == json!(true));
        assert!(big.to_string().len() <= MAX_BYTES + 1024);
    }
}
