//! The leaf's loop (dms-e2ee.md §5.2): while this Claktory's leaf is
//! enrolled, it keeps its KeyPackages stocked at the worker, joins the
//! groups it's welcomed to, and processes every group it's in, in order,
//! each message in one step (its MLS state, its history row and the group's
//! position together). MLS state only moves forward through every commit
//! and message, so every group is processed, not only followed ones.
//!
//! - Woken by the worker's feed for this leaf (`GET claktory/events`):
//!   ids only, one long poll at a time.
//! - **Paused, never skipping,** while the vault is locked or the person's
//!   device hasn't enrolled it: positions don't move, and the 30-day mailbox
//!   covers the wait.
//! - A group the worker no longer lists for it (its grant ended, the
//!   friendship gone) is forgotten here, history and all.
//! - A conversation it's too far behind in is forgotten too (a device adds
//!   it again); its own channel it rejoins by external commit.

use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use claktory_dm_crypto::certificate::LeafKind;
use claktory_dm_crypto::content::{Content, kind};
use claktory_dm_crypto::envelope::Envelope;
use claktory_dm_crypto::group::{Group, Incoming};
use claktory_dm_crypto::validation::Pins;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Notify;

use super::leaf::{self, KeptGroup, StepError, Unready};
use super::store;
use crate::account::apps::Problem;
use crate::factory::Factory;

/// How long the worker may hold a call for events, and how long this side waits.
const WAIT: u64 = 25;
const CALL: Duration = Duration::from_secs(WAIT + 15);
/// Between looks while the leaf can't run.
const IDLE: Duration = Duration::from_secs(60);
/// Between looks while the vault is locked: opening it is all it waits for,
/// and that check is in memory.
const LOCKED: Duration = Duration::from_secs(3);
/// A full round (key packages, welcomes, every group) at least this often.
const ROUND: Duration = Duration::from_secs(600);
/// KeyPackages kept at the worker: topped up to `STOCK` below `LOW`.
const STOCK: i64 = 100;
const LOW: i64 = 30;
/// Messages asked for at a time.
const PAGE: i64 = 100;

fn notify() -> &'static Notify {
    static WAKE: OnceLock<Notify> = OnceLock::new();
    WAKE.get_or_init(Notify::new)
}

/// Something changed here (an enrollment): look now.
pub(crate) fn wake() {
    notify().notify_waiters();
}

#[derive(Debug)]
enum Stop {
    /// The leaf can't run now: wait.
    Unready(Unready),
    /// The worker or the network: try again later.
    Problem(Problem),
    Failed(String),
}

impl From<StepError> for Stop {
    fn from(error: StepError) -> Stop {
        match error {
            StepError::Unready(unready) => Stop::Unready(unready),
            StepError::Failed(error) => Stop::Failed(error),
        }
    }
}

impl From<Problem> for Stop {
    fn from(problem: Problem) -> Stop {
        Stop::Problem(problem)
    }
}

fn unreadable(error: impl std::fmt::Display) -> Stop {
    Stop::Problem(Problem::ForNow(format!("an unreadable answer: {error}")))
}

/// The leaf's call to the worker, as this Claktory (no agent).
async fn call(factory: &Factory, method: &str, route: &str, query: &[(&str, String)], body: Option<Value>) -> Result<Vec<u8>, Problem> {
    factory.account.dms(method, route, query, None, body, None).await
}

async fn get<T: serde::de::DeserializeOwned>(factory: &Factory, route: &str, query: &[(&str, String)]) -> Result<T, Stop> {
    let answer = call(factory, "GET", route, query, None).await?;
    serde_json::from_slice(&answer).map_err(unreadable)
}

