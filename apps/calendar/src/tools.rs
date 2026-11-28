//! Agents' tools. `view`: calendar_list and calendar_get. `edit`: also
//! calendar_put, calendar_move, calendar_delete, calendar_calendars and
//! calendar_prefs. An agent sees and changes its own project's placed
//! events and entries (an admin agent all of them); what an agent that has
//! read the web or a pipe places is marked as from outside.

use std::fmt::Write as _;

use serde_json::{Map, Value, json};

use crate::layers::{self, Agents, Layer, Look, Row};
use crate::store::{self, Calendar, Event, Link, Repeat, Unit};
use crate::time::{self, DAY, Written, Zone};
use crate::{Caller, Host, Output, SCHEDULE};

/// The longest range calendar_list reads.
const MOST_RANGE: i64 = 62 * DAY;
/// Lines calendar_list writes at most; its data holds them all.
const MOST_LINES: usize = 200;
const MOST_TITLE: usize = 200;
const MOST_NOTE: usize = 2_000;
const MOST_CALENDARS: usize = 50;

fn ok(text: impl Into<String>, data: Value) -> Output {
    Output { text: text.into(), data: Some(data), is_error: false }
}

fn fail(text: impl Into<String>) -> Output {
    Output { text: text.into(), data: None, is_error: true }
}

pub fn call(host: &mut impl Host, tool: &str, args: &str, caller: &str) -> Output {
    let args: Value = match serde_json::from_str(if args.trim().is_empty() { "{}" } else { args }) {
        Ok(Value::Object(args)) => Value::Object(args),
        _ => return fail("The arguments must be a JSON object."),
    };
    // Before the host said who calls, an agent alone in the workspace.
    let caller = host.caller_info().unwrap_or(Caller { id: caller.to_owned(), project: None, admin: false, tainted: false });
    let result = match tool {
        "calendar_list" => list(host, &args, &caller),
        "calendar_get" => get(host, &args, &caller),
        "calendar_put" => put(host, &args, &caller),
        "calendar_move" => move_event(host, &args, &caller),
        "calendar_delete" => delete(host, &args, &caller),
        "calendar_calendars" => calendars(host, &args),
        "calendar_prefs" => prefs(host, &args),
        other => Err(format!("Calendar has no tool \"{other}\".")),
    };
    result.unwrap_or_else(fail)
}

/// What a tool needs from the source: which, the Mac's clock and the agents.
struct Reading {
    source: Option<&'static str>,
    zone: Zone,
    agents: Agents,
}

fn reading(host: &mut impl Host, from: i64, to: i64) -> Reading {
    let sources = host.data_sources();
    let source = layers::source(&sources);
    let zone = layers::zone(host, source, from, to).unwrap_or_default();
    let agents = source.and_then(|source| layers::agents(host, source).ok()).unwrap_or_default();
    Reading { source, zone, agents }
}

/// A time an agent wrote, as UTC ms (a date: the Mac's midnight).
fn instant(host: &mut impl Host, written: Written) -> Result<i64, String> {
    let local = match written {
        Written::Instant(at) => return Ok(at),
        Written::Date(days) => days * DAY,
        Written::Local(local) => local,
    };
    let reading = reading(host, local - DAY, local + DAY);
    if !reading.zone.is_known() {
        return Err("Calendar can't read the Mac's clock yet (the person hasn't let it read Activity or Schedule): write the time with its offset, such as \"2026-12-03 14:00+01:00\", or Z for UTC.".into());
    }
    Ok(reading.zone.utc(local))
}

fn field<'a>(args: &'a Value, key: &str) -> Option<&'a Value> {
    args.get(key).filter(|value| !value.is_null())
}

fn text_field(args: &Value, key: &str) -> Result<Option<String>, String> {
    match field(args, key) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(Value::Number(number)) => Ok(Some(number.to_string())),
        Some(_) => Err(format!("{key} must be text.")),
    }
}

