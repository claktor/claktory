//! Keys the vault makes itself (#252): an agent asks for a key by type, the
//! person approves, and the keypair is made here, in factoryd's memory, the
//! private half sealed straight into a new secret. It's never written,
//! shown, logged or sent anywhere else; the asker is told only the public
//! half. OpenSSH's formats are RustCrypto's `ssh-key`, its randomness the
//! system's.

use ssh_key::getrandom::SysRng;
use ssh_key::rand_core::UnwrapErr;
use ssh_key::{Algorithm, LineEnding, PrivateKey};
use zeroize::Zeroizing;

use super::VaultError;

/// An ed25519 key for ssh: its private half an OpenSSH private key file.
pub(crate) const SSH_ED25519: &str = "ssh-ed25519";

/// The key types the vault makes.
pub(crate) const KINDS: &[&str] = &[SSH_ED25519];

/// The variable a generated key's file goes in when the agent names none.
pub(crate) const VARIABLE: &str = "SSH_KEY";

/// What a kind is called in sentences: "an ssh-ed25519 key".
pub(crate) fn called(kind: &str) -> String {
    format!("an {kind} key")
}

/// A keypair just made. Deliberately neither Debug nor Display: the
/// private half is only ever moved into the vault.
pub(crate) struct Made {
    /// The private key's file, as OpenSSH writes it, with its last newline.
    pub private: Zeroizing<String>,
    /// The public key's line, "ssh-ed25519 AAAA… <comment>".
    pub public: String,
}

/// Makes a keypair of `kind`, commented `comment` (the secret's name). Its
/// errors never carry key material.
pub(crate) fn make(kind: &str, comment: &str) -> Result<Made, VaultError> {
    let algorithm = match kind {
        SSH_ED25519 => Algorithm::Ed25519,
        _ => return Err(VaultError::Invalid(format!("The vault can't make that kind of key; it makes {}.", KINDS.join(", ")))),
    };
    let failed = || VaultError::Invalid("The vault couldn't make the key.".to_owned());
    let mut key = PrivateKey::random(&mut UnwrapErr(SysRng), algorithm).map_err(|_| failed())?;
    key.set_comment(comment);
    let mut private = key.to_openssh(LineEnding::LF).map_err(|_| failed())?;
    // ssh refuses a key file without its last newline.
    if !private.ends_with('\n') {
        private.push('\n');
    }
    let public = key.public_key().to_openssh().map_err(|_| failed())?;
    Ok(Made { private, public })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_made_key_reads_back_as_the_same_key() {
        let made = make(SSH_ED25519, "Deploy key").expect("made");
        // Split, so a secret scanner doesn't take the test for a key.
        let armor = |edge: &str| format!("-----{edge} OPENSSH {}-----\n", "PRIVATE KEY");
        assert!(made.private.starts_with(&armor("BEGIN")));
        assert!(made.private.ends_with(&armor("END")));
        assert!(made.public.starts_with("ssh-ed25519 AAAA") && made.public.ends_with(" Deploy key"), "{}", made.public);
        let read = PrivateKey::from_openssh(made.private.as_bytes()).expect("parses");
        assert_eq!(read.public_key().to_openssh().expect("public"), made.public);
        assert_eq!(read.comment().as_str_lossy(), "Deploy key");
        // Two keys are two keys.
        assert_ne!(make(SSH_ED25519, "Deploy key").expect("made").public, made.public);
        assert!(make("rsa", "x").is_err());
    }
}
