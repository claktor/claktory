//! Grok on a subscription, as the Claktory keeps it: its sign-in
//! only in the vault (never in `auth.json`, not even its copy for an older
//! backend, and refused while the vault is locked), its tokens in no log,
//! transcript or other file, and revoked at xAI on sign-out.

use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::accounts::account;
use crate::common::{View, apply_request, describe, home, send_message, temp_dir};
use crate::sign_ins::{locked_after_restart, moved, open, plain, until};
use crate::usage_switch::{Answer, Route, create, serve_routes};
use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body};
use serde_json::{Value, json};

/// What Grok's tokens end with: never to be found in plaintext.
const SENTINEL: &[u8] = b"grok-sentinel";

/// An unsigned token for user-0000abcd, its signature the sentinel.
fn access() -> String {
    use base64::Engine;
    let encode = |value: Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.to_string());
    format!("{}.{}.grok-sentinel-access", encode(json!({"alg": "none"})), encode(json!({"sub": "user-0000abcd", "tier": "SuperGrok"})))
}

/// xAI's sign-in and the proxy, on this computer: the device code, its
/// token at the first poll, revoke, and "hi from grok" for any request.
fn xai(base: &Arc<Mutex<String>>) -> Route {
    let base = base.clone();
    Arc::new(move |path, _| {
        let json = |body: Value| -> Answer { (200, String::new(), "application/json", body.to_string()) };
        let base = base.lock().expect("lock").clone();
        if path.ends_with("/oauth2/device/code") {
            return json(json!({
                "device_code": "device-1",
                "user_code": "AB12-CD34",
                "verification_uri": format!("{base}/oauth2/device"),
                "verification_uri_complete": format!("{base}/oauth2/device?user_code=AB12-CD34"),
                "expires_in": 600,
                "interval": 1,
            }));
        }
        if path.ends_with("/oauth2/token") {
            return json(json!({
                "access_token": access(),
                "refresh_token": "grok-sentinel-refresh",
                "id_token": "grok-sentinel-id",
                "token_type": "Bearer",
                "expires_in": 21600,
                "scope": "openid offline_access grok-cli:access",
            }));
        }
        if path.ends_with("/oauth2/revoke") {
            return json(json!({}));
        }
        if path.ends_with("/responses") {
            let events = [
                json!({"type": "response.created", "response": {"id": "r1", "status": "in_progress"}}),
                json!({"type": "response.output_item.added", "output_index": 0, "item": {"type": "message", "id": "m1", "role": "assistant", "status": "in_progress", "content": []}}),
                json!({"type": "response.output_text.delta", "output_index": 0, "delta": "hi from grok"}),
                json!({"type": "response.output_item.done", "output_index": 0, "item": {"type": "message", "id": "m1", "role": "assistant", "status": "completed", "content": [{"type": "output_text", "text": "hi from grok", "annotations": []}]}}),
                json!({"type": "response.completed", "response": {"id": "r1", "status": "completed", "usage": {"input_tokens": 10, "output_tokens": 3, "total_tokens": 13}}}),
            ];
            let body =
                events.iter().map(|event| format!("event: {}\ndata: {event}\n\n", event["type"].as_str().unwrap_or_default())).collect();
            return (200, String::new(), "text/event-stream", body);
        }
        (404, String::new(), "application/json", "{}".to_owned())
    })
}

/// The mock, serving: its address and the paths it was asked for.
async fn serve_xai() -> (String, crate::usage_switch::Seen) {
    let base = Arc::new(Mutex::new(String::new()));
    let (url, seen) = serve_routes(tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind"), xai(&base));
    *base.lock().expect("lock") = url.clone();
    (url, seen)
}

fn asked(seen: &crate::usage_switch::Seen, path: &str) -> bool {
    seen.lock().expect("lock").iter().any(|(asked, _)| asked.ends_with(path))
}

/// The files under `dir` with a Grok token in them, sealed ones aside.
fn sentinels_in(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(path) = pending.pop() {
        let Ok(metadata) = std::fs::symlink_metadata(&path) else { continue };
        if metadata.is_dir() {
            pending.extend(std::fs::read_dir(&path).expect("dir").map(|entry| entry.expect("entry").path()));
        } else if std::fs::read(&path).is_ok_and(|bytes| bytes.windows(SENTINEL.len()).any(|window| window == SENTINEL)) {
            found.push(path.display().to_string());
        }
    }
    found
}

/// What Claktory's own code logs, at every level, in every test of this
/// binary (a scoped subscriber misses what other tests' threads first
/// logged): the lines about Grok alone are kept.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.windows(4).any(|window| window.eq_ignore_ascii_case(b"grok")) {
            self.0.lock().expect("lock").extend_from_slice(bytes);
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;

    fn make_writer(&'a self) -> Captured {
        self.clone()
    }
}

