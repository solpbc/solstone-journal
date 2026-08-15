// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Small, native model-pricing table for persisted talent-run usage.

use std::cmp::Ordering;

const PICODOLLARS_PER_DOLLAR: u128 = 1_000_000_000_000;

#[derive(Clone, Copy)]
struct Price {
    input: u128,
    cached: u128,
    output: u128,
}

#[derive(Clone, Copy)]
struct Row {
    id: &'static str,
    provider: &'static str,
    family: &'static str,
    version: (u16, u16),
    price: Price,
}

/// Token accounting persisted on a completed talent run.
#[derive(Clone, Copy)]
pub struct Usage<'a> {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cached_tokens: u64,
    pub model_version: Option<&'a str>,
}

// These rows are the base rates from the installed genai-prices snapshot used
// by the retained Python route. The third-party catalog is mutable, so this
// deliberately tables only families configured by Sol rather than vendoring it.
const ROWS: &[Row] = &[
    Row {
        id: "gpt-5",
        provider: "openai",
        family: "base",
        version: (5, 0),
        price: Price {
            input: 1_250_000,
            cached: 125_000,
            output: 10_000_000,
        },
    },
    Row {
        id: "gpt-5.2",
        provider: "openai",
        family: "base",
        version: (5, 2),
        price: Price {
            input: 1_750_000,
            cached: 175_000,
            output: 14_000_000,
        },
    },
    Row {
        id: "gpt-5-mini",
        provider: "openai",
        family: "mini",
        version: (5, 0),
        price: Price {
            input: 250_000,
            cached: 25_000,
            output: 2_000_000,
        },
    },
    Row {
        id: "gemini-3-flash-preview",
        provider: "google",
        family: "flash",
        version: (3, 0),
        price: Price {
            input: 500_000,
            cached: 50_000,
            output: 3_000_000,
        },
    },
    Row {
        id: "claude-opus-4-6",
        provider: "anthropic",
        family: "opus",
        version: (4, 6),
        price: Price {
            input: 5_000_000,
            cached: 500_000,
            output: 25_000_000,
        },
    },
    Row {
        id: "claude-sonnet-4-6",
        provider: "anthropic",
        family: "sonnet",
        version: (4, 6),
        price: Price {
            input: 3_000_000,
            cached: 300_000,
            output: 15_000_000,
        },
    },
];

/// Return the persisted-run price in USD.
///
/// Recognized but untabled families deliberately return `None`: Python gets its
/// catalog at runtime, while the native route must not copy that mutable package.
pub fn agent_cost(model: Option<&str>, usage: Option<Usage<'_>>) -> Option<f64> {
    let usage = usage?;
    let model = usage
        .model_version
        .filter(|value| !value.is_empty())
        .or(model)?;
    let model = strip_effort_suffix(model);
    if is_local(model) {
        return Some(0.0);
    }
    let provider = provider(model)?;
    let row = ROWS
        .iter()
        .find(|row| row.provider == provider && row.id == model)
        .copied()
        .or_else(|| pricing_fallback(model, provider))?;
    let uncached = u128::from(usage.input_tokens.saturating_sub(usage.cached_tokens));
    let cached = u128::from(usage.cached_tokens);
    let output = u128::from(usage.output_tokens).checked_add(u128::from(usage.reasoning_tokens))?;
    let total = uncached
        .checked_mul(row.price.input)?
        .checked_add(cached.checked_mul(row.price.cached)?)?
        .checked_add(output.checked_mul(row.price.output)?)?;
    Some(total as f64 / PICODOLLARS_PER_DOLLAR as f64)
}

fn strip_effort_suffix(model: &str) -> &str {
    for suffix in ["-none", "-low", "-medium", "-high", "-xhigh"] {
        if let Some(stripped) = model.strip_suffix(suffix) {
            return stripped;
        }
    }
    model
}

fn is_local(model: &str) -> bool {
    matches!(model, "qwen3.5:9b" | "gemma-4-26b-a4b-it-mlx-4bit") || model.starts_with("local/")
}

fn provider(model: &str) -> Option<&'static str> {
    if model.starts_with("gpt") {
        Some("openai")
    } else if model.starts_with("gemini") {
        Some("google")
    } else if model.starts_with("claude") {
        Some("anthropic")
    } else {
        None
    }
}

fn pricing_fallback(model: &str, provider: &str) -> Option<Row> {
    let (family, _version) = parse_family(model, provider)?;
    ROWS.iter()
        .copied()
        .filter(|row| row.provider == provider && row.family == family)
        .max_by(|left, right| compare_version(left.version, right.version))
}

fn compare_version(left: (u16, u16), right: (u16, u16)) -> Ordering {
    left.cmp(&right)
}

