//! What Calendar keeps in its own store: the events agents place, the
//! calendars they go in, and how the person arranges it. Nothing derived
//! (monitors, items, tasks, runs) is ever kept: it's read live.
//!
//! Keys: `events`, the index (each event's id and the stretch its
//! occurrences fall in, so a range reads only the events in it);
//! `ev/<id>`, an event; `prev/<id>`, the version before its last change or
//! its deletion; `calendars`; `prefs`; `device/<stand-in>`, where each
//! device left it; `next`, the next event number.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Host;
use crate::time::{self, DAY, Zone};

/// Past events are dropped this long after their last occurrence.
pub const KEPT_MS: i64 = 400 * DAY;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Day,
    Week,
    Month,
}

/// How a placed event repeats: a small part of what calendars allow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repeat {
    pub every: Unit,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub interval: u32,
    /// Weekdays ("mo" … "su"), for every week; its own weekday when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on: Vec<String>,
    /// No occurrence starts after this (UTC ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<i64>,
    /// Or after this many.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
}

fn one() -> u32 {
    1
}

fn is_one(value: &u32) -> bool {
    *value == 1
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// What a placed event links to: its maker's monitor, an item or a task,
/// so it opens there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monitor: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

impl Link {
    /// Where the event opens: "monitor:fugo:m3", "item:lucy:12",
    /// "task:giorno:task-3" or "agent:fugo".
    pub fn open(&self) -> String {
        match (&self.item, &self.monitor, &self.task) {
            (Some(item), _, _) => format!("item:{}:{item}", self.agent),
            (_, Some(monitor), _) => format!("monitor:{}:m{monitor}", self.agent),
            (_, _, Some(task)) => format!("task:{}:{task}", self.agent),
            _ => format!("agent:{}", self.agent),
        }
    }
}

/// An event an agent placed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar: Option<String>,
    pub title: String,
    /// UTC ms; an all-day event's first day as its UTC midnight.
    pub start: i64,
    /// Exclusive; an all-day event's as the UTC midnight after its last
    /// day. None: a point in time, or one day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<i64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub all_day: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<Repeat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub deadline: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<Link>,
    #[serde(rename = "_rev")]
    pub rev: u64,
    /// The agent that placed it.
    pub created_by: String,
    pub created: i64,
    pub updated_by: String,
    pub updated: i64,
    /// The project it was placed from; None for the workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Placed or changed by an agent that had read the web or a pipe.
    #[serde(default, skip_serializing_if = "is_false")]
    pub outside: bool,
    /// Only on a `prev/<id>` kept at its deletion.
    #[serde(default, skip_serializing_if = "is_false")]
    pub deleted: bool,
}

/// Occurrences an event yields at most in one range, and steps tried.
const MOST_OCCURRENCES: usize = 400;
const MOST_STEPS: usize = 100_000;

impl Event {
    fn length(&self) -> i64 {
        self.end.map_or(0, |end| end - self.start)
    }

    /// Where its occurrences can fall: (first start, last end), the last
    /// None while it repeats for ever.
    pub fn span(&self) -> (i64, Option<i64>) {
        let length = self.length().max(if self.all_day { DAY } else { 0 });
        let Some(repeat) = &self.repeat else { return (self.start, Some(self.start + length)) };
        let step = i64::from(repeat.interval)
            * match repeat.every {
                Unit::Day => DAY,
                Unit::Week => 7 * DAY,
                Unit::Month => 31 * DAY,
            };
        let by_count = repeat.count.map(|count| self.start + i64::from(count) * step);
        let last = match (repeat.until, by_count) {
            (Some(until), Some(by_count)) => Some(until.min(by_count)),
            (until, by_count) => until.or(by_count),
        };
        (self.start, last.map(|last| last + length + DAY))
    }

