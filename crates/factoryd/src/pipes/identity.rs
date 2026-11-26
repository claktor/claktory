//! Verified pipes: whose Claktory is at the other end, as Claktory's account
//! service vouches. Each side asks the service for a statement binding its
//! own tunnel key (the key the pipe pins) to its account and email, and
//! sends it on every ping (`pipe_link`); the other side checks it with its
//! own pinned service key, against the key pinned to the pipe, and pins the
//! identity with the pipe.
//!
//! Changes are loud: another account holds what comes from the other side
//! and stops agents sending until the person accepts it
//! (AcceptPipeIdentity); another email of the same account is a notice; a
//! statement that ran out (a week) or a peer that signed out is "no longer
//! verified". The comparison is always with what the person last accepted
//! (the pin): a lapse never clears it. Another account clears by itself only
//! when the pinned one vouches again, with a statement made after the other
//! one's; what came meanwhile waits for the person's review. What this side
//! can't check (signed out, or another account service) changes nothing.

use std::sync::{Arc, Mutex};

use factory_protocol::v1;
use rusqlite::{OptionalExtension, params};

use super::{MessageState, Record};
use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;

pub(crate) const SCHEMA: &str = "
-- Who each pipe's other side is, as the account service vouches
-- (pipes/identity.rs): the identity the person accepted (pinned: the first
-- one checked, or one accepted after a change; '' when none), and the
-- latest statement checked here, which may differ.
CREATE TABLE pipe_identities (
    pipe TEXT PRIMARY KEY,
    pinned_account TEXT NOT NULL DEFAULT '',
    pinned_email TEXT NOT NULL DEFAULT '',
    account TEXT NOT NULL DEFAULT '',
    email TEXT NOT NULL DEFAULT '',
    -- The latest statement's times, in unix seconds.
    iat INTEGER NOT NULL DEFAULT 0,
    exp INTEGER NOT NULL DEFAULT 0,
    -- 1 when the other side said, after it, that it has none (signed out).
    withdrawn INTEGER NOT NULL DEFAULT 0,
    -- What stays until the person accepts it: 2 another email, 3 another
    -- account (PipeIdentityChange).
    change INTEGER NOT NULL DEFAULT 0,
    -- 1 when the person dismissed No longer verified, until the other
    -- side is vouched for again.
    lapse_seen INTEGER NOT NULL DEFAULT 0
);
-- This Claktory's own statement, which it sends its pipes' other sides.
CREATE TABLE pipe_identity_own (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    statement TEXT NOT NULL,
    -- Unix seconds.
    exp INTEGER NOT NULL,
    -- When it was fetched, and when a fetch was last tried, in ms.
    fetched INTEGER NOT NULL,
    tried INTEGER NOT NULL
);
";

/// The registration a kept statement of this side's was asked for under
/// (`own`): '' in rows kept before it, which are asked for again.
pub(crate) const OWN_REGISTRATION: &str = "
ALTER TABLE pipe_identity_own ADD COLUMN registration TEXT NOT NULL DEFAULT '';
";

/// The signed iat of the latest statement for another account than the
/// pinned one (unix seconds; 0 in rows kept before it): only a statement
/// for the pinned account made after it clears the change (`heard`, Fugo
/// on row 1327).
pub(crate) const CHANGE_IAT: &str = "
ALTER TABLE pipe_identities ADD COLUMN change_iat INTEGER NOT NULL DEFAULT 0;
";

/// The most of a peer's `identity` looked at.
const MOST_BYTES: usize = 4 * 1024;
/// How long a statement lasts (the account service's week).
const PIPE_PEER_LIFETIME: i64 = 7 * 24 * 3600;
/// A statement is asked for again half an hour after the last, so it's
/// always fresh enough for the other side (`FRESH_S`).
const REFRESH_MS: i64 = 30 * 60 * 1000;
/// How long after the account service made it a statement vouches for the
/// other side (Fugo, row 1252), so a Claktory that signed in to another
/// account, or out, isn't shown as the first one for longer, even if it
/// kept presenting that one's statement. Seconds. 25 hours for now (#199,
/// Bruno 35906, Fugo 35908): Claktories before this one ask for a statement
/// once a day, and an hour would show their pipes as no longer verified
/// most of the day. A release after this one has been out two weeks, it's
/// an hour.
const FRESH_S: i64 = 25 * 3600;
/// After a failed ask, the next waits this long.
const RETRY_MS: i64 = 10 * 60 * 1000;
/// How the note on what waits for the person while the other side is
/// another account ends (`held_note`), which tells those messages apart.
const HELD_NOTE_ENDS: &str = "then choose Trust this Claktory… to let it through.";
/// The note on what came while the other side was another account, once
/// it's back to the one the person accepted (`back_to_accepted`).
const CAME_MEANWHILE_NOTE: &str =
    "Waiting for you: this came while their Claktory was signed in to a different account. Let it in, or discard it.";

