//! This computer's own app signs in to the account as a device through this
//! Claktory's sign-in (#529): VouchDevice. The Claktory, signed in, asks its
//! account service for a one-time code for the app's account key
//! (`POST /v1/devices/vouch`, signed as this Claktory), and hands it to the
//! app, which signs in with it at the service itself (sign-in/verify, signed
//! by that key: the proof it holds the key). claktory.com alone then vouches
//! for the app to other Claktories, with its ordinary device assertions;
//! this Claktory never signs anything they read.
//!
//! Only this computer's own app may ask (`connection`); the code goes
//! straight back to it, never logged or kept.

use std::sync::Arc;

use claktory_account::protocol::{DeviceVouch, DeviceVouched};
use factory_protocol::v1::{self, ErrorCode};

use super::{Failure, refused, service_name, try_again};
use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;

/// The longest name, system and version passed on, as the service tidies
/// them anyway.
const FIELD_CHARS: usize = 64;

/// VouchDevice from this computer's own app (the caller checks it is).
pub(crate) async fn vouch(factory: &Arc<Factory>, request: &v1::VouchDevice) -> Result<v1::DeviceVouched, RequestError> {
    let config = factory.lock().config.account.clone();
    let Some((_, email, service)) = factory.account.signed_in_as(&config) else {
        return Err(refused(
            ErrorCode::SignInRequired,
            "This Claktory isn't signed in to an account. Sign it in under Settings → Account first.",
        ));
    };
    let account_key = request.account_key.trim();
    if account_key.is_empty() || account_key.len() > 64 {
        return Err(refused(ErrorCode::InvalidRequest, "That isn't an account key."));
    }
    let field = |text: &str| text.trim().chars().take(FIELD_CHARS).collect::<String>();
    let body = DeviceVouch {
        account_key: account_key.to_owned(),
        name: field(&request.name),
        platform: field(&request.platform),
        version: field(&request.version),
    };
    let name = service_name(&service);
    let answered = factory.account.signed::<DeviceVouched>("/v1/devices/vouch", &body).await;
    match answered {
        Ok(given) if !given.request_id.is_empty() && !given.code.is_empty() && given.expires_in > 0 => {
            tracing::info!("this computer's app was given a code to sign in to the account as a device");
            Ok(v1::DeviceVouched {
                request_id: given.request_id,
                code: given.code,
                expires_ms: now_ms() + given.expires_in.saturating_mul(1000),
                service,
                email,
                ..v1::DeviceVouched::default()
            })
        }
        Ok(_) => Err(refused(ErrorCode::Unreachable, format!("{name} gave an answer Claktory can't use. Try again later."))),
        Err(failure) => said_no(&name, failure),
    }
}

/// The service's no, as the app acts on it: what concerns the app's key in
/// `refused`, the rest as errors.
fn said_no(name: &str, failure: Failure) -> Result<v1::DeviceVouched, RequestError> {
    let no = |why: &str, message: String| Ok(v1::DeviceVouched { refused: why.to_owned(), message, ..v1::DeviceVouched::default() });
    match failure {
        Failure::Refused(404, _) => no("unavailable", format!("{name} can't sign this app in through this Claktory yet.")),
        Failure::Refused(_, body) => match body.error.as_str() {
            "device_removed" => {
                no("device_removed", "This app was removed from your account. Sign in again to start over as a new device.".to_owned())
            }
            "key_of_another_account" => no("key_of_another_account", body.message),
            "rate_limited" => {
                let when = try_again(body.retry_after.unwrap_or(60), &jiff::ZonedDateTime::now());
                Err(refused(ErrorCode::RateLimited, format!("Too many sign-ins through this Claktory. Try again {when}.")))
            }
            "revoked" => Err(refused(
                ErrorCode::SignInRequired,
                format!("This Claktory was removed from your account at {name}. Sign it in again under Settings → Account."),
            )),
            "internal" => Err(refused(ErrorCode::Unreachable, format!("{name} has a problem right now. Try again later."))),
            _ => {
                tracing::warn!(error = %body.error, "the account service refused to vouch for this computer's app");
                Err(refused(ErrorCode::InvalidRequest, body.message))
            }
        },
        Failure::OtherKey | Failure::Unreachable(_) => {
            tracing::warn!(%failure, "could not ask the account service to vouch for this computer's app");
            Err(refused(ErrorCode::Unreachable, format!("Can't reach {name}. Check your internet connection and try again.")))
        }
    }
}

#[cfg(test)]
mod tests {
    use claktory_account::protocol::ErrorBody;

    use super::*;

    fn body(error: &str) -> ErrorBody {
        serde_json::from_value(serde_json::json!({"error": error, "message": "said"})).unwrap()
    }

    #[test]
    fn the_services_no_tells_the_app_what_to_do() {
        let refused = |status, error| said_no("claktory.com", Failure::Refused(status, body(error))).ok().map(|given| given.refused);
        assert_eq!(refused(404, "not_found").as_deref(), Some("unavailable"));
        assert_eq!(refused(403, "device_removed").as_deref(), Some("device_removed"));
        assert_eq!(refused(403, "key_of_another_account").as_deref(), Some("key_of_another_account"));
        let code = |status, error| match said_no("claktory.com", Failure::Refused(status, body(error))) {
            Err(error) => error.code(),
            Ok(_) => ErrorCode::Unspecified,
        };
        assert_eq!(code(429, "rate_limited"), ErrorCode::RateLimited);
        assert_eq!(code(403, "revoked"), ErrorCode::SignInRequired);
        assert_eq!(code(500, "internal"), ErrorCode::Unreachable);
        assert!(
            matches!(said_no("claktory.com", Failure::Unreachable("down".into())), Err(error) if error.code() == ErrorCode::Unreachable)
        );
    }
}
