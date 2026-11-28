//! The app against a stand-in host: a store, timers, sources whose queries
//! run in SQLite (tables shaped like Claktory's records, Activity refusing
//! what only Schedule reads), a Mac clock with a daylight-saving change,
//! and surfaces that apply what's pushed as a renderer would.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::cron::Cron;
use crate::time::{self, DAY, HOUR, MINUTE, Written, Zone};
use crate::*;

/// Thursday 3 December 2026, 13:00 UTC.
fn now() -> i64 {
    time::days_from_civil(2026, 12, 3) * DAY + 13 * HOUR
}

fn day(y: i64, m: u32, d: u32) -> i64 {
    time::days_from_civil(y, m, d) * DAY
}

#[derive(Default, Clone)]
struct Surface {
    components: BTreeMap<String, Value>,
    data: Value,
}

struct Stand {
    kv: BTreeMap<String, Vec<u8>>,
    timers: BTreeMap<String, u64>,
    activity: bool,
    schedule: bool,
    db: rusqlite::Connection,
    /// The Mac's offset in minutes: from each UTC ms on, in order.
    zone: Vec<(i64, i32)>,
    queries: Vec<(String, String)>,
    surfaces: BTreeMap<String, Surface>,
    caller: Option<Caller>,
    told: Vec<(String, String, String)>,
    pushed: usize,
    now: i64,
}

impl Stand {
    fn new() -> Self {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE runs (id INTEGER PRIMARY KEY, agent TEXT, version TEXT, started INTEGER, ended INTEGER, outcome TEXT, last_call INTEGER);
             CREATE TABLE agent_usage (hour INTEGER, provider TEXT, model TEXT, agent TEXT, agent_name TEXT, requests INTEGER, input INTEGER, output INTEGER, cache_read INTEGER, cache_write INTEGER);
             CREATE TABLE tasks (id INTEGER PRIMARY KEY, agent TEXT, task TEXT, command TEXT, started INTEGER, ended INTEGER, status TEXT);
             CREATE TABLE monitors (agent TEXT, id INTEGER, name TEXT, prompt TEXT, kind TEXT, spec TEXT, until INTEGER, max_fires INTEGER,
                 created INTEGER, state TEXT, next_due INTEGER, last_fired INTEGER, fires INTEGER, updated INTEGER);
             CREATE TABLE monitor_events (seq INTEGER PRIMARY KEY, agent TEXT, monitor INTEGER, time INTEGER, actor TEXT, kind TEXT, value TEXT);
             CREATE TABLE checklist_items (agent TEXT, id INTEGER, text TEXT, status TEXT, owner TEXT, created INTEGER, updated INTEGER, retry_at INTEGER, reask_at INTEGER);
             CREATE TABLE agents (id TEXT, name TEXT, project TEXT, project_name TEXT, admin INTEGER);
             CREATE VIEW monitor_specs AS SELECT agent, id, kind, spec FROM monitors WHERE kind IN ('at', 'every', 'cron');
             INSERT INTO agents VALUES ('fugo', 'Fugo', 'claktory', 'Claktory', 0), ('lucy', 'Lucy', 'claktory', 'Claktory', 0),
                 ('giorno', 'Giorno', 'claktory', 'Claktory', 0), ('kenji', 'Kenji', 'web-site', 'Web Site', 0), ('rivet', 'Rivet', NULL, NULL, 1);
             INSERT INTO agent_usage VALUES (0, 'p', 'm', 'fugo', 'Fugo', 1, 1, 1, 1, 1), (0, 'p', 'm', 'lucy', 'Lucy', 1, 1, 1, 1, 1);",
        )
        .unwrap();
        // Rome's clock: +1, then +2 from 28 March 2027 01:00 UTC.
        let summer = day(2027, 3, 28) + HOUR;
        Stand {
            kv: BTreeMap::new(),
            timers: BTreeMap::new(),
            activity: true,
            schedule: true,
            db,
            zone: vec![(i64::MIN, 60), (summer, 120)],
            queries: Vec::new(),
            surfaces: BTreeMap::new(),
            caller: None,
            told: Vec::new(),
            pushed: 0,
            now: now(),
        }
    }

    fn sql(&self, sql: &str) {
        self.db.execute_batch(sql).unwrap();
    }

    fn offset(&self, utc: i64) -> i32 {
        self.zone.iter().rev().find(|(from, _)| *from <= utc).map_or(0, |(_, offset)| *offset)
    }

    fn as_agent(&mut self, id: &str, project: Option<&str>) {
        self.caller = Some(Caller { id: id.into(), project: project.map(str::to_owned), admin: false, tainted: false });
    }
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
        assert_eq!(message["version"], "1.0");
        self.pushed += 1;
        let shown = self.surfaces.entry(surface.to_owned()).or_default();
        if let Some(create) = message.get("createSurface") {
            assert_eq!(create["catalogId"], CATALOG);
            *shown = Surface::default();
            for component in create["components"].as_array().unwrap() {
                shown.components.insert(component["id"].as_str().unwrap().to_owned(), component.clone());
            }
            shown.data = create["dataModel"].clone();
        } else if let Some(update) = message.get("updateComponents") {
            for component in update["components"].as_array().unwrap() {
                shown.components.insert(component["id"].as_str().unwrap().to_owned(), component.clone());
            }
        } else if let Some(update) = message.get("updateDataModel") {
            assert_eq!(update["path"], "/rows");
            shown.data["rows"] = update["value"].clone();
        } else {
            panic!("unexpected {message}");
        }
        Ok(())
    }

    fn data_sources(&mut self) -> Vec<DataSource> {
        vec![
            DataSource { id: ACTIVITY.into(), label: "Activity".into(), granted: self.activity },
            DataSource { id: SCHEDULE.into(), label: "Schedule".into(), granted: self.schedule },
        ]
    }

    fn data_query(&mut self, source: &str, sql: &str, params: &Value) -> Result<Value, DataError> {
        self.queries.push((source.to_owned(), sql.to_owned()));
        let granted = match source {
            ACTIVITY => self.activity,
            SCHEDULE => self.schedule,
            _ => return Err(DataError::UnknownSource),
        };
        if !granted {
            return Err(DataError::NotGranted);
        }
        if source == ACTIVITY {
            for only_schedule in ["monitor_specs", "FROM agents", "retry_at", "reask_at", "text", "name,", "spec"] {
                if sql.contains(only_schedule) {
                    return Err(DataError::Invalid(format!("Activity doesn't read {only_schedule}")));
                }
            }
        }
        if sql == time::ZONE_SQL {
            let (from, to) = (params[0].as_i64().unwrap(), params[1].as_i64().unwrap());
            let rows: Vec<Value> =
                (0..).map(|hour| from + hour * 3600).take_while(|t| *t <= to).map(|t| json!([t, self.offset(t * 1000)])).collect();
            return Ok(json!({"columns": ["t", "offset"], "rows": rows, "more": false}));
        }
        let mut statement = self.db.prepare(sql).map_err(|error| DataError::Failed(error.to_string()))?;
        let width = statement.column_count();
        let values: Vec<rusqlite::types::Value> = params
            .as_array()
            .unwrap()
            .iter()
            .map(|value| match value {
                Value::Null => rusqlite::types::Value::Null,
                Value::Number(n) => rusqlite::types::Value::Integer(n.as_i64().unwrap()),
                Value::String(s) => rusqlite::types::Value::Text(s.clone()),
                other => panic!("a param is a scalar: {other}"),
            })
            .collect();
        let mut rows = statement.query(rusqlite::params_from_iter(values)).map_err(|error| DataError::Failed(error.to_string()))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            out.push(Value::Array(
                (0..width)
                    .map(|index| match row.get_ref(index).unwrap() {
                        rusqlite::types::ValueRef::Null => Value::Null,
                        rusqlite::types::ValueRef::Integer(n) => json!(n),
                        rusqlite::types::ValueRef::Real(n) => json!(n),
                        rusqlite::types::ValueRef::Text(text) => json!(String::from_utf8_lossy(text)),
                        rusqlite::types::ValueRef::Blob(_) => json!("blob"),
                    })
                    .collect(),
            ));
        }
        Ok(json!({"columns": [], "rows": out, "more": false}))
    }

    fn now_ms(&mut self) -> i64 {
        self.now
    }

    fn log(&mut self, _level: Level, _message: &str) {}

    fn caller_info(&mut self) -> Option<Caller> {
        self.caller.clone()
    }

    fn emit_to(&mut self, agent: &str, kind: &str, text: &str, _scope: &str) {
        self.told.push((agent.to_owned(), kind.to_owned(), text.to_owned()));
    }
}

