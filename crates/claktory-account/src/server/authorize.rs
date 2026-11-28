//! A computer joins the account with the person's approval, and no code
//! anywhere: it asks (`start`, or a sign-in verify once the account has a
//! device, path 4b), the account's devices show what the service saw
//! (`list`), and the person approves it on one of them (`decide`), with the
//! word of a Claktory of theirs that the device is allowed on it now and,
//! once the account has a DM identity, the worker's word that the device is
//! on its signed list. The computer polls (`poll`) and is signed in as a
//! code would sign it in.
//!
//! A move (kind "move") goes the same way to the approval, with the
//! device's own signed approval of it, then differs: it's shown only in
//! Settings → Move (`moves`), never with the bell; the source fetches it
//! (`approved_moves`) and takes it once (`take`); and the destination's
//! poll then tells it the one endpoint to let in, signing nothing in.

use hyper::StatusCode;

use super::api::{finish_sign_in, key_retired, review_box_only, signed_by};
use super::{ApiError, Call, Reply, Service, apps, db, devices, mail};
use crate::protocol::*;

/// Requests to join an email may start an hour.
const STARTS_PER_EMAIL: u32 = 5;
/// Requests to join an address may start an hour.
const STARTS_PER_IP: u32 = 10;

fn unauthorized() -> ApiError {
    ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized", "This request isn't signed by the computer that asked.")
}

fn no_device() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "no_device_to_approve",
        "No device of this account can approve a computer yet. Sign in on your own computer with the emailed code instead.",
    )
}

fn two_at_once() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "two_at_once",
        "Two computers asked to join this account at the same time, so neither was added. Start again.",
    )
}

fn expired() -> ApiError {
    ApiError::new(StatusCode::GONE, "expired_token", "This request to join is no longer waiting. Start again.")
}

/// `409 authorization_pending`, with what to poll.
fn pending(request_id: &str, expires_in: i64) -> ApiError {
    let mut error = ApiError::new(StatusCode::CONFLICT, "authorization_pending", "Approve this computer on one of your devices.");
    error.body.pending =
        Some(Box::new(AuthorizationPending { request_id: request_id.to_owned(), expires_in: expires_in.max(0), interval: AUTHORIZE_INTERVAL }));
    error
}

/// A request to join, as the service saw it at `now`, waiting.
#[allow(clippy::too_many_arguments)]
fn asked(
    service: &Service,
    call: &Call,
    id: String,
    account_id: &str,
    email: &str,
    claktory_id: &str,
    backend_key: &str,
    says: AuthorizationSays,
    receive: Option<MoveReceive>,
) -> db::Authorization {
    let now = service.now();
    db::Authorization {
        id,
        kind: if receive.is_some() { KIND_MOVE } else { KIND_CLAKTORY }.to_owned(),
        account_id: account_id.to_owned(),
        email: email.to_owned(),
        claktory_id: claktory_id.to_owned(),
        name: clean_card_name(&says.name),
        backend_key: backend_key.to_owned(),
        version: says.version,
        platform: says.platform,
        ip: call.ip.clone(),
        network: service.network(&call.ip).unwrap_or_default(),
        place: service.locate(&call.ip).and_then(|place| place.describe()).unwrap_or_default(),
        started: now,
        expires: now + AUTHORIZE_LIFETIME,
        state: "waiting".to_owned(),
        approver: None,
        decided: None,
        receive_relays: receive.map(|receive| receive.relays).unwrap_or_default(),
        move_approval: None,
        source_claktory_id: None,
        source_key: None,
        declined_by: None,
    }
}

/// A move's destination: its receive key as an endpoint id (64 lowercase
/// hex), which its `backend_key` (base64url) is.
fn endpoint_of(backend_key: &str) -> String {
    unb64_32(backend_key).map(|key| key.iter().map(|byte| format!("{byte:02x}")).collect()).unwrap_or_default()
}

/// The relays a move's source may dial its destination through: https
/// addresses only, at most 4, each a label's length.
fn relays(given: &[String]) -> Vec<String> {
    given.iter().filter(|relay| relay.starts_with("https://") && relay.len() <= 256).take(4).cloned().collect()
}

