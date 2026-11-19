mod support;

use std::sync::Arc;

use factory_agent::providers::openai_codex::stream_simple;
use factory_agent::providers::openai_codex::websocket::{close_sessions, connection_count};
use factory_agent::stream::{EventStream, LlmContext, StreamOptions};
use factory_agent::types::{AssistantMessage, Message, StopReason, StreamEvent, UserMessage, same_json};
use parking_lot::Mutex;
use serde_json::{Value, json};
use support::codex_server::{Action, CodexServer, Script, serve};
use support::fixtures::message_json;
use support::server::{HANG, eventually};
use tokio_util::sync::CancellationToken;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/openai_codex_ws.json")).expect("fixture")
}

fn scenario(name: &str) -> Value {
    fixture()["scenarios"].as_array().expect("scenarios").iter().find(|scenario| scenario["name"] == name).cloned().expect(name)
}

fn luna(base_url: &str) -> factory_agent::types::Model {
    let mut model =
        factory_agent::providers::catalog::models("openai-codex").into_iter().find(|model| model.id == "gpt-5.6-luna").expect("luna");
    model.base_url = base_url.to_owned();
    model
}

fn text_events(id: &str, text: &str) -> Vec<Value> {
    let usage = json!({"input_tokens": 1200, "input_tokens_details": {"cached_tokens": 200}, "output_tokens": 80, "output_tokens_details": {"reasoning_tokens": 30}, "total_tokens": 1280});
    vec![
        json!({"type": "response.created", "response": {"id": id, "status": "in_progress"}}),
        json!({"type": "response.output_item.added", "output_index": 0, "item": {"type": "message", "id": format!("msg_{id}"), "role": "assistant", "status": "in_progress", "content": []}}),
        json!({"type": "response.output_text.delta", "output_index": 0, "delta": text}),
        json!({"type": "response.output_item.done", "output_index": 0, "item": {"type": "message", "id": format!("msg_{id}"), "role": "assistant", "status": "completed", "content": [{"type": "output_text", "text": text, "annotations": []}]}}),
        json!({"type": "response.completed", "response": {"id": id, "status": "completed", "usage": usage}}),
    ]
}

fn hello_events() -> Vec<Value> {
    text_events("resp_1", "Hello.")
}

/// A `response.created` followed by a terminal `response.{status}` frame.
fn terminal_response(status: &str, extra: Value) -> Vec<Value> {
    let mut response = json!({"id": "resp_1", "status": status});
    response.as_object_mut().expect("object").extend(extra.as_object().expect("object").clone());
    vec![
        json!({"type": "response.created", "response": {"id": "resp_1", "status": "in_progress"}}),
        json!({"type": format!("response.{status}"), "response": response}),
    ]
}

fn send_all(events: Vec<Value>) -> Vec<Action> {
    events.into_iter().map(Action::Send).collect()
}

/// Send the first `take` hello events, then perform `action`.
fn cut_off(action: &Action, take: usize) -> Vec<Action> {
    let mut actions = send_all(hello_events().into_iter().take(take).collect());
    actions.push(action.clone());
    actions
}

fn user(text: &str, timestamp: u64) -> Message {
    Message::User(UserMessage::text(text, timestamp))
}

fn fix() -> Vec<Message> {
    vec![user("Fix", 1)]
}

fn continue_with(first: &AssistantMessage) -> Vec<Message> {
    vec![user("Fix", 1), assistant(first), user("Continue", 3)]
}

async fn serve_with_sse(script: Script) -> CodexServer {
    serve(script, text_events("resp_sse", "From SSE.")).await
}

async fn run(server: &CodexServer, session: &str, messages: Vec<Message>) -> (Vec<StreamEvent>, AssistantMessage) {
    let token = fixture()["token"].as_str().expect("token").to_owned();
    let context =
        LlmContext { system_prompt: Some(fixture()["systemPrompt"].as_str().expect("prompt").to_owned()), messages, tools: Vec::new() };
    let options = StreamOptions { api_key: Some(token), session_id: Some(session.into()), ..StreamOptions::default() };
    let mut stream: EventStream = stream_simple(&luna(&server.base_url), context, options).expect("stream");
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event);
    }
    let message = match events.last().expect("terminal") {
        StreamEvent::Done { message, .. } | StreamEvent::Error { error: message, .. } => message.clone(),
        other => panic!("{other:?}"),
    };
    (events, message)
}

