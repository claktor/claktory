//! Dictation: the person says what they'd type in any field (a review
//! answer, an Ask, a checklist reply, an approval's comment), Claktory
//! transcribes it, and the words fill the field to edit and send as text.
//! Unlike a voice message nothing is delivered to an agent, and nothing is
//! kept on disk: the recording is deleted once it's transcribed or refused,
//! and the words go only to the connections that asked (TranscribeAudio,
//! AudioTranscribed).
//!
//! On a weak connection the app asks again, from its next connection, with
//! the same request and upload ids: a repeat waits with the first
//! for the words, or gets them at once once they're there, without sending
//! the recording again. So the answer is kept, in memory only, for an hour.
//!
//! The recording comes as an upload for no agent (UPLOAD_PURPOSE_DICTATION),
//! kept in `<home>/speech/dictation/` (0700) until a TranscribeAudio takes
//! it, or an hour; whatever is there when factoryd starts is deleted. The
//! voice worker transcribes dictations first, since someone is waiting.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use factory_protocol::v1;
use factory_protocol::v1::server_message::Body;

use crate::factory::{Factory, RequestError};
use crate::speech::model::ModelState;

/// The biggest dictation upload: 10 minutes of the apps' AAC is a few MB.
pub(crate) const MAX_SIZE: u64 = 16 * 1024 * 1024;
/// The longest recording transcribed.
pub(crate) const MAX_MS: u64 = 10 * 60 * 1000;
/// How long a finished upload waits for its TranscribeAudio, and a
/// dictation's answer for the same request asked again.
const KEPT: Duration = Duration::from_secs(3600);
/// AudioTranscribed.error when the recording isn't here (never uploaded,
/// over an hour old, or from before a restart) and nothing was heard for
/// this request: the app uploads it again.
const NO_RECORDING: &str = "no_recording";
/// Dictations waiting at once, across connections.
const WAITING: usize = 16;

/// Uploads ready to transcribe, and dictations waiting for the worker.
#[derive(Default)]
pub(crate) struct Dictations {
    /// Finished uploads by upload id: where, and since when.
    ready: Mutex<HashMap<String, (PathBuf, Instant)>>,
    waiting: Mutex<VecDeque<Job>>,
    asked: Asked,
}

/// Each TranscribeAudio by its request and upload ids: the connections
/// waiting for its words, or its answer, for `KEPT`.
type Asked = Arc<Mutex<HashMap<(String, String), Heard>>>;

/// Where an answer goes: a connection's `Port`.
type Answer = tokio::sync::mpsc::UnboundedSender<v1::AudioTranscribed>;

enum Heard {
    Waiting(Vec<Answer>),
    Answered(v1::AudioTranscribed, Instant),
}

/// One dictation to transcribe, and where its answer goes.
pub(crate) struct Job {
    key: (String, String),
    pub(crate) recording: PathBuf,
    asked: Asked,
    answered: bool,
}

impl Job {
    /// Answers every connection waiting for it, keeps the answer for a
    /// repeat, and deletes the recording.
    pub(crate) fn answer(mut self, text: String, error: &str, duration_ms: u64) {
        remove(&self.recording);
        if !error.is_empty() {
            tracing::info!(error, "a dictation wasn't transcribed");
        }
        self.answered = true;
        self.deliver(v1::AudioTranscribed { request_id: self.key.0.clone(), text, error: error.to_owned(), duration_ms });
    }

    fn deliver(&self, answer: v1::AudioTranscribed) {
        let mut asked = self.asked.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(heard) = asked.get_mut(&self.key) else { return };
        if let Heard::Waiting(waiting) = std::mem::replace(heard, Heard::Answered(answer.clone(), Instant::now())) {
            for connection in waiting {
                let _ = connection.send(answer.clone());
            }
        }
    }
}

