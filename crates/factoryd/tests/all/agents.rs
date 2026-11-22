use crate::common::{
    Client, MODEL, Scripted, View, apply_request, create_agent, create_project, describe, send_message, start, start_with, temp_dir,
};
use factory_protocol::v1::{self, AgentStatus, client_message};
use factoryd::config::Config;

/// Waits until factory.db records that the agent has the message.
async fn delivered(dir: &std::path::Path, text: &str) {
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("open factory.db");
    for _ in 0..500 {
        let delivered: Option<i64> =
            db.query_row("SELECT delivered FROM messages WHERE text = ?1", [text], |row| row.get(0)).expect("the message is recorded");
        if delivered.is_some() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("\"{text}\" was never delivered");
}

fn idle(view: &View, id: &str) -> bool {
    let agent = view.agent(id);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

#[tokio::test]
async fn a_message_streams_a_reply_and_the_conversation_survives_a_restart() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    client.ok(create_project("Site", &dir.join("site"))).await;
    let mut view = View::default();
    for event in client.ok(create_agent("site", "Builder", "")).await {
        view.apply(&event.body.expect("body"));
    }

    for event in client.ok(send_message("builder", "hello")).await {
        view.apply(&event.body.expect("body"));
    }
    view.follow(&mut client, |view| view.entries.len() == 2 && idle(view, "builder")).await;
    assert_eq!(describe(&view.entries), ["user: hello", "assistant: echo: hello"]);
    assert_eq!(view.deltas, 2, "the reply streamed in");
    assert_eq!(view.agent("builder").context_tokens, 1_200);

    // A second message continues the same conversation.
    for event in client.ok(send_message("builder", "again")).await {
        view.apply(&event.body.expect("body"));
    }
    view.follow(&mut client, |view| view.entries.len() == 4 && idle(view, "builder")).await;

    // A new backend on the same home picks up where this one left off.
    let (restarted, _factory) = start(&dir).await;
    let (_client, workspace, transcript) = Client::connect(restarted).await;
    let transcript = transcript.expect("selected agent's transcript");
    assert_eq!(describe(&transcript.entries), ["user: hello", "assistant: echo: hello", "user: again", "assistant: echo: again"]);
    assert_eq!(transcript.entries, view.entries, "same entries as the live view, seqs included");
    let builder = workspace.agents.iter().find(|agent| agent.id == "builder").expect("builder");
    assert_eq!(builder.context_tokens, 1_200);
}

#[tokio::test]
async fn a_message_to_an_agent_without_a_usable_model_says_why_it_was_not_sent() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    client.ok(create_project("Site", &dir.join("site"))).await;
    let mut view = View::default();
    for event in client.ok(create_agent("site", "Writer", "")).await {
        view.apply(&event.body.expect("body"));
    }
    let update = client_message::Body::UpdateAgent(v1::UpdateAgent {
        agent_id: "writer".into(),
        model: Some("anthropic/not-signed-in".into()),
        ..Default::default()
    });
    for event in client.ok(update).await {
        view.apply(&event.body.expect("body"));
    }
    for event in client.ok(send_message("writer", "draft it")).await {
        view.apply(&event.body.expect("body"));
    }
    view.follow(&mut client, |view| view.entries.len() == 2 && idle(view, "writer")).await;
    let entries = describe(&view.entries);
    assert_eq!(entries[0], "user: draft it");
    assert!(entries[1].starts_with("notice: No model available: \"anthropic/not-signed-in\""), "{entries:?}");
}

