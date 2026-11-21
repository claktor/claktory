//! The API a Claktory's backend uses (API.md).

use hyper::StatusCode;

use super::{ApiError, Call, Reply, Service, db, mail};
use crate::protocol::{self, *};

/// Sign-ins an IP may start a day.
pub const SIGN_INS_PER_IP: u32 = 30;

/// What a sign-in may make of the limits: per email 5 an hour and 20 a day,
/// per IP 30 a day (people behind one address, at a meetup or an office,
/// sign in together).
pub(super) fn sign_in_limits(service: &Service, email: &str, ip: &str) -> Result<(), ApiError> {
    let hour = db::email_key("email-hour", email);
    let day = db::email_key("email-day", email);
    let ip = format!("ip-sign-in:{ip}");
    service.limit(&[(&hour, 3600, 5), (&day, 86_400, 20), (&ip, 86_400, SIGN_INS_PER_IP)])
}

/// Six random digits, every one as likely.
pub(super) fn new_code() -> String {
    loop {
        let mut bytes = [0u8; 4];
        getrandom::fill(&mut bytes).expect("the system's random source");
        let number = u32::from_le_bytes(bytes);
        // The largest multiple of a million below 2^32, so none is likelier.
        if number < 4_294_000_000 {
            return format!("{:06}", number % 1_000_000);
        }
    }
}

/// Records a sign-in and emails its code; the request's id.
pub(super) async fn start(service: &Service, mut request: db::Request, code: &str) -> Result<String, ApiError> {
    request.id = b64(&random_32()[..16]);
    request.code_hash = db::code_hash(code, &request.id);
    request.expires = service.now() + CODE_LIFETIME;
    db::insert_request(&service.db(), &request)?;
    let mail = match (request.kind.as_str(), &request.name) {
        ("device", Some(device)) => {
            mail::device_code_mail(&service.config.mail_domain, &service.config.public_url, &request.email, code, device)
        }
        _ => mail::code_mail(&service.config.mail_domain, &service.config.public_url, &request.email, code),
    };
    if service.send(&mail, true).await.is_err() {
        db::delete_request(&service.db(), &request.id)?;
        return Err(ApiError::email_unavailable());
    }
    Ok(request.id)
}

/// The request a code is right for, of `kind`; counts a wrong one.
pub(super) fn check_code(service: &Service, request_id: &str, code: &str, kind: &str) -> Result<db::Request, ApiError> {
    let conn = service.db();
    let expired = || ApiError::new(StatusCode::GONE, "code_expired", "This code has expired. Ask for a new one.");
    let Some(request) = db::request(&conn, request_id)?.filter(|request| request.kind == kind) else { return Err(expired()) };
    if request.expires <= service.now() || request.attempts >= CODE_TRIES {
        db::delete_request(&conn, &request.id)?;
        return Err(expired());
    }
    if db::code_hash(code.trim(), &request.id) != request.code_hash {
        db::count_attempt(&conn, &request.id)?;
        let left = CODE_TRIES - request.attempts - 1;
        if left == 0 {
            db::delete_request(&conn, &request.id)?;
        }
        let mut error = ApiError::new(StatusCode::BAD_REQUEST, "wrong_code", "That code isn't right.");
        error.body.attempts_left = Some(left);
        return Err(error);
    }
    Ok(request)
}

pub(super) fn well_known(service: &Service) -> Result<Reply, ApiError> {
    Ok(Reply::json(&WellKnown {
        service: "claktory-account".to_owned(),
        api: API,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        service_key: service.service_key(),
        service_key_statement: service.config.service_key_statement.clone(),
    }))
}

