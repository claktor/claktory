//! App views as surfaces: one for each view of an app with the same
//! params. The app asks for its device's own with `"device"` in them, which
//! factoryd sets to the device the connection is from (the paired device's
//! id, or "local"), whatever it said, so each device has its own surface of
//! a view, shared by the connections it has showing it; the app itself is given
//! only an opaque stand-in for the device (`opaque_device`), the same for
//! that device every time, and unlike any other app's. factoryd keeps what
//! each shows (`a2ui::Content`), sends it whole to a connection that opens
//! it (AppSurface) and then each change (AppViewUpdate), only to the
//! connections showing it (`Viewer`). Surfaces live in memory: a restart
//! of factoryd makes new ones when devices open their views again.
//!
//! A surface opens when the app runs (OPENING until its createSurface,
//! then OPEN), waits while it doesn't (STOPPED, opened again when it runs
//! again), and ends when the app sends something invalid or can't open it
//! (FAILED), closes it or is uninstalled (CLOSED), or nobody has shown it
//! for 10 s, when the app hears view-closed.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use factory_protocol::apps::{self as wire, frame::Body as HostBody};
use factory_protocol::v1::{self, AppSurfaceState as State, server_message::Body};
use tokio::sync::mpsc;

use super::a2ui::{Applied, CATALOG, Content};
use super::host::Host;
use crate::factory::Factory;

/// How long a surface nobody shows waits before it's closed.
const CLOSE_GRACE: Duration = Duration::from_secs(10);
/// The most surfaces an app has open at once on one device (by its
/// params' "device"), so one device can't crowd out another.
const MAX_SURFACES: usize = 16;
/// The most messages an app sends a surface in a second…
const MAX_RATE: u32 = 50;
/// …and the most bytes it sends all its surfaces in a second, so a view
/// can't drain a phone's data or battery: past it, the surface fails.
const MAX_BYTES_PER_SECOND: usize = 1 << 20;
const MAX_PARAMS: usize = 4096;
/// What waits for a connection before it falls behind (and gets a fresh
/// snapshot instead).
const VIEWER_QUEUE: usize = 64;

pub(crate) struct Surfaces {
    registry: Mutex<Registry>,
    next_viewer: AtomicU64,
    /// CLOSE_GRACE, or shorter in tests.
    grace: Mutex<Duration>,
    /// What devices are hashed under for apps (`device_key`).
    device_key: std::sync::OnceLock<[u8; 32]>,
}

impl Default for Surfaces {
    fn default() -> Surfaces {
        Surfaces {
            registry: Mutex::default(),
            next_viewer: AtomicU64::new(1),
            grace: Mutex::new(CLOSE_GRACE),
            device_key: std::sync::OnceLock::new(),
        }
    }
}

#[derive(Default)]
struct Registry {
    surfaces: BTreeMap<String, Surface>,
    viewers: HashMap<u64, Slot>,
    /// What each app pushed in the current second: when it began, and how
    /// many bytes.
    budgets: HashMap<String, (Instant, usize)>,
}

/// A connection's way in.
struct Slot {
    sender: mpsc::Sender<Body>,
    /// Surfaces whose changes didn't fit its queue: it gets their snapshot.
    stale: BTreeSet<String>,
}

struct Surface {
    id: String,
    app: String,
    view: String,
    /// As the device asked, canonical: what devices find it by.
    params: String,
    /// What the app is given: `params` with its device made opaque.
    app_params: String,
    /// Its params' device ("" for none), which its app's limit counts by.
    device: String,
    title: String,
    state: State,
    error: String,
    content: Content,
    /// The last change applied.
    seq: u64,
    /// The connections showing it.
    viewers: BTreeSet<u64>,
    /// Which opening this is: answers about an earlier one are ignored.
    epoch: u64,
    /// Since when nobody has shown it.
    unseen: Option<Instant>,
    /// The messages in the current second: when it began, and how many.
    window: (Instant, u32),
}

