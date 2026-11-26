//! Signing Claktory's browser out of an identity provider (#435 W6; Fugo
//! 41228, 41235; Giorno 41238), and noticing it signed in to one (Fugo
//! 41632): what the host reads and deletes of the kept profile's cookies.
//!
//! A sign-out deletes every cookie whose domain the provider owns
//! (`errands::fill::owner`) and clears the storage of its sign-in origins,
//! over the browser target while Chrome is driven and no errand drives it.
//! Otherwise it waits in `<home>/browser-sign-outs.json` (this user's only,
//! written whole) until the next driven start, before Chrome runs any page,
//! or until the errand using it ends; Stop leaves it waiting, Delete
//! browser data makes it moot. Only once the cookies are gone does the
//! vault forget the account (`Factory::signed_out`); meanwhile nothing is
//! filled or pressed on the provider's sites.
//!
//! A sign-in is noticed by its session cookies, read whenever the host
//! reads the signed-in sites: at a start, back from the person's turn, at
//! an errand's end. Only a sign-in that worked leaves them, whatever way it
//! was made. Names and domains are read; never a value.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::errands::fill;

/// The waiting sign-outs' file, in Claktory's home.
const FILE: &str = "browser-sign-outs.json";

/// The cookies that say a provider is signed in: the session's own, set
/// only once a sign-in has worked (a sign-in page alone sets others).
const SESSION_COOKIES: &[(&str, &[&str])] = &[
    ("Google", &["SID", "__Secure-1PSID", "__Secure-3PSID"]),
    ("Apple", &["myacinfo", "X-APPLE-WEBAUTH-TOKEN"]),
    ("Microsoft", &["ESTSAUTH", "ESTSAUTHPERSISTENT", "MSPAuth", "__Host-MSAAUTH"]),
];

/// A sign-out waiting to be applied.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Waiting {
    /// By name, as the vault keys it ("Google").
    pub provider: String,
    /// When the person asked for it, in ms since 1970.
    pub asked_ms: i64,
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    sign_outs: Vec<Waiting>,
}

fn path(home: &Path) -> PathBuf {
    home.join(FILE)
}

/// The sign-outs waiting, as last saved: none when there's no file, and
/// only providers Claktory knows. A file that's there but doesn't read
/// fails closed (Fugo 42798): every provider in `accounts` (those the
/// browser is signed in to) waits, so no sign-out the person asked for is
/// lost and fills on it don't resume.
pub(crate) fn load(home: &Path, accounts: impl FnOnce() -> Vec<String>) -> Vec<Waiting> {
    let bytes = match std::fs::read(path(home)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => return unreadable(&error.to_string(), accounts()),
    };
    let saved: Saved = match serde_json::from_slice(&bytes) {
        Ok(saved) => saved,
        Err(error) => return unreadable(&error.to_string(), accounts()),
    };
    let mut waiting: Vec<Waiting> = saved.sign_outs.into_iter().filter(|one| known(&one.provider).is_some()).collect();
    waiting.dedup_by(|a, b| a.provider == b.provider);
    waiting
}

/// What waits when the file can't be read: every provider signed in to.
fn unreadable(why: &str, accounts: Vec<String>) -> Vec<Waiting> {
    tracing::warn!(why, "the browser's waiting sign-outs didn't read: every provider it's signed in to waits");
    let mut waiting: Vec<Waiting> = accounts
        .into_iter()
        .filter_map(|provider| known(&provider).map(|known| Waiting { provider: known.name.to_owned(), asked_ms: 0 }))
        .collect();
    waiting.dedup_by(|a, b| a.provider == b.provider);
    waiting
}

/// Saves `waiting` (removing the file when it's empty).
pub(crate) fn save(home: &Path, waiting: &[Waiting]) -> std::io::Result<()> {
    if waiting.is_empty() {
        return match std::fs::remove_file(path(home)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let text = serde_json::to_vec_pretty(&Saved { sign_outs: waiting.to_vec() }).unwrap_or_default();
    crate::remote::write_private(&path(home), &text)
}

/// The provider called `name`, as Claktory knows it ("google" is Google).
pub(crate) fn known(name: &str) -> Option<&'static fill::IdentityProvider> {
    fill::IDENTITY_PROVIDERS.iter().find(|known| known.name.eq_ignore_ascii_case(name.trim()))
}

/// The cookies in a `Storage.getCookies` result.
fn cookies(result: &Value) -> impl Iterator<Item = &Value> {
    result["cookies"].as_array().into_iter().flatten()
}

/// The providers whose session cookies are among `result`'s.
pub(crate) fn signed_in(result: &Value) -> Vec<&'static str> {
    let mut providers: Vec<&'static str> = cookies(result)
        .filter_map(|cookie| {
            let provider = fill::owner(cookie["domain"].as_str()?)?;
            let name = cookie["name"].as_str()?;
            SESSION_COOKIES.iter().any(|(who, names)| *who == provider && names.contains(&name)).then_some(provider)
        })
        .collect();
    providers.sort_unstable();
    providers.dedup();
    providers
}

