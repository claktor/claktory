//! Report this reply (store launch: Google Play's AI-generated content
//! policy): one of an agent's or a chat's replies, with the person's note,
//! sent to Claktory's team at the account service (`POST /v1/reports/ai`),
//! which keeps it 30 days.
//!
//! The app names the reply (whose conversation, which entry) and never
//! sends its text: this reads it here, so only a finished assistant reply
//! can be sent as one. A preview (`send` false) answers with exactly what
//! a send would carry, which the app shows before the person's Send; the
//! send recomputes it the same way. Signed in, the report goes signed, so
//! the service can limit it per key; signed out, unsigned, limited per
//! address. Either way it carries the reply, the model's name, this
//! Claktory's version and platform, and the note: no account, agent,
//! project, path or device.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use claktory_account::protocol::{self as api, AiReport, AiReportKept};
use factory_protocol::v1::{self, ErrorCode, entry::Kind};

use super::{Failure, anchor, configured_service, refused, service_name, try_again};
use crate::factory::{Factory, RequestError};

/// Reports this Claktory sends in an hour, at most, whatever the service
/// allows.
const PER_HOUR: usize = 10;
const HOUR: Duration = Duration::from_secs(3600);

/// When the last reports were sent, for `PER_HOUR`.
#[derive(Default)]
pub(crate) struct Sent(std::sync::Mutex<VecDeque<Instant>>);

impl Sent {
    /// Counts one now, unless `PER_HOUR` were sent in the last hour: then
    /// how long until one can be.
    fn take(&self) -> Result<(), Duration> {
        let mut sent = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        while sent.front().is_some_and(|at| now.duration_since(*at) >= HOUR) {
            sent.pop_front();
        }
        if sent.len() >= PER_HOUR {
            return Err(HOUR.saturating_sub(now.duration_since(sent[0])));
        }
        sent.push_back(now);
        Ok(())
    }
}

fn not_found(what: impl Into<String>) -> RequestError {
    refused(ErrorCode::NotFound, what)
}

fn invalid(why: impl Into<String>) -> RequestError {
    refused(ErrorCode::InvalidRequest, why)
}

/// ReportReply: what a report of the reply carries, sent when `send`.
pub(crate) async fn report(factory: &Arc<Factory>, request: v1::ReportReply) -> Result<v1::ReplyReport, RequestError> {
    let id = match (request.agent_id.as_str(), request.chat_id.as_str()) {
        (agent, "") if !agent.is_empty() => {
            factory.known_agent(agent)?;
            agent
        }
        ("", chat) if !chat.is_empty() => {
            if !factory.chats.exists(chat) {
                return Err(not_found(format!("No chat \"{chat}\".")));
            }
            chat
        }
        _ => return Err(invalid("Name the agent or the chat whose reply to report.")),
    };
    factory.ensure_loaded(id).await?;
    let entry = crate::transcript::entries_of(factory, id, &[request.seq]).await.remove(&request.seq);
    let (text, model) = {
        let inner = factory.lock();
        let transcript = inner.agents.get(id).and_then(|runtime| runtime.transcript.as_ref());
        let Some(entry) = entry else {
            return Err(not_found("That reply isn't in the conversation."));
        };
        let Some(Kind::Assistant(reply)) = &entry.kind else {
            return Err(invalid("Only replies can be reported."));
        };
        let Some(model) = transcript.and_then(|transcript| transcript.response_model(entry.reply)) else {
            return Err(invalid("This reply is still being written. Report it once it's finished."));
        };
        let name = inner.models.iter().find(|listed| listed.id == model).map(|listed| listed.name.clone());
        (reply.text.clone(), name.ok_or_else(|| model.to_owned()))
    };
    let model = match model {
        Ok(name) => name,
        Err(id) => factory.models.resolve(&id).map(|known| known.name).filter(|name| !name.is_empty()).unwrap_or(id),
    };
    // Locked, the vault's values aren't known, so none could be hidden.
    if factory.vault.exists() && !factory.vault.is_open() {
        return Err(invalid("Unlock your secrets first, so any secrets in this reply can be hidden before it's sent."));
    }
    // Hidden before it's cut, so a value the cut would split is still hidden.
    let hidden = factory.redaction.redact(&text);
    let (text, truncated) = api::ai_report_text(&hidden);
    if text.trim().is_empty() {
        return Err(invalid("This reply has no text to report."));
    }
    let note = api::ai_report_note(&request.note);
    if note.chars().count() > api::AI_REPORT_NOTE_MAX {
        return Err(invalid("Keep your note under 1,000 characters."));
    }
    let shown = v1::ReplyReport {
        text: text.to_owned(),
        model: model.chars().take(100).collect(),
        version: crate::update::VERSION.to_owned(),
        note,
        truncated,
    };
    if request.send {
        send(factory, &shown).await?;
        tracing::info!(agent = id, seq = request.seq, "a reply was reported");
    }
    Ok(shown)
}