pub(crate) async fn monitor(factory: Weak<Factory>) {
    crate::network::ready().await;
    let mut cursor: Option<String> = None;
    let mut last_round: Option<tokio::time::Instant> = None;
    let mut backoff = Duration::from_secs(30);
    loop {
        let Some(strong) = factory.upgrade() else { return };
        let ready = leaf::step(&strong, |step| step.leaf().map(drop));
        let pause = match ready {
            Err(StepError::Unready(Unready::Locked)) => Some(LOCKED),
            Err(StepError::Unready(_)) => Some(IDLE),
            Err(StepError::Failed(error)) => {
                tracing::warn!(%error, "the direct-message leaf can't run");
                Some(IDLE)
            }
            Ok(()) => {
                // The feed's cursor first, so nothing between it and the round is missed.
                let mut outcome = Ok(());
                if cursor.is_none() {
                    // Without the feed the round still runs; the feed is tried again after it.
                    if let Err(stop) = events(&strong, &mut cursor).await {
                        tracing::debug!(?stop, "the direct-message feed isn't there");
                    }
                    last_round = None;
                }
                if last_round.is_none_or(|at| at.elapsed() >= ROUND) {
                    outcome = full_round(&strong).await;
                    if outcome.is_ok() {
                        last_round = Some(tokio::time::Instant::now());
                    }
                }
                if outcome.is_ok() {
                    outcome = match events(&strong, &mut cursor).await {
                        Ok(happened) => follow(&strong, happened).await,
                        Err(error) => Err(error),
                    };
                }
                match outcome {
                    Ok(()) => {
                        backoff = Duration::from_secs(30);
                        None
                    }
                    Err(Stop::Unready(Unready::Locked)) => Some(LOCKED),
                    Err(Stop::Unready(_)) => Some(IDLE),
                    Err(stop) => {
                        match &stop {
                            Stop::Failed(error) => tracing::warn!(%error, "the direct-message leaf stopped for now"),
                            stop => tracing::debug!(?stop, "the direct-message leaf stopped for now"),
                        }
                        let pause = backoff;
                        backoff = (backoff * 2).min(Duration::from_secs(300));
                        Some(pause)
                    }
                }
            }
        };
        drop(strong);
        if let Some(pause) = pause {
            tokio::select! {
                () = tokio::time::sleep(pause) => {}
                () = notify().notified() => {}
            }
        }
    }
}

#[derive(Deserialize)]
struct Events {
    #[serde(default)]
    events: Vec<Event>,
    next_cursor: String,
}

#[derive(Debug, Deserialize)]
struct Event {
    kind: String,
    #[serde(default)]
    group_id: String,
}

/// What happened since the cursor: one long poll. Without a cursor, takes
/// one and reports nothing.
async fn events(factory: &Arc<Factory>, cursor: &mut Option<String>) -> Result<Vec<Event>, Stop> {
    let query: Vec<(&str, String)> = match cursor {
        Some(since) => vec![("since", since.clone()), ("wait", WAIT.to_string())],
        None => Vec::new(),
    };
    let answer = factory.account.dms("GET", "claktory/events", &query, None, None, Some(CALL)).await;
    let answer = match answer {
        Err(Problem::Refused(410, _)) => {
            // Too far behind the feed: start again from a full round.
            *cursor = None;
            return Ok(vec![Event { kind: "groups".to_owned(), group_id: String::new() }]);
        }
        other => other?,
    };
    let events: Events = serde_json::from_slice(&answer).map_err(unreadable)?;
    let had = cursor.is_some();
    *cursor = Some(events.next_cursor);
    Ok(if had { events.events } else { Vec::new() })
}

/// Acts on the feed's events.
async fn follow(factory: &Arc<Factory>, events: Vec<Event>) -> Result<(), Stop> {
    if events.iter().any(|event| event.kind == "groups") {
        return full_round(factory).await;
    }
    if events.iter().any(|event| event.kind == "welcome") {
        welcomes(factory).await?;
    }
    let mut seen = Vec::new();
    for event in events.iter().filter(|event| event.kind == "group_message") {
        if seen.contains(&event.group_id) {
            continue;
        }
        seen.push(event.group_id.clone());
        let Ok(group_id) = URL_SAFE_NO_PAD.decode(&event.group_id) else { continue };
        let kept = leaf::step(factory, |step| Ok(step.groups()?.into_iter().find(|group| group.group_id == group_id)))?;
        if let Some(group) = kept {
            catch_up(factory, &group).await?;
        }
    }
    Ok(())
}

/// Everything: KeyPackages, Welcomes, which groups, and each one.
async fn full_round(factory: &Arc<Factory>) -> Result<(), Stop> {
    if let Err(stop) = key_packages(factory).await {
        // The person's devices may not list this leaf yet: the rest still runs.
        tracing::debug!(?stop, "this Claktory's KeyPackages weren't topped up");
    }
    let peers: Vec<String> = leaf::step(factory, |step| Ok(step.groups()?.into_iter().map(|group| group.peer).collect()))?;
    refresh_keys(factory, &peers).await?;
    welcomes(factory).await?;
    groups(factory).await?;
    for group in leaf::step(factory, |step| step.groups())? {
        catch_up(factory, &group).await?;
    }
    Ok(())
}

