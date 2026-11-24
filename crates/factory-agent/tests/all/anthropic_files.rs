//! #382: an image read since goes to Anthropic once, as a file, and every
//! later request names it; what a request sent never changes after (an
//! image sent inline stays inline), a file Anthropic lost is uploaded again
//! with one retry, and an account whose uploads are refused goes inline.

use std::sync::Arc;

use crate::support::server::{Served, Server, serve, served_json};
use base64::Engine;
use factory_agent::providers::anthropic::files::{Forgotten, ImageFiles, ImageFilesFor};
use factory_agent::providers::anthropic::stream_simple;
use factory_agent::providers::catalog::models;
use factory_agent::stream::{LlmContext, StreamOptions, pending_message};
use factory_agent::types::{
    AssistantMessage, AssistantPart, Message, Model, StopReason, ToolResultMessage, ToolResultPart, UserMessage,
};
use serde_json::{Value, json};

fn hello() -> Served {
    let events = [
        json!({"type": "message_start", "message": {"id": "msg_1", "model": "claude-sonnet-4-5", "usage": {"input_tokens": 3, "output_tokens": 1}}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Seen."}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 2}}),
        json!({"type": "message_stop"}),
    ];
    let chunks = events.iter().map(|event| format!("event: {}\ndata: {event}\n\n", event["type"].as_str().expect("type"))).collect();
    Served { status: 200, content_type: "text/event-stream", chunks, hold: false, hold_before_headers: false }
}

fn uploaded(id: &str) -> Served {
    served_json(200, json!({"type": "file", "id": id, "size_bytes": 4, "mime_type": "image/png", "downloadable": false}))
}

fn model(server: &Server) -> Model {
    let mut model = models("anthropic").into_iter().find(|model| model.id == "claude-sonnet-4-5").expect("sonnet");
    model.base_url = server.base_url.clone();
    model
}

fn image(bytes: &[u8], digest: Option<&str>) -> ToolResultPart {
    ToolResultPart::Image {
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
        mime_type: "image/png".into(),
        digest: digest.map(str::to_owned),
    }
}

/// A conversation that read `images`, one tool call each.
fn context(images: Vec<ToolResultPart>) -> LlmContext {
    let mut messages = vec![Message::User(UserMessage::text("look", 1))];
    for (index, image) in images.into_iter().enumerate() {
        let id = format!("toolu_{index}");
        let sonnet = models("anthropic").into_iter().find(|model| model.id == "claude-sonnet-4-5").expect("sonnet");
        let call = AssistantPart::ToolCall { id: id.clone(), name: "read_image".into(), arguments: json!({"path": "a.png"}), thought_signature: None };
        messages.push(Message::Assistant(AssistantMessage {
            content: vec![call],
            stop_reason: StopReason::ToolUse,
            ..pending_message(&sonnet)
        }));
        messages.push(Message::ToolResult(ToolResultMessage {
            tool_call_id: id,
            tool_name: "read_image".into(),
            content: vec![ToolResultPart::Text { text: "a.png".into() }, image],
            details: Value::Null,
            usage: None,
            is_error: false,
            timestamp: 2,
            started: None,
        }));
    }
    LlmContext { system_prompt: Some("You are a coding agent.".into()), messages, tools: vec![] }
}

fn options(files: &Arc<ImageFiles>, account: &str) -> StreamOptions {
    StreamOptions {
        api_key: Some("test-key".into()),
        image_files: Some(ImageFilesFor { files: files.clone(), account: account.into() }),
        ..StreamOptions::default()
    }
}

async fn ask(server: &Server, context: LlmContext, options: StreamOptions) {
    let message = stream_simple(&model(server), context, options).expect("stream").result().await.expect("a message");
    assert_eq!(message.stop_reason, StopReason::Stop, "{:?}", message.error_message);
}

/// The images' sources in a Messages request, in order.
fn sources(body: &Value) -> Vec<Value> {
    let mut found = Vec::new();
    for message in body["messages"].as_array().expect("messages") {
        for block in message["content"].as_array().into_iter().flatten() {
            for inner in block["content"].as_array().into_iter().flatten() {
                if inner["type"] == "image" {
                    let source = &inner["source"];
                    found.push(if source["type"] == "file" { source["file_id"].clone() } else { json!("inline") });
                }
            }
        }
    }
    found
}

fn paths(server: &Server) -> Vec<String> {
    server.requests.lock().iter().map(|request| format!("{} {}", request.method, request.path)).collect()
}

#[tokio::test]
async fn a_new_image_is_uploaded_once_and_named_after_and_an_old_one_stays_inline() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("anthropic-files.jsonl");
    let files = Arc::new(ImageFiles::new(Some(path.clone())));
    let server = serve(vec![uploaded("file_1"), hello(), hello(), hello()]).await;
    let images = || vec![image(b"old", None), image(b"new", Some("d1"))];
    ask(&server, context(images()), options(&files, "anthropic/1")).await;
    // Then again, as the next call sends it: no upload, the same file.
    ask(&server, context(images()), options(&files, "anthropic/1")).await;
    let requests = server.requests.lock().clone();
    assert_eq!(paths(&server), ["POST /v1/files", "POST /v1/messages", "POST /v1/messages"]);
    let upload = &requests[0];
    let kind = upload.headers.iter().find(|(name, _)| name == "content-type").map(|(_, value)| value.clone()).unwrap_or_default();
    assert!(kind.starts_with("multipart/form-data; boundary="), "{kind}");
    assert!(upload.raw.contains("name=\"file\"; filename=\"image.png\"\r\nContent-Type: image/png\r\n\r\nnew\r\n"), "{}", upload.raw);
    assert!(upload.raw.contains("name=\"expires_in_seconds\"\r\n\r\n604800\r\n"), "{}", upload.raw);
    assert!(upload.headers.iter().any(|(name, value)| name == "x-api-key" && value == "test-key"));
    for request in &requests[1..] {
        assert_eq!(sources(&request.body), [json!("inline"), json!("file_1")]);
    }

    // After a restart: the same file, from what was kept.
    let again = Arc::new(ImageFiles::new(Some(path.clone())));
    ask(&server, context(images()), options(&again, "anthropic/1")).await;
    assert_eq!(paths(&server).len(), 4, "no upload after a restart");
    assert_eq!(sources(&server.requests.lock()[3].body), [json!("inline"), json!("file_1")]);
}

#[tokio::test]
async fn another_account_gets_its_own_upload() {
    let files = Arc::new(ImageFiles::new(None));
    let server = serve(vec![uploaded("file_a"), hello(), uploaded("file_b"), hello()]).await;
    ask(&server, context(vec![image(b"shot", Some("d1"))]), options(&files, "anthropic/1")).await;
    ask(&server, context(vec![image(b"shot", Some("d1"))]), options(&files, "anthropic/2")).await;
    let requests = server.requests.lock().clone();
    assert_eq!(paths(&server), ["POST /v1/files", "POST /v1/messages", "POST /v1/files", "POST /v1/messages"]);
    assert_eq!((sources(&requests[1].body), sources(&requests[3].body)), (vec![json!("file_a")], vec![json!("file_b")]));
}

#[tokio::test]
async fn a_file_anthropic_no_longer_has_sends_the_image_inline_once_and_for_good() {
    // Another conversation with the same image let go of it, say.
    let files = Arc::new(ImageFiles::new(None));
    let lost = served_json(404, json!({"type": "error", "error": {"type": "not_found_error", "message": "File not found: file_1"}}));
    let server = serve(vec![uploaded("file_1"), hello(), lost, hello(), uploaded("file_2"), hello()]).await;
    let shot = || image(b"shot", Some("d1"));
    ask(&server, context(vec![shot()]), options(&files, "anthropic/1")).await;
    ask(&server, context(vec![shot()]), options(&files, "anthropic/1")).await;
    // No loop, and the account still uploads what's new.
    ask(&server, context(vec![shot(), image(b"next", Some("d2"))]), options(&files, "anthropic/1")).await;
    let requests = server.requests.lock().clone();
    assert_eq!(
        paths(&server),
        ["POST /v1/files", "POST /v1/messages", "POST /v1/messages", "POST /v1/messages", "POST /v1/files", "POST /v1/messages"]
    );
    let sent: Vec<Vec<Value>> = [1, 2, 3, 5].iter().map(|index| sources(&requests[*index].body)).collect();
    assert_eq!(sent, [vec![json!("file_1")], vec![json!("file_1")], vec![json!("inline")], vec![json!("inline"), json!("file_2")]]);
}

#[tokio::test]
async fn files_refused_right_after_their_upload_send_the_accounts_images_inline() {
    let files = Arc::new(ImageFiles::new(None));
    let refused = served_json(400, json!({"type": "error", "error": {"type": "invalid_request_error", "message": "file source is not supported"}}));
    let server = serve(vec![uploaded("file_1"), refused, hello(), hello()]).await;
    ask(&server, context(vec![image(b"shot", Some("d1"))]), options(&files, "anthropic/1")).await;
    // Inline from then on, and a new image too, with no upload tried.
    ask(&server, context(vec![image(b"shot", Some("d1")), image(b"next", Some("d2"))]), options(&files, "anthropic/1")).await;
    let requests = server.requests.lock().clone();
    assert_eq!(paths(&server), ["POST /v1/files", "POST /v1/messages", "POST /v1/messages", "POST /v1/messages"]);
    assert_eq!(sources(&requests[1].body), [json!("file_1")]);
    assert_eq!(sources(&requests[2].body), [json!("inline")]);
    assert_eq!(sources(&requests[3].body), [json!("inline"), json!("inline")]);
}

#[tokio::test]
async fn an_upload_refused_sends_the_image_inline_and_it_stays_inline() {
    let files = Arc::new(ImageFiles::new(None));
    let refused = served_json(404, json!({"type": "error", "error": {"type": "not_found_error", "message": "Not found"}}));
    let server = serve(vec![refused, hello(), hello()]).await;
    ask(&server, context(vec![image(b"shot", Some("d1"))]), options(&files, "anthropic/1")).await;
    ask(&server, context(vec![image(b"shot", Some("d1")), image(b"next", Some("d2"))]), options(&files, "anthropic/1")).await;
    let requests = server.requests.lock().clone();
    assert_eq!(paths(&server), ["POST /v1/files", "POST /v1/messages", "POST /v1/messages"]);
    assert_eq!(sources(&requests[1].body), [json!("inline")]);
    assert_eq!(sources(&requests[2].body), [json!("inline"), json!("inline")]);
}

#[tokio::test]
async fn an_upload_that_fails_on_a_limit_sends_that_image_inline_for_good_and_uploads_the_next() {
    let files = Arc::new(ImageFiles::new(None));
    let busy = served_json(529, json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}));
    let server = serve(vec![busy, hello(), uploaded("file_2"), hello()]).await;
    ask(&server, context(vec![image(b"shot", Some("d1"))]), options(&files, "anthropic/1")).await;
    ask(&server, context(vec![image(b"shot", Some("d1")), image(b"next", Some("d2"))]), options(&files, "anthropic/1")).await;
    let requests = server.requests.lock().clone();
    assert_eq!(paths(&server), ["POST /v1/files", "POST /v1/messages", "POST /v1/files", "POST /v1/messages"]);
    assert_eq!(sources(&requests[3].body), [json!("inline"), json!("file_2")]);
}

