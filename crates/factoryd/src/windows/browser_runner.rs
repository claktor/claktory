//! `browser_runner` on Windows (lib.rs): Claktory's browser runner runs on
//! Linux (inside a VM on a Mac), never on Windows, where the browser isn't
//! available yet (`browser_host::setup` says so). What the browser's host
//! and errands read of it, its wire and the input lease, are the same
//! files as elsewhere.

// Its `Lease` is the runner's own: only the judging is used here.
#[path = "../browser_runner/lease.rs"]
#[allow(dead_code, reason = "the runner's half of the lease doesn't run on Windows")]
pub(crate) mod lease;
#[path = "../browser_runner/wire.rs"]
pub mod wire;
// What factoryd signs for the gate; the gate itself runs on Linux only.
#[path = "../browser_runner/gate.rs"]
#[allow(dead_code, reason = "the gate doesn't run on Windows")]
pub mod gate;

use std::process::ExitCode;

/// `factoryd browser-runner …`: not on Windows.
pub fn main(_args: &[String]) -> ExitCode {
    eprintln!("Claktory's browser runner runs on Linux only.");
    ExitCode::from(2)
}