fn written(args: &Value, key: &str, now: i64) -> Result<Option<Written>, String> {
    text_field(args, key)?.map(|text| time::parse(&text, now).map_err(|why| format!("{key}: {why}"))).transpose()
}

/// Whether `caller` may see and change `event`.
fn mine(event: &Event, caller: &Caller) -> bool {
    caller.admin || event.project == caller.project
}

/// A derived entry's id, and where to change it instead.
fn derived(id: &str, agents: &Agents) -> Option<String> {
    let mut parts = id.split(':');
    let kind = parts.next()?;
    let agent = parts.next().unwrap_or_default();
    let what = parts.next().unwrap_or_default();
    let who = agents.name(agent);
    let possessive = if who.ends_with('s') { format!("{who}'") } else { format!("{who}'s") };
    Some(match kind {
        "mon" | "band" | "fire" | "fired" => {
            format!("This comes from {possessive} monitor {what}. Change it there, with monitors_change or by asking {who}.")
        }
        "item" => {
            format!("This comes from {possessive} checklist, item #{what}. Change it there, with checklist_update or by asking {who}.")
        }
        "task" => format!("This is {possessive} background task {what}. Calendar only shows it."),
        "run" => "This is an agent's long run. Calendar only shows it.".into(),
        _ => return None,
    })
}

/// A placed event's id from an id calendar_list gives ("ev:e3:<start>" or "e3").
fn event_id(id: &str) -> &str {
    id.strip_prefix("ev:").map_or(id, |rest| rest.split(':').next().unwrap_or(rest))
}

/// When an entry is, as agents read it.
fn when(zone: &Zone, start: i64, end: Option<i64>, all_day: bool) -> String {
    if all_day {
        let first = start.div_euclid(DAY);
        let last = end.map_or(first, |end| (end - 1).div_euclid(DAY));
        return if last > first {
            format!("{} – {}, all day", time::date_text(first), time::date_text(last))
        } else {
            format!("{}, all day", time::date_text(first))
        };
    }
    match end {
        Some(end) if end > start => format!("{} – {}", zone.text(start), zone.text(end)),
        _ => zone.text(start),
    }
}

