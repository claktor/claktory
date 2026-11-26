//! Review cards follow their file (Bruno 46189): when the file a waiting
//! review card shows changes, Claktory marks the card updated by itself, as
//! an ask_update would (`crate::extensions::ask::file_changed`), so the
//! person sees that it changed and Approve means the file as it reads now.
//! An agent that changes the file needn't remember to change the card.
//!
//! It looks every few seconds at the files of the review cards waiting,
//! only their size and modification time; once a file has stopped changing
//! for `QUIET` (an agent saving ten times makes one update), it reads it
//! and compares what it holds with what the card was asked with. The same
//! content saved again marks nothing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime};

use sha2::{Digest, Sha256};

use crate::factory::Factory;

/// How often it looks at the files.
const POLL_EVERY: Duration = Duration::from_secs(5);
/// How long a file must stay unchanged before its card is marked.
const QUIET: Duration = Duration::from_secs(30);

/// Tests' shorter times, by Claktory (its home).
static TIMES_FOR_TEST: Mutex<Option<HashMap<PathBuf, (Duration, Duration)>>> = Mutex::new(None);

fn times(factory: &Factory) -> (Duration, Duration) {
    crate::locking::lock(&TIMES_FOR_TEST)
        .as_ref()
        .and_then(|all| all.get(factory.home.root()).copied())
        .unwrap_or((POLL_EVERY, QUIET))
}

/// Tests: `factory` looks at review files every `poll` and marks a card
/// once its file has been quiet for `quiet`.
#[doc(hidden)]
pub fn set_times_for_test(factory: &Factory, poll: Duration, quiet: Duration) {
    crate::locking::lock(&TIMES_FOR_TEST).get_or_insert_with(HashMap::new).insert(factory.home.root().to_path_buf(), (poll, quiet));
}

/// What a file holds, to tell whether it changed: its size and SHA-256,
/// "<size>:<hex>"; '' for no file (or none that reads).
pub(crate) fn fingerprint(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let Ok(mut file) = std::fs::File::open(path) else { return String::new() };
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut size = 0_u64;
    loop {
        match std::io::Read::read(&mut file, &mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                hasher.update(&buffer[..read]);
                size += read as u64;
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return String::new(),
        }
    }
    format!("{size}:{}", crate::encoding::hex(&hasher.finalize()))
}

/// A file's size and modification time: whether it's still being written.
fn stamp(path: &Path) -> Option<(u64, SystemTime)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

/// What it knows of a waiting card's file.
struct Watched {
    stamp: Option<(u64, SystemTime)>,
    /// Since when it has had that stamp.
    since: Instant,
    /// Compared since it last changed.
    compared: bool,
}

pub(crate) async fn monitor(factory: Weak<Factory>) {
    let mut watched: HashMap<i64, Watched> = HashMap::new();
    loop {
        let Some(every) = factory.upgrade().map(|factory| times(&factory).0) else { return };
        tokio::time::sleep(every).await;
        let Some(strong) = factory.upgrade() else { return };
        let kept = std::mem::take(&mut watched);
        match tokio::task::spawn_blocking(move || look(&strong, kept)).await {
            Ok(kept) => watched = kept,
            Err(_) => return,
        }
    }
}

/// One look at the waiting review cards' files: marks those whose file
/// changed and has since been quiet.
fn look(factory: &Arc<Factory>, mut watched: HashMap<i64, Watched>) -> HashMap<i64, Watched> {
    let quiet = times(factory).1;
    let cards = match crate::extensions::ask::waiting_reviews(factory) {
        Ok(cards) => cards,
        Err(error) => {
            tracing::warn!(%error, "couldn't read the review cards waiting");
            return watched;
        }
    };
    watched.retain(|id, _| cards.iter().any(|card| card.id == *id));
    let now = Instant::now();
    for card in cards {
        let stamp = stamp(Path::new(&card.review));
        let watch = watched.entry(card.id).or_insert(Watched { stamp, since: now, compared: false });
        if watch.stamp != stamp {
            (watch.stamp, watch.since, watch.compared) = (stamp, now, false);
            continue;
        }
        if watch.compared || stamp.is_none() || now.duration_since(watch.since) < quiet {
            continue;
        }
        watch.compared = true;
        let print = fingerprint(&card.review);
        if print.is_empty() || print == card.seen {
            continue;
        }
        crate::extensions::ask::file_changed(factory, &card, &print);
    }
    watched
}
