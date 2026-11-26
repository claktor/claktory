//! The person's devices and their home (#451 H3; dms-home-design.md §B):
//! DmSend, DmFollow, DmAck and DmRoutes, over the device's own connection.
//!
//! - **Send.** A device hands the home an MLS message (an application
//!   message, or a commit claktory.com just accepted) with its leaf and
//!   id. The home keeps it in the group's log and queues it for the
//!   friends' homes routed for the group (`dm_link`); the receipt says what
//!   became of it. Each leaf sends at most 60 a minute and 1,000 an hour.
//! - **Follow.** A connection follows one leaf: answered with its groups,
//!   then pushed every entry after the device's cursor (or the leaf's kept
//!   place) a page at a time, then each new one as it arrives, and its own
//!   messages' deliveries, until the connection closes or follows again.
//! - **Ack** lets the home forget what every leaf of the person's kept.
//! - **Routes** replace the routing table with the friends' homes the
//!   device verified, and answer how the home is doing (DmHome).
//!
//! Only while the preview is on; sends and follows only while this
//! Claktory is the person's home (their signed device list names it).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, Weak};

use factory_protocol::v1::{self, ErrorCode, server_message::Body};
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::sync::mpsc;

use crate::encoding::hex;
use crate::factory::{Factory, RequestError};
use crate::locking::lock;
use crate::transcript::now_ms;
use crate::user_apps::dms::{self, home};

/// Entries a page holds at most (the home caps it at 256 KiB too).
const PAGE: usize = 100;
/// Past deliveries a follow's answer carries at most, newest first.
const DELIVERIES: usize = 1000;
/// Pushes waiting for a slow device before its follow waits for it.
const PUSHES_AHEAD: usize = 8;
/// The most routes, and groups in them all, a device may give.
const MOST_ROUTES: usize = 1000;
const MOST_ROUTED_GROUPS: usize = 20_000;

const NOT_HOME: &str = "This Claktory isn't your home for direct messages.";
const NOT_ON: &str = "This Claktory doesn't take direct messages yet.";

/// What the device side keeps while Factory runs.
#[derive(Default)]
pub(crate) struct State {
    /// Each leaf's sends in the last hour, for its rate.
    sends: Mutex<HashMap<String, VecDeque<i64>>>,
    /// The home's state as it last told every connection (`home_changed`).
    said: Mutex<Option<v1::DmHome>>,
}

/// A connection's follow: what it pushes, for it alone.
pub(crate) struct Port {
    sender: mpsc::Sender<Body>,
    receiver: tokio::sync::Mutex<mpsc::Receiver<Body>>,
    following: Mutex<Option<tokio::task::AbortHandle>>,
}

impl Port {
    pub(crate) fn new() -> Port {
        let (sender, receiver) = mpsc::channel(PUSHES_AHEAD);
        Port { sender, receiver: tokio::sync::Mutex::new(receiver), following: Mutex::new(None) }
    }

    /// The next push for this connection.
    pub(crate) async fn next(&self) -> Body {
        match self.receiver.lock().await.recv().await {
            Some(body) => body,
            // It holds a sender itself: never.
            None => std::future::pending().await,
        }
    }

    /// Follows with `follow` in place of the one before.
    fn follow(&self, follow: impl Future<Output = ()> + Send + 'static) {
        let task = tokio::spawn(follow).abort_handle();
        if let Some(before) = lock(&self.following).replace(task) {
            before.abort();
        }
    }
}

impl Drop for Port {
    fn drop(&mut self) {
        if let Some(task) = lock(&self.following).take() {
            task.abort();
        }
    }
}

fn error(code: ErrorCode, message: impl Into<String>) -> Body {
    Body::Error(v1::Error { code: code.into(), message: message.into(), ..Default::default() })
}

/// A leaf id as the devices make them: 1 to 128 visible characters.
pub(crate) fn check_leaf(leaf: &str) -> Result<(), String> {
    if leaf.is_empty() || leaf.len() > 128 || !leaf.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err("leaf_id is 1 to 128 visible characters.".to_owned());
    }
    Ok(())
}

/// An application message's id: 22 to 64 letters, digits, - and _; empty
/// for a commit.
fn check_client_id(id: &str) -> Result<(), String> {
    let fits = (22..=64).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    if !id.is_empty() && !fits {
        return Err("client_message_id is 22 to 64 letters, digits, - and _.".to_owned());
    }
    Ok(())
}

