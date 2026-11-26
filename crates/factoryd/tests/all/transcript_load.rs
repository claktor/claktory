//! Reading a conversation back from its segment files (#377): what it shows
//! and what the model is given.

use std::path::Path;

use factory_agent::{AssistantMessage, AssistantPart, Message, StopReason, Usage, UserContent, UserPart};
use factory_protocol::v1::{self, Entry, entry::Kind};
use factoryd::transcript::{Page, Transcript};
use serde_json::{Value, json};

use crate::common::{Client, View, apply_request, create_agent, start, temp_dir};
use factory_protocol::v1::{client_message::Body as Request, server_message::Body};

fn user(text: &str, image: &str, timestamp: u64) -> Value {
    json!({
        "role": "user",
        "content": [{"type": "text", "text": text}, {"type": "image", "data": image, "mimeType": "image/png"}],
        "timestamp": timestamp,
    })
}

/// An older segment is read without its images' data (step 1 of #377): it
/// shows the same, a message compaction carried over is still shown once,
/// and the newest segment, the model's context, keeps its images whole.
#[tokio::test]
async fn older_segments_show_the_same_without_their_images_data() {
    let dir = temp_dir();
    let screenshot = json!({
        "role": "toolResult",
        "toolCallId": "c1",
        "toolName": "read_image",
        "content": [{"type": "image", "data": "BBBB", "mimeType": "image/png"}],
        "isError": false,
        "timestamp": 1001,
    });
    let first = json!({"messages": [user("look", "AAAA", 1000), screenshot, user("and this", "DDDD", 1002)]});
    // Compaction kept the last prompt after its summary.
    let second = json!({
        "compaction": {"summary": "What happened", "tokens_before": 10},
        "messages": [
            {"role": "user", "content": "What happened", "timestamp": 1003},
            user("and this", "DDDD", 1002),
            user("new", "CCCC", 1004),
        ],
    });
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join("segment-0001.json"), first.to_string()).expect("write");
    std::fs::write(dir.join("segment-0002.json"), second.to_string()).expect("write");

    let transcript = Transcript::load(dir.clone(), "builder".into()).await.expect("load");
    let shown: Vec<String> = transcript
        .window()
        .1
        .iter()
        .map(|entry| match &entry.kind {
            Some(factory_protocol::v1::entry::Kind::User(user)) => format!("user: {}", user.text),
            Some(factory_protocol::v1::entry::Kind::Compaction(compaction)) => format!("compaction: {}", compaction.summary),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(shown, ["user: look\n[image]", "user: and this\n[image]", "compaction: What happened", "user: new\n[image]"]);

    // The model's context: the newest segment, images and all.
    let images: Vec<String> = transcript
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => match &user.content {
                UserContent::Parts(parts) => parts.iter().find_map(|part| match part {
                    UserPart::Image { data, .. } => data.base64().map(|data| data.to_string()),
                    UserPart::Text { .. } => None,
                }),
                UserContent::Text(_) => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(images, ["DDDD", "CCCC"]);
}

fn said(text: &str, model: &str, timestamp: u64) -> Value {
    reply(vec![AssistantPart::Text { text: text.into(), text_signature: None }], model, timestamp)
}

fn calls(id: &str, name: &str, timestamp: u64) -> Value {
    let part = AssistantPart::ToolCall { id: id.into(), name: name.into(), arguments: json!({}), thought_signature: None };
    reply(vec![part], "anthropic/claude-sonnet-4-6", timestamp)
}

fn reply(content: Vec<AssistantPart>, model: &str, timestamp: u64) -> Value {
    let (provider, model) = model.split_once('/').expect("provider/model");
    serde_json::to_value(Message::Assistant(AssistantMessage {
        content,
        api: "anthropic-messages".into(),
        provider: provider.into(),
        model: model.into(),
        usage: Usage::empty(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp,
        extra: Default::default(),
    }))
    .expect("json")
}

fn result(id: &str, name: &str, details: Value, timestamp: u64) -> Value {
    json!({
        "role": "toolResult", "toolCallId": id, "toolName": name, "content": [{"type": "text", "text": format!("done {id}")}],
        "details": details, "isError": false, "timestamp": timestamp,
    })
}

fn prompt(text: &str, timestamp: u64) -> Value {
    json!({"role": "user", "content": text, "timestamp": timestamp})
}

/// `count` exchanges of segment `number`: the person's message and a reply.
fn exchanges(number: u64, count: u64, model: &str) -> Vec<Value> {
    let at = number * 1_000_000;
    (0..count)
        .flat_map(|i| [prompt(&format!("m{number}-{i}"), at + i * 10), said(&format!("r{number}-{i}"), model, at + i * 10 + 1)])
        .collect()
}

fn segment(compacted: bool, number: u64, messages: Vec<Value>) -> Value {
    let mut messages = messages;
    if compacted {
        messages.insert(0, prompt(&format!("summary {number}"), number * 1_000_000 - 1));
        json!({"compaction": {"summary": format!("summary {number}"), "tokens_before": 10}, "messages": messages})
    } else {
        json!({"messages": messages})
    }
}

fn write(dir: &Path, number: u64, segment: &Value) {
    std::fs::write(dir.join(format!("segment-{number:04}.json")), segment.to_string()).expect("write");
}

/// A long conversation: six segments of 150 exchanges (300 entries), each
/// after the first compacted, with what crosses segments: a card, a call
/// whose result comes in the next segment, one that never gets one, the
/// last prompt carried over a compaction, and a change of model.
fn conversation(dir: &Path) {
    let sonnet = "anthropic/claude-sonnet-4-6";
    let mut one = vec![user("look", "AAAA", 1)];
    one.extend(exchanges(1, 150, sonnet));
    one.push(calls("ask-1", "ask_person", 1_900_000));
    one.push(result("ask-1", "ask_person", json!({"ask": "7", "title": "Pick one"}), 1_900_001));
    write(dir, 1, &segment(false, 1, one));
    let mut two = exchanges(2, 150, sonnet);
    two.push(calls("cross", "bash", 2_900_000));
    write(dir, 2, &segment(true, 2, two));
    let mut three = vec![result("cross", "bash", json!(null), 3_000_005)];
    three.extend(exchanges(3, 150, sonnet));
    three.push(prompt("last3", 3_900_000));
    write(dir, 3, &segment(true, 3, three));
    let mut four = vec![prompt("last3", 3_900_000)];
    four.extend(exchanges(4, 150, "anthropic/claude-opus-4-8"));
    four.push(calls("lost", "bash", 4_900_000));
    write(dir, 4, &segment(true, 4, four));
    let mut five = exchanges(5, 150, "anthropic/claude-opus-4-8");
    five.push(prompt("last5", 5_900_000));
    write(dir, 5, &segment(true, 5, five));
    let mut six = vec![prompt("last5", 5_900_000)];
    six.extend(exchanges(6, 150, sonnet));
    six.push(prompt("last6", 6_900_000));
    write(dir, 6, &segment(true, 6, six));
}

/// Every entry: the older ones read back from disk, then the ones held.
fn all(transcript: &Transcript) -> Vec<Entry> {
    let older = transcript.older();
    let mut all: Vec<Entry> = (0..older.sections()).flat_map(|at| older.section(at).expect("a section")).collect();
    all.extend(transcript.window().1.iter().cloned());
    for (index, entry) in all.iter().enumerate() {
        assert_eq!(entry.seq, index as u64);
    }
    all
}

fn page(transcript: &Transcript, before: u64, limit: u32, from: Option<u64>) -> Option<(Vec<Entry>, bool)> {
    match transcript.page(before, limit, from)? {
        Page::Here(entries, more) => Some((entries, more)),
        Page::Older(page) => Some(page.read().expect("read")),
    }
}

/// A page as Claktory made it with every entry held.
fn page_of_all(entries: &[Entry], before: u64, limit: u32, from: Option<u64>) -> (Vec<Entry>, bool) {
    let end = before as usize;
    let start = match from {
        Some(from) => (from as usize).min(end),
        None => {
            let limit = if limit == 0 { 200 } else { (limit as usize).min(1000) };
            let (mut start, mut bytes) = (end, 0);
            while start > 0 && end - start < limit {
                let size = prost::Message::encoded_len(&entries[start - 1]);
                if start < end && bytes + size > 256 * 1024 {
                    break;
                }
                bytes += size;
                start -= 1;
            }
            if let Some(first) = entries.get(start)
                && first.reply != 0
                && (first.reply as usize) < start
            {
                let extra: usize = entries[first.reply as usize..start].iter().map(prost::Message::encoded_len).sum();
                if bytes + extra <= 512 * 1024 {
                    start = first.reply as usize;
                }
            }
            start
        }
    };
    (entries[start..end].to_vec(), start > 0)
}

fn texts(entries: &[Entry]) -> Vec<String> {
    entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::User(user)) => Some(user.text.clone()),
            _ => None,
        })
        .collect()
}

