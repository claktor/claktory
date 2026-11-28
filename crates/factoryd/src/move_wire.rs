//! `claktory-move/1`, the handshake that moves a Claktory to another
//! computer over iroh (#217, phase 2; docs/MOVE.md, "The handshake"): what
//! both sides share. The source (the running Claktory, `move_source`) dials
//! the target (`factoryd receive`, `move_import::receive`) at the receive
//! code it shows, and they talk in frames on one bidirectional stream.
//!
//! - A frame is its header's length (4 bytes, big-endian) and the header, a
//!   JSON object whose `type` says what it is; a `block` is followed by its
//!   `size` bytes.
//! - What's moved is the tree of an unpacked bundle (`manifest.json`,
//!   `home/…`, `folders/<n>/…`, `checkouts/…`), sent in rounds: each round
//!   lists every path with its blocks' SHA-256, and the target asks only for
//!   the blocks it doesn't have.

use std::io::Read;
use std::net::SocketAddr;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::encoding::{hex, unhex};

/// The move's protocol on iroh.
pub const ALPN: &[u8] = b"claktory-move/1";
/// What a receive code starts with.
pub const SCHEME: &str = "claktory-move://";
/// The largest header a side reads.
pub const MAX_HEADER: usize = 16 << 20;
/// The smallest and largest blocks.
pub const MIN_BLOCK: u64 = 128 << 10;
pub const MAX_BLOCK: u64 = 16 << 20;
/// Files have at most about this many blocks (fewer once blocks are as
/// large as they get).
const BLOCKS_PER_FILE: u64 = 2000;

/// One thing said on the wire. Fields default when absent, so either side
/// can add some; a frame of a type a side doesn't know is `Unknown`, which
/// it ignores.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Frame {
    /// Source → target, first: the receive code's secret, and who it is.
    Hello {
        secret: String,
        #[serde(default)]
        version: String,
        #[serde(default)]
        machine: String,
        #[serde(default)]
        os: String,
        #[serde(default)]
        arch: String,
    },
    /// Target → source: the secret is right; what the target is.
    Welcome {
        #[serde(default)]
        version: String,
        #[serde(default)]
        machine: String,
        #[serde(default)]
        os: String,
        #[serde(default)]
        arch: String,
        /// The target's Claktory home, and its user's home.
        #[serde(default)]
        home: String,
        #[serde(default)]
        user_home: String,
        /// Free bytes where the home is.
        #[serde(default)]
        free_bytes: u64,
        /// The home is on a local disk, and holds no Claktory with work in
        /// it (else the target says `failed` instead).
        #[serde(default)]
        local_disk: bool,
        #[serde(default)]
        fresh: bool,
        /// The source key confirmed the digits before: this carries on a
        /// move, and needs no digits again.
        #[serde(default)]
        resumed: bool,
    },
    /// Either side: its person confirmed the digits.
    Confirmed {},
    /// Source → target: what it means to move, for the target's checks.
    Plan {
        /// The manifest, as `manifest.json` will be.
        manifest: serde_json::Value,
        /// Where folders go, as the person chose: `[[from, to], …]`.
        #[serde(default)]
        map: Vec<(String, String)>,
        /// Commands the agents use, to say which this computer lacks.
        #[serde(default)]
        tools: Vec<String>,
    },
    /// Target → source: what stops the move here (sentences; none: it can
    /// go), the paths as they'll be, and the tools this computer lacks.
    Checked {
        #[serde(default)]
        problems: Vec<String>,
        #[serde(default)]
        map: Vec<(String, String)>,
        #[serde(default)]
        missing: Vec<String>,
        #[serde(default)]
        free_bytes: u64,
    },
    /// Source → target: a round's listing starts. The final one comes while
    /// the source is frozen, and is the whole tree.
    Round {
        n: u32,
        #[serde(default, rename = "final")]
        last: bool,
    },
    /// Source → target: one path of the round's listing.
    Entry(Entry),
    /// Source → target: the round's listing is complete.
    RoundEnd { n: u32 },
    /// Target → source: the blocks of `path` it needs, by index.
    Need { path: String, blocks: Vec<u32> },
    /// Target → source: it needs nothing more this round.
    NeedEnd { n: u32 },
    /// Source → target: block `index` of `path`; `size` bytes follow.
    Block { path: String, index: u32, size: u32 },
    /// Source → target: every needed block was sent.
    BlocksEnd { n: u32 },
    /// Target → source: the round is done; what it has of the tree, and
    /// how many blocks had changed since they were listed.
    RoundDone {
        n: u32,
        #[serde(default)]
        bytes_have: u64,
        #[serde(default)]
        bytes_total: u64,
        #[serde(default)]
        stale: u32,
    },
    /// Target → source: what the import is doing, for the person.
    Progress {
        #[serde(default)]
        step: String,
        #[serde(default)]
        text: String,
    },
    /// Target → source: imported and verified; it starts once committed.
    Ready {
        /// Always None now (no pairing code exists for the old
        /// computer's app: it comes over as a device let in by its account
        /// key); kept so either side parses the other's.
        #[serde(default)]
        old_app_code: Option<String>,
    },
    /// Source → target: the source has moved away (`moved.json` written):
    /// the target is the Claktory now.
    Commit {
        #[serde(default)]
        at_ms: i64,
        /// The manifest's: the move being committed.
        #[serde(default)]
        export_id: String,
    },
    /// Target → source: the commit is recorded; it starts. The point of no
    /// return: after it, the source never cancels.
    Done {},
    /// Source → target: the move is off; the target discards what it has.
    Cancel {
        #[serde(default)]
        why: String,
    },
    /// Either side: the move can't go on, in a sentence for the person.
    Failed { why: String },
    /// A frame of a type this doesn't know.
    #[serde(other)]
    Unknown,
}