/// Why the home couldn't do it, as an error.
fn refused(refusal: home::Refusal) -> (ErrorCode, String) {
    match refusal {
        home::Refusal::Invalid(why) => (ErrorCode::InvalidRequest, why),
        home::Refusal::NotHome => (ErrorCode::InvalidRequest, NOT_HOME.to_owned()),
        home::Refusal::Unready(why) => {
            tracing::warn!(why, "the direct-message home can't serve a device now");
            (ErrorCode::Internal, "This Claktory can't keep direct messages now. Try again later.".to_owned())
        }
        other => {
            tracing::warn!(?other, "the direct-message home refused a device unexpectedly");
            (ErrorCode::Internal, "This Claktory can't keep direct messages now. Try again later.".to_owned())
        }
    }
}

/// DmSend: answered with DmFeed's `sent`.
/// Whether `leaf` may send one more now (DmSend, and what the person's
/// home before hands on for it): counts it if so, else how long until it
/// may.
pub(crate) fn within_leaf_rate(factory: &Factory, leaf: &str, now: i64) -> Result<(), i64> {
    crate::dm_link::within_rate(lock(&factory.dm_device.sends).entry(leaf.to_owned()).or_default(), now)
}

/// Gives back the send `within_leaf_rate` just counted for `leaf`, when
/// what it counted for was refused (Fugo 33878: a hand-over for a leaf
/// that isn't the person's spends nothing, and leaves no entry).
pub(crate) fn refund_leaf_rate(factory: &Factory, leaf: &str) {
    let mut sends = lock(&factory.dm_device.sends);
    if let Some(taken) = sends.get_mut(leaf) {
        taken.pop_back();
        if taken.is_empty() {
            sends.remove(leaf);
        }
    }
}

pub(crate) fn send(factory: &Factory, request: v1::DmSend) -> Body {
    let v1::DmSend { group_id, envelope, client_message_id, leaf_id } = request;
    let mut receipt = v1::DmReceipt { group_id: group_id.clone(), client_message_id: client_message_id.clone(), ..Default::default() };
    let answer = |mut receipt: v1::DmReceipt, result: v1::DmSendResult| {
        receipt.result = result.into();
        Body::DmFeed(v1::DmFeed { sent: Some(receipt), ..Default::default() })
    };
    if !dms::preview() || !home::is_home(factory) {
        return answer(receipt, v1::DmSendResult::NotHome);
    }
    if let Err(why) = check_leaf(&leaf_id).and_then(|()| check_client_id(&client_message_id)) {
        return error(ErrorCode::InvalidRequest, why);
    }
    let now = now_ms();
    if let Err(wait) = within_leaf_rate(factory, &leaf_id, now) {
        receipt.retry_after_ms = wait.max(1000);
        return answer(receipt, v1::DmSendResult::RateLimited);
    }
    let intake = home::Intake { group_id: group_id.clone(), envelope, source: home::Source::Device { leaf_id, client_message_id } };
    let result = match home::take(factory, intake) {
        Ok(home::Taken::Stored { seq, accepted_ms, duplicate }) => {
            receipt.seq = u64::try_from(seq).unwrap_or_default();
            receipt.accepted_ms = accepted_ms;
            if duplicate { v1::DmSendResult::Duplicate } else { v1::DmSendResult::Stored }
        }
        Ok(home::Taken::Held) => v1::DmSendResult::Held,
        Ok(home::Taken::Bounce { epoch }) => {
            receipt.epoch = epoch;
            v1::DmSendResult::TooOld
        }
        Err(home::Refusal::Waiting) => v1::DmSendResult::WaitingForFriend,
        Err(home::Refusal::Conflict) => v1::DmSendResult::Conflict,
        Err(home::Refusal::Full { retry_after_ms }) => {
            receipt.retry_after_ms = retry_after_ms;
            v1::DmSendResult::HomeFull
        }
        Err(home::Refusal::NotHome) => v1::DmSendResult::NotHome,
        Err(refusal) => {
            let (code, why) = refused(refusal);
            return error(code, why);
        }
    };
    // What it queued for the friends' homes goes now.
    if matches!(result, v1::DmSendResult::Stored | v1::DmSendResult::Held) {
        crate::dm_link::wake_group(factory, &group_id);
    }
    answer(receipt, result)
}

