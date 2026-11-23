//! Agents and the person's user-level apps (U2): apps kept at the person's
//! account rather than in any one Claktory, direct messages first. The
//! account service holds the grants and enforces them
//! (crates/claktory-user-apps/README.md); this side decides who may ask for
//! one, and keeps a mirror.
//!
//! - **Incarnation.** The service knows an agent by its id and incarnation:
//!   32 random hex characters in `<home>/agents/<id>/incarnation`, made the
//!   first time one is needed. The folder is what makes an agent the same
//!   agent, so renaming it, changing its model or moving it to another
//!   project keeps it, and deleting it removes it (older backends too): an
//!   agent made again with the same id is another agent, and its
//!   predecessor's grants don't match it. Nothing about it is in
//!   config.json.
//! - **Proposals.** Only Rivet (`proposals::rivet`, the settings' admin
//!   agent, checked at every call) proposes a grant, with
//!   `app_grants_propose` (the App grants extension), never after reading a
//!   pipe. The proposal is a card in its conversation
//!   (`v1::AppGrantProposal`) and waits for the person; nothing is granted
//!   by default. The person approves it (DecideAppGrant), and only then
//!   does this Claktory ask the service for the grant, for the agent as it
//!   was when proposed: deleted and made again since, it fails.
//! - **Mirror.** The grants made are kept in factory.db (`app_grants`) and
//!   reconciled at every check-in: the service's list wins, and grants of
//!   agents that are gone, or another incarnation's, are revoked there.
//!   Deleting an agent revokes its grants at once, as far as the service
//!   can be reached; the next check-in catches the rest.

pub mod dms;

use std::io::Write as _;
use std::sync::Arc;

use factory_protocol::v1::{self, AppGrantState as State};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::account::apps::{Actor, Problem, Put, Resource};
use crate::factory::{Factory, RequestError, host_event};
use crate::home::Home;
use crate::store::Store;
use crate::transcript::now_ms;

pub(crate) const SCHEMA: &str = "
-- Rivet's proposals to give an agent access to an app at the person's
-- account (app_grants_propose): the card in its conversation.
CREATE TABLE app_grant_proposals (
    id TEXT PRIMARY KEY,
    -- Rivet, whose conversation shows it.
    agent TEXT NOT NULL,
    -- The agent it would give access, as it was when proposed.
    grantee TEXT NOT NULL,
    incarnation TEXT NOT NULL,
    app TEXT NOT NULL,
    -- JSON: [\"dm.read\"], [{\"kind\":…,\"id\":…}].
    scopes TEXT NOT NULL,
    resources TEXT NOT NULL,
    -- ms since 1970; 0 for until it's taken back.
    expires INTEGER NOT NULL,
    reason TEXT NOT NULL,
    -- pending, granted, declined, failed, or cleared (/clear closed it
    -- unseen; shown as declined)
    state TEXT NOT NULL,
    created INTEGER NOT NULL,
    decided INTEGER,
    comment TEXT NOT NULL DEFAULT '',
    error TEXT NOT NULL DEFAULT '',
    grant_id TEXT NOT NULL DEFAULT '',
    -- The grantee had read a pipe, when proposed and again when decided.
    read_pipes INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX app_grant_proposals_by_agent ON app_grant_proposals (agent, created);
-- The grants this Claktory's agents hold at the account's apps, as the
-- service last said (it wins).
CREATE TABLE app_grants (
    id TEXT PRIMARY KEY,
    app TEXT NOT NULL,
    agent TEXT NOT NULL,
    incarnation TEXT NOT NULL,
    scopes TEXT NOT NULL,
    resources TEXT NOT NULL,
    granted INTEGER NOT NULL,
    expires INTEGER NOT NULL
);
CREATE INDEX app_grants_by_agent ON app_grants (agent);
";

/// An app at the account, as Claktory shows it. The service has the same
/// list and enforces it; this one words the card.
pub(crate) struct App {
    pub id: &'static str,
    pub name: &'static str,
    /// Each scope, and what it lets an agent do.
    pub scopes: &'static [(&'static str, &'static str)],
    pub resource_kinds: &'static [&'static str],
    /// A grant must name what it reaches.
    pub needs_resources: bool,
}

pub(crate) const APPS: &[App] = &[App {
    id: "dms",
    name: "Direct messages",
    scopes: &[
        ("dm.read", "Read these conversations"),
        ("dm.draft", "Write drafts in them for you to send"),
        ("dm.send", "Send messages as you in them"),
        ("dm.subscribe", "Know when a new message arrives in them"),
    ],
    resource_kinds: &["conversation"],
    needs_resources: true,
}];

pub(crate) fn app(id: &str) -> Option<&'static App> {
    APPS.iter().find(|app| app.id == id)
}

