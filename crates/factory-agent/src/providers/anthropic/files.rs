//! Images sent to Anthropic once. Every
//! model call sends the whole conversation, so a screenshot read once went
//! up again with every later call: most of what Claktory uploaded. An image
//! read since (one with a `digest`) is uploaded to Anthropic's Files API the
//! first time a request to an account carries it, and every later request
//! to that account names it by its file id. The conversation keeps the
//! bytes, so another provider, a fallback or another account still gets
//! them inline.
//!
//! What a request sent never changes later (the prompt cache, AGENTS.md):
//! an image sent inline to an account stays inline for it, and one sent as
//! a file stays that file until it expires (after `KEEP_MS`, longer than
//! any cache lasts). A request Anthropic answers that it hasn't files it
//! names (a 404 naming them) is sent again once with just those uploaded
//! again; any other refusal for a file it names, or another after that, is
//! sent again once with its images inline, for good.
//!
//! Which file each image is, per account, is kept in a JSON Lines file
//! beside the sign-ins, only ever appended to (and written again without
//! what expired when it's first read), so a restart sends the same ids and
//! the cache holds. The files are deleted at Anthropic when the
//! conversation lets go of them (compaction, /clear), or expire there.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hashbrown::HashMap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::clock::now_ms;
use crate::providers::client;

/// How long an uploaded file lives at Anthropic: long past any prompt
/// cache, short enough that what a conversation left behind goes by itself.
pub const KEEP_MS: u64 = 7 * 24 * 3_600_000;
/// A file this close to expiring is uploaded again rather than named: the
/// request naming it may be retried, or run long.
const MARGIN_MS: u64 = 3_600_000;
/// How long an account whose uploads Anthropic refused sends images inline.
const OFF_MS: u64 = 24 * 3_600_000;
/// How long one upload or deletion may take.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// How an image went to an account.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Sent {
    File(String),
    Inline,
}

#[derive(Clone, Debug)]
struct Entry {
    sent: Sent,
    expires: u64,
    /// Where a file was uploaded to (the API's base URL), to delete it there.
    base: String,
}

/// One line of the file: how an image went to an account, or (`gone`)
/// that it's forgotten.
#[derive(Serialize, Deserialize)]
struct Line {
    account: String,
    digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(default)]
    expires: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    base: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    gone: bool,
}

type Key = (String, String);

#[derive(Default)]
struct State {
    loaded: bool,
    /// By (account, digest).
    entries: HashMap<Key, Entry>,
    /// Accounts whose uploads Anthropic refused, until when.
    off: HashMap<String, u64>,
}

/// The images Anthropic holds as files, per account.
pub struct ImageFiles {
    path: Option<PathBuf>,
    state: Mutex<State>,
}

impl std::fmt::Debug for ImageFiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageFiles").field("path", &self.path).finish_non_exhaustive()
    }
}

/// `ImageFiles` for the account a request goes to: the provider's id and
/// the account's as the sign-ins name it, `anthropic/1`.
#[derive(Clone, Debug)]
pub struct ImageFilesFor {
    pub files: Arc<ImageFiles>,
    pub account: String,
}

/// An image a request carries: its digest, data and type.
pub struct Image<'a> {
    pub digest: &'a str,
    pub data: &'a crate::types::ImageData,
    pub mime_type: &'a str,
}

/// A file to delete at Anthropic: its account (as `ImageFilesFor` names
/// it), where it is and its id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Forgotten {
    pub account: String,
    pub base_url: String,
    pub id: String,
}

/// Why an upload didn't give a file.
enum Failed {
    /// Anthropic won't take files from this account (or it isn't Anthropic).
    Refused(String),
    /// Anything else: the network, a limit, an outage.
    Other(String),
    Cancelled,
}

impl ImageFiles {
    /// Kept in `path` (None: only while the process runs).
    pub fn new(path: Option<PathBuf>) -> ImageFiles {
        ImageFiles { path, state: Mutex::new(State::default()) }
    }

