//! QA only:
//! a Claktory that lies about whose it is, so QA can check that the
//! person's device trusts claktory.com's list and the endpoint it dials,
//! never a Claktory's word. Built only with the `qa-lies` feature, which a
//! release build refuses (scripts/check-qa-lies.sh checks it), and it lies
//! only when told to:
//!
//! - FACTORY_QA_CLAIM_EMAIL: the Workspace's account email says this one
//!   (say ana@…) instead of the account it's signed in to.
//! - FACTORY_QA_CLAIM_CLAKTORY_ID: its direct-message leaf offer names this
//!   Claktory id (one of the other account's listed Claktories), and so its
//!   leaf id, instead of its own. Set either alone, or both.
//!
//! A real Claktory's folder (~/.factory, ~/.factory-dev) or port (7420,
//! 7421) refuses to start with either set.

#[cfg(not(debug_assertions))]
compile_error!("The qa-lies feature is for QA builds only: a release build never carries it.");

use std::path::Path;
use std::sync::OnceLock;

pub const EMAIL_VAR: &str = "FACTORY_QA_CLAIM_EMAIL";
pub const CLAKTORY_VAR: &str = "FACTORY_QA_CLAIM_CLAKTORY_ID";

/// The real Claktories' ports, never a sandbox's.
const REAL_PORTS: [u16; 2] = [7420, 7421];

#[derive(Debug, Default, PartialEq, Eq)]
struct Lies {
    email: Option<String>,
    claktory_id: Option<String>,
}

static LIES: OnceLock<Lies> = OnceLock::new();

/// Reads the variables once, at startup, for a backend in `home` on `port`:
/// Ok with nothing to lie about when neither is set, or why it refuses.
pub fn init(home: &Path, port: u16) -> Result<(), String> {
    let lies = check(std::env::var(EMAIL_VAR).ok(), std::env::var(CLAKTORY_VAR).ok(), home, port)?;
    if lies != Lies::default() {
        tracing::warn!(email = ?lies.email, claktory_id = ?lies.claktory_id, "QA lies on: this Claktory claims another's account");
    }
    let _ = LIES.set(lies);
    Ok(())
}

fn check(email: Option<String>, claktory_id: Option<String>, home: &Path, port: u16) -> Result<Lies, String> {
    let email = email.map(|email| email.trim().to_owned()).filter(|email| !email.is_empty());
    let claktory_id = claktory_id.map(|id| id.trim().to_owned()).filter(|id| !id.is_empty());
    if email.is_none() && claktory_id.is_none() {
        return Ok(Lies::default());
    }
    if REAL_PORTS.contains(&port) {
        return Err(format!("{EMAIL_VAR} or {CLAKTORY_VAR} is set on port {port}, a real Claktory's: the QA lie is refused."));
    }
    let named = home.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    if matches!(named, ".factory" | ".factory-dev") {
        return Err(format!("{EMAIL_VAR} or {CLAKTORY_VAR} is set in {}, a real Claktory's folder: the QA lie is refused.", home.display()));
    }
    if let Some(email) = &email
        && (email.len() > 254 || !email.contains('@') || email.chars().any(char::is_control))
    {
        return Err(format!("{EMAIL_VAR} isn't an email address: the QA lie is refused."));
    }
    if let Some(id) = &claktory_id
        && (id.len() > 128 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
    {
        return Err(format!("{CLAKTORY_VAR} isn't a Claktory id: the QA lie is refused."));
    }
    Ok(Lies { email, claktory_id })
}

/// The account email the Workspace shows: the lie when one is set.
pub(crate) fn email(honest: String) -> String {
    LIES.get().and_then(|lies| lies.email.clone()).unwrap_or(honest)
}

/// The Claktory id the leaf offer names: the lie when one is set.
pub(crate) fn claktory_id(honest: String) -> String {
    LIES.get().and_then(|lies| lies.claktory_id.clone()).unwrap_or(honest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_set_lies_about_nothing() {
        assert_eq!(check(None, Some("  ".into()), Path::new("/tmp/qa/home"), 7500), Ok(Lies::default()));
    }

    #[test]
    fn either_lie_alone_or_both() {
        let home = Path::new("/tmp/qa/sandboxes/bo/home");
        let email = check(Some("ana@example.com".into()), None, home, 7501).unwrap();
        assert_eq!(email, Lies { email: Some("ana@example.com".into()), claktory_id: None });
        let both = check(Some("ana@example.com".into()), Some("c-ana_1".into()), home, 7501).unwrap();
        assert_eq!(both.claktory_id.as_deref(), Some("c-ana_1"));
    }

    #[test]
    fn a_real_claktory_refuses_to_lie() {
        let lie = || Some("ana@example.com".to_owned());
        assert!(check(lie(), None, Path::new("/tmp/qa/home"), 7420).is_err());
        assert!(check(lie(), None, Path::new("/tmp/qa/home"), 7421).is_err());
        assert!(check(lie(), None, Path::new("/Users/someone/.factory"), 7500).is_err());
        assert!(check(None, Some("c1".into()), Path::new("/Users/someone/.factory-dev"), 7500).is_err());
    }

    #[test]
    fn malformed_lies_are_refused() {
        let home = Path::new("/tmp/qa/home");
        assert!(check(Some("not an email".into()), None, home, 7500).is_err());
        assert!(check(None, Some("../c1".into()), home, 7500).is_err());
    }
}