fn forget_index(dir: &Path) {
    for name in ["transcript-index.json", "transcript-entries.bin"] {
        let _ = std::fs::remove_file(dir.join(name));
    }
}

/// Two conversations show the same: every entry, what waits for the
/// person, where they go back to, which model wrote each reply, and pages.
fn same(read: &Transcript, whole: &Transcript) {
    let entries = all(whole);
    assert_eq!(all(read), entries);
    assert_eq!(read.len(), whole.len());
    assert_eq!(read.waiting_seqs(), whole.waiting_seqs());
    assert_eq!(read.origin_seqs().collect::<Vec<_>>(), whole.origin_seqs().collect::<Vec<_>>());
    assert_eq!(read.recent(), whole.recent());
    for entry in &entries {
        assert_eq!(
            read.fork_point(entry.seq, Some(entry), entry.timestamp_ms),
            whole.fork_point(entry.seq, Some(entry), entry.timestamp_ms)
        );
        if entry.reply != 0 {
            assert_eq!(read.response_model(entry.reply), whole.response_model(entry.reply), "{entry:?}");
        }
    }
    let len = entries.len() as u64;
    for before in [len, len - 1, len - 150, len - 299, len - 300, len - 301, 1_000, 901, 650, 300, 200, 199, 3, 1, 0] {
        for (limit, from) in [(0, None), (1, None), (50, None), (1000, None), (0, Some(0)), (0, Some(before.saturating_sub(700)))] {
            assert_eq!(page(read, before, limit, from), Some(page_of_all(&entries, before, limit, from)), "{before} {limit} {from:?}");
        }
    }
    assert!(page(read, len + 1, 0, None).is_none());
}

