use std::{
    ops::Deref,
    path::{Path, PathBuf},
};

use factory_agent::errors::{AgentFailure, Error};
use factory_agent::tools::{Tool, bounded, coding_tools};
use factory_agent::types::{ToolResult, ToolResultPart};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn tool<'a>(tools: &'a [Tool], name: &str) -> &'a Tool {
    tools.iter().find(|tool| tool.spec.name == name).expect("tool")
}

fn no_update(_: ToolResult) {}

struct TestTools {
    tools: Vec<Tool>,
    dir: tempfile::TempDir,
}

#[allow(clippy::unwrap_used)]
impl TestTools {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let tools = coding_tools(dir.path(), &[], None);
        Self { tools, dir }
    }

    fn editing() -> Self {
        let tools = Self::new();
        std::fs::create_dir_all(tools.path("src")).unwrap();
        tools
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root().join(relative)
    }

    fn lib(&self) -> PathBuf {
        self.path("src/lib.rs")
    }

    fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.path(relative)).unwrap()
    }

    fn read_lib(&self) -> String {
        self.read("src/lib.rs")
    }

    fn assert_lib<T: AsRef<str>>(&self, expected: impl FnOnce() -> T) {
        let actual = self.read_lib();
        assert_eq!(actual, expected().as_ref());
    }
}

impl Deref for TestTools {
    type Target = [Tool];

    fn deref(&self) -> &Self::Target {
        &self.tools
    }
}

