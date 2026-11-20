//! Slash commands the person runs for an agent: /clear starts a new
//! conversation, /compact summarizes the current one now.

use crate::common::{Client, View, apply_request, create_agent, describe, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, server_message::Body};

fn command(agent: &str, name: &str, args: &str) -> client_message::Body {
    client_message::Body::RunCommand(v1::RunCommand { agent_id: agent.into(), name: name.into(), args: args.into() })
}

fn refused(reply: Body) -> String {
    match reply {
        Body::Error(error) => {
            assert_eq!(error.code(), v1::ErrorCode::InvalidRequest, "{}", error.message);
            error.message
        }
        other => panic!("expected an error, got {other:?}"),
    }
}

fn idle(view: &View) -> bool {
    let agent = view.agent("ops");
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

#[tokio::test]
async fn clear_starts_a_new_conversation_and_compact_summarizes_now() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_agent("", "Ops", "")).await;
    let names: Vec<&str> = view.agent("ops").commands.iter().map(|command| command.name.as_str()).collect();
    assert_eq!(names, ["clear", "compact"]);
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "ops".into() })).await;

    assert_eq!(refused(apply_request(&mut client, &mut view, command("ops", "compact", "")).await), "Nothing to compact yet.");
    assert_eq!(
        refused(apply_request(&mut client, &mut view, command("ops", "nope", "")).await),
        "There's no command /nope. Commands: /clear, /compact."
    );
    assert_eq!(refused(apply_request(&mut client, &mut view, command("ops", "clear", "all")).await), "/clear takes nothing after it.");

    apply_request(&mut client, &mut view, send_message("ops", "hello")).await;
    view.follow(&mut client, |view| view.entries.len() == 2 && idle(view)).await;
    assert!(matches!(apply_request(&mut client, &mut view, command("ops", "compact", "")).await, Body::Ack(_)));
    view.follow(&mut client, |view| view.entries.len() == 3 && idle(view)).await;
    assert_eq!(describe(&view.entries), ["user: hello", "assistant: echo: hello", "compaction: 2 earlier messages."]);

    // Working, it can't be compacted; /clear stops it and starts afresh.
    apply_request(&mut client, &mut view, send_message("ops", "hang")).await;
    view.follow(&mut client, |view| view.agent("ops").status() == AgentStatus::Working).await;
    assert_eq!(
        refused(apply_request(&mut client, &mut view, command("ops", "compact", "")).await),
        "Ops is working; stop it or wait, then /compact."
    );
    // What's queued goes with the old conversation (Stop keeps it; /clear doesn't).
    apply_request(&mut client, &mut view, send_message("ops", "queued before the clear")).await;
    assert_eq!(view.agent("ops").queued_messages, 1);
    assert!(matches!(apply_request(&mut client, &mut view, command("ops", "clear", "")).await, Body::Ack(_)));
    view.follow(&mut client, |view| describe(&view.entries) == ["notice: Conversation cleared"] && idle(view)).await;
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("db");
    let dropped: i64 =
        db.query_row("SELECT dropped FROM messages WHERE text = 'queued before the clear'", [], |row| row.get(0)).expect("dropped");
    assert!(dropped > 0, "dropped when cleared");
    assert_eq!(view.agent("ops").context_tokens, 0);

    // The old conversation is archived in the agent's folder.
    let agent_dir = dir.join("home/agents/ops");
    let archived: Vec<_> = std::fs::read_dir(agent_dir.join("archive")).expect("archive").collect();
    assert_eq!(archived.len(), 1);
    let old = archived[0].as_ref().expect("entry").path();
    assert!(old.join("segment-0001.json").exists() && old.join("segment-0002.json").exists());

    // The new one starts empty: the model sees only what comes after.
    apply_request(&mut client, &mut view, send_message("ops", "again")).await;
    view.follow(&mut client, |view| view.entries.len() == 3 && idle(view)).await;
    assert_eq!(describe(&view.entries), ["notice: Conversation cleared", "user: again", "assistant: echo: again"]);
    let segment: serde_json::Value =
        serde_json::from_slice(&std::fs::read(agent_dir.join("segment-0001.json")).expect("segment")).expect("json");
    assert_eq!(segment["messages"].as_array().map(Vec::len), Some(2));

    // And so it reads after a restart.
    let (addr, _new) = start(&dir).await;
    let (_client, _, transcript) = Client::connect(addr).await;
    let entries = transcript.expect("the selected agent's transcript").entries;
    assert_eq!(describe(&entries), ["notice: Conversation cleared", "user: again", "assistant: echo: again"]);
}
