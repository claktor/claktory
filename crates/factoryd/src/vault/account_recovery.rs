//! The account's recovery key (#239; coordination/security/
//! recovery-and-sign-in.md, part 1): one key for every Claktory of an
//! account at claktory.com. Its 20 bytes make an X25519 key pair; each
//! Claktory seals its own vault's key to the public half (protector kind
//! `account-recovery`, params `{"account_id", "public_key"}`), which can
//! seal but never open. The key is made on the person's device, which
//! shows it and forgets it; no Claktory ever holds its private half. The
//! account module decides which key to seal to (`account::recovery`); this
//! only seals and opens.

use curve25519_dalek::montgomery::MontgomeryPoint;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::{Key, Protector, Vault, VaultError, aad, crypto};

/// The protector's kind.
pub(crate) const KIND: &str = "account-recovery";

const PAIR_SALT: &[u8] = b"claktory account recovery key";
const PAIR_INFO: &str = "x25519 v1";
const SEAL_INFO: &str = "claktory vault account recovery v1";

/// The key pair a recovery key's 20 bytes make: secret =
/// HKDF-SHA256(bytes, salt "claktory account recovery key", info "x25519
/// v1"), public = X25519(secret, the base point). The apps make the same
/// pair from a key they make (NewRecoveryKey.public_key).
pub(crate) fn pair(bytes: &[u8; 20]) -> (Key, [u8; 32]) {
    let secret = crypto::derive(bytes, PAIR_SALT, PAIR_INFO);
    let public = MontgomeryPoint::mul_base_clamped(*secret).to_bytes();
    (secret, public)
}

/// `plaintext` sealed to `public` (X25519): a fresh key pair's public half,
/// then `crypto::seal` under HKDF(the shared secret, salt = that half ‖
/// `public`). None for a public half that gives no shared secret (a
/// low-order point, such as all zeros).
pub(crate) fn seal_to(public: &[u8; 32], plaintext: &[u8], aad: &[u8]) -> Option<Vec<u8>> {
    let ephemeral = crypto::new_key();
    let sent = MontgomeryPoint::mul_base_clamped(*ephemeral).to_bytes();
    let shared = Zeroizing::new(MontgomeryPoint(*public).mul_clamped(*ephemeral).to_bytes());
    if shared.iter().all(|byte| *byte == 0) {
        return None;
    }
    let key = crypto::derive(shared.as_slice(), &[sent.as_slice(), public.as_slice()].concat(), SEAL_INFO);
    let mut out = sent.to_vec();
    out.extend_from_slice(&crypto::seal(&key, plaintext, aad));
    Some(out)
}

/// What `seal_to` sealed to the public half of `secret`.
pub(crate) fn open_from(secret: &[u8; 32], sealed: &[u8], aad: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    let (sent, rest) = sealed.split_at_checked(32)?;
    let sent: [u8; 32] = sent.try_into().ok()?;
    let public = MontgomeryPoint::mul_base_clamped(*secret).to_bytes();
    let shared = Zeroizing::new(MontgomeryPoint(sent).mul_clamped(*secret).to_bytes());
    if shared.iter().all(|byte| *byte == 0) {
        return None;
    }
    let key = crypto::derive(shared.as_slice(), &[sent.as_slice(), public.as_slice()].concat(), SEAL_INFO);
    crypto::open(&key, rest, aad)
}

/// A public half that can be sealed to: 32 bytes of base64url that give a
/// shared secret.
pub(crate) fn public_of(text: &str) -> Option<[u8; 32]> {
    let public = claktory_account::protocol::unb64_32(text)?;
    seal_to(&public, b"", b"").map(|_| public)
}

/// The protector's params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Sealed {
    pub account_id: String,
    /// base64url.
    pub public_key: String,
}

