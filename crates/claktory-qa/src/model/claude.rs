//! qa-model as Claude, for a sandbox's fake Claude sign-in (`up --claude`):
//! Anthropic's wire (the Messages API, streamed) and the subscription's
//! account endpoints (usage, profile, the sign-in's token), answering only
//! the fake tokens claktory-qa writes into the sandbox. factoryd reaches it
//! through FACTORY_QA_ANTHROPIC_URL, which only a QA build of factoryd reads
//! (Fugo's guard, in factory-agent's providers/anthropic/qa.rs).
//!
//! The answers are the script's, as on OpenRouter's wire. While the limit
//! file (`limit`) says so, the account is at its usage limit: every message
//! is refused with a 429 and the usage-limit headers, saying when it resets,
//! and the usage reads 100% until then. After that time, or once the limit
//! is lifted (`limit --off`, as a plan upgrade would), it answers again.
//!
//! It keeps Anthropic's Files API too (#382), so a sandbox shows images
//! going to Claude once, as files, and named after: an upload is kept in
//! the sandbox's `claude-files/`, a message naming a file that isn't there
//! (deleted by hand, as Anthropic letting it go) is refused as Anthropic
//! refuses it, and each message with images logs which went as files and
//! how many inline.

use std::path::Path;

use serde_json::{Value, json};
use tokio::io::AsyncWriteExt as _;
use tokio::net::TcpStream;

use super::{Answer, Last, Model, agent_of, answer, log, respond, text_of, untagged};

/// The fake sign-in's tokens: obviously fake, never copied or made from
/// anything real. The access token reads as a subscription's
/// (`sk-ant-oat…`), which is what makes factory-agent use Claude's wire.
pub const ACCESS: &str = "sk-ant-oat01-qa-fake-access";
pub const REFRESH: &str = "sk-ant-ort01-qa-fake-refresh";
/// Who the fake account is.
pub const EMAIL: &str = "claude@example.test";

/// The model a sandbox's agents use with `--claude`: Claude's, without
/// reasoning.
pub const MODEL: &str = "anthropic/claude-haiku-4-5";

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(0))
}

/// The sandbox's auth.json: Claude signed in with the fake tokens, valid for
/// ten years, so nothing ever refreshes them.
pub fn credentials() -> Value {
    json!({"anthropic": {
        "type": "oauth",
        "access": ACCESS,
        "refresh": REFRESH,
        "expires": now_ms() + 10 * 365 * 24 * 3600 * 1000,
    }})
}

/// Until when (ms since 1970) the account is at its limit, if it still is.
pub fn limited_until(file: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(file).ok()?;
    let until = serde_json::from_str::<Value>(&text).ok()?["until_ms"].as_u64()?;
    (until > now_ms()).then_some(until)
}

/// Puts the account at its limit until `until` (ms since 1970), or lifts
/// it with None.
pub fn set_limit(file: &Path, until: Option<u64>) -> std::io::Result<()> {
    let text = match until {
        Some(until) => json!({"until_ms": until}).to_string(),
        None => "{}".to_owned(),
    };
    std::fs::write(file, text)
}

/// A time as RFC 3339, as Anthropic's usage gives its resets.
fn rfc3339(ms: u64) -> String {
    let secs = ms / 1000;
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // Days since 1970 to a civil date (Howard Hinnant's algorithm).
    let z = i64::try_from(days).unwrap_or(0) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3600, rest % 3600 / 60, rest % 60)
}