    fn state(&self) -> parking_lot::MutexGuard<'_, State> {
        let mut state = self.state.lock();
        if !state.loaded {
            state.loaded = true;
            self.load(&mut state);
        }
        state
    }

    /// Reads the file, and writes it again with only what still holds when
    /// it has more than that.
    fn load(&self, state: &mut State) {
        let Some(path) = &self.path else { return };
        let Ok(text) = std::fs::read_to_string(path) else { return };
        let now = now_ms();
        let mut lines = 0;
        for line in text.lines() {
            lines += 1;
            let Ok(line) = serde_json::from_str::<Line>(line) else { continue };
            let key = (line.account, line.digest);
            if line.gone || line.expires <= now {
                state.entries.remove(&key);
            } else {
                let sent = line.id.map_or(Sent::Inline, Sent::File);
                state.entries.insert(key, Entry { sent, expires: line.expires, base: line.base });
            }
        }
        if lines > state.entries.len() {
            let kept: String = state.entries.iter().map(|(key, entry)| line(key, entry) + "\n").collect();
            if let Err(error) = write_atomic(path, kept.as_bytes()) {
                tracing::warn!(%error, "could not tidy the list of images sent to Anthropic");
            }
        }
    }

    fn append(&self, lines: &[String]) {
        let Some(path) = &self.path else { return };
        if lines.is_empty() {
            return;
        }
        let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
        if let Err(error) = private(std::fs::OpenOptions::new().create(true).append(true)).open(path).and_then(|mut file| file.write_all(text.as_bytes())) {
            tracing::warn!(%error, "could not keep which images were sent to Anthropic");
        }
    }

    fn keep(state: &mut State, key: Key, sent: Sent, base: &str, now: u64) -> String {
        let entry = Entry { sent, expires: now + KEEP_MS, base: base.to_owned() };
        let written = line(&key, &entry);
        state.entries.insert(key, entry);
        written
    }

    /// The file id each of `images` goes as to `account`, uploading first
    /// (with `headers`, to `base_url`) the ones it hasn't had, and whether
    /// any was uploaded now. An image missing from the answer goes inline:
    /// one sent inline before, or whose upload failed (inline from then on,
    /// as it was sent).
    pub async fn refs(
        &self,
        account: &str,
        images: &[Image<'_>],
        base_url: &str,
        headers: &[(String, String)],
        cancel: &CancellationToken,
    ) -> (HashMap<String, String>, bool) {
        let now = now_ms();
        let mut refs = HashMap::new();
        let mut uploads: Vec<&Image<'_>> = Vec::new();
        let mut lines = Vec::new();
        {
            let mut state = self.state();
            let off = state.off.get(account).is_some_and(|until| *until > now);
            for image in images {
                if refs.contains_key(image.digest) || uploads.iter().any(|upload| upload.digest == image.digest) {
                    continue;
                }
                let key = (account.to_owned(), image.digest.to_owned());
                match state.entries.get(&key) {
                    Some(Entry { sent: Sent::File(id), expires, .. }) if *expires > now + MARGIN_MS => {
                        refs.insert(image.digest.to_owned(), id.clone());
                    }
                    Some(Entry { sent: Sent::Inline, expires, .. }) if *expires > now => {}
                    _ if off => lines.push(Self::keep(&mut state, key, Sent::Inline, "", now)),
                    _ => uploads.push(image),
                }
            }
        }
        self.append(&lines);
        if uploads.is_empty() {
            return (refs, false);
        }
        // One at a time: a request rarely carries more than one new image.
        let mut results = Vec::with_capacity(uploads.len());
        for image in &uploads {
            results.push(upload(base_url, headers, image, cancel).await);
        }
        let mut lines = Vec::new();
        let mut fresh = false;
        {
            let mut state = self.state();
            for (image, result) in uploads.iter().zip(results) {
                let key = (account.to_owned(), image.digest.to_owned());
                match result {
                    Ok(id) => {
                        fresh = true;
                        refs.insert(image.digest.to_owned(), id.clone());
                        lines.push(Self::keep(&mut state, key, Sent::File(id), base_url, now));
                    }
                    // Never sent: nothing to keep.
                    Err(Failed::Cancelled) => {}
                    Err(Failed::Refused(why)) => {
                        tracing::info!(%account, %why, "Anthropic didn't take an image as a file; this account's images go inline for a day");
                        state.off.insert(account.to_owned(), now + OFF_MS);
                        lines.push(Self::keep(&mut state, key, Sent::Inline, "", now));
                    }
                    Err(Failed::Other(why)) => {
                        tracing::info!(%account, %why, "an image couldn't be uploaded; it goes inline");
                        lines.push(Self::keep(&mut state, key, Sent::Inline, "", now));
                    }
                }
            }
        }
        self.append(&lines);
        (refs, fresh)
    }

    /// Anthropic answered a request naming `refs` on `account` that it
    /// hasn't some of them (a 404 whose `text` names their ids: another
    /// conversation let go of the same image, say). Forgets those, so the
    /// next `refs` uploads them again, keeps the others as they are, and
    /// gives the digests it forgot: none when `text` names none (one lost
    /// file sent every image inline, 147 KB a request became 47.7 MB).
    pub fn lost(&self, account: &str, refs: &HashMap<String, String>, text: &str) -> Vec<String> {
        let digests: Vec<String> = refs.iter().filter(|(_, id)| names(text, id)).map(|(digest, _)| digest.clone()).collect();
        let lines: Vec<String> = {
            let mut state = self.state();
            digests
                .iter()
                .map(|digest| {
                    let key = (account.to_owned(), digest.clone());
                    state.entries.remove(&key);
                    gone(&key)
                })
                .collect()
        };
        self.append(&lines);
        digests
    }

    /// Anthropic refused a request naming `refs` on `account`: a file it no
    /// longer has that `lost` couldn't tell, or files it won't read from
    /// this account. They go inline from now on, never uploaded again; and
    /// when one was uploaded for that very request (`fresh`), it's the
    /// account that won't read them, so its new images go inline for a
    /// while too.
    pub fn refuse(&self, account: &str, refs: &HashMap<String, String>, fresh: bool) {
        let now = now_ms();
        let lines: Vec<String> = {
            let mut state = self.state();
            if fresh {
                state.off.insert(account.to_owned(), now + OFF_MS);
            }
            refs.keys().map(|digest| Self::keep(&mut state, (account.to_owned(), digest.clone()), Sent::Inline, "", now)).collect()
        };
        self.append(&lines);
    }

    /// Forgets the images of `digests` on every account, and gives the
    /// files to delete at Anthropic.
    pub fn forget(&self, digests: &[String]) -> Vec<Forgotten> {
        if digests.is_empty() {
            return Vec::new();
        }
        let mut files = Vec::new();
        let lines: Vec<String> = {
            let mut state = self.state();
            let keys: Vec<Key> = state.entries.keys().filter(|(_, digest)| digests.contains(digest)).cloned().collect();
            keys.into_iter()
                .map(|key| {
                    if let Some(Entry { sent: Sent::File(id), base, .. }) = state.entries.remove(&key) {
                        files.push(Forgotten { account: key.0.clone(), base_url: base, id });
                    }
                    gone(&key)
                })
                .collect()
        };
        self.append(&lines);
        files
    }
}

