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
    // On Windows each stream has its own reader thread, and Git Bash's
    // printf writes a line at a time, so where stderr's write lands among
    // stdout's isn't fixed: stdout's own order is.
    if cfg!(windows) {
        let text = shown(&both);
        let output = text.strip_suffix("\n[exit 0]").unwrap_or_else(|| panic!("{text}"));
        assert!(output.replace('c', "") == "a\nb" && output.matches('c').count() == 1, "{text}");
    } else {
        assert_eq!(shown(&both), "a\nbc\n[exit 0]");
    }
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
    let Watched { background, started, mut finished, .. } = watched(dir.path());
    let _reaper = Reaper(started.clone());
    let bash = bash_with(dir.path(), &background);
    // The command can't end before the test makes `go`, so returning at all means bash didn't wait for it.
    let command = &format!("echo first; {}; echo second; exit 4", until_file("go"));
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
        done.summary().starts_with("task-1 finished (exit 4, ") && done.summary().contains("s): echo first; n=0; until [ -e go ]"),
        "{}",
        done.summary()
    );
    assert_eq!(started.lock()[0].log, done.log, "the program running the agent heard it start");

    // The next task gets the next number.
    let result = bash.execute(&args, &CancellationToken::new(), &|_| {}).await.expect("started");
    assert_eq!(result.details["task"], "task-2");
}