    /// Its occurrences that overlap `from`..`to`: (start, end), in order.
    /// Repeats keep the Mac's wall-clock time across daylight saving; an
    /// all-day event's dates are dates wherever it's read.
    pub fn occurrences(&self, zone: &Zone, from: i64, to: i64) -> Vec<(i64, Option<i64>)> {
        let length = self.length();
        let overlaps = |start: i64| {
            let end = start + length.max(if self.all_day { DAY } else { 0 });
            start < to && (end > from || start >= from)
        };
        let Some(repeat) = &self.repeat else {
            return if overlaps(self.start) { vec![(self.start, self.end)] } else { Vec::new() };
        };
        // Its first day and time of day, on the Mac's clock (or as a date).
        let local = if self.all_day { self.start } else { zone.local(self.start) };
        let first_day = local.div_euclid(DAY);
        let clock = local.rem_euclid(DAY);
        let at = |days: i64| if self.all_day { days * DAY } else { zone.utc(days * DAY + clock) };
        let weekdays: Vec<u32> = if repeat.on.is_empty() {
            vec![time::weekday(first_day)]
        } else {
            let mut on: Vec<u32> =
                repeat.on.iter().filter_map(|day| time::WEEKDAYS.iter().position(|name| name == day).map(|at| at as u32)).collect();
            on.sort_unstable();
            on
        };
        let interval = i64::from(repeat.interval.max(1));
        let (first_year, first_month, first_date) = time::civil_from_days(first_day);
        let mut out = Vec::new();
        let mut counted = 0u32;
        for step in 0..MOST_STEPS as i64 {
            let days: Vec<i64> = match repeat.every {
                Unit::Day => vec![first_day + step * interval],
                Unit::Week => {
                    let monday = first_day - i64::from(time::weekday(first_day)) + step * interval * 7;
                    weekdays.iter().map(|day| monday + i64::from(*day)).filter(|days| *days >= first_day).collect()
                }
                Unit::Month => {
                    let months = i64::from(first_month) - 1 + step * interval;
                    let (year, month) = (first_year + months.div_euclid(12), (months.rem_euclid(12) + 1) as u32);
                    // A month without its date (the 31st, 29 February) is skipped.
                    if first_date <= time::days_in_month(year, month) {
                        vec![time::days_from_civil(year, month, first_date)]
                    } else {
                        Vec::new()
                    }
                }
            };
            for days in days {
                let start = at(days);
                if repeat.until.is_some_and(|until| start > until) || repeat.count.is_some_and(|count| counted >= count) || start >= to {
                    return out;
                }
                counted += 1;
                if overlaps(start) {
                    out.push((start, self.end.map(|_| start + length)));
                    if out.len() >= MOST_OCCURRENCES {
                        return out;
                    }
                }
            }
        }
        out
    }
}

/// A calendar placed events go in, such as Releases or Deadlines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Calendar {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// The colors a calendar can have, by name, as Charts' are.
pub const COLORS: [&str; 13] = [
    "green",
    "yellow",
    "blue",
    "orange",
    "red",
    "purple",
    "dark-green",
    "dark-yellow",
    "dark-blue",
    "dark-orange",
    "dark-red",
    "dark-purple",
    "text",
];

/// How the person arranges it, the same on every device.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prefs {
    /// Layers turned off under Show.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<String>,
    /// monday, sunday or saturday; the device's own when None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub week_start: Option<String>,
    /// The view a device opens on before it has chosen one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
}

pub const WEEK_STARTS: [&str; 3] = ["monday", "sunday", "saturday"];
pub const VIEWS: [&str; 3] = ["day", "week", "agenda"];

