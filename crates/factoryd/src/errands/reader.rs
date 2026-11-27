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

use std::time::{Duration, Instant};

use unicode_segmentation::UnicodeSegmentation;

use super::cdp::{Cdp, Error};
use super::keys;
use super::pace::{self, Point};
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
         globalThis.__claktoryFills = true;\n\
         globalThis.__claktoryErrand = true;\n\
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
    /// The frame it's in, when it's a frame of another page: a session of
    /// a frame in another process (site isolation) has it as its root.
    /// Empty for a tab's own page.
    pub parent: String,
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
    /// Where the errand may turn on two-step sign-in, as the person
    /// approved (`super::code::two_step_refusal`).
    two_step: std::sync::Mutex<Vec<String>>,
    /// What dismiss cleared: (document, what it was), so one that comes
    /// back isn't dismissed again in a loop (Fugo 40527).
    dismissed: Mutex<Vec<(String, String)>>,
    /// Refs read in a frame, and the frame's world they're acted on in.
    frame_refs: Mutex<std::collections::HashMap<String, frames::FrameRef>>,
    /// Sessions attached to frames in processes of their own, let go at
    /// the next read.
    frame_sessions: Mutex<Vec<String>>,
    /// The reader's worlds in frames in the page's process, by frame and
    /// document.
    frame_worlds: Mutex<std::collections::HashMap<(String, String), i64>>,
}

impl Page {
    pub(crate) fn new(cdp: Arc<Cdp>, session: String) -> Page {
        Page {
            cdp,
            session,
            world: Mutex::new(None),
            documents: AtomicU64::new(0),
            two_step: std::sync::Mutex::new(Vec::new()),
            dismissed: Mutex::new(Vec::new()),
            frame_refs: Mutex::default(),
            frame_sessions: Mutex::default(),
            frame_worlds: Mutex::default(),
        }
    }

    /// Where the errand may turn on two-step sign-in now.
    pub(crate) fn set_two_step(&self, origins: Vec<String>) {
        *crate::locking::lock(&self.two_step) = origins;
    }

