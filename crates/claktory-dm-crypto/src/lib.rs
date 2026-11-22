//! End-to-end encryption for Claktory's direct messages (dms-e2ee.md, in the
//! Research folder): MLS through OpenMLS, ciphersuite 0x0001.
//!
//! One crate for every endpoint (the app's Rust, factoryd's Claktory leaf)
//! and for the delivery service (claktory-user-apps' worker), so all of them
//! run one OpenMLS version and the same validation. Co-owned by file:
//! - Mista: `storage`, `provider`, `envelope`, `content`, and the golden
//!   vectors in `tests/`.
//! - Ghiaccio: `identity`, `certificate`, `device_list`, `validation`,
//!   `safety`, `handoff`, `group`.
//!
//! This file changes only by agreement between the two.

pub mod certificate;
pub mod content;
pub mod device_list;
pub mod envelope;
pub mod group;
pub mod handoff;
pub mod identity;
pub mod provider;
pub mod safety;
pub mod storage;
pub mod transfer;
pub mod validation;

/// MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519, MLS's mandatory one.
pub const CIPHERSUITE: openmls::prelude::Ciphersuite = openmls::prelude::Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;

/// What can go wrong, shared by every module.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The store couldn't be read or written.
    #[error("storage: {0}")]
    Storage(String),
    /// A sealed value didn't open: another key, or tampered with.
    #[error("a sealed value didn't open")]
    Sealed,
    /// Over the limit for its kind.
    #[error("{kind} is over {most} bytes")]
    TooBig { kind: &'static str, most: usize },
    /// Not an MLS message of a kind this accepts.
    #[error("not an MLS message: {0}")]
    NotMls(String),
    /// Malformed: an envelope, content or certificate.
    #[error("{0}")]
    Format(String),
    /// Refused by the shared validation (`validation`), and why.
    #[error("refused: {0}")]
    Refused(String),
    /// A leaf certified by a key that isn't the one pinned for its account,
    /// nor one it had before: the account may have made a new one (dms-e2ee
    /// §2.1). A friend's device fetches that account's keys, applies
    /// `Pins::key_changed` (the notice) and processes the same commit again;
    /// for the person's own account a new key comes only from their own
    /// devices (M3), never from what the service says.
    #[error("refused: {account}'s certificate isn't from their security key")]
    UnknownIdentity { account: String },
    /// OpenMLS failed.
    #[error("mls: {0}")]
    Mls(String),
}

pub type Result<T> = std::result::Result<T, Error>;
