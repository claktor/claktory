//! History (#458): every change to the Map is kept in `map_history`, as it
//! happens, whatever made it.

use crate::common::{Client, View, apply_request, create_agent, create_project, start, temp_dir};
use factory_protocol::v1::{self, client_message, server_message::Body};

/// The rows recorded: when, kind, node or edge, data, why.
fn rows(dir: &std::path::Path) -> Vec<(i64, String, String, serde_json::Value, String)> {
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
    let mut statement =
        db.prepare("SELECT at, kind, coalesce(node, edge), data, why FROM map_history WHERE inferred = 0 ORDER BY seq").expect("prepare");
    statement
        .query_map([], |row| {
            let data: String = row.get(3)?;
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, serde_json::from_str(&data).expect("json"), row.get(4)?))
        })
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows")
}

fn ack(reply: Body) {
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
}

#[tokio::test]
async fn every_change_to_the_map_is_kept_with_names_from_the_first_on() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    ack(apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await);
    ack(apply_request(&mut client, &mut view, create_agent("site", "Mike", "")).await);
    ack(apply_request(&mut client, &mut view, create_agent("site", "Sam", "")).await);

    // The first recording is the Map as it was then; since is its time.
    let recorded = rows(&dir);
    let since = recorded[0].0;
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
    let (meta, backfilled): (i64, i64) =
        db.query_row("SELECT since, backfilled FROM map_history_meta", [], |row| Ok((row.get(0)?, row.get(1)?))).expect("meta");
    assert_eq!((meta, backfilled), (since, 0));
    assert!(recorded.iter().any(|row| row.0 == since && row.2 == "workspace" && row.4 == "start"), "{recorded:?}");
    let sam = recorded.iter().find(|row| row.1 == "node_add" && row.2 == "agent:sam").expect("Sam recorded");
    assert_eq!((sam.3["kind"].as_str(), sam.3["label"].as_str(), sam.3["parent_id"].as_str()), (Some("agent"), Some("Sam"), Some("project:site")));
    assert!(recorded.iter().any(|row| row.1 == "edge_add" && row.2 == "agent:mike|agent:sam|messages"), "{recorded:?}");

    // A rename, then a lead: each recorded once, with only what changed.
    let rename = v1::UpdateAgent { agent_id: "sam".into(), name: Some("Samantha".into()), ..Default::default() };
    ack(apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(rename)).await);
    let lead = v1::UpdateAgent { agent_id: "sam".into(), lead_id: Some("mike".into()), ..Default::default() };
    ack(apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(lead)).await);
    let after = rows(&dir).split_off(recorded.len());
    let changes: Vec<_> = after.iter().map(|row| (row.1.as_str(), row.2.as_str(), row.4.as_str())).collect();
    assert_eq!(
        changes,
        [
            ("node_change", "agent:sam", "config"),
            ("node_change", "agent:sam", "config"),
            ("edge_add", "agent:sam|agent:mike|lead", "config"),
        ],
        "{after:?}"
    );
    assert_eq!(after[0].3, serde_json::json!({"label": "Samantha"}));
    assert_eq!(after[1].3, serde_json::json!({"parent_id": "agent:mike"}), "nested under its lead");

    // Deleted: its node goes, its name stays in what was recorded.
    let count = rows(&dir).len();
    ack(apply_request(&mut client, &mut view, client_message::Body::DeleteAgent(v1::DeleteAgent { agent_id: "sam".into() })).await);
    let gone = rows(&dir).split_off(count);
    assert!(gone.iter().any(|row| row.1 == "node_remove" && row.2 == "agent:sam"), "{gone:?}");
    assert!(gone.iter().any(|row| row.1 == "edge_remove" && row.2 == "agent:sam|agent:mike|lead"), "{gone:?}");

    // Restarted, it carries on from what it recorded: nothing again.
    let count = rows(&dir).len();
    drop(client);
    let (restarted, _again) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(restarted).await;
    let mut view = View { workspace, ..View::default() };
    ack(apply_request(&mut client, &mut view, create_agent("site", "Zoe", "")).await);
    let later = rows(&dir).split_off(count);
    assert!(later.iter().all(|row| row.2.contains("zoe")), "{later:?}");
    assert!(later.iter().any(|row| row.1 == "node_add" && row.2 == "agent:zoe" && row.4 == "config"), "{later:?}");
}

