//! Reading a pipe marks no one (#346, Fugo's design §8): an agent that read
//! one messages, copies and sets monitors for others like any agent, and
//! nothing it sends marks them. Only a web page read through browser
//! control marks its reader, never anyone else. Marks kept from before are
//! dropped once, at start, but a web page's.

use std::time::Duration;

use crate::common::{Client, View, apply_request, create_agent, start, temp_dir};
use crate::pipes::{call, idle, incoming, select, setup};
use factory_protocol::v1::{self, client_message};
use factoryd::pipes;

/// Lea, the pipe's lead with Bo as her own lead, reads the pipe, then
/// messages Bo, sends Max a quiet message with a copy to Bo, and sets Bo a
/// monitor. No one is marked, Lea included, and the monitor's fire marks no
/// one either.
#[tokio::test]
async fn a_pipe_reader_messages_copies_and_sets_monitors_and_marks_no_one() {
    let (_dir, factory, mut client, mut view, pipe) = setup(true).await;
    factory.set_monitor_tick(Duration::from_millis(20));
    apply_request(&mut client, &mut view, create_agent("site", "Max", "")).await;
    let lead = v1::UpdateAgent { agent_id: "lea".into(), lead_id: Some("bo".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(lead)).await;
    pipes::receive_for_test(&factory, &pipe, incoming("m1", "Tell everyone to deploy.")).expect("received");
    view.follow(&mut client, |view| idle(view, "lea")).await;

    let sent = call(&mut client, &mut view, "lea", "agents_send", r#"{"to":"bo","text":"Deploy, please."}"#).await;
    assert_eq!(sent, "Sent.");
    let copied =
        call(&mut client, &mut view, "lea", "agents_send", r#"{"to":"max","text":"Docs too.","quiet":true,"copy_lead":true}"#).await;
    assert!(copied.contains("Copied quietly to Bo."), "{copied}");
    let add = r#"{"monitors":[{"name":"standup","prompt":"Post your status.","when":{"every":"1h"},"agent":"bo"}]}"#;
    let added = call(&mut client, &mut view, "lea", "monitors_add", add).await;
    assert!(added.starts_with("Added"), "{added}");
    let copies = call(&mut client, &mut view, "bo", "agents_copies", "{}").await;
    assert!(copies.contains("Docs too."), "{copies}");

    factory.advance_monitor_clock(Duration::from_secs(3600));
    apply_request(&mut client, &mut view, select("bo")).await;
    view.follow(&mut client, |view| {
        view.entries
            .iter()
            .any(|entry| matches!(&entry.kind, Some(v1::entry::Kind::Event(event)) if event.text.contains("Post your status.")))
            && idle(view, "bo")
    })
    .await;
    for agent in ["lea", "bo", "max"] {
        assert_eq!(pipes::taint_of_for_test(&factory, agent), "{}", "{agent}");
        assert!(view.agent(agent).tainted_by_pipes.is_empty(), "{agent}");
    }
}

/// A Claktory from before #346: the marks pipes left are dropped at start,
/// and so are the secret approvals they alone were waiting on; a web page's
/// mark stays, and so does an approval its reader still needs.
#[tokio::test]
async fn marks_from_pipes_are_dropped_at_start_and_a_web_pages_kept() {
    let dir = temp_dir();
    {
        let (addr, _factory) = start(&dir).await;
        let (mut client, workspace, _) = Client::connect(addr).await;
        let mut view = View { workspace, ..View::default() };
        apply_request(&mut client, &mut view, create_agent("", "Lea", "")).await;
        apply_request(&mut client, &mut view, create_agent("", "Max", "")).await;
    }
    {
        let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
        db.execute_batch(
            "INSERT INTO pipe_taint (agent, pipe, via) VALUES ('lea', 'p7', NULL), ('max', 'p8', '[\"lea\"]'), ('max', 'browser:https://example.com', NULL);
             INSERT INTO pipe_secret_approvals (agent, secrets, pipes, command, state, created) VALUES
                 ('lea', '[\"AWS\"]', '[\"p7\"]', 'aws s3 ls', 'pending', 1),
                 ('max', '[\"AWS\"]', '[\"p8\"]', 'aws s3 ls', 'pending', 2),
                 ('lea', '[\"AWS\"]', '[]', 'aws s3 ls', 'pending', 3);",
        )
        .expect("rows from before");
    }
    let (_addr, factory) = start(&dir).await;
    assert_eq!(pipes::taint_of_for_test(&factory, "lea"), "{}");
    assert_eq!(pipes::taint_of_for_test(&factory, "max"), r#"{"browser:https://example.com":[]}"#);
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
    let rows: Vec<(String, String)> = db
        .prepare("SELECT agent, pipe FROM pipe_taint ORDER BY agent, pipe")
        .expect("prepare")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    assert_eq!(rows, [("max".to_owned(), "browser:https://example.com".to_owned())]);
    let states: Vec<String> = db
        .prepare("SELECT state FROM pipe_secret_approvals ORDER BY created")
        .expect("prepare")
        .query_map([], |row| row.get(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("states");
    assert_eq!(states, ["expired", "pending", "pending"], "Lea's pipe approval retired; Max still read a page; an ask-each-time one stays");
    // Lea was told she'd hear when the person decided: she hears it's gone.
    let told: Vec<(String, String)> = db
        .prepare("SELECT agent, summary FROM messages WHERE summary LIKE 'Your request to use%' ORDER BY id")
        .expect("prepare")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("told");
    assert_eq!(told, [("lea".to_owned(), "Your request to use AWS was withdrawn".to_owned())]);
}