async fn run(tools: &[Tool], name: &str, args: Value) -> Result<ToolResult, Error> {
    tool(tools, name).execute(&args, &CancellationToken::new(), &no_update).await
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

#[test]
fn tool_specs_serialize_like_the_typescript_agents() {
    let tools = coding_tools(Path::new("/tmp/w"), &[], None);
    let names: Vec<&str> = tools.iter().map(|tool| tool.spec.name.as_ref()).collect();
    assert_eq!(names, ["read_file", "read_image", "write_file", "edit_file", "bash"]);
    let read = serde_json::to_value(&tool(&tools, "read_file").spec).unwrap();
    assert_eq!(
        read,
        json!({"name":"read_file","label":"Read file","description":"Read numbered UTF-8 lines. offset is one-based; limit defaults to 200. Use smaller ranges when output is truncated.","parameters":{"type":"object","required":["path"],"properties":{"path":{"type":"string"},"offset":{"anyOf":[{"type":"integer","minimum":1},{"type":"null"}]},"limit":{"anyOf":[{"type":"integer","minimum":1,"maximum":2000},{"type":"null"}]}},"additionalProperties":false},"constrainedSampling":{"type":"json_schema","strict":"prefer"}})
    );
    let labels: Vec<(&str, &str)> = tools.iter().map(|tool| (tool.spec.name.as_ref(), tool.spec.label.as_ref())).collect();
    assert_eq!(
        labels,
        [
            ("read_file", "Read file"),
            ("read_image", "Read image"),
            ("write_file", "Write file"),
            ("edit_file", "Edit file"),
            ("bash", "Run command"),
        ]
    );
    assert_eq!(
        tool(&tools, "read_image").spec.description,
        "Look at an image file: PNG, JPEG, GIF or WebP. It's added to the conversation as an image you see, at this point. One too big for your model is scaled down for you; the result says so. Only an agent whose model reads images can use it."
    );
    assert_eq!(
        tool(&tools, "write_file").spec.description,
        "Create or replace a whole UTF-8 file and its parent directories. Use edit_file for partial changes to an existing file."
    );
    assert_eq!(
        tool(&tools, "edit_file").spec.description,
        "Apply exact replacements to an existing file and return a unified diff. Each oldText must match exactly once in the original file, or set replaceAll to replace every occurrence; a unique match that differs only in leading or trailing whitespace on each line is accepted. Edits must not overlap; errors name each failing edit."
    );
    assert_eq!(
        tool(&tools, "bash").spec.description,
        "Run bash. Every command starts in your working folder, so don't cd there first; a cd lasts only for that command. Unhandled failures stop the script (errexit and pipefail); use if or || for expected failures. Piping to head can cause SIGPIPE; use a draining filter such as sed -n instead. command is the shell script itself, not a bash -c/bash -lc invocation — put the script text directly in command. Run tests and inspect git diff. timeout is seconds (default 120, maximum 600). A command still running at its timeout moves to the background and keeps running, and you get an event when it ends (one that starts with sleep is stopped instead). Set run_in_background up front for servers, watchers and anything you know is long: it returns at once with a task id and a log file, and the command is stopped after 30 minutes unless timeout says otherwise (up to 7200). Never wait with sleep: background tasks and messages wake you by themselves. A task's result comes only as its own event, in a turn of its own; never report it, or what it printed, before that event arrives. Output up to 30,000 characters comes back whole; past that you get its start, its end and the path of the whole output. Every result ends with the exit status and how long the command took; tool updates contain output as it arrives. Files written by commands inherit the environment's resource limits."
    );
    assert!(!tool(&tools, "bash").spec.description.contains("rg"), "no ripgrep unless it's there");
    assert_eq!(bounded("abc"), "abc");
    let long = "x".repeat(24_001);
    assert_eq!(bounded(&long), format!("{}\n[Output truncated. Request a smaller range.]", "x".repeat(24_000)));
    let emoji = "😀".repeat(12_001);
    assert!(bounded(&emoji).starts_with(&"😀".repeat(12_000)));
    assert!(bounded(&emoji).ends_with("[Output truncated. Request a smaller range.]"));
}

#[tokio::test]
async fn ranged_reads_and_new_files_preserve_untouched_crlf_and_unicode_source() {
    let tools = TestTools::new();
    let created =
        run(&tools, "write_file", json!({"path": "src/code.ts", "content": "// 😀\r\nconst a = 1;\r\nconst b = 2;\r\n"})).await.unwrap();
    assert_eq!(text(&created), "Created src/code.ts (35 characters).");
    assert_eq!(created.details, json!({"path": "src/code.ts", "existed": false}));
    let read = run(&tools, "read_file", json!({"path": "src/code.ts", "offset": 2, "limit": 1})).await.unwrap();
    assert_eq!(text(&read), "2: const a = 1;\n[Lines 2-2 of 4]");
    assert_eq!(read.details, json!({"path": "src/code.ts", "offset": 2, "end": 2, "totalLines": 4}));
    let whole = run(&tools, "read_file", json!({"path": "src/code.ts"})).await.unwrap();
    assert_eq!(text(&whole), "1: // 😀\n2: const a = 1;\n3: const b = 2;\n4: \n[Lines 1-4 of 4]");
    let beyond = run(&tools, "read_file", json!({"path": "src/code.ts", "offset": 10, "limit": null})).await.unwrap();
    assert_eq!(text(&beyond), "\n[Lines 10-4 of 4]");
    let replaced = run(&tools, "write_file", json!({"path": "src/code.ts", "content": "clobber"})).await.unwrap();
    assert_eq!(text(&replaced), "Replaced src/code.ts (7 characters).");
    assert_eq!(tools.read("src/code.ts"), "clobber");
    run(&tools, "write_file", json!({"path": "..valid", "content": "legal filename"})).await.unwrap();
    assert_eq!(tools.read("..valid"), "legal filename");
    std::fs::write(tools.path("src/long.txt"), "y".repeat(30_000)).unwrap();
    let long = run(&tools, "read_file", json!({"path": "src/long.txt"})).await.unwrap();
    assert!(text(&long).contains("[Output truncated. Request a smaller range.]"));
    assert!(text(&long).ends_with("\n[Lines 1-1 of 1]"));
}

// Unix modes and symlinks (Windows: Developer Mode or an administrator for links).
#[cfg(unix)]
#[tokio::test]
async fn writes_over_directories_and_escapes_are_refused_without_side_effects() {
    let tools = TestTools::new();
    std::fs::create_dir_all(tools.path("src/full")).unwrap();
    std::fs::write(tools.path("src/full/keep"), "kept").unwrap();
    let error = run(&tools, "write_file", json!({"path": "src/full", "content": "over a directory"})).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("EISDIR: illegal operation on a directory, rename '{0}/src/full.tmp' -> '{0}/src/full'", tools.root().display())
    );
    assert_eq!(tools.read("src/full/keep"), "kept");
    assert!(!tools.path("src/full.tmp").exists());
    std::fs::write(tools.path("locked.tmp"), "stale").unwrap();
    std::fs::set_permissions(tools.path("locked.tmp"), std::os::unix::fs::PermissionsExt::from_mode(0o444)).unwrap();
    let error = run(&tools, "write_file", json!({"path": "locked", "content": "x"})).await.unwrap_err();
    assert_eq!(error.to_string(), format!("EACCES: permission denied, open '{}/locked.tmp'", tools.root().display()));
    assert!(!tools.path("locked.tmp").exists());
    assert!(!tools.path("locked").exists());
    std::os::unix::fs::symlink(std::env::temp_dir(), tools.path("escape")).unwrap();
    for (name, args) in [("read_file", json!({"path": "escape/secret"})), ("write_file", json!({"path": "escape/secret", "content": "no"}))]
    {
        let error = run(&tools, name, args).await.unwrap_err();
        assert_eq!(error.failure(), Some(&AgentFailure::WorkspaceEscape { path: "escape/secret".into() }), "{name}");
    }
    assert!(!std::env::temp_dir().join("secret").exists());
    for (name, args) in [
        ("read_file", json!({"path": "../secret"})),
        ("write_file", json!({"path": "../secret", "content": "text"})),
        ("read_file", json!({"path": "/etc/passwd"})),
    ] {
        let error = run(&tools, name, args).await.unwrap_err();
        assert!(matches!(error.failure(), Some(AgentFailure::WorkspaceEscape { .. })), "{name}");
    }
}

