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

/// Credentials on disk, one per provider id; writes are serialized because a provider refreshes tokens inside `modify`.
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

    fn parse(value: Option<&Value>) -> Result<Option<Credential>, Error> {
        value.map(|value| serde_json::from_value(value.clone())).transpose().map_err(Into::into)
    }

    pub async fn read(&self, provider_id: &str) -> Result<Option<Credential>, Error> {
        FileCredentialStore::parse(self.read_all()?.get(provider_id))
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

    pub async fn modify<F, Fut>(&self, provider_id: &str, f: F) -> Result<Option<Credential>, Error>
    where
        F: FnOnce(Option<Credential>) -> Fut,
        Fut: Future<Output = Result<Option<Credential>, Error>>,
    {
        let _guard = self.lock.lock().await;
        let mut all = self.read_all()?;
        let current = FileCredentialStore::parse(all.get(provider_id))?;
        let Some(next) = f(current.clone()).await? else { return Ok(current) };
        all.insert(provider_id.to_owned(), serde_json::to_value(&next)?);
        write_json(&self.path, &Value::Object(all))?;
        Ok(Some(next))
    }

    pub async fn delete(&self, provider_id: &str) -> Result<(), Error> {
        let _guard = self.lock.lock().await;
        let mut all = self.read_all()?;
        if all.remove(provider_id).is_none() {
            return Ok(());
        }
        write_json(&self.path, &Value::Object(all))
    }
}
