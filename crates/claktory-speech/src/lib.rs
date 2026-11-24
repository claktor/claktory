//! Claktory's speech recognition: a voice recording in, its words out, all
//! on this computer. The audio is decoded in Rust (symphonia: AAC in .m4a,
//! WAV, MP3, FLAC, Ogg Vorbis, AIFF, CAF), mixed down to mono and resampled
//! to 16 kHz (rubato), then transcribed by transcribe.cpp (ggml: Metal on
//! Apple silicon, the CPU elsewhere) with a speech model, Parakeet.
//!
//! factoryd runs this as a child (`claktory-speech serve`), so the C++
//! engine and the model's memory stay out of the backend; agents can run it
//! themselves (`claktory-speech transcribe`).

use std::path::Path;

use serde::Serialize;

/// The sample rate speech models take.
pub const SAMPLE_RATE: u32 = 16_000;

/// The longest recording transcribed: 30 minutes.
pub const LONGEST_MS: u64 = 30 * 60 * 1000;

/// Why a recording wasn't transcribed. `code` is for programs: factoryd
/// keeps it on the voice message (`Transcription.error_code`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Failure {
    /// `decode_failed`, `too_long`, `no_speech`, `model`, `unsupported_cpu`,
    /// `transcribe_failed` or `invalid_request`.
    pub code: &'static str,
    /// What went wrong, for people.
    pub error: String,
    /// What the decoder or engine said, for the log: never shown to people.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

impl Failure {
    pub fn new(code: &'static str, error: impl Into<String>) -> Failure {
        Failure { code, error: error.into(), detail: String::new() }
    }

    /// With what the decoder or engine said, for the log.
    #[must_use]
    pub fn with_detail(self, detail: impl Into<String>) -> Failure {
        Failure { detail: detail.into(), ..self }
    }
}

/// What a person reads when a recording can't be read: plain words, with
/// the decoder's own kept for the log (`Failure::detail`).
const DAMAGED: &str = "Couldn't read the recording. It may be damaged.";
const SILENT: &str = "Couldn't read the recording. It has no sound.";
const UNSUPPORTED: &str = "Couldn't read the recording. Its format isn't supported.";

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.error, self.code)
    }
}

/// A recording as models take it: 16 kHz mono samples in [-1, 1].
#[derive(Clone, Debug, Default)]
pub struct Audio {
    pub samples: Vec<f32>,
    /// How long the recording is, in ms.
    pub duration_ms: u64,
}

/// Decodes an audio file to 16 kHz mono. Refuses one longer than
/// `LONGEST_MS`, without decoding all of it.
pub fn decode(path: &Path) -> Result<Audio, Failure> {
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::errors::Error;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
    use symphonia::core::meta::MetadataOptions;

    let failed = |said: &str, detail: String| Failure::new("decode_failed", said).with_detail(detail);
    let damaged = |detail: String| failed(DAMAGED, detail);
    let file = std::fs::File::open(path).map_err(|error| damaged(format!("open: {error}")))?;
    let stream = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        hint.with_extension(extension);
    }
    let mut format = symphonia::default::get_probe()
        .probe(&hint, stream, FormatOptions::default(), MetadataOptions::default())
        .map_err(|error| damaged(format!("probe: {error}")))?;
    let track = format.default_track(TrackType::Audio).ok_or_else(|| failed(SILENT, "no audio track".to_owned()))?;
    let track_id = track.id;
    let params =
        track.codec_params.as_ref().and_then(|params| params.audio()).ok_or_else(|| failed(SILENT, "no audio parameters".to_owned()))?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|error| failed(UNSUPPORTED, format!("codec: {error}")))?;

    let mut rate = 0u32;
    let mut mono: Vec<f32> = Vec::new();
    let mut interleaved: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            // A stream cut short (a recording that was still being written)
            // keeps what came before.
            Err(Error::IoError(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(damaged(format!("read: {error}"))),
        };
        if packet.track_id != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // One bad packet loses a moment of sound, not the recording.
            Err(Error::DecodeError(_)) => continue,
            Err(error) => return Err(damaged(format!("decode: {error}"))),
        };
        let spec = decoded.spec();
        let channels = spec.channels().count().max(1);
        if rate == 0 {
            rate = spec.rate();
        } else if rate != spec.rate() {
            return Err(damaged("its sample rate changes partway".to_owned()));
        }
        decoded.copy_to_vec_interleaved(&mut interleaved);
        mono.extend(interleaved.chunks(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32));
        if rate > 0 && mono.len() as u64 * 1000 / u64::from(rate) > LONGEST_MS {
            return Err(Failure::new("too_long", "The recording is longer than 30 minutes."));
        }
    }
    if rate == 0 || mono.is_empty() {
        return Err(Failure::new("no_speech", "Didn't hear any speech."));
    }
    let duration_ms = mono.len() as u64 * 1000 / u64::from(rate);
    Ok(Audio { samples: resample(mono, rate).map_err(|error| failed("Couldn't read the recording.", format!("resample: {error}")))?, duration_ms })
}

