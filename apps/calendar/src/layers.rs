//! What's on the calendar in a stretch of time: the layers read live from
//! Claktory at every look (monitors and their fires, checklist items'
//! dates, background tasks, long runs) and the events agents placed.
//!
//! It reads Schedule when the person granted it (names, schedules, item
//! text and agents' projects), else Activity (times and states alone, so
//! monitors read "Fugo's monitor" and items, having no dates there, don't
//! show).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::cron::Cron;
use crate::store::{self, Calendar, Event};
use crate::time::{self, DAY, MINUTE, Zone};
use crate::{ACTIVITY, DataError, DataSource, Host, SCHEDULE};

/// A monitor firing more often than this a day is one band, not pills.
pub const MOST_A_DAY: usize = 4;
/// Runs shorter than this aren't drawn.
pub const LONG_RUN: i64 = 30 * MINUTE;
/// Background tasks shorter than this aren't drawn (builds and tests
/// would bury the rest).
pub const LONG_TASK: i64 = 5 * MINUTE;
/// Rows a view or a list gets at most.
pub const MOST_ROWS: usize = 3_000;

/// What the person turns on and off under Show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Layer {
    Monitors,
    Checklists,
    Tasks,
    Events,
}

impl Layer {
    pub const ALL: [Layer; 4] = [Layer::Monitors, Layer::Checklists, Layer::Tasks, Layer::Events];

    pub fn id(self) -> &'static str {
        match self {
            Layer::Monitors => "monitors",
            Layer::Checklists => "checklists",
            Layer::Tasks => "tasks",
            Layer::Events => "events",
        }
    }

    pub fn words(self) -> &'static str {
        match self {
            Layer::Monitors => "Monitors",
            Layer::Checklists => "Checklists",
            Layer::Tasks => "Tasks and runs",
            Layer::Events => "Events",
        }
    }

    pub fn of(id: &str) -> Option<Layer> {
        Layer::ALL.into_iter().find(|layer| layer.id() == id)
    }
}

/// One entry, as the Calendar component takes it.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: String,
    pub start: i64,
    pub end: Option<i64>,
    pub title: String,
    pub color: String,
    pub all_day: bool,
    /// monitor, fire, item, task, run, event or deadline.
    pub kind: &'static str,
    pub agent: String,
    pub repeat: String,
    pub detail: String,
    pub open: Option<String>,
    pub live: bool,
    pub outside: bool,
    pub layer: Layer,
}

pub const COLUMNS: [&str; 13] =
    ["id", "start", "end", "title", "color", "allDay", "kind", "agent", "repeat", "detail", "open", "live", "outside"];

impl Row {
    pub fn cells(&self) -> Value {
        json!([
            self.id,
            self.start,
            self.end,
            self.title,
            self.color,
            self.all_day,
            self.kind,
            self.agent,
            self.repeat,
            self.detail,
            self.open,
            self.live,
            self.outside
        ])
    }
}

/// The agents, as the source knows them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Agents {
    pub names: BTreeMap<String, String>,
    /// Each agent's project (None: the workspace), when Schedule says.
    pub projects: Option<BTreeMap<String, Option<String>>>,
    /// Each project's name by its id, from a backend that gives them.
    pub project_names: BTreeMap<String, String>,
}

impl Agents {
    pub fn name(&self, id: &str) -> String {
        self.names.get(id).cloned().unwrap_or_else(|| id.to_owned())
    }

    /// Whether `id` is an agent now; unknown without Schedule.
    pub fn exists(&self, id: &str) -> Option<bool> {
        self.projects.as_ref().map(|projects| projects.contains_key(id))
    }

    /// Whether `agent`'s entries are `caller`'s to see: its own project's,
    /// or all for an admin; all when projects aren't known.
    pub fn visible(&self, agent: &str, caller: Option<&crate::Caller>) -> bool {
        let (Some(caller), Some(projects)) = (caller, &self.projects) else { return true };
        caller.admin || projects.get(agent).is_some_and(|project| *project == caller.project)
    }
}