/// The vault's key, if `typed` is the account's recovery key it's sealed to.
pub(super) fn open_with(vault_id: &str, protector: &Protector, typed: &str) -> Option<Key> {
    let bytes = crypto::parse_recovery(typed)?;
    let (secret, _) = pair(&bytes);
    let vmk = open_from(&secret, &protector.sealed_vmk, &aad(&["vmk", vault_id, KIND]))?;
    let vmk: [u8; 32] = vmk.as_slice().try_into().ok()?;
    Some(Zeroizing::new(vmk))
}

/// Which account key the vault holds now: a hash of its protector, which
/// a new seal replaces. None if it has none.
pub(super) fn generation(db: &Connection) -> Result<Option<String>, VaultError> {
    let sealed: Option<Vec<u8>> =
        db.query_row("SELECT sealed_vmk FROM protectors WHERE kind = ?1", [KIND], |row| row.get(0)).optional()?;
    Ok(sealed.map(|sealed| crypto::hex(&Sha256::digest(&sealed)[..16])))
}

fn sealed_of(db: &Connection) -> Result<Option<Sealed>, VaultError> {
    let params: Option<String> = db.query_row("SELECT params FROM protectors WHERE kind = ?1", [KIND], |row| row.get(0)).optional()?;
    Ok(params.and_then(|params| serde_json::from_str(&params).ok()))
}

pub(super) fn kept(db: &Connection, name: &str) -> Result<bool, VaultError> {
    Ok(db.query_row("SELECT COUNT(*) > 0 FROM kept WHERE name = ?1", [name], |row| row.get(0))?)
}

impl Vault {
    /// The account key the vault is sealed to, if any (locked too).
    pub(crate) fn account_key(&self) -> Option<Sealed> {
        let inner = self.lock();
        sealed_of(inner.db.as_ref()?).ok().flatten()
    }

    /// Whether the vault is sealed to an account's recovery key.
    pub(crate) fn account_covered_now(&self) -> bool {
        let inner = self.lock();
        inner.db.as_ref().is_some_and(|db| Self::account_covered(db).unwrap_or(false))
    }

    /// Seals the vault to the account key `public` of `account_id`, in place
    /// of any account key before, and drops this Claktory's own recovery
    /// key, kept, confirmed or not: the person made a new key and saved it,
    /// so a key saved before stops opening the vault here (copies made
    /// earlier still open with it). In one transaction, so the own key goes
    /// only once the account's protector is saved. Whether anything changed:
    /// already sealed to it, nothing.
    pub(crate) fn seal_to_account_key(&self, account_id: &str, public: &[u8; 32]) -> Result<bool, VaultError> {
        let wanted = Sealed { account_id: account_id.to_owned(), public_key: claktory_account::protocol::b64(public) };
        self.with_unlocked_transaction(|db, vault_id, vmk| {
            if sealed_of(db)?.as_ref() == Some(&wanted) {
                return Ok(false);
            }
            Self::save_account_protector(db, vault_id, vmk, account_id, public)?;
            db.execute("DELETE FROM protectors WHERE kind = 'recovery'", [])?;
            db.execute("DELETE FROM kept WHERE name IN ('recovery', 'recovery-confirmed')", [])?;
            Ok(true)
        })
    }

    fn save_account_protector(db: &Connection, vault_id: &str, vmk: &[u8; 32], account_id: &str, public: &[u8; 32]) -> Result<(), VaultError> {
        let sealed_vmk = seal_to(public, vmk.as_slice(), &aad(&["vmk", vault_id, KIND]))
            .ok_or_else(|| VaultError::Invalid("That isn't a recovery key Claktory can use.".to_owned()))?;
        let params = Sealed { account_id: account_id.to_owned(), public_key: claktory_account::protocol::b64(public) };
        let params = serde_json::to_string(&params).map_err(|error| VaultError::Invalid(error.to_string()))?;
        Self::save_protector(db, &Protector { kind: KIND.to_owned(), params, salt: Vec::new(), sealed_vmk })
    }
}

#[cfg(test)]
mod tests {
    use super::super::Inner;
    use super::*;

