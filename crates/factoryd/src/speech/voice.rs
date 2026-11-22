//! Voice messages (#197): the person says a message instead of typing it.
//! The app uploads the recording and sends it (SendMessage.voice); it waits
//! in the agent's queue, PENDING, while `claktory-speech` transcribes it on
//! this computer, then the agent gets its words as the person's message,
//! with `voice="42s"` on its `<sent>` tag.
//!
//! The person's messages after a PENDING one wait behind it, so the agent
//! reads them in the order they were sent; events don't. One that FAILED
//! holds nothing up, and waits for RetryVoiceMessage or
//! DiscardVoiceMessage. What's waiting, and how far it got, is kept in the
//! store, so a restart carries on with it.
//!
//! One worker transcribes, oldest first across agents, with one
//! `claktory-speech serve` kept running while there's work (it holds the
//! model in memory) and stopped after a while without any.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::Duration;

use factory_protocol::v1;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};

use crate::factory::{Factory, Queued, RequestError};
use crate::speech::model::{self, ModelState};

/// The store's record of voice messages.
pub(crate) const SCHEMA: &str = "
-- A voice message's transcription, as JSON ({\"state\": pending, done or
-- failed, \"duration_ms\", \"error\", \"error_code\", \"language\"}); null for
-- any other message. `text` is what was heard, once it's done.
ALTER TABLE messages ADD COLUMN voice TEXT;
";

/// A voice message's transcription.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Voice {
    pub(crate) state: State,
    /// How long the recording is; 0 until it's read.
    #[serde(default)]
    pub(crate) duration_ms: u64,
    /// FAILED: why, for people.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) error: String,
    /// FAILED: why, for programs (see Transcription.error_code).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) error_code: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) language: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum State {
    Pending,
    Done,
    Failed,
}

impl Voice {
    fn pending() -> Voice {
        Voice { state: State::Pending, duration_ms: 0, error: String::new(), error_code: String::new(), language: String::new() }
    }

    pub(crate) fn view(&self) -> v1::Transcription {
        let state = match self.state {
            State::Pending => v1::TranscriptionState::Pending,
            State::Done => v1::TranscriptionState::Done,
            State::Failed => v1::TranscriptionState::Failed,
        };
        v1::Transcription {
            state: state.into(),
            duration_ms: self.duration_ms,
            error: self.error.clone(),
            error_code: self.error_code.clone(),
            language: self.language.clone(),
        }
    }

    /// As the agent reads it, once it's done.
    pub(crate) fn heard(&self) -> Option<factory_agent::Voice> {
        (self.state == State::Done).then(|| factory_agent::Voice { duration_ms: self.duration_ms, language: self.language.clone() })
    }
}

/// Which of the queued messages can go to the agent now: events, and the
/// person's messages up to their first voice message still being
/// transcribed; never a voice message that failed.
pub(crate) fn deliverable(queue: &[Queued]) -> Vec<bool> {
    let mut held = false;
    queue
        .iter()
        .map(|queued| {
            if queued.event.is_some() {
                return true;
            }
            match queued.voice.as_ref().map(|voice| voice.state) {
                Some(State::Pending) => {
                    held = true;
                    false
                }
                Some(State::Failed) => false,
                Some(State::Done) | None => !held,
            }
        })
        .collect()
}

/// Wakes the worker that transcribes; and the dictations it does first.
pub(crate) struct Voices {
    work: Arc<tokio::sync::Notify>,
    pub(crate) dictations: crate::speech::dictation::Dictations,
}

impl Voices {
    pub(crate) fn new() -> Voices {
        Voices { work: Arc::new(tokio::sync::Notify::new()), dictations: crate::speech::dictation::Dictations::default() }
    }

    /// There may be something to transcribe.
    pub(crate) fn kick(&self) {
        self.work.notify_one();
    }
}

fn refused(why: impl Into<String>) -> RequestError {
    RequestError::Account(v1::ErrorCode::InvalidRequest, why.into())
}

/// Why a voice message can't be sent while the model is absent.
const NEEDS_MODEL: &str = "Voice messages need the speech model. Download it first.";

