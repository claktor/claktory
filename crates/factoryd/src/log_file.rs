//! factoryd.log stays bounded (#525): whatever started the backend (launchd,
//! systemd, scripts/dev.sh) writes its output to `<home>/factoryd.log`,
//! opened to append, and nothing rotated it. Past [`CAP`], the backend copies
//! it to `factoryd.log.1` (the one before to `.2`, [`KEPT`] in all) and empties
//! it, which every process writing it (the supervisor too) follows, since
//! they append. And a line logged over and over is shown at most
//! [`SHOWN`] times a minute, then counted: "… repeated N more times".

use std::collections::HashMap;
use std::io::Write;
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How big factoryd.log grows before it's rotated.
pub const CAP: u64 = 50 * 1024 * 1024;
/// How many old logs are kept: factoryd.log.1 and .2.
const KEPT: u32 = 2;
/// How often its size is checked.
const EVERY: Duration = Duration::from_secs(60);
/// How many times the same line is written in a minute.
const SHOWN: u32 = 5;
const WINDOW: Duration = Duration::from_secs(60);
/// The most different lines counted at once.
const MOST_LINES: usize = 512;

/// Checks the log every minute while the backend runs.
pub(crate) async fn watch(log: PathBuf) {
    loop {
        tokio::time::sleep(EVERY).await;
        let path = log.clone();
        let _ = tokio::task::spawn_blocking(move || match rotate(&path, &std::io::stdout(), CAP) {
            Ok(true) => tracing::info!(log = %path.display(), "rotated the log"),
            Ok(false) => {}
            Err(error) => tracing::warn!(%error, "could not rotate the log"),
        })
        .await;
        repeats_said();
    }
}

/// Rotates `log` when `output` (the backend's standard output) is that very
/// file, opened to append, and bigger than `cap`: copies it to `log.1`
/// (after moving the older ones up) and empties it. A writer at its own
/// place would leave a hole, so then it's left alone. Whether it rotated.
#[cfg(unix)]
pub fn rotate(log: &Path, output: &impl AsRawFd, cap: u64) -> std::io::Result<bool> {
    let fd = output.as_raw_fd();
    // SAFETY: fd is open for as long as `output` is; F_GETFL only reads its flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || flags & libc::O_APPEND == 0 {
        return Ok(false);
    }
    // SAFETY: an all-zero stat is a valid value for fstat to fill in.
    let mut ours: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fd is open, and `ours` is a stat to write.
    if unsafe { libc::fstat(fd, &raw mut ours) } != 0 {
        return Ok(false);
    }
    let Ok(at) = std::fs::metadata(log) else { return Ok(false) };
    if !at.is_file() || at.dev() != ours.st_dev as u64 || at.ino() != ours.st_ino as u64 || at.len() <= cap {
        return Ok(false);
    }
    let old = |n: u32| PathBuf::from(format!("{}.{n}", log.display()));
    let _ = std::fs::remove_file(old(KEPT));
    for n in (1..KEPT).rev() {
        match std::fs::rename(old(n), old(n + 1)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
    }
    // Copied to its end as it is when read, then emptied at once: what's
    // written in between can be lost.
    let mut from = std::fs::File::open(log)?;
    let mut to = std::fs::OpenOptions::new().write(true).create_new(true).open(old(1))?;
    std::io::copy(&mut from, &mut to)?;
    let file = std::fs::OpenOptions::new().write(true).open(log)?;
    let check = file.metadata()?;
    if check.ino() != ours.st_ino as u64 {
        return Ok(false);
    }
    file.set_len(0)?;
    Ok(true)
}

