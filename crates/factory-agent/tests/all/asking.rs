//! Agents' commands never keep a program that asks who you are (Bruno
//! 36035, Fugo 36039 and 36040): sudo and the like are stopped as soon as
//! they're seen, in the foreground, through a log or in the background,
//! and the agent is told. Never the real programs: copies of /bin/sleep
//! (and /bin/bash, for osascript's arguments) under their names.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use factory_agent::tools::asking::{Process, asking, told};
use factory_agent::tools::{Tool, coding_tools};
use factory_agent::types::{ToolResult, ToolResultPart};
use parking_lot::Mutex;
use serde_json::json;
use tokio_util::sync::CancellationToken;

const HANG: Duration = Duration::from_secs(60);

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

/// A copy of `program` named `name` in `dir`. macOS kills a copy of a
/// system binary that keeps Apple's signature, so it's signed again, ad hoc.
fn copy_named(dir: &Path, program: &str, name: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::copy(program, &path).expect("copy");
    if cfg!(target_os = "macos") {
        let signed = std::process::Command::new("codesign").args(["-f", "-s", "-"]).arg(&path).output().expect("codesign");
        assert!(signed.status.success(), "{signed:?}");
    }
    path
}

fn tool(dir: &Path, background: Option<factory_agent::Background>) -> Tool {
    coding_tools(dir, &[], background).into_iter().find(|tool| tool.spec.name == "bash").expect("bash")
}

async fn run(bash: &Tool, command: &str) -> (String, Duration) {
    let began = Instant::now();
    let result = tokio::time::timeout(HANG, bash.execute(&json!({"command": command}), &CancellationToken::new(), &|_| {}))
        .await
        .expect("it ended")
        .expect("ran");
    (text(&result), began.elapsed())
}

#[test]
fn what_asks_who_you_are_is_known_by_its_name_or_its_file() {
    let process = |name: &str, path: &str| Process { pid: 0, name: name.to_owned(), path: path.to_owned() };
    assert_eq!(asking(&process("sudo", "")), Some("sudo"));
    // Run as root, its name can't be read; its file can.
    assert_eq!(asking(&process("", "/usr/bin/sudo")), Some("sudo"));
    assert_eq!(asking(&process("", "/usr/libexec/authopen")), Some("authopen"));
    assert_eq!(asking(&process("pkexec", "/usr/bin/pkexec")), Some("pkexec"));
    assert_eq!(asking(&process("su", "/usr/bin/su")), Some("su"));
    assert_eq!(asking(&process("sudoku", "/usr/local/bin/sudoku")), None);
    assert_eq!(asking(&process("bash", "/bin/bash")), None);
    assert_eq!(told("sudo"), "sudo isn't for agents: it was stopped. Ask the person to run it themselves.");
}

#[tokio::test(flavor = "multi_thread")]
async fn sudo_and_the_like_are_stopped_in_agents_commands_and_the_agent_is_told() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).expect("bin");
    for name in ["sudo", "doas", "su", "pkexec", "authopen"] {
        copy_named(&bin, "/bin/sleep", name);
    }
    copy_named(&bin, "/bin/bash", "osascript");

    // In the foreground, through pipes.
    let bash = tool(dir.path(), None);
    for name in ["sudo", "doas", "su", "pkexec", "authopen"] {
        let (said, took) = run(&bash, &format!("'{}' 30", bin.join(name).display())).await;
        assert!(said.contains(&told(name)), "{name}: {said}");
        assert!(took < Duration::from_secs(25), "{name} was stopped, not waited for: {took:?}");
    }
    // osascript only with administrator privileges.
    let osascript = bin.join("osascript");
    let (said, _) = run(&bash, &format!("'{}' -c 'sleep 1; echo plain; true' x 'display dialog'", osascript.display())).await;
    assert!(said.contains("plain") && !said.contains("isn't for agents"), "{said}");
    let (said, _) = run(&bash, &format!("'{}' -c 'sleep 2; true' x 'do shell script \"x\" with Administrator Privileges'", osascript.display())).await;
    assert!(said.contains(&told("osascript with administrator privileges")), "{said}");

    // Through a log, in the foreground and in the background.
    let (sender, mut finished) = tokio::sync::mpsc::unbounded_channel();
    let started: Arc<Mutex<Vec<factory_agent::Task>>> = Arc::default();
    let background = factory_agent::Background {
        dir: dir.path().join("tasks"),
        environment: vec![("FACTORY_AGENT_ID".to_owned(), "lea".to_owned())],
        started: {
            let started = started.clone();
            Arc::new(move |task: &factory_agent::Task| started.lock().push(task.clone()))
        },
        finished: Arc::new(move |done| {
            let _ = sender.send(done);
        }),
    };
    let bash = tool(dir.path(), Some(background));
    let (said, took) = run(&bash, &format!("echo before; '{}' 30; echo after", bin.join("sudo").display())).await;
    assert!(said.starts_with("before\n") && said.contains(&told("sudo")), "{said}");
    assert!(took < Duration::from_secs(25), "{took:?}");
    let args = json!({"command": format!("'{}' 30", bin.join("doas").display()), "run_in_background": true});
    bash.execute(&args, &CancellationToken::new(), &|_| {}).await.expect("started");
    let done = tokio::time::timeout(HANG, finished.recv()).await.expect("finished in time").expect("finished");
    assert!(done.tail.contains(&told("doas")), "{}", done.tail);

    // Anything else runs as it would.
    let (said, _) = run(&bash, "sleep 0.3; echo fine").await;
    assert!(said.starts_with("fine\n") && !said.contains("isn't for agents"), "{said}");
}
