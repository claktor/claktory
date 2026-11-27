//! The speech model voice messages are transcribed with: downloaded on
//! first use (InstallSpeechModel, after the app asks the person), from a
//! pinned revision, resumable, checked against its SHA-256 before it's
//! used, and kept in `<home>/models/speech/` with a receipt so it isn't
//! hashed again at every start. Nothing here runs it: that's
//! `claktory-speech`, beside factoryd, which also says whether this
//! computer can.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use factory_protocol::v1;
use serde::{Deserialize, Serialize};

use crate::factory::{Factory, RequestError};

/// What's downloaded: one file, its size and SHA-256.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artifact {
    pub url: String,
    pub file: String,
    pub bytes: u64,
    /// Lowercase hex.
    pub sha256: String,
}

impl Artifact {
    /// Parakeet TDT 0.6B v3 at Q8_0, as transcribe.cpp's catalog has it:
    /// 25 European languages, which it tells apart itself.
    pub fn parakeet() -> Artifact {
        Artifact {
            url: "https://huggingface.co/handy-computer/parakeet-tdt-0.6b-v3-gguf/resolve/d8d8c32cb2017c57e5307767343e97870f8a7f53/parakeet-tdt-0.6b-v3-Q8_0.gguf".to_owned(),
            file: "parakeet-tdt-0.6b-v3-Q8_0.gguf".to_owned(),
            bytes: 739_508_576,
            sha256: "5859f77944efcd8eafa23a6350731960b2b55b2203df51f319665c807d802cc7".to_owned(),
        }
    }
}

/// How the app names the model.
pub(crate) const NAME: &str = "Parakeet v3 (25 languages)";
/// Its licence's credit (CC BY 4.0), shown where it's named.
pub(crate) const ATTRIBUTION: &str = "Parakeet TDT 0.6B v3 by NVIDIA, CC BY 4.0, converted to GGUF by handy-computer.";

/// What a download leaves free on the disk at least, as uploads do.
const SPARE: u64 = 1_000_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ModelState {
    Absent,
    Downloading { bytes: u64 },
    Verifying,
    Ready,
    Failed(String),
    Unsupported(String),
}

/// Kept beside the model: what it was when it was checked, so a start
/// needn't hash 740 MB again unless it changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Receipt {
    bytes: u64,
    sha256: String,
    modified_ms: i64,
}

pub(crate) struct Model {
    dir: PathBuf,
    inner: Mutex<Inner>,
}

struct Inner {
    artifact: Artifact,
    /// `claktory-speech`, when it's here.
    program: Option<PathBuf>,
    state: ModelState,
    download: Option<tokio::task::JoinHandle<()>>,
    /// Counts downloads and removals, so a download that was stopped and
    /// replaced doesn't write over the newer one's state.
    generation: u64,
}

/// Where `claktory-speech` is: `FACTORY_SPEECH`, or beside factoryd's own
/// program (links resolved), or else this version's in the
/// home's releases, then the installed one in its `bin`, for a backend
/// started from somewhere else.
pub(crate) fn program(home: &crate::home::Home) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("FACTORY_SPEECH").filter(|path| !path.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let name = crate::update::SPEECH_HOST;
    crate::update::beside_backend(name).or_else(|| {
        [home.releases().join(crate::update::VERSION).join(name), home.root().join("bin").join(name)]
            .into_iter()
            .find(|path| path.is_file())
    })
}

/// Why this computer can't, when there's no `claktory-speech` to run it.
const NO_PROGRAM: &str = "Speech recognition isn't installed with this Claktory.";