/// What a statement vouches, once checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Vouched {
    pub account_id: String,
    pub email: String,
    /// Unix seconds.
    pub iat: i64,
    pub exp: i64,
}

type Check = dyn Fn(&str, &str, i64) -> Option<Vouched> + Send + Sync;

/// How statements are fetched and checked: the account service's, or a
/// test's.
#[derive(Default)]
pub(crate) struct Identity {
    test: Mutex<Option<TestVouch>>,
}

#[derive(Clone)]
struct TestVouch {
    can_check: bool,
    own: Option<String>,
    check: Arc<Check>,
}

impl Identity {
    fn test(&self) -> Option<TestVouch> {
        self.test.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

/// Whether this Claktory can check statements now: signed in to an account
/// service whose key it pinned (`account`: the settings' account).
fn can_check(factory: &Factory, account: &crate::config::AccountConfig) -> bool {
    if let Some(test) = factory.pipes.identity.test() {
        return test.can_check;
    }
    factory.account.signed_in_as(account).is_some()
}

/// `statement` checked with this Claktory's pinned service key, for the
/// peer pinned to `endpoint`, at `now` (unix seconds).
fn check(factory: &Factory, statement: &str, endpoint: &str, now: i64) -> Option<Vouched> {
    if let Some(test) = factory.pipes.identity.test() {
        return (test.check)(statement, endpoint, now);
    }
    service::verify(factory, statement, endpoint, now)
}

/// The account service's side (`crate::account`).
mod service {
    use std::sync::Arc;

    use super::Vouched;
    use crate::factory::Factory;

    /// Checked with the service key this Claktory pinned at sign-in, never
    /// one the statement names; None when it can't check (signed out).
    pub(super) fn verify(factory: &Factory, statement: &str, endpoint: &str, now: i64) -> Option<Vouched> {
        let statement = factory.account.verify_pipe_peer(statement, endpoint, now)?;
        Some(Vouched { account_id: statement.account_id, email: statement.email, iat: statement.iat, exp: statement.exp })
    }

    /// Ok(Some): this side's statement; Ok(None): it can't have one (send
    /// ""); Err: not now (keep what's kept, try later).
    pub(super) async fn statement(factory: &Arc<Factory>) -> Result<Option<String>, String> {
        crate::account::pipe_identity(factory).await
    }
}

/// The latest checked here, and what the person accepted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Row {
    pinned_account: String,
    pinned_email: String,
    account: String,
    email: String,
    iat: i64,
    exp: i64,
    withdrawn: bool,
    change: i32,
    lapse_seen: bool,
    /// `CHANGE_IAT`.
    change_iat: i64,
}

const COLUMNS: &str = "pinned_account, pinned_email, account, email, iat, exp, withdrawn, change, lapse_seen, change_iat";

fn read(factory: &Factory, pipe: &str) -> Row {
    let db = factory.store.db();
    db.query_row(&format!("SELECT {COLUMNS} FROM pipe_identities WHERE pipe = ?1"), [pipe], |row| {
        Ok(Row {
            pinned_account: row.get(0)?,
            pinned_email: row.get(1)?,
            account: row.get(2)?,
            email: row.get(3)?,
            iat: row.get(4)?,
            exp: row.get(5)?,
            withdrawn: row.get(6)?,
            change: row.get(7)?,
            lapse_seen: row.get(8)?,
            change_iat: row.get(9)?,
        })
    })
    .optional()
    .ok()
    .flatten()
    .unwrap_or_default()
}

fn write(factory: &Factory, pipe: &str, row: &Row) {
    let written = factory.store.db().execute(
        &format!("INSERT OR REPLACE INTO pipe_identities (pipe, {COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"),
        params![
            pipe,
            row.pinned_account,
            row.pinned_email,
            row.account,
            row.email,
            row.iat,
            row.exp,
            row.withdrawn,
            row.change,
            row.lapse_seen,
            row.change_iat
        ],
    );
    if let Err(error) = written {
        tracing::error!(%pipe, %error, "could not keep who a pipe's other side is");
    }
}

const EMAIL_CHANGED: i32 = v1::PipeIdentityChange::EmailChanged as i32;
const ACCOUNT_CHANGED: i32 = v1::PipeIdentityChange::AccountChanged as i32;

/// The other side presented `identity` (a statement, or "" for none).
pub(crate) fn heard(factory: &Arc<Factory>, pipe: &str, endpoint: &str, identity: &str) {
    if identity.len() > MOST_BYTES || factory.pipes.get(pipe).is_none() {
        return;
    }
    let before = read(factory, pipe);
    let mut row = before.clone();
    if identity.is_empty() {
        row.withdrawn = !row.account.is_empty();
    } else {
        // What can't be checked here changes nothing.
        let now = now_ms() / 1000;
        let Some(vouched) = check(factory, identity, endpoint, now) else { return };
        // Nor does a statement past its freshness (`FRESH_S`).
        if vouched.iat.saturating_add(FRESH_S) < now {
            return;
        }
        // Never back to an older statement: a peer can't flip to a previous one.
        if vouched.iat < row.iat {
            return;
        }
        // Nor the account the person accepted, while it's another one, with
        // a statement made before that one's (Fugo on row 1327): one kept
        // from before can't clear the change.
        if row.change == ACCOUNT_CHANGED && vouched.account_id == row.pinned_account && vouched.iat <= row.change_iat {
            return;
        }
        // It vouches only while it's fresh, however long the service made it for.
        let exp = vouched.exp.min(vouched.iat.saturating_add(FRESH_S));
        (row.account, row.email, row.iat, row.exp, row.withdrawn, row.lapse_seen) =
            (vouched.account_id, vouched.email, vouched.iat, exp, false, false);
        if row.pinned_account.is_empty() {
            // The first identity checked, or the first since the person
            // accepted none: pinned as it is.
            if row.change != ACCOUNT_CHANGED {
                (row.pinned_account, row.pinned_email, row.change) = (row.account.clone(), row.email.clone(), 0);
            }
        } else if row.account != row.pinned_account {
            (row.change, row.change_iat) = (ACCOUNT_CHANGED, row.iat);
        } else {
            // The account the person accepted, or back to it (Abbacchio's
            // row 1327) with a statement made after the other account's.
            row.change = if row.email == row.pinned_email { 0 } else { EMAIL_CHANGED };
        }
    }
    let back = before.change == ACCOUNT_CHANGED && row.change != ACCOUNT_CHANGED;
    if row != before {
        write(factory, pipe, &row);
        if back {
            back_to_accepted(factory, pipe, &row);
        }
        let shown = |row: &Row| peer_fields(row, true, 0);
        if shown(&row) != shown(&before) {
            factory.broadcast_workspace(&factory.lock());
            // A card asking to accept the change before is void (#471 G7).
            let why = if back {
                "the other side signs in with the account you accepted again"
            } else if identity.is_empty() {
                // Signed out there, or signing in again: it presents nothing
                // for a moment first, whichever account comes next (row 1355).
                "the other side's Claktory no longer says which account it's signed in to"
            } else {
                "who's at the other end of the pipe changed again since it was asked"
            };
            super::trust_card::settle(factory, pipe, why);
        }
    }
}

/// The other side signs in with the account the person accepted again
/// (row 1327): what came while it was another account isn't let in but
/// waits for the person's review, agents' messages queued for it go on,
/// and the pipe says so.
fn back_to_accepted(factory: &Arc<Factory>, pipe: &str, row: &Row) {
    if factory.pipes.get(pipe).is_none() {
        return;
    }
    let held = held_for_account(factory, pipe);
    for seq in &held {
        super::set_state(factory, pipe, *seq, MessageState::HeldIncoming, Some(CAME_MEANWHILE_NOTE));
    }
    let mut line = format!("The other side's Claktory signs in with the account you accepted again, {}.", row.pinned_email);
    if !held.is_empty() {
        line.push_str(" What came from it while it was another account waits for you.");
    }
    super::line(factory, pipe, line);
    crate::pipe_link::wake(factory, pipe);
}

/// (verified, verified_email, identity_change, previous_email) as shown
/// (`now`: unix seconds; 0 to leave out what time changes).
fn peer_fields(row: &Row, can_check: bool, now: i64) -> (bool, String, i32, String) {
    let held = !row.account.is_empty() && !row.withdrawn && (now == 0 || now <= row.exp);
    if row.change == ACCOUNT_CHANGED {
        let email = if held { row.email.clone() } else { String::new() };
        return (false, email, ACCOUNT_CHANGED, row.pinned_email.clone());
    }
    if !can_check || row.pinned_account.is_empty() {
        return (false, String::new(), 0, String::new());
    }
    if !held {
        if row.lapse_seen {
            return (false, String::new(), 0, String::new());
        }
        return (false, String::new(), v1::PipeIdentityChange::Lapsed as i32, row.pinned_email.clone());
    }
    if row.change == EMAIL_CHANGED {
        return (true, row.email.clone(), EMAIL_CHANGED, row.pinned_email.clone());
    }
    (true, row.email.clone(), 0, String::new())
}

/// The pipe's PipePeer, with who it is filled in.
pub(crate) fn peer(factory: &Factory, pipe: &str, can_check: bool, mut peer: v1::PipePeer) -> v1::PipePeer {
    let (verified, verified_email, change, previous_email) = peer_fields(&read(factory, pipe), can_check, now_ms() / 1000);
    (peer.verified, peer.verified_email, peer.identity_change, peer.previous_email) = (verified, verified_email, change, previous_email);
    peer
}

/// Whether views can check statements now (once per view: `account`, the
/// settings' account).
pub(crate) fn checks(factory: &Factory, account: &crate::config::AccountConfig) -> bool {
    can_check(factory, account)
}

/// The other side is another account than the person accepted: what it
/// sends waits for them, and agents can't send to it.
pub(crate) fn changed_account(factory: &Factory, pipe: &str) -> bool {
    read(factory, pipe).change == ACCOUNT_CHANGED
}

/// The note on a message from the other side held because it's another
/// account now (Trish's words, as the pipe's warning says it); None when
/// it isn't.
pub(crate) fn held_note(factory: &Factory, record: &Record) -> Option<String> {
    let row = read(factory, &record.id);
    if row.change != ACCOUNT_CHANGED {
        return None;
    }
    let name = record.peer.as_ref().map(|peer| if peer.owner_name.is_empty() { &peer.claktory } else { &peer.owner_name });
    let name = name.filter(|name| !name.is_empty()).map_or("the other side", |name| name.as_str());
    Some(if row.pinned_email.is_empty() {
        format!("Waiting for you: this pipe's Claktory now belongs to a different account. Check with {name}, {HELD_NOTE_ENDS}")
    } else {
        format!("Waiting for you: this pipe's Claktory is no longer confirmed as {}'s. Check with {name}, {HELD_NOTE_ENDS}", row.pinned_email)
    })
}

/// What an agent is told when it can't send for that.
pub(crate) fn refusal(factory: &Factory) -> String {
    let person = factory.lock().config.person().to_owned();
    format!("The other side's Claktory now belongs to a different account; {person} has to accept it first.")
}

/// What the other side presents now, for a card to be bound to
/// (`super::trust_card`): the change shown (`v1::PipeIdentityChange`), and
/// who it was and is, with its key.
pub(crate) fn presented(factory: &Factory, record: &Record) -> (i32, String) {
    let row = read(factory, &record.id);
    let account = factory.lock().config.account.clone();
    let (_, _, change, _) = peer_fields(&row, can_check(factory, &account), now_ms() / 1000);
    let endpoint = record.peer.as_ref().map(|peer| peer.endpoint_id.as_str()).unwrap_or_default();
    let withdrawn = if row.withdrawn { "1" } else { "0" };
    let presented = [row.pinned_account.as_str(), &row.pinned_email, &row.account, &row.email, withdrawn, endpoint].join("\n");
    (change, presented)
}

/// AcceptPipeIdentity, in the pipe or from an agent's card (`card_id`, #471
/// G7): the card is checked and claimed first, and its agent told.
pub(crate) fn accept_request(factory: &Arc<Factory>, request: &v1::AcceptPipeIdentity) -> Result<(), RequestError> {
    let record = super::known(factory, &request.pipe_id)?;
    unpresented(&read(factory, &request.pipe_id))?;
    let card = super::trust_card::claim(factory, &record, &request.card_id)?;
    let accepted = accept(factory, &request.pipe_id);
    if let Some(id) = card {
        super::trust_card::settled(factory, id, &record, accepted.is_ok());
    }
    accepted
}

/// AcceptPipeIdentity: what the other side presents now is pinned (with
/// nothing, the pin stays and "no longer verified" is dismissed), the
/// change cleared, and what it held goes on.
pub(crate) fn accept(factory: &Arc<Factory>, pipe: &str) -> Result<(), RequestError> {
    let record = super::known(factory, pipe)?;
    let mut row = read(factory, pipe);
    unpresented(&row)?;
    // Only what's presented now is pinned: a lapse accepted keeps the pin,
    // so another account later is still a change.
    if presents(&row) {
        (row.pinned_account, row.pinned_email) = (row.account.clone(), row.email.clone());
    } else {
        row.lapse_seen = true;
    }
    let was = std::mem::take(&mut row.change);
    write(factory, pipe, &row);
    super::trust_card::settle(factory, pipe, "the change was accepted in the pipe");
    if was == ACCOUNT_CHANGED {
        release(factory, &record);
        // Agents' messages queued for the other side go on too.
        crate::pipe_link::wake(factory, pipe);
    }
    factory.broadcast_workspace(&factory.lock());
    Ok(())
}

/// Whether the other side presents an identity now (a statement, fresh,
/// not withdrawn since).
fn presents(row: &Row) -> bool {
    !row.account.is_empty() && !row.withdrawn && now_ms() / 1000 <= row.exp
}

/// Another account with nothing presented now (signed out there, or its
/// statement ran out) can't be accepted: the person would let in what came
/// from an account they never saw (Abbacchio's row 1328, Fugo).
fn unpresented(row: &Row) -> Result<(), RequestError> {
    if row.change == ACCOUNT_CHANGED && !presents(row) {
        return Err(RequestError::Pipe(
            "Wait until the other side's Claktory says which account it's signed in to; it doesn't say now.".to_owned(),
        ));
    }
    Ok(())
}

/// What waited because the other side was another account goes on, in
/// order: in, or on to wait for the person's review when that's on.
fn release(factory: &Arc<Factory>, record: &Record) {
    for seq in held_for_account(factory, &record.id) {
        if record.review_incoming {
            super::set_state(factory, &record.id, seq, MessageState::HeldIncoming, Some(super::REVIEW_NOTE));
            continue;
        }
        let let_in = v1::DecidePipeMessage { pipe_id: record.id.clone(), seq, allow: true, ..Default::default() };
        if let Err(error) = super::decide_message(factory, &let_in) {
            tracing::warn!(pipe = %record.id, %error, "could not let in a message held for a change of account");
        }
    }
}

/// Every message from the other side held because it was another account
/// (`held_note`), oldest first, however many came since (Fugo 37600: not
/// only the latest page).
fn held_for_account(factory: &Factory, pipe: &str) -> Vec<u64> {
    const HELD: &str = "SELECT seq, note FROM pipe_messages WHERE pipe = ?1 AND state = 'held_incoming' ORDER BY seq";
    let db = factory.store.db();
    let read = |row: &rusqlite::Row<'_>| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?));
    let held = db.prepare(HELD).and_then(|mut statement| statement.query_map([pipe], read)?.collect::<Result<Vec<_>, _>>());
    let held = held.unwrap_or_else(|error| {
        tracing::error!(%pipe, %error, "could not read what a pipe holds for a change of account");
        Vec::new()
    });
    held.into_iter().filter(|(_, note)| note.ends_with(HELD_NOTE_ENDS)).filter_map(|(seq, _)| u64::try_from(seq).ok()).collect()
}

// ---- This side's own statement ----

/// What this side presents (as `heard` takes it): its statement while
/// signed in, "" when it isn't. Asks for a new one, in the background,
/// when it's half an hour old or missing, and at once when this Claktory
/// signed in again, or to another account, since it was asked for
/// (Abbacchio's row 1252). Then the statement kept is dropped at once, and
/// none is presented until the new one comes (Fugo): never a statement of a
/// sign-in that's over.
pub(crate) fn own(factory: &Arc<Factory>) -> Option<String> {
    if let Some(test) = factory.pipes.identity.test() {
        return test.own;
    }
    let account = factory.lock().config.account.clone();
    let (Some((account_id, email, service)), Some((_, claktory_id, epoch))) =
        (factory.account.signed_in_as(&account), factory.account.registration(&account))
    else {
        let _ = factory.store.db().execute("DELETE FROM pipe_identity_own", []);
        return Some(String::new());
    };
    // The sign-in a statement was asked under.
    let registration = format!("{service}\n{account_id}\n{email}\n{claktory_id}\n{epoch}");
    let now = now_ms();
    let kept: Option<(String, i64, i64, i64, String)> = factory
        .store
        .db()
        .query_row("SELECT statement, exp, fetched, tried, registration FROM pipe_identity_own WHERE id = 1", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
        })
        .optional()
        .ok()
        .flatten();
    let (statement, exp, fetched, tried, kept_registration) = kept.unwrap_or_default();
    if kept_registration != registration {
        fetch(factory, now, &registration, true);
        return Some(String::new());
    }
    if now - fetched > REFRESH_MS && now - tried > RETRY_MS {
        fetch(factory, now, &registration, false);
    }
    Some(if exp * 1000 > now { statement } else { String::new() })
}

