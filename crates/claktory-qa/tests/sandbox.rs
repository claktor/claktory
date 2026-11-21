//! claktory-qa end to end: a sandbox comes up with its account service,
//! signs in, is driven through the protocol, refuses what would reach
//! outside it, and goes without a trace.
//!
//! It runs the factoryd and claktory-account built beside claktory-qa, as
//! `cargo test --workspace` builds them; alone (`cargo test -p claktory-qa`)
//! build them first: `cargo build -p factoryd -p claktory-account`.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const QA: &str = env!("CARGO_BIN_EXE_claktory-qa");

fn qa(root: &Path, args: &[&str]) -> (i32, Value) {
    let output = Command::new(QA).args(args).env("CLAKTORY_QA_ROOT", root).output().expect("claktory-qa runs");
    let text = String::from_utf8_lossy(&output.stdout);
    let json = serde_json::from_str(text.trim()).unwrap_or_else(|_| Value::String(text.into_owned()));
    (output.status.code().unwrap_or(-1), json)
}

fn ok(root: &Path, args: &[&str]) -> Value {
    let (code, json) = qa(root, args);
    assert_eq!((code, &json["ok"]), (0, &Value::Bool(true)), "{args:?}: {json}");
    json
}

fn refused(root: &Path, args: &[&str]) -> String {
    let (code, json) = qa(root, args);
    assert_eq!(code, 3, "{args:?}: {json}");
    json["message"].as_str().unwrap_or_default().to_owned()
}

/// Removes the test's root even when it fails.
struct Root(PathBuf);

