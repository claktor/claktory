//! The browser's navigation fence: an agent's browser goes to any site on
//! the internet over https, never file:, chrome:, data:, javascript: or any other scheme,
//! never an address on this computer or its network, by name, by literal
//! address, or by where the name resolves. The fence is on
//! where the agent goes, not on what a page loads; the runner's network
//! rules keep the rest of the page off this computer and its network.

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use std::sync::Mutex;
use serde_json::json;
use tokio::sync::broadcast;

use super::cdp::Error;
use super::reader::Page;
use crate::browser::origin;
use crate::browser::schema::{self, Answer, Op};

/// Where `address` may go: the address as Chrome should open it, or why
/// not, in a line the agent reads. Checks the name only; [`resolved`]
/// checks where it points.
pub(crate) fn check(address: &str) -> Result<url::Url, String> {
    let address = address.trim();
    let url = url::Url::parse(address).map_err(|_| format!("\"{}\" isn't a web address.", short(address)))?;
    if url.scheme() != "https" {
        return Err(format!("{}: addresses aren't for Claktory's browser: only https:// pages of sites on the internet.", url.scheme()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("An address with a user name or password in it isn't for Claktory's browser.".to_owned());
    }
    // No IP literal, no local name.
    origin::canonical(&url.origin().ascii_serialization())?;
    if password_manager(&url) {
        return Err(PASSWORD_MANAGERS.to_owned());
    }
    Ok(url)
}

/// Why a password manager's pages are refused.
pub(crate) const PASSWORD_MANAGERS: &str = "Claktory's browser doesn't open password managers.";

/// Whether `url` is a web password manager's vault:
/// a signed-in session there would show the person's saved passwords as
/// page text. A fixed list, so depth, not completeness.
pub(crate) fn password_manager(url: &url::Url) -> bool {
    let host = url.host_str().unwrap_or_default().trim_end_matches('.').to_ascii_lowercase();
    // LastPass and Keeper serve their vaults under paths a prefix can't
    // hold (/%76ault, /Vault, /vault;x, ?ac=1): their whole sites (Fugo
    // 54568).
    let whole = ["lastpass.com", "lastpass.eu", "keepersecurity.com", "keepersecurity.eu"];
    matches!(
        host.as_str(),
        "passwords.google.com" | "vault.bitwarden.com" | "vault.bitwarden.eu" | "app.dashlane.com" | "pass.proton.me" | "app.nordpass.com"
    ) || [".1password.com", ".1password.ca", ".1password.eu"].iter().any(|domain| host.ends_with(domain))
        || whole.iter().any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
}

/// Whether where `url`'s host resolves is on the internet: every address it
/// resolves to, since Chrome may pick any. Err says why not.
pub(crate) async fn resolved(url: &url::Url) -> Result<(), String> {
    let Some(host) = url.host_str() else {
        return Err("That address has no site in it.".to_owned());
    };
    let port = url.port_or_known_default().unwrap_or(443);
    let addresses: Vec<IpAddr> = if let Some(address) = resolves_for_test(host) {
        vec![address]
    } else {
        tokio::time::timeout(std::time::Duration::from_secs(10), tokio::net::lookup_host((host, port)))
            .await
            .map_err(|_| format!("{host} didn't resolve in time."))?
            .map_err(|_| format!("{host} doesn't resolve to an address."))?
            .map(|address| address.ip())
            .collect()
    };
    if addresses.is_empty() {
        return Err(format!("{host} doesn't resolve to an address."));
    }
    match addresses.iter().find(|address| !public(**address)) {
        Some(address) => Err(format!(
            "{host} points to {address}, an address on this computer or its network: Claktory's browser only goes to sites on the internet."
        )),
        None => Ok(()),
    }
}

/// Whether `address` is on the internet: not loopback, private, link-local,
/// shared (CGNAT), unspecified, multicast, broadcast, documentation or
/// reserved, in v4 or v6 (v4-mapped and NAT64 addresses read as their v4).
pub(crate) fn public(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_broadcast()
                || v4.is_documentation()
                || a == 0
                || (a == 100 && (64..128).contains(&b))
                || (a == 192 && b == 0 && c == 0)
                || (a == 198 && (18..20).contains(&b))
                || a >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return public(IpAddr::V4(v4));
            }
            let segments = v6.segments();
            // NAT64's well-known prefix, 64:ff9b::/96: the v4 inside.
            if segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                let [_, _, _, _, _, _, high, low] = segments;
                let v4 = std::net::Ipv4Addr::from((u32::from(high) << 16) | u32::from(low));
                return public(IpAddr::V4(v4));
            }
            // 6to4, 2002::/16: the v4 in its next 32 bits (Fugo 37189).
            if segments[0] == 0x2002 {
                let v4 = std::net::Ipv4Addr::from((u32::from(segments[1]) << 16) | u32::from(segments[2]));
                return public(IpAddr::V4(v4));
            }
            let first = segments[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
                || (first & 0xffc0) == 0xfec0
                || (first == 0x2001 && segments[1] == 0x0db8)
                || (first == 0x64 && segments[1] == 0xff9b && segments[2] == 1)
                || segments[..6] == [0, 0, 0, 0, 0, 0])
        }
    }
}

