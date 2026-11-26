//! The signed device list (dms-e2ee.md §2): a person's current leaves, their
//! devices and Claktories, with a version that only goes up, signed by
//! their identity key. Members accept a leaf only if its certificate chains
//! to the pinned identity and the latest list they've seen names it, so the
//! service can't slip in a leaf, and a removed one never comes back.
//!
//! The version is scoped to the identity key that signs it (Fugo L1): it
//! starts again at 1 when the key rotates, and versions are only ever
//! compared under one key (`validation::Pins`).
//!
//! A list can also name the person's home Claktory (#451): the iroh endpoint
//! id of the Claktory that keeps their direct messages, signed with the
//! rest, so friends' homes deliver only where the person's own devices said.
//! A list with a home is format 2; one without is format 1, byte for byte
//! as before, so lists made before read as they did.

use tls_codec::{Deserialize as _, Serialize as _, TlsDeserialize, TlsSerialize, TlsSize, VLBytes};

use crate::Error;
use crate::certificate::{LeafCertificate, LeafKind, MAX_ACCOUNT_ID, MAX_LEAF_ID};
use crate::identity::{IdentityKey, IdentityPublic, Purpose};

const VERSION: u8 = 1;
/// Format 2: format 1 with the home's endpoint id after the leaves.
const WITH_HOME: u8 = 2;
/// The most leaves one person has: devices and Claktories together.
pub const MAX_LEAVES: usize = 64;

/// One leaf on the list: enough to match its certificate.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Listed {
    pub kind: LeafKind,
    pub leaf_id: String,
    pub signature_key: [u8; 32],
}

/// A person's leaves at one version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceList {
    pub account_id: String,
    pub version: u64,
    pub created_ms: u64,
    pub leaves: Vec<Listed>,
}

#[derive(Debug, Clone, PartialEq, TlsSerialize, TlsDeserialize, TlsSize)]
struct WireLeaf {
    kind: u8,
    leaf_id: VLBytes,
    signature_key: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, TlsSerialize, TlsDeserialize, TlsSize)]
struct Body {
    format: u8,
    account_id: VLBytes,
    identity: [u8; 32],
    version: u64,
    created_ms: u64,
    leaves: Vec<WireLeaf>,
}

#[derive(Debug, Clone, PartialEq, TlsSerialize, TlsDeserialize, TlsSize)]
struct Wire {
    body: Body,
    signature: [u8; 64],
}

#[derive(Debug, Clone, PartialEq, TlsSerialize, TlsDeserialize, TlsSize)]
struct HomeBody {
    body: Body,
    home: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, TlsSerialize, TlsDeserialize, TlsSize)]
struct HomeWire {
    body: HomeBody,
    signature: [u8; 64],
}

/// A device list as its identity signed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedDeviceList {
    pub list: DeviceList,
    pub identity: IdentityPublic,
    /// The person's home Claktory: its iroh endpoint id, if the list names
    /// one.
    pub home: Option<[u8; 32]>,
    signature: [u8; 64],
}

fn kind_code(kind: LeafKind) -> u8 {
    match kind {
        LeafKind::Device => 1,
        LeafKind::Claktory => 2,
    }
}

impl DeviceList {
    fn check(&self) -> Result<(), Error> {
        let id_ok = |id: &str, most: usize| !id.is_empty() && id.len() <= most && id.bytes().all(|b| b.is_ascii_graphic());
        if !id_ok(&self.account_id, MAX_ACCOUNT_ID) {
            return Err(Error::Format("a device list's account id isn't one".to_owned()));
        }
        if self.version == 0 {
            return Err(Error::Format("a device list's version starts at 1".to_owned()));
        }
        if self.leaves.len() > MAX_LEAVES {
            return Err(Error::TooBig { kind: "device list", most: MAX_LEAVES });
        }
        for (n, leaf) in self.leaves.iter().enumerate() {
            if !id_ok(&leaf.leaf_id, MAX_LEAF_ID) {
                return Err(Error::Format("a listed leaf id isn't one".to_owned()));
            }
            if self.leaves[..n].iter().any(|other| other.leaf_id == leaf.leaf_id || other.signature_key == leaf.signature_key) {
                return Err(Error::Format("a leaf is listed twice".to_owned()));
            }
        }
        Ok(())
    }

