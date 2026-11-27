//! Checks that the visitor is a person, which an errand can't pass (W5;
//! Matt's card 222, Melone R12.4). The person never drives the browser, so
//! there's no handoff: Claktory tries the page again about 2, 15 and 60
//! minutes later, and the errand's steps wait meanwhile. When the check
//! clears, the agent is told to read the page; after the last try, the
//! errand fails, said in its log, and the agent is told to report it.
//!
//! Kept in memory: after Claktory restarts, the agent's next step meets
//! the check again and the tries start over.

use std::sync::Arc;
use std::time::Duration;

use factory_protocol::v1;

use super::{State, fence, log};
use crate::factory::{Factory, host_event};
use crate::transcript::now_ms;

/// When to try again after each try: the first, about 2 minutes after the
/// check showed; the last, an hour after the one before.
const AFTER: [Duration; 3] = [Duration::from_secs(2 * 60), Duration::from_secs(15 * 60), Duration::from_secs(60 * 60)];

/// How long a check gets to clear by itself on arrival, and when tried
/// again.
pub(super) const CLEARS: Duration = Duration::from_secs(10);

/// An errand waiting to try a page again.
#[derive(Clone, Debug)]
pub(crate) struct Waiting {
    errand: String,
    agent: String,
    origin: String,
    /// Where the check showed: the page tried again (an https address of
    /// the errand's).
    address: String,
    /// The tries made so far.
    tries: usize,
    /// When the next is, ms.
    at: i64,
}

/// How long tests wait instead: before each try, and for a check to clear.
#[derive(Clone, Copy)]
pub(crate) struct Timing {
    pub after: [Duration; 3],
    pub clears: Duration,
}

impl Default for Timing {
    fn default() -> Timing {
        Timing { after: AFTER, clears: CLEARS }
    }
}

/// The site, or "the page" when there's none.
fn site(origin: &str) -> &str {
    if origin.is_empty() { "the page" } else { origin }
}

fn at(ms: i64) -> String {
    crate::transcript::local_time(ms, "%H:%M")
}

impl Factory {
    /// The errand's wait, if it's waiting to try a check's page again:
    /// what its steps are told.
    pub(crate) fn check_waiting(&self, errand: &str) -> Option<String> {
        let waiting = crate::locking::lock(&self.errands.checking).clone().filter(|waiting| waiting.errand == errand)?;
        Some(format!(
            "Claktory tries {} again at {} for its check that you're a person: wait until you're told how it went",
            site(&waiting.origin),
            at(waiting.at)
        ))
    }

    /// A check on `origin` (at `address`) didn't clear by itself: the
    /// errand waits and tries again later. What the agent is told.
    pub(crate) fn check_wait(self: &Arc<Self>, errand: &v1::Errand, agent: &str, origin: &str, address: &str) -> String {
        let timing = *crate::locking::lock(&self.errands.check_timing);
        let next = now_ms() + timing.after[0].as_millis() as i64;
        let waiting = Waiting { errand: errand.id.clone(), agent: agent.to_owned(), origin: origin.to_owned(), address: address.to_owned(), tries: 0, at: next };
        log::write(
            &self.store,
            &log::Entry { errand: errand.id.clone(), agent: agent.to_owned(), origin: origin.to_owned(), action: "A check that you're a person showed".to_owned(), ..Default::default() },
        );
        self.wait_to_try(waiting, timing.after[0]);
        let minutes = timing.after[0].as_secs().div_ceil(60);
        format!(
            "{} put up a check that you're a person, which Claktory's browser can't pass and no one can do for it. Claktory tries the page again at {} (in about {minutes} minute{}), later again if it must, and tells you how it went. Until then the errand's steps wait: do nothing else for it.",
            capitalized(site(origin)),
            at(next),
            if minutes == 1 { "" } else { "s" }
        )
    }

    fn wait_to_try(self: &Arc<Self>, waiting: Waiting, after: Duration) {
        *crate::locking::lock(&self.errands.checking) = Some(waiting.clone());
        let factory = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            factory.try_again(waiting).await;
        });
    }

    /// One try: the page again, and whether its check clears.
    async fn try_again(self: &Arc<Self>, waiting: Waiting) {
        let current = crate::locking::lock(&self.errands.checking).clone();
        if !current.is_some_and(|current| current.errand == waiting.errand && current.tries == waiting.tries) {
            return;
        }
        let errand = self.errands.get(&waiting.errand).filter(|errand| State::from_wire(errand.state) == State::Running);
        let Some(errand) = errand else {
            *crate::locking::lock(&self.errands.checking) = None;
            return;
        };
        let timing = *crate::locking::lock(&self.errands.check_timing);
        let cleared = self.check_cleared(&errand, &waiting.address, timing.clears).await;
        let tries = waiting.tries + 1;
        let entry = |action: &str| log::Entry {
            errand: errand.id.clone(),
            agent: waiting.agent.clone(),
            origin: waiting.origin.clone(),
            action: action.to_owned(),
            ..Default::default()
        };
        if cleared {
            *crate::locking::lock(&self.errands.checking) = None;
            log::write(&self.store, &entry("The check that you're a person cleared when Claktory tried again"));
            let text = format!(
                "{}'s check that you're a person cleared when Claktory tried the page again: read it with errand_read and carry on.",
                capitalized(site(&waiting.origin))
            );
            self.deliver_event(&waiting.agent, text, host_event(format!("Errand {}: the check cleared", errand.id)));
            return;
        }
        if tries < timing.after.len() {
            log::write(&self.store, &entry("The check that you're a person was still there when Claktory tried again"));
            let after = timing.after[tries];
            self.wait_to_try(Waiting { tries, at: now_ms() + after.as_millis() as i64, ..waiting }, after);
            return;
        }
        *crate::locking::lock(&self.errands.checking) = None;
        log::write(&self.store, &entry("Gave up: the check that you're a person didn't clear"));
        let reason = format!("{} kept showing a check that you're a person, which Claktory's browser can't pass.", capitalized(site(&waiting.origin)));
        self.end_errand(&errand.id, State::Failed, &reason).await;
        let text = format!(
            "{reason} Claktory tried the page {tries} more times and the check was still there, so errand {} failed. Tell your lead, or whoever opened it, what was done and what wasn't.",
            errand.id
        );
        self.deliver_event(&waiting.agent, text, host_event(format!("Errand {} failed", errand.id)));
    }

    /// The page at `address` again (through the fence), and whether its
    /// check clears within `clears`. In the errand's lane, so no step of
    /// its runs meanwhile.
    async fn check_cleared(self: &Arc<Self>, errand: &v1::Errand, address: &str, clears: Duration) -> bool {
        let _lane = self.errands.lane.lock().await;
        let mut live = self.errands.live.lock().await;
        let Ok(page) = self.page(&mut live, errand).await else { return false };
        drop(live);
        if address.starts_with("https://") {
            let _ = fence::go(&page, &errand.sites, address).await;
        }
        let until = std::time::Instant::now() + clears;
        loop {
            if page.check().await.is_none() {
                return true;
            }
            if std::time::Instant::now() >= until {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// Tests: tries again after `after` each, a check given `clears`.
#[doc(hidden)]
pub fn check_timing_for_test(factory: &Factory, after: [Duration; 3], clears: Duration) {
    *crate::locking::lock(&factory.errands.check_timing) = Timing { after, clears };
}
