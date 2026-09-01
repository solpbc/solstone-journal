// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared vocabulary for provider-native tool conversations.

use serde_json::Value;

pub(crate) fn converse_failure_flags(reason_code: &str) -> (bool, bool) {
    match reason_code {
        "tool_calls_missing" | "tool_call_arguments_invalid" | "tool_call_synthesized_as_prose" => {
            (true, false)
        }
        known => {
            let entry = solstone_core_generate::contract()["reason_codes"]
                .as_array()
                .expect("generate contract reason codes are an array")
                .iter()
                .find(|entry| entry["code"].as_str() == Some(known))
                .unwrap_or_else(|| {
                    panic!(
                        "converse reason code {known:?} is not a recognized generate-contract code or converse-only code"
                    )
                });
            (
                entry["retryable"].as_bool().expect("retryable is boolean"),
                entry["blocking"].as_bool().expect("blocking is boolean"),
            )
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConverseMessage {
    User {
        text: String,
    },
    Assistant {
        text: String,
        tool_calls: Vec<ConverseToolCall>,
    },
    ToolResult {
        tool_call_id: String,
        tool_name: String,
        output: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConverseToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConverseToolCall {
    /// The provider identifier required when submitting the result of this call.
    pub id: String,
    pub name: String,
    pub arguments: Value,
    pub not_offered: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConverseTurn {
    pub text: String,
    pub tool_calls: Vec<ConverseToolCall>,
    pub finish_reason: String,
    pub usage: Value,
    pub model: String,
    pub thinking: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConverseFailure {
    pub reason_code: String,
    pub retryable: bool,
    pub blocking: bool,
    pub detail: Option<String>,
}

#[cfg(test)]
pub(crate) fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries = object.iter().collect::<Vec<_>>();
            entries.sort_by_key(|(left, _)| *left);
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key.clone(), canonical_json(value)))
                    .collect(),
            )
        }
        Value::Array(values) => Value::Array(values.iter().map(canonical_json).collect()),
        value => value.clone(),
    }
}
