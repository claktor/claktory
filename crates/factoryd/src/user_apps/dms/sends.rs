//! An agent's message to one of the person's friends, sent only once the
//! person approves exactly it (D4b; dms.md §7, "Rivet path and
//! exact-content review"; end to end, dms-e2ee.md §6). The account service
//! keeps the proposal and its review (`claktory-user-apps`' `dms::review`),
//! but never its text; this side proposes it for the agent, follows what
//! becomes of it, and lists what waits in Needs you.
//!
//! - **Never a send from here.** dm_send encrypts the proposal into this
//!   Claktory's channel with the person's own devices (`leaf`), as content
//!   of kind "proposal" that names the conversation, its generation, the
//!   agent and a `sequence` that only goes up, and asks the service to keep
//!   it (`POST conversations/<id>/proposals`, for the agent, which needs a
//!   live `dm.send` grant on the conversation). Only the person's own device
//!   decrypts it, shows it, and on approval encrypts that exact text into
//!   the conversation and sends it itself: this Claktory never holds a
//!   device key, never makes a device's call and never passes an approval
//!   on. Until a device has given this Claktory its channel, dm_send says
//!   so and proposes nothing.
//! - **Kept sealed until decided.** A device added to the channel later
//!   can't open what was sent before it came, so a waiting proposal's text
//!   is kept in the leaf's sealed store, never in factory.db, and sent
//!   again as the proposal's next revision whenever a device joins
//!   (`reposted`). It's forgotten once the proposal ends.
//! - **Waiting.** A proposal waits up to 3 days. While it does, Needs you
//!   lists it (`v1::DmSendWaiting`): the agent, and the friend's name and
//!   tag, never the text. One poller follows every proposal waiting here,
//!   one call at a time (the service lets a Claktory hold only a few long
//!   polls), and tells the agent what the person decided.
//! - **Kept here:** ids, states, times and sequences (`dm_sends`), never
//!   the text.

use std::fmt::Write as _;
use std::sync::{Arc, Weak};
use std::time::Duration;

use factory_agent::ToolResult;
use factory_protocol::v1;
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use serde_json::json;

use claktory_dm_crypto::content::{Content, kind};
use claktory_dm_crypto::envelope::Envelope;

use super::leaf::{self, PendingProposal, StepError};
use super::{MAX_TEXT, conversations, one_line, out_of_reach, peer_of, refused, valid_id, who};
use crate::account::apps::{Actor, Problem};
use crate::factory::{Factory, host_event};
use crate::transcript::now_ms;

pub(crate) const SCHEMA: &str = "
-- Agents' messages proposed to the person's friends (dm_send), waiting for
-- the person's review or decided: ids, states and times, never the text.
CREATE TABLE dm_sends (
    -- The account service's proposal id.
    id TEXT PRIMARY KEY,
    agent TEXT NOT NULL,
    incarnation TEXT NOT NULL,
    conversation TEXT NOT NULL,
    -- The id it was proposed under, which a new text revises it under.
    client_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    -- pending, sent, declined, withdrawn, expired or closed
    state TEXT NOT NULL,
    created INTEGER NOT NULL,
    expires INTEGER NOT NULL,
    decided INTEGER
);
CREATE INDEX dm_sends_by_state ON dm_sends (state, agent);
";

/// End to end (dms-e2ee.md §6): the `sequence` of the proposal's latest
/// text as encrypted here, counting up from 1, so the person's device can
/// refuse an older one served as the latest. NULL in rows from before.
pub(crate) const SEQUENCES: &str = "
ALTER TABLE dm_sends ADD COLUMN sequence INTEGER;
";

pub(super) const SEND: &str = "Propose a message in a direct-message conversation, for the person to approve before it's sent: conversation, its id; text, the whole message; replaces, the id of a proposal of yours still waiting, to change its text instead of proposing another. Needs access to send there. Nothing is sent until the person approves exactly this text on their device; you're told when they decide. It waits up to 3 days. Never put secrets in a message.";
pub(super) const WITHDRAW: &str = "Take back a message you proposed with dm_send that still waits for the person: proposal, its id.";

/// How long the service may hold a call for one proposal, and how long
/// this side waits for its answer.
const WAIT: u64 = 25;
/// The shortest hold, with many proposals waiting.
const SHORTEST: u64 = 3;
/// Between looks while nothing waits.
const IDLE: Duration = Duration::from_secs(600);

