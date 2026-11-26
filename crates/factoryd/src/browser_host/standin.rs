//! The Mac dev stand-in (#435 P4, SPLIT.md "Dev aid"): a runner in
//! factoryd's own process over this Mac's Chrome, headless, driven through
//! its DevTools pipe, speaking C1 as the Linux runner does, so the app's
//! live view (P5) can be built before Linux runs. Frames come from CDP's
//! screencast and input goes in through CDP's Input domain; there's no
//! private screen, user, Landlock or firewall: it's only for development,
//! with its own throwaway profile in the dev home, and never undriven (the
//! mode is only reported).

use std::sync::Arc;
use std::time::Instant;

use base64::Engine;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::Io;
use crate::browser_runner::wire::{self, Button, Channel, Control, FrameHeader, InputEvent, Mode, Report};
use crate::factory::Factory;

/// Ids of the stand-in's own CDP calls: above anything a client uses, and
/// within the 32-bit integer Chrome takes.
const OWN_IDS: u64 = 1_000_000_000;

pub(crate) fn spawn(factory: &Arc<Factory>) -> Result<Io, String> {
    // An uncontained Chrome: only ever for the dev Claktory or a debug
    // build, never the real one (Fugo 37166).
    if super::profile(factory) != "dev" && !cfg!(debug_assertions) {
        return Err("Claktory's browser isn't available on this computer yet.".to_owned());
    }
    let profile = factory.home().root().join("browser-standin");
    let (to, from_host) = mpsc::channel::<(Channel, Vec<u8>)>(64);
    let (to_host, from) = mpsc::channel::<(Channel, Vec<u8>)>(64);
    let quit = tokio_util::sync::CancellationToken::new();
    let quitting = quit.clone();
    tokio::spawn(async move {
        let mut standin = Standin {
            profile,
            to_host,
            chrome: None,
            session: String::new(),
            next_id: OWN_IDS,
            screencast: None,
            seq: 0,
            size: super::DESKTOP,
            view: None,
            casting: false,
            events: None,
            later: std::collections::VecDeque::new(),
            mode: None,
        };
        standin.run(from_host, quitting).await;
    });
    Ok(Io { to, from, quit })
}

struct Chrome {
    /// Chrome itself, killed when dropped.
    _local: crate::errands::cdp::Local,
    send: mpsc::Sender<Vec<u8>>,
}

struct Standin {
    profile: std::path::PathBuf,
    to_host: mpsc::Sender<(Channel, Vec<u8>)>,
    chrome: Option<Chrome>,
    /// The page's CDP session.
    session: String,
    next_id: u64,
    /// The screencast frame waiting for its ack: its session id.
    screencast: Option<i64>,
    seq: u32,
    /// The page's size now.
    size: (u16, u16),
    /// The widest frame the viewer wants, while one watches.
    view: Option<u16>,
    /// Whether the screencast runs (Chrome refuses it while the page is
    /// between two documents: it's asked again once it navigated).
    casting: bool,
    /// Chrome's answers to its own calls, and the screencast's events.
    events: Option<mpsc::Receiver<Value>>,
    /// Events that came while it waited for an answer.
    later: std::collections::VecDeque<Value>,
    /// How Chrome runs, as asked: a change takes the runner a moment (it
    /// starts Chrome again), which the stand-in takes too.
    mode: Option<Mode>,
}

/// About how long the runner takes to start Chrome again in another mode.
const RELAUNCH: std::time::Duration = std::time::Duration::from_millis(300);

impl Standin {
    async fn report(&self, report: Report) {
        let _ = self.to_host.send((Channel::Control, wire::encode(&report))).await;
    }

