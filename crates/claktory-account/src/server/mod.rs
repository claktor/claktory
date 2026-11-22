//! The service: an HTTP server over one SQLite file and one secrets file.

mod api;
mod apps;
pub mod db;
mod devices;
pub mod geo;
pub mod mail;
mod pipes;
mod reports;
pub mod review;
pub mod secrets;
pub mod statement;
pub mod stats;
mod web;

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt as _, Full, Limited};
use hyper::body::Incoming;
use hyper::header::{HeaderMap, HeaderName, HeaderValue};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use rusqlite::Connection;
use serde::Serialize;

use crate::protocol::{self, ErrorBody};
pub use mail::{Mail, MailError, Mailer};
pub use secrets::{Rotate, Secrets};

/// The time, in unix seconds; tests set their own.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

pub fn system_clock() -> Clock {
    Arc::new(|| jiff::Timestamp::now().as_second())
}

/// Bodies up to this many bytes: a check-in after a week offline carries
/// a week of periods.
const BODY_LIMIT: usize = 1024 * 1024;
/// How long the public stats are kept before they're counted again.
const PUBLIC_STATS_FOR: i64 = 60;
/// Requests an IP may make a minute.
const IP_PER_MINUTE: u32 = 30;

/// Whether a route is for requests signed by a Claktory's backend key or a
/// device's account key, which every one of its handlers checks first.
/// The rest (sign-in, codes, web pages, public answers) are limited by
/// address.
fn signed(method: &Method, path: &str) -> bool {
    matches!(
        (method, path),
        (
            &Method::POST,
            "/v1/share"
                | "/v1/check-in"
                | "/v1/move"
                | "/v1/devices/sign-out"
                | "/v1/devices/remove"
                | "/v1/devices/assertion"
                | "/v1/pipe-identity"
        ) | (&Method::PUT, "/v1/endpoints")
            | (&Method::GET, "/v1/devices/me" | "/v1/claktories")
    ) || path.starts_with("/v1/apps/")
}

pub struct Config {
    /// Where the account page is, as emails write it.
    pub account_page: String,
    /// Where the service is. Emails no longer load its mark (`/mark.png`,
    /// still served for those sent before); kept so `--public-url` still
    /// starts it.
    pub public_url: String,
    /// The domain emails come from: code@ and notices@ it.
    pub mail_domain: String,
    /// Behind a proxy (Caddy): the client's IP is the last of
    /// X-Forwarded-For, and cookies are Secure.
    pub trust_proxy: bool,
    /// Emails a day (UTC) at most, below the mail plan's quota. Sign-in
    /// codes stop at it; unlock notices always go.
    pub email_daily_cap: u32,
    /// Who runs the service: told when 80% of the day's email is used, and
    /// shown the usage counts at /account/admin once signed in.
    pub admins: Vec<String>,
    /// Finds a Claktory's place from its IP.
    pub locator: Option<Arc<dyn geo::Locator>>,
    /// Relays Claktories should use, in check-in answers.
    pub relays: Vec<String>,
    /// An offline root's word for the service key, served at well-known
    /// (`--service-key-statement`).
    pub service_key_statement: Option<protocol::SignedStatement>,
    /// The user-apps worker: its socket (`--user-apps`) and its user
    /// (`--user-apps-uid`), which whatever listens there must run as.
    /// Without one, app routes answer 503.
    pub user_apps: Option<UserApps>,
    /// The claktory.com addresses `POST /v1/pipe-identity` may vouch for
    /// (`--vouch-claktory-email`); none by default, since "Verified" with
    /// one reads as Claktory's staff and a shared inbox (QA's) can sign in.
    pub vouched_claktory_emails: Vec<String>,
    /// The store reviewers' account (`--review-account`): its email signs
    /// in with a fixed code, and is sent nothing. Off by default.
    pub review_account: Option<review::ReviewAccount>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            account_page: "claktory.com/account".to_owned(),
            public_url: "https://account.claktory.com".to_owned(),
            mail_domain: "auth.claktory.com".to_owned(),
            trust_proxy: false,
            email_daily_cap: 90,
            admins: Vec::new(),
            locator: None,
            relays: Vec::new(),
            service_key_statement: None,
            user_apps: None,
            vouched_claktory_emails: Vec::new(),
            review_account: None,
        }
    }
}

