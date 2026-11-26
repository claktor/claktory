//! Model lists read from the providers themselves (friend bug #14), so a
//! new model shows up without waiting for a Claktory update: Anthropic's
//! and OpenAI's `GET /v1/models` and Codex's `GET /codex/models`, with the
//! signed-in account (OpenRouter's is `catalog::parse_openrouter`, U4).
//!
//! A listing never takes a model away: every model Claktory came with
//! stays, since a provider's list leaves out aliases agents run on (Claude's
//! "claude-sonnet-4-5"). What the provider says of a model (its efforts,
//! context size, longest answer and image input) is taken over the bundled
//! values; what it doesn't say (prices, how its API is spoken) comes from
//! the bundled list. A model the bundled list doesn't have is made from the
//! closest one it has (the newest of its line or family) with the
//! provider's own id, name and what it says.

use serde_json::{Map, Value};

use crate::auth::RequestAuth;
use crate::providers::catalog;
use crate::types::Model;

/// The most a listing may be.
pub const LIST_LIMIT: usize = 8 * 1024 * 1024;
const NAME_LIMIT: usize = 200;
/// The version Codex's model list is asked for as: models for newer
/// clients than this are left out of its answer.
pub const CODEX_CLIENT_VERSION: &str = "0.150.0";

/// The providers whose list Claktory reads from them.
pub const PROVIDERS: [&str; 5] = ["anthropic", "openai", "openai-codex", "openrouter", "grok"];

/// The provider's name in a sentence.
pub fn name(provider: &str) -> &str {
    match provider {
        "anthropic" => "Anthropic",
        "openai" => "OpenAI",
        "openai-codex" => "OpenAI Codex",
        "openrouter" => "OpenRouter",
        "grok" => "Grok",
        other => other,
    }
}

/// A model id as Claktory takes one typed: letters, digits and `._:-` (and
/// `/` on OpenRouter, whose ids name their maker), 128 at most.
pub fn valid_id(provider: &str, id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-') || (byte == b'/' && provider == "openrouter")
        })
        && !id.starts_with(['.', '/', '-'])
        && !id.contains("..")
}

/// Reads the provider's model list with `auth` from `base_url`, as it came.
/// An answer that refuses the credentials (401) is `Error::Models` with the
/// `UNAUTHORIZED` code, for a sign-in to refresh and ask again; anything
/// else fails with a sentence.
pub async fn fetch(provider: &str, base_url: &str, auth: &RequestAuth) -> Result<Vec<u8>, crate::Error> {
    let base = base_url.trim_end_matches('/');
    if provider == "grok" && !crate::providers::grok::allowed(base) {
        return Err(crate::providers::grok::not_allowed(base));
    }
    let url = match provider {
        "openrouter" => return catalog::fetch_openrouter().await.map_err(crate::Error::Other),
        "anthropic" => format!("{base}/v1/models?limit=1000"),
        "openai" | "grok" => format!("{base}/models"),
        "openai-codex" => format!("{base}/codex/models?client_version={CODEX_CLIENT_VERSION}"),
        other => return Err(crate::Error::Other(format!("{other}'s models can't be read"))),
    };
    let (status, body) = get(provider, &url, auth).await.map_err(crate::Error::Other)?;
    if !(200..300).contains(&status) {
        let said = answered(provider, status, &body, auth);
        if crate::accounts::refused_error(status, &String::from_utf8_lossy(&body)) {
            return Err(crate::Error::Models { code: crate::errors::UNAUTHORIZED, message: said });
        }
        return Err(crate::Error::Other(said));
    }
    Ok(body)
}

