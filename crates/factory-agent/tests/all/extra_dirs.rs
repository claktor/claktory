use std::path::{Path, PathBuf};

use factory_agent::errors::Error;
use factory_agent::tools::{Tool, coding_tools};
use factory_agent::types::{ToolResult, ToolResultPart};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

#[derive(Serialize)]
struct Write<'a> {
    path: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct Read<'a> {
    path: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Replacement<'a> {
    old_text: &'a str,
    new_text: &'a str,
}

#[derive(Serialize)]
struct Edit<'a> {
    path: &'a str,
    edits: Vec<Replacement<'a>>,
}

async fn execute(tools: &[Tool], name: &str, args: impl Serialize) -> Result<ToolResult, Error> {
    tools
        .iter()
        .find(|tool| tool.spec.name == name)
        .expect("tool")
        .execute(&serde_json::to_value(args)?, &CancellationToken::new(), &|_| {})
        .await
}

fn tools(workspace: &Path, extra: &[&Path]) -> Vec<Tool> {
    let extra: Vec<PathBuf> = extra.iter().map(|dir| dir.to_path_buf()).collect();
    coding_tools(workspace, &extra, None)
}

#[tokio::test]
async fn every_extra_dir_can_be_written_edited_and_read_without_entering_the_project() {
    let workspace = tempfile::tempdir().unwrap();
    let settings = tempfile::tempdir().unwrap();
    let notes = tempfile::tempdir().unwrap();
    let tools = tools(workspace.path(), &[settings.path(), notes.path()]);
    for dir in [settings.path(), notes.path()] {
        let path = dir.join("agents/self.json").display().to_string();
        execute(&tools, "write_file", Write { path: &path, content: "{\"model\": \"old\"}" }).await.unwrap();
        execute(&tools, "edit_file", Edit { path: &path, edits: vec![Replacement { old_text: "old", new_text: "new" }] }).await.unwrap();
        let read = execute(&tools, "read_file", Read { path: &path }).await.unwrap();
        assert!(read.content.iter().any(|part| matches!(part, ToolResultPart::Text { text } if text.contains("new"))));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"model\": \"new\"}");
    }
    assert_eq!(std::fs::read_dir(workspace.path()).unwrap().count(), 0);
    execute(&tools, "write_file", Write { path: "source.py", content: "pass" }).await.unwrap();
    assert_eq!(std::fs::read_to_string(workspace.path().join("source.py")).unwrap(), "pass");
}

#[cfg(unix)]
#[tokio::test]
async fn extra_dirs_are_opt_in_and_cannot_be_escaped_through_paths_or_symlinks() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    let state = root.path().join("state");
    let outside = root.path().join("state-other");
    for path in [&workspace, &state, &outside] {
        std::fs::create_dir(path).unwrap();
    }
    let path = state.join("recipe.json").display().to_string();
    assert!(execute(&tools(&workspace, &[]), "write_file", Write { path: &path, content: "{}" }).await.is_err());
    std::os::unix::fs::symlink(&outside, state.join("escape")).unwrap();
    std::os::unix::fs::symlink(&state, workspace.join("alias")).unwrap();
    let tools = tools(&workspace, &[&state]);
    for path in [state.join("../state-other/new"), state.join("escape/new"), workspace.join("alias/new"), outside.join("new")] {
        assert!(execute(&tools, "write_file", Write { path: &path.display().to_string(), content: "bad" }).await.is_err());
    }
    assert!(execute(&tools, "write_file", Write { path: "../state/new", content: "bad" }).await.is_err());
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}

#[tokio::test]
async fn extra_dirs_do_not_move_shell_commands() {
    #[derive(Serialize)]
    struct Shell {
        command: &'static str,
    }
    let workspace = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let tools = tools(workspace.path(), &[state.path()]);
    execute(&tools, "bash", Shell { command: "printf local > shell-output" }).await.unwrap();
    assert_eq!(std::fs::read_to_string(workspace.path().join("shell-output")).unwrap(), "local");
    assert!(!state.path().join("shell-output").exists());
}

