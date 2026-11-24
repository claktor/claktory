//! What a monitor waits for: read from what the agent wrote, kept as JSON
//! with the monitor, and described in one line. Times are local, as the
//! person reads them.

use std::path::Path;

use jiff::civil::{Date, DateTime};
use jiff::tz::TimeZone;
use jiff::{Timestamp, ZonedDateTime};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const SECOND: i64 = 1000;
pub(crate) const MINUTE: i64 = 60 * SECOND;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// The shortest period of `every` and `cron`, and the soonest `at`.
const MIN_TIME: i64 = MINUTE;
/// The shortest period of `check`.
const MIN_CHECK: i64 = 30 * SECOND;
/// Between a watch's fires, unless it says otherwise; never less than `MIN_GAP`.
const DEFAULT_GAP: i64 = 30 * SECOND;
const MIN_GAP: i64 = 10 * SECOND;
/// Paths a files monitor names at most.
const MAX_PATHS: usize = 20;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum When {
    /// Once, at a time (ms since 1970).
    At { at: i64 },
    /// Every period (ms), counted from the last fire.
    Every { every: i64 },
    /// At each match of a 5-field cron schedule, in local time.
    Cron { cron: String },
    /// Runs `command` every period (ms) and fires when `fire_on` becomes true.
    Check {
        command: String,
        every: i64,
        fire_on: FireOn,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pattern: Option<String>,
        #[serde(default)]
        repeat: Repeat,
        /// Secrets of the vault its command gets, as bash's `secrets`
        /// does: named by the agent itself when it set the monitor.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        secrets: Vec<String>,
    },
    /// Fires when any of these files, or anything in these folders, changes.
    Files {
        paths: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        glob: Option<String>,
    },
    /// Keeps `command` running; fires on its output lines (those matching `pattern`).
    Watch {
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pattern: Option<String>,
        /// The least time between fires, in ms.
        gap: i64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FireOn {
    Success,
    Failure,
    Match,
    Changed,
}

/// When a check fires again after its first fire.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Repeat {
    /// Never: one fire, then it's done.
    #[default]
    Once,
    /// Each time the condition becomes true again.
    Again,
    /// At every check where the condition holds.
    Always,
}

impl FireOn {
    fn name(self) -> &'static str {
        match self {
            FireOn::Success => "success",
            FireOn::Failure => "failure",
            FireOn::Match => "match",
            FireOn::Changed => "changed",
        }
    }
}

impl When {
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            When::At { .. } => "at",
            When::Every { .. } => "every",
            When::Cron { .. } => "cron",
            When::Check { .. } => "check",
            When::Files { .. } => "files",
            When::Watch { .. } => "watch",
        }
    }

    /// Fires at times rather than on what happens.
    pub(crate) fn is_time(&self) -> bool {
        matches!(self, When::At { .. } | When::Every { .. } | When::Cron { .. })
    }

    /// When it's next due after firing (or checking) at `at`; None when it's done.
    pub(crate) fn next(&self, at: i64) -> Option<i64> {
        match self {
            When::At { .. } => None,
            When::Every { every } => Some(at + every),
            When::Cron { cron } => Cron::parse(cron).ok().and_then(|cron| cron.next_after(at)),
            When::Check { every, .. } => Some(at + every),
            When::Files { .. } | When::Watch { .. } => None,
        }
    }

    /// "every 24h", "check `gh run view` every 5m on match \"success\"".
    pub(crate) fn describe(&self, now: i64) -> String {
        match self {
            When::At { at } => format!("at {}", local(*at, now)),
            When::Every { every } => format!("every {}", duration(*every)),
            When::Cron { cron } => format!("cron {cron}"),
            When::Check { command, every, fire_on, pattern, repeat, secrets } => {
                let on = match (fire_on, pattern) {
                    (FireOn::Match, Some(pattern)) => format!("match \"{pattern}\""),
                    (fire_on, _) => fire_on.name().to_owned(),
                };
                let again = match repeat {
                    Repeat::Once => "",
                    Repeat::Again => ", each time",
                    Repeat::Always => ", whenever it holds",
                };
                let with = match secrets.as_slice() {
                    [] => String::new(),
                    [one] => format!(", with the secret {one}"),
                    many => format!(", with the secrets {}", many.join(", ")),
                };
                format!("check `{}` every {}, fires on {on}{again}{with}", short(command), duration(*every))
            }
            When::Files { paths, glob } => {
                let only = glob.as_ref().map(|glob| format!(" ({glob})")).unwrap_or_default();
                format!("files {}{only}", paths.join(", "))
            }
            When::Watch { command, pattern, .. } => match pattern {
                Some(pattern) => format!("watch `{}` lines matching \"{pattern}\"", short(command)),
                None => format!("watch `{}`", short(command)),
            },
        }
    }
}

