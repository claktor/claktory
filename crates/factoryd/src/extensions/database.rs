//! The Database extension: a SQLite database of the agent's own, for
//! structured data it collects or keeps track of, made on first use in
//! `<home>/agents/<id>/database.sqlite`.
//!
//! Its instance "database:factory" is Factory's own records, factory.db, to
//! read: admin agents such as Rivet have it unless it's turned off. Agents'
//! memory (the memory_ tables) stays out of it.
//!
//! Which database a call means, and whether the agent may use it, is decided
//! in one place, `access`, and every statement runs through `sql::open`, so
//! sharing a database between agents can hook in there later.

pub(crate) mod sql;

use std::sync::Arc;

use factory_agent::{Tool, ToolResult};
use factory_protocol::v1;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Extension, InstanceDefault, InstanceSpec, Manifest, PromptKind, PromptSection, enabled};
use crate::config::AgentConfig;
use crate::factory::{Factory, Inner};
use sql::Target;

pub(crate) const ID: &str = "database";

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Database",
    icon: "database",
    description: "Gives it a database of its own for records, lists and results it keeps over time. You can browse its tables.",
    default_enabled: false,
    options: &[],
};

static INSTANCES: [InstanceSpec; 1] = [InstanceSpec {
    key: "database:factory",
    name: "factory",
    label: "Claktory's records",
    icon: "database",
    description: "Lets it look through everything that happened in Claktory: messages, runs, tasks and checklists, but not memories. Rivet has it; turn it on for agents that look into problems.",
    default: InstanceDefault::Admin,
}];
const FACTORY: &InstanceSpec = &INSTANCES[0];

const PROMPT: &str = "## Database

You have a SQLite database of your own for structured data you collect or keep track of over time (records, lists, results) and want to query rather than reread. db_exec changes it (CREATE TABLE, INSERT, UPDATE, DELETE; several statements at once, applied all or none), db_query reads it (one SELECT), db_schema shows its tables; `params` fill ?1, ?2… It lasts across compactions and /clear, only you use it, and the person sees its tables beside your conversation. Results are capped at 200 rows, so select the columns and rows you need. Documents and code still belong in files.
";

const RECORDS_PROMPT: &str = "## Claktory's records

db_query and db_schema read Claktory's own records, factory.db{instance}: every message an agent was sent (`messages`, by `agent`, the one it was sent to: the person's and events, when each was created and delivered), each agent's runs (`runs`), background tasks (`tasks`) and checklists (`checklist_items`, `checklist_events`), as Claktory writes them. Use them to see what happened, such as what an agent was sent and when, or why a run failed. They're read only, and agents' memory isn't among them. Times are milliseconds since 1970.
";

const QUERY: &str = "Read your database with one SELECT (or WITH … SELECT). `params` fill ?1, ?2… Shows a header line and up to 200 rows, cells joined with \" | \", and how many more there were.";
const QUERY_RECORDS: &str = "Read Claktory's records (factory.db) with one SELECT (or WITH … SELECT). `params` fill ?1, ?2… Shows a header line and up to 200 rows, cells joined with \" | \", and how many more there were.";
const QUERY_BOTH: &str = "Read your database, or with instance \"factory\" Claktory's records (factory.db), with one SELECT (or WITH … SELECT). `params` fill ?1, ?2… Shows a header line and up to 200 rows, cells joined with \" | \", and how many more there were.";
const EXEC: &str = "Change your database: CREATE TABLE, INSERT, UPDATE, DELETE and the like. Without params, sql may hold several statements; with params (filling ?1, ?2…), one. All of it is applied or none; leave out BEGIN and COMMIT. The database is made on first use.";
const SCHEMA: &str =
    "Your database's tables and views, each with its row count and columns; with `table`, its CREATE statement and indexes.";
const SCHEMA_RECORDS: &str =
    "The tables of Claktory's records (factory.db), each with its row count and columns; with `table`, its CREATE statement and indexes.";
const SCHEMA_BOTH: &str = "Your database's tables and views (or with instance \"factory\" those of Claktory's records, factory.db), each with its row count and columns; with `table`, its CREATE statement and indexes.";

pub(crate) struct Database;

