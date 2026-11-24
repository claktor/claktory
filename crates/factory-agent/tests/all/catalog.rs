use crate::support::luna;
use factory_agent::providers::catalog::models;
use serde_json::json;

#[test]
fn the_four_providers_load_from_pi_data() {
    assert_eq!(models("openai").len(), 38);
    assert_eq!(models("openai-codex").len(), 8);
    assert_eq!(models("openrouter").len(), 356);
    assert_eq!(models("anthropic").len(), 15);
    assert!(models("google").is_empty());
}

#[test]
fn luna_matches_the_catalog_entry() {
    let catalog = models("openai-codex").into_iter().find(|model| model.id == "gpt-5.6-luna").expect("luna");
    let mut expected = luna();
    expected.thinking_level_map = Some(json!({"xhigh": "xhigh", "max": "max", "minimal": "low"}).as_object().expect("map").clone());
    expected.compat = Some(
        json!({"supportsOpenAIGrammarTools": true, "supportsAdditionalTools": true, "supportsToolSearch": true})
            .as_object()
            .expect("map")
            .clone(),
    );
    assert_eq!(catalog, expected);
}

#[test]
fn openai_models_carry_their_level_maps_and_compat() {
    let gpt = models("openai").into_iter().find(|model| model.id == "gpt-5.5").expect("gpt-5.5");
    assert_eq!(gpt.api, "openai-responses");
    assert_eq!(gpt.base_url, "https://api.openai.com/v1");
    assert_eq!(gpt.max_tokens, 128_000);
    assert_eq!(gpt.thinking_level_map.as_ref().and_then(|map| map.get("off")), Some(&json!("none")));
    assert_eq!(gpt.compat.as_ref().and_then(|compat| compat.get("supportsStrictMode")), Some(&json!(true)));
    let deepseek = models("openrouter").into_iter().find(|model| model.id == "deepseek/deepseek-v4-flash-0731").expect("deepseek");
    assert_eq!(deepseek.context_window, 1_048_576);
    assert_eq!(deepseek.compat.as_ref().and_then(|compat| compat.get("thinkingFormat")), Some(&json!("openrouter")));
}

#[test]
fn sol_6_1_is_available_to_codex_jobs_with_its_own_pricing() {
    let model = models("openai-codex").into_iter().find(|model| model.id == "gpt-6.1-sol").expect("sol 6.1");
    assert_eq!(model.api, "openai-codex-responses");
    assert_eq!(model.context_window, 272_000);
    assert_eq!(model.max_tokens, 128_000);
    assert_eq!(model.cost.input, 2.0);
    assert_eq!(model.cost.output, 10.0);
    assert_eq!(model.cost.cache_read, 0.1);
    assert_eq!(model.cost.cache_write, 2.5);
}

#[test]
fn opus_5_5_keeps_thinking_on_with_its_own_pricing() {
    let model = models("anthropic").into_iter().find(|model| model.id == "claude-opus-5-5").expect("opus 5.5");
    assert_eq!((model.context_window, model.max_tokens), (1_000_000, 128_000));
    assert_eq!((model.cost.input, model.cost.output, model.cost.cache_read, model.cost.cache_write), (4.0, 20.0, 0.2, 5.0));
    assert_eq!(model.thinking_level_map.as_ref().and_then(|map| map.get("off")), Some(&json!(null)));
    assert_eq!(model.compat.as_ref().and_then(|compat| compat.get("forceAdaptiveThinking")), Some(&json!(true)));
}

/// A listing as OpenRouter's `GET /api/v1/models` sends it: `filler` plain
/// models that run agents, then `extra`.
fn listing(filler: usize, extra: Vec<serde_json::Value>) -> Vec<u8> {
    let mut data: Vec<serde_json::Value> = (0..filler)
        .map(|index| {
            json!({
                "id": format!("vendor/model-{index:03}"),
                "name": format!("Vendor: Model {index}"),
                "context_length": 128_000,
                "architecture": {"input_modalities": ["text"], "output_modalities": ["text"]},
                "pricing": {"prompt": "0.000001", "completion": "0.000002"},
                "top_provider": {"max_completion_tokens": null},
                "supported_parameters": ["tools", "max_tokens"],
                "endpoints": ["/api/v1/chat/completions"],
            })
        })
        .collect();
    data.extend(extra);
    serde_json::to_vec(&json!({"data": data, "total_count": 0})).expect("json")
}