#[tokio::test]
async fn compaction_shows_a_divider_continues_from_the_summary_and_reloads_identically() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    client.ok(create_project("Site", &dir.join("site"))).await;
    let mut view = View::default();
    for event in client.ok(create_agent("site", "Builder", "")).await {
        view.apply(&event.body.expect("body"));
    }
    // The scripted model reports 1200 tokens of context per reply.
    let config_path = dir.join("home/config.json");
    let mut config = Config::parse(&std::fs::read_to_string(&config_path).expect("read"), &dir.join("home")).expect("parse");
    config.compact_at_tokens = 1_000;
    std::fs::write(&config_path, config.to_json(&dir.join("home")).expect("json")).expect("write");
    view.follow(&mut client, |view| view.agent("builder").compact_at_tokens == 1_000).await;

    for event in client.ok(send_message("builder", "first")).await {
        view.apply(&event.body.expect("body"));
    }
    view.follow(&mut client, |view| view.entries.len() == 2 && idle(view, "builder")).await;

    // Over the limit: the next prompt compacts first, then is answered from the summary.
    for event in client.ok(send_message("builder", "second")).await {
        view.apply(&event.body.expect("body"));
    }
    view.follow(&mut client, |view| view.entries.len() == 5 && idle(view, "builder")).await;
    let entries = describe(&view.entries);
    assert_eq!(entries[..3], ["user: first", "assistant: echo: first", "user: second"]);
    assert!(entries[3].starts_with("compaction: "), "{entries:?}");
    // The prompt survived compaction verbatim, so the model answered it.
    assert_eq!(entries[4], "assistant: echo: second");

    let (restarted, _factory) = start(&dir).await;
    let (_client, _, transcript) = Client::connect(restarted).await;
    assert_eq!(transcript.expect("transcript").entries, view.entries);
    assert!(dir.join("home/agents/builder/segment-0002.json").is_file());
}

#[tokio::test]
async fn a_new_factory_opens_on_rivet_who_knows_how_factory_works() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let system_prompt = models.system_prompt.clone();
    let (addr, _factory) = start_with(&dir, models).await;
    let (mut client, workspace, transcript) = Client::connect(addr).await;
    assert!(workspace.projects.is_empty());
    let mut agents = Vec::new();
    for agent in &workspace.agents {
        agents.push((agent.id.as_str(), agent.name.as_str(), agent.project_id.as_str()));
    }
    assert_eq!(agents, [("rivet", "Rivet", "")], "Rivet lives in the workspace");
    assert_eq!(workspace.selected_agent_id, "rivet");
    assert!(transcript.expect("Rivet's conversation").entries.is_empty());

    let mut view = View::default();
    let use_scripted = client_message::Body::UpdateAgent(v1::UpdateAgent {
        agent_id: "rivet".into(),
        model: Some(MODEL.into()),
        effort: Some("off".into()),
        ..Default::default()
    });
    apply_request(&mut client, &mut view, use_scripted).await;
    apply_request(&mut client, &mut view, send_message("rivet", "How do I add a project?")).await;
    view.follow(&mut client, |view| view.entries.len() == 2 && idle(view, "rivet")).await;

    // Rivet's prompt and the manual, with this Factory's real paths.
    let prompt = system_prompt.lock().expect("lock").clone();
    assert!(prompt.contains("You are Rivet. You help the person achieve what they want with Claktory"), "{prompt}");
    assert!(prompt.contains("# Claktory manual"));
    let workspace_dir = dir.join("home/workspace");
    assert!(prompt.contains(&format!("`{}/web-site`", workspace_dir.display())), "paths are filled in");
    assert!(!prompt.contains("{{workspace}}") && !prompt.contains("{{config}}") && !prompt.contains("{{home}}"));

    // Its message box says what it's for; that's display only, like a color.
    let examples: Vec<&str> = workspace.agents[0].placeholder.lines().collect();
    assert_eq!(examples.len(), 14, "{examples:?}");
    assert_eq!(examples[0], "Set up a team that works on my codebase while I sleep");
    assert_eq!(examples[13], "Set up a team that turns my podcast episodes into blog posts and clips");
    let placeholder = |text: &str| {
        client_message::Body::UpdateAgent(v1::UpdateAgent {
            agent_id: "rivet".into(),
            placeholder: Some(text.into()),
            ..Default::default()
        })
    };
    apply_request(&mut client, &mut view, placeholder(" Ask me anything ")).await;
    assert_eq!(view.agent("rivet").placeholder, "Ask me anything");
    apply_request(&mut client, &mut view, send_message("rivet", "And now?")).await;
    view.follow(&mut client, |view| view.entries.len() == 4 && idle(view, "rivet")).await;
    assert_eq!(*system_prompt.lock().expect("lock"), prompt, "the prompt doesn't change");
    apply_request(&mut client, &mut view, placeholder("")).await;
    assert_eq!(view.agent("rivet").placeholder, "");
    let written = std::fs::read_to_string(dir.join("home/config.json")).expect("config");
    assert!(!written.contains("placeholder"), "{written}");

    // Rivet is an ordinary agent: once deleted, it stays deleted.
    let delete = client_message::Body::DeleteAgent(v1::DeleteAgent { agent_id: "rivet".into() });
    apply_request(&mut client, &mut view, delete).await;
    let (restarted, _factory) = start(&dir).await;
    let (_client, workspace, _) = Client::connect(restarted).await;
    assert!(workspace.agents.is_empty());
}

