//! qa-model: a scripted model on this computer, so agents in a sandbox run
//! without a real one. It speaks OpenRouter's wire (OpenAI's Chat
//! Completions, streamed), and a sandbox started with `--model` points its
//! OpenRouter provider here (`OPENROUTER_BASE_URL`, read through
//! `EnvApiKeyAuth.base_url_var`, with a made-up key). It reaches nothing.
//!
//! Its answers are deterministic: the first rule of the script that fits the
//! conversation's last message, else the built-in ones:
//! - a message with lines `call TOOL {json}` calls those tools;
//! - a tool's result is answered "TOOL answered: <its first line>";
//! - anything else is answered "OK: <its first line>" (or the script's
//!   `default`).
//!
//! It also serves OpenRouter's model list (`GET /api/v1/models`, #185 U4),
//! which a QA build of factoryd fetches from here
//! (`FACTORY_QA_OPENROUTER_MODELS_URL`): `openrouter-models.json` beside
//! the script, as OpenRouter's answer (`{"data": [...]}`), read again for
//! every fetch, else a built-in list (`default_models`).
//!
//! The script is read again for every request, so a test can change it
//! between steps. Every request and its answer go to the log, one JSON line
//! each, with the agent's system prompt and tools, so a test can check what
//! the agent was told.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use factory_agent::estimate::ImageRule;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

use crate::Failure;

pub mod claude;

/// The model a sandbox's agents use: in the OpenRouter catalog, without
/// reasoning, with a context too large to compact in a test.
pub const MODEL: &str = "openrouter/openai/gpt-4.1-mini";

/// What a script's rule answers with, when it fits.
#[derive(Debug, Default, PartialEq)]
struct Answer {
    text: String,
    calls: Vec<(String, Value)>,
}

/// The conversation's last message, as the rules look at it.
#[derive(Debug, Default)]
struct Last {
    /// "user" (a message or an event), "tool" (a result) or "assistant".
    role: String,
    /// Its text: for a message from the person or an event, what's inside
    /// its `<sent …>` or `<event …>` tag.
    text: String,
    /// As the agent got it, tag and all.
    raw: String,
    /// For a result: the tool that gave it.
    tool: String,
}

/// Checks a script: JSON, `{"default"?: text, "rules": [rule…]}`, each rule
/// valid. Returns what's wrong.
pub fn check_script(text: &str) -> Result<(), String> {
    let script: Value = serde_json::from_str(text).map_err(|error| format!("The script isn't JSON: {error}"))?;
    let Some(object) = script.as_object() else { return Err("The script is a JSON object: {\"rules\": [...]}".to_owned()) };
    for key in object.keys() {
        if !matches!(key.as_str(), "rules" | "default") {
            return Err(format!("The script has \"{key}\": it takes \"rules\" and \"default\"."));
        }
    }
    if object.get("default").is_some_and(|default| !default.is_string()) {
        return Err("\"default\" is the text of a reply.".to_owned());
    }
    let rules = match object.get("rules") {
        None => return Ok(()),
        Some(Value::Array(rules)) => rules,
        Some(_) => return Err("\"rules\" is a list.".to_owned()),
    };
    for (number, rule) in rules.iter().enumerate() {
        let Some(rule) = rule.as_object() else { return Err(format!("Rule {number} isn't an object.")) };
        for (key, value) in rule {
            let fine = match key.as_str() {
                "match" | "after" | "result" | "agent" | "reply" => value.is_string(),
                "regex" => value.as_str().is_some_and(|pattern| regex::Regex::new(pattern).is_ok()),
                "once" => value.is_boolean(),
                "calls" => value.as_array().is_some_and(|calls| {
                    calls.iter().all(|call| call.get("tool").is_some_and(Value::is_string) && call.get("args").is_none_or(Value::is_object))
                }),
                _ => {
                    return Err(format!(
                        "Rule {number} has \"{key}\": rules take match, regex, after, result, agent, once, reply and calls."
                    ));
                }
            };
            if !fine {
                return Err(format!("Rule {number}'s \"{key}\" isn't right (see claktory-qa's README)."));
            }
        }
        if !rule.contains_key("reply") && !rule.contains_key("calls") {
            return Err(format!("Rule {number} says nothing: give it \"reply\", \"calls\" or both."));
        }
    }
    Ok(())
}