impl Surface {
    fn info(&self) -> v1::AppSurface {
        v1::AppSurface {
            surface_id: self.id.clone(),
            app_id: self.app.clone(),
            view: self.view.clone(),
            params: self.params.clone(),
            title: self.title.clone(),
            state: self.state.into(),
            error: self.error.clone(),
            catalog: CATALOG.to_owned(),
            seq: self.seq,
            snapshot: self.content.snapshot(&self.id),
        }
    }

    fn live(&self) -> bool {
        matches!(self.state, State::Opening | State::Open)
    }
}

impl Registry {
    /// Sends `body` to every connection showing `surface`.
    fn tell(&mut self, surface: &str, body: &Body) {
        let Some(viewers) = self.surfaces.get(surface).map(|surface| surface.viewers.clone()) else { return };
        for viewer in viewers {
            let Some(slot) = self.viewers.get_mut(&viewer) else { continue };
            if let Err(mpsc::error::TrySendError::Full(_)) = slot.sender.try_send(body.clone()) {
                slot.stale.insert(surface.to_owned());
            }
        }
    }

    /// Tells the surface's viewers its state.
    fn tell_info(&mut self, surface: &str) {
        if let Some(info) = self.surfaces.get(surface).map(Surface::info) {
            self.tell(surface, &Body::AppSurface(info));
        }
    }

    /// Ends a surface: its viewers hear `state` and `error`, then it's
    /// forgotten. Returns whether the app has it open (to tell it).
    fn end(&mut self, surface: &str, state: State, error: String) -> bool {
        let Some(found) = self.surfaces.get_mut(surface) else { return false };
        let live = found.live();
        found.state = state;
        found.error = error;
        self.tell_info(surface);
        self.surfaces.remove(surface);
        live
    }
}

