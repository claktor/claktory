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

/// The signed-in device a request is from; counts as its contact.
pub(super) fn authenticate(service: &Service, call: &Call) -> Result<db::Device, ApiError> {
    let account_key = signer(service, call)?;
    let device = db::device(&service.db(), &account_key)?.ok_or_else(unauthorized)?;
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
    // Only the key that asked for the code may use it: another one is
    // turned away before it can spend the code's tries.
    let asked_by =
        db::request(&service.db(), &body.request_id)?.filter(|request| request.kind == "device").and_then(|request| request.backend_key);
    if asked_by.is_some_and(|asked_by| asked_by != account_key) {
        return Err(unauthorized());
    }
    let request = check_code(service, &body.request_id, &body.code, "device", &call.ip)?;
    let account_id = match db::bind_device(&mut service.db(), &request, service.now())? {
        db::Bound::Done { account_id } => account_id,
        db::Bound::TooMany => {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "too_many_devices",
                &format!("This account has as many devices as it can ({}). Remove one at claktory.com/account.", db::DEVICES_PER_ACCOUNT),
            ));
        }
        db::Bound::Removed => return Err(removed()),
    };
    tracing::info!("device signed in to account {account_id}");
    Ok(Reply::json(&DeviceAccount { account_id, email: request.email, name: request.name.unwrap_or_default() }))
}

pub(super) fn me(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let device = authenticate(service, call)?;
    Ok(Reply::json(&account(service, &device)?))
}

pub(super) fn sign_out(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let account_key = signer(service, call)?;
    if db::unbind_device(&mut service.db(), &account_key, None)? {
        tracing::info!("device signed out");
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
pub(super) fn remove(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let device = authenticate(service, call)?;
    let body: DeviceRemove = call.json()?;
    if verifying_key(&body.account_key).is_none() {
        return Err(ApiError::invalid("account_key isn't a device's key."));
    }
    if body.account_key == device.account_key {
        return Err(ApiError::invalid("A device signs itself out with sign-out."));
    }
    service.limit(&[(&format!("remove-device:{}", device.account_key), 3600, REMOVALS_PER_HOUR)])?;
    let removed = db::remove_device(&mut service.db(), &body.account_key, &device.account_id, service.now())?;
    if removed {
        tracing::info!("removed: a device of account {}, by another of its devices", device.account_id);
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