#[derive(Deserialize)]
struct Supply {
    remaining: i64,
    #[serde(default)]
    last_resort: bool,
}

/// Keeps the leaf's KeyPackages at the worker topped up: the last-resort
/// one first, so a leaf the worker won't take yet makes one, not a hundred.
async fn key_packages(factory: &Arc<Factory>) -> Result<(), Stop> {
    let supply: Supply = get(factory, "key-packages", &[]).await?;
    let mut body = json!({"key_packages": []});
    if !supply.last_resort {
        let package = leaf::step(factory, |step| {
            let leaf = step.leaf()?;
            Ok(Envelope::new(leaf.me.last_resort_key_package(&step.provider()?)?)?)
        })?;
        body["last_resort"] = package.to_json();
        call(factory, "POST", "key-packages", &[], Some(body.clone())).await?;
        body = json!({"key_packages": []});
    }
    if supply.remaining >= LOW {
        return Ok(());
    }
    let packages = leaf::step(factory, |step| {
        let leaf = step.leaf()?;
        let provider = step.provider()?;
        (supply.remaining..STOCK).map(|_| Ok(Envelope::new(leaf.me.key_package(&provider)?)?.to_json())).collect::<Result<Vec<_>, StepError>>()
    })?;
    body["key_packages"] = Value::Array(packages);
    call(factory, "POST", "key-packages", &[], Some(body)).await?;
    Ok(())
}

#[derive(Deserialize)]
struct Keys {
    #[serde(default)]
    device_list: String,
}

