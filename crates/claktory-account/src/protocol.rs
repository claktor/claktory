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

/// A Unicode format character (category Cf): invisible, it changes how
/// text around it reads (direction overrides such as U+202E, zero-width
/// joiners and spaces, tags), so no email may carry one.
pub fn format_character(character: char) -> bool {
    matches!(
        character,
        '\u{00AD}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061C}'
            | '\u{06DD}'
            | '\u{070F}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08E2}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{110BD}'
            | '\u{110CD}'
            | '\u{13430}'..='\u{1343F}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

/// Whether an email is printable ASCII only, as the ones pipe peer
/// statements vouch for must be: no look-alike letters from other scripts,
/// nothing that reverses or hides, so the address shown is the one owned.
pub fn ascii_email(email: &str) -> bool {
    !email.is_empty() && email.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
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
        && !email.chars().any(|character| {
            character.is_whitespace() || character.is_control() || format_character(character) || "<>()[]\\,;:\"".contains(character)
        })
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
    /// An offline root's word that `service_key` is the service's
    /// (`KeyStatement`); left out by a service that has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_key_statement: Option<SignedStatement>,
}

/// The first line of a service-key statement.
pub const STATEMENT_HEAD: &str = "claktory-account-service-key/1";

/// The offline keys that vouch for an account service's signing key, by id:
/// raw ed25519 public keys, in standard base64 (as OpenSSL writes them).
/// The account root, taken out at each rotation of the service's key, and
/// the release recovery key as its backup. Never the release working key
/// (`claktory-release-1`), and the account root is never a release key, so
/// neither can stand in for the other (factoryd's tests hold both).
/// The account root is added once it's made.
pub const ACCOUNT_ROOTS: &[(&str, &str)] = &[("claktory-release-recovery-1", "aGdAkbnB7pkB/dRRfPnkFKfKyWzPs+cF7FVHMlb7JpM=")];

/// An offline root's word that `key` is `service`'s signing key from
/// `from` (unix seconds) on. Signed as exactly its `text()`: four lines,
/// each ending in LF, nothing else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyStatement {
    pub service: String,
    /// The service key as the API writes it (base64url, no padding).
    pub key: String,
    pub from: i64,
}

impl KeyStatement {
    pub fn text(&self) -> String {
        format!("{STATEMENT_HEAD}\nservice: {}\nkey: {}\nfrom: {}\n", self.service, self.key, self.from)
    }

    /// Reads exactly what `text()` writes, and nothing else: no CR, no
    /// spaces but the ones after each colon, a canonical 32-byte key, and
    /// `from` as digits with no sign or leading zero.
    pub fn parse(text: &str) -> Option<KeyStatement> {
        let body = text.strip_suffix('\n')?;
        let lines: Vec<&str> = body.split('\n').collect();
        let [head, service, key, from] = lines.as_slice() else { return None };
        if *head != STATEMENT_HEAD {
            return None;
        }
        let service = service.strip_prefix("service: ")?;
        if service.is_empty() || !service.chars().all(|character| character.is_ascii_graphic()) {
            return None;
        }
        let key = key.strip_prefix("key: ")?;
        if b64(unb64_32(key)?) != key {
            return None;
        }
        let from = from.strip_prefix("from: ")?;
        let digits = !from.is_empty() && from.len() <= 18 && from.bytes().all(|byte| byte.is_ascii_digit());
        if !digits || from.starts_with('0') {
            return None;
        }
        Some(KeyStatement { service: service.to_owned(), key: key.to_owned(), from: from.parse().ok()? })
    }
}

/// A statement and its signature, `<root id>:<base64 ed25519 signature>`
/// over the statement's exact bytes (as releases' `latest.json.sig`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedStatement {
    pub text: String,
    pub signature: String,
}

