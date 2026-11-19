use crate::types::{Cost, ModelCost, Usage};

/// Tokens the context held for this response: what approaches the context window.
pub fn calculate_context_tokens(usage: &Usage) -> u64 {
    if usage.total_tokens != 0 { usage.total_tokens } else { usage.input + usage.output + usage.cache_read + usage.cache_write }
}

pub fn calculate_cost(cost: &ModelCost, usage: &mut Usage) -> Cost {
    let input_tokens = usage.input + usage.cache_read + usage.cache_write;
    let mut rates = (cost.input, cost.output, cost.cache_read, cost.cache_write);
    let mut matched: Option<u64> = None;
    for tier in cost.tiers.iter().flatten() {
        if input_tokens > tier.input_tokens_above && matched.is_none_or(|threshold| tier.input_tokens_above > threshold) {
            rates = (tier.input, tier.output, tier.cache_read, tier.cache_write);
            matched = Some(tier.input_tokens_above);
        }
    }
    let (input, output, cache_read, cache_write) = rates;
    let long_write = usage.cache_write_1h.unwrap_or(0) as f64;
    let short_write = usage.cache_write as f64 - long_write;
    usage.cost.input = (input / 1_000_000.0) * usage.input as f64;
    usage.cost.output = (output / 1_000_000.0) * usage.output as f64;
    usage.cost.cache_read = (cache_read / 1_000_000.0) * usage.cache_read as f64;
    usage.cost.cache_write = (cache_write * short_write + input * 2.0 * long_write) / 1_000_000.0;
    usage.cost.total = usage.cost.input + usage.cost.output + usage.cost.cache_read + usage.cost.cache_write;
    usage.cost.clone()
}
