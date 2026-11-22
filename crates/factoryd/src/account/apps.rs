//! Grants at the account's user-level apps (`/v1/apps/grants`, the
//! contract in crates/claktory-user-apps/README.md): the signed calls
//! `crate::user_apps` makes as this Claktory, after the person approved
//! Rivet's proposal, at check-in for its mirror, and when an agent is
//! deleted.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{Failure, State};

const GRANTS: &str = "/v1/apps/grants";
const DMS: &str = "/v1/apps/dms";

/// An agent of this Claktory, as the service names it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Actor {
    pub id: String,
    pub incarnation: String,
}

/// Something a grant names.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Resource {
    pub kind: String,
    pub id: String,
}

/// `PUT /v1/apps/grants`.
#[derive(Debug, Serialize)]
pub(crate) struct Put {
    pub app: String,
    pub agent: Actor,
    pub scopes: Vec<String>,
    pub resources: Vec<Resource>,
    pub expires_ms: i64,
}

/// A grant as the service shows it.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Grant {
    pub id: String,
    pub app: String,
    pub agent: Actor,
    pub scopes: Vec<String>,
    #[serde(default)]
    pub resources: Vec<Resource>,
    pub granted_ms: i64,
    pub expires_ms: i64,
}

#[derive(Deserialize)]
struct Grants {
    grants: Vec<Grant>,
}

/// Why a call didn't go through.
#[derive(Debug)]
pub(crate) enum Problem {
    /// This Claktory isn't signed in to an account (or was removed there).
    SignedOut,
    /// The service couldn't be reached, or its answer isn't its own: asking
    /// again later may work.
    ForNow(String),
    /// It answered, but asks to wait (429) or its apps aren't running
    /// (5xx): asking again in a minute may work.
    Busy(String),
    /// It refused: its words.
    Refused(u16, String),
}

impl From<Failure> for Problem {
    fn from(failure: Failure) -> Problem {
        match failure {
            Failure::Refused(status, body) if status == 429 || status >= 500 => Problem::Busy(body.message),
            Failure::Refused(401, body) if body.error == "revoked" => Problem::SignedOut,
            Failure::Refused(status, body) => Problem::Refused(status, body.message),
            Failure::Unreachable(why) => Problem::ForNow(why),
            Failure::OtherKey => Problem::ForNow("an answer signed by another key than the service's".to_owned()),
        }
    }
}

impl State {
    /// What the person calls the account's service: "claktory.com", or the
    /// host of one of their own.
    pub(crate) fn service_name(&self) -> String {
        super::service_name(&self.lock().saved.service)
    }

    /// Checks in now, which reconciles the app grants (an agent holding
    /// some was deleted).
    pub(crate) fn check_in_soon(&self) {
        self.lock().next_check_in = std::time::Instant::now();
        self.wake.notify_one();
    }

    /// A call to the account's apps as the signed-in Claktory: the answer's
    /// bytes (empty for 204).
    async fn apps_call(&self, method: &str, path: &str, body: Vec<u8>) -> Result<Vec<u8>, Problem> {
        self.apps_call_waiting(method, path, body, None).await
    }

    async fn apps_call_waiting(&self, method: &str, path: &str, body: Vec<u8>, wait: Option<Duration>) -> Result<Vec<u8>, Problem> {
        let (service, service_key, token) = {
            let inner = self.lock();
            let saved = &inner.saved;
            if !saved.signed_in() || saved.revoked {
                return Err(Problem::SignedOut);
            }
            (saved.service.clone(), saved.service_key.clone(), saved.machine_token.clone())
        };
        let key = claktory_account::protocol::verifying_key(&service_key).ok_or(Problem::SignedOut)?;
        let (_, backend_key) = self.identity().map_err(|error| Problem::ForNow(error.to_string()))?;
        Ok(self.send_waiting(&service, Some(&key), method, path, body, Some((&token, &backend_key)), wait).await?)
    }

    /// A call to the account's direct messages, `/v1/apps/dms/<route>`, as
    /// `agent` (its id and incarnation go in the signed query) or as this
    /// Claktory. Each call carries a fresh `request_nonce` (in the query, or
    /// in a JSON body), so two alike in one second still differ. `wait`: a
    /// long poll's time instead of the usual timeout.
    pub(crate) async fn dms(
        &self,
        method: &str,
        route: &str,
        query: &[(&str, String)],
        agent: Option<&Actor>,
        body: Option<serde_json::Value>,
        wait: Option<Duration>,
    ) -> Result<Vec<u8>, Problem> {
        let nonce = crate::encoding::hex(&crate::vault::crypto::random::<16>());
        let mut pairs: Vec<(&str, &str)> = query.iter().map(|(name, value)| (*name, value.as_str())).collect();
        if let Some(agent) = agent {
            pairs.extend([("agent", agent.id.as_str()), ("incarnation", agent.incarnation.as_str())]);
        }
        let body = match body {
            Some(mut body) => {
                if let Some(fields) = body.as_object_mut() {
                    fields.insert("request_nonce".to_owned(), serde_json::Value::String(nonce.clone()));
                }
                serde_json::to_vec(&body).map_err(|error| Problem::ForNow(error.to_string()))?
            }
            None => {
                pairs.push(("request_nonce", nonce.as_str()));
                Vec::new()
            }
        };
        let query = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish();
        let path = if query.is_empty() { format!("{DMS}/{route}") } else { format!("{DMS}/{route}?{query}") };
        self.apps_call_waiting(method, &path, body, wait).await
    }

    /// Makes the grant `put` asks for, replacing the agent's earlier one in
    /// that app.
    pub(crate) async fn put_grant(&self, put: &Put) -> Result<Grant, Problem> {
        let body = serde_json::to_vec(put).map_err(|error| Problem::ForNow(error.to_string()))?;
        let answer = self.apps_call("PUT", GRANTS, body).await?;
        serde_json::from_slice(&answer).map_err(|error| Problem::ForNow(format!("an unreadable answer: {error}")))
    }

    /// This Claktory's live grants.
    pub(crate) async fn grants(&self) -> Result<Vec<Grant>, Problem> {
        let answer = self.apps_call("GET", GRANTS, Vec::new()).await?;
        let grants: Grants = serde_json::from_slice(&answer).map_err(|error| Problem::ForNow(format!("an unreadable answer: {error}")))?;
        Ok(grants.grants)
    }

    /// Revokes one of this Claktory's grants; one already gone is fine.
    pub(crate) async fn revoke_grant(&self, id: &str) -> Result<(), Problem> {
        let valid = !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric());
        if !valid {
            return Ok(());
        }
        match self.apps_call("DELETE", &format!("{GRANTS}/{id}"), Vec::new()).await {
            Ok(_) | Err(Problem::Refused(404, _)) => Ok(()),
            Err(problem) => Err(problem),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(status: u16, error: &str) -> Problem {
        let body = serde_json::from_value(serde_json::json!({ "error": error, "message": "Too many tries. Try again later." }))
            .expect("an error body");
        Problem::from(Failure::Refused(status, body))
    }

    /// A service that answered and asked to wait is busy, not out of reach.
    #[test]
    fn a_service_that_asks_to_wait_is_busy_not_unreachable() {
        assert!(matches!(said(429, "rate_limited"), Problem::Busy(_)));
        assert!(matches!(said(503, "apps_unavailable"), Problem::Busy(_)));
        assert!(matches!(Problem::from(Failure::Unreachable("connection refused".into())), Problem::ForNow(_)));
        assert!(matches!(said(401, "revoked"), Problem::SignedOut));
        assert!(matches!(said(400, "bad_request"), Problem::Refused(400, _)));
    }
}
