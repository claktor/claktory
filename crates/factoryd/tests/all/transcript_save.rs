//! Saving a conversation (#383): its current segment is JSON Lines, a
//! message at a time, appended; a segment from before is read as it is and
//! written whole as JSON Lines at its first save. Whatever happens, it
//! reads back as it was.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use factory_agent::{AgentEvent, AssistantMessage, AssistantPart, Message, StopReason, Usage};
use factoryd::transcript::{Frame, Persist, Transcript};
use serde_json::{Value, json};

use crate::common::temp_dir;

fn message(value: Value) -> Message {
    serde_json::from_value(value).expect("a message")
}

fn user(text: &str, at: u64) -> Message {
    message(
        json!({"role": "user", "content": [{"type": "text", "text": text}, {"type": "image", "data": "SU1BR0U=", "mimeType": "image/png"}], "timestamp": at}),
    )
}

fn said(text: &str, at: u64) -> Message {
    Message::Assistant(AssistantMessage {
        content: vec![AssistantPart::Text { text: text.into(), text_signature: None }],
        api: "anthropic-messages".into(),
        provider: "anthropic".into(),
        model: "claude-sonnet-4-6".into(),
        usage: Usage::empty(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: at,
        extra: Default::default(),
    })
}

fn frame(prompt: &str) -> Frame {
    serde_json::from_value(json!({"settings": "s", "system_prompt": prompt, "tools": []})).expect("a frame")
}

/// Shows `message` in `transcript` and saves it as Claktory does.
fn end(transcript: &mut Transcript, message: Message) {
    let applied = transcript.apply(AgentEvent::MessageEnd { message }).expect("applied");
    save(applied.persist.expect("a save"));
}

fn save(mut persist: Persist) {
    persist.write_now().expect("saved");
}

async fn load(dir: &Path) -> Transcript {
    Transcript::load(dir.to_owned(), "builder".into()).await.expect("load")
}

fn file(dir: &Path, number: u32) -> PathBuf {
    factoryd::transcript::segment_file_for_test(dir, number).expect("the segment's file")
}

/// What two loads of a conversation hold alike: every entry shown, the
/// model's context, its frame and pinned sections.
async fn same(dir: &Path, before: &mut Transcript) {
    let read = load(dir).await;
    assert_eq!(read.window().1, before.window().1);
    assert_eq!(read.messages(), before.messages());
    assert_eq!(read.frame(), before.frame());
    let mut read = read;
    assert_eq!(read.pinned_mut(), before.pinned_mut());
}

/// A conversation from before JSON Lines carries on: its segment is
/// written again whole as JSON Lines at its first save, and from then on
/// each save appends only what's new, frame and pinned sections included.
/// A compaction starts a new file. It reads back the same after each.
#[tokio::test]
async fn a_conversation_is_saved_a_message_at_a_time_and_reads_back_the_same() {
    let dir = temp_dir();
    let old = json!({"messages": [user("one", 1), said("echo: one", 2)], "frame": frame("S1"), "pinned": {"memory": "M1"}});
    std::fs::write(dir.join("segment-0001.json"), old.to_string()).expect("write");

    let mut transcript = load(&dir).await;
    end(&mut transcript, user("two", 3));
    assert_eq!(file(&dir, 1), dir.join("segment-0001.jsonl"));
    assert!(!dir.join("segment-0001.json").exists(), "the JSON is gone once saved again");
    same(&dir, &mut transcript).await;

    // Appended: what was there stays byte for byte; one line more.
    let before = std::fs::read(file(&dir, 1)).expect("read");
    end(&mut transcript, said("echo: two", 4));
    let after = std::fs::read(file(&dir, 1)).expect("read");
    assert!(after.starts_with(&before));
    let added = &after[before.len()..];
    assert_eq!(added.iter().filter(|byte| **byte == b'\n').count(), 1);
    assert!(added.starts_with(br#"{"message":{"role":"assistant""#), "{}", String::from_utf8_lossy(added));
    same(&dir, &mut transcript).await;

    // A new frame and pinned section are lines of their own.
    transcript.set_frame(frame("S2"));
    transcript.pinned_mut().insert("memory".into(), "M2".into());
    end(&mut transcript, user("three", 5));
    let text = std::fs::read_to_string(file(&dir, 1)).expect("read");
    assert_eq!(text.lines().filter(|line| line.starts_with(r#"{"frame":"#)).count(), 2, "{text}");
    same(&dir, &mut transcript).await;
    assert_eq!(load(&dir).await.frame().map(|frame| frame.system_prompt.as_str()), Some("S2"));

    // A compaction: a new segment, its own file.
    let summary = message(json!({"role": "user", "content": "What happened", "timestamp": 6}));
    let compacted = AgentEvent::Compacted {
        summary: "What happened".into(),
        tokens_before: 10,
        messages: vec![summary, user("three", 5)],
        usage: Usage::empty(),
        reason: Default::default(),
    };
    save(transcript.apply(compacted).expect("applied").persist.expect("a save"));
    assert_eq!(file(&dir, 2), dir.join("segment-0002.jsonl"));
    end(&mut transcript, said("echo: three", 7));
    same(&dir, &mut transcript).await;
    let lines = std::fs::read_to_string(file(&dir, 2)).expect("read");
    assert!(lines.starts_with(r#"{"compaction":"#), "{lines}");
    // The model's context still has its images.
    assert!(lines.contains("SU1BR0U="));
}

/// A save a crash cut off: everything before it reads as it was, and the
/// next save writes the segment whole, with nothing of the cut line.
#[tokio::test]
async fn a_save_cut_off_by_a_crash_loses_only_itself() {
    let dir = temp_dir();
    save(Transcript::cleared(dir.clone(), "builder".into(), Arc::new(AtomicBool::new(false))).1);
    let mut transcript = load(&dir).await;
    end(&mut transcript, user("one", 1));
    end(&mut transcript, said("echo: one", 2));
    let path = file(&dir, 1);
    let whole = std::fs::read(&path).expect("read");
    let mut cut = whole.clone();
    cut.extend_from_slice(br#"{"message":{"role":"user","content":"half a me"#);
    std::fs::write(&path, &cut).expect("write");

    let mut read = load(&dir).await;
    assert_eq!(read.messages(), transcript.messages());
    assert_eq!(read.window().1, transcript.window().1);
    end(&mut read, user("two", 3));
    let saved = std::fs::read_to_string(&path).expect("read");
    assert!(!saved.contains("half a me") && saved.ends_with('\n'), "{saved}");
    same(&dir, &mut read).await;
    assert_eq!(read.messages().len(), 3);
}

/// A save that's never made (it failed, or was dropped) leaves nothing out:
/// the next one writes the segment whole. And a file another name links to
/// (a version set aside) is replaced, never written into.
#[tokio::test]
async fn a_save_not_made_or_a_linked_file_has_the_next_save_write_it_whole() {
    let dir = temp_dir();
    save(Transcript::cleared(dir.clone(), "builder".into(), Arc::new(AtomicBool::new(false))).1);
    let mut transcript = load(&dir).await;
    let applied = transcript.apply(AgentEvent::MessageEnd { message: user("lost", 1) }).expect("applied");
    drop(applied.persist);
    end(&mut transcript, said("kept", 2));
    same(&dir, &mut transcript).await;
    assert!(std::fs::read_to_string(file(&dir, 1)).expect("read").contains("lost"));

    let linked = dir.join("version.jsonl");
    std::fs::hard_link(file(&dir, 1), &linked).expect("link");
    let version = std::fs::read(&linked).expect("read");
    let mut transcript = load(&dir).await;
    end(&mut transcript, user("after", 3));
    assert_eq!(std::fs::read(&linked).expect("read"), version, "the version keeps what it had");
    std::fs::remove_file(&linked).expect("remove");
    same(&dir, &mut transcript).await;
}

/// While an update is on trial, a segment still JSON, or a new one, is saved
/// as JSON, which the version it would roll back to reads; once it passes,
/// the next save writes JSON Lines.
#[tokio::test]
async fn while_an_update_is_on_trial_segments_stay_json() {
    let dir = temp_dir();
    let old = json!({"messages": [user("one", 1)]});
    std::fs::write(dir.join("segment-0001.json"), old.to_string()).expect("write");
    let trial = Arc::new(AtomicBool::new(true));
    let mut transcript = load(&dir).await;
    transcript.save_old_form(trial.clone());
    end(&mut transcript, said("echo: one", 2));
    assert_eq!(file(&dir, 1), dir.join("segment-0001.json"));
    let segment: Value = serde_json::from_slice(&std::fs::read(file(&dir, 1)).expect("read")).expect("one JSON object");
    assert_eq!(segment["messages"].as_array().map(Vec::len), Some(2));

    let compacted = AgentEvent::Compacted {
        summary: "S".into(),
        tokens_before: 1,
        messages: vec![message(json!({"role": "user", "content": "S", "timestamp": 3}))],
        usage: Usage::empty(),
        reason: Default::default(),
    };
    save(transcript.apply(compacted).expect("applied").persist.expect("a save"));
    assert_eq!(file(&dir, 2), dir.join("segment-0002.json"));

    trial.store(false, std::sync::atomic::Ordering::Relaxed);
    end(&mut transcript, user("two", 4));
    assert_eq!(file(&dir, 2), dir.join("segment-0002.jsonl"));
    same(&dir, &mut transcript).await;
}