/// The source the layers read: Schedule if granted, else Activity, else
/// none.
pub fn source(sources: &[DataSource]) -> Option<&'static str> {
    [SCHEDULE, ACTIVITY].into_iter().find(|id| sources.iter().any(|source| source.id == *id && source.granted))
}

/// A query's rows.
pub fn rows(host: &mut impl Host, source: &str, sql: &str, params: Value) -> Result<Vec<Vec<Value>>, DataError> {
    let answer = host.data_query(source, sql, &params)?;
    Ok(answer["rows"].as_array().map(|rows| rows.iter().filter_map(|row| row.as_array().cloned()).collect()).unwrap_or_default())
}

/// The Mac's clock over `from`..`to`, or None when nothing's granted.
pub fn zone(host: &mut impl Host, source: Option<&str>, from: i64, to: i64) -> Option<Zone> {
    let (start, end) = Zone::bounds(from, to);
    let rows = rows(host, source?, time::ZONE_SQL, json!([start, end])).ok()?;
    Some(Zone::from_rows(&rows)).filter(Zone::is_known)
}

pub fn agents(host: &mut impl Host, source: &str) -> Result<Agents, DataError> {
    if source == SCHEDULE {
        // A backend before 0.1.28 has no project_name: the ids stand in.
        let rows = match rows(host, source, "SELECT id, name, project, project_name FROM agents", json!([])) {
            Ok(rows) => rows,
            Err(_) => rows(host, source, "SELECT id, name, project FROM agents", json!([]))?,
        };
        let mut agents = Agents { projects: Some(BTreeMap::new()), ..Agents::default() };
        for row in rows {
            let Some(id) = row.first().and_then(Value::as_str) else { continue };
            agents.names.insert(id.to_owned(), row.get(1).and_then(Value::as_str).unwrap_or(id).to_owned());
            let project = text(&row, 2);
            if let (Some(project), Some(name)) = (&project, text(&row, 3)) {
                agents.project_names.insert(project.clone(), name);
            }
            agents.projects.as_mut().expect("projects").insert(id.to_owned(), project);
        }
        return Ok(agents);
    }
    // Activity holds names only where agents used tokens: each one's name
    // at its latest hour.
    let rows = rows(
        host,
        source,
        "SELECT agent, agent_name FROM agent_usage a WHERE agent <> '' AND hour = (SELECT MAX(hour) FROM agent_usage b WHERE b.agent = a.agent)",
        json!([]),
    )?;
    let mut agents = Agents::default();
    for row in rows {
        if let (Some(id), Some(name)) = (row.first().and_then(Value::as_str), row.get(1).and_then(Value::as_str)) {
            agents.names.insert(id.to_owned(), name.to_owned());
        }
    }
    Ok(agents)
}

/// Everything a look at `from`..`to` needs from the source.
pub struct Look<'a> {
    pub source: &'a str,
    pub zone: &'a Zone,
    pub agents: &'a Agents,
    pub from: i64,
    pub to: i64,
    pub now: i64,
}

fn text(row: &[Value], at: usize) -> Option<String> {
    row.get(at).and_then(Value::as_str).map(str::to_owned)
}

fn int(row: &[Value], at: usize) -> Option<i64> {
    row.get(at).and_then(Value::as_i64)
}

/// The first line of what someone wrote, cut to fit a calendar entry.
pub fn title_of(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or_default();
    if line.chars().count() > 120 { format!("{}…", line.chars().take(119).collect::<String>()) } else { line.to_owned() }
}

fn possessive(name: &str) -> String {
    if name.ends_with('s') { format!("{name}'") } else { format!("{name}'s") }
}