/// DmFollow: answered with the leaf's groups; the entries follow as pushes.
pub(crate) fn follow(factory: &Arc<Factory>, request: v1::DmFollow, port: &Port) -> Body {
    if !dms::preview() {
        return error(ErrorCode::InvalidRequest, NOT_ON);
    }
    if !home::is_home(factory) {
        return error(ErrorCode::InvalidRequest, NOT_HOME);
    }
    let leaf = request.leaf_id;
    if let Err(why) = check_leaf(&leaf) {
        return error(ErrorCode::InvalidRequest, why);
    }
    // Before reading, so nothing that arrives meanwhile is missed.
    let arrivals = home::arrivals(factory);
    let views = match home::groups(&**factory, &leaf) {
        Ok(views) => views,
        Err(refusal) => {
            let (code, why) = refused(refusal);
            return error(code, why);
        }
    };
    // A cursor from before the home (the worker's seqs) counts for nothing:
    // the leaf reads on from its place here.
    let cursors: HashMap<Vec<u8>, i64> = request
        .cursors
        .into_iter()
        .filter_map(|cursor| i64::try_from(cursor.seq).ok().filter(|seq| *seq >= home::FIRST_SEQ - 1).map(|seq| (cursor.group_id, seq)))
        .collect();
    let mut following = Following::default();
    let mut states = Vec::new();
    let mut pending = false;
    for view in &views {
        let start = cursors.get(&view.group_id).copied().unwrap_or(view.place);
        let behind = start < view.pruned;
        states.push(state(view, behind, view.epoch));
        following.known.insert(view.group_id.clone());
        if !behind {
            following.read.insert(view.group_id.clone(), start);
            pending |= start < view.last_seq;
        }
    }
    // What friends' homes took of its messages, so a device that restarted
    // still knows (a bounce is pushed as it comes, and only then).
    let accounts: HashMap<String, String> =
        home::routes(&**factory).unwrap_or_default().into_iter().map(|route| (route.endpoint, route.account)).collect();
    let deliveries = match home::deliveries(&**factory, &leaf, DELIVERIES) {
        Ok(delivered) => delivered
            .into_iter()
            .map(|delivered| v1::DmDelivery {
                account_id: accounts.get(&delivered.endpoint).cloned().unwrap_or_default(),
                group_id: delivered.group_id,
                client_message_id: delivered.client_message_id,
                state: v1::DmDeliveryState::Delivered.into(),
                at_ms: delivered.at_ms,
                ..Default::default()
            })
            .collect(),
        Err(refusal) => {
            tracing::warn!(?refusal, "couldn't read a leaf's deliveries at its home");
            Vec::new()
        }
    };
    port.follow(run(Arc::downgrade(factory), leaf, following, arrivals, port.sender.clone()));
    Body::DmFeed(v1::DmFeed { groups: states, deliveries, has_more: pending, ..Default::default() })
}

fn state(view: &home::GroupView, behind: bool, epoch: u64) -> v1::DmGroupState {
    v1::DmGroupState { group_id: view.group_id.clone(), epoch, last_seq: u64::try_from(view.last_seq).unwrap_or_default(), behind }
}

/// Where a follow is.
#[derive(Default)]
struct Following {
    /// What the device was sent through, in each group it reads.
    read: HashMap<Vec<u8>, i64>,
    /// The groups whose state the device was sent (reading or behind).
    known: HashSet<Vec<u8>>,
}