    async fn run(&mut self, mut from_host: mpsc::Receiver<(Channel, Vec<u8>)>, quit: tokio_util::sync::CancellationToken) {
        loop {
            while let Some(message) = self.later.pop_front() {
                self.on_chrome(message).await;
            }
            let event = async {
                match &mut self.events {
                    Some(events) => events.recv().await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                () = quit.cancelled() => break,
                message = from_host.recv() => {
                    let Some((channel, payload)) = message else { break };
                    if !self.on_host(channel, &payload).await {
                        break;
                    }
                }
                Some(message) = event => self.on_chrome(message).await,
            }
        }
        self.chrome = None;
    }

    /// What the host sends; false once the stand-in ends.
    async fn on_host(&mut self, channel: Channel, payload: &[u8]) -> bool {
        match channel {
            Channel::Control => {
                let Some(control) = wire::decode::<Control>(payload) else { return true };
                match control {
                    Control::Start { mode, url, width, height, .. } => {
                        if self.chrome.is_some() {
                            return true;
                        }
                        if let Err(what) = self.start(width, height).await {
                            self.report(Report::Error { what, fatal: true }).await;
                            return false;
                        }
                        if let Some(url) = url {
                            self.call("Page.navigate", json!({"url": url})).await;
                        }
                        let chrome = self.version().await;
                        let downloads = self.downloads();
                        let download_dir = std::fs::create_dir_all(&downloads).is_ok().then(|| downloads.display().to_string());
                        self.mode = Some(mode);
                        self.report(Report::Ready { wire: wire::VERSION, chrome, landlock_abi: 0, mode, download_dir }).await;
                    }
                    Control::Mode { mode, url } => {
                        if self.mode.replace(mode) != Some(mode) {
                            tokio::time::sleep(RELAUNCH).await;
                        }
                        if let Some(url) = url {
                            self.call("Page.navigate", json!({"url": url})).await;
                        }
                        self.report(Report::ModeChanged { mode }).await;
                    }
                    Control::View { on: true, max_width, .. } => {
                        self.view = Some(max_width);
                        self.casting = false;
                        self.cast().await;
                    }
                    Control::View { on: false, .. } => {
                        self.view = None;
                        self.casting = false;
                        self.screencast = None;
                        self.call("Page.stopScreencast", json!({})).await;
                    }
                    Control::Resize { width, height } => {
                        let mobile = width < height;
                        let params = json!({"width": width, "height": height, "deviceScaleFactor": 1, "mobile": mobile});
                        self.call("Emulation.setDeviceMetricsOverride", params).await;
                        self.size = (width, height);
                        self.report(Report::Resized { width, height }).await;
                    }
                    // No clipboard watch or downloads in the stand-in.
                    Control::Clipboard { .. } => {}
                    // Nor an input gate: every lease is granted (dev only).
                    Control::LeaseInput { id, .. } => self.report(Report::Lease { id, granted: true, why: String::new() }).await,
                    Control::EndLease { .. } => {}
                    Control::DownloadRead { guid } => self.download(&guid).await,
                    Control::DownloadDrop { guid } => {
                        if wire::guid_ok(&guid) {
                            let _ = std::fs::remove_file(self.downloads().join(&guid));
                        }
                    }
                    Control::Stop => {
                        let started = Instant::now();
                        self.call("Browser.close", json!({})).await;
                        self.report(Report::Stopped { ms: started.elapsed().as_millis() as u64 }).await;
                        return false;
                    }
                    Control::DeleteData { .. } => {
                        self.call("Browser.close", json!({})).await;
                        self.chrome = None;
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        let _ = std::fs::remove_dir_all(&self.profile);
                        self.report(Report::DataDeleted).await;
                        self.report(Report::Stopped { ms: 0 }).await;
                        return false;
                    }
                }
            }
            Channel::Frames => {
                if wire::read_ack(payload).is_some()
                    && let Some(session) = self.screencast.take()
                {
                    self.call("Page.screencastFrameAck", json!({"sessionId": session})).await;
                }
            }
            Channel::Input => {
                if let Some(event) = InputEvent::parse(payload) {
                    self.input(event).await;
                }
            }
            Channel::Clipboard => {
                let text = String::from_utf8_lossy(payload).into_owned();
                self.call("Input.insertText", json!({"text": text})).await;
            }
            // A client's own CDP: as it is, to the browser.
            Channel::Cdp => self.send(payload).await,
            Channel::Files => {}
        }
        true
    }

    async fn start(&mut self, width: u16, height: u16) -> Result<(), String> {
        self.size = (width, height);
        use crate::errands::cdp::Transport;
        let (events, received) = mpsc::channel::<Value>(256);
        self.events = Some(received);
        std::fs::create_dir_all(&self.profile).map_err(|error| error.to_string())?;
        preferences(&self.profile).map_err(|error| format!("couldn't set Chrome up: {error}"))?;
        let local = crate::errands::cdp::Local::start(std::path::Path::new(super::setup::MAC_CHROME), &self.profile)
            .map_err(|error| format!("couldn't start Chrome: {error}"))?;
        let crate::errands::cdp::Pipe { send, mut receive } = local.connect()?;
        let to_host = self.to_host.clone();
        tokio::spawn(async move {
            while let Some(message) = receive.recv().await {
                let Ok(value) = serde_json::from_slice::<Value>(&message) else { continue };
                // Its own calls' answers and the screencast go to the
                // stand-in; the rest to whoever drives it.
                let own = value["id"].as_u64().is_some_and(|id| (OWN_IDS..=super::sites::HOST_IDS).contains(&id))
                    || value["method"].as_str().is_some_and(|method| method.starts_with("Page.screencast"));
                // A page that navigated: the stand-in hears it too (a
                // screencast refused mid-navigation starts then).
                let navigated = matches!(value["method"].as_str(), Some("Page.frameNavigated" | "Page.loadEventFired"));
                if navigated && events.send(value.clone()).await.is_err() {
                    return;
                }
                if own {
                    if events.send(value).await.is_err() {
                        return;
                    }
                } else if to_host.send((Channel::Cdp, message)).await.is_err() {
                    return;
                }
            }
            let _ = to_host.send((Channel::Control, wire::encode(&Report::ChromeExited { code: None }))).await;
        });
        self.chrome = Some(Chrome { _local: local, send });
        // The page to show: one of its own (the first may not be there yet).
        let created = self.call_wait("Target.createTarget", json!({"url": "about:blank"}), false).await;
        let Some(page) = created["result"]["targetId"].as_str().map(str::to_owned) else {
            return Err("Chrome opened no page".to_owned());
        };
        let attached = self.call_wait("Target.attachToTarget", json!({"targetId": page, "flatten": true}), false).await;
        self.session = attached["result"]["sessionId"].as_str().unwrap_or_default().to_owned();
        self.call("Page.enable", json!({})).await;
        let params = json!({"width": width, "height": height, "deviceScaleFactor": 1, "mobile": false});
        self.call("Emulation.setDeviceMetricsOverride", params).await;
        Ok(())
    }

    /// Where its Chrome's downloads land (the errand sets it so).
    fn downloads(&self) -> std::path::PathBuf {
        self.profile.with_file_name("browser-standin-downloads")
    }

    /// Sends download `guid` on channel 5, as the runner does.
    async fn download(&mut self, guid: &str) {
        let Some(name) = wire::guid_bytes(guid) else { return };
        let Ok(bytes) = std::fs::read(self.downloads().join(guid)) else {
            self.report(Report::DownloadCancelled { guid: guid.to_owned(), why: "it isn't there".to_owned() }).await;
            return;
        };
        for chunk in bytes.chunks(wire::MAX_FILE_CHUNK) {
            let mut payload = name.to_vec();
            payload.extend_from_slice(chunk);
            if self.to_host.send((Channel::Files, payload)).await.is_err() {
                return;
            }
        }
        let _ = self.to_host.send((Channel::Files, name.to_vec())).await;
        let sha256 = crate::encoding::hex(&<sha2::Sha256 as sha2::Digest>::digest(&bytes));
        self.report(Report::DownloadDone { guid: guid.to_owned(), bytes: bytes.len() as u64, sha256 }).await;
    }

    async fn version(&mut self) -> String {
        let version = self.call_wait("Browser.getVersion", json!({}), false).await;
        version["result"]["product"].as_str().unwrap_or_default().trim_start_matches("HeadlessChrome/").to_owned()
    }

    async fn send(&mut self, message: &[u8]) {
        if let Some(chrome) = &self.chrome {
            let _ = chrome.send.send(message.to_vec()).await;
        }
    }

    /// One of its own calls on the page, not waited for.
    async fn call(&mut self, method: &str, params: Value) {
        self.next_id += 1;
        let mut message = json!({"id": self.next_id, "method": method, "params": params});
        if !self.session.is_empty() {
            message["sessionId"] = json!(self.session);
        }
        let bytes = serde_json::to_vec(&message).unwrap_or_default();
        self.send(&bytes).await;
    }

    /// One of its own calls, answered (by the browser itself unless
    /// `on_page`): Null if Chrome went or took over 10 s. Events meanwhile
    /// wait their turn.
    async fn call_wait(&mut self, method: &str, params: Value, on_page: bool) -> Value {
        let session = if on_page { self.session.clone() } else { std::mem::take(&mut self.session) };
        self.call(method, params).await;
        self.session = session;
        let id = self.next_id;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let Some(events) = &mut self.events else { return Value::Null };
            match tokio::time::timeout_at(deadline, events.recv()).await {
                Ok(Some(message)) if message["id"].as_u64() == Some(id) => return message,
                Ok(Some(message)) if message["id"].is_null() => self.later.push_back(message),
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => return Value::Null,
            }
        }
    }

