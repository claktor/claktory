//! QA only: Anthropic's subscription pointed at qa-model on this computer,
//! so a QA sandbox can show what a Claude sign-in does at its usage limit
//! without a real account. Compiled only with the `qa-provider`
//! feature, which scripts/qa.sh builds sandboxes with and a release build
//! can't have (lib.rs), and in this crate's own tests.
//!
//! Anything that redirects a subscription's requests could
//! send the person's real Claude token elsewhere, so even when compiled in,
//! the override is taken only for an address on this computer (loopback),
//! never for the real Claktories' homes (`~/.factory`, `~/.factory-dev`)
//! or ports (7420, 7421). factoryd refuses to start otherwise, saying why.
//! It covers everything the subscription reaches: the messages, the usage,
//! plan and account reads, the resets and the sign-in's token.
//!
//! OpenRouter's model list can come from qa-model the same way
//! (`MODELS_URL_VAR`, within the same guard), so a sandbox shows a list
//! arriving, refreshing and changing without reaching openrouter.ai.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::oauth::AnthropicOAuth;
use crate::providers::Provider;

/// The variable factoryd reads the override from.
pub const URL_VAR: &str = "FACTORY_QA_ANTHROPIC_URL";

/// The variable factoryd reads OpenRouter's model list's address from.
pub const MODELS_URL_VAR: &str = "FACTORY_QA_OPENROUTER_MODELS_URL";

/// The real Claktories' ports, never a sandbox's.
const REAL_PORTS: [u16; 2] = [7420, 7421];

/// The Anthropic provider factoryd uses, for a backend in `home` on `port`,
/// when `url` (the variable's value) asks for one: None without it, the
/// provider at that address when it passes the guard, or why not.
pub fn provider_for(url: Option<&str>, home: &Path, port: u16) -> Result<Option<(String, Provider)>, String> {
    let Some(url) = url else { return Ok(None) };
    let base = check(URL_VAR, url, home, &real_homes(), port)?;
    Ok(Some((base.clone(), provider_at(&base))))
}

/// Where OpenRouter's model list comes from, for a backend in `home` on
/// `port`, when `url` (MODELS_URL_VAR's value) asks: None without it, the
/// address when it passes the guard, or why not.
pub fn models_url_for(url: Option<&str>, home: &Path, port: u16) -> Result<Option<String>, String> {
    let Some(url) = url else { return Ok(None) };
    check(MODELS_URL_VAR, url, home, &real_homes(), port).map(Some)
}

/// The address, as a base URL without a trailing slash, when the guard
/// lets it through.
fn check(var: &str, url: &str, home: &Path, real_homes: &[PathBuf], port: u16) -> Result<String, String> {
    let parsed =
        reqwest::Url::parse(url.trim()).map_err(|error| format!("{var} isn't an address ({error}): the QA override is refused."))?;
    // An IP address only, never a name such as localhost that could resolve elsewhere.
    let loopback = parsed
        .host_str()
        .map(|host| host.trim_start_matches('[').trim_end_matches(']'))
        .and_then(|host| host.parse::<std::net::IpAddr>().ok())
        .is_some_and(|ip| ip.is_loopback());
    if !matches!(parsed.scheme(), "http" | "https") || !loopback || parsed.username() != "" || parsed.password().is_some() {
        return Err(format!(
            "{var} is {url}: the QA override takes only an http(s) address on this computer (127.0.0.1 or [::1]), so it's refused."
        ));
    }
    if REAL_PORTS.contains(&port) {
        return Err(format!("{var} is set on port {port}, a real Claktory's: the QA override is refused."));
    }
    let same = |a: &Path, b: &Path| {
        let canonical = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        canonical(a) == canonical(b)
    };
    if let Some(real) = real_homes.iter().find(|real| same(home, real)) {
        return Err(format!("{var} is set for {}, a real Claktory's home: the QA override is refused.", real.display()));
    }
    Ok(parsed.as_str().trim_end_matches('/').to_owned())
}

/// The real Claktories' homes, under this user's home as the user database
/// has it (a sandbox's factoryd runs with a HOME of its own) and as HOME says.
#[cfg(windows)]
fn real_homes() -> Vec<PathBuf> {
    // Where a Windows Claktory lives (factoryd's windows::dirs), and the
    // Mac-style homes under the profile, in case one was made there.
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).map(|local| local.join("Claktory"));
    let profile = std::env::var_os("USERPROFILE").map(PathBuf::from);
    let mut homes: Vec<PathBuf> = local.iter().flat_map(|claktory| [claktory.join("factory"), claktory.join("factory-dev")]).collect();
    homes.extend(profile.iter().flat_map(|user| [user.join(".factory"), user.join(".factory-dev")]));
    homes
}

