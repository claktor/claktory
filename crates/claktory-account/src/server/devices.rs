//! The API a person's devices use (API.md's Devices): an app signs in to
//! the account with its own key, finds the account's Claktories, and gets
//! assertions that it's signed in, for the Claktories it connects to.

use hyper::StatusCode;

use super::api::{check_code, new_code, sign_in_limits, signed_by, start};
use super::{ApiError, Call, Reply, Service, db, mail};
use crate::protocol::*;

/// Assertions a device may get an hour.
const ASSERTIONS_PER_HOUR: u32 = 60;
/// Other devices a device may remove an hour.
const REMOVALS_PER_HOUR: u32 = 10;
/// Vouches a Claktory may make an hour (#529).
const VOUCHES_PER_HOUR: u32 = 10;

fn unauthorized() -> ApiError {
    ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized", "This device isn't signed in. Sign in again.")
}

/// A key removed from the account (by another of its devices, or at
/// claktory.com/account): it never signs in to that account again. The app
/// starts over with a new key, which joins as a new device.
fn removed() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "device_removed",
        "This device was removed from your account. Sign in again to start over as a new device.",
    )
}

/// A key last signed in to another account (row 671), signed in there now
/// or signed out since: it never signs in to this one, so its direct-message
/// leaf never spans two accounts. The app makes a new key and starts again
/// as a new device.
fn of_another_account() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "key_of_another_account",
        "This device was used with another account. Sign in again to start over as a new device.",
    )
}

/// The account key a device-signed request is from, its time and
/// signature checked and seen only once; signed in or not.
fn signer(service: &Service, call: &Call) -> Result<String, ApiError> {
    let account_key = call
        .header("authorization")
        .and_then(|value| value.strip_prefix(DEVICE_AUTH))
        .and_then(|value| value.strip_prefix(' '))
        .map(str::trim)
        .ok_or_else(unauthorized)?
        .to_owned();
    let key = verifying_key(&account_key).ok_or_else(unauthorized)?;
    signed_by(service, call, &key, unauthorized)?;
    Ok(account_key)
}

/// The signed-in device a request is from; counts as its contact. A key
/// that isn't bound but was removed from an account is told so (`403
/// device_removed`), only once its signature, time and limits passed, so
/// only whoever holds it learns it; any other is `401 unauthorized`.
pub(super) fn authenticate(service: &Service, call: &Call) -> Result<db::Device, ApiError> {
    let account_key = signer(service, call)?;
    let Some(device) = db::device(&service.db(), &account_key)? else {
        if db::key_removed(&service.db(), &account_key)? {
            service.verified(call, &account_key)?;
            return Err(removed());
        }
        return Err(unauthorized());
    };
    service.verified(call, &account_key)?;
    db::touch_device(&service.db(), &account_key, service.now())?;
    Ok(device)
}

fn account(service: &Service, device: &db::Device) -> Result<DeviceAccount, ApiError> {
    let email = db::account_email(&service.db(), &device.account_id)?.ok_or_else(unauthorized)?;
    Ok(DeviceAccount { account_id: device.account_id.clone(), email, name: device.name.clone() })
}

