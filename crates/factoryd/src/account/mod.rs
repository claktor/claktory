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

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use claktory_account::protocol::{
    self as api, CheckIn, CheckedIn, Endpoints, Ephemeral, ErrorBody, MoveDone, MoveRequest, Share, ShareRequest, SignInStart,
    SignInStarted, SignInVerify, SignedIn, WellKnown,
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
}

const ASKED: &str = "asked";

/// Where codes come from when the service doesn't say: claktory.com's.
const DEFAULT_CODE_FROM: &str = "code@auth.claktory.com";

impl Saved {
    fn signed_in(&self) -> bool {
        !self.machine_token.is_empty()
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
}

/// The account's state, held by `Factory`.
pub(crate) struct State {
    dir: PathBuf,
    inner: Mutex<Inner>,
    /// One sign-in, unlock or check-in at a time.
    busy: tokio::sync::Mutex<()>,
    /// Wakes `monitor`: something's due now.
    wake: Notify,
    http: reqwest::Client,
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

impl State {
    /// The account as `<home>/account` keeps it. With signing in on
    /// (`enabled`), the step is "asked", whatever it was; while it's off,
    /// nothing is written.
    pub(crate) fn new(home: &crate::home::Home, enabled: bool) -> State {
        let dir = home.root().join("account");
        let mut saved: Saved = match std::fs::read(dir.join("account.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                tracing::error!(%error, "account.json can't be read; signed out");
                Saved::default()
            }),
            Err(_) => Saved::default(),
        };
        let first = enabled && saved.step != ASKED;
        if first {
            saved.step = ASKED.to_owned();
        }
        let now = Instant::now();
        let inner = Inner {
            saved,
            pending: None,
            problem: String::new(),
            clock_offset: 0,
            next_unlock: now,
            unlock_tries: 0,
            unlock_stopped: false,
            share_asked: None,
            next_check_in: now,
            endpoints_sent: None,
            reseal_due: None,
        };
        let http = reqwest::Client::builder()
            .timeout(CALL_TIMEOUT)
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(format!("Claktory/{}", crate::update::VERSION))
            .build()
            .unwrap_or_default();
        let state = State { dir, inner: Mutex::new(inner), busy: tokio::sync::Mutex::new(()), wake: Notify::new(), http };
        if first {
            state.save();
        }
        state
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn save(&self) {
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
                let time = now_s() + self.lock().clock_offset;
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
        let (service, service_key, token) = {
            let inner = self.lock();
            (inner.saved.service.clone(), inner.saved.service_key.clone(), inner.saved.machine_token.clone())
        };
        let key = api::verifying_key(&service_key).ok_or_else(|| Failure::Unreachable("no service key kept".to_owned()))?;
        let (_, backend_key) = self.identity().map_err(|error| Failure::Unreachable(error.to_string()))?;
        self.call(&service, Some(&key), "POST", path, body, Some((&token, &backend_key))).await
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
    pub(crate) fn signed_in_as(&self, config: &AccountConfig) -> Option<(String, String, String)> {
        let inner = self.lock();
        let saved = &inner.saved;
        (config.enabled && saved.signed_in() && !saved.revoked)
            .then(|| (saved.account_id.clone(), saved.email.clone(), saved.service.clone()))
    }

    /// This Claktory's registration while it's signed in and not removed:
    /// the account, its Claktory id and the registration's epoch, which a
    /// sign-in again or a move changes (its direct-message leaf goes with it).
    pub(crate) fn registration(&self, config: &AccountConfig) -> Option<(String, String, u64)> {
        let inner = self.lock();
        let saved = &inner.saved;
        (config.enabled && saved.signed_in() && !saved.revoked).then(|| (saved.account_id.clone(), saved.claktory_id.clone(), saved.epoch))
    }

    /// What the app shows: None while signing in is off.
    pub(crate) fn view(&self, config: &AccountConfig) -> Option<v1::ClaktoryAccount> {
        if !config.enabled {
            return None;
        }
        let inner = self.lock();
        let saved = &inner.saved;
        let state = if inner.pending.is_some() {
            v1::ClaktoryAccountState::AwaitingCode
        } else if saved.signed_in() && saved.revoked {
            v1::ClaktoryAccountState::Revoked
        } else if saved.signed_in() {
            v1::ClaktoryAccountState::SignedIn
        } else {
            v1::ClaktoryAccountState::SignedOut
        };
        let (email, service) = match &inner.pending {
            Some(pending) => (pending.start.email.clone(), pending.service.clone()),
            None if saved.signed_in() => (saved.email.clone(), saved.service.clone()),
            None => (String::new(), configured_service(config)),
        };
        let name = if saved.name.is_empty() { computer_name() } else { saved.name.clone() };
        Some(v1::ClaktoryAccount {
            state: state.into(),
            email,
            service,
            name,
            last_contact_ms: saved.last_contact_ms,
            problem: inner.problem.clone(),
            code_expires_ms: inner.pending.as_ref().map_or(0, |pending| pending.expires_ms),
            last_sign_in_json: saved.last_sign_in_json.clone(),
            last_check_in_json: saved.last_check_in_json.clone(),
            last_sign_in_ms: saved.last_sign_in_ms,
            last_check_in_ms: saved.last_check_in_ms,
            // Every Claktory must sign in (#195): only an unreachable
            // service lets the app go on without it, for now.
            skippable: false,
            code_from: inner.pending.as_ref().map(|pending| pending.code_from.clone()).unwrap_or_default(),
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

fn create_private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
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
    let service = configured_service(&config);
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

/// Sends `start`, and waits for its code.
async fn send_code(
    factory: &Arc<Factory>,
    service: &str,
    service_key: &str,
    key_from: Option<i64>,
    verifying: &VerifyingKey,
    start: SignInStart,
) -> Result<(), RequestError> {
    let account = &factory.account;
    let started: SignInStarted = account
        .call(service, Some(verifying), "POST", "/v1/sign-in/start", &start, None)
        .await
        .map_err(|failure| sign_in_error(service, failure))?;
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

/// AccountVerify: signs in with the emailed code, and protects the vault
/// with the account.
pub(crate) async fn verify(factory: &Arc<Factory>, code: &str) -> Result<(), RequestError> {
    enabled(factory)?;
    let account = &factory.account;
    let _busy = account.busy.lock().await;
    let Some(pending) = account.lock().pending.clone() else {
        return Err(refused(ErrorCode::InvalidRequest, "Enter your email address first."));
    };
    let Some(verifying) = api::verifying_key(&pending.service_key) else {
        return Err(refused(ErrorCode::Internal, "The service's key is unreadable."));
    };
    let code: String = code.chars().filter(|character| !character.is_whitespace() && *character != '-').collect();
    let ephemeral = Ephemeral::new();
    let asked = SignInVerify { request_id: pending.request_id.clone(), code, eph_pub: ephemeral.public_b64() };
    // Signed with the backend key, which proves to the service that it's
    // this Claktory: signed out of another account, it moves to this one
    // with this code, rather than being turned away as taken.
    let (_, key) = account.identity()?;
    let signer = Some(("", &key));
    let signed_in: SignedIn = match account.call(&pending.service, Some(&verifying), "POST", "/v1/sign-in/verify", &asked, signer).await {
        Ok(signed_in) => signed_in,
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
        inner.saved.name = pending.start.name.clone();
        inner.saved.revoked = false;
        inner.saved.signed_in_ms = now_ms();
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
    } else if factory.vault.exists() {
        account.set_problem("Your secrets stay locked until you open them with the recovery key.");
    }
    factory.vault_changed();
    account.wake.notify_one();
    Ok(())
}

/// AccountSignOut: no more calls to the service. An open vault opens by
/// itself again, with a key on disk.
pub(crate) async fn sign_out(factory: &Arc<Factory>) -> Result<(), RequestError> {
    enabled(factory)?;
    let account = &factory.account;
    let _busy = account.busy.lock().await;
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
    }
    account.save();
    tracing::info!("signed out of the account service");
    factory.vault_changed();
    Ok(())
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
            reseal(factory, checked.epoch).await;
            register_endpoints(factory).await;
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
    changed(factory);
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
        account.lock().saved.machine_token = pair.token.clone();
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

/// Runs the account while Claktory runs: after a restart, opens the vault
/// with the service's share (trying again while it can't be reached); checks
/// in at start and every hour.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    loop {
        let Some(strong) = factory.upgrade() else { return };
        let wait = tick(&strong).await;
        let woken = async {
            strong.account.wake.notified().await;
        };
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = woken => {}
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
    let (signed_in, revoked) = {
        let inner = account.lock();
        (inner.saved.signed_in(), inner.saved.revoked)
    };
    if !signed_in || revoked {
        return IDLE;
    }
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
    next.max(Duration::from_millis(100))
}

#[cfg(test)]
mod tests {
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
