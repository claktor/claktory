//! claktory-qa end to end: a sandbox comes up with its account service,
//! signs in, is driven through the protocol, refuses what would reach
//! outside it, and goes without a trace.
//!
//! It runs the factoryd and claktory-account built beside claktory-qa, as
//! `cargo test --workspace` builds them; alone (`cargo test -p claktory-qa`)
//! build them first: `cargo build -p factoryd -p claktory-account`. Without
//! them the tests fail, never pass unseen, unless CLAKTORY_QA_SKIP_E2E=1.

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
        let _ = Command::new(QA).args(["down", "--all-agents"]).env("CLAKTORY_QA_ROOT", &self.0).output();
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Whether the factoryd and claktory-account it runs are built beside
/// claktory-qa. Missing, the test fails rather than passing unseen, unless
/// CLAKTORY_QA_SKIP_E2E=1 says to skip it.
fn built() -> bool {
    let bin = Path::new(QA).parent().expect("bin");
    if bin.join("factoryd").is_file() && bin.join("claktory-account").is_file() {
        return true;
    }
    if std::env::var("CLAKTORY_QA_SKIP_E2E").is_ok_and(|skip| skip == "1") {
        eprintln!("skipped (CLAKTORY_QA_SKIP_E2E=1): factoryd and claktory-account aren't built");
        return false;
    }
    panic!(
        "build factoryd and claktory-account first (cargo build -p factoryd -p claktory-account), or set CLAKTORY_QA_SKIP_E2E=1 to skip this test"
    );
}

