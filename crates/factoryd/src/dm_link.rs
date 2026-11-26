//! Direct messages between home Claktories (#451, ALPN `claktory-dm/1`):
//! each person's home sends what their devices hand it to each friend's
//! home, and the person's own other Claktories with a leaf in the group,
//! and takes what theirs send, for its own person's devices. Only
//! ciphertext crosses: MLS messages the devices made, which no home opens.
//!
//! - **Who gets in.** Only endpoints in the routing table the person's own
//!   devices gave this home (`DmRoutes`, Fugo's ruling (a)): the gate turns
//!   any other key away before a frame, and each frame is checked again, so
//!   a route taken away ends that peer's next frame. A peer sends only in
//!   the groups its route lists.
//! - **What a frame is.** One request on a stream of its own, JSON after its
//!   length (4 bytes, big-endian), answered the same way: deliver a message
//!   (its id is the SHA-256 of its bytes), hint that a group has a new
//!   commit (which only wakes the home's pull from claktory.com: a peer
//!   never puts a commit into this home's state), or knock (this side just
//!   started: send what you hold for it now).
//! - **Bounded.** 60 messages a minute and 1,000 an hour per peer; the home
//!   holds at most 1 GiB per friend and 5 GiB in all, and takes nothing
//!   with under 5 GiB free on its disk (`Mailbox::take`). A message more
//!   than 5 epochs behind is bounced, only to the home that sent it, only
//!   for an id it sent.
//! - **Sending.** One worker per peer takes what the home queued for it, in
//!   order, and tries again after 1 s, doubling to 15 minutes; something
//!   newly queued, a knock or a frame from that peer wakes it at once.
//!
//! The home's mailbox (`user_apps::dms::home`, H1) keeps the queue, the
//! routes and what it took; this only carries them, through `Mailbox`.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::encoding::hex;
use crate::factory::Factory;
use crate::iroh_link::{DialError, FIRST_BACKOFF, MAX_BACKOFF};
use crate::locking::lock;
use crate::transcript::now_ms;
use crate::user_apps::dms::home;

/// The links between homes, on the tunnel's endpoint.
pub const ALPN_DM: &[u8] = b"claktory-dm/1";

/// At most this big, a request: a message (20 KB, the most a person
/// sends) in base64 with what describes it, and room to spare.
const MAX_REQUEST: usize = 64 * 1024;
const MAX_REPLY: usize = 4 * 1024;
/// How long the peer may take to answer.
const ANSWER: Duration = Duration::from_secs(30);
/// How long a dial may take.
const DIAL: Duration = Duration::from_secs(15);
/// Requests read at once per connection.
const STREAMS_AT_ONCE: usize = 8;
/// Messages a peer may deliver a minute, and an hour.
const PER_MINUTE: usize = 60;
const PER_HOUR: usize = 1000;
/// Messages taken from the queue at once.
const BATCH: usize = 32;

/// A peer this home may talk to, as the routing table has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Route {
    pub account: String,
    /// Its iroh endpoint id, lowercase hex.
    pub endpoint: String,
    pub groups: Vec<Vec<u8>>,
}

/// What the home did with a delivered message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Took {
    Stored,
    /// A future epoch: kept until the home has its commit.
    Held,
    /// More than 5 epochs behind: the sender's device encrypts it again.
    Bounce {
        epoch: u64,
    },
}

/// Why the home wouldn't take it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// Not a group this peer's route lists.
    NotRouted,
    /// The friend has no device in the conversation yet.
    Waiting,
    Invalid(String),
    /// A cap, or the free-space floor: try again after this long.
    Full {
        retry_after_ms: i64,
    },
    /// It can't take anything now: try again later.
    Busy,
    /// This Claktory isn't the person's home (or isn't one for this peer).
    NotHome,
}

/// A message queued for a peer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Outgoing {
    pub message_id: String,
    pub group_id: Vec<u8>,
    pub envelope: Vec<u8>,
    /// What this Claktory, no longer the home, hands on to the person's new
    /// home (Fugo 33778 (a)): the leaf that sent it and its client id.
    pub handover: Option<(String, String)>,
    /// What this Claktory, no longer the home, passes on to the person's
    /// new home that their leaves hadn't all read (Fugo 39876): whose it
    /// is, a hint.
    pub pass_on: Option<String>,
}

/// A message one of the person's other Claktories, their home before,
/// hands on to this one: as its leaf's submission, from that endpoint.
pub(crate) struct HandedOver<'a> {
    pub endpoint: &'a str,
    pub message_id: &'a str,
    pub group: &'a [u8],
    pub envelope: &'a [u8],
    pub leaf_id: &'a str,
    pub client_message_id: &'a str,
}

/// The home's side (`user_apps::dms::home`): routes, intake and the queue.
pub(crate) trait Mailbox: Send + Sync {
    fn routes(&self, factory: &Factory) -> Vec<Route>;
    fn take(&self, factory: &Factory, endpoint: &str, message_id: &str, group: &[u8], envelope: &[u8]) -> Result<Took, Refused>;
    /// What the person's home before hands on, as a device's submission.
    fn take_handed(&self, factory: &Factory, handed: HandedOver<'_>) -> Result<Took, Refused>;
    /// What the person's home before passes on, for their leaves only.
    fn take_passed(
        &self,
        factory: &Factory,
        endpoint: &str,
        message_id: &str,
        group: &[u8],
        envelope: &[u8],
        account: &str,
    ) -> Result<Took, Refused>;
    /// A peer says the group has a commit at `epoch`: pull from claktory.com.
    fn hint(&self, factory: &Factory, group: &[u8], epoch: u64);
    fn outbox(&self, factory: &Factory, endpoint: &str, limit: usize) -> Vec<Outgoing>;
    fn sent(&self, factory: &Factory, endpoint: &str, ids: &[String]);
    fn bounced(&self, factory: &Factory, endpoint: &str, id: &str, epoch: u64);
}

/// A peer's link, as the person's devices see it (DmHome.peers).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PeerStatus {
    pub reachable: bool,
    /// When it last answered, in ms since 1970; 0 for never.
    pub last_reachable_ms: i64,
    /// A cap it said it reached ("full"), or [`TURNED_AWAY`]; empty if
    /// none.
    pub refused: String,
}

