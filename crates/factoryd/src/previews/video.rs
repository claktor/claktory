//! Phone-sized previews of videos (#379 step 2): H.264 at about 800 kbit/s,
//! at most 720p and 30 frames a second, with its index first so it plays
//! while it loads. On a Mac, `claktory-video-preview` (app/macos/VideoPreview,
//! AVAssetWriter) makes it, from beside factoryd; elsewhere ffmpeg, when
//! it's on the PATH; without either, the original comes, marked
//! FILE_PREVIEW_UNAVAILABLE. A video whose bitrate is already about a
//! preview's comes as it is.
//!
//! Kept as images' are (`super::FOLDER`): the header, then the MP4, read a
//! range at a time.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::Duration;

use factory_protocol::v1;

use super::{Header, Screen};
use crate::extensions::viewer;

/// A preview's bitrate, video and audio together, bits a second.
const PREVIEW_BITRATE: f64 = 864_000.0;
/// A video at most this many times a preview's bitrate comes as it is.
const SKIP_BELOW: f64 = 1.2;
/// The most pixels a preview has either way, whatever the screen.
const SHORT_MOST: u32 = 720;
const LONG_MOST: u32 = 1280;
/// A range of a preview, as viewer.rs reads videos.
const RANGE: u64 = 8 * 1024 * 1024;
/// How long making one may take, at most.
const MAKING_MOST: Duration = Duration::from_secs(15 * 60);
/// `show` makes previews ahead only of videos this long at most.
const AHEAD_MOST: f64 = 10.0 * 60.0;
/// The most memory ffmpeg may map, on Linux.
#[cfg(target_os = "linux")]
const FFMPEG_MEMORY: libc::rlim_t = 2 << 30;
/// ffmpeg's threads for a person's preview: measured on a 40 s 720p clip,
/// 6.3 s with one, 3.0 s with 4, on a busy Mac.
const PERSON_THREADS: &str = "4";
/// The helper, beside factoryd.
const HELPER: &str = crate::update::VIDEO_PREVIEW;

/// Who a preview is made for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Asker {
    /// A person opening the video, who waits for it.
    Person,
    /// `show`, ahead of anyone opening it.
    Ahead,
}

/// Previews being made, apart from images'. A person's never waits behind
/// one made ahead (Bruno 28190): one made ahead runs only while no
/// person's is waiting or being made, and a person's stops one already
/// running ahead, which is made again once the person's is done. Running
/// beside it, even at a lower priority, slowed a person's several times
/// over, since both share the Mac's video encoder (Sale, 28592). One of each
/// at a time, and never the same preview twice at once: who asks for one
/// being made waits for it.
struct Making {
    /// Being made: the video's version and the screen.
    making: Vec<(String, Screen)>,
    person: bool,
    ahead: bool,
    /// People waiting for their turn.
    waiting: usize,
    /// The one being made ahead is to stop, for a person's.
    stop_ahead: bool,
}

static MAKING: Mutex<Making> = Mutex::new(Making { making: Vec::new(), person: false, ahead: false, waiting: 0, stop_ahead: false });
/// What `make` says when one made ahead stopped for a person's.
const STOPPED: &str = "stopped for one someone is waiting for";
/// How many times one made ahead is started again after it stopped.
const RESTARTS: usize = 5;
/// Wakes those waiting for `MAKING` to change.
static TURNS: Condvar = Condvar::new();

fn making() -> MutexGuard<'static, Making> {
    MAKING.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The turn to make one preview, given back when it's dropped.
struct Turn {
    key: (String, Screen),
    asker: Asker,
}

impl Turn {
    /// Waits for the turn to make `key`'s preview for `asker`; None when
    /// someone else made it meanwhile, kept in `file`.
    fn take(key: (String, Screen), asker: Asker, file: &Path) -> Option<Turn> {
        let mut state = making();
        if asker == Asker::Person {
            state.waiting += 1;
        }
        let turn = loop {
            let same = state.making.contains(&key);
            let free = match asker {
                Asker::Person => !state.person,
                Asker::Ahead => !state.ahead && !state.person && state.waiting == 0,
            };
            if !same && free {
                break Some(Turn { key: key.clone(), asker });
            }
            state = TURNS.wait(state).unwrap_or_else(std::sync::PoisonError::into_inner);
            if !state.making.contains(&key) && super::header_of(file).is_some() {
                break None;
            }
        };
        if asker == Asker::Person {
            state.waiting -= 1;
        }
        if let Some(turn) = &turn {
            state.making.push(turn.key.clone());
            match asker {
                Asker::Person => {
                    state.person = true;
                    // Never the same preview: that one is waited for.
                    state.stop_ahead = state.ahead;
                }
                Asker::Ahead => state.ahead = true,
            }
        }
        drop(state);
        // A person who stopped waiting may let one made ahead start.
        TURNS.notify_all();
        turn
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let mut state = making();
        state.making.retain(|key| *key != self.key);
        match self.asker {
            Asker::Person => state.person = false,
            Asker::Ahead => {
                state.ahead = false;
                state.stop_ahead = false;
            }
        }
        drop(state);
        TURNS.notify_all();
    }
}

