//! Pushing notifications to devices that aren't connected, through the push
//! relay (`claktory-push`, notifications.md §3).
//!
//! A device registers with the relay, which gives it a handle, makes a
//! 32-byte key, and hands both to this Claktory (RegisterNotifications). They
//! live in `<home>/remote/notifications.json` (only this user can read it),
//! by device id, deliberately outside the vault: a locked vault mustn't
//! silence the alerts about what locked it. Removing the device forgets them.
//!
//! Each push is sealed with the device's key (AES-256-GCM, a random 96-bit
//! nonce): JSON padded to 512, 1024 or 2048 bytes, so its size says little,
//! holding a per-device counter and the send time, so the phone can tell a
//! replayed push (Fugo's change C). The relay, Apple and Google see only
//! ciphertext, and opaque collapse and thread ids (HMACs under a key derived
//! from the device's).

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use aws_lc_rs::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use factory_protocol::v1::{self, NotifyKind as Kind};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;

use crate::config::Config;
use crate::factory::{Factory, RequestError};

/// Claktory's own push relay, when the settings name none.
pub(crate) const RELAY: &str = "https://push.claktory.com";
/// The sizes a sealed notification's plaintext is padded to.
const BUCKETS: [usize; 3] = [512, 1024, 2048];
/// The longest title, in characters.
const MAX_TITLE: usize = 100;
/// How many times a push the relay couldn't take is tried.
const ATTEMPTS: u32 = 3;
/// How long a push to the relay may take.
const TIMEOUT: Duration = Duration::from_secs(15);

/// A device registered for pushes, as kept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Registration {
    /// Its handle at the relay, base64url.
    handle: String,
    /// Its key, base64.
    key: String,
    /// "apns", "apns_sandbox", "fcm" or "".
    #[serde(default)]
    platform: String,
    /// The last counter sealed into a push to it.
    #[serde(default)]
    ctr: u64,
    /// When it registered, in ms since 1970.
    #[serde(default)]
    registered: i64,
}

/// Pushes and the devices that get them.
#[derive(Default)]
pub(crate) struct State {
    /// notifications.json, once read.
    registry: Mutex<Option<BTreeMap<String, Registration>>>,
    /// Pushes waiting to be sent, oldest first.
    outbox: Mutex<VecDeque<Job>>,
}

/// One push for the relay.
pub(crate) struct Job {
    relay: String,
    device: String,
    /// The request's body.
    body: serde_json::Value,
    attempts: u32,
}

fn path(factory: &Factory) -> PathBuf {
    factory.home.root().join("remote").join("notifications.json")
}

/// The registrations, read the first time.
fn registry(factory: &Factory) -> MutexGuard<'_, Option<BTreeMap<String, Registration>>> {
    let mut registry = factory.notify.push.registry.lock().unwrap_or_else(PoisonError::into_inner);
    if registry.is_none() {
        let read = match std::fs::read(path(factory)) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                tracing::error!(%error, "notifications.json can't be read; devices register again");
                BTreeMap::new()
            }),
            Err(_) => BTreeMap::new(),
        };
        *registry = Some(read);
    }
    registry
}

fn save(factory: &Factory, registrations: &BTreeMap<String, Registration>) -> std::io::Result<()> {
    let path = path(factory);
    if let Some(dir) = path.parent() {
        crate::remote::private_dir(dir)?;
    }
    let mut json = serde_json::to_vec_pretty(registrations).map_err(std::io::Error::other)?;
    json.push(b'\n');
    crate::remote::write_private(&path, &json)
}

/// RegisterNotifications from `device`: its handle and key, or, with an
/// empty handle, none any more.
pub(crate) fn register(factory: &Factory, device: &str, request: &v1::RegisterNotifications) -> Result<(), RequestError> {
    let handle = request.handle.trim();
    if handle.is_empty() {
        forget(factory, device);
        return Ok(());
    }
    if URL_SAFE_NO_PAD.decode(handle).map_or(true, |bytes| bytes.len() != 32) {
        return Err(RequestError::Remote("That isn't a push relay's handle: 32 bytes, base64url.".to_owned()));
    }
    if request.key.len() != 32 {
        return Err(RequestError::Remote("A notification key is 32 bytes.".to_owned()));
    }
    let platform = match request.platform() {
        v1::PushPlatform::Apns => "apns",
        v1::PushPlatform::ApnsSandbox => "apns_sandbox",
        v1::PushPlatform::Fcm => "fcm",
        v1::PushPlatform::Unspecified => "",
    };
    let key = STANDARD.encode(&request.key);
    let mut registry = registry(factory);
    let registrations = registry.get_or_insert_default();
    let same_key = registrations.get(device).filter(|before| before.key == key);
    // A phone sends its registration again on every connect, to heal a
    // Claktory that lost it: the same one changes nothing.
    if same_key.is_some_and(|before| before.handle == handle && before.platform == platform) {
        return Ok(());
    }
    // Under the same key the counter goes on: the phone drops a push whose
    // counter isn't past the highest it saw. A new key starts again.
    let registration = Registration {
        handle: handle.to_owned(),
        ctr: same_key.map_or(0, |before| before.ctr),
        key,
        platform: platform.to_owned(),
        registered: crate::transcript::now_ms(),
    };
    let before = registrations.insert(device.to_owned(), registration);
    if let Err(error) = save(factory, registrations) {
        match before {
            Some(before) => registrations.insert(device.to_owned(), before),
            None => registrations.remove(device),
        };
        return Err(error.into());
    }
    drop(registry);
    tracing::info!(%device, platform, "a device registered for notifications");
    factory.broadcast_workspace(&factory.lock());
    Ok(())
}

