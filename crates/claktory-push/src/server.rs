//! The HTTP service: register, push, unregister, health.

use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use base64::Engine;
use bytes::Bytes;
use http_body_util::{BodyExt as _, Full, Limited};
use hyper::body::Incoming;
use hyper::header::{HeaderMap, HeaderValue};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::limits::Limits;
use crate::{Delivery, MAX_CIPHERTEXT, MAX_TTL, Platform, Push, Transport, Urgency, db};

/// The time, in unix seconds; tests set their own.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Requests are small.
const BODY_LIMIT: usize = 16 * 1024;
/// The longest push token Apple or Google gives, with room to spare.
const MAX_TOKEN: usize = 4096;
/// The longest collapse or thread id: APNs's collapse-id limit.
const MAX_ID: usize = 64;
const HOUR: i64 = 3600;

/// How much each may do.
#[derive(Clone, Debug)]
pub struct Rates {
    /// Registrations from one IP an hour.
    pub register_per_ip: u32,
    /// Registrations an hour, from everyone.
    pub register_total: u32,
    /// Registrations of one push token an hour, from wherever.
    pub register_per_token: u32,
    /// Pushes to one handle a minute, and an hour.
    pub handle_per_minute: u32,
    pub handle_per_hour: u32,
    /// Pushes to one device a minute, and an hour, through all its handles
    /// (one for each Claktory it gets notifications from).
    pub token_per_minute: u32,
    pub token_per_hour: u32,
    /// Pushes from one IP an hour (a Claktory sends from one).
    pub push_per_ip: u32,
    /// Pushes an hour, from everyone: past it, something's wrong, and
    /// Apple's key is worth more than an hour's notifications.
    pub push_total: u32,
}

impl Default for Rates {
    fn default() -> Rates {
        Rates {
            register_per_ip: 20,
            register_total: 2000,
            register_per_token: 10,
            handle_per_minute: 10,
            handle_per_hour: 60,
            token_per_minute: 20,
            token_per_hour: 120,
            push_per_ip: 1000,
            push_total: 50_000,
        }
    }
}

#[derive(Default)]
pub struct Config {
    /// Behind a TLS proxy (Caddy): the client's IP is the last of
    /// X-Forwarded-For.
    pub trust_proxy: bool,
    pub rates: Rates,
}

pub struct Service {
    config: Config,
    transport: Arc<dyn Transport>,
    clock: Clock,
    db: Mutex<Connection>,
    limits: Limits,
}

impl Service {
    pub fn new(db: &Path, transport: Arc<dyn Transport>, config: Config, clock: Clock) -> Result<Arc<Service>, String> {
        let conn = db::open(db).map_err(|error| format!("can't open {}: {error}", db.display()))?;
        Ok(Arc::new(Service { config, transport, clock, db: Mutex::new(conn), limits: Limits::default() }))
    }

    fn now(&self) -> i64 {
        (self.clock)()
    }

