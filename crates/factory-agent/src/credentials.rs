use std::fs;
#[cfg(unix)]
use std::fs::{DirBuilder, OpenOptions};
use std::future::Future;
#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, fchown};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::Mutex;

use crate::errors::{AgentError, AgentFailure, Error};

fn corrupt(file: &Path) -> AgentFailure {
    AgentFailure::CorruptCredentials { file: file.display().to_string() }
}

/// The credentials document in `file` (`auth.json`): by provider id; empty
/// when there's no file.
pub fn read_document(file: &Path) -> Result<Map<String, Value>, Error> {
    Ok(read_json(file)?.and_then(|value| value.as_object().cloned()).unwrap_or_default())
}

/// Writes the credentials document to `file`, as `auth.json` is written:
/// 0600, through a temp file, keeping the file's owner.
pub fn write_document(file: &Path, all: &Map<String, Value>) -> Result<(), Error> {
    write_json(file, &Value::Object(all.clone()))
}

fn read_json(file: &Path) -> Result<Option<Value>, Error> {
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    serde_json::from_str(&text).map(Some).map_err(|cause| AgentError::with_cause(corrupt(file), cause).into())
}

/// Written through a temp file so an interrupted write cannot truncate the session; a rewrite keeps the file's owner and group.
/// On Windows a private file (this user and SYSTEM), replaced whole the
/// same way; its files have no owner and group to keep.
#[cfg(windows)]
fn write_json(file: &Path, data: &Value) -> Result<(), Error> {
    if let Some(parent) = file.parent() {
        crate::windows::private::create_dir_all(parent)?;
    }
    let text = format!("{}\n", serde_json::to_string_pretty(data)?);
    Ok(crate::windows::private::write(file, text.as_bytes())?)
}

