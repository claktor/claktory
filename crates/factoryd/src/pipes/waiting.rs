//! A pipe's liaison waits (#343 U18, Fugo's ruling): while this side's lead
//! waits out a usage limit (or credit, or a sign-in), or its daily spending
//! cap holds its pipes' messages until midnight (#357 L2), the other side is
//! told it's waiting, and until when when that's known, and told again
//! when it's awake; what the other side says of its own shows on this
//! side's pipe, and as a line in it, for the person only.
//!
//! Only the time goes, never why: no provider, model, plan, account or
//! amount, and the time rounded up to a quarter of an hour, so the other
//! side can't read the plan's reset schedule from it. It's said when it
//! changes, at most once a minute per pipe, only while the pipe is open; a
//! Claktory from before doesn't take it, and isn't asked again. What the
//! other side says is checked by the transport, and shown in this side's
//! own sentence with this side's name for the pipe: no agent is told of
//! it, and it counts toward no limit.

use std::sync::{Arc, Weak};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{Message, Record, State, add, change};
use crate::factory::Factory;
use crate::pipe_link::{self, Liaison};
use crate::transcript::now_ms;

/// What this side last told the other of its liaison, and when it was heard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Told {
    pub waiting: bool,
    /// ms since 1970, a quarter of an hour; 0 when there's no known end.
    pub until_ms: i64,
    /// When the other side heard it, in ms since 1970.
    pub at_ms: i64,
}

/// The id of the pipe's lines about the other side's liaison: never read by
/// an agent (`agents::shown`), only shown to the person.
pub(crate) const LINE_ID: &str = "claktory:peer-liaison";

/// How often this side looks whether its liaisons' waits changed.
const LOOK_EVERY: Duration = Duration::from_secs(15);

/// A quarter of an hour, in ms: what a wait's end is rounded up to.
const QUARTER_MS: i64 = 15 * 60 * 1000;

/// `until_ms` rounded up to the next quarter of an hour (itself when it's
/// on one); 0 stays 0.
pub(crate) fn rounded(until_ms: i64) -> i64 {
    if until_ms <= 0 {
        return 0;
    }
    let over = until_ms % QUARTER_MS;
    if over == 0 { until_ms } else { until_ms.saturating_add(QUARTER_MS - over) }
}

/// What the other side should hear of this pipe's liaison now: waiting
/// while its lead waits out a usage limit (or credit, or a sign-in), or is
/// past its daily spending cap (#357 L2: its pipes' messages wait until
/// midnight); else awake. Both at once, the later end: none when either has
/// no known end.
fn wanted(factory: &Factory, record: &Record) -> Liaison {
    let (lead, usage) = {
        let inner = factory.lock();
        let lead = super::lead::effective(&inner.config, record);
        let usage = lead.as_ref().and_then(|lead| inner.agents.get(lead)?.usage_wait.as_ref().map(|wait| wait.until_ms));
        (lead, usage)
    };
    let capped = lead.and_then(|lead| super::spend::of(factory, &lead)).filter(super::spend::Spend::over).map(|spend| spend.resets_ms);
    let until = match (usage, capped) {
        (None, None) => return Liaison::Awake,
        (Some(0), _) | (_, Some(0)) => 0,
        (usage, capped) => usage.max(capped).unwrap_or_default(),
    };
    Liaison::Waiting { until_ms: rounded(until) }
}

/// What `told` said, as a `Liaison`: none told yet is awake.
fn said(told: Option<Told>) -> Liaison {
    match told {
        Some(told) if told.waiting => Liaison::Waiting { until_ms: told.until_ms },
        _ => Liaison::Awake,
    }
}

/// Tells each open pipe's other side of its liaison, where that changed
/// since it was last told and a minute has passed since. The transport
/// says it once the pipe flows and the other side can be reached.
pub(crate) fn tell(factory: &Arc<Factory>) {
    for record in factory.pipes.all() {
        if let Some(liaison) = due(factory, &record) {
            pipe_link::tell_liaison(factory, &record.id, liaison);
        }
    }
}

/// What the other side of `record` is to be told of its liaison now: None
/// when it isn't open, nothing changed since it was told, or it was told
/// less than a minute ago.
pub(crate) fn due(factory: &Factory, record: &Record) -> Option<Liaison> {
    if record.state != State::Open {
        return None;
    }
    let wanted = wanted(factory, record);
    if wanted == said(record.told_liaison) {
        return None;
    }
    let every = i64::try_from(pipe_link::liaison_every().as_millis()).unwrap_or(i64::MAX);
    if record.told_liaison.is_some_and(|told| now_ms() - told.at_ms < every) {
        return None;
    }
    Some(wanted)
}

/// Looks every `LOOK_EVERY` for as long as Claktory runs.
pub(crate) async fn watch(factory: Weak<Factory>) {
    loop {
        tokio::time::sleep(LOOK_EVERY).await;
        let Some(factory) = factory.upgrade() else { return };
        tell(&factory);
    }
}

/// The other side heard what this side told it of its liaison.
pub(crate) fn told(factory: &Factory, pipe: &str, liaison: Liaison) {
    let (waiting, until_ms) = match liaison {
        Liaison::Waiting { until_ms } => (true, until_ms),
        Liaison::Awake => (false, 0),
    };
    change(factory, pipe, |record| record.told_liaison = Some(Told { waiting, until_ms, at_ms: now_ms() }));
}

/// The other side says its liaison waits (until when, checked by the
/// transport), or is awake: kept, shown on the pipe and as a line in it,
/// in this side's words. Only on an open pipe; nothing when it's the same
/// as before.
pub(crate) fn heard(factory: &Factory, pipe: &str, liaison: Liaison) {
    let (waiting, until_ms) = match liaison {
        Liaison::Waiting { until_ms } => (true, until_ms),
        Liaison::Awake => (false, 0),
    };
    let changed = change(factory, pipe, |record| {
        if record.state != State::Open || (record.peer_waiting, record.peer_waiting_until) == (waiting, until_ms) {
            return None;
        }
        record.peer_waiting = waiting;
        record.peer_waiting_until = until_ms;
        Some(record.name.clone())
    })
    .flatten();
    let Some(name) = changed else { return };
    let text = if !waiting {
        format!("{name} isn't waiting any more.")
    } else {
        match factory_agent::accounts::local_time(u64::try_from(until_ms).unwrap_or_default()).filter(|_| until_ms > 0) {
            Some(at) => format!("{name} is waiting until {at}."),
            None => format!("{name} is waiting."),
        }
    };
    add(factory, pipe, Message { msg_id: LINE_ID.to_owned(), text, ..Message::default() });
    factory.broadcast_workspace(&factory.lock());
}

/// What the pipe shows of the other side's liaison: waiting, and until
/// when (0: no known end). Only while it's open.
pub(crate) fn shown(record: &Record) -> (bool, i64) {
    if record.state == State::Open && record.peer_waiting { (true, record.peer_waiting_until) } else { (false, 0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wait_s_end_is_rounded_up_to_a_quarter_of_an_hour() {
        assert_eq!(rounded(0), 0);
        assert_eq!(rounded(QUARTER_MS * 4), QUARTER_MS * 4);
        assert_eq!(rounded(QUARTER_MS * 4 + 1), QUARTER_MS * 5);
        assert_eq!(rounded(QUARTER_MS * 5 - 1), QUARTER_MS * 5);
    }
}
