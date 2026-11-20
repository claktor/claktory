macro_rules! spawn_stream {
    ($model:ident, $options:ident, $state:ident = $init:expr, $output:expr, $call:ident($($arg:expr),*)) => {{
        let (sink, stream) = $crate::stream::event_stream();
        let $model = $model.clone();
        tokio::spawn(async move {
            let mut $state = $init;
            let outcome = $call(&$model, $($arg,)* &mut $state, &sink).await;
            sink.finish($output, outcome, $options.cancel.is_cancelled());
        });
        Ok(stream)
    }};
}

pub mod anthropic;
pub mod catalog;
pub mod keys;
pub mod models;
pub mod openai;
pub mod openai_codex;
pub mod openai_responses;
pub mod openrouter;
pub mod sdk;

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use serde::Serialize;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::auth::{ApiKeyAuth, OAuthFlow};
use crate::errors::Error;
use crate::estimate::estimate_context;
use crate::stream::{EventSink, EventStream, LlmContext, StreamOptions, ThinkingLevel};
use crate::types::{AssistantMessage, Model, StopReason, StreamEvent, ToolResultPart};

const CONTEXT_SAFETY_TOKENS: u64 = 4096;
const PROMPT_CACHE_KEY_MAX_CHARS: usize = 64;

pub trait Api: Send + Sync {
    fn stream_simple(&self, model: &Model, context: LlmContext, options: StreamOptions) -> Result<EventStream, Error>;
}

impl<F> Api for F
where
    F: Fn(&Model, LlmContext, StreamOptions) -> Result<EventStream, Error> + Send + Sync,
{
    fn stream_simple(&self, model: &Model, context: LlmContext, options: StreamOptions) -> Result<EventStream, Error> {
        self(model, context, options)
    }
}

fn push(output: &AssistantMessage, sink: &EventSink, build: impl FnOnce(AssistantMessage) -> StreamEvent) {
    sink.push(build(output.clone()));
}

fn toolcall_delta(output: &AssistantMessage, sink: &EventSink, content_index: usize, delta: &str) {
    push(output, sink, |partial| StreamEvent::ToolcallDelta { content_index, delta: delta.to_owned(), partial });
}

async fn send_with_timeout(
    request: reqwest::RequestBuilder,
    timeout: Duration,
    cancel: &CancellationToken,
    timeout_error: &str,
    cancel_error: &str,
) -> Result<reqwest::Response, Error> {
    tokio::select! {
        sent = tokio::time::timeout(timeout, request.send()) => match sent {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) => Err(Error::Other(error.to_string())),
            Err(_) => Err(Error::Other(timeout_error.into())),
        },
        () = cancel.cancelled() => Err(Error::Other(cancel_error.into())),
    }
}

