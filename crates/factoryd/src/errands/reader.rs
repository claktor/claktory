//! The errand's reader (agent-browser.md §3.1, §5.4): Claktory's Chrome
//! extension's reader, `browser-extension/src/content.js`, included as it
//! is and run in an isolated world of the errand's page, which the page can
//! neither see nor call. Its refusal rules and escaping hold as they do in
//! the person's Chrome; on top of them, agreement checkboxes are the
//! person's (§4) and clicks are the browser's own input, as a person's are.
//!
//! What it answers is read as strictly as the extension's
//! (`browser::schema`): the page is hostile.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use std::sync::Mutex;
use serde_json::{Value, json};

use super::cdp::{Cdp, Error};
use crate::browser::schema::{self, Answer, Op};

/// The reader, as the extension ships it.
const CONTENT: &str = include_str!("../../../../browser-extension/src/content.js");

/// The isolated world's name.
const WORLD: &str = "claktory-errand";

/// The most page targets asked which one is shown.
const MOST_PAGES: usize = 16;

/// How long a relaunched Chrome is given to list the tabs it restores.
const RESTORING: std::time::Duration = std::time::Duration::from_secs(2);

/// How likely a page target is the errand's tab: seen, then where the
/// errand last was, then with an address.
fn rank(visible: bool, last: bool, url: &str) -> u8 {
    u8::from(visible) * 4 + u8::from(last) * 2 + u8::from(!blank(url))
}

fn blank(url: &str) -> bool {
    url.is_empty() || url == "about:blank"
}

/// Whether a page at `url` may be the errand's: blank, or on one of its
/// `sites`.
pub(crate) fn errands_own(sites: &[String], url: &str) -> bool {
    blank(url) || super::fence::check(sites, url).is_ok()
}

/// What the reader gets in place of the extension's `chrome`: the listener
/// it adds becomes `__claktoryAsk`, a promise of its answer as JSON; an
/// element's `click()`, in this world only, notes where the element is
/// instead, for the browser to click there as a person would; and
/// `__claktoryDownloads` lets it click a download link, which the
/// extension leaves to the person (R9).
fn source() -> String {
    format!(
        "(() => {{\n\
         if (globalThis.__claktoryAsk) return;\n\
         globalThis.__claktoryDownloads = true;\n\
         HTMLElement.prototype.click = function () {{\n\
           const r = this.getBoundingClientRect();\n\
           globalThis.__claktoryClick = {{x: r.left + r.width / 2, y: r.top + r.height / 2}};\n\
         }};\n\
         const chrome = {{runtime: {{id: '{WORLD}', onMessage: {{addListener: (listener) => {{\n\
           globalThis.__claktoryAsk = (command) => new Promise((reply) => {{\n\
             if (listener(command, {{id: '{WORLD}'}}, reply) !== true) reply({{error: 'unsupported_page', why: 'the page couldn\\'t be read'}});\n\
           }}).then((answer) => JSON.stringify(answer));\n\
         }}}}}}}};\n\
         {CONTENT}\n\
         }})();"
    )
}

/// Recognises a check that the visitor is a person, which an automated
/// browser can't pass (Melone 37116, Bruno 37122): Cloudflare's "Just a
/// moment..." page or its challenge form, a Turnstile frame that shows,
/// Google's /sorry page, DataDome's captcha frame that shows and HUMAN's
/// px-captcha. Not Cloudflare's script a site keeps on its ordinary pages
/// once the check passed, nor a frame it hides (Ghiaccio 40233: a shop's
/// cookie wall read as a second check). Run in the reader's world, so the
/// page can't hide one by changing what its own scripts see. Its answer:
/// who's checking, or "".
const CHECK: &str = r#"(() => {
  const shown = (el) => {
    const box = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    return box.width >= 20 && box.height >= 20 && style.display !== 'none' && style.visibility !== 'hidden' && Number(style.opacity) > 0;
  };
  const any = (selector) => Array.from(document.querySelectorAll(selector)).some(shown);
  const title = (document.title || '').trim().toLowerCase();
  if (title === 'just a moment...' || title === 'just a moment\u2026') return 'cloudflare';
  if (/(^|\.)google\.[a-z.]+$/.test(location.hostname) && location.pathname.startsWith('/sorry')) return 'google';
  if (document.querySelector('script[src*="/cdn-cgi/challenge-platform/"][src*="/orchestrate/"],#challenge-form,#challenge-stage')) return 'cloudflare';
  if (any('iframe[src*="challenges.cloudflare.com"]')) return 'cloudflare';
  if (any('iframe[src*="captcha-delivery.com"]')) return 'datadome';
  if (document.querySelector('#px-captcha,[id^="px-captcha"]')) return 'human';
  return '';
})()"#;

