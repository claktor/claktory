//! Quiet messages: an agents_send with `quiet` is for a heads-up, an FYI or
//! a copy that shouldn't wake its recipient.
//!
//! It's an agent's message like any other (a row of `messages`, with its
//! number, provenance and the sender's taint, spread at send time), marked
//! `quiet`. It never makes its recipient ready (`Runtime::has_ready`), so
//! it never starts a run, moves a command to the background, or keeps an
//! idle agent from compacting. It waits, however long, until something
//! else wakes the agent (the person, another message, a task, a monitor, a
//! Claktory event, a checklist nudge) and then comes with that, oldest
//! first, at most [`BATCH`] at once and about [`BATCH_BYTES`] of text. A
//! working agent gets it after its current step, as it would any message.
//! Past the cap, the rest are held back until the next wake, or until the
//! agent asks for them with `agents_quiet`; they then come after its
//! current step. Every one reaches the conversation as its own event,
//! marked quiet; the model reads Claktory's line on the batch ("While you
//! were idle, 4 quiet messages came…") just before the first.
//!
//! Quiet copies for a lead (`provenance`, #285) are separate: they're never
//! in `messages`.

use std::sync::Arc;

use factory_agent::{Tool, ToolResult};
use serde::Deserialize;
use serde_json::json;

use crate::factory::{Factory, Queued, Runtime};

pub(crate) const SCHEMA: &str = "
-- Sent quietly (agents_send's quiet): 1; NULL when not. It waits for the
-- agent's next wake and doesn't cause one.
ALTER TABLE messages ADD COLUMN quiet INTEGER;
";

pub(crate) const TOOL: &str = "agents_quiet";
const ABOUT: &str = "Brings the quiet messages still waiting for you, past the most that came at once: up to `limit` (10 at most) of them, oldest first, arrive after this step, as the others did.";
/// The most quiet messages that come at once.
pub(crate) const BATCH: usize = 10;
/// About the most text that comes at once in quiet messages; one longer
/// than this still comes, alone.
const BATCH_BYTES: usize = 24_000;

/// When quiet messages are taken with what's ready.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum At {
    /// A run's start: they come only with something that wakes the agent,
    /// and those held back at the last wake come again.
    Wake,
    /// Between steps of its work: they come as any message would.
    Step,
    /// Not at all (compacting while idle).
    Never,
}

/// Takes what's ready for the agent now, oldest first: what isn't quiet,
/// and, `at` a wake or a step, the quiet messages that fit in a batch; the
/// first of these says how many came, and how many more wait.
pub(crate) fn take(runtime: &mut Runtime, at: At) -> Vec<Queued> {
    let woken = at == At::Wake && runtime.has_ready();
    if woken {
        for queued in &mut runtime.queue {
            queued.held = false;
        }
    }
    let with_quiet = woken || at == At::Step;
    let ready = crate::speech::voice::deliverable(&runtime.queue);
    let (mut taken, mut kept) = (Vec::new(), Vec::new());
    let (mut count, mut bytes) = (0, 0);
    for (mut queued, ready) in std::mem::take(&mut runtime.queue).into_iter().zip(ready) {
        if !queued.quiet() {
            if ready { taken.push(queued) } else { kept.push(queued) }
            continue;
        }
        if !ready || !with_quiet || queued.held {
            kept.push(queued);
            continue;
        }
        if count < BATCH && (count == 0 || bytes + queued.text.len() <= BATCH_BYTES) {
            count += 1;
            bytes += queued.text.len();
            taken.push(queued);
        } else {
            queued.held = true;
            kept.push(queued);
        }
    }
    runtime.queue = kept;
    if count > 0 {
        let held = runtime.queue.iter().filter(|queued| queued.quiet() && queued.held).count();
        let direct = runtime
            .transcript
            .as_ref()
            .and_then(|transcript| transcript.frame())
            .is_some_and(|frame| frame.tools.iter().any(|tool| tool.name == TOOL));
        if let Some(note) = note(woken, count, held, direct)
            && let Some(provenance) =
                taken.iter_mut().find(|queued| queued.quiet()).and_then(|queued| queued.event.as_mut()?.provenance.as_mut())
        {
            provenance.quiet_note = note;
        }
    }
    taken
}

