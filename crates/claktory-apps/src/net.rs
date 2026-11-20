//! Which hosts an app may reach over HTTP: those its approved manifest names.

use std::sync::Arc;

use wasmtime_wasi_http::{Error, RequestOptions, WasiBody, WasiHttpHooks};

/// One of the hosts an app may reach, as `Load.hosts` gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rule {
    /// "api.telegram.org": HTTPS to that host, on port 443.
    Host(String),
    /// "*.example.com": HTTPS to any host under example.com, on port 443.
    Under(String),
    /// "http://127.0.0.1:8123": exactly that scheme, host and port (for
    /// tests; factoryd doesn't take it in a real manifest).
    Origin { https: bool, host: String, port: u16 },
}

impl Rule {
    /// The rule `text` writes, or None when it isn't one.
    pub fn parse(text: &str) -> Option<Rule> {
        let text = text.trim();
        if let Some((scheme, rest)) = text.split_once("://") {
            let https = match scheme.to_ascii_lowercase().as_str() {
                "https" => true,
                "http" => false,
                _ => return None,
            };
            let (host, port) = rest.trim_end_matches('/').rsplit_once(':')?;
            return Some(Rule::Origin { https, host: name(host)?, port: port.parse().ok()? });
        }
        if let Some(under) = text.strip_prefix("*.") {
            return Some(Rule::Under(name(under)?));
        }
        Some(Rule::Host(name(text)?))
    }

    fn allows(&self, https: bool, host: &str, port: u16) -> bool {
        match self {
            Rule::Host(name) => https && port == 443 && host == name,
            Rule::Under(name) => {
                https && port == 443 && host.strip_suffix(name.as_str()).is_some_and(|sub| sub.len() > 1 && sub.ends_with('.'))
            }
            Rule::Origin { https: scheme, host: name, port: allowed } => https == *scheme && port == *allowed && host == name,
        }
    }
}

/// A host name as rules compare it: lowercase, without a trailing dot.
fn name(host: &str) -> Option<String> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let fine = !host.is_empty() && !host.contains(['/', '@', '*', ' ', '?', '#']) && (host.starts_with('[') || !host.contains(':'));
    fine.then_some(host)
}

/// Whether `rules` let a request go to `uri`.
pub fn allowed(rules: &[Rule], uri: &http::Uri) -> bool {
    let https = match uri.scheme_str() {
        Some(scheme) if scheme.eq_ignore_ascii_case("https") => true,
        Some(scheme) if scheme.eq_ignore_ascii_case("http") => false,
        _ => return false,
    };
    let Some(host) = uri.host().and_then(name) else { return false };
    let port = uri.port_u16().unwrap_or(if https { 443 } else { 80 });
    rules.iter().any(|rule| rule.allows(https, &host, port))
}

/// wasi:http's hooks for one app: requests to hosts its rules don't allow
/// are denied before any connection.
pub struct Filter {
    pub rules: Arc<Vec<Rule>>,
}

impl WasiHttpHooks for Filter {
    fn send_request(
        &mut self,
        request: http::Request<WasiBody>,
        options: Option<RequestOptions>,
        _fut: Box<dyn Future<Output = Result<(), Error>> + Send>,
    ) -> Box<dyn Future<Output = Result<(http::Response<WasiBody>, Box<dyn Future<Output = Result<(), Error>> + Send>), Error>> + Send>
    {
        if !allowed(&self.rules, request.uri()) {
            tracing::warn!(uri = %request.uri(), "request denied: not a host the app may reach");
            return Box::new(async { Err(Error::HttpRequestDenied) });
        }
        Box::new(async move {
            use http_body_util::BodyExt;
            let (response, io) = wasmtime_wasi_http::default_send_request(request, options).await?;
            Ok((response.map(BodyExt::boxed_unsync), Box::new(io) as Box<dyn Future<Output = _> + Send>))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(texts: &[&str]) -> Vec<Rule> {
        texts.iter().map(|text| Rule::parse(text).expect(text)).collect()
    }

    fn lets(rules: &[Rule], uri: &str) -> bool {
        allowed(rules, &uri.parse().expect(uri))
    }

    #[test]
    fn a_named_host_is_reached_over_https_only() {
        let telegram = rules(&["api.telegram.org"]);
        assert!(lets(&telegram, "https://api.telegram.org/bot1/getUpdates"));
        assert!(lets(&telegram, "https://API.Telegram.org./x"));
        assert!(lets(&telegram, "https://api.telegram.org:443/x"));
        assert!(!lets(&telegram, "http://api.telegram.org/x"), "not in the clear");
        assert!(!lets(&telegram, "https://api.telegram.org:8443/x"));
        assert!(!lets(&telegram, "https://telegram.org/x"));
        assert!(!lets(&telegram, "https://api.telegram.org.evil.com/x"));
        assert!(!lets(&telegram, "https://evil.com/?api.telegram.org"));
        assert!(!lets(&telegram, "https://api.telegram.org@evil.com/x"));
    }

    #[test]
    fn a_wildcard_covers_names_under_it_and_an_origin_exactly_itself() {
        let some = rules(&["*.example.com", "http://127.0.0.1:8123"]);
        assert!(lets(&some, "https://a.example.com/"));
        assert!(lets(&some, "https://a.b.example.com/"));
        assert!(!lets(&some, "https://example.com/"));
        assert!(!lets(&some, "https://badexample.com/"));
        assert!(lets(&some, "http://127.0.0.1:8123/x"));
        assert!(!lets(&some, "http://127.0.0.1:8124/x"));
        assert!(!lets(&some, "https://127.0.0.1:8123/x"));
        assert!(!lets(&some, "http://localhost:8123/x"));
        assert!(!lets(&[], "https://a.example.com/"), "no hosts, no requests");
    }

    #[test]
    fn what_isnt_a_rule_is_refused() {
        for text in ["", "*", "*.", "ftp://x:1", "http://x", "a/b", "user@host", "http://x:port"] {
            assert_eq!(Rule::parse(text), None, "{text:?}");
        }
    }
}