/// The provider's models from a listing `fetch` brought: the bundled ones,
/// with what the listing says of them, then those it lists that the
/// bundled list doesn't have. Fails, and nothing of it is used, unless it's
/// the provider's list.
pub fn parse(provider: &str, body: &[u8]) -> Result<Vec<Model>, String> {
    if provider == "openrouter" {
        return catalog::parse_openrouter(body);
    }
    if body.len() > LIST_LIMIT {
        return Err("the list is too big".into());
    }
    let listing: Value = serde_json::from_slice(body).map_err(|error| format!("the list isn't JSON: {error}"))?;
    let bundled = catalog::models(provider);
    let entries: &[Value] = match provider {
        "openai-codex" => listing.get("models"),
        _ => listing.get("data"),
    }
    .and_then(Value::as_array)
    .ok_or("the list has no models")?;
    let mut models = bundled.clone();
    let mut added = std::collections::BTreeMap::new();
    let mut known = 0;
    for entry in entries {
        let Some(id) = entry_id(provider, entry) else { continue };
        if !valid_id(provider, id) || !listed(provider, entry) {
            continue;
        }
        if let Some(model) = models.iter_mut().find(|model| model.id == id) {
            known += 1;
            apply(provider, model, entry);
        } else if let Some(model) = new_model(provider, id, entry, &bundled) {
            added.insert(id.to_owned(), model);
        }
    }
    let theirs = match provider {
        // Every one of OpenAI's lists names some model Claktory knows.
        "openai" => known > 0,
        "anthropic" => {
            known + added.len() > 0 && entries.iter().any(|entry| entry_id(provider, entry).is_some_and(|id| id.starts_with("claude-")))
        }
        _ => known + added.len() > 0,
    };
    if !theirs {
        return Err(format!("it doesn't look like {}'s list", name(provider)));
    }
    models.extend(added.into_values());
    Ok(models)
}

/// Asks the provider whether it has the model `id` for the sign-in in
/// `auth` and whether an agent can run on it: the model as Claktory would
/// list it. Why not, in a sentence to show, when it can't be used.
pub async fn check(provider: &str, base_url: &str, auth: &RequestAuth, id: &str) -> Result<Model, String> {
    let who = name(provider);
    if !valid_id(provider, id) {
        let allowed = if provider == "openrouter" { ". _ - : /" } else { ". _ - :" };
        return Err(format!("A model id is letters, digits and {allowed} only."));
    }
    let base = base_url.trim_end_matches('/');
    let none = || format!("{who} has no model “{id}” for this sign-in.");
    match provider {
        "anthropic" | "openai" => {
            let url = if provider == "anthropic" { format!("{base}/v1/models/{id}") } else { format!("{base}/models/{id}") };
            let (status, body) = get(provider, &url, auth).await?;
            if status == 404 {
                return Err(none());
            }
            if !(200..300).contains(&status) {
                return Err(answered(provider, status, &body, auth));
            }
            let entry: Value = serde_json::from_slice(&body).map_err(|_| format!("{who}'s answer couldn't be read."))?;
            // An alias ("claude-haiku-4-5") answers as the model it names.
            let Some(answered) = entry_id(provider, &entry) else { return Err(none()) };
            let bundled = catalog::models(provider);
            if let Some(mut model) = bundled.iter().find(|model| model.id == id || model.id == answered).cloned() {
                model.id = id.to_owned();
                apply(provider, &mut model, &entry);
                return Ok(model);
            }
            if provider == "openai" && !openai_runs_agents(id, false) {
                return Err(format!("OpenAI's “{id}” isn't a model agents can run on: it doesn't write text with tools."));
            }
            new_model(provider, id, &entry, &bundled).ok_or_else(|| format!("Claktory can't tell how to run {who}'s “{id}” yet."))
        }
        "openai-codex" => {
            let body = fetch(provider, base_url, auth).await.map_err(|error| error.to_string())?;
            let listing: Value = serde_json::from_slice(&body).map_err(|_| format!("{who}'s answer couldn't be read."))?;
            let entries = listing.get("models").and_then(Value::as_array).ok_or_else(|| format!("{who}'s answer couldn't be read."))?;
            // A hidden model is still one the sign-in can use.
            let entry = entries.iter().find(|entry| entry_id(provider, entry) == Some(id)).ok_or_else(none)?;
            let bundled = catalog::models(provider);
            if let Some(mut model) = bundled.iter().find(|model| model.id == id).cloned() {
                apply(provider, &mut model, entry);
                return Ok(model);
            }
            new_model(provider, id, entry, &bundled).ok_or_else(|| format!("Claktory can't tell how to run {who}'s “{id}” yet."))
        }
        "openrouter" => {
            let body = catalog::fetch_openrouter().await?;
            catalog::openrouter_model(&body, id)
        }
        "grok" => {
            let body = fetch(provider, base_url, auth).await.map_err(|error| error.to_string())?;
            let listing: Value = serde_json::from_slice(&body).map_err(|_| format!("{who}'s answer couldn't be read."))?;
            let entries = listing.get("data").and_then(Value::as_array).ok_or_else(|| format!("{who}'s answer couldn't be read."))?;
            let entry = entries.iter().find(|entry| entry_id(provider, entry) == Some(id)).ok_or_else(none)?;
            let bundled = catalog::models(provider);
            if let Some(mut model) = bundled.iter().find(|model| model.id == id).cloned() {
                apply(provider, &mut model, entry);
                return Ok(model);
            }
            new_model(provider, id, entry, &bundled).ok_or_else(|| format!("Claktory can't tell how to run {who}'s “{id}” yet."))
        }
        other => Err(format!("Claktory can't check {other}'s models.")),
    }
}