/// Words on a checkbox that make it an agreement: the person accepts it,
/// never an agent (§4). A heuristic, as the reader's own rules are.
fn agreement(words: &str) -> bool {
    let words = words.to_lowercase();
    ["i agree", "agree to", "accept", "terms", "agreement", "consent"].iter().any(|word| words.contains(word))
}

/// Why an agreement checkbox isn't the agent's.
pub(crate) const AGREEMENT: &str = "an agreement: the person accepts it (errand_handoff, kind \"agreement\")";

/// Roles a box the person ticks to agree may have.
fn tickable(role: &str) -> bool {
    matches!(role, "checkbox" | "switch" | "radio")
}

/// The page's top frame, as Chrome says (bounded).
pub(crate) struct Frame {
    pub id: String,
    pub loader: String,
    pub url: String,
    /// Whose its document is (securityOrigin): for about:blank, the page
    /// that made it, if any.
    pub origin: String,
}

/// The reader's world in the page's current document.
struct World {
    /// Chrome's loader of the document it's in: a new one is a new page.
    loader: String,
    context: i64,
    /// The document as the agent names it.
    document: String,
}

/// The errand's page: its CDP session, and the reader in it.
pub(crate) struct Page {
    cdp: Arc<Cdp>,
    session: String,
    world: Mutex<Option<World>>,
    documents: AtomicU64,
}

impl Page {
    pub(crate) fn new(cdp: Arc<Cdp>, session: String) -> Page {
        Page { cdp, session, world: Mutex::new(None), documents: AtomicU64::new(0) }
    }