/// The service's limits on a grant.
pub(crate) const MAX_SCOPES: usize = 16;
pub(crate) const MAX_RESOURCES: usize = 64;
const MAX_RESOURCE_ID: usize = 256;

/// Proposals that may wait for the person at once.
pub(crate) const MAX_PENDING: i64 = 5;

/// How the service takes an agent's id, an incarnation or a resource's id:
/// letters, digits and `- _ . :`.
pub(crate) fn valid_name(text: &str, max: usize) -> bool {
    !text.is_empty() && text.len() <= max && text.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

pub(crate) fn valid_resource_id(id: &str) -> bool {
    valid_name(id, MAX_RESOURCE_ID)
}

const INCARNATION: &str = "incarnation";

fn well_formed(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn read_incarnation(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim();
    well_formed(text).then(|| text.to_owned())
}

/// The agent's incarnation, if it has one yet; never makes one.
pub(crate) fn current_incarnation(home: &Home, agent: &str) -> Option<String> {
    read_incarnation(&home.agent_dir(agent).join(INCARNATION))
}

/// The agent's incarnation, made the first time. Two callers at once get
/// the same one: a new one is written aside and linked into place, which
/// fails if another got there first. One that doesn't read (changed by
/// hand) is replaced: the agent's grants stop matching, which is the safe
/// way to fail.
pub(crate) fn incarnation(home: &Home, agent: &str) -> std::io::Result<String> {
    let dir = home.agent_dir(agent);
    let path = dir.join(INCARNATION);
    if let Some(found) = read_incarnation(&path) {
        return Ok(found);
    }
    std::fs::create_dir_all(&dir)?;
    let fresh = crate::encoding::hex(&crate::vault::crypto::random::<16>());
    let aside = dir.join(format!(".{INCARNATION}-{}", crate::encoding::hex(&crate::vault::crypto::random::<6>())));
    let written = (|| {
        let mut file = std::fs::File::create(&aside)?;
        file.write_all(format!("{fresh}\n").as_bytes())?;
        file.sync_all()
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&aside);
        return Err(error);
    }
    let linked = std::fs::hard_link(&aside, &path);
    let result = match linked {
        Ok(()) => Ok(fresh),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => match read_incarnation(&path) {
            Some(theirs) => Ok(theirs),
            None => {
                tracing::warn!(%agent, "an agent's incarnation didn't read; it has a new one, and its app grants stop matching");
                std::fs::rename(&aside, &path).map(|()| fresh)
            }
        },
        Err(error) => Err(error),
    };
    let _ = std::fs::remove_file(&aside);
    result
}

/// A proposal, as kept.
struct Row {
    id: String,
    agent: String,
    grantee: String,
    incarnation: String,
    app: String,
    scopes: Vec<String>,
    resources: Vec<Resource>,
    expires: i64,
    reason: String,
    state: State,
    created: i64,
    decided: Option<i64>,
    comment: String,
    error: String,
    grant_id: String,
    read_pipes: bool,
}

const COLUMNS: &str = "id, agent, grantee, incarnation, app, scopes, resources, expires, reason, state, created, decided, comment, error, grant_id, read_pipes";

fn state_of(word: &str) -> State {
    match word {
        "granted" => State::Granted,
        "declined" | "cleared" => State::Declined,
        "failed" => State::Failed,
        _ => State::Pending,
    }
}

impl Row {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
        let scopes: String = row.get(5)?;
        let resources: String = row.get(6)?;
        let state: String = row.get(9)?;
        Ok(Row {
            id: row.get(0)?,
            agent: row.get(1)?,
            grantee: row.get(2)?,
            incarnation: row.get(3)?,
            app: row.get(4)?,
            scopes: serde_json::from_str(&scopes).unwrap_or_default(),
            resources: serde_json::from_str(&resources).unwrap_or_default(),
            expires: row.get(7)?,
            reason: row.get(8)?,
            state: state_of(&state),
            created: row.get(10)?,
            decided: row.get(11)?,
            comment: row.get(12)?,
            error: row.get(13)?,
            grant_id: row.get(14)?,
            read_pipes: row.get(15)?,
        })
    }

    fn to_proto(&self) -> v1::AppGrantProposal {
        card(&Card {
            id: self.id.clone(),
            app: self.app.clone(),
            agent: self.grantee.clone(),
            scopes: self.scopes.clone(),
            resources: self.resources.clone(),
            expires_ms: self.expires,
            reason: self.reason.clone(),
            created_ms: self.created,
            read_pipes: self.read_pipes,
        })
        .map(|card| v1::AppGrantProposal {
            state: self.state.into(),
            decided_ms: self.decided.unwrap_or_default(),
            comment: self.comment.clone(),
            error: self.error.clone(),
            grant_id: self.grant_id.clone(),
            ..card
        })
        .unwrap_or_default()
    }
}

/// What a proposal's card is made from: kept in factory.db, and in the
/// tool result's details, which show it after the call.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Card {
    pub id: String,
    pub app: String,
    pub agent: String,
    pub scopes: Vec<String>,
    pub resources: Vec<Resource>,
    pub expires_ms: i64,
    pub reason: String,
    pub created_ms: i64,
    /// The grantee has read a web page (a pipe, before #346).
    #[serde(default)]
    pub read_pipes: bool,
}

