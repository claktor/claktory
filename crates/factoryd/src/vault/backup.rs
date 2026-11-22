//! Backing the vault up off this computer, and restoring it (damage
//! reduction §2): `factoryd vault backup --recipient R` writes the vault and
//! what opens it to standard output, encrypted to an age recipient, so a
//! nightly job never needs to know how the vault opens or read the home
//! itself; `factoryd vault restore FILE --identity ID --home DIR` puts it
//! back in a fresh home, for a restore or a drill.
//!
//! The file is age (age-encryption.org/v1), to an X25519 recipient
//! (`age1…`) or an SSH key (`ssh-ed25519 …`), holding a tar: `manifest.json`
//! first (what's in it and how the vault opens), then `vault/vault.db` (a
//! consistent copy, taken while Claktory runs), the vault folder's other
//! files (`auto.key`, the sign-ins' outline…) and, when the vault opens with
//! the Claktory's account at claktory.com, the `account/` files that sign in
//! there. So `age -d -i ID FILE | tar -t` lists it too. Restoring checks
//! every file against the manifest before writing anything.

use std::collections::BTreeMap;
use std::io::{self, BufReader, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::{Vault, crypto};

/// What the manifest says it is.
const KIND: &str = "claktory vault backup";
/// The version of the format this writes, and the newest it reads.
const FORMAT: u32 = 1;
const MANIFEST: &str = "manifest.json";
const VAULT_DB: &str = "vault.db";
/// The most a backup's files may hold, together.
const MOST: u64 = 1 << 30;

/// How the vault in a backup opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Opens {
    /// By itself, with the key kept beside it (`auto.key`).
    Key,
    /// With this Claktory's account at claktory.com (its half of the key is
    /// in `account/share.key`), or the recovery key.
    Account,
    /// Locked mode: only with its passphrase or recovery key.
    Locked,
}

