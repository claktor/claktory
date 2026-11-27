//! Signing Claktory's browser out of an identity provider (#435 W6; Fugo
//! 41228, 41235; Giorno 41238), and noticing it signed in to one (Fugo
//! 41632): what the host reads and deletes of the kept profile's cookies.
//!
//! A sign-out deletes every cookie whose domain the provider owns
//! (`errands::fill::owner`) and clears the storage of its sign-in origins,
//! over the browser target while Chrome is driven and no errand drives it.
//! Otherwise it waits in its browser's file (`<home>/browser-sign-outs.json`
//! for the person's own, `<home>/browser-sign-outs-<profile>.json` for an
//! agent's: this user's only, written whole) until the next driven start, before Chrome runs any page,
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

/// The waiting sign-outs' file of the person's own browser, in Claktory's
/// home; an agent's is `browser-sign-outs-<profile>.json` beside it.
const FILE: &str = "browser-sign-outs.json";

/// The cookies that say a provider is signed in: the session's own, set
/// only once a sign-in has worked (a sign-in page alone sets others).
const SESSION_COOKIES: &[(&str, &[&str])] = &[
    ("Google", &["SID", "__Secure-1PSID", "__Secure-3PSID"]),
    ("Apple", &["myacinfo", "X-APPLE-WEBAUTH-TOKEN"]),
    ("Microsoft", &["ESTSAUTH", "ESTSAUTHPERSISTENT", "MSPAuth", "__Host-MSAAUTH"]),
];

/// A sign-out waiting to be applied: of an identity provider, or of one
/// site.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Waiting {
    /// By name, as the vault keys it ("Google"); empty for a site's.
    #[serde(default)]
    pub provider: String,
    /// The site, as the browser's signed-in sites list it
    /// ("https://x.com"); empty for a provider's.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub site: String,
    /// When the person asked for it, in ms since 1970.
    pub asked_ms: i64,
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    sign_outs: Vec<Waiting>,
}

/// The file of the browser whose profile is `profile` (`super::profile`),
/// the person's own when `agent` is empty.
pub(crate) fn file(home: &Path, agent: &str, profile: &str) -> PathBuf {
    if agent.is_empty() { home.join(FILE) } else { home.join(format!("browser-sign-outs-{profile}.json")) }
}

/// The sign-outs waiting, as last saved: none when there's no file, and
/// only providers Claktory knows. A file that's there but doesn't read
/// fails closed: every site in `sites` (those the browser is signed in to)
/// waits, so no sign-out the person asked for is lost.
pub(crate) fn load(file: &Path, sites: impl FnOnce() -> Vec<String>) -> Vec<Waiting> {
    let bytes = match std::fs::read(file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => return unreadable(&error.to_string(), sites()),
    };
    let saved: Saved = match serde_json::from_slice(&bytes) {
        Ok(saved) => saved,
        Err(error) => return unreadable(&error.to_string(), sites()),
    };
    let mut waiting: Vec<Waiting> =
        saved.sign_outs.into_iter().filter(|one| if one.site.is_empty() { known(&one.provider).is_some() } else { site_host(&one.site).is_some() }).collect();
    waiting.dedup_by(|a, b| a.provider == b.provider && a.site == b.site);
    waiting
}

/// What waits when the file can't be read: every site signed in to.
fn unreadable(why: &str, sites: Vec<String>) -> Vec<Waiting> {
    tracing::warn!(why, "the browser's waiting sign-outs didn't read: every site it's signed in to waits");
    let mut waiting: Vec<Waiting> =
        sites.into_iter().filter(|site| site_host(site).is_some()).map(|site| Waiting { provider: String::new(), site, asked_ms: 0 }).collect();
    waiting.dedup_by(|a, b| a.site == b.site);
    waiting
}

/// Saves `waiting` (removing the file when it's empty).
pub(crate) fn save(file: &Path, waiting: &[Waiting]) -> std::io::Result<()> {
    if waiting.is_empty() {
        return match std::fs::remove_file(file) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let text = serde_json::to_vec_pretty(&Saved { sign_outs: waiting.to_vec() }).unwrap_or_default();
    crate::remote::write_private(file, &text)
}

/// The provider called `name`, as Claktory knows it ("google" is Google).
pub(crate) fn known(name: &str) -> Option<&'static fill::IdentityProvider> {
    fill::IDENTITY_PROVIDERS.iter().find(|known| known.name.eq_ignore_ascii_case(name.trim()))
}

/// The host of `site`, a signed-in site as listed ("https://x.com"):
/// None for anything else.
pub(crate) fn site_host(site: &str) -> Option<&str> {
    let host = site.strip_prefix("https://")?;
    let plain = !host.is_empty() && host.len() <= 253 && host.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-');
    (plain && !host.starts_with('.') && !host.ends_with('.')).then_some(host)
}

