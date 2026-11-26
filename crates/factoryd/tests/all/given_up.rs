//! Agents give up access on their own (#236): a pipe, or only reading or
//! leading it (pipe_leave); an app (apps_give_up); messaging an agent of
//! another project (agents_give_up_reach). At once, with no card, only
//! their own and only what they hold when the call runs; History keeps it
//! with their reason; only the person gives it back.

use serde_json::json;

use crate::common::{Client, View, apply_request, create_agent, create_project, start, temp_dir};
use crate::pipes::{call, idle, pipe_of, select, setup};
use factory_protocol::v1::{self, client_message::Body as Request, entry::Kind, server_message::Body};
use factoryd::pipes;

async fn given_up(client: &mut Client, view: &mut View) -> Vec<v1::NeedsYouRecord> {
    let Body::NeedsYouHistory(history) =
        apply_request(client, view, Request::ListNeedsYouHistory(v1::ListNeedsYouHistory::default())).await
    else {
        panic!("history")
    };
    history.records.into_iter().filter(|record| record.kind() == v1::NeedsYouRecordKind::GivenUp).collect()
}

fn update(pipe: &str, members: Option<&[&str]>, lead: Option<&str>, limited: Option<&[&str]>) -> Request {
    let ids = |ids: &[&str]| v1::PipeMembers { agent_ids: ids.iter().map(|id| (*id).to_owned()).collect() };
    Request::UpdatePipe(v1::UpdatePipe {
        pipe_id: pipe.into(),
        members: members.map(ids),
        lead_id: lead.map(str::to_owned),
        quarantined: limited.map(ids),
        ..Default::default()
    })
}

fn lines(factory: &factoryd::factory::Factory, pipe: &str) -> Vec<String> {
    pipes::messages_for_test(factory, pipe).into_iter().map(|message| message.text).collect()
}

/// The entry at `seq` in the open conversation is a call of `tool`.
fn at_call(view: &View, seq: u64, tool: &str) -> bool {
    view.entries.iter().any(|entry| entry.seq == seq && matches!(&entry.kind, Some(Kind::ToolCall(call)) if call.name == tool))
}