pub(super) fn is_video(mime: &str) -> bool {
    matches!(mime, "video/mp4" | "video/quicktime")
}

/// What makes previews here.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Maker {
    /// `claktory-video-preview` (a Mac).
    Helper(PathBuf),
    /// ffmpeg, found on the PATH.
    Ffmpeg(PathBuf),
}

/// `claktory-video-preview`: FACTORY_VIDEO_PREVIEW (set but empty, none),
/// or beside factoryd. It renders PDFs' pages too (`super::pdf`).
pub(super) fn helper() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("FACTORY_VIDEO_PREVIEW") {
        return (!path.is_empty()).then(|| PathBuf::from(path));
    }
    crate::update::beside_backend(HELPER)
}

fn maker() -> Option<Maker> {
    if std::env::var_os("FACTORY_VIDEO_PREVIEW").is_some() {
        return helper().map(Maker::Helper);
    }
    if let Some(helper) = helper() {
        return Some(Maker::Helper(helper));
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|folder| folder.join("ffmpeg"))
        .find(|path| path.is_file())
        .map(Maker::Ffmpeg)
}

/// ReadFile of a video with a screen: its preview, made now if it isn't
/// kept; the original as it is when its bitrate is low already; the original
/// marked FILE_PREVIEW_UNAVAILABLE when nothing here can make one.
pub(super) fn read(data: &Path, request: &v1::ReadFile, start: &v1::FileContent, screen: Screen) -> v1::FileContent {
    read_with(data, request, start, screen, maker)
}

/// `read`, with `maker` saying what makes previews here.
pub(super) fn read_with(
    data: &Path,
    request: &v1::ReadFile,
    start: &v1::FileContent,
    screen: Screen,
    maker: impl FnOnce() -> Option<Maker>,
) -> v1::FileContent {
    let path = PathBuf::from(&start.path);
    let sha = start.sha256.clone();
    let original = || viewer::read_requested(request);
    let unavailable = || {
        let mut content = original();
        if content.error.is_empty() {
            content.preview = v1::FilePreview::Unavailable.into();
        }
        content
    };
    if sha.is_empty() || !worth_it(&path, start.size) {
        return original();
    }
    let file = super::kept_file(data, &sha, screen);
    let header = match super::header_of(&file) {
        Some(header) => header,
        None => {
            let Some(maker) = maker() else { return unavailable() };
            match make(data, &path, &sha, start.size, screen, &maker, Asker::Person) {
                Ok(header) => header,
                Err(why) => {
                    tracing::info!("no preview of {}: {why}", path.display());
                    return unavailable();
                }
            }
        }
    };
    if header.width == 0 {
        return original();
    }
    // The original's picture as it shows; read again for previews kept
    // before it was kept with them.
    let (original_width, original_height) = match (header.original_width, header.original_height) {
        (0, _) | (_, 0) => picture_size(&path).unwrap_or_default(),
        known => known,
    };
    super::touch(&file);
    let take = if request.max_bytes == 0 { RANGE } else { request.max_bytes.min(RANGE) };
    match super::range(&file, request.offset, take) {
        Some((data, size)) => {
            let offset = request.offset.min(size);
            v1::FileContent {
                path: start.path.clone(),
                mime: "video/mp4".to_owned(),
                size,
                truncated: offset > 0 || offset + (data.len() as u64) < size,
                data,
                modified_ms: start.modified_ms,
                sha256: sha,
                offset,
                preview: v1::FilePreview::Made.into(),
                pixel_width: header.width,
                pixel_height: header.height,
                original_size: start.size,
                original_pixel_width: original_width,
                original_pixel_height: original_height,
                ..v1::FileContent::default()
            }
        }
        None => original(),
    }
}