/// Step 2 of #377: a restart reads only the newest segment, and the older
/// ones' entries come back from disk as they were. A conversation read from
/// its index shows exactly what it shows read whole; one that has grown since
/// (a compaction) or whose older segment changed (taken back) too.
#[tokio::test]
async fn older_segments_come_back_from_disk_as_they_were() {
    let dir = temp_dir();
    conversation(&dir);
    let whole = Transcript::load(dir.clone(), "builder".into()).await.expect("load");
    let entries = all(&whole);
    assert!(dir.join("transcript-index.json").is_file() && dir.join("transcript-entries.bin").is_file());
    // Held: the newest segment's 302 entries (a first page needs only 200).
    assert_eq!(whole.window().0, entries.len() as u64 - 302);
    let shown = texts(&entries);
    assert_eq!(shown.iter().filter(|text| *text == "last3").count(), 1, "a carried prompt shows once");
    assert_eq!(shown.iter().filter(|text| *text == "last5").count(), 1, "a carried prompt shows once");
    let call = |id: &str| {
        entries
            .iter()
            .find_map(|entry| match &entry.kind {
                Some(Kind::ToolCall(call)) if call.call_id == id => Some(call.clone()),
                _ => None,
            })
            .expect("the call")
    };
    assert_eq!((call("cross").state(), call("cross").output.as_str()), (v1::ToolCallState::Done, "done cross"));
    assert_eq!(call("lost").state(), v1::ToolCallState::Failed);
    let ask = entries.iter().find(|entry| matches!(entry.kind, Some(Kind::AskPerson(_)))).expect("the card").seq;
    assert_eq!(whole.waiting_seqs(), [ask]);

    // Read again, from the index.
    let mut read = Transcript::load(dir.clone(), "builder".into()).await.expect("load");
    same(&read, &whole);
    // The card is held, and changes on disk too.
    let Some(Kind::AskPerson(card)) = read.entry(ask).and_then(|entry| entry.kind.clone()) else { panic!("the card is held") };
    let answered = v1::AskPerson { state: v1::AskPersonState::Answered.into(), answer: "B".into(), ..card };
    assert!(read.set_ask(answered.clone()).is_some());
    assert!(read.waiting_seqs().is_empty());
    assert_eq!(all(&read)[ask as usize].kind, Some(Kind::AskPerson(answered)));

    // A compaction since: the newest segment is now an older one.
    let mut seven = vec![prompt("last6", 6_900_000)];
    seven.extend(exchanges(7, 150, "anthropic/claude-sonnet-4-6"));
    write(&dir, 7, &segment(true, 7, seven));
    let grown = Transcript::load(dir.clone(), "builder".into()).await.expect("load");
    let again = Transcript::load(dir.clone(), "builder".into()).await.expect("load");
    // Without what the index knew of the newest segment, the segment before
    // it is read for the prompt it carried over.
    let path = dir.join("transcript-index.json");
    let mut index: Value = serde_json::from_slice(&std::fs::read(&path).expect("index")).expect("json");
    assert!(index["next"].is_object(), "{}", index["next"]);
    index["next"] = Value::Null;
    std::fs::write(&path, index.to_string()).expect("write");
    let unknown = Transcript::load(dir.clone(), "builder".into()).await.expect("load");
    forget_index(&dir);
    let whole = Transcript::load(dir.clone(), "builder".into()).await.expect("load");
    assert_eq!(texts(&all(&whole)).iter().filter(|text| *text == "last6").count(), 1, "a carried prompt shows once");
    same(&grown, &whole);
    same(&again, &whole);
    same(&unknown, &whole);

    // An older segment taken back: it and the ones after it are read again.
    let mut two = exchanges(2, 120, "anthropic/claude-sonnet-4-6");
    two.push(calls("cross", "bash", 2_900_000));
    write(&dir, 2, &segment(true, 2, two));
    let changed = Transcript::load(dir.clone(), "builder".into()).await.expect("load");
    forget_index(&dir);
    let whole = Transcript::load(dir.clone(), "builder".into()).await.expect("load");
    assert_eq!(whole.len(), grown.len() - 60);
    same(&changed, &whole);
}

