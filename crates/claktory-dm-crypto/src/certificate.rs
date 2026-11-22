//! Leaf certificates (dms-e2ee.md §2): the identity key's signature over
//! one leaf of a person's, a device or a Claktory: who it belongs to
//! (`account_id`), which leaf it is, the leaf's MLS signature key, its name
//! and when it was made. It rides in the leaf's MLS credential, a
//! BasicCredential whose identity bytes are the certificate, TLS-encoded,
//! so every member can check a leaf against the person's pinned identity
//! key (`validation`) and the service can't make one.

use openmls::prelude::{BasicCredential, Credential, CredentialType};
use tls_codec::{DeserializeBytes, Serialize as _, TlsDeserializeBytes, TlsSerialize, TlsSize, VLBytes};

use crate::Error;
use crate::identity::{IdentityKey, IdentityPublic, Purpose};

/// The certificate's format.
const VERSION: u8 = 1;
/// The longest account id, leaf id and name, in bytes.
pub const MAX_ACCOUNT_ID: usize = 64;
pub const MAX_LEAF_ID: usize = 128;
pub const MAX_NAME: usize = 64;

/// What kind of leaf: one of the person's devices, or one of their
/// Claktories (dms-e2ee.md §2, "Claktories").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum LeafKind {
    Device,
    Claktory,
}

impl LeafKind {
    fn code(self) -> u8 {
        match self {
            LeafKind::Device => 1,
            LeafKind::Claktory => 2,
        }
    }

    fn from_code(code: u8) -> Result<LeafKind, Error> {
        match code {
            1 => Ok(LeafKind::Device),
            2 => Ok(LeafKind::Claktory),
            _ => Err(Error::Format(format!("unknown leaf kind {code}"))),
        }
    }
}

/// A leaf, as its certificate names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leaf {
    pub kind: LeafKind,
    pub account_id: String,
    /// A device's account key, or a Claktory's `claktory_leaf_id`.
    pub leaf_id: String,
    /// The leaf's MLS signature key (Ed25519).
    pub signature_key: [u8; 32],
    /// What people see it called ("iPad", "mac-mini"): one line.
    pub name: String,
    pub created_ms: u64,
}

/// A Claktory's leaf id: its id and its registration epoch, so a Claktory
/// signed in again (a new epoch, a move) needs a new certificate.
pub fn claktory_leaf_id(claktory_id: &str, registration_epoch: u64) -> String {
    format!("{claktory_id}#{registration_epoch}")
}

/// The Claktory id in a Claktory's leaf id.
pub fn claktory_of(leaf_id: &str) -> Option<&str> {
    let (claktory, epoch) = leaf_id.rsplit_once('#')?;
    (!claktory.is_empty() && !epoch.is_empty() && epoch.bytes().all(|b| b.is_ascii_digit())).then_some(claktory)
}

#[derive(Debug, Clone, PartialEq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
struct Body {
    version: u8,
    kind: u8,
    account_id: VLBytes,
    leaf_id: VLBytes,
    signature_key: [u8; 32],
    name: VLBytes,
    created_ms: u64,
    identity: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
struct Wire {
    body: Body,
    signature: [u8; 64],
}

/// A leaf certificate: the leaf, signed by `identity`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeafCertificate {
    pub leaf: Leaf,
    /// The identity key that signed it (checked against the pin, never
    /// trusted for itself).
    pub identity: IdentityPublic,
    signature: [u8; 64],
}

fn ascii_id(what: &str, id: &str, most: usize) -> Result<(), Error> {
    if id.is_empty() || id.len() > most || !id.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(Error::Format(format!("{what} is 1 to {most} printable ASCII characters")));
    }
    Ok(())
}

/// A character that shows nothing, or shows as something else: the same
/// set the app's review card marks (`hiddenCharacter` in
/// app/lib/people/review.dart), plus every control character and the
/// braille blank. Friends see these names.
pub fn hidden(c: char) -> bool {
    let rune = u32::from(c);
    c.is_control()
        || rune == 0x00ad
        || rune == 0x034f
        || rune == 0x061c
        || rune == 0x115f
        || rune == 0x1160
        || rune == 0x17b4
        || rune == 0x17b5
        || (0x180b..=0x180f).contains(&rune)
        || (0x200b..=0x200f).contains(&rune)
        || (0x2028..=0x202e).contains(&rune)
        || (0x2060..=0x206f).contains(&rune)
        || rune == 0x2800
        || rune == 0x3164
        || rune == 0xfeff
        || rune == 0xffa0
        || (0xfff9..=0xfffb).contains(&rune)
        || (0x1d173..=0x1d17a).contains(&rune)
        || (0xe0000..=0xe0fff).contains(&rune)
}

