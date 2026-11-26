//! The person's input on the browser's screen (#435 P2): what the live
//! view sends on C1's input channel, applied through XTest as if typed and
//! clicked there. The host decides whose input reaches the runner (only
//! the turn's session, capped: P4); this only applies it.
//!
//! Text is typed key by key with the keyboard's map, as `xdotool type`
//! does: a character no key has gets a spare keycode for the moment, put
//! back when the text is done.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{self, ConnectionExt as _};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use super::keysym::{self, Keysym, Map, Press};
use super::wire::{Button, InputEvent, MAX_TEXT};
use super::xvfb::Display;

/// The most wheel notches one event turns, either way (as C1 caps them).
const NOTCHES_MAX: i32 = 10;
/// Between two typed characters: as xdotool's, so pages that listen to
/// each key keep up.
const KEY_GAP: Duration = Duration::from_millis(3);
/// A click's button held down, as xdotool's.
const CLICK_HOLD: Duration = Duration::from_millis(12);
/// Between the pointer arriving somewhere new and a press there.
const MOVE_SETTLE: Duration = Duration::from_millis(10);
/// After a spare key is mapped, for the browser to read the new map.
const REMAP_GAP: Duration = Duration::from_millis(15);

// XTest's event types.
const KEY_PRESS: u8 = 2;
const KEY_RELEASE: u8 = 3;
const BUTTON_PRESS: u8 = 4;
const BUTTON_RELEASE: u8 = 5;
const MOTION: u8 = 6;

/// Applies input to the screen.
pub struct Input {
    connection: RustConnection,
    root: xproto::Window,
    width: u16,
    height: u16,
    map: Map,
    /// Set to stop text being typed, from another thread.
    halt: Arc<AtomicBool>,
    /// Where this last put the pointer.
    at: Option<(i16, i16)>,
}

impl Input {
    pub fn new(display: &Display) -> Result<Input, String> {
        let connection = display.connect()?;
        let (root, width, height) = {
            let screen = connection.setup().roots.first().ok_or("the screen has no root window")?;
            (screen.root, screen.width_in_pixels, screen.height_in_pixels)
        };
        connection.xtest_get_version(2, 2).map_err(fail)?.reply().map_err(|_| "the screen has no XTest".to_owned())?;
        let map = keyboard(&connection)?;
        Ok(Input { connection, root, width, height, map, halt: Arc::new(AtomicBool::new(false)), at: None })
    }

