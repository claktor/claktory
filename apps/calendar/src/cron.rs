//! A 5-field cron schedule (minute, hour, day of month, month, day of
//! week), read as Claktory's monitors read it (vixie cron's rules: when
//! both day fields are restricted, either matches), and its fires in a
//! stretch of time, on the Mac's clock.

use crate::time::{self, DAY, HOUR, MINUTE, Zone};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
    minutes: Vec<bool>,
    hours: Vec<bool>,
    days: Vec<bool>,
    months: Vec<bool>,
    /// Sunday 0 … Saturday 6.
    weekdays: Vec<bool>,
    days_restricted: bool,
    weekdays_restricted: bool,
}

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
const DAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

fn field(text: &str, low: u32, high: u32, names: &[&str]) -> Option<Vec<bool>> {
    let value = |part: &str| -> Option<u32> {
        let lower = part.to_ascii_lowercase();
        if let Some(index) = names.iter().position(|name| *name == lower) {
            return Some(index as u32 + low);
        }
        part.parse().ok().filter(|number| (low..=high).contains(number))
    };
    let mut set = vec![false; high as usize + 1];
    for part in text.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => (range, step.parse::<u32>().ok().filter(|step| *step > 0)?),
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
            return None;
        }
        for number in (first..=last).step_by(step as usize) {
            set[number as usize] = true;
        }
    }
    Some(set)
}

impl Cron {
    pub fn parse(text: &str) -> Option<Cron> {
        let fields: Vec<&str> = text.split_whitespace().collect();
        let [minute, hour, day, month, weekday] = fields[..] else { return None };
        let mut weekdays = field(weekday, 0, 7, &DAYS)?;
        if weekdays[7] {
            weekdays[0] = true;
        }
        weekdays.truncate(7);
        Some(Cron {
            minutes: field(minute, 0, 59, &[])?,
            hours: field(hour, 0, 23, &[])?,
            days: field(day, 1, 31, &[])?,
            months: field(month, 1, 12, &MONTHS)?,
            weekdays,
            days_restricted: day != "*",
            weekdays_restricted: weekday != "*",
        })
    }

    fn on_day(&self, days: i64) -> bool {
        let (_, month, day) = time::civil_from_days(days);
        if !self.months[month as usize] {
            return false;
        }
        let by_date = self.days[day as usize];
        let by_weekday = self.weekdays[((time::weekday(days) + 1) % 7) as usize];
        match (self.days_restricted, self.weekdays_restricted) {
            (true, true) => by_date || by_weekday,
            (true, false) => by_date,
            (false, true) => by_weekday,
            (false, false) => true,
        }
    }

    /// Its fires from `from` up to `to` (UTC ms), at most `most`, in order.
    pub fn fires(&self, zone: &Zone, from: i64, to: i64, most: usize) -> Vec<i64> {
        let mut out = Vec::new();
        for days in zone.day(from)..=zone.day(to) {
            if !self.on_day(days) {
                continue;
            }
            for hour in (0..24).filter(|hour| self.hours[*hour]) {
                for minute in (0..60).filter(|minute| self.minutes[*minute]) {
                    let local = days * DAY + hour as i64 * HOUR + minute as i64 * MINUTE;
                    // A time a clock change skips runs just after the gap,
                    // as Claktory's monitors run it: once.
                    let at = zone.utc(local);
                    if at < from || at >= to || out.last().is_some_and(|last| *last >= at) {
                        continue;
                    }
                    out.push(at);
                    if out.len() >= most {
                        return out;
                    }
                }
            }
        }
        out
    }

    /// How it repeats, in words without a time of day (devices show times
    /// in their own zone): "every day", "weekdays", "every Monday",
    /// "every hour", "every 15 min"; else "on a schedule".
    pub fn words(&self) -> String {
        let count = |set: &[bool]| set.iter().filter(|on| **on).count();
        let all_dates = !self.days_restricted && count(&self.months[1..]) == 12;
        let times = count(&self.minutes) * count(&self.hours);
        if all_dates && times > 1 && count(&self.hours) == 24 && !self.weekdays_restricted {
            let minutes = count(&self.minutes);
            return if minutes == 1 {
                "every hour".into()
            } else if 60 % minutes == 0 && self.evenly(&self.minutes) {
                format!("every {} min", 60 / minutes)
            } else {
                "on a schedule".into()
            };
        }
        if !all_dates || times != 1 {
            return "on a schedule".into();
        }
        if !self.weekdays_restricted {
            return "every day".into();
        }
        let on: Vec<usize> = (0..7).filter(|day| self.weekdays[*day]).collect();
        match on[..] {
            [1, 2, 3, 4, 5] => "weekdays".into(),
            [0, 6] => "weekends".into(),
            [day] => format!("every {}", time::WEEKDAY_NAMES[(day + 6) % 7]),
            _ => "on a schedule".into(),
        }
    }

    fn evenly(&self, set: &[bool]) -> bool {
        let on: Vec<usize> = (0..set.len()).filter(|at| set[*at]).collect();
        on.windows(2).map(|pair| pair[1] - pair[0]).all(|gap| gap == 60 / on.len())
    }
}