/// One request whose message is checked against the recorded request.
async fn run_checked(
    server: &CodexServer,
    session: &str,
    scenario: &Value,
    index: usize,
    messages: Vec<Message>,
) -> (Vec<StreamEvent>, AssistantMessage) {
    let (events, message) = run(server, session, messages).await;
    check_message(scenario, index, &message);
    (events, message)
}

/// The recurring checked two-turn conversation.
async fn run_conversation(server: &CodexServer, session: &str, scenario: &Value) {
    let (_, first) = run_checked(server, session, scenario, 0, fix()).await;
    run_checked(server, session, scenario, 1, continue_with(&first)).await;
}

fn assistant(message: &AssistantMessage) -> Message {
    let mut message = message.clone();
    message.timestamp = 2;
    Message::Assistant(message)
}

/// The recording's `requests[i].message` without the fields the test cannot reproduce byte for byte.
fn expected_message(scenario: &Value, index: usize) -> Value {
    let mut expected = scenario["requests"][index]["message"].clone();
    if let Some(diagnostics) = expected.get_mut("diagnostics").and_then(Value::as_array_mut) {
        for diagnostic in diagnostics {
            diagnostic.as_object_mut().expect("object").remove("timestamp");
        }
    }
    expected
}

fn actual_message(message: &AssistantMessage) -> Value {
    let mut actual = message_json(message);
    if let Some(diagnostics) = actual.get_mut("diagnostics").and_then(Value::as_array_mut) {
        for diagnostic in diagnostics {
            diagnostic.as_object_mut().expect("object").remove("timestamp");
            if let Some(details) = diagnostic.get_mut("details").and_then(Value::as_object_mut) {
                details.remove("requestBytes");
            }
        }
    }
    if let Some(diagnostics) = actual.get("diagnostics").cloned() {
        actual["diagnostics"] = diagnostics;
    }
    actual
}

fn check_message(scenario: &Value, index: usize, message: &AssistantMessage) {
    let mut expected = expected_message(scenario, index);
    if let Some(diagnostics) = expected.get_mut("diagnostics").and_then(Value::as_array_mut) {
        for diagnostic in diagnostics {
            if let Some(details) = diagnostic.get_mut("details").and_then(Value::as_object_mut) {
                details.remove("requestBytes");
            }
        }
    }
    let actual = actual_message(message);
    assert!(same_json(&actual, &expected), "{}: request {index}: {actual} != {expected}", scenario["name"]);
}

fn check_frames(scenario: &Value, server: &CodexServer) {
    let recorded = server.recorded.lock();
    let expected = scenario["connections"].as_array().expect("connections");
    assert_eq!(recorded.connections.len(), expected.len(), "{}: connections", scenario["name"]);
    for (connection, expected) in recorded.connections.iter().zip(expected) {
        let frames: Vec<Value> = expected["frames"].as_array().expect("frames").clone();
        assert_eq!(connection.frames.len(), frames.len(), "{}: frames", scenario["name"]);
        for (frame, expected) in connection.frames.iter().zip(&frames) {
            assert!(same_json(frame, expected), "{}: frame {frame} != {expected}", scenario["name"]);
        }
        let header = |name: &str| connection.headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str());
        for name in ["authorization", "chatgpt-account-id", "openai-beta", "originator", "session-id", "x-client-request-id"] {
            assert_eq!(header(name), expected["headers"][name].as_str(), "{}: header {name}", scenario["name"]);
        }
        assert_eq!(header("user-agent"), Some(factory_agent::providers::user_agent()));
        assert_eq!(header("accept"), None);
        assert_eq!(header("content-type"), None);
    }
    let sse = scenario["sseRequests"].as_array().expect("sse");
    assert_eq!(recorded.sse.len(), sse.len(), "{}: sse requests", scenario["name"]);
    for (request, expected) in recorded.sse.iter().zip(sse) {
        let header = |name: &str| request.headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str());
        assert_eq!(header("openai-beta"), Some("responses=experimental"));
        assert_eq!(header("accept"), Some("text/event-stream"));
        assert_eq!(header("session-id"), expected["headers"]["session-id"].as_str());
        assert_eq!(header("content-encoding"), None);
        assert_eq!(request.body["input"].as_array().map(Vec::len), expected["body"]["input"].as_array().map(Vec::len));
    }
}