    /// A flag that, set, stops the text being typed now (the runner
    /// stopping mid-way through a long one).
    pub fn halt(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.halt)
    }

    /// Keeps the pointer within `width` × `height` from the top left: the
    /// view, resized.
    pub fn bound(&mut self, width: u16, height: u16) {
        (self.width, self.height) = (width, height);
    }

    /// Applies one event.
    pub fn apply(&mut self, event: &InputEvent) -> Result<(), String> {
        match event {
            InputEvent::Move { x, y } => {
                self.point(*x, *y)?;
            }
            InputEvent::Down { button, x, y } => {
                self.point_before_press(*x, *y)?;
                self.fake(BUTTON_PRESS, pointer_button(*button))?;
            }
            InputEvent::Up { button, x, y } => {
                self.point(*x, *y)?;
                self.fake(BUTTON_RELEASE, pointer_button(*button))?;
            }
            InputEvent::Click { x, y } => {
                self.point_before_press(*x, *y)?;
                self.fake(BUTTON_PRESS, 1)?;
                // Held a moment, as a hand (and xdotool) does.
                self.connection.sync().map_err(fail)?;
                std::thread::sleep(CLICK_HOLD);
                self.fake(BUTTON_RELEASE, 1)?;
            }
            InputEvent::Wheel { x, y, dx, dy } => {
                self.point(*x, *y)?;
                for (notches, back, forth) in [(*dy, 4, 5), (*dx, 6, 7)] {
                    let button = if notches < 0 { back } else { forth };
                    for _ in 0..notches.clamp(-NOTCHES_MAX, NOTCHES_MAX).unsigned_abs() {
                        self.fake(BUTTON_PRESS, button)?;
                        self.fake(BUTTON_RELEASE, button)?;
                    }
                }
            }
            InputEvent::Key { key, mods } => {
                let Some(keysym) = keysym::of_key(key) else { return Ok(()) };
                let held: Vec<Keysym> = mods.iter().map(|modifier| keysym::of_modifier(*modifier)).collect();
                self.chord(&held, keysym)?;
            }
            InputEvent::Text { s } => self.type_text(s)?,
        }
        self.connection.sync().map_err(fail)
    }

    /// Presses `keysym` with `held` down, then lets all go; sent before it
    /// returns.
    pub fn chord(&mut self, held: &[Keysym], keysym: Keysym) -> Result<(), String> {
        let mut down = Vec::new();
        for modifier in held {
            if let Some(press) = self.map.find(*modifier) {
                self.fake(KEY_PRESS, press.keycode)?;
                down.push(press.keycode);
            }
        }
        let mut remapped = Vec::new();
        let pressed = self.press(keysym, &mut remapped);
        for keycode in down.into_iter().rev() {
            self.fake(KEY_RELEASE, keycode)?;
        }
        self.connection.sync().map_err(fail)?;
        self.restore(remapped)?;
        pressed
    }

    /// Puts spare keys mapped for the moment back to nothing.
    fn restore(&mut self, remapped: Vec<u8>) -> Result<(), String> {
        for keycode in remapped {
            self.map.set(keycode, 0);
            self.remap(keycode, 0)?;
        }
        Ok(())
    }

    /// Types `text`, key by key; spare keys it mapped are put back after.
    fn type_text(&mut self, text: &str) -> Result<(), String> {
        let mut remapped = Vec::new();
        let mut typed = Ok(());
        for char in text.chars().take(MAX_TEXT) {
            if self.halt.load(Ordering::Relaxed) {
                break;
            }
            let Some(keysym) = keysym::of_char(char) else { continue };
            typed = self.press(keysym, &mut remapped);
            if typed.is_err() {
                break;
            }
            self.connection.flush().map_err(fail)?;
            std::thread::sleep(KEY_GAP);
        }
        self.restore(remapped)?;
        typed
    }

    /// Presses and releases the key for `keysym` (with Shift if it's on
    /// the shifted level), mapping a spare key to it if none has it.
    fn press(&mut self, keysym: Keysym, remapped: &mut Vec<u8>) -> Result<(), String> {
        let press = match self.map.find(keysym) {
            Some(press) => press,
            None => {
                let spares = self.map.spare();
                // Spares in turn, the oldest remapped first when all are used.
                let keycode = match spares.first() {
                    Some(keycode) => *keycode,
                    None if !remapped.is_empty() => remapped.remove(0),
                    None => return Ok(()),
                };
                self.map.set(keycode, keysym);
                self.remap(keycode, keysym)?;
                if !remapped.contains(&keycode) {
                    remapped.push(keycode);
                }
                std::thread::sleep(REMAP_GAP);
                Press { keycode, shift: false }
            }
        };
        let shift = match press.shift {
            true => self.map.find(keysym::SHIFT_L).map(|shift| shift.keycode),
            false => None,
        };
        if let Some(shift) = shift {
            self.fake(KEY_PRESS, shift)?;
        }
        self.fake(KEY_PRESS, press.keycode)?;
        self.fake(KEY_RELEASE, press.keycode)?;
        if let Some(shift) = shift {
            self.fake(KEY_RELEASE, shift)?;
        }
        Ok(())
    }

    /// Tells the server `keycode` gives `keysym` now (0: nothing).
    fn remap(&self, keycode: u8, keysym: Keysym) -> Result<(), String> {
        let per = self.map.per_keycode.max(1);
        let mut keysyms = vec![0; usize::from(per)];
        keysyms[0] = keysym;
        if per > 1 {
            keysyms[1] = keysym;
        }
        self.connection.change_keyboard_mapping(1, keycode, per, &keysyms).map_err(fail)?;
        self.connection.sync().map_err(fail)
    }

    /// Moves the pointer to (`x`, `y`), kept on the view; whether it moved.
    fn point(&mut self, x: u32, y: u32) -> Result<bool, String> {
        let x = i16::try_from(x.min(u32::from(self.width.saturating_sub(1)))).unwrap_or(0);
        let y = i16::try_from(y.min(u32::from(self.height.saturating_sub(1)))).unwrap_or(0);
        if self.at == Some((x, y)) {
            return Ok(false);
        }
        self.connection.xtest_fake_input(MOTION, 0, 0, self.root, x, y, 0).map_err(fail)?;
        self.at = Some((x, y));
        Ok(true)
    }

    /// Moves the pointer for a press: when it moved, the browser sees it
    /// arrive before the press, as from a hand, rather than both at once.
    fn point_before_press(&mut self, x: u32, y: u32) -> Result<(), String> {
        if self.point(x, y)? {
            self.connection.sync().map_err(fail)?;
            std::thread::sleep(MOVE_SETTLE);
        }
        Ok(())
    }

    fn fake(&self, kind: u8, detail: u8) -> Result<(), String> {
        self.connection.xtest_fake_input(kind, detail, 0, x11rb::NONE, 0, 0, 0).map_err(fail)?;
        Ok(())
    }
}

/// X's number for a button.
fn pointer_button(button: Button) -> u8 {
    match button {
        Button::Left => 1,
        Button::Middle => 2,
        Button::Right => 3,
    }
}

/// The keyboard's map as the server has it.
fn keyboard(connection: &RustConnection) -> Result<Map, String> {
    let setup = connection.setup();
    let (min, max) = (setup.min_keycode, setup.max_keycode);
    let reply = connection.get_keyboard_mapping(min, max - min + 1).map_err(fail)?.reply().map_err(fail)?;
    Ok(Map { min_keycode: min, per_keycode: reply.keysyms_per_keycode, keysyms: reply.keysyms })
}

fn fail(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_are_xs() {
        assert_eq!((pointer_button(Button::Left), pointer_button(Button::Middle), pointer_button(Button::Right)), (1, 2, 3));
    }
}