/// `PeerStatus.refused` when the peer doesn't take this home's link at all
/// (Zucchero's row 567): it isn't the friend's home any more, or isn't
/// routing this one yet. The person's device reads the friend's list again
/// and gives its home new routes; until then it isn't "offline".
pub(crate) const TURNED_AWAY: &str = "turned_away";

/// What [`connection`] says when the peer's gate refused the dial.
const NOT_ROUTED: &str = "it doesn't route this home";

/// What the links keep while Factory runs.
#[derive(Default)]
pub(crate) struct State {
    /// The endpoints routed now: the gate's list.
    admitted: Mutex<HashMap<String, Route>>,
    workers: Mutex<HashMap<String, Arc<Worker>>>,
    status: Mutex<HashMap<String, PeerStatus>>,
    /// Each peer's deliveries in the last hour, for its rate.
    taken: Mutex<HashMap<String, VecDeque<i64>>>,
    /// What the tunnel's gate reads, before any frame.
    pub(crate) gate: Arc<Admits>,
    /// The connections peers opened to this home, by endpoint: closed when
    /// the endpoint is no longer routed.
    incoming: Mutex<HashMap<String, Vec<Connection>>>,
    /// `FIRST_BACKOFF` in ms, 0 unless a test set it.
    first_backoff_ms: std::sync::atomic::AtomicU64,
}

impl State {
    /// Whether the gate lets `endpoint` onto the DM ALPN.
    pub(crate) fn admits(&self, endpoint: &str) -> bool {
        !self.gate.offline() && lock(&self.admitted).contains_key(endpoint)
    }

    /// Whether remote access must stay on for these links.
    pub(crate) fn wants_tunnel(&self) -> bool {
        !lock(&self.admitted).is_empty()
    }

    /// The links to each routed peer, for DmHome.
    pub(crate) fn peers(&self) -> Vec<(Route, PeerStatus)> {
        let status = lock(&self.status);
        lock(&self.admitted).values().map(|route| (route.clone(), status.get(&route.endpoint).cloned().unwrap_or_default())).collect()
    }

    fn first_backoff(&self) -> Duration {
        match self.first_backoff_ms.load(std::sync::atomic::Ordering::Relaxed) {
            0 => FIRST_BACKOFF,
            ms => Duration::from_millis(ms),
        }
    }
}

/// Who the tunnel's gate lets onto `claktory-dm/1`: the endpoints routed
/// (friends' homes, the person's own Claktories bound by their hello, and
/// a non-home's home), and, while this Claktory is the person's home,
/// strangers, which then get only a hello (`dm_hello`).
#[derive(Debug, Default)]
pub(crate) struct Admits {
    routed: Mutex<std::collections::HashSet<String>>,
    hellos: std::sync::atomic::AtomicBool,
    /// A test took this home's links down (`set_offline_for_test`).
    offline: std::sync::atomic::AtomicBool,
}

impl Admits {
    pub(crate) fn admits(&self, endpoint: &str) -> bool {
        !self.offline() && (self.hellos.load(std::sync::atomic::Ordering::Relaxed) || lock(&self.routed).contains(endpoint))
    }

    fn offline(&self) -> bool {
        self.offline.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Takes this home's links to other homes down, or back up, as if it were
/// offline and then started again: a test's way to stop one Claktory's
/// `claktory-dm/1` without ending its process. Down, the gate turns every
/// endpoint away, every link closes and its sending stops; what comes in
/// meanwhile from the person's devices waits in the outbox. Up, each
/// routed peer's link starts over with a knock, as at a start.
pub(crate) fn set_offline_for_test(factory: &Arc<Factory>, offline: bool) {
    let state = &factory.dm_link;
    state.gate.offline.store(offline, std::sync::atomic::Ordering::Relaxed);
    if !offline {
        reload(factory);
        return;
    }
    for (_, connections) in lock(&state.incoming).drain() {
        for connection in connections {
            connection.close(0u32.into(), b"offline");
        }
    }
    for (_, worker) in lock(&state.workers).drain() {
        worker.wake.notify_one();
        if let Some(connection) = lock(&worker.connection).take() {
            connection.close(0u32.into(), b"offline");
        }
    }
}

#[derive(Default)]
struct Worker {
    wake: tokio::sync::Notify,
    connection: Mutex<Option<Connection>>,
}

/// The home's mailbox.
fn mailbox(_factory: &Factory) -> Option<&'static dyn Mailbox> {
    Some(&HOME)
}

/// The home's mailbox (`user_apps::dms::home`).
struct HomeMailbox;

static HOME: HomeMailbox = HomeMailbox;

impl Mailbox for HomeMailbox {
    fn routes(&self, factory: &Factory) -> Vec<Route> {
        match home::routes(factory) {
            Ok(routes) => routes
                .into_iter()
                .map(|route| Route { account: route.account, endpoint: route.endpoint, groups: route.group_ids })
                .collect(),
            Err(refusal) => {
                tracing::warn!(?refusal, "couldn't read the home's routes");
                Vec::new()
            }
        }
    }

    fn take(&self, factory: &Factory, endpoint: &str, message_id: &str, group: &[u8], envelope: &[u8]) -> Result<Took, Refused> {
        let source = home::Source::Home { endpoint: endpoint.to_owned(), message_id: message_id.to_owned() };
        took(home::take(factory, home::Intake { group_id: group.to_vec(), envelope: envelope.to_vec(), source }))
    }

