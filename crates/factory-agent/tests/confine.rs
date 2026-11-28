//! Agents' commands run under what the embedder says
//! (`tools::confine`): the shell's in the foreground and the background
//! alike. A file of its own, since the rule is the whole process's.

#![cfg(target_os = "macos")]

use std::sync::Arc;
use std::time::Duration;

use factory_agent::tools::coding_tools;
use factory_agent::types::{ToolResult, ToolResultPart};
use serde_json::json;
use tokio_util::sync::CancellationToken;

fn text(result: &ToolResult) -> String {
    result.content.iter().map(|part| if let ToolResultPart::Text { text } = part { text.clone() } else { String::new() }).collect()
}

#[tokio::test]
async fn agents_commands_run_under_the_embedders_sandbox() {
    let dir = tempfile::tempdir().expect("a folder");
    let dir = std::fs::canonicalize(dir.path()).expect("its real path");
    let kept = dir.join("kept");
    std::fs::create_dir(&kept).unwrap();
    std::fs::write(kept.join("volume.img"), "cookies").unwrap();
    let profile = format!("(version 1)(allow default)(deny file-read* file-write* (subpath \"{}\"))", kept.display());
    let prefix = vec!["/usr/bin/sandbox-exec".into(), "-p".into(), profile.into()];
    factory_agent::tools::confine::set(Some(Arc::new(move || Some(prefix.clone()))));

    let bash = coding_tools(&dir, &[], None).into_iter().find(|tool| tool.spec.name == "bash").expect("bash");
    let run = |command: &str| {
        let args = json!({"command": command});
        let bash = bash.clone();
        async move { bash.execute(&args, &CancellationToken::new(), &|_| {}).await }
    };
    // In the foreground: the folder is out of reach, the rest works.
    let said = text(&run("cat kept/volume.img; echo exit=$?").await.expect("ran"));
    assert!(!said.contains("cookies") && said.contains("Operation not permitted"), "{said}");
    let said = text(&run("echo fine").await.expect("ran"));
    assert!(said.contains("fine"), "{said}");

    // A background task's shell too.
    let log = dir.join("task.log");
    let (mut child, _) = factory_agent::tools::bash::spawn_detached(&dir, "cat kept/volume.img", &log, &[], "agent").expect("started");
    tokio::time::timeout(Duration::from_secs(60), child.wait()).await.expect("ended").expect("waited");
    let logged = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(!logged.contains("cookies") && logged.contains("Operation not permitted"), "{logged}");

    // Nothing set: nothing in the way.
    factory_agent::tools::confine::set(None);
    let said = text(&run("cat kept/volume.img").await.expect("ran"));
    assert!(said.contains("cookies"), "{said}");
}
