//! This Claktory's account at claktory.com (#116), or at a service of the
//! person's own (`account.service`): the client of `claktory-account`, whose
//! API (crates/claktory-account/API.md) is the contract.
//!
//! - Signing in is the backend's: the app sends the email (AccountSignIn),
//!   then the emailed code (AccountVerify), and every device shows the state
//!   (`Workspace.account`).
//! - Signed in, the vault is protected by the account instead of a key on
//!   disk: its key is sealed under `HKDF(L ‖ S)`, L kept here and S released
//!   by the service only to this Claktory (`vault::account`). After a restart
//!   the backend asks for S again, with its machine token and backend key,
//!   and opens the vault; while the service can't be reached it tries again,
//!   and agents can't use secrets meanwhile. Updates hand the vault's key
//!   over and never need the service (`vault::handoff`).
//! - It checks in at start and every hour, and registers the iroh endpoint
//!   ids of the backend and its paired devices.
//!
//! Files, in `<home>/account/` (0600): `account.json` (the sign-in: service,
//! its pinned key, account, email, Claktory id, machine token, epoch, and
//! what was last sent), `claktory.key` (the backend key, Ed25519) and
//! `share.key` (L). Off unless `account.enabled`: then nothing here calls
//! the service and the app isn't asked to sign in.

pub(crate) mod analytics;
pub(crate) mod anchor;
pub(crate) mod apps;
pub(crate) mod moves;
pub(crate) mod recovery;
pub(crate) mod relay_token;
pub(crate) mod reports;
pub(crate) mod vouch;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use claktory_account::protocol::{
    self as api, AuthorizePoll, AuthorizeStart, AuthorizeStarted, Authorized, CheckIn, CheckedIn, ClaktoryAuthorizations, Endpoints,
    Ephemeral, ErrorBody, MoveDone, MoveRequest, Share, ShareRequest, SignInStart, SignInStarted, SignInVerify, SignedIn, WellKnown,
};
use ed25519_dalek::{SigningKey, VerifyingKey};
use factory_protocol::v1::{self, ErrorCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use zeroize::Zeroizing;

use crate::config::AccountConfig;
use crate::encoding::unhex;
use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;

/// Claktory's own account service.
pub const DEFAULT_SERVICE: &str = "https://account.claktory.com";

/// How long a call to the service may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// After a restart, the vault's share is asked for again this long after
/// each try that couldn't reach the service; then every 5 minutes.
const UNLOCK_RETRIES: [Duration; 3] = [Duration::from_secs(5), Duration::from_secs(30), Duration::from_secs(120)];
const UNLOCK_RETRY_LAST: Duration = Duration::from_secs(300);
/// The service releases a share once a minute: an app connecting or an
/// agent asking for a secret tries again at once only if the last try
/// didn't reach it, or was this long ago.
const SHARE_GAP: Duration = Duration::from_secs(60);
/// Check-ins: at start and every hour, give or take 5 minutes; 15 minutes
/// after one that failed.
const CHECK_IN_EVERY: Duration = Duration::from_secs(3600);
const CHECK_IN_JITTER: u64 = 300;
const CHECK_IN_RETRY: Duration = Duration::from_secs(900);
/// Endpoint ids the service takes, at most.
const MAX_ENDPOINTS: usize = 20;

/// `account.json`: the sign-in, and what was last sent. Every field has a
/// default, so older and newer files read.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    /// This Claktory at the service; kept across sign-outs, so signing in
    /// again is the same Claktory there.
    claktory_id: String,
    /// Signed in: the service, its pinned key (b64), the account and the
    /// email, the machine token and the share's epoch. Empty when not.
    service: String,
    service_key: String,
    /// When the account root's statement for the pinned key says it's the
    /// service's from (unix seconds); None when there was none (`anchor`).
    service_key_from: Option<i64>,
    account_id: String,
    email: String,
    machine_token: String,
    epoch: u64,
    /// The name sent at sign-in.
    name: String,
    /// Removed at the service: share, check-ins and endpoints are refused.
    revoked: bool,
    last_contact_ms: i64,
    /// Exactly what was last sent, for "What claktory.com receives".
    last_sign_in_json: String,
    last_sign_in_ms: i64,
    last_check_in_json: String,
    last_check_in_ms: i64,
    /// Periods made for check-ins and not yet accepted, oldest first
    /// (`analytics`), and where the next one starts (unix seconds, a whole
    /// hour; 0 before the first sign-in).
    periods: Vec<api::Period>,
    periods_until: i64,
    /// The sign-in step: "asked", which asks until it's signed in. Every
    /// Claktory must sign in (#195), so a start with signing in on makes it
    /// "asked" (`State::new`), also over "offered" (a Claktory from before
    /// sign-in, which could skip it) and "skipped" (one that did), which
    /// earlier versions wrote; kept so a rollback to one still asks.
    step: String,
    /// When it last signed in (the code was given), in ms since 1970; 0 for
    /// a sign-in from before this was kept, which is never new.
    signed_in_ms: i64,
    /// The move epoch (`<home>/move-epoch`, #217) its token and backend key
    /// were last changed for: a Claktory that moved here changes them once
    /// (`after_move`), so the copy it moved from can't use them.
    move_epoch: u64,
    /// Sign-outs the service hasn't heard of yet (#524), one per service:
    /// told when it can be reached (`tell_sign_outs`).
    signing_out: Vec<SigningOut>,
    /// The account's recovery key (#239), when claktory.com's last matched
    /// the one the vault is sealed to: the Claktory that set it (its id at
    /// the service) and when, in ms since 1970. Empty when it didn't.
    recovery_made_by: String,
    /// Its name as claktory.com gave it then (untrusted, `recovery::name`).
    recovery_made_name: String,
    recovery_made_ms: i64,
    /// claktory.com's key when it's another, not verified: it waits for
    /// the person (`recovery::use_new`).
    recovery_waiting: Option<recovery::Waiting>,
    /// The account's key as claktory.com last gave it (its public half),
    /// whatever the vault is sealed to; empty while it has none. What a new
    /// key made here replaces (`recovery::make`).
    recovery_account_key: String,
    /// How far its check-ins have heard of device keys removed from the
    /// account (E11-8 point 6): the service's `removed_cursor`, an opaque
    /// word, sent back as `removed_since`; empty for none yet.
    removed_cursor: String,
    /// Signed in with no code, approved on a device of the person's: that
    /// device's account key, the only device let in at once, within
    /// `FIRST_DEVICE_WINDOW` of `signed_in_ms`. Empty for a sign-in with the
    /// emailed code, which lets in the first device that comes.
    first_device: String,
    /// The account key this Claktory last vouched for, this computer's own
    /// app's (#529, `vouch`), with the sign-in it vouched under (#619).
    vouched_app: Option<VouchedApp>,
}

/// This computer's app, as vouched for: its account key, the account and
/// the sign-in's epoch then. It counts only while the same sign-in holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct VouchedApp {
    account_id: String,
    epoch: u64,
    account_key: String,
}

/// A sign-out to tell a service of: the sign-in it ends. The backend key
/// is the one that signed in, which the sign-out replaced here.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct SigningOut {
    service: String,
    service_key: String,
    machine_token: String,
    /// The backend key's seed, hex.
    backend_key: String,
    /// When it signed out here, in ms since 1970.
    since_ms: i64,
    /// Tries that didn't reach it, for the wait before the next.
    tries: u32,
}

impl SigningOut {
    fn key(&self) -> Option<SigningKey> {
        let bytes = Zeroizing::new(unhex(&self.backend_key)?);
        Some(SigningKey::from_bytes(&<[u8; 32]>::try_from(bytes.as_slice()).ok()?))
    }
}

/// The service's path for a Claktory's sign-out.
const SIGN_OUT_PATH: &str = "/v1/claktories/sign-out";
/// How long a sign-out waits for the service before it's queued.
const SIGN_OUT_WAIT: Duration = Duration::from_secs(5);

const ASKED: &str = "asked";

/// Where codes come from when the service doesn't say: claktory.com's.
const DEFAULT_CODE_FROM: &str = "code@auth.claktory.com";

impl Saved {
    fn signed_in(&self) -> bool {
        !self.machine_token.is_empty()
    }
}

/// Whether this Claktory was never signed in to an account, as
/// `<home>/account` keeps it, read without the backend (`factoryd pair
/// --show-code`): no file yet, or one that never signed in. One that can't
/// be read counts as signed in.
pub(crate) fn never_signed_in_here(home: &crate::home::Home) -> bool {
    match std::fs::read(home.root().join("account").join("account.json")) {
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
        Ok(bytes) => serde_json::from_slice::<Saved>(&bytes)
            .is_ok_and(|saved| !saved.signed_in() && saved.signed_in_ms == 0 && saved.last_check_in_ms == 0 && saved.signing_out.is_empty()),
    }
}

/// The account protector's params in the vault: what opening it needs.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Params {
    service: String,
    account_id: String,
    claktory_id: String,
    epoch: u64,
    service_key: String,
}

/// A code emailed, waiting for AccountVerify. In memory only: after a
/// restart the person asks for another.
#[derive(Clone)]
struct Pending {
    service: String,
    service_key: String,
    /// Its statement's `from`, if it had one.
    key_from: Option<i64>,
    start: SignInStart,
    request_id: String,
    expires_ms: i64,
    /// The address the code comes from; empty when unknown.
    code_from: String,
    /// Waiting for the person to approve it on a device instead of for a
    /// code: AccountAuthorize, or a code the account answered with
    /// `authorization_pending`.
    approval: Option<Approval>,
}

/// A request to join the account waiting for the person's approval, polled
/// every `interval` (`poll_approval`).
#[derive(Clone)]
struct Approval {
    interval: Duration,
    next_poll: Instant,
}

impl Approval {
    fn new(interval_s: i64) -> Approval {
        let interval = Duration::from_secs(interval_s.clamp(1, 60) as u64);
        Approval { interval, next_poll: Instant::now() + interval }
    }
}

struct Inner {
    saved: Saved,
    pending: Option<Pending>,
    /// What went wrong with the last call, for the person; empty when fine.
    problem: String,
    /// The service's clock minus this one's, in seconds, from `clock_skew`.
    clock_offset: i64,
    next_unlock: Instant,
    unlock_tries: usize,
    /// The share can't open the vault (refused, or another epoch's): no
    /// more tries until the next sign-in or restart.
    unlock_stopped: bool,
    /// When a share request last reached the service.
    share_asked: Option<Instant>,
    next_check_in: Instant,
    /// The endpoints last registered, so they're sent only when changed.
    endpoints_sent: Option<Endpoints>,
    /// The vault was opened by hand (passphrase or recovery key) while
    /// signed in: when to seal it under the account's current share, so it
    /// opens by itself after the next restart.
    reseal_due: Option<Instant>,
    /// Each signed request's last time, by a digest of what else it signs
    /// (`stamp`), for the last `MAX_SKEW` seconds.
    stamped: std::collections::HashMap<[u8; 32], i64>,
    /// When the sign-outs not yet told are tried again.
    next_sign_out: Instant,
    /// When a check-in since this backend started last heard of the
    /// account's removed devices, and when a device's Hello last asked for
    /// one (`removals_fresh`).
    removals_heard: Option<Instant>,
    removals_asked: Option<Instant>,
    /// A computer asking to join the account, waiting for the person's
    /// approval, by request id: the bell their devices hear. Asked for
    /// every `BELL_EVERY` while a device is connected, and heard with
    /// every check-in.
    waiting: Vec<String>,
    waiting_asked: Option<Instant>,
    /// Moves to another computer the person approved with this Claktory as
    /// their source (`moves`): when they were last asked for, until when
    /// they're asked for often (a device of the person's just asked for
    /// this Claktory's word to approve one), and those this Claktory
    /// turned down, by request id, so they aren't checked again.
    moves_asked: Option<Instant>,
    moves_soon_until: Option<Instant>,
    moves_turned_down: Vec<String>,
    /// A recovery key a device is making for the account, waiting for
    /// its signature (`recovery::make`, `recovery::publish`).
    recovery_making: Option<recovery::Making>,
}

/// The time to sign a request with, `now` by the service's clock: now, or
/// a second past the last time the same request was signed (`stamped`, by
/// a digest of what else it signs). Signatures are deterministic, so the
/// same request twice in one second (several callers at once, as when the
/// service comes back after a restart) would be refused as a replay. Never
/// more than a minute ahead, well within what the service accepts.
fn stamp(stamped: &mut std::collections::HashMap<[u8; 32], i64>, now: i64, method: &str, path: &str, body: &[u8]) -> i64 {
    use sha2::Digest;
    stamped.retain(|_, time| *time > now - api::MAX_SKEW);
    let mut digest = sha2::Sha256::new();
    for part in [method.as_bytes(), b"\n", path.as_bytes(), b"\n", body] {
        digest.update(part);
    }
    let key: [u8; 32] = digest.finalize().into();
    let time = match stamped.get(&key) {
        Some(&last) if last >= now && last < now + 60 => last + 1,
        _ => now,
    };
    stamped.insert(key, time);
    time
}

/// The account's state, held by `Factory`.
pub(crate) struct State {
    dir: PathBuf,
    inner: Mutex<Inner>,
    /// One sign-in, unlock or check-in at a time.
    busy: tokio::sync::Mutex<()>,
    /// Wakes `monitor`: something's due now.
    wake: Notify,
    /// Notified after every check-in, whatever its outcome.
    checked: Notify,
    http: reqwest::Client,
    /// Replies reported lately (`reports`).
    reports: reports::Sent,
    /// Tests: a Factory a test is done with, whose home another now uses
    /// in the same process; it neither runs nor writes account.json again
    /// (`stop_for_tests`).
    stopped: std::sync::atomic::AtomicBool,
}