/// A command's first line, cut to 60 characters.
fn short(command: &str) -> String {
    let first = command.lines().next().unwrap_or_default().trim();
    let cut: String = first.chars().take(60).collect();
    if cut.len() < command.trim().len() { format!("{cut}…") } else { cut }
}

/// Secrets a check may name at most.
const MAX_SECRETS: usize = 10;

/// A check's `secrets`: a name or a list of names, each once.
fn secrets(fields: &serde_json::Map<String, Value>) -> Result<Vec<String>, String> {
    let listed: Vec<&Value> = match fields.get("secrets") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(names)) => names.iter().collect(),
        Some(name) => vec![name],
    };
    let mut names: Vec<String> = Vec::new();
    for name in listed {
        let Some(name) = name.as_str().map(str::trim).filter(|name| !name.is_empty()) else {
            return Err("secrets must be a secret's name or a list of names, as bash's secrets.".to_owned());
        };
        if !names.iter().any(|seen| seen == name) {
            names.push(name.to_owned());
        }
    }
    if names.len() > MAX_SECRETS {
        return Err(format!("A check can use at most {MAX_SECRETS} secrets."));
    }
    Ok(names)
}

/// Reads the agent's `when`: what it waits for, and when it's first due (None
/// for files and watch, which don't run on a schedule). Relative paths are
/// in `folder`.
pub(crate) fn parse(when: &Value, folder: &Path, now: i64) -> Result<(When, Option<i64>), String> {
    let Some(fields) = when.as_object() else {
        return Err("when must be an object, e.g. {\"every\": \"24h\"} or {\"check\": \"<command>\", \"every\": \"5m\"}.".to_owned());
    };
    let text = |key: &str| -> Result<Option<String>, String> {
        match fields.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) if !text.trim().is_empty() => Ok(Some(text.trim().to_owned())),
            Some(_) => Err(format!("{key} must be a non-empty string.")),
        }
    };
    let pattern = text("match")?;
    if let Some(pattern) = &pattern {
        regex::Regex::new(pattern).map_err(|error| format!("match isn't a valid regex: {error}"))?;
    }
    if let Some(command) = text("check")? {
        let every = match text("every")? {
            Some(every) => parse_duration(&every)?,
            None => 5 * MINUTE,
        };
        if every < MIN_CHECK {
            return Err(format!("A check's every must be at least 30s; got {}. For faster reactions use watch or files.", duration(every)));
        }
        let fire_on = match text("fire_on")?.as_deref() {
            None if pattern.is_some() => FireOn::Match,
            None | Some("success") => FireOn::Success,
            Some("failure") => FireOn::Failure,
            Some("match") => FireOn::Match,
            Some("changed") => FireOn::Changed,
            Some(other) => return Err(format!("fire_on must be success, failure, match or changed; got \"{other}\".")),
        };
        if fire_on == FireOn::Match && pattern.is_none() {
            return Err("fire_on match needs match, a regex for the output.".to_owned());
        }
        let repeat = match fields.get("repeat") {
            None | Some(Value::Null) | Some(Value::Bool(false)) => Repeat::Once,
            Some(Value::Bool(true)) => Repeat::Again,
            Some(Value::String(always)) if always == "always" => Repeat::Always,
            Some(other) => return Err(format!("repeat must be true, false or \"always\"; got {other}.")),
        };
        return Ok((When::Check { command, every, fire_on, pattern, repeat, secrets: secrets(fields)? }, Some(now)));
    }
    if fields.get("secrets").is_some_and(|secrets| !secrets.is_null()) {
        return Err("secrets works only with check: a check's command gets them each time it runs.".to_owned());
    }
    if let Some(command) = text("watch")? {
        let gap = match text("gap")? {
            Some(gap) => parse_duration(&gap)?,
            None => DEFAULT_GAP,
        };
        if gap < MIN_GAP {
            return Err(format!("A watch's gap must be at least 10s; got {}.", duration(gap)));
        }
        return Ok((When::Watch { command, pattern, gap }, None));
    }
    if let Some(files) = fields.get("files") {
        let listed: Vec<&str> = match files {
            Value::String(path) => vec![path.as_str()],
            Value::Array(paths) => paths.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        let listed: Vec<&str> = listed.into_iter().map(str::trim).filter(|path| !path.is_empty()).collect();
        if listed.is_empty() {
            return Err("files must be a path or a list of paths.".to_owned());
        }
        if listed.len() > MAX_PATHS {
            return Err(format!("files takes at most {MAX_PATHS} paths; watch their folder instead."));
        }
        let paths = listed.iter().map(|path| folder.join(path).to_string_lossy().into_owned()).collect();
        let glob = text("glob")?;
        if let Some(glob) = &glob {
            globset::Glob::new(glob).map_err(|error| format!("glob isn't valid: {error}"))?;
        }
        return Ok((When::Files { paths, glob }, None));
    }
    if let Some(cron) = text("cron")? {
        let parsed = Cron::parse(&cron)?;
        let first = parsed.next_after(now).ok_or("That cron schedule never matches.")?;
        return Ok((When::Cron { cron }, Some(first)));
    }
    if let Some(every) = text("every")? {
        let every = parse_duration(&every)?;
        if every < MIN_TIME {
            return Err(format!("every must be at least 1m; got {}. For faster reactions use watch or files.", duration(every)));
        }
        let first = match text("start")? {
            Some(start) => parse_time(&start, now)?,
            None => now + every,
        };
        return Ok((When::Every { every }, Some(first)));
    }
    let at = match (text("at")?, text("in")?) {
        (Some(at), None) => parse_time(&at, now)?,
        (None, Some(after)) => now + parse_duration(&after)?,
        (Some(_), Some(_)) => return Err("Give at or in, not both.".to_owned()),
        (None, None) => return Err("when needs one of: at, in, every, cron, check, files or watch.".to_owned()),
    };
    if at < now + MIN_TIME {
        return Err(format!("at must be at least 1m from now; got {}.", local(at, now)));
    }
    Ok((When::At { at }, Some(at)))
}