/// Claktory's line on a batch of `count` quiet messages: at a wake, always;
/// between steps, only when `held` more wait. `direct`: whether it has
/// agents_quiet among its tools (otherwise it reaches it through
/// extension_call).
fn note(woken: bool, count: usize, held: usize, direct: bool) -> Option<String> {
    let messages = |count: usize| if count == 1 { "1 quiet message".to_owned() } else { format!("{count} quiet messages") };
    let mut note = if woken {
        let came = if count == 1 { "came; it follows" } else { "came; they follow, oldest first" };
        format!("While you were idle, {} {came}.", messages(count))
    } else if held > 0 {
        format!("{} came while you worked.", messages(count))
    } else {
        return None;
    };
    if held > 0 {
        let how = if direct { format!("{TOOL} brings them") } else { format!("extension_call with tool \"{TOOL}\" brings them") };
        let more = if held == 1 { "1 more is" } else { &format!("{held} more are") };
        note.push_str(&format!(" {more} waiting: {how}, {BATCH} at a time, when they'd help your work."));
    }
    Some(note)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    #[serde(default)]
    limit: Option<usize>,
}

pub(crate) fn tool(factory: &Arc<Factory>, agent: &str) -> Tool {
    let parameters = json!({
        "type": "object",
        "properties": {"limit": {"type": "integer", "minimum": 1, "maximum": BATCH}},
        "additionalProperties": false,
    });
    super::super::tool(
        TOOL,
        "Read quiet messages",
        ABOUT,
        parameters,
        super::super::typed_handler(factory, agent, |factory, agent, args: Args| Ok(release(factory, agent, args.limit))),
    )
}

/// Lets up to `limit` of the quiet messages held back for `agent` come
/// after its current step. Its details: how many were `released`, and how
/// many are still `waiting` after them.
fn release(factory: &Factory, agent: &str, limit: Option<usize>) -> ToolResult {
    let limit = limit.unwrap_or(BATCH).clamp(1, BATCH);
    let (released, coming, waiting) = factory.lock().agents.get_mut(agent).map_or((0, 0, 0), |runtime| unhold(runtime, limit));
    let come = |count: usize| if count == 1 { "1 quiet message comes".to_owned() } else { format!("{count} quiet messages come") };
    let mut text = match (released, coming) {
        (0, 0) => "No quiet messages are waiting for you.".to_owned(),
        (0, coming) => format!("{} after this step anyway: none were held back.", come(coming)),
        (released, _) => format!("{} after this step.", come(released)),
    };
    if waiting > 0 {
        let more = if waiting == 1 { "1 more is".to_owned() } else { format!("{waiting} more are") };
        text.push_str(&format!(" {more} still waiting; call {TOOL} again for them."));
    }
    ToolResult::text(text, json!({"released": released, "waiting": waiting}))
}