/// The monitors and their fires.
pub fn monitors(host: &mut impl Host, look: &Look) -> Result<Vec<Row>, DataError> {
    let sql = if look.source == SCHEDULE {
        "SELECT m.agent, m.id, m.kind, m.name, s.spec, m.until, m.max_fires, m.fires, m.state, m.next_due, m.created \
         FROM monitors m LEFT JOIN monitor_specs s ON s.agent = m.agent AND s.id = m.id WHERE m.state = 'active' OR m.updated >= ?1"
    } else {
        "SELECT agent, id, kind, NULL, NULL, until, max_fires, fires, state, next_due, created FROM monitors WHERE state = 'active' OR updated >= ?1"
    };
    let found = rows(host, look.source, sql, json!([look.from]))?;
    let mut out = Vec::new();
    // Each monitor's name, and those drawn as one band (their fires go in it).
    let mut names = BTreeMap::new();
    let mut banded = BTreeSet::new();
    for row in &found {
        let (Some(agent), Some(id), Some(kind)) = (text(row, 0), int(row, 1), text(row, 2)) else { continue };
        let title = text(row, 3).filter(|name| !name.trim().is_empty()).map_or_else(|| format!("{} monitor", possessive(&look.agents.name(&agent))), |name| title_of(&name));
        names.insert((agent.clone(), id), title.clone());
        if text(row, 8).as_deref() != Some("active") {
            continue;
        }
        let spec: Option<Value> = text(row, 4).and_then(|spec| serde_json::from_str(&spec).ok());
        let until = int(row, 5);
        let left = int(row, 6).map(|most| (most - int(row, 7).unwrap_or(0)).max(0) as usize);
        let next = int(row, 9);
        let created = int(row, 10).unwrap_or(look.from);
        let who = look.agents.name(&agent);
        let base = Row {
            id: String::new(),
            start: 0,
            end: None,
            title,
            color: format!("agent:{agent}"),
            all_day: false,
            kind: "monitor",
            agent: agent.clone(),
            repeat: String::new(),
            detail: format!("From {} monitor.", possessive(&who)),
            open: Some(format!("monitor:{agent}:m{id}")),
            live: false,
            outside: false,
            layer: Layer::Monitors,
        };
        let end = until.map_or(look.to, |until| until.min(look.to));
        // When it stops, as a date: devices show times in their own zone.
        let ending = |words: String| match until {
            Some(until) => format!("{words} · until {}", time::short_date(look.zone.day(until))),
            None => words,
        };
        let band = |repeat: String, last: i64| Row {
            id: format!("band:{agent}:m{id}"),
            start: look.zone.day(created.max(look.from)) * DAY,
            end: Some((look.zone.day((last - 1).max(created.max(look.from))) + 1) * DAY),
            all_day: true,
            repeat,
            ..base.clone()
        };
        let (fires, repeat): (Vec<i64>, String) = match kind.as_str() {
            "at" => (next.into_iter().collect(), String::new()),
            "every" => match spec.as_ref().and_then(|spec| spec["every"].as_i64()).filter(|every| *every >= MINUTE) {
                Some(every) if every * MOST_A_DAY as i64 >= DAY => {
                    let fires = next.map_or_else(Vec::new, |next| {
                        (0..).map(|step| next + step * every).take_while(|at| *at < end).take(left.unwrap_or(usize::MAX)).collect()
                    });
                    (fires, ending(time::every_text(every)))
                }
                Some(every) => {
                    banded.insert((agent.clone(), id));
                    out.push(band(ending(time::every_text(every)), end));
                    continue;
                }
                None => (next.into_iter().collect(), "repeats".to_owned()),
            },
            "cron" => match spec.as_ref().and_then(|spec| spec["cron"].as_str()).and_then(Cron::parse) {
                Some(cron) => {
                    let fires: Vec<i64> = next.map_or_else(Vec::new, |next| {
                        let mut fires = cron.fires(look.zone, next.max(look.from), end + 1, 20_000);
                        fires.truncate(left.unwrap_or(usize::MAX));
                        fires
                    });
                    let mut per_day: BTreeMap<i64, usize> = BTreeMap::new();
                    for at in &fires {
                        *per_day.entry(look.zone.day(*at)).or_default() += 1;
                    }
                    if per_day.values().any(|count| *count > MOST_A_DAY) {
                        let most = per_day.values().max().copied().unwrap_or(0);
                        banded.insert((agent.clone(), id));
                        out.push(band(ending(format!("{} · up to {most} times a day", cron.words())), end));
                        continue;
                    }
                    (fires, ending(cron.words()))
                }
                None => (next.into_iter().collect(), "repeats".to_owned()),
            },
            // Waits for something rather than a time: one band for the
            // days it watches.
            other => {
                let words = match other {
                    "check" => "runs a check",
                    "files" => "when files change",
                    "watch" => "on a command's output",
                    "pipes" => "when pipes change",
                    _ => "watching",
                };
                banded.insert((agent.clone(), id));
                out.push(band(ending(words.to_owned()), end));
                continue;
            }
        };
        for at in fires.into_iter().filter(|at| *at >= look.from && *at < look.to) {
            out.push(Row { id: format!("mon:{agent}:m{id}:{at}"), start: at, repeat: repeat.clone(), ..base.clone() });
        }
    }

    // What already fired: a pill each, or a band for a day it fired more
    // than MOST_A_DAY times.
    let fired = rows(
        host,
        look.source,
        "SELECT agent, monitor, time FROM monitor_events WHERE kind = 'fired' AND time >= ?1 AND time < ?2 ORDER BY time",
        json!([look.from, look.to.min(look.now + 1)]),
    )?;
    let mut by_day: BTreeMap<(String, i64, i64), Vec<i64>> = BTreeMap::new();
    for row in fired {
        let (Some(agent), Some(id), Some(at)) = (text(&row, 0), int(&row, 1), int(&row, 2)) else { continue };
        if banded.contains(&(agent.clone(), id)) {
            continue;
        }
        by_day.entry((agent, id, look.zone.day(at))).or_default().push(at);
    }
    for ((agent, id, day), times) in by_day {
        let who = look.agents.name(&agent);
        let base = Row {
            id: String::new(),
            start: 0,
            end: None,
            title: names.get(&(agent.clone(), id)).cloned().unwrap_or_else(|| format!("{} monitor", possessive(&who))),
            color: format!("agent:{agent}"),
            all_day: false,
            kind: "fire",
            agent: agent.clone(),
            repeat: String::new(),
            detail: format!("{} monitor ran.", possessive(&who)),
            open: Some(format!("monitor:{agent}:m{id}")),
            live: false,
            outside: false,
            layer: Layer::Monitors,
        };
        if times.len() > MOST_A_DAY {
            out.push(Row {
                id: format!("fired:{agent}:m{id}:{day}"),
                start: day * DAY,
                end: Some((day + 1) * DAY),
                all_day: true,
                repeat: format!("fired {} times", times.len()),
                ..base
            });
        } else {
            for at in times {
                out.push(Row { id: format!("fire:{agent}:m{id}:{at}"), start: at, ..base.clone() });
            }
        }
    }
    Ok(out)
}

