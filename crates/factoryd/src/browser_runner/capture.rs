//! The browser's screen as the live view sees it (#435 P2): the X screen
//! captured when it changes (XDamage), through shared memory (MIT-SHM)
//! where the server offers it, scaled to the width the viewer asked for and
//! sent as JPEG on C1's frames channel, one frame in flight until the
//! viewer acks it and at most `FPS` a second. While the screen changes
//! fast, frames are a little softer, so they keep up; once it's still, one
//! sharper frame follows.
//!
//! The view is a region from the screen's top left, the browser window's
//! size: Xvfb can't change its screen's size (its RandR offers one mode),
//! so a phone's shape (R8.4) is the browser window resized to it, and the
//! view cropped to match. Start Xvfb with a screen at least as large as
//! any shape asked for.

use std::os::fd::AsRawFd as _;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use x11rb::connection::{Connection as _, RequestConnection as _};
use x11rb::protocol::Event;
use x11rb::protocol::damage::{self, ConnectionExt as _};
use x11rb::protocol::shm::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{self, ConnectionExt as _};
use x11rb::rust_connection::RustConnection;

use super::Outbox;
use super::wire::{Channel, FrameHeader};
use super::xvfb::Display;

/// Frames a second, at most.
const FPS: u32 = 10;
/// An ack that doesn't come in this long frees the next frame anyway: the
/// viewer may have gone without saying.
const ACK_WAIT: Duration = Duration::from_secs(5);
/// Changes closer than this make the screen "changing fast".
const FAST: Duration = Duration::from_millis(300);
/// Still for this long after soft frames: one sharp frame.
const SETTLE: Duration = Duration::from_millis(400);
/// A still frame's quality when the view leaves it to the runner.
const QUALITY: u8 = 70;

/// What the viewer asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct View {
    on: bool,
    max_width: u16,
    /// 1 to 100: the quality of a still frame; moving ones are softer.
    quality: u8,
}

enum Control {
    View(View),
    Ack(u32),
    Resize(u16, u16, mpsc::Sender<Result<(), String>>),
    Stop,
}

/// The capture thread, and how to steer it.
pub struct Capture {
    control: mpsc::Sender<Control>,
    thread: Option<JoinHandle<()>>,
}

impl Capture {
    /// Starts capturing the screen's region of `width` × `height` from its
    /// top left; frames go out on C1's channel 2 while the view is on.
    pub fn start(display: &Display, width: u16, height: u16, out: Outbox) -> Result<Capture, String> {
        let grabber = Grabber::new(display, width, height)?;
        let (control, controls) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("browser-capture".to_owned())
            .spawn(move || run(grabber, &controls, &out))
            .map_err(|error| error.to_string())?;
        Ok(Capture { control, thread: Some(thread) })
    }

    /// Turns the view on or off, at most `max_width` wide, at a quality
    /// from 1 to 100 (0: the runner's own).
    pub fn set_view(&self, on: bool, max_width: u16, quality_hint: u8) {
        let quality = if quality_hint == 0 { QUALITY } else { quality_hint.min(100) };
        let view = View { on, max_width: max_width.max(16), quality };
        let _ = self.control.send(Control::View(view));
    }

    /// The viewer showed frame `seq`: the next may go.
    pub fn ack(&self, seq: u32) {
        let _ = self.control.send(Control::Ack(seq));
    }

    /// Resizes the view, and the browser's windows with it, to `width` ×
    /// `height` (a phone's shape, or back): no larger than the screen.
    pub fn resize(&self, width: u16, height: u16) -> Result<(), String> {
        let (done, answer) = mpsc::channel();
        self.control.send(Control::Resize(width, height, done)).map_err(|_| "the capture stopped".to_owned())?;
        answer.recv_timeout(Duration::from_secs(5)).map_err(|_| "the screen didn't answer".to_owned())?
    }
}

