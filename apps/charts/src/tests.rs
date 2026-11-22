//! The app against a stand-in host: a store, timers, sources whose queries
//! run in SQLite (tables shaped like Claktory's records), and surfaces that
//! apply what's pushed as a renderer would, so a test reads what the
//! person sees and presses what they'd press.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::model::{self, Dashboard, Grid, Panel, Query, Viz};
use crate::view;
use crate::*;

const NOW: i64 = 1_795_312_380_280;
const AGENT_DB: &str = "agent:rivet";

/// A surface as a renderer holds it.
#[derive(Default, Clone)]
struct Surface {
    components: BTreeMap<String, Value>,
    data: Value,
}

struct Stand {
    kv: BTreeMap<String, Vec<u8>>,
    timers: BTreeMap<String, u64>,
    /// Granted: Claktory's records, Rivet's database.
    records: bool,
    rivet: bool,
    db: rusqlite::Connection,
    /// Every query, bound: (source, sql, params).
    queries: Vec<(String, String, Value)>,
    pushed: Vec<(String, Value)>,
    surfaces: BTreeMap<String, Surface>,
    refuse_push: bool,
    now: i64,
}

impl Host for Stand {
    fn kv_get(&mut self, key: &str) -> Option<Vec<u8>> {
        self.kv.get(key).cloned()
    }

    fn kv_set(&mut self, key: &str, value: &[u8]) -> Result<(), String> {
        self.kv.insert(key.to_owned(), value.to_vec());
        Ok(())
    }

    fn kv_delete(&mut self, key: &str) {
        self.kv.remove(key);
    }

    fn set_timer(&mut self, name: &str, after_ms: u64) {
        self.timers.insert(name.to_owned(), after_ms);
    }

    fn cancel_timer(&mut self, name: &str) {
        self.timers.remove(name);
    }

    fn push_view(&mut self, surface: &str, message: &Value) -> Result<(), String> {
        if self.refuse_push {
            return Err("no such surface".into());
        }
        assert_eq!(message["version"], "1.0");
        let shown = self.surfaces.entry(surface.to_owned()).or_default();
        if let Some(create) = message.get("createSurface") {
            assert_eq!(create["surfaceId"], surface);
            assert_eq!(create["catalogId"], CATALOG);
            *shown = Surface::default();
            for component in create["components"].as_array().unwrap() {
                shown.components.insert(component["id"].as_str().unwrap().to_owned(), component.clone());
            }
            shown.data = create["dataModel"].clone();
        } else if let Some(update) = message.get("updateComponents") {
            assert_eq!(update["surfaceId"], surface);
            for component in update["components"].as_array().unwrap() {
                shown.components.insert(component["id"].as_str().unwrap().to_owned(), component.clone());
            }
        } else if let Some(update) = message.get("updateDataModel") {
            assert_eq!(update["surfaceId"], surface);
            set(&mut shown.data, update["path"].as_str().unwrap(), update["value"].clone());
        } else {
            panic!("unexpected {message}");
        }
        assert!(serde_json::to_vec(message).unwrap().len() < 1 << 20, "a message is under a megabyte");
        self.pushed.push((surface.to_owned(), message.clone()));
        Ok(())
    }

    fn data_sources(&mut self) -> Vec<DataSource> {
        vec![
            DataSource { id: RECORDS.into(), label: "Claktory's records".into(), granted: self.records },
            DataSource { id: AGENT_DB.into(), label: "Rivet's database".into(), granted: self.rivet },
        ]
    }

    fn data_query(&mut self, source: &str, sql: &str, params: &Value) -> Result<Value, DataError> {
        self.queries.push((source.to_owned(), sql.to_owned(), params.clone()));
        let granted = match source {
            RECORDS => self.records,
            AGENT_DB => self.rivet,
            _ => return Err(DataError::UnknownSource),
        };
        if !granted {
            return Err(DataError::NotGranted);
        }
        let mut statement = self.db.prepare(sql).map_err(|error| DataError::Failed(error.to_string()))?;
        let columns: Vec<String> = statement.column_names().map(str::to_owned).collect();
        let values: Vec<rusqlite::types::Value> = params
            .as_array()
            .unwrap()
            .iter()
            .map(|value| match value {
                Value::Null => rusqlite::types::Value::Null,
                Value::Bool(b) => rusqlite::types::Value::Integer(i64::from(*b)),
                Value::Number(n) if n.is_i64() => rusqlite::types::Value::Integer(n.as_i64().unwrap()),
                Value::Number(n) => rusqlite::types::Value::Real(n.as_f64().unwrap()),
                Value::String(s) => rusqlite::types::Value::Text(s.clone()),
                _ => panic!("a param is a scalar"),
            })
            .collect();
        let width = columns.len();
        let mut rows = statement.query(rusqlite::params_from_iter(values)).map_err(|error| DataError::Failed(error.to_string()))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|error| DataError::Failed(error.to_string()))? {
            let cells: Vec<Value> = (0..width)
                .map(|index| match row.get_ref(index).unwrap() {
                    rusqlite::types::ValueRef::Null => Value::Null,
                    rusqlite::types::ValueRef::Integer(n) => json!(n),
                    rusqlite::types::ValueRef::Real(n) => json!(n),
                    rusqlite::types::ValueRef::Text(text) => json!(String::from_utf8_lossy(text)),
                    rusqlite::types::ValueRef::Blob(_) => json!("blob"),
                })
                .collect();
            out.push(Value::Array(cells));
        }
        Ok(json!({"columns": columns, "rows": out, "more": false}))
    }

    fn now_ms(&mut self) -> i64 {
        self.now
    }

    fn log(&mut self, _level: Level, _message: &str) {}
}