/// Why a call to the service failed.
enum Failure {
    /// It couldn't be reached, or the answer isn't the service's (its
    /// signature) or can't be read.
    Unreachable(String),
    /// It answered with an error.
    Refused(u16, ErrorBody),
    /// An answer signed, but not by the key pinned at sign-in: the service
    /// changed its key (a rotation), or someone else answered. Either way
    /// nothing it says is used, and asking again won't help.
    OtherKey,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Failure::Unreachable(why) => write!(f, "unreachable: {why}"),
            Failure::Refused(status, body) => write!(f, "{status} {}: {}", body.error, body.message),
            Failure::OtherKey => write!(f, "an answer signed by another key than the service's"),
        }
    }
}

/// The service's address as calls use it, such as
/// "https://account.claktory.com": https, or http on this computer only;
/// no path. None when `text` isn't one.
pub(crate) fn service_url(text: &str) -> Option<String> {
    let url = reqwest::Url::parse(text.trim()).ok()?;
    let host = url.host_str()?;
    let local = matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1");
    let fine = (url.scheme() == "https" || (url.scheme() == "http" && local))
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none()
        && url.username().is_empty()
        && url.password().is_none();
    fine.then(|| url.as_str().trim_end_matches('/').to_owned())
}

/// The service a sign-in goes to now.
fn configured_service(config: &AccountConfig) -> String {
    if config.service.is_empty() { DEFAULT_SERVICE.to_owned() } else { service_url(&config.service).unwrap_or_default() }
}

/// An email as the app's screen shows it: its first character, then "…"
/// and its domain ("m…@example.com").
fn masked_email(email: &str) -> String {
    match email.split_once('@') {
        Some((user, domain)) => format!("{}…@{domain}", user.chars().next().map(String::from).unwrap_or_default()),
        None => String::new(),
    }
}

/// How the service is named to the person: "claktory.com", or its host.
fn service_name(service: &str) -> String {
    if service.is_empty() || service == DEFAULT_SERVICE {
        return "claktory.com".to_owned();
    }
    reqwest::Url::parse(service).ok().and_then(|url| url.host_str().map(str::to_owned)).unwrap_or_else(|| service.to_owned())
}

/// This computer's name as the service shows it: the one devices show
/// (`remote::machine_name`), tidied as the service wants it.
fn computer_name() -> String {
    api::clean_name(crate::remote::machine_name())
}

/// This build's platform, as the service counts them: "macos-arm64".
fn platform() -> String {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };
    format!("{}-{arch}", std::env::consts::OS)
}

fn now_s() -> i64 {
    now_ms() / 1000
}

fn hex(bytes: &[u8]) -> String {
    api::hex(bytes)
}

fn refused(code: ErrorCode, message: impl Into<String>) -> RequestError {
    RequestError::Account(code, message.into())
}

/// When to try again, `seconds` after `now`: "in 5 minutes" up to an hour and
/// a half, then "tomorrow" if it's on a later day here, else "in 3 hours".
fn try_again(seconds: i64, now: &jiff::ZonedDateTime) -> String {
    let seconds = seconds.max(1);
    let minutes = (seconds + 59) / 60;
    if minutes <= 90 {
        return if minutes == 1 { "in 1 minute".to_owned() } else { format!("in {minutes} minutes") };
    }
    let later_day = now.checked_add(jiff::SignedDuration::from_secs(seconds)).is_ok_and(|then| then.date() > now.date());
    if later_day {
        return "tomorrow".to_owned();
    }
    format!("in {} hours", (minutes + 59) / 60)
}

/// A sign-in request's failure, in the app's words.
impl Inner {
    /// Signed in, not removed, with a sign-in waiting for its code: Switch
    /// account… (V6), which keeps the account signed in until the code
    /// checks out.
    fn switching(&self) -> bool {
        self.pending.is_some() && self.saved.signed_in() && !self.saved.revoked
    }
}

fn sign_in_error(service: &str, failure: Failure) -> RequestError {
    let name = service_name(service);
    match failure {
        Failure::OtherKey => {
            tracing::warn!(service, "an answer to the sign-in wasn't signed by the service's key");
            refused(ErrorCode::Unreachable, format!("Can't reach {name}. Check your internet connection and try again."))
        }
        Failure::Unreachable(why) => {
            tracing::warn!(service, why, "the account service can't be reached");
            refused(ErrorCode::Unreachable, format!("Can't reach {name}. Check your internet connection and try again."))
        }
        Failure::Refused(_, body) => match body.error.as_str() {
            "invalid_email" => refused(ErrorCode::InvalidRequest, "Enter an email address, like you@example.com."),
            "rate_limited" => {
                let when = try_again(body.retry_after.unwrap_or(60), &jiff::ZonedDateTime::now());
                refused(ErrorCode::RateLimited, format!("Too many codes requested. Try again {when}."))
            }
            "email_unavailable" => refused(ErrorCode::Unreachable, format!("{name} can't send email right now. Try again later.")),
            "wrong_code" => match body.attempts_left.unwrap_or(0) {
                0 => refused(ErrorCode::WrongProof, "Too many tries. Get a new code."),
                1 => refused(ErrorCode::WrongProof, "That code isn't right. 1 try left."),
                left => refused(ErrorCode::WrongProof, format!("That code isn't right. {left} tries left.")),
            },
            "code_expired" => refused(ErrorCode::InvalidRequest, "That code has expired."),
            "internal" => refused(ErrorCode::Unreachable, format!("{name} has a problem right now. Try again later.")),
            _ => refused(ErrorCode::InvalidRequest, body.message),
        },
    }
}

/// Deletes the sign-ins set aside as unreadable (`State::new`): signed in
/// again, they're of no more use, and one may still hold a working token
/// (Fugo).
fn forget_set_aside(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with("account.json.unreadable-")
            && let Err(error) = std::fs::remove_file(entry.path())
        {
            tracing::warn!(%error, "could not delete a sign-in set aside as unreadable");
        }
    }
}

impl Factory {
    /// Checks in with the account service now rather than at its next
    /// time; tests'.
    pub fn check_in_now(&self) {
        self.account.lock().next_check_in = Instant::now();
        self.account.wake.notify_one();
    }
}

impl State {
    /// The account as `<home>/account` keeps it. With signing in on
    /// (`enabled`), the step is "asked", whatever it was; while it's off,
    /// nothing is written.
    pub(crate) fn new(home: &crate::home::Home, enabled: bool) -> State {
        let dir = home.root().join("account");
        let path = dir.join("account.json");
        let read = std::fs::read(&path).map_err(|error| error.to_string()).and_then(|bytes| match bytes.is_empty() {
            true => Err("it's empty".to_owned()),
            false => serde_json::from_slice::<Saved>(&bytes).map_err(|error| error.to_string()),
        });
        // One that's there but can't be read (cut short or garbled by a
        // failing disk: friend bug #38) is never written over: it's set
        // aside, and the person is told to sign in again, which brings back
        // everything the account had.
        let mut kept = true;
        let mut problem = String::new();
        let mut saved = match read {
            Ok(saved) => saved,
            Err(_) if !path.exists() => Saved::default(),
            Err(error) => {
                let aside = dir.join(format!("account.json.unreadable-{}", crate::transcript::now_ms()));
                kept = std::fs::rename(&path, &aside).is_ok();
                tracing::error!(%error, set_aside = kept, "account.json can't be read; signed out until this Claktory signs in again");
                problem = "This Claktory's sign-in couldn't be read after a problem with its disk. Sign in again.".to_owned();
                Saved::default()
            }
        };
        let first = enabled && saved.step != ASKED && kept;
        if first {
            saved.step = ASKED.to_owned();
        }
        let now = Instant::now();
        let inner = Inner {
            saved,
            pending: None,
            problem,
            clock_offset: 0,
            next_unlock: now,
            unlock_tries: 0,
            unlock_stopped: false,
            share_asked: None,
            next_check_in: now,
            endpoints_sent: None,
            reseal_due: None,
            stamped: Default::default(),
            next_sign_out: now,
            removals_heard: None,
            removals_asked: None,
            waiting: Vec::new(),
            waiting_asked: None,
            moves_asked: None,
            moves_soon_until: None,
            moves_turned_down: Vec::new(),
            recovery_making: None,
        };
        let http = reqwest::Client::builder()
            .timeout(CALL_TIMEOUT)
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(format!("Claktory/{}", crate::update::VERSION))
            .build()
            .unwrap_or_default();
        let state = State {
            dir,
            inner: Mutex::new(inner),
            busy: tokio::sync::Mutex::new(()),
            wake: Notify::new(),
            checked: Notify::new(),
            http,
            reports: reports::Sent::default(),
            stopped: std::sync::atomic::AtomicBool::new(false),
        };
        if first {
            state.save();
        }
        state
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn save(&self) {
        if self.stopped.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let saved = self.lock().saved.clone();
        let result = (|| {
            create_private_dir(&self.dir)?;
            let mut json = serde_json::to_string_pretty(&saved).map_err(std::io::Error::other)?;
            json.push('\n');
            crate::remote::write_private(&self.dir.join("account.json"), json.as_bytes())
        })();
        if let Err(error) = result {
            tracing::error!(%error, "could not save account.json");
        }
    }

    /// This Claktory's id and backend key at the service, made the first
    /// time.
    fn identity(&self) -> std::io::Result<(String, SigningKey)> {
        create_private_dir(&self.dir)?;
        let path = self.dir.join("claktory.key");
        let key = match std::fs::read_to_string(&path) {
            Ok(text) => {
                let bytes = Zeroizing::new(unhex(&text).unwrap_or_default());
                let seed: [u8; 32] =
                    bytes.as_slice().try_into().map_err(|_| std::io::Error::other(format!("{} isn't a key", path.display())))?;
                SigningKey::from_bytes(&seed)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let key = api::new_signing_key();
                crate::remote::write_private(&path, format!("{}\n", hex(key.as_bytes())).as_bytes())?;
                key
            }
            Err(error) => return Err(error),
        };
        let id = {
            let mut inner = self.lock();
            if !api::valid_claktory_id(&inner.saved.claktory_id) {
                inner.saved.claktory_id = api::b64(&api::random_32()[..18]);
            }
            inner.saved.claktory_id.clone()
        };
        Ok((id, key))
    }

    /// A new id and backend key: the old id is another account's.
    fn new_identity(&self) {
        self.lock().saved.claktory_id.clear();
        let _ = std::fs::remove_file(self.dir.join("claktory.key"));
        self.save();
    }

    /// A new backend key for the same id, made at the next `identity`: the
    /// old one signed out (#524), and the service retires it.
    fn new_key(&self) {
        if let Err(error) = std::fs::remove_file(self.dir.join("claktory.key"))
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::error!(%error, "could not replace the backend key");
        }
    }

    /// Tells the service of a sign-out: whether that's done (it heard, or
    /// had nothing left to end), or still to do.
    async fn tell_signed_out(&self, out: &SigningOut) -> bool {
        let (Some(service_key), Some(key)) = (api::verifying_key(&out.service_key), out.key()) else {
            tracing::error!(service = out.service, "a sign-out kept without its keys can't be sent; dropped");
            return true;
        };
        let signer = Some((out.machine_token.as_str(), &key));
        let sent = self.send_waiting(&out.service, Some(&service_key), "POST", SIGN_OUT_PATH, b"{}".to_vec(), signer, Some(SIGN_OUT_WAIT));
        match sent.await {
            Ok(_) => {
                tracing::info!(service = out.service, "the account service heard of the sign-out");
                true
            }
            // Its token no longer works: signed out there already (the
            // answer was lost), or signed in again since. Nothing to end.
            Err(Failure::Refused(401, body)) if body.error == "unauthorized" => true,
            Err(Failure::Refused(403, body)) if body.error == "revoked" => true,
            Err(failure) => {
                tracing::info!(service = out.service, %failure, "couldn't tell the account service of the sign-out yet");
                false
            }
        }
    }

