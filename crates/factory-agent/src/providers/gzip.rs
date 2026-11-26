//! Request bodies sent gzip-compressed (#454), to providers that take them.
//!
//! A request carries the whole conversation, so most of what Claktory
//! uploads is request bodies; their JSON shrinks about 2× with gzip at its
//! fastest level (2.1× on a conversation's text, ~90 MB/s). Anthropic's API
//! takes gzip and refuses zstd ("Unsupported Content-Encoding", 400
//! `request_body_encoding_unsupported`), as a Claude subscription's
//! requests to Haiku 4.5 and Opus 5.5 showed on 2026-11-25. A provider that
//! refuses a compressed body gets it again as it is, and nothing compressed
//! from then on, until Claktory restarts.

use std::io::Write;
use std::sync::LazyLock;

use hashbrown::HashSet;
use parking_lot::Mutex;

/// Smaller bodies go as they are: too little to save.
const MIN_BYTES: usize = 1024;

/// Base URLs that refused a compressed body.
static REFUSED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// Whether a request to `base_url` goes compressed, when its provider takes
/// compressed bodies (`takes_gzip`): unless that address refused one.
pub fn wanted(base_url: &str, takes_gzip: bool) -> bool {
    takes_gzip && !REFUSED.lock().contains(base_url)
}

/// `body` gzip-compressed, when it's worth it.
pub fn compress(body: &str) -> Option<Vec<u8>> {
    if body.len() < MIN_BYTES {
        return None;
    }
    let mut encoder = flate2::write::GzEncoder::new(Vec::with_capacity(body.len() / 2), flate2::Compression::fast());
    encoder.write_all(body.as_bytes()).ok()?;
    encoder.finish().ok()
}

/// Whether a response refused the request for its body's encoding:
/// Anthropic's 400 says `request_body_encoding_unsupported`; a server that
/// doesn't decompress commonly answers 415, or 400 naming Content-Encoding.
pub fn refused(status: u16, text: &str) -> bool {
    status == 415
        || (status == 400 && (text.contains("request_body_encoding_unsupported") || text.to_ascii_lowercase().contains("content-encoding")))
}

/// Sends nothing compressed to `base_url` from now on.
pub fn refuse(base_url: &str) {
    tracing::warn!(base_url, "a provider refused a gzip-compressed request body; sending them uncompressed");
    REFUSED.lock().insert(base_url.to_owned());
}
