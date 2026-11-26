//! C1 (#435, SPLIT.md): what factoryd and its browser runner say to each
//! other over the runner's stdin and stdout, the only way they talk: no
//! socket, on the host or in the Mac's VM.
//!
//! One stream each way of frames: a channel byte, the payload's length
//! (u32, big-endian), then the payload. Each channel has its own cap,
//! checked before anything is read or allocated, both ways (Fugo 36536:
//! whatever the runner sends is a hostile page's, after a Chrome escape).
//!
//! - 0 control: one JSON object ([`Control`] in, [`Report`] out).
//! - 1 CDP: one DevTools Protocol message (JSON), as is, both ways; only
//!   while Chrome is driven, since an undriven Chrome has no pipe at all.
//! - 2 frames: out, a [`FrameHeader`] then the JPEG; in, the ack, the
//!   frame's seq as 4 bytes. One frame in flight.
//! - 3 input, in: one JSON [`InputEvent`].
//! - 4 clipboard: UTF-8 text, both ways: in, pasted; out, what was copied,
//!   only while watched ([`Control::Clipboard`]).
//! - 5 files, out only: a download's bytes, read with
//!   [`Control::DownloadRead`]: its 16-byte GUID, then at most
//!   [`MAX_FILE_CHUNK`] of the file; a frame with the GUID alone ends it
//!   (agent-browser.md R9, Fugo 36914).

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

/// This wire's version, in [`Report::Ready`]: a runner of another (an
/// older Mac image, say) is stopped rather than half understood.
/// 2: input leases ([`Control::LeaseInput`], R11): a runner that gates
/// `Input.*` on them, so a host never drives one that doesn't.
pub const VERSION: u32 = 2;

/// The bytes before each payload: the channel, then its length.
pub const HEADER: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Control = 0,
    Cdp = 1,
    Frames = 2,
    Input = 3,
    Clipboard = 4,
    Files = 5,
}

/// Which way a frame goes: its caps differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
    /// factoryd to the runner.
    In,
    /// The runner to factoryd.
    Out,
}

/// The most a control message takes.
pub const MAX_CONTROL: usize = 64 * 1024;
/// The most a CDP message takes, either way.
pub const MAX_CDP: usize = 8 * 1024 * 1024;
/// The most a frame takes: its header and the JPEG.
pub const MAX_FRAME: usize = 8 * 1024 * 1024;
/// The most an input event takes (its text at most [`MAX_TEXT`] characters).
pub const MAX_INPUT: usize = 32 * 1024;
/// The most text one input event types.
pub const MAX_TEXT: usize = 4096;
/// The most clipboard text, either way.
pub const MAX_CLIPBOARD: usize = 1024 * 1024;
/// The most of a download one frame carries, after its GUID.
pub const MAX_FILE_CHUNK: usize = 1024 * 1024;

impl Channel {
    pub fn from_byte(byte: u8) -> Option<Channel> {
        Some(match byte {
            0 => Channel::Control,
            1 => Channel::Cdp,
            2 => Channel::Frames,
            3 => Channel::Input,
            4 => Channel::Clipboard,
            5 => Channel::Files,
            _ => return None,
        })
    }