#[derive(Deserialize)]
pub(super) struct SendArgs {
    conversation: String,
    text: String,
    #[serde(default)]
    replaces: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct WithdrawArgs {
    proposal: String,
}

/// What the service says of a proposal to its agent: never the person's
/// words.
#[derive(Deserialize)]
struct Status {
    proposal_id: String,
    revision: i64,
    state: String,
    expires_ms: i64,
    #[serde(default)]
    seq: Option<i64>,
}

/// A proposal as kept here.
struct Row {
    id: String,
    agent: String,
    incarnation: String,
    conversation: String,
    client_id: String,
    expires: i64,
    sequence: i64,
}

impl Row {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
        Ok(Row {
            id: row.get(0)?,
            agent: row.get(1)?,
            incarnation: row.get(2)?,
            conversation: row.get(3)?,
            client_id: row.get(4)?,
            expires: row.get(5)?,
            sequence: row.get(6)?,
        })
    }

    fn actor(&self) -> Actor {
        Actor { id: self.agent.clone(), incarnation: self.incarnation.clone() }
    }
}

const COLUMNS: &str = "id, agent, incarnation, conversation, client_id, expires, COALESCE(sequence, revision)";

/// Something changed (a proposal, the agent's grants): look again now,
/// cutting short the call waiting on the service.
pub(crate) fn wake(factory: &Factory) {
    factory.dm_wakes.sends.notify_waiters();
}

fn unreadable() -> String {
    "The account service's answer didn't read. Try again later.".to_owned()
}

/// What the service said of a proposal, its id checked before any route
/// is made from it.
fn status_of(answer: &[u8]) -> Result<Status, String> {
    let status: Status = serde_json::from_slice(answer).map_err(|_| unreadable())?;
    if !valid_id(&status.proposal_id) {
        return Err(unreadable());
    }
    Ok(status)
}

/// What a refusal means to an agent proposing or withdrawing a message.
fn send_refused(factory: &Factory, problem: Problem) -> String {
    match problem {
        Problem::Refused(410, _) => {
            "This conversation has ended (the friendship was removed or blocked): nothing can be sent in it.".to_owned()
        }
        // Too many wait for the person already: the service's words.
        Problem::Busy(words) if words.contains("waiting for review") => words,
        problem => refused(factory, problem),
    }
}

/// The agent's own proposal `id`, still waiting.
fn pending_of(factory: &Factory, agent: &str, id: &str) -> Option<Row> {
    factory
        .store
        .db()
        .query_row(&format!("SELECT {COLUMNS} FROM dm_sends WHERE id = ?1 AND agent = ?2 AND state = 'pending'"), [id, agent], Row::read)
        .optional()
        .ok()
        .flatten()
}