/// The rows a surface shows, each as {column: value}.
fn rows(stand: &Stand, surface: &str) -> Vec<BTreeMap<String, Value>> {
    let data = &stand.surfaces[surface].data["rows"];
    let columns: Vec<String> = data["columns"].as_array().unwrap().iter().map(|c| c.as_str().unwrap().to_owned()).collect();
    data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| columns.iter().cloned().zip(row.as_array().unwrap().iter().cloned()).collect())
        .collect()
}

fn titled<'a>(rows: &'a [BTreeMap<String, Value>], title: &str) -> Vec<&'a BTreeMap<String, Value>> {
    rows.iter().filter(|row| row["title"] == title).collect()
}

fn open(app: &mut App, stand: &mut Stand, surface: &str, device: &str) {
    app.view_open(stand, "main", &json!({"device": device}).to_string(), surface).unwrap();
}

fn act(app: &mut App, stand: &mut Stand, surface: &str, name: &str, context: Value) {
    app.view_action(stand, surface, &json!({"version": "1.0", "action": {"name": name, "context": context}}).to_string());
}

fn call(app: &mut App, stand: &mut Stand, tool: &str, args: Value) -> Output {
    app.call(stand, tool, &args.to_string(), stand.caller.as_ref().map_or("fugo", |caller| caller.id.as_str()).to_owned().as_str())
}

/// The UTC instant the stand's Mac clock reads `hours`:`minutes` on a day.
fn mac(stand: &Stand, days_ms: i64, hours: i64, minutes: i64) -> i64 {
    let local = days_ms + hours * HOUR + minutes * MINUTE;
    let guess = local - i64::from(stand.offset(local)) * MINUTE;
    local - i64::from(stand.offset(guess)) * MINUTE
}

#[test]
fn until_a_source_is_granted_it_says_so_then_shows_the_calendar_within_moments() {
    let mut stand = Stand::new();
    stand.activity = false;
    stand.schedule = false;
    let mut app = App::default();
    open(&mut app, &mut stand, "s1", "d1");
    let shown = &stand.surfaces["s1"];
    assert_eq!(shown.components["ask"]["title"], "Calendar can't see anything yet");
    assert!(!shown.components.contains_key("cal"));
    assert_eq!(stand.timers[REFRESH], ASK_AGAIN_MS);
    assert!(stand.queries.is_empty(), "it reads nothing before it's granted");

    stand.activity = true;
    app.on_timer(&mut stand, REFRESH);
    let shown = &stand.surfaces["s1"];
    assert_eq!(shown.components["root"]["children"], json!(["cal"]));
    let cal = &shown.components["cal"];
    assert_eq!(cal["component"], "Calendar");
    assert_eq!(cal["data"], json!({"@path": "/rows"}));
    assert_eq!(cal["view"], "week");
    assert_eq!(cal["anchor"], now());
    assert_eq!(cal["macZone"], json!({"start": 60, "end": 60}));
    assert_eq!(cal["tools"], json!(["agent_menu", "show_menu", "more_menu"]));
    for tool in ["agent_menu", "show_menu", "more_menu"] {
        assert_eq!(shown.components[tool]["component"], "Button");
    }
    assert_eq!(stand.timers[REFRESH], REFRESH_MS);
}

#[test]
fn monitors_on_activity_alone_show_their_next_fire_untitled_and_with_schedule_every_fire_by_name() {
    let mut stand = Stand::new();
    let next = now() + 5 * HOUR;
    stand.sql(&format!(
        "INSERT INTO monitors VALUES ('fugo', 3, 'Production logs', 'p', 'every', '{{\"kind\":\"every\",\"every\":21600000}}', NULL, NULL, 0, 'active', {next}, NULL, 4, 0);"
    ));
    stand.schedule = false;
    let mut app = App::default();
    open(&mut app, &mut stand, "s1", "d1");
    let shown = rows(&stand, "s1");
    let only = titled(&shown, "Fugo's monitor");
    assert_eq!(only.len(), 1, "{shown:?}");
    assert_eq!(only[0]["start"], next);
    assert_eq!(only[0]["open"], "monitor:fugo:m3");
    assert_eq!(only[0]["agent"], "fugo");
    assert_eq!(only[0]["color"], "agent:fugo");
    assert_eq!(only[0]["kind"], "monitor");

    stand.schedule = true;
    app.on_timer(&mut stand, REFRESH);
    let shown = rows(&stand, "s1");
    let fires = titled(&shown, "Production logs");
    // Every 6 hours from its next fire to the end of the 8 days after now.
    let expected: Vec<i64> = (0..).map(|n| next + n * 6 * HOUR).take_while(|at| *at < now() + 8 * DAY).collect();
    assert_eq!(fires.iter().map(|row| row["start"].as_i64().unwrap()).collect::<Vec<_>>(), expected);
    assert!(fires.iter().all(|row| row["repeat"] == "every 6 hours" && row["allDay"] == false));
    assert!(titled(&shown, "Fugo's monitor").is_empty());
}

