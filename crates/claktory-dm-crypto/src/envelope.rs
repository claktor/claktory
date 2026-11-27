//! Formats: the format-2 envelope the worker
//! stores and relays, what it reads from the MLS framing, and the caps by
//! kind.
//!
//! An envelope is just `{"format": 2, "bytes": "<base64url, no padding>"}`
//! around one MLS message. The worker never trusts a declared group, epoch or
//! kind: [`parse`] reads them from the bytes, and the route checks the group
//! against the one it names.
//! - Application messages are `PrivateMessage`s.
//! - Commits and proposals are `PublicMessage`s, so the worker can check them
//!   with OpenMLS's `PublicGroup`.
//! - Welcomes, GroupInfos and KeyPackages travel as themselves.
//!
//! A size is checked before anything is parsed, then again by kind.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use openmls::prelude::{ContentType, GroupId, MlsMessageBodyIn, MlsMessageIn, ProtocolMessage};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tls_codec::Deserialize as _;

use crate::{Error, Result};

/// The envelope's format: the sender's own MLS message. Format 1 (text the
/// worker sealed) is refused everywhere once this is on.
pub const FORMAT: u8 = 2;

/// Application messages: 16 KiB of text in a padded content
/// ([`crate::content::MOST`]), plus MLS's sender data, AEAD and framing.
pub const APPLICATION_MOST: usize = 20 * 1024;
/// Commits, proposals, Welcomes and GroupInfos, which grow with the tree.
pub const HANDSHAKE_MOST: usize = 128 * 1024;
/// A KeyPackage: about 1 KiB with its leaf certificate; the cap leaves room.
pub const KEY_PACKAGE_MOST: usize = 4 * 1024;
/// A chunk of a history transfer (E8), which isn't an MLS message.
pub const HISTORY_CHUNK_MOST: usize = 256 * 1024;
/// The most any one MLS message may be, checked before parsing.
pub const MOST: usize = HANDSHAKE_MOST;

/// What an MLS message is, read from its framing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A `PrivateMessage` carrying application data.
    Application,
    /// A `PublicMessage` carrying a proposal.
    Proposal,
    /// A `PublicMessage` carrying a commit.
    Commit,
    Welcome,
    GroupInfo,
    KeyPackage,
}

impl Kind {
    /// The largest an envelope of this kind may be, in bytes of MLS.
    pub fn most(self) -> usize {
        match self {
            Kind::Application => APPLICATION_MOST,
            Kind::Proposal | Kind::Commit | Kind::Welcome | Kind::GroupInfo => HANDSHAKE_MOST,
            Kind::KeyPackage => KEY_PACKAGE_MOST,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Kind::Application => "an application message",
            Kind::Proposal => "a proposal",
            Kind::Commit => "a commit",
            Kind::Welcome => "a Welcome",
            Kind::GroupInfo => "a GroupInfo",
            Kind::KeyPackage => "a KeyPackage",
        }
    }
}

/// A format-2 envelope: one MLS message's bytes, as sent.
#[derive(Clone, PartialEq, Eq)]
pub struct Envelope {
    bytes: Vec<u8>,
}

/// On the wire.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    format: u8,
    bytes: String,
}

impl Envelope {
    /// An envelope around `bytes`, which must parse as an MLS message
    /// within its kind's cap.
    pub fn new(bytes: Vec<u8>) -> Result<Envelope> {
        parse(&bytes)?;
        Ok(Envelope { bytes })
    }

    /// From its JSON, `{"format": 2, "bytes": "…"}`; anything else, or bytes
    /// that aren't an MLS message within its cap, is refused.
    pub fn from_json(value: &serde_json::Value) -> Result<Envelope> {
        let wire = Wire::deserialize(value).map_err(|_| Error::Format("an envelope is {\"format\": 2, \"bytes\": \"…\"}".to_owned()))?;
        if wire.format != FORMAT {
            return Err(Error::Format(format!("format {} isn't accepted; only format {FORMAT}", wire.format)));
        }
        // Base64 of more than the largest message is refused unread.
        if wire.bytes.len() > MOST.div_ceil(3) * 4 {
            return Err(Error::TooBig { kind: "an MLS message", most: MOST });
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(&wire.bytes)
            .map_err(|_| Error::Format("an envelope's bytes are base64url without padding".to_owned()))?;
        Envelope::new(bytes)
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({"format": FORMAT, "bytes": URL_SAFE_NO_PAD.encode(&self.bytes)})
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// What it says, read again from the bytes.
    pub fn parsed(&self) -> Parsed {
        parse(&self.bytes).expect("checked when made")
    }

    /// What an idempotent retry is compared by: SHA-256 of the format-2
    /// bytes. A retry resends the exact bytes, since encrypting again moves
    /// the sender's ratchet.
    pub fn digest(&self) -> [u8; 32] {
        digest(&self.bytes)
    }
}

impl std::fmt::Debug for Envelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Envelope({} bytes)", self.bytes.len())
    }
}

/// SHA-256 of an envelope's bytes.
pub fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// An MLS message as its framing reads, for the worker's checks.
#[derive(Debug)]
pub struct Parsed {
    pub kind: Kind,
    /// The group and epoch, for application messages, proposals and commits.
    pub group: Option<(GroupId, u64)>,
    /// The message itself, to hand to OpenMLS.
    pub message: MlsMessageIn,
}

impl Parsed {
    /// The message as OpenMLS processes it, for application messages,
    /// proposals and commits.
    pub fn into_protocol_message(self) -> Result<ProtocolMessage> {
        self.message.try_into_protocol_message().map_err(|error| Error::NotMls(format!("{error:?}")))
    }
}

/// Reads an MLS message's kind, group and epoch from its framing, refusing
/// anything over [`MOST`] unread, anything that isn't exactly one MLS
/// message, the wrong wire format for its content, and anything over its
/// kind's cap.
pub fn parse(bytes: &[u8]) -> Result<Parsed> {
    if bytes.len() > MOST {
        return Err(Error::TooBig { kind: "an MLS message", most: MOST });
    }
    let message = MlsMessageIn::tls_deserialize_exact(bytes).map_err(|error| Error::NotMls(error.to_string()))?;
    let (kind, group) = match message.clone().extract() {
        MlsMessageBodyIn::PrivateMessage(private) => match private.content_type() {
            ContentType::Application => (Kind::Application, Some((private.group_id().clone(), private.epoch().as_u64()))),
            _ => return Err(Error::NotMls("commits and proposals are PublicMessages, so the service can check them".to_owned())),
        },
        MlsMessageBodyIn::PublicMessage(public) => {
            let kind = match public.content_type() {
                ContentType::Commit => Kind::Commit,
                ContentType::Proposal => Kind::Proposal,
                ContentType::Application => return Err(Error::NotMls("application messages are PrivateMessages".to_owned())),
            };
            (kind, Some((public.group_id().clone(), public.epoch().as_u64())))
        }
        MlsMessageBodyIn::Welcome(_) => (Kind::Welcome, None),
        MlsMessageBodyIn::GroupInfo(_) => (Kind::GroupInfo, None),
        MlsMessageBodyIn::KeyPackage(_) => (Kind::KeyPackage, None),
    };
    if bytes.len() > kind.most() {
        return Err(Error::TooBig { kind: kind.name(), most: kind.most() });
    }
    Ok(Parsed { kind, group, message })
}
