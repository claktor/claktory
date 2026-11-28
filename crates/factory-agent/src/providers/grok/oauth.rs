//! Signing in to Grok with a Grok subscription: xAI's
//! device sign-in (RFC 8628) at auth.x.ai, as its own command-line tool
//! signs in, with the narrowest scopes that chat. The person opens xAI's
//! page on any device and confirms the code there; Claktory never sees a
//! password. Tokens last 6 hours and refresh with a refresh token that
//! changes each time. A refusal from xAI ends the sign-in with what xAI
//! said: never asked again another way.

use std::time::Duration;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::auth::{AuthEvent, AuthFuture, AuthInteraction, OAuthFlow, RequestAuth};
use crate::clock::now_ms;
use crate::credentials::Credential;
use crate::errors::Error;
use crate::oauth::{CANCEL_MESSAGE, check_cancel, send_cancellable};
use crate::providers::{client, response_text};

/// xAI's sign-in.
pub const ISSUER: &str = "https://auth.x.ai";
/// The Grok command-line tool's client: xAI registers no other for
/// subscription sign-ins.
pub const CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
/// The least that chats: no access to Grok's conversations or workspaces,
/// nothing on X. xAI granted exactly what's asked in the probe (25 Nov 2026).
/// `api:access`: the proxy refuses a model request without it (403, "OAuth2
/// token missing required scope: api:access", 2026-11-25); the tool itself
/// is granted it. Its token goes only to the proxy
/// (`super::allowed`), never to xAI's API.
pub const SCOPE: &str = "openid offline_access grok-cli:access api:access";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// How long a code lasts when xAI doesn't say (it said 30 minutes).
const DEFAULT_CODE_SECONDS: u64 = 30 * 60;
const DEFAULT_POLL_SECONDS: u64 = 5;
const SLOW_DOWN_SECONDS: u64 = 5;
/// How long an access token lasts when xAI doesn't say (it said 6 hours).
const DEFAULT_TOKEN_SECONDS: u64 = 6 * 3600;
/// The most of xAI's own words shown in a refusal.
const SAID_LIMIT: usize = 300;

/// How xAI's answers begin a refusal, everywhere the person sees one.
pub const REFUSED: &str = "xAI refused this sign-in";

/// xAI's device sign-in. `issuer` is xAI's unless a test points it at its
/// own server, whose pages are then on 127.0.0.1 too.
pub struct GrokOAuth {
    pub issuer: String,
}

impl Default for GrokOAuth {
    fn default() -> GrokOAuth {
        GrokOAuth { issuer: ISSUER.to_owned() }
    }
}

/// What xAI said in a refusal, in a sentence: its description, else its
/// error code, else the body, never more than `SAID_LIMIT` characters.
pub fn said(body: &str) -> String {
    let json = serde_json::from_str::<Value>(body).ok();
    let text = json
        .as_ref()
        .and_then(|json| {
            let error = &json["error"];
            json["error_description"]
                .as_str()
                .or_else(|| error["message"].as_str())
                .or_else(|| error.as_str())
                .or_else(|| json["message"].as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| body.trim().to_owned());
    let text: String = text.chars().filter(|c| !c.is_control()).take(SAID_LIMIT).collect();
    if text.is_empty() { "no reason given".to_owned() } else { text }
}

fn refused(status: u16, body: &str) -> Error {
    Error::Other(format!("{REFUSED}: {} ({status})", said(body)))
}

impl GrokOAuth {
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.issuer.trim_end_matches('/'))
    }

    /// Whether `uri` is a page of xAI's to show the person: https on x.ai
    /// or one of its subdomains (a test's issuer: its own 127.0.0.1).
    fn trusted(&self, uri: &str) -> bool {
        let Ok(url) = reqwest::Url::parse(uri) else { return false };
        let host = url.host_str().unwrap_or_default();
        if self.issuer == ISSUER {
            return url.scheme() == "https" && (host == "x.ai" || host.ends_with(".x.ai"));
        }
        let issuer = reqwest::Url::parse(&self.issuer).ok();
        issuer.is_some_and(|issuer| issuer.host_str() == Some(host) && host == "127.0.0.1")
    }

    async fn post(&self, path: &str, form: &[(&str, &str)], cancel: &CancellationToken) -> Result<(u16, String), Error> {
        let response = send_cancellable(client().post(self.url(path)).form(form), cancel).await?;
        let status = response.status().as_u16();
        Ok((status, response_text(response).await))
    }

    async fn login_device(&self, interaction: &dyn AuthInteraction) -> Result<Credential, Error> {
        let cancel = interaction.cancel();
        let (status, body) = self.post("/oauth2/device/code", &[("client_id", CLIENT_ID), ("scope", SCOPE)], &cancel).await?;
        if !(200..300).contains(&status) {
            return Err(refused(status, &body));
        }
        let device: Value = serde_json::from_str(&body).map_err(|_| Error::Other(format!("{REFUSED}: its answer couldn't be read.")))?;
        let (Some(device_code), Some(user_code), Some(uri)) =
            (device["device_code"].as_str(), device["user_code"].as_str(), device["verification_uri"].as_str())
        else {
            return Err(Error::Other(format!("{REFUSED}: its answer had no code.")));
        };
        if user_code.is_empty() || !user_code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(Error::Other(format!("{REFUSED}: its code isn't one to show.")));
        }
        // Only ever xAI's own page, never one an answer names elsewhere.
        let complete = device["verification_uri_complete"].as_str();
        if !self.trusted(uri) || complete.is_some_and(|complete| !self.trusted(complete)) {
            return Err(Error::Other(format!("{REFUSED}: the page it named isn't xAI's.")));
        }
        let expires = device["expires_in"].as_u64().filter(|seconds| *seconds > 0).unwrap_or(DEFAULT_CODE_SECONDS);
        let mut interval = device["interval"].as_u64().filter(|seconds| *seconds > 0).unwrap_or(DEFAULT_POLL_SECONDS);
        interaction.notify(AuthEvent::DeviceCode {
            user_code: user_code.to_owned(),
            verification_uri: complete.unwrap_or(uri).to_owned(),
            interval_seconds: Some(interval),
            expires_in_seconds: Some(expires),
        });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(expires);
        loop {
            let wait = Duration::from_secs(interval).min(deadline.saturating_duration_since(tokio::time::Instant::now()));
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = cancel.cancelled() => return Err(Error::Other(CANCEL_MESSAGE.into())),
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Other("The code expired before it was confirmed. Sign in again.".into()));
            }
            check_cancel(&cancel)?;
            let form = [("grant_type", DEVICE_GRANT), ("device_code", device_code), ("client_id", CLIENT_ID)];
            let (status, body) = self.post("/oauth2/token", &form, &cancel).await?;
            if (200..300).contains(&status) {
                return credential(&body, None);
            }
            let error = serde_json::from_str::<Value>(&body).ok().and_then(|json| json["error"].as_str().map(str::to_owned));
            match error.as_deref() {
                Some("authorization_pending") => {}
                Some("slow_down") => interval += SLOW_DOWN_SECONDS,
                Some("access_denied") => return Err(Error::Other("The sign-in was declined at xAI.".into())),
                Some("expired_token") => return Err(Error::Other("The code expired before it was confirmed. Sign in again.".into())),
                _ => return Err(refused(status, &body)),
            }
        }
    }

    async fn refresh_token(&self, refresh: &str, cancel: &CancellationToken) -> Result<Credential, Error> {
        let form = [("grant_type", "refresh_token"), ("refresh_token", refresh), ("client_id", CLIENT_ID)];
        let (status, body) = self.post("/oauth2/token", &form, cancel).await?;
        if !(200..300).contains(&status) {
            let message = format!("Grok token refresh failed ({status}): {}", said(&body));
            return Err(crate::auth::refusal(crate::auth::refresh_refused(status, &body), message));
        }
        credential(&body, Some(refresh))
    }

    async fn revoke_token(&self, refresh: &str) -> Result<(), Error> {
        let form = [("token", refresh), ("token_type_hint", "refresh_token"), ("client_id", CLIENT_ID)];
        let (status, body) = self.post("/oauth2/revoke", &form, &CancellationToken::new()).await?;
        if (200..300).contains(&status) {
            Ok(())
        } else {
            Err(Error::Other(format!("xAI didn't revoke the sign-in ({status}): {}", said(&body))))
        }
    }
}