/// Holds the page to the fence whoever moves it (a link, a form, a
/// redirect, the page's own script): each document the top frame is about
/// to load is paused, checked as [`check`] and [`resolved`] do, and loaded
/// only if it passes; otherwise it's aborted, the tab staying where it
/// was, and `blocked` gets its site for the agent. Frames inside the page
/// load as the page asks. Runs until the page's connection ends.
///
/// While `hops` is on (the agent asked for a file, R9), what the top frame
/// gets back may be a file. While `fetching` is on (browser_download
/// itself), a PDF counts as a file and comes as one, saved rather than
/// shown in Chrome's viewer.
///
/// `held`, a site of a check handed to the person: the page never goes
/// there meanwhile, so the check keeps its site to itself.
pub(crate) async fn guard(
    page: Arc<Page>,
    blocked: Arc<Mutex<Option<Blocked>>>,
    hops: Arc<AtomicBool>,
    fetching: Arc<AtomicBool>,
    held: Option<String>,
) -> Result<(), Error> {
    let mut events = page.cdp().events();
    // The top frame keeps its id for the page's life: asking for it while
    // a navigation is paused would wait on that navigation.
    let top = page.frame().await?.id;
    page.call(
        "Fetch.enable",
        json!({"patterns": [
            {"urlPattern": "*", "resourceType": "Document", "requestStage": "Request"},
            {"urlPattern": "*", "resourceType": "Document", "requestStage": "Response"},
        ]}),
    )
    .await?;
    tokio::spawn(async move {
        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            };
            if event.method != "Fetch.requestPaused" || event.session.as_deref() != Some(page.session()) {
                continue;
            }
            let Some(request) = event.params["requestId"].as_str().filter(|id| id.len() <= 128) else { continue };
            let request = request.to_owned();
            let url = event.params["request"]["url"].as_str().unwrap_or_default().to_owned();
            let frame = event.params["frameId"].as_str().unwrap_or_default().to_owned();
            let page = page.clone();
            let blocked = blocked.clone();
            let held = held.clone().filter(|site| frame == top && crate::browser_host::tab_view::site_of(&url) == *site);
            let params = event.params.clone();
            let paused = Paused {
                top: frame == top,
                answered: params.get("responseStatusCode").is_some() || params.get("responseErrorReason").is_some(),
                status: params["responseStatusCode"].as_u64().unwrap_or(0),
                file: is_file(&params["responseHeaders"]),
                pdf: is_pdf(&params["responseHeaders"]),
                redirect: params["redirectedRequestId"].as_str().is_some_and(|id| !id.is_empty()),
                hopping: hops.load(Ordering::SeqCst),
                fetching: fetching.load(Ordering::SeqCst),
            };
            // Each on its own, so a slow lookup holds no other request.
            tokio::spawn(async move {
                let allowed = held.is_none() && decide(&url, paused).await;
                // A form sent with a body (a file, say): where it went, never what.
                if paused.top && !paused.answered && params["request"]["hasPostData"] == true {
                    let entries = params["request"]["postDataEntries"].as_array().map_or(0, Vec::len);
                    let origin = url::Url::parse(&url).map(|url| url.origin().ascii_serialization()).unwrap_or_default();
                    let method = params["request"]["method"].as_str().unwrap_or_default();
                    tracing::info!(%origin, %method, entries, allowed, "the fence weighed a form the page sent");
                }
                if allowed {
                    if !paused.answered && served(&page, &request, &url).await {
                        return;
                    }
                    let_through(&page, &request, &params, paused.saves_pdf()).await;
                } else {
                    let parsed = url::Url::parse(&url);
                    let why = match held {
                        Some(site) => Kept::Held(site),
                        None if parsed.as_ref().is_ok_and(password_manager) => Kept::Manager,
                        None => Kept::Fenced,
                    };
                    let site = parsed.map(|url| url.origin().ascii_serialization()).unwrap_or_else(|_| short(&url));
                    *crate::locking::lock(&blocked) = Some(Blocked { site, why });
                    // Aborted, as a navigation the person stopped: the tab
                    // stays on its page rather than Chrome's error page.
                    let _ = page.call("Fetch.failRequest", json!({"requestId": request, "errorReason": "Aborted"})).await;
                }
            });
        }
    });
    Ok(())
}

