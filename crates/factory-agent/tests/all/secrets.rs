//! Secrets in bash's commands: a fake vault resolves them, and their values
//! never show in what the agent sees.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use factory_agent::agent::{Agent, AgentOptions};
use factory_agent::secrets::Resolve;
use factory_agent::stream::{StreamFn, ThinkingLevel, event_stream};
use factory_agent::tools::{Tool, coding_tools};
use factory_agent::types::{AssistantPart, StreamEvent, ToolResult, ToolResultPart};
use factory_agent::{Redaction, Redactor, SecretUse, SecretVariable, Secrets};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use crate::support::{luna, reply};

const HANG: Duration = Duration::from_secs(60);
const KEY: &str = "sk_live_4f9a8b7c6d5e4f3a2b1c";
const TOKEN: &str = "kube-token-0123456789abcdef";
const PROD: &str = "prod-secret-abcdefgh";
const HIDDEN: &str = "[secret Stripe/STRIPE_KEY]";

fn variable(secret: &str, name: &str, value: &str, file: bool) -> SecretVariable {
    SecretVariable { secret: secret.into(), variable: name.into(), value: Zeroizing::new(value.into()), file, public: false, binary: false }
}

fn kubeconfig() -> String {
    format!("apiVersion: v1\nkind: Config\nusers:\n- name: deploy\n  user:\n    token: {TOKEN}\n")
}