pub(super) async fn sign_in_start(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let body: SignInStart = call.json()?;
    let email = normalize_email(&body.email)
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "invalid_email", "That isn't an email address."))?;
    if !valid_claktory_id(&body.claktory_id)
        || verifying_key(&body.backend_key).is_none()
        || !valid_label(&body.version)
        || !valid_label(&body.platform)
    {
        return Err(ApiError::invalid("A field is missing or out of range."));
    }
    sign_in_limits(service, &email, &call.ip)?;
    let request = db::Request {
        id: String::new(),
        kind: "claktory".to_owned(),
        email,
        claktory_id: Some(body.claktory_id),
        name: Some(clean_name(&body.name)),
        backend_key: Some(body.backend_key),
        version: Some(body.version),
        platform: Some(body.platform),
        code_hash: String::new(),
        attempts: 0,
        expires: 0,
    };
    let request_id = start(service, request, &new_code()).await?;
    let code_from = mail::code_address(&service.config.mail_domain);
    Ok(Reply::json(&SignInStarted { request_id, expires_in: CODE_LIFETIME, code_from }))
}

pub(super) fn sign_in_verify(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let body: SignInVerify = call.json()?;
    let eph_pub = unb64_32(&body.eph_pub)
        .filter(|key| seal_share(&[0; 32], key, "check", 0).is_some())
        .ok_or_else(|| ApiError::invalid("eph_pub isn't a key to seal to."))?;
    let request = check_code(service, &body.request_id, &body.code, "claktory")?;
    let proven = holds_key(service, call, request.claktory_id.as_deref().unwrap_or_default())?;
    let token = b64(*random_32());
    let now = service.now();
    let registered = db::register(&mut service.db(), &request, &token, now, proven)?;
    if let (db::Registered::Done { .. }, Some(location)) = (&registered, service.locate(&call.ip)) {
        let conn = service.db();
        if let Some(claktory) = db::claktory(&conn, request.claktory_id.as_deref().unwrap_or_default())? {
            db::touch(&conn, &claktory, Some(&location), now)?;
        }
    }
    let (account_id, epoch) = match registered {
        db::Registered::Done { account_id, epoch } => (account_id, epoch),
        db::Registered::Taken => {
            db::delete_request(&service.db(), &request.id)?;
            return Err(ApiError::new(StatusCode::CONFLICT, "claktory_taken", "This Claktory is signed in to another account."));
        }
        db::Registered::TooMany => {
            db::delete_request(&service.db(), &request.id)?;
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "too_many_claktories",
                "This account has as many Claktories as it can. Remove one at claktory.com/account.",
            ));
        }
    };
    let claktory_id = request.claktory_id.unwrap_or_default();
    let share = derive_share(&service.secrets.k_share, &claktory_id, epoch);
    let sealed_share =
        seal_share(&share, &eph_pub, &claktory_id, epoch).ok_or_else(|| ApiError::invalid("eph_pub isn't a key to seal to."))?;
    tracing::info!("signed in: Claktory {claktory_id} of account {account_id}, epoch {epoch}");
    Ok(Reply::json(&SignedIn {
        account_id,
        email: request.email,
        claktory_id,
        machine_token: token,
        epoch,
        sealed_share,
        service_key: service.service_key(),
    }))
}

/// Whether a sign-in's verify is signed (Claktory-Time and
/// Claktory-Signature, without a token) by the backend key the Claktory
/// with that id has now: only it may move to another account. Unsigned, or
/// signed by another key, it's false; a clock that's off or a signature
/// seen before is refused as for signed requests, leaving the code to use
/// again.
fn holds_key(service: &Service, call: &Call, claktory_id: &str) -> Result<bool, ApiError> {
    if call.header(SIGNATURE_HEADER).is_none() {
        return Ok(false);
    }
    let Some(key) = db::claktory(&service.db(), claktory_id)?.and_then(|existing| verifying_key(&existing.backend_key)) else {
        return Ok(false);
    };
    match signed_by(service, call, &key, ApiError::unauthorized) {
        Ok(()) => Ok(true),
        Err(error) if error.body.error == "unauthorized" => Ok(false),
        Err(error) => Err(error),
    }
}