/// The statement, if one of `roots` signed exactly it and it reads as one.
pub fn verify_statement(signed: &SignedStatement, roots: &[(&str, &str)]) -> Result<KeyStatement, String> {
    use base64::engine::general_purpose::STANDARD;
    let (id, signature) = signed.signature.trim().split_once(':').ok_or_else(|| "its signature names no key".to_owned())?;
    let shown: String = id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(40).collect();
    let Some((_, root)) = roots.iter().find(|(known, _)| *known == id) else {
        return Err(format!("it's signed with a key that vouches for no service ({shown})"));
    };
    let root: [u8; 32] = STANDARD.decode(root).ok().and_then(|bytes| bytes.try_into().ok()).ok_or("a root isn't a key")?;
    let root = VerifyingKey::from_bytes(&root).map_err(|_| "a root isn't a key".to_owned())?;
    let bytes = STANDARD.decode(signature.trim()).map_err(|_| "its signature isn't base64".to_owned())?;
    let signature = Signature::from_slice(&bytes).map_err(|_| "its signature isn't an ed25519 one".to_owned())?;
    root.verify_strict(signed.text.as_bytes(), &signature).map_err(|_| format!("its signature by {shown} doesn't match it"))?;
    KeyStatement::parse(&signed.text).ok_or_else(|| "it isn't a service-key statement".to_owned())
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
    /// How far its setup got, each reached once and never undone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<Setup>,
    /// Its paired devices now (this computer's own app among them), counted
    /// by what they are: never named. None from Claktories before they
    /// counted them, which leaves the last list as it was; an empty list
    /// says there are none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub devices: Option<Vec<DeviceCount>>,
}

/// A Claktory's agents, projects and paired devices, at a check-in; newer
/// ones also its pipes, secrets and monitors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub agents: u32,
    pub projects: u32,
    pub devices: u32,
    #[serde(default)]
    pub pipes: u32,
    #[serde(default)]
    pub secrets: u32,
    /// Monitors active now.
    #[serde(default)]
    pub monitors: u32,
}

/// Steps of a Claktory's setup it has reached: true once, for good.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Setup {
    /// A model provider was signed in.
    #[serde(default)]
    pub model_connected: bool,
    /// An agent ran for the first time.
    #[serde(default)]
    pub first_run: bool,
}

/// How many of a Claktory's devices are one kind of device on one system
/// with one app version.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCount {
    /// "ios", "ipados", "android", "macos", "linux", "windows"; "unknown"
    /// for apps from before they said.
    pub platform: String,
    /// "desktop", "laptop", "phone", "tablet" or "unknown".
    pub kind: String,
    /// The app's version ("v0.1.16", "dev"); "unknown" from older apps.
    pub app_version: String,
    pub count: u32,
}

/// A backend version's health in a period: how often an update to it
/// didn't start and was rolled back by the supervisor, and how often it
/// stopped without stopping cleanly (a crash, or the computer losing power).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    /// "v0.1.16"; "unknown" when the supervisor couldn't tell.
    pub version: String,
    #[serde(default)]
    pub rolled_back: u32,
    #[serde(default)]
    pub crashes: u32,
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
    /// How many paired devices (this computer's own app among them)
    /// connected in the period.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub devices_connected: Option<u32>,
    /// Backend versions that were rolled back or crashed in the period.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub health: Vec<Health>,
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

/// `POST /v1/relay/token` (#450): a token for Claktory's relays, bound to
/// the Claktory's endpoint `endpoint_id` (64 hex characters).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RelayTokenRequest {
    pub endpoint_id: String,
}

/// A relay token and when it stops working (unix seconds). The Claktory
/// gives it to Claktory's relays as `Authorization: Bearer <token>`; it
/// fetches another before it's within 3 days of `expires_at`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RelayToken {
    pub token: String,
    pub expires_at: i64,
}

/// How long a relay token works, in seconds.
pub const RELAY_TOKEN_LIFETIME: i64 = 7 * 86_400;
/// What a relay token's signature covers before its payload, so the key
/// never signs anything another protocol could take for its own.
pub const RELAY_TOKEN_CONTEXT: &[u8] = b"claktory-relay-token-v1\0";