    /// The most a payload on this channel may take going `way`; None for a
    /// channel that never goes that way.
    pub fn cap(self, way: Way) -> Option<usize> {
        match (self, way) {
            (Channel::Control, _) => Some(MAX_CONTROL),
            (Channel::Cdp, _) => Some(MAX_CDP),
            (Channel::Frames, Way::In) => Some(4),
            (Channel::Frames, Way::Out) => Some(MAX_FRAME),
            (Channel::Input, Way::In) => Some(MAX_INPUT),
            (Channel::Input, Way::Out) => None,
            (Channel::Clipboard, _) => Some(MAX_CLIPBOARD),
            (Channel::Files, Way::In) => None,
            (Channel::Files, Way::Out) => Some(16 + MAX_FILE_CHUNK),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("the stream ended in the middle of a frame")]
    Truncated,
    #[error("unknown channel {0}")]
    UnknownChannel(u8),
    #[error("channel {0:?} doesn't go this way")]
    WrongWay(Channel),
    #[error("a {channel:?} frame of {len} bytes, more than its {cap}")]
    TooLong { channel: Channel, len: usize, cap: usize },
    #[error("{0}")]
    Io(#[from] io::Error),
}

/// A frame's header, read: its channel and its payload's length, checked
/// against the channel's cap going `way` before anything is allocated.
/// factoryd's side (tokio) reads [`HEADER`] bytes itself and calls this.
pub fn parse_header(header: [u8; HEADER], way: Way) -> Result<(Channel, usize), WireError> {
    let channel = Channel::from_byte(header[0]).ok_or(WireError::UnknownChannel(header[0]))?;
    let cap = channel.cap(way).ok_or(WireError::WrongWay(channel))?;
    let len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
    if len > cap {
        return Err(WireError::TooLong { channel, len, cap });
    }
    Ok((channel, len))
}

/// A frame's header for `payload` on `channel` going `way`; refused past
/// the channel's cap, so neither side ever sends what the other refuses.
pub fn header(channel: Channel, payload: usize, way: Way) -> Result<[u8; HEADER], WireError> {
    let cap = channel.cap(way).ok_or(WireError::WrongWay(channel))?;
    if payload > cap {
        return Err(WireError::TooLong { channel, len: payload, cap });
    }
    let len = (payload as u32).to_be_bytes();
    Ok([channel as u8, len[0], len[1], len[2], len[3]])
}

/// Writes one frame (blocking: the runner's side).
pub fn write(out: &mut impl Write, channel: Channel, payload: &[u8], way: Way) -> Result<(), WireError> {
    out.write_all(&header(channel, payload.len(), way)?)?;
    out.write_all(payload)?;
    Ok(())
}

/// Reads one frame (blocking: the runner's side); None at the end of the
/// stream between frames.
pub fn read(input: &mut impl Read, way: Way) -> Result<Option<(Channel, Vec<u8>)>, WireError> {
    let mut head = [0u8; HEADER];
    let mut got = 0;
    while got < HEADER {
        match input.read(&mut head[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(WireError::Truncated),
            Ok(n) => got += n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
    let (channel, len) = parse_header(head, way)?;
    let mut payload = vec![0u8; len];
    input.read_exact(&mut payload).map_err(|error| match error.kind() {
        io::ErrorKind::UnexpectedEof => WireError::Truncated,
        _ => WireError::Io(error),
    })?;
    Ok(Some((channel, payload)))
}

/// Whether Chrome runs with the DevTools pipe (an agent drives it) or
/// without (the person's turn: no pipe exists, so nothing can drive it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Driven,
    Undriven,
}

/// Control, factoryd to the runner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Control {
    /// Start the screen and Chrome on the kept profile `profile` (a short
    /// name, `[a-z0-9-]`, 1 to 32; the runner keeps it under the uid of
    /// the person who ran it, so one person's never opens another's), at
    /// `url` or where it left off.
    Start {
        profile: String,
        mode: Mode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        width: u16,
        height: u16,
        /// The person's time zone, as IANA names it ("Europe/London"), so
        /// the browser's is theirs (agent-browser-checks.md change 3); UTC
        /// when left out. For the whole session, every mode.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        time_zone: Option<String>,
        /// Their languages, BCP 47 tags, the one they prefer first
        /// (["en-GB", "en"]); en-US and en when left out.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        languages: Vec<String>,
    },
    /// Quit Chrome as a person would (so its cookies are written) and start
    /// it again in `mode`, on the same profile and screen.
    Mode {
        mode: Mode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },
    /// Quit Chrome as a person would, then the screen; the runner ends.
    Stop,
    /// The person's Delete browser data: Chrome quit if it runs (on this
    /// profile only), the kept profile `profile` deleted, and the runner
    /// ends, as after [`Control::Stop`]. It can come first, without a start.
    DeleteData { profile: String },
    /// Frames on or off, at most `max_width` wide; `quality_hint` 0–100,
    /// 0 for the runner's own choice.
    View { on: bool, max_width: u16, quality_hint: u8 },
    /// The screen's new size (a phone's shape, say).
    Resize { width: u16, height: u16 },
    /// Send what's copied (channel 4 out) or not: on only during the
    /// person's turn or a take-over.
    Clipboard { watch: bool },
    /// Send the finished download `guid` on channel 5 (R9).
    DownloadRead { guid: String },
    /// Delete the download `guid`, read or not.
    DownloadDrop { guid: String },
    /// Let the host's `Input.*` calls through to the driven Chrome for at
    /// most `for_ms` (capped at ten minutes) or until [`Control::EndLease`]:
    /// one agent action's (R11.3 #5). It replaces a lease still live.
    /// Answered with [`Report::Lease`].
    LeaseInput { id: u64, for_ms: u32 },
    /// The lease `id` is over: `Input.*` is refused again.
    EndLease { id: u64 },
}

/// Control, the runner to factoryd. Hostile input once read (a page that
/// escaped Chrome runs as the runner's user): factoryd checks and caps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Report {
    /// Started: the wire's [`VERSION`], Chrome's version, the kernel's
    /// Landlock ABI (0 in none), the mode Chrome runs in, and the folder
    /// driven downloads land in (R9; absent from a runner without them).
    Ready {
        wire: u32,
        chrome: String,
        landlock_abi: u32,
        mode: Mode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        download_dir: Option<String>,
    },
    /// Chrome started again in `mode` after [`Control::Mode`].
    ModeChanged { mode: Mode },
    /// The screen's size now, after [`Control::Resize`].
    Resized { width: u16, height: u16 },
    /// Chrome ended on its own (crashed, or quit from its menu).
    ChromeExited {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<i32>,
    },
    /// The profile was deleted.
    DataDeleted,
    /// Stopped, Chrome having quit in `ms`; the runner ends after this.
    Stopped { ms: u64 },
    /// What went wrong, in a short line; `fatal` when the runner ends.
    Error { what: String, fatal: bool },
    /// A download finished in Chrome (R9): its GUID, and its size and
    /// SHA-256 as the runner saw them, a hint only (factoryd counts and
    /// hashes what it reads itself).
    DownloadDone { guid: String, bytes: u64, sha256: String },
    /// A download the runner cancelled and deleted: past the size cap,
    /// too many at once, the disk too full, or not driven.
    DownloadCancelled { guid: String, why: String },
    /// The lease `id` asked for: granted, or refused and why (no driven
    /// Chrome).
    Lease {
        id: u64,
        granted: bool,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        why: String,
    },
    /// A lease ended before its [`Control::EndLease`]: it ran out, another
    /// replaced it, or Chrome stopped, changed mode or started again.
    LeaseEnded { id: u64, why: String },
}

/// A frame's header (channel 2 out), 12 bytes, big-endian: its seq, the
/// JPEG's size and the screen's, so input can be given in screen pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub seq: u32,
    pub width: u16,
    pub height: u16,
    pub screen_width: u16,
    pub screen_height: u16,
}

impl FrameHeader {
    pub const LEN: usize = 12;

    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut bytes = [0u8; Self::LEN];
        bytes[0..4].copy_from_slice(&self.seq.to_be_bytes());
        bytes[4..6].copy_from_slice(&self.width.to_be_bytes());
        bytes[6..8].copy_from_slice(&self.height.to_be_bytes());
        bytes[8..10].copy_from_slice(&self.screen_width.to_be_bytes());
        bytes[10..12].copy_from_slice(&self.screen_height.to_be_bytes());
        bytes
    }

