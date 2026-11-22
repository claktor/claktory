//! Rate limits, in memory: fixed windows per key. A restart forgets them,
//! which costs at most one window's worth.

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub(crate) struct Limits {
    /// By key: the window's start (unix seconds) and the count in it.
    windows: Mutex<HashMap<String, (i64, u32)>>,
}

impl Limits {
    /// Counts one against every `(key, window seconds, most)`, or refuses
    /// with the seconds until it may try again (counting nothing).
    pub(crate) fn take(&self, limits: &[(&str, i64, u32)], now: i64) -> Result<(), i64> {
        let mut windows = self.windows.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut wait = 0;
        for (key, window, most) in limits {
            let (start, count) = windows.get(*key).copied().unwrap_or((now, 0));
            if now - start < *window && count >= *most {
                wait = wait.max(start + window - now);
            }
        }
        if wait > 0 {
            return Err(wait);
        }
        for (key, window, _) in limits {
            let entry = windows.entry((*key).to_owned()).or_insert((now, 0));
            if now - entry.0 >= *window {
                *entry = (now, 0);
            }
            entry.1 += 1;
        }
        Ok(())
    }

    /// Forgets windows over for at least a day.
    pub(crate) fn purge(&self, now: i64) {
        let mut windows = self.windows.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        windows.retain(|_, (start, _)| now - *start < 24 * 3600);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_refuses_past_its_count_until_it_ends() {
        let limits = Limits::default();
        for _ in 0..3 {
            assert_eq!(limits.take(&[("a", 60, 3), ("all", 60, 100)], 1000), Ok(()));
        }
        assert_eq!(limits.take(&[("a", 60, 3)], 1010), Err(50));
        // A refusal counts nothing against the other limits.
        assert_eq!(limits.take(&[("b", 60, 1), ("a", 60, 3)], 1010), Err(50));
        assert_eq!(limits.take(&[("b", 60, 1)], 1010), Ok(()));
        assert_eq!(limits.take(&[("a", 60, 3)], 1060), Ok(()));
    }
}