pub(super) async fn sign_in_start(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let body: DeviceSignInStart = call.json()?;
    let email = normalize_email(&body.email)
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "invalid_email", "That isn't an email address."))?;
    if verifying_key(&body.account_key).is_none() || !valid_label(&body.version) || !valid_label(&body.platform) {
        return Err(ApiError::invalid("A field is missing or out of range."));
    }
    sign_in_limits(service, &email, &call.ip)?;
    if db::device_removed(&service.db(), &email, &body.account_key)? {
        return Err(removed());
    }
    // Signed by the key it names, it's told now, before a code is sent,
    // that the key is another account's. Unsigned (older apps), it learns
    // only at verify: a public key alone mustn't tell which account it's
    // in (row 671, Secco).
    if call.header("authorization").is_some() {
        if signer(service, call)? != body.account_key {
            return Err(unauthorized());
        }
        if db::key_of_another_account(&service.db(), &email, &body.account_key)? {
            return Err(of_another_account());
        }
    }
    let request = db::Request {
        id: String::new(),
        kind: "device".to_owned(),
        email,
        claktory_id: None,
        name: Some(clean_name(&body.name)),
        backend_key: Some(body.account_key),
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
    let account_key = signer(service, call)?;
    let body: DeviceSignInVerify = call.json()?;
    // Only the key that asked for the code (or was vouched for) may use
    // it: another one is turned away before it can spend the code's tries.
    let asked = db::request(&service.db(), &body.request_id)?.filter(|request| request.kind == "device" || request.kind == "vouch");
    if asked.as_ref().and_then(|request| request.backend_key.as_deref()).is_some_and(|asked_by| asked_by != account_key) {
        return Err(unauthorized());
    }
    let kind = if asked.is_some_and(|request| request.kind == "vouch") { "vouch" } else { "device" };
    let request = check_code(service, &body.request_id, &body.code, kind, &call.ip)?;
    let apps = service.config.user_apps.is_some();
    let account_id = match db::bind_device(&mut service.db(), &request, apps, service.now())? {
        db::Bound::Done { account_id } => account_id,
        db::Bound::TooMany => {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "too_many_devices",
                &format!("This account has as many devices as it can ({}). Remove one at claktory.com/account.", db::DEVICES_PER_ACCOUNT),
            ));
        }
        db::Bound::Removed => return Err(removed()),
        db::Bound::OtherAccount => return Err(of_another_account()),
        db::Bound::Unvouched => return Err(ApiError::new(StatusCode::GONE, "code_expired", "This code has expired. Ask for a new one.")),
    };
    match &request.claktory_id {
        Some(by) if kind == "vouch" => tracing::info!("device signed in to account {account_id} through Claktory {by}"),
        _ => tracing::info!("device signed in to account {account_id}"),
    }
    Ok(Reply::json(&DeviceAccount { account_id, email: request.email, name: request.name.unwrap_or_default() }))
}

/// A signed-in Claktory vouches for the app on its own computer (#529): a
/// code, bound to that app's account key and this Claktory's account,
/// that the app gives sign-in/verify, signed by that key, within
/// `VOUCH_LIFETIME`, once. No email is sent; the device shows as signed
/// in through this Claktory, and is signed out when it signs out, signs in
/// again or elsewhere, moves or is removed.
pub(super) fn vouch(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let claktory = super::api::authenticate(service, call)?;
    let body: DeviceVouch = call.json()?;
    if verifying_key(&body.account_key).is_none() || !valid_label(&body.version) || !valid_label(&body.platform) {
        return Err(ApiError::invalid("A field is missing or out of range."));
    }
    service.limit(&[(&format!("vouch:{}", claktory.id), 3600, VOUCHES_PER_HOUR)])?;
    let conn = service.db();
    let email = db::account_email(&conn, &claktory.account_id)?.ok_or_else(ApiError::unauthorized)?;
    super::api::review_only_box(service, &email, &claktory.backend_key)?;
    if db::device_removed(&conn, &email, &body.account_key)? {
        return Err(removed());
    }
    if db::key_of_another_account(&conn, &email, &body.account_key)? {
        return Err(of_another_account());
    }
    // Carried by the app, never typed: long enough that it can't be
    // guessed in its tries. Kept only as its hash, and never logged.
    let code = b64(&random_32()[..16]);
    let request = db::Request {
        id: b64(&random_32()[..16]),
        kind: "vouch".to_owned(),
        email,
        claktory_id: Some(claktory.id.clone()),
        name: Some(clean_name(&body.name)),
        backend_key: Some(body.account_key),
        version: Some(body.version),
        platform: Some(body.platform),
        code_hash: String::new(),
        attempts: 0,
        expires: service.now() + VOUCH_LIFETIME,
    };
    let request = db::Request { code_hash: db::code_hash(&code, &request.id), ..request };
    // One code at a time per Claktory: an earlier one unused goes.
    db::forget_vouches(&conn, &claktory.id)?;
    db::insert_request(&conn, &request)?;
    tracing::info!("Claktory {} vouched for a device", claktory.id);
    Ok(Reply::json(&DeviceVouched { request_id: request.id, code, expires_in: VOUCH_LIFETIME }))
}

