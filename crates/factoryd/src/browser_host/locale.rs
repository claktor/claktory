//! The time zone and languages Claktory's browser's pages see (#435,
//! change 3): the person's own, so a site sees what it would from their
//! own browser, never a server's UTC and en-US. Kept in config.json
//! (`browser`), set in Settings (SetAgentBrowserLocale) or filled once from
//! the first device that opens or watches the browser, and given to the
//! runner at each start. The person's only (rivet_reach), and checked here
//! and again in the runner (Fugo 37384): what reaches Chrome's environment
//! and command line is a well-formed IANA name and BCP 47 tags, nothing
//! else.

use std::sync::Arc;

use factory_protocol::v1;

use super::{refused, view};
use crate::browser_runner::wire::{self, MAX_LANGUAGES};
use crate::config::BrowserConfig;
use crate::factory::{Factory, RequestError};

/// Whether `zone` is an IANA name this Claktory knows.
pub(crate) fn zone_known(zone: &str) -> bool {
    wire::zone_name_ok(zone) && jiff::tz::db().get(zone).is_ok()
}

/// The well-formed languages of `tags`, each once, in order.
fn unique(tags: &[String]) -> Vec<String> {
    let mut unique: Vec<String> = Vec::new();
    for tag in tags.iter().map(|tag| tag.trim()) {
        if wire::language_ok(tag) && !unique.iter().any(|known| known.eq_ignore_ascii_case(tag)) {
            unique.push(tag.to_owned());
        }
    }
    unique
}

/// The languages kept of `tags`: well formed, each once, at most 8.
fn kept(tags: &[String]) -> Vec<String> {
    let mut kept = unique(tags);
    kept.truncate(MAX_LANGUAGES);
    kept
}

/// SetAgentBrowserLocale: both replaced, or refused in words the app shows.
pub(crate) async fn set(factory: &Factory, request: &v1::SetAgentBrowserLocale) -> Result<(), RequestError> {
    let zone = request.time_zone.trim().to_owned();
    if !zone.is_empty() && !zone_known(&zone) {
        return Err(refused(format!("Claktory doesn't know the time zone “{}”.", view::cleaned(&zone, 64))));
    }
    if let Some(bad) = request.languages.iter().map(|tag| tag.trim()).find(|tag| !wire::language_ok(tag)) {
        return Err(refused(format!(
            "“{}” isn't a language Claktory's browser can use: use tags such as en-GB or fr.",
            view::cleaned(bad, 40)
        )));
    }
    let languages = unique(&request.languages);
    if languages.len() > MAX_LANGUAGES {
        return Err(refused(format!("Choose at most {MAX_LANGUAGES} languages.")));
    }
    factory
        .edit_config(|config| {
            config.browser.time_zone = zone;
            config.browser.languages = languages;
            Ok(())
        })
        .await
}

/// What a device says of itself (Watch, Open): it fills what isn't set yet,
/// never what is, and what isn't well formed is left out. The setting as
/// it is with them (saved meanwhile), for a start now. Not with the
/// Factory's lock held.
pub(crate) fn offer(factory: &Arc<Factory>, zone: &str, languages: &[String]) -> BrowserConfig {
    let zone = Some(zone.trim().to_owned()).filter(|zone| zone_known(zone)).unwrap_or_default();
    let languages = kept(languages);
    let mut browser = factory.lock().config.browser.clone();
    let wanted = (browser.time_zone.is_empty() && !zone.is_empty()) || (browser.languages.is_empty() && !languages.is_empty());
    if !wanted {
        return browser;
    }
    if browser.time_zone.is_empty() {
        browser.time_zone = zone.clone();
    }
    if browser.languages.is_empty() {
        browser.languages = languages.clone();
    }
    let factory = factory.clone();
    tokio::spawn(async move {
        let filled = factory
            .edit_config(|config| {
                if config.browser.time_zone.is_empty() {
                    config.browser.time_zone = zone;
                }
                if config.browser.languages.is_empty() {
                    config.browser.languages = languages;
                }
                Ok(())
            })
            .await;
        if let Err(error) = filled {
            tracing::warn!(%error, "Claktory's browser's time zone and languages weren't saved");
        }
    });
    browser
}

/// What the next start gets: the setting's zone, or this computer's when
/// unset (left out when neither is one the runner takes: UTC); its
/// languages, or this computer's when unset (none either: en-US). Nobody
/// opens the browser from a device to fill them any more (#435, Matt's
/// card 222), so on a Mac it's the person's own; on a server, the
/// server's until Settings says otherwise (R11 Q9).
pub(crate) fn for_start(browser: &BrowserConfig) -> (Option<String>, Vec<String>) {
    let system = || jiff::tz::TimeZone::system().iana_name().map(str::to_owned);
    let zone = Some(browser.time_zone.clone()).filter(|zone| zone_known(zone)).or_else(system).filter(|zone| zone_known(zone));
    let languages = kept(&browser.languages);
    let languages = if languages.is_empty() { kept(&system_languages()) } else { languages };
    (zone, languages)
}