/// Events as (when, kind, from, to), nodes by id ("" the person).
fn decoded(page: &v1::HistoryEvents, nodes: &[v1::HistoryNode]) -> Vec<(i64, v1::HistoryEventKind, String, Vec<String>, u32)> {
    let id = |at: u32| if at == 0 { String::new() } else { nodes[at as usize - 1].node.clone().expect("node").id };
    let (mut at, mut to) = (page.base_ms, page.to.iter());
    (0..page.dt.len())
        .map(|n| {
            at += page.dt[n];
            let recipients = (0..page.to_count[n]).map(|_| id(*to.next().expect("to"))).collect();
            (at, v1::HistoryEventKind::try_from(page.kind[n]).expect("kind"), id(page.from[n]), recipients, page.flags[n])
        })
        .collect()
}


async fn overview(client: &mut Client) -> v1::HistoryOverview {
    project_overview(client, "").await
}

async fn project_overview(client: &mut Client, project: &str) -> v1::HistoryOverview {
    match client.request(client_message::Body::GetHistoryOverview(v1::GetHistoryOverview { project_id: project.into() })).await.body {
        Body::HistoryOverview(overview) => overview,
        other => panic!("expected the overview, got {other:?}"),
    }
}

async fn page(client: &mut Client, after: Vec<u8>, limit: u32, runs: bool) -> v1::HistoryEvents {
    project_page(client, after, limit, runs, "").await
}

async fn project_page(client: &mut Client, after: Vec<u8>, limit: u32, runs: bool, project: &str) -> v1::HistoryEvents {
    let ask = v1::GetHistoryEvents { after_cursor: after, until_ms: 0, limit, include_runs: runs, project_id: project.into() };
    match client.request(client_message::Body::GetHistoryEvents(ask)).await.body {
        Body::HistoryEvents(page) => page,
        other => panic!("expected events, got {other:?}"),
    }
}

fn now() -> i64 {
    i64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("time").as_millis()).expect("ms")
}

const WORDS: &str = "NEVER-THESE-WORDS";

/// Messages of every kind, a card, a proposal, a pipe message and a run,
/// from `t0` on, each with words that must never come out.
fn what_happened(dir: &std::path::Path, t0: i64) {
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
    let message = |agent: &str, source: &str, sender: &str, sender_id: &str, at: i64, send_id: Option<&str>, extra: (&str, i64, &str)| {
        let (intent, quiet, reply_to) = extra;
        db.execute(
            "INSERT INTO messages (agent, source, sender, sender_id, summary, text, created, send_id, intent, quiet, reply_to)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7, nullif(?8, ''), ?9, nullif(?10, ''))",
            rusqlite::params![agent, source, sender, sender_id, WORDS, at, send_id, intent, quiet, reply_to],
        )
        .expect("message");
    };
    let none = ("", 0, "");
    message("mike", "person", "", "", t0, None, none);
    // A list send, quiet, a handoff: one event to both.
    message("sam", "agent", "Mike", "mike", t0 + 1000, Some("s1"), ("handoff", 1, ""));
    message("zoe", "agent", "Mike", "mike", t0 + 1000, Some("s1"), ("handoff", 1, ""));
    // One from before send_id: the same words to two within a second.
    message("mike", "agent", "Sam", "sam", t0 + 2000, None, none);
    message("zoe", "agent", "Sam", "sam", t0 + 2400, None, none);
    // From an agent deleted before History recorded it.
    message("mike", "agent", "Ghosty", "ghost", t0 + 3000, None, none);
    message("mike", "task", "t1", "t1", t0 + 4000, None, none);
    message("mike", "monitor", "m1", "m1", t0 + 5000, None, none);
    message("mike", "factory", "Claktory", "", t0 + 6000, None, none);
    message("mike", "app", "Telegram", "telegram", t0 + 7000, None, none);
    message("mike", "pipe", "Their Claktory", "pipe:p9", t0 + 8000, None, none);
    message("mike", "agent", "Zoe", "zoe", t0 + 9000, None, ("decision", 0, "41"));
    db.execute(
        "INSERT INTO asks (agent, title, body, wants_text, created, state, answer) VALUES ('mike', ?1, ?1, 0, ?2, 'pending', '')",
        rusqlite::params![WORDS, t0 + 10_000],
    )
    .expect("card");
    db.execute(
        "INSERT INTO proposals (agent, text, created, state, comment) VALUES ('sam', ?1, ?2, 'pending', '')",
        rusqlite::params![WORDS, t0 + 11_000],
    )
    .expect("proposal");
    db.execute(
        "INSERT INTO pipe_messages (pipe, seq, msg_id, created, author, name, role, agent, text, files, state, note)
         VALUES ('p9', 1, 'x', ?2, 'my_agent', ?1, ?1, 'mike', ?1, '[]', 'delivered', '')",
        rusqlite::params![WORDS, t0 + 12_000],
    )
    .expect("pipe message");
    db.execute(
        "INSERT INTO runs (agent, version, started, ended, outcome) VALUES ('mike', 'v0.1.99', ?1, ?2, 'finished')",
        rusqlite::params![t0, t0 + 60_000],
    )
    .expect("run");
    db.execute(
        "INSERT INTO checklist_goal_events (agent, goal, time, actor, kind, value) VALUES ('mike', 1, ?1, 'mike', 'status', 'done')",
        rusqlite::params![t0 + 13_000],
    )
    .expect("goal");
}