/// Asks the account service for this side's statement under
/// `registration`, in the background; `anew`, for a new registration, drops
/// the statement kept now.
fn fetch(factory: &Arc<Factory>, now: i64, registration: &str, anew: bool) {
    let tried = if anew {
        factory.store.db().execute(
            "INSERT INTO pipe_identity_own (id, statement, exp, fetched, tried, registration) VALUES (1, '', 0, 0, ?1, ?2)
             ON CONFLICT (id) DO UPDATE SET statement = '', exp = 0, fetched = 0, tried = ?1, registration = ?2",
            params![now, registration],
        )
    } else {
        factory.store.db().execute(
            "INSERT INTO pipe_identity_own (id, statement, exp, fetched, tried, registration) VALUES (1, '', 0, 0, ?1, ?2)
             ON CONFLICT (id) DO UPDATE SET tried = ?1",
            params![now, registration],
        )
    };
    if tried.is_err() {
        return;
    }
    let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
    let (factory, registration) = (factory.clone(), registration.to_owned());
    runtime.spawn(async move {
        let statement = match service::statement(&factory).await {
            Ok(statement) => statement.unwrap_or_default(),
            Err(error) => {
                tracing::info!(%error, "the account service didn't vouch for this Claktory's pipes now");
                return;
            }
        };
        let exp = if statement.is_empty() { 0 } else { now_ms() / 1000 + PIPE_PEER_LIFETIME };
        // Kept only while it's still the registration it was asked under: a
        // sign-in meanwhile asked again.
        let kept = factory.store.db().execute(
            "UPDATE pipe_identity_own SET statement = ?1, exp = ?2, fetched = ?3 WHERE id = 1 AND registration = ?4",
            params![statement, exp, now_ms(), registration],
        );
        if let Err(error) = kept {
            tracing::error!(%error, "could not keep this Claktory's statement for pipes");
        }
    });
}

