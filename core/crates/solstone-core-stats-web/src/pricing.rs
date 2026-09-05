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
    let model = model.to_ascii_lowercase();
    if model == "local"
        || model.starts_with("local/")
        || model == "qwen3.5:9b"
        || model == "gemma-4-26b-a4b-it-mlx-4bit"
    {
        "local"
    } else if model.starts_with("gpt") {
        "openai"
    } else if model.starts_with("claude") {
        "anthropic"
    } else if model.starts_with("gemini") {
        "google"
    } else {
        "unknown"
    }
}

pub(super) fn calc_token_cost(entry: &Value) -> Option<Value> {
    let normalized = entry.get("model")?.as_str()?.to_ascii_lowercase();
    let model = strip_effort_suffix(&normalized);
    let usage_value = entry.get("usage")?;
    if !json_truthy(usage_value) {
        return None;
    }
    match provider(model) {
        "unknown" => None,
        "local" => Some(json!({"input_cost":0.0,"output_cost":0.0,"total_cost":0.0})),
        _ => {
            let usage = usage_value.as_object()?;
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

fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
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
    let family = pricing_family(model)?;
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

fn pricing_family(model: &str) -> Option<&'static str> {
    if let Some(rest) = model.strip_prefix("gpt-") {
        let (version, variant) = rest
            .strip_suffix("-mini")
            .map(|value| (value, "gpt-mini"))
            .or_else(|| rest.strip_suffix("-nano").map(|value| (value, "gpt-nano")))
            .or_else(|| rest.strip_suffix("-pro").map(|value| (value, "gpt-pro")))
            .unwrap_or((rest, "gpt"));
        return numeric_version(version).then_some(variant);
    }
    if let Some(rest) = model.strip_prefix("claude-") {
        for (kind, family) in [
            ("sonnet-", "claude-sonnet"),
            ("opus-", "claude-opus"),
            ("haiku-", "claude-haiku"),
        ] {
            if let Some(version) = rest.strip_prefix(kind) {
                return numeric_version(version).then_some(family);
            }
        }
        return None;
    }
    if model == "gemini-flash-latest" {
        return Some("gemini-flash");
    }
    if model == "gemini-pro-latest" {
        return Some("gemini-pro");
    }
    if model == "gemini-flash-lite-latest" {
        return Some("gemini-flash-lite");
    }
    let rest = model
        .strip_prefix("gemini-")?
        .strip_suffix("-preview")
        .unwrap_or(model.strip_prefix("gemini-")?);
    for (kind, family) in [
        ("-flash-lite", "gemini-flash-lite"),
        ("-flash", "gemini-flash"),
        ("-pro", "gemini-pro"),
    ] {
        if let Some(version) = rest.strip_suffix(kind) {
            return numeric_version(version).then_some(family);
        }
    }
    None
}

fn numeric_version(value: &str) -> bool {
    let mut parts = value.split('-');
    let version = parts.next().unwrap_or_default();
    parts.all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        && !version.is_empty()
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
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
    fn miss_behaviors_match_the_bounded_python_port() {
        assert!(calc_token_cost(&json!({"model":"not-a-model","usage":{}})).is_none());
        assert_eq!(
            calc_token_cost(&json!({"model":"local/qwen","usage":{"total_tokens":0}}))
                .expect("local")["total_cost"],
            0.0
        );
        assert!(calc_token_cost(&json!({"model":"local/qwen","usage":{}})).is_none());
        assert_eq!(
            calc_token_cost(&json!({"model":"local/qwen","usage":"wrong"}))
                .expect("truthy malformed local usage")["total_cost"],
            0.0
        );
        assert!(calc_token_cost(&json!({"model":"gpt-5.99","usage":{"total_tokens":0}})).is_some());
        assert!(calc_token_cost(&json!({"model":"gpt-5.99","usage":"wrong"})).is_none());
        assert!(calc_token_cost(&json!({"model":"gpt-not-priced","usage":{}})).is_none());
    }

    #[test]
    fn grounded_provider_and_pricing_classes_match_the_reference() {
        let usage = json!({"input_tokens":1000,"cached_tokens":200,"output_tokens":300,"reasoning_tokens":100,"total_tokens":1400});
        for model in [
            "local/qwen3.5-4b",
            "qwen3.5:9b",
            "gemma-4-26b-a4b-it-mlx-4bit",
        ] {
            assert_eq!(provider(model), "local", "{model}");
            assert_eq!(
                calc_token_cost(&json!({"model":model,"usage":usage})).expect("local pricing")["total_cost"],
                0.0,
                "{model}"
            );
        }
        for model in [
            "gpt-5.5",
            "gpt-5.4-mini",
            "claude-opus-4-7",
            "claude-sonnet-4-6",
            "gemini-3.5-flash",
            "claude-sonnet-4-5",
            "claude-sonnet-4-5-20250929",
            "claude-3-5-haiku-20241022",
            "gpt-5.5-high",
        ] {
            assert!(
                calc_token_cost(&json!({"model":model,"usage":usage})).is_some(),
                "{model}"
            );
        }
        let dated = calc_token_cost(&json!({
            "model":"claude-3-5-haiku-20241022",
            "usage":usage
        }))
        .expect("dated Anthropic snapshot")["total_cost"]
            .as_f64()
            .expect("dated cost");
        assert!((dated - 0.002_256).abs() < f64::EPSILON);
        let dated_sonnet = calc_token_cost(&json!({
            "model":"claude-sonnet-4-5-20250929",
            "usage":usage
        }))
        .expect("dated Sonnet snapshot")["total_cost"]
            .as_f64()
            .expect("dated Sonnet cost");
        assert!((dated_sonnet - 0.008_46).abs() < f64::EPSILON);
        assert_eq!(provider("gpt-not-priced"), "openai");
        assert!(calc_token_cost(&json!({"model":"gpt-not-priced","usage":usage})).is_none());
    }
}