/// GETs `url` with the provider's sign-in, within `LIST_LIMIT` and a
/// timeout, following no redirect: the status and body.
async fn get(provider: &str, url: &str, auth: &RequestAuth) -> Result<(u16, Vec<u8>), String> {
    let who = name(provider);
    let client = reqwest::Client::builder()
        .use_preconfigured_tls((*crate::providers::tls_config()).clone())
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|error| format!("could not make a client: {error}"))?;
    let key = auth.api_key.clone().unwrap_or_default();
    let mut request = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::USER_AGENT, crate::providers::user_agent());
    request = match provider {
        "anthropic" if key.contains("sk-ant-oat") => {
            request.header("anthropic-version", "2023-06-01").bearer_auth(&key).header("anthropic-beta", "oauth-2025-04-20")
        }
        "anthropic" if !key.is_empty() => request.header("anthropic-version", "2023-06-01").header("x-api-key", &key),
        "anthropic" => request.header("anthropic-version", "2023-06-01"),
        "openai-codex" => {
            let account = crate::providers::openai_codex::oauth::account_id(&key).ok_or("the sign-in has no ChatGPT account")?;
            request.bearer_auth(&key).header("chatgpt-account-id", account).header("originator", "pi")
        }
        _ if !key.is_empty() => request.bearer_auth(&key),
        _ => request,
    };
    for (name, value) in &auth.headers {
        request = request.header(name, value);
    }
    let mut response = request.send().await.map_err(|_| format!("{who} couldn't be reached."))?;
    if response.content_length().is_some_and(|length| length > LIST_LIMIT as u64) {
        return Err(format!("{who}'s model list is too big."));
    }
    let status = response.status().as_u16();
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| format!("{who}'s answer couldn't be read."))? {
        if body.len() + chunk.len() > LIST_LIMIT {
            return Err(format!("{who}'s model list is too big."));
        }
        body.extend_from_slice(&chunk);
    }
    Ok((status, body))
}

/// A refusal as a sentence: the status and the provider's own message,
/// with the key taken out, never the body.
fn answered(provider: &str, status: u16, body: &[u8], auth: &RequestAuth) -> String {
    let said = crate::providers::keys::said(&String::from_utf8_lossy(body), auth.api_key.as_deref().unwrap_or_default());
    format!("{} answered {status}{said}.", name(provider))
}

fn entry_id<'a>(provider: &str, entry: &'a Value) -> Option<&'a str> {
    match provider {
        "openai-codex" => entry.get("slug").or_else(|| entry.get("id")),
        _ => entry.get("id"),
    }
    .and_then(Value::as_str)
}

/// Whether a listed model belongs in the list: Anthropic's not retired,
/// OpenAI's one that runs agents, Codex's not hidden.
fn listed(provider: &str, entry: &Value) -> bool {
    match provider {
        "anthropic" => entry.get("lifecycle").and_then(Value::as_str) != Some("retired"),
        "openai" => entry_id(provider, entry).is_some_and(|id| openai_runs_agents(id, true)),
        "openai-codex" => entry.get("visibility").and_then(Value::as_str) != Some("hide"),
        _ => true,
    }
}

/// Whether an OpenAI id names a model that writes text and takes tools, by
/// its form: `gpt-<version>` or `o<n>`, then plain words, none of which
/// marks audio, images, search, embeddings and the like. A dated snapshot
/// isn't one (its words aren't plain). `new` also wants GPT-5 or later, as
/// the list's new models are; a typed id may be older.
fn openai_runs_agents(id: &str, new: bool) -> bool {
    const NOT: [&str; 14] = [
        "audio",
        "realtime",
        "tts",
        "transcribe",
        "transcription",
        "image",
        "search",
        "embedding",
        "instruct",
        "moderation",
        "deep",
        "research",
        "diarize",
        "computer",
    ];
    let mut parts = id.split('-');
    let words: Vec<&str> = match parts.next() {
        Some("gpt") => {
            let Some(version) = parts.next() else { return false };
            let major = version.split('.').next().and_then(|major| major.parse::<u32>().ok());
            if !version.bytes().all(|byte| byte.is_ascii_digit() || byte == b'.') || major.is_none_or(|major| new && major < 5) {
                return false;
            }
            parts.collect()
        }
        Some(head) if head.len() > 1 && head.starts_with('o') && head[1..].bytes().all(|byte| byte.is_ascii_digit()) => parts.collect(),
        _ => return false,
    };
    words.iter().all(|word| !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_lowercase()) && !NOT.contains(word))
}

