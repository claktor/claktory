//! This device's key with the person's Claktory account (claktory.com),
//! for Dart: it signs the device's calls to the account service
//! (crates/claktory-account/API.md, "Devices"). Like a device key, it's
//! any 32 bytes as lowercase hex, an Ed25519 secret key, made and kept on
//! this device; only its public half is ever sent.

use flutter_rust_bridge::frb;

use crate::tunnel;

/// The public half of the account key [key], 32 bytes.
#[frb(sync)]
pub fn account_public_key(key: String) -> anyhow::Result<Vec<u8>> {
    Ok(tunnel::parse_key(&key)?.public().as_bytes().to_vec())
}

/// [message] signed with the account key [key]: an Ed25519 signature, 64
/// bytes.
#[frb(sync)]
pub fn account_sign(key: String, message: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    Ok(tunnel::parse_key(&key)?.sign(&message).to_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_verify_with_the_public_key() {
        let key = tunnel::generate_key();
        let public: [u8; 32] = account_public_key(key.clone()).unwrap().try_into().unwrap();
        let signature: [u8; 64] = account_sign(key, b"claktory-account request v1\n".to_vec()).unwrap().try_into().unwrap();
        let public = iroh::PublicKey::from_bytes(&public).unwrap();
        let signature = iroh::Signature::from_bytes(&signature);
        assert!(public.verify(b"claktory-account request v1\n", &signature).is_ok());
        assert!(public.verify(b"something else", &signature).is_err());
    }

    #[test]
    fn a_key_that_isnt_32_bytes_of_hex_is_refused() {
        assert!(account_public_key("abc".into()).is_err());
        assert!(account_sign("zz".repeat(32), vec![]).is_err());
    }
}
