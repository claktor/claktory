//! This Claktory's token for Claktory's relays (#450; Fugo's
//! coordination/security/relay-access.md).
//!
//! Claktory's relay forwards only to or from a Claktory signed in to a
//! claktory.com account, which shows it a token from the account service
//! (`POST /v1/relay/token`), bound to its tunnel key and good for 7 days.
//! A signed-in Claktory gets one at its first check-in, and another at the
//! check-in when it has less than 3 days left. It's kept in factory.db, so a
//! restart doesn't wait for the service, and only there: never in logs,
//! config.json or the Workspace. The tunnel gives it to Claktory's relay
//! alone (`remote`); signing out forgets it.

use std::sync::Arc;

use claktory_account::protocol::{RelayToken, RelayTokenRequest};
use rusqlite::OptionalExtension;

use super::{Failure, now_s};
use crate::factory::Factory;

pub(crate) const SCHEMA: &str = "
-- This Claktory's token for Claktory's relays (#450), for the tunnel key
-- `endpoint`, until `expires_at` (unix seconds); none while signed out.
CREATE TABLE relay_token (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    endpoint TEXT NOT NULL,
    token TEXT NOT NULL,
    expires_at INTEGER NOT NULL
);
";

/// A token with less than this left is replaced at the next check-in.
const RENEW_WITHIN: i64 = 3 * 86_400;

/// The token kept, for which tunnel key and until when.
struct Kept {
    endpoint: String,
    token: String,
    expires_at: i64,
}

fn kept(factory: &Factory) -> Option<Kept> {
    let db = factory.store.db();
    let read = db
        .query_row("SELECT endpoint, token, expires_at FROM relay_token WHERE id = 1", [], |row| {
            Ok(Kept { endpoint: row.get(0)?, token: row.get(1)?, expires_at: row.get(2)? })
        })
        .optional();
    read.unwrap_or_else(|error| {
        tracing::warn!(%error, "could not read the relay token");
        None
    })
}

/// The token for the tunnel key `endpoint` while it works, if there is one.
pub(crate) fn current(factory: &Factory, endpoint: &str) -> Option<String> {
    kept(factory).filter(|kept| kept.endpoint == endpoint && kept.expires_at > now_s()).map(|kept| kept.token)
}

fn forget(factory: &Factory) {
    if let Err(error) = factory.store.db().execute("DELETE FROM relay_token", []) {
        tracing::warn!(%error, "could not forget the relay token");
    }
}

/// This Claktory's tunnel key, as the service and the relay name it.
fn endpoint(factory: &Factory) -> Option<String> {
    crate::remote::secret_key(&factory.home).ok().map(|key| key.public().to_string())
}

/// Signed in (AccountVerify): the token comes with the check-in the
/// sign-in starts, and the tunnel moves to Claktory's relays with it.
pub(crate) fn signed_in(factory: &Arc<Factory>) {
    crate::remote::relay_access_changed(factory);
}

/// Signed out: the token is forgotten and the tunnel leaves Claktory's
/// relays.
pub(crate) fn signed_out(factory: &Arc<Factory>) {
    forget(factory);
    crate::remote::relay_access_changed(factory);
}

/// After a check-in went through: a token for this tunnel key when there's
/// none, or it has less than 3 days left. Failing, the one kept stays (it
/// may still have days) and the next check-in tries again.
pub(crate) async fn renew(factory: &Arc<Factory>) {
    let config = factory.lock().config.account.clone();
    if factory.account.signed_in_as(&config).is_none() {
        return;
    }
    let Some(endpoint) = endpoint(factory) else { return };
    if kept(factory).is_some_and(|kept| kept.endpoint == endpoint && kept.expires_at - now_s() > RENEW_WITHIN) {
        return;
    }
    let asked = factory.account.signed::<RelayToken>("/v1/relay/token", &RelayTokenRequest { endpoint_id: endpoint.clone() }).await;
    match asked {
        Ok(given) if given.expires_at > now_s() && !given.token.is_empty() => {
            let stored = factory.store.db().execute(
                "INSERT INTO relay_token (id, endpoint, token, expires_at) VALUES (1, ?1, ?2, ?3)
                 ON CONFLICT (id) DO UPDATE SET endpoint = excluded.endpoint, token = excluded.token, expires_at = excluded.expires_at",
                rusqlite::params![endpoint, given.token, given.expires_at],
            );
            match stored {
                Ok(_) => tracing::info!(expires_at = given.expires_at, "got a token for Claktory's relay"),
                Err(error) => tracing::warn!(%error, "could not keep the relay token"),
            }
            crate::remote::relay_access_changed(factory);
        }
        Ok(_) => tracing::warn!("the account service gave a relay token that has already expired"),
        Err(Failure::Refused(_, body)) if body.error == "revoked" => {
            tracing::warn!("no relay token: this Claktory was removed at the account service");
            forget(factory);
            crate::remote::relay_access_changed(factory);
        }
        // The service's error, never the token.
        Err(failure) => tracing::info!(%failure, "could not get a token for Claktory's relay; trying again at the next check-in"),
    }
}
