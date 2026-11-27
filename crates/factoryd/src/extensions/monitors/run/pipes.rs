//! Pipes monitors: fire when pipes their agent may see
//! change: opened, closed, paused, resumed, a website shared over one or
//! stopped, the other side coming online or going offline.
//!
//! An admin agent hears of every pipe; another agent only of its own
//! project's, as it stands at each look. What a fire says is this side's
//! own: the pipe's id, the name the person gave it (never one the other
//! side chose), its project, who paused it, and this side's websites by
//! the names the person gave them. Never what the other side says about
//! itself or calls its websites.
//!
//! Polled, like files: each look compares the pipes with how the monitor
//! last saw them, kept with it (`Cursor::pipes`) so what changed while
//! Factory was down fires once it's back. The other side's being online
//! fires only once it has held for `SETTLE`, so a link that drops and
//! comes back at once fires nothing.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::when::{MINUTE, PipeChange};
use super::{Agent, Monitor, SECOND, Status, Trigger, end, save, trigger};
use crate::factory::Factory;
use crate::pipes::{Pause, Record, State};

/// How often a pipes monitor looks.
const EVERY: i64 = 2 * SECOND;
/// How long the other side must stay online, or offline, before it fires.
const SETTLE: i64 = MINUTE;

/// A pipe as a pipes monitor last saw it: this side's facts only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct PipeSeen {
    state: State,
    #[serde(default)]
    paused: Pause,
    /// Whether the other side is online, while the pipe is open or paused
    /// and that's known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    online: Option<bool>,
    /// This side's websites shared over it: their names, by id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    mine: BTreeMap<String, String>,
    /// The other side's, by this side's id for them.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    theirs: BTreeSet<String>,
}

/// Whether an agent (an admin one, or one in `project`) may hear of `pipe`.
pub(crate) fn sees_pipe(admin: bool, project: Option<&str>, pipe: &Record) -> bool {
    admin || project == Some(pipe.project.as_str())
}

/// The pipes the monitor watches now: the ones it names (all, if none)
/// that its agent may see.
fn watched(factory: &Factory, admin: bool, project: Option<&str>, named: &[String]) -> Vec<Record> {
    factory
        .pipes
        .all()
        .into_iter()
        .filter(|record| sees_pipe(admin, project, record) && (named.is_empty() || named.contains(&record.id)))
        .collect()
}

/// Whether the other side is online now, as far as this side knows: only
/// while the pipe is open or paused, and not before it's been tried.
fn online(factory: &Factory, record: &Record) -> Option<bool> {
    if !matches!(record.state, State::Open | State::Paused) {
        return None;
    }
    let link = crate::pipe_link::status(factory, &record.id);
    if link.online {
        Some(true)
    } else if link.offline_since_ms != 0 {
        Some(false)
    } else {
        None
    }
}

fn seen(record: &Record, shared: &mut HashMap<String, crate::pipes::shares::OnPipe>, online: Option<bool>) -> PipeSeen {
    let on_pipe = shared.remove(&record.id).unwrap_or_default();
    PipeSeen { state: record.state, paused: record.paused, online, mine: on_pipe.mine, theirs: on_pipe.theirs }
}

/// A new pipes monitor notes the pipes as they are now, so what changes
/// from here on fires it.
pub(super) fn baseline(factory: &Factory, mut monitor: Monitor, named: &[String]) {
    let (admin, project) = {
        let inner = factory.lock();
        let Some(config) = inner.config.agent(&monitor.agent) else { return };
        (crate::proposals::is_admin(config), config.project.clone())
    };
    let mut shared = crate::pipes::shares::by_pipe(factory);
    let pipes = watched(factory, admin, project.as_deref(), named)
        .iter()
        .map(|record| (record.id.clone(), seen(record, &mut shared, online(factory, record))))
        .collect();
    let now = factory.monitors.now();
    monitor.cursor.pipes = Some(pipes);
    monitor.cursor.pipes_seen_at = Some(now);
    if let Err(error) = save(&factory.store, &monitor, now, "", None, true) {
        tracing::error!(%error, "could not note a pipes monitor's pipes");
    }
}

