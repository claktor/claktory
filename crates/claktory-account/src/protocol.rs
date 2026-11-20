//! The account service's API (API.md) as types, and its cryptography: what
//! a Claktory and the service both need, with no server in it.

use std::fmt::Write as _;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use curve25519_dalek::montgomery::MontgomeryPoint;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// The API's version, in `/.well-known/claktory-account`.
pub const API: u32 = 1;
/// Where a Claktory finds the service's key.
pub const WELL_KNOWN: &str = "/.well-known/claktory-account";
/// The header with a request's time, in unix seconds.
pub const TIME_HEADER: &str = "Claktory-Time";
/// The header with a request's or an answer's signature.
pub const SIGNATURE_HEADER: &str = "Claktory-Signature";
/// How far a signed request's time may be from the service's, in seconds.
pub const MAX_SKEW: i64 = 300;
/// How long an emailed code works, in seconds.
pub const CODE_LIFETIME: i64 = 15 * 60;
/// Wrong codes a sign-in may have before it must start again.
pub const CODE_TRIES: u32 = 5;
/// A Claktory's name, at most, in characters.
pub const NAME_LIMIT: usize = 40;
/// The `Authorization` scheme of a device's signed calls: `Device <account
/// key, base64url>`, with `Claktory-Time` and `Claktory-Signature` as a
/// Claktory's.
pub const DEVICE_AUTH: &str = "Device";
/// How long a device assertion works, in seconds.
pub const ASSERTION_LIFETIME: i64 = 3600;
/// How far a device assertion's times may be from the verifier's clock.
pub const ASSERTION_SKEW: i64 = 60;

/// Base64url without padding, as the API writes binary values.
pub fn b64(bytes: impl AsRef<[u8]>) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn unb64(text: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(text).ok()
}

/// 32 bytes from base64url, as keys are.
pub fn unb64_32(text: &str) -> Option<[u8; 32]> {
    unb64(text)?.try_into().ok()
}