/// Let the connections drain, every frame the scenario expects in, then verify them.
async fn settle(scenario: &Value, server: &CodexServer) {
    let expected: Vec<usize> = scenario["connections"]
        .as_array()
        .expect("connections")
        .iter()
        .map(|connection| connection["frames"].as_array().expect("frames").len())
        .collect();
    eventually("the frames are in", || {
        let recorded = server.recorded.lock();
        recorded.connections.len() >= expected.len()
            && recorded.connections.iter().zip(&expected).all(|(connection, frames)| connection.frames.len() >= *frames)
    })
    .await;
    check_frames(scenario, server);
}

/// Settle, then report how the first connection closed, once it has.
async fn finish(scenario: &Value, server: &CodexServer) -> Option<(u16, String)> {
    settle(scenario, server).await;
    eventually("the first connection closes", || closed(server, 0).is_some()).await;
    closed(server, 0)
}

/// Settle, then assert the clean `(1000, "done")` close.
async fn finish_done(scenario: &Value, server: &CodexServer) {
    assert_eq!(finish(scenario, server).await, Some((1000, "done".into())));
}

fn closed(server: &CodexServer, index: usize) -> Option<(u16, String)> {
    server.recorded.lock().connections[index].closed.clone()
}

#[tokio::test]
async fn two_requests_reuse_the_socket_and_the_second_sends_a_delta() {
    let scenario = scenario("two requests reuse the socket and the second sends a delta");
    let script: Script = Arc::new(|_, frame, _| {
        send_all(text_events(if frame == 0 { "resp_1" } else { "resp_2" }, if frame == 0 { "Hello." } else { "Again." }))
    });
    let server = serve(script, Vec::new()).await;
    let (events, first) = run_checked(&server, "s1", &scenario, 0, fix()).await;
    assert!(matches!(events.first(), Some(StreamEvent::Start { .. })));
    let (_, _second) = run_checked(&server, "s1", &scenario, 1, continue_with(&first)).await;
    assert_eq!(connection_count("s1", "acct_123"), 1);
    close_sessions(Some("s1"));
    assert_eq!(finish(&scenario, &server).await, Some((1000, "debug_close".into())));
    assert_eq!(connection_count("s1", "acct_123"), 0);
}

#[tokio::test]
async fn a_different_session_opens_its_own_socket() {
    let scenario = scenario("a different session opens its own socket");
    let script: Script = Arc::new(|connection, _, _| send_all(text_events(&format!("resp_{connection}"), "Hi.")));
    let server = serve(script, Vec::new()).await;
    run_checked(&server, "s2", &scenario, 0, fix()).await;
    run_checked(&server, "s3", &scenario, 1, fix()).await;
    close_sessions(Some("s2"));
    close_sessions(Some("s3"));
    settle(&scenario, &server).await;
}

#[tokio::test]
async fn previous_response_not_found_retries_once_with_the_full_body() {
    let scenario = scenario("previous_response_not_found retries once with the full body");
    let script: Script = Arc::new(|_, frame, body| {
        if body.get("previous_response_id").is_some() {
            return vec![Action::Send(
                json!({"type": "error", "code": "previous_response_not_found", "message": "Previous response not found"}),
            )];
        }
        send_all(text_events(&format!("resp_{frame}"), if frame == 0 { "Hello." } else { "Again." }))
    });
    let server = serve(script, Vec::new()).await;
    run_conversation(&server, "s4", &scenario).await;
    close_sessions(Some("s4"));
    finish_done(&scenario, &server).await;
}