/// The sign-in a token answer gives. A refresh that gives no new refresh
/// token keeps `kept`; the scope xAI granted is kept with it, to show what
/// the sign-in may do. Never anything of the answer in an error: it holds
/// tokens.
fn credential(body: &str, kept: Option<&str>) -> Result<Credential, Error> {
    let unreadable = || Error::Other(format!("{REFUSED}: its answer had no token."));
    let json: Value = serde_json::from_str(body).map_err(|_| unreadable())?;
    let access = json["access_token"].as_str().filter(|token| !token.is_empty()).ok_or_else(unreadable)?;
    let refresh = json["refresh_token"].as_str().filter(|token| !token.is_empty()).or(kept).ok_or_else(unreadable)?;
    let seconds = json["expires_in"].as_u64().filter(|seconds| *seconds > 0).unwrap_or(DEFAULT_TOKEN_SECONDS);
    let mut extra = serde_json::Map::new();
    if let Some(scope) = json["scope"].as_str() {
        extra.insert("scope".into(), Value::from(scope));
    }
    Ok(Credential::Oauth {
        access: access.to_owned(),
        refresh: refresh.to_owned(),
        expires: now_ms() + seconds * 1000,
        account_id: None,
        extra,
    })
}

impl OAuthFlow for GrokOAuth {
    fn name(&self) -> &str {
        "Grok (your Grok subscription)"
    }

    fn is_subscription(&self) -> bool {
        true
    }

    fn login<'a>(&'a self, interaction: &'a dyn AuthInteraction) -> AuthFuture<'a, Result<Credential, Error>> {
        Box::pin(self.login_device(interaction))
    }

    fn refresh<'a>(&'a self, credential: Credential, cancel: CancellationToken) -> AuthFuture<'a, Result<Credential, Error>> {
        Box::pin(async move {
            let Credential::Oauth { refresh, .. } = credential else {
                return Err(Error::Other("A Grok sign-in has no key to refresh.".into()));
            };
            self.refresh_token(&refresh, &cancel).await
        })
    }

    fn to_auth(&self, credential: &Credential) -> Result<RequestAuth, Error> {
        match credential {
            Credential::Oauth { access, .. } => {
                Ok(RequestAuth { api_key: Some(access.clone()), headers: super::proxy_headers(), base_url: None })
            }
            Credential::ApiKey { .. } => Err(Error::Other("A Grok sign-in is a subscription, not a key.".into())),
        }
    }

    fn revoke<'a>(&'a self, credential: &'a Credential) -> AuthFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            match credential {
                Credential::Oauth { refresh, .. } if !refresh.is_empty() => self.revoke_token(refresh).await,
                _ => Ok(()),
            }
        })
    }
}