    /// The header and the JPEG after it, from a frame's payload.
    pub fn split(payload: &[u8]) -> Option<(FrameHeader, &[u8])> {
        if payload.len() < Self::LEN {
            return None;
        }
        let u16_at = |at: usize| u16::from_be_bytes([payload[at], payload[at + 1]]);
        let header = FrameHeader {
            seq: u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]),
            width: u16_at(4),
            height: u16_at(6),
            screen_width: u16_at(8),
            screen_height: u16_at(10),
        };
        Some((header, &payload[Self::LEN..]))
    }
}

/// A frame's ack (channel 2 in): its seq.
pub fn ack(seq: u32) -> [u8; 4] {
    seq.to_be_bytes()
}

/// A frame's ack, read; None for a payload that isn't one.
pub fn read_ack(payload: &[u8]) -> Option<u32> {
    <[u8; 4]>::try_from(payload).ok().map(u32::from_be_bytes)
}

/// A mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub enum Button {
    Left,
    Middle,
    Right,
}

impl TryFrom<u8> for Button {
    type Error = String;
    fn try_from(value: u8) -> Result<Self, String> {
        match value {
            0 => Ok(Button::Left),
            1 => Ok(Button::Middle),
            2 => Ok(Button::Right),
            other => Err(format!("no button {other}")),
        }
    }
}