#[tokio::test]
async fn agents_working_when_factory_stops_carry_on_when_it_starts_again() {
    let dir = temp_dir();
    let (addr, _old) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let mut view = View::default();
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Idle", "")).await;
    apply_request(&mut client, &mut view, send_message("builder", "hang")).await;
    view.follow(&mut client, |view| view.agent("builder").status() == AgentStatus::Working && view.agent("builder").queued_messages == 0)
        .await;
    // Sent while it works: waits in the queue.
    apply_request(&mut client, &mut view, send_message("builder", "and then the tests")).await;
    assert_eq!(view.agent("builder").queued_messages, 1);
    // Factory stops once Builder has "hang" in its saved conversation; until
    // then, the message counts as waiting and would be given to it again.
    delivered(&dir, "hang").await;

    // Factory stops mid-run (an update, say) and starts again on the same home.
    let (addr, _new) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let mut view = View::default();
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    view.follow(&mut client, |view| view.entries.len() == 4 && idle(view, "builder")).await;
    let entries = describe(&view.entries);
    assert_eq!(entries[0], "user: hang");
    assert_eq!(
        entries[1..],
        ["event from Claktory: Claktory restarted", "user: and then the tests", "assistant: echo: and then the tests"]
    );
    let Some(v1::entry::Kind::Event(note)) = &view.entries[1].kind else { panic!("the note is an event") };
    assert_eq!(note.source(), v1::EventSource::Factory);
    assert!(note.text.starts_with("Claktory restarted while you were working. Whatever you were doing"), "{}", note.text);
    assert_eq!(view.agent("idle").status(), AgentStatus::Idle, "only agents that were working carry on");
}

#[tokio::test]
async fn an_agent_working_when_the_computer_restarted_is_told_so() {
    let dir = temp_dir();
    let (addr, _old) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let mut view = View::default();
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Idle", "")).await;
    apply_request(&mut client, &mut view, send_message("builder", "hang")).await;
    delivered(&dir, "hang").await;
    // Its run began before the computer last started.
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("db");
    db.execute("UPDATE runs SET started = 1000 WHERE ended IS NULL", []).expect("update");

    let (addr, _new) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let mut view = View::default();
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    view.follow(&mut client, |view| view.entries.len() >= 2 && idle(view, "builder")).await;
    let Some(v1::entry::Kind::Event(note)) = &view.entries[1].kind else { panic!("the note is an event: {:?}", view.entries) };
    assert_eq!(note.summary, "The computer restarted");
    assert!(note.text.starts_with("The computer restarted while you were working."), "{}", note.text);
}