/// An agent showed a video: its previews for `screens`, made now, when it's
/// worth one and not too long.
pub(super) fn ahead(data: &Path, path: &Path, sha: &str, size: u64, screens: &[Screen]) {
    ahead_with(data, path, sha, size, screens, maker);
}

/// `ahead`, with `maker` saying what makes previews here.
fn ahead_with(data: &Path, path: &Path, sha: &str, size: u64, screens: &[Screen], maker: impl FnOnce() -> Option<Maker>) {
    if !worth_it(path, size) || seconds(path).is_none_or(|seconds| seconds > AHEAD_MOST) {
        return;
    }
    let Some(maker) = maker() else { return };
    for screen in screens {
        for _ in 0..=RESTARTS {
            if super::header_of(&super::kept_file(data, sha, *screen)).is_some() {
                break;
            }
            match make(data, path, sha, size, *screen, &maker, Asker::Ahead) {
                Err(why) if why == STOPPED => continue,
                Err(why) => tracing::info!("no preview of {}: {why}", path.display()),
                Ok(_) => {}
            }
            break;
        }
    }
}

/// Whether a preview would be much smaller: the original's bitrate is over
/// `SKIP_BELOW` times a preview's (or its length can't be told).
fn worth_it(path: &Path, size: u64) -> bool {
    match seconds(path) {
        Some(seconds) if seconds > 0.0 => size as f64 * 8.0 / seconds > PREVIEW_BITRATE * SKIP_BELOW,
        _ => true,
    }
}

/// Makes and keeps the preview of the video at `path` (version `sha`) for
/// `screen`, for `asker`; its header. One that isn't smaller than the
/// original is kept as none needed.
fn make(data: &Path, path: &Path, sha: &str, size: u64, screen: Screen, maker: &Maker, asker: Asker) -> Result<Header, String> {
    let file = super::kept_file(data, sha, screen);
    let Some(_turn) = Turn::take((sha.to_owned(), screen), asker, &file) else {
        return super::header_of(&file).ok_or_else(|| "made meanwhile, then gone".to_owned());
    };
    if let Some(header) = super::header_of(&file) {
        return Ok(header);
    }
    let folder = data.join(super::FOLDER);
    std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
    let job = super::Job::new(&folder).map_err(|error| error.to_string())?;
    let made = job.file("preview.mp4");
    let (short, long) = (screen.short.min(SHORT_MOST), screen.long.min(LONG_MOST));
    let mut command = match maker {
        Maker::Helper(helper) => {
            let mut command = Command::new(helper);
            command.arg(path).arg(&made).arg(short.to_string()).arg(long.to_string());
            command
        }
        Maker::Ffmpeg(ffmpeg) => {
            let mut command = Command::new(ffmpeg);
            // Fits the box the way round the picture is, never larger.
            let scale = format!(
                "scale=w='min(iw,if(lte(iw,ih),{short},{long}))':h='min(ih,if(lte(iw,ih),{long},{short}))':force_original_aspect_ratio=decrease:force_divisible_by=2"
            );
            // One thread made ahead; a few for a person waiting for it,
            // which makes it several times sooner (Sale, 28789).
            let threads = if asker == Asker::Person { PERSON_THREADS } else { "1" };
            // Only the file it's given, read as what it was sniffed as
            // (Fugo, 24362): no other file or URL a movie could point at.
            command
                .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-protocol_whitelist", "file", "-enable_drefs", "0"])
                .args(["-threads", threads, "-f", "mov", "-i"])
                .arg(path)
                .args([
                    "-threads",
                    threads,
                    "-vf",
                    &scale,
                    "-fpsmax",
                    "30",
                    "-c:v",
                    "libx264",
                    "-preset",
                    "veryfast",
                    "-b:v",
                    "800k",
                    "-maxrate",
                    "900k",
                    "-bufsize",
                    "1600k",
                    "-pix_fmt",
                    "yuv420p",
                    "-c:a",
                    "aac",
                    "-b:a",
                    "64k",
                    "-ac",
                    "2",
                    "-movflags",
                    "+faststart",
                    "-f",
                    "mp4",
                ]);
            command.arg(&made);
            #[cfg(target_os = "linux")]
            {
                use std::os::unix::process::CommandExt;
                // SAFETY: only setrlimit, async-signal-safe, between fork
                // and exec.
                unsafe {
                    command.pre_exec(|| {
                        let most = libc::rlimit { rlim_cur: FFMPEG_MEMORY, rlim_max: FFMPEG_MEMORY };
                        if libc::setrlimit(libc::RLIMIT_AS, &most) != 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                        Ok(())
                    });
                }
            }
            command
        }
    };
    #[cfg(unix)]
    if asker == Asker::Ahead {
        use std::os::unix::process::CommandExt;
        // SAFETY: only setpriority, a system call, between fork and exec.
        unsafe {
            command.pre_exec(|| {
                // Below a person's, whose preview may be made beside it.
                libc::setpriority(libc::PRIO_PROCESS, 0, 10);
                Ok(())
            });
        }
    }
    // Below a person's on Windows too: BELOW_NORMAL_PRIORITY_CLASS.
    #[cfg(windows)]
    if asker == Asker::Ahead {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0000_4000);
    }
    let stop = || asker == Asker::Ahead && making().stop_ahead;
    let ran = super::run_unless(&mut command, MAKING_MOST, &stop);
    if ran.is_err() && stop() {
        return Err(STOPPED.to_owned());
    }
    let made_it = ran.and_then(|ran| {
        if !ran.status.success() {
            return Err(format!("{}: {}", ran.status, ran.said.trim()));
        }
        let (width, height) = picture_size(&made).ok_or("the preview it made has no picture")?;
        let (original_width, original_height) = picture_size(path).unwrap_or_default();
        Ok(Header { width, height, original_width, original_height, pages: 0 })
    });
    let header = made_it?;
    // Kept only as the version asked about, and only when smaller.
    // A video that changed meanwhile comes as it is now.
    let unchanged = viewer::whole_sha256(path).is_ok_and(|now| now == sha);
    let smaller = std::fs::metadata(&made).is_ok_and(|made| made.len() < size);
    let header = if smaller && unchanged { header } else { Header::none() };
    let kept = if unchanged { super::keep_file(&folder, &file, &header, smaller.then_some(made.as_path())) } else { Ok(()) };
    drop(job);
    kept.map_err(|error| error.to_string())?;
    Ok(header)
}

