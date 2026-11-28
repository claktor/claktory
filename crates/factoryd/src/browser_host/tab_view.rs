//! Live views of one agent's browser (WatchAgentTab): watch only, following
//! the page it shows, or the one tab of a check the agent handed the person
//! (`check_opened`), with input for that request alone.
//!
//! Each view is a CDP session of the host's own on one tab, attached flat
//! with the host's ids, whose screencast is that page's pixels alone: never
//! the browser's bar, another tab or another agent's browser. What comes on
//! that session is taken here (`event`) and never reaches the errand; its
//! frames go to the one connection watching, one at a time, and nothing of
//! them is kept. A check's view ends before the next frame once its tab's
//! page is of another site, and its input goes only into the check itself,
//! a check-provider's frame on that page (`Hands`), under a short input
//! lease of the host's: a page can't draw a form of its own beside the
//! check and take what the person types.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use factory_protocol::v1::{self, AgentBrowserPointerKind as PointerKind, AgentTabViewEnd as End, agent_browser_event::Event, server_message::Body};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use unicode_segmentation::UnicodeSegmentation;

use super::{Inner, One, State, control, refused, sites, view};
use crate::browser_runner::wire::{Control, Mode};
use crate::errands::screenshot::{FrameCalls, Innermost, check_frame, frame_address, innermost_frame};
use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;

/// The widest frame sent, whatever a device asks.
const MAX_WIDTH: u32 = 2048;
/// The width sent when a device doesn't say.
const DEFAULT_WIDTH: u32 = 1280;
const MAX_HEIGHT: u16 = 4096;
/// The biggest frame sent: a page can't make one huge one.
const MAX_JPEG: usize = 1 << 20;
/// The least time between two frames (at most 15 a second).
const FRAME_GAP: Duration = Duration::from_millis(66);
/// How often a view looks at its tab again: still there, still the site,
/// and (watch only) still the page shown.
const EVERY: Duration = Duration::from_millis(500);
const ANSWER_WAIT: Duration = Duration::from_secs(5);
/// The input lease the host takes for the person's input in a check.
const LEASE_MS: u32 = 5_000;
/// The frames remembered, for input placed on an earlier one.
const SIZES: usize = 8;
/// How long the frame found under the pointer is taken as known, for
/// moves with no button held.
const FOUND_FOR: Duration = Duration::from_millis(200);
/// How deep the focus is followed down a page's frames.
const DEPTH: usize = 6;
/// The most sessions the host keeps on a check page's frames.
const SESSIONS: usize = 16;
/// The batches of input waiting for `hands`.
const BATCHES: usize = 32;
const NOTE_GAP: Duration = Duration::from_secs(1);
const ONLY_CLICKS: &str = "Only the check takes your clicks here.";
const ONLY_TYPING: &str = "Click in the check first: only it takes your typing.";
/// The isolated world the host reads the focus in, out of the page's reach.
const WORLD: &str = "claktory-tab-view";

type Point = (f64, f64);

/// A check the agent handed the person, from `check_opened` until it ends.
pub(super) struct Check {
    seq: u64,
    target: String,
    domain: String,
    /// Told once, if the tab leaves `domain`.
    left: Option<Box<dyn FnOnce(String) + Send>>,
}

/// One connection's view of one agent's browser.
pub(super) struct View {
    id: u64,
    /// The check's request seq; 0 watching only.
    seq: u64,
    port: u64,
    sender: mpsc::Sender<Body>,
    max_width: u32,
    /// The tab shown (its CDP target id), and the host's session on it.
    target: String,
    session: Option<String>,
    site: String,
    origin: String,
    title: String,
    frame_seq: u64,
    /// The frame sent and not acked yet.
    in_flight: Option<u64>,
    /// Chrome's frame waiting for the host's ack (its screencast session).
    unacked: Option<i64>,
    last_sent: Option<Instant>,
    /// Recent frames' page size in CSS pixels, by frame seq.
    sizes: VecDeque<(u64, f64, f64)>,
    /// A check's: its input, to `hands`.
    input: Option<mpsc::Sender<Vec<Intent>>>,
    input_window: (i64, u32),
    stop: CancellationToken,
}

/// An agent's browser's views and open checks.
#[derive(Default)]
pub(super) struct Views {
    views: Vec<View>,
    checks: Vec<Check>,
    next: u64,
}

impl Views {
    fn view(&mut self, id: u64) -> Option<&mut View> {
        self.views.iter_mut().find(|view| view.id == id)
    }
}

/// `url`'s site in Claktory's words: its registrable domain in ASCII, any
/// character that isn't plain ASCII shown as a code; empty for a page of no
/// web site (about:blank, data:, file:).
pub(crate) fn site_of(url: &str) -> String {
    let Ok(url) = url::Url::parse(url) else { return String::new() };
    if !matches!(url.scheme(), "http" | "https") {
        return String::new();
    }
    let site = match url.host() {
        Some(url::Host::Domain(_)) => crate::errands::handoff::domain(&url),
        Some(address) => address.to_string(),
        None => String::new(),
    };
    site.chars()
        .map(|c| if c.is_ascii_graphic() { c.to_string() } else { format!("\\u{{{:x}}}", c as u32) })
        .collect()
}

fn origin_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .map(|url| view::cleaned(&url.origin().ascii_serialization(), 200))
        .unwrap_or_default()
}

fn ended(agent: &str, seq: u64, reason: End, message: String, device: &str) -> Body {
    Body::AgentTabViewEnded(v1::AgentTabViewEnded {
        agent_id: agent.to_owned(),
        request_seq: seq,
        reason: reason.into(),
        message,
        device: device.to_owned(),
    })
}

/// One of the host's calls, its answer not waited for.
fn send(inner: &mut Inner, session: &str, method: &str, params: Value) -> Option<oneshot::Receiver<Value>> {
    sites::ask(inner, session, method, params)
}

/// One of the host's calls, and its answer; None when there's none in time.
async fn ask(one: &One, session: &str, method: &str, params: Value) -> Option<Value> {
    let answered = send(&mut one.inner(), session, method, params)?;
    let answer = tokio::time::timeout(ANSWER_WAIT, answered).await.ok()?.ok()?;
    answer.get("error").is_none().then_some(answer)
}

/// Lets the view go in Chrome: its screencast stops, the chooser comes
/// back and its session is detached.
fn let_go(inner: &mut Inner, view: &View) {
    view.stop.cancel();
    if let Some(session) = &view.session {
        send(inner, session, "Page.stopScreencast", json!({}));
        if view.seq != 0 {
            send(inner, session, "Page.setInterceptFileChooserDialog", json!({"enabled": false}));
        }
        send(inner, "", "Target.detachFromTarget", json!({"sessionId": session}));
    }
}

/// The errand log's line for a view opening or closing: the site, never the
/// device (agents may read the log).
fn log(factory: &Factory, agent: &str, errand: &str, origin: &str, action: &str) {
    let entry = crate::errands::log::Entry {
        errand: errand.to_owned(),
        agent: agent.to_owned(),
        origin: origin.to_owned(),
        action: action.to_owned(),
        ..Default::default()
    };
    crate::errands::log::write(&factory.store, &entry);
}

fn closed_line(seq: u64) -> &'static str {
    if seq == 0 { "You stopped watching its browser" } else { "The check closed on your device" }
}

/// Ends view `id` of `one` from Claktory's side: the device is told why.
fn finish(factory: &Factory, one: &One, id: u64, reason: End, message: String) {
    let (errand, origin, seq) = {
        let mut inner = one.inner();
        let Some(at) = inner.views.views.iter().position(|view| view.id == id) else { return };
        let view = inner.views.views.remove(at);
        let _ = view.sender.try_send(ended(&one.agent, view.seq, reason, message, ""));
        let_go(&mut inner, &view);
        (inner.errand.clone(), view.origin, view.seq)
    };
    log(factory, &one.agent, &errand, &origin, closed_line(seq));
}

/// Ends every view of connection `port`, with nothing said to it (it asked,
/// or it's gone).
pub(super) fn stop_port(factory: &Factory, port: u64) {
    for one in factory.agent_browser.all() {
        let gone: Vec<(String, String, u64)> = {
            let mut inner = one.inner();
            let (mine, others): (Vec<View>, Vec<View>) = std::mem::take(&mut inner.views.views).into_iter().partition(|view| view.port == port);
            inner.views.views = others;
            mine.iter()
                .map(|view| {
                    let_go(&mut inner, view);
                    (inner.errand.clone(), view.origin.clone(), view.seq)
                })
                .collect()
        };
        for (errand, origin, seq) in gone {
            log(factory, &one.agent, &errand, &origin, closed_line(seq));
        }
    }
}

