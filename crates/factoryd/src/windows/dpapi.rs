//! The Data Protection API in this user's scope, for what the backend keeps
//! on disk that only this user, signed in on this PC, may open: the vault's
//! key on disk (`auto.key`).
//!
//! Always the user's scope, never `CRYPTPROTECT_LOCAL_MACHINE` (any account
//! on the PC could open that), never a prompt, and always with entropy, so a
//! blob made for one purpose doesn't open for another. It keeps a copy taken
//! off the PC, or read by another account, closed; it doesn't keep it from
//! other programs of the same user, which can call the same function.
//!
//! Any failure is an error for the caller to fail closed on (stay locked and
//! ask), never a fallback.

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData};
use zeroize::{Zeroize, Zeroizing};

/// The only flags used: no prompt, user scope.
const FLAGS: u32 = CRYPTPROTECT_UI_FORBIDDEN;

/// The largest blob `unprotect` takes: a sealed 32-byte key is a few
/// hundred bytes.
pub const MOST_SEALED: usize = 4096;

/// `secret` sealed for this user, bound to `entropy`.
pub fn protect(secret: &[u8], entropy: &[u8]) -> std::io::Result<Vec<u8>> {
    let input = blob(secret)?;
    let entropy = blob(entropy)?;
    let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    // SAFETY: the input blobs point at live slices CryptProtectData only
    // reads; it allocates the output with LocalAlloc, freed below.
    let made = unsafe { CryptProtectData(&input, std::ptr::null(), &entropy, std::ptr::null(), std::ptr::null(), FLAGS, &mut output) };
    if made == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: on success the output holds cbData bytes at pbData, ours to free.
    Ok(unsafe { take(output, false) }.to_vec())
}

/// What `protect` sealed, if this user sealed it with this `entropy` and it
/// wasn't changed since.
pub fn unprotect(sealed: &[u8], entropy: &[u8]) -> std::io::Result<Zeroizing<Vec<u8>>> {
    if sealed.len() > MOST_SEALED {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "too large to be a sealed key"));
    }
    let input = blob(sealed)?;
    let entropy = blob(entropy)?;
    let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    // SAFETY: as in `protect`; no description is asked for.
    let opened =
        unsafe { CryptUnprotectData(&input, std::ptr::null_mut(), &entropy, std::ptr::null(), std::ptr::null(), FLAGS, &mut output) };
    if opened == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: as in `protect`; the plain bytes are wiped before they're freed.
    Ok(unsafe { take(output, true) })
}

/// How a sealed key file starts: the format's version.
pub const SEALED_KEY: &str = "dpapi1:";

/// The entropy for the vault's key on disk: this vault, this format
/// (no home id, so a renamed home still opens).
fn auto_key_entropy(vault_id: &str) -> Vec<u8> {
    let mut entropy = b"claktory vault auto.key v1\0".to_vec();
    entropy.extend_from_slice(vault_id.as_bytes());
    entropy
}

/// The text of the vault's key file for the key in `hex`: `dpapi1:` and
/// the sealed key, in hex.
pub fn seal_auto_key(hex: &str, vault_id: &str) -> std::io::Result<String> {
    let sealed = protect(hex.as_bytes(), &auto_key_entropy(vault_id))?;
    Ok(format!("{SEALED_KEY}{}", crate::encoding::hex(&sealed)))
}

/// The key, in hex, from a key file's `text`, if it's sealed (a plain one
/// is refused), by this user, for this vault.
pub fn open_auto_key(text: &str, vault_id: &str) -> Option<Zeroizing<String>> {
    let sealed = crate::encoding::unhex(text.trim_end().strip_prefix(SEALED_KEY)?)?;
    let opened = unprotect(&sealed, &auto_key_entropy(vault_id))
        .inspect_err(|error| tracing::warn!(%error, "the vault's key on disk doesn't open for this user"))
        .ok()?;
    Some(Zeroizing::new(String::from_utf8(opened.to_vec()).ok()?))
}

fn blob(bytes: &[u8]) -> std::io::Result<CRYPT_INTEGER_BLOB> {
    let length = u32::try_from(bytes.len()).map_err(|_| std::io::Error::other("too large to protect"))?;
    // DPAPI takes a mutable pointer but only reads its inputs.
    Ok(CRYPT_INTEGER_BLOB { cbData: length, pbData: bytes.as_ptr().cast_mut() })
}

