//! Find in a conversation (SearchConversation): the backend searches the
//! whole transcript, not only what the app holds, newest first, a page at a
//! time; and a pipe's timeline, which the app loads back to a match with
//! LoadPipeMessages.from_seq.

use crate::common::{Client, View, apply_request, create_agent, send_message, start, temp_dir};
use crate::pipes::{idle, select, setup};
use factory_protocol::v1::{self, ErrorCode, client_message::Body as Request, server_message::Body};

fn search(agent: &str, query: &str, before: Option<u64>, limit: u32) -> Request {
    Request::SearchConversation(v1::SearchConversation {
        agent_id: agent.into(),
        query: query.into(),
        before_seq: before,
        limit,
        ..Default::default()
    })
}

async fn matches(client: &mut Client, request: Request) -> v1::ConversationMatches {
    match client.request(request).await.body {
        Body::ConversationMatches(matches) => matches,
        other => panic!("expected matches, got {other:?}"),
    }
}

async fn error(client: &mut Client, request: Request) -> ErrorCode {
    match client.request(request).await.body {
        Body::Error(error) => error.code(),
        other => panic!("expected an error, got {other:?}"),
    }
}

fn utf16(text: &str, start: u32, length: u32) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    String::from_utf16(&units[start as usize..(start + length) as usize]).unwrap()
}

#[tokio::test]
async fn find_searches_the_whole_conversation_newest_first_a_page_at_a_time() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_agent("", "Builder", "")).await;
    apply_request(&mut client, &mut view, create_agent("", "Other", "")).await;
    apply_request(&mut client, &mut view, select("builder")).await;
    for n in 0..12 {
        let text = if n % 3 == 0 { format!("The Needle number {n}, and needle again") } else { format!("hay {n}") };
        apply_request(&mut client, &mut view, send_message("builder", &text)).await;
        view.follow(&mut client, |view| idle(view, "builder")).await;
    }

    // Each of 4 messages and its echo holds the needle twice.
    let first = matches(&mut client, search("builder", "NEEDLE", None, 3)).await;
    assert_eq!((first.query.as_str(), first.agent_id.as_str(), first.before_seq), ("NEEDLE", "builder", None));
    assert_eq!((first.total, first.total_capped, first.more), (16, false, true));
    assert_eq!(first.matches.len(), 3);
    let seqs: Vec<u64> = first.matches.iter().map(|hit| hit.seq).collect();
    assert!(seqs.windows(2).all(|pair| pair[0] > pair[1]), "newest first: {seqs:?}");
    for hit in &first.matches {
        assert_eq!(hit.hits, 2, "{hit:?}");
        assert!(hit.snippet.contains("Needle number "), "{hit:?}");
        assert!(hit.timestamp_ms > 0);
        let words: Vec<String> = hit.starts.iter().zip(&hit.lengths).map(|(start, length)| utf16(&hit.snippet, *start, *length)).collect();
        assert_eq!(words, ["Needle", "needle"], "{hit:?}");
        let entry = view.entries.iter().find(|entry| entry.seq == hit.seq).expect("a shown entry");
        assert!(matches!(entry.kind, Some(v1::entry::Kind::User(_) | v1::entry::Kind::Assistant(_))), "{entry:?}");
    }

    // Paging back: no total, and every match once.
    let mut all = seqs.clone();
    let mut before = *seqs.last().unwrap();
    loop {
        let page = matches(&mut client, search("builder", "needle", Some(before), 3)).await;
        assert_eq!(page.total, 0);
        all.extend(page.matches.iter().map(|hit| hit.seq));
        before = page.matches.last().map_or(before, |hit| hit.seq);
        if !page.more {
            break;
        }
    }
    assert_eq!(all.len(), 8, "{all:?}");
    assert!(all.windows(2).all(|pair| pair[0] > pair[1]), "{all:?}");

    // An agent that isn't loaded is read from its files: after a restart,
    // with another agent shown.
    apply_request(&mut client, &mut view, select("other")).await;
    drop(client);
    drop(factory);
    let (addr, _factory) = crate::common::start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let again = matches(&mut client, search("builder", "needle number 0", None, 0)).await;
    assert_eq!((again.total, again.matches.len(), again.more), (2, 2, false));

    // Nothing to find, and what can't be.
    let none = matches(&mut client, search("builder", "   ", None, 0)).await;
    assert!(none.matches.is_empty() && none.total == 0);
    let none = matches(&mut client, search("other", "needle", None, 0)).await;
    assert!(none.matches.is_empty() && !none.more);
    assert_eq!(error(&mut client, search("nobody", "needle", None, 0)).await, ErrorCode::NotFound);
    assert_eq!(error(&mut client, search("", "needle", None, 0)).await, ErrorCode::InvalidRequest);
    assert_eq!(error(&mut client, search("builder", &"x".repeat(501), None, 0)).await, ErrorCode::InvalidRequest);
    let both = Request::SearchConversation(v1::SearchConversation {
        agent_id: "builder".into(),
        pipe_id: "p".into(),
        query: "needle".into(),
        ..Default::default()
    });
    assert_eq!(error(&mut client, both).await, ErrorCode::InvalidRequest);
}

#[tokio::test]
async fn find_searches_a_pipes_timeline_and_the_app_loads_back_to_a_match() {
    let (_dir, _factory, mut client, mut view, pipe) = setup(false).await;
    for n in 0..6 {
        let text = if n == 1 { "Where's the Launch plan?".to_owned() } else { format!("note {n}") };
        apply_request(&mut client, &mut view, Request::SendPipeMessage(v1::SendPipeMessage { pipe_id: pipe.clone(), text })).await;
    }
    let request =
        Request::SearchConversation(v1::SearchConversation { pipe_id: pipe.clone(), query: "launch PLAN".into(), ..Default::default() });
    let found = matches(&mut client, request).await;
    assert_eq!((found.pipe_id.as_str(), found.total, found.matches.len()), (pipe.as_str(), 1, 1));
    let hit = &found.matches[0];
    assert_eq!(utf16(&hit.snippet, hit.starts[0], hit.lengths[0]), "Launch plan");

    // Every message from the match on, with no gap.
    let load = Request::LoadPipeMessages(v1::LoadPipeMessages { pipe_id: pipe.clone(), before_seq: 0, limit: 1, from_seq: Some(hit.seq) });
    let Body::PipeMessagePage(page) = client.request(load).await.body else { panic!("expected a page") };
    assert_eq!(page.messages.first().map(|message| message.text.as_str()), Some("Where's the Launch plan?"));
    let seqs: Vec<u64> = page.messages.iter().map(|message| message.seq).collect();
    assert!(seqs.windows(2).all(|pair| pair[1] == pair[0] + 1) && seqs.len() >= 5, "{seqs:?}");
    assert_eq!(page.has_earlier, hit.seq > 1);
}