impl Opens {
    fn said(self) -> &'static str {
        match self {
            Opens::Key => "It opens by itself, with its key on disk.",
            Opens::Account => {
                "It opens with this Claktory's account at claktory.com once Claktory starts in that home, or with the recovery key."
            }
            Opens::Locked => "It's in locked mode: it opens with its passphrase or recovery key.",
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    kind: String,
    format: u32,
    /// The Claktory that made it.
    version: String,
    created: i64,
    vault_id: String,
    opens: Opens,
    secrets: u64,
    files: Vec<Entry>,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct Entry {
    path: String,
    size: u64,
    sha256: String,
}

/// A file's contents, wiped when dropped.
type Bytes = Zeroizing<Vec<u8>>;

/// A vault's consistent copy: its bytes, id, how it opens and how many secrets it holds.
type Snapshot = (Bytes, String, Opens, u64);

/// What a backup holds, as it was made.
#[derive(Debug)]
pub struct Made {
    pub vault_id: String,
    pub opens: Opens,
    pub secrets: u64,
    pub files: usize,
}

/// A recipient: `age1…`, or an SSH public key's line.
pub fn recipient(text: &str) -> Result<Box<dyn age::Recipient + Send>, String> {
    let text = text.trim();
    if text.starts_with("age1") {
        return age::x25519::Recipient::from_str(text)
            .map(|recipient| Box::new(recipient) as _)
            .map_err(|error| format!("{text} isn't an age recipient: {error}."));
    }
    if text.starts_with("ssh-") {
        return age::ssh::Recipient::from_str(text)
            .map(|recipient| Box::new(recipient) as _)
            .map_err(|error| format!("That SSH key can't be a recipient: {error:?}."));
    }
    Err(format!("\"{text}\" isn't a recipient: give an age1… key or an SSH public key (ssh-ed25519 …)."))
}

/// The identities in `text`: an age identity file (AGE-SECRET-KEY-1…) or
/// an unencrypted OpenSSH private key.
pub fn identities(text: &str) -> Result<Vec<Box<dyn age::Identity>>, String> {
    if text.contains("-----BEGIN") {
        let identity =
            age::ssh::Identity::from_buffer(BufReader::new(text.as_bytes()), None).map_err(|_| "That SSH key can't be read.".to_owned())?;
        return match identity {
            age::ssh::Identity::Unencrypted(_) => Ok(vec![Box::new(identity)]),
            age::ssh::Identity::Encrypted(_) => {
                Err("That SSH key has a passphrase: remove it from a copy first (ssh-keygen -p), and wipe the copy after.".to_owned())
            }
            age::ssh::Identity::Unsupported(_) => Err("age can't use that kind of SSH key: use an ssh-ed25519 key.".to_owned()),
        };
    }
    let file =
        age::IdentityFile::from_buffer(BufReader::new(text.as_bytes())).map_err(|error| format!("The identity can't be read: {error}."))?;
    let identities = file.into_identities().map_err(|error| format!("The identity can't be used: {error}."))?;
    if identities.is_empty() {
        return Err("The identity file holds no identity (AGE-SECRET-KEY-1…).".to_owned());
    }
    Ok(identities.into_iter().map(|identity| identity as Box<dyn age::Identity>).collect())
}

fn digest(bytes: &[u8]) -> String {
    crypto::hex(&Sha256::digest(bytes))
}

/// A file's name that can go in a backup: a plain name in its folder.
fn plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

/// The regular files directly in `dir`, but `skip`, by name.
fn files_in(dir: &Path, skip: impl Fn(&str) -> bool) -> Result<Vec<(String, Bytes)>, String> {
    let mut files = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(files),
        Err(error) => return Err(format!("{} can't be read: {error}", dir.display())),
    };
    for entry in entries {
        let entry = entry.map_err(|error| format!("{} can't be read: {error}", dir.display()))?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
        let is_file = entry.file_type().is_ok_and(|kind| kind.is_file());
        if !is_file || !plain_name(&name) || name.starts_with('.') || skip(&name) {
            continue;
        }
        let bytes = std::fs::read(entry.path()).map_err(|error| format!("{} can't be read: {error}", entry.path().display()))?;
        files.push((name, Zeroizing::new(bytes)));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

/// A consistent copy of `db`, made while Claktory may be writing it, and
/// what it says: its id, how it opens and how many secrets it holds.
fn snapshot(db: &Path) -> Result<Snapshot, String> {
    if !db.exists() {
        return Err(format!("There's no vault in {} to back up.", db.parent().unwrap_or(db).display()));
    }
    let failed = |error: rusqlite::Error| format!("The vault can't be read: {error}.");
    let scratch = std::env::temp_dir().join(format!("claktory-vault-backup-{}", crypto::hex(&crypto::random::<8>())));
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(&scratch).map_err(|error| format!("{}: {error}", scratch.display()))?;
    }
    let copy = scratch.join(VAULT_DB);
    let result = (|| {
        let source = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(failed)?;
        source.busy_timeout(std::time::Duration::from_secs(10)).map_err(failed)?;
        source.execute("VACUUM INTO ?1", [copy.to_string_lossy()]).map_err(failed)?;
        drop(source);
        let snapshot = Connection::open_with_flags(&copy, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(failed)?;
        let (vault_id, locked_mode): (String, bool) =
            snapshot.query_row("SELECT vault_id, locked_mode FROM vault WHERE id = 1", [], crate::store::row2).map_err(failed)?;
        let account: bool =
            snapshot.query_row("SELECT COUNT(*) > 0 FROM protectors WHERE kind LIKE 'account:%'", [], |row| row.get(0)).map_err(failed)?;
        let secrets: i64 = snapshot.query_row("SELECT COUNT(*) FROM secrets", [], |row| row.get(0)).map_err(failed)?;
        let secrets = u64::try_from(secrets).unwrap_or(0);
        drop(snapshot);
        let opens = if locked_mode {
            Opens::Locked
        } else if account {
            Opens::Account
        } else {
            Opens::Key
        };
        let bytes = Zeroizing::new(std::fs::read(&copy).map_err(|error| format!("The vault's copy can't be read: {error}."))?);
        Ok((bytes, vault_id, opens, secrets))
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

/// Writes a backup of the vault in `home` to `out`, encrypted to `recipients`.
pub fn backup(home: &Path, recipients: &[Box<dyn age::Recipient + Send>], out: impl Write) -> Result<Made, String> {
    if recipients.is_empty() {
        return Err("Say whom to encrypt the backup to: --recipient age1… (or an SSH public key).".to_owned());
    }
    let dir = home.join("vault");
    let (db, vault_id, opens, secrets) = snapshot(&dir.join(VAULT_DB))?;
    // vault.db's own files (its journal) are in the copy already.
    let mut files: Vec<(String, Zeroizing<Vec<u8>>)> = vec![(format!("vault/{VAULT_DB}"), db)];
    let others = files_in(&dir, |name| name == VAULT_DB || name.starts_with("vault.db-"))?;
    files.extend(others.into_iter().map(|(name, bytes)| (format!("vault/{name}"), bytes)));
    if opens == Opens::Account {
        let account = files_in(&home.join("account"), |_| false)?;
        if !account.iter().any(|(name, _)| name == "share.key") {
            return Err(
                "The vault opens with the account at claktory.com, but this computer's half of its key (account/share.key) isn't there."
                    .to_owned(),
            );
        }
        files.extend(account.into_iter().map(|(name, bytes)| (format!("account/{name}"), bytes)));
    }
    let manifest = Manifest {
        kind: KIND.to_owned(),
        format: FORMAT,
        version: crate::update::VERSION.to_owned(),
        created: crate::transcript::now_ms(),
        vault_id: vault_id.clone(),
        opens,
        secrets,
        files: files.iter().map(|(path, bytes)| Entry { path: path.clone(), size: bytes.len() as u64, sha256: digest(bytes) }).collect(),
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?;

    let encryptor = age::Encryptor::with_recipients(recipients.iter().map(|recipient| recipient.as_ref() as &dyn age::Recipient))
        .map_err(|error| format!("The backup can't be encrypted to that: {error}."))?;
    let written = |error: io::Error| format!("The backup couldn't be written: {error}.");
    let stream = encryptor.wrap_output(out).map_err(written)?;
    let mut tar = tar::Builder::new(stream);
    let mtime = u64::try_from(manifest.created / 1000).unwrap_or(0);
    let mut append = |path: &str, bytes: &[u8]| {
        let mut header = tar::Header::new_ustar();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o600);
        header.set_mtime(mtime);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        tar.append_data(&mut header, path, bytes)
    };
    append(MANIFEST, &manifest_bytes).map_err(written)?;
    for (path, bytes) in &files {
        append(path, bytes).map_err(written)?;
    }
    let stream = tar.into_inner().map_err(written)?;
    let mut out = stream.finish().map_err(written)?;
    out.flush().map_err(written)?;
    Ok(Made { vault_id, opens, secrets, files: files.len() })
}

/// What a restore put back.
#[derive(Debug)]
pub struct Restored {
    pub vault_id: String,
    pub opens: Opens,
    pub secrets: u64,
    /// Whether it opened (only one that opens by itself is tried).
    pub opened: bool,
    /// When it was backed up.
    pub created: i64,
}

/// Reads a backup's files, checked against its manifest: nothing is
/// written until all of it reads and checks.
fn read_backup(file: impl Read, identities: &[Box<dyn age::Identity>]) -> Result<(Manifest, BTreeMap<String, Bytes>), String> {
    let decryptor = age::Decryptor::new_buffered(BufReader::new(file)).map_err(|error| match error {
        age::DecryptError::InvalidHeader | age::DecryptError::UnknownFormat => "That isn't an encrypted vault backup.".to_owned(),
        error => format!("The backup can't be read: {error}."),
    })?;
    let reader = decryptor.decrypt(identities.iter().map(|identity| identity.as_ref())).map_err(|error| match error {
        age::DecryptError::NoMatchingKeys => "That identity doesn't open this backup.".to_owned(),
        error => format!("The backup can't be opened: {error}."),
    })?;
    let damaged = |why: String| format!("The backup is damaged or incomplete ({why}). Use another copy.");
    let mut archive = tar::Archive::new(reader);
    let mut files: BTreeMap<String, Zeroizing<Vec<u8>>> = BTreeMap::new();
    let mut total = 0u64;
    for entry in archive.entries().map_err(|error| damaged(error.to_string()))? {
        let mut entry = entry.map_err(|error| damaged(error.to_string()))?;
        if entry.header().entry_type() != tar::EntryType::Regular {
            return Err(damaged("an entry that isn't a file".to_owned()));
        }
        let path = String::from_utf8(entry.path_bytes().into_owned()).map_err(|_| damaged("a name that isn't text".to_owned()))?;
        let allowed = path == MANIFEST
            || path.split_once('/').is_some_and(|(folder, name)| (folder == "vault" || folder == "account") && plain_name(name));
        if !allowed {
            return Err(damaged(format!("{path} has no place in a vault backup")));
        }
        let size = entry.size();
        total = total.saturating_add(size);
        if total > MOST {
            return Err(damaged("it's bigger than a vault backup can be".to_owned()));
        }
        let mut bytes = Zeroizing::new(Vec::with_capacity(usize::try_from(size).unwrap_or(0)));
        entry.read_to_end(&mut bytes).map_err(|error| damaged(error.to_string()))?;
        if files.insert(path.clone(), bytes).is_some() {
            return Err(damaged(format!("{path} is there twice")));
        }
    }
    // To its end, so its last part is checked too.
    io::copy(&mut archive.into_inner(), &mut io::sink()).map_err(|error| damaged(error.to_string()))?;
    let manifest_bytes = files.remove(MANIFEST).ok_or_else(|| damaged("no manifest".to_owned()))?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes).map_err(|error| damaged(format!("its manifest: {error}")))?;
    if manifest.kind != KIND {
        return Err("That isn't a vault backup.".to_owned());
    }
    if manifest.format > FORMAT {
        return Err(format!("A newer Claktory made this backup (format {}). Restore it with that version or later.", manifest.format));
    }
    let listed: BTreeMap<&str, &Entry> = manifest.files.iter().map(|entry| (entry.path.as_str(), entry)).collect();
    if listed.len() != files.len() || files.keys().any(|path| !listed.contains_key(path.as_str())) {
        return Err(damaged("its files aren't the ones its manifest lists".to_owned()));
    }
    for (path, bytes) in &files {
        let entry = listed[path.as_str()];
        if entry.size != bytes.len() as u64 || entry.sha256 != digest(bytes) {
            return Err(damaged(format!("{path} isn't as it was backed up")));
        }
    }
    if !files.contains_key(&format!("vault/{VAULT_DB}")) {
        return Err(damaged("no vault.db".to_owned()));
    }
    Ok((manifest, files))
}

fn private_dir(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let failed = |error: io::Error| format!("{} can't be made: {error}", dir.display());
    if dir.is_dir() {
        return std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(failed);
    }
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(failed)
}

fn holds_files(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some())
}

/// Restores the backup in `file` into `home`, a fresh home: one with a
/// vault or an account already is refused, so nothing is ever overwritten.
pub fn restore(file: impl Read, identities: &[Box<dyn age::Identity>], home: &Path) -> Result<Restored, String> {
    for (folder, called) in [("vault", "a vault"), ("account", "an account")] {
        if holds_files(&home.join(folder)) {
            return Err(format!(
                "{} already has {called} folder: restore into a fresh home, one with no vault or account, so nothing is overwritten.",
                home.display()
            ));
        }
    }
    let (manifest, files) = read_backup(file, identities)?;
    if !home.exists() {
        private_dir(home)?;
    }
    // Put together beside them first, and checked there: only a restore
    // that holds goes into place, so a failed one leaves the home as it was.
    let staging = home.join(format!(".vault-restore-{}", crypto::hex(&crypto::random::<8>())));
    let result = (|| {
        private_dir(&staging)?;
        let folders: Vec<&str> =
            ["vault", "account"].into_iter().filter(|folder| files.keys().any(|path| path.starts_with(&format!("{folder}/")))).collect();
        for folder in &folders {
            private_dir(&staging.join(folder))?;
        }
        for (path, bytes) in &files {
            let target = staging.join(path);
            let mut out = {
                use std::os::unix::fs::OpenOptionsExt;
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&target)
                    .map_err(|error| format!("{} can't be written: {error}", target.display()))?
            };
            out.write_all(bytes)
                .and_then(|()| out.sync_all())
                .map_err(|error| format!("{} can't be written: {error}", target.display()))?;
        }
        let opened = Vault::open(staging.join("vault")).is_open();
        if manifest.opens == Opens::Key && !opened {
            return Err("The backup's key on disk doesn't open its vault: it's damaged or wasn't made by Claktory. Nothing was restored; use another copy.".to_owned());
        }
        for folder in &folders {
            // Over an empty folder of that name, if there is one.
            std::fs::rename(staging.join(folder), home.join(folder))
                .map_err(|error| format!("{} can't be put in place: {error}", home.join(folder).display()))?;
        }
        Ok(opened)
    })();
    let _ = std::fs::remove_dir_all(&staging);
    let opened = result?;
    Ok(Restored { vault_id: manifest.vault_id, opens: manifest.opens, secrets: manifest.secrets, opened, created: manifest.created })
}

fn usage() -> ExitCode {
    eprintln!(
        "Usage:\n  factoryd [--dev] vault backup --recipient RECIPIENT [--recipient …] > FILE\n  factoryd vault restore FILE --identity IDENTITY_FILE --home DIR [--json]\n\nA recipient is an age X25519 key (age1…) or an SSH public key (ssh-ed25519 …); an identity file holds an age identity (AGE-SECRET-KEY-1…) or an unencrypted SSH private key."
    );
    ExitCode::from(2)
}

/// `factoryd vault …`.
pub fn main(dev: bool, args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("backup") => backup_main(dev, &args[1..]),
        Some("restore") => restore_main(&args[1..]),
        _ => usage(),
    }
}

fn backup_main(dev: bool, args: &[String]) -> ExitCode {
    let mut recipients = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match (arg.as_str(), args.next()) {
            ("--recipient", Some(text)) => match recipient(text) {
                Ok(recipient) => recipients.push(recipient),
                Err(error) => {
                    eprintln!("{error}");
                    return ExitCode::from(2);
                }
            },
            _ => return usage(),
        }
    }
    if recipients.is_empty() {
        return usage();
    }
    let stdout = io::stdout();
    if stdout.is_terminal() {
        eprintln!("The backup goes to standard output: send it to a file, such as factoryd vault backup --recipient … > vault.tar.age");
        return ExitCode::from(2);
    }
    let Some(place) = crate::move_export::Place::this_user(dev) else {
        eprintln!("HOME isn't set.");
        return ExitCode::from(2);
    };
    match backup(place.home.root(), &recipients, stdout.lock()) {
        Ok(made) => {
            let secrets = if made.secrets == 1 { "1 secret".to_owned() } else { format!("{} secrets", made.secrets) };
            eprintln!("Backed up the vault ({secrets}, {} files). {}", made.files, made.opens.said());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(3)
        }
    }
}

fn restore_main(args: &[String]) -> ExitCode {
    let (mut file, mut identity, mut home, mut json): (Option<PathBuf>, Option<PathBuf>, Option<PathBuf>, bool) = (None, None, None, false);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--identity" => match args.next() {
                Some(path) => identity = Some(path.into()),
                None => return usage(),
            },
            "--home" => match args.next() {
                Some(path) => home = Some(path.into()),
                None => return usage(),
            },
            path if file.is_none() && !path.starts_with("--") => file = Some(path.into()),
            _ => return usage(),
        }
    }
    let (Some(file), Some(identity), Some(home)) = (file, identity, home) else { return usage() };
    let say = |ok: bool, text: &str, extra: serde_json::Value| {
        if json {
            let mut line = serde_json::json!({"ok": ok, "message": text});
            if let (Some(line), Some(extra)) = (line.as_object_mut(), extra.as_object()) {
                line.extend(extra.clone());
            }
            println!("{line}");
        } else if ok {
            println!("{text}");
        } else {
            eprintln!("{text}");
        }
        ExitCode::from(if ok { 0 } else { 3 })
    };
    let restored = std::fs::read_to_string(&identity)
        .map(Zeroizing::new)
        .map_err(|error| format!("{} can't be read: {error}", identity.display()))
        .and_then(|text| identities(&text))
        .and_then(|identities| {
            let input = std::fs::File::open(&file).map_err(|error| format!("{} can't be read: {error}", file.display()))?;
            restore(input, &identities, &home)
        });
    match restored {
        Ok(restored) => {
            let secrets = if restored.secrets == 1 { "1 secret".to_owned() } else { format!("{} secrets", restored.secrets) };
            let opened = if restored.opened { " It opened: its secrets are all there." } else { "" };
            let text = format!("Restored the vault ({secrets}) into {}. {}{opened}", home.display(), restored.opens.said());
            say(
                true,
                &text,
                serde_json::json!({
                    "vault_id": restored.vault_id,
                    "opens": restored.opens,
                    "secrets": restored.secrets,
                    "opened": restored.opened,
                    "created": restored.created,
                }),
            )
        }
        Err(error) => say(false, &error, serde_json::json!({})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::Value;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vault-backup-test-{}", crypto::hex(&crypto::random::<8>())));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// A home whose vault holds one secret, opened as `locked` says.
    fn home_with_a_secret(locked: bool) -> (PathBuf, Option<String>) {
        let home = temp();
        let vault = Vault::open(home.join("vault"));
        vault.set_passphrase_params("argon2id m=1024 t=1 p=1");
        let recovery = vault.create(locked.then_some("a long passphrase"), locked).expect("vault");
        let value =
            Value { variable: "TOKEN".to_owned(), value: Zeroizing::new("s3cr3t-value-0123".to_owned()), file: false, binary: false };
        vault.create_secret("Cloudflare", "", &[value], "").expect("secret");
        (home, recovery)
    }

    fn x25519() -> (age::x25519::Identity, Vec<Box<dyn age::Recipient + Send>>) {
        let identity = age::x25519::Identity::generate();
        let recipient = recipient(&identity.to_public().to_string()).expect("recipient");
        (identity, vec![recipient])
    }

    fn the(identity: age::x25519::Identity) -> Vec<Box<dyn age::Identity>> {
        vec![Box::new(identity)]
    }

    fn revealed(home: &Path) -> String {
        let vault = Vault::open(home.join("vault"));
        assert!(vault.is_open(), "the restored vault opens");
        let id: String = {
            let db = Connection::open(home.join("vault/vault.db")).expect("db");
            db.query_row("SELECT id FROM secrets", [], |row| row.get(0)).expect("a secret")
        };
        let (_, values) = vault.reveal(&id, "test").expect("revealed");
        values[0].value.to_string()
    }

    #[test]
    fn a_vault_that_opens_by_itself_comes_back_whole() {
        let (home, _) = home_with_a_secret(false);
        let (identity, recipients) = x25519();
        let mut file = Vec::new();
        let made = backup(&home, &recipients, &mut file).expect("backed up");
        assert_eq!((made.opens, made.secrets), (Opens::Key, 1));
        assert!(!file.windows(17).any(|window| window == b"s3cr3t-value-0123"), "nothing in the clear");

        let fresh = temp().join("home");
        let restored = restore(file.as_slice(), &the(identity), &fresh).expect("restored");
        assert!(restored.opened && restored.opens == Opens::Key && restored.secrets == 1);
        assert_eq!(revealed(&fresh), "s3cr3t-value-0123");
        use std::os::unix::fs::PermissionsExt;
        for path in ["vault", "vault/vault.db", "vault/auto.key"] {
            let mode = std::fs::metadata(fresh.join(path)).expect(path).permissions().mode() & 0o777;
            assert_eq!(mode, if path == "vault" { 0o700 } else { 0o600 }, "{path}");
        }
    }

    #[test]
    fn a_locked_vault_comes_back_locked_and_opens_with_its_passphrase() {
        let (home, _) = home_with_a_secret(true);
        let (identity, recipients) = x25519();
        let mut file = Vec::new();
        assert_eq!(backup(&home, &recipients, &mut file).expect("backed up").opens, Opens::Locked);
        let fresh = temp();
        let restored = restore(file.as_slice(), &the(identity), &fresh).expect("restored");
        assert!(!restored.opened && restored.opens == Opens::Locked);
        assert!(!fresh.join("vault/auto.key").exists());
        let vault = Vault::open(fresh.join("vault"));
        let (id, protectors) = vault.protectors().expect("protectors");
        assert!(Vault::check(&id, &protectors, "a long passphrase").is_some(), "its passphrase opens it");
    }

    #[test]
    fn a_vault_that_opens_with_the_account_brings_its_half_of_the_key() {
        let (home, _) = home_with_a_secret(false);
        let vault = Vault::open(home.join("vault"));
        vault.protect_with_account("acct", "{}", &[7u8; 64]).expect("account");
        std::fs::create_dir_all(home.join("account")).expect("account dir");
        for name in ["account.json", "claktory.key", "share.key"] {
            std::fs::write(home.join("account").join(name), name).expect(name);
        }
        let (identity, recipients) = x25519();
        let mut file = Vec::new();
        assert_eq!(backup(&home, &recipients, &mut file).expect("backed up").opens, Opens::Account);
        let fresh = temp();
        let restored = restore(file.as_slice(), &the(identity), &fresh).expect("restored");
        assert!(!restored.opened && restored.opens == Opens::Account);
        assert_eq!(std::fs::read_to_string(fresh.join("account/share.key")).expect("share"), "share.key");
        assert!(!fresh.join("vault/auto.key").exists(), "the key on disk went at sign-in");

        // Its half of the key missing, it says so rather than back up what won't open.
        std::fs::remove_file(home.join("account/share.key")).expect("removed");
        assert!(backup(&home, &recipients, &mut Vec::new()).expect_err("refused").contains("share.key"));
    }

    #[test]
    fn an_ssh_key_backs_up_and_restores_too() {
        let (home, _) = home_with_a_secret(false);
        let made = crate::vault::generate::make("ssh-ed25519", "Backup decryption key").expect("key");
        let recipients = vec![recipient(&made.public).expect("an ssh recipient")];
        let mut file = Vec::new();
        backup(&home, &recipients, &mut file).expect("backed up");
        let fresh = temp();
        restore(file.as_slice(), &identities(&made.private).expect("an ssh identity"), &fresh).expect("restored");
        assert_eq!(revealed(&fresh), "s3cr3t-value-0123");
    }

    #[test]
    fn a_wrong_identity_or_a_damaged_file_restores_nothing() {
        let (home, _) = home_with_a_secret(false);
        let (identity, recipients) = x25519();
        let mut file = Vec::new();
        backup(&home, &recipients, &mut file).expect("backed up");

        let fresh = temp();
        let other = age::x25519::Identity::generate();
        assert_eq!(restore(file.as_slice(), &the(other), &fresh).expect_err("refused"), "That identity doesn't open this backup.");
        let mut damaged = file.clone();
        let at = damaged.len() - 40;
        damaged[at] ^= 1;
        assert!(restore(damaged.as_slice(), &the(identity.clone()), &fresh).is_err());
        let cut = &file[..file.len() - 100];
        assert!(restore(cut, &the(identity.clone()), &fresh).is_err());
        assert!(!fresh.join("vault").exists(), "nothing written");
        assert!(identities("not an identity").is_err());

        // A home with a vault is never written over.
        let refused = restore(file.as_slice(), &the(identity), &home).expect_err("refused");
        assert!(refused.contains("fresh home"), "{refused}");
    }

    #[test]
    fn a_key_that_doesnt_open_its_vault_restores_nothing() {
        let (home, _) = home_with_a_secret(false);
        std::fs::write(home.join("vault/auto.key"), crypto::hex(&crypto::random::<32>())).expect("another key");
        let (identity, recipients) = x25519();
        let mut file = Vec::new();
        backup(&home, &recipients, &mut file).expect("backed up");
        let fresh = temp();
        let refused = restore(file.as_slice(), &the(identity), &fresh).expect_err("refused");
        assert!(refused.contains("Nothing was restored"), "{refused}");
        assert_eq!(std::fs::read_dir(&fresh).expect("home").count(), 0, "the home is as it was");
    }

    #[test]
    fn only_a_vault_backup_and_its_files_are_restored() {
        let (identity, recipients) = x25519();
        let encrypt = |entries: &[(&str, &[u8])]| {
            let encryptor = age::Encryptor::with_recipients(recipients.iter().map(|recipient| recipient.as_ref() as &dyn age::Recipient))
                .expect("encryptor");
            let mut tar = tar::Builder::new(encryptor.wrap_output(Vec::new()).expect("stream"));
            for (path, bytes) in entries {
                // Named as it comes, `..` and all: tar's own checks would refuse it.
                let mut header = tar::Header::new_old();
                header.as_old_mut().name[..path.len()].copy_from_slice(path.as_bytes());
                header.set_size(bytes.len() as u64);
                header.set_mode(0o600);
                header.set_cksum();
                tar.append(&header, *bytes).expect("entry");
            }
            tar.into_inner().expect("tar").finish().expect("age")
        };
        let fresh = temp();
        let escape = encrypt(&[("manifest.json", b"{}"), ("vault/../../evil", b"x")]);
        assert!(restore(escape.as_slice(), &the(identity.clone()), &fresh).expect_err("refused").contains("no place"));
        let unlisted = encrypt(&[("vault/vault.db", b"x")]);
        assert!(restore(unlisted.as_slice(), &the(identity), &fresh).expect_err("refused").contains("no manifest"));
        assert!(!fresh.join("vault").exists() && !fresh.parent().expect("parent").join("evil").exists());
    }
}