// ---- MP4 and QuickTime: just enough to read a length and a size ----

/// A box's type, and where its contents are in the file.
struct Atom {
    kind: [u8; 4],
    start: u64,
    end: u64,
}

/// The boxes from `start` to `end` of `file`.
fn atoms(file: &mut std::fs::File, start: u64, end: u64) -> Vec<Atom> {
    let mut found = Vec::new();
    let mut at = start;
    while at + 8 <= end && found.len() < 1000 {
        let mut head = [0; 16];
        if file.seek(SeekFrom::Start(at)).is_err() || file.read_exact(&mut head[..8]).is_err() {
            break;
        }
        let size = u64::from(u32::from_be_bytes(head[..4].try_into().expect("four bytes")));
        let kind: [u8; 4] = head[4..8].try_into().expect("four bytes");
        let (size, header) = match size {
            0 => (end - at, 8),
            1 => {
                if file.read_exact(&mut head[8..16]).is_err() {
                    break;
                }
                (u64::from_be_bytes(head[8..16].try_into().expect("eight bytes")), 16)
            }
            size => (size, 8),
        };
        if size < header || at.saturating_add(size) > end {
            break;
        }
        found.push(Atom { kind, start: at + header, end: at + size });
        at += size;
    }
    found
}

fn child(file: &mut std::fs::File, within: &Atom, kind: &[u8; 4]) -> Option<Atom> {
    atoms(file, within.start, within.end).into_iter().find(|atom| &atom.kind == kind)
}

fn contents(file: &mut std::fs::File, atom: &Atom, most: u64) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    file.seek(SeekFrom::Start(atom.start)).ok()?;
    file.take((atom.end - atom.start).min(most)).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

fn movie(file: &mut std::fs::File) -> Option<Atom> {
    let end = file.metadata().ok()?.len();
    atoms(file, 0, end).into_iter().find(|atom| &atom.kind == b"moov")
}

/// How long a video is, in seconds, from its movie header.
fn seconds(path: &Path) -> Option<f64> {
    let mut file = std::fs::File::open(path).ok()?;
    let movie = movie(&mut file)?;
    let mvhd = child(&mut file, &movie, b"mvhd")?;
    let header = contents(&mut file, &mvhd, 32)?;
    let number = |at: usize, wide: bool| -> Option<u64> {
        Some(if wide {
            u64::from_be_bytes(header.get(at..at + 8)?.try_into().ok()?)
        } else {
            u64::from(u32::from_be_bytes(header.get(at..at + 4)?.try_into().ok()?))
        })
    };
    let (scale, length) =
        if *header.first()? == 1 { (number(20, false)?, number(24, true)?) } else { (number(12, false)?, number(16, false)?) };
    (scale > 0).then(|| length as f64 / scale as f64)
}

