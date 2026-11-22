//! Agents are prompted by more than the person: by other agents and by their
//! own background tasks finishing. Those arrive as events, which the model
//! reads tagged with where they're from and the app shows as events. Anything
//! that arrives while an agent works reaches it after its current step, and
//! factory.db keeps a record of all of it.

use std::time::Duration;

use crate::common::{Client, View, apply_request, create_agent, create_project, describe, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind};

fn select(agent: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

fn idle(view: &View, agent: &str) -> bool {
    view.workspace.agents.iter().any(|candidate| candidate.id == agent && candidate.status() == AgentStatus::Idle)
}

#[tokio::test]
async fn an_agent_messages_another_which_wakes_and_knows_who_it_is_from() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let mut view = View::default();
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;

    apply_request(&mut client, &mut view, select("builder")).await;
    let ask = r#"call agents_send {"to":"writer","text":"The build is green."}"#;
    apply_request(&mut client, &mut view, send_message("builder", ask)).await;
    view.follow(&mut client, |view| view.entries.len() == 3 && idle(view, "builder")).await;
    assert_eq!(describe(&view.entries)[1..], ["tool agents_send: Sent.", "assistant: done: Sent."]);

    // The writer woke up on its own.
    apply_request(&mut client, &mut view, select("writer")).await;
    view.follow(&mut client, |view| view.entries.len() == 2 && idle(view, "writer")).await;
    let Some(Kind::Event(event)) = &view.entries[0].kind else { panic!("an event, not the person's message: {:?}", view.entries[0]) };
    assert_eq!(event.source(), v1::EventSource::Agent);
    assert_eq!((event.from.as_str(), event.from_id.as_str(), event.text.as_str()), ("Builder", "builder", "The build is green."));
    assert_eq!(
        describe(&view.entries)[1],
        format!("assistant: echo: <event source=\"agent\" from=\"Builder\" id=\"builder\" msg=\"{}\">\nThe build is green.\n</event>", event.message_id),
        "the model reads it tagged with who it's from"
    );

    // Agents know who they can message.
    apply_request(&mut client, &mut view, send_message("writer", "who else is here")).await;
    view.follow(&mut client, |view| view.entries.len() == 4 && idle(view, "writer")).await;
    let messages = r#"call agents_send {"to":"nobody","text":"hi"}"#;
    apply_request(&mut client, &mut view, send_message("writer", messages)).await;
    view.follow(&mut client, |view| view.entries.len() == 7 && idle(view, "writer")).await;
    assert_eq!(describe(&view.entries)[5], "tool agents_send: No agent you can message is called \"nobody\"; you can message: builder.");
}

#[tokio::test]
async fn an_agent_hears_when_its_background_task_finishes() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let mut view = View::default();
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, select("builder")).await;

    let start_task = r#"call bash {"command":"sleep 0.3; echo built","run_in_background":true}"#;
    apply_request(&mut client, &mut view, send_message("builder", start_task)).await;
    // It's started, the run ends, and then the task's event wakes the agent.
    let finished =
        tokio::time::timeout(Duration::from_secs(10), view.follow(&mut client, |view| view.entries.len() == 5 && idle(view, "builder")))
            .await;
    assert!(finished.is_ok(), "{:#?}", describe(&view.entries));
    let entries = describe(&view.entries);
    assert!(entries[1].starts_with("tool bash: Started task-1 in the background"), "{}", entries[1]);
    // With how long it ran.
    let (head, rest) = entries[3].split_once(", ").expect("a duration");
    assert_eq!(
        (head, rest.split_once("s): ").map(|(_, command)| command)),
        ("event from task-1: task-1 finished (exit 0", Some("sleep 0.3; echo built"))
    );
    let Some(Kind::Event(event)) = &view.entries[3].kind else { panic!("an event") };
    assert_eq!(event.source(), v1::EventSource::Task);
    assert!(event.text.ends_with("End of its output:\nbuilt\n"), "{}", event.text);
    let log = dir.join("home/agents/builder/tasks/task-1.log");
    assert_eq!(std::fs::read_to_string(&log).expect("the task's log"), "built\n");
}

/// factory.db's rows for a query, as strings joined by " | ".
fn records(dir: &std::path::Path, query: &str) -> Vec<String> {
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
    let mut statement = db.prepare(query).expect("query");
    let columns = statement.column_count();
    let mut rows = statement.query([]).expect("rows");
    let mut out = Vec::new();
    while let Some(row) = rows.next().expect("row") {
        let mut fields = Vec::new();
        for column in 0..columns {
            let value: rusqlite::types::Value = row.get(column).expect("value");
            fields.push(match value {
                rusqlite::types::Value::Null => "null".to_owned(),
                rusqlite::types::Value::Integer(number) => number.to_string(),
                rusqlite::types::Value::Text(text) => text,
                other => format!("{other:?}"),
            });
        }
        out.push(fields.join(" | "));
    }
    out
}

