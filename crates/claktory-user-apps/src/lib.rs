//! Claktory's user-level apps: what a person keeps at their account rather
//! than in any one Claktory (DMs first), served by a worker process of its
//! own beside the account service (the root). The root authenticates every
//! call and forwards it over [`ipc`]; the worker keeps the apps' data and
//! their grants to agents ([`grants`]) in its own database, and holds none
//! of the root's keys. README.md has the whole contract.

pub mod ipc;

#[cfg(feature = "worker")]
pub mod grants;
#[cfg(feature = "worker")]
pub mod worker;