/// "45m", "24h", "1h30m", "30s", "2d", "1w" in ms.
pub(crate) fn parse_duration(text: &str) -> Result<i64, String> {
    let bad = || format!("\"{text}\" isn't a duration; write e.g. 30s, 45m, 24h, 1h30m or 2d.");
    let mut total: i64 = 0;
    let mut number = String::new();
    for character in text.trim().chars() {
        if character.is_ascii_digit() {
            number.push(character);
            continue;
        }
        let unit = match character {
            's' => SECOND,
            'm' => MINUTE,
            'h' => HOUR,
            'd' => DAY,
            'w' => 7 * DAY,
            ' ' if number.is_empty() => continue,
            _ => return Err(bad()),
        };
        let value: i64 = number.parse().map_err(|_| bad())?;
        total = total.checked_add(value.checked_mul(unit).ok_or_else(bad)?).ok_or_else(bad)?;
        number.clear();
    }
    if !number.is_empty() || total <= 0 {
        return Err(bad());
    }
    Ok(total)
}

/// "24h", "1h30m", "45s".
pub(crate) fn duration(ms: i64) -> String {
    let mut seconds = ms.max(0) / SECOND;
    if seconds == 0 {
        return "0s".to_owned();
    }
    let mut text = String::new();
    for (unit, size) in [("d", 86_400), ("h", 3600), ("m", 60), ("s", 1)] {
        if seconds >= size {
            text.push_str(&format!("{}{unit}", seconds / size));
            seconds %= size;
        }
    }
    text
}

fn zone() -> TimeZone {
    TimeZone::system()
}

fn zoned(ms: i64) -> Option<ZonedDateTime> {
    Some(Timestamp::from_millisecond(ms).ok()?.to_zoned_datetime(zone()))
}