/// The usage the account reads: its five-hour window full while it's at its
/// limit, resetting then; else 10% used.
fn usage(limited: Option<u64>) -> Value {
    let now = now_ms();
    let (used, resets) = match limited {
        Some(until) => (100.0, until),
        None => (10.0, now + 5 * 3600 * 1000),
    };
    json!({
        "five_hour": {"utilization": used, "resets_at": rfc3339(resets)},
        "seven_day": {"utilization": 20.0, "resets_at": rfc3339(now + 7 * 24 * 3600 * 1000)},
    })
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

/// Answers one request on Claude's side.
pub(super) async fn handle(
    model: &Model,
    limit: &Path,
    stream: &mut TcpStream,
    method: &str,
    path: &str,
    head: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let error = |kind: &str, message: &str| json!({"type": "error", "error": {"type": kind, "message": message}});
    // The sign-in's token: only the fake refresh token gets the fake access token.
    if method == "POST" && path.starts_with("/v1/oauth/token") {
        let request: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        if request["refresh_token"] != REFRESH {
            return respond(
                stream,
                400,
                "",
                &json!({"error": "invalid_grant", "error_description": "qa-model: not the fake refresh token"}),
            )
            .await;
        }
        return respond(stream, 200, "", &json!({"access_token": ACCESS, "refresh_token": REFRESH, "expires_in": 31_536_000})).await;
    }
    if header(head, "authorization") != Some(&format!("Bearer {ACCESS}")) {
        return respond(stream, 401, "", &error("authentication_error", "qa-model answers only the sandbox's fake Claude sign-in")).await;
    }
    let limited = limited_until(limit);
    match (method, path.split('?').next().unwrap_or_default()) {
        ("GET", "/api/oauth/usage") => respond(stream, 200, "", &usage(limited)).await,
        ("GET", "/api/oauth/profile") => {
            let profile = json!({
                "account": {"uuid": "qa-claude-account", "email": EMAIL},
                "organization": {"uuid": "qa-claude-organization", "organization_type": "claude_max", "rate_limit_tier": "default_claude_max_5x"},
            });
            respond(stream, 200, "", &profile).await
        }
        ("GET", "/v1/models") => respond(stream, 200, "", &json!({"data": [], "has_more": false})).await,
        ("POST", "/v1/files") => {
            let Some((filename, mime_type, bytes)) = header(head, "content-type").and_then(|kind| uploaded(kind, body)) else {
                return respond(stream, 400, "", &error("invalid_request_error", "qa-model: no file in this upload")).await;
            };
            let files = files_dir(limit);
            let id = format!("file_qa_{}_{}", now_ms(), bytes.len());
            if let Err(problem) = std::fs::create_dir_all(&files).and_then(|()| std::fs::write(files.join(&id), &bytes)) {
                return respond(stream, 500, "", &error("api_error", &format!("qa-model: can't keep the file: {problem}"))).await;
            }
            log(&model.log, &json!({"wire": "anthropic", "uploaded": id, "bytes": bytes.len(), "mime_type": mime_type}));
            let file = json!({"type": "file", "id": id, "size_bytes": bytes.len(), "created_at": rfc3339(now_ms()),
                              "filename": filename, "mime_type": mime_type, "downloadable": false});
            respond(stream, 200, "", &file).await
        }
        ("DELETE", file) if file.starts_with("/v1/files/") => {
            let id = file.trim_start_matches("/v1/files/");
            let kept = files_dir(limit).join(id);
            if !valid_id(id) || std::fs::remove_file(&kept).is_err() {
                return respond(stream, 404, "", &error("not_found_error", &format!("File not found: {id}"))).await;
            }
            log(&model.log, &json!({"wire": "anthropic", "deleted": id}));
            respond(stream, 200, "", &json!({"id": id, "type": "file_deleted"})).await
        }
        ("POST", "/v1/messages") => {
            let request: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
            let (system, last, tools) = read_request(&request);
            let (files, inline) = images_of(&request);
            if files.len() + inline > 0 {
                let missing: Vec<&String> =
                    files.iter().filter(|id| !valid_id(id) || !files_dir(limit).join(id.as_str()).is_file()).collect();
                log(
                    &model.log,
                    &json!({"wire": "anthropic", "agent": agent_of(&system), "image_files": files, "inline_images": inline, "missing": missing}),
                );
                if let Some(id) = missing.first() {
                    return respond(stream, 404, "", &error("not_found_error", &format!("File not found: {id}"))).await;
                }
            }
            if let Some(until) = limited {
                log(
                    &model.log,
                    &json!({"wire": "anthropic", "agent": agent_of(&system), "last": {"role": last.role, "text": last.text, "tool": last.tool},
                            "limited_until_ms": until, "status": 429}),
                );
                let headers = format!(
                    "anthropic-ratelimit-unified-status: rejected\r\nanthropic-ratelimit-unified-reset: {}\r\n",
                    until.div_ceil(1000)
                );
                return respond(stream, 429, &headers, &error("rate_limit_error", "qa-model: this account is at its usage limit")).await;
            }
            let (number, answer) = match answer(model, &system, &last, &tools, "anthropic", json!({})) {
                Ok(answered) => answered,
                Err(problem) => return respond(stream, 400, "", &error("invalid_request_error", &format!("qa-model: {problem}"))).await,
            };
            let model_id = request["model"].as_str().unwrap_or("claude").to_owned();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nanthropic-ratelimit-unified-status: allowed\r\nConnection: close\r\n\r\n")
                .await?;
            let kept = files_dir(limit);
            let file = |id: &str| valid_id(id).then(|| std::fs::read(kept.join(id)).ok()).flatten();
            let input = super::input_tokens(body.len(), &request, factory_agent::estimate::ImageRule::Anthropic, &file);
            for event in events(&answer, number, &model_id, input) {
                stream.write_all(format!("event: {}\ndata: {event}\n\n", event["type"].as_str().unwrap_or_default()).as_bytes()).await?;
            }
            stream.shutdown().await
        }
        _ => respond(stream, 404, "", &error("not_found_error", &format!("qa-model doesn't answer {method} {path}"))).await,
    }
}

/// Where the sandbox's uploaded files are kept: beside its limit file.
fn files_dir(limit: &Path) -> std::path::PathBuf {
    limit.with_file_name("claude-files")
}

/// A file id qa-model could have made: nothing that reaches outside its folder.
fn valid_id(id: &str) -> bool {
    id.starts_with("file_qa_") && id.chars().all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// An upload's file (`multipart/form-data`, its part named "file"): its
/// filename, its type and its bytes.
fn uploaded(content_type: &str, body: &[u8]) -> Option<(String, String, Vec<u8>)> {
    let boundary = content_type.split(';').find_map(|part| part.trim().strip_prefix("boundary="))?.trim_matches('"');
    let delimiter = format!("--{boundary}");
    let find = |haystack: &[u8], needle: &[u8]| haystack.windows(needle.len()).position(|window| window == needle);
    let mut rest = body;
    while let Some(at) = find(rest, delimiter.as_bytes()) {
        rest = &rest[at + delimiter.len()..];
        let head_end = find(rest, b"\r\n\r\n")?;
        let head = String::from_utf8_lossy(&rest[..head_end]).into_owned();
        let content = &rest[head_end + 4..];
        let end = find(content, format!("\r\n{delimiter}").as_bytes()).unwrap_or(content.len());
        if head.contains("name=\"file\"") {
            let filename = head.split("filename=\"").nth(1).and_then(|rest| rest.split('"').next()).unwrap_or("file").to_owned();
            let mime_type = header(&head, "content-type").unwrap_or("application/octet-stream").to_owned();
            return Some((filename, mime_type, content[..end].to_vec()));
        }
        rest = &content[end..];
    }
    None
}

/// The images a Messages request carries, anywhere in its messages (a
/// tool's result included): the file ids it names, and how many go inline.
fn images_of(request: &Value) -> (Vec<String>, usize) {
    fn walk(value: &Value, files: &mut Vec<String>, inline: &mut usize) {
        match value {
            Value::Array(items) => items.iter().for_each(|item| walk(item, files, inline)),
            Value::Object(fields) => {
                if value["type"] == "image" {
                    match value["source"]["type"].as_str() {
                        Some("file") => files.extend(value["source"]["file_id"].as_str().map(str::to_owned)),
                        _ => *inline += 1,
                    }
                    return;
                }
                fields.values().for_each(|field| walk(field, files, inline));
            }
            _ => {}
        }
    }
    let (mut files, mut inline) = (Vec::new(), 0);
    walk(&request["messages"], &mut files, &mut inline);
    (files, inline)
}

/// A Messages request's system prompt, last message and tools, as the
/// rules look at them.
fn read_request(request: &Value) -> (String, Last, Vec<String>) {
    let system = text_of(&request["system"]);
    let messages = request["messages"].as_array().cloned().unwrap_or_default();
    let mut last = Last::default();
    if let Some(message) = messages.last() {
        let blocks = match &message["content"] {
            Value::String(text) => vec![json!({"type": "text", "text": text})],
            Value::Array(blocks) => blocks.clone(),
            _ => Vec::new(),
        };
        let result = blocks.iter().rev().find(|block| block["type"] == "tool_result");
        match result {
            // A tool's result, unless the person or an event wrote after it.
            Some(result) if blocks.last().is_some_and(|block| block["type"] == "tool_result") => {
                last.role = "tool".to_owned();
                last.text = text_of(&result["content"]);
                last.raw = last.text.clone();
                let id = &result["tool_use_id"];
                last.tool = messages
                    .iter()
                    .flat_map(|message| message["content"].as_array().cloned().unwrap_or_default())
                    .find(|block| block["type"] == "tool_use" && &block["id"] == id)
                    .and_then(|block| block["name"].as_str().map(str::to_owned))
                    .unwrap_or_default();
            }
            _ => {
                last.role = message["role"].as_str().unwrap_or_default().to_owned();
                last.raw = text_of(&Value::Array(blocks.into_iter().filter(|block| block["type"] == "text").collect()));
                last.text = if last.role == "user" { untagged(&last.raw) } else { last.raw.clone() };
            }
        }
    }
    let tools = request["tools"].as_array().into_iter().flatten().filter_map(|tool| tool["name"].as_str().map(str::to_owned)).collect();
    (system, last, tools)
}

/// The answer as Messages stream events.
fn events(answer: &Answer, number: u64, model: &str, input: usize) -> Vec<Value> {
    let output = (answer.text.len() + answer.calls.iter().map(|(_, args)| args.to_string().len()).sum::<usize>()) / 4 + 1;
    let mut events =
        vec![json!({"type": "message_start", "message": {"id": format!("msg_qa_{number}"), "type": "message", "role": "assistant",
        "model": model, "content": [], "usage": {"input_tokens": input, "output_tokens": 1}}})];
    let mut index = 0;
    if !answer.text.is_empty() {
        events.push(json!({"type": "content_block_start", "index": index, "content_block": {"type": "text", "text": ""}}));
        events.push(json!({"type": "content_block_delta", "index": index, "delta": {"type": "text_delta", "text": answer.text}}));
        events.push(json!({"type": "content_block_stop", "index": index}));
        index += 1;
    }
    for (call, (tool, args)) in answer.calls.iter().enumerate() {
        events.push(json!({"type": "content_block_start", "index": index,
            "content_block": {"type": "tool_use", "id": format!("toolu_qa_{number}_{call}"), "name": tool, "input": {}}}));
        events.push(
            json!({"type": "content_block_delta", "index": index, "delta": {"type": "input_json_delta", "partial_json": args.to_string()}}),
        );
        events.push(json!({"type": "content_block_stop", "index": index}));
        index += 1;
    }
    let stop = if answer.calls.is_empty() { "end_turn" } else { "tool_use" };
    events.push(json!({"type": "message_delta", "delta": {"stop_reason": stop}, "usage": {"output_tokens": output}}));
    events.push(json!({"type": "message_stop"}));
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_as_rfc_3339() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400_000), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_795_000_000_123), "2026-11-18T11:06:40Z");
    }

    #[test]
    fn the_limit_holds_until_it_resets_or_is_lifted() {
        let dir = std::env::temp_dir().join(format!("qa-claude-limit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let file = dir.join("claude.json");
        assert_eq!(limited_until(&file), None);
        let until = now_ms() + 60_000;
        set_limit(&file, Some(until)).expect("set");
        assert_eq!(limited_until(&file), Some(until));
        assert_eq!(usage(Some(until))["five_hour"]["utilization"], 100.0);
        set_limit(&file, Some(now_ms() - 1)).expect("past");
        assert_eq!(limited_until(&file), None, "past its reset");
        set_limit(&file, None).expect("off");
        assert_eq!(limited_until(&file), None);
        assert_eq!(usage(None)["five_hour"]["utilization"], 10.0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_upload_is_read_as_factory_agent_sends_it() {
        let body = b"--claktory-d1\r\nContent-Disposition: form-data; name=\"file\"; filename=\"image.png\"\r\nContent-Type: image/png\r\n\r\n\x89PNG\r\n--x\r\n--claktory-d1\r\nContent-Disposition: form-data; name=\"expires_in_seconds\"\r\n\r\n604800\r\n--claktory-d1--\r\n";
        let (filename, mime_type, bytes) = uploaded("multipart/form-data; boundary=claktory-d1", body).expect("a file");
        assert_eq!((filename.as_str(), mime_type.as_str()), ("image.png", "image/png"));
        assert_eq!(bytes, b"\x89PNG\r\n--x", "the bytes as they are, up to the next part");
        assert!(uploaded("multipart/form-data; boundary=other", body).is_none());
    }

    #[test]
    fn a_messages_requests_images_are_counted_as_files_or_inline() {
        let request = json!({"messages": [
            {"role": "user", "content": [{"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AA=="}}]},
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": [
                {"type": "text", "text": "a.png"},
                {"type": "image", "source": {"type": "file", "file_id": "file_qa_1_4"}},
            ]}]},
        ]});
        assert_eq!(images_of(&request), (vec!["file_qa_1_4".to_owned()], 1));
        assert!(valid_id("file_qa_1795_4"));
        assert!(!valid_id("file_qa_../../auth.json") && !valid_id("file_01abc"), "only ids it made, within its folder");
    }

    #[test]
    fn a_messages_request_is_read_as_the_agent_sends_it() {
        let request = json!({
            "model": "claude-haiku-4-5",
            "system": [{"type": "text", "text": "You are Claude Code, Anthropic's official CLI for Claude."},
                       {"type": "text", "text": "You are Lea, a persistent agent in Claktory."}],
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "<sent from=\"Mac\">\ncall bash {}\n</sent>"}]},
                {"role": "assistant", "content": [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": [{"type": "text", "text": "ran"}]}]},
            ],
            "tools": [{"name": "Bash"}, {"name": "pipe_send"}],
        });
        let (system, last, tools) = read_request(&request);
        assert_eq!(agent_of(&system), "Lea");
        assert_eq!((last.role.as_str(), last.text.as_str(), last.tool.as_str()), ("tool", "ran", "Bash"));
        assert_eq!(tools, ["Bash", "pipe_send"]);
        let mut asked = request.clone();
        asked["messages"] = json!([{"role": "user", "content": "<sent from=\"Mac\">\nHello\n</sent>"}]);
        let (_, last, _) = read_request(&asked);
        assert_eq!((last.role.as_str(), last.text.as_str()), ("user", "Hello"));

        let events = events(&Answer { text: "Hi".into(), calls: vec![("bash".into(), json!({"command": "ls"}))] }, 3, "m", 100);
        let types: Vec<&str> = events.iter().filter_map(|event| event["type"].as_str()).collect();
        assert_eq!(
            types,
            [
                "message_start",
                "content_block_start",
                "content_block_delta",
                "content_block_stop",
                "content_block_start",
                "content_block_delta",
                "content_block_stop",
                "message_delta",
                "message_stop"
            ]
        );
        assert_eq!(events[5]["delta"]["partial_json"], "{\"command\":\"ls\"}");
        assert_eq!(events[7]["delta"]["stop_reason"], "tool_use");
        assert_eq!(events[0]["message"]["usage"]["input_tokens"], 100);
    }
}
