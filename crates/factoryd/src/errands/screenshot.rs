//! What the agent's tab looks like, for its own model: browser_screenshot
//! takes the page as a person sees it (its viewport, in CSS pixels, with
//! every password, code and card field blacked out, and every other site's
//! frame but a check's), and browser_act's click_at presses a point of that
//! picture: only inside a check's frame, where a check that the visitor is
//! a person draws its box or puzzle.

use std::io::Cursor;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{Value, json};

use super::pace::{self, Point};
use super::reader::{Hand, Page};

/// The viewport's size in CSS pixels and its device pixel ratio.
const VIEW: &str = "[innerWidth, innerHeight, devicePixelRatio || 1]";

/// Where the fields that hold a secret are, in the viewport's CSS pixels:
/// password fields, fields for a one-time code and card fields, through
/// open shadow roots and frames of the page's own origin (offset by where
/// each frame sits). Blacked out of the picture whatever they show.
const SECRET_FIELDS: &str = r#"(() => {
  const boxes = [];
  const secretWords = ['current-password', 'new-password', 'one-time-code', 'cc-number', 'cc-csc', 'cc-exp', 'cc-exp-month', 'cc-exp-year'];
  const visit = (root, dx, dy, depth) => {
    for (const field of root.querySelectorAll('input')) {
      const tokens = (field.getAttribute('autocomplete') || '').toLowerCase().split(/\s+/);
      const secret = (field.type || '').toLowerCase() === 'password' || tokens.some((token) => secretWords.includes(token));
      if (!secret) continue;
      const box = field.getBoundingClientRect();
      if (box.width >= 1 && box.height >= 1) boxes.push([box.left + dx, box.top + dy, box.width, box.height]);
    }
    for (const host of root.querySelectorAll('*')) if (host.shadowRoot) visit(host.shadowRoot, dx, dy, depth);
    if (depth > 4) return;
    for (const frame of root.querySelectorAll('iframe, frame')) {
      let inner = null;
      try { inner = frame.contentDocument; } catch (e) { inner = null; }
      if (!inner) continue;
      const box = frame.getBoundingClientRect();
      visit(inner, dx + box.left + frame.clientLeft, dy + box.top + frame.clientTop, depth + 1);
    }
  };
  visit(document, 0, 0, 0);
  return boxes.slice(0, 400);
})()"#;

/// Where the element is in the viewport, if it's still on the page.
const PLACE: &str = r#"function () {
  if (!this.isConnected) return null;
  const box = this.getBoundingClientRect();
  return [box.left, box.top, box.width, box.height];
}"#;