/// Whether a cookie for `domain` is one of `host`'s: for it, or for a
/// name under it.
fn of_site(domain: &str, host: &str) -> bool {
    let domain = domain.trim_start_matches('.').to_ascii_lowercase();
    domain == host || domain.strip_suffix(host).is_some_and(|rest| rest.ends_with('.'))
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

/// Whether a cookie for `domain` goes with sign-out `waiting`.
fn goes(waiting: &Waiting, domain: &str) -> bool {
    match site_host(&waiting.site) {
        Some(host) => of_site(domain, host),
        None => waiting.site.is_empty() && fill::owner(domain).is_some_and(|owner| owner == waiting.provider),
    }
}

/// The cookies `waiting` signs out of among `result`'s, as
/// `Storage.setCookies` takes them to delete each (expired, empty): the way
/// Puppeteer deletes one.
pub(crate) fn expired(result: &Value, waiting: &Waiting) -> Vec<Value> {
    cookies(result)
        .filter(|cookie| cookie["domain"].as_str().is_some_and(|domain| goes(waiting, domain)))
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

/// How many of the cookies `waiting` signs out of `result` still has.
pub(crate) fn left(result: &Value, waiting: &Waiting) -> usize {
    cookies(result).filter(|cookie| cookie["domain"].as_str().is_some_and(|domain| goes(waiting, domain))).count()
}

/// The origins whose storage `waiting` clears.
pub(crate) fn origins(waiting: &Waiting) -> Vec<String> {
    match (site_host(&waiting.site), known(&waiting.provider)) {
        (Some(_), _) => vec![waiting.site.clone()],
        (None, Some(known)) if waiting.site.is_empty() => known.origins.iter().map(|origin| (*origin).to_owned()).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(provider: &str) -> Waiting {
        Waiting { provider: provider.to_owned(), site: String::new(), asked_ms: 0 }
    }

    fn site(site: &str) -> Waiting {
        Waiting { provider: String::new(), site: site.to_owned(), asked_ms: 0 }
    }

    #[test]
    fn a_sites_sign_out_deletes_its_cookies_and_its_names_under_it_only() {
        let jar = json!({"cookies": [
            {"name": "auth", "value": "a", "domain": ".x.com", "path": "/"},
            {"name": "ct0", "value": "b", "domain": "api.x.com", "path": "/"},
            {"name": "c", "value": "c", "domain": "box.com", "path": "/"},
            {"name": "d", "value": "d", "domain": ".notx.com", "path": "/"},
            {"name": "e", "value": "e", "domain": ".com", "path": "/"},
        ]});
        let x = site("https://x.com");
        let gone = expired(&jar, &x);
        let domains: Vec<&str> = gone.iter().map(|cookie| cookie["domain"].as_str().unwrap()).collect();
        assert_eq!(domains, [".x.com", "api.x.com"]);
        assert_eq!(left(&jar, &x), 2);
        assert_eq!(origins(&x), ["https://x.com"]);
        assert_eq!(left(&jar, &site("https://api.x.com")), 1, "a name under it leaves the site above alone");
        for bad in ["http://x.com", "https://", "https://x.com/", "https://X.com", "https://.com", "x.com", "https://a b"] {
            assert!(site_host(bad).is_none() && expired(&jar, &site(bad)).is_empty() && origins(&site(bad)).is_empty(), "{bad}");
        }
        assert!(origins(&of("Google")).iter().all(|origin| origin.starts_with("https://")) && !origins(&of("Google")).is_empty());
    }

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
        let gone = expired(&jar(), &of("Google"));
        let domains: Vec<&str> = gone.iter().map(|cookie| cookie["domain"].as_str().unwrap()).collect();
        assert_eq!(domains, [".google.com", ".google.co.uk", ".youtube.com", "accounts.google.com"]);
        for cookie in &gone {
            assert_eq!(cookie["value"], "");
            assert_eq!(cookie["expires"], 1);
            assert!(!cookie.to_string().contains("\"s1\""), "never a value: {cookie}");
        }
        assert_eq!(gone[1]["sameSite"], "None");
        assert_eq!(gone[3]["partitionKey"]["topLevelSite"], "https://shop.example");
        assert_eq!(left(&jar(), &of("Google")), 4);
        assert_eq!(left(&jar(), &of("Apple")), 0);
        assert!(expired(&jar(), &of("Apple")).is_empty());
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
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;
        let home = std::env::temp_dir().join(format!("claktory-signouts-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        // The person's own browser keeps the file it always had; each
        // agent's has its own.
        assert_eq!(file(&home, "", "c0123"), home.join("browser-sign-outs.json"));
        assert_eq!(file(&home, "yukako", "c0123-a4f"), home.join("browser-sign-outs-c0123-a4f.json"));
        let path = file(&home, "yukako", "c0123-a4f");
        assert!(load(&path, Vec::new).is_empty());
        let waiting = vec![Waiting { provider: "Google".into(), site: String::new(), asked_ms: 5 }, site("https://x.com")];
        let none = Vec::new;
        save(&path, &waiting).unwrap();
        assert_eq!(load(&path, none), waiting);
        // A Claktory before sites' sign-outs reads the provider's and
        // passes over the site's, whose provider is no one it knows.
        let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["sign_outs"][1]["provider"], "");
        // A site that isn't one as listed is no sign-out.
        std::fs::write(&path, r#"{"sign_outs": [{"site": "file:///etc", "asked_ms": 1}]}"#).unwrap();
        assert!(load(&path, none).is_empty());
        #[cfg(unix)]
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        #[cfg(windows)]
        assert!(factory_agent::windows::system::is_private_path(&path).unwrap());
        // A provider Claktory doesn't know is no sign-out.
        std::fs::write(&path, r#"{"sign_outs": [{"provider": "Evil", "asked_ms": 1}]}"#).unwrap();
        assert!(load(&path, none).is_empty());
        // A file that doesn't read: every site it's signed in to waits.
        std::fs::write(&path, "{").unwrap();
        let signed_in = || vec!["https://x.com".to_owned(), "https://mail.google.com".to_owned(), "file:///etc".to_owned()];
        let sites: Vec<String> = load(&path, signed_in).into_iter().map(|one| one.site).collect();
        assert_eq!(sites, ["https://x.com", "https://mail.google.com"]);
        save(&path, &[]).unwrap();
        assert!(!path.exists());
        std::fs::remove_dir_all(&home).unwrap();
    }
}