impl From<Button> for u8 {
    fn from(button: Button) -> u8 {
        match button {
            Button::Left => 0,
            Button::Middle => 1,
            Button::Right => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modifier {
    Ctrl,
    Shift,
    Alt,
    Meta,
}

/// Input (channel 3 in), the person's, in screen pixels (never the
/// frame's: factoryd or the app scales by the frame's header). The runner
/// applies what comes and drops what it can't read; the turn and the rate
/// are factoryd's to enforce.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum InputEvent {
    Move { x: u32, y: u32 },
    Down { button: Button, x: u32, y: u32 },
    Up { button: Button, x: u32, y: u32 },
    Click { x: u32, y: u32 },
    /// In notches, each at most ±10.
    Wheel { x: u32, y: u32, dx: i32, dy: i32 },
    /// A DOM `KeyboardEvent.key` ("Enter", "ArrowLeft", "F5", "a"), held
    /// with `mods` as a chord.
    Key {
        key: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        mods: Vec<Modifier>,
    },
    /// Text typed as it is, at most [`MAX_TEXT`] characters.
    Text { s: String },
}

impl InputEvent {
    /// The event in a payload, within its limits; None for one to drop.
    pub fn parse(payload: &[u8]) -> Option<InputEvent> {
        if payload.len() > MAX_INPUT {
            return None;
        }
        let event: InputEvent = serde_json::from_slice(payload).ok()?;
        match &event {
            InputEvent::Wheel { dx, dy, .. } if dx.abs() > 10 || dy.abs() > 10 => None,
            InputEvent::Text { s } if s.chars().count() > MAX_TEXT => None,
            InputEvent::Key { key, mods } if key.is_empty() || key.chars().count() > 32 || mods.len() > 4 => None,
            _ => Some(event),
        }
    }
}

/// A control message's payload.
pub fn encode<T: Serialize>(message: &T) -> Vec<u8> {
    serde_json::to_vec(message).unwrap_or_default()
}

/// A control message, read from a payload already within [`MAX_CONTROL`].
pub fn decode<'a, T: Deserialize<'a>>(payload: &'a [u8]) -> Option<T> {
    if payload.len() > MAX_CONTROL {
        return None;
    }
    serde_json::from_slice(payload).ok()
}

/// Whether `guid` is a download's GUID as Chrome names its files
/// (allowAndName): 36 of `[0-9a-f-]`, and nothing else ever reaches a path.
pub fn guid_ok(guid: &str) -> bool {
    guid.len() == 36 && guid.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte) || byte == b'-')
}