/// What the listing says of a model Claktory has, over its values.
fn apply(provider: &str, model: &mut Model, entry: &Value) {
    match provider {
        "anthropic" => apply_anthropic(model, entry),
        "openai-codex" => apply_codex(model, entry),
        "grok" => apply_grok(model, entry),
        _ => {}
    }
}

fn supported(value: Option<&Value>) -> Option<bool> {
    value.and_then(|value| value.get("supported")).and_then(Value::as_bool)
}

fn positive(entry: &Value, key: &str) -> Option<u64> {
    entry.get(key).and_then(Value::as_u64).filter(|value| *value > 0)
}

fn set_image(model: &mut Model, image: bool) {
    model.input = if image { vec!["text".to_owned(), "image".to_owned()] } else { vec!["text".to_owned()] };
}

/// Grok's proxy's list: its context size, and the efforts it takes
/// (`reasoning_efforts`, each `value` one of OpenAI's levels).
fn apply_grok(model: &mut Model, entry: &Value) {
    if let Some(context) = positive(entry, "context_window") {
        model.context_window = context;
    }
    if let Some(efforts) = entry.get("reasoning_efforts").and_then(Value::as_array) {
        let values: Vec<&str> = efforts.iter().filter_map(|effort| effort.get("value").and_then(Value::as_str)).collect();
        if !values.is_empty() {
            model.reasoning = true;
            let map = model.thinking_level_map.get_or_insert_with(Map::new);
            map.insert("off".into(), Value::Null);
            map.insert("minimal".into(), if values.contains(&"minimal") { "minimal".into() } else { "low".into() });
            for level in ["low", "medium", "high"] {
                if !values.contains(&level) {
                    map.insert(level.into(), Value::Null);
                }
            }
            if values.contains(&"xhigh") {
                map.insert("xhigh".into(), "xhigh".into());
            } else {
                map.remove("xhigh");
            }
        }
    }
}

/// Anthropic's `ModelInfo`: context size, longest answer, image input and
/// efforts (`capabilities.effort`, and whether thinking can be turned off).
fn apply_anthropic(model: &mut Model, entry: &Value) {
    if let Some(context) = positive(entry, "max_input_tokens") {
        model.context_window = context;
    }
    if let Some(most) = positive(entry, "max_tokens") {
        model.max_tokens = most;
    }
    let Some(capabilities) = entry.get("capabilities").filter(|value| value.is_object()) else { return };
    if let Some(image) = supported(capabilities.get("image_input")) {
        set_image(model, image);
    }
    let effort = capabilities.get("effort");
    if model.reasoning && supported(effort) == Some(true) {
        let mut levels: Vec<&str> = ["low", "medium", "high", "xhigh", "max"]
            .into_iter()
            .filter(|level| supported(effort.and_then(|effort| effort.get(*level))) == Some(true))
            .collect();
        // Minimal is sent as low.
        if levels.contains(&"low") {
            levels.push("minimal");
        }
        let off = supported(capabilities.pointer("/thinking/types/disabled"));
        model.thinking_level_map = efforts(model.thinking_level_map.take(), &levels, off);
    }
}

/// Codex's model entry: context size, image input and efforts
/// (`supported_reasoning_levels`).
fn apply_codex(model: &mut Model, entry: &Value) {
    if let Some(context) = positive(entry, "context_window").or_else(|| positive(entry, "max_context_window")) {
        model.context_window = context;
    }
    if let Some(inputs) = entry.get("input_modalities").and_then(Value::as_array) {
        set_image(model, inputs.iter().any(|input| input.as_str() == Some("image")));
    }
    let levels: Vec<&str> = entry
        .get("supported_reasoning_levels")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|level| {
            level.get("effort").or_else(|| level.get("reasoning_effort")).and_then(Value::as_str).or_else(|| level.as_str())
        })
        .collect();
    if model.reasoning && levels.iter().any(|level| ["low", "medium", "high"].contains(level)) {
        // Codex's minimal is sent as low, as the bundled list says.
        let mut levels = levels;
        if !levels.contains(&"minimal") && levels.contains(&"low") {
            levels.push("minimal");
        }
        model.thinking_level_map = efforts(model.thinking_level_map.take(), &levels, None);
    }
}