/// A time the agent wrote: "2026-11-19 09:00" (local), an RFC 3339 time with
/// its offset, or "09:00", the next one.
pub(crate) fn parse_time(text: &str, now: i64) -> Result<i64, String> {
    let text = text.trim();
    if let Ok(timestamp) = text.parse::<Timestamp>() {
        return Ok(timestamp.as_millisecond());
    }
    let bad = || format!("\"{text}\" isn't a time; write e.g. \"2026-11-19 09:00\" (local time) or \"09:00\".");
    let datetime = match text.parse::<DateTime>() {
        Ok(datetime) => datetime,
        Err(_) => {
            let time: jiff::civil::Time = text.parse().map_err(|_| bad())?;
            let today = zoned(now).ok_or_else(bad)?.date();
            let at = today.to_datetime(time);
            let at_ms = at.to_zoned_datetime(zone()).map_err(|_| bad())?.timestamp().as_millisecond();
            if at_ms > now { at } else { today.tomorrow().map_err(|_| bad())?.to_datetime(time) }
        }
    };
    Ok(datetime.to_zoned_datetime(zone()).map_err(|_| bad())?.timestamp().as_millisecond())
}

/// A time as the agent and the person read it: "14:10" today, "tomorrow
/// 09:12", "11-21 09:00", or with the year when it isn't this one.
pub(crate) fn local(ms: i64, now: i64) -> String {
    let (Some(at), Some(today)) = (zoned(ms), zoned(now)) else { return ms.to_string() };
    let (day, today) = (at.date(), today.date());
    if day == today {
        at.strftime("%H:%M").to_string()
    } else if today.tomorrow().is_ok_and(|tomorrow| tomorrow == day) {
        at.strftime("tomorrow %H:%M").to_string()
    } else if day.year() == today.year() {
        at.strftime("%m-%d %H:%M").to_string()
    } else {
        at.strftime("%Y-%m-%d %H:%M").to_string()
    }
}

/// A 5-field cron schedule (minute, hour, day of month, month, day of week),
/// as vixie cron reads it: when both days are restricted, either matches.
#[derive(Debug, PartialEq)]
pub(crate) struct Cron {
    minutes: Vec<bool>,
    hours: Vec<bool>,
    days: Vec<bool>,
    months: Vec<bool>,
    weekdays: Vec<bool>,
    any_day: bool,
    any_weekday: bool,
}

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
const WEEKDAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

impl Cron {
    pub(crate) fn parse(text: &str) -> Result<Cron, String> {
        let fields: Vec<&str> = text.split_whitespace().collect();
        let [minute, hour, day, month, weekday] = fields.as_slice() else {
            return Err(format!("cron takes 5 fields (minute hour day month weekday), e.g. \"0 9 * * 1-5\"; got \"{text}\"."));
        };
        let mut weekdays = field(weekday, 0, 7, &WEEKDAYS, "weekday")?;
        // 7 is Sunday too.
        if weekdays[7] {
            weekdays[0] = true;
        }
        weekdays.truncate(7);
        Ok(Cron {
            minutes: field(minute, 0, 59, &[], "minute")?,
            hours: field(hour, 0, 23, &[], "hour")?,
            days: field(day, 1, 31, &[], "day")?,
            months: field(month, 1, 12, &MONTHS, "month")?,
            weekdays,
            any_day: day.starts_with('*'),
            any_weekday: weekday.starts_with('*'),
        })
    }

    fn day_matches(&self, date: Date) -> bool {
        let day = self.days[date.day() as usize];
        let weekday = self.weekdays[date.weekday().to_sunday_zero_offset() as usize];
        match (self.any_day, self.any_weekday) {
            (false, false) => day || weekday,
            _ => day && weekday,
        }
    }

    /// The first matching minute after `after` (ms), within five years.
    pub(crate) fn next_after(&self, after: i64) -> Option<i64> {
        let start = zoned(after)?.datetime();
        // The next whole minute.
        let mut at = start
            .date()
            .to_datetime(jiff::civil::time(start.hour(), start.minute(), 0, 0))
            .checked_add(jiff::Span::new().minutes(1))
            .ok()?;
        let limit = start.date().year() + 5;
        while at.date().year() <= limit {
            let date = at.date();
            if !self.months[date.month() as usize] {
                let next_month = date.first_of_month().checked_add(jiff::Span::new().months(1)).ok()?;
                at = next_month.to_datetime(jiff::civil::Time::midnight());
                continue;
            }
            if !self.day_matches(date) {
                at = date.tomorrow().ok()?.to_datetime(jiff::civil::Time::midnight());
                continue;
            }
            if !self.hours[at.hour() as usize] {
                at = date.to_datetime(jiff::civil::time(at.hour(), 0, 0, 0)).checked_add(jiff::Span::new().hours(1)).ok()?;
                continue;
            }
            if !self.minutes[at.minute() as usize] {
                at = at.checked_add(jiff::Span::new().minutes(1)).ok()?;
                continue;
            }
            // A local time skipped by a clock change runs at the time after the gap.
            let ms = at.to_zoned_datetime(zone()).ok()?.timestamp().as_millisecond();
            if ms > after {
                return Some(ms);
            }
            at = at.checked_add(jiff::Span::new().minutes(1)).ok()?;
        }
        None
    }
}