#[test]
fn a_monitor_that_fires_more_than_four_times_a_day_or_waits_for_something_is_one_band_and_a_check_is_never_read() {
    let mut stand = Stand::new();
    let created = now() - 2 * DAY;
    let until = day(2026, 12, 6) + 20 * HOUR;
    stand.sql(&format!(
        "INSERT INTO monitors VALUES ('fugo', 1, 'Logs', 'p', 'every', '{{\"kind\":\"every\",\"every\":300000}}', NULL, NULL, {created}, 'active', {}, NULL, 9, 0),
                                    ('fugo', 2, 'Deploy check', 'p', 'check', '{{\"kind\":\"check\",\"command\":\"curl secret\"}}', {until}, NULL, {created}, 'active', {}, NULL, 0, 0),
                                    ('lucy', 4, 'Quarter hours', 'p', 'cron', '{{\"kind\":\"cron\",\"cron\":\"*/15 9-17 * * *\"}}', NULL, NULL, {created}, 'active', {}, NULL, 0, 0);
         INSERT INTO monitor_events (agent, monitor, time, actor, kind, value) VALUES ('fugo', 1, {}, '', 'fired', '');",
        now() + MINUTE,
        now() + MINUTE,
        now() + HOUR,
        now() - HOUR,
    ));
    let mut app = App::default();
    open(&mut app, &mut stand, "s1", "d1");
    let shown = rows(&stand, "s1");
    let logs = titled(&shown, "Logs");
    assert_eq!(logs.len(), 1, "one band, and its fires go in it: {logs:?}");
    assert_eq!(logs[0]["allDay"], true);
    assert_eq!(logs[0]["repeat"], "every 5 min");
    // From the day it was made to the last day shown, as dates.
    assert_eq!(logs[0]["start"], day(2026, 12, 1));
    assert_eq!(logs[0]["end"], day(2026, 12, 12));
    assert_eq!(logs[0]["repeat"], "every 5 min", "no end of its own, so no until");
    let check = titled(&shown, "Deploy check");
    assert_eq!(check.len(), 1);
    assert_eq!((&check[0]["allDay"], &check[0]["repeat"]), (&json!(true), &json!("runs a check · until 6 Dec")));
    assert_eq!(check[0]["end"], day(2026, 12, 7), "through its last day");
    let quarters = titled(&shown, "Quarter hours");
    assert_eq!(quarters.len(), 1);
    assert_eq!(quarters[0]["repeat"], "on a schedule · up to 36 times a day");
    assert!(
        stand.queries.iter().all(|(_, sql)| !sql.contains("m.spec") && !sql.contains("monitors.spec")),
        "specs come only from monitor_specs, which holds no check"
    );
}

#[test]
fn cron_fires_at_the_macs_wall_clock_across_daylight_saving() {
    let mut stand = Stand::new();
    // Weekdays at 9:00, around the change to summer time on 28 March 2027.
    stand.now = day(2027, 3, 25) + 12 * HOUR;
    stand.sql(&format!(
        "INSERT INTO monitors VALUES ('lucy', 7, 'Standup', 'p', 'cron', '{{\"kind\":\"cron\",\"cron\":\"0 9 * * 1-5\"}}', NULL, NULL, 0, 'active', {}, NULL, 0, 0);",
        mac(&stand, day(2027, 3, 26), 9, 0)
    ));
    let mut app = App::default();
    open(&mut app, &mut stand, "s1", "d1");
    let shown = rows(&stand, "s1");
    let starts: Vec<i64> = titled(&shown, "Standup").iter().map(|row| row["start"].as_i64().unwrap()).collect();
    assert_eq!(
        starts,
        vec![
            day(2027, 3, 26) + 8 * HOUR, // Friday, 9:00 at +1
            day(2027, 3, 29) + 7 * HOUR, // Monday, 9:00 at +2
            day(2027, 3, 30) + 7 * HOUR,
            day(2027, 3, 31) + 7 * HOUR,
            day(2027, 4, 1) + 7 * HOUR,
            day(2027, 4, 2) + 7 * HOUR,
        ]
    );
    assert_eq!(titled(&shown, "Standup")[0]["repeat"], "weekdays");
    assert_eq!(stand.surfaces["s1"].components["cal"]["macZone"], json!({"start": 60, "end": 120}));
}

#[test]
fn cron_reads_days_as_claktorys_monitors_do() {
    let utc = Zone::fixed(0);
    let either = Cron::parse("0 0 1 * sun").unwrap();
    let fires = either.fires(&utc, day(2026, 11, 1), day(2026, 12, 2), 100);
    // The 1st of each month, and every Sunday.
    assert_eq!(fires, vec![day(2026, 11, 1), day(2026, 11, 8), day(2026, 11, 15), day(2026, 11, 22), day(2026, 11, 29), day(2026, 12, 1)]);
    let leap = Cron::parse("0 12 29 feb *").unwrap();
    assert_eq!(leap.fires(&utc, day(2027, 1, 1), day(2029, 1, 1), 10), vec![day(2028, 2, 29) + 12 * HOUR]);
    assert!(Cron::parse("0 9 * *").is_none());
    assert!(Cron::parse("61 * * * *").is_none());
    assert_eq!(Cron::parse("30 8 * * mon").unwrap().words(), "every Monday");
    assert_eq!(Cron::parse("0 * * * *").unwrap().words(), "every hour");
}