    fn take_handed(&self, factory: &Factory, handed: HandedOver<'_>) -> Result<Took, Refused> {
        let source = home::Source::HandOver {
            endpoint: handed.endpoint.to_owned(),
            message_id: handed.message_id.to_owned(),
            leaf_id: handed.leaf_id.to_owned(),
            client_message_id: handed.client_message_id.to_owned(),
        };
        took(home::take(factory, home::Intake { group_id: handed.group.to_vec(), envelope: handed.envelope.to_vec(), source }))
    }

    fn take_passed(
        &self,
        factory: &Factory,
        endpoint: &str,
        message_id: &str,
        group: &[u8],
        envelope: &[u8],
        account: &str,
    ) -> Result<Took, Refused> {
        let source = home::Source::PassOn { endpoint: endpoint.to_owned(), message_id: message_id.to_owned(), account: account.to_owned() };
        took(home::take(factory, home::Intake { group_id: group.to_vec(), envelope: envelope.to_vec(), source }))
    }

    fn hint(&self, factory: &Factory, group: &[u8], epoch: u64) {
        if let Err(refusal) = home::hint(factory, group, epoch) {
            tracing::warn!(?refusal, "couldn't take a hint");
        }
    }

    fn outbox(&self, factory: &Factory, endpoint: &str, limit: usize) -> Vec<Outgoing> {
        home::outbox(factory, endpoint, limit)
            .unwrap_or_default()
            .into_iter()
            .map(|out| Outgoing {
                message_id: out.message_id,
                group_id: out.group_id,
                envelope: out.envelope,
                handover: out.handover,
                pass_on: out.pass_on,
            })
            .collect()
    }

    fn sent(&self, factory: &Factory, endpoint: &str, ids: &[String]) {
        if let Err(refusal) = home::sent(factory, endpoint, ids) {
            tracing::warn!(?refusal, "couldn't mark messages sent");
        }
    }

    fn bounced(&self, factory: &Factory, endpoint: &str, id: &str, epoch: u64) {
        if let Err(refusal) = home::bounced(factory, endpoint, id, epoch) {
            tracing::warn!(?refusal, "couldn't take a bounce");
        }
    }
}

/// What the home's intake answered, as the link says it.
fn took(taken: Result<home::Taken, home::Refusal>) -> Result<Took, Refused> {
    match taken {
        Ok(home::Taken::Stored { .. }) => Ok(Took::Stored),
        Ok(home::Taken::Held) => Ok(Took::Held),
        Ok(home::Taken::Bounce { epoch }) => Ok(Took::Bounce { epoch }),
        Err(home::Refusal::NotRouted) => Err(Refused::NotRouted),
        Err(home::Refusal::Waiting) => Err(Refused::Waiting),
        Err(home::Refusal::Full { retry_after_ms }) => Err(Refused::Full { retry_after_ms }),
        Err(home::Refusal::NotHome) => Err(Refused::NotHome),
        Err(home::Refusal::Unready(why)) => {
            tracing::warn!(why, "the home couldn't take a friend's message now");
            Err(Refused::Busy)
        }
        Err(home::Refusal::Conflict) => Err(Refused::Invalid("another message under that id".to_owned())),
        Err(home::Refusal::Invalid(why)) => Err(Refused::Invalid(why)),
    }
}

/// A request between homes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Request {
    Deliver {
        id: String,
        group_id: String,
        envelope: String,
    },
    Hint {
        group_id: String,
        epoch: u64,
    },
    Knock,
    /// The person's home before hands on what it held for friends when it
    /// stopped being the home (Fugo 33778 (a)): a device's message, its
    /// leaf and client id.
    HandOver {
        id: String,
        group_id: String,
        envelope: String,
        leaf_id: String,
        client_message_id: String,
    },
    /// The person's home before passes on what it had taken that their
    /// leaves hadn't all read when it stopped being the home (Zucchero's
    /// row 567, Fugo 39876): an application message, for their leaves
    /// only; `account` is a hint.
    PassOn {
        id: String,
        group_id: String,
        envelope: String,
        account: String,
    },
    /// One of the person's own Claktories says who it is (`dm_hello`).
    Hello {
        account_id: String,
        leaf_id: String,
        at_ms: i64,
        signature: String,
    },
    /// A friend's home says it's their home now, with their signed device
    /// list (`dm_hello::HomeList`, row 595): the first request on each new
    /// connection to a friend's home. A home that doesn't know it answers
    /// "bad", and the knock goes on.
    HomeList {
        list: String,
    },
}

/// Its answer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Reply {
    /// took, held, too_old, not_routed, waiting, full, bad, ok.
    code: String,
    #[serde(skip_serializing_if = "is_zero")]
    epoch: u64,
    #[serde(skip_serializing_if = "is_zero_i64")]
    retry_after_ms: i64,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

fn is_zero_i64(value: &i64) -> bool {
    *value == 0
}

impl Reply {
    fn code(code: &str) -> Reply {
        Reply { code: code.to_owned(), ..Reply::default() }
    }
}

/// The SHA-256 of a message, the id both homes know it by.
pub(crate) fn message_id(envelope: &[u8]) -> String {
    hex(&Sha256::digest(envelope))
}

