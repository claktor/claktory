use serde_json::Value;

use crate::types::Model;

const OPENAI: &str = include_str!("data/openai.json");
const OPENAI_CODEX: &str = include_str!("data/openai-codex.json");
const OPENROUTER: &str = include_str!("data/openrouter.json");
const ANTHROPIC: &str = include_str!("data/anthropic.json");

/// Model data is grouped by API in one file per provider.
pub fn models(provider: &str) -> Vec<Model> {
    let text = match provider {
        "openai" => OPENAI,
        "openai-codex" => OPENAI_CODEX,
        "openrouter" => OPENROUTER,
        "anthropic" => ANTHROPIC,
        _ => return Vec::new(),
    };
    let groups: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    groups
        .as_object()
        .into_iter()
        .flat_map(|groups| groups.values())
        .filter_map(Value::as_object)
        .flat_map(|group| group.values())
        .filter_map(|model| serde_json::from_value(model.clone()).ok())
        .collect()
}

/// Whether `provider`'s model `id` caches for a lifetime it's asked for
/// (OpenAI's GPT-5.6 and later; `openai::has_cache_lifetime`). Read once.
pub fn has_cache_lifetime(provider: &str, id: &str) -> bool {
    static KNOWN: std::sync::OnceLock<hashbrown::HashSet<String>> = std::sync::OnceLock::new();
    KNOWN
        .get_or_init(|| {
            ["openai", "openai-codex"]
                .into_iter()
                .flat_map(|provider| models(provider).into_iter().map(move |model| (provider, model)))
                .filter(|(_, model)| crate::providers::openai::has_cache_lifetime(model))
                .map(|(provider, model)| format!("{provider}/{}", model.id))
                .collect()
        })
        .contains(&format!("{provider}/{id}"))
}

/// OpenRouter's API, where its models run.
pub const OPENROUTER_BASE: &str = "https://openrouter.ai/api/v1";
/// Where OpenRouter lists its models; public, no sign-in needed.
pub const OPENROUTER_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";
/// The most a model list may be: OpenRouter's is about 1 MB.
pub const OPENROUTER_LIST_LIMIT: usize = 8 * 1024 * 1024;
/// A list with fewer models than this is taken for a broken one and kept
/// out: the bundled list stays.
const OPENROUTER_LIST_MIN: usize = 50;
const ID_LIMIT: usize = 128;
const NAME_LIMIT: usize = 200;
/// What a model's answer may be capped at when OpenRouter doesn't say.
const DEFAULT_MAX_TOKENS: u64 = 4096;

/// QA only (`qa-provider`): where qa-model serves OpenRouter's model list,
/// set once by factoryd after Fugo's guard (`anthropic::qa::models_url_for`).
#[cfg(feature = "qa-provider")]
static QA_MODELS_URL: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// QA only: OpenRouter's model list comes from `url`, an address on this
/// computer the guard let through, instead of openrouter.ai.
#[cfg(feature = "qa-provider")]
pub fn use_qa_models_url(url: String) {
    let _ = QA_MODELS_URL.set(url);
}

/// Whether OpenRouter's list comes from qa-model (a QA build only): then
/// it's read whatever OPENROUTER_BASE_URL says, with no sign-in, as
/// openrouter.ai's is public.
pub fn qa_models() -> bool {
    #[cfg(feature = "qa-provider")]
    return QA_MODELS_URL.get().is_some();
    #[cfg(not(feature = "qa-provider"))]
    false
}

/// Where the list comes from, and whether only HTTPS will do: openrouter.ai,
/// or in a QA build qa-model on this computer.
fn models_url() -> (&'static str, bool) {
    #[cfg(feature = "qa-provider")]
    if let Some(url) = QA_MODELS_URL.get() {
        return (url.as_str(), false);
    }
    (OPENROUTER_MODELS_URL, true)
}