// Symlinks, and Node's texts for Unix errors.
#[cfg(unix)]
#[tokio::test]
async fn native_filesystem_errors_keep_nodes_text() {
    let tools = TestTools::new();
    std::fs::create_dir_all(tools.path("src/full")).unwrap();
    std::fs::write(tools.path("src/code.ts"), "const a = 1;\n").unwrap();
    std::os::unix::fs::symlink(tools.path("src/code.ts"), tools.path("link.ts")).unwrap();
    std::os::unix::fs::symlink(tools.path("nowhere"), tools.path("dangling")).unwrap();
    let root = tools.root().display();
    let cases = [
        ("read_file", json!({"path": "missing.txt"}), format!("ENOENT: no such file or directory, open '{root}/missing.txt'")),
        ("read_file", json!({"path": "src/full"}), "EISDIR: illegal operation on a directory, read".to_owned()),
        ("read_file", json!({"path": "dangling"}), format!("ENOENT: no such file or directory, open '{root}/dangling'")),
        ("read_file", json!({"path": "src/code.ts/x"}), format!("ENOTDIR: not a directory, realpath '{root}/src/code.ts/x'")),
        (
            "write_file",
            json!({"path": "src/code.ts/x", "content": "x"}),
            format!("ENOTDIR: not a directory, realpath '{root}/src/code.ts/x'"),
        ),
    ];
    for (name, args, message) in cases {
        let error = run(&tools, name, args.clone()).await.unwrap_err();
        assert!(matches!(error, Error::Fs(_)), "{name} {args}");
        assert_eq!(error.to_string(), message);
    }
    let linked = run(&tools, "read_file", json!({"path": "link.ts"})).await.unwrap();
    assert_eq!(text(&linked), "1: const a = 1;\n2: \n[Lines 1-2 of 2]");
    let dangling = run(&tools, "write_file", json!({"path": "dangling", "content": "x"})).await.unwrap();
    assert_eq!(text(&dangling), "Created dangling (1 characters).");
    assert_eq!(tools.read("dangling"), "x");
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let error = tool(&tools, "write_file").execute(&json!({"path": "late", "content": "x"}), &cancelled, &no_update).await.unwrap_err();
    assert!(matches!(error, Error::Aborted));
    assert!(!tools.path("late").exists());
}

