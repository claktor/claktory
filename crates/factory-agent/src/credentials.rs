use std::fs::{self, DirBuilder, OpenOptions};
use std::future::Future;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, fchown};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::Mutex;

use crate::errors::{AgentError, AgentFailure, Error};

fn corrupt(file: &Path) -> AgentFailure {
    AgentFailure::CorruptCredentials { file: file.display().to_string() }
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
const OUT_OF_CREDIT: &str = "factoryOutOfCredit";
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
    /// An API key that ran out of credit: since when, in ms since 1970; 0
    /// when it has credit as far as Factory knows.
    pub out_of_credit_since: u64,
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
            out_of_credit_since: 0,
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
    let out_of_credit_since = object.remove(OUT_OF_CREDIT).and_then(|value| value.as_u64()).unwrap_or(0);
    let credential =
        if value.is_object() { serde_json::from_value(Value::Object(object))? } else { serde_json::from_value(value.clone())? };
    let id = if id.is_empty() { (position + 1).to_string() } else { id };
    Ok(Account { id, label, identity, exhausted_until, fable_exhausted_until, out_of_credit_since, credential })
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
        if account.out_of_credit_since != 0 {
            object.insert(OUT_OF_CREDIT.into(), Value::from(account.out_of_credit_since));
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

/// Credentials on disk, by provider id, each provider's accounts in the
/// order requests try them; writes are serialized because a provider
/// refreshes tokens inside `modify`.
pub struct FileCredentialStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl FileCredentialStore {
    pub fn new(path: PathBuf) -> FileCredentialStore {
        FileCredentialStore { path, lock: Mutex::new(()) }
    }

    fn read_all(&self) -> Result<Map<String, Value>, Error> {
        Ok(read_json(&self.path)?.and_then(|value| value.as_object().cloned()).unwrap_or_default())
    }

    /// Writes a provider's accounts into `all`; none removes the provider.
    fn write_accounts(&self, mut all: Map<String, Value>, provider_id: &str, accounts: &[Account]) -> Result<(), Error> {
        match accounts_value(accounts)? {
            Some(value) => {
                all.insert(provider_id.to_owned(), value);
            }
            None => {
                all.remove(provider_id);
            }
        }
        write_json(&self.path, &Value::Object(all))
    }

    /// The credential of the provider's first account.
    pub async fn read(&self, provider_id: &str) -> Result<Option<Credential>, Error> {
        Ok(self.accounts(provider_id).await?.into_iter().next().map(|account| account.credential))
    }

    /// The provider's accounts, in the order requests try them.
    pub async fn accounts(&self, provider_id: &str) -> Result<Vec<Account>, Error> {
        parse_accounts(self.read_all()?.get(provider_id))
    }

    pub async fn list(&self) -> Result<Vec<CredentialInfo>, Error> {
        Ok(self
            .read_all()?
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
        let all = self.read_all()?;
        let mut accounts = parse_accounts(all.get(provider_id))?;
        let current = accounts.first().map(|account| account.credential.clone());
        let Some(next) = f(current.clone()).await? else { return Ok(current) };
        match accounts.first_mut() {
            Some(first) => first.credential = next.clone(),
            None => accounts.push(Account::new(next.clone())),
        }
        self.write_accounts(all, provider_id, &accounts)?;
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
        let all = self.read_all()?;
        let mut accounts = parse_accounts(all.get(provider_id))?;
        let Some(position) = accounts.iter().position(|account| account.id == account_id) else { return Ok(None) };
        let current = accounts[position].credential.clone();
        let Some(next) = f(Some(current.clone())).await? else { return Ok(Some(current)) };
        accounts[position].credential = next.clone();
        self.write_accounts(all, provider_id, &accounts)?;
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
        let all = self.read_all()?;
        let accounts = parse_accounts(all.get(provider_id))?;
        let Some(next) = f(accounts.clone()).await? else { return Ok(accounts) };
        self.write_accounts(all, provider_id, &next)?;
        Ok(next)
    }

    /// Removes all of the provider's accounts.
    pub async fn delete(&self, provider_id: &str) -> Result<(), Error> {
        let _guard = self.lock.lock().await;
        let mut all = self.read_all()?;
        if all.remove(provider_id).is_none() {
            return Ok(());
        }
        write_json(&self.path, &Value::Object(all))
    }
}