#[tokio::test]
async fn a_factory_from_before_factory_db_keeps_its_working_agents_and_what_it_showed() {
    let dir = temp_dir();
    let (addr, _old) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let mut view = View::default();
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    // What an older Factory left behind: no factory.db, but state.json, and
    // runs.json from stopping while Builder worked.
    for name in ["factory.db", "factory.db-wal", "factory.db-shm"] {
        let _ = std::fs::remove_file(dir.join("home").join(name));
    }
    let state = r#"{"selected_agent": "builder", "focused_project": "site", "explorer": {"site": {"show_all": true}}}"#;
    std::fs::write(dir.join("home/state.json"), state).expect("state.json");
    let runs = r#"{"version": "2026.11.18-1755", "agents": {"builder": ["and then the tests"]}}"#;
    std::fs::write(dir.join("home/runs.json"), runs).expect("runs.json");

    let (addr, _new) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    assert_eq!((workspace.selected_agent_id.as_str(), workspace.focused_project_id.as_str()), ("builder", "site"));
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    view.follow(&mut client, |view| view.entries.len() == 3 && idle(view, "builder")).await;
    assert_eq!(
        describe(&view.entries),
        [
            "event from Claktory: Claktory restarted: updated from 2026.11.18-1755 to dev",
            "user: and then the tests",
            "assistant: echo: and then the tests"
        ]
    );
    assert!(view.explorer.as_ref().is_some_and(|explorer| explorer.show_all), "the explorer still shows everything");
    assert!(!dir.join("home/runs.json").exists() && !dir.join("home/state.json").exists(), "moved into factory.db");
}

fn unseen(view: &View, id: &str) -> bool {
    view.agent(id).unseen
}

fn select(id: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: id.into() })
}

#[tokio::test]
async fn an_agent_that_finished_shows_it_until_the_person_opens_it_even_after_a_restart() {
    let dir = temp_dir();
    let (addr, _old) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let mut view = View::default();
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    // Creating an agent opens it, so the person is now looking at Helper.
    apply_request(&mut client, &mut view, create_agent("site", "Helper", "")).await;
    assert!(!unseen(&view, "builder") && !unseen(&view, "helper"));

    // Builder finishes while the person looks at Helper.
    apply_request(&mut client, &mut view, send_message("builder", "build it")).await;
    view.follow(&mut client, |view| idle(view, "builder") && unseen(view, "builder")).await;
    // Helper finishes while the person looks at it.
    apply_request(&mut client, &mut view, send_message("helper", "help")).await;
    view.follow(&mut client, |view| view.entries.len() == 2 && idle(view, "helper")).await;
    assert!(!unseen(&view, "helper"), "the person saw it happen");

    // Opening Builder ticks it off.
    apply_request(&mut client, &mut view, select("builder")).await;
    assert!(!unseen(&view, "builder"));
    apply_request(&mut client, &mut view, select("helper")).await;

    // A run the person stopped leaves nothing new.
    apply_request(&mut client, &mut view, send_message("builder", "hang")).await;
    view.follow(&mut client, |view| view.agent("builder").status() == AgentStatus::Working).await;
    apply_request(&mut client, &mut view, client_message::Body::StopAgent(v1::StopAgent { agent_id: "builder".into() })).await;
    view.follow(&mut client, |view| idle(view, "builder")).await;
    assert!(!unseen(&view, "builder"), "stopped by the person");

    // Unseen survives a restart, on every device.
    apply_request(&mut client, &mut view, send_message("builder", "build more")).await;
    view.follow(&mut client, |view| idle(view, "builder") && unseen(view, "builder")).await;
    let (addr, _new) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert!(unseen(&view, "builder") && !unseen(&view, "helper"));
    apply_request(&mut client, &mut view, select("builder")).await;
    assert!(!unseen(&view, "builder"));
    let (addr, _newer) = start(&dir).await;
    let (_client, workspace, _) = Client::connect(addr).await;
    assert!(!workspace.agents.iter().any(|agent| agent.unseen), "opened before the restart");
}