/// WatchAgentTab from connection `port`.
pub(super) fn watch(factory: &Arc<Factory>, port: u64, request: &v1::WatchAgentTab) -> Result<(), RequestError> {
    if !request.on {
        stop_port(factory, port);
        return Ok(());
    }
    let host = &factory.agent_browser;
    let (device, sender) = host
        .ports()
        .get(&port)
        .map(|slot| (slot.device.clone(), slot.sender.clone()))
        .ok_or_else(|| refused("This connection is closing."))?;
    let agent = request.agent_id.as_str();
    let seq = request.request_seq;
    let name = {
        let inner = factory.lock();
        let found = inner.config.agent(agent).filter(|found| super::browsing(found));
        found.map(|found| found.name.clone()).ok_or_else(|| refused("That agent has no browser."))?
    };
    let one = host.get(agent).filter(|one| super::open_now(one)).ok_or_else(|| refused(format!("{name}'s browser isn't open.")))?;
    let target = {
        let inner = one.inner();
        if inner.mode != Some(Mode::Driven) {
            return Err(refused(format!("{name}'s browser can't be shown now: it's waiting for you in another way.")));
        }
        if seq == 0 {
            String::new()
        } else {
            let check = inner.views.checks.iter().find(|check| check.seq == seq);
            check.map(|check| check.target.clone()).ok_or_else(|| refused("This check isn't waiting for you any more."))?
        }
    };
    stop_port(factory, port);
    let max_width = if request.max_width == 0 { DEFAULT_WIDTH } else { request.max_width.clamp(160, MAX_WIDTH) };
    let (id, errand) = {
        let mut inner = one.inner();
        if let Some(at) = inner.views.views.iter().position(|view| view.seq == seq) {
            let before = inner.views.views.remove(at);
            let moved = format!("It opened on {device}.");
            let _ = before.sender.try_send(ended(agent, seq, End::Moved, moved, &device));
            let_go(&mut inner, &before);
        }
        inner.views.next += 1;
        let id = inner.views.next;
        let input = (seq != 0).then(|| {
            let (input, batches) = mpsc::channel(BATCHES);
            tokio::spawn(hands(one.clone(), id, batches));
            input
        });
        inner.views.views.push(View {
            id,
            seq,
            port,
            sender,
            max_width,
            target,
            session: None,
            site: String::new(),
            origin: String::new(),
            title: String::new(),
            frame_seq: 0,
            in_flight: None,
            unacked: None,
            last_sent: None,
            sizes: VecDeque::new(),
            input,
            input_window: (0, 0),
            stop: CancellationToken::new(),
        });
        (id, inner.errand.clone())
    };
    log(factory, agent, &errand, "", if seq == 0 { "You watched its browser" } else { "The check opened on your device" });
    tokio::spawn(run(Arc::downgrade(factory), one, id));
    Ok(())
}

/// Keeps view `id` on its tab until it ends.
async fn run(factory: Weak<Factory>, one: Arc<One>, id: u64) {
    let Some(stop) = one.inner().views.view(id).map(|view| view.stop.clone()) else { return };
    let mut every = tokio::time::interval(EVERY);
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            _ = every.tick() => {}
        }
        let Some(factory) = factory.upgrade() else { return };
        if let Err((reason, message)) = look(&one, id).await {
            if reason == End::LeftSite {
                left(&one, id, &message);
            }
            finish(&factory, &one, id, reason, message);
            return;
        }
    }
}

/// What a check's site, and a view's tab, are now: Err ends the view.
async fn look(one: &One, id: u64) -> Result<(), (End, String)> {
    let (seq, mut target, session, check) = {
        let mut inner = one.inner();
        if inner.runner.is_none() || matches!(inner.state, State::Off | State::Stopping) {
            return Err((End::Closed, "The browser closed.".to_owned()));
        }
        if inner.mode != Some(Mode::Driven) {
            return Err((End::Unavailable, "The browser is waiting for you in another way, so the view stopped.".to_owned()));
        }
        let check = |seq: u64, inner: &Inner| inner.views.checks.iter().find(|check| check.seq == seq).map(|check| check.domain.clone());
        let Some(view) = inner.views.view(id) else { return Ok(()) };
        let (seq, target, session) = (view.seq, view.target.clone(), view.session.clone());
        let domain = if seq == 0 { None } else { Some(check(seq, &inner).ok_or((End::RequestEnded, "The check is over.".to_owned()))?) };
        (seq, target, session, domain)
    };
    if seq == 0 {
        target = shown(one, &target).await.unwrap_or(target);
    }
    if target.is_empty() {
        return Ok(());
    }
    let info = ask(one, "", "Target.getTargetInfo", json!({"targetId": target})).await;
    let Some(info) = info.as_ref().map(|answer| &answer["result"]["targetInfo"]) else {
        return match check {
            Some(_) => Err((End::Closed, "The check's tab closed.".to_owned())),
            None => Ok(()),
        };
    };
    let url = info["url"].as_str().unwrap_or_default();
    let site = site_of(url);
    if let Some(domain) = &check
        && site != *domain
    {
        return Err((End::LeftSite, format!("The page left {domain}, so the view stopped.")));
    }
    let title = view::cleaned(info["title"].as_str().unwrap_or_default(), 200);
    let attach = {
        let mut inner = one.inner();
        let Some(view) = inner.views.view(id) else { return Ok(()) };
        view.site = site;
        view.origin = origin_of(url);
        view.title = title;
        session.is_none() || view.target != target
    };
    if attach {
        on_tab(one, id, &target).await;
    }
    if check.is_some() {
        close_popups(one, &target).await;
    }
    Ok(())
}

/// The page the agent's browser shows: the one its errand is attached to
/// before one it isn't, a web page before a blank one, and `now` while it's
/// as good as any.
async fn shown(one: &One, now: &str) -> Option<String> {
    let answer = ask(one, "", "Target.getTargets", json!({})).await?;
    let pages: Vec<&Value> =
        answer["result"]["targetInfos"].as_array()?.iter().filter(|target| target["type"] == "page").collect();
    let web = |target: &Value| target["url"].as_str().is_some_and(|url| url.starts_with("http://") || url.starts_with("https://"));
    let rank = |target: &Value| (target["attached"] != Value::Bool(true), !web(target), target["targetId"] != now);
    pages.into_iter().min_by_key(|target| rank(target)).and_then(|target| target["targetId"].as_str().map(str::to_owned))
}

/// Puts view `id` on tab `target`: the session before goes, a new one is
/// attached, and its screencast starts (a check's with no file chooser).
async fn on_tab(one: &One, id: u64, target: &str) {
    {
        let mut inner = one.inner();
        let Some(view) = inner.views.view(id) else { return };
        let before = view.session.take();
        view.target = target.to_owned();
        view.in_flight = None;
        view.unacked = None;
        if let Some(session) = before {
            send(&mut inner, "", "Target.detachFromTarget", json!({"sessionId": session}));
        }
    }
    let answer = ask(one, "", "Target.attachToTarget", json!({"targetId": target, "flatten": true})).await;
    let Some(session) = answer.as_ref().and_then(|answer| answer["result"]["sessionId"].as_str()).map(str::to_owned) else { return };
    let mut inner = one.inner();
    let Some(view) = inner.views.view(id).filter(|view| view.target == target && view.session.is_none()) else {
        send(&mut inner, "", "Target.detachFromTarget", json!({"sessionId": session}));
        return;
    };
    view.session = Some(session.clone());
    let (seq, max_width) = (view.seq, view.max_width);
    // In front, so Chrome paints it (a tab behind others sends no frame).
    send(&mut inner, "", "Target.activateTarget", json!({"targetId": target}));
    send(&mut inner, &session, "Page.enable", json!({}));
    if seq != 0 && send(&mut inner, &session, "Page.setInterceptFileChooserDialog", json!({"enabled": true})).is_none() {
        tracing::warn!("a check's tab kept its file chooser");
    }
    let screencast = json!({"format": "jpeg", "quality": 70, "maxWidth": max_width, "maxHeight": MAX_HEIGHT, "everyNthFrame": 1});
    send(&mut inner, &session, "Page.startScreencast", screencast);
}

/// Closes the windows a check's tab opened: nothing new comes of it.
async fn close_popups(one: &One, target: &str) {
    let Some(answer) = ask(one, "", "Target.getTargets", json!({})).await else { return };
    let opened: Vec<String> = answer["result"]["targetInfos"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|info| info["openerId"] == target && info["type"] == "page")
        .filter_map(|info| info["targetId"].as_str().map(str::to_owned))
        .collect();
    let mut inner = one.inner();
    for popup in opened {
        send(&mut inner, "", "Target.closeTarget", json!({"targetId": popup}));
    }
}