/// A chatty task's log keeps its start and its most recent output, at most
/// 50 MB, with a line saying how much went; the event's end of its
/// output is still its end.
#[tokio::test]
async fn a_chatty_tasks_log_keeps_its_start_and_its_end() {
    use factory_agent::tools::task_logs::CAP;
    let dir = tempfile::tempdir().expect("tempdir");
    let Watched { background, started, mut finished, _reaper } = watched(dir.path());
    let bash = bash_with(dir.path(), &background);
    // 60 MB of lines (yes stops at a broken pipe: not a failure here),
    // then it waits to be measured, then a last word.
    let command = format!(
        "echo first; yes 'a chatty line of output, again and again' | head -c 62914500 || true; {}; echo last",
        until_file("go")
    );
    let args = json!({"command": command, "run_in_background": true});
    bash.execute(&args, &CancellationToken::new(), &|_| {}).await.expect("started");
    let log = started.lock()[0].log.clone();
    // Past the cap, then cut: measured by its size, never read meanwhile.
    let size = || std::fs::metadata(&log).map_or(0, |metadata| metadata.len());
    let until = std::time::Instant::now() + HANG;
    let mut grew = false;
    while std::time::Instant::now() < until {
        let now = size();
        grew |= now > CAP;
        if grew && now <= CAP {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    std::fs::write(dir.path().join("go"), "").expect("go");
    let done = tokio::time::timeout(HANG, finished.recv()).await.expect("finished in time").expect("finished");
    let text = std::fs::read_to_string(&done.log).expect("log");
    assert!((text.len() as u64) < CAP / 2 + 2 * 1024 * 1024, "{} bytes", text.len());
    assert!(text.starts_with("first\na chatty line"), "its start");
    assert!(text.ends_with("again and again\nlast\n") && done.tail.ends_with("again and again\nlast\n"), "its end: {}", done.tail);
    let line = text.lines().find(|line| line.starts_with("[Claktory cut ")).expect("where it was cut");
    assert!(line.contains(" bytes of this task's output here, to keep its log under 50 MB"), "{line}");
    let went: u64 = line["[Claktory cut ".len()..].split(' ').next().unwrap().parse().unwrap();
    assert_eq!(went + text.len() as u64 - line.len() as u64 - 1, "first\n".len() as u64 + 62914500 + "last\n".len() as u64, "every byte counted");
}

/// One that writes it all and ends before it's measured is cut as it ends.
#[tokio::test]
async fn a_task_that_dumps_past_the_cap_and_ends_is_cut_as_it_ends() {
    use factory_agent::tools::task_logs::CAP;
    let dir = tempfile::tempdir().expect("tempdir");
    let Watched { background, mut finished, _reaper, .. } = watched(dir.path());
    let bash = bash_with(dir.path(), &background);
    let command = "yes 'a chatty line of output, again and again' | head -c 62914500 || true; echo last";
    bash.execute(&json!({"command": command, "run_in_background": true}), &CancellationToken::new(), &|_| {}).await.expect("started");
    let done = tokio::time::timeout(HANG, finished.recv()).await.expect("finished in time").expect("finished");
    let size = std::fs::metadata(&done.log).expect("log").len();
    assert!(size < CAP / 2 + 2 * 1024 * 1024, "{size} bytes");
    assert!(done.tail.ends_with("again and again\nlast\n"), "{}", done.tail);
}

#[tokio::test]
async fn a_background_task_outlives_whoever_started_it_and_can_be_picked_up_again() {
    let dir = tempfile::tempdir().expect("tempdir");
    let started: Arc<Mutex<Vec<factory_agent::Task>>> = Arc::default();
    let background = factory_agent::Background {
        dir: dir.path().join("tasks"),
        environment: Vec::new(),
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
    let done = tokio::time::timeout(HANG, task.wait_detached(&factory_agent::Redaction::default())).await.expect("ended in time");
    assert_eq!(done.status, "exit 7");
    assert_eq!(done.tail, "late\n");
}

// A process group as the pid; on Windows a task is its job.
#[cfg(unix)]
#[tokio::test]
async fn a_task_from_before_the_computer_restarted_has_ended_whatever_has_its_pid_now() {
    use factory_agent::tools::bash::{boot_time_ms, started_before_boot};
    let boot = boot_time_ms().expect("this computer's boot time");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("time").as_millis() as u64;
    assert!(boot < now && boot > now - 10 * 365 * 86_400_000, "{boot}");
    assert!(started_before_boot(boot - 1) && !started_before_boot(boot + 1) && !started_before_boot(0));

    let dir = tempfile::tempdir().expect("tempdir");
    let log = dir.path().join("task-1.log");
    std::fs::write(&log, "building\n").expect("log");
    // Its pid is now this test's own process group, which is alive.
    // SAFETY: getpgrp has no preconditions.
    let pid = unsafe { libc::getpgrp() } as u32;
    let task = factory_agent::Task { id: "task-1".into(), command: "cargo build".into(), pid, log, started: boot - 60_000, folder: Default::default() };
    let done = tokio::time::timeout(HANG, task.wait_detached(&factory_agent::Redaction::default())).await.expect("at once");
    assert_eq!((done.status.as_str(), done.seconds), ("the computer restarted", None));
    assert!(done.text().contains("The computer restarted while it ran, which ended it."), "{}", done.text());
    assert_eq!(done.summary(), "task-1 finished (the computer restarted): cargo build");
}

/// A shell loop that waits for `file` to appear, checking every 0.1 s, but
/// for a minute at most: a test that fails, or ends, first never leaves it
/// running.
fn until_file(file: &str) -> String {
    format!("n=0; until [ -e {file} ] || [ $n -ge 600 ]; do sleep 0.1; n=$((n+1)); done")
}

/// Kills the process groups of the tasks it holds when dropped, when the test
/// ends, passed or failed, so none outlives it.
struct Reaper(Arc<Mutex<Vec<factory_agent::Task>>>);

impl Drop for Reaper {
    fn drop(&mut self) {
        for task in self.0.lock().iter() {
            if let Ok(group) = i32::try_from(task.pid)
                && group > 1
            {
                // SAFETY: killpg only sends a signal.
                #[cfg(unix)]
                unsafe { libc::killpg(group, libc::SIGKILL) };
                #[cfg(windows)]
                factory_agent::windows::job::stop(group as u32);
            }
        }
    }
}

/// A Background whose started tasks and finished events the test can see.
/// Its tasks are killed when it's dropped.
struct Watched {
    background: factory_agent::Background,
    started: Arc<Mutex<Vec<factory_agent::Task>>>,
    finished: tokio::sync::mpsc::UnboundedReceiver<factory_agent::Finished>,
    _reaper: Reaper,
}

fn watched(dir: &Path) -> Watched {
    let (sender, finished) = tokio::sync::mpsc::unbounded_channel();
    let started: Arc<Mutex<Vec<factory_agent::Task>>> = Arc::default();
    let background = factory_agent::Background {
        dir: dir.join("tasks"),
        environment: Vec::new(),
        started: {
            let started = started.clone();
            Arc::new(move |task: &factory_agent::Task| started.lock().push(task.clone()))
        },
        finished: Arc::new(move |done| {
            let _ = sender.send(done);
        }),
    };
    Watched { background, _reaper: Reaper(started.clone()), started, finished }
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
    let command = &format!("echo first; {}; echo second; exit 4", until_file("go"));
    // Git Bash takes longer than that to start on Windows.
    let timeout = if cfg!(windows) { 3.0 } else { 0.3 };
    let result =
        tokio::time::timeout(HANG, bash.execute(&json!({"command": command, "timeout": timeout}), &CancellationToken::new(), &no_update))
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
    let aborted = bash.execute(&json!({"command": until_file("never")}), &token, &no_update).await.expect("ran");
    assert_eq!(aborted.details["stopped"], "aborted");
    assert!(watched.started.lock().is_empty());
    assert!(!dir.path().join("late").exists());
}

#[tokio::test]
async fn a_background_command_is_stopped_at_its_time_limit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut watched = watched(dir.path());
    let bash = bash_with(dir.path(), &watched.background);
    let args = json!({"command": until_file("never"), "run_in_background": true, "timeout": 0.2});
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

#[tokio::test]
async fn a_long_leading_sleep_is_refused_in_the_foreground() {
    use factory_agent::tools::checks::long_sleep;
    for command in ["sleep 25", "sleep 30; cargo test", "  sleep 60 && make", "sleep 25.5\necho done", "sleep 300 | cat", "sleep 30 & wait"]
    {
        assert!(matches!(long_sleep(command), Some(AgentFailure::BashLongSleep { .. })), "{command}");
    }
    for command in
        ["sleep 5", "sleep 24.9", "echo hi; sleep 30", "sleep $N", "sleep 1m", "until test -f x; do sleep 30; done", "'sleep 30'"]
    {
        assert_eq!(long_sleep(command), None, "{command}");
    }
    let dir = tempfile::tempdir().unwrap();
    let error = run(dir.path(), json!({"command": "sleep 40 && echo ready"})).await.unwrap_err();
    assert_eq!(error.failure().map(AgentFailure::kind), Some("bash-long-sleep"));
    assert!(error.to_string().starts_with("Blocked: sleep 40. Claktory wakes you with an event"), "{error}");
    // In the background it's the agent's own wait, and it isn't blocked.
    let error = run(dir.path(), json!({"command": "sleep 40", "run_in_background": true})).await.unwrap_err();
    assert!(error.failure().is_none(), "{error}");
}

#[tokio::test]
async fn copying_a_repository_the_agent_works_in_or_reads_is_refused() {
    use factory_agent::tools::checks::{copies_repository, repository_root};
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(repo.join("src/deep")).unwrap();
    let repo = repo.canonicalize().unwrap();
    let parent = repo.parent().unwrap().to_path_buf();
    let sub = repo.join("src");
    assert_eq!(repository_root(&sub.join("deep")), Some(repo.clone()));
    assert_eq!(repository_root(&parent), None);
    let roots = [repo.clone()];
    let r = repo.display().to_string();
    let p = parent.display().to_string();
    let refused = [
        (repo.as_path(), "cp -R . /tmp/copy".to_owned()),
        (sub.as_path(), "cp -a .. /tmp/copy 2>&1".to_owned()),
        (sub.as_path(), format!("FOO=1 cp -rv {r}/ /tmp/copy > log")),
        (parent.as_path(), "ditto repo /tmp/copy".to_owned()),
        (parent.as_path(), format!("cp -R {p} /tmp/copy")),
        (repo.as_path(), "rsync -a --exclude target ./ /tmp/copy".to_owned()),
        (repo.as_path(), "git clone . /tmp/copy".to_owned()),
        (parent.as_path(), format!("git clone --depth 1 file://{r} /tmp/copy")),
        (parent.as_path(), "git clone repo".to_owned()),
        (sub.as_path(), "git worktree add /tmp/wt".to_owned()),
        (parent.as_path(), format!("git -C {r} worktree add ../wt")),
        (parent.as_path(), "cd repo && cp -R . /tmp/copy".to_owned()),
        (repo.as_path(), "echo start; cp -R * /tmp/copy".to_owned()),
    ];
    for (cwd, command) in &refused {
        let failure = copies_repository(command, cwd, &roots);
        assert_eq!(failure, Some(AgentFailure::BashCopiesRepository { path: r.clone() }), "{command} in {}", cwd.display());
    }
    let allowed = [
        (repo.as_path(), "cp -R src /tmp/copy"),
        (repo.as_path(), "cp README.md /tmp/copy"),
        (repo.as_path(), "cp -R /tmp/a ."),
        (repo.as_path(), "cp -R /tmp/a . > log"),
        (repo.as_path(), "rsync -a src/ /tmp/copy"),
        (repo.as_path(), "git archive HEAD | tar -x -C /tmp/copy"),
        (repo.as_path(), "git clone https://github.com/example/other"),
        (repo.as_path(), "git worktree list"),
        (repo.as_path(), "echo 'cp -R . /tmp/copy'"),
        (repo.as_path(), "cd /tmp && cp -R . copy"),
        (parent.as_path(), "cp -R elsewhere /tmp/copy"),
    ];
    for (cwd, command) in allowed {
        assert_eq!(copies_repository(command, cwd, &roots), None, "{command} in {}", cwd.display());
    }
    assert_eq!(copies_repository("cp -R . /tmp/copy", &repo, &[]), None, "no repository, nothing to refuse");

    // Through the tool: a repository it only reads counts too.
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let mut tools = coding_tools(&work, &[], None);
    factory_agent::tools::read_only(&mut tools, std::slice::from_ref(&repo));
    let tool = tools.into_iter().find(|tool| tool.spec.name == "bash").unwrap();
    let command = format!("cp -R {r} copy");
    let error = tool.execute(&json!({"command": command}), &CancellationToken::new(), &no_update).await.unwrap_err();
    assert_eq!(error.failure().map(AgentFailure::kind), Some("bash-copies-repository"));
    assert!(error.to_string().contains("Claktory's Own checkout extension") && error.to_string().contains("git archive HEAD"), "{error}");
    assert!(!work.join("copy").exists());
}

#[tokio::test]
async fn detaching_moves_a_running_command_to_the_background_so_a_message_can_reach_the_agent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut watched = watched(dir.path());
    let bash = bash_with(dir.path(), &watched.background);
    let detach = CancellationToken::new();
    let later = detach.clone();
    tokio::spawn(async move {
        // Git Bash takes longer than that to start on Windows.
        tokio::time::sleep(Duration::from_millis(if cfg!(windows) { 3000 } else { 300 })).await;
        later.cancel();
    });
    let command = &format!("echo first; {}; echo second", until_file("go"));
    let result =
        tokio::time::timeout(HANG, bash.execute_detachable(&json!({"command": command}), &CancellationToken::new(), &detach, &no_update))
            .await
            .expect("it didn't wait for the command")
            .expect("moved");
    let moved = text(&result);
    assert_eq!(result.details["movedToBackground"], "task-1");
    assert!(moved.starts_with("A new message came for you after "), "{moved}");
    assert!(moved.contains("so this command moved to the background as task-1") && !moved.contains("find out why"), "{moved}");
    assert!(moved.contains("Output so far ends with:\nfirst\n") && moved.ends_with("s]"), "{moved}");
    std::fs::write(dir.path().join("go"), "").expect("go");
    let done = tokio::time::timeout(HANG, watched.finished.recv()).await.expect("finished in time").expect("finished");
    assert_eq!((done.id.as_str(), done.status.as_str(), done.tail.as_str()), ("task-1", "exit 0", "first\nsecond\n"));

    // A wait is only stopped; a command that ends first isn't touched.
    let fired = CancellationToken::new();
    fired.cancel();
    let slept = bash.execute_detachable(&json!({"command": "sleep 20"}), &CancellationToken::new(), &fired, &no_update).await.expect("ran");
    assert!(shown(&slept).ends_with("[stopped so a new message could reach you]"), "{}", shown(&slept));
    let quick =
        bash.execute_detachable(&json!({"command": "echo hi"}), &CancellationToken::new(), &CancellationToken::new(), &no_update).await;
    assert_eq!(shown(&quick.expect("ran")), "hi\n\n[exit 0]");
    assert_eq!(watched.started.lock().len(), 1);
}
