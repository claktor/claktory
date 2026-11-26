//! Keys and characters as X keysyms (#435 P2): what the live view's keys
//! (DOM `KeyboardEvent.key` names, "Enter", "ArrowLeft", "a") and typed
//! text become on the browser's X screen, as `xdotool type` does it.

use super::wire::Modifier;

/// A keysym, X's name for what a key means.
pub type Keysym = u32;

pub const SHIFT_L: Keysym = 0xffe1;
pub const CONTROL_L: Keysym = 0xffe3;
pub const ALT_L: Keysym = 0xffe9;
pub const SUPER_L: Keysym = 0xffeb;
pub const RETURN: Keysym = 0xff0d;
pub const TAB: Keysym = 0xff09;

/// DOM key names (`KeyboardEvent.key`) with a keysym of their own.
const NAMED: &[(&str, Keysym)] = &[
    ("Enter", RETURN),
    ("Tab", TAB),
    ("Backspace", 0xff08),
    ("Escape", 0xff1b),
    ("Delete", 0xffff),
    ("Insert", 0xff63),
    ("Home", 0xff50),
    ("End", 0xff57),
    ("PageUp", 0xff55),
    ("PageDown", 0xff56),
    ("ArrowLeft", 0xff51),
    ("ArrowUp", 0xff52),
    ("ArrowRight", 0xff53),
    ("ArrowDown", 0xff54),
    ("ContextMenu", 0xff67),
    ("Shift", SHIFT_L),
    ("Control", CONTROL_L),
    ("Alt", ALT_L),
    ("Meta", SUPER_L),
    ("CapsLock", 0xffe5),
    (" ", 0x20),
    ("Spacebar", 0x20),
];

/// The keysym of a DOM key name: a named key, a function key (F1 to F24),
/// or a single character; None for anything else ("Dead", "Unidentified",
/// "Process", a name the view doesn't pass on).
pub fn of_key(key: &str) -> Option<Keysym> {
    if let Some((_, keysym)) = NAMED.iter().find(|(name, _)| *name == key) {
        return Some(*keysym);
    }
    if let Some(number) = key.strip_prefix('F').and_then(|number| number.parse::<u32>().ok())
        && (1..=24).contains(&number)
    {
        // F1 is 0xffbe, and the rest follow it.
        return Some(0xffbe + number - 1);
    }
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(char), None) => of_char(char),
        _ => None,
    }
}

/// The keysym that types `char`: a line break is Return and a tab Tab;
/// Latin-1 has keysyms equal to its code points; any other character is
/// its code point plus 0x01000000, which X takes for any Unicode
/// character. None for other control characters, which type nothing.
pub fn of_char(char: char) -> Option<Keysym> {
    match char {
        '\n' | '\r' => Some(RETURN),
        '\t' => Some(TAB),
        char if char.is_control() => None,
        char => {
            let code = u32::from(char);
            if (0x20..=0x7e).contains(&code) || (0xa0..=0xff).contains(&code) { Some(code) } else { Some(0x0100_0000 | code) }
        }
    }
}

/// The keysym of a modifier an input event holds.
pub fn of_modifier(modifier: Modifier) -> Keysym {
    match modifier {
        Modifier::Shift => SHIFT_L,
        Modifier::Ctrl => CONTROL_L,
        Modifier::Alt => ALT_L,
        Modifier::Meta => SUPER_L,
    }
}

/// The keysyms a keyboard map lists for each keycode, from `min_keycode`,
/// `per_keycode` each, as GetKeyboardMapping answers.
pub struct Map {
    pub min_keycode: u8,
    pub per_keycode: u8,
    pub keysyms: Vec<Keysym>,
}

/// How to press a keysym on a map: its keycode, and whether Shift goes
/// with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Press {
    pub keycode: u8,
    pub shift: bool,
}