/// Copies DPAPI's output out and frees it, wiping it first when `wipe`.
///
/// # Safety
/// `output` must be a successful call's, not freed yet.
unsafe fn take(output: CRYPT_INTEGER_BLOB, wipe: bool) -> Zeroizing<Vec<u8>> {
    if output.pbData.is_null() {
        return Zeroizing::new(Vec::new());
    }
    // SAFETY: the caller's promise: cbData bytes at pbData.
    let bytes = unsafe { std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize) };
    let copy = Zeroizing::new(bytes.to_vec());
    if wipe {
        bytes.zeroize();
    }
    // SAFETY: DPAPI allocated it with LocalAlloc; nothing refers to it any more.
    unsafe { LocalFree(output.pbData.cast()) };
    copy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_opens_only_with_its_entropy_and_unchanged() {
        let secret = [7u8; 32];
        let sealed = protect(&secret, b"claktory test v1\0vault-a").expect("protect");
        assert!(!sealed.windows(secret.len()).any(|window| window == secret), "the secret shows in the blob");
        let opened = unprotect(&sealed, b"claktory test v1\0vault-a").expect("unprotect");
        assert_eq!(opened.as_slice(), secret.as_slice());
        // Another vault's entropy, none, or a changed byte: closed.
        assert!(unprotect(&sealed, b"claktory test v1\0vault-b").is_err());
        assert!(unprotect(&sealed, b"").is_err());
        let mut changed = sealed.clone();
        let last = changed.len() - 1;
        changed[last] ^= 1;
        assert!(unprotect(&changed, b"claktory test v1\0vault-a").is_err());
        assert!(unprotect(b"not a blob", b"claktory test v1\0vault-a").is_err());
        assert!(unprotect(&vec![0u8; MOST_SEALED + 1], b"claktory test v1\0vault-a").is_err());
    }

    #[test]
    fn a_key_file_opens_only_sealed_and_for_its_vault() {
        let hex = "00ff".repeat(16);
        let text = seal_auto_key(&hex, "v1").expect("sealed");
        assert!(text.starts_with(SEALED_KEY) && !text.contains(&hex));
        assert_eq!(open_auto_key(&text, "v1").as_deref().map(String::as_str), Some(hex.as_str()));
        assert!(open_auto_key(&text, "v2").is_none(), "another vault");
        assert!(open_auto_key(&hex, "v1").is_none(), "a plain key is refused");
        assert!(open_auto_key("dpapi1:zz", "v1").is_none());
    }

    /// The vault's own use of it: its key on disk is only ever
    /// sealed, and anything else leaves the vault locked and the file as it is.
    #[test]
    fn the_vaults_key_on_disk_is_sealed_and_fails_closed() {
        use crate::vault::Vault;
        let dir = std::env::temp_dir().join(format!("dpapi-vault-{}", crate::encoding::hex(&crate::vault::crypto::random::<8>())));
        let key = dir.join("auto.key");
        let read = || std::fs::read_to_string(&key).expect("the key on disk");
        let opens = || Vault::open(dir.clone()).is_open();
        let vault = Vault::open_or_make(dir.clone());
        assert!(vault.is_open());
        let vault_id: String = rusqlite::Connection::open(dir.join("vault.db"))
            .and_then(|db| db.query_row("SELECT vault_id FROM vault", [], |row| row.get(0)))
            .expect("vault id");
        let sealed = read();
        assert!(sealed.starts_with(SEALED_KEY), "{sealed}");
        assert!(factory_agent::windows::system::is_private_path(&key).expect("acl"));
        let hex = open_auto_key(&sealed, &vault_id).expect("it opens for this user");
        assert!(!sealed.contains(hex.as_str()), "the key shows on disk");
        drop(vault);
        assert!(opens(), "it opens again after a restart");

        // A new key (a move's rotation) is sealed too.
        assert!(Vault::open(dir.clone()).rotate_auto_key().expect("rotated"));
        let rotated = read();
        assert!(rotated.starts_with(SEALED_KEY) && rotated != sealed);
        let hex = open_auto_key(&rotated, &vault_id).expect("opens");

        // The same key unsealed, a changed seal, another vault's: locked, untouched.
        let changed = format!("{}{}", &rotated[..rotated.len() - 2], if rotated.ends_with('0') { "11" } else { "00" });
        let other = seal_auto_key(&hex, "another-vault").expect("sealed");
        for text in [hex.as_str(), changed.as_str(), other.as_str()] {
            std::fs::write(&key, text).expect("write");
            assert!(!opens(), "{text} opened it");
            assert_eq!(read(), text, "the file was changed");
        }
        std::fs::write(&key, &rotated).expect("back");
        assert!(opens());

        // Opened by hand (its recovery key) over a plain key, it
        // writes a fresh sealed one, and opens by itself from then on.
        let recovery = Vault::open(dir.clone()).new_recovery_key().expect("a recovery key");
        std::fs::write(&key, hex.as_str()).expect("plain");
        let vault = Vault::open(dir.clone());
        assert!(!vault.is_open());
        let (vault_id, protectors) = vault.protectors().expect("protectors");
        vault.unlock(Vault::check(&vault_id, &protectors, &recovery).expect("the recovery key opens it"));
        assert!(vault.mend_auto_key().expect("mended"));
        let mended = read();
        assert!(mended.starts_with(SEALED_KEY) && !mended.contains(hex.as_str()), "{mended}");
        assert!(!vault.mend_auto_key().expect("nothing to mend"), "a key that opens it is kept");
        drop(vault);
        assert!(opens(), "it opens by itself again");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
