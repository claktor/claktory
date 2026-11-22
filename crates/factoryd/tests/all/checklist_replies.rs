//! The person's reply on a checklist item: it reaches the agent the item is
//! for as the person's message, naming the item, shows in the conversation
//! with a reference to it, waiting too, and joins the item's history as
//! theirs, its status untouched. What it can't be is refused with a reason.

use crate::common::{Client, Scripted, View, apply_request, create_agent, create_project, send_message, start_with, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind, server_message::Body};

fn select(agent: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

fn reply(agent: &str, checklist: &str, item: u32, text: &str) -> client_message::Body {
    client_message::Body::SendMessage(v1::SendMessage {
        agent_id: agent.into(),
        text: text.into(),
        checklist_item: item,
        checklist_agent_id: checklist.into(),
        ..Default::default()
    })
}

async fn refused(client: &mut Client, view: &mut View, body: client_message::Body) -> String {
    match apply_request(client, view, body).await {
        Body::Error(error) => error.message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// Has `agent` call `tool` with `args` and waits until it's idle again.
async fn call(client: &mut Client, view: &mut View, agent: &str, tool: &str, args: &str) {
    apply_request(client, view, select(agent)).await;
    apply_request(client, view, send_message(agent, &format!("call {tool} {args}"))).await;
    view.follow(client, |view| {
        idle(view, agent) && view.entries.iter().any(|entry| matches!(&entry.kind, Some(Kind::ToolCall(call)) if call.name == tool))
    })
    .await;
}

/// The checklist's items, as its panel shows them.
async fn panel(client: &mut Client, view: &mut View, agent: &str) -> Vec<v1::ChecklistItem> {
    apply_request(client, view, select(agent)).await;
    let open = client_message::Body::OpenPanel(v1::OpenPanel { extension_id: "checklist".into(), agent_id: String::new() });
    assert!(matches!(apply_request(client, view, open).await, Body::Ack(_)));
    match view.extension_view.as_ref().and_then(|view| view.body.clone()) {
        Some(v1::extension_view::Body::Checklist(checklist)) => checklist.items,
        other => panic!("expected the checklist, got {other:?}"),
    }
}

/// The person's messages in the open conversation, with the item each is about.
fn said(view: &View) -> Vec<(String, Option<v1::ChecklistItemRef>)> {
    view.entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::User(user)) => Some((user.text.clone(), user.checklist_item.clone())),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn the_person_replies_on_an_item_and_the_agent_it_is_for_reads_it() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let requests = models.requests.clone();
    let (addr, _factory) = start_with(&dir, models).await;
    let last = || requests.lock().expect("lock").last().expect("a request").prompt.clone();
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Lead", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Worker", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Other", "")).await;
    let on = v1::ExtensionChange { id: "checklist".into(), enabled: Some(true), ..Default::default() };
    let on = client_message::Body::UpdateAgent(v1::UpdateAgent { agent_id: "lead".into(), extension: Some(on), ..Default::default() });
    assert!(matches!(apply_request(&mut client, &mut view, on).await, Body::Ack(_)));
    call(&mut client, &mut view, "lead", "checklist_add", r#"{"items":["Pick the \"launch\" color"]}"#).await;
    call(&mut client, &mut view, "lead", "checklist_add", r#"{"items":["Build the page"],"owner":"worker"}"#).await;
    call(&mut client, &mut view, "lead", "checklist_update", r#"{"updates":[{"id":1,"status":"blocked","waits_on":"person_decision","note":"Blue or green?"}]}"#).await;

    // On Lead's own item: Lead reads it as the person's, naming the item.
    assert!(matches!(apply_request(&mut client, &mut view, reply("lead", "", 1, " Blue. ")).await, Body::Ack(_)));
    view.follow(&mut client, |view| idle(view, "lead") && said(view).iter().any(|(text, _)| text == "Blue.")).await;
    let prompt = last();
    assert!(prompt.contains(r#"about="checklist item #1: Pick the 'launch' color">"#), "{prompt}");
    assert!(prompt.ends_with("\nBlue.\n</sent>"), "{prompt}");
    let (_, item) = said(&view).pop().expect("the reply");
    assert_eq!(item, Some(v1::ChecklistItemRef { agent_id: "lead".into(), id: 1, text: "Pick the \"launch\" color".into() }));

    // The item's history has it as the person's; it's still blocked, and
    // still shows what Lead asked.
    let items = panel(&mut client, &mut view, "lead").await;
    let change = items[0].log.last().expect("its history");
    assert_eq!((change.text.as_str(), change.author.as_str()), ("Blue.", ""));
    assert_eq!((items[0].status(), items[0].note.as_str()), (v1::ChecklistStatus::Blocked, "Blue or green?"));

    // On Lead's item that Worker owns, to Worker: named as Lead's.
    apply_request(&mut client, &mut view, select("worker")).await;
    assert!(matches!(apply_request(&mut client, &mut view, reply("worker", "lead", 2, "Use the new logo.")).await, Body::Ack(_)));
    view.follow(&mut client, |view| idle(view, "worker") && !said(view).is_empty()).await;
    assert!(last().contains(r#"about="checklist item Lead#2: Build the page">"#), "{}", last());
    let (_, item) = said(&view).pop().expect("the reply");
    assert_eq!(item.map(|item| (item.agent_id, item.id)), Some(("lead".into(), 2)));
    let items = panel(&mut client, &mut view, "lead").await;
    assert_eq!(items[1].log.last().map(|change| (change.text.as_str(), change.author.as_str())), Some(("Use the new logo.", "")));

    // Waiting behind Worker's work, it names the item too.
    apply_request(&mut client, &mut view, select("worker")).await;
    apply_request(&mut client, &mut view, send_message("worker", "hang")).await;
    view.follow(&mut client, |view| view.agent("worker").status() == AgentStatus::Working && view.agent("worker").queued_messages == 0)
        .await;
    assert!(matches!(apply_request(&mut client, &mut view, reply("worker", "lead", 2, "And the footer.")).await, Body::Ack(_)));
    let waiting = view.queued.as_ref().expect("the queue").messages[0].checklist_item.clone();
    assert_eq!(waiting.map(|item| (item.agent_id, item.id)), Some(("lead".into(), 2)));

    // What a reply can't be.
    assert_eq!(refused(&mut client, &mut view, reply("lead", "", 9, "Hm")).await, "That item is no longer on Lead's checklist.");
    assert_eq!(
        refused(&mut client, &mut view, reply("worker", "lead", 1, "Blue")).await,
        "That item is on Lead's checklist, so reply to Lead."
    );
    assert_eq!(refused(&mut client, &mut view, reply("other", "lead", 2, "Hm")).await, "That item is Worker's, so reply to Worker.");
    assert_eq!(refused(&mut client, &mut view, reply("worker", "", 1, "Blue")).await, "That item is no longer on Worker's checklist.");
    assert_eq!(refused(&mut client, &mut view, reply("lead", "", 1, "  ")).await, "Write your reply first.");
    let voice = client_message::Body::SendMessage(v1::SendMessage {
        agent_id: "lead".into(),
        voice: true,
        checklist_item: 1,
        ..Default::default()
    });
    assert_eq!(refused(&mut client, &mut view, voice).await, "Type your reply. Voice messages can't be used here.");
    let items = panel(&mut client, &mut view, "lead").await;
    assert_eq!(items[0].log.iter().filter(|change| change.author.is_empty()).count(), 1, "refusals leave no history");
}