fn tool_text(content: &[ToolResultPart]) -> (String, bool) {
    let text = content
        .iter()
        .filter_map(|part| match part {
            ToolResultPart::Text { text } => Some(text.as_str()),
            ToolResultPart::Image { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    (text, content.iter().any(|part| matches!(part, ToolResultPart::Image { .. })))
}

fn user_content(content: impl Serialize) -> Value {
    json!({"role": "user", "content": content})
}

pub struct Provider {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    pub api_key: Option<Arc<dyn ApiKeyAuth>>,
    pub oauth: Option<Arc<dyn OAuthFlow>>,
    pub models: Vec<Model>,
    pub api: Option<Arc<dyn Api>>,
    /// Reads the usage limits of a subscription sign-in; None when the provider has none.
    pub limits: Option<crate::limits::LimitsFn>,
    /// Reads which plan a subscription sign-in has; None when the provider can't say.
    pub plan: Option<crate::limits::PlanFn>,
    /// Reads who a subscription sign-in is, to tell its accounts apart; None when the provider can't say.
    pub identity: Option<crate::limits::IdentityFn>,
    /// Uses one of a subscription sign-in's usage resets; None when the provider has none.
    pub reset: Option<crate::limits::ResetFn>,
    /// Asks the provider whether it accepts a new API key, before it's kept;
    /// None when it can't be asked.
    pub check_key: Option<keys::CheckKeyFn>,
}

fn api_key(auth: impl ApiKeyAuth + 'static) -> Option<Arc<dyn ApiKeyAuth>> {
    Some(Arc::new(auth))
}

fn oauth(auth: impl OAuthFlow + 'static) -> Option<Arc<dyn OAuthFlow>> {
    Some(Arc::new(auth))
}

impl Provider {
    fn new(
        id: &'static str,
        name: &'static str,
        base_url: &'static str,
        api_key: Option<Arc<dyn ApiKeyAuth>>,
        oauth: Option<Arc<dyn OAuthFlow>>,
        api: impl Api + 'static,
    ) -> Provider {
        Provider {
            id,
            name,
            base_url,
            api_key,
            oauth,
            models: catalog::models(id),
            api: Some(Arc::new(api)),
            limits: None,
            plan: None,
            identity: None,
            reset: None,
            check_key: None,
        }
    }
}

fn has_header(headers: &[(String, String)], name: &str) -> bool {
    headers.iter().any(|(key, value)| key.eq_ignore_ascii_case(name) && !value.trim().is_empty())
}

fn no_api_key(model: &Model) -> Error {
    Error::Other(format!("No API key for provider: {}", model.provider))
}

/// The configured key, a placeholder when an auth header stands in for it, or a missing-key error.
fn client_api_key(model: &Model, options: &StreamOptions, extra_headers: &[&str]) -> Result<String, Error> {
    if let Some(key) = &options.api_key {
        return Ok(key.clone());
    }
    if ["authorization", "cf-aig-authorization"].iter().chain(extra_headers).any(|name| has_header(&options.headers, name)) {
        return Ok("unused".into());
    }
    Err(no_api_key(model))
}

fn check_cancelled(options: &StreamOptions) -> Result<(), String> {
    if options.cancel.is_cancelled() {
        return Err("Request was aborted".into());
    }
    Ok(())
}

fn ephemeral() -> Value {
    json!({"type": "ephemeral"})
}

fn model_headers(model: &Model) -> Vec<(String, String)> {
    model.headers.iter().flatten().filter_map(|(name, value)| value.as_str().map(|value| (name.clone(), value.to_owned()))).collect()
}

fn model_params(model: &Model) -> Map<String, Value> {
    let mut params = Map::new();
    params.insert("model".into(), json!(model.id));
    params
}

fn prompt_cache_key(session: &str) -> String {
    session.chars().take(PROMPT_CACHE_KEY_MAX_CHARS).collect()
}

fn kernel_release() -> String {
    // SAFETY: uname fills a zeroed utsname; the release field is NUL-terminated by the kernel.
    unsafe {
        let mut name: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut name) != 0 {
            return String::new();
        }
        std::ffi::CStr::from_ptr(name.release.as_ptr()).to_string_lossy().into_owned()
    }
}

/// `pi (linux 6.1.0; x64)`: Node's `os.platform()`, `os.release()` and `os.arch()` names.
pub fn user_agent() -> &'static str {
    static AGENT: LazyLock<String> = LazyLock::new(|| {
        let arch = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            "x86" => "ia32",
            other => other,
        };
        format!("pi ({} {}; {arch})", std::env::consts::OS, kernel_release())
    });
    &AGENT
}

fn user_agent_header() -> (String, String) {
    ("User-Agent".into(), user_agent().into())
}

/// Later entries replace earlier ones whose name matches case-insensitively, keeping the later spelling.
pub fn merge_headers(base: &[(String, String)], overrides: &[(String, String)]) -> Vec<(String, String)> {
    let mut merged: Vec<(String, String)> = base.to_vec();
    for (name, value) in overrides {
        merged.retain(|(existing, _)| !existing.eq_ignore_ascii_case(name));
        merged.push((name.clone(), value.clone()));
    }
    merged
}