/// Lower-case hex.
pub fn hex(bytes: impl AsRef<[u8]>) -> String {
    let mut out = String::with_capacity(bytes.as_ref().len() * 2);
    for byte in bytes.as_ref() {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

pub fn sha256(bytes: impl AsRef<[u8]>) -> [u8; 32] {
    Sha256::digest(bytes.as_ref()).into()
}

/// What a Claktory's backend key signs for a request.
pub fn request_message(method: &str, path_and_query: &str, time: i64, body: &[u8]) -> Vec<u8> {
    format!("claktory-account request v1\n{method}\n{path_and_query}\n{time}\n{}", hex(sha256(body))).into_bytes()
}

/// What the service key signs for an answer: its status, and tied to the
/// request it answers.
pub fn response_message(status: u16, method: &str, path_and_query: &str, request_body: &[u8], response_body: &[u8]) -> Vec<u8> {
    format!(
        "claktory-account response v1\n{status}\n{method}\n{path_and_query}\n{}\n{}",
        hex(sha256(request_body)),
        hex(sha256(response_body))
    )
    .into_bytes()
}

/// A request's `Claktory-Signature`.
pub fn sign_request(key: &SigningKey, method: &str, path_and_query: &str, time: i64, body: &[u8]) -> String {
    b64(key.sign(&request_message(method, path_and_query, time, body)).to_bytes())
}

/// Whether `signature` (base64url) is `key`'s over `message`.
pub fn verify(key: &VerifyingKey, message: &[u8], signature: &str) -> bool {
    let Some(bytes) = unb64(signature).and_then(|bytes| <[u8; 64]>::try_from(bytes).ok()) else { return false };
    key.verify_strict(message, &Signature::from_bytes(&bytes)).is_ok()
}

/// Whether an answer is the service's: its `Claktory-Signature` checked
/// against the pinned service key.
pub fn verify_response(
    service_key: &VerifyingKey,
    status: u16,
    method: &str,
    path_and_query: &str,
    request_body: &[u8],
    response_body: &[u8],
    signature: &str,
) -> bool {
    verify(service_key, &response_message(status, method, path_and_query, request_body, response_body), signature)
}

/// A key from its base64url form.
pub fn verifying_key(text: &str) -> Option<VerifyingKey> {
    VerifyingKey::from_bytes(&unb64_32(text)?).ok()
}

/// What binds a service share to its Claktory and epoch.
pub fn share_label(claktory_id: &str, epoch: u64) -> Vec<u8> {
    let mut label = b"claktory share v1\0".to_vec();
    label.extend_from_slice(claktory_id.as_bytes());
    label.push(0);
    label.extend_from_slice(&epoch.to_be_bytes());
    label
}

/// The service share of a Claktory at an epoch: HMAC-SHA256(K_share, label).
/// The service derives it when asked and never stores it.
pub fn derive_share(k_share: &[u8; 32], claktory_id: &str, epoch: u64) -> Zeroizing<[u8; 32]> {
    let mut mac = <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(k_share).expect("any key length");
    mac.update(&share_label(claktory_id, epoch));
    Zeroizing::new(mac.finalize().into_bytes().into())
}

/// 32 random bytes.
pub fn random_32() -> Zeroizing<[u8; 32]> {
    let mut bytes = Zeroizing::new([0u8; 32]);
    getrandom::fill(bytes.as_mut()).expect("the system's random source");
    bytes
}

fn seal_key(shared: &MontgomeryPoint, sender: &[u8; 32], receiver: &[u8; 32]) -> Option<Zeroizing<[u8; 32]>> {
    // A low-order point gives an all-zero secret anyone knows.
    if shared.as_bytes().iter().all(|byte| *byte == 0) {
        return None;
    }
    let mut salt = [0u8; 64];
    salt[..32].copy_from_slice(sender);
    salt[32..].copy_from_slice(receiver);
    let mut key = Zeroizing::new([0u8; 32]);
    hkdf::Hkdf::<Sha256>::new(Some(&salt), shared.as_bytes()).expand(b"claktory share seal v1", key.as_mut()).ok()?;
    Some(key)
}

/// Seals a share to a Claktory's one-time X25519 key: base64url of the
/// service's one-time public key (32 bytes), the nonce (24) and the
/// ciphertext. None when `eph_pub` is a key no one should seal to.
pub fn seal_share(share: &[u8; 32], eph_pub: &[u8; 32], claktory_id: &str, epoch: u64) -> Option<String> {
    let secret = random_32();
    let epk = MontgomeryPoint::mul_base_clamped(*secret).to_bytes();
    let key = seal_key(&MontgomeryPoint(*eph_pub).mul_clamped(*secret), &epk, eph_pub)?;
    let mut nonce = [0u8; 24];
    getrandom::fill(&mut nonce).expect("the system's random source");
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref()).ok()?;
    let aad = share_label(claktory_id, epoch);
    let sealed = cipher.encrypt(&XNonce::from(nonce), Payload { msg: share, aad: &aad }).ok()?;
    let mut out = Vec::with_capacity(32 + 24 + sealed.len());
    out.extend_from_slice(&epk);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    Some(b64(out))
}

/// A one-time X25519 key a Claktory makes to receive its share.
pub struct Ephemeral {
    secret: Zeroizing<[u8; 32]>,
    /// For the request's `eph_pub`.
    pub public: [u8; 32],
}

impl Default for Ephemeral {
    fn default() -> Ephemeral {
        Ephemeral::new()
    }
}

impl Ephemeral {
    pub fn new() -> Ephemeral {
        let secret = random_32();
        let public = MontgomeryPoint::mul_base_clamped(*secret).to_bytes();
        Ephemeral { secret, public }
    }

    /// `eph_pub`, base64url.
    pub fn public_b64(&self) -> String {
        b64(self.public)
    }

    /// Opens a `sealed_share` sealed to this key for `claktory_id` at `epoch`.
    pub fn open(&self, sealed_share: &str, claktory_id: &str, epoch: u64) -> Option<Zeroizing<[u8; 32]>> {
        let sealed = unb64(sealed_share)?;
        if sealed.len() < 32 + 24 + 16 {
            return None;
        }
        let (epk, rest) = sealed.split_at(32);
        let (nonce, ciphertext) = rest.split_at(24);
        let epk: [u8; 32] = epk.try_into().ok()?;
        let key = seal_key(&MontgomeryPoint(epk).mul_clamped(*self.secret), &epk, &self.public)?;
        let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref()).ok()?;
        let nonce: [u8; 24] = nonce.try_into().ok()?;
        let aad = share_label(claktory_id, epoch);
        let opened = Zeroizing::new(cipher.decrypt(&XNonce::from(nonce), Payload { msg: ciphertext, aad: &aad }).ok()?);
        opened.as_slice().try_into().ok().map(Zeroizing::new)
    }
}