/// One dropped without an answer (the worker gone mid-run, say) deletes its
/// recording all the same, at once, and says it failed.
impl Drop for Job {
    fn drop(&mut self) {
        remove(&self.recording);
        if !self.answered {
            self.deliver(v1::AudioTranscribed { request_id: self.key.0.clone(), error: "failed".to_owned(), ..Default::default() });
        }
    }
}

fn remove(path: &Path) {
    if let Err(error) = std::fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%error, "could not delete a dictation's recording");
    }
}

/// Where dictations are kept, made private if it isn't there yet.
pub(crate) fn staging(home: &crate::home::Home) -> std::io::Result<PathBuf> {
    let dir = home.root().join("speech").join("dictation");
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        match std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir) {
            Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => return Err(error),
            _ => {}
        }
    }
    #[cfg(windows)]
    factory_agent::windows::private::create_dir_all(&dir)?;
    Ok(dir)
}

/// At start: nothing from before is kept.
pub(crate) fn clear(home: &crate::home::Home) {
    let dir = home.root().join("speech").join("dictation");
    if dir.exists()
        && let Err(error) = std::fs::remove_dir_all(&dir)
    {
        tracing::warn!(%error, "could not delete dictations left from before");
    }
}

/// Whether a dictation upload of this type and size may start.
pub(crate) fn check_upload(mime: &str, size: u64) -> Result<(), String> {
    if !mime.starts_with("audio/") {
        return Err("A dictation is a recording (an audio type, such as audio/mp4).".to_owned());
    }
    if size == 0 || size > MAX_SIZE {
        return Err(format!("A dictation is 1 byte to {} MB; this one is {size} bytes.", MAX_SIZE / (1024 * 1024)));
    }
    Ok(())
}

impl Dictations {
    /// A dictation upload is all there: kept for its TranscribeAudio. Old
    /// ones nobody asked for go.
    pub(crate) fn uploaded(&self, home: &crate::home::Home, upload_id: &str, partial: &Path) -> std::io::Result<()> {
        let dir = staging(home)?;
        let name: String = upload_id
            .chars()
            .map(|character| if character.is_ascii_alphanumeric() || character == '-' { character } else { '_' })
            .collect();
        let path = dir.join(format!("{name}-{}.audio", crate::transcript::now_ms()));
        std::fs::rename(partial, &path)?;
        let mut ready = self.ready.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        ready.retain(|_, (path, since)| {
            let keep = since.elapsed() < KEPT;
            if !keep {
                remove(path);
            }
            keep
        });
        if let Some((old, _)) = ready.insert(upload_id.to_owned(), (path, Instant::now())) {
            remove(&old);
        }
        Ok(())
    }

    fn take(&self, upload_id: &str) -> Option<PathBuf> {
        let (path, since) = self.ready.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(upload_id)?;
        if since.elapsed() < KEPT {
            return Some(path);
        }
        remove(&path);
        None
    }

    /// The next dictation to transcribe.
    pub(crate) fn next(&self) -> Option<Job> {
        self.waiting.lock().unwrap_or_else(std::sync::PoisonError::into_inner).pop_front()
    }

    /// Whether any is waiting.
    pub(crate) fn any(&self) -> bool {
        !self.waiting.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_empty()
    }

    /// The model can't be had: every dictation waiting is answered so.
    fn refuse_all(&self, error: &str) {
        let waiting: Vec<Job> = self.waiting.lock().unwrap_or_else(std::sync::PoisonError::into_inner).drain(..).collect();
        for job in waiting {
            job.answer(String::new(), error, 0);
        }
    }
}

/// Why the model can't take a dictation now, as AudioTranscribed.error;
/// None while it's there or on its way (it waits). Never starts a
/// download: the app asks the person first.
fn unusable(state: &ModelState) -> Option<&'static str> {
    match state {
        ModelState::Unsupported(_) => Some("unsupported"),
        ModelState::Absent | ModelState::Failed(_) => Some("no_model"),
        ModelState::Downloading { .. } | ModelState::Verifying | ModelState::Ready => None,
    }
}