#[test]
fn past_fires_are_pills_and_a_day_of_many_is_one_band() {
    let mut stand = Stand::new();
    stand.sql("INSERT INTO monitors VALUES ('fugo', 5, 'Sweep', 'p', 'at', '{\"kind\":\"at\",\"at\":1}', NULL, NULL, 0, 'done', NULL, NULL, 9, 1796000000000);");
    let yesterday = now() - DAY;
    for n in 0..6 {
        stand.sql(&format!(
            "INSERT INTO monitor_events (agent, monitor, time, actor, kind, value) VALUES ('fugo', 5, {}, '', 'fired', '');",
            yesterday + n * MINUTE
        ));
    }
    stand.sql(&format!("INSERT INTO monitor_events (agent, monitor, time, actor, kind, value) VALUES ('fugo', 5, {}, '', 'fired', ''), ('fugo', 5, {}, '', 'paused', '');", now() - HOUR, now() - MINUTE));
    let mut app = App::default();
    open(&mut app, &mut stand, "s1", "d1");
    let shown = rows(&stand, "s1");
    let sweep = titled(&shown, "Sweep");
    assert_eq!(sweep.len(), 2, "{sweep:?}");
    assert_eq!((&sweep[0]["allDay"], &sweep[0]["repeat"], &sweep[0]["kind"]), (&json!(true), &json!("fired 6 times"), &json!("fire")));
    assert_eq!(sweep[0]["start"], day(2026, 12, 2));
    assert_eq!((&sweep[1]["start"], &sweep[1]["kind"]), (&json!(now() - HOUR), &json!("fire")));
}

#[test]
fn checklist_dates_show_by_the_items_text_and_only_while_they_still_wait() {
    let mut stand = Stand::new();
    stand.sql(&format!(
        "INSERT INTO checklist_items VALUES ('lucy', 12, 'Check the shots\nwith details', 'blocked', '', 0, 0, {a}, NULL),
                                            ('lucy', 13, 'Resume after release', 'parked', '', 0, 0, {b}, NULL),
                                            ('lucy', 14, 'Already done', 'done', '', 0, 0, {a}, NULL),
                                            ('lucy', 15, 'Ask Matt again', 'open', '', 0, 0, NULL, {b});",
        a = now() + 2 * HOUR,
        b = now() + DAY
    ));
    let mut app = App::default();
    open(&mut app, &mut stand, "s1", "d1");
    let shown = rows(&stand, "s1");
    let items: Vec<_> = shown.iter().filter(|row| row["kind"] == "item").collect();
    assert_eq!(
        items.iter().map(|row| row["title"].as_str().unwrap()).collect::<Vec<_>>(),
        ["Check the shots", "Resume after release", "Ask Matt again"]
    );
    assert_eq!(items[0]["open"], "item:lucy:12");
    assert_eq!(items[0]["detail"], "From Lucy's checklist, waiting on someone. Lucy checks it again then.");
    assert_eq!(items[2]["detail"], "From Lucy's checklist. Claktory asks you about it again then.");

    stand.schedule = false;
    app.on_timer(&mut stand, REFRESH);
    assert!(rows(&stand, "s1").iter().all(|row| row["kind"] != "item"), "Activity has no dates for items");
}

#[test]
fn long_tasks_and_runs_are_blocks_and_running_ones_are_live() {
    let mut stand = Stand::new();
    stand.sql(&format!(
        "INSERT INTO tasks (agent, task, command, started, ended, status) VALUES
             ('giorno', 'task-1', 'cargo test', {}, {}, 'exit 0'),
             ('giorno', 'task-2', 'quick', {}, {}, 'exit 0'),
             ('giorno', 'task-3', 'serve', {}, NULL, NULL);
         INSERT INTO runs (agent, version, started, ended) VALUES ('giorno', 'v', {}, NULL), ('giorno', 'v', {}, {});",
        now() - 3 * HOUR,
        now() - 2 * HOUR,
        now() - HOUR,
        now() - HOUR + MINUTE,
        now() - 10 * MINUTE,
        now() - 70 * MINUTE,
        now() - 5 * HOUR,
        now() - 5 * HOUR + 10 * MINUTE,
    ));
    let mut app = App::default();
    open(&mut app, &mut stand, "s1", "d1");
    let shown = rows(&stand, "s1");
    let tasks: Vec<_> = shown.iter().filter(|row| row["kind"] == "task").collect();
    assert_eq!(tasks.iter().map(|row| row["title"].as_str().unwrap()).collect::<Vec<_>>(), ["task-1", "task-3"]);
    assert_eq!((&tasks[0]["end"], &tasks[0]["live"]), (&json!(now() - 2 * HOUR), &json!(false)));
    assert_eq!((&tasks[1]["end"], &tasks[1]["live"], &tasks[1]["open"]), (&Value::Null, &json!(true), &json!("task:giorno:task-3")));
    assert_eq!(tasks[0]["detail"], "From a task Giorno started. It ended: exit 0.");
    assert!(stand.queries.iter().all(|(_, sql)| !sql.contains("command")), "a task's command is never read");
    let runs: Vec<_> = shown.iter().filter(|row| row["kind"] == "run").collect();
    assert_eq!(runs.len(), 1, "only the run over 30 minutes");
    assert_eq!((&runs[0]["live"], &runs[0]["open"]), (&json!(true), &json!("agent:giorno")));
    assert_eq!(runs[0]["detail"], "Giorno has been working for 1 h 10 min.");
}

#[test]
fn an_agent_places_an_event_and_every_open_view_shows_it() {
    let mut stand = Stand::new();
    let mut app = App::default();
    open(&mut app, &mut stand, "s1", "d1");
    stand.as_agent("fugo", Some("claktory"));
    let put = call(
        &mut app,
        &mut stand,
        "calendar_put",
        json!({"event": {"title": "v0.1.26 release", "start": "2026-12-04 14:00", "end": "2026-12-04 15:30", "note": "Signed and notarized."}}),
    );
    assert!(!put.is_error, "{}", put.text);
    assert_eq!(put.text, "Placed e1 (_rev 1): \"v0.1.26 release\", 2026-12-04 14:00+01:00 – 2026-12-04 15:30+01:00.");
    assert_eq!(stand.timers[REFRESH], 1, "views look again right after");
    app.on_timer(&mut stand, REFRESH);
    let shown = rows(&stand, "s1");
    let release = titled(&shown, "v0.1.26 release");
    assert_eq!(release.len(), 1);
    assert_eq!(release[0]["start"], day(2026, 12, 4) + 13 * HOUR);
    assert_eq!(release[0]["end"], day(2026, 12, 4) + 14 * HOUR + 30 * MINUTE);
    assert_eq!(release[0]["kind"], "event");
    assert_eq!(release[0]["open"], "agent:fugo");
    assert_eq!(release[0]["detail"], "Signed and notarized. Added by Fugo.");

    // An all-day deadline is its date on every device: UTC midnights, end exclusive.
    let due = call(
        &mut app,
        &mut stand,
        "calendar_put",
        json!({"event": {"title": "Store review", "start": "2026-12-07", "end": "2026-12-08", "deadline": true}}),
    );
    assert!(!due.is_error, "{}", due.text);
    app.on_timer(&mut stand, REFRESH);
    let shown = rows(&stand, "s1");
    let review = titled(&shown, "Store review");
    assert_eq!((&review[0]["start"], &review[0]["end"]), (&json!(day(2026, 12, 7)), &json!(day(2026, 12, 9))));
    assert_eq!((&review[0]["allDay"], &review[0]["kind"]), (&json!(true), &json!("deadline")));
    assert_eq!(shown[0]["title"], "Store review", "all-day entries come first");
}