fn parse_family<'a>(model: &'a str, provider: &str) -> Option<(&'a str, (u16, u16))> {
    match provider {
        "openai" => parse_openai(model),
        "google" => parse_gemini(model),
        "anthropic" => parse_anthropic(model),
        _ => None,
    }
}

fn parse_openai(model: &str) -> Option<(&str, (u16, u16))> {
    if model.starts_with("ft:") || model.contains("-image") {
        return None;
    }
    let rest = model.strip_prefix("gpt-")?;
    let (version, suffix) = split_version(rest)?;
    let family = match suffix {
        "" => "base",
        "-mini" => "mini",
        "-nano" => "nano",
        "-pro" => "pro",
        _ => return None,
    };
    Some((family, version))
}

fn parse_gemini(model: &str) -> Option<(&str, (u16, u16))> {
    let model = model.strip_suffix("-preview").unwrap_or(model);
    let rest = model.strip_prefix("gemini-")?;
    let (version, suffix) = split_version(rest)?;
    let family = match suffix {
        "-pro" => "pro",
        "-flash" => "flash",
        "-flash-lite" => "flash-lite",
        _ => return None,
    };
    Some((family, version))
}

fn parse_anthropic(model: &str) -> Option<(&str, (u16, u16))> {
    let rest = model.strip_prefix("claude-")?;
    let (family, rest) = rest.split_once('-')?;
    if !matches!(family, "opus" | "sonnet" | "haiku") {
        return None;
    }
    let mut values = rest.split('-');
    let major = values.next()?.parse().ok()?;
    let minor = values.next().map_or(Some(0), |value| value.parse().ok())?;
    if values.next().is_some() {
        return None;
    }
    Some((family, (major, minor)))
}

fn split_version(value: &str) -> Option<((u16, u16), &str)> {
    let digits = value.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let major = value[..digits].parse().ok()?;
    let mut rest = &value[digits..];
    let mut minor = 0;
    if let Some(after_dot) = rest.strip_prefix('.') {
        let digits = after_dot.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        minor = after_dot[..digits].parse().ok()?;
        rest = &after_dot[digits..];
    }
    Some(((major, minor), rest))
}

#[cfg(test)]
mod tests {
    use super::{Usage, agent_cost};

    fn usage(model_version: Option<&str>) -> Usage<'_> {
        Usage {
            input_tokens: 1,
            output_tokens: 1,
            reasoning_tokens: 0,
            cached_tokens: 0,
            model_version,
        }
    }

    #[test]
    fn configured_models_match_executed_python_oracle() {
        // Python oracle: calc_agent_cost(model, {"input_tokens": 1, "output_tokens": 1}).
        for (model, expected) in [
            ("claude-opus-4-7", 0.000_03),
            ("claude-sonnet-4-6", 0.000_018),
            ("gemini-3.5-flash", 0.000_003_5),
            ("gemma-4-26b-a4b-it-mlx-4bit", 0.0),
            ("gpt-5.4-mini", 0.000_002_25),
            ("gpt-5.5", 0.000_015_75),
            ("local/qwen3.5-4b", 0.0),
            ("qwen3.5:9b", 0.0),
        ] {
            assert_eq!(
                agent_cost(Some(model), Some(usage(None))),
                Some(expected),
                "{model}"
            );
        }
    }

    #[test]
    fn pinned_usage_and_model_version_oracles_match_python() {
        // Python oracle: calc_agent_cost("gpt-5.5", {"input_tokens": 10, "output_tokens": 3, "reasoning_tokens": 4, "cached_tokens": 2}).
        assert_eq!(
            agent_cost(
                Some("gpt-5.5"),
                Some(Usage {
                    input_tokens: 10,
                    output_tokens: 3,
                    reasoning_tokens: 4,
                    cached_tokens: 2,
                    model_version: None,
                }),
            ),
            Some(0.000_112_35)
        );
        // Python oracle: calc_agent_cost("gpt-5.5", {"input_tokens": 1, "output_tokens": 1, "model_version": "gpt-5"}).
        assert_eq!(
            agent_cost(Some("gpt-5.5"), Some(usage(Some("gpt-5")))),
            Some(0.000_011_25)
        );
        assert_eq!(
            agent_cost(Some("gpt-5.5-high"), Some(usage(None))),
            Some(0.000_015_75)
        );
        assert_eq!(
            agent_cost(Some("gpt-5.9"), Some(usage(None))),
            Some(0.000_015_75)
        );
        assert_eq!(agent_cost(Some("unknown"), Some(usage(None))), None);
        assert_eq!(agent_cost(Some("gpt-5.5"), None), None);
        assert_eq!(
            agent_cost(Some("local/qwen3.5-4b"), Some(usage(None))),
            Some(0.0)
        );
        assert_eq!(
            agent_cost(Some("claude-haiku-4-6"), Some(usage(None))),
            None
        );
    }
}