/// Through the app's requests, an agent's older entries on disk are where
/// they were: scrolling back reaches the first, a search finds what only an
/// older segment has, and the conversation goes back to a message there.
#[tokio::test]
async fn scrolling_back_searching_and_going_back_reach_entries_on_disk() {
    let dir = temp_dir();
    {
        let (addr, _factory) = start(&dir).await;
        let (mut client, workspace, _) = Client::connect(addr).await;
        let mut view = View { workspace, ..View::default() };
        apply_request(&mut client, &mut view, create_agent("", "Builder", "")).await;
    }
    let folder = dir.join("home/agents/builder");
    std::fs::create_dir_all(&folder).expect("folder");
    conversation(&folder);
    let whole = all(&Transcript::load(folder.clone(), "builder".into()).await.expect("load"));
    forget_index(&folder);

    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    assert!(view.has_earlier && view.entries.len() <= 200, "{}", view.entries.len());
    assert!(folder.join("transcript-index.json").is_file(), "read once, the older segments are on disk");

    // Scrolling back, a page at a time, to the first entry.
    let mut held = view.entries.clone();
    loop {
        let before = held[0].seq;
        let load = Request::LoadEntries(v1::LoadEntries { agent_id: "builder".into(), before_seq: before, limit: 0, from_seq: None });
        let Body::EntryPage(page) = client.request(load).await.body else { panic!("a page") };
        assert_eq!(page.entries.last().map(|entry| entry.seq), Some(before - 1));
        let more = page.has_earlier;
        held.splice(0..0, page.entries);
        if !more {
            break;
        }
    }
    assert_eq!(held, whole);

    // Found where only the first segment has it, newest first, all counted.
    let search = Request::SearchConversation(v1::SearchConversation {
        agent_id: "builder".into(),
        query: "m1-3".into(),
        limit: 50,
        ..Default::default()
    });
    let Body::ConversationMatches(found) = client.request(search).await.body else { panic!("matches") };
    let expected: Vec<u64> = whole
        .iter()
        .rev()
        .filter(|entry| matches!(&entry.kind, Some(Kind::User(user)) if user.text.contains("m1-3")))
        .map(|entry| entry.seq)
        .collect();
    assert_eq!(expected.len(), 11);
    assert_eq!(found.matches.iter().map(|hit| hit.seq).collect::<Vec<_>>(), expected);
    assert_eq!((found.total, found.more), (11, false));

    // Back to right before a message of the second segment.
    let back = whole.iter().find(|entry| matches!(&entry.kind, Some(Kind::User(user)) if user.text == "m2-10")).expect("m2-10");
    let fork = v1::ForkConversation { agent_id: "builder".into(), seq: back.seq, timestamp_ms: back.timestamp_ms };
    apply_request(&mut client, &mut view, Request::ForkConversation(fork)).await;
    view.follow(&mut client, |view| {
        let users = texts(&view.entries);
        users.last().is_some_and(|last| last == "m2-9") && !users.iter().any(|text| text == "m2-10")
    })
    .await;
}
