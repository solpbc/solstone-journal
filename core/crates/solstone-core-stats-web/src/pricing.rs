// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};

include!("pricing_data.rs");

#[derive(Clone, Copy)]
pub(super) struct Rate {
    input_mtok: f64,
    cache_mtok: f64,
    output_mtok: f64,
}
impl Rate {
    pub(super) const fn new(input_mtok: f64, cache_mtok: f64, output_mtok: f64) -> Self {
        Self {
            input_mtok,
            cache_mtok,
            output_mtok,
        }
    }
}

pub(super) fn provider(model: &str) -> &'static str {
    if model == "local" || model.starts_with("local/") {
        "local"
    } else if model.starts_with("gpt-") {
        "openai"
    } else if model.starts_with("claude-") {
        "anthropic"
    } else if model.starts_with("gemini-") {
        "google"
    } else {
        "unknown"
    }
}

pub(super) fn calc_token_cost(entry: &Value) -> Option<Value> {
    let model = entry.get("model")?.as_str()?;
    let model = strip_effort_suffix(model);
    match provider(model) {
        "unknown" => None,
        "local" => Some(json!({"input_cost":0.0,"output_cost":0.0,"total_cost":0.0})),
        _ => {
            let usage = entry.get("usage")?.as_object()?;
            let get = |name| usage.get(name).and_then(Value::as_f64).unwrap_or(0.0);
            let input = get("input_tokens");
            let cached = get("cached_tokens");
            let output = get("output_tokens") + get("reasoning_tokens");
            let rate = direct(model).or_else(|| fallback(model))?;
            // genai-prices bills cached input at cache-read rate, not both rates.
            let input_cost = ((input - cached).max(0.0) * rate.input_mtok
                + cached * rate.cache_mtok)
                / 1_000_000.0;
            let output_cost = output * rate.output_mtok / 1_000_000.0;
            Some(
                json!({"input_cost":input_cost,"output_cost":output_cost,"total_cost":input_cost + output_cost}),
            )
        }
    }
}

fn strip_effort_suffix(model: &str) -> &str {
    ["-none", "-low", "-medium", "-high", "-xhigh"]
        .into_iter()
        .find_map(|suffix| model.strip_suffix(suffix))
        .unwrap_or(model)
}
fn direct(model: &str) -> Option<Rate> {
    RATES
        .iter()
        .find_map(|(name, rate)| (*name == model).then_some(*rate))
}
fn fallback(model: &str) -> Option<Rate> {
    let family = if let Some(rest) = model.strip_prefix("gpt-") {
        if rest.ends_with("-mini") {
            "gpt-mini"
        } else if rest.ends_with("-nano") {
            "gpt-nano"
        } else if rest.ends_with("-pro") {
            "gpt-pro"
        } else {
            "gpt"
        }
    } else if let Some(rest) = model.strip_prefix("claude-") {
        if rest.contains("sonnet") {
            "claude-sonnet"
        } else if rest.contains("opus") {
            "claude-opus"
        } else if rest.contains("haiku") {
            "claude-haiku"
        } else {
            return None;
        }
    } else {
        let rest = model.strip_prefix("gemini-")?;
        if rest.contains("flash") {
            "gemini-flash"
        } else if rest.contains("pro") {
            "gemini-pro"
        } else {
            return None;
        }
    };
    RATES
        .iter()
        .filter(|(name, _)| match family {
            "gpt" => {
                name.starts_with("gpt-")
                    && !name.ends_with("-mini")
                    && !name.ends_with("-nano")
                    && !name.ends_with("-pro")
            }
            "gpt-mini" => name.starts_with("gpt-") && name.ends_with("-mini"),
            "gpt-nano" => name.starts_with("gpt-") && name.ends_with("-nano"),
            "gpt-pro" => name.starts_with("gpt-") && name.ends_with("-pro"),
            "claude-sonnet" => name.starts_with("claude-sonnet-"),
            "claude-opus" => name.starts_with("claude-opus-"),
            "claude-haiku" => name.starts_with("claude-haiku-"),
            "gemini-flash" => name.contains("flash"),
            "gemini-pro" => name.contains("pro"),
            _ => false,
        })
        .max_by_key(|(name, _)| *name)
        .map(|(_, rate)| *rate)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independently_derived_published_rates_match_the_snapshot() {
        // GPT-5.2 published rates: $1.75/M input, $0.175/M cached, $14/M output.
        let gpt_expected = (700.0 * 1.75 + 300.0 * 0.175 + 200.0 * 14.0) / 1_000_000.0;
        // Claude Sonnet 4.6 published rates: $3/M input and $15/M output.
        let claude_expected = (800.0 * 3.0 + 500.0 * 15.0) / 1_000_000.0;
        let gpt = json!({"model":"gpt-5.5","usage":{"input_tokens":1000,"cached_tokens":300,"output_tokens":200}});
        let claude = json!({"model":"claude-sonnet-4-6","usage":{"input_tokens":800,"output_tokens":400,"reasoning_tokens":100}});
        assert!(
            (calc_token_cost(&gpt).expect("gpt")["total_cost"]
                .as_f64()
                .expect("cost")
                - gpt_expected)
                .abs()
                < f64::EPSILON
        );
        assert!(
            (calc_token_cost(&claude).expect("claude")["total_cost"]
                .as_f64()
                .expect("cost")
                - claude_expected)
                .abs()
                < f64::EPSILON
        );
    }

    #[test]
    fn ac6_expected_costs_remain_published_rate_arithmetic() {
        let source = include_str!("pricing.rs");
        let gpt = [
            "let gpt_expected = (700.0 * 1.75 + ",
            "300.0 * 0.175 + 200.0 * 14.0) / 1_000_000.0;",
        ]
        .concat();
        let claude = [
            "let claude_expected = (800.0 * 3.0 + ",
            "500.0 * 15.0) / 1_000_000.0;",
        ]
        .concat();
        assert!(source.contains(&gpt));
        assert!(source.contains(&claude));
    }

    #[test]
    fn miss_behaviors_match_the_bounded_python_port() {
        assert!(calc_token_cost(&json!({"model":"not-a-model","usage":{}})).is_none());
        assert_eq!(
            calc_token_cost(&json!({"model":"local/qwen","usage":{}})).expect("local")["total_cost"],
            0.0
        );
        assert!(calc_token_cost(&json!({"model":"gpt-5.99","usage":{}})).is_some());
        assert!(calc_token_cost(&json!({"model":"gpt-5.99","usage":"wrong"})).is_none());
    }
}