/// The TLS settings for connections Factory makes itself (the Codex WebSocket),
/// with their crypto provider named, never the process default: a program
/// linking more than one provider (factoryd has aws-lc-rs for remote access
/// beside reqwest's ring) has no default, and rustls panics looking for it.
pub fn tls_config() -> Arc<rustls::ClientConfig> {
    static CONFIG: LazyLock<Arc<rustls::ClientConfig>> = LazyLock::new(|| {
        let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .expect("ring supports rustls' default protocol versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
        Arc::new(config)
    });
    CONFIG.clone()
}

pub fn client() -> &'static reqwest::Client {
    static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| reqwest::Client::builder().build().unwrap_or_default());
    &CLIENT
}

/// `getSupportedThinkingLevels`: a level mapped to `null` is unsupported; `xhigh` and `max` need an explicit mapping.
pub fn supported_thinking_levels(model: &Model) -> Vec<ThinkingLevel> {
    if !model.reasoning {
        return vec![ThinkingLevel::Off];
    }
    ThinkingLevel::ALL
        .into_iter()
        .filter(|level| {
            let mapped = model.thinking_level_map.as_ref().and_then(|map| map.get(level.as_str()));
            match mapped {
                Some(Value::Null) => false,
                _ if matches!(level, ThinkingLevel::Xhigh | ThinkingLevel::Max) => mapped.is_some(),
                _ => true,
            }
        })
        .collect()
}

/// `clampThinkingLevel`: the nearest supported level above, else below.
pub fn clamp_thinking_level(model: &Model, level: ThinkingLevel) -> ThinkingLevel {
    let supported = supported_thinking_levels(model);
    if supported.contains(&level) {
        return level;
    }
    let index = ThinkingLevel::ALL.iter().position(|candidate| *candidate == level).unwrap_or(0);
    ThinkingLevel::ALL[index..]
        .iter()
        .chain(ThinkingLevel::ALL[..index].iter().rev())
        .copied()
        .find(|candidate| supported.contains(candidate))
        .unwrap_or(ThinkingLevel::Off)
}

pub(crate) fn count(value: &Value) -> u64 {
    value.as_u64().unwrap_or(0)
}

/// Record the provider's raw stop reason before mapping, then apply the mapped reason and error message.
pub(crate) fn set_stop_reason(
    output: &mut AssistantMessage,
    raw: Option<&str>,
    mapped: Result<(StopReason, Option<String>), String>,
    clear_error_message: bool,
) -> Result<(), String> {
    if let Some(raw) = raw {
        output.extra.insert("rawStopReason".into(), json!(raw));
    }
    let (stop_reason, error) = mapped?;
    output.stop_reason = stop_reason;
    if error.is_some() || clear_error_message {
        output.error_message = error;
    }
    Ok(())
}

pub(crate) fn set_response_id(output: &mut AssistantMessage, id: Option<&str>) {
    if let Some(id) = id {
        output.extra.insert("responseId".into(), json!(id));
    }
}

pub(crate) fn insert_reasoning_effort(params: &mut Map<String, Value>, effort: Value) {
    params.insert("reasoning".into(), json!({"effort": effort}));
}

pub(crate) async fn response_text(response: reqwest::Response) -> String {
    response.text().await.unwrap_or_default()
}

pub(crate) fn stream_effort(model: &Model, options: &StreamOptions) -> Option<ThinkingLevel> {
    options.reasoning.map(|level| clamp_thinking_level(model, level)).filter(|level| *level != ThinkingLevel::Off)
}

/// `clampMaxTokensToContext`: the model's cap, less the estimated context and a safety margin, never below one.
pub fn clamp_max_tokens(model: &Model, context: &LlmContext, max_tokens: u64) -> u64 {
    if model.context_window == 0 {
        return max_tokens.max(1);
    }
    let tools = serde_json::to_value(&context.tools).unwrap_or(Value::Null);
    let estimate = estimate_context(context.system_prompt.as_deref(), &context.messages, Some(&tools)).tokens;
    let available = model.context_window.saturating_sub(estimate).saturating_sub(CONTEXT_SAFETY_TOKENS);
    max_tokens.min(available.max(1))
}
