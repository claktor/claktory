//! Going back in a conversation: the conversation goes back
//! to right before a message, and what came after is set aside as another
//! version, to switch back to or delete; the agent hears what that part did
//! that still stands, in a quiet note with whatever wakes it next.

use std::sync::Arc;

use crate::common::{Client, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind};
use factoryd::fork;

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

/// The texts of the person's messages and the agent's replies shown.
fn shown(view: &View) -> Vec<String> {
    view.entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::User(user)) => Some(user.text.clone()),
            Some(Kind::Assistant(reply)) => Some(reply.text.clone()),
            _ => None,
        })
        .collect()
}

/// The entry of the person's message `text`, or of the reply to it: its seq
/// and time.
fn entry(view: &View, text: &str) -> (u64, i64) {
    let found = view.entries.iter().find(|entry| match &entry.kind {
        Some(Kind::User(user)) => user.text == text,
        Some(Kind::Assistant(reply)) => reply.text == text,
        _ => false,
    });
    let found = found.unwrap_or_else(|| panic!("no entry {text:?} in {:?}", shown(view)));
    (found.seq, found.timestamp_ms)
}

/// Claktory's events shown, as their text.
fn notes(view: &View) -> Vec<(String, bool)> {
    view.entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::Event(event)) if event.source() == v1::EventSource::Factory => Some((event.text.clone(), event.quiet)),
            _ => None,
        })
        .collect()
}

async fn say(client: &mut Client, view: &mut View, text: &str) {
    let before = view.entries.len();
    apply_request(client, view, send_message("builder", text)).await;
    view.follow(client, |view| view.entries.len() > before + 1 && idle(view, "builder")).await;
}

