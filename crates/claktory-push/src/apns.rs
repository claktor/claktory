//! Apple Push Notification service: token-based auth (an ES256 JWT from the
//! `.p8` key, refreshed every 50 minutes), HTTP/2.
//!
//! Every push is an alert with the relay's fixed text and `mutable-content`,
//! so the phone's Notification Service Extension runs, decrypts the
//! ciphertext (`c`) and shows what it says; whatever it can't decrypt stays
//! the fixed text.

use std::sync::Mutex;
use std::time::Duration;

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair};
use serde_json::{Value, json};

use crate::{ALERT_BODY, ALERT_TITLE, Delivery, Push, Urgency, b64url};

pub const PRODUCTION: &str = "https://api.push.apple.com";
pub const SANDBOX: &str = "https://api.sandbox.push.apple.com";
/// A token is used this long, inside Apple's 20 to 60 minutes.
const TOKEN_FOR: i64 = 50 * 60;
const TIMEOUT: Duration = Duration::from_secs(20);

pub struct Apns {
    key: EcdsaKeyPair,
    key_id: String,
    team_id: String,
    /// The app's bundle id: com.claktory.app.
    topic: String,
    production: String,
    sandbox: String,
    client: reqwest::Client,
    /// The token in use, and when it was made.
    token: Mutex<Option<(i64, String)>>,
}

impl Apns {
    /// From the `.p8` key's text (PKCS#8 PEM), its id and the team's.
    pub fn new(p8: &str, key_id: &str, team_id: &str, topic: &str) -> Result<Apns, String> {
        let der = crate::pem_der(p8).ok_or("the APNs key isn't a .p8 (PEM) file")?;
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &der).map_err(|_| "the APNs key isn't a P-256 key")?;
        if key_id.is_empty() || team_id.is_empty() || topic.is_empty() {
            return Err("APNs needs the key's id, the team id and the app's bundle id".to_owned());
        }
        let client = reqwest::Client::builder().timeout(TIMEOUT).build().map_err(|error| error.to_string())?;
        Ok(Apns {
            key,
            key_id: key_id.to_owned(),
            team_id: team_id.to_owned(),
            topic: topic.to_owned(),
            production: PRODUCTION.to_owned(),
            sandbox: SANDBOX.to_owned(),
            client,
            token: Mutex::new(None),
        })
    }

    /// Sends to other gateways than Apple's (tests).
    pub fn with_gateways(mut self, production: &str, sandbox: &str) -> Apns {
        self.production = production.trim_end_matches('/').to_owned();
        self.sandbox = sandbox.trim_end_matches('/').to_owned();
        self
    }

    /// The provider token: the one in use, or a new one once it's 50 minutes old.
    fn token(&self, now: i64) -> Result<String, String> {
        let mut token = self.token.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((made, token)) = token.as_ref()
            && now - made < TOKEN_FOR
        {
            return Ok(token.clone());
        }
        let header = b64url(json!({"alg": "ES256", "kid": self.key_id}).to_string());
        let claims = b64url(json!({"iss": self.team_id, "iat": now}).to_string());
        let signing = format!("{header}.{claims}");
        let signature = self.key.sign(&SystemRandom::new(), signing.as_bytes()).map_err(|_| "could not sign the APNs token")?;
        let made = format!("{signing}.{}", b64url(signature.as_ref()));
        *token = Some((now, made.clone()));
        Ok(made)
    }

    fn forget_token(&self) {
        *self.token.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    pub async fn send(&self, sandbox: bool, device: &str, push: &Push) -> Delivery {
        if device.is_empty() || !device.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Delivery::Gone;
        }
        let token = match self.token(crate::now()) {
            Ok(token) => token,
            Err(error) => return Delivery::Refused(error),
        };
        let gateway = if sandbox { &self.sandbox } else { &self.production };
        let mut request = self
            .client
            .post(format!("{gateway}/3/device/{device}"))
            .bearer_auth(token)
            .header("apns-push-type", "alert")
            .header("apns-topic", &self.topic)
            .header("apns-priority", if push.urgency == Urgency::Low { "5" } else { "10" })
            .header("apns-expiration", (crate::now() + i64::try_from(push.ttl).unwrap_or(0)).to_string())
            .json(&payload(push));
        if !push.collapse.is_empty() {
            request = request.header("apns-collapse-id", &push.collapse);
        }
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => return Delivery::Transient(format!("APNs: {error}")),
        };
        let status = response.status().as_u16();
        if status == 200 {
            return Delivery::Sent;
        }
        let reason = response.json::<Value>().await.ok().and_then(|body| body["reason"].as_str().map(str::to_owned)).unwrap_or_default();
        match (status, reason.as_str()) {
            (410, _) | (400, "BadDeviceToken" | "DeviceTokenNotForTopic") => Delivery::Gone,
            (403, "ExpiredProviderToken" | "InvalidProviderToken") => {
                self.forget_token();
                Delivery::Transient(format!("APNs: {reason}"))
            }
            (429 | 500 | 503, _) => Delivery::Transient(format!("APNs {status}: {reason}")),
            _ => Delivery::Refused(format!("APNs {status}: {reason}")),
        }
    }
}

/// The push as Apple gets it: the fixed alert, and the ciphertext.
pub fn payload(push: &Push) -> Value {
    let mut aps = json!({
        "alert": {"title": ALERT_TITLE, "body": ALERT_BODY},
        "mutable-content": 1,
        "sound": "default",
        "interruption-level": if push.urgency == Urgency::Urgent { "time-sensitive" } else { "active" },
    });
    if !push.thread.is_empty() {
        aps["thread-id"] = push.thread.clone().into();
    }
    json!({"aps": aps, "c": push.ciphertext})
}
