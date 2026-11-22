//! Data apps read: the sources their manifest's `[[data]]` asks for
//! (Claktory's records, agents' own databases), each read only once the
//! person grants it (`AppConfig.sources`), and checked again on every
//! query. Queries run here, in factoryd, never in the app's sandbox: one
//! read-only SELECT or WITH statement, bound params, the Database
//! extension's authorizer (agents' memory, pipes and comments on files
//! stay unreadable), 10 s, and at most 10,000 rows and 2 MB an answer.

use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use factory_protocol::apps::{self as wire, DataError};
use factory_protocol::v1;
use rusqlite::types::ValueRef;
use serde_json::{Value, json};

use super::bundle::{AGENT_DATABASE, FACTORY_RECORDS, Manifest};
use super::{AppConfig, no_app};
use crate::config::Config;
use crate::extensions::database::sql::{self, Target};
use crate::factory::{Factory, RequestError};

/// Claktory's records' source id.
pub(crate) const FACTORY_SOURCE: &str = "factory-records";
/// An agent's database's source id: this, then the agent's id.
const AGENT_PREFIX: &str = "agent:";
const MAX_ROWS: usize = 10_000;
const MAX_BYTES: usize = 2 << 20;

/// A source the app can be given, as things are now.
struct Source {
    id: String,
    kind: v1::AppDataKind,
    agent: String,
    label: String,
    reason: String,
}

impl Source {
    fn info(&self, granted: bool) -> v1::AppDataSource {
        v1::AppDataSource {
            id: self.id.clone(),
            kind: self.kind.into(),
            agent_id: self.agent.clone(),
            label: self.label.clone(),
            reason: self.reason.clone(),
            granted,
        }
    }

    fn target(&self) -> Target {
        if self.kind == v1::AppDataKind::FactoryRecords { Target::Factory } else { Target::Own(self.agent.clone()) }
    }
}

fn records(reason: &str) -> Source {
    Source {
        id: FACTORY_SOURCE.to_owned(),
        kind: v1::AppDataKind::FactoryRecords,
        agent: String::new(),
        label: "Claktory's records".to_owned(),
        reason: reason.to_owned(),
    }
}

fn agent_source(config: &Config, agent: &str, reason: &str) -> Option<Source> {
    let found = config.agent(agent)?;
    Some(Source {
        id: format!("{AGENT_PREFIX}{agent}"),
        kind: v1::AppDataKind::AgentDatabase,
        agent: agent.to_owned(),
        label: format!("{}'s database", found.name),
        reason: reason.to_owned(),
    })
}

/// The sources `manifest` asks for: one for each agent there is when it
/// asks for any agent's database.
fn sources(config: &Config, manifest: &Manifest) -> Vec<Source> {
    let mut sources: Vec<Source> = Vec::new();
    for data in &manifest.data {
        let found: Vec<Source> = match data.kind.as_str() {
            FACTORY_RECORDS => vec![records(&data.reason)],
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
    sources
}

/// AppInfo.sources: what it can be given, and what it was.
pub(crate) fn infos(config: &Config, manifest: &Manifest, app: &AppConfig) -> Vec<v1::AppDataSource> {
    sources(config, manifest).iter().map(|source| source.info(app.sources.contains(&source.id))).collect()
}

/// AppPermissions.sources: what it asks for, as its manifest says it.
pub(crate) fn requested(config: &Config, manifest: &Manifest) -> Vec<v1::AppDataSource> {
    manifest
        .data
        .iter()
        .map(|data| match data.kind.as_str() {
            FACTORY_RECORDS => records(&data.reason).info(false),
            _ if data.agent == "*" => v1::AppDataSource {
                id: format!("{AGENT_PREFIX}*"),
                kind: v1::AppDataKind::AgentDatabase.into(),
                agent_id: "*".to_owned(),
                label: "Agents' own databases (you choose which)".to_owned(),
                reason: data.reason.clone(),
                granted: false,
            },
            _ => agent_source(config, &data.agent, &data.reason).map_or_else(
                || v1::AppDataSource {
                    id: format!("{AGENT_PREFIX}{}", data.agent),
                    kind: v1::AppDataKind::AgentDatabase.into(),
                    agent_id: data.agent.clone(),
                    label: format!("The database of an agent \"{}\", which isn't here", data.agent),
                    reason: data.reason.clone(),
                    granted: false,
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
            (FACTORY_RECORDS, _) => "Ask to read Claktory's records".to_owned(),
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
    let sources = sources(&inner.config, &manifest)
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
    let Some(manifest) = factory.apps.manifest(&asked.app_id) else {
        return answer(DataError::UnknownSource, "");
    };
    let source = {
        let inner = factory.lock();
        let Some(installed) = inner.config.apps.iter().find(|installed| installed.id == asked.app_id) else {
            return answer(DataError::UnknownSource, "");
        };
        let Some(source) = sources(&inner.config, &manifest).into_iter().find(|source| source.id == asked.source) else {
            return answer(DataError::UnknownSource, "");
        };
        if !installed.sources.contains(&source.id) {
            return answer(DataError::NotGranted, "");
        }
        source
    };
    let home = factory.home.clone();
    let ran = tokio::task::spawn_blocking(move || {
        let target = source.target();
        let db = sql::open_read_only(&home, &target).map_err(|why| (DataError::Failed, why))?;
        let Some(db) = db else {
            return Err((DataError::Failed, format!("{} has nothing in it yet.", source.label)));
        };
        run(&db, &target, &asked.sql, &asked.params)
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
            "Agents' memory, pipes and comments on files can't be read, nor can other databases be attached.".into(),
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

fn cell(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(integer) => json!(integer),
        ValueRef::Real(real) => serde_json::Number::from_f64(real).map_or(Value::Null, Value::Number),
        ValueRef::Text(text) => Value::String(String::from_utf8_lossy(text).into_owned()),
        ValueRef::Blob(blob) => Value::String(STANDARD.encode(blob)),
    }
}

impl Factory {
    /// GrantAppSource: the person gives the app a source it asks for, or
    /// takes it back.
    pub(crate) async fn grant_app_source(&self, request: &v1::GrantAppSource) -> Result<(), RequestError> {
        let manifest = self.apps.manifest(&request.app_id).ok_or_else(|| no_app(&request.app_id))?;
        self.edit_config(|config| {
            if !sources(config, &manifest).iter().any(|source| source.id == request.source_id) {
                return Err(RequestError::Apps(format!("The {} app doesn't ask to read \"{}\".", manifest.name, request.source_id)));
            }
            let app = config.apps.iter_mut().find(|app| app.id == request.app_id).ok_or_else(|| no_app(&request.app_id))?;
            app.sources.retain(|source| source != &request.source_id);
            if request.granted {
                app.sources.push(request.source_id.clone());
                app.sources.sort();
            }
            Ok(())
        })
        .await?;
        self.apps_changed();
        Ok(())
    }
}

/// An agent is gone: no app reads its database any more.
pub(crate) fn forget_agent(app: &mut AppConfig, gone: &str) {
    let id = format!("{AGENT_PREFIX}{gone}");
    app.sources.retain(|source| *source != id);
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