/// A paused request, as the fence weighs it.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Paused {
    /// The top frame's (anything else loads as the page asks).
    pub top: bool,
    /// At the response stage.
    pub answered: bool,
    pub status: u64,
    /// The response is a file, not a page to show.
    pub file: bool,
    /// A redirect's next hop.
    pub redirect: bool,
    /// The agent asked for a file just now.
    pub hopping: bool,
    /// The response is a PDF.
    pub pdf: bool,
    /// The agent asked for this file itself (browser_download).
    pub fetching: bool,
}

impl Paused {
    /// The page's own response is a PDF the agent asked to download: it
    /// comes as a file, never into Chrome's viewer.
    fn saves_pdf(self) -> bool {
        self.top && self.answered && self.fetching && self.pdf && (200..300).contains(&self.status)
    }
}

/// Lets a paused request or response go on: as an attachment when
/// `as_file` (Chrome then saves it, as any download), else as it is.
pub(crate) async fn let_through(page: &Page, request: &str, params: &serde_json::Value, as_file: bool) {
    if as_file {
        // Chrome takes new headers only with the status they go with.
        let headers = attached(&params["responseHeaders"]);
        let given = json!({"requestId": request, "responseCode": params["responseStatusCode"], "responseHeaders": headers});
        if page.call("Fetch.continueResponse", given).await.is_ok() {
            return;
        }
    }
    if let Err(error) = page.call("Fetch.continueRequest", json!({"requestId": request})).await {
        tracing::warn!(%error, "Chrome didn't let a request the fence allowed go on");
    }
}

/// Response `headers` (CDP's [{name, value}]) with the file made an
/// attachment: any Content-Disposition of its own replaced.
fn attached(headers: &serde_json::Value) -> Vec<serde_json::Value> {
    let mut kept: Vec<serde_json::Value> = headers
        .as_array()
        .into_iter()
        .flatten()
        .filter(|header| !header["name"].as_str().is_some_and(|name| name.eq_ignore_ascii_case("content-disposition")))
        .take(200)
        .cloned()
        .collect();
    kept.push(json!({"name": "Content-Disposition", "value": "attachment"}));
    kept
}

/// Tests: what a page at an address shows instead of the network's (tests
/// have none), and where a host resolves. Nothing else fills them.
static SERVED: Mutex<Vec<(String, Option<String>)>> = Mutex::new(Vec::new());
static RESOLVES: Mutex<Vec<(String, IpAddr)>> = Mutex::new(Vec::new());

