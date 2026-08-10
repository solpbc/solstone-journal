// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! OpenAI-compatible chat-completions request, response, and context fitting.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use crate::{Usage, estimate_tokens, prepare_local_schema};

#[derive(Debug, Clone, PartialEq)]
pub struct LocalConverseRequest<'a> {
    pub model: &'a str,
    pub system_instruction: Option<&'a str>,
    /// JSON array of chat-completions role-list message objects.
    pub messages: &'a Value,
    /// JSON array of chat-completions tool specifications.
    pub tools: &'a Value,
    pub temperature: f64,
    pub max_tokens: u32,
    pub json_output: bool,
    pub json_schema: Option<&'a Value>,
    pub include_qwen_sampling_controls: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalConverseToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalConverseResponse {
    pub text: String,
    pub tool_calls: Vec<LocalConverseToolCall>,
    pub finish_reason: String,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalConverseError {
    ContextBudgetExceeded,
    ResponseInvalid,
    ToolCallsMissing,
    ToolCallArgumentsInvalid,
}

/// Build an OpenAI-compatible chat-completions request body.
pub fn build_converse_request_body(
    request: &LocalConverseRequest<'_>,
    input_budget_tokens: Option<u32>,
) -> Result<Value, LocalConverseError> {
    let messages = match input_budget_tokens {
        Some(budget) => fit_converse_messages(request, budget)?,
        None => request.messages.clone(),
    };
    Ok(assemble_converse_body(request, messages))
}

/// Fit a role-list conversation by evicting oldest complete conversation units.
pub fn fit_converse_messages(
    request: &LocalConverseRequest<'_>,
    input_budget_tokens: u32,
) -> Result<Value, LocalConverseError> {
    let units = atomic_units(
        request
            .messages
            .as_array()
            .expect("local converse messages are an array"),
    );
    let mut first_retained = 0;

    loop {
        let messages = Value::Array(units[first_retained..].iter().flatten().cloned().collect());
        let body = assemble_converse_body(request, messages.clone());
        let serialized = serde_json::to_string(&body).expect("JSON value serializes");
        if estimate_tokens(&serialized) <= input_budget_tokens {
            return Ok(messages);
        }
        if first_retained + 1 >= units.len() {
            return Err(LocalConverseError::ContextBudgetExceeded);
        }
        first_retained += 1;
    }
}

/// Parse a chat-completions response with optional function tool calls.
pub fn parse_converse_response(data: &Value) -> Result<LocalConverseResponse, LocalConverseError> {
    let choices = data
        .get("choices")
        .and_then(Value::as_array)
        .filter(|choices| !choices.is_empty())
        .ok_or(LocalConverseError::ResponseInvalid)?;
    let choice = choices[0]
        .as_object()
        .ok_or(LocalConverseError::ResponseInvalid)?;
    let message = choice
        .get("message")
        .and_then(Value::as_object)
        .ok_or(LocalConverseError::ResponseInvalid)?;
    let text = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let tool_calls = parse_tool_calls(message.get("tool_calls"))?;

    let finish_reason = if !tool_calls.is_empty() {
        "tool_calls".to_owned()
    } else {
        normalize_converse_finish_reason(choice.get("finish_reason"))?
    };
    Ok(LocalConverseResponse {
        text,
        tool_calls,
        finish_reason,
        usage: crate::generate::extract_usage(data),
    })
}

fn assemble_converse_body(request: &LocalConverseRequest<'_>, messages: Value) -> Value {
    let mut messages = messages
        .as_array()
        .expect("local converse messages are an array")
        .clone();
    if let Some(instruction) = request
        .system_instruction
        .filter(|instruction| !instruction.is_empty())
    {
        messages.insert(0, json!({"role": "system", "content": instruction}));
    }

    let mut body = Map::new();
    body.insert("model".into(), Value::String(request.model.into()));
    body.insert("messages".into(), Value::Array(messages));
    body.insert("temperature".into(), json!(request.temperature));
    body.insert("max_tokens".into(), json!(request.max_tokens));
    body.insert("stream".into(), Value::Bool(false));
    if matches!(request.tools, Value::Array(tools) if !tools.is_empty()) {
        body.insert("tools".into(), request.tools.clone());
    }
    if request.include_qwen_sampling_controls {
        body.insert(
            "chat_template_kwargs".into(),
            json!({"enable_thinking": false}),
        );
        body.insert("top_p".into(), json!(0.8));
        body.insert("top_k".into(), json!(20));
        body.insert("min_p".into(), json!(0.0));
        body.insert("presence_penalty".into(), json!(1.5));
    }
    if let Some(schema) = request.json_schema {
        body.insert(
            "response_format".into(),
            json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "local_schema",
                    "schema": prepare_local_schema(schema),
                    "strict": true,
                }
            }),
        );
    } else if request.json_output {
        body.insert("response_format".into(), json!({"type": "json_object"}));
    }
    Value::Object(body)
}

