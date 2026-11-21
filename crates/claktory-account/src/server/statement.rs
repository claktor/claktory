//! The account root's word for the service key (`protocol::KeyStatement`):
//! the file the service serves at well-known, and the check that what it
//! serves holds, as a Claktory checks it (for deploys).

use std::path::Path;

use crate::protocol::{KeyStatement, SignedStatement, WELL_KNOWN, WellKnown, verify_statement};

/// The statement in `path` (JSON: text and signature), if one of `roots`
/// signed it for `service_key`.
pub fn read(path: &Path, service_key: &str, roots: &[(&str, &str)]) -> Result<SignedStatement, String> {
    let text = std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let signed: SignedStatement = serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    let statement = verify_statement(&signed, roots)?;
    if statement.key != service_key {
        return Err(format!("it's for the key {}, not this service's {service_key}", statement.key));
    }
    Ok(signed)
}

/// Fetches `url`'s well-known, without following a redirect, and checks
/// its statement: signed by one of `roots`, for `url` and for the key it
/// serves.
pub async fn check(url: &str, roots: &[(&str, &str)]) -> Result<KeyStatement, String> {
    let url = url.trim_end_matches('/');
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|error| error.to_string())?;
    let answer = client.get(format!("{url}{WELL_KNOWN}")).send().await.map_err(|error| format!("{url}: {error}"))?;
    if !answer.status().is_success() {
        return Err(format!("{url}{WELL_KNOWN} answered {}", answer.status()));
    }
    let known: WellKnown = answer.json().await.map_err(|error| format!("{url}{WELL_KNOWN}: {error}"))?;
    let signed = known.service_key_statement.ok_or_else(|| format!("{url} serves no service-key statement"))?;
    let statement = verify_statement(&signed, roots).map_err(|error| format!("{url}'s statement doesn't hold: {error}"))?;
    if statement.service != url {
        return Err(format!("{url}'s statement is for {}", statement.service));
    }
    if statement.key != known.service_key {
        return Err(format!("{url}'s statement is for the key {}, but it serves {}", statement.key, known.service_key));
    }
    Ok(statement)
}