/// The agent's name, from its system prompt's "You are <name>, a
/// persistent agent".
fn agent_of(system: &str) -> String {
    system
        .split("You are ")
        .skip(1)
        .find_map(|rest| rest.split_once(", a persistent agent").map(|(name, _)| name).filter(|name| !name.contains('\n')))
        .unwrap_or_default()
        .trim()
        .to_owned()
}

/// A message's content as text: a string, or the text of its parts.
fn text_of(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts.iter().filter_map(|part| part.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// What's inside a message's `<sent …>` or `<event …>` tag, trimmed; the
/// text as it is when it has none.
fn untagged(text: &str) -> String {
    let trimmed = text.trim();
    for tag in ["sent", "event"] {
        if trimmed.starts_with(&format!("<{tag} "))
            && let Some(open) = trimmed.find('>')
            && let Some(close) = trimmed.rfind(&format!("</{tag}>"))
            && close > open
        {
            return trimmed[open + 1..close].trim().to_owned();
        }
    }
    trimmed.to_owned()
}

/// The request's system prompt, last message and tools.
fn read_request(request: &Value) -> (String, Last, Vec<String>) {
    let messages = request["messages"].as_array().cloned().unwrap_or_default();
    let system = messages
        .iter()
        .filter(|message| matches!(message["role"].as_str(), Some("system" | "developer")))
        .map(|message| text_of(&message["content"]))
        .collect::<Vec<_>>()
        .join("\n");
    let mut last = Last::default();
    if let Some(message) = messages.last() {
        last.role = message["role"].as_str().unwrap_or_default().to_owned();
        last.raw = text_of(&message["content"]);
        last.text = if last.role == "user" { untagged(&last.raw) } else { last.raw.clone() };
        if last.role == "tool" {
            let id = message["tool_call_id"].as_str().unwrap_or_default();
            last.tool = messages
                .iter()
                .flat_map(|message| message["tool_calls"].as_array().cloned().unwrap_or_default())
                .find(|call| call["id"].as_str() == Some(id))
                .and_then(|call| call["function"]["name"].as_str().map(str::to_owned))
                .unwrap_or_default();
        }
    }
    let tools =
        request["tools"].as_array().into_iter().flatten().filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned)).collect();
    (system, last, tools)
}

/// The first rule that fits, by its number; rules marked `once` that were
/// used are skipped.
fn pick(script: &Value, system: &str, last: &Last, used: &HashSet<String>) -> Option<(usize, Answer)> {
    let rules = script["rules"].as_array()?;
    rules.iter().enumerate().find_map(|(number, rule)| {
        let text = |key: &str| rule.get(key).and_then(Value::as_str);
        if rule["once"].as_bool() == Some(true) && used.contains(&rule.to_string()) {
            return None;
        }
        if let Some(agent) = text("agent")
            && agent_of(system) != agent
        {
            return None;
        }
        let user = last.role == "user";
        if let Some(wanted) = text("match")
            && !(user && last.text.to_lowercase().contains(&wanted.to_lowercase()))
        {
            return None;
        }
        if let Some(pattern) = text("regex")
            && !(user && regex::Regex::new(pattern).is_ok_and(|regex| regex.is_match(&last.text)))
        {
            return None;
        }
        let result = last.role == "tool";
        if let Some(tool) = text("after")
            // Any case: Claude's sign-in names tools as Claude Code does (Bash).
            && !(result && (tool == "*" || tool.eq_ignore_ascii_case(&last.tool)))
        {
            return None;
        }
        if let Some(wanted) = text("result")
            && !(result && last.text.contains(wanted))
        {
            return None;
        }
        let calls = rule["calls"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|call| (call["tool"].as_str().unwrap_or_default().to_owned(), call.get("args").cloned().unwrap_or_else(|| json!({}))))
            .collect();
        Some((number, Answer { text: text("reply").unwrap_or_default().to_owned(), calls }))
    })
}