    async fn on_chrome(&mut self, message: Value) {
        if matches!(message["method"].as_str(), Some("Page.frameNavigated" | "Page.loadEventFired")) {
            self.cast().await;
            return;
        }
        if message["method"] == "Page.screencastFrame" {
            let params = &message["params"];
            let Some(data) = params["data"].as_str() else { return };
            self.screencast = params["sessionId"].as_i64();
            let width = params["metadata"]["deviceWidth"].as_f64().map(|width| width as u16);
            let height = params["metadata"]["deviceHeight"].as_f64().map(|height| height as u16);
            // One it can't read is acked here, or Chrome would wait for
            // the host's ack forever.
            if !self.frame(data, width, height).await
                && let Some(session) = self.screencast.take()
            {
                self.call("Page.screencastFrameAck", json!({"sessionId": session})).await;
            }
        }
    }

    /// Starts the screencast the viewer wants, unless it runs; Chrome
    /// refuses it while the page is between two documents.
    async fn cast(&mut self) {
        let Some(max_width) = self.view.filter(|_| !self.casting) else { return };
        let params = json!({"format": "jpeg", "quality": 60, "maxWidth": max_width, "maxHeight": 4096, "everyNthFrame": 1});
        let answer = self.call_wait("Page.startScreencast", params, true).await;
        if answer["result"].is_object() {
            self.casting = true;
            self.first_frame(max_width).await;
        }
    }