/// One field: "*", "*/15", "1-5", "mon-fri", "0,30", "9-17/2".
fn field(text: &str, low: u32, high: u32, names: &[&str], what: &str) -> Result<Vec<bool>, String> {
    let bad = || format!("cron's {what} field \"{text}\" isn't valid; use numbers {low}-{high}, *, ranges a-b, lists a,b and steps */n.");
    let value = |part: &str| -> Result<u32, String> {
        let lower = part.to_ascii_lowercase();
        if let Some(index) = names.iter().position(|name| *name == lower) {
            // Months are named from 1, weekdays from 0.
            return Ok(index as u32 + low);
        }
        let number: u32 = part.parse().map_err(|_| bad())?;
        if number < low || number > high { Err(bad()) } else { Ok(number) }
    };
    let mut set = vec![false; high as usize + 1];
    for part in text.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => (range, step.parse::<u32>().ok().filter(|step| *step > 0).ok_or_else(bad)?),
            None => (part, 1),
        };
        let (first, last) = if range == "*" {
            (low, high)
        } else if let Some((first, last)) = range.split_once('-') {
            (value(first)?, value(last)?)
        } else {
            let single = value(range)?;
            (single, if part.contains('/') { high } else { single })
        };
        if first > last {
            return Err(bad());
        }
        for number in (first..=last).step_by(step as usize) {
            set[number as usize] = true;
        }
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> i64 {
        parse_time(text, 0).expect("time")
    }

    #[test]
    fn durations_read_and_print_alike() {
        assert_eq!(parse_duration("45m"), Ok(45 * MINUTE));
        assert_eq!(parse_duration("1h30m"), Ok(90 * MINUTE));
        assert_eq!(parse_duration("2d"), Ok(2 * DAY));
        assert!(parse_duration("90").is_err());
        assert!(parse_duration("soon").is_err());
        assert!(parse_duration("0m").is_err());
        assert_eq!(duration(90 * MINUTE), "1h30m");
        assert_eq!(duration(DAY), "1d");
    }

    #[test]
    fn kinds_are_read_from_their_keys() {
        let folder = Path::new("/work");
        let now = at("2026-11-19 08:00");
        let read = |json: Value| parse(&json, folder, now);
        assert_eq!(read(serde_json::json!({"every": "24h"})), Ok((When::Every { every: DAY }, Some(now + DAY))));
        assert_eq!(read(serde_json::json!({"in": "45m"})), Ok((When::At { at: now + 45 * MINUTE }, Some(now + 45 * MINUTE))));
        let check = read(serde_json::json!({"check": "gh run view", "every": "5m", "match": "success"})).expect("check");
        assert_eq!(
            check,
            (
                When::Check {
                    command: "gh run view".into(),
                    every: 5 * MINUTE,
                    fire_on: FireOn::Match,
                    pattern: Some("success".into()),
                    repeat: Repeat::Once,
                    secrets: Vec::new()
                },
                Some(now)
            )
        );
        assert_eq!(
            read(serde_json::json!({"files": ["src/", "/abs/report.csv"]})),
            Ok((When::Files { paths: vec!["/work/src/".into(), "/abs/report.csv".into()], glob: None }, None))
        );
        assert_eq!(
            read(serde_json::json!({"watch": "tail -F app.log"})),
            Ok((When::Watch { command: "tail -F app.log".into(), pattern: None, gap: 30 * SECOND }, None))
        );
        assert_eq!(
            read(serde_json::json!({"cron": "0 9 * * *"})),
            Ok((When::Cron { cron: "0 9 * * *".into() }, Some(at("2026-11-19 09:00"))))
        );
    }

    #[test]
    fn errors_say_what_would_work() {
        let read = |json: Value| parse(&json, Path::new("/"), 0).expect_err("refused");
        assert_eq!(
            read(serde_json::json!({"every": "20s"})),
            "every must be at least 1m; got 20s. For faster reactions use watch or files."
        );
        assert!(read(serde_json::json!({"check": "x", "every": "10s"})).contains("at least 30s"));
        assert!(read(serde_json::json!({"check": "x", "match": "("})).contains("regex"));
        assert!(read(serde_json::json!({"cron": "0 9 * *"})).contains("5 fields"));
        assert!(read(serde_json::json!({"cron": "61 * * * *"})).contains("minute"));
        assert!(read(serde_json::json!({"in": "30s"})).contains("at least 1m"));
        assert!(read(serde_json::json!({})).contains("one of"));
    }

    #[test]
    fn a_check_names_secrets_once_each_and_only_a_check_can() {
        let read = |json: Value| parse(&json, Path::new("/"), 0);
        let (check, _) = read(serde_json::json!({"check": "./export.sh", "secrets": [" Export ", "Export", "Grafana"]})).expect("check");
        let When::Check { secrets, .. } = &check else { panic!("a check") };
        assert_eq!(secrets, &["Export".to_owned(), "Grafana".to_owned()]);
        assert_eq!(check.describe(0), "check `./export.sh` every 5m, fires on success, with the secrets Export, Grafana");
        let (one, _) = read(serde_json::json!({"check": "x", "secrets": "Export"})).expect("one");
        assert!(one.describe(0).ends_with("fires on success, with the secret Export"));
        // None named: kept as before, so an older Claktory reads it as it was.
        let (plain, _) = read(serde_json::json!({"check": "x", "secrets": []})).expect("plain");
        assert_eq!(serde_json::to_string(&plain).unwrap(), r#"{"kind":"check","command":"x","every":300000,"fire_on":"success","repeat":"once"}"#);
        assert_eq!(serde_json::from_str::<When>(r#"{"kind":"check","command":"x","every":300000,"fire_on":"success"}"#).unwrap(), plain);

        let refused = |json: Value| read(json).expect_err("refused");
        assert!(refused(serde_json::json!({"watch": "tail -F log", "secrets": ["Export"]})).starts_with("secrets works only with check"));
        assert!(refused(serde_json::json!({"every": "1h", "secrets": "Export"})).starts_with("secrets works only with check"));
        assert!(refused(serde_json::json!({"check": "x", "secrets": [""]})).contains("a secret's name or a list of names"));
        assert!(refused(serde_json::json!({"check": "x", "secrets": [1]})).contains("a secret's name or a list of names"));
        let many: Vec<String> = (0..11).map(|n| format!("S{n}")).collect();
        assert!(refused(serde_json::json!({"check": "x", "secrets": many})).contains("at most 10 secrets"));
    }

    #[test]
    fn cron_finds_the_next_matching_minute() {
        let weekdays = Cron::parse("0 9 * * 1-5").expect("cron");
        // Thursday 2026-11-19, 08:00 → 09:00 the same day; after that, Friday; after Friday's, Monday.
        assert_eq!(weekdays.next_after(at("2026-11-19 08:00")), Some(at("2026-11-19 09:00")));
        assert_eq!(weekdays.next_after(at("2026-11-19 09:00")), Some(at("2026-11-20 09:00")));
        assert_eq!(weekdays.next_after(at("2026-11-20 09:00")), Some(at("2026-11-23 09:00")));
        let quarter = Cron::parse("*/15 * * * *").expect("cron");
        assert_eq!(quarter.next_after(at("2026-11-19 08:07")), Some(at("2026-11-19 08:15")));
        // Day of month or day of week, as vixie cron reads both restricted.
        let either = Cron::parse("0 0 1 * sun").expect("cron");
        assert_eq!(either.next_after(at("2026-11-19 08:00")), Some(at("2026-11-22 00:00")));
        let leap = Cron::parse("0 12 29 feb *").expect("cron");
        assert_eq!(leap.next_after(at("2026-11-19 08:00")), Some(at("2028-02-29 12:00")));
        assert_eq!(Cron::parse("0 0 31 feb *").expect("cron").next_after(0), None);
    }
}