/// Mono samples at `rate` to `SAMPLE_RATE`.
fn resample(samples: Vec<f32>, rate: u32) -> Result<Vec<f32>, String> {
    use rubato::audioadapter_buffers::direct::InterleavedSlice;
    use rubato::{Fft, FixedSync, Resampler as _};

    if rate == SAMPLE_RATE {
        return Ok(samples);
    }
    let frames = samples.len();
    let mut resampler =
        Fft::<f32>::new(rate as usize, SAMPLE_RATE as usize, 1024, 1, FixedSync::Input).map_err(|error| error.to_string())?;
    let input = InterleavedSlice::new(&samples[..], 1, frames).map_err(|error| error.to_string())?;
    let output = resampler.process_all(&input, frames, None).map_err(|error| error.to_string())?;
    Ok(output.take_data())
}

/// Whether this computer's processor can run the engine as built, and if
/// not why: an x86 one needs the instructions it was compiled for, which
/// some virtual servers hide.
pub fn cpu_supported() -> Result<(), Failure> {
    #[cfg(target_arch = "x86_64")]
    {
        // The floor release builds compile ggml for (scripts/build-linux.sh,
        // TRANSCRIBE_CMAKE_ARGS): AVX2, FMA and F16C on Linux; AVX and F16C
        // on Intel Macs (transcribe-cpp-sys's own macOS floor).
        let needed: &[(&str, bool)] = if cfg!(target_os = "macos") {
            &[("AVX", std::arch::is_x86_feature_detected!("avx")), ("F16C", std::arch::is_x86_feature_detected!("f16c"))]
        } else {
            &[
                ("AVX2", std::arch::is_x86_feature_detected!("avx2")),
                ("FMA", std::arch::is_x86_feature_detected!("fma")),
                ("F16C", std::arch::is_x86_feature_detected!("f16c")),
            ]
        };
        let missing: Vec<&str> = needed.iter().filter(|(_, has)| !has).map(|(name, _)| *name).collect();
        if !missing.is_empty() {
            return Err(Failure::new(
                "unsupported_cpu",
                format!("This computer's processor can't run speech recognition: it lacks {}.", missing.join(", ")),
            ));
        }
    }
    Ok(())
}

/// What a recording said.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Transcribed {
    pub text: String,
    /// The language heard, such as "pl", when the model said.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub language: String,
    pub duration_ms: u64,
    /// How long transcribing took, decoding included, in ms.
    pub ms: u64,
}

/// A speech model, loaded.
pub struct Engine {
    model: transcribe_cpp::Model,
    session: transcribe_cpp::Session,
    /// How long loading took, in ms.
    pub load_ms: u64,
}

impl Engine {
    pub fn load(path: &Path) -> Result<Engine, Failure> {
        cpu_supported()?;
        let started = std::time::Instant::now();
        let model = {
            // CLAKTORY_SPEECH_CPU=1 keeps it off the GPU: to compare, and for
            // a Mac whose Metal misbehaves.
            let cpu = std::env::var_os("CLAKTORY_SPEECH_CPU").is_some_and(|value| value == "1");
            let backend = if cpu { transcribe_cpp::Backend::Cpu } else { transcribe_cpp::Backend::Auto };
            transcribe_cpp::Model::load_with(path, &transcribe_cpp::ModelOptions { backend, ..Default::default() })
        }
        .map_err(|error| Failure::new("model", "Couldn't load the speech model.").with_detail(error.to_string()))?;
        let session = model.session().map_err(|error| Failure::new("model", "Couldn't load the speech model.").with_detail(error.to_string()))?;
        Ok(Engine { model, session, load_ms: started.elapsed().as_millis() as u64 })
    }

    /// The device it runs on, as ggml names it: "MTL0" (Metal), "CPU"…
    pub fn device(&self) -> String {
        self.model.backend()
    }