/// A relay token's payload, exactly as it's signed: JSON with `v` 1, the
/// endpoint `e` (64 lower-case hex), the opaque account `a` (32 lower-case
/// hex), and when it was made and stops working, in unix seconds.
pub fn relay_token_payload(endpoint: &str, account: &str, issued: i64, expires: i64) -> String {
    format!(r#"{{"v":1,"e":"{endpoint}","a":"{account}","iat":{issued},"exp":{expires}}}"#)
}

/// A relay token: base64url(payload) "." base64url(Ed25519 signature of
/// `RELAY_TOKEN_CONTEXT` and the payload's bytes), unpadded.
pub fn sign_relay_token(key: &SigningKey, payload: &[u8]) -> String {
    let mut message = RELAY_TOKEN_CONTEXT.to_vec();
    message.extend_from_slice(payload);
    format!("{}.{}", b64(payload), b64(key.sign(&message).to_bytes()))
}

/// A relay token's payload, if `key` signed it (its fields unchecked).
pub fn open_relay_token(key: &VerifyingKey, token: &str) -> Option<Vec<u8>> {
    let (payload, signature) = token.split_once('.')?;
    let (payload, signature) = (unb64(payload)?, unb64(signature)?);
    let signature = Signature::from_slice(&signature).ok()?;
    let mut message = RELAY_TOKEN_CONTEXT.to_vec();
    message.extend_from_slice(&payload);
    key.verify_strict(&message, &signature).ok()?;
    Some(payload)
}

/// `POST /v1/move`, signed with the old token and key: the moved Claktory's new ones.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MoveRequest {
    /// The new backend key (Ed25519 public, b64).
    pub backend_key: String,
    /// The new machine token: 32 random bytes, b64, chosen by the Claktory.
    pub machine_token: String,
    /// The new computer's name.
    pub name: String,
    pub version: String,
    pub platform: String,
    /// The old computer's name, for the email.
    #[serde(default)]
    pub from: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MoveDone {
    pub epoch: u64,
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

/// `POST /v1/devices/vouch` (#529), signed by a signed-in Claktory: it
/// vouches for the app on its own computer, which then signs in as a device
/// with the code it's given, no email sent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceVouch {
    /// The app's account key (Ed25519 public, base64url), as sign-in/start's:
    /// the code works only for it.
    pub account_key: String,
    /// The device's name: "Matt's MacBook Pro".
    pub name: String,
    pub platform: String,
    pub version: String,
}

/// The answer to `POST /v1/devices/vouch`: what the app gives
/// sign-in/verify, signed by the key vouched for. The code works once,
/// for `expires_in` seconds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceVouched {
    pub request_id: String,
    pub code: String,
    pub expires_in: i64,
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

/// `POST /v1/devices/remove`, signed: another device of the same account
/// to sign out (lost, stolen, given away).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceRemove {
    /// The device's account key (its DM leaf id).
    pub account_key: String,
}

/// What `POST /v1/devices/remove` did: false when that key wasn't (or is
/// no longer) a device of this account, so a retry is no error either.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceRemoved {
    pub removed: bool,
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

/// What a pipe peer statement says it is, in its payload and before it in
/// what the service key signs.
pub const PIPE_PEER_LABEL: &str = "claktory-pipe-peer/1";
/// How long a pipe peer statement works: a week, refreshed over the pipe
/// long before. A Claktory signed out, removed or whose account is deleted
/// gets no new one, so its peers' verification ends by expiry.
pub const PIPE_PEER_LIFETIME: i64 = 7 * 86_400;
/// At most this many bytes in a pipe peer statement; a longer one isn't read.
pub const PIPE_PEER_MAX: usize = 4096;

/// `POST /v1/pipe-identity`, signed: the service's word on whose Claktory
/// holds the pipe key `endpoint_id`, for the Claktories it has pipes with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PipeIdentityRequest {
    /// The Claktory's tunnel key (iroh endpoint id, 64 hex): its registered
    /// `backend` endpoint, the key its pipes pin.
    pub endpoint_id: String,
    /// Unix seconds, within `MAX_SKEW` of the service's clock.
    pub time: i64,
    /// The endpoint key's own signature over `pipe_identity_proof(..)`
    /// (base64url): it holds the key it asks about.
    pub proof: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PipeIdentityIssued {
    pub statement: String,
    pub expires_in: i64,
}

/// What the endpoint key signs for `POST /v1/pipe-identity`: tied to the
/// Claktory asking and the time, so it serves no other Claktory or later.
pub fn pipe_identity_proof(claktory_id: &str, endpoint_id: &str, time: i64) -> Vec<u8> {
    format!("{PIPE_PEER_LABEL} proof\n{claktory_id}\n{endpoint_id}\n{time}").into_bytes()
}