/// A thinking level map that offers exactly `levels` (of minimal to max),
/// keeping what `map` sends each as; `off` says whether reasoning can be
/// turned off (None: as `map` has it).
fn efforts(map: Option<Map<String, Value>>, levels: &[&str], off: Option<bool>) -> Option<Map<String, Value>> {
    let mut map = map.unwrap_or_default();
    for level in ["minimal", "low", "medium", "high"] {
        match (levels.contains(&level), map.get(level)) {
            (true, Some(Value::Null)) => {
                map.remove(level);
            }
            (true, _) => {}
            (false, _) => {
                map.insert(level.to_owned(), Value::Null);
            }
        }
    }
    for level in ["xhigh", "max"] {
        if levels.contains(&level) {
            if !map.get(level).is_some_and(Value::is_string) {
                map.insert(level.to_owned(), level.into());
            }
        } else {
            map.remove(level);
        }
    }
    match (off, map.get("off")) {
        (Some(true), Some(Value::Null)) => {
            map.remove("off");
        }
        (Some(false), _) => {
            map.insert("off".to_owned(), Value::Null);
        }
        _ => {}
    }
    (!map.is_empty()).then_some(map)
}

/// A model the bundled list doesn't have, from the closest one it has.
fn new_model(provider: &str, id: &str, entry: &Value, bundled: &[Model]) -> Option<Model> {
    let template = match provider {
        "anthropic" => anthropic_template(entry, bundled),
        _ => template(id, bundled),
    }?;
    let mut model = template.clone();
    model.id = id.to_owned();
    model.name = entry
        .get("display_name")
        .or_else(|| entry.get("name").filter(|_| provider == "grok"))
        .and_then(Value::as_str)
        .map(|name| name.chars().filter(|c| !c.is_control()).take(NAME_LIMIT).collect::<String>().trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| pretty(id));
    if provider == "anthropic" {
        // Fallbacks name other models by what this one's maker chose.
        if let Some(compat) = model.compat.as_mut() {
            compat.remove("allowedFallbackModels");
            compat.remove("serverSideFallback");
        }
        if let Some(capabilities) = entry.get("capabilities").filter(|value| value.is_object()) {
            if let Some(thinking) = supported(capabilities.get("thinking")) {
                model.reasoning = thinking;
            }
            let compat = model.compat.get_or_insert_with(Map::new);
            if let Some(adaptive) = supported(capabilities.pointer("/thinking/types/adaptive")) {
                compat.insert("forceAdaptiveThinking".into(), adaptive.into());
            }
            if let Some(strict) = supported(capabilities.get("structured_outputs")) {
                compat.insert("supportsStrictTools".into(), strict.into());
            }
        }
    }
    apply(provider, &mut model, entry);
    Some(model)
}

/// Anthropic's closest: the newest bundled model of the same line (opus,
/// sonnet…), else the dearest one, so a new line is never priced low.
fn anthropic_template<'a>(entry: &Value, bundled: &'a [Model]) -> Option<&'a Model> {
    let line = entry.get("line").and_then(Value::as_str).filter(|line| line.bytes().all(|byte| byte.is_ascii_lowercase()));
    let of_line = line.and_then(|line| {
        bundled
            .iter()
            .filter(|model| model.id.contains(&format!("-{line}-")) && !dated(&model.id))
            .max_by(|a, b| version(&a.id).cmp(&version(&b.id)))
    });
    of_line.or_else(|| bundled.iter().max_by(|a, b| a.cost.output.total_cmp(&b.cost.output)))
}

/// The newest bundled model of the same family as `id` (its numbers aside:
/// "gpt-5.7-sol" is "gpt-#-sol"), else of its base ("gpt-#").
fn template<'a>(id: &str, bundled: &'a [Model]) -> Option<&'a Model> {
    let newest = |key: &str| {
        bundled.iter().filter(|model| family(&model.id) == key && !dated(&model.id)).max_by(|a, b| version(&a.id).cmp(&version(&b.id)))
    };
    let key = family(id);
    let base = key.split_once('#').map(|(head, _)| format!("{head}#")).unwrap_or_default();
    newest(&key).or_else(|| newest(&base))
}