/// As above, on Windows: `output` is the log when it's the same file
/// (volume and index) opened to append only (FILE_APPEND_DATA without
/// FILE_WRITE_DATA), which writes at the end whatever its length.
#[cfg(windows)]
pub fn rotate(log: &Path, output: &impl std::os::windows::io::AsRawHandle, cap: u64) -> std::io::Result<bool> {
    use crate::windows::fs::{appends_only, identity, identity_of};
    if !appends_only(output).unwrap_or(false) {
        return Ok(false);
    }
    let Ok(ours) = identity_of(output) else { return Ok(false) };
    let Ok(at) = std::fs::metadata(log) else { return Ok(false) };
    if !at.is_file() || identity(log).ok() != Some(ours) || at.len() <= cap {
        return Ok(false);
    }
    let old = |n: u32| PathBuf::from(format!("{}.{n}", log.display()));
    let _ = std::fs::remove_file(old(KEPT));
    for n in (1..KEPT).rev() {
        match std::fs::rename(old(n), old(n + 1)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
    }
    let mut from = std::fs::File::open(log)?;
    let mut to = std::fs::OpenOptions::new().write(true).create_new(true).open(old(1))?;
    std::io::copy(&mut from, &mut to)?;
    let file = std::fs::OpenOptions::new().write(true).open(log)?;
    if identity_of(&file)?.index != ours.index {
        return Ok(false);
    }
    file.set_len(0)?;
    Ok(true)
}

/// The lines written lately, by what they say (all but their time).
struct Repeats {
    lines: HashMap<String, Seen>,
}

struct Seen {
    since: Instant,
    times: u32,
    /// The level and target, to say the count as the line did.
    said: String,
}

static REPEATS: Mutex<Option<Repeats>> = Mutex::new(None);

/// Whether to write `line` now; a line saying how often the one before it
/// went unshown, to write first, when its minute is over.
fn heard(line: &str, now: Instant) -> (bool, Option<String>) {
    let what = line.split_once(' ').map_or(line, |(_, rest)| rest.trim_start());
    let mut repeats = crate::locking::lock(&REPEATS);
    let repeats = repeats.get_or_insert_with(|| Repeats { lines: HashMap::new() });
    if repeats.lines.len() >= MOST_LINES && !repeats.lines.contains_key(what) {
        repeats.lines.retain(|_, seen| now.duration_since(seen.since) < WINDOW);
        if repeats.lines.len() >= MOST_LINES {
            repeats.lines.clear();
        }
    }
    let seen = repeats.lines.entry(what.to_owned()).or_insert_with(|| Seen { since: now, times: 0, said: what.to_owned() });
    let mut before = None;
    if now.duration_since(seen.since) >= WINDOW {
        before = summary(seen);
        seen.since = now;
        seen.times = 0;
    }
    seen.times += 1;
    (seen.times <= SHOWN, before)
}

/// "… repeated N more times": for a line shown SHOWN times in its minute
/// and then held back.
fn summary(seen: &Seen) -> Option<String> {
    let held = seen.times.checked_sub(SHOWN).filter(|held| *held > 0)?;
    let line = seen.said.trim_end();
    let line = line.char_indices().nth(300).map_or(line, |(at, _)| &line[..at]);
    let times = if held == 1 { "once more".to_owned() } else { format!("{held} more times") };
    Some(format!("{} … repeated {times} in a minute, not shown: {line}\n", stamp()))
}

/// Says, for every line held back in a minute that's over, how often.
fn repeats_said() {
    let now = Instant::now();
    let said: Vec<String> = {
        let mut repeats = crate::locking::lock(&REPEATS);
        let Some(repeats) = repeats.as_mut() else { return };
        let over: Vec<String> = repeats.lines.iter().filter(|(_, seen)| now.duration_since(seen.since) >= WINDOW).map(|(line, _)| line.clone()).collect();
        over.into_iter().filter_map(|line| repeats.lines.remove(&line)).filter_map(|seen| summary(&seen)).collect()
    };
    let mut out = std::io::stdout();
    for line in said {
        let _ = out.write_all(line.as_bytes());
    }
}

fn stamp() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let seconds = now.as_secs();
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:06}Z  INFO factoryd::log_file:",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60,
        now.subsec_micros()
    )
}