/// Keeps a request to join; a second one waiting for the account ends both,
/// recorded with the address that asked second.
fn keep(service: &Service, asked: &db::Authorization) -> Result<db::Asked, ApiError> {
    let taken = db::ask_to_join(&mut service.db(), asked)?;
    if taken == db::Asked::TwoAtOnce {
        db::log_authorization(&service.db(), &asked.account_id, &asked.id, "two_at_once", &asked.ip, asked.started)?;
        tracing::warn!("two computers asked to join account {} at once: neither joins", asked.account_id);
    }
    Ok(taken)
}

/// `POST /v1/claktories/authorize/start`.
pub(super) async fn start(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let body: AuthorizeStart = call.json()?;
    let email = normalize_email(&body.email)
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "invalid_email", "That isn't an email address."))?;
    let key = verifying_key(&body.backend_key);
    // A move's destination has no Claktory id yet, and says where the
    // source dials it: its receive key, the one that signs.
    let receive = match body.kind.as_str() {
        KIND_CLAKTORY if valid_claktory_id(&body.claktory_id) => None,
        KIND_MOVE if body.claktory_id.is_empty() => {
            let receive = body.receive.as_ref().ok_or_else(|| ApiError::invalid("A move needs where to receive it."))?;
            if !valid_endpoint(&receive.endpoint) || receive.endpoint != endpoint_of(&body.backend_key) {
                return Err(ApiError::invalid("receive.endpoint isn't the receive key."));
            }
            Some(MoveReceive { endpoint: receive.endpoint.clone(), relays: relays(&receive.relays) })
        }
        _ => return Err(ApiError::invalid("A field is missing or out of range.")),
    };
    if key.is_none() || !valid_label(&body.version) || !valid_label(&body.platform) {
        return Err(ApiError::invalid("A field is missing or out of range."));
    }
    let key = key.ok_or_else(unauthorized)?;
    service.limit(&[
        (&db::email_key("authorize-hour", &email), 3600, STARTS_PER_EMAIL),
        (&format!("authorize-ip:{}", call.ip), 3600, STARTS_PER_IP),
    ])?;
    signed_by(service, call, &key, unauthorized)?;
    if db::backend_key_retired(&service.db(), &body.backend_key)? {
        return Err(key_retired());
    }
    // A review email joins no Claktory but the review box: nothing
    // waits for approval.
    review_box_only(service, &email, &body.backend_key)?;
    let account = db::account_by_email(&service.db(), &email)?.ok_or_else(no_device)?;
    if !db::may_be_approved(&service.db(), &account.id)? {
        return Err(no_device());
    }
    let says = AuthorizationSays { name: body.name, platform: body.platform, version: body.version };
    let moving = receive.is_some();
    let asked = asked(service, call, b64(&random_32()[..16]), &account.id, &email, &body.claktory_id, &body.backend_key, says, receive);
    if keep(service, &asked)? == db::Asked::TwoAtOnce {
        return Err(two_at_once());
    }
    let seen = [asked.place.as_str(), asked.ip.as_str()].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join(", ");
    let domain = &service.config.mail_domain;
    let heads_up = if moving {
        mail::move_mail(domain, &email, &asked.name, asked.started, &seen)
    } else {
        mail::authorize_mail(domain, &email, &asked.name, asked.started, &seen)
    };
    if service.send(&heads_up, false).await.is_err() {
        tracing::warn!("the heads-up for a computer asking to join account {} wasn't sent", account.id);
    }
    tracing::info!(
        "a computer asks to {} account {}: request {}",
        if moving { "receive a Claktory of" } else { "join" },
        account.id,
        asked.id
    );
    Ok(Reply::json(&AuthorizeStarted { request_id: asked.id, expires_in: AUTHORIZE_LIFETIME, interval: AUTHORIZE_INTERVAL }))
}

/// Path 4b: a sign-in whose code was right waits for the person's approval
/// when the account has a device that may approve, unless it's a Claktory
/// signing in again to its account with the backend key it has there now,
/// which it proved by signing the verify with it. The `409
/// authorization_pending` (or `two_at_once`) to answer with; None to sign
/// it in now.
pub(super) fn wait_for_approval(service: &Service, call: &Call, request: &db::Request, proven: bool) -> Result<Option<ApiError>, ApiError> {
    let Some(account) = db::account_by_email(&service.db(), &request.email)? else { return Ok(None) };
    if !db::may_be_approved(&service.db(), &account.id)? {
        return Ok(None);
    }
    let (claktory_id, backend_key) = (request.claktory_id.clone().unwrap_or_default(), request.backend_key.clone().unwrap_or_default());
    let again = db::claktory(&service.db(), &claktory_id)?.is_some_and(|existing| {
        proven && existing.account_id == account.id && existing.revoked_at.is_none() && existing.backend_key == backend_key
    });
    if again {
        return Ok(None);
    }
    let says = AuthorizationSays {
        name: request.name.clone().unwrap_or_default(),
        platform: request.platform.clone().unwrap_or_default(),
        version: request.version.clone().unwrap_or_default(),
    };
    let asked = asked(service, call, request.id.clone(), &account.id, &request.email, &claktory_id, &backend_key, says, None);
    db::delete_request(&service.db(), &request.id)?;
    if keep(service, &asked)? == db::Asked::TwoAtOnce {
        return Ok(Some(two_at_once()));
    }
    tracing::info!("a Claktory signing in to account {} waits for approval: request {}", account.id, asked.id);
    Ok(Some(pending(&asked.id, AUTHORIZE_LIFETIME)))
}

