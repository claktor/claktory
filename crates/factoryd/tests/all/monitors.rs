//! Monitors: prompts that wake an agent later, at a time, on a schedule, or
//! when a command, files or a running command's output change. The tests move
//! the monitors' clock forward rather than wait.

use std::time::Duration;

use crate::common::{Client, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind, server_message::Body};

fn select(agent: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

/// Has `agent` call `tool` with `args` and returns what the tool answered.
async fn call(client: &mut Client, view: &mut View, agent: &str, tool: &str, args: &str) -> String {
    apply_request(client, view, select(agent)).await;
    let before = view.entries.len();
    apply_request(client, view, send_message(agent, &format!("call {tool} {args}"))).await;
    view.follow(client, |view| view.entries.len() >= before + 3 && idle(view, agent)).await;
    view.entries[before..]
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) => Some(call.output.clone()),
            _ => None,
        })
        .expect("a tool call")
}

/// The events the selected agent got from its monitors (and Factory's notes about them).
fn fires(view: &View) -> Vec<v1::Event> {
    view.entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::Event(event)) if event.summary.starts_with('m') => Some(event.clone()),
            _ => None,
        })
        .collect()
}

/// Lets the scheduler run a few times, applying what the client hears meanwhile.
async fn settle(client: &mut Client, view: &mut View, for_ms: u64) {
    let until = tokio::time::Instant::now() + Duration::from_millis(for_ms);
    while let Ok(message) = tokio::time::timeout_at(until, client.receive()).await {
        if let Some(body) = &message.body {
            view.apply(body);
        }
    }
}

async fn setup() -> (std::path::PathBuf, std::sync::Arc<factoryd::Factory>, Client, View) {
    let (dir, factory, client, view, _) = setup_at().await;
    (dir, factory, client, view)
}

/// `setup`, and where the Factory listens.
async fn setup_at() -> (std::path::PathBuf, std::sync::Arc<factoryd::Factory>, Client, View, std::net::SocketAddr) {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_monitor_tick(Duration::from_millis(20));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Lead", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Worker", "")).await;
    apply_request(&mut client, &mut view, select("lead")).await;
    (dir, factory, client, view, addr)
}

fn running(pattern: &str) -> bool {
    std::process::Command::new("pgrep").args(["-f", pattern]).output().is_ok_and(|output| output.status.success())
}