#[tokio::test]
async fn a_connection_limit_error_before_the_first_event_retries_once() {
    let scenario = scenario("a connection-limit error before the first event retries once");
    let errored = Arc::new(Mutex::new(false));
    let flag = errored.clone();
    let script: Script = Arc::new(move |_, _, _| {
        let mut errored = flag.lock();
        if !*errored {
            *errored = true;
            return vec![Action::Send(
                json!({"type": "error", "code": "websocket_connection_limit_reached", "message": "Too many sockets"}),
            )];
        }
        send_all(hello_events())
    });
    let server = serve(script, Vec::new()).await;
    run_checked(&server, "s5", &scenario, 0, fix()).await;
    close_sessions(Some("s5"));
    finish_done(&scenario, &server).await;
}

#[tokio::test]
async fn a_close_before_the_first_event_falls_back_to_sse_for_the_session() {
    let scenario = scenario("a close before the first event falls back to SSE for the session");
    let script: Script = Arc::new(|_, _, _| vec![Action::Close(1011, "server going away".into())]);
    let server = serve(script, text_events("resp_sse_1", "From SSE.")).await;
    let (_, first) = run_checked(&server, "s6", &scenario, 0, fix()).await;
    let (_, second) = run(&server, "s6", continue_with(&first)).await;
    assert_eq!(second.stop_reason, StopReason::Stop);
    assert!(second.extra.get("diagnostics").is_none());
    assert_eq!(server.recorded.lock().connections.len(), 1);
    assert_eq!(server.recorded.lock().sse.len(), 2);
    let frames_only = scenario.clone();
    check_frames(&frames_only, &server);
}

#[tokio::test]
async fn a_message_too_big_close_before_the_first_event_falls_back_to_sse() {
    let scenario = scenario("a message-too-big close before the first event falls back to SSE");
    let script: Script = Arc::new(|_, _, _| vec![Action::Close(1009, String::new())]);
    let server = serve_with_sse(script).await;
    run_checked(&server, "s7", &scenario, 0, fix()).await;
    check_frames(&scenario, &server);
}

#[tokio::test]
async fn a_close_after_events_started_is_an_error_and_marks_the_session() {
    let scenario = scenario("a close after events started is an error and marks the session");
    let script: Script = Arc::new(|_, _, _| cut_off(&Action::Close(1011, "mid-stream".into()), 3));
    let server = serve_with_sse(script).await;
    let (events, _first) = run_checked(&server, "s8", &scenario, 0, fix()).await;
    assert!(events.iter().any(|event| matches!(event, StreamEvent::TextDelta { .. })));
    run_checked(&server, "s8", &scenario, 1, fix()).await;
    check_frames(&scenario, &server);
}

#[tokio::test]
async fn a_protocol_error_is_not_retried_on_the_socket() {
    let scenario = scenario("a protocol error is not retried on the socket");
    let script: Script = Arc::new(|_, _, _| vec![Action::SendText("{not json".into())]);
    let server = serve_with_sse(script).await;
    let (_, message) = run(&server, "s9", fix()).await;
    assert_eq!(message.stop_reason, StopReason::Error);
    assert!(message.error_message.as_deref().unwrap().starts_with("Invalid Codex WebSocket JSON: "), "{:?}", message.error_message);
    assert!(message.extra.get("diagnostics").is_none());
    finish_done(&scenario, &server).await;
}

#[tokio::test]
async fn response_failed_on_the_socket_is_a_final_error() {
    let scenario = scenario("response.failed on the socket is a final error");
    let script: Script =
        Arc::new(|_, _, _| send_all(terminal_response("failed", json!({"error": {"code": "server_error", "message": "boom"}}))));
    let server = serve_with_sse(script).await;
    run_checked(&server, "s10", &scenario, 0, fix()).await;
    finish_done(&scenario, &server).await;
}

#[tokio::test]
async fn a_socket_closed_by_the_server_between_requests_is_replaced() {
    let scenario = scenario("a socket closed by the server between requests is replaced");
    let script: Script = Arc::new(|connection, _, _| {
        let mut actions = send_all(text_events(&format!("resp_{connection}"), "Hello."));
        if connection == 0 {
            actions.push(Action::CloseLater(20, 1000, "idle".into()));
        }
        actions
    });
    let server = serve(script, Vec::new()).await;
    let (_, first) = run(&server, "s11", fix()).await;
    eventually("the closed socket is dropped", || connection_count("s11", "acct_123") == 0).await;
    let (_, second) = run(&server, "s11", continue_with(&first)).await;
    check_message(&scenario, 0, &first);
    check_message(&scenario, 1, &second);
    close_sessions(Some("s11"));
    settle(&scenario, &server).await;
}

