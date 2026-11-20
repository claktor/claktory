//! A comment's place in an image or a video, in words for agents: the W3C
//! media fragment the app sends (`xywh=percent:12.5,8,30,20`, or in pixels;
//! `t=31.2,45` for a video's moments, and with `&xywh=…` a region of its
//! frame), and an image's size in pixels read from its header (PNG, JPEG,
//! GIF, WebP).

use serde_json::Value;

/// Where in an image or a frame a comment is.
#[derive(Debug, PartialEq)]
pub(super) struct Place {
    /// A spot (a region of no size, `xywh=percent:62,30,0,0`) rather than a region.
    pub spot: bool,
    /// In words, with pixels when the image's size is known: a region "30%
    /// wide and 20% tall, its top left 12.5% across and 8% down (that's 384 ×
    /// 216 px at 160, 86 of 1280 × 1080)"; a spot "62% across and 30% down
    /// (that's pixel 794, 324 of 1280 × 1080)".
    pub words: String,
}

/// The region or spot in `selectors` (a JSON array), as a `Place`.
pub(super) fn describe(selectors: &str, size: Option<(u32, u32)>) -> Option<Place> {
    let fragment = part(selectors, "xywh")?;
    let (unit, numbers) = match fragment.split_once(':') {
        Some((unit, numbers)) => (unit, numbers),
        None => ("pixel", fragment.as_str()),
    };
    let numbers: Vec<f64> = numbers.split(',').map(|number| number.trim().parse::<f64>()).collect::<Result<_, _>>().ok()?;
    let [x, y, w, h] = numbers.as_slice() else { return None };
    if [x, y, w, h].iter().any(|number| !number.is_finite() || **number < 0.0) {
        return None;
    }
    let of = |(width, height): (u32, u32)| format!(" of {width} × {height}");
    let spot = *w == 0.0 && *h == 0.0;
    let words = match unit {
        "percent" if spot => {
            let mut words = format!("{}% across and {}% down", number(*x), number(*y));
            if let Some((width, height)) = size {
                let px = |percent: f64, whole: u32| (percent / 100.0 * f64::from(whole)).round();
                words.push_str(&format!(" (that's pixel {}, {}{})", px(*x, width), px(*y, height), of((width, height))));
            }
            words
        }
        "percent" => {
            let mut words =
                format!("{}% wide and {}% tall, its top left {}% across and {}% down", number(*w), number(*h), number(*x), number(*y));
            if let Some((width, height)) = size {
                let px = |percent: f64, whole: u32| (percent / 100.0 * f64::from(whole)).round();
                let (left, top) = (px(*x, width), px(*y, height));
                let (wide, tall) = (px(*w, width), px(*h, height));
                words.push_str(&format!(" (that's {wide} × {tall} px at {left}, {top}{})", of((width, height))));
            }
            words
        }
        "pixel" if spot => format!("pixel {}, {}{}", number(*x), number(*y), size.map(of).unwrap_or_default()),
        "pixel" => format!("{} × {} px at {}, {}{}", number(*w), number(*h), number(*x), number(*y), size.map(of).unwrap_or_default()),
        _ => return None,
    };
    Some(Place { spot, words })
}

/// A media fragment's `name=` part, from the first fragment selector.
fn part(selectors: &str, name: &str) -> Option<String> {
    let selectors: Vec<Value> = serde_json::from_str(selectors).ok()?;
    let value = selectors.iter().find_map(|selector| {
        let kind = selector["type"].as_str().unwrap_or_default();
        (kind == "FragmentSelector" || kind == "Fragment").then(|| selector["value"].as_str()).flatten()
    })?;
    value.trim().split('&').find_map(|part| part.strip_prefix(name)?.strip_prefix('=')).map(str::to_owned)
}

