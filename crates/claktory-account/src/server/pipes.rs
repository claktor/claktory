//! Verified pipes (API.md's `POST /v1/pipe-identity`): the service's word,
//! for the Claktories a Claktory has pipes with, that whoever holds its
//! pipe key is a Claktory of its account, with the account's email. Only
//! for the key the Claktory registered as its backend endpoint and proves
//! it holds; only for an email in printable ASCII, and never a claktory.com
//! one the operator didn't allow.

use hyper::StatusCode;

use super::api::authenticate;
use super::{ApiError, Call, Reply, Service, db};
use crate::protocol::*;

/// Statements a Claktory may get a day: it asks about once a day, and again
/// on signing in or out.
const STATEMENTS_PER_DAY: u32 = 48;

pub(super) fn identity(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let claktory = authenticate(service, call)?;
    service.limit(&[(&format!("pipe-identity:{}", claktory.id), 86_400, STATEMENTS_PER_DAY)])?;
    let body: PipeIdentityRequest = call.json()?;
    if !valid_endpoint(&body.endpoint_id) {
        return Err(ApiError::invalid("endpoint_id isn't 64 hex characters."));
    }
    let endpoint_id = body.endpoint_id.to_ascii_lowercase();
    let now = service.now();
    if (body.time - now).abs() > MAX_SKEW {
        return Err(ApiError::invalid("time is too far from now."));
    }
    // The endpoint key itself signed the request for this Claktory and
    // time, so the service never vouches for a key the asker doesn't hold.
    let proven = unhex_32(&endpoint_id)
        .and_then(|bytes| ed25519_dalek::VerifyingKey::from_bytes(&bytes).ok())
        .is_some_and(|key| verify(&key, &pipe_identity_proof(&claktory.id, &endpoint_id, body.time), &body.proof));
    if !proven {
        return Err(ApiError::invalid("proof isn't the endpoint key's signature for this Claktory and time."));
    }
    let registered = db::backend_endpoint(&service.db(), &claktory.id)?;
    if registered.as_deref() != Some(endpoint_id.as_str()) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "wrong_endpoint",
            "That isn't this Claktory's endpoint. Register it with PUT /v1/endpoints first.",
        ));
    }
    let email = db::account_email(&service.db(), &claktory.account_id)?
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized", "This Claktory's account is gone."))?;
    if !ascii_email(&email) || (claktorys_own(&email) && !allowed(&service.config.vouched_claktory_emails, &email)) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "unsupported_email",
            "This account's email can't be confirmed to others: only one in plain letters, digits and punctuation can, and not a claktory.com one.",
        ));
    }
    let statement = PipePeerStatement {
        v: 1,
        label: PIPE_PEER_LABEL.to_owned(),
        service: service.config.public_url.trim_end_matches('/').to_owned(),
        service_key: service.service_key(),
        endpoint_id,
        account_id: claktory.account_id.clone(),
        email,
        iat: now,
        exp: now + PIPE_PEER_LIFETIME,
    };
    let statement = sign_pipe_peer(&service.secrets.signing_key, &statement);
    db::touch(&service.db(), &claktory, None, now)?;
    Ok(Reply::json(&PipeIdentityIssued { statement, expires_in: PIPE_PEER_LIFETIME }))
}

/// An address at claktory.com or any of its subdomains: Claktory's own,
/// which "Verified" would show as its staff's.
fn claktorys_own(email: &str) -> bool {
    let domain = email.rsplit_once('@').map_or("", |(_, domain)| domain);
    domain == "claktory.com" || domain.ends_with(".claktory.com")
}

/// Whether the operator allowed this claktory.com address to be vouched for.
fn allowed(allowlist: &[String], email: &str) -> bool {
    allowlist.iter().any(|allowed| normalize_email(allowed).as_deref() == Some(email))
}

fn unhex_32(text: &str) -> Option<[u8; 32]> {
    let bytes: Vec<u8> = (0..text.len())
        .step_by(2)
        .map(|at| text.get(at..at + 2).and_then(|pair| u8::from_str_radix(pair, 16).ok()))
        .collect::<Option<_>>()?;
    bytes.try_into().ok()
}