impl Model {
    pub(crate) fn new(home: &crate::home::Home) -> Model {
        let model = Model {
            dir: home.root().join("models").join("speech"),
            inner: Mutex::new(Inner {
                artifact: Artifact::parakeet(),
                program: program(home),
                state: ModelState::Absent,
                download: None,
                generation: 0,
            }),
        };
        model.settle();
        model
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn program(&self) -> Option<PathBuf> {
        self.lock().program.clone()
    }

    /// Another model or program than Claktory's own: tests, and mirrors.
    pub(crate) fn set_source(&self, program: Option<PathBuf>, artifact: Artifact) {
        {
            let mut inner = self.lock();
            if let Some(download) = inner.download.take() {
                download.abort();
            }
            inner.generation += 1;
            inner.program = program;
            inner.artifact = artifact;
            inner.state = ModelState::Absent;
        }
        self.settle();
    }

    /// The state from what's on disk: ready when the file matches its
    /// receipt; unsupported without the program.
    fn settle(&self) {
        let mut inner = self.lock();
        if inner.program.is_none() {
            inner.state = ModelState::Unsupported(NO_PROGRAM.to_owned());
            return;
        }
        let path = self.dir.join(&inner.artifact.file);
        let receipt: Option<Receipt> = std::fs::read(receipt_path(&path)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let matches = receipt.is_some_and(|receipt| {
            receipt.sha256 == inner.artifact.sha256
                && receipt.bytes == inner.artifact.bytes
                && file_facts(&path) == Some((receipt.bytes, receipt.modified_ms))
        });
        inner.state = if matches { ModelState::Ready } else { ModelState::Absent };
    }

    pub(crate) fn state(&self) -> ModelState {
        self.lock().state.clone()
    }

    /// The model's file, when it's ready.
    pub(crate) fn ready_path(&self) -> Option<PathBuf> {
        let inner = self.lock();
        (inner.state == ModelState::Ready).then(|| self.dir.join(&inner.artifact.file))
    }

    /// A download was started and not finished: absent, with part of it here.
    fn started(&self) -> bool {
        let inner = self.lock();
        inner.state == ModelState::Absent && partial_path(&self.dir.join(&inner.artifact.file)).exists()
    }

    pub(crate) fn view(&self) -> v1::SpeechModel {
        let inner = self.lock();
        let total = inner.artifact.bytes;
        let (state, bytes, error) = match &inner.state {
            ModelState::Absent => (v1::SpeechModelState::Absent, 0, String::new()),
            ModelState::Downloading { bytes } => (v1::SpeechModelState::Downloading, *bytes, String::new()),
            ModelState::Verifying => (v1::SpeechModelState::Verifying, total, String::new()),
            ModelState::Ready => (v1::SpeechModelState::Ready, total, String::new()),
            ModelState::Failed(why) => (v1::SpeechModelState::Failed, 0, why.clone()),
            ModelState::Unsupported(why) => (v1::SpeechModelState::Unsupported, 0, why.clone()),
        };
        v1::SpeechModel { state: state.into(), name: NAME.to_owned(), bytes, total, error, attribution: ATTRIBUTION.to_owned() }
    }
}

fn receipt_path(model: &Path) -> PathBuf {
    model.with_extension("receipt")
}

fn partial_path(model: &Path) -> PathBuf {
    model.with_extension("gguf.partial")
}

/// A file's size and modification time (ms), if it's there.
fn file_facts(path: &Path) -> Option<(u64, i64)> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as i64;
    Some((metadata.len(), modified))
}

fn refused(why: impl Into<String>) -> RequestError {
    RequestError::Account(v1::ErrorCode::InvalidRequest, why.into())
}

/// "740 MB".
fn mb(bytes: u64) -> String {
    format!("{} MB", bytes.div_ceil(1_000_000))
}

/// Checks this computer can run speech recognition (`claktory-speech
/// check`), once at start: a processor it can't run on makes the model
/// unsupported, so it's never downloaded. One it can carries on with a
/// download a restart stopped (the person already said yes to it).
pub(crate) async fn check(factory: Weak<Factory>) {
    let Some(program) = factory.upgrade().and_then(|factory| factory.speech.program()) else { return };
    let output = tokio::process::Command::new(&program).arg("check").kill_on_drop(true).output().await;
    let why = match output {
        Ok(output) if output.status.success() => {
            let Some(factory) = factory.upgrade() else { return };
            if factory.speech.started()
                && let Err(error) = install(&factory)
            {
                tracing::warn!(?error, "the speech model's download didn't carry on");
            }
            // Voice messages left waiting at the last stop go on, or fail.
            crate::speech::voice::model_changed(&factory);
            return;
        }
        Ok(output) => serde_json::from_slice::<serde_json::Value>(&output.stdout)
            .ok()
            .and_then(|answer| answer["error"].as_str().map(str::to_owned))
            .unwrap_or_else(|| "This computer can't run speech recognition.".to_owned()),
        Err(error) => {
            tracing::warn!(%error, program = %program.display(), "claktory-speech didn't run");
            NO_PROGRAM.to_owned()
        }
    };
    let Some(factory) = factory.upgrade() else { return };
    tracing::info!(why, "speech recognition isn't available");
    factory.speech.lock().state = ModelState::Unsupported(why);
    factory.broadcast_workspace(&factory.lock());
    crate::speech::voice::model_changed(&factory);
}

/// InstallSpeechModel: starts (or resumes) the download. Refused when this
/// computer can't run it, or there isn't room for it.
pub(crate) fn install(factory: &Arc<Factory>) -> Result<(), RequestError> {
    let model = &factory.speech;
    let (path, artifact, generation) = {
        let mut inner = model.lock();
        match &inner.state {
            ModelState::Unsupported(why) => return Err(refused(why.clone())),
            ModelState::Downloading { .. } | ModelState::Verifying | ModelState::Ready => return Ok(()),
            ModelState::Absent | ModelState::Failed(_) => {}
        }
        std::fs::create_dir_all(&model.dir).map_err(|error| refused(format!("Can't make {}: {error}", model.dir.display())))?;
        let path = model.dir.join(&inner.artifact.file);
        let have = std::fs::metadata(partial_path(&path)).map_or(0, |metadata| metadata.len());
        let left = inner.artifact.bytes.saturating_sub(have);
        if let Some(free) = crate::uploads::available(&model.dir)
            && free < left + SPARE
        {
            return Err(refused(format!(
                "There isn't room for the speech model ({}) on the computer Claktory runs on: {} free, and it leaves 1 GB.",
                mb(left),
                mb(free)
            )));
        }
        inner.generation += 1;
        inner.state = ModelState::Downloading { bytes: have };
        (path, inner.artifact.clone(), inner.generation)
    };
    tracing::info!(url = artifact.url, "downloading the speech model");
    let weak = Arc::downgrade(factory);
    let handle = tokio::spawn(async move {
        let result = download(&weak, &path, &artifact, generation).await;
        let Some(factory) = weak.upgrade() else { return };
        let state = match result {
            Ok(()) => {
                tracing::info!("the speech model is ready");
                ModelState::Ready
            }
            Err(why) => {
                tracing::warn!(why, "the speech model didn't download");
                ModelState::Failed(why)
            }
        };
        {
            let mut inner = factory.speech.lock();
            if inner.generation != generation {
                return;
            }
            inner.state = state;
            inner.download = None;
        }
        factory.broadcast_workspace(&factory.lock());
        crate::speech::voice::model_changed(&factory);
    });
    model.lock().download = Some(handle);
    factory.broadcast_workspace(&factory.lock());
    Ok(())
}

/// RemoveSpeechModel: stops a download and deletes the model.
pub(crate) fn remove(factory: &Arc<Factory>) -> Result<(), RequestError> {
    let model = &factory.speech;
    {
        let mut inner = model.lock();
        if let Some(download) = inner.download.take() {
            download.abort();
        }
        inner.generation += 1;
        let path = model.dir.join(&inner.artifact.file);
        for file in [partial_path(&path), receipt_path(&path), path] {
            match std::fs::remove_file(&file) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(refused(format!("Can't delete {}: {error}", file.display()))),
            }
        }
        if !matches!(inner.state, ModelState::Unsupported(_)) {
            inner.state = ModelState::Absent;
        }
    }
    tracing::info!("the speech model was removed");
    factory.broadcast_workspace(&factory.lock());
    crate::speech::voice::model_changed(factory);
    Ok(())
}

