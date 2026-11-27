//! A segment's file (#383). A segment Claktory writes now is JSON Lines,
//! `segment-NNNN.jsonl`: a line per message, appended as the message ends,
//! a line when another part of the segment changes (its frame, its pinned
//! prompt sections), and lines for how it started (a compaction's summary,
//! a /clear). Each save writes only what's new, serialized outside the
//! state lock, instead of the whole segment. Segments written before are
//! `segment-NNNN.json`, one JSON object, and are still read; the current
//! one is written again whole, as JSON Lines, at its next save.
//!
//! A save cut off by a crash leaves at most its last line half written:
//! reading skips it, and the next save writes the segment whole again. So
//! does the next save after one that failed or was dropped, and one to a
//! file another name links to (a version `fork` set aside): it's replaced,
//! never written into.
//!
//! While an update is on trial (`crate::update`), a new segment, or one
//! still JSON, is saved as JSON: the version it rolls back to may not read
//! JSON Lines. Moves and imports already refuse an older Claktory.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::Write as _;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::{CompactionInfo, Frame};

/// A line of a JSON Lines segment.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Line<M> {
    Compaction(CompactionInfo),
    Cleared(i64),
    Message(M),
    Pinned(BTreeMap<String, String>),
    Frame(Frame),
}

/// A segment, its messages as `M`: `factory_agent::Message`, or JSON values
/// to keep them byte for byte as written.
pub(crate) struct SegmentFile<M> {
    pub compaction: Option<CompactionInfo>,
    pub cleared: Option<i64>,
    pub messages: Vec<M>,
    pub pinned: BTreeMap<String, String>,
    pub frame: Option<Frame>,
    /// Its last line was cut off (a crash mid-save): the next save writes
    /// it whole.
    pub torn: bool,
}

impl<M> Default for SegmentFile<M> {
    fn default() -> Self {
        SegmentFile { compaction: None, cleared: None, messages: Vec::new(), pinned: BTreeMap::new(), frame: None, torn: false }
    }
}

#[derive(Deserialize)]
struct Whole<M> {
    #[serde(default)]
    compaction: Option<CompactionInfo>,
    messages: Vec<M>,
    #[serde(default)]
    pinned: BTreeMap<String, String>,
    #[serde(default)]
    cleared: Option<i64>,
    #[serde(default)]
    frame: Option<Frame>,
}

/// Whether `path` names a JSON Lines segment.
pub(crate) fn is_lines(path: &Path) -> bool {
    path.extension().and_then(OsStr::to_str) == Some("jsonl")
}

/// Reads a segment from `bytes`, the contents of the file at `path`.
pub(crate) fn parse<M: DeserializeOwned>(path: &Path, bytes: &[u8]) -> Result<SegmentFile<M>, serde_json::Error> {
    if !is_lines(path) {
        let whole: Whole<M> = serde_json::from_slice(bytes)?;
        return Ok(SegmentFile {
            compaction: whole.compaction,
            cleared: whole.cleared,
            messages: whole.messages,
            pinned: whole.pinned,
            frame: whole.frame,
            torn: false,
        });
    }
    let mut segment = SegmentFile::default();
    // Every line a save wrote ends with its newline; what follows the last
    // one is a save a crash cut off.
    let (whole, cut) = match bytes.iter().rposition(|byte| *byte == b'\n') {
        Some(end) => (&bytes[..=end], &bytes[end + 1..]),
        None => (&bytes[..0], bytes),
    };
    segment.torn = !cut.iter().all(u8::is_ascii_whitespace);
    for line in whole.split(|byte| *byte == b'\n').filter(|line| !line.iter().all(u8::is_ascii_whitespace)) {
        match serde_json::from_slice::<Line<M>>(line)? {
            Line::Compaction(info) => segment.compaction = Some(info),
            Line::Cleared(at) => segment.cleared = Some(at),
            Line::Message(message) => segment.messages.push(message),
            Line::Pinned(pinned) => segment.pinned = pinned,
            Line::Frame(frame) => segment.frame = Some(frame),
        }
    }
    Ok(segment)
}

/// Reads the segment file at `path`.
pub(crate) fn read<M: DeserializeOwned>(path: &Path) -> std::io::Result<SegmentFile<M>> {
    parse(path, &std::fs::read(path)?).map_err(std::io::Error::other)
}

/// The number of the segment file named `name`, and whether it's JSON Lines.
pub(crate) fn number(name: &OsStr) -> Option<(u32, bool)> {
    let rest = name.to_str()?.strip_prefix("segment-")?;
    match rest.strip_suffix(".jsonl") {
        Some(number) => Some((number.parse().ok()?, true)),
        None => Some((rest.strip_suffix(".json")?.parse().ok()?, false)),
    }
}

