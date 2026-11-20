//! A QR code drawn with text, for `factoryd pair` on a machine with no
//! screen of its own: the person scans the terminal with their phone, or
//! with Claktory on another device.

use qrcode::{Color, QrCode};

/// Modules of light around the code, which scanners need to find it.
const QUIET: usize = 2;

/// `text` as a QR code in lines of text: each character is two modules, one
/// above the other (▀ ▄ █ or a space), black on white whatever the
/// terminal's colors; None when it's too long for a QR code.
pub fn lines(text: &str) -> Option<Vec<String>> {
    let code = QrCode::new(text.as_bytes()).ok()?;
    let width = code.width();
    let colors = code.to_colors();
    let dark = |x: usize, y: usize| {
        x >= QUIET && y >= QUIET && x < width + QUIET && y < width + QUIET && colors[(y - QUIET) * width + x - QUIET] == Color::Dark
    };
    let size = width + 2 * QUIET;
    let mut lines = Vec::new();
    for y in (0..size).step_by(2) {
        // Black on bright white.
        let mut line = String::from("\x1b[30;107m");
        for x in 0..size {
            line.push(match (dark(x, y), dark(x, y + 1)) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        line.push_str("\x1b[0m");
        lines.push(line);
    }
    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_module_is_drawn_inside_a_light_border() {
        let lines = lines("claktory://pair?code=abc").expect("a code");
        let width = QrCode::new(b"claktory://pair?code=abc").expect("code").width() + 2 * QUIET;
        assert_eq!(lines.len(), width.div_ceil(2));
        for line in &lines {
            let body = line.strip_prefix("\x1b[30;107m").and_then(|line| line.strip_suffix("\x1b[0m")).expect("colored");
            assert_eq!(body.chars().count(), width);
            assert!(body.starts_with("  ") && body.ends_with("  "), "{body:?}");
        }
        // Two light rows, then the top-left finder pattern: its dark edge row
        // over the row with dark only at the ends.
        assert!(lines[0].contains(&" ".repeat(width)));
        assert!(lines[1].contains("  █▀▀▀▀▀█"), "{}", lines[1]);
    }

    #[test]
    fn text_too_long_for_a_qr_code_has_none() {
        assert!(lines(&"x".repeat(8000)).is_none());
    }
}