/// One line of at most MAX_NAME bytes with nothing hidden in it. A
/// variation selector (U+FE00–FE0F) is a visible character's only when it's
/// the one right after it (an emoji's); anywhere else it's refused.
fn check_name(name: &str) -> Result<(), Error> {
    if one_line(name, MAX_NAME) {
        Ok(())
    } else {
        Err(Error::Format(format!("a leaf's name is one line of 1 to {MAX_NAME} bytes, with nothing hidden in it")))
    }
}

/// Whether `text` is one line of 1 to `most` bytes with nothing hidden in it:
/// a variation selector only right after a visible character.
pub(crate) fn one_line(text: &str, most: usize) -> bool {
    if text.trim().is_empty() || text.len() > most {
        return false;
    }
    let mut after_visible = false;
    for c in text.chars() {
        if ('\u{fe00}'..='\u{fe0f}').contains(&c) {
            if !after_visible {
                return false;
            }
            after_visible = false;
        } else if hidden(c) {
            return false;
        } else {
            after_visible = !c.is_whitespace();
        }
    }
    true
}

impl Leaf {
    fn check(&self) -> Result<(), Error> {
        ascii_id("an account id", &self.account_id, MAX_ACCOUNT_ID)?;
        ascii_id("a leaf id", &self.leaf_id, MAX_LEAF_ID)?;
        if self.kind == LeafKind::Claktory && claktory_of(&self.leaf_id).is_none() {
            return Err(Error::Format("a Claktory's leaf id is its id and registration epoch".to_owned()));
        }
        check_name(&self.name)
    }

    fn body(&self, identity: IdentityPublic) -> Body {
        Body {
            version: VERSION,
            kind: self.kind.code(),
            account_id: self.account_id.as_bytes().to_vec().into(),
            leaf_id: self.leaf_id.as_bytes().to_vec().into(),
            signature_key: self.signature_key,
            name: self.name.as_bytes().to_vec().into(),
            created_ms: self.created_ms,
            identity: identity.0,
        }
    }
}

fn text(bytes: &VLBytes) -> Result<String, Error> {
    String::from_utf8(bytes.as_slice().to_vec()).map_err(|_| Error::Format("not UTF-8".to_owned()))
}

impl LeafCertificate {
    /// `identity` certifies `leaf`.
    pub fn issue(identity: &IdentityKey, leaf: Leaf) -> Result<LeafCertificate, Error> {
        leaf.check()?;
        let public = identity.public();
        let body = leaf.body(public).tls_serialize_detached().map_err(|error| Error::Format(error.to_string()))?;
        let signature = identity.sign(Purpose::LeafCertificate, &body);
        Ok(LeafCertificate { leaf, identity: public, signature })
    }

    /// Checks the signature against the identity named inside. Whether that
    /// identity is the one pinned for the account is `validation`'s.
    pub fn verify(&self) -> Result<(), Error> {
        let body = self.leaf.body(self.identity).tls_serialize_detached().map_err(|error| Error::Format(error.to_string()))?;
        self.identity.verify(Purpose::LeafCertificate, &body, &self.signature)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let wire = Wire { body: self.leaf.body(self.identity), signature: self.signature };
        wire.tls_serialize_detached().map_err(|error| Error::Format(error.to_string()))
    }

    /// Reads a certificate, all of `bytes` and nothing after, checking its
    /// form and its signature.
    pub fn from_bytes(bytes: &[u8]) -> Result<LeafCertificate, Error> {
        let wire = Wire::tls_deserialize_exact_bytes(bytes).map_err(|_| Error::Format("not a leaf certificate".to_owned()))?;
        let body = wire.body;
        if body.version != VERSION {
            return Err(Error::Format(format!("leaf certificate version {}", body.version)));
        }
        let leaf = Leaf {
            kind: LeafKind::from_code(body.kind)?,
            account_id: text(&body.account_id)?,
            leaf_id: text(&body.leaf_id)?,
            signature_key: body.signature_key,
            name: text(&body.name)?,
            created_ms: body.created_ms,
        };
        leaf.check()?;
        let certificate = LeafCertificate { leaf, identity: IdentityPublic::from_slice(&body.identity)?, signature: wire.signature };
        certificate.verify()?;
        Ok(certificate)
    }