#[tokio::test]
async fn an_agent_gives_up_reading_leading_or_a_whole_pipe_and_only_the_person_gives_it_back() {
    let (_dir, factory, mut client, mut view, pipe) = setup(false).await;
    let leave = |args: serde_json::Value| args.to_string();

    // A reason is required, and only what it is.
    let said = call(&mut client, &mut view, "bo", "pipe_leave", &leave(json!({"only": "reading"}))).await;
    assert!(said.contains("must have required properties reason"), "{said}");
    let said = call(&mut client, &mut view, "bo", "pipe_leave", &leave(json!({"only": "reading", "reason": " "}))).await;
    assert!(said.starts_with("Say why you're giving it up") && said.ends_with("Nothing was changed."), "{said}");
    let said = call(&mut client, &mut view, "bo", "pipe_leave", &leave(json!({"only": "writing", "reason": "x"}))).await;
    assert_eq!(said, "only is \"reading\" or \"leading\", not \"writing\". Nothing was changed.");
    // Its lead can't stop reading before it stops leading.
    let said = call(&mut client, &mut view, "lea", "pipe_leave", &leave(json!({"only": "reading", "reason": "Done reading."}))).await;
    assert!(said.contains("give up leading it first") && said.ends_with("Nothing was changed."), "{said}");
    assert_eq!(pipe_of(&view, &pipe).member_ids, ["lea", "bo"]);

    // Bo only sends now.
    let said = call(&mut client, &mut view, "bo", "pipe_leave", &leave(json!({"only": "reading", "reason": "I only post updates."}))).await;
    assert!(said.starts_with("You now only send to the pipe \"Website ⇄ Marketing\""), "{said}");
    view.follow(&mut client, |view| pipe_of(view, &pipe).send_only_ids == ["bo"]).await;
    let read = call(&mut client, &mut view, "bo", "pipe_read", "{}").await;
    // Gone from its tools, as for any member that only sends.
    assert!(read.contains("pipe_read doesn't work any more"), "{read}");
    let said = call(&mut client, &mut view, "bo", "pipe_leave", &leave(json!({"only": "reading", "reason": "Again."}))).await;
    assert!(said.starts_with("You already only send to") && said.ends_with("Nothing was changed."), "{said}");

    // Lea stops leading: no one else may (Bo only sends), so no one does.
    let said = call(&mut client, &mut view, "lea", "pipe_leave", &leave(json!({"only": "leading", "reason": "Too many wakes."}))).await;
    assert!(said.starts_with("You no longer lead the pipe \"Website ⇄ Marketing\""), "{said}");
    view.follow(&mut client, |view| pipe_of(view, &pipe).lead_id.is_empty()).await;
    assert_eq!(pipe_of(&view, &pipe).member_ids, ["lea", "bo"], "still one of its agents");
    let said = call(&mut client, &mut view, "lea", "pipe_leave", &leave(json!({"only": "leading", "reason": "Again."}))).await;
    assert!(said.starts_with("You don't lead") && said.ends_with("Nothing was changed."), "{said}");

    // Bo leaves it; the pipe stays as it is.
    let said = call(&mut client, &mut view, "bo", "pipe_leave", &leave(json!({"reason": "Not my project any more."}))).await;
    assert!(said.starts_with("Left the pipe \"Website ⇄ Marketing\""), "{said}");
    view.follow(&mut client, |view| pipe_of(view, &pipe).member_ids == ["lea"]).await;
    let shown = pipe_of(&view, &pipe);
    assert!(shown.send_only_ids.is_empty());
    assert_eq!(shown.state(), v1::PipeState::Open);
    // Its frame still lists the tool: it says why it's gone.
    let said = call(&mut client, &mut view, "bo", "pipe_leave", &leave(json!({"reason": "Again."}))).await;
    assert!(said.starts_with("You no longer have a pipe"), "{said}");
    let said: Vec<String> = lines(&factory, &pipe)
        .into_iter()
        .filter(|line| line.starts_with("Bo ") || line.starts_with("Lea ") || line.starts_with("No agent answers"))
        .collect();
    assert_eq!(
        said,
        [
            "Bo stopped reading this pipe: it only sends to it now.",
            "Lea stopped leading this pipe.",
            "No agent answers this pipe now: messages from Ana wait for you here. Choose its lead in the pipe's settings.",
            "Bo left this pipe.",
        ]
    );

    // History: Claktory's sentences, by its own pipe label, each with its
    // reason and its call.
    let records = given_up(&mut client, &mut view).await;
    let titles: Vec<(&str, &str, &str, &str)> = records
        .iter()
        .map(|record| (record.title.as_str(), record.agent_id.as_str(), record.agent_reason.as_str(), record.pipe_id.as_str()))
        .collect();
    let left = "Bo left the pipe \"Website ⇄ Marketing\"";
    let leading = "Lea stopped leading the pipe \"Website ⇄ Marketing\"";
    let reading = "Bo stopped reading the pipe \"Website ⇄ Marketing\": it only sends to it now";
    assert_eq!(
        titles,
        [
            (left, "bo", "Not my project any more.", pipe.as_str()),
            (leading, "lea", "Too many wakes.", pipe.as_str()),
            (reading, "bo", "I only post updates.", pipe.as_str()),
        ]
    );
    assert!(records.iter().all(|record| record.outcome() == v1::NeedsYouOutcome::Changed));
    apply_request(&mut client, &mut view, select("bo")).await;
    assert!(at_call(&view, records[0].seq, "pipe_leave"), "its call");

    // The person adds Bo back and limits both to the pipe: Bo leads, never
    // Lea, who gave it up, until the person chooses her.
    apply_request(&mut client, &mut view, update(&pipe, Some(&["lea", "bo"]), None, Some(&["lea", "bo"]))).await;
    view.follow(&mut client, |view| pipe_of(view, &pipe).lead_id == "bo").await;
    apply_request(&mut client, &mut view, update(&pipe, None, Some("lea"), None)).await;
    view.follow(&mut client, |view| pipe_of(view, &pipe).lead_id == "lea").await;
    // Limited to it, it's the person's to take off.
    let said = call(&mut client, &mut view, "bo", "pipe_leave", &leave(json!({"reason": "Let me out."}))).await;
    assert!(!said.starts_with("Left"), "{said}");
    assert_eq!(pipe_of(&view, &pipe).member_ids, ["lea", "bo"]);
    assert_eq!(given_up(&mut client, &mut view).await.len(), 3);
}