/// The Claktory a signed request is from: its token, its time and its
/// backend key's signature checked, and seen only once.
fn authenticate(service: &Service, call: &Call) -> Result<db::Claktory, ApiError> {
    let token = call.header("authorization").and_then(|value| value.strip_prefix("Bearer ")).ok_or_else(ApiError::unauthorized)?;
    let claktory = db::claktory_by_token(&service.db(), token.trim())?.ok_or_else(ApiError::unauthorized)?;
    let key = verifying_key(&claktory.backend_key).ok_or_else(ApiError::unauthorized)?;
    signed_by(service, call, &key, ApiError::unauthorized)?;
    if claktory.revoked_at.is_some() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "revoked",
            "This Claktory was removed from its account. Sign in on it again to use it with the account.",
        ));
    }
    Ok(claktory)
}

/// Checks a signed request's time, its signature by `key` and that it's
/// seen only once; `unauthorized` is the error for a bad signature.
pub(super) fn signed_by(
    service: &Service,
    call: &Call,
    key: &ed25519_dalek::VerifyingKey,
    unauthorized: fn() -> ApiError,
) -> Result<(), ApiError> {
    let now = service.now();
    let time: i64 = call.header(TIME_HEADER).and_then(|value| value.trim().parse().ok()).ok_or_else(unauthorized)?;
    if (time - now).abs() > MAX_SKEW {
        let mut error = ApiError::new(StatusCode::UNAUTHORIZED, "clock_skew", "This computer's clock is off. Set it right and try again.");
        error.body.server_time = Some(now);
        return Err(error);
    }
    let signature = call.header(SIGNATURE_HEADER).ok_or_else(unauthorized)?.trim().to_owned();
    if !protocol::verify(key, &request_message(call.method.as_str(), &call.path_and_query, time, &call.body), &signature) {
        return Err(unauthorized());
    }
    let mut seen = service.seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    seen.retain(|_, until| *until >= now);
    if seen.insert(signature, time + MAX_SKEW).is_some() {
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "replayed", "This request was sent already."));
    }
    Ok(())
}

pub(super) async fn share(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let claktory = authenticate(service, call)?;
    let body: ShareRequest = call.json()?;
    let eph_pub = unb64_32(&body.eph_pub).ok_or_else(|| ApiError::invalid("eph_pub isn't a key to seal to."))?;
    let epoch = match body.epoch {
        None => claktory.epoch,
        Some(epoch) if epoch == claktory.epoch || (epoch >= 1 && epoch + 1 == claktory.epoch) => epoch,
        Some(_) => return Err(ApiError::invalid("Only the current epoch's share, or the one before's, can be asked for.")),
    };
    let now = service.now();
    let account_email = {
        let conn = service.db();
        if let Some(last) = db::last_release(&conn, &claktory.id)?
            && now - last < 60
        {
            return Err(ApiError::rate_limited(last + 60 - now));
        }
        let share = derive_share(&service.secrets.k_share, &claktory.id, epoch);
        let sealed_share =
            seal_share(&share, &eph_pub, &claktory.id, epoch).ok_or_else(|| ApiError::invalid("eph_pub isn't a key to seal to."))?;
        db::record_release(&conn, &claktory.id, now, &call.ip, epoch)?;
        db::touch(&conn, &claktory, service.locate(&call.ip).as_ref(), now)?;
        tracing::info!("share released: Claktory {}, epoch {epoch}", claktory.id);
        (db::account_email(&conn, &claktory.account_id)?, sealed_share)
    };
    let (email, sealed_share) = account_email;
    if let Some(email) = email {
        let notice = mail::unlock_mail(&service.config.mail_domain, &email, &claktory.name, now, &call.ip, &service.config.account_page);
        // The share goes either way: a mail outage mustn't keep a Claktory
        // locked. `send` logs a failure.
        let _ = service.send(&notice, false).await;
    }
    Ok(Reply::json(&Share { epoch, sealed_share }))
}

/// Periods one check-in may carry: more than two weeks of hourly ones.
const PERIODS_PER_CHECK_IN: usize = 400;
/// What no Claktory reports: a count, token count, request count, minutes
/// or runs above these is a mistake, and its period is ignored.
const MOST_COUNTED: u32 = 1_000_000;
const MOST_TOKENS: u64 = 10_000_000_000_000;
const MOST_REQUESTS: u64 = 1_000_000_000;
/// The longest a period may be: a month.
const LONGEST_PERIOD: i64 = 31 * 86_400;

