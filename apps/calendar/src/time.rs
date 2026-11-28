//! Dates and the Mac's clock. Every time is kept as UTC milliseconds; the
//! Mac's local time (where cron schedules and repeats run, and how agents
//! write times) comes from SQLite's `localtime` in factoryd, sampled each
//! hour over the range a view or a tool needs ([`Zone`]), daylight saving
//! included. Devices draw in their own zone; all-day dates go to them as
//! UTC midnights, read as dates.

use std::fmt::Write as _;

pub const MINUTE: i64 = 60_000;
pub const HOUR: i64 = 60 * MINUTE;
pub const DAY: i64 = 24 * HOUR;

/// Days since 1970-01-01 of a civil date.
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let month = i64::from(month);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date of a count of days since 1970-01-01.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 }, month, day)
}

pub fn days_in_month(year: i64, month: u32) -> u32 {
    let next = if month == 12 { days_from_civil(year + 1, 1, 1) } else { days_from_civil(year, month + 1, 1) };
    (next - days_from_civil(year, month, 1)) as u32
}

/// The weekday of a day since 1970, Monday 0 … Sunday 6.
pub fn weekday(days: i64) -> u32 {
    (days + 3).rem_euclid(7) as u32
}

pub const WEEKDAYS: [&str; 7] = ["mo", "tu", "we", "th", "fr", "sa", "su"];
pub const WEEKDAY_NAMES: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

/// "2026-12-03".
pub fn date_text(days: i64) -> String {
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

pub const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// "11 Dec".
pub fn short_date(days: i64) -> String {
    let (_, month, day) = civil_from_days(days);
    format!("{day} {}", MONTHS[month as usize - 1])
}

/// The Mac's UTC offset over a stretch of time, sampled each hour.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Zone {
    /// (UTC ms, offset in minutes), in order. Empty reads as UTC.
    samples: Vec<(i64, i32)>,
}

/// The query that samples the Mac's offset each hour from ?1 to ?2 (Unix
/// seconds).
pub const ZONE_SQL: &str = "WITH RECURSIVE h(t) AS (SELECT ?1 UNION ALL SELECT t + 3600 FROM h WHERE t + 3600 <= ?2) \
SELECT t, CAST(ROUND((julianday(t, 'unixepoch', 'localtime') - julianday(t, 'unixepoch')) * 1440) AS INTEGER) FROM h";

impl Zone {
    pub fn fixed(offset_minutes: i32) -> Self {
        Self { samples: vec![(i64::MIN, offset_minutes)] }
    }

    /// From [`ZONE_SQL`]'s answer.
    pub fn from_rows(rows: &[Vec<serde_json::Value>]) -> Self {
        let mut samples: Vec<(i64, i32)> =
            rows.iter().filter_map(|row| Some((row.first()?.as_i64()? * 1000, i32::try_from(row.get(1)?.as_i64()?).ok()?))).collect();
        samples.sort_unstable();
        Self { samples }
    }

    /// The bounds [`ZONE_SQL`] takes to cover `from`..`to` (ms), a day of room each side.
    pub fn bounds(from: i64, to: i64) -> (i64, i64) {
        ((from - DAY).div_euclid(HOUR) * 3600, (to + DAY).div_euclid(HOUR) * 3600 + 3600)
    }

    pub fn is_known(&self) -> bool {
        !self.samples.is_empty()
    }

    /// The offset, in minutes, at `utc`.
    pub fn offset_at(&self, utc: i64) -> i32 {
        let after = self.samples.partition_point(|(at, _)| *at <= utc);
        match after {
            0 => self.samples.first().map_or(0, |(_, offset)| *offset),
            n => self.samples[n - 1].1,
        }
    }

    /// The Mac's wall clock at `utc`, as ms since 1970 read as local.
    pub fn local(&self, utc: i64) -> i64 {
        utc + i64::from(self.offset_at(utc)) * MINUTE
    }

    /// The instant the Mac's clock reads `local`: the earlier one when it
    /// reads it twice (the hour clocks go back), and as long after the gap
    /// when it never does (the hour they go forward), as Claktory's
    /// monitors read local times.
    pub fn utc(&self, local: i64) -> i64 {
        let around = [-14 * HOUR, 0, 14 * HOUR].map(|by| local - i64::from(self.offset_at(local + by)) * MINUTE);
        around.into_iter().filter(|at| self.local(*at) == local).min().unwrap_or(around[0])
    }

    /// The Mac's local day (days since 1970) holding `utc`.
    pub fn day(&self, utc: i64) -> i64 {
        self.local(utc).div_euclid(DAY)
    }

    /// When the Mac's local day `days` starts.
    pub fn day_start(&self, days: i64) -> i64 {
        self.utc(days * DAY)
    }

    /// "2026-12-03 14:00+01:00": how agents read a time.
    pub fn text(&self, utc: i64) -> String {
        let offset = self.offset_at(utc);
        let local = utc + i64::from(offset) * MINUTE;
        let days = local.div_euclid(DAY);
        let minutes = local.rem_euclid(DAY) / MINUTE;
        let mut out = format!("{} {:02}:{:02}", date_text(days), minutes / 60, minutes % 60);
        let sign = if offset < 0 { '-' } else { '+' };
        let _ = write!(out, "{sign}{:02}:{:02}", offset.abs() / 60, offset.abs() % 60);
        out
    }
}