/// What a tool does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Query,
    Exec,
    Schema,
}

#[derive(Deserialize)]
struct Args {
    #[serde(default)]
    sql: Option<String>,
    #[serde(default)]
    params: Option<Value>,
    #[serde(default)]
    table: Option<String>,
    #[serde(default)]
    instance: Option<String>,
}

/// Whether the agent has its own database on, and Factory's records. A
/// curator (`crate::pipes::curator`) always has its own and never the
/// records, whatever its settings say: it reads friends' words, and its
/// database is where it keeps them.
fn on(factory: &Factory, agent: &str) -> (bool, bool) {
    let curator = factory.pipes.curator(agent);
    let inner = factory.lock();
    let config = inner.config.agent(agent);
    (curator || config.is_some_and(|config| enabled(config, ID)), !curator && config.is_some_and(|config| enabled(config, FACTORY.key)))
}

/// Which database a call of `agent`'s means (`instance` as it gave it), and
/// whether it may use it so: the one place that decides.
fn access(factory: &Factory, agent: &str, instance: Option<&str>, write: bool) -> Result<Target, String> {
    let (own, records) = on(factory, agent);
    match instance.map(str::trim).filter(|instance| !instance.is_empty()) {
        Some(name) if name == FACTORY.name && !records => Err("Claktory's records aren't on for you.".to_owned()),
        Some(name) if name == FACTORY.name && write => Err("Claktory's records are read only.".to_owned()),
        Some(name) if name == FACTORY.name => Ok(Target::Factory),
        Some(other) => {
            Err(format!("instance is \"{}\" for Claktory's records, or left out for your own database; got \"{other}\".", FACTORY.name))
        }
        None if own => Ok(Target::Own(agent.to_owned())),
        None if records && write => Err("Claktory's records are read only, and you have no database of your own.".to_owned()),
        None if records => Ok(Target::Factory),
        None => Err("The Database extension is turned off for you.".to_owned()),
    }
}

/// Runs one call against `target`. Blocking: SQLite works synchronously.
fn run(home: &crate::home::Home, target: &Target, kind: Kind, args: &Args) -> Result<sql::Answer, String> {
    let sql = || args.sql.as_deref().map(str::trim).filter(|sql| !sql.is_empty()).ok_or_else(|| "sql is empty.".to_owned());
    let table = args.table.as_deref().map(str::trim).filter(|table| !table.is_empty());
    let Some(mut db) = sql::open(home, target, kind == Kind::Exec)? else {
        // The agent's own database, before its first write.
        return match (kind, table) {
            (Kind::Schema, None) => {
                Ok(sql::Answer { text: "No tables yet; create them with db_exec.".to_owned(), details: json!({"tables": 0}) })
            }
            _ => Err("Your database has no tables yet; create them with db_exec.".to_owned()),
        };
    };
    match kind {
        Kind::Query => sql::query(&db, target, sql()?, args.params.as_ref()),
        Kind::Exec => sql::exec(&mut db, target, sql()?, args.params.as_ref()),
        Kind::Schema => sql::schema(&db, target, table),
    }
}

async fn call(factory: &Factory, agent: &str, kind: Kind, args: Value) -> Result<ToolResult, String> {
    let args: Args = serde_json::from_value(args).map_err(|error| error.to_string())?;
    let target = access(factory, agent, args.instance.as_deref(), kind == Kind::Exec)?;
    let instance = if target == Target::Factory { FACTORY.name } else { "" };
    let home = factory.home.clone();
    let done = tokio::task::spawn_blocking(move || run(&home, &target, kind, &args))
        .await
        .map_err(|error| format!("The database call failed: {error}"))?;
    if kind == Kind::Exec {
        // Its tables may have changed: the panel follows.
        factory.refresh_view(agent);
    }
    let mut answer = done?;
    answer.details["instance"] = json!(instance);
    Ok(ToolResult::text(answer.text, answer.details))
}

fn db_tool(
    factory: &Arc<Factory>,
    agent: &str,
    name: &'static str,
    label: &'static str,
    description: &'static str,
    parameters: Value,
    kind: Kind,
) -> Tool {
    super::tool(
        name,
        label,
        description,
        parameters,
        super::agent_handler(factory, agent, move |factory, agent, args| async move {
            let factory = super::live(factory)?;
            call(&factory, &agent, kind, args).await.map_err(factory_agent::Error::Other)
        }),
    )
}