/// Whether a period is one to record: in range, not in the future, and
/// with values a Claktory could have.
fn sound(period: &Period, now: i64) -> bool {
    let hours = period.from - period.from.rem_euclid(3600)..period.to;
    valid_claktory_id(&period.id)
        && period.from < period.to
        && period.to - period.from <= LONGEST_PERIOD
        && period.to <= now + MAX_SKEW
        && period.from >= now - db::PERIOD_AGE
        && period.usage.len() <= 200
        && period.usage.iter().all(|usage| {
            valid_model(&usage.provider)
                && valid_model(&usage.model)
                && usage.model_name.as_ref().is_none_or(|name| {
                    (1..=100).contains(&name.chars().count())
                        && !name.chars().any(|character| character.is_control() || "<>".contains(character))
                })
                && usage.requests <= MOST_REQUESTS
                && [usage.input_tokens, usage.output_tokens, usage.cache_read_tokens, usage.cache_write_tokens]
                    .iter()
                    .all(|tokens| *tokens <= MOST_TOKENS)
        })
        && period.activity.len() <= 31 * 24
        && period.activity.iter().all(|activity| {
            activity.hour.rem_euclid(3600) == 0
                && hours.contains(&activity.hour)
                && activity.agents_working <= MOST_COUNTED
                && activity.agent_minutes <= activity.agents_working.saturating_mul(60)
                && activity.runs <= MOST_COUNTED
        })
}

pub(super) fn check_in(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let claktory = authenticate(service, call)?;
    let mut body: CheckIn = call.json()?;
    let counts_fine =
        body.counts.is_none_or(|counts| [counts.agents, counts.projects, counts.devices].iter().all(|count| *count <= MOST_COUNTED));
    if !valid_label(&body.version) || !valid_label(&body.platform) || !counts_fine || body.periods.len() > PERIODS_PER_CHECK_IN {
        return Err(ApiError::invalid("A field is missing or out of range."));
    }
    let now = service.now();
    let sent = body.periods.len();
    body.periods.retain(|period| sound(period, now));
    let ignored = (sent - body.periods.len()) as u32;
    if ignored > 0 {
        tracing::warn!("check-in from Claktory {}: {ignored} periods out of range, ignored", claktory.id);
    }
    let location = service.locate(&call.ip);
    let recorded = db::check_in(&mut service.db(), &claktory, &body, location.as_ref(), now)?;
    Ok(Reply::json(&CheckedIn {
        relays: service.config.relays.clone(),
        server_time: now,
        epoch: claktory.epoch,
        periods_recorded: recorded,
        periods_ignored: ignored,
    }))
}

/// `GET /v1/stats/public`, for anyone: a minute old at most, and any web
/// page may read it.
pub(super) fn public_stats(service: &Service) -> Result<Reply, ApiError> {
    let mut reply = Reply::json(&service.public_stats()?);
    reply.headers.push((hyper::header::CACHE_CONTROL, hyper::header::HeaderValue::from_static("public, max-age=300")));
    reply.headers.push((hyper::header::ACCESS_CONTROL_ALLOW_ORIGIN, hyper::header::HeaderValue::from_static("*")));
    Ok(reply)
}

/// Endpoints a Claktory may have for the relay allowlist.
const ENDPOINTS_PER_CLAKTORY: usize = 20;
/// Relays kept for a Claktory, for devices that find it.
const RELAYS_PER_CLAKTORY: usize = 8;

/// A relay's address as a pairing code gives it: an http(s) URL of up to
/// 200 characters, nothing in it a page or a mail program would trip on.
fn valid_relay(relay: &str) -> bool {
    (relay.starts_with("https://") || relay.starts_with("http://"))
        && relay.len() <= 200
        && relay.bytes().all(|byte| byte.is_ascii_graphic() && !b"\"'<>\\`".contains(&byte))
}