// ---- Tests ----

/// Tests: statements are checked by `check` (statement, endpoint, now),
/// this side can check them or not, and presents `own`.
#[doc(hidden)]
pub fn vouch_for_test(
    factory: &Factory,
    can_check: bool,
    own: Option<String>,
    check: impl Fn(&str, &str, i64) -> Option<(String, String, i64, i64)> + Send + Sync + 'static,
) {
    let check = Arc::new(move |statement: &str, endpoint: &str, now: i64| {
        check(statement, endpoint, now).map(|(account_id, email, iat, exp)| Vouched { account_id, email, iat, exp })
    });
    *factory.pipes.identity.test.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(TestVouch { can_check, own, check });
}

/// Tests: what this Claktory presents its pipes' other sides now (`own`).
#[doc(hidden)]
pub fn own_for_test(factory: &Arc<Factory>) -> Option<String> {
    own(factory)
}

/// Tests: this Claktory's own statement, as the account service gives it.
#[doc(hidden)]
pub async fn own_statement_for_test(factory: &Arc<Factory>) -> Result<Option<String>, String> {
    service::statement(factory).await
}

/// Tests: this Claktory's tunnel key, as the pipes pin it (iroh's form).
#[doc(hidden)]
pub fn endpoint_for_test(factory: &Factory) -> String {
    crate::remote::secret_key(&factory.home).expect("a tunnel key").public().to_string()
}

