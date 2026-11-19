use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use factory_agent::errors::{AgentFailure, Error};
use factory_agent::tools::bash::{OUTPUT_BYTES, Utf8Decoder};
use factory_agent::tools::{Tool, coding_tools};
use factory_agent::types::{ToolResult, ToolResultPart};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// How long a test waits for what should end before calling it a hang; never a
/// measure of speed, since a starved test process can stall for seconds.
const HANG: Duration = Duration::from_secs(60);

fn bash(dir: &Path) -> Tool {
    coding_tools(dir, &[], None).into_iter().find(|tool| tool.spec.name == "bash").expect("bash")
}

fn text(result: &ToolResult) -> String {
    result
        .content
        .iter()
        .map(|part| match part {
            ToolResultPart::Text { text } => text.clone(),
            ToolResultPart::Image { .. } => String::new(),
        })
        .collect()
}

/// The result's text with how long the command took left out of its last
/// line: "[exit 0, 0.1s]" reads "[exit 0]".
fn shown(result: &ToolResult) -> String {
    let text = text(result);
    let Some(start) = text.rfind("\n[") else { return text };
    let (body, last) = text.split_at(start);
    match last.rfind(", ") {
        Some(comma) if last.ends_with("s]") => format!("{body}{}]", &last[..comma]),
        _ => text,
    }
}

fn no_update(_: ToolResult) {}

async fn run(dir: &Path, args: Value) -> Result<ToolResult, Error> {
    bash(dir).execute(&args, &CancellationToken::new(), &no_update).await
}

async fn run_collecting(dir: &Path, args: Value) -> (ToolResult, Vec<ToolResult>) {
    let updates = Arc::new(Mutex::new(Vec::new()));
    let sink = updates.clone();
    let result = bash(dir).execute(&args, &CancellationToken::new(), &move |update| sink.lock().push(update)).await.expect("result");
    let collected = updates.lock().clone();
    (result, collected)
}

#[tokio::test]
async fn the_shell_reports_exit_status_signals_and_interleaved_output() {
    let dir = tempfile::tempdir().unwrap();
    let result = run(dir.path(), json!({"command": "printf 'hello'; printf 'oops' >&2; exit 7"})).await.unwrap();
    assert_eq!(result.details["exitCode"], 7);
    let output = shown(&result);
    assert!(output.contains("hello") && output.contains("oops") && output.ends_with("\n[exit 7]"), "{output}");
    assert!(text(&result).ends_with("s]"), "how long it took");
    let plain = run(dir.path(), json!({"command": "exit 3"})).await.unwrap();
    assert_eq!(shown(&plain), "\n[exit 3]");
    assert_eq!(plain.details, json!({"exitCode": 3, "exitSignal": null, "stopped": null, "characters": 0, "outputBytes": 0}));
    let killed = run(dir.path(), json!({"command": "kill -TERM $$"})).await.unwrap();
    assert_eq!(shown(&killed), "\n[signal SIGTERM]");
    assert_eq!(killed.details["exitSignal"], "SIGTERM");
    assert_eq!(killed.details["exitCode"], Value::Null);
    let both = run(dir.path(), json!({"command": "printf 'a\\nb'; printf 'c' >&2", "timeout": null})).await.unwrap();
    assert_eq!(shown(&both), "a\nbc\n[exit 0]");
    assert_eq!(both.details["characters"], 4);
}

#[tokio::test]
async fn large_output_streams_in_updates_and_the_result_keeps_the_tail() {
    let dir = tempfile::tempdir().unwrap();
    let (result, updates) = run_collecting(dir.path(), json!({"command": "head -c 30000 /dev/zero | tr '\\0' x"})).await;
    assert!(text(&result).starts_with("[Earlier output truncated]\n"));
    assert_eq!(shown(&result).len(), "[Earlier output truncated]\n".len() + 24_000 + "\n[exit 0]".len());
    let streamed: usize = updates.iter().map(|update| text(update).len()).sum();
    assert_eq!(streamed, 30_000);
    assert!(updates.iter().filter(|update| !text(update).is_empty()).all(|update| update.details["stream"] == "stdout"));
    assert!(updates.iter().any(|update| text(update).is_empty() && update.details["stream"] == "stderr"));
    assert_eq!(result.details["characters"], 30_000);
    assert_eq!(result.details["outputBytes"], 30_000);
}

