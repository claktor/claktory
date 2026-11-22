//! The app as a component of the `claktory:app@1.1` world `app-with-views`:
//! its exports call [`App`], and [`WasmHost`] is the host's imports.

use std::cell::RefCell;

use serde_json::Value;

use crate::{App, DataError, DataSource, Host, Level};

wit_bindgen::generate!({
    world: "app-with-views",
    // The host's own world, so the app always builds against it.
    path: "../../crates/claktory-apps/wit",
});

use claktory::app::host;
use claktory::app::types;

thread_local! {
    static APP: RefCell<App> = RefCell::new(App::default());
}

fn with<T>(run: impl FnOnce(&mut App, &mut WasmHost) -> T) -> T {
    APP.with(|app| run(&mut app.borrow_mut(), &mut WasmHost))
}

struct Component;

impl Guest for Component {
    fn start() {
        with(|app, host| app.start(host));
    }

    fn stop() {
        with(|app, host| app.stop(host));
    }

    fn upgrade(_from_version: String) {}

    fn call(tool: String, args: String, caller: String) -> types::ToolResult {
        let output = with(|app, host| app.call(host, &tool, &args, &caller));
        types::ToolResult { text: output.text, data: output.data.map(|data| data.to_string()), is_error: output.is_error }
    }

    fn on_timer(name: String) {
        with(|app, host| app.on_timer(host, &name));
    }
}

impl exports::claktory::app::views::Guest for Component {
    fn view_open(view: String, _params: String, surface: String) -> Result<(), String> {
        with(|app, host| app.view_open(host, &view, &surface))
    }

    fn view_action(surface: String, message: String) {
        with(|app, host| app.view_action(host, &surface, &message));
    }

    fn view_closed(surface: String) {
        with(|app, host| app.view_closed(host, &surface));
    }
}

export!(Component);

/// The host's imports.
struct WasmHost;

impl Host for WasmHost {
    fn kv_get(&mut self, key: &str) -> Option<Vec<u8>> {
        host::kv_get(key)
    }

    fn kv_set(&mut self, key: &str, value: &[u8]) -> Result<(), String> {
        host::kv_set(key, value)
    }

    fn kv_delete(&mut self, key: &str) {
        host::kv_delete(key);
    }

    fn set_timer(&mut self, name: &str, after_ms: u64) {
        host::set_timer(name, after_ms);
    }

    fn cancel_timer(&mut self, name: &str) {
        host::cancel_timer(name);
    }

    fn push_view(&mut self, surface: &str, message: &Value) -> Result<(), String> {
        host::push_view(surface, &message.to_string())
    }

    fn data_sources(&mut self) -> Vec<DataSource> {
        host::data_sources().into_iter().map(|source| DataSource { id: source.id, label: source.label, granted: source.granted }).collect()
    }

    fn data_query(&mut self, source: &str, sql: &str, params: &Value) -> Result<Value, DataError> {
        let answer = host::data_query(source, sql, &params.to_string()).map_err(|error| match error {
            types::DataError::NotGranted => DataError::NotGranted,
            types::DataError::UnknownSource => DataError::UnknownSource,
            types::DataError::Invalid(why) => DataError::Invalid(why),
            types::DataError::Failed(why) => DataError::Failed(why),
        })?;
        serde_json::from_str(&answer).map_err(|error| DataError::Failed(format!("its answer can't be read: {error}")))
    }

    fn now_ms(&mut self) -> i64 {
        let now = wasi::clocks::wall_clock::now();
        now.seconds as i64 * 1000 + i64::from(now.nanoseconds / 1_000_000)
    }

    fn log(&mut self, level: Level, message: &str) {
        let level = match level {
            Level::Error => host::LogLevel::Error,
            Level::Warn => host::LogLevel::Warn,
            Level::Info => host::LogLevel::Info,
            Level::Debug => host::LogLevel::Debug,
        };
        host::log(level, message);
    }
}
