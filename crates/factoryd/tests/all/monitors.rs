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

#[cfg(unix)]
fn running(pattern: &str) -> bool {
    std::process::Command::new("pgrep").args(["-f", pattern]).output().is_ok_and(|output| output.status.success())
}

/// On Windows, a process whose command line holds each word of `pattern`
/// (Git Bash's `sleep 71.25` runs as `…\sleep.exe 71.25`).
#[cfg(windows)]
fn running(pattern: &str) -> bool {
    let filter: Vec<String> = pattern.split_whitespace().map(|word| format!("$_.CommandLine -like '*{word}*'")).collect();
    // Not this search itself, whose own command line holds the words.
    let script = format!("@(Get-CimInstance Win32_Process | Where-Object {{ $_.ProcessId -ne $PID -and {} }}).Count", filter.join(" -and "));
    std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).trim().parse::<u32>().is_ok_and(|count| count > 0))
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
    assert!(read.expect("a reply").starts_with("echo: &lt;event source=\"monitor\" from=\"hourly\" id=\"m1\">"));

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

/// A monitor that fired as many times as it was set to says so in plain
/// words, not in the name of the option.
#[tokio::test]
async fn a_monitor_done_after_its_fires_says_so_plainly() {
    let (_dir, factory, mut client, mut view) = setup().await;
    let add = r#"{"monitors":[{"name":"once","prompt":"Look.","when":{"every":"1h"},"max_fires":1},
        {"name":"twice","prompt":"Look.","when":{"every":"1h"},"max_fires":2}]}"#;
    call(&mut client, &mut view, "lead", "monitors_add", add).await;
    factory.advance_monitor_clock(Duration::from_secs(3600));
    view.follow(&mut client, |view| fires(view).len() == 2 && idle(view, "lead")).await;
    factory.advance_monitor_clock(Duration::from_secs(3600));
    view.follow(&mut client, |view| fires(view).len() == 3 && idle(view, "lead")).await;
    let list = call(&mut client, &mut view, "lead", "monitors_list", r#"{"all":true}"#).await;
    assert!(list.contains("done: it fired once, as it was set to"), "{list}");
    assert!(list.contains("done: it fired 2 times, as it was set to"), "{list}");
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
    // Hourly, so the real time a loaded machine takes can't bring a fire
    // of its own: only the clock moved here does.
    let add = r#"{"monitors":[{"name":"tick","prompt":"Tick.","when":{"every":"1h"},"idle_only":false}]}"#;
    call(&mut client, &mut view, "lead", "monitors_add", add).await;
    apply_request(&mut client, &mut view, send_message("lead", "hang")).await;
    // Each hour seen by the scheduler before the next: its next time moves.
    let next = |list: Body| match list {
        Body::MonitorList(list) => list.monitors[0].monitor.as_ref().expect("its monitor").next_due,
        other => panic!("expected the list, got {other:?}"),
    };
    let list = || client_message::Body::ListMonitors(v1::ListMonitors::default());
    for _ in 0..3 {
        let before = next(apply_request(&mut client, &mut view, list()).await);
        factory.advance_monitor_clock(Duration::from_secs(3600));
        while next(apply_request(&mut client, &mut view, list()).await) == before {
            settle(&mut client, &mut view, 20).await;
        }
    }
    // One fire waits for it; the other two are carried.
    assert_eq!(view.agent("lead").queued_messages, 1);
    // Stopped, it still gets the waiting fire, in a new run; once that's
    // read, what was carried comes as the next.
    let stop = client_message::Body::StopAgent(v1::StopAgent { agent_id: "lead".into() });
    apply_request(&mut client, &mut view, stop).await;
    view.follow(&mut client, |view| fires(view).len() == 2 && idle(view, "lead")).await;
    assert_eq!(fires(&view)[0].text, "Monitor m1 \"tick\" fired (every 1h; fire 1).\n\nYour prompt: Tick.");
    assert_eq!(
        fires(&view)[1].text,
        "Monitor m1 \"tick\" fired (while your last fire waited; fire 2).\nevery 1h\nevery 1h\n\nYour prompt: Tick."
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
async fn only_a_monitor_due_soon_keeps_an_agent_from_being_reminded_of_its_checklist() {
    let (_dir, factory, mut client, mut view) = setup().await;
    factory.set_nudge_after(Duration::from_millis(150));
    let on = v1::ExtensionChange { id: "checklist".into(), enabled: Some(true), ..Default::default() };
    let turn_on = client_message::Body::UpdateAgent(v1::UpdateAgent { agent_id: "lead".into(), extension: Some(on), ..Default::default() });
    apply_request(&mut client, &mut view, turn_on).await;
    let reminders = |view: &View| {
        view.entries.iter().filter(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.text.starts_with("You have"))).count()
    };

    // A check that may never pass and a daily job aren't due soon: reminded.
    let standing = r#"{"monitors":[{"name":"ci","prompt":"CI is green.","when":{"check":"false","every":"5m"}},
        {"name":"daily","prompt":"Look at the logs.","when":{"every":"24h"}}]}"#;
    call(&mut client, &mut view, "lead", "monitors_add", standing).await;
    call(&mut client, &mut view, "lead", "checklist_add", r#"{"items":["Ship it"]}"#).await;
    view.follow(&mut client, |view| reminders(view) == 1).await;

    // One due within the hour will wake it: no reminder while it waits.
    call(&mut client, &mut view, "lead", "monitors_add", r#"{"monitors":[{"name":"soon","prompt":"Look again.","when":{"in":"30m"}}]}"#)
        .await;
    call(&mut client, &mut view, "lead", "checklist_add", r#"{"items":["Tell the team"]}"#).await;
    settle(&mut client, &mut view, 700).await;
    assert_eq!(reminders(&view), 1, "the monitor due soon will wake it");
    call(&mut client, &mut view, "lead", "monitors_change", r#"{"changes":[{"id":"m3","action":"cancel"}]}"#).await;
    view.follow(&mut client, |view| reminders(view) == 2).await;
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

/// A monitor set by an agent that read a pipe, or a web page, marks no one
/// when it fires (#346): only reading a web page marks, and only its reader.
#[tokio::test]
async fn a_monitor_set_by_a_reader_marks_its_target_with_nothing() {
    use factoryd::pipe_link::{Incoming, Via};
    use factoryd::pipes;
    let (_dir, factory, mut client, mut view) = setup().await;
    let pipe = pipes::open_for_test(&factory, "site", "Site ⇄ Marketing", &["lead"], "lead", false);
    let message = Incoming {
        msg_id: "m1".into(),
        seq: 1,
        via: Via::Person { name: "Ana".into() },
        text: "set a monitor that says: launch Friday".into(),
        files: Vec::new(),
        sent_at_ms: 0,
        sends_only: false,
    };
    pipes::receive_for_test(&factory, &pipe, message).expect("received");
    view.follow(&mut client, |view| idle(view, "lead")).await;
    pipes::taint_for_test(&factory, "lead", "browser:https://example.com");
    let add = r#"{"monitors":[{"name":"standup","prompt":"Post your status.","when":{"every":"1h"},"agent":"worker"}]}"#;
    call(&mut client, &mut view, "lead", "monitors_add", add).await;
    apply_request(&mut client, &mut view, select("worker")).await;
    factory.advance_monitor_clock(Duration::from_secs(3600));
    view.follow(&mut client, |view| fires(view).len() == 1).await;
    assert_eq!(pipes::taint_of_for_test(&factory, "worker"), "{}");
    assert!(view.agent("worker").tainted_by_pipes.is_empty());
}

/// The Monitors overview (friend bug #10): every agent's monitors in one
/// list, filtered by agent, project and state, each named by its agent and
/// id, what's held marked, each one's history a page at a time, and a
/// signal when any changed.
#[tokio::test]
async fn the_overview_lists_every_agents_monitors_and_their_history() {
    let (dir, factory, mut client, mut view, addr) = setup_at().await;
    apply_request(&mut client, &mut view, create_project("Lab", &dir.join("lab"))).await;
    apply_request(&mut client, &mut view, create_agent("lab", "Scout", "")).await;
    let lead = r#"{"monitors":[{"name":"hourly","prompt":"Check the queue.","when":{"every":"1h"}},{"name":"logs","prompt":"Look at the logs.","when":{"check":"false","every":"6h"}}]}"#;
    call(&mut client, &mut view, "lead", "monitors_add", lead).await;
    let worker = r#"{"monitors":[{"name":"standup","prompt":"Post your status.","when":{"cron":"0 9 * * 1-5"},"agent":"worker"}]}"#;
    call(&mut client, &mut view, "lead", "monitors_add", worker).await;
    call(&mut client, &mut view, "scout", "monitors_add", r#"{"monitors":[{"name":"scan","prompt":"Scan.","when":{"every":"2h"}}]}"#).await;

    async fn list(client: &mut Client, view: &mut View, request: v1::ListMonitors) -> Vec<(String, u32, String, v1::MonitorHeld)> {
        match apply_request(client, view, client_message::Body::ListMonitors(request)).await {
            Body::MonitorList(list) => list
                .monitors
                .iter()
                .map(|row| {
                    let monitor = row.monitor.clone().expect("its monitor");
                    (row.agent_id.clone(), monitor.id, monitor.name, row.held())
                })
                .collect(),
            other => panic!("expected the list, got {other:?}"),
        }
    }
    let row = |agent: &str, id: u32, name: &str| (agent.to_owned(), id, name.to_owned(), v1::MonitorHeld::Unspecified);
    // Two agents both have an m1: each row says whose.
    let everyone = list(&mut client, &mut view, v1::ListMonitors::default()).await;
    assert_eq!(everyone, [row("lead", 1, "hourly"), row("lead", 2, "logs"), row("scout", 1, "scan"), row("worker", 1, "standup")]);
    let lab = v1::ListMonitors { project_id: "lab".into(), ..Default::default() };
    assert_eq!(list(&mut client, &mut view, lab).await, [row("scout", 1, "scan")]);
    let workers = v1::ListMonitors { agent_id: "worker".into(), ..Default::default() };
    assert_eq!(list(&mut client, &mut view, workers).await, [row("worker", 1, "standup")]);

    // Pausing names the right agent's m1, and the state filter finds it.
    let pause = v1::ChangeMonitor { agent_id: "worker".into(), monitor_id: 1, action: v1::MonitorAction::Pause.into() };
    apply_request(&mut client, &mut view, client_message::Body::ChangeMonitor(pause)).await;
    let paused = v1::ListMonitors { states: vec![v1::MonitorState::Paused.into()], ..Default::default() };
    assert_eq!(list(&mut client, &mut view, paused).await, [row("worker", 1, "standup")]);
    let active = v1::ListMonitors { states: vec![v1::MonitorState::Active.into()], ..Default::default() };
    assert_eq!(list(&mut client, &mut view, active).await.len(), 3);

    // Monitors turned off for an agent: its active ones are held, and every
    // client hears that monitors changed.
    let (mut other, _, _) = Client::connect(addr).await;
    let off = v1::ExtensionChange { id: "monitors".into(), enabled: Some(false), ..Default::default() };
    let off = client_message::Body::UpdateAgent(v1::UpdateAgent { agent_id: "scout".into(), extension: Some(off), ..Default::default() });
    apply_request(&mut client, &mut view, off).await;
    other.wait_for(|body| matches!(body, Body::MonitorsChanged(_)).then_some(())).await;
    let scouts = v1::ListMonitors { agent_id: "scout".into(), ..Default::default() };
    assert_eq!(list(&mut client, &mut view, scouts).await[0].3, v1::MonitorHeld::ExtensionOff);

    // A fire and the person's fire by hand, newest first, a page at a time.
    factory.advance_monitor_clock(Duration::from_secs(3600));
    view.follow(&mut client, |view| idle(view, "lead")).await;
    async fn history(client: &mut Client, view: &mut View, agent: &str, id: u32, before_seq: u64, limit: u32) -> Body {
        let request = v1::ListMonitorHistory { agent_id: agent.into(), monitor_id: id, before_seq, limit };
        apply_request(client, view, client_message::Body::ListMonitorHistory(request)).await
    }
    let events = loop {
        let Body::MonitorHistory(page) = history(&mut client, &mut view, "lead", 1, 0, 0).await else { panic!("a history") };
        if page.events.len() >= 2 {
            break page;
        }
        settle(&mut client, &mut view, 50).await;
    };
    assert_eq!((events.agent_id.as_str(), events.monitor_id, events.more), ("lead", 1, false));
    let kinds: Vec<v1::MonitorEventKind> = events.events.iter().map(|event| event.kind()).collect();
    assert_eq!(kinds, [v1::MonitorEventKind::Fired, v1::MonitorEventKind::Created]);
    let (fired, created) = (&events.events[0], &events.events[1]);
    assert!(fired.seq > created.seq && fired.time_ms > created.time_ms, "{events:?}");
    assert_eq!((created.actor.as_str(), created.detail.as_str()), ("lead", "every 1h"));
    assert_eq!(fired.actor, "", "on schedule: Claktory's");
    let Body::MonitorHistory(first) = history(&mut client, &mut view, "lead", 1, 0, 1).await else { panic!("a page") };
    assert_eq!((first.events.len(), first.more, first.events[0].seq), (1, true, fired.seq));
    let Body::MonitorHistory(next) = history(&mut client, &mut view, "lead", 1, first.events[0].seq, 1).await else { panic!("a page") };
    assert_eq!((next.events.len(), next.more, next.events[0].seq), (1, false, created.seq));
    // Fired by hand, by the agent: its actor says so.
    call(&mut client, &mut view, "lead", "monitors_change", r#"{"changes":[{"id":"m1","action":"fire"}]}"#).await;
    let Body::MonitorHistory(latest) = history(&mut client, &mut view, "lead", 1, 0, 1).await else { panic!("a page") };
    let fire = &latest.events[0];
    assert_eq!((fire.kind(), fire.actor.as_str(), fire.detail.as_str()), (v1::MonitorEventKind::Fired, "lead", "fired by lead"));
    view.follow(&mut client, |view| idle(view, "lead")).await;
    // The worker's m1 is its own: created by lead, paused by the person.
    let Body::MonitorHistory(workers) = history(&mut client, &mut view, "worker", 1, 0, 0).await else { panic!("a history") };
    let kinds: Vec<v1::MonitorEventKind> = workers.events.iter().map(|event| event.kind()).collect();
    assert_eq!(kinds, [v1::MonitorEventKind::Paused, v1::MonitorEventKind::Created]);
    assert_eq!((workers.events[0].actor.as_str(), workers.events[0].detail.as_str()), ("", "by the person"));
    // Resuming says who did, as pausing does.
    let resume = v1::ChangeMonitor { agent_id: "worker".into(), monitor_id: 1, action: v1::MonitorAction::Resume.into() };
    apply_request(&mut client, &mut view, client_message::Body::ChangeMonitor(resume)).await;
    let Body::MonitorHistory(latest) = history(&mut client, &mut view, "worker", 1, 0, 1).await else { panic!("a page") };
    let resumed = &latest.events[0];
    assert_eq!(
        (resumed.kind(), resumed.actor.as_str(), resumed.detail.as_str()),
        (v1::MonitorEventKind::Resumed, "", "by the person")
    );
    match history(&mut client, &mut view, "worker", 9, 0, 0).await {
        Body::Error(error) => assert_eq!((error.code(), error.message.as_str()), (v1::ErrorCode::NotFound, "worker has no monitor m9.")),
        other => panic!("expected an error, got {other:?}"),
    }
}

/// Makes the secret "Export" (TOKEN, and KEYFILE as a file) for `agent`,
/// and "Other", granted to no one; returns Export's id.
async fn export_secret(client: &mut Client, view: &mut View, agent: &str) -> String {
    let entry = |variable: &str, value: &str, file: bool| v1::SecretEntry { variable: variable.into(), value: value.into(), file };
    let secret = v1::CreateSecret {
        name: "Export".into(),
        description: "Reads the usage export.".into(),
        entries: vec![entry("TOKEN", "tok-0123456789abcdef", false), entry("KEYFILE", "key-material-0123456789", true)],
        ask_each_time: false,
        sites: Vec::new(),
    };
    apply_request(client, view, client_message::Body::CreateSecret(secret)).await;
    let other = v1::CreateSecret { name: "Other".into(), entries: vec![entry("OTHER", "other-0123456789", false)], ..Default::default() };
    apply_request(client, view, client_message::Body::CreateSecret(other)).await;
    let id = view.workspace.vault.clone().expect("vault").secrets.iter().find(|secret| secret.name == "Export").expect("Export").id.clone();
    let grant = v1::GrantSecret { id: id.clone(), agent_id: agent.into(), proof: None, fill_only: false };
    apply_request(client, view, client_message::Body::GrantSecret(grant)).await;
    id
}

fn add(name: &str, when: serde_json::Value) -> String {
    serde_json::json!({"monitors": [{"name": name, "prompt": "Look.", "when": when}]}).to_string()
}

/// (a), Fugo's conditions: a check's command gets the secrets its agent
/// named, as bash's would; what it prints has their values hidden, a file
/// secret's file is gone after it, and the vault logs the use.
#[tokio::test]
async fn a_check_gets_the_secrets_it_names_with_their_values_hidden_and_files_removed() {
    let (_dir, _factory, mut client, mut view) = setup().await;
    export_secret(&mut client, &mut view, "lead").await;
    let command = r#"echo "token $TOKEN"; echo "file $(cat "$KEYFILE")"; echo "path $KEYFILE""#;
    let added = call(&mut client, &mut view, "lead", "monitors_add", &add("export", serde_json::json!({"check": command, "every": "30s", "secrets": ["Export"]}))).await;
    assert!(added.ends_with(", fires on success, with the secret Export; first check now."), "{added}");
    view.follow(&mut client, |view| fires(view).len() == 1 && idle(view, "lead")).await;
    let fire = fires(&view)[0].text.clone();
    assert!(fire.contains("token [secret Export/TOKEN]\nfile [secret Export/KEYFILE]\npath "), "{fire}");
    assert!(!fire.contains("0123456789"), "{fire}");
    let path = fire.split("path ").nth(1).and_then(|rest| rest.lines().next()).expect("the file's path");
    assert!(std::path::Path::new(path).ends_with("monitors/m1.secrets/KEYFILE"), "{path}");
    assert!(!std::path::Path::new(path).exists(), "removed after the run");
    view.follow(&mut client, |view| {
        view.workspace.vault.as_ref().is_some_and(|vault| vault.secrets.iter().any(|secret| secret.name == "Export" && secret.last_used_by == "lead"))
    })
    .await;

    // A check that names none gets none.
    call(&mut client, &mut view, "lead", "monitors_add", &add("plain", serde_json::json!({"check": "echo \"plain [$TOKEN]\"", "every": "30s"}))).await;
    view.follow(&mut client, |view| fires(view).len() == 2 && idle(view, "lead")).await;
    assert!(fires(&view)[1].text.contains("Output (end):\nplain []\n"), "{:?}", fires(&view)[1].text);

    // Output read up to its 64 KB ending partway into a value: that end goes.
    let long = serde_json::json!({"check": "head -c 65530 /dev/zero | tr '\\0' x; echo \"$TOKEN\"", "every": "30s", "secrets": ["Export"]});
    call(&mut client, &mut view, "lead", "monitors_add", &add("long", long)).await;
    view.follow(&mut client, |view| fires(view).len() == 3 && idle(view, "lead")).await;
    let fire = fires(&view)[2].text.clone();
    assert!(fire.contains("xxx\n[the rest cut]") && !fire.contains("tok-"), "{}", &fire[fire.len().saturating_sub(200)..]);
}

#[tokio::test]
async fn a_check_can_name_only_its_own_agents_secrets_never_ones_that_ask_each_time() {
    let (_dir, factory, mut client, mut view) = setup().await;
    export_secret(&mut client, &mut view, "lead").await;
    let check = |secrets: serde_json::Value| serde_json::json!({"check": "true", "secrets": secrets});
    // Not granted to it, or not in the vault: as bash says.
    for name in ["Other", "Nowhere"] {
        let refused = call(&mut client, &mut view, "lead", "monitors_add", &add("x", check(serde_json::json!([name])))).await;
        assert!(refused.starts_with(&format!("\"{name}\" isn't one of your secrets.")) && refused.ends_with("Nothing was added."), "{refused}");
    }
    // Another agent's monitor, even with a secret that agent has.
    let id = view.workspace.vault.clone().expect("vault").secrets.iter().find(|secret| secret.name == "Export").expect("Export").id.clone();
    let grant = v1::GrantSecret { id, agent_id: "worker".into(), proof: None, fill_only: false };
    apply_request(&mut client, &mut view, client_message::Body::GrantSecret(grant)).await;
    let for_worker = serde_json::json!({"monitors": [{"name": "x", "prompt": "y", "when": check(serde_json::json!(["Export"])), "agent": "worker"}]});
    let refused = call(&mut client, &mut view, "lead", "monitors_add", &for_worker.to_string()).await;
    assert!(refused.contains("A check with secrets can only be its agent's own"), "{refused}");
    // One that asks the person each time: nobody is there when it runs.
    let asks = v1::CreateSecret {
        name: "Deploy".into(),
        entries: vec![v1::SecretEntry { variable: "DEPLOY".into(), value: "deploy-0123456789".into(), file: false }],
        ask_each_time: true,
        ..Default::default()
    };
    apply_request(&mut client, &mut view, client_message::Body::CreateSecret(asks)).await;
    let id = view.workspace.vault.clone().expect("vault").secrets.iter().find(|secret| secret.name == "Deploy").expect("Deploy").id.clone();
    let grant = v1::GrantSecret { id, agent_id: "lead".into(), proof: None, fill_only: false };
    apply_request(&mut client, &mut view, client_message::Body::GrantSecret(grant)).await;
    let refused = call(&mut client, &mut view, "lead", "monitors_add", &add("x", check(serde_json::json!(["Export", "Deploy"])))).await;
    assert!(refused.starts_with("Deploy asks the person each time it's used, and nobody is there to ask when a check runs"), "{refused}");
    // Only a check takes secrets.
    let refused = call(&mut client, &mut view, "lead", "monitors_add", &add("x", serde_json::json!({"watch": "tail -F log", "secrets": ["Export"]}))).await;
    assert!(refused.starts_with("secrets works only with check"), "{refused}");
    // An agent that has read a web page: its secrets need the person for each command.
    factoryd::pipes::taint_for_test(&factory, "lead", "browser:https://example.com");
    let refused = call(&mut client, &mut view, "lead", "monitors_add", &add("x", check(serde_json::json!(["Export"])))).await;
    assert!(refused.starts_with("You've read a page on https://example.com, so each command with a secret needs the person's approval"), "{refused}");
    assert_eq!(call(&mut client, &mut view, "lead", "monitors_list", "{}").await.lines().filter(|line| line.starts_with('m')).count(), 0);
}

/// Nothing a check with secrets prints reaches its agent but a fire; once
/// its secret is taken back, its runs fail and it stops, saying why.
#[tokio::test]
async fn a_check_whose_secret_is_taken_back_stops_and_shows_nothing_until_then() {
    let (_dir, factory, mut client, mut view) = setup().await;
    let id = export_secret(&mut client, &mut view, "lead").await;
    let never = serde_json::json!({"check": "echo \"$TOKEN\"; false", "every": "30s", "secrets": ["Export"]});
    call(&mut client, &mut view, "lead", "monitors_add", &add("quiet", never)).await;
    let entries = view.entries.len();
    for _ in 0..3 {
        factory.advance_monitor_clock(Duration::from_secs(30));
        settle(&mut client, &mut view, 300).await;
    }
    assert_eq!(view.entries.len(), entries, "nothing reached the conversation");

    let revoke = v1::RevokeSecret { id, agent_id: "lead".into(), stop_running_tasks: false };
    apply_request(&mut client, &mut view, client_message::Body::RevokeSecret(revoke)).await;
    for _ in 0..20 {
        if !fires(&view).is_empty() {
            break;
        }
        factory.advance_monitor_clock(Duration::from_secs(30));
        settle(&mut client, &mut view, 300).await;
    }
    let stopped = fires(&view).pop().expect("a notice").text;
    assert!(
        stopped.starts_with(
            "Monitor m1 \"quiet\" stopped: its check couldn't run 5 times in a row (last: its secrets couldn't be had: \"Export\" isn't one of your secrets."
        ),
        "{stopped}"
    );
    assert!(!stopped.contains("0123456789"), "{stopped}");
}

/// Fugo 28177: the files a check runs can change after it's set, so while
/// its agent has read a web page, or is contained, its check is held: not
/// run, nothing logged, one notice, no failure counted; it carries on once
/// that ends.
#[tokio::test]
async fn a_check_with_secrets_is_held_while_its_agent_has_read_a_page_or_is_contained() {
    let (dir, factory, mut client, mut view) = setup().await;
    export_secret(&mut client, &mut view, "lead").await;
    let marker = dir.join("site").join("ran");
    let check = serde_json::json!({"check": "echo \"$TOKEN\" > ran; false", "every": "30s", "secrets": ["Export"]});
    call(&mut client, &mut view, "lead", "monitors_add", &add("collect", check)).await;
    let used = |view: &View| {
        view.workspace.vault.as_ref().and_then(|vault| vault.secrets.iter().find(|secret| secret.name == "Export")).map(|secret| secret.last_used_ms)
    };
    view.follow(&mut client, |view| used(view).is_some_and(|used| used > 0)).await;
    for _ in 0..50 {
        if marker.exists() {
            break;
        }
        settle(&mut client, &mut view, 100).await;
    }
    std::fs::remove_file(&marker).expect("it ran");
    let logged = used(&view);

    // Tainted: held, with one notice, past the hour its use would be logged again.
    factoryd::pipes::taint_for_test(&factory, "lead", "browser:https://example.com");
    factory.advance_monitor_clock(Duration::from_secs(2 * 3600));
    view.follow(&mut client, |view| !fires(view).is_empty() && idle(view, "lead")).await;
    let notice = fires(&view)[0].text.clone();
    assert_eq!(
        notice,
        "Monitor m1 \"collect\" is held: you've read a page on https://example.com, so it doesn't run with its secrets until your conversation is cleared. Then it carries on by itself."
    );
    for _ in 0..6 {
        factory.advance_monitor_clock(Duration::from_secs(30));
        settle(&mut client, &mut view, 200).await;
    }
    assert!(!marker.exists(), "not run while held");
    assert_eq!(fires(&view).len(), 1, "told once, and never stopped");
    assert_eq!(used(&view), logged, "nothing logged");

    // Cleared: it runs again by itself.
    let clear = v1::RunCommand { agent_id: "lead".into(), name: "clear".into(), args: String::new() };
    apply_request(&mut client, &mut view, client_message::Body::RunCommand(clear)).await;
    for _ in 0..50 {
        if marker.exists() {
            break;
        }
        factory.advance_monitor_clock(Duration::from_secs(30));
        settle(&mut client, &mut view, 100).await;
    }
    assert!(marker.exists(), "it carried on");
    std::fs::remove_file(&marker).expect("ran");

    // Made a curator (contained): held too, until that's lifted.
    factoryd::pipes::make_curator_for_test(&factory, "lead", false).await.expect("a curator");
    view.follow(&mut client, |view| fires(view).iter().any(|fire| fire.text.contains("limited to a pipe's work"))).await;
    let held = fires(&view).into_iter().find(|fire| fire.text.contains("limited to a pipe's work")).expect("a notice").text;
    assert_eq!(
        held,
        "Monitor m1 \"collect\" is held: you're limited to a pipe's work, so it doesn't run with its secrets until the person lifts that. Then it carries on by itself."
    );
    std::fs::remove_file(&marker).ok();
    for _ in 0..3 {
        factory.advance_monitor_clock(Duration::from_secs(30));
        settle(&mut client, &mut view, 200).await;
    }
    assert!(!marker.exists(), "not run while contained");
    factoryd::pipes::release_curator_for_test(&factory, "lead").await.expect("released");
    for _ in 0..50 {
        if marker.exists() {
            break;
        }
        factory.advance_monitor_clock(Duration::from_secs(30));
        settle(&mut client, &mut view, 100).await;
    }
    assert!(marker.exists(), "it carried on");
}

fn set_pipe(pipe: &str, action: v1::PipeAction) -> client_message::Body {
    client_message::Body::SetPipeState(v1::SetPipeState { pipe_id: pipe.into(), action: action.into() })
}

/// Lets the pipes monitors look again, until Lead (selected) has `count`
/// fires and is idle.
async fn pipe_fires(factory: &factoryd::Factory, client: &mut Client, view: &mut View, count: usize) -> Vec<String> {
    for _ in 0..200 {
        if fires(view).len() >= count && idle(view, "lead") {
            break;
        }
        factory.advance_monitor_clock(Duration::from_secs(2));
        settle(client, view, 100).await;
    }
    fires(view).iter().map(|fire| fire.text.clone()).collect()
}

/// Friend bug #34: a pipes monitor fires when its project's pipes are
/// paused, resumed, shared over or closed, in this side's words: never the
/// other side's, and never of another project's pipes, which only an admin
/// agent hears of.
#[tokio::test]
async fn a_pipes_monitor_fires_on_its_projects_pipes_in_this_sides_words() {
    use factoryd::pipes;
    let (dir, factory, mut client, mut view) = setup().await;
    apply_request(&mut client, &mut view, create_project("Other", &dir.join("other"))).await;
    let launch = pipes::open_for_test(&factory, "site", "Launch", &["lead"], "lead", false);
    let plans = pipes::open_for_test(&factory, "other", "Plans", &[], "", false);
    apply_request(&mut client, &mut view, select("lead")).await;

    let refused = call(&mut client, &mut view, "lead", "monitors_add", &add("other", serde_json::json!({"pipes": "Plans"}))).await;
    assert!(refused.contains("There's no pipe \"Plans\" you can hear of"), "{refused}");
    let added = call(&mut client, &mut view, "lead", "monitors_add", &add("pipes", serde_json::json!({"pipes": "*"}))).await;
    assert!(added.contains("\"pipes\": pipes: any it can see; watching from now."), "{added}");

    // Launch paused by the other side, Plans (another project's) closed: one fire, for Launch.
    pipes::peer_paused_for_test(&factory, &launch, true);
    apply_request(&mut client, &mut view, set_pipe(&plans, v1::PipeAction::Close)).await;
    let texts = pipe_fires(&factory, &mut client, &mut view, 1).await;
    assert_eq!(texts.len(), 1, "{texts:?}");
    let paused = format!("Monitor m1 \"pipes\" fired (pipe paused; fire 1).\nThe pipe \"Launch\" (id {launch}, in Site) was paused by the other side.\n\nYour prompt: Look.");
    assert_eq!(texts[0], paused);

    // The other side shares a website and resumes; then the person closes it.
    pipes::shares::offered_for_test(&factory, &launch, "peer", "ab12", "Ana's secret dashboard");
    pipes::peer_paused_for_test(&factory, &launch, false);
    let texts = pipe_fires(&factory, &mut client, &mut view, 2).await;
    let all = texts.join("\n");
    assert!(all.contains(&format!("The pipe \"Launch\" (id {launch}, in Site) was resumed: it's open again.")), "{all}");
    assert!(all.contains("The other side shared a website over the pipe \"Launch\""), "{all}");
    apply_request(&mut client, &mut view, set_pipe(&launch, v1::PipeAction::Close)).await;
    let texts = pipe_fires(&factory, &mut client, &mut view, texts.len() + 1).await;
    let all = texts.join("\n");
    assert!(all.contains(&format!("The pipe \"Launch\" (id {launch}, in Site) was closed.")), "{all}");
    // Nothing the other side says about itself or its website, nor of Plans.
    for theirs in ["Ana", "Marketing", "dashboard", "Plans", &plans] {
        assert!(!all.contains(theirs), "{theirs} in {all}");
    }

    // An admin agent hears of every project's pipes.
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(crate::common::MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;
    let added = call(&mut client, &mut view, "rivet", "monitors_add", &add("plans", serde_json::json!({"pipes": "Plans", "on": "closed"}))).await;
    assert!(added.contains(&format!("\"plans\": pipe {plans}, on closed; watching from now.")), "{added}");
}

/// The wall clock set back a day (Abbacchio's row 1413): no every, cron or
/// check monitor stays due a day ahead, a paused one resumed included; each
/// is due again within its period of the new now, and then fires once.
#[tokio::test]
async fn a_clock_set_back_leaves_no_monitor_silent_for_the_step() {
    let (_dir, factory, mut client, mut view) = setup().await;
    let add = r#"{"monitors":[
        {"name":"hourly","prompt":"Look.","when":{"every":"1h"}},
        {"name":"cron","prompt":"Look.","when":{"cron":"0 * * * *"}},
        {"name":"check","prompt":"Look.","when":{"check":"false","every":"5m"}},
        {"name":"paused","prompt":"Look.","when":{"every":"2h"}}]}"#;
    call(&mut client, &mut view, "lead", "monitors_add", &add.replace('\n', "")).await;
    call(&mut client, &mut view, "lead", "monitors_change", r#"{"changes":[{"id":"m4","action":"pause"}]}"#).await;
    let day = Duration::from_secs(86_400);
    factory.rewind_monitor_clock(day);
    call(&mut client, &mut view, "lead", "monitors_change", r#"{"changes":[{"id":"m4","action":"resume"}]}"#).await;

    let open = v1::OpenPanel { extension_id: "monitors".into(), agent_id: "lead".into() };
    apply_request(&mut client, &mut view, client_message::Body::OpenPanel(open)).await;
    let hour = 3_600_000;
    let periods = [(1, hour), (2, hour), (3, 5 * 60_000), (4, 2 * hour)];
    let within = |view: &View| match view.extension_view.as_ref().and_then(|view| view.body.as_ref()) {
        Some(v1::extension_view::Body::Monitors(monitors)) => {
            let now = factory_now() - day.as_millis() as i64;
            periods.iter().all(|(id, period)| {
                monitors.monitors.iter().any(|monitor| monitor.id == *id && monitor.next_due > 0 && monitor.next_due <= now + period + 5_000)
            })
        }
        _ => false,
    };
    view.follow(&mut client, within).await;

    // An hour on: the hourly and the cron fire, once each; no storm.
    factory.advance_monitor_clock(Duration::from_secs(3600));
    view.follow(&mut client, |view| fires(view).len() == 2 && idle(view, "lead")).await;
    settle(&mut client, &mut view, 300).await;
    let names: Vec<String> = fires(&view).iter().map(|fire| fire.summary.clone()).collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names.contains(&"m1 hourly fired".to_owned()) && names.contains(&"m2 cron fired".to_owned()), "{names:?}");
}

/// Friend bug #55: an archived project's agents' monitors pause, and no one
/// can message those agents; an admin agent still lists and cancels them,
/// as the person does in the Monitors panel, without bringing the project
/// back. Nothing fires or wakes the agent meanwhile; others can't name it.
#[tokio::test]
async fn an_admin_or_the_person_cancels_an_archived_agents_monitors_and_nothing_runs() {
    let (_dir, _factory, mut client, mut view) = setup().await;
    let added = call(
        &mut client,
        &mut view,
        "lead",
        "monitors_add",
        r#"{"monitors": [{"name": "nightly", "prompt": "Check the build.", "when": {"every": "1h"}}, {"name": "weekly", "prompt": "Sum up.", "when": {"every": "7d"}}]}"#,
    )
    .await;
    assert!(added.starts_with("Added"), "{added}");
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(crate::common::MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rivet)).await;
    apply_request(&mut client, &mut view, create_agent("", "Ana", "")).await;
    let archive = v1::UpdateProject { project_id: "site".into(), name: None, archived: Some(true), before_id: None, brief: None };
    apply_request(&mut client, &mut view, client_message::Body::UpdateProject(archive)).await;

    // Rivet lists and cancels them; firing one is refused.
    let listed = call(&mut client, &mut view, "rivet", "monitors_list", r#"{"agent": "lead"}"#).await;
    assert!(listed.contains("nightly") && listed.contains("weekly"), "{listed}");
    let fired = call(&mut client, &mut view, "rivet", "monitors_change", r#"{"agent": "lead", "changes": [{"id": "m1", "action": "fire"}]}"#).await;
    assert!(fired.contains("can't fire while its agent's project is archived") && fired.ends_with("Nothing changed."), "{fired}");
    let cancelled = call(&mut client, &mut view, "rivet", "monitors_change", r#"{"agent": "lead", "changes": [{"id": "m1", "action": "cancel"}]}"#).await;
    assert_eq!(cancelled, "m1 cancelled.");
    // Adding one for it is still refused, and an agent that isn't an admin can't name it.
    let add = r#"{"monitors": [{"name": "x", "prompt": "x", "when": {"in": "1h"}, "agent": "lead"}]}"#;
    let refused = call(&mut client, &mut view, "rivet", "monitors_add", add).await;
    assert!(!refused.starts_with("Added"), "{refused}");
    let refused = call(&mut client, &mut view, "ana", "monitors_list", r#"{"agent": "lead"}"#).await;
    assert!(refused.starts_with("No agent you can message is called \"lead\""), "{refused}");

    // The person cancels the other in the Monitors panel, which shows it held.
    let Body::MonitorList(list) =
        apply_request(&mut client, &mut view, client_message::Body::ListMonitors(v1::ListMonitors { agent_id: "lead".into(), ..Default::default() })).await
    else {
        panic!("a list")
    };
    let weekly = list.monitors.iter().find(|shown| shown.monitor.as_ref().is_some_and(|monitor| monitor.name == "weekly")).expect("weekly");
    assert_eq!(weekly.held(), v1::MonitorHeld::ProjectArchived);
    let cancel = v1::ChangeMonitor { agent_id: "lead".into(), monitor_id: 2, action: v1::MonitorAction::Cancel.into() };
    assert!(matches!(apply_request(&mut client, &mut view, client_message::Body::ChangeMonitor(cancel)).await, Body::Ack(_)));
    let again = v1::ChangeMonitor { agent_id: "lead".into(), monitor_id: 2, action: v1::MonitorAction::Cancel.into() };
    assert!(matches!(apply_request(&mut client, &mut view, client_message::Body::ChangeMonitor(again)).await, Body::Error(_)), "ended");
    let listed = call(&mut client, &mut view, "rivet", "monitors_list", r#"{"agent": "lead"}"#).await;
    assert_eq!(listed.lines().filter(|line| line.starts_with("m")).count(), 0, "{listed}");

    // The agent never woke.
    apply_request(&mut client, &mut view, select("lead")).await;
    assert!(fires(&view).is_empty());
    assert!(idle(&view, "lead"));
}
