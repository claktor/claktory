//! Tokens for Claktory's relays (#450; coordination/security/relay-access.md).
//! Our relays carry a packet only when its sender or its recipient is a
//! signed-in Claktory, which shows it with a token from here: bound to its
//! endpoint, naming its account only by an opaque id (`a`, for the relays'
//! denylist), working 7 days, signed by the relay-token key.
//!
//! The key is derived from `k_share` (HKDF-SHA256, info "claktory relay-token
//! v1"), so there's nothing new to back up; `a` is HMAC-SHA256 with another
//! key derived the same way, cut to 16 bytes. A new `k_share`
//! (`rotate-secrets --k-share`) makes a new relay-token key: the relays then
//! need its public half (`relay-token-pubkey`) beside the old one's.

use ed25519_dalek::SigningKey;
use hmac::{Hmac, Mac};
use hyper::StatusCode;
use sha2::Sha256;
use zeroize::Zeroizing;

use super::{ApiError, Call, Reply, Service, api, db};
use crate::protocol::{self, RelayToken, RelayTokenRequest, hex};

/// Endpoints an account may get tokens for a day (UTC), new ones.
pub const ENDPOINTS_PER_DAY: u32 = 4;
/// Tokens a Claktory may get a day.
pub const TOKENS_PER_DAY: u32 = 24;

/// The relay-token key, and the key `a` is made with.
pub struct Keys {
    signing: SigningKey,
    account: Zeroizing<[u8; 32]>,
}

impl Keys {
    /// Both, from the service's `k_share`.
    pub fn derive(k_share: &[u8; 32]) -> Keys {
        let hkdf = hkdf::Hkdf::<Sha256>::new(None, k_share);
        let mut signing = Zeroizing::new([0u8; 32]);
        let mut account = Zeroizing::new([0u8; 32]);
        hkdf.expand(b"claktory relay-token v1", signing.as_mut()).expect("32 bytes is a valid length");
        hkdf.expand(b"claktory relay-token account v1", account.as_mut()).expect("32 bytes is a valid length");
        Keys { signing: SigningKey::from_bytes(&signing), account }
    }

    /// The public half, as the relays' key file has it: 64 lower-case hex.
    pub fn public_hex(&self) -> String {
        hex(self.signing.verifying_key().to_bytes())
    }

    /// The account's opaque id in its tokens: 32 lower-case hex.
    pub fn account(&self, account_id: &str) -> String {
        let mut mac = <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(self.account.as_ref()).expect("any key length");
        mac.update(account_id.as_bytes());
        hex(&mac.finalize().into_bytes()[..16])
    }

    /// A token for `endpoint` (64 lower-case hex) of `account_id`'s, made
    /// `now`: it and when it stops working.
    pub fn issue(&self, endpoint: &str, account_id: &str, now: i64) -> (String, i64) {
        let expires = now + protocol::RELAY_TOKEN_LIFETIME;
        let payload = protocol::relay_token_payload(endpoint, &self.account(account_id), now, expires);
        (protocol::sign_relay_token(&self.signing, payload.as_bytes()), expires)
    }
}

/// `POST /v1/relay/token`: a signed-in Claktory's token for its endpoint.
pub(super) fn token(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let claktory = api::authenticate(service, call)?;
    let body: RelayTokenRequest = call.json()?;
    if !protocol::valid_endpoint(&body.endpoint_id) {
        return Err(ApiError::invalid("An endpoint id isn't 64 hex characters."));
    }
    let endpoint = body.endpoint_id.to_ascii_lowercase();
    let now = service.now();
    {
        let conn = service.db();
        let Some(email) = db::account_email(&conn, &claktory.account_id)? else {
            return Err(ApiError::new(StatusCode::FORBIDDEN, "revoked", "This Claktory's account is gone. Sign in on it again."));
        };
        drop(conn);
        // A review account's Claktory under any key but the review box's,
        // one signed in before the pin, say, gets no relay (#319, Fugo 46276).
        api::review_box_only(service, &email, &claktory.backend_key)?;
    }
    service.limit(&[(&format!("relay-token:{}", claktory.id), 86_400, TOKENS_PER_DAY)])?;
    // A new endpoint today counts toward the account's; one it had a token
    // for today already doesn't.
    let known = format!("relay-endpoint:{}:{endpoint}", claktory.account_id);
    if db::today(&service.db(), &known, now)? == 0 {
        service.limit(&[(&format!("relay-endpoints:{}", claktory.account_id), 86_400, ENDPOINTS_PER_DAY)])?;
        db::count_today(&service.db(), &known, now)?;
    }
    let (token, expires_at) = service.relay_tokens.issue(&endpoint, &claktory.account_id, now);
    tracing::info!(claktory = %claktory.id, endpoint = &endpoint[..8], "relay token issued");
    Ok(Reply::json(&RelayToken { token, expires_at }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The test vector the relay checks against too: a fixed signing key,
    /// payload and token.
    #[test]
    fn a_token_is_its_payload_and_signature_over_the_context() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let endpoint = "ab".repeat(32);
        let payload = protocol::relay_token_payload(&endpoint, &"0f".repeat(16), 1_795_183_320, 1_795_183_320 + 604_800);
        assert_eq!(
            payload,
            r#"{"v":1,"e":"abababababababababababababababababababababababababababababababab","a":"0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f","iat":1795183320,"exp":1795788120}"#
        );
        let token = protocol::sign_relay_token(&key, payload.as_bytes());
        assert_eq!(protocol::open_relay_token(&key.verifying_key(), &token).as_deref(), Some(payload.as_bytes()));
        // Over the context and the payload, not the payload alone.
        let (_, signature) = token.split_once('.').expect("two parts");
        let signature = ed25519_dalek::Signature::from_slice(&protocol::unb64(signature).expect("b64")).expect("a signature");
        assert!(key.verifying_key().verify_strict(payload.as_bytes(), &signature).is_err());
        assert_eq!(hex(key.verifying_key().to_bytes()), "ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c");
        assert_eq!(token, VECTOR);
    }

    /// Ed25519 signs deterministically: this token, every time.
    const VECTOR: &str = "eyJ2IjoxLCJlIjoiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYiIsImEiOiIwZjBmMGYwZjBmMGYwZjBmMGYwZjBmMGYwZjBmMGYwZiIsImlhdCI6MTc5NTE4MzMyMCwiZXhwIjoxNzk1Nzg4MTIwfQ.8Nxh_AlyH94f0DFSTGxUGAEBPCUhNMDl9WYu8kboyeC1kXtP5iQnLaUcsVUOAeaYWQAalSVasysxRNU_Jtt0CQ";

    #[test]
    fn keys_come_from_k_share_and_an_account_s_id_is_opaque() {
        let keys = Keys::derive(&[1; 32]);
        assert_eq!(keys.public_hex(), Keys::derive(&[1; 32]).public_hex(), "the same every start");
        assert_ne!(keys.public_hex(), Keys::derive(&[2; 32]).public_hex());
        let a = keys.account("account-1");
        assert_eq!((a.len(), a.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())), (32, true));
        assert_ne!(a, keys.account("account-2"));
        assert!(!a.contains("account"));
    }
}