/// A follow: what's new for `leaf`, pushed until the connection goes.
async fn run(
    factory: Weak<Factory>,
    leaf: String,
    mut following: Following,
    mut arrivals: broadcast::Receiver<home::Arrival>,
    out: mpsc::Sender<Body>,
) {
    let mut look = true;
    loop {
        if look {
            let Some(strong) = factory.upgrade() else { return };
            if catch_up(&strong, &leaf, &mut following, &out).await.is_err() {
                return;
            }
            look = false;
        }
        let delivery = match arrivals.recv().await {
            Ok(home::Arrival::Entry { group_id, seq }) => {
                look = following.read.get(&group_id).is_none_or(|read| seq > *read);
                continue;
            }
            Ok(home::Arrival::Delivered { leaf_id, client_message_id, group_id, endpoint })
                if leaf_id == leaf && !client_message_id.is_empty() =>
            {
                let Some(strong) = factory.upgrade() else { return };
                v1::DmDelivery {
                    group_id,
                    client_message_id,
                    state: v1::DmDeliveryState::Delivered.into(),
                    at_ms: now_ms(),
                    account_id: account_at(&strong, &endpoint),
                    ..Default::default()
                }
            }
            Ok(home::Arrival::Bounced { leaf_id, client_message_id, group_id, epoch, endpoint })
                if leaf_id == leaf && !client_message_id.is_empty() =>
            {
                let Some(strong) = factory.upgrade() else { return };
                v1::DmDelivery {
                    group_id,
                    client_message_id,
                    state: v1::DmDeliveryState::Bounced.into(),
                    at_ms: now_ms(),
                    epoch,
                    account_id: account_at(&strong, &endpoint),
                }
            }
            Ok(_) => continue,
            // It missed some: the log has every entry, so look again.
            Err(RecvError::Lagged(missed)) => {
                tracing::debug!(missed, "a device's follow fell behind its home");
                look = true;
                continue;
            }
            Err(RecvError::Closed) => return,
        };
        let feed = v1::DmFeed { deliveries: vec![delivery], ..Default::default() };
        if out.send(Body::DmFeed(feed)).await.is_err() {
            return;
        }
    }
}

/// The account whose home is at `endpoint`, as the routes have it.
fn account_at(factory: &Factory, endpoint: &str) -> String {
    home::route_of(factory, endpoint).ok().flatten().map(|route| route.account).unwrap_or_default()
}

/// Pushes everything after where the device is, a page at a time; the last
/// page says `has_more: false`. Err when the connection's gone.
async fn catch_up(factory: &Arc<Factory>, leaf: &str, following: &mut Following, out: &mpsc::Sender<Body>) -> Result<(), ()> {
    let views = match home::groups(&**factory, leaf) {
        Ok(views) => views,
        Err(refusal) => {
            tracing::warn!(?refusal, "couldn't read a leaf's groups at its home");
            return Ok(());
        }
    };
    // Groups the leaf left.
    following.read.retain(|group, _| views.iter().any(|view| &view.group_id == group));
    following.known.retain(|group| views.iter().any(|view| &view.group_id == group));
    // Groups it joined since: from its place.
    let mut joined = Vec::new();
    for view in &views {
        if following.known.insert(view.group_id.clone()) {
            let behind = view.place < view.pruned;
            joined.push(state(view, behind, view.epoch));
            if !behind {
                following.read.insert(view.group_id.clone(), view.place);
            }
        }
    }
    let due: Vec<&home::GroupView> =
        views.iter().filter(|view| following.read.get(&view.group_id).is_some_and(|read| *read < view.last_seq)).collect();
    if due.is_empty() {
        if !joined.is_empty() {
            out.send(Body::DmFeed(v1::DmFeed { groups: joined, ..Default::default() })).await.map_err(|_| ())?;
        }
        return Ok(());
    }
    for (index, view) in due.iter().enumerate() {
        let later = index + 1 < due.len();
        loop {
            let after = following.read.get(&view.group_id).copied().unwrap_or(view.place);
            let mut groups = std::mem::take(&mut joined);
            let feed = match home::page(&**factory, leaf, &view.group_id, after, PAGE) {
                Ok(Ok(page)) => {
                    let Some(last) = page.entries.last().map(|entry| entry.seq) else {
                        if !groups.is_empty() {
                            out.send(Body::DmFeed(v1::DmFeed { groups, has_more: later, ..Default::default() })).await.map_err(|_| ())?;
                        }
                        break;
                    };
                    following.read.insert(view.group_id.clone(), last);
                    groups.push(state(view, false, view.epoch));
                    let has_more = page.has_more || later;
                    let more_here = page.has_more;
                    let feed =
                        v1::DmFeed { entries: page.entries.into_iter().map(entry).collect(), groups, has_more, ..Default::default() };
                    out.send(Body::DmFeed(feed)).await.map_err(|_| ())?;
                    if more_here {
                        continue;
                    }
                    break;
                }
                // Pruned past it meanwhile (away too long): it rejoins.
                Ok(Err(home::TooFarBehind { epoch })) => {
                    following.read.remove(&view.group_id);
                    groups.push(state(view, true, epoch));
                    v1::DmFeed { groups, has_more: later, ..Default::default() }
                }
                Err(refusal) => {
                    tracing::warn!(?refusal, "couldn't read a page of a leaf's group at its home");
                    if groups.is_empty() {
                        break;
                    }
                    v1::DmFeed { groups, has_more: later, ..Default::default() }
                }
            };
            out.send(Body::DmFeed(feed)).await.map_err(|_| ())?;
            break;
        }
    }
    Ok(())
}