fn atomic_units(messages: &[Value]) -> Vec<Vec<Value>> {
    let mut units = Vec::new();
    let mut index = 0;
    while index < messages.len() {
        let message = &messages[index];
        let tool_call_ids = assistant_tool_call_ids(message);
        let mut unit = vec![message.clone()];
        index += 1;
        if !tool_call_ids.is_empty() {
            while let Some(next) = messages.get(index) {
                let Some(tool_call_id) = next
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .filter(|_| next.get("role").and_then(Value::as_str) == Some("tool"))
                else {
                    break;
                };
                if !tool_call_ids.contains(tool_call_id) {
                    break;
                }
                unit.push(next.clone());
                index += 1;
            }
        }
        units.push(unit);
    }
    units
}

fn assistant_tool_call_ids(message: &Value) -> BTreeSet<&str> {
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return BTreeSet::new();
    }
    message
        .get("tool_calls")
        .and_then(Value::as_array)
        .filter(|calls| !calls.is_empty())
        .into_iter()
        .flatten()
        .filter_map(|call| call.get("id").and_then(Value::as_str))
        .collect()
}

fn parse_tool_calls(
    value: Option<&Value>,
) -> Result<Vec<LocalConverseToolCall>, LocalConverseError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    let calls = value
        .as_array()
        .ok_or(LocalConverseError::ToolCallArgumentsInvalid)?;
    calls
        .iter()
        .map(|call| {
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or(LocalConverseError::ToolCallArgumentsInvalid)?;
            if call.get("type").and_then(Value::as_str) != Some("function") {
                return Err(LocalConverseError::ToolCallArgumentsInvalid);
            }
            let function = call
                .get("function")
                .and_then(Value::as_object)
                .ok_or(LocalConverseError::ToolCallArgumentsInvalid)?;
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or(LocalConverseError::ToolCallArgumentsInvalid)?;
            let arguments = function
                .get("arguments")
                .and_then(Value::as_str)
                .ok_or(LocalConverseError::ToolCallArgumentsInvalid)?;
            let arguments = serde_json::from_str::<Value>(arguments)
                .map_err(|_| LocalConverseError::ToolCallArgumentsInvalid)?;
            if !arguments.is_object() {
                return Err(LocalConverseError::ToolCallArgumentsInvalid);
            }
            Ok(LocalConverseToolCall {
                id: id.to_owned(),
                name: name.to_owned(),
                arguments,
            })
        })
        .collect()
}

