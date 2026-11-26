//! Following one conversation without selecting it (#514, spec §9.8:
//! focus mode's Context). A connection watches at most one agent, chat or
//! pipe (WatchTranscript): its snapshot comes as selecting it sends it,
//! then its agent's or chat's changes and queue, to that connection alone,
//! each with ServerMessage.watched. Watching only reads: the selection,
//! `unseen` and what's read stay as they are, and nothing is tainted.
//! A pipe's messages already reach every connection (PipeMessageUpdate),
//! so a watched pipe gets only its snapshot here. It ends when the
//! connection closes, and when what it watches is deleted.
//!
//! Changes are sent with the Factory's lock held (`Factory::send_change`),
//! and a watch starts with it held too, from the frame counted last: what
//! it reads in its snapshot is never sent to it again.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use bytes::Bytes;
use factory_protocol::v1::{self, server_message::Body};
use prost::Message as _;
use tokio::sync::broadcast;

use crate::factory::{Factory, Inner, RequestError};

/// How many frames wait for a slow connection before it's sent its
/// snapshot again.
const BACKLOG: usize = 1024;

pub(crate) fn agent_key(id: &str) -> String {
    format!("agent:{id}")
}

pub(crate) fn chat_key(id: &str) -> String {
    format!("chat:{id}")
}

pub(crate) fn pipe_key(id: &str) -> String {
    format!("pipe:{id}")
}

/// A frame for whoever watches `key`, the `n`th; None: it was deleted.
#[derive(Clone)]
struct Watched {
    key: Arc<str>,
    n: u64,
    frame: Option<Bytes>,
}

struct Shared {
    sender: broadcast::Sender<Watched>,
    /// The latest frame counted.
    counter: AtomicU64,
    /// How many connections watch each key.
    watchers: Mutex<HashMap<String, usize>>,
    /// The queue last sent for each agent or chat watched.
    queued: Mutex<HashMap<String, Bytes>>,
}

/// The Factory's side: what's watched, and the frames for its watchers.
pub(crate) struct Hub(Arc<Shared>);

