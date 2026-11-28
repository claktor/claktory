//! Claktory's Calendar app: everything time-based in one place. It shows
//! what agents have scheduled (monitors' next fires, checklist items'
//! dates), what they're running (background tasks, long runs) and what
//! they've planned (events they place through its tools), drawn by
//! Claktory's Calendar component in each device's own time.
//!
//! Derived entries are read live from the source the person granted at
//! every look ([`layers`]) and never kept; placed events are kept in the
//! app's own store ([`store`]), each change keeping the version before.
//! Agents read and change it through its tools ([`tools`]), each seeing
//! its own project's entries (admins all of them). A view ([`view`]) is
//! read-only: the person opens an entry where it lives, and asks an agent
//! to change it.
//!
//! The logic speaks to the host only through [`Host`], so it's tested
//! natively with a stand-in ([`tests`]); `wasm.rs` is the real host.

pub mod cron;
pub mod layers;
pub mod store;
pub mod time;
pub mod tools;
pub mod view;

#[cfg(test)]
mod tests;
#[cfg(target_arch = "wasm32")]
mod wasm;

use serde_json::Value;

/// The catalog its views are made of.
pub const CATALOG: &str = "https://claktory.com/a2ui/catalog/v1";

/// Your agents' activity: times, states and counts, never text.
pub const ACTIVITY: &str = "activity";
/// Activity, plus monitors' names and schedules, checklist items' text and
/// dates, and agents' names and projects.
pub const SCHEDULE: &str = "schedule";

/// The timer that refreshes open views.
pub const REFRESH: &str = "refresh";
/// How often an open view looks again.
pub const REFRESH_MS: u64 = 60_000;
/// While a view waits for a source, how soon it looks again.
pub const ASK_AGAIN_MS: u64 = 3_000;
/// The timer that drops long-past events.
pub const PRUNE: &str = "prune";
pub const PRUNE_MS: u64 = 24 * 3_600_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataSource {
    pub id: String,
    pub label: String,
    pub granted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataError {
    NotGranted,
    UnknownSource,
    Invalid(String),
    Failed(String),
}

impl std::fmt::Display for DataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DataError::NotGranted => write!(f, "it isn't allowed to read that yet"),
            DataError::UnknownSource => write!(f, "that source isn't there"),
            DataError::Invalid(why) | DataError::Failed(why) => write!(f, "{why}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

/// The agent calling a tool, as the host tells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    pub id: String,
    /// Its project; None for the workspace.
    pub project: Option<String>,
    /// An admin agent sees every project's entries.
    pub admin: bool,
    /// It has read the web or a pipe since its conversation started afresh.
    pub tainted: bool,
}

/// What the app can ask of Claktory's app host.
pub trait Host {
    fn kv_get(&mut self, key: &str) -> Option<Vec<u8>>;
    fn kv_set(&mut self, key: &str, value: &[u8]) -> Result<(), String>;
    fn kv_delete(&mut self, key: &str);
    fn set_timer(&mut self, name: &str, after_ms: u64);
    fn cancel_timer(&mut self, name: &str);
    fn push_view(&mut self, surface: &str, message: &Value) -> Result<(), String>;
    fn data_sources(&mut self) -> Vec<DataSource>;
    fn data_query(&mut self, source: &str, sql: &str, params: &Value) -> Result<Value, DataError>;
    fn now_ms(&mut self) -> i64;
    fn log(&mut self, level: Level, message: &str);
    /// The agent calling the tool being run; None outside a call.
    fn caller_info(&mut self) -> Option<Caller>;
    /// Tells one agent, if it hears `kind`.
    fn emit_to(&mut self, agent: &str, kind: &str, text: &str, scope: &str);
}

/// A tool's answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub text: String,
    pub data: Option<Value>,
    pub is_error: bool,
}

/// The app: the views open on it.
#[derive(Debug, Default)]
pub struct App {
    views: std::collections::BTreeMap<String, view::View>,
}

impl App {
    pub fn start(&mut self, host: &mut impl Host) {
        self.prune(host);
    }

    fn prune(&mut self, host: &mut impl Host) {
        let now = host.now_ms();
        if let Err(why) = store::prune(host, now) {
            host.log(Level::Warn, &format!("Couldn't drop past events: {why}"));
        }
        host.set_timer(PRUNE, PRUNE_MS);
    }

