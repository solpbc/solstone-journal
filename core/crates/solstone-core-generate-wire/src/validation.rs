// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared usage logging and strict result validation.

use std::io;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

use serde_json::Value;

pub use crate::token_log::usage_for_log;

use crate::token_log::record_generate_usage;

const MAX_SAFE_FINISH_REASON_LENGTH: usize = 64;

/// A successfully parsed provider completion.
///
/// `text` and `finish_reason` are borrowed strings rather than optional/raw JSON values:
/// both provider lanes construct this view only after the shared local parser has rejected
/// malformed result shapes and finish reasons. Those failure modes are consequently
/// unrepresentable at this boundary.
pub struct ProviderResultView<'a> {
    pub journal_path: &'a Path,
    pub context: &'a str,
    pub model: &'a str,
    pub text: &'a str,
    pub finish_reason: &'a str,
    pub usage: &'a Value,
    pub json_output: bool,
    pub raw_response_snippet: Option<&'a str>,
    /// The provider returned thinking content, which marks a thinking-consumed
    /// reply even when no token count for it comes back (Anthropic's thinking
    /// blocks).
    pub thinking_seen: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SanitizedFinishReason {
    Stop,
    ToolCalls,
    MaxTokens,
    ContentFilter,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationFailure {
    ProviderResponseInvalid {
        raw_response_snippet: Option<String>,
    },
    IncompleteJson {
        finish_reason: SanitizedFinishReason,
    },
    /// The model reached its ceiling while thinking and wrote no visible output.
    ThinkingConsumedBudget { json_output: bool },
}

#[derive(Debug)]
pub struct ProviderResultAssessment {
    pub failure: Option<ValidationFailure>,
    pub token_log_error: Option<io::Error>,
}

pub fn assess_provider_result(view: ProviderResultView<'_>) -> ProviderResultAssessment {
    let token_log_error = if has_usage(view.usage) {
        record_generate_usage(
            view.journal_path,
            view.model,
            view.context,
            &usage_for_log(view.usage),
        )
        .err()
    } else {
        None
    };

    let finish_reason = sanitize_finish_reason(view.finish_reason);
    let blank = blank_visible_output(view.text);
    // Blank visible output is never a generation, whatever the finish reason. When
    // the ceiling was reached with thinking done, say so: the talent's budget was
    // spent on reasoning, not on a bad response. A tool-call turn is the one shape
    // that carries no text by design.
    let failure = if blank
        && finish_reason == SanitizedFinishReason::MaxTokens
        && (view.thinking_seen || reasoning_tokens(view.usage) > 0)
    {
        Some(ValidationFailure::ThinkingConsumedBudget {
            json_output: view.json_output,
        })
    } else if view.json_output && finish_reason != SanitizedFinishReason::Stop {
        Some(ValidationFailure::IncompleteJson { finish_reason })
    } else if blank && finish_reason != SanitizedFinishReason::ToolCalls {
        Some(ValidationFailure::ProviderResponseInvalid {
            raw_response_snippet: view.raw_response_snippet.map(str::to_owned),
        })
    } else {
        None
    };

    ProviderResultAssessment {
        failure,
        token_log_error,
    }
}

pub fn sanitize_finish_reason(value: &str) -> SanitizedFinishReason {
    if value.chars().count() > MAX_SAFE_FINISH_REASON_LENGTH {
        return SanitizedFinishReason::Unknown;
    }
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty()
        || !normalized.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
    {
        return SanitizedFinishReason::Unknown;
    }
    match normalized.as_str() {
        "stop" => SanitizedFinishReason::Stop,
        "tool_calls" => SanitizedFinishReason::ToolCalls,
        "length" | "max_tokens" => SanitizedFinishReason::MaxTokens,
        "content_filter" => SanitizedFinishReason::ContentFilter,
        _ => SanitizedFinishReason::Unknown,
    }
}

fn has_usage(usage: &Value) -> bool {
    matches!(usage, Value::Object(values) if !values.is_empty())
}

fn reasoning_tokens(usage: &Value) -> u64 {
    usage
        .get("reasoning_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn blank_visible_output(text: &str) -> bool {
    text.trim().is_empty()
}

#[cfg(test)]
pub(crate) fn isolated_journal_dir(purpose: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static SEQ: AtomicUsize = AtomicUsize::new(1);
    loop {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("solstone-gw-{purpose}-{}-{n}", std::process::id()));
        match std::fs::create_dir(&path) {
            Ok(()) => return path,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("create isolated journal dir {}: {error}", path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;

    fn temp_journal() -> std::path::PathBuf {
        isolated_journal_dir("validation")
    }

    fn view<'a>(
        journal: &'a Path,
        text: &'a str,
        finish_reason: &'a str,
        usage: &'a Value,
    ) -> ProviderResultView<'a> {
        ProviderResultView {
            journal_path: journal,
            context: "test.generate",
            model: "model",
            text,
            finish_reason,
            usage,
            json_output: false,
            raw_response_snippet: None,
            thinking_seen: false,
        }
    }

    fn only_token_log(journal: &std::path::Path) -> std::path::PathBuf {
        let files = crate::token_log::token_log_files(journal);
        assert_eq!(files.len(), 1);
        files[0].clone()
    }

    #[test]
    fn normalizes_all_usage_keys_and_python_compatibility_rules() {
        assert_eq!(
            usage_for_log(&json!({
                "input_tokens": 2,
                "output_tokens": 3,
                "cached_input_tokens": 4,
                "reasoning_tokens": 5,
                "cache_creation_tokens": 6,
                "requests": 7,
            })),
            json!({
                "input_tokens": 2,
                "output_tokens": 3,
                "total_tokens": 5,
                "cached_tokens": 4,
                "reasoning_tokens": 5,
                "cache_creation_tokens": 6,
                "requests": 7,
            })
        );
    }

    #[test]
    fn json_truncation_is_reported_after_logging() {
        let journal = temp_journal();
        let usage = json!({"input_tokens": 2});
        let mut result = view(&journal, "{\"partial\": ", "max_tokens", &usage);
        result.json_output = true;
        let assessment = assess_provider_result(result);
        assert_eq!(
            assessment.failure,
            Some(ValidationFailure::IncompleteJson {
                finish_reason: SanitizedFinishReason::MaxTokens,
            })
        );
        assert!(assessment.token_log_error.is_none());
        let text = fs::read_to_string(only_token_log(&journal)).unwrap();
        let line: Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(line["usage"]["input_tokens"], 2);
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn reasoning_tokens_usage_reaches_token_log_file() {
        let journal = temp_journal();
        let usage = json!({"input_tokens": 2, "output_tokens": 3, "reasoning_tokens": 5});
        let assessment = assess_provider_result(view(&journal, "useful output", "stop", &usage));
        assert!(assessment.token_log_error.is_none());
        let token_file = only_token_log(&journal);
        let line: Value =
            serde_json::from_str(fs::read_to_string(token_file).unwrap().trim()).unwrap();
        assert_eq!(line["usage"]["reasoning_tokens"], 5);
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn blank_visible_output_is_never_a_generation() {
        let journal = temp_journal();
        let usage = json!({});
        for finish_reason in ["stop", "max_tokens", "length", "content_filter", "odd"] {
            assert_eq!(
                assess_provider_result(view(&journal, "  ", finish_reason, &usage)).failure,
                Some(ValidationFailure::ProviderResponseInvalid {
                    raw_response_snippet: None,
                }),
                "{finish_reason}"
            );
        }
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn a_ceiling_spent_on_thinking_has_its_own_reason() {
        let journal = temp_journal();
        let reasoned = json!({"output_tokens": 700, "reasoning_tokens": 700});
        let silent = json!({"output_tokens": 700});
        assert_eq!(
            assess_provider_result(view(&journal, "", "max_tokens", &reasoned)).failure,
            Some(ValidationFailure::ThinkingConsumedBudget { json_output: false })
        );
        // Anthropic reports no thinking count; its thinking blocks are the signal.
        let mut seen = view(&journal, "", "max_tokens", &silent);
        seen.thinking_seen = true;
        seen.json_output = true;
        assert_eq!(
            assess_provider_result(seen).failure,
            Some(ValidationFailure::ThinkingConsumedBudget { json_output: true })
        );
        // Visible output that ran out is not a thinking failure.
        assert_eq!(
            assess_provider_result(view(&journal, "partial", "max_tokens", &reasoned)).failure,
            None
        );
        // Blank at a natural stop is a bad response even with reasoning done.
        assert_eq!(
            assess_provider_result(view(&journal, "", "stop", &reasoned)).failure,
            Some(ValidationFailure::ProviderResponseInvalid {
                raw_response_snippet: None,
            })
        );
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn unsafe_finish_reason_is_sanitized_without_non_json_rejection() {
        let journal = temp_journal();
        let usage = json!({});
        let unsafe_reason = "provider metadata: <secret>";
        assert_eq!(
            sanitize_finish_reason(unsafe_reason),
            SanitizedFinishReason::Unknown
        );
        assert_eq!(
            assess_provider_result(view(&journal, "useful answer", unsafe_reason, &usage)).failure,
            None
        );
    }

    #[test]
    fn a_tool_call_turn_carries_no_text_by_design() {
        let journal = temp_journal();
        let usage = json!({});
        assert_eq!(
            sanitize_finish_reason("tool_calls"),
            SanitizedFinishReason::ToolCalls
        );
        assert_eq!(
            assess_provider_result(view(&journal, "", "tool_calls", &usage)).failure,
            None
        );
        let _ = fs::remove_dir_all(journal);
    }
}
