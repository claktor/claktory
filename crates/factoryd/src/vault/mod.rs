//! The vault (#107): secrets agents use without seeing them, kept in
//! `<home>/vault/vault.db`, apart from factory.db, which admin agents read.
//!
//! Each secret's values are sealed with its own key (DEK), the DEKs with the
//! vault's key (VMK), and the VMK once per protector: the passphrase
//! (Argon2id), the recovery key, and unless the vault is in locked mode a
//! key kept on disk (`auto.key`, 0600) so the vault opens by itself when the
//! backend starts. In locked mode it opens only when the person unlocks it.
//!
//! Grants carry a MAC made with a key derived from the VMK, so editing
//! vault.db grants nothing. Names, descriptions and variable names aren't
//! secret; values are only ever decrypted to be revealed to the person or
//! given to a command.

mod account;
pub(crate) mod asks;
pub mod backup;
pub(crate) mod connections;
pub(crate) mod crypto;
pub(crate) mod dm;
pub(crate) mod env;
pub(crate) mod generate;
pub(crate) mod given_up;
pub mod handoff;
pub(crate) mod held_codes;
pub(crate) mod import;
pub mod memory;
pub(crate) mod recovery;
pub(crate) mod requests;
pub(crate) mod signins;
pub(crate) mod sites;

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use factory_protocol::v1;
use rusqlite::{Connection, OptionalExtension, params};
use zeroize::Zeroizing;

use crate::store::query;
use crate::transcript::now_ms;
use crypto::Key;

const SCHEMA: &str = "
-- The vault: one row.
CREATE TABLE vault (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    -- Random; part of what every sealed value is bound to.
    vault_id TEXT NOT NULL,
    created INTEGER NOT NULL,
    -- No key on disk: it opens only with the passphrase or recovery key.
    locked_mode INTEGER NOT NULL
);
-- The ways to open it, each sealing the vault's key: passphrase, recovery, auto.
CREATE TABLE protectors (
    kind TEXT PRIMARY KEY,
    -- How the key is made from what's given: 'argon2id m=… t=… p=…' for the
    -- passphrase, 'hkdf' for the recovery key, 'file' for auto.key.
    params TEXT NOT NULL,
    salt BLOB NOT NULL,
    sealed_vmk BLOB NOT NULL,
    created INTEGER NOT NULL
);
CREATE TABLE secrets (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE,
    description TEXT NOT NULL,
    version INTEGER NOT NULL,
    -- Its key, sealed with the vault's.
    sealed_dek BLOB NOT NULL,
    -- The agent that saved it; '' for the person.
    owner TEXT NOT NULL,
    created INTEGER NOT NULL,
    updated INTEGER NOT NULL,
    last_used_by TEXT NOT NULL DEFAULT '',
    last_used INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE variables (
    secret TEXT NOT NULL,
    position INTEGER NOT NULL,
    name TEXT NOT NULL,
    -- The value goes in a file whose path the variable holds.
    file INTEGER NOT NULL,
    -- The value's last 4 characters, or '' when it's under 12.
    hint TEXT NOT NULL,
    -- Under 8 bytes: too short to hide in agents' output.
    short INTEGER NOT NULL,
    -- Sealed with the secret's key.
    sealed_value BLOB NOT NULL,
    PRIMARY KEY (secret, name)
);
CREATE TABLE grants (
    secret TEXT NOT NULL,
    agent TEXT NOT NULL,
    granted INTEGER NOT NULL,
    -- HMAC of the grant with a key from the vault's; a row without the
    -- right one is ignored.
    mac BLOB NOT NULL,
    PRIMARY KEY (secret, agent)
);
-- What happened to secrets, never changed afterwards.
CREATE TABLE uses (
    id INTEGER PRIMARY KEY,
    secret TEXT NOT NULL,
    secret_name TEXT NOT NULL,
    time INTEGER NOT NULL,
    -- use, reveal, grant, revoke
    kind TEXT NOT NULL,
    agent TEXT NOT NULL,
    task TEXT NOT NULL,
    -- The command's first line (use), or the device (reveal).
    detail TEXT NOT NULL
);
CREATE INDEX uses_by_secret ON uses (secret, time);
-- Agents' requests (secrets_request).
CREATE TABLE requests (
    id TEXT PRIMARY KEY,
    agent TEXT NOT NULL,
    secret TEXT NOT NULL,
    secret_name TEXT NOT NULL,
    -- A new secret asked for, as JSON {name, description, variables}; '' for an existing one.
    new_secret TEXT NOT NULL,
    reason TEXT NOT NULL,
    -- pending, granted, dismissed
    state TEXT NOT NULL,
    created INTEGER NOT NULL,
    decided INTEGER NOT NULL DEFAULT 0,
    comment TEXT NOT NULL DEFAULT ''
);
";

/// Changes to the schema after the first, in order.
const MIGRATIONS: &[&str] = &[
    "
-- What the vault keeps for the person, sealed with the vault's key: the
-- recovery key ('recovery'), to show it again.
CREATE TABLE kept (
    name TEXT PRIMARY KEY,
    sealed BLOB NOT NULL
);
",
    "
-- The secret given for a request, which the person picks: its id and name.
ALTER TABLE requests ADD COLUMN given TEXT NOT NULL DEFAULT '';
ALTER TABLE requests ADD COLUMN given_name TEXT NOT NULL DEFAULT '';
",
    "
-- Connected services (docs/CONNECTORS.md): a secret's kind ('' the
-- person's, 'connection', 'export'), its connector, an export's connection,
-- and what it reaches as JSON (never a token).
ALTER TABLE secrets ADD COLUMN kind TEXT NOT NULL DEFAULT '';
ALTER TABLE secrets ADD COLUMN connector TEXT NOT NULL DEFAULT '';
ALTER TABLE secrets ADD COLUMN parent TEXT NOT NULL DEFAULT '';
ALTER TABLE secrets ADD COLUMN meta TEXT NOT NULL DEFAULT '';
-- A connection's tokens, the connector's JSON, sealed with its secret's key
-- and bound to the vault, secret, connector and generation; never a
-- variable, so never given to a command.
CREATE TABLE controls (
    secret TEXT PRIMARY KEY,
    version INTEGER NOT NULL,
    sealed BLOB NOT NULL
);
-- Agents' proposals to connect a service (secrets_propose_connection).
CREATE TABLE connect_proposals (
    id TEXT PRIMARY KEY,
    agent TEXT NOT NULL,
    connector TEXT NOT NULL,
    access_id TEXT NOT NULL,
    purpose TEXT NOT NULL,
    access TEXT NOT NULL,
    reason TEXT NOT NULL,
    -- pending, connected, dismissed
    state TEXT NOT NULL,
    created INTEGER NOT NULL,
    decided INTEGER NOT NULL DEFAULT 0,
    comment TEXT NOT NULL DEFAULT '',
    secret TEXT NOT NULL DEFAULT '',
    secret_name TEXT NOT NULL DEFAULT ''
);
",
    "
-- A proposal's exact permissions, where the service takes them (AWS): its
-- Allow statements as JSON [{actions, resources, condition}], and its region.
ALTER TABLE connect_proposals ADD COLUMN statements TEXT NOT NULL DEFAULT '';
ALTER TABLE connect_proposals ADD COLUMN region TEXT NOT NULL DEFAULT '';
",
    "
-- Other names a grant's agent gets the secret's values under, as the
-- request it answered asked: JSON {name: the secret's variable}; '' for none.
ALTER TABLE grants ADD COLUMN aliases TEXT NOT NULL DEFAULT '';
-- When the agent last changed its request; 0 if never.
ALTER TABLE requests ADD COLUMN updated INTEGER NOT NULL DEFAULT 0;
",
    "
-- The person approves the need and an admin agent does the vault work
-- (state 'approved'): when they approved, which admin agent it passed to
-- (picked when the request is made), and whether that agent's add-secret
-- card is open (secrets_collect).
ALTER TABLE requests ADD COLUMN approved INTEGER NOT NULL DEFAULT 0;
ALTER TABLE requests ADD COLUMN admin TEXT NOT NULL DEFAULT '';
ALTER TABLE requests ADD COLUMN collecting INTEGER NOT NULL DEFAULT 0;
",
    "
-- Each command that uses it waits for the person's Allow, for that exact
-- command, once (`asks_each_time`). Part of what its key is sealed with
-- (`dek_aad`), so changing it here alone makes the secret unusable rather
-- than free. Secrets from before were sealed without it: 0, as they are.
ALTER TABLE secrets ADD COLUMN ask INTEGER NOT NULL DEFAULT 0;
",
    "
-- A file's bytes that aren't text (an imported .p12), sealed as standard
-- base64; a command's file gets the bytes themselves.
ALTER TABLE variables ADD COLUMN binary INTEGER NOT NULL DEFAULT 0;
",
    "
-- #370 Submit: the person answered a request or a proposal to connect with
-- a comment instead (state 'dismissed': nothing was given or connected).
ALTER TABLE requests ADD COLUMN replied INTEGER NOT NULL DEFAULT 0;
ALTER TABLE connect_proposals ADD COLUMN replied INTEGER NOT NULL DEFAULT 0;
",
    "
-- #435 W6, agent-browser.md §5.3: the sites a secret may be filled into in
-- Claktory's browser, as a JSON list of exact https origins ('' for none),
-- and their MAC with the grants' key (`sites::message`), so editing
-- vault.db can't bind a secret to another site.
ALTER TABLE secrets ADD COLUMN sites TEXT NOT NULL DEFAULT '';
ALTER TABLE secrets ADD COLUMN sites_mac BLOB NOT NULL DEFAULT x'';
-- A grant that only lets its agent fill the secret into those sites, never
-- give it to a command; part of the grant's MAC when set.
ALTER TABLE grants ADD COLUMN fill_only INTEGER NOT NULL DEFAULT 0;
-- The account Claktory's browser signed in with at each identity provider
-- (Google, Apple, Microsoft): one each (Fugo 40552), until the person signs
-- it out or deletes the browser's data.
CREATE TABLE browser_accounts (
    provider TEXT PRIMARY KEY,
    secret TEXT NOT NULL,
    since INTEGER NOT NULL
);
",
];

/// How long a command's approval stays armed between the gate that passed
/// it and its resolving the secrets (`Vault::arm`).
const ARMED_MS: i64 = 60_000;

/// A value is too short to hide in output below this many bytes.
const SHORT: usize = 8;
/// A value shows its last 4 characters from this many on.
const HINTED: usize = 12;
/// The largest value: a credentials file at most.
const MAX_VALUE: usize = 1024 * 1024;
/// Commands that got a secret this recently may still be running:
/// background tasks stop after 2 hours at most.
const RECENT_MS: i64 = 24 * 3600 * 1000;

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("There's no vault: Claktory couldn't make it (its log says why).")]
    NoVault,
    #[error("There's a vault already.")]
    Exists,
    #[error("The vault is locked: it opens with its passphrase or recovery key.")]
    Locked,
    /// What was given doesn't open the vault, in words for the person.
    #[error("{0}")]
    WrongProof(&'static str),
    #[error("This needs the vault passphrase.")]
    ProofRequired,
    #[error("There's no secret \"{0}\".")]
    NoSecret(String),
    #[error("There's no request \"{0}\" from that agent.")]
    NoRequest(String),
    #[error("{secret} isn't granted to {agent}.")]
    NotGranted { secret: String, agent: String },
    #[error("{0}")]
    Invalid(String),
    /// A recovery key check's wrong group, which never says what was typed.
    #[error("That isn't block {0} of your recovery key. Check what you saved and try again.")]
    WrongGroup(u32),
    /// Too many wrong groups lately; says how long to wait.
    #[error("{0}")]
    TooManyTries(String),
    #[error("could not read or write the vault: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("could not read or write the vault: {0}")]
    Io(#[from] std::io::Error),
}

/// A variable's value, as given or decrypted; wiped when dropped, and never
/// shown by Debug.
pub(crate) struct Value {
    pub variable: String,
    pub value: Zeroizing<String>,
    pub file: bool,
    /// A file's bytes that aren't text, as standard base64 in `value`.
    pub binary: bool,
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Value {{ variable: {:?}, file: {}, value: [hidden] }}", self.variable, self.file)
    }
}

impl Value {
    pub(crate) fn from_entry(entry: &v1::SecretEntry) -> Value {
        Value { variable: entry.variable.trim().to_owned(), value: Zeroizing::new(entry.value.clone()), file: entry.file, binary: false }
    }
}

/// Why a command can't have the secrets it asked for.
enum Refusal {
    Said(String),
    /// Not a secret granted to the agent (or not one at all).
    NotYours(String),
}

impl From<VaultError> for Refusal {
    fn from(error: VaultError) -> Refusal {
        Refusal::Said(error.to_string())
    }
}

impl From<rusqlite::Error> for Refusal {
    fn from(error: rusqlite::Error) -> Refusal {
        Refusal::Said(VaultError::from(error).to_string())
    }
}

/// A protector as stored: enough to try a passphrase or recovery key on it
/// without holding the vault's lock while Argon2 runs.
pub(crate) struct Protector {
    kind: String,
    params: String,
    salt: Vec<u8>,
    sealed_vmk: Vec<u8>,
}

pub(crate) struct Vault {
    dir: PathBuf,
    inner: Mutex<Inner>,
}

/// The tasks that got each secret, by (secret, agent), within RECENT_MS.
type Recent = HashMap<(String, String), Vec<String>>;

#[derive(Default)]
struct Inner {
    /// None until there's a vault.
    db: Option<Connection>,
    vault_id: String,
    locked_mode: bool,
    /// The vault's key while it's open, in a page of its own.
    vmk: Option<memory::Locked>,
    /// What clients were last sent, until something changes…
    state: Option<v1::VaultState>,
    /// …and the background tasks that got each secret lately, by (secret,
    /// agent), to count those still running.
    recent: Recent,
    /// The cost of new passphrase keys (cheaper in tests).
    passphrase_params: Option<&'static str>,
    /// Commands the person allowed that use ask-each-time secrets, between
    /// the gate and resolving them: (agent, secret names, until).
    armed: Vec<(String, Vec<String>, i64)>,
    /// Checks that the person saved the recovery key, running now.
    checks: recovery::Checks,
}

fn aad(parts: &[&str]) -> Vec<u8> {
    parts.join("\0").into_bytes()
}

/// What a secret's key is sealed with: with "ask" when it asks each time,
/// so the flag can't change without the vault's key.
pub(crate) fn dek_aad(vault_id: &str, id: &str, version: &str, ask: bool) -> Vec<u8> {
    let mut parts = vec!["dek", vault_id, id, version];
    if ask {
        parts.push("ask");
    }
    aad(&parts)
}

/// A secret's key, sealed as `dek_aad` says. One whose flag says it asks
/// each time but whose key isn't sealed with "ask" was changed by a
/// Claktory from before Ask each time (one an update went back to, where the
/// person changed its values): it opens all the same and still asks (the
/// flag decides that), and the bool says to seal it with "ask" again. The
/// other way round, a flag turned off outside Claktory, it doesn't open.
fn open_dek(vmk: &[u8; 32], sealed: &[u8], vault_id: &str, id: &str, version: &str, ask: bool) -> Option<(Key, bool)> {
    if let Some(dek) = crypto::open_key(vmk, sealed, &dek_aad(vault_id, id, version, ask)) {
        return Some((dek, false));
    }
    if !ask {
        return None;
    }
    crypto::open_key(vmk, sealed, &dek_aad(vault_id, id, version, false)).map(|dek| (dek, true))
}

/// What a grant's MAC covers; its aliases (JSON) only when it has any, and
/// "fill" only when it's fill-only, so grants from before them still check
/// (and an older Claktory, which doesn't know fill-only grants, ignores
/// one rather than giving it to commands).
fn grant_message(vault_id: &str, secret: &str, agent: &str, granted: i64, aliases: &str, fill_only: bool) -> Vec<u8> {
    let granted = granted.to_string();
    let mut parts = vec!["grant", vault_id, secret, agent, &granted];
    if !aliases.is_empty() {
        parts.push(aliases);
    }
    if fill_only {
        parts.push("fill");
    }
    aad(&parts)
}

/// A grant whose MAC checks.
struct Grant {
    aliases: BTreeMap<String, String>,
    /// Only to fill into the secret's sites in Claktory's browser
    /// (`sites`), never to a command or an app.
    fill_only: bool,
}

/// A grant's aliases as kept: '' for none.
fn aliases_text(aliases: &BTreeMap<String, String>) -> String {
    if aliases.is_empty() { String::new() } else { serde_json::to_string(aliases).unwrap_or_default() }
}

fn aliases_of(text: &str) -> BTreeMap<String, String> {
    serde_json::from_str(text).unwrap_or_default()
}

/// A secret's name: what agents pass to bash's `secrets`.
fn check_name(name: &str) -> Result<String, VaultError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(VaultError::Invalid("A secret needs a name.".to_owned()));
    }
    if name.chars().count() > 64 || !name.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.' | '@' | '(' | ')')) {
        return Err(VaultError::Invalid(format!(
            "\"{name}\" can't be a secret's name. Use letters, digits, spaces and - _ . @ ( ), up to 64 characters."
        )));
    }
    Ok(name.to_owned())
}