/// An id with each run of digits and dots as "#".
fn family(id: &str) -> String {
    let mut key = String::new();
    for c in id.chars() {
        if c.is_ascii_digit() || c == '.' {
            if !key.ends_with('#') {
                key.push('#');
            }
        } else {
            key.push(c);
        }
    }
    key
}

/// The numbers in an id, in order, to tell the newer of two apart.
fn version(id: &str) -> Vec<u64> {
    id.split(|c: char| !c.is_ascii_digit()).filter_map(|part| part.parse().ok()).collect()
}

/// A dated snapshot, such as "claude-opus-4-5-20251101".
fn dated(id: &str) -> bool {
    id.rsplit('-').next().is_some_and(|last| last.len() == 8 && last.bytes().all(|byte| byte.is_ascii_digit()))
}

/// The newer version of the model `current` among `ids` (#404), the same
/// provider's: the newest one of its line with a higher version, if any.
/// An id's line is its words, its version its numbers, the same rule for
/// every provider, as each puts its versions in its ids: Anthropic's
/// "claude-opus-4-5" is (claude, opus) 4.5, and so is "claude-3-5-sonnet"
/// sonnet's 3.5; OpenAI's and Codex's "gpt-6.1-sol" is (gpt, sol) 6.1, "o3"
/// (o#) 3; OpenRouter's "google/gemini-2.5-pro" (google, gemini, pro) 2.5.
/// A different word is another model, not a successor: a mini, a size
/// ("70b"), a variant (":free"), another line. A dated snapshot
/// (Anthropic's "-20251101", OpenAI's "-2026-11-01") is the version it
/// names, and is never offered itself.
pub fn successor<'a>(current: &str, ids: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let (line, version) = line_and_version(current);
    if version.is_empty() {
        return None;
    }
    ids.into_iter()
        .filter(|id| !snapshot(id).1)
        .filter_map(|id| {
            let (their_line, their_version) = line_and_version(id);
            (their_line == line && their_version > version).then_some((their_version, id))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, id)| id)
}

/// An id without its snapshot date, and whether it had one.
fn snapshot(id: &str) -> (&str, bool) {
    let digits = |part: &str, length: usize| part.len() == length && part.bytes().all(|byte| byte.is_ascii_digit());
    let parts: Vec<&str> = id.rsplitn(4, '-').collect();
    // OpenAI's: "-2026-11-01".
    if parts.len() == 4 && digits(parts[2], 4) && digits(parts[1], 2) && digits(parts[0], 2) {
        return (parts[3], true);
    }
    match id.rsplit_once('-') {
        // Anthropic's: "-20251101".
        Some((head, last)) if digits(last, 8) => (head, true),
        _ => (id, false),
    }
}

/// An id's words (its line) and numbers (its version), its date aside: a
/// part that's all digits and dots is version; one letter and then digits
/// ("o3", "r1") is a word and a version; any other part is a word as it is.
fn line_and_version(id: &str) -> (Vec<String>, Vec<u64>) {
    let mut line = Vec::new();
    let mut version = Vec::new();
    let numbers = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit() || byte == b'.');
    for part in snapshot(id).0.split(['-', '/']) {
        if numbers(part) {
            version.extend(part.split('.').filter_map(|number| number.parse::<u64>().ok()));
        } else if part.len() > 1 && part.as_bytes()[0].is_ascii_lowercase() && numbers(&part[1..]) {
            line.push(format!("{}#", &part[..1]));
            version.extend(part[1..].split('.').filter_map(|number| number.parse::<u64>().ok()));
        } else {
            line.push(part.to_owned());
        }
    }
    (line, version)
}

