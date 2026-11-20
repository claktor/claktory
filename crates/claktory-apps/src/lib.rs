//! claktory-apps: the host that runs Claktory's apps, WebAssembly components
//! of the `claktory:app@1` world (wit/app.wit), each in its own sandbox.
//! factoryd starts it as a child and talks to it over stdin and stdout
//! (proto/factory/apps/v1/host.proto); the host is mechanism (instances,
//! limits, the store, timers, filtered HTTP) and factoryd is policy.

mod app;
pub mod frames;
pub mod host;
mod kv;
mod net;

pub use app::Deadlines;
pub use host::{serve, serve_with};