/// Whether the model can take voice messages: ready, or on its way (a
/// download that failed carries on, since the person already said yes).
fn usable(factory: &Arc<Factory>) -> Result<(), RequestError> {
    match factory.speech.state() {
        ModelState::Absent => Err(refused(NEEDS_MODEL)),
        ModelState::Unsupported(why) => Err(refused(why)),
        ModelState::Failed(_) => model::install(factory),
        ModelState::Downloading { .. } | ModelState::Verifying | ModelState::Ready => Ok(()),
    }
}

/// SendMessage with `voice`: queues the recording, PENDING, to transcribe.
pub(crate) fn send(
    factory: &Arc<Factory>,
    agent: &str,
    text: &str,
    sent_from: Option<factory_agent::SentFrom>,
    client_id: &str,
    attached: &[v1::Attachment],
) -> Result<(), RequestError> {
    if !text.trim().is_empty() {
        return Err(refused("A voice message has no text: its words come from the recording."));
    }
    if attached.len() != 1 {
        return Err(refused("A voice message carries one recording."));
    }
    factory.known_agent(agent)?;
    let attachments = factory.attachments(agent, attached)?;
    usable(factory)?;
    let queued =
        Queued { client_id: client_id.to_owned(), attachments, voice: Some(Voice::pending()), ..Queued::from_person("", sent_from) };
    factory.enqueue(&mut factory.lock(), agent, queued)?;
    factory.voices.kick();
    Ok(())
}

/// RetryVoiceMessage: transcribes a FAILED voice message again.
pub(crate) fn retry(factory: &Arc<Factory>, agent: &str, id: i64) -> Result<(), RequestError> {
    factory.known_agent(agent)?;
    usable(factory)?;
    let mut inner = factory.lock();
    let queued = inner
        .agents
        .get_mut(agent)
        .and_then(|runtime| runtime.queue.iter_mut().find(|queued| queued.id == id))
        .ok_or_else(|| refused("That voice message isn't waiting any more."))?;
    let Some(voice) = queued.voice.as_mut().filter(|voice| voice.state == State::Failed) else {
        return Err(refused("Only a voice message that couldn't be transcribed can be tried again."));
    };
    *voice = Voice { duration_ms: voice.duration_ms, ..Voice::pending() };
    factory.store.set_voice(id, &queued.text, voice)?;
    factory.broadcast_workspace(&inner);
    drop(inner);
    factory.voices.kick();
    Ok(())
}

/// DiscardVoiceMessage: drops a PENDING or FAILED voice message,
/// undelivered; its recording stays in the agent's uploads.
pub(crate) fn discard(factory: &Arc<Factory>, agent: &str, id: i64) -> Result<(), RequestError> {
    factory.known_agent(agent)?;
    let mut inner = factory.lock();
    let runtime = inner.agents.get_mut(agent).ok_or_else(|| refused("That voice message isn't waiting any more."))?;
    let at =
        runtime.queue.iter().position(|queued| queued.id == id).ok_or_else(|| refused("That voice message isn't waiting any more."))?;
    if runtime.queue[at].voice.as_ref().is_none_or(|voice| voice.state == State::Done) {
        return Err(refused("Only a voice message that's waiting to be transcribed, or couldn't be, can be discarded."));
    }
    let dropped = runtime.queue.remove(at);
    factory.settle(&[dropped], crate::store::Outcome::Dropped);
    // The person's messages after it no longer wait for it.
    factory.wake(&mut inner, agent);
    Ok(())
}