/// One change, said in a line.
struct Change {
    kind: PipeChange,
    line: String,
}

pub(super) fn poll(
    factory: &Arc<Factory>,
    monitor: &mut Monitor,
    agent: &Agent,
    named: &[String],
    on: &[PipeChange],
    now: i64,
) -> rusqlite::Result<()> {
    let key = (monitor.agent.clone(), monitor.id);
    {
        let mut live = factory.monitors.live();
        let entry = live.entry(key.clone()).or_default();
        if now - entry.last_poll < EVERY {
            return Ok(());
        }
        entry.last_poll = now;
    }
    if !named.is_empty() && named.iter().all(|id| factory.pipes.get(id).is_none()) {
        let reason = if named.len() == 1 { "its pipe was deleted" } else { "its pipes were deleted" };
        return end(factory, monitor, Status::Done, reason, now);
    }
    let records = watched(factory, agent.admin, agent.project.as_deref(), named);
    let mut shared = crate::pipes::shares::by_pipe(factory);
    let Some(before) = monitor.cursor.pipes.clone() else {
        // From before it was noted (Factory stopped at once): from now on.
        let pipes = records.iter().map(|record| (record.id.clone(), seen(record, &mut shared, online(factory, record)))).collect();
        monitor.cursor.pipes = Some(pipes);
        monitor.cursor.pipes_seen_at = Some(now);
        return save(&factory.store, monitor, now, "", None, true).map(drop);
    };
    let since = monitor.cursor.pipes_seen_at.unwrap_or(0);
    let (projects, person) = {
        let inner = factory.lock();
        let projects: HashMap<String, String> =
            inner.config.projects.iter().map(|project| (project.id.clone(), project.name.clone())).collect();
        (projects, inner.config.person().to_owned())
    };
    let mut after = BTreeMap::new();
    let mut changes = Vec::new();
    {
        let mut live = factory.monitors.live();
        let settling = &mut live.entry(key).or_default().pipes_settling;
        settling.retain(|id, _| records.iter().any(|record| &record.id == id));
        for record in &records {
            let old = before.get(&record.id);
            let called = called(record, projects.get(&record.project).map_or("its project", String::as_str));
            let kept_online = if matches!(record.state, State::Open | State::Paused) {
                let mut pending = settling.remove(&record.id);
                let (kept, changed) = settle(old.and_then(|old| old.online), online(factory, record), &mut pending, now);
                if let Some(pending) = pending {
                    settling.insert(record.id.clone(), pending);
                }
                match changed {
                    Some(true) => changes.push(Change { kind: PipeChange::Online, line: format!("The other side of {called} came online.") }),
                    Some(false) => changes.push(Change { kind: PipeChange::Offline, line: format!("The other side of {called} went offline.") }),
                    None => {}
                }
                kept
            } else {
                settling.remove(&record.id);
                None
            };
            let seen = seen(record, &mut shared, kept_online);
            match old {
                // New here: open already only if it was made since the last look.
                None if record.state == State::Open && record.created > since => {
                    changes.push(Change { kind: PipeChange::Opened, line: format!("{} opened: both people approved it.", capital(&called)) });
                }
                None => {}
                Some(old) => {
                    changes.extend(state_change(old, &seen, &called, &person));
                    if seen.state != State::Closed {
                        changes.extend(share_changes(old, &seen, &called, &person));
                    }
                }
            }
            after.insert(record.id.clone(), seen);
        }
    }
    if after == before {
        return Ok(());
    }
    monitor.cursor.pipes = Some(after);
    monitor.cursor.pipes_seen_at = Some(now);
    let changes: Vec<Change> = changes.into_iter().filter(|change| on.is_empty() || on.contains(&change.kind)).collect();
    if changes.is_empty() {
        return save(&factory.store, monitor, now, "", None, true).map(drop);
    }
    let cause = match changes.as_slice() {
        [one] => format!("pipe {}", one.kind.name()),
        many => format!("{} pipe changes", many.len()),
    };
    let what = changes.iter().map(|change| change.line.as_str()).collect::<Vec<_>>().join("\n");
    trigger(factory, monitor, Trigger { cause, what, late: None, last: false, by: String::new() }, now)
}