fn read<T: for<'a> Deserialize<'a>>(host: &mut impl Host, key: &str) -> Option<T> {
    host.kv_get(key).and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

fn write<T: Serialize>(host: &mut impl Host, key: &str, value: &T) -> Result<(), String> {
    host.kv_set(key, &serde_json::to_vec(value).map_err(|error| error.to_string())?)
}

/// The index: each event's id and (first start, last end).
pub type Index = BTreeMap<String, (i64, Option<i64>)>;

pub fn index(host: &mut impl Host) -> Index {
    read(host, "events").unwrap_or_default()
}

pub fn load(host: &mut impl Host, id: &str) -> Option<Event> {
    read(host, &format!("ev/{id}"))
}

pub fn previous(host: &mut impl Host, id: &str) -> Option<Event> {
    read(host, &format!("prev/{id}"))
}

/// The events with an occurrence in `from`..`to`.
pub fn in_range(host: &mut impl Host, from: i64, to: i64) -> Vec<Event> {
    let ids: Vec<String> =
        index(host).into_iter().filter(|(_, (first, last))| *first < to && last.is_none_or(|last| last > from)).map(|(id, _)| id).collect();
    ids.iter().filter_map(|id| load(host, id)).collect()
}

/// A new event's id: e1, e2…
pub fn next_id(host: &mut impl Host) -> Result<String, String> {
    let next: u64 = read(host, "next").unwrap_or(1);
    write(host, "next", &(next + 1))?;
    Ok(format!("e{next}"))
}

/// Keeps `event`, the version it replaces as its previous one.
pub fn save(host: &mut impl Host, event: &Event) -> Result<(), String> {
    if let Some(was) = load(host, &event.id) {
        write(host, &format!("prev/{}", event.id), &was)?;
    }
    write(host, &format!("ev/{}", event.id), event)?;
    let mut index = index(host);
    index.insert(event.id.clone(), event.span());
    write(host, "events", &index)
}

/// Deletes it, keeping it as its previous version.
pub fn delete(host: &mut impl Host, event: &Event) -> Result<(), String> {
    write(host, &format!("prev/{}", event.id), &Event { deleted: true, ..event.clone() })?;
    let mut index = index(host);
    index.remove(&event.id);
    write(host, "events", &index)?;
    host.kv_delete(&format!("ev/{}", event.id));
    Ok(())
}

/// Drops events whose last occurrence ended over [`KEPT_MS`] ago.
pub fn prune(host: &mut impl Host, now: i64) -> Result<usize, String> {
    let mut index = index(host);
    let old: Vec<String> =
        index.iter().filter(|(_, (_, last))| last.is_some_and(|last| last < now - KEPT_MS)).map(|(id, _)| id.clone()).collect();
    if old.is_empty() {
        return Ok(0);
    }
    for id in &old {
        index.remove(id);
        host.kv_delete(&format!("ev/{id}"));
        host.kv_delete(&format!("prev/{id}"));
    }
    write(host, "events", &index)?;
    Ok(old.len())
}

pub fn calendars(host: &mut impl Host) -> Vec<Calendar> {
    read(host, "calendars").unwrap_or_default()
}

pub fn save_calendars(host: &mut impl Host, calendars: &[Calendar]) -> Result<(), String> {
    write(host, "calendars", &calendars)
}

pub fn prefs(host: &mut impl Host) -> Prefs {
    read(host, "prefs").unwrap_or_default()
}

pub fn save_prefs(host: &mut impl Host, prefs: &Prefs) -> Result<(), String> {
    write(host, "prefs", prefs)
}

/// Where a device left Calendar: its view and Agent filter, kept, and
/// the day it was on, with when it left it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub anchor: i64,
    /// When it last moved or looked.
    pub at: i64,
}

/// How long a device's day is kept after it last looked: Open an entry,
/// come back, and it's where it was; later, it opens on today.
pub const DAY_KEPT_MS: i64 = 3_600_000;

pub fn device_state(host: &mut impl Host, device: &str) -> Option<DeviceState> {
    if device.is_empty() {
        return None;
    }
    read(host, &format!("device/{device}"))
}

pub fn save_device_state(host: &mut impl Host, device: &str, state: &DeviceState) -> Result<(), String> {
    if device.is_empty() || device.len() > 200 {
        return Ok(());
    }
    write(host, &format!("device/{device}"), state)
}