/// The model came or went: voice messages waiting for it are transcribed
/// once it's ready, and fail when it can't be had.
pub(crate) fn model_changed(factory: &Arc<Factory>) {
    crate::speech::dictation::model_changed(factory);
    let (code, why) = match factory.speech.state() {
        ModelState::Ready => {
            factory.voices.kick();
            return;
        }
        ModelState::Downloading { .. } | ModelState::Verifying => return,
        ModelState::Absent => ("no_model", "The speech model isn't downloaded.".to_owned()),
        ModelState::Failed(why) => ("download_failed", why),
        ModelState::Unsupported(why) => ("unsupported_cpu", why),
    };
    let mut inner = factory.lock();
    let mut failed = Vec::new();
    for (agent, runtime) in &mut inner.agents {
        for queued in &mut runtime.queue {
            if let Some(voice) = queued.voice.as_mut().filter(|voice| voice.state == State::Pending) {
                *voice = Voice { state: State::Failed, error: why.clone(), error_code: code.to_owned(), ..voice.clone() };
                if let Err(error) = factory.store.set_voice(queued.id, &queued.text, voice) {
                    tracing::error!(%agent, %error, "could not record a voice message's failure");
                }
                failed.push(agent.clone());
            }
        }
    }
    failed.dedup();
    for agent in &failed {
        factory.wake(&mut inner, agent);
    }
    if failed.is_empty() {
        return;
    }
    tracing::info!(agents = failed.len(), code, "voice messages failed");
}

/// One voice message to transcribe.
struct Job {
    agent: String,
    id: i64,
    recording: PathBuf,
    program: PathBuf,
    model: PathBuf,
}

/// The oldest voice message waiting, across agents, once the model is ready.
fn next_job(factory: &Factory) -> Option<Job> {
    let model = factory.speech.ready_path()?;
    let program = factory.speech.program()?;
    let inner = factory.lock();
    let (agent, queued) = inner
        .agents
        .iter()
        .flat_map(|(agent, runtime)| runtime.queue.iter().map(move |queued| (agent, queued)))
        .filter(|(_, queued)| queued.voice.as_ref().is_some_and(|voice| voice.state == State::Pending))
        .min_by_key(|(_, queued)| queued.id)?;
    let recording = queued.attachments.first().map(|attachment| PathBuf::from(&attachment.path)).unwrap_or_default();
    Some(Job { agent: agent.clone(), id: queued.id, recording, program, model })
}

/// How long `claktory-speech` keeps the model in memory with nothing to do.
const KEEP: Duration = Duration::from_secs(5 * 60);

/// A dictation to transcribe, once the model is ready: they go first.
fn next_dictation(factory: &Factory) -> Option<(crate::speech::dictation::Job, PathBuf, PathBuf)> {
    if !factory.voices.dictations.any() {
        return None;
    }
    let model = factory.speech.ready_path()?;
    let program = factory.speech.program()?;
    Some((factory.voices.dictations.next()?, program, model))
}

/// Transcribes dictations and voice messages as they come, one at a time.
pub(crate) async fn work(factory: Weak<Factory>) {
    let Some(work) = factory.upgrade().map(|factory| {
        crate::speech::dictation::clear(&factory.home);
        factory.voices.work.clone()
    }) else {
        return;
    };
    let mut sidecar: Option<Sidecar> = None;
    loop {
        let dictation = match factory.upgrade() {
            Some(factory) => next_dictation(&factory),
            None => return,
        };
        if let Some((dictation, program, model)) = dictation {
            let outcome =
                transcribe(&mut sidecar, &program, &model, &dictation.recording, |_| {}, Some(crate::speech::dictation::MAX_MS)).await;
            match outcome {
                Ok(heard) => dictation.answer(heard.text, "", heard.duration_ms),
                Err(failure) => dictation.answer(String::new(), crate::speech::dictation::error_for(&failure.code), failure.duration_ms),
            }
            continue;
        }
        let job = match factory.upgrade() {
            Some(factory) => next_job(&factory),
            None => return,
        };
        let Some(job) = job else {
            // Nothing to do: the model's memory goes back after a while.
            let wait = if sidecar.is_some() { KEEP } else { Duration::from_secs(3600) };
            if tokio::time::timeout(wait, work.notified()).await.is_err() {
                sidecar = None;
            }
            continue;
        };
        let decoded = |duration_ms| {
            if let Some(factory) = factory.upgrade() {
                read(&factory, &job, duration_ms);
            }
        };
        let outcome = transcribe(&mut sidecar, &job.program, &job.model, &job.recording, decoded, None).await;
        let Some(factory) = factory.upgrade() else { return };
        finish(&factory, &job, outcome);
    }
}