/// Whether the other side is online, as the monitor keeps it: `was`, as
/// it last saw it; `is`, as known now (None: not known yet). A change
/// waits in `pending` (what, since when) until it has held for `SETTLE`.
/// What to keep, and the change to fire once it held.
fn settle(was: Option<bool>, is: Option<bool>, pending: &mut Option<(bool, i64)>, now: i64) -> (Option<bool>, Option<bool>) {
    match (was, is) {
        // Not known now: as it was.
        (was, None) => (was, None),
        // Known for the first time: where it stands, not a change.
        (None, is) => {
            *pending = None;
            (is, None)
        }
        (Some(was), Some(is)) if was == is => {
            *pending = None;
            (Some(was), None)
        }
        (Some(was), Some(is)) => match *pending {
            Some((waiting, since)) if waiting == is && now - since >= SETTLE => {
                *pending = None;
                (Some(is), Some(is))
            }
            Some((waiting, _)) if waiting == is => (Some(was), None),
            _ => {
                *pending = Some((is, now));
                (Some(was), None)
            }
        },
    }
}

/// "the pipe "Launch" (id 3f2a…, in Site)": the name only when the person
/// gave it, never one the other side chose.
fn called(record: &Record, project: &str) -> String {
    let named_here = record.named_here.unwrap_or(record.peer.is_none()) && !record.name.trim().is_empty();
    if named_here {
        format!("the pipe \"{}\" (id {}, in {})", crate::pipes::agents::escape(&record.name), record.id, crate::pipes::agents::escape(project))
    } else {
        format!("the pipe {} (named by the other side, in {})", record.id, crate::pipes::agents::escape(project))
    }
}

