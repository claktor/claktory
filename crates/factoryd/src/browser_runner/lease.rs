//! The agent's input lease (R11.3 #5, #530): the host's `Input.*` calls
//! reach a driven Chrome only while a lease the host took for one action
//! (a click, a fill, a scroll) is live. It ends when the host ends it, at
//! its hard expiry (enforced here, whether or not the host says so), and
//! when the browser stops, changes mode or Chrome starts again; losing the
//! host ends the runner, and so it. Outside a lease the call is answered
//! at once with a CDP error of the same id, never forwarded nor silently
//! dropped, so the caller doesn't wait out its timeout.
//!
//! A message that doesn't read as one CDP command is refused too (a
//! duplicate "method" key could say one thing here and another to
//! Chrome), and so is `Target.sendMessageToTarget`, which could carry an
//! `Input.*` call inside a string: sessions are flattened.

use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

/// The longest a lease can be asked for: one action, however long its
/// typing (R11.4 paces 8192 characters at a few minutes).
pub(crate) const MOST_MS: u32 = 10 * 60 * 1000;

/// The CDP error code of a refused call (server-defined range).
const REFUSED: i64 = -32001;

#[derive(Debug)]
pub(crate) struct Lease {
    pub id: u64,
    /// The Chrome it was taken for: another one ends it.
    pub generation: u64,
    pub until: Instant,
}

impl Lease {
    pub(crate) fn new(id: u64, generation: u64, for_ms: u32, now: Instant) -> Lease {
        Lease { id, generation, until: now + Duration::from_millis(u64::from(for_ms.min(MOST_MS))) }
    }

    pub(crate) fn live(&self, generation: u64, now: Instant) -> bool {
        self.generation == generation && now < self.until
    }
}

/// Just enough of a CDP command to judge it. Unknown fields are ignored;
/// a duplicate field fails to read, and so is refused.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Command {
    id: Option<Value>,
    method: Option<String>,
    session_id: Option<String>,
}

/// What becomes of one of the host's CDP messages.
#[derive(Debug, PartialEq)]
pub(crate) enum Verdict {
    /// On to Chrome.
    Forward,
    /// Back to the host instead, as this reply.
    Refuse(Vec<u8>),
    /// Nothing to answer (no id to answer to): dropped, and said.
    Drop(&'static str),
}

/// Judges one of the host's CDP messages for a driven Chrome, with the
/// lease live now or none.
pub(crate) fn judge(message: &[u8], leased: bool) -> Verdict {
    let Ok(command) = serde_json::from_slice::<Command>(message) else {
        return Verdict::Drop("a DevTools message from factoryd that doesn't read as one command was dropped");
    };
    let refuse = |why: &str| match &command.id {
        Some(id) => {
            let mut reply = json!({"id": id, "error": {"code": REFUSED, "message": why}});
            if let Some(session) = &command.session_id {
                reply["sessionId"] = Value::String(session.clone());
            }
            Verdict::Refuse(serde_json::to_vec(&reply).unwrap_or_default())
        }
        None => Verdict::Drop("a DevTools command with no id was dropped"),
    };
    match command.method.as_deref() {
        Some("Target.sendMessageToTarget") => refuse("sessions are flattened here: send to the session instead"),
        Some(method) if method.starts_with("Input.") && !leased => refuse("no input lease"),
        _ => Verdict::Forward,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(verdict: Verdict) -> Value {
        match verdict {
            Verdict::Refuse(bytes) => serde_json::from_slice(&bytes).unwrap(),
            other => panic!("not refused: {other:?}"),
        }
    }

    #[test]
    fn input_goes_through_only_under_a_lease() {
        let click = br#"{"id":7,"sessionId":"S1","method":"Input.dispatchMouseEvent","params":{"type":"mousePressed","x":1,"y":2}}"#;
        assert_eq!(judge(click, true), Verdict::Forward);
        let refused = reply(judge(click, false));
        assert_eq!(refused["id"], 7);
        assert_eq!(refused["sessionId"], "S1");
        assert_eq!(refused["error"]["code"], REFUSED);
        // Anything else goes through either way.
        assert_eq!(judge(br#"{"id":8,"method":"Runtime.evaluate","params":{}}"#, false), Verdict::Forward);
        // An escaped name is still Input.
        assert!(matches!(judge(br#"{"id":9,"method":"\u0049nput.insertText","params":{"text":"x"}}"#, false), Verdict::Refuse(_)));
    }

    #[test]
    fn what_could_hide_input_is_refused() {
        // A second "method" would read one way here and another in Chrome.
        assert!(matches!(judge(br#"{"id":1,"method":"Runtime.evaluate","method":"Input.dispatchKeyEvent"}"#, true), Verdict::Drop(_)));
        assert!(matches!(judge(b"not json", true), Verdict::Drop(_)));
        let wrapped = br#"{"id":2,"method":"Target.sendMessageToTarget","params":{"message":"{\"id\":1,\"method\":\"Input.insertText\"}"}}"#;
        assert_eq!(reply(judge(wrapped, true))["id"], 2);
        assert!(matches!(judge(br#"{"method":"Input.insertText"}"#, false), Verdict::Drop(_)));
    }

    #[test]
    fn a_lease_ends_at_its_time_and_with_its_chrome() {
        let now = Instant::now();
        let lease = Lease::new(1, 3, 1000, now);
        assert!(lease.live(3, now + Duration::from_millis(999)));
        assert!(!lease.live(3, now + Duration::from_millis(1000)));
        assert!(!lease.live(4, now));
        // Never longer than the most.
        let long = Lease::new(2, 3, u32::MAX, now);
        assert!(!long.live(3, now + Duration::from_millis(u64::from(MOST_MS))));
    }
}