/// Where the user-apps worker listens, and as whom.
#[derive(Clone, Debug)]
pub struct UserApps {
    pub socket: std::path::PathBuf,
    pub uid: u32,
}

pub struct Service {
    config: Config,
    secrets: Secrets,
    mailer: Arc<dyn Mailer>,
    clock: Clock,
    db: Mutex<Connection>,
    /// Signatures of signed requests seen in the last `MAX_SKEW`s, with
    /// when each can be forgotten.
    seen: Mutex<HashMap<String, i64>>,
    /// The public stats, and when they were counted.
    public_stats: Mutex<Option<(i64, protocol::PublicStats)>>,
}

impl Service {
    pub fn new(db: &Path, secrets: Secrets, config: Config, mailer: Arc<dyn Mailer>, clock: Clock) -> Result<Arc<Service>, String> {
        let conn = db::open(db).map_err(|error| format!("can't open {}: {error}", db.display()))?;
        Ok(Arc::new(Service {
            config,
            secrets,
            mailer,
            clock,
            db: Mutex::new(conn),
            seen: Mutex::default(),
            public_stats: Mutex::default(),
        }))
    }

    /// The service's public key, base64url, as Claktories pin it.
    pub fn service_key(&self) -> String {
        protocol::b64(self.secrets.signing_key.verifying_key().to_bytes())
    }

    fn now(&self) -> i64 {
        (self.clock)()
    }

    fn db(&self) -> MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Sends an email, counting it toward the day's cap and telling the
    /// admin once 80% of it is used. `capped` mail (sign-in codes) isn't
    /// sent at the cap.
    async fn send(&self, mail: &Mail, capped: bool) -> Result<(), MailError> {
        if self.review(&mail.to).is_some() {
            // No mailbox: nothing goes to it, and its email is never logged.
            tracing::info!("review account: an email to it isn't sent");
            return Ok(());
        }
        let now = self.now();
        let cap = self.config.email_daily_cap;
        let sent = {
            let conn = self.db();
            let failed = |error: rusqlite::Error| MailError::NotSent(error.to_string());
            if capped && db::today(&conn, "mail-day", now).map_err(failed)? >= cap {
                tracing::warn!("the day's email cap ({cap}) is reached: no sign-in code sent");
                return Err(MailError::NotSent("the day's email cap is reached".to_owned()));
            }
            db::count_today(&conn, "mail-day", now).map_err(failed)?
        };
        let result = self.mailer.send(mail).await;
        match &result {
            Ok(()) => {}
            Err(MailError::NotSent(error)) => tracing::error!("email not sent: {error}"),
            Err(MailError::Refused { kind, text }) => {
                tracing::error!("email not sent: the mail provider refused ({kind}): {text}");
                // Counted for the usage report, and told to whoever runs it
                // once a day (it may not arrive: the log has it too).
                let first = db::count_today(&self.db(), &format!("mail-refused:{kind}"), now).is_ok_and(|count| count == 1);
                if first {
                    for admin in &self.config.admins {
                        let alert = mail::refused_alert_mail(&self.config.mail_domain, admin, text);
                        if let Err(error) = self.mailer.send(&alert).await {
                            tracing::error!("the alert that email is refused wasn't sent either: {error}");
                        }
                    }
                }
            }
        }
        if sent == cap.saturating_mul(4).div_ceil(5) {
            tracing::warn!("{sent} of the day's {cap} emails are used");
            for admin in &self.config.admins {
                let alert = mail::cap_alert_mail(&self.config.mail_domain, admin, sent, cap);
                if let Err(error) = self.mailer.send(&alert).await {
                    tracing::error!("the email cap alert wasn't sent: {error}");
                }
            }
        }
        result
    }

