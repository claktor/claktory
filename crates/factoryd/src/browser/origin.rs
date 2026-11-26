//! Which sites a browser session may be for, and the one spelling of each
//! that both ends compare: browser-control.md §6. HTTPS only; never an IP
//! literal, this computer or a local name. This rule is about the name typed,
//! not where it resolves: HTTPS with valid certificates raises the bar, it
//! doesn't prove the site isn't on the local network.

/// Local names and suffixes a session is never for.
const LOCAL_NAMES: &[&str] = &["localhost"];
const LOCAL_SUFFIXES: &[&str] = &[".localhost", ".local", ".localdomain", ".internal", ".lan", ".home", ".home.arpa", ".corp", ".intranet"];

/// `input`'s origin, spelled as Chrome's `location.origin` does: lowercase,
/// the host in ASCII (punycode), the default port left out, no slash. Or
/// why it can't be a session's.
pub(crate) fn canonical(input: &str) -> Result<String, String> {
    let input = input.trim();
    let url = url::Url::parse(input).map_err(|_| format!("\"{input}\" isn't a web address. Give one like https://example.com."))?;
    if url.scheme() != "https" {
        return Err("Only https:// sites can be read.".to_owned());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Give the site without a user name or password in it.".to_owned());
    }
    if url.query().is_some() || url.fragment().is_some() || url.path() != "/" {
        return Err("Give only the site, such as https://example.com, without a path, query or #.".to_owned());
    }
    let Some(url::Host::Domain(host)) = url.host() else {
        return Err("A session can't be for an IP address: give the site's name.".to_owned());
    };
    let host = host.trim_end_matches('.');
    // Plain letter-digit-hyphen labels only (Fugo 41161): never a pattern
    // (`*`), an escape, or an empty label, which Chrome never names as a
    // page's origin, so they'd only seem to work.
    if !host.split('.').all(|label| !label.is_empty() && label.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')) {
        return Err(format!("{host} isn't a site's name: give one like https://example.com, exactly, never a pattern."));
    }
    if !host.contains('.') || LOCAL_NAMES.contains(&host) || LOCAL_SUFFIXES.iter().any(|suffix| host.ends_with(suffix)) {
        return Err(format!("{host} is a name on this computer or its network: a session is only for a site on the internet."));
    }
    let origin = url.origin().ascii_serialization();
    if origin == "null" {
        return Err(format!("\"{input}\" isn't a site Claktory can read."));
    }
    Ok(origin)
}

#[cfg(test)]
mod tests {
    use super::canonical;

    #[test]
    fn both_ends_get_the_same_spelling() {
        assert_eq!(canonical("https://Example.COM").unwrap(), "https://example.com");
        assert_eq!(canonical("https://example.com:443/").unwrap(), "https://example.com");
        assert_eq!(canonical("https://example.com:8443").unwrap(), "https://example.com:8443");
        assert_eq!(canonical("https://bücher.example").unwrap(), "https://xn--bcher-kva.example");
        // A Cyrillic а in "clаktory" is another site, spelled so.
        assert_eq!(canonical("https://clаktory.com").unwrap(), "https://xn--clktory-3fg.com");
        // An escape is the name it stands for, as Chrome reads it.
        assert_eq!(canonical("https://ex%41mple.com").unwrap(), "https://example.com");
    }

    #[test]
    fn only_https_sites_on_the_internet() {
        for refused in [
            "http://example.com",
            "https://user:pw@example.com",
            "https://example.com/inbox",
            "https://example.com/?q=1",
            "https://example.com/#top",
            "https://127.0.0.1",
            "https://[::1]",
            "https://2130706433",
            "https://10.0.0.1",
            "https://localhost",
            "https://printer.local",
            "https://wiki.corp",
            "https://intranet",
            "file:///etc/passwd",
            "chrome://settings",
            "javascript:alert(1)",
            "example.com",
            "https://*.example.com",
            "https://shop..example",
            "https://shop_1.example",
        ] {
            assert!(canonical(refused).is_err(), "{refused}");
        }
    }
}