#[tokio::test]
async fn targeted_edits_return_a_diff_and_refuse_ambiguous_or_overlapping_text() {
    let tools = TestTools::new();
    let expected = "// 😀\r\nconst a = 3;\r\nconst b = 4;\r\n";
    run(&tools, "write_file", json!({"path": "src/code.ts", "content": "// 😀\r\nconst a = 1;\r\nconst b = 2;\r\n"})).await.unwrap();
    let edit = run(
        &tools,
        "edit_file",
        json!({"path": "src/code.ts", "edits": [{"oldText": "a = 1", "newText": "a = 3"}, {"oldText": "b = 2", "newText": "b = 4"}]}),
    )
    .await
    .unwrap();
    assert!(text(&edit).contains("+const a = 3"), "{}", text(&edit));
    assert!(text(&edit).starts_with("Index: src/code.ts\n"));
    assert_eq!(edit.details["path"], "src/code.ts");
    assert_eq!(edit.details["diff"], text(&edit));
    assert_eq!(tools.read("src/code.ts"), expected);
    for (edits, reason) in [
        (
            json!([{"oldText": "const ", "newText": "let "}]),
            "edits[0]: oldText matches 2 times, at lines 2, 3; add surrounding context or set replaceAll",
        ),
        (json!([{"oldText": "missing", "newText": "x"}]), "edits[0]: oldText not found"),
        (
            json!([{"oldText": "const a = 3", "newText": "a"}, {"oldText": "a = 3", "newText": "b"}]),
            "edits[0] and edits[1]: replacement ranges overlap",
        ),
    ] {
        let error = run(&tools, "edit_file", json!({"path": "src/code.ts", "edits": edits})).await.unwrap_err();
        assert_eq!(error.failure(), Some(&AgentFailure::InvalidEdit { path: "src/code.ts".into(), reason: reason.into() }));
        assert_eq!(error.to_string(), format!("Cannot edit src/code.ts: {reason}"));
        assert_eq!(tools.read("src/code.ts"), expected);
    }
    std::fs::write(tools.path("aaa.txt"), "aaa").unwrap();
    let error = run(&tools, "edit_file", json!({"path": "aaa.txt", "edits": [{"oldText": "aa", "newText": "b"}]})).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Cannot edit aaa.txt: edits[0]: oldText matches 2 times, at lines 1; add surrounding context or set replaceAll"
    );
    std::fs::write(tools.path("lf.txt"), "one\ntwo\n").unwrap();
    let lf = run(&tools, "edit_file", json!({"path": "lf.txt", "edits": [{"oldText": "two", "newText": "2\r\n3"}]})).await.unwrap();
    assert_eq!(tools.read("lf.txt"), "one\n2\n3\n");
    assert!(text(&lf).contains("-two\n+2\n+3"));
    let error = run(&tools, "edit_file", json!({"path": "missing.txt", "edits": [{"oldText": "a", "newText": "b"}]})).await.unwrap_err();
    assert_eq!(error.to_string(), format!("ENOENT: no such file or directory, open '{}'", tools.root().join("missing.txt").display()));
    let error = run(&tools, "edit_file", json!({"path": "../secret", "edits": [{"oldText": "a", "newText": "b"}]})).await.unwrap_err();
    assert_eq!(error.failure(), Some(&AgentFailure::WorkspaceEscape { path: "../secret".into() }));
}

const ROUTES: &str =
    "fn alpha() {\n    let total = compute(1);\n    log(total);\n}\n\nfn beta() {\n    let total = compute(2);\n    log(total);\n}\n";

async fn edit(tools: &[Tool], edits: Value) -> Result<ToolResult, Error> {
    run(tools, "edit_file", json!({"path": "src/lib.rs", "edits": edits})).await
}