pub(super) fn me(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let device = authenticate(service, call)?;
    Ok(Reply::json(&account(service, &device)?))
}

pub(super) async fn sign_out(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let account_key = signer(service, call)?;
    let now = service.now();
    let apps = service.config.user_apps.is_some();
    let unbound = db::unbind_device(&mut service.db(), &account_key, None, apps, now)?;
    if let Some(account) = unbound {
        tracing::info!("device signed out");
        super::apps::signed_out(service, &account, &account_key, now, 0).await;
    }
    Ok(Reply::json(&serde_json::json!({})))
}

/// Signs another of the account's devices out (lost, stolen, given away),
/// as removing it at claktory.com/account does: its key is unbound, so this
/// service and the apps behind it answer it 401 from now on, and its
/// assertions' keys leave the relay allowlist, and the key is remembered,
/// so it never signs in to the account again (a thief with the device and
/// the mail can't get its identity back). Only a device of the same
/// account; never itself (that's sign-out).
pub(super) async fn remove(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let device = authenticate(service, call)?;
    let body: DeviceRemove = call.json()?;
    if verifying_key(&body.account_key).is_none() {
        return Err(ApiError::invalid("account_key isn't a device's key."));
    }
    if body.account_key == device.account_key {
        return Err(ApiError::invalid("A device signs itself out with sign-out."));
    }
    service.limit(&[(&format!("remove-device:{}", device.account_key), 3600, REMOVALS_PER_HOUR)])?;
    let now = service.now();
    let apps = service.config.user_apps.is_some();
    let removed = db::remove_device(&mut service.db(), &body.account_key, &device.account_id, apps, now)?;
    if removed {
        tracing::info!("removed: a device of account {}, by another of its devices", device.account_id);
        super::apps::signed_out(service, &device.account_id, &body.account_key, now, 0).await;
    }
    Ok(Reply::json(&DeviceRemoved { removed }))
}

pub(super) fn claktories(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let device = authenticate(service, call)?;
    let conn = service.db();
    let mut listed: Vec<db::Claktory> =
        db::claktories(&conn, &device.account_id)?.into_iter().filter(|claktory| claktory.revoked_at.is_none()).collect();
    listed.sort_by_key(|claktory| std::cmp::Reverse(claktory.last_seen));
    let mut claktories = Vec::with_capacity(listed.len());
    for claktory in listed {
        claktories.push(ListedClaktory {
            endpoint_id: db::backend_endpoint(&conn, &claktory.id)?.unwrap_or_default(),
            name: claktory.display_name.clone().unwrap_or_else(|| claktory.name.clone()),
            color: claktory.color.unwrap_or_default(),
            machine: claktory.name,
            platform: claktory.platform,
            version: claktory.version,
            relays: claktory.relays,
            last_contact_ms: claktory.last_seen.saturating_mul(1000),
            claktory_id: claktory.id,
        });
    }
    Ok(Reply::json(&ClaktoryList { claktories }))
}

pub(super) fn assertion(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let device = authenticate(service, call)?;
    let body: AssertionRequest = call.json()?;
    if !valid_endpoint(&body.device_key) {
        return Err(ApiError::invalid("device_key isn't 64 hex characters."));
    }
    service.limit(&[(&format!("assertion:{}", device.account_key), 3600, ASSERTIONS_PER_HOUR)])?;
    let now = service.now();
    let device_key = body.device_key.to_ascii_lowercase();
    let said = DeviceAssertion {
        v: 1,
        account_id: device.account_id,
        account_key: device.account_key.clone(),
        device_key: device_key.clone(),
        iat: now,
        exp: now + ASSERTION_LIFETIME,
    };
    let assertion = sign_device_assertion(&service.secrets.signing_key, &said);
    db::issued_assertion(&service.db(), &device.account_key, &device_key, now)?;
    Ok(Reply::json(&AssertionIssued { assertion, expires_in: ASSERTION_LIFETIME }))
}