    /// The MLS credential that carries it.
    pub fn to_credential(&self) -> Result<Credential, Error> {
        Ok(BasicCredential::new(self.to_bytes()?).into())
    }

    /// The certificate in an MLS credential, checked; refused for any other
    /// kind of credential.
    pub fn from_credential(credential: &Credential) -> Result<LeafCertificate, Error> {
        if credential.credential_type() != CredentialType::Basic {
            return Err(Error::Format("not a basic credential".to_owned()));
        }
        let basic = BasicCredential::try_from(credential.clone()).map_err(|_| Error::Format("not a basic credential".to_owned()))?;
        LeafCertificate::from_bytes(basic.identity())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn leaf(kind: LeafKind, account: &str, id: &str, key: [u8; 32]) -> Leaf {
        Leaf {
            kind,
            account_id: account.to_owned(),
            leaf_id: id.to_owned(),
            signature_key: key,
            name: "iPad".to_owned(),
            created_ms: 1_763_700_000_000,
        }
    }

    #[test]
    fn a_certificate_travels_in_a_credential_and_any_change_breaks_it() {
        let identity = IdentityKey::generate().unwrap();
        let certificate = LeafCertificate::issue(&identity, leaf(LeafKind::Device, "acct-1", "dev-1", [7; 32])).unwrap();
        let credential = certificate.to_credential().unwrap();
        let read = LeafCertificate::from_credential(&credential).unwrap();
        assert_eq!(read, certificate);
        assert_eq!(read.identity, identity.public());

        // A byte changed anywhere, or one added, is refused.
        let bytes = certificate.to_bytes().unwrap();
        for at in [1, 5, bytes.len() / 2, bytes.len() - 1] {
            let mut changed = bytes.clone();
            changed[at] ^= 1;
            assert!(LeafCertificate::from_bytes(&changed).is_err(), "byte {at}");
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(LeafCertificate::from_bytes(&longer).is_err());

        // Another identity claiming it is refused: the signature is over it.
        let mut stolen = certificate.clone();
        stolen.identity = IdentityKey::generate().unwrap().public();
        assert_eq!(stolen.verify().map_err(|e| e.to_string()), Err("refused: a signature doesn't verify".to_owned()));
    }

    #[test]
    fn what_a_certificate_may_say_is_checked() {
        let identity = IdentityKey::generate().unwrap();
        let mut bad = leaf(LeafKind::Device, "acct-1", "dev-1", [7; 32]);
        bad.name = "two\nlines".to_owned();
        assert!(LeafCertificate::issue(&identity, bad).is_err());
        let mut bad = leaf(LeafKind::Device, "acct-1", "dev-1", [7; 32]);
        bad.name = "a\u{202e}gnp.exe".to_owned();
        assert!(LeafCertificate::issue(&identity, bad).is_err());
        for hidden in ["a\u{2028}b", "a\u{2063}b", "soft\u{ad}hyphen", "\u{3164}", "a\u{fe0f}\u{fe0f}", " \u{fe0f}", "a\u{e0041}"] {
            let mut bad = leaf(LeafKind::Device, "acct-1", "dev-1", [7; 32]);
            bad.name = hidden.to_owned();
            assert!(LeafCertificate::issue(&identity, bad).is_err(), "{hidden:?}");
        }
        let mut heart = leaf(LeafKind::Device, "acct-1", "dev-1", [7; 32]);
        heart.name = "Ana\u{2764}\u{fe0f} iPad".to_owned();
        assert!(LeafCertificate::issue(&identity, heart).is_ok(), "an emoji's own selector");
        assert!(LeafCertificate::issue(&identity, leaf(LeafKind::Device, "", "dev-1", [7; 32])).is_err());
        assert!(LeafCertificate::issue(&identity, leaf(LeafKind::Claktory, "acct-1", "no-epoch", [7; 32])).is_err());
        let id = claktory_leaf_id("mac-mini-1a2b", 3);
        assert_eq!(claktory_of(&id), Some("mac-mini-1a2b"));
        assert!(LeafCertificate::issue(&identity, leaf(LeafKind::Claktory, "acct-1", &id, [7; 32])).is_ok());
    }
}