/// A Claktory with Builder, selected, in a project at `dir/site`.
async fn setup(dir: &std::path::Path) -> (Arc<factoryd::Factory>, Client, View) {
    let (addr, factory) = start(dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    (factory, client, view)
}

fn messages_in(dir: &std::path::Path, number: u32) -> usize {
    let segment = factoryd::transcript::segment_for_test(dir, number).expect("segment");
    segment["messages"].as_array().expect("messages").len()
}

/// Segment `number` of the conversation in `dir`, as written.
fn segment_text(dir: &std::path::Path, number: u32) -> String {
    std::fs::read_to_string(factoryd::transcript::segment_file_for_test(dir, number).expect("segment")).expect("segment")
}

#[tokio::test]
async fn going_back_sets_the_rest_aside_and_the_agent_hears_what_still_stands() {
    let dir = temp_dir();
    let (factory, mut client, mut view) = setup(&dir).await;
    let folder = dir.join("home/agents/builder");
    say(&mut client, &mut view, "one").await;
    say(&mut client, &mut view, "two").await;
    say(&mut client, &mut view, r#"call write_file {"path":"notes.md","content":"kept\n"}"#).await;
    say(&mut client, &mut view, "three").await;

    // Refused, changing nothing: a time that isn't the entry's (the
    // conversation changed), the first message (nothing before it), a reply
    // with nothing after it yet.
    let (two, at) = entry(&view, "two");
    let refused = fork::fork_for_test(&factory, "builder", two, at + 1).await;
    assert_eq!(refused, Err("The conversation changed. Try again.".to_owned()));
    let (first, at_first) = entry(&view, "one");
    let refused = fork::fork_for_test(&factory, "builder", first, at_first).await.expect_err("the first message");
    assert!(refused.contains("where the conversation starts"), "{refused}");
    let (last, at_last) = entry(&view, "echo: three");
    let refused = fork::fork_for_test(&factory, "builder", last, at_last).await.expect_err("nothing after it");
    assert!(refused.starts_with("Nothing came after this yet"), "{refused}");
    assert!(fork::branches_for_test(&factory, "builder").is_empty());

    // Back to right before "two": only "one" and its reply are left, on
    // disk too, and the rest is another version.
    fork::fork_for_test(&factory, "builder", two, at).await.expect("forked");
    view.follow(&mut client, |view| !shown(view).contains(&"three".to_owned())).await;
    assert_eq!(shown(&view), ["one", "echo: one"]);
    assert_eq!(messages_in(&folder, 1), 2);
    let branches = fork::branches_for_test(&factory, "builder");
    assert_eq!(branches.len(), 1);
    let version = &branches[0];
    assert_eq!((version.label.as_str(), version.made_by.as_str(), version.fork.segment, version.fork.index), ("two", "", 1, 2));
    assert_eq!(version.fork.ms, at, "it parts at the message's time");
    // two, the write_file call, three, and the four replies.
    assert_eq!(version.messages, 7);
    assert!(factoryd::transcript::segment_file_for_test(&folder.join("branches").join(&version.id), 1).is_some());
    // What it did stands.
    assert_eq!(std::fs::read_to_string(dir.join("site/notes.md")).expect("its file"), "kept\n");

    // The note waits quietly, and comes with the person's next message, first.
    assert!(idle(&view, "builder"));
    assert!(notes(&view).is_empty(), "not shown in the conversation yet");
    say(&mut client, &mut view, "four").await;
    let given = notes(&view);
    assert_eq!(given.len(), 1, "{given:?}");
    let (note, quiet) = &given[0];
    assert!(quiet);
    assert!(note.starts_with("The person took your conversation back to "), "{note}");
    assert!(note.contains("is kept as another version the person can switch back to. You remember only what's above."), "{note}");
    assert!(note.contains("\n- Messages you sent: none. Pipe messages: none."), "{note}");
    assert!(note.contains("\n- Files you wrote or edited: notes.md. Commands run: none."), "{note}");
    assert_eq!(&shown(&view)[2..], ["four", "echo: four"]);

    // Switched back: the version comes back whole, and the conversation as
    // it was is set aside in its place.
    fork::switch_for_test(&factory, "builder", &version.id).await.expect("switched");
    view.follow(&mut client, |view| shown(view).contains(&"three".to_owned())).await;
    assert_eq!(shown(&view)[..4], ["one", "echo: one", "two", "echo: two"]);
    assert!(!shown(&view).contains(&"four".to_owned()));
    let branches = fork::branches_for_test(&factory, "builder");
    assert_eq!(branches.len(), 1);
    assert_eq!((branches[0].label.as_str(), branches[0].fork.index), ("four", 2));
    assert!(!folder.join("branches").join(&version.id).exists(), "the version is the conversation again");
    say(&mut client, &mut view, "five").await;
    let note = notes(&view).pop().expect("a note").0;
    assert!(note.starts_with("The person switched you to another version of your conversation"), "{note}");

    // Deleted, it's gone; a name that isn't a version's is refused.
    let other = branches[0].id.clone();
    fork::delete_for_test(&factory, "builder", &other).await.expect("deleted");
    assert!(fork::branches_for_test(&factory, "builder").is_empty());
    let refused = fork::delete_for_test(&factory, "builder", &other).await;
    assert_eq!(refused, Err("This version of the conversation is gone.".to_owned()));
    assert!(fork::delete_for_test(&factory, "builder", "../builder").await.is_err());
    assert!(fork::switch_for_test(&factory, "builder", "nope").await.is_err());

    // /clear takes the other versions with the conversation into its archive.
    let (five, at_five) = entry(&view, "five");
    fork::fork_for_test(&factory, "builder", five, at_five).await.expect("forked again");
    assert_eq!(fork::branches_for_test(&factory, "builder").len(), 1);
    let clear = v1::RunCommand { agent_id: "builder".into(), name: "clear".into(), args: String::new() };
    apply_request(&mut client, &mut view, client_message::Body::RunCommand(clear)).await;
    assert!(fork::branches_for_test(&factory, "builder").is_empty());
    let archived =
        std::fs::read_dir(folder.join("archive")).expect("archive").flatten().any(|entry| entry.path().join("branches").is_dir());
    assert!(archived, "the versions are archived");
}

/// The note waits across a restart, still quiet: it doesn't start the
/// agent, and comes with the person's next message.
#[tokio::test]
async fn the_note_waits_quietly_across_a_restart() {
    let dir = temp_dir();
    let (factory, mut client, mut view) = setup(&dir).await;
    say(&mut client, &mut view, "one").await;
    say(&mut client, &mut view, "two").await;
    let (reply, at) = entry(&view, "echo: one");
    // From the agent's reply: right before the next message.
    fork::fork_for_test(&factory, "builder", reply, at).await.expect("forked");
    view.follow(&mut client, |view| !shown(view).contains(&"two".to_owned())).await;
    assert_eq!(fork::branches_for_test(&factory, "builder")[0].label, "two");
    drop(client);
    drop(factory);

    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    assert!(idle(&view, "builder"), "the note doesn't start it");
    assert!(notes(&view).is_empty());
    say(&mut client, &mut view, "three").await;
    assert!(notes(&view).iter().any(|(note, quiet)| *quiet && note.contains("took your conversation back")), "{:?}", notes(&view));
    assert_eq!(shown(&view), ["one", "echo: one", "three", "echo: three"]);
}

fn branches(view: &View) -> u32 {
    view.agent("builder").branches
}

/// The person goes back over the protocol: Agent.branches counts the
/// versions, ListBranches lists them, and each refusal reads as it is.
#[tokio::test]
async fn the_person_goes_back_lists_switches_and_deletes_versions() {
    use v1::server_message::Body;
    let dir = temp_dir();
    let (_factory, mut client, mut view) = setup(&dir).await;
    say(&mut client, &mut view, "one").await;
    say(&mut client, &mut view, "two").await;
    say(&mut client, &mut view, "three").await;
    assert_eq!(branches(&view), 0);

    let (two, at) = entry(&view, "two");
    let stale = v1::ForkConversation { agent_id: "builder".into(), seq: two, timestamp_ms: at + 1 };
    let refused = apply_request(&mut client, &mut view, client_message::Body::ForkConversation(stale)).await;
    assert!(
        matches!(&refused, Body::Error(error) if error.code() == v1::ErrorCode::InvalidRequest && error.message == "The conversation changed. Try again."),
        "{refused:?}"
    );
    let fork = v1::ForkConversation { agent_id: "builder".into(), seq: two, timestamp_ms: at };
    let done = apply_request(&mut client, &mut view, client_message::Body::ForkConversation(fork)).await;
    assert!(matches!(done, Body::Ack(_)), "{done:?}");
    view.follow(&mut client, |view| branches(view) == 1 && !shown(view).contains(&"three".to_owned())).await;
    assert_eq!(shown(&view), ["one", "echo: one"]);

    let list = v1::ListBranches { agent_id: "builder".into() };
    let Body::Branches(listed) = apply_request(&mut client, &mut view, client_message::Body::ListBranches(list.clone())).await else {
        panic!("Branches")
    };
    assert_eq!(listed.agent_id, "builder");
    let [version] = listed.branches.as_slice() else { panic!("one version: {listed:?}") };
    assert_eq!((version.label.as_str(), version.forked_at_ms, version.messages, version.made_by.as_str()), ("two", at, 4, ""));

    // Switched to it before the agent heard of the first change: it's as if
    // nothing happened, with no note, and nothing set aside.
    let switch = v1::SwitchBranch { agent_id: "builder".into(), branch_id: version.id.clone() };
    let done = apply_request(&mut client, &mut view, client_message::Body::SwitchBranch(switch.clone())).await;
    assert!(matches!(done, Body::Ack(_)), "{done:?}");
    view.follow(&mut client, |view| shown(view).contains(&"three".to_owned())).await;
    let refused = apply_request(&mut client, &mut view, client_message::Body::SwitchBranch(switch)).await;
    assert!(
        matches!(&refused, Body::Error(error) if error.code() == v1::ErrorCode::NotFound && error.message == "This version of the conversation is gone."),
        "{refused:?}"
    );
    view.follow(&mut client, |view| branches(view) == 0).await;
    say(&mut client, &mut view, "four").await;
    assert!(notes(&view).is_empty(), "{:?}", notes(&view));
    assert_eq!(shown(&view)[4..], ["three", "echo: three", "four", "echo: four"]);

    let fork = v1::ForkConversation { agent_id: "builder".into(), seq: two, timestamp_ms: at };
    apply_request(&mut client, &mut view, client_message::Body::ForkConversation(fork)).await;
    view.follow(&mut client, |view| branches(view) == 1).await;
    let Body::Branches(listed) = apply_request(&mut client, &mut view, client_message::Body::ListBranches(list.clone())).await else {
        panic!("Branches")
    };
    let delete = v1::DeleteBranch { agent_id: "builder".into(), branch_id: listed.branches[0].id.clone() };
    let done = apply_request(&mut client, &mut view, client_message::Body::DeleteBranch(delete)).await;
    assert!(matches!(done, Body::Ack(_)), "{done:?}");
    view.follow(&mut client, |view| branches(view) == 0).await;
    let unknown = v1::ListBranches { agent_id: "nobody".into() };
    let refused = apply_request(&mut client, &mut view, client_message::Body::ListBranches(unknown)).await;
    assert!(matches!(&refused, Body::Error(error) if error.code() == v1::ErrorCode::NotFound), "{refused:?}");
}

/// Changed again before the agent read its note: it
/// gets one note, about the conversation it remembers and the one it's in
/// now, never one about a conversation it's no longer in.
#[tokio::test]
async fn a_second_change_before_the_note_is_read_says_both_in_one() {
    let dir = temp_dir();
    let (factory, mut client, mut view) = setup(&dir).await;
    say(&mut client, &mut view, "one").await;
    say(&mut client, &mut view, "two").await;
    let (two, at) = entry(&view, "two");
    fork::fork_for_test(&factory, "builder", two, at).await.expect("forked");
    view.follow(&mut client, |view| !shown(view).contains(&"two".to_owned())).await;
    let older = fork::branches_for_test(&factory, "builder")[0].id.clone();
    say(&mut client, &mut view, "three").await;
    assert_eq!(notes(&view).len(), 1);

    // Back to where the older version parts (its note waits), then switched
    // to that version, which sets nothing aside: the note says the switch,
    // from where the two part, and nothing of going back.
    let (reply, at) = entry(&view, "echo: one");
    fork::fork_for_test(&factory, "builder", reply, at).await.expect("forked");
    view.follow(&mut client, |view| !shown(view).contains(&"three".to_owned())).await;
    fork::switch_for_test(&factory, "builder", &older).await.expect("switched");
    view.follow(&mut client, |view| shown(view).contains(&"two".to_owned())).await;
    say(&mut client, &mut view, "four").await;
    let given = notes(&view);
    assert_eq!(given.len(), 1, "only the new note: {given:?}");
    let note = &given[0].0;
    assert!(note.starts_with("The person switched you to another version of your conversation"), "{note}");
    // The note before, "three" and its reply.
    assert!(note.contains(", 3 messages) is kept as another version"), "{note}");
    assert_eq!(shown(&view), ["one", "echo: one", "two", "echo: two", "four", "echo: four"]);

    // Back twice: one note, from the earlier point, with all it forgot.
    say(&mut client, &mut view, "five").await;
    let (five, at_five) = entry(&view, "five");
    fork::fork_for_test(&factory, "builder", five, at_five).await.expect("forked");
    view.follow(&mut client, |view| !shown(view).contains(&"five".to_owned())).await;
    let (four, at_four) = entry(&view, "four");
    fork::fork_for_test(&factory, "builder", four, at_four).await.expect("forked");
    view.follow(&mut client, |view| !shown(view).contains(&"four".to_owned())).await;
    say(&mut client, &mut view, "six").await;
    let given = notes(&view);
    assert_eq!(given.len(), 2, "the switch's and one more: {given:?}");
    let note = &given[1].0;
    assert!(note.starts_with("The person took your conversation back to "), "{note}");
    // four, five and their replies.
    assert!(note.contains(", 4 messages) is kept as another version"), "{note}");
    assert_eq!(&shown(&view)[4..], ["six", "echo: six"]);
}

/// `agent`, selected, calls `tool` with `args`; returns what it said, and
/// the call's seq.
async fn call(client: &mut Client, view: &mut View, agent: &str, tool: &str, args: serde_json::Value) -> (String, u64) {
    let before = view.entries.len();
    apply_request(client, view, send_message(agent, &format!("call {tool} {args}"))).await;
    view.follow(client, |view| view.entries.len() >= before + 3 && view.agent(agent).status() == AgentStatus::Idle).await;
    view.entries[before..]
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) if call.name == tool => Some((call.output.clone(), entry.seq)),
            _ => None,
        })
        .expect("the call")
}