#[test]
fn a_repeating_event_keeps_its_time_on_the_macs_clock_and_stops_where_told() {
    let mut stand = Stand::new();
    stand.now = day(2027, 3, 24) + 12 * HOUR;
    let mut app = App::default();
    stand.as_agent("lucy", Some("claktory"));
    let put = call(
        &mut app,
        &mut stand,
        "calendar_put",
        json!({"event": {"title": "QA pass", "start": "2027-03-24 10:00", "end": "2027-03-24 11:00", "repeat": {"every": "week", "on": ["mo", "we"], "count": 3}}}),
    );
    assert!(!put.is_error, "{}", put.text);
    open(&mut app, &mut stand, "s1", "d1");
    let shown = rows(&stand, "s1");
    let passes: Vec<(i64, i64)> =
        titled(&shown, "QA pass").iter().map(|row| (row["start"].as_i64().unwrap(), row["end"].as_i64().unwrap())).collect();
    assert_eq!(
        passes,
        vec![
            (day(2027, 3, 24) + 9 * HOUR, day(2027, 3, 24) + 10 * HOUR),
            (day(2027, 3, 29) + 8 * HOUR, day(2027, 3, 29) + 9 * HOUR),
            (day(2027, 3, 31) + 8 * HOUR, day(2027, 3, 31) + 9 * HOUR),
        ]
    );
    assert_eq!(titled(&shown, "QA pass")[0]["repeat"], "every week on Monday, Wednesday");
}

#[test]
fn changes_need_the_rev_read_keep_the_version_before_and_derived_entries_say_where_to_change_them() {
    let mut stand = Stand::new();
    let mut app = App::default();
    stand.as_agent("fugo", Some("claktory"));
    call(
        &mut app,
        &mut stand,
        "calendar_put",
        json!({"event": {"title": "Launch", "start": "2026-12-10 09:00", "end": "2026-12-10 10:00"}}),
    );
    let stale = call(
        &mut app,
        &mut stand,
        "calendar_put",
        json!({"event": {"id": "e1", "title": "Launch!", "start": "2026-12-10 09:00", "_rev": 7}}),
    );
    assert!(stale.is_error);
    assert!(stale.text.contains("changed since you read it"), "{}", stale.text);
    let changed = call(
        &mut app,
        &mut stand,
        "calendar_put",
        json!({"event": {"id": "e1", "title": "Launch!", "start": "2026-12-10 09:00", "_rev": 1}}),
    );
    assert!(!changed.is_error, "{}", changed.text);
    let previous = call(&mut app, &mut stand, "calendar_get", json!({"id": "e1", "previous": true}));
    assert_eq!(previous.data.unwrap()["title"], "Launch");

    // Moving keeps its length.
    let moved = call(
        &mut app,
        &mut stand,
        "calendar_put",
        json!({"event": {"id": "e1", "title": "Launch", "start": "2026-12-10 09:00", "end": "2026-12-10 10:00", "_rev": 2}}),
    );
    assert!(!moved.is_error, "{}", moved.text);
    let moved = call(&mut app, &mut stand, "calendar_move", json!({"id": "e1", "start": "2026-12-11 15:00"}));
    assert_eq!(moved.text, "Moved e1 (_rev 4): \"Launch\", 2026-12-11 15:00+01:00 – 2026-12-11 16:00+01:00.");

    let deleted = call(&mut app, &mut stand, "calendar_delete", json!({"id": "e1"}));
    assert!(!deleted.is_error);
    assert!(call(&mut app, &mut stand, "calendar_get", json!({"id": "e1"})).is_error);
    let kept = call(&mut app, &mut stand, "calendar_get", json!({"id": "e1", "previous": true}));
    assert!(kept.text.starts_with("Deleted;"), "{}", kept.text);
    let back = call(&mut app, &mut stand, "calendar_put", json!({"event": {"id": "e1", "title": "Launch", "start": "2026-12-11 15:00"}}));
    assert!(!back.is_error, "{}", back.text);

    let derived = call(&mut app, &mut stand, "calendar_move", json!({"id": "mon:lucy:m3:1796000000000", "start": "2026-12-11 15:00"}));
    assert!(derived.is_error);
    assert_eq!(derived.text, "This comes from Lucy's monitor m3. Change it there, with monitors_change or by asking Lucy.");
}

#[test]
fn an_agent_sees_its_own_projects_events_an_admin_all_and_an_outside_one_is_marked() {
    let mut stand = Stand::new();
    let mut app = App::default();
    stand.as_agent("kenji", Some("web-site"));
    call(&mut app, &mut stand, "calendar_put", json!({"event": {"title": "Site launch", "start": "2026-12-05"}}));
    stand.caller = Some(Caller { id: "fugo".into(), project: Some("claktory".into()), admin: false, tainted: true });
    call(&mut app, &mut stand, "calendar_put", json!({"event": {"title": "Read on a forum", "start": "2026-12-05"}}));
    stand.sql(&format!(
        "INSERT INTO tasks (agent, task, command, started, ended, status) VALUES ('kenji', 'task-9', 'x', {}, NULL, NULL);",
        now() - HOUR
    ));

    let listed = call(&mut app, &mut stand, "calendar_list", json!({}));
    assert!(listed.text.contains("Read on a forum · from outside [e2]"), "{}", listed.text);
    assert!(!listed.text.contains("Site launch") && !listed.text.contains("task-9"), "another project's: {}", listed.text);
    assert!(call(&mut app, &mut stand, "calendar_get", json!({"id": "e1"})).is_error);
    assert!(call(&mut app, &mut stand, "calendar_delete", json!({"id": "e1"})).is_error);

    stand.caller = Some(Caller { id: "rivet".into(), project: None, admin: true, tainted: false });
    let all = call(&mut app, &mut stand, "calendar_list", json!({}));
    assert!(all.text.contains("Site launch") && all.text.contains("Read on a forum") && all.text.contains("task-9"), "{}", all.text);

    // The person sees everything.
    open(&mut app, &mut stand, "s1", "d1");
    let shown = rows(&stand, "s1");
    assert_eq!(titled(&shown, "Read on a forum")[0]["outside"], true);
    assert_eq!(titled(&shown, "Read on a forum")[0]["detail"], "Added by Fugo. Fugo added this after reading a website or a pipe, so it may come from someone else.");
    assert_eq!(titled(&shown, "Site launch")[0]["outside"], false);
}