/// When in a video, in words: "at 0:31–0:45", or "at 0:31" for a moment.
pub(super) fn when(selectors: &str) -> Option<String> {
    let times = part(selectors, "t")?;
    let times = times.strip_prefix("npt:").unwrap_or(&times);
    let (start, end) = match times.split_once(',') {
        Some((start, end)) => (if start.is_empty() { Some(0.0) } else { seconds(start) }, Some(seconds(end)?)),
        None => (seconds(times), None),
    };
    let start = start?;
    match end {
        Some(end) if clock(end) != clock(start) && end > start => Some(format!("at {}–{}", clock(start), clock(end))),
        _ => Some(format!("at {}", clock(start))),
    }
}

/// Seconds from "31.2", "1:02.5" or "1:00:02".
fn seconds(text: &str) -> Option<f64> {
    let mut total = 0.0;
    for part in text.trim().split(':') {
        let value: f64 = part.parse().ok()?;
        if !value.is_finite() || value < 0.0 {
            return None;
        }
        total = total * 60.0 + value;
    }
    Some(total)
}

/// A time as players show it: "0:31", "12:05", "1:02:03".
fn clock(seconds: f64) -> String {
    // Whole seconds only; truncated, as a player's clock is.
    let whole = seconds.floor() as u64;
    let (hours, minutes, seconds) = (whole / 3600, whole / 60 % 60, whole % 60);
    if hours > 0 { format!("{hours}:{minutes:02}:{seconds:02}") } else { format!("{minutes}:{seconds:02}") }
}

