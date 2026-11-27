//! The keys an errand's typing presses (#530, R11.3 #3): for each character
//! a person types on a US keyboard, the key, its code and whether Shift is
//! held; anything else is inserted as text, one character at a time. Only
//! characters: never Tab, Enter or a chord, which reach the browser's own
//! shortcuts or send forms. Ctrl+A, to replace a field's text, is
//! [`SELECT_ALL`], sent only while the field read has the focus.

/// A key that types one character.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Key {
    pub key: &'static str,
    pub code: String,
    /// Windows' virtual key code, which Chrome also takes as the native one.
    pub vk: u32,
    pub shift: bool,
    /// What the key types without Shift.
    pub unshifted: String,
}

/// The symbols on a US keyboard: (unshifted, shifted, code, virtual key).
const SYMBOLS: [(char, char, &str, u32); 21] = [
    ('`', '~', "Backquote", 192),
    ('-', '_', "Minus", 189),
    ('=', '+', "Equal", 187),
    ('[', '{', "BracketLeft", 219),
    (']', '}', "BracketRight", 221),
    ('\\', '|', "Backslash", 220),
    (';', ':', "Semicolon", 186),
    ('\'', '"', "Quote", 222),
    (',', '<', "Comma", 188),
    ('.', '>', "Period", 190),
    ('/', '?', "Slash", 191),
    ('1', '!', "Digit1", 49),
    ('2', '@', "Digit2", 50),
    ('3', '#', "Digit3", 51),
    ('4', '$', "Digit4", 52),
    ('5', '%', "Digit5", 53),
    ('6', '^', "Digit6", 54),
    ('7', '&', "Digit7", 55),
    ('8', '*', "Digit8", 56),
    ('9', '(', "Digit9", 57),
    ('0', ')', "Digit0", 48),
];

/// The key for `grapheme`, if one key types it; None for anything else
/// (accented letters, other scripts, emoji), which is inserted as text.
pub(crate) fn key(grapheme: &str) -> Option<Key> {
    let mut chars = grapheme.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else { return None };
    let leaked = |c: char| -> &'static str {
        // Every key is one of the printable ASCII characters: the same few
        // strings each time, so a small table rather than a leak.
        const PRINTABLE: &str = " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~";
        let at = PRINTABLE.find(c).unwrap_or(0);
        &PRINTABLE[at..at + 1]
    };
    match c {
        ' ' => Some(Key { key: " ", code: "Space".into(), vk: 32, shift: false, unshifted: " ".into() }),
        'a'..='z' => Some(Key { key: leaked(c), code: format!("Key{}", c.to_ascii_uppercase()), vk: c.to_ascii_uppercase() as u32, shift: false, unshifted: c.to_string() }),
        'A'..='Z' => Some(Key { key: leaked(c), code: format!("Key{c}"), vk: c as u32, shift: true, unshifted: c.to_ascii_lowercase().to_string() }),
        _ => SYMBOLS.iter().find_map(|&(plain, shifted, code, vk)| {
            (c == plain || c == shifted).then(|| Key { key: leaked(c), code: code.into(), vk, shift: c == shifted, unshifted: plain.to_string() })
        }),
    }
}

/// Ctrl+A with the editing command that selects a field's whole text, on
/// any system (a Mac's Ctrl+A moves to the line's start instead).
pub(crate) const SELECT_ALL: &str = "selectAll";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_us_keyboard_types_ascii_and_nothing_else() {
        assert_eq!(key("a"), Some(Key { key: "a", code: "KeyA".into(), vk: 65, shift: false, unshifted: "a".into() }));
        assert_eq!(key("Q"), Some(Key { key: "Q", code: "KeyQ".into(), vk: 81, shift: true, unshifted: "q".into() }));
        assert_eq!(key("7").map(|k| (k.code, k.shift)), Some(("Digit7".into(), false)));
        assert_eq!(key("@").map(|k| (k.code, k.shift, k.unshifted)), Some(("Digit2".into(), true, "2".into())));
        assert_eq!(key("\"").map(|k| (k.code, k.shift)), Some(("Quote".into(), true)));
        assert_eq!(key(" ").map(|k| k.code), Some("Space".into()));
        for other in ["é", "ß", "日", "👍🏽", "\t", "\n", "\r", "ab"] {
            assert_eq!(key(other), None, "{other:?}");
        }
    }
}