/// The hosts whose frames are checks that the visitor is a person, as
/// Chrome says the frame's address is: Cloudflare's Turnstile, reCAPTCHA,
/// hCaptcha, Arkose, DataDome and HUMAN. The only frames click_at presses
/// into, and the only other sites' frames a screenshot shows.
pub(crate) fn check_frame(address: &str) -> bool {
    let Ok(url) = url::Url::parse(address) else { return false };
    if url.scheme() != "https" {
        return false;
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let under = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
    match host.as_str() {
        "challenges.cloudflare.com" => true,
        "www.google.com" | "www.recaptcha.net" | "recaptcha.net" => url.path().starts_with("/recaptcha/"),
        _ => {
            under("hcaptcha.com")
                || under("arkoselabs.com")
                || under("funcaptcha.com")
                || under("captcha-delivery.com")
                || under("px-cloud.net")
                || under("px-cdn.net")
                || under("perimeterx.net")
        }
    }
}

/// A frame of the page below its top one, as Chrome lists it.
struct Listed {
    id: String,
    address: String,
    /// Its document's origin as Chrome holds it: the parent's for a
    /// same-origin `srcdoc` or `about:blank` frame, opaque for a sandboxed
    /// one.
    origin: String,
    /// Whether the page's own frame tree has it (else it's a target of its
    /// own, inside another site's frame).
    in_tree: bool,
}

fn origin_of(address: &str) -> String {
    url::Url::parse(address).map(|url| url.origin().ascii_serialization()).unwrap_or_default()
}

async fn frames(page: &Page) -> Vec<Listed> {
    fn walk(node: &Value, found: &mut Vec<Listed>) {
        for child in node["childFrames"].as_array().into_iter().flatten() {
            let frame = &child["frame"];
            if let (Some(id), Some(url)) = (frame["id"].as_str(), frame["url"].as_str()) {
                let origin = frame["securityOrigin"].as_str().map_or_else(|| origin_of(url), str::to_owned);
                // Chrome lists a frame's fragment apart.
                let address = match frame["urlFragment"].as_str().filter(|fragment| fragment.starts_with('#')) {
                    Some(fragment) => format!("{url}{fragment}"),
                    None => url.to_owned(),
                };
                found.push(Listed { id: id.to_owned(), address, origin, in_tree: true });
            }
            walk(child, found);
        }
    }
    let mut found = Vec::new();
    if let Ok(tree) = page.call("Page.getFrameTree", json!({})).await {
        walk(&tree["frameTree"], &mut found);
    }
    // Chrome lists no origin for a srcdoc frame: its document says, in a
    // world the page's scripts can't reach.
    for listed in found.iter_mut().filter(|listed| listed.address.starts_with("about:")) {
        listed.origin = document_origin(page, &listed.id).await.unwrap_or_default();
    }
    for Target { id, address, .. } in iframe_targets(&OnPage(page)).await {
        if !found.iter().any(|known| known.id == id) {
            let origin = origin_of(&address);
            found.push(Listed { id, address, origin, in_tree: false });
        }
    }
    found.truncate(200);
    found
}

/// A frame of the browser in a process of its own.
struct Target {
    id: String,
    address: String,
    /// The frame it's in.
    parent: String,
}

/// The origin of the document frame `frame` (in the page's process)
/// shows, as that document has it.
async fn document_origin(page: &Page, frame: &str) -> Option<String> {
    let made = page.call("Page.createIsolatedWorld", json!({"frameId": frame, "worldName": ORIGIN_WORLD, "grantUniveralAccess": false})).await.ok()?;
    let context = made["executionContextId"].as_i64()?;
    let said = page.call("Runtime.evaluate", json!({"expression": "self.origin", "contextId": context, "returnByValue": true})).await.ok()?;
    said["result"]["value"].as_str().map(str::to_owned)
}

const ORIGIN_WORLD: &str = "claktory-origin";

/// Every frame target of the browser.
async fn iframe_targets(calls: &impl FrameCalls) -> Vec<Target> {
    let Some(targets) = calls.call("", "Target.getTargets", json!({})).await else { return Vec::new() };
    targets["targetInfos"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|target| target["type"] == "iframe")
        .filter_map(|target| {
            Some(Target {
                id: target["targetId"].as_str()?.to_owned(),
                address: target["url"].as_str()?.to_owned(),
                parent: target["parentFrameId"].as_str().unwrap_or_default().to_owned(),
            })
        })
        .collect()
}

/// Where a frame is, as Chrome lays out the element that holds it.
enum Placed {
    At((f64, f64, f64, f64)),
    /// Its element isn't laid out: nothing of it shows.
    NotShown,
    /// No element of the page holds it.
    Unknown,
}

/// Where the element that holds frame `id` is, in the viewport: its
/// border box.
async fn frame_box(page: &Page, id: &str) -> Placed {
    let Some(node) = page.call("DOM.getFrameOwner", json!({"frameId": id})).await.ok().and_then(|owner| owner["backendNodeId"].as_i64()) else {
        return Placed::Unknown;
    };
    let Ok(model) = page.call("DOM.getBoxModel", json!({"backendNodeId": node})).await else { return Placed::NotShown };
    let quad: Vec<f64> = model["model"]["border"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
    if quad.len() != 8 {
        return Placed::Unknown;
    }
    let xs = [quad[0], quad[2], quad[4], quad[6]];
    let ys = [quad[1], quad[3], quad[5], quad[7]];
    let (left, right) = (xs.iter().copied().fold(f64::INFINITY, f64::min), xs.iter().copied().fold(f64::NEG_INFINITY, f64::max));
    let (top, bottom) = (ys.iter().copied().fold(f64::INFINITY, f64::min), ys.iter().copied().fold(f64::NEG_INFINITY, f64::max));
    if left.is_finite() && top.is_finite() { Placed::At((left, top, right - left, bottom - top)) } else { Placed::Unknown }
}

/// What to black out now: the secret fields the page's own documents show,
/// the fields Claktory filled from the vault on this page (whatever they
/// show now), and every frame of another site that isn't a check's, whose
/// fields Claktory can't see into.
async fn secrets(page: &Page) -> Result<Vec<(f64, f64, f64, f64)>, (String, String)> {
    let mut boxes = Vec::new();
    let number = |found: &Value, at: usize| found[at].as_f64().filter(|n| n.is_finite()).unwrap_or(0.0);
    for found in page.in_world(SECRET_FIELDS).await.unwrap_or(Value::Null).as_array().into_iter().flatten() {
        boxes.push((number(found, 0), number(found, 1), number(found, 2), number(found, 3)));
    }
    for object in page.filled() {
        let placed = page.call("Runtime.callFunctionOn", json!({"objectId": object, "functionDeclaration": PLACE, "returnByValue": true})).await;
        if let Ok(placed) = placed
            && placed["result"]["value"].is_array()
        {
            let found = &placed["result"]["value"];
            boxes.push((number(found, 0), number(found, 1), number(found, 2), number(found, 3)));
        }
    }
    let (top, own) = page.frame().await.map(|frame| (frame.id, frame.origin)).unwrap_or_default();
    let listed = frames(page).await;
    let parents = parents(page).await;
    for Listed { id, address, origin, in_tree } in &listed {
        let (id, in_tree) = (id.as_str(), *in_tree);
        if *origin == own || check_frame(address) {
            continue;
        }
        match frame_box(page, id).await {
            // Covered but where a check inside it shows.
            Placed::At(found) => {
                let mut covered = vec![found];
                for Listed { id: inner, address: inner_address, .. } in &listed {
                    if !check_frame(inner_address) || !inside(&parents, inner, id) {
                        continue;
                    }
                    if let Some(hole) = placed_deep(page, &parents, &top, inner).await {
                        covered = covered.into_iter().flat_map(|rect| without(rect, hole)).collect();
                    }
                }
                boxes.extend(covered);
            }
            Placed::NotShown => {}
            // A frame inside another site's frame, covered with it.
            Placed::Unknown if !in_tree => {}
            // One of the page's own that can't be placed can't be
            // covered: no picture.
            Placed::Unknown => return Err(failed("the page has a frame Claktory can't place: take the screenshot again")),
        }
    }
    Ok(boxes)
}

/// Each frame of the page and its parent's id: those of its own tree, and
/// those in processes of their own.
async fn parents(page: &Page) -> std::collections::HashMap<String, String> {
    fn walk(node: &Value, found: &mut std::collections::HashMap<String, String>) {
        let parent = node["frame"]["id"].as_str().unwrap_or_default();
        for child in node["childFrames"].as_array().into_iter().flatten() {
            if let Some(id) = child["frame"]["id"].as_str() {
                found.insert(id.to_owned(), parent.to_owned());
            }
            walk(child, found);
        }
    }
    let mut found = std::collections::HashMap::new();
    if let Ok(tree) = page.call("Page.getFrameTree", json!({})).await {
        walk(&tree["frameTree"], &mut found);
    }
    for target in iframe_targets(&OnPage(page)).await {
        if !target.parent.is_empty() {
            found.entry(target.id).or_insert(target.parent);
        }
    }
    found
}

/// Where frame `frame` shows in the viewport (its element's border box),
/// down through the frames it's in, other processes' too: None where
/// Chrome can't say.
async fn placed_deep(page: &Page, parents: &std::collections::HashMap<String, String>, top: &str, frame: &str) -> Option<(f64, f64, f64, f64)> {
    let mut chain = vec![frame.to_owned()];
    while let Some(parent) = parents.get(chain.last()?) {
        if parent == top || chain.len() > DEEPEST {
            break;
        }
        chain.push(parent.clone());
    }
    if parents.get(chain.last()?).map(String::as_str) != Some(top) {
        return None;
    }
    chain.reverse();
    let calls = OnPage(page);
    let targets: Vec<String> = iframe_targets(&calls).await.into_iter().map(|target| target.id).collect();
    let mut session = page.session().to_owned();
    let mut offset = (0.0, 0.0);
    let mut attached = Vec::new();
    let mut placed = None;
    for (at, step) in chain.iter().enumerate() {
        let owner = calls.call(&session, "DOM.getFrameOwner", json!({"frameId": step})).await.and_then(|owner| owner["backendNodeId"].as_i64());
        let Some(model) = (match owner {
            Some(owner) => calls.call(&session, "DOM.getBoxModel", json!({"backendNodeId": owner})).await,
            None => None,
        }) else {
            break;
        };
        let quad = |which: &str| -> Option<[f64; 4]> {
            let quad: Vec<f64> = model["model"][which].as_array()?.iter().filter_map(Value::as_f64).collect();
            (quad.len() == 8).then(|| {
                let xs = [quad[0], quad[2], quad[4], quad[6]];
                let ys = [quad[1], quad[3], quad[5], quad[7]];
                [xs.iter().copied().fold(f64::INFINITY, f64::min), ys.iter().copied().fold(f64::INFINITY, f64::min), xs.iter().copied().fold(f64::NEG_INFINITY, f64::max), ys.iter().copied().fold(f64::NEG_INFINITY, f64::max)]
            })
        };
        if at + 1 == chain.len() {
            placed = quad("border").map(|[left, upper, right, lower]| (left + offset.0, upper + offset.1, right - left, lower - upper));
            break;
        }
        if !targets.contains(step) {
            continue;
        }
        let Some([left, upper, _, _]) = quad("content") else { break };
        let Some(inner) = calls.attach(step).await else { break };
        attached.push(inner.clone());
        offset = (offset.0 + left, offset.1 + upper);
        session = inner;
    }
    for session in &attached {
        calls.detach(session).await;
    }
    placed.filter(|placed| placed.0.is_finite() && placed.1.is_finite())
}

/// Whether frame `frame` is inside frame `outer`, at any depth.
fn inside(parents: &std::collections::HashMap<String, String>, frame: &str, outer: &str) -> bool {
    let mut at = frame;
    for _ in 0..parents.len() {
        match parents.get(at) {
            Some(parent) if parent == outer => return true,
            Some(parent) => at = parent,
            None => return false,
        }
    }
    false
}

/// `rect` (left, top, width, height) without `hole`: up to four pieces.
fn without(rect: (f64, f64, f64, f64), hole: (f64, f64, f64, f64)) -> Vec<(f64, f64, f64, f64)> {
    let (left, top, right, bottom) = (rect.0, rect.1, rect.0 + rect.2, rect.1 + rect.3);
    let (hole_left, hole_top) = (hole.0.max(left), hole.1.max(top));
    let (hole_right, hole_bottom) = ((hole.0 + hole.2).min(right), (hole.1 + hole.3).min(bottom));
    if hole_left >= hole_right || hole_top >= hole_bottom {
        return vec![rect];
    }
    [
        (left, top, right - left, hole_top - top),
        (left, hole_bottom, right - left, bottom - hole_bottom),
        (left, hole_top, hole_left - left, hole_bottom - hole_top),
        (hole_right, hole_top, right - hole_right, hole_bottom - hole_top),
    ]
    .into_iter()
    .filter(|piece| piece.2 > 0.0 && piece.3 > 0.0)
    .collect()
}

/// The most a picture may be wide or high, in pixels.
const LONGEST: u32 = 2048;

/// The picture's JPEG quality.
const QUALITY: u8 = 80;

/// A picture of the tab: JPEG bytes, as base64, and its size in pixels
/// (the viewport's CSS pixels: what click_at takes).
pub(crate) struct Picture {
    pub data: String,
    pub width: u32,
    pub height: u32,
}

fn failed(why: impl Into<String>) -> (String, String) {
    ("interrupted".to_owned(), why.into())
}

/// The viewport's size and the device pixel ratio.
async fn view(page: &Page) -> Result<(f64, f64, f64), (String, String)> {
    let said = page.in_world(VIEW).await.ok_or_else(|| failed("the page didn't answer"))?;
    let number = |at: usize| said[at].as_f64().filter(|n| n.is_finite() && *n > 0.0 && *n < 100_000.0);
    match (number(0), number(1), number(2)) {
        (Some(width), Some(height), Some(ratio)) => Ok((width, height, ratio)),
        _ => Err(failed("the page's size can't be read")),
    }
}

/// The tab as a person sees it now.
pub(crate) async fn take(page: &Page) -> Result<Picture, (String, String)> {
    let (width, height, _) = view(page).await?;
    // Where they are before and after the picture: one that moved meanwhile
    // is covered where it was and where it went.
    let mut boxes = secrets(page).await?;
    let shot = match page.call("Page.captureScreenshot", json!({"format": "png", "captureBeyondViewport": false, "fromSurface": true})).await {
        Ok(shot) => shot["data"].as_str().map(str::to_owned),
        // A runner that doesn't forward it: one frame of a screencast.
        Err(super::cdp::Error::Refused(..)) => one_frame(page).await,
        Err(error) => return Err(failed(crate::browser::schema::one_line(&error.to_string(), 200))),
    };
    let png = shot.and_then(|data| base64::engine::general_purpose::STANDARD.decode(data).ok()).ok_or_else(|| failed("no picture came"))?;
    let picture = image::load_from_memory(&png).map_err(|_| failed("the picture can't be read"))?;
    boxes.extend(secrets(page).await?);
    // In CSS pixels, so a point of it is a point click_at takes.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (css_width, css_height) = ((width.round() as u32).clamp(1, LONGEST), (height.round() as u32).clamp(1, LONGEST));
    let mut picture = if (picture.width(), picture.height()) == (css_width, css_height) {
        picture.to_rgb8()
    } else {
        image::imageops::resize(&picture.to_rgb8(), css_width, css_height, image::imageops::FilterType::Triangle)
    };
    for found in boxes {
        black_out(&mut picture, found);
    }
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(Cursor::new(&mut jpeg), QUALITY)
        .encode_image(&picture)
        .map_err(|_| failed("the picture couldn't be made"))?;
    Ok(Picture { data: base64::engine::general_purpose::STANDARD.encode(&jpeg), width: css_width, height: css_height })
}

/// The first frame a screencast of the page sends, as base64.
async fn one_frame(page: &Page) -> Option<String> {
    let mut events = page.cdp().events();
    page.call("Page.startScreencast", json!({"format": "png", "everyNthFrame": 1})).await.ok()?;
    let frame = tokio::time::timeout(Duration::from_secs(5), async {
        while let Ok(event) = events.recv().await {
            if event.method == "Page.screencastFrame" && event.session.as_deref() == Some(page.session()) {
                let _ = page.call("Page.screencastFrameAck", json!({"sessionId": event.params["sessionId"]})).await;
                return event.params["data"].as_str().map(str::to_owned);
            }
        }
        None
    })
    .await
    .ok()
    .flatten();
    let _ = page.call("Page.stopScreencast", json!({})).await;
    frame
}

/// Paints `rect` (left, top, width, height) black, with a pixel's margin.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn black_out(picture: &mut image::RgbImage, rect: (f64, f64, f64, f64)) {
    let (width, height) = (f64::from(picture.width()), f64::from(picture.height()));
    let left = (rect.0 - 1.0).clamp(0.0, width) as u32;
    let top = (rect.1 - 1.0).clamp(0.0, height) as u32;
    let right = (rect.0 + rect.2 + 1.0).clamp(0.0, width).ceil() as u32;
    let bottom = (rect.1 + rect.3 + 1.0).clamp(0.0, height).ceil() as u32;
    for y in top..bottom {
        for x in left..right {
            picture.put_pixel(x, y, image::Rgb([0, 0, 0]));
        }
    }
}

/// The longest click_at holds the button down: a check that asks to press
/// and hold takes up to about ten seconds.
pub(crate) const HOLD_MAX: f64 = 15.0;

/// Why a point outside a check's frame isn't clicked.
const PAGES_OWN: &str = "that point isn't inside a check that you're a person (a Cloudflare, reCAPTCHA, hCaptcha, Arkose, DataDome or HUMAN frame): click the page's elements with op click and the ref browser_read gave";

/// browser_act's click_at: the mouse goes to (`x`, `y`) of the viewport (a
/// browser_screenshot's pixels) at a person's pace and presses it, held
/// `hold` seconds, or, given `to`, drags it there with the button down and
/// lets go (a slider, a piece to put in place). Only inside a check's frame,
/// both ends: the page's own controls go by ref, under the reader's rules.
pub(crate) async fn click_at(page: &Page, hand: &mut Hand<'_>, x: f64, y: f64, hold: f64, to: Option<Point>) -> Result<(), (String, String)> {
    let (width, height, _) = view(page).await?;
    let inside = |(x, y): Point| x.is_finite() && y.is_finite() && (0.0..width).contains(&x) && (0.0..height).contains(&y);
    if !inside((x, y)) || to.is_some_and(|to| !inside(to)) {
        return Err(("invalid_arguments".to_owned(), format!("x and y are a point of the screenshot: 0 to {} across, 0 to {} down", width.floor(), height.floor())));
    }
    let at = pace::pixel((x, y), (width, height));
    let to = to.map(|to| pace::pixel(to, (width, height)));
    let Some(frame) = check_frame_at(page, at).await? else {
        return Err(("unsupported_target".to_owned(), PAGES_OWN.to_owned()));
    };
    if to.is_some_and(|to| to == at) {
        return Err(("unsupported_target".to_owned(), PAGES_OWN.to_owned()));
    }
    if let Some(to) = to
        && check_frame_at(page, to).await?.as_ref() != Some(&frame)
    {
        return Err(("unsupported_target".to_owned(), format!("a drag ends inside the same check's frame: {PAGES_OWN}")));
    }
    page.move_to(hand, at, (width, height), 24.0).await?;
    let pause = pace::hover(hand.rng);
    hand.wait_until(std::time::Instant::now() + pause).await?;
    // Checked again right before the press: the page may have moved.
    if check_frame_at(page, at).await?.as_ref() != Some(&frame) {
        return Err(("stale_target".to_owned(), "the check's frame moved: take another screenshot".to_owned()));
    }
    let gone = |error: super::cdp::Error| failed(crate::browser::schema::one_line(&error.to_string(), 200));
    page.mouse("mousePressed", at).await.map_err(gone)?;
    let moved = match to {
        Some(to) => drag(page, hand, at, to).await,
        None => {
            let held = if hold > 0.0 { std::time::Duration::from_secs_f64(hold.min(HOLD_MAX)) } else { pace::hold(hand.rng) };
            hand.wait_until(std::time::Instant::now() + held).await
        }
    };
    let end = if moved.is_ok() { to.unwrap_or(at) } else { at };
    page.mouse("mouseReleased", end).await.map_err(gone)?;
    moved
}

/// The mouse, its button down, from `from` to `to` over a person's time:
/// eased, a move about every 16 ms, a moment's rest at each end.
async fn drag(page: &Page, hand: &mut Hand<'_>, from: Point, to: Point) -> Result<(), (String, String)> {
    let rest = pace::hold(hand.rng);
    hand.wait_until(std::time::Instant::now() + rest).await?;
    let took = pace::travel(hand.rng, (to.0 - from.0).hypot(to.1 - from.1), 24.0) * 2;
    let steps = (took.as_millis() / 16).clamp(8, 200) as u32;
    let start = std::time::Instant::now();
    for step in 1..=steps {
        let t = f64::from(step) / f64::from(steps);
        let eased = t * t * (3.0 - 2.0 * t);
        let wobble = if step < steps { hand.rng.between(-0.8, 0.8) } else { 0.0 };
        let at = ((from.0 + (to.0 - from.0) * eased + wobble).round(), (from.1 + (to.1 - from.1) * eased + wobble).round());
        let moved = json!({"type": "mouseMoved", "x": at.0, "y": at.1, "button": "left", "buttons": 1, "pointerType": "mouse"});
        page.call("Input.dispatchMouseEvent", moved).await.map_err(|error| failed(crate::browser::schema::one_line(&error.to_string(), 200)))?;
        hand.wait_until(start + took * step / steps).await?;
    }
    *hand.pointer = Some(to);
    let rest = pace::hold(hand.rng);
    hand.wait_until(std::time::Instant::now() + rest).await
}

/// Whether the node at `at` is in a check's frame, or is one (Chrome's
/// answer, through shadow roots, and the frame's address as Chrome lists
/// it, never as the page says).
/// The id of the check's frame at `at`, if the innermost frame Chrome finds
/// there is a check's: frames on the way to it can be anyone's (a site's
/// own challenge frame holding Google's box), never the one pressed.
async fn check_frame_at(page: &Page, at: Point) -> Result<Option<String>, (String, String)> {
    let Some((frame, address)) = innermost_frame_at(page, at).await? else { return Ok(None) };
    Ok(check_frame(&address).then_some(frame))
}

/// The innermost frame Chrome hit-tests at `at`, a point of the viewport,
/// as `innermost_frame` finds it on the errand's page: its id and address.
async fn innermost_frame_at(page: &Page, at: Point) -> Result<Option<(String, String)>, (String, String)> {
    let top = page.frame().await.map_err(|error| failed(crate::browser::schema::one_line(&error.to_string(), 200)))?.id;
    let found = innermost_frame(&OnPage(page), page.session(), &top, at).await;
    Ok(found.map(|found| (found.frame, found.address)))
}

/// The CDP calls a walk down a page's frames makes, on whatever carries
/// them: the errand's connection, or the host's own for a live view.
pub(crate) trait FrameCalls: Sync {
    /// CDP `method` in `session` ("" browser-wide): its result, None on an
    /// error or no answer.
    fn call(&self, session: &str, method: &str, params: Value) -> impl std::future::Future<Output = Option<Value>> + Send;
    /// A flat session on an out-of-process frame's target; None when it
    /// isn't one.
    fn attach(&self, frame: &str) -> impl std::future::Future<Output = Option<String>> + Send;
    fn detach(&self, session: &str) -> impl std::future::Future<Output = ()> + Send;
}

/// The errand's page, for a walk down its frames.
struct OnPage<'a>(&'a Page);

