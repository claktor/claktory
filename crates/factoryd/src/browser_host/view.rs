//! The live view (#435 P4): frames to the one connection watching, what's
//! copied during the person's turn, and the person's input on its way to
//! the runner, each checked: a frame's JPEG must be the size its header
//! says and no bigger than asked (a hostile runner's decompression bomb
//! would otherwise reach the phone's decoder, Fugo 36598), and input is
//! clamped and capped before it's scaled to the screen (Fugo 36686).

use factory_protocol::v1::{self, AgentBrowserPointerKind as PointerKind, agent_browser_event::Event, server_message::Body};

use crate::browser_runner::wire::{self, Button, Channel, FrameHeader, InputEvent, Modifier};

pub(crate) use super::tab_view::{check_opened, check_tab, end_check};

/// The most input events taken a second from the person.
pub(crate) const EVENTS_PER_SECOND: u32 = 240;
/// The most text in one AgentBrowserInput, typed and pasted together.
const MAX_TEXT_BYTES: usize = 4096;
/// The most events in one AgentBrowserInput.
const MAX_EVENTS: usize = 256;
/// The biggest frame forwarded, either way.
const MAX_SIDE: u16 = 4096;
/// Screen pixels per wheel notch.
const NOTCH: f32 = 50.0;
/// The longest web address opened.
const MAX_URL: usize = 2048;

/// What the runner says, made safe to show: no control or invisible
/// characters, at most `max` characters.
pub(crate) fn cleaned(text: &str, max: usize) -> String {
    crate::pipes::shares::shown(text).chars().take(max).collect()
}

/// A frame from the runner: to the viewer, when it's the size it says.
pub(crate) fn frame(one: &super::One, payload: &[u8]) {
    let Some((header, jpeg)) = FrameHeader::split(payload) else { return };
    let mut inner = one.inner();
    let ack = |inner: &super::Inner| {
        if let Some(runner) = &inner.runner {
            let _ = runner.try_send((Channel::Frames, wire::ack(header.seq).to_vec()));
        }
    };
    let Some(viewer) = &inner.viewer else {
        ack(&inner);
        return;
    };
    let fits = jpeg_size(jpeg) == Some((header.width, header.height))
        && header.width > 0
        && header.height > 0
        && header.width <= MAX_SIDE
        && header.height <= MAX_SIDE
        && u32::from(header.width) <= viewer.max_width
        && (1..=MAX_SIDE).contains(&header.screen_width)
        && (1..=MAX_SIDE).contains(&header.screen_height);
    if !fits || inner.in_flight.is_some() {
        if !fits {
            tracing::warn!(
                width = header.width,
                height = header.height,
                "a frame from Claktory's browser wasn't the size it said: dropped"
            );
        }
        ack(&inner);
        return;
    }
    let frame = v1::AgentBrowserFrame {
        seq: u64::from(header.seq),
        jpeg: jpeg.to_vec(),
        width: u32::from(header.width),
        height: u32::from(header.height),
        origin: inner.origin.clone(),
        title: inner.title.clone(),
        text_field_focused: inner.text_field_focused,
        screen_width: u32::from(header.screen_width),
        screen_height: u32::from(header.screen_height),
    };
    if viewer.sender.try_send(Body::AgentBrowserFrame(frame)).is_ok() {
        inner.in_flight = Some(header.seq);
        inner.screen = (header.screen_width, header.screen_height);
        // The page's origin and title for the frames to come.
        super::sites::read_page(&mut inner);
    } else {
        ack(&inner);
    }
}

/// What was copied in the browser: to the viewer, only in its turn.
pub(crate) fn clipboard(one: &super::One, payload: Vec<u8>) {
    let inner = one.inner();
    if !super::persons_turn(inner.state) {
        return;
    }
    let Some(viewer) = inner.viewer.as_ref() else { return };
    let text = String::from_utf8_lossy(&payload).into_owned();
    let _ = viewer.sender.try_send(Body::AgentBrowserClipboard(v1::AgentBrowserClipboard { text }));
}

/// A JPEG's size, from its frame header (SOF), without decoding it.
pub(crate) fn jpeg_size(jpeg: &[u8]) -> Option<(u16, u16)> {
    if jpeg.get(..2)? != [0xff, 0xd8] {
        return None;
    }
    let mut at = 2;
    loop {
        // Markers may be padded with 0xff.
        while *jpeg.get(at)? == 0xff && *jpeg.get(at + 1)? == 0xff {
            at += 1;
        }
        if *jpeg.get(at)? != 0xff {
            return None;
        }
        let marker = *jpeg.get(at + 1)?;
        if marker == 0xd8 || marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            at += 2;
            continue;
        }
        if marker == 0xd9 || marker == 0xda {
            return None;
        }
        let len = usize::from(u16::from_be_bytes([*jpeg.get(at + 2)?, *jpeg.get(at + 3)?]));
        if len < 2 {
            return None;
        }
        let sof = matches!(marker, 0xc0..=0xcf) && !matches!(marker, 0xc4 | 0xc8 | 0xcc);
        if sof {
            let body = jpeg.get(at + 4..at + 2 + len)?;
            let height = u16::from_be_bytes([*body.get(1)?, *body.get(2)?]);
            let width = u16::from_be_bytes([*body.get(3)?, *body.get(4)?]);
            return Some((width, height));
        }
        at += 2 + len;
    }
}