impl ImageFiles {
    /// The accounts it holds images of.
    pub fn accounts(&self) -> Vec<String> {
        let state = self.state();
        let mut accounts: Vec<String> = state.entries.keys().map(|(account, _)| account.clone()).collect();
        accounts.sort();
        accounts.dedup();
        accounts
    }

    /// Forgets every image of `account` (signed out of), and gives its
    /// files to delete at Anthropic while its sign-in still can.
    pub fn forget_account(&self, account: &str) -> Vec<Forgotten> {
        let mut files = Vec::new();
        let lines: Vec<String> = {
            let mut state = self.state();
            state.off.remove(account);
            let keys: Vec<Key> = state.entries.keys().filter(|(owner, _)| owner == account).cloned().collect();
            keys.into_iter()
                .map(|key| {
                    if let Some(Entry { sent: Sent::File(id), base, .. }) = state.entries.remove(&key) {
                        files.push(Forgotten { account: key.0.clone(), base_url: base, id });
                    }
                    gone(&key)
                })
                .collect()
        };
        self.append(&lines);
        files
    }
}

/// Whether `text` names the file `id`: as a word of its own, so `file_1`
/// isn't in `file_12`.
fn names(text: &str, id: &str) -> bool {
    let word = |character: char| character.is_ascii_alphanumeric() || character == '_' || character == '-';
    !id.is_empty()
        && text.match_indices(id).any(|(at, _)| {
            !text[..at].chars().next_back().is_some_and(word) && !text[at + id.len()..].chars().next().is_some_and(word)
        })
}