/// This computer's languages, most preferred first: on a Mac the person's
/// (AppleLanguages), else the POSIX locale's (LANGUAGE, then LC_ALL,
/// LC_MESSAGES, LANG). Empty when none says.
fn system_languages() -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        let asked = std::process::Command::new("/usr/bin/defaults").args(["read", "-g", "AppleLanguages"]).output();
        if let Ok(output) = asked
            && output.status.success()
        {
            let languages = apple_languages(&String::from_utf8_lossy(&output.stdout));
            if !languages.is_empty() {
                return languages;
            }
        }
    }
    let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    posix_languages(var("LANGUAGE").as_deref(), ["LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|name| var(name)).as_deref())
}

/// `defaults read -g AppleLanguages`: a property list array, one quoted
/// or bare tag a line ("(\n    \"en-GB\",\n    it\n)").
#[cfg_attr(windows, allow(dead_code, reason = "read on a Mac only; its test runs everywhere"))]
fn apple_languages(said: &str) -> Vec<String> {
    said.lines()
        .map(|line| line.trim().trim_end_matches(',').trim_matches('"'))
        .filter(|tag| !tag.is_empty() && *tag != "(" && *tag != ")")
        .map(str::to_owned)
        .collect()
}

/// LANGUAGE ("en_GB:en"), else the locale ("en_GB.UTF-8"), as BCP 47 tags;
/// C and POSIX say nothing.
fn posix_languages(language: Option<&str>, locale: Option<&str>) -> Vec<String> {
    let tag = |value: &str| {
        let bare = value.split(['.', '@']).next().unwrap_or_default().trim();
        (!bare.is_empty() && bare != "C" && bare != "POSIX").then(|| bare.replace('_', "-"))
    };
    let listed: Vec<String> = language.unwrap_or_default().split(':').filter_map(tag).collect();
    if listed.is_empty() { locale.and_then(tag).into_iter().collect() } else { listed }
}

/// What the Workspace shows of the setting: only what passes the checks
/// (an admin agent may have edited config.json to anything; Fugo 37975).
pub(crate) fn shown(browser: &BrowserConfig) -> (String, Vec<String>) {
    let zone = Some(browser.time_zone.clone()).filter(|zone| zone_known(zone)).unwrap_or_default();
    (zone, kept(&browser.languages))
}

/// ListTimeZones: the IANA names this Claktory knows, for a picker; not
/// the database's own files (posix/, right/, posixrules), and of the bare
/// names only UTC.
pub(crate) fn time_zones() -> v1::TimeZones {
    let mut names: Vec<String> = jiff::tz::db()
        .available()
        .map(|name| name.as_str().to_owned())
        .filter(|name| (name.contains('/') && !name.starts_with("posix/") && !name.starts_with("right/")) || name == "UTC")
        .filter(|name| wire::zone_name_ok(name))
        .collect();
    names.sort();
    names.dedup();
    v1::TimeZones { names }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_computers_languages_read_as_tags() {
        assert_eq!(apple_languages("(\n    \"en-GB\",\n    it,\n    \"zh-Hans-CN\"\n)\n"), ["en-GB", "it", "zh-Hans-CN"]);
        assert_eq!(posix_languages(Some("en_GB:en"), Some("de_DE.UTF-8")), ["en-GB", "en"]);
        assert_eq!(posix_languages(None, Some("de_DE.UTF-8@euro")), ["de-DE"]);
        assert!(posix_languages(Some(""), Some("C.UTF-8")).is_empty());
        assert!(posix_languages(None, Some("POSIX")).is_empty());
        // Unset, a start gets them; set, the setting wins.
        let set = BrowserConfig { languages: vec!["fr-FR".into()], ..Default::default() };
        assert_eq!(for_start(&set).1, ["fr-FR"]);
    }

    #[test]
    fn only_known_zones_and_well_formed_languages_are_kept() {
        assert!(zone_known("Europe/London"));
        assert!(zone_known("UTC"));
        for bad in ["Europe/Lndon", "/etc/passwd", ":/etc/localtime", "../zoneinfo/UTC", "Europe//London", "", "Europe/./London"] {
            assert!(!zone_known(bad), "{bad}");
        }
        let tags = ["en-GB", "en", "EN-gb", "x", "fr-CA-u-ca-gregory", "--lang=x", "de", "it", "es", "pt", "nl", "sv", "da"];
        let kept = kept(&tags.map(str::to_owned));
        assert_eq!(kept, ["en-GB", "en", "fr-CA-u-ca-gregory", "de", "it", "es", "pt", "nl"]);
        // What the app is shown of an edited config.json: only what passes.
        let edited =
            BrowserConfig { time_zone: "/etc/passwd".into(), languages: vec!["--lang=x".into(), "en".into()], ..Default::default() };
        assert_eq!(shown(&edited), (String::new(), vec!["en".to_owned()]));
    }

    #[test]
    fn a_start_gets_the_setting_or_this_computers_zone_and_languages() {
        let set = BrowserConfig { time_zone: "Asia/Tokyo".into(), languages: vec!["ja".into(), "en".into()], ..Default::default() };
        assert_eq!(for_start(&set), (Some("Asia/Tokyo".to_owned()), vec!["ja".to_owned(), "en".to_owned()]));
        let unset = for_start(&BrowserConfig::default());
        assert_eq!(unset.0, jiff::tz::TimeZone::system().iana_name().map(str::to_owned).filter(|zone| zone_known(zone)));
        assert_eq!(unset.1, kept(&system_languages()), "this computer's languages");
    }

    #[test]
    fn the_zones_listed_are_ones_a_start_takes() {
        let zones = time_zones().names;
        assert!(zones.iter().any(|zone| zone == "Europe/London"));
        assert!(zones.iter().all(|zone| zone_known(zone) && !zone.starts_with("posix/")));
    }
}