/// Checklist items' dates: when a blocked item is checked again, a parked
/// one resumes, or the person is asked again. Schedule alone has them.
pub fn items(host: &mut impl Host, look: &Look) -> Result<Vec<Row>, DataError> {
    if look.source != SCHEDULE {
        return Ok(Vec::new());
    }
    let found = rows(
        host,
        look.source,
        "SELECT agent, id, text, status, retry_at, reask_at FROM checklist_items \
         WHERE (retry_at >= ?1 AND retry_at < ?2 AND status IN ('blocked', 'parked')) \
            OR (reask_at >= ?1 AND reask_at < ?2 AND status NOT IN ('done', 'dropped'))",
        json!([look.from, look.to]),
    )?;
    let mut out = Vec::new();
    for row in found {
        let (Some(agent), Some(id)) = (text(&row, 0), int(&row, 1)) else { continue };
        let who = look.agents.name(&agent);
        let status = text(&row, 3).unwrap_or_default();
        let base = Row {
            id: String::new(),
            start: 0,
            end: None,
            title: title_of(&text(&row, 2).unwrap_or_default()),
            color: format!("agent:{agent}"),
            all_day: false,
            kind: "item",
            agent: agent.clone(),
            repeat: String::new(),
            detail: String::new(),
            open: Some(format!("item:{agent}:{id}")),
            live: false,
            outside: false,
            layer: Layer::Checklists,
        };
        if let Some(at) = int(&row, 4).filter(|at| *at >= look.from && *at < look.to && matches!(status.as_str(), "blocked" | "parked")) {
            let detail = if status == "parked" {
                format!("From {} checklist, set aside until then.", possessive(&who))
            } else {
                format!("From {} checklist, waiting on someone. {who} checks it again then.", possessive(&who))
            };
            out.push(Row { id: format!("item:{agent}:{id}:retry"), start: at, detail, ..base.clone() });
        }
        if let Some(at) = int(&row, 5).filter(|at| *at >= look.from && *at < look.to) {
            let detail = format!("From {} checklist. Claktory asks you about it again then.", possessive(&who));
            out.push(Row { id: format!("item:{agent}:{id}:reask"), start: at, detail, ..base });
        }
    }
    Ok(out)
}

