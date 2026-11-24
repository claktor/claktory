//! `claktory-speech`: turns voice recordings into text on this computer.
//!
//! ```text
//! claktory-speech transcribe [--model FILE] [--json] RECORDING
//! claktory-speech serve --model FILE
//! claktory-speech check
//! ```
//!
//! `serve` is how factoryd uses it: it loads the model once, says
//! `{"ready": true, "device", "load_ms"}` (or `{"code", "error"}` and
//! exits), then answers each request line on standard input,
//! `{"id", "transcribe": "<path>"}`, until its input closes: first
//! `{"id", "decoded": true, "duration_ms"}` once it has read the recording
//! (so factoryd knows how long transcribing it may take), then
//! `{"id", "text", "language", "duration_ms", "ms"}`; or, instead of either,
//! `{"id", "code", "error"}`.

use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use claktory_speech::{Engine, Failure};
use serde::Deserialize;
use serde_json::{Value, json};

/// The model factoryd downloads, where it keeps it.
const MODEL_FILE: &str = "parakeet-tdt-0.6b-v3-Q8_0.gguf";

const USAGE: &str = "usage:
  claktory-speech transcribe [--model FILE] [--json] RECORDING
      Prints what the recording says (with --json: {text, language, duration_ms, ms}).
      The model is Claktory's own unless --model names another.
  claktory-speech serve --model FILE
      For factoryd: JSON lines on standard input and output.
  claktory-speech check
      Whether this computer can run speech recognition.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let verbose = args.iter().any(|arg| arg == "--verbose");
    if !verbose {
        // ggml and transcribe.cpp log every load to standard error.
        transcribe_cpp::disable_logging();
    }
    let args: Vec<&str> = args.iter().map(String::as_str).filter(|arg| *arg != "--verbose").collect();
    match args.as_slice() {
        ["transcribe", rest @ ..] => transcribe(rest),
        ["serve", "--model", model] => serve(Path::new(model)),
        ["check"] => match claktory_speech::cpu_supported() {
            Ok(()) => {
                println!("{}", json!({"supported": true}));
                ExitCode::SUCCESS
            }
            Err(failure) => {
                println!("{}", json!({"supported": false, "code": failure.code, "error": failure.error}));
                ExitCode::FAILURE
            }
        },
        ["help" | "--help" | "-h"] => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn transcribe(args: &[&str]) -> ExitCode {
    let mut model: Option<PathBuf> = None;
    let mut json_out = false;
    let mut recording: Option<&str> = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match *arg {
            "--model" => model = rest.next().map(PathBuf::from),
            "--json" => json_out = true,
            other if recording.is_none() && !other.starts_with("--") => recording = Some(other),
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let Some(recording) = recording else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let model = model.unwrap_or_else(default_model);
    let result = Engine::load(&model).and_then(|mut engine| engine.transcribe(Path::new(recording)));
    match (result, json_out) {
        (Ok(said), true) => println!("{}", serde_json::to_string(&said).unwrap_or_default()),
        (Ok(said), false) => println!("{}", said.text),
        (Err(failure), true) => {
            println!("{}", serde_json::to_string(&failure).unwrap_or_default());
            return ExitCode::FAILURE;
        }
        (Err(failure), false) => {
            eprintln!("{}", failure.error);
            if !failure.detail.is_empty() {
                eprintln!("({})", failure.detail);
            }
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

/// Claktory's own copy of the model: `~/.factory/models/speech/…`.
fn default_model() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".factory/models/speech").join(MODEL_FILE)
}

#[derive(Deserialize)]
struct Request {
    #[serde(default)]
    id: Value,
    transcribe: Option<PathBuf>,
}

fn serve(model: &Path) -> ExitCode {
    let mut out = std::io::stdout().lock();
    let mut engine = match Engine::load(model) {
        Ok(engine) => engine,
        Err(failure) => {
            let _ = writeln!(out, "{}", serde_json::to_string(&failure).unwrap_or_default());
            return ExitCode::FAILURE;
        }
    };
    let ready = json!({"ready": true, "device": engine.device(), "load_ms": engine.load_ms});
    if writeln!(out, "{ready}").and_then(|()| out.flush()).is_err() {
        return ExitCode::FAILURE;
    }
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let answer = match serde_json::from_str::<Request>(&line) {
            Ok(Request { id, transcribe: Some(path) }) => {
                match decoded(&mut out, &id, &path).and_then(|(audio, started)| engine.transcribe_audio(&audio, started)) {
                    Ok(said) => {
                        let mut answer = serde_json::to_value(&said).unwrap_or_default();
                        answer["id"] = id;
                        answer
                    }
                    Err(failure) => failed(id, &failure),
                }
            }
            Ok(Request { id, transcribe: None }) => failed(id, &Failure::new("invalid_request", "Nothing to do.")),
            Err(error) => failed(Value::Null, &Failure::new("invalid_request", error.to_string())),
        };
        if writeln!(out, "{answer}").and_then(|()| out.flush()).is_err() {
            break;
        }
    }
    ExitCode::SUCCESS
}

/// Decodes a recording for `serve`, and says how long it is.
fn decoded(out: &mut impl std::io::Write, id: &Value, path: &Path) -> Result<(claktory_speech::Audio, std::time::Instant), Failure> {
    let started = std::time::Instant::now();
    let audio = claktory_speech::decode(path)?;
    let _ = writeln!(out, "{}", json!({"id": id, "decoded": true, "duration_ms": audio.duration_ms})).and_then(|()| out.flush());
    Ok((audio, started))
}

fn failed(id: Value, failure: &Failure) -> Value {
    let mut answer = json!({"id": id, "code": failure.code, "error": failure.error});
    // For factoryd's log, never the person.
    if !failure.detail.is_empty() {
        answer["detail"] = json!(failure.detail);
    }
    answer
}
