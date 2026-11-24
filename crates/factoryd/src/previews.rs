//! Phone-sized previews of files (#379, Polpo's file-previews.md): an app
//! on a small screen asks ReadFile for a preview sized to it
//! (`preview_width`/`preview_height`), and gets one when it's worth it,
//! never in place of the original without saying so (FileContent.preview).
//!
//! Images (PNG, JPEG, still WebP) are scaled down to fit the screen either
//! way round and encoded as WebP at quality 80 (libwebp), when the original
//! is bigger than the screen or over 200 KB and the preview comes out
//! smaller. Previews are kept in the data folder's `cache/previews`, by the
//! original's SHA-256 and the screen, within a budget, oldest first out; an
//! agent's `show` makes them ahead for the screens that asked lately.
//! Videos: `video`; big PDFs' pages: `pdf`.

mod pdf;
mod video;

use std::io::{Cursor, Read};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use factory_protocol::v1;

use crate::extensions::viewer;

/// Where previews are kept, in the data folder.
const FOLDER: &str = "cache/previews";
/// The most the previews kept take on disk; the oldest go first.
const BUDGET: u64 = 500 << 20;
/// An image over this many bytes gets a preview even when it fits.
const IMAGE_OVER: u64 = 200 * 1024;
/// The biggest image a preview is made of (a plain read takes 20 MB).
const IMAGE_MOST: u64 = 64 << 20;
/// The most pixels either way an image may have to be decoded.
const SIDE_MOST: u32 = 16_384;
/// libwebp's quality: text in a screenshot reads as in the original.
const QUALITY: f32 = 80.0;
/// A screen bigger than this either way isn't a phone's or a tablet's.
const SCREEN_MOST: u32 = 8192;
/// How many recent screens `show` makes previews ahead for.
const AHEAD_SCREENS: usize = 3;
/// What a kept preview starts with: then its width, height, and the
/// original's width and height (u32, little-endian), then its WebP. A width
/// of 0 keeps "none needed", so the original isn't decoded again.
const MAGIC: &[u8; 4] = b"CKP2";
const HEADER: usize = 4 + 5 * 4;

/// One preview is made at a time: they're CPU-heavy, and a second ask for
/// the same one then finds it kept.
static MAKING: Mutex<()> = Mutex::new(());

/// The screen a preview fits: its short and long side, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Screen {
    short: u32,
    long: u32,
}

impl Screen {
    fn of(width: u32, height: u32) -> Option<Screen> {
        let (short, long) = (width.min(height), width.max(height));
        (short > 0 && long <= SCREEN_MOST).then_some(Screen { short, long })
    }

    /// The size `width` x `height` comes to, fitting this screen either way
    /// round, never larger than it is.
    fn fit(self, width: u32, height: u32) -> (u32, u32) {
        let (across, down) = if width <= height { (self.short, self.long) } else { (self.long, self.short) };
        let scale = (f64::from(across) / f64::from(width)).min(f64::from(down) / f64::from(height)).min(1.0);
        let side = |length: u32| ((f64::from(length) * scale).round() as u32).clamp(1, length);
        (side(width), side(height))
    }

    fn name(self) -> String {
        format!("{}x{}", self.short, self.long)
    }
}

/// What a kept preview starts with: its size and the original's (0 when
/// unknown); a width of 0 keeps "none needed".
#[derive(Clone, Copy, Debug, PartialEq)]
struct Header {
    width: u32,
    height: u32,
    original_width: u32,
    original_height: u32,
    /// A PDF page's: how many pages the PDF has.
    pages: u32,
}

impl Header {
    fn none() -> Header {
        Header { width: 0, height: 0, original_width: 0, original_height: 0, pages: 0 }
    }

    fn bytes(&self) -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        for value in [self.width, self.height, self.original_width, self.original_height, self.pages] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    fn parse(bytes: &[u8]) -> Option<Header> {
        if bytes.len() < HEADER || &bytes[..4] != MAGIC {
            return None;
        }
        let value = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"));
        Some(Header { width: value(4), height: value(8), original_width: value(12), original_height: value(16), pages: value(20) })
    }
}

