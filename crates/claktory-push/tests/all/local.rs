//! `claktory-push --local FILE`: a relay for tests on this computer, with no
//! keys, that holds pushes in a file instead of sending them.

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

const BINARY: &str = env!("CARGO_BIN_EXE_claktory-push");

/// Kills the relay when the test ends.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").expect("bind").local_addr().expect("addr").port()
}

#[tokio::test]
async fn a_local_relay_needs_no_keys_and_holds_each_push_in_its_file() {
    let dir = tempfile::tempdir().expect("dir");
    let held = dir.path().join("pushes.jsonl");
    let listen = format!("127.0.0.1:{}", free_port());
    let db = dir.path().join("push.db");
    let child = Command::new(BINARY)
        .args(["--db", db.to_str().expect("utf-8"), "--listen", &listen, "--local", held.to_str().expect("utf-8")])
        .env_remove("CREDENTIALS_DIRECTORY")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("started");
    let _running = Running(child);
    let url = format!("http://{listen}");
    let client = reqwest::Client::new();
    let mut health = None;
    for _ in 0..100 {
        if let Ok(response) = client.get(format!("{url}/v1/health")).send().await {
            health = Some(response.json::<Value>().await.expect("JSON"));
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(health, Some(json!({"ok": true, "apns": true, "fcm": true})), "every platform, with no keys");

    let post = async |path: &str, body: Value| {
        let response = client.post(format!("{url}{path}")).json(&body).send().await.expect("answered");
        (response.status().as_u16(), response.json::<Value>().await.expect("JSON"))
    };
    let mut handles = Vec::new();
    for (platform, token) in [("apns_sandbox", "a1b2c3"), ("fcm", "fcm-token:1")] {
        let (status, body) = post("/v1/register", json!({"platform": platform, "token": token})).await;
        assert_eq!(status, 200, "{body}");
        handles.push(body["handle"].as_str().expect("a handle").to_owned());
    }
    for (handle, urgency) in handles.iter().zip(["urgent", "normal"]) {
        let (status, body) = post("/v1/push", json!({"handle": handle, "ciphertext": "AAAA", "urgency": urgency, "thread": "dGhyZWFk"})).await;
        assert_eq!(status, 200, "{body}");
    }

    let text = std::fs::read_to_string(&held).expect("the file");
    let lines: Vec<Value> = text.lines().map(|line| serde_json::from_str(line).expect("a JSON line")).collect();
    let [apns, fcm] = lines.as_slice() else { panic!("{text}") };
    assert_eq!((apns["platform"].as_str(), apns["token"].as_str(), apns["urgency"].as_str()), (Some("apns_sandbox"), Some("a1b2c3"), Some("urgent")));
    // What Apple would have got, for `xcrun simctl push`.
    assert_eq!(apns["payload"]["c"], "AAAA");
    assert_eq!(apns["payload"]["aps"]["alert"]["body"], claktory_push::ALERT_BODY);
    assert_eq!(apns["payload"]["aps"]["interruption-level"], "time-sensitive");
    assert_eq!(apns["payload"]["aps"]["thread-id"], "dGhyZWFk");
    assert_eq!((fcm["platform"].as_str(), fcm["token"].as_str()), (Some("fcm"), Some("fcm-token:1")));
    assert_eq!(fcm["payload"]["message"]["data"]["c"], "AAAA");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&held).expect("meta").permissions().mode() & 0o777, 0o600);
    }
}

#[test]
fn a_local_relay_takes_no_keys() {
    let dir = tempfile::tempdir().expect("dir");
    let output = Command::new(BINARY)
        .args(["--db", "push.db", "--local", "pushes.jsonl", "--apns-key-id", "K"])
        .current_dir(dir.path())
        .output()
        .expect("ran");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--local sends nothing to Apple or Google"), "{output:?}");
}