/// Takes the routing table the person's devices gave the home: the gate
/// lets only these in, and each has a worker sending it what's queued.
pub(crate) fn reload(factory: &Arc<Factory>) {
    let Some(mailbox) = mailbox(factory) else { return };
    let routes: HashMap<String, Route> =
        mailbox.routes(factory).into_iter().map(|route| (route.endpoint.to_ascii_lowercase(), route)).collect();
    let state = &factory.dm_link;
    *lock(&state.admitted) = routes.clone();
    *lock(&state.gate.routed) = routes.keys().cloned().collect();
    let is_home = home::is_home(factory);
    let moved = state.gate.hellos.swap(is_home, std::sync::atomic::Ordering::Relaxed) != is_home;
    let mut workers = lock(&state.workers);
    lock(&state.incoming).retain(|endpoint, connections| {
        let keep = routes.contains_key(endpoint) || bound(factory, endpoint);
        if !keep {
            for connection in connections.drain(..) {
                // Refused, as the gate refuses it: the peer can tell this
                // from a home that's offline.
                connection.close(crate::remote::REFUSED.into(), b"no longer routed");
            }
        }
        keep
    });
    workers.retain(|endpoint, worker| {
        let keep = routes.contains_key(endpoint);
        if !keep {
            worker.wake.notify_one();
            if let Some(connection) = lock(&worker.connection).take() {
                connection.close(0u32.into(), b"no longer routed");
            }
        } else if moved {
            // The home now, or no longer: a peer that turned away its knock
            // (no hello, no list yet) is knocked again now, with them, not
            // after its backoff (row 606).
            worker.wake.notify_one();
        }
        keep
    });
    // Offline (a test's): no links until it's back (`set_offline_for_test`).
    let offline = state.gate.offline();
    for endpoint in routes.keys().filter(|_| !offline) {
        if !workers.contains_key(endpoint) {
            let worker = Arc::new(Worker::default());
            workers.insert(endpoint.clone(), worker.clone());
            tokio::spawn(work(Arc::downgrade(factory), endpoint.clone(), worker));
        }
    }
    drop(workers);
    // Its place or its routes may have changed: the person's devices hear.
    crate::dm_device::home_changed(factory);
}

/// Something was queued for `endpoint`, or it knocked: send now.
pub(crate) fn wake(factory: &Factory, endpoint: &str) {
    if let Some(worker) = lock(&factory.dm_link.workers).get(endpoint) {
        worker.wake.notify_one();
    }
}

/// Something was queued in `group`: wakes every peer routed for it.
pub(crate) fn wake_group(factory: &Factory, group: &[u8]) {
    let endpoints: Vec<String> = lock(&factory.dm_link.admitted)
        .values()
        .filter(|route| route.groups.iter().any(|listed| listed == group))
        .map(|route| route.endpoint.clone())
        .collect();
    for endpoint in endpoints {
        wake(factory, &endpoint);
    }
}

fn set_status(factory: &Factory, endpoint: &str, change: impl FnOnce(&mut PeerStatus)) {
    let changed = {
        let mut statuses = lock(&factory.dm_link.status);
        let status = statuses.entry(endpoint.to_owned()).or_default();
        let before = (status.reachable, status.refused.clone());
        change(status);
        before != (status.reachable, status.refused.clone())
    };
    // Reachable or not, full or not: the person's devices hear at once (when
    // it was last reached, and what waits for it, go along).
    if changed {
        crate::dm_device::home_changed(factory);
    }
}

/// One peer's worker: knocks once, then sends what's queued for it until
/// it's no longer routed.
async fn work(factory: Weak<Factory>, endpoint: String, worker: Arc<Worker>) {
    let mut backoff: Option<Duration> = None;
    let mut knocked = false;
    loop {
        if let Some(wait) = backoff {
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = worker.wake.notified() => {}
            }
        }
        let Some(strong) = factory.upgrade() else { return };
        if !strong.dm_link.admits(&endpoint) {
            return;
        }
        let Some(mailbox) = mailbox(&strong) else { return };
        let queued = mailbox.outbox(&strong, &endpoint, BATCH);
        if queued.is_empty() && knocked {
            backoff = None;
            drop(strong);
            worker.wake.notified().await;
            continue;
        }
        match flush(&strong, &endpoint, &worker, mailbox, queued, !knocked).await {
            Ok(Flushed::Done) => {
                knocked = true;
                backoff = None;
            }
            Ok(Flushed::Wait(wait)) => {
                knocked = true;
                backoff = Some(wait);
            }
            Err(error) => {
                tracing::debug!(%endpoint, %error, "couldn't reach a friend's home");
                let turned = error == NOT_ROUTED
                    || lock(&worker.connection)
                        .as_ref()
                        .and_then(Connection::close_reason)
                        .is_some_and(|reason| crate::iroh_link::turned_away(&reason));
                set_status(&strong, &endpoint, |status| {
                    status.reachable = false;
                    if turned {
                        status.refused = TURNED_AWAY.to_owned();
                    } else if status.refused == TURNED_AWAY {
                        status.refused.clear();
                    }
                });
                if let Some(connection) = lock(&worker.connection).take() {
                    connection.close(0u32.into(), b"failed");
                }
                let first = strong.dm_link.first_backoff();
                backoff = Some(backoff.map_or(first, |wait| (wait * 2).min(MAX_BACKOFF)));
            }
        }
    }
}

enum Flushed {
    /// Everything taken: look for more.
    Done,
    /// The peer is full or busy: try again after this.
    Wait(Duration),
}

async fn connection(factory: &Arc<Factory>, endpoint: &str, worker: &Worker) -> Result<Connection, String> {
    if let Some(connection) = lock(&worker.connection).clone().filter(|connection| connection.close_reason().is_none()) {
        return Ok(connection);
    }
    let connection = crate::iroh_link::dial(factory, endpoint, ALPN_DM, &[], &[], DIAL).await.map_err(|error| match error {
        DialError::TurnedAway => NOT_ROUTED.to_owned(),
        DialError::Failed(why) => why,
    })?;
    *lock(&worker.connection) = Some(connection.clone());
    Ok(connection)
}