async fn select(client: &mut Client, view: &mut View, agent: &str) {
    apply_request(client, view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })).await;
}

/// History's conversation changes, newest first.
async fn changes(client: &mut Client, view: &mut View) -> Vec<v1::NeedsYouRecord> {
    use v1::server_message::Body;
    let list = v1::ListNeedsYouHistory { query: String::new(), before_ms: 0, limit: 0 };
    let Body::NeedsYouHistory(history) = apply_request(client, view, client_message::Body::ListNeedsYouHistory(list)).await else {
        panic!("history")
    };
    history.records.into_iter().filter(|record| record.kind() == v1::NeedsYouRecordKind::Conversation).collect()
}

/// Rivet, an admin agent, has the Conversations extension: it lists where
/// an agent's conversation can go back to, takes it back and switches it,
/// with a reason the agent reads and the person sees in History; never its
/// own conversation, and nothing after reading a web page.
#[tokio::test]
async fn rivet_takes_an_agents_conversation_back_with_its_reason() {
    let dir = temp_dir();
    let (factory, mut client, mut view) = setup(&dir).await;
    let rivet = v1::UpdateAgent {
        agent_id: "rivet".into(),
        model: Some(crate::common::MODEL.into()),
        effort: Some("off".into()),
        ..Default::default()
    };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;
    say(&mut client, &mut view, "one").await;
    say(&mut client, &mut view, "two").await;
    say(&mut client, &mut view, "three").await;
    let (two, at) = entry(&view, "two");
    let (three, _) = entry(&view, "three");
    let enabled = |view: &View, agent: &str| view.agent(agent).extensions.iter().any(|ext| ext.id == "conversations" && ext.enabled);
    assert!(enabled(&view, "rivet") && !enabled(&view, "builder"));

    select(&mut client, &mut view, "rivet").await;
    let (points, _) = call(&mut client, &mut view, "rivet", "conversation_points", serde_json::json!({"agent": "Builder"})).await;
    let lines: Vec<&str> = points.lines().collect();
    assert_eq!(lines[0], "Builder's conversation can go back to right before any of these, newest first:", "{points}");
    assert!(lines[1].starts_with(&format!("p{three} · ")) && lines[1].ends_with(" · the person: “three”"), "{points}");
    assert!(lines[2].starts_with(&format!("p{two} · ")) && lines[2].ends_with(" · the person: “two”"), "{points}");
    // The first message is where it starts: nothing before it.
    assert_eq!(lines.len(), 3, "{points}");

    let (refused, _) =
        call(&mut client, &mut view, "rivet", "conversation_fork", serde_json::json!({"agent": "rivet", "point": "p1", "reason": "x"}))
            .await;
    assert_eq!(refused, "You can't change your own conversation: it would stop the run you're in. Nothing was changed.");
    let (refused, _) =
        call(&mut client, &mut view, "rivet", "conversation_fork", serde_json::json!({"agent": "builder", "point": "two", "reason": "x"}))
            .await;
    assert!(refused.starts_with("\"two\" isn't a point"), "{refused}");

    let fork = serde_json::json!({"agent": "builder", "point": format!("p{two}"), "reason": "Matt wants to try two again."});
    let (done, call_seq) = call(&mut client, &mut view, "rivet", "conversation_fork", fork).await;
    assert!(done.starts_with(&format!("Builder's conversation went back to right before p{two} (")), "{done}");
    assert!(done.ends_with("Builder was told, with your reason, and it shows in the person's History."), "{done}");
    view.follow(&mut client, |view| branches(view) == 1).await;
    let versions = fork::branches_for_test(&factory, "builder");
    assert_eq!((versions[0].made_by.as_str(), versions[0].fork.ms), ("rivet", at));

    let records = changes(&mut client, &mut view).await;
    let [record] = records.as_slice() else { panic!("one record: {records:?}") };
    let time = jiff::Timestamp::from_millisecond(at)
        .expect("a time")
        .to_zoned_datetime(jiff::tz::TimeZone::system())
        .strftime("%H:%M")
        .to_string();
    assert_eq!(record.title, format!("Rivet took Builder's conversation back to {time}"));
    assert_eq!(record.outcome(), v1::NeedsYouOutcome::Changed);
    assert_eq!(
        (record.agent_id.as_str(), record.subject_agent_id.as_str(), record.agent_reason.as_str(), record.seq),
        ("rivet", "builder", "Matt wants to try two again.", call_seq)
    );

    // Its versions are listed, to switch to.
    let (points, _) = call(&mut client, &mut view, "rivet", "conversation_points", serde_json::json!({"agent": "builder"})).await;
    let line = format!("{} · set aside ", versions[0].id);
    assert!(points.contains(&line) && points.contains(" by Rivet · 4 messages · “two”"), "{points}");

    // Builder reads Rivet's reason in its note.
    select(&mut client, &mut view, "builder").await;
    say(&mut client, &mut view, "four").await;
    let note = notes(&view).pop().expect("a note").0;
    assert!(note.starts_with("Rivet took your conversation back to "), "{note}");
    assert!(note.contains("Rivet's reason: Matt wants to try two again."), "{note}");

    select(&mut client, &mut view, "rivet").await;
    let switch = serde_json::json!({"agent": "builder", "branch": versions[0].id, "reason": "Back to where it was."});
    let (done, _) = call(&mut client, &mut view, "rivet", "conversation_switch", switch).await;
    assert!(done.starts_with("Builder is now in the version you chose"), "{done}");
    let records = changes(&mut client, &mut view).await;
    assert_eq!(records[0].title, "Rivet switched Builder to another version");
    assert_eq!(records[0].agent_reason, "Back to where it was.");
    let (refused, _) = call(
        &mut client,
        &mut view,
        "rivet",
        "conversation_switch",
        serde_json::json!({"agent": "builder", "branch": "nope", "reason": ""}),
    )
    .await;
    assert_eq!(refused, "Builder has no version \"nope\": list them with conversation_points. Nothing was changed.");

    select(&mut client, &mut view, "builder").await;
    say(&mut client, &mut view, "five").await;
    let note = notes(&view).pop().expect("a note").0;
    assert!(note.starts_with("Rivet switched you to another version of your conversation"), "{note}");
    assert!(note.contains("Rivet's reason: Back to where it was."), "{note}");

    // A version set aside is deleted for good, with its reason in History.
    select(&mut client, &mut view, "rivet").await;
    let aside = fork::branches_for_test(&factory, "builder");
    assert_eq!(aside.len(), 1, "{aside:?}");
    let delete = serde_json::json!({"agent": "builder", "branch": aside[0].id, "reason": "Matt doesn't need it."});
    let (done, _) = call(&mut client, &mut view, "rivet", "conversation_delete", delete).await;
    assert_eq!(done, "Deleted that version of Builder's conversation, for good. It shows in the person's History.");
    assert!(fork::branches_for_test(&factory, "builder").is_empty());
    let records = changes(&mut client, &mut view).await;
    assert_eq!((records[0].title.as_str(), records[0].agent_reason.as_str()), ("Rivet deleted one of Builder's other versions", "Matt doesn't need it."));
    let again = serde_json::json!({"agent": "builder", "branch": aside[0].id, "reason": "Again."});
    let (refused, _) = call(&mut client, &mut view, "rivet", "conversation_delete", again).await;
    assert!(refused.ends_with("list them with conversation_points. Nothing was deleted."), "{refused}");
    // Never without a reason: it can't be undone.
    let bare = serde_json::json!({"agent": "builder", "branch": aside[0].id, "reason": " "});
    let (refused, _) = call(&mut client, &mut view, "rivet", "conversation_delete", bare).await;
    assert_eq!(refused, "Give reason: why this version can go, in a short line the person reads in History. Nothing was deleted.");

    // Never after reading a web page.
    factoryd::pipes::taint_for_test(&factory, "rivet", "browser:https://example.com");
    select(&mut client, &mut view, "rivet").await;
    let (refused, _) = call(&mut client, &mut view, "rivet", "conversation_points", serde_json::json!({"agent": "builder"})).await;
    assert!(
        refused.starts_with("You've read a page on https://example.com, so you can't look at or change other agents' conversations"),
        "{refused}"
    );
    // A delete says where the person deletes a version instead.
    let delete = serde_json::json!({"agent": "builder", "branch": "b1", "reason": "Gone."});
    let (refused, _) = call(&mut client, &mut view, "rivet", "conversation_delete", delete).await;
    assert!(refused.ends_with("The person can delete a version with Delete… in the agent's Other versions. Nothing was deleted."), "{refused}");
}