#[tokio::test]
async fn queued_messages_show_until_the_agent_takes_them_even_across_a_stop() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_agent("", "Ops", "")).await;
    apply_request(&mut client, &mut view, create_agent("", "Lead", "")).await;
    apply_request(&mut client, &mut view, select("ops")).await;
    let waiting = |view: &View| -> Vec<(String, String, String)> {
        let queued = view.queued.as_ref().expect("the queue");
        assert_eq!(queued.agent_id, "ops");
        queued.messages.iter().map(|message| (message.source.clone(), message.sender_id.clone(), message.text.clone())).collect()
    };
    assert!(waiting(&view).is_empty());

    // Taken at once by an idle agent, a message never waits.
    apply_request(&mut client, &mut view, send_message("ops", "hang")).await;
    view.follow(&mut client, |view| view.agent("ops").status() == AgentStatus::Working && waiting(view).is_empty()).await;
    apply_request(&mut client, &mut view, send_message("ops", "first")).await;
    apply_request(&mut client, &mut view, send_message("ops", "second")).await;
    assert_eq!(waiting(&view), [("person".into(), String::new(), "first".into()), ("person".into(), String::new(), "second".into())]);

    // Events wait too, from whom they came.
    apply_request(&mut client, &mut view, select("lead")).await;
    assert_eq!(view.queued.as_ref().map(|queued| queued.agent_id.as_str()), Some("lead"));
    apply_request(&mut client, &mut view, send_message("lead", r#"call agents_send {"to":"ops","text":"status?"}"#)).await;
    view.follow(&mut client, |view| idle(view, "lead") && view.agent("ops").queued_messages == 3).await;
    apply_request(&mut client, &mut view, select("ops")).await;
    let event = view.queued.as_ref().expect("the queue").messages[2].clone();
    assert_eq!(
        (event.source.as_str(), event.sender.as_str(), event.sender_id.as_str(), event.text.as_str()),
        ("agent", "Lead", "lead", "status?")
    );
    assert!(event.created > 0 && event.id > 0);

    // A client connecting now sees it all.
    let (mut other, workspace, _) = Client::connect(addr).await;
    let mut seen = View { workspace, ..View::default() };
    seen.follow(&mut other, |view| view.queued.is_some()).await;
    assert_eq!(seen.queued.as_ref().map(|queued| queued.messages.len()), Some(3));

    // Stopping the run keeps them: they reach the agent after the stop, as
    // a new run, in the order they came.
    apply_request(&mut client, &mut view, client_message::Body::StopAgent(v1::StopAgent { agent_id: "ops".into() })).await;
    view.follow(&mut client, |view| waiting(view).is_empty() && idle(view, "ops")).await;
    let users: Vec<String> = describe(&view.entries).into_iter().filter(|entry| entry.starts_with("user: ")).collect();
    assert_eq!(users, ["user: hang", "user: first", "user: second"]);
    assert!(describe(&view.entries).iter().any(|entry| entry.starts_with("event from Lead")), "{:?}", describe(&view.entries));
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("db");
    let read = |sql: &str| -> Vec<String> {
        let mut statement = db.prepare(sql).expect("sql");
        statement.query_map([], |row| row.get::<_, String>(0)).expect("query").map(|row| row.expect("row")).collect()
    };
    assert_eq!(
        read(
            "SELECT text || ' ' || (SELECT count(*) FROM runs WHERE agent = 'ops' AND id <= run) || ' ' || (delivered IS NOT NULL) || ' ' || ifnull(dropped, '-') FROM messages WHERE agent = 'ops' ORDER BY id"
        ),
        ["hang 1 1 -", "first 2 1 -", "second 2 1 -", "status? 2 1 -"],
        "the first run's, then the second's; all delivered, none dropped"
    );
    assert_eq!(read("SELECT outcome FROM runs WHERE agent = 'ops' ORDER BY id"), ["stopped", "finished"]);
}

#[tokio::test]
async fn entries_say_which_model_response_they_came_from_live_and_after_a_restart() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_agent("", "Ops", "")).await;
    apply_request(&mut client, &mut view, select("ops")).await;
    apply_request(&mut client, &mut view, send_message("ops", r#"call agents_status {}"#)).await;
    view.follow(&mut client, |view| view.entries.len() >= 3 && idle(view, "ops")).await;
    let replies: Vec<(String, u64)> = describe(&view.entries).into_iter().zip(view.entries.iter().map(|entry| entry.reply)).collect();
    assert_eq!(replies.len(), 3, "{replies:?}");
    // The person's message, the response with the call, and the response after it.
    assert_eq!((replies[0].1, replies[1].1, replies[2].1), (0, 1, 2), "{replies:?}");
    assert!(replies[1].0.starts_with("tool agents_status") && replies[2].0.starts_with("assistant: done: "), "{replies:?}");

    let (addr, _new) = start(&dir).await;
    let (_client, _, transcript) = Client::connect(addr).await;
    assert_eq!(transcript.expect("transcript").entries, view.entries, "a reload shows the same");
}

#[tokio::test]
async fn a_tool_call_shows_its_arguments_while_the_model_writes_them() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    let raw = r#"{"command":"echo written slowly"}"#;
    apply_request(&mut client, &mut view, send_message("builder", &format!("slowcall bash {raw}"))).await;
    let input = |view: &View| {
        view.entries.iter().find_map(|entry| match &entry.kind {
            Some(v1::entry::Kind::ToolCall(call)) => Some(call.input.clone()),
            _ => None,
        })
    };
    // Half written: the raw JSON so far.
    view.follow(&mut client, |view| input(view).is_some_and(|input| !input.is_empty())).await;
    assert_eq!(input(&view).as_deref(), Some(&raw[..raw.len() / 2]));
    // Written: the whole, pretty.
    view.follow(&mut client, |view| idle(view, "builder")).await;
    assert_eq!(input(&view).as_deref(), Some("{\n  \"command\": \"echo written slowly\"\n}"));
}