/// The check of view `id` left its site: its agent's side is told once,
/// with the site it left.
fn left(one: &One, id: u64, message: &str) {
    let tell = {
        let mut inner = one.inner();
        let Some(seq) = inner.views.view(id).map(|view| view.seq) else { return };
        let Some(at) = inner.views.checks.iter().position(|check| check.seq == seq) else { return };
        let mut check = inner.views.checks.remove(at);
        check.left.take().map(|left| (left, check.domain))
    };
    tracing::info!(message, "a check's tab left its site");
    if let Some((left, site)) = tell {
        left(site);
    }
}

/// A CDP message from the runner on one of the views' sessions: taken here
/// (true) and never the errand's; false for anything else.
pub(super) fn event(factory: &Arc<Factory>, one: &One, payload: &[u8]) -> bool {
    let sessions: Vec<String> = one.inner().views.views.iter().filter_map(|view| view.session.clone()).collect();
    if sessions.is_empty() {
        return false;
    }
    let Ok(text) = std::str::from_utf8(payload) else { return false };
    if !sessions.iter().any(|session| text.contains(session.as_str())) {
        return false;
    }
    let Ok(message) = serde_json::from_str::<Value>(text) else { return false };
    let method = message["method"].as_str().unwrap_or_default();
    let about = match method {
        "Target.detachedFromTarget" | "Target.attachedToTarget" => message["params"]["sessionId"].as_str(),
        _ => message["sessionId"].as_str(),
    };
    let Some(session) = about.filter(|about| sessions.iter().any(|session| session == about)) else { return false };
    let Some(id) = one.inner().views.views.iter().find(|view| view.session.as_deref() == Some(session)).map(|view| view.id) else {
        return true;
    };
    let params = &message["params"];
    match method {
        "Page.screencastFrame" => frame(one, id, session, params),
        "Page.frameNavigated" if params["frame"]["parentId"].is_null() => {
            navigated(factory, one, id, params["frame"]["url"].as_str().unwrap_or_default());
        }
        "Target.detachedFromTarget" => {
            let check = {
                let mut inner = one.inner();
                let Some(view) = inner.views.view(id) else { return true };
                view.session = None;
                view.seq != 0
            };
            if check {
                finish(factory, one, id, End::Closed, "The check's tab closed.".to_owned());
            }
        }
        _ => {}
    }
    true
}

/// The tab's main frame went to `url`: a check's view ends, before any of
/// the new page's frames, once it isn't the check's site.
fn navigated(factory: &Factory, one: &One, id: u64, url: &str) {
    let site = site_of(url);
    let domain = {
        let mut inner = one.inner();
        let Some(view) = inner.views.view(id) else { return };
        let seq = view.seq;
        view.site = site.clone();
        view.origin = origin_of(url);
        if seq == 0 {
            return;
        }
        inner.views.checks.iter().find(|check| check.seq == seq).map(|check| check.domain.clone())
    };
    match domain {
        Some(domain) if domain == site => {}
        Some(domain) => {
            let message = format!("The page left {domain}, so the view stopped.");
            left(one, id, &message);
            finish(factory, one, id, End::LeftSite, message);
        }
        None => finish(factory, one, id, End::RequestEnded, "The check is over.".to_owned()),
    }
}

/// A screencast frame: to the connection watching, when it's within bounds
/// and none is in flight; Chrome's next waits for its ack.
fn frame(one: &One, id: u64, session: &str, params: &Value) {
    let chrome = params["sessionId"].as_i64().unwrap_or_default();
    let jpeg = params["data"].as_str().and_then(|data| STANDARD.decode(data).ok()).unwrap_or_default();
    let css = (params["metadata"]["deviceWidth"].as_f64().unwrap_or(0.0), params["metadata"]["deviceHeight"].as_f64().unwrap_or(0.0));
    let mut inner = one.inner();
    let agent = one.agent.clone();
    let Some(view) = inner.views.view(id) else { return };
    let size = view::jpeg_size(&jpeg);
    let fits = jpeg.len() <= MAX_JPEG
        && size.is_some_and(|(width, height)| {
            width > 0 && height > 0 && u32::from(width) <= view.max_width && height <= MAX_HEIGHT
        });
    if !fits || view.in_flight.is_some() {
        if !fits {
            tracing::warn!(bytes = jpeg.len(), ?size, "a frame of an agent's tab was too big: dropped");
        }
        send(&mut inner, session, "Page.screencastFrameAck", json!({"sessionId": chrome}));
        return;
    }
    let (width, height) = size.unwrap_or_default();
    view.frame_seq += 1;
    let frame = v1::AgentTabFrame {
        agent_id: agent,
        request_seq: view.seq,
        frame_seq: view.frame_seq,
        jpeg,
        width: u32::from(width),
        height: u32::from(height),
        site: view.site.clone(),
        origin: view.origin.clone(),
        title: view.title.clone(),
    };
    if view.sender.try_send(Body::AgentTabFrame(frame)).is_ok() {
        view.in_flight = Some(view.frame_seq);
        view.unacked = Some(chrome);
        view.last_sent = Some(Instant::now());
        view.sizes.push_back((view.frame_seq, css.0, css.1));
        if view.sizes.len() > SIZES {
            view.sizes.pop_front();
        }
    } else {
        send(&mut inner, session, "Page.screencastFrameAck", json!({"sessionId": chrome}));
    }
}

/// The connection that watches `agent`'s view `seq`: its view's id.
fn watched(factory: &Factory, port: u64, agent: &str, seq: u64) -> Option<(Arc<One>, u64)> {
    let one = factory.agent_browser.get(agent)?;
    let id = one.inner().views.views.iter().find(|view| view.port == port && view.seq == seq).map(|view| view.id)?;
    Some((one, id))
}

/// AgentTabFrameAck: Chrome's next frame may come, no sooner than
/// FRAME_GAP after the last.
pub(super) fn ack(factory: &Factory, port: u64, request: &v1::AgentTabFrameAck) {
    let Some((one, id)) = watched(factory, port, &request.agent_id, request.request_seq) else { return };
    let wait = {
        let mut inner = one.inner();
        let Some(view) = inner.views.view(id) else { return };
        if view.in_flight != Some(request.frame_seq) {
            return;
        }
        view.in_flight = None;
        FRAME_GAP.saturating_sub(view.last_sent.map_or(FRAME_GAP, |sent| sent.elapsed()))
    };
    let next = move || {
        let mut inner = one.inner();
        let Some(view) = inner.views.view(id) else { return };
        let (Some(chrome), Some(session)) = (view.unacked.take(), view.session.clone()) else { return };
        send(&mut inner, &session, "Page.screencastFrameAck", json!({"sessionId": chrome}));
    };
    if wait.is_zero() {
        next();
    } else {
        tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            next();
        });
    }
}

/// AgentTabInput: into a check's tab, from the connection watching it,
/// while the check is open.
pub(super) fn input(factory: &Factory, port: u64, request: &v1::AgentTabInput) -> Result<(), RequestError> {
    if request.request_seq == 0 {
        return Err(refused("Watching an agent's browser takes no input."));
    }
    let (one, id) = watched(factory, port, &request.agent_id, request.request_seq)
        .ok_or_else(|| refused("Open the check here first."))?;
    let mut inner = one.inner();
    if !inner.views.checks.iter().any(|check| check.seq == request.request_seq) {
        return Err(refused("This check isn't waiting for you any more."));
    }
    if !matches!(inner.state, State::Driven | State::PersonsTurn) || inner.mode != Some(Mode::Driven) {
        return Err(refused("The browser can't take your input now."));
    }
    let Some(view) = inner.views.view(id) else { return Err(refused("Open the check here first.")) };
    let intents = intents(view, &request.events).map_err(refused)?;
    let now = now_ms();
    if now - view.input_window.0 >= 1000 {
        view.input_window = (now, 0);
    }
    let room = view::EVENTS_PER_SECOND.saturating_sub(view.input_window.1) as usize;
    let taken: Vec<Intent> = intents.into_iter().take(room).collect();
    view.input_window.1 += taken.len() as u32;
    if !taken.is_empty()
        && let Some(input) = &view.input
        && input.try_send(taken).is_err()
    {
        tracing::warn!("a check's input came faster than it could go: dropped");
    }
    Ok(())
}