    /// L, the vault's share kept here, made the first time.
    fn local_share(&self) -> std::io::Result<Zeroizing<[u8; 32]>> {
        create_private_dir(&self.dir)?;
        let path = self.dir.join("share.key");
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let bytes = Zeroizing::new(unhex(&text).unwrap_or_default());
                let share: [u8; 32] =
                    bytes.as_slice().try_into().map_err(|_| std::io::Error::other(format!("{} isn't a share", path.display())))?;
                Ok(Zeroizing::new(share))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let share = api::random_32();
                crate::remote::write_private(&path, Zeroizing::new(format!("{}\n", hex(share.as_slice()))).as_bytes())?;
                Ok(share)
            }
            Err(error) => Err(error),
        }
    }

    /// Sends a request and checks the answer is the service's: signed with
    /// `service_key` (None: the key the answer itself holds, for the
    /// well-known one). `signer`: a signed request, with the machine token
    /// (none when it's empty) and backend key; signed again with the service's time after
    /// `clock_skew`, or a second later after `replayed`.
    async fn send(
        &self,
        service: &str,
        service_key: Option<&VerifyingKey>,
        method: &str,
        path: &str,
        body: Vec<u8>,
        signer: Option<(&str, &SigningKey)>,
    ) -> Result<Vec<u8>, Failure> {
        self.send_waiting(service, service_key, method, path, body, signer, None).await
    }

    /// `send`, allowed `wait` instead of the usual timeout: a long poll
    /// (the apps' DM events), which the service holds for up to 25 seconds.
    #[allow(clippy::too_many_arguments)]
    async fn send_waiting(
        &self,
        service: &str,
        service_key: Option<&VerifyingKey>,
        method: &str,
        path: &str,
        body: Vec<u8>,
        signer: Option<(&str, &SigningKey)>,
        wait: Option<Duration>,
    ) -> Result<Vec<u8>, Failure> {
        let unreachable = |why: String| Failure::Unreachable(why);
        for attempt in 0..3 {
            let mut request =
                self.http.request(method.parse().map_err(|_| unreachable(format!("bad method {method}")))?, format!("{service}{path}"));
            if let Some(wait) = wait {
                request = request.timeout(wait);
            }
            if !body.is_empty() {
                request = request.header(reqwest::header::CONTENT_TYPE, "application/json").body(body.clone());
            }
            if let Some((token, key)) = signer {
                let time = {
                    let mut inner = self.lock();
                    let now = now_s() + inner.clock_offset;
                    stamp(&mut inner.stamped, now, method, path, &body)
                };
                if !token.is_empty() {
                    request = request.bearer_auth(token);
                }
                request = request
                    .header(api::TIME_HEADER, time.to_string())
                    .header(api::SIGNATURE_HEADER, api::sign_request(key, method, path, time, &body));
            }
            let response = request.send().await.map_err(|error| unreachable(error.to_string()))?;
            let status = response.status().as_u16();
            let signature = response.headers().get(api::SIGNATURE_HEADER).and_then(|value| value.to_str().ok()).map(str::to_owned);
            let answer = response.bytes().await.map_err(|error| unreachable(error.to_string()))?.to_vec();
            let key = match service_key {
                Some(key) => *key,
                None => serde_json::from_slice::<WellKnown>(&answer)
                    .ok()
                    .and_then(|known| api::verifying_key(&known.service_key))
                    .ok_or_else(|| unreachable(format!("{status}: not an account service")))?,
            };
            let Some(signature) = signature else {
                return Err(unreachable(format!("{status}: an answer not signed by the service's key")));
            };
            if !api::verify_response(&key, status, method, path, &body, &answer, &signature) {
                return Err(if service_key.is_some() {
                    Failure::OtherKey
                } else {
                    unreachable(format!("{status}: an answer not signed by the service's key"))
                });
            }
            self.lock().saved.last_contact_ms = now_ms();
            // 204: done, with nothing to say (revoking an app grant).
            if status == 200 || status == 204 {
                return Ok(answer);
            }
            let error: ErrorBody = serde_json::from_slice(&answer).map_err(|_| unreachable(format!("{status}: an unreadable error")))?;
            if signer.is_some() && status == 401 && attempt < 2 {
                if error.error == "clock_skew"
                    && let Some(time) = error.server_time
                {
                    self.lock().clock_offset = time - now_s();
                    continue;
                }
                if error.error == "replayed" {
                    tokio::time::sleep(Duration::from_millis(1100)).await;
                    continue;
                }
            }
            return Err(Failure::Refused(status, error));
        }
        Err(unreachable("refused again and again".to_owned()))
    }

    async fn call<T: DeserializeOwned>(
        &self,
        service: &str,
        service_key: Option<&VerifyingKey>,
        method: &str,
        path: &str,
        body: &impl Serialize,
        signer: Option<(&str, &SigningKey)>,
    ) -> Result<T, Failure> {
        let body = serde_json::to_vec(body).map_err(|error| Failure::Unreachable(error.to_string()))?;
        let answer = self.send(service, service_key, method, path, body, signer).await?;
        serde_json::from_slice(&answer).map_err(|error| Failure::Unreachable(format!("an unreadable answer: {error}")))
    }

    /// The service's key, from its well-known address.
    async fn well_known(&self, service: &str) -> Result<WellKnown, Failure> {
        let answer = self.send(service, None, "GET", api::WELL_KNOWN, Vec::new(), None).await?;
        let known: WellKnown = serde_json::from_slice(&answer).map_err(|error| Failure::Unreachable(error.to_string()))?;
        if known.service != "claktory-account" || known.api != api::API {
            return Err(Failure::Unreachable(format!("{} API {}, not claktory-account API {}", known.service, known.api, api::API)));
        }
        Ok(known)
    }

    /// A signed call as the signed-in Claktory.
    async fn signed<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> Result<T, Failure> {
        self.signed_with("POST", path, body).await
    }

    /// `signed`, with the method given.
    async fn signed_with<T: DeserializeOwned>(&self, method: &str, path: &str, body: &impl Serialize) -> Result<T, Failure> {
        let (service, service_key, token) = {
            let inner = self.lock();
            (inner.saved.service.clone(), inner.saved.service_key.clone(), inner.saved.machine_token.clone())
        };
        let key = api::verifying_key(&service_key).ok_or_else(|| Failure::Unreachable("no service key kept".to_owned()))?;
        let (_, backend_key) = self.identity().map_err(|error| Failure::Unreachable(error.to_string()))?;
        self.call(&service, Some(&key), method, path, body, Some((&token, &backend_key))).await
    }

    /// A signed call with a token and backend key of its choosing.
    async fn signed_as<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize, token: &str, key: &SigningKey) -> Result<T, Failure> {
        let (service, service_key) = {
            let inner = self.lock();
            (inner.saved.service.clone(), inner.saved.service_key.clone())
        };
        let service_key = api::verifying_key(&service_key).ok_or_else(|| Failure::Unreachable("no service key kept".to_owned()))?;
        self.call(&service, Some(&service_key), "POST", path, body, Some((token, key))).await
    }

    /// The share S at `epoch` (None: the current one), opened.
    async fn share(&self, epoch: Option<u64>) -> Result<(u64, Zeroizing<[u8; 32]>), Failure> {
        let ephemeral = Ephemeral::new();
        let asked = self.signed::<Share>("/v1/share", &ShareRequest { eph_pub: ephemeral.public_b64(), epoch }).await;
        if !matches!(asked, Err(Failure::Unreachable(_))) {
            self.lock().share_asked = Some(Instant::now());
        }
        let share = asked?;
        if epoch.is_some_and(|epoch| epoch != share.epoch) {
            return Err(Failure::Unreachable(format!("asked for epoch {epoch:?}, got {}", share.epoch)));
        }
        let claktory_id = self.lock().saved.claktory_id.clone();
        let opened = ephemeral
            .open(&share.sealed_share, &claktory_id, share.epoch)
            .ok_or_else(|| Failure::Unreachable("a share that doesn't open".to_owned()))?;
        Ok((share.epoch, opened))
    }

    /// The account it's signed in with, while signing in is on: its id,
    /// email and service.
    /// This computer's app vouched for (#619): `account_key` from now on,
    /// under the sign-in that holds now.
    pub(crate) fn note_vouched_app(&self, account_key: &str) {
        {
            let mut inner = self.lock();
            let saved = &mut inner.saved;
            saved.vouched_app =
                Some(VouchedApp { account_id: saved.account_id.clone(), epoch: saved.epoch, account_key: account_key.to_owned() });
        }
        self.save();
    }

    /// Whether `account_key` is the one this Claktory vouched for its own
    /// app (#619), under the sign-in that holds now: not after a sign-out,
    /// another sign-in, another account or a removal.
    pub(crate) fn is_vouched_app(&self, account_key: &str) -> bool {
        let inner = self.lock();
        let saved = &inner.saved;
        saved.signed_in()
            && !saved.revoked
            && !account_key.is_empty()
            && saved.vouched_app.as_ref().is_some_and(|vouched| {
                vouched.account_id == saved.account_id && vouched.epoch == saved.epoch && vouched.account_key == account_key
            })
    }

    pub(crate) fn signed_in_as(&self, config: &AccountConfig) -> Option<(String, String, String)> {
        let inner = self.lock();
        let saved = &inner.saved;
        (config.enabled && saved.signed_in() && !saved.revoked)
            .then(|| (saved.account_id.clone(), saved.email.clone(), saved.service.clone()))
    }

    /// Where a device this Claktory recognizes signs in when it's turned
    /// away to (Error.account_service, account_email; Sale's row 189, Fugo
    /// 47717): the service, empty for claktory.com, and the email signed in
    /// with, masked as the app shows it ("m…@example.com"), empty when not
    /// signed in.
    pub(crate) fn where_to_sign_in(&self, config: &AccountConfig) -> (String, String) {
        if !config.enabled {
            return (String::new(), String::new());
        }
        let inner = self.lock();
        let saved = &inner.saved;
        let signed_in = saved.signed_in() && !saved.revoked;
        let service = if signed_in { saved.service.clone() } else { configured_service(config) };
        let service = if service == DEFAULT_SERVICE { String::new() } else { service };
        (service, if signed_in { masked_email(&saved.email) } else { String::new() })
    }

    /// This Claktory's registration while it's signed in and not removed:
    /// the account, its Claktory id and the registration's epoch, which a
    /// sign-in again or a move changes (its direct-message leaf goes with it).
    pub(crate) fn registration(&self, config: &AccountConfig) -> Option<(String, String, u64)> {
        let inner = self.lock();
        let saved = &inner.saved;
        (config.enabled && saved.signed_in() && !saved.revoked).then(|| (saved.account_id.clone(), saved.claktory_id.clone(), saved.epoch))
    }

    /// What a pipe peer's statement says, checked with the service key this
    /// Claktory pinned when it signed in (never the key the statement
    /// names), for the pipe's pinned peer key `endpoint_id`, at `now` (unix
    /// seconds). None when it doesn't verify, or when this Claktory isn't
    /// signed in and so can check nothing.
    pub(crate) fn verify_pipe_peer(&self, statement: &str, endpoint_id: &str, now: i64) -> Option<api::PipePeerStatement> {
        let service_key = {
            let inner = self.lock();
            let saved = &inner.saved;
            if !saved.signed_in() || saved.revoked {
                return None;
            }
            api::verifying_key(&saved.service_key)?
        };
        api::verify_pipe_peer(&service_key, statement, endpoint_id, now)
    }

    /// What the app shows: None while signing in is off.
    pub(crate) fn view(&self, config: &AccountConfig) -> Option<v1::ClaktoryAccount> {
        if !config.enabled {
            return None;
        }
        let inner = self.lock();
        let saved = &inner.saved;
        // Switch account… (V6): signed in, with a code asked for another
        // address, it shows as signed in, with where the code went.
        let switching = inner.switching();
        let state = if inner.pending.as_ref().is_some_and(|pending| pending.approval.is_some()) {
            v1::ClaktoryAccountState::AwaitingApproval
        } else if inner.pending.is_some() && !switching {
            v1::ClaktoryAccountState::AwaitingCode
        } else if saved.signed_in() && saved.revoked {
            v1::ClaktoryAccountState::Revoked
        } else if saved.signed_in() {
            v1::ClaktoryAccountState::SignedIn
        } else {
            v1::ClaktoryAccountState::SignedOut
        };
        let (email, service) = match inner.pending.as_ref().filter(|_| !switching) {
            Some(pending) => (pending.start.email.clone(), pending.service.clone()),
            None if saved.signed_in() => (saved.email.clone(), saved.service.clone()),
            None => (String::new(), configured_service(config)),
        };
        // Cleaned as read: one kept before names were cleaned may hide characters.
        let name = if saved.name.is_empty() { computer_name() } else { api::clean_name(&saved.name) };
        let switching_to = inner.pending.as_ref().filter(|_| switching).map(|pending| pending.start.email.clone()).unwrap_or_default();
        #[cfg(feature = "qa-lies")]
        let (email, switching_to) =
            (crate::qa_lies::email(email), if switching_to.is_empty() { switching_to } else { crate::qa_lies::email(switching_to) });
        Some(v1::ClaktoryAccount {
            state: state.into(),
            email,
            service,
            name,
            last_contact_ms: saved.last_contact_ms,
            problem: match inner.saved.signing_out.first() {
                Some(out) if inner.problem.is_empty() && !saved.signed_in() && inner.pending.is_none() => {
                    format!("Signed out. Claktory tells {} as soon as it can reach it.", service_name(&out.service))
                }
                _ => inner.problem.clone(),
            },
            code_expires_ms: inner.pending.as_ref().filter(|pending| pending.approval.is_none()).map_or(0, |pending| pending.expires_ms),
            last_sign_in_json: saved.last_sign_in_json.clone(),
            last_check_in_json: saved.last_check_in_json.clone(),
            last_sign_in_ms: saved.last_sign_in_ms,
            last_check_in_ms: saved.last_check_in_ms,
            // Every Claktory must sign in (#195): only an unreachable
            // service lets the app go on without it, for now.
            skippable: false,
            code_from: inner.pending.as_ref().map(|pending| pending.code_from.clone()).unwrap_or_default(),
            switching_to,
            approval_expires_ms: inner
                .pending
                .as_ref()
                .filter(|pending| pending.approval.is_some())
                .map_or(0, |pending| pending.expires_ms),
            authorizations_waiting: if saved.signed_in() && !saved.revoked { inner.waiting.clone() } else { Vec::new() },
        })
    }

    /// The vault's account protector's params, if it has one.
    fn params(factory: &Factory) -> Option<Params> {
        serde_json::from_str(&factory.vault.account_params()?).ok()
    }

    fn set_problem(&self, problem: impl Into<String>) {
        self.lock().problem = problem.into();
    }

    fn offline(&self) -> String {
        let service = self.lock().saved.service.clone();
        format!("Can't reach {}. Claktory keeps working and tries again on its own.", service_name(&service))
    }
}

/// The account this Claktory is signed in to, as devices are checked
/// against it (onboarding slice 6).
#[derive(Clone, Debug)]
pub(crate) struct Owner {
    pub account_id: String,
    /// The service's key, pinned at sign-in: device assertions are checked
    /// with it.
    pub service_key: VerifyingKey,
    /// When it signed in, in ms since 1970; 0 when that's unknown (a
    /// sign-in from before it was kept).
    pub signed_in_ms: i64,
    /// The pinned key is from before this build's floor (`anchor`): it no
    /// longer vouches for devices.
    pub retired: bool,
    /// Signed in with no code: the account key of the device that
    /// approved it, the only one let in at once; empty otherwise.
    pub first_device: String,
}

/// The account it's signed in to, while signing in is on (`config`, so it
/// can be called with the Factory locked) and it's signed in and not
/// removed at the service.
pub(crate) fn owner(factory: &Factory, config: &AccountConfig) -> Option<Owner> {
    if !config.enabled {
        return None;
    }
    let inner = factory.account.lock();
    let saved = &inner.saved;
    if !saved.signed_in() || saved.revoked {
        return None;
    }
    Some(Owner {
        account_id: saved.account_id.clone(),
        service_key: api::verifying_key(&saved.service_key)?,
        signed_in_ms: saved.signed_in_ms,
        retired: anchor::retired(&saved.service, saved.service_key_from),
        first_device: saved.first_device.clone(),
    })
}

