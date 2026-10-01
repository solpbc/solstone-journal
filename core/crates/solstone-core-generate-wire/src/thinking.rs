// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The owner's thinking choice on their own model, and each provider's reading of it.
//!
//! Thinking is off by default everywhere. On the owner's own model (a cloud key or
//! their own endpoint) one setting, `providers.byo_thinking_budget`, may turn it on
//! at 8k, 16k or 32k. The budget is always added to the talent's visible output
//! budget, never taken from it.
//!
//! "Off" is not "send nothing": with no thinking control, a default-reasoning cloud
//! model can spend a small talent's whole ceiling reasoning and return nothing. So
//! each cloud arm sends the lowest setting the model accepts. Which settings a model
//! accepts changes with every provider release, so the arms never consult a model
//! catalog. They send the first candidate below and step to the next only when the
//! provider refuses the thinking field itself.

use serde_json::{Map, Value, json};

/// The config key under `providers` that holds the owner's choice.
pub const BYO_THINKING_BUDGET_KEY: &str = "byo_thinking_budget";

/// The budgets an owner may choose. Anything else reads as off.
pub const BYO_THINKING_BUDGETS: [u64; 3] = [8_192, 16_384, 32_768];

/// Room for the reasoning a cloud model still does at its lowest setting.
///
/// No cloud provider has a true zero on every current model: `o4-mini` reasoned
/// 128 tokens at effort `low`, Claude Opus 5 thinks at `low`, and Gemini 3.1 Pro
/// reasoned 489 tokens under a thinking budget of 128. Reasoning shares the
/// output ceiling on all three, so without this a small talent's visible budget
/// could be spent before it writes anything.
pub const OFF_REASONING_HEADROOM: u64 = 1_024;

/// Gemini's largest accepted `maxOutputTokens`.
const GOOGLE_OUTPUT_LIMIT: u64 = 65_535;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thinking {
    Off,
    Budget(u64),
}

impl Thinking {
    pub const fn budget(self) -> u64 {
        match self {
            Self::Off => 0,
            Self::Budget(budget) => budget,
        }
    }
}

/// The owner's choice, normalized: absent, `0` or any unlisted value is off.
pub fn byo_thinking(config: &Map<String, Value>) -> Thinking {
    config
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get(BYO_THINKING_BUDGET_KEY))
        .and_then(Value::as_u64)
        .filter(|budget| BYO_THINKING_BUDGETS.contains(budget))
        .map_or(Thinking::Off, Thinking::Budget)
}

/// How much longer than the lane default a reply may take for its thinking room,
/// at a conservative 64 tokens a second. A caller's own timeout is never stretched.
pub(crate) fn thinking_time(room_tokens: u64) -> std::time::Duration {
    std::time::Duration::from_secs(room_tokens.div_ceil(64))
}

/// Thinking room on top of a cloud model's visible budget.
pub(crate) fn cloud_room(thinking: Thinking) -> u64 {
    cloud_thinking_room(thinking)
}

fn cloud_thinking_room(thinking: Thinking) -> u64 {
    match thinking {
        Thinking::Off => OFF_REASONING_HEADROOM,
        Thinking::Budget(budget) => budget,
    }
}

/// The output ceiling sent to OpenAI and Anthropic.
pub(crate) fn shared_ceiling(visible: u64, thinking: Thinking) -> u64 {
    visible.saturating_add(cloud_thinking_room(thinking))
}

/// Gemini's `maxOutputTokens`: its total cap clamps the thinking part, never the
/// talent's visible part.
pub(crate) fn google_ceiling(visible: u64, thinking: Thinking) -> u64 {
    let room = GOOGLE_OUTPUT_LIMIT.saturating_sub(visible);
    visible.saturating_add(cloud_thinking_room(thinking).min(room))
}

/// Headroom on the owner's own endpoint, where no thinking field is ever sent: a
/// model that thinks on its own gets the chosen budget as room to finish.
pub(crate) fn endpoint_headroom(thinking: Thinking) -> u64 {
    thinking.budget()
}