/// The built-in answers, for when no rule fits.
fn built_in(script: &Value, last: &Last) -> Answer {
    let first_line = |text: &str| text.lines().find(|line| !line.trim().is_empty()).unwrap_or("").chars().take(200).collect::<String>();
    if last.role == "user" {
        let calls: Vec<(String, Value)> = last
            .text
            .lines()
            .filter_map(|line| {
                let rest = line.trim().strip_prefix("call ")?;
                let (tool, args) = rest.split_once(' ').unwrap_or((rest, "{}"));
                let args = serde_json::from_str(args.trim()).ok().filter(Value::is_object)?;
                Some((tool.to_owned(), args))
            })
            .collect();
        if !calls.is_empty() {
            return Answer { text: String::new(), calls };
        }
    }
    if last.role == "tool" {
        return Answer { text: format!("{} answered: {}", last.tool, first_line(&last.text)), calls: Vec::new() };
    }
    match script["default"].as_str() {
        Some(default) => Answer { text: default.to_owned(), calls: Vec::new() },
        None => Answer { text: format!("OK: {}", first_line(&last.text)), calls: Vec::new() },
    }
}

/// The server's state between requests.
struct Model {
    script: PathBuf,
    log: PathBuf,
    /// With a fake Claude sign-in (`up --claude`): the file saying until
    /// when it's at its usage limit (`limit`).
    claude: Option<PathBuf>,
    used: Mutex<HashSet<String>>,
    count: Mutex<u64>,
}

/// Runs the model on `port` until it's stopped; with `claude`, Anthropic's
/// wire too, for the sandbox's fake Claude sign-in, at its usage limit while
/// that file says so.
pub async fn serve(port: u16, script: PathBuf, log: PathBuf, claude: Option<PathBuf>) -> Result<(), Failure> {
    let listener =
        TcpListener::bind(("127.0.0.1", port)).await.map_err(|error| Failure::failed(format!("Can't listen on {port}: {error}")))?;
    let model = Arc::new(Model { script, log, claude, used: Mutex::default(), count: Mutex::new(0) });
    loop {
        let Ok((stream, _)) = listener.accept().await else { continue };
        let model = model.clone();
        tokio::spawn(async move {
            let _ = handle(&model, stream).await;
        });
    }
}

/// Reads one HTTP request: its first line, its head (headers) and body.
async fn read_http(stream: &mut TcpStream) -> std::io::Result<(String, String, Vec<u8>)> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 16 * 1024];
    let end = loop {
        if let Some(at) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break at + 4;
        }
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&buffer[..end]).into_owned();
    let mut body = buffer[end..].to_vec();
    let header = |name: &str| {
        head.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim().eq_ignore_ascii_case(name).then(|| value.trim().to_owned())
        })
    };
    if header("transfer-encoding").is_some_and(|value| value.eq_ignore_ascii_case("chunked")) {
        // Chunked: read to the last chunk, then join the chunks.
        while !body.windows(5).any(|window| window == b"0\r\n\r\n") {
            let read = stream.read(&mut chunk).await?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..read]);
        }
        let mut joined = Vec::new();
        let mut rest = body.as_slice();
        while let Some(at) = rest.windows(2).position(|window| window == b"\r\n") {
            let size = usize::from_str_radix(String::from_utf8_lossy(&rest[..at]).trim(), 16).unwrap_or(0);
            if size == 0 {
                break;
            }
            let start = at + 2;
            joined.extend_from_slice(&rest[start..(start + size).min(rest.len())]);
            rest = &rest[(start + size + 2).min(rest.len())..];
        }
        body = joined;
    } else {
        let length: usize = header("content-length").and_then(|value| value.parse().ok()).unwrap_or(0);
        while body.len() < length {
            let read = stream.read(&mut chunk).await?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..read]);
        }
    }
    Ok((head.lines().next().unwrap_or_default().to_owned(), head, body))
}

async fn handle(model: &Model, mut stream: TcpStream) -> std::io::Result<()> {
    let (line, head, body) = read_http(&mut stream).await?;
    let mut words = line.split(' ');
    let (method, path) = (words.next().unwrap_or_default(), words.next().unwrap_or_default());
    if method == "POST" && path.ends_with("/chat/completions") {
        return chat_completions(model, &mut stream, &body).await;
    }
    if method == "GET" && path.split('?').next() == Some("/api/v1/models") {
        return models_list(model, &mut stream).await;
    }
    if let Some(limit) = &model.claude {
        return claude::handle(model, limit, &mut stream, method, path, &head, &body).await;
    }
    respond(&mut stream, 404, "", &json!({"error": {"message": "qa-model answers only POST …/chat/completions"}})).await
}