/// `device` gets no more pushes: it unregistered, was removed, or the relay
/// no longer knows its handle.
pub(crate) fn forget(factory: &Factory, device: &str) {
    let mut registry = registry(factory);
    let registrations = registry.get_or_insert_default();
    if registrations.remove(device).is_none() {
        return;
    }
    if let Err(error) = save(factory, registrations) {
        tracing::error!(%device, %error, "could not forget a device's notification key");
    }
    tracing::info!(%device, "a device no longer gets notifications");
}

/// Whether `device` gets pushes (PairedDevice.notifications).
pub(crate) fn registered(factory: &Factory, device: &str) -> bool {
    registry(factory).as_ref().is_some_and(|registrations| registrations.contains_key(device))
}

/// Seals `notify` for each registered device it should be pushed to, and
/// queues it for the relay: devices that aren't in use, while the person
/// isn't using another (`State::push_waits`); every device for a test. How many it was queued for.
pub(crate) fn queue(factory: &Factory, config: &Config, settings: &super::NotificationsConfig, notify: &v1::Notify, now: i64) -> usize {
    let test = notify.kind() == Kind::Test;
    let paired: HashSet<String> = match crate::remote::devices(&factory.home) {
        Ok(devices) => devices.into_iter().map(|device| device.id).collect(),
        Err(error) => {
            tracing::error!(%error, "could not read the paired devices; nothing pushed");
            return 0;
        }
    };
    let relay = relay_of(settings);
    let badge = factory.notify.badge();
    let claktory = config.claktory_name();
    let mut jobs = Vec::new();
    {
        let mut registry = registry(factory);
        let registrations = registry.get_or_insert_default();
        let gone: Vec<String> = registrations.keys().filter(|device| !paired.contains(*device)).cloned().collect();
        for device in &gone {
            registrations.remove(device);
        }
        for (device, registration) in registrations.iter_mut() {
            if !test && factory.notify.push_waits(device, now) {
                continue;
            }
            let Some(key) = STANDARD.decode(&registration.key).ok().filter(|key| key.len() == 32) else { continue };
            registration.ctr += 1;
            match seal_push(&key, registration.ctr, now, &claktory, badge, notify) {
                Some(sealed) => jobs.push(Job {
                    relay: relay.clone(),
                    device: device.clone(),
                    body: json!({
                        "handle": registration.handle,
                        "ciphertext": sealed.ciphertext,
                        "urgency": if notify.urgent { "urgent" } else { "normal" },
                        "collapse": sealed.collapse,
                        "thread": sealed.thread,
                        "ttl": ttl(notify.kind()),
                    }),
                    attempts: 0,
                }),
                None => tracing::error!(%device, "could not seal a notification"),
            }
        }
        if (!jobs.is_empty() || !gone.is_empty())
            && let Err(error) = save(factory, registrations)
        {
            // Without the counter saved, a later push could repeat it, and
            // the phone would show it as a replay: send nothing.
            tracing::error!(%error, "could not save the notification counters; nothing pushed");
            return 0;
        }
    }
    let count = jobs.len();
    if count > 0 {
        factory.notify.push.outbox.lock().unwrap_or_else(PoisonError::into_inner).extend(jobs);
        factory.notify.wake();
    }
    count
}

/// The relay pushes go to.
fn relay_of(settings: &super::NotificationsConfig) -> String {
    match settings.relay.as_str() {
        "" => RELAY.to_owned(),
        relay => crate::account::service_url(relay).unwrap_or_else(|| RELAY.to_owned()),
    }
}

/// How long the relay, and Apple or Google, keep trying to deliver a push,
/// in seconds; the phone shows one older than this (and a few minutes) as
/// the generic line.
fn ttl(kind: Kind) -> u64 {
    match kind {
        Kind::NeedsYou | Kind::Alert | Kind::Unspecified => 24 * 3600,
        Kind::Reply => 3600,
        Kind::Test => 600,
    }
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::NeedsYou => "needs_you",
        Kind::Alert => "alert",
        Kind::Reply => "reply",
        Kind::Test => "test",
        Kind::Unspecified => "",
    }
}

/// A notification sealed for one device.
#[derive(Debug)]
pub(crate) struct Sealed {
    /// base64 of nonce ‖ ciphertext ‖ tag.
    pub ciphertext: String,
    /// Opaque ids for the relay: empty when it has none.
    pub collapse: String,
    pub thread: String,
}