/// A path of the tree, in a round's listing.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Relative, `/`-separated, with no `.`, `..` or empty parts.
    pub path: String,
    pub kind: Kind,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub mode: u32,
    #[serde(default)]
    pub mtime_ms: i64,
    /// The file's block size (`block_size`), and each block's SHA-256.
    #[serde(default)]
    pub block: u64,
    #[serde(default)]
    pub hashes: Vec<String>,
    /// A symlink's target.
    #[serde(default)]
    pub target: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    #[default]
    File,
    Dir,
    Symlink,
}

#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("a frame that can't be read: {0}")]
    Bad(String),
}

/// Writes `frame`, then `bytes` (a block's).
pub async fn write<W: AsyncWrite + Unpin>(out: &mut W, frame: &Frame, bytes: &[u8]) -> Result<(), WireError> {
    let header = serde_json::to_vec(frame).map_err(|error| WireError::Bad(error.to_string()))?;
    if header.len() > MAX_HEADER {
        return Err(WireError::Bad("too large".to_owned()));
    }
    let mut buffer = Vec::with_capacity(4 + header.len() + bytes.len());
    buffer.extend_from_slice(&u32::try_from(header.len()).unwrap_or(u32::MAX).to_be_bytes());
    buffer.extend_from_slice(&header);
    buffer.extend_from_slice(bytes);
    out.write_all(&buffer).await?;
    Ok(())
}

/// Reads a frame, and a block's bytes after it (empty for other frames).
pub async fn read<R: AsyncRead + Unpin>(input: &mut R) -> Result<(Frame, Vec<u8>), WireError> {
    let length = input.read_u32().await? as usize;
    if length > MAX_HEADER {
        return Err(WireError::Bad("too large".to_owned()));
    }
    let mut header = vec![0; length];
    input.read_exact(&mut header).await?;
    let frame: Frame = serde_json::from_slice(&header).map_err(|error| WireError::Bad(error.to_string()))?;
    let mut bytes = Vec::new();
    if let Frame::Block { size, .. } = &frame {
        if u64::from(*size) > MAX_BLOCK {
            return Err(WireError::Bad("a block too large".to_owned()));
        }
        bytes = vec![0; *size as usize];
        input.read_exact(&mut bytes).await?;
    }
    Ok((frame, bytes))
}

// ---- blocks ----

/// The block size of a file of `len` bytes: the smallest power of two from
/// 128 KiB to 16 MiB that cuts it into at most 2000 blocks.
pub fn block_size(len: u64) -> u64 {
    let mut block = MIN_BLOCK;
    while block < MAX_BLOCK && len.div_ceil(block) > BLOCKS_PER_FILE {
        block *= 2;
    }
    block
}