/// Whether a Claktory id is well formed: 16 to 64 of `A-Z a-z 0-9 _ -`.
pub fn valid_claktory_id(id: &str) -> bool {
    (16..=64).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Whether an iroh endpoint id is well formed: 64 hex characters.
pub fn valid_endpoint(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// A version or platform: 1 to 40 of `A-Z a-z 0-9 . _ + -`.
pub fn valid_label(text: &str) -> bool {
    (1..=40).contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
}

/// An email address in the form accounts keep: trimmed, in lower case, and
/// roughly well formed. None when it isn't an address.
pub fn normalize_email(email: &str) -> Option<String> {
    let email = email.trim().to_lowercase();
    let (local, domain) = email.split_once('@')?;
    let fine = !local.is_empty()
        && email.len() <= 254
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains("..")
        && !email.chars().any(|character| character.is_whitespace() || character.is_control() || "<>()[]\\,;:\"".contains(character))
        && !domain.contains('@');
    fine.then_some(email)
}

/// A Claktory's name as the service shows it, in emails too: no control
/// characters, nothing that looks like a link, at most `NAME_LIMIT`
/// characters, and never empty.
pub fn clean_name(name: &str) -> String {
    let words: Vec<&str> = name
        .split_whitespace()
        .filter(|word| {
            let lower = word.to_lowercase();
            !(lower.contains("://") || lower.starts_with("www.") || lower.contains('@') || looks_like_domain(&lower))
        })
        .collect();
    let joined: String = words.join(" ").chars().filter(|character| !character.is_control() && !"<>".contains(*character)).collect();
    let cut: String = joined.chars().take(NAME_LIMIT).collect();
    let cut = cut.trim();
    if cut.is_empty() { "Claktory".to_owned() } else { cut.to_owned() }
}

/// "example.com", "evil.co/x": a word a mail program would make a link of.
fn looks_like_domain(word: &str) -> bool {
    let word = word.trim_matches(|character: char| !character.is_alphanumeric());
    let host = word.split('/').next().unwrap_or_default();
    let mut labels = host.split('.');
    let parts: Vec<&str> = labels.by_ref().collect();
    parts.len() >= 2
        && parts.iter().all(|label| !label.is_empty())
        && parts.last().is_some_and(|tld| tld.len() >= 2 && tld.chars().all(|character| character.is_ascii_alphabetic()))
}

/// `GET /.well-known/claktory-account`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WellKnown {
    pub service: String,
    pub api: u32,
    pub version: String,
    pub service_key: String,
}

/// `POST /v1/sign-in/start`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignInStart {
    pub email: String,
    pub claktory_id: String,
    pub name: String,
    pub backend_key: String,
    pub version: String,
    pub platform: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignInStarted {
    pub request_id: String,
    pub expires_in: i64,
    /// The address the code comes from ("code@auth.claktory.com"), so the
    /// app can say what to look for; empty from a service before it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub code_from: String,
}

/// `POST /v1/sign-in/verify`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignInVerify {
    pub request_id: String,
    pub code: String,
    pub eph_pub: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignedIn {
    pub account_id: String,
    pub email: String,
    pub claktory_id: String,
    pub machine_token: String,
    pub epoch: u64,
    pub sealed_share: String,
    pub service_key: String,
}

/// `POST /v1/share`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShareRequest {
    pub eph_pub: String,
    /// The share of this epoch rather than the current one: the current
    /// one or the one before it only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Share {
    pub epoch: u64,
    pub sealed_share: String,
}