/// The card, waiting, worded for the person.
fn card(card: &Card) -> Option<v1::AppGrantProposal> {
    let app = app(&card.app)?;
    let scopes = card
        .scopes
        .iter()
        .map(|scope| {
            let label = app.scopes.iter().find(|(id, _)| id == scope).map_or(scope.as_str(), |(_, label)| label);
            v1::AppGrantScope { id: scope.clone(), label: label.to_owned() }
        })
        .collect();
    let resources = card
        .resources
        .iter()
        .map(|resource| v1::AppGrantResource { kind: resource.kind.clone(), id: resource.id.clone(), label: resource.id.clone() })
        .collect();
    Some(v1::AppGrantProposal {
        id: card.id.clone(),
        app_id: app.id.to_owned(),
        app_name: app.name.to_owned(),
        agent_id: card.agent.clone(),
        scopes,
        resources,
        expires_ms: card.expires_ms,
        reason: card.reason.clone(),
        state: State::Pending.into(),
        created_ms: card.created_ms,
        grantee_read_pipes: card.read_pipes,
        ..Default::default()
    })
}

/// The tool whose result shows a card.
pub(crate) const TOOL: &str = "app_grants_propose";

/// The card an app_grants_propose result shows (called directly, or
/// through extension_call), waiting; None for any other result.
pub(crate) fn proposed(tool: &str, is_error: bool, details: &serde_json::Value) -> Option<v1::AppGrantProposal> {
    if !matches!(tool, TOOL | "extension_call") || is_error {
        return None;
    }
    let shown: Card = serde_json::from_value(details.get("app_grant")?.clone()).ok()?;
    card(&shown)
}