impl FrameCalls for OnPage<'_> {
    async fn call(&self, session: &str, method: &str, params: Value) -> Option<Value> {
        self.0.cdp().call(method, params, (!session.is_empty()).then_some(session)).await.ok()
    }

    async fn attach(&self, frame: &str) -> Option<String> {
        let joined = self.0.cdp().call("Target.attachToTarget", json!({"targetId": frame, "flatten": true}), None).await.ok()?;
        joined["sessionId"].as_str().filter(|id| !id.is_empty() && id.len() <= 128).map(str::to_owned)
    }

    async fn detach(&self, session: &str) {
        let _ = self.0.cdp().call("Target.detachFromTarget", json!({"sessionId": session}), None).await;
    }
}

/// The innermost frame at a point, as Chrome hit-tests it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Innermost {
    pub frame: String,
    /// Its address, as Chrome lists it.
    pub address: String,
    /// Where its content shows, in the top frame's viewport: left, top,
    /// right, bottom.
    pub content: [f64; 4],
}

/// The innermost frame Chrome hit-tests at `at`, a point of the top
/// frame's viewport (`top`, in `session`), down through every frame on the
/// way, another process's too. None on the top page itself, on a frame
/// element's border or padding (that's its parent's), or where Chrome
/// can't say.
pub(crate) async fn innermost_frame(calls: &impl FrameCalls, session: &str, top: &str, at: Point) -> Option<Innermost> {
    let mut session = session.to_owned();
    let mut own = top.to_owned();
    let mut point = at;
    let mut offset = (0.0, 0.0);
    let mut found: Option<(String, [f64; 4])> = None;
    let mut attached = Vec::new();
    for _ in 0..DEEPEST {
        let Some(hit) = calls.call(&session, "DOM.getNodeForLocation", json!({"x": point.0.floor(), "y": point.1.floor(), "includeUserAgentShadowDOM": false, "ignorePointerEventsNone": true})).await
        else {
            break;
        };
        let Some(node) = hit["backendNodeId"].as_i64() else { break };
        let described = calls.call(&session, "DOM.describeNode", json!({"backendNodeId": node})).await.unwrap_or(Value::Null);
        let element = matches!(described["node"]["nodeName"].as_str(), Some("IFRAME" | "FRAME"));
        let child = hit["frameId"].as_str().filter(|frame| !frame.is_empty() && *frame != own).map(str::to_owned);
        // The frame and the element holding it: the element hit, or the
        // holder of the frame of this process the node is in.
        let (inner, owner) = if element {
            let Some(inner) = described["node"]["frameId"].as_str().filter(|frame| !frame.is_empty()) else { break };
            (inner.to_owned(), node)
        } else if let Some(child) = child {
            let Some(owner) = calls.call(&session, "DOM.getFrameOwner", json!({"frameId": child})).await.and_then(|owner| owner["backendNodeId"].as_i64()) else {
                break;
            };
            (child, owner)
        } else {
            break;
        };
        let Some(model) = calls.call(&session, "DOM.getBoxModel", json!({"backendNodeId": owner})).await else { break };
        let quad: Vec<f64> = model["model"]["content"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
        if quad.len() != 8 {
            break;
        }
        let xs = [quad[0], quad[2], quad[4], quad[6]];
        let ys = [quad[1], quad[3], quad[5], quad[7]];
        let (left, right) = (xs.iter().copied().fold(f64::INFINITY, f64::min), xs.iter().copied().fold(f64::NEG_INFINITY, f64::max));
        let (upper, lower) = (ys.iter().copied().fold(f64::INFINITY, f64::min), ys.iter().copied().fold(f64::NEG_INFINITY, f64::max));
        if !(left..right).contains(&point.0) || !(upper..lower).contains(&point.1) {
            break;
        }
        found = Some((inner.clone(), [left + offset.0, upper + offset.1, right + offset.0, lower + offset.1]));
        if !element {
            break;
        }
        // Another process's frame: hit-tested in its own session, the point
        // taken into it from where its element's content shows.
        let Some(inner_session) = calls.attach(&inner).await else { break };
        attached.push(inner_session.clone());
        point = (point.0 - left, point.1 - upper);
        offset = (offset.0 + left, offset.1 + upper);
        session = inner_session;
        own = inner;
    }
    let found = match found {
        Some((frame, content)) => frame_address(calls, &session, &frame).await.map(|address| Innermost { frame, address, content }),
        None => None,
    };
    for session in &attached {
        calls.detach(session).await;
    }
    found
}

/// Frame `frame`'s address as Chrome lists it, for a frame laid out in
/// `session`: in that session's frame tree (its root included), or among
/// the browser's frame targets.
pub(crate) async fn frame_address(calls: &impl FrameCalls, session: &str, frame: &str) -> Option<String> {
    fn find(node: &Value, frame: &str) -> Option<String> {
        if node["frame"]["id"].as_str() == Some(frame) {
            return node["frame"]["url"].as_str().map(str::to_owned);
        }
        node["childFrames"].as_array().into_iter().flatten().find_map(|child| find(child, frame))
    }
    if let Some(tree) = calls.call(session, "Page.getFrameTree", json!({})).await
        && let Some(address) = find(&tree["frameTree"], frame)
    {
        return Some(address);
    }
    iframe_targets(calls).await.into_iter().find_map(|target| (target.id == frame).then_some(target.address))
}

/// The most frames deep a point is followed.
const DEEPEST: usize = 6;

/// The check that the visitor is a person a frame shows, by its address,
/// wherever in the page it is: Google's box or its pictures (never its
/// invisible badge), hCaptcha's, Turnstile's, Arkose's, DataDome's,
/// HUMAN's.
fn check_shown_by(address: &str) -> Option<&'static str> {
    if !check_frame(address) {
        return None;
    }
    let url = url::Url::parse(address).ok()?;
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let under = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
    if under("hcaptcha.com") {
        // hCaptcha names its frame in the address's fragment.
        let fragment = url::form_urlencoded::parse(url.fragment().unwrap_or_default().as_bytes());
        let frame = url.query_pairs().chain(fragment).find(|(key, _)| key == "frame").map(|(_, value)| value.into_owned());
        return matches!(frame.as_deref(), Some("checkbox" | "challenge")).then_some("hcaptcha");
    }
    if host == "challenges.cloudflare.com" {
        return Some("cloudflare");
    }
    if under("arkoselabs.com") || under("funcaptcha.com") {
        return Some("arkose");
    }
    if under("captcha-delivery.com") {
        return Some("datadome");
    }
    if under("px-cloud.net") || under("px-cdn.net") || under("perimeterx.net") {
        return Some("human");
    }
    let path = url.path();
    let invisible = url.query_pairs().any(|(key, value)| key == "size" && value == "invisible");
    ((path.ends_with("/anchor") && !invisible) || path.ends_with("/bframe")).then_some("recaptcha")
}