    /// Forgets what has expired, hourly.
    pub fn spawn_upkeep(self: &Arc<Self>) {
        let service = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut every = tokio::time::interval(Duration::from_secs(3600));
            loop {
                every.tick().await;
                let Some(service) = service.upgrade() else { return };
                if let Err(error) = db::purge(&service.db(), service.now()) {
                    tracing::error!("upkeep: {error}");
                }
            }
        });
    }

    /// Asks the user-apps worker every minute to purge the deleted accounts
    /// it hasn't yet (it was down when they were deleted, say).
    pub fn spawn_app_purges(self: &Arc<Self>) {
        if self.config.user_apps.is_none() {
            return;
        }
        let service = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut every = tokio::time::interval(Duration::from_secs(60));
            loop {
                every.tick().await;
                let Some(service) = service.upgrade() else { return };
                apps::retry_purges(&service).await;
            }
        });
    }

    /// Purges, now, the deleted accounts due to be purged from the
    /// user-level apps: how many were. For tests; `spawn_app_purges` does it
    /// every minute.
    pub async fn retry_app_purges(&self) -> usize {
        apps::retry_purges(self).await
    }

    /// The review account, when `email` (normalized) is its.
    fn review(&self, email: &str) -> Option<&review::ReviewAccount> {
        self.config.review_account.as_ref().filter(|review| review.is(email))
    }

    /// Where a request came from, when the service has a GeoIP file and
    /// the IP is a public one.
    fn locate(&self, ip: &str) -> Option<geo::Location> {
        let ip: IpAddr = ip.parse().ok()?;
        self.config.locator.as_ref().filter(|_| geo::public(ip))?.locate(ip)
    }

    /// The public stats, counted at most once a minute.
    fn public_stats(&self) -> Result<protocol::PublicStats, ApiError> {
        let now = self.now();
        let mut cached = self.public_stats.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((at, stats)) = cached.as_ref()
            && now - at < PUBLIC_STATS_FOR
        {
            return Ok(stats.clone());
        }
        let stats = db::public_stats(&self.db())?;
        *cached = Some((now, stats.clone()));
        Ok(stats)
    }

    fn client_ip(&self, headers: &HeaderMap, peer: IpAddr) -> String {
        if self.config.trust_proxy
            && let Some(forwarded) = headers.get("x-forwarded-for").and_then(|value| value.to_str().ok())
            && let Some(last) = forwarded.rsplit(',').next().map(str::trim).filter(|ip| ip.parse::<IpAddr>().is_ok())
        {
            return last.to_owned();
        }
        peer.to_string()
    }

    async fn handle(self: Arc<Self>, request: Request<Incoming>, peer: IpAddr) -> Response<Full<Bytes>> {
        let ip = self.client_ip(request.headers(), peer);
        let (parts, body) = request.into_parts();
        let path_and_query = parts.uri.path_and_query().map_or("/", |path| path.as_str()).to_owned();
        let body = match Limited::new(body, BODY_LIMIT).collect().await {
            Ok(body) => body.to_bytes(),
            Err(_) => Bytes::new(),
        };
        let call = Call {
            method: parts.method,
            path: parts.uri.path().to_owned(),
            query: parts.uri.query().unwrap_or_default().to_owned(),
            path_and_query,
            headers: parts.headers,
            body,
            ip,
            verified: AtomicBool::new(false),
        };
        let reply = match self.route(&call).await {
            Ok(reply) => reply,
            Err(error) => error.reply(),
        };
        self.respond(&call, reply)
    }

    async fn route(&self, call: &Call) -> Result<Reply, ApiError> {
        let relay = call.path == "/v1/relay/allowlist";
        // The mark is fetched by mail providers' image proxies, from few
        // addresses for many people's emails.
        if call.path == "/mark.png" && matches!(call.method, Method::GET | Method::HEAD) {
            return Ok(mark());
        }
        // A signed request is limited by its key once it's verified
        // (Service::verified), not by its address: a person's Claktory and
        // devices, and a household or a carrier's NAT behind one address,
        // share one. Its address counts only the requests that don't
        // verify, refused or never checked, and once those are spent,
        // refuses the rest before checking them, except those naming a
        // Claktory or device it knows: someone else's bad requests on the
        // address mustn't lock a person's phone out.
        // A report of an AI reply is signed when its Claktory is signed in,
        // and unsigned when it isn't.
        let report = call.method == Method::POST && call.path == protocol::AI_REPORT_PATH;
        let signed = signed(&call.method, &call.path) || (report && call.header("authorization").is_some());
        let unverified = format!("ip-unverified:{}", call.ip);
        if signed {
            let spent = db::spent(&self.db(), &[(&unverified, 60, IP_PER_MINUTE)], self.now())?;
            if let Some(wait) = spent
                && !self.names_known_key(call)?
            {
                return Err(ApiError::rate_limited(wait));
            }
        } else if !relay {
            let key = format!("ip-minute:{}", call.ip);
            self.limit(&[(&key, 60, IP_PER_MINUTE)])?;
        }
        let result = self.dispatch(call).await;
        if signed && !call.verified.load(Ordering::Relaxed) {
            // Full meanwhile, it's full: nothing more to count.
            let _ = db::take(&self.db(), &[(&unverified, 60, IP_PER_MINUTE)], self.now())?;
        }
        result
    }

    async fn dispatch(&self, call: &Call) -> Result<Reply, ApiError> {
        match (&call.method, call.path.as_str()) {
            (&Method::GET, protocol::WELL_KNOWN) => api::well_known(self),
            (&Method::POST, "/v1/sign-in/start") => api::sign_in_start(self, call).await,
            (&Method::POST, "/v1/sign-in/verify") => api::sign_in_verify(self, call),
            (&Method::POST, "/v1/share") => api::share(self, call).await,
            (&Method::POST, "/v1/check-in") => api::check_in(self, call),
            (&Method::PUT, "/v1/endpoints") => api::endpoints(self, call),
            (&Method::POST, "/v1/move") => api::move_claktory(self, call).await,
            (&Method::POST, "/v1/devices/sign-in/start") => devices::sign_in_start(self, call).await,
            (&Method::POST, "/v1/devices/sign-in/verify") => devices::sign_in_verify(self, call),
            (&Method::GET, "/v1/devices/me") => devices::me(self, call),
            (&Method::POST, "/v1/devices/sign-out") => devices::sign_out(self, call),
            (&Method::POST, "/v1/devices/remove") => devices::remove(self, call),
            (&Method::GET, "/v1/claktories") => devices::claktories(self, call),
            (&Method::POST, "/v1/devices/assertion") => devices::assertion(self, call),
            (&Method::POST, "/v1/pipe-identity") => pipes::identity(self, call),
            (&Method::POST, protocol::AI_REPORT_PATH) => reports::report(self, call),
            (&Method::GET, "/v1/relay/allowlist") => api::allowlist(self, call),
            (&Method::GET, "/v1/stats/public") => api::public_stats(self),
            (_, path) if path.starts_with("/v1/apps/") => apps::call(self, call).await,
            (&Method::GET | &Method::HEAD, "/") => Ok(web::root()),
            (_, path) if path == "/account" || path.starts_with("/account/") => Ok(web::page(self, call).await),
            _ => Err(ApiError::new(StatusCode::NOT_FOUND, "not_found", "There's nothing here.")),
        }
    }

    /// Whether a request names a Claktory (by its token) or a signed-in
    /// device (by its account key) that's known here, checked or not.
    fn names_known_key(&self, call: &Call) -> Result<bool, ApiError> {
        let Some(authorization) = call.header("authorization") else { return Ok(false) };
        if let Some(token) = authorization.strip_prefix("Bearer ") {
            return Ok(db::claktory_by_token(&self.db(), token.trim())?.is_some());
        }
        match authorization.strip_prefix(protocol::DEVICE_AUTH).and_then(|value| value.strip_prefix(' ')) {
            Some(account_key) => Ok(db::device(&self.db(), account_key.trim())?.is_some()),
            None => Ok(false),
        }
    }

    /// Marks a request as from a Claktory or a signed-in device, its
    /// signature checked by the key it's known by, and counts it against
    /// that key's requests a minute instead of its address's.
    fn verified(&self, call: &Call, key: &str) -> Result<(), ApiError> {
        call.verified.store(true, Ordering::Relaxed);
        self.limit(&[(&format!("key-minute:{key}"), 60, api::KEY_PER_MINUTE)])
    }

    /// Counts against limits, or refuses with when to try again.
    fn limit(&self, limits: &[(&str, i64, u32)]) -> Result<(), ApiError> {
        match db::take(&self.db(), limits, self.now())? {
            Ok(()) => Ok(()),
            Err(wait) => Err(ApiError::rate_limited(wait)),
        }
    }

    /// The reply, signed by the service key for the request it answers.
    fn respond(&self, call: &Call, reply: Reply) -> Response<Full<Bytes>> {
        use ed25519_dalek::Signer as _;
        let message =
            protocol::response_message(reply.status.as_u16(), call.method.as_str(), &call.path_and_query, &call.body, &reply.body);
        let signature = protocol::b64(self.secrets.signing_key.sign(&message).to_bytes());
        let mut response = Response::new(Full::new(Bytes::from(reply.body)));
        *response.status_mut() = reply.status;
        let headers = response.headers_mut();
        headers.insert(hyper::header::CONTENT_TYPE, HeaderValue::from_static(reply.content_type));
        headers.insert(hyper::header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        headers.insert(HeaderName::from_static("x-content-type-options"), HeaderValue::from_static("nosniff"));
        if let Ok(value) = HeaderValue::from_str(&signature) {
            headers.insert(HeaderName::from_static("claktory-signature"), value);
        }
        for (name, value) in reply.headers {
            if name == hyper::header::CACHE_CONTROL {
                headers.insert(name, value);
            } else {
                headers.append(name, value);
            }
        }
        response
    }
}