/// Why a device assertion doesn't let a device in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AssertionRefusal {
    /// This Claktory isn't signed in (or signing in is off, or it was
    /// removed at the service): it can't check one.
    NotSignedIn,
    /// Not the service's, or not one this version reads.
    Invalid,
    /// The service's, but no longer working: the device gets a fresh one.
    Expired,
    /// For another account than this Claktory's.
    OtherAccount,
    /// For another tunnel key than the one the device connected with.
    OtherKey,
}

/// What a device's assertion says, if it lets the device with tunnel key
/// `endpoint_id` in as this Claktory's owner: the service's, working now,
/// for this Claktory's account and that key.
pub(crate) fn check_assertion(
    factory: &Factory,
    config: &AccountConfig,
    assertion: &str,
    endpoint_id: &str,
) -> Result<api::DeviceAssertion, AssertionRefusal> {
    let owner = owner(factory, config).ok_or(AssertionRefusal::NotSignedIn)?;
    if owner.retired {
        return Err(AssertionRefusal::Invalid);
    }
    let said = api::read_device_assertion(&owner.service_key, assertion).ok_or(AssertionRefusal::Invalid)?;
    if api::verify_device_assertion(&owner.service_key, assertion, now_s()).is_none() {
        return Err(AssertionRefusal::Expired);
    }
    if said.account_id != owner.account_id {
        return Err(AssertionRefusal::OtherAccount);
    }
    if !said.device_key.eq_ignore_ascii_case(endpoint_id) {
        return Err(AssertionRefusal::OtherKey);
    }
    Ok(said)
}

/// The account key of the assertion this computer's own app showed on its
/// socket (E11-8 point 6), for its DMs: the service's, unexpired, for this
/// Claktory's account. Its device key isn't compared: the socket has no
/// tunnel key, and the app is known there by its code signature. None
/// without one that checks out.
pub(crate) fn app_account_key(factory: &Factory, assertion: &str) -> Option<String> {
    app_account(factory, assertion).map(|(account_key, _)| account_key)
}

/// `app_account_key`, with the account it was proven for: (account key,
/// account id).
pub(crate) fn app_account(factory: &Factory, assertion: &str) -> Option<(String, String)> {
    if assertion.is_empty() {
        return None;
    }
    let config = factory.lock().config.account.clone();
    let owner = owner(factory, &config)?;
    if owner.retired {
        return None;
    }
    let said = api::verify_device_assertion(&owner.service_key, assertion, now_s())?;
    (said.account_id == owner.account_id && !said.account_key.is_empty()).then_some((said.account_key, said.account_id))
}

#[cfg(unix)]
fn create_private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
}

/// 0700 on Windows: this user and SYSTEM only (`windows::private`).
#[cfg(windows)]
fn create_private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    factory_agent::windows::private::create_dir_all(dir)
}

/// The account settings, if signing in is on.
fn enabled(factory: &Factory) -> Result<AccountConfig, RequestError> {
    let config = factory.lock().config.account.clone();
    if !config.enabled {
        return Err(refused(ErrorCode::InvalidRequest, "Signing in to claktory.com isn't on in this Claktory."));
    }
    Ok(config)
}

fn changed(factory: &Factory) {
    factory.broadcast_workspace(&factory.lock());
    // Devices let in through the account it no longer has are turned away
    // at once (row 713), not at the tunnel's next look.
    factory.account_open();
}

/// AccountSignIn: emails a code. Signing in again while signed in (another
/// email, or after a removal) replaces the sign-in once the code is given.
pub(crate) async fn sign_in(factory: &Arc<Factory>, email: &str) -> Result<(), RequestError> {
    let config = enabled(factory)?;
    let Some(email) = api::normalize_email(email) else {
        return Err(refused(ErrorCode::InvalidRequest, "Enter an email address, like you@example.com."));
    };
    let account = &factory.account;
    let _busy = account.busy.lock().await;
    let (service, service_key, key_from, verifying) = service_for_sign_in(factory, &config).await?;
    let (claktory_id, key) = account.identity()?;
    let start = SignInStart {
        email,
        claktory_id,
        name: computer_name(),
        backend_key: api::b64(key.verifying_key().to_bytes()),
        version: crate::update::VERSION.to_owned(),
        platform: platform(),
    };
    send_code(factory, &service, &service_key, key_from, &verifying, start).await
}

/// The service to sign in at, as `config` says, with its key checked
/// (`anchor`): its address and key, the key's `from`, and the key to check
/// its answers with. A sign-out there it hasn't heard of yet is told first.
async fn service_for_sign_in(
    factory: &Arc<Factory>,
    config: &AccountConfig,
) -> Result<(String, String, Option<i64>, VerifyingKey), RequestError> {
    let account = &factory.account;
    let service = configured_service(config);
    let known = account.well_known(&service).await.map_err(|failure| sign_in_error(&service, failure))?;
    let key_from = match anchor::vouch(&service, &known) {
        anchor::Vouch::Free(from) => from,
        anchor::Vouch::Vouched(from) => Some(from),
        anchor::Vouch::Refused(why) => {
            tracing::warn!(service, why, "the account service's key isn't vouched for; not signing in");
            let unverified = unverified_key(&service_name(&service));
            return Err(refused(ErrorCode::Unreachable, format!("{unverified}, so you can't sign in right now. Try again later.")));
        }
    };
    let service_key = known.service_key;
    let Some(verifying) = api::verifying_key(&service_key) else {
        return Err(sign_in_error(&service, Failure::Unreachable("a bad service key".to_owned())));
    };
    // A sign-out there not told yet goes first, so the sign-in it ended
    // ends before this one starts.
    if account.lock().saved.signing_out.iter().any(|out| out.service == service) {
        tell_sign_outs(factory, true).await;
    }
    Ok((service, service_key, key_from, verifying))
}

/// Sends `start`, and waits for its code.
async fn send_code(
    factory: &Arc<Factory>,
    service: &str,
    service_key: &str,
    key_from: Option<i64>,
    verifying: &VerifyingKey,
    mut start: SignInStart,
) -> Result<(), RequestError> {
    let account = &factory.account;
    let mut asked = account.call(service, Some(verifying), "POST", "/v1/sign-in/start", &start, None).await;
    // A backend key this Claktory signed out with (#524), back from a
    // backup, say: the service won't take it again, so a new one; never
    // while it's signed in with it (Switch account…, V6), which a new key
    // would end before the new code checks out.
    let switching = {
        let inner = account.lock();
        inner.saved.signed_in() && !inner.saved.revoked
    };
    if !switching && matches!(&asked, Err(Failure::Refused(_, body)) if body.error == "backend_key_retired") {
        tracing::info!(service, "the account service retired this backend key; signing in with a new one");
        account.new_key();
        let (_, key) = account.identity()?;
        start.backend_key = api::b64(key.verifying_key().to_bytes());
        asked = account.call(service, Some(verifying), "POST", "/v1/sign-in/start", &start, None).await;
    }
    let started: SignInStarted = asked.map_err(|failure| sign_in_error(service, failure))?;
    {
        let mut inner = account.lock();
        inner.saved.last_sign_in_json = serde_json::to_string_pretty(&start).unwrap_or_default();
        inner.saved.last_sign_in_ms = now_ms();
        inner.pending = Some(Pending {
            service: service.to_owned(),
            service_key: service_key.to_owned(),
            key_from,
            start,
            request_id: started.request_id,
            expires_ms: now_ms() + started.expires_in * 1000,
            code_from: match started.code_from {
                from if from.is_empty() && service == DEFAULT_SERVICE => DEFAULT_CODE_FROM.to_owned(),
                from => from,
            },
            approval: None,
        });
    }
    account.save();
    tracing::info!(service, "sign-in code sent");
    changed(factory);
    Ok(())
}

/// AccountResend: a new code to the same address.
pub(crate) async fn resend(factory: &Arc<Factory>) -> Result<(), RequestError> {
    enabled(factory)?;
    let account = &factory.account;
    let _busy = account.busy.lock().await;
    let Some(pending) = account.lock().pending.clone() else {
        return Err(refused(ErrorCode::InvalidRequest, "Enter your email address first."));
    };
    let Some(verifying) = api::verifying_key(&pending.service_key) else {
        return Err(refused(ErrorCode::Internal, "The service's key is unreadable."));
    };
    send_code(factory, &pending.service, &pending.service_key, pending.key_from, &verifying, pending.start).await
}

/// AccountSignInCancel (V6): drops the sign-in waiting for its code; the
/// account signed in, if any, stays exactly as it was. Nothing waiting:
/// nothing changes.
pub(crate) async fn cancel_sign_in(factory: &Arc<Factory>) -> Result<(), RequestError> {
    enabled(factory)?;
    let account = &factory.account;
    let _busy = account.busy.lock().await;
    if account.lock().pending.take().is_some() {
        tracing::info!("a sign-in waiting for its code was cancelled");
        changed(factory);
    }
    Ok(())
}

/// What a sign-in waiting for the person's approval says to a code.
const WAITING_FOR_APPROVAL: &str = "This Claktory is waiting for you to approve it on one of your devices.";
/// A computer's request to join waits this long at most (the service's own
/// `expires_in` is what counts; this only bounds a wrong one).
const APPROVAL_LONGEST: i64 = 15 * 60;

/// AccountAuthorize (the no-code setup): asks the account's devices to
/// approve this Claktory, with no code anywhere, then waits for their answer
/// (`poll_approval`). Asking again for the same email while it waits changes
/// nothing: another request would cancel both.
pub(crate) async fn authorize(factory: &Arc<Factory>, email: &str) -> Result<(), RequestError> {
    let config = enabled(factory)?;
    let Some(email) = api::normalize_email(email) else {
        return Err(refused(ErrorCode::InvalidRequest, "Enter an email address, like you@example.com."));
    };
    let account = &factory.account;
    let _busy = account.busy.lock().await;
    if account
        .lock()
        .pending
        .as_ref()
        .is_some_and(|pending| pending.approval.is_some() && pending.start.email == email && pending.expires_ms > now_ms())
    {
        return Ok(());
    }
    let (service, service_key, key_from, verifying) = service_for_sign_in(factory, &config).await?;
    let (claktory_id, key) = account.identity()?;
    let mut start = AuthorizeStart {
        email,
        claktory_id,
        name: computer_name(),
        backend_key: api::b64(key.verifying_key().to_bytes()),
        version: crate::update::VERSION.to_owned(),
        platform: platform(),
        kind: api::KIND_CLAKTORY.to_owned(),
        receive: None,
    };
    let path = "/v1/claktories/authorize/start";
    let mut asked = account.call::<AuthorizeStarted>(&service, Some(&verifying), "POST", path, &start, Some(("", &key))).await;
    // A backend key this Claktory signed out with: a new one.
    if matches!(&asked, Err(Failure::Refused(_, body)) if body.error == "backend_key_retired") {
        tracing::info!(service, "the account service retired this backend key; asking with a new one");
        account.new_key();
        let (_, key) = account.identity()?;
        start.backend_key = api::b64(key.verifying_key().to_bytes());
        asked = account.call(&service, Some(&verifying), "POST", path, &start, Some(("", &key))).await;
    }
    let started = match asked {
        Ok(started) => started,
        Err(Failure::Refused(_, body)) if body.error == "no_device_to_approve" => {
            return Err(refused(ErrorCode::InvalidRequest, NO_DEVICE_TO_APPROVE));
        }
        Err(Failure::Refused(_, body)) if body.error == "two_at_once" => return Err(refused(ErrorCode::InvalidRequest, TWO_AT_ONCE)),
        Err(failure) => return Err(sign_in_error(&service, failure)),
    };
    let signed = SignInStart {
        email: start.email.clone(),
        claktory_id: start.claktory_id.clone(),
        name: start.name.clone(),
        backend_key: start.backend_key.clone(),
        version: start.version.clone(),
        platform: start.platform.clone(),
    };
    {
        let mut inner = account.lock();
        inner.saved.last_sign_in_json = serde_json::to_string_pretty(&start).unwrap_or_default();
        inner.saved.last_sign_in_ms = now_ms();
        inner.problem.clear();
        inner.pending = Some(Pending {
            service: service.clone(),
            service_key,
            key_from,
            start: signed,
            request_id: started.request_id,
            expires_ms: now_ms() + started.expires_in.clamp(1, APPROVAL_LONGEST) * 1000,
            code_from: String::new(),
            approval: Some(Approval::new(started.interval)),
        });
    }
    account.save();
    tracing::info!(service, "asked the account's devices to approve this Claktory");
    account.wake.notify_one();
    changed(factory);
    Ok(())
}

const NO_DEVICE_TO_APPROVE: &str = "None of your devices can approve this Claktory yet. On your phone or Mac, sign in to your Claktory account with your email, connect it to one of your Claktories, then ask again.";
const TWO_AT_ONCE: &str = "Two computers asked to join your account at the same time, so neither was added. Ask again.";

