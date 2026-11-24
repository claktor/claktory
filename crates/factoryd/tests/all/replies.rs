//! Replies quote the person's message (`Entry.replied_to`): the first
//! assistant text after a person's message carries that message's entry seq,
//! while it streams too, so the app can show what it answers. One text per
//! message at most, never tool calls or thinking; replies to events quote
//! nothing; with several messages queued, the reply quotes the latest; and a
//! reload shows what was shown live.

use crate::common::{Client, View, apply_request, create_agent, create_project, describe, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind};

fn select(agent: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

fn idle(view: &View, agent: &str) -> bool {
    view.workspace.agents.iter().any(|candidate| candidate.id == agent && candidate.status() == AgentStatus::Idle)
}

/// The `replied_to` of each entry, with a letter for its kind.
fn replies(entries: &[v1::Entry]) -> Vec<(char, Option<u64>)> {
    entries
        .iter()
        .map(|entry| {
            let kind = match &entry.kind {
                Some(Kind::User(_)) => 'u',
                Some(Kind::Event(_)) => 'e',
                Some(Kind::Assistant(_)) => 'a',
                Some(Kind::ToolCall(_)) => 't',
                Some(Kind::Notice(_)) => 'n',
                _ => '?',
            };
            (kind, entry.replied_to)
        })
        .collect()
}

/// The first assistant text after the person's message quotes it, not a tool
/// call on the way there, and a restart shows the same.
#[tokio::test]
async fn the_first_assistant_text_quotes_the_persons_message() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let mut view = View::default();
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, select("builder")).await;

    apply_request(&mut client, &mut view, send_message("builder", "hi")).await;
    view.follow(&mut client, |view| view.entries.len() == 2 && idle(view, "builder")).await;
    assert_eq!(replies(&view.entries), [('u', None), ('a', Some(0))]);

    // A tool call on the way carries nothing; the text after it quotes.
    let ask = r#"call bash {"command":"true"}"#;
    apply_request(&mut client, &mut view, send_message("builder", ask)).await;
    view.follow(&mut client, |view| view.entries.len() == 5 && idle(view, "builder")).await;
    assert_eq!(replies(&view.entries)[2..], [('u', None), ('t', None), ('a', Some(2))]);

    // Reloaded from its files, the conversation quotes the same.
    drop(client);
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let mut view = View::default();
    apply_request(&mut client, &mut view, select("builder")).await;
    view.follow(&mut client, |view| view.entries.len() == 5).await;
    assert_eq!(replies(&view.entries), [('u', None), ('a', Some(0)), ('u', None), ('t', None), ('a', Some(2))]);
}

/// Messages answered together are quoted once, the latest of them: the ones
/// before it went unanswered when the reply was written.
#[tokio::test]
async fn a_reply_to_several_queued_messages_quotes_the_latest() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let mut view = View::default();
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, select("builder")).await;

    // The agent hangs on the first message; three more queue behind it.
    apply_request(&mut client, &mut view, send_message("builder", "hang")).await;
    view.follow(&mut client, |view| view.entries.len() == 1).await;
    for text in ["one", "two", "three"] {
        apply_request(&mut client, &mut view, send_message("builder", text)).await;
    }

    // Stopped, it takes all three in the next run; the answer quotes the
    // latest.
    let stop = client_message::Body::StopAgent(v1::StopAgent { agent_id: "builder".into() });
    apply_request(&mut client, &mut view, stop).await;
    view.follow(&mut client, |view| {
        describe(&view.entries).iter().any(|entry| entry.starts_with("assistant: echo")) && idle(view, "builder")
    })
    .await;
    let shown = describe(&view.entries);
    let three = view.entries.iter().find(|entry| matches!(&entry.kind, Some(Kind::User(user)) if user.text == "three")).expect("three");
    let answer = view.entries.iter().rfind(|entry| matches!(&entry.kind, Some(Kind::Assistant(_)))).expect("an answer");
    assert_eq!(answer.replied_to, Some(three.seq), "{shown:?}");
    for entry in &view.entries {
        if entry.seq != answer.seq {
            assert_eq!(entry.replied_to, None, "only the answer quotes: {shown:?}");
        }
    }
}

/// A reply to an event (another agent's message here) quotes nothing, and an
/// unanswered person message isn't quoted later by it either.
#[tokio::test]
async fn a_reply_to_an_event_quotes_nothing() {
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

    // The writer's reply to the event quotes nothing.
    apply_request(&mut client, &mut view, select("writer")).await;
    view.follow(&mut client, |view| view.entries.len() == 2 && idle(view, "writer")).await;
    assert_eq!(replies(&view.entries), [('e', None), ('a', None)]);

    // The person's own message afterwards is quoted as usual.
    apply_request(&mut client, &mut view, send_message("writer", "thanks")).await;
    view.follow(&mut client, |view| view.entries.len() == 4 && idle(view, "writer")).await;
    assert_eq!(replies(&view.entries)[2..], [('u', None), ('a', Some(2))]);
}