/// A download's GUID as channel 5 carries it: its 32 hex digits as 16
/// bytes; None for one that isn't a GUID.
pub fn guid_bytes(guid: &str) -> Option<[u8; 16]> {
    if !guid_ok(guid) {
        return None;
    }
    let hex: Vec<u8> = guid.bytes().filter(|byte| *byte != b'-').collect();
    if hex.len() != 32 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for (at, pair) in hex.chunks(2).enumerate() {
        bytes[at] = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(bytes)
}

/// The most languages a session takes.
pub const MAX_LANGUAGES: usize = 8;

/// Whether `zone` is a time zone's name as IANA writes them (Fugo 37383):
/// 1 to 64 of `[A-Za-z0-9_+-/]` in segments, none empty, `.` or `..`, so
/// never a path (glibc reads a `TZ` starting with `/` or `:` as a file).
pub fn zone_name_ok(zone: &str) -> bool {
    (1..=64).contains(&zone.len())
        && zone.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_+-/".contains(&byte))
        && zone.split('/').all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

/// Whether `tag` is a language as BCP 47 writes them, as far as Chrome
/// needs: 2 or 3 letters, then up to 4 subtags of 1 to 8 letters or digits
/// (`^[A-Za-z]{2,3}(-[A-Za-z0-9]{1,8}){0,4}$`, Fugo 37383).
pub fn language_ok(tag: &str) -> bool {
    let mut parts = tag.split('-');
    let first = parts.next().unwrap_or_default();
    let rest: Vec<&str> = parts.collect();
    (2..=3).contains(&first.len())
        && first.bytes().all(|byte| byte.is_ascii_alphabetic())
        && rest.len() <= 4
        && rest.iter().all(|part| (1..=8).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_alphanumeric()))
}

/// Whether `name` may name a kept profile.
pub fn profile_name_ok(name: &str) -> bool {
    (1..=32).contains(&name.len()) && name.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_go_both_ways_within_their_channels_caps() {
        let mut stream = Vec::new();
        write(&mut stream, Channel::Control, &encode(&Control::Stop), Way::In).unwrap();
        write(&mut stream, Channel::Frames, &ack(7), Way::In).unwrap();
        write(&mut stream, Channel::Input, br#"{"t":"click","x":3,"y":4}"#, Way::In).unwrap();
        let mut reader = stream.as_slice();
        let (channel, payload) = read(&mut reader, Way::In).unwrap().unwrap();
        assert_eq!((channel, decode::<Control>(&payload)), (Channel::Control, Some(Control::Stop)));
        let (channel, payload) = read(&mut reader, Way::In).unwrap().unwrap();
        assert_eq!((channel, read_ack(&payload)), (Channel::Frames, Some(7)));
        let (channel, payload) = read(&mut reader, Way::In).unwrap().unwrap();
        assert_eq!((channel, InputEvent::parse(&payload)), (Channel::Input, Some(InputEvent::Click { x: 3, y: 4 })));
        assert!(read(&mut reader, Way::In).unwrap().is_none());

        // Refused before anything is allocated: too long, unknown, the wrong way.
        let long = [Channel::Control as u8, 0xff, 0xff, 0xff, 0xff];
        assert!(matches!(parse_header(long, Way::Out), Err(WireError::TooLong { .. })));
        assert!(matches!(parse_header([9, 0, 0, 0, 0], Way::Out), Err(WireError::UnknownChannel(9))));
        assert!(matches!(parse_header([Channel::Input as u8, 0, 0, 0, 1], Way::Out), Err(WireError::WrongWay(Channel::Input))));
        // An ack is 4 bytes; a frame's 8 MiB only goes out.
        assert!(matches!(parse_header([Channel::Frames as u8, 0, 0, 0, 5], Way::In), Err(WireError::TooLong { .. })));
        assert!(header(Channel::Frames, MAX_FRAME, Way::Out).is_ok());
        assert!(write(&mut Vec::new(), Channel::Control, &vec![b' '; MAX_CONTROL + 1], Way::In).is_err());
        // Cut off in the middle.
        let mut cut = Vec::new();
        write(&mut cut, Channel::Clipboard, b"hello", Way::Out).unwrap();
        cut.truncate(HEADER + 2);
        assert!(matches!(read(&mut cut.as_slice(), Way::Out), Err(WireError::Truncated)));
        assert!(matches!(read(&mut &cut[..3], Way::Out), Err(WireError::Truncated)));
    }

    #[test]
    fn messages_read_as_the_contract_spells_them() {
        let start: Control =
            decode(br#"{"op":"start","profile":"claktory","mode":"driven","url":"https://play.google.com/console","width":1280,"height":800}"#)
                .unwrap();
        assert_eq!(
            start,
            Control::Start {
                profile: "claktory".into(),
                mode: Mode::Driven,
                url: Some("https://play.google.com/console".into()),
                width: 1280,
                height: 800,
                time_zone: None,
                languages: Vec::new()
            }
        );
        assert_eq!(
            decode::<Control>(br#"{"op":"start","profile":"p","mode":"driven","width":1920,"height":1080,"time_zone":"Europe/London","languages":["en-GB","en"]}"#),
            Some(Control::Start {
                profile: "p".into(),
                mode: Mode::Driven,
                url: None,
                width: 1920,
                height: 1080,
                time_zone: Some("Europe/London".into()),
                languages: vec!["en-GB".into(), "en".into()]
            })
        );
        for zone in ["UTC", "Europe/London", "America/Argentina/Buenos_Aires", "Etc/GMT+5", "America/Port-au-Prince"] {
            assert!(zone_name_ok(zone), "{zone}");
        }
        for zone in ["", "/etc/passwd", ":/etc/passwd", "../../etc/passwd", "Europe/../../etc", "Europe//London", "Europe/London/", "Europe/./London", "Europe/Lon don", "Europe\\London", &"A".repeat(65)] {
            assert!(!zone_name_ok(zone), "{zone}");
        }
        for tag in ["en", "en-GB", "zh-Hant-TW", "sr-Latn-RS", "de-CH-1996", "yue"] {
            assert!(language_ok(tag), "{tag}");
        }
        for tag in ["", "e", "english", "en_GB", "en-", "-en", "en-GB-x-y-z-w", "en-123456789", "en,de", "en;q=0.9", "1en"] {
            assert!(!language_ok(tag), "{tag}");
        }
        assert_eq!(decode::<Control>(br#"{"op":"view","on":true,"max_width":780,"quality_hint":0}"#), Some(Control::View { on: true, max_width: 780, quality_hint: 0 }));
        assert_eq!(decode::<Control>(br#"{"op":"clipboard","watch":false}"#), Some(Control::Clipboard { watch: false }));
        assert_eq!(decode::<Control>(br#"{"op":"launch_missiles"}"#), None);
        assert_eq!(
            String::from_utf8(encode(&Report::Ready {
                wire: VERSION,
                chrome: "158.0.8127.88".into(),
                landlock_abi: 4,
                mode: Mode::Undriven,
                download_dir: None
            }))
            .unwrap(),
            r#"{"op":"ready","wire":2,"chrome":"158.0.8127.88","landlock_abi":4,"mode":"undriven"}"#
        );
        // Input leases (R11, wire 2).
        assert_eq!(decode::<Control>(br#"{"op":"lease_input","id":7,"for_ms":4000}"#), Some(Control::LeaseInput { id: 7, for_ms: 4000 }));
        assert_eq!(decode::<Control>(br#"{"op":"end_lease","id":7}"#), Some(Control::EndLease { id: 7 }));
        assert_eq!(String::from_utf8(encode(&Report::Lease { id: 7, granted: true, why: String::new() })).unwrap(), r#"{"op":"lease","id":7,"granted":true}"#);
        assert_eq!(
            String::from_utf8(encode(&Report::LeaseEnded { id: 7, why: "it ran out".into() })).unwrap(),
            r#"{"op":"lease_ended","id":7,"why":"it ran out"}"#
        );

        let frame = FrameHeader { seq: 0x0102_0304, width: 780, height: 487, screen_width: 1280, screen_height: 800 };
        let mut payload = frame.encode().to_vec();
        payload.extend_from_slice(b"\xff\xd8jpeg");
        assert_eq!(FrameHeader::split(&payload), Some((frame, &b"\xff\xd8jpeg"[..])));
        assert_eq!(FrameHeader::split(&payload[..11]), None);

        assert_eq!(
            InputEvent::parse(br#"{"t":"key","key":"a","mods":["ctrl"]}"#),
            Some(InputEvent::Key { key: "a".into(), mods: vec![Modifier::Ctrl] })
        );
        assert_eq!(InputEvent::parse(br#"{"t":"down","button":2,"x":1,"y":2}"#), Some(InputEvent::Down { button: Button::Right, x: 1, y: 2 }));
        assert_eq!(InputEvent::parse(br#"{"t":"down","button":3,"x":1,"y":2}"#), None);
        assert_eq!(InputEvent::parse(br#"{"t":"wheel","x":1,"y":2,"dx":0,"dy":-11}"#), None);
        let long = format!(r#"{{"t":"text","s":"{}"}}"#, "é".repeat(MAX_TEXT + 1));
        assert_eq!(InputEvent::parse(long.as_bytes()), None);
        assert_eq!(InputEvent::parse(br#"{"t":"exec"}"#), None);

        // Downloads (R9): channel 5 only goes out, a GUID and 1 MiB at most.
        assert!(matches!(parse_header([Channel::Files as u8, 0, 0, 0, 16], Way::In), Err(WireError::WrongWay(Channel::Files))));
        assert!(header(Channel::Files, 16 + MAX_FILE_CHUNK, Way::Out).is_ok() && header(Channel::Files, 17 + MAX_FILE_CHUNK, Way::Out).is_err());
        assert_eq!(
            decode::<Control>(br#"{"op":"download_read","guid":"3f1c9a2e-0b6d-4e8f-9a1b-2c3d4e5f6a7b"}"#),
            Some(Control::DownloadRead { guid: "3f1c9a2e-0b6d-4e8f-9a1b-2c3d4e5f6a7b".into() })
        );
        assert_eq!(guid_bytes("3f1c9a2e-0b6d-4e8f-9a1b-2c3d4e5f6a7b").map(|bytes| bytes[0]), Some(0x3f));
        for bad in ["../../etc/passwd", "3F1C9A2E-0B6D-4E8F-9A1B-2C3D4E5F6A7B", "3f1c9a2e-0b6d-4e8f-9a1b-2c3d4e5f6a7", "3f1c9a2e/0b6d-4e8f-9a1b-2c3d4e5f6a7b"] {
            assert!(!guid_ok(bad) && guid_bytes(bad).is_none(), "{bad}");
        }
        assert!(profile_name_ok("claktory") && profile_name_ok("dev-2"));
        assert!(!profile_name_ok("") && !profile_name_ok("../x") && !profile_name_ok("Claktory") && !profile_name_ok(&"a".repeat(33)));
    }
}