/// Sends the report to the service, signed when signed in.
async fn send(factory: &Arc<Factory>, shown: &v1::ReplyReport) -> Result<(), RequestError> {
    let config = factory.lock().config.account.clone();
    if !config.enabled {
        return Err(invalid("Reports go to claktory.com, and this Claktory is set not to contact it."));
    }
    let account = &factory.account;
    let signed_in = account.signed_in_as(&config).map(|(_, _, service)| service);
    let service = signed_in.clone().unwrap_or_else(|| configured_service(&config));
    let name = service_name(&service);
    if let Err(wait) = account.reports.take() {
        let when = try_again(wait.as_secs() as i64, &jiff::ZonedDateTime::now());
        return Err(refused(ErrorCode::RateLimited, format!("You've sent a lot of reports. Try again {when}.")));
    }
    let report = AiReport {
        text: shown.text.clone(),
        model: shown.model.clone(),
        version: shown.version.clone(),
        platform: super::platform(),
        note: shown.note.clone(),
    };
    let kept: Result<AiReportKept, Failure> =
        if signed_in.is_some() { account.signed(api::AI_REPORT_PATH, &report).await } else { unsigned(account, &service, &report).await };
    kept.map(|_| ()).map_err(|failure| report_error(&name, failure))
}

/// An unsigned report to `service`, the answer checked with the key its
/// well-known address gives (vouched for, for claktory.com).
async fn unsigned(account: &super::State, service: &str, report: &AiReport) -> Result<AiReportKept, Failure> {
    let known = account.well_known(service).await?;
    if let anchor::Vouch::Refused(why) = anchor::vouch(service, &known) {
        return Err(Failure::Unreachable(format!("the service's key isn't vouched for: {why}")));
    }
    let key = api::verifying_key(&known.service_key).ok_or_else(|| Failure::Unreachable("a bad service key".to_owned()))?;
    account.call(service, Some(&key), "POST", api::AI_REPORT_PATH, report, None).await
}

/// A report's failure, in the app's words.
fn report_error(name: &str, failure: Failure) -> RequestError {
    let unreachable = || refused(ErrorCode::Unreachable, format!("Can't reach {name}. Check your internet connection and try again."));
    match failure {
        Failure::Unreachable(why) => {
            tracing::warn!(why, "a report couldn't reach the account service");
            unreachable()
        }
        Failure::OtherKey => {
            tracing::warn!("an answer to a report wasn't signed by the service's key");
            unreachable()
        }
        Failure::Refused(status, body) => {
            tracing::warn!(status, error = body.error, "the account service refused a report");
            match body.error.as_str() {
                "rate_limited" => {
                    let when = try_again(body.retry_after.unwrap_or(3600), &jiff::ZonedDateTime::now());
                    refused(ErrorCode::RateLimited, format!("{name} is taking too many reports right now. Try again {when}."))
                }
                "internal" | "not_found" => {
                    refused(ErrorCode::Unreachable, format!("{name} can't take reports right now. Try again later."))
                }
                _ => invalid(format!("{name} couldn't take this report. Try again later.")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_an_hour() {
        let sent = Sent::default();
        for _ in 0..PER_HOUR {
            assert!(sent.take().is_ok());
        }
        let wait = sent.take().unwrap_err();
        assert!(wait > HOUR - Duration::from_secs(60) && wait <= HOUR);
    }
}