impl Surfaces {
    fn lock(&self) -> MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn grace(&self) -> Duration {
        *self.grace.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Tests: a connection's views from `device`, without a connection.
#[doc(hidden)]
pub struct ViewerForTest(Viewer);

impl ViewerForTest {
    pub fn open(&self, factory: &Arc<Factory>, request: &v1::OpenAppView) -> Body {
        factory.open_app_view(&self.0, request)
    }
}

impl Factory {
    /// Tests: views opened as a connection from `device` would.
    #[doc(hidden)]
    pub fn app_viewer_for_test(self: &Arc<Self>, device: &str) -> ViewerForTest {
        ViewerForTest(Viewer::new(self, device))
    }

    /// How long a view nobody shows waits before the app hears it's
    /// closed: 10 s, shorter in tests.
    pub fn set_app_view_grace(&self, grace: Duration) {
        *self.apps.surfaces.grace.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = grace;
    }
}

/// One connection's views: what it shows gets to it through `next`; when
/// it's dropped (the connection ended), it shows nothing any more.
pub(crate) struct Viewer {
    id: u64,
    /// The device the connection is from: a paired device's id, or "local".
    device: String,
    factory: Weak<Factory>,
    receiver: tokio::sync::Mutex<mpsc::Receiver<Body>>,
}

impl Viewer {
    /// For a connection from `device` (empty: this computer's, "local").
    pub(crate) fn new(factory: &Arc<Factory>, device: &str) -> Viewer {
        let surfaces = &factory.apps.surfaces;
        let id = surfaces.next_viewer.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel(VIEWER_QUEUE);
        surfaces.lock().viewers.insert(id, Slot { sender, stale: BTreeSet::new() });
        let device = if device.is_empty() { crate::remote::LOCAL_DEVICE.to_owned() } else { device.to_owned() };
        Viewer { id, device, factory: Arc::downgrade(factory), receiver: tokio::sync::Mutex::new(receiver) }
    }

    /// The next message for this connection: a change, or a fresh snapshot
    /// of a surface whose changes it fell behind on.
    pub(crate) async fn next(&self) -> Body {
        let mut receiver = self.receiver.lock().await;
        match receiver.try_recv() {
            Ok(body) => return body,
            Err(mpsc::error::TryRecvError::Disconnected) => return std::future::pending().await,
            Err(mpsc::error::TryRecvError::Empty) => {}
        }
        if let Some(factory) = self.factory.upgrade() {
            let mut registry = factory.apps.surfaces.lock();
            let stale = registry.viewers.get_mut(&self.id).and_then(|slot| slot.stale.pop_first());
            if let Some(info) = stale.and_then(|surface| registry.surfaces.get(&surface).map(Surface::info)) {
                return Body::AppSurface(info);
            }
        }
        match receiver.recv().await {
            Some(body) => body,
            None => std::future::pending().await,
        }
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        let Some(factory) = self.factory.upgrade() else { return };
        let unseen: Vec<String> = {
            let mut registry = factory.apps.surfaces.lock();
            registry.viewers.remove(&self.id);
            let mut unseen = Vec::new();
            for surface in registry.surfaces.values_mut() {
                if surface.viewers.remove(&self.id) && surface.viewers.is_empty() {
                    surface.unseen = Some(Instant::now());
                    unseen.push(surface.id.clone());
                }
            }
            unseen
        };
        if !unseen.is_empty() && tokio::runtime::Handle::try_current().is_ok() {
            tokio::spawn(close_unseen(Arc::downgrade(&factory), unseen));
        }
    }
}

/// After the grace, closes the surfaces nobody has shown since.
async fn close_unseen(factory: Weak<Factory>, surfaces: Vec<String>) {
    let Some(grace) = factory.upgrade().map(|factory| factory.apps.surfaces.grace()) else { return };
    tokio::time::sleep(grace).await;
    let Some(factory) = factory.upgrade() else { return };
    let closed: Vec<(String, String)> = {
        let mut registry = factory.apps.surfaces.lock();
        let mut closed = Vec::new();
        for id in surfaces {
            let due = registry
                .surfaces
                .get(&id)
                .is_some_and(|surface| surface.viewers.is_empty() && surface.unseen.is_some_and(|since| since.elapsed() >= grace));
            if due
                && let Some(surface) = registry.surfaces.remove(&id)
                && surface.live()
            {
                closed.push((surface.app, surface.id));
            }
        }
        closed
    };
    for (app, surface) in closed {
        tell_closed(&factory, &app, &surface);
    }
}

/// The app's host, when the app runs now.
fn running(factory: &Factory, app: &str) -> Option<Arc<Host>> {
    let state = factory.apps.state();
    let running = state.apps.get(app).is_some_and(|runtime| runtime.running);
    state.host.clone().filter(|_| running)
}

/// The app's name, for the person.
fn name(factory: &Factory, app: &str) -> String {
    factory.apps.manifest(app).map_or_else(|| app.to_owned(), |manifest| manifest.name.clone())
}

fn not_running(name: &str) -> String {
    format!("The {name} app isn't running now. This opens again once it runs.")
}

/// Tells the app it no longer has `surface` open (it stops refreshing it,
/// and can't push to it).
fn tell_closed(factory: &Arc<Factory>, app: &str, surface: &str) {
    let Some(host) = running(factory, app) else { return };
    let closed = wire::ViewClosed { app_id: app.to_owned(), surface: surface.to_owned() };
    tokio::spawn(async move {
        if let Err(problem) = super::done_of(host.ask(HostBody::ViewClosed(closed), super::CALL_DEADLINE).await) {
            tracing::debug!(%problem, "an app didn't take view-closed");
        }
    });
}

/// Asks the app to open the surface, as its `epoch`'th opening.
fn open_in_app(factory: &Arc<Factory>, host: Arc<Host>, surface: &Surface) {
    let ask = wire::ViewOpen {
        app_id: surface.app.clone(),
        view: surface.view.clone(),
        params: surface.app_params.clone(),
        surface: surface.id.clone(),
    };
    let (weak, epoch) = (Arc::downgrade(factory), surface.epoch);
    tokio::spawn(async move {
        let (app, id) = (ask.app_id.clone(), ask.surface.clone());
        let answer = super::done_of(host.ask(HostBody::ViewOpen(ask), super::CALL_DEADLINE).await);
        let Some(factory) = weak.upgrade() else { return };
        let name = name(&factory, &app);
        let tell = {
            let mut registry = factory.apps.surfaces.lock();
            let Some(surface) = registry.surfaces.get(&id).filter(|surface| surface.epoch == epoch && surface.live()) else { return };
            match answer {
                Ok(()) if surface.content.created() => return,
                Ok(()) => registry.end(&id, State::Failed, format!("The {name} app opened this view without showing anything.")),
                Err(problem) => registry.end(&id, State::Failed, format!("The {name} app couldn't show this view: {problem}")),
            }
        };
        if tell {
            tell_closed(&factory, &app, &id);
        }
    });
}

/// Params as one canonical JSON object.
fn canonical(params: &str) -> Result<String, String> {
    if params.len() > MAX_PARAMS {
        return Err(format!("A view's params are at most {MAX_PARAMS} bytes."));
    }
    if params.trim().is_empty() {
        return Ok("{}".to_owned());
    }
    match serde_json::from_str::<serde_json::Value>(params) {
        Ok(value @ serde_json::Value::Object(_)) => Ok(sorted(value).to_string()),
        _ => Err("A view's params are a JSON object.".to_owned()),
    }
}

/// `value` with every object's keys in order (serde_json keeps them as
/// they came here): one text for one object.
fn sorted(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(fields) => {
            let mut fields: Vec<(String, serde_json::Value)> = fields.into_iter().collect();
            fields.sort_by(|a, b| a.0.cmp(&b.0));
            serde_json::Value::Object(fields.into_iter().map(|(key, value)| (key, sorted(value))).collect())
        }
        serde_json::Value::Array(items) => serde_json::Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

/// Canonical `params` whose `"device"` (when they have one) is `device`,
/// the connection's: what a device says it is never counts, so it can't
/// take another's surface, or get past its own limit with a new name each
/// time (Fugo 28393).
fn as_device(params: &str, device: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(params) {
        Ok(serde_json::Value::Object(mut fields)) if fields.contains_key("device") => {
            fields.insert("device".to_owned(), serde_json::Value::String(device.to_owned()));
            sorted(serde_json::Value::Object(fields)).to_string()
        }
        _ => params.to_owned(),
    }
}

/// The device canonical `params` name (`"device"`: a paired device's id,
/// "local", or any other value as its JSON text), and the params as the
/// app is given them: that device replaced by `opaque_device`'s stand-in.
/// None named: "", and the params as they are.
fn device_of(params: &str, key: &[u8; 32], app: &str) -> (String, String) {
    let Ok(serde_json::Value::Object(mut fields)) = serde_json::from_str::<serde_json::Value>(params) else {
        return (String::new(), params.to_owned());
    };
    let Some(device) = fields.get("device") else { return (String::new(), params.to_owned()) };
    let device = match device {
        serde_json::Value::String(device) => device.clone(),
        other => other.to_string(),
    };
    fields.insert("device".to_owned(), serde_json::Value::String(opaque_device(key, app, &device)));
    (device, serde_json::Value::Object(fields).to_string())
}

/// How `app` sees `device`: an HMAC of both under this Claktory's key, so
/// one device is the same to one app every time, while two apps can't
/// match theirs, and none learns the device's id.
fn opaque_device(key: &[u8; 32], app: &str, device: &str) -> String {
    use base64::Engine as _;
    let mut mac = <hmac::Hmac<sha2::Sha256> as hmac::KeyInit>::new_from_slice(key).expect("any key length");
    hmac::Mac::update(&mut mac, format!("claktory app device\0{app}\0{device}").as_bytes());
    let tag = hmac::Mac::finalize(mac).into_bytes();
    format!("d-{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&tag[..16]))
}

/// The key devices are hashed under for apps: 32 random bytes kept in
/// `apps/device-key` (0600), made the first time. If it can't be kept,
/// one for this run, so devices still look new to apps after a restart
/// rather than showing through.
fn device_key(factory: &Factory) -> [u8; 32] {
    *factory.apps.surfaces.device_key.get_or_init(|| {
        let path = factory.home.root().join("apps").join("device-key");
        if let Ok(kept) = std::fs::read(&path)
            && let Ok(key) = <[u8; 32]>::try_from(kept.as_slice())
        {
            return key;
        }
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).expect("the system's random source");
        let kept = (|| -> std::io::Result<()> {
            use std::io::Write as _;
            use std::os::unix::fs::OpenOptionsExt as _;
            std::fs::create_dir_all(path.parent().expect("in apps"))?;
            let temporary = path.with_extension("new");
            let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&temporary)?;
            file.write_all(&key)?;
            file.sync_all()?;
            std::fs::rename(&temporary, &path)
        })();
        if let Err(error) = kept {
            tracing::warn!(%error, "couldn't keep the key apps' device ids are made with; one for this run");
        }
        key
    })
}

fn error(code: v1::ErrorCode, message: impl Into<String>) -> Body {
    Body::Error(v1::Error { code: code.into(), message: message.into(), ..Default::default() })
}

fn new_id(app: &str) -> String {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).expect("the system's random source");
    format!("{app}-{}", crate::encoding::hex(&bytes))
}

// ---- The person's requests ----

impl Factory {
    /// OpenAppView: the surface for that view (made if there's none), shown
    /// by this connection from now on.
    pub(crate) fn open_app_view(self: &Arc<Self>, viewer: &Viewer, request: &v1::OpenAppView) -> Body {
        if !self.apps.available() {
            return error(v1::ErrorCode::NotFound, "Apps aren't available in this Claktory.");
        }
        let installed = self.lock().config.apps.iter().any(|app| app.id == request.app_id);
        let Some(manifest) = self.apps.manifest(&request.app_id).filter(|_| installed) else {
            return error(v1::ErrorCode::NotFound, format!("There's no app \"{}\" installed.", request.app_id));
        };
        let Some(spec) = manifest.view(&request.view) else {
            let why = if manifest.views.is_empty() {
                format!("The {} app has no views.", manifest.name)
            } else {
                format!("The {} app has no view \"{}\".", manifest.name, request.view)
            };
            return error(v1::ErrorCode::NotFound, why);
        };
        let params = match canonical(&request.params) {
            Ok(params) => as_device(&params, &viewer.device),
            Err(why) => return error(v1::ErrorCode::InvalidRequest, why),
        };
        if !request.catalogs.iter().any(|catalog| catalog == CATALOG) {
            return Body::AppSurface(v1::AppSurface {
                app_id: manifest.id.clone(),
                view: spec.id.clone(),
                params,
                title: spec.title.clone(),
                state: State::Unsupported.into(),
                error: "This version of the Claktory app can't show this view. Update it to see it.".to_owned(),
                catalog: CATALOG.to_owned(),
                ..Default::default()
            });
        }
        let (device, app_params) = device_of(&params, &device_key(self), &manifest.id);
        let host = running(self, &manifest.id);
        let mut registry = self.apps.surfaces.lock();
        let found =
            registry.surfaces.values().find(|surface| surface.app == manifest.id && surface.view == spec.id && surface.params == params);
        let id = match found {
            Some(surface) => surface.id.clone(),
            None => {
                if registry.surfaces.values().filter(|surface| surface.app == manifest.id && surface.device == device).count()
                    >= MAX_SURFACES
                {
                    let on = if device.is_empty() { "" } else { " on this device" };
                    let why = format!("The {} app has {MAX_SURFACES} views open{on} already. Close one first.", manifest.name);
                    return error(v1::ErrorCode::InvalidRequest, why);
                }
                let surface = Surface {
                    id: new_id(&manifest.id),
                    app: manifest.id.clone(),
                    view: spec.id.clone(),
                    params,
                    app_params,
                    device,
                    title: spec.title.clone(),
                    state: if host.is_some() { State::Opening } else { State::Stopped },
                    error: if host.is_some() { String::new() } else { not_running(&manifest.name) },
                    content: Content::default(),
                    seq: 0,
                    viewers: BTreeSet::new(),
                    epoch: 1,
                    unseen: None,
                    window: (Instant::now(), 0),
                };
                if let Some(host) = host {
                    open_in_app(self, host, &surface);
                }
                let id = surface.id.clone();
                registry.surfaces.insert(id.clone(), surface);
                id
            }
        };
        if let Some(slot) = registry.viewers.get_mut(&viewer.id) {
            slot.stale.remove(&id);
        }
        let surface = registry.surfaces.get_mut(&id).expect("just found or made");
        surface.viewers.insert(viewer.id);
        surface.unseen = None;
        Body::AppSurface(surface.info())
    }

