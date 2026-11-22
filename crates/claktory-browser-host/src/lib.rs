//! Claktory's browser helper (browser-control.md §6, B4a): the native
//! messaging host Chrome starts when Claktory's extension connects, which
//! relays one session between the extension and the Claktory app on this
//! Mac over a Unix socket in a private folder, never TCP, never a debug port
//! and never factoryd directly.
//!
//! - Chrome launches it over stdio, naming the extension; it refuses any
//!   other caller, and won't run by hand from a terminal.
//! - It checks the app's code signature from the socket's audit token (the
//!   same Developer ID team, the app's identifier), and the app checks it
//!   the same way. A Dev build signed ad hoc pins the app it sits in by
//!   cdhash instead; a real build never accepts ad hoc code.
//! - Every frame is capped before anything is allocated, and checked
//!   against the schema (browser-extension/PROTOCOL.md): one tab and origin
//!   pinned by the extension's hello, one command in flight, read, scroll,
//!   and (B3b) click, fill and navigation, each of which the person reviews
//!   on the Mac: the extension's review must carry the hash that binds what
//!   it shows to the exact command relayed (`canonical`), and the app's
//!   decision must name that review, once.
//! - It grants nothing: the app binds a helper to a session only on the
//!   person's choice there, so one launched by hand sits unbound until the
//!   app closes it. It never starts the app or takes focus.

pub mod canonical;
pub mod flavor;
pub mod frame;
pub mod peer;
pub mod register;
pub mod relay;
pub mod wire;
