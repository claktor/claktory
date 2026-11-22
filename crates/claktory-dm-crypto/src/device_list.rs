//! The signed device list (dms-e2ee.md §2): a person's current leaves, their
//! devices and Claktories, with a version that only goes up, signed by
//! their identity key. Members accept a leaf only if its certificate chains
//! to the pinned identity and the latest list they've seen names it, so the
//! service can't slip in a leaf, and a removed one never comes back.
//!
//! The version is scoped to the identity key that signs it (Fugo L1): it
//! starts again at 1 when the key rotates, and versions are only ever
//! compared under one key (`validation::Pins`).

use tls_codec::{Deserialize as _, Serialize as _, TlsDeserialize, TlsSerialize, TlsSize, VLBytes};

use crate::Error;
use crate::certificate::{LeafCertificate, LeafKind, MAX_ACCOUNT_ID, MAX_LEAF_ID};
use crate::identity::{IdentityKey, IdentityPublic, Purpose};

const VERSION: u8 = 1;
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

/// A device list as its identity signed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedDeviceList {
    pub list: DeviceList,
    pub identity: IdentityPublic,
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

    fn body(&self, identity: IdentityPublic) -> Body {
        Body {
            format: VERSION,
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
        self.check()?;
        let public = identity.public();
        let body = self.body(public).tls_serialize_detached().map_err(|error| Error::Format(error.to_string()))?;
        let signature = identity.sign(Purpose::DeviceList, &body);
        Ok(SignedDeviceList { list: self, identity: public, signature })
    }
}

fn text(bytes: &VLBytes) -> Result<String, Error> {
    String::from_utf8(bytes.as_slice().to_vec()).map_err(|_| Error::Format("not UTF-8".to_owned()))
}

impl SignedDeviceList {
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        Wire { body: self.list.body(self.identity), signature: self.signature }
            .tls_serialize_detached()
            .map_err(|error| Error::Format(error.to_string()))
    }

    /// Reads a list, all of `bytes`, checking its form and that the
    /// identity inside signed it. Whether that identity is the account's
    /// pinned one is `validation`'s.
    pub fn from_bytes(bytes: &[u8]) -> Result<SignedDeviceList, Error> {
        let wire = Wire::tls_deserialize_exact(bytes).map_err(|_| Error::Format("not a device list".to_owned()))?;
        let body = &wire.body;
        if body.format != VERSION {
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
        let signed = body.tls_serialize_detached().map_err(|error| Error::Format(error.to_string()))?;
        identity.verify(Purpose::DeviceList, &signed, &wire.signature)?;
        Ok(SignedDeviceList { list, identity, signature: wire.signature })
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
}