/// `POST /v1/claktories/authorize/poll`.
pub(super) fn poll(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let body: AuthorizePoll = call.json()?;
    let asked = db::authorization(&service.db(), &body.request_id)?.ok_or_else(expired)?;
    let key = verifying_key(&asked.backend_key).ok_or_else(unauthorized)?;
    signed_by(service, call, &key, unauthorized)?;
    service.verified(call, &asked.backend_key)?;
    let eph_pub = unb64_32(&body.eph_pub)
        .filter(|key| seal_share(&[0; 32], key, "check", 0).is_some())
        .ok_or_else(|| ApiError::invalid("eph_pub isn't a key to seal to."))?;
    let now = service.now();
    if asked.kind == KIND_MOVE {
        return move_polled(service, &asked, now);
    }
    match asked.state.as_str() {
        "waiting" if asked.expires > now => return Err(pending(&asked.id, asked.expires - now)),
        "declined" => return Err(ApiError::new(StatusCode::FORBIDDEN, "access_denied", "The person declined this computer.")),
        "two_at_once" => return Err(two_at_once()),
        "approved" => {}
        _ => return Err(expired()),
    }
    if db::backend_key_retired(&service.db(), &asked.backend_key)? {
        return Err(key_retired());
    }
    // Checked again where it registers: the review box's key may have
    // changed since it asked, or it asked before the check. Ended.
    if let Err(error) = review_box_only(service, &asked.email, &asked.backend_key) {
        db::spend_authorization(&service.db(), &asked.id)?;
        return Err(error);
    }
    if !db::spend_authorization(&service.db(), &asked.id)? {
        return Err(expired());
    }
    let first_device = asked.approver.clone().unwrap_or_default();
    let request = db::Request {
        id: asked.id.clone(),
        kind: "claktory".to_owned(),
        email: asked.email.clone(),
        claktory_id: Some(asked.claktory_id.clone()),
        name: Some(asked.name.clone()),
        backend_key: Some(asked.backend_key.clone()),
        version: Some(asked.version.clone()),
        platform: Some(asked.platform.clone()),
        code_hash: String::new(),
        attempts: 0,
        expires: asked.expires,
    };
    // Its own key, signing this poll, proves it holds the key the Claktory
    // with that id has, when it's that one: it may move, as at verify.
    let proven = db::claktory(&service.db(), &asked.claktory_id)?.is_some_and(|existing| existing.backend_key == asked.backend_key);
    let signed_in = finish_sign_in(service, &request, &eph_pub, proven, &call.ip)?;
    tracing::info!("approved: Claktory {} joins account {}", signed_in.claktory_id, signed_in.account_id);
    Ok(Reply::json(&Authorized { signed_in, first_device }))
}

/// A move's destination polls: it waits until the source took the move,
/// approved or not yet, then hears, once, the only endpoint to let in.
/// Nothing signs in: the moved Claktory keeps its source's registration.
fn move_polled(service: &Service, asked: &db::Authorization, now: i64) -> Result<Reply, ApiError> {
    match asked.state.as_str() {
        "waiting" | "approved" if asked.expires > now => return Err(pending(&asked.id, asked.expires - now)),
        "declined" => return Err(ApiError::new(StatusCode::FORBIDDEN, "access_denied", "The person declined moving a Claktory here.")),
        "two_at_once" => return Err(two_at_once()),
        "taken" => {}
        _ => return Err(expired()),
    }
    let (Some(source_key), Some(source_claktory_id), Some(approver)) =
        (asked.source_key.clone(), asked.source_claktory_id.clone(), asked.approver.clone())
    else {
        return Err(expired());
    };
    if !db::spend_move(&service.db(), &asked.id)? {
        return Err(expired());
    }
    tracing::info!("a move of account {}'s Claktory {source_claktory_id} goes ahead: request {}", asked.account_id, asked.id);
    Ok(Reply::json(&MoveAuthorized { kind: KIND_MOVE.to_owned(), source_key, source_claktory_id, approver }))
}