/// A vault with Stripe (granted), Kube (a file, granted) and Prod (not
/// granted), for the agent "june"; and the uses it heard of.
fn vault() -> (Secrets, Arc<Mutex<Vec<SecretUse>>>) {
    let all = vec![
        variable("Stripe", "STRIPE_KEY", KEY, false),
        variable("Kube", "KUBECONFIG", &kubeconfig(), true),
        variable("Prod", "PROD_KEY", PROD, false),
    ];
    let redaction = Redaction::new(Redactor::new(&all));
    let resolve: Resolve = Arc::new(move |agent, names| {
        let all = all.clone();
        Box::pin(async move {
            assert_eq!(agent, "june");
            let mut found = Vec::new();
            for name in names {
                if !all.iter().any(|variable| variable.secret == name) {
                    return Err(format!("There's no secret {name}."));
                }
                if name == "Prod" {
                    return Err(format!("{name} isn't granted to you; ask with secret_request."));
                }
                found.extend(all.iter().filter(|variable| variable.secret == name).cloned());
            }
            Ok(found)
        })
    });
    let uses: Arc<Mutex<Vec<SecretUse>>> = Arc::default();
    let used = {
        let uses = uses.clone();
        Arc::new(move |used: SecretUse| uses.lock().push(used))
    };
    (Secrets { agent: "june".into(), resolve, used, redaction, filter: Vec::new() }, uses)
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

/// Every form of a value someone could spot it in.
fn leaks(text: &str, value: &str) -> bool {
    let hex: String = value.bytes().map(|byte| format!("{byte:02x}")).collect();
    let base64 = base64::engine::general_purpose::STANDARD_NO_PAD.encode(value);
    text.contains(value) || text.contains(&hex) || text.contains(&base64[..base64.len() - 2])
}

struct Watched {
    background: factory_agent::Background,
    finished: tokio::sync::mpsc::UnboundedReceiver<factory_agent::Finished>,
}

fn watched(dir: &Path) -> Watched {
    let (sender, finished) = tokio::sync::mpsc::unbounded_channel();
    let background = factory_agent::Background {
        dir: dir.join("tasks"),
        environment: Vec::new(),
        started: Arc::new(|_| {}),
        finished: Arc::new(move |done| {
            let _ = sender.send(done);
        }),
    };
    Watched { background, finished }
}

fn bash(dir: &Path, background: Option<&factory_agent::Background>) -> Tool {
    coding_tools(dir, &[], background.cloned()).into_iter().find(|tool| tool.spec.name == "bash").expect("bash")
}

/// Runs a call; its result and every update it streamed.
async fn call(tool: &Tool, args: Value, secrets: Option<&Secrets>) -> (Result<ToolResult, factory_agent::Error>, Vec<String>) {
    let updates: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = updates.clone();
    let update = move |partial: ToolResult| sink.lock().push(text(&partial));
    let result =
        tokio::time::timeout(HANG, tool.execute_with(&args, &CancellationToken::new(), &CancellationToken::new(), &update, secrets))
            .await
            .expect("no hang");
    let updates = updates.lock().clone();
    (result, updates)
}

#[tokio::test]
async fn a_granted_secret_reaches_its_command_only_and_never_shows_even_encoded_or_split() {
    let dir = tempfile::tempdir().unwrap();
    let watched = watched(dir.path());
    // With pipes (no background) and through a log (Claktory's way).
    for background in [None, Some(&watched.background)] {
        let (secrets, uses) = vault();
        let tool = bash(dir.path(), background);
        let command = r#"[ "${#STRIPE_KEY}" -eq 28 ] && echo set
echo "raw $STRIPE_KEY"
printf %s "$STRIPE_KEY" | base64
printf %s "$STRIPE_KEY" | od -An -tx1 | tr -d ' \n'; echo
printf '{"key":"%s"}\n' "$STRIPE_KEY"
printf 'split %s' "${STRIPE_KEY:0:12}"; sleep 0.4; printf '%s end\n' "${STRIPE_KEY:12}"
echo "to stderr $STRIPE_KEY" >&2"#;
        let (result, updates) = call(&tool, json!({"command": command, "secrets": ["Stripe"]}), Some(&secrets)).await;
        let result = result.expect("ran");
        let shown = text(&result);
        let streamed = updates.concat();
        for (what, output) in [("result", &shown), ("streamed", &streamed)] {
            assert!(output.contains("set\n"), "{what}: the command had it: {output}");
            assert!(!leaks(output, KEY), "{what} leaks the value: {output}");
            assert!(output.contains(&format!("raw {HIDDEN}")), "{what}: {output}");
            assert!(output.contains(&format!("split {HIDDEN} end")), "{what}: split across chunks: {output}");
            assert!(output.contains(&format!("{{\"key\":\"{HIDDEN}\"}}")), "{what}: {output}");
            assert!(output.contains(&format!("to stderr {HIDDEN}")), "{what}: {output}");
        }
        for update in &updates {
            assert!(!update.contains(&KEY[..12]), "no partial shows part of it: {update:?}");
        }
        let used = uses.lock().clone();
        assert_eq!(used.len(), 1, "{used:?}");
        assert_eq!((used[0].agent.as_str(), used[0].secrets.clone(), used[0].task.clone()), ("june", vec!["Stripe".to_owned()], None));
        assert_eq!(used[0].command, command);

        // The next command, asking for nothing, doesn't have it.
        let (result, _) = call(&tool, json!({"command": "echo \"[${STRIPE_KEY:-unset}]\""}), Some(&secrets)).await;
        assert!(text(&result.expect("ran")).starts_with("[unset]\n"));
        assert_eq!(uses.lock().len(), 1, "a command without secrets isn't a use");
    }
}

#[tokio::test]
async fn an_ungranted_or_unknown_secret_is_refused_and_nothing_runs() {
    let dir = tempfile::tempdir().unwrap();
    let (secrets, uses) = vault();
    let tool = bash(dir.path(), None);
    for (names, refusal) in [(json!(["Stripe", "Prod"]), "Prod isn't granted to you"), (json!(["Nope"]), "There's no secret Nope.")] {
        let (result, _) = call(&tool, json!({"command": "touch ran", "secrets": names}), Some(&secrets)).await;
        let error = result.expect_err("refused").to_string();
        assert!(error.contains(refusal), "{error}");
    }
    let (result, _) = call(&tool, json!({"command": "touch ran", "secrets": ["Stripe"]}), None).await;
    assert_eq!(result.expect_err("refused").to_string(), "Secrets aren't available here.");
    assert!(!dir.path().join("ran").exists());
    assert!(uses.lock().is_empty());
}

#[tokio::test]
async fn a_file_secret_is_a_private_file_removed_when_its_command_ends() {
    let dir = tempfile::tempdir().unwrap();
    let mut watched = watched(dir.path());
    let command = "ls -l \"$KUBECONFIG\" | cut -c1-10; echo \"path=$KUBECONFIG\"; cat \"$KUBECONFIG\"";
    for background in [None, Some(&watched.background)] {
        let (secrets, _) = vault();
        let tool = bash(dir.path(), background);
        let (result, _) = call(&tool, json!({"command": command, "secrets": ["Kube"]}), Some(&secrets)).await;
        let shown = text(&result.expect("ran"));
        assert!(shown.starts_with("-rw-------\n"), "{shown}");
        assert!(!shown.contains(TOKEN), "{shown}");
        assert!(shown.contains("[secret Kube/KUBECONFIG]"), "{shown}");
        let path = shown.lines().find_map(|line| line.strip_prefix("path=")).expect("its path");
        assert!(path.ends_with("/KUBECONFIG"), "{path}");
        assert!(!Path::new(path).exists(), "gone after the command: {path}");
        assert!(!Path::new(path).parent().unwrap().exists(), "its folder too");
    }

    // A background task keeps its file until it ends.
    let (secrets, uses) = vault();
    let tool = bash(dir.path(), Some(&watched.background));
    let args = json!({"command": "echo \"path=$KUBECONFIG\"; cat \"$KUBECONFIG\"; printf '%s\\n' \"$STRIPE_KEY\"; sleep 0.5; cat \"$KUBECONFIG\" >/dev/null && echo still", "secrets": ["Kube", "Stripe"], "run_in_background": true});
    let (result, _) = call(&tool, args, Some(&secrets)).await;
    let task = result.expect("started").details["task"].as_str().expect("task").to_owned();
    let done = tokio::time::timeout(HANG, watched.finished.recv()).await.expect("in time").expect("finished");
    assert_eq!(done.id, task);
    assert_eq!(done.status, "exit 0", "{}", done.tail);
    assert!(done.tail.ends_with("still\n"), "{}", done.tail);
    assert!(!done.tail.contains(TOKEN) && !leaks(&done.tail, KEY), "the task's end hides them: {}", done.tail);
    assert!(done.tail.contains(HIDDEN), "{}", done.tail);
    let path = done.tail.lines().find_map(|line| line.strip_prefix("path=")).expect("its path");
    assert!(!Path::new(path).exists(), "gone once it ended: {path}");
    let used = uses.lock().clone();
    assert_eq!(used.len(), 1);
    assert_eq!(used[0].secrets, ["Kube", "Stripe"]);
    assert_eq!(used[0].task.as_deref(), Some(task.as_str()));
}

#[tokio::test]
async fn a_command_that_moves_to_the_background_is_one_use_under_its_task() {
    let dir = tempfile::tempdir().unwrap();
    let mut watched = watched(dir.path());
    let (secrets, uses) = vault();
    let tool = bash(dir.path(), Some(&watched.background));
    let args = json!({"command": "echo start; sleep 1; echo \"late $STRIPE_KEY\"", "secrets": ["Stripe"], "timeout": 0.3});
    let (result, _) = call(&tool, args, Some(&secrets)).await;
    let task = result.expect("moved").details["movedToBackground"].as_str().expect("moved").to_owned();
    let done = tokio::time::timeout(HANG, watched.finished.recv()).await.expect("in time").expect("finished");
    assert_eq!(done.tail, format!("start\nlate {HIDDEN}\n"));
    let used = uses.lock().clone();
    assert_eq!(used.len(), 1, "{used:?}");
    assert_eq!(used[0].task.as_deref(), Some(task.as_str()));
}

#[tokio::test]
async fn bash_takes_secrets_only_where_it_was_given_them() {
    let dir = tempfile::tempdir().unwrap();
    let mut tools = coding_tools(dir.path(), &[], None);
    let plain = tools.iter().find(|tool| tool.spec.name == "bash").expect("bash").spec.clone();
    assert!(plain.parameters["properties"].get("secrets").is_none());
    factory_agent::tools::secrets(&mut tools);
    factory_agent::tools::secrets(&mut tools);
    let bash = tools.iter().find(|tool| tool.spec.name == "bash").expect("bash");
    assert_eq!(bash.spec.parameters["properties"]["secrets"]["anyOf"][0]["type"], "array");
    assert_eq!(bash.spec.description.matches("secrets names").count(), 1, "said once");
    assert_eq!(tools.iter().filter(|tool| tool.spec.parameters["properties"].get("secrets").is_some()).count(), 1, "bash only");
}

/// The agent's bash and read_file results, streamed and final, hide every
/// value in the vault.
#[tokio::test]
async fn nothing_the_agent_sees_or_records_holds_a_value() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".env"), format!("STRIPE_KEY={KEY}\nPROD_KEY={PROD}\n")).unwrap();
    let (secrets, _) = vault();
    let mut tools = coding_tools(dir.path(), &[], None);
    factory_agent::tools::secrets(&mut tools);
    let calls = Arc::new(Mutex::new(0));
    let stream: StreamFn = Arc::new(move |model, _context, _options| {
        let (sink, stream) = event_stream();
        *calls.lock() += 1;
        let content = if *calls.lock() == 1 {
            vec![
                AssistantPart::ToolCall {
                    id: "a".into(),
                    name: "bash".into(),
                    arguments: json!({"command": "printf '%s' \"$STRIPE_KEY\"; sleep 0.2; echo \" $STRIPE_KEY\"", "secrets": ["Stripe"]}),
                    thought_signature: None,
                },
                AssistantPart::ToolCall {
                    id: "b".into(),
                    name: "read_file".into(),
                    arguments: json!({"path": ".env"}),
                    thought_signature: None,
                },
                AssistantPart::ToolCall {
                    id: "c".into(),
                    name: "bash".into(),
                    arguments: json!({"command": "echo \"$STRIPE_KEY\"", "secrets": ["Prod"]}),
                    thought_signature: None,
                },
            ]
        } else {
            vec![AssistantPart::Text { text: "Done".into(), text_signature: None }]
        };
        let message = reply(model, content);
        sink.push(StreamEvent::Start { partial: message.clone() });
        sink.push(StreamEvent::Done { reason: message.stop_reason, message });
        Ok(stream)
    });
    let mut agent = Agent::new(AgentOptions {
        system_prompt: "You are a coding agent.".into(),
        model: luna(),
        thinking_level: ThinkingLevel::Medium,
        tools,
        messages: Vec::new(),
        stream,
        session_id: Some("june".into()),
        transform_context: None,
        should_stop_after_turn: None,
        before_tool_call: None,
        compact_at: None,
        inbox: None,
        after_compaction: None,
        secrets: Some(secrets),
        fallback_model: None,
    });
    let events = crate::support::collect(&mut agent);
    tokio::time::timeout(HANG, agent.prompt("Deploy")).await.expect("no hang").expect("ran");
    let recorded = serde_json::to_string(&*events.lock()).unwrap() + &serde_json::to_string(&agent.state.messages).unwrap();
    assert!(!recorded.contains(KEY) && !recorded.contains(PROD), "{recorded}");
    assert!(recorded.contains(&format!("{HIDDEN} {HIDDEN}")), "bash's output, hidden");
    assert!(recorded.contains("STRIPE_KEY=[secret Stripe/STRIPE_KEY]"), "read_file's too");
    assert!(recorded.contains("PROD_KEY=[secret Prod/PROD_KEY]"), "values the agent wasn't given too");
    assert!(recorded.contains("Prod isn't granted to you"), "the refusal");
}