/// A number as people write it: up to 2 decimals, none if whole.
fn number(value: f64) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// An image's width and height in pixels, from its header.
pub(super) fn size(bytes: &[u8]) -> Option<(u32, u32)> {
    let be32 = |at: usize| Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    let be16 = |at: usize| Some(u32::from(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?)));
    let le16 = |at: usize| Some(u32::from(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?)));
    let le24 = |at: usize| Some(u32::from(*bytes.get(at)?) | u32::from(*bytes.get(at + 1)?) << 8 | u32::from(*bytes.get(at + 2)?) << 16);
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.get(12..16) == Some(b"IHDR") {
        return Some((be32(16)?, be32(20)?));
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some((le16(6)?, le16(8)?));
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return match bytes.get(12..16)? {
            b"VP8 " => Some((le16(26)? & 0x3fff, le16(28)? & 0x3fff)),
            b"VP8L" => {
                let bits = u32::from_le_bytes(bytes.get(21..25)?.try_into().ok()?);
                Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
            }
            b"VP8X" => Some((le24(24)? + 1, le24(27)? + 1)),
            _ => None,
        };
    }
    if bytes.starts_with(&[0xff, 0xd8]) {
        // Segments up to a start of frame (SOF0–SOF15, but not DHT, JPG or DAC).
        let mut at = 2;
        while at + 4 <= bytes.len() {
            if bytes[at] != 0xff {
                return None;
            }
            let marker = bytes[at + 1];
            if marker == 0xff {
                at += 1;
                continue;
            }
            if (0xd0..=0xd9).contains(&marker) || marker == 0x01 {
                at += 2;
                continue;
            }
            if (0xc0..=0xcf).contains(&marker) && ![0xc4, 0xc8, 0xcc].contains(&marker) {
                return Some((be16(at + 7)?, be16(at + 5)?));
            }
            at += 2 + usize::try_from(be16(at + 2)?).ok()?;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A region's words; None for a spot or nothing.
    fn words(place: Option<Place>) -> Option<String> {
        place.filter(|place| !place.spot).map(|place| place.words)
    }

    #[test]
    fn a_spot_is_said_as_one() {
        let spot = r#"[{"type":"FragmentSelector","value":"xywh=percent:62,30,0,0"}]"#;
        let place = |size| describe(spot, size).filter(|place| place.spot).map(|place| place.words);
        assert_eq!(place(None).as_deref(), Some("62% across and 30% down"));
        assert_eq!(place(Some((1280, 1080))).as_deref(), Some("62% across and 30% down (that's pixel 794, 324 of 1280 × 1080)"));
        let video = r#"[{"type":"FragmentSelector","value":"t=31.2&xywh=percent:62,30,0,0"}]"#;
        assert!(describe(video, None).is_some_and(|place| place.spot));
        let pixels = r#"[{"type":"FragmentSelector","value":"xywh=pixel:794,324,0,0"}]"#;
        assert_eq!(describe(pixels, Some((1280, 1080))).map(|place| place.words).as_deref(), Some("pixel 794, 324 of 1280 × 1080"));
        // Only no size at all is a spot.
        let thin = r#"[{"type":"FragmentSelector","value":"xywh=percent:62,30,0,5"}]"#;
        assert!(describe(thin, None).is_some_and(|place| !place.spot));
    }

    #[test]
    fn regions_are_said_in_percent_and_pixels() {
        let percent =
            r#"[{"type":"FragmentSelector","conformsTo":"http://www.w3.org/TR/media-frags/","value":"xywh=percent:12.5,8,30,20"}]"#;
        assert_eq!(words(describe(percent, None)).as_deref(), Some("30% wide and 20% tall, its top left 12.5% across and 8% down"));
        assert_eq!(
            words(describe(percent, Some((1280, 1080)))).as_deref(),
            Some("30% wide and 20% tall, its top left 12.5% across and 8% down (that's 384 × 216 px at 160, 86 of 1280 × 1080)")
        );
        let pixels = r#"[{"type":"FragmentSelector","value":"xywh=pixel:154,82,384,216"}]"#;
        assert_eq!(words(describe(pixels, Some((1280, 1080)))).as_deref(), Some("384 × 216 px at 154, 82 of 1280 × 1080"));
        for bad in
            ["[]", "nonsense", r#"[{"type":"FragmentSelector","value":"t=10"}]"#, r#"[{"type":"FragmentSelector","value":"xywh=1,2,3"}]"#]
        {
            assert_eq!(describe(bad, None), None, "{bad}");
        }
        assert_eq!(number(12.345), "12.35");
        let video = r#"[{"type":"FragmentSelector","value":"t=31.2,45&xywh=percent:10,20,30,25"}]"#;
        assert_eq!(when(video).as_deref(), Some("at 0:31–0:45"));
        assert_eq!(words(describe(video, None)).as_deref(), Some("30% wide and 25% tall, its top left 10% across and 20% down"));
        assert_eq!(when(r#"[{"type":"FragmentSelector","value":"t=3725"}]"#).as_deref(), Some("at 1:02:05"));
        assert_eq!(when(r#"[{"type":"FragmentSelector","value":"t=npt:,1:05.5"}]"#).as_deref(), Some("at 0:00–1:05"));
        assert_eq!(when(r#"[{"type":"FragmentSelector","value":"t=31.2,31.8"}]"#).as_deref(), Some("at 0:31"));
        assert_eq!(describe(r#"[{"type":"FragmentSelector","value":"t=31.2"}]"#, None), None, "no region");
        assert_eq!(when(r#"[{"type":"FragmentSelector","value":"xywh=1,2,3,4"}]"#), None);
        assert_eq!(number(8.0), "8");
    }

    #[test]
    fn sizes_come_from_image_headers() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&1280u32.to_be_bytes());
        png.extend_from_slice(&1080u32.to_be_bytes());
        assert_eq!(size(&png), Some((1280, 1080)));
        let gif = [b"GIF89a".as_slice(), &640u16.to_le_bytes(), &480u16.to_le_bytes()].concat();
        assert_eq!(size(&gif), Some((640, 480)));
        // A JPEG with an APP0 segment, then its frame: 1080 tall, 1920 wide.
        let jpeg = [
            [0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x00, 0x00].as_slice(),
            &[0xff, 0xc0, 0x00, 0x11, 0x08],
            &1080u16.to_be_bytes(),
            &1920u16.to_be_bytes(),
        ]
        .concat();
        assert_eq!(size(&jpeg), Some((1920, 1080)));
        let mut webp = b"RIFF\0\0\0\0WEBPVP8X\0\0\0\0\0\0\0\0".to_vec();
        webp.extend_from_slice(&[0x7f, 0x02, 0x00, 0xcf, 0x01, 0x00]);
        assert_eq!(size(&webp), Some((640, 464)));
        assert_eq!(size(b"plain text"), None);
    }
}
