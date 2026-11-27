//! A conversation's images on disk, each once, as a file.
//!
//! A segment used to carry every image the agent read or was given as
//! base64 inside its JSON: a third bigger than the image, and again for
//! each time the same image came back. A save now writes each image to
//! `segment-images/<sha256>.<ext>` in the agent's folder, once, and the
//! segment keeps only `"data": "claktory-image:<sha256>.<ext>"` in its
//! place.
//!
//! In memory, too, an image once saved is only where its file is: the save
//! turns every copy of it (the transcript's, the run's context, a request's)
//! into its file (`ImageData::store`), and reading a segment whole (the
//! model's context, a fork) finds each file without reading it. A request
//! reads the file when it sends the image, and holds nothing after (the
//! youtube agents' 200 MB of images took factoryd from 0.3 to 3.4 GB). Read
//! without its images' data (an older segment, only shown), nothing is
//! looked for.
//!
//! The name is the SHA-256 of the image's bytes, worked out here from
//! them, never taken from the message: an image from elsewhere (a pipe's)
//! can't take another's place. While an update is on trial, saves write
//! images inline, as the version it rolls back to reads them.
//!
//! A segment set aside by a fork (`branches/<name>/`) or a clear
//! (`archive/<ms>/`) reads its images from the agent's folder above it.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use factory_agent::types::{IMAGE_GONE, ImageData};
use factory_agent::{Message, ToolResultPart, UserContent, UserMessage, UserPart};
use sha2::Digest as _;

/// The folder of an agent's conversation images, beside its segments; named
/// so the agent's own files can't take it, and counted with conversations.
pub(crate) const DIR: &str = "segment-images";

/// How many folders up from a segment its agent's folder can be:
/// `archive/<ms>/branches/<name>/`.
const UP: usize = 5;

fn extension(mime_type: &str) -> &'static str {
    match mime_type.to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "bin",
    }
}

/// Whether `name` is a stored image's, as a segment gives it: 64 hex
/// digits, a dot and a short extension.
fn plain(name: &str) -> bool {
    let Some((digest, extension)) = name.split_once('.') else { return false };
    digest.len() == 64
        && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        && (1..=4).contains(&extension.len())
        && extension.bytes().all(|byte| byte.is_ascii_lowercase())
}

/// Each image's data in `message`, with its type, to `f`.
fn each_image(message: &mut Message, mut f: impl FnMut(&mut ImageData, &str)) {
    match message {
        Message::User(UserMessage { content: UserContent::Parts(parts), .. }) => {
            for part in parts {
                if let UserPart::Image { data, mime_type } = part {
                    f(data, mime_type);
                }
            }
        }
        Message::ToolResult(result) => {
            for part in &mut result.content {
                if let ToolResultPart::Image { data, mime_type, .. } = part {
                    f(data, mime_type);
                }
            }
        }
        _ => {}
    }
}

/// Writes each image in `message` to its file in `dir`, once, and from then
/// on every copy of it holds only where the file is: a segment then names
/// it (`factory_agent::types::naming_stored_images`), a request reads it.
/// An image whose data isn't base64 stays as it is.
pub(crate) fn store(message: &mut Message, dir: &Path) -> std::io::Result<()> {
    let mut result = Ok(());
    each_image(message, |data, mime_type| {
        if result.is_err() || data.is_empty() || data.named().is_some() {
            return;
        }
        // Already a file: this conversation's own, so its segment's name
        // reads back here (one shared from elsewhere is copied in).
        if let Some((path, name)) = data.file() {
            let here = dir.join(&name);
            if path != here && !here.exists() {
                result = std::fs::create_dir_all(dir).and_then(|()| copy_once(&path, &here));
            }
            return;
        }
        let Some(bytes) = data.bytes() else { return };
        let digest: String = sha2::Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect();
        let name = format!("{digest}.{}", extension(mime_type));
        match write_once(dir, &name, &bytes) {
            Ok(()) => data.store(dir.join(&name), &name),
            Err(error) => result = Err(error),
        }
    });
    result
}

/// A file beside `to`, renamed over it, so a crash never leaves half an
/// image under its name.
fn temporary(to: &Path) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| elapsed.subsec_nanos());
    let name = to.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    to.with_file_name(format!(".{name}.{}.{nanos}.tmp", std::process::id()))
}