    fn body(&self, identity: IdentityPublic, format: u8) -> Body {
        Body {
            format,
            account_id: self.account_id.as_bytes().to_vec().into(),
            identity: identity.0,
            version: self.version,
            created_ms: self.created_ms,
            leaves: self
                .leaves
                .iter()
                .map(|leaf| WireLeaf {
                    kind: kind_code(leaf.kind),
                    leaf_id: leaf.leaf_id.as_bytes().to_vec().into(),
                    signature_key: leaf.signature_key,
                })
                .collect(),
        }
    }

    /// The person's identity signs this list.
    pub fn sign(self, identity: &IdentityKey) -> Result<SignedDeviceList, Error> {
        self.sign_with(identity, None)
    }

    /// The person's identity signs this list, naming `home` (the home
    /// Claktory's iroh endpoint id) when given.
    pub fn sign_with(self, identity: &IdentityKey, home: Option<[u8; 32]>) -> Result<SignedDeviceList, Error> {
        self.check()?;
        let public = identity.public();
        let signed = signed_bytes(&self, public, home)?;
        let signature = identity.sign(Purpose::DeviceList, &signed);
        Ok(SignedDeviceList { list: self, identity: public, home, signature })
    }
}

/// What the identity signs: the body, and the home in format 2.
fn signed_bytes(list: &DeviceList, identity: IdentityPublic, home: Option<[u8; 32]>) -> Result<Vec<u8>, Error> {
    let written = match home {
        None => list.body(identity, VERSION).tls_serialize_detached(),
        Some(home) => HomeBody { body: list.body(identity, WITH_HOME), home }.tls_serialize_detached(),
    };
    written.map_err(|error| Error::Format(error.to_string()))
}

fn text(bytes: &VLBytes) -> Result<String, Error> {
    String::from_utf8(bytes.as_slice().to_vec()).map_err(|_| Error::Format("not UTF-8".to_owned()))
}