/// dm_send: a proposal for the person, sent only once they approve it.
pub(super) async fn send(factory: &Arc<Factory>, agent: &str, args: SendArgs) -> Result<ToolResult, String> {
    let actor = super::actor(factory, agent)?;
    let (person, name) = {
        let inner = factory.lock();
        let name = inner.config.agent(agent).map(|agent| agent.name.clone()).unwrap_or_default();
        (inner.config.person().to_owned(), name)
    };
    let conversation = args.conversation.trim().to_owned();
    if !valid_id(&conversation) {
        return Err(out_of_reach(&person));
    }
    let text = args.text;
    if text.trim().is_empty() || text.len() > MAX_TEXT {
        return Err("A message has 1 to 16,384 bytes of text.".to_owned());
    }
    if let Some(secrets) = crate::pipes::agents::secrets_in(factory, &text, &[]) {
        return Err(format!("That message holds the secret {secrets}, which never goes in a message. Nothing was proposed."));
    }
    let (client_id, revising) = match args.replaces.map(|id| id.trim().to_owned()).filter(|id| !id.is_empty()) {
        Some(id) => match pending_of(factory, agent, &id) {
            Some(row) if row.conversation == conversation => (row.client_id.clone(), Some(row)),
            Some(_) => return Err("That proposal is in another conversation. Propose a new message there instead.".to_owned()),
            None => return Err("That isn't one of your proposals still waiting for the person. Propose a new message instead.".to_owned()),
        },
        None => (format!("s{}", crate::encoding::hex(&crate::vault::crypto::random::<16>())), None),
    };
    let generation = match peer_of(factory, &conversation) {
        Some((_, generation)) => generation,
        None => {
            conversations(factory, &actor).await?;
            peer_of(factory, &conversation).map(|(_, generation)| generation).ok_or_else(|| out_of_reach(&person))?
        }
    };
    let proposal = Proposal {
        actor: &actor,
        name: &name,
        conversation: &conversation,
        client_id: &client_id,
        proposal_id: revising.as_ref().map(|row| row.id.clone()),
        text: &text,
        sequence: revising.as_ref().map_or(1, |row| row.sequence + 1),
    };
    // Too long once written as content: say so before anything is kept.
    proposal.content(generation).encode().map_err(|_| {
        "That message is too long once encoded (line breaks and quotes count double). Shorten it, then propose it again.".to_owned()
    })?;
    let status = propose(factory, &proposal, generation, false).await?;
    let peer = peer_of(factory, &conversation).map(|(peer, _)| peer).unwrap_or_default();
    let with = who(&peer);
    let id = &status.proposal_id;
    let text = match status.state.as_str() {
        "pending" => {
            let what = if status.revision > 1 { "Changed your message" } else { "Proposed your message" };
            format!(
                "{what} to {with} for {person}'s review (proposal {id}). Nothing is sent until {person} approves exactly this text on their device; you'll be told what they decide. It waits up to 3 days: to change it meanwhile, dm_send again with replaces {id}; to take it back, dm_withdraw."
            )
        }
        "sent" => format!("{person} already approved that message: it was sent to {with} (proposal {id})."),
        _ => format!("That proposal can't be sent any more (it was {}). Propose a new message instead.", status.state),
    };
    Ok(ToolResult::text(text, json!({})))
}

/// One text of a proposal, as encrypted for the person's devices.
struct Proposal<'a> {
    actor: &'a Actor,
    /// The agent's name now.
    name: &'a str,
    conversation: &'a str,
    client_id: &'a str,
    /// The service's id, once it gave one.
    proposal_id: Option<String>,
    text: &'a str,
    sequence: i64,
}

impl Proposal<'_> {
    /// What the person's device decrypts (dms-e2ee.md §6): the text, and
    /// what it's bound to, which the device checks against the route it
    /// came under and hashes with the text. Which Claktory sent it the
    /// device reads from the channel's certified leaf, never from here.
    fn content(&self, generation: i64) -> Content {
        let mut content = Content::text(self.client_id, self.text);
        content.kind = kind::PROPOSAL.to_owned();
        let extra = [
            ("conversation_id", json!(self.conversation)),
            ("contact_generation", json!(generation)),
            ("agent_id", json!(self.actor.id)),
            ("incarnation", json!(self.actor.incarnation)),
            ("agent_name", json!(one_line(self.name, 64))),
            ("sequence", json!(self.sequence)),
        ];
        content.extra.extend(extra.map(|(key, value)| (key.to_owned(), value)));
        content
    }

    fn pending(&self, generation: i64, proposal_id: Option<String>) -> PendingProposal {
        PendingProposal {
            client_message_id: self.client_id.to_owned(),
            proposal_id,
            agent: self.actor.id.clone(),
            conversation: self.conversation.to_owned(),
            generation,
            text: self.text.to_owned(),
        }
    }
}

/// Words for an agent when the leaf can't act.
fn leaf_words(factory: &Factory, error: &StepError) -> String {
    let person = factory.lock().config.person_capitalized().to_owned();
    error.words(&person)
}

/// Encrypts the proposal's text into the channel, keeping it sealed with
/// it (in one step), as the content the person's device will decrypt.
fn encrypt(factory: &Factory, proposal: &Proposal<'_>, generation: i64) -> Result<Envelope, String> {
    leaf::step(factory, |step| {
        let envelope = step.channel_send(&proposal.content(generation))?;
        step.keep_proposal(&proposal.pending(generation, proposal.proposal_id.clone()))?;
        Ok(envelope)
    })
    .map_err(|error| leaf_words(factory, &error))
}