/// Seals `notify` under `key` as push number `ctr`, sent at `now`.
pub(crate) fn seal_push(key: &[u8], ctr: u64, now: i64, claktory: &str, badge: u32, notify: &v1::Notify) -> Option<Sealed> {
    let title: String = notify.title.chars().take(MAX_TITLE).collect();
    let mut body = notify.body.clone();
    let plaintext = loop {
        let payload = json!({
            "v": 1,
            "id": notify.id,
            "ctr": ctr,
            "sent_at": now,
            "ttl": ttl(notify.kind()),
            "claktory": claktory,
            "kind": kind_name(notify.kind()),
            "title": title,
            "body": body,
            "badge": badge,
            "urgent": notify.urgent,
            "target": {"agent": notify.agent_id, "pipe": notify.pipe_id, "chat": notify.chat_id, "seq": notify.seq},
        });
        let bytes = serde_json::to_vec(&payload).ok()?;
        if bytes.len() <= BUCKETS[BUCKETS.len() - 1] {
            break bytes;
        }
        if body.is_empty() {
            return None;
        }
        // Too long for the largest size: shorter.
        let keep = body.chars().count() * 3 / 4;
        body = body.chars().take(keep).collect::<String>() + "…";
    };
    let size = BUCKETS.iter().copied().find(|size| plaintext.len() <= *size)?;
    let mut padded = plaintext;
    // JSON ignores whitespace after its value.
    padded.resize(size, b' ');
    let ciphertext = seal(key, &padded)?;
    Some(Sealed { ciphertext, collapse: opaque(key, "collapse", &notify.collapse), thread: opaque(key, "thread", &notify.thread) })
}

/// AES-256-GCM under `key`, with a random nonce: base64 of nonce ‖
/// ciphertext ‖ tag.
pub(crate) fn seal(key: &[u8], plaintext: &[u8]) -> Option<String> {
    let cipher = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).ok()?);
    let mut nonce = [0u8; 12];
    getrandom::fill(&mut nonce).ok()?;
    let mut sealed = plaintext.to_vec();
    cipher.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut sealed).ok()?;
    Some(STANDARD.encode([&nonce[..], &sealed].concat()))
}

/// An id the relay and Apple can't read names out of: 16 bytes of an HMAC
/// of `value` under a key derived from the device's, base64url (22
/// characters, inside APNs's 64). Empty for an empty value.
fn opaque(key: &[u8], purpose: &str, value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let mut derived = [0u8; 32];
    if hkdf::Hkdf::<Sha256>::new(None, key).expand(format!("claktory-push {purpose}").as_bytes(), &mut derived).is_err() {
        return String::new();
    }
    let mut mac = <hmac::Hmac<Sha256> as hmac::KeyInit>::new_from_slice(&derived).expect("any key length");
    hmac::Mac::update(&mut mac, value.as_bytes());
    let tag = hmac::Mac::finalize(mac).into_bytes();
    URL_SAFE_NO_PAD.encode(&tag[..16])
}

/// Sends the pushes waiting. Those the relay couldn't take for now are
/// tried again later (true: some wait).
pub(crate) async fn flush(factory: &Factory) -> bool {
    let jobs: Vec<Job> = factory.notify.push.outbox.lock().unwrap_or_else(PoisonError::into_inner).drain(..).collect();
    if jobs.is_empty() {
        return false;
    }
    let client = match reqwest::Client::builder().timeout(TIMEOUT).build() {
        Ok(client) => client,
        Err(error) => {
            tracing::error!(%error, "no HTTP client for pushes");
            return false;
        }
    };
    let mut again = Vec::new();
    let mut forgotten = false;
    for mut job in jobs {
        let url = format!("{}/v1/push", job.relay);
        match client.post(&url).json(&job.body).send().await {
            Ok(response) if response.status().is_success() => {}
            // The relay no longer knows the handle (the app was deleted, or
            // Apple or Google retired its token).
            Ok(response) if matches!(response.status().as_u16(), 404 | 410) => {
                tracing::info!(device = %job.device, "the push relay no longer knows a device's handle");
                forget(factory, &job.device);
                forgotten = true;
            }
            Ok(response) if response.status().is_server_error() || response.status().as_u16() == 429 => {
                tracing::warn!(device = %job.device, status = %response.status(), "the push relay couldn't take a push for now");
                job.attempts += 1;
                if job.attempts < ATTEMPTS {
                    again.push(job);
                }
            }
            Ok(response) => tracing::error!(device = %job.device, status = %response.status(), "the push relay refused a push"),
            Err(error) => {
                tracing::warn!(device = %job.device, %error, "could not reach the push relay");
                job.attempts += 1;
                if job.attempts < ATTEMPTS {
                    again.push(job);
                }
            }
        }
    }
    if forgotten {
        factory.broadcast_workspace(&factory.lock());
    }
    let waiting = !again.is_empty();
    if waiting {
        let mut outbox = factory.notify.push.outbox.lock().unwrap_or_else(PoisonError::into_inner);
        for job in again.into_iter().rev() {
            outbox.push_front(job);
        }
    }
    waiting
}