fn normalize_converse_finish_reason(raw: Option<&Value>) -> Result<String, LocalConverseError> {
    let raw = raw
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(LocalConverseError::ResponseInvalid)?;
    match raw.to_ascii_lowercase().as_str() {
        "stop" => Ok("stop".into()),
        "length" | "max_tokens" => Ok("max_tokens".into()),
        "content_filter" => Ok("content_filter".into()),
        "tool_calls" => Err(LocalConverseError::ToolCallsMissing),
        _ => Err(LocalConverseError::ResponseInvalid),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn request<'a>(messages: &'a Value, tools: &'a Value) -> LocalConverseRequest<'a> {
        LocalConverseRequest {
            model: "served",
            system_instruction: None,
            messages,
            tools,
            temperature: 0.2,
            max_tokens: 128,
            json_output: false,
            json_schema: None,
            include_qwen_sampling_controls: false,
        }
    }

    fn tool_call(id: &str, name: &str, arguments: &str) -> Value {
        json!({
            "id": id,
            "type": "function",
            "function": {"name": name, "arguments": arguments},
        })
    }

    #[test]
    fn request_body_uses_chat_completions_tool_shapes() {
        let messages = json!([
            {"role": "user", "content": "ask"},
            {"role": "assistant", "content": "working", "tool_calls": [tool_call("call-1", "weather", "{\"city\":\"Denver\"}")]},
            {"role": "tool", "tool_call_id": "call-1", "content": "sunny"},
        ]);
        let tools = json!([{
            "type": "function",
            "function": {
                "name": "weather",
                "description": "weather",
                "parameters": {"type": "object"},
            }
        }]);
        let request = LocalConverseRequest {
            system_instruction: Some("system"),
            json_schema: Some(&json!({"type": "object"})),
            include_qwen_sampling_controls: true,
            ..request(&messages, &tools)
        };

        assert_eq!(
            build_converse_request_body(&request, None).expect("request body"),
            json!({
                "model": "served",
                "messages": [
                    {"role": "system", "content": "system"},
                    {"role": "user", "content": "ask"},
                    {"role": "assistant", "content": "working", "tool_calls": [tool_call("call-1", "weather", "{\"city\":\"Denver\"}")]},
                    {"role": "tool", "tool_call_id": "call-1", "content": "sunny"},
                ],
                "tools": tools,
                "temperature": 0.2,
                "max_tokens": 128,
                "stream": false,
                "chat_template_kwargs": {"enable_thinking": false},
                "top_p": 0.8,
                "top_k": 20,
                "min_p": 0.0,
                "presence_penalty": 1.5,
                "response_format": {
                    "type": "json_schema",
                    "json_schema": {"name": "local_schema", "schema": {"type": "object"}, "strict": true},
                },
            })
        );
    }

    #[test]
    fn request_body_without_budget_preserves_messages() {
        let messages = json!([{"role": "user", "content": "keep"}]);
        let tools = json!([]);
        let body = build_converse_request_body(&request(&messages, &tools), None).expect("body");
        assert_eq!(body["messages"], messages);
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn parser_handles_text_tool_and_mixed_turns() {
        let text = parse_converse_response(&json!({
            "choices": [{"message": {"content": "plain"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5},
        }))
        .expect("text response");
        assert_eq!(text.text, "plain");
        assert!(text.tool_calls.is_empty());
        assert_eq!(text.finish_reason, "stop");
        assert_eq!(text.usage.expect("usage").total_tokens, 5);

        for (content, expected_text) in [(Value::Null, ""), (json!("before"), "before")] {
            let turn = parse_converse_response(&json!({
                "choices": [{
                    "message": {"content": content, "tool_calls": [tool_call("call-1", "weather", "{\"city\":\"Denver\"}")]},
                    "finish_reason": "stop",
                }],
            }))
            .expect("tool response");
            assert_eq!(turn.text, expected_text);
            assert_eq!(turn.finish_reason, "tool_calls");
            assert_eq!(turn.tool_calls[0].id, "call-1");
            assert_eq!(turn.tool_calls[0].name, "weather");
            assert_eq!(turn.tool_calls[0].arguments, json!({"city": "Denver"}));
        }
    }

    #[test]
    fn parser_rejects_invalid_tool_turns() {
        assert_eq!(
            parse_converse_response(&json!({})),
            Err(LocalConverseError::ResponseInvalid)
        );
        for call in [
            tool_call("call", "weather", "not json"),
            tool_call("call", "weather", "[]"),
            json!({"type": "function", "function": {"name": "weather", "arguments": "{}"}}),
            json!({"id": "call", "type": "function", "function": {"arguments": "{}"}}),
        ] {
            assert_eq!(
                parse_converse_response(&json!({
                    "choices": [{"message": {"tool_calls": [call]}, "finish_reason": "tool_calls"}],
                })),
                Err(LocalConverseError::ToolCallArgumentsInvalid)
            );
        }
        assert_eq!(
            parse_converse_response(&json!({
                "choices": [{"message": {"tool_calls": []}, "finish_reason": "tool_calls"}],
            })),
            Err(LocalConverseError::ToolCallsMissing)
        );
    }

    #[test]
    fn fitter_keeps_messages_within_budget_and_evicts_oldest_units() {
        let messages = json!([
            {"role": "user", "content": "old ".repeat(100)},
            {"role": "user", "content": "latest"},
        ]);
        let tools = json!([]);
        let request = request(&messages, &tools);
        let full = estimate_body_tokens(&request, messages.clone());
        let latest = json!([{"role": "user", "content": "latest"}]);
        let latest_tokens = estimate_body_tokens(&request, latest.clone());
        let budget = (full + latest_tokens) / 2;

        assert_eq!(
            fit_converse_messages(&request, budget).expect("fitted messages"),
            latest
        );
        assert_eq!(
            fit_converse_messages(&request, full).expect("already fitting"),
            messages
        );
    }

    #[test]
    fn fitter_evicts_tool_call_and_results_together() {
        let messages = json!([
            {"role": "assistant", "content": "call", "tool_calls": [tool_call("call-1", "weather", "{\"city\":\"Denver\"}")]},
            {"role": "tool", "tool_call_id": "call-1", "content": "sunny ".repeat(80)},
            {"role": "user", "content": "latest"},
        ]);
        let tools = json!([]);
        let request = request(&messages, &tools);
        let latest = json!([{"role": "user", "content": "latest"}]);
        let full = estimate_body_tokens(&request, messages.clone());
        let latest_tokens = estimate_body_tokens(&request, latest.clone());

        assert_eq!(
            fit_converse_messages(&request, (full + latest_tokens) / 2).expect("fitted"),
            latest
        );
    }

    #[test]
    fn fitter_preserves_newest_unit_and_counts_full_body() {
        let messages = json!([{"role": "user", "content": "latest"}]);
        let tools = json!([{
            "type": "function",
            "function": {"name": "large", "description": "x".repeat(500), "parameters": {"type": "object"}},
        }]);
        let request = LocalConverseRequest {
            system_instruction: Some("system"),
            ..request(&messages, &tools)
        };
        let body_tokens = estimate_body_tokens(&request, messages.clone());

        assert_eq!(
            fit_converse_messages(&request, body_tokens - 1),
            Err(LocalConverseError::ContextBudgetExceeded)
        );
    }

    fn estimate_body_tokens(request: &LocalConverseRequest<'_>, messages: Value) -> u32 {
        let body = assemble_converse_body(request, messages);
        estimate_tokens(&serde_json::to_string(&body).expect("JSON value serializes"))
    }
}