/// Lets up to `limit` held-back quiet messages come after the current
/// step: how many it let go, how many now come, and how many still wait.
fn unhold(runtime: &mut Runtime, limit: usize) -> (usize, usize, usize) {
    let mut released = 0;
    for queued in runtime.queue.iter_mut().filter(|queued| queued.quiet() && queued.held).take(limit) {
        queued.held = false;
        released += 1;
    }
    let coming = runtime.queue.iter().filter(|queued| queued.quiet() && !queued.held).count();
    (released, coming, runtime.queue.iter().filter(|queued| queued.quiet() && queued.held).count())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(id: i64, text: &str, quiet: bool) -> Queued {
        let event = factory_agent::Event {
            source: factory_agent::EventSource::Agent,
            from: "Bo".into(),
            id: "bo".into(),
            summary: String::new(),
            transient: false,
            pipe_seqs: Vec::new(),
            provenance: Some(factory_agent::Provenance { message: id as u64, quiet, ..Default::default() }),
        };
        Queued { id, ..Queued::event(text.to_owned(), event) }
    }

    fn ids(queued: &[Queued]) -> Vec<i64> {
        queued.iter().map(|queued| queued.id).collect()
    }

    fn notes(queued: &[Queued]) -> Vec<String> {
        queued
            .iter()
            .filter_map(|queued| queued.event.as_ref()?.provenance.as_ref())
            .map(|provenance| provenance.quiet_note.clone())
            .filter(|note| !note.is_empty())
            .collect()
    }

    #[test]
    fn quiet_messages_come_only_with_a_wake_and_at_most_a_batch_at_once() {
        let mut runtime = Runtime { queue: (1..=12).map(|id| message(id, "FYI", true)).collect(), ..Runtime::default() };
        assert!(!runtime.has_ready(), "quiet messages alone never make it ready");
        assert!(take(&mut runtime, At::Wake).is_empty(), "nothing wakes it");
        assert!(take(&mut runtime, At::Never).is_empty());
        assert_eq!(runtime.quiet_waiting(), 12);

        // The person writes: it wakes, with the first 10, oldest first.
        runtime.queue.push(Queued { id: 13, ..Queued::from_person("Hi", None) });
        let taken = take(&mut runtime, At::Wake);
        assert_eq!(ids(&taken), [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 13]);
        assert_eq!(
            notes(&taken),
            ["While you were idle, 10 quiet messages came; they follow, oldest first. 2 more are waiting: extension_call with tool \"agents_quiet\" brings them, 10 at a time, when they'd help your work."]
        );
        assert_eq!(ids(&runtime.queue), [11, 12]);
        assert!(runtime.queue.iter().all(|queued| queued.held));

        // Between steps, held ones stay; a new one comes as any message would.
        runtime.queue.push(message(14, "FYI", true));
        let taken = take(&mut runtime, At::Step);
        assert_eq!(ids(&taken), [14]);
        assert_eq!(notes(&taken), ["1 quiet message came while you worked. 2 more are waiting: extension_call with tool \"agents_quiet\" brings them, 10 at a time, when they'd help your work."]);

        // agents_quiet lets them come after the step.
        assert_eq!(unhold(&mut runtime, 1), (1, 1, 1));
        assert_eq!(ids(&take(&mut runtime, At::Step)), [11]);
        assert_eq!(unhold(&mut runtime, 10), (1, 1, 0));
        assert_eq!(ids(&take(&mut runtime, At::Step)), [12]);
        assert_eq!(unhold(&mut runtime, 10), (0, 0, 0));
    }

    #[test]
    fn a_batch_holds_about_its_size_of_text_but_always_one() {
        let long = "x".repeat(BATCH_BYTES);
        let mut runtime = Runtime { queue: vec![message(1, &long, true), message(2, "short", true)], ..Runtime::default() };
        runtime.queue.push(Queued { id: 3, ..Queued::from_person("Hi", None) });
        assert_eq!(ids(&take(&mut runtime, At::Wake)), [1, 3]);
        assert_eq!(ids(&runtime.queue), [2]);
        // The next wake brings what was held.
        runtime.queue.push(Queued { id: 4, ..Queued::from_person("Again", None) });
        let taken = take(&mut runtime, At::Wake);
        assert_eq!(ids(&taken), [2, 4]);
        assert_eq!(notes(&taken), ["While you were idle, 1 quiet message came; it follows."]);
    }

    #[test]
    fn the_line_on_a_batch_says_how_many_came_and_how_to_get_the_rest() {
        assert_eq!(note(true, 1, 0, true).as_deref(), Some("While you were idle, 1 quiet message came; it follows."));
        assert_eq!(
            note(true, 10, 3, true).as_deref(),
            Some(
                "While you were idle, 10 quiet messages came; they follow, oldest first. 3 more are waiting: agents_quiet brings them, 10 at a time, when they'd help your work."
            )
        );
        assert_eq!(note(false, 2, 0, true), None, "between steps, they come as any message would");
        assert!(note(false, 10, 1, false).is_some_and(|note| note.starts_with("10 quiet messages came while you worked. 1 more is waiting: extension_call with tool \"agents_quiet\"")));
    }
}