    /// A headless Chrome sends no screencast frame until the page paints
    /// again, so an idle page would show nothing: the first frame is a
    /// screenshot, scaled to fit `max_width`.
    async fn first_frame(&mut self, max_width: u16) {
        let (width, height) = self.size;
        let scale = if width > max_width { f64::from(max_width.saturating_sub(1)) / f64::from(width) } else { 1.0 };
        let clip = json!({"x": 0, "y": 0, "width": width, "height": height, "scale": scale});
        let shot = self.call_wait("Page.captureScreenshot", json!({"format": "jpeg", "quality": 60, "clip": clip}), true).await;
        if let Some(data) = shot["result"]["data"].as_str() {
            self.frame(data, Some(width), Some(height)).await;
        }
    }

    /// One frame to the host: a base64 JPEG on a screen of the size given
    /// (or its own); false when it isn't one.
    async fn frame(&mut self, data: &str, screen_width: Option<u16>, screen_height: Option<u16>) -> bool {
        let Ok(jpeg) = base64::engine::general_purpose::STANDARD.decode(data) else { return false };
        let Some((width, height)) = super::view::jpeg_size(&jpeg) else { return false };
        self.seq = self.seq.wrapping_add(1);
        let header = FrameHeader {
            seq: self.seq,
            width,
            height,
            screen_width: screen_width.unwrap_or(width),
            screen_height: screen_height.unwrap_or(height),
        };
        let mut payload = header.encode().to_vec();
        payload.extend_from_slice(&jpeg);
        self.to_host.send((Channel::Frames, payload)).await.is_ok()
    }