/// A check that the visitor is a person shown in a frame anywhere in the
/// page, its box on screen: who shows it.
pub(crate) async fn check_in_frames(page: &Page) -> Option<&'static str> {
    let (width, height, _) = view(page).await.ok()?;
    for Listed { id, address, .. } in frames(page).await {
        let Some(who) = check_shown_by(&address) else { continue };
        if let Placed::At((left, top, w, h)) = frame_box(page, &id).await
            && w >= 20.0
            && h >= 20.0
            && left < width
            && top < height
            && left + w > 0.0
            && top + h > 0.0
        {
            return Some(who);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_screenshot_blacks_out_password_fields_and_is_in_css_pixels() {
        let html = r#"<!doctype html><body style="margin:0;background:#fff">
            <input type=password value="hunter2hunter2" style="position:absolute;left:20px;top:30px;width:200px;height:40px;background:#f00;border:0">
            <input type=text value="visible" style="position:absolute;left:20px;top:100px;width:200px;height:40px;background:#0f0;border:0">"#;
        let Some((_local, page, profile)) = super::super::reader::tests::page_with(html).await else { return };
        let picture = take(&page).await.expect("a picture");
        let (width, height, _) = view(&page).await.unwrap();
        assert_eq!((picture.width, picture.height), (width.round() as u32, height.round() as u32));
        let jpeg = base64::engine::general_purpose::STANDARD.decode(&picture.data).unwrap();
        let shown = image::load_from_memory(&jpeg).unwrap().to_rgb8();
        let at = |x: u32, y: u32| shown.get_pixel(x, y).0;
        let dark = |[r, g, b]: [u8; 3]| r < 40 && g < 40 && b < 40;
        assert!(dark(at(120, 50)), "the password field is blacked out: {:?}", at(120, 50));
        let [r, g, b] = at(120, 120);
        assert!(g > 180 && r < 80 && b < 80, "the text field shows: {:?}", (r, g, b));
        let _ = std::fs::remove_dir_all(profile);
    }

    /// A page of shop.test with frames of other sites, each a block of its
    /// colour that counts its presses in its parent's `presses`.
    fn framed(frames: &[(&str, &str)]) -> String {
        let mut html = String::from(r#"<!doctype html><body style="margin:0;background:#fff"><script>window.presses = {}; addEventListener('message', (e) => { window.presses[e.data] = (window.presses[e.data] || 0) + 1; });</script>"#);
        for (at, (host, _)) in frames.iter().enumerate() {
            html.push_str(&format!(r#"<iframe src="https://{host}:PORT/frame" style="position:absolute;left:0;top:{}px;width:300px;height:80px;border:0"></iframe>"#, at * 90));
        }
        html
    }

    fn frame_page(name: &str, colour: &str) -> String {
        format!(r#"<!doctype html><body style="margin:0;background:{colour}" onclick="parent.postMessage('{name}', '*')"><input type=password value="hunter2" style="margin:40px;width:100px"></body>"#)
    }

    /// A site's own challenge frame holding Google's box, as LinkedIn's
    /// checkpoint does, and beside it a box on another host whose address
    /// only looks like Google's.
    #[tokio::test]
    async fn a_check_inside_the_sites_own_frame_is_found_and_pressed_and_nothing_else_is() {
        const ANCHOR: &str = "/recaptcha/enterprise/anchor?k=key&size=normal";
        let html = r#"<!doctype html><body style="margin:0;background:#fff"><script>window.presses = []; addEventListener('message', (e) => window.presses.push(e.data));</script>
            <iframe src="https://shop.test:PORT/challenge" style="position:absolute;left:0;top:0;width:600px;height:300px;border:0"></iframe>"#;
        let challenge = format!(
            r#"<!doctype html><body style="margin:0;background:#eee" onclick="top.postMessage('outer', '*')"><p>Let's do a quick security check</p>
            <iframe src="https://www.google.com:PORT{ANCHOR}" style="position:absolute;left:50px;top:100px;width:300px;height:80px;border:0"></iframe>
            <iframe src="https://recaptcha.evil.test:PORT{ANCHOR}" style="position:absolute;left:50px;top:200px;width:300px;height:80px;border:0"></iframe>"#
        );
        let pressed = |name: &str| format!(r#"<!doctype html><body style="margin:0;background:#0a0;height:80px" onmousedown="top.postMessage('{name}', '*')">"#);
        let pages: Vec<(&'static str, &'static str, String)> =
            vec![("shop.test", "/", html.to_owned()), ("shop.test", "/challenge", challenge), ("www.google.com", ANCHOR, pressed("google")), ("recaptcha.evil.test", ANCHOR, pressed("evil"))];
        let port = super::super::reader::frame_tests::served(pages).await;
        let Some((_local, page, profile)) = super::super::reader::frame_tests::page_at(&format!("https://shop.test:{port}/")).await else { return };
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert_eq!(page.check().await.as_deref(), Some("recaptcha"), "a check inside the site's own frame is one");
        let mut hand = super::super::reader::tests::TestHand::new();
        click_at(&page, &mut hand.hand(), 100.0, 140.0, 0.0, None).await.expect("Google's box is pressed");
        for (x, y, what) in [(500.0, 20.0, "the site's own frame"), (100.0, 240.0, "a box on another host"), (580.0, 290.0, "the outer frame's corner")] {
            let refused = click_at(&page, &mut hand.hand(), x, y, 0.0, None).await.unwrap_err();
            assert_eq!(refused.0, "unsupported_target", "{what}");
        }
        let dragged = click_at(&page, &mut hand.hand(), 100.0, 140.0, 0.0, Some((100.0, 240.0))).await.unwrap_err();
        assert_eq!(dragged.0, "unsupported_target", "a drag out of the check into the look-alike");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(super::super::reader::tests::seen(&page, "window.presses").await, json!(["google"]));
        let _ = std::fs::remove_dir_all(profile);
    }

    #[tokio::test]
    async fn hcaptchas_challenge_named_in_its_fragment_inside_a_srcdoc_frame_is_a_check() {
        let html = r#"<!doctype html><body style="margin:0;background:#fff">
            <iframe srcdoc='<body style="margin:0"><iframe src="https://newassets.hcaptcha.com:PORT/hcaptcha.html#frame=challenge&amp;id=nested" style="width:300px;height:80px;border:0"></iframe>' style="position:absolute;left:0;top:0;width:400px;height:200px;border:0"></iframe>"#;
        let pages: Vec<(&'static str, &'static str, String)> =
            vec![("shop.test", "/", html.to_owned()), ("newassets.hcaptcha.com", "/hcaptcha.html", "<!doctype html><body style='margin:0;background:#0a0'>".to_owned())];
        let port = super::super::reader::frame_tests::served(pages).await;
        let Some((_local, page, profile)) = super::super::reader::frame_tests::page_at(&format!("https://shop.test:{port}/")).await else { return };
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert_eq!(page.check().await.as_deref(), Some("hcaptcha"));
        let _ = std::fs::remove_dir_all(profile);
    }

    #[tokio::test]
    async fn another_sites_frame_is_blacked_out_but_where_a_check_inside_it_shows() {
        let html = r#"<!doctype html><body style="margin:0;background:#fff">
            <iframe src="https://login.other.test:PORT/outer" style="position:absolute;left:0;top:0;width:400px;height:200px;border:0"></iframe>"#;
        let outer = r#"<!doctype html><body style="margin:0;background:#00f">
            <iframe src="https://challenges.cloudflare.com:PORT/frame" style="position:absolute;left:100px;top:50px;width:200px;height:100px;border:0"></iframe>"#;
        let pages: Vec<(&'static str, &'static str, String)> = vec![
            ("shop.test", "/", html.to_owned()),
            ("login.other.test", "/outer", outer.to_owned()),
            ("challenges.cloudflare.com", "/frame", "<!doctype html><body style='margin:0;background:#0a0'>".to_owned()),
        ];
        let port = super::super::reader::frame_tests::served(pages).await;
        let Some((_local, page, profile)) = super::super::reader::frame_tests::page_at(&format!("https://shop.test:{port}/")).await else { return };
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let picture = take(&page).await.expect("a picture");
        let jpeg = base64::engine::general_purpose::STANDARD.decode(&picture.data).unwrap();
        let shown = image::load_from_memory(&jpeg).unwrap().to_rgb8();
        let at = |x: u32, y: u32| shown.get_pixel(x, y).0;
        let dark = |[r, g, b]: [u8; 3]| r < 40 && g < 40 && b < 40;
        assert!(dark(at(50, 100)) && dark(at(350, 100)) && dark(at(200, 20)), "the other site's frame is covered");
        let [r, g, b] = at(200, 100);
        assert!(g > 120 && r < 80 && b < 80, "the check inside it shows: {:?}", (r, g, b));
        let _ = std::fs::remove_dir_all(profile);
    }

    #[tokio::test]
    async fn the_pages_own_srcdoc_frame_shows_with_its_secrets_covered_and_a_sandboxed_one_doesnt() {
        let html = r#"<!doctype html><body style="margin:0;background:#fff">
            <iframe srcdoc='<body style="margin:0;background:#f00"><input type="password" style="position:absolute;left:20px;top:20px;width:100px;height:40px;border:0;background:#f00">' style="position:absolute;left:0;top:0;width:300px;height:150px;border:0"></iframe>
            <iframe sandbox srcdoc='<body style="margin:0;background:#f00">' style="position:absolute;left:0;top:200px;width:300px;height:150px;border:0"></iframe>"#;
        let port = super::super::reader::frame_tests::served(vec![("shop.test", "/", html.to_owned())]).await;
        let Some((_local, page, profile)) = super::super::reader::frame_tests::page_at(&format!("https://shop.test:{port}/")).await else { return };
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let picture = take(&page).await.expect("a picture");
        let jpeg = base64::engine::general_purpose::STANDARD.decode(&picture.data).unwrap();
        let shown = image::load_from_memory(&jpeg).unwrap().to_rgb8();
        let at = |x: u32, y: u32| shown.get_pixel(x, y).0;
        let dark = |[r, g, b]: [u8; 3]| r < 40 && g < 40 && b < 40;
        let [r, g, b] = at(200, 100);
        assert!(r > 150 && g < 80 && b < 80, "the page's own frame shows: {:?}", (r, g, b));
        assert!(dark(at(70, 40)), "its password field is covered");
        assert!(dark(at(150, 275)), "the sandboxed frame is another origin, covered");
        let _ = std::fs::remove_dir_all(profile);
    }

    #[tokio::test]
    async fn click_at_presses_only_inside_a_checks_frame() {
        let frames = [
            ("challenges.cloudflare.com", "check"),
            ("accounts.google.com", "google"),
            ("js.stripe.com", "stripe"),
            ("www.paypal.com", "paypal"),
        ];
        let mut pages: Vec<(&'static str, &'static str, String)> = vec![("shop.test", "/", framed(&frames))];
        for (host, name) in frames {
            pages.push((host, "/frame", frame_page(name, "#0a0")));
        }
        let port = super::super::reader::frame_tests::served(pages).await;
        let Some((_local, page, profile)) = super::super::reader::frame_tests::page_at(&format!("https://shop.test:{port}/")).await else { return };
        let mut hand = super::super::reader::tests::TestHand::new();
        for (at, (_, name)) in frames.iter().enumerate() {
            let y = 90.0 * at as f64 + 10.0;
            let pressed = click_at(&page, &mut hand.hand(), 150.0, y, 0.0, None).await;
            if *name == "check" {
                pressed.unwrap_or_else(|why| panic!("{name}: {why:?}"));
            } else {
                assert_eq!(pressed.map_err(|why| why.0), Err("unsupported_target".to_owned()), "{name}");
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert_eq!(super::super::reader::tests::seen(&page, "JSON.stringify(window.presses)").await, r#"{"check":1}"#);
        let refused = click_at(&page, &mut hand.hand(), 500.0, 50.0, 0.0, None).await.unwrap_err();
        assert_eq!(refused.0, "unsupported_target", "the page itself");
        let _ = std::fs::remove_dir_all(profile);
    }

    #[tokio::test]
    async fn click_at_drags_with_the_button_down_inside_a_checks_frame() {
        let html = r#"<!doctype html><body style="margin:0"><script>window.dragged = null; addEventListener('message', (e) => { window.dragged = e.data; });</script>
            <iframe src="https://challenges.cloudflare.com:PORT/frame" style="position:absolute;left:0;top:0;width:400px;height:100px;border:0"></iframe>
            <iframe src="https://challenges.cloudflare.com:PORT/frame" style="position:absolute;left:0;top:120px;width:400px;height:100px;border:0"></iframe>"#;
        let slider = r#"<!doctype html><body style="margin:0;height:100px"><script>
            let down = null, moves = 0, held = true;
            addEventListener('mousedown', (e) => { down = [e.clientX, e.clientY]; });
            addEventListener('mousemove', (e) => { if (down) { moves++; held = held && e.buttons === 1; } });
            addEventListener('mouseup', (e) => parent.postMessage({down, up: [e.clientX, e.clientY], moves, held}, '*'));
            </script>"#;
        let pages: Vec<(&'static str, &'static str, String)> = vec![("shop.test", "/", html.to_owned()), ("challenges.cloudflare.com", "/frame", slider.to_owned())];
        let port = super::super::reader::frame_tests::served(pages).await;
        let Some((_local, page, profile)) = super::super::reader::frame_tests::page_at(&format!("https://shop.test:{port}/")).await else { return };
        let mut hand = super::super::reader::tests::TestHand::new();
        let off = click_at(&page, &mut hand.hand(), 20.0, 50.0, 0.0, Some((500.0, 50.0))).await.unwrap_err();
        assert_eq!(off.0, "unsupported_target", "a drag out of the check's frame");
        let across = click_at(&page, &mut hand.hand(), 20.0, 50.0, 0.0, Some((20.0, 170.0))).await.unwrap_err();
        assert_eq!(across.0, "unsupported_target", "a drag from one check's frame into another's");
        assert_eq!(super::super::reader::tests::seen(&page, "window.dragged").await, Value::Null, "nothing pressed");
        click_at(&page, &mut hand.hand(), 20.0, 50.0, 0.0, Some((300.0, 50.0))).await.expect("dragged");
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let dragged = super::super::reader::tests::seen(&page, "window.dragged").await;
        assert_eq!((dragged["down"].clone(), dragged["up"].clone()), (json!([20, 50]), json!([300, 50])), "{dragged}");
        assert!(dragged["moves"].as_u64().unwrap_or_default() >= 8 && dragged["held"] == true, "{dragged}");
        let _ = std::fs::remove_dir_all(profile);
    }

    #[tokio::test]
    async fn a_scrolled_pages_frames_are_blacked_out_where_they_show() {
        let frames = [("challenges.cloudflare.com", "check"), ("login.other.test", "login")];
        let mut html = framed(&frames);
        html.push_str(r#"<div style="height:3000px"></div>"#);
        let pages: Vec<(&'static str, &'static str, String)> = vec![
            ("shop.test", "/", html),
            ("challenges.cloudflare.com", "/frame", "<!doctype html><body style='margin:0;background:#0a0'>".to_owned()),
            ("login.other.test", "/frame", "<!doctype html><body style='margin:0;background:#0a0'>".to_owned()),
        ];
        let port = super::super::reader::frame_tests::served(pages).await;
        let Some((_local, page, profile)) = super::super::reader::frame_tests::page_at(&format!("https://shop.test:{port}/")).await else { return };
        page.call("Runtime.evaluate", json!({"expression": "scrollTo(0, 40)"})).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let picture = take(&page).await.expect("a picture");
        let jpeg = base64::engine::general_purpose::STANDARD.decode(&picture.data).unwrap();
        let shown = image::load_from_memory(&jpeg).unwrap().to_rgb8();
        let at = |x: u32, y: u32| shown.get_pixel(x, y).0;
        let dark = |[r, g, b]: [u8; 3]| r < 40 && g < 40 && b < 40;
        let white = |[r, g, b]: [u8; 3]| r > 200 && g > 200 && b > 200;
        // The other site's frame is 90 to 170 down the page: 50 to 130 in view.
        assert!(dark(at(150, 60)) && dark(at(150, 125)), "{:?} {:?}", at(150, 60), at(150, 125));
        assert!(white(at(150, 150)), "below it the page shows: {:?}", at(150, 150));
        let _ = std::fs::remove_dir_all(profile);
    }

    #[tokio::test]
    async fn a_screenshot_blacks_out_other_sites_frames_but_a_checks_and_fields_claktory_filled() {
        let frames = [("challenges.cloudflare.com", "check"), ("login.other.test", "login")];
        let mut html = framed(&frames);
        html.push_str(r#"<input id=shown type=text value="Sw0rdfish" style="position:absolute;left:400px;top:20px;width:200px;height:40px;background:#0f0;border:0">"#);
        let pages: Vec<(&'static str, &'static str, String)> = vec![
            ("shop.test", "/", html),
            ("challenges.cloudflare.com", "/frame", "<!doctype html><body style='margin:0;background:#0a0'>".to_owned()),
            ("login.other.test", "/frame", "<!doctype html><body style='margin:0;background:#0a0'><input type=text value=secret>".to_owned()),
        ];
        let port = super::super::reader::frame_tests::served(pages).await;
        let Some((_local, page, profile)) = super::super::reader::frame_tests::page_at(&format!("https://shop.test:{port}/")).await else { return };
        // A password field Claktory filled, shown as text since.
        let Ok(crate::browser::schema::Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
        let field = read.elements.iter().find(|element| element.role == "textbox").expect("the field");
        let (object, _) = page.element(&read.document, &field.reference).await.expect("its node");
        page.remember_filled(&object);
        let picture = take(&page).await.expect("a picture");
        let jpeg = base64::engine::general_purpose::STANDARD.decode(&picture.data).unwrap();
        let shown = image::load_from_memory(&jpeg).unwrap().to_rgb8();
        let at = |x: u32, y: u32| shown.get_pixel(x, y).0;
        let dark = |[r, g, b]: [u8; 3]| r < 40 && g < 40 && b < 40;
        let green = |[r, g, b]: [u8; 3]| g > 120 && r < 80 && b < 80;
        assert!(green(at(150, 40)), "the check's frame shows: {:?}", at(150, 40));
        assert!(dark(at(150, 130)), "another site's frame is blacked out: {:?}", at(150, 130));
        assert!(dark(at(500, 40)), "the field Claktory filled is blacked out: {:?}", at(500, 40));
        let _ = std::fs::remove_dir_all(profile);
    }
}