#[tokio::test]
async fn timeouts_and_cancellation_kill_the_process_group() {
    let dir = tempfile::tempdir().unwrap();
    let command = "(trap '' TERM; sleep 1; touch escaped) & wait";
    let timed = run(dir.path(), json!({"command": command, "timeout": 0.05})).await.unwrap();
    assert_eq!(timed.details["stopped"], "timed out");
    assert!(shown(&timed).ends_with("\n[timed out]"));
    let token = CancellationToken::new();
    let cancel = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();
    });
    let aborted = bash(dir.path()).execute(&json!({"command": command}), &token, &no_update).await.unwrap();
    assert_eq!(aborted.details["stopped"], "aborted");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(!dir.path().join("escaped").exists());
    let already = CancellationToken::new();
    already.cancel();
    let error = bash(dir.path()).execute(&json!({"command": "touch bad"}), &already, &no_update).await.unwrap_err();
    assert!(matches!(error, Error::Aborted));
    assert!(!dir.path().join("bad").exists());
}

#[tokio::test]
async fn a_bare_bash_or_sh_invocation_is_rejected_instead_of_spawned() {
    let dir = tempfile::tempdir().unwrap();
    for command in ["bash -lc", "sh -c", "bash -c ''", "  /bin/sh   -c \"\"  "] {
        let error = run(dir.path(), json!({"command": command})).await.unwrap_err();
        assert!(matches!(error.failure(), Some(AgentFailure::BashWrappedInvocation { .. })), "{command}");
        assert!(error.to_string().contains("script text itself in command"));
    }
    let ran = run(dir.path(), json!({"command": "printf ok"})).await.unwrap();
    assert_eq!(ran.details["exitCode"], 0);
    assert!(text(&ran).contains("ok"));
    let wrapped = run(dir.path(), json!({"command": "bash -lc 'printf ok'"})).await.unwrap();
    assert_eq!(wrapped.details["exitCode"], 0);
    assert!(text(&wrapped).contains("ok"));
}

#[tokio::test]
async fn combined_output_is_bounded_and_the_next_command_can_run() {
    let dir = tempfile::tempdir().unwrap();
    let flood = "while true; do head -c 65536 /dev/zero | tr '\\0' x; head -c 65536 /dev/zero | tr '\\0' x >&2; done";
    let (result, updates) = run_collecting(dir.path(), json!({"command": flood})).await;
    assert_eq!(result.details["stopped"], "output limit exceeded");
    let streamed: usize = updates.iter().map(|update| text(update).len()).sum();
    assert_eq!(streamed, OUTPUT_BYTES);
    assert!(shown(&result).ends_with("\n[output limit exceeded]"));
    let next = run(dir.path(), json!({"command": "printf recovered"})).await.unwrap();
    assert_eq!(next.details["exitCode"], 0);
    assert!(text(&next).contains("recovered"));
}

#[tokio::test]
async fn commands_can_write_build_artifacts_larger_than_128_mib() {
    #[derive(serde::Serialize)]
    struct BashCommand<'a> {
        command: &'a str,
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Status {
        exit_code: Option<i32>,
    }
    let dir = tempfile::tempdir().unwrap();
    let args =
        serde_json::to_value(BashCommand { command: "dd if=/dev/zero of=artifact bs=1 seek=268435456 count=1 2>/dev/null" }).unwrap();
    let result = run(dir.path(), args).await.unwrap();
    let status: Status = serde_json::from_value(result.details).unwrap();
    assert_eq!(status.exit_code, Some(0));
    assert_eq!(std::fs::metadata(dir.path().join("artifact")).unwrap().len(), 268_435_457);
}