/// A time as an agent writes it: ms since 1970, "now", "now+7d" or
/// "now-2h", a date ("2026-12-03", its local midnight), or a local time
/// ("2026-12-03 14:00", "2026-12-03T14:00"), with an offset or Z when it
/// isn't the Mac's ("2026-12-03 14:00+01:00").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Written {
    /// A point in time, already UTC.
    Instant(i64),
    /// A date alone (days since 1970).
    Date(i64),
    /// The Mac's wall clock, ms since 1970 read as local.
    Local(i64),
}

pub fn parse(text: &str, now: i64) -> Result<Written, String> {
    let text = text.trim();
    let bad = || format!("\"{text}\" isn't a time: write \"2026-12-03 14:00\" (the Mac's time), \"2026-12-03\" for a day, or \"now+2d\".");
    if let Ok(ms) = text.parse::<i64>() {
        return Ok(Written::Instant(ms));
    }
    if let Some(rest) = text.strip_prefix("now") {
        if rest.is_empty() {
            return Ok(Written::Instant(now));
        }
        let (sign, amount) = match rest.split_at(1) {
            ("+", amount) => (1, amount),
            ("-", amount) => (-1, amount),
            _ => return Err(bad()),
        };
        return span(amount).map(|ms| Written::Instant(now + sign * ms)).ok_or_else(bad);
    }
    let (date, rest) = text.split_at(text.len().min(10));
    let days = date_days(date).ok_or_else(bad)?;
    if rest.is_empty() {
        return Ok(Written::Date(days));
    }
    let rest = rest.strip_prefix(' ').or_else(|| rest.strip_prefix('T')).ok_or_else(bad)?;
    let (clock, zone) = match rest.find(['Z', 'z', '+', '-']) {
        Some(at) => rest.split_at(at),
        None => (rest, ""),
    };
    let mut parts = clock.split(':');
    let hours: i64 = parts.next().and_then(|part| part.parse().ok()).filter(|hours| (0..24).contains(hours)).ok_or_else(bad)?;
    let minutes: i64 = parts.next().and_then(|part| part.parse().ok()).filter(|minutes| (0..60).contains(minutes)).ok_or_else(bad)?;
    let seconds: f64 = match parts.next() {
        Some(part) => part.parse().ok().filter(|seconds| (0.0..61.0).contains(seconds)).ok_or_else(bad)?,
        None => 0.0,
    };
    if parts.next().is_some() {
        return Err(bad());
    }
    let local = days * DAY + hours * HOUR + minutes * MINUTE + (seconds * 1000.0) as i64;
    if zone.is_empty() {
        return Ok(Written::Local(local));
    }
    if zone.eq_ignore_ascii_case("z") {
        return Ok(Written::Instant(local));
    }
    let (sign, offset) = zone.split_at(1);
    let sign = if sign == "-" { -1 } else { 1 };
    let (offset_hours, offset_minutes) = offset.split_once(':').unwrap_or((offset, "0"));
    let offset_hours: i64 = offset_hours.parse().ok().filter(|hours| *hours <= 14).ok_or_else(bad)?;
    let offset_minutes: i64 = offset_minutes.parse().ok().filter(|minutes| *minutes < 60).ok_or_else(bad)?;
    Ok(Written::Instant(local - sign * (offset_hours * HOUR + offset_minutes * MINUTE)))
}

/// "2026-12-03" as days since 1970.
pub fn date_days(text: &str) -> Option<i64> {
    let mut parts = text.split('-');
    let year: i64 = parts.next()?.parse().ok().filter(|year| (1970..=9999).contains(year))?;
    let month: u32 = parts.next()?.parse().ok().filter(|month| (1..=12).contains(month))?;
    let day: u32 = parts.next()?.parse().ok().filter(|day| *day >= 1 && *day <= days_in_month(year, month))?;
    if parts.next().is_some() || text.len() != 10 {
        return None;
    }
    Some(days_from_civil(year, month, day))
}

/// "7d", "2h", "30m", "1w" as ms.
pub fn span(text: &str) -> Option<i64> {
    let unit = text.chars().last()?;
    let count: i64 = text[..text.len() - unit.len_utf8()].parse().ok().filter(|count| *count >= 0 && *count <= 100_000)?;
    Some(
        count
            * match unit {
                'm' => MINUTE,
                'h' => HOUR,
                'd' => DAY,
                'w' => 7 * DAY,
                _ => return None,
            },
    )
}

/// "every 6 hours", "every 30 min", "every day", "every 2 days".
pub fn every_text(ms: i64) -> String {
    let unit = |count: i64, one: &str, many: &str| if count == 1 { format!("every {one}") } else { format!("every {count} {many}") };
    if ms > 0 && ms % DAY == 0 {
        if ms % (7 * DAY) == 0 { unit(ms / (7 * DAY), "week", "weeks") } else { unit(ms / DAY, "day", "days") }
    } else if ms > 0 && ms % HOUR == 0 {
        unit(ms / HOUR, "hour", "hours")
    } else if ms >= MINUTE {
        unit(ms / MINUTE, "minute", "min")
    } else {
        format!("every {} s", ms / 1000)
    }
}

/// "1 h 10 min", "45 min", "2 days".
pub fn length_text(ms: i64) -> String {
    let minutes = ms / MINUTE;
    if minutes < 60 {
        format!("{} min", minutes.max(1))
    } else if minutes < 48 * 60 {
        match minutes % 60 {
            0 => format!("{} h", minutes / 60),
            rest => format!("{} h {rest} min", minutes / 60),
        }
    } else {
        format!("{} days", minutes / (24 * 60))
    }
}