/// Keeps a new proposal, waiting. Refused when `agent` has too many waiting.
pub(crate) fn open(store: &Store, agent: &str, incarnation: &str, shown: &Card) -> Result<(), String> {
    let db = store.db();
    let waiting: i64 = db
        .query_row("SELECT COUNT(*) FROM app_grant_proposals WHERE agent = ?1 AND state = 'pending'", [agent], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if waiting >= MAX_PENDING {
        return Err(format!("{waiting} of your proposals already wait for the person. Wait for them to decide before proposing more."));
    }
    db.execute(
        &format!(
            "INSERT INTO app_grant_proposals ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'pending', ?10, NULL, '', '', '', ?11)"
        ),
        params![
            shown.id,
            agent,
            shown.agent,
            incarnation,
            shown.app,
            serde_json::to_string(&shown.scopes).unwrap_or_default(),
            serde_json::to_string(&shown.resources).unwrap_or_default(),
            shown.expires_ms,
            shown.reason,
            shown.created_ms,
            shown.read_pipes,
        ],
    )
    .map_err(|error| format!("Couldn't keep the proposal: {error}"))?;
    Ok(())
}

fn row(store: &Store, id: &str) -> Option<Row> {
    store.db().query_row(&format!("SELECT {COLUMNS} FROM app_grant_proposals WHERE id = ?1"), [id], Row::read).optional().ok().flatten()
}

/// Shows the agent's proposals as they are in its just-loaded conversation.
pub(crate) fn restore(store: &Store, agent: &str, transcript: &mut crate::transcript::Transcript) -> rusqlite::Result<()> {
    let db = store.db();
    let mut statement = db.prepare(&format!("SELECT {COLUMNS} FROM app_grant_proposals WHERE agent = ?1 ORDER BY created"))?;
    for row in statement.query_map([agent], Row::read)? {
        transcript.set_app_grant(row?.to_proto());
    }
    Ok(())
}

/// Whether a proposal of the agent's waits for the person.
pub(crate) fn pending_of(store: &Store, agent: &str) -> bool {
    store
        .db()
        .query_row("SELECT EXISTS (SELECT 1 FROM app_grant_proposals WHERE agent = ?1 AND state = 'pending')", [agent], |row| row.get(0))
        .unwrap_or(false)
}

/// Rivet's conversation was cleared: its waiting proposals can't be seen
/// any more, so they're closed as 'cleared' (shown as declined, and told
/// apart from the person's decline in Needs you's History), without telling
/// it.
pub(crate) fn cleared(store: &Store, agent: &str) -> rusqlite::Result<()> {
    store.db().execute(
        "UPDATE app_grant_proposals SET state = 'cleared', decided = ?2 WHERE agent = ?1 AND state = 'pending'",
        params![agent, now_ms()],
    )?;
    Ok(())
}

/// The agent's grants, as the mirror has them: (app, scopes, resources).
pub(crate) fn held(store: &Store, agent: &str) -> Vec<(String, Vec<String>, Vec<Resource>)> {
    let db = store.db();
    let Ok(mut statement) = db.prepare("SELECT app, scopes, resources FROM app_grants WHERE agent = ?1 ORDER BY app") else {
        return Vec::new();
    };
    statement
        .query_map([agent], |row| {
            let (app, scopes, resources): (String, String, String) = (row.get(0)?, row.get(1)?, row.get(2)?);
            Ok((app, serde_json::from_str(&scopes).unwrap_or_default(), serde_json::from_str(&resources).unwrap_or_default()))
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

fn mirror(store: &Store, grant: &crate::account::apps::Grant) -> rusqlite::Result<()> {
    store.db().execute(
        "INSERT OR REPLACE INTO app_grants (id, app, agent, incarnation, scopes, resources, granted, expires)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            grant.id,
            grant.app,
            grant.agent.id,
            grant.agent.incarnation,
            serde_json::to_string(&grant.scopes).unwrap_or_default(),
            serde_json::to_string(&grant.resources).unwrap_or_default(),
            grant.granted_ms,
            grant.expires_ms,
        ],
    )?;
    Ok(())
}

/// One decision, check-in or revoke at a time, so a grant made while the
/// mirror is reconciled isn't lost from it.
static BUSY: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// DecideAppGrant: the person approves Rivet's proposal at `seq`, and the
/// service makes the grant, or they decline it. Its card shows the
/// decision, and Rivet is told.
pub(crate) async fn decide(factory: &Arc<Factory>, request: &v1::DecideAppGrant) -> Result<(), RequestError> {
    let agent = request.agent_id.as_str();
    let comment = crate::decisions::comment(&request.comment)?;
    factory.known_agent(agent)?;
    factory.ensure_loaded(agent).await?;
    let id = {
        let inner = factory.lock();
        let entry = inner
            .agents
            .get(agent)
            .and_then(|runtime| runtime.transcript.as_ref())
            .and_then(|transcript| transcript.entries().iter().find(|entry| entry.seq == request.seq).cloned());
        match entry.and_then(|entry| entry.kind) {
            Some(v1::entry::Kind::AppGrant(proposal)) => proposal.id,
            _ => return Err(RequestError::Proposal("There's no such proposal in this conversation.".to_owned())),
        }
    };
    let _busy = BUSY.lock().await;
    let Some(row) = row(&factory.store, &id).filter(|row| row.agent == agent) else {
        return Err(RequestError::Proposal("There's no such proposal.".to_owned()));
    };
    if row.state != State::Pending {
        return Ok(());
    }
    if !request.approve {
        if let Some(row) = end(factory, &row.id, State::Declined, comment, "", "") {
            tell(factory, &row);
        }
        return Ok(());
    }
    // The agent as it was when proposed: one deleted and made again since
    // is another agent.
    let same = {
        let inner = factory.lock();
        let configured = inner.config.agent(&row.grantee).is_some();
        configured && current_incarnation(&factory.home, &row.grantee).as_deref() == Some(row.incarnation.as_str())
    };
    let name = agent_name(factory, &row.grantee);
    if !same {
        let error = format!("{name} was deleted since Rivet suggested this, so nothing was given.");
        if let Some(row) = end(factory, &row.id, State::Failed, comment, &error, "") {
            tell(factory, &row);
        }
        return Ok(());
    }
    // A liaison, limited to a pipe, would bridge the other Claktory to the
    // person's app (Fugo's review).
    if factory.pipes.quarantined(&row.grantee) {
        let error = format!("{name} is limited to a pipe now, so it can't be given access to your apps. Nothing was given.");
        if let Some(row) = end(factory, &row.id, State::Failed, comment, &error, "") {
            tell(factory, &row);
        }
        return Ok(());
    }
    // It read a web page after the card was shown: the card says so now, and
    // the person decides again, knowing (Fugo's review).
    if !row.read_pipes && !factory.pipes.tainted(&row.grantee).is_empty() {
        factory.store.db().execute("UPDATE app_grant_proposals SET read_pipes = 1 WHERE id = ?1", [&row.id])?;
        reshow(factory, &row.id);
        return Err(RequestError::Proposal(format!(
            "{name} has read {} since Rivet suggested this. Look at it again.",
            crate::pipes::agents::what_was_read(factory, &row.grantee)
        )));
    }
    if row.expires != 0 && row.expires <= now_ms() {
        let error = "The access would already have ended, so nothing was given.";
        if let Some(row) = end(factory, &row.id, State::Failed, comment, error, "") {
            tell(factory, &row);
        }
        return Ok(());
    }
    let put = Put {
        app: row.app.clone(),
        agent: Actor { id: row.grantee.clone(), incarnation: row.incarnation.clone() },
        scopes: row.scopes.clone(),
        resources: row.resources.clone(),
        expires_ms: row.expires,
    };
    let service = factory.account.service_name();
    match factory.account.put_grant(&put).await {
        Ok(grant) => {
            // It replaced the agent's earlier grant in that app.
            let replaced = factory.store.db().execute(
                "DELETE FROM app_grants WHERE app = ?1 AND agent = ?2 AND incarnation = ?3",
                params![grant.app, grant.agent.id, grant.agent.incarnation],
            );
            if let Err(error) = replaced.and_then(|_| mirror(&factory.store, &grant)) {
                tracing::error!(%error, "an app grant was made but isn't in the mirror until the next check-in");
            }
            // Its tools first, so the agent told of it next has them.
            dms::granted(factory, &grant.agent.id, &grant.app).await;
            if let Some(row) = end(factory, &row.id, State::Granted, comment, "", &grant.id) {
                tell(factory, &row);
            }
            Ok(())
        }
        Err(Problem::SignedOut) => Err(RequestError::Proposal(format!(
            "This Claktory is signed out of your {service} account. Sign in again in Settings → Account, then approve it."
        ))),
        Err(Problem::ForNow(why)) => {
            tracing::info!(%why, "an app grant couldn't be made for now");
            Err(RequestError::Unreachable(format!("Couldn't reach {service} to give the access. Try again in a little while.")))
        }
        Err(Problem::Busy(why)) => {
            tracing::info!(%why, "the account service asked to wait with an app grant");
            Err(RequestError::Unreachable(format!("{service} is busy right now. Try again in a minute.")))
        }
        Err(Problem::Refused(status, words)) => {
            tracing::warn!(status, %words, "the account service refused an app grant");
            let error = format!("{service} refused to give it: {words}");
            if let Some(row) = end(factory, &row.id, State::Failed, comment, &error, "") {
                tell(factory, &row);
            }
            Ok(())
        }
    }
}

fn agent_name(factory: &Factory, id: &str) -> String {
    factory.lock().config.agent(id).map_or_else(|| id.to_owned(), |agent| agent.name.clone())
}

/// Ends a waiting proposal, shows it as it is now, and returns it; None if
/// it wasn't waiting.
fn end(factory: &Factory, id: &str, state: State, comment: &str, error: &str, grant: &str) -> Option<Row> {
    let word = match state {
        State::Granted => "granted",
        State::Declined => "declined",
        _ => "failed",
    };
    let changed = factory
        .store
        .db()
        .execute(
            "UPDATE app_grant_proposals SET state = ?2, decided = ?3, comment = ?4, error = ?5, grant_id = ?6
             WHERE id = ?1 AND state = 'pending'",
            params![id, word, now_ms(), comment, error, grant],
        )
        .unwrap_or(0);
    if changed == 0 {
        return None;
    }
    let row = row(&factory.store, id)?;
    let mut inner = factory.lock();
    let entry = inner
        .agents
        .get_mut(&row.agent)
        .and_then(|runtime| runtime.transcript.as_mut())
        .and_then(|transcript| transcript.set_app_grant(row.to_proto()));
    if let Some(entry) = entry {
        factory.send_change(&inner, &row.agent, crate::transcript::Change::Entry(entry));
        // It no longer waits for the person.
        factory.broadcast_workspace(&inner);
    }
    drop(inner);
    Some(row)
}

/// Shows a proposal as it is now in its agent's conversation.
fn reshow(factory: &Factory, id: &str) {
    let Some(row) = row(&factory.store, id) else { return };
    let mut inner = factory.lock();
    let entry = inner
        .agents
        .get_mut(&row.agent)
        .and_then(|runtime| runtime.transcript.as_mut())
        .and_then(|transcript| transcript.set_app_grant(row.to_proto()));
    if let Some(entry) = entry {
        factory.send_change(&inner, &row.agent, crate::transcript::Change::Entry(entry));
    }
}

/// Tells Rivet how the person decided.
fn tell(factory: &Arc<Factory>, row: &Row) {
    let person = factory.lock().config.person_capitalized().to_owned();
    let name = agent_name(factory, &row.grantee);
    let app = app(&row.app).map_or(row.app.as_str(), |app| app.name);
    let (summary, text) = match row.state {
        State::Granted => {
            let until = if row.expires == 0 { "until they take it back".to_owned() } else { format!("until {}", day(row.expires)) };
            let summary = format!("{person} gave {name} access to {app}");
            let text = format!("{summary} ({}), {until}.{}", row.scopes.join(", "), crate::decisions::note(&person, &row.comment));
            told_grantee(factory, row, &person, app, &until);
            (summary, text)
        }
        State::Declined => {
            let summary = format!("{person} declined giving {name} access to {app}");
            let text = format!("{summary}. Nothing was given.{}", crate::decisions::note(&person, &row.comment));
            (summary, text)
        }
        _ => {
            let summary = format!("{person} approved giving {name} access to {app}, but it couldn't be given");
            (summary.clone(), format!("{summary}: {}{}", row.error, crate::decisions::note(&person, &row.comment)))
        }
    };
    factory.deliver_event(&row.agent, text, host_event(summary));
}

/// The agent given access hears it, in Claktory's words: the app, what it
/// may do there and on what, and until when.
fn told_grantee(factory: &Arc<Factory>, row: &Row, person: &str, app: &str, until: &str) {
    let Some(known) = self::app(&row.app) else { return };
    let can: Vec<&str> = row
        .scopes
        .iter()
        .map(|scope| known.scopes.iter().find(|(id, _)| id == scope).map_or(scope.as_str(), |(_, label)| *label))
        .collect();
    let on: Vec<String> = row.resources.iter().map(|resource| format!("{} {}", resource.kind, resource.id)).collect();
    let summary = format!("{person} gave you access to {app}");
    let mut text = format!("{summary}, {until}: {}", can.join("; "));
    if !on.is_empty() {
        text.push_str(&format!(" ({})", on.join(", ")));
    }
    text.push('.');
    factory.deliver_event(&row.grantee, text, host_event(summary));
}

fn day(ms: i64) -> String {
    jiff::Timestamp::from_millisecond(ms)
        .map(|at| at.to_zoned_datetime(jiff::tz::TimeZone::system()).strftime("%-d %b %Y").to_string())
        .unwrap_or_default()
}

/// At check-in: the mirror becomes the service's list, and grants of agents
/// that are gone, or of another incarnation, are revoked there.
pub(crate) async fn reconcile(factory: &Arc<Factory>) {
    let _busy = BUSY.lock().await;
    let grants = match factory.account.grants().await {
        Ok(grants) => grants,
        // The service's apps may not run (503), or not yet: nothing to mirror.
        Err(problem) => {
            tracing::debug!(?problem, "app grants weren't listed");
            return;
        }
    };
    let mut kept = Vec::new();
    for grant in grants {
        let live = {
            let inner = factory.lock();
            inner.config.agent(&grant.agent.id).is_some()
                && current_incarnation(&factory.home, &grant.agent.id).as_deref() == Some(grant.agent.incarnation.as_str())
        };
        if live {
            kept.push(grant);
        } else if let Err(problem) = factory.account.revoke_grant(&grant.id).await {
            tracing::info!(?problem, "a gone agent's app grant wasn't revoked; trying again at the next check-in");
        }
    }
    let replaced = (|| {
        let mut db = factory.store.db();
        let transaction = db.transaction()?;
        transaction.execute("DELETE FROM app_grants", [])?;
        for grant in &kept {
            transaction.execute(
                "INSERT OR REPLACE INTO app_grants (id, app, agent, incarnation, scopes, resources, granted, expires)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    grant.id,
                    grant.app,
                    grant.agent.id,
                    grant.agent.incarnation,
                    serde_json::to_string(&grant.scopes).unwrap_or_default(),
                    serde_json::to_string(&grant.resources).unwrap_or_default(),
                    grant.granted_ms,
                    grant.expires_ms,
                ],
            )?;
        }
        transaction.commit()
    })();
    if let Err(error) = replaced {
        tracing::error!(%error, "could not mirror the app grants");
    }
    dms::watch::wake(factory);
}

/// An agent is deleted: its proposals go, and its grants are revoked by a
/// check-in made now, which finds them without an agent (and, while the
/// service can't be reached, by the next one).
pub(crate) fn forget(factory: &Factory, agent: &str) -> rusqlite::Result<()> {
    let db = factory.store.db();
    db.execute("DELETE FROM app_grant_proposals WHERE agent = ?1", [agent])?;
    let held = db.execute("DELETE FROM app_grants WHERE agent = ?1", [agent])?;
    drop(db);
    dms::forget(factory, agent)?;
    if held > 0 {
        factory.account.check_in_soon();
    }
    Ok(())
}