/// A preview as kept: its size, the original's, and its WebP; or none
/// needed (`webp` empty).
#[derive(Debug, PartialEq)]
struct Kept {
    width: u32,
    height: u32,
    original_width: u32,
    original_height: u32,
    webp: Vec<u8>,
}

impl Kept {
    fn none() -> Kept {
        Kept { width: 0, height: 0, original_width: 0, original_height: 0, webp: Vec::new() }
    }

    fn header(&self) -> Header {
        Header {
            width: self.width,
            height: self.height,
            original_width: self.original_width,
            original_height: self.original_height,
            pages: 0,
        }
    }

    fn encode(&self) -> Vec<u8> {
        [self.header().bytes(), self.webp.clone()].concat()
    }

    fn decode(bytes: Vec<u8>) -> Option<Kept> {
        let Header { width, height, original_width, original_height, .. } = Header::parse(&bytes)?;
        let webp = bytes[HEADER..].to_vec();
        (width == 0 || !webp.is_empty()).then_some(Kept { width, height, original_width, original_height, webp })
    }
}

/// ReadFile: with a screen (`preview_width`/`preview_height`), a preview
/// when one is worth it; otherwise `viewer::read_requested` as before.
pub(crate) fn read_requested(data: &Path, request: &v1::ReadFile) -> v1::FileContent {
    let Some(screen) = Screen::of(request.preview_width, request.preview_height) else {
        return viewer::read_requested(request);
    };
    // Its kind, and whether it's still the version asked for.
    let start = viewer::read_requested(&v1::ReadFile {
        path: request.path.clone(),
        max_bytes: 512,
        expected_sha256: request.expected_sha256.clone(),
        ..v1::ReadFile::default()
    });
    if start.error.is_empty() && video::is_video(&start.mime) {
        return video::read(data, request, &start, screen);
    }
    if start.error.is_empty() && pdf::is_pdf(&start.mime) {
        return pdf::read(data, request, &start, screen);
    }
    if !start.error.is_empty() || !is_image(&start.mime) || start.size > IMAGE_MOST {
        return viewer::read_requested(request);
    }
    let path = PathBuf::from(&start.path);
    let Ok(sha) = viewer::whole_sha256(&path) else { return viewer::read_requested(request) };
    let expected = request.expected_sha256.trim();
    if !expected.is_empty() && !sha.eq_ignore_ascii_case(expected) {
        return v1::FileContent { path: start.path, error: "changed".to_owned(), ..v1::FileContent::default() };
    }
    let kept = match image_preview(data, &path, &sha, screen) {
        Some(kept) if !kept.webp.is_empty() => kept,
        _ => return viewer::read_requested(request),
    };
    let size = kept.webp.len() as u64;
    let offset = request.offset.min(size);
    let left = size - offset;
    let take = if request.max_bytes == 0 { left } else { request.max_bytes.min(left) };
    let begin = usize::try_from(offset).unwrap_or(usize::MAX);
    let end = begin + usize::try_from(take).unwrap_or(0);
    v1::FileContent {
        path: start.path,
        mime: "image/webp".to_owned(),
        size,
        data: kept.webp[begin..end].to_vec(),
        truncated: offset > 0 || take < size,
        modified_ms: start.modified_ms,
        sha256: sha,
        offset,
        preview: v1::FilePreview::Made.into(),
        pixel_width: kept.width,
        pixel_height: kept.height,
        original_size: start.size,
        original_pixel_width: kept.original_width,
        original_pixel_height: kept.original_height,
        ..v1::FileContent::default()
    }
}

