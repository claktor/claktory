//! The page's frames, for an errand (#435, Fugo 40527). A frame of the
//! page's own site (https, the same registrable domain by the public
//! suffix list) is read and acted on as the page is, its text and elements
//! beside the page's, under its origin. Another site's shows only as its
//! place, except one that covers the page (most of it, or keeping it from
//! scrolling): dismiss may read its buttons' names, never its text, and
//! press its refusal or its close. Sign-in, payment and check services
//! ([`super::UNTOUCHED`]) are never read past their place, whoever's frame.
//!
//! Only the page's own frames are listed (not a frame's frames). Chrome
//! says where each is from and where it is: the frame tree, its targets and
//! the box of the element that holds it, never the page.

use serde_json::{Value, json};

use super::{Covering, Page, Rect, UNTOUCHED_WHY, gone, mark_sensitive, stale_document, unreadable};
use crate::browser::schema::{self, Answer, Op};
use crate::errands::cdp::Error;
use crate::errands::pace::Point;

/// The most frames listed with a page.
const FRAMES_MOST: usize = 10;
/// The most text read from one frame, in bytes, and its whole answer.
const FRAME_TEXT: usize = 4096;
const FRAME_BUDGET: usize = 16 * 1024;
/// How much of the page a frame of another site covers to be an overlay:
/// most of it, or a tenth while the page can't scroll.
const COVERS_MOST: f64 = 0.5;
const COVERS_BLOCKED: f64 = 0.1;

/// Where a ref is acted on: the reader's world (`context`) on `session`
/// (a frame in a process of its own; the page's when None), in a frame
/// (its id, and the node of the element that holds it in the page) or the
/// page itself.
#[derive(Clone, Debug)]
pub(in crate::errands) struct Place {
    pub session: Option<String>,
    pub context: i64,
    pub frame: Option<(String, i64)>,
}

/// A ref read in a frame: where it's acted on, and the frame's document
/// then (a new one is a new page in the frame).
#[derive(Clone, Debug)]
pub(super) struct FrameRef {
    place: Place,
    loader: String,
    /// The frame's origin when it was read.
    origin: String,
}

/// Where an element is, for the mouse: its box and the viewport, in the
/// page's coordinates, and how far its frame is from the page's corner.
pub(in crate::errands) struct Spot<'a> {
    pub place: &'a Place,
    pub rect: Rect,
    pub view: Point,
    offset: Point,
}

/// A frame of the page, as Chrome lists it.
struct Listed {
    id: String,
    url: String,
    /// Its document, when it's in the page's process.
    loader: Option<String>,
    /// A target of its own: a process of its own.
    own_process: bool,
    owner: i64,
}

/// Whether `frame` is of `page`'s site: both https, the same registrable
/// domain (Fugo 40527).
pub(crate) fn same_site(page: &str, frame: &str) -> bool {
    let site = |address: &str| {
        let url = url::Url::parse(address).ok().filter(|url| url.scheme() == "https")?;
        let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
        psl::domain_str(&host).map(str::to_owned)
    };
    matches!((site(page), site(frame)), (Some(page), Some(frame)) if page == frame)
}

/// An address's origin, as the agent reads it.
fn origin_of(address: &str) -> String {
    url::Url::parse(address)
        .ok()
        .map(|url| url.origin().ascii_serialization())
        .filter(|origin| origin != "null")
        .unwrap_or_else(|| schema::one_line(address, 100))
}

impl Page {
    /// The place `target` of `document` is acted on: the page's, or the
    /// frame it was read in, while that frame shows the same document.
    pub(super) async fn place_of(&self, document: &str, target: &str) -> Result<Place, (String, String)> {
        let (context, now, new) = self.world().await.map_err(gone)?;
        if new || document != now {
            return Err(stale_document());
        }
        let found = crate::locking::lock(&self.frame_refs).get(target).cloned();
        let Some(found) = found else { return Ok(Place { session: None, context, frame: None }) };
        let (frame, _) = found.place.frame.as_ref().ok_or_else(unreadable)?;
        if self.loader_of(found.place.session.as_deref(), frame).await.as_deref() != Some(found.loader.as_str()) {
            return Err(("stale_document".to_owned(), "the frame it's in changed since it was read: read the page again".to_owned()));
        }
        Ok(found.place)
    }