/// Tests: what `statement` vouches when checked here for `endpoint`.
#[doc(hidden)]
pub fn check_for_test(factory: &Factory, statement: &str, endpoint: &str) -> Option<(String, String)> {
    service::verify(factory, statement, endpoint, now_ms() / 1000).map(|vouched| (vouched.account_id, vouched.email))
}

/// Tests: a pipe message's state and note, however far back it is.
#[doc(hidden)]
pub fn kept_for_test(factory: &Factory, pipe: &str, seq: u64) -> Option<(String, String)> {
    let seq = i64::try_from(seq).ok()?;
    factory
        .store
        .db()
        .query_row("SELECT state, note FROM pipe_messages WHERE pipe = ?1 AND seq = ?2", params![pipe, seq], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()
        .ok()
        .flatten()
}

/// Tests: the other side presented `identity`, as the transport hands it.
#[doc(hidden)]
pub fn heard_for_test(factory: &Arc<Factory>, pipe: &str, endpoint: &str, identity: &str) {
    heard(factory, pipe, endpoint, identity);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pinned: (&str, &str), now: (&str, &str), exp: i64, change: i32) -> Row {
        Row {
            pinned_account: pinned.0.into(),
            pinned_email: pinned.1.into(),
            account: now.0.into(),
            email: now.1.into(),
            iat: 1,
            exp,
            withdrawn: false,
            change,
            lapse_seen: false,
            change_iat: 0,
        }
    }

    #[test]
    fn what_shows_follows_the_pin_and_what_this_side_can_check() {
        let ana = ("a1", "ana@example.com");
        assert_eq!(peer_fields(&row(ana, ana, 100, 0), true, 50), (true, "ana@example.com".into(), 0, String::new()));
        // Ran out: no longer verified, naming who it was.
        assert_eq!(peer_fields(&row(ana, ana, 100, 0), true, 200), (false, String::new(), 1, "ana@example.com".into()));
        // Signed out there.
        let mut withdrawn = row(ana, ana, 100, 0);
        withdrawn.withdrawn = true;
        assert_eq!(peer_fields(&withdrawn, true, 50).2, 1);
        // Dismissed: not verified, without the notice.
        withdrawn.lapse_seen = true;
        assert_eq!(peer_fields(&withdrawn, true, 50), (false, String::new(), 0, String::new()));
        // Can't check here: nothing to say, nothing changed.
        assert_eq!(peer_fields(&row(ana, ana, 100, 0), false, 200), (false, String::new(), 0, String::new()));
        assert_eq!(peer_fields(&row(("", ""), ("", ""), 0, 0), true, 50), (false, String::new(), 0, String::new()));
        // Another email: verified, with a notice.
        let new = row(ana, ("a1", "ana@new.example"), 100, EMAIL_CHANGED);
        assert_eq!(peer_fields(&new, true, 50), (true, "ana@new.example".into(), 2, "ana@example.com".into()));
        // Another account: never verified until accepted, whatever this side can check.
        let bo = row(ana, ("b1", "bo@example.com"), 100, ACCOUNT_CHANGED);
        assert_eq!(peer_fields(&bo, false, 50), (false, "bo@example.com".into(), 3, "ana@example.com".into()));
        assert_eq!(peer_fields(&bo, true, 200), (false, String::new(), 3, "ana@example.com".into()));
    }
}
