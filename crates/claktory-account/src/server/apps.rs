//! User-level apps (`/v1/apps/…`): the service authenticates each call and
//! forwards it to the user-apps worker (crates/claktory-user-apps), a
//! process of its own with its own database and none of the service's
//! keys, with who it's from. It parses nothing of the app's, passes back
//! only JSON answers with an app's statuses, and signs them as it signs
//! every answer. Without a worker (`--user-apps`), or while it's down, every
//! app route answers 503: there's no fallback here.

use claktory_user_apps::ipc::{self, Actor, Caller, Registration, Request};
use hyper::StatusCode;
use serde::Deserialize;

use super::{ApiError, Call, Reply, Service, api, db, devices};
use crate::protocol::DEVICE_AUTH;

fn unavailable() -> ApiError {
    ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "app_unavailable", "This part of Claktory isn't available right now. Try again later.")
}

/// Who a call is from: a Claktory's backend (`Bearer` token, signed), for
/// itself or one of its agents, or a person's signed-in device.
fn caller(service: &Service, call: &Call, agent: Option<Actor>) -> Result<Caller, ApiError> {
    let device = call.header("authorization").is_some_and(|value| value.starts_with(DEVICE_AUTH));
    if device {
        if agent.is_some() {
            return Err(ApiError::invalid("A device acts for no agent."));
        }
        let device = devices::authenticate(service, call)?;
        let registrations = db::claktories(&service.db(), &device.account_id)?
            .into_iter()
            .filter(|claktory| claktory.revoked_at.is_none())
            .map(|claktory| Registration { claktory_id: claktory.id, epoch: claktory.epoch })
            .collect();
        return Ok(Caller::Device { account_id: device.account_id, device_key: device.account_key, registrations });
    }
    let claktory = api::authenticate(service, call)?;
    Ok(Caller::Backend { account_id: claktory.account_id, claktory_id: claktory.id, epoch: claktory.epoch, agent })
}

/// The query without `agent` and `incarnation`, and the agent they name:
/// both or neither, well formed.
fn split_query(query: &str) -> Result<(String, Option<Actor>), ApiError> {
    let (mut id, mut incarnation) = (None, None);
    let mut rest = form_urlencoded::Serializer::new(String::new());
    for (name, value) in form_urlencoded::parse(query.as_bytes()) {
        match name.as_ref() {
            "agent" if id.is_none() => id = Some(value.into_owned()),
            "incarnation" if incarnation.is_none() => incarnation = Some(value.into_owned()),
            "agent" | "incarnation" => return Err(ApiError::invalid("An agent is named once.")),
            _ => {
                rest.append_pair(&name, &value);
            }
        }
    }
    let agent = match (id, incarnation) {
        (None, None) => None,
        (Some(id), Some(incarnation)) => {
            let actor = Actor { id, incarnation };
            let valid = |text: &str| {
                !text.is_empty()
                    && text.len() <= 64
                    && text.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
            };
            if !valid(&actor.id) || !valid(&actor.incarnation) {
                return Err(ApiError::invalid("The agent's id or incarnation isn't valid."));
            }
            Some(actor)
        }
        _ => return Err(ApiError::invalid("An agent is named with its incarnation.")),
    };
    Ok((rest.finish(), agent))
}

/// Forwards an app call to the worker, authenticated.
pub(super) async fn call(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let Some(apps) = service.config.user_apps.clone() else { return Err(unavailable()) };
    let (query, agent) = split_query(&call.query)?;
    let caller = caller(service, call, agent)?;
    let request = Request::Call { method: call.method.as_str().to_owned(), path: call.path.clone(), query, time: service.now(), caller };
    let (status, body) = match ipc::ask(&apps.socket, apps.uid, &request, &call.body).await {
        Ok(answer) => answer,
        Err(error) => {
            tracing::warn!("{error}");
            return Err(unavailable());
        }
    };
    if !ipc::acceptable(status, &body) {
        tracing::error!(status, "the user-apps worker answered something it may not; refused");
        return Err(unavailable());
    }
    let status = StatusCode::from_u16(status).map_err(|_| unavailable())?;
    Ok(Reply { status, content_type: "application/json", headers: Vec::new(), body })
}