/// Tests: the browser shows `html` at `address`, or Chrome's error page
/// with None (blocked, as when Chrome warns a site is unsafe);
/// its host resolves to an address on the internet.
#[doc(hidden)]
pub fn serve_for_test(address: &str, html: Option<&str>) {
    let url = url::Url::parse(address).expect("an address");
    let host = url.host_str().expect("a host").to_owned();
    resolve_for_test(&host, IpAddr::from([93, 184, 215, 14]));
    let mut served = crate::locking::lock(&SERVED);
    served.retain(|(served, _)| served != url.as_str());
    served.push((url.to_string(), html.map(str::to_owned)));
}

/// Tests: `host` resolves to `address`.
#[doc(hidden)]
pub fn resolve_for_test(host: &str, address: IpAddr) {
    let host = host.to_ascii_lowercase();
    let mut resolves = crate::locking::lock(&RESOLVES);
    resolves.retain(|(name, _)| *name != host);
    resolves.push((host, address));
}

fn resolves_for_test(host: &str) -> Option<IpAddr> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    crate::locking::lock(&RESOLVES).iter().find(|(name, _)| *name == host).map(|(_, address)| *address)
}

/// Answers a paused request a test serves (`serve_for_test`): whether it
/// was one.
async fn served(page: &Page, request: &str, url: &str) -> bool {
    let served = crate::locking::lock(&SERVED).iter().find(|(served, _)| served == url).map(|(_, html)| html.clone());
    match served {
        None => false,
        Some(None) => {
            let _ = page.call("Fetch.failRequest", json!({"requestId": request, "errorReason": "BlockedByClient"})).await;
            true
        }
        Some(Some(html)) => {
            let body = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, html.as_bytes());
            let headers = [json!({"name": "Content-Type", "value": "text/html; charset=utf-8"})];
            let fulfilled = json!({"requestId": request, "responseCode": 200, "responseHeaders": headers, "body": body});
            let _ = page.call("Fetch.fulfillRequest", fulfilled).await;
            true
        }
    }
}

/// Whether the fence lets `url` through.
pub(crate) async fn decide(url: &str, paused: Paused) -> bool {
    if !paused.top {
        true
    } else if paused.answered {
        // A redirect goes on (its next address is checked), a page comes
        // from a site the fence lets the browser go to, and a file the
        // agent asked for comes as it does.
        (300..400).contains(&paused.status) || check(url).is_ok() || ((paused.file || (paused.pdf && paused.fetching)) && paused.hopping)
    } else {
        allowed(url).await.is_ok()
    }
}

/// Where the fence stopped the page, to tell the agent once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Blocked {
    /// The site's origin.
    pub site: String,
    pub why: Kept,
}

/// Why the fence kept the page from a site.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Kept {
    /// Not a site on the internet over https.
    Fenced,
    /// A web password manager's: a site on the internet all the same, so
    /// the line says why that one isn't.
    Manager,
    /// The site of a check handed to the person, held for them.
    Held(String),
}

impl Blocked {
    /// The line the agent reads.
    pub(crate) fn said(&self) -> String {
        match &self.why {
            Kept::Manager => format!("Claktory kept the page from going to {}: {PASSWORD_MANAGERS}", self.site),
            Kept::Held(site) => format!(
                "Claktory kept the page from going to {}: a check on {site} waits for the person in its own tab, so go there once they answer.",
                self.site
            ),
            Kept::Fenced => format!("Claktory kept the page from going to {}: it isn't a site on the internet over https.", self.site),
        }
    }
}