/// The models a fresh sandbox's list has: as many as factoryd takes for a
/// real list (50 at least), all able to run agents, the sandbox's own
/// model (`MODEL`) among them.
pub fn default_models() -> Value {
    let entry = |id: &str, name: &str| {
        json!({
            "id": id,
            "name": name,
            "context_length": 128_000,
            "architecture": {"input_modalities": ["text"], "output_modalities": ["text"]},
            "supported_parameters": ["tools", "max_tokens"],
            "top_provider": {"max_completion_tokens": 16_384},
            "pricing": {"prompt": "0.0000004", "completion": "0.0000016"},
        })
    };
    let mut data = vec![entry(MODEL.trim_start_matches("openrouter/"), "OpenAI: GPT-4.1 Mini")];
    data.extend((1..=55).map(|number| entry(&format!("qa/model-{number:02}"), &format!("QA Model {number:02}"))));
    json!({"data": data})
}

/// OpenRouter's model list: `openrouter-models.json` beside the script, as
/// it is (a QA agent edits it between fetches), else `default_models`.
async fn models_list(model: &Model, stream: &mut TcpStream) -> std::io::Result<()> {
    let file = model.script.with_file_name(MODELS_FILE);
    let list = match std::fs::read(&file) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(list) => list,
            Err(error) => {
                let message = format!("qa-model: {} isn't JSON: {error}", file.display());
                return respond(stream, 500, "", &json!({"error": {"message": message}})).await;
            }
        },
        Err(_) => default_models(),
    };
    // Not in the log: `model` lists requests to the model, and a refresh
    // between steps would push one out of `--last`.
    respond(stream, 200, "", &list).await
}

/// The file qa-model serves OpenRouter's model list from, beside its script.
pub const MODELS_FILE: &str = "openrouter-models.json";

/// A whole JSON answer, with `extra` header lines (each ending in CRLF).
async fn respond(stream: &mut TcpStream, status: u16, extra: &str, body: &Value) -> std::io::Result<()> {
    let body = body.to_string();
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        429 => "Too Many Requests",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.shutdown().await
}

/// The script's answer to a conversation, and the log's line for it:
/// the rule that fit (or "built-in"), or the script's mistake.
fn answer(model: &Model, system: &str, last: &Last, tools: &[String], wire: &str, extra: Value) -> Result<(u64, Answer), String> {
    let script_text = std::fs::read_to_string(&model.script).unwrap_or_else(|_| "{}".to_owned());
    let checked = check_script(&script_text);
    let script: Value = serde_json::from_str(&script_text).unwrap_or(Value::Null);
    let number = {
        let mut count = model.count.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        *count += 1;
        *count
    };
    let (rule, answer) = match &checked {
        Err(_) => (Value::Null, Answer::default()),
        Ok(()) => {
            let mut used = model.used.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            match pick(&script, system, last, &used) {
                Some((index, answer)) => {
                    used.insert(script["rules"][index].to_string());
                    (json!(index), answer)
                }
                None => (Value::from("built-in"), built_in(&script, last)),
            }
        }
    };
    let mut line = json!({
        "request": number,
        "wire": wire,
        "agent": agent_of(system),
        "last": { "role": last.role, "text": last.text, "tool": last.tool, "raw": last.raw },
        "tools": tools,
        "rule": rule,
        "reply": answer.text,
        "calls": answer.calls.iter().map(|(tool, args)| json!({"tool": tool, "args": args})).collect::<Vec<_>>(),
        "error": checked.as_ref().err(),
        "system": system,
    });
    if let (Some(line), Value::Object(extra)) = (line.as_object_mut(), extra) {
        line.extend(extra);
    }
    log(&model.log, &line);
    checked.map(|()| (number, answer))
}