#[tokio::test]
async fn history_plays_back_who_wrote_to_whom_and_never_what() {
    use v1::HistoryEventKind as K;
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    ack(apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await);
    for name in ["Mike", "Sam", "Zoe"] {
        ack(apply_request(&mut client, &mut view, create_agent("site", name, "")).await);
    }
    let t0 = now() - 3 * 3_600_000;
    let before = overview(&mut client).await;
    what_happened(&dir, t0);

    let overview = overview(&mut client).await;
    assert_eq!(overview.message_count - before.message_count, 13, "{overview:?}");
    assert_eq!(overview.histogram.iter().map(|count| u64::from(*count)).sum::<u64>(), overview.message_count);
    assert!(overview.start_ms <= t0 && overview.recording_since_ms > t0);
    let node = |id: &str| overview.nodes.iter().find(|node| node.node.as_ref().is_some_and(|node| node.id == id)).expect(id).clone();
    // Gone before it was recorded: under the name it signed, reconstructed.
    let ghost = node("agent:ghost");
    assert_eq!(ghost.node.as_ref().map(|node| node.label.as_str()), Some("Ghosty"));
    assert!(ghost.present.iter().all(|interval| interval.inferred), "{ghost:?}");
    let pipe = node("pipe:p9");
    assert_eq!(pipe.node.as_ref().map(|node| (node.label.as_str(), node.detail.as_str())), Some(("Pipe", "Another Claktory")));
    let mike = node("agent:mike");
    assert!(mike.present.iter().any(|interval| interval.to_ms == 0 && !interval.inferred), "{mike:?}");
    let kinds: Vec<_> = overview.chapters.iter().map(|chapter| chapter.kind()).collect();
    for kind in [v1::HistoryChapterKind::Recording, v1::HistoryChapterKind::Project, v1::HistoryChapterKind::Agents, v1::HistoryChapterKind::Release] {
        assert!(kinds.contains(&kind), "{kind:?}: {:?}", overview.chapters);
    }
    let goal = overview.chapters.iter().find(|chapter| chapter.kind() == v1::HistoryChapterKind::Goal).expect("a goal");
    assert_eq!(goal.title, "Mike finished a goal");
    assert_eq!(goal.node_ids, ["agent:mike"]);

    // Page by page, three at a time: every event once, in order.
    let mut events = Vec::new();
    let mut after = Vec::new();
    let mut pages = Vec::new();
    loop {
        let page = page(&mut client, after, 3, false).await;
        assert!(page.dt.len() <= 3);
        events.extend(decoded(&page, &overview.nodes));
        after = page.next_cursor.clone();
        pages.push(page);
        if after.is_empty() {
            break;
        }
    }
    assert_eq!(events.len() as u64, overview.message_count);
    let ours: Vec<_> = events.iter().filter(|event| event.0 >= t0 && event.0 < t0 + 60_000).cloned().collect();
    let agent = |id: &str| format!("agent:{id}");
    let expect = [
        (t0, K::Person, String::new(), vec![agent("mike")], 0),
        (t0 + 1000, K::Agent, agent("mike"), vec![agent("sam"), agent("zoe")], 1 | (1 << 2)),
        (t0 + 2000, K::Agent, agent("sam"), vec![agent("mike"), agent("zoe")], 0),
        (t0 + 3000, K::Agent, agent("ghost"), vec![agent("mike")], 0),
        (t0 + 4000, K::Task, String::new(), vec![agent("mike")], 0),
        (t0 + 5000, K::Monitor, String::new(), vec![agent("mike")], 0),
        (t0 + 6000, K::Claktory, String::new(), vec![agent("mike")], 0),
        (t0 + 7000, K::App, "app:telegram".to_owned(), vec![agent("mike")], 0),
        (t0 + 8000, K::PipeIn, "pipe:p9".to_owned(), vec![agent("mike")], 0),
        (t0 + 9000, K::Agent, agent("zoe"), vec![agent("mike")], 2 | (3 << 2)),
        (t0 + 10_000, K::Card, agent("mike"), vec![String::new()], 0),
        (t0 + 11_000, K::Proposal, agent("sam"), vec![String::new()], 0),
        (t0 + 12_000, K::PipeOut, agent("mike"), vec!["pipe:p9".to_owned()], 0),
    ];
    assert_eq!(ours, expect);

    // With runs: when Mike worked, and for how long.
    let with_runs = page(&mut client, Vec::new(), 0, true).await;
    let at = with_runs.run_node.iter().position(|at| overview.nodes[*at as usize - 1].node.as_ref().is_some_and(|node| node.id == "agent:mike"));
    let at = at.expect("Mike's run");
    assert_eq!(with_runs.run_ms[at], 60_000);
    assert_eq!(with_runs.base_ms + with_runs.run_start[..=at].iter().sum::<i64>(), t0);

    // Nothing that was said, asked or written on a pipe ever comes out.
    let all: Vec<u8> = std::iter::once(prost::Message::encode_to_vec(&overview))
        .chain(pages.iter().map(prost::Message::encode_to_vec))
        .chain(std::iter::once(prost::Message::encode_to_vec(&with_runs)))
        .flatten()
        .collect();
    assert!(!String::from_utf8_lossy(&all).contains(WORDS));

    // A node keeps its place; a new one comes at the end.
    ack(apply_request(&mut client, &mut view, create_agent("site", "Lena", "")).await);
    let again = self::overview(&mut client).await;
    let ids = |overview: &v1::HistoryOverview| -> Vec<String> { overview.nodes.iter().map(|node| node.node.clone().unwrap_or_default().id).collect() };
    assert_eq!(ids(&again)[..overview.nodes.len()], ids(&overview)[..]);
    assert_eq!(ids(&again).last().map(String::as_str), Some("agent:lena"));
    assert!(again.history_version > overview.history_version);

    // Watching: what's new comes by itself.
    client.ok(client_message::Body::WatchHistory(v1::WatchHistory { on: true, project_id: String::new() })).await;
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
    db.execute(
        "INSERT INTO messages (agent, source, sender, sender_id, summary, text, created) VALUES ('zoe', 'person', '', '', '', ?1, ?2)",
        rusqlite::params![WORDS, now()],
    )
    .expect("message");
    let pushed = client.wait_for(|body| if let Body::HistoryEvents(page) = body { Some(page.clone()) } else { None }).await;
    let pushed = decoded(&pushed, &again.nodes);
    assert_eq!(pushed.iter().map(|event| (event.1, event.3.clone())).collect::<Vec<_>>(), [(K::Person, vec![agent("zoe")])]);
    client.ok(client_message::Body::WatchHistory(v1::WatchHistory { on: false, project_id: String::new() })).await;
}