/// What was heard.
struct Heard {
    text: String,
    language: String,
    duration_ms: u64,
}

/// Why it wasn't.
#[derive(Debug)]
struct Failure {
    code: String,
    error: String,
    /// The program stopped answering, or ended: it's started afresh.
    dead: bool,
    /// How long the recording is, when it was read.
    duration_ms: u64,
}

impl Failure {
    fn new(code: &str, error: impl Into<String>) -> Failure {
        Failure { code: code.to_owned(), error: error.into(), dead: false, duration_ms: 0 }
    }

    fn dead(code: &str, error: impl Into<String>) -> Failure {
        Failure { dead: true, ..Failure::new(code, error) }
    }
}

/// Transcribes `recording`, starting `claktory-speech` if it isn't running;
/// `decoded` hears its length once it's read, and one over `max_ms` stops
/// there (too_long).
async fn transcribe(
    sidecar: &mut Option<Sidecar>,
    program: &Path,
    model: &Path,
    recording: &Path,
    decoded: impl FnMut(u64),
    max_ms: Option<u64>,
) -> Result<Heard, Failure> {
    if sidecar.is_none() {
        *sidecar = Some(Sidecar::start(program, model).await?);
    }
    let running = sidecar.as_mut().expect("started");
    let result = running.transcribe(recording, decoded, max_ms).await;
    if result.as_ref().is_err_and(|failure| failure.dead) {
        *sidecar = None;
    }
    result
}

/// The recording was read: the app shows how long it is meanwhile.
fn read(factory: &Arc<Factory>, job: &Job, duration_ms: u64) {
    let mut inner = factory.lock();
    let Some(runtime) = inner.agents.get_mut(&job.agent) else { return };
    let Some(queued) = runtime.queue.iter_mut().find(|queued| queued.id == job.id) else { return };
    let Some(voice) = queued.voice.as_mut().filter(|voice| voice.state == State::Pending) else { return };
    voice.duration_ms = duration_ms;
    if let Err(error) = factory.store.set_voice(job.id, &queued.text, voice) {
        tracing::warn!(agent = %job.agent, %error, "could not record a voice message's length");
    }
    factory.broadcast_workspace(&inner);
}

/// Done or failed: the agent gets its words, or the person sees why not. A
/// message discarded meanwhile, or failed for want of the model, stays so.
fn finish(factory: &Arc<Factory>, job: &Job, outcome: Result<Heard, Failure>) {
    let redactor = factory.redaction.current();
    let mut inner = factory.lock();
    let Some(runtime) = inner.agents.get_mut(&job.agent) else { return };
    let Some(queued) = runtime.queue.iter_mut().find(|queued| queued.id == job.id) else { return };
    let Some(voice) = queued.voice.as_mut().filter(|voice| voice.state == State::Pending) else { return };
    match outcome {
        Ok(heard) => {
            tracing::info!(agent = %job.agent, duration_ms = heard.duration_ms, "a voice message was transcribed");
            // No secret's value reaches an agent, as in any message.
            queued.text = if redactor.is_empty() { heard.text } else { redactor.redact(&heard.text).into_owned() };
            *voice = Voice { state: State::Done, duration_ms: heard.duration_ms, language: heard.language, ..Voice::pending() };
        }
        Err(failure) => {
            tracing::warn!(agent = %job.agent, code = failure.code, error = failure.error, "a voice message wasn't transcribed");
            *voice = Voice { state: State::Failed, error: failure.error, error_code: failure.code, ..voice.clone() };
        }
    }
    if let Err(error) = factory.store.set_voice(job.id, &queued.text, voice) {
        tracing::error!(agent = %job.agent, %error, "could not record a voice message's transcription");
    }
    factory.wake(&mut inner, &job.agent);
}

/// How long loading the model may take.
const LOAD: Duration = Duration::from_secs(120);
/// How long reading a recording may take.
const DECODE: Duration = Duration::from_secs(60);
/// How long transcribing may take on top of the recording's own length:
/// the slowest computer measured (a 2-core server) took under half of it.
const SPARE: Duration = Duration::from_secs(60);