    /// Transcribes an audio file, a minute or so at a time (`windows`).
    pub fn transcribe(&mut self, path: &Path) -> Result<Transcribed, Failure> {
        let started = std::time::Instant::now();
        let audio = decode(path)?;
        self.transcribe_audio(&audio, started)
    }

    /// Transcribes a decoded recording; `started` is when its decoding began.
    pub fn transcribe_audio(&mut self, audio: &Audio, started: std::time::Instant) -> Result<Transcribed, Failure> {
        let mut said: Vec<String> = Vec::new();
        let mut language = String::new();
        for window in windows(&audio.samples) {
            let transcript = self
                .session
                .run(&audio.samples[window], &transcribe_cpp::RunOptions::default())
                .map_err(|error| Failure::new("transcribe_failed", "Couldn't transcribe the recording.").with_detail(error.to_string()))?;
            let text = transcript.text.trim();
            if !text.is_empty() {
                said.push(text.to_owned());
            }
            if language.is_empty() {
                language = transcript.language.unwrap_or_default();
            }
        }
        if said.is_empty() {
            return Err(Failure::new("no_speech", "Didn't hear any speech."));
        }
        Ok(Transcribed { text: said.join(" "), language, duration_ms: audio.duration_ms, ms: started.elapsed().as_millis() as u64 })
    }
}

/// How much audio is transcribed at once: on the CPU, memory grows by about
/// 4.4 MB a second of it, on top of ~1.1 GB (measured on Linux), so a
/// minute at a time keeps a long recording to ~1.4 GB.
const WINDOW: usize = 60 * SAMPLE_RATE as usize;
/// A window ends at the quietest moment of its last 15 seconds, so it
/// doesn't cut a word in half.
const LOOK_BACK: usize = 15 * SAMPLE_RATE as usize;
/// The moments compared: 100 ms each.
const FRAME: usize = SAMPLE_RATE as usize / 10;

/// Splits a recording into windows of at most `WINDOW`, each cut at a
/// pause. A recording up to a window long is one.
fn windows(samples: &[f32]) -> Vec<std::ops::Range<usize>> {
    let mut windows = Vec::new();
    let mut start = 0;
    while samples.len() - start > WINDOW {
        let from = start + WINDOW - LOOK_BACK;
        let quietest = (from..start + WINDOW - FRAME)
            .step_by(FRAME / 2)
            .min_by(|a, b| energy(&samples[*a..*a + FRAME]).total_cmp(&energy(&samples[*b..*b + FRAME])))
            .unwrap_or(from);
        let end = quietest + FRAME / 2;
        windows.push(start..end);
        start = end;
    }
    windows.push(start..samples.len());
    windows
}

