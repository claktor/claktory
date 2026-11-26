//! What a conversation's images keep in memory (#454 Part 1 #9): once
//! saved, an image is only where its file is, in every copy, and a
//! conversation read back finds its images' files without reading them.
//! Measured with a counting allocator, so a file of its own (the allocator
//! is the process's); `--nocapture` prints the numbers. While an update is
//! on trial, images stay inline, which is the measure to compare with.

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use base64::Engine as _;
use factory_agent::{AgentEvent, Message, ToolResultPart};
use factoryd::transcript::Transcript;

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            LIVE.fetch_add(layout.size(), Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let moved = unsafe { System.realloc(pointer, layout, size) };
        if !moved.is_null() {
            if size > layout.size() {
                LIVE.fetch_add(size - layout.size(), Ordering::Relaxed);
            } else {
                LIVE.fetch_sub(layout.size() - size, Ordering::Relaxed);
            }
        }
        moved
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const MB: f64 = 1024.0 * 1024.0;
/// Images in the conversation, and each one's bytes: 24 MB, 32 MB as base64.
const IMAGES: usize = 24;
const IMAGE_BYTES: usize = 1 << 20;

fn live() -> usize {
    LIVE.load(Ordering::Relaxed)
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("factoryd-image-memory-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    dir
}

/// A screenshot the agent read: `n`'s own bytes, so each is its own file.
fn screenshot(n: usize) -> Message {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64 ^ n as u64;
    let bytes: Vec<u8> = (0..IMAGE_BYTES)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect();
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    serde_json::from_value(serde_json::json!({
        "role": "toolResult", "toolCallId": format!("c{n}"), "toolName": "read_image", "isError": false, "timestamp": n,
        "content": [{"type": "image", "data": data, "mimeType": "image/png"}, {"type": "text", "text": "a screenshot"}],
    }))
    .expect("a message")
}

/// Reads `IMAGES` screenshots into a conversation in `dir`, saving each as
/// Claktory does; what it holds after, beyond what it held before.
async fn read_screenshots(dir: &Path, trial: bool) -> (Transcript, usize) {
    let mut transcript = Transcript::load(dir.to_owned(), "builder".into()).await.expect("load");
    transcript.save_old_form(Arc::new(AtomicBool::new(trial)));
    let before = live();
    for n in 0..IMAGES {
        let applied = transcript.apply(AgentEvent::MessageEnd { message: screenshot(n) }).expect("applied");
        applied.persist.expect("a save").write_now().expect("saved");
    }
    let held = live().saturating_sub(before);
    (transcript, held)
}

/// Each image's base64, as a request reads it.
fn images(transcript: &Transcript) -> Vec<usize> {
    transcript
        .messages()
        .iter()
        .flat_map(|message| match message {
            Message::ToolResult(result) => result.content.clone(),
            _ => Vec::new(),
        })
        .filter_map(|part| match part {
            ToolResultPart::Image { data, .. } => data.base64().map(|data| data.len()),
            ToolResultPart::Text { .. } => None,
        })
        .collect()
}

#[tokio::test]
async fn a_conversations_images_live_in_their_files_not_in_memory() {
    let base64 = IMAGES * IMAGE_BYTES.div_ceil(3) * 4;

    // On trial, as before: every image's base64 in memory.
    let inline_dir = temp_dir("inline");
    let (inline, inline_held) = read_screenshots(&inline_dir, true).await;
    drop(inline);

    // Saved as files: each copy holds only where its file is.
    let dir = temp_dir("files");
    let (saved, saved_held) = read_screenshots(&dir, false).await;
    assert_eq!(images(&saved), vec![base64 / IMAGES; IMAGES], "a request still reads every image whole");
    drop(saved);

    // Read back: the files are found, not read.
    let before = live();
    let read = Transcript::load(dir.clone(), "builder".into()).await.expect("load");
    let read_held = live().saturating_sub(before);
    assert_eq!(images(&read), vec![base64 / IMAGES; IMAGES]);
    drop(read);

    // The same conversation read back with its images inline, as before.
    let before = live();
    let inline_read = Transcript::load(inline_dir.clone(), "builder".into()).await.expect("load");
    let inline_read_held = live().saturating_sub(before);
    drop(inline_read);

    println!(
        "{IMAGES} images, {:.1} MB as base64: held after reading them {:.1} MB inline, {:.1} MB as files; after loading {:.1} MB inline, {:.1} MB as files",
        base64 as f64 / MB,
        inline_held as f64 / MB,
        saved_held as f64 / MB,
        inline_read_held as f64 / MB,
        read_held as f64 / MB
    );
    assert!(inline_held >= base64, "the measure to compare with holds every image: {inline_held}");
    assert!(saved_held < base64 / 10, "saved as files, a tenth at most of their base64: {saved_held}");
    assert!(read_held < base64 / 10, "read back, a tenth at most of their base64: {read_held}");
    let _ = std::fs::remove_dir_all(&inline_dir);
    let _ = std::fs::remove_dir_all(&dir);
}