/// `claktory-speech serve`, the model loaded.
struct Sidecar {
    _child: tokio::process::Child,
    input: tokio::process::ChildStdin,
    output: tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>,
    next: u64,
}

impl Sidecar {
    async fn start(program: &Path, model: &Path) -> Result<Sidecar, Failure> {
        let mut child = tokio::process::Command::new(program)
            .arg("serve")
            .arg("--model")
            .arg(model)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| Failure::dead("transcribe_failed", format!("Couldn't start speech recognition: {error}")))?;
        let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(Failure::dead("transcribe_failed", "Couldn't start speech recognition."));
        };
        let mut output = tokio::io::BufReader::new(output).lines();
        let first = tokio::time::timeout(LOAD, output.next_line())
            .await
            .map_err(|_| Failure::dead("transcribe_failed", "Speech recognition took too long to start."))?;
        let answer: serde_json::Value = first.ok().flatten().and_then(|line| serde_json::from_str(&line).ok()).unwrap_or_default();
        if answer["ready"] != true {
            return Err(Failure::dead(&code(&answer), message(&answer, "Speech recognition didn't start.")));
        }
        tracing::info!(device = %answer["device"], load_ms = %answer["load_ms"], "speech recognition started");
        Ok(Sidecar { _child: child, input, output, next: 1 })
    }

    /// Transcribes a recording; `decoded` hears its length once it's read.
    /// One longer than `max_ms` isn't transcribed: the program is stopped
    /// (too_long), since it can't be told to drop what it's doing.
    async fn transcribe(&mut self, recording: &Path, mut decoded: impl FnMut(u64), max_ms: Option<u64>) -> Result<Heard, Failure> {
        let id = self.next;
        self.next += 1;
        let request = serde_json::json!({"id": id, "transcribe": recording});
        self.input
            .write_all(format!("{request}\n").as_bytes())
            .await
            .map_err(|_| Failure::dead("transcribe_failed", "Speech recognition stopped."))?;
        let mut deadline = tokio::time::Instant::now() + DECODE;
        loop {
            let line = match tokio::time::timeout_at(deadline, self.output.next_line()).await {
                Err(_) => return Err(Failure::dead("timed_out", "Transcribing it took too long.")),
                Ok(Ok(Some(line))) => line,
                Ok(_) => return Err(Failure::dead("transcribe_failed", "Speech recognition stopped.")),
            };
            let Ok(answer) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
            if answer["id"] != id {
                continue;
            }
            if answer["decoded"] == true {
                let duration_ms = answer["duration_ms"].as_u64().unwrap_or_default();
                if max_ms.is_some_and(|max_ms| duration_ms > max_ms) {
                    let failure = Failure::dead("too_long", "The recording is too long.");
                    return Err(Failure { duration_ms, ..failure });
                }
                decoded(duration_ms);
                deadline = tokio::time::Instant::now() + SPARE + Duration::from_millis(duration_ms);
            } else if let Some(text) = answer["text"].as_str() {
                return Ok(Heard {
                    text: text.to_owned(),
                    language: answer["language"].as_str().unwrap_or_default().to_owned(),
                    duration_ms: answer["duration_ms"].as_u64().unwrap_or_default(),
                });
            } else {
                return Err(Failure::new(&code(&answer), message(&answer, "Couldn't transcribe the recording.")));
            }
        }
    }
}

/// The error code a program reads (Transcription.error_code), from
/// `claktory-speech`'s own.
fn code(answer: &serde_json::Value) -> String {
    match answer["code"].as_str().unwrap_or_default() {
        "model" => "no_model",
        code @ ("too_long" | "no_speech" | "decode_failed" | "unsupported_cpu") => code,
        _ => "transcribe_failed",
    }
    .to_owned()
}

fn message(answer: &serde_json::Value, otherwise: &str) -> String {
    answer["error"].as_str().filter(|error| !error.is_empty()).unwrap_or(otherwise).to_owned()
}