fn capital(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// Opened, closed, paused or resumed, from `old` to `new`.
fn state_change(old: &PipeSeen, new: &PipeSeen, called: &str, person: &str) -> Option<Change> {
    if old.state == new.state {
        return None;
    }
    let called = capital(called);
    Some(match new.state {
        State::Open if old.state == State::Paused => Change { kind: PipeChange::Resumed, line: format!("{called} was resumed: it's open again.") },
        State::Open => Change { kind: PipeChange::Opened, line: format!("{called} opened: both people approved it.") },
        State::Paused => {
            let line = match new.paused {
                Pause::Me => format!("{called} was paused by {person}."),
                Pause::Peer => format!("{called} was paused by the other side."),
                Pause::Loop => format!("{called} was paused: its agents talked on their own; {person} can continue it."),
                Pause::None => format!("{called} was paused."),
            };
            Change { kind: PipeChange::Paused, line }
        }
        State::Closed => Change { kind: PipeChange::Closed, line: format!("{called} was closed.") },
        State::Invited | State::Joining | State::Approval | State::WaitingPeer => return None,
    })
}

/// Websites shared over the pipe, or no longer: this side's by the names
/// the person gave them, the other side's only counted.
fn share_changes(old: &PipeSeen, new: &PipeSeen, called: &str, person: &str) -> Vec<Change> {
    let website = |name: &str| format!("\"{}\"", crate::pipes::agents::escape(name));
    let mut changes = Vec::new();
    for (id, name) in &new.mine {
        if !old.mine.contains_key(id) {
            let line = format!("{person}'s website {} is now shared over {called}.", website(name));
            changes.push(Change { kind: PipeChange::Shared, line });
        }
    }
    for (id, name) in &old.mine {
        if !new.mine.contains_key(id) {
            let line = format!("{person}'s website {} is no longer shared over {called}.", website(name));
            changes.push(Change { kind: PipeChange::Unshared, line });
        }
    }
    let websites = |count: usize| if count == 1 { "a website".to_owned() } else { format!("{count} websites") };
    let came = new.theirs.difference(&old.theirs).count();
    if came > 0 {
        changes.push(Change { kind: PipeChange::Shared, line: format!("The other side shared {} over {called}.", websites(came)) });
    }
    let went = old.theirs.difference(&new.theirs).count();
    if went > 0 {
        changes.push(Change { kind: PipeChange::Unshared, line: format!("The other side stopped sharing {} over {called}.", websites(went)) });
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pipe(state: State, paused: Pause) -> PipeSeen {
        PipeSeen { state, paused, online: None, mine: BTreeMap::new(), theirs: BTreeSet::new() }
    }

    #[test]
    fn states_say_what_happened_in_this_sides_words() {
        let called = "the pipe \"Launch\" (id p1, in Site)";
        let said = |old: State, new: State, paused: Pause| {
            state_change(&pipe(old, Pause::None), &pipe(new, paused), called, "Matt").map(|change| (change.kind, change.line))
        };
        assert_eq!(
            said(State::WaitingPeer, State::Open, Pause::None),
            Some((PipeChange::Opened, "The pipe \"Launch\" (id p1, in Site) opened: both people approved it.".to_owned()))
        );
        assert_eq!(said(State::Paused, State::Open, Pause::None).map(|said| said.0), Some(PipeChange::Resumed));
        assert_eq!(said(State::Open, State::Paused, Pause::Peer).map(|said| said.1).as_deref(), Some("The pipe \"Launch\" (id p1, in Site) was paused by the other side."));
        assert!(said(State::Open, State::Paused, Pause::Loop).is_some_and(|said| said.1.ends_with("talked on their own; Matt can continue it.")));
        assert_eq!(said(State::Open, State::Closed, Pause::None).map(|said| said.0), Some(PipeChange::Closed));
        assert_eq!(said(State::Approval, State::WaitingPeer, Pause::None), None, "only this side approved it");
        assert_eq!(said(State::Open, State::Open, Pause::None), None);
    }

    #[test]
    fn the_other_side_online_fires_only_once_it_held() {
        let mut pending = None;
        assert_eq!(settle(None, None, &mut pending, 0), (None, None), "not known yet");
        assert_eq!(settle(None, Some(true), &mut pending, 0), (Some(true), None), "first known: no fire");
        // Gone, back within the minute: nothing.
        assert_eq!(settle(Some(true), Some(false), &mut pending, 1_000), (Some(true), None));
        assert_eq!(settle(Some(true), Some(true), &mut pending, 30_000), (Some(true), None));
        assert_eq!(pending, None);
        // Gone for a minute: offline, once.
        assert_eq!(settle(Some(true), Some(false), &mut pending, 40_000), (Some(true), None));
        assert_eq!(settle(Some(true), None, &mut pending, 70_000), (Some(true), None), "unknown meanwhile changes nothing");
        assert_eq!(settle(Some(true), Some(false), &mut pending, 99_000), (Some(true), None));
        assert_eq!(settle(Some(true), Some(false), &mut pending, 100_000), (Some(false), Some(false)));
        assert_eq!(settle(Some(false), Some(false), &mut pending, 200_000), (Some(false), None));
    }

    #[test]
    fn websites_shared_name_only_this_sides() {
        let mut old = pipe(State::Open, Pause::None);
        old.mine.insert("a".into(), "Old".into());
        old.theirs.insert("x".into());
        let mut new = pipe(State::Open, Pause::None);
        new.mine.insert("b".into(), "Dash<board>".into());
        new.theirs.extend(["y".to_owned(), "z".to_owned()]);
        let lines: Vec<(PipeChange, String)> =
            share_changes(&old, &new, "the pipe p1", "Matt").into_iter().map(|change| (change.kind, change.line)).collect();
        assert_eq!(
            lines,
            vec![
                (PipeChange::Shared, "Matt's website \"Dash&lt;board&gt;\" is now shared over the pipe p1.".to_owned()),
                (PipeChange::Unshared, "Matt's website \"Old\" is no longer shared over the pipe p1.".to_owned()),
                (PipeChange::Shared, "The other side shared 2 websites over the pipe p1.".to_owned()),
                (PipeChange::Unshared, "The other side stopped sharing a website over the pipe p1.".to_owned()),
            ]
        );
    }
}