#[tokio::test]
async fn a_tool_call_says_when_it_ran_and_its_timeout_even_after_a_restart() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    let calls = |entries: &[v1::Entry]| -> Vec<v1::ToolCall> {
        entries
            .iter()
            .filter_map(|entry| match &entry.kind {
                Some(v1::entry::Kind::ToolCall(call)) => Some(call.clone()),
                _ => None,
            })
            .collect()
    };
    let before = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("time").as_millis() as i64;
    apply_request(&mut client, &mut view, send_message("builder", r#"call bash {"command":"sleep 0.3"}"#)).await;
    // While it runs: when it started and when it will be stopped.
    view.follow(&mut client, |view| calls(&view.entries).first().is_some_and(|call| call.state() == v1::ToolCallState::Running)).await;
    let running = &calls(&view.entries)[0];
    assert!(running.started >= before && running.ended == 0, "{running:?}");
    assert_eq!(running.timeout_ms, 120_000, "the default timeout");
    view.follow(&mut client, |view| idle(view, "builder")).await;
    let done = &calls(&view.entries)[0];
    assert!(done.ended - done.started >= 300, "{done:?}");

    let commands = [r#"{"command":"true","timeout":5}"#, r#"{"command":"true","run_in_background":true}"#];
    for command in commands {
        apply_request(&mut client, &mut view, send_message("builder", &format!("call bash {command}"))).await;
        view.follow(&mut client, |view| idle(view, "builder")).await;
    }
    apply_request(&mut client, &mut view, send_message("builder", r#"call tasks_list {}"#)).await;
    view.follow(&mut client, |view| calls(&view.entries).len() == 4 && idle(view, "builder")).await;
    let timeouts: Vec<u64> = calls(&view.entries).iter().map(|call| call.timeout_ms).collect();
    assert_eq!(timeouts, [120_000, 5_000, 0, 0], "bash's own, none in the background, none for other tools");

    // A new backend shows the same.
    let (restarted, _factory) = start(&dir).await;
    let (_client, _, transcript) = Client::connect(restarted).await;
    assert_eq!(calls(&transcript.expect("transcript").entries), calls(&view.entries));
}

#[tokio::test]
async fn the_persons_messages_say_which_device_they_came_from_and_their_apps_id_even_waiting_across_a_restart() {
    let dir = temp_dir();
    let (addr, _old) = start(&dir).await;
    let (mut client, _, _) = Client::connect_as(addr, v1::DeviceKind::Phone).await;
    let mut view = View::default();
    apply_request(&mut client, &mut view, create_agent("", "Builder", "")).await;
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    apply_request(&mut client, &mut view, send_message("builder", "hang")).await;
    view.follow(&mut client, |view| view.agent("builder").status() == AgentStatus::Working && view.agent("builder").queued_messages == 0)
        .await;
    // Waiting, it says where it came from: this computer's name, as a phone.
    // With the id the app made up for it, so it can match what it showed.
    let send = v1::SendMessage { agent_id: "builder".into(), text: "on my way".into(), client_id: "c-1".into(), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::SendMessage(send)).await;
    assert_eq!(view.queued.as_ref().expect("the queue").messages[0].client_id, "c-1");
    let waiting = view.queued.as_ref().expect("the queue").messages[0].sent_from.clone().expect("where it came from");
    assert!(!waiting.device_name.is_empty());
    assert_eq!(waiting.kind(), v1::DeviceKind::Phone);
    delivered(&dir, "hang").await;

    // It keeps that across a restart, and the model reads it with it.
    let models = Scripted::signed_in();
    let requests = models.requests.clone();
    let (addr, _new) = start_with(&dir, models).await;
    let last = || requests.lock().expect("lock").last().expect("a request").prompt.clone();
    // Builder is still selected, so its conversation comes with the connection.
    let (mut client, workspace, transcript) = Client::connect(addr).await;
    let mut view = View { workspace, entries: transcript.map(|transcript| transcript.entries).unwrap_or_default(), ..View::default() };
    view.follow(&mut client, |view| view.entries.len() == 4 && idle(view, "builder")).await;
    let name = &waiting.device_name;
    let Some(v1::entry::Kind::User(user)) = &view.entries[2].kind else { panic!("the person's message") };
    assert_eq!((user.text.as_str(), user.sent_from.as_ref()), ("on my way", Some(&waiting)));
    assert_eq!(user.client_id, "c-1");
    assert_eq!(last(), format!("<sent from=\"{name}\" device=\"phone\">\non my way\n</sent>"));
    // An app that doesn't say what it runs on: the name alone.
    apply_request(&mut client, &mut view, send_message("builder", "at the desk")).await;
    view.follow(&mut client, |view| view.entries.len() == 6 && idle(view, "builder")).await;
    assert_eq!(last(), format!("<sent from=\"{name}\">\nat the desk\n</sent>"));
    // A client connecting now gets it with the conversation; one sent
    // without an id has none.
    let (_other, _, transcript) = Client::connect(addr).await;
    let users: Vec<(String, String)> = transcript
        .expect("the conversation")
        .entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(v1::entry::Kind::User(user)) => Some((user.text.clone(), user.client_id.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(users[1..], [("on my way".into(), "c-1".into()), ("at the desk".into(), String::new())]);
    // Events don't: they say where they're from their own way.
    let Some(v1::entry::Kind::Event(note)) = &view.entries[1].kind else { panic!("the restart note") };
    assert!(note.text.starts_with("Claktory restarted"), "{}", note.text);
}

/// The Rivet prompt the version before placeholders shipped: today's with
/// "the person" where it says {{user}}, and without what came after (its
/// line on connected services, and the approved-request sentences in its
/// secrets line).
fn last_shipped_rivet_prompt(current: &str) -> String {
    let before: Vec<&str> = current
        .lines()
        .filter(|line| !line.starts_with("- A connected service") && !line.starts_with("- Ask {{user}} one thing per card"))
        .collect();
    let approved = " When {{user}} approves an agent's request for a secret, you're told what it needs and why: give it one of the vault's secrets that fits (secrets_grant with request, and aliases when the variable names differ), or, when none fits, ask {{user}} to add one with secrets_collect, which opens a filled-in card in your conversation. Hold these to the same bar: check the request's reason against the agent's work, and give the narrower thing when in doubt.";
    let import = " When a secret is a file already on this computer (a keystore, a credentials file), never ask {{user}} to copy its contents into the vault by hand; have the agent that needs it ask to import it (secrets_request with import), which {{user}} approves on a card that shows exactly which file.";
    before.join("\n").replace(import, "").replace(approved, "").replace("{{user}}", "the person").replace("{{User}}", "The person")
}

#[tokio::test]
async fn a_rivet_prompt_nobody_edited_follows_the_default() {
    let dir = temp_dir();
    let current = Config::default().agents[0].prompt.clone();
    assert!(current.contains("You help {{user}} achieve"), "the default has placeholders");
    let old = last_shipped_rivet_prompt(&current);
    let mut config = Config::default();
    config.user.name = "Matt".into();
    config.agents[0].prompt = format!("{old}\n\n  ");
    config.agents[0].model = MODEL.into();
    config.agents[0].effort = "off".into();
    std::fs::create_dir_all(dir.join("home")).expect("home");
    std::fs::write(dir.join("home/config.json"), config.to_json(&dir.join("home")).expect("json")).expect("config");

    let models = Scripted::signed_in();
    let system_prompt = models.system_prompt.clone();
    let (addr, _factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let rivet = &workspace.agents[0];
    assert_eq!(rivet.prompt, current, "an old default became this version's");
    assert_eq!(rivet.default_prompt, current);
    let written = Config::parse(&std::fs::read_to_string(dir.join("home/config.json")).expect("config"), &dir.join("home")).expect("valid");
    assert_eq!(written.agents[0].prompt, current, "saved with its placeholders");

    // The placeholders are filled in for the person.
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    apply_request(&mut client, &mut view, send_message("rivet", "Hi")).await;
    view.follow(&mut client, |view| view.entries.len() == 2 && idle(view, "rivet")).await;
    let prompt = system_prompt.lock().expect("lock").clone();
    assert!(prompt.contains("## Your instructions\n\nYou are Rivet. You help Matt achieve what they want"), "{prompt}");
    assert!(!prompt.contains("{{user}}") && !prompt.contains("{{User}}"));

    // Edited, it's the person's: a new version leaves it alone.
    let edited = format!("{old}\n- Be brief.");
    let edit = v1::UpdateAgent { agent_id: "rivet".into(), prompt: Some(edited.clone()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(edit)).await;
    assert_eq!(view.agent("rivet").prompt, edited);
    drop(client);
    let (addr, _again) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    assert_eq!(workspace.agents[0].prompt, edited, "kept across a restart");

    // Reset to the default: the app sets it to default_prompt.
    let mut view = View { workspace, ..View::default() };
    let reset =
        v1::UpdateAgent { agent_id: "rivet".into(), prompt: Some(view.agent("rivet").default_prompt.clone()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(reset)).await;
    assert_eq!(view.agent("rivet").prompt, current);

    // Pasting an old default into the file follows too.
    let mut config =
        Config::parse(&std::fs::read_to_string(dir.join("home/config.json")).expect("config"), &dir.join("home")).expect("valid");
    config.agents[0].prompt = old;
    std::fs::write(dir.join("home/config.json"), config.to_json(&dir.join("home")).expect("json")).expect("config");
    let saved = || {
        Config::parse(&std::fs::read_to_string(dir.join("home/config.json")).expect("config"), &dir.join("home"))
            .ok()
            .map(|config| config.agents[0].prompt.clone())
    };
    for _ in 0..100 {
        if saved().as_deref() == Some(current.as_str()) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(saved().as_deref(), Some(current.as_str()), "read, followed and saved");
    assert_eq!(view.agent("rivet").prompt, current);
}

#[tokio::test]
async fn placeholders_work_in_any_agents_prompt() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let system_prompt = models.system_prompt.clone();
    let (addr, _factory) = start_with(&dir, models).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let mut view = View::default();
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(
        &mut client,
        &mut view,
        create_agent("site", "Writer", "{{User}} reads what you write. Report to {{user}}; notes go in {{workspace}}/notes."),
    )
    .await;
    assert_eq!(view.agent("writer").default_prompt, "", "only Rivet has a default");
    apply_request(&mut client, &mut view, send_message("writer", "Hi")).await;
    view.follow(&mut client, |view| view.entries.len() == 2 && idle(view, "writer")).await;
    let prompt = system_prompt.lock().expect("lock").clone();
    let notes = format!("{}/notes", dir.join("home/workspace").display());
    assert!(prompt.contains(&format!("The person reads what you write. Report to the person; notes go in {notes}.")), "{prompt}");
    assert_eq!(
        view.agent("writer").prompt,
        "{{User}} reads what you write. Report to {{user}}; notes go in {{workspace}}/notes.",
        "kept as written"
    );
}
