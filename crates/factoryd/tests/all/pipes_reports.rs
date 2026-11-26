//! Reports (pipes-automation.md §2, H1 and H5's backend): a pipe's liaison
//! forwards finished reports, in a schema's fields, to the agent the person
//! chose to read them; what the reader reads marks it, what the app reads
//! marks no one; nothing the liaison sent comes back in a refusal; a change
//! of where reports go makes unread ones wait for the person; and Rivet can
//! set it only while untainted, and only toward containment.

use std::sync::Arc;

use crate::common::{Client, MODEL, Scripted, View, apply_request, create_project, start_with_models, temp_dir};
use crate::pipes::{call, idle, select, shown};
use factory_protocol::v1::{self, client_message::Body as Request, entry::Kind, server_message};
use factoryd::factory::Factory;
use factoryd::pipes;

const MARKER: &str = "MARKER-c10cc0";

/// `tool` through extension_call: a tool that came after the agent's context began.
async fn later(client: &mut Client, view: &mut View, agent: &str, tool: &str, args: &str) -> String {
    call(client, view, agent, "extension_call", &format!(r#"{{"tool": "{tool}", "args": {args}}}"#)).await
}

/// Site with Lea, Ingrid and Rivet (the admin agent), an open pipe with its
/// liaison, and reports' wakes at once.
async fn setup() -> (std::path::PathBuf, Arc<Factory>, Client, View, String) {
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("home")).expect("home");
    let config = serde_json::json!({
        "agents": [
            {"id": "rivet", "name": "Rivet", "model": MODEL, "effort": "off", "extensions": {"agents": {"admin": true}}},
            {"id": "dev", "name": "Dev", "model": MODEL, "effort": "off", "extensions": {"pipes": {"enabled": true}}},
        ],
    });
    std::fs::write(dir.join("home/config.json"), config.to_string()).expect("config");
    let (addr, factory) = start_with_models(&dir, Arc::new(Scripted::signed_in())).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    for name in ["Lea", "Ingrid"] {
        apply_request(&mut client, &mut view, crate::common::create_agent("site", name, "")).await;
    }
    let settings = v1::UpdateSettings { default_model: Some(MODEL.into()), default_effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdateSettings(settings)).await;
    pipes::set_wake_every(&factory, std::time::Duration::ZERO);
    pipes::handoff::set_wake_every(&factory, std::time::Duration::ZERO);
    let pipe = pipes::open_for_test(&factory, "site", "Website ⇄ Marketing", &["lea"], "lea", false);
    let add = v1::AddPipeLiaison { pipe_id: pipe.clone(), name: "Liaison".into() };
    apply_request(&mut client, &mut view, Request::AddPipeLiaison(add)).await;
    view.follow(&mut client, |view| shown(view, &pipe).is_some_and(|pipe| pipe.lead_id == "liaison")).await;
    (dir, factory, client, view, pipe)
}

fn forward(pipe: &str, to: &str, make_reader: bool) -> Request {
    Request::SetPipeForward(v1::SetPipeForward {
        pipe_id: pipe.into(),
        recipient_id: to.into(),
        make_reader,
        fresh: make_reader,
        ..Default::default()
    })
}

async fn refused(client: &mut Client, view: &mut View, request: Request) -> String {
    match apply_request(client, view, request).await {
        server_message::Body::Error(error) => error.message,
        other => panic!("not refused: {other:?}"),
    }
}

fn report(title: &str) -> String {
    serde_json::json!({
        "title": title,
        "what_happened": format!("The app quit when I opened Settings. {MARKER}-words"),
        "steps": ["Open the app", "Tap Settings"],
        "platform": "iphone",
        "app_version": "v0.1.16",
        "area": "settings",
        "severity": "crash",
        "reproducible": "always",
        "friend_confirmed": true,
        "token": title,
    })
    .to_string()
}

fn host_events(view: &View) -> Vec<String> {
    view.entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::Event(event)) => Some(event.text.clone()),
            _ => None,
        })
        .collect()
}