/// An environment variable's name.
pub(crate) fn valid_variable(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn check_values(values: &[Value]) -> Result<(), VaultError> {
    if values.is_empty() {
        return Err(VaultError::Invalid("A secret needs at least one variable.".to_owned()));
    }
    let mut seen = std::collections::HashSet::new();
    for value in values {
        if !valid_variable(&value.variable) {
            return Err(VaultError::Invalid(format!(
                "\"{}\" can't be a variable's name: use letters, digits and '_', not starting with a digit.",
                value.variable
            )));
        }
        if !seen.insert(value.variable.as_str()) {
            return Err(VaultError::Invalid(format!("{} is there twice.", value.variable)));
        }
        // A binary file's base64 is a third longer than the file: its size
        // is what the base64 decodes to, padding left out.
        let size = if value.binary {
            let padding = value.value.bytes().rev().take_while(|byte| *byte == b'=').count().min(2);
            (value.value.len() / 4 * 3).saturating_sub(padding)
        } else {
            value.value.len()
        };
        if size > MAX_VALUE {
            return Err(VaultError::Invalid(format!("{}'s value is over 1 MB.", value.variable)));
        }
    }
    Ok(())
}

/// The last 4 characters, to tell secrets apart in Settings → Secrets. A
/// key in armor would only show its armor ("----"): an SSH private key
/// shows the end of its fingerprint instead (as `ssh-keygen -l` shows it,
/// so the same for the key and its public half), any other the end of
/// what's inside the armor.
fn hint(value: &str) -> String {
    let value = value.trim_end();
    if value.ends_with("-----") {
        if let Ok(key) = ssh_key::PrivateKey::from_openssh(value) {
            return last_four(&key.public_key().fingerprint(ssh_key::HashAlg::Sha256).to_string());
        }
        let inside: Zeroizing<String> = Zeroizing::new(value.lines().filter(|line| !line.starts_with("-----")).map(str::trim).collect());
        return if inside.chars().count() < HINTED { String::new() } else { last_four(&inside) };
    }
    if value.chars().count() < HINTED { String::new() } else { last_four(value) }
}

fn last_four(text: &str) -> String {
    let count = text.chars().count();
    text.chars().skip(count.saturating_sub(4)).collect()
}

/// Creates a file only its owner can read and write.
fn private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// The schema version this backend writes: a vault above it was made by a
/// newer one.
pub(crate) fn schema_version() -> i64 {
    MIGRATIONS.len() as i64 + 1
}

/// What `kept` names the note that the vault's free pages were wiped once.
const WIPED: &str = "free-pages-wiped";

fn open_db(path: &Path) -> Result<Connection, VaultError> {
    let db = Connection::open(path)?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    // What's deleted (a secret, recovery codes) is overwritten, never left
    // sealed in a free page (Fugo, LOW).
    db.pragma_update(None, "secure_delete", true)?;
    crate::store::migrate(&db, SCHEMA, MIGRATIONS)?;
    Ok(db)
}

/// Once per vault: rewrites the file, so pages freed before secure_delete
/// was on hold nothing of what was deleted. Logged when it can't.
fn wipe_free_pages(db: &Connection) {
    let wiped: bool = db.query_row("SELECT COUNT(*) > 0 FROM kept WHERE name = ?1", [WIPED], |row| row.get(0)).unwrap_or(true);
    if wiped {
        return;
    }
    let done = db.execute_batch("VACUUM").and_then(|()| db.execute("INSERT OR REPLACE INTO kept (name, sealed) VALUES (?1, x'')", [WIPED]));
    if let Err(error) = done {
        tracing::warn!(%error, "the vault's free pages weren't wiped; trying again at the next start");
    }
}

impl Vault {
    /// Opens the vault in `dir`, if there is one, and unless it's in locked
    /// mode, opens it with the key kept on disk. Problems are logged: the
    /// vault then stays locked, and Claktory runs on without it.
    pub(crate) fn open(dir: PathBuf) -> Vault {
        let vault = Vault { dir, inner: Mutex::new(Inner::default()) };
        if let Err(error) = vault.load() {
            tracing::error!(%error, "could not open the vault; it stays locked");
        }
        vault
    }

    /// Opens the vault in `dir`, making it first if there's none: Claktory
    /// has one from the start, opened by the key on disk, with nothing to
    /// set up. One that's there but won't open is left as it is.
    pub(crate) fn open_or_make(dir: PathBuf) -> Vault {
        let vault = Vault::open(dir);
        if !vault.db_path().exists()
            && let Err(error) = vault.create(None, false)
        {
            tracing::error!(%error, "could not make the vault");
        }
        vault
    }

    fn load(&self) -> Result<(), VaultError> {
        let path = self.db_path();
        if !path.exists() {
            return Ok(());
        }
        let db = open_db(&path)?;
        wipe_free_pages(&db);
        let (vault_id, locked_mode): (String, bool) =
            db.query_row("SELECT vault_id, locked_mode FROM vault WHERE id = 1", [], crate::store::row2)?;
        let mut inner = self.lock();
        inner.vault_id = vault_id;
        inner.locked_mode = locked_mode;
        inner.db = Some(db);
        if !locked_mode {
            let vmk = self.auto_vmk(&inner);
            if vmk.is_none() && account::protector(&inner).is_none() {
                tracing::warn!("the vault's key on disk is missing or doesn't open it; it stays locked until unlocked");
            }
            if let Some(vmk) = &vmk {
                Self::set_open(&mut inner, vmk);
            }
        }
        Ok(())
    }

    fn auto_vmk(&self, inner: &Inner) -> Option<Key> {
        Self::vmk_with_key_file(inner, &self.auto_path())
    }

    /// The vault's key, if the key on disk at `path` opens it.
    fn vmk_with_key_file(inner: &Inner, path: &Path) -> Option<Key> {
        let text = Zeroizing::new(std::fs::read_to_string(path).ok()?);
        let bytes = Zeroizing::new(crypto::unhex(&text)?);
        let kek: [u8; 32] = bytes.as_slice().try_into().ok()?;
        let sealed: Vec<u8> = inner
            .db
            .as_ref()?
            .query_row("SELECT sealed_vmk FROM protectors WHERE kind = 'auto'", [], |row| row.get(0))
            .optional()
            .ok()??;
        crypto::open_key(&kek, &sealed, &aad(&["vmk", &inner.vault_id, "auto"]))
    }

    fn db_path(&self) -> PathBuf {
        self.dir.join("vault.db")
    }

    fn auto_path(&self) -> PathBuf {
        self.dir.join("auto.key")
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Cheaper passphrase keys, for tests.
    #[doc(hidden)]
    pub fn set_passphrase_params(&self, params: &'static str) {
        self.lock().passphrase_params = Some(params);
    }

    pub(crate) fn exists(&self) -> bool {
        self.lock().db.is_some()
    }

    pub(crate) fn is_open(&self) -> bool {
        self.lock().vmk.is_some()
    }

    fn passphrase_protector(inner: &Inner, vault_id: &str, vmk: &[u8; 32], passphrase: &str) -> Result<Protector, VaultError> {
        if passphrase.chars().count() < 8 {
            return Err(VaultError::Invalid("The passphrase needs 8 characters at least.".to_owned()));
        }
        let params = inner.passphrase_params.unwrap_or(crypto::PASSPHRASE_PARAMS).to_owned();
        let salt = crypto::random::<16>().to_vec();
        let kek =
            crypto::passphrase_key(passphrase, &salt, &params).ok_or_else(|| VaultError::Invalid("bad passphrase cost".to_owned()))?;
        let sealed_vmk = crypto::seal(&kek, vmk.as_slice(), &aad(&["vmk", vault_id, "passphrase"]));
        Ok(Protector { kind: "passphrase".to_owned(), params, salt, sealed_vmk })
    }

    fn recovery_protector(vault_id: &str, vmk: &[u8; 32]) -> (String, Protector) {
        let (text, bytes) = crypto::new_recovery();
        let kek = crypto::derive(bytes.as_slice(), vault_id.as_bytes(), "claktory vault recovery v1");
        let sealed_vmk = crypto::seal(&kek, vmk.as_slice(), &aad(&["vmk", vault_id, "recovery"]));
        (text, Protector { kind: "recovery".to_owned(), params: "hkdf".to_owned(), salt: Vec::new(), sealed_vmk })
    }

    /// A new key on disk, and its protector.
    fn auto_protector(&self, vault_id: &str, vmk: &[u8; 32]) -> Result<Protector, VaultError> {
        let kek = crypto::new_key();
        private_file(&self.auto_path(), crypto::hex(kek.as_slice()).as_bytes())?;
        let sealed_vmk = crypto::seal(&kek, vmk.as_slice(), &aad(&["vmk", vault_id, "auto"]));
        Ok(Protector { kind: "auto".to_owned(), params: "file".to_owned(), salt: Vec::new(), sealed_vmk })
    }

    fn save_protector(db: &Connection, protector: &Protector) -> Result<(), VaultError> {
        db.execute(
            "INSERT OR REPLACE INTO protectors (kind, params, salt, sealed_vmk, created) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![protector.kind, protector.params, protector.salt, protector.sealed_vmk, now_ms()],
        )?;
        Ok(())
    }

    /// Makes the vault, open. Claktory makes it by itself, with only the key
    /// on disk (`auto.key`); with a `passphrase` (tests, and keys the person
    /// holds later) it also gets that and a recovery key, which it returns,
    /// and may be in locked mode.
    pub(crate) fn create(&self, passphrase: Option<&str>, locked_mode: bool) -> Result<Option<String>, VaultError> {
        let mut inner = self.lock();
        if inner.db.is_some() {
            return Err(VaultError::Exists);
        }
        if locked_mode && passphrase.is_none() {
            return Err(VaultError::Invalid("A vault in locked mode needs a passphrase.".to_owned()));
        }
        let vault_id = crypto::hex(&crypto::random::<16>());
        let vmk = crypto::new_key();
        let passphrase = passphrase.map(|passphrase| Self::passphrase_protector(&inner, &vault_id, &vmk, passphrase)).transpose()?;
        let (recovery_text, recovery) = match passphrase {
            Some(_) => {
                let (text, protector) = Self::recovery_protector(&vault_id, &vmk);
                (Some(text), Some(protector))
            }
            None => (None, None),
        };
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&self.dir)?;
        }
        let auto = if locked_mode { None } else { Some(self.auto_protector(&vault_id, &vmk)?) };
        // Made aside and moved into place, so a vault is never half made.
        let draft = self.dir.join("vault.db.new");
        let _ = std::fs::remove_file(&draft);
        private_file(&draft, b"")?;
        {
            let mut db = open_db(&draft)?;
            let transaction = db.transaction()?;
            transaction.execute(
                "INSERT INTO vault (id, vault_id, created, locked_mode) VALUES (1, ?1, ?2, ?3)",
                params![vault_id, now_ms(), locked_mode],
            )?;
            for protector in [passphrase.as_ref(), recovery.as_ref(), auto.as_ref()].into_iter().flatten() {
                Self::save_protector(&transaction, protector)?;
            }
            transaction.commit()?;
        }
        std::fs::rename(&draft, self.db_path())?;
        inner.db = Some(open_db(&self.db_path())?);
        inner.vault_id = vault_id;
        inner.locked_mode = locked_mode;
        Self::set_open(&mut inner, &vmk);
        Ok(recovery_text)
    }

    /// The protectors to try a passphrase or recovery key on.
    pub(crate) fn protectors(&self) -> Result<(String, Vec<Protector>), VaultError> {
        let inner = self.lock();
        let Some(db) = &inner.db else { return Err(VaultError::NoVault) };
        let mut statement = db.prepare("SELECT kind, params, salt, sealed_vmk FROM protectors WHERE kind IN ('passphrase', 'recovery')")?;
        let protectors = statement
            .query_map([], |row| Ok(Protector { kind: row.get(0)?, params: row.get(1)?, salt: row.get(2)?, sealed_vmk: row.get(3)? }))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok((inner.vault_id.clone(), protectors))
    }

    /// The vault's key, if `given` is its passphrase or recovery key. Slow
    /// (Argon2): call it off the async threads, without the vault's lock.
    pub(crate) fn check(vault_id: &str, protectors: &[Protector], given: &str) -> Option<Key> {
        for protector in protectors {
            let kek = match protector.kind.as_str() {
                "recovery" => {
                    let Some(bytes) = crypto::parse_recovery(given) else { continue };
                    crypto::derive(bytes.as_slice(), vault_id.as_bytes(), "claktory vault recovery v1")
                }
                "passphrase" if !given.is_empty() => {
                    let Some(key) = crypto::passphrase_key(given, &protector.salt, &protector.params) else { continue };
                    key
                }
                _ => continue,
            };
            if let Some(vmk) = crypto::open_key(&kek, &protector.sealed_vmk, &aad(&["vmk", vault_id, &protector.kind])) {
                return Some(vmk);
            }
        }
        None
    }

    /// Opens the vault with its key, found with `check`.
    pub(crate) fn unlock(&self, vmk: Key) {
        let mut inner = self.lock();
        if inner.vmk.is_none() {
            Self::set_open(&mut inner, &vmk);
        }
    }

    /// Opens the vault with `vmk`, known to be its key, keeping a check of it
    /// the first time (to know a key handed over at an update).
    fn set_open(inner: &mut Inner, vmk: &[u8; 32]) {
        inner.vmk = memory::Locked::new(vmk);
        inner.state = None;
        let Some(db) = &inner.db else { return };
        let checked: bool = db.query_row("SELECT COUNT(*) > 0 FROM kept WHERE name = 'check'", [], |row| row.get(0)).unwrap_or(true);
        if !checked && let Err(error) = Self::keep(db, &inner.vault_id, vmk, "check", b"claktory") {
            tracing::warn!(%error, "could not keep a check of the vault's key");
        }
    }

    /// Opens the vault with a key the backend before this one handed over,
    /// if it's this vault's. Whether it's open now.
    pub(crate) fn take(&self, handed: handoff::Handed) -> bool {
        let mut inner = self.lock();
        if inner.vmk.is_some() {
            return true;
        }
        let Some(db) = &inner.db else { return false };
        if handed.vault_id != inner.vault_id {
            return false;
        }
        let sealed: Option<Vec<u8>> =
            db.query_row("SELECT sealed FROM kept WHERE name = 'check'", [], |row| row.get(0)).optional().ok().flatten();
        let good = sealed.is_some_and(|sealed| crypto::open(&handed.vmk, &sealed, &aad(&["kept", &inner.vault_id, "check"])).is_some());
        if good {
            inner.vmk = Some(handed.vmk);
            inner.state = None;
        }
        good
    }

    /// Hands the open vault's key to the next backend, before this one exits
    /// for an update: through the supervisor, or `exec` (Some: the fd to
    /// leave open, for `handoff::FD_ENV`). Without a key, nothing.
    pub(crate) fn hand_over(&self, exec: bool) -> Option<std::os::fd::OwnedFd> {
        let inner = self.lock();
        let vmk = inner.vmk.as_deref()?;
        if exec {
            return handoff::for_exec(&inner.vault_id, vmk)
                .inspect_err(|error| tracing::warn!(%error, "could not hand the vault's key over"))
                .ok();
        }
        if !handoff::keep(&inner.vault_id, vmk) {
            tracing::info!("no supervisor to hand the vault's key to; it opens again by itself, or asks");
        }
        None
    }

    fn open_vmk(inner: &Inner) -> Result<&[u8; 32], VaultError> {
        if inner.db.is_none() {
            return Err(VaultError::NoVault);
        }
        inner.vmk.as_deref().ok_or(VaultError::Locked)
    }

    fn db(inner: &Inner) -> Result<&Connection, VaultError> {
        inner.db.as_ref().ok_or(VaultError::NoVault)
    }

    /// Holds the unlocked vault through a write, committing before invalidating its cached view.
    fn with_unlocked_transaction<T>(
        &self,
        write: impl FnOnce(&rusqlite::Transaction<'_>, &str, &[u8; 32]) -> Result<T, VaultError>,
    ) -> Result<T, VaultError> {
        let mut inner = self.lock();
        let vmk = Zeroizing::new(*Self::open_vmk(&inner)?);
        let vault_id = inner.vault_id.clone();
        let Some(db) = inner.db.as_mut() else { return Err(VaultError::NoVault) };
        let transaction = db.transaction()?;
        let value = write(&transaction, &vault_id, &vmk)?;
        transaction.commit()?;
        inner.state = None;
        Ok(value)
    }

    /// Replaces the passphrase, given the vault's key.
    pub(crate) fn set_passphrase(&self, vmk: &Key, passphrase: &str) -> Result<(), VaultError> {
        let inner = self.lock();
        let protector = Self::passphrase_protector(&inner, &inner.vault_id, vmk, passphrase)?;
        Self::save_protector(Self::db(&inner)?, &protector)
    }

    /// A new recovery key; the old one stops working.
    pub(crate) fn new_recovery_key(&self) -> Result<String, VaultError> {
        let mut inner = self.lock();
        let vmk = Self::open_vmk(&inner)?;
        let (text, protector) = Self::recovery_protector(&inner.vault_id, vmk);
        let db = Self::db(&inner)?;
        Self::save_protector(db, &protector)?;
        Self::keep(db, &inner.vault_id, vmk, "recovery", text.as_bytes())?;
        // It needs confirming again (recovery::).
        inner.state = None;
        Ok(text)
    }

    /// A new key on disk for a vault that moved to this computer
    /// (`move_import`), so the old key, which the move carried, no longer
    /// opens it; its values are unchanged. Safe to run again after an
    /// interruption: a new key whose protector was saved before it was moved
    /// into place (`auto.key.new`) is moved into place. Ok(false) when the
    /// vault keeps no key on disk (locked mode, or opened by the account).
    pub(crate) fn rotate_auto_key(&self) -> Result<bool, VaultError> {
        let next = self.dir.join("auto.key.new");
        let mut inner = self.lock();
        let has_auto: bool =
            Self::db(&inner)?.query_row("SELECT COUNT(*) > 0 FROM protectors WHERE kind = 'auto'", [], |row| row.get(0))?;
        if inner.locked_mode || !has_auto {
            return Ok(false);
        }
        if inner.vmk.is_none()
            && let Some(vmk) = Self::vmk_with_key_file(&inner, &next)
        {
            std::fs::rename(&next, self.auto_path())?;
            Self::set_open(&mut inner, &vmk);
            return Ok(true);
        }
        let vmk = Zeroizing::new(*Self::open_vmk(&inner)?);
        let kek = crypto::new_key();
        private_file(&next, crypto::hex(kek.as_slice()).as_bytes())?;
        let sealed_vmk = crypto::seal(&kek, vmk.as_slice(), &aad(&["vmk", &inner.vault_id, "auto"]));
        let protector = Protector { kind: "auto".to_owned(), params: "file".to_owned(), salt: Vec::new(), sealed_vmk };
        Self::save_protector(Self::db(&inner)?, &protector)?;
        std::fs::rename(&next, self.auto_path())?;
        Ok(true)
    }

    /// Locked mode on: the key on disk is deleted. Off: it's kept again.
    pub(crate) fn set_locked_mode(&self, locked_mode: bool) -> Result<(), VaultError> {
        let mut inner = self.lock();
        let vmk = Zeroizing::new(*Self::open_vmk(&inner)?);
        if inner.locked_mode == locked_mode {
            return Ok(());
        }
        if locked_mode {
            let held: i64 =
                Self::db(&inner)?
                    .query_row("SELECT COUNT(*) FROM protectors WHERE kind IN ('passphrase', 'recovery')", [], |row| row.get(0))?;
            if held == 0 {
                return Err(VaultError::Invalid(
                    "Locked mode needs a passphrase or recovery key to open the vault with, and it has neither.".to_owned(),
                ));
            }
            Self::db(&inner)?.execute("DELETE FROM protectors WHERE kind = 'auto'", [])?;
            match std::fs::remove_file(self.auto_path()) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
                _ => {}
            }
        } else {
            let protector = self.auto_protector(&inner.vault_id, &vmk)?;
            Self::save_protector(Self::db(&inner)?, &protector)?;
        }
        Self::db(&inner)?.execute("UPDATE vault SET locked_mode = ?1", [locked_mode])?;
        inner.locked_mode = locked_mode;
        inner.state = None;
        Ok(())
    }

    fn secret_id(db: &Connection, id_or_name: &str) -> Result<(String, String), VaultError> {
        db.query_row(
            "SELECT id, name FROM secrets WHERE id = ?1 OR name = ?1 ORDER BY id = ?1 DESC LIMIT 1",
            [id_or_name],
            crate::store::row2,
        )
        .optional()?
        .ok_or_else(|| VaultError::NoSecret(id_or_name.to_owned()))
    }

    /// Seals `values` as `version` of the secret, replacing its variables.
    fn write_values(
        db: &Connection,
        vault_id: &str,
        vmk: &[u8; 32],
        id: &str,
        version: u32,
        values: &[Value],
        ask: bool,
    ) -> Result<Vec<u8>, VaultError> {
        let dek = crypto::new_key();
        let version = version.to_string();
        let sealed_dek = crypto::seal(vmk, dek.as_slice(), &dek_aad(vault_id, id, &version, ask));
        db.execute("DELETE FROM variables WHERE secret = ?1", [id])?;
        for (position, value) in (0i64..).zip(values) {
            let sealed = crypto::seal(&dek, value.value.as_bytes(), &aad(&["value", vault_id, id, &version, &value.variable]));
            let hint = if value.binary || value.variable == held_codes::CODES_VARIABLE { String::new() } else { hint(&value.value) };
            db.execute(
                "INSERT INTO variables (secret, position, name, file, hint, short, sealed_value, binary) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![id, position, value.variable, value.file, hint, value.value.len() < SHORT, sealed, value.binary],
            )?;
        }
        Ok(sealed_dek)
    }

    fn name_taken(db: &Connection, name: &str, except: &str) -> Result<(), VaultError> {
        let taken: Option<String> =
            db.query_row("SELECT name FROM secrets WHERE name = ?1 AND id != ?2", [name, except], |row| row.get(0)).optional()?;
        match taken {
            Some(taken) => Err(VaultError::Invalid(format!("There's a secret named {taken} already."))),
            None => Ok(()),
        }
    }

    /// Adds a secret, granted to no one; returns its id.
    pub(crate) fn create_secret(&self, name: &str, description: &str, values: &[Value], owner: &str) -> Result<String, VaultError> {
        let name = check_name(name)?;
        check_values(values)?;
        self.with_unlocked_transaction(|transaction, vault_id, vmk| {
            Self::name_taken(transaction, &name, "")?;
            let id = format!("s{}", crypto::hex(&crypto::random::<6>()));
            let sealed_dek = Self::write_values(transaction, vault_id, vmk, &id, 1, values, false)?;
            let now = now_ms();
            transaction.execute(
                "INSERT INTO secrets (id, name, description, version, sealed_dek, owner, created, updated) VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6, ?6)",
                params![id, name, description.trim(), sealed_dek, owner, now],
            )?;
            Ok(id)
        })
    }

    /// Renames and redescribes a secret, and with `values`, replaces its
    /// variables (a new version). Returns its name, before the change.
    pub(crate) fn update_secret(&self, id: &str, name: &str, description: &str, values: Option<&[Value]>) -> Result<String, VaultError> {
        let name = check_name(name)?;
        if let Some(values) = values {
            check_values(values)?;
        }
        let mut inner = self.lock();
        let vmk = if values.is_some() { Some(Zeroizing::new(*Self::open_vmk(&inner)?)) } else { None };
        let vault_id = inner.vault_id.clone();
        let Some(db) = inner.db.as_mut() else { return Err(VaultError::NoVault) };
        let transaction = db.transaction()?;
        let (id, before) = Self::secret_id(&transaction, id)?;
        Self::name_taken(&transaction, &name, &id)?;
        if values.is_some() {
            match Self::kind_of(&transaction, &id)?.as_str() {
                "" => {}
                import::KIND_IMPORT => {
                    return Err(VaultError::Invalid(format!(
                        "{before} was imported from a file, so its values can't be edited. Rename it, or import the file again."
                    )));
                }
                _ => {
                    return Err(VaultError::Invalid(format!(
                        "{before} is made from a connected service, so its values can't be edited. Rename it, or connect the service again."
                    )));
                }
            }
        }
        let now = now_ms();
        transaction.execute(
            "UPDATE secrets SET name = ?2, description = ?3, updated = ?4 WHERE id = ?1",
            params![id, name, description.trim(), now],
        )?;
        if let (Some(values), Some(vmk)) = (values, vmk) {
            let (version, ask): (u32, bool) =
                transaction.query_row("SELECT version, ask FROM secrets WHERE id = ?1", [&id], crate::store::row2)?;
            let sealed_dek = Self::write_values(&transaction, &vault_id, &vmk, &id, version + 1, values, ask)?;
            transaction.execute("UPDATE secrets SET version = ?2, sealed_dek = ?3 WHERE id = ?1", params![id, version + 1, sealed_dek])?;
        }
        transaction.commit()?;
        inner.state = None;
        Ok(before)
    }

    /// Deletes a secret and its grants; its uses stay in the log. Returns
    /// its name and the agents it was granted to.
    pub(crate) fn delete_secret(&self, id: &str) -> Result<(String, Vec<String>), VaultError> {
        let mut inner = self.lock();
        let Some(db) = inner.db.as_mut() else { return Err(VaultError::NoVault) };
        let transaction = db.transaction()?;
        let (id, name) = Self::secret_id(&transaction, id)?;
        if Self::kind_of(&transaction, &id)? == connections::KIND_CONNECTION {
            return Err(VaultError::Invalid(format!("To remove {name}, disconnect it.")));
        }
        let agents = query(&transaction, "SELECT agent FROM grants WHERE secret = ?1 ORDER BY granted", [&id], |row| row.get(0))?;
        for table in ["variables", "grants"] {
            transaction.execute(&format!("DELETE FROM {table} WHERE secret = ?1"), [&id])?;
        }
        transaction.execute("DELETE FROM secrets WHERE id = ?1", [&id])?;
        transaction.commit()?;
        inner.state = None;
        Ok((name, agents))
    }

    fn grant_key(inner: &Inner) -> Result<Key, VaultError> {
        let vmk = Self::open_vmk(inner)?;
        Ok(crypto::derive(vmk.as_slice(), inner.vault_id.as_bytes(), "claktory vault grants v1"))
    }

    /// Lets `agent` use the secret; `by` is the admin agent that did it, ''
    /// for the person. Returns its name.
    pub(crate) fn grant(&self, id: &str, agent: &str, by: &str) -> Result<String, VaultError> {
        self.grant_as(id, agent, by, &BTreeMap::new())
    }

    /// `grant`, with other names `agent`'s commands get the secret's values
    /// under (`aliases`: name → the secret's variable), checked.
    pub(crate) fn grant_as(&self, id: &str, agent: &str, by: &str, aliases: &BTreeMap<String, String>) -> Result<String, VaultError> {
        self.grant_with(id, agent, by, aliases, false)
    }

    /// `grant`, letting `agent` only fill the secret into its sites in
    /// Claktory's browser (agent-browser.md §5.3): its commands can't have
    /// it. Replaces a full grant it held.
    pub(crate) fn grant_fill_only(&self, id: &str, agent: &str, by: &str) -> Result<String, VaultError> {
        self.grant_with(id, agent, by, &BTreeMap::new(), true)
    }

    fn grant_with(&self, id: &str, agent: &str, by: &str, aliases: &BTreeMap<String, String>, fill_only: bool) -> Result<String, VaultError> {
        let mut inner = self.lock();
        let key = Self::grant_key(&inner)?;
        let db = Self::db(&inner)?;
        let (id, name) = Self::secret_id(db, id)?;
        if Self::kind_of(db, &id)? == connections::KIND_CONNECTION {
            return Err(VaultError::Invalid(format!(
                "Agents can't be given {name} itself. Make a secret from it with only the access the work needs, and give that."
            )));
        }
        let variables: Vec<String> = query(db, "SELECT name FROM variables WHERE secret = ?1", [&id], |row| row.get(0))?;
        for (alias, variable) in aliases {
            if !valid_variable(alias) {
                return Err(VaultError::Invalid(format!(
                    "\"{alias}\" isn't a variable name: letters, digits and _, not starting with a digit."
                )));
            }
            if variables.contains(alias) {
                return Err(VaultError::Invalid(format!(
                    "{name} sets {alias} already. Aliases go the other way: each name asked for, to the variable of {name} it stands for; with a single mismatch on each side, leave aliases out and they pair by themselves."
                )));
            }
            if !variables.contains(variable) {
                return Err(VaultError::Invalid(format!("{name} has no variable {variable}.")));
            }
        }
        let aliases = aliases_text(aliases);
        let now = now_ms();
        let mac = crypto::mac(&key, &grant_message(&inner.vault_id, &id, agent, now, &aliases, fill_only));
        db.execute(
            "INSERT OR REPLACE INTO grants (secret, agent, granted, mac, aliases, fill_only) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, agent, now, mac, aliases, fill_only],
        )?;
        Self::log(db, &id, &name, "grant", agent, "", by)?;
        inner.state = None;
        Ok(name)
    }

    /// Stops `agent` using the secret; `by` as for `grant`. Returns its name.
    pub(crate) fn revoke(&self, id: &str, agent: &str, by: &str) -> Result<String, VaultError> {
        let mut inner = self.lock();
        let db = Self::db(&inner)?;
        let (id, name) = Self::secret_id(db, id)?;
        if db.execute("DELETE FROM grants WHERE secret = ?1 AND agent = ?2", [&id, agent])? == 0 {
            return Err(VaultError::NotGranted { secret: name, agent: agent.to_owned() });
        }
        Self::log(db, &id, &name, "revoke", agent, "", by)?;
        inner.state = None;
        Ok(name)
    }

    /// The secret `agent` holds named `name` (any case): its id and name.
    /// Read from the grants as they are, without checking them, so an agent
    /// can give one up while the vault is locked too; None for a name it
    /// doesn't hold, whether or not the vault has it.
    pub(crate) fn held(&self, agent: &str, name: &str) -> Result<Option<(String, String)>, VaultError> {
        let inner = self.lock();
        let Some(db) = &inner.db else { return Ok(None) };
        Ok(db
            .query_row(
                "SELECT secrets.id, secrets.name FROM grants JOIN secrets ON secrets.id = grants.secret
                 WHERE grants.agent = ?1 AND secrets.name = ?2 COLLATE NOCASE ORDER BY secrets.name = ?2 DESC LIMIT 1",
                [agent, name.trim()],
                crate::store::row2,
            )
            .optional()?)
    }

    /// The names of the secrets `agent` holds, as `held` reads them.
    pub(crate) fn held_names(&self, agent: &str) -> Vec<String> {
        let inner = self.lock();
        let Some(db) = &inner.db else { return Vec::new() };
        query(
            db,
            "SELECT secrets.name FROM grants JOIN secrets ON secrets.id = grants.secret WHERE grants.agent = ?1 ORDER BY secrets.name COLLATE NOCASE",
            [agent],
            |row| row.get(0),
        )
        .unwrap_or_default()
    }

    /// Takes every grant from an agent that's gone. Returns the secrets'
    /// names.
    pub(crate) fn forget_agent(&self, agent: &str) -> Result<Vec<String>, VaultError> {
        let mut inner = self.lock();
        let Some(db) = &inner.db else { return Ok(Vec::new()) };
        let names = query(
            db,
            "SELECT secrets.name FROM grants JOIN secrets ON secrets.id = grants.secret WHERE grants.agent = ?1",
            [agent],
            |row| row.get(0),
        )?;
        db.execute("DELETE FROM grants WHERE agent = ?1", [agent])?;
        db.execute("DELETE FROM requests WHERE agent = ?1", [agent])?;
        db.execute("DELETE FROM connect_proposals WHERE agent = ?1", [agent])?;
        inner.state = None;
        Ok(names)
    }

    fn log(db: &Connection, id: &str, name: &str, kind: &str, agent: &str, task: &str, detail: &str) -> Result<(), VaultError> {
        db.execute(
            "INSERT INTO uses (secret, secret_name, time, kind, agent, task, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, name, now_ms(), kind, agent, task, detail],
        )?;
        Ok(())
    }

    /// A secret's values, decrypted.
    fn values_of(db: &Connection, vault_id: &str, vmk: &[u8; 32], id: &str) -> Result<Vec<Value>, VaultError> {
        let (version, sealed_dek, ask, name): (u32, Vec<u8>, bool, String) =
            db.query_row("SELECT version, sealed_dek, ask, name FROM secrets WHERE id = ?1", [id], crate::store::row4)?;
        let version = version.to_string();
        let corrupt = || VaultError::Invalid(format!("The secret {name} can't be opened: the vault was changed outside Claktory."));
        let (dek, older) = open_dek(vmk, &sealed_dek, vault_id, id, &version, ask).ok_or_else(corrupt)?;
        if older {
            let resealed = crypto::seal(vmk, dek.as_slice(), &dek_aad(vault_id, id, &version, true));
            db.execute("UPDATE secrets SET sealed_dek = ?2 WHERE id = ?1 AND sealed_dek = ?3", params![id, resealed, sealed_dek])?;
        }
        let mut statement = db.prepare("SELECT name, file, sealed_value, binary FROM variables WHERE secret = ?1 ORDER BY position")?;
        let rows = statement.query_map([id], crate::store::row4::<String, bool, Vec<u8>, bool>)?.collect::<Result<Vec<_>, _>>()?;
        let mut values = Vec::with_capacity(rows.len());
        for (variable, file, sealed, binary) in rows {
            let bytes = crypto::open(&dek, &sealed, &aad(&["value", vault_id, id, &version, &variable])).ok_or_else(corrupt)?;
            let value = String::from_utf8(bytes.to_vec()).map_err(|_| corrupt())?;
            values.push(Value { variable, value: Zeroizing::new(value), file, binary });
        }
        Ok(values)
    }

    /// A secret's values for the person, logged with the device.
    pub(crate) fn reveal(&self, id: &str, device: &str) -> Result<(String, Vec<Value>), VaultError> {
        let inner = self.lock();
        let vmk = Self::open_vmk(&inner)?;
        let db = Self::db(&inner)?;
        let (id, name) = Self::secret_id(db, id)?;
        let values = Self::values_of(db, &inner.vault_id, vmk, &id)?;
        Self::log(db, &id, &name, "reveal", "", "", device)?;
        Ok((id, values))
    }

    /// Who used secrets and how, newest first.
    pub(crate) fn uses(&self, id: &str, limit: u32) -> Result<Vec<v1::SecretUse>, VaultError> {
        let inner = self.lock();
        let db = Self::db(&inner)?;
        let limit = if limit == 0 { 100 } else { limit.min(1000) };
        Ok(query(
            db,
            "SELECT secret, secret_name, time, kind, agent, task, detail FROM uses WHERE ?1 = '' OR secret = ?1 ORDER BY id DESC LIMIT ?2",
            params![id, limit],
            |row| {
                Ok(v1::SecretUse {
                    secret_id: row.get(0)?,
                    secret_name: row.get(1)?,
                    time_ms: row.get(2)?,
                    kind: row.get(3)?,
                    agent_id: row.get(4)?,
                    task_id: row.get(5)?,
                    detail: row.get(6)?,
                })
            },
        )?)
    }

    /// The vault as clients see it: never values. `running` says whether an
    /// agent's background task (agent, task id) runs now, asked for each task
    /// that got a secret lately, of each grant: the Workspace answers it from
    /// one read of the running tasks (#402), never a lookup per task.
    pub(crate) fn state(&self, running: impl Fn(&str, &str) -> bool) -> v1::VaultState {
        let mut inner = self.lock();
        if inner.state.is_none() {
            let (state, recent) = match Self::read_state(&inner) {
                Ok(read) => read,
                Err(error) => {
                    tracing::error!(%error, "could not read the vault");
                    let state = v1::VaultState { exists: inner.db.is_some(), locked_mode: inner.locked_mode, ..v1::VaultState::default() };
                    (state, HashMap::new())
                }
            };
            inner.state = Some(state);
            inner.recent = recent;
        }
        let mut state = inner.state.clone().unwrap_or_default();
        for secret in &mut state.secrets {
            for grant in &mut secret.grants {
                let tasks = inner.recent.get(&(secret.id.clone(), grant.agent_id.clone())).map_or(&[][..], Vec::as_slice);
                grant.running_tasks = tasks.iter().filter(|task| running(&grant.agent_id, task)).count() as u32;
            }
        }
        state
    }

    /// The background tasks of `agent` that got the secret lately (some may
    /// have ended).
    pub(crate) fn recent_tasks(&self, secret: &str, agent: &str) -> Vec<String> {
        let inner = self.lock();
        let Some(db) = &inner.db else { return Vec::new() };
        let Ok((id, _)) = Self::secret_id(db, secret) else { return Vec::new() };
        Self::tasks_of(db, &id, agent).unwrap_or_default()
    }

    fn tasks_of(db: &Connection, secret: &str, agent: &str) -> Result<Vec<String>, VaultError> {
        let mut statement =
            db.prepare("SELECT DISTINCT task FROM uses WHERE secret = ?1 AND agent = ?2 AND kind = 'use' AND task != '' AND time > ?3")?;
        let tasks =
            statement.query_map(params![secret, agent, now_ms() - RECENT_MS], |row| row.get(0))?.collect::<Result<Vec<String>, _>>()?;
        Ok(tasks)
    }

    fn read_state(inner: &Inner) -> Result<(v1::VaultState, Recent), VaultError> {
        let Some(db) = &inner.db else { return Ok((v1::VaultState::default(), HashMap::new())) };
        let mut recent = HashMap::new();
        let grant_key = Self::grant_key(inner).ok();
        let mut secrets = Vec::new();
        let mut statement = db.prepare(
            "SELECT id, name, description, version, owner, created, updated, last_used_by, last_used, ask FROM secrets ORDER BY name COLLATE NOCASE",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(v1::SecretInfo {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                version: row.get(3)?,
                owner_agent_id: row.get(4)?,
                created_ms: row.get(5)?,
                updated_ms: row.get(6)?,
                last_used_by: row.get(7)?,
                last_used_ms: row.get(8)?,
                ask_each_time: row.get(9)?,
                ..v1::SecretInfo::default()
            })
        })?;
        for secret in rows {
            let mut secret = secret?;
            let mut variables = db.prepare("SELECT name, hint, file, short FROM variables WHERE secret = ?1 ORDER BY position")?;
            for row in variables.query_map([&secret.id], |row| {
                Ok((v1::SecretVariable { name: row.get(0)?, hint: row.get(1)?, file: row.get(2)? }, row.get::<_, bool>(3)?))
            })? {
                let (variable, short) = row?;
                if short {
                    secret.warnings.push(format!("{} is too short to hide in agents' output.", variable.name));
                }
                secret.variables.push(variable);
            }
            let mut grants = db.prepare("SELECT agent, granted, mac, aliases, fill_only FROM grants WHERE secret = ?1 ORDER BY granted")?;
            let rows = grants
                .query_map([&secret.id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, Vec<u8>>(2)?, row.get::<_, String>(3)?, row.get::<_, bool>(4)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            for (agent, granted, mac, aliases, fill_only) in rows {
                // Locked, grants can't be checked, and can't be used either.
                if let Some(key) = &grant_key
                    && !crypto::mac_matches(key, &grant_message(&inner.vault_id, &secret.id, &agent, granted, &aliases, fill_only), &mac)
                {
                    secret.warnings.push(format!("A grant to {agent} was changed outside Claktory, so it's ignored."));
                    continue;
                }
                let tasks = Self::tasks_of(db, &secret.id, &agent)?;
                if !tasks.is_empty() {
                    recent.insert((secret.id.clone(), agent.clone()), tasks);
                }
                secret.grants.push(v1::SecretGrant {
                    agent_id: agent,
                    granted_ms: granted,
                    running_tasks: 0,
                    aliases: aliases_of(&aliases).into_iter().collect(),
                    fill_only,
                });
            }
            Self::sites_view(inner, db, &mut secret)?;
            Self::connection_view(db, &mut secret)?;
            secrets.push(secret);
        }
        let state = v1::VaultState {
            exists: true,
            unlocked: inner.vmk.is_some(),
            locked_mode: inner.locked_mode,
            secrets,
            // Requests show only in the conversation of the agent that asked.
            proposals: connections::pending_proposals(db)?,
            recovery_key_unconfirmed: Self::recovery_unconfirmed(inner)?,
            ..v1::VaultState::default()
        };
        Ok((state, recent))
    }

    /// Whether `agent` holds a grant of the secret with the right MAC that
    /// lets it use the values (not a fill-only one).
    fn is_granted(inner: &Inner, key: &Key, secret: &str, agent: &str) -> Result<bool, VaultError> {
        Ok(Self::grant_of(inner, key, secret, agent)?.is_some_and(|grant| !grant.fill_only))
    }

    /// `agent`'s grant of the secret, if it holds one with the right MAC.
    fn grant_of(inner: &Inner, key: &Key, secret: &str, agent: &str) -> Result<Option<Grant>, VaultError> {
        let row: Option<(i64, Vec<u8>, String, bool)> = Self::db(inner)?
            .query_row(
                "SELECT granted, mac, aliases, fill_only FROM grants WHERE secret = ?1 AND agent = ?2",
                [secret, agent],
                crate::store::row4,
            )
            .optional()?;
        Ok(row
            .filter(|(granted, mac, aliases, fill_only)| {
                crypto::mac_matches(key, &grant_message(&inner.vault_id, secret, agent, *granted, aliases, *fill_only), mac)
            })
            .map(|(_, _, aliases, fill_only)| Grant { aliases: aliases_of(&aliases), fill_only }))
    }

    /// The secrets granted to `agent`, by id, as it's told of them: "Name
    /// (sets A, B): description". Empty while the vault is locked, since
    /// grants can't be checked then, nor used.
    pub(crate) fn granted(&self, agent: &str) -> BTreeMap<String, String> {
        let inner = self.lock();
        let Ok(key) = Self::grant_key(&inner) else { return BTreeMap::new() };
        let read = || -> Result<BTreeMap<String, String>, VaultError> {
            let db = Self::db(&inner)?;
            let mut statement = db.prepare(
                "SELECT secrets.id, secrets.name, secrets.description, secrets.ask FROM grants JOIN secrets ON secrets.id = grants.secret
                 WHERE grants.agent = ?1 ORDER BY secrets.name COLLATE NOCASE",
            )?;
            let rows = statement.query_map([agent], crate::store::row4::<String, String, String, bool>)?.collect::<Result<Vec<_>, _>>()?;
            let mut granted = BTreeMap::new();
            for (id, name, description, ask) in rows {
                let Some(Grant { aliases, fill_only }) = Self::grant_of(&inner, &key, &id, agent)? else { continue };
                let variables = db
                    .prepare("SELECT name, file FROM variables WHERE secret = ?1 ORDER BY position")?
                    .query_map([&id], crate::store::row2::<String, bool>)?
                    .map(|row| {
                        row.map(|(name, file)| {
                            let mut label = name.clone();
                            for (alias, _) in aliases.iter().filter(|(_, variable)| **variable == name) {
                                label.push_str(&format!(" also as {alias}"));
                            }
                            if file { format!("{label} (a file's path)") } else { label }
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let about = if description.is_empty() { String::new() } else { format!(": {description}") };
                // Told once in its prompt, rather than learning it from a refusal.
                let asks = if ask {
                    " It asks the person each time it's used: a command with it isn't run at first, and they're asked to allow that exact command once; when you're told they allowed it, run it again exactly as it was."
                } else {
                    ""
                };
                // Where it may be filled in Claktory's browser, if anywhere.
                let sites = Self::sites_of(&inner, &key, &id)?;
                let fills = match (fill_only, sites.is_empty()) {
                    (true, true) => "; fill only, and bound to no site yet, so it can't be filled anywhere".to_owned(),
                    (true, false) => format!("; fill only: browser_fill_secret puts it into {}, never a command", sites.join(", ")),
                    (false, true) => String::new(),
                    (false, false) => format!("; browser_fill_secret also puts it into {}", sites.join(", ")),
                };
                let asks = if fill_only { "" } else { asks };
                granted.insert(id, format!("{name} (sets {}{fills}){about}{asks}", variables.join(", ")));
            }
            Ok(granted)
        };
        read().unwrap_or_else(|error| {
            tracing::error!(%error, agent, "could not read the vault's grants");
            BTreeMap::new()
        })
    }

    /// The first value of secret `id` for `grantee` (an app's "app:<id>"),
    /// if the vault keeps it granted to it; None if not, or it's gone.
    pub(crate) fn value_for(&self, id: &str, grantee: &str) -> Result<Option<Zeroizing<String>>, VaultError> {
        let inner = self.lock();
        let key = Self::grant_key(&inner)?;
        let vmk = Self::open_vmk(&inner)?;
        let db = Self::db(&inner)?;
        let exists: Option<String> = db.query_row("SELECT id FROM secrets WHERE id = ?1", [id], |row| row.get(0)).optional()?;
        if exists.is_none() || !Self::is_granted(&inner, &key, id, grantee)? {
            return Ok(None);
        }
        // Nothing can ask the person for an app's use: it can't have it.
        if Self::asks(db, id)? {
            return Err(VaultError::Invalid(
                "This secret asks the person each time it's used, so an app can't use it. Turn Ask each time off for it in Settings → Secrets to let apps use it.".to_owned(),
            ));
        }
        Ok(Self::values_of(db, &inner.vault_id, vmk, id)?.into_iter().next().map(|value| value.value))
    }

    /// Every variable of the secrets `names` (by name, any case), for one of
    /// `agent`'s commands; or why not, in words for the agent.
    pub(crate) fn resolve(&self, agent: &str, names: &[String]) -> Result<Vec<factory_agent::SecretVariable>, String> {
        match self.resolve_granted(agent, names) {
            Ok(variables) => Ok(variables),
            Err(Refusal::Said(text)) => Err(text),
            Err(Refusal::NotYours(name)) => {
                let yours: Vec<String> =
                    self.granted(agent).into_values().map(|label| label.split(" (sets").next().unwrap_or_default().to_owned()).collect();
                let yours = if yours.is_empty() { "You have none yet.".to_owned() } else { format!("Yours: {}.", yours.join(", ")) };
                Err(format!(
                    "\"{name}\" isn't one of your secrets. {yours} If you need another, describe it and ask with `secrets_request`."
                ))
            }
        }
    }

    fn resolve_granted(&self, agent: &str, names: &[String]) -> Result<Vec<factory_agent::SecretVariable>, Refusal> {
        let inner = self.lock();
        let Some(db) = &inner.db else {
            return Err(Refusal::Said(
                "There's no vault in this Claktory, so there are no secrets. Ask the person to add the one you need in Settings → Secrets."
                    .to_owned(),
            ));
        };
        let (Ok(key), Ok(vmk)) = (Self::grant_key(&inner), Self::open_vmk(&inner)) else {
            if account::protector(&inner).is_some() {
                return Err(Refusal::Said(
                    "The vault is locked: Claktory restarted and can't reach claktory.com yet to open it. It opens by itself once it \
                     can, or the person can open it with the recovery key in Settings → Secrets. Try again later."
                        .to_owned(),
                ));
            }
            return Err(Refusal::Said(
                "The vault is locked: Claktory restarted, and it opens only with its passphrase. Ask the person to unlock it in Settings → Secrets.".to_owned(),
            ));
        };
        let mut variables: Vec<factory_agent::SecretVariable> = Vec::new();
        for asked in names {
            let found: Option<(String, String)> =
                db.query_row("SELECT id, name FROM secrets WHERE name = ?1", [asked.trim()], crate::store::row2).optional()?;
            let Some((id, name)) = found else { return Err(Refusal::NotYours(asked.trim().to_owned())) };
            // Said as for a name the vault doesn't have: nothing shows through.
            let Some(Grant { aliases, fill_only }) = Self::grant_of(&inner, &key, &id, agent)? else {
                return Err(Refusal::NotYours(asked.trim().to_owned()));
            };
            if fill_only {
                return Err(Refusal::Said(format!(
                    "{name} is yours only to fill into its sites in Claktory's browser, with browser_fill_secret: no command can have it."
                )));
            }
            if Self::asks(db, &id)? && !Self::is_armed(&inner, agent, &name) {
                return Err(Refusal::Said(format!(
                    "{name} asks the person each time it's used, and this command wasn't allowed: run it with bash's `secrets`, and they're asked."
                )));
            }
            let public = Self::public_variables(db, &id)?;
            for value in Self::values_of(db, &inner.vault_id, vmk, &id)? {
                // Its own name, and any other the grant gives it.
                let names = std::iter::once(value.variable.clone())
                    .chain(aliases.iter().filter(|(_, variable)| **variable == value.variable).map(|(alias, _)| alias.clone()));
                for variable in names.collect::<Vec<_>>() {
                    if let Some(other) = variables.iter().find(|other| other.variable == variable) {
                        return Err(Refusal::Said(format!(
                            "{} and {name} both set {variable}: use one of them per command.",
                            other.secret
                        )));
                    }
                    variables.push(factory_agent::SecretVariable {
                        secret: name.clone(),
                        public: public.contains(&value.variable),
                        variable,
                        value: value.value.clone(),
                        file: value.file,
                        binary: value.binary,
                    });
                }
            }
        }
        // Allowed once: what was armed for these is used up.
        drop(inner);
        let resolved: Vec<String> = variables.iter().map(|variable| variable.secret.clone()).collect();
        self.lock().armed.retain(|(armed, names, _)| {
            !(armed == agent && names.iter().any(|name| resolved.iter().any(|done| done.eq_ignore_ascii_case(name.trim()))))
        });
        Ok(variables)
    }

    /// Whether the secret (by id) asks the person each time it's used.
    fn asks(db: &Connection, id: &str) -> Result<bool, VaultError> {
        Ok(db.query_row("SELECT ask FROM secrets WHERE id = ?1", [id], |row| row.get(0)).optional()?.unwrap_or(false))
    }

    /// Which of `names` (secrets, by name, any case) ask the person each
    /// time they're used, by their own names.
    pub(crate) fn asking(&self, names: &[String]) -> Vec<String> {
        let inner = self.lock();
        let Some(db) = &inner.db else { return Vec::new() };
        names
            .iter()
            .filter_map(|asked| {
                db.query_row("SELECT name FROM secrets WHERE name = ?1 AND ask = 1", [asked.trim()], |row| row.get::<_, String>(0))
                    .optional()
                    .ok()
                    .flatten()
            })
            .collect()
    }

    /// The person allowed `agent`'s command with these secrets: resolving
    /// them is allowed once, within a minute. Only the gate that checked the
    /// person's approval arms it.
    pub(crate) fn arm(&self, agent: &str, names: &[String]) {
        let mut inner = self.lock();
        let now = now_ms();
        inner.armed.retain(|(_, _, until)| *until > now);
        inner.armed.push((agent.to_owned(), names.to_vec(), now + ARMED_MS));
    }

    fn is_armed(inner: &Inner, agent: &str, name: &str) -> bool {
        let now = now_ms();
        inner.armed.iter().any(|(armed, names, until)| {
            armed == agent && *until > now && names.iter().any(|armed| armed.trim().eq_ignore_ascii_case(name))
        })
    }

    /// Ask each time on or off for a secret: its key is sealed again with
    /// the flag. A connected service's own can't: only Claktory's broker
    /// uses it, and nothing could ask. Returns its name.
    pub(crate) fn set_ask_each_time(&self, id: &str, ask: bool) -> Result<String, VaultError> {
        self.with_unlocked_transaction(|transaction, vault_id, vmk| {
            let (id, name) = Self::secret_id(transaction, id)?;
            if Self::kind_of(transaction, &id)? == connections::KIND_CONNECTION {
                return Err(VaultError::Invalid(format!(
                    "{name} is a connected service: only Claktory uses it, to make secrets. Set Ask each time on the secrets made from it."
                )));
            }
            let (version, sealed, was): (u32, Vec<u8>, bool) =
                transaction.query_row("SELECT version, sealed_dek, ask FROM secrets WHERE id = ?1", [&id], crate::store::row3)?;
            if was == ask {
                return Ok(name);
            }
            let version = version.to_string();
            let (dek, _) = open_dek(vmk, &sealed, vault_id, &id, &version, was).ok_or_else(|| {
                VaultError::Invalid(format!("The secret {name} can't be opened: the vault was changed outside Claktory."))
            })?;
            let resealed = crypto::seal(vmk, dek.as_slice(), &dek_aad(vault_id, &id, &version, ask));
            transaction.execute(
                "UPDATE secrets SET ask = ?2, sealed_dek = ?3, updated = ?4 WHERE id = ?1",
                params![id, ask, resealed, now_ms()],
            )?;
            Ok(name)
        })
    }

    /// Every value in the vault, to hide in what agents and the app are
    /// shown; none while it's locked.
    pub(crate) fn every_variable(&self) -> Vec<factory_agent::SecretVariable> {
        let inner = self.lock();
        let (Ok(vmk), Ok(db)) = (Self::open_vmk(&inner), Self::db(&inner)) else { return Vec::new() };
        let read = || -> Result<Vec<factory_agent::SecretVariable>, VaultError> {
            let secrets = db
                .prepare("SELECT id, name, kind, meta FROM secrets")?
                .query_map([], crate::store::row4::<String, String, String, String>)?
                .collect::<Result<Vec<_>, _>>()?;
            let mut variables = Vec::new();
            for (id, name, kind, meta) in secrets {
                // What a connection's recipe says isn't secret (a region) stays readable.
                let public = connections::public_of(&kind, &meta);
                for value in Self::values_of(db, &inner.vault_id, vmk, &id)?.into_iter().filter(|value| !public.contains(&value.variable)) {
                    variables.push(factory_agent::SecretVariable {
                        secret: name.clone(),
                        variable: value.variable,
                        value: value.value,
                        file: value.file,
                        public: false,
                        binary: value.binary,
                    });
                }
            }
            Ok(variables)
        };
        read().unwrap_or_else(|error| {
            tracing::error!(%error, "could not read the vault's values to hide them");
            Vec::new()
        })
    }

    /// Logs a command that got secrets.
    pub(crate) fn record_use(&self, used: &factory_agent::SecretUse) -> Result<(), VaultError> {
        let mut inner = self.lock();
        let db = Self::db(&inner)?;
        let command: String = used.command.lines().next().unwrap_or_default().chars().take(200).collect();
        let task = used.task.as_deref().unwrap_or_default();
        let time = i64::try_from(used.started).unwrap_or_else(|_| now_ms());
        for name in &used.secrets {
            let Ok((id, name)) = Self::secret_id(db, name.trim()) else { continue };
            db.execute(
                "INSERT INTO uses (secret, secret_name, time, kind, agent, task, detail) VALUES (?1, ?2, ?3, 'use', ?4, ?5, ?6)",
                params![id, name, time, used.agent, task, command],
            )?;
            db.execute("UPDATE secrets SET last_used_by = ?2, last_used = ?3 WHERE id = ?1", params![id, used.agent, time])?;
        }
        inner.state = None;
        Ok(())
    }

    /// The identity providers Claktory's browser is signed in to (#435),
    /// by name ("Google"): the secret's id and its name now (empty when
    /// it's gone), and when it first signed in there. Empty without a vault.
    pub(crate) fn browser_accounts(&self) -> Vec<BrowserAccount> {
        let inner = self.lock();
        let Some(db) = inner.db.as_ref() else { return Vec::new() };
        query(
            db,
            "SELECT b.provider, b.secret, COALESCE(s.name, ''), b.since FROM browser_accounts b LEFT JOIN secrets s ON s.id = b.secret ORDER BY b.since",
            [],
            |row| Ok(BrowserAccount { provider: row.get(0)?, secret_id: row.get(1)?, secret_name: row.get(2)?, since: row.get(3)? }),
        )
        .unwrap_or_default()
    }
}

/// An identity provider the browser is signed in to (`Vault::browser_accounts`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BrowserAccount {
    pub provider: String,
    pub secret_id: String,
    pub secret_name: String,
    pub since: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hint_is_the_end_of_the_value_not_of_its_armor_or_newline() {
        assert_eq!(hint("a-token-of-some-length\n"), "ngth");
        assert_eq!(hint("short\n"), "");
        let armor = |edge: &str| format!("-----{edge} {}-----", "PRIVATE KEY");
        let pem = format!("{}\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASC\nBKcwggSjAgEAAoIBAQC7abcd\n{}\n", armor("BEGIN"), armor("END"));
        assert_eq!(hint(&pem), "abcd");
        let made = generate::make(generate::SSH_ED25519, "k").expect("made");
        let fingerprint = ssh_key::PublicKey::from_openssh(&made.public).expect("public").fingerprint(ssh_key::HashAlg::Sha256).to_string();
        assert_eq!(hint(&made.private), fingerprint[fingerprint.len() - 4..]);
    }

    const CHEAP: &str = "argon2id m=1024 t=1 p=1";

    pub(super) fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vault-test-{}", crypto::hex(&crypto::random::<8>())));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    pub(super) fn vault(dir: &Path) -> Vault {
        let vault = Vault::open(dir.join("vault"));
        vault.set_passphrase_params(CHEAP);
        vault
    }

    pub(super) fn value(variable: &str, value: &str) -> Value {
        Value { variable: variable.to_owned(), value: Zeroizing::new(value.to_owned()), file: false, binary: false }
    }

    fn unlock(vault: &Vault, given: &str) -> bool {
        let (id, protectors) = vault.protectors().expect("protectors");
        match Vault::check(&id, &protectors, given) {
            Some(vmk) => {
                vault.unlock(vmk);
                true
            }
            None => false,
        }
    }

    #[test]
    fn an_ask_each_time_secret_goes_only_to_an_allowed_command_once() {
        let dir = temp();
        let vault = vault(&dir);
        vault.create(Some("correct horse battery"), false).expect("create");
        let id = vault.create_secret("AWS", "Deploys.", &[value("AWS_SECRET", "wJalrXUtnFEMI-K7MDENG")], "").expect("secret");
        vault.grant(&id, "bo", "").expect("grant");
        vault.grant(&id, "app:deploy", "").expect("grant");
        let names = vec!["aws".to_owned()];
        assert!(vault.resolve("bo", &names).is_ok(), "off, it's given as before");
        assert!(vault.asking(&names).is_empty());

        assert_eq!(vault.set_ask_each_time(&id, true).expect("on"), "AWS");
        assert_eq!(vault.asking(&names), ["AWS"]);
        let refused = vault.resolve("bo", &names).expect_err("not allowed");
        assert!(refused.starts_with("AWS asks the person each time it's used"), "{refused}");
        assert!(vault.value_for(&id, "app:deploy").is_err(), "no app can ask the person");
        // Armed by the gate: once, and only for that agent.
        vault.arm("bo", &names);
        assert!(vault.resolve("lea", &names).is_err());
        assert_eq!(vault.resolve("bo", &names).expect("allowed")[0].value.as_str(), "wJalrXUtnFEMI-K7MDENG");
        assert!(vault.resolve("bo", &names).is_err(), "used up");
        assert!(vault.granted("bo").values().next().expect("granted").contains("It asks the person each time"));
        // Its values stay hidden in output all the same.
        assert!(vault.every_variable().iter().any(|variable| variable.value.as_str() == "wJalrXUtnFEMI-K7MDENG"));

        // Changed in vault.db alone, it opens for no one, rather than for everyone.
        let db = Connection::open(dir.join("vault/vault.db")).expect("db");
        db.execute("UPDATE secrets SET ask = 0", []).expect("flip");
        drop(db);
        let flipped = vault.resolve("bo", &names).expect_err("unusable");
        assert!(flipped.contains("changed outside Claktory"), "{flipped}");
        let db = Connection::open(dir.join("vault/vault.db")).expect("db");
        db.execute("UPDATE secrets SET ask = 1", []).expect("back");
        drop(db);

        // Changed by a Claktory from before Ask each time (one an update went
        // back to): its key isn't sealed with "ask", but its flag still asks.
        // It opens, still asks, and is sealed with "ask" again.
        vault.set_ask_each_time(&id, false).expect("off");
        let db = Connection::open(dir.join("vault/vault.db")).expect("db");
        db.execute("UPDATE secrets SET ask = 1", []).expect("an older Claktory's change");
        drop(db);
        assert!(vault.resolve("bo", &names).is_err(), "it still asks");
        vault.arm("bo", &names);
        assert_eq!(vault.resolve("bo", &names).expect("allowed")[0].value.as_str(), "wJalrXUtnFEMI-K7MDENG");
        let db = Connection::open(dir.join("vault/vault.db")).expect("db");
        db.execute("UPDATE secrets SET ask = 0", []).expect("flip");
        drop(db);
        assert!(vault.resolve("bo", &names).expect_err("sealed with ask again").contains("changed outside Claktory"));
        let db = Connection::open(dir.join("vault/vault.db")).expect("db");
        db.execute("UPDATE secrets SET ask = 1", []).expect("back");
        drop(db);

        // Off again, it's given as before; new values keep the flag.
        vault.set_ask_each_time(&id, false).expect("off");
        assert!(vault.resolve("bo", &names).is_ok());
        vault.set_ask_each_time(&id, true).expect("on");
        vault.update_secret(&id, "AWS", "Deploys.", Some(&[value("AWS_SECRET", "a-new-value-123456")])).expect("update");
        assert!(vault.resolve("bo", &names).is_err());
        vault.arm("bo", &names);
        assert_eq!(vault.resolve("bo", &names).expect("allowed")[0].value.as_str(), "a-new-value-123456");
    }

    #[test]
    fn a_deleted_secret_leaves_nothing_of_it_in_the_file() {
        let dir = temp();
        let vault = vault(&dir);
        vault.create(Some("correct horse battery"), false).expect("create");
        // A value long enough to find, sealed: look for its sealed bytes.
        let id = vault.create_secret("Long", "", &[value("X", &"v".repeat(4096))], "").expect("made");
        let sealed: Vec<u8> = {
            let inner = vault.lock();
            Vault::db(&inner).unwrap().query_row("SELECT sealed_value FROM variables WHERE secret = ?1", [&id], |row| row.get(0)).unwrap()
        };
        let file = || std::fs::read(dir.join("vault/vault.db")).unwrap();
        let held = |bytes: &[u8]| bytes.windows(64).any(|window| window == &sealed[..64]);
        assert!(held(&file()), "the sealed value is in the file");
        vault.delete_secret(&id).expect("deleted");
        assert!(!held(&file()), "deleted, its sealed bytes are gone");
        // Opened again: wiped once, noted.
        drop(vault);
        let again = Vault::open(dir.join("vault"));
        let inner = again.lock();
        let noted: bool = Vault::db(&inner).unwrap().query_row("SELECT COUNT(*) > 0 FROM kept WHERE name = ?1", [WIPED], |row| row.get(0)).unwrap();
        assert!(noted);
        drop(inner);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_vault_opens_by_itself_unless_in_locked_mode() {
        let dir = temp();
        let first = vault(&dir);
        assert!(!first.exists());
        let recovery = first.create(Some("correct horse battery"), false).expect("create").expect("recovery");
        let id = first.create_secret("AWS", "Deploys.", &[value("AWS_SECRET_ACCESS_KEY", "wJalrXUtnFEMI/K7MDENG")], "").expect("secret");
        drop(first);

        let again = vault(&dir);
        assert!(again.is_open(), "the key on disk opens it");
        let (_, values) = again.reveal(&id, "Mac").expect("reveal");
        assert_eq!((values[0].variable.as_str(), values[0].value.as_str()), ("AWS_SECRET_ACCESS_KEY", "wJalrXUtnFEMI/K7MDENG"));
        again.set_locked_mode(true).expect("locked mode");
        assert!(!dir.join("vault/auto.key").exists());
        drop(again);

        let locked = vault(&dir);
        assert!(locked.exists() && !locked.is_open());
        assert!(matches!(locked.reveal(&id, "Mac"), Err(VaultError::Locked)));
        assert!(!unlock(&locked, "wrong horse battery"));
        assert!(unlock(&locked, &recovery.to_lowercase()), "the recovery key opens it too");
        assert!(locked.reveal(&id, "Mac").is_ok());
    }

    #[test]
    fn the_passphrase_and_recovery_key_can_be_replaced() {
        let dir = temp();
        let vault = vault(&dir);
        let old_recovery = vault.create(Some("first passphrase"), true).expect("create").expect("recovery");
        let (id, protectors) = vault.protectors().expect("protectors");
        let vmk = Vault::check(&id, &protectors, "first passphrase").expect("right");
        vault.set_passphrase(&vmk, "second passphrase").expect("change");
        assert!(vault.set_passphrase(&vmk, "short").is_err());
        let recovery = vault.new_recovery_key().expect("new recovery key");
        let (id, protectors) = vault.protectors().expect("protectors");
        assert!(Vault::check(&id, &protectors, "first passphrase").is_none());
        assert!(Vault::check(&id, &protectors, "second passphrase").is_some());
        assert!(Vault::check(&id, &protectors, &old_recovery).is_none(), "the old recovery key stops working");
        assert!(Vault::check(&id, &protectors, &recovery).is_some());
        assert!(matches!(vault.create(Some("again and again"), false), Err(VaultError::Exists)));
    }

    #[test]
    fn values_are_sealed_and_bound_to_their_secret() {
        let dir = temp();
        let vault = vault(&dir);
        vault.create(Some("correct horse battery"), true).expect("create").expect("recovery");
        let a = vault.create_secret("A", "", &[value("TOKEN", "aaaaaaaaaaaaaaaaaaaa")], "").expect("a");
        let b = vault.create_secret("B", "", &[value("TOKEN", "bbbbbbbbbbbbbbbbbbbb")], "").expect("b");
        let bytes = std::fs::read(dir.join("vault/vault.db")).expect("db");
        assert!(!bytes.windows(20).any(|window| window == b"aaaaaaaaaaaaaaaaaaaa"), "no value in the clear on disk");
        // Moving one secret's sealed value onto another's row doesn't open.
        {
            let inner = vault.lock();
            let db = inner.db.as_ref().expect("db");
            let sealed: Vec<u8> = db.query_row("SELECT sealed_value FROM variables WHERE secret = ?1", [&a], |row| row.get(0)).expect("a");
            db.execute("UPDATE variables SET sealed_value = ?2 WHERE secret = ?1", params![b, sealed]).expect("swap");
        }
        assert!(matches!(vault.reveal(&b, "Mac"), Err(VaultError::Invalid(_))));
        assert_eq!(vault.reveal(&a, "Mac").expect("a").1[0].value.as_str(), "aaaaaaaaaaaaaaaaaaaa");
        assert_eq!(
            format!("{:?}", vault.reveal(&a, "Mac").expect("a").1[0]),
            "Value { variable: \"TOKEN\", file: false, value: [hidden] }"
        );
    }

    #[test]
    fn grants_edited_outside_claktory_are_ignored() {
        let dir = temp();
        let vault = vault(&dir);
        vault.create(Some("correct horse battery"), true).expect("create").expect("recovery");
        let id = vault.create_secret("Cloudflare", "", &[value("CLOUDFLARE_API_TOKEN", "x".repeat(40).as_str())], "").expect("secret");
        vault.grant(&id, "builder", "").expect("grant");
        {
            let inner = vault.lock();
            let db = inner.db.as_ref().expect("db");
            db.execute("INSERT INTO grants (secret, agent, granted, mac) VALUES (?1, 'intruder', 1, x'00')", [&id]).expect("forge");
            db.execute("UPDATE grants SET granted = granted + 1 WHERE agent = 'builder'", []).expect("tamper");
        }
        vault.lock().state = None;
        let state = vault.state(|_, _| false);
        assert!(state.secrets[0].grants.is_empty(), "{:?}", state.secrets[0].grants);
        assert_eq!(state.secrets[0].warnings.len(), 2, "{:?}", state.secrets[0].warnings);
        vault.grant(&id, "builder", "").expect("grant again");
        for task in ["task-1", "task-2", "task-3"] {
            let used = factory_agent::SecretUse {
                agent: "builder".into(),
                secrets: vec!["Cloudflare".into()],
                command: "deploy".into(),
                task: Some(task.into()),
                started: now_ms() as u64,
            };
            vault.record_use(&used).expect("use");
        }
        // Of the tasks that got it, the ones still running.
        let state = vault.state(|agent, task| agent == "builder" && task != "task-2");
        assert_eq!(
            state.secrets[0].grants.iter().map(|grant| (grant.agent_id.as_str(), grant.running_tasks)).collect::<Vec<_>>(),
            [("builder", 2)]
        );
        assert!(matches!(vault.revoke(&id, "designer", ""), Err(VaultError::NotGranted { .. })));
        assert_eq!(vault.revoke("Cloudflare", "builder", "").expect("revoke by name"), "Cloudflare");
        let kinds: Vec<String> = vault.uses("", 0).expect("uses").into_iter().map(|row| row.kind).collect();
        assert_eq!(kinds, ["revoke", "use", "use", "use", "grant", "grant"]);
    }

    #[test]
    fn secrets_have_names_and_variables_agents_can_use() {
        let dir = temp();
        let vault = vault(&dir);
        assert!(matches!(vault.create_secret("A", "", &[value("X", "y")], ""), Err(VaultError::NoVault)));
        vault.create(Some("correct horse battery"), true).expect("create").expect("recovery");
        for (name, values) in [
            ("", vec![value("X", "y")]),
            ("A/B", vec![value("X", "y")]),
            ("A", vec![]),
            ("A", vec![value("1X", "y")]),
            ("A", vec![value("X", "y"), value("X", "z")]),
        ] {
            assert!(matches!(vault.create_secret(name, "", &values, ""), Err(VaultError::Invalid(_))), "{name} {values:?}");
        }
        let id =
            vault.create_secret("Resend", "Mail.", &[value("RESEND_API_KEY", "re_123456789abcdef"), value("PIN", "1234")], "").expect("ok");
        assert!(matches!(vault.create_secret("resend", "", &[value("X", "y")], ""), Err(VaultError::Invalid(_))), "names are unique");
        let state = vault.state(|_, _| false);
        let secret = &state.secrets[0];
        assert_eq!(
            secret.variables.iter().map(|variable| (variable.name.as_str(), variable.hint.as_str())).collect::<Vec<_>>(),
            [("RESEND_API_KEY", "cdef"), ("PIN", "")]
        );
        assert_eq!(secret.warnings, ["PIN is too short to hide in agents' output."]);
        assert_eq!(
            vault.update_secret(&id, "Resend mail", "Mail, again.", Some(&[value("RESEND_API_KEY", "re_new_value_here")])).expect("update"),
            "Resend"
        );
        let state = vault.state(|_, _| false);
        assert_eq!((state.secrets[0].name.as_str(), state.secrets[0].version, state.secrets[0].variables.len()), ("Resend mail", 2, 1));
        assert_eq!(vault.reveal(&id, "Mac").expect("reveal").1[0].value.as_str(), "re_new_value_here");
        vault.update_secret(&id, "Resend mail", "Only words.", None).expect("words only");
        assert_eq!(vault.reveal(&id, "Mac").expect("reveal").1[0].value.as_str(), "re_new_value_here");
        assert_eq!(vault.delete_secret(&id).expect("delete").0, "Resend mail");
        assert!(vault.state(|_, _| false).secrets.is_empty());
    }
}