/// `provider`'s cookies among `result`'s, as `Storage.setCookies` takes
/// them to delete each (expired, empty): the way Puppeteer deletes one.
pub(crate) fn expired(result: &Value, provider: &str) -> Vec<Value> {
    cookies(result)
        .filter(|cookie| cookie["domain"].as_str().and_then(fill::owner) == Some(provider))
        .map(|cookie| {
            let mut gone = json!({
                "name": cookie["name"],
                "value": "",
                "domain": cookie["domain"],
                "path": cookie["path"],
                "secure": cookie["secure"],
                "httpOnly": cookie["httpOnly"],
                "expires": 1,
            });
            for kept in ["sameSite", "partitionKey", "sourceScheme", "sourcePort"] {
                if !cookie[kept].is_null() {
                    gone[kept] = cookie[kept].clone();
                }
            }
            gone
        })
        .collect()
}

/// How many of `provider`'s cookies `result` still has.
pub(crate) fn left(result: &Value, provider: &str) -> usize {
    cookies(result).filter(|cookie| cookie["domain"].as_str().and_then(fill::owner) == Some(provider)).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jar() -> Value {
        json!({"cookies": [
            {"name": "SID", "value": "s1", "domain": ".google.com", "path": "/", "secure": true, "httpOnly": true},
            {"name": "NID", "value": "s2", "domain": ".google.co.uk", "path": "/", "secure": true, "httpOnly": true, "sameSite": "None"},
            {"name": "VISITOR", "value": "s3", "domain": ".youtube.com", "path": "/", "secure": true, "httpOnly": false},
            {"name": "sid", "value": "s4", "domain": "shop.example", "path": "/", "secure": true, "httpOnly": true},
            {"name": "OTZ", "value": "s5", "domain": "accounts.google.com", "path": "/", "secure": true, "httpOnly": false,
             "partitionKey": {"topLevelSite": "https://shop.example", "hasCrossSiteAncestor": true}},
        ]})
    }

    #[test]
    fn a_sign_out_deletes_every_cookie_the_provider_owns_and_no_other() {
        let gone = expired(&jar(), "Google");
        let domains: Vec<&str> = gone.iter().map(|cookie| cookie["domain"].as_str().unwrap()).collect();
        assert_eq!(domains, [".google.com", ".google.co.uk", ".youtube.com", "accounts.google.com"]);
        for cookie in &gone {
            assert_eq!(cookie["value"], "");
            assert_eq!(cookie["expires"], 1);
            assert!(!cookie.to_string().contains("\"s1\""), "never a value: {cookie}");
        }
        assert_eq!(gone[1]["sameSite"], "None");
        assert_eq!(gone[3]["partitionKey"]["topLevelSite"], "https://shop.example");
        assert_eq!(left(&jar(), "Google"), 4);
        assert_eq!(left(&jar(), "Apple"), 0);
        assert!(expired(&jar(), "Apple").is_empty());
    }

    #[test]
    fn only_a_session_cookie_says_it_signed_in() {
        assert_eq!(signed_in(&jar()), ["Google"]);
        // A sign-in page's cookies alone (NID, OTZ) aren't a sign-in.
        let page = json!({"cookies": [{"name": "NID", "domain": ".google.com"}, {"name": "SID", "domain": "shop.example"}]});
        assert!(signed_in(&page).is_empty());
        let apple = json!({"cookies": [{"name": "myacinfo", "domain": ".apple.com"}, {"name": "MSPAuth", "domain": ".live.com"}]});
        assert_eq!(signed_in(&apple), ["Apple", "Microsoft"]);
    }

    #[test]
    fn waiting_sign_outs_are_kept_whole_and_only_this_users() {
        use std::os::unix::fs::PermissionsExt;
        let home = std::env::temp_dir().join(format!("claktory-signouts-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        assert!(load(&home, Vec::new).is_empty());
        let waiting = vec![Waiting { provider: "Google".into(), asked_ms: 5 }];
        let none = Vec::new;
        save(&home, &waiting).unwrap();
        assert_eq!(load(&home, none), waiting);
        assert_eq!(std::fs::metadata(path(&home)).unwrap().permissions().mode() & 0o777, 0o600);
        // A provider Claktory doesn't know is no sign-out.
        std::fs::write(path(&home), r#"{"sign_outs": [{"provider": "Evil", "asked_ms": 1}]}"#).unwrap();
        assert!(load(&home, none).is_empty());
        // A file that doesn't read: every provider it's signed in to waits.
        std::fs::write(path(&home), "{").unwrap();
        let signed_in = || vec!["Google".to_owned(), "Microsoft".to_owned(), "Evil".to_owned()];
        let providers: Vec<String> = load(&home, signed_in).into_iter().map(|one| one.provider).collect();
        assert_eq!(providers, ["Google", "Microsoft"]);
        save(&home, &[]).unwrap();
        assert!(!path(&home).exists());
        std::fs::remove_dir_all(&home).unwrap();
    }
}