async fn chat_completions(model: &Model, stream: &mut TcpStream, body: &[u8]) -> std::io::Result<()> {
    let request: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let (system, last, tools) = read_request(&request);
    let (number, answer) = match answer(model, &system, &last, &tools, "openrouter", json!({})) {
        Ok(answered) => answered,
        // The agent shows it as the model's error, and the log has it.
        Err(problem) => return respond(stream, 400, "", &json!({"error": {"message": format!("qa-model: {problem}"), "code": 400}})).await,
    };
    let model_id = request["model"].as_str().unwrap_or("qa-model").to_owned();
    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n").await?;
    let rule = ImageRule::for_model("openai-completions", &model_id);
    let prompt = input_tokens(body.len(), &request, rule, &|_| None);
    for event in events(&answer, number, &model_id, prompt) {
        stream.write_all(format!("data: {event}\n\n").as_bytes()).await?;
    }
    stream.write_all(b"data: [DONE]\n\n").await?;
    stream.shutdown().await
}

/// A request's input tokens as its provider counts them, so a sandbox's
/// context fills as a real one's: its text at four bytes a token, and each
/// image by its size under `rule`, never its base64 (#490: three
/// screenshots read as a million tokens and forced a compaction). `file`
/// gives the bytes of an image a request names by id (Anthropic's files).
pub(crate) fn input_tokens(body_len: usize, request: &Value, rule: ImageRule, file: &dyn Fn(&str) -> Option<Vec<u8>>) -> usize {
    use base64::Engine as _;
    fn cost(rule: ImageRule, bytes: Option<Vec<u8>>) -> usize {
        let size = bytes.as_deref().and_then(factory_agent::estimate::image_size);
        size.map_or(1200, |(width, height)| usize::try_from(factory_agent::estimate::image_tokens(rule, width, height)).unwrap_or(usize::MAX))
    }
    fn walk(value: &Value, rule: ImageRule, file: &dyn Fn(&str) -> Option<Vec<u8>>, base64: &mut usize, tokens: &mut usize) {
        let decode = |data: &str| base64::engine::general_purpose::STANDARD.decode(data).ok();
        match value {
            Value::Array(items) => items.iter().for_each(|item| walk(item, rule, file, base64, tokens)),
            Value::Object(fields) => {
                // Anthropic's: base64 or a file's id.
                if value["type"] == "image" {
                    match (value["source"]["type"].as_str(), value["source"]["data"].as_str(), value["source"]["file_id"].as_str()) {
                        (Some("base64"), Some(data), _) => {
                            *base64 += data.len();
                            *tokens += cost(rule, decode(data));
                        }
                        (Some("file"), _, Some(id)) => *tokens += cost(rule, file(id)),
                        _ => {}
                    }
                    return;
                }
                fields.values().for_each(|field| walk(field, rule, file, base64, tokens));
            }
            // OpenAI's and OpenRouter's: a data URL, wherever it is.
            Value::String(text) if text.starts_with("data:image/") => {
                *base64 += text.len();
                *tokens += cost(rule, text.split_once(";base64,").and_then(|(_, data)| decode(data)));
            }
            _ => {}
        }
    }
    let (mut base64, mut images) = (0, 0);
    walk(request, rule, file, &mut base64, &mut images);
    body_len.saturating_sub(base64) / 4 + images
}

