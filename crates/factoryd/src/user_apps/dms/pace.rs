//! Pacing for the direct-message loops' long polls (`process`, `watch`,
//! `sends`). The worker holds a Claktory's long polls only while it has
//! places for them (4 at once), and a poll this side leaves behind (woken
//! mid-wait, to look again now) keeps its place until its wait is over. A
//! poll without a place is answered at once with nothing, so a loop that
//! asked again straight away would spin until the account service refused
//! this Claktory altogether for a minute, agents' direct-message tools
//! included. A loop whose poll came back with nothing well before its wait
//! pauses before the next instead: 2 seconds, doubling up to the wait,
//! until one is held again.

use std::time::Duration;

/// The first pause after a poll that wasn't held.
const FIRST: Duration = Duration::from_secs(2);

/// Whether a long poll that waited up to `wait` and answered `nothing`
/// after `took` came back early: the worker didn't hold it.
pub(super) fn early(wait: Duration, took: Duration, nothing: bool) -> bool {
    nothing && took < wait / 2
}

/// One loop's pause after polls that weren't held.
#[derive(Default)]
pub(super) struct Pace {
    pause: Duration,
}

impl Pace {
    /// After a poll (or a round of them, for a loop that asks several), and
    /// whether it came back `early`: how long to pause before the next, at
    /// most `most`. A poll that was held starts the pauses over.
    pub(super) fn after(&mut self, early: bool, most: Duration) -> Option<Duration> {
        if !early {
            self.pause = Duration::ZERO;
            return None;
        }
        self.pause = (self.pause * 2).clamp(FIRST, most.max(FIRST));
        Some(self.pause)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAIT: Duration = Duration::from_secs(25);

    #[test]
    fn only_nothing_well_before_the_wait_is_early() {
        assert!(early(WAIT, Duration::from_millis(2), true));
        // Something to answer comes back at once, as it should.
        assert!(!early(WAIT, Duration::from_millis(2), false));
        // Held to (nearly) the end of its wait.
        assert!(!early(WAIT, Duration::from_secs(25), true));
        assert!(!early(WAIT, Duration::from_millis(12_500), true));
    }

    #[test]
    fn polls_not_held_pause_longer_each_time_up_to_the_wait_and_one_held_starts_over() {
        let mut pace = Pace::default();
        let pauses: Vec<u64> = (0..7).map(|_| pace.after(true, WAIT).expect("a pause").as_secs()).collect();
        assert_eq!(pauses, [2, 4, 8, 16, 25, 25, 25]);
        assert_eq!(pace.after(false, WAIT), None);
        assert_eq!(pace.after(true, WAIT), Some(FIRST));
        // A loop whose polls wait less pauses no longer than they do, and
        // never less than the first pause.
        let mut short = Pace::default();
        assert_eq!(short.after(true, Duration::from_secs(1)), Some(FIRST));
        assert_eq!(short.after(true, Duration::from_secs(3)), Some(Duration::from_secs(3)));
    }
}