/// Asks the service whether the person approved this Claktory, if a request
/// waits and it's time: signed in when they did (`complete`), and the device
/// that approved is the one let in at once; told why when they declined, it
/// ran out or another computer asked too. Called with `busy` held.
async fn poll_approval(factory: &Arc<Factory>) {
    let account = &factory.account;
    let Some(pending) = account
        .lock()
        .pending
        .clone()
        .filter(|pending| pending.approval.as_ref().is_some_and(|approval| approval.next_poll <= Instant::now()))
    else {
        return;
    };
    let stop = |problem: &str| {
        let mut inner = account.lock();
        inner.pending = None;
        inner.problem = problem.to_owned();
    };
    if now_ms() >= pending.expires_ms {
        stop(NOT_APPROVED_IN_TIME);
        changed(factory);
        return;
    }
    let (Some(verifying), Ok((_, key))) = (api::verifying_key(&pending.service_key), account.identity()) else {
        tracing::warn!("couldn't read this Claktory's keys to finish signing in");
        stop("Claktory couldn't finish signing in. Ask again.");
        changed(factory);
        return;
    };
    let ephemeral = Ephemeral::new();
    let asked = AuthorizePoll { request_id: pending.request_id.clone(), eph_pub: ephemeral.public_b64() };
    let polled = account
        .call::<Authorized>(&pending.service, Some(&verifying), "POST", "/v1/claktories/authorize/poll", &asked, Some(("", &key)))
        .await;
    let again = |account: &State| {
        if let Some(approval) = account.lock().pending.as_mut().and_then(|pending| pending.approval.as_mut()) {
            approval.next_poll = Instant::now() + approval.interval;
        }
    };
    match polled {
        Ok(authorized) => match complete(factory, &pending, &authorized.signed_in, &ephemeral, &authorized.first_device).await {
            Ok(_) => tracing::info!(service = pending.service, "signed in to the account service, approved on a device"),
            Err(error) => {
                tracing::warn!(%error, "an approved sign-in couldn't be finished");
                stop(&error.to_string());
            }
        },
        Err(Failure::Refused(_, body)) if body.error == "authorization_pending" => again(account),
        Err(Failure::Refused(_, body)) if body.error == "rate_limited" => again(account),
        Err(Failure::Refused(_, body)) => {
            tracing::info!(error = body.error, "the request to join the account ended");
            let problem = match body.error.as_str() {
                "access_denied" => "This Claktory was declined on one of your devices. If that was a mistake, ask again.".to_owned(),
                "two_at_once" => TWO_AT_ONCE.to_owned(),
                "expired_token" => NOT_APPROVED_IN_TIME.to_owned(),
                "claktory_taken" => {
                    account.new_identity();
                    "This Claktory was signed in to another account before. Ask again to use this one.".to_owned()
                }
                "backend_key_retired" => {
                    account.new_key();
                    "That request no longer works. Ask again.".to_owned()
                }
                _ => sign_in_error(&pending.service, Failure::Refused(409, body)).to_string(),
            };
            stop(&problem);
        }
        // Not reached, or not the service: asked again until it runs out.
        Err(failure) => {
            tracing::info!(%failure, "couldn't ask whether this Claktory was approved; asking again");
            again(account);
        }
    }
    account.save();
    changed(factory);
}

const NOT_APPROVED_IN_TIME: &str = "This Claktory wasn't approved in time. Ask again.";

/// How often the bell (`Inner.waiting`) is asked for while a device is
/// connected; check-ins bring it too.
const BELL_EVERY: Duration = Duration::from_secs(60);

/// Asks the service which computers wait for the person's approval, while
/// this Claktory is signed in and an app is connected to hear the bell.
/// Called with `busy` held.
async fn ring_bell(factory: &Arc<Factory>) {
    let account = &factory.account;
    let (service, service_key, token) = {
        let mut inner = account.lock();
        if !inner.saved.signed_in() || inner.saved.revoked || inner.waiting_asked.is_some_and(|asked| asked.elapsed() < BELL_EVERY) {
            return;
        }
        if factory.clients() == 0 {
            // Nobody to ring: a check-in brings it later.
            return;
        }
        inner.waiting_asked = Some(Instant::now());
        (inner.saved.service.clone(), inner.saved.service_key.clone(), inner.saved.machine_token.clone())
    };
    let (Some(verifying), Ok((_, key))) = (api::verifying_key(&service_key), account.identity()) else { return };
    let path = "/v1/claktories/authorizations";
    let answer = account.send(&service, Some(&verifying), "GET", path, Vec::new(), Some((&token, &key))).await;
    match answer
        .and_then(|bytes| serde_json::from_slice::<ClaktoryAuthorizations>(&bytes).map_err(|error| Failure::Unreachable(error.to_string())))
    {
        Ok(heard) => heard_waiting(factory, heard.waiting),
        Err(failure) => tracing::debug!(%failure, "couldn't ask which computers wait for approval"),
    }
}

/// The computers waiting for the person's approval, as the service said:
/// told to every device when it changed.
fn heard_waiting(factory: &Factory, mut waiting: Vec<String>) {
    waiting.retain(|id| !id.is_empty() && id.len() <= 200);
    waiting.truncate(10);
    let changed_now = {
        let mut inner = factory.account.lock();
        let changed_now = inner.waiting != waiting;
        inner.waiting = waiting;
        changed_now
    };
    if changed_now {
        changed(factory);
    }
}

/// Tests only: asks the service for the bell at once, rather than within
/// `BELL_EVERY`.
#[cfg(feature = "test-hooks")]
pub fn ring_bell_for_tests(factory: &Factory) {
    factory.account.lock().waiting_asked = None;
    factory.account.wake.notify_one();
}

/// Tests only: how many apps are connected.
#[cfg(feature = "test-hooks")]
pub fn apps_connected_for_tests(factory: &Factory) -> usize {
    factory.clients()
}

/// Tests only: this Claktory's backend key, public half.
#[cfg(feature = "test-hooks")]
pub fn backend_key_for_tests(factory: &Factory) -> VerifyingKey {
    factory.account.identity().expect("the backend key").1.verifying_key()
}

/// Why a connection whose account key is `account_key` can't ask for this
/// Claktory's word (ProveAllowedDevice), as Welcome.cannot_approve says it
/// and the refusal does: empty when it can. A device on another machine
/// (`remote`) that connected with a pairing code before it showed the
/// account has none until it connects again with its assertion, which
/// admission keeps; this computer's own app has one when it's signed in.
pub(crate) fn cannot_approve(factory: &Factory, account_key: Option<&str>, remote: bool) -> String {
    if account_key.is_some_and(|key| !key.is_empty()) {
        return String::new();
    }
    if remote {
        let name = factory.identity().name;
        format!(
            "This device connected to {name} with a code, not with your Claktory account. To approve here, sign in to your account on this device and connect again."
        )
    } else {
        "Sign in to your Claktory account on this device first.".to_owned()
    }
}

/// ProveAllowedDevice: this Claktory's word, signed with its backend key,
/// that the device whose account key is `account_key` (the one its
/// connection proved) is allowed on it, for approving request `request_id`
/// at the service. Only for a connection of the person's, which the caller
/// checks; working for 120 seconds by the service's clock.
pub(crate) fn allowed_device_word(factory: &Factory, account_key: Option<&str>, remote: bool, request_id: &str) -> Result<String, RequestError> {
    enabled(factory)?;
    let Some(account_key) = account_key.filter(|key| !key.is_empty()) else {
        return Err(refused(ErrorCode::SignInRequired, cannot_approve(factory, None, remote)));
    };
    if request_id.is_empty() || request_id.len() > 200 || request_id.chars().any(|character| character.is_control()) {
        return Err(refused(ErrorCode::InvalidRequest, "That isn't a request to approve."));
    }
    let account = &factory.account;
    let (account_id, claktory_id, offset) = {
        let inner = account.lock();
        if !inner.saved.signed_in() || inner.saved.revoked {
            return Err(refused(ErrorCode::SignInRequired, "This Claktory isn't signed in to your account."));
        }
        (inner.saved.account_id.clone(), inner.saved.claktory_id.clone(), inner.clock_offset)
    };
    let (_, key) = account.identity()?;
    let allowed = api::AllowedDevice {
        v: 1,
        account_id,
        claktory_id,
        account_key: account_key.to_owned(),
        request_id: request_id.to_owned(),
        iat: now_s() + offset,
    };
    Ok(api::sign_allowed_device(&key, &allowed))
}

/// Whether this Claktory has never been signed in to an account, by what it
/// keeps (not by whether it's signed in now): no sign-in time kept (a
/// sign-out keeps it), no check-in ever made, and no sign-out still to tell
/// a service of. The Windows app's token signs in only then
/// (`authority::Class::LocalBeforeFirstSignIn`).
pub(crate) fn never_signed_in(factory: &Factory) -> bool {
    let inner = factory.account.lock();
    let saved = &inner.saved;
    !saved.signed_in() && saved.signed_in_ms == 0 && saved.last_check_in_ms == 0 && saved.signing_out.is_empty()
}

/// Whether agents' commands may ask the account's devices to approve this
/// Claktory (AccountAuthorize from a Tool connection): only a Claktory being
/// set up, never signed in and with no device yet (Fugo). One that's in use
/// is the person's to sign in: an agent could otherwise sign it in to an
/// account of someone else's, whose device would then get in.
pub(crate) fn tools_may_authorize(factory: &Factory) -> bool {
    let never_signed_in = {
        let inner = factory.account.lock();
        let saved = &inner.saved;
        !saved.signed_in() && saved.signed_in_ms == 0 && saved.signing_out.is_empty()
    };
    never_signed_in && !crate::remote::has_devices(&factory.home) && !crate::remote::app_ever_connected(&factory.home)
}

/// AccountVerify: signs in with the emailed code, and protects the vault
/// with the account. Returns the account it signed in to, once it's this
/// Claktory's and saved; None when the account answered that the person
/// approves it on a device first (path 4b: an account that has one), so it
/// now waits for that (`poll_approval`).
pub(crate) async fn verify(factory: &Arc<Factory>, code: &str) -> Result<Option<String>, RequestError> {
    enabled(factory)?;
    let account = &factory.account;
    let _busy = account.busy.lock().await;
    let (pending, switching_from) = {
        let inner = account.lock();
        (inner.pending.clone(), inner.switching().then(|| inner.saved.email.clone()))
    };
    let Some(pending) = pending else {
        return Err(refused(ErrorCode::InvalidRequest, "Enter your email address first."));
    };
    if pending.approval.is_some() {
        return Err(refused(ErrorCode::InvalidRequest, WAITING_FOR_APPROVAL));
    }
    let Some(verifying) = api::verifying_key(&pending.service_key) else {
        return Err(refused(ErrorCode::Internal, "The service's key is unreadable."));
    };
    let code: String = code.chars().filter(|character| !character.is_whitespace() && *character != '-').collect();
    let ephemeral = Ephemeral::new();
    let asked = SignInVerify { request_id: pending.request_id.clone(), code, eph_pub: ephemeral.public_b64() };
    // Signed with the backend key, which proves to the service that it's
    // this Claktory: signed out of another account, it moves to this one
    // with this code, rather than being turned away as taken.
    // A sign-out there the service hasn't heard of left the sign-in it
    // ended registered with the key it signed in with: that one proves it.
    let (_, key) = account.identity()?;
    let signed_out_with = account.lock().saved.signing_out.iter().find(|out| out.service == pending.service).and_then(SigningOut::key);
    let signer = Some(("", signed_out_with.as_ref().unwrap_or(&key)));
    let signed_in: SignedIn = match account.call(&pending.service, Some(&verifying), "POST", "/v1/sign-in/verify", &asked, signer).await {
        Ok(signed_in) => signed_in,
        // Switching accounts (V6): the account signed in stays as it was,
        // its key and identity too; only the sign-in asked for ends.
        Err(Failure::Refused(_, body)) if switching_from.is_some() && ["backend_key_retired", "claktory_taken"].contains(&body.error.as_str()) => {
            account.lock().pending = None;
            changed(factory);
            let current = switching_from.unwrap_or_default();
            return Err(refused(ErrorCode::InvalidRequest, format!("Claktory couldn't switch to that account. It's still signed in as {current}.")));
        }
        // The account has a device that may approve: the code is spent, and
        // the person approves this Claktory there (path 4b).
        Err(Failure::Refused(_, ErrorBody { error, pending: Some(waiting), .. })) if error == "authorization_pending" => {
            {
                let mut inner = account.lock();
                if let Some(pending) = inner.pending.as_mut() {
                    pending.request_id.clone_from(&waiting.request_id);
                    pending.expires_ms = now_ms() + waiting.expires_in * 1000;
                    pending.approval = Some(Approval::new(waiting.interval));
                }
                inner.problem.clear();
            }
            tracing::info!(service = pending.service, "the code was right; waiting for the person to approve this Claktory on a device");
            account.wake.notify_one();
            changed(factory);
            return Ok(None);
        }
        Err(Failure::Refused(_, body)) if body.error == "backend_key_retired" => {
            account.lock().pending = None;
            account.new_key();
            changed(factory);
            return Err(refused(ErrorCode::InvalidRequest, "That code no longer works. Sign in again."));
        }
        Err(Failure::Refused(_, body)) if body.error == "claktory_taken" => {
            account.lock().pending = None;
            account.new_identity();
            changed(factory);
            return Err(refused(
                ErrorCode::InvalidRequest,
                "This Claktory was signed in to another account before. Sign in again to use this one.",
            ));
        }
        Err(failure) => return Err(sign_in_error(&pending.service, failure)),
    };
    complete(factory, &pending, &signed_in, &ephemeral, "").await.map(Some)
}