/// The answer as Chat Completions chunks.
fn events(answer: &Answer, number: u64, model: &str, prompt: usize) -> Vec<Value> {
    let chunk = |delta: Value, finish: Value| {
        json!({"id": format!("qa-{number}"), "object": "chat.completion.chunk", "created": 0, "model": model,
               "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]})
    };
    let mut events = vec![chunk(json!({"role": "assistant", "content": ""}), Value::Null)];
    if !answer.text.is_empty() {
        events.push(chunk(json!({"content": answer.text}), Value::Null));
    }
    for (index, (tool, args)) in answer.calls.iter().enumerate() {
        let call = json!({"index": index, "id": format!("call_qa_{number}_{index}"), "type": "function",
                          "function": {"name": tool, "arguments": args.to_string()}});
        events.push(chunk(json!({"tool_calls": [call]}), Value::Null));
    }
    let finish = if answer.calls.is_empty() { "stop" } else { "tool_calls" };
    events.push(chunk(json!({}), Value::from(finish)));
    let completion = (answer.text.len() + answer.calls.iter().map(|(_, args)| args.to_string().len()).sum::<usize>()) / 4 + 1;
    events.push(json!({"id": format!("qa-{number}"), "object": "chat.completion.chunk", "created": 0, "model": model, "choices": [],
                       "usage": {"prompt_tokens": prompt, "completion_tokens": completion, "total_tokens": prompt + completion}}));
    events
}

fn log(path: &Path, line: &Value) {
    use std::io::Write as _;
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PNG of `width` × `height` that doesn't compress: as big as a
    /// screenshot's bytes get.
    fn noisy_png(width: u32, height: u32) -> Vec<u8> {
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let pixels = image::RgbImage::from_fn(width, height, |_, _| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            image::Rgb([state as u8, (state >> 8) as u8, (state >> 16) as u8])
        });
        let mut bytes = Vec::new();
        pixels.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).expect("png");
        bytes
    }

    /// #490: three screenshots cost what the provider charges for them, by
    /// their size, never their base64 (which read as a million tokens and
    /// forced a compaction after a restart).
    #[test]
    fn images_cost_what_their_provider_charges_never_their_base64() {
        use base64::Engine as _;
        let data = base64::engine::general_purpose::STANDARD.encode(noisy_png(800, 600));
        let text = "x".repeat(4000);
        let chat = json!({"model": "anthropic/claude-sonnet-4.5", "messages": [
            {"role": "user", "content": [{"type": "text", "text": text}]},
            {"role": "user", "content": (0..3).map(|_| json!({"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{data}")}})).collect::<Vec<_>>()},
        ]});
        let body = chat.to_string();
        assert!(body.len() / 4 > 1_000_000, "the base64 alone would read as {} tokens", body.len() / 4);
        let rule = ImageRule::for_model("openai-completions", "anthropic/claude-sonnet-4.5");
        let tokens = input_tokens(body.len(), &chat, rule, &|_| None);
        // Anthropic's: 800 × 600 / 750 = 640 an image, and the text.
        let rest = body.len() - 3 * format!("data:image/png;base64,{data}").len();
        assert_eq!(tokens, rest / 4 + 3 * 640);
        // OpenAI's: four 512-pixel tiles, 85 + 4 × 170 = 765.
        let openai = input_tokens(body.len(), &chat, ImageRule::for_model("openai-completions", "openai/gpt-5"), &|_| None);
        assert_eq!(openai, rest / 4 + 3 * 765);

        // Anthropic's Messages: base64 or a file's id, its bytes from `file`.
        let png = noisy_png(2400, 1800);
        let messages = json!({"messages": [{"role": "user", "content": [
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": base64::engine::general_purpose::STANDARD.encode(&png)}},
            {"type": "image", "source": {"type": "file", "file_id": "file_qa_1"}},
        ]}]});
        let body = messages.to_string();
        let tokens = input_tokens(body.len(), &messages, ImageRule::Anthropic, &|id| (id == "file_qa_1").then(|| png.clone()));
        // Scaled to about 1600 tokens each, its most.
        assert!(tokens < 3_400 && tokens > 3_000, "{tokens}");
    }

    fn last(role: &str, text: &str, tool: &str) -> Last {
        Last { role: role.to_owned(), text: text.to_owned(), tool: tool.to_owned(), raw: text.to_owned() }
    }

    #[test]
    fn the_person_and_events_are_read_inside_their_tags() {
        assert_eq!(untagged("<sent from=\"Mac\">\n\nHello\n\n</sent>"), "Hello");
        assert_eq!(untagged("<event source=\"pipe\" from=\"Launch\">\nLaunch is Friday.\n</event>"), "Launch is Friday.");
        assert_eq!(untagged("plain"), "plain");
    }

    #[test]
    fn rules_fit_in_order_and_the_built_ins_cover_the_rest() {
        let script = json!({"rules": [
            {"agent": "Max", "match": "launch", "reply": "Max knows."},
            {"match": "launch", "reply": "Launch is Friday."},
            {"regex": "^send (\\w+)$", "calls": [{"tool": "pipe_send", "args": {"text": "The plan."}}]},
            {"after": "pipe_send", "result": "Held", "reply": "It waits for you."},
            {"after": "*", "once": true, "reply": "First result."},
        ]});
        let none = HashSet::new();
        let system = "You are a coding agent.\nYou are Lea, a persistent agent in Claktory.";
        let picked = |last: &Last| pick(&script, system, last, &none).map(|(number, answer)| (number, answer.text, answer.calls));
        assert_eq!(picked(&last("user", "When's the LAUNCH?", "")), Some((1, "Launch is Friday.".to_owned(), Vec::new())));
        assert_eq!(pick(&script, "You are Max, a persistent agent", &last("user", "launch", ""), &none).map(|pair| pair.0), Some(0));
        let send = picked(&last("user", "send plan", "")).expect("a call");
        assert_eq!((send.0, send.2), (2, vec![("pipe_send".to_owned(), json!({"text": "The plan."}))]));
        assert_eq!(picked(&last("tool", "Held for Matt: …", "pipe_send")).map(|pair| pair.0), Some(3));
        assert_eq!(picked(&last("tool", "ran", "bash")).map(|pair| pair.0), Some(4));
        let used: HashSet<String> = [script["rules"][4].to_string()].into();
        assert_eq!(pick(&script, system, &last("tool", "ran", "bash"), &used), None);
        assert_eq!(picked(&last("user", "hello", "")), None);

        assert_eq!(
            built_in(&script, &last("user", "Please\ncall bash {\"command\": \"echo hi\"}\ncall pipe_status", "")),
            Answer {
                text: String::new(),
                calls: vec![("bash".to_owned(), json!({"command": "echo hi"})), ("pipe_status".to_owned(), json!({}))]
            }
        );
        assert_eq!(built_in(&script, &last("tool", "\nran\nmore", "bash")).text, "bash answered: ran");
        assert_eq!(built_in(&script, &last("user", "Hello there", "")).text, "OK: Hello there");
        assert_eq!(built_in(&json!({"default": "Sure."}), &last("user", "Hello", "")).text, "Sure.");
    }

    #[test]
    fn a_bad_script_says_what_is_wrong() {
        assert!(check_script(r#"{"rules": [{"match": "x", "reply": "y"}], "default": "z"}"#).is_ok());
        assert!(check_script("{}").is_ok());
        for (script, says) in [
            ("nope", "isn't JSON"),
            (r#"{"rule": []}"#, "\"rule\""),
            (r#"{"rules": [{"match": "x"}]}"#, "says nothing"),
            (r#"{"rules": [{"regex": "(", "reply": "y"}]}"#, "\"regex\""),
            (r#"{"rules": [{"when": "x", "reply": "y"}]}"#, "\"when\""),
            (r#"{"rules": [{"calls": [{"args": {}}]}]}"#, "\"calls\""),
        ] {
            let problem = check_script(script).expect_err(script);
            assert!(problem.contains(says), "{script}: {problem}");
        }
    }

    #[test]
    fn a_request_is_read_as_the_agent_sends_it() {
        let request = json!({
            "model": "openai/gpt-4.1-mini",
            "messages": [
                {"role": "system", "content": "You are a coding agent working in /tmp.\n\nYou are Lea, a persistent agent in Claktory."},
                {"role": "user", "content": [{"type": "text", "text": "call bash {}"}]},
                {"role": "assistant", "content": null, "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "bash", "arguments": "{}"}}]},
                {"role": "tool", "tool_call_id": "c1", "content": "ran"},
            ],
            "tools": [{"type": "function", "function": {"name": "bash"}}, {"type": "function", "function": {"name": "pipe_send"}}],
        });
        let (system, last, tools) = read_request(&request);
        assert_eq!(agent_of(&system), "Lea");
        assert_eq!((last.role.as_str(), last.text.as_str(), last.tool.as_str()), ("tool", "ran", "bash"));
        assert_eq!(tools, ["bash", "pipe_send"]);
        let events = events(&Answer { text: "Hi".into(), calls: vec![("bash".into(), json!({"command": "ls"}))] }, 3, "m", 100);
        assert_eq!(events[1]["choices"][0]["delta"]["content"], "Hi");
        assert_eq!(events[2]["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"], "{\"command\":\"ls\"}");
        assert_eq!(events[3]["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(events[4]["usage"]["prompt_tokens"], 100);
    }
}