/// What a pipe peer statement says: the service at `service`, with the key
/// `service_key`, vouches that whoever holds the tunnel key `endpoint_id`
/// is a Claktory of the account `account_id`, whose email (confirmed with a
/// code at sign-in) is `email`, from `iat` to `exp` (unix seconds). Nothing
/// in it is what the Claktory merely says, such as names; and it vouches
/// for the Claktory's account, not for the person.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipePeerStatement {
    pub v: u32,
    pub label: String,
    pub service: String,
    /// The service key that signs it, as the API writes keys (base64url).
    pub service_key: String,
    pub endpoint_id: String,
    pub account_id: String,
    pub email: String,
    pub iat: i64,
    pub exp: i64,
}

/// A pipe peer statement, signed with the service key: `b64(payload) "."
/// b64(signature)`, the signature over `PIPE_PEER_LABEL "\n" ‖ payload`.
pub fn sign_pipe_peer(service_key: &SigningKey, statement: &PipePeerStatement) -> String {
    let payload = serde_json::to_vec(statement).unwrap_or_default();
    let signature = service_key.sign(&pipe_peer_message(&payload));
    format!("{}.{}", b64(&payload), b64(signature.to_bytes()))
}

fn pipe_peer_message(payload: &[u8]) -> Vec<u8> {
    [PIPE_PEER_LABEL.as_bytes(), b"\n", payload].concat()
}

/// What a pipe peer statement says, if `service_key` signed it, names that
/// key and its label, is version 1, is about `endpoint_id` (the pipe's
/// pinned peer key, hex in either case), lasts no longer than
/// `PIPE_PEER_LIFETIME` and works at `now` (unix seconds, give or take
/// `ASSERTION_SKEW`). A statement for any other key is worth nothing.
pub fn verify_pipe_peer(service_key: &VerifyingKey, text: &str, endpoint_id: &str, now: i64) -> Option<PipePeerStatement> {
    if text.len() > PIPE_PEER_MAX {
        return None;
    }
    let (payload, signature) = text.trim().split_once('.')?;
    let payload = unb64(payload)?;
    if !verify(service_key, &pipe_peer_message(&payload), signature) {
        return None;
    }
    let statement: PipePeerStatement = serde_json::from_slice(&payload).ok()?;
    let fine = statement.v == 1
        && statement.label == PIPE_PEER_LABEL
        && ascii_email(&statement.email)
        && unb64_32(&statement.service_key) == Some(service_key.to_bytes())
        && valid_endpoint(&statement.endpoint_id)
        && statement.endpoint_id.eq_ignore_ascii_case(endpoint_id)
        && statement.exp >= statement.iat
        && statement.exp - statement.iat <= PIPE_PEER_LIFETIME
        && statement.iat <= now + ASSERTION_SKEW
        && now <= statement.exp + ASSERTION_SKEW;
    fine.then_some(statement)
}

/// Where reports of AI replies go.
pub const AI_REPORT_PATH: &str = "/v1/reports/ai";

/// `POST /v1/reports/ai`: a reply a person reports to Claktory's team
/// (Report this reply). Exactly what their sheet showed, nothing more:
/// no account, Claktory, agent or device.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiReport {
    /// The reply, the vault's values hidden, at most `AI_REPORT_TEXT_MAX` bytes.
    pub text: String,
    /// The model's name ("Claude Opus 4.5") or id, up to 100 characters.
    pub model: String,
    /// The Claktory's version and platform ("macos-arm64"), up to 40 each.
    pub version: String,
    pub platform: String,
    /// The person's note, `ai_report_note` clean, at most
    /// `AI_REPORT_NOTE_MAX` characters; may be empty.
    #[serde(default)]
    pub note: String,
}

/// The answer: kept, for this many days.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiReportKept {
    pub keep_days: i64,
}

/// A reported reply is cut to this many bytes (16 KB).
pub const AI_REPORT_TEXT_MAX: usize = 16 * 1024;
/// A report's note, at most this many characters.
pub const AI_REPORT_NOTE_MAX: usize = 1000;
/// How long the service keeps a report.
pub const AI_REPORT_KEEP_DAYS: i64 = 30;