    /// CloseAppView: this connection no longer shows the surface.
    pub(crate) fn close_app_view(self: &Arc<Self>, viewer: &Viewer, surface: &str) -> Body {
        let unseen = {
            let mut registry = self.apps.surfaces.lock();
            if let Some(slot) = registry.viewers.get_mut(&viewer.id) {
                slot.stale.remove(surface);
            }
            match registry.surfaces.get_mut(surface) {
                Some(found) => {
                    let unseen = found.viewers.remove(&viewer.id) && found.viewers.is_empty();
                    if unseen {
                        found.unseen = Some(Instant::now());
                    }
                    unseen
                }
                None => false,
            }
        };
        if unseen {
            tokio::spawn(close_unseen(Arc::downgrade(self), vec![surface.to_owned()]));
        }
        Body::Ack(v1::Ack {})
    }

    /// AppViewAction: checked against the surface, then handed to the app.
    pub(crate) fn app_view_action(self: &Arc<Self>, request: &v1::AppViewAction) -> Body {
        let app = {
            let registry = self.apps.surfaces.lock();
            let Some(surface) = registry.surfaces.get(&request.surface_id).filter(|surface| surface.state == State::Open) else {
                return error(v1::ErrorCode::NotFound, "That view isn't open now. Open it again.");
            };
            if let Err(invalid) = surface.content.check_renderer(&surface.id, &request.message) {
                return error(v1::ErrorCode::InvalidRequest, invalid.to_string());
            }
            surface.app.clone()
        };
        let Some(host) = running(self, &app) else {
            return error(v1::ErrorCode::NotFound, not_running(&name(self, &app)));
        };
        let action = wire::ViewAction { app_id: app, surface: request.surface_id.clone(), message: request.message.clone() };
        tokio::spawn(async move {
            if let Err(problem) = super::done_of(host.ask(HostBody::ViewAction(action), super::CALL_DEADLINE).await) {
                tracing::info!(%problem, "an app didn't take a view's action");
            }
        });
        Body::Ack(v1::Ack {})
    }
}

// ---- From the app and the apps' lifecycle ----

/// A message the app pushed: checked, applied and sent on; or why not.
/// An invalid one fails the surface.
pub(crate) fn push(factory: &Arc<Factory>, pushed: &wire::PushView) -> Result<(), String> {
    let name = name(factory, &pushed.app_id);
    let mut registry = factory.apps.surfaces.lock();
    let Some(surface) = registry.surfaces.get_mut(&pushed.surface).filter(|surface| surface.app == pushed.app_id && surface.live()) else {
        return Err(format!("The surface \"{}\" isn't open.", pushed.surface));
    };
    let now = Instant::now();
    if now.duration_since(surface.window.0) >= Duration::from_secs(1) {
        surface.window = (now, 0);
    }
    if surface.window.1 >= MAX_RATE {
        return Err(format!("Too many messages: at most {MAX_RATE} a second go to a view."));
    }
    surface.window.1 += 1;
    let budget = registry.budgets.entry(pushed.app_id.clone()).or_insert((now, 0));
    if now.duration_since(budget.0) >= Duration::from_secs(1) {
        *budget = (now, 0);
    }
    if budget.1 + pushed.message.len() > MAX_BYTES_PER_SECOND {
        let why = format!("an app's views take at most {MAX_BYTES_PER_SECOND} bytes a second");
        registry.end(&pushed.surface, State::Failed, format!("The {name} app sent too much at once: {why}."));
        drop(registry);
        tell_closed(factory, &pushed.app_id, &pushed.surface);
        return Err(format!("Too much at once: {why}."));
    }
    budget.1 += pushed.message.len();
    let surface = registry.surfaces.get_mut(&pushed.surface).expect("found above");
    let applied = surface.content.apply(&surface.id, &pushed.message);
    let id = surface.id.clone();
    match applied {
        Ok(Applied::Created) => {
            surface.seq += 1;
            surface.state = State::Open;
            registry.tell_info(&id);
            Ok(())
        }
        Ok(Applied::Changed(message)) => {
            surface.seq += 1;
            let update = v1::AppViewUpdate { surface_id: id.clone(), seq: surface.seq, message };
            registry.tell(&id, &Body::AppViewUpdate(update));
            Ok(())
        }
        Ok(Applied::Deleted) => {
            registry.end(&id, State::Closed, format!("The {name} app closed this view."));
            drop(registry);
            tell_closed(factory, &pushed.app_id, &id);
            Ok(())
        }
        Err(invalid) => {
            tracing::info!(app = %pushed.app_id, %invalid, "an app sent a view something invalid");
            registry.end(&id, State::Failed, format!("The {name} app sent something Claktory can't show: {invalid}"));
            drop(registry);
            tell_closed(factory, &pushed.app_id, &id);
            Err(invalid.to_string())
        }
    }
}

/// The app runs now: its surfaces waiting for it open again.
pub(crate) fn app_running(factory: &Arc<Factory>, app: &str) {
    let Some(host) = running(factory, app) else { return };
    let mut registry = factory.apps.surfaces.lock();
    let waiting: Vec<String> =
        registry.surfaces.values().filter(|surface| surface.app == app && surface.state == State::Stopped).map(|s| s.id.clone()).collect();
    for id in waiting {
        let surface = registry.surfaces.get_mut(&id).expect("listed");
        if surface.viewers.is_empty() {
            registry.surfaces.remove(&id);
            continue;
        }
        surface.state = State::Opening;
        surface.error.clear();
        surface.epoch += 1;
        open_in_app(factory, host.clone(), surface);
        registry.tell_info(&id);
    }
}

/// The app stopped (`app`, or every app for None): its surfaces wait
/// for it, empty, saying `why`.
pub(crate) fn app_stopped(factory: &Factory, app: Option<&str>, why: &dyn Fn(&str) -> String) {
    let mut registry = factory.apps.surfaces.lock();
    let affected: Vec<(String, String)> = registry
        .surfaces
        .values()
        .filter(|surface| app.is_none_or(|app| surface.app == app) && surface.live())
        .map(|surface| (surface.id.clone(), surface.app.clone()))
        .collect();
    for (id, app) in affected {
        let surface = registry.surfaces.get_mut(&id).expect("listed");
        if surface.viewers.is_empty() {
            registry.surfaces.remove(&id);
            continue;
        }
        surface.state = State::Stopped;
        surface.error = why(&name(factory, &app));
        surface.content = Content::default();
        surface.epoch += 1;
        registry.tell_info(&id);
    }
}

/// The app stopped by itself or was turned off: the usual sentence.
pub(crate) fn app_not_running(factory: &Factory, app: &str) {
    app_stopped(factory, Some(app), &|name| not_running(name));
}

/// The app was uninstalled: its surfaces end.
pub(crate) fn app_uninstalled(factory: &Factory, app: &str) {
    let mut registry = factory.apps.surfaces.lock();
    registry.budgets.remove(app);
    let ids: Vec<String> = registry.surfaces.values().filter(|surface| surface.app == app).map(|surface| surface.id.clone()).collect();
    let name = name(factory, app);
    for id in ids {
        registry.end(&id, State::Closed, format!("The {name} app was uninstalled."));
    }
}

#[cfg(test)]
mod tests {
    use super::{as_device, canonical, device_of};