/// How often the app hears the download's progress, at most.
const PROGRESS_EVERY: Duration = Duration::from_secs(1);

/// Downloads to `<file>.partial` from where it stopped, checks its size and
/// SHA-256, then puts it in place with its receipt.
async fn download(factory: &Weak<Factory>, path: &Path, artifact: &Artifact, generation: u64) -> Result<(), String> {
    use futures_util::StreamExt as _;
    use tokio::io::AsyncWriteExt as _;

    let partial = partial_path(path);
    let mut have = std::fs::metadata(&partial).map_or(0, |metadata| metadata.len());
    if have > artifact.bytes {
        let _ = std::fs::remove_file(&partial);
        have = 0;
    }
    if have < artifact.bytes {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .read_timeout(Duration::from_secs(60))
            .user_agent(format!("Claktory/{}", crate::update::VERSION))
            .build()
            .map_err(|error| error.to_string())?;
        let mut request = client.get(&artifact.url);
        if have > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let response = request.send().await.map_err(|error| format!("Couldn't reach {}: {error}", host(&artifact.url)))?;
        let resumed = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        if !response.status().is_success() {
            return Err(format!("{} answered {}.", host(&artifact.url), response.status()));
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(resumed)
            .truncate(!resumed)
            .open(&partial)
            .await
            .map_err(|error| format!("Can't write {}: {error}", partial.display()))?;
        if !resumed {
            have = 0;
        }
        let mut told = Instant::now();
        let mut stream = response.bytes_stream();
        // A connection dropped midway ends the download like a short
        // answer does: what came is kept, to carry on from.
        while let Some(Ok(chunk)) = stream.next().await {
            have += chunk.len() as u64;
            if have > artifact.bytes {
                drop(file);
                let _ = std::fs::remove_file(&partial);
                return Err("The download was bigger than the model is. Try again.".to_owned());
            }
            file.write_all(&chunk).await.map_err(|error| format!("Can't write {}: {error}", partial.display()))?;
            if told.elapsed() >= PROGRESS_EVERY {
                told = Instant::now();
                progress(factory, generation, ModelState::Downloading { bytes: have });
            }
        }
        file.flush().await.map_err(|error| error.to_string())?;
        file.sync_all().await.map_err(|error| error.to_string())?;
        if have < artifact.bytes {
            return Err(format!("The download stopped at {} of {}. Try again to carry on.", mb(have), mb(artifact.bytes)));
        }
    }
    progress(factory, generation, ModelState::Verifying);
    let sha256 = {
        let partial = partial.clone();
        tokio::task::spawn_blocking(move || hash_file(&partial)).await.map_err(|error| error.to_string())?
    }
    .map_err(|error| format!("Can't read {}: {error}", partial.display()))?;
    if sha256 != artifact.sha256 {
        let _ = std::fs::remove_file(&partial);
        return Err("The download was damaged. Try again.".to_owned());
    }
    std::fs::rename(&partial, path).map_err(|error| format!("Can't put the model in place: {error}"))?;
    let (bytes, modified_ms) = file_facts(path).ok_or("The model went missing.")?;
    let receipt = Receipt { bytes, sha256, modified_ms };
    std::fs::write(receipt_path(path), serde_json::to_vec_pretty(&receipt).unwrap_or_default())
        .map_err(|error| format!("Can't write the model's receipt: {error}"))?;
    Ok(())
}

/// Shows the download's progress, unless it was replaced.
fn progress(factory: &Weak<Factory>, generation: u64, state: ModelState) {
    let Some(factory) = factory.upgrade() else { return };
    {
        let mut inner = factory.speech.lock();
        if inner.generation != generation {
            return;
        }
        inner.state = state;
    }
    factory.broadcast_workspace(&factory.lock());
}

fn hash_file(path: &Path) -> std::io::Result<String> {
    crate::encoding::sha256_file(path, 1 << 20)
}

/// "huggingface.co", for messages.
fn host(url: &str) -> &str {
    url.split("://").nth(1).and_then(|rest| rest.split('/').next()).unwrap_or(url)
}

#[cfg(test)]
mod program_tests {
    use super::program;

    /// A backend started from somewhere else (through a link on PATH, say)
    /// still finds this version's speech host in the home's releases, else
    /// the installed one in its bin.
    #[test]
    fn a_backend_started_from_elsewhere_finds_the_installed_speech_host() {
        if std::env::var_os("FACTORY_SPEECH").is_some() || crate::update::beside_backend(crate::update::SPEECH_HOST).is_some() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("factoryd-speech-program-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = crate::home::Home::new(dir.clone());
        assert_eq!(program(&home), None);
        let installed = dir.join("bin").join(crate::update::SPEECH_HOST);
        std::fs::create_dir_all(installed.parent().expect("bin")).expect("bin");
        std::fs::write(&installed, "#!/bin/sh\n").expect("installed");
        assert_eq!(program(&home), Some(installed));
        let released = home.releases().join(crate::update::VERSION).join(crate::update::SPEECH_HOST);
        std::fs::create_dir_all(released.parent().expect("release")).expect("release");
        std::fs::write(&released, "#!/bin/sh\n").expect("released");
        assert_eq!(program(&home), Some(released));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