/// Claktory's mark (the app's icon, 128px), as the sign-in emails show it.
const MARK: &[u8] = include_bytes!("../../assets/mark.png");

fn mark() -> Reply {
    let mut reply = Reply { status: StatusCode::OK, content_type: "image/png", headers: Vec::new(), body: MARK.to_vec() };
    reply.headers.push((hyper::header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=604800")));
    reply
}

/// Serves until the listener fails.
pub async fn serve(listener: tokio::net::TcpListener, service: Arc<Service>) -> std::io::Result<()> {
    loop {
        let (stream, peer): (_, SocketAddr) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                // Out of file descriptors, say: wait rather than spin.
                tracing::warn!("accept: {error}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let service = service.clone();
        tokio::spawn(async move {
            let handler = hyper::service::service_fn(move |request| {
                let service = service.clone();
                async move { Ok::<_, Infallible>(service.handle(request, peer.ip()).await) }
            });
            let connection = hyper::server::conn::http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(Duration::from_secs(20))
                .serve_connection(TokioIo::new(stream), handler);
            if let Err(error) = connection.await {
                tracing::debug!("connection from {peer}: {error}");
            }
        });
    }
}

/// A request, read whole.
pub(crate) struct Call {
    method: Method,
    path: String,
    query: String,
    path_and_query: String,
    headers: HeaderMap,
    body: Bytes,
    ip: String,
    /// Set once the request is known to be from a Claktory or a signed-in
    /// device (Service::verified).
    verified: AtomicBool,
}

impl Call {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, ApiError> {
        serde_json::from_slice(&self.body).map_err(|_| ApiError::invalid("The request isn't what this endpoint takes."))
    }
}

pub(crate) struct Reply {
    status: StatusCode,
    content_type: &'static str,
    headers: Vec<(HeaderName, HeaderValue)>,
    body: Vec<u8>,
}

impl Reply {
    fn json(value: &impl Serialize) -> Reply {
        Reply::json_status(StatusCode::OK, value)
    }

    fn json_status(status: StatusCode, value: &impl Serialize) -> Reply {
        let body = serde_json::to_vec(value).unwrap_or_default();
        Reply { status, content_type: "application/json", headers: Vec::new(), body }
    }
}

/// An error as the API answers it.
#[derive(Debug)]
pub(crate) struct ApiError {
    status: StatusCode,
    body: ErrorBody,
}

impl ApiError {
    fn new(status: StatusCode, code: &str, message: &str) -> ApiError {
        ApiError {
            status,
            body: ErrorBody {
                error: code.to_owned(),
                message: message.to_owned(),
                attempts_left: None,
                retry_after: None,
                server_time: None,
            },
        }
    }

    fn invalid(message: &str) -> ApiError {
        ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }

    fn unauthorized() -> ApiError {
        ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized", "This Claktory isn't signed in. Sign in again.")
    }

    fn rate_limited(wait: i64) -> ApiError {
        let mut error = ApiError::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", "Too many tries. Try again later.");
        error.body.retry_after = Some(wait.max(1));
        error
    }

    fn email_unavailable() -> ApiError {
        ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "email_unavailable", "The code can't be sent right now. Try again later.")
    }

    fn reply(self) -> Reply {
        let mut reply = Reply::json_status(self.status, &self.body);
        if let Some(wait) = self.body.retry_after {
            reply.headers.push((hyper::header::RETRY_AFTER, HeaderValue::from(wait)));
        }
        reply
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(error: rusqlite::Error) -> ApiError {
        tracing::error!("database: {error}");
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "Something went wrong. Try again later.")
    }
}