/// The person's input, as C1 payloads, in the screen's pixels; or why
/// it's refused whole.
pub(crate) fn input(events: &[v1::AgentBrowserEvent], screen: (u16, u16)) -> Result<Vec<(Channel, Vec<u8>)>, String> {
    check_input(events)?;
    let (width, height) = (f32::from(screen.0.max(1)), f32::from(screen.1.max(1)));
    // A fraction of the frame, refused when it's no number, clamped to it.
    let at = |x: f32, y: f32| -> Option<(u32, u32)> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let x = (x.clamp(0.0, 1.0) * (width - 1.0)).round() as u32;
        let y = (y.clamp(0.0, 1.0) * (height - 1.0)).round() as u32;
        Some((x, y))
    };
    let mut payloads = Vec::new();
    let input = |event: InputEvent| (Channel::Input, wire::encode(&event));
    for event in events {
        match &event.event {
            Some(Event::Pointer(pointer)) => {
                let Some((x, y)) = at(pointer.x, pointer.y) else { continue };
                let button = match pointer.button {
                    2 => Button::Middle,
                    3 => Button::Right,
                    _ => Button::Left,
                };
                match pointer.kind() {
                    PointerKind::Move => payloads.push(input(InputEvent::Move { x, y })),
                    PointerKind::Down => payloads.push(input(InputEvent::Down { button, x, y })),
                    PointerKind::Up => payloads.push(input(InputEvent::Up { button, x, y })),
                    PointerKind::Unspecified => {}
                }
            }
            Some(Event::Wheel(wheel)) => {
                let Some((x, y)) = at(wheel.x, wheel.y) else { continue };
                let (Some(dx), Some(dy)) = (notches(wheel.dx), notches(wheel.dy)) else { continue };
                if dx != 0 || dy != 0 {
                    payloads.push(input(InputEvent::Wheel { x, y, dx, dy }));
                }
            }
            Some(Event::Key(key)) => {
                // The runner presses a key as a chord: on its way down.
                if !key.down || key.key.is_empty() || key.key.chars().count() > 32 {
                    continue;
                }
                if matches!(key.key.as_str(), "Shift" | "Control" | "Alt" | "Meta" | "CapsLock" | "Fn") {
                    continue;
                }
                let mods: Vec<Modifier> = [(2, Modifier::Ctrl), (1, Modifier::Shift), (4, Modifier::Alt), (8, Modifier::Meta)]
                    .into_iter()
                    .filter(|(bit, _)| key.modifiers & bit != 0)
                    .map(|(_, modifier)| modifier)
                    .collect();
                payloads.push(input(InputEvent::Key { key: key.key.clone(), mods }));
            }
            Some(Event::Text(text)) if !text.is_empty() => payloads.push(input(InputEvent::Text { s: text.clone() })),
            Some(Event::Paste(text)) if !text.is_empty() => payloads.push((Channel::Clipboard, text.clone().into_bytes())),
            _ => {}
        }
    }
    Ok(payloads)
}

/// Whether one message of the person's input is within bounds: at most
/// MAX_EVENTS events and MAX_TEXT_BYTES of text, typed and pasted together.
pub(crate) fn check_input(events: &[v1::AgentBrowserEvent]) -> Result<(), String> {
    if events.len() > MAX_EVENTS {
        return Err(format!("At most {MAX_EVENTS} input events at a time."));
    }
    let text: usize = events
        .iter()
        .map(|event| match &event.event {
            Some(Event::Text(text) | Event::Paste(text)) => text.len(),
            _ => 0,
        })
        .sum();
    if text > MAX_TEXT_BYTES {
        return Err("At most 4 KB of text at a time.".to_owned());
    }
    Ok(())
}

/// Frame pixels of a wheel, as notches (at most 10 either way); None for
/// no number.
fn notches(pixels: f32) -> Option<i32> {
    if !pixels.is_finite() {
        return None;
    }
    let notches = (pixels / NOTCH).round();
    let notches = if notches == 0.0 && pixels.abs() >= 1.0 { pixels.signum() } else { notches };
    Some(notches.clamp(-10.0, 10.0) as i32)
}

