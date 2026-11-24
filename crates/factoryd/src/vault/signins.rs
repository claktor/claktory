//! Provider sign-ins kept in the vault (rivet-move.md §3, P2): the
//! credentials document (`factory_agent::credentials`) sealed with the
//! vault's key in `kept`, bound to the vault, instead of in plaintext in
//! `auth.json`. Never a secret: no agent can resolve or be granted it.
//!
//! - Beside it, `vault/sign-ins.json` (0600) is its outline: the accounts,
//!   their kinds, labels and states, with no keys or tokens, nor anything
//!   made from them. While the vault is locked that's what's known of them,
//!   so Settings → Providers and the models stay as they are, and requests
//!   wait for the vault to open. Its being there means the sign-ins moved
//!   into the vault; until then `auth.json` is used as before.
//! - `auth.json` is read always: sign-ins made while the vault is locked go
//!   there (a locked vault never keeps the person from signing in), and so
//!   do `factoryd login`'s with no Claktory running and an older Claktory's.
//!   When the vault is open they're taken in (`merge`: the file's newer).
//! - Rollback: an older backend reads only `auth.json`. So it keeps a full
//!   copy, written with every change, until every backend the supervisor
//!   could go back to reads the vault (`bin/factoryd.previous`'s hash is
//!   among those that ran with this, `sign-ins-backends`); then it's
//!   emptied, and stays so.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use factory_agent::Error;
use factory_agent::credentials::{CredentialBacking, Loaded, merge, outline, read_document, write_document};
use rusqlite::OptionalExtension;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::{Vault, VaultError, aad, crypto, private_file};
use crate::factory::Factory;
use crate::home::Home;

/// Its name in `kept`.
const KEPT: &str = "sign-ins";
/// In the vault's folder: the outline…
const OUTLINE: &str = "sign-ins.json";
/// …the hash of `auth.json` as last taken in or written…
const TAKEN: &str = "sign-ins-taken";
/// …and the hashes of the backends that ran with this, one a line.
const BACKENDS: &str = "sign-ins-backends";
/// How many backends' hashes are kept.
const KEEP_BACKENDS: usize = 20;

impl Vault {
    /// The sign-ins kept in the vault, as kept; None when there are none yet.
    pub(crate) fn sign_ins(&self) -> Result<Option<Zeroizing<Vec<u8>>>, VaultError> {
        let inner = self.lock();
        let vmk = Self::open_vmk(&inner)?;
        let sealed: Option<Vec<u8>> =
            Self::db(&inner)?.query_row("SELECT sealed FROM kept WHERE name = ?1", [KEPT], |row| row.get(0)).optional()?;
        let Some(sealed) = sealed else { return Ok(None) };
        crypto::open(vmk, &sealed, &aad(&["kept", &inner.vault_id, KEPT]))
            .map(Some)
            .ok_or_else(|| VaultError::Invalid("The sign-ins kept in the vault don't open.".to_owned()))
    }

    /// Whether it's in locked mode: it opens only with its passphrase or
    /// recovery key.
    pub(crate) fn locked_mode(&self) -> bool {
        self.lock().locked_mode
    }

    /// Keeps `value` as the sign-ins, sealed with the vault's key.
    pub(crate) fn keep_sign_ins(&self, value: &[u8]) -> Result<(), VaultError> {
        let inner = self.lock();
        let vmk = Self::open_vmk(&inner)?;
        Self::keep(Self::db(&inner)?, &inner.vault_id, vmk, KEPT, value)
    }
}

fn vault_error(error: VaultError) -> Error {
    Error::Other(error.to_string())
}

fn digest(bytes: &[u8]) -> String {
    crypto::hex(&Sha256::digest(bytes))
}

/// A file's hash; None when it can't be read.
fn file_digest(path: &Path) -> Option<String> {
    crate::encoding::sha256_file(path, 1 << 16).ok()
}

/// The sign-ins as kept; none when there are none yet.
fn parse(kept: Option<Zeroizing<Vec<u8>>>) -> Result<Map<String, Value>, Error> {
    match kept {
        Some(bytes) => Ok(serde_json::from_slice::<Value>(&bytes)?.as_object().cloned().unwrap_or_default()),
        None => Ok(Map::new()),
    }
}

