//! Typed blocks (#279, #282): a blocked checklist item says what it waits
//! on, and Claktory follows up on it. A wait on another item opens again
//! when that one is done; a wait on an agent tells it; a parked item
//! resumes at its time; a wait on the person shows in Needs you, and a
//! dismissed question about it is asked again; an outside wait is checked
//! again, less and less often; a long block is asked about, and told to
//! the owner's lead. The intervals are Settings → Checklist's.

use crate::common::{Client, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind, server_message::Body};

fn select(agent: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

fn ack(reply: Body) {
    assert!(matches!(reply, Body::Ack(_)), "expected ack, got {reply:?}");
}

fn checklist_on(agent: &str) -> client_message::Body {
    let change = v1::ExtensionChange { id: "checklist".into(), enabled: Some(true), ..Default::default() };
    client_message::Body::UpdateAgent(v1::UpdateAgent { agent_id: agent.into(), extension: Some(change), ..Default::default() })
}

fn lead(agent: &str, lead: &str) -> client_message::Body {
    client_message::Body::UpdateAgent(v1::UpdateAgent { agent_id: agent.into(), lead_id: Some(lead.into()), ..Default::default() })
}

fn intervals(times: v1::ChecklistSettings) -> client_message::Body {
    client_message::Body::UpdateSettings(v1::UpdateSettings { checklist: Some(times), ..Default::default() })
}

/// Has `agent` call `tool` with `args` (the scripted model does what it's
/// told) and returns what the tool answered.
async fn call(client: &mut Client, view: &mut View, agent: &str, tool: &str, args: &str) -> String {
    apply_request(client, view, select(agent)).await;
    let before = view.entries.len();
    apply_request(client, view, send_message(agent, &format!("call {tool} {args}"))).await;
    // Until the call is there and the agent is idle: a follow-up from
    // Claktory (a re-ask, a check) can run first, and its entries alone
    // mustn't pass for the call.
    let output = |view: &View| {
        view.entries.get(before..).unwrap_or_default().iter().find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) if call.name == tool => Some(call.output.clone()),
            _ => None,
        })
    };
    view.follow(client, |view| output(view).is_some() && idle(view, agent)).await;
    output(view).expect("a tool call")
}

/// The events from Claktory whose summary starts with `summary`.
fn events(view: &View, summary: &str) -> Vec<v1::Event> {
    view.entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::Event(event)) if event.summary.starts_with(summary) => Some(event.clone()),
            _ => None,
        })
        .collect()
}

/// The agent's checklist items, as its panel shows them.
async fn panel(client: &mut Client, view: &mut View, agent: &str) -> Vec<v1::ChecklistItem> {
    apply_request(client, view, select(agent)).await;
    let open = client_message::Body::OpenPanel(v1::OpenPanel { extension_id: "checklist".into(), agent_id: String::new() });
    ack(apply_request(client, view, open).await);
    match view.extension_view.as_ref().and_then(|view| view.body.clone()) {
        Some(v1::extension_view::Body::Checklist(checklist)) => checklist.items,
        other => panic!("expected the checklist, got {other:?}"),
    }
}

/// Waits for an event to `agent` whose summary starts with `summary`, and
/// returns it.
async fn told(client: &mut Client, view: &mut View, agent: &str, summary: &str) -> v1::Event {
    apply_request(client, view, select(agent)).await;
    view.follow(client, |view| !events(view, summary).is_empty()).await;
    events(view, summary).remove(0)
}