/// Sends `queued` to the peer, in order, after a knock when `knock`.
async fn flush(
    factory: &Arc<Factory>,
    endpoint: &str,
    worker: &Worker,
    mailbox: &dyn Mailbox,
    queued: Vec<Outgoing>,
    knock: bool,
) -> Result<Flushed, String> {
    let connection = connection(factory, endpoint, worker).await?;
    let ask = async |request: &Request| -> Result<Reply, String> {
        let reply: Reply = crate::iroh_link::exchange(&connection, &frame(request), ANSWER, MAX_REPLY).await?;
        set_status(factory, endpoint, |status| {
            status.reachable = true;
            status.last_reachable_ms = now_ms();
            if status.refused == TURNED_AWAY {
                status.refused.clear();
            }
        });
        Ok(reply)
    };
    if knock {
        // To the person's home from another of their Claktories: who it is
        // first, on every new connection.
        if let Some(hello) = hello_for(factory, endpoint) {
            let reply = ask(&hello).await?;
            if reply.code != "ok" {
                return Err(format!("the home didn't take this Claktory's hello ({})", reply.code));
            }
        }
        // To a friend's home from the person's: that it's their home, so a
        // friend's home that still routes to the one before (all of the
        // friend's devices off since the move) takes it (row 595).
        if let Some(proof) = home_list_for(factory, endpoint) {
            let reply = ask(&proof).await?;
            if reply.code != "ok" {
                tracing::debug!(%endpoint, code = reply.code, "a friend's home didn't take this home's list");
            }
        }
        ask(&Request::Knock).await?;
    }
    // Kept back from this round: the peer took the others meanwhile.
    let mut kept = false;
    for outgoing in queued {
        // A commit goes as a hint: the peer only ever takes commits from
        // claktory.com (Fugo's condition 4), and one can be 128 KiB.
        let commit = claktory_dm_crypto::envelope::parse(&outgoing.envelope)
            .ok()
            .filter(|parsed| parsed.kind == claktory_dm_crypto::envelope::Kind::Commit)
            .and_then(|parsed| parsed.group.map(|(_, epoch)| epoch));
        let request = match (&outgoing.handover, commit, &outgoing.pass_on) {
            // Passed on: only ever an application message (the queue holds
            // nothing else), for the person's leaves at the new home.
            (_, _, Some(account)) => Request::PassOn {
                id: outgoing.message_id.clone(),
                group_id: B64.encode(&outgoing.group_id),
                envelope: B64.encode(&outgoing.envelope),
                account: account.clone(),
            },
            // Handed on whole, a commit too: the new home takes it only as
            // it would from a device, and never seals anything again.
            (Some((leaf_id, client_message_id)), _, None) => Request::HandOver {
                id: outgoing.message_id.clone(),
                group_id: B64.encode(&outgoing.group_id),
                envelope: B64.encode(&outgoing.envelope),
                leaf_id: leaf_id.clone(),
                client_message_id: client_message_id.clone(),
            },
            (None, Some(epoch), None) => Request::Hint { group_id: B64.encode(&outgoing.group_id), epoch },
            (None, None, None) => Request::Deliver {
                id: outgoing.message_id.clone(),
                group_id: B64.encode(&outgoing.group_id),
                envelope: B64.encode(&outgoing.envelope),
            },
        };
        let reply = ask(&request).await?;
        match reply.code.as_str() {
            "took" | "held" | "ok" => {
                mailbox.sent(factory, endpoint, std::slice::from_ref(&outgoing.message_id));
                set_status(factory, endpoint, |status| status.refused.clear());
            }
            "too_old" => mailbox.bounced(factory, endpoint, &outgoing.message_id, reply.epoch),
            "full" => {
                set_status(factory, endpoint, |status| status.refused = "full".to_owned());
                return Ok(Flushed::Wait(Duration::from_millis(reply.retry_after_ms.clamp(1000, 3_600_000) as u64)));
            }
            "busy" => return Ok(Flushed::Wait(Duration::from_millis(reply.retry_after_ms.clamp(1000, 3_600_000) as u64))),
            // It doesn't route this group (yet), the friend has no device
            // in it yet, or it refused this one: kept, tried again later,
            // and the rest still go.
            code => {
                tracing::debug!(%endpoint, code, "a friend's home didn't take a message yet");
                kept = true;
            }
        }
    }
    Ok(if kept { Flushed::Wait(MAX_BACKOFF) } else { Flushed::Done })
}

/// The hello this Claktory says to `endpoint` when that's the person's home
/// and this isn't (its route is the person's own account's).
fn hello_for(factory: &Factory, endpoint: &str) -> Option<Request> {
    if home::is_home(factory) {
        return None;
    }
    let account = lock(&factory.dm_link.admitted).get(endpoint)?.account.clone();
    let there: [u8; 32] = crate::encoding::unhex(endpoint)?.try_into().ok()?;
    let here: [u8; 32] = crate::dm_device::own_endpoint(factory)?.try_into().ok()?;
    let hello = match crate::dm_hello::make(factory, &there, &here, now_ms()) {
        Ok(hello) => hello,
        Err(why) => {
            tracing::warn!(why, "couldn't say hello to the person's home");
            return None;
        }
    };
    (hello.account_id == account).then_some(Request::Hello {
        account_id: hello.account_id,
        leaf_id: hello.leaf_id,
        at_ms: hello.at_ms,
        signature: hello.signature,
    })
}

/// The `HomeList` this Claktory shows `endpoint` when it's the person's
/// home and `endpoint` is a friend's (its route is another account's).
fn home_list_for(factory: &Factory, endpoint: &str) -> Option<Request> {
    if !home::is_home(factory) {
        return None;
    }
    let account = lock(&factory.dm_link.admitted).get(endpoint)?.account.clone();
    let (own, _, _) = factory.account.registration(&factory.lock().config.account.clone())?;
    if account == own {
        return None;
    }
    let here: [u8; 32] = crate::dm_device::own_endpoint(factory)?.try_into().ok()?;
    let proof = crate::dm_hello::home_list(factory, &here)?;
    Some(Request::HomeList { list: proof.list })
}

