//! Resuming the conversation shown on reconnect.
//!
//! Every device shows the same agent (the selected one), and its app holds
//! that agent's latest entries. Each whole Transcript sent, and each change
//! to an entry of the agent it's for, carries a revision
//! (ServerMessage.transcript_revision). An app that comes back with the
//! agent and the revision it last applied (Hello.transcript_agent_id and
//! transcript_revision) gets only the entries added or changed since, when
//! that agent is still the one shown and this log can tell; otherwise the
//! latest page, as before.
//!
//! The log follows one agent's conversation at a time: the last one sent
//! whole. It's kept with the Factory's lock held (every change and every
//! whole Transcript is), so a snapshot's revision matches its entries. In
//! memory only: another process's revision never matches.

use std::collections::BTreeMap;
use std::sync::Mutex;

/// The most entries a resume sends: more have changed, the latest page goes
/// instead.
const MOST: usize = 200;
/// The most changed entries followed: past it the log starts afresh, and
/// apps that held an older revision get the latest page.
const FOLLOWED: usize = 4096;

pub(crate) struct Log {
    /// This process's: no other's revision ever matches.
    nonce: u64,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Whose conversation it follows.
    agent: Option<String>,
    /// The latest revision given.
    revision: u64,
    /// The revision it follows changes from: an app holding an older one
    /// gets the latest page.
    base: u64,
    /// The entries changed since `base`, by seq: their last change's revision.
    changed: BTreeMap<u64, u64>,
}

/// What an app gets on reconnect, when it can resume.
pub(crate) struct Resumed {
    /// The entries added or changed since the revision it held, oldest first.
    pub seqs: Vec<u64>,
    /// The revision that brings it to.
    pub revision: String,
}

impl Default for Log {
    fn default() -> Self {
        let mut nonce = [0u8; 8];
        let _ = getrandom::fill(&mut nonce);
        Self { nonce: u64::from_le_bytes(nonce), state: Mutex::default() }
    }
}

impl Log {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn token(&self, revision: u64) -> String {
        format!("{:016x}-{revision}", self.nonce)
    }

    /// `agent`'s conversation was sent whole: the log follows it from here.
    /// The revision that Transcript brings apps to.
    pub(crate) fn whole(&self, agent: &str) -> String {
        let mut state = self.state();
        state.revision += 1;
        state.agent = Some(agent.to_owned());
        state.base = state.revision;
        state.changed.clear();
        self.token(state.revision)
    }

    /// The revision of `agent`'s conversation as it is now, for a snapshot
    /// of it: the log's own when it follows `agent`, else it follows it from
    /// here.
    pub(crate) fn now(&self, agent: &str) -> String {
        let state = self.state();
        if state.agent.as_deref() == Some(agent) {
            return self.token(state.revision);
        }
        drop(state);
        self.whole(agent)
    }

    /// `agent`'s entry `seq` was added or changed: the revision it brings
    /// apps to, when the log follows `agent`.
    pub(crate) fn changed(&self, agent: &str, seq: u64) -> Option<String> {
        let mut state = self.state();
        if state.agent.as_deref() != Some(agent) {
            return None;
        }
        state.revision += 1;
        let revision = state.revision;
        state.changed.insert(seq, revision);
        if state.changed.len() > FOLLOWED {
            state.base = revision;
            state.changed.clear();
        }
        Some(self.token(revision))
    }

    /// What an app holding `agent`'s conversation at `held` (Hello's) needs
    /// to catch up; None when the log can't tell (another process's
    /// revision, another agent, older than the log, or too many changes).
    pub(crate) fn since(&self, agent: &str, held: &str) -> Option<Resumed> {
        let (nonce, revision) = held.split_once('-')?;
        let revision: u64 = revision.parse().ok()?;
        if u64::from_str_radix(nonce, 16).ok()? != self.nonce || agent.is_empty() {
            return None;
        }
        let state = self.state();
        if state.agent.as_deref() != Some(agent) || revision < state.base || revision > state.revision {
            return None;
        }
        let seqs: Vec<u64> = state.changed.iter().filter(|(_, changed)| **changed > revision).map(|(seq, _)| *seq).collect();
        if seqs.len() > MOST {
            return None;
        }
        Some(Resumed { seqs, revision: self.token(state.revision) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_app_catches_up_on_what_changed_since_its_revision_or_gets_the_page() {
        let log = Log::default();
        assert!(log.changed("sam", 0).is_none(), "follows no one yet");
        let page = log.whole("sam");
        let first = log.changed("sam", 7).expect("followed");
        log.changed("sam", 8);
        let latest = log.changed("sam", 7).expect("followed");
        assert!(log.changed("ana", 3).is_none(), "another agent's isn't followed");

        // From the page: 7 and 8, once each, oldest first.
        let resumed = log.since("sam", &page).expect("resumes");
        assert_eq!((resumed.seqs, resumed.revision.as_str()), (vec![7, 8], latest.as_str()));
        // From the first change: 8 changed since, and 7 again.
        assert_eq!(log.since("sam", &first).expect("resumes").seqs, [7, 8]);
        // Up to date: nothing, the same revision.
        let resumed = log.since("sam", &latest).expect("resumes");
        assert_eq!((resumed.seqs.len(), resumed.revision), (0, latest.clone()));
        assert_eq!(log.now("sam"), latest);

        // Can't tell: another agent, another process's, a revision it never
        // gave, nonsense, or from before the log started afresh.
        assert!(log.since("ana", &latest).is_none());
        assert!(Log::default().since("sam", &latest).is_none());
        let (nonce, _) = latest.split_once('-').expect("token");
        assert!(log.since("sam", &format!("{nonce}-999")).is_none());
        assert!(log.since("sam", "").is_none() && log.since("sam", "x-1").is_none());
        let again = log.whole("ana");
        assert!(log.since("sam", &latest).is_none(), "follows Ana now");
        assert!(log.since("ana", &latest).is_none(), "older than Ana's page");
        assert_eq!(log.since("ana", &again).expect("resumes").seqs.len(), 0);
        assert_ne!(log.now("sam"), latest, "a snapshot of Sam's follows Sam afresh");

        // Too many changed: the page instead, and past what's followed the
        // log starts afresh.
        let page = log.whole("sam");
        for seq in 0..=MOST as u64 {
            log.changed("sam", seq);
        }
        assert!(log.since("sam", &page).is_none(), "more than a resume sends");
        for seq in 0..=FOLLOWED as u64 {
            log.changed("sam", seq);
        }
        assert!(log.since("sam", &page).is_none(), "older than the log");
    }
}
