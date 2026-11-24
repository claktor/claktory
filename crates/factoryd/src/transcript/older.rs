//! A conversation's older segments, kept on disk (#377). Every segment but
//! the newest is only shown, so once read its entries are written to
//! `transcript-entries.bin` (each section's, one after the other) with
//! what restoring the next segment needs in `transcript-index.json`. A
//! restart then reads only the newest segment, and an older section's
//! entries are read back when they're asked for (scrolling back, a search,
//! going back, a report). Entries that can still change, or are looked up
//! by what they show (cards, calls still waiting for a result), stay in
//! memory as well (`Transcript::kept`), and win over what's on disk.
//!
//! Both files are a cache of the segments: each section names its segment
//! file as it was read (inode, size, time), so a segment that changed (a
//! conversation taken back or switched) or a different file in its place
//! has its section and the ones after it read again. Older Claktories
//! ignore them.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use factory_protocol::v1::Entry;
use prost::Message as _;
use serde::{Deserialize, Serialize};

use super::Origin;

pub(super) const INDEX: &str = "transcript-index.json";
pub(super) const ENTRIES: &str = "transcript-entries.bin";

/// What the files' contents follow. Bump it when restoring a segment shows
/// something else (`Transcript::show`, `Transcript::restore`) or keeps
/// other entries in memory (`keeps`): every conversation's older segments
/// are then read again once, instead of showing what an earlier version
/// made of them.
const VERSION: u32 = 1;

/// Which file a section was read from: a segment rewritten, or another
/// file put in its place, no longer matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Stamp {
    inode: u64,
    len: u64,
    modified_ns: u64,
}

impl Stamp {
    pub(super) fn of(path: &Path) -> std::io::Result<Stamp> {
        let meta = std::fs::metadata(path)?;
        let modified = meta.modified()?.duration_since(UNIX_EPOCH).map_or(0, |since| since.as_nanos() as u64);
        Ok(Stamp { inode: meta.ino(), len: meta.len(), modified_ns: modified })
    }
}

/// An older segment's entries in the entries file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Section {
    pub(super) number: u32,
    pub(super) file: Stamp,
    /// Its entries' seqs: `first..first + count`.
    pub(super) first: u64,
    pub(super) count: u64,
    /// Where they are in the entries file, length-delimited one after the
    /// other.
    pub(super) offset: u64,
    pub(super) bytes: u64,
}

impl Section {
    fn end(&self) -> u64 {
        self.first + self.count
    }
}

/// Where the newest segment's shown messages start (`Transcript::restore`):
/// after its summary and the messages compaction carried over from the
/// segment before, which were shown there. Known from that segment's
/// messages, which a restart no longer reads, so it's kept for the segment
/// whose summary was written at `summary_ms`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Next {
    pub(super) number: u32,
    pub(super) summary_ms: u64,
    pub(super) shown_from: u32,
}

/// `transcript-index.json`: the sections, and the conversation's state
/// after the last of them, from which the next segment is restored.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct Index {
    pub(super) version: u32,
    pub(super) sections: Vec<Section>,
    #[serde(default)]
    pub(super) awaiting_reply: Option<u64>,
    /// Calls still without a result (call id, seq): one may come in a later
    /// segment.
    #[serde(default)]
    pub(super) tools: Vec<(String, u64)>,
    #[serde(default)]
    pub(super) models: Vec<(u64, String)>,
    #[serde(default)]
    pub(super) origins: Vec<(u64, Origin)>,
    /// The entries kept in memory, encoded (base64 of the protocol's bytes).
    #[serde(default)]
    pub(super) kept: Vec<String>,
    #[serde(default)]
    pub(super) next: Option<Next>,
}

impl Index {
    /// The folder's index, if it's this version's and its sections are
    /// the first of `older` (number, file) and in its entries file.
    pub(super) fn read(dir: &Path, older: &[(u32, Stamp)]) -> Option<Index> {
        let bytes = std::fs::read(dir.join(INDEX)).ok()?;
        let index: Index = serde_json::from_slice(&bytes).ok()?;
        if index.version != VERSION || index.sections.is_empty() || index.sections.len() > older.len() {
            return None;
        }
        let same = index.sections.iter().zip(older).all(|(section, (number, file))| section.number == *number && section.file == *file);
        let written = std::fs::metadata(dir.join(ENTRIES)).ok()?.len();
        let last = index.sections.last()?;
        (same && last.offset + last.bytes <= written).then_some(index)
    }

    pub(super) fn kept(&self) -> Vec<Entry> {
        self.kept.iter().filter_map(|text| STANDARD.decode(text).ok()).filter_map(|bytes| Entry::decode(&bytes[..]).ok()).collect()
    }

    pub(super) fn encode_kept<'a>(kept: impl Iterator<Item = &'a Entry>) -> Vec<String> {
        kept.map(|entry| STANDARD.encode(entry.encode_to_vec())).collect()
    }

    /// Where the entries file's written part ends.
    pub(super) fn end(&self) -> u64 {
        self.sections.last().map_or(0, |last| last.offset + last.bytes)
    }

    /// Writes the index, after `added` (the encoded entries of its sections
    /// from `from` on) at the end of the entries file's part it keeps. The
    /// entries reach the disk before the index that points at them.
    pub(super) fn write(&self, dir: &Path, from: u64, added: &[u8]) -> std::io::Result<()> {
        let mut index = self.clone();
        index.version = VERSION;
        if !added.is_empty() {
            let mut file = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(dir.join(ENTRIES))?;
            file.set_len(from)?;
            file.seek(SeekFrom::Start(from))?;
            file.write_all(added)?;
            file.sync_data()?;
        }
        let path = dir.join(INDEX);
        let partial = dir.join(format!("{INDEX}.partial"));
        std::fs::write(&partial, serde_json::to_vec(&index).map_err(std::io::Error::other)?)?;
        std::fs::rename(&partial, &path)
    }
}

