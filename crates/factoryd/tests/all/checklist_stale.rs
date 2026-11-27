//! The daily list of stale work: items left unchanged too long are listed
//! to their checklist's agent once a day, oldest first, whatever may wake
//! it, and those left three times as long to its lead too, with the agents
//! it leads that sit free.

use crate::common::{Client, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind, server_message::Body};

/// A minute, in these tests: a day is 28.8 s, longer than any test runs,
/// so only items aged by hand are stale.
const MINUTE: i64 = 20;
const DAY: i64 = 24 * 60 * MINUTE;

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

fn checklist(agent: &str, option: Option<&str>) -> client_message::Body {
    let change = match option {
        None => v1::ExtensionChange { id: "checklist".into(), enabled: Some(true), ..Default::default() },
        Some(choice) => v1::ExtensionChange {
            id: "checklist".into(),
            option_id: "stale_after".into(),
            value: Some(v1::OptionValue { choice: choice.into(), ..Default::default() }),
            ..Default::default()
        },
    };
    client_message::Body::UpdateAgent(v1::UpdateAgent { agent_id: agent.into(), extension: Some(change), ..Default::default() })
}

fn lead(agent: &str, lead: &str) -> client_message::Body {
    client_message::Body::UpdateAgent(v1::UpdateAgent { agent_id: agent.into(), lead_id: Some(lead.into()), ..Default::default() })
}

async fn call(client: &mut Client, view: &mut View, agent: &str, tool: &str, args: &str) -> String {
    apply_request(client, view, select(agent)).await;
    let text = format!("call {tool} {args}");
    for _ in 0..10 {
        let before = view.entries.len();
        apply_request(client, view, send_message(agent, &text)).await;
        let output = |view: &View| {
            view.entries.get(before..).unwrap_or_default().iter().find_map(|entry| match &entry.kind {
                Some(Kind::ToolCall(call)) if call.name == tool => Some(call.output.clone()),
                _ => None,
            })
        };
        let taken = |view: &View| {
            view.entries
                .get(before..)
                .unwrap_or_default()
                .iter()
                .any(|entry| matches!(&entry.kind, Some(Kind::User(user)) if user.text == text))
        };
        view.follow(client, |view| idle(view, agent) && (output(view).is_some() || taken(view))).await;
        if let Some(output) = output(view) {
            return output;
        }
    }
    panic!("{agent} never called {tool}")
}

fn stale(view: &View) -> Vec<v1::Event> {
    view.entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::Event(event)) if event.summary.starts_with("Stale: ") => Some(event.clone()),
            _ => None,
        })
        .collect()
}

/// Reads what comes for a while, long enough for a list to come if one would.
async fn quiet(client: &mut Client, view: &mut View) {
    let until = tokio::time::Instant::now() + std::time::Duration::from_millis(700);
    while let Ok(message) = tokio::time::timeout_at(until, client.receive()).await {
        if let Some(body) = &message.body {
            view.apply(body);
        }
    }
}

/// Sets when items were last changed, `ago` before now, as if they'd sat.
fn age(dir: &std::path::Path, agent: &str, items: &[(u32, i64)]) {
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("db");
    db.busy_timeout(std::time::Duration::from_secs(5)).expect("busy timeout");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_millis() as i64;
    for (id, ago) in items {
        db.execute("UPDATE checklist_items SET updated = ?3 WHERE agent = ?1 AND id = ?2", rusqlite::params![agent, id, now - ago])
            .expect("age");
    }
}

/// A new day for `agent`'s list.
fn next_day(dir: &std::path::Path, agent: &str) {
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("db");
    db.busy_timeout(std::time::Duration::from_secs(5)).expect("busy timeout");
    db.execute("DELETE FROM checklist_stale_nudges WHERE agent = ?1", [agent]).expect("next day");
}