#[test]
fn the_decoder_joins_split_sequences_and_replaces_invalid_bytes() {
    let mut decoder = Utf8Decoder::default();
    assert_eq!(decoder.write(b"ab\xf0\x9f"), "ab");
    assert_eq!(decoder.write(b"\x98\x80c"), "😀c");
    assert_eq!(decoder.write(b"\xffz"), "\u{fffd}z");
    assert_eq!(decoder.write(b"\xe2\x82"), "");
    assert_eq!(decoder.end(), "\u{fffd}");
    assert_eq!(decoder.end(), "");
}

#[tokio::test]
async fn shell_failures_cannot_be_hidden_by_later_commands_or_pipeline_consumers() {
    #[derive(serde::Serialize)]
    struct Script<'a> {
        command: &'a str,
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Status {
        exit_code: i32,
    }
    let dir = tempfile::tempdir().unwrap();
    for (command, expected) in [
        ("sh -c 'printf broken >&2; exit 7' | cat; touch hidden", 7),
        ("false; touch hidden", 1),
        ("if false; then touch hidden; fi; false || printf handled", 0),
        ("yes | head -n 1", 141),
        ("printf 'one\\ntwo\\n' | sed -n '1p'", 0),
    ] {
        let result = run(dir.path(), serde_json::to_value(Script { command }).unwrap()).await.unwrap();
        let status: Status = serde_json::from_value(result.details).unwrap();
        assert_eq!(status.exit_code, expected, "{command}");
        assert!(!dir.path().join("hidden").exists());
    }
}

#[tokio::test]
async fn a_background_command_returns_at_once_and_reports_when_it_finishes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (sender, mut finished) = tokio::sync::mpsc::unbounded_channel();
    let started: Arc<Mutex<Vec<factory_agent::Task>>> = Arc::default();
    let background = factory_agent::Background {
        dir: dir.path().join("tasks"),
        started: {
            let started = started.clone();
            Arc::new(move |task: &factory_agent::Task| started.lock().push(task.clone()))
        },
        finished: Arc::new(move |done| {
            let _ = sender.send(done);
        }),
    };
    let tools = coding_tools(dir.path(), &[], Some(background));
    let bash = tools.into_iter().find(|tool| tool.spec.name == "bash").expect("bash");
    // The command can't end before the test makes `go`, so returning at all means bash didn't wait for it.
    let command = "echo first; until [ -e go ]; do sleep 0.01; done; echo second; exit 4";
    let args = serde_json::json!({"command": command, "run_in_background": true});
    let result = tokio::time::timeout(HANG, bash.execute(&args, &CancellationToken::new(), &|_| {}))
        .await
        .expect("it didn't wait for the command")
        .expect("started");
    assert_eq!(result.details["task"], "task-1");
    std::fs::write(dir.path().join("go"), "").expect("go");
    let done = tokio::time::timeout(HANG, finished.recv()).await.expect("finished in time").expect("finished");
    assert_eq!(done.id, "task-1");
    assert_eq!(done.status, "exit 4");
    assert_eq!(done.tail, "first\nsecond\n");
    assert_eq!(std::fs::read_to_string(&done.log).expect("log"), "first\nsecond\n");
    assert!(
        done.summary().starts_with("task-1 finished (exit 4, ") && done.summary().ends_with(&format!("s): {command}")),
        "{}",
        done.summary()
    );
    assert_eq!(started.lock()[0].log, done.log, "the program running the agent heard it start");

    // The next task gets the next number.
    let result = bash.execute(&args, &CancellationToken::new(), &|_| {}).await.expect("started");
    assert_eq!(result.details["task"], "task-2");
}