/// The calendar date `days` after 1970-01-01.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// The log's writer: standard output, with a line said over and over held
/// back past [`SHOWN`] a minute.
pub struct Quieted;

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Quieted {
    type Writer = QuietedWriter;

    fn make_writer(&'a self) -> Self::Writer {
        QuietedWriter
    }
}

pub struct QuietedWriter;

impl Write for QuietedWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let line = String::from_utf8_lossy(buffer);
        let (show, before) = heard(&line, Instant::now());
        let mut out = std::io::stdout().lock();
        if let Some(before) = before {
            out.write_all(before.as_bytes())?;
        }
        if show {
            out.write_all(buffer)?;
        }
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stdout().flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_said_over_and_over_is_shown_five_times_a_minute_then_counted() {
        let start = Instant::now();
        let line = |n: u32| format!("2026-11-26T02:00:{n:02}.000000Z  WARN factoryd::netwatch: the network changed\n");
        let shown: Vec<bool> = (0..20).map(|n| heard(&line(n), start + Duration::from_millis(u64::from(n))).0).collect();
        assert_eq!(shown.iter().filter(|shown| **shown).count(), 5);
        assert!(shown[..5].iter().all(|shown| *shown));
        // Another line isn't held back by it.
        assert!(heard("2026-11-26T02:00:30.000000Z  INFO factoryd: something else\n", start).0);
        // A minute later: shown again, after the count of what wasn't.
        let (show, before) = heard(&line(0), start + WINDOW + Duration::from_secs(1));
        assert!(show);
        let before = before.expect("the count");
        assert!(before.contains("repeated 15 more times in a minute, not shown: WARN factoryd::netwatch: the network changed"), "{before}");
        assert!(before.starts_with("20") && before.ends_with('\n'));
    }

    #[test]
    fn dates_read_as_the_calendar_has_them() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_418), (2025, 11, 26));
        assert_eq!(civil(19_782), (2024, 2, 29));
    }

    #[test]
    fn the_log_is_rotated_only_when_it_is_the_output_and_appended_to() {
        let dir = std::env::temp_dir().join(format!("factoryd-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("factoryd.log");
        let appended = std::fs::OpenOptions::new().create(true).append(true).open(&log).unwrap();
        (&appended).write_all(&[b'x'; 2000]).unwrap();
        // Small: kept.
        assert!(!rotate(&log, &appended, 4000).unwrap());
        // Big: copied to .1 and emptied; the writer goes on at its start.
        assert!(rotate(&log, &appended, 1000).unwrap());
        assert_eq!(std::fs::metadata(dir.join("factoryd.log.1")).unwrap().len(), 2000);
        assert_eq!(std::fs::metadata(&log).unwrap().len(), 0);
        (&appended).write_all(b"after\n").unwrap();
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "after\n");
        // Again: the old one moves up, two kept.
        (&appended).write_all(&[b'y'; 2000]).unwrap();
        assert!(rotate(&log, &appended, 1000).unwrap());
        (&appended).write_all(&[b'z'; 2000]).unwrap();
        assert!(rotate(&log, &appended, 1000).unwrap());
        assert!(std::fs::read_to_string(dir.join("factoryd.log.1")).unwrap().starts_with('z'));
        assert!(std::fs::read_to_string(dir.join("factoryd.log.2")).unwrap().starts_with("after\ny"));
        assert!(!dir.join("factoryd.log.3").exists());
        // Written at its own place, or another file: left alone.
        let own = std::fs::OpenOptions::new().write(true).open(&log).unwrap();
        (&appended).write_all(&[b'w'; 2000]).unwrap();
        assert!(!rotate(&log, &own, 1000).unwrap());
        let other = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("other.log")).unwrap();
        assert!(!rotate(&log, &other, 1000).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