    /// The origin of the frame `target` was read in, as read; None for one
    /// of the page's own.
    pub(super) fn frame_origin(&self, target: &str) -> Option<String> {
        crate::locking::lock(&self.frame_refs).get(target).map(|found| found.origin.clone())
    }

    /// An errand op in `place`.
    pub(super) async fn op_at(&self, place: &Place, command: Value) -> Result<Value, (String, String)> {
        let json = self.ask_on(place.session.as_deref(), place.context, command).await?;
        let value: Value = serde_json::from_str(&json).map_err(|_| unreadable())?;
        if let Some(error) = value.get("error").and_then(Value::as_str) {
            return Err(super::failed(error, value["why"].as_str().unwrap_or_default()));
        }
        Ok(value)
    }

    /// Where the element `aim` answered for is, in the page's coordinates:
    /// in a frame, moved by the frame's place and cut to what of the frame
    /// shows.
    pub(super) async fn spot<'a>(&self, place: &'a Place, aimed: &Value) -> Result<Spot<'a>, (String, String)> {
        let (rect, view) = super::boxes(aimed)?;
        let Some((_, owner)) = &place.frame else { return Ok(Spot { place, rect, view, offset: (0.0, 0.0) }) };
        let frame = self.owner_box(*owner).await.map_err(gone)?.ok_or_else(|| {
            ("stale_target".to_owned(), "its frame isn't shown now: read the page again".to_owned())
        })?;
        let view = self.view().await.map_err(gone)?;
        let (left, top) = ((rect.0 + frame.0).max(frame.0).max(0.0), (rect.1 + frame.1).max(frame.1).max(0.0));
        let right = (rect.0 + frame.0 + rect.2).min(frame.0 + frame.2).min(view.0);
        let bottom = (rect.1 + frame.1 + rect.3).min(frame.1 + frame.3).min(view.1);
        if right - left < 1.0 || bottom - top < 1.0 {
            return Err(("stale_target".to_owned(), "offscreen in its frame: scroll, read, then act".to_owned()));
        }
        Ok(Spot { place, rect: (left, top, right - left, bottom - top), view, offset: (frame.0, frame.1) })
    }

    /// Whether `target` is what's at `at` (the page's coordinates): in a
    /// frame, the frame is what's there in the page, and the element what's
    /// there in the frame.
    pub(super) async fn hits(&self, spot: &Spot<'_>, target: &str, at: Point) -> Result<(), (String, String)> {
        if let Some((frame, owner)) = &spot.place.frame {
            let found = self
                .call("DOM.getNodeForLocation", json!({"x": at.0.round() as i64, "y": at.1.round() as i64, "includeUserAgentShadowDOM": false}))
                .await
                .map_err(gone)?;
            if found["frameId"].as_str() != Some(frame.as_str()) && found["backendNodeId"].as_i64() != Some(*owner) {
                return Err(("stale_target".to_owned(), "covered by something on the page over its frame: read the page again".to_owned()));
            }
        }
        let local = (at.0 - spot.offset.0, at.1 - spot.offset.1);
        self.op_at(spot.place, json!({"op": "hits", "target": target, "x": local.0, "y": local.1})).await.map(drop)
    }

    /// The page's viewport, CSS pixels.
    async fn view(&self) -> Result<Point, Error> {
        let metrics = self.call("Page.getLayoutMetrics", json!({})).await?;
        let viewport = &metrics["cssVisualViewport"];
        let size = |key: &str| viewport[key].as_f64().filter(|n| n.is_finite() && (1.0..1e6).contains(n));
        match (size("clientWidth"), size("clientHeight")) {
            (Some(width), Some(height)) => Ok((width, height)),
            _ => Err(Error::Refused("Page.getLayoutMetrics".into(), "no viewport".into())),
        }
    }

    /// The content box of the element `owner` (a frame's) in the page's
    /// viewport, if it's shown.
    async fn owner_box(&self, owner: i64) -> Result<Option<Rect>, Error> {
        let Ok(model) = self.call("DOM.getBoxModel", json!({"backendNodeId": owner})).await else { return Ok(None) };
        let quad: Vec<f64> = model["model"]["content"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
        if quad.len() != 8 || quad.iter().any(|n| !n.is_finite() || n.abs() > 1e7) {
            return Ok(None);
        }
        let xs = [quad[0], quad[2], quad[4], quad[6]];
        let ys = [quad[1], quad[3], quad[5], quad[7]];
        let (left, top) = (xs.iter().copied().fold(f64::MAX, f64::min), ys.iter().copied().fold(f64::MAX, f64::min));
        let (right, bottom) = (xs.iter().copied().fold(f64::MIN, f64::max), ys.iter().copied().fold(f64::MIN, f64::max));
        Ok((right - left >= 2.0 && bottom - top >= 2.0).then_some((left, top, right - left, bottom - top)))
    }

    /// The document `frame` shows now: its loader, on `session` (its own
    /// process) or in the page's frame tree.
    async fn loader_of(&self, session: Option<&str>, frame: &str) -> Option<String> {
        let tree = self.call_on(session, "Page.getFrameTree", json!({})).await.ok()?;
        let root = &tree["frameTree"];
        let found = if session.is_some() {
            Some(&root["frame"])
        } else {
            root["childFrames"].as_array().into_iter().flatten().map(|child| &child["frame"]).find(|child| child["id"] == frame)
        };
        found.and_then(|frame| frame["loaderId"].as_str()).filter(|id| id.len() <= 128).map(str::to_owned)
    }

    /// The page's own frames: those in its process from its frame tree,
    /// and those in processes of their own from Chrome's targets; each with
    /// the element that holds it.
    async fn listed(&self) -> Result<Vec<Listed>, Error> {
        let tree = self.call("Page.getFrameTree", json!({})).await?;
        let top = tree["frameTree"]["frame"]["id"].as_str().unwrap_or_default().to_owned();
        let text = |value: &Value, max: usize| value.as_str().filter(|s| s.len() <= max).map(str::to_owned);
        let mut found = Vec::new();
        for child in tree["frameTree"]["childFrames"].as_array().into_iter().flatten().take(FRAMES_MOST * 4) {
            let frame = &child["frame"];
            let (Some(id), Some(url), Some(loader)) = (text(&frame["id"], 128), text(&frame["url"], 8192), text(&frame["loaderId"], 128)) else { continue };
            // about:blank and srcdoc frames are their maker's: Chrome's
            // origin for them says whose.
            let url = if url.starts_with("http") { url } else { text(&frame["securityOrigin"], 512).unwrap_or(url) };
            found.push((id, url, Some(loader), false));
        }
        let targets = self.cdp.call("Target.getTargets", json!({}), None).await?;
        for target in targets["targetInfos"].as_array().into_iter().flatten() {
            if target["type"] != "iframe" || target["parentFrameId"] != top.as_str() || found.len() >= FRAMES_MOST * 4 {
                continue;
            }
            let (Some(id), Some(url)) = (text(&target["targetId"], 128), text(&target["url"], 8192)) else { continue };
            found.push((id, url, None, true));
        }
        let mut listed = Vec::new();
        for (id, url, loader, own_process) in found {
            let Ok(owner) = self.call("DOM.getFrameOwner", json!({"frameId": id})).await else { continue };
            let Some(owner) = owner["backendNodeId"].as_i64() else { continue };
            listed.push(Listed { id, url, loader, own_process, owner });
        }
        Ok(listed)
    }

    /// The reader's world in `frame` (on `session`, or the page's): its
    /// context.
    async fn frame_world(&self, session: Option<&str>, frame: &str) -> Result<i64, Error> {
        let made = self
            .call_on(session, "Page.createIsolatedWorld", json!({"frameId": frame, "worldName": super::WORLD, "grantUniveralAccess": false}))
            .await?;
        let context = made["executionContextId"].as_i64().ok_or_else(|| Error::Refused("Page.createIsolatedWorld".into(), "no context".into()))?;
        let injected = self.call_on(session, "Runtime.evaluate", json!({"expression": super::source(), "contextId": context, "returnByValue": true})).await?;
        if injected.get("exceptionDetails").is_some() {
            return Err(Error::Refused("Runtime.evaluate".into(), "the reader didn't start in this frame".into()));
        }
        Ok(context)
    }

    /// The place to read `listed` in: attached to it first when it's in a
    /// process of its own. With the document it shows.
    async fn enter(&self, listed: &Listed) -> Result<(Place, String), Error> {
        let session = if listed.own_process {
            let attached = self.cdp.call("Target.attachToTarget", json!({"targetId": listed.id, "flatten": true}), None).await?;
            let session = attached["sessionId"].as_str().filter(|id| id.len() <= 128).unwrap_or_default().to_owned();
            crate::locking::lock(&self.frame_sessions).push(session.clone());
            Some(session)
        } else {
            None
        };
        let loader = match &listed.loader {
            Some(loader) => loader.clone(),
            None => self.loader_of(session.as_deref(), &listed.id).await.ok_or_else(|| Error::Refused("Page.getFrameTree".into(), "no frame".into()))?,
        };
        // A frame in the page's process keeps its world while it shows the
        // same document, as the page does.
        let key = (listed.id.clone(), loader.clone());
        let kept = if session.is_none() { crate::locking::lock(&self.frame_worlds).get(&key).copied() } else { None };
        let context = match kept {
            Some(context) => context,
            None => {
                let context = self.frame_world(session.as_deref(), &listed.id).await?;
                if session.is_none() {
                    let mut worlds = crate::locking::lock(&self.frame_worlds);
                    if worlds.len() >= FRAMES_MOST * 4 {
                        worlds.clear();
                    }
                    worlds.insert(key, context);
                }
                context
            }
        };
        Ok((Place { session, context, frame: Some((listed.id.clone(), listed.owner)) }, loader))
    }

    /// Lets go of what the last read held in frames.
    async fn forget_frames(&self) {
        crate::locking::lock(&self.frame_refs).clear();
        let sessions = std::mem::take(&mut *crate::locking::lock(&self.frame_sessions));
        for session in sessions {
            let _ = self.cdp.call("Target.detachFromTarget", json!({"sessionId": session}), None).await;
        }
    }

    /// Adds the page's frames to `page` as it was read: shown ones, at
    /// most [`FRAMES_MOST`]. A frame that covers the page is an overlay of
    /// the page's too.
    pub(super) async fn read_frames(&self, page: &mut schema::Page) {
        self.forget_frames().await;
        let Ok(top) = self.frame().await else { return };
        let Ok(listed) = self.listed().await else { return };
        let Ok(view) = self.view().await else { return };
        let blocked = self.errand_op(json!({"op": "covers"})).await.is_ok_and(|covers| covers["scroll_blocked"] == true);
        let mut more = 0;
        for listed in listed {
            let Ok(Some(rect)) = self.owner_box(listed.owner).await else { continue };
            let shown = (rect.0 + rect.2).min(view.0) - rect.0.max(0.0);
            let tall = (rect.1 + rect.3).min(view.1) - rect.1.max(0.0);
            if shown <= 0.0 || tall <= 0.0 {
                continue;
            }
            if page.frames.len() == FRAMES_MOST {
                more += 1;
                continue;
            }
            let covers = shown * tall / (view.0 * view.1);
            let mut frame = schema::Frame {
                origin: origin_of(&listed.url),
                same_site: same_site(&top.url, &listed.url),
                left: rect.0.round() as i64,
                top: rect.1.round() as i64,
                width: rect.2.round() as i64,
                height: rect.3.round() as i64,
                title: None,
                text: None,
                elements: Vec::new(),
                overlay: None,
                not_read: None,
            };
            if super::untouched(&listed.url) {
                frame.not_read = Some(UNTOUCHED_WHY.to_owned());
            } else if frame.same_site {
                match self.read_frame(&listed, &page.document).await {
                    Ok(read) => {
                        frame.title = Some(read.title);
                        frame.text = Some(read.text);
                        frame.elements = read.elements;
                    }
                    Err(_) => frame.not_read = Some("it couldn't be read".to_owned()),
                }
            } else if covers >= COVERS_MOST || (blocked && covers >= COVERS_BLOCKED) {
                // Whether it's about cookies, from inside, never said.
                let kind = match self.frame_covering(&listed).await {
                    Ok((covering, _)) => covering.kind,
                    Err(_) => "pop-up",
                };
                frame.overlay = Some(kind.to_owned());
                if !page.overlays.iter().any(|known| known == kind) {
                    page.overlays.push(kind.to_owned());
                }
            }
            page.frames.push(frame);
        }
        if more > 0 {
            page.unsupported.push(format!("{more} more frames not listed"));
        }
    }

    /// A frame of the page's site, read as the page is.
    async fn read_frame(&self, listed: &Listed, document: &str) -> Result<schema::Page, (String, String)> {
        let (place, loader) = self.enter(listed).await.map_err(gone)?;
        let json = self.ask_on(place.session.as_deref(), place.context, json!({"op": "read", "limit": FRAME_TEXT, "budget": FRAME_BUDGET})).await?;
        let mut value: Value = serde_json::from_str(&json).map_err(|_| unreadable())?;
        if value.get("error").is_none() {
            value["document"] = Value::String(document.to_owned());
        }
        let Answer::Page(mut read) = schema::answer(&Op::Read { limit: FRAME_TEXT }, &value.to_string()).map_err(|_| unreadable())? else {
            return Err(unreadable());
        };
        let origin = origin_of(&listed.url);
        mark_sensitive(&mut read.elements, &origin, &self.two_step());
        let mut refs = crate::locking::lock(&self.frame_refs);
        for element in &read.elements {
            refs.insert(element.reference.clone(), FrameRef { place: place.clone(), loader: loader.clone(), origin: origin.clone() });
        }
        Ok(*read)
    }

    /// What a frame of another site that covers the page offers: its
    /// buttons (their names only), as a cookie notice when it's about
    /// cookies, else a pop-up; and where they're pressed.
    async fn frame_covering(&self, listed: &Listed) -> Result<(Covering, Place), (String, String)> {
        let (place, loader) = self.enter(listed).await.map_err(gone)?;
        Ok((self.offer(&place, loader, origin_of(&listed.url)).await?, place))
    }

    /// The buttons a frame at `place` offers, showing the document
    /// `loader` of `origin`; their refs kept to press them.
    async fn offer(&self, place: &Place, loader: String, origin: String) -> Result<Covering, (String, String)> {
        let said = self.op_at(place, json!({"op": "frame_buttons"})).await?;
        let kind = if said["cookies"] == true { "cookie notice" } else { "pop-up" };
        let covering = Covering { kind, personal: said["personal"] != false, buttons: Covering::buttons(&said)?, switches: Covering::switches(&said)? };
        let mut refs = crate::locking::lock(&self.frame_refs);
        let switches = covering.switches.iter().filter_map(|switch| switch.reference.as_ref());
        for reference in covering.buttons.iter().map(|button| &button.reference).chain(switches) {
            refs.insert(reference.clone(), FrameRef { place: place.clone(), loader: loader.clone(), origin: origin.clone() });
        }
        Ok(covering)
    }

    /// The frame of another site that covers the page most, for a dismiss:
    /// what it offers and where, or why it's not Claktory's to touch.
    pub(super) async fn covering_frame(&self) -> Result<Option<(Covering, Place)>, (String, String)> {
        let top = self.frame().await.map_err(gone)?;
        let view = self.view().await.map_err(gone)?;
        let blocked = self.errand_op(json!({"op": "covers"})).await?["scroll_blocked"] == true;
        let mut best: Option<(f64, Listed)> = None;
        for listed in self.listed().await.map_err(gone)? {
            if same_site(&top.url, &listed.url) {
                continue;
            }
            let Ok(Some(rect)) = self.owner_box(listed.owner).await else { continue };
            let covers = ((rect.0 + rect.2).min(view.0) - rect.0.max(0.0)).max(0.0) * ((rect.1 + rect.3).min(view.1) - rect.1.max(0.0)).max(0.0)
                / (view.0 * view.1);
            if (covers >= COVERS_MOST || (blocked && covers >= COVERS_BLOCKED)) && best.as_ref().is_none_or(|(most, _)| covers > *most) {
                best = Some((covers, listed));
            }
        }
        let Some((_, listed)) = best else { return Ok(None) };
        if super::untouched(&listed.url) {
            return Err(("sensitive_action".to_owned(), UNTOUCHED_WHY.to_owned()));
        }
        self.frame_covering(&listed).await.map(Some)
    }

    /// What covers the page now where `place` is: the frame's offer, or
    /// the page's overlays.
    pub(super) async fn covering_at(&self, place: Option<&Place>) -> Result<Vec<Covering>, (String, String)> {
        match place {
            Some(place @ Place { frame: Some((frame, _)), session, .. }) => {
                // Gone, or showing another document: nothing of it covers.
                let Some(loader) = self.loader_of(session.as_deref(), frame).await else { return Ok(Vec::new()) };
                let origin = crate::locking::lock(&self.frame_refs).values().find(|found| found.place.frame == place.frame).map(|found| found.origin.clone()).unwrap_or_default();
                Ok(self.offer(place, loader, origin).await.map(|covering| vec![covering]).unwrap_or_default())
            }
            _ => Covering::read(&self.errand_op(json!({"op": "covers"})).await?),
        }
    }
}