/// The size a video's picture shows at, from its first track with one:
/// turned as its track's matrix turns it (a phone's portrait video is
/// stored 1920 x 1080, turned a quarter).
fn picture_size(path: &Path) -> Option<(u32, u32)> {
    let mut file = std::fs::File::open(path).ok()?;
    let movie = movie(&mut file)?;
    for track in atoms(&mut file, movie.start, movie.end).into_iter().filter(|atom| &atom.kind == b"trak") {
        let Some(header) = child(&mut file, &track, b"tkhd").and_then(|atom| contents(&mut file, &atom, 96)) else { continue };
        let at = if header.first() == Some(&1) { 88 } else { 76 };
        let word = |at: usize| Some(u32::from_be_bytes(header.get(at..at + 4)?.try_into().ok()?));
        let fixed = |at: usize| word(at).map(|word| word >> 16);
        if let (Some(width), Some(height)) = (fixed(at), fixed(at + 4))
            && width > 0
            && height > 0
        {
            // The matrix's a, b, c and d, before the size: a quarter turn
            // has a and d 0.
            let matrix = at - 36;
            let turned = matches!(
                (word(matrix), word(matrix + 4), word(matrix + 12), word(matrix + 16)),
                (Some(0), Some(b), Some(c), Some(0)) if b != 0 && c != 0
            );
            return Some(if turned { (height, width) } else { (width, height) });
        }
    }
    None
}