/// A request's bytes on the wire.
fn frame(request: &Request) -> Vec<u8> {
    let body = serde_json::to_vec(request).unwrap_or_default();
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&u32::try_from(body.len()).unwrap_or(u32::MAX).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

async fn read_request(receive: &mut RecvStream) -> Result<Request, String> {
    let length = receive.read_u32().await.map_err(|error| error.to_string())? as usize;
    if length > MAX_REQUEST {
        return Err("too large".to_owned());
    }
    let mut body = vec![0; length];
    receive.read_exact(&mut body).await.map_err(|error| error.to_string())?;
    serde_json::from_slice(&body).map_err(|error| error.to_string())
}

/// Serves a peer home's connection: each stream one request, answered.
pub(crate) async fn serve(connection: Connection, factory: Weak<Factory>) {
    let peer = connection.remote_id().to_string();
    {
        let Some(strong) = factory.upgrade() else { return };
        if !known(&strong, &peer) {
            // A stranger: its hello, or nothing.
            if !stranger(&strong, &connection).await {
                connection.close(crate::remote::REFUSED.into(), b"not routed");
                return;
            }
            reload(&strong);
        }
        lock(&strong.dm_link.incoming).entry(peer.clone()).or_default().push(connection.clone());
    }
    serve_routed(&connection, &factory, &peer).await;
    if let Some(strong) = factory.upgrade() {
        let mut incoming = lock(&strong.dm_link.incoming);
        if let Some(connections) = incoming.get_mut(&peer) {
            connections.retain(|kept| kept.stable_id() != connection.stable_id());
            if connections.is_empty() {
                incoming.remove(&peer);
            }
        }
    }
}

/// Each stream of a routed peer's connection, one request answered, while
/// it's routed.
async fn serve_routed(connection: &Connection, factory: &Weak<Factory>, peer: &str) {
    let peer = peer.to_owned();
    let reading = Arc::new(tokio::sync::Semaphore::new(STREAMS_AT_ONCE));
    while let Ok((send, receive)) = connection.accept_bi().await {
        let Some(strong) = factory.upgrade() else { return };
        if !known(&strong, &peer) {
            connection.close(crate::remote::REFUSED.into(), b"not routed");
            return;
        }
        drop(strong);
        let (factory, reading, peer) = (factory.clone(), reading.clone(), peer.clone());
        tokio::spawn(async move {
            let Ok(_reading) = reading.acquire_owned().await else { return };
            if let Err(error) = respond(factory, &peer, send, receive).await {
                tracing::debug!(%error, "a friend home's request failed");
            }
        });
    }
}

/// Whether `peer` is let in without a hello: routed, or one of the
/// person's own Claktories bound here (Fugo's condition 3: even while the
/// hello limits are spent, and before its leaf is in any group).
fn known(factory: &Factory, peer: &str) -> bool {
    !factory.dm_link.gate.offline() && (factory.dm_link.admits(peer) || bound(factory, peer))
}

/// Whether `endpoint` is one of the person's own Claktories bound to this
/// home by its hello.
fn bound(factory: &Factory, endpoint: &str) -> bool {
    home::is_home(factory) && matches!(home::bound_leaf(factory, endpoint), Ok(Some(_)))
}

/// A stranger's hello, within its limits: whether it bound one of the
/// person's own Claktories (answered "ok" then).
async fn stranger(factory: &Arc<Factory>, connection: &Connection) -> bool {
    let Ok(_slot) = factory.dm_hello.strangers.clone().try_acquire_owned() else {
        tracing::debug!("too many strangers at once on the direct-message link");
        return false;
    };
    if !home::is_home(factory) || !crate::dm_hello::may_hear(factory, now_ms()) {
        return false;
    }
    // One of the person's Claktories (a hello), or a friend's new home
    // (its list): read strictly, either way.
    enum First {
        Hello(crate::dm_hello::Hello),
        HomeList(crate::dm_hello::HomeList),
    }
    let read = async {
        let (send, mut receive) = connection.accept_bi().await.ok()?;
        let length = receive.read_u32().await.ok()? as usize;
        if length > crate::dm_hello::MOST.max(crate::dm_hello::HOME_LIST_MOST) {
            return None;
        }
        let mut body = vec![0; length];
        receive.read_exact(&mut body).await.ok()?;
        let first = match serde_json::from_slice::<crate::dm_hello::Hello>(&body) {
            Ok(hello) if length <= crate::dm_hello::MOST => First::Hello(hello),
            Ok(_) => return None,
            Err(_) => First::HomeList(serde_json::from_slice(&body).ok()?),
        };
        Some((send, first))
    };
    let Ok(Some((mut send, first))) = tokio::time::timeout(crate::dm_hello::DEADLINE, read).await else {
        tracing::debug!(endpoint = %connection.remote_id(), "a stranger said no hello");
        return false;
    };
    let from = connection.remote_id().to_string();
    let taken = match first {
        First::Hello(hello) => hear(factory, &from, &hello),
        First::HomeList(proof) => hear_home(factory, &from, &proof),
    };
    if !taken {
        return false;
    }
    crate::iroh_link::write_json(&mut send, &Reply::code("ok")).await.is_ok()
}

/// Checks a friend's new home's list from the connection's endpoint `from`
/// (lowercase hex) and moves that friend's routes to it: whether it did.
fn hear_home(factory: &Factory, from: &str, proof: &crate::dm_hello::HomeList) -> bool {
    let Some(from_bytes) = crate::encoding::unhex(from).and_then(|bytes| <[u8; 32]>::try_from(bytes).ok()) else { return false };
    match crate::dm_hello::hear_home(factory, &from_bytes, proof) {
        Ok((account, old)) => {
            tracing::info!(account, from = ?old, to = %from, "a friend's home moved: their groups go to the new one");
            true
        }
        Err(why) => {
            tracing::info!(endpoint = %from, ?why, "turned away a friend's new home");
            false
        }
    }
}

/// Checks a hello from the connection's endpoint `from` (lowercase hex)
/// and binds it: whether it did.
fn hear(factory: &Factory, from: &str, hello: &crate::dm_hello::Hello) -> bool {
    let Some(here) = crate::dm_device::own_endpoint(factory).and_then(|here| <[u8; 32]>::try_from(here).ok()) else { return false };
    let Some(from_bytes) = crate::encoding::unhex(from).and_then(|bytes| <[u8; 32]>::try_from(bytes).ok()) else { return false };
    match crate::dm_hello::hear(factory, &here, &from_bytes, hello, now_ms()) {
        Ok(leaf) => {
            tracing::info!(leaf, endpoint = %from, "one of the person's Claktories said hello");
            true
        }
        Err(why) => {
            // A stranger's string: escaped and cut short, so it can't forge
            // log lines (Fugo 33386).
            let leaf: String = hello.leaf_id.chars().take(64).collect();
            tracing::warn!(leaf = ?leaf, ?why, "turned away a hello");
            false
        }
    }
}

async fn respond(factory: Weak<Factory>, peer: &str, mut send: SendStream, mut receive: RecvStream) -> std::io::Result<()> {
    let request = match tokio::time::timeout(ANSWER, read_request(&mut receive)).await {
        Ok(Ok(request)) => request,
        Ok(Err(error)) => {
            tracing::warn!(endpoint = %peer, %error, "turned away a friend home's request");
            return crate::iroh_link::write_json(&mut send, &Reply::code("bad")).await;
        }
        Err(_) => return Ok(()),
    };
    let Some(factory) = factory.upgrade() else { return Ok(()) };
    let reply = answer(&factory, peer, request, now_ms());
    crate::iroh_link::write_json(&mut send, &reply).await
}

/// Whether `peer` may deliver one more now; counts it if so, else how long
/// until it may.
pub(crate) fn within_rate(taken: &mut VecDeque<i64>, now: i64) -> Result<(), i64> {
    while taken.front().is_some_and(|at| *at <= now - 3_600_000) {
        taken.pop_front();
    }
    if taken.len() >= PER_HOUR {
        return Err(taken.front().map_or(1000, |at| at + 3_600_000 - now));
    }
    let minute: Vec<&i64> = taken.iter().filter(|at| **at > now - 60_000).collect();
    if minute.len() >= PER_MINUTE {
        return Err(minute.first().map_or(1000, |at| **at + 60_000 - now));
    }
    taken.push_back(now);
    Ok(())
}

/// What this home answers `peer`'s request.
fn answer(factory: &Arc<Factory>, peer: &str, request: Request, now: i64) -> Reply {
    // Bound already (it said hello on an earlier connection): a newer one
    // binds it again.
    if let Request::Hello { account_id, leaf_id, at_ms, signature } = request {
        let hello = crate::dm_hello::Hello { kind: "hello".to_owned(), account_id, leaf_id, at_ms, signature };
        if !home::is_home(factory) || !crate::dm_hello::may_hear(factory, now) || !hear(factory, peer, &hello) {
            return Reply::code("bad");
        }
        reload(factory);
        return Reply::code("ok");
    }
    // A friend's home, known already, says it's their home: within its
    // rate, as a delivery is.
    if let Request::HomeList { list } = request {
        if within_rate(lock(&factory.dm_link.taken).entry(peer.to_owned()).or_default(), now).is_err() {
            return Reply::code("busy");
        }
        let proof = crate::dm_hello::HomeList { kind: "home_list".to_owned(), list };
        if !hear_home(factory, peer, &proof) {
            return Reply::code("bad");
        }
        reload(factory);
        return Reply::code("ok");
    }
    if let Request::HandOver { id, group_id, envelope, leaf_id, client_message_id } = request {
        return hand_over(factory, peer, &id, &group_id, &envelope, &leaf_id, &client_message_id, now);
    }
    if let Request::PassOn { id, group_id, envelope, account } = request {
        return pass_on(factory, peer, &id, &group_id, &envelope, &account, now);
    }
    let Some(route) = lock(&factory.dm_link.admitted).get(peer).cloned() else { return Reply::code("not_routed") };
    let Some(mailbox) = mailbox(factory) else { return Reply::code("bad") };
    match request {
        Request::Knock => {
            wake(factory, peer);
            Reply::code("ok")
        }
        Request::Hint { group_id, epoch } => {
            let Ok(group) = B64.decode(&group_id) else { return Reply::code("bad") };
            if route.groups.contains(&group) {
                mailbox.hint(factory, &group, epoch);
            }
            Reply::code("ok")
        }
        Request::Hello { .. } | Request::HandOver { .. } | Request::PassOn { .. } | Request::HomeList { .. } => Reply::code("bad"),
        Request::Deliver { id, group_id, envelope } => {
            let (Ok(group), Ok(envelope)) = (B64.decode(&group_id), B64.decode(&envelope)) else { return Reply::code("bad") };
            if message_id(&envelope) != id {
                return Reply::code("bad");
            }
            if !route.groups.contains(&group) {
                return Reply::code("not_routed");
            }
            if let Err(wait) = within_rate(lock(&factory.dm_link.taken).entry(peer.to_owned()).or_default(), now) {
                return Reply { code: "busy".to_owned(), retry_after_ms: wait.max(1000), ..Reply::default() };
            }
            // A frame from the peer: it's there, so send it what's queued.
            wake(factory, peer);
            reply(mailbox.take(factory, peer, &id, &group, &envelope), peer)
        }
    }
}

/// The person's home before hands on a message (Fugo 33778 (a)): taken
/// as their device's would be, only from one of their Claktories bound
/// here now and only within the leaf's own rate, shared with its DmSend.
#[allow(clippy::too_many_arguments)]
fn hand_over(
    factory: &Arc<Factory>,
    peer: &str,
    id: &str,
    group_id: &str,
    envelope: &str,
    leaf_id: &str,
    client_message_id: &str,
    now: i64,
) -> Reply {
    if !bound(factory, peer) {
        return Reply::code("not_routed");
    }
    let (Ok(group), Ok(envelope)) = (B64.decode(group_id), B64.decode(envelope)) else { return Reply::code("bad") };
    if message_id(&envelope) != id || crate::dm_device::check_leaf(leaf_id).is_err() {
        return Reply::code("bad");
    }
    if let Err(wait) = crate::dm_device::within_leaf_rate(factory, leaf_id, now) {
        return Reply { code: "busy".to_owned(), retry_after_ms: wait.max(1000), ..Reply::default() };
    }
    let Some(mailbox) = mailbox(factory) else { return Reply::code("bad") };
    let handed = HandedOver { endpoint: peer, message_id: id, group: &group, envelope: &envelope, leaf_id, client_message_id };
    let taken = mailbox.take_handed(factory, handed);
    match &taken {
        // Queued for the friends' homes, as a device's send is: send it now.
        Ok(_) => wake_group(factory, &group),
        // Refused: it spends none of the leaf's sends.
        Err(_) => crate::dm_device::refund_leaf_rate(factory, leaf_id),
    }
    reply(taken, peer)
}

/// What the person's home before passes on (`Request::PassOn`): only from
/// one of their Claktories bound here by its hello, within the link's rate
/// for that peer; the intake checks the rest (Fugo 39876).
fn pass_on(factory: &Arc<Factory>, peer: &str, id: &str, group_id: &str, envelope: &str, account: &str, now: i64) -> Reply {
    if !bound(factory, peer) {
        return Reply::code("not_routed");
    }
    let (Ok(group), Ok(envelope)) = (B64.decode(group_id), B64.decode(envelope)) else { return Reply::code("bad") };
    if message_id(&envelope) != id {
        return Reply::code("bad");
    }
    if let Err(wait) = within_rate(lock(&factory.dm_link.taken).entry(peer.to_owned()).or_default(), now) {
        return Reply { code: "busy".to_owned(), retry_after_ms: wait.max(1000), ..Reply::default() };
    }
    let Some(mailbox) = mailbox(factory) else { return Reply::code("bad") };
    match mailbox.take_passed(factory, peer, id, &group, &envelope, account) {
        // Not yet (the group's state is on its way from claktory.com): soon.
        Err(Refused::Busy) => Reply { code: "busy".to_owned(), retry_after_ms: PASS_ON_RETRY_MS, ..Reply::default() },
        taken => reply(taken, peer),
    }
}

/// When a former home tries a message again that the new home couldn't
/// take yet.
const PASS_ON_RETRY_MS: i64 = 5_000;

/// The answer to what the intake did.
fn reply(taken: Result<Took, Refused>, peer: &str) -> Reply {
    match taken {
        Ok(Took::Stored) => Reply::code("took"),
        Ok(Took::Held) => Reply::code("held"),
        Ok(Took::Bounce { epoch }) => Reply { code: "too_old".to_owned(), epoch, ..Reply::default() },
        Err(Refused::NotRouted) => Reply::code("not_routed"),
        Err(Refused::Waiting) => Reply::code("waiting"),
        Err(Refused::Invalid(why)) => {
            tracing::warn!(endpoint = %peer, why, "a home sent a message that isn't one");
            Reply::code("bad")
        }
        Err(Refused::Full { retry_after_ms }) => Reply { code: "full".to_owned(), retry_after_ms, ..Reply::default() },
        Err(Refused::Busy) => Reply { code: "busy".to_owned(), retry_after_ms: 60_000, ..Reply::default() },
        Err(Refused::NotHome) => Reply::code("not_home"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_peer_delivers_60_a_minute_and_1000_an_hour() {
        let mut taken = VecDeque::new();
        let start = 10_000_000;
        for n in 0..60 {
            assert_eq!(within_rate(&mut taken, start + n), Ok(()));
        }
        assert_eq!(within_rate(&mut taken, start + 60), Err(60_000 - 60), "the 61st in the minute waits for the first to age out");
        // Past the minute, more; past 1,000 in the hour, none.
        let mut at = start + 60_000;
        while taken.len() < PER_HOUR {
            match within_rate(&mut taken, at) {
                Ok(()) => {}
                Err(wait) => at += wait,
            }
        }
        let hour_wait = within_rate(&mut taken, at).expect_err("the hour is full");
        assert_eq!(hour_wait, start + 3_600_000 - at);
        assert_eq!(within_rate(&mut taken, start + 3_600_000 + 1), Ok(()), "the first aged out");
    }

    #[test]
    fn requests_and_replies_keep_their_shape_on_the_wire() {
        let request = Request::Deliver { id: message_id(b"mls"), group_id: B64.encode(b"g1"), envelope: B64.encode(b"mls") };
        let bytes = frame(&request);
        assert_eq!(u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize, bytes.len() - 4);
        let body: serde_json::Value = serde_json::from_slice(&bytes[4..]).unwrap();
        assert_eq!(body["kind"], "deliver");
        assert_eq!(serde_json::from_slice::<Request>(&bytes[4..]).unwrap(), request);
        assert_eq!(serde_json::to_string(&Request::Knock).unwrap(), r#"{"kind":"knock"}"#);
        assert_eq!(serde_json::to_string(&Reply::code("took")).unwrap(), r#"{"code":"took"}"#);
        let bounce = Reply { code: "too_old".to_owned(), epoch: 9, ..Reply::default() };
        assert_eq!(serde_json::from_str::<Reply>(r#"{"code":"too_old","epoch":9}"#).unwrap(), bounce);
        let handed = Request::HandOver {
            id: message_id(b"mls"),
            group_id: B64.encode(b"g1"),
            envelope: B64.encode(b"mls"),
            leaf_id: "phone".to_owned(),
            client_message_id: "c1".to_owned(),
        };
        let body = serde_json::to_value(&handed).unwrap();
        assert_eq!((body["kind"].as_str(), body["leaf_id"].as_str()), (Some("hand_over"), Some("phone")));
        assert_eq!(serde_json::from_value::<Request>(body).unwrap(), handed);
        let passed =
            Request::PassOn { id: message_id(b"mls"), group_id: B64.encode(b"g1"), envelope: B64.encode(b"mls"), account: "bo".to_owned() };
        let body = serde_json::to_value(&passed).unwrap();
        assert_eq!((body["kind"].as_str(), body["account"].as_str()), (Some("pass_on"), Some("bo")));
        assert_eq!(serde_json::from_value::<Request>(body).unwrap(), passed);
    }
}