/// The panel: `target`'s tables and views, and where its file is.
fn database_view(factory: &Factory, agent: &str, target: &Target, key: &str, title: &str) -> v1::ExtensionView {
    let tables = match sql::open(&factory.home, target, false) {
        Ok(Some(db)) => sql::tables(&db, target).unwrap_or_else(|error| {
            tracing::error!(%agent, %error, "could not list the database's tables");
            Vec::new()
        }),
        Ok(None) => Vec::new(),
        Err(error) => {
            tracing::error!(%agent, %error, "could not open the database");
            Vec::new()
        }
    };
    let tables = tables
        .into_iter()
        .map(|table| v1::DatabaseTable {
            name: table.name,
            view: table.kind == "view",
            rows: table.rows,
            columns: table.columns.into_iter().map(|(name, r#type)| v1::DatabaseColumn { name, r#type }).collect(),
        })
        .collect();
    v1::ExtensionView {
        extension_id: key.to_owned(),
        agent_id: agent.to_owned(),
        title: title.to_owned(),
        body: Some(v1::extension_view::Body::Database(v1::Database {
            tables,
            path: sql::path(&factory.home, target).display().to_string(),
        })),
    }
}

impl Extension for Database {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn prompt(&self, factory: &Factory, _inner: &Inner, agent: &AgentConfig) -> Vec<PromptSection> {
        let curator = factory.pipes.curator(&agent.id);
        let own = curator || enabled(agent, ID);
        let mut sections = Vec::new();
        if own {
            sections.push(PromptSection { kind: PromptKind::Static, text: PROMPT.to_owned() });
        }
        if !curator && enabled(agent, FACTORY.key) {
            let instance = if own { ", with instance: \"factory\"" } else { "" };
            sections.push(PromptSection { kind: PromptKind::Static, text: RECORDS_PROMPT.replace("{instance}", instance) });
        }
        sections
    }

    fn tools(&self, factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
        let (own, records) = on(factory, agent);
        // Optional parameters are the ones left out of `required` (see `super::tool`).
        let params = json!({"type": "array", "items": {"type": ["string", "number", "boolean", "null"]}});
        let mut query = json!({
            "type": "object",
            "required": ["sql"],
            "properties": {"sql": {"type": "string"}, "params": params},
            "additionalProperties": false,
        });
        let exec = query.clone();
        let mut schema = json!({
            "type": "object",
            "properties": {"table": {"type": "string"}},
            "additionalProperties": false,
        });
        if own && records {
            for parameters in [&mut query, &mut schema] {
                parameters["properties"]["instance"] = json!({"type": "string", "enum": [FACTORY.name]});
            }
        }
        let (query_about, schema_about) = match (own, records) {
            (true, true) => (QUERY_BOTH, SCHEMA_BOTH),
            (false, true) => (QUERY_RECORDS, SCHEMA_RECORDS),
            _ => (QUERY, SCHEMA),
        };
        let mut tools = vec![db_tool(factory, agent, "db_query", "Query database", query_about, query, Kind::Query)];
        if own {
            tools.push(db_tool(factory, agent, "db_exec", "Change database", EXEC, exec, Kind::Exec));
        }
        tools.push(db_tool(factory, agent, "db_schema", "Database tables", schema_about, schema, Kind::Schema));
        tools
    }

    fn owns_tool(&self, name: &str) -> bool {
        name.starts_with("db_")
    }

    fn view(&self, factory: &Factory, _inner: &Inner, agent: &str) -> Option<v1::ExtensionView> {
        Some(database_view(factory, agent, &Target::Own(agent.to_owned()), ID, MANIFEST.name))
    }

    fn instances(&self) -> &'static [InstanceSpec] {
        &INSTANCES
    }

    fn instance_view(&self, factory: &Factory, _inner: &Inner, agent: &str, instance: &'static InstanceSpec) -> Option<v1::ExtensionView> {
        Some(database_view(factory, agent, &Target::Factory, instance.key, instance.label))
    }
}