#[test]
fn moving_an_event_its_maker_linked_to_a_monitor_tells_the_maker() {
    let mut stand = Stand::new();
    stand.sql(&format!("INSERT INTO monitors VALUES ('fugo', 3, 'Release day', 'p', 'at', '{{\"kind\":\"at\",\"at\":1}}', NULL, NULL, 0, 'active', {}, NULL, 0, 0);", now() + DAY));
    let mut app = App::default();
    stand.as_agent("fugo", Some("claktory"));
    let put = call(
        &mut app,
        &mut stand,
        "calendar_put",
        json!({"event": {"title": "Release", "start": "2026-12-04 10:00", "link": {"agent": "fugo", "monitor": "m3"}}}),
    );
    assert!(!put.is_error, "{}", put.text);
    let missing = call(
        &mut app,
        &mut stand,
        "calendar_put",
        json!({"event": {"title": "X", "start": "2026-12-04 10:00", "link": {"agent": "fugo", "monitor": 9}}}),
    );
    assert_eq!(missing.text, "fugo has no monitor m9.");
    // Never an id a device couldn't open (it would refuse the whole view).
    for link in [json!({"agent": "fu go"}), json!({"agent": "fugo", "task": "task:3"}), json!({"agent": "x".repeat(65)})] {
        let bad = call(&mut app, &mut stand, "calendar_put", json!({"event": {"title": "X", "start": "2026-12-04 10:00", "link": link}}));
        assert!(bad.is_error, "{link}: {}", bad.text);
    }

    // Its maker moving it tells no one.
    call(&mut app, &mut stand, "calendar_move", json!({"id": "e1", "start": "2026-12-04 11:00"}));
    assert!(stand.told.is_empty());
    stand.as_agent("lucy", Some("claktory"));
    let moved = call(&mut app, &mut stand, "calendar_move", json!({"id": "e1", "start": "2026-12-05 11:00"}));
    assert!(
        moved.text.ends_with("Fugo linked it to their monitor m3, so Calendar told them; if they don't hear Calendar, tell them yourself."),
        "{}",
        moved.text
    );
    assert_eq!(stand.told.len(), 1);
    let (to, kind, text) = &stand.told[0];
    assert_eq!((to.as_str(), kind.as_str()), ("fugo", "moved"));
    assert_eq!(
        text,
        "Calendar: Lucy moved \"Release\" (e1), which you linked to your monitor m3. Change the monitor if it should fire at another time, or cancel it."
    );
    call(&mut app, &mut stand, "calendar_delete", json!({"id": "e1"}));
    assert!(stand.told[1].2.starts_with("Calendar: Lucy deleted \"Release\""));

    open(&mut app, &mut stand, "s1", "d1");
    assert!(titled(&rows(&stand, "s1"), "Release").is_empty());
}

#[test]
fn each_device_comes_back_where_it_was_and_the_arrangement_is_everyones() {
    let mut stand = Stand::new();
    let mut app = App::default();
    open(&mut app, &mut stand, "phone", "d-phone");
    open(&mut app, &mut stand, "mac", "d-mac");
    let later = now() + 20 * DAY;
    act(&mut app, &mut stand, "phone", "calendar", json!({"anchor": later, "view": "agenda"}));
    assert_eq!(stand.surfaces["phone"].components["cal"]["anchor"], later);
    assert_eq!(stand.surfaces["phone"].components["cal"]["view"], "agenda");
    assert_eq!(stand.surfaces["phone"].components["cal"]["emptyTitle"], "Nothing on the calendar from 23 Dec on");
    assert_eq!(stand.surfaces["mac"].components["cal"]["view"], "week");
    act(&mut app, &mut stand, "phone", "today", json!({}));
    assert_eq!(stand.surfaces["phone"].components["cal"]["anchor"], now());

    // Back within the hour, the phone is where it was: the day, the view
    // and the agent; after that, today, with its view and agent still.
    act(&mut app, &mut stand, "phone", "calendar", json!({"anchor": later, "view": "agenda"}));
    act(&mut app, &mut stand, "phone", "agent", json!({"id": "lucy"}));
    app.view_closed(&mut stand, "phone");
    stand.now += 50 * MINUTE;
    open(&mut app, &mut stand, "phone2", "d-phone");
    let cal = &stand.surfaces["phone2"].components["cal"];
    assert_eq!((&cal["anchor"], &cal["view"]), (&json!(later), &json!("agenda")));
    assert_eq!(stand.surfaces["phone2"].components["agent_menu_t"]["text"], "Agent: Lucy");
    app.view_closed(&mut stand, "phone2");
    stand.now += 61 * MINUTE;
    open(&mut app, &mut stand, "phone3", "d-phone");
    let cal = &stand.surfaces["phone3"].components["cal"];
    assert_eq!((&cal["anchor"], &cal["view"]), (&json!(stand.now), &json!("agenda")));
    assert_eq!(stand.surfaces["phone3"].components["agent_menu_t"]["text"], "Agent: Lucy");
    act(&mut app, &mut stand, "phone3", "agent", json!({"id": ""}));
    stand.now = now();
    open(&mut app, &mut stand, "ipad", "d-ipad");
    assert_eq!(stand.surfaces["ipad"].components["cal"]["view"], "week");

    // Show and the week's first day are the same everywhere.
    stand.as_agent("fugo", Some("claktory"));
    call(&mut app, &mut stand, "calendar_put", json!({"event": {"title": "Launch", "start": "2026-12-04 09:00"}}));
    app.on_timer(&mut stand, REFRESH);
    act(&mut app, &mut stand, "mac", "layer", json!({"layer": "events", "on": false}));
    act(&mut app, &mut stand, "mac", "week_start", json!({"day": "sunday"}));
    for surface in ["mac", "ipad"] {
        assert!(titled(&rows(&stand, surface), "Launch").is_empty(), "{surface}");
        assert_eq!(stand.surfaces[surface].components["cal"]["weekStart"], "sunday");
        let show = &stand.surfaces[surface].components["show_menu"];
        assert_eq!(stand.surfaces[surface].components["show_menu_t"]["text"], "Show: Monitors, Checklists, Tasks and runs");
        let events = &show["menu"]["options"][3];
        assert_eq!((&events["label"], events.get("selected")), (&json!("Events"), None));
        // The same in ⋯, for a narrow view without Show ▾.
        let more = &stand.surfaces[surface].components["more_menu"]["menu"]["options"];
        assert_eq!((&more[4]["label"], &more[4]["section"], more[4].get("selected")), (&json!("Events"), &json!("Show"), None));
        assert_eq!((&more[1]["label"], &more[1]["selected"]), (&json!("Monitors"), &json!(true)));
    }
}