fn entry(entry: home::Entry) -> v1::DmEntry {
    let kind = match entry.kind.as_str() {
        "commit" => v1::DmEntryKind::Commit,
        _ => v1::DmEntryKind::Application,
    };
    v1::DmEntry {
        group_id: entry.group_id,
        seq: u64::try_from(entry.seq).unwrap_or_default(),
        kind: kind.into(),
        epoch: entry.epoch,
        envelope: entry.envelope,
        accepted_ms: entry.accepted_ms,
        from_me: entry.from_me,
        client_message_id: entry.client_message_id.unwrap_or_default(),
    }
}

/// DmAck: the leaf kept these.
pub(crate) fn ack(factory: &Factory, request: v1::DmAck) -> Result<(), RequestError> {
    let fail = |code: ErrorCode, why: String| RequestError::Account(code, why);
    if !dms::preview() {
        return Err(fail(ErrorCode::InvalidRequest, NOT_ON.to_owned()));
    }
    if !home::is_home(factory) {
        return Err(fail(ErrorCode::InvalidRequest, NOT_HOME.to_owned()));
    }
    check_leaf(&request.leaf_id).map_err(|why| fail(ErrorCode::InvalidRequest, why))?;
    for cursor in request.through {
        let Ok(through) = i64::try_from(cursor.seq) else { continue };
        home::ack(factory, &request.leaf_id, &cursor.group_id, through).map_err(|refusal| {
            let (code, why) = refused(refusal);
            fail(code, why)
        })?;
    }
    Ok(())
}

/// DmRoutes: the friends' homes the device verified, for this home's
/// table and link; answered with how the home is doing.
pub(crate) fn routes(factory: &Arc<Factory>, request: v1::DmRoutes) -> Body {
    if !dms::preview() {
        return error(ErrorCode::InvalidRequest, NOT_ON);
    }
    let own = own_endpoint(factory);
    let mut routes = Vec::new();
    let mut groups = 0;
    if request.routes.len() > MOST_ROUTES {
        return error(ErrorCode::InvalidRequest, format!("At most {MOST_ROUTES} routes."));
    }
    for route in request.routes {
        let account = route.account_id.trim();
        if account.is_empty() || account.len() > 128 || !account.bytes().all(|byte| byte.is_ascii_graphic()) {
            return error(ErrorCode::InvalidRequest, "A route's account_id is 1 to 128 visible characters.");
        }
        if route.home_endpoint.len() != 32 {
            return error(ErrorCode::InvalidRequest, "A route's home_endpoint is 32 bytes.");
        }
        if own.as_deref() == Some(route.home_endpoint.as_slice()) {
            return error(ErrorCode::InvalidRequest, "A route can't be to this Claktory itself.");
        }
        if route.group_ids.iter().any(|group| !(16..=32).contains(&group.len())) {
            return error(ErrorCode::InvalidRequest, "A group's id is 16 to 32 bytes.");
        }
        groups += route.group_ids.len();
        if groups > MOST_ROUTED_GROUPS {
            return error(ErrorCode::InvalidRequest, format!("At most {MOST_ROUTED_GROUPS} routed groups in all."));
        }
        routes.push(home::Route { account: account.to_owned(), endpoint: hex(&route.home_endpoint), group_ids: route.group_ids });
    }
    if let Err(refusal) = home::set_routes(&**factory, routes) {
        let (code, why) = refused(refusal);
        return error(code, why);
    }
    crate::dm_link::reload(factory);
    // The device sends its routes again whenever it publishes a new list:
    // this Claktory reads the list now, so a Claktory taken off it loses
    // its binding within seconds, not at the next round.
    crate::user_apps::dms::process::wake(factory);
    Body::DmHome(home_state(factory))
}

/// This Claktory's iroh endpoint id: what a device signs into its list as
/// its home.
pub(crate) fn own_endpoint(factory: &Factory) -> Option<Vec<u8>> {
    if let Some(endpoint) = factory.remote_endpoint() {
        return Some(endpoint.id().as_bytes().to_vec());
    }
    match crate::remote::secret_key(&factory.home) {
        Ok(key) => Some(key.public().as_bytes().to_vec()),
        Err(error) => {
            tracing::warn!(%error, "no iroh key for the direct-message home");
            None
        }
    }
}