impl SignedDeviceList {
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let written = match self.home {
            None => Wire { body: self.list.body(self.identity, VERSION), signature: self.signature }.tls_serialize_detached(),
            Some(home) => HomeWire { body: HomeBody { body: self.list.body(self.identity, WITH_HOME), home }, signature: self.signature }
                .tls_serialize_detached(),
        };
        written.map_err(|error| Error::Format(error.to_string()))
    }

    /// Reads a list, all of `bytes`, checking its form and that the
    /// identity inside signed it. Whether that identity is the account's
    /// pinned one is `validation`'s.
    pub fn from_bytes(bytes: &[u8]) -> Result<SignedDeviceList, Error> {
        let not = || Error::Format("not a device list".to_owned());
        let (body, home, signature) = match bytes.first() {
            Some(&WITH_HOME) => {
                let wire = HomeWire::tls_deserialize_exact(bytes).map_err(|_| not())?;
                (wire.body.body, Some(wire.body.home), wire.signature)
            }
            _ => {
                let wire = Wire::tls_deserialize_exact(bytes).map_err(|_| not())?;
                (wire.body, None, wire.signature)
            }
        };
        let body = &body;
        let expected = if home.is_some() { WITH_HOME } else { VERSION };
        if body.format != expected {
            return Err(Error::Format(format!("device list format {}", body.format)));
        }
        let mut leaves = Vec::with_capacity(body.leaves.len());
        for leaf in &body.leaves {
            let kind = match leaf.kind {
                1 => LeafKind::Device,
                2 => LeafKind::Claktory,
                other => return Err(Error::Format(format!("unknown leaf kind {other}"))),
            };
            leaves.push(Listed { kind, leaf_id: text(&leaf.leaf_id)?, signature_key: leaf.signature_key });
        }
        let list = DeviceList { account_id: text(&body.account_id)?, version: body.version, created_ms: body.created_ms, leaves };
        list.check()?;
        let identity = IdentityPublic::from_slice(&body.identity)?;
        let signed = signed_bytes(&list, identity, home)?;
        identity.verify(Purpose::DeviceList, &signed, &signature)?;
        Ok(SignedDeviceList { list, identity, home, signature })
    }

    /// Whether the list names the certificate's leaf, signed by the same
    /// identity for the same account.
    pub fn names(&self, certificate: &LeafCertificate) -> bool {
        let leaf = &certificate.leaf;
        certificate.identity == self.identity
            && leaf.account_id == self.list.account_id
            && self
                .list
                .leaves
                .iter()
                .any(|listed| listed.kind == leaf.kind && listed.leaf_id == leaf.leaf_id && listed.signature_key == leaf.signature_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::certificate::tests::leaf;

    #[test]
    fn a_list_names_only_its_leaves_and_any_change_breaks_it() {
        let identity = IdentityKey::generate().unwrap();
        let phone = LeafCertificate::issue(&identity, leaf(LeafKind::Device, "acct-1", "phone", [1; 32])).unwrap();
        let mac = LeafCertificate::issue(&identity, leaf(LeafKind::Device, "acct-1", "mac", [2; 32])).unwrap();
        let list = DeviceList {
            account_id: "acct-1".to_owned(),
            version: 3,
            created_ms: 1,
            leaves: vec![Listed { kind: LeafKind::Device, leaf_id: "phone".to_owned(), signature_key: [1; 32] }],
        }
        .sign(&identity)
        .unwrap();
        let read = SignedDeviceList::from_bytes(&list.to_bytes().unwrap()).unwrap();
        assert_eq!(read, list);
        assert!(read.names(&phone) && !read.names(&mac));

        // The same leaf id with another key isn't named.
        let swapped = LeafCertificate::issue(&identity, leaf(LeafKind::Device, "acct-1", "phone", [9; 32])).unwrap();
        assert!(!read.names(&swapped));
        // Nor a certificate from another identity.
        let other = IdentityKey::generate().unwrap();
        let forged = LeafCertificate::issue(&other, leaf(LeafKind::Device, "acct-1", "phone", [1; 32])).unwrap();
        assert!(!read.names(&forged));

        let bytes = list.to_bytes().unwrap();
        for at in [2, bytes.len() / 2, bytes.len() - 1] {
            let mut changed = bytes.clone();
            changed[at] ^= 1;
            assert!(SignedDeviceList::from_bytes(&changed).is_err(), "byte {at}");
        }
        let twice = DeviceList {
            account_id: "acct-1".to_owned(),
            version: 1,
            created_ms: 1,
            leaves: vec![
                Listed { kind: LeafKind::Device, leaf_id: "a".to_owned(), signature_key: [1; 32] },
                Listed { kind: LeafKind::Device, leaf_id: "b".to_owned(), signature_key: [1; 32] },
            ],
        };
        assert!(twice.sign(&identity).is_err());
    }

    #[test]
    fn a_list_names_its_home_under_the_same_signature_and_one_without_reads_as_before() {
        let identity = IdentityKey::generate().unwrap();
        let list = || DeviceList {
            account_id: "acct-1".to_owned(),
            version: 2,
            created_ms: 5,
            leaves: vec![Listed { kind: LeafKind::Claktory, leaf_id: "mac".to_owned(), signature_key: [3; 32] }],
        };
        // Without a home: format 1, as every list before it.
        let plain = list().sign(&identity).unwrap();
        let bytes = plain.to_bytes().unwrap();
        assert_eq!((bytes[0], plain.home), (1, None));
        assert_eq!(SignedDeviceList::from_bytes(&bytes).unwrap(), plain);

        // With one: format 2, read back with it.
        let homed = list().sign_with(&identity, Some([7; 32])).unwrap();
        let bytes = homed.to_bytes().unwrap();
        assert_eq!(bytes[0], 2);
        let read = SignedDeviceList::from_bytes(&bytes).unwrap();
        assert_eq!((read.home, &read), (Some([7; 32]), &homed));

        // The home is signed: another one, or none, doesn't verify.
        let at = bytes.len() - 64 - 1;
        let mut moved = bytes.clone();
        moved[at] ^= 1;
        assert!(SignedDeviceList::from_bytes(&moved).is_err(), "another home");
        let mut dropped = bytes[..at - 31].to_vec();
        dropped[0] = 1;
        dropped.extend_from_slice(&bytes[at + 1..]);
        assert!(SignedDeviceList::from_bytes(&dropped).is_err(), "the home cut out");
        let mut relabelled = plain.to_bytes().unwrap();
        relabelled[0] = 2;
        assert!(SignedDeviceList::from_bytes(&relabelled).is_err(), "format 1 read as 2");
    }
}
