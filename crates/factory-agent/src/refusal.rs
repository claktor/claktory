//! A model's refusal, carried structurally on the message so the harness
//! can retry it on a fallback model and the transcript can show it as
//! itself, not a generic failure.
//!
//! The refusal rides in the message's `extra` map (like the account marks),
//! never as a `StopReason` variant: a backend from before this one reads
//! the saved conversation fine after a rollback, since unknown `extra` keys
//! are kept and the stop reason stays `Error`.

use serde_json::{Value, json};

use crate::types::AssistantMessage;

/// The key in a message's `extra` holding a refusal's details.
const REFUSAL: &str = "refusal";

/// The key in a message's `extra` marking that another model served the
/// response after this one's model declined (the provider's own fallback).
const FALLBACK_SERVED: &str = "fallbackServed";

/// What a model's safety classifiers said when they declined a request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Refusal {
    /// The policy area ("cyber", "bio", …), or empty when the provider
    /// named none (a normal, permanent value, not a placeholder).
    pub category: String,
    /// The provider's human-readable description; display, never parse.
    pub explanation: String,
    /// A model the provider suggests retrying on, when it skipped its own
    /// fallback attempt (it was rate limited, say); a hint, often empty.
    pub recommended_model: String,
}

impl Refusal {
    /// The category as shown: the provider's, written out, or "unspecified".
    pub fn shown_category(&self) -> &str {
        if self.category.is_empty() { "unspecified" } else { area(&self.category) }
    }
}

/// A policy area as people read it: the provider's short names written
/// out; anything else as the provider gave it, and empty stays empty.
pub fn area(category: &str) -> &str {
    match category {
        "cyber" => "cybersecurity",
        "bio" => "biology",
        other => other,
    }
}

/// The handoff notice, as the person reads it: display names and the
/// policy area written out, no area when the provider named none.
pub fn handoff_text(provider: &str, from: &str, to: &str, category: &str) -> String {
    let name = |id: &str| {
        crate::providers::catalog::models(provider)
            .into_iter()
            .find(|model| model.id == id)
            .map_or_else(|| id.to_owned(), |model| model.name)
    };
    let area = area(category);
    let area = if area.is_empty() { String::new() } else { format!(" ({area})") };
    format!("{} declined this step{area}; {} answered instead.", name(from), name(to))
}

/// Marks `output` as a refusal, from the provider's details (Anthropic's
/// `stop_details`: category, explanation, recommended_model, each nullable).
pub fn mark(output: &mut AssistantMessage, details: &Value) {
    let text = |key: &str| details[key].as_str().unwrap_or_default();
    output.extra.insert(
        REFUSAL.into(),
        json!({
            "category": text("category"),
            "explanation": text("explanation"),
            "recommendedModel": text("recommended_model"),
        }),
    );
}

/// The refusal `message` carries, if it is one.
pub fn of(message: &AssistantMessage) -> Option<Refusal> {
    let details = message.extra.get(REFUSAL)?;
    let text = |key: &str| details[key].as_str().unwrap_or_default().to_owned();
    Some(Refusal { category: text("category"), explanation: text("explanation"), recommended_model: text("recommendedModel") })
}

/// Marks `output` as served by `to` after `from` declined it (`category` as
/// the provider gave it), inside the provider's own fallback.
pub fn mark_fallback_served(output: &mut AssistantMessage, from: &str, to: &str, category: &str) {
    output.extra.insert(FALLBACK_SERVED.into(), json!({"from": from, "to": to, "category": category}));
}

/// Who served `message` in the provider's own fallback, when a model
/// declined it first: (declined model, serving model, category).
pub fn fallback_served(message: &AssistantMessage) -> Option<(String, String, String)> {
    let details = message.extra.get(FALLBACK_SERVED)?;
    let text = |key: &str| details[key].as_str().unwrap_or_default().to_owned();
    Some((text("from"), text("to"), text("category")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{StopReason, Usage};

    fn empty() -> AssistantMessage {
        AssistantMessage {
            content: Vec::new(),
            api: String::new(),
            provider: String::new(),
            model: String::new(),
            usage: Usage::empty(),
            stop_reason: StopReason::Pending,
            error_message: None,
            timestamp: 0,
            extra: serde_json::Map::new(),
        }
    }

    #[test]
    fn a_refusal_rides_in_extra_and_reads_back() {
        let mut message = empty();
        assert_eq!(of(&message), None);
        mark(&mut message, &json!({"category": "cyber", "explanation": "Declined.", "recommended_model": null}));
        let refusal = of(&message).expect("marked");
        assert_eq!(
            (refusal.category.as_str(), refusal.explanation.as_str(), refusal.recommended_model.as_str()),
            ("cyber", "Declined.", "")
        );
        // Shown written out, as the person and the agent read it.
        assert_eq!(refusal.shown_category(), "cybersecurity");
        // Null category: a refusal with no named policy area.
        mark(&mut message, &json!({"category": null, "explanation": null}));
        assert_eq!(of(&message).expect("marked").shown_category(), "unspecified");
    }

    #[test]
    fn a_served_fallback_rides_in_extra() {
        let mut message = empty();
        assert_eq!(fallback_served(&message), None);
        mark_fallback_served(&mut message, "claude-fable-5", "claude-opus-4-8", "cyber");
        assert_eq!(fallback_served(&message), Some(("claude-fable-5".into(), "claude-opus-4-8".into(), "cyber".into())));
    }

    #[test]
    fn the_handoff_notice_uses_display_names_and_plain_areas() {
        assert_eq!(
            handoff_text("anthropic", "claude-fable-5", "claude-opus-4-8", "cyber"),
            "Claude Fable 5 declined this step (cybersecurity); Claude Opus 4.8 answered instead."
        );
        // No named area: no parentheses. A model the catalog doesn't know
        // keeps its id.
        assert_eq!(
            handoff_text("anthropic", "claude-new-6", "claude-opus-4-8", ""),
            "claude-new-6 declined this step; Claude Opus 4.8 answered instead."
        );
        assert_eq!(area("bio"), "biology");
        assert_eq!(area("frontier_llm"), "frontier_llm");
    }
}