/// A name for an id the provider didn't name: "gpt-5.7-sol" is "GPT-5.7 Sol".
fn pretty(id: &str) -> String {
    let mut parts = id.split('-');
    match parts.next() {
        Some("gpt") => {
            let version = parts.next().unwrap_or_default();
            let words: Vec<String> = parts
                .map(|word| {
                    let mut chars = word.chars();
                    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
                })
                .collect();
            std::iter::once(format!("GPT-{version}")).chain(words).collect::<Vec<_>>().join(" ")
        }
        _ => id.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn levels(model: &Model) -> Vec<&'static str> {
        crate::providers::supported_thinking_levels(model).into_iter().map(|level| level.as_str()).collect()
    }

    fn anthropic_entry(id: &str, line: &str, xhigh: bool) -> Value {
        json!({
            "type": "model", "id": id, "display_name": format!("Claude {id}"), "lifecycle": "active", "line": line,
            "max_input_tokens": 2_000_000, "max_tokens": 64_000,
            "capabilities": {
                "image_input": {"supported": true},
                "structured_outputs": {"supported": true},
                "effort": {"supported": true, "low": {"supported": true}, "medium": {"supported": true}, "high": {"supported": true},
                    "xhigh": {"supported": xhigh}, "max": {"supported": true}},
                "thinking": {"supported": true, "types": {"adaptive": {"supported": true}, "disabled": {"supported": false}, "enabled": {"supported": false}}}
            }
        })
    }

    #[test]
    fn anthropics_list_adds_its_new_models_and_keeps_every_bundled_one() {
        let body = json!({"data": [
            anthropic_entry("claude-sonnet-6", "sonnet", false),
            anthropic_entry("claude-opus-5", "opus", true),
            {"id": "claude-old-1", "lifecycle": "retired"},
            {"id": "../escape", "line": "opus"},
        ], "has_more": false});
        let models = parse("anthropic", body.to_string().as_bytes()).expect("parsed");
        let bundled = catalog::models("anthropic");
        assert!(bundled.iter().all(|old| models.iter().any(|model| model.id == old.id)), "none taken away");
        let new = models.iter().find(|model| model.id == "claude-sonnet-6").expect("added");
        assert_eq!(new.name, "Claude claude-sonnet-6");
        assert_eq!((new.context_window, new.max_tokens), (2_000_000, 64_000));
        assert_eq!(new.api, "anthropic-messages");
        assert_eq!(
            new.cost.input,
            bundled.iter().find(|model| model.id == "claude-sonnet-5-5").expect("bundled").cost.input,
            "priced as its line's newest"
        );
        assert_eq!(
            levels(new),
            ["minimal", "low", "medium", "high", "max"],
            "minimal as low, as the bundled models; no xhigh; off can't be had"
        );
        assert!(
            new.compat.as_ref().is_some_and(|compat| compat["forceAdaptiveThinking"] == true && !compat.contains_key("serverSideFallback"))
        );
        let opus = models.iter().find(|model| model.id == "claude-opus-5").expect("bundled");
        assert_eq!(opus.context_window, 2_000_000, "the provider's word over the bundle's");
        assert!(!models.iter().any(|model| model.id == "claude-old-1" || model.id == "../escape"));
        assert!(parse("anthropic", br#"{"data": [{"id": "gpt-5"}]}"#).is_err(), "not Anthropic's list");
    }

    #[test]
    fn openais_list_adds_new_models_that_run_agents_from_their_family() {
        let ids = [
            "gpt-5.5",
            "gpt-5.7-sol",
            "gpt-6",
            "gpt-5.7-2026-11-01",
            "gpt-5.7-mini-tts",
            "gpt-image-2",
            "o5",
            "text-embedding-4",
            "gpt-4.5",
        ];
        let body = json!({"object": "list", "data": ids.iter().map(|id| json!({"id": id, "object": "model"})).collect::<Vec<_>>()});
        let models = parse("openai", body.to_string().as_bytes()).expect("parsed");
        let added: Vec<&str> = models
            .iter()
            .filter(|model| !catalog::models("openai").iter().any(|old| old.id == model.id))
            .map(|model| model.id.as_str())
            .collect();
        assert_eq!(added, ["gpt-5.7-sol", "gpt-6", "o5"]);
        let sol = models.iter().find(|model| model.id == "gpt-5.7-sol").expect("added");
        assert_eq!(sol.name, "GPT-5.7 Sol");
        assert_eq!(sol.cost.input, catalog::models("openai").iter().find(|model| model.id == "gpt-5.6-sol").expect("bundled").cost.input);
        assert!(parse("openai", br#"{"data": [{"id": "not-a-model"}]}"#).is_err(), "none it knows: not OpenAI's list");
    }

    #[test]
    fn codexs_list_takes_its_efforts_and_leaves_hidden_models_out() {
        let body = json!({"models": [
            {"slug": "gpt-6.2-sol", "display_name": "GPT-6.2 Sol", "visibility": "list", "context_window": 400_000,
             "supported_reasoning_levels": [{"effort": "low"}, {"effort": "medium"}, {"effort": "high"}, {"effort": "xhigh"}], "input_modalities": ["text", "image"]},
            {"slug": "gpt-6.2-secret", "display_name": "Secret", "visibility": "hide", "supported_reasoning_levels": []},
        ]});
        let models = parse("openai-codex", body.to_string().as_bytes()).expect("parsed");
        let new = models.iter().find(|model| model.id == "gpt-6.2-sol").expect("added");
        assert_eq!((new.name.as_str(), new.context_window), ("GPT-6.2 Sol", 400_000));
        assert!(levels(new).contains(&"xhigh") && !levels(new).contains(&"max"), "{:?}", levels(new));
        assert!(!models.iter().any(|model| model.id == "gpt-6.2-secret"));
    }

    #[test]
    fn a_successor_is_the_newest_of_the_same_line_never_a_sibling() {
        // Anthropic: numbers split by dashes, dated snapshots, older names.
        let anthropic = [
            "claude-opus-4-5",
            "claude-opus-4-5-20251101",
            "claude-opus-5",
            "claude-opus-5-5",
            "claude-opus-6-20270101",
            "claude-sonnet-6",
            "claude-haiku-4-5",
            "claude-fable-5",
        ];
        assert_eq!(successor("claude-opus-4-5", anthropic), Some("claude-opus-5-5"), "a dated one isn't offered");
        assert_eq!(successor("claude-opus-4-5-20251101", anthropic), Some("claude-opus-5-5"), "a snapshot is its version");
        assert_eq!(successor("claude-opus-5-5", anthropic), None, "the newest");
        assert_eq!(successor("claude-3-5-sonnet-20241022", anthropic), Some("claude-sonnet-6"), "the older naming");
        assert_eq!(successor("claude-haiku-4-5", anthropic), None, "no other line");
        assert_eq!(successor("claude-fable-5", ["claude-fable-5", "claude-fable-5-1", "claude-fable-5-2"]), Some("claude-fable-5-2"));

        // OpenAI and Codex: Matt's GPT 6.1 and 6.2; minis, sizes and
        // variants aren't upgrades.
        let openai = ["gpt-5.4", "gpt-5.4-mini", "gpt-5.5", "gpt-5.6-sol", "gpt-5.6-luna", "gpt-6.1-sol", "gpt-6.2-sol", "gpt-6.3-mini"];
        assert_eq!(successor("gpt-6.1-sol", openai), Some("gpt-6.2-sol"));
        assert_eq!(successor("gpt-5.6-sol", openai), Some("gpt-6.2-sol"));
        assert_eq!(successor("gpt-5.6-luna", openai), None, "sol is a sibling");
        assert_eq!(successor("gpt-5.4", openai), Some("gpt-5.5"), "a mini isn't one");
        assert_eq!(successor("gpt-5.4-mini", openai), Some("gpt-6.3-mini"));
        assert_eq!(successor("gpt-5.5", ["gpt-5.5", "gpt-5.6-2026-11-01"]), None, "a dated snapshot isn't offered");
        assert_eq!(successor("o3", ["o3", "o4-mini", "o5"]), Some("o5"));
        assert_eq!(successor("gpt-5.3-codex-spark", openai), None);

        // OpenRouter: the maker's name, sizes and variants.
        let openrouter = [
            "google/gemini-2.5-pro",
            "google/gemini-3-pro",
            "google/gemini-3-flash",
            "meta-llama/llama-3.1-70b",
            "meta-llama/llama-3.1-405b",
            "deepseek/deepseek-r1:free",
            "deepseek/deepseek-r2",
            "anthropic/claude-sonnet-4.5",
            "anthropic/claude-sonnet-5",
        ];
        assert_eq!(successor("google/gemini-2.5-pro", openrouter), Some("google/gemini-3-pro"));
        assert_eq!(successor("meta-llama/llama-3.1-70b", openrouter), None, "a bigger size isn't newer");
        assert_eq!(successor("deepseek/deepseek-r1:free", openrouter), None, "the free variant is another model");
        assert_eq!(successor("anthropic/claude-sonnet-4.5", openrouter), Some("anthropic/claude-sonnet-5"));
        assert_eq!(successor("openrouter/auto", openrouter), None, "no version");
    }

    #[test]
    fn typed_ids_are_plain() {
        assert!(valid_id("anthropic", "claude-opus-6"));
        assert!(
            !valid_id("anthropic", "claude/../x") && !valid_id("openai", "a?b") && !valid_id("openai", "") && !valid_id("openai", "..")
        );
        assert!(valid_id("openrouter", "google/gemini-4-neon") && !valid_id("anthropic", "google/gemini-4-neon"));
    }
}