fn opus_listed(id: &str) -> serde_json::Value {
    json!({
        "id": id,
        "name": "Anthropic: Claude Opus 9\u{7}",
        "context_length": 1_000_000,
        "architecture": {"input_modalities": ["text", "image", "file"], "output_modalities": ["text"]},
        "pricing": {"prompt": "0.000004", "completion": "0.00002", "input_cache_read": "0.0000002", "input_cache_write": "0.000005"},
        "top_provider": {"max_completion_tokens": 128_000, "context_length": 1_000_000},
        "supported_parameters": ["reasoning", "tools", "max_tokens"],
        "endpoints": ["/api/v1/chat/completions", "/api/v1/messages"],
        "reasoning": {"mandatory": true, "supported_efforts": ["max", "xhigh", "high", "medium", "low"], "default_effort": "high"},
        // Fields that would route it elsewhere are OpenRouter's to ignore.
        "provider": "anthropic",
        "baseUrl": "https://elsewhere.example/v1",
        "api": "anthropic-messages",
    })
}

#[test]
fn a_new_openrouter_model_gets_its_fields_from_the_listing_and_routes_through_openrouter() {
    use factory_agent::providers::catalog::parse_openrouter;
    use factory_agent::providers::supported_thinking_levels;
    use factory_agent::stream::ThinkingLevel;
    let models = parse_openrouter(&listing(60, vec![opus_listed("anthropic/claude-opus-9")])).expect("a list");
    assert_eq!(models.len(), 61);
    let opus = models.iter().find(|model| model.id == "anthropic/claude-opus-9").expect("opus 9");
    assert_eq!(
        (opus.provider.as_str(), opus.api.as_str(), opus.base_url.as_str()),
        ("openrouter", "openai-completions", "https://openrouter.ai/api/v1")
    );
    assert_eq!(opus.name, "Anthropic: Claude Opus 9");
    assert_eq!((opus.context_window, opus.max_tokens), (1_000_000, 128_000));
    assert_eq!(opus.input, ["text", "image"]);
    assert_eq!((opus.cost.input, opus.cost.output, opus.cost.cache_read, opus.cost.cache_write), (4.0, 20.0, 0.2, 5.0));
    assert!(opus.reasoning);
    assert_eq!(opus.compat.as_ref().and_then(|compat| compat.get("cacheControlFormat")), Some(&json!("anthropic")));
    assert_eq!(
        supported_thinking_levels(opus),
        [ThinkingLevel::Low, ThinkingLevel::Medium, ThinkingLevel::High, ThinkingLevel::Xhigh, ThinkingLevel::Max]
    );
    let plain = models.iter().find(|model| model.id == "vendor/model-000").expect("a plain one");
    assert!(!plain.reasoning);
    assert_eq!(plain.max_tokens, 4096);
    assert_eq!(plain.compat.as_ref().and_then(|compat| compat.get("supportsDeveloperRole")), Some(&json!(false)));
}

#[test]
fn a_bundled_openrouter_model_keeps_its_bundled_entry_and_one_no_longer_listed_goes() {
    use factory_agent::providers::catalog::parse_openrouter;
    let bundled = models("openrouter").into_iter().find(|model| model.id == "anthropic/claude-opus-5.5").expect("bundled");
    let mut listed = opus_listed("anthropic/claude-opus-5.5");
    listed["context_length"] = json!(5);
    listed["name"] = json!("Renamed");
    let models = parse_openrouter(&listing(60, vec![listed])).expect("a list");
    assert_eq!(models.iter().find(|model| model.id == "anthropic/claude-opus-5.5"), Some(&bundled));
    assert!(!models.iter().any(|model| model.id == "openai/o1"), "bundled, but OpenRouter no longer lists it");
}