/// `POST /v1/check-in`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CheckIn {
    pub version: String,
    pub platform: String,
    /// How many there are now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counts: Option<Counts>,
    /// What happened since the last accepted check-in, in periods the
    /// Claktory keeps until one is accepted; a period sent again is
    /// recorded once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub periods: Vec<Period>,
}

/// A Claktory's agents, projects and paired devices, at a check-in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub agents: u32,
    pub projects: u32,
    pub devices: u32,
}

/// A stretch of time a check-in reports on.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Period {
    /// Made by the Claktory, unique among its periods: 16 to 64 of
    /// `A-Z a-z 0-9 _ -`. A resent period has the same id.
    pub id: String,
    /// Unix seconds, `from` < `to`.
    pub from: i64,
    pub to: i64,
    /// Model calls in the period, one entry per provider and model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub usage: Vec<Usage>,
    /// Agents' work in the period, per UTC hour.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub activity: Vec<Activity>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// "anthropic", "openai-codex"…
    pub provider: String,
    /// "claude-opus-4-8"…
    pub model: String,
    /// The model's name as the app shows it ("Claude Opus 4.8"), for the
    /// account page; 1 to 100 characters, no control characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    /// The hour's start, unix seconds (a multiple of 3600).
    pub hour: i64,
    /// Minutes agents spent working in the hour, summed over agents.
    pub agent_minutes: u32,
    /// The most agents working at once in the hour.
    pub agents_working: u32,
    /// Runs (a prompt or event through to the agent's reply) started in the hour.
    pub runs: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CheckedIn {
    pub relays: Vec<String>,
    pub server_time: i64,
    /// The Claktory's current epoch, so a backend notices one it missed.
    pub epoch: u64,
    /// Periods of this check-in recorded now (not those recorded before).
    #[serde(default)]
    pub periods_recorded: u32,
    /// Periods of this check-in left out as out of range (too old, in the
    /// future, or with a value no Claktory could have): never send them again.
    #[serde(default)]
    pub periods_ignored: u32,
}

/// `GET /v1/stats/public`: for a counter on claktory.com.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PublicStats {
    /// Claktories signed in (not removed).
    pub claktories: u64,
    /// Tokens all Claktories have used: input, output, cache reads and
    /// cache writes, the four below.
    pub tokens_total: u64,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
    /// Since when, unix seconds: the first account's sign-in.
    pub since: i64,
}

/// Whether a provider or model name is one the service keeps: 1 to 100 of
/// `A-Z a-z 0-9 . _ : / @ + -`.
pub fn valid_model(text: &str) -> bool {
    (1..=100).contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._:/@+-".contains(&byte))
}

/// `PUT /v1/endpoints`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Endpoints {
    pub backend: String,
    #[serde(default)]
    pub devices: Vec<String>,
    /// The Claktory's own name and color (its Settings), and the relays its
    /// pairing codes give, for devices that find it through the account
    /// (`GET /v1/claktories`). Absent from Claktories before them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relays: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EndpointsSet {
    pub count: usize,
}

/// `GET /v1/relay/allowlist`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Allowlist {
    pub version: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub endpoints: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unchanged: bool,
}

/// Every error's body; extras such as `attempts_left` sit beside these.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempts_left: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_time: Option<i64>,
}

/// A new Ed25519 key, such as a Claktory's backend key.
pub fn new_signing_key() -> SigningKey {
    SigningKey::from_bytes(&random_32())
}

/// `POST /v1/devices/sign-in/start`: a device (an app) signs in to the
/// account with its account key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceSignInStart {
    pub email: String,
    /// The device's account key (Ed25519 public, base64url): every later
    /// call is signed with it, and the code works only for it.
    pub account_key: String,
    /// The device's name, as the email says it: "Matt's iPhone".
    pub name: String,
    pub platform: String,
    pub version: String,
}

/// `POST /v1/devices/sign-in/verify`, signed by the account key that started it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceSignInVerify {
    pub request_id: String,
    pub code: String,
}