/// The conversation's segment files in `dir`, by number. A segment saved
/// whole as JSON Lines just before a crash may still have its JSON beside
/// it, older: it's removed, so nothing takes it for the segment.
pub(crate) fn files(dir: &Path) -> std::io::Result<BTreeMap<u32, PathBuf>> {
    let mut found: BTreeMap<u32, PathBuf> = BTreeMap::new();
    let mut stale = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(found),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let Some((number, lines)) = number(&entry.file_name()) else { continue };
        match found.get(&number) {
            Some(had) if is_lines(had) => stale.push(entry.path()),
            Some(had) if lines => {
                stale.push(had.clone());
                found.insert(number, entry.path());
            }
            _ => {
                found.insert(number, entry.path());
            }
        }
    }
    for path in stale {
        if let Err(error) = std::fs::remove_file(&path) {
            tracing::warn!(path = %path.display(), %error, "could not remove a segment saved again as JSON Lines");
        }
    }
    Ok(found)
}

/// A segment's file path in `dir`, as JSON Lines or JSON.
pub(crate) fn path(dir: &Path, number: u32, lines: bool) -> PathBuf {
    dir.join(format!("segment-{number:04}.{}", if lines { "jsonl" } else { "json" }))
}

/// Whether a segment is saved by appending to its file: it's JSON Lines and
/// no other name links to it.
pub(crate) fn appendable(path: &Path) -> bool {
    is_lines(path) && links(path) == Some(1)
}

#[cfg(unix)]
fn links(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|meta| meta.nlink())
}

#[cfg(windows)]
fn links(path: &Path) -> Option<u64> {
    crate::windows::fs::identity(path).ok().map(|identity| identity.links)
}

/// A segment to write whole.
pub(crate) struct WholeSegment<'a, M> {
    pub compaction: Option<&'a CompactionInfo>,
    pub cleared: Option<i64>,
    pub messages: &'a [M],
    pub pinned: &'a BTreeMap<String, String>,
    pub frame: Option<&'a Frame>,
}

#[derive(Serialize)]
struct WholeRef<'a, M> {
    #[serde(skip_serializing_if = "Option::is_none")]
    compaction: Option<&'a CompactionInfo>,
    messages: &'a [M],
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pinned: &'a BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cleared: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame: Option<&'a Frame>,
}

/// Appends `line` and its newline to `bytes`.
pub(crate) fn push_line<T: Serialize>(line: &T, bytes: &mut Vec<u8>) -> Result<(), serde_json::Error> {
    serde_json::to_writer(&mut *bytes, line)?;
    bytes.push(b'\n');
    Ok(())
}

impl<M: Serialize> WholeSegment<'_, M> {
    /// The segment as JSON Lines, or as one JSON object.
    pub(crate) fn bytes(&self, lines: bool) -> Result<Vec<u8>, serde_json::Error> {
        if !lines {
            let whole = WholeRef {
                compaction: self.compaction,
                messages: self.messages,
                pinned: self.pinned,
                cleared: self.cleared,
                frame: self.frame,
            };
            return serde_json::to_vec(&whole);
        }
        let mut bytes = Vec::new();
        if let Some(compaction) = self.compaction {
            push_line(&Line::<&M>::Compaction(compaction.clone()), &mut bytes)?;
        }
        if let Some(at) = self.cleared {
            push_line(&Line::<&M>::Cleared(at), &mut bytes)?;
        }
        for message in self.messages {
            push_line(&Line::Message(message), &mut bytes)?;
        }
        if !self.pinned.is_empty() {
            push_line(&Line::<&M>::Pinned(self.pinned.clone()), &mut bytes)?;
        }
        if let Some(frame) = self.frame {
            push_line(&Line::<&M>::Frame(frame.clone()), &mut bytes)?;
        }
        Ok(bytes)
    }
}

/// Writes a segment whole to `path`, through a file beside it renamed over
/// it (so a name linked to the old file keeps the old), then removes the
/// segment's file of the other form, if there's one.
pub(crate) fn replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut partial = path.as_os_str().to_owned();
    partial.push(".tmp");
    std::fs::write(&partial, bytes)?;
    std::fs::rename(&partial, path)?;
    if let Some((number, lines)) = path.file_name().and_then(number)
        && let Some(dir) = path.parent()
    {
        match std::fs::remove_file(self::path(dir, number, !lines)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Appends `bytes`, whole lines, to the JSON Lines segment at `path`.
pub(crate) fn append(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new().append(true).open(path)?;
    file.write_all(bytes)
}