/// A command with secrets writes its log through the filter, which gets the
/// values to hide; the command itself doesn't see them there.
#[tokio::test]
async fn a_command_with_secrets_writes_its_log_through_the_filter() {
    let dir = tempfile::tempdir().unwrap();
    let mut watched = watched(dir.path());
    let (mut secrets, _) = vault();
    // Stands in for factoryd's `redact`: it says what it was given, and
    // copies the output in capitals.
    secrets.filter = ["bash", "-c", "printf 'filter has %s\\n' \"${CLAKTORY_REDACT:+the values}\"; tr a-z A-Z"].map(Into::into).to_vec();
    let tool = bash(dir.path(), Some(&watched.background));
    let command = "echo \"command has ${CLAKTORY_REDACT:-nothing extra}\"; [ -n \"$STRIPE_KEY\" ] && echo set; exit 3";

    // In the foreground: the result is read from the log.
    let (result, _) = call(&tool, json!({"command": command, "secrets": ["Stripe"]}), Some(&secrets)).await;
    let shown = text(&result.expect("ran"));
    assert!(shown.starts_with("filter has the values\nCOMMAND HAS NOTHING EXTRA\nSET\n\n[exit 3, "), "{shown}");

    // In the background: the log on disk is the filter's.
    let args = json!({"command": command, "secrets": ["Stripe"], "run_in_background": true});
    let (result, _) = call(&tool, args, Some(&secrets)).await;
    result.expect("started");
    let done = tokio::time::timeout(HANG, watched.finished.recv()).await.expect("in time").expect("finished");
    assert_eq!(done.status, "exit 3");
    assert_eq!(std::fs::read_to_string(&done.log).unwrap(), "filter has the values\nCOMMAND HAS NOTHING EXTRA\nSET\n");

    // A command without secrets doesn't go through it.
    let (result, _) = call(&tool, json!({"command": "echo plain"}), Some(&secrets)).await;
    assert!(text(&result.expect("ran")).starts_with("plain\n"));
}