#[tokio::test]
async fn without_files_kept_every_image_goes_inline() {
    let server = serve(vec![hello()]).await;
    let options = StreamOptions { api_key: Some("test-key".into()), ..StreamOptions::default() };
    ask(&server, context(vec![image(b"shot", Some("d1"))]), options).await;
    assert_eq!(paths(&server), ["POST /v1/messages"]);
    assert_eq!(sources(&server.requests.lock()[0].body), [json!("inline")]);
}

#[tokio::test]
async fn forgetting_an_image_gives_its_files_to_delete_and_the_next_request_uploads_it_again() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("anthropic-files.jsonl");
    let files = Arc::new(ImageFiles::new(Some(path.clone())));
    let server = serve(vec![uploaded("file_1"), uploaded("file_k"), hello(), uploaded("file_2"), hello()]).await;
    ask(&server, context(vec![image(b"shot", Some("d1")), image(b"keep", Some("d2"))]), options(&files, "anthropic/1")).await;
    let forgotten = |id: &str| Forgotten { account: "anthropic/1".into(), base_url: server.base_url.clone(), id: id.into() };
    assert_eq!(files.forget(&["d1".to_owned()]), [forgotten("file_1")]);
    // Forgotten after a restart too; the one kept stays.
    let again = Arc::new(ImageFiles::new(Some(path)));
    assert!(again.forget(&["d1".to_owned()]).is_empty());
    ask(&server, context(vec![image(b"shot", Some("d1"))]), options(&again, "anthropic/1")).await;
    assert_eq!(paths(&server).len(), 5);
    assert_eq!(sources(&server.requests.lock()[4].body), [json!("file_2")]);
    assert_eq!(again.forget(&["d2".to_owned()]), [forgotten("file_k")]);
}