    async fn input(&mut self, event: InputEvent) {
        let button = |button: Button| match button {
            Button::Left => "left",
            Button::Middle => "middle",
            Button::Right => "right",
        };
        match event {
            InputEvent::Move { x, y } => self.call("Input.dispatchMouseEvent", json!({"type": "mouseMoved", "x": x, "y": y})).await,
            InputEvent::Down { button: which, x, y } => {
                self.call(
                    "Input.dispatchMouseEvent",
                    json!({"type": "mousePressed", "x": x, "y": y, "button": button(which), "clickCount": 1}),
                )
                .await;
            }
            InputEvent::Up { button: which, x, y } => {
                self.call(
                    "Input.dispatchMouseEvent",
                    json!({"type": "mouseReleased", "x": x, "y": y, "button": button(which), "clickCount": 1}),
                )
                .await;
            }
            InputEvent::Click { x, y } => {
                for kind in ["mousePressed", "mouseReleased"] {
                    self.call("Input.dispatchMouseEvent", json!({"type": kind, "x": x, "y": y, "button": "left", "clickCount": 1})).await;
                }
            }
            InputEvent::Wheel { x, y, dx, dy } => {
                self.call(
                    "Input.dispatchMouseEvent",
                    json!({"type": "mouseWheel", "x": x, "y": y, "deltaX": dx * 100, "deltaY": dy * 100}),
                )
                .await;
            }
            InputEvent::Key { key, mods } => {
                let modifiers: u32 = mods
                    .iter()
                    .map(|modifier| match modifier {
                        wire::Modifier::Alt => 1,
                        wire::Modifier::Ctrl => 2,
                        wire::Modifier::Meta => 4,
                        wire::Modifier::Shift => 8,
                    })
                    .sum();
                let text = if key.chars().count() == 1 && modifiers & 7 == 0 { key.clone() } else { String::new() };
                let code = if key == "Enter" { "\r".to_owned() } else { text.clone() };
                self.call("Input.dispatchKeyEvent", json!({"type": "keyDown", "key": key, "text": code, "modifiers": modifiers})).await;
                self.call("Input.dispatchKeyEvent", json!({"type": "keyUp", "key": key, "modifiers": modifiers})).await;
            }
            InputEvent::Text { s } => self.call("Input.insertText", json!({"text": s})).await,
        }
    }
}

/// What the runner sets in its profile before each launch, here too: Chrome
/// never signs itself in or syncs (so a site's sign-in, Google's say, stays
/// a site's: Fugo 41228), offers no autofill of cards or addresses (Fugo
/// 41400), and restores no pages when it starts.
fn preferences(profile: &std::path::Path) -> std::io::Result<()> {
    let default = profile.join("Default");
    std::fs::create_dir_all(&default)?;
    let path = default.join("Preferences");
    let mut preferences: Value =
        std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_else(|| json!({}));
    if !preferences.is_object() {
        preferences = json!({});
    }
    for (group, key, value) in [
        ("signin", "allowed", false),
        ("signin", "allowed_on_next_startup", false),
        ("sync", "requested", false),
        ("autofill", "credit_card_enabled", false),
        ("autofill", "profile_enabled", false),
    ] {
        if !preferences[group].is_object() {
            preferences[group] = json!({});
        }
        preferences[group][key] = Value::Bool(value);
    }
    // Driven, Chrome restores no pages (W1's runner does the same): nothing
    // runs before the host has set the browser up (Fugo 42798).
    if !preferences["session"].is_object() {
        preferences["session"] = json!({});
    }
    preferences["session"]["restore_on_startup"] = json!(5);
    std::fs::write(path, preferences.to_string())
}