/// The latest signed device lists of this Claktory's own account and of
/// `accounts`, from the worker: taken only when signed by the identity the
/// person's devices pinned (a key the service swapped is never adopted
/// here; the person's devices tell this Claktory of a real change).
async fn refresh_keys(factory: &Arc<Factory>, accounts: &[String]) -> Result<(), Stop> {
    let own = leaf::step(factory, |step| Ok(step.registration().0.clone()))?;
    let mut wanted: Vec<&str> = std::iter::once(own.as_str()).chain(accounts.iter().map(String::as_str)).filter(|account| !account.is_empty()).collect();
    wanted.sort_unstable();
    wanted.dedup();
    for account in wanted {
        if !super::valid_id(account) {
            continue;
        }
        let keys: Keys = match get(factory, &format!("keys/{account}"), &[]).await {
            Err(Stop::Problem(Problem::Refused(403 | 404 | 409, _))) => continue,
            other => other?,
        };
        let Ok(list) = URL_SAFE_NO_PAD.decode(&keys.device_list) else { continue };
        let accepted = leaf::step(factory, |step| step.accept_list(account, &list));
        if let Err(error) = accepted {
            tracing::debug!(?error, "a device list this Claktory didn't take");
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct Welcomes {
    #[serde(default)]
    welcomes: Vec<Welcome>,
}

#[derive(Deserialize)]
struct Welcome {
    id: String,
    group_id: String,
    kind: String,
    #[serde(default)]
    conversation_id: String,
    #[serde(default)]
    contact_generation: i64,
    #[serde(default)]
    claktory_id: String,
    #[serde(default)]
    accounts: Vec<String>,
    welcome: Value,
    /// The commit that added this leaf: it reads on after it.
    #[serde(default)]
    seq: i64,
}

/// Joins the groups this leaf is welcomed to: its channel, and the
/// conversations the person gave its agents.
async fn welcomes(factory: &Arc<Factory>) -> Result<(), Stop> {
    let listed: Welcomes = get(factory, "welcomes", &[]).await?;
    if listed.welcomes.is_empty() {
        return Ok(());
    }
    // A device pins a friend and tells the channel before it adds this leaf
    // to their conversation: read the channel first, or the friend isn't
    // pinned yet and the Welcome is lost.
    if let Err(error) = catch_up_channel(factory).await {
        tracing::debug!(?error, "the channel wasn't read before the Welcomes");
    }
    let peers: Vec<String> = listed.welcomes.iter().flat_map(|welcome| welcome.accounts.clone()).collect();
    if !peers.is_empty() {
        refresh_keys(factory, &peers).await?;
    }
    for welcome in listed.welcomes {
        let joined = leaf::step(factory, |step| join(step, &welcome));
        match joined {
            Ok(()) => {}
            Err(StepError::Unready(unready)) => return Err(Stop::Unready(unready)),
            // One that doesn't join never will: it goes.
            Err(StepError::Failed(error)) => tracing::warn!(%error, kind = %welcome.kind, "a Welcome for this Claktory was refused"),
        }
        match call(factory, "DELETE", &format!("welcomes/{}", welcome.id), &[], None).await {
            Ok(_) | Err(Problem::Refused(404, _)) => {}
            Err(problem) => return Err(problem.into()),
        }
    }
    Ok(())
}

fn join(step: &mut leaf::Step<'_>, welcome: &Welcome) -> Result<(), StepError> {
    let leaf = step.leaf()?;
    let group_id = URL_SAFE_NO_PAD.decode(&welcome.group_id).map_err(|_| StepError::Failed("a group id that doesn't read".to_owned()))?;
    if step.groups()?.iter().any(|group| group.group_id == group_id) {
        return Ok(());
    }
    let kept = match welcome.kind.as_str() {
        "channel" if welcome.claktory_id == leaf.claktory_id => KeptGroup {
            group_id: group_id.clone(),
            channel: true,
            conversation: String::new(),
            generation: 0,
            peer: String::new(),
            position: welcome.seq,
        },
        "conversation" if welcome.accounts.len() == 2 && welcome.accounts.contains(&leaf.account_id) => {
            let peer = welcome.accounts.iter().find(|account| **account != leaf.account_id).cloned().unwrap_or_default();
            if peer.is_empty() {
                return Err(StepError::Failed("a Welcome to a conversation with no friend in it".to_owned()));
            }
            KeptGroup {
                group_id: group_id.clone(),
                channel: false,
                conversation: welcome.conversation_id.clone(),
                generation: welcome.contact_generation,
                peer,
                position: welcome.seq,
            }
        }
        other => return Err(StepError::Failed(format!("a Welcome to a {other} group this leaf isn't for"))),
    };
    if !super::valid_id(&kept.conversation) && !kept.channel {
        return Err(StepError::Failed("a Welcome to a conversation whose id doesn't read".to_owned()));
    }
    let bytes = Envelope::from_json(&welcome.welcome)?;
    let kind = step.kind_of(&leaf, &kept);
    let group = Group::join(&step.provider()?, &leaf.pins, kind, &group_id, bytes.bytes())?;
    // The friend comes from the service's labels: the tree itself has to
    // hold exactly the person and that friend, or a service could swap two
    // granted conversations' labels (Fugo). Refused, the transaction takes
    // the join back.
    if !kept.channel {
        let mut accounts = std::collections::BTreeSet::new();
        for member in group.members(&leaf.pins) {
            accounts.insert(member?.account().to_owned());
        }
        let expected = std::collections::BTreeSet::from([leaf.account_id.clone(), kept.peer.clone()]);
        if accounts != expected {
            return Err(StepError::Failed("a Welcome to a conversation whose people aren't the ones the service named".to_owned()));
        }
    }
    step.keep_group(&kept)?;
    Ok(())
}

#[derive(Deserialize)]
struct Groups {
    #[serde(default)]
    groups: Vec<Listed>,
}

#[derive(Deserialize)]
struct Listed {
    group_id: String,
}

/// Forgets the groups the worker no longer lists for this leaf.
async fn groups(factory: &Arc<Factory>) -> Result<(), Stop> {
    let listed: Groups = get(factory, "groups", &[]).await?;
    let listed: Vec<Vec<u8>> = listed.groups.iter().filter_map(|group| URL_SAFE_NO_PAD.decode(&group.group_id).ok()).collect();
    let gone: Vec<KeptGroup> =
        leaf::step(factory, |step| Ok(step.groups()?.into_iter().filter(|group| !listed.contains(&group.group_id)).collect()))?;
    for group in gone {
        tracing::info!(channel = group.channel, "a direct-message group this Claktory can't reach any more is forgotten");
        // It asks to be removed, for the next device commit to take in
        // (the person's device usually has already); then it forgets.
        let leave = leaf::step(factory, |step| {
            let leaf = step.leaf()?;
            let provider = step.provider()?;
            let Some(mut mls) = Group::load(&provider, step.kind_of(&leaf, &group), &group.group_id)? else { return Ok(None) };
            Ok(Some(Envelope::new(mls.leave(&provider, &leaf.me)?)?))
        });
        if let Ok(Some(proposal)) = leave {
            let route = format!("groups/{}/proposals", URL_SAFE_NO_PAD.encode(&group.group_id));
            if let Err(problem) = call(factory, "POST", &route, &[], Some(json!({"proposal": proposal.to_json()}))).await {
                tracing::debug!(?problem, "this Claktory's leaving wasn't taken");
            }
        }
        leaf::step(factory, |step| Ok(store::forget_group(step.db(), &group.group_id)?))?;
    }
    Ok(())
}

#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    messages: Vec<Message>,
    #[serde(default)]
    has_more: bool,
}

#[derive(Deserialize)]
struct Message {
    seq: i64,
    /// "application", "commit" or "proposal", as the service says.
    #[serde(default)]
    kind: String,
    envelope: Value,
    #[serde(default)]
    accepted_ms: i64,
    #[serde(default)]
    epoch: u64,
}

/// What processing one message did that matters after its step.
#[derive(Default)]
struct Done {
    /// A commit in the channel added someone: a device that can't read
    /// what was proposed before it.
    added_in_channel: bool,
}

/// Processes `group` from its position to the end of its mailbox.
async fn catch_up(factory: &Arc<Factory>, group: &KeptGroup) -> Result<(), Stop> {
    let id = URL_SAFE_NO_PAD.encode(&group.group_id);
    loop {
        let position = leaf::step(factory, |step| Ok(store::position(step.db(), &group.group_id)?))?;
        let Some(position) = position else { return Ok(()) };
        let route = format!("groups/{id}/messages");
        let page: Page = match get(factory, &route, &[("after", position.to_string()), ("limit", PAGE.to_string())]).await {
            Err(Stop::Problem(Problem::Refused(410, _))) => return behind(factory, group).await,
            other => other?,
        };
        let mut added = false;
        for message in &page.messages {
            let done = leaf::step(factory, |step| process(step, group, message))?;
            added |= done.added_in_channel;
        }
        if added {
            super::sends::reposted(factory).await;
        }
        if !page.has_more || page.messages.is_empty() {
            return Ok(());
        }
    }
}

/// One message, in one step: whatever happens, the position moves past it,
/// so one bad message never stops a group.
fn process(step: &mut leaf::Step<'_>, kept: &KeptGroup, message: &Message) -> Result<Done, StepError> {
    // Read already: the loop and an agent's read may fetch the same page.
    if store::position(step.db(), &kept.group_id)?.is_none_or(|position| message.seq <= position) {
        return Ok(Done::default());
    }
    let leaf = step.leaf()?;
    let kind = step.kind_of(&leaf, kept);
    let provider = step.provider()?;
    let Some(mut group) = Group::load(&provider, kind, &kept.group_id)? else { return Ok(Done::default()) };
    let mut done = Done::default();
    // A commit or proposal from an earlier epoch: from before this leaf
    // joined, or its own commit, already merged. `epoch` is the service's
    // figure, so a lie there can only drop a message, never put one in.
    // Application messages always go to MLS, which reads what's from an
    // epoch it still holds and refuses the rest.
    if message.kind == "application" || message.epoch >= group.epoch() {
        let before = group.members(&leaf.pins).len();
        let bytes = Envelope::from_json(&message.envelope).map(Envelope::into_bytes);
        match bytes.map_err(StepError::from).and_then(|bytes| Ok(group.process(&provider, &leaf.pins, &bytes)?)) {
            Ok(Incoming::Application { from, content }) => {
                received(step, &leaf, kept, message, &from, &content)?;
            }
            Ok(Incoming::Committed { .. }) => done.added_in_channel = kept.channel && group.members(&leaf.pins).len() > before,
            Ok(Incoming::Proposal) => {}
            Err(error) => tracing::warn!(?error, seq = message.seq, channel = kept.channel, "a direct message this Claktory couldn't take"),
        }
    }
    store::set_position(step.db(), &kept.group_id, message.seq)?;
    Ok(done)
}

/// What someone said: kept in the conversation's history, or, in the
/// channel, what the person's devices tell this Claktory.
fn received(
    step: &mut leaf::Step<'_>,
    leaf: &leaf::Leaf,
    kept: &KeptGroup,
    message: &Message,
    from: &claktory_dm_crypto::validation::Member,
    content: &[u8],
) -> Result<(), StepError> {
    let content = Content::decode(content)?;
    if kept.channel {
        // Pins only from the person's own devices, merged as at enrollment.
        if content.kind == kind::PINS && from.kind() == LeafKind::Device && from.account() == leaf.account_id {
            let given: Pins = content
                .extra
                .get("pins")
                .cloned()
                .and_then(|pins| serde_json::from_value(pins).ok())
                .ok_or_else(|| StepError::Failed("pins that don't read".to_owned()))?;
            let mut given = given;
            given.reparse()?;
            if let Err(error) = step.import_pins(&given) {
                tracing::warn!(?error, "pins from the person's device disagree with this Claktory's: kept its own");
            }
        }
        return Ok(());
    }
    if content.kind != kind::TEXT {
        return Ok(());
    }
    let row = json!({
        "from_account": from.account(),
        "from_leaf": from.leaf_id(),
        "content": content,
    });
    let place: [&[u8]; 3] = [b"history", &kept.group_id, &message.seq.to_be_bytes()];
    let sealed = step.seal(&place, &serde_json::to_vec(&row).map_err(|error| StepError::Failed(error.to_string()))?)?;
    step.db().execute(
        "INSERT OR REPLACE INTO history (group_id, seq, accepted, sealed) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![kept.group_id, message.seq, message.accepted_ms, sealed],
    )?;
    Ok(())
}

/// Too far behind a group's mailbox: its own channel it rejoins by external
/// commit; a conversation is forgotten until a device adds it again.
async fn behind(factory: &Arc<Factory>, group: &KeptGroup) -> Result<(), Stop> {
    if !group.channel {
        leaf::step(factory, |step| Ok(store::forget_group(step.db(), &group.group_id)?))?;
        return Ok(());
    }
    let id = URL_SAFE_NO_PAD.encode(&group.group_id);
    #[derive(Deserialize)]
    struct Info {
        group_info: Value,
    }
    let info: Info = get(factory, &format!("groups/{id}/group-info"), &[]).await?;
    let info = Envelope::from_json(&info.group_info).map_err(unreadable)?;
    let outgoing = leaf::step(factory, |step| {
        let leaf = step.leaf()?;
        store::forget_group(step.db(), &group.group_id)?;
        let (_, outgoing) = Group::rejoin(&step.provider()?, &leaf.me, &leaf.pins, step.kind_of(&leaf, group), info.bytes())?;
        Ok(outgoing)
    })?;
    let mut body = json!({"commit": Envelope::new(outgoing.commit).map_err(unreadable)?.to_json()});
    if let Some(info) = outgoing.group_info {
        body["group_info"] = Envelope::new(info).map_err(unreadable)?.to_json();
    }
    let taken = call(factory, "POST", &format!("groups/{id}/commits"), &[], Some(body)).await;
    #[derive(Deserialize)]
    struct Taken {
        seq: i64,
    }
    match taken {
        Ok(answer) => {
            let taken: Taken = serde_json::from_slice(&answer).map_err(unreadable)?;
            leaf::step(factory, |step| {
                let leaf = step.leaf()?;
                let provider = step.provider()?;
                let kind = step.kind_of(&leaf, group);
                if let Some(mut rejoined) = Group::load(&provider, kind, &group.group_id)? {
                    rejoined.merge_pending(&provider)?;
                }
                step.keep_group(&KeptGroup { position: taken.seq, ..group.clone() })?;
                Ok(())
            })?;
            Ok(())
        }
        Err(problem) => {
            // Not taken (another commit came first): the next round tries again.
            leaf::step(factory, |step| Ok(store::forget_group(step.db(), &group.group_id)?))?;
            Err(problem.into())
        }
    }
}

/// Processes a conversation's group up to now, before an agent reads it;
/// what fails is left to the loop.
pub(crate) async fn catch_up_conversation(factory: &Arc<Factory>, group_id: &str) {
    let Ok(group_id) = URL_SAFE_NO_PAD.decode(group_id) else { return };
    let kept = leaf::step(factory, |step| Ok(step.groups()?.into_iter().find(|group| group.group_id == group_id)));
    if let Ok(Some(group)) = kept
        && let Err(stop) = catch_up(factory, &group).await
    {
        tracing::debug!(?stop, "a conversation wasn't caught up before reading it");
    }
}

/// Processes this Claktory's channel up to now: for a send the worker
/// refused as made at an old epoch.
pub(crate) async fn catch_up_channel(factory: &Arc<Factory>) -> Result<(), StepError> {
    let channel = leaf::step(factory, |step| Ok(step.groups()?.into_iter().find(|group| group.channel)))?;
    let Some(channel) = channel else { return Err(StepError::Unready(Unready::NoChannel)) };
    match catch_up(factory, &channel).await {
        Ok(()) => Ok(()),
        Err(Stop::Unready(unready)) => Err(StepError::Unready(unready)),
        Err(stop) => Err(StepError::Failed(format!("{stop:?}"))),
    }
}

