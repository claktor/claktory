//! The secrets file: what the service must keep and never put in its
//! database. `K_share` derives every Claktory's service share and the signing
//! key is what Claktories pin, so both are backed up offline: losing them
//! leaves every Claktory's vault openable only with its recovery key.

use std::io::Write as _;
use std::path::Path;

use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::protocol::{b64, random_32, unb64_32};

#[derive(Serialize, Deserialize)]
struct File {
    /// 32 bytes, base64url.
    k_share: String,
    /// The service's Ed25519 key: 32 bytes, base64url.
    signing_key: String,
    /// Resend's API key; empty sends no email through Resend.
    #[serde(default)]
    resend_api_key: String,
    /// A Cloudflare API token with Email Sending: Edit; empty sends no
    /// email through Cloudflare. Left out of a file without one, so an
    /// older service reads it still.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    cloudflare_api_token: String,
    /// What relays send to read the allowlist; empty turns it off.
    #[serde(default)]
    relay_bearer: String,
}

pub struct Secrets {
    pub k_share: Zeroizing<[u8; 32]>,
    pub signing_key: SigningKey,
    pub resend_api_key: Option<String>,
    pub cloudflare_api_token: Option<String>,
    pub relay_bearer: Option<String>,
}

impl Secrets {
    /// New keys, and a relay bearer; no Resend key.
    pub fn generate() -> Secrets {
        Secrets {
            k_share: random_32(),
            signing_key: SigningKey::from_bytes(&random_32()),
            resend_api_key: None,
            cloudflare_api_token: None,
            relay_bearer: Some(b64(*random_32())),
        }
    }

    pub fn load(path: &Path) -> Result<Secrets, String> {
        let text = Zeroizing::new(std::fs::read_to_string(path).map_err(|error| format!("can't read {}: {error}", path.display()))?);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            if let Ok(meta) = std::fs::metadata(path)
                && meta.permissions().mode() & 0o077 != 0
            {
                tracing::warn!("{} can be read by other users; chmod 600 it", path.display());
            }
        }
        let file: File = serde_json::from_str(&text).map_err(|error| format!("{} isn't a secrets file: {error}", path.display()))?;
        let file = Zeroizing::new(file);
        let k_share = Zeroizing::new(unb64_32(&file.k_share).ok_or("k_share isn't 32 bytes of base64url")?);
        let signing_key = Zeroizing::new(unb64_32(&file.signing_key).ok_or("signing_key isn't 32 bytes of base64url")?);
        let some = |text: &str| (!text.trim().is_empty()).then(|| text.trim().to_owned());
        Ok(Secrets {
            k_share,
            signing_key: SigningKey::from_bytes(&signing_key),
            resend_api_key: some(&file.resend_api_key),
            cloudflare_api_token: some(&file.cloudflare_api_token),
            relay_bearer: some(&file.relay_bearer),
        })
    }

    /// Writes new secrets to `path`, readable only by its owner. Never
    /// replaces a file: that would orphan every Claktory's vault share.
    pub fn create(path: &Path) -> Result<Secrets, String> {
        let secrets = Secrets::generate();
        let file = Zeroizing::new(File {
            k_share: b64(*secrets.k_share),
            signing_key: b64(secrets.signing_key.to_bytes()),
            resend_api_key: String::new(),
            cloudflare_api_token: String::new(),
            relay_bearer: secrets.relay_bearer.clone().unwrap_or_default(),
        });
        let text = Zeroizing::new(serde_json::to_string_pretty(&*file).map_err(|error| error.to_string())? + "\n");
        write_new(path, &text)?;
        Ok(secrets)
    }
}

impl Secrets {
    /// Puts a Resend API key in a secrets file, keeping the rest as it is.
    pub fn set_resend_key(path: &Path, key: &str) -> Result<(), String> {
        Secrets::set_key(path, key, |file| &mut file.resend_api_key)
    }

    /// Puts a Cloudflare API token (Email Sending: Edit) in a secrets file,
    /// keeping the rest as it is.
    pub fn set_cloudflare_token(path: &Path, token: &str) -> Result<(), String> {
        Secrets::set_key(path, token, |file| &mut file.cloudflare_api_token)
    }

    fn set_key(path: &Path, key: &str, field: impl FnOnce(&mut File) -> &mut String) -> Result<(), String> {
        if key.is_empty() || key.chars().any(|character| character.is_whitespace() || character.is_control()) {
            return Err("that isn't an API key".to_owned());
        }
        let (_, mut file) = Secrets::read_file(path)?;
        *field(&mut file) = key.to_owned();
        Secrets::replace(path, &file)
    }

    /// Replaces the keys `rotate` names with new ones, keeping everything
    /// else, after copying the file as it was to `<FILE>.before-<unix time>`
    /// (readable only by its owner), which it returns. A new `signing_key`
    /// makes every Claktory sign in again (it pins the key); their vaults
    /// open by themselves after. A new `k_share` changes every Claktory's
    /// service share: each vault opens only with its recovery key, once.
    pub fn rotate(path: &Path, rotate: Rotate, now: i64) -> Result<(Secrets, std::path::PathBuf), String> {
        if !rotate.signing_key && !rotate.k_share {
            return Err("say which key to rotate".to_owned());
        }
        let (text, mut file) = Secrets::read_file(path)?;
        let before = path.with_file_name(format!(
            "{}.before-{now}",
            path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
        ));
        write_new(&before, &text)?;
        if rotate.k_share {
            file.k_share = b64(*random_32());
        }
        if rotate.signing_key {
            file.signing_key = b64(*random_32());
        }
        Secrets::replace(path, &file)?;
        Ok((Secrets::load(path)?, before))
    }

    /// The file's text and contents, once it loads as secrets.
    fn read_file(path: &Path) -> Result<(Zeroizing<String>, Zeroizing<File>), String> {
        Secrets::load(path)?;
        let text = Zeroizing::new(std::fs::read_to_string(path).map_err(|error| format!("can't read {}: {error}", path.display()))?);
        let file: File = serde_json::from_str(&text).map_err(|error| format!("{} isn't a secrets file: {error}", path.display()))?;
        Ok((text, Zeroizing::new(file)))
    }

    /// Writes `file` beside `path`, then over it in one step.
    fn replace(path: &Path, file: &File) -> Result<(), String> {
        let text = Zeroizing::new(serde_json::to_string_pretty(file).map_err(|error| error.to_string())? + "\n");
        let partial = path.with_extension("partial");
        let _ = std::fs::remove_file(&partial);
        write_new(&partial, &text)?;
        std::fs::rename(&partial, path).map_err(|error| format!("can't replace {}: {error}", path.display()))
    }
}

/// Which keys `Secrets::rotate` replaces.
#[derive(Clone, Copy, Debug, Default)]
pub struct Rotate {
    pub signing_key: bool,
    pub k_share: bool,
}

/// Writes a new file only its owner can read.
fn write_new(path: &Path, text: &str) -> Result<(), String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut out = options.open(path).map_err(|error| format!("can't create {}: {error}", path.display()))?;
    out.write_all(text.as_bytes()).and_then(|()| out.sync_all()).map_err(|error| format!("can't write {}: {error}", path.display()))
}

impl zeroize::Zeroize for File {
    fn zeroize(&mut self) {
        self.k_share.zeroize();
        self.signing_key.zeroize();
        self.resend_api_key.zeroize();
        self.cloudflare_api_token.zeroize();
        self.relay_bearer.zeroize();
    }
}
