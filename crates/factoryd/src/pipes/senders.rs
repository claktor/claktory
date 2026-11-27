//! Members that only send: an admin agent such as Rivet lets an
//! agent send to a pipe the person approved without reading it, or stops it,
//! with pipes_sender (`extensions::pipes`). The person chooses the same in
//! the pipe's settings (UpdatePipe.send_only); an admin agent only adds an
//! agent that isn't one of the pipe's to it this way, or takes one that only
//! sends out, never touching members that read it. Each change it makes is
//! kept in `pipe_member_changes` for Needs you's history.

use std::sync::Arc;

use rusqlite::params;

use super::handoff::label;
use super::{Record, State, change, checked_members_sending, line};
use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;

pub(crate) const SCHEMA: &str = "
-- Agents an admin agent let only send to a pipe, or stopped (#414), for
-- Needs you's history.
CREATE TABLE IF NOT EXISTS pipe_member_changes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    pipe TEXT NOT NULL,
    -- The agent let send or stopped, and the admin agent that did it.
    agent TEXT NOT NULL,
    by TEXT NOT NULL,
    -- 1: let it send; 0: stopped it.
    sends INTEGER NOT NULL,
    at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS pipe_member_changes_by_time ON pipe_member_changes (at);
";

/// A row of pipe_member_changes.
pub(crate) struct MemberChange {
    pub id: i64,
    pub pipe: String,
    pub agent: String,
    pub by: String,
    pub sends: bool,
    pub at: i64,
}

impl MemberChange {
    /// Claktory's sentence for it, with agents' names as they are now.
    pub(crate) fn title(&self, names: &std::collections::HashMap<String, String>) -> String {
        let name = |id: &str| names.get(id).cloned().unwrap_or_else(|| id.to_owned());
        sentence(&name(&self.by), &name(&self.agent), self.by == self.agent, self.sends)
    }
}

/// "Rivet let Mike send to this pipe without reading it", "Rivet stopped
/// sending to this pipe" (`itself`: `by` is the agent).
fn sentence(by: &str, agent: &str, itself: bool, sends: bool) -> String {
    match (sends, itself) {
        (true, true) => format!("{by} let itself send to this pipe without reading it"),
        (true, false) => format!("{by} let {agent} send to this pipe without reading it"),
        (false, true) => format!("{by} stopped sending to this pipe"),
        (false, false) => format!("{by} stopped {agent} sending to this pipe"),
    }
}

/// The changes made from `since` to before `before`, newest first, at most
/// `limit`.
pub(crate) fn between(db: &rusqlite::Connection, since: i64, before: i64, limit: i64) -> rusqlite::Result<Vec<MemberChange>> {
    let mut statement = db.prepare(
        "SELECT id, pipe, agent, by, sends, at FROM pipe_member_changes WHERE at >= ?1 AND at < ?2 ORDER BY at DESC, id DESC LIMIT ?3",
    )?;
    statement
        .query_map(params![since, before, limit], |row| {
            Ok(MemberChange {
                id: row.get(0)?,
                pipe: row.get(1)?,
                agent: row.get(2)?,
                by: row.get(3)?,
                sends: row.get(4)?,
                at: row.get(5)?,
            })
        })?
        .collect()
}

/// `by` (an admin agent, checked by its caller) lets `agent` only send to
/// the pipe (`send`), or stops it: the pipe must be one the person
/// approved and not closed. Only an agent that isn't one of the pipe's is
/// added, and only one that only sends is taken out. What it says it did.
pub(crate) fn set(factory: &Arc<Factory>, by: &str, pipe: &str, agent: &str, send: bool) -> Result<String, String> {
    let record = factory.pipes.get(pipe).ok_or_else(|| "That pipe is gone.".to_owned())?;
    if !record.approved || record.state == State::Closed || !record.granted() {
        return Err(format!(
            "The pipe {} isn't one the person approved, or it's closed: agents use a pipe only once they approve it.",
            label(&record).trim_start_matches("pipe ")
        ));
    }
    let (name, by_name) = {
        let inner = factory.lock();
        let name = inner.config.agent(agent).map(|agent| agent.name.clone()).ok_or_else(|| format!("There's no agent \"{agent}\"."))?;
        let by_name = inner.config.agent(by).map_or_else(|| by.to_owned(), |agent| agent.name.clone());
        (name, by_name)
    };
    if send {
        // Any agent but a curator, in the pipe's project or not.
        checked_members_sending(factory, &record, &[agent.to_owned()], &[agent.to_owned()]).map_err(|error| refused(&error))?;
        // One that reads another pipe uses no other.
        let members = [agent.to_owned()];
        let roles = super::sealed::Roles { members: &members, lead: "", bound: "" };
        super::sealed::check(factory, pipe, &roles).map_err(|error| refused(&error))?;
    }
    let pipe_called = called(&record);
    // Checked again as it changes, so nothing in between slips past.
    let changed = change(factory, pipe, |record| {
        let member = record.members.iter().any(|id| id == agent);
        match (send, member, record.sends_only(agent)) {
            (true, true, true) => Err(format!("{name} already only sends to {pipe_called}.")),
            (true, true, false) => Err(format!(
                "{name} is already one of the agents of {pipe_called}, reading it: only the person changes that, in the pipe's settings."
            )),
            (true, false, _) => {
                record.members.push(agent.to_owned());
                record.send_only.push(agent.to_owned());
                Ok(())
            }
            (false, false, _) => Err(format!("{name} isn't one of the agents of {pipe_called}.")),
            (false, true, false) => {
                Err(format!("{name} reads {pipe_called}, so it doesn't only send: only the person takes it out, in the pipe's settings."))
            }
            (false, true, true) => {
                record.members.retain(|id| id != agent);
                record.send_only.retain(|id| id != agent);
                Ok(())
            }
        }
    });
    match changed {
        None => return Err("That pipe is gone.".to_owned()),
        Some(Err(error)) => return Err(error),
        Some(Ok(())) => {}
    }
    let now = now_ms();
    if let Err(error) = factory.store.db().execute(
        "INSERT INTO pipe_member_changes (pipe, agent, by, sends, at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![pipe, agent, by, send, now],
    ) {
        tracing::error!(pipe, agent, %error, "could not keep a change of who sends to a pipe");
    }
    line(factory, pipe, format!("{}.", sentence(&by_name, &name, agent == by, send)));
    factory.broadcast_workspace(&factory.lock());
    Ok(match (send, agent == by) {
        (true, true) => format!(
            "You now only send to {pipe_called}, with pipe_send (through extension_call until your conversation is next summarized or cleared): you can't read it, and you aren't told what the other side says. The person sees this in Needs you's history."
        ),
        (true, false) => format!(
            "{name} now only sends to {pipe_called}: it can't read it, and isn't told what the other side says. It's told it can send. The person sees this in Needs you's history."
        ),
        (false, true) => format!("You no longer send to {pipe_called}. The person sees this in Needs you's history."),
        (false, false) => format!("{name} no longer sends to {pipe_called}. The person sees this in Needs you's history."),
    })
}

/// The pipe as an admin agent reads it: the person's name for it, else its id.
fn called(record: &Record) -> String {
    format!("the pipe {}", label(record).trim_start_matches("pipe "))
}

/// A refusal of the member check, as the agent reads it.
fn refused(error: &RequestError) -> String {
    match error {
        RequestError::Pipe(text) => text.clone(),
        other => other.to_string(),
    }
}