/// A request as the account's devices are shown it at `now`.
fn shown_to(device: &db::Device, asked: db::Authorization, now: i64) -> DeviceAuthorization {
    let endpoint = if asked.kind == KIND_MOVE { endpoint_of(&asked.backend_key) } else { String::new() };
    DeviceAuthorization {
        fingerprint: key_fingerprint(&asked.backend_key),
        expires_in: if asked.state == "waiting" { (asked.expires - now).max(0) } else { 0 },
        seen: AuthorizationSeen { ip: asked.ip, network: asked.network, place: asked.place },
        says: AuthorizationSays { name: asked.name, platform: asked.platform, version: asked.version },
        id: asked.id,
        kind: asked.kind,
        state: asked.state,
        started: asked.started,
        may_approve: device.vouched_by.is_none(),
        endpoint,
        answered_by: String::new(),
        answered_at: 0,
    }
}

/// `GET /v1/devices/authorizations`: computers asking to join, never a
/// move. One answered lately is listed too, saying how and on which
/// device, with nothing that lets a device act on it.
pub(super) fn list(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let device = devices::authenticate(service, call)?;
    let now = service.now();
    let conn = service.db();
    let mut authorizations = Vec::new();
    for asked in db::authorizations_shown(&conn, &device.account_id, KIND_CLAKTORY, now)? {
        let answered = match asked.state.as_str() {
            "approved" | "spent" => Some(("approved", asked.approver.clone())),
            "declined" => Some(("declined", asked.declined_by.clone())),
            _ => None,
        };
        let Some((state, by)) = answered else {
            authorizations.push(shown_to(&device, asked, now));
            continue;
        };
        let by = match by {
            Some(key) => db::device(&conn, &key)?.filter(|by| by.account_id == asked.account_id).map(|by| clean_card_name(&by.name)),
            None => None,
        };
        let answered_at = asked.decided.unwrap_or_default();
        let mut shown = shown_to(&device, asked, now);
        shown.state = state.to_owned();
        shown.fingerprint = String::new();
        shown.may_approve = false;
        shown.answered_by = by.unwrap_or_default();
        shown.answered_at = answered_at;
        authorizations.push(shown);
    }
    Ok(Reply::json(&DeviceAuthorizations { authorizations }))
}

/// `GET /v1/devices/moves`: the account's move waiting, for Settings →
/// Move, the only place a move is shown.
pub(super) fn moves(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let device = devices::authenticate(service, call)?;
    let now = service.now();
    let shown = db::authorizations_shown(&service.db(), &device.account_id, KIND_MOVE, now)?;
    Ok(Reply::json(&DeviceMoves { moves: shown.into_iter().map(|asked| shown_to(&device, asked, now)).collect() }))
}

/// `GET /v1/claktories/moves`: the moves approved with this Claktory as
/// their source, not taken yet, each with the device's approval as it came
/// for the source to check. Spends nothing.
pub(super) fn approved_moves(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let claktory = super::api::authenticate(service, call)?;
    let now = service.now();
    let moves = db::moves_approved(&service.db(), &claktory.id, now)?
        .into_iter()
        .filter(|asked| asked.account_id == claktory.account_id)
        .filter_map(|asked| {
            Some(WaitingMove {
                approval: asked.move_approval?,
                receive: MoveReceive { endpoint: endpoint_of(&asked.backend_key), relays: asked.receive_relays },
                seen: AuthorizationSeen { ip: asked.ip, network: asked.network, place: asked.place },
                says: AuthorizationSays { name: asked.name, platform: asked.platform, version: asked.version },
                fingerprint: key_fingerprint(&asked.backend_key),
                expires_in: (asked.expires - now).max(0),
                request_id: asked.id,
            })
        })
        .collect();
    Ok(Reply::json(&ClaktoryMoves { moves }))
}