#[tokio::test]
async fn read_only_dirs_can_be_read_but_not_changed_even_inside_the_working_folder() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    let mine = project.join("mine");
    let theirs = project.join("theirs");
    let elsewhere = root.path().join("elsewhere");
    for path in [&mine, &theirs, &elsewhere] {
        std::fs::create_dir_all(path).unwrap();
        std::fs::write(path.join("notes.md"), "old").unwrap();
    }
    std::fs::write(project.join("top.md"), "old").unwrap();
    // An agent in the project's own folder, with two of the project's folders read only to it.
    let mut in_project = tools(&project, &[]);
    factory_agent::tools::read_only(&mut in_project, &[mine.clone(), elsewhere.clone()]);
    execute(&in_project, "write_file", Write { path: "top.md", content: "new" }).await.unwrap();
    execute(&in_project, "write_file", Write { path: "theirs/notes.md", content: "new" }).await.unwrap();
    for path in ["mine/notes.md".to_owned(), mine.join("notes.md").display().to_string(), elsewhere.join("notes.md").display().to_string()]
    {
        let error = execute(&in_project, "write_file", Write { path: &path, content: "bad" }).await.unwrap_err();
        assert_eq!(error.failure(), Some(&factory_agent::errors::AgentFailure::ReadOnly { path: path.clone() }));
        let edit = Edit { path: &path, edits: vec![Replacement { old_text: "old", new_text: "bad" }] };
        assert!(execute(&in_project, "edit_file", edit).await.is_err());
        let read = execute(&in_project, "read_file", Read { path: &path }).await.unwrap();
        assert!(read.content.iter().any(|part| matches!(part, ToolResultPart::Text { text } if text.contains("old"))));
    }
    // An agent in one of the folders, with the project's folder read only: the more specific folder wins.
    let mut in_mine = tools(&mine, &[]);
    factory_agent::tools::read_only(&mut in_mine, std::slice::from_ref(&project));
    execute(&in_mine, "write_file", Write { path: &mine.join("notes.md").display().to_string(), content: "mine" }).await.unwrap();
    let top = project.join("top.md").display().to_string();
    assert!(execute(&in_mine, "write_file", Write { path: &top, content: "bad" }).await.is_err());
    assert!(execute(&in_mine, "read_file", Read { path: &top }).await.is_ok());
    assert_eq!(std::fs::read_to_string(project.join("top.md")).unwrap(), "new");
}

#[tokio::test]
async fn a_denied_folder_is_refused_in_its_own_words_through_a_link_too() {
    use factory_agent::errors::AgentFailure;
    use factory_agent::tools::Denied;
    let root = tempfile::tempdir().unwrap();
    let work = root.path().join("work");
    let kept = root.path().join("kept");
    let private = root.path().join("private");
    for path in [&work, &kept, &private] {
        std::fs::create_dir_all(path).unwrap();
        std::fs::write(path.join("notes.md"), "secret").unwrap();
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&kept, work.join("link")).unwrap();
    let mut tools = tools(&work, &[]);
    let why = "holds something no agent's tools can read or change.";
    factory_agent::tools::denied(&mut tools, &[private.clone().into(), Denied { dir: kept.clone(), why: Some(why.into()) }]);
    let path = kept.join("notes.md").display().to_string();
    let error = execute(&tools, "read_file", Read { path: &path }).await.unwrap_err();
    assert_eq!(error.failure(), Some(&AgentFailure::Kept { path: path.clone(), why: why.into() }));
    assert_eq!(error.failure().unwrap().message(), format!("{path} {why}"));
    #[cfg(unix)]
    {
        let error = execute(&tools, "read_file", Read { path: "link/notes.md" }).await.unwrap_err();
        assert_eq!(error.failure(), Some(&AgentFailure::Kept { path: "link/notes.md".into(), why: why.into() }));
    }
    let path = private.join("notes.md").display().to_string();
    let error = execute(&tools, "write_file", Write { path: &path, content: "bad" }).await.unwrap_err();
    assert_eq!(error.failure(), Some(&AgentFailure::Private { path }));
    assert!(execute(&tools, "read_file", Read { path: "notes.md" }).await.is_ok());
}