#[cfg(unix)]
fn write_json(file: &Path, data: &Value) -> Result<(), Error> {
    if let Some(parent) = file.parent() {
        DirBuilder::new().recursive(true).mode(0o700).create(parent)?;
    }
    let owner = match fs::metadata(file) {
        Ok(metadata) => Some((metadata.uid(), metadata.gid())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let text = format!("{}\n", serde_json::to_string_pretty(data)?);
    let temp = PathBuf::from(format!("{}.{}.tmp", file.display(), std::process::id()));
    let mut handle = OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&temp)?;
    let written = owner.map_or(Ok(()), |(uid, gid)| fchown(&handle, Some(uid), Some(gid))).and_then(|()| handle.write_all(text.as_bytes()));
    drop(handle);
    written.and_then(|()| fs::rename(&temp, file)).map_err(|error| {
        let _ = fs::remove_file(&temp);
        error.into()
    })
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum Credential {
    ApiKey {
        key: String,
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    Oauth {
        access: String,
        refresh: String,
        expires: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account_id: Option<String>,
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}

impl Credential {
    pub fn kind(&self) -> &'static str {
        match self {
            Credential::ApiKey { .. } => "api_key",
            Credential::Oauth { .. } => "oauth",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialInfo {
    pub provider_id: String,
    pub kind: String,
}

// Factory's own fields on a stored credential. A provider's first account is
// stored where its one credential always was, so a Factory from before
// several accounts (rolled back to, say) still reads and refreshes it, and
// keeps these fields, the rest of the accounts included, as ones it doesn't
// model; the others follow it in `factoryAccounts`.
const ACCOUNT_ID: &str = "factoryAccount";
const ACCOUNT_LABEL: &str = "factoryLabel";
const ACCOUNT_IDENTITY: &str = "factoryIdentity";
const EXHAUSTED_UNTIL: &str = "factoryExhaustedUntil";
const FABLE_EXHAUSTED_UNTIL: &str = "factoryFableExhaustedUntil";
const OPUS_EXHAUSTED_UNTIL: &str = "factoryOpusExhaustedUntil";
const SONNET_EXHAUSTED_UNTIL: &str = "factorySonnetExhaustedUntil";
const OUT_OF_CREDIT: &str = "factoryOutOfCredit";
const REFUSED_SINCE: &str = "factoryRefusedSince";
const MORE_ACCOUNTS: &str = "factoryAccounts";

/// One account signed in to a provider: its credential and what Factory
/// knows of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Account {
    /// Made by Factory, stable while it's signed in. A credential stored
    /// before accounts had ids is the provider's first, "1".
    pub id: String,
    /// Who it is, as the provider says (an email); empty until known.
    pub label: String,
    /// What tells it apart from the provider's other accounts (the
    /// provider's user id, say), so signing in to it again replaces it;
    /// empty when unknown.
    pub identity: String,
    /// It reached a usage limit: skipped until then, in ms since 1970; 0
    /// when it has capacity. Models that draw on the normal limits; Fable
    /// has `fable_exhausted_until`.
    pub exhausted_until: u64,
    /// As `exhausted_until`, for Anthropic's Fable models, whose usage
    /// counts against a weekly limit of their own: at the normal weekly
    /// limit the account can still run Fable, and the other way round.
    pub fable_exhausted_until: u64,
    /// Anthropic's weekly limit of Opus models alone reached, until then
    /// (ms since 1970); 0 when it hasn't. Opus requests wait for it and
    /// for `exhausted_until`; other models carry on.
    pub opus_exhausted_until: u64,
    /// As `opus_exhausted_until`, for Sonnet models' weekly limit.
    pub sonnet_exhausted_until: u64,
    /// An API key that ran out of credit: since when, in ms since 1970; 0
    /// when it has credit as far as Factory knows.
    pub out_of_credit_since: u64,
    /// The provider refused to refresh its sign-in: since when, in ms since
    /// 1970; 0 when it hasn't. Skipped until it's signed in to again, which
    /// makes it anew.
    pub refused_since: u64,
    pub credential: Credential,
}

impl Account {
    /// A newly signed-in account, with an id of its own.
    pub fn new(credential: Credential) -> Account {
        let mut id = uuid::Uuid::new_v4().simple().to_string();
        id.truncate(8);
        Account {
            id,
            label: String::new(),
            identity: String::new(),
            exhausted_until: 0,
            fable_exhausted_until: 0,
            opus_exhausted_until: 0,
            sonnet_exhausted_until: 0,
            out_of_credit_since: 0,
            refused_since: 0,
            credential,
        }
    }
}

fn account_from(value: &Value, position: usize) -> Result<Account, Error> {
    let mut object = value.as_object().cloned().unwrap_or_default();
    object.remove(MORE_ACCOUNTS);
    let mut text = |key: &str| object.remove(key).and_then(|value| value.as_str().map(str::to_owned)).unwrap_or_default();
    let (id, label, identity) = (text(ACCOUNT_ID), text(ACCOUNT_LABEL), text(ACCOUNT_IDENTITY));
    let exhausted_until = object.remove(EXHAUSTED_UNTIL).and_then(|value| value.as_u64()).unwrap_or(0);
    let fable_exhausted_until = object.remove(FABLE_EXHAUSTED_UNTIL).and_then(|value| value.as_u64()).unwrap_or(0);
    let opus_exhausted_until = object.remove(OPUS_EXHAUSTED_UNTIL).and_then(|value| value.as_u64()).unwrap_or(0);
    let sonnet_exhausted_until = object.remove(SONNET_EXHAUSTED_UNTIL).and_then(|value| value.as_u64()).unwrap_or(0);
    let out_of_credit_since = object.remove(OUT_OF_CREDIT).and_then(|value| value.as_u64()).unwrap_or(0);
    let refused_since = object.remove(REFUSED_SINCE).and_then(|value| value.as_u64()).unwrap_or(0);
    let credential =
        if value.is_object() { serde_json::from_value(Value::Object(object))? } else { serde_json::from_value(value.clone())? };
    let id = if id.is_empty() { (position + 1).to_string() } else { id };
    Ok(Account {
        id,
        label,
        identity,
        exhausted_until,
        fable_exhausted_until,
        opus_exhausted_until,
        sonnet_exhausted_until,
        out_of_credit_since,
        refused_since,
        credential,
    })
}

fn account_value(account: &Account) -> Result<Value, Error> {
    let mut value = serde_json::to_value(&account.credential)?;
    if let Some(object) = value.as_object_mut() {
        object.insert(ACCOUNT_ID.into(), Value::from(account.id.clone()));
        if !account.label.is_empty() {
            object.insert(ACCOUNT_LABEL.into(), Value::from(account.label.clone()));
        }
        if !account.identity.is_empty() {
            object.insert(ACCOUNT_IDENTITY.into(), Value::from(account.identity.clone()));
        }
        if account.exhausted_until != 0 {
            object.insert(EXHAUSTED_UNTIL.into(), Value::from(account.exhausted_until));
        }
        if account.fable_exhausted_until != 0 {
            object.insert(FABLE_EXHAUSTED_UNTIL.into(), Value::from(account.fable_exhausted_until));
        }
        if account.opus_exhausted_until != 0 {
            object.insert(OPUS_EXHAUSTED_UNTIL.into(), Value::from(account.opus_exhausted_until));
        }
        if account.sonnet_exhausted_until != 0 {
            object.insert(SONNET_EXHAUSTED_UNTIL.into(), Value::from(account.sonnet_exhausted_until));
        }
        if account.out_of_credit_since != 0 {
            object.insert(OUT_OF_CREDIT.into(), Value::from(account.out_of_credit_since));
        }
        if account.refused_since != 0 {
            object.insert(REFUSED_SINCE.into(), Value::from(account.refused_since));
        }
    }
    Ok(value)
}

/// A provider's accounts as stored: the first, then its `factoryAccounts`.
fn parse_accounts(value: Option<&Value>) -> Result<Vec<Account>, Error> {
    let Some(value) = value else { return Ok(Vec::new()) };
    let mut accounts = vec![account_from(value, 0)?];
    for (position, more) in value.get(MORE_ACCOUNTS).and_then(Value::as_array).into_iter().flatten().enumerate() {
        let account = account_from(more, position + 1)?;
        // Ids are unique; one that isn't (written by hand) gets a new one.
        if accounts.iter().any(|known| known.id == account.id) {
            accounts.push(Account { id: Account::new(account.credential.clone()).id, ..account });
        } else {
            accounts.push(account);
        }
    }
    Ok(accounts)
}

/// How `accounts` are stored; None when there are none.
fn accounts_value(accounts: &[Account]) -> Result<Option<Value>, Error> {
    let Some((first, rest)) = accounts.split_first() else { return Ok(None) };
    let mut value = account_value(first)?;
    if !rest.is_empty()
        && let Some(object) = value.as_object_mut()
    {
        let rest = rest.iter().map(account_value).collect::<Result<Vec<_>, _>>()?;
        object.insert(MORE_ACCOUNTS.into(), Value::Array(rest));
    }
    Ok(Some(value))
}

/// The fields of a stored credential that are no secret, nor made from one:
/// what an outline keeps.
const OUTLINED: [&str; 9] = [
    "type",
    ACCOUNT_ID,
    ACCOUNT_LABEL,
    EXHAUSTED_UNTIL,
    FABLE_EXHAUSTED_UNTIL,
    OPUS_EXHAUSTED_UNTIL,
    SONNET_EXHAUSTED_UNTIL,
    OUT_OF_CREDIT,
    REFUSED_SINCE,
];

fn outline_value(value: &Value) -> Value {
    let Some(object) = value.as_object() else { return Value::Object(Map::new()) };
    let mut outlined: Map<String, Value> =
        object.iter().filter(|(key, _)| OUTLINED.contains(&key.as_str())).map(|(key, value)| (key.clone(), value.clone())).collect();
    // Empty in place of the secrets, so it reads as a credential of its kind.
    let blanks = match object.get("type").and_then(Value::as_str) {
        Some("oauth") => vec![("access", Value::from("")), ("refresh", Value::from("")), ("expires", Value::from(0))],
        _ => vec![("key", Value::from(""))],
    };
    for (key, blank) in blanks {
        outlined.insert(key.to_owned(), blank);
    }
    if let Some(more) = object.get(MORE_ACCOUNTS).and_then(Value::as_array) {
        outlined.insert(MORE_ACCOUNTS.to_owned(), Value::Array(more.iter().map(outline_value).collect()));
    }
    Value::Object(outlined)
}

/// The credentials document without its credentials: each provider's
/// accounts, their ids, kinds, labels and states, with the keys and tokens
/// (and anything made from them, such as an identity) left out. What's known
/// of the sign-ins while they're locked away.
pub fn outline(all: &Map<String, Value>) -> Map<String, Value> {
    all.iter().map(|(provider_id, value)| (provider_id.clone(), outline_value(value))).collect()
}

/// Brings the sign-ins in `from` (a credentials document written elsewhere:
/// `factoryd login` with no Claktory running, sign-ins made while they were
/// locked away, an older Claktory's) into `into`, as signing in to each
/// again would: one with the same identity or id replaces it, keeping its
/// id; any other is added after the provider's others. Whether `into` changed.
pub fn merge(into: &mut Map<String, Value>, from: &Map<String, Value>) -> Result<bool, Error> {
    let mut changed = false;
    for (provider_id, value) in from {
        let Ok(mut accounts) = parse_accounts(into.get(provider_id)) else {
            // What's kept can't be read either: the newer one is kept, as it is.
            changed |= into.get(provider_id) != Some(value);
            into.insert(provider_id.clone(), value.clone());
            continue;
        };
        let Ok(from_accounts) = parse_accounts(Some(value)) else {
            // One this can't read (edited by hand, or a newer Claktory's) is
            // carried along as it is, unless it would replace sign-ins this
            // can read: those are kept, and it's left out.
            if accounts.is_empty() {
                into.insert(provider_id.clone(), value.clone());
                changed = true;
            } else {
                tracing::warn!(provider = %provider_id, "left out a sign-in that can't be read: the ones kept for this provider stay");
            }
            continue;
        };
        for account in from_accounts {
            // The same id is the same account unless they say they're two.
            let agrees = |known: &Account| known.identity.is_empty() || account.identity.is_empty() || known.identity == account.identity;
            let same = accounts
                .iter()
                .position(|known| !account.identity.is_empty() && known.identity == account.identity)
                .or_else(|| accounts.iter().position(|known| known.id == account.id && agrees(known)));
            match same {
                Some(position) if accounts[position] == Account { id: accounts[position].id.clone(), ..account.clone() } => {}
                Some(position) => {
                    accounts[position] = Account { id: accounts[position].id.clone(), ..account };
                    changed = true;
                }
                None => {
                    let id = if accounts.iter().any(|known| known.id == account.id) {
                        Account::new(account.credential.clone()).id
                    } else {
                        account.id.clone()
                    };
                    accounts.push(Account { id, ..account });
                    changed = true;
                }
            }
        }
        if let Some(value) = accounts_value(&accounts)? {
            into.insert(provider_id.clone(), value);
        }
    }
    Ok(changed)
}

/// The credentials document as read from where it's kept. Not Debug: it
/// holds tokens.
#[derive(Clone, Default)]
pub struct Loaded {
    /// The credentials to use and change.
    pub all: Map<String, Value>,
    /// The outline of sign-ins that are locked away (`outline`), while they
    /// are: `all` then holds only sign-ins made meanwhile.
    pub locked: Option<Map<String, Value>>,
}

/// Where the credentials document is kept: `auth.json`, or somewhere a
/// Claktory keeps it sealed (its vault). Calls come one at a time, under the
/// store's lock.
pub trait CredentialBacking: Send + Sync {
    fn load(&self) -> Result<Loaded, Error>;
    /// Keeps `all`, as `load` gave it: `locked` when it was loaded while the
    /// sign-ins were locked away.
    fn save(&self, all: &Map<String, Value>, locked: bool) -> Result<(), Error>;
    /// Takes in what was kept elsewhere meanwhile (see `merge`); whether
    /// anything changed.
    fn take_in(&self) -> Result<bool, Error> {
        Ok(false)
    }
}

/// The credentials in a file of their own, `auth.json`.
pub struct FileBacking {
    path: PathBuf,
}

impl FileBacking {
    pub fn new(path: PathBuf) -> FileBacking {
        FileBacking { path }
    }
}

impl CredentialBacking for FileBacking {
    fn load(&self) -> Result<Loaded, Error> {
        Ok(Loaded { all: read_document(&self.path)?, locked: None })
    }

    fn save(&self, all: &Map<String, Value>, _locked: bool) -> Result<(), Error> {
        write_document(&self.path, all)
    }
}

/// The error for sign-ins that are locked away until the person opens
/// their secrets. Providers whose sign-ins are kept only in the vault,
/// sealed: never in `auth.json`, not as a copy for an older backend, nor
/// while the vault is locked (Grok).
pub const VAULT_ONLY: [&str; 1] = ["grok"];

/// The first provider in `all` whose sign-ins may only be kept in the vault.
pub fn vault_only_in(all: &Map<String, Value>) -> Option<&'static str> {
    VAULT_ONLY.into_iter().find(|provider| all.contains_key(*provider))
}

/// `all` without the sign-ins kept only in the vault: what may go into
/// `auth.json`.
pub fn plaintext(all: &Map<String, Value>) -> Map<String, Value> {
    all.iter().filter(|(provider, _)| !VAULT_ONLY.contains(&provider.as_str())).map(|(key, value)| (key.clone(), value.clone())).collect()
}

/// Why a sign-in kept only in the vault can't be kept now.
pub fn vault_only_error(provider_id: &str) -> Error {
    let name = if provider_id == "grok" { "Grok" } else { provider_id };
    Error::Models {
        code: crate::errors::LOCKED,
        message: format!(
            "A {name} sign-in is kept only with your secrets, which can't be opened now. Unlock them, then sign in to {name} again."
        ),
    }
}

pub fn locked_error(provider_id: &str) -> Error {
    tracing::debug!(provider = provider_id, "sign-ins locked away");
    Error::Models { code: crate::errors::LOCKED, message: "Your sign-ins are locked with your secrets.".to_owned() }
}

/// Credentials by provider id, each provider's accounts in the order
/// requests try them, kept by its backing (`auth.json` unless given
/// another); writes are serialized because a provider refreshes tokens
/// inside `modify`.
pub struct FileCredentialStore {
    /// Its file, `auth.json`: where it keeps them unless given a backing.
    path: PathBuf,
    backing: RwLock<Arc<dyn CredentialBacking>>,
    lock: Mutex<()>,
}

impl FileCredentialStore {
    pub fn new(path: PathBuf) -> FileCredentialStore {
        FileCredentialStore { backing: RwLock::new(Arc::new(FileBacking::new(path.clone()))), path, lock: Mutex::new(()) }
    }

    /// Its file, `auth.json`.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Keeps the credentials with `backing` from now on.
    pub fn set_backing(&self, backing: Arc<dyn CredentialBacking>) {
        *self.backing.write() = backing;
    }

    fn backing(&self) -> Arc<dyn CredentialBacking> {
        self.backing.read().clone()
    }

    fn load(&self) -> Result<Loaded, Error> {
        self.backing().load()
    }

    /// Writes a provider's accounts into `loaded`; none removes the provider.
    fn write_accounts(&self, loaded: Loaded, provider_id: &str, accounts: &[Account]) -> Result<(), Error> {
        let Loaded { mut all, locked } = loaded;
        match accounts_value(accounts)? {
            Some(value) => {
                all.insert(provider_id.to_owned(), value);
            }
            None => {
                all.remove(provider_id);
            }
        }
        self.backing().save(&all, locked.is_some())
    }

    /// Takes in sign-ins kept elsewhere meanwhile (`CredentialBacking::take_in`).
    pub async fn take_in(&self) -> Result<bool, Error> {
        let _guard = self.lock.lock().await;
        self.backing().take_in()
    }

    /// The credential of the provider's first account.
    pub async fn read(&self, provider_id: &str) -> Result<Option<Credential>, Error> {
        Ok(self.accounts(provider_id).await?.into_iter().next().map(|account| account.credential))
    }

    /// The provider's accounts, in the order requests try them. While
    /// sign-ins are locked away, those made meanwhile; none of them, and some
    /// locked away, is `locked_error`.
    pub async fn accounts(&self, provider_id: &str) -> Result<Vec<Account>, Error> {
        let loaded = self.load()?;
        if !loaded.all.contains_key(provider_id) && loaded.locked.as_ref().is_some_and(|locked| locked.contains_key(provider_id)) {
            return Err(locked_error(provider_id));
        }
        parse_accounts(loaded.all.get(provider_id))
    }

    /// The provider's accounts as far as they're known, to show: while
    /// sign-ins are locked away, those made meanwhile, then the outline of
    /// those locked away (credentials empty).
    pub async fn outlined_accounts(&self, provider_id: &str) -> Result<Vec<Account>, Error> {
        let loaded = self.load()?;
        let mut accounts = parse_accounts(loaded.all.get(provider_id))?;
        if let Some(locked) = &loaded.locked {
            for account in parse_accounts(locked.get(provider_id))? {
                if !accounts.iter().any(|known| known.id == account.id) {
                    accounts.push(account);
                }
            }
        }
        Ok(accounts)
    }

    /// The provider's accounts locked away now (outlined); empty while
    /// they're open.
    pub async fn locked_accounts(&self, provider_id: &str) -> Result<Vec<Account>, Error> {
        parse_accounts(self.load()?.locked.as_ref().and_then(|locked| locked.get(provider_id)))
    }

    pub async fn list(&self) -> Result<Vec<CredentialInfo>, Error> {
        let loaded = self.load()?;
        let mut all = loaded.locked.unwrap_or_default();
        all.extend(loaded.all);
        Ok(all
            .iter()
            .map(|(provider_id, credential)| CredentialInfo {
                provider_id: provider_id.clone(),
                kind: credential.get("type").and_then(Value::as_str).unwrap_or_default().to_owned(),
            })
            .collect())
    }

    /// Changes the credential of the provider's first account (making one
    /// when it has none): `f` gets it and returns the new one, or None to
    /// leave it. Returns the credential it ends with.
    pub async fn modify<F, Fut>(&self, provider_id: &str, f: F) -> Result<Option<Credential>, Error>
    where
        F: FnOnce(Option<Credential>) -> Fut,
        Fut: Future<Output = Result<Option<Credential>, Error>>,
    {
        let _guard = self.lock.lock().await;
        let loaded = self.load()?;
        let mut accounts = parse_accounts(loaded.all.get(provider_id))?;
        let current = accounts.first().map(|account| account.credential.clone());
        let Some(next) = f(current.clone()).await? else { return Ok(current) };
        match accounts.first_mut() {
            Some(first) => first.credential = next.clone(),
            None => accounts.push(Account::new(next.clone())),
        }
        self.write_accounts(loaded, provider_id, &accounts)?;
        Ok(Some(next))
    }

    /// Changes the credential of one of the provider's accounts, as `modify`;
    /// None when it has no such account.
    pub async fn modify_account<F, Fut>(&self, provider_id: &str, account_id: &str, f: F) -> Result<Option<Credential>, Error>
    where
        F: FnOnce(Option<Credential>) -> Fut,
        Fut: Future<Output = Result<Option<Credential>, Error>>,
    {
        let _guard = self.lock.lock().await;
        let loaded = self.load()?;
        let mut accounts = parse_accounts(loaded.all.get(provider_id))?;
        let Some(position) = accounts.iter().position(|account| account.id == account_id) else { return Ok(None) };
        let current = accounts[position].credential.clone();
        let Some(next) = f(Some(current.clone())).await? else { return Ok(Some(current)) };
        accounts[position].credential = next.clone();
        self.write_accounts(loaded, provider_id, &accounts)?;
        Ok(Some(next))
    }

    /// Changes the provider's accounts: `f` gets them and returns them
    /// changed (added, removed, reordered), or None to leave them. Returns
    /// the accounts it ends with.
    pub async fn modify_accounts<F, Fut>(&self, provider_id: &str, f: F) -> Result<Vec<Account>, Error>
    where
        F: FnOnce(Vec<Account>) -> Fut,
        Fut: Future<Output = Result<Option<Vec<Account>>, Error>>,
    {
        let _guard = self.lock.lock().await;
        let loaded = self.load()?;
        let accounts = parse_accounts(loaded.all.get(provider_id))?;
        let Some(next) = f(accounts.clone()).await? else { return Ok(accounts) };
        self.write_accounts(loaded, provider_id, &next)?;
        Ok(next)
    }

    /// Removes all of the provider's accounts. Not while some are locked
    /// away: they'd come back when they open.
    pub async fn delete(&self, provider_id: &str) -> Result<(), Error> {
        let _guard = self.lock.lock().await;
        let Loaded { mut all, locked } = self.load()?;
        if locked.as_ref().is_some_and(|locked| locked.contains_key(provider_id)) {
            return Err(locked_error(provider_id));
        }
        if all.remove(provider_id).is_none() {
            return Ok(());
        }
        self.backing().save(&all, locked.is_some())
    }
}