#[test]
fn the_agent_menu_lists_agents_by_project_and_filters_the_view() {
    let mut stand = Stand::new();
    stand.sql(&format!(
        "INSERT INTO tasks (agent, task, command, started, ended, status) VALUES ('fugo', 'task-1', 'x', {t}, NULL, NULL), ('kenji', 'task-2', 'x', {t}, NULL, NULL);",
        t = now() - HOUR
    ));
    let listed = |stand: &mut Stand| {
        let mut app = App::default();
        open(&mut app, stand, "s0", "d0");
        let options = stand.surfaces["s0"].components["agent_menu"]["menu"]["options"].as_array().unwrap().clone();
        options.iter().map(|o| (o["label"].as_str().unwrap().to_owned(), o["section"].as_str().unwrap_or("").to_owned())).collect::<Vec<_>>()
    };
    let menu = |sections: [&str; 3]| -> Vec<(String, String)> {
        [("All agents", ""), ("Fugo", sections[0]), ("Giorno", sections[0]), ("Lucy", sections[0]), ("Kenji", sections[1]), ("Rivet", sections[2])]
            .map(|(label, section)| (label.to_owned(), section.to_owned()))
            .to_vec()
    };
    assert_eq!(listed(&mut stand), menu(["Claktory", "Web Site", "Workspace"]));
    // A backend without project names: their ids.
    stand.sql("ALTER TABLE agents DROP COLUMN project_name;");
    assert_eq!(listed(&mut stand), menu(["claktory", "web-site", "Workspace"]));

    let mut app = App::default();
    open(&mut app, &mut stand, "s1", "d1");
    act(&mut app, &mut stand, "s1", "agent", json!({"id": "kenji"}));
    let shown = rows(&stand, "s1");
    assert_eq!(shown.iter().map(|row| row["title"].as_str().unwrap()).collect::<Vec<_>>(), ["task-2"]);
    assert_eq!(stand.surfaces["s1"].components["agent_menu_t"]["text"], "Agent: Kenji");
}

#[test]
fn times_are_read_on_the_macs_clock_unless_they_say_otherwise() {
    let zone = Zone::fixed(60);
    assert_eq!(time::parse("2026-12-03 14:00", 0), Ok(Written::Local(day(2026, 12, 3) + 14 * HOUR)));
    assert_eq!(zone.utc(day(2026, 12, 3) + 14 * HOUR), day(2026, 12, 3) + 13 * HOUR);
    assert_eq!(time::parse("2026-12-03T14:00:00Z", 0), Ok(Written::Instant(day(2026, 12, 3) + 14 * HOUR)));
    assert_eq!(time::parse("2026-12-03 14:00-05:30", 0), Ok(Written::Instant(day(2026, 12, 3) + 19 * HOUR + 30 * MINUTE)));
    assert_eq!(time::parse("2026-12-03", 0), Ok(Written::Date(day(2026, 12, 3) / DAY)));
    assert_eq!(time::parse("now+2d", 5), Ok(Written::Instant(5 + 2 * DAY)));
    assert!(time::parse("2026-02-30", 0).is_err());
    assert!(time::parse("next friday", 0).is_err());
    assert_eq!(zone.text(day(2026, 12, 3) + 13 * HOUR), "2026-12-03 14:00+01:00");

    // Without a source, a time with no offset is refused, saying how.
    let mut stand = Stand::new();
    stand.activity = false;
    stand.schedule = false;
    let mut app = App::default();
    let refused = call(&mut app, &mut stand, "calendar_put", json!({"event": {"title": "X", "start": "2026-12-04 10:00"}}));
    assert!(refused.is_error && refused.text.contains("write the time with its offset"), "{}", refused.text);
    let fine = call(&mut app, &mut stand, "calendar_put", json!({"event": {"title": "X", "start": "2026-12-04 10:00+01:00"}}));
    assert!(!fine.is_error, "{}", fine.text);
}

#[test]
fn events_long_past_are_dropped_and_the_rest_kept() {
    let mut stand = Stand::new();
    let mut app = App::default();
    stand.as_agent("fugo", Some("claktory"));
    call(&mut app, &mut stand, "calendar_put", json!({"event": {"title": "Old", "start": "2025-01-05"}}));
    call(
        &mut app,
        &mut stand,
        "calendar_put",
        json!({"event": {"title": "Weekly", "start": "2025-01-06 10:00", "repeat": {"every": "week"}}}),
    );
    call(&mut app, &mut stand, "calendar_put", json!({"event": {"title": "Recent", "start": "2026-06-01"}}));
    app.start(&mut stand);
    let left: Vec<String> = store::index(&mut stand).into_keys().collect();
    assert_eq!(left, ["e2", "e3"]);
    assert!(!stand.kv.contains_key("ev/e1") && !stand.kv.contains_key("prev/e1"));
    assert_eq!(stand.timers[PRUNE], PRUNE_MS);
}