/// How many blocks of `block` bytes a file of `len` bytes has.
pub fn block_count(len: u64, block: u64) -> u64 {
    if block == 0 { 0 } else { len.div_ceil(block) }
}

/// The SHA-256 of a block, in hex.
pub fn block_hash(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// The SHA-256 of each block of the file at `path`, cut in blocks of
/// `block` bytes.
pub fn hash_file(path: &Path, block: u64) -> std::io::Result<Vec<String>> {
    let mut file = std::fs::File::open(path)?;
    let mut hashes = Vec::new();
    let mut buffer = vec![0; usize::try_from(block).unwrap_or(usize::MAX)];
    loop {
        let mut filled = 0;
        while filled < buffer.len() {
            let read = file.read(&mut buffer[filled..])?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        if filled == 0 {
            break;
        }
        hashes.push(block_hash(&buffer[..filled]));
        if filled < buffer.len() {
            break;
        }
    }
    Ok(hashes)
}

/// Whether `path` is a tree path both sides accept: relative, `/`-separated,
/// with no `.`, `..` or empty parts.
pub fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains('\0')
        && path.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
}

/// Whether a symlink at `path` pointing to `target` stays in the tree, as a
/// bundle's must: relative, never climbing above the tree's top.
pub fn safe_link(path: &str, target: &str) -> bool {
    if target.is_empty() || target.starts_with('/') || target.contains('\0') {
        return false;
    }
    let mut depth = path.split('/').count().saturating_sub(1);
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." if depth == 0 => return false,
            ".." => depth -= 1,
            _ => depth += 1,
        }
    }
    true
}

// ---- the receive code and the digits ----

/// What `factoryd receive` shows: where the target is, its one-time secret
/// and what it is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReceiveCode {
    /// The target's endpoint id (its own key, not a Claktory's).
    pub endpoint: String,
    /// 32 random bytes, in hex.
    pub secret: String,
    pub machine: String,
    pub version: String,
    pub os: String,
    pub arch: String,
    pub relays: Vec<String>,
    pub addresses: Vec<SocketAddr>,
}

impl ReceiveCode {
    /// `claktory-move://?e=<endpoint>&s=<secret>&m=<machine>&v=<version>&o=<os>&h=<arch>`,
    /// then `&r=<relay>` and `&a=<ip:port>` for each, percent-encoded.
    pub fn text(&self) -> String {
        use crate::remote::encode;
        let mut code = format!(
            "{SCHEME}?e={}&s={}&m={}&v={}&o={}&h={}",
            self.endpoint,
            self.secret,
            encode(&self.machine),
            encode(&self.version),
            encode(&self.os),
            encode(&self.arch)
        );
        for relay in &self.relays {
            code.push_str(&format!("&r={}", encode(relay)));
        }
        for address in &self.addresses {
            code.push_str(&format!("&a={}", encode(&address.to_string())));
        }
        code
    }

    /// Reads a receive code; None when it isn't one.
    pub fn parse(code: &str) -> Option<ReceiveCode> {
        let code = code.trim();
        if !code.starts_with(SCHEME) {
            return None;
        }
        let one = |name: &str| crate::remote::code_values(code, name).next().unwrap_or_default();
        let parsed = ReceiveCode {
            endpoint: one("e"),
            secret: one("s"),
            machine: one("m"),
            version: one("v"),
            os: one("o"),
            arch: one("h"),
            relays: crate::remote::code_values(code, "r").collect(),
            addresses: crate::remote::code_values(code, "a").filter_map(|address| address.parse().ok()).collect(),
        };
        let secret_ok = parsed.secret.len() == 64 && unhex(&parsed.secret).is_some();
        (secret_ok && parsed.endpoint.parse::<iroh::EndpointId>().is_ok()).then_some(parsed)
    }
}

/// The six digits both computers show, so the person can tell the target
/// they're moving to from one that copied its code: from the target's key,
/// the source's and the secret. Written "123 456".
pub fn digits(target: &str, source: &str, secret: &str) -> String {
    let digest = Sha256::digest(format!("claktory move\n{target}\n{source}\n{secret}").as_bytes());
    let number = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) % 1_000_000;
    format!("{:03} {:03}", number / 1000, number % 1000)
}