/// Signed in, by the emailed code or a device's approval (`first_device`,
/// its account key; empty for a code): keeps the sign-in, and protects the
/// vault with the account. Returns the account. Called with `busy` held.
async fn complete(
    factory: &Arc<Factory>,
    pending: &Pending,
    signed_in: &SignedIn,
    ephemeral: &Ephemeral,
    first_device: &str,
) -> Result<String, RequestError> {
    let account = &factory.account;
    if signed_in.service_key != pending.service_key || signed_in.claktory_id != pending.start.claktory_id {
        return Err(sign_in_error(&pending.service, Failure::Unreachable("a sign-in for another key or Claktory".to_owned())));
    }
    let Some(share) = ephemeral.open(&signed_in.sealed_share, &signed_in.claktory_id, signed_in.epoch) else {
        return Err(sign_in_error(&pending.service, Failure::Unreachable("a share that doesn't open".to_owned())));
    };
    let local = account.local_share()?;
    // The vault's account protector from before, if it's locked: its share
    // is the last epoch's, which the new token may ask for once.
    let before = State::params(factory);
    {
        let mut inner = account.lock();
        inner.pending = None;
        inner.problem.clear();
        inner.saved.service = pending.service.clone();
        inner.saved.service_key = signed_in.service_key.clone();
        inner.saved.service_key_from = pending.key_from;
        inner.saved.account_id = signed_in.account_id.clone();
        inner.saved.email = signed_in.email.clone();
        inner.saved.machine_token = signed_in.machine_token.clone();
        inner.saved.epoch = signed_in.epoch;
        // Signing in again signs out the app vouched for before.
        inner.saved.vouched_app = None;
        inner.saved.name = pending.start.name.clone();
        inner.saved.revoked = false;
        inner.saved.signed_in_ms = now_ms();
        inner.saved.first_device = first_device.to_owned();
        // Signed in there again: the sign-in a sign-out not told yet ended
        // has a new token now, so nothing's left to tell.
        inner.saved.signing_out.retain(|out| out.service != pending.service);
        if inner.saved.periods_until == 0 {
            inner.saved.periods_until = now_s() / 3600 * 3600;
        }
        inner.unlock_tries = 0;
        inner.unlock_stopped = false;
        inner.endpoints_sent = None;
        inner.next_check_in = Instant::now();
    }
    account.save();
    tracing::info!(service = pending.service, epoch = signed_in.epoch, "signed in to the account service");
    if factory.vault.exists() && !factory.vault.is_open() {
        match before.filter(|before| before.claktory_id == signed_in.claktory_id) {
            Some(before) => match account.share(Some(before.epoch)).await {
                Ok((_, old)) => {
                    let shares = Zeroizing::new([local.as_slice(), old.as_slice()].concat());
                    if !factory.vault.open_with_account(&shares)? {
                        // The service's shares changed (a key rotation): asking
                        // again would only spend its one share a minute.
                        tracing::warn!("the last epoch's share doesn't open the vault");
                        account.lock().unlock_stopped = true;
                    }
                }
                Err(Failure::Refused(_, body)) if body.error == "rate_limited" => {
                    let wait = Duration::from_secs(body.retry_after.unwrap_or(60).clamp(1, 3600) as u64);
                    account.lock().next_unlock = Instant::now() + wait;
                }
                Err(failure) => tracing::warn!(%failure, "could not fetch the last epoch's share"),
            },
            None => tracing::warn!("signed in while the vault is locked, without an account protector of this Claktory"),
        }
    }
    if factory.vault.is_open() {
        let params = Params {
            service: pending.service.clone(),
            account_id: signed_in.account_id.clone(),
            claktory_id: signed_in.claktory_id.clone(),
            epoch: signed_in.epoch,
            service_key: signed_in.service_key.clone(),
        };
        let shares = Zeroizing::new([local.as_slice(), share.as_slice()].concat());
        factory.vault.protect_with_account(&signed_in.account_id, &serde_json::to_string(&params)?, &shares)?;
        // The account's recovery key (#239), compared with the one the vault
        // is sealed to.
        recovery::compare(factory, signed_in.recovery_key.clone()).await;
    } else if factory.vault.exists() {
        account.set_problem("Your secrets stay locked until you open them with the recovery key.");
    }
    factory.vault_changed();
    forget_set_aside(&account.dir);
    relay_token::signed_in(factory);
    // Switched from another account: devices let in through that one are
    // turned away (row 713).
    factory.account_open();
    account.wake.notify_one();
    Ok(signed_in.account_id.clone())
}

/// AccountSignOut: the service is told first (#524), so the sign-in ends
/// there too: its token, backend key, app grants and relay tokens no longer
/// work for anyone holding a copy. When it can't be reached, it's told
/// later (`tell_sign_outs`), and the app says so meanwhile. Here it signs
/// out either way, with a new backend key for the next sign-in. An open
/// vault opens by itself again, with a key on disk.
pub(crate) async fn sign_out(factory: &Arc<Factory>) -> Result<(), RequestError> {
    enabled(factory)?;
    let account = &factory.account;
    let _busy = account.busy.lock().await;
    let out = {
        let inner = account.lock();
        let saved = &inner.saved;
        saved.signed_in().then(|| SigningOut {
            service: saved.service.clone(),
            service_key: saved.service_key.clone(),
            machine_token: saved.machine_token.clone(),
            backend_key: String::new(),
            since_ms: now_ms(),
            tries: 0,
        })
    };
    if let Some(mut out) = out {
        let (_, key) = account.identity()?;
        out.backend_key = hex(key.as_bytes());
        if !account.tell_signed_out(&out).await {
            let mut inner = account.lock();
            inner.saved.signing_out.retain(|kept| kept.service != out.service);
            inner.saved.signing_out.push(out);
            inner.next_sign_out = Instant::now() + sign_out_wait(0);
            account.wake.notify_one();
        }
        account.new_key();
    }
    factory.vault.drop_account()?;
    {
        let mut inner = account.lock();
        inner.pending = None;
        inner.problem.clear();
        let saved = &mut inner.saved;
        for field in [&mut saved.service, &mut saved.service_key, &mut saved.account_id, &mut saved.email, &mut saved.machine_token] {
            field.clear();
        }
        saved.epoch = 0;
        saved.service_key_from = None;
        saved.revoked = false;
        saved.periods.clear();
        saved.periods_until = 0;
        saved.vouched_app = None;
    }
    account.save();
    tracing::info!("signed out of the account service");
    relay_token::signed_out(factory);
    factory.vault_changed();
    // Devices let in through the account are turned away (row 713), each
    // after the answer it's being sent.
    factory.account_open();
    Ok(())
}

/// How long after `tries` failed tries a sign-out is sent again: a minute,
/// then twice as long each time, up to an hour.
fn sign_out_wait(tries: u32) -> Duration {
    Duration::from_secs((60_u64 << tries.min(6)).min(3600))
}

/// Tells the services the sign-outs they haven't heard of yet, if it's
/// time (all of them with `now`). Called with `busy` held.
async fn tell_sign_outs(factory: &Arc<Factory>, now: bool) {
    let account = &factory.account;
    let due = {
        let inner = account.lock();
        let due = now || inner.next_sign_out <= Instant::now();
        if due { inner.saved.signing_out.clone() } else { Vec::new() }
    };
    if due.is_empty() {
        return;
    }
    let mut told = Vec::new();
    for out in &due {
        if account.tell_signed_out(out).await {
            told.push(out.service.clone());
        }
    }
    {
        let mut inner = account.lock();
        inner.saved.signing_out.retain(|kept| !told.contains(&kept.service));
        for kept in &mut inner.saved.signing_out {
            kept.tries = kept.tries.saturating_add(1);
        }
        let tries = inner.saved.signing_out.iter().map(|kept| kept.tries).min().unwrap_or(0);
        inner.next_sign_out = Instant::now() + sign_out_wait(tries);
    }
    account.save();
    if !told.is_empty() {
        changed(factory);
    }
}

/// SkipAccountSignIn, from apps that offered Not now: refused, since every
/// Claktory must sign in (#195).
pub(crate) fn skip(factory: &Arc<Factory>) -> Result<(), RequestError> {
    enabled(factory)?;
    let why =
        if factory.account.lock().saved.signed_in() { "This Claktory is already signed in." } else { "Sign in to use this Claktory." };
    Err(refused(ErrorCode::InvalidRequest, why))
}

/// SetAccountService: where the next sign-in goes; empty for claktory.com.
pub(crate) async fn set_service(factory: &Arc<Factory>, url: &str) -> Result<(), RequestError> {
    enabled(factory)?;
    {
        let inner = factory.account.lock();
        if inner.pending.is_some() || (inner.saved.signed_in() && !inner.saved.revoked) {
            return Err(refused(ErrorCode::InvalidRequest, "Sign out first."));
        }
    }
    let url = url.trim();
    let service = if url.is_empty() || service_url(url).as_deref() == Some(DEFAULT_SERVICE) {
        String::new()
    } else {
        service_url(url)
            .ok_or_else(|| refused(ErrorCode::InvalidRequest, "Enter the service's https:// address, like https://account.example.com."))?
    };
    factory
        .edit_config(|config| {
            config.account.service = service;
            Ok(())
        })
        .await
}

/// Opens the vault with the account now, if it's locked and waiting for
/// the service, unless the service was asked a moment ago: an app
/// connected, or an agent asks for a secret.
pub(crate) async fn unlock_now(factory: &Arc<Factory>) {
    let due = || {
        if factory.vault.is_open() || !factory.vault.exists() || !factory.lock().config.account.enabled {
            return false;
        }
        let inner = factory.account.lock();
        let asked_lately = inner.share_asked.is_some_and(|asked| asked.elapsed() < SHARE_GAP);
        inner.saved.signed_in() && !inner.saved.revoked && !inner.unlock_stopped && !asked_lately
    };
    if !due() {
        return;
    }
    let _busy = factory.account.busy.lock().await;
    // Again: what held it may have opened the vault, or stopped asking.
    if due() {
        try_unlock(factory).await;
    }
}

/// Asks for the share and opens the vault; schedules the next try.
async fn try_unlock(factory: &Arc<Factory>) {
    let account = &factory.account;
    let Some(params) = State::params(factory) else { return };
    let stop = |problem: String| {
        let mut inner = account.lock();
        inner.unlock_stopped = true;
        inner.problem = problem;
    };
    let name = service_name(&params.service);
    if params.claktory_id != account.lock().saved.claktory_id {
        stop(format!("Your secrets don't open with this sign-in. Open them with the recovery key, then sign in to {name} again."));
        return;
    }
    let asked = account.share(Some(params.epoch)).await;
    let change = if matches!(asked, Err(Failure::OtherKey)) { key_change(account).await } else { KeyChange::Same };
    match asked {
        Ok((_, share)) => {
            let opened = account.local_share().map_err(|error| error.to_string()).and_then(|local| {
                let shares = Zeroizing::new([local.as_slice(), share.as_slice()].concat());
                factory.vault.open_with_account(&shares).map_err(|error| error.to_string())
            });
            match opened {
                Ok(true) => {
                    tracing::info!("the vault is open: the account service gave its share");
                    let mut inner = account.lock();
                    inner.unlock_tries = 0;
                    inner.problem.clear();
                }
                Ok(false) => {
                    tracing::error!("the account's share doesn't open the vault");
                    stop("Your secrets don't open with this Claktory's sign-in. Open them with the recovery key.".to_owned());
                }
                Err(error) => {
                    tracing::error!(error, "could not open the vault with the account");
                    stop("Your secrets don't open with this Claktory's sign-in. Open them with the recovery key.".to_owned());
                }
            }
        }
        Err(Failure::Refused(_, body)) if body.error == "revoked" => {
            account.lock().saved.revoked = true;
            stop(String::new());
        }
        // Asking again would only spend the service's one share a minute,
        // which signing in again then needs.
        Err(Failure::OtherKey) if change == KeyChange::SignIn => stop(changed_key(&name)),
        // The new key is the service's on its root's word: asked again with it.
        Err(Failure::OtherKey) if change == KeyChange::Taken => account.lock().next_unlock = Instant::now(),
        Err(Failure::OtherKey) if change == KeyChange::Unverified => {
            let mut inner = account.lock();
            let wait = UNLOCK_RETRIES.get(inner.unlock_tries).copied().unwrap_or(UNLOCK_RETRY_LAST);
            inner.unlock_tries += 1;
            inner.next_unlock = Instant::now() + wait;
            inner.problem = format!("{}, so your secrets stay locked for now. Claktory keeps trying.", unverified_key(&name));
        }
        Err(Failure::Refused(_, body)) if body.error == "rate_limited" => {
            let wait = Duration::from_secs(body.retry_after.unwrap_or(60).clamp(1, 3600) as u64);
            account.lock().next_unlock = Instant::now() + wait;
        }
        Err(Failure::Refused(status, body)) if status == 401 || body.error == "invalid_request" => {
            tracing::warn!(status, error = body.error, "the account service refused the share");
            stop(format!("{name} no longer knows this Claktory. Sign in again, or open your secrets with the recovery key."));
        }
        Err(failure) => {
            tracing::warn!(%failure, "could not fetch the vault's share; trying again");
            let problem = account.offline();
            let mut inner = account.lock();
            let wait = UNLOCK_RETRIES.get(inner.unlock_tries).copied().unwrap_or(UNLOCK_RETRY_LAST);
            inner.unlock_tries += 1;
            inner.next_unlock = Instant::now() + wait;
            inner.problem = problem;
        }
    }
    account.save();
    factory.vault_changed();
}

/// What an answer signed by another key than the pinned one means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyChange {
    /// Nothing known: its well-known names the pinned key, or can't be had.
    Same,
    /// It changed its key, and isn't anchored (`anchor`): signing in again
    /// takes the new one.
    SignIn,
    /// It changed its key, and its root vouched for the new one, newer than
    /// the pinned one's statement: it's pinned now.
    Taken,
    /// Another key, without the root's word for it (or an older one): the
    /// pinned key stays.
    Unverified,
}

/// Asks the service's well-known what key it has now, and takes it when its
/// root vouches for it.
async fn key_change(account: &State) -> KeyChange {
    let (service, pinned, pinned_from) = {
        let inner = account.lock();
        (inner.saved.service.clone(), inner.saved.service_key.clone(), inner.saved.service_key_from)
    };
    let Ok(known) = account.well_known(&service).await else { return KeyChange::Same };
    if known.service_key == pinned {
        return KeyChange::Same;
    }
    if !anchor::anchored(&service) {
        tracing::warn!("the account service changed its key since this Claktory signed in");
        return KeyChange::SignIn;
    }
    match anchor::vouch(&service, &known) {
        anchor::Vouch::Vouched(from) if pinned_from.is_none_or(|pinned| from > pinned) => {
            tracing::warn!(from, "the account service changed its key, and its root vouches for the new one; taking it");
            let mut inner = account.lock();
            inner.saved.service_key = known.service_key;
            inner.saved.service_key_from = Some(from);
            drop(inner);
            account.save();
            KeyChange::Taken
        }
        vouched => {
            tracing::error!(?vouched, "the account service answers with a key its root doesn't vouch for; keeping the pinned one");
            KeyChange::Unverified
        }
    }
}