#[tokio::test]
async fn monitors_are_added_all_or_none_listed_and_changed() {
    let (dir, _factory, mut client, mut view) = setup().await;
    apply_request(&mut client, &mut view, create_project("Lab", &dir.join("lab"))).await;
    apply_request(&mut client, &mut view, create_agent("lab", "Scout", "")).await;

    let added = call(
        &mut client,
        &mut view,
        "lead",
        "monitors_add",
        r#"{"monitors":[{"name":"daily review","prompt":"Review the day.","when":{"every":"24h"}},{"name":"deploy","prompt":"Say if it deployed.","when":{"check":"false","every":"5m"}}]}"#,
    )
    .await;
    let lines: Vec<&str> = added.lines().collect();
    assert!(lines[0].starts_with("Added m1 \"daily review\": every 1d; first fire tomorrow "), "{added}");
    assert_eq!(lines[1], "Added m2 \"deploy\": check `false` every 5m, fires on success; first check now.");

    // Errors teach, and a batch with one bad monitor adds none.
    let short =
        call(&mut client, &mut view, "lead", "monitors_add", r#"{"monitors":[{"name":"x","prompt":"y","when":{"every":"20s"}}]}"#).await;
    assert!(short.contains("every must be at least 1m; got 20s. For faster reactions use watch or files."), "{short}");
    let batch = r#"{"monitors":[{"name":"ok","prompt":"y","when":{"in":"2h"}},{"name":"bad","prompt":"y","when":{"cron":"0 9 * *"}}]}"#;
    let refused = call(&mut client, &mut view, "lead", "monitors_add", batch).await;
    assert!(refused.contains("monitor 2: cron takes 5 fields") && refused.contains("Nothing was added."), "{refused}");

    let list = call(&mut client, &mut view, "lead", "monitors_list", "{}").await;
    let lines: Vec<&str> = list.lines().collect();
    assert_eq!(lines.len(), 2, "{list}");
    assert!(lines[0].starts_with("m1 daily review — every 1d · next tomorrow ") && lines[0].ends_with(" · fired 0"), "{list}");
    assert!(lines[1].starts_with("m2 deploy — check `false` every 5m, fires on success · next "), "{list}");

    let changed = call(
        &mut client,
        &mut view,
        "lead",
        "monitors_change",
        r#"{"changes":[{"id":"m2","action":"pause"},{"id":"m1","action":"cancel"}]}"#,
    )
    .await;
    assert_eq!(changed, "m2 paused; m1 cancelled.");
    let again = call(&mut client, &mut view, "lead", "monitors_change", r#"{"changes":[{"id":"m1","action":"pause"}]}"#).await;
    assert!(again.contains("m1 is cancelled, so it can't be paused. Nothing changed."), "{again}");
    let list = call(&mut client, &mut view, "lead", "monitors_list", "{}").await;
    assert!(list.starts_with("m2 deploy — ") && list.contains(" · paused: by lead"), "{list}");
    assert!(
        call(&mut client, &mut view, "lead", "monitors_list", r#"{"all":true}"#).await.contains("m1 daily review — every 1d · cancelled")
    );

    // For agents it can message, and only those.
    let for_worker = r#"{"monitors":[{"name":"standup","prompt":"Post your status.","when":{"cron":"0 9 * * 1-5"},"agent":"worker"}]}"#;
    assert!(
        call(&mut client, &mut view, "lead", "monitors_add", for_worker)
            .await
            .starts_with("Added m1 for worker \"standup\": cron 0 9 * * 1-5")
    );
    let for_scout = r#"{"monitors":[{"name":"x","prompt":"y","when":{"every":"1h"},"agent":"scout"}]}"#;
    let refused = call(&mut client, &mut view, "lead", "monitors_add", for_scout).await;
    assert!(refused.contains("No agent you can message is called \"scout\"; you can message: worker."), "{refused}");
    assert!(call(&mut client, &mut view, "worker", "monitors_list", "{}").await.contains(" · set by lead"));
}

#[tokio::test]
async fn a_time_monitor_fires_with_its_prompt_and_waits_while_the_agent_works() {
    let (_dir, factory, mut client, mut view) = setup().await;
    let add = r#"{"monitors":[{"name":"hourly","prompt":"Check the queue.","when":{"every":"1h"}}]}"#;
    call(&mut client, &mut view, "lead", "monitors_add", add).await;
    factory.advance_monitor_clock(Duration::from_secs(3600));
    view.follow(&mut client, |view| fires(view).len() == 1 && idle(view, "lead")).await;
    let fire = &fires(&view)[0];
    assert_eq!(fire.summary, "m1 hourly fired");
    assert_eq!((fire.source(), fire.from.as_str(), fire.from_id.as_str()), (v1::EventSource::Monitor, "hourly", "m1"));
    assert_eq!(fire.text, "Monitor m1 \"hourly\" fired (every 1h; fire 1).\n\nYour prompt: Check the queue.");
    // The model reads it as an event from the monitor.
    let read = view.entries.iter().rev().find_map(|entry| match &entry.kind {
        Some(Kind::Assistant(assistant)) => Some(assistant.text.clone()),
        _ => None,
    });
    assert!(read.expect("a reply").starts_with("echo: <event source=\"monitor\" from=\"hourly\" id=\"m1\">"));

    // Working: the next fire waits until it's done.
    apply_request(&mut client, &mut view, send_message("lead", "hang")).await;
    factory.advance_monitor_clock(Duration::from_secs(3600));
    settle(&mut client, &mut view, 300).await;
    assert_eq!(fires(&view).len(), 1);
    let stop = client_message::Body::StopAgent(v1::StopAgent { agent_id: "lead".into() });
    apply_request(&mut client, &mut view, stop).await;
    view.follow(&mut client, |view| fires(view).len() == 2).await;
}

#[tokio::test]
async fn checks_fire_when_their_condition_becomes_true() {
    let (dir, factory, mut client, mut view) = setup().await;
    let site = dir.join("site");
    let add = r#"{"monitors":[
        {"name":"flag","prompt":"The flag is up.","when":{"check":"test -f flag","every":"30s"}},
        {"name":"version","prompt":"The version changed.","when":{"check":"cat version","every":"30s","fire_on":"changed","repeat":true}},
        {"name":"since","prompt":"Report.","when":{"check":"echo since $FACTORY_MONITOR_LAST_FIRED","every":"30s","repeat":"always"}}]}"#;
    std::fs::write(site.join("version"), "1").expect("version");
    call(&mut client, &mut view, "lead", "monitors_add", &add.replace('\n', "")).await;
    // "since" holds at once; the others don't yet.
    view.follow(&mut client, |view| fires(view).len() == 1 && idle(view, "lead")).await;
    assert!(fires(&view)[0].text.starts_with("Monitor m3 \"since\" fired (check succeeded; fire 1).\nOutput (end):\nsince "));
    // The others have run their first check too (m2's output is its baseline)
    // before anything changes: each is then due again in 30s.
    let open = v1::OpenPanel { extension_id: "monitors".into(), agent_id: "lead".into() };
    apply_request(&mut client, &mut view, client_message::Body::OpenPanel(open)).await;
    let checked = |view: &View| match view.extension_view.as_ref().and_then(|view| view.body.as_ref()) {
        Some(v1::extension_view::Body::Monitors(monitors)) => {
            monitors.monitors.iter().filter(|monitor| monitor.id != 3).all(|monitor| monitor.next_due > factory_now() + 15_000)
        }
        _ => false,
    };
    view.follow(&mut client, checked).await;

    std::fs::write(site.join("flag"), "").expect("flag");
    std::fs::write(site.join("version"), "2").expect("version");
    factory.advance_monitor_clock(Duration::from_secs(30));
    view.follow(&mut client, |view| fires(view).len() == 4 && idle(view, "lead")).await;
    let texts: Vec<String> = fires(&view).iter().map(|fire| fire.text.clone()).collect();
    assert!(texts.iter().any(|text| text.starts_with("Monitor m1 \"flag\" fired (check succeeded; fire 1, now done).")), "{texts:?}");
    assert!(texts.iter().any(|text| text.starts_with("Monitor m2 \"version\" fired (check's output changed; fire 1).\nOutput (end):\n2")));
    assert!(texts.iter().any(|text| text.starts_with("Monitor m3 \"since\" fired (check succeeded; fire 2).")));

    // Nothing changed: only "always" fires again; "flag" is done.
    factory.advance_monitor_clock(Duration::from_secs(30));
    view.follow(&mut client, |view| fires(view).len() == 5 && idle(view, "lead")).await;
    settle(&mut client, &mut view, 300).await;
    assert_eq!(fires(&view).len(), 5);
    let list = call(&mut client, &mut view, "lead", "monitors_list", r#"{"all":true}"#).await;
    assert!(list.contains("m1 flag — check `test -f flag` every 30s, fires on success · done: it fired · fired 1"), "{list}");
}

#[tokio::test]
async fn files_and_watches_fire_on_what_they_see() {
    let (dir, factory, mut client, mut view) = setup().await;
    let site = dir.join("site");
    std::fs::create_dir_all(site.join("docs")).expect("docs");
    let add = r#"{"monitors":[
        {"name":"docs","prompt":"Read the new docs.","when":{"files":["docs/"],"glob":"*.md"}},
        {"name":"log","prompt":"Look at the errors.","when":{"watch":"echo ok; echo ERROR one; echo ERROR two; sleep 71.25","match":"ERROR"}}]}"#;
    call(&mut client, &mut view, "lead", "monitors_add", &add.replace('\n', "")).await;
    std::fs::write(site.join("docs/guide.md"), "hi").expect("guide");
    std::fs::write(site.join("docs/skip.txt"), "not watched").expect("skip");
    for _ in 0..4 {
        settle(&mut client, &mut view, 150).await;
        factory.advance_monitor_clock(Duration::from_secs(2));
    }
    view.follow(&mut client, |view| fires(view).len() == 2 && idle(view, "lead")).await;
    let texts: Vec<String> = fires(&view).iter().map(|fire| fire.text.clone()).collect();
    let created = format!("Monitor m1 \"docs\" fired (files changed; fire 1).\nCreated: {}", site.join("docs/guide.md").display());
    assert!(texts.iter().any(|text| text.starts_with(&created) && !text.contains("skip.txt")), "{texts:?}");
    assert!(
        texts
            .iter()
            .any(|text| text
                .starts_with("Monitor m2 \"log\" fired (2 new lines matching \"ERROR\"; fire 1).\nERROR one\nERROR two\nFull log: ")),
        "{texts:?}"
    );
    assert!(running("sleep 71.25"));
    call(&mut client, &mut view, "lead", "monitors_change", r#"{"changes":[{"id":"m2","action":"cancel"}]}"#).await;
    // Stopping a process group is quick, but under load not always within a fixed wait.
    for _ in 0..50 {
        if !running("sleep 71.25") {
            break;
        }
        settle(&mut client, &mut view, 200).await;
    }
    assert!(!running("sleep 71.25"), "cancelling stops its process");
}

#[tokio::test]
async fn a_busy_agent_gets_one_fire_with_what_happened_meanwhile() {
    let (_dir, factory, mut client, mut view) = setup().await;
    let add = r#"{"monitors":[{"name":"tick","prompt":"Tick.","when":{"every":"1m"},"idle_only":false}]}"#;
    call(&mut client, &mut view, "lead", "monitors_add", add).await;
    apply_request(&mut client, &mut view, send_message("lead", "hang")).await;
    for _ in 0..3 {
        factory.advance_monitor_clock(Duration::from_secs(60));
        settle(&mut client, &mut view, 150).await;
    }
    // One fire waits for it; the other two are carried.
    assert_eq!(view.agent("lead").queued_messages, 1);
    // Stopped, it still gets the waiting fire, in a new run; once that's
    // read, what was carried comes as the next.
    let stop = client_message::Body::StopAgent(v1::StopAgent { agent_id: "lead".into() });
    apply_request(&mut client, &mut view, stop).await;
    view.follow(&mut client, |view| fires(view).len() == 2 && idle(view, "lead")).await;
    assert_eq!(fires(&view)[0].text, "Monitor m1 \"tick\" fired (every 1m; fire 1).\n\nYour prompt: Tick.");
    assert_eq!(
        fires(&view)[1].text,
        "Monitor m1 \"tick\" fired (while your last fire waited; fire 2).\nevery 1m\nevery 1m\n\nYour prompt: Tick."
    );
}

#[tokio::test]
async fn an_agents_monitors_firing_too_often_are_paused_with_one_notice() {
    let (_dir, _factory, mut client, mut view) = setup().await;
    call(&mut client, &mut view, "lead", "monitors_add", r#"{"monitors":[{"name":"noisy","prompt":"Again.","when":{"every":"1h"}}]}"#)
        .await;
    for _ in 0..30 {
        assert_eq!(
            call(&mut client, &mut view, "lead", "monitors_change", r#"{"changes":[{"id":"m1","action":"fire"}]}"#).await,
            "m1 fired."
        );
        view.follow(&mut client, |view| idle(view, "lead")).await;
    }
    call(&mut client, &mut view, "lead", "monitors_change", r#"{"changes":[{"id":"m1","action":"fire"}]}"#).await;
    view.follow(&mut client, |view| fires(view).iter().any(|fire| fire.summary == "m1 noisy paused") && idle(view, "lead")).await;
    let notices: Vec<v1::Event> = fires(&view).into_iter().filter(|fire| fire.summary == "m1 noisy paused").collect();
    assert_eq!(notices.len(), 1);
    assert_eq!(
        notices[0].text,
        "Monitor m1 \"noisy\" paused: your monitors fired 30 times in the last hour. Resume it with monitors_change when it's fixed."
    );
    assert!(call(&mut client, &mut view, "lead", "monitors_list", "{}").await.contains(" · paused: your monitors fired 30 times"));
}

#[tokio::test]
async fn a_deleted_agent_takes_its_monitors_and_their_processes_with_it() {
    let (_dir, _factory, mut client, mut view) = setup().await;
    let add = r#"{"monitors":[{"name":"log","prompt":"Look.","when":{"watch":"sleep 72.75"}}]}"#;
    call(&mut client, &mut view, "worker", "monitors_add", add).await;
    for _ in 0..50 {
        if running("sleep 72.75") {
            break;
        }
        settle(&mut client, &mut view, 100).await;
    }
    assert!(running("sleep 72.75"));
    let delete = client_message::Body::DeleteAgent(v1::DeleteAgent { agent_id: "worker".into() });
    assert!(matches!(apply_request(&mut client, &mut view, delete).await, Body::Ack(_)));
    settle(&mut client, &mut view, 800).await;
    assert!(!running("sleep 72.75"));
}

#[tokio::test]
async fn an_agent_a_monitor_will_wake_is_not_reminded_of_its_checklist() {
    let (_dir, factory, mut client, mut view) = setup().await;
    factory.set_nudge_after(Duration::from_millis(150));
    let on = v1::ExtensionChange { id: "checklist".into(), enabled: Some(true), ..Default::default() };
    let turn_on = client_message::Body::UpdateAgent(v1::UpdateAgent { agent_id: "lead".into(), extension: Some(on), ..Default::default() });
    apply_request(&mut client, &mut view, turn_on).await;
    call(
        &mut client,
        &mut view,
        "lead",
        "monitors_add",
        r#"{"monitors":[{"name":"ci","prompt":"CI is green.","when":{"check":"false","every":"5m"}}]}"#,
    )
    .await;
    call(&mut client, &mut view, "lead", "checklist_add", r#"{"items":["Ship it"]}"#).await;
    let reminders = |view: &View| {
        view.entries.iter().filter(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.text.starts_with("You have"))).count()
    };
    settle(&mut client, &mut view, 700).await;
    assert_eq!(reminders(&view), 0, "the check will wake it");
    call(&mut client, &mut view, "lead", "monitors_change", r#"{"changes":[{"id":"m1","action":"cancel"}]}"#).await;
    view.follow(&mut client, |view| reminders(view) == 1).await;
}

#[tokio::test]
async fn the_panel_shows_an_agents_monitors_and_the_person_pauses_resumes_and_cancels_them() {
    let (_dir, factory, mut client, mut view, addr) = setup_at().await;
    let open = v1::OpenPanel { extension_id: "monitors".into(), agent_id: "lead".into() };
    apply_request(&mut client, &mut view, client_message::Body::OpenPanel(open)).await;
    let panel = |view: &View| match view.extension_view.as_ref().and_then(|view| view.body.clone()) {
        Some(v1::extension_view::Body::Monitors(monitors)) => monitors,
        other => panic!("expected the Monitors panel, got {other:?}"),
    };
    assert!(panel(&view).monitors.is_empty());

    let add = r#"{"monitors":[{"name":"hourly","prompt":"Check the queue.","when":{"every":"1h"},"max_fires":3}]}"#;
    call(&mut client, &mut view, "lead", "monitors_add", add).await;
    view.follow(&mut client, |view| panel(view).monitors.len() == 1).await;
    let monitor = panel(&view).monitors[0].clone();
    let now = factory_now();
    assert_eq!((monitor.id, monitor.name.as_str(), monitor.prompt.as_str()), (1, "hourly", "Check the queue."));
    assert_eq!((monitor.when.as_str(), monitor.kind.as_str()), ("every 1h", "every"));
    assert_eq!((monitor.state(), monitor.fires, monitor.max_fires, monitor.idle_only), (v1::MonitorState::Active, 0, 3, true));
    assert_eq!((monitor.created_by.as_str(), monitor.last_fired, monitor.until), ("lead", 0, 0));
    assert!((monitor.next_due - now - 3_600_000).abs() < 60_000, "{}", monitor.next_due - now);

    // A client that connects now gets the open panel with everything else.
    let (mut other, _, _) = Client::connect(addr).await;
    let shown = other
        .wait_for(|body| match body {
            Body::ExtensionView(view) => Some(view.clone()),
            _ => None,
        })
        .await;
    match shown.body {
        Some(v1::extension_view::Body::Monitors(monitors)) => assert_eq!(monitors.monitors.len(), 1),
        other => panic!("expected the Monitors panel, got {other:?}"),
    }
    // Fires show as they happen.
    factory.advance_monitor_clock(Duration::from_secs(3600));
    view.follow(&mut client, |view| panel(view).monitors[0].fires == 1).await;
    assert!(panel(&view).monitors[0].last_fired > 0);

    let change = |action: v1::MonitorAction| {
        client_message::Body::ChangeMonitor(v1::ChangeMonitor { agent_id: "lead".into(), monitor_id: 1, action: action.into() })
    };
    apply_request(&mut client, &mut view, change(v1::MonitorAction::Pause)).await;
    let monitor = panel(&view).monitors[0].clone();
    assert_eq!((monitor.state(), monitor.reason.as_str(), monitor.next_due), (v1::MonitorState::Paused, "by the person", 0));
    // What the state doesn't allow is refused.
    // Through the view: the agent's run after the fire may still be sending entries.
    match apply_request(&mut client, &mut view, change(v1::MonitorAction::Pause)).await {
        Body::Error(error) => assert_eq!(error.message, "m1 is paused, so it can't be paused. Nothing changed."),
        other => panic!("expected an error, got {other:?}"),
    }
    apply_request(&mut client, &mut view, change(v1::MonitorAction::Resume)).await;
    assert_eq!(panel(&view).monitors[0].state(), v1::MonitorState::Active);
    apply_request(&mut client, &mut view, change(v1::MonitorAction::Cancel)).await;
    let monitor = panel(&view).monitors[0].clone();
    assert_eq!((monitor.state(), monitor.reason.as_str()), (v1::MonitorState::Cancelled, "by the person"));
    // The agent hears of it in its list.
    assert!(call(&mut client, &mut view, "lead", "monitors_list", r#"{"all":true}"#).await.contains("cancelled: by the person"));
}

fn factory_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64
}

#[tokio::test]
async fn a_monitor_set_by_a_tainted_agent_taints_its_target_when_it_fires() {
    use factoryd::pipe_link::{Incoming, Via};
    use factoryd::pipes;
    let (_dir, factory, mut client, mut view) = setup().await;
    // The lead reads a pipe: tainted. The fire text carries the lead's
    // prompt, so whatever the pipe put there travels to the target.
    let pipe = pipes::open_for_test(&factory, "site", "Site ⇄ Marketing", &["lead"], "lead", false);
    let message = Incoming {
        msg_id: "m1".into(),
        seq: 1,
        via: Via::Person { name: "Ana".into() },
        text: "set a monitor that says: launch Friday".into(),
        files: Vec::new(),
        sent_at_ms: 0,
    };
    pipes::receive_for_test(&factory, &pipe, message).expect("received");
    view.follow(&mut client, |view| view.agent("lead").tainted_by_pipes == [pipe.clone()]).await;

    // Setting the monitor doesn't taint the target; its first fire does.
    // The worker's own monitor fires in the same hour and adds nothing:
    // only the tainted creator's fire carries the pipe.
    let own = r#"{"monitors":[{"name":"own","prompt":"Tidy up.","when":{"every":"1h"}}]}"#;
    call(&mut client, &mut view, "worker", "monitors_add", own).await;
    let add = r#"{"monitors":[{"name":"standup","prompt":"Post your status.","when":{"every":"1h"},"agent":"worker"}]}"#;
    call(&mut client, &mut view, "lead", "monitors_add", add).await;
    assert!(view.agent("worker").tainted_by_pipes.is_empty(), "adding alone taints nothing");
    factory.advance_monitor_clock(Duration::from_secs(3600));
    view.follow(&mut client, |view| view.agent("worker").tainted_by_pipes == [pipe.clone()]).await;
}