/// A response header's value, lowercase; empty when it has none.
fn header(headers: &serde_json::Value, name: &str) -> String {
    headers
        .as_array()
        .into_iter()
        .flatten()
        .find(|header| header["name"].as_str().is_some_and(|key| key.eq_ignore_ascii_case(name)))
        .and_then(|header| header["value"].as_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

/// The response's type, without its parameters.
fn kind_of(headers: &serde_json::Value) -> String {
    header(headers, "content-type").split(';').next().unwrap_or_default().trim().to_owned()
}

/// Whether response `headers` (CDP's [{name, value}]) make a file Chrome
/// downloads rather than a page it shows: an attachment, or a type it
/// doesn't show.
fn is_file(headers: &serde_json::Value) -> bool {
    if header(headers, "content-disposition").trim_start().starts_with("attachment") {
        return true;
    }
    let shown = ["xhtml+xml", "xml", "json", "pdf", "javascript", "ecmascript", "rss+xml", "atom+xml", "x-javascript"];
    kind_of(headers).strip_prefix("application/").is_some_and(|kind| !shown.contains(&kind))
}

/// Whether the response is a PDF, which Chrome shows in its viewer.
fn is_pdf(headers: &serde_json::Value) -> bool {
    matches!(kind_of(headers).as_str(), "application/pdf" | "application/x-pdf")
}

/// browser_download: Chrome fetches `address` as a download (R9);
/// whether one began is the tracker's to say.
pub(crate) async fn fetch(page: &Page, address: &str) -> Result<(), (String, String)> {
    let url = allowed(address).await.map_err(|why| ("sensitive_action".to_owned(), why))?;
    let interrupted = |error: Error| ("interrupted".to_owned(), schema::one_line(&error.to_string(), 200));
    let went = page.call("Page.navigate", json!({"url": url.as_str()})).await.map_err(interrupted)?;
    page.forget();
    match went["errorText"].as_str().filter(|error| !error.is_empty()) {
        // A download ends the navigation it came from.
        None => Ok(()),
        Some(error) if error.contains("ERR_ABORTED") => Ok(()),
        Some(error) if error.contains("BLOCKED_BY_CLIENT") => {
            Err((BLOCKED.to_owned(), "it led to an address Claktory's browser doesn't go to, so Claktory stopped it".to_owned()))
        }
        Some(error) => Err(("outcome_unknown".to_owned(), format!("the file didn't come ({})", schema::one_line(error, 100)))),
    }
}

/// What `go` and `fetch` answer when Chrome blocked the page: the fence,
/// whose words they give, or Safe Browsing, which the step tells apart
/// by whether the fence stopped anything.
pub(crate) const BLOCKED: &str = "blocked";

/// What `go` answers when the navigation was aborted: by the fence, which
/// aborts what it stops so the tab stays on its page, or by something else
/// (a download, another navigation); the step tells them apart.
pub(crate) const ABORTED: &str = "aborted";

/// Why `go` didn't get there, when the fence stopped it.
pub(crate) const WENT_FENCED: &str = "it went to an address Claktory's browser doesn't go to, so Claktory stopped it";

/// Why a page Safe Browsing flags doesn't open.
pub(crate) const DANGEROUS: &str =
    "Chrome warns that this site is dangerous, so Claktory's browser won't open it. Don't look for another way to it.";

/// [`check`], then [`resolved`].
pub(crate) async fn allowed(address: &str) -> Result<url::Url, String> {
    let url = check(address)?;
    resolved(&url).await?;
    Ok(url)
}

/// How long a navigation waits for its page to load.
const LOAD: Duration = Duration::from_secs(30);

/// Takes the page to `address`, if the fence lets it, and waits for it to
/// load: where it is then, and its document, for the agent to read next.
pub(crate) async fn go(page: &Page, address: &str) -> Result<Answer, (String, String)> {
    let url = allowed(address).await.map_err(|why| ("sensitive_action".to_owned(), why))?;
    let mut events = page.cdp().events();
    let interrupted = |error: Error| ("interrupted".to_owned(), schema::one_line(&error.to_string(), 200));
    page.call("Page.enable", json!({})).await.map_err(interrupted)?;
    let went = page.call("Page.navigate", json!({"url": url.as_str()})).await.map_err(interrupted)?;
    page.forget();
    if let Some(error) = went["errorText"].as_str().filter(|error| !error.is_empty()) {
        if error.contains("BLOCKED_BY_CLIENT") {
            return Err((BLOCKED.to_owned(), WENT_FENCED.to_owned()));
        }
        if error.contains("ERR_ABORTED") {
            return Err((ABORTED.to_owned(), format!("the page didn't load ({})", schema::one_line(error, 100))));
        }
        return Err(("outcome_unknown".to_owned(), format!("the page didn't load ({})", schema::one_line(error, 100))));
    }
    // Its load, or the time it had for it: the page is read as it is then.
    let session = page.session().to_owned();
    let _ = tokio::time::timeout(LOAD, async {
        loop {
            match events.recv().await {
                Ok(event) if event.method == "Page.loadEventFired" && event.session.as_deref() == Some(&session) => return,
                Err(broadcast::error::RecvError::Closed) => return,
                _ => {}
            }
        }
    })
    .await;
    let frame = page.frame().await.map_err(interrupted)?;
    let document = page.document().await.map_err(interrupted)?;
    let path = url::Url::parse(&frame.url).map(|url| path_of(&url)).unwrap_or_else(|_| "/".to_owned());
    let json = json!({"done": true, "path": path, "document": document}).to_string();
    let op = Op::Navigate { url: url.to_string() };
    schema::answer(&op, &json).map_err(|_| ("unsupported_page".to_owned(), "the page's address isn't one Claktory reads".to_owned()))
}

/// A page's path for the agent: never its query or fragment, bounded.
pub(crate) fn path_of(url: &url::Url) -> String {
    let path: String = url.path().chars().filter(|c| !c.is_control() && !matches!(c, '?' | '#')).take(300).collect();
    if path.starts_with('/') { path } else { format!("/{path}") }
}

/// An address cut for an error line.
fn short(address: &str) -> String {
    let mut cut: String = address.chars().filter(|c| !c.is_control()).take(200).collect();
    if cut.len() < address.len() {
        cut.push('…');
    }
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A password manager's vault is never a page the browser
    /// opens, wherever the agent or a page sends it.
    #[test]
    fn password_managers_are_never_opened() {
        for refused in [
            "https://passwords.google.com/",
            "https://my.1password.com/signin",
            "https://team.1password.eu/",
            "https://vault.bitwarden.com/#/vault",
            "https://lastpass.com/vault/",
            "https://www.lastpass.com/vault",
            "https://lastpass.com/%76ault",
            "https://lastpass.com/pricing",
            "https://lastpass.com/?ac=1",
            "https://keepersecurity.eu/vault/#",
            "https://app.dashlane.com/",
            "https://pass.proton.me/u/0",
            "https://app.nordpass.com/",
            "https://PASSWORDS.google.com./",
        ] {
            assert_eq!(check(refused).unwrap_err(), PASSWORD_MANAGERS, "{refused}");
        }
        for allowed in ["https://www.google.com/", "https://bitwarden.com/", "https://proton.me/pass", "https://1password.com/"] {
            assert!(check(allowed).is_ok(), "{allowed}");
        }
    }

    #[test]
    fn only_https_pages_of_sites_on_the_internet() {
        let fine = check("https://play.google.com/console/signup?x=1#top").unwrap();
        assert_eq!(fine.path(), "/console/signup");
        assert!(check("https://PLAY.google.com:443/").is_ok());
        assert!(check("https://evil.example/").is_ok(), "any site: no list of them");
        for refused in [
            "http://play.google.com/",
            "https://user@play.google.com/",
            "file:///etc/passwd",
            "chrome://settings",
            "chrome-extension://abc/page.html",
            "data:text/html,hi",
            "javascript:alert(1)",
            "about:blank",
            "blob:https://play.google.com/1",
            "view-source:https://play.google.com/",
            "https://127.0.0.1/",
            "https://[::1]/",
            "https://localhost/",
            "play.google.com",
        ] {
            assert!(check(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn addresses_on_this_computer_or_its_network_arent_public() {
        for local in [
            "127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.169.254", "100.64.0.1", "0.0.0.0",
            "224.0.0.1", "255.255.255.255", "192.0.0.1", "198.18.0.1", "240.0.0.1", "::1", "::", "fc00::1",
            "fd12::1", "fe80::1", "::ffff:127.0.0.1", "::ffff:10.0.0.1", "64:ff9b::a00:1", "64:ff9b::7f00:1",
            "2001:db8::1", "::127.0.0.1", "ff02::1", "2002:a00:1::1", "2002:7f00:1::", "2002:a9fe:a9fe::1",
        ] {
            assert!(!public(local.parse().unwrap()), "{local}");
        }
        for internet in ["142.250.1.1", "8.8.8.8", "2a00:1450:4001::1", "::ffff:8.8.8.8", "64:ff9b::808:808", "2002:808:808::1"] {
            assert!(public(internet.parse().unwrap()), "{internet}");
        }
    }

    /// The top frame's responses: a redirect goes on, a page comes from a
    /// site the browser may go to, and a file or a PDF the agent asked for
    /// comes as one.
    #[tokio::test]
    async fn what_comes_back_to_the_top_frame() {
        let file = Paused { top: true, answered: true, status: 200, file: true, hopping: true, ..Paused::default() };
        assert!(decide("https://cdn.example/f.zip", file).await);
        assert!(decide("https://cdn.example/f.html", Paused { file: false, ..file }).await);
        assert!(decide("https://cdn.example/next", Paused { status: 302, file: false, ..file }).await);
        assert!(!decide("http://cdn.example/f.html", Paused { file: false, ..file }).await, "never a page over http");
        assert!(decide("http://cdn.example/f.zip", file).await, "a file asked for, wherever it ends");
        assert!(!decide("http://cdn.example/f.zip", Paused { hopping: false, ..file }).await);
        let pdf = Paused { file: false, pdf: true, ..file };
        assert!(!decide("http://cdn.example/f.pdf", pdf).await);
        assert!(decide("http://cdn.example/f.pdf", Paused { fetching: true, ..pdf }).await);
        assert!(Paused { fetching: true, ..pdf }.saves_pdf() && !pdf.saves_pdf());
        // Frames inside the page load as it asks.
        assert!(decide("http://drive.example/", Paused::default()).await);
    }

    #[tokio::test]
    async fn a_page_that_leads_to_this_computer_is_stopped_whoever_moves_it() {
        let html = r#"<a href="https://localhost/steal">Continue</a>"#;
        let Some((_local, page, profile)) = crate::errands::reader::tests::page_with(html).await else { return };
        let blocked = Arc::new(Mutex::new(None));
        guard(page.clone(), blocked.clone(), Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)), None).await.unwrap();
        let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
        let link = read.elements.iter().find(|element| element.name == "Continue").unwrap().reference.clone();
        // The reader says the link leaves; the browser's own click is stopped all the same.
        let mut hands = crate::errands::reader::tests::TestHand::new();
        let _ = page.act(&mut hands.hand(), &Op::on_element(&read.document, 0, &link, None).unwrap()).await;
        let stopped = |site: &'static str| {
            let blocked = blocked.clone();
            async move {
                for _ in 0..50 {
                    if crate::locking::lock(&blocked).as_ref().is_some_and(|blocked| blocked.site == site) {
                        return true;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                false
            }
        };
        assert!(stopped("https://localhost").await, "{:?}", crate::locking::lock(&blocked));
        // The page's own script, the same.
        let went = page.call("Runtime.evaluate", json!({"expression": "location.href = 'http://other.example/x'"})).await.unwrap();
        assert!(went.get("exceptionDetails").is_none(), "{went}");
        assert!(stopped("http://other.example").await, "{:?}", crate::locking::lock(&blocked));
        let frame = page.frame().await.unwrap();
        assert!(!frame.url.contains("localhost") && !frame.url.contains("other"), "{}", frame.url);
        // And the agent's own navigation is refused before it starts.
        let refused = go(&page, "https://10.0.0.1/").await.unwrap_err();
        assert_eq!(refused.0, "sensitive_action", "{refused:?}");
        let _ = std::fs::remove_dir_all(&profile);
    }

    #[tokio::test]
    async fn a_name_that_points_here_is_refused() {
        // localhost.localdomain-style names are refused by spelling; a name
        // that resolves here is caught by where it points.
        let url = url::Url::parse("https://localhost/").unwrap();
        let said = resolved(&url).await.unwrap_err();
        assert!(said.contains("on this computer or its network"), "{said}");
    }
}