/// Notes when the root's statement for the pinned key says it's the
/// service's from, for a key pinned without one (`anchor`): asked at each
/// check-in until it's known.
async fn note_key_from(account: &State) {
    let (service, pinned) = {
        let inner = account.lock();
        if inner.saved.service_key_from.is_some() {
            return;
        }
        (inner.saved.service.clone(), inner.saved.service_key.clone())
    };
    let Ok(known) = account.well_known(&service).await else { return };
    if known.service_key != pinned {
        return;
    }
    let (anchor::Vouch::Free(Some(from)) | anchor::Vouch::Vouched(from)) = anchor::vouch(&service, &known) else { return };
    tracing::info!(from, "the account root vouches for the pinned service key");
    account.lock().saved.service_key_from = Some(from);
    account.save();
}

/// What the person reads, first, when the service answers with a key the
/// root doesn't vouch for.
fn unverified_key(name: &str) -> String {
    format!("Couldn't confirm this is really {name}")
}

/// What the person reads when the service changed its key.
fn changed_key(name: &str) -> String {
    format!("{name} changed its key. Sign in again, and your secrets open by themselves.")
}

/// Checks in; reseals the vault when the service moved to a new epoch, and
/// registers endpoints that changed.
async fn check_in(factory: &Arc<Factory>) {
    let account = &factory.account;
    factory.store.write_calls();
    let made = {
        let since = account.lock().saved.periods_until;
        let names = analytics::names(factory);
        analytics::new_periods(&factory.store.db(), since, now_s(), &names)
    };
    match made {
        Ok(made) => {
            let mut inner = account.lock();
            if let Some(last) = made.last() {
                inner.saved.periods_until = last.to;
            }
            inner.saved.periods.extend(made);
        }
        Err(error) => tracing::warn!(%error, "could not count usage for the check-in"),
    }
    account.save();
    let mut body = CheckIn {
        version: crate::update::VERSION.to_owned(),
        platform: platform(),
        counts: Some(analytics::counts(factory)),
        periods: Vec::new(),
        setup: Some(analytics::setup(factory)),
        devices: Some(analytics::devices(factory)),
        removed_since: Some(account.lock().saved.removed_cursor.clone()),
    };
    let base = serde_json::to_string(&body).map_or(0, |json| json.len());
    body.periods = analytics::to_send(&account.lock().saved.periods, base, now_s());
    let jitter = Duration::from_secs(u64::from(api::random_32()[0]) * CHECK_IN_JITTER * 2 / 255);
    let next = CHECK_IN_EVERY - Duration::from_secs(CHECK_IN_JITTER) + jitter;
    let checked = account.signed::<CheckedIn>("/v1/check-in", &body).await;
    let change = if matches!(checked, Err(Failure::OtherKey)) { key_change(account).await } else { KeyChange::Same };
    match checked {
        Ok(checked) => {
            {
                let mut inner = account.lock();
                inner.saved.last_check_in_json = serde_json::to_string_pretty(&body).unwrap_or_default();
                inner.saved.last_check_in_ms = now_ms();
                inner.next_check_in = Instant::now() + next;
                // Recorded, or out of range and never to be sent again.
                let sent: Vec<&str> = body.periods.iter().map(|period| period.id.as_str()).collect();
                inner.saved.periods.retain(|period| !sent.contains(&period.id.as_str()));
                if checked.periods_ignored > 0 {
                    tracing::warn!(ignored = checked.periods_ignored, "the account service left out periods as out of range");
                }
                if !inner.unlock_stopped {
                    inner.problem.clear();
                }
            }
            // Accepted periods are gone from disk at once, before anything
            // else is awaited (and broadcast).
            account.save();
            heard_waiting(factory, checked.authorizations_waiting.clone());
            moves::heard_waiting(factory, &checked.moves_waiting);
            removed_heard(factory, &checked).await;
            reseal(factory, checked.epoch).await;
            recovery::compare(factory, checked.recovery_key).await;
            register_endpoints(factory).await;
            relay_token::renew(factory).await;
            note_key_from(account).await;
            crate::user_apps::reconcile(factory).await;
        }
        Err(Failure::Refused(_, body)) if body.error == "revoked" => {
            tracing::warn!("this Claktory was removed at the account service");
            let mut inner = account.lock();
            inner.saved.revoked = true;
            inner.next_check_in = Instant::now() + next;
        }
        Err(Failure::OtherKey) if change == KeyChange::SignIn => {
            let mut inner = account.lock();
            inner.problem = changed_key(&service_name(&inner.saved.service));
            inner.next_check_in = Instant::now() + next;
        }
        // Checked in again at once, with the key its root vouched for.
        Err(Failure::OtherKey) if change == KeyChange::Taken => account.lock().next_check_in = Instant::now(),
        Err(Failure::OtherKey) if change == KeyChange::Unverified => {
            let locked = factory.vault.exists() && !factory.vault.is_open();
            let mut inner = account.lock();
            let unverified = unverified_key(&service_name(&inner.saved.service));
            inner.problem = if locked {
                format!("{unverified}, so your secrets stay locked for now. Claktory keeps trying.")
            } else {
                format!("{unverified}. Claktory keeps trying.")
            };
            inner.next_check_in = Instant::now() + CHECK_IN_RETRY;
        }
        Err(Failure::Refused(401, body)) => {
            tracing::warn!(error = body.error, "the account service doesn't know this Claktory's token");
            let mut inner = account.lock();
            inner.problem = format!("{} no longer knows this Claktory. Sign in again.", service_name(&inner.saved.service));
            inner.next_check_in = Instant::now() + next;
        }
        Err(failure) => {
            tracing::info!(%failure, "check-in failed; trying again later");
            let problem = account.offline();
            let mut inner = account.lock();
            inner.next_check_in = Instant::now() + CHECK_IN_RETRY;
            if !inner.unlock_stopped {
                inner.problem = problem;
            }
        }
    }
    account.save();
    account.checked.notify_waiters();
    changed(factory);
}

/// How long what check-ins heard of removed devices counts as fresh
/// before a device is let in (E11-8 point 6), and how long a device's Hello
/// waits for a check-in that hears of them again.
const REMOVALS_FRESH: Duration = Duration::from_secs(3600);
const REMOVALS_WAIT: Duration = Duration::from_secs(5);
/// A check-in that couldn't hear of them isn't asked for again by every
/// Hello: at most this often.
const REMOVALS_ASK_GAP: Duration = Duration::from_secs(60);

/// What a check-in heard of devices removed from the account (Lock out on
/// another device, Remove at claktory.com/account): each device here that
/// connected with one of those keys is unpaired, then the cursor is kept.
/// A service from before sends no cursor, and nothing changes.
async fn removed_heard(factory: &Arc<Factory>, checked: &CheckedIn) {
    // As fresh as the service can tell, whether or not it keeps a cursor.
    factory.account.lock().removals_heard = Some(Instant::now());
    let Some(cursor) = &checked.removed_cursor else { return };
    for key in &checked.removed_device_keys {
        let unpaired = factory.unpair_account_key(key, crate::remote::Removal::Account).await;
        if unpaired > 0 {
            tracing::warn!(unpaired, "unpaired devices removed from the account");
        }
    }
    let account = &factory.account;
    account.lock().saved.removed_cursor.clone_from(cursor);
    account.save();
}

/// Before a device is let in by its assertion or its key (E11-8 point 6):
/// while signed in, a check-in hears of devices removed from the account
/// first, at start and once what the last one heard is over an hour old,
/// so a Claktory that was offline during Lock out turns the device away on
/// its first connection back. Waits `REMOVALS_WAIT` at most, and lets the
/// device in as before when the service can't be reached (fails open).
pub(crate) async fn removals_fresh(factory: &Arc<Factory>) {
    if !factory.lock().config.account.enabled {
        return;
    }
    let account = &factory.account;
    let checked = account.checked.notified();
    tokio::pin!(checked);
    {
        let mut inner = account.lock();
        let due = inner.saved.signed_in()
            && !inner.saved.revoked
            && inner.removals_heard.is_none_or(|heard| heard.elapsed() > REMOVALS_FRESH)
            && inner.removals_asked.is_none_or(|asked| asked.elapsed() > REMOVALS_ASK_GAP);
        if !due {
            return;
        }
        inner.removals_asked = Some(Instant::now());
        inner.next_check_in = Instant::now();
    }
    checked.as_mut().enable();
    account.wake.notify_one();
    if tokio::time::timeout(REMOVALS_WAIT, checked).await.is_err() {
        tracing::info!("no check-in in time to hear of removed devices; letting the device in as before");
    }
}

/// The vault was opened by hand, with its passphrase or recovery key: while
/// signed in, it's sealed under the account's current share at once (by
/// `monitor`), rather than at the next check-in. Its account protector may
/// no longer open (the service's share changed, as after a key rotation),
/// and a restart before then would lock it again.
pub(crate) fn opened_by_hand(factory: &Arc<Factory>) {
    if !factory.lock().config.account.enabled || !factory.vault.is_open() {
        return;
    }
    let account = &factory.account;
    {
        let mut inner = account.lock();
        if !inner.saved.signed_in() || inner.saved.revoked {
            return;
        }
        inner.reseal_due = Some(Instant::now());
    }
    account.wake.notify_one();
}

/// Seals the open vault under the account's current share; tried again
/// when the service can't be asked yet (its one share a minute, say).
async fn reseal_now(factory: &Arc<Factory>) {
    let account = &factory.account;
    let retry = |wait: Duration| account.lock().reseal_due = Some(Instant::now() + wait);
    let (got, share) = match account.share(None).await {
        Ok(share) => share,
        Err(Failure::Refused(_, body)) if body.error == "rate_limited" => {
            return retry(Duration::from_secs(body.retry_after.unwrap_or(60).clamp(1, 3600) as u64 + 1));
        }
        Err(Failure::Refused(status, body)) => {
            // It doesn't know this Claktory, or removed it: signing in
            // again seals the vault anyway.
            tracing::warn!(status, error = body.error, "the account service refused the share to reseal the vault");
            account.lock().reseal_due = None;
            return;
        }
        Err(failure) => {
            tracing::info!(%failure, "could not fetch the share to reseal the vault; trying again");
            return retry(Duration::from_secs(60));
        }
    };
    let params = {
        let mut inner = account.lock();
        inner.saved.epoch = got;
        Params {
            service: inner.saved.service.clone(),
            account_id: inner.saved.account_id.clone(),
            claktory_id: inner.saved.claktory_id.clone(),
            epoch: got,
            service_key: inner.saved.service_key.clone(),
        }
    };
    let sealed = (|| {
        let local = account.local_share().map_err(|error| error.to_string())?;
        let shares = Zeroizing::new([local.as_slice(), share.as_slice()].concat());
        let text = serde_json::to_string(&params).map_err(|error| error.to_string())?;
        factory.vault.protect_with_account(&params.account_id, &text, &shares).map_err(|error| error.to_string())
    })();
    match sealed {
        Ok(()) => {
            tracing::info!(epoch = got, "the vault, opened by hand, is sealed under the account's share");
            let mut inner = account.lock();
            inner.reseal_due = None;
            inner.unlock_stopped = false;
            inner.unlock_tries = 0;
            inner.problem.clear();
        }
        Err(error) => {
            tracing::warn!(error, "could not seal the vault under the account's share; trying again");
            retry(Duration::from_secs(60));
        }
    }
    account.save();
}

/// The service moved to `epoch`: the open vault is sealed under its share.
async fn reseal(factory: &Arc<Factory>, epoch: u64) {
    let account = &factory.account;
    account.lock().saved.epoch = epoch;
    let Some(params) = State::params(factory) else { return };
    if params.epoch >= epoch || !factory.vault.is_open() {
        return;
    }
    let (got, share) = match account.share(None).await {
        Ok(share) => share,
        // The share was given a moment ago, to open the vault after a
        // restart: once the service allows another, check in again.
        Err(Failure::Refused(_, body)) if body.error == "rate_limited" => {
            let wait = Duration::from_secs(body.retry_after.unwrap_or(60).clamp(1, 3600) as u64 + 1);
            account.lock().next_check_in = Instant::now() + wait;
            return;
        }
        Err(failure) => {
            tracing::warn!(%failure, epoch, "could not fetch the account's new share");
            return;
        }
    };
    let result = (|| {
        if got != epoch {
            return Err(format!("asked for epoch {epoch}, got {got}"));
        }
        let local = account.local_share().map_err(|error| error.to_string())?;
        let shares = Zeroizing::new([local.as_slice(), share.as_slice()].concat());
        let params = Params { epoch, ..params };
        let text = serde_json::to_string(&params).map_err(|error| error.to_string())?;
        factory.vault.protect_with_account(&params.account_id, &text, &shares).map_err(|error| error.to_string())
    })();
    match result {
        Ok(()) => {
            tracing::info!(epoch, "the vault is sealed under the account's new share");
            // It opens by itself again: nothing waits for the recovery key.
            let mut inner = account.lock();
            inner.reseal_due = None;
            inner.unlock_stopped = false;
            inner.problem.clear();
        }
        Err(error) => tracing::warn!(error, epoch, "could not reseal the vault under the account's new share"),
    }
}

/// The iroh endpoint ids of this backend and its paired devices, with this
/// Claktory's name, color and relays for devices that find it through the
/// account.
fn endpoints(factory: &Factory) -> Option<Endpoints> {
    let backend = crate::remote::secret_key(&factory.home).ok()?.public().to_string();
    if !api::valid_endpoint(&backend) {
        return None;
    }
    let mut devices: Vec<String> = crate::remote::devices(&factory.home)
        .ok()?
        .into_iter()
        .map(|device| device.endpoint_id)
        .filter(|id| api::valid_endpoint(id) && *id != backend)
        .collect();
    devices.sort();
    devices.dedup();
    devices.truncate(MAX_ENDPOINTS - 1);
    let identity = factory.identity();
    // The relays its pairing codes give: the one it's reached through now,
    // then those it's set to use.
    let shown = factory.remote_view(&factory.lock());
    let mut relays: Vec<String> = Vec::new();
    for relay in std::iter::once(shown.home_relay).chain(shown.relays) {
        if !relay.is_empty() && !relays.iter().any(|known| known.trim_end_matches('/') == relay.trim_end_matches('/')) {
            relays.push(relay);
        }
    }
    Some(Endpoints { backend, devices, name: Some(identity.name), color: Some(identity.color), relays: Some(relays) })
}