fn registry(dir: &std::path::Path, variable: &'static str, value: &'static str) -> factory_agent::providers::models::Registry {
    use factory_agent::credentials::FileCredentialStore;
    use factory_agent::providers::models::{Registry, default_providers};
    let env: factory_agent::auth::Env = Arc::new(move |name| (name == variable).then(|| value.to_owned()));
    Registry::with_providers(FileCredentialStore::new(dir.join("auth.json")), env, default_providers())
        .keeping_image_files(Some(dir.join("anthropic-files.jsonl")))
}

fn profile(organization_type: &str) -> Served {
    served_json(200, json!({"account": {"email": "lee@example.com"}, "organization": {"organization_type": organization_type}}))
}

async fn ask_registry(registry: &factory_agent::providers::models::Registry, server: &Server) {
    let context = context(vec![image(b"shot", Some("d1"))]);
    let message = registry.stream_fn()(&model(server), context, StreamOptions::default()).expect("stream").result().await.expect("a message");
    assert_eq!(message.stop_reason, StopReason::Stop, "{:?}", message.error_message);
}

#[tokio::test]
async fn a_personal_subscription_keeps_files_and_deletes_them_when_forgotten_or_signed_out() {
    let dir = tempfile::tempdir().unwrap();
    let registry = registry(dir.path(), "ANTHROPIC_OAUTH_TOKEN", "sk-ant-oat01-test");
    let deleted = |id: &str| served_json(200, json!({"id": id, "type": "file_deleted"}));
    let responses =
        vec![profile("claude_max"), uploaded("file_1"), hello(), hello(), deleted("file_1"), uploaded("file_2"), hello(), deleted("file_2")];
    let server = serve(responses).await;
    ask_registry(&registry, &server).await;
    ask_registry(&registry, &server).await;
    registry.forget_images(&["d1".to_owned()]).await;
    // Signing out deletes what the account still has, with its own sign-in.
    ask_registry(&registry, &server).await;
    registry.logout("anthropic").await.unwrap();
    // Signed in again: whose it is is read anew (a Team's now: inline).
    let team = serve(vec![profile("claude_team"), hello()]).await;
    ask_registry(&registry, &team).await;
    assert_eq!(paths(&team), ["GET /api/oauth/profile", "POST /v1/messages"]);
    let requests = server.requests.lock().clone();
    assert_eq!(
        paths(&server),
        [
            "GET /api/oauth/profile",
            "POST /v1/files",
            "POST /v1/messages",
            "POST /v1/messages",
            "DELETE /v1/files/file_1",
            "POST /v1/files",
            "POST /v1/messages",
            "DELETE /v1/files/file_2"
        ]
    );
    assert_eq!(sources(&requests[3].body), [json!("file_1")]);
    let bearer = |index: usize| requests[index].headers.iter().any(|(name, value)| name == "authorization" && value == "Bearer sk-ant-oat01-test");
    assert!(bearer(1) && bearer(4) && bearer(7));
    // Kept beside the sign-ins, readable by this user alone.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join("anthropic-files.jsonl")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[tokio::test]
async fn an_api_key_a_team_and_an_unread_plan_send_images_inline() {
    // Files are reachable to everyone with access to the workspace (Fugo).
    for (variable, value, answers) in [
        ("ANTHROPIC_API_KEY", "sk-ant-api03-test", vec![hello()]),
        ("ANTHROPIC_OAUTH_TOKEN", "sk-ant-oat01-test", vec![profile("claude_team"), hello()]),
        ("ANTHROPIC_OAUTH_TOKEN", "sk-ant-oat01-test", vec![profile("claude_enterprise"), hello()]),
        ("ANTHROPIC_OAUTH_TOKEN", "sk-ant-oat01-test", vec![served_json(500, json!({})), hello()]),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let registry = registry(dir.path(), variable, value);
        let server = serve(answers).await;
        ask_registry(&registry, &server).await;
        let requests = server.requests.lock().clone();
        let last = requests.last().expect("the request");
        assert_eq!((last.path.as_str(), sources(&last.body)), ("/v1/messages", vec![json!("inline")]), "{variable}");
        assert!(!paths(&server).iter().any(|path| path.contains("/v1/files")), "{:?}", paths(&server));
    }
}