/// `text` cut to `AI_REPORT_TEXT_MAX` bytes on a character's boundary, and
/// whether it was cut.
pub fn ai_report_text(text: &str) -> (&str, bool) {
    if text.len() <= AI_REPORT_TEXT_MAX {
        return (text, false);
    }
    let mut end = AI_REPORT_TEXT_MAX;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

/// A report's note as it's sent: line breaks as `\n`, no control
/// characters but line breaks and tabs, no format characters (direction
/// overrides, zero-width ones), trimmed. Its length is checked after.
pub fn ai_report_note(note: &str) -> String {
    let note = note.replace("\r\n", "\n").replace('\r', "\n");
    let kept: String = note
        .chars()
        .filter(|character| matches!(character, '\n' | '\t') || !(character.is_control() || format_character(*character)))
        .collect();
    kept.trim().to_owned()
}

/// Whether a report holds what the service keeps: a text within its cap,
/// a clean note within its, and short single-line model, version and
/// platform.
pub fn valid_ai_report(report: &AiReport) -> bool {
    let line = |text: &str, max: usize| {
        !text.trim().is_empty()
            && text.chars().count() <= max
            && !text.chars().any(|character| character.is_control() || format_character(character))
    };
    !report.text.trim().is_empty()
        && report.text.len() <= AI_REPORT_TEXT_MAX
        && ai_report_note(&report.note) == report.note
        && report.note.chars().count() <= AI_REPORT_NOTE_MAX
        && line(&report.model, 100)
        && line(&report.version, 40)
        && line(&report.platform, 40)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_reports_are_cut_and_cleaned_alike_on_both_sides() {
        let long = "é".repeat(AI_REPORT_TEXT_MAX);
        let (cut, truncated) = ai_report_text(&long);
        assert!(truncated && cut.len() == AI_REPORT_TEXT_MAX && cut.chars().all(|character| character == 'é'));
        let odd = format!("a{}", "é".repeat(AI_REPORT_TEXT_MAX / 2));
        let (cut, truncated) = ai_report_text(&odd);
        assert!(truncated && cut.len() == AI_REPORT_TEXT_MAX - 1);
        assert_eq!(ai_report_text("short"), ("short", false));
        assert_eq!(ai_report_note("  It said\r\nthis\u{202E}\u{7}\tthing.  "), "It said\nthis\tthing.");
        let report = |note: &str| AiReport {
            text: "Hello".into(),
            model: "Claude Opus 4.5".into(),
            version: "v0.1.9".into(),
            platform: "macos-arm64".into(),
            note: note.into(),
        };
        assert!(valid_ai_report(&report("")) && valid_ai_report(&report("Rude.\nVery.")));
        assert!(!valid_ai_report(&report(" padded")) && !valid_ai_report(&report("\u{202E}evil")));
        assert!(!valid_ai_report(&report(&"x".repeat(AI_REPORT_NOTE_MAX + 1))));
        assert!(!valid_ai_report(&AiReport { model: "a\nb".into(), ..report("") }));
        assert!(!valid_ai_report(&AiReport { text: " ".into(), ..report("") }));
        assert!(serde_json::from_str::<AiReport>(r#"{"text":"t","model":"m","version":"v","platform":"p","account":"x"}"#).is_err());
    }

    fn statement() -> KeyStatement {
        let key = SigningKey::from_bytes(&[7; 32]);
        KeyStatement { service: "https://account.claktory.com".into(), key: b64(key.verifying_key().to_bytes()), from: 1_795_000_000 }
    }

    /// A root of the test's own, and a statement signed by it.
    fn signed(text: &str) -> (SignedStatement, Vec<(String, String)>) {
        use base64::engine::general_purpose::STANDARD;
        let root = SigningKey::from_bytes(&[9; 32]);
        let signature = format!("test-root:{}", STANDARD.encode(root.sign(text.as_bytes()).to_bytes()));
        let roots = vec![("test-root".to_owned(), STANDARD.encode(root.verifying_key().to_bytes()))];
        (SignedStatement { text: text.to_owned(), signature }, roots)
    }

    fn roots(owned: &[(String, String)]) -> Vec<(&str, &str)> {
        owned.iter().map(|(id, key)| (id.as_str(), key.as_str())).collect()
    }

    #[test]
    fn a_statement_reads_back_exactly_and_nothing_near_it_does() {
        let good = statement();
        assert_eq!(KeyStatement::parse(&good.text()), Some(good.clone()));
        let text = good.text();
        let short_key = b64([1u8; 31]);
        let refused = [
            text.replace('\n', "\r\n"),
            text.replace("from: ", "from:  "),
            text.replacen('\n', " \n", 1),
            format!("{text}extra: line\n"),
            text.trim_end().to_owned(),
            text.replace(&good.key, &short_key),
            text.replace(&good.key, &format!("{}=", good.key)),
            text.replace("1795000000", "01795000000"),
            text.replace("1795000000", "+1795000000"),
            text.replace("1795000000", "0"),
            text.replace("/1", "/2"),
            text.replace("service: https", "service: https x"),
        ];
        for text in refused {
            assert_eq!(KeyStatement::parse(&text), None, "{text:?}");
        }
    }

    /// Made exactly as the root ceremony makes one: `openssl genpkey
    /// -algorithm ed25519`, the text by `printf`, signed with `openssl
    /// pkeyutl -sign -rawin`, then `base64 | tr -d '\n'`.
    #[test]
    fn a_statement_signed_with_openssl_as_the_ceremony_does_holds() {
        let text = "claktory-account-service-key/1\nservice: https://account.claktory.com\nkey: AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8\nfrom: 1795300000\n";
        let signature = "claktory-account-root-1:iBD3uqw76yVG254wYT9BoX98gKaz9sB7AsyO/9gsWJ808qXho3RjFP6KMQMCVEUJBd/cXiCRiLv+50wOHHuBBQ==";
        let roots = [("claktory-account-root-1", "LYUS3gEWXD9LKqGeb7YJJ40Og7N87Qo/PQCxNSM00N8=")];
        let signed = SignedStatement { text: text.to_owned(), signature: signature.to_owned() };
        let statement = verify_statement(&signed, &roots).expect("it holds");
        assert_eq!((statement.from, statement.key.as_str()), (1_795_300_000, "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8"));
    }

    #[test]
    fn a_statement_counts_only_signed_by_a_root_over_its_exact_bytes() {
        let text = statement().text();
        let (signed, owned) = signed(&text);
        assert_eq!(verify_statement(&signed, &roots(&owned)), Ok(statement()));
        // Another root's key under the same id, the text changed, an unknown id.
        let other = vec![("test-root", "aGdAkbnB7pkB/dRRfPnkFKfKyWzPs+cF7FVHMlb7JpM=")];
        assert!(verify_statement(&signed, &other).is_err());
        let changed = SignedStatement { text: text.replace("1795000000", "1795000001"), ..signed.clone() };
        assert!(verify_statement(&changed, &roots(&owned)).is_err());
        let renamed = SignedStatement { signature: signed.signature.replacen("test-root", "other", 1), ..signed.clone() };
        assert!(verify_statement(&renamed, &roots(&owned)).is_err());
        // Signed, but not a statement: refused all the same.
        let (json, owned) = self::signed("{\"version\":\"v0.1.14\"}");
        assert!(verify_statement(&json, &roots(&owned)).is_err());
        // Every pinned root is a key.
        for (id, key) in ACCOUNT_ROOTS {
            let bytes = base64::engine::general_purpose::STANDARD.decode(key).expect(id);
            assert!(VerifyingKey::from_bytes(&bytes.try_into().expect(id)).is_ok(), "{id}");
        }
    }

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

    fn pipe_peer(service: &SigningKey) -> PipePeerStatement {
        PipePeerStatement {
            v: 1,
            label: PIPE_PEER_LABEL.into(),
            service: "https://account.claktory.com".into(),
            service_key: b64(service.verifying_key().to_bytes()),
            endpoint_id: "ab".repeat(32),
            account_id: "acct".into(),
            email: "ana@example.com".into(),
            iat: 1000,
            exp: 1000 + PIPE_PEER_LIFETIME,
        }
    }

    /// Fugo's hostile cases for verified pipes, the verifier's part: only
    /// the pinned service key's statement about the pipe's own peer key, in
    /// its lifetime, with a plain email, ever counts.
    #[test]
    fn a_pipe_peer_statement_counts_only_for_its_key_and_week() {
        let service = new_signing_key();
        let key = service.verifying_key();
        let said = pipe_peer(&service);
        let text = sign_pipe_peer(&service, &said);
        let endpoint = "ab".repeat(32);
        assert_eq!(verify_pipe_peer(&key, &text, &endpoint, 1500), Some(said.clone()));
        assert_eq!(verify_pipe_peer(&key, &text, &endpoint.to_uppercase(), 1500), Some(said.clone()), "hex in either case");
        let end = 1000 + PIPE_PEER_LIFETIME + ASSERTION_SKEW;
        assert!(verify_pipe_peer(&key, &text, &endpoint, end).is_some());
        assert_eq!(verify_pipe_peer(&key, &text, &endpoint, end + 1), None, "expired");
        assert_eq!(verify_pipe_peer(&key, &text, &endpoint, 1000 - ASSERTION_SKEW - 1), None, "not yet");
        // Relayed from another Claktory: about a key this pipe didn't pin.
        assert_eq!(verify_pipe_peer(&key, &text, &"cd".repeat(32), 1500), None, "another endpoint");
        assert_eq!(verify_pipe_peer(&new_signing_key().verifying_key(), &text, &endpoint, 1500), None, "another service key");
        // Signed by another key that the payload names: the pinned key decides.
        let other = new_signing_key();
        let mut theirs = said.clone();
        theirs.service_key = b64(other.verifying_key().to_bytes());
        assert_eq!(verify_pipe_peer(&key, &sign_pipe_peer(&other, &theirs), &endpoint, 1500), None, "signed by another key");
        let mut named_other = said.clone();
        named_other.service_key = b64(other.verifying_key().to_bytes());
        assert_eq!(verify_pipe_peer(&key, &sign_pipe_peer(&service, &named_other), &endpoint, 1500), None, "names another key");
        let (_, signature) = text.split_once('.').expect("two parts");
        let mut changed = said.clone();
        changed.account_id = "other".into();
        let forged = format!("{}.{signature}", b64(serde_json::to_vec(&changed).expect("json")));
        assert_eq!(verify_pipe_peer(&key, &forged, &endpoint, 1500), None, "changed after signing");
        // A device assertion's signature over the same bytes isn't one.
        let payload = serde_json::to_vec(&said).expect("json");
        let assertion_signed = format!("{}.{}", b64(&payload), b64(service.sign(&[ASSERTION_LABEL, &payload].concat()).to_bytes()));
        assert_eq!(verify_pipe_peer(&key, &assertion_signed, &endpoint, 1500), None, "another label");

        let refused = |change: &dyn Fn(&mut PipePeerStatement), why: &str| {
            let mut statement = said.clone();
            change(&mut statement);
            assert_eq!(verify_pipe_peer(&key, &sign_pipe_peer(&service, &statement), &endpoint, 1500), None, "{why}");
        };
        refused(&|s| s.exp = s.iat + PIPE_PEER_LIFETIME + 1, "longer than a week");
        refused(&|s| s.exp = s.iat - 1, "ends before it starts");
        refused(&|s| s.v = 2, "a version it doesn't know");
        refused(&|s| s.label = "claktory-pipe-peer/2".into(), "another label inside");
        refused(&|s| s.email = "ana@exаmple.com".into(), "a Cyrillic look-alike");
        refused(&|s| s.email = "moc.elpmaxe\u{202E}@ana".into(), "a direction override");
        refused(&|s| s.email = "ana@exa\u{200B}mple.com".into(), "zero width");
        refused(&|s| s.email = String::new(), "no email");
        refused(&|s| s.endpoint_id = "ab".repeat(31), "not an endpoint");
        let long = format!("{text}{}", " ".repeat(PIPE_PEER_MAX));
        assert_eq!(verify_pipe_peer(&key, &long, &endpoint, 1500), None, "too long to read");
        assert_eq!(verify_pipe_peer(&key, "", &endpoint, 1500), None);
        assert_eq!(verify_pipe_peer(&key, "x.y", &endpoint, 1500), None);
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
        // Nothing invisible that turns or hides what an address reads as.
        for bad in [
            "matt\u{202E}moc@example.com",
            "ma\u{200B}tt@example.com",
            "matt@exa\u{2066}mple.com",
            "\u{FEFF}m@example.com",
            "m\u{E0041}@a.com",
        ] {
            assert_eq!(normalize_email(bad), None, "{bad:?}");
        }
        assert_eq!(normalize_email("mätt@example.com"), Some("mätt@example.com".into()), "other letters still sign in");
        assert!(ascii_email("matt@example.com"));
        assert!(!ascii_email("mätt@example.com") && !ascii_email("") && !ascii_email("m att@example.com"));
        assert!(valid_claktory_id("abcdefgh_ijkl-mnop"));
        assert!(!valid_claktory_id("short") && !valid_claktory_id("has a space in it ok"));
    }
}