    pub fn stop(&mut self, host: &mut impl Host) {
        host.cancel_timer(REFRESH);
        self.views.clear();
    }

    pub fn on_timer(&mut self, host: &mut impl Host, name: &str) {
        match name {
            REFRESH => self.refresh(host, false),
            PRUNE => self.prune(host),
            _ => {}
        }
    }

    pub fn call(&mut self, host: &mut impl Host, tool: &str, args: &str, caller: &str) -> Output {
        let mut output = tools::call(host, tool, args, caller);
        let changed = output.data.as_mut().and_then(Value::as_object_mut).and_then(|data| data.remove("changed"));
        if changed == Some(Value::Bool(true)) {
            self.touch(host);
        }
        output
    }

    /// The person opened `view` as `surface`.
    pub fn view_open(&mut self, host: &mut impl Host, view: &str, params: &str, surface: &str) -> Result<(), String> {
        if !view.is_empty() && view != "main" {
            return Err(format!("Calendar has no view \"{view}\"."));
        }
        let params: Value = serde_json::from_str(params).unwrap_or(Value::Null);
        let device = params["device"].as_str().unwrap_or_default().to_owned();
        let now = host.now_ms();
        let left = store::device_state(host, &device).unwrap_or_default();
        let chosen = left.view.clone().or_else(|| store::prefs(host).view).unwrap_or_else(|| "week".into());
        let mut open = view::View::new(device, chosen, now);
        open.agent = left.agent;
        if now - left.at <= store::DAY_KEPT_MS {
            open.anchor = left.anchor;
        }
        self.views.insert(surface.to_owned(), open);
        if let Err(why) = self.show(host, surface) {
            self.views.remove(surface);
            return Err(why);
        }
        self.schedule(host);
        Ok(())
    }

    pub fn view_closed(&mut self, host: &mut impl Host, surface: &str) {
        self.views.remove(surface);
        if self.views.is_empty() {
            host.cancel_timer(REFRESH);
        }
    }

    pub fn open_surfaces(&self) -> Vec<String> {
        self.views.keys().cloned().collect()
    }

    pub fn view_action(&mut self, host: &mut impl Host, surface: &str, message: &str) {
        let Ok(message) = serde_json::from_str::<Value>(message) else { return };
        let Some(action) = message.get("action") else {
            host.log(Level::Warn, &format!("{surface}: {message}"));
            return;
        };
        let Some(open) = self.views.get_mut(surface) else { return };
        let now = host.now_ms();
        let name = action["name"].as_str().unwrap_or_default();
        match view::act(host, open, name, &action["context"], now) {
            view::Acted::Nothing => return,
            view::Acted::This => {}
            view::Acted::Everyone => {
                for other in self.views.values_mut() {
                    other.stale = true;
                }
            }
        }
        let surfaces: Vec<String> = self.views.iter().filter(|(id, view)| *id == surface || view.stale).map(|(id, _)| id.clone()).collect();
        for surface in surfaces {
            if let Err(why) = self.show(host, &surface) {
                host.log(Level::Warn, &format!("Couldn't update {surface}: {why}"));
                self.views.remove(&surface);
            }
        }
        self.schedule(host);
    }

    /// Placed events changed: every view looks again, from a timer right
    /// after this call.
    fn touch(&mut self, host: &mut impl Host) {
        if !self.views.is_empty() {
            host.set_timer(REFRESH, 1);
        }
    }

    pub fn refresh(&mut self, host: &mut impl Host, _forced: bool) {
        let surfaces = self.open_surfaces();
        for surface in surfaces {
            if let Err(why) = self.show(host, &surface) {
                host.log(Level::Warn, &format!("Couldn't update {surface}: {why}"));
                self.views.remove(&surface);
            }
        }
        self.schedule(host);
    }

    fn schedule(&mut self, host: &mut impl Host) {
        if self.views.is_empty() {
            host.cancel_timer(REFRESH);
            return;
        }
        let waiting = self.views.values().any(|view| view.waiting);
        host.set_timer(REFRESH, if waiting { ASK_AGAIN_MS } else { REFRESH_MS });
    }

    fn show(&mut self, host: &mut impl Host, surface: &str) -> Result<(), String> {
        let Some(open) = self.views.get_mut(surface) else { return Ok(()) };
        view::show(host, surface, open)
    }
}
