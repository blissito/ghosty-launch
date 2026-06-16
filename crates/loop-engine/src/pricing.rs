//! Cost estimation for DeepSeek API usage.
//!
//! Extracted from `ghostycode/crates/tui/src/pricing.rs`, trimmed to the single
//! entry point the vendored client's tests exercise
//! (`calculate_turn_cost_from_usage`) plus its private helpers. The original
//! imports `chrono` only to thread a `_now` value that `pricing_for_model_at`
//! never reads, so we drop the time dependency entirely (behavior identical).
//! Pricing tables are copied verbatim.

use crate::models::Usage;

/// Per-million-token pricing for a model.
#[derive(Debug, Clone, Copy)]
struct CurrencyPricing {
    input_cache_hit_per_million: f64,
    input_cache_miss_per_million: f64,
    output_per_million: f64,
}

/// Per-million-token pricing for a model in both official currencies.
#[derive(Debug, Clone, Copy)]
struct ModelPricing {
    usd: CurrencyPricing,
    #[allow(dead_code)]
    cny: CurrencyPricing,
}

/// Look up pricing for a model name (verbatim, sans the unused `_now`).
fn pricing_for_model(model: &str) -> Option<ModelPricing> {
    let lower = model.to_lowercase();
    if lower.starts_with("deepseek-ai/") {
        // NVIDIA NIM-hosted DeepSeek uses NVIDIA's catalog/account terms, not
        // DeepSeek Platform pricing. Avoid showing misleading DeepSeek costs.
        return None;
    }
    if !lower.contains("deepseek") {
        return None;
    }
    if lower.contains("v4-pro") || lower.contains("v4pro") {
        Some(ModelPricing {
            usd: CurrencyPricing {
                input_cache_hit_per_million: 0.003625,
                input_cache_miss_per_million: 0.435,
                output_per_million: 0.87,
            },
            cny: CurrencyPricing {
                input_cache_hit_per_million: 0.025,
                input_cache_miss_per_million: 3.0,
                output_per_million: 6.0,
            },
        })
    } else {
        // deepseek-v4-flash pricing.
        Some(ModelPricing {
            usd: CurrencyPricing {
                input_cache_hit_per_million: 0.0028,
                input_cache_miss_per_million: 0.14,
                output_per_million: 0.28,
            },
            cny: CurrencyPricing {
                input_cache_hit_per_million: 0.02,
                input_cache_miss_per_million: 1.0,
                output_per_million: 2.0,
            },
        })
    }
}

/// Calculate cost from provider usage, honoring DeepSeek context-cache fields.
#[must_use]
pub fn calculate_turn_cost_from_usage(model: &str, usage: &Usage) -> Option<f64> {
    let pricing = pricing_for_model(model)?;
    Some(calculate_turn_cost_from_usage_with_pricing(pricing.usd, usage))
}

fn calculate_turn_cost_from_usage_with_pricing(pricing: CurrencyPricing, usage: &Usage) -> f64 {
    let hit_tokens = usage.prompt_cache_hit_tokens.unwrap_or(0);
    let miss_tokens = usage
        .prompt_cache_miss_tokens
        .unwrap_or_else(|| usage.input_tokens.saturating_sub(hit_tokens));
    let accounted_input = hit_tokens.saturating_add(miss_tokens);
    let uncategorized_input = usage.input_tokens.saturating_sub(accounted_input);

    let hit_cost = (hit_tokens as f64 / 1_000_000.0) * pricing.input_cache_hit_per_million;
    let miss_cost = ((miss_tokens.saturating_add(uncategorized_input)) as f64 / 1_000_000.0)
        * pricing.input_cache_miss_per_million;
    let reasoning = usage.reasoning_tokens.unwrap_or(0);
    let effective_output = usage.output_tokens.saturating_add(reasoning);
    let output_cost = (effective_output as f64 / 1_000_000.0) * pricing.output_per_million;
    hit_cost + miss_cost + output_cost
}