pub(super) fn endpoints(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let claktory = authenticate(service, call)?;
    let body: Endpoints = call.json()?;
    if !valid_endpoint(&body.backend) || !body.devices.iter().all(|device| valid_endpoint(device)) {
        return Err(ApiError::invalid("An endpoint id isn't 64 hex characters."));
    }
    let backend = body.backend.to_ascii_lowercase();
    let mut devices: Vec<String> =
        body.devices.iter().map(|device| device.to_ascii_lowercase()).filter(|device| *device != backend).collect();
    devices.sort();
    devices.dedup();
    let count = devices.len() + 1;
    if count > ENDPOINTS_PER_CLAKTORY {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "too_many_endpoints", "A Claktory can have 20 endpoints at most."));
    }
    // Its name, color and relays are for devices to find it by: ones that
    // aren't what a Claktory sends are left out rather than refusing the
    // endpoints, which relays need.
    let name = body.name.as_deref().map(clean_name);
    let color = body.color.filter(|color| color.is_empty() || valid_label(color));
    let relays = body.relays.map(|relays| {
        let mut kept: Vec<String> = Vec::new();
        for relay in relays.into_iter().filter(|relay| valid_relay(relay)) {
            if !kept.contains(&relay) && kept.len() < RELAYS_PER_CLAKTORY {
                kept.push(relay);
            }
        }
        kept
    });
    let now = service.now();
    let mut conn = service.db();
    db::set_endpoints(&mut conn, &claktory.id, &backend, &devices, now)?;
    db::set_identity(&conn, &claktory.id, name.as_deref(), color.as_deref(), relays.as_deref())?;
    db::touch(&conn, &claktory, None, now)?;
    Ok(Reply::json(&EndpointsSet { count }))
}

/// `POST /v1/move`: a Claktory moved to another computer, and its new copy
/// replaces the token and backend key the old copy has.
pub(super) async fn move_claktory(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let claktory = authenticate(service, call)?;
    let body: MoveRequest = call.json()?;
    let token_fine = unb64_32(&body.machine_token).is_some();
    if !token_fine || verifying_key(&body.backend_key).is_none() || !valid_label(&body.version) || !valid_label(&body.platform) {
        return Err(ApiError::invalid("A field is missing or out of range."));
    }
    service.limit(&[(&format!("move:{}", claktory.id), 86_400, 10)])?;
    let name = clean_name(&body.name);
    let from = clean_name(&body.from);
    let now = service.now();
    let email = {
        let conn = service.db();
        if !db::moved(&conn, &claktory.id, &body.backend_key, &body.machine_token, &name, &body.version, &body.platform)? {
            return Err(ApiError::invalid("That token is in use."));
        }
        db::touch(&conn, &claktory, service.locate(&call.ip).as_ref(), now)?;
        db::account_email(&conn, &claktory.account_id)?
    };
    tracing::info!("moved: Claktory {} to {name}", claktory.id);
    if let Some(email) = email {
        let notice = mail::moved_mail(&service.config.mail_domain, &email, &from, &name, now, &call.ip, &service.config.account_page);
        let _ = service.send(&notice, false).await;
    }
    Ok(Reply::json(&MoveDone { epoch: claktory.epoch }))
}

pub(super) fn allowlist(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let given = call.header("authorization").and_then(|value| value.strip_prefix("Bearer ")).map(str::trim);
    let allowed = match (&service.secrets.relay_bearer, given) {
        (Some(bearer), Some(given)) => sha256(bearer.as_bytes()) == sha256(given.as_bytes()),
        _ => false,
    };
    if !allowed {
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized", "Only Claktory's relays can read this."));
    }
    let since: Option<i64> =
        form_urlencoded::parse(call.query.as_bytes()).find(|(key, _)| key == "since").and_then(|(_, value)| value.parse().ok());
    let conn = service.db();
    let version = db::allowlist_version(&conn)?;
    if since == Some(version) {
        return Ok(Reply::json(&Allowlist { version, endpoints: Vec::new(), unchanged: true }));
    }
    Ok(Reply::json(&Allowlist { version, endpoints: db::allowlist(&conn)?, unchanged: false }))
}