/// Background tasks that ran at least [`LONG_TASK`] or run now, and runs
/// over [`LONG_RUN`].
pub fn work(host: &mut impl Host, look: &Look) -> Result<Vec<Row>, DataError> {
    let mut out = Vec::new();
    let tasks = rows(
        host,
        look.source,
        "SELECT agent, task, started, ended, status FROM tasks \
         WHERE started < ?2 AND (ended IS NULL OR (ended >= ?1 AND ended - started >= ?3))",
        json!([look.from, look.to, LONG_TASK]),
    )?;
    for row in tasks {
        let (Some(agent), Some(task), Some(started)) = (text(&row, 0), text(&row, 1), int(&row, 2)) else { continue };
        let ended = int(&row, 3);
        let who = look.agents.name(&agent);
        let detail = match (ended, text(&row, 4)) {
            (None, _) => format!("From a task {who} started. It's running."),
            (Some(_), Some(status)) if !status.is_empty() => format!("From a task {who} started. It ended: {status}."),
            (Some(_), _) => format!("From a task {who} started."),
        };
        out.push(Row {
            id: format!("task:{agent}:{task}"),
            start: started,
            end: ended,
            title: task.clone(),
            color: format!("agent:{agent}"),
            all_day: false,
            kind: "task",
            agent: agent.clone(),
            repeat: String::new(),
            detail,
            open: Some(format!("task:{agent}:{task}")),
            live: ended.is_none(),
            outside: false,
            layer: Layer::Tasks,
        });
    }
    let runs = rows(
        host,
        look.source,
        "SELECT id, agent, started, ended FROM runs WHERE started < ?2 AND (ended IS NULL OR ended >= ?1) AND COALESCE(ended, ?3) - started >= ?4",
        json!([look.from, look.to, look.now, LONG_RUN]),
    )?;
    for row in runs {
        let (Some(id), Some(agent), Some(started)) = (int(&row, 0), text(&row, 1), int(&row, 2)) else { continue };
        let ended = int(&row, 3);
        let who = look.agents.name(&agent);
        let detail = match ended {
            None => format!("{who} has been working for {}.", time::length_text(look.now - started)),
            Some(ended) => format!("{who} worked for {}.", time::length_text(ended - started)),
        };
        out.push(Row {
            id: format!("run:{id}"),
            start: started,
            end: ended,
            title: "Working".into(),
            color: format!("agent:{agent}"),
            all_day: false,
            kind: "run",
            agent: agent.clone(),
            repeat: String::new(),
            detail,
            open: Some(format!("agent:{agent}")),
            live: ended.is_none(),
            outside: false,
            layer: Layer::Tasks,
        });
    }
    Ok(out)
}

