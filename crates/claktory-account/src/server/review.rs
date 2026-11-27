//! The store reviewers' account (`--review-account`): one configured email
//! that signs in with a fixed 6-digit code instead of an emailed one, since
//! App Review and Play need sign-in details that work at any time, from
//! anywhere, without a one-time code. The email is a secret too (it's no
//! mailbox, and about 82 bits): the pair is the credential, so neither is
//! ever logged, only "the review account".
//!
//! Off without the option, or when its file isn't there (the operator's
//! `review-account off` removes it). The file, a systemd credential, is one
//! line: `<email> <salt> <sha256(salt || code), hex>`, the salt taken as the
//! text it is (`printf '%s%s' "$salt" "$code" | sha256sum`).
//!
//! Up to two such accounts, one per store (App Store and Google
//! Play), each its own file and email, rotated on its own: the option may
//! be given twice. Play may re-review at any time, so its pair mustn't
//! change when Apple's is rotated for a submission. A review email signs in
//! no Claktory but the review box (`--review-claktory-key`), and its
//! Claktories can't be removed on the web pages, so the details the stores
//! hold are no relay credential, and a reviewer trying Remove doesn't break
//! the next review. Delete account works, as Apple's review needs: the
//! account comes back at the next sign-in.

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::protocol::{hex, normalize_email};

/// Wrong codes the review account may get an hour, counted by its email
/// and by the address they come from, across sign-ins.
pub const WRONG_PER_HOUR: u32 = 5;

/// The most review accounts the service takes: one per store.
pub const MOST: usize = 2;

pub struct ReviewAccount {
    email: String,
    salt: String,
    digest: String,
}

impl std::fmt::Debug for ReviewAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReviewAccount(..)")
    }
}

impl ReviewAccount {
    /// From the email and the code itself, as the file's line would say.
    pub fn new(email: &str, salt: &str, code: &str) -> Result<ReviewAccount, String> {
        let digest = digest(salt, code);
        ReviewAccount::parse(&format!("{email} {salt} {digest}"))
    }

    /// The file's line.
    pub fn parse(line: &str) -> Result<ReviewAccount, String> {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [email, salt, digest] = fields.as_slice() else {
            return Err("the review account file is one line: <email> <salt> <sha256 of salt and code, hex>".to_owned());
        };
        let email = normalize_email(email).ok_or("the review account file's email isn't an email address")?;
        if salt.len() < 16 || !salt.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err("the review account file's salt is 16 or more letters and digits".to_owned());
        }
        if digest.len() != 64 || !digest.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("the review account file's hash is 64 hex characters".to_owned());
        }
        Ok(ReviewAccount { email, salt: (*salt).to_owned(), digest: digest.to_ascii_lowercase() })
    }

    /// The file at `path`: None when it isn't there (the account is off).
    /// One others may read is refused, as is one that doesn't parse.
    pub fn read(path: &Path) -> Result<Option<ReviewAccount>, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("can't read the review account file {}: {error}", path.display())),
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(path).map_err(|error| error.to_string())?.permissions().mode();
            if mode & 0o007 != 0 {
                return Err(format!("the review account file {} is readable by others: chmod o-rwx it", path.display()));
            }
        }
        ReviewAccount::parse(text.trim()).map(Some)
    }

    /// Checks a set of accounts the service would take: at most `MOST`,
    /// each its own email.
    pub fn check_all(accounts: &[ReviewAccount]) -> Result<(), String> {
        if accounts.len() > MOST {
            return Err(format!("at most {MOST} review accounts, one per store"));
        }
        for (index, account) in accounts.iter().enumerate() {
            if accounts[..index].iter().any(|other| other.email == account.email) {
                return Err("two review accounts have the same email".to_owned());
            }
        }
        Ok(())
    }

    /// Whether `email` (normalized) is this review account's.
    pub fn is(&self, email: &str) -> bool {
        self.email == email
    }

    /// What a sign-in request's code hash is made from: the code's digest,
    /// so the request is verified by the code without the service knowing it.
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// The digest of a code someone typed, to compare as `digest`.
    pub fn digest_of(&self, code: &str) -> String {
        digest(&self.salt, code)
    }
}

/// The review box's backend key in the file at `path`, its one line, as
/// a sign-in sends it (base64url of its 32 bytes): None when it isn't
/// there, so no Claktory signs in with a review email. A public key, so
/// others may read the file; one that isn't a key stops the service.
pub fn read_claktory_key(path: &Path) -> Result<Option<String>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("can't read the review box's key file {}: {error}", path.display())),
    };
    crate::protocol::verifying_key(text.trim())
        .map(|key| Some(crate::protocol::b64(key.to_bytes())))
        .ok_or_else(|| format!("the review box's key file {} isn't one backend key in base64url", path.display()))
}

fn digest(salt: &str, code: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(salt.as_bytes());
    hasher.update(code.as_bytes());
    hex(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_parses_and_its_code_matches() {
        // printf '%s%s' 0123456789abcdef 424242 | sha256sum
        let line = format!("Review-X@Example.com 0123456789abcdef {}", digest("0123456789abcdef", "424242"));
        let review = ReviewAccount::parse(&line).unwrap();
        assert!(review.is("review-x@example.com"));
        assert_eq!(review.digest_of("424242"), review.digest());
        assert_ne!(review.digest_of("424243"), review.digest());
        assert_eq!(format!("{review:?}"), "ReviewAccount(..)", "never the email");
        for bad in ["", "a@b.c salt", "a@b.c short 00", &format!("a@b.c 0123456789abcdef {}", "g".repeat(64))] {
            assert!(ReviewAccount::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_digest_is_sha256_of_the_salt_and_code_as_text() {
        // printf '%s%s' abcdefghijklmnop 123456 | shasum -a 256
        assert_eq!(digest("abcdefghijklmnop", "123456"), "89b60376530836972723c085cad9fa19b9168ff2289e06652b5a4a0b6ea3bfea");
    }
}