#[test]
fn openrouter_models_that_cant_run_agents_or_have_odd_ids_are_left_out() {
    use factory_agent::providers::catalog::parse_openrouter;
    let mut no_tools = opus_listed("vendor/no-tools");
    no_tools["supported_parameters"] = json!(["max_tokens"]);
    let mut batch = opus_listed("vendor/model:batch");
    batch["endpoints"] = json!(["/api/v1/batches"]);
    let mut images = opus_listed("vendor/images-only");
    images["architecture"]["output_modalities"] = json!(["image"]);
    let long = format!("vendor/{}", "a".repeat(130));
    let odd: Vec<serde_json::Value> =
        ["~vendor/latest", "vendor/a b", "vendor/<script>", "vendor/ünïcode", long.as_str(), ""].into_iter().map(opus_listed).collect();
    let mut extra = vec![no_tools, batch, images];
    extra.extend(odd);
    let models = parse_openrouter(&listing(60, extra)).expect("a list");
    assert_eq!(models.len(), 60);
}

#[test]
fn a_broken_or_short_openrouter_list_is_refused_whole() {
    use factory_agent::providers::catalog::parse_openrouter;
    assert!(parse_openrouter(b"<html>Bad gateway</html>").is_err());
    assert!(parse_openrouter(br#"{"error": {"code": 500}}"#).is_err());
    assert!(parse_openrouter(&listing(10, Vec::new())).is_err(), "ten models is no list");
    let oversized = vec![b' '; factory_agent::providers::catalog::OPENROUTER_LIST_LIMIT + 1];
    assert!(parse_openrouter(&oversized).is_err());
}

#[test]
fn a_fetched_list_replaces_openrouters_bundled_one_in_the_registry() {
    use factory_agent::providers::catalog::parse_openrouter;
    use factory_agent::providers::models::Registry;
    let dir = tempfile::tempdir().expect("dir");
    let registry = Registry::new(dir.path().join("auth.json"));
    assert!(registry.resolve("openrouter/openai/o1").is_some());
    let fetched = parse_openrouter(&listing(60, vec![opus_listed("anthropic/claude-opus-9")])).expect("a list");
    assert!(registry.set_fetched_models("openrouter", fetched.clone()));
    assert!(!registry.set_fetched_models("openrouter", fetched), "the same list again changes nothing");
    assert_eq!(registry.get_models(Some("openrouter")).len(), 61);
    let opus = registry.resolve("openrouter/anthropic/claude-opus-9").expect("the new one");
    assert_eq!(opus.base_url, "https://openrouter.ai/api/v1");
    assert!(registry.resolve("openrouter/openai/o1").is_none());
    assert!(registry.resolve("anthropic/claude-opus-5-5").is_some(), "other providers keep theirs");
    assert!(!registry.set_fetched_models("google", Vec::new()), "no such provider");
}

#[test]
fn only_a_provider_at_its_own_address_fetches_its_models() {
    use factory_agent::credentials::FileCredentialStore;
    use factory_agent::providers::models::{Registry, default_providers};
    let dir = tempfile::tempdir().expect("dir");
    let with_env = |pairs: &'static [(&'static str, &'static str)]| -> factory_agent::auth::Env {
        std::sync::Arc::new(move |name| pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| (*value).to_owned()))
    };
    let store = || FileCredentialStore::new(dir.path().join("auth.json"));
    let plain = Registry::with_providers(store(), with_env(&[]), default_providers());
    assert!(plain.fetches_models("openrouter"));
    // Friend bug #14: Anthropic's, OpenAI's and Codex's too.
    assert!(["anthropic", "openai", "openai-codex"].iter().all(|id| plain.fetches_models(id)));
    assert!(!plain.fetches_models("google"));
    let pointed = Registry::with_providers(store(), with_env(&[("OPENROUTER_BASE_URL", "http://127.0.0.1:7500/v1")]), default_providers());
    assert!(!pointed.fetches_models("openrouter"), "a sandbox's qa-model isn't OpenRouter");
    let pointed = Registry::with_providers(store(), with_env(&[("ANTHROPIC_BASE_URL", "http://127.0.0.1:7500")]), default_providers());
    assert!(!pointed.fetches_models("anthropic"));
    let mut providers = default_providers();
    for provider in &mut providers {
        provider.base_url = "http://127.0.0.1:9/v1";
    }
    let elsewhere = Registry::with_providers(store(), with_env(&[]), providers);
    assert!(["anthropic", "openai", "openai-codex", "openrouter"].iter().all(|id| !elsewhere.fetches_models(id)));
}