#[cfg(unix)]
fn real_homes() -> Vec<PathBuf> {
    let mut users: Vec<PathBuf> = std::env::var_os("HOME").map(PathBuf::from).into_iter().collect();
    // SAFETY: getpwuid_r fills `entry` from `buffer`, both ours and big
    // enough; its home is read before either goes away.
    unsafe {
        let mut entry: libc::passwd = std::mem::zeroed();
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let mut buffer = vec![0 as libc::c_char; 16 * 1024];
        if libc::getpwuid_r(libc::getuid(), &mut entry, buffer.as_mut_ptr(), buffer.len(), &mut found) == 0
            && !found.is_null()
            && !entry.pw_dir.is_null()
            && let Ok(dir) = std::ffi::CStr::from_ptr(entry.pw_dir).to_str()
        {
            users.push(PathBuf::from(dir));
        }
    }
    users.iter().flat_map(|user| [user.join(".factory"), user.join(".factory-dev")]).collect()
}

/// Anthropic's provider with every address it reaches at `base`: the API
/// (messages, usage, plan, account, resets, key checks), every model, and
/// the sign-in's authorize and token addresses.
fn provider_at(base: &str) -> Provider {
    let mut provider = super::provider();
    provider.base_url = Box::leak(base.to_owned().into_boxed_str());
    for model in &mut provider.models {
        model.base_url = base.to_owned();
    }
    let mut oauth = AnthropicOAuth::new();
    oauth.authorize_url = format!("{base}/oauth/authorize");
    oauth.token_url = format!("{base}/v1/oauth/token");
    provider.oauth = Some(Arc::new(oauth));
    provider
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_address_on_this_computer_is_taken() {
        let homes = [PathBuf::from("/Users/someone/.factory"), PathBuf::from("/Users/someone/.factory-dev")];
        let sandbox = Path::new("/tmp/claktory-qa/sandboxes/qa1/home");
        assert_eq!(check(URL_VAR, "http://127.0.0.1:7503/", sandbox, &homes, 7500), Ok("http://127.0.0.1:7503".to_owned()));
        assert_eq!(check(URL_VAR, "http://[::1]:7503", sandbox, &homes, 7500), Ok("http://[::1]:7503".to_owned()));
        for url in [
            "https://api.anthropic.com",
            "http://localhost:7503",
            "http://10.0.0.2:7503",
            "http://127.0.0.1.example.com:7503",
            "http://user:pass@127.0.0.1:7503",
            "file:///tmp/x",
            "127.0.0.1:7503",
            "",
        ] {
            let refused = check(URL_VAR, url, sandbox, &homes, 7500).expect_err(url);
            assert!(refused.contains("refused"), "{url}: {refused}");
        }
    }

    #[test]
    fn the_real_claktories_homes_and_ports_are_refused() {
        let dir = tempfile::tempdir().expect("dir");
        let real = dir.path().join(".factory");
        std::fs::create_dir_all(&real).expect("home");
        let homes = [real.clone(), dir.path().join(".factory-dev")];
        let url = "http://127.0.0.1:7503";
        for port in [7420, 7421] {
            assert!(check(URL_VAR, url, Path::new("/tmp/qa/home"), &homes, port).expect_err("port").contains(&port.to_string()));
        }
        // Named another way (a trailing slash, a symlink), it's the same home.
        let refused = check(URL_VAR, url, &dir.path().join(".factory/"), &homes, 7500).expect_err("home");
        assert!(refused.contains("a real Claktory's home"), "{refused}");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&real, dir.path().join("link")).expect("link");
            assert!(check(URL_VAR, url, &dir.path().join("link"), &homes, 7500).is_err());
        }
        assert!(check(URL_VAR, url, &dir.path().join(".factory-dev"), &homes, 7500).is_err());
        assert!(check(URL_VAR, url, &dir.path().join("sandbox"), &homes, 7500).is_ok());
        // The user database's home counts, whatever HOME says.
        assert!(real_homes().len() >= 2);
    }

    #[test]
    fn every_address_of_the_subscription_moves() {
        assert_eq!(provider_for(None, Path::new("/tmp/qa/home"), 7500).map(|found| found.is_none()), Ok(true));
        let (base, provider) = provider_for(Some("http://127.0.0.1:7503"), Path::new("/tmp/qa/home"), 7500).expect("ok").expect("some");
        assert_eq!((base.as_str(), provider.id, provider.base_url), ("http://127.0.0.1:7503", "anthropic", "http://127.0.0.1:7503"));
        assert!(!provider.models.is_empty());
        assert!(provider.models.iter().all(|model| model.base_url == base));
        assert!(provider.limits.is_some() && provider.identity.is_some() && provider.reset.is_some());
    }
}