#[tokio::test]
async fn a_background_task_outlives_whoever_started_it_and_can_be_picked_up_again() {
    let dir = tempfile::tempdir().expect("tempdir");
    let started: Arc<Mutex<Vec<factory_agent::Task>>> = Arc::default();
    let background = factory_agent::Background {
        dir: dir.path().join("tasks"),
        started: {
            let started = started.clone();
            Arc::new(move |task: &factory_agent::Task| started.lock().push(task.clone()))
        },
        // Whoever started it is gone before it ends.
        finished: Arc::new(|_| {}),
    };
    let tools = coding_tools(dir.path(), &[], Some(background));
    let bash = tools.into_iter().find(|tool| tool.spec.name == "bash").expect("bash");
    let args = json!({"command": "sleep 1.2; echo late; exit 7", "run_in_background": true});
    bash.execute(&args, &CancellationToken::new(), &|_| {}).await.expect("started");
    let task = started.lock()[0].clone();
    let done = tokio::time::timeout(HANG, task.wait_detached()).await.expect("ended in time");
    assert_eq!(done.status, "exit 7");
    assert_eq!(done.tail, "late\n");
}

/// A Background whose started tasks and finished events the test can see.
struct Watched {
    background: factory_agent::Background,
    started: Arc<Mutex<Vec<factory_agent::Task>>>,
    finished: tokio::sync::mpsc::UnboundedReceiver<factory_agent::Finished>,
}

fn watched(dir: &Path) -> Watched {
    let (sender, finished) = tokio::sync::mpsc::unbounded_channel();
    let started: Arc<Mutex<Vec<factory_agent::Task>>> = Arc::default();
    let background = factory_agent::Background {
        dir: dir.join("tasks"),
        started: {
            let started = started.clone();
            Arc::new(move |task: &factory_agent::Task| started.lock().push(task.clone()))
        },
        finished: Arc::new(move |done| {
            let _ = sender.send(done);
        }),
    };
    Watched { background, started, finished }
}

fn bash_with(dir: &Path, background: &factory_agent::Background) -> Tool {
    coding_tools(dir, &[], Some(background.clone())).into_iter().find(|tool| tool.spec.name == "bash").expect("bash")
}

#[tokio::test]
async fn foreground_output_goes_through_a_log_that_short_output_leaves_behind_and_long_output_keeps() {
    let dir = tempfile::tempdir().expect("tempdir");
    let watched = watched(dir.path());
    let bash = bash_with(dir.path(), &watched.background);
    let updates = Arc::new(Mutex::new(String::new()));
    let sink = updates.clone();
    let update = move |update: ToolResult| sink.lock().push_str(&text(&update));
    let result = bash
        .execute(&json!({"command": "printf 'hello'; printf 'oops' >&2; exit 7"}), &CancellationToken::new(), &update)
        .await
        .expect("ran");
    assert_eq!(shown(&result), "hellooops\n[exit 7]");
    assert_eq!((result.details["exitCode"].clone(), result.details["log"].clone()), (json!(7), Value::Null));
    assert_eq!(*updates.lock(), "hellooops");
    assert_eq!(std::fs::read_dir(dir.path().join("tasks")).expect("tasks").count(), 0, "its log and status are gone");
    let killed = bash.execute(&json!({"command": "kill -TERM $$"}), &CancellationToken::new(), &no_update).await.expect("ran");
    assert!(shown(&killed).ends_with("\n[signal SIGTERM]"), "{}", shown(&killed));

    // Past 30,000 characters: its start and end, and where the whole output is.
    let long = bash
        .execute(&json!({"command": "head -c 50000 /dev/zero | tr '\\0' x; echo; echo done"}), &CancellationToken::new(), &no_update)
        .await
        .expect("ran");
    let output = shown(&long);
    let log = long.details["log"].as_str().expect("a log").to_owned();
    assert!(
        output.starts_with(&"x".repeat(2_000)) && output.contains(&format!("characters left out; the whole output is in {log}]")),
        "{}",
        &output[..2_200]
    );
    assert!(output.ends_with("x\ndone\n\n[exit 0]"));
    assert_eq!(std::fs::metadata(&log).expect("kept").len(), 50_006);
    assert!(watched.started.lock().is_empty(), "nothing went to the background");
}