/// An agent showed `path`: makes its previews for the screens that asked
/// lately, in the background, so a phone opening it needn't wait.
pub(crate) fn ahead(data: &Path, path: &Path) {
    let (data, path) = (data.to_owned(), path.to_owned());
    std::thread::spawn(move || {
        let screens = recent_screens(&data);
        if screens.is_empty() {
            return;
        }
        let start = viewer::read_file(&path.display().to_string(), 0, 512);
        let Ok(sha) = viewer::whole_sha256(&path) else { return };
        if start.error.is_empty() && video::is_video(&start.mime) {
            video::ahead(&data, &path, &sha, start.size, &screens);
            return;
        }
        if start.error.is_empty() && pdf::is_pdf(&start.mime) {
            pdf::ahead(&data, &path, &sha, start.size, &screens);
            return;
        }
        if !start.error.is_empty() || !is_image(&start.mime) || start.size > IMAGE_MOST {
            return;
        }
        for screen in screens {
            image_preview(&data, &path, &sha, screen);
        }
    });
}

/// The images previews are made of.
fn is_image(mime: &str) -> bool {
    matches!(mime, "image/png" | "image/jpeg" | "image/webp")
}

/// The kept preview of the image at `path` (version `sha`) for `screen`,
/// made now if it isn't kept. None when it can't be read or decoded.
fn image_preview(data: &Path, path: &Path, sha: &str, screen: Screen) -> Option<Kept> {
    let folder = data.join(FOLDER);
    let file = kept_file(data, sha, screen);
    if let Some(kept) = std::fs::read(&file).ok().and_then(Kept::decode) {
        touch(&file);
        return Some(kept);
    }
    let _making = MAKING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(kept) = std::fs::read(&file).ok().and_then(Kept::decode) {
        return Some(kept);
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.take(IMAGE_MOST + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > IMAGE_MOST {
        return None;
    }
    // Kept only as the version asked about: the file may have changed since.
    let read_sha = crate::encoding::hex(&<sha2::Sha256 as sha2::Digest>::digest(&bytes));
    let kept = match make(&bytes, screen) {
        Ok(kept) => kept,
        Err(why) => {
            tracing::info!("no preview of {}: {why}", path.display());
            return None;
        }
    };
    if read_sha == sha {
        keep(&folder, &file, &kept);
    }
    Some(kept)
}

/// A preview of an image's bytes for `screen`, or none needed.
fn make(bytes: &[u8], screen: Screen) -> Result<Kept, String> {
    use image::ImageDecoder;
    if animated(bytes) {
        return Ok(Kept::none());
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|error| error.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(SIDE_MOST);
    limits.max_image_height = Some(SIDE_MOST);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|error| error.to_string())?;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut picture = image::DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())?;
    picture.apply_orientation(orientation);
    let (original_width, original_height) = (picture.width(), picture.height());
    let (width, height) = screen.fit(original_width, original_height);
    if (width, height) == (original_width, original_height) && bytes.len() as u64 <= IMAGE_OVER {
        return Ok(Kept::none());
    }
    if (width, height) != (original_width, original_height) {
        picture = picture.resize_exact(width, height, image::imageops::FilterType::Lanczos3);
    }
    let webp = webp(&picture)?;
    if webp.len() >= bytes.len() {
        return Ok(Kept::none());
    }
    Ok(Kept { width, height, original_width, original_height, webp })
}

/// An animated PNG or WebP: kept whole, so it still moves.
fn animated(bytes: &[u8]) -> bool {
    if bytes.starts_with(b"\x89PNG") {
        // An APNG's acTL chunk comes before its first IDAT.
        let before = bytes.windows(4).position(|window| window == b"IDAT").unwrap_or(bytes.len());
        return bytes[..before].windows(4).any(|window| window == b"acTL");
    }
    // A WebP's VP8X header has the animation flag.
    bytes.len() > 20 && &bytes[..4] == b"RIFF" && &bytes[8..16] == b"WEBPVP8X" && bytes[20] & 0x02 != 0
}

/// `picture` as a lossy WebP at `QUALITY`, with its alpha only when it has
/// some.
fn webp(picture: &image::DynamicImage) -> Result<Vec<u8>, String> {
    let (width, height) = (picture.width(), picture.height());
    let opaque = !picture.color().has_alpha() || picture.to_rgba8().pixels().all(|pixel| pixel.0[3] == u8::MAX);
    let (pixels, channels) = if opaque { (picture.to_rgb8().into_raw(), 3) } else { (picture.to_rgba8().into_raw(), 4) };
    let too_big = |_| "it's too big to encode".to_owned();
    let (columns, rows) = (i32::try_from(width).map_err(too_big)?, i32::try_from(height).map_err(too_big)?);
    let stride = columns.checked_mul(channels).ok_or("it's too big to encode")?;
    let mut output: *mut u8 = std::ptr::null_mut();
    // SAFETY: `pixels` holds `rows` rows of `stride` bytes and outlives the
    // call; libwebp writes a buffer of the returned length to `output`,
    // which is freed with WebPFree once copied.
    let length = unsafe {
        if opaque {
            libwebp_sys::WebPEncodeRGB(pixels.as_ptr(), columns, rows, stride, QUALITY, &mut output)
        } else {
            libwebp_sys::WebPEncodeRGBA(pixels.as_ptr(), columns, rows, stride, QUALITY, &mut output)
        }
    };
    if output.is_null() || length == 0 {
        return Err("libwebp couldn't encode it".to_owned());
    }
    // SAFETY: as above; copied before it's freed.
    let encoded = unsafe { std::slice::from_raw_parts(output, length).to_vec() };
    // SAFETY: `output` came from libwebp's encoder and is freed once.
    unsafe { libwebp_sys::WebPFree(output.cast()) };
    Ok(encoded)
}

/// Keeps a preview, written whole before it shows, then holds the folder
/// to its budget.
fn keep(folder: &Path, file: &Path, kept: &Kept) {
    if std::fs::create_dir_all(folder).is_err() {
        return;
    }
    let partial = file.with_extension("partial");
    if std::fs::write(&partial, kept.encode()).and_then(|()| std::fs::rename(&partial, file)).is_err() {
        let _ = std::fs::remove_file(&partial);
        return;
    }
    trim(folder, BUDGET);
}

/// Keeps a preview made in memory, as `keep` does.
fn keep_bytes(folder: &Path, file: &Path, header: &Header, bytes: &[u8]) -> std::io::Result<()> {
    let partial = file.with_extension("partial");
    let written = std::fs::write(&partial, [header.bytes().as_slice(), bytes].concat()).and_then(|()| std::fs::rename(&partial, file));
    if written.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    trim(folder, BUDGET);
    written
}

/// Keeps a preview made into a file of its own (`payload`, or none
/// needed), as `keep` does.
fn keep_file(folder: &Path, file: &Path, header: &Header, payload: Option<&Path>) -> std::io::Result<()> {
    let partial = file.with_extension("partial");
    let written = (|| {
        let mut out = std::fs::File::create(&partial)?;
        std::io::Write::write_all(&mut out, &header.bytes())?;
        if let Some(payload) = payload {
            std::io::copy(&mut std::fs::File::open(payload)?, &mut out)?;
        }
        out.sync_all()?;
        std::fs::rename(&partial, file)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    trim(folder, BUDGET);
    written
}

/// Where the preview of version `sha` for `screen` is kept.
fn kept_file(data: &Path, sha: &str, screen: Screen) -> PathBuf {
    data.join(FOLDER).join(format!("{sha}-{}.preview", screen.name()))
}

/// The header of the preview kept in `file`, if it's kept whole.
fn header_of(file: &Path) -> Option<Header> {
    let mut bytes = [0; HEADER];
    let mut open = std::fs::File::open(file).ok()?;
    open.read_exact(&mut bytes).ok()?;
    let header = Header::parse(&bytes)?;
    let payload = open.metadata().ok()?.len() - HEADER as u64;
    (header.width == 0 || payload > 0).then_some(header)
}

/// Up to `take` bytes of the preview kept in `file` from `offset`, and its
/// whole size.
fn range(file: &Path, offset: u64, take: u64) -> Option<(Vec<u8>, u64)> {
    let mut open = std::fs::File::open(file).ok()?;
    let size = open.metadata().ok()?.len().checked_sub(HEADER as u64)?;
    std::io::Seek::seek(&mut open, std::io::SeekFrom::Start(HEADER as u64 + offset.min(size))).ok()?;
    let mut data = Vec::new();
    open.take(take).read_to_end(&mut data).ok()?;
    Some((data, size))
}

/// A folder of its own for one helper's run, which the helper may write in
/// and nothing else (its sandbox: `claktory-video-preview`'s shutIn), gone
/// with this.
struct Job {
    dir: PathBuf,
}

impl Job {
    fn new(folder: &Path) -> std::io::Result<Job> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let number = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = folder.join(format!(".making-{}-{number}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
        Ok(Job { dir })
    }

    fn file(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// How a helper's run ended: its status, what it printed and what it said
/// went wrong.
struct Ran {
    status: std::process::ExitStatus,
    out: String,
    said: String,
}

/// Runs a helper (`claktory-video-preview`, ffmpeg) to its end, with
/// nothing of factoryd's environment but where programs are, killed after
/// `limit`.
fn run(command: &mut std::process::Command, limit: std::time::Duration) -> Result<Ran, String> {
    use std::process::Stdio;
    command.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("can't run it: {error}"))?;
    let reader = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(pipe) = pipe {
                let _ = pipe.take(4096).read_to_string(&mut text);
            }
            text
        })
    };
    let out = reader(child.stdout.take().map(|pipe| Box::new(pipe) as Box<dyn Read + Send>));
    let said = reader(child.stderr.take().map(|pipe| Box::new(pipe) as Box<dyn Read + Send>));
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("it took too long".to_owned());
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
            Err(error) => return Err(error.to_string()),
        }
    };
    Ok(Ran { status, out: out.join().unwrap_or_default(), said: said.join().unwrap_or_default() })
}