/// A web address to open: http or https only, whole, never anything a
/// program could take for an option (Fugo 36686); None for where it left
/// off.
pub(crate) fn web_address(url: &str) -> Result<Option<String>, String> {
    let url = url.trim();
    if url.is_empty() {
        return Ok(None);
    }
    let refused = || "Claktory's browser opens only web addresses (http or https).".to_owned();
    if url.len() > MAX_URL || url.chars().any(|char| char.is_control() || char.is_whitespace()) {
        return Err(refused());
    }
    let parsed = url::Url::parse(url).map_err(|_| refused())?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none_or(str::is_empty) {
        return Err(refused());
    }
    Ok(Some(parsed.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use v1::{AgentBrowserEvent, AgentBrowserKey, AgentBrowserPointer, AgentBrowserWheel};

    fn jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut image = image::RgbImage::new(u32::from(width), u32::from(height));
        image.put_pixel(0, 0, image::Rgb([1, 2, 3]));
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut bytes).encode_image(&image).expect("jpeg");
        bytes
    }

    #[test]
    fn a_jpeg_says_its_size_before_it_is_decoded() {
        assert_eq!(jpeg_size(&jpeg(640, 400)), Some((640, 400)));
        assert_eq!(jpeg_size(&jpeg(3, 7)), Some((3, 7)));
        assert_eq!(jpeg_size(b"\xff\xd8"), None);
        assert_eq!(jpeg_size(b"not a jpeg"), None);
        // A header claiming 65535 x 65535 says so: the caller drops it.
        let mut bomb = jpeg(8, 8);
        let at = bomb.windows(2).position(|pair| pair == [0xff, 0xc0]).expect("SOF0");
        bomb[at + 5..at + 9].copy_from_slice(&[0xff, 0xff, 0xff, 0xff]);
        assert_eq!(jpeg_size(&bomb), Some((65535, 65535)));
        // Cut short anywhere: no size, never a panic.
        let whole = jpeg(16, 16);
        for end in 0..whole.len().min(700) {
            let _ = jpeg_size(&whole[..end]);
        }
    }

    fn pointer(kind: PointerKind, x: f32, y: f32) -> AgentBrowserEvent {
        AgentBrowserEvent { event: Some(Event::Pointer(AgentBrowserPointer { kind: kind.into(), x, y, button: 1, frame_seq: 1 })) }
    }

    #[test]
    fn input_is_clamped_scaled_and_capped() {
        let screen = (1280, 800);
        let events = [
            pointer(PointerKind::Down, 0.5, 0.5),
            pointer(PointerKind::Up, 2.0, -1.0),
            pointer(PointerKind::Move, f32::NAN, 0.5),
            pointer(PointerKind::Move, f32::INFINITY, 0.5),
            AgentBrowserEvent { event: Some(Event::Wheel(AgentBrowserWheel { x: 0.1, y: 0.1, dx: 0.0, dy: 1e9, frame_seq: 1 })) },
            AgentBrowserEvent { event: Some(Event::Key(AgentBrowserKey { down: true, key: "a".into(), modifiers: 2 })) },
            AgentBrowserEvent { event: Some(Event::Key(AgentBrowserKey { down: false, key: "a".into(), modifiers: 2 })) },
            AgentBrowserEvent { event: Some(Event::Key(AgentBrowserKey { down: true, key: "Shift".into(), modifiers: 1 })) },
            AgentBrowserEvent { event: Some(Event::Text("héllo".into())) },
            AgentBrowserEvent { event: Some(Event::Paste("pass".into())) },
        ];
        let payloads = input(&events, screen).expect("taken");
        let read: Vec<(Channel, Option<InputEvent>)> =
            payloads.iter().map(|(channel, payload)| (*channel, InputEvent::parse(payload))).collect();
        assert_eq!(
            read,
            [
                (Channel::Input, Some(InputEvent::Down { button: Button::Left, x: 640, y: 400 })),
                (Channel::Input, Some(InputEvent::Up { button: Button::Left, x: 1279, y: 0 })),
                (Channel::Input, Some(InputEvent::Wheel { x: 128, y: 80, dx: 0, dy: 10 })),
                (Channel::Input, Some(InputEvent::Key { key: "a".into(), mods: vec![Modifier::Ctrl] })),
                (Channel::Input, Some(InputEvent::Text { s: "héllo".into() })),
                (Channel::Clipboard, None),
            ]
        );
        assert_eq!(payloads[5].1, b"pass");

        let long = AgentBrowserEvent { event: Some(Event::Text("x".repeat(4097))) };
        assert!(input(&[long], screen).is_err());
        let many = vec![pointer(PointerKind::Move, 0.0, 0.0); MAX_EVENTS + 1];
        assert!(input(&many, screen).is_err());
    }

    #[test]
    fn only_web_addresses_open() {
        assert_eq!(web_address(""), Ok(None));
        assert_eq!(web_address("https://play.google.com/console"), Ok(Some("https://play.google.com/console".into())));
        for refused in [
            "--remote-debugging-port=9222",
            "-x",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "chrome://settings",
            "data:text/html,hi",
            "https://a.example/ --load-extension=/tmp/x",
            "https://",
        ] {
            assert!(web_address(refused).is_err(), "{refused}");
        }
    }
}