    /// The pair the apps must make too (NewRecoveryKey.public_key): fixed
    /// bytes, the key's text, its secret and public halves.
    #[test]
    fn the_key_pair_matches_the_test_vectors() {
        for (bytes, text, secret, public) in VECTORS {
            let bytes: [u8; 20] = crypto::unhex(bytes).expect("hex").try_into().expect("20 bytes");
            assert_eq!(*crypto::parse_recovery(text).expect("a key"), bytes, "{text}");
            let (made_secret, made_public) = pair(&bytes);
            assert_eq!(crypto::hex(made_secret.as_slice()), *secret);
            assert_eq!(claktory_account::protocol::b64(made_public), *public);
        }
    }

    /// Made independently (scratch Python: HKDF-SHA256 and RFC 7748's
    /// ladder, checked against its test vector); the apps test against
    /// these too. The secret is HKDF's output, before X25519 clamps it.
    const VECTORS: &[(&str, &str, &str, &str)] = &[
        (
            "0000000000000000000000000000000000000000",
            "AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA",
            "5906fe73256ccab22ef9d9894461438e376a8c420307c8f9f15e37f172c37b5e",
            "OY7HY7p0nFXYrit_NNSrRzmY1naOPSQKwl6ClraDTB0",
        ),
        (
            "ffffffffffffffffffffffffffffffffffffffff",
            "7777-7777-7777-7777-7777-7777-7777-7777",
            "5b4be80b6006371ac1757eac3b647c2925646524559b6a46c5d015843014a24b",
            "imjB5yXy5aSTfJFELO2BIWvXoDCvsHK4l2aiaIibTB0",
        ),
        (
            "000102030405060708090a0b0c0d0e0f10111213",
            "AAAQ-EAYE-AUDA-OCAJ-BIFQ-YDIO-B4IB-CEQT",
            "cb76fa5021ee491a0059e4153a9bea00a8d23f7de4b1410259e86dce4ad5994d",
            "qU6lrzk7yMBKtkxxfLgKy--CXtyNbdMMhbY1UTf8LXA",
        ),
        (
            "7c3f9a12e05bd4886a1f2c3e4d5b6a7988a9bacb",
            "PQ7Z-UEXA-LPKI-Q2Q7-FQ7E-2W3K-PGEK-TOWL",
            "84d0d623cfe7b32e94b3d243468d4d391cc0afdd5097adc1f567e90a453ebb0b",
            "97qGmEPgeDisxBR-rxzKuht-GT2rfJ6EURUeilzWTzg",
        ),
    ];

    fn vault() -> Vault {
        let dir = std::env::temp_dir().join(format!("vault-account-recovery-{}", crypto::hex(&crypto::random::<8>())));
        let vault = Vault::open_or_make(dir.join("vault"));
        vault.protect_with_account("acct1", "{}", &[7u8; 64]).expect("signed in");
        vault
    }

    fn read<T>(vault: &Vault, what: impl Fn(&Inner) -> Result<T, VaultError>) -> T {
        what(&vault.lock()).expect("read")
    }

    /// The account's key typed back: whether it opens the vault, and the
    /// generation it was.
    fn opens(vault: &Vault, typed: &str) -> Option<String> {
        let (vault_id, protectors) = vault.protectors().expect("protectors");
        Vault::check_which(&vault_id, &protectors, typed).map(|(_, generation)| generation)
    }