/// One of the person's events in a check, as the page would take it, at
/// CSS pixels of the page.
#[derive(Debug, Clone, PartialEq)]
enum Intent {
    Press { button: &'static str, at: Point },
    Move { at: Point },
    Release { at: Point },
    Wheel { at: Point, delta: Point },
    Key(Value),
    Text(String),
}

/// Where a check's input may go, as `hands` last found it.
#[derive(Default)]
struct Hands {
    /// The session it was found on: anything else is found again.
    on: String,
    /// The press taken: the check's frame it went into, its box on the
    /// page, the button and the pointer's last place.
    held: Option<(Innermost, &'static str, Point)>,
    /// The frame last found under the pointer, and when: hovering is
    /// taken over a check's box, the frame found again at most every
    /// FOUND_FOR.
    found: Option<(Instant, Option<Innermost>)>,
    /// The host's sessions on the page's out-of-process frames, by frame
    /// (None: it isn't one), kept while the view lasts.
    sessions: std::sync::Mutex<HashMap<String, Option<String>>>,
    /// The isolated worlds the focus is read in, by session and frame.
    worlds: HashMap<(String, String), i64>,
    /// The focus was found outside the check: no keys or text until a
    /// press goes into the check again.
    latched: bool,
    noted: Option<Instant>,
}

/// The host's calls on a check's page for the frame walk, its sessions on
/// the page's out-of-process frames kept in `sessions`.
struct HostFrames<'a> {
    one: &'a One,
    sessions: &'a std::sync::Mutex<HashMap<String, Option<String>>>,
}

impl FrameCalls for HostFrames<'_> {
    async fn call(&self, session: &str, method: &str, params: Value) -> Option<Value> {
        ask(self.one, session, method, params).await.map(|mut answer| answer["result"].take())
    }

    async fn attach(&self, frame: &str) -> Option<String> {
        if let Some(known) = crate::locking::lock(self.sessions).get(frame) {
            return known.clone();
        }
        let made = ask(self.one, "", "Target.attachToTarget", json!({"targetId": frame, "flatten": true})).await;
        let session = made.and_then(|made| made["result"]["sessionId"].as_str().map(str::to_owned));
        let mut sessions = crate::locking::lock(self.sessions);
        if sessions.len() < SESSIONS {
            sessions.insert(frame.to_owned(), session.clone());
        } else if let Some(session) = &session {
            send(&mut self.one.inner(), "", "Target.detachFromTarget", json!({"sessionId": session}));
            return None;
        }
        session
    }

    async fn detach(&self, _session: &str) {}
}

/// Takes a check view's input, in order, into the check's own frames:
/// presses and wheels only where Chrome finds a check-provider's frame the
/// innermost under the pointer, moves only over one (a drag only over the
/// frame it began in), a release only after a press taken, and keys and
/// text only while the page's focus, followed down its frames, is in one:
/// text a grapheme at a time, and none once the focus was found elsewhere,
/// even just after a key (a page taking the focus back on a timer gets one
/// character at most) until a press goes into the check again. Everything
/// else goes nowhere, the view told why.
async fn hands(one: Arc<One>, id: u64, mut batches: mpsc::Receiver<Vec<Intent>>) {
    let mut hands = Hands::default();
    while let Some(batch) = batches.recv().await {
        let one_at_a_time = |intent: Intent| match intent {
            Intent::Text(text) => text.graphemes(true).map(|grapheme| Intent::Text(grapheme.to_owned())).collect(),
            intent => vec![intent],
        };
        for intent in batch.into_iter().flat_map(one_at_a_time) {
            let (session, target) = {
                let mut inner = one.inner();
                let Some(view) = inner.views.view(id) else { break };
                let Some(session) = view.session.clone() else { continue };
                (session, view.target.clone())
            };
            if hands.on != session {
                hands.let_go(&one);
                hands = Hands { on: session.clone(), noted: hands.noted, ..Hands::default() };
            }
            let typing = matches!(intent, Intent::Key(_) | Intent::Text(_));
            match hands.gate(&one, &session, &target, intent).await {
                Ok(Some((method, params))) => {
                    if press_on(&one, id, &session, method, params) && typing && !hands.focused(&one, &session, &target).await {
                        hands.note(&one, id, ONLY_TYPING);
                    }
                }
                Ok(None) => {}
                Err(why) => hands.note(&one, id, why),
            }
        }
        if one.inner().views.view(id).is_none() {
            break;
        }
    }
    hands.let_go(&one);
}