/// `POST /v1/claktories/moves/<id>/take`: the source takes an approved
/// move, once, within its lifetime, naming the endpoint it dials from, the
/// only one the destination lets in.
pub(super) fn take(service: &Service, call: &Call, id: &str) -> Result<Reply, ApiError> {
    let claktory = super::api::authenticate(service, call)?;
    let body: MoveTake = call.json()?;
    if !valid_endpoint(&body.endpoint) || body.endpoint.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err(ApiError::invalid("endpoint isn't an endpoint id."));
    }
    let not_found = || ApiError::new(StatusCode::NOT_FOUND, "not_found", "There's no such move.");
    let asked = db::authorization(&service.db(), id)?
        .filter(|asked| {
            asked.kind == KIND_MOVE
                && asked.account_id == claktory.account_id
                && asked.source_claktory_id.as_deref() == Some(claktory.id.as_str())
        })
        .ok_or_else(not_found)?;
    let now = service.now();
    if db::take_move(&service.db(), &asked.id, &claktory.id, &body.endpoint, now)? {
        tracing::info!("Claktory {} took its move: request {}", claktory.id, asked.id);
        return Ok(Reply::json(&serde_json::json!({})));
    }
    match asked.state.as_str() {
        "taken" | "spent" => Err(ApiError::new(StatusCode::CONFLICT, "move_taken", "This move was taken already.")),
        "approved" => Err(expired()),
        _ => Err(not_found()),
    }
}

fn not_allowed(why: &str) -> ApiError {
    tracing::warn!("an approval refused: {why}");
    ApiError::new(
        StatusCode::FORBIDDEN,
        "not_allowed_to_approve",
        "This device can't approve a computer. Approve it on a device you signed in to by email that's connected to one of your Claktories.",
    )
}

/// `POST /v1/devices/authorizations/<id>`.
pub(super) async fn decide(service: &Service, call: &Call, id: &str) -> Result<Reply, ApiError> {
    let device = devices::authenticate(service, call)?;
    let body: AuthorizationDecision = call.json()?;
    let not_found = || ApiError::new(StatusCode::NOT_FOUND, "not_found", "There's no such request.");
    let asked = db::authorization(&service.db(), id)?.filter(|asked| asked.account_id == device.account_id).ok_or_else(not_found)?;
    let now = service.now();
    if asked.state != "waiting" || asked.expires <= now {
        return Err(expired());
    }
    if body.fingerprint != key_fingerprint(&asked.backend_key) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "fingerprint_changed",
            "This isn't the computer the card showed. Look at the card again.",
        ));
    }
    if !body.approve {
        if !db::decide_authorization(&service.db(), &asked.id, false, &device.account_key, now)? {
            return Err(expired());
        }
        db::log_authorization(&service.db(), &asked.account_id, &asked.id, "declined", &call.ip, now)?;
        tracing::info!("declined: a computer asking to join account {}", asked.account_id);
        return Ok(Reply::json(&serde_json::json!({})));
    }
    if device.vouched_by.is_some() {
        return Err(not_allowed("a device a Claktory vouched for"));
    }
    let source = allowed_by_a_claktory(service, &device, &asked, body.allowed.as_deref().unwrap_or_default(), now)?;
    on_the_dm_list(service, &device, &asked).await?;
    if asked.kind == KIND_MOVE {
        let approval = body.move_approval.as_deref().unwrap_or_default();
        move_approved(&device, &asked, approval, &source, now)?;
        if !db::approve_move(&service.db(), &asked.id, &device.account_key, approval.trim(), &source, service.now())? {
            return Err(expired());
        }
        tracing::info!("approved: moving a Claktory of account {}, by one of its devices", asked.account_id);
        return Ok(Reply::json(&serde_json::json!({})));
    }
    if !db::decide_authorization(&service.db(), &asked.id, true, &device.account_key, service.now())? {
        return Err(expired());
    }
    tracing::info!("approved: a computer asking to join account {}, by one of its devices", asked.account_id);
    Ok(Reply::json(&serde_json::json!({})))
}

