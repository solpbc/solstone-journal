// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared usage logging, strict result validation, and responsiveness handling.

use std::io;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

use serde_json::Value;

pub use crate::token_log::usage_for_log;

use crate::responsiveness::{
    NON_RESPONSIVE_RAW_OUTPUT_CAP_CHARS, ResponsivenessSignal, classify_output_responsiveness,
};
use crate::token_log::{GenerateUsageMetadata, record_generate_usage};

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
    pub enforce_responsiveness: bool,
    pub raw_response_snippet: Option<&'a str>,
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
    NonResponsiveOutput,
}

#[derive(Debug)]
pub struct ProviderResultAssessment {
    pub failure: Option<ValidationFailure>,
    pub token_log_error: Option<io::Error>,
}

pub fn assess_provider_result(view: ProviderResultView<'_>) -> ProviderResultAssessment {
    let verdict = view
        .enforce_responsiveness
        .then(|| classify_output_responsiveness(view.text));
    let non_responsive = verdict.is_some_and(|verdict| verdict.non_responsive);
    let capped_output = non_responsive.then(|| {
        view.text
            .chars()
            .take(NON_RESPONSIVE_RAW_OUTPUT_CAP_CHARS)
            .collect::<String>()
    });
    let matched_signal = verdict
        .and_then(|verdict| verdict.matched_signal)
        .map(ResponsivenessSignal::as_log_value);

    let token_log_error = if has_usage(view.usage) || non_responsive {
        let metadata = GenerateUsageMetadata {
            non_responsive_output: capped_output.as_deref(),
            non_responsive_matched_signal: matched_signal,
        };
        record_generate_usage(
            view.journal_path,
            view.model,
            view.context,
            &usage_for_log(view.usage),
            non_responsive.then_some(&metadata),
        )
        .err()
    } else {
        None
    };

    let finish_reason = sanitize_finish_reason(view.finish_reason);
    let failure = if view.json_output && finish_reason != SanitizedFinishReason::Stop {
        Some(ValidationFailure::IncompleteJson { finish_reason })
    } else if finish_reason == SanitizedFinishReason::Stop && blank_visible_output(view.text) {
        Some(ValidationFailure::ProviderResponseInvalid {
            raw_response_snippet: view.raw_response_snippet.map(str::to_owned),
        })
    } else if non_responsive {
        Some(ValidationFailure::NonResponsiveOutput)
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

    use serde_json::{Map, json};

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
            enforce_responsiveness: true,
            raw_response_snippet: None,
        }
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
    fn json_truncation_wins_over_responsiveness_after_logging() {
        let journal = temp_journal();
        let usage = json!({"input_tokens": 2});
        let mut result = view(
            &journal,
            "I cannot complete that request.",
            "max_tokens",
            &usage,
        );
        result.json_output = true;
        let assessment = assess_provider_result(result);
        assert_eq!(
            assessment.failure,
            Some(ValidationFailure::IncompleteJson {
                finish_reason: SanitizedFinishReason::MaxTokens,
            })
        );
        assert!(assessment.token_log_error.is_none());
        let text = fs::read_to_string(
            fs::read_dir(journal.join("tokens"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path(),
        )
        .unwrap();
        let line: Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(line["non_responsive_matched_signal"], "i cannot");
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn reasoning_tokens_usage_reaches_token_log_file() {
        let journal = temp_journal();
        let usage = json!({"input_tokens": 2, "output_tokens": 3, "reasoning_tokens": 5});
        let assessment = assess_provider_result(view(&journal, "useful output", "stop", &usage));
        assert!(assessment.token_log_error.is_none());
        let token_file = fs::read_dir(journal.join("tokens"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let line: Value =
            serde_json::from_str(fs::read_to_string(token_file).unwrap().trim()).unwrap();
        assert_eq!(line["usage"]["reasoning_tokens"], 5);
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn blank_stop_is_provider_response_invalid_but_blank_max_tokens_is_generated() {
        let journal = temp_journal();
        let usage = json!({});
        assert_eq!(
            assess_provider_result(view(&journal, "  ", "stop", &usage)).failure,
            Some(ValidationFailure::ProviderResponseInvalid {
                raw_response_snippet: None,
            })
        );
        assert_eq!(
            assess_provider_result(view(&journal, "  ", "max_tokens", &usage)).failure,
            None
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
    fn tool_calls_finish_reason_does_not_turn_blank_output_into_provider_invalid() {
        let journal = std::env::temp_dir().join("solstone-tool-calls-finish-reason");
        let assessment = assess_provider_result(ProviderResultView {
            journal_path: &journal,
            context: "test.generate",
            model: "model",
            text: "",
            finish_reason: "tool_calls",
            usage: &Value::Object(Map::new()),
            json_output: false,
            enforce_responsiveness: false,
            raw_response_snippet: None,
        });
        assert_eq!(
            sanitize_finish_reason("tool_calls"),
            SanitizedFinishReason::ToolCalls
        );
        assert_eq!(assessment.failure, None);
    }

    #[test]
    fn cap_is_utf8_safe_and_exactly_512_characters() {
        let journal = temp_journal();
        let usage = json!({});
        let text = format!("I cannot {}", "界".repeat(600));
        let assessment = assess_provider_result(view(&journal, &text, "stop", &usage));
        assert_eq!(
            assessment.failure,
            Some(ValidationFailure::NonResponsiveOutput)
        );
        let token_file = fs::read_dir(journal.join("tokens"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let line: Value =
            serde_json::from_str(fs::read_to_string(token_file).unwrap().trim()).unwrap();
        assert_eq!(
            line["non_responsive_output"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            512
        );
        let _ = fs::remove_dir_all(journal);
    }
}