#[tokio::test]
async fn every_failing_edit_is_named_and_the_file_is_left_untouched() {
    let tools = TestTools::editing();
    std::fs::write(tools.lib(), ROUTES).unwrap();
    let error = edit(
        &tools,
        json!([
            {"oldText": "log(total);", "newText": "emit(total);"},
            {"oldText": "fn alpha() {", "newText": "fn first() {"},
            {"oldText": "let total = compute(3);", "newText": "let total = compute(4);"},
            {"oldText": "fn gamma() {", "newText": "fn third() {"}
        ]),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Cannot edit src/lib.rs: edits[0]: oldText matches 2 times, at lines 3, 8; add surrounding context or set replaceAll\n\
         edits[2]: oldText not found; closest lines:\n  2:     let total = compute(1);\n  7:     let total = compute(2);\n\
         edits[3]: oldText not found"
    );
    assert!(matches!(error.failure(), Some(AgentFailure::InvalidEdit { .. })));
    tools.assert_lib(|| ROUTES);
    let many = (1..=12).map(|n| format!("x{n}")).collect::<Vec<_>>().join("\n");
    std::fs::write(tools.lib(), &many).unwrap();
    let error = edit(&tools, json!([{"oldText": "x", "newText": "y"}])).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Cannot edit src/lib.rs: edits[0]: oldText matches 12 times, at lines 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, …; add surrounding context or set replaceAll"
    );
    let long = format!("{} = compute(5);", "value".repeat(30));
    std::fs::write(tools.lib(), &long).unwrap();
    let error = edit(&tools, json!([{"oldText": format!("{} = compute(6);", "value".repeat(30)), "newText": "x"}])).await.unwrap_err();
    assert_eq!(error.to_string(), format!("Cannot edit src/lib.rs: edits[0]: oldText not found; closest lines:\n  1: {}…", &long[..120]));
}

#[tokio::test]
async fn unique_matches_may_differ_only_in_whitespace_at_line_ends() {
    let tools = TestTools::editing();
    std::fs::write(tools.lib(), ROUTES.replace("compute(1);", "compute(1);   ")).unwrap();
    let error = edit(&tools, json!([{"oldText": "  log(total); \n}", "newText": "}"}])).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Cannot edit src/lib.rs: edits[0]: oldText matches 2 times after ignoring whitespace, at lines 3, 8; add surrounding context"
    );
    let error = edit(&tools, json!([{"oldText": "\t\n", "newText": "x"}])).await.unwrap_err();
    assert_eq!(error.to_string(), "Cannot edit src/lib.rs: edits[0]: oldText not found");
    let trailing = edit(
        &tools,
        json!([
            {"oldText": "    let total = compute(1);\n    log(total);", "newText": "    log(compute(1));"},
            {"oldText": "fn beta() {", "newText": "fn second() {"}
        ]),
    )
    .await
    .unwrap();
    assert!(text(&trailing).starts_with("edits[0] matched only after ignoring whitespace.\nIndex: src/lib.rs\n"), "{}", text(&trailing));
    assert_eq!(trailing.details["diff"], text(&trailing).split_once('\n').unwrap().1);
    let expected = ROUTES.replace("    let total = compute(1);\n    log(total);", "    log(compute(1));").replace("beta", "second");
    tools.assert_lib(|| &expected);
    let indented = edit(
        &tools,
        json!([{"oldText": "      let total = compute(2);\n", "newText": "    let total = compute(20);\n"}, {"oldText": "fn alpha", "newText": "fn first"}]),
    )
    .await
    .unwrap();
    assert!(text(&indented).starts_with("edits[0] matched only after ignoring whitespace.\n"));
    let expected = expected.replace("compute(2)", "compute(20)").replace("alpha", "first");
    tools.assert_lib(|| &expected);
    let crlf = "let config = {\r\n      value: 1,\r\n      other: 2,\r\n};\r\n";
    std::fs::write(tools.lib(), crlf).unwrap();
    let seven = edit(&tools, json!([{"oldText": "       value: 1,\r\n       other: 2,", "newText": "      value: 3,\n      other: 4,"}]))
        .await
        .unwrap();
    assert!(text(&seven).starts_with("edits[0] matched only after ignoring whitespace.\n"));
    tools.assert_lib(|| "let config = {\r\n      value: 3,\r\n      other: 4,\r\n};\r\n");
    std::fs::write(tools.lib(), "\tx = 1;  \n  x = 1;\n").unwrap();
    edit(&tools, json!([{"oldText": "\tx = 1;\n", "newText": "\tx = 2;\n"}])).await.unwrap();
    tools.assert_lib(|| "\tx = 2;\n  x = 1;\n");
}