// Its helpers and ffmpeg are stood in for by shell scripts.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    const PHONE: Screen = Screen { short: 1320, long: 2868 };

    /// An MP4 `seconds` long whose media takes `media` bytes, its picture
    /// `width` x `height`.
    fn mp4(seconds: u32, media: usize, width: u32, height: u32) -> Vec<u8> {
        let mut mvhd = vec![0; 100];
        mvhd[12..16].copy_from_slice(&1000u32.to_be_bytes());
        mvhd[16..20].copy_from_slice(&(seconds * 1000).to_be_bytes());
        let mut tkhd = vec![0; 84];
        tkhd[76..80].copy_from_slice(&(width << 16).to_be_bytes());
        tkhd[80..84].copy_from_slice(&(height << 16).to_be_bytes());
        let moov = atom(b"moov", &[atom(b"mvhd", &mvhd), atom(b"trak", &atom(b"tkhd", &tkhd))].concat());
        [atom(b"ftyp", b"isom\0\0\0\0"), moov, atom(b"mdat", &vec![9; media])].concat()
    }

    /// A stand-in for `claktory-video-preview` that copies `made` to its
    /// output and counts its runs, or fails.
    fn helper(dir: &Path, made: Option<&[u8]>) -> Maker {
        let script = dir.join(if made.is_some() { "helper" } else { "failing-helper" });
        let body = match made {
            Some(made) => {
                std::fs::write(dir.join("made.mp4"), made).unwrap();
                format!("#!/bin/sh\necho run >> '{0}/runs'\ncp '{0}/made.mp4' \"$2\"\n", dir.display())
            }
            None => "#!/bin/sh\necho 'no codec for this' >&2\nexit 1\n".to_owned(),
        };
        std::fs::write(&script, body).unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        Maker::Helper(script)
    }

    fn ask(path: &Path, offset: u64, max_bytes: u64) -> (v1::ReadFile, v1::FileContent) {
        let request = v1::ReadFile {
            path: path.display().to_string(),
            offset,
            max_bytes,
            preview_width: PHONE.short,
            preview_height: PHONE.long,
            ..v1::ReadFile::default()
        };
        let start = viewer::read_requested(&v1::ReadFile { path: request.path.clone(), max_bytes: 512, ..v1::ReadFile::default() });
        (request, start)
    }

    #[test]
    fn a_video_gets_a_preview_made_once_and_read_a_range_at_a_time() {
        let dir = std::env::temp_dir().join(format!("claktory-video-previews-{}", std::process::id()));
        let data = dir.join("data");
        std::fs::create_dir_all(&data).unwrap();
        // 10 s in 3 MB: 2.4 Mbit/s, worth a preview.
        let video = dir.join("render.mp4");
        let original = mp4(10, 3_000_000, 1080, 1920);
        std::fs::write(&video, &original).unwrap();
        let preview = mp4(10, 900_000, 720, 1280);
        let made = helper(&dir, Some(&preview));

        let (request, start) = ask(&video, 0, 0);
        let content = read_with(&data, &request, &start, PHONE, || Some(made.clone()));
        assert_eq!(content.error, "");
        assert_eq!(content.preview(), v1::FilePreview::Made);
        assert_eq!((content.mime.as_str(), content.size, content.data == preview), ("video/mp4", preview.len() as u64, true));
        assert_eq!((content.pixel_width, content.pixel_height, content.original_size), (720, 1280, original.len() as u64));
        assert_eq!((content.original_pixel_width, content.original_pixel_height), (1080, 1920), "the original's picture");
        assert_eq!(content.sha256, start.sha256, "the original's version");
        // A range of it, from the kept one: the helper ran once.
        let (request, start) = ask(&video, 1000, 500);
        let piece = read_with(&data, &request, &start, PHONE, || panic!("made again"));
        assert_eq!((piece.offset, piece.truncated, piece.data.as_slice()), (1000, true, &preview[1000..1500]));
        assert_eq!((piece.original_pixel_width, piece.original_pixel_height), (1080, 1920));
        // A preview kept before the original's size was kept with it: read
        // from the original.
        let kept = crate::previews::kept_file(&data, &start.sha256, PHONE);
        let old = [Header { width: 720, height: 1280, original_width: 0, original_height: 0, pages: 0 }.bytes(), preview.clone()].concat();
        std::fs::write(&kept, old).unwrap();
        let piece = read_with(&data, &request, &start, PHONE, || panic!("made again"));
        assert_eq!((piece.preview(), piece.original_pixel_width, piece.original_pixel_height), (v1::FilePreview::Made, 1080, 1920));
        assert_eq!(std::fs::read_to_string(dir.join("runs")).unwrap(), "run\n");

        // Already about a preview's bitrate (10 s in 1 MB): the original.
        let small = dir.join("screen.mp4");
        std::fs::write(&small, mp4(10, 1_000_000, 1206, 2622)).unwrap();
        let (request, start) = ask(&small, 0, 0);
        let content = read_with(&data, &request, &start, PHONE, || panic!("not worth one"));
        assert_eq!((content.preview(), content.mime.as_str(), content.size), (v1::FilePreview::None, "video/mp4", start.size));

        // Nothing here makes them, or it fails: the original, saying so.
        let other = dir.join("footage.mp4");
        std::fs::write(&other, mp4(10, 4_000_000, 1080, 1920)).unwrap();
        let (request, start) = ask(&other, 0, 0);
        let content = read_with(&data, &request, &start, PHONE, || None);
        assert_eq!((content.preview(), content.size, content.error.as_str()), (v1::FilePreview::Unavailable, start.size, ""));
        let failing = helper(&dir, None);
        let content = read_with(&data, &request, &start, PHONE, || Some(failing.clone()));
        assert_eq!(content.preview(), v1::FilePreview::Unavailable);

        // A preview no smaller than the original is kept as none needed.
        let bigger = helper(&dir, Some(&mp4(10, 5_000_000, 720, 1280)));
        let content = read_with(&data, &request, &start, PHONE, || Some(bigger.clone()));
        assert_eq!((content.preview(), content.size), (v1::FilePreview::None, start.size));
        let again = read_with(&data, &request, &start, PHONE, || panic!("kept as none needed"));
        assert_eq!(again.preview(), v1::FilePreview::None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Bruno 28190: a person opening a video never waits behind a preview
    /// made ahead for another video or another screen, nor shares the
    /// computer with it (Sale, 28592), and one asking for the very preview
    /// being made ahead gets it once it's done, made once.
    #[test]
    fn a_persons_preview_goes_before_those_made_ahead() {
        let dir = std::env::temp_dir().join(format!("claktory-video-turns-{}", std::process::id()));
        let data = dir.join("data");
        std::fs::create_dir_all(&data).unwrap();
        let preview = mp4(10, 900_000, 720, 1280);
        std::fs::write(dir.join("made.mp4"), &preview).unwrap();
        // Says when it starts; a video named slow-*, or one for a screen 360
        // pixels across, waits for a .go file beside the video first.
        let script = dir.join("slow-helper");
        let body = format!(
            "#!/bin/sh\necho \"$(basename \"$1\") $3\" >> '{0}/started'\ncase \"$(basename \"$1\")-$3\" in slow-*|*-360) while [ ! -f \"$1.go\" ]; do sleep 0.02; done;; esac\ncp '{0}/made.mp4' \"$2\"\n",
            dir.display()
        );
        std::fs::write(&script, body).unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let maker = Maker::Helper(script);
        // Each a video of its own: no two alike.
        let made = std::sync::atomic::AtomicUsize::new(0);
        let video = |name: &str| {
            let path = dir.join(name);
            let other = made.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            std::fs::write(&path, mp4(10, 3_000_000 + other, 1080, 1920)).unwrap();
            path
        };
        // Waits for `line` to have started `times` times.
        let started = |line: &str, times: usize| {
            for _ in 0..500 {
                if std::fs::read_to_string(dir.join("started")).is_ok_and(|said| said.lines().filter(|said| *said == line).count() >= times) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("{line} never started {times} times");
        };
        let ahead = |path: &Path, screen: Screen| {
            let (data, path, maker) = (data.clone(), path.to_owned(), maker.clone());
            std::thread::spawn(move || {
                let (_, start) = ask(&path, 0, 0);
                ahead_with(&data, &path, &start.sha256, start.size, &[screen], || Some(maker));
            })
        };
        let open = |path: &Path| {
            let (request, start) = ask(path, 0, 0);
            read_with(&data, &request, &start, PHONE, || Some(maker.clone()))
        };
        let small = Screen { short: 360, long: 640 };

        // Another video being made ahead: it stops for the person's, which
        // has the computer to itself, and starts again once it's done.
        let slow = video("slow-b.mp4");
        let behind = ahead(&slow, PHONE);
        started("slow-b.mp4 720", 1);
        assert_eq!(open(&video("a.mp4")).preview(), v1::FilePreview::Made);
        started("slow-b.mp4 720", 2);
        assert!(!behind.is_finished());
        std::fs::write(slow.with_extension("mp4.go"), "").unwrap();
        behind.join().unwrap();
        let slow_kept = crate::previews::kept_file(&data, &ask(&slow, 0, 0).1.sha256, PHONE);
        assert!(crate::previews::header_of(&slow_kept).is_some(), "made in the end");

        // The same video for another screen: the same.
        let c = video("c.mp4");
        let behind = ahead(&c, small);
        started("c.mp4 360", 1);
        assert_eq!(open(&c).preview(), v1::FilePreview::Made);
        started("c.mp4 360", 2);
        assert!(!behind.is_finished());
        std::fs::write(c.with_extension("mp4.go"), "").unwrap();
        behind.join().unwrap();

        // The very preview being made ahead: waited for, and made once.
        let d = video("slow-d.mp4");
        let behind = ahead(&d, PHONE);
        started("slow-d.mp4 720", 1);
        let opening = {
            let (data, d, maker) = (data.clone(), d.clone(), maker.clone());
            std::thread::spawn(move || {
                let (request, start) = ask(&d, 0, 0);
                read_with(&data, &request, &start, PHONE, || Some(maker)).preview()
            })
        };
        std::thread::sleep(Duration::from_millis(200));
        assert!(!opening.is_finished(), "it waits for the one being made");
        std::fs::write(d.with_extension("mp4.go"), "").unwrap();
        behind.join().unwrap();
        assert_eq!(opening.join().unwrap(), v1::FilePreview::Made);
        let runs = std::fs::read_to_string(dir.join("started")).unwrap();
        assert_eq!(runs.lines().filter(|line| line.starts_with("slow-d.mp4")).count(), 1, "{runs}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Sale, 28789: ffmpeg makes a person's preview with a few threads, and
    /// one made ahead with one.
    #[test]
    fn ffmpeg_gives_a_persons_preview_more_threads() {
        let dir = std::env::temp_dir().join(format!("claktory-video-threads-{}", std::process::id()));
        let data = dir.join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(dir.join("made.mp4"), mp4(10, 900_000, 720, 1280)).unwrap();
        let script = dir.join("ffmpeg");
        let body = format!("#!/bin/sh\necho \"$*\" >> '{0}/args'\nfor last; do :; done\ncp '{0}/made.mp4' \"$last\"\n", dir.display());
        std::fs::write(&script, body).unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let maker = Maker::Ffmpeg(script);
        let (opened, shown) = (dir.join("opened.mp4"), dir.join("shown.mp4"));
        std::fs::write(&opened, mp4(10, 3_000_000, 1080, 1920)).unwrap();
        std::fs::write(&shown, mp4(10, 3_000_001, 1080, 1920)).unwrap();
        let (request, start) = ask(&opened, 0, 0);
        assert_eq!(read_with(&data, &request, &start, PHONE, || Some(maker.clone())).preview(), v1::FilePreview::Made);
        let (_, start) = ask(&shown, 0, 0);
        ahead_with(&data, &shown, &start.sha256, start.size, &[PHONE], || Some(maker));
        let args = std::fs::read_to_string(dir.join("args")).unwrap();
        // The one made ahead runs again when another test's person's preview
        // stopped it (MAKING is the whole process's), so by file, not count.
        let runs = |file: &str| args.lines().filter(|line| line.contains(&format!("{file} -threads"))).collect::<Vec<_>>();
        let (person, ahead) = (runs("opened.mp4"), runs("shown.mp4"));
        assert_eq!(person.len(), 1, "{args}");
        assert!(!ahead.is_empty() && person.len() + ahead.len() == args.lines().count(), "{args}");
        assert!(person[0].contains("-threads 4 -f mov -i") && person[0].contains("opened.mp4 -threads 4 -vf"), "{args}");
        assert!(ahead.iter().all(|line| line.contains("-threads 1 -f mov -i") && line.contains("shown.mp4 -threads 1 -vf")), "{args}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A box: its size, type and contents.
    fn atom(kind: &[u8; 4], contents: &[u8]) -> Vec<u8> {
        [&((contents.len() + 8) as u32).to_be_bytes()[..], kind, contents].concat()
    }

    #[test]
    fn a_videos_length_and_picture_are_read_from_its_boxes() {
        // mvhd, version 0: timescale 600, duration 6030 (10.05 s).
        let mut mvhd = vec![0; 100];
        mvhd[12..16].copy_from_slice(&600u32.to_be_bytes());
        mvhd[16..20].copy_from_slice(&6030u32.to_be_bytes());
        // A sound track's tkhd (no picture), then a video's, 720 x 1280.
        let sound = atom(b"trak", &atom(b"tkhd", &[0; 84]));
        let mut tkhd = vec![0; 84];
        tkhd[76..80].copy_from_slice(&(720u32 << 16).to_be_bytes());
        tkhd[80..84].copy_from_slice(&(1280u32 << 16).to_be_bytes());
        let picture = atom(b"trak", &atom(b"tkhd", &tkhd));
        let moov = atom(b"moov", &[atom(b"mvhd", &mvhd), sound, picture].concat());
        // The index after the media, as a camera writes it.
        let file = [atom(b"ftyp", b"isom\0\0\0\0"), atom(b"mdat", &[7; 5000]), moov].concat();
        let path = std::env::temp_dir().join(format!("claktory-video-boxes-{}.mp4", std::process::id()));
        std::fs::write(&path, &file).unwrap();
        assert_eq!(seconds(&path), Some(10.05));
        assert_eq!(picture_size(&path), Some((720, 1280)));
        // A phone's portrait video: stored 1920 x 1080, its matrix a quarter
        // turn (0 1 / -1 0), shown 1080 x 1920; version 0 and version 1.
        let quarter = |tkhd: &mut Vec<u8>, matrix: usize| {
            tkhd[matrix + 4..matrix + 8].copy_from_slice(&0x0001_0000u32.to_be_bytes());
            tkhd[matrix + 12..matrix + 16].copy_from_slice(&0xffff_0000u32.to_be_bytes());
        };
        let mut v0 = vec![0; 84];
        v0[76..80].copy_from_slice(&(1920u32 << 16).to_be_bytes());
        v0[80..84].copy_from_slice(&(1080u32 << 16).to_be_bytes());
        quarter(&mut v0, 40);
        let mut v1 = vec![0; 96];
        v1[0] = 1;
        v1[88..92].copy_from_slice(&(1920u32 << 16).to_be_bytes());
        v1[92..96].copy_from_slice(&(1080u32 << 16).to_be_bytes());
        quarter(&mut v1, 52);
        for tkhd in [v0, v1] {
            let moov = atom(b"moov", &[atom(b"mvhd", &mvhd), atom(b"trak", &atom(b"tkhd", &tkhd))].concat());
            std::fs::write(&path, [atom(b"ftyp", b"isom\0\0\0\0"), moov].concat()).unwrap();
            assert_eq!(picture_size(&path), Some((1080, 1920)));
        }
        std::fs::write(&path, &file[..file.len() - 10]).unwrap();
        assert_eq!((seconds(&path), picture_size(&path)), (None, None), "cut short");
        std::fs::remove_file(&path).unwrap();
    }
}
