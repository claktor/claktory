//! Reports of AI replies (API.md's `POST /v1/reports/ai`, Report this
//! reply): a person sends one of their agents' replies to Claktory's team,
//! kept 30 days for the team to read on the server and never through any
//! route. From a signed-in Claktory, signed, limited by its key; from one
//! that isn't, unsigned, limited by its address. The report itself holds
//! only what the person's sheet showed: no account, Claktory, key or
//! address, so a signed one is the same as an unsigned one. Nothing here
//! sends email or reaches anyone, so the route can't relay spam.

use super::api::authenticate;
use super::{ApiError, Call, Reply, Service, db};
use crate::protocol::{AI_REPORT_KEEP_DAYS, AI_REPORT_TEXT_MAX, AiReport, AiReportKept, valid_ai_report};

/// The largest body taken: the reply's 16 KB twice over, since JSON writes
/// a line break or a quote in two bytes (code is full of them), and the
/// note (1000 characters of up to 4 bytes) beside it.
pub const BODY_MAX: usize = 2 * AI_REPORT_TEXT_MAX + 8 * 1024;
/// Reports an address may send unsigned: 10 an hour, 50 a day.
pub const PER_IP_HOUR: u32 = 10;
pub const PER_IP_DAY: u32 = 50;
/// Reports a signed-in Claktory may send a day.
pub const PER_KEY_DAY: u32 = 20;
/// Reports the service takes a day, from everyone together.
pub const PER_DAY: u32 = 2000;

pub(super) fn report(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    if call.body.len() > BODY_MAX {
        return Err(ApiError::invalid("The report is too big."));
    }
    // Who sent it counts only toward the limits, by a hash, and isn't kept
    // with the report.
    let mut limits = vec![("ai-report-day".to_owned(), 86_400, PER_DAY)];
    if call.header("authorization").is_some() {
        let claktory = authenticate(service, call)?;
        limits.push((db::email_key("ai-report-key-day", &claktory.id), 86_400, PER_KEY_DAY));
    } else {
        limits.push((db::email_key("ai-report-ip-hour", &call.ip), 3600, PER_IP_HOUR));
        limits.push((db::email_key("ai-report-ip-day", &call.ip), 86_400, PER_IP_DAY));
    }
    let report: AiReport = call.json()?;
    if !valid_ai_report(&report) {
        return Err(ApiError::invalid("The report isn't one this service keeps."));
    }
    let limits: Vec<(&str, i64, u32)> = limits.iter().map(|(key, period, most)| (key.as_str(), *period, *most)).collect();
    service.limit(&limits)?;
    db::add_ai_report(&service.db(), &report, service.now())?;
    tracing::info!(model = report.model, version = report.version, "an AI reply was reported");
    Ok(Reply::json(&AiReportKept { keep_days: AI_REPORT_KEEP_DAYS }))
}

/// A kept report, as `claktory-account ai-reports` reads it.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Kept {
    pub id: i64,
    /// When it arrived, to the hour (unix seconds).
    pub received: i64,
    pub model: String,
    pub version: String,
    pub platform: String,
    pub note: String,
    pub text: String,
}

/// The reports kept now, oldest first: those after `after` (an id), at most
/// `most`, and whether more follow. One past its 30 days that the daily
/// purge hasn't reached yet is left out, as it's no longer kept.
pub fn kept(conn: &rusqlite::Connection, now: i64, after: i64, most: usize) -> rusqlite::Result<(Vec<Kept>, bool)> {
    let mut statement = conn.prepare(
        "SELECT id, received, model, version, platform, note, text FROM ai_reports
         WHERE id > ?1 AND received >= ?2 ORDER BY id LIMIT ?3",
    )?;
    let oldest = now - AI_REPORT_KEEP_DAYS * 86_400;
    let limit = i64::try_from(most.saturating_add(1)).unwrap_or(i64::MAX);
    let mut reports = statement
        .query_map(rusqlite::params![after, oldest, limit], |row| {
            Ok(Kept {
                id: row.get(0)?,
                received: row.get(1)?,
                model: row.get(2)?,
                version: row.get(3)?,
                platform: row.get(4)?,
                note: row.get(5)?,
                text: row.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let more = reports.len() > most;
    reports.truncate(most);
    Ok((reports, more))
}

/// `reports` for a terminal. Anyone can send a report, so nothing in one
/// reaches the terminal as it is: a control or invisible format character
/// (an escape sequence, a bidi override) shows as its code, and every line
/// of a reply or note starts with a bar, so none can pass for a heading.
pub fn text(reports: &[Kept], more: bool) -> String {
    let mut out = String::new();
    for report in reports {
        let received = jiff::Timestamp::from_second(report.received)
            .map(|time| time.strftime("%Y-%m-%d %H:00 UTC").to_string())
            .unwrap_or_default();
        out.push_str(&format!(
            "#{} {received} · {} · {} · {}\n",
            report.id,
            shown(&report.model),
            shown(&report.version),
            shown(&report.platform)
        ));
        if !report.note.is_empty() {
            out.push_str("Note:\n");
            barred(&mut out, &report.note);
        }
        out.push_str("Reply:\n");
        barred(&mut out, &report.text);
        out.push('\n');
    }
    match (reports.last(), more) {
        (Some(last), true) => out.push_str(&format!("More follow: --after {}\n", last.id)),
        (None, _) => out.push_str("No reports.\n"),
        (Some(_), false) => {}
    }
    out
}

fn barred(out: &mut String, text: &str) {
    for line in text.split('\n') {
        out.push_str("  │ ");
        out.push_str(&shown(line));
        out.push('\n');
    }
}

fn shown(text: &str) -> String {
    let mut shown = String::with_capacity(text.len());
    for character in text.chars() {
        if character == '\t' || !(character.is_control() || crate::protocol::format_character(character)) {
            shown.push(character);
        } else {
            shown.push_str(&format!("\\u{{{:04X}}}", u32::from(character)));
        }
    }
    shown
}