#[tokio::test]
async fn replace_all_rewrites_every_exact_occurrence() {
    let tools = TestTools::editing();
    std::fs::write(tools.lib(), ROUTES.replace('\n', "\r\n")).unwrap();
    let error = edit(&tools, json!([{"oldText": "log(total); ", "newText": "emit(total);", "replaceAll": true}])).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Cannot edit src/lib.rs: edits[0]: oldText not found; closest lines:\n  3:     log(total);\n  8:     log(total);"
    );
    let error = edit(
        &tools,
        json!([{"oldText": "total", "newText": "sum", "replaceAll": true}, {"oldText": "let total = compute(1);", "newText": "let sum = 1;"}]),
    )
    .await
    .unwrap_err();
    assert_eq!(error.to_string(), "Cannot edit src/lib.rs: edits[0] and edits[1]: replacement ranges overlap");
    tools.assert_lib(|| ROUTES.replace('\n', "\r\n"));
    let replaced = edit(
        &tools,
        json!([
            {"oldText": "log(total);", "newText": "emit(total);", "replaceAll": true},
            {"oldText": "fn alpha() {", "newText": "fn first() {", "replaceAll": null},
            {"oldText": "fn beta() {", "newText": "fn second() {", "replaceAll": false}
        ]),
    )
    .await
    .unwrap();
    assert!(text(&replaced).starts_with("Index: src/lib.rs\n"));
    let expected = ROUTES.replace("log(total)", "emit(total)").replace("alpha", "first").replace("beta", "second").replace('\n', "\r\n");
    tools.assert_lib(|| &expected);
}

/// Bruno 29528, 29755: bash's description names ripgrep and jq, each only
/// when it resolves on the PATH the agent's commands run with: telling
/// agents to use a program they then can't run taught them to stop trying it.
// Executable bits; on Windows the tools are Claktory's own, always there.
#[cfg(unix)]
#[test]
fn bash_names_ripgrep_and_jq_only_when_they_are_on_the_path() {
    use std::os::unix::fs::PermissionsExt;
    let rg_hint = "ripgrep (rg) is installed: prefer it to grep -r and find for searching code and files.";
    let jq_hint = "jq is installed: use it to read and filter JSON.";
    let bash = |tools: &[Tool]| tool(tools, "bash").spec.description.to_string();
    let program = |dir: &Path, name: &str, mode: u32| {
        std::fs::write(dir.join(name), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(dir.join(name), std::fs::Permissions::from_mode(mode)).unwrap();
    };
    let (empty, only_rg, only_jq, plain) =
        (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    program(only_rg.path(), "rg", 0o755);
    program(only_jq.path(), "jq", 0o755);
    // A file that isn't a program doesn't count.
    program(plain.path(), "rg", 0o644);
    program(plain.path(), "jq", 0o644);
    let path = |dirs: &[&Path]| std::env::join_paths(dirs).unwrap().to_string_lossy().into_owned();
    let fresh = || coding_tools(Path::new("/tmp/w"), &[], None);

    let mut tools = fresh();
    let before = bash(&tools);
    factory_agent::tools::installed(&mut tools, &path(&[empty.path(), plain.path()]));
    assert_eq!(bash(&tools), before, "not there: not named");

    // Each on its own.
    let mut tools = fresh();
    factory_agent::tools::installed(&mut tools, &path(&[only_rg.path()]));
    let described = bash(&tools);
    assert!(described.contains(&format!("directly in command. {rg_hint} Run tests")) && !described.contains("jq"), "{described}");
    let mut tools = fresh();
    factory_agent::tools::installed(&mut tools, &path(&[only_jq.path()]));
    let described = bash(&tools);
    assert!(described.contains(&format!("directly in command. {jq_hint} Run tests")) && !described.contains("rg"), "{described}");

    // Both, once each however often it's called.
    let mut tools = fresh();
    factory_agent::tools::installed(&mut tools, &path(&[empty.path(), only_rg.path(), only_jq.path()]));
    factory_agent::tools::installed(&mut tools, &path(&[only_rg.path(), only_jq.path()]));
    let described = bash(&tools);
    assert_eq!((described.matches("ripgrep").count(), described.matches("jq is").count()), (1, 1), "once: {described}");
    assert!(
        described.contains(&format!(
            "put the script text directly in command. {rg_hint} {jq_hint} Run tests and inspect git diff. timeout is seconds"
        )),
        "{described}"
    );
    assert_eq!(tool(&tools, "read_file").spec.description, tool(&fresh(), "read_file").spec.description);
}