fn list(host: &mut impl Host, args: &Value, caller: &Caller) -> Result<Output, String> {
    let now = host.now_ms();
    let from = match written(args, "from", now)? {
        Some(at) => instant(host, at)?,
        None => now,
    };
    let to = match written(args, "to", now)? {
        Some(Written::Date(days)) => instant(host, Written::Date(days + 1))?,
        Some(at) => instant(host, at)?,
        None => from + 7 * DAY,
    };
    if to <= from {
        return Err("to must come after from.".into());
    }
    if to - from > MOST_RANGE {
        return Err("Ask for at most 62 days at a time.".into());
    }
    let layers: Vec<Layer> = match field(args, "layers") {
        None => Layer::ALL.to_vec(),
        Some(Value::Array(names)) => names
            .iter()
            .map(|name| {
                name.as_str().and_then(Layer::of).ok_or_else(|| format!("layers are monitors, checklists, tasks and events; not {name}."))
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("layers must be a list.".into()),
    };
    let agent = text_field(args, "agent")?.filter(|agent| !agent.is_empty());
    let reading = reading(host, from, to);
    let events: Vec<Event> = store::in_range(host, from, to).into_iter().filter(|event| mine(event, caller)).collect();
    let calendars = store::calendars(host);
    let rows: Vec<Row> = match reading.source {
        Some(source) => {
            let look = Look { source, zone: &reading.zone, agents: &reading.agents, from, to, now };
            layers::all(host, &look, &layers, &events, &calendars).map_err(|why| format!("Couldn't read the calendar: {why}."))?
        }
        None if layers.contains(&Layer::Events) => {
            let look = Look { source: "", zone: &reading.zone, agents: &reading.agents, from, to, now };
            layers::placed(&events, &calendars, &look)
        }
        None => Vec::new(),
    };
    let rows: Vec<Row> = rows
        .into_iter()
        .filter(|row| row.layer == Layer::Events || reading.agents.visible(&row.agent, Some(caller)))
        .filter(|row| agent.as_ref().is_none_or(|agent| row.agent == *agent))
        .collect();

    let mut out = String::new();
    match reading.source {
        None => out.push_str("Calendar can't read Claktory yet (the person hasn't let it read Activity or Schedule), so this is only placed events, in UTC.\n"),
        Some(source) if source != SCHEDULE => out.push_str("Calendar reads Activity alone, so monitors have no names and checklist items aren't shown; the person can let it read Schedule.\n"),
        Some(_) => {}
    }
    if rows.is_empty() {
        let _ = writeln!(out, "Nothing on the calendar from {} to {}.", reading.zone.text(from), reading.zone.text(to));
    }
    for row in rows.iter().take(MOST_LINES) {
        let mut line = format!(
            "{} · {} · {} · {}",
            when(&reading.zone, row.start, row.end, row.all_day),
            reading.agents.name(&row.agent),
            row.kind,
            row.title
        );
        if !row.repeat.is_empty() {
            let _ = write!(line, " · {}", row.repeat);
        }
        if row.live {
            line.push_str(" · now");
        }
        if row.outside {
            line.push_str(" · from outside");
        }
        let id = if row.layer == Layer::Events { event_id(&row.id) } else { &row.id };
        let _ = writeln!(out, "{line} [{id}]");
    }
    if rows.len() > MOST_LINES {
        let _ = writeln!(out, "…and {} more; ask for a shorter range or one agent.", rows.len() - MOST_LINES);
    }
    let data: Vec<Value> = rows
        .iter()
        .map(|row| {
            json!({
                "id": if row.layer == Layer::Events { event_id(&row.id) } else { &row.id },
                "kind": row.kind, "title": row.title, "agent": row.agent,
                "start": reading.zone.text(row.start), "end": row.end.map(|end| reading.zone.text(end)),
                "all_day": row.all_day, "repeat": row.repeat, "live": row.live, "outside": row.outside,
            })
        })
        .collect();
    Ok(ok(out.trim_end().to_owned(), json!({"entries": data})))
}

/// An event as agents read it: its times in the Mac's clock.
fn shown(event: &Event, zone: &Zone) -> Value {
    let mut out = serde_json::to_value(event).unwrap_or(Value::Null);
    if event.all_day {
        out["start"] = json!(time::date_text(event.start.div_euclid(DAY)));
        out["end"] = match event.end {
            Some(end) if end - event.start > DAY => json!(time::date_text((end - 1).div_euclid(DAY))),
            _ => Value::Null,
        };
    } else {
        out["start"] = json!(zone.text(event.start));
        out["end"] = event.end.map_or(Value::Null, |end| json!(zone.text(end)));
    }
    if let Some(until) = event.repeat.as_ref().and_then(|repeat| repeat.until) {
        out["repeat"]["until"] = json!(if event.all_day { time::date_text(until.div_euclid(DAY)) } else { zone.text(until) });
    }
    if let Some(object) = out.as_object_mut() {
        object.retain(|_, value| !value.is_null());
    }
    out
}

fn get(host: &mut impl Host, args: &Value, caller: &Caller) -> Result<Output, String> {
    let id = text_field(args, "id")?.ok_or("id is needed.")?;
    let previous = field(args, "previous").and_then(Value::as_bool).unwrap_or(false);
    let found = if previous { store::previous(host, event_id(&id)) } else { store::load(host, event_id(&id)) };
    match found.filter(|event| mine(event, caller)) {
        Some(event) => {
            let reading = reading(host, event.start - DAY, event.start + DAY);
            let mut text = serde_json::to_string_pretty(&shown(&event, &reading.zone)).unwrap_or_default();
            if event.deleted {
                text = format!("Deleted; calendar_put of it (without its id's _rev) brings it back.\n{text}");
            }
            Ok(ok(text, shown(&event, &reading.zone)))
        }
        None => {
            let now = host.now_ms();
            let reading = reading(host, now, now);
            match derived(&id, &reading.agents) {
                Some(where_) => Ok(ok(where_, json!({"id": id, "derived": true}))),
                None if previous => Err(format!("{id} has no previous version.")),
                None => Err(format!("There's no event {id}; calendar_list gives the ids.")),
            }
        }
    }
}

/// Reads a repeat an agent wrote.
fn repeat_of(host: &mut impl Host, value: &Value, all_day: bool, now: i64) -> Result<Repeat, String> {
    let every = match value["every"].as_str() {
        Some("day") => Unit::Day,
        Some("week") => Unit::Week,
        Some("month") => Unit::Month,
        _ => return Err("repeat.every is day, week or month.".into()),
    };
    let interval = match field(value, "interval") {
        None => 1,
        Some(interval) => {
            interval.as_u64().filter(|n| (1..=99).contains(n)).ok_or("repeat.interval is a whole number from 1 to 99.")? as u32
        }
    };
    let on: Vec<String> = match field(value, "on") {
        None => Vec::new(),
        Some(Value::Array(days)) => days
            .iter()
            .map(|day| {
                let day = day.as_str().unwrap_or_default().to_ascii_lowercase();
                let day: String = day.chars().take(2).collect();
                if time::WEEKDAYS.contains(&day.as_str()) {
                    Ok(day)
                } else {
                    Err("repeat.on lists weekdays: mo, tu, we, th, fr, sa, su.".to_owned())
                }
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("repeat.on is a list of weekdays.".into()),
    };
    if !on.is_empty() && every != Unit::Week {
        return Err("repeat.on goes with every week.".into());
    }
    let until = match written(value, "until", now)? {
        None => None,
        Some(Written::Date(days)) if all_day => Some(days * DAY),
        Some(Written::Date(days)) => Some(instant(host, Written::Date(days + 1))? - 1),
        Some(at) => Some(instant(host, at)?),
    };
    let count = match field(value, "count") {
        None => None,
        Some(count) => {
            Some(count.as_u64().filter(|n| (1..=1000).contains(n)).ok_or("repeat.count is a whole number from 1 to 1000.")? as u32)
        }
    };
    if until.is_some() && count.is_some() {
        return Err("Give repeat.until or repeat.count, not both.".into());
    }
    Ok(Repeat { every, interval, on, until, count })
}

fn link_of(host: &mut impl Host, value: &Value) -> Result<Link, String> {
    let agent = value["agent"].as_str().filter(|agent| !agent.is_empty()).ok_or("link.agent is needed: the agent's id.")?.to_owned();
    let item = match field(value, "item") {
        None => None,
        Some(item) => Some(
            item.as_u64()
                .or_else(|| item.as_str().and_then(|item| item.trim_start_matches('#').parse().ok()))
                .ok_or("link.item is the item's number.")?,
        ),
    };
    let monitor = match field(value, "monitor") {
        None => None,
        Some(monitor) => Some(
            monitor
                .as_u64()
                .or_else(|| monitor.as_str().and_then(|monitor| monitor.trim_start_matches('m').parse().ok()))
                .ok_or("link.monitor is the monitor's id, such as m3.")?,
        ),
    };
    let task = text_field(value, "task")?;
    // As devices open them: ASCII letters, digits, _ and -, up to 64.
    let plain = |id: &str| !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !plain(&agent) {
        return Err(format!("link.agent \"{agent}\" isn't an agent's id."));
    }
    if task.as_deref().is_some_and(|task| !plain(task)) {
        return Err("link.task is a task's id, such as task-3.".into());
    }
    if [item.is_some(), monitor.is_some(), task.is_some()].into_iter().filter(|set| *set).count() > 1 {
        return Err("A link names one of item, monitor or task.".into());
    }
    // What it names must be there, when Calendar can read Claktory.
    let sources = host.data_sources();
    if let Some(source) = layers::source(&sources) {
        let mut check = |sql: &str, params: Value| -> Result<bool, String> {
            Ok(!layers::rows(host, source, sql, params).map_err(|why| format!("Couldn't check the link: {why}."))?.is_empty())
        };
        let (found, what) = match (&item, &monitor, &task) {
            (Some(item), _, _) => {
                (check("SELECT 1 FROM checklist_items WHERE agent = ?1 AND id = ?2", json!([agent, item]))?, format!("item #{item}"))
            }
            (_, Some(monitor), _) => {
                (check("SELECT 1 FROM monitors WHERE agent = ?1 AND id = ?2", json!([agent, monitor]))?, format!("monitor m{monitor}"))
            }
            (_, _, Some(task)) => {
                (check("SELECT 1 FROM tasks WHERE agent = ?1 AND task = ?2", json!([agent, task]))?, format!("task {task}"))
            }
            _ => (true, String::new()),
        };
        if !found {
            return Err(format!("{agent} has no {what}."));
        }
        if source == SCHEDULE && layers::agents(host, source).is_ok_and(|agents| agents.exists(&agent) == Some(false)) {
            return Err(format!("There's no agent {agent}."));
        }
    }
    Ok(Link { agent, item, monitor, task })
}

/// The start and end an agent wrote, as kept: an all-day event's as UTC
/// midnights (end exclusive), a timed one's as instants.
fn span_of(host: &mut impl Host, start: Written, end: Option<Written>, all_day: bool) -> Result<(i64, Option<i64>), String> {
    if all_day {
        let first = day_of(host, start);
        let last = end.map(|end| day_of(host, end));
        if last.is_some_and(|last| last < first) {
            return Err("end comes before start.".into());
        }
        let end = last.filter(|last| *last > first).map(|last| (last + 1) * DAY);
        return Ok((first * DAY, end));
    }
    let start = instant(host, start)?;
    let end = match end {
        Some(Written::Date(days)) => Some(instant(host, Written::Date(days + 1))?),
        Some(end) => Some(instant(host, end)?),
        None => None,
    };
    if end.is_some_and(|end| end < start) {
        return Err("end comes before start.".into());
    }
    Ok((start, end.filter(|end| *end > start)))
}

/// The day a written time falls on, on the Mac's clock (UTC when it
/// can't read it).
fn day_of(host: &mut impl Host, at: Written) -> i64 {
    match at {
        Written::Date(days) => days,
        Written::Local(local) => local.div_euclid(DAY),
        Written::Instant(at) => {
            let reading = reading(host, at, at);
            if reading.zone.is_known() { reading.zone.day(at) } else { at.div_euclid(DAY) }
        }
    }
}

fn check_rev(args: &Value, event: &Event) -> Result<(), String> {
    match field(args, "_rev").and_then(Value::as_u64) {
        Some(rev) if rev == event.rev => Ok(()),
        Some(rev) => {
            Err(format!("{} changed since you read it (_rev {} now, you sent {rev}): calendar_get it again.", event.id, event.rev))
        }
        None => Err(format!("Send the _rev you read ({} has _rev {}), so a change made since isn't lost.", event.id, event.rev)),
    }
}

/// Tells an event's maker that someone else moved or deleted it, when the
/// maker linked it to its monitor: the monitor may need moving too.
/// Answers what the caller is told.
fn tell_maker(host: &mut impl Host, event: &Event, caller: &Caller, done: &str) -> String {
    let Some(monitor) = event.link.as_ref().filter(|link| link.agent == event.created_by).and_then(|link| link.monitor) else {
        return String::new();
    };
    if event.created_by == caller.id || event.created_by.is_empty() {
        return String::new();
    }
    let now = host.now_ms();
    let reading = reading(host, now, now);
    let (maker, mover) = (reading.agents.name(&event.created_by), reading.agents.name(&caller.id));
    let text = format!(
        "Calendar: {mover} {done} \"{}\" ({}), which you linked to your monitor m{monitor}. Change the monitor if it should fire at another time, or cancel it.",
        event.title, event.id
    );
    host.emit_to(&event.created_by, "moved", &text, "edit");
    format!(" {maker} linked it to their monitor m{monitor}, so Calendar told them; if they don't hear Calendar, tell them yourself.")
}

fn put(host: &mut impl Host, args: &Value, caller: &Caller) -> Result<Output, String> {
    let Some(given) = field(args, "event").filter(|event| event.is_object()) else {
        return Err("event is needed: {title, start, …}.".into());
    };
    let now = host.now_ms();
    let had = match text_field(given, "id")?.filter(|id| !id.is_empty()) {
        Some(id) => {
            let reading = reading(host, now, now);
            if let Some(where_) = derived(&id, &reading.agents) {
                return Err(where_);
            }
            let had = store::load(host, event_id(&id)).filter(|event| mine(event, caller));
            match had {
                Some(had) => {
                    check_rev(given, &had)?;
                    Some(had)
                }
                // Back from its deletion.
                None if store::previous(host, event_id(&id)).is_some_and(|event| event.deleted && mine(&event, caller)) => None,
                None => return Err(format!("There's no event {id}; leave id out to add one.")),
            }
        }
        None => None,
    };
    let title = text_field(given, "title")?.map(|title| title.trim().to_owned()).unwrap_or_default();
    if title.is_empty() || title.chars().count() > MOST_TITLE || title.chars().any(char::is_control) {
        return Err(format!("title is needed: one line of up to {MOST_TITLE} characters."));
    }
    let start = written(given, "start", now)?.ok_or("start is needed: \"2026-12-03 14:00\", or \"2026-12-03\" for an all-day event.")?;
    let end = written(given, "end", now)?;
    let all_day = match field(given, "all_day") {
        Some(all_day) => all_day.as_bool().ok_or("all_day is true or false.")?,
        None => matches!(start, Written::Date(_)),
    };
    let (start, end) = span_of(host, start, end, all_day)?;
    if end.is_some_and(|end| end - start > 366 * DAY) {
        return Err("An event lasts at most a year; repeat it instead.".into());
    }
    let calendar = text_field(given, "calendar")?.filter(|calendar| !calendar.is_empty());
    if let Some(calendar) = &calendar
        && !store::calendars(host).iter().any(|known| known.id == *calendar)
    {
        return Err(format!("There's no calendar \"{calendar}\"; calendar_calendars lists them and makes one."));
    }
    let note = text_field(given, "note")?.map(|note| note.trim().to_owned()).filter(|note| !note.is_empty());
    if note.as_ref().is_some_and(|note| note.chars().count() > MOST_NOTE) {
        return Err(format!("note is at most {MOST_NOTE} characters."));
    }
    let repeat = field(given, "repeat").map(|repeat| repeat_of(host, repeat, all_day, now)).transpose()?;
    let link = field(given, "link").map(|link| link_of(host, link)).transpose()?;
    let deadline = field(given, "deadline").and_then(Value::as_bool).unwrap_or(false);
    let id = match (&had, text_field(given, "id")?.filter(|id| !id.is_empty())) {
        (Some(had), _) => had.id.clone(),
        (None, Some(id)) => event_id(&id).to_owned(),
        (None, None) => store::next_id(host)?,
    };
    let event = Event {
        id,
        calendar,
        title,
        start,
        end,
        all_day,
        repeat,
        note,
        deadline,
        link,
        rev: had.as_ref().map_or(1, |had| had.rev + 1),
        created_by: had.as_ref().map_or_else(|| caller.id.clone(), |had| had.created_by.clone()),
        created: had.as_ref().map_or(now, |had| had.created),
        updated_by: caller.id.clone(),
        updated: now,
        project: had.as_ref().map_or_else(|| caller.project.clone(), |had| had.project.clone()),
        outside: had.as_ref().is_some_and(|had| had.outside) || caller.tainted,
        deleted: false,
    };
    store::save(host, &event)?;
    let moved = had.as_ref().is_some_and(|had| (had.start, had.end, &had.repeat) != (event.start, event.end, &event.repeat));
    let told = if moved { tell_maker(host, &event, caller, "moved") } else { String::new() };
    let reading = reading(host, event.start - DAY, event.start + DAY);
    let verb = if had.is_some() { "Changed" } else { "Placed" };
    Ok(ok(
        format!(
            "{verb} {} (_rev {}): \"{}\", {}.{told}",
            event.id,
            event.rev,
            event.title,
            when(&reading.zone, event.start, event.end, event.all_day)
        ),
        json!({"id": event.id, "_rev": event.rev, "changed": true}),
    ))
}

fn move_event(host: &mut impl Host, args: &Value, caller: &Caller) -> Result<Output, String> {
    let id = text_field(args, "id")?.ok_or("id is needed.")?;
    let now = host.now_ms();
    let Some(had) = store::load(host, event_id(&id)).filter(|event| mine(event, caller)) else {
        let reading = reading(host, now, now);
        return Err(derived(&id, &reading.agents).unwrap_or_else(|| format!("There's no event {id}; calendar_list gives the ids.")));
    };
    if field(args, "_rev").is_some() {
        check_rev(args, &had)?;
    }
    let start = written(args, "start", now)?.ok_or("start is needed: where it moves to.")?;
    let end = written(args, "end", now)?;
    let (start, end) = match end {
        Some(end) => span_of(host, start, Some(end), had.all_day)?,
        None => {
            let (start, _) = span_of(host, start, None, had.all_day)?;
            (start, had.end.map(|end| start + (end - had.start)))
        }
    };
    let event = Event {
        start,
        end,
        rev: had.rev + 1,
        updated_by: caller.id.clone(),
        updated: now,
        outside: had.outside || caller.tainted,
        ..had.clone()
    };
    store::save(host, &event)?;
    let told = tell_maker(host, &event, caller, "moved");
    let reading = reading(host, start - DAY, start + DAY);
    Ok(ok(
        format!(
            "Moved {} (_rev {}): \"{}\", {}.{told}",
            event.id,
            event.rev,
            event.title,
            when(&reading.zone, event.start, event.end, event.all_day)
        ),
        json!({"id": event.id, "_rev": event.rev, "changed": true}),
    ))
}

fn delete(host: &mut impl Host, args: &Value, caller: &Caller) -> Result<Output, String> {
    let id = text_field(args, "id")?.ok_or("id is needed.")?;
    let Some(had) = store::load(host, event_id(&id)).filter(|event| mine(event, caller)) else {
        let now = host.now_ms();
        let reading = reading(host, now, now);
        return Err(derived(&id, &reading.agents).unwrap_or_else(|| format!("There's no event {id}.")));
    };
    store::delete(host, &had)?;
    let told = tell_maker(host, &had, caller, "deleted");
    Ok(ok(
        format!(
            "Deleted {} (\"{}\"). calendar_get with previous: true shows it, and calendar_put of it brings it back.{told}",
            had.id, had.title
        ),
        json!({"id": had.id, "changed": true}),
    ))
}

fn calendars(host: &mut impl Host, args: &Value) -> Result<Output, String> {
    let mut all = store::calendars(host);
    let mut changed = false;
    if let Some(put) = field(args, "put") {
        let name = put["name"]
            .as_str()
            .map(str::trim)
            .filter(|name| !name.is_empty() && name.chars().count() <= 60)
            .ok_or("put.name is needed: up to 60 characters.")?;
        let color = text_field(put, "color")?.filter(|color| !color.is_empty());
        if let Some(color) = &color
            && !store::COLORS.contains(&color.as_str())
        {
            return Err(format!("put.color is one of {}.", store::COLORS.join(", ")));
        }
        let id = match text_field(put, "id")?.filter(|id| !id.is_empty()) {
            Some(id) => id,
            None => name
                .to_lowercase()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect::<String>()
                .trim_matches('-')
                .to_owned(),
        };
        if id.is_empty() || id.len() > 40 || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') {
            return Err("put.id is lowercase letters, digits, - and _.".into());
        }
        let calendar = Calendar { id: id.clone(), name: name.to_owned(), color };
        match all.iter().position(|known| known.id == id) {
            Some(at) => all[at] = calendar,
            None if all.len() >= MOST_CALENDARS => return Err(format!("Calendar keeps at most {MOST_CALENDARS} calendars.")),
            None => all.push(calendar),
        }
        changed = true;
    }
    if let Some(id) = text_field(args, "delete")? {
        if !all.iter().any(|known| known.id == id) {
            return Err(format!("There's no calendar \"{id}\"."));
        }
        let index = store::index(host);
        let used = index
            .keys()
            .filter(|event| store::load(host, event).is_some_and(|event| event.calendar.as_deref() == Some(id.as_str())))
            .count();
        if used > 0 {
            return Err(format!("{used} events are in \"{id}\": move them to another calendar, or delete them, first."));
        }
        all.retain(|known| known.id != id);
        changed = true;
    }
    if changed {
        store::save_calendars(host, &all)?;
    }
    let text = if all.is_empty() {
        "No calendars yet: events go in none unless you make one (put: {name, color?}).".to_owned()
    } else {
        all.iter()
            .map(|calendar| {
                format!(
                    "{} · {}{}",
                    calendar.id,
                    calendar.name,
                    calendar.color.as_ref().map(|color| format!(" · {color}")).unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(ok(text, json!({"calendars": all, "changed": changed})))
}

fn prefs(host: &mut impl Host, args: &Value) -> Result<Output, String> {
    let mut prefs = store::prefs(host);
    let mut changed = false;
    if let Some(day) = text_field(args, "week_starts")? {
        if !day.is_empty() && !store::WEEK_STARTS.contains(&day.as_str()) {
            return Err("week_starts is monday, sunday or saturday; \"\" for each device's own.".into());
        }
        prefs.week_start = Some(day).filter(|day| !day.is_empty());
        changed = true;
    }
    if let Some(hidden) = field(args, "hidden_layers") {
        let Value::Array(names) = hidden else { return Err("hidden_layers is a list.".into()) };
        prefs.hidden = names
            .iter()
            .map(|name| {
                name.as_str()
                    .and_then(Layer::of)
                    .map(|layer| layer.id().to_owned())
                    .ok_or("hidden_layers are monitors, checklists, tasks and events.")
            })
            .collect::<Result<_, _>>()?;
        changed = true;
    }
    if let Some(view) = text_field(args, "view")? {
        if !view.is_empty() && !store::VIEWS.contains(&view.as_str()) {
            return Err("view is day, week or agenda.".into());
        }
        prefs.view = Some(view).filter(|view| !view.is_empty());
        changed = true;
    }
    if changed {
        store::save_prefs(host, &prefs)?;
    }
    let mut data = Map::new();
    data.insert("week_starts".into(), json!(prefs.week_start.as_deref().unwrap_or("")));
    data.insert("hidden_layers".into(), json!(prefs.hidden));
    data.insert("view".into(), json!(prefs.view.as_deref().unwrap_or("")));
    data.insert("changed".into(), json!(changed));
    Ok(ok(
        format!(
            "Week starts on: {}. Hidden: {}. Opens on: {}.",
            prefs.week_start.as_deref().unwrap_or("each device's own"),
            if prefs.hidden.is_empty() { "nothing".to_owned() } else { prefs.hidden.join(", ") },
            prefs.view.as_deref().unwrap_or("week (agenda where narrow)"),
        ),
        Value::Object(data),
    ))
}