/// OpenAI Responses `reasoning.effort`, in the order to try. `None` sends no
/// `reasoning` object, which is the only form a non-reasoning model accepts.
pub(crate) fn openai_efforts(thinking: Thinking) -> &'static [Option<&'static str>] {
    match thinking {
        Thinking::Off => &[Some("none"), Some("low"), None],
        Thinking::Budget(8_192) => &[Some("medium"), Some("low"), None],
        Thinking::Budget(16_384) => &[Some("high"), Some("medium"), None],
        Thinking::Budget(_) => &[Some("xhigh"), Some("high"), None],
    }
}

/// OpenAI names the refused parameter, so the step-down keys on that and nothing else.
pub(crate) fn openai_refused_thinking(status: u16, body: &str) -> bool {
    status == 400
        && serde_json::from_str::<Value>(body).is_ok_and(|body| {
            body.pointer("/error/param").and_then(Value::as_str) == Some("reasoning.effort")
        })
}

/// One Anthropic thinking configuration: the `thinking` object and the
/// `output_config.effort`, either of which may be absent.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AnthropicThinking {
    pub thinking: Option<Value>,
    pub effort: Option<&'static str>,
}

/// Anthropic configurations in the order to try.
///
/// Current models take adaptive thinking and an effort; Haiku 4.5 and older take
/// neither and need a fixed `budget_tokens`.
pub(crate) fn anthropic_candidates(thinking: Thinking) -> Vec<AnthropicThinking> {
    let adaptive = || Some(json!({"type": "adaptive"}));
    let mut candidates = match thinking {
        Thinking::Off => vec![AnthropicThinking {
            thinking: None,
            effort: Some("low"),
        }],
        Thinking::Budget(budget) => {
            let mut steps = vec![AnthropicThinking {
                thinking: adaptive(),
                effort: Some(match budget {
                    8_192 => "medium",
                    16_384 => "high",
                    _ => "xhigh",
                }),
            }];
            if budget > 16_384 {
                steps.push(AnthropicThinking {
                    thinking: adaptive(),
                    effort: Some("high"),
                });
            }
            steps.push(AnthropicThinking {
                thinking: Some(json!({"type": "enabled", "budget_tokens": budget})),
                effort: None,
            });
            steps
        }
    };
    if thinking == Thinking::Off {
        candidates.push(AnthropicThinking {
            thinking: None,
            effort: None,
        });
    }
    candidates
}

/// Anthropic returns no parameter name, but every refusal of a thinking setting
/// names effort or thinking in its message.
pub(crate) fn anthropic_refused_thinking(status: u16, body: &str) -> bool {
    status == 400
        && serde_json::from_str::<Value>(body).is_ok_and(|body| {
            body.pointer("/error/type").and_then(Value::as_str) == Some("invalid_request_error")
                && body
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .is_some_and(|message| {
                        let message = message.to_ascii_lowercase();
                        message.contains("effort") || message.contains("thinking")
                    })
        })
}

pub(crate) fn apply_anthropic(body: &mut Value, candidate: &AnthropicThinking) {
    if let Some(thinking) = &candidate.thinking {
        body["thinking"] = thinking.clone();
    }
    if let Some(effort) = candidate.effort {
        body["output_config"] = json!({"effort": effort});
    }
}

/// Gemini `thinkingBudget` values in the order to try. Models that must think
/// refuse `0` (floors of 128 and 512 were measured), and Gemini 2.5 Flash caps a
/// budget at 24,576.
pub(crate) fn google_budgets(thinking: Thinking) -> &'static [u64] {
    match thinking {
        Thinking::Off => &[0, 128, 512],
        Thinking::Budget(8_192) => &[8_192],
        Thinking::Budget(16_384) => &[16_384],
        Thinking::Budget(_) => &[32_768, 24_576],
    }
}