/// The answer to `sign-in/verify` and `GET /v1/devices/me`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceAccount {
    pub account_id: String,
    pub email: String,
    /// The device's name, as it signed in.
    #[serde(default)]
    pub name: String,
}

/// `GET /v1/claktories`: the account's Claktories, newest contact first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClaktoryList {
    pub claktories: Vec<ListedClaktory>,
}

/// A Claktory as a device finds it, to connect to it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ListedClaktory {
    pub claktory_id: String,
    /// Its own name and color (its Settings); its computer's name and ""
    /// until it has sent them.
    pub name: String,
    pub color: String,
    /// The computer's name, as sent at sign-in.
    pub machine: String,
    pub platform: String,
    pub version: String,
    /// Its tunnel endpoint id (64 hex) and relays, as its pairing codes give
    /// them (e= and r=); empty when it never registered one.
    pub endpoint_id: String,
    pub relays: Vec<String>,
    /// When it was last heard from, in ms since 1970.
    pub last_contact_ms: i64,
}

/// `POST /v1/devices/assertion`, signed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssertionRequest {
    /// The tunnel endpoint id (64 hex) the device will connect with.
    pub device_key: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssertionIssued {
    pub assertion: String,
    pub expires_in: i64,
}

/// What a device assertion says: the service's word that whoever holds the
/// tunnel key `device_key` is signed in to `account_id` (through the device
/// with `account_key`), from `iat` to `exp` (unix seconds).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceAssertion {
    pub v: u32,
    pub account_id: String,
    pub account_key: String,
    pub device_key: String,
    pub iat: i64,
    pub exp: i64,
}

/// What a device assertion's signature covers, before its payload.
const ASSERTION_LABEL: &[u8] = b"claktory device assertion v1\n";

/// A device assertion, signed with the service key: `b64(payload) "."
/// b64(signature)`, the signature over `ASSERTION_LABEL ‖ payload`.
pub fn sign_device_assertion(service_key: &SigningKey, assertion: &DeviceAssertion) -> String {
    let payload = serde_json::to_vec(assertion).unwrap_or_default();
    let signature = service_key.sign(&[ASSERTION_LABEL, &payload].concat());
    format!("{}.{}", b64(&payload), b64(signature.to_bytes()))
}

/// What a device assertion says, if `service_key` signed it and it's
/// version 1, whenever it works: `verify_device_assertion` checks that too.
pub fn read_device_assertion(service_key: &VerifyingKey, text: &str) -> Option<DeviceAssertion> {
    let (payload, signature) = text.trim().split_once('.')?;
    let payload = unb64(payload)?;
    if !verify(service_key, &[ASSERTION_LABEL, &payload].concat(), signature) {
        return None;
    }
    let assertion: DeviceAssertion = serde_json::from_slice(&payload).ok()?;
    (assertion.v == 1).then_some(assertion)
}

