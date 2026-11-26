//! The errand's navigation fence (agent-browser.md §5.5): the agent goes
//! only to the sites its errand was opened for, fixed by its lead or the
//! person, never by the agent. Never file:, chrome:, data:, javascript: or
//! any other scheme than https, never an address on this computer or its
//! network, by name, by literal address, or by where the name resolves
//! (Fugo 36539). The fence is on where the agent goes, not on what a page
//! loads.

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

/// Where `address` may go for an errand on `sites` (canonical origins):
/// the address as Chrome should open it, or why not, in a line the agent
/// reads. Checks the name only; [`resolved`] checks where it points.
pub(crate) fn check(sites: &[String], address: &str) -> Result<url::Url, String> {
    let address = address.trim();
    let url = url::Url::parse(address).map_err(|_| format!("\"{}\" isn't a web address.", short(address)))?;
    if url.scheme() != "https" {
        return Err(format!("{}: addresses aren't for errands: only https:// pages of the errand's sites.", url.scheme()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("An address with a user name or password in it isn't for errands.".to_owned());
    }
    let site = url.origin().ascii_serialization();
    // The same spelling rules as the sites themselves: no IP literal, no
    // local name.
    origin::canonical(&site)?;
    if !sites.contains(&site) {
        return Err(format!(
            "{site} isn't one of this errand's sites ({}). Only its lead or the person can add one: ask them with errand_handoff, kind \"other\".",
            sites.join(", ")
        ));
    }
    Ok(url)
}

/// Whether where `url`'s host resolves is on the internet: every address it
/// resolves to, since Chrome may pick any. Err says why not.
pub(crate) async fn resolved(url: &url::Url) -> Result<(), String> {
    let Some(host) = url.host_str() else {
        return Err("That address has no site in it.".to_owned());
    };
    let port = url.port_or_known_default().unwrap_or(443);
    let addresses: Vec<IpAddr> = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::net::lookup_host((host, port)),
    )
    .await
    .map_err(|_| format!("{host} didn't resolve in time."))?
    .map_err(|_| format!("{host} doesn't resolve to an address."))?
    .map(|address| address.ip())
    .collect();
    if addresses.is_empty() {
        return Err(format!("{host} doesn't resolve to an address."));
    }
    match addresses.iter().find(|address| !public(**address)) {
        Some(address) => Err(format!(
            "{host} points to {address}, an address on this computer or its network: errands only go to sites on the internet."
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
/// only if it passes; otherwise it fails as blocked, and `blocked` gets its
/// site for the agent. Frames inside the page load as the page asks. Runs
/// until the page's connection ends.
///
/// While `hops` is on (the agent asked for a file, R9), a redirect may lead
/// off the sites to any https address on the internet, since files often
/// come from elsewhere (a CDN); what comes back from off the sites is let
/// through only if it's a file, never a page to show.
pub(crate) async fn guard(
    page: Arc<Page>,
    sites: Vec<String>,
    blocked: Arc<Mutex<Option<Blocked>>>,
    hops: Arc<AtomicBool>,
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
            let (sites, blocked) = (sites.clone(), blocked.clone());
            let params = &event.params;
            let paused = Paused {
                top: frame == top,
                answered: params.get("responseStatusCode").is_some() || params.get("responseErrorReason").is_some(),
                status: params["responseStatusCode"].as_u64().unwrap_or(0),
                file: is_file(&params["responseHeaders"]),
                redirect: params["redirectedRequestId"].as_str().is_some_and(|id| !id.is_empty()),
                hopping: hops.load(Ordering::SeqCst),
            };
            // Each on its own, so a slow lookup holds no other request.
            tokio::spawn(async move {
                let verdict = decide(&page, &sites, &url, paused).await;
                let (fine, signed_in) = (verdict.is_ok(), verdict.err().unwrap_or(false));
                if fine {
                    let _ = page.call("Fetch.continueRequest", json!({"requestId": request})).await;
                } else {
                    let site = url::Url::parse(&url).map(|url| url.origin().ascii_serialization()).unwrap_or_else(|_| short(&url));
                    *crate::locking::lock(&blocked) = Some(Blocked { site, signed_in });
                    let _ = page.call("Fetch.failRequest", json!({"requestId": request, "errorReason": "BlockedByClient"})).await;
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
}

/// Whether the fence lets `url` through: Ok, or Err(signed in) when it
/// stops it, true when it's for the person's cookies there.
pub(crate) async fn decide(page: &Page, sites: &[String], url: &str, paused: Paused) -> Result<(), bool> {
    let fine = if !paused.top {
        true
    } else if paused.answered {
        // Only a hop off the sites gets here off them: a redirect goes on
        // (its next address is checked), a file comes, a page doesn't.
        (300..400).contains(&paused.status) || check(sites, url).is_ok() || (paused.file && paused.hopping)
    } else if paused.hopping && paused.redirect && check(sites, url).is_err() {
        // Never with the person's cookies for a site the errand wasn't
        // opened for: what it fetched as them would leave with the agent
        // (Fugo 38000).
        if signed_in_at(page, url).await {
            return Err(true);
        }
        off_sites(url).await.is_ok()
    } else {
        allowed(sites, url).await.is_ok()
    };
    if fine { Ok(()) } else { Err(false) }
}

/// Where the fence stopped the page, to tell the agent once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Blocked {
    /// The site's origin.
    pub site: String,
    /// It was a redirect to a site the browser has cookies for.
    pub signed_in: bool,
}

impl Blocked {
    /// The line the agent reads.
    pub(crate) fn said(&self) -> String {
        let site = &self.site;
        if self.signed_in {
            format!(
                "Claktory kept the page from going to {site}: the file is on a site you're signed in to in Claktory's browser, so only the errand's own sites may give it. Ask your lead to add {site} to the errand's sites."
            )
        } else {
            format!("Claktory kept the page from going to {site}: it isn't one of this errand's sites.")
        }
    }
}

/// Whether Chrome would send cookies with a request to `address`: the
/// browser is signed in there, or was. Unknown counts as yes.
pub(crate) async fn signed_in_at(page: &Page, address: &str) -> bool {
    match page.call("Network.getCookies", json!({"urls": [address]})).await {
        Ok(cookies) => cookies["cookies"].as_array().is_none_or(|cookies| !cookies.is_empty()),
        Err(_) => true,
    }
}

/// Where a redirect may hop while the agent asks for a file: any https
/// address on the internet, with no user name or password in it.
async fn off_sites(address: &str) -> Result<(), String> {
    let url = url::Url::parse(address).map_err(|_| "not a web address".to_owned())?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err("not an https address".to_owned());
    }
    origin::canonical(&url.origin().ascii_serialization())?;
    resolved(&url).await
}

/// Whether response `headers` (CDP's [{name, value}]) make a file Chrome
/// downloads rather than a page it shows: an attachment, or a type it
/// doesn't show.
fn is_file(headers: &serde_json::Value) -> bool {
    let header = |name: &str| {
        headers
            .as_array()
            .into_iter()
            .flatten()
            .find(|header| header["name"].as_str().is_some_and(|key| key.eq_ignore_ascii_case(name)))
            .and_then(|header| header["value"].as_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default()
    };
    if header("content-disposition").trim_start().starts_with("attachment") {
        return true;
    }
    let kind = header("content-type");
    let kind = kind.split(';').next().unwrap_or_default().trim();
    let shown = ["xhtml+xml", "xml", "json", "pdf", "javascript", "ecmascript", "rss+xml", "atom+xml", "x-javascript"];
    kind.strip_prefix("application/").is_some_and(|kind| !shown.contains(&kind))
}

/// browser_download: Chrome fetches `address`, on the errand's sites, as
/// a download (R9); whether one began is the tracker's to say.
pub(crate) async fn fetch(page: &Page, sites: &[String], address: &str) -> Result<(), (String, String)> {
    let url = allowed(sites, address).await.map_err(|why| ("sensitive_action".to_owned(), why))?;
    let interrupted = |error: Error| ("interrupted".to_owned(), schema::one_line(&error.to_string(), 200));
    let went = page.call("Page.navigate", json!({"url": url.as_str()})).await.map_err(interrupted)?;
    page.forget();
    match went["errorText"].as_str().filter(|error| !error.is_empty()) {
        // A download ends the navigation it came from.
        None => Ok(()),
        Some(error) if error.contains("ERR_ABORTED") => Ok(()),
        Some(error) if error.contains("BLOCKED_BY_CLIENT") => {
            Err(("outcome_unknown".to_owned(), "it led to a page off this errand's sites, so Claktory stopped it".to_owned()))
        }
        Some(error) => Err(("outcome_unknown".to_owned(), format!("the file didn't come ({})", schema::one_line(error, 100)))),
    }
}

/// [`check`], then [`resolved`].
pub(crate) async fn allowed(sites: &[String], address: &str) -> Result<url::Url, String> {
    let url = check(sites, address)?;
    resolved(&url).await?;
    Ok(url)
}

/// How long a navigation waits for its page to load.
const LOAD: Duration = Duration::from_secs(30);

/// Takes the page to `address`, if the fence lets it, and waits for it to
/// load: where it is then, and its document, for the agent to read next.
pub(crate) async fn go(page: &Page, sites: &[String], address: &str) -> Result<Answer, (String, String)> {
    let url = allowed(sites, address).await.map_err(|why| ("sensitive_action".to_owned(), why))?;
    let mut events = page.cdp().events();
    let interrupted = |error: Error| ("interrupted".to_owned(), schema::one_line(&error.to_string(), 200));
    page.call("Page.enable", json!({})).await.map_err(interrupted)?;
    let went = page.call("Page.navigate", json!({"url": url.as_str()})).await.map_err(interrupted)?;
    page.forget();
    if let Some(error) = went["errorText"].as_str().filter(|error| !error.is_empty()) {
        let why = if error.contains("BLOCKED_BY_CLIENT") {
            "it went to a site this errand isn't for, so Claktory stopped it".to_owned()
        } else {
            format!("the page didn't load ({})", schema::one_line(error, 100))
        };
        return Err(("outcome_unknown".to_owned(), why));
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

    fn sites() -> Vec<String> {
        vec!["https://play.google.com".to_owned(), "https://accounts.google.com".to_owned()]
    }

    #[test]
    fn only_https_pages_of_the_errands_sites() {
        let fine = check(&sites(), "https://play.google.com/console/signup?x=1#top").unwrap();
        assert_eq!(fine.path(), "/console/signup");
        assert!(check(&sites(), "https://PLAY.google.com:443/").is_ok());
        for refused in [
            "https://evil.example/",
            "https://play.google.com.evil.example/",
            "https://play.google.com:8443/",
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
            assert!(check(&sites(), refused).is_err(), "{refused}");
        }
        let said = check(&sites(), "https://evil.example/").unwrap_err();
        assert!(said.contains("https://play.google.com, https://accounts.google.com"), "{said}");
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

    /// While the agent asks for a file, a redirect may hop off the sites,
    /// never to one the browser has cookies for (Fugo 38000), and what
    /// comes back from off them passes only as a file.
    #[tokio::test]
    async fn a_hop_for_a_file_never_goes_where_the_browser_is_signed_in() {
        let Some((_local, page, _)) = crate::errands::reader::tests::page_with("<p>Assets</p>").await else { return };
        let cookie = json!({"name": "SID", "value": "1", "domain": "drive.example", "path": "/", "secure": true});
        page.call("Network.setCookie", cookie).await.unwrap();
        let hop = Paused { top: true, redirect: true, hopping: true, ..Paused::default() };
        assert_eq!(decide(&page, &sites(), "https://drive.example/uc?id=1", hop).await, Err(true));
        // Not signed in there: weighed as any hop (this one doesn't resolve).
        assert_eq!(decide(&page, &sites(), "https://cdn.example/f.zip", hop).await, Err(false));
        // Not asking for a file: a page like any other.
        assert_eq!(decide(&page, &sites(), "https://drive.example/uc?id=1", Paused { hopping: false, ..hop }).await, Err(false));
        // The response, off the sites: a file, only while asked; a redirect goes on.
        let file = Paused { top: true, answered: true, status: 200, file: true, hopping: true, ..Paused::default() };
        assert_eq!(decide(&page, &sites(), "https://cdn.example/f.zip", file).await, Ok(()));
        assert_eq!(decide(&page, &sites(), "https://cdn.example/f.html", Paused { file: false, ..file }).await, Err(false));
        assert_eq!(decide(&page, &sites(), "https://cdn.example/f.zip", Paused { hopping: false, ..file }).await, Err(false));
        assert_eq!(decide(&page, &sites(), "https://cdn.example/next", Paused { status: 302, file: false, ..file }).await, Ok(()));
        // Frames inside the page load as it asks.
        assert_eq!(decide(&page, &sites(), "https://drive.example/", Paused::default()).await, Ok(()));
    }

    #[tokio::test]
    async fn a_page_that_leads_off_the_errands_sites_is_stopped_whoever_moves_it() {
        let html = r#"<a href="https://evil.example/steal">Continue</a>"#;
        let Some((_local, page, profile)) = crate::errands::reader::tests::page_with(html).await else { return };
        let blocked = Arc::new(Mutex::new(None));
        guard(page.clone(), sites(), blocked.clone(), Arc::new(AtomicBool::new(false))).await.unwrap();
        let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
        let link = read.elements.iter().find(|element| element.name == "Continue").unwrap().reference.clone();
        // The reader says the link leaves; the browser's own click is stopped all the same.
        let _ = page.act(&Op::on_element(&read.document, 0, &link, None).unwrap()).await;
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
        assert!(stopped("https://evil.example").await, "{:?}", crate::locking::lock(&blocked));
        // The page's own script, the same.
        let went = page.call("Runtime.evaluate", json!({"expression": "location.href = 'https://other.example/x'"})).await.unwrap();
        assert!(went.get("exceptionDetails").is_none(), "{went}");
        assert!(stopped("https://other.example").await, "{:?}", crate::locking::lock(&blocked));
        let frame = page.frame().await.unwrap();
        assert!(!frame.url.contains("evil") && !frame.url.contains("other"), "{}", frame.url);
        // And the agent's own navigation is refused before it starts.
        let refused = go(&page, &sites(), "https://evil.example/").await.unwrap_err();
        assert!(refused.1.contains("isn't one of this errand's sites"), "{refused:?}");
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