impl Map {
    /// The keycode that gives `keysym`, unshifted where one does, else with
    /// Shift; None when no key of the map has it (it gets a spare one).
    pub fn find(&self, keysym: Keysym) -> Option<Press> {
        let per = usize::from(self.per_keycode.max(1));
        let found = |level: usize| {
            self.keysyms
                .chunks(per)
                .position(|keysyms| keysyms.get(level) == Some(&keysym))
                .and_then(|index| u8::try_from(usize::from(self.min_keycode) + index).ok())
        };
        if let Some(keycode) = found(0) {
            return Some(Press { keycode, shift: false });
        }
        found(1).map(|keycode| Press { keycode, shift: true })
    }

    /// Keycodes with no keysym at all, to map a character no key has for
    /// the moment, as xdotool does.
    pub fn spare(&self) -> Vec<u8> {
        let per = usize::from(self.per_keycode.max(1));
        self.keysyms
            .chunks(per)
            .enumerate()
            .filter(|(_, keysyms)| keysyms.iter().all(|keysym| *keysym == 0))
            .filter_map(|(index, _)| u8::try_from(usize::from(self.min_keycode) + index).ok())
            .collect()
    }

    /// Maps `keycode` to `keysym` alone, as ChangeKeyboardMapping is told.
    pub fn set(&mut self, keycode: u8, keysym: Keysym) {
        let per = usize::from(self.per_keycode.max(1));
        let Some(index) = usize::from(keycode).checked_sub(usize::from(self.min_keycode)) else { return };
        if let Some(keysyms) = self.keysyms.chunks_mut(per).nth(index) {
            keysyms.fill(0);
            keysyms[0] = keysym;
            if let Some(shifted) = keysyms.get_mut(1) {
                *shifted = keysym;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_named_as_the_browser_names_them() {
        assert_eq!(of_key("Enter"), Some(0xff0d));
        assert_eq!(of_key("ArrowLeft"), Some(0xff51));
        assert_eq!(of_key("F1"), Some(0xffbe));
        assert_eq!(of_key("F12"), Some(0xffc9));
        assert_eq!(of_key("F25"), None);
        assert_eq!(of_key("a"), Some(0x61));
        assert_eq!(of_key("A"), Some(0x41));
        assert_eq!(of_key(" "), Some(0x20));
        assert_eq!(of_key("é"), Some(0xe9));
        assert_eq!(of_key("€"), Some(0x0100_20ac));
        assert_eq!(of_key("Dead"), None);
        assert_eq!(of_key("Unidentified"), None);
        assert_eq!(of_key(""), None);
    }

    #[test]
    fn text_is_typed_character_by_character() {
        assert_eq!(of_char('\n'), Some(RETURN));
        assert_eq!(of_char('\t'), Some(TAB));
        assert_eq!(of_char('\u{7}'), None);
        assert_eq!(of_char('ß'), Some(0xdf));
        assert_eq!(of_char('😀'), Some(0x0101_f600));
        assert_eq!(of_modifier(Modifier::Ctrl), CONTROL_L);
    }

    #[test]
    fn a_map_finds_unshifted_first_then_shifted_and_spares_the_empty_keys() {
        // Keycodes 10, 11, 12: "a A", "1 !", nothing.
        let mut map = Map { min_keycode: 10, per_keycode: 2, keysyms: vec![0x61, 0x41, 0x31, 0x21, 0, 0] };
        assert_eq!(map.find(0x61), Some(Press { keycode: 10, shift: false }));
        assert_eq!(map.find(0x41), Some(Press { keycode: 10, shift: true }));
        assert_eq!(map.find(0x21), Some(Press { keycode: 11, shift: true }));
        assert_eq!(map.find(0xe9), None);
        assert_eq!(map.spare(), [12]);
        map.set(12, 0xe9);
        assert_eq!(map.find(0xe9), Some(Press { keycode: 12, shift: false }));
        assert!(map.spare().is_empty());
    }
}