/// Notes `binary`, this backend as an update keeps it to go back to
/// (`bin/factoryd.previous`), as one that reads the sign-ins from the vault:
/// the backend after it then needs no copy in `auth.json` for it.
pub(crate) fn note_backend(home: &Home, binary: &[u8]) {
    let path = home.root().join("vault").join(BACKENDS);
    let mut known: Vec<String> = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    let own = digest(binary);
    if known.contains(&own) {
        return;
    }
    known.push(own);
    let start = known.len().saturating_sub(KEEP_BACKENDS);
    if let Err(error) = private_file(&path, format!("{}\n", known[start..].join("\n")).as_bytes()) {
        tracing::warn!(%error, "could not note this backend as one that keeps sign-ins in the vault");
    }
}

/// The sign-ins' backing in a Claktory with a vault.
pub(crate) struct SignIns {
    factory: Weak<Factory>,
    auth: PathBuf,
    dir: PathBuf,
    previous: PathBuf,
    /// `auth.json` keeps a full copy, for a backend the supervisor may go
    /// back to; until `settle` finds none can, it does.
    mirror: AtomicBool,
}

impl SignIns {
    /// The backing of the sign-ins in `auth` (`auth.json`), in `home`'s vault.
    pub(crate) fn new(factory: Weak<Factory>, home: &Home, auth: PathBuf) -> SignIns {
        SignIns { factory, auth, dir: home.root().join("vault"), previous: home.previous_backend(), mirror: AtomicBool::new(true) }
    }

    /// Whether the sign-ins moved into the vault (their outline is there).
    fn moved(&self) -> bool {
        self.dir.join(OUTLINE).exists()
    }

    /// The Claktory, while its vault is there and open.
    fn open(&self) -> Option<Arc<Factory>> {
        self.factory.upgrade().filter(|factory| factory.vault.exists() && factory.vault.is_open())
    }

    fn sealed(factory: &Factory) -> Result<Map<String, Value>, Error> {
        parse(factory.vault.sign_ins().map_err(vault_error)?)
    }

    /// Seals `all` into the vault, then writes its outline.
    fn seal(&self, factory: &Factory, all: &Map<String, Value>) -> Result<(), Error> {
        let bytes = Zeroizing::new(serde_json::to_vec(&Value::Object(all.clone()))?);
        factory.vault.keep_sign_ins(&bytes).map_err(vault_error)?;
        write_document(&self.dir.join(OUTLINE), &outline(all))
    }

    /// `auth.json` as it is: its bytes (none when it isn't there) and what it holds.
    fn plain(&self) -> Result<(Vec<u8>, Map<String, Value>), Error> {
        let bytes = match std::fs::read(&self.auth) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error.into()),
        };
        Ok((bytes, read_document(&self.auth)?))
    }

    fn taken(&self) -> String {
        std::fs::read_to_string(self.dir.join(TAKEN)).map(|text| text.trim().to_owned()).unwrap_or_default()
    }

    /// Notes `auth.json` as it is now as taken in.
    fn note_taken(&self) -> Result<(), Error> {
        let (bytes, _) = self.plain()?;
        private_file(&self.dir.join(TAKEN), digest(&bytes).as_bytes())?;
        Ok(())
    }

    /// Whether `auth.json` can stop keeping a copy: no backend the
    /// supervisor could go back to needs it. Reads the previous backend:
    /// call it off the async threads.
    pub(crate) fn settle(&self) {
        let known = std::fs::read_to_string(self.dir.join(BACKENDS)).unwrap_or_default();
        let safe = !self.previous.exists()
            || file_digest(&self.previous).is_some_and(|previous| known.lines().any(|line| line.trim() == previous));
        if !safe {
            tracing::info!("auth.json keeps a copy of the sign-ins: the backend before this one reads only that");
        }
        self.mirror.store(!safe, Ordering::Relaxed);
    }
}

impl CredentialBacking for SignIns {
    fn load(&self) -> Result<Loaded, Error> {
        let Some(factory) = self.factory.upgrade().filter(|factory| factory.vault.exists() && self.moved()) else {
            return Ok(Loaded { all: read_document(&self.auth)?, locked: None });
        };
        match factory.vault.sign_ins() {
            Ok(kept) => Ok(Loaded { all: parse(kept)?, locked: None }),
            // What's known of them, and the sign-ins made meanwhile.
            Err(VaultError::Locked) => {
                Ok(Loaded { all: read_document(&self.auth)?, locked: Some(read_document(&self.dir.join(OUTLINE))?) })
            }
            Err(error) => Err(vault_error(error)),
        }
    }

