//! Which device the person decided each card from (#514, spec §9.2), so
//! another device can say "Answered on Matt's iPhone" (Entry.answered_on),
//! and History can too (NeedsYouRecord.answered_on, from the card).
//!
//! Taken in one place for every kind of card: a request that decides cards
//! (`deciding`) notes what waits for the person before it (every agent's
//! and chat's `waiting_seqs`), and what stopped waiting when it's done was
//! decided from the connection's device, the one it signed in as, never
//! one a request names (Fugo). Kept in factory.db by conversation and
//! entry; set on loaded entries at once, and on a conversation's when it's
//! read again (`restore`).

use std::collections::{BTreeMap, BTreeSet};

use factory_protocol::v1::{self, client_message::Body as Request};
use rusqlite::params;

use crate::factory::Factory;
use crate::store::Store;
use crate::transcript::{Change, Transcript, now_ms};

pub(crate) const SCHEMA: &str = "
-- The device a card was decided from (answered_on.rs): by its
-- conversation (an agent's or a chat's id) and entry.
CREATE TABLE answered_on (
    owner TEXT NOT NULL,
    seq INTEGER NOT NULL,
    device_id TEXT NOT NULL,
    device_name TEXT NOT NULL,
    at_ms INTEGER NOT NULL,
    PRIMARY KEY (owner, seq)
);
";

/// What waited for the person before a request: each conversation's
/// waiting entries.
pub(crate) struct Before(BTreeMap<String, BTreeSet<u64>>);

/// Requests with which the person decides a card. Nothing else is taken
/// as deciding one, so a card that stops waiting otherwise (withdrawn,
/// expired, a conversation cleared) says no device.
fn deciding(request: &Request) -> bool {
    matches!(
        request,
        Request::AnswerAsk(_)
            | Request::DecideProposal(_)
            | Request::DecideSecretRequest(_)
            | Request::GrantSecret(_)
            | Request::DecideSecretApproval(_)
            | Request::DismissPipeStart(_)
            | Request::CreatePipeInvite(_)
            | Request::AcceptPipeInvite(_)
            | Request::DecideAppGrant(_)
            | Request::DecideAppAccess(_)
            | Request::DismissConnectProposal(_)
            | Request::StartConnect(_)
            | Request::FinishConnect(_)
            | Request::ApproveDevice(_)
            | Request::DenyDevice(_)
    )
}

/// What waits now, when `request` decides cards.
pub(crate) fn before(factory: &Factory, request: Option<&Request>) -> Option<Before> {
    request.is_some_and(deciding).then(|| Before(waiting(factory)))
}

fn waiting(factory: &Factory) -> BTreeMap<String, BTreeSet<u64>> {
    let inner = factory.lock();
    inner
        .agents
        .iter()
        .filter_map(|(id, runtime)| {
            let waiting: BTreeSet<u64> = runtime.transcript.as_ref()?.waiting_seqs().into_iter().collect();
            (!waiting.is_empty()).then(|| (id.clone(), waiting))
        })
        .collect()
}

/// After the request: what stopped waiting was decided from `device_id`
/// (`device_name`, as it said in its hello).
pub(crate) fn after(factory: &Factory, before: Option<Before>, device_id: &str, device_name: &str) {
    let Some(Before(before)) = before else { return };
    let now = waiting(factory);
    let decided: Vec<(String, u64)> = before
        .into_iter()
        .flat_map(|(owner, seqs)| {
            let still = now.get(&owner).cloned().unwrap_or_default();
            seqs.into_iter().filter(move |seq| !still.contains(seq)).map(move |seq| (owner.clone(), seq))
        })
        .collect();
    if decided.is_empty() {
        return;
    }
    let on = v1::AnsweredOn { device_id: device_id.to_owned(), device_name: device_name.to_owned(), at_ms: now_ms() };
    for (owner, seq) in &decided {
        let kept = factory.store.db().execute(
            "INSERT OR REPLACE INTO answered_on (owner, seq, device_id, device_name, at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![owner, i64::try_from(*seq).unwrap_or(i64::MAX), on.device_id, on.device_name, on.at_ms],
        );
        if let Err(error) = kept {
            tracing::error!(%owner, seq, %error, "could not keep which device a card was decided from");
        }
    }
    let mut inner = factory.lock();
    for (owner, seq) in decided {
        let entry = inner
            .agents
            .get_mut(&owner)
            .and_then(|runtime| runtime.transcript.as_mut())
            .and_then(|transcript| transcript.set_answered_on(seq, on.clone()));
        if let Some(entry) = entry {
            factory.send_change(&inner, &owner, Change::Entry(entry));
        }
    }
    drop(inner);
}

/// A conversation read again: its decided cards say where from.
pub(crate) fn restore(store: &Store, owner: &str, transcript: &mut Transcript) -> rusqlite::Result<()> {
    let db = store.db();
    let mut statement = db.prepare("SELECT seq, device_id, device_name, at_ms FROM answered_on WHERE owner = ?1")?;
    let rows = statement.query_map([owner], |row| {
        Ok((row.get::<_, i64>(0)?, v1::AnsweredOn { device_id: row.get(1)?, device_name: row.get(2)?, at_ms: row.get(3)? }))
    })?;
    for row in rows {
        let (seq, on) = row?;
        if let Ok(seq) = u64::try_from(seq) {
            transcript.set_answered_on(seq, on);
        }
    }
    Ok(())
}

/// An agent or a chat is deleted: where its cards were decided goes too.
pub(crate) fn forget(store: &Store, owner: &str) -> rusqlite::Result<()> {
    store.db().execute("DELETE FROM answered_on WHERE owner = ?1", [owner])?;
    Ok(())
}
