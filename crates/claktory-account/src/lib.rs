//! Claktory's account service (`claktory-account`): email sign-in, machine
//! tokens, the vault's service share, check-ins and the relay allowlist.
//! API.md is the contract; `protocol` is what a client needs of it.

pub mod protocol;
#[cfg(feature = "server")]
pub mod server;