    fn db(&self) -> MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Forgets unused handles and old rate windows, hourly.
    pub fn spawn_upkeep(self: &Arc<Self>) {
        let service = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut every = tokio::time::interval(Duration::from_secs(3600));
            loop {
                every.tick().await;
                let Some(service) = service.upgrade() else { return };
                let now = service.now();
                service.limits.purge(now);
                match db::purge(&service.db(), now) {
                    Ok(0) => {}
                    Ok(purged) => tracing::info!(purged, "forgot unused handles"),
                    Err(error) => tracing::error!("upkeep: {error}"),
                }
            }
        });
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
        let body = match Limited::new(body, BODY_LIMIT).collect().await {
            Ok(body) => body.to_bytes(),
            Err(_) => return reply(Err(ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "too_large", "The request is too large."))),
        };
        let result = match (&parts.method, parts.uri.path()) {
            (&Method::POST, "/v1/register") => self.register(&body, &ip),
            (&Method::POST, "/v1/push") => self.push(&body, &ip).await,
            (&Method::POST, "/v1/unregister") => self.unregister(&body, &ip),
            (&Method::GET, "/v1/health") => self.health(),
            _ => Err(ApiError::new(StatusCode::NOT_FOUND, "not_found", "There's nothing here.")),
        };
        reply(result)
    }

    fn limit(&self, limits: &[(&str, i64, u32)]) -> Result<(), ApiError> {
        self.limits.take(limits, self.now()).map_err(ApiError::rate_limited)
    }

    fn register(&self, body: &[u8], ip: &str) -> Result<Value, ApiError> {
        // Fields it doesn't take are refused: no text gets through.
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Register {
            platform: String,
            token: String,
        }
        let request: Register = json_of(body)?;
        let platform = Platform::parse(&request.platform).ok_or(ApiError::invalid("platform is apns, apns_sandbox or fcm."))?;
        let token = request.token.trim();
        if token.is_empty() || token.len() > MAX_TOKEN || !token.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(ApiError::invalid("That isn't a push token."));
        }
        let (apns, fcm) = self.transport.platforms();
        let served = match platform {
            Platform::Apns | Platform::ApnsSandbox => apns,
            Platform::Fcm => fcm,
        };
        if !served {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, "unsupported", "This relay can't push to that platform."));
        }
        let rates = &self.config.rates;
        self.limit(&[
            (&format!("register-ip:{ip}"), HOUR, rates.register_per_ip),
            (&format!("register-token:{}", db::hash(token)), HOUR, rates.register_per_token),
            ("register", HOUR, rates.register_total),
        ])?;
        let handle = db::register(&self.db(), platform, token, self.now())?;
        tracing::info!(platform = platform.as_str(), "registered a device");
        Ok(json!({"handle": handle}))
    }

    async fn push(&self, body: &[u8], ip: &str) -> Result<Value, ApiError> {
        // Fields it doesn't take are refused: no text gets through.
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct PushRequest {
            handle: String,
            ciphertext: String,
            #[serde(default)]
            urgency: String,
            #[serde(default)]
            collapse: String,
            #[serde(default)]
            thread: String,
            #[serde(default)]
            ttl: Option<u64>,
        }
        let request: PushRequest = json_of(body)?;
        let urgency = match request.urgency.as_str() {
            "" | "normal" => Urgency::Normal,
            "urgent" => Urgency::Urgent,
            "low" => Urgency::Low,
            _ => return Err(ApiError::invalid("urgency is normal, urgent or low.")),
        };
        let decoded = base64::engine::general_purpose::STANDARD.decode(&request.ciphertext).ok();
        if decoded.is_none_or(|bytes| bytes.is_empty() || bytes.len() > MAX_CIPHERTEXT) {
            return Err(ApiError::invalid("ciphertext is base64, at most 2200 bytes."));
        }
        let opaque = |id: &str| id.len() <= MAX_ID && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
        if !opaque(&request.collapse) || !opaque(&request.thread) {
            return Err(ApiError::invalid("collapse and thread are base64url, at most 64 characters."));
        }
        let handle = request.handle.trim();
        let rates = &self.config.rates;
        let per_handle = format!("handle:{}", db::hash(handle));
        let per_minute = format!("handle-minute:{}", db::hash(handle));
        self.limit(&[(&format!("push-ip:{ip}"), HOUR, rates.push_per_ip)])?;
        let Some((platform, token)) = db::lookup(&self.db(), handle, self.now())? else {
            return Err(ApiError::new(StatusCode::NOT_FOUND, "unknown_handle", "There's no such handle. Register the device again."));
        };
        let device = db::hash(&token);
        self.limit(&[
            (&per_minute, 60, rates.handle_per_minute),
            (&per_handle, HOUR, rates.handle_per_hour),
            (&format!("token-minute:{device}"), 60, rates.token_per_minute),
            (&format!("token:{device}"), HOUR, rates.token_per_hour),
        ])?;
        if let Err(error) = self.limit(&[("push", HOUR, rates.push_total)]) {
            tracing::error!("the hour's push limit for everyone is reached");
            return Err(error);
        }
        let push = Push {
            ciphertext: request.ciphertext,
            urgency,
            collapse: request.collapse,
            thread: request.thread,
            ttl: request.ttl.unwrap_or(24 * 3600).min(MAX_TTL),
        };
        match self.transport.send(platform, &token, &push).await {
            Delivery::Sent => Ok(json!({})),
            Delivery::Gone => {
                let forgotten = db::forget_token(&self.db(), &token)?;
                tracing::info!(platform = platform.as_str(), forgotten, "a device's token is no longer valid");
                Err(ApiError::new(StatusCode::GONE, "gone", "The device no longer takes pushes. Register it again."))
            }
            Delivery::Transient(why) => {
                tracing::warn!(platform = platform.as_str(), "push not sent for now: {why}");
                Err(ApiError::new(StatusCode::BAD_GATEWAY, "unavailable", "The push couldn't be sent now. Try again later."))
            }
            Delivery::Refused(why) => {
                tracing::error!(platform = platform.as_str(), "push refused: {why}");
                Err(ApiError::new(StatusCode::BAD_GATEWAY, "refused", "The push was refused."))
            }
        }
    }

    fn unregister(&self, body: &[u8], ip: &str) -> Result<Value, ApiError> {
        // Fields it doesn't take are refused: no text gets through.
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Unregister {
            handle: String,
        }
        let request: Unregister = json_of(body)?;
        self.limit(&[(&format!("unregister-ip:{ip}"), HOUR, 60)])?;
        db::unregister(&self.db(), request.handle.trim())?;
        Ok(json!({}))
    }

    fn health(&self) -> Result<Value, ApiError> {
        let (apns, fcm) = self.transport.platforms();
        db::count(&self.db())?;
        Ok(json!({"ok": true, "apns": apns, "fcm": fcm}))
    }
}

fn json_of<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, ApiError> {
    serde_json::from_slice(body).map_err(|_| ApiError::invalid("The request isn't what this endpoint takes."))
}

fn reply(result: Result<Value, ApiError>) -> Response<Full<Bytes>> {
    let (status, body, retry) = match result {
        Ok(body) => (StatusCode::OK, body, None),
        Err(error) => {
            let mut body = json!({"error": error.code, "message": error.message});
            if let Some(wait) = error.retry_after {
                body["retry_after"] = wait.into();
            }
            (error.status, body, error.retry_after)
        }
    };
    let mut response = Response::new(Full::new(Bytes::from(body.to_string())));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(hyper::header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(hyper::header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Some(wait) = retry {
        headers.insert(hyper::header::RETRY_AFTER, HeaderValue::from(wait));
    }
    response
}

/// Serves until the listener fails.
pub async fn serve(listener: tokio::net::TcpListener, service: Arc<Service>) -> std::io::Result<()> {
    loop {
        let (stream, peer): (_, SocketAddr) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
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

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    retry_after: Option<i64>,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: &'static str) -> ApiError {
        ApiError { status, code, message, retry_after: None }
    }

    fn invalid(message: &'static str) -> ApiError {
        ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }

    fn rate_limited(wait: i64) -> ApiError {
        ApiError {
            retry_after: Some(wait.max(1)),
            ..ApiError::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", "Too many requests. Try again later.")
        }
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(error: rusqlite::Error) -> ApiError {
        tracing::error!("database: {error}");
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "Something went wrong. Try again later.")
    }
}