/// Proposes `proposal` (new, a revision, or sent again for a device that
/// joined): encrypts it, asks the service to keep it, and keeps what it
/// said. On a conflict (the friendship's generation or the channel's epoch
/// moved on), it looks again and tries once more, encrypted anew. If it
/// isn't kept, neither is its text: the one before stays, for a revision.
/// `again`: sent again from within the channel's catch-up (`reposted`),
/// which mustn't start another.
async fn propose(factory: &Arc<Factory>, proposal: &Proposal<'_>, generation: i64, again: bool) -> Result<Status, String> {
    let before = leaf::step(factory, |step| step.proposal(proposal.client_id)).ok().flatten();
    let route = format!("conversations/{}/proposals", proposal.conversation);
    let mut generation = generation;
    let mut tries = 0;
    let answer = loop {
        let envelope = encrypt(factory, proposal, generation)?;
        let body = json!({"client_message_id": proposal.client_id, "contact_generation": generation, "envelope": envelope.to_json()});
        let answer = factory.account.dms("POST", &route, &[], Some(proposal.actor), Some(body), None).await;
        tries += 1;
        match answer {
            Err(Problem::Refused(409, _)) if tries == 1 => {
                // A stale generation or epoch: catch up on both, then again.
                // Boxed: catching up can send waiting proposals again (`reposted`).
                if !again && let Err(error) = Box::pin(leaf::catch_up_channel(factory)).await {
                    tracing::debug!(?error, "the channel didn't catch up before proposing again");
                }
                conversations(factory, proposal.actor).await?;
                generation = peer_of(factory, proposal.conversation).map_or(generation, |(_, generation)| generation);
            }
            answer => break answer,
        }
    };
    let status = match answer.map_err(|problem| send_refused(factory, problem)).and_then(|answer| status_of(&answer)) {
        Ok(status) => status,
        Err(words) => {
            match before {
                Some(before) => {
                    let _ = leaf::step(factory, |step| step.keep_proposal(&before));
                }
                None => leaf::forget_proposal(factory, proposal.client_id),
            }
            return Err(words);
        }
    };
    if status.state == "pending" {
        let id = Some(status.proposal_id.clone());
        if let Err(error) = leaf::step(factory, |step| step.keep_proposal(&proposal.pending(generation, id))) {
            tracing::warn!(?error, "a proposal's id wasn't kept with its text");
        }
    } else {
        leaf::forget_proposal(factory, proposal.client_id);
    }
    keep(factory, proposal.actor, proposal.conversation, proposal.client_id, &status, Some(proposal.sequence));
    Ok(status)
}

/// A device joined this Claktory's channel: it can't open what was sent
/// before it came, so every proposal still waiting goes again, as its next
/// revision, for every device there now. One that isn't waiting any more
/// is forgotten instead.
pub(crate) async fn reposted(factory: &Arc<Factory>) {
    let kept = match leaf::step(factory, |step| step.proposals()) {
        Ok(kept) => kept,
        Err(error) => {
            tracing::warn!(?error, "the waiting proposals couldn't be read to send again");
            return;
        }
    };
    for pending in kept {
        let row = factory
            .store
            .db()
            .query_row(
                &format!("SELECT {COLUMNS} FROM dm_sends WHERE client_id = ?1 AND agent = ?2 AND state = 'pending'"),
                [&pending.client_message_id, &pending.agent],
                Row::read,
            )
            .optional()
            .ok()
            .flatten();
        let name = factory.lock().config.agent(&pending.agent).map(|agent| agent.name.clone());
        let (Some(row), Some(name)) = (row, name) else {
            leaf::forget_proposal(factory, &pending.client_message_id);
            continue;
        };
        let proposal = Proposal {
            actor: &row.actor(),
            name: &name,
            conversation: &row.conversation,
            client_id: &row.client_id,
            proposal_id: Some(row.id.clone()),
            text: &pending.text,
            sequence: row.sequence + 1,
        };
        if let Err(error) = propose(factory, &proposal, pending.generation, true).await {
            tracing::warn!(proposal = %row.id, %error, "a waiting proposal wasn't sent again for a new device");
        }
    }
}

/// Keeps what the service said of a proposal, and the `sequence` of the
/// text it holds when it's new; one that waits shows in Needs you, and the
/// poller follows it.
fn keep(factory: &Factory, actor: &Actor, conversation: &str, client_id: &str, status: &Status, sequence: Option<i64>) {
    let now = now_ms();
    let decided = (status.state != "pending").then_some(now);
    let kept = factory.store.db().execute(
        "INSERT INTO dm_sends (id, agent, incarnation, conversation, client_id, revision, state, created, expires, decided, sequence)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT (id) DO UPDATE SET revision = ?6, state = ?7, expires = ?9, decided = ?10, sequence = COALESCE(?11, sequence)",
        params![
            status.proposal_id,
            actor.id,
            actor.incarnation,
            conversation,
            client_id,
            status.revision,
            status.state,
            now,
            status.expires_ms,
            decided,
            sequence
        ],
    );
    if let Err(error) = kept {
        tracing::warn!(%error, "couldn't keep an agent's proposed message");
    }
    factory.broadcast_workspace(&factory.lock());
    wake(factory);
}