#[tokio::test]
async fn an_item_waiting_on_another_opens_again_when_that_one_is_done() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    // The nudge loop looks often, so a parked item resumes on time.
    factory.set_nudge_after(std::time::Duration::from_millis(150));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Lead", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Worker", "")).await;
    ack(apply_request(&mut client, &mut view, checklist_on("lead")).await);
    ack(apply_request(&mut client, &mut view, checklist_on("worker")).await);
    call(&mut client, &mut view, "lead", "checklist_add", r#"{"items":["Ship the release","Tidy the docs","Write the notes"]}"#).await;
    call(&mut client, &mut view, "worker", "checklist_add", r#"{"items":["Build the APK","Old task"]}"#).await;
    call(&mut client, &mut view, "worker", "checklist_update", r#"{"updates":[{"id":2,"status":"done"}]}"#).await;

    // What a wait on an item can't be.
    let update = |args: &str| format!(r#"{{"updates":[{{"id":1,"status":"blocked","waits_on":"item"{args}}}]}}"#);
    let refused = call(&mut client, &mut view, "lead", "checklist_update", &update("")).await;
    assert!(refused.contains("#1: waits_on item needs ref"), "{refused}");
    let refused = call(&mut client, &mut view, "lead", "checklist_update", &update(r#","ref":"1""#)).await;
    assert!(refused.contains("#1: an item can't wait on itself"), "{refused}");
    let refused = call(&mut client, &mut view, "lead", "checklist_update", &update(r#","ref":"Worker#2""#)).await;
    assert!(refused.contains("#1: worker #2 is already done: there's nothing to wait on"), "{refused}");
    let refused = call(&mut client, &mut view, "lead", "checklist_update", r#"{"updates":[{"id":1,"waits_on":"external"}]}"#).await;
    assert!(refused.contains("#1: waits_on goes with status blocked"), "{refused}");

    // Lead's #1 waits on Worker's #1; Worker's #1 can't then wait on it.
    let said = call(&mut client, &mut view, "lead", "checklist_update", &update(r#","ref":"Worker#1""#)).await;
    assert!(said.contains("#1 blocked, waits on item worker#1"), "{said}");
    let cycle = r#"{"updates":[{"id":1,"status":"blocked","waits_on":"item","ref":"Lead#1"}]}"#;
    let refused = call(&mut client, &mut view, "worker", "checklist_update", cycle).await;
    assert!(refused.contains("lead #1 already waits, through items, on this one"), "{refused}");

    // A wait on an agent tells it.
    let on_worker = r#"{"updates":[{"id":3,"status":"blocked","waits_on":"agent","ref":"Worker"}]}"#;
    call(&mut client, &mut view, "lead", "checklist_update", on_worker).await;
    let notice = told(&mut client, &mut view, "worker", "Lead is waiting on you").await;
    assert_eq!(
        notice.text,
        "Lead is waiting on you for its #3 \"Write the notes\". Nothing to answer if you already know; when what it needs is ready, tell Lead."
    );
    // The same wait set again, with other words, tells it nothing more.
    let again = r#"{"updates":[{"id":3,"status":"blocked","waits_on":"agent","ref":"Worker","blocked_on":"his review"}]}"#;
    let said = call(&mut client, &mut view, "lead", "checklist_update", again).await;
    assert!(said.contains("#3 blocked, waits on agent worker"), "{said}");

    // Parked, with a time to resume.
    let parked =
        call(&mut client, &mut view, "lead", "checklist_update", r#"{"updates":[{"id":2,"status":"parked","retry_at":"1h"}]}"#).await;
    assert!(parked.contains("#2 parked, resumes "), "{parked}");
    let refused =
        call(&mut client, &mut view, "lead", "checklist_update", r#"{"updates":[{"id":3,"retry_at":"1h","status":"open"}]}"#).await;
    assert!(refused.contains("#3: retry_at goes with waits_on external, or status parked"), "{refused}");
    let listed = call(&mut client, &mut view, "lead", "checklist_list", "{}").await;
    let lines: Vec<&str> = listed.lines().collect();
    assert!(lines[0].starts_with("#1 [!] Ship the release [waits on item worker#1] "), "{listed}");
    assert!(lines[1].starts_with("#2 [=] Tidy the docs [resumes "), "{listed}");
    assert!(lines[2].starts_with("#3 [!] Write the notes [waits on agent worker] "), "{listed}");
    let items = panel(&mut client, &mut view, "lead").await;
    assert_eq!(
        (items[0].blocked_kind(), items[0].blocked_ref.as_str(), items[0].blocked_on.as_str()),
        (v1::BlockedKind::Item, "worker#1", "Build the APK"),
        "an item wait shows the waited-on item's text"
    );
    assert_eq!((items[1].status(), items[1].retry_at_ms > 0), (v1::ChecklistStatus::Parked, true));
    assert_eq!(items[2].blocked_kind(), v1::BlockedKind::Agent);

    apply_request(&mut client, &mut view, select("worker")).await;
    assert_eq!(events(&view, "Lead is waiting on you").len(), 1, "told once");

    // Worker finishes its #1: Lead's opens again, and Lead is told.
    call(&mut client, &mut view, "worker", "checklist_update", r#"{"updates":[{"id":1,"status":"done","note":"built"}]}"#).await;
    let open = told(&mut client, &mut view, "lead", "#1 is open again").await;
    assert_eq!(
        open.text,
        "#1 \"Ship the release\" was waiting on Worker's #1 \"Build the APK\", which is done: it's open again. \
         Carry on with it, or mark it again if it still can't move."
    );
    let listed = call(&mut client, &mut view, "lead", "checklist_list", "{}").await;
    assert!(listed.starts_with("#1 [ ] Ship the release "), "{listed}");

    // A parked item whose time comes opens again, its owner told.
    call(&mut client, &mut view, "lead", "checklist_update", r#"{"updates":[{"id":2,"status":"parked","retry_at":"1s"}]}"#).await;
    let resumed = told(&mut client, &mut view, "lead", "#2 resumes").await;
    assert_eq!(resumed.text, "#2 \"Tidy the docs\" was parked until now: it's open again. Pick it up, or park it again.");
    let items = panel(&mut client, &mut view, "lead").await;
    assert_eq!((items[1].status(), items[1].retry_at_ms), (v1::ChecklistStatus::Open, 0));
}

#[tokio::test]
async fn a_wait_on_the_person_shows_in_needs_you_and_claktory_follows_up_as_set() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_nudge_after(std::time::Duration::from_millis(150));
    // A minute lasts 1ms: Settings' intervals below run in milliseconds.
    factory.set_nudge_minute(std::time::Duration::from_millis(1));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Lead", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Worker", "")).await;
    ack(apply_request(&mut client, &mut view, lead("worker", "lead")).await);
    ack(apply_request(&mut client, &mut view, checklist_on("worker")).await);

    // Settings → Checklist: the defaults, then only what's sent changes;
    // out of range is refused whole.
    let shown = |view: &View| view.workspace.settings.as_ref().and_then(|settings| settings.checklist).expect("checklist settings");
    assert_eq!(
        shown(&view),
        v1::ChecklistSettings {
            agent_blocked_minutes: 240,
            person_blocked_minutes: 1440,
            external_retry_minutes: 120,
            reask_minutes: 1440,
            overdue_minutes: 1440
        }
    );
    let refused = apply_request(&mut client, &mut view, intervals(v1::ChecklistSettings { reask_minutes: 5, ..Default::default() })).await;
    let Body::Error(error) = refused else { panic!("{refused:?}") };
    assert_eq!(error.message, "Choose from 15 minutes to 7 days.");
    // Long enough that nothing but what's tested comes due meanwhile.
    let quiet = v1::ChecklistSettings {
        agent_blocked_minutes: 10080,
        person_blocked_minutes: 10080,
        external_retry_minutes: 1000,
        reask_minutes: 600,
        overdue_minutes: 10080,
    };
    ack(apply_request(&mut client, &mut view, intervals(quiet)).await);
    view.follow(&mut client, |view| shown(view) == quiet).await;
    // Kept in factory.db: config.json gains nothing an older backend would refuse.
    let config = std::fs::read_to_string(dir.join("home/config.json")).unwrap_or_default();
    assert!(!config.contains("reask") && !config.contains("external_retry"), "{config}");

    // A decision waits on the person: it's in Needs you with no card.
    call(&mut client, &mut view, "worker", "checklist_add", r#"{"items":["Pick the logo","Renew the certificate","Install the build"]}"#)
        .await;
    let said = call(
        &mut client,
        &mut view,
        "worker",
        "checklist_update",
        r#"{"updates":[{"id":1,"status":"blocked","waits_on":"person_decision"}]}"#,
    )
    .await;
    assert!(said.contains("#1 blocked, waits on person decision"), "{said}");
    let waiting = |view: &View| view.agent("worker").waiting_items.clone();
    view.follow(&mut client, |view| waiting(view).len() == 1).await;
    let item = &waiting(&view)[0];
    assert_eq!(
        (item.item, item.text.as_str(), item.kind(), item.has_pending_ask),
        (1, "Pick the logo", v1::BlockedKind::PersonDecision, false)
    );

    // Asked about with item, the card covers it.
    let ask = r#"{"title":"Round or square?","body":"For the logo.","item":"1"}"#;
    let asked = call(&mut client, &mut view, "worker", "ask_person", ask).await;
    assert!(asked.starts_with("Asked the person"), "{asked}");
    view.follow(&mut client, |view| waiting(view).first().is_some_and(|item| item.has_pending_ask)).await;
    let refused = call(&mut client, &mut view, "worker", "ask_person", r#"{"title":"T","body":"B","item":"9"}"#).await;
    assert!(refused.contains("item: there's no #9 on worker's checklist. Nothing was asked."), "{refused}");

    // Dismissed: decide if it can; else Claktory has it ask again, and again.
    let seq = |view: &View| {
        view.entries
            .iter()
            .rev()
            .find_map(|entry| match &entry.kind {
                Some(Kind::AskPerson(ask)) if ask.state() == v1::AskPersonState::Pending => Some(entry.seq),
                _ => None,
            })
            .expect("a pending card")
    };
    let answer = |seq: u64, dismiss: bool, text: &str| {
        client_message::Body::AnswerAsk(v1::AnswerAsk { agent_id: "worker".into(), seq, dismiss, text: text.into(), ..Default::default() })
    };
    let card = seq(&view);
    ack(apply_request(&mut client, &mut view, answer(card, true, "")).await);
    let dismissed = told(&mut client, &mut view, "worker", "The person dismissed").await;
    assert!(
        dismissed.text.contains(
            "It was about #1 \"Pick the logo\", which still waits on the person. Decide it yourself if you can, and update #1; \
             note what you chose. If you truly can't move without them, leave #1 blocked and say why in a note: it stays in their \
             Needs you, and Claktory has you ask them again "
        ),
        "{}",
        dismissed.text
    );
    let item = waiting(&view)[0].clone();
    assert!(item.dismissed_ms > 0 && item.reask_at_ms > item.dismissed_ms && !item.has_pending_ask, "{item:?}");
    let again = told(&mut client, &mut view, "worker", "Ask the person again: #1").await;
    assert_eq!(
        again.text,
        "The person dismissed your question about #1 \"Pick the logo\", and it still waits on them. If you can decide it \
         yourself now, do, and update #1. If not, ask them again with ask_person (item: \"1\"), saying in a line why only \
         they can settle it."
    );

    // Asked again and answered: told to clear the block.
    call(&mut client, &mut view, "worker", "ask_person", ask).await;
    let card = seq(&view);
    ack(apply_request(&mut client, &mut view, answer(card, false, "")).await);
    let answered = told(&mut client, &mut view, "worker", "The person approved").await;
    assert!(
        answered.text.ends_with(
            "It was about #1 \"Pick the logo\", still blocked on the person. If this settles it, update #1 now; \
             if something is still missing, say what in a note."
        ),
        "{}",
        answered.text
    );
    // The answer ends what the dismissal set going: no re-ask on a question answered.
    view.follow(&mut client, |view| waiting(view).first().is_some_and(|item| item.dismissed_ms == 0 && item.reask_at_ms == 0)).await;
    call(&mut client, &mut view, "worker", "checklist_update", r#"{"updates":[{"id":1,"status":"done"}]}"#).await;
    view.follow(&mut client, |view| waiting(view).is_empty()).await;

    // Something outside, with no retry time: checked again after the
    // retry interval, then later (twice as long: `retry_minutes`).
    let outside = r#"{"updates":[{"id":2,"status":"blocked","waits_on":"external","blocked_on":"the CA's reply"}]}"#;
    let said = call(&mut client, &mut view, "worker", "checklist_update", outside).await;
    assert!(said.contains("#2 blocked, waits on external"), "{said}");
    let first = panel(&mut client, &mut view, "worker").await[1].retry_at_ms;
    assert!(first > 0);
    let check = told(&mut client, &mut view, "worker", "Check again: #2").await;
    assert!(
        check.text.starts_with("#2 \"Renew the certificate\" waits on the CA's reply, and it's time to check again."),
        "{}",
        check.text
    );
    let next = panel(&mut client, &mut view, "worker").await[1].retry_at_ms;
    assert!(next > first, "a next retry is set: {first} then {next}");
    // The same wait set again, with no words or time, keeps both.
    call(&mut client, &mut view, "worker", "checklist_update", r#"{"updates":[{"id":2,"status":"blocked","waits_on":"external"}]}"#).await;
    let kept = &panel(&mut client, &mut view, "worker").await[1];
    assert!(kept.retry_at_ms >= next, "the next check stays: {next} then {}", kept.retry_at_ms);
    assert_eq!(kept.blocked_on, "the CA's reply");
    call(&mut client, &mut view, "worker", "checklist_update", r#"{"updates":[{"id":2,"status":"dropped"}]}"#).await;

    // Something only the person can do: in Needs you, no reminder to ask.
    call(&mut client, &mut view, "worker", "checklist_update", r#"{"updates":[{"id":3,"status":"blocked","waits_on":"person_action"}]}"#)
        .await;
    view.follow(&mut client, |view| waiting(view).len() == 1).await;
    assert_eq!(waiting(&view)[0].kind(), v1::BlockedKind::PersonAction);

    // Still blocked? A wait on an agent is asked about after the agent
    // interval; a block past the overdue interval goes to the lead.
    call(&mut client, &mut view, "worker", "checklist_add", r#"{"items":["Merge the docs"]}"#).await;
    call(
        &mut client,
        &mut view,
        "worker",
        "checklist_update",
        r#"{"updates":[{"id":4,"status":"blocked","waits_on":"agent","ref":"lead"}]}"#,
    )
    .await;
    let soon = v1::ChecklistSettings { agent_blocked_minutes: 15, overdue_minutes: 15, ..Default::default() };
    ack(apply_request(&mut client, &mut view, intervals(soon)).await);
    let still = told(&mut client, &mut view, "worker", "Still blocked?").await;
    assert!(still.text.starts_with("Still blocked? #4 \"Merge the docs\", waiting on lead, blocked "), "{}", still.text);
    assert!(still.text.contains("It may not be any more: try it again."), "{}", still.text);
    let overdue = told(&mut client, &mut view, "lead", "Overdue: Worker's #").await;
    assert!(overdue.text.starts_with("Worker's #"), "{}", overdue.text);
    assert!(overdue.text.contains("See whether you can unblock it, or settle with Worker what happens next."), "{}", overdue.text);
    // Never a reminder to ask about the action.
    apply_request(&mut client, &mut view, select("worker")).await;
    let reminders: Vec<v1::Event> =
        events(&view, "Reminder:").into_iter().filter(|event| event.summary.ends_with("with nothing asked")).collect();
    assert!(reminders.iter().all(|event| !event.text.contains("#3")), "{reminders:?}");
}