    /// Where the person approved two-step sign-in.
    fn two_step(&self) -> Vec<String> {
        crate::locking::lock(&self.two_step).clone()
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

    /// A call on `session` (a frame's in a process of its own), or the
    /// page's.
    async fn call_on(&self, session: Option<&str>, method: &str, params: Value) -> Result<Value, Error> {
        self.cdp.call(method, params, Some(session.unwrap_or(&self.session))).await
    }

    /// The top frame: its id, its document's loader and its address.
    pub(crate) async fn frame(&self) -> Result<Frame, Error> {
        let tree = self.call("Page.getFrameTree", json!({})).await?;
        let frame = &tree["frameTree"]["frame"];
        let text = |key: &str, max: usize| frame[key].as_str().filter(|s| s.len() <= max).map(str::to_owned);
        match (text("id", 128), text("loaderId", 128), text("url", 8192)) {
            (Some(id), Some(loader), Some(url)) => Ok(Frame {
                id,
                loader,
                url,
                origin: text("securityOrigin", 512).unwrap_or_default(),
                parent: text("parentId", 128).unwrap_or_default(),
            }),
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
        self.ask_on(None, context, command).await
    }

    /// Asks the reader in `context` of `session` (a frame's), or of the
    /// page's.
    async fn ask_on(&self, session: Option<&str>, context: i64, command: Value) -> Result<String, (String, String)> {
        let expression = format!("globalThis.__claktoryAsk({command})");
        let evaluated = self
            .call_on(
                session,
                "Runtime.evaluate",
                json!({"expression": expression, "contextId": context, "awaitPromise": true, "returnByValue": true}),
            )
            .await;
        let evaluated = match evaluated {
            Ok(evaluated) => evaluated,
            // The world went with its document between the check and the
            // call: the page changed.
            Err(Error::Refused(..)) => {
                if session.is_none() {
                    self.forget();
                }
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
            value["document"] = Value::String(document.clone());
        }
        let op = Op::Read { limit };
        let mut answer = schema::answer(&op, &value.to_string()).map_err(|_| unreadable())?;
        if let Answer::Page(page) = &mut answer {
            let origin = self.frame().await.map(|frame| frame.origin).unwrap_or_default();
            mark_sensitive(&mut page.elements, &origin, &self.two_step());
            // Its frames, beside it: Claktory's to read, never the page's
            // to say (Fugo 40527).
            self.read_frames(page).await;
        }
        Ok(answer)
    }

    /// Scrolls the page `pages` screens as a person does (R11.4): the
    /// mouse goes where the wheel may turn (never over a field, a list, a
    /// frame or a box that scrolls on its own), and turns it in bouts,
    /// each checked again, until the screens asked for or the page's end.
    pub(crate) async fn scroll(&self, hand: &mut Hand<'_>, direction: schema::Direction, pages: u8) -> Result<Answer, (String, String)> {
        let op = Op::Scroll { direction, pages: pages.clamp(1, 3) };
        let sign = if direction == schema::Direction::Up { -1.0 } else { 1.0 };
        let mut spot = self.errand_op(json!({"op": "wheel_spot"})).await?;
        let view = (number(&spot, "view_width")?, number(&spot, "view_height")?);
        let mut at = (number(&spot, "x")?, number(&spot, "y")?);
        self.move_to(hand, at, view, 200.0).await?;
        for (n, bout) in pace::bouts(hand.rng, view.1, pages.clamp(1, 3)).into_iter().enumerate() {
            if n > 0 {
                let pause = pace::between_bouts(hand.rng);
                hand.wait_until(Instant::now() + pause).await?;
                spot = self.errand_op(json!({"op": "wheel_spot", "x": at.0, "y": at.1})).await?;
                let now = (number(&spot, "x")?, number(&spot, "y")?);
                if now != at {
                    self.move_to(hand, now, view, 200.0).await?;
                    at = now;
                }
            }
            let (y, end) = (number(&spot, "scroll_y")?, number(&spot, "scroll_max")?);
            if (sign > 0.0 && y >= end) || (sign < 0.0 && y <= 0.0) {
                break;
            }
            let mut next = Instant::now();
            for (delta, gap) in bout {
                next += gap;
                hand.wait_until(next).await?;
                let wheel = json!({"type": "mouseWheel", "x": at.0, "y": at.1, "deltaX": 0, "deltaY": sign * delta, "pointerType": "mouse"});
                self.call("Input.dispatchMouseEvent", wheel).await.map_err(gone)?;
            }
        }
        let pause = pace::between_bouts(hand.rng);
        hand.wait_until(Instant::now() + pause).await?;
        let spot = self.errand_op(json!({"op": "wheel_spot", "x": at.0, "y": at.1})).await?;
        let scrolled = json!({"scroll_y": number(&spot, "scroll_y")?});
        schema::answer(&op, &scrolled.to_string()).map_err(|_| unreadable())
    }

    /// The element `target` of `document`, as the agent read it, for a fill
    /// from the vault (#435 W6, `super::fill`): the reader checks it's still
    /// the element read, visible, enabled, on screen and not covered, then
    /// hands it over by node, never its value. Its Chrome object id, in the
    /// reader's world, and the frame that world lives in (id, loader and
    /// security origin as Chrome says them).
    pub(crate) async fn element(&self, document: &str, target: &str) -> Result<(String, Frame), (String, String)> {
        let (context, now, new) = self.world().await.map_err(gone)?;
        if new || document != now {
            return Err(stale_document());
        }
        let frame = self.frame().await.map_err(gone)?;
        let checked = self.ask(context, json!({"op": "element", "target": target})).await?;
        let checked: Value = serde_json::from_str(&checked).map_err(|_| unreadable())?;
        if let Some(error) = checked["error"].as_str() {
            return Err(failed(error, checked["why"].as_str().unwrap_or_default()));
        }
        let held = self
            .call(
                "Runtime.evaluate",
                json!({"expression": "(() => { const el = globalThis.__claktoryElement; globalThis.__claktoryElement = null; return el; })()", "contextId": context, "returnByValue": false}),
            )
            .await
            .map_err(gone)?;
        let Some(object) = held["result"]["objectId"].as_str().filter(|id| !id.is_empty() && id.len() <= 256) else {
            return Err(("stale_target".to_owned(), "the field went: read the page again".to_owned()));
        };
        // The world is the one made in this document: same loader.
        let world = crate::locking::lock(&self.world).as_ref().map(|world| world.loader.clone());
        if world.as_deref() != Some(frame.loader.as_str()) {
            return Err(stale_document());
        }
        Ok((object.to_owned(), frame))
    }

    /// Checks `target` of `document`, as the agent read it, for a click or
    /// a fill of `text`: still the element read, where it was, not covered
    /// and not off-limits.
    async fn validated(&self, document: &str, target: &str, kind: &str, text: &str) -> Result<frames::Place, (String, String)> {
        let place = self.place_of(document, target).await?;
        let described = self.ask_on(place.session.as_deref(), place.context, json!({"op": "describe", "kind": kind, "target": target, "text": text})).await?;
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
        // On an identity provider's sign-in, only Next and the like (#435
        // W6): a target in a child frame passes its own frame's origin, and
        // false.
        let (origin, main_frame) = match self.frame_origin(target) {
            Some(origin) => (origin, false),
            None => (self.frame().await.map_err(gone)?.origin, true),
        };
        // Two-step sign-in (#435 W8b): only where the person approved it,
        // and never an authenticator, security key or passkey.
        if let Some(why) = super::code::two_step_refusal(&words, &origin, &self.two_step()) {
            return Err(("sensitive_action".to_owned(), why.to_owned()));
        }
        if let Some(refused) = super::fill::on_identity_provider(&origin, main_frame, kind, action) {
            return Err(refused);
        }
        Ok(place)
    }

    /// Clicks or fills an element the agent read, as a person does
    /// (R11.4): the mouse travels to it, rests, and presses where the
    /// element is checked to be right then; a fill types each character
    /// with the browser's own keys, the field checked to hold the focus
    /// before every one, and what it holds read back.
    pub(crate) async fn act(&self, hand: &mut Hand<'_>, op: &Op) -> Result<Answer, (String, String)> {
        match op {
            Op::Click { document, target, .. } => {
                let place = self.validated(document, target, "click", "").await?;
                self.click(hand, &place, target).await?;
                schema::answer(op, r#"{"done":true}"#).map_err(|_| unreadable())
            }
            Op::Fill { document, target, text, .. } => self.fill(hand, op, document, target, text).await,
            _ => Err(("unsupported_target".to_owned(), "only a click or a fill acts on an element".to_owned())),
        }
    }

    /// Clicks `target`, validated: the mouse travels to it and presses,
    /// and the page is given a moment to answer.
    async fn click(&self, hand: &mut Hand<'_>, place: &frames::Place, target: &str) -> Result<(), (String, String)> {
        let aimed = self.op_at(place, json!({"op": "aim", "target": target})).await?;
        let spot = self.spot(place, &aimed).await?;
        let at = self.point_in(hand, &spot, target).await?;
        self.press(hand, &spot, target, at).await?;
        // Pressed and let go: the click is done, whatever comes next.
        tokio::time::sleep(pace::settle(hand.rng)).await;
        Ok(())
    }

    /// Clears what covers the page (#435, Fugo 40527): the overlay that
    /// covers most of it, never one that asks for a password, a card or a
    /// code. A cookie notice gets its most private choice (refusing what's
    /// optional, or only what's needed), else its close, else its
    /// settings and the most private choice there, and its Accept only when
    /// it offers nothing else, said in the answer (Bruno 40530); a pop-up
    /// its close. Only a button pressed as a person does, never a link or
    /// one that sends a form; Escape when there's none. One that comes back
    /// in the same page isn't dismissed again.
    pub(crate) async fn dismiss(&self, hand: &mut Hand<'_>, document: &str) -> Result<Answer, (String, String)> {
        let (context, now, new) = self.world().await.map_err(gone)?;
        if new || document != now {
            return Err(stale_document());
        }
        if untouched(&self.frame().await.map_err(gone)?.url) {
            return Err(("sensitive_action".to_owned(), UNTOUCHED_WHY.to_owned()));
        }
        // The page's own overlay, or else (or when it's only a frame's
        // holder, with no button of its own) another site's frame that
        // covers it.
        let covering = Covering::read(&self.errand_op_in(context, json!({"op": "covers"})).await?)?;
        let mut first = covering.into_iter().next().map(|over| (over, None));
        if first.as_ref().is_none_or(|(over, _)| over.buttons.is_empty())
            && let Some((over, place)) = self.covering_frame().await?
        {
            first = Some((over, Some(place)));
        }
        let Some((over, frame)) = first else {
            return Err(("unsupported_target".to_owned(), "nothing covers the page now: read it again".to_owned()));
        };
        if over.personal {
            return Err((
                "sensitive_action".to_owned(),
                format!("the {} asks for a password, a card or a code: the person's, with errand_handoff", over.kind),
            ));
        }
        let signature = format!("{}\u{1f}{}", frame.is_some(), over.signature());
        if crate::locking::lock(&self.dismissed).iter().any(|(on, what)| *on == now && *what == signature) {
            return Err((
                "unsupported_target".to_owned(),
                format!(
                    "this {} came back after it was dismissed: Claktory doesn't dismiss it again. Work around it, or hand it to the person with errand_handoff",
                    over.kind
                ),
            ));
        }
        let mut answer = schema::Dismissed { done: true, kind: over.kind.to_owned(), pressed: Vec::new(), turned_off: 0, escape: false, accepted_cookies: false, gone: false };
        let pressing = self.pressing(hand, &now, over, frame.as_ref(), &mut answer).await;
        // Once anything was pressed, it counts as dismissed, done or not.
        if !answer.pressed.is_empty() || answer.escape {
            let mut dismissed = crate::locking::lock(&self.dismissed);
            if dismissed.len() >= DISMISSED_KEPT {
                dismissed.remove(0);
            }
            dismissed.push((now.clone(), signature));
        }
        pressing?;
        let (_, current, new) = self.world().await.map_err(gone)?;
        answer.gone = new
            || current != now
            || match &frame {
                None => !self.covering_at(None).await?.iter().any(|left| left.kind == answer.kind),
                Some(_) => self.covering_frame().await.is_ok_and(|left| left.is_none()),
            };
        Ok(Answer::Dismissed(answer))
    }

    /// Presses what dismisses `over` in `document` (in `frame`, when it's
    /// a frame's), saying in `answer` what it pressed: a cookie notice's
    /// settings first, when that's where its private choice is.
    async fn pressing(
        &self,
        hand: &mut Hand<'_>,
        document: &str,
        mut over: Covering,
        frame: Option<&frames::Place>,
        answer: &mut schema::Dismissed,
    ) -> Result<(), (String, String)> {
        let mut opened_settings = false;
        loop {
            match over.pick(opened_settings) {
                Pick::Press(button, accepts) => {
                    let place = self.validated(document, &button.reference, "click", "").await?;
                    answer.pressed.push(button.name.clone());
                    answer.accepted_cookies = accepts;
                    return self.click(hand, &place, &button.reference).await;
                }
                Pick::Settings(button) => {
                    let place = self.validated(document, &button.reference, "click", "").await?;
                    answer.pressed.push(button.name.clone());
                    self.click(hand, &place, &button.reference).await?;
                    opened_settings = true;
                    let (_, now, new) = self.world().await.map_err(gone)?;
                    if new || now != document {
                        return Err(stale_document());
                    }
                    // Its settings, shown in place of the notice or over it.
                    let covering = self.covering_at(frame).await?;
                    match covering.into_iter().find(|over| over.kind == "cookie notice" && !over.personal) {
                        Some(settings) => over = settings,
                        None => return Ok(()),
                    }
                }
                Pick::Save(button) => {
                    // Saved only with every purpose off: a panel may come
                    // with some ticked, and saving those agrees to them
                    // (Fugo 41149). Each one on is turned off, once, as a
                    // person would, then all are read again. Vendor and
                    // legitimate-interest lists on another tab of the
                    // panel aren't reached: a residual Fugo accepted.
                    if over.switches.iter().any(|switch| switch.on) {
                        let mut off = Vec::new();
                        for switch in over.switches.iter().filter(|switch| switch.on).take(20) {
                            let Some(reference) = &switch.reference else { return self.escape(hand, answer).await };
                            let place = self.validated(document, reference, "click", "").await?;
                            self.click(hand, &place, reference).await?;
                            off.push(reference.clone());
                        }
                        answer.turned_off = off.len() as u32;
                        let covering = self.covering_at(frame).await?;
                        let again = covering.into_iter().find(|over| over.kind == "cookie notice" && !over.personal);
                        let save = again.as_ref().and_then(|again| match again.pick(true) {
                            Pick::Save(save) if again.switches.iter().all(|switch| !switch.on) => Some(save.reference.clone()),
                            _ => None,
                        });
                        let Some(save) = save else { return self.escape(hand, answer).await };
                        let name = again.as_ref().and_then(|again| again.buttons.iter().find(|button| button.reference == save)).map(|button| button.name.clone());
                        let place = self.validated(document, &save, "click", "").await?;
                        answer.pressed.push(name.unwrap_or_default());
                        return self.click(hand, &place, &save).await;
                    }
                    let place = self.validated(document, &button.reference, "click", "").await?;
                    answer.pressed.push(button.name.clone());
                    return self.click(hand, &place, &button.reference).await;
                }
                Pick::Escape => return self.escape(hand, answer).await,
            }
        }
    }

    /// Escape, as a person presses it, when nothing else may clear it.
    async fn escape(&self, hand: &mut Hand<'_>, answer: &mut schema::Dismissed) -> Result<(), (String, String)> {
        let pause = pace::hover(hand.rng);
        hand.wait_until(Instant::now() + pause).await?;
        answer.escape = true;
        self.press_key("Escape", "Escape", 27, pace::key_hold(hand.rng)).await.map_err(gone)?;
        tokio::time::sleep(pace::settle(hand.rng)).await;
        Ok(())
    }

    async fn fill(&self, hand: &mut Hand<'_>, op: &Op, document: &str, target: &str, text: &str) -> Result<Answer, (String, String)> {
        // Typed text is only characters: Tab moves the keyboard to another
        // field, Enter can send a form or a message (R11.3 #3).
        if text.contains('\t') {
            return Err(("unsupported_target".to_owned(), "a tab moves the keyboard to another field: fill this one without tabs".to_owned()));
        }
        if text.contains(['\n', '\r']) {
            return Err((
                "unsupported_target".to_owned(),
                "Enter can send what's typed: fill it without line breaks, and send it with the page's own button".to_owned(),
            ));
        }
        let place = self.validated(document, target, "fill", text).await?;
        let planned = pace::typing_time(hand.person, text);
        if planned > FILL_LONGEST {
            return Err((
                "limit_exceeded".to_owned(),
                format!(
                    "typing it at a person's pace takes about {} minutes, more than the {} a fill may: fill a shorter text",
                    planned.as_secs().div_ceil(60),
                    FILL_LONGEST.as_secs() / 60
                ),
            ));
        }
        let aimed = self.op_at(&place, json!({"op": "aim", "target": target})).await?;
        let spot = self.spot(&place, &aimed).await?;
        // In a frame, the frame must hold the keyboard as well.
        if aimed["focused"] != true || place.frame.is_some() {
            // Clicked into, as a person would, to take the keyboard.
            let at = self.point_in(hand, &spot, target).await?;
            self.press(hand, &spot, target, at).await?;
            let pause = pace::hover(hand.rng);
            hand.wait_until(Instant::now() + pause).await?;
            if self.focus_held(&place, target).await.is_err() {
                return Err(("unsupported_target".to_owned(), "the field didn't take the keyboard when clicked: the person fills it".to_owned()));
            }
        }
        let graphemes: Vec<&str> = text.graphemes(true).collect();
        let of = graphemes.len();
        let mut typist = pace::Typist::new(hand.person, hand.rng);
        let mut next = Instant::now();
        if aimed["holds_text"] == true {
            // Its text selected whole, to be typed over (or deleted).
            self.focus_held(&place, target).await?;
            self.select_all().await.map_err(|error| failed_after(gone(error), 0, of))?;
            let held = pace::key_hold(hand.rng);
            next = Instant::now() + typist.gap(hand.rng, "", held);
            if graphemes.is_empty() {
                hand.wait_until(next).await?;
                self.focus_held(&place, target).await?;
                self.press_key("Backspace", "Backspace", 8, pace::key_hold(hand.rng)).await.map_err(|error| failed_after(gone(error), 0, of))?;
            }
        }
        for (done, grapheme) in graphemes.iter().enumerate() {
            hand.wait_until(next).await.map_err(|failure| failed_after(failure, done, of))?;
            self.focus_held(&place, target).await.map_err(|failure| failed_after(failure, done, of))?;
            let held = pace::key_hold(hand.rng);
            let down = Instant::now();
            let lead = Duration::from_millis(u64::from(hand.rng.whole(25, 70)));
            if let Err(error) = self.type_one(grapheme, held, lead).await {
                let failure = ("outcome_unknown".to_owned(), format!("a key may not have gone through ({error})"));
                return Err(failed_after(failure, done, of));
            }
            next = down + typist.gap(hand.rng, grapheme, held);
        }
        // The page reacts (a framework re-rendering, an editor's own
        // rules), then what the field holds is read back.
        tokio::time::sleep(Duration::from_millis(u64::from(hand.rng.whole(150, 400)))).await;
        let typed = self.op_at(&place, json!({"op": "typed", "target": target, "text": text})).await?;
        schema::answer(op, &typed.to_string()).map_err(|_| unreadable())
    }

    /// The errand-only ops (content.js's errandOp): their answer, or the
    /// reader's refusal as the extension's.
    async fn errand_op(&self, command: Value) -> Result<Value, (String, String)> {
        let (context, _, _) = self.world().await.map_err(gone)?;
        self.errand_op_in(context, command).await
    }

    async fn errand_op_in(&self, context: i64, command: Value) -> Result<Value, (String, String)> {
        let json = self.ask(context, command).await?;
        let value: Value = serde_json::from_str(&json).map_err(|_| unreadable())?;
        if let Some(error) = value.get("error").and_then(Value::as_str) {
            return Err(failed(error, value["why"].as_str().unwrap_or_default()));
        }
        Ok(value)
    }

    /// The field read still has the keyboard: checked before every key.
    async fn focus_held(&self, place: &frames::Place, target: &str) -> Result<(), (String, String)> {
        if self.op_at(place, json!({"op": "focused", "target": target, "frame": place.frame.is_some()})).await?["focused"] == true {
            Ok(())
        } else {
            Err(("interrupted".to_owned(), "the field lost the keyboard".to_owned()))
        }
    }

    /// Where in `target` to press: near its middle, checked to be on it;
    /// its very middle when that isn't.
    async fn point_in(&self, hand: &mut Hand<'_>, spot: &frames::Spot<'_>, target: &str) -> Result<Point, (String, String)> {
        let (rect, view) = (spot.rect, spot.view);
        let aimed = pace::aim(hand.rng, rect, view);
        if self.hits(spot, target, aimed).await.is_ok() {
            return Ok(aimed);
        }
        let middle = ((rect.0 + rect.2 / 2.0).clamp(0.0, view.0 - 1.0), (rect.1 + rect.3 / 2.0).clamp(0.0, view.1 - 1.0));
        self.hits(spot, target, middle).await?;
        Ok(middle)
    }

    /// The mouse travels to `at`, rests, and presses there: right after
    /// `target` is checked to be the element there, nothing awaited
    /// between (R11.3 #1). Held, then let go.
    async fn press(&self, hand: &mut Hand<'_>, spot: &frames::Spot<'_>, target: &str, at: Point) -> Result<(), (String, String)> {
        self.move_to(hand, at, spot.view, spot.rect.2.min(spot.rect.3)).await?;
        let pause = pace::hover(hand.rng);
        hand.wait_until(Instant::now() + pause).await?;
        self.hits(spot, target, at).await?;
        let hold = pace::hold(hand.rng);
        if let Err(error) = self.mouse("mousePressed", at).await {
            return Err(("outcome_unknown".to_owned(), format!("the click didn't finish ({error})")));
        }
        tokio::time::sleep(hold).await;
        if let Err(error) = self.mouse("mouseReleased", at).await {
            return Err(("outcome_unknown".to_owned(), format!("the click may have happened: read the page ({error})")));
        }
        Ok(())
    }

    /// The mouse goes from where it is to `to` over a person's time, on a
    /// gentle curve inside the `view`; it first shows up somewhere inside.
    async fn move_to(&self, hand: &mut Hand<'_>, to: Point, view: Point, width: f64) -> Result<(), (String, String)> {
        let from = match *hand.pointer {
            Some((x, y)) => (x.clamp(0.0, view.0 - 1.0), y.clamp(0.0, view.1 - 1.0)),
            None => {
                let start = (view.0 * hand.rng.between(0.2, 0.8), view.1 * hand.rng.between(0.2, 0.8));
                (hand.still)()?;
                self.mouse("mouseMoved", start).await.map_err(gone)?;
                *hand.pointer = Some(start);
                start
            }
        };
        let took = pace::travel(hand.rng, (to.0 - from.0).hypot(to.1 - from.1), width);
        let mut next = Instant::now();
        for (point, gap) in pace::path(hand.rng, from, to, view, took) {
            next += gap;
            hand.wait_until(next).await?;
            self.mouse("mouseMoved", point).await.map_err(gone)?;
            *hand.pointer = Some(point);
        }
        Ok(())
    }

    /// One mouse event at `at`: the left button goes down with a press
    /// and up with a release.
    async fn mouse(&self, kind: &str, at: Point) -> Result<Value, Error> {
        let mut event = json!({"type": kind, "x": at.0, "y": at.1, "pointerType": "mouse"});
        match kind {
            "mousePressed" => {
                event["button"] = json!("left");
                event["buttons"] = json!(1);
                event["clickCount"] = json!(1);
            }
            "mouseReleased" => {
                event["button"] = json!("left");
                event["buttons"] = json!(0);
                event["clickCount"] = json!(1);
            }
            _ => event["buttons"] = json!(0),
        }
        self.call("Input.dispatchMouseEvent", event).await
    }

    /// Types `grapheme`: its key, held `held` (Shift first, `lead` before,
    /// for a capital or a symbol), or as text when no key types it.
    ///
    /// No key event carries `nativeVirtualKeyCode`, ever: it's the
    /// platform's own key number, which Chrome reads as such, so a Windows
    /// code sent on a Mac pressed other keys (Shift and C opened Chrome's
    /// settings). Chrome derives the native key from `code` and the
    /// Windows virtual key instead (Fugo 40712).
    async fn type_one(&self, grapheme: &str, held: Duration, lead: Duration) -> Result<(), Error> {
        let Some(key) = keys::key(grapheme) else {
            return self.call("Input.insertText", json!({"text": grapheme})).await.map(drop);
        };
        let modifiers = if key.shift { SHIFT } else { 0 };
        if key.shift {
            let shift = json!({"type": "rawKeyDown", "key": "Shift", "code": "ShiftLeft", "windowsVirtualKeyCode": 16, "modifiers": SHIFT});
            self.call("Input.dispatchKeyEvent", shift).await?;
            tokio::time::sleep(lead).await;
        }
        let down = json!({
            "type": "keyDown", "key": key.key, "code": key.code, "windowsVirtualKeyCode": key.vk,
            "text": grapheme, "unmodifiedText": key.unshifted, "modifiers": modifiers,
        });
        self.call("Input.dispatchKeyEvent", down).await?;
        tokio::time::sleep(held).await;
        let up = json!({"type": "keyUp", "key": key.key, "code": key.code, "windowsVirtualKeyCode": key.vk, "modifiers": modifiers});
        self.call("Input.dispatchKeyEvent", up).await?;
        if key.shift {
            let shift = json!({"type": "keyUp", "key": "Shift", "code": "ShiftLeft", "windowsVirtualKeyCode": 16, "modifiers": 0});
            self.call("Input.dispatchKeyEvent", shift).await?;
        }
        Ok(())
    }

    /// A key that types nothing (Backspace), held `held`.
    async fn press_key(&self, key: &str, code: &str, vk: u32, held: Duration) -> Result<(), Error> {
        let event = |kind: &str| json!({"type": kind, "key": key, "code": code, "windowsVirtualKeyCode": vk});
        self.call("Input.dispatchKeyEvent", event("rawKeyDown")).await?;
        tokio::time::sleep(held).await;
        self.call("Input.dispatchKeyEvent", event("keyUp")).await.map(drop)
    }

    /// Ctrl+A in the field that has the keyboard: its whole text selected.
    async fn select_all(&self) -> Result<(), Error> {
        let control = |kind: &str, modifiers: u32| {
            json!({"type": kind, "key": "Control", "code": "ControlLeft", "windowsVirtualKeyCode": 17, "modifiers": modifiers})
        };
        let a = |kind: &str| {
            let mut event = json!({"type": kind, "key": "a", "code": "KeyA", "windowsVirtualKeyCode": 65, "modifiers": CONTROL});
            if kind == "rawKeyDown" {
                event["commands"] = json!([keys::SELECT_ALL]);
            }
            event
        };
        self.call("Input.dispatchKeyEvent", control("rawKeyDown", CONTROL)).await?;
        tokio::time::sleep(Duration::from_millis(60)).await;
        self.call("Input.dispatchKeyEvent", a("rawKeyDown")).await?;
        tokio::time::sleep(Duration::from_millis(80)).await;
        self.call("Input.dispatchKeyEvent", a("keyUp")).await?;
        self.call("Input.dispatchKeyEvent", control("keyUp", 0)).await.map(drop)
    }
}

/// CDP's modifier bits.
const CONTROL: u32 = 2;
const SHIFT: u32 = 8;

/// The longest a fill may take, typed at the errand's pace: past it, it's
/// refused rather than typed faster (R11.3 #7).
const FILL_LONGEST: Duration = Duration::from_secs(15 * 60);

/// An element's box: left, top, width, height, CSS pixels.
type Rect = (f64, f64, f64, f64);

/// Sign-in, payment and check-you're-a-person services: what they show is
/// never read past its place, pressed or dismissed by an errand, whatever
/// covers what (Fugo 40527). Each with its subdomains.
const UNTOUCHED: &[&str] = &[
    "accounts.google.com",
    "appleid.apple.com",
    "login.microsoftonline.com",
    "login.live.com",
    "stripe.com",
    "stripe.network",
    "paypal.com",
    "paypalobjects.com",
    "adyen.com",
    "adyenpayments.com",
    "checkout.com",
    "braintreegateway.com",
    "braintree-api.com",
    "klarna.com",
    "klarnaservices.com",
    "challenges.cloudflare.com",
    "hcaptcha.com",
    "recaptcha.net",
    "captcha-delivery.com",
    "arkoselabs.com",
    "funcaptcha.com",
];

pub(crate) const UNTOUCHED_WHY: &str = "it's a sign-in, a payment or a check that you're a person: the person's, with errand_handoff";

/// Whether `url` is on one of [`UNTOUCHED`]'s services, or Google's
/// reCAPTCHA.
pub(crate) fn untouched(url: &str) -> bool {
    let Ok(url) = url::Url::parse(url) else { return false };
    let Some(host) = url.host_str().map(|host| host.trim_end_matches('.').to_ascii_lowercase()) else { return false };
    let under = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
    UNTOUCHED.iter().any(|domain| under(domain)) || ((under("google.com") || under("gstatic.com")) && url.path().starts_with("/recaptcha"))
}

/// How many dismissals a page keeps in mind.
const DISMISSED_KEPT: usize = 64;

/// Something covering the page, as the reader's `covers` said.
struct Covering {
    kind: &'static str,
    personal: bool,
    buttons: Vec<Button>,
    switches: Vec<Switch>,
}

/// A purpose a consent panel may switch on: whether it's on, and the ref
/// that presses it, when something of it shows.
struct Switch {
    reference: Option<String>,
    on: bool,
}

/// A button on it: what it's called, which choice that is, and whether
/// dismiss may press it (a button, not a link, a form's sending or a
/// control that's the person's).
struct Button {
    reference: String,
    name: String,
    choice: Option<&'static str>,
    usable: bool,
}

/// What dismiss does next.
enum Pick<'a> {
    /// Press it; true when it accepts the notice's cookies.
    Press(&'a Button, bool),
    /// Open a cookie notice's settings, for a more private choice there.
    Settings(&'a Button),
    /// Save the settings, once every purpose there is off.
    Save(&'a Button),
    Escape,
}

/// The choices a button's name may be.
const CHOICES: [&str; 5] = ["private", "close", "save", "settings", "accept"];

impl Covering {
    /// The reader's answer, read strictly: at most five, each with at most
    /// 40 buttons.
    fn read(covers: &Value) -> Result<Vec<Covering>, (String, String)> {
        let mut found = Vec::new();
        for over in covers["overlays"].as_array().ok_or_else(unreadable)?.iter().take(5) {
            let kind = schema::OVERLAYS.into_iter().find(|kind| over["kind"] == *kind).ok_or_else(unreadable)?;
            found.push(Covering { kind, personal: over["personal"] != false, buttons: Covering::buttons(over)?, switches: Covering::switches(over)? });
        }
        Ok(found)
    }

    /// Its buttons, as the reader listed them (at most 40).
    fn buttons(over: &Value) -> Result<Vec<Button>, (String, String)> {
        let mut buttons = Vec::new();
        for button in over["buttons"].as_array().ok_or_else(unreadable)?.iter().take(40) {
            let reference = button["ref"].as_str().filter(|id| !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric())).ok_or_else(unreadable)?;
            let choice = match &button["choice"] {
                Value::Null => None,
                said => Some(CHOICES.into_iter().find(|choice| said == *choice).ok_or_else(unreadable)?),
            };
            let usable = button.get("link").is_none() && button.get("submits").is_none() && button.get("sensitive").is_none();
            let name = schema::one_line(button["name"].as_str().unwrap_or_default(), 200);
            buttons.push(Button { reference: reference.to_owned(), name, choice, usable });
        }
        Ok(buttons)
    }

    /// Its purposes' switches, as the reader listed them (at most 40).
    fn switches(over: &Value) -> Result<Vec<Switch>, (String, String)> {
        let mut switches = Vec::new();
        for switch in over["switches"].as_array().ok_or_else(unreadable)?.iter().take(40) {
            let reference = match &switch["ref"] {
                Value::Null => None,
                said => Some(
                    said.as_str()
                        .filter(|id| !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric()))
                        .ok_or_else(unreadable)?
                        .to_owned(),
                ),
            };
            switches.push(Switch { reference, on: switch["on"] != false });
        }
        Ok(switches)
    }

    /// What it is, to know it again: its kind and its buttons' names.
    fn signature(&self) -> String {
        let names: Vec<&str> = self.buttons.iter().take(20).map(|button| button.name.as_str()).collect();
        format!("{}\u{1f}{}", self.kind, names.join("\u{1f}"))
    }

    /// The button to press, the most private first: a cookie notice's
    /// refusal, its close, its settings (then saving them as they are,
    /// once open), and its Accept only when it offers nothing else; a
    /// pop-up's refusal or close. Escape otherwise.
    fn pick(&self, opened_settings: bool) -> Pick<'_> {
        let find = |choice: &str| self.buttons.iter().find(|button| button.usable && button.choice == Some(choice));
        if self.kind == "cookie notice" {
            if let Some(button) = find("private") {
                return Pick::Press(button, false);
            }
            if opened_settings && let Some(button) = find("save") {
                return Pick::Save(button);
            }
            if let Some(button) = find("close") {
                return Pick::Press(button, false);
            }
            if !opened_settings && let Some(button) = find("settings") {
                return Pick::Settings(button);
            }
            // Accept only when it's all the notice offers: never while a
            // refusal, a close or settings is there, even one dismiss can't
            // press (Bruno 40530).
            let offered = |choice: &str| self.buttons.iter().any(|button| button.choice == Some(choice));
            let other = opened_settings || ["private", "close", "save", "settings"].into_iter().any(offered);
            return match find("accept") {
                Some(button) if !other => Pick::Press(button, true),
                _ => Pick::Escape,
            };
        }
        find("private").or_else(|| find("close")).map_or(Pick::Escape, |button| Pick::Press(button, false))
    }
}

/// What paces one step: the errand's person, its random source, where its
/// mouse is, and the check, before every input event, that the errand may
/// still drive (its state, its connection, the browser driven).
pub(crate) struct Hand<'a> {
    pub person: &'a pace::Person,
    pub rng: &'a mut pace::Rng,
    pub pointer: &'a mut Option<Point>,
    pub still: &'a (dyn Fn() -> Result<(), (String, String)> + Send + Sync),
}

impl Hand<'_> {
    /// Waits until `at`, checking at least every quarter second, and once
    /// more at the end, that the errand may still drive.
    pub(crate) async fn wait_until(&self, at: Instant) -> Result<(), (String, String)> {
        loop {
            let now = Instant::now();
            if now >= at {
                return (self.still)();
            }
            (self.still)()?;
            tokio::time::sleep((at - now).min(Duration::from_millis(250))).await;
        }
    }
}

/// A number the reader gave, bounded: the page's script can't reach the
/// reader's world, but a page can be any size.
fn number(value: &Value, key: &str) -> Result<f64, (String, String)> {
    value[key].as_f64().filter(|n| n.is_finite() && (-1e7..1e7).contains(n)).ok_or_else(unreadable)
}

/// The box `aim` gave and the viewport's size, both of some size.
fn boxes(aimed: &Value) -> Result<(Rect, Point), (String, String)> {
    let rect = (number(aimed, "left")?, number(aimed, "top")?, number(aimed, "width")?, number(aimed, "height")?);
    let view = (number(aimed, "view_width")?, number(aimed, "view_height")?);
    if view.0 < 1.0 || view.1 < 1.0 || rect.2 <= 0.0 || rect.3 <= 0.0 {
        return Err(unreadable());
    }
    Ok((rect, view))
}

/// A failure partway through typing: how much went in, never typed again.
fn failed_after((error, why): (String, String), done: usize, of: usize) -> (String, String) {
    if done == 0 {
        return (error, why);
    }
    (error, format!("{why}, after {done} of {of} characters were typed: read the page and look at the field; Claktory doesn't type them again"))
}

/// Agreement checkboxes, and controls that turn on two-step sign-in where
/// the person didn't approve it (#435 W8b), marked as the person's: on a
/// page or frame of `origin`.
fn mark_sensitive(elements: &mut [schema::Element], origin: &str, approved: &[String]) {
    for element in elements {
        let words = format!("{} {}", element.name, element.shows.as_deref().unwrap_or(""));
        if element.sensitive.is_none() && tickable(&element.role) && agreement(&words) {
            element.sensitive = Some(AGREEMENT.to_owned());
        }
        if let Some(why) = element.sensitive.is_none().then(|| super::code::two_step_refusal(&words, origin, approved)).flatten() {
            element.sensitive = Some(why.to_owned());
        }
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

#[path = "frames.rs"]
mod frames;

#[cfg(test)]
#[path = "reader_overlays.rs"]
mod overlay_tests;

#[cfg(test)]
#[path = "reader_frames.rs"]
mod frame_tests;

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
        room_for_chromes();
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

    /// Room for the many Chromes tests run at once: a Mac's 256 open
    /// files by default run out.
    pub(crate) fn room_for_chromes() {
        static RAISED: std::sync::Once = std::sync::Once::new();
        RAISED.call_once(|| {
            let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
            // SAFETY: getrlimit and setrlimit only read and write `limit`.
            unsafe {
                if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) == 0 && limit.rlim_cur < 4096 {
                    limit.rlim_cur = limit.rlim_max.min(4096);
                    libc::setrlimit(libc::RLIMIT_NOFILE, &limit);
                }
            }
        });
    }

    const FORM: &str = r#"<!doctype html><title>Sign up</title>
        <h1>Organization</h1><p>Fill this in.</p>
        <label>Name <input id=name></label>
        <label>Password <input type=password id=pw></label>
        <label><input type=checkbox id=terms> I agree to the Developer Distribution Agreement</label>
        <button id=next onclick="document.title = 'clicked ' + event.isTrusted">Next</button>"#;

    pub(crate) fn element<'a>(page: &'a schema::Page, name: &str) -> &'a schema::Element {
        page.elements.iter().find(|element| element.name.contains(name)).unwrap_or_else(|| panic!("{name} in {:?}", page.elements))
    }

    /// A hand for tests: a quick person, a fixed seed, always free to drive.
    pub(crate) struct TestHand {
        person: pace::Person,
        rng: pace::Rng,
        pointer: Option<Point>,
    }

    fn free() -> Result<(), (String, String)> {
        Ok(())
    }

    impl TestHand {
        pub(crate) fn new() -> TestHand {
            TestHand { person: pace::Person { typing_wpm: 75.0, reading_wpm: 300.0 }, rng: pace::Rng::seeded(1), pointer: None }
        }

        pub(crate) fn hand(&mut self) -> Hand<'_> {
            Hand { person: &self.person, rng: &mut self.rng, pointer: &mut self.pointer, still: &free }
        }
    }

    #[tokio::test]
    async fn the_reader_reads_a_page_and_keeps_the_persons_controls_from_the_agent() {
        let Some((_local, page, profile)) = page_with(FORM).await else { return };
        let mut hands = TestHand::new();
        let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
        assert!(read.text.contains("Organization") && read.title == "Sign up", "{read:?}");
        assert_eq!(element(&read, "Password").sensitive.as_deref(), Some("password"));
        assert_eq!(element(&read, "I agree").sensitive.as_deref(), Some(AGREEMENT));
        assert!(element(&read, "Next").sensitive.is_none());
        let document = read.document.clone();

        // The agreement is refused, read or not.
        let terms = element(&read, "I agree").reference.clone();
        let refused = page.act(&mut hands.hand(), &Op::on_element(&document, 0, &terms, None).unwrap()).await.unwrap_err();
        assert_eq!(refused.0, "sensitive_action");
        let password = element(&read, "Password").reference.clone();
        let refused = page.act(&mut hands.hand(), &Op::on_element(&document, 0, &password, Some("hunter2")).unwrap()).await.unwrap_err();
        assert_eq!(refused.0, "sensitive_action");

        // A fill types; a click is the browser's own, as a person's.
        let name = element(&read, "Name").reference.clone();
        let filled = page.act(&mut hands.hand(), &Op::on_element(&document, 0, &name, Some("Copium Labs")).unwrap()).await.unwrap();
        assert!(matches!(filled, Answer::Filled(schema::Filled { took: true, .. })), "{filled:?}");
        let next = element(&read, "Next").reference.clone();
        let clicked = page.act(&mut hands.hand(), &Op::on_element(&document, 0, &next, None).unwrap()).await.unwrap();
        assert!(matches!(clicked, Answer::Clicked(_)));
        let title = page.call("Runtime.evaluate", json!({"expression": "document.title", "returnByValue": true})).await.unwrap();
        assert_eq!(title["result"]["value"], "clicked true");
        let value = page.call("Runtime.evaluate", json!({"expression": "document.getElementById('name').value", "returnByValue": true})).await.unwrap();
        assert_eq!(value["result"]["value"], "Copium Labs");

        // The page can't reach the reader: it lives in a world of its own.
        let seen = page.call("Runtime.evaluate", json!({"expression": "typeof globalThis.__claktoryAsk", "returnByValue": true})).await.unwrap();
        assert_eq!(seen["result"]["value"], "undefined");

        // A ref of another document is stale.
        let stale = page.act(&mut hands.hand(), &Op::on_element("d999", 0, &next, None).unwrap()).await.unwrap_err();
        assert_eq!(stale.0, "stale_document");
        let _ = std::fs::remove_dir_all(&profile);
    }

    /// What the page saw of the input, as its own script records it.
    pub(crate) async fn seen(page: &Page, expression: &str) -> Value {
        page.call("Runtime.evaluate", json!({"expression": expression, "returnByValue": true})).await.unwrap()["result"]["value"].clone()
    }

    const RECORDED: &str = r#"<script>
        window.events = [];
        for (const kind of ['mousemove', 'mousedown', 'mouseup', 'click', 'keydown', 'keyup', 'input', 'wheel'])
          addEventListener(kind, (e) => events.push({kind, trusted: e.isTrusted, at: e.timeStamp, key: e.key || '', field: (e.target && e.target.id) || ''}), true);
        </script>"#;

    /// R11 (#530): a person's input, from the browser itself. The mouse
    /// travels before it presses and holds the button a moment; a fill types
    /// every character with a trusted key, over what the field held; a
    /// scroll turns the wheel.
    #[tokio::test]
    async fn the_errand_moves_types_and_scrolls_as_a_person_does() {
        let html = format!(
            r#"{RECORDED}<label>Name <input id=name value="Old name"></label><label>About <textarea id=about></textarea></label>
            <button id=next onclick="document.title = 'clicked ' + event.isTrusted">Next</button><div style="height:4000px">Long</div>"#
        );
        let Some((_local, page, profile)) = page_with(&html).await else { return };
        let mut hands = TestHand::new();
        let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
        let document = read.document.clone();
        let name = element(&read, "Name").reference.clone();
        let filled = page.act(&mut hands.hand(), &Op::on_element(&document, 0, &name, Some("Copium Labs")).unwrap()).await.unwrap();
        assert!(matches!(filled, Answer::Filled(schema::Filled { took: true, .. })), "{filled:?}");
        assert_eq!(seen(&page, "document.getElementById('name').value").await, "Copium Labs");
        let keys = seen(&page, "events.filter(e => e.kind == 'keydown' && e.field == 'name').map(e => e.key).join('')").await;
        assert_eq!(keys, "ControlaShiftCopium ShiftLabs", "{keys}");
        assert_eq!(seen(&page, "events.every(e => e.trusted)").await, true);
        // Keys at a typist's pace, not at once.
        let span = seen(&page, "(() => { const k = events.filter(e => e.kind == 'keydown'); return k[k.length - 1].at - k[0].at; })()").await;
        assert!(span.as_f64().unwrap() > 1500.0, "{span}");

        let next = element(&read, "Next").reference.clone();
        let clicked = page.act(&mut hands.hand(), &Op::on_element(&document, 0, &next, None).unwrap()).await.unwrap();
        assert!(matches!(clicked, Answer::Clicked(_)));
        assert_eq!(seen(&page, "document.title").await, "clicked true");
        let moves = seen(&page, "events.filter(e => e.kind == 'mousemove').length").await;
        assert!(moves.as_u64().unwrap() >= 10, "the mouse travelled: {moves}");
        let held = seen(&page, "(() => { const d = events.findLast(e => e.kind == 'mousedown'), u = events.findLast(e => e.kind == 'mouseup'); return u.at - d.at; })()").await;
        assert!((35.0..400.0).contains(&held.as_f64().unwrap()), "held {held}");

        // Tab and Enter are never typed.
        let about = element(&read, "About").reference.clone();
        for text in ["one\ttwo", "one\ntwo"] {
            let refused = page.act(&mut hands.hand(), &Op::on_element(&document, 0, &about, Some(text)).unwrap()).await.unwrap_err();
            assert_eq!(refused.0, "unsupported_target", "{text:?}");
        }

        let scrolled = page.scroll(&mut hands.hand(), schema::Direction::Down, 1).await.unwrap();
        let Answer::Scrolled(scrolled) = scrolled else { panic!("scrolled") };
        assert!(scrolled.scroll_y > 100.0, "{scrolled:?}");
        let wheels = seen(&page, "events.filter(e => e.kind == 'wheel' && e.trusted).length").await;
        assert!(wheels.as_u64().unwrap() >= 5, "{wheels}");
        let _ = std::fs::remove_dir_all(&profile);
    }

    /// The field loses the keyboard mid-word (the page moves it): typing
    /// stops, says how far it got, and never types again; the other field
    /// gets at most the one key that raced it.
    #[tokio::test]
    async fn typing_stops_when_the_field_loses_the_keyboard() {
        let html = r#"<label>Name <input id=name></label><label>Other <input id=other></label>
            <script>let n = 0; document.getElementById('name').addEventListener('keydown', () => { if (++n == 3) document.getElementById('other').focus(); });</script>"#;
        let Some((_local, page, profile)) = page_with(html).await else { return };
        let mut hands = TestHand::new();
        let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
        let name = element(&read, "Name").reference.clone();
        let refused = page.act(&mut hands.hand(), &Op::on_element(&read.document, 0, &name, Some("Copium")).unwrap()).await.unwrap_err();
        assert_eq!(refused.0, "interrupted", "{refused:?}");
        // The third keydown (Shift, C, o) moved it: "o" raced, "p" waits.
        assert!(refused.1.contains("after 2 of 6 characters") && refused.1.contains("doesn't type them again"), "{}", refused.1);
        let other = seen(&page, "document.getElementById('other').value").await;
        assert!(other.as_str().unwrap().chars().count() <= 1, "{other}");
        let _ = std::fs::remove_dir_all(&profile);
    }

    /// Every key keys.rs maps, typed into a field, with Ctrl+A over what it
    /// held: none reaches Chrome's own shortcuts (Fugo 40712, after Shift
    /// and C opened a Mac's settings): the page, its tabs and its window
    /// stay as they were, and the field holds exactly the text.
    #[tokio::test]
    async fn no_key_typed_reaches_the_browsers_own_shortcuts() {
        let Some((_local, page, profile)) = page_with(r#"<label>Everything <input id=all value="was here"></label>"#).await else { return };
        let mut hands = TestHand::new();
        let targets = |cdp: Arc<Cdp>| async move {
            let listed = cdp.call("Target.getTargets", json!({}), None).await.unwrap();
            // Workers come and go as Chrome's own extensions wake: not a key's doing.
            let mut urls: Vec<String> = listed["targetInfos"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|t| !t["type"].as_str().unwrap().ends_with("worker"))
                .map(|t| format!("{} {}", t["type"].as_str().unwrap(), t["url"].as_str().unwrap()))
                .collect();
            urls.sort();
            urls
        };
        // Chrome 158 preloads its omnibox popup's page a few seconds after it
        // starts, with no input at all (Fugo 41917): waited for before any
        // key, so it's in the targets before as well as after, and anything
        // else new is a key's doing. An older Chrome never shows it.
        let popup = "browser_ui chrome://omnibox-popup.top-chrome/".to_owned();
        for _ in 0..150 {
            if targets(page.cdp().clone()).await.contains(&popup) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let window = |page: &Page| {
            let page = page.cdp().clone();
            async move { page.call("Browser.getWindowForTarget", json!({}), None).await.map(|w| w["bounds"].clone()).ok() }
        };
        let (before, url, bounds) = (targets(page.cdp().clone()).await, page.frame().await.unwrap().url, window(&page).await);
        let every: String = (' '..='~').collect();
        assert_eq!(every.chars().filter(|c| keys::key(&c.to_string()).is_some()).count(), 95, "every printable ASCII has its key");
        let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
        let field = element(&read, "Everything").reference.clone();
        let filled = page.act(&mut hands.hand(), &Op::on_element(&read.document, 0, &field, Some(&every)).unwrap()).await.unwrap();
        assert!(matches!(filled, Answer::Filled(schema::Filled { took: true, .. })), "{filled:?}");
        assert_eq!(seen(&page, "document.getElementById('all').value").await, every.as_str());
        assert_eq!(page.frame().await.unwrap().url, url, "the page stayed");
        let after = targets(page.cdp().clone()).await;
        assert_eq!(after, before, "no tab, window or Chrome page opened");
        assert!(after.iter().all(|target| !target.starts_with("page chrome://")), "{after:?}");
        assert_eq!(window(&page).await, bounds, "the window as it was");
        let _ = std::fs::remove_dir_all(&profile);
    }

    /// Stopped mid-step (the person's Stop or turn): no more input.
    #[tokio::test]
    async fn a_step_stops_at_its_next_event_once_the_errand_may_not_drive() {
        let html = format!("{RECORDED}<label>Name <input id=name></label>");
        let Some((_local, page, profile)) = page_with(&html).await else { return };
        let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
        let name = element(&read, "Name").reference.clone();
        let checks = std::sync::atomic::AtomicUsize::new(0);
        let still = || {
            if checks.fetch_add(1, Ordering::SeqCst) < 20 { Ok(()) } else { Err(("paused".to_owned(), "the person took over".to_owned())) }
        };
        let (person, mut rng, mut pointer) = (pace::Person { typing_wpm: 75.0, reading_wpm: 300.0 }, pace::Rng::seeded(2), None);
        let mut hand = Hand { person: &person, rng: &mut rng, pointer: &mut pointer, still: &still };
        let refused = page.act(&mut hand, &Op::on_element(&read.document, 0, &name, Some("A longer name than twenty checks")).unwrap()).await.unwrap_err();
        assert_eq!(refused.0, "paused", "{refused:?}");
        let after = seen(&page, "events.length").await;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert_eq!(seen(&page, "events.length").await, after, "nothing more");
        let typed = seen(&page, "document.getElementById('name').value").await;
        assert!(typed.as_str().unwrap().len() < "A longer name than twenty checks".len(), "{typed}");
        let _ = std::fs::remove_dir_all(&profile);
    }

    /// The wheel never turns over a field or a box that scrolls itself:
    /// with nowhere else on the page, the scroll is refused.
    #[tokio::test]
    async fn the_wheel_turns_only_where_it_changes_nothing_but_the_page() {
        let html = r#"<textarea style="position:fixed;inset:0;width:100%;height:100%"></textarea><div style="height:4000px"></div>"#;
        let Some((_local, page, profile)) = page_with(html).await else { return };
        let mut hands = TestHand::new();
        let refused = page.scroll(&mut hands.hand(), schema::Direction::Down, 1).await.unwrap_err();
        assert_eq!(refused.0, "unsupported_target", "{refused:?}");
        assert_eq!(seen(&page, "scrollY").await, 0);
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

    /// #435 W8b (Fugo 40691): recovery codes are withheld whole, text and
    /// all, offered only as a ref; what's inside them isn't offered; and
    /// once withheld they stay withheld, even if the page changes their
    /// words.
    #[tokio::test]
    async fn recovery_codes_are_withheld_from_every_read() {
        let codes = r#"<!doctype html><title>Two-step sign-in</title>
            <h1>Save your backup codes</h1><p>Each works once.</p>
            <div id=codes><ul><li>a8f2-91kd</li><li>c3m9-2wle</li><li>k1vv-o3ds</li></ul>
            <ul><li>7bq1-x0pe</li><li>p0za-77rt</li><li>u8yn-41bc</li></ul><button>Copy u8yn-41bc</button></div>
            <p>Order 1234 shipped on 2026-11-02.</p><button>Continue</button>"#;
        let Some((_local, page, profile)) = page_with(codes).await else { return };
        let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
        let block = element(&read, "recovery codes (withheld)");
        assert_eq!(block.sensitive.as_deref(), Some("recovery codes: the person's (errand_handoff, kind recovery_codes)"));
        let everything = format!("{read:?}");
        for code in ["a8f2-91kd", "7bq1-x0pe", "u8yn-41bc"] {
            assert!(!everything.contains(code), "{code} in {everything}");
        }
        assert!(read.text.contains("Save your backup codes") && read.text.contains("Order 1234"), "{}", read.text);
        assert!(element(&read, "Continue").sensitive.is_none());
        assert!(!read.elements.iter().any(|element| element.name.contains("Copy")), "{:?}", read.elements);
        // Claktory reads them by the ref, as errand_handoff does.
        let (object, _) = page.element(&read.document, &block.reference).await.expect("the block");
        let text = page
            .call("Runtime.callFunctionOn", json!({"objectId": object, "functionDeclaration": "function () { return this.innerText; }", "returnByValue": true}))
            .await
            .unwrap();
        assert!(text["result"]["value"].as_str().unwrap().contains("p0za-77rt"));
        // The page renames its heading: still withheld.
        page.call("Runtime.evaluate", json!({"expression": "document.querySelector('h1').textContent = 'Done'; document.title = 'Done'"})).await.unwrap();
        let Ok(Answer::Page(again)) = page.read(4096).await else { panic!("a page") };
        assert!(!format!("{again:?}").contains("a8f2-91kd"), "{again:?}");
        // An authenticator's setup key (Fugo 42966): an otpauth:// address
        // anywhere, or a typed key on a page about it, is never read nor
        // offered, even as a ref.
        let frame = page.frame().await.unwrap();
        let qr = r#"<title>Authenticator</title><h1>Scan this with your authenticator app</h1>
            <a href="otpauth://totp/Shop:matt?secret=JBSWY3DPEHPK3PXP&issuer=Shop"><img alt="otpauth://totp/Shop:matt?secret=JBSWY3DPEHPK3PXP" src="data:image/png;base64,AAAA"></a>
            <p>otpauth://totp/Shop:matt?secret=JBSWY3DPEHPK3PXP</p><button>Next</button>"#;
        let typed = r#"<title>Authenticator</title><p>Can't scan it? Enter this key instead:</p>
            <p><code>jbsw y3dp ehpk 3pxp</code></p><p>Then type the 6-digit code it shows.</p><button>Verify</button>"#;
        for html in [qr, typed] {
            page.call("Page.setDocumentContent", json!({"frameId": frame.id, "html": html})).await.unwrap();
            let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
            let everything = format!("{read:?}").to_lowercase();
            assert!(!everything.contains("jbswy3dp") && !everything.contains("jbsw y3dp") && !everything.contains("otpauth"), "{read:?}");
            assert!(!read.elements.iter().any(|element| element.name.contains("withheld")), "{:?}", read.elements);
            assert!(read.elements.iter().any(|element| element.name == "Next" || element.name == "Verify"), "{:?}", read.elements);
        }
        // Secco 43242: a key drawn a group per element, a key with no
        // digits, a key in a read-only field's value, and a field named by
        // a withheld block: none read nor offered; a sentence of
        // four-letter words on the same page is read.
        let spans = r#"<title>Set up your authenticator</title>
            <p id=k>Your key: <span>JBSW</span><span>Y3DP</span><span>EHPK</span><span>3PXP</span></p>
            <p>Or this one: <b>QWRT XKPL MNBV ZCSD</b></p>
            <input readonly aria-label="Setup key" value="KRSX G5CT MVRX AAAA">
            <input aria-labelledby=k placeholder="Code"><p>Then type this code from your app.</p><button>Verify</button>"#;
        page.call("Page.setDocumentContent", json!({"frameId": frame.id, "html": spans})).await.unwrap();
        let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
        let everything = format!("{read:?}").to_lowercase();
        for part in ["jbsw", "ehpk", "3pxp", "qwrt", "zcsd", "krsx", "setup key"] {
            assert!(!everything.contains(part), "{part} in {read:?}");
        }
        assert!(read.text.contains("this code from your app"), "{}", read.text);
        assert!(read.elements.iter().any(|element| element.name == "Code"), "{:?}", read.elements);
        assert!(read.elements.iter().any(|element| element.name == "Verify"), "{:?}", read.elements);
        // A page about codes too long to scan whole: nothing past the scan
        // is read, and the reader says so.
        let long = "<title>Backup codes</title><h1>Your backup codes</h1><div id=filler></div><ul id=late><li>z9q1-4kfp</li><li>m2xc-81bd</li><li>r5tt-0lqa</li><li>h7wn-3ys2</li></ul>";
        page.call("Page.setDocumentContent", json!({"frameId": frame.id, "html": long})).await.unwrap();
        page.call(
            "Runtime.evaluate",
            json!({"expression": "const f = document.getElementById('filler'); for (let i = 0; i < 6000; i++) { const s = document.createElement('span'); s.textContent = 'word '; f.appendChild(s); }"}),
        )
        .await
        .unwrap();
        let Ok(Answer::Page(long)) = page.read(65536).await else { panic!("a page") };
        assert!(!format!("{long:?}").contains("z9q1"), "{long:?}");
        assert!(long.unsupported.iter().any(|why| why.contains("too long to check")), "{:?}", long.unsupported);
        // Open shadow roots, which an errand reads and the scan doesn't see:
        // one that may hold codes or a key is left out whole, its text and
        // its elements, and the reader says so; another's text is read.
        let shadowed = |html: &str, inside: &str| {
            format!(
                "{{ document.documentElement.innerHTML = {html:?}; document.getElementById('host').attachShadow({{mode: 'open'}}).innerHTML = {inside:?}; }}"
            )
        };
        // A component too long to check whole, its codes past the scan's
        // limit (Fugo 48548): left out, as the page past its cutoff is.
        let long = format!("{}<ul><li>q4ty-8mzx</li><li>b2rk-61dn</li><li>w0pl-3vhc</li><li>e5ga-97sj</li></ul>", "<span>word</span>".repeat(5100));
        let cases = [
            ("<title>Backup codes</title><h1>Save your backup codes</h1><div id=host></div>", "<ul><li>q4ty-8mzx</li><li>b2rk-61dn</li><li>w0pl-3vhc</li><li>e5ga-97sj</li></ul><button>Copy q4ty-8mzx</button>"),
            ("<title>Authenticator</title><p>Can't scan it? Enter this key instead:</p><div id=host></div>", "<code>mfrg gzdf mztw q2lk</code><button>Copy mfrg gzdf mztw q2lk</button>"),
            ("<title>Backup codes</title><h1>Save your backup codes</h1><div id=host></div>", long.as_str()),
        ];
        for (html, inside) in cases {
            let done = page.call("Runtime.evaluate", json!({"expression": shadowed(html, inside)})).await.unwrap();
            assert!(done.get("exceptionDetails").is_none(), "{done}");
            let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
            assert_eq!(read.title, html[7..html.find("</title>").unwrap()]);
            let everything = format!("{read:?}").to_lowercase();
            for part in ["q4ty", "e5ga", "mfrg", "q2lk", "copy"] {
                assert!(!everything.contains(part), "{part} in {read:?}");
            }
            assert!(read.unsupported.iter().any(|why| why.contains("inside a component")), "{:?}", read.unsupported);
        }
        let shop = shadowed("<title>Kettles</title><h1>Kettles</h1><div id=host></div>", "<p>Free delivery over 40 pounds</p><button>Add to basket</button>");
        let done = page.call("Runtime.evaluate", json!({"expression": shop})).await.unwrap();
        assert!(done.get("exceptionDetails").is_none(), "{done}");
        let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
        assert!(read.text.contains("Free delivery"), "{read:?}");
        assert!(read.elements.iter().any(|element| element.name == "Add to basket"), "{:?}", read.elements);
        assert!(!read.unsupported.iter().any(|why| why.contains("inside a component")), "{:?}", read.unsupported);
        // An ordinary page with numbers isn't touched.
        let orders = "<title>Orders</title><p>Orders 1001 1002 1003 1004 1005 shipped.</p>";
        page.call("Page.setDocumentContent", json!({"frameId": frame.id, "html": orders})).await.unwrap();
        let Ok(Answer::Page(plain)) = page.read(4096).await else { panic!("a page") };
        assert!(plain.text.contains("1004") && plain.elements.is_empty(), "{plain:?}");
        let _ = std::fs::remove_dir_all(&profile);
    }

    #[test]
    fn the_reader_is_included_whole() {
        let source = source();
        assert!(source.contains("chrome.runtime.onMessage.addListener"));
        assert!(source.contains("__claktoryReader"));
    }
}