#[tokio::test]
async fn a_command_past_its_timeout_moves_to_the_background_and_reports_when_it_ends() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut watched = watched(dir.path());
    let bash = bash_with(dir.path(), &watched.background);
    let command = "echo first; until [ -e go ]; do sleep 0.01; done; echo second; exit 4";
    let result =
        tokio::time::timeout(HANG, bash.execute(&json!({"command": command, "timeout": 0.3}), &CancellationToken::new(), &no_update))
            .await
            .expect("it didn't wait for the command")
            .expect("moved");
    let moved = text(&result);
    assert_eq!(result.details["movedToBackground"], "task-1");
    assert!(moved.contains("moved to the background as task-1") && moved.contains("find out why it's slow"), "{moved}");
    assert!(moved.contains("Output so far ends with:\nfirst\n"), "{moved}");
    assert_eq!(watched.started.lock()[0].id, "task-1");
    std::fs::write(dir.path().join("go"), "").expect("go");
    let done = tokio::time::timeout(HANG, watched.finished.recv()).await.expect("finished in time").expect("finished");
    assert_eq!((done.id.as_str(), done.status.as_str(), done.tail.as_str()), ("task-1", "exit 4", "first\nsecond\n"));
    assert!(done.seconds.is_some_and(|seconds| seconds >= 0.3), "{:?}", done.seconds);
    assert!(done.text().starts_with("Background task-1 finished (exit 4, "), "{}", done.text());
}

#[tokio::test]
async fn a_command_that_only_sleeps_is_stopped_at_its_timeout_and_cancel_stops_any() {
    let dir = tempfile::tempdir().expect("tempdir");
    let watched = watched(dir.path());
    let bash = bash_with(dir.path(), &watched.background);
    let slept =
        bash.execute(&json!({"command": "sleep 5; touch late", "timeout": 0.2}), &CancellationToken::new(), &no_update).await.expect("ran");
    assert_eq!(slept.details["stopped"], "timed out");
    assert!(shown(&slept).ends_with("\n[timed out]"));
    assert!(watched.started.lock().is_empty());
    let token = CancellationToken::new();
    let cancel = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();
    });
    let aborted = bash.execute(&json!({"command": "until [ -e never ]; do sleep 0.01; done"}), &token, &no_update).await.expect("ran");
    assert_eq!(aborted.details["stopped"], "aborted");
    assert!(watched.started.lock().is_empty());
    assert!(!dir.path().join("late").exists());
}

#[tokio::test]
async fn a_background_command_is_stopped_at_its_time_limit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut watched = watched(dir.path());
    let bash = bash_with(dir.path(), &watched.background);
    let args = json!({"command": "until [ -e never ]; do sleep 0.01; done", "run_in_background": true, "timeout": 0.2});
    let started = bash.execute(&args, &CancellationToken::new(), &no_update).await.expect("started");
    assert!(text(&started).contains("It's stopped if it runs past 0.2s."), "{}", text(&started));
    let done = tokio::time::timeout(HANG, watched.finished.recv()).await.expect("finished in time").expect("finished");
    assert_eq!(done.status, "stopped at its time limit (0.2s)");
    assert!(done.text().contains("only if the work still needs it"), "{}", done.text());
}

#[test]
fn durations_read_at_a_glance() {
    use factory_agent::tools::bash::took;
    assert_eq!((took(0.04), took(4.21), took(59.9)), ("0.0s".to_owned(), "4.2s".to_owned(), "59.9s".to_owned()));
    assert_eq!((took(187.0), took(3_725.0)), ("3m 07s".to_owned(), "1h 02m".to_owned()));
}