fn line(key: &Key, entry: &Entry) -> String {
    let id = match &entry.sent {
        Sent::File(id) => Some(id.clone()),
        Sent::Inline => None,
    };
    let line = Line { account: key.0.clone(), digest: key.1.clone(), id, expires: entry.expires, base: entry.base.clone(), gone: false };
    serde_json::to_string(&line).unwrap_or_default()
}

fn gone(key: &Key) -> String {
    let line = Line { account: key.0.clone(), digest: key.1.clone(), id: None, expires: 0, base: String::new(), gone: true };
    serde_json::to_string(&line).unwrap_or_default()
}

/// Readable by this user alone.
fn private(options: &mut std::fs::OpenOptions) -> &mut std::fs::OpenOptions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temporary = path.with_extension("new");
    private(std::fs::OpenOptions::new().create(true).write(true).truncate(true)).open(&temporary)?.write_all(bytes)?;
    std::fs::rename(&temporary, path)
}

/// Uploads one image, to expire after `KEEP_MS`; its file id.
async fn upload(base_url: &str, headers: &[(String, String)], image: &Image<'_>, cancel: &CancellationToken) -> Result<String, Failed> {
    let bytes = image.data.bytes().ok_or_else(|| Failed::Other("the image's data can't be read".to_owned()))?;
    let (mime_type, extension) = (image.mime_type, image.mime_type.trim_start_matches("image/"));
    let boundary = format!("claktory-{}", &image.digest[..image.digest.len().min(32)]);
    let mut body = Vec::with_capacity(bytes.len() + 512);
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"image.{extension}\"\r\nContent-Type: {mime_type}\r\n\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(&bytes);
    let seconds = KEEP_MS / 1000;
    body.extend_from_slice(
        format!("\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"expires_in_seconds\"\r\n\r\n{seconds}\r\n--{boundary}--\r\n").as_bytes(),
    );
    let mut builder = client().post(format!("{}/v1/files", base_url.trim_end_matches('/'))).timeout(TIMEOUT);
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    let sent = builder.header("content-type", format!("multipart/form-data; boundary={boundary}")).body(body).send();
    let response = tokio::select! {
        response = sent => response.map_err(|error| Failed::Other(error.to_string()))?,
        () = cancel.cancelled() => return Err(Failed::Cancelled),
    };
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        let why = format!("{status}: {}", text.chars().take(300).collect::<String>());
        // A limit or an outage passes; anything else Anthropic says won't.
        return Err(if status == 429 || status >= 500 { Failed::Other(why) } else { Failed::Refused(why) });
    }
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|answer| answer["id"].as_str().map(str::to_owned))
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Failed::Refused(format!("no file id in {}", text.chars().take(200).collect::<String>())))
}

/// Deletes a file at Anthropic; one already gone is as good.
pub async fn delete(base_url: &str, headers: &[(String, String)], id: &str) -> Result<(), String> {
    let mut builder = client().delete(format!("{}/v1/files/{id}", base_url.trim_end_matches('/'))).timeout(TIMEOUT);
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    let response = builder.send().await.map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    if (200..300).contains(&status) || status == 404 {
        Ok(())
    } else {
        Err(format!("{status}: {}", response.text().await.unwrap_or_default().chars().take(200).collect::<String>()))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_file_is_named_as_a_word_of_its_own() {
        assert!(super::names("File not found: file_1", "file_1"));
        assert!(super::names(r#"{"message": "file_1 not found"}"#, "file_1"));
        assert!(!super::names("File not found: file_12", "file_1"));
        assert!(!super::names("File not found: xfile_1", "file_1"));
        assert!(!super::names("File not found", ""));
    }
}