impl Default for Hub {
    fn default() -> Hub {
        let (sender, _) = broadcast::channel(BACKLOG);
        Hub(Arc::new(Shared { sender, counter: AtomicU64::new(0), watchers: Mutex::default(), queued: Mutex::default() }))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `body` as a watched conversation's message.
fn encode(body: Body) -> Bytes {
    v1::ServerMessage { body: Some(body), watched: true, ..Default::default() }.encode_to_vec().into()
}

impl Hub {
    /// Whether a connection watches `key` now.
    pub(crate) fn watched(&self, key: &str) -> bool {
        lock(&self.0.watchers).contains_key(key)
    }

    /// Sends `body` to the connections watching `key` (with the Factory's
    /// lock held).
    pub(crate) fn send(&self, key: &str, body: Body) {
        if !self.watched(key) {
            return;
        }
        self.publish(key, Some(encode(body)));
    }

    fn publish(&self, key: &str, frame: Option<Bytes>) {
        let n = self.0.counter.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.0.sender.send(Watched { key: key.into(), n, frame });
    }

    /// `key` was deleted: its watches end, with nothing more sent.
    pub(crate) fn gone(&self, key: &str) {
        if !self.watched(key) {
            return;
        }
        lock(&self.0.queued).remove(key);
        self.publish(key, None);
    }

    /// The queues of the agents and chats watched, sent when they changed
    /// (with each Workspace, as the selected agent's is).
    pub(crate) fn queues(&self, inner: &Inner) {
        let keys: Vec<String> = lock(&self.0.watchers).keys().cloned().collect();
        for key in keys {
            let Some(body) = queue(inner, &key) else { continue };
            let frame = encode(body);
            let mut queued = lock(&self.0.queued);
            if queued.get(&key) == Some(&frame) {
                continue;
            }
            queued.insert(key.clone(), frame.clone());
            drop(queued);
            self.publish(&key, Some(frame));
        }
    }
}

/// The queue of the agent or chat `key` names; None for a pipe's.
fn queue(inner: &Inner, key: &str) -> Option<Body> {
    if let Some(id) = key.strip_prefix("agent:") {
        return Some(Body::QueuedMessages(crate::factory::queued_for(inner, id)));
    }
    let id = key.strip_prefix("chat:")?;
    Some(Body::ChatQueuedMessages(crate::chats::queue_of(inner, id)))
}

/// A connection's side: what it watches, and the frames for it.
pub(crate) struct Port {
    shared: Arc<Shared>,
    receiver: tokio::sync::Mutex<broadcast::Receiver<Watched>>,
    /// The key watched, and the latest frame counted when it started.
    target: Mutex<Option<(String, u64)>>,
}

impl Port {
    pub(crate) fn new(factory: &Factory) -> Port {
        let shared = factory.watches.0.clone();
        let receiver = shared.sender.subscribe();
        Port { shared, receiver: tokio::sync::Mutex::new(receiver), target: Mutex::new(None) }
    }

    /// Watches `key` from now on (with the Factory's lock held, for an
    /// agent or a chat), instead of what it watched.
    fn start(&self, key: String) {
        self.stop();
        let from = self.shared.counter.load(Ordering::SeqCst);
        *lock(&self.shared.watchers).entry(key.clone()).or_default() += 1;
        *lock(&self.target) = Some((key, from));
    }

    /// Watches nothing.
    fn stop(&self) {
        let Some((key, _)) = lock(&self.target).take() else { return };
        let mut watchers = lock(&self.shared.watchers);
        if let Some(count) = watchers.get_mut(&key) {
            *count -= 1;
            if *count == 0 {
                watchers.remove(&key);
                lock(&self.shared.queued).remove(&key);
            }
        }
    }

    /// WatchTranscript: what it watches now, and its snapshot to send
    /// before the answer.
    pub(crate) async fn watch(&self, factory: &Arc<Factory>, request: &v1::WatchTranscript) -> Result<Vec<Bytes>, RequestError> {
        use v1::watch_transcript::Target;
        match &request.target {
            None => {
                self.stop();
                Ok(Vec::new())
            }
            Some(Target::AgentId(id)) => {
                factory.known_agent(id)?;
                factory.ensure_loaded(id).await?;
                let inner = factory.lock();
                self.start(agent_key(id));
                Ok(agent_snapshot(&inner, id))
            }
            Some(Target::ChatId(id)) => {
                crate::chats::known(factory, id)?;
                factory.ensure_loaded(id).await?;
                let inner = factory.lock();
                self.start(chat_key(id));
                Ok(chat_snapshot(&inner, id))
            }
            Some(Target::PipeId(id)) => {
                if factory.pipes.get(id).is_none() {
                    return Err(RequestError::NoPipe(id.clone()));
                }
                self.start(pipe_key(id));
                Ok(vec![encode(Body::PipeTranscript(crate::pipes::transcript(factory, id)))])
            }
        }
    }

    /// The next frames for this connection: its watched conversation's, or
    /// its snapshot again after it fell behind.
    pub(crate) async fn next(&self, factory: &Factory) -> Vec<Bytes> {
        let mut receiver = self.receiver.lock().await;
        loop {
            match receiver.recv().await {
                Ok(watched) => {
                    let mut target = lock(&self.target);
                    let Some((key, from)) = target.as_ref() else { continue };
                    if **key != *watched.key || watched.n <= *from {
                        continue;
                    }
                    match watched.frame {
                        Some(frame) => return vec![frame],
                        None => {
                            *target = None;
                            drop(target);
                            let mut watchers = lock(&self.shared.watchers);
                            if let Some(count) = watchers.get_mut(&*watched.key) {
                                *count -= 1;
                                if *count == 0 {
                                    watchers.remove(&*watched.key);
                                }
                            }
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let key = lock(&self.target).as_ref().map(|(key, _)| key.clone());
                    let Some(key) = key else { continue };
                    let inner = factory.lock();
                    if let Some((_, from)) = lock(&self.target).as_mut() {
                        *from = self.shared.counter.load(Ordering::SeqCst);
                    }
                    let frames = snapshot(factory, &inner, &key);
                    if !frames.is_empty() {
                        return frames;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return std::future::pending().await,
            }
        }
    }
}

impl Drop for Port {
    fn drop(&mut self) {
        self.stop();
    }
}

/// What a watch of `key` starts with.
fn snapshot(factory: &Factory, inner: &Inner, key: &str) -> Vec<Bytes> {
    if let Some(id) = key.strip_prefix("agent:") {
        agent_snapshot(inner, id)
    } else if let Some(id) = key.strip_prefix("chat:") {
        chat_snapshot(inner, id)
    } else if let Some(id) = key.strip_prefix("pipe:") {
        vec![encode(Body::PipeTranscript(crate::pipes::transcript(factory, id)))]
    } else {
        Vec::new()
    }
}

fn agent_snapshot(inner: &Inner, id: &str) -> Vec<Bytes> {
    let transcript =
        crate::factory::transcript_of(inner, id).unwrap_or_else(|| v1::Transcript { agent_id: id.to_owned(), ..Default::default() });
    vec![encode(Body::Transcript(transcript)), encode(Body::QueuedMessages(crate::factory::queued_for(inner, id)))]
}

fn chat_snapshot(inner: &Inner, id: &str) -> Vec<Bytes> {
    let (entries, has_earlier) =
        inner.agents.get(id).and_then(|runtime| runtime.transcript.as_ref()).map(crate::transcript::Transcript::recent).unwrap_or_default();
    let transcript = v1::ChatTranscript { chat_id: id.to_owned(), entries, has_earlier };
    vec![encode(Body::ChatTranscript(transcript)), encode(Body::ChatQueuedMessages(crate::chats::queue_of(inner, id)))]
}