impl Hands {
    /// `intent` as the CDP call that goes into the check, None for one that
    /// goes nowhere unsaid, or why it went nowhere.
    async fn gate(&mut self, one: &One, session: &str, top: &str, intent: Intent) -> Result<Option<(&'static str, Value)>, &'static str> {
        let mouse = |kind: &str, (x, y): Point, button: &str, buttons: u32| {
            ("Input.dispatchMouseEvent", json!({"type": kind, "x": x, "y": y, "button": button, "buttons": buttons, "clickCount": 1}))
        };
        Ok(Some(match intent {
            Intent::Press { button, at } => {
                let Some(frame) = self.check_at(one, session, top, at).await else { return Err(ONLY_CLICKS) };
                self.held = Some((frame, button, at));
                self.latched = false;
                mouse("mousePressed", at, button, 1)
            }
            Intent::Move { at } => {
                if let Some((frame, button, last)) = &mut self.held {
                    if !inside(&frame.content, at) {
                        return Ok(None);
                    }
                    *last = at;
                    mouse("mouseMoved", at, button, 1)
                } else if self.hovers(one, session, top, at).await {
                    mouse("mouseMoved", at, "none", 0)
                } else {
                    return Ok(None);
                }
            }
            Intent::Release { at } => {
                let Some((frame, button, last)) = self.held.take() else { return Ok(None) };
                mouse("mouseReleased", if inside(&frame.content, at) { at } else { last }, button, 0)
            }
            Intent::Wheel { at, delta } => {
                if self.check_at(one, session, top, at).await.is_none() {
                    return Err(ONLY_CLICKS);
                }
                ("Input.dispatchMouseEvent", json!({"type": "mouseWheel", "x": at.0, "y": at.1, "deltaX": delta.0, "deltaY": delta.1}))
            }
            Intent::Key(params) => {
                if self.latched || !self.focused(one, session, top).await {
                    return Err(ONLY_TYPING);
                }
                ("Input.dispatchKeyEvent", params)
            }
            Intent::Text(text) => {
                if self.latched || !self.focused(one, session, top).await {
                    return Err(ONLY_TYPING);
                }
                ("Input.insertText", json!({"text": text}))
            }
        }))
    }

    /// The check-provider frame at `at`, when it's the innermost frame there
    /// by Chrome's hit test, down through the page's frames.
    async fn check_at(&mut self, one: &One, session: &str, top: &str, at: Point) -> Option<Innermost> {
        let calls = HostFrames { one, sessions: &self.sessions };
        let found = innermost_frame(&calls, session, top, at).await.filter(|found| check_frame(&found.address));
        self.found = Some((Instant::now(), found.clone()));
        found
    }

    /// Whether the pointer, not pressed, is over a check's frame: the frame
    /// last found, or found again once that's FOUND_FOR old.
    async fn hovers(&mut self, one: &One, session: &str, top: &str, at: Point) -> bool {
        if let Some((when, found)) = &self.found
            && when.elapsed() < FOUND_FOR
        {
            return found.as_ref().is_some_and(|found| inside(&found.content, at));
        }
        self.check_at(one, session, top, at).await.is_some()
    }

    /// Whether the page's focus is in one of the check's frames: the active
    /// element of each document from the top down, read in a world of the
    /// host's (where the page can't change what it reads), until it isn't a
    /// frame; that innermost frame is a check-provider's by its address as
    /// Chrome says. Once it isn't, `latched`.
    async fn focused(&mut self, one: &One, session: &str, top: &str) -> bool {
        let focused = self.focus_in_check(one, session, top).await;
        self.latched |= !focused;
        focused
    }

    async fn focus_in_check(&mut self, one: &One, session: &str, top: &str) -> bool {
        let (mut session, mut frame) = (session.to_owned(), top.to_owned());
        for _ in 0..DEPTH {
            let Some(node) = self.active_element(one, &session, &frame).await else { return false };
            let child = node["frameId"].as_str().filter(|_| matches!(node["nodeName"].as_str(), Some("IFRAME" | "FRAME")));
            let Some(child) = child.map(str::to_owned) else {
                return frame != top && frame_address(&HostFrames { one, sessions: &self.sessions }, &session, &frame).await.is_some_and(|address| check_frame(&address));
            };
            if let Some(inner) = (HostFrames { one, sessions: &self.sessions }).attach(&child).await {
                session = inner;
            }
            frame = child;
        }
        false
    }

    /// The active element of frame `frame`'s document, laid out in
    /// `session`: Chrome's description of it.
    async fn active_element(&mut self, one: &One, session: &str, frame: &str) -> Option<Value> {
        let key = (session.to_owned(), frame.to_owned());
        for _ in 0..2 {
            let context = match self.worlds.get(&key) {
                Some(context) => *context,
                None => {
                    let world = json!({"frameId": frame, "worldName": WORLD, "grantUniveralAccess": false});
                    let made = ask(one, session, "Page.createIsolatedWorld", world).await?;
                    let context = made["result"]["executionContextId"].as_i64()?;
                    if self.worlds.len() >= SESSIONS * 2 {
                        self.worlds.clear();
                    }
                    self.worlds.insert(key.clone(), context);
                    context
                }
            };
            let read = ask(one, session, "Runtime.evaluate", json!({"expression": "document.activeElement", "contextId": context})).await;
            let Some(read) = read.filter(|read| read["result"].get("exceptionDetails").is_none()) else {
                // The world went with the document it was made in.
                self.worlds.remove(&key);
                continue;
            };
            let object = read["result"]["result"]["objectId"].as_str()?;
            let mut node = ask(one, session, "DOM.describeNode", json!({"objectId": object})).await?;
            return Some(node["result"]["node"].take());
        }
        None
    }

    /// Detaches the host's sessions on the page's frames.
    fn let_go(&mut self, one: &One) {
        let sessions = std::mem::take(&mut *crate::locking::lock(&self.sessions));
        let mut inner = one.inner();
        for session in sessions.into_values().flatten() {
            send(&mut inner, "", "Target.detachFromTarget", json!({"sessionId": session}));
        }
    }

    /// Tells the view why its input went nowhere, at most once a second.
    fn note(&mut self, one: &One, id: u64, why: &str) {
        if self.noted.is_some_and(|noted| noted.elapsed() < NOTE_GAP) {
            return;
        }
        let mut inner = one.inner();
        let Some(view) = inner.views.view(id) else { return };
        let note = v1::AgentTabNote { agent_id: one.agent.clone(), request_seq: view.seq, message: why.to_owned() };
        if view.sender.try_send(Body::AgentTabNote(note)).is_ok() {
            self.noted = Some(Instant::now());
        }
    }
}

/// One of the person's calls into view `id`'s check, while it's still open
/// and the browser takes input, under the host's input lease: whether it
/// went.
fn press_on(one: &One, id: u64, session: &str, method: &'static str, params: Value) -> bool {
    let mut inner = one.inner();
    let Some(seq) = inner.views.view(id).map(|view| view.seq) else { return false };
    if !inner.views.checks.iter().any(|check| check.seq == seq)
        || !matches!(inner.state, State::Driven | State::PersonsTurn)
        || inner.mode != Some(Mode::Driven)
    {
        return false;
    }
    if !inner.lease.is_some_and(|(_, until)| Instant::now() < until) {
        inner.next_lease += 1;
        let lease = inner.next_lease;
        if control(&inner, &Control::LeaseInput { id: lease, for_ms: LEASE_MS }) {
            inner.lease = Some((lease, Instant::now() + Duration::from_millis(u64::from(LEASE_MS))));
        }
    }
    send(&mut inner, session, method, params).is_some()
}

fn inside([left, top, right, bottom]: &[f64; 4], (x, y): Point) -> bool {
    (*left..*right).contains(&x) && (*top..*bottom).contains(&y)
}

/// The keys typed with no character of their own, as Chrome's virtual key
/// codes.
const KEYS: &[(&str, u32)] = &[
    ("Enter", 13),
    ("Backspace", 8),
    ("Tab", 9),
    ("Escape", 27),
    ("Delete", 46),
    ("ArrowLeft", 37),
    ("ArrowUp", 38),
    ("ArrowRight", 39),
    ("ArrowDown", 40),
    ("Home", 36),
    ("End", 35),
    ("PageUp", 33),
    ("PageDown", 34),
];

/// The person's events as they'd go into the page, or why they're refused
/// whole: positions placed on the frame they were made on, the secondary
/// button and paste never, and keys held with Control or Command only for
/// select all and undo.
fn intents(view: &View, events: &[v1::AgentBrowserEvent]) -> Result<Vec<Intent>, String> {
    let (_, latest_w, latest_h) = view.sizes.back().copied().unwrap_or((0, 0.0, 0.0));
    view::check_input(events)?;
    let size = |frame: u64| {
        view.sizes.iter().find(|(seq, _, _)| *seq == frame).map_or((latest_w, latest_h), |(_, width, height)| (*width, *height))
    };
    let at = |x: f32, y: f32, (width, height): (f64, f64)| -> Option<Point> {
        if !x.is_finite() || !y.is_finite() || width <= 0.0 || height <= 0.0 {
            return None;
        }
        Some((f64::from(x.clamp(0.0, 1.0)) * (width - 1.0), f64::from(y.clamp(0.0, 1.0)) * (height - 1.0)))
    };
    let mut intents = Vec::new();
    for event in events {
        match &event.event {
            Some(Event::Pointer(pointer)) => {
                let button = match pointer.button {
                    0 | 1 => "left",
                    2 => "middle",
                    _ => continue,
                };
                let Some(at) = at(pointer.x, pointer.y, size(pointer.frame_seq)) else { continue };
                match pointer.kind() {
                    PointerKind::Move => intents.push(Intent::Move { at }),
                    PointerKind::Down => intents.push(Intent::Press { button, at }),
                    PointerKind::Up => intents.push(Intent::Release { at }),
                    PointerKind::Unspecified => {}
                }
            }
            Some(Event::Wheel(wheel)) => {
                let Some(at) = at(wheel.x, wheel.y, size(wheel.frame_seq)) else { continue };
                if !wheel.dx.is_finite() || !wheel.dy.is_finite() {
                    continue;
                }
                let delta = |pixels: f32| f64::from(pixels.clamp(-2000.0, 2000.0));
                intents.push(Intent::Wheel { at, delta: (delta(wheel.dx), delta(wheel.dy)) });
            }
            Some(Event::Key(key)) => intents.extend(key_event(key).map(Intent::Key)),
            Some(Event::Text(text)) if !text.is_empty() => intents.push(Intent::Text(text.clone())),
            Some(Event::Paste(_)) => return Err("Paste doesn't reach the check's page: type it instead.".to_owned()),
            _ => {}
        }
    }
    Ok(intents)
}

/// A key as Input.dispatchKeyEvent's parameters: a character, a key of
/// `KEYS`, or select all and undo with Control or Command; None for any
/// other chord.
fn key_event(key: &v1::AgentBrowserKey) -> Option<Value> {
    let chord = key.modifiers & (2 | 8) != 0;
    // CDP's modifiers: 1 Alt, 2 Control, 4 Meta, 8 Shift.
    let modifiers = [(1, 8), (2, 2), (4, 1), (8, 4)].iter().filter(|(ours, _)| key.modifiers & ours != 0).map(|(_, theirs)| theirs).sum::<u32>();
    let kind = if key.down { "keyDown" } else { "keyUp" };
    if chord {
        let command = match key.key.to_ascii_lowercase().as_str() {
            "a" => crate::errands::keys::SELECT_ALL,
            "z" => "undo",
            _ => return None,
        };
        let code = if command == "undo" { 90 } else { 65 };
        let mut event = json!({"type": kind, "key": key.key, "windowsVirtualKeyCode": code, "modifiers": modifiers});
        if key.down {
            event["commands"] = json!([command]);
        }
        return Some(event);
    }
    if let Some((name, vk)) = KEYS.iter().find(|(name, _)| *name == key.key) {
        let mut event = json!({"type": kind, "key": name, "code": name, "windowsVirtualKeyCode": vk, "modifiers": modifiers});
        if key.down && *name == "Enter" {
            event["text"] = json!("\r");
        }
        return Some(event);
    }
    let typed = crate::errands::keys::key(&key.key)?;
    let mut event = json!({"type": kind, "key": typed.key, "code": typed.code, "windowsVirtualKeyCode": typed.vk, "modifiers": modifiers});
    if key.down {
        event["text"] = json!(typed.key);
    }
    Some(event)
}

/// The agent handed the person a check (`seq` in its conversation) on tab
/// `target`, of site `domain`: views of it may open until `end_check`, and
/// `left` is told once if the tab leaves the site.
pub(crate) fn check_opened(factory: &Factory, agent: &str, seq: u64, target: &str, domain: &str, left: Box<dyn FnOnce(String) + Send>) {
    let one = factory.agent_browser.one(agent);
    let mut inner = one.inner();
    inner.views.checks.retain(|check| check.seq != seq);
    inner.views.checks.push(Check { seq, target: target.to_owned(), domain: domain.to_owned(), left: Some(left) });
}

/// The check at `seq` is over (answered, dismissed, taken back): its view
/// ends.
pub(crate) fn end_check(factory: &Factory, agent: &str, seq: u64) {
    let Some(one) = factory.agent_browser.get(agent) else { return };
    let id = {
        let mut inner = one.inner();
        inner.views.checks.retain(|check| check.seq != seq);
        inner.views.views.iter().find(|view| view.seq == seq).map(|view| view.id)
    };
    if let Some(id) = id {
        finish(factory, &one, id, End::RequestEnded, "The check is over.".to_owned());
    }
}

/// The tab of `agent`'s open check, if one is open: its target id.
pub(crate) fn check_tab(factory: &Factory, agent: &str) -> Option<String> {
    let one = factory.agent_browser.get(agent)?;
    let inner = one.inner();
    inner.views.checks.first().map(|check| check.target.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use v1::{AgentBrowserEvent, AgentBrowserKey, AgentBrowserPointer};

    fn view(sender: mpsc::Sender<Body>) -> View {
        View {
            id: 1,
            seq: 7,
            port: 1,
            sender,
            max_width: 640,
            target: "T".into(),
            session: Some("S".into()),
            site: "x.com".into(),
            origin: "https://x.com".into(),
            title: "Check".into(),
            frame_seq: 0,
            in_flight: None,
            unacked: None,
            last_sent: None,
            sizes: VecDeque::new(),
            input: None,
            input_window: (0, 0),
            stop: CancellationToken::new(),
        }
    }

    fn jpeg(width: u32, height: u32) -> String {
        let image = image::RgbImage::new(width, height);
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut bytes).encode_image(&image).expect("jpeg");
        STANDARD.encode(bytes)
    }

    #[test]
    fn a_site_is_its_registrable_domain_and_a_page_of_none_has_none() {
        assert_eq!(site_of("https://accounts.x.com/login?next=1"), "x.com");
        assert_eq!(site_of("https://pay.stripe.com"), "stripe.com");
        assert_eq!(site_of("https://www.bbc.co.uk/news"), "bbc.co.uk");
        assert_eq!(site_of("https://пример.рф/"), "xn--e1afmkfd.xn--p1ai");
        assert_eq!(site_of("http://127.0.0.1:8080/"), "127.0.0.1");
        assert_eq!(site_of("http://[::1]:8080/"), "[::1]");
        for none in ["about:blank", "data:text/html,<p>x.com</p>", "blob:https://x.com/1", "file:///x.com", "javascript:1", ""] {
            assert_eq!(site_of(none), "", "{none}");
        }
        assert_eq!(origin_of("https://accounts.x.com/login?next=1"), "https://accounts.x.com");
    }

    #[test]
    fn frames_go_one_at_a_time_within_their_bounds() {
        let one = One::new("yukako");
        let (runner, _to_runner) = mpsc::channel(64);
        let (sender, mut device) = mpsc::channel(8);
        {
            let mut inner = one.inner();
            inner.runner = Some(runner);
            inner.views.views.push(view(sender));
        }
        let params = |data: String, at: i64| json!({"data": data, "sessionId": at, "metadata": {"deviceWidth": 1280.0, "deviceHeight": 800.0}});
        frame(&one, 1, "S", &params(jpeg(640, 400), 1));
        let Ok(Body::AgentTabFrame(sent)) = device.try_recv() else { panic!("a frame") };
        assert_eq!((sent.agent_id.as_str(), sent.request_seq, sent.frame_seq, sent.width, sent.height), ("yukako", 7, 1, 640, 400));
        assert_eq!((sent.site.as_str(), sent.origin.as_str(), sent.title.as_str()), ("x.com", "https://x.com", "Check"));
        // None more until it's acked; wider than asked, or no JPEG, never.
        frame(&one, 1, "S", &params(jpeg(640, 400), 2));
        assert!(device.try_recv().is_err(), "one in flight");
        one.inner().views.view(1).expect("view").in_flight = None;
        frame(&one, 1, "S", &params(jpeg(641, 10), 3));
        frame(&one, 1, "S", &params(STANDARD.encode(b"not a jpeg"), 4));
        assert!(device.try_recv().is_err(), "out of bounds");
        let huge = STANDARD.encode(vec![0u8; MAX_JPEG + 1]);
        frame(&one, 1, "S", &params(huge, 5));
        assert!(device.try_recv().is_err());
        frame(&one, 1, "S", &params(jpeg(320, 200), 6));
        let Ok(Body::AgentTabFrame(next)) = device.try_recv() else { panic!("the next frame") };
        assert_eq!(next.frame_seq, 2);
    }

    fn pointer(kind: PointerKind, button: u32, frame_seq: u64) -> AgentBrowserEvent {
        AgentBrowserEvent { event: Some(Event::Pointer(AgentBrowserPointer { kind: kind.into(), x: 0.5, y: 0.25, button, frame_seq })) }
    }

    fn key(key: &str, modifiers: u32) -> AgentBrowserEvent {
        AgentBrowserEvent { event: Some(Event::Key(AgentBrowserKey { down: true, key: key.into(), modifiers })) }
    }

    #[test]
    fn input_is_placed_on_its_frame_and_takes_no_secondary_button_paste_or_chord() {
        let (sender, _device) = mpsc::channel(1);
        let mut view = view(sender);
        view.sizes.push_back((3, 1001.0, 801.0));
        view.sizes.push_back((4, 501.0, 401.0));
        let events = [
            pointer(PointerKind::Down, 1, 3),
            pointer(PointerKind::Move, 0, 3),
            pointer(PointerKind::Up, 1, 4),
            pointer(PointerKind::Down, 3, 4),
            key("a", 8),
            key("v", 8),
            key("t", 2),
            key("Enter", 0),
            key("q", 1),
            key("F12", 0),
            AgentBrowserEvent { event: Some(Event::Text("héllo".into())) },
        ];
        let intents = intents(&view, &events).expect("taken");
        assert_eq!(intents.len(), 7, "{intents:#?}");
        assert_eq!(intents[0], Intent::Press { button: "left", at: (500.0, 200.0) }, "on frame 3's size");
        assert_eq!(intents[1], Intent::Move { at: (500.0, 200.0) });
        assert_eq!(intents[2], Intent::Release { at: (250.0, 100.0) }, "on frame 4's size");
        let Intent::Key(select) = &intents[3] else { panic!("{intents:#?}") };
        assert_eq!(select["commands"], json!(["selectAll"]), "Command-A selects all; Command-V and Control-T go nowhere");
        let Intent::Key(enter) = &intents[4] else { panic!("{intents:#?}") };
        assert_eq!((enter["key"].as_str(), enter["text"].as_str()), (Some("Enter"), Some("\r")));
        let Intent::Key(q) = &intents[5] else { panic!("{intents:#?}") };
        assert_eq!((q["key"].as_str(), q["code"].as_str(), q["modifiers"].as_u64()), (Some("q"), Some("KeyQ"), Some(8)));
        assert_eq!(intents[6], Intent::Text("héllo".into()));

        let paste = AgentBrowserEvent { event: Some(Event::Paste("secret".into())) };
        assert!(super::intents(&view, &[paste]).is_err(), "no clipboard");
        let long = AgentBrowserEvent { event: Some(Event::Text("x".repeat(4097))) };
        assert!(super::intents(&view, &[long]).is_err());
    }

    /// A check's page as Chrome lays it out, in the host's sessions: on
    /// x.com (session S), hCaptcha's frame F at 0..300 x 0..80 and the
    /// page's own field everywhere else, and the site's own challenge frame
    /// O at 0..400 x 100..400, of another site, holding reCAPTCHA's frame F2
    /// at 50..350 x 150..230, a field of its own at 50..350 x 300..340, and
    /// a frame R at 50..350 x 350..390 whose address only says "recaptcha";
    /// and the page's own frame D (about:srcdoc, in the page's process) at
    /// 0..400 x 420..600, holding hCaptcha's frame F3 at 50..350 x 450..530
    /// and a field of its own. Every frame of another site is out of
    /// process, with a session of its own. `focus` is where the focus is:
    /// "page", "check" (F), "outer" (O's field), "nested" (F2), "fake" (R),
    /// "srcdoc" (D's field) or "srcdoc-check" (F3); a press into F, F2 or
    /// F3 puts it there. With `steal`, the page takes the focus to its field as soon
    /// as a key reaches the check, and gives it back once that's been seen.
    /// Each of the host's Input calls is kept.
    struct Page {
        focus: std::sync::Mutex<&'static str>,
        steal: std::sync::atomic::AtomicU8,
        typed: std::sync::Mutex<Vec<Value>>,
    }

    impl Page {
        fn focus(&self, focus: &'static str) {
            *self.focus.lock().unwrap() = focus;
        }

        fn answer(&self, session: &str, method: &str, params: &Value) -> Value {
            use std::sync::atomic::Ordering::SeqCst;
            let urls = [
                ("T", "https://x.com/"),
                ("F", "https://newassets.hcaptcha.com/captcha/v1/1/static/hcaptcha.html"),
                ("O", "https://challenge.x-static.net/checkpoint"),
                ("F2", "https://www.google.com/recaptcha/api2/anchor?k=1"),
                ("R", "https://evil.example/recaptcha/api2/anchor"),
                ("D", "about:srcdoc"),
                ("F3", "https://newassets.hcaptcha.com/captcha/v1/2/static/hcaptcha.html"),
            ];
            let url = |id: &str| urls.iter().find(|(known, _)| *known == id).map(|(_, url)| *url);
            let element = |node: i64| match node {
                10 => json!({"nodeName": "IFRAME", "frameId": "F"}),
                30 => json!({"nodeName": "IFRAME", "frameId": "O"}),
                40 => json!({"nodeName": "IFRAME", "frameId": "F2"}),
                42 => json!({"nodeName": "IFRAME", "frameId": "R"}),
                80 => json!({"nodeName": "IFRAME", "frameId": "F3"}),
                82 => json!({"nodeName": "IFRAME", "frameId": "D"}),
                20 | 41 | 81 => json!({"nodeName": "INPUT"}),
                _ => json!({"nodeName": "DIV"}),
            };
            let within = |at: Point, area: [f64; 4]| inside(&area, at);
            match method {
                "DOM.getNodeForLocation" => {
                    let at = (params["x"].as_f64().unwrap(), params["y"].as_f64().unwrap());
                    let (node, frame) = match session {
                        "S" if within(at, [0.0, 0.0, 300.0, 80.0]) => (10, "T"),
                        "S" if within(at, [0.0, 100.0, 400.0, 400.0]) => (30, "T"),
                        "S" if within(at, [50.0, 450.0, 350.0, 530.0]) => (80, "D"),
                        "S" if within(at, [0.0, 420.0, 400.0, 600.0]) => (81, "D"),
                        "S" => (20, "T"),
                        "SO" if within(at, [50.0, 50.0, 350.0, 130.0]) => (40, "O"),
                        "SO" if within(at, [50.0, 200.0, 350.0, 240.0]) => (41, "O"),
                        "SO" if within(at, [50.0, 250.0, 350.0, 290.0]) => (42, "O"),
                        "SO" => (43, "O"),
                        "SF2" => (50, "F2"),
                        "SR" => (60, "R"),
                        "SF" => (70, "F"),
                        "SF3" => (90, "F3"),
                        _ => return Value::Null,
                    };
                    json!({"backendNodeId": node, "frameId": frame})
                }
                "DOM.describeNode" => {
                    let node = params["backendNodeId"].as_i64().or_else(|| params["objectId"].as_str()?.strip_prefix("node:")?.parse().ok());
                    json!({"node": element(node.unwrap_or_default())})
                }
                "DOM.getBoxModel" => {
                    let [left, top, right, bottom] = match params["backendNodeId"].as_i64() {
                        Some(10) => [0.0, 0.0, 300.0, 80.0],
                        Some(30) => [0.0, 100.0, 400.0, 400.0],
                        Some(40) => [50.0, 50.0, 350.0, 130.0],
                        Some(42) => [50.0, 250.0, 350.0, 290.0],
                        Some(80) => [50.0, 450.0, 350.0, 530.0],
                        Some(82) => [0.0, 420.0, 400.0, 600.0],
                        _ => return Value::Null,
                    };
                    let quad = json!([left, top, right, top, right, bottom, left, bottom]);
                    json!({"model": {"content": quad, "padding": quad, "border": quad, "margin": quad}})
                }
                "DOM.getFrameOwner" => match params["frameId"].as_str() {
                    Some("F") => json!({"backendNodeId": 10}),
                    Some("O") => json!({"backendNodeId": 30}),
                    Some("F2") => json!({"backendNodeId": 40}),
                    Some("R") => json!({"backendNodeId": 42}),
                    Some("F3") => json!({"backendNodeId": 80}),
                    Some("D") => json!({"backendNodeId": 82}),
                    _ => Value::Null,
                },
                "Target.attachToTarget" => match params["targetId"].as_str() {
                    Some(frame @ ("F" | "O" | "F2" | "R" | "F3")) => json!({"sessionId": format!("S{frame}")}),
                    _ => Value::Null,
                },
                "Target.detachFromTarget" => json!({}),
                "Page.getFrameTree" => {
                    let (id, parent) = match session {
                        "S" => ("T", None),
                        "SO" => ("O", Some("T")),
                        "SF2" => ("F2", Some("O")),
                        "SR" => ("R", Some("O")),
                        "SF" => ("F", Some("T")),
                        "SF3" => ("F3", Some("D")),
                        _ => return Value::Null,
                    };
                    let children = if id == "T" { json!([{"frame": {"id": "D", "url": url("D").unwrap(), "parentId": "T"}}]) } else { json!([]) };
                    json!({"frameTree": {"frame": {"id": id, "url": url(id).unwrap(), "parentId": parent}, "childFrames": children}})
                }
                "Target.getTargets" => {
                    {
                    let targets = ["F", "O", "F2", "R", "F3"].map(|id| json!({"targetId": id, "type": "iframe", "url": url(id).unwrap()}));
                    json!({"targetInfos": targets})
                }
                }
                "Target.getTargetInfo" => match params["targetId"].as_str().and_then(|id| Some((id, url(id)?))) {
                    Some((id, url)) => json!({"targetInfo": {"targetId": id, "type": "iframe", "url": url}}),
                    None => Value::Null,
                },
                "Page.createIsolatedWorld" => match (session, params["frameId"].as_str()) {
                    ("S", Some("T")) => json!({"executionContextId": 5}),
                    ("SO", Some("O")) => json!({"executionContextId": 6}),
                    ("SF2", Some("F2")) => json!({"executionContextId": 7}),
                    ("SF", Some("F")) => json!({"executionContextId": 8}),
                    ("SR", Some("R")) => json!({"executionContextId": 9}),
                    ("S", Some("D")) => json!({"executionContextId": 10}),
                    ("SF3", Some("F3")) => json!({"executionContextId": 11}),
                    _ => Value::Null,
                },
                "Runtime.evaluate" => {
                    let focus = *self.focus.lock().unwrap();
                    let active = match (params["contextId"].as_i64(), focus) {
                        (Some(5), "page") => 20,
                        (Some(5), "check") => 10,
                        (Some(5), "srcdoc" | "srcdoc-check") => 82,
                        (Some(5), _) => 30,
                        (Some(10), "srcdoc") => 81,
                        (Some(10), _) => 80,
                        (Some(6), "outer") => 41,
                        (Some(6), "nested") => 40,
                        (Some(6), "fake") => 42,
                        (Some(7..=9 | 11), _) => 99,
                        _ => return Value::Null,
                    };
                    if params["contextId"] == 5 && self.steal.compare_exchange(2, 0, SeqCst, SeqCst).is_ok() {
                        self.focus("check");
                    }
                    json!({"result": {"type": "object", "subtype": "node", "objectId": format!("node:{active}")}})
                }
                method if method.starts_with("Input.") => {
                    self.typed.lock().unwrap().push(json!({"method": method, "params": params}));
                    if params["type"] == "mousePressed" {
                        let at = (params["x"].as_f64().unwrap(), params["y"].as_f64().unwrap());
                        if within(at, [0.0, 0.0, 300.0, 80.0]) {
                            self.focus("check");
                        } else if within(at, [50.0, 150.0, 350.0, 230.0]) {
                            self.focus("nested");
                        } else if within(at, [50.0, 450.0, 350.0, 530.0]) {
                            self.focus("srcdoc-check");
                        }
                    } else if method != "Input.dispatchMouseEvent" && self.steal.compare_exchange(1, 2, SeqCst, SeqCst).is_ok() {
                        self.focus("page");
                    }
                    json!({})
                }
                _ => Value::Null,
            }
        }
    }

    /// Chrome answering the host's calls on `page`.
    fn chrome(one: Arc<One>, mut calls: mpsc::Receiver<(crate::browser_runner::wire::Channel, Vec<u8>)>, page: Arc<Page>) {
        tokio::spawn(async move {
            while let Some((channel, bytes)) = calls.recv().await {
                if channel != crate::browser_runner::wire::Channel::Cdp {
                    continue;
                }
                let call: Value = serde_json::from_slice(&bytes).expect("json");
                let result = page.answer(call["sessionId"].as_str().unwrap_or_default(), call["method"].as_str().unwrap_or_default(), &call["params"]);
                let answer = if result.is_null() { json!({"id": call["id"], "error": {"message": "no"}}) } else { json!({"id": call["id"], "result": result}) };
                if let Some(waiting) = one.inner().waiting.remove(&call["id"].as_u64().unwrap_or_default()) {
                    let _ = waiting.send(answer);
                }
            }
        });
    }

    /// Runs `batch` through a check view's hands, and what went into the
    /// page.
    async fn into_check(one: &Arc<One>, page: &Page, batch: Vec<Intent>) -> Vec<Value> {
        page.typed.lock().unwrap().clear();
        let (input, batches) = mpsc::channel(BATCHES);
        let taking = tokio::spawn(hands(one.clone(), 1, batches));
        input.send(batch).await.unwrap();
        drop(input);
        taking.await.unwrap();
        // Chrome takes the host's calls in order: once this one is answered,
        // every one before it was taken.
        let _ = ask(one, "", "Target.getTargets", json!({})).await;
        page.typed.lock().unwrap().clone()
    }

    /// Check 7 open on `page`, viewed from the device whose messages come
    /// on what's returned.
    fn check_on_page() -> (Arc<One>, Arc<Page>, mpsc::Receiver<Body>) {
        let one = Arc::new(One::new("yukako"));
        let (runner, calls) = mpsc::channel(256);
        let (sender, device) = mpsc::channel(8);
        {
            let mut inner = one.inner();
            inner.runner = Some(runner);
            inner.state = State::PersonsTurn;
            inner.mode = Some(Mode::Driven);
            inner.views.views.push(view(sender));
            inner.views.checks.push(Check { seq: 7, target: "T".into(), domain: "x.com".into(), left: None });
        }
        let page = Arc::new(Page { focus: std::sync::Mutex::new("page"), steal: 0.into(), typed: Default::default() });
        chrome(one.clone(), calls, page.clone());
        (one, page, device)
    }

    /// A check the page draws itself (a "#px-captcha" box to press and hold,
    /// a password field beside it): none of the person's input reaches it,
    /// however they press, hold, drag or type.
    #[tokio::test]
    async fn a_page_faking_a_check_gets_none_of_the_persons_input() {
        let (one, page, mut device) = check_on_page();
        let (fake_box, password) = ((500.0, 40.0), (600.0, 60.0));
        let tried = into_check(&one, &page, vec![
            Intent::Move { at: fake_box },
            Intent::Press { button: "left", at: fake_box },
            Intent::Move { at: (510.0, 42.0) },
            Intent::Release { at: (510.0, 42.0) },
            Intent::Press { button: "left", at: password },
            Intent::Release { at: password },
            Intent::Key(key_event(&AgentBrowserKey { down: true, key: "p".into(), modifiers: 0 }).unwrap()),
            Intent::Text("hunter2".into()),
            Intent::Key(key_event(&AgentBrowserKey { down: true, key: "Enter".into(), modifiers: 0 }).unwrap()),
            Intent::Wheel { at: password, delta: (0.0, 300.0) },
        ])
        .await;
        assert_eq!(tried, Vec::<Value>::new());
        let Ok(Body::AgentTabNote(note)) = device.try_recv() else { panic!("the view is told") };
        assert_eq!(note.message, ONLY_CLICKS);
    }

    #[tokio::test]
    async fn a_checks_input_reaches_only_the_check_never_a_form_beside_it() {
        let (one, page, mut device) = check_on_page();
        let kinds = |calls: &[Value]| calls.iter().map(|call| call["params"]["type"].as_str().unwrap_or("text").to_owned()).collect::<Vec<_>>();
        let shown = |calls: &[Value]| {
            calls.iter().map(|call| call["params"]["text"].as_str().or(call["params"]["type"].as_str()).unwrap_or_default().to_owned()).collect::<Vec<_>>()
        };
        let press = |at: Point| vec![Intent::Press { button: "left", at }, Intent::Release { at }];

        let on_the_form = into_check(&one, &page, vec![
            Intent::Press { button: "left", at: (500.0, 40.0) },
            Intent::Move { at: (510.0, 40.0) },
            Intent::Release { at: (510.0, 40.0) },
            Intent::Wheel { at: (500.0, 40.0), delta: (0.0, 100.0) },
        ])
        .await;
        assert_eq!(on_the_form, Vec::<Value>::new(), "a click on the page beside the check does nothing");
        let Ok(Body::AgentTabNote(note)) = device.try_recv() else { panic!("the view is told") };
        assert_eq!((note.request_seq, note.message.as_str()), (7, ONLY_CLICKS));
        assert!(device.try_recv().is_err(), "once");

        let in_the_check = into_check(&one, &page, vec![
            Intent::Move { at: (100.0, 10.0) },
            Intent::Press { button: "left", at: (150.0, 40.0) },
            Intent::Move { at: (200.0, 50.0) },
            Intent::Move { at: (500.0, 40.0) },
            Intent::Release { at: (500.0, 40.0) },
            Intent::Wheel { at: (100.0, 10.0), delta: (0.0, 100.0) },
        ])
        .await;
        assert_eq!(kinds(&in_the_check), ["mouseMoved", "mousePressed", "mouseMoved", "mouseReleased", "mouseWheel"]);
        assert_eq!(in_the_check[2]["params"]["buttons"], 1, "a drag inside the check");
        let released = &in_the_check[3]["params"];
        assert_eq!((released["x"].as_f64(), released["y"].as_f64()), (Some(200.0), Some(50.0)), "let go where it last was inside");

        // Inside the site's own frame: its reCAPTCHA takes the press and a
        // drag within it; the frame's own field, the frame around it and a
        // frame that only says "recaptcha" take nothing.
        let nested = into_check(&one, &page, vec![
            Intent::Press { button: "left", at: (100.0, 180.0) },
            Intent::Move { at: (300.0, 220.0) },
            Intent::Move { at: (300.0, 320.0) },
            Intent::Release { at: (300.0, 320.0) },
        ])
        .await;
        assert_eq!(kinds(&nested), ["mousePressed", "mouseMoved", "mouseReleased"], "{nested:#?}");
        assert_eq!(nested[2]["params"]["y"].as_f64(), Some(220.0), "a drag stays in the frame it began in");
        assert_eq!(into_check(&one, &page, press((100.0, 320.0))).await, Vec::<Value>::new(), "the outer frame's own field");
        assert_eq!(into_check(&one, &page, press((100.0, 370.0))).await, Vec::<Value>::new(), "a frame that only says recaptcha");
        assert_eq!(into_check(&one, &page, press((390.0, 120.0))).await, Vec::<Value>::new(), "the outer frame around it");
        // And inside the page's own frame, in its process.
        assert_eq!(kinds(&into_check(&one, &page, press((100.0, 500.0))).await), ["mousePressed", "mouseReleased"], "a check in the page's own frame");
        assert_eq!(into_check(&one, &page, press((100.0, 560.0))).await, Vec::<Value>::new(), "the page's own frame's field");

        let typing = vec![Intent::Key(key_event(&AgentBrowserKey { down: true, key: "h".into(), modifiers: 0 }).unwrap()), Intent::Text("hunter2".into())];
        while device.try_recv().is_ok() {}
        for elsewhere in ["page", "outer", "fake", "srcdoc"] {
            page.focus(elsewhere);
            assert_eq!(into_check(&one, &page, typing.clone()).await, Vec::<Value>::new(), "typing with the focus in {elsewhere} goes nowhere");
            let Ok(Body::AgentTabNote(note)) = device.try_recv() else { panic!("the view is told") };
            assert_eq!(note.message, ONLY_TYPING);
        }
        for check in ["check", "nested", "srcdoc-check"] {
            page.focus(check);
            let typed_in = into_check(&one, &page, typing.clone()).await;
            assert_eq!(shown(&typed_in), ["h", "h", "u", "n", "t", "e", "r", "2"], "a key, then the text a grapheme at a time, into {check}");
        }

        // A page that takes the focus back the moment the check has it gets
        // one character at most: no more keys until a press goes into the
        // check again, even with the focus back there.
        page.focus("check");
        page.steal.store(1, std::sync::atomic::Ordering::SeqCst);
        let mut stolen = vec![Intent::Text("secret".into()), typing[0].clone()];
        stolen.extend(press((150.0, 40.0)));
        stolen.push(Intent::Text("ok".into()));
        let stolen = into_check(&one, &page, stolen).await;
        assert_eq!(shown(&stolen), ["s", "mousePressed", "mouseReleased", "o", "k"]);
        let Ok(Body::AgentTabNote(note)) = device.try_recv() else { panic!("the view is told") };
        assert_eq!(note.message, ONLY_TYPING);

        one.inner().views.checks.clear();
        assert_eq!(into_check(&one, &page, typing).await, Vec::<Value>::new(), "nothing once the check is over");
    }
}
