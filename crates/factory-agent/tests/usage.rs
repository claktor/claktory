use factory_agent::types::{Cost, CostTier, ModelCost, Usage};
use factory_agent::usage::{calculate_context_tokens, calculate_cost};

fn luna() -> ModelCost {
    ModelCost {
        input: 0.2,
        output: 1.2,
        cache_read: 0.02,
        cache_write: 0.25,
        tiers: Some(vec![CostTier {
            input_tokens_above: 272_000,
            input: 0.4,
            output: 1.8,
            cache_read: 0.04,
            cache_write: 0.5,
            extra: Default::default(),
        }]),
        extra: Default::default(),
    }
}

fn usage(input: u64, output: u64, cache_read: u64, cache_write: u64) -> Usage {
    Usage {
        input,
        output,
        cache_read,
        cache_write,
        cache_write_1h: None,
        reasoning: Some(5),
        total_tokens: input + output + cache_read + cache_write,
        cost: Cost::ZERO,
    }
}

#[test]
fn cost_uses_base_rates_below_the_tier_and_tier_rates_above_it() {
    let mut below = usage(100, 20, 10, 2);
    let cost = calculate_cost(&luna(), &mut below);
    assert_eq!(cost, below.cost);
    assert_eq!(cost.input, 0.2 / 1_000_000.0 * 100.0);
    assert_eq!(cost.output, 1.2 / 1_000_000.0 * 20.0);
    assert_eq!(cost.cache_read, 0.02 / 1_000_000.0 * 10.0);
    assert_eq!(cost.cache_write, (0.25 * 2.0) / 1_000_000.0);
    assert_eq!(cost.total, cost.input + cost.output + cost.cache_read + cost.cache_write);
    let mut above = usage(200_000, 1_000, 80_000, 0);
    let cost = calculate_cost(&luna(), &mut above);
    assert_eq!(cost.input, 0.4 / 1_000_000.0 * 200_000.0);
    assert_eq!(cost.output, 1.8 / 1_000_000.0 * 1_000.0);
    let mut long = usage(0, 0, 0, 10);
    long.cache_write_1h = Some(4);
    let cost = calculate_cost(&luna(), &mut long);
    assert_eq!(cost.cache_write, (0.25 * 6.0 + 0.2 * 2.0 * 4.0) / 1_000_000.0);
}

#[test]
fn context_tokens_prefer_the_provider_total() {
    let mut usage = usage(1, 2, 3, 4);
    assert_eq!(calculate_context_tokens(&usage), 10);
    usage.total_tokens = 0;
    assert_eq!(calculate_context_tokens(&usage), 10);
    usage.total_tokens = 1236;
    assert_eq!(calculate_context_tokens(&usage), 1236);
}