#[tokio::test]
async fn a_pipe_the_person_took_away_first_is_refused_with_no_change() {
    let (_dir, factory, mut client, mut view, pipe) = setup(false).await;
    let other = pipes::open_for_test(&factory, "site", "Bugs", &["lea", "bo"], "lea", false);
    view.follow(&mut client, |view| crate::pipes::shown(view, &other).is_some()).await;
    // Bo's context begins with the pipes' tools.
    assert!(call(&mut client, &mut view, "bo", "pipe_status", "{}").await.contains("Website"));
    apply_request(&mut client, &mut view, update(&pipe, Some(&["lea"]), None, None)).await;
    view.follow(&mut client, |view| pipe_of(view, &pipe).member_ids == ["lea"]).await;
    let said = call(&mut client, &mut view, "bo", "pipe_leave", &json!({"pipe": pipe, "reason": "Done."}).to_string()).await;
    assert!(said.starts_with(&format!("You have no pipe \"{pipe}\"")) && said.ends_with("Nothing was changed."), "{said}");
    assert_eq!(pipe_of(&view, &other).member_ids, ["lea", "bo"]);
    assert!(given_up(&mut client, &mut view).await.is_empty());

    // The last agents leave: the pipe stays as it is, open, with no lead.
    for agent in ["bo", "lea"] {
        let said = call(&mut client, &mut view, agent, "pipe_leave", &json!({"pipe": "Bugs", "reason": "Done."}).to_string()).await;
        assert!(said.starts_with("Left the pipe \"Bugs\""), "{said}");
    }
    view.follow(&mut client, |view| pipe_of(view, &other).member_ids.is_empty()).await;
    let shown = pipe_of(&view, &other);
    assert_eq!((shown.state(), shown.lead_id.as_str()), (v1::PipeState::Open, ""));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_gives_up_an_app_as_the_persons_removal_takes_it() {
    let dir = temp_dir();
    let (mut client, mut view, _factory, _stand, _tools) = crate::app_access::charts(&dir).await;
    let grant = v1::GrantApp { app_id: "charts".into(), agent_id: "lea".into(), scopes: vec!["view".into(), "edit".into()] };
    apply_request(&mut client, &mut view, Request::GrantApp(grant)).await;
    let subscribe = v1::SubscribeApp { app_id: "charts".into(), event_kind: "alert".into(), agent_ids: vec!["lea".into()] };
    apply_request(&mut client, &mut view, Request::SubscribeApp(subscribe)).await;
    view.follow(&mut client, |view| crate::app_access::app(view).subscriptions.iter().any(|heard| heard.agent_ids == ["lea"])).await;

    let said = call(&mut client, &mut view, "lea", "apps_give_up", r#"{"app": "Nope", "reason": "x"}"#).await;
    assert_eq!(said, "\"Nope\" isn't an app you were given. Yours: Charts. Nothing was changed.");
    let said = call(&mut client, &mut view, "lea", "apps_give_up", r#"{"app": "charts", "reason": "I don't chart any more."}"#).await;
    assert!(said.starts_with("Gave up Charts: its tools and events no longer reach you"), "{said}");
    view.follow(&mut client, |view| crate::app_access::app(view).grants.is_empty()).await;
    // Its events go to no one else: nobody hears them now.
    assert!(crate::app_access::app(&view).subscriptions.is_empty());
    // Its frame still lists the tools: they say why they're gone.
    let said = call(&mut client, &mut view, "lea", "apps_give_up", r#"{"app": "charts", "reason": "Again."}"#).await;
    assert_eq!(said, "You no longer have any app, so apps_give_up has nothing to give up.");
    let said = call(&mut client, &mut view, "lea", "charts_list", "{}").await;
    assert!(said.starts_with("You no longer have charts_list"), "{said}");

    // History: one GIVEN_UP record, not an app access change too.
    let records = given_up(&mut client, &mut view).await;
    let shown: Vec<(&str, &str, &str, &str)> = records
        .iter()
        .map(|record| (record.title.as_str(), record.agent_id.as_str(), record.agent_reason.as_str(), record.app_id.as_str()))
        .collect();
    assert_eq!(shown, [("Lea gave up Charts", "lea", "I don't chart any more.", "charts")]);
    assert!(at_call(&view, records[0].seq, "apps_give_up"));
    let changes = crate::app_access::history(&mut client, &mut view).await;
    assert!(changes.iter().all(|record| !record.title.contains("gave up")), "{changes:?}");
}

#[tokio::test]
async fn an_agent_gives_up_messaging_an_agent_of_another_project_and_nothing_else() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_project("Lab", &dir.join("lab"))).await;
    for (project, name) in [("site", "Lea"), ("site", "Kai"), ("lab", "Bo"), ("lab", "Mo")] {
        apply_request(&mut client, &mut view, create_agent(project, name, "")).await;
    }
    let grant = |agent: &str, others: &[&str]| {
        let value = v1::OptionValue { agent_ids: others.iter().map(|other| (*other).to_owned()).collect(), ..Default::default() };
        let change = v1::ExtensionChange { id: "agents".into(), option_id: "also".into(), value: Some(value), ..Default::default() };
        Request::UpdateAgent(v1::UpdateAgent { agent_id: agent.into(), extension: Some(change), ..Default::default() })
    };
    apply_request(&mut client, &mut view, grant("lea", &["bo", "mo"])).await;
    apply_request(&mut client, &mut view, grant("bo", &["lea"])).await;
    let also = |view: &View, agent: &str| view.agent(agent).extensions[0].options["also"].agent_ids.clone();

    // Only an entry of its own list; its project's agents stay reachable.
    let said = call(&mut client, &mut view, "lea", "agents_give_up_reach", r#"{"to": "Kai", "reason": "x"}"#).await;
    assert_eq!(
        said,
        "\"Kai\" isn't an agent you were let message outside your project. Those you were let message outside your project: Bo, Mo. Nothing was changed."
    );
    let said = call(&mut client, &mut view, "lea", "agents_give_up_reach", r#"{"to": "Bo", "reason": "Our work together is done."}"#).await;
    assert!(said.starts_with("Gave up messaging Bo: it's no longer on your list."), "{said}");
    view.follow(&mut client, |view| also(view, "lea") == ["mo"]).await;
    // Bo still messages Lea: its own list is its own.
    assert_eq!(also(&view, "bo"), ["lea"]);
    let said = call(&mut client, &mut view, "lea", "agents_send", r#"{"to": "Bo", "text": "hi"}"#).await;
    assert!(said.starts_with("No agent you can message is called \"Bo\""), "{said}");
    // Told before its next step, as any change of whom it can message.
    assert!(
        view.entries
            .iter()
            .any(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.text.contains("You can no longer message: Bo"))),
        "told"
    );
    let said = call(&mut client, &mut view, "lea", "agents_give_up_reach", r#"{"to": "bo", "reason": "Again."}"#).await;
    assert!(said.contains("Those you were let message outside your project: Mo.") && said.ends_with("Nothing was changed."), "{said}");
    view.follow(&mut client, |view| idle(view, "lea")).await;

    let records = given_up(&mut client, &mut view).await;
    let shown: Vec<(&str, &str, &str, &str)> = records
        .iter()
        .map(|record| (record.title.as_str(), record.agent_id.as_str(), record.agent_reason.as_str(), record.subject_agent_id.as_str()))
        .collect();
    assert_eq!(shown, [("Lea gave up messaging Bo", "lea", "Our work together is done.", "bo")]);
}