/// A chat is the person's own conversation, never part of History (#473):
/// not a node, not one of who joined, not an event or a run, not a goal;
/// one placed before keeps its place, empty. A dev build isn't a release.
#[tokio::test]
async fn a_chat_is_never_in_history() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    ack(apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await);
    ack(apply_request(&mut client, &mut view, create_agent("site", "Mike", "")).await);
    let before = overview(&mut client).await;
    let t0 = now() - 3_600_000;
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
    let message = |agent: &str, source: &str, at: i64| {
        db.execute(
            "INSERT INTO messages (agent, source, sender, sender_id, summary, text, created) VALUES (?1, ?2, '', '', '', 'hi', ?3)",
            rusqlite::params![agent, source, at],
        )
        .expect("message");
    };
    message("chat:1", "person", t0);
    message("chat:1", "factory", t0 + 1000);
    message("mike", "person", t0 + 2000);
    for (agent, version) in [("chat:1", "v0.1.99"), ("chat:2", "dev")] {
        db.execute(
            "INSERT INTO runs (agent, version, started, ended, outcome) VALUES (?1, ?2, ?3, ?4, 'finished')",
            rusqlite::params![agent, version, t0, t0 + 5000],
        )
        .expect("run");
    }
    db.execute(
        "INSERT INTO asks (agent, title, body, wants_text, created, state, answer) VALUES ('chat:1', 't', 'b', 0, ?1, 'pending', '')",
        [t0 + 3000],
    )
    .expect("card");
    db.execute(
        "INSERT INTO checklist_goal_events (agent, goal, time, actor, kind, value) VALUES ('chat:1', 1, ?1, 'chat:1', 'status', 'done')",
        [t0 + 4000],
    )
    .expect("goal");
    // As an earlier version left it: a chat placed among History's nodes.
    db.execute(
        "INSERT INTO map_history_nodes (node, idx) VALUES ('agent:chat:1', (SELECT coalesce(max(idx), 0) + 1 FROM map_history_nodes))",
        [],
    )
    .expect("placed");

    let overview = overview(&mut client).await;
    let ids: Vec<String> = overview.nodes.iter().filter_map(|node| node.node.as_ref()).map(|node| node.id.clone()).collect();
    assert!(ids.iter().all(|id| !id.contains("chat")), "{ids:?}");
    assert!(ids.contains(&"agent:mike".to_owned()), "{ids:?}");
    assert_eq!(overview.nodes.len(), before.nodes.len() + 1, "the chat's place is kept, empty: {:?}", overview.nodes);
    assert_eq!(overview.nodes.last(), Some(&v1::HistoryNode::default()));
    assert_eq!(overview.message_count - before.message_count, 1, "only Mike's message");
    for chapter in &overview.chapters {
        assert!(chapter.node_ids.iter().all(|id| !id.contains("chat")), "{chapter:?}");
        assert!(!chapter.title.contains("chat") && chapter.title != "Claktory dev", "{chapter:?}");
    }
    assert!(overview.chapters.iter().all(|chapter| chapter.kind() != v1::HistoryChapterKind::Goal), "{:?}", overview.chapters);
    let with_runs = page(&mut client, Vec::new(), 0, true).await;
    let events = decoded(&with_runs, &overview.nodes);
    assert!(events.iter().all(|event| !event.2.contains("chat") && event.3.iter().all(|to| !to.contains("chat"))), "{events:?}");
    assert!(with_runs.run_node.is_empty(), "no chat's run: {:?}", with_runs.run_node);
}

