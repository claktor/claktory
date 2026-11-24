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
            DataSource { id: ACTIVITY.into(), label: "Activity".into(), granted: self.records },
            DataSource { id: AGENT_DB.into(), label: "Rivet's database".into(), granted: self.rivet },
        ]
    }

    fn data_query(&mut self, source: &str, sql: &str, params: &Value) -> Result<Value, DataError> {
        self.queries.push((source.to_owned(), sql.to_owned(), params.clone()));
        let granted = match source {
            ACTIVITY => self.records,
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
         CREATE TABLE tasks (id INTEGER PRIMARY KEY, agent TEXT NOT NULL, task TEXT NOT NULL, started INTEGER NOT NULL, ended INTEGER);
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
    db.execute("INSERT INTO tasks (agent, task, started) VALUES ('builder', 'task-1', ?1)", [NOW - HOUR]).unwrap();
    db.execute("INSERT INTO tasks (agent, task, started, ended) VALUES ('rivet', 'task-1', 1, 2)", []).unwrap();
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

    /// A button's words: its child's text, or its child's first text's.
    fn button_words(&self, surface: &str, button: &Value) -> Value {
        let child = self.component(surface, button["child"].as_str().unwrap());
        match child["children"].as_array().and_then(|children| children.first()) {
            Some(first) => self.resolve(surface, &self.component(surface, first.as_str().unwrap())["text"]),
            None => self.resolve(surface, &child["text"]),
        }
    }

    /// The button whose words are `label`.
    fn button(&self, surface: &str, label: &str) -> Value {
        self.tree(surface)
            .into_iter()
            .find(|component| component["component"] == "Button" && self.button_words(surface, component) == label)
            .unwrap_or_else(|| panic!("no button \"{label}\" in {:?}", self.words(surface)))
    }

    /// The person opens the menu `menu` (its button's id) and chooses
    /// `label` in it.
    fn choose(&mut self, app: &mut App, surface: &str, menu: &str, label: &str) {
        self.press_id(app, surface, menu);
        assert_eq!(self.component(surface, "menu")["component"], "Card", "{menu} opened");
        self.press(app, surface, label);
        assert!(self.tree(surface).iter().all(|component| component["id"] != "menu"), "choosing closes it");
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
fn the_tokens_stat_counts_the_hour_the_range_began_in() {
    // Rivet's tokens 1 hour ago are kept in an hour that began before the
    // last hour did (NOW is 53 minutes into its hour): the stat counts
    // them, as Tokens by agent does.
    let (mut app, mut host) = opened(true);
    host.choose(&mut app, "s1", "range", "Last 1 hour");
    let data = host.surface("s1").data.clone();
    assert_eq!(data["panels"][3]["value"], 120);
    let rows = data["panels"][4]["data"]["rows"].as_array().unwrap().clone();
    let by_agent: i64 = rows.iter().map(|row| row[2].as_i64().unwrap()).sum();
    assert_eq!(by_agent, 120, "{rows:?}");
}

#[test]
fn the_default_tokens_stat_is_fixed_where_it_was_left_alone() {
    // 0.2.0's query, as it kept it.
    let old = "WITH RECURSIVE t(at) AS (SELECT ($from / $step) * $step UNION ALL SELECT at + $step FROM t WHERE at + $step < $to), \
               c AS (SELECT ((hour * 1000) / $step) * $step AS at, SUM(input + output) AS n FROM agent_usage \
               WHERE hour >= $from_s - 3599 AND hour < $to_s AND ($agent = '' OR agent = $agent) GROUP BY 1) \
               SELECT t.at AS time, COALESCE(c.n, 0) AS tokens FROM t LEFT JOIN c USING (at) ORDER BY t.at";
    let mut host = stand(true);
    App::default().start(&mut host);
    let mut kept = host.dashboard("claktory");
    kept.panels[3].query.sql = old.into();
    model::save(&mut host, kept, Some(1), "").unwrap();

    App::default().start(&mut host);
    let fixed = host.dashboard("claktory");
    assert_eq!(fixed.rev, 3);
    assert_eq!(fixed.panels, model::default_dashboard().panels);
    assert_eq!(model::previous(&mut host, "claktory").unwrap().panels[3].query.sql, old);

    // Once fixed, or changed by someone, it's left alone.
    App::default().start(&mut host);
    assert_eq!(host.dashboard("claktory").rev, 3);
    let mut theirs = host.dashboard("claktory");
    theirs.panels[3].query.sql = old.replace("SUM(input + output)", "SUM(output)");
    model::save(&mut host, theirs.clone(), Some(3), "rivet").unwrap();
    App::default().start(&mut host);
    assert_eq!(host.dashboard("claktory").panels[3].query.sql, theirs.panels[3].query.sql);
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
        query: Query { source: ACTIVITY.into(), sql: "SELECT 1".into() },
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
        "panels[1].query.source: \"elsewhere\" isn't a source Charts reads; it reads activity, agent:rivet",
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
        ["Claktory", "Edit", "Last 7 days", "Agent: All", "Runs", "Tokens by agent", "Background tasks running now"]
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
    assert_eq!(data["panels"][6]["data"]["rows"], json!([["builder", "task-1", NOW - HOUR]]));
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
    assert_eq!(host.component("s1", "p4_body")["height"], 258, "7 of Grafana's rows");
}

#[test]
fn the_range_and_variables_are_bound_values() {
    let (mut app, mut host) = opened(true);
    host.queries.clear();
    // The range's menu: Grafana's quick ranges, the one shown chosen;
    // opening it runs nothing, pressing its button again closes it.
    host.press_id(&mut app, "s1", "range");
    assert!(host.queries.is_empty());
    let options: Vec<String> = host.component("s1", "menu_col")["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| host.button_words("s1", &host.component("s1", id.as_str().unwrap())).as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        options.join(", "),
        "Last 5 minutes, Last 15 minutes, Last 30 minutes, Last 1 hour, Last 3 hours, Last 6 hours, Last 12 hours, \
         Last 24 hours, Last 2 days, Last 7 days, Last 30 days, Last 90 days, Last 1 year"
    );
    assert_eq!(host.button_words("s1", &host.component("s1", "menu_o9")), "Last 7 days");
    assert_eq!(host.component("s1", "menu_o9")["variant"], "primary");
    // Each a row: a Button over a List of its words alone, so a renderer
    // draws it full width, the chosen one marked.
    assert_eq!(host.component("s1", "menu_o9")["child"], "menu_o9_c");
    assert_eq!((&host.component("s1", "menu_o9_c")["component"], &host.component("s1", "menu_o9_c")["children"]), (&json!("List"), &json!(["menu_o9_t"])));
    assert_eq!(host.component("s1", "menu_o8")["variant"], "borderless");
    host.press_id(&mut app, "s1", "range");
    assert!(host.tree("s1").iter().all(|component| component["id"] != "menu"));
    host.choose(&mut app, "s1", "range", "Last 24 hours");
    let (_, _, params) = host.queries.iter().find(|(_, sql, _)| sql.contains("FROM runs WHERE started")).unwrap().clone();
    assert!(params.as_array().unwrap().contains(&json!(NOW - 24 * HOUR)), "{params}");
    assert!(host.shows("s1", "Last 24 hours"));
    // In 24 hours: 2 runs.
    assert_eq!(host.surface("s1").data["panels"][0]["value"], 2);

    host.queries.clear();
    host.press_id(&mut app, "s1", "var0");
    assert!(host.shows("s1", "Builder"));
    assert_eq!(host.button("s1", "All")["variant"], "primary");
    host.press(&mut app, "s1", "Rivet");
    assert!(host.queries.iter().any(|(_, _, params)| params.as_array().unwrap().contains(&json!("rivet"))));
    assert_eq!(host.surface("s1").data["panels"][0]["value"], 1, "Rivet's run in the last day");
    assert!(host.shows("s1", "Agent: Rivet"));
    host.choose(&mut app, "s1", "var0", "All");
    assert_eq!(host.surface("s1").data["panels"][0]["value"], 2);

    // A year's range.
    host.choose(&mut app, "s1", "range", "Last 1 year");
    assert_eq!(host.surface("s1").data["panels"][0]["value"], 5, "the run 9 days ago too");
}

#[test]
fn a_fixed_range_shows_as_dates_stays_in_its_menu_and_spans_the_time_axis() {
    let (mut app, mut host) = opened(true);
    let (from, to) = (1_795_420_800_000_i64, 1_795_442_400_000_i64);
    let fixed = Dashboard {
        id: "fixed".into(),
        name: "Fixed".into(),
        time: model::TimeRange { from: from.to_string(), to: to.to_string() },
        ..every_kind()
    };
    model::save(&mut host, fixed, None, "rivet").unwrap();
    host.now += 30_000;
    app.on_timer(&mut host, REFRESH);
    host.choose(&mut app, "s1", "name", "Fixed");
    let span = |host: &Stand| {
        let line = host.tree("s1").into_iter().find(|component| component["kind"] == "timeseries").expect("a line");
        (line["timeFrom"].clone(), line["timeTo"].clone())
    };
    // Short enough for a phone's bar, in UTC: an app can't know the
    // device's time zone. The menu has Grafana's whole dates.
    let words = "2026-11-23 08:00:00 to 2026-11-23 14:00:00 UTC";
    assert_eq!(host.button_words("s1", &host.component("s1", "range")), "23 Nov 08:00 \u{2013} 14:00 UTC");
    assert_eq!(span(&host), (json!(from), json!(to)));
    assert!(host.tree("s1").iter().filter(|component| component["kind"] == "bars").all(|bars| bars.get("timeFrom").is_none()));

    // Its menu has it first, chosen; a quick range, then back to it.
    host.press_id(&mut app, "s1", "range");
    assert_eq!(host.button_words("s1", &host.component("s1", "menu_o0")), words);
    assert_eq!(host.component("s1", "menu_o0")["variant"], "primary");
    assert_eq!(host.component("s1", "menu_o10")["variant"], "borderless", "Last 7 days isn't chosen");
    host.press_id(&mut app, "s1", "range");
    host.choose(&mut app, "s1", "range", "Last 1 hour");
    assert!(host.shows("s1", "Last 1 hour"));
    assert_eq!(span(&host), (json!(host.now - HOUR), json!(host.now)));
    host.choose(&mut app, "s1", "range", words);
    assert_eq!(span(&host), (json!(from), json!(to)));

    // A fixed start up to now.
    assert_eq!(view::shown_range(&from.to_string(), "now"), "2026-11-23 08:00:00 to now UTC");
    assert_eq!(view::shown_range("now-45m", "now"), "Last 45 minutes");
    assert_eq!(view::shown_range("now-2d", "now-1d"), "Last 2 days to Last 1 day");
    assert_eq!(view::range_label("0"), "1970-01-01 00:00:00");
    let (day, second) = (86_400_000, 1000);
    assert_eq!(view::short_range(&from.to_string(), "now", NOW), "23 Nov 08:00 \u{2013} now UTC");
    assert_eq!(view::short_range(&from.to_string(), &(to + day + 5 * second).to_string(), NOW), "23 Nov 08:00 \u{2013} 24 Nov 14:00:05 UTC");
    assert_eq!(view::short_range(&(from - 365 * day).to_string(), &to.to_string(), NOW), "23 Nov 2025 08:00 \u{2013} 23 Nov 14:00 UTC");
    assert_eq!(view::short_range("now-7d", "now", NOW), "Last 7 days");
    assert_eq!(view::range_label("951782400000"), "2000-02-29 00:00:00");
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
    assert!(host.shows("s1", "Charts has nothing to show yet"));
    assert!(host.shows("s1", "Tell Rivet what you'd like to see, and it sets Charts up for you."));
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
    assert!(host.shows("s1", "Charts has nothing to show yet"));
}

#[test]
fn a_panel_whose_source_isnt_granted_says_so_while_the_others_show() {
    let (mut app, mut host) = opened(true);
    let mut dashboard = host.dashboard("claktory");
    dashboard.panels.push(panel("notes", "Notes", AGENT_DB, "SELECT COUNT(*) AS n FROM notes", "stat"));
    model::save(&mut host, dashboard, None, "rivet").unwrap();
    host.now += 30_000;
    app.on_timer(&mut host, REFRESH);
    assert_eq!(host.surface("s1").data["panels"][7]["error"], "To see this panel, Charts needs to read Rivet's database. Ask Rivet to set it up.");
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
    assert_eq!(host.surface("s1").data["editor"]["source"], json!([ACTIVITY]));

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
    assert_eq!((added.viz.kind.as_str(), added.grid), ("bars", Grid { x: 0, y: 3, w: 4, h: 8 }), "under the full last row");
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
    // The name is the dashboards' menu, the one shown chosen.
    assert_eq!(host.button("s1", "My agents' week")["id"], "name");
    host.press_id(&mut app, "s1", "name");
    assert_eq!(host.component("s1", "menu_o1")["variant"], "primary");
    host.press_id(&mut app, "s1", "name");

    host.press(&mut app, "s1", "Rename");
    assert_eq!(host.surface("s1").data["form"]["name"], "My agents' week");
    host.enter("s1", "/form/name", json!("Week"));
    host.press(&mut app, "s1", "Save");
    assert_eq!(host.dashboard("my-agents-week").name, "Week");

    host.choose(&mut app, "s1", "name", "Claktory");
    assert_eq!(host.surface("s1").data["panels"][0]["value"], 4);
    host.choose(&mut app, "s1", "name", "Week");
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
        let label = self.button_words(surface, &self.component(surface, id)).as_str().unwrap().to_owned();
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
        "query": {"source": "activity", "sql": "SELECT agent, COUNT(*) AS n FROM runs WHERE outcome = 'failed' AND started >= $from GROUP BY agent"},
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
fn heights_kept_before_0_5_0_move_to_grafanas_rows_once_and_stay_as_tall() {
    let mut host = stand(true);
    let old = |rev: u64, heights: &[u32]| {
        let panels: Vec<Value> = heights
            .iter()
            .enumerate()
            .map(|(y, h)| {
                json!({"id": format!("p{y}"), "title": "Runs", "grid": {"x": 0, "y": y, "w": 12, "h": h},
                       "query": {"source": "activity", "sql": "SELECT COUNT(*) FROM runs"}, "viz": {"kind": "bars"}})
            })
            .collect();
        serde_json::to_vec(&json!({"id": "old", "name": "Old", "panels": panels, "_rev": rev})).unwrap()
    };
    host.kv.insert(model::INDEX_KEY.into(), br#"["old"]"#.to_vec());
    host.kv.insert("dashboard/old".into(), old(2, &[1, 2, 4, 6, 9, 12]));
    host.kv.insert("previous/old".into(), old(1, &[4]));
    let mut app = App::default();
    app.start(&mut host);
    app.view_open(&mut host, "main", "s1").unwrap();

    // 50 points a unit, then 38 a row: each as tall as it was, near enough.
    let heights = |dashboard: &Dashboard| dashboard.panels.iter().map(|panel| panel.grid.h).collect::<Vec<_>>();
    assert_eq!(heights(&host.dashboard("old")), [1, 3, 5, 8, 12, 13], "12 units were 600, drawn 480; 13 rows are 486");
    let drawn: Vec<Value> = (0..6).map(|index| host.component("s1", &format!("p{index}_body"))["height"].clone()).collect();
    assert_eq!(drawn, [json!(120), json!(120), json!(182), json!(296), json!(448), json!(480)], "were 120, 120, 200, 300, 450, 480");
    assert_eq!(model::previous(&mut host, "old").map(|previous| heights(&previous)), Some(vec![5]));

    // Agents see and write Grafana's rows, never how Charts keeps them.
    let got = app.call(&mut host, "charts_get", r#"{"id": "old"}"#, "rivet").data.unwrap();
    assert_eq!(got["panels"][3]["grid"]["h"], 8);
    assert!(got.get("_v").is_none(), "{got}");
    let put = app.call(&mut host, "charts_put", &json!({"dashboard": got}).to_string(), "rivet");
    assert!(!put.is_error, "{}", put.text);
    let kept: Value = serde_json::from_slice(&host.kv["dashboard/old"]).unwrap();
    assert_eq!((&kept["_v"], &kept["panels"][3]["grid"]["h"]), (&json!(model::FORMAT), &json!(8)), "once: kept in rows");
    assert_eq!(heights(&host.dashboard("old")), [1, 3, 5, 8, 12, 13], "never moved twice");
    // The version it replaced, kept as the previous one, in rows too; put
    // back, it's as it was.
    let previous: Value = serde_json::from_slice(&host.kv["previous/old"]).unwrap();
    assert_eq!((&previous["_v"], &previous["panels"][2]["grid"]["h"]), (&json!(model::FORMAT), &json!(5)));
    model::restore(&mut host, "old", "").unwrap();
    assert_eq!(heights(&host.dashboard("old")), [1, 3, 5, 8, 12, 13]);
    // A panel an agent puts without a height is Grafana's new one's.
    let bare = json!({"dashboard": {"id": "new", "name": "New", "panels": [
        {"id": "a", "title": "A", "query": {"source": "activity", "sql": "SELECT COUNT(*) FROM runs"}, "viz": {"kind": "bars"}}]}});
    assert!(!app.call(&mut host, "charts_put", &bare.to_string(), "rivet").is_error);
    assert_eq!(heights(&host.dashboard("new")), [8]);
}

#[test]
fn a_put_says_what_to_fix_and_which_panels_fail() {
    let (mut app, mut host) = opened(true);
    let wrong = json!({"dashboard": {"id": "x", "name": "X", "panels": [
        {"id": "a", "title": "A", "query": {"source": "activity", "sql": "SELECT $nope"}, "viz": {"kind": "pie"}},
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
            {"id": "broken", "title": "Broken", "query": {"source": "activity", "sql": "SELECT x FROM nowhere"}, "viz": {"kind": "table"}},
            {"id": "mine", "title": "Mine", "query": {"source": "agent:rivet", "sql": "SELECT COUNT(*) FROM notes"}, "viz": {"kind": "stat"}},
        ]})
        .to_string(),
        "rivet",
    );
    assert!(!kept.is_error, "{}", kept.text);
    assert!(kept.text.contains("Panel broken's query failed: no such table: nowhere"), "{}", kept.text);
    assert!(kept.text.contains("Panel mine shows nothing until the person gives Charts Rivet's database"), "{}", kept.text);
    assert_eq!(model::ids(&mut host), vec!["claktory", "notes"]);
}

#[test]
fn an_agent_deletes_and_queries() {
    let (mut app, mut host) = opened(true);
    let queried = app.call(
        &mut host,
        "charts_query",
        r#"{"source": "activity", "sql": "SELECT agent, COUNT(*) AS n FROM runs WHERE started >= $from AND agent != $skip GROUP BY agent", "from": "now-24h", "variables": {"skip": "nobody"}}"#,
        "rivet",
    );
    assert!(!queried.is_error, "{}", queried.text);
    assert_eq!(queried.text, "agent\tn\nbuilder\t1\nrivet\t1\n(2 rows)");
    for (args, why) in [
        (r#"{"source": "activity", "sql": "SELECT ?1"}"#, "not ?"),
        (r#"{"source": "activity", "sql": "SELECT $who"}"#, "$who isn't a variable"),
        (r#"{"source": "agent:rivet", "sql": "SELECT 1"}"#, "hasn't given Charts Rivet's database"),
        (r#"{"source": "elsewhere", "sql": "SELECT 1"}"#, "doesn't read \"elsewhere\""),
        (r#"{"source": "activity", "sql": "SELECT 1", "from": "later"}"#, "from and to are"),
        (r#"{"source": "activity", "sql": "SELECT $from", "variables": {"from": 1}}"#, "is built in"),
    ] {
        let refused = app.call(&mut host, "charts_query", args, "rivet");
        assert!(refused.is_error && refused.text.contains(why), "{why} in {}", refused.text);
    }
    // A panel's SQL as it is, its variables at their defaults, or given.
    let panel = r#"SELECT COUNT(*) AS n FROM runs WHERE started >= $from AND ($agent = '' OR agent = $agent)"#;
    let args = |extra: Value| {
        let mut args = json!({"source": "activity", "sql": panel});
        args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        args.to_string()
    };
    let refused = app.call(&mut host, "charts_query", &args(json!({})), "rivet");
    assert!(refused.text.contains("$agent isn't a variable") && refused.text.contains("\"dashboard\": \"<its id>\""), "{}", refused.text);
    assert_eq!(app.call(&mut host, "charts_query", &args(json!({"dashboard": "claktory"})), "rivet").text, "n\n4\n(1 row)");
    let rivet = app.call(&mut host, "charts_query", &args(json!({"dashboard": "claktory", "variables": {"agent": "rivet"}})), "rivet");
    assert_eq!(rivet.text, "n\n2\n(1 row)");
    let many = app.call(&mut host, "charts_query", r#"{"source": "activity", "sql": "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 500) SELECT i FROM n"}"#, "rivet");
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

#[test]
fn a_dashboard_made_for_all_of_claktorys_records_reads_activity_where_charts_no_longer_has_them() {
    let activity = vec![DataSource { id: ACTIVITY.into(), label: "Activity".into(), granted: true }];
    assert_eq!(source_for(RECORDS, &activity), ACTIVITY);
    assert_eq!(source_for("agent:rivet", &activity), "agent:rivet");
    let records = vec![DataSource { id: RECORDS.into(), label: "All of Claktory's records".into(), granted: true }];
    assert_eq!(source_for(RECORDS, &records), RECORDS, "kept where it was given before");
}

// ---- Dashboards saved by earlier versions ----

/// The store and views of Charts 0.4.0, frozen: `saved_0_4_0_store.json` is
/// the store as 0.4.0 left it (its default dashboard and one with a panel
/// of every kind), and `saved_0_4_0_views.json` what each dashboard showed
/// then, components and data. `CHARTS_FREEZE=1` writes them again, only
/// ever from 0.4.0's code.
const SAVED_STORE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/testdata/saved_0_4_0_store.json");
const SAVED_VIEWS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/testdata/saved_0_4_0_views.json");

/// A dashboard with a panel of every kind, as the person and agents made
/// them with 0.4.0.
fn every_kind() -> Dashboard {
    let activity = |sql: &str| Query { source: ACTIVITY.into(), sql: sql.into() };
    let over_time = "SELECT (started / $step) * $step AS time, COUNT(*) AS runs, COUNT(*) * 2 AS doubled FROM runs \
                     WHERE started >= $from AND started < $to GROUP BY 1 ORDER BY 1";
    let by_agent = "SELECT (started / $step) * $step AS time, agent, COUNT(*) AS runs FROM runs \
                    WHERE started >= $from AND started < $to GROUP BY 1, 2 ORDER BY 1";
    let per_agent = "SELECT agent, COUNT(*) AS runs FROM runs WHERE started >= $from AND started < $to GROUP BY 1 ORDER BY 1";
    let panel = |id: &str, title: &str, grid: Grid, sql: &str, viz: Viz| Panel {
        id: id.into(),
        title: title.into(),
        grid,
        query: activity(sql),
        viz,
    };
    let viz = |kind: &str| Viz { kind: kind.into(), ..Viz::default() };
    Dashboard {
        id: "every-kind".into(),
        name: "Every kind".into(),
        time: model::TimeRange { from: "now-30d".into(), to: "now".into() },
        refresh_s: 60,
        variables: Vec::new(),
        panels: vec![
            panel("total", "Runs", Grid { x: 0, y: 0, w: 4, h: 2 }, over_time, Viz { unit: "runs".into(), ..viz("stat") }),
            panel("muted", "Quiet", Grid { x: 4, y: 0, w: 4, h: 2 }, over_time, Viz { color: Some("muted".into()), ..viz("stat") }),
            panel(
                "line",
                "Runs over time",
                Grid { x: 0, y: 1, w: 12, h: 5 },
                over_time,
                Viz { x: Some("time".into()), y: vec!["runs".into()], unit: "runs".into(), ..viz("timeseries") },
            ),
            panel(
                "lines",
                "Runs by agent",
                Grid { x: 0, y: 2, w: 6, h: 4 },
                by_agent,
                Viz { series_by: Some("agent".into()), color: Some("agent".into()), ..viz("timeseries") },
            ),
            panel("scatter", "Doubled", Grid { x: 6, y: 2, w: 6, h: 4 }, over_time, Viz { x: Some("runs".into()), ..viz("scatter") }),
            panel("bars", "Per agent", Grid { x: 0, y: 3, w: 4, h: 3 }, per_agent, viz("bars")),
            panel("stacked", "Stacked", Grid { x: 4, y: 3, w: 4, h: 3 }, by_agent, Viz { series_by: Some("agent".into()), ..viz("stackedBars") }),
            panel("pie", "Share", Grid { x: 8, y: 3, w: 4, h: 3 }, per_agent, Viz { color: Some("palette".into()), ..viz("pie") }),
            panel(
                "table",
                "Runs table",
                Grid { x: 3, y: 4, w: 6, h: 4 },
                per_agent,
                Viz {
                    columns: vec![model::Column { name: "agent".into(), title: Some("Agent".into()), format: Some("text".into()) }],
                    ..viz("table")
                },
            ),
        ],
        rev: 0,
        by: String::new(),
        format: model::FORMAT,
    }
}

/// A component as 0.4.0 showed it, with what's changed on purpose since,
/// each change checked: a stat no longer says signal (orange) for want of
/// a color: it says one threshold, green, instead (its total in one color,
/// as before, not the renderer's default red from 80); and it says its
/// height, so its number fills it (#380 step 2). A row of panels says
/// below what width they go one under another (step 4).
fn since_0_4_0(was: &Value, is: &Value) -> Value {
    let mut was = was.clone();
    // A row of panels goes one under another on a phone.
    if was["component"] == "Row" {
        was["stackBelow"] = json!(768);
    }
    if was["component"] == "Stat" {
        let fields = was.as_object_mut().unwrap();
        if fields.get("color") == Some(&json!("signal")) {
            fields.remove("color");
            fields.insert("thresholds".into(), json!([{"color": "green"}]));
        }
        let height = is["height"].as_u64().expect("a stat says its height");
        assert!((60..=480).contains(&height), "{is}");
        fields.insert("height".into(), json!(height));
    }
    // Heights in Grafana's rows, about as tall as they were.
    if was["component"] == "Chart" {
        let (then, now) = (was["height"].as_i64().unwrap(), is["height"].as_i64().expect("a chart says its height"));
        assert!((120..=480).contains(&now) && (now - then).abs() <= 27, "{was} {is}");
        was["height"] = json!(now);
    }
    // A time axis spans the range shown.
    if was["component"] == "Chart" && was["kind"] == "timeseries" {
        let (from, to) = (is["timeFrom"].as_i64().expect("its start"), is["timeTo"].as_i64().expect("its end"));
        assert!(0 <= from && from < to, "{is}");
        was["timeFrom"] = json!(from);
        was["timeTo"] = json!(to);
    }
    was
}

/// What a surface shows: its components from root, and its data.
fn shown(host: &Stand, surface: &str) -> Value {
    json!({"components": host.tree(surface), "data": host.surface(surface).data})
}

#[test]
fn dashboards_saved_by_0_4_0_show_as_they_did() {
    let freeze = std::env::var("CHARTS_FREEZE").is_ok_and(|value| value == "1");
    let mut host = stand(true);
    if freeze {
        App::default().start(&mut host);
        model::save(&mut host, every_kind(), None, "rivet").unwrap();
        let store: serde_json::Map<String, Value> =
            host.kv.iter().map(|(key, value)| (key.clone(), serde_json::from_slice(value).unwrap())).collect();
        std::fs::create_dir_all(std::path::Path::new(SAVED_STORE).parent().unwrap()).unwrap();
        std::fs::write(SAVED_STORE, serde_json::to_string_pretty(&store).unwrap() + "\n").unwrap();
    } else {
        let store: serde_json::Map<String, Value> = serde_json::from_str(&std::fs::read_to_string(SAVED_STORE).unwrap()).unwrap();
        host.kv = store.into_iter().map(|(key, value)| (key, serde_json::to_vec(&value).unwrap())).collect();
    }
    let mut app = App::default();
    app.start(&mut host);
    let mut views = serde_json::Map::new();
    for (index, id) in model::ids(&mut host).iter().enumerate() {
        let surface = format!("s{index}");
        app.view_open(&mut host, "main", &surface).unwrap();
        let name = host.dashboard(id).name;
        if index > 0 {
            host.choose(&mut app, &surface, "name", &name);
        }
        views.insert(id.clone(), shown(&host, &surface));
    }
    if freeze {
        std::fs::write(SAVED_VIEWS, serde_json::to_string_pretty(&Value::Object(views)).unwrap() + "\n").unwrap();
        return;
    }
    let then: serde_json::Map<String, Value> = serde_json::from_str(&std::fs::read_to_string(SAVED_VIEWS).unwrap()).unwrap();
    assert_eq!(views.keys().collect::<Vec<_>>(), then.keys().collect::<Vec<_>>());
    for (id, was) in &then {
        let is = &views[id];
        // Component by component, so a failure says which one changed: the
        // panels' (their rows and cards), not the controls above them,
        // which are menus since 0.5.0.
        let panels = |components: &Value| -> Vec<Value> {
            let ids = |component: &&Value| {
                let id = component["id"].as_str().unwrap();
                id.starts_with("row") || id.strip_prefix('p').is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
            };
            components.as_array().unwrap().iter().filter(ids).cloned().collect()
        };
        let (was, is) = (&panels(&was["components"]), &panels(&is["components"]));
        assert!(was.len() > 10, "{id}: {was:?}");
        for (was, is) in was.iter().zip(is) {
            assert_eq!(is, &since_0_4_0(was, is), "{id}: {} changed", was["id"]);
        }
        assert_eq!(is.len(), was.len(), "{id}: as many components");
        assert_eq!(views[id]["data"], then[id]["data"], "{id}: its data");
    }
}

// ---- How a chart draws ----

#[test]
fn the_person_sets_how_a_line_draws_and_defaults_stay_unset() {
    let (mut app, mut host) = opened(true);
    host.press(&mut app, "s1", "Edit");
    host.press(&mut app, "s1", "Add panel");
    host.enter("s1", "/editor/title", json!("Runs over time"));
    host.enter("s1", "/editor/sql", json!("SELECT (started / $step) * $step AS time, COUNT(*) AS runs FROM runs GROUP BY 1"));
    host.press(&mut app, "s1", "Run");
    let preview = host.component("s1", "ed_preview_body");
    assert_eq!(preview["kind"], "timeseries");
    for key in ["lineWidth", "fill", "points", "pointSize", "min", "max", "decimals", "legend"] {
        assert!(preview.get(key).is_none(), "{key} is left to the renderer: {preview}");
    }
    for (label, chips) in [("Line width", "None 1 2 3 5"), ("Fill", "None 10% 25% 50% 100%"), ("Points", "Auto Always Never")] {
        let picker = host.tree("s1").into_iter().find(|component| component["label"] == label).unwrap();
        let labels: Vec<&str> = picker["options"].as_array().unwrap().iter().map(|option| option["label"].as_str().unwrap()).collect();
        assert_eq!(labels.join(" "), chips);
    }

    host.enter("s1", "/editor/line_width", json!(["2"]));
    host.enter("s1", "/editor/fill", json!(["25"]));
    host.enter("s1", "/editor/points", json!(["always"]));
    host.enter("s1", "/editor/legend", json!(false));
    host.enter("s1", "/editor/min", json!("0"));
    host.enter("s1", "/editor/max", json!("x"));
    host.press(&mut app, "s1", "Run");
    assert!(host.shows("s1", "Enter numbers for min and max, or leave them empty to fit the data."), "{:?}", host.words("s1"));
    host.enter("s1", "/editor/max", json!("-1"));
    host.press(&mut app, "s1", "Save");
    assert!(host.shows("s1", "Make min lower than max."));
    host.enter("s1", "/editor/max", json!("100"));
    host.enter("s1", "/editor/decimals", json!("11"));
    host.press(&mut app, "s1", "Save");
    assert!(host.shows("s1", "Enter a whole number from 0 to 10 for decimals, or leave it empty."));
    assert!(host.dashboard("claktory").panel("runs_over_time").is_none(), "nothing kept yet");
    host.enter("s1", "/editor/decimals", json!("1"));
    host.press(&mut app, "s1", "Run");
    let preview = host.component("s1", "ed_preview_body");
    let style = |chart: &Value| {
        ["lineWidth", "fill", "points", "min", "max", "decimals", "legend"].map(|key| chart[key].clone()).to_vec()
    };
    let set = vec![json!(2), json!(25), json!("always"), json!(0), json!(100), json!(1), json!(false)];
    assert_eq!(style(&preview), set);

    host.press(&mut app, "s1", "Save");
    let panel = host.dashboard("claktory").panel("runs_over_time").unwrap().clone();
    assert_eq!(
        (panel.viz.line_width, panel.viz.fill, panel.viz.points.as_deref(), panel.viz.legend),
        (Some(2.0), Some(25.0), Some("always"), Some(false))
    );
    assert_eq!((panel.viz.min, panel.viz.max, panel.viz.decimals), (Some(0.0), Some(100.0), Some(1)));
    let index = host.dashboard("claktory").panels.iter().position(|p| p.id == "runs_over_time").unwrap();
    assert_eq!(style(&host.component("s1", &format!("p{index}_body"))), set, "the panel draws as the preview did");

    // Editing it shows what it has; back at the defaults, they're unset again.
    host.press_id(&mut app, "s1", &format!("p{index}_edit"));
    let editor = host.surface("s1").data["editor"].clone();
    assert_eq!(
        [&editor["line_width"], &editor["fill"], &editor["points"], &editor["legend"], &editor["min"], &editor["max"], &editor["decimals"]],
        [&json!(["2"]), &json!(["25"]), &json!(["always"]), &json!(false), &json!("0"), &json!("100"), &json!("1")]
    );
    host.enter("s1", "/editor/line_width", json!(["1"]));
    host.enter("s1", "/editor/fill", json!(["0"]));
    host.enter("s1", "/editor/points", json!(["auto"]));
    host.enter("s1", "/editor/legend", json!(true));
    for field in ["min", "max", "decimals"] {
        host.enter("s1", &format!("/editor/{field}"), json!(""));
    }
    host.press(&mut app, "s1", "Save");
    let panel = host.dashboard("claktory").panel("runs_over_time").unwrap().clone();
    assert_eq!(Viz { kind: "timeseries".into(), ..Viz::default() }, panel.viz);
    host.tree("s1");
}

#[test]
fn an_agents_style_is_checked_kept_by_the_editor_and_drawn() {
    let (mut app, mut host) = opened(true);
    let mut dashboard = host.dashboard("claktory");
    let viz = &mut dashboard.panels[4].viz;
    viz.line_width = Some(11.0);
    viz.fill = Some(-1.0);
    viz.points = Some("sometimes".into());
    viz.point_size = Some(0.0);
    viz.min = Some(5.0);
    viz.max = Some(5.0);
    viz.decimals = Some(11);
    let problems = dashboard.problems(&sources());
    for expected in [
        "panels[4].viz.line_width: 0 to 10",
        "panels[4].viz.fill: 0 to 100 (percent)",
        "panels[4].viz.points: one of auto, always, never",
        "panels[4].viz.point_size: 1 to 40",
        "panels[4].viz.min: below max",
        "panels[4].viz.decimals: 0 to 10",
    ] {
        assert!(problems.iter().any(|problem| problem == expected), "{expected} in {problems:#?}");
    }

    // An agent's choices the editor has no chip for show as one, and what
    // the editor doesn't show (point size) is kept.
    let viz = &mut dashboard.panels[4].viz;
    *viz = Viz { line_width: Some(4.0), fill: Some(15.0), point_size: Some(8.0), min: Some(-2.5), ..viz.clone() };
    viz.points = None;
    viz.max = None;
    viz.decimals = None;
    assert!(dashboard.problems(&sources()).is_empty(), "{:?}", dashboard.problems(&sources()));
    model::save(&mut host, dashboard, Some(1), "rivet").unwrap();
    host.press(&mut app, "s1", "Edit");
    let body = host.component("s1", "p4_body");
    assert_eq!((&body["lineWidth"], &body["fill"], &body["pointSize"], &body["min"]), (&json!(4), &json!(15), &json!(8), &json!(-2.5)));
    host.press_id(&mut app, "s1", "p4_edit");
    let line = host.component("s1", "ed_line");
    assert_eq!(line["options"].as_array().unwrap().last().unwrap(), &json!({"label": "4", "value": "4"}));
    assert_eq!(host.component("s1", "ed_fill")["options"].as_array().unwrap().last().unwrap(), &json!({"label": "15%", "value": "15"}));
    host.enter("s1", "/editor/title", json!("Tokens, by agent"));
    host.press(&mut app, "s1", "Save");
    let viz = host.dashboard("claktory").panels[4].viz.clone();
    assert_eq!((viz.line_width, viz.fill, viz.point_size, viz.min), (Some(4.0), Some(15.0), Some(8.0), Some(-2.5)));
}

// ---- What a number shows ----

#[test]
fn a_stat_shows_its_columns_last_total_mean_max_or_min() {
    let table = json!({"columns": ["time", "n"], "rows": [[1, 1], [2, null], [3, 3.5], [4, 2]]});
    let value = |reduce: &str| view::stat(&table, reduce)["value"].clone();
    assert_eq!(
        model::REDUCERS.map(value).to_vec(),
        vec![json!(2), json!(6.5), json!(6.5 / 3.0), json!(3.5), json!(1)],
        "last, sum, mean, max, min of the numbers, nulls left out"
    );
    assert_eq!(view::stat(&table, "last")["spark"], json!([1.0, 3.5, 2.0]));
    let empty = json!({"columns": ["n"], "rows": []});
    assert_eq!(view::stat(&empty, "sum")["value"], 0, "a total of nothing is 0");
    for reduce in ["last", "mean", "max", "min"] {
        assert!(view::stat(&empty, reduce)["value"].is_null(), "{reduce} of nothing is nothing");
    }
}

#[test]
fn saved_stats_keep_their_total_and_new_ones_show_the_last_number() {
    let (mut app, mut host) = opened(true);
    let total = host.surface("s1").data["panels"][0]["value"].clone();
    assert_eq!(total, 4);

    // The person edits a saved stat: it says Total, and keeps it.
    host.press(&mut app, "s1", "Edit");
    host.press_id(&mut app, "s1", "p0_edit");
    assert_eq!(host.surface("s1").data["editor"]["reduce"], json!(["sum"]));
    assert_eq!(host.surface("s1").data["editor"]["thresholds"], "green", "its one color, written out");
    let shows = host.component("s1", "ed_reduce");
    assert_eq!(shows["label"], "Number shows");
    let labels: Vec<&str> = shows["options"].as_array().unwrap().iter().map(|option| option["label"].as_str().unwrap()).collect();
    assert_eq!(labels, ["Last", "Total", "Mean", "Max", "Min"]);
    host.enter("s1", "/editor/title", json!("All runs"));
    host.press(&mut app, "s1", "Save");
    assert_eq!(host.dashboard("claktory").panels[0].viz.reduce.as_deref(), Some("sum"));
    assert_eq!(host.dashboard("claktory").panels[0].viz.thresholds, vec![model::Threshold { color: "green".into(), value: None }]);
    assert_eq!(host.surface("s1").data["panels"][0]["value"], total);
    assert_eq!(host.component("s1", "p0_body")["thresholds"], json!([{"color": "green"}]), "still green");

    // A new one from the editor shows the last number.
    host.press(&mut app, "s1", "Add panel");
    assert_eq!(host.surface("s1").data["editor"]["reduce"], json!(["last"]));
    host.enter("s1", "/editor/title", json!("Runs now"));
    host.enter("s1", "/editor/kind", json!(["stat"]));
    host.enter("s1", "/editor/sql", json!("SELECT 1 AS time, 5 AS n UNION ALL SELECT 2, 7"));
    host.press(&mut app, "s1", "Save");
    let dashboard = host.dashboard("claktory");
    let index = dashboard.panels.iter().position(|panel| panel.id == "runs_now").unwrap();
    assert_eq!(dashboard.panels[index].viz.reduce.as_deref(), Some("last"));
    assert_eq!(host.surface("s1").data["panels"][index]["value"], 7);
    assert!(host.component("s1", &format!("p{index}_body")).get("thresholds").is_none(), "a new one: the renderer's default steps");

    // From an agent: a new stat that doesn't say shows the last number; one
    // kept before, or brought back, its total; what one says, it keeps.
    let mut put = host.dashboard("claktory");
    put.panels[1].viz.reduce = None;
    put.panels.push(panel("new_stat", "New", ACTIVITY, "SELECT 1 AS n", "stat"));
    put.panels.push(Panel { id: "said".into(), ..panel("said", "Said", ACTIVITY, "SELECT 1 AS n", "stat") });
    put.panels.last_mut().unwrap().viz.reduce = Some("max".into());
    let mut turned = put.panels[4].clone();
    turned.viz = Viz { kind: "stat".into(), ..Viz::default() };
    put.panels[4] = turned;
    let output = app.call(&mut host, "charts_put", &json!({"dashboard": put}).to_string(), "rivet");
    assert!(!output.is_error, "{}", output.text);
    let kept = host.dashboard("claktory");
    let reduce = |id: &str| kept.panel(id).unwrap().viz.reduce.clone();
    assert_eq!(reduce("messages"), None, "a stat kept before keeps its total");
    assert_eq!(reduce("new_stat").as_deref(), Some("last"));
    assert_eq!(reduce("said").as_deref(), Some("max"));
    assert_eq!(reduce("tokens_by_agent").as_deref(), Some("last"), "a chart turned into a stat is a new stat");
    // Put again without saying: each keeps what it showed (Scolippi 24818).
    let mut again = kept.clone();
    for panel in &mut again.panels {
        panel.viz.reduce = None;
    }
    assert!(!app.call(&mut host, "charts_put", &json!({"dashboard": again}).to_string(), "rivet").is_error);
    let kept = host.dashboard("claktory");
    let reduce = |id: &str| kept.panel(id).unwrap().viz.reduce.clone();
    assert_eq!((reduce("new_stat").as_deref(), reduce("said").as_deref(), reduce("messages")), (Some("last"), Some("max"), None));

    let old = model::default_dashboard();
    model::delete(&mut host, "claktory").unwrap();
    let output = app.call(&mut host, "charts_put", &json!({"dashboard": old}).to_string(), "rivet");
    assert!(!output.is_error, "{}", output.text);
    assert!(
        host.dashboard("claktory").panels.iter().all(|panel| matches!(panel.viz.reduce.as_deref(), None | Some("sum"))),
        "brought back, its stats keep their totals"
    );

    let mut wrong = host.dashboard("claktory");
    wrong.panels[0].viz.reduce = Some("median".into());
    let output = app.call(&mut host, "charts_put", &json!({"dashboard": wrong}).to_string(), "rivet");
    assert!(output.text.contains("panels[0].viz.reduce: one of last, sum, mean, max, min"), "{}", output.text);
}

#[test]
fn the_person_sets_a_numbers_thresholds_color_mode_and_sparkline() {
    let (mut app, mut host) = opened(true);
    let runs = host.component("s1", "p0_body");
    assert_eq!((&runs["component"], &runs["height"]), (&json!("Stat"), &json!(140)), "2 units, but at least 140");
    for key in ["color", "colorMode", "graph", "decimals"] {
        assert!(runs.get(key).is_none(), "{key} is left to the renderer: {runs}");
    }
    // Its total in one color, as before 0.5.0, not red from 80.
    assert_eq!(runs["thresholds"], json!([{"color": "green"}]));

    host.press(&mut app, "s1", "Edit");
    host.press(&mut app, "s1", "Add panel");
    let modes = host.component("s1", "ed_color_mode");
    let labels: Vec<&str> = modes["options"].as_array().unwrap().iter().map(|option| option["label"].as_str().unwrap()).collect();
    assert_eq!(labels, ["Value", "Background gradient", "Background solid", "None"]);
    assert_eq!(host.surface("s1").data["editor"]["graph"], true);
    host.enter("s1", "/editor/title", json!("Load"));
    host.enter("s1", "/editor/kind", json!(["stat"]));
    host.enter("s1", "/editor/sql", json!("SELECT 1 AS time, 42.25 AS load"));
    for (written, why) in [
        ("green 80", "Enter a color, then a value and a color for each step, such as \u{201c}green, 80 red\u{201d}."),
        ("green, red", "Enter a color, then a value"),
        ("green, 80 pink", "\u{201c}pink\u{201d} isn't a color here. Use green, yellow, semi-dark-yellow"),
        ("red, 80 blue, 50 green", "Make each threshold value higher than the one before."),
    ] {
        host.enter("s1", "/editor/thresholds", json!(written));
        host.press(&mut app, "s1", "Save");
        assert!(host.shows("s1", why), "{written}: {:?}", host.words("s1"));
    }
    host.enter("s1", "/editor/thresholds", json!(" Green, 50 semi-dark-yellow ,80 RED"));
    host.enter("s1", "/editor/color_mode", json!(["background"]));
    host.enter("s1", "/editor/graph", json!(false));
    host.enter("s1", "/editor/decimals", json!("1"));
    host.press(&mut app, "s1", "Save");
    let dashboard = host.dashboard("claktory");
    let index = dashboard.panels.iter().position(|panel| panel.id == "load").unwrap();
    let viz = &dashboard.panels[index].viz;
    let steps = |pairs: &[(&str, Option<f64>)]| -> Vec<model::Threshold> {
        pairs.iter().map(|(color, value)| model::Threshold { color: (*color).into(), value: *value }).collect()
    };
    assert_eq!(viz.thresholds, steps(&[("green", None), ("semi-dark-yellow", Some(50.0)), ("red", Some(80.0))]));
    assert_eq!((viz.color_mode.as_deref(), viz.graph.as_deref(), viz.decimals), (Some("background"), Some("none"), Some(1)));
    let stat = host.component("s1", &format!("p{index}_body"));
    assert_eq!(stat["thresholds"], json!([{"color": "green"}, {"color": "semi-dark-yellow", "value": 50.0}, {"color": "red", "value": 80.0}]));
    assert_eq!((&stat["colorMode"], &stat["graph"], &stat["decimals"], &stat["height"]), (&json!("background"), &json!("none"), &json!(1), &json!(296)), "Grafana's 8 rows");
    assert_eq!(host.surface("s1").data["panels"][index]["value"], 42.25);

    // Editing it writes them as they were entered; back at the defaults,
    // they're unset.
    host.press_id(&mut app, "s1", &format!("p{index}_edit"));
    let editor = host.surface("s1").data["editor"].clone();
    assert_eq!((&editor["thresholds"], &editor["color_mode"], &editor["graph"]), (&json!("green, 50 semi-dark-yellow, 80 red"), &json!(["background"]), &json!(false)));
    host.enter("s1", "/editor/thresholds", json!(""));
    host.enter("s1", "/editor/color_mode", json!(["value"]));
    host.enter("s1", "/editor/graph", json!(true));
    host.press(&mut app, "s1", "Save");
    let viz = host.dashboard("claktory").panels[index].viz.clone();
    assert_eq!((viz.thresholds.len(), viz.color_mode, viz.graph), (0, None, None));
}

#[test]
fn an_agents_stat_style_is_checked_and_a_fixed_color_kept() {
    let (mut app, mut host) = opened(true);
    let mut dashboard = host.dashboard("claktory");
    let threshold = |color: &str, value: Option<f64>| model::Threshold { color: color.into(), value };
    let cases: [(Vec<model::Threshold>, &str); 4] = [
        (vec![threshold("green", Some(1.0))], "panels[0].viz.thresholds: the first is the base, with no value"),
        (vec![threshold("green", None), threshold("red", None)], "panels[0].viz.thresholds: each after the first has a value"),
        (vec![threshold("#00ff00", None)], "panels[0].viz.thresholds: \"#00ff00\" isn't a color"),
        (vec![threshold("green", None); 11], "panels[0].viz.thresholds: at most 10"),
    ];
    for (steps, expected) in cases {
        dashboard.panels[0].viz.thresholds = steps;
        let problems = dashboard.problems(&sources());
        assert!(problems.iter().any(|problem| problem.starts_with(expected)), "{expected} in {problems:#?}");
    }
    dashboard.panels[0].viz.thresholds = vec![threshold("blue", None), threshold("dark-red", Some(-5.0))];
    dashboard.panels[0].viz.color_mode = Some("gradient".into());
    dashboard.panels[0].viz.graph = Some("line".into());
    dashboard.panels[1].viz.color = Some("red".into());
    dashboard.panels[4].viz.color = Some("red".into());
    let problems = dashboard.problems(&sources());
    for expected in [
        "panels[0].viz.color_mode: one of value, background, solid, none",
        "panels[0].viz.graph: one of area, none",
        "panels[4].viz.color: one of palette, agent, signal, muted",
    ] {
        assert!(problems.iter().any(|problem| problem == expected), "{expected} in {problems:#?}");
    }
    assert!(!problems.iter().any(|problem| problem.starts_with("panels[1]") || problem.contains("thresholds")), "{problems:#?}");

    // A stat's fixed color reaches the renderer; muted as before.
    dashboard.panels[0].viz.color_mode = Some("solid".into());
    dashboard.panels[0].viz.graph = Some("area".into());
    dashboard.panels[2].viz.color = Some("muted".into());
    dashboard.panels[4].viz.color = Some("agent".into());
    let output = app.call(&mut host, "charts_put", &json!({"dashboard": dashboard}).to_string(), "rivet");
    assert!(!output.is_error, "{}", output.text);
    app.on_timer(&mut host, REFRESH);
    let stat = |index: usize| host.component("s1", &format!("p{index}_body"));
    assert_eq!(stat(0)["thresholds"], json!([{"color": "blue"}, {"color": "dark-red", "value": -5.0}]));
    assert_eq!((&stat(0)["colorMode"], &stat(0)["graph"]), (&json!("solid"), &json!("area")));
    assert_eq!(stat(1)["color"], "red");
    assert_eq!(stat(2)["color"], "muted");
    assert!(stat(3).get("color").is_none());
}