/// The capture, from the first call on.
fn captured() -> &'static Captured {
    static CAPTURED: std::sync::OnceLock<Captured> = std::sync::OnceLock::new();
    CAPTURED.get_or_init(|| {
        let captured = Captured::default();
        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new("warn,factoryd=trace,factory_agent=trace"))
            .with_ansi(false)
            .with_writer(captured.clone())
            .finish();
        tracing::subscriber::set_global_default(subscriber).expect("no other subscriber");
        captured
    })
}

fn signing_in() -> Request {
    Request::StartLogin(v1::StartLogin { provider_id: "grok".into(), method_id: "oauth".into(), ..Default::default() })
}

fn grok(view: &View) -> v1::Provider {
    view.workspace.providers.iter().find(|provider| provider.id == "grok").cloned().expect("grok")
}

/// Signed in to Grok with its device code, an agent works on it; its
/// tokens are in the vault only: not in `auth.json`'s copy for the backend
/// before (which keeps the other sign-ins), nor in any log, transcript or
/// other file. Signing out revokes the sign-in at xAI.
#[tokio::test]
async fn a_grok_sign_in_is_kept_in_the_vault_alone_and_revoked_on_sign_out() {
    let captured = captured();
    let dir = temp_dir();
    let (base_url, seen) = serve_xai().await;
    // The backend before reads only auth.json: it keeps a copy of the others.
    let bin = home(&dir).root().join("bin");
    std::fs::create_dir_all(&bin).expect("bin");
    std::fs::write(bin.join("factoryd.previous"), b"an older backend").expect("previous");
    std::fs::write(dir.join("auth.json"), json!({"anthropic": account("a")}).to_string()).expect("credentials");
    let (_running, mut client, mut view) = open(&dir, &base_url).await;
    until("the sign-ins move into the vault", || moved(&dir)).await;

    assert!(matches!(apply_request(&mut client, &mut view, signing_in()).await, Body::Ack(_)));
    view.follow(&mut client, |view| view.workspace.login.as_ref().is_some_and(|login| !login.user_code.is_empty())).await;
    let login = view.workspace.login.clone().expect("login");
    assert_eq!(login.user_code, "AB12-CD34");
    assert!(login.verification_url.starts_with(&format!("{base_url}/oauth2/device")), "{}", login.verification_url);
    view.follow(&mut client, |view| view.workspace.login.is_none()).await;
    let provider = grok(&view);
    assert_eq!(provider.signed_in_with, "Grok (SuperGrok or X Premium+)");
    assert_eq!(provider.accounts.iter().map(|account| account.label.as_str()).collect::<Vec<_>>(), ["Grok …abcd"]);

    create(&mut client, &mut view, "Builder", "grok/grok-4.7", "high").await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
    view.follow(&mut client, |view| describe(&view.entries).iter().any(|entry| entry == "assistant: hi from grok")).await;

    let copy = plain(&dir);
    assert_eq!(copy["anthropic"]["access"], "sk-ant-oat01-a", "the copy for the backend before stays");
    assert!(copy.get("grok").is_none(), "{copy}");
    assert!(home(&dir).root().join("vault/sign-ins.json").exists());

    assert!(matches!(
        apply_request(&mut client, &mut view, Request::SignOut(v1::SignOut { provider_id: "grok".into(), ..Default::default() })).await,
        Body::Ack(_)
    ));
    until("the sign-in is revoked at xAI", || asked(&seen, "/oauth2/revoke")).await;
    assert_eq!(grok(&view).signed_in_with, "");

    assert_eq!(sentinels_in(&dir), Vec::<String>::new());
    let logged = String::from_utf8_lossy(&captured.0.lock().expect("lock")).into_owned();
    assert!(logged.contains("provider=grok"), "the capture works: {logged}");
    assert!(!logged.contains("grok-sentinel"), "a token was logged");
}

/// Signing in to Grok while the vault is locked is refused, saying why:
/// its sign-in is never kept in plaintext instead.
#[tokio::test]
async fn signing_in_to_grok_while_the_vault_is_locked_is_refused() {
    let dir = temp_dir();
    let (base_url, _) = serve_xai().await;
    let (_factory, mut client, mut view, _) = locked_after_restart(&dir, &base_url).await;
    assert!(matches!(apply_request(&mut client, &mut view, signing_in()).await, Body::Ack(_)));
    view.follow(&mut client, |view| view.workspace.login.as_ref().is_some_and(|login| !login.error.is_empty())).await;
    let error = view.workspace.login.clone().expect("login").error;
    assert!(error.starts_with("A Grok sign-in is kept only with your secrets, which can't be opened now."), "{error}");
    assert!(plain(&dir).get("grok").is_none());
    assert_eq!(grok(&view).signed_in_with, "");
    assert_eq!(sentinels_in(&dir), Vec::<String>::new());
}