/// TranscribeAudio: queues the dictation, or answers at once why not.
pub(crate) fn request(factory: &Arc<Factory>, request: &v1::TranscribeAudio, port: &Port) -> Result<(), RequestError> {
    if request.request_id.trim().is_empty() {
        return Err(RequestError::Empty("request_id"));
    }
    if request.upload_id.trim().is_empty() {
        return Err(RequestError::Empty("upload_id"));
    }
    let dictations = &factory.voices.dictations;
    let answer = port.sender.clone();
    let key = (request.request_id.clone(), request.upload_id.clone());
    let job = {
        let mut asked = dictations.asked.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        asked.retain(|_, heard| !matches!(heard, Heard::Answered(_, at) if at.elapsed() >= KEPT));
        // Asked again (from the next connection, after a drop): with the
        // first, or answered as it was.
        match asked.get_mut(&key) {
            Some(Heard::Waiting(waiting)) => {
                waiting.retain(|connection| !connection.is_closed());
                waiting.push(answer);
                return Ok(());
            }
            Some(Heard::Answered(heard, _)) => {
                let _ = answer.send(heard.clone());
                return Ok(());
            }
            None => {}
        }
        // Gone (or never sent): to upload again.
        let Some(recording) = dictations.take(&request.upload_id) else {
            drop(asked);
            let error = NO_RECORDING.to_owned();
            let _ = answer.send(v1::AudioTranscribed { request_id: request.request_id.clone(), error, ..Default::default() });
            return Ok(());
        };
        asked.insert(key.clone(), Heard::Waiting(vec![answer]));
        Job { key, recording, asked: dictations.asked.clone(), answered: false }
    };
    if let Some(error) = unusable(&factory.speech.state()) {
        job.answer(String::new(), error, 0);
        return Ok(());
    }
    {
        let mut waiting = dictations.waiting.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if waiting.len() >= WAITING {
            drop(waiting);
            job.answer(String::new(), "failed", 0);
            return Ok(());
        }
        waiting.push_back(job);
    }
    factory.voices.kick();
    Ok(())
}

/// The model came or went: dictations waiting for one that can't be had
/// now are answered so.
pub(crate) fn model_changed(factory: &Factory) {
    if let Some(error) = unusable(&factory.speech.state()) {
        factory.voices.dictations.refuse_all(error);
    }
}

/// What AudioTranscribed.error says for a failure of `claktory-speech`'s
/// (Transcription.error_code).
pub(crate) fn error_for(code: &str) -> &'static str {
    match code {
        "no_model" => "no_model",
        "unsupported_cpu" => "unsupported",
        "too_long" => "too_long",
        "no_speech" => "no_speech",
        _ => "failed",
    }
}

/// A connection's answers to its TranscribeAudio requests.
pub(crate) struct Port {
    sender: tokio::sync::mpsc::UnboundedSender<v1::AudioTranscribed>,
    receiver: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<v1::AudioTranscribed>>,
}

impl Port {
    pub(crate) fn new() -> Port {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        Port { sender, receiver: tokio::sync::Mutex::new(receiver) }
    }

    /// The next answer for this connection.
    pub(crate) async fn next(&self) -> Body {
        match self.receiver.lock().await.recv().await {
            Some(answer) => Body::AudioTranscribed(answer),
            // It holds a sender itself: never.
            None => std::future::pending().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_dropped_unanswered_deletes_its_recording() {
        let dir = std::env::temp_dir().join(format!("factory-dictation-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let recording = dir.join("said.audio");
        std::fs::write(&recording, "words").expect("recording");
        let (answer, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let asked = Asked::default();
        let key = ("r".to_owned(), "u".to_owned());
        asked.lock().unwrap().insert(key.clone(), Heard::Waiting(vec![answer]));
        drop(Job { key, recording: recording.clone(), asked, answered: false });
        assert!(!recording.exists());
        assert_eq!(receiver.try_recv().expect("an answer").error, "failed", "the connection waiting hears it failed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