#[tokio::test]
async fn a_message_sent_while_an_agent_works_reaches_it_after_its_current_step() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let mut view = View::default();
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, select("builder")).await;

    apply_request(&mut client, &mut view, send_message("builder", r#"call bash {"command":"sleep 1"}"#)).await;
    view.follow(&mut client, |view| view.entries.iter().any(|entry| matches!(entry.kind, Some(Kind::ToolCall(_))))).await;
    // Sent while its command runs.
    apply_request(&mut client, &mut view, send_message("builder", "and also this")).await;
    view.follow(&mut client, |view| view.entries.len() == 4 && idle(view, "builder")).await;
    let entries = describe(&view.entries);
    assert!(entries[1].starts_with("tool bash"), "{entries:?}");
    // It read the message as soon as the command finished, before it went on.
    assert_eq!(entries[2..], ["user: and also this", "assistant: echo: and also this"]);

    // Both messages went to the same run, which then finished.
    let messages = records(&dir, "SELECT agent, source, text, delivered IS NOT NULL, run, dropped FROM messages ORDER BY id");
    assert_eq!(
        messages,
        [r#"builder | person | call bash {"command":"sleep 1"} | 1 | 1 | null"#, "builder | person | and also this | 1 | 1 | null"]
    );
    let runs = records(&dir, "SELECT id, agent, outcome FROM runs");
    assert_eq!(runs, ["1 | builder | finished"]);
}

#[tokio::test]
async fn a_background_task_outlives_the_backend_and_its_agent_still_hears_when_it_ends() {
    let dir = temp_dir();
    // The first backend runs on its own runtime, so it can stop for good.
    let old = tokio::runtime::Runtime::new().expect("runtime");
    let (addr, _) = old
        .spawn({
            let dir = dir.clone();
            async move { start(&dir).await }
        })
        .await
        .expect("first backend");
    let (mut client, _, _) = Client::connect(addr).await;
    let mut view = View::default();
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, select("builder")).await;
    let start_task = r#"call bash {"command":"sleep 1.5; echo late","run_in_background":true}"#;
    apply_request(&mut client, &mut view, send_message("builder", start_task)).await;
    view.follow(&mut client, |view| view.entries.len() == 3 && idle(view, "builder")).await;
    drop(client);
    old.shutdown_background();

    let (addr, _new) = start(&dir).await;
    // Builder is still selected: its transcript comes with the welcome, and
    // what it does next (the task may have ended already) follows it.
    let (mut client, workspace, transcript) = Client::connect(addr).await;
    let entries = transcript.expect("Builder's transcript").entries;
    let mut view = View { workspace, entries, ..View::default() };
    let finished =
        tokio::time::timeout(Duration::from_secs(10), view.follow(&mut client, |view| view.entries.len() == 5 && idle(view, "builder")))
            .await;
    assert!(finished.is_ok(), "{:#?}", describe(&view.entries));
    let entry = describe(&view.entries)[3].clone();
    assert!(entry.starts_with("event from task-1: task-1 finished (exit 0, ") && entry.ends_with("s): sleep 1.5; echo late"), "{entry}");
    let Some(Kind::Event(event)) = &view.entries[3].kind else { panic!("an event") };
    assert!(event.text.ends_with("End of its output:\nlate\n"), "{}", event.text);
    assert_eq!(records(&dir, "SELECT agent, task, status FROM tasks"), ["builder | task-1 | exit 0"]);
}

#[tokio::test]
async fn the_persons_message_moves_a_running_command_to_the_background_and_another_agents_lets_it_run_a_minute() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let mut view = View::default();
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Lead", "")).await;
    apply_request(&mut client, &mut view, select("builder")).await;
    // Waits for the file, for a minute at most, so a failing test leaves nothing running.
    let wait_for = |file: &str| {
        format!(
            r#"call bash {{"command":"echo started; n=0; until [ -e {file} ] || [ $n -ge 600 ]; do sleep 0.1; n=$((n+1)); done; echo finished"}}"#
        )
    };

    // The person's message: the command moves at once and runs on.
    apply_request(&mut client, &mut view, send_message("builder", &wait_for("go"))).await;
    view.follow(&mut client, |view| view.entries.iter().any(|entry| matches!(entry.kind, Some(Kind::ToolCall(_))))).await;
    apply_request(&mut client, &mut view, send_message("builder", "and also this")).await;
    view.follow(&mut client, |view| view.entries.len() == 4 && idle(view, "builder")).await;
    let entries = describe(&view.entries);
    assert!(entries[1].starts_with("tool bash: A new message came for you after "), "{entries:?}");
    assert!(entries[1].contains("so this command moved to the background as task-1"), "{entries:?}");
    assert_eq!(entries[2..], ["user: and also this", "assistant: echo: and also this"]);
    std::fs::write(dir.join("site/go"), "").expect("go");
    view.follow(&mut client, |view| view.entries.len() == 6 && idle(view, "builder")).await;
    let entries = describe(&view.entries);
    assert!(entries[4].starts_with("event from task-1: "), "{entries:?}");

    // Another agent's message waits while the command has run less than a minute.
    apply_request(&mut client, &mut view, send_message("builder", &wait_for("go2"))).await;
    view.follow(&mut client, |view| view.entries.len() == 8).await;
    apply_request(&mut client, &mut view, send_message("lead", r#"call agents_send {"to":"builder","text":"status?"}"#)).await;
    view.follow(&mut client, |view| view.agent("builder").queued_messages == 1).await;
    std::fs::write(dir.join("site/go2"), "").expect("go2");
    view.follow(&mut client, |view| view.agent("builder").queued_messages == 0 && idle(view, "builder")).await;
    let entries = describe(&view.entries);
    assert!(entries[7].starts_with("tool bash: started\nfinished\n\n[exit 0, "), "{entries:?}");
    assert!(entries[8].starts_with("event from Lead"), "{entries:?}");
}