/// How a placed event repeats, in words.
pub fn repeat_words(event: &Event) -> String {
    let Some(repeat) = &event.repeat else { return String::new() };
    let every = match (repeat.every, repeat.interval) {
        (store::Unit::Day, 1) => "every day".to_owned(),
        (store::Unit::Week, 1) => "every week".to_owned(),
        (store::Unit::Month, 1) => "every month".to_owned(),
        (store::Unit::Day, n) => format!("every {n} days"),
        (store::Unit::Week, n) => format!("every {n} weeks"),
        (store::Unit::Month, n) => format!("every {n} months"),
    };
    if repeat.every == store::Unit::Week && !repeat.on.is_empty() {
        let days: Vec<&str> = repeat
            .on
            .iter()
            .filter_map(|day| time::WEEKDAYS.iter().position(|name| name == day).map(|at| time::WEEKDAY_NAMES[at]))
            .collect();
        if repeat.interval == 1 && days == ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday"] {
            return "weekdays".into();
        }
        return format!("{every} on {}", days.join(", "));
    }
    every
}

/// The placed events' occurrences.
pub fn placed(events: &[Event], calendars: &[Calendar], look: &Look) -> Vec<Row> {
    let mut out = Vec::new();
    for event in events {
        let calendar = event.calendar.as_ref().and_then(|id| calendars.iter().find(|calendar| calendar.id == *id));
        let maker = &event.created_by;
        let gone = look.agents.exists(maker) == Some(false);
        let mut detail = Vec::new();
        if let Some(note) = &event.note {
            detail.push(note.trim().to_owned());
        }
        let who = look.agents.name(maker);
        detail.push(match calendar {
            Some(calendar) => format!("Added by {who}, in {}.", calendar.name),
            None => format!("Added by {who}."),
        });
        if gone {
            detail.push(format!("{who} has since been deleted."));
        }
        if event.outside {
            detail.push(format!("{who} added this after reading a website or a pipe, so it may come from someone else."));
        }
        let color = calendar.and_then(|calendar| calendar.color.clone()).unwrap_or_else(|| format!("agent:{maker}"));
        let open = match &event.link {
            Some(link) => Some(link.open()),
            None if !gone => Some(format!("agent:{maker}")),
            None => None,
        };
        let repeat = repeat_words(event);
        for (start, end) in event.occurrences(look.zone, look.from, look.to) {
            out.push(Row {
                id: format!("ev:{}:{start}", event.id),
                start,
                end,
                title: event.title.clone(),
                color: color.clone(),
                all_day: event.all_day,
                kind: if event.deadline { "deadline" } else { "event" },
                agent: maker.clone(),
                repeat: repeat.clone(),
                detail: detail.join(" "),
                open: open.clone(),
                live: false,
                outside: event.outside,
                layer: Layer::Events,
            });
        }
    }
    out
}

/// Everything in `look`'s range on the layers asked for, in time order,
/// placed events from `events`.
pub fn all(host: &mut impl Host, look: &Look, layers: &[Layer], events: &[Event], calendars: &[Calendar]) -> Result<Vec<Row>, DataError> {
    let mut out = Vec::new();
    if layers.contains(&Layer::Monitors) {
        out.extend(monitors(host, look)?);
    }
    if layers.contains(&Layer::Checklists) {
        out.extend(items(host, look)?);
    }
    if layers.contains(&Layer::Tasks) {
        out.extend(work(host, look)?);
    }
    if layers.contains(&Layer::Events) {
        out.extend(placed(events, calendars, look));
    }
    out.sort_by(|a, b| (!a.all_day, a.start, &a.id).cmp(&(!b.all_day, b.start, &b.id)));
    out.truncate(MOST_ROWS);
    Ok(out)
}