/// Fetches OpenRouter's model list: over HTTPS from openrouter.ai only
/// (redirects aren't followed), within `OPENROUTER_LIST_LIMIT` and a
/// timeout. The body as it came, for `parse_openrouter`.
pub async fn fetch_openrouter() -> Result<Vec<u8>, String> {
    let (url, https_only) = models_url();
    let client = reqwest::Client::builder()
        .use_preconfigured_tls((*crate::providers::tls_config()).clone())
        .redirect(reqwest::redirect::Policy::none())
        .https_only(https_only)
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|error| format!("could not make a client: {error}"))?;
    let mut response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::USER_AGENT, crate::providers::user_agent())
        .send()
        .await
        .map_err(|error| format!("could not reach OpenRouter: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("OpenRouter answered {}", response.status()));
    }
    if response.content_length().is_some_and(|length| length > OPENROUTER_LIST_LIMIT as u64) {
        return Err("OpenRouter's model list is too big".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| format!("could not read OpenRouter's model list: {error}"))? {
        if body.len() + chunk.len() > OPENROUTER_LIST_LIMIT {
            return Err("OpenRouter's model list is too big".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// OpenRouter's model list (`GET /api/v1/models`) as models agents can run
/// on: those that take tools and answer in text through Chat Completions,
/// in id order. A model the bundled list has keeps its bundled entry whole;
/// a new one gets its fields from the listing, with OpenRouter's own API,
/// address and provider, which nothing in the listing can change. Fails,
/// and nothing of it is used, unless it's a list of at least
/// `OPENROUTER_LIST_MIN` such models.
pub fn parse_openrouter(body: &[u8]) -> Result<Vec<Model>, String> {
    if body.len() > OPENROUTER_LIST_LIMIT {
        return Err("the list is too big".into());
    }
    let listing: Value = serde_json::from_slice(body).map_err(|error| format!("the list isn't JSON: {error}"))?;
    let entries = listing.get("data").and_then(Value::as_array).ok_or("the list has no models")?;
    let bundled: hashbrown::HashMap<String, Model> = models("openrouter").into_iter().map(|model| (model.id.clone(), model)).collect();
    let mut listed = std::collections::BTreeMap::new();
    for entry in entries {
        let Some(id) = entry.get("id").and_then(Value::as_str) else { continue };
        if !valid_id(id) || !runs_agents(entry) {
            continue;
        }
        let model = match bundled.get(id) {
            Some(model) => model.clone(),
            None => match listed_model(id, entry) {
                Some(model) => model,
                None => continue,
            },
        };
        listed.insert(id.to_owned(), model);
    }
    if listed.len() < OPENROUTER_LIST_MIN {
        return Err(format!("the list has only {} models agents can run on", listed.len()));
    }
    Ok(listed.into_values().collect())
}

/// One model of OpenRouter's list, for an id typed in (friend bug #14):
/// the bundled entry, else one from its listing; why not, in a sentence,
/// when OpenRouter doesn't list it or an agent can't run on it.
pub fn openrouter_model(body: &[u8], id: &str) -> Result<Model, String> {
    let listing: Value = serde_json::from_slice(body).map_err(|_| "OpenRouter's answer couldn't be read.".to_owned())?;
    let entries = listing.get("data").and_then(Value::as_array).ok_or("OpenRouter's answer couldn't be read.")?;
    let entry = entries
        .iter()
        .find(|entry| entry.get("id").and_then(Value::as_str) == Some(id))
        .ok_or_else(|| format!("OpenRouter has no model “{id}”."))?;
    if !runs_agents(entry) {
        return Err(format!("OpenRouter's “{id}” can't run an agent: it doesn't take tools."));
    }
    match models("openrouter").into_iter().find(|model| model.id == id) {
        Some(model) => Ok(model),
        None => listed_model(id, entry).ok_or_else(|| format!("OpenRouter doesn't say how big “{id}”'s context is.")),
    }
}

/// An OpenRouter model id: letters, digits and `._:/-`, 128 at most (an
/// alias such as `~openai/gpt-latest` isn't one).
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= ID_LIMIT
        && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'/' | b'-'))
}

/// Whether a listed model can run an agent: it takes tools, answers in
/// text, and through Chat Completions (a `:batch` model only in batches).
fn runs_agents(entry: &Value) -> bool {
    let has = |list: Option<&Value>, item: &str| {
        list.and_then(Value::as_array).is_some_and(|list| list.iter().any(|value| value.as_str() == Some(item)))
    };
    let chat = match entry.get("endpoints") {
        None | Some(Value::Null) => true,
        endpoints => has(endpoints, "/api/v1/chat/completions"),
    };
    has(entry.get("supported_parameters"), "tools") && has(entry.pointer("/architecture/output_modalities"), "text") && chat
}

/// A model the bundled list doesn't have, from its listing.
fn listed_model(id: &str, entry: &Value) -> Option<Model> {
    let name: String =
        entry.get("name").and_then(Value::as_str).unwrap_or(id).chars().filter(|c| !c.is_control()).take(NAME_LIMIT).collect();
    let name = if name.trim().is_empty() { id.to_owned() } else { name.trim().to_owned() };
    let context_window = entry.get("context_length").and_then(Value::as_u64).filter(|length| *length > 0)?;
    let max_tokens = entry
        .pointer("/top_provider/max_completion_tokens")
        .and_then(Value::as_u64)
        .filter(|tokens| *tokens > 0)
        .unwrap_or(DEFAULT_MAX_TOKENS);
    let supports = |parameter: &str| {
        entry
            .get("supported_parameters")
            .and_then(Value::as_array)
            .is_some_and(|list| list.iter().any(|value| value.as_str() == Some(parameter)))
    };
    let reasoning = supports("reasoning");
    let image = entry
        .pointer("/architecture/input_modalities")
        .and_then(Value::as_array)
        .is_some_and(|list| list.iter().any(|value| value.as_str() == Some("image")));
    let input = if image { vec!["text".to_owned(), "image".to_owned()] } else { vec!["text".to_owned()] };
    // Dollars per token, as text; a million tokens' worth, never below 0
    // (a router's "-1").
    let price = |key: &str| {
        let per_token = entry.pointer(&format!("/pricing/{key}")).and_then(Value::as_str).and_then(|text| text.parse::<f64>().ok());
        per_token
            .filter(|value| value.is_finite() && *value > 0.0)
            // To a millionth of a dollar, without the float's noise.
            .map_or(0.0, |value| (value * 1_000_000_000_000.0).round() / 1_000_000.0)
    };
    let cost = serde_json::json!({
        "input": price("prompt"),
        "output": price("completion"),
        "cacheRead": price("input_cache_read"),
        "cacheWrite": price("input_cache_write"),
    });
    // As the bundled list has them, by who makes the model.
    let mut compat = serde_json::Map::new();
    if id.starts_with("anthropic/") {
        compat.insert("cacheControlFormat".into(), "anthropic".into());
    } else if !id.starts_with("openai/") {
        compat.insert("supportsDeveloperRole".into(), false.into());
    }
    if reasoning && id.starts_with("deepseek/") {
        compat.insert("requiresReasoningContentOnAssistantMessages".into(), true.into());
    }
    compat.insert("thinkingFormat".into(), "openrouter".into());
    let mut model = serde_json::json!({
        "id": id,
        "name": name,
        "api": "openai-completions",
        "provider": "openrouter",
        "baseUrl": OPENROUTER_BASE,
        "reasoning": reasoning,
        "input": input,
        "cost": cost,
        "contextWindow": context_window,
        "maxTokens": max_tokens,
        "compat": compat,
    });
    if let Some(map) = reasoning.then(|| level_map(entry)).flatten() {
        model["thinkingLevelMap"] = Value::Object(map);
    }
    serde_json::from_value(model).ok()
}

/// The efforts a listed model takes (`reasoning.supported_efforts`) as a
/// thinking level map: one it doesn't take is null, `xhigh` and `max` are
/// named when it does, and off is null when its reasoning can't be turned
/// off. None when the listing doesn't say.
fn level_map(entry: &Value) -> Option<serde_json::Map<String, Value>> {
    const LEVELS: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];
    let efforts: Vec<&str> = entry
        .pointer("/reasoning/supported_efforts")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(Value::as_str)
        .filter(|effort| LEVELS.contains(effort))
        .collect();
    if !efforts.iter().any(|effort| *effort != "none") {
        return None;
    }
    let mut map = serde_json::Map::new();
    let mandatory = entry.pointer("/reasoning/mandatory").and_then(Value::as_bool).unwrap_or(false);
    if mandatory && !efforts.contains(&"none") {
        map.insert("off".into(), Value::Null);
    }
    for level in ["minimal", "low", "medium", "high"] {
        if !efforts.contains(&level) {
            map.insert(level.into(), Value::Null);
        }
    }
    for level in ["xhigh", "max"] {
        if efforts.contains(&level) {
            map.insert(level.into(), level.into());
        }
    }
    Some(map)
}
