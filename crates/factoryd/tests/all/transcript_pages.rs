//! A long conversation reaches the app a page at a time: a snapshot (on
//! connect, select or a resync) holds only its latest entries, and the app
//! asks for earlier ones (LoadEntries) as it scrolls up, or for a whole run
//! of them to jump to an older entry.

use crate::common::{Client, View, apply_request, create_agent, send_message, start, temp_dir};
use factory_protocol::v1::{self, ErrorCode, client_message::Body as Request, server_message::Body};

fn load(agent: &str, before: u64, limit: u32, from: Option<u64>) -> Request {
    Request::LoadEntries(v1::LoadEntries { agent_id: agent.into(), before_seq: before, limit, from_seq: from })
}

async fn page(client: &mut Client, request: Request) -> v1::EntryPage {
    match client.request(request).await.body {
        Body::EntryPage(page) => page,
        other => panic!("expected a page, got {other:?}"),
    }
}

fn seqs(entries: &[v1::Entry]) -> Vec<u64> {
    entries.iter().map(|entry| entry.seq).collect()
}

#[tokio::test]
async fn a_long_conversation_comes_a_page_at_a_time() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_agent("", "Builder", "")).await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    for n in 0..130 {
        apply_request(&mut client, &mut view, send_message("builder", &format!("message {n}"))).await;
        view.follow(&mut client, |view| {
            let agent = view.agent("builder");
            agent.status() == v1::AgentStatus::Idle && agent.queued_messages == 0
        })
        .await;
    }
    let total = view.entries.last().expect("entries").seq + 1;
    assert!(total >= 260, "{total}");
    assert!(!view.has_earlier, "selected while short: it holds every entry, and what came since");

    // A new connection gets the latest 200, starting at a response's first entry.
    drop(client);
    let (mut client, _, transcript) = Client::connect(addr).await;
    let transcript = transcript.expect("the selected agent's transcript");
    assert!(transcript.has_earlier);
    assert!(transcript.entries.len() <= 200 && transcript.entries.len() >= 190, "{}", transcript.entries.len());
    assert_eq!(transcript.entries.last().map(|entry| entry.seq), Some(total - 1));
    let first = &transcript.entries[0];
    assert!(first.reply == 0 || first.reply == first.seq, "never partway through a response: {first:?}");

    // Scrolling up: page after page, until the first entry, with no gap.
    let mut held = transcript.entries.clone();
    loop {
        let before = held[0].seq;
        let earlier = page(&mut client, load("builder", before, 50, None)).await;
        assert!(!earlier.entries.is_empty() && earlier.entries.len() <= 60, "{}", earlier.entries.len());
        assert_eq!(earlier.entries.last().map(|entry| entry.seq), Some(before - 1));
        let more = earlier.has_earlier;
        held.splice(0..0, earlier.entries);
        if !more {
            break;
        }
    }
    assert_eq!(seqs(&held), (0..total).collect::<Vec<_>>());

    // Jumping to an older entry: everything from it up to what's held.
    let run = page(&mut client, load("builder", 100, 0, Some(5))).await;
    assert_eq!(seqs(&run.entries), (5..100).collect::<Vec<_>>());
    assert!(run.has_earlier);
    let start = page(&mut client, load("builder", 3, 0, Some(0))).await;
    assert_eq!((seqs(&start.entries), start.has_earlier), (vec![0, 1, 2], false));

    // What can't be.
    let beyond = client.request(load("builder", total + 1, 0, None)).await;
    assert!(matches!(beyond.body, Body::Error(ref error) if error.code() == ErrorCode::InvalidRequest), "{:?}", beyond.body);
    let nobody = client.request(load("nobody", 1, 0, None)).await;
    assert!(matches!(nobody.body, Body::Error(ref error) if error.code() == ErrorCode::NotFound), "{:?}", nobody.body);
}

#[tokio::test]
async fn a_snapshot_stops_at_about_256_kb_but_holds_a_bigger_entry_whole() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_agent("", "Builder", "")).await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    let big = "x".repeat(300 * 1024);
    for text in ["first", big.as_str()] {
        apply_request(&mut client, &mut view, send_message("builder", text)).await;
        view.follow(&mut client, |view| {
            let agent = view.agent("builder");
            agent.status() == v1::AgentStatus::Idle && agent.queued_messages == 0
        })
        .await;
    }
    drop(client);
    let (_client, _, transcript) = Client::connect(addr).await;
    let transcript = transcript.expect("transcript");
    // The echo of the big message is bigger than a page on its own: it comes alone.
    assert_eq!(transcript.entries.len(), 1, "{:?}", transcript.entries.iter().map(|entry| entry.seq).collect::<Vec<_>>());
    assert!(transcript.has_earlier);
    assert!(prost::Message::encoded_len(&transcript) > 300 * 1024);
}