/// Removes the folder's index, so its entries file is read again from the
/// segments; the entries file too when nothing needs it.
pub(super) fn forget(dir: &Path, entries_too: bool) {
    let mut names = vec![INDEX];
    if entries_too {
        names.push(ENTRIES);
    }
    for name in names {
        match std::fs::remove_file(dir.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(dir = %dir.display(), %error, "could not remove a conversation's index"),
        }
    }
}

/// Appends `entries`, length-delimited, to `bytes`.
pub(super) fn encode(entries: &[Entry], bytes: &mut Vec<u8>) {
    for entry in entries {
        // Writing to a Vec can't fail.
        let _ = entry.encode_length_delimited(bytes);
    }
}

/// The older sections no longer held in memory, to read entries back from
/// off the async threads: a snapshot, so it's read without the lock.
#[derive(Clone)]
pub struct Older {
    dir: PathBuf,
    sections: Arc<[Section]>,
    /// What's kept in memory of them, as it is now.
    kept: BTreeMap<u64, Entry>,
}

impl Older {
    pub(super) fn new(dir: PathBuf, sections: Arc<[Section]>, kept: BTreeMap<u64, Entry>) -> Older {
        Older { dir, sections, kept }
    }

    /// Where the entries on disk end: the first seq held in memory.
    pub(super) fn end(&self) -> u64 {
        self.sections.last().map_or(0, Section::end)
    }

    /// How many sections there are.
    pub fn sections(&self) -> usize {
        self.sections.len()
    }

    /// The section holding `seq`.
    fn holding(&self, seq: u64) -> Option<usize> {
        let at = self.sections.partition_point(|section| section.first <= seq).checked_sub(1)?;
        (seq < self.sections[at].end()).then_some(at)
    }

    /// The entries of section `at`, oldest first, as they show now.
    pub fn section(&self, at: usize) -> std::io::Result<Vec<Entry>> {
        let section = &self.sections[at];
        let mut file = std::fs::File::open(self.dir.join(ENTRIES))?;
        file.seek(SeekFrom::Start(section.offset))?;
        let mut bytes = vec![0; usize::try_from(section.bytes).map_err(std::io::Error::other)?];
        file.read_exact(&mut bytes)?;
        let mut read = &bytes[..];
        let mut entries = Vec::with_capacity(usize::try_from(section.count).unwrap_or(0));
        while !read.is_empty() {
            let mut entry = Entry::decode_length_delimited(&mut read).map_err(std::io::Error::other)?;
            if entry.seq != section.first + entries.len() as u64 {
                return Err(std::io::Error::other(format!(
                    "entry {} where {} was expected",
                    entry.seq,
                    section.first + entries.len() as u64
                )));
            }
            if let Some(kept) = self.kept.get(&entry.seq) {
                entry.clone_from(kept);
            }
            entries.push(entry);
        }
        if entries.len() as u64 != section.count {
            return Err(std::io::Error::other(format!("{} entries where {} were expected", entries.len(), section.count)));
        }
        Ok(entries)
    }

    /// The entries of `seqs` there are.
    pub fn entries(&self, seqs: &[u64]) -> std::io::Result<HashMap<u64, Entry>> {
        let mut found = HashMap::new();
        let mut sections: Vec<usize> = seqs.iter().filter_map(|seq| self.holding(*seq)).collect();
        sections.sort_unstable();
        sections.dedup();
        for at in sections {
            for entry in self.section(at)? {
                if seqs.contains(&entry.seq) {
                    found.insert(entry.seq, entry);
                }
            }
        }
        Ok(found)
    }

    /// The sections holding entries before `end`, newest first.
    pub fn before(&self, end: u64) -> impl Iterator<Item = usize> + '_ {
        (0..self.sections.len()).rev().filter(move |at| self.sections[*at].first < end)
    }

    /// A page of entries (`Transcript::page`) that starts on disk: `held`
    /// are the entries from `end()` to `before` held in memory.
    pub fn page(&self, held: Vec<Entry>, before: u64, limit: u32, from: Option<u64>) -> std::io::Result<(Vec<Entry>, bool)> {
        let mut entries = held;
        let mut offset = self.end().min(before);
        for at in self.before(offset) {
            let mut section = self.section(at)?;
            section.retain(|entry| entry.seq < offset);
            offset = self.sections[at].first;
            section.append(&mut entries);
            entries = section;
            if let Some(start) = super::page_start(&entries, offset, before, limit, from) {
                let start = usize::try_from(start - offset).unwrap_or(usize::MAX).min(entries.len());
                let more = offset + start as u64 > 0;
                entries.drain(..start);
                return Ok((entries, more));
            }
        }
        Ok((entries, offset > 0))
    }
}