/// A kept preview is used now: the last to go.
fn touch(file: &Path) {
    let _ = std::fs::File::options().write(true).open(file).and_then(|open| open.set_modified(SystemTime::now()));
}

/// Removes the previews used longest ago until the folder holds `budget`
/// bytes at most.
fn trim(folder: &Path, budget: u64) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    let mut kept: Vec<(SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            metadata.is_file().then(|| (metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH), metadata.len(), entry.path()))
        })
        .collect();
    let mut total: u64 = kept.iter().map(|(_, size, _)| size).sum();
    kept.sort();
    for (_, size, path) in kept {
        if total <= budget {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= size;
        }
    }
}

/// The screens previews were made for lately, newest first.
fn recent_screens(data: &Path) -> Vec<Screen> {
    let Ok(entries) = std::fs::read_dir(data.join(FOLDER)) else { return Vec::new() };
    let mut made: Vec<(SystemTime, Screen)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let size = name.strip_suffix(".preview")?.rsplit_once('-')?.1.to_owned();
            let (short, long) = size.split_once('x')?;
            let screen = Screen::of(short.parse().ok()?, long.parse().ok()?)?;
            Some((entry.metadata().ok()?.modified().ok()?, screen))
        })
        .collect();
    made.sort_by_key(|(when, _)| std::cmp::Reverse(*when));
    let mut screens = Vec::new();
    for (_, screen) in made {
        if !screens.contains(&screen) {
            screens.push(screen);
            if screens.len() == AHEAD_SCREENS {
                break;
            }
        }
    }
    screens
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHONE: Screen = Screen { short: 1320, long: 2868 };

    #[test]
    fn a_preview_fits_the_screen_either_way_round_and_never_grows() {
        assert_eq!(PHONE.fit(2064, 2752), (1320, 1760));
        assert_eq!(PHONE.fit(3500, 2705), (1708, 1320), "landscape: its long side across the screen's long side");
        assert_eq!(PHONE.fit(1080, 1920), (1080, 1920), "already fits");
        assert_eq!(PHONE.fit(4000, 10), (2868, 7));
        assert_eq!(PHONE.fit(10, 100_000), (1, 2868), "never 0");
        assert_eq!(Screen::of(2868, 1320), Some(PHONE), "either way round");
        assert_eq!(Screen::of(0, 100), None);
        assert_eq!(Screen::of(100, 9000), None);
    }

    #[test]
    fn a_kept_preview_reads_back_as_it_was_kept() {
        let kept = Kept { width: 1, height: 2, original_width: 3, original_height: 4, webp: b"RIFF".to_vec() };
        assert_eq!(Kept::decode(kept.encode()), Some(kept));
        assert_eq!(Kept::decode(Kept::none().encode()), Some(Kept::none()));
        assert_eq!(Kept::decode(b"CKP1".to_vec()), None);
        assert_eq!(Kept::decode(b"nonsense, not a preview".to_vec()), None);
    }

    #[test]
    fn the_oldest_previews_go_first_to_keep_the_budget() {
        let folder = std::env::temp_dir().join(format!("claktory-previews-trim-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let now = SystemTime::now();
        for (index, name) in ["old", "middle", "new"].iter().enumerate() {
            let path = folder.join(name);
            std::fs::write(&path, [0; 100]).unwrap();
            let when = now - std::time::Duration::from_secs(100 - index as u64 * 10);
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(when).unwrap();
        }
        trim(&folder, 250);
        let left: Vec<_> = ["old", "middle", "new"].iter().filter(|name| folder.join(name).exists()).collect();
        assert_eq!(left, [&"middle", &"new"]);
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn animated_images_are_told_apart() {
        let mut apng = b"\x89PNG\r\n\x1a\n....IHDR........acTL....IDAT".to_vec();
        assert!(animated(&apng));
        apng = b"\x89PNG\r\n\x1a\n....IHDR........IDAT....acTL".to_vec();
        assert!(!animated(&apng), "acTL after IDAT isn't an animation");
        let mut webp = b"RIFF\0\0\0\0WEBPVP8X\0\0\0\0".to_vec();
        webp.extend([0x02, 0, 0, 0]);
        assert!(animated(&webp));
        webp[20] = 0x10;
        assert!(!animated(&webp));
    }

    /// The real `claktory-video-preview`, built from its source for this
    /// test; None (the test says why and checks nothing) without Xcode.
    #[cfg(target_os = "macos")]
    fn real_helper(dir: &Path) -> Option<PathBuf> {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/macos/VideoPreview/main.swift");
        let helper = dir.join("claktory-video-preview");
        let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" };
        let built = std::process::Command::new("xcrun")
            .args(["swiftc", "-target", &format!("{arch}-apple-macos12"), "-O", "-o"])
            .arg(&helper)
            .arg(&source)
            .output();
        match built {
            Ok(built) if built.status.success() => Some(helper),
            Ok(built) => panic!("the helper doesn't build: {}", String::from_utf8_lossy(&built.stderr)),
            Err(error) => {
                eprintln!("no Xcode here ({error}): the helper's sandbox isn't checked");
                None
            }
        }
    }

    /// A video's frame at 1 s in grey, `width` x `height`, as ffmpeg shows
    /// it (turned as tagged).
    #[cfg(target_os = "macos")]
    fn frame(path: &Path, width: u32, height: u32) -> Vec<u8> {
        let out = std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-loglevel", "error", "-ss", "1", "-i"])
            .arg(path)
            .args(["-frames:v", "1", "-vf", &format!("scale={width}:{height}"), "-f", "rawvideo", "-pix_fmt", "gray", "-"])
            .output()
            .unwrap();
        assert_eq!(out.stdout.len(), (width * height) as usize, "{}", String::from_utf8_lossy(&out.stderr));
        out.stdout
    }

    /// A preview (`bytes`, kept at `file`): its frame's mean grey, and how
    /// far its pixels are on average from `like`'s frame as ffmpeg shows it.
    #[cfg(target_os = "macos")]
    fn grey(file: &Path, bytes: &[u8], size: (u32, u32), like: Option<&Path>) -> (f64, f64) {
        std::fs::write(file, bytes).unwrap();
        let shown = frame(file, size.0, size.1);
        let mean = shown.iter().map(|&pixel| f64::from(pixel)).sum::<f64>() / shown.len() as f64;
        let off = like.map_or(0.0, |like| {
            let like = frame(like, size.0, size.1);
            shown.iter().zip(&like).map(|(&a, &b)| f64::from(a.abs_diff(b))).sum::<f64>() / shown.len() as f64
        });
        (mean, off)
    }

    /// A PDF of `pages` Letter pages, padded to over 5 MB.
    #[cfg(target_os = "macos")]
    fn pdf(pages: usize) -> Vec<u8> {
        let mut objects = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_owned()];
        let kids: Vec<String> = (0..pages).map(|page| format!("{} 0 R", 3 + page * 2)).collect();
        objects.push(format!("<< /Type /Pages /Kids [{}] /Count {pages} >>", kids.join(" ")));
        for page in 0..pages {
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> /Contents {} 0 R >>",
                4 + page * 2
            ));
            let content = format!("0 0 1 rg 50 50 512 692 re f BT /F1 64 Tf 100 400 Td (Page {}) Tj ET", page + 1);
            objects.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
        }
        objects.push(format!("<< /Length 6000000 >>\nstream\n{}\nendstream", "0".repeat(6_000_000)));
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (number, object) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{object}\nendobj\n", number + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for offset in offsets {
            out.extend(format!("{offset:010} 00000 n \n").bytes());
        }
        out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).bytes());
        out
    }

    /// Fugo's #379 review: while it parses a file, the real helper reads
    /// only that file and writes only in its own folder: not Claktory's
    /// data, not the home folder, not beside its folder, not the network.
    /// And it still makes PDF pages and videos' previews.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_helper_is_shut_in_and_still_makes_previews() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("claktory-helper-sandbox-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        let Some(helper) = real_helper(&dir) else { return };
        let data = dir.join("data");
        std::fs::create_dir_all(data.join(FOLDER)).unwrap();
        // Claktory's data, as a hostile file would look for it.
        let secret = data.join("config.json");
        std::fs::write(&secret, "{\"secret\": true}").unwrap();
        let document = dir.join("scan.pdf");
        std::fs::write(&document, pdf(3)).unwrap();
        // Files are read through links too: the helper opens the real path,
        // the only one its sandbox lets it read.
        let linked = dir.join("link.pdf");
        std::os::unix::fs::symlink(&document, &linked).unwrap();

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let job = Job::new(&data.join(FOLDER)).unwrap();
        let mut command = std::process::Command::new(&helper);
        command.arg("probe").arg(&linked).arg(job.file("out.bin")).arg(&secret).arg(port.to_string());
        let ran = run(&mut command, std::time::Duration::from_secs(60)).unwrap();
        assert!(ran.status.success(), "{}", ran.said);
        let got: serde_json::Value = serde_json::from_str(ran.out.trim()).unwrap();
        assert_eq!(
            got,
            serde_json::json!({"read_input": true, "write_output": true, "read_secret": false, "list_home": false, "write_beside": false, "connect": false}),
        );
        assert!(listener.accept().is_err(), "nothing connected");
        assert!(
            std::fs::read_dir(data.join(FOLDER))
                .unwrap()
                .flatten()
                .all(|entry| !entry.file_name().to_string_lossy().starts_with("escaped"))
        );
        drop(job);

        // A big PDF's pages, rendered shut in.
        let screen = Screen { short: 1320, long: 2868 };
        let ask = |path: &Path, page: u32| {
            let request = v1::ReadFile {
                path: path.display().to_string(),
                preview_width: screen.short,
                preview_height: screen.long,
                preview_page: page,
                ..v1::ReadFile::default()
            };
            let start = viewer::read_requested(&v1::ReadFile { path: request.path.clone(), max_bytes: 512, ..v1::ReadFile::default() });
            (request, start)
        };
        let (request, start) = ask(&linked, 2);
        let page = pdf::read_with(&data, &request, &start, screen, || Some(helper.clone()));
        assert_eq!((page.error.as_str(), page.preview()), ("", v1::FilePreview::Made));
        assert_eq!((page.pixel_width, page.pixel_height, page.page_count), (1320, 1708, 3));
        let shown = image::load_from_memory_with_format(&page.data, image::ImageFormat::WebP).unwrap().to_rgb8();
        assert!(shown.get_pixel(660, 854).0[2] > 200, "the page's blue, drawn");

        // A video's preview, shut in, when ffmpeg is here to make one to start from.
        let footage = dir.join("footage.mp4");
        let made = std::process::Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=1080x1920:rate=60",
                "-t",
                "3",
                "-b:v",
                "6M",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&footage)
            .status();
        if made.is_ok_and(|status| status.success()) {
            let linked = dir.join("link.mp4");
            std::os::unix::fs::symlink(&footage, &linked).unwrap();
            let (request, start) = ask(&linked, 0);
            let maker = video::Maker::Helper(helper.clone());
            let preview = video::read_with(&data, &request, &start, screen, || Some(maker));
            assert_eq!((preview.error.as_str(), preview.preview()), ("", v1::FilePreview::Made));
            assert_eq!((preview.mime.as_str(), preview.pixel_width, preview.pixel_height), ("video/mp4", 720, 1280));
            assert_eq!((preview.original_pixel_width, preview.original_pixel_height), (1080, 1920));
            assert!(preview.size < start.size);
            let upright = dir.join("upright.mp4");
            assert!(grey(&upright, &preview.data, (720, 1280), None).0 > 100.0, "the picture is in it");

            // A phone's portrait video: stored on its side, 1920 x 1080, and
            // tagged to turn a quarter (ffmpeg's tag, with no move after the
            // turn). Its preview is upright, the picture in it, not black.
            let side = dir.join("side.mp4");
            let turned = dir.join("turned.mp4");
            let ffmpeg = |before: &[&str], from: &Path, after: &[&str], to: &Path| {
                std::process::Command::new("ffmpeg")
                    .args(["-nostdin", "-loglevel", "error", "-y"])
                    .args(before)
                    .arg("-i")
                    .arg(from)
                    .args(after)
                    .arg(to)
                    .status()
                    .is_ok_and(|status| status.success())
            };
            if ffmpeg(&[], &footage, &["-vf", "transpose=2", "-b:v", "6M", "-pix_fmt", "yuv420p"], &side)
                && ffmpeg(&["-display_rotation:v:0", "-90"], &side, &["-c", "copy"], &turned)
            {
                let (request, start) = ask(&turned, 0);
                let maker = video::Maker::Helper(helper.clone());
                let preview = video::read_with(&data, &request, &start, screen, || Some(maker));
                assert_eq!((preview.error.as_str(), preview.preview()), ("", v1::FilePreview::Made));
                assert_eq!((preview.pixel_width, preview.pixel_height), (720, 1280), "upright");
                assert_eq!((preview.original_pixel_width, preview.original_pixel_height), (1080, 1920));
                // As the same picture never turned comes out, and far from
                // it read on its side.
                let (mean, off) = grey(&dir.join("turned-preview.mp4"), &preview.data, (720, 1280), Some(&upright));
                assert!(mean > 100.0, "black: mean grey {mean}");
                assert!(off < 3.0, "not upright: off by {off} a pixel");
                let (_, sideways) = grey(&dir.join("turned-preview.mp4"), &preview.data, (720, 1280), Some(&side));
                assert!(sideways > 10.0, "the check can't tell: {sideways}");
            } else {
                eprintln!("ffmpeg couldn't tag a video turned: that half isn't checked");
            }
        } else {
            eprintln!("no ffmpeg to make a video from: the video half isn't checked");
        }
        // Nothing left of the jobs' folders.
        let left: Vec<_> = std::fs::read_dir(data.join(FOLDER)).unwrap().flatten().map(|entry| entry.file_name()).collect();
        assert!(left.iter().all(|name| name.to_string_lossy().ends_with(".preview")), "{left:?}");
        let _ = std::io::stderr().flush();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