/// Compares in full whatever matches, so the time taken says nothing.
pub fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0, |diff, (x, y)| diff | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_grow_with_the_file_from_128_kib_to_16_mib() {
        assert_eq!(block_size(0), MIN_BLOCK);
        assert_eq!(block_size(2000 * MIN_BLOCK), MIN_BLOCK);
        assert_eq!(block_size(2000 * MIN_BLOCK + 1), 2 * MIN_BLOCK);
        assert_eq!(block_size(u64::MAX / 2), MAX_BLOCK);
        assert_eq!(block_count(0, MIN_BLOCK), 0);
        assert_eq!(block_count(MIN_BLOCK + 1, MIN_BLOCK), 2);
    }

    #[test]
    fn a_file_hashes_block_by_block() {
        let dir = std::env::temp_dir().join(format!("move-wire-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f");
        let content: Vec<u8> = (0..(MIN_BLOCK + 10)).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &content).unwrap();
        let hashes = hash_file(&path, MIN_BLOCK).unwrap();
        assert_eq!(hashes, vec![block_hash(&content[..MIN_BLOCK as usize]), block_hash(&content[MIN_BLOCK as usize..])]);
        std::fs::write(&path, b"").unwrap();
        assert!(hash_file(&path, MIN_BLOCK).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn paths_stay_in_the_tree() {
        assert!(safe_path("home/agents/a/segment-1.json"));
        for bad in ["", "/etc/passwd", "a/../b", "./a", "a//b", "a/", "a\\b"] {
            assert!(!safe_path(bad), "{bad}");
        }
        assert!(safe_link("home/a/link", "../b") && safe_link("home/link", "../folders/0"));
        assert!(!safe_link("home/link", "../../etc") && !safe_link("link", "..") && !safe_link("a/l", "/etc"));
    }

    #[test]
    fn a_receive_code_reads_back_and_refuses_what_isnt_one() {
        let key = iroh::SecretKey::generate();
        let code = ReceiveCode {
            endpoint: key.public().to_string(),
            secret: "ab".repeat(32),
            machine: "Matt's cloud box".to_owned(),
            version: "v0.1.20".to_owned(),
            os: "linux".to_owned(),
            arch: "x86_64".to_owned(),
            relays: vec!["https://relay.example/".to_owned()],
            addresses: vec!["192.0.2.1:4433".parse().unwrap()],
        };
        assert_eq!(ReceiveCode::parse(&code.text()), Some(code.clone()));
        assert_eq!(ReceiveCode::parse(&code.text().replace(&code.secret, "ab")), None);
        assert_eq!(ReceiveCode::parse("factory://?t=x"), None);
    }

    #[test]
    fn digits_are_six_and_differ_by_key() {
        let one = digits("t", "s", "x");
        assert_eq!(one.len(), 7);
        assert_eq!(one, digits("t", "s", "x"));
        assert_ne!(one, digits("t", "other", "x"));
    }

    #[tokio::test]
    async fn frames_and_blocks_go_through_and_unknown_ones_are_kept_apart() {
        let (mut a, mut b) = tokio::io::duplex(1 << 20);
        let entry = Frame::Entry(Entry {
            path: "home/config.json".to_owned(),
            size: 3,
            block: MIN_BLOCK,
            hashes: vec![block_hash(b"abc")],
            ..Entry::default()
        });
        write(&mut a, &entry, &[]).await.unwrap();
        write(&mut a, &Frame::Block { path: "home/config.json".to_owned(), index: 0, size: 3 }, b"abc").await.unwrap();
        write(&mut a, &Frame::Round { n: 2, last: true }, &[]).await.unwrap();
        let unknown = br#"{"type":"from_the_future","x":1}"#;
        a.write_all(&(unknown.len() as u32).to_be_bytes()).await.unwrap();
        a.write_all(unknown).await.unwrap();
        assert_eq!(read(&mut b).await.unwrap(), (entry, Vec::new()));
        assert_eq!(read(&mut b).await.unwrap().1, b"abc");
        let (round, _) = read(&mut b).await.unwrap();
        assert_eq!(round, Frame::Round { n: 2, last: true });
        assert_eq!(serde_json::to_value(&round).unwrap()["final"], true);
        assert_eq!(read(&mut b).await.unwrap().0, Frame::Unknown);
    }
}