/// Checks a device's own approval of a move, kept as it came for the source
/// to check again: signed by the approving device's own key, for this
/// request, account, destination and its fingerprint, the Claktory whose
/// word came with it (`source`, the one that moves), and ending no later
/// than the request.
fn move_approved(device: &db::Device, asked: &db::Authorization, approval: &str, source: &str, now: i64) -> Result<(), ApiError> {
    if approval.trim().is_empty() || approval.len() > 4096 {
        return Err(not_allowed("a move without the device's own approval"));
    }
    let approval = verify_move_approval(approval, now).ok_or_else(|| not_allowed("a move approval that doesn't check"))?;
    if approval.device_key != device.account_key {
        return Err(not_allowed("a move approval by another device"));
    }
    if approval.request_id != asked.id
        || approval.account_id != device.account_id
        || approval.endpoint != endpoint_of(&asked.backend_key)
        || approval.fingerprint != key_fingerprint(&asked.backend_key)
    {
        return Err(not_allowed("a move approval for another request"));
    }
    if approval.source_claktory_id != source {
        return Err(not_allowed("a move approval for another Claktory than the word's"));
    }
    if approval.exp > asked.expires {
        return Err(not_allowed("a move approval outlasting the request"));
    }
    Ok(())
}

/// Checks a Claktory's word that `device` is allowed on it now, for this
/// request: a Claktory of the account, signed in before the request
/// started, never the computer asking (by id or key), whose backend key
/// signed it within its lifetime. The Claktory whose word it is.
fn allowed_by_a_claktory(
    service: &Service,
    device: &db::Device,
    asked: &db::Authorization,
    word: &str,
    now: i64,
) -> Result<String, ApiError> {
    if word.is_empty() {
        return Err(not_allowed("no Claktory's word"));
    }
    // Who claims to have signed it, read before the signature is checked
    // with that Claktory's key.
    let claimed: Option<AllowedDevice> =
        word.split_once('.').and_then(|(payload, _)| unb64(payload)).and_then(|payload| serde_json::from_slice(&payload).ok());
    let claimed = claimed.ok_or_else(|| not_allowed("a word that doesn't read"))?;
    let signer = db::claktory(&service.db(), &claimed.claktory_id)?.ok_or_else(|| not_allowed("an unknown Claktory's word"))?;
    if signer.account_id != device.account_id || signer.revoked_at.is_some() {
        return Err(not_allowed("the word of a Claktory not of this account now"));
    }
    if signer.id == asked.claktory_id || signer.backend_key == asked.backend_key {
        return Err(not_allowed("the word of the computer asking"));
    }
    // Every registration since the column was added keeps its time (a new
    // row and a sign-in again both set it in db::register); a row with none
    // registered before then, so before any request.
    if db::registered_at(&service.db(), &signer.id)?.is_some_and(|registered| registered >= asked.started) {
        return Err(not_allowed("the word of a Claktory signed in since the request"));
    }
    if db::backend_key_retired(&service.db(), &signer.backend_key)? {
        return Err(not_allowed("the word of a retired key"));
    }
    let key = verifying_key(&signer.backend_key).ok_or_else(|| not_allowed("a Claktory without a key"))?;
    let word = verify_allowed_device(&key, word, now).ok_or_else(|| not_allowed("a word not signed by its Claktory, or too old"))?;
    if word.account_id != device.account_id || word.account_key != device.account_key || word.request_id != asked.id {
        return Err(not_allowed("a word for another device, account or request"));
    }
    Ok(signer.id)
}

/// Once the account has a DM identity its devices set up before the
/// request, by a device never vouched for, the approver must be on its
/// current signed device list too, as the worker holds it. A service
/// without a worker checks nothing here; a worker that can't be asked
/// decides nothing.
async fn on_the_dm_list(service: &Service, device: &db::Device, asked: &db::Authorization) -> Result<(), ApiError> {
    let listed = match apps::device_listed(service, &device.account_id, &device.account_key).await {
        Ok(Some(listed)) => listed,
        Ok(None) => return Ok(()),
        Err(failure) => {
            tracing::error!("can't ask the worker whether an approver is on the DM list: {failure}");
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Something went wrong. Nothing was decided; try again.",
            ));
        }
    };
    if !listed.identity {
        return Ok(());
    }
    let older = listed.since.is_none_or(|since| since < asked.started.saturating_mul(1000));
    let by_the_person = match listed.set_by.as_deref() {
        Some(set_by) => !db::ever_vouched(&service.db(), set_by)?,
        None => true,
    };
    if older && by_the_person && !listed.listed {
        return Err(not_allowed("not on the account's DM device list"));
    }
    Ok(())
}

/// `GET /v1/claktories/authorizations`.
pub(super) fn waiting(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let claktory = super::api::authenticate(service, call)?;
    let waiting = db::authorizations_waiting(&service.db(), &claktory.account_id, service.now())?;
    Ok(Reply::json(&ClaktoryAuthorizations { waiting }))
}
