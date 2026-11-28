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
//!
//! Only the methods factoryd itself sends go through at all (`ALLOWED`,
//! Bruno 54069, Gyro): whatever reaches the runner's DevTools channel can't
//! read the browser's cookie jar wholesale, its storage or a snapshot of a
//! page. The cookie lists factoryd reads (which sites it's signed in to,
//! which cookies to expire) come back with every value blanked (`redact`):
//! factoryd never needs one.
//!
//! `DOM.setFileInputFiles` gives a page files from the runner's disk, so
//! it takes only files factoryd staged for an upload (`uploads`), by the
//! exact path the runner gave: never the profile, its cookies or anything
//! else Chrome could read.

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

/// Target.attachToTarget's params, as far as judging needs: a duplicate
/// field fails to read, and so is refused.
#[derive(Deserialize)]
struct Attach {
    params: AttachParams,
}

#[derive(Deserialize)]
struct AttachParams {
    #[serde(default)]
    flatten: bool,
}

/// DOM.setFileInputFiles's params, as far as judging needs.
#[derive(Deserialize)]
struct SetFiles {
    params: SetFilesParams,
}

#[derive(Deserialize)]
struct SetFilesParams {
    files: Vec<String>,
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

/// The DevTools methods factoryd sends to a driven Chrome, sorted: every
/// one its code calls (browser_host, errands), nothing more. `Input.*`
/// needs a lease besides; `Target.sendMessageToTarget` stays out.
const ALLOWED: &[&str] = &[
    "Browser.cancelDownload",
    "Browser.close",
    "Browser.getVersion",
    "Browser.getWindowForTarget",
    "Browser.setDownloadBehavior",
    "DOM.describeNode",
    "DOM.getBoxModel",
    "DOM.getFrameOwner",
    "DOM.getNodeForLocation",
    "DOM.setFileInputFiles",
    "Emulation.setDeviceMetricsOverride",
    "Fetch.continueRequest",
    "Fetch.continueResponse",
    "Fetch.enable",
    "Fetch.failRequest",
    "Fetch.fulfillRequest",
    "Input.dispatchKeyEvent",
    "Input.dispatchMouseEvent",
    "Input.insertText",
    "Network.clearBrowserCache",
    "Network.getCookies",
    "Page.captureScreenshot",
    "Page.createIsolatedWorld",
    "Page.enable",
    "Page.getFrameTree",
    "Page.getLayoutMetrics",
    "Page.navigate",
    "Page.screencastFrameAck",
    "Page.setDocumentContent",
    "Page.setInterceptFileChooserDialog",
    "Page.startScreencast",
    "Page.stopScreencast",
    "Runtime.callFunctionOn",
    "Runtime.evaluate",
    "Storage.clearDataForOrigin",
    "Storage.getCookies",
    "Storage.setCookies",
    "Target.activateTarget",
    "Target.attachToTarget",
    "Target.closeTarget",
    "Target.createTarget",
    "Target.detachFromTarget",
    "Target.getTargetInfo",
    "Target.getTargets",
];

/// Judges one of the host's CDP messages for a driven Chrome, with the
/// lease live now or none, and no file staged for an upload.
pub(crate) fn judge(message: &[u8], leased: bool) -> Verdict {
    judge_with_uploads(message, leased, &|_| false)
}

/// `judge`, with `staged` saying which paths are files staged for an
/// upload (`uploads`).
pub(crate) fn judge_with_uploads(message: &[u8], leased: bool, staged: &dyn Fn(&str) -> bool) -> Verdict {
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
        Some(method) if ALLOWED.binary_search(&method).is_err() => refuse("Claktory's browser doesn't take that method"),
        Some(method) if method.starts_with("Input.") && !leased => refuse("no input lease"),
        // A session not flattened would carry its calls past this judge.
        Some("Target.attachToTarget") if !serde_json::from_slice::<Attach>(message).is_ok_and(|attach| attach.params.flatten) => {
            refuse("sessions are flattened here: attach with flatten")
        }
        Some("DOM.setFileInputFiles")
            if !serde_json::from_slice::<SetFiles>(message)
                .is_ok_and(|set| !set.params.files.is_empty() && set.params.files.iter().all(|file| staged(file))) =>
        {
            refuse("only a file uploaded through Claktory can go to a page")
        }
        Some(_) => Verdict::Forward,
        None => refuse("no method"),
    }
}