fn energy(frame: &[f32]) -> f32 {
    frame.iter().map(|sample| sample * sample).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WAV file of a 440 Hz tone: 16-bit PCM, `channels` channels.
    fn tone(path: &Path, rate: u32, channels: u16, seconds: f32) {
        let frames = (rate as f32 * seconds) as u32;
        let data = frames * u32::from(channels) * 2;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
        bytes.extend_from_slice(&(channels * 2).to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data.to_le_bytes());
        for frame in 0..frames {
            let sample = ((frame as f32 / rate as f32 * 440.0 * std::f32::consts::TAU).sin() * 16_000.0) as i16;
            for _ in 0..channels {
                bytes.extend_from_slice(&sample.to_le_bytes());
            }
        }
        std::fs::write(path, bytes).expect("write");
    }

    #[test]
    fn a_recording_is_decoded_to_16_khz_mono() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("tone.wav");
        tone(&path, 44_100, 2, 2.0);
        let audio = decode(&path).expect("decoded");
        assert_eq!(audio.duration_ms, 2000);
        let expected = 2 * SAMPLE_RATE as usize;
        assert!(audio.samples.len().abs_diff(expected) < 50, "{} samples", audio.samples.len());
        let loudest = audio.samples.iter().fold(0f32, |most, sample| most.max(sample.abs()));
        assert!((0.4..0.6).contains(&loudest), "the tone's level survives: {loudest}");

        let path = dir.path().join("mono16k.wav");
        tone(&path, SAMPLE_RATE, 1, 0.5);
        assert_eq!(decode(&path).expect("decoded").samples.len(), 8000, "already 16 kHz: as it is");
    }

    #[test]
    fn what_isnt_a_recording_says_so() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("notes.m4a");
        std::fs::write(&path, b"not audio at all").expect("write");
        // In plain words; the decoder's own go to the log (#367, Trish).
        let damaged = decode(&path).expect_err("not audio");
        assert_eq!((damaged.code, damaged.error.as_str()), ("decode_failed", "Couldn't read the recording. It may be damaged."));
        assert!(damaged.detail.starts_with("probe: "), "{}", damaged.detail);
        let said = serde_json::to_value(&damaged).expect("json");
        assert_eq!(said["error"], "Couldn't read the recording. It may be damaged.");
        assert!(said["detail"].as_str().is_some_and(|detail| !detail.is_empty()));
        assert!(serde_json::to_value(Failure::new("too_long", "Too long.")).expect("json").get("detail").is_none());
        // A recording cut off partway through its header.
        let path = dir.path().join("cut.wav");
        tone(&path, SAMPLE_RATE, 1, 1.0);
        let bytes = std::fs::read(&path).expect("read");
        std::fs::write(&path, &bytes[..30]).expect("cut");
        let cut = decode(&path).expect_err("cut");
        assert_eq!((cut.code, cut.error.as_str()), ("decode_failed", "Couldn't read the recording. It may be damaged."));
        let missing = decode(&dir.path().join("missing.wav")).expect_err("missing");
        assert_eq!((missing.code, missing.error.as_str()), ("decode_failed", "Couldn't read the recording. It may be damaged."));
        let path = dir.path().join("empty.wav");
        tone(&path, SAMPLE_RATE, 1, 0.0);
        assert_eq!(decode(&path).expect_err("nothing in it").code, "no_speech");
    }

    /// The app's recordings: AAC in .m4a at 16 kHz mono, made here with
    /// macOS's own encoder (`afconvert`); elsewhere there's nothing to check.
    #[test]
    fn an_aac_recording_from_the_app_decodes() {
        let dir = tempfile::tempdir().expect("dir");
        let wav = dir.path().join("tone.wav");
        tone(&wav, 44_100, 1, 3.0);
        let m4a = dir.path().join("Voice.m4a");
        let made = std::process::Command::new("afconvert")
            .args(["-f", "m4af", "-d", "aac", "-r", "16000", "-c", "1"])
            .arg(&wav)
            .arg(&m4a)
            .status();
        if !made.is_ok_and(|status| status.success()) {
            return;
        }
        let audio = decode(&m4a).expect("decoded");
        assert!(audio.duration_ms.abs_diff(3000) < 100, "{} ms", audio.duration_ms);
        assert!(audio.samples.len().abs_diff(48_000) < 2000, "{} samples", audio.samples.len());
    }

    #[test]
    fn a_long_recording_is_cut_at_its_pauses() {
        let second = SAMPLE_RATE as usize;
        assert_eq!(windows(&vec![0.5; 60 * second]), vec![0..60 * second], "a minute is one");
        assert_eq!(windows(&[]), vec![0..0]);

        // Speech everywhere but for a pause at 50 s and one at 1:45.
        let mut samples: Vec<f32> = (0..150 * second).map(|n| (n as f32 / 7.0).sin() * 0.3).collect();
        for pause in [50 * second, 105 * second] {
            samples[pause..pause + second / 2].fill(0.0);
        }
        let cut = windows(&samples);
        assert_eq!(cut.len(), 3, "{cut:?}");
        assert_eq!((cut[0].start, cut[2].end), (0, samples.len()));
        for (window, pause) in cut.iter().zip([50 * second, 105 * second]) {
            assert!(window.len() <= WINDOW, "{window:?}");
            assert!((pause..pause + second / 2).contains(&window.end), "cut in the pause: {window:?}");
        }
        assert!(cut.windows(2).all(|pair| pair[0].end == pair[1].start), "nothing left out");
    }

    /// With the speech model (CLAKTORY_SPEECH_MODEL: Parakeet v3's GGUF),
    /// a tone isn't speech; without it there's nothing to check.
    #[test]
    fn a_tone_isnt_speech() {
        let Some(model) = std::env::var_os("CLAKTORY_SPEECH_MODEL") else { return };
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("tone.wav");
        tone(&path, SAMPLE_RATE, 1, 2.0);
        let mut engine = Engine::load(Path::new(&model)).expect("loads");
        match engine.transcribe(&path) {
            Err(failure) => assert_eq!(failure.code, "no_speech"),
            Ok(said) => assert!(said.text.split_whitespace().count() <= 2, "{said:?}"),
        }
    }
}
