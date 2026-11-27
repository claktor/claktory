//! Whether this computer has the memory for another agent's Chrome: each browsing agent has one of its own, so several can
//! run at once, but never so many that the computer would start swapping.
//! A start that wouldn't fit is refused, and its agent is told why; the
//! ones open close by themselves once they've gone unused for 10 minutes.

/// What one browser takes at most: on a Mac its VM's memory (`mac`), on
/// Linux about the same for Chrome with a few tabs.
pub(crate) const PER_BROWSER: u64 = 2 * 1024 * 1024 * 1024;
/// What's left free for everything else, besides the new browser.
const RESERVE: u64 = 1024 * 1024 * 1024;

/// Ok when another browser fits beside the `others` open now; else why
/// not, in words for its agent. When the computer won't say how much
/// memory it has, it's let start.
pub(crate) fn room(others: usize) -> Result<(), String> {
    let Some((total, available)) = measured() else { return Ok(()) };
    fits(others, total, available)
}

/// `room`'s rule: browsers take at most half the computer's memory (one at
/// least), and a new one starts only with its share and the reserve free.
fn fits(others: usize, total: u64, available: u64) -> Result<(), String> {
    let most = usize::try_from(total / 2 / PER_BROWSER).unwrap_or(usize::MAX).max(1);
    if others < most && available >= PER_BROWSER + RESERVE {
        return Ok(());
    }
    Err(match others {
        0 => "Your browser can't open now: this computer doesn't have enough free memory for it. Try again later.".to_owned(),
        1 => "Your browser can't open now: this computer doesn't have enough free memory for it beside another agent's browser. Try again in a few minutes: a browser closes by itself after 10 minutes unused.".to_owned(),
        open => format!(
            "Your browser can't open now: this computer doesn't have enough free memory for it beside the {open} other agents' browsers open. Try again in a few minutes: a browser closes by itself after 10 minutes unused."
        ),
    })
}

/// This computer's memory, in all and available now, in bytes.
fn measured() -> Option<(u64, u64)> {
    let mut system = sysinfo::System::new().ok()?;
    system.refresh_memory();
    let (total, available) = (system.total_memory(), system.available_memory());
    (total > 0 && available > 0).then_some((total, available))
}

#[cfg(test)]
mod tests {
    use super::{PER_BROWSER, fits};

    const GIB: u64 = 1024 * 1024 * 1024;

    /// Several browsers fit, but never more than half the memory
    /// or past what's free; the first always may when there's room.
    #[test]
    fn a_browser_starts_only_while_the_computer_has_room() {
        // 16 GB: four at most, each needing 3 GB free.
        assert!(fits(0, 16 * GIB, 8 * GIB).is_ok());
        assert!(fits(3, 16 * GIB, 8 * GIB).is_ok());
        let full = fits(4, 16 * GIB, 8 * GIB).unwrap_err();
        assert!(full.contains("the 4 other agents' browsers"), "{full}");
        let short = fits(1, 16 * GIB, 2 * GIB).unwrap_err();
        assert!(short.contains("beside another agent's browser"), "{short}");
        // A small computer still runs one, when it's free.
        assert!(fits(0, 3 * GIB, PER_BROWSER + GIB).is_ok());
        assert!(fits(1, 3 * GIB, 8 * GIB).is_err());
        assert!(fits(0, 3 * GIB, PER_BROWSER).unwrap_err().contains("doesn't have enough free memory for it."));
    }
}