/// Writes `bytes` to `dir/name` unless it's there.
fn write_once(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let path = dir.join(name);
    if path.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    let tmp = temporary(&path);
    let written = (|| {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, &path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// Copies the stored image at `from` to `to`.
fn copy_once(from: &Path, to: &Path) -> std::io::Result<()> {
    let tmp = temporary(to);
    let copied = std::fs::copy(from, &tmp).and_then(|_| std::fs::rename(&tmp, to));
    if copied.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    copied
}

/// Where the stored image `name` is for the segment at `segment`: in its
/// folder's `segment-images`, or a folder above (a fork's or a clear's).
fn find(segment: &Path, name: &str) -> Option<PathBuf> {
    segment.parent()?.ancestors().take(UP).map(|dir| dir.join(DIR).join(name)).find(|path| path.is_file())
}

/// Finds the file of each stored image in `messages`, of the segment at
/// `segment`: each then holds where it is, read when a request needs it.
/// One whose file is gone says so in its place.
pub(crate) fn load(messages: &mut [Message], segment: &Path) {
    for message in messages {
        let mut gone = false;
        each_image(message, |data, _| {
            let Some(name) = data.named() else { return };
            match Some(name.as_str()).filter(|name| plain(name)).and_then(|name| find(segment, name)) {
                Some(path) => *data = ImageData::stored(path, &name),
                None => {
                    tracing::warn!(segment = %segment.display(), image = %name, "a conversation's image file is missing");
                    data.clear();
                    gone = true;
                }
            }
        });
        if gone {
            missing(message);
        }
    }
}

/// Puts what was said for an image whose file is gone in its place.
fn missing(message: &mut Message) {
    match message {
        Message::User(UserMessage { content: UserContent::Parts(parts), .. }) => {
            for part in parts {
                if matches!(part, UserPart::Image { data, .. } if data.is_empty()) {
                    *part = UserPart::Text { text: IMAGE_GONE.to_owned() };
                }
            }
        }
        Message::ToolResult(result) => {
            for part in &mut result.content {
                if matches!(part, ToolResultPart::Image { data, .. } if data.is_empty()) {
                    *part = ToolResultPart::Text { text: IMAGE_GONE.to_owned() };
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use factory_agent::types::STORED_IMAGE as MARKER;

    /// A folder of its own for a test, removed after.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Scratch {
            static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("factoryd-images-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("dir");
            Scratch(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn png(seed: u8) -> String {
        base64::engine::general_purpose::STANDARD.encode([0x89, b'P', b'N', b'G', seed, seed, seed])
    }

    fn result(images: &[String]) -> Message {
        Message::ToolResult(factory_agent::types::ToolResultMessage {
            tool_call_id: "c".into(),
            tool_name: "read_image".into(),
            content: images
                .iter()
                .map(|data| ToolResultPart::Image { data: data.clone().into(), mime_type: "image/png".into(), digest: Some("0".repeat(64)) })
                .chain(std::iter::once(ToolResultPart::Text { text: "two".into() }))
                .collect(),
            details: serde_json::Value::Null,
            usage: None,
            is_error: false,
            timestamp: 1,
            started: None,
        })
    }

    fn user(data: &str) -> Message {
        Message::User(UserMessage {
            content: UserContent::Parts(vec![
                UserPart::Text { text: "look".into() },
                UserPart::Image { data: data.to_owned().into(), mime_type: "image/jpeg".into() },
            ]),
            timestamp: 1,
            event: None,
            sent_from: None,
            client_id: String::new(),
            ..serde_json::from_value(serde_json::json!({"content": "", "timestamp": 1})).expect("a user message")
        })
    }

    /// Each image's base64 in `messages`, in order: None for one that can't be read.
    fn images(messages: &mut [Message]) -> Vec<Option<String>> {
        let mut found = Vec::new();
        for message in messages {
            each_image(message, |data, _| found.push(data.base64().map(|data| data.to_string())));
        }
        found
    }

    /// Saved as a segment would be, and read back from one at `segment`.
    fn round_trip(saved: &[Message], segment: &Path) -> Vec<Message> {
        let json = factory_agent::types::naming_stored_images(|| serde_json::to_string(saved)).expect("json");
        let mut read: Vec<Message> = serde_json::from_str(&json).expect("parsed");
        load(&mut read, segment);
        read
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir.join(DIR))
            .map(|entries| entries.filter_map(Result::ok).map(|entry| entry.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn each_image_is_stored_once_by_its_bytes_and_read_back_the_same() {
        let dir = Scratch::new();
        let (a, b) = (png(1), png(2));
        let original = vec![result(&[a.clone(), b.clone(), a.clone()]), user(&a), result(std::slice::from_ref(&b))];
        let expected = [&a, &b, &a, &a, &b].map(|data| Some(data.to_string())).to_vec();
        let mut saved = original.clone();
        for message in &mut saved {
            store(message, &dir.path().join(DIR)).expect("stored");
        }
        // Every copy let go of its base64: it's the file now.
        let Message::ToolResult(first) = &original[0] else { panic!("a tool result") };
        let ToolResultPart::Image { data, .. } = &first.content[0] else { panic!("an image") };
        assert!(data.file().is_some(), "{data:?}");
        // Two files: the same bytes are one, whatever the message said its
        // digest was, and whatever the type (its extension) — a JPEG named
        // so is another file.
        assert_eq!(files(dir.path()).len(), 3, "{:?}", files(dir.path()));
        let json = factory_agent::types::naming_stored_images(|| serde_json::to_string(&saved)).expect("json");
        assert!(!json.contains(&a) && !json.contains(&b), "{json}");
        assert_eq!(json.matches(MARKER).count(), 5, "{json}");
        // Stored again: nothing new, nothing changed.
        let mut again = saved.clone();
        for message in &mut again {
            store(message, &dir.path().join(DIR)).expect("stored");
        }
        assert_eq!(again, saved);
        // Read back from a segment there, or one set aside below it.
        let mut read = round_trip(&saved, &dir.path().join("segment-0001.jsonl"));
        assert_eq!(read, original);
        assert_eq!(images(&mut read), expected);
        let mut aside = round_trip(&saved, &dir.path().join("archive/1795/branches/b1/segment-0002.jsonl"));
        assert_eq!(images(&mut aside), expected);
        // Anywhere but a segment, a stored image is its base64.
        let wire = serde_json::to_string(&saved).expect("json");
        assert!(wire.contains(&a) && !wire.contains(MARKER), "{wire}");
    }

    #[test]
    fn an_image_whose_file_is_gone_says_so_and_a_name_that_isnt_one_is_never_read() {
        let dir = Scratch::new();
        let mut kept = vec![result(&[png(3)])];
        store(&mut kept[0], &dir.path().join(DIR)).expect("stored");
        let segment = dir.path().join("segment-0001.jsonl");
        let json = factory_agent::types::naming_stored_images(|| serde_json::to_string(&kept)).expect("json");
        std::fs::remove_dir_all(dir.path().join(DIR)).expect("removed");
        let mut saved: Vec<Message> = serde_json::from_str(&json).expect("parsed");
        load(&mut saved, &segment);
        // A name reaching out of the folder.
        std::fs::write(dir.path().join("secret.png"), b"no").expect("written");
        let sneaky = serde_json::to_string(&vec![user(&format!("{MARKER}../secret.png"))]).expect("json");
        let mut sneaky: Vec<Message> = serde_json::from_str(&sneaky).expect("parsed");
        load(&mut sneaky, &segment);
        let Message::ToolResult(result) = &saved[0] else { panic!("a tool result") };
        assert_eq!(result.content[0], ToolResultPart::Text { text: IMAGE_GONE.to_owned() });
        let Message::User(UserMessage { content: UserContent::Parts(parts), .. }) = &sneaky[0] else { panic!("parts") };
        assert_eq!(parts[1], UserPart::Text { text: IMAGE_GONE.to_owned() });
    }

    #[test]
    fn data_that_isnt_base64_stays_inline() {
        let dir = Scratch::new();
        let mut message = user("not base64!");
        let before = message.clone();
        store(&mut message, &dir.path().join(DIR)).expect("stored");
        assert_eq!(message, before);
        assert!(files(dir.path()).is_empty());
    }
}