impl Drop for Capture {
    /// Stops the thread, and waits for it.
    fn drop(&mut self) {
        let _ = self.control.send(Control::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// How pixels are read: through a shared memory segment, or GetImage.
enum Pixels {
    Shared { segment: shm::Seg, map: SharedMap },
    Plain,
}

/// A shared memory segment mapped here, unmapped on drop.
struct SharedMap {
    address: *mut u8,
    length: usize,
}

// The map is used only by the capture thread that owns it.
unsafe impl Send for SharedMap {}

impl Drop for SharedMap {
    fn drop(&mut self) {
        // SAFETY: address and length are a mapping this made, unmapped once.
        unsafe { libc::munmap(self.address.cast(), self.length) };
    }
}

/// Reads the screen.
struct Grabber {
    connection: RustConnection,
    root: xproto::Window,
    screen_width: u16,
    screen_height: u16,
    /// The view: a region from the top left.
    width: u16,
    height: u16,
    pixels: Pixels,
}

impl Grabber {
    fn new(display: &Display, width: u16, height: u16) -> Result<Grabber, String> {
        let connection = display.connect()?;
        let setup = connection.setup();
        let root_screen = setup.roots.first().ok_or("the screen has no root window")?;
        let (root, depth) = (root_screen.root, root_screen.root_depth);
        let (screen_width, screen_height) = (root_screen.width_in_pixels, root_screen.height_in_pixels);
        let bits = setup.pixmap_formats.iter().find(|format| format.depth == depth).map(|format| format.bits_per_pixel);
        if depth != 24 || bits != Some(32) {
            return Err(format!("the screen is {depth} bits deep ({bits:?} per pixel); the live view reads 24 in 32"));
        }
        // Changes reported once until taken (NonEmpty), on the whole screen.
        connection.damage_query_version(1, 1).map_err(fail)?.reply().map_err(fail)?;
        let damage = connection.generate_id().map_err(fail)?;
        connection.damage_create(damage, root, damage::ReportLevel::NON_EMPTY).map_err(fail)?;
        let mut grabber = Grabber {
            connection,
            root,
            screen_width,
            screen_height,
            width: width.clamp(1, screen_width),
            height: height.clamp(1, screen_height),
            pixels: Pixels::Plain,
        };
        grabber.pixels = grabber.share().unwrap_or_else(|why| {
            tracing::info!(%why, "the live view reads the screen without shared memory");
            Pixels::Plain
        });
        grabber.connection.flush().map_err(fail)?;
        Ok(grabber)
    }

    /// A shared memory segment the size of the whole screen, made by the
    /// server (MIT-SHM 1.2) and mapped here.
    fn share(&self) -> Result<Pixels, String> {
        if self.connection.extension_information(shm::X11_EXTENSION_NAME).map_err(fail)?.is_none() {
            return Err("no MIT-SHM".to_owned());
        }
        let version = self.connection.shm_query_version().map_err(fail)?.reply().map_err(fail)?;
        if (version.major_version, version.minor_version) < (1, 2) {
            return Err(format!("MIT-SHM {}.{}", version.major_version, version.minor_version));
        }
        let length = usize::from(self.screen_width) * usize::from(self.screen_height) * 4;
        let segment = self.connection.generate_id().map_err(fail)?;
        let size = u32::try_from(length).map_err(fail)?;
        let reply = self.connection.shm_create_segment(segment, size, false).map_err(fail)?.reply().map_err(fail)?;
        let fd = reply.shm_fd;
        // SAFETY: a shared mapping of the server's segment, `length` bytes,
        // read only here; the fd is closed when `reply` drops, the mapping
        // stays until SharedMap drops.
        let address = unsafe { libc::mmap(std::ptr::null_mut(), length, libc::PROT_READ, libc::MAP_SHARED, fd.as_raw_fd(), 0) };
        if address == libc::MAP_FAILED {
            let _ = self.connection.shm_detach(segment);
            return Err(format!("couldn't map the segment: {}", std::io::Error::last_os_error()));
        }
        Ok(Pixels::Shared { segment, map: SharedMap { address: address.cast(), length } })
    }

    /// The view's pixels now, as RGB.
    fn grab(&self) -> Result<image::RgbImage, String> {
        let (width, height) = (self.width, self.height);
        let count = usize::from(width) * usize::from(height);
        let mut rgb = Vec::with_capacity(count * 3);
        let mut push = |bgrx: &[u8]| {
            for pixel in bgrx.chunks_exact(4) {
                rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
            }
        };
        match &self.pixels {
            Pixels::Shared { segment, map } => {
                let format = xproto::ImageFormat::Z_PIXMAP.into();
                self.connection
                    .shm_get_image(self.root, 0, 0, width, height, !0, format, *segment, 0)
                    .map_err(fail)?
                    .reply()
                    .map_err(fail)?;
                // SAFETY: the server wrote width × height pixels of 4 bytes
                // at offset 0, within the segment (the whole screen's size),
                // and has answered, so it's done writing.
                let bytes = unsafe { std::slice::from_raw_parts(map.address, (count * 4).min(map.length)) };
                push(bytes);
            }
            Pixels::Plain => {
                let reply = self
                    .connection
                    .get_image(xproto::ImageFormat::Z_PIXMAP, self.root, 0, 0, width, height, !0)
                    .map_err(fail)?
                    .reply()
                    .map_err(fail)?;
                push(&reply.data);
            }
        }
        image::RgbImage::from_raw(u32::from(width), u32::from(height), rgb).ok_or_else(|| "the screen sent too few pixels".to_owned())
    }

    /// Resizes the view and the browser's top-level windows.
    fn resize(&mut self, width: u16, height: u16) -> Result<(), String> {
        if width == 0 || height == 0 || width > self.screen_width || height > self.screen_height {
            return Err(format!("{width}×{height} doesn't fit the screen ({}×{})", self.screen_width, self.screen_height));
        }
        let tree = self.connection.query_tree(self.root).map_err(fail)?.reply().map_err(fail)?;
        for window in tree.children {
            let viewable = self
                .connection
                .get_window_attributes(window)
                .map_err(fail)?
                .reply()
                .is_ok_and(|attributes| attributes.map_state == xproto::MapState::VIEWABLE && !attributes.override_redirect);
            if viewable {
                let size = xproto::ConfigureWindowAux::new().x(0).y(0).width(u32::from(width)).height(u32::from(height));
                self.connection.configure_window(window, &size).map_err(fail)?;
            }
        }
        self.connection.flush().map_err(fail)?;
        (self.width, self.height) = (width, height);
        Ok(())
    }

    /// Whether the screen changed since the last time, taking its changes.
    fn changed(&self) -> Result<bool, String> {
        let mut changed = false;
        while let Some(event) = self.connection.poll_for_event().map_err(fail)? {
            if let Event::DamageNotify(notify) = event {
                self.connection.damage_subtract(notify.damage, x11rb::NONE, x11rb::NONE).map_err(fail)?;
                changed = true;
            }
        }
        if changed {
            self.connection.flush().map_err(fail)?;
        }
        Ok(changed)
    }
}

fn fail(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// A frame as C1 sends it: the header, then the JPEG.
fn frame(seq: u32, jpeg: &[u8], size: (u16, u16), view: (u16, u16)) -> Vec<u8> {
    let header = FrameHeader { seq, width: size.0, height: size.1, screen_width: view.0, screen_height: view.1 };
    let mut payload = Vec::with_capacity(FrameHeader::LEN + jpeg.len());
    payload.extend_from_slice(&header.encode());
    payload.extend_from_slice(jpeg);
    payload
}

/// The image scaled to at most `max_width`, as JPEG at `quality`, with its
/// size.
fn encode(image: &image::RgbImage, max_width: u16, quality: u8) -> Result<(Vec<u8>, (u16, u16)), String> {
    let (width, height) = image.dimensions();
    let scaled;
    let image = if width > u32::from(max_width) {
        let new_height = (u64::from(height) * u64::from(max_width) / u64::from(width)).max(1);
        scaled = image::imageops::resize(
            image,
            u32::from(max_width),
            u32::try_from(new_height).unwrap_or(1),
            image::imageops::FilterType::Triangle,
        );
        &scaled
    } else {
        image
    };
    let mut jpeg = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, quality);
    encoder.encode_image(image).map_err(fail)?;
    let size = (u16::try_from(image.width()).unwrap_or(u16::MAX), u16::try_from(image.height()).unwrap_or(u16::MAX));
    Ok((jpeg, size))
}

/// The quality of a frame: softer while the screen changes fast.
fn quality(view: View, fast: bool) -> u8 {
    if fast { (u16::from(view.quality) * 7 / 10).max(20).try_into().unwrap_or(20) } else { view.quality }
}

/// The capture loop: frames go out while the view is on, when the screen
/// changed, one in flight, at most `FPS` a second.
fn run(mut grabber: Grabber, controls: &mpsc::Receiver<Control>, out: &Outbox) {
    let gap = Duration::from_secs(1) / FPS;
    let mut view = View { on: false, max_width: 1280, quality: QUALITY };
    let mut seq: u32 = 0;
    let mut in_flight: Option<(u32, Instant)> = None;
    // Something to send: the screen changed, or the view just opened.
    let mut dirty = true;
    let mut last_change = Instant::now();
    let mut last_sent: Option<Instant> = None;
    // The last change came soon after the one before.
    let mut moving = false;
    // The last frame went out soft: once still, a sharp one follows.
    let mut soft = false;
    loop {
        let wait = if view.on { Duration::from_millis(10) } else { Duration::from_millis(200) };
        match controls.recv_timeout(wait) {
            Ok(Control::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Ok(Control::View(next)) => {
                if next.on && (!view.on || next.max_width != view.max_width) {
                    dirty = true;
                    in_flight = None;
                }
                view = next;
            }
            Ok(Control::Ack(acked)) => {
                if in_flight.is_some_and(|(sent, _)| sent == acked) {
                    in_flight = None;
                }
            }
            Ok(Control::Resize(width, height, done)) => {
                let resized = grabber.resize(width, height);
                if resized.is_ok() {
                    dirty = true;
                }
                let _ = done.send(resized);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        match grabber.changed() {
            Ok(true) => {
                let now = Instant::now();
                moving = now.duration_since(last_change) < FAST;
                last_change = now;
                dirty = true;
            }
            Ok(false) => {}
            Err(why) => {
                tracing::warn!(%why, "the live view lost the screen");
                return;
            }
        }
        if !view.on {
            continue;
        }
        if in_flight.is_some_and(|(_, at)| at.elapsed() < ACK_WAIT) {
            continue;
        }
        if last_sent.is_some_and(|at| at.elapsed() < gap) {
            continue;
        }
        let settle = !dirty && soft && last_change.elapsed() >= SETTLE;
        if !dirty && !settle {
            continue;
        }
        // Soft while it moves; the settling frame is sharp.
        let fast = dirty && moving;
        let pixels = match grabber.grab() {
            Ok(pixels) => pixels,
            Err(why) => {
                tracing::warn!(%why, "the live view couldn't read the screen");
                return;
            }
        };
        let (jpeg, size) = match encode(&pixels, view.max_width, quality(view, fast)) {
            Ok(encoded) => encoded,
            Err(why) => {
                tracing::warn!(%why, "the live view couldn't encode a frame");
                continue;
            }
        };
        seq = seq.wrapping_add(1);
        out.send(Channel::Frames, frame(seq, &jpeg, size, (grabber.width, grabber.height)));
        let now = Instant::now();
        in_flight = Some((seq, now));
        last_sent = Some(now);
        dirty = false;
        soft = fast;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_its_header_then_its_jpeg() {
        let payload = frame(0x0102_0304, b"jpeg", (640, 400), (1280, 800));
        assert_eq!(&payload[..FrameHeader::LEN], &[1, 2, 3, 4, 2, 128, 1, 144, 5, 0, 3, 32]);
        assert_eq!(&payload[FrameHeader::LEN..], b"jpeg");
    }

    #[test]
    fn frames_are_scaled_to_the_width_asked_and_softer_while_moving() {
        let image = image::RgbImage::from_pixel(1280, 800, image::Rgb([200, 100, 50]));
        let (jpeg, size) = encode(&image, 640, 70).expect("encoded");
        assert_eq!(size, (640, 400));
        assert!(jpeg.starts_with(&[0xff, 0xd8]), "a JPEG");
        let (_, size) = encode(&image, 2000, 70).expect("encoded");
        assert_eq!(size, (1280, 800), "never larger than the screen");
        let view = View { on: true, max_width: 1280, quality: 80 };
        assert_eq!((quality(view, false), quality(view, true)), (80, 56));
        assert_eq!(quality(View { quality: 10, ..view }, true), 20);
    }
}
