//! claktory.com's signing key is taken only on its account root's word
//! (F-OK1): a statement at well-known (`api::KeyStatement`), signed offline
//! by one of `api::ACCOUNT_ROOTS`, that the key is the service's from a time
//! on. At sign-in a key without one is refused; a key the service changed to
//! is taken by itself only with a statement newer than the pinned key's; and
//! a pinned key from before this build's floor (a key that leaked, say) no
//! longer checks devices' assertions.
//!
//! Only claktory.com is anchored, and only once a build sets `FLOOR`: a
//! service someone runs for themselves has no root here, so its key is taken
//! at sign-in, as before. Until then statements are only noted (the pinned
//! key's `from`), so Claktories signed in before have one when it's set.

use std::sync::{Mutex, MutexGuard};

use claktory_account::protocol::{self as api, WellKnown};

/// The earliest `from` a statement for claktory.com's key may have, in this
/// build: the current key's, bumped by a release after a rotation. None
/// until the service serves the account root's statement.
const FLOOR: Option<i64> = None;

/// How a service's key is checked.
#[derive(Clone, Debug)]
struct Anchor {
    roots: Vec<(String, String)>,
    floor: i64,
}

/// Services tests anchor to roots of their own (`anchor_for_tests`).
static TESTS: Mutex<Vec<(String, Anchor)>> = Mutex::new(Vec::new());

fn tests() -> MutexGuard<'static, Vec<(String, Anchor)>> {
    TESTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// For tests, which run their own service: anchors `service` as claktory.com
/// is, to `roots` (id, standard base64 ed25519 key) and `floor`.
#[doc(hidden)]
pub fn anchor_for_tests(service: &str, roots: Vec<(String, String)>, floor: i64) {
    let mut tests = tests();
    tests.retain(|(known, _)| known != service);
    tests.push((service.trim_end_matches('/').to_owned(), Anchor { roots, floor }));
}

fn anchor(service: &str) -> Option<Anchor> {
    if service == super::DEFAULT_SERVICE {
        return FLOOR.map(|floor| Anchor { roots: own(api::ACCOUNT_ROOTS), floor });
    }
    tests().iter().find(|(known, _)| known == service).map(|(_, anchor)| anchor.clone())
}

fn own(roots: &[(&str, &str)]) -> Vec<(String, String)> {
    roots.iter().map(|(id, key)| ((*id).to_owned(), (*key).to_owned())).collect()
}

/// The `from` of the statement `known` serves, if one of `roots` signed it
/// for `service` and the key it serves.
fn statement_from(service: &str, known: &WellKnown, roots: &[(String, String)]) -> Result<i64, String> {
    let signed = known.service_key_statement.as_ref().ok_or("it serves no statement for its key")?;
    let roots: Vec<(&str, &str)> = roots.iter().map(|(id, key)| (id.as_str(), key.as_str())).collect();
    let statement = api::verify_statement(signed, &roots)?;
    if statement.service != service {
        return Err(format!("its statement is for {}", statement.service));
    }
    if statement.key != known.service_key {
        return Err("its statement is for another key".to_owned());
    }
    Ok(statement.from)
}

/// What to make of the key `service`'s well-known names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Vouch {
    /// Not anchored: taken as it is; the `from` of a root's statement for
    /// it, when there's one.
    Free(Option<i64>),
    /// Anchored, and the root's statement for it is from `from`, at or
    /// after this build's floor.
    Vouched(i64),
    /// Anchored, and not vouched for: why.
    Refused(String),
}

pub(super) fn vouch(service: &str, known: &WellKnown) -> Vouch {
    match anchor(service) {
        None => Vouch::Free(statement_from(service, known, &own(api::ACCOUNT_ROOTS)).ok()),
        Some(anchor) => match statement_from(service, known, &anchor.roots) {
            Ok(from) if from >= anchor.floor => Vouch::Vouched(from),
            Ok(from) => Vouch::Refused(format!("its statement is from {from}, before {}", anchor.floor)),
            Err(why) => Vouch::Refused(why),
        },
    }
}

/// Whether `service` is anchored: a key it changes to needs a statement.
pub(super) fn anchored(service: &str) -> bool {
    anchor(service).is_some()
}

/// Whether a key of `service` pinned with a statement from `from` (None:
/// pinned without one) is retired: from before this build's floor.
pub(super) fn retired(service: &str, from: Option<i64>) -> bool {
    anchor(service).is_some_and(|anchor| from.is_none_or(|from| from < anchor.floor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::{Manifest, RELEASE_KEYS};

    #[test]
    fn the_account_roots_and_the_release_keys_stand_in_for_each_other_only_through_the_offline_recovery_key() {
        assert!(
            api::ACCOUNT_ROOTS.iter().all(|(id, _)| *id != "claktory-release-1"),
            "the release working key, which publishes, never vouches for the service"
        );
        assert!(RELEASE_KEYS.iter().all(|(id, _)| !id.starts_with("claktory-account-root")), "the account root never signs releases");
        // A key both pin is only the offline recovery key, under its own id.
        for (id, key) in api::ACCOUNT_ROOTS {
            for (release, release_key) in RELEASE_KEYS {
                if release_key == key || release == id {
                    assert_eq!((*id, *release, *release_key), ("claktory-release-recovery-1", "claktory-release-recovery-1", *key));
                }
            }
        }
    }

    #[test]
    fn a_statement_is_never_a_release_manifest_and_a_manifest_never_a_statement() {
        let statement = api::KeyStatement { service: "https://account.claktory.com".into(), key: api::b64([1u8; 32]), from: 1_795_000_000 };
        assert!(Manifest::parse(statement.text().as_bytes()).is_err());
        let manifest = br#"{"version":"v0.1.14","url":"https://example.com/release.tar.gz","sha256":"00"}"#;
        assert_eq!(api::KeyStatement::parse(std::str::from_utf8(manifest).unwrap()), None);
    }

    #[test]
    fn a_key_pinned_before_the_floor_or_without_a_statement_is_retired() {
        anchor_for_tests("http://retired.test", Vec::new(), 100);
        assert!(retired("http://retired.test", None), "pinned without a statement");
        assert!(retired("http://retired.test", Some(99)));
        assert!(!retired("http://retired.test", Some(100)));
    }

    #[test]
    fn claktory_com_is_anchored_only_once_a_build_sets_the_floor_and_no_other_service_is() {
        assert_eq!(anchored(super::super::DEFAULT_SERVICE), FLOOR.is_some());
        assert!(!anchored("https://account.example.com"));
        assert!(!retired("https://account.example.com", None));
    }
}