    #[test]
    fn the_device_in_params_is_always_the_connections() {
        assert_eq!(as_device(r#"{"device":"someone-else","range":"7d"}"#, "phone"), r#"{"device":"phone","range":"7d"}"#);
        assert_eq!(as_device(r#"{"device":12}"#, "local"), r#"{"device":"local"}"#);
        assert_eq!(as_device(r#"{"range":"7d"}"#, "phone"), r#"{"range":"7d"}"#, "only when it asks for its own");
    }

    /// Bruno 28347: an app never gets a device's id, only a stand-in, the
    /// same for one device and app every time, different across apps.
    #[test]
    fn an_app_sees_a_device_only_as_its_own_opaque_stand_in() {
        let key = [7u8; 32];
        let params = canonical(r#"{"range":"7d","device":"dev-0123456789"}"#).unwrap();
        let (device, given) = device_of(&params, &key, "charts");
        assert_eq!(device, "dev-0123456789");
        assert!(!given.contains("dev-0123456789"), "{given}");
        let value: serde_json::Value = serde_json::from_str(&given).unwrap();
        let stand_in = value["device"].as_str().expect("a stand-in").to_owned();
        assert!(stand_in.starts_with("d-") && stand_in.len() == 24, "{stand_in}");
        assert_eq!(value["range"], "7d");
        assert_eq!(device_of(&params, &key, "charts").1, given, "the same every time");
        let (_, other_app) = device_of(&params, &key, "notes");
        assert_ne!(serde_json::from_str::<serde_json::Value>(&other_app).unwrap()["device"], stand_in.as_str(), "another app's differs");
        let (_, other_key) = device_of(&params, &[8u8; 32], "charts");
        assert_ne!(other_key, given, "another Claktory's differs");
        // "local" too, and a value that isn't text.
        let stand_in_of = |given: &str| serde_json::from_str::<serde_json::Value>(given).unwrap()["device"].clone();
        let (local, given) = device_of(r#"{"device":"local"}"#, &key, "charts");
        assert_eq!(local, "local");
        assert!(stand_in_of(&given).as_str().is_some_and(|stand_in| stand_in.starts_with("d-")), "{given}");
        let (number, given) = device_of(r#"{"device":12}"#, &key, "charts");
        assert_eq!(number, "12");
        assert!(stand_in_of(&given).as_str().is_some_and(|stand_in| stand_in.starts_with("d-")), "{given}");
        // None: as they were.
        assert_eq!(device_of(r#"{"range":"7d"}"#, &key, "charts"), (String::new(), r#"{"range":"7d"}"#.to_owned()));
    }

    #[test]
    fn params_are_one_canonical_object() {
        assert_eq!(canonical("").unwrap(), "{}");
        assert_eq!(canonical(r#"{"b": 1, "a": [{"d": 2, "c": 3}]}"#).unwrap(), r#"{"a":[{"c":3,"d":2}],"b":1}"#);
        assert_eq!(canonical(r#"{"a":[2],"b":1}"#).unwrap(), canonical(r#"{ "b":1,"a":[2] }"#).unwrap());
        assert!(canonical("[1]").is_err());
        assert!(canonical(&format!(r#"{{"a":"{}"}}"#, "x".repeat(5000))).is_err());
    }
}