/// What a device assertion says, if `service_key` signed it, it's version 1,
/// and it works at `now` (unix seconds, give or take `ASSERTION_SKEW`).
/// Whether its account and device key are the ones expected is the
/// caller's to check.
pub fn verify_device_assertion(service_key: &VerifyingKey, text: &str, now: i64) -> Option<DeviceAssertion> {
    read_device_assertion(service_key, text)
        .filter(|assertion| assertion.iat <= now + ASSERTION_SKEW && now <= assertion.exp + ASSERTION_SKEW)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sealed_share_opens_only_for_its_key_claktory_and_epoch() {
        let share = derive_share(&[7u8; 32], "claktory-0123456789", 3);
        assert_eq!(*share, *derive_share(&[7u8; 32], "claktory-0123456789", 3), "derived, not random");
        assert_ne!(*share, *derive_share(&[7u8; 32], "claktory-0123456789", 4), "each epoch its own");
        let ephemeral = Ephemeral::new();
        let sealed = seal_share(&share, &ephemeral.public, "claktory-0123456789", 3).expect("sealed");
        assert_eq!(*ephemeral.open(&sealed, "claktory-0123456789", 3).expect("opens"), *share);
        assert!(ephemeral.open(&sealed, "claktory-0123456789", 4).is_none(), "another epoch");
        assert!(ephemeral.open(&sealed, "claktory-9876543210", 3).is_none(), "another Claktory");
        assert!(Ephemeral::new().open(&sealed, "claktory-0123456789", 3).is_none(), "another key");
        assert!(seal_share(&share, &[0u8; 32], "claktory-0123456789", 3).is_none(), "a low-order key");
    }

    #[test]
    fn signatures_bind_the_whole_request() {
        let key = new_signing_key();
        let signature = sign_request(&key, "POST", "/v1/share", 100, b"{}");
        assert!(verify(&key.verifying_key(), &request_message("POST", "/v1/share", 100, b"{}"), &signature));
        for (method, path, time, body) in [
            ("PUT", "/v1/share", 100, &b"{}"[..]),
            ("POST", "/v1/check-in", 100, b"{}"),
            ("POST", "/v1/share", 101, b"{}"),
            ("POST", "/v1/share", 100, b"{ }"),
        ] {
            assert!(!verify(&key.verifying_key(), &request_message(method, path, time, body), &signature));
        }
    }

    #[test]
    fn a_device_assertion_holds_only_with_the_service_key_and_while_it_works() {
        let service = new_signing_key();
        let said = DeviceAssertion {
            v: 1,
            account_id: "acct".into(),
            account_key: b64([7u8; 32]),
            device_key: "ab".repeat(32),
            iat: 1000,
            exp: 1000 + ASSERTION_LIFETIME,
        };
        let text = sign_device_assertion(&service, &said);
        assert_eq!(verify_device_assertion(&service.verifying_key(), &text, 1500), Some(said.clone()));
        assert_eq!(
            verify_device_assertion(&service.verifying_key(), &text, 1000 + ASSERTION_LIFETIME + ASSERTION_SKEW),
            Some(said.clone())
        );
        assert_eq!(
            verify_device_assertion(&service.verifying_key(), &text, 1000 + ASSERTION_LIFETIME + ASSERTION_SKEW + 1),
            None,
            "expired"
        );
        assert_eq!(read_device_assertion(&service.verifying_key(), &text), Some(said.clone()), "still readable");
        assert_eq!(verify_device_assertion(&service.verifying_key(), &text, 900), None, "not yet");
        assert_eq!(verify_device_assertion(&new_signing_key().verifying_key(), &text, 1500), None, "another key");
        let (payload, signature) = text.split_once('.').expect("two parts");
        let mut changed = said.clone();
        changed.account_id = "other".into();
        let forged = format!("{}.{signature}", b64(serde_json::to_vec(&changed).expect("json")));
        assert_eq!(verify_device_assertion(&service.verifying_key(), &forged, 1500), None, "changed");
        let mut later = said;
        later.v = 2;
        assert_eq!(
            read_device_assertion(&service.verifying_key(), &sign_device_assertion(&service, &later)),
            None,
            "a version it doesn't know"
        );
        assert!(unb64(payload).is_some());
    }

    #[test]
    fn names_and_emails_are_kept_tidy() {
        assert_eq!(clean_name("Mac mini"), "Mac mini");
        assert_eq!(clean_name("Click https://evil.example/x now"), "Click now");
        assert_eq!(clean_name("visit evil.com or www.x.org"), "visit or");
        assert_eq!(clean_name("v0.1.9 build"), "v0.1.9 build", "a version isn't a domain");
        assert_eq!(clean_name("a\u{7}b<c>"), "abc");
        assert_eq!(clean_name(&"x".repeat(60)).len(), NAME_LIMIT);
        assert_eq!(clean_name("  "), "Claktory");
        assert_eq!(normalize_email(" Matt@Example.COM "), Some("matt@example.com".into()));
        for bad in ["matt", "matt@", "@example.com", "matt@example", "a b@example.com", "m@exa..mple.com", "m@a@b.com"] {
            assert_eq!(normalize_email(bad), None, "{bad}");
        }
        assert!(valid_claktory_id("abcdefgh_ijkl-mnop"));
        assert!(!valid_claktory_id("short") && !valid_claktory_id("has a space in it ok"));
    }
}