    fn save(&self, all: &Map<String, Value>, locked: bool) -> Result<(), Error> {
        let open = self.open().filter(|_| self.moved());
        let Some(factory) = open.filter(|_| !locked) else {
            // Loaded from the vault, which can't be written now: never
            // into plaintext instead.
            if !locked && self.moved() && self.factory.upgrade().is_some_and(|factory| factory.vault.exists()) {
                return Err(factory_agent::credentials::locked_error(""));
            }
            // Sign-ins made while it's locked, or before they moved.
            return write_document(&self.auth, all);
        };
        let mut all = all.clone();
        let mirror = self.mirror.load(Ordering::Relaxed);
        if mirror {
            // About to be written over: anything new in it comes in first.
            let (bytes, plain) = self.plain()?;
            if !plain.is_empty() && digest(&bytes) != self.taken() {
                merge(&mut all, &plain)?;
            }
        }
        self.seal(&factory, &all)?;
        if mirror {
            write_document(&self.auth, &all)?;
            self.note_taken()?;
        }
        Ok(())
    }

    fn take_in(&self) -> Result<bool, Error> {
        let Some(factory) = self.open() else { return Ok(false) };
        let moved = self.moved();
        let mut all = if moved { Self::sealed(&factory)? } else { Map::new() };
        let (bytes, plain) = self.plain()?;
        let changed = !plain.is_empty() && digest(&bytes) != self.taken() && merge(&mut all, &plain)?;
        if !moved || changed {
            self.seal(&factory, &all)?;
            if !moved {
                tracing::info!(providers = all.len(), "moved the sign-ins into the vault");
            } else {
                tracing::info!("took in sign-ins from auth.json");
            }
        }
        if self.mirror.load(Ordering::Relaxed) {
            if plain != all {
                write_document(&self.auth, &all)?;
            }
        } else if !plain.is_empty() {
            write_document(&self.auth, &Map::new())?;
            tracing::info!("auth.json no longer keeps the sign-ins: they're in the vault");
        }
        if self.auth.exists() {
            self.note_taken()?;
        }
        Ok(!moved || changed)
    }
}

/// How often the monitor looks whether the Claktory is still there, when
/// nothing tells it the vault changed.
const LOOK_AGAIN: std::time::Duration = std::time::Duration::from_secs(60);

/// Takes the sign-ins up as the vault opens: moved into it the first time,
/// those made meanwhile taken in, and requests waiting for them sent again.
/// First notes whether `auth.json` still keeps a copy (`SignIns::settle`).
pub(crate) async fn monitor(factory: Weak<Factory>, sign_ins: Arc<SignIns>) {
    if let Err(error) = tokio::task::spawn_blocking(move || sign_ins.settle()).await {
        tracing::warn!(%error, "could not tell which backends read the sign-ins from the vault");
    }
    let mut was_open = false;
    loop {
        let Some(strong) = factory.upgrade() else { return };
        let open = strong.vault.exists() && strong.vault.is_open();
        if open && !was_open {
            match strong.models.take_in_sign_ins().await {
                Ok(true) => strong.refresh_models().await,
                Ok(false) => {}
                Err(error) => tracing::error!(%error, "could not move the sign-ins into the vault"),
            }
            // Requests waiting for the sign-ins go again, and their usage
            // is read (`usage::check_usage` skips it while they're locked).
            strong.capacity_changed();
            strong.usage_wake.notify_one();
            strong.broadcast_workspace(&strong.lock());
        }
        was_open = open;
        let changed = strong.vault_moved.clone();
        drop(strong);
        let _ = tokio::time::timeout(LOOK_AGAIN, changed.notified()).await;
    }
}

/// Why the vault is locked, for VaultState.locked_why: the Claktory
/// account's `problem` when it has one.
pub(crate) fn locked_why(vault: &Vault, problem: String) -> String {
    if !problem.is_empty() {
        problem
    } else if vault.locked_mode() {
        "Locked mode is on: unlock them with your passphrase or recovery key.".to_owned()
    } else if vault.account_params().is_some() {
        "Unlocking with claktory.com… If that doesn't work, unlock them with your recovery key.".to_owned()
    } else {
        "Unlock them with your recovery key.".to_owned()
    }
}