/// Every event, page by page from `after` (`limit` at a time), with runs
/// when `runs`, one project's when `project` isn't empty.
async fn every_page(client: &mut Client, nodes: &[v1::HistoryNode], after: Vec<u8>, limit: u32, runs: bool, project: &str) -> (Vec<Event>, Vec<String>) {
    let (mut events, mut run_nodes, mut after) = (Vec::new(), Vec::new(), after);
    loop {
        let page = project_page(client, after, limit, runs, project).await;
        assert!(limit == 0 || page.dt.len() <= limit as usize);
        events.extend(decoded(&page, nodes));
        run_nodes.extend(page.run_node.iter().map(|at| nodes[*at as usize - 1].node.clone().expect("node").id));
        after = page.next_cursor;
        if after.is_empty() {
            return (events, run_nodes);
        }
    }
}

type Event = (i64, v1::HistoryEventKind, String, Vec<String>, u32);

/// Pages come in time order even where a batch of list sends makes fewer
/// events than the others' rows: the messages not read yet came after the
/// cards read past them before.
#[tokio::test]
async fn pages_keep_time_order_through_list_sends() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let t0 = now() - 3 * 3_600_000;
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
    for send in 0..60 {
        for to in ["a1", "a2", "a3", "a4", "a5"] {
            db.execute(
                "INSERT INTO messages (agent, source, sender, sender_id, summary, text, created, send_id) VALUES (?1, 'agent', 'Mike', 'mike', '', 'hi', ?2, ?3)",
                rusqlite::params![to, t0 + send * 1000, format!("s{send}")],
            )
            .expect("message");
        }
        db.execute(
            "INSERT INTO asks (agent, title, body, wants_text, created, state, answer) VALUES ('mike', 't', 'b', 0, ?1, 'pending', '')",
            [t0 + send * 1000 + 500],
        )
        .expect("card");
    }
    let overview = overview(&mut client).await;
    let (events, _) = every_page(&mut client, &overview.nodes, Vec::new(), 100, false, "").await;
    let ours: Vec<_> = events.iter().filter(|event| event.0 >= t0 && event.0 < t0 + 60_000).collect();
    assert_eq!(ours.len(), 120, "60 list sends of 5, one event each, and 60 cards");
    let times: Vec<i64> = events.iter().map(|event| event.0).collect();
    assert!(times.is_sorted(), "{times:?}");
    assert_eq!(events.len() as u64, overview.message_count);
}