/// Gemini answers a refused budget with a 400 `INVALID_ARGUMENT`, sometimes with no
/// more detail than that, so any such refusal that is not about the context window
/// steps to the next budget.
pub(crate) fn google_refused_thinking(status: u16, body: &str, context_window: bool) -> bool {
    status == 400
        && !context_window
        && serde_json::from_str::<Value>(body).is_ok_and(|body| {
            body.pointer("/error/status").and_then(Value::as_str) == Some("INVALID_ARGUMENT")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(value: Value) -> Map<String, Value> {
        value.as_object().expect("object").clone()
    }

    #[test]
    fn only_the_three_listed_budgets_turn_thinking_on() {
        for (value, expected) in [
            (json!({}), Thinking::Off),
            (json!({"providers": {}}), Thinking::Off),
            (
                json!({"providers": {"byo_thinking_budget": 0}}),
                Thinking::Off,
            ),
            (
                json!({"providers": {"byo_thinking_budget": 5_000}}),
                Thinking::Off,
            ),
            (
                json!({"providers": {"byo_thinking_budget": "8192"}}),
                Thinking::Off,
            ),
            (
                json!({"providers": {"byo_thinking_budget": 8_192}}),
                Thinking::Budget(8_192),
            ),
            (
                json!({"providers": {"byo_thinking_budget": 16_384}}),
                Thinking::Budget(16_384),
            ),
            (
                json!({"providers": {"byo_thinking_budget": 32_768}}),
                Thinking::Budget(32_768),
            ),
        ] {
            assert_eq!(byo_thinking(&config(value.clone())), expected, "{value}");
        }
    }

    #[test]
    fn a_budget_is_added_to_the_visible_budget_and_never_taken_from_it() {
        assert_eq!(shared_ceiling(700, Thinking::Off), 1_724);
        assert_eq!(shared_ceiling(700, Thinking::Budget(8_192)), 8_892);
        assert_eq!(google_ceiling(700, Thinking::Off), 1_724);
        assert_eq!(google_ceiling(700, Thinking::Budget(32_768)), 33_468);
        // Gemini's total cap clamps the thinking part only.
        assert_eq!(google_ceiling(40_000, Thinking::Budget(32_768)), 65_535);
        assert_eq!(google_ceiling(70_000, Thinking::Budget(32_768)), 70_000);
        assert_eq!(endpoint_headroom(Thinking::Off), 0);
        assert_eq!(endpoint_headroom(Thinking::Budget(16_384)), 16_384);
    }

    #[test]
    fn thinking_room_stretches_only_the_default_timeout() {
        assert_eq!(thinking_time(0), std::time::Duration::ZERO);
        assert_eq!(thinking_time(8_192), std::time::Duration::from_secs(128));
        assert_eq!(thinking_time(32_768), std::time::Duration::from_secs(512));
        assert_eq!(cloud_room(Thinking::Off), OFF_REASONING_HEADROOM);
    }

    #[test]
    fn every_ladder_ends_in_a_form_every_model_accepts() {
        for thinking in [
            Thinking::Off,
            Thinking::Budget(8_192),
            Thinking::Budget(16_384),
            Thinking::Budget(32_768),
        ] {
            assert_eq!(openai_efforts(thinking).last(), Some(&None));
            assert!(!google_budgets(thinking).is_empty());
            let candidates = anthropic_candidates(thinking);
            assert!(!candidates.is_empty());
        }
        let fixed = anthropic_candidates(Thinking::Budget(32_768));
        assert_eq!(
            fixed
                .last()
                .and_then(|candidate| candidate.thinking.clone()),
            Some(json!({"type": "enabled", "budget_tokens": 32_768}))
        );
    }

    #[test]
    fn step_down_keys_on_the_refused_field_not_on_any_400() {
        let effort = r#"{"error":{"message":"Unsupported value: 'none' is not supported with the 'o4-mini' model.","type":"invalid_request_error","param":"reasoning.effort"}}"#;
        let schema = r#"{"error":{"message":"Invalid schema","type":"invalid_request_error","param":"text.format.schema"}}"#;
        assert!(openai_refused_thinking(400, effort));
        assert!(!openai_refused_thinking(400, schema));
        assert!(!openai_refused_thinking(500, effort));

        let effort = r#"{"type":"error","error":{"type":"invalid_request_error","message":"This model does not support the effort parameter."}}"#;
        let tool = r#"{"type":"error","error":{"type":"invalid_request_error","message":"tools.0.name: String should match pattern"}}"#;
        assert!(anthropic_refused_thinking(400, effort));
        assert!(!anthropic_refused_thinking(400, tool));

        let budget = r#"{"error":{"code":400,"message":"Request contains an invalid argument.","status":"INVALID_ARGUMENT"}}"#;
        assert!(google_refused_thinking(400, budget, false));
        assert!(!google_refused_thinking(400, budget, true));
        assert!(!google_refused_thinking(429, budget, false));
    }
}
