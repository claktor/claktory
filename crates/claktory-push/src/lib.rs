//! Claktory's push relay (notifications.md §3, in Research): Claktories push
//! notifications to their people's phones through it, since only the holder
//! of Claktory's Apple key can push to `com.claktory.app` (and Google's
//! service account for Android).
//!
//! It learns almost nothing. A phone registers its push token and gets back a
//! random handle, which it gives to its Claktory: whoever holds a handle can
//! push to that one device, within limits. The relay doesn't know which
//! person, account or Claktory a handle belongs to, and keeps only the
//! handle's SHA-256, so its database hands out no capabilities. A push's
//! content is sealed by the Claktory with a key only the phone has; the
//! relay passes the ciphertext on, under an alert text it alone sets ("Claktory
//! has something for you"): its API takes no text from callers, so a caller
//! can't put words on anyone's lock screen.
//!
//! API (JSON over HTTPS, behind a TLS proxy):
//! - `POST /v1/register {platform, token}` → `{handle}`. platform: "apns",
//!   "apns_sandbox" or "fcm". Limited per IP.
//! - `POST /v1/push {handle, ciphertext, urgency?, collapse?, thread?, ttl?}`
//!   → `{}`. ciphertext: base64, at most `MAX_CIPHERTEXT` bytes decoded.
//!   urgency: "normal" (the default), "urgent" (time-sensitive) or "low".
//!   collapse, thread: opaque ids, base64url, at most 64 characters. ttl:
//!   seconds Apple or Google keep trying, at most a week. 404 `unknown_handle`
//!   or 410 `gone` (Apple or Google retired the token: the handle is deleted):
//!   forget the handle. 429 `rate_limited` with `retry_after`. 502
//!   `unavailable`: try again later.
//! - `POST /v1/unregister {handle}` → `{}`.
//! - `GET /v1/health` → `{ok, apns, fcm}`.

pub mod apns;
pub mod db;
pub mod fcm;
mod limits;
pub mod server;

use std::future::Future;
use std::pin::Pin;

/// The only alert text the relay ever sends.
pub const ALERT_TITLE: &str = "Claktory";
pub const ALERT_BODY: &str = "Claktory has something for you";

/// The most a push's ciphertext may be, decoded: a 2048-byte payload, its
/// nonce and its tag, with room to spare.
pub const MAX_CIPHERTEXT: usize = 2200;
/// The longest a push is kept trying, in seconds.
pub const MAX_TTL: u64 = 7 * 24 * 3600;

/// Where a device gets its pushes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Apns,
    /// An app built for development, on APNs's sandbox.
    ApnsSandbox,
    Fcm,
}

impl Platform {
    pub fn parse(text: &str) -> Option<Platform> {
        match text {
            "apns" => Some(Platform::Apns),
            "apns_sandbox" => Some(Platform::ApnsSandbox),
            "fcm" => Some(Platform::Fcm),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Apns => "apns",
            Platform::ApnsSandbox => "apns_sandbox",
            Platform::Fcm => "fcm",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Urgency {
    Normal,
    /// Time-sensitive: it may break through Focus.
    Urgent,
    /// Delivered when it suits the phone.
    Low,
}

/// One push, as the relay hands it to Apple or Google.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Push {
    /// base64, as the Claktory sealed it.
    pub ciphertext: String,
    pub urgency: Urgency,
    /// Opaque; empty for none.
    pub collapse: String,
    pub thread: String,
    /// Seconds.
    pub ttl: u64,
}

/// What became of a push.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Delivery {
    Sent,
    /// The token is no longer valid: the app was deleted, or it got a new one.
    Gone,
    /// Apple or Google can't take it now.
    Transient(String),
    /// Refused for good, or this relay can't push there.
    Refused(String),
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Sends pushes to Apple or Google; tests use one of their own.
pub trait Transport: Send + Sync {
    fn send<'a>(&'a self, platform: Platform, token: &'a str, push: &'a Push) -> BoxFuture<'a, Delivery>;
    /// Which platforms it can push to: (APNs, FCM).
    fn platforms(&self) -> (bool, bool);
}

/// Apple's and Google's gateways, as configured.
pub struct Gateways {
    pub apns: Option<apns::Apns>,
    pub fcm: Option<fcm::Fcm>,
}

impl Transport for Gateways {
    fn send<'a>(&'a self, platform: Platform, token: &'a str, push: &'a Push) -> BoxFuture<'a, Delivery> {
        Box::pin(async move {
            match platform {
                Platform::Apns | Platform::ApnsSandbox => match &self.apns {
                    Some(apns) => apns.send(platform == Platform::ApnsSandbox, token, push).await,
                    None => Delivery::Refused("this relay has no Apple key".to_owned()),
                },
                Platform::Fcm => match &self.fcm {
                    Some(fcm) => fcm.send(token, push).await,
                    None => Delivery::Refused("this relay has no Firebase service account".to_owned()),
                },
            }
        })
    }

    fn platforms(&self) -> (bool, bool) {
        (self.apns.is_some(), self.fcm.is_some())
    }
}

/// The time, in unix seconds.
pub fn now() -> i64 {
    jiff::Timestamp::now().as_second()
}

/// base64url without padding, as handles and JWTs write bytes.
pub(crate) fn b64url(bytes: impl AsRef<[u8]>) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The DER inside a PEM file's first block.
pub(crate) fn pem_der(pem: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let body: String = pem.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with("-----")).collect();
    base64::engine::general_purpose::STANDARD.decode(body).ok()
}