    #[test]
    fn the_accounts_key_opens_the_vault_sealed_to_it_and_covers_it() {
        let vault = vault();
        let own = vault.recovery_key("Mac").expect("its own, as before");
        assert!(read(&vault, Vault::recovery_unconfirmed) && read(&vault, Vault::recovery_showable));
        let (text, bytes) = crypto::new_recovery();
        let (_, public) = pair(&bytes);
        assert!(vault.seal_to_account_key("acct1", &public).expect("sealed"));
        assert!(!vault.seal_to_account_key("acct1", &public).expect("again"), "already sealed to it");
        assert_eq!(vault.account_key(), Some(Sealed { account_id: "acct1".into(), public_key: claktory_account::protocol::b64(public) }));
        assert!(vault.account_covered_now());
        let generation = opens(&vault, &text).expect("the account's key opens it");
        assert_eq!(Some(generation.clone()), read(&vault, |inner| super::generation(Vault::db(inner)?)));
        assert!(opens(&vault, &crypto::new_recovery().0).is_none(), "another key doesn't");
        // Its own key no longer opens it, and isn't asked about or shown:
        // no Claktory keeps the account's either.
        assert!(opens(&vault, &own).is_none(), "the key saved before no longer opens it");
        assert!(!read(&vault, Vault::recovery_unconfirmed) && !read(&vault, Vault::recovery_showable));
        let refused = vault.recovery_key("Mac").expect_err("nothing to show");
        assert!(refused.to_string().contains("uses your account's recovery key"), "{refused}");

        // Typed to open it, it's been seen here, until it's another key.
        assert!(!read(&vault, Vault::recovery_used));
        vault.note_recovery_used(&generation).expect("noted");
        assert!(read(&vault, Vault::recovery_used));
        let (_, other) = pair(&[9; 20]);
        assert!(vault.seal_to_account_key("acct1", &other).expect("sealed"));
        assert!(!read(&vault, Vault::recovery_used));
        assert!(opens(&vault, &text).is_none(), "the old key no longer opens this vault");
    }

    /// Sealed to the account's key, a key of its own the person confirmed
    /// (saved) goes too, and isn't made again at the next sign-in; a copy
    /// of the vault made before still opens with it.
    #[test]
    fn a_key_of_its_own_stops_opening_the_vault_once_the_accounts_covers_it() {
        let vault = vault();
        let own = vault.recovery_key("Mac").expect("its own");
        let check = vault.check_recovery_key("Mac").expect("check");
        let group = own.split('-').nth(check.group as usize - 1).expect("group").to_owned();
        vault.confirm_recovery_key("Mac", &check.check_id, &group).expect("confirmed");
        let (before_id, before) = vault.protectors().expect("protectors");
        let (text, bytes) = crypto::new_recovery();
        let (_, public) = pair(&bytes);
        assert!(vault.seal_to_account_key("acct1", &public).expect("sealed"));
        assert!(opens(&vault, &own).is_none(), "the key saved before no longer opens it");
        assert!(opens(&vault, &text).is_some(), "the new one does");
        assert!(Vault::check_which(&before_id, &before, &own).is_some(), "a copy made before still opens with it");
        assert!(!read(&vault, Vault::recovery_showable) && !read(&vault, Vault::recovery_unconfirmed));
        assert!(vault.recovery_key("Mac").is_err(), "nothing of its own to show");
        vault.protect_with_account("acct1", "{}", &[8u8; 64]).expect("signed in again");
        assert!(opens(&vault, &own).is_none() && !read(&vault, Vault::recovery_showable), "no key of its own made again");
        let (_, protectors) = vault.protectors().expect("protectors");
        assert!(protectors.iter().any(|protector| protector.kind == KIND), "tried on a typed key");
        assert!(!protectors.iter().any(|protector| protector.kind == "recovery"));
    }

    #[test]
    fn sealed_to_a_public_half_opens_only_with_its_secret() {
        let (secret, public) = pair(&[7; 20]);
        let sealed = seal_to(&public, b"the vault's key", b"aad").expect("sealed");
        assert_eq!(open_from(&secret, &sealed, b"aad").as_deref().map(Vec::as_slice), Some(&b"the vault's key"[..]));
        assert!(open_from(&secret, &sealed, b"other").is_none(), "other associated data");
        let (other, _) = pair(&[8; 20]);
        assert!(open_from(&other, &sealed, b"aad").is_none(), "another key");
        assert_ne!(seal_to(&public, b"x", b"").unwrap(), seal_to(&public, b"x", b"").unwrap(), "a fresh ephemeral each time");
        assert!(seal_to(&[0; 32], b"x", b"").is_none(), "a low-order point");
        assert!(public_of(&claktory_account::protocol::b64([0u8; 32])).is_none());
        assert_eq!(public_of(&claktory_account::protocol::b64(public)), Some(public));
    }
}