#[tokio::test]
async fn a_failed_completion_on_the_socket_moves_the_session_to_sse() {
    let script: Script =
        Arc::new(|_, _, _| send_all(terminal_response("incomplete", json!({"incomplete_details": {"reason": "content_filter"}}))));
    let server = serve_with_sse(script).await;
    let (_, first) = run(&server, "s13", fix()).await;
    assert_eq!(first.stop_reason, StopReason::Error);
    assert_eq!(first.error_message.as_deref(), Some("Response incomplete: content_filter"));
    assert_eq!(
        actual_message(&first)["diagnostics"],
        json!([{"type": "provider_transport_failure", "error": {"name": "Error", "message": "Response incomplete: content_filter"}, "details": {"configuredTransport": "auto", "eventsEmitted": true, "phase": "after_message_stream_start"}}])
    );
    let (_, second) = run(&server, "s13", fix()).await;
    assert_eq!(second.stop_reason, StopReason::Stop);
    let recorded = server.recorded.lock();
    assert_eq!((recorded.connections.len(), recorded.connections[0].frames.len(), recorded.sse.len()), (1, 1, 1));
}

async fn stream_cut_off_by(action: Action, session: &str) -> AssistantMessage {
    let script: Script = Arc::new(move |_, _, _| cut_off(&action, 3));
    let server = serve(script, Vec::new()).await;
    let (_, message) = tokio::time::timeout(HANG, run(&server, session, fix())).await.expect("the stream ends");
    message
}

#[tokio::test]
async fn abnormal_socket_closures_report_the_browser_texts() {
    let dropped = stream_cut_off_by(Action::Drop, "s14").await;
    assert_eq!(dropped.error_message.as_deref(), Some("WebSocket error"));
    assert_eq!(actual_message(&dropped)["diagnostics"][0]["error"], json!({"name": "Error", "message": "WebSocket error"}));
    let unstated = stream_cut_off_by(Action::CloseWithoutStatus, "s15").await;
    assert_eq!(unstated.error_message.as_deref(), Some("WebSocket closed 1005"));
    assert_eq!(
        actual_message(&unstated)["diagnostics"][0]["error"],
        json!({"name": "WebSocketCloseError", "message": "WebSocket closed 1005", "code": 1005})
    );
}

#[tokio::test]
async fn cancellation_during_the_socket_stream_aborts_and_closes_it() {
    let script: Script = Arc::new(|_, _, _| send_all(hello_events().into_iter().take(2).collect()));
    let server = serve(script, Vec::new()).await;
    let token = fixture()["token"].as_str().expect("token").to_owned();
    let cancel = CancellationToken::new();
    let options =
        StreamOptions { api_key: Some(token), session_id: Some("s12".into()), cancel: cancel.clone(), ..StreamOptions::default() };
    let context = LlmContext { system_prompt: Some("x".into()), messages: vec![user("Fix", 1)], tools: Vec::new() };
    let mut stream = stream_simple(&luna(&server.base_url), context, options).expect("stream");
    // Once the client has read the response starting, the server's first event.
    let mut last = None;
    while let Some(event) = tokio::time::timeout(HANG, stream.next()).await.expect("the stream ends") {
        if matches!(event, StreamEvent::Start { .. }) {
            cancel.cancel();
        }
        last = Some(event);
    }
    let Some(StreamEvent::Error { error, .. }) = last else { panic!("{last:?}") };
    assert_eq!(error.stop_reason, StopReason::Aborted);
    assert_eq!(error.error_message.as_deref(), Some("Request was aborted"));
    assert!(error.extra.get("diagnostics").is_none());
    eventually("the socket closes", || connection_count("s12", "acct_123") == 0 && closed(&server, 0).is_some()).await;
}