    /// The errand's tab, attached and brought to the front: the one the
    /// person sees among those on the errand's `sites` or blank. Chrome can
    /// hold page targets its tab strip doesn't show (an about:blank beside
    /// the tabs a relaunch restored, Ghiaccio 38463), so with more than
    /// one, each is asked whether it's visible, in a world of Claktory's
    /// where its own script can't answer for it: a visible page beats a
    /// hidden one, then the one at `last` (the address the errand last
    /// drove: a relaunch gives tabs new ids, Ghiaccio 38596), then one with
    /// an address; the first of the best; the others are let go. A tab
    /// elsewhere (the person's mail, opened in their turn to copy a code,
    /// Fugo 38499) is never attached, read or moved. Just relaunched,
    /// Chrome lists the tabs it restores a moment later: it's given
    /// [`RESTORING`] to before a new blank tab is made for want of one.
    pub(crate) async fn shown(cdp: Arc<Cdp>, sites: &[String], last: Option<&str>) -> Result<Page, Error> {
        let until = std::time::Instant::now() + RESTORING;
        let pages = loop {
            let targets = cdp.call("Target.getTargets", json!({}), None).await?;
            let pages: Vec<(String, String)> = targets["targetInfos"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|target| target["type"] == "page")
                .filter_map(|target| {
                    let id = target["targetId"].as_str().filter(|id| id.len() <= 128)?;
                    let url = target["url"].as_str().unwrap_or_default();
                    errands_own(sites, url).then(|| (id.to_owned(), url.to_owned()))
                })
                .take(MOST_PAGES)
                .collect();
            if !pages.is_empty() || std::time::Instant::now() >= until {
                break pages;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        };
        let attach = async |target: &str| -> Result<String, Error> {
            let attached = cdp.call("Target.attachToTarget", json!({"targetId": target, "flatten": true}), None).await?;
            Ok(attached["sessionId"].as_str().filter(|id| id.len() <= 128).unwrap_or_default().to_owned())
        };
        let detach = async |session: &str| {
            let _ = cdp.call("Target.detachFromTarget", json!({"sessionId": session}), None).await;
        };
        // (rank, target, session)
        let mut best: Option<(u8, String, String)> = None;
        let alone = pages.len() == 1;
        for (target, url) in &pages {
            let Ok(session) = attach(target).await else { continue };
            // Alone, it's the one, shown or not.
            let rank = if alone { 0 } else { rank(Page::new(cdp.clone(), session.clone()).visible().await, last == Some(url.as_str()), url) };
            match &best {
                Some((top, _, _)) if *top >= rank => detach(&session).await,
                _ => {
                    if let Some((_, _, worse)) = best.replace((rank, target.clone(), session)) {
                        detach(&worse).await;
                    }
                }
            }
        }
        // As it is now, not as listed: it may have gone elsewhere since.
        let mut chosen = None;
        if let Some((_, target, session)) = best {
            let page = Page::new(cdp.clone(), session.clone());
            if matches!(page.elsewhere(sites).await, Ok(None)) {
                chosen = Some((target, page));
            } else {
                detach(&session).await;
            }
        }
        let (target, page) = match chosen {
            Some(chosen) => chosen,
            None => {
                let made = cdp.call("Target.createTarget", json!({"url": "about:blank"}), None).await?;
                let target = made["targetId"].as_str().unwrap_or_default().to_owned();
                let session = attach(&target).await?;
                (target, Page::new(cdp.clone(), session))
            }
        };
        // In front, so the person's live view shows the page the errand
        // drives.
        let _ = cdp.call("Target.activateTarget", json!({"targetId": target}), None).await;
        Ok(page)
    }

    /// Where the page is when it isn't the errand's: not on one of its
    /// `sites`, nor a blank page of none of them (a popup an off-site page
    /// wrote). Read, scroll and act refuse it (Fugo 38499).
    pub(crate) async fn elsewhere(&self, sites: &[String]) -> Result<Option<String>, Error> {
        let frame = self.frame().await?;
        let made_by = url::Url::parse(&frame.origin).ok().filter(|origin| matches!(origin.scheme(), "http" | "https"));
        let theirs = made_by.as_ref().is_some_and(|origin| !sites.contains(&origin.origin().ascii_serialization()));
        if errands_own(sites, &frame.url) && !theirs {
            return Ok(None);
        }
        let shown = url::Url::parse(&frame.url)
            .ok()
            .map(|url| url.origin().ascii_serialization())
            .filter(|origin| origin != "null")
            .or_else(|| made_by.map(|origin| origin.origin().ascii_serialization()))
            .unwrap_or_else(|| crate::browser::schema::one_line(&frame.url, 100));
        Ok(Some(shown))
    }

    /// Whether the page is shown, asked in a world of Claktory's.
    async fn visible(&self) -> bool {
        let asked = async {
            let Frame { id: frame, .. } = self.frame().await?;
            let made = self
                .call("Page.createIsolatedWorld", json!({"frameId": frame, "worldName": WORLD, "grantUniveralAccess": false}))
                .await?;
            let context = made["executionContextId"].as_i64().unwrap_or_default();
            let state = json!({"expression": "document.visibilityState", "contextId": context, "returnByValue": true});
            self.call("Runtime.evaluate", state).await
        };
        asked.await.is_ok_and(|said| said["result"]["value"] == "visible")
    }

    pub(crate) fn cdp(&self) -> &Arc<Cdp> {
        &self.cdp
    }

    pub(crate) fn session(&self) -> &str {
        &self.session
    }

    pub(crate) async fn call(&self, method: &str, params: Value) -> Result<Value, Error> {
        self.cdp.call(method, params, Some(&self.session)).await
    }

    /// The top frame: its id, its document's loader and its address.
    pub(crate) async fn frame(&self) -> Result<Frame, Error> {
        let tree = self.call("Page.getFrameTree", json!({})).await?;
        let frame = &tree["frameTree"]["frame"];
        let text = |key: &str, max: usize| frame[key].as_str().filter(|s| s.len() <= max).map(str::to_owned);
        match (text("id", 128), text("loaderId", 128), text("url", 8192)) {
            (Some(id), Some(loader), Some(url)) => Ok(Frame { id, loader, url, origin: text("securityOrigin", 512).unwrap_or_default() }),
            _ => Err(Error::Refused("Page.getFrameTree".into(), "no top frame".into())),
        }
    }

    /// The name of the document the page shows now, as the agent reads it.
    pub(crate) async fn document(&self) -> Result<String, Error> {
        Ok(self.world().await?.1)
    }

    /// The reader's world in the current document, made when it's new:
    /// (its context, the document's name, whether it's new).
    async fn world(&self) -> Result<(i64, String, bool), Error> {
        let Frame { id: frame, loader, .. } = self.frame().await?;
        if let Some(world) = crate::locking::lock(&self.world).as_ref().filter(|world| world.loader == loader) {
            return Ok((world.context, world.document.clone(), false));
        }
        let made = self
            .call("Page.createIsolatedWorld", json!({"frameId": frame, "worldName": WORLD, "grantUniveralAccess": false}))
            .await?;
        let context = made["executionContextId"]
            .as_i64()
            .ok_or_else(|| Error::Refused("Page.createIsolatedWorld".into(), "no context".into()))?;
        let injected = self
            .call("Runtime.evaluate", json!({"expression": source(), "contextId": context, "returnByValue": true}))
            .await?;
        if injected.get("exceptionDetails").is_some() {
            return Err(Error::Refused("Runtime.evaluate".into(), "the reader didn't start on this page".into()));
        }
        let document = format!("d{}", self.documents.fetch_add(1, Ordering::Relaxed) + 1);
        *crate::locking::lock(&self.world) = Some(World { loader, context, document: document.clone() });
        Ok((context, document, true))
    }

    /// Who's checking that the visitor is a person on the page now, if
    /// anyone ([`CHECK`]).
    pub(crate) async fn check(&self) -> Option<String> {
        let (context, _, _) = self.world().await.ok()?;
        let found = self.call("Runtime.evaluate", json!({"expression": CHECK, "contextId": context, "returnByValue": true})).await.ok()?;
        found["result"]["value"].as_str().filter(|who| matches!(*who, "cloudflare" | "google" | "datadome" | "human")).map(str::to_owned)
    }

    /// Forgets the reader's world: the page went elsewhere.
    pub(crate) fn forget(&self) {
        *crate::locking::lock(&self.world) = None;
    }

    /// Asks the reader `command` in `context`: its answer as JSON, at most
    /// `schema::RESULT_MAX`.
    async fn ask(&self, context: i64, command: Value) -> Result<String, (String, String)> {
        let expression = format!("globalThis.__claktoryAsk({command})");
        let evaluated = self
            .call(
                "Runtime.evaluate",
                json!({"expression": expression, "contextId": context, "awaitPromise": true, "returnByValue": true}),
            )
            .await;
        let evaluated = match evaluated {
            Ok(evaluated) => evaluated,
            // The world went with its document between the check and the
            // call: the page changed.
            Err(Error::Refused(..)) => {
                self.forget();
                return Err(stale_document());
            }
            Err(error) => return Err(gone(error)),
        };
        if evaluated.get("exceptionDetails").is_some() {
            return Err(("unsupported_page".to_owned(), "the page couldn't be read".to_owned()));
        }
        match evaluated["result"]["value"].as_str() {
            Some(json) if json.len() <= schema::RESULT_MAX => Ok(json.to_owned()),
            _ => Err(("unsupported_page".to_owned(), "the page's answer was too big or not one".to_owned())),
        }
    }

    /// Reads the page: at most `limit` bytes of its text, and its elements,
    /// agreement checkboxes marked as the person's.
    pub(crate) async fn read(&self, limit: usize) -> Result<Answer, (String, String)> {
        let limit = limit.clamp(1, schema::TEXT_MAX);
        let (context, document, _) = self.world().await.map_err(gone)?;
        let json = self.ask(context, json!({"op": "read", "limit": limit, "budget": schema::RESULT_MAX - 1024})).await?;
        let mut value: Value = serde_json::from_str(&json).map_err(|_| unreadable())?;
        if value.get("error").is_none() {
            value["document"] = Value::String(document);
        }
        let op = Op::Read { limit };
        let mut answer = schema::answer(&op, &value.to_string()).map_err(|_| unreadable())?;
        if let Answer::Page(page) = &mut answer {
            for element in &mut page.elements {
                let words = format!("{} {}", element.name, element.shows.as_deref().unwrap_or(""));
                if element.sensitive.is_none() && tickable(&element.role) && agreement(&words) {
                    element.sensitive = Some(AGREEMENT.to_owned());
                }
            }
        }
        Ok(answer)
    }

    /// Scrolls the page by `pages` screens.
    pub(crate) async fn scroll(&self, direction: schema::Direction, pages: u8) -> Result<Answer, (String, String)> {
        let (context, _, _) = self.world().await.map_err(gone)?;
        let op = Op::Scroll { direction, pages: pages.clamp(1, 3) };
        let json = self.ask(context, serde_json::to_value(&op).unwrap_or_default()).await?;
        schema::answer(&op, &json).map_err(|_| unreadable())
    }

    /// Clicks or fills `target` of `document`, as the agent read it: the
    /// reader checks it's still the element read, where it was, not covered
    /// and not off-limits; a click is then the browser's own, at its middle.
    pub(crate) async fn act(&self, op: &Op) -> Result<Answer, (String, String)> {
        let (document, target, kind, text) = match op {
            Op::Click { document, target, .. } => (document, target, "click", ""),
            Op::Fill { document, target, text, .. } => (document, target, "fill", text.as_str()),
            _ => return Err(("unsupported_target".to_owned(), "only a click or a fill acts on an element".to_owned())),
        };
        let (context, now, new) = self.world().await.map_err(gone)?;
        if new || *document != now {
            return Err(stale_document());
        }
        let described = self.ask(context, json!({"op": "describe", "kind": kind, "target": target, "text": text})).await?;
        let described: Value = serde_json::from_str(&described).map_err(|_| unreadable())?;
        if let Some(error) = described["error"].as_str() {
            let why = described["why"].as_str().unwrap_or_default();
            return Err(failed(error, why));
        }
        let action = &described["action"];
        let words = format!("{} {}", action["name"].as_str().unwrap_or(""), action["shows"].as_str().unwrap_or(""));
        if tickable(action["role"].as_str().unwrap_or("")) && agreement(&words) {
            return Err(("sensitive_action".to_owned(), AGREEMENT.to_owned()));
        }
        if kind == "click" {
            let cleared = json!({"expression": "globalThis.__claktoryClick = null", "contextId": context});
            self.call("Runtime.evaluate", cleared).await.map_err(gone)?;
        }
        let json = self.ask(context, json!({"op": "act", "kind": kind, "target": target, "text": text})).await?;
        let answer = schema::answer(op, &json).map_err(|_| unreadable())?;
        if !matches!(answer, Answer::Clicked(_)) {
            return Ok(answer);
        }
        // The reader checked the element and called click(): in this world
        // that only noted where; the browser clicks there now.
        let place = self
            .call(
                "Runtime.evaluate",
                json!({"expression": "(() => { const at = globalThis.__claktoryClick; globalThis.__claktoryClick = null; return at; })()", "contextId": context, "returnByValue": true}),
            )
            .await
            .map_err(gone)?;
        let at = &place["result"]["value"];
        let (Some(x), Some(y)) = (at["x"].as_f64(), at["y"].as_f64()) else {
            return Err(("outcome_unknown".to_owned(), "the click wasn't placed: read the page again".to_owned()));
        };
        if !(x.is_finite() && y.is_finite() && (0.0..100_000.0).contains(&x) && (0.0..100_000.0).contains(&y)) {
            return Err(("outcome_unknown".to_owned(), "the click wasn't placed: read the page again".to_owned()));
        }
        for kind in ["mouseMoved", "mousePressed", "mouseReleased"] {
            let mut event = json!({"type": kind, "x": x, "y": y});
            if kind != "mouseMoved" {
                event["button"] = json!("left");
                event["clickCount"] = json!(1);
            }
            if let Err(error) = self.call("Input.dispatchMouseEvent", event).await {
                let why = if kind == "mouseReleased" { "the click may have happened: read the page" } else { "the click didn't finish" };
                return Err(("outcome_unknown".to_owned(), format!("{why} ({error})")));
            }
        }
        Ok(answer)
    }
}

/// The page went elsewhere since it was read.
fn stale_document() -> (String, String) {
    ("stale_document".to_owned(), "the page changed since it was read: read it again".to_owned())
}

fn unreadable() -> (String, String) {
    ("unsupported_page".to_owned(), "the page's answer wasn't one Claktory reads".to_owned())
}

/// A reader's failure, as the extension's: only its listed words.
fn failed(error: &str, why: &str) -> (String, String) {
    if schema::ERRORS.contains(&error) {
        (error.to_owned(), schema::one_line(why, 200))
    } else {
        unreadable()
    }
}

/// The browser didn't answer.
fn gone(error: Error) -> (String, String) {
    match error {
        Error::Closed => ("interrupted".to_owned(), "the browser closed".to_owned()),
        other => ("interrupted".to_owned(), schema::one_line(&other.to_string(), 200)),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn the_tab_shown_beats_a_blank_one() {
        assert!(rank(true, false, "https://example.com/dashboard") > rank(true, false, "about:blank"));
        assert!(rank(true, false, "about:blank") > rank(false, true, "https://example.com/"));
        assert!(rank(false, true, "https://example.com/a") > rank(false, false, "https://example.com/b"));
        assert!(rank(false, false, "https://example.com/") > rank(false, false, ""));
    }

    /// A local headless Chrome and a connection to it: None where there's
    /// no Chrome.
    fn chrome(name: &str) -> Option<(super::super::cdp::Local, Arc<Cdp>, std::path::PathBuf)> {
        use super::super::cdp::{Local, Transport, find_chrome};
        let chrome = find_chrome()?;
        let profile = std::env::temp_dir().join(format!("factoryd-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&profile).unwrap();
        let local = Local::start(&chrome, &profile).unwrap();
        let cdp = Arc::new(Cdp::new(local.connect().unwrap()));
        Some((local, cdp, profile))
    }

    /// Chrome's page targets: (id, address, attached).
    async fn pages(cdp: &Cdp) -> Vec<(String, String, bool)> {
        let listed = cdp.call("Target.getTargets", json!({}), None).await.unwrap();
        listed["targetInfos"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["type"] == "page")
            .map(|t| (t["targetId"].as_str().unwrap().to_owned(), t["url"].as_str().unwrap().to_owned(), t["attached"] == true))
            .collect()
    }

    /// Points the first page at `url` (a data: page, which is no errand's
    /// site), and lets it go.
    async fn first_to(cdp: &Cdp, url: &str) -> String {
        let first = pages(cdp).await[0].0.clone();
        let attached = cdp.call("Target.attachToTarget", json!({"targetId": first, "flatten": true}), None).await.unwrap();
        let session = attached["sessionId"].as_str().unwrap();
        cdp.call("Page.navigate", json!({"url": url}), Some(session)).await.unwrap();
        cdp.call("Target.detachFromTarget", json!({"sessionId": session}), None).await.unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !pages(cdp).await.iter().any(|(id, at, _)| *id == first && at == url) && std::time::Instant::now() < until {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        first
    }

    const MAIL: &str = "data:text/html,<title>Inbox</title>Your code is 123456";
    const SITES: &[&str] = &["https://www.scrapingcourse.com"];

    fn sites() -> Vec<String> {
        SITES.iter().map(|site| (*site).to_owned()).collect()
    }

    /// The person left another site's tab in front (their mail, for a
    /// code) beside a blank one Chrome doesn't show (Ghiaccio 38463, Fugo
    /// 38499): the errand drives the blank one, and the other tab is never
    /// attached, read or moved.
    #[tokio::test]
    async fn the_page_driven_is_the_errands_never_another_sites_tab_in_front() {
        let Some((local, cdp, profile)) = chrome("shown") else { return };
        let mail = first_to(&cdp, MAIL).await;
        let made = cdp.call("Target.createTarget", json!({"url": "about:blank", "background": true}), None).await.unwrap();
        let blank = made["targetId"].as_str().unwrap().to_owned();
        assert_eq!(pages(&cdp).await.len(), 2);

        let page = Page::shown(cdp.clone(), &sites(), None).await.unwrap();
        assert_eq!(page.frame().await.unwrap().url, "about:blank");
        let now = pages(&cdp).await;
        assert_eq!(now.len(), 2, "no new tab: {now:?}");
        assert!(now.iter().any(|(id, _, attached)| *id == blank && *attached), "{now:?}");
        assert!(now.iter().any(|(id, url, attached)| *id == mail && url == MAIL && !*attached), "the mail untouched: {now:?}");
        drop(local);
        let _ = std::fs::remove_dir_all(&profile);
    }

    /// Only another site's tab: a new blank one, the other left alone.
    #[tokio::test]
    async fn with_only_another_sites_tab_a_new_blank_one_is_driven() {
        let Some((local, cdp, profile)) = chrome("shown-new") else { return };
        let mail = first_to(&cdp, MAIL).await;
        let page = Page::shown(cdp.clone(), &sites(), None).await.unwrap();
        assert_eq!(page.frame().await.unwrap().url, "about:blank");
        let now = pages(&cdp).await;
        assert_eq!(now.len(), 2, "{now:?}");
        assert!(now.iter().any(|(id, url, attached)| *id == mail && url == MAIL && !*attached), "{now:?}");
        drop(local);
        let _ = std::fs::remove_dir_all(&profile);
    }

    /// Just relaunched, Chrome lists the tabs it restores a moment later:
    /// one that comes within the wait is driven, no new tab made
    /// (Ghiaccio 38596).
    #[tokio::test]
    async fn a_tab_restored_a_moment_later_is_driven_not_a_new_one() {
        let Some((local, cdp, profile)) = chrome("shown-restored") else { return };
        let mail = first_to(&cdp, MAIL).await;
        let later = cdp.clone();
        let restored = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let made = later.call("Target.createTarget", json!({"url": "about:blank", "background": true}), None).await.unwrap();
            made["targetId"].as_str().unwrap().to_owned()
        });
        let page = Page::shown(cdp.clone(), &sites(), None).await.unwrap();
        let restored = restored.await.unwrap();
        let info = page.call("Target.getTargetInfo", json!({})).await.unwrap();
        assert_eq!(info["targetInfo"]["targetId"].as_str(), Some(restored.as_str()));
        let now = pages(&cdp).await;
        assert_eq!(now.len(), 2, "no new tab: {now:?}");
        assert!(now.iter().any(|(id, url, attached)| *id == mail && url == MAIL && !*attached), "{now:?}");
        drop(local);
        let _ = std::fs::remove_dir_all(&profile);
    }

    /// Among the errand's own, the tab shown beats a hidden one.
    #[tokio::test]
    async fn among_blank_tabs_the_one_shown_is_driven() {
        let Some((local, cdp, profile)) = chrome("shown-blank") else { return };
        let first = pages(&cdp).await[0].0.clone();
        let made = cdp.call("Target.createTarget", json!({"url": "about:blank", "background": true}), None).await.unwrap();
        let hidden = made["targetId"].as_str().unwrap().to_owned();
        let page = Page::shown(cdp.clone(), &sites(), None).await.unwrap();
        let info = page.call("Target.getTargetInfo", json!({})).await.unwrap();
        let driven = info["targetInfo"]["targetId"].as_str().unwrap_or_default().to_owned();
        let now = pages(&cdp).await;
        assert_eq!(now.iter().filter(|(_, _, attached)| *attached).count(), 1, "the other let go: {now:?}");
        assert!(driven == first && driven != hidden, "{driven} of {first} and {hidden}");
        drop(local);
        let _ = std::fs::remove_dir_all(&profile);
    }

    /// The reader won't read a tab that went to another site.
    #[tokio::test]
    async fn a_tab_on_another_site_is_elsewhere() {
        let Some((_local, page, profile)) = page_with(FORM).await else { return };
        assert_eq!(page.elsewhere(&sites()).await.unwrap(), None, "blank, the reader's own");
        let frame = page.frame().await.unwrap();
        page.call("Page.navigate", json!({"url": MAIL, "frameId": frame.id})).await.unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while page.frame().await.unwrap().url != MAIL && std::time::Instant::now() < until {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let there = page.elsewhere(&sites()).await.unwrap().expect("elsewhere");
        assert!(there.starts_with("data:text/html"), "{there}");
        let _ = std::fs::remove_dir_all(&profile);
    }

    #[test]
    fn agreements_are_the_persons() {
        for words in ["I agree to the Developer Distribution Agreement", "Accept the terms", "I consent to processing"] {
            assert!(agreement(words), "{words}");
        }
        for words in ["Organization", "Remember me", "Developer account type"] {
            assert!(!agreement(words), "{words}");
        }
        assert!(tickable("checkbox") && !tickable("button"));
    }

    /// A real headless Chrome's page, with `html` as its document: None
    /// where there's no Chrome.
    pub(crate) async fn page_with(html: &str) -> Option<(super::super::cdp::Local, Arc<Page>, std::path::PathBuf)> {
        use super::super::cdp::{Local, Transport, find_chrome};
        let chrome = find_chrome()?;
        static N: AtomicU64 = AtomicU64::new(0);
        let profile =
            std::env::temp_dir().join(format!("factoryd-reader-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&profile).unwrap();
        let local = Local::start(&chrome, &profile).unwrap();
        let cdp = Arc::new(Cdp::new(local.connect().unwrap()));
        let targets = cdp.call("Target.getTargets", json!({}), None).await.unwrap();
        let target = targets["targetInfos"].as_array().unwrap().iter().find(|t| t["type"] == "page").unwrap()["targetId"].clone();
        let attached = cdp.call("Target.attachToTarget", json!({"targetId": target, "flatten": true}), None).await.unwrap();
        let page = Arc::new(Page::new(cdp, attached["sessionId"].as_str().unwrap().to_owned()));
        let frame = page.frame().await.unwrap();
        page.call("Page.setDocumentContent", json!({"frameId": frame.id, "html": html})).await.unwrap();
        Some((local, page, profile))
    }

    const FORM: &str = r#"<!doctype html><title>Sign up</title>
        <h1>Organization</h1><p>Fill this in.</p>
        <label>Name <input id=name></label>
        <label>Password <input type=password id=pw></label>
        <label><input type=checkbox id=terms> I agree to the Developer Distribution Agreement</label>
        <button id=next onclick="document.title = 'clicked ' + event.isTrusted">Next</button>"#;

    fn element<'a>(page: &'a schema::Page, name: &str) -> &'a schema::Element {
        page.elements.iter().find(|element| element.name.contains(name)).unwrap_or_else(|| panic!("{name} in {:?}", page.elements))
    }

    #[tokio::test]
    async fn the_reader_reads_a_page_and_keeps_the_persons_controls_from_the_agent() {
        let Some((_local, page, profile)) = page_with(FORM).await else { return };
        let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
        assert!(read.text.contains("Organization") && read.title == "Sign up", "{read:?}");
        assert_eq!(element(&read, "Password").sensitive.as_deref(), Some("password"));
        assert_eq!(element(&read, "I agree").sensitive.as_deref(), Some(AGREEMENT));
        assert!(element(&read, "Next").sensitive.is_none());
        let document = read.document.clone();

        // The agreement is refused, read or not.
        let terms = element(&read, "I agree").reference.clone();
        let refused = page.act(&Op::on_element(&document, 0, &terms, None).unwrap()).await.unwrap_err();
        assert_eq!(refused.0, "sensitive_action");
        let password = element(&read, "Password").reference.clone();
        let refused = page.act(&Op::on_element(&document, 0, &password, Some("hunter2")).unwrap()).await.unwrap_err();
        assert_eq!(refused.0, "sensitive_action");

        // A fill types; a click is the browser's own, as a person's.
        let name = element(&read, "Name").reference.clone();
        let filled = page.act(&Op::on_element(&document, 0, &name, Some("Copium Labs")).unwrap()).await.unwrap();
        assert!(matches!(filled, Answer::Filled(schema::Filled { took: true, .. })), "{filled:?}");
        let next = element(&read, "Next").reference.clone();
        let clicked = page.act(&Op::on_element(&document, 0, &next, None).unwrap()).await.unwrap();
        assert!(matches!(clicked, Answer::Clicked(_)));
        let title = page.call("Runtime.evaluate", json!({"expression": "document.title", "returnByValue": true})).await.unwrap();
        assert_eq!(title["result"]["value"], "clicked true");
        let value = page.call("Runtime.evaluate", json!({"expression": "document.getElementById('name').value", "returnByValue": true})).await.unwrap();
        assert_eq!(value["result"]["value"], "Copium Labs");

        // The page can't reach the reader: it lives in a world of its own.
        let seen = page.call("Runtime.evaluate", json!({"expression": "typeof globalThis.__claktoryAsk", "returnByValue": true})).await.unwrap();
        assert_eq!(seen["result"]["value"], "undefined");

        // A ref of another document is stale.
        let stale = page.act(&Op::on_element("d999", 0, &next, None).unwrap()).await.unwrap_err();
        assert_eq!(stale.0, "stale_document");
        let _ = std::fs::remove_dir_all(&profile);
    }

    #[tokio::test]
    async fn a_check_that_the_visitor_is_a_person_is_recognised() {
        let Some((_local, page, profile)) = page_with("<title>Just a moment...</title><p>Checking your browser</p>").await else { return };
        assert_eq!(page.check().await.as_deref(), Some("cloudflare"));
        let frame = page.frame().await.unwrap();
        let turnstile = r#"<form><iframe src="https://challenges.cloudflare.com/cdn-cgi/challenge-platform/x"></iframe></form>"#;
        page.call("Page.setDocumentContent", json!({"frameId": frame.id, "html": turnstile})).await.unwrap();
        assert_eq!(page.check().await.as_deref(), Some("cloudflare"));
        let challenge = r#"<script src="/cdn-cgi/challenge-platform/h/g/orchestrate/chl_page/v1?ray=1"></script><p>Checking</p>"#;
        page.call("Page.setDocumentContent", json!({"frameId": frame.id, "html": challenge})).await.unwrap();
        assert_eq!(page.check().await.as_deref(), Some("cloudflare"));
        page.call("Page.setDocumentContent", json!({"frameId": frame.id, "html": FORM})).await.unwrap();
        assert_eq!(page.check().await, None);
        // Once it passed, a shop keeps Cloudflare's script and hidden frames
        // on its ordinary pages, a cookie wall over them: no check
        // (Ghiaccio 40233).
        let passed = r#"<script src="/cdn-cgi/challenge-platform/scripts/jsd/main.js"></script>
            <iframe src="https://challenges.cloudflare.com/cdn-cgi/challenge-platform/x" style="display:none"></iframe>
            <iframe src="https://geo.captcha-delivery.com/x" width="0" height="0"></iframe>
            <div role="dialog"><p>We use cookies</p><button>Accept all</button></div>"#;
        page.call("Page.setDocumentContent", json!({"frameId": frame.id, "html": passed})).await.unwrap();
        assert_eq!(page.check().await, None);
        let _ = std::fs::remove_dir_all(&profile);
    }

    #[test]
    fn the_reader_is_included_whole() {
        let source = source();
        assert!(source.contains("chrome.runtime.onMessage.addListener"));
        assert!(source.contains("__claktoryReader"));
    }
}