#[test]
fn a_view_pushes_only_what_changed() {
    let mut stand = Stand::new();
    let mut app = App::default();
    open(&mut app, &mut stand, "s1", "d1");
    let pushed = stand.pushed;
    app.on_timer(&mut stand, REFRESH);
    assert_eq!(stand.pushed, pushed, "nothing changed, nothing pushed");
    stand.sql(&format!(
        "INSERT INTO tasks (agent, task, command, started, ended, status) VALUES ('fugo', 'task-1', 'x', {}, NULL, NULL);",
        now() - HOUR
    ));
    app.on_timer(&mut stand, REFRESH);
    assert_eq!(stand.pushed, pushed + 1, "the rows alone");
    assert_eq!(titled(&rows(&stand, "s1"), "task-1").len(), 1);
}

#[test]
fn the_macs_clock_maps_to_instants_around_a_change_of_offset() {
    let summer = day(2027, 3, 28) + HOUR;
    let winter = day(2027, 10, 31) + HOUR;
    let rome = Zone::from_rows(
        &[(summer - HOUR, 60), (summer, 120), (winter - HOUR, 120), (winter, 60)]
            .iter()
            .map(|(at, offset)| vec![json!(at / 1000), json!(offset)])
            .collect::<Vec<_>>(),
    );
    let local = |date: i64, h: i64, m: i64| date + h * HOUR + m * MINUTE;
    // Just before the spring change, 01:30 is still +1.
    assert_eq!(rome.utc(local(day(2027, 3, 28), 1, 30)), day(2027, 3, 28) + 30 * MINUTE);
    // 02:30 doesn't exist: as long after the gap, 03:30.
    assert_eq!(rome.utc(local(day(2027, 3, 28), 2, 30)), summer + 30 * MINUTE);
    assert_eq!(rome.utc(local(day(2027, 3, 28), 3, 30)), summer + 30 * MINUTE);
    // Autumn: 02:30 happens twice; the first one.
    assert_eq!(rome.utc(local(day(2027, 10, 31), 2, 30)), winter - 30 * MINUTE);
    for at in [summer - 3 * HOUR, summer + 3 * HOUR, winter - 3 * HOUR, winter + 3 * HOUR] {
        assert_eq!(rome.utc(rome.local(at)), at);
    }
    // A daily 1:30 cron keeps 1:30 on both sides of the change.
    let nightly = Cron::parse("30 1 * * *").unwrap();
    let fires = nightly.fires(&rome, day(2027, 3, 27), day(2027, 3, 30), 10);
    assert_eq!(
        fires,
        vec![
            day(2027, 3, 27) + 30 * MINUTE,
            day(2027, 3, 28) + 30 * MINUTE,
            day(2027, 3, 28) + 23 * HOUR + 30 * MINUTE,
            day(2027, 3, 29) + 23 * HOUR + 30 * MINUTE
        ]
    );
    // 2:30 and 3:30 on the night 2:30 is skipped: one fire, at 3:30.
    let both = Cron::parse("30 2,3 * * *").unwrap();
    assert_eq!(both.fires(&rome, day(2027, 3, 28), day(2027, 3, 28) + 12 * HOUR, 10), vec![summer + 30 * MINUTE]);
}

#[test]
fn a_monthly_event_skips_months_without_its_date() {
    let mut stand = Stand::new();
    let mut app = App::default();
    stand.as_agent("fugo", Some("claktory"));
    let put = call(
        &mut app,
        &mut stand,
        "calendar_put",
        json!({"event": {"title": "Invoices", "start": "2026-10-31", "repeat": {"every": "month"}}}),
    );
    assert!(!put.is_error, "{}", put.text);
    let listed = call(&mut app, &mut stand, "calendar_list", json!({"from": "2026-10-01", "to": "2026-12-31", "layers": ["events"]}));
    assert!(listed.is_error, "62 days at most");
    let listed = call(&mut app, &mut stand, "calendar_list", json!({"from": "2026-10-20", "to": "2026-12-20", "layers": ["events"]}));
    assert_eq!(listed.text, "2026-10-31, all day · Fugo · event · Invoices · every month [e1]");
    let listed = call(&mut app, &mut stand, "calendar_list", json!({"from": "2026-12-20", "to": "2027-02-01", "layers": ["events"]}));
    assert_eq!(
        listed.text,
        "2026-12-31, all day · Fugo · event · Invoices · every month [e1]\n2027-01-31, all day · Fugo · event · Invoices · every month [e1]"
    );
}

#[test]
fn the_empty_words_say_today_and_this_week_only_when_they_are() {
    let mut stand = Stand::new();
    let mut app = App::default();
    open(&mut app, &mut stand, "s1", "d1");
    let title = |stand: &Stand| stand.surfaces["s1"].components["cal"]["emptyTitle"].as_str().unwrap().to_owned();

    act(&mut app, &mut stand, "s1", "calendar", json!({"anchor": now(), "view": "day"}));
    assert_eq!(title(&stand), "Nothing on the calendar today");
    // 23:30 UTC on Thursday is already Friday on the Mac.
    act(&mut app, &mut stand, "s1", "calendar", json!({"anchor": day(2026, 12, 3) + 23 * HOUR + 30 * MINUTE, "view": "day"}));
    assert_eq!(title(&stand), "Nothing on the calendar on Friday 4 Dec");
    act(&mut app, &mut stand, "s1", "calendar", json!({"anchor": now(), "view": "agenda"}));
    assert_eq!(title(&stand), "Nothing on the calendar coming up");

    let sunday = day(2026, 11, 29) + 12 * HOUR;
    act(&mut app, &mut stand, "s1", "calendar", json!({"anchor": day(2026, 11, 30) + 12 * HOUR, "view": "week"}));
    assert_eq!(title(&stand), "Nothing on the calendar this week");
    act(&mut app, &mut stand, "s1", "calendar", json!({"anchor": sunday, "view": "week"}));
    assert_eq!(title(&stand), "Nothing on the calendar that week");
    // A phone draws Agenda whatever the view, from the same day.
    assert_eq!(stand.surfaces["s1"].components["cal"]["agendaEmptyTitle"], "Nothing on the calendar from 29 Nov on");
    act(&mut app, &mut stand, "s1", "week_start", json!({"day": "sunday"}));
    assert_eq!(title(&stand), "Nothing on the calendar this week");

    for layer in ["monitors", "checklists", "tasks", "events"] {
        act(&mut app, &mut stand, "s1", "layer", json!({"layer": layer, "on": false}));
    }
    assert_eq!(title(&stand), "Everything is hidden");
    act(&mut app, &mut stand, "s1", "layer", json!({"layer": "events", "on": true}));
    assert_eq!(title(&stand), "Nothing on the calendar this week");
}