#[test]
fn a_sandbox_comes_up_is_driven_and_goes() {
    if !built() {
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
    if !built() {
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

/// Whether the factoryd beside claktory-qa has the QA provider override
/// (built with factoryd's qa-provider feature, as scripts/qa.sh builds it).
fn qa_build() -> bool {
    let factoryd = Path::new(QA).parent().expect("bin").join("factoryd");
    let bytes = std::fs::read(factoryd).unwrap_or_default();
    let var = b"FACTORY_QA_ANTHROPIC_URL";
    bytes.windows(var.len()).any(|window| window == var)
}

/// `up --claude`: Claude signed in, fake, on qa-model's Anthropic side. A
/// factoryd without the QA override (the workspace's own build, as a
/// release) is refused before anything reaches Anthropic. With it (built
/// as `cargo build -p factoryd -p claktory-qa --features
/// factoryd/qa-provider`, or by scripts/qa.sh), an agent runs on Claude,
/// waits at the usage limit `limit` sets, carries on at Try now once it's
/// lifted, and by itself once it resets.
#[test]
fn a_fake_claude_sign_in_reaches_its_usage_limit() {
    if !built() {
        return;
    }
    let root = Root(std::env::temp_dir().join(format!("claktory-qa-claude-{}", std::process::id())));
    let root = root.0.as_path();
    if !qa_build() {
        let (code, json) = qa(root, &["up", "four", "--no-account", "--claude"]);
        assert_eq!(code, 1, "{json}");
        assert!(json["message"].as_str().is_some_and(|message| message.contains("QA provider override")), "{json}");
        assert!(!root.join("four").exists(), "nothing is left of it");
        return;
    }
    let up = ok(root, &["up", "three", "--no-account", "--claude"]);
    assert_eq!((&up["model"], &up["claude"]["account"]), (&Value::from("anthropic/claude-haiku-4-5"), &Value::from("claude@example.test")));
    assert!(refused(root, &["send", "three", "StartLogin", r#"{"provider_id": "anthropic"}"#]).contains("no real model"));
    ok(root, &["send", "three", "CreateProject", r#"{"name": "Site"}"#]);
    ok(root, &["send", "three", "CreateAgent", r#"{"project_id": "site", "name": "Lea"}"#]);
    assert_eq!(ok(root, &["workspace", "three", "agents[name=Lea].model_available"])["value"], true);
    let lea = ok(root, &["workspace", "three", "agents[name=Lea].id"])["value"].as_str().expect("id").to_owned();

    // On Claude's wire: the script's answers, tools and all.
    assert_eq!(ok(root, &["ask", "three", "Lea", "Hello"])["reply"], "OK: Hello");
    let ran = ok(root, &["ask", "three", "Lea", r#"call bash {"command": "echo hi from claude"}"#]);
    assert!(ran["reply"].as_str().is_some_and(|reply| reply.ends_with("answered: hi from claude")), "{ran}");
    let asked = ok(root, &["model", "three", "--agent", "Lea", "--last", "1"]);
    assert_eq!(asked["requests"][0]["wire"], "anthropic", "{asked}");
    // Who's signed in, as Anthropic says.
    let account = ok(root, &["workspace", "three", "providers[id=anthropic].accounts[0].label"]);
    assert_eq!(account["value"], "claude@example.test");

    // At its limit: Lea waits, saying until when; Try now once it's lifted.
    let waiting_until = |root: &Path| -> u64 {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let value = ok(root, &["workspace", "three", "agents[name=Lea].usage_wait_until_ms"])["value"].clone();
            let ms = value.as_str().and_then(|ms| ms.parse::<u64>().ok()).or_else(|| value.as_u64()).unwrap_or(0);
            if ms > 0 || std::time::Instant::now() > deadline {
                return ms;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    };
    let replied = |root: &Path, text: &str, secs: u64| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
        loop {
            let last = ok(root, &["transcript", "three", "Lea", "--last", "1"]);
            if last["entries"][0]["assistant"]["text"] == text {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "no {text:?} after {secs} s: {last}");
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    };
    let until = ok(root, &["limit", "three", "--for", "600"])["limited_until_ms"].as_u64().expect("until");
    ok(root, &["send", "three", "SendMessage", &format!(r#"{{"agent_id": "{lea}", "text": "Still there?"}}"#)]);
    let waits = waiting_until(root);
    assert!(waits >= until - 1000, "Lea waits until {waits}, the limit resets at {until}");
    assert!(ok(root, &["workspace", "three", "agents[name=Lea].usage_wait"])["value"].as_str().is_some_and(|wait| !wait.is_empty()));
    ok(root, &["limit", "three", "--off"]);
    ok(root, &["send", "three", "TryAgainNow", &format!(r#"{{"agent_id": "{lea}"}}"#)]);
    replied(root, "OK: Still there?", 60);

    // And by itself, once the limit resets.
    ok(root, &["limit", "three", "--for", "3"]);
    ok(root, &["send", "three", "SendMessage", &format!(r#"{{"agent_id": "{lea}", "text": "And now?"}}"#)]);
    assert!(waiting_until(root) > 0, "Lea waits");
    replied(root, "OK: And now?", 120);
    let env = Command::new(QA).args(["env", "three"]).env("CLAKTORY_QA_ROOT", root).output().expect("env");
    assert!(String::from_utf8_lossy(&env.stdout).contains("export FACTORY_QA_ANTHROPIC_URL='http://127.0.0.1:"));
    ok(root, &["down", "three"]);
}

/// A PNG of `width` × `height` that doesn't compress, as big as a
/// screenshot's bytes get.
fn noisy_png(width: u32, height: u32, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    let pixels = image::RgbImage::from_fn(width, height, |_, _| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let [red, green, blue, ..] = state.to_le_bytes();
        image::Rgb([red, green, blue])
    });
    let mut bytes = Vec::new();
    pixels.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).expect("png");
    bytes
}

/// An agent holding three large screenshots holds a few thousand
/// tokens of them, what its provider charges for their size, never their
/// base64's million: it doesn't compact, before a restart or after.
#[test]
fn an_agent_holding_three_large_images_doesnt_compact_after_a_restart() {
    if !built() {
        return;
    }
    let root = Root(std::env::temp_dir().join(format!("claktory-qa-images-{}", std::process::id())));
    let root = root.0.as_path();
    std::fs::create_dir_all(root).expect("root");
    let script = root.join("script.json");
    std::fs::write(&script, r#"{"default": "Noted."}"#).expect("script");
    let mut paths = Vec::new();
    for number in 1..=3_u64 {
        let path = root.join(format!("shot-{number}.png"));
        let png = noisy_png(1100, 800, number);
        assert!(png.len() > 2_000_000, "{} bytes", png.len());
        std::fs::write(&path, png).expect("png");
        paths.push(path.to_str().expect("path").to_owned());
    }
    ok(root, &["up", "five", "--no-account", "--script", script.to_str().expect("path")]);
    ok(root, &["send", "five", "CreateProject", r#"{"name": "Site"}"#]);
    ok(root, &["send", "five", "CreateAgent", r#"{"project_id": "site", "name": "Lea"}"#]);
    let tokens = |root: &Path| -> u64 {
        let value = ok(root, &["workspace", "five", "agents[name=Lea].context_tokens"])["value"].clone();
        value.as_str().and_then(|tokens| tokens.parse().ok()).or_else(|| value.as_u64()).unwrap_or(0)
    };
    let compactions = |root: &Path| -> usize {
        let all = ok(root, &["transcript", "five", "Lea", "--from", "0"]);
        all["entries"].as_array().expect("entries").iter().filter(|entry| entry.get("compaction").is_some()).count()
    };
    assert_eq!(ok(root, &["ask", "five", "Lea", "Hello"])["reply"], "Noted.");
    let before = tokens(root);
    assert!(before > 0, "the model reported its input");

    // Copied into Lea's folder and looked at, all three.
    let copy = format!("cp {} .", paths.join(" "));
    let looks = format!(
        "call bash {}\ncall read_image {{\"path\": \"shot-1.png\"}}\ncall read_image {{\"path\": \"shot-2.png\"}}\ncall read_image {{\"path\": \"shot-3.png\"}}",
        serde_json::json!({"command": copy})
    );
    ok(root, &["ask", "five", "Lea", &looks]);
    assert_eq!(ok(root, &["ask", "five", "Lea", "Seen them?"])["reply"], "Noted.");
    let with_images = tokens(root);
    // gpt-4.1-mini, at OpenAI's rule: 1100 × 800 is 6 tiles, 1105 tokens.
    let images = with_images - before;
    assert!((3 * 1105..3 * 1105 + 2000).contains(&images), "three images held {images} tokens ({before} before, {with_images} after)");
    assert_eq!(compactions(root), 0);

    // Restarted, it holds them (from their files) as it did, and carries on.
    ok(root, &["restart", "five"]);
    assert_eq!(ok(root, &["ask", "five", "Lea", "Still there?"])["reply"], "Noted.");
    let after = tokens(root);
    assert!(after < with_images + 500, "after a restart Lea holds {after} tokens, {with_images} before it");
    assert_eq!(compactions(root), 0, "Lea compacted");
    ok(root, &["down", "five"]);
}