/// A teammate's message queued while the agent works
/// waits out the fork and comes after the note, in the conversation as it
/// is now, never handed to the stopped run on the conversation as it was;
/// the command the fork cut off says so in the version set aside.
#[tokio::test]
async fn a_message_queued_during_a_fork_comes_after_the_note() {
    let dir = temp_dir();
    let (factory, mut client, mut view) = setup(&dir).await;
    apply_request(&mut client, &mut view, create_agent("site", "Ana", "")).await;
    select(&mut client, &mut view, "builder").await;
    say(&mut client, &mut view, "one").await;
    // Runs until the file is there, for a minute at most.
    let long = r#"call bash {"command":"n=0; until [ -e go ] || [ $n -ge 600 ]; do sleep 0.1; n=$((n+1)); done"}"#;
    apply_request(&mut client, &mut view, send_message("builder", long)).await;
    view.follow(&mut client, |view| {
        view.entries.iter().any(|entry| matches!(&entry.kind, Some(Kind::ToolCall(call)) if call.name == "bash"))
    })
    .await;
    let ana = r#"call agents_send {"to":"builder","text":"Ana here"}"#;
    apply_request(&mut client, &mut view, send_message("ana", ana)).await;
    view.follow(&mut client, |view| view.agent("builder").queued_messages == 1).await;

    let (call, at) = entry(&view, long);
    fork::fork_for_test(&factory, "builder", call, at).await.expect("forked");
    let from_ana = |view: &View| {
        view.entries.iter().position(
            |entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.source() == v1::EventSource::Agent && event.text == "Ana here"),
        )
    };
    view.follow(&mut client, |view| from_ana(view).is_some() && idle(view, "builder")).await;
    let note = view.entries.iter().position(|entry| {
        matches!(&entry.kind, Some(Kind::Event(event)) if event.source() == v1::EventSource::Factory && event.text.contains("took your conversation back"))
    });
    assert!(note.expect("the note") < from_ana(&view).expect("Ana's message"), "{:?}", crate::common::describe(&view.entries));
    assert!(!shown(&view).contains(&long.to_owned()));

    // Kept in the conversation's file, after the note.
    let segment = segment_text(&dir.join("home/agents/builder"), 1);
    let (note_at, ana_at) = (segment.find("took your conversation back").expect("the note"), segment.find("Ana here").expect("Ana's"));
    assert!(note_at < ana_at);
    // The command cut off says why in the version set aside.
    let version = &fork::branches_for_test(&factory, "builder")[0];
    let set_aside = segment_text(&dir.join("home/agents/builder/branches").join(&version.id), 1);
    assert!(set_aside.contains("Stopped: the conversation was taken back, or switched to another version, before this finished."));
    std::fs::write(dir.join("site/go"), "").expect("go");
}