/// One of Chrome's messages to the host with every cookie value blanked:
/// a reply whose result lists cookies (Storage.getCookies,
/// Network.getCookies) keeps their names, domains and the rest, never a
/// value. Judged by its shape, never by its id, so a call can't pass one
/// off as another's answer. None: nothing to blank, sent as it is.
pub(crate) fn redact(message: &[u8]) -> Option<Vec<u8>> {
    const KEY: &[u8] = b"\"cookies\"";
    if !message.windows(KEY.len()).any(|window| window == KEY) {
        return None;
    }
    let mut reply: Value = serde_json::from_slice(message).ok()?;
    let cookies = reply.get_mut("result")?.get_mut("cookies")?.as_array_mut()?;
    for cookie in cookies.iter_mut() {
        if let Some(value) = cookie.get_mut("value") {
            *value = Value::String(String::new());
        }
    }
    serde_json::to_vec(&reply).ok()
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
        // Anything else factoryd sends goes through either way.
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

    /// Bruno 54069: only factoryd's own methods; the jar, storage and
    /// snapshots stay shut, leased or not.
    #[test]
    fn only_the_methods_factoryd_sends_go_through() {
        assert!(ALLOWED.windows(2).all(|pair| pair[0] < pair[1]), "sorted, for binary_search");
        for method in ["Network.getAllCookies", "Network.enable", "Storage.getStorageKeyForFrame", "Storage.getUsageAndQuota",
            "Page.captureSnapshot", "Page.printToPDF", "Debugger.enable", "IO.read", "DOMStorage.getDOMStorageItems",
            "IndexedDB.requestData", "Runtime.Evaluate", "runtime.evaluate", ""]
        {
            let message = json!({"id": 3, "sessionId": "S", "method": method}).to_string();
            let refused = reply(judge(message.as_bytes(), true));
            assert_eq!((refused["id"].clone(), refused["sessionId"].clone()), (json!(3), json!("S")), "{method}");
            assert_eq!(refused["error"]["code"], REFUSED, "{method}");
        }
        assert!(matches!(judge(br#"{"id":4}"#, true), Verdict::Refuse(_)), "no method");
        assert!(matches!(judge(br#"{"method":"Network.getAllCookies"}"#, true), Verdict::Drop(_)), "no id");
        // Escaped, it's the same name.
        assert!(matches!(judge(br#"{"id":5,"method":"Network.getAll\u0043ookies"}"#, true), Verdict::Refuse(_)));
        for method in ["Target.exposeDevToolsProtocol", "Target.createBrowserContext", "Target.setAutoAttach", "Target.sendMessageToTarget"] {
            let message = json!({"id": 3, "method": method, "params": {}}).to_string();
            assert!(matches!(judge(message.as_bytes(), true), Verdict::Refuse(_)), "{method}");
        }
        // Attached only flattened (Fugo 54214).
        let attach = |params: &str| judge(format!(r#"{{"id":9,"method":"Target.attachToTarget","params":{params}}}"#).as_bytes(), true);
        assert_eq!(attach(r#"{"targetId":"T","flatten":true}"#), Verdict::Forward);
        for params in [r#"{"targetId":"T"}"#, r#"{"targetId":"T","flatten":false}"#, r#"{"targetId":"T","flatten":true,"flatten":false}"#, "null"] {
            assert!(matches!(attach(params), Verdict::Refuse(_)), "{params}");
        }
        for method in ["Storage.getCookies", "Network.getCookies", "Page.navigate", "Target.createTarget", "Fetch.continueResponse"] {
            let message = json!({"id": 6, "method": method, "params": {}}).to_string();
            assert_eq!(judge(message.as_bytes(), false), Verdict::Forward, "{method}");
        }
    }

    /// A page gets only files staged for an upload, each by its exact path.
    #[test]
    fn only_staged_files_go_to_a_page() {
        let staged = |path: &str| path == "/tmp/claktory-browser-1/uploads/0000000000000001/cv.pdf";
        let set = |files: &str| judge_with_uploads(format!(r#"{{"id":4,"sessionId":"S","method":"DOM.setFileInputFiles","params":{{"files":{files},"backendNodeId":7}}}}"#).as_bytes(), false, &staged);
        assert_eq!(set(r#"["/tmp/claktory-browser-1/uploads/0000000000000001/cv.pdf"]"#), Verdict::Forward);
        for files in [
            r#"["/home/u/.ssh/id_ed25519"]"#,
            r#"["/tmp/claktory-browser-1/uploads/0000000000000001/cv.pdf","/etc/passwd"]"#,
            r#"["/tmp/claktory-browser-1/uploads/0000000000000001/../../../profiles/Cookies"]"#,
            r#""/tmp/claktory-browser-1/uploads/0000000000000001/cv.pdf""#,
            "[]",
            "null",
        ] {
            let refused = reply(set(files));
            assert_eq!((refused["id"].clone(), refused["error"]["code"].clone()), (json!(4), json!(REFUSED)), "{files}");
        }
        let twice = br#"{"id":5,"method":"DOM.setFileInputFiles","params":{"files":["/tmp/claktory-browser-1/uploads/0000000000000001/cv.pdf"],"files":["/etc/passwd"]}}"#;
        assert!(matches!(judge_with_uploads(twice, false, &staged), Verdict::Refuse(_)), "a second files list is refused");
        let no_files = br#"{"id":7,"method":"DOM.setFileInputFiles","params":{"backendNodeId":7}}"#;
        assert!(matches!(judge_with_uploads(no_files, false, &staged), Verdict::Refuse(_)), "never without files");
        let staged_one = br#"{"id":6,"method":"DOM.setFileInputFiles","params":{"files":["/tmp/claktory-browser-1/uploads/0000000000000001/cv.pdf"]}}"#;
        assert!(matches!(judge(staged_one, true), Verdict::Refuse(_)), "with no uploads, none");
    }

    /// Every cookie a reply lists comes back without its value; anything
    /// else, as it is.
    #[test]
    fn cookie_values_never_come_back() {
        let jar = br#"{"id":2000000002,"result":{"cookies":[{"name":"SID","value":"secret","domain":".google.com","path":"/"},{"name":"x","value":"y","domain":"a.example"}]}}"#;
        let blanked: Value = serde_json::from_slice(&redact(jar).expect("blanked")).unwrap();
        assert_eq!(blanked["id"], 2000000002u64);
        let cookies = blanked["result"]["cookies"].as_array().unwrap();
        assert_eq!(cookies.len(), 2);
        assert!(cookies.iter().all(|cookie| cookie["value"] == ""));
        assert_eq!((cookies[0]["name"].as_str(), cookies[0]["domain"].as_str()), (Some("SID"), Some(".google.com")));
        assert!(!String::from_utf8(redact(jar).unwrap()).unwrap().contains("secret"));
        // In a session, from Network.getCookies: the same.
        let session = br#"{"id":7,"sessionId":"S","result":{"cookies":[{"name":"a","value":"b"}]}}"#;
        assert!(!String::from_utf8(redact(session).unwrap()).unwrap().contains("\"b\""));
        // Nothing to blank: sent as it is.
        assert_eq!(redact(br#"{"id":1,"result":{"result":{"type":"string","value":"cookies"}}}"#), None);
        assert_eq!(redact(br#"{"method":"Page.screencastFrame","params":{"data":"..."}}"#), None);
        assert_eq!(redact(br#"{"id":1,"result":{"value":"\"cookies\""}}"#), None);
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
