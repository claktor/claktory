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
