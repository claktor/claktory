//! Firebase Cloud Messaging (HTTP v1), for Android: a high-priority data
//! message carrying only the ciphertext, which the app decrypts and shows
//! itself. Authorized with an OAuth token from the project's service
//! account (an RS256 JWT exchanged at Google's token endpoint).

use std::sync::Mutex;
use std::time::Duration;

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Delivery, Push, Urgency, b64url};

const SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
const SEND: &str = "https://fcm.googleapis.com";
const TIMEOUT: Duration = Duration::from_secs(20);

/// The service account's file, as Google gives it (what's used of it).
#[derive(Deserialize)]
struct ServiceAccount {
    project_id: String,
    client_email: String,
    private_key: String,
    token_uri: String,
}

pub struct Fcm {
    project: String,
    email: String,
    key: RsaKeyPair,
    token_uri: String,
    send: String,
    client: reqwest::Client,
    /// The access token in use, and when it stops working.
    token: tokio::sync::Mutex<Option<(i64, String)>>,
    /// Pushes since the token was last refreshed failed for it.
    stale: Mutex<bool>,
}

impl Fcm {
    /// From the service account's JSON file's text.
    pub fn new(service_account: &str) -> Result<Fcm, String> {
        let account: ServiceAccount =
            serde_json::from_str(service_account).map_err(|error| format!("the service account file isn't one: {error}"))?;
        let der = crate::pem_der(&account.private_key).ok_or("the service account's private key isn't PEM")?;
        let key = RsaKeyPair::from_pkcs8(&der).map_err(|_| "the service account's private key isn't an RSA key")?;
        let client = reqwest::Client::builder().timeout(TIMEOUT).build().map_err(|error| error.to_string())?;
        Ok(Fcm {
            project: account.project_id,
            email: account.client_email,
            key,
            token_uri: account.token_uri,
            send: SEND.to_owned(),
            client,
            token: tokio::sync::Mutex::new(None),
            stale: Mutex::new(false),
        })
    }

    /// Sends to another endpoint than Google's (tests).
    pub fn with_endpoints(mut self, send: &str, token_uri: &str) -> Fcm {
        self.send = send.trim_end_matches('/').to_owned();
        self.token_uri = token_uri.to_owned();
        self
    }

    /// The signed assertion the token endpoint takes.
    fn assertion(&self, now: i64) -> Result<String, String> {
        let header = b64url(json!({"alg": "RS256", "typ": "JWT"}).to_string());
        let claims = b64url(json!({"iss": self.email, "scope": SCOPE, "aud": self.token_uri, "iat": now, "exp": now + 3600}).to_string());
        let signing = format!("{header}.{claims}");
        let mut signature = vec![0u8; self.key.public_modulus_len()];
        self.key
            .sign(&RSA_PKCS1_SHA256, &SystemRandom::new(), signing.as_bytes(), &mut signature)
            .map_err(|_| "could not sign the service account's assertion")?;
        Ok(format!("{signing}.{}", b64url(signature)))
    }

    async fn access_token(&self) -> Result<String, String> {
        let mut token = self.token.lock().await;
        let now = crate::now();
        let stale = std::mem::take(&mut *self.stale.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        if let Some((until, token)) = token.as_ref()
            && !stale
            && now < until - 60
        {
            return Ok(token.clone());
        }
        let body = format!("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion={}", self.assertion(now)?);
        let response = self
            .client
            .post(&self.token_uri)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|error| format!("Google's token endpoint: {error}"))?;
        if !response.status().is_success() {
            return Err(format!("Google's token endpoint answered {}", response.status()));
        }
        let answer: Value = response.json().await.map_err(|error| format!("Google's token endpoint: {error}"))?;
        let access = answer["access_token"].as_str().ok_or("Google's token endpoint gave no token")?.to_owned();
        let expires = answer["expires_in"].as_i64().unwrap_or(3600);
        *token = Some((now + expires, access.clone()));
        Ok(access)
    }

    pub async fn send(&self, device: &str, push: &Push) -> Delivery {
        let token = match self.access_token().await {
            Ok(token) => token,
            Err(error) => return Delivery::Transient(error),
        };
        let url = format!("{}/v1/projects/{}/messages:send", self.send, self.project);
        let response = match self.client.post(url).bearer_auth(token).json(&message(device, push)).send().await {
            Ok(response) => response,
            Err(error) => return Delivery::Transient(format!("FCM: {error}")),
        };
        let status = response.status().as_u16();
        if status == 200 {
            return Delivery::Sent;
        }
        let body: Value = response.json().await.unwrap_or_default();
        let code = body["error"]["status"].as_str().unwrap_or_default().to_owned();
        let details = body["error"]["details"].as_array().cloned().unwrap_or_default();
        // The token will never take a push: retired (UNREGISTERED), or not
        // a token at all (a BadRequest on message.token). Not
        // SENDER_ID_MISMATCH: with both apps in the relay's one project,
        // that's the relay's service account being another project's, and
        // forgetting every phone for it would outlast the fix.
        let unusable = details.iter().any(|detail| {
            detail["errorCode"] == "UNREGISTERED"
                || detail["fieldViolations"]
                    .as_array()
                    .is_some_and(|violations| violations.iter().any(|violation| violation["field"] == "message.token"))
        });
        match (status, code.as_str()) {
            (404, _) | (_, "NOT_FOUND") => Delivery::Gone,
            _ if unusable => Delivery::Gone,
            (401, _) => {
                *self.stale.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = true;
                Delivery::Transient("FCM: the access token was refused".to_owned())
            }
            (429 | 500 | 503, _) => Delivery::Transient(format!("FCM {status}: {code}")),
            _ => Delivery::Refused(format!("FCM {status}: {code}")),
        }
    }
}

/// The message as Google gets it: data only, which the app shows itself.
pub fn message(device: &str, push: &Push) -> Value {
    let mut android = json!({
        "priority": if push.urgency == Urgency::Low { "NORMAL" } else { "HIGH" },
        "ttl": format!("{}s", push.ttl),
    });
    if !push.collapse.is_empty() {
        android["collapse_key"] = push.collapse.clone().into();
    }
    let mut data = json!({"c": push.ciphertext});
    if !push.thread.is_empty() {
        data["t"] = push.thread.clone().into();
    }
    if push.urgency == Urgency::Urgent {
        data["u"] = "1".into();
    }
    json!({"message": {"token": device, "android": android, "data": data}})
}
