//! A person's identity key (dms-e2ee.md §2): one Ed25519 key per person,
//! made on their first device and copied only to their own devices, by the
//! QR hand-off (`handoff`). Never on a Claktory, the service or a backup.
//! It signs leaf certificates (`certificate`) and the signed device list
//! (`device_list`); its public half is what friends pin and verify.
//!
//! Every signature is over a purpose label and the signed bytes, so one
//! kind of signed thing can never pass for another.

use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::Error;

/// What a signature is for: part of what's signed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    LeafCertificate,
    DeviceList,
}

impl Purpose {
    fn label(self) -> &'static [u8] {
        match self {
            Purpose::LeafCertificate => b"claktory-dm/v1 leaf certificate",
            Purpose::DeviceList => b"claktory-dm/v1 device list",
        }
    }
}

/// The bytes actually signed: the purpose's label, a zero byte, the bytes.
fn bad_signature() -> Error {
    Error::Refused("a signature doesn't verify".to_owned())
}

fn signed(purpose: Purpose, bytes: &[u8]) -> Vec<u8> {
    let label = purpose.label();
    let mut out = Vec::with_capacity(label.len() + 1 + bytes.len());
    out.extend_from_slice(label);
    out.push(0);
    out.extend_from_slice(bytes);
    out
}

/// The identity key's secret half, kept only on the person's own devices.
pub struct IdentityKey {
    key: SigningKey,
}

impl std::fmt::Debug for IdentityKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "IdentityKey({})", self.public().fingerprint_hex())
    }
}

impl IdentityKey {
    /// A new identity, for a person's first device (or a lost-or-stolen
    /// rotation).
    pub fn generate() -> Result<IdentityKey, Error> {
        let mut seed = Zeroizing::new([0u8; 32]);
        getrandom::fill(seed.as_mut()).map_err(|error| Error::Format(format!("no randomness: {error}")))?;
        Ok(IdentityKey { key: SigningKey::from_bytes(&seed) })
    }

    /// The key from its 32 secret bytes, as the platform key store keeps it.
    pub fn from_secret(secret: &[u8; 32]) -> IdentityKey {
        IdentityKey { key: SigningKey::from_bytes(secret) }
    }

    /// Its 32 secret bytes, for the platform key store and the hand-off only.
    pub fn secret(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(self.key.to_bytes())
    }

    pub fn public(&self) -> IdentityPublic {
        IdentityPublic(self.key.verifying_key().to_bytes())
    }

    pub fn sign(&self, purpose: Purpose, bytes: &[u8]) -> [u8; 64] {
        self.key.sign(&signed(purpose, bytes)).to_bytes()
    }
}

/// An identity key's public half.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct IdentityPublic(pub [u8; 32]);

impl std::fmt::Debug for IdentityPublic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "IdentityPublic({})", self.fingerprint_hex())
    }
}

impl IdentityPublic {
    pub fn from_slice(bytes: &[u8]) -> Result<IdentityPublic, Error> {
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| Error::Format("an identity key is 32 bytes".to_owned()))?;
        // A point that isn't on the curve can verify nothing: refused here.
        VerifyingKey::from_bytes(&bytes).map_err(|_| Error::Format("not an Ed25519 key".to_owned()))?;
        Ok(IdentityPublic(bytes))
    }

    /// Checks `signature` over `bytes` for `purpose` (strict: no malleable
    /// or small-order forms).
    pub fn verify(&self, purpose: Purpose, bytes: &[u8], signature: &[u8]) -> Result<(), Error> {
        let key = VerifyingKey::from_bytes(&self.0).map_err(|_| bad_signature())?;
        let signature: [u8; 64] = signature.try_into().map_err(|_| bad_signature())?;
        key.verify_strict(&signed(purpose, bytes), &Signature::from_bytes(&signature)).map_err(|_| bad_signature())
    }

    /// SHA-256 of the key, labelled: what logs and debug output name it by.
    /// Not the safety number (`safety`), which is what people compare.
    pub fn fingerprint(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(b"claktory-dm/v1 identity fingerprint\0");
        hash.update(self.0);
        hash.finalize().into()
    }

    pub fn fingerprint_hex(&self) -> String {
        self.fingerprint()[..8].iter().map(|byte| format!("{byte:02x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signature_holds_only_for_its_purpose_and_bytes() {
        let key = IdentityKey::generate().unwrap();
        let public = key.public();
        let signature = key.sign(Purpose::DeviceList, b"list");
        assert!(public.verify(Purpose::DeviceList, b"list", &signature).is_ok());
        assert_eq!(
            public.verify(Purpose::LeafCertificate, b"list", &signature).map_err(|e| e.to_string()),
            Err("refused: a signature doesn't verify".to_owned())
        );
        assert_eq!(
            public.verify(Purpose::DeviceList, b"lisT", &signature).map_err(|e| e.to_string()),
            Err("refused: a signature doesn't verify".to_owned())
        );
        let other = IdentityKey::generate().unwrap().public();
        assert_eq!(
            other.verify(Purpose::DeviceList, b"list", &signature).map_err(|e| e.to_string()),
            Err("refused: a signature doesn't verify".to_owned())
        );
        // Kept and read back, it's the same key.
        let again = IdentityKey::from_secret(&key.secret());
        assert_eq!(again.public(), public);
        assert!(IdentityPublic::from_slice(&[0u8; 31]).is_err());
    }
}