#[tokio::test]
async fn stale_work_is_listed_once_a_day_oldest_first_whatever_may_wake_the_agent() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_nudge_after(std::time::Duration::from_millis(150));
    factory.set_nudge_minute(std::time::Duration::from_millis(MINUTE as u64));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Worker", "")).await;
    ack(apply_request(&mut client, &mut view, checklist("worker", None)).await);
    // A standing check: the idle reminder would hold back for less.
    let standing = r#"{"monitors":[{"name":"ci","prompt":"CI is green.","when":{"check":"false","every":"5m"}}]}"#;
    call(&mut client, &mut view, "worker", "monitors_add", standing).await;
    let items = r#"{"items":["Fix login","Write the docs","Renew the domain","Tidy the wiki","Wait for the vendor"]}"#;
    call(&mut client, &mut view, "worker", "checklist_add", items).await;
    let updates = r#"{"updates":[{"id":2,"owner":"worker"},{"id":3,"status":"parked","retry_at":"1d"},{"id":4,"status":"parked"},
        {"id":5,"status":"blocked","waits_on":"external","retry_at":"1d"}]}"#;
    call(&mut client, &mut view, "worker", "checklist_update", updates).await;
    quiet(&mut client, &mut view).await;
    assert!(stale(&view).is_empty(), "nothing has sat yet");

    // Open items past a day, oldest first; a dated parked item, an undated
    // one not yet three days old and a blocked one aren't listed.
    age(&dir, "worker", &[(1, 2 * DAY), (2, 3 * DAY), (3, 9 * DAY), (4, 2 * DAY), (5, 9 * DAY)]);
    view.follow(&mut client, |view| !stale(view).is_empty()).await;
    let first = stale(&view).remove(0);
    assert_eq!((first.from.as_str(), first.summary.as_str()), ("Claktory", "Stale: 2 items"));
    assert!(
        first.text.starts_with(
            "Stale work: 2 items on your checklist haven't changed in a while, oldest first: #2 Write the docs (open, 3 days"
        ),
        "{}",
        first.text
    );
    assert!(first.text.contains(", owner Worker); #1 Fix login (open, 2 days"), "{}", first.text);
    assert!(
        first.text.ends_with(
            ", no owner). For each, start it, hand it to someone, give it a date (parked with retry_at, or blocked with waits_on), \
             mark it done if it shipped, or drop it with a reason."
        ),
        "{}",
        first.text
    );
    for absent in ["#3", "#4", "#5", "Free now", "agents you lead"] {
        assert!(!first.text.contains(absent), "{absent}: {}", first.text);
    }
    quiet(&mut client, &mut view).await;
    assert_eq!(stale(&view).len(), 1, "once a day");

    // The next day the undated parked item has sat three days too.
    age(&dir, "worker", &[(4, 4 * DAY)]);
    next_day(&dir, "worker");
    view.follow(&mut client, |view| stale(view).len() == 2).await;
    let second = stale(&view).remove(1);
    assert_eq!(second.summary, "Stale: 3 items");
    assert!(second.text.contains("oldest first: #4 Tidy the wiki (parked, 4 days"), "{}", second.text);
    assert!(second.text.find("#4 ").unwrap() < second.text.find("#2 ").unwrap(), "{}", second.text);

    // Longer before an item counts, or off.
    ack(apply_request(&mut client, &mut view, checklist("worker", Some("1w"))).await);
    next_day(&dir, "worker");
    quiet(&mut client, &mut view).await;
    assert_eq!(stale(&view).len(), 2, "nothing has sat a week");
    age(&dir, "worker", &[(1, 8 * DAY)]);
    view.follow(&mut client, |view| stale(view).len() == 3).await;
    assert_eq!(stale(&view)[2].summary, "Stale: 1 item");
    ack(apply_request(&mut client, &mut view, checklist("worker", Some("off"))).await);
    next_day(&dir, "worker");
    quiet(&mut client, &mut view).await;
    assert_eq!(stale(&view).len(), 3, "off");
}

#[tokio::test]
async fn work_left_three_times_as_long_reaches_the_lead_with_the_agents_free() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_nudge_after(std::time::Duration::from_millis(150));
    factory.set_nudge_minute(std::time::Duration::from_millis(MINUTE as u64));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    for name in ["Lead", "Worker", "Spare", "Solo"] {
        apply_request(&mut client, &mut view, create_agent("site", name, "")).await;
    }
    ack(apply_request(&mut client, &mut view, lead("worker", "lead")).await);
    ack(apply_request(&mut client, &mut view, lead("spare", "lead")).await);
    ack(apply_request(&mut client, &mut view, checklist("worker", None)).await);
    ack(apply_request(&mut client, &mut view, checklist("solo", None)).await);
    call(&mut client, &mut view, "worker", "checklist_add", r#"{"items":["Fix login","Write the docs"]}"#).await;
    call(&mut client, &mut view, "solo", "checklist_add", r#"{"items":["Ship it"]}"#).await;

    // Worker's #1 has sat four days, #2 two; Solo's one, five, with no lead.
    age(&dir, "worker", &[(1, 4 * DAY), (2, 2 * DAY)]);
    age(&dir, "solo", &[(1, 5 * DAY)]);
    apply_request(&mut client, &mut view, select("lead")).await;
    view.follow(&mut client, |view| !stale(view).is_empty()).await;
    let told = stale(&view).remove(0);
    assert_eq!(told.summary, "Stale: 1 on your team");
    assert!(
        told.text.starts_with(
            "On the checklists of agents you lead, 1 item untouched for longer still: Worker's #1 Fix login (open, 4 days"
        ),
        "{}",
        told.text
    );
    assert!(
        told.text.ends_with(", no owner). Settle with each agent what happens next.\n\nFree now, idle with nothing open: Spare."),
        "{}",
        told.text
    );

    // Worker hears of both; Solo of its own, and no one else of it.
    apply_request(&mut client, &mut view, select("worker")).await;
    view.follow(&mut client, |view| !stale(view).is_empty()).await;
    assert_eq!(stale(&view)[0].summary, "Stale: 2 items");
    apply_request(&mut client, &mut view, select("solo")).await;
    view.follow(&mut client, |view| !stale(view).is_empty()).await;
    assert_eq!(stale(&view)[0].summary, "Stale: 1 item");
    for agent in ["rivet", "spare"] {
        apply_request(&mut client, &mut view, select(agent)).await;
        quiet(&mut client, &mut view).await;
        assert!(stale(&view).is_empty(), "{agent}");
    }
    apply_request(&mut client, &mut view, select("lead")).await;
    quiet(&mut client, &mut view).await;
    assert_eq!(stale(&view).len(), 1, "once a day for the lead too");
}
