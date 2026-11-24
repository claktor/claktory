//! The Settings extension (#422): an admin agent reads and changes the
//! settings kept in factory.db through the app's own paths; an agent that
//! isn't an admin has no such tool, even with the extension on.

use crate::common::{Client, MODEL, View, apply_request, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, NotifyMode, NotifyPreview, client_message, entry::Kind, server_message::Body};

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

/// `agent` calls `tool` with `args`; returns what it said.
async fn call(client: &mut Client, view: &mut View, agent: &str, tool: &str, args: serde_json::Value) -> String {
    apply_request(client, view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })).await;
    let before = view.entries.len();
    apply_request(client, view, send_message(agent, &format!("call {tool} {args}"))).await;
    view.follow(client, |view| view.entries.len() >= before + 3 && idle(view, agent)).await;
    view.entries[before..]
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) => Some(call.output.clone()),
            _ => None,
        })
        .expect("the call")
}

fn settings(view: &View) -> v1::Settings {
    view.workspace.settings.clone().expect("settings")
}

fn shared(view: &View) -> bool {
    view.workspace.diagnostics.as_ref().is_some_and(|diagnostics| diagnostics.share)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_admin_agent_reads_and_changes_the_settings_kept_in_the_database_as_the_app_does() {
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("home")).expect("home");
    let config = serde_json::json!({"agents": [
        {"id": "rivet", "name": "Rivet", "model": MODEL, "effort": "off", "extensions": {"agents": {"admin": true}}},
        {"id": "lea", "name": "Lea", "model": MODEL, "effort": "off", "extensions": {"settings": {"enabled": true}}},
    ]});
    std::fs::write(dir.join("home/config.json"), config.to_string()).expect("config");
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };

    // As they are: the defaults.
    let read = call(&mut client, &mut view, "rivet", "settings", serde_json::json!({})).await;
    for line in [
        "Advanced diagnostics: off\n",
        "Notifications: needs_you on, alerts on, replies on, desktop_replies off; preview full; quiet_hours off (22:00 to 07:00, urgent_breaks_quiet off)\n",
        "Agents' Notify me: all default\n",
        "Checklist (minutes): agent_blocked 240, person_blocked 1440, external_retry 120, reask 1440, overdue 1440\n",
        "Panes beside a conversation: 4 (the default)\n",
        "Voice model: ",
        "App dock: ",
    ] {
        assert!(read.contains(line), "{line:?} in {read}");
    }
    assert!(!read.starts_with("Changed"), "{read}");

    // Several at once, each as the app would change it.
    let changed = call(
        &mut client,
        &mut view,
        "rivet",
        "settings",
        serde_json::json!({
            "notifications": {"quiet_hours": true, "quiet_start": "21:30", "preview": "who"},
            "agent_notify": {"Lea": "never"},
            "checklist": {"reask": 60},
            "max_open_panes": 6,
            "keep_awake": false,
            "voice_model": "remove",
        }),
    )
    .await;
    assert!(
        changed.starts_with("Changed: keep this Mac awake off; notifications; lea's Notify me; the checklist's intervals; the pane limit; the voice model removed."),
        "{changed}"
    );
    assert!(changed.contains("preview who; quiet_hours on (21:30 to 07:00"), "{changed}");
    assert!(changed.contains("Agents' Notify me: lea never; the rest default"), "{changed}");
    assert!(changed.contains("reask 60, overdue 1440"), "{changed}");
    assert!(changed.contains("Panes beside a conversation: 6\n"), "{changed}");
    assert!(changed.contains("Voice model: not installed") || changed.contains("Voice model: not available here"), "{changed}");
    view.follow(&mut client, |view| settings(view).max_open_panes == 6).await;
    let now = settings(&view);
    let notifications = now.notifications.expect("notifications");
    assert!(notifications.quiet_hours && notifications.needs_you, "{notifications:?}");
    assert_eq!((notifications.quiet_start, notifications.preview()), (21 * 60 + 30, NotifyPreview::Who));
    assert_eq!(now.checklist.expect("checklist").reask_minutes, 60);
    assert_eq!(view.agent("lea").notify(), NotifyMode::Never);
    if let Some(awake) = view.workspace.keep_awake.as_ref().filter(|awake| awake.available) {
        assert!(!awake.on && awake.answered, "{awake:?}");
    }

    // What the app would refuse, it refuses too.
    let refused = call(&mut client, &mut view, "rivet", "settings", serde_json::json!({"checklist": {"reask": 5}})).await;
    assert!(refused.contains("checklist.reask: must be >= 15"), "{refused}");
    let refused = call(&mut client, &mut view, "rivet", "settings", serde_json::json!({"notifications": {"quiet_end": "25:00"}})).await;
    assert!(refused.contains("\"25:00\" isn't a time of day such as 22:00."), "{refused}");
    let refused = call(&mut client, &mut view, "rivet", "settings", serde_json::json!({"agent_notify": {"Nobody": "always"}})).await;
    assert!(refused.contains("No agent is called \"Nobody\"."), "{refused}");
    assert_eq!(settings(&view).checklist.expect("checklist").reask_minutes, 60);

    // Advanced diagnostics on: a device's events arrive, and it says so.
    let on = call(&mut client, &mut view, "rivet", "settings", serde_json::json!({"diagnostics": true})).await;
    assert!(on.starts_with("Changed: advanced diagnostics on.\n\nAdvanced diagnostics: on, nothing kept yet\n"), "{on}");
    view.follow(&mut client, shared).await;
    let event = v1::DiagnosticEvent {
        area: "connection".into(),
        session: 1,
        seq: 1,
        at_ms: factoryd::transcript::now_ms(),
        kind: "connect".into(),
        detail: "route=tunnel".into(),
        duration_ms: 812,
    };
    let sent = client.request(client_message::Body::SendDiagnostics(v1::SendDiagnostics { events: vec![event] })).await;
    assert!(matches!(sent.body, Body::Ack(_)), "{:?}", sent.body);
    let kept = call(&mut client, &mut view, "rivet", "settings", serde_json::json!({})).await;
    assert!(kept.starts_with("Advanced diagnostics: on, kept (read it with db_query on diagnostics):\n  - "), "{kept}");
    assert!(kept.contains("(device:local): 1 event,") && kept.contains("connection; last sent"), "{kept}");

    // Lea has the extension, but isn't an admin: no such tool.
    let said = call(&mut client, &mut view, "lea", "settings", serde_json::json!({"diagnostics": false})).await;
    assert!(!said.starts_with("Changed"), "{said}");
    assert!(shared(&view));

    // Off deletes everything kept, and nothing more is taken.
    let off = call(&mut client, &mut view, "rivet", "settings", serde_json::json!({"diagnostics": false})).await;
    assert!(off.starts_with("Changed: advanced diagnostics off, and everything kept deleted (1 event).\n\nAdvanced diagnostics: off\n"), "{off}");
    view.follow(&mut client, |view| !shared(view)).await;
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
    let left: i64 = db.query_row("SELECT COUNT(*) FROM diagnostics", [], |row| row.get(0)).expect("count");
    assert_eq!(left, 0);
    let refused = client.request(client_message::Body::SendDiagnostics(v1::SendDiagnostics { events: Vec::new() })).await;
    assert!(matches!(refused.body, Body::Error(_)), "{:?}", refused.body);

    // After reading a web page it still reads them, but changes nothing
    // (Fugo, as for agents_stop).
    factoryd::pipes::taint_for_test(&factory, "rivet", "browser:https://example.com");
    let refused = call(&mut client, &mut view, "rivet", "settings", serde_json::json!({"diagnostics": true})).await;
    assert_eq!(
        refused,
        "You've read a page on https://example.com, so you can't change these settings: what you read there may be someone else's instructions. The person can change them in Settings. Nothing was changed."
    );
    let read = call(&mut client, &mut view, "rivet", "settings", serde_json::json!({})).await;
    assert!(read.starts_with("Advanced diagnostics: off\n"), "{read}");
    assert!(!shared(&view));
}
