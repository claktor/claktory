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
