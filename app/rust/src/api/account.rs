//! This device's key with the person's Claktory account (claktory.com),
//! for Dart: it signs the device's calls to the account service
//! (crates/claktory-account/API.md, "Devices"). Like a device key, it's
//! any 32 bytes as lowercase hex, an Ed25519 secret key, made and kept on
//! this device; only its public half is ever sent.

use flutter_rust_bridge::frb;
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroize;

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

/// The public half of the X25519 pair the account recovery key [key] (its
/// 20 bytes) stands for: the secret is HKDF-SHA256 of the key, with salt
/// "claktory account recovery key" and info "x25519 v1", and the public
/// half X25519 of it and the base point. factoryd opens a vault with a
/// typed key the same way, so only the public half ever leaves the device.
#[frb(sync)]
pub fn recovery_public_key(key: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    let mut key = key;
    if key.len() != 20 {
        let length = key.len();
        key.zeroize();
        anyhow::bail!("a recovery key is 20 bytes, not {length}");
    }
    let mut secret = [0u8; 32];
    Hkdf::<Sha256>::new(Some(b"claktory account recovery key"), &key)
        .expand(b"x25519 v1", &mut secret)
        .map_err(|_| anyhow::anyhow!("HKDF couldn't make 32 bytes"))?;
    let public = x25519_dalek::x25519(secret, x25519_dalek::X25519_BASEPOINT_BYTES);
    secret.zeroize();
    key.zeroize();
    Ok(public.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// factoryd's vectors (vault/account_recovery.rs): the key's bytes and
    /// the public half it makes there, which the device must make too.
    #[test]
    fn a_recovery_key_makes_the_public_half_factoryd_makes() {
        use base64::Engine;
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        for (bytes, public) in [
            ("0000000000000000000000000000000000000000", "OY7HY7p0nFXYrit_NNSrRzmY1naOPSQKwl6ClraDTB0"),
            ("ffffffffffffffffffffffffffffffffffffffff", "imjB5yXy5aSTfJFELO2BIWvXoDCvsHK4l2aiaIibTB0"),
            ("000102030405060708090a0b0c0d0e0f10111213", "qU6lrzk7yMBKtkxxfLgKy--CXtyNbdMMhbY1UTf8LXA"),
            ("7c3f9a12e05bd4886a1f2c3e4d5b6a7988a9bacb", "97qGmEPgeDisxBR-rxzKuht-GT2rfJ6EURUeilzWTzg"),
        ] {
            assert_eq!(b64.encode(recovery_public_key(hex::decode(bytes).unwrap()).unwrap()), public, "{bytes}");
        }
    }

    #[test]
    fn a_recovery_key_that_isnt_20_bytes_is_refused() {
        assert!(recovery_public_key(vec![0; 19]).is_err());
        assert!(recovery_public_key(vec![0; 32]).is_err());
    }

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

    /// The app's move approval (app/lib/device_approval.dart) is
    /// byte for byte claktory-account's sign_move_approval: this vector is
    /// that function's answer for the key 03…03 and the payload below,
    /// which test/device_approval_test.dart builds too.
    #[test]
    fn a_move_approval_signs_as_the_account_service_does() {
        use base64::Engine;
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let vector = "eyJ2IjoxLCJraW5kIjoibW92ZSIsInJlcXVlc3RfaWQiOiJyZXEtMSIsImFjY291bnRfaWQiOiJhY2N0LTEiLCJzb3VyY2VfY2xha3RvcnlfaWQiOiJzdHVkaW8tMSIsImVuZHBvaW50IjoiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYiIsImZpbmdlcnByaW50IjoiMGU5NDc3Y2Y0NjA1YmUzYyIsImRldmljZV9rZXkiOiI3VWtveGlqUndzYnE2UU00a0ZtVllTbFpKenBjWV9rMk5zRkdGS3lITjlFIiwiaWF0IjoxMDAwLCJleHAiOjE2MDB9.ar5HwXBga_pMT6SAnPnrxyU8kHXWceZo5HjwywCdtJ8mUinyDLbmnFqQmAj8kplWVha5PrUqige1c5rhhw4FAQ";
        let (payload, signature) = vector.split_once('.').unwrap();
        let payload = b64.decode(payload).unwrap();
        let key = "03".repeat(32);
        assert_eq!(
            b64.encode(account_public_key(key.clone()).unwrap()),
            "7UkoxijRwsbq6QM4kFmVYSlZJzpcY_k2NsFGFKyHN9E"
        );
        let message = [b"claktory move approval v1\n".as_slice(), &payload].concat();
        assert_eq!(b64.encode(account_sign(key, message).unwrap()), signature);
    }

    #[test]
    fn a_key_that_isnt_32_bytes_of_hex_is_refused() {
        assert!(account_public_key("abc".into()).is_err());
        assert!(account_sign("zz".repeat(32), vec![]).is_err());
    }
}