/// dm_withdraw: takes back a proposal still waiting.
pub(super) async fn withdraw(factory: &Arc<Factory>, agent: &str, args: WithdrawArgs) -> Result<ToolResult, String> {
    let id = args.proposal.trim();
    let Some(row) = pending_of(factory, agent, id) else {
        return Err("That isn't one of your proposals still waiting for the person.".to_owned());
    };
    if super::actor(factory, agent)?.incarnation != row.incarnation {
        return Err("That isn't one of your proposals still waiting for the person.".to_owned());
    }
    let route = format!("proposals/{}", row.id);
    match factory.account.dms("DELETE", &route, &[], Some(&row.actor()), None, None).await {
        Ok(answer) => {
            let status = status_of(&answer)?;
            if status.proposal_id != row.id {
                return Err(unreadable());
            }
            let state = status.state.clone();
            if status.state != "pending" {
                leaf::forget_proposal(factory, &row.client_id);
            }
            keep(factory, &row.actor(), &row.conversation, &row.client_id, &status, None);
            let person = factory.lock().config.person().to_owned();
            let text = match state.as_str() {
                "withdrawn" => "You took it back: it won't be sent.".to_owned(),
                "sent" => format!("Too late: {person} had already approved it, and it was sent."),
                state => format!("It wasn't waiting any more (it was {state}); nothing was sent."),
            };
            Ok(ToolResult::text(text, json!({})))
        }
        Err(Problem::Refused(403 | 404, _)) => {
            end(factory, &row, "closed", None, Ended::Quietly);
            Ok(ToolResult::text("Your access to these messages ended, so the message wasn't sent.".to_owned(), json!({})))
        }
        Err(problem) => Err(send_refused(factory, problem)),
    }
}