impl Drop for Root {
    fn drop(&mut self) {
        let _ = Command::new(QA).args(["down", "--all"]).env("CLAKTORY_QA_ROOT", &self.0).output();
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_sandbox_comes_up_is_driven_and_goes() {
    let bin = Path::new(QA).parent().expect("bin");
    if !bin.join("factoryd").is_file() || !bin.join("claktory-account").is_file() {
        eprintln!("skipped: build factoryd and claktory-account first (cargo build -p factoryd -p claktory-account)");
        return;
    }
    let root = Root(std::env::temp_dir().join(format!("claktory-qa-test-{}", std::process::id())));
    let root = root.0.as_path();

    let up = ok(root, &["up", "one"]);
    let port = up["port"].as_u64().expect("port");
    assert!((7500..8000).contains(&port), "{up}");
    assert!(up["account_service"].as_str().is_some_and(|url| url.starts_with("http://127.0.0.1:")), "{up}");
    assert!(refused(root, &["up", "one"]).contains("already a sandbox called one"));
    assert!(refused(root, &["up", "two", "--port", "7420"]).contains("real Claktory's"));
    // It never shows this computer's name: devices and the footer read qa-one.
    assert_eq!(up["machine_name"], "qa-one");
    assert_eq!(ok(root, &["workspace", "one", "remote.machine"])["value"], "qa-one");

    // Fresh, and asking to sign in, at its own service; then signed in.
    let state = ok(root, &["workspace", "one", "account.state"]);
    assert_eq!(state["value"], "CLAKTORY_ACCOUNT_STATE_SIGNED_OUT");
    let account = ok(root, &["sign-in", "one", "nora@example.test"]);
    assert_eq!(account["value"]["state"], "CLAKTORY_ACCOUNT_STATE_SIGNED_IN", "{account}");
    assert_eq!(ok(root, &["code", "one"])["to"], "nora@example.test");

    // Any request, by name, with its fields as JSON.
    let made = ok(root, &["send", "one", "CreateProject", r#"{"name": "Site"}"#, "--then", "projects[name=Site].id"]);
    assert_eq!((&made["answer"], &made["then"]), (&serde_json::json!({"ack": {}}), &Value::from("site")));
    ok(root, &["send", "one", "CreateAgent", r#"{"project_id": "site", "name": "Lea"}"#]);
    assert_eq!(ok(root, &["wait", "one", "agents[name=Lea].project_id", "site", "--secs", "5"])["value"], "site");
    let (code, error) = qa(root, &["send", "one", "CreateAgent", r#"{"project_id": "nowhere", "name": "Bo"}"#]);
    assert_eq!(code, 3, "{error}");
    assert_eq!(ok(root, &["requests", "CreatePipeInvite"])["fields"][0]["name"], "project_id");

    // factoryd's own commands reach it.
    let status = Command::new(QA).args(["cli", "one", "status", "--json"]).env("CLAKTORY_QA_ROOT", root).output().expect("status");
    let status: Value = serde_json::from_slice(&status.stdout).expect("json");
    assert_eq!(status["account"]["email"], "nora@example.test", "{status}");

    // Nothing reaches claktory.com, a real model or the real Claktories.
    assert!(refused(root, &["send", "one", "SetAccountService", r#"{"url": ""}"#]).contains("on this computer"));
    assert!(
        refused(root, &["send", "one", "SetAccountService", r#"{"url": "https://account.claktory.com"}"#]).contains("on this computer")
    );
    assert!(refused(root, &["send", "one", "StartLogin", r#"{"provider_id": "anthropic"}"#]).contains("no real model"));
    assert!(refused(root, &["cli", "one", "login", "anthropic"]).contains("no real model"));
    assert!(
        refused(root, &["cli", "one", "account", "sign-in", "--email", "a@b.c", "--service", "https://account.claktory.com"])
            .contains("on this computer")
    );
    let home = std::env::home_dir().expect("home");
    let real = Command::new(QA).args(["list"]).env("CLAKTORY_QA_ROOT", home.join(".factory")).output().expect("list");
    assert_eq!(real.status.code(), Some(3), "{}", String::from_utf8_lossy(&real.stdout));
    assert!(!home.join(".factory/sandboxes").exists(), "nothing was made there");

    // Gone, processes and folder.
    let folder = PathBuf::from(up["folder"].as_str().expect("folder"));
    assert_eq!(ok(root, &["down", "one"])["ended"], serde_json::json!(["one"]));
    assert!(!folder.exists());
    let (code, _) = qa(root, &["workspace", "one"]);
    assert_eq!(code, 3);
    assert!(std::net::TcpStream::connect(("127.0.0.1", u16::try_from(port).expect("port"))).is_err(), "it still answers");
}

#[test]
fn agents_in_a_sandbox_run_on_the_scripted_model() {
    let bin = Path::new(QA).parent().expect("bin");
    if !bin.join("factoryd").is_file() || !bin.join("claktory-account").is_file() {
        eprintln!("skipped: build factoryd and claktory-account first (cargo build -p factoryd -p claktory-account)");
        return;
    }
    let root = Root(std::env::temp_dir().join(format!("claktory-qa-model-{}", std::process::id())));
    let root = root.0.as_path();
    std::fs::create_dir_all(root).expect("root");
    let script = root.join("script.json");
    std::fs::write(&script, r#"{"rules": [{"agent": "Lea", "match": "launch", "reply": "Launch is Friday."}]}"#).expect("script");
    let bad = root.join("bad.json");
    std::fs::write(&bad, r#"{"rules": [{"match": "x"}]}"#).expect("script");
    let (code, json) = qa(root, &["up", "two", "--no-account", "--script", bad.to_str().expect("path")]);
    assert_eq!(code, 2, "{json}");
    assert!(json["message"].as_str().is_some_and(|message| message.contains("says nothing")), "{json}");

    let up = ok(root, &["up", "two", "--no-account", "--script", script.to_str().expect("path"), "--machine-name", "Studio"]);
    assert_eq!(up["model"], "openrouter/openai/gpt-4.1-mini");
    assert_eq!(ok(root, &["workspace", "two", "remote.machine"])["value"], "Studio", "the name chosen, for a video");
    ok(root, &["send", "two", "CreateProject", r#"{"name": "Site"}"#]);
    ok(root, &["send", "two", "CreateAgent", r#"{"project_id": "site", "name": "Lea"}"#]);
    assert_eq!(ok(root, &["workspace", "two", "agents[name=Lea].model_available"])["value"], true);

    // A rule, then the built-in answers: a tool call, and its result.
    assert_eq!(ok(root, &["ask", "two", "Lea", "When's the launch?"])["reply"], "Launch is Friday.");
    let ran = ok(root, &["ask", "two", "Lea", r#"call bash {"command": "echo hi from qa-model"}"#]);
    assert_eq!(ran["reply"], "bash answered: hi from qa-model", "{ran}");
    let call = ran["entries"].as_array().expect("entries").iter().find_map(|entry| entry.get("tool_call")).expect("a tool call");
    assert!(call["output"].as_str().is_some_and(|output| output.starts_with("hi from qa-model")), "{call}");
    assert_eq!(ok(root, &["ask", "two", "Rivet", "Hello"])["reply"], "OK: Hello");

    // A new script answers from the next request on.
    std::fs::write(&script, r#"{"default": "Noted."}"#).expect("script");
    ok(root, &["script", "two", script.to_str().expect("path")]);
    assert_eq!(ok(root, &["ask", "two", "Lea", "When's the launch?"])["reply"], "Noted.");

    // What it was asked, by whom, with which tools and system prompt.
    let asked = ok(root, &["model", "two", "--agent", "Lea", "--last", "1", "--system"]);
    let request = &asked["requests"][0];
    assert_eq!(request["last"]["role"], "user");
    assert!(request["tools"].as_array().is_some_and(|tools| tools.iter().any(|tool| tool == "bash")), "{request}");
    assert!(request["system"].as_str().is_some_and(|system| system.contains("You are Lea, a persistent agent")), "{request}");
    assert_eq!(ok(root, &["model", "two", "--agent", "Rivet"])["requests"].as_array().map(Vec::len), Some(1));

    // Its transcript: as the app opens it, the last few, or from a seq on.
    let replies = |json: &Value| -> Vec<String> {
        json["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .filter_map(|entry| entry["assistant"]["text"].as_str().filter(|text| !text.is_empty()).map(str::to_owned))
            .collect()
    };
    let opened = ok(root, &["transcript", "two", "Lea"]);
    assert_eq!(replies(&opened).last().map(String::as_str), Some("Noted."), "{opened}");
    let all = ok(root, &["transcript", "two", "Lea", "--from", "0"]);
    assert_eq!(all["entries"].as_array().map(Vec::len), opened["entries"].as_array().map(Vec::len), "{all}");
    assert_eq!(all["has_earlier"], false);
    let last = ok(root, &["transcript", "two", "Lea", "--last", "1"]);
    assert_eq!(replies(&last), ["Noted."], "{last}");

    // Restarted, even killed, it comes back with its model and its agents.
    let env = Command::new(QA).args(["env", "two"]).env("CLAKTORY_QA_ROOT", root).output().expect("env");
    let env = String::from_utf8_lossy(&env.stdout);
    assert!(env.contains("export OPENROUTER_BASE_URL='http://127.0.0.1:") && env.contains("export FACTORY_PORT="), "{env}");
    for how in [&["restart", "two"][..], &["restart", "two", "--kill"]] {
        let restarted = ok(root, how);
        assert_eq!(restarted["port"], up["port"], "{restarted}");
        assert_eq!(ok(root, &["workspace", "two", "agents[name=Lea].model_available"])["value"], true);
        assert_eq!(ok(root, &["ask", "two", "Lea", "Still there?"])["reply"], "Noted.");
    }
    ok(root, &["down", "two"]);
}