/// Registers the endpoints, if they changed since they last were.
async fn register_endpoints(factory: &Arc<Factory>) {
    let account = &factory.account;
    let Some(now) = endpoints(factory) else { return };
    if account.lock().endpoints_sent.as_ref() == Some(&now) {
        return;
    }
    let body = now.clone();
    let (service, service_key, token) = {
        let inner = account.lock();
        (inner.saved.service.clone(), inner.saved.service_key.clone(), inner.saved.machine_token.clone())
    };
    let (Some(key), Ok((_, backend_key))) = (api::verifying_key(&service_key), account.identity()) else { return };
    let result: Result<api::EndpointsSet, Failure> =
        account.call(&service, Some(&key), "PUT", "/v1/endpoints", &body, Some((&token, &backend_key))).await;
    match result {
        Ok(set) => {
            tracing::info!(count = set.count, "endpoints registered with the account service");
            account.lock().endpoints_sent = Some(now);
        }
        Err(failure) => tracing::info!(%failure, "could not register endpoints"),
    }
}

/// This Claktory's pipe peer statement, for the Claktories it has pipes
/// with: the account service's word that its pipe key (its tunnel key) is a
/// Claktory of its account, with the account's email, for a week. Its
/// endpoints are registered first, since the service vouches only for the
/// registered one, and the tunnel key signs the request too, proving it's
/// held. Ok(None) when it can't have one: not signed in, removed from its
/// account, unknown to the service, or an email that can't be confirmed to
/// others. Err when the service couldn't be asked now; try again later.
pub(crate) async fn pipe_identity(factory: &Arc<Factory>) -> Result<Option<String>, String> {
    let account = &factory.account;
    let registration = account.registration(&factory.lock().config.account);
    let Some((_, claktory_id, _)) = registration else { return Ok(None) };
    let endpoint = crate::remote::secret_key(&factory.home).map_err(|error| error.to_string())?;
    register_endpoints(factory).await;
    let endpoint_id = endpoint.public().to_string();
    let time = now_s();
    let proof = api::b64(endpoint.sign(&api::pipe_identity_proof(&claktory_id, &endpoint_id, time)).to_bytes());
    let request = api::PipeIdentityRequest { endpoint_id, time, proof };
    match account.signed::<api::PipeIdentityIssued>("/v1/pipe-identity", &request).await {
        Ok(issued) => Ok(Some(issued.statement)),
        Err(Failure::Refused(401, _)) => Ok(None),
        Err(Failure::Refused(403, body)) if body.error == "revoked" || body.error == "unsupported_email" => Ok(None),
        Err(failure) => Err(failure.to_string()),
    }
}

/// How soon a move's change of credentials is tried again.
const MOVE_RETRY: Duration = Duration::from_secs(300);

/// The new pair a move's change asked for, kept until the service took it,
/// so a lost answer can be checked (`<account>/moving.json`).
#[derive(Serialize, Deserialize)]
struct MovingPair {
    epoch: u64,
    /// The new backend key's seed, hex.
    key: String,
    token: String,
}

/// After Claktory moved here (#217): asks the account service to replace
/// this Claktory's token and backend key with new ones (`POST /v1/move`),
/// so the copy on the old computer can't use them. Once per move: true when
/// done or nothing to do, false to try again later (the service couldn't be
/// reached). Called with `busy` held.
async fn after_move(factory: &Arc<Factory>) -> bool {
    let account = &factory.account;
    let home = account.dir.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
    let epoch: u64 = std::fs::read_to_string(home.join("move-epoch")).ok().and_then(|text| text.trim().parse().ok()).unwrap_or(0);
    if epoch <= account.lock().saved.move_epoch {
        return true;
    }
    let done = |account: &State| {
        account.lock().saved.move_epoch = epoch;
        account.save();
        let _ = std::fs::remove_file(account.dir.join("moving.json"));
    };
    // The pair asked for, the same on every try for this move.
    let path = account.dir.join("moving.json");
    let kept =
        std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice::<MovingPair>(&bytes).ok()).filter(|pair| pair.epoch == epoch);
    let pair = match kept {
        Some(pair) => pair,
        None => {
            let key = api::new_signing_key();
            let pair = MovingPair { epoch, key: hex(key.as_bytes()), token: api::b64(*api::random_32()) };
            let written = serde_json::to_vec(&pair).map_err(std::io::Error::other).and_then(|json| {
                create_private_dir(&account.dir)?;
                crate::remote::write_private(&path, &json)
            });
            if let Err(error) = written {
                tracing::error!(%error, "could not keep the new account credentials for the move; trying again later");
                return false;
            }
            pair
        }
    };
    let Some(new_key) =
        unhex(&pair.key).and_then(|bytes| <[u8; 32]>::try_from(bytes.as_slice()).ok()).map(|seed| SigningKey::from_bytes(&seed))
    else {
        let _ = std::fs::remove_file(&path);
        return false;
    };
    let from: String = factory
        .store
        .db()
        .query_row("SELECT machine FROM moves WHERE epoch = ?1 ORDER BY imported DESC LIMIT 1", [epoch as i64], |row| row.get(0))
        .unwrap_or_default();
    let request = MoveRequest {
        backend_key: api::b64(new_key.verifying_key().as_bytes()),
        machine_token: pair.token.clone(),
        name: computer_name(),
        version: crate::update::VERSION.to_owned(),
        platform: platform(),
        from,
    };
    let adopt = |account: &State| -> std::io::Result<()> {
        crate::remote::write_private(&account.dir.join("claktory.key"), format!("{}\n", hex(new_key.as_bytes())).as_bytes())?;
        {
            let mut inner = account.lock();
            inner.saved.machine_token = pair.token.clone();
            // The app vouched for on the computer it left is signed out.
            inner.saved.vouched_app = None;
        }
        account.save();
        Ok(())
    };
    match account.signed::<MoveDone>("/v1/move", &request).await {
        Ok(_) => {}
        // Moved already, its answer lost (or a crash before it was kept):
        // the new pair works if the service took it.
        Err(Failure::Refused(401, _)) => {
            let probe = CheckIn { version: crate::update::VERSION.to_owned(), platform: platform(), ..CheckIn::default() };
            if account.signed_as::<CheckedIn>("/v1/check-in", &probe, &pair.token, &new_key).await.is_err() {
                tracing::warn!("the account service didn't take this Claktory's new credentials after its move; it keeps the old ones");
                done(account);
                return true;
            }
        }
        Err(Failure::Refused(status, body)) => {
            tracing::warn!(status, error = body.error, "the account service refused the move's new credentials; it keeps the old ones");
            done(account);
            return true;
        }
        Err(failure @ (Failure::Unreachable(_) | Failure::OtherKey)) => {
            tracing::info!(%failure, "couldn't reach the account service to change credentials after the move; trying again later");
            return false;
        }
    }
    if let Err(error) = adopt(account) {
        tracing::error!(%error, "could not keep the new account credentials; trying again later");
        return false;
    }
    tracing::info!(epoch, "the account service has this Claktory's new credentials; the old copy's no longer work");
    done(account);
    changed(factory);
    true
}

/// For tests, which start another Factory on the same home in the same
/// process (the first one's serving goes on): this one's account no longer
/// runs, nor writes account.json over the other's.
#[doc(hidden)]
pub fn stop_for_tests(factory: &Factory) {
    factory.account.stopped.store(true, std::sync::atomic::Ordering::SeqCst);
    factory.account.wake.notify_one();
}

/// Runs the account while Claktory runs: after a restart, opens the vault
/// with the service's share (trying again while it can't be reached); checks
/// in at start and every hour.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    // An app connecting wakes the loops: the bell and the moves are asked
    // for only while one is, so a tick with none slept until the next
    // check-in.
    let Some(mut loops) = factory.upgrade().map(|factory| factory.loops_woken()) else { return };
    loop {
        let Some(strong) = factory.upgrade() else { return };
        if strong.account.stopped.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        loops.borrow_and_update();
        let wait = tick(&strong).await;
        let woken = async {
            strong.account.wake.notified().await;
        };
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = woken => {}
            seen = loops.changed() => {
                if seen.is_err() {
                    return;
                }
            }
        }
    }
}

/// Does what's due; how long until something is.
async fn tick(factory: &Arc<Factory>) -> Duration {
    const IDLE: Duration = Duration::from_secs(60);
    if !factory.lock().config.account.enabled {
        return IDLE;
    }
    let account = &factory.account;
    let _busy = account.busy.lock().await;
    tell_sign_outs(factory, false).await;
    poll_approval(factory).await;
    let (signed_in, revoked, sign_outs, approval) = {
        let inner = account.lock();
        let sign_outs = (!inner.saved.signing_out.is_empty()).then(|| inner.next_sign_out.saturating_duration_since(Instant::now()));
        let approval = inner
            .pending
            .as_ref()
            .and_then(|pending| pending.approval.as_ref())
            .map(|approval| approval.next_poll.saturating_duration_since(Instant::now()));
        (inner.saved.signed_in(), inner.saved.revoked, sign_outs, approval)
    };
    if !signed_in || revoked {
        let due = [sign_outs, approval].into_iter().flatten().min();
        return due.map_or(IDLE, |due| due.clamp(Duration::from_millis(100), IDLE));
    }
    ring_bell(factory).await;
    moves::take_approved(factory).await;
    // Moved here: new credentials first, so nothing more goes out with the old.
    let rotating = !after_move(factory).await;
    let locked = factory.vault.exists() && !factory.vault.is_open() && State::params(factory).is_some();
    let (unlock_due, stopped) = {
        let inner = account.lock();
        (inner.next_unlock <= Instant::now(), inner.unlock_stopped)
    };
    if locked && unlock_due && !stopped {
        try_unlock(factory).await;
    }
    let reseal_due = account.lock().reseal_due.is_some_and(|due| due <= Instant::now());
    if reseal_due && factory.vault.is_open() {
        reseal_now(factory).await;
        changed(factory);
    }
    let check_in_due = {
        let inner = account.lock();
        inner.next_check_in <= Instant::now() && !inner.saved.revoked
    };
    if check_in_due {
        check_in(factory).await;
    }
    let inner = account.lock();
    let now = Instant::now();
    let mut next = inner.next_check_in.saturating_duration_since(now);
    if factory.vault.exists() && !factory.vault.is_open() && !inner.unlock_stopped && !inner.saved.revoked {
        next = next.min(inner.next_unlock.saturating_duration_since(now));
    }
    if rotating {
        next = next.min(MOVE_RETRY);
    }
    if let Some(due) = inner.reseal_due {
        next = next.min(due.saturating_duration_since(now));
    }
    if let Some(due) = sign_outs {
        next = next.min(due);
    }
    if let Some(due) = approval {
        next = next.min(due);
    }
    // The bell, asked again while an app is connected.
    if factory.clients() > 0 {
        next = next.min(inner.waiting_asked.map_or(Duration::ZERO, |asked| BELL_EVERY.saturating_sub(asked.elapsed())));
    }
    if let Some(due) = moves::next_due(&inner, factory.clients() > 0) {
        next = next.min(due);
    }
    next.max(Duration::from_millis(100))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_same_request_signed_again_within_a_second_signs_a_later_time_so_it_isnt_a_replay() {
        let mut stamped = std::collections::HashMap::new();
        let now = 1_800_000_000;
        // Three callers list the same thing at once: three times, so three
        // signatures, none refused as seen before.
        let times: Vec<i64> = (0..3).map(|_| super::stamp(&mut stamped, now, "GET", "/v1/apps/grants", b"")).collect();
        assert_eq!(times, [now, now + 1, now + 2]);
        // Another request, or another body, is signed with now.
        assert_eq!(super::stamp(&mut stamped, now, "GET", "/v1/claktories", b""), now);
        assert_eq!(super::stamp(&mut stamped, now, "POST", "/v1/check-in", b"{\"a\":1}"), now);
        assert_eq!(super::stamp(&mut stamped, now, "POST", "/v1/check-in", b"{\"a\":2}"), now);
        // Once the clock passes the times used, it's now again.
        assert_eq!(super::stamp(&mut stamped, now + 5, "GET", "/v1/apps/grants", b""), now + 5);
        // Never more than a minute ahead: past it, now (the service's
        // replay answer then waits a second).
        let mut many = std::collections::HashMap::new();
        let last = (0..80).map(|_| super::stamp(&mut many, now, "GET", "/v1/apps/grants", b"")).max();
        assert_eq!(last, Some(now + 60));
        // Forgotten once the service would refuse the time anyway.
        super::stamp(&mut many, now + 400, "GET", "/v1/claktories", b"");
        assert_eq!(many.len(), 1);
    }

    #[test]
    fn a_long_wait_says_hours_or_tomorrow() {
        let at = |time: &str| -> jiff::ZonedDateTime { time.parse().expect("a time") };
        let morning = at("2026-11-20T09:00:00[Europe/Amsterdam]");
        assert_eq!(super::try_again(30, &morning), "in 1 minute");
        assert_eq!(super::try_again(90 * 60, &morning), "in 90 minutes");
        assert_eq!(super::try_again(91 * 60, &morning), "in 2 hours");
        assert_eq!(super::try_again(10 * 3600, &morning), "in 10 hours");
        assert_eq!(super::try_again(1316 * 60, &morning), "tomorrow");
        assert_eq!(super::try_again(2 * 3600, &at("2026-11-20T23:00:00[Europe/Amsterdam]")), "tomorrow");
    }
}