/// Tells the worker an account was deleted, so its apps forget it. Its
/// calls no longer authenticate, so what's left is unreachable meanwhile.
pub(super) async fn purge_account(service: &Service, account_id: &str) {
    let Some(apps) = service.config.user_apps.clone() else { return };
    let request = Request::PurgeAccount { account_id: account_id.to_owned(), time: service.now() };
    match ipc::ask(&apps.socket, apps.uid, &request, &[]).await {
        Ok((204, _)) => {}
        Ok((status, _)) => tracing::error!(status, "the user-apps worker didn't purge a deleted account"),
        Err(error) => tracing::error!("a deleted account's apps weren't purged: {error}"),
    }
}

/// The account page's view of the account's grants.
pub(super) enum Shown {
    /// The service runs no user-level apps.
    NoApps,
    /// It does, but the worker didn't answer: say so rather than "none".
    Unavailable,
    Listed(Vec<ShownGrant>),
}

/// A grant as the account page shows it (the worker's `Grant`).
#[derive(Deserialize, Debug)]
pub(super) struct ShownGrant {
    pub id: String,
    pub app: String,
    pub claktory_id: String,
    pub agent: Actor,
    pub scopes: Vec<String>,
    pub resources: Vec<serde_json::Value>,
    pub granted_ms: i64,
    pub expires_ms: i64,
}

#[derive(Deserialize)]
struct Listed {
    grants: Vec<ShownGrant>,
}

/// The person at the account page, with the account's live registrations.
fn account_caller(service: &Service, account_id: &str) -> Result<Caller, ApiError> {
    let registrations = db::claktories(&service.db(), account_id)?
        .into_iter()
        .filter(|claktory| claktory.revoked_at.is_none())
        .map(|claktory| Registration { claktory_id: claktory.id, epoch: claktory.epoch })
        .collect();
    Ok(Caller::Account { account_id: account_id.to_owned(), registrations })
}

/// Asks the worker on the account page's behalf: its answer, or None when
/// it's not there or answered wrongly.
async fn ask_for_account(service: &Service, account_id: &str, method: &str, path: &str) -> Option<(u16, Vec<u8>)> {
    let apps = service.config.user_apps.clone()?;
    let caller = account_caller(service, account_id).ok()?;
    let request = Request::Call { method: method.to_owned(), path: path.to_owned(), query: String::new(), time: service.now(), caller };
    match ipc::ask(&apps.socket, apps.uid, &request, &[]).await {
        Ok((status, body)) if ipc::acceptable(status, &body) => Some((status, body)),
        Ok((status, _)) => {
            tracing::error!(status, "the user-apps worker answered something it may not; refused");
            None
        }
        Err(error) => {
            tracing::warn!("{error}");
            None
        }
    }
}

/// The account's live grants, for its page.
pub(super) async fn account_grants(service: &Service, account_id: &str) -> Shown {
    if service.config.user_apps.is_none() {
        return Shown::NoApps;
    }
    match ask_for_account(service, account_id, "GET", "/v1/apps/grants").await {
        Some((200, body)) => match serde_json::from_slice::<Listed>(&body) {
            Ok(listed) => Shown::Listed(listed.grants),
            Err(error) => {
                tracing::error!("the user-apps worker's grants don't read: {error}");
                Shown::Unavailable
            }
        },
        _ => Shown::Unavailable,
    }
}

/// Revokes one of the account's grants from its page: Some(true) when it
/// went, Some(false) when there's no such grant of the account's, None when
/// the worker didn't answer.
pub(super) async fn revoke_for_account(service: &Service, account_id: &str, id: &str) -> Option<bool> {
    let valid = !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_hexdigit());
    if !valid {
        return Some(false);
    }
    match ask_for_account(service, account_id, "DELETE", &format!("/v1/apps/grants/{id}")).await? {
        (204, _) => Some(true),
        (404, _) => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_is_named_by_both_its_id_and_incarnation_once() {
        let (rest, agent) = split_query("since=5&agent=june&incarnation=inc-1&x=a%20b").expect("split");
        assert_eq!(rest, "since=5&x=a+b");
        assert_eq!(agent, Some(Actor { id: "june".into(), incarnation: "inc-1".into() }));
        assert_eq!(split_query("").expect("split"), (String::new(), None));
        for bad in
            ["agent=june", "incarnation=x", "agent=june&agent=kai&incarnation=x", "agent=ju%20ne&incarnation=x", "agent=&incarnation=x"]
        {
            assert!(split_query(bad).is_err(), "{bad}");
        }
    }
}