/// Needs you: the agents' messages waiting for the person's review, oldest
/// first. Only in the preview.
pub(crate) fn waiting(factory: &Factory) -> Vec<v1::DmSendWaiting> {
    if !super::preview() {
        return Vec::new();
    }
    let db = factory.store.db();
    let listed = db
        .prepare(
            "SELECT s.id, s.agent, s.conversation, COALESCE(p.name, ''), COALESCE(p.tag, ''), s.created, s.expires
             FROM dm_sends s LEFT JOIN dm_peers p ON p.conversation = s.conversation
             WHERE s.state = 'pending' AND s.expires > ?1 ORDER BY s.created, s.id",
        )
        .and_then(|mut statement| {
            statement
                .query_map([now_ms()], |row| {
                    Ok(v1::DmSendWaiting {
                        proposal_id: row.get(0)?,
                        agent_id: row.get(1)?,
                        conversation_id: row.get(2)?,
                        friend_name: one_line(&row.get::<_, String>(3)?, 80),
                        friend_tag: one_line(&row.get::<_, String>(4)?, 32),
                        created_ms: row.get(5)?,
                        expires_ms: row.get(6)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
        });
    listed.unwrap_or_else(|error| {
        tracing::warn!(%error, "couldn't list the agents' messages waiting for review");
        Vec::new()
    })
}

/// Whether the agent is told how it ended.
#[derive(PartialEq)]
enum Ended {
    Told,
    /// The agent learned it from its own tool call, or isn't there any more.
    Quietly,
}

/// Ends a waiting proposal: it leaves Needs you, and its agent hears what
/// became of it.
fn end(factory: &Arc<Factory>, row: &Row, state: &str, seq: Option<i64>, tell: Ended) {
    let changed = factory
        .store
        .db()
        .execute("UPDATE dm_sends SET state = ?2, decided = ?3 WHERE id = ?1 AND state = 'pending'", params![row.id, state, now_ms()])
        .unwrap_or(0);
    if changed == 0 {
        return;
    }
    leaf::forget_proposal(factory, &row.client_id);
    factory.broadcast_workspace(&factory.lock());
    if tell == Ended::Quietly || state == "withdrawn" {
        return;
    }
    let (person, writer) = {
        let inner = factory.lock();
        if inner.config.agent(&row.agent).is_none() {
            return;
        }
        (inner.config.person().to_owned(), inner.config.person_capitalized().to_owned())
    };
    if crate::user_apps::current_incarnation(&factory.home, &row.agent).as_deref() != Some(row.incarnation.as_str()) {
        return;
    }
    let with = peer_of(factory, &row.conversation).map(|(peer, _)| who(&peer)).unwrap_or_else(|| "a friend".to_owned());
    let id = &row.id;
    let (summary, mut text) = match state {
        "sent" => (format!("{writer} sent your message"), format!("{writer} approved your message to {with}, and it was sent (proposal {id}")),
        "declined" => {
            (format!("{writer} declined your message"), format!("{writer} declined your message to {with} (proposal {id}): nothing was sent"))
        }
        "expired" => (
            "Your message expired".to_owned(),
            format!("Your message to {with} waited 3 days without {person}'s review and expired (proposal {id}): nothing was sent"),
        ),
        _ => (
            "Your message can't be sent".to_owned(),
            format!(
                "Your message to {with} can't be sent any more: the friendship ended, or your access to send there did (proposal {id}). Nothing was sent"
            ),
        ),
    };
    if state == "sent" {
        if let Some(seq) = seq {
            let _ = write!(text, ", message #{seq}");
        }
        text.push(')');
    }
    text.push('.');
    factory.deliver_event(&row.agent, text, host_event(summary));
}

/// Follows every proposal waiting here, one call at a time, while
/// Claktory runs. Started with the message watch, in the preview only.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    let mut backoff = Duration::from_secs(30);
    let mut pace = super::pace::Pace::default();
    loop {
        let Some(strong) = factory.upgrade() else { return };
        // Listening before looking, so a change meanwhile isn't missed.
        let wake = strong.dm_wakes.sends.clone();
        let woken = wake.notified();
        tokio::pin!(woken);
        woken.as_mut().enable();
        let rows = pending(&strong);
        if rows.is_empty() {
            drop(strong);
            tokio::select! {
                () = tokio::time::sleep(IDLE) => {}
                () = woken => {}
            }
            continue;
        }
        let wait = (WAIT / rows.len() as u64).max(SHORTEST);
        let mut trouble = false;
        // Whether every look came back still waiting well before its wait
        // (none was held at the worker); unknown when cut short.
        let mut early = Some(true);
        for row in rows {
            let asked = tokio::time::Instant::now();
            match look(&strong, &row, wait, woken.as_mut()).await {
                Ok(Looked::Done { waiting }) => {
                    if !super::pace::early(Duration::from_secs(wait), asked.elapsed(), waiting) {
                        early = early.map(|_| false);
                    }
                }
                // Something changed: start the round again.
                Ok(Looked::Woken) => {
                    early = None;
                    break;
                }
                Err(problem) => {
                    tracing::debug!(?problem, "an agent's proposed message wasn't looked at");
                    trouble = true;
                    break;
                }
            }
        }
        drop(strong);
        let pause = if trouble {
            let pause = backoff;
            backoff = (backoff * 2).min(Duration::from_secs(300));
            Some(pause)
        } else {
            backoff = Duration::from_secs(30);
            // None held at the worker: not again at once.
            early.and_then(|early| pace.after(early, Duration::from_secs(WAIT)))
        };
        if let Some(pause) = pause {
            // Listened to since before the round, so a change meanwhile
            // ends the pause.
            tokio::select! {
                () = tokio::time::sleep(pause) => {}
                () = woken => {}
            }
        }
    }
}

/// The proposals waiting, oldest first; those whose time ran out, or
/// whose agent is gone or another one now, end here without asking.
fn pending(factory: &Arc<Factory>) -> Vec<Row> {
    let rows: Vec<Row> = {
        let db = factory.store.db();
        db.prepare(&format!("SELECT {COLUMNS} FROM dm_sends WHERE state = 'pending' ORDER BY created, id"))
            .and_then(|mut statement| statement.query_map([], Row::read)?.collect::<Result<Vec<_>, _>>())
            .unwrap_or_default()
    };
    let now = now_ms();
    let mut waiting = Vec::new();
    for row in rows {
        let here = factory.lock().config.agent(&row.agent).is_some()
            && crate::user_apps::current_incarnation(&factory.home, &row.agent).as_deref() == Some(row.incarnation.as_str());
        if !here {
            end(factory, &row, "closed", None, Ended::Quietly);
        } else if row.expires <= now {
            end(factory, &row, "expired", None, Ended::Told);
        } else {
            waiting.push(row);
        }
    }
    waiting
}

enum Looked {
    /// Answered; `waiting` while the proposal still waits.
    Done { waiting: bool },
    /// Cut short: something changed here.
    Woken,
}

/// Asks the service what became of one proposal, holding the call up to
/// `wait` seconds while it still waits, unless `woken` first.
async fn look(
    factory: &Arc<Factory>,
    row: &Row,
    wait: u64,
    woken: std::pin::Pin<&mut tokio::sync::futures::Notified<'_>>,
) -> Result<Looked, Problem> {
    let route = format!("proposals/{}", row.id);
    let query = [("wait", wait.to_string())];
    let call = Duration::from_secs(wait + 15);
    let actor = row.actor();
    let asked = factory.account.dms("GET", &route, &query, Some(&actor), None, Some(call));
    let answer = tokio::select! {
        answer = asked => answer,
        () = woken => return Ok(Looked::Woken),
    };
    match answer {
        Ok(answer) => {
            let status: Status =
                serde_json::from_slice(&answer).map_err(|error| Problem::ForNow(format!("an unreadable answer: {error}")))?;
            let waiting = status.state == "pending";
            if !waiting {
                end(factory, row, &status.state, status.seq, Ended::Told);
            }
            Ok(Looked::Done { waiting })
        }
        // Its own proposal it may no longer see: its access ended.
        Err(Problem::Refused(403 | 404, _)) => {
            end_for_access(factory, row);
            Ok(Looked::Done { waiting: false })
        }
        Err(problem) => Err(problem),
    }
}

/// Ends a proposal the agent may no longer see, saying so in Bruno's words.
fn end_for_access(factory: &Arc<Factory>, row: &Row) {
    let changed = factory
        .store
        .db()
        .execute("UPDATE dm_sends SET state = 'closed', decided = ?2 WHERE id = ?1 AND state = 'pending'", params![row.id, now_ms()])
        .unwrap_or(0);
    if changed == 0 {
        return;
    }
    leaf::forget_proposal(factory, &row.client_id);
    factory.broadcast_workspace(&factory.lock());
    if factory.lock().config.agent(&row.agent).is_none() {
        return;
    }
    let text = format!("Your access to these messages ended, so the message wasn't sent (proposal {}).", row.id);
    factory.deliver_event(&row.agent, text, host_event("Your message wasn't sent".to_owned()));
}

/// An agent is deleted: its proposals leave Needs you. The service closes
/// them, since the agent's grants go with it.
pub(crate) fn forget(factory: &Factory, agent: &str) -> rusqlite::Result<()> {
    factory.store.db().execute("DELETE FROM dm_sends WHERE agent = ?1", [agent])?;
    leaf::forget_agent_proposals(factory, agent);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proposal_carries_what_the_device_binds_it_to_and_nothing_of_the_claktory() {
        let actor = Actor { id: "june".into(), incarnation: "i1".into() };
        let proposal = Proposal {
            actor: &actor,
            name: "June\u{202e}",
            conversation: "c1",
            client_id: "s00",
            proposal_id: None,
            text: "See you at 6",
            sequence: 3,
        };
        let content = proposal.content(7);
        let decoded = Content::decode(&content.encode().expect("encoded")).expect("decoded");
        assert_eq!((decoded.kind.as_str(), decoded.client_message_id.as_str(), decoded.text.as_deref()), ("proposal", "s00", Some("See you at 6")));
        assert_eq!(decoded.by_agent, None, "only the approving device says who wrote it");
        let extra: serde_json::Value = decoded.extra.into_iter().collect::<serde_json::Map<_, _>>().into();
        assert_eq!(
            extra,
            json!({"conversation_id": "c1", "contact_generation": 7, "agent_id": "june", "incarnation": "i1", "agent_name": one_line("June\u{202e}", 64), "sequence": 3})
        );
        assert!(!extra.to_string().contains("claktory"), "which Claktory comes from the sender's certified leaf");
    }
}