/// Sets a JSON pointer as a renderer does; null deletes.
fn set(data: &mut Value, path: &str, value: Value) {
    if path == "/" || path.is_empty() {
        *data = value;
        return;
    }
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let mut at = data;
    for part in &parts[..parts.len() - 1] {
        if !at[*part].is_object() {
            at[*part] = json!({});
        }
        at = &mut at[*part];
    }
    let last = parts[parts.len() - 1];
    if value.is_null() {
        at.as_object_mut().unwrap().remove(last);
    } else {
        at[last] = value;
    }
}

const HOUR: i64 = 3_600_000;

/// Claktory's records, as far as the default dashboard reads them: a few
/// runs, messages, done items, tokens and one running task over the last
/// days.
fn records() -> rusqlite::Connection {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    db.execute_batch(
        "CREATE TABLE runs (id INTEGER PRIMARY KEY, agent TEXT NOT NULL, version TEXT NOT NULL, started INTEGER NOT NULL, ended INTEGER, outcome TEXT);
         CREATE TABLE messages (id INTEGER PRIMARY KEY, agent TEXT NOT NULL, created INTEGER NOT NULL);
         CREATE TABLE checklist_items (agent TEXT NOT NULL, id INTEGER NOT NULL, text TEXT NOT NULL, status TEXT NOT NULL, created INTEGER NOT NULL, updated INTEGER NOT NULL);
         CREATE TABLE agent_usage (hour INTEGER NOT NULL, agent TEXT NOT NULL, agent_name TEXT NOT NULL, input INTEGER NOT NULL, output INTEGER NOT NULL);
         CREATE TABLE tasks (id INTEGER PRIMARY KEY, agent TEXT NOT NULL, task TEXT NOT NULL, command TEXT NOT NULL, started INTEGER NOT NULL, ended INTEGER);
         CREATE TABLE notes (id INTEGER PRIMARY KEY, text TEXT);",
    )
    .unwrap();
    for (agent, ago) in [("rivet", 1), ("rivet", 30), ("builder", 2), ("builder", 50), ("builder", 24 * 9)] {
        let at = NOW - ago * HOUR;
        db.execute("INSERT INTO runs (agent, version, started) VALUES (?1, 'v', ?2)", rusqlite::params![agent, at]).unwrap();
        db.execute("INSERT INTO messages (agent, created) VALUES (?1, ?2)", rusqlite::params![agent, at]).unwrap();
        let hour = (at / HOUR) * 3600;
        db.execute(
            "INSERT INTO agent_usage VALUES (?1, ?2, ?3, 100, 20)",
            rusqlite::params![hour, agent, if agent == "rivet" { "Rivet" } else { "Builder" }],
        )
        .unwrap();
    }
    // A chat of the person's: in the totals, not by agent.
    db.execute("INSERT INTO agent_usage VALUES (?1, 'chat:1', 'The chat', 100, 20)", [(NOW / HOUR - 5) * 3600]).unwrap();
    db.execute("INSERT INTO checklist_items VALUES ('rivet', 1, 'a', 'done', 0, ?1)", [NOW - 3 * HOUR]).unwrap();
    db.execute("INSERT INTO checklist_items VALUES ('rivet', 2, 'b', 'open', 0, ?1)", [NOW - 3 * HOUR]).unwrap();
    db.execute("INSERT INTO tasks (agent, task, command, started) VALUES ('builder', 'task-1', 'cargo test', ?1)", [NOW - HOUR]).unwrap();
    db.execute("INSERT INTO tasks (agent, task, command, started, ended) VALUES ('rivet', 'task-1', 'ls', 1, 2)", []).unwrap();
    db.execute("INSERT INTO notes (text) VALUES ('first'), ('second')", []).unwrap();
    db
}

fn stand(records_granted: bool) -> Stand {
    Stand {
        kv: BTreeMap::new(),
        timers: BTreeMap::new(),
        records: records_granted,
        rivet: false,
        db: records(),
        queries: Vec::new(),
        pushed: Vec::new(),
        surfaces: BTreeMap::new(),
        refuse_push: false,
        now: NOW,
    }
}

/// A started app with a view open as "s1".
fn opened(records_granted: bool) -> (App, Stand) {
    let mut host = stand(records_granted);
    let mut app = App::default();
    app.start(&mut host);
    app.view_open(&mut host, "main", "s1").unwrap();
    (app, host)
}

impl Stand {
    fn surface(&self, surface: &str) -> &Surface {
        &self.surfaces[surface]
    }

    fn component(&self, surface: &str, id: &str) -> Value {
        self.surface(surface).components.get(id).cloned().unwrap_or(Value::Null)
    }

    /// A value as the surface reads it: a literal, or its binding's.
    fn resolve(&self, surface: &str, value: &Value) -> Value {
        match value.get("@path").and_then(Value::as_str) {
            Some(path) => self.surface(surface).data.pointer(path).cloned().unwrap_or(Value::Null),
            None => value.clone(),
        }
    }