/// Tells every connection how the home is doing when that changed since it
/// last said (DmHome): its place by the person's list, its links to
/// friends' homes, what it holds. A device that asked before the list
/// watcher placed this Claktory hears it then (row 548).
pub(crate) fn home_changed(factory: &Factory) {
    if !dms::preview() {
        return;
    }
    // Read and said under the one lock, so two changes at once are said in
    // the order they were read.
    let mut said = lock(&factory.dm_device.said);
    let now = home_state(factory);
    if said.as_ref() == Some(&now) {
        return;
    }
    *said = Some(now.clone());
    factory.send(Body::DmHome(now));
}

/// How the home is doing (DmHome).
pub(crate) fn home_state(factory: &Factory) -> v1::DmHome {
    let counts = home::outbox_counts(factory).unwrap_or_default();
    let held = home::held_bytes(factory).unwrap_or_default();
    let mut peers: Vec<v1::DmPeer> = factory
        .dm_link
        .peers()
        .into_iter()
        .map(|(route, status)| v1::DmPeer {
            account_id: route.account,
            home_endpoint: crate::encoding::unhex(&route.endpoint).unwrap_or_default(),
            reachable: status.reachable,
            last_reachable_ms: status.last_reachable_ms,
            outbox_count: u32::try_from(counts.get(&route.endpoint).copied().unwrap_or_default()).unwrap_or(u32::MAX),
            refused: status.refused,
        })
        .collect();
    peers.sort_by(|a, b| (&a.account_id, &a.home_endpoint).cmp(&(&b.account_id, &b.home_endpoint)));
    let is_home = home::is_home(factory);
    v1::DmHome {
        is_home,
        home_since_ms: if is_home { home::home_since(factory).unwrap_or_default() } else { 0 },
        endpoint_id: own_endpoint(factory).unwrap_or_default(),
        peers,
        held_bytes: u64::try_from(held.values().sum::<i64>()).unwrap_or_default(),
        intake_paused: home::intake_paused(factory),
    }
}

/// Tests only: this Claktory as the person's home or not, as the list
/// watcher sets it from their signed device list.
#[doc(hidden)]
pub fn set_home_for_test(factory: &Factory, is_home: bool) {
    home::set_is_home(factory, is_home);
}

/// Tests only: a group's state from its GroupInfo, as the home takes it
/// from claktory.com.
#[doc(hidden)]
pub fn seed_for_test(factory: &Factory, group_info: &[u8]) -> Result<(), String> {
    home::seed(factory, group_info).map_err(|refusal| format!("{refusal:?}"))
}

/// Tests only: takes this home's links to other homes down, or back up, as
/// if it were offline and then started again (`dm_link`).
#[doc(hidden)]
pub fn set_offline_for_test(factory: &Arc<Factory>, offline: bool) {
    crate::dm_link::set_offline_for_test(factory, offline);
}

/// Tests only: this Claktory's iroh endpoint id, the one a device list
/// names as the home.
#[doc(hidden)]
pub fn endpoint_for_test(factory: &Factory) -> Option<[u8; 32]> {
    own_endpoint(factory).and_then(|id| id.try_into().ok())
}

/// Tests only: a hello as one of the person's Claktories says it, signed by
/// `key` (its leaf's) for the home at `home` from the endpoint `from`, as
/// it goes on the wire.
#[doc(hidden)]
pub fn hello_for_test(
    key: &claktory_dm_crypto::group::LeafKey,
    (home, from): (&[u8; 32], &[u8; 32]),
    at_ms: i64,
    account: &str,
    leaf: &str,
) -> serde_json::Value {
    crate::dm_hello::signed(key, home, from, at_ms, account, leaf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_checked_as_the_protocol_says() {
        assert!(check_leaf("ann-phone").is_ok());
        assert!(check_leaf("").is_err());
        assert!(check_leaf("has space").is_err());
        assert!(check_leaf(&"x".repeat(129)).is_err());
        assert!(check_client_id("").is_ok(), "a commit has none");
        assert!(check_client_id("message-000000000000001").is_ok());
        assert!(check_client_id("short").is_err());
        assert!(check_client_id("message-00000000000000\u{e9}").is_err());
        assert!(check_client_id(&"x".repeat(65)).is_err());
    }
}