/// One project's History: the events its agents sent or got while in it
/// (a moved agent's in its project of the time), its runs, its histogram,
/// count and cursors; the nodes stay whole. And it's counted again from
/// what was kept when something new comes.
#[tokio::test]
async fn one_projects_history_is_its_agents_events_at_the_time() {
    use v1::HistoryEventKind as K;
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    ack(apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await);
    ack(apply_request(&mut client, &mut view, create_project("Lab", &dir.join("lab"))).await);
    for (project, name) in [("site", "Mike"), ("site", "Sam"), ("lab", "Zoe")] {
        ack(apply_request(&mut client, &mut view, create_agent(project, name, "")).await);
    }
    let all_before = overview(&mut client).await;
    let site_before = project_overview(&mut client, "site").await;
    let lab_before = project_overview(&mut client, "lab").await;
    let t0 = now() - 3 * 3_600_000;
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
    let message = |agent: &str, source: &str, sender_id: &str, at: i64, send_id: Option<&str>| {
        db.execute(
            "INSERT INTO messages (agent, source, sender, sender_id, summary, text, created, send_id) VALUES (?1, ?2, '', ?3, '', 'hi', ?4, ?5)",
            rusqlite::params![agent, source, sender_id, at, send_id],
        )
        .expect("message");
    };
    let run = |agent: &str, at: i64| {
        db.execute("INSERT INTO runs (agent, version, started, ended, outcome) VALUES (?1, 'dev', ?2, ?3, 'finished')", rusqlite::params![agent, at, at + 500])
            .expect("run");
    };
    message("sam", "agent", "mike", t0, Some("s1"));
    message("zoe", "person", "", t0 + 1000, None);
    // A list send across projects: one event, in both, whole.
    message("sam", "agent", "mike", t0 + 2000, Some("s2"));
    message("zoe", "agent", "mike", t0 + 2000, Some("s2"));
    message("mike", "task", "t1", t0 + 3000, None);
    run("zoe", t0 + 1000);
    run("mike", t0 + 3000);

    let agent = |id: &str| format!("agent:{id}");
    let site = project_overview(&mut client, "site").await;
    let lab = project_overview(&mut client, "lab").await;
    let all = overview(&mut client).await;
    assert_eq!(all.message_count - all_before.message_count, 4);
    assert_eq!(site.message_count - site_before.message_count, 3);
    assert_eq!(lab.message_count - lab_before.message_count, 2);
    for overview in [&site, &lab] {
        assert_eq!(overview.nodes, all.nodes, "the nodes stay whole");
        assert_eq!(overview.start_ms, all.start_ms);
        assert_eq!(overview.histogram.len(), all.histogram.len());
        assert_eq!(overview.histogram.iter().map(|count| u64::from(*count)).sum::<u64>(), overview.message_count);
    }
    let (events, runs) = every_page(&mut client, &site.nodes, site.cursors[0].cursor.clone(), 2, true, "site").await;
    let ours: Vec<_> = events.iter().filter(|event| event.0 >= t0 && event.0 < t0 + 60_000).map(|event| (event.0, event.1, event.3.clone())).collect();
    assert_eq!(
        ours,
        [(t0, K::Agent, vec![agent("sam")]), (t0 + 2000, K::Agent, vec![agent("sam"), agent("zoe")]), (t0 + 3000, K::Task, vec![agent("mike")])]
    );
    assert_eq!(events.len() as u64, site.message_count);
    assert_eq!(runs, [agent("mike")]);
    let (events, runs) = every_page(&mut client, &lab.nodes, Vec::new(), 0, true, "lab").await;
    let ours: Vec<_> = events.iter().filter(|event| event.0 >= t0 && event.0 < t0 + 60_000).map(|event| (event.0, event.1)).collect();
    assert_eq!(ours, [(t0 + 1000, K::Person), (t0 + 2000, K::Agent)]);
    assert_eq!(runs, [agent("zoe")]);

    // Zoe moves to Site: from then on what she gets is Site's; before, Lab's.
    let moved = v1::UpdateAgent { agent_id: "zoe".into(), project_id: Some("site".into()), ..Default::default() };
    ack(apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(moved)).await);
    let after_move = now() + 1;
    message("zoe", "person", "", after_move, None);
    let site_now = project_overview(&mut client, "site").await;
    let lab_now = project_overview(&mut client, "lab").await;
    assert_eq!(site_now.message_count, site.message_count + 1);
    assert_eq!(lab_now.message_count, lab.message_count);
    let all_now = overview(&mut client).await;
    assert_eq!(all_now.message_count, all.message_count + 1, "counted on from what was kept");

    // Watching one project: another's events aren't sent.
    client.ok(client_message::Body::WatchHistory(v1::WatchHistory { on: true, project_id: "lab".into() })).await;
    message("mike", "person", "", now(), None);
    message("zoe", "person", "", now(), None);
    let lab_watch = project_overview(&mut client, "lab").await;
    assert_eq!(lab_watch.message_count, lab.message_count, "neither is Lab's now");
    // Past the watch's next look (every 2 s): had it sent them, they'd come
    // before Site's below.
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
    client.ok(client_message::Body::WatchHistory(v1::WatchHistory { on: true, project_id: "site".into() })).await;
    message("sam", "person", "", now(), None);
    let pushed = client.wait_for(|body| if let Body::HistoryEvents(page) = body { Some(page.clone()) } else { None }).await;
    let pushed = decoded(&pushed, &all_now.nodes);
    assert_eq!(pushed.iter().map(|event| event.3.clone()).collect::<Vec<_>>(), [vec![agent("sam")]]);
}