    /// The components shown, from root, each once; panics on a reference to
    /// one that isn't there.
    fn tree(&self, surface: &str) -> Vec<Value> {
        let shown = self.surface(surface);
        let mut out = Vec::new();
        let mut stack = vec!["root".to_owned()];
        while let Some(id) = stack.pop() {
            let component = shown.components.get(&id).unwrap_or_else(|| panic!("{id} is referenced but not there")).clone();
            if let Some(children) = component["children"].as_array() {
                stack.extend(children.iter().rev().map(|child| child.as_str().unwrap().to_owned()));
            }
            if let Some(child) = component["child"].as_str() {
                stack.push(child.to_owned());
            }
            out.push(component);
        }
        out
    }

    /// The words shown, in order: texts, buttons' words, labels and titles.
    fn words(&self, surface: &str) -> Vec<String> {
        self.tree(surface)
            .iter()
            .flat_map(|component| {
                ["text", "label", "title", "body"]
                    .iter()
                    .filter_map(|key| component.get(*key))
                    .map(|value| match self.resolve(surface, value) {
                        Value::String(text) => text,
                        other => other.to_string(),
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn shows(&self, surface: &str, text: &str) -> bool {
        self.words(surface).iter().any(|words| words.contains(text))
    }

    /// The button whose words are `label`.
    fn button(&self, surface: &str, label: &str) -> Value {
        self.tree(surface)
            .into_iter()
            .find(|component| {
                component["component"] == "Button" && self.component(surface, component["child"].as_str().unwrap())["text"] == label
            })
            .unwrap_or_else(|| panic!("no button \"{label}\" in {:?}", self.words(surface)))
    }

    /// The person presses the button `label`: its action, its context
    /// resolved against the surface's data, as the renderer sends it.
    fn press(&mut self, app: &mut App, surface: &str, label: &str) {
        let button = self.button(surface, label);
        let event = &button["action"]["event"];
        let context: serde_json::Map<String, Value> = event["context"]
            .as_object()
            .map(|context| context.iter().map(|(key, value)| (key.clone(), self.resolve(surface, value))).collect())
            .unwrap_or_default();
        let message = json!({"version": "1.0", "action": {
            "name": event["name"], "surfaceId": surface, "sourceComponentId": button["id"],
            "timestamp": "2026-11-22T10:00:00Z", "context": context,
        }});
        app.view_action(self, surface, &message.to_string());
    }

    /// The person types or picks `value` at `path`, as a field does.
    fn enter(&mut self, surface: &str, path: &str, value: Value) {
        let shown = self.surfaces.get_mut(surface).unwrap();
        set(&mut shown.data, path, value);
    }

    fn dashboard(&mut self, id: &str) -> Dashboard {
        model::load(self, id).unwrap()
    }

    /// What was pushed, by kind ("createSurface", "updateComponents"…).
    fn kinds(&self) -> Vec<String> {
        self.pushed.iter().map(|(_, message)| message.as_object().unwrap().keys().find(|key| *key != "version").unwrap().clone()).collect()
    }
}

fn panel(id: &str, title: &str, source: &str, sql: &str, kind: &str) -> Panel {
    Panel {
        id: id.into(),
        title: title.into(),
        grid: Grid::default(),
        query: Query { source: source.into(), sql: sql.into() },
        viz: Viz { kind: kind.into(), ..Viz::default() },
    }
}

fn sources() -> Vec<DataSource> {
    stand(true).data_sources()
}

// ---- The store ----

#[test]
fn it_starts_with_the_default_dashboard_and_never_replaces_one() {
    let mut host = stand(true);
    host.kv.insert(model::LEGACY_KEY.into(), b"{}".to_vec());
    let mut app = App::default();
    app.start(&mut host);
    assert!(!host.kv.contains_key(model::LEGACY_KEY), "C4a's one dashboard is given up");
    assert_eq!(model::ids(&mut host), vec!["claktory"]);
    let kept = host.dashboard("claktory");
    assert_eq!(kept.rev, 1);
    assert_eq!(Dashboard { rev: 0, ..kept.clone() }, model::default_dashboard());
    assert!(kept.problems(&sources()).is_empty(), "{:?}", kept.problems(&sources()));

    // Started again, it keeps what's there, even no dashboard at all.
    model::delete(&mut host, "claktory").unwrap();
    app.start(&mut host);
    assert!(model::ids(&mut host).is_empty());
}

#[test]
fn every_save_keeps_the_version_before_it() {
    let mut host = stand(true);
    App::default().start(&mut host);
    let mut next = host.dashboard("claktory");
    next.name = "Mine".into();
    let saved = model::save(&mut host, next.clone(), Some(1), "rivet").unwrap();
    assert_eq!((saved.rev, saved.by.as_str()), (2, "rivet"));
    assert_eq!(model::previous(&mut host, "claktory").unwrap().name, "Claktory");

    // Put on a revision that's gone: refused, nothing changes.
    next.name = "Theirs".into();
    assert_eq!(model::save(&mut host, next, Some(1), "builder"), Err(model::SaveError::Changed(2)));
    assert_eq!(host.dashboard("claktory").name, "Mine");

    // Restoring swaps them, as a new revision: restoring again undoes it.
    let restored = model::restore(&mut host, "claktory", "").unwrap();
    assert_eq!((restored.name.as_str(), restored.rev), ("Claktory", 3));
    assert_eq!(model::previous(&mut host, "claktory").unwrap().name, "Mine");
    assert_eq!(model::restore(&mut host, "claktory", "").unwrap().name, "Mine");

    // Deleted, its last version is kept, and a new one counts on from it.
    model::delete(&mut host, "claktory").unwrap();
    assert!(model::load(&mut host, "claktory").is_none());
    let gone = model::previous(&mut host, "claktory").unwrap();
    assert_eq!(gone.rev, 4);
    assert_eq!(model::save(&mut host, gone, None, "").unwrap().rev, 5);
    assert_eq!(model::ids(&mut host), vec!["claktory"]);
}

#[test]
fn a_definition_is_checked_and_says_where_its_problems_are() {
    let mut dashboard = model::default_dashboard();
    dashboard.id = "Not An Id".into();
    dashboard.time.from = "yesterday".into();
    dashboard.refresh_s = 1;
    dashboard.variables.push(model::Variable {
        name: "from".into(),
        label: String::new(),
        query: Query { source: RECORDS.into(), sql: "SELECT 1".into() },
        default: String::new(),
    });
    dashboard.panels[0].viz.kind = "radar".into();
    dashboard.panels[1].query.source = "elsewhere".into();
    dashboard.panels[2].query.sql = "SELECT * FROM runs WHERE agent = $who".into();
    dashboard.panels[3].grid = Grid { x: 8, y: 0, w: 6, h: 4 };
    dashboard.panels[4].id = dashboard.panels[5].id.clone();
    dashboard.panels[5].query.sql = "SELECT ?".into();
    let problems = dashboard.problems(&sources());
    for expected in [
        "id: use 1 to 40 lowercase",
        "time: each end is",
        "refresh_s: 10 to 86400",
        "variables[1].name: lowercase letters",
        "panels[0].viz.kind: one of stat, timeseries",
        "panels[1].query.source: \"elsewhere\" isn't a source Charts reads; it reads factory-records, agent:rivet",
        "panels[2].query.sql: $who isn't a variable",
        "panels[3].grid: w is 1 to 12",
        "panels[5].id: \"runs_by_agent\" is there twice",
        "panels[5].query.sql: use $names",
    ] {
        assert!(problems.iter().any(|problem| problem.starts_with(expected)), "{expected} in {problems:#?}");
    }
}

// ---- The view ----

#[test]
fn the_default_dashboard_shows_what_the_records_say() {
    let (_, host) = opened(true);
    assert_eq!(host.kinds(), vec!["createSurface"]);
    let words = host.words("s1");
    for expected in
        ["Claktory", "Edit", "7 days", "Agent", "All", "Builder", "Rivet", "Runs", "Tokens by agent", "Background tasks running now"]
    {
        assert!(words.iter().any(|word| word == expected), "{expected} in {words:?}");
    }
    let data = &host.surface("s1").data;
    // In the last 7 days: 4 runs and messages (one was 9 days ago), one item
    // done, 5 × 120 tokens (a chat's among them).
    assert_eq!(data["panels"][0]["value"], 4);
    assert_eq!(data["panels"][1]["value"], 4);
    assert_eq!(data["panels"][2]["value"], 1);
    assert_eq!(data["panels"][3]["value"], 600);
    // A spark point a bucket ($step is 3 hours over 7 days).
    assert_eq!(data["panels"][0]["spark"].as_array().unwrap().len(), 57);
    let by_agent = &data["panels"][4]["data"];
    assert_eq!(by_agent["columns"], json!(["time", "agent", "tokens"]));
    let rows = by_agent["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 4, "no chat: {rows:?}");
    // Bars over time are labelled with local times, which charts read.
    assert!(rows.iter().all(|row| row[0].as_str().is_some_and(|time| time.len() == 16 && time.as_bytes()[10] == b' ')), "{rows:?}");
    assert!(!host.shows("s1", "The chat"));
    assert_eq!(data["panels"][6]["data"]["rows"], json!([["builder", "task-1", "cargo test", NOW - HOUR]]));
    // Every query is bound: no value is in its text.
    for (_, sql, params) in &host.queries {
        assert!(!sql.contains('$'), "{sql}");
        assert!(params.as_array().unwrap().iter().all(|param| !param.is_null()));
    }
    // Panels sit on the grid: four stats in a row, a quarter each.
    let row0 = host.component("s1", "row0");
    assert_eq!(row0["children"], json!(["p0", "p1", "p2", "p3"]));
    assert_eq!(host.component("s1", "p0")["weight"], 3);
    let row2 = host.component("s1", "row2");
    assert_eq!(row2["children"], json!(["p5", "p6"]));
    assert_eq!(host.component("s1", "p4_body")["height"], 250);
}

#[test]
fn the_range_and_variables_are_bound_values() {
    let (mut app, mut host) = opened(true);
    host.queries.clear();
    host.press(&mut app, "s1", "24 hours");
    let (_, _, params) = host.queries.iter().find(|(_, sql, _)| sql.contains("FROM runs WHERE started")).unwrap().clone();
    assert!(params.as_array().unwrap().contains(&json!(NOW - 24 * HOUR)), "{params}");
    assert_eq!(host.button("s1", "24 hours")["variant"], "primary");
    assert_eq!(host.button("s1", "7 days")["variant"], "default");
    // In 24 hours: 2 runs.
    assert_eq!(host.surface("s1").data["panels"][0]["value"], 2);

    host.queries.clear();
    host.press(&mut app, "s1", "Rivet");
    assert!(host.queries.iter().any(|(_, _, params)| params.as_array().unwrap().contains(&json!("rivet"))));
    assert_eq!(host.surface("s1").data["panels"][0]["value"], 1, "Rivet's run in the last day");
    assert_eq!(host.button("s1", "Rivet")["variant"], "primary");
    host.press(&mut app, "s1", "All");
    assert_eq!(host.surface("s1").data["panels"][0]["value"], 2);
}

#[test]
fn refreshing_pushes_only_what_changed_and_stops_when_the_last_view_closes() {
    let (mut app, mut host) = opened(true);
    assert_eq!(host.timers[REFRESH], 30_000);
    host.now += 30_000;
    app.on_timer(&mut host, REFRESH);
    assert_eq!(host.kinds(), vec!["createSurface"], "nothing changed, nothing pushed");

    host.db.execute("INSERT INTO runs (agent, version, started) VALUES ('rivet', 'v', ?1)", [host.now - 1000]).unwrap();
    host.now += 30_000;
    app.on_timer(&mut host, REFRESH);
    assert_eq!(host.kinds(), vec!["createSurface", "updateDataModel"]);
    let (_, update) = host.pushed.last().unwrap();
    assert_eq!(update["updateDataModel"]["path"], "/panels", "never the editor's drafts");
    assert_eq!(host.surface("s1").data["panels"][0]["value"], 5);

    // Not due yet: nothing runs.
    let queries = host.queries.len();
    host.now += 1_000;
    app.on_timer(&mut host, REFRESH);
    assert_eq!(host.queries.len(), queries);

    app.view_open(&mut host, "main", "s2").unwrap();
    app.view_closed(&mut host, "s1");
    assert!(host.timers.contains_key(REFRESH), "s2 is still open");
    app.view_closed(&mut host, "s2");
    assert!(!host.timers.contains_key(REFRESH));
    host.now += 60_000;
    let queries = host.queries.len();
    app.on_timer(&mut host, REFRESH);
    assert_eq!(host.queries.len(), queries);
}

#[test]
fn ungranted_it_reads_nothing_and_says_how_then_shows_the_charts_once_granted() {
    let (mut app, mut host) = opened(false);
    assert!(host.queries.is_empty());
    assert!(host.shows("s1", "Charts can't read anything yet"));
    assert!(host.shows("s1", "Settings → Apps"));
    assert_eq!(host.timers[REFRESH], ASK_AGAIN_MS);
    host.now += 3_000;
    app.on_timer(&mut host, REFRESH);
    assert!(host.queries.is_empty(), "looking again reads nothing");

    host.records = true;
    host.now += 3_000;
    app.on_timer(&mut host, REFRESH);
    assert!(host.shows("s1", "Tokens by agent"));
    assert_eq!(host.surface("s1").data["panels"][0]["value"], 4);
    assert_eq!(host.timers[REFRESH], 30_000);

    // Taken back: the same again.
    host.records = false;
    host.now += 3_000;
    app.on_timer(&mut host, REFRESH);
    assert!(host.shows("s1", "Charts can't read anything yet"));
}

#[test]
fn a_panel_whose_source_isnt_granted_says_so_while_the_others_show() {
    let (mut app, mut host) = opened(true);
    let mut dashboard = host.dashboard("claktory");
    dashboard.panels.push(panel("notes", "Notes", AGENT_DB, "SELECT COUNT(*) AS n FROM notes", "stat"));
    model::save(&mut host, dashboard, None, "rivet").unwrap();
    host.now += 30_000;
    app.on_timer(&mut host, REFRESH);
    assert_eq!(host.surface("s1").data["panels"][7]["error"], "To see this panel, turn on Rivet's database for Charts in Settings → Apps.");
    assert_eq!(host.timers[REFRESH], ASK_AGAIN_MS, "it looks again soon");
    assert!(!host.queries.iter().any(|(source, _, _)| source == AGENT_DB));

    host.rivet = true;
    host.now += 3_000;
    app.on_timer(&mut host, REFRESH);
    assert_eq!(host.surface("s1").data["panels"][7]["value"], 2);
    assert_eq!(host.component("s1", "p7_body")["component"], "Stat");
}

#[test]
fn a_failing_query_shows_why_in_its_panel() {
    let (mut app, mut host) = opened(true);
    let mut dashboard = host.dashboard("claktory");
    dashboard.panels[0].query.sql = "SELECT x FROM nowhere".into();
    model::save(&mut host, dashboard, None, "").unwrap();
    host.now += 30_000;
    app.on_timer(&mut host, REFRESH);
    let error = host.surface("s1").data["panels"][0]["error"].as_str().unwrap().to_owned();
    assert!(error.starts_with("Couldn't run this panel's query: no such table"), "{error}");
    assert_eq!(host.component("s1", "p0_body_error")["text"], json!({"@path": "/panels/0/error"}));
    assert!(host.shows("s1", "Couldn't run this panel's query"));
}

#[test]
fn the_person_adds_a_panel_with_a_run_preview_first() {
    let (mut app, mut host) = opened(true);
    host.press(&mut app, "s1", "Edit");
    host.press(&mut app, "s1", "Add panel");
    assert!(host.shows("s1", "New panel"));
    assert_eq!(host.surface("s1").data["editor"]["source"], json!([RECORDS]));

    // A query that can't run says why, and keeps nothing.
    host.enter("s1", "/editor/sql", json!("SELECT agent FROM runs WHERE agent = $who"));
    host.press(&mut app, "s1", "Run");
    assert!(host.shows("s1", "$who isn't a variable"), "{:?}", host.words("s1"));

    host.enter("s1", "/editor/title", json!("Runs per agent"));
    host.enter("s1", "/editor/sql", json!("SELECT agent, COUNT(*) AS runs FROM runs WHERE started >= $from GROUP BY agent"));
    host.enter("s1", "/editor/kind", json!(["bars"]));
    host.enter("s1", "/editor/width", json!(["4"]));
    host.press(&mut app, "s1", "Run");
    assert_eq!(host.component("s1", "ed_preview_body")["component"], "Chart");
    assert_eq!(host.component("s1", "ed_preview_body")["kind"], "bars");
    assert_eq!(host.surface("s1").data["editor"]["preview"]["data"]["rows"], json!([["builder", 2], ["rivet", 2]]));
    assert!(host.dashboard("claktory").panel("runs_per_agent").is_none(), "Run keeps nothing");

    host.press(&mut app, "s1", "Save");
    let dashboard = host.dashboard("claktory");
    let added = dashboard.panel("runs_per_agent").unwrap();
    assert_eq!((added.viz.kind.as_str(), added.grid), ("bars", Grid { x: 0, y: 3, w: 4, h: 4 }), "under the full last row");
    assert_eq!((dashboard.rev, dashboard.by.as_str()), (2, ""));
    assert!(!host.shows("s1", "New panel"), "the editor closed");
    assert!(host.surface("s1").data.get("editor").is_none());
    assert_eq!(host.surface("s1").data["panels"][7]["data"]["rows"], json!([["builder", 2], ["rivet", 2]]));
    assert!(host.shows("s1", "Runs per agent"));
    // The tree holds together after all of it.
    host.tree("s1");
}

#[test]
fn the_person_edits_and_removes_a_panel_and_restores_it() {
    let (mut app, mut host) = opened(true);
    host.press(&mut app, "s1", "Edit");
    let edit = host.component("s1", "p0_edit");
    assert_eq!(edit["action"]["event"]["context"]["panel"], "runs");
    host.press_id(&mut app, "s1", "p0_edit");
    assert_eq!(host.surface("s1").data["editor"]["title"], "Runs");
    assert!(host.shows("s1", "Edit panel"));
    host.enter("s1", "/editor/title", json!("All runs"));
    host.enter("s1", "/editor/kind", json!([]));
    host.press(&mut app, "s1", "Save");
    assert!(host.shows("s1", "Choose how to show it."));
    host.enter("s1", "/editor/kind", json!(["stat"]));
    host.press(&mut app, "s1", "Save");
    assert_eq!(host.dashboard("claktory").panels[0].title, "All runs");

    host.press_id(&mut app, "s1", "p0_remove");
    assert!(host.dashboard("claktory").panel("runs").is_none());
    assert!(host.shows("s1", "Removed All runs. Restore previous version brings it back."));
    host.press(&mut app, "s1", "Restore previous version");
    assert_eq!(host.dashboard("claktory").panels[0].title, "All runs");
    assert!(host.shows("s1", "Restored the previous version."));
    host.tree("s1");
}

#[test]
fn the_person_makes_renames_switches_and_deletes_dashboards() {
    let (mut app, mut host) = opened(true);
    host.press(&mut app, "s1", "Edit");
    host.press(&mut app, "s1", "New dashboard");
    host.press(&mut app, "s1", "Create");
    assert!(host.shows("s1", "Enter a name of up to 60 characters."));
    host.enter("s1", "/form/name", json!("My agents' week"));
    host.press(&mut app, "s1", "Create");
    assert_eq!(model::ids(&mut host), vec!["claktory", "my-agents-week"]);
    assert!(host.shows("s1", "My agents' week"));
    assert!(host.shows("s1", "Choose Add panel to chart something."));
    assert_eq!(host.button("s1", "My agents' week")["variant"], "primary");

    host.press(&mut app, "s1", "Rename");
    assert_eq!(host.surface("s1").data["form"]["name"], "My agents' week");
    host.enter("s1", "/form/name", json!("Week"));
    host.press(&mut app, "s1", "Save");
    assert_eq!(host.dashboard("my-agents-week").name, "Week");

    host.press(&mut app, "s1", "Claktory");
    assert_eq!(host.surface("s1").data["panels"][0]["value"], 4);
    host.press(&mut app, "s1", "Week");
    host.press(&mut app, "s1", "Edit");
    host.press(&mut app, "s1", "Delete dashboard");
    assert!(host.shows("s1", "Delete Week and its panels?"));
    host.press(&mut app, "s1", "Delete");
    assert_eq!(model::ids(&mut host), vec!["claktory"]);
    assert!(host.shows("s1", "Tokens by agent"), "back on the one there is");

    // None left: it offers to make one.
    host.press(&mut app, "s1", "Edit");
    host.press(&mut app, "s1", "Delete dashboard");
    host.press(&mut app, "s1", "Delete");
    assert!(host.shows("s1", "No dashboards yet"));
    host.press(&mut app, "s1", "New dashboard");
    host.enter("s1", "/form/name", json!("Fresh"));
    host.press(&mut app, "s1", "Create");
    assert_eq!(model::ids(&mut host), vec!["fresh"]);
    host.tree("s1");
}

impl Stand {
    /// Presses the button with id `id`.
    fn press_id(&mut self, app: &mut App, surface: &str, id: &str) {
        let label = self.component(surface, self.component(surface, id)["child"].as_str().unwrap())["text"].as_str().unwrap().to_owned();
        let event = self.component(surface, id)["action"]["event"].clone();
        let context: serde_json::Map<String, Value> =
            event["context"].as_object().unwrap().iter().map(|(key, value)| (key.clone(), self.resolve(surface, value))).collect();
        let message = json!({"version": "1.0", "action": {"name": event["name"], "surfaceId": surface, "sourceComponentId": id, "timestamp": "t", "context": context}});
        assert!(!label.is_empty());
        app.view_action(self, surface, &message.to_string());
    }
}

#[test]
fn a_surface_that_went_is_forgotten_and_only_its_view_opens() {
    let (mut app, mut host) = opened(true);
    host.refuse_push = true;
    host.db.execute("INSERT INTO runs (agent, version, started) VALUES ('rivet', 'v', ?1)", [NOW]).unwrap();
    host.now += 30_000;
    app.on_timer(&mut host, REFRESH);
    assert!(app.open_surfaces().is_empty());
    assert!(!host.timers.contains_key(REFRESH));
    host.refuse_push = false;
    assert_eq!(app.view_open(&mut host, "other", "s3"), Err("Charts has no view \"other\".".into()));
}

// ---- The tools ----

#[test]
fn an_agent_with_the_edit_grant_adds_a_panel_and_open_views_show_it() {
    let (mut app, mut host) = opened(true);
    let listed = app.call(&mut host, "charts_list", "{}", "rivet");
    assert!(!listed.is_error);
    assert!(listed.text.contains("- claktory \"Claktory\": 7 panels, variables $agent, _rev 1"), "{}", listed.text);
    assert!(listed.text.contains("- agent:rivet (Rivet's database): not granted"), "{}", listed.text);

    let got = app.call(&mut host, "charts_get", r#"{"id": "claktory"}"#, "rivet");
    let mut dashboard = got.data.unwrap();
    dashboard["panels"].as_array_mut().unwrap().push(json!({
        "id": "failed_runs", "title": "Failed runs",
        "grid": {"x": 0, "y": 3, "w": 12, "h": 4},
        "query": {"source": "factory-records", "sql": "SELECT agent, COUNT(*) AS n FROM runs WHERE outcome = 'failed' AND started >= $from GROUP BY agent"},
        "viz": {"kind": "table"},
    }));
    let put = app.call(&mut host, "charts_put", &json!({"dashboard": dashboard}).to_string(), "rivet");
    assert!(!put.is_error, "{}", put.text);
    assert_eq!(put.text, "Kept \"Claktory\" (claktory) as _rev 2; the version before it is kept too.");
    let kept = host.dashboard("claktory");
    assert_eq!((kept.rev, kept.by.as_str(), kept.panels.len()), (2, "rivet", 8));

    // The open view shows it right after the call.
    assert_eq!(host.timers[REFRESH], 1);
    app.on_timer(&mut host, REFRESH);
    assert!(host.shows("s1", "Failed runs"));
    assert_eq!(host.component("s1", "p7_body_table")["component"], "Table");
    assert_eq!(host.surface("s1").data["panels"][7]["data"]["columns"], json!(["agent", "n"]));

    // Put again on the revision it read: refused, nothing changes.
    let stale = app.call(&mut host, "charts_put", &json!({"dashboard": dashboard}).to_string(), "builder");
    assert!(stale.is_error);
    assert!(stale.text.contains("it's at _rev 2 now"), "{}", stale.text);

    // Its previous version is there to see, and to put back.
    let previous = app.call(&mut host, "charts_get", r#"{"id": "claktory", "previous": true}"#, "rivet");
    assert_eq!(previous.data.unwrap()["panels"].as_array().unwrap().len(), 7);
}

#[test]
fn a_put_says_what_to_fix_and_which_panels_fail() {
    let (mut app, mut host) = opened(true);
    let wrong = json!({"dashboard": {"id": "x", "name": "X", "panels": [
        {"id": "a", "title": "A", "query": {"source": "factory-records", "sql": "SELECT $nope"}, "viz": {"kind": "pie"}},
        {"id": "a", "title": "B", "query": {"source": "nowhere", "sql": "SELECT 1"}, "viz": {"kind": "gauge"}},
    ]}});
    let refused = app.call(&mut host, "charts_put", &wrong.to_string(), "rivet");
    assert!(refused.is_error);
    for expected in [
        "panels[0].query.sql: $nope isn't a variable",
        "panels[1].id: \"a\" is there twice",
        "panels[1].query.source",
        "panels[1].viz.kind",
    ] {
        assert!(refused.text.contains(expected), "{expected} in {}", refused.text);
    }
    let unknown = app.call(&mut host, "charts_put", r#"{"dashboard": {"id": "x", "name": "X", "colour": "red"}}"#, "rivet");
    assert!(unknown.text.contains("unknown field `colour`"), "{}", unknown.text);
    assert_eq!(model::ids(&mut host), vec!["claktory"]);
    assert!(!host.timers.contains_key(REFRESH) || host.timers[REFRESH] != 1, "nothing changed, nothing refreshes");

    // Kept, with the panels that fail named.
    let kept = app.call(
        &mut host,
        "charts_put",
        &json!({"id": "notes", "name": "Notes", "panels": [
            {"id": "broken", "title": "Broken", "query": {"source": "factory-records", "sql": "SELECT x FROM nowhere"}, "viz": {"kind": "table"}},
            {"id": "mine", "title": "Mine", "query": {"source": "agent:rivet", "sql": "SELECT COUNT(*) FROM notes"}, "viz": {"kind": "stat"}},
        ]})
        .to_string(),
        "rivet",
    );
    assert!(!kept.is_error, "{}", kept.text);
    assert!(kept.text.contains("Panel broken's query failed: no such table: nowhere"), "{}", kept.text);
    assert!(kept.text.contains("Panel mine shows nothing until the person turns on Rivet's database"), "{}", kept.text);
    assert_eq!(model::ids(&mut host), vec!["claktory", "notes"]);
}

#[test]
fn an_agent_deletes_and_queries() {
    let (mut app, mut host) = opened(true);
    let queried = app.call(
        &mut host,
        "charts_query",
        r#"{"source": "factory-records", "sql": "SELECT agent, COUNT(*) AS n FROM runs WHERE started >= $from AND agent != $skip GROUP BY agent", "from": "now-24h", "variables": {"skip": "nobody"}}"#,
        "rivet",
    );
    assert!(!queried.is_error, "{}", queried.text);
    assert_eq!(queried.text, "agent\tn\nbuilder\t1\nrivet\t1\n(2 rows)");
    for (args, why) in [
        (r#"{"source": "factory-records", "sql": "SELECT ?1"}"#, "not ?"),
        (r#"{"source": "factory-records", "sql": "SELECT $who"}"#, "$who isn't a variable"),
        (r#"{"source": "agent:rivet", "sql": "SELECT 1"}"#, "hasn't turned on Rivet's database"),
        (r#"{"source": "elsewhere", "sql": "SELECT 1"}"#, "doesn't read \"elsewhere\""),
        (r#"{"source": "factory-records", "sql": "SELECT 1", "from": "later"}"#, "from and to are"),
        (r#"{"source": "factory-records", "sql": "SELECT $from", "variables": {"from": 1}}"#, "is built in"),
    ] {
        let refused = app.call(&mut host, "charts_query", args, "rivet");
        assert!(refused.is_error && refused.text.contains(why), "{why} in {}", refused.text);
    }
    // A panel's SQL as it is, its variables at their defaults, or given.
    let panel = r#"SELECT COUNT(*) AS n FROM runs WHERE started >= $from AND ($agent = '' OR agent = $agent)"#;
    let args = |extra: Value| {
        let mut args = json!({"source": "factory-records", "sql": panel});
        args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        args.to_string()
    };
    let refused = app.call(&mut host, "charts_query", &args(json!({})), "rivet");
    assert!(refused.text.contains("$agent isn't a variable") && refused.text.contains("\"dashboard\": \"<its id>\""), "{}", refused.text);
    assert_eq!(app.call(&mut host, "charts_query", &args(json!({"dashboard": "claktory"})), "rivet").text, "n\n4\n(1 row)");
    let rivet = app.call(&mut host, "charts_query", &args(json!({"dashboard": "claktory", "variables": {"agent": "rivet"}})), "rivet");
    assert_eq!(rivet.text, "n\n2\n(1 row)");
    let many = app.call(&mut host, "charts_query", r#"{"source": "factory-records", "sql": "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 500) SELECT i FROM n"}"#, "rivet");
    assert!(many.text.ends_with("(the first 200 rows; there are more)"), "{}", &many.text[many.text.len() - 60..]);

    let deleted = app.call(&mut host, "charts_delete", r#"{"id": "claktory"}"#, "rivet");
    assert!(deleted.text.starts_with("Deleted \"Claktory\" (claktory). Its last version is kept"), "{}", deleted.text);
    app.on_timer(&mut host, REFRESH);
    assert!(host.shows("s1", "No dashboards yet"));
    assert!(app.call(&mut host, "charts_delete", r#"{"id": "claktory"}"#, "rivet").is_error);
    assert!(app.call(&mut host, "charts_nothing", "{}", "rivet").is_error);
}

#[test]
fn long_answers_are_held_to_their_bounds() {
    let rows: Vec<Value> = (0..10_000).map(|i| json!([i, if i % 2 == 0 { "a" } else { "b" }, i])).collect();
    let table = json!({"columns": ["t", "s", "v"], "rows": rows, "more": false});
    let down = view::downsample(&table, 2_000, Some("s"));
    let kept = down["rows"].as_array().unwrap();
    assert_eq!(kept.len(), 2_000);
    assert_eq!(kept.first().unwrap(), &json!([0, "a", 0]));
    assert_eq!(kept.last().unwrap(), &json!([9999, "b", 9999]), "the newest is kept");
    assert_eq!(kept.iter().filter(|row| row[1] == "a").count(), 1_000);
    let cut = view::truncate(&table, 200);
    assert_eq!((cut["rows"].as_array().unwrap().len(), cut["more"].clone()), (200, json!(true)));
    let small = json!({"columns": ["t"], "rows": [[1], [2]], "more": false});
    assert_eq!(view::downsample(&small, 2_000, None), small);
}