async fn list(client: &mut Client, view: &mut View, pipe: &str, recipient: &str) -> v1::PipeReportPage {
    let request = Request::ListPipeReports(v1::ListPipeReports { pipe_id: pipe.into(), recipient_id: recipient.into(), ..Default::default() });
    match apply_request(client, view, request).await {
        server_message::Body::PipeReportPage(page) => page,
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_liaison_forwards_a_report_and_only_its_reader_reads_its_words() {
    let (_dir, factory, mut client, mut view, pipe) = setup().await;
    // No forward: the liaison has no report tools.
    let none = call(&mut client, &mut view, "liaison", "handoff_submit", &report("Crash on Settings")).await;
    assert!(none.contains("handoff_submit"), "{none}");

    // Only an agent that reads reports can be chosen.
    let said = refused(&mut client, &mut view, forward(&pipe, "lea", false)).await;
    assert!(said.contains("doesn't read reports yet"), "{said}");
    apply_request(&mut client, &mut view, forward(&pipe, "ingrid", true)).await;
    view.follow(&mut client, |view| view.agent("ingrid").reads_reports).await;
    let set = shown(&view, &pipe).and_then(|pipe| pipe.forward).expect("forward");
    assert_eq!((set.recipient_id.as_str(), set.schema_id.as_str(), set.revision, set.set_by_id.as_str()), ("ingrid", "bug_report", 1, ""));
    assert_eq!(set.limits.expect("limits").per_hour, 6);
    assert_eq!(view.workspace.report_schemas[0].name, "Bug report");

    // Refusals name a code and the schema's own field, never what was sent.
    let mut bad: serde_json::Value = serde_json::from_str(&report("x")).expect("json");
    bad[MARKER] = "x".into();
    let said = later(&mut client, &mut view, "liaison", "handoff_submit", &bad.to_string()).await;
    assert_eq!(said, "refused: unknown_field");
    let mut bad: serde_json::Value = serde_json::from_str(&report("x")).expect("json");
    bad["severity"] = format!("crash {MARKER}").into();
    let said = later(&mut client, &mut view, "liaison", "handoff_submit", &bad.to_string()).await;
    assert_eq!(said, "refused: invalid severity");

    // Taken; a retry with the same token is the same report.
    let said = later(&mut client, &mut view, "liaison", "handoff_submit", &report("Crash on Settings")).await;
    assert_eq!(said, "accepted #1");
    let again = later(&mut client, &mut view, "liaison", "handoff_submit", &report("Crash on Settings")).await;
    assert_eq!(again, "accepted #1");

    // The reader is woken with a count, not the words, and isn't marked by it.
    apply_request(&mut client, &mut view, select("ingrid")).await;
    view.follow(&mut client, |view| host_events(view).iter().any(|text| text.contains("A new report")) && idle(view, "ingrid")).await;
    let woke = host_events(&view).into_iter().find(|text| text.contains("A new report")).expect("woken");
    assert!(woke.contains("via Liaison: 1") && !woke.contains(MARKER) && !woke.contains("Crash"), "{woke}");
    assert_eq!(pipes::taint_of_for_test(&factory, "ingrid"), "{}");

    // Facts only: no words, no mark.
    let facts = call(&mut client, &mut view, "ingrid", "handoff_read", r#"{"text": false, "since": 0}"#).await;
    assert!(facts.contains("severity: crash") && facts.contains("app_version: v0.1.16"), "{facts}");
    assert!(!facts.contains(MARKER) && !facts.contains("Crash on Settings") && !facts.contains("Tap Settings"), "{facts}");
    assert_eq!(pipes::taint_of_for_test(&factory, "ingrid"), "{}");

    // Its words, fenced as untrusted; reading them marks her with nothing
    // (#346).
    let words = call(&mut client, &mut view, "ingrid", "handoff_read", "{}").await;
    assert!(words.contains(&format!("{MARKER}-words")) && words.contains("2. Tap Settings") && words.contains("untrusted"), "{words}");
    assert_eq!(pipes::taint_of_for_test(&factory, "ingrid"), "{}");

    // Marked known: the pipe's side hears its own alias, never the issue key.
    let id = list(&mut client, &mut view, &pipe, "").await.reports.iter().find(|report| report.number == 1).expect("#1").id;
    let marked = call(&mut client, &mut view, "ingrid", "handoff_mark", &format!(r#"{{"report": {id}, "status": "known", "issue": "settings-crash-{MARKER}"}}"#)).await;
    assert!(marked.contains("known as A1"), "{marked}");
    let status = later(&mut client, &mut view, "liaison", "handoff_status", "{}").await;
    assert_eq!(status, "#1: known as A1");

    // The app reads it all, marking no one; refusals keep no values.
    let page = list(&mut client, &mut view, &pipe, "").await;
    let first = page.reports.iter().find(|report| report.number == 1).expect("#1");
    assert_eq!((first.state(), first.alias.as_str(), first.read), (v1::PipeReportState::Known, "A1", true));
    assert!(first.values.iter().any(|value| value.field == "steps" && value.items == ["Open the app", "Tap Settings"]));
    let refusals: Vec<_> = page.reports.iter().filter(|report| report.state() == v1::PipeReportState::Refused).collect();
    assert_eq!(refusals.len(), 2);
    assert!(refusals.iter().all(|report| report.values.is_empty()));
    assert_eq!(refusals.iter().map(|report| report.refused_field.as_str()).collect::<Vec<_>>(), ["severity", ""]);
    let counts = shown(&view, &pipe).and_then(|pipe| pipe.reports).expect("counts");
    assert_eq!((counts.today, counts.room_today, counts.refused_today), (1, 5, 2));
    assert_eq!(list(&mut client, &mut view, "", "ingrid").await.reports.len(), 1);
}

#[tokio::test]
async fn a_changed_forward_makes_unread_reports_wait_for_the_person() {
    let (_dir, factory, mut client, mut view, pipe) = setup().await;
    pipes::handoff::set_wake_every(&factory, std::time::Duration::from_secs(3600));
    apply_request(&mut client, &mut view, forward(&pipe, "ingrid", true)).await;
    view.follow(&mut client, |view| view.agent("ingrid").reads_reports).await;
    // The first wakes it at once, the second waits for the hour.
    assert_eq!(call(&mut client, &mut view, "liaison", "handoff_submit", &report("One")).await, "accepted #1");
    assert_eq!(call(&mut client, &mut view, "liaison", "handoff_submit", &report("Two")).await, "accepted #2");

    // To no one: the liaison can't forward, and #2 waits; it never goes to
    // the next reader by itself.
    apply_request(&mut client, &mut view, forward(&pipe, "", false)).await;
    let said = call(&mut client, &mut view, "liaison", "handoff_submit", &report("Three")).await;
    assert!(said.contains("isn't available with your current settings"), "{said}");
    // Told, unread, or not yet told: both wait.
    let page = list(&mut client, &mut view, &pipe, "").await;
    let states: Vec<_> = page.reports.iter().filter(|report| report.number > 0).map(|report| report.state()).collect();
    assert_eq!(states, [v1::PipeReportState::Waiting, v1::PipeReportState::Waiting]);
    assert_eq!(shown(&view, &pipe).and_then(|pipe| pipe.reports).map(|counts| counts.waiting), Some(2));


    // Ingrid can't read what waits.
    let read = call(&mut client, &mut view, "ingrid", "handoff_read", r#"{"since": 0}"#).await;
    assert!(!read.contains("Two"), "{read}");

    // Only the person sends them, to the forward as it is now.
    let said = refused(&mut client, &mut view, Request::DecidePipeReports(v1::DecidePipeReports { pipe_id: pipe.clone(), send: true })).await;
    assert!(said.contains("Choose who reads"), "{said}");
    apply_request(&mut client, &mut view, forward(&pipe, "ingrid", false)).await;
    assert_eq!(shown(&view, &pipe).and_then(|pipe| pipe.forward).map(|forward| forward.revision), Some(3));
    apply_request(&mut client, &mut view, Request::DecidePipeReports(v1::DecidePipeReports { pipe_id: pipe.clone(), send: true })).await;
    let page = list(&mut client, &mut view, &pipe, "").await;
    let states: Vec<_> = page.reports.iter().filter(|report| report.number > 0).map(|report| report.state()).collect();
    assert_eq!(states, [v1::PipeReportState::New, v1::PipeReportState::New]);

    // Its history: each change, as the person's.
    let history = match apply_request(&mut client, &mut view, Request::ListNeedsYouHistory(v1::ListNeedsYouHistory::default())).await {
        server_message::Body::NeedsYouHistory(history) => history,
        other => panic!("{other:?}"),
    };
    let titles: Vec<&str> = history
        .records
        .iter()
        .filter(|record| record.kind() == v1::NeedsYouRecordKind::PipeForward)
        .map(|record| record.title.as_str())
        .collect();
    assert_eq!(
        titles,
        [
            "You set this pipe's reports to go to Ingrid",
            "You set this pipe's reports to go to no one instead of Ingrid",
            // Its types stay as the app showed them: "Bug report", so no row.
            "You set this pipe's reports to go to Ingrid",
            "You let Ingrid read reports, starting a fresh conversation",
        ]
    );
}

#[tokio::test]
async fn reports_wait_when_their_reader_moves_away_or_is_deleted() {
    let (dir, _factory, mut client, mut view, pipe) = setup().await;
    apply_request(&mut client, &mut view, forward(&pipe, "ingrid", true)).await;
    view.follow(&mut client, |view| view.agent("ingrid").reads_reports).await;
    assert_eq!(call(&mut client, &mut view, "liaison", "handoff_submit", &report("One")).await, "accepted #1");
    apply_request(&mut client, &mut view, select("ingrid")).await;
    view.follow(&mut client, |view| host_events(view).iter().any(|text| text.contains("A new report")) && idle(view, "ingrid")).await;

    // Moved out of the pipe's project: what it was told, and what comes
    // after, wait for the person; it reads none of it.
    apply_request(&mut client, &mut view, create_project("Other", &dir.join("other"))).await;
    let other = view.workspace.projects.iter().find(|project| project.name == "Other").map(|project| project.id.clone()).expect("other");
    let moved = v1::UpdateAgent { agent_id: "ingrid".into(), project_id: Some(other), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdateAgent(moved)).await;
    view.follow(&mut client, |view| shown(view, &pipe).and_then(|pipe| pipe.reports).is_some_and(|counts| counts.waiting == 1)).await;
    assert_eq!(call(&mut client, &mut view, "liaison", "handoff_submit", &report("Two")).await, "accepted #2");
    view.follow(&mut client, |view| shown(view, &pipe).and_then(|pipe| pipe.reports).is_some_and(|counts| counts.waiting == 2)).await;
    let read = call(&mut client, &mut view, "ingrid", "handoff_read", r#"{"since": 0}"#).await;
    assert!(!read.contains(MARKER), "{read}");

    // Deleted: the forward goes to no one, so an agent made later with its
    // id inherits nothing, and the history says why.
    apply_request(&mut client, &mut view, crate::common::create_agent("site", "Quinn", "")).await;
    apply_request(&mut client, &mut view, forward(&pipe, "quinn", true)).await;
    view.follow(&mut client, |view| view.agent("quinn").reads_reports).await;
    // The person sends what waited to the new reader.
    apply_request(&mut client, &mut view, Request::DecidePipeReports(v1::DecidePipeReports { pipe_id: pipe.clone(), send: true })).await;
    let page = list(&mut client, &mut view, "", "quinn").await;
    assert_eq!(page.reports.iter().map(|report| report.state()).collect::<Vec<_>>(), [v1::PipeReportState::New; 2]);
    assert_eq!(call(&mut client, &mut view, "liaison", "handoff_submit", &report("Three")).await, "accepted #3");
    apply_request(&mut client, &mut view, Request::DeleteAgent(v1::DeleteAgent { agent_id: "quinn".into() })).await;
    view.follow(&mut client, |view| shown(view, &pipe).and_then(|pipe| pipe.forward).is_some_and(|forward| forward.recipient_id.is_empty())).await;
    let page = list(&mut client, &mut view, &pipe, "").await;
    let states: Vec<_> = page.reports.iter().filter(|report| report.number > 0).map(|report| report.state()).collect();
    assert_eq!(states, [v1::PipeReportState::Waiting; 3]);
    let history = match apply_request(&mut client, &mut view, Request::ListNeedsYouHistory(v1::ListNeedsYouHistory::default())).await {
        server_message::Body::NeedsYouHistory(history) => history,
        other => panic!("{other:?}"),
    };
    let titles: Vec<&str> = history.records.iter().map(|record| record.title.as_str()).collect();
    assert!(titles.contains(&"Quinn was deleted, so this pipe's reports go to no one"), "{titles:?}");
    // Earlier rows name a deleted agent by its id, as Needs you does.
    assert!(titles.contains(&"You set this pipe's reports to go to quinn instead of Ingrid"), "{titles:?}");
}

#[tokio::test]
async fn only_an_untainted_admin_agent_forwards_reports_and_never_away_from_containment() {
    let (_dir, factory, mut client, mut view, pipe) = setup().await;
    let args = format!(r#"{{"pipe": "{pipe}", "to": "Ingrid", "make_reader": true}}"#);
    // Not Lea, even with the Pipes extension on through extension_call.
    let lea = call(&mut client, &mut view, "lea", "extension_call", &format!(r#"{{"tool": "pipes_forward", "args": {args}}}"#)).await;
    assert!(lea.contains("pipes_forward"), "{lea}");
    // Not Dev either, though it has the Pipes extension: factoryd checks.
    let dev = call(&mut client, &mut view, "dev", "pipes_forward", &args).await;
    assert!(dev.starts_with("Only an admin agent"), "{dev}");
    assert!(!view.agent("ingrid").reads_reports);

    // Rivet without make_reader is told what it would mean.
    let said = call(&mut client, &mut view, "rivet", "pipes_forward", &format!(r#"{{"pipe": "{pipe}", "to": "Ingrid"}}"#)).await;
    assert!(said.contains("doesn't read reports yet"), "{said}");
    let said = call(&mut client, &mut view, "rivet", "pipes_forward", &args).await;
    assert!(said.starts_with("Reports from the pipe") && said.contains("Ingrid now reads reports"), "{said}");
    view.follow(&mut client, |view| view.agent("ingrid").reads_reports).await;
    let set = shown(&view, &pipe).and_then(|pipe| pipe.forward).expect("forward");
    assert_eq!((set.recipient_id.as_str(), set.set_by_id.as_str()), ("ingrid", "rivet"));

    // A project's default, taken by a new pipe at revision 1.
    let said = call(&mut client, &mut view, "rivet", "pipes_forward", r#"{"project": "Site", "to": "ingrid"}"#).await;
    assert!(said.starts_with("Reports from Site's new pipes now go to Ingrid"), "{said}");
    let project = view.workspace.projects.iter().find(|project| project.id == "site").cloned().expect("site");
    assert_eq!(project.report_forward.map(|forward| forward.recipient_id), Some("ingrid".to_owned()));
    let second = pipes::open_for_test(&factory, "site", "Docs", &[], "", false);
    let adopted = pipes::handoff::forward_for_test(&factory, &second);
    assert_eq!(adopted, Some(("ingrid".to_owned(), 1, "rivet".to_owned())));

    // An agent that isn't contained is never a forward's recipient, whoever asks.
    let said = call(&mut client, &mut view, "rivet", "pipes_forward", &format!(r#"{{"pipe": "{pipe}", "to": "Lea"}}"#)).await;
    assert!(said.contains("doesn't read reports yet"), "{said}");
    // Lea is in the pipe: she can't be made a reader either.
    let said = call(&mut client, &mut view, "rivet", "pipes_forward", &format!(r#"{{"pipe": "{pipe}", "to": "Lea", "make_reader": true}}"#)).await;
    assert!(said.contains("Lea is in a pipe"), "{said}");

    // Tainted, Rivet is refused, whatever it asks.
    pipes::taint_for_test(&factory, "rivet", "browser:https://example.com");
    let said = call(&mut client, &mut view, "rivet", "pipes_forward", &format!(r#"{{"pipe": "{pipe}", "to": ""}}"#)).await;
    assert!(said.starts_with("Not while you've read a page on https://example.com"), "{said}");
    assert_eq!(shown(&view, &pipe).and_then(|pipe| pipe.forward).map(|forward| forward.recipient_id), Some("ingrid".to_owned()));

    // History says who did it, and that Ingrid kept her conversation.
    let history = match apply_request(&mut client, &mut view, Request::ListNeedsYouHistory(v1::ListNeedsYouHistory::default())).await {
        server_message::Body::NeedsYouHistory(history) => history,
        other => panic!("{other:?}"),
    };
    let forwards: Vec<_> = history.records.iter().filter(|record| record.kind() == v1::NeedsYouRecordKind::PipeForward).collect();
    assert!(forwards.iter().all(|record| record.agent_id == "rivet" && record.outcome() == v1::NeedsYouOutcome::Changed));
    let titles: Vec<&str> = forwards.iter().map(|record| record.title.as_str()).collect();
    assert!(titles.contains(&"Rivet let Ingrid read reports (Ingrid's earlier conversation is kept)"), "{titles:?}");
    assert!(titles.contains(&"Rivet set this pipe's reports to go to Ingrid"), "{titles:?}");
    assert!(titles.contains(&"Rivet set Site's new pipes to forward reports to Ingrid"), "{titles:?}");

    // Only the person lets a reader go, and not while reports go to it.
    let said = refused(&mut client, &mut view, Request::SetReportReader(v1::SetReportReader { agent_id: "ingrid".into(), reads: false, fresh: true })).await;
    // It says where, in the app's words (Abbacchio's row 645, A).
    assert_eq!(said, "Ingrid still gets reports from Docs, Website ⇄ Marketing and Site's new pipes. Choose who reads them there first.");
}

/// A closed pipe forwards nothing, so it doesn't keep its reader from being
/// let go; and a reader let go with a fresh start isn't told that what it
/// read is still in its conversation (Abbacchio's row 645, B).
#[tokio::test]
async fn a_closed_pipe_doesnt_hold_its_reader_and_a_fresh_start_says_so() {
    let (_dir, _factory, mut client, mut view, pipe) = setup().await;
    apply_request(&mut client, &mut view, forward(&pipe, "ingrid", true)).await;
    view.follow(&mut client, |view| view.agent("ingrid").reads_reports).await;
    let close = v1::SetPipeState { pipe_id: pipe.clone(), action: v1::PipeAction::Close as i32 };
    apply_request(&mut client, &mut view, Request::SetPipeState(close)).await;
    let release = v1::SetReportReader { agent_id: "ingrid".into(), reads: false, fresh: true };
    apply_request(&mut client, &mut view, Request::SetReportReader(release)).await;
    view.follow(&mut client, |view| !view.agent("ingrid").reads_reports).await;
    apply_request(&mut client, &mut view, select("ingrid")).await;
    view.follow(&mut client, |view| host_events(view).iter().any(|text| text.contains("no longer limits you to reading reports"))).await;
    let told = host_events(&view).into_iter().find(|text| text.contains("no longer limits you")).expect("told");
    assert!(told.ends_with("Your conversation started fresh, so nothing you read is in it."), "{told}");
}

#[tokio::test]
async fn a_reports_files_are_copied_out_of_the_inbox_and_open_only_for_its_reader() {
    let (dir, factory, mut client, mut view, pipe) = setup().await;
    apply_request(&mut client, &mut view, forward(&pipe, "ingrid", true)).await;
    view.follow(&mut client, |view| view.agent("ingrid").reads_reports).await;
    let inbox = crate::pipes::inbox(&dir);
    std::fs::create_dir_all(&inbox).expect("inbox");
    let png = b"\x89PNG\r\n\x1a\n-not-really-an-image".to_vec();
    std::fs::write(inbox.join("shot\u{202E}gnp.png"), &png).expect("shot");
    std::fs::write(inbox.join("log.txt"), format!("panic at settings.rs {MARKER}-log")).expect("log");
    std::fs::write(dir.join("site/private.txt"), "private").expect("private");
    std::fs::write(inbox.join("big.bin"), vec![0u8; 2 * 1024 * 1024 + 1]).expect("big");
    #[cfg(unix)]
    std::os::unix::fs::symlink(dir.join("site/private.txt"), inbox.join("link.txt")).expect("link");

    let with = |files: serde_json::Value| {
        let mut report: serde_json::Value = serde_json::from_str(&report(&format!("Files {files}"))).expect("json");
        report["attachments"] = files;
        report.to_string()
    };
    for (files, code) in [
        (serde_json::json!(["../../private.txt"]), "refused: invalid attachments"),
        (serde_json::json!(["link.txt"]), "refused: invalid attachments"),
        (serde_json::json!(["big.bin"]), "refused: too_large attachments"),
        (serde_json::json!(["a", "b", "c", "d"]), "refused: too_many attachments"),
    ] {
        assert_eq!(call(&mut client, &mut view, "liaison", "handoff_submit", &with(files)).await, code);
    }
    let said = call(&mut client, &mut view, "liaison", "handoff_submit", &with(serde_json::json!(["shot\u{202E}gnp.png", "log.txt"]))).await;
    assert_eq!(said, "accepted #1");

    // Copied out of the project, under Claktory's names.
    let page = list(&mut client, &mut view, &pipe, "").await;
    let files = page.reports.iter().find(|report| report.number == 1).expect("#1").attachments.clone();
    assert_eq!(files.iter().map(|file| (file.name.as_str(), file.image)).collect::<Vec<_>>(), [("shotgnp.png", true), ("log.txt", false)]);
    let store = dir.join("home/pipes/reports").join(&pipe);
    for file in &files {
        let path = std::path::Path::new(&file.path);
        assert!(path.starts_with(&store) && path.exists(), "{}", file.path);
        let name = path.file_name().expect("name").to_string_lossy().into_owned();
        assert!(name.len() >= 64 && name.chars().take(64).all(|c| c.is_ascii_hexdigit()), "{name}");
    }
    assert!(!files.iter().any(|file| file.path.contains("site/")));

    // Facts say how many; opening one shows it, fenced, and marks no one.
    apply_request(&mut client, &mut view, select("ingrid")).await;
    view.follow(&mut client, |view| idle(view, "ingrid")).await;
    let facts = call(&mut client, &mut view, "ingrid", "handoff_read", r#"{"text": false, "since": 0}"#).await;
    assert!(facts.contains("files: 2") && !facts.contains("log.txt"), "{facts}");
    assert_eq!(pipes::taint_of_for_test(&factory, "ingrid"), "{}");
    let id = page.reports.iter().find(|report| report.number == 1).expect("#1").id;
    let log = call(&mut client, &mut view, "ingrid", "handoff_read", &format!(r#"{{"report": {id}, "file": 2}}"#)).await;
    assert!(log.contains(&format!("{MARKER}-log")) && log.contains("untrusted"), "{log}");
    assert_eq!(pipes::taint_of_for_test(&factory, "ingrid"), "{}");
    let shot = call(&mut client, &mut view, "ingrid", "handoff_read", &format!(r#"{{"report": {id}, "file": 1}}"#)).await;
    assert!(shot.contains("An image") && shot.contains("shotgnp.png"), "{shot}");
    // Not another agent's: Lea has no report tools at all.
    let lea = call(&mut client, &mut view, "lea", "extension_call", &format!(r#"{{"tool": "handoff_read", "args": {{"report": {id}, "file": 2}}}}"#)).await;
    assert!(!lea.contains(MARKER), "{lea}");
}

/// #471 G6 (Fugo 33607): Rivet asks on cards to send or discard a pipe's
/// waiting reports, bound to those reports and their reader, and to let a
/// reader go, fresh unless asked, refused while reports still go to it.
#[tokio::test]
async fn rivet_asks_on_cards_to_send_waiting_reports_and_to_let_a_reader_go() {
    use crate::rivet_cards::{answer, cards};
    let (_dir, factory, mut client, mut view, pipe) = setup().await;
    pipes::handoff::set_wake_every(&factory, std::time::Duration::from_secs(3600));
    apply_request(&mut client, &mut view, forward(&pipe, "ingrid", true)).await;
    view.follow(&mut client, |view| view.agent("ingrid").reads_reports).await;
    assert_eq!(call(&mut client, &mut view, "liaison", "handoff_submit", &report("One")).await, "accepted #1");
    assert_eq!(call(&mut client, &mut view, "liaison", "handoff_submit", &report("Two")).await, "accepted #2");
    apply_request(&mut client, &mut view, forward(&pipe, "", false)).await;
    view.follow(&mut client, |view| shown(view, &pipe).and_then(|pipe| pipe.reports).is_some_and(|counts| counts.waiting == 2)).await;

    // No reader now: nothing to send them to.
    let send = r#"{"pipe": "Website ⇄ Marketing", "send": true, "reason": "Matt wants Ingrid to read them."}"#;
    let said = call(&mut client, &mut view, "rivet", "pipes_reports", send).await;
    assert_eq!(said, "No agent reads that pipe's reports now: set who does first (pipes_forward). Nothing was asked.");
    apply_request(&mut client, &mut view, forward(&pipe, "ingrid", false)).await;
    let said = call(&mut client, &mut view, "rivet", "pipes_reports", send).await;
    assert!(said.starts_with("Asked "), "{said}");
    let (seq, card) = cards(&view).pop().expect("a card");
    assert_eq!(card.title, "Send the reports waiting on the pipe \"Website ⇄ Marketing\" to Ingrid");
    assert!(card.action_description.starts_with("Rivet asks to send the 2 reports waiting on the pipe"), "{}", card.action_description);
    assert!(!card.action_description.contains(MARKER), "never their words");
    assert_eq!(card.action_reason, "Matt wants Ingrid to read them.");

    // Who reads them changes first: void.
    apply_request(&mut client, &mut view, forward(&pipe, "", false)).await;
    apply_request(&mut client, &mut view, select("rivet")).await;
    let why = refused(&mut client, &mut view, answer(seq, false)).await;
    assert_eq!(why, "Who reads that pipe's reports changed since this card was asked: dismiss it, and ask again.");
    apply_request(&mut client, &mut view, forward(&pipe, "ingrid", false)).await;
    apply_request(&mut client, &mut view, answer(seq, false)).await;
    let page = list(&mut client, &mut view, &pipe, "").await;
    let states: Vec<_> = page.reports.iter().filter(|report| report.number > 0).map(|report| report.state()).collect();
    assert_eq!(states, [v1::PipeReportState::New, v1::PipeReportState::New]);

    // Ingrid still gets the pipe's reports: she can't be let go.
    let release = r#"{"agent": "Ingrid", "reason": "Matt wants her back on the site."}"#;
    let said = call(&mut client, &mut view, "rivet", "pipes_release_reader", release).await;
    assert_eq!(said, "Reports still go to Ingrid from 1 place: choose who reads them there first. Nothing was asked.");
    apply_request(&mut client, &mut view, forward(&pipe, "", false)).await;
    let said = call(&mut client, &mut view, "rivet", "pipes_release_reader", release).await;
    assert!(said.starts_with("Asked "), "{said}");
    let (seq, card) = cards(&view).pop().expect("a card");
    assert_eq!(card.title, "Let Ingrid stop reading reports");
    assert!(card.action_description.contains("Its conversation holds the reports it read, written on the other side of pipes."));
    assert!(card.action_description.ends_with("It starts a fresh conversation, the old one kept, so none of that stays in it."));
    apply_request(&mut client, &mut view, select("rivet")).await;
    apply_request(&mut client, &mut view, answer(seq, false)).await;
    view.follow(&mut client, |view| !view.agent("ingrid").reads_reports).await;

    // Tainted, Rivet asks neither.
    pipes::taint_for_test(&factory, "rivet", "browser:https://example.com");
    let said = call(&mut client, &mut view, "rivet", "pipes_reports", send).await;
    assert!(said.starts_with("Not while you've read"), "{said}");
}
