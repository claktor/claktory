//! The console, for the few commands a person types an answer into
//! (`factoryd login`'s pasted code): echo off while they type, as termios
//! does elsewhere.

use windows_sys::Win32::System::Console::{
    CONSOLE_MODE, ENABLE_ECHO_INPUT, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, SetConsoleMode,
};

/// Turns the console's echo off until dropped. None when stdin isn't a
/// console (a pipe, a file, an ssh session without one).
pub struct EchoOff(CONSOLE_MODE);

impl EchoOff {
    pub fn new() -> Option<EchoOff> {
        // SAFETY: plain calls on this process's stdin handle.
        unsafe {
            let input = GetStdHandle(STD_INPUT_HANDLE);
            let mut original: CONSOLE_MODE = 0;
            if input.is_null() || GetConsoleMode(input, &raw mut original) == 0 {
                return None;
            }
            if SetConsoleMode(input, original & !ENABLE_ECHO_INPUT) == 0 {
                return None;
            }
            Some(EchoOff(original))
        }
    }
}

impl Drop for EchoOff {
    fn drop(&mut self) {
        // SAFETY: restores the mode read from the same handle in `new`.
        unsafe {
            SetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), self.0);
        }
    }
}
