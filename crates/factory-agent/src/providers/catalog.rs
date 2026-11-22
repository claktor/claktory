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