/// A while after a start every event is counted ahead, so the first
/// overview reads only what's new; and it counts what came meanwhile.
#[tokio::test]
async fn the_events_are_counted_ahead_after_a_start() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let t0 = now() - 3_600_000;
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
    let message = |at: i64| {
        db.execute("INSERT INTO messages (agent, source, sender, sender_id, summary, text, created) VALUES ('mike', 'person', '', '', '', 'hi', ?1)", [at])
            .expect("message");
    };
    message(t0);
    message(t0 + 1000);
    assert_eq!(factoryd::history_counted_for_test(&factory), None, "nothing counted before it's time");
    let counted = factoryd::history_warm_for_test(&factory);
    assert!(counted >= 2, "{counted}");
    assert_eq!(factoryd::history_counted_for_test(&factory), Some(counted));
    message(t0 + 2000);
    let overview = overview(&mut client).await;
    assert_eq!(overview.message_count, counted + 1);
}

/// History at scale (Polpo's history-scale.md items 5 and 6): times the
/// overview, once and again, and every page, on a Claktory filled by
/// efficiency/tools/history_synth.py. Run on purpose, in release:
/// `CLAKTORY_HISTORY_SCALE=<folder whose home/factory.db is filled> cargo
/// test --release -p factoryd --test all history::at_scale -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "needs a filled factory.db (CLAKTORY_HISTORY_SCALE)"]
async fn at_scale() {
    let Some(dir) = std::env::var_os("CLAKTORY_HISTORY_SCALE").map(std::path::PathBuf::from) else { return };
    let timed = std::time::Instant::now();
    let (addr, factory) = start(&dir).await;
    println!("MEASURE started (migrations included): {:.2?}", timed.elapsed());
    let timed = std::time::Instant::now();
    let counted = factoryd::history_warm_for_test(&factory);
    println!("MEASURE warmed (late indexes built, {counted} events counted): {:.2?}", timed.elapsed());
    let (mut client, _, _) = Client::connect(addr).await;
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
    let scan = |sql: &str| {
        let timed = std::time::Instant::now();
        let rows: i64 = db.query_row(&format!("SELECT count(*) FROM ({sql})"), [], |row| row.get(0)).expect("scan");
        (rows, timed.elapsed())
    };
    // The messages' read before and after, alone.
    let before = "SELECT id, agent, source, sender_id, created, coalesce(send_id, ''), coalesce(quiet, 0), coalesce(reply_to, ''), coalesce(intent, ''),
        CASE WHEN source = 'agent' AND coalesce(send_id, '') = '' THEN length(text) || ':' || substr(text, 1, 200) ELSE '' END
        FROM messages NOT INDEXED WHERE id > 0 AND substr(agent, 1, 5) != 'chat:' ORDER BY id";
    let after = "SELECT id, agent, source, sender_id, created, coalesce(send_id, ''), coalesce(quiet, 0), coalesce(reply_to, ''), coalesce(intent, '')
        FROM messages INDEXED BY messages_history WHERE id > 0 AND substr(agent, 1, 5) != 'chat:' ORDER BY id";
    for (name, sql) in [("before", before), ("after", after), ("before", before), ("after", after)] {
        let (rows, took) = scan(sql);
        println!("MEASURE messages read {name}: {rows} rows in {took:.2?}");
    }
    for (what, project) in [("whole, warmed", ""), ("again", ""), ("project factory", "factory"), ("project site", "site")] {
        let timed = std::time::Instant::now();
        let overview = project_overview(&mut client, project).await;
        println!(
            "MEASURE overview {what}: {:.2?} ({} events, {} nodes, {} cursors, {} bytes)",
            timed.elapsed(),
            overview.message_count,
            overview.nodes.len(),
            overview.cursors.len(),
            prost::Message::encoded_len(&overview)
        );
    }
    db.execute(
        "INSERT INTO messages (agent, source, sender, sender_id, summary, text, created) VALUES ('a000', 'person', '', '', '', 'hi', ?1)",
        [now()],
    )
    .expect("message");
    let timed = std::time::Instant::now();
    let overview = overview(&mut client).await;
    println!("MEASURE overview after one more message: {:.2?}", timed.elapsed());
    for project in ["", "site"] {
        let timed = std::time::Instant::now();
        let (mut after, mut pages, mut events, mut bytes) = (Vec::new(), 0, 0, 0);
        loop {
            let page = project_page(&mut client, after, 0, true, project).await;
            pages += 1;
            events += page.dt.len();
            bytes += prost::Message::encoded_len(&page);
            after = page.next_cursor;
            if after.is_empty() {
                break;
            }
        }
        println!("MEASURE every page{}: {:.2?} ({pages} pages, {events} events, {bytes} bytes)", if project.is_empty() { String::new() } else { format!(" of {project}") }, timed.elapsed());
    }
    assert!(overview.message_count > 0);
}
