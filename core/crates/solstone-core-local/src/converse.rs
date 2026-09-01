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
    ToolCallSynthesizedAsProse,
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
    if tool_calls.is_empty() && finish_reason == "stop" && text.contains(TOOL_CALL_OPEN) {
        // The model DID call the tool; the server just did not structure it.
        if let Some((recovered, remainder)) = recover_prose_tool_calls(&text) {
            return Ok(LocalConverseResponse {
                text: remainder,
                tool_calls: recovered,
                finish_reason: "tool_calls".to_owned(),
                usage: crate::generate::extract_usage(data),
            });
        }
        // ⚠ Structural facts only -- never the content itself, which in real use is
        // owner text. This exists because a bare `tool_call_synthesized_as_prose`
        // says nothing about WHY recovery declined, and the difference between
        // "unterminated", "not JSON" and "missing name" decides the fix.
        if let Err(why) = recover_prose_tool_calls_detailed(&text) {
            eprintln!("tool-call prose recovery declined: {why}");
        }
        return Err(LocalConverseError::ToolCallSynthesizedAsProse);
    }
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

const TOOL_CALL_OPEN: &str = "<tool_call>";
const TOOL_CALL_CLOSE: &str = "</tool_call>";

/// Recover Qwen-style `<tool_call>` blocks that a server left inside message content.
///
/// Qwen emits tool calls as `<tool_call>{"name":..,"arguments":{..}}</tool_call>` in
/// the text, and an OpenAI-compatible server is expected to lift them into the
/// structured `tool_calls` field. Not every server does: SGLang and vLLM need an
/// explicit tool-call-parser flag, and a BYO endpoint may have none at all. When that
/// happens the model genuinely called the tool and the server simply did not parse it,
/// so refusing the whole turn throws away a correct answer.
///
/// Measured on the founder's journal 2026-09-01: every `cogitate` probe against the
/// SPP lane failed `tool_call_synthesized_as_prose`, which is raised only when the
/// content contains `<tool_call>` and `tool_calls` is empty -- i.e. the model answered
/// and the server passed the markup through verbatim. Thinking was down for that reason
/// alone.
///
/// 🔒 Deliberately conservative. Every block must close, parse as JSON, carry a
/// non-empty `name`, and carry `arguments` as an object (or a JSON string encoding
/// one). If ANY block fails these, this returns `None` and the caller keeps its
/// original refusal -- guessing at a malformed tool call is worse than failing.
fn recover_prose_tool_calls(text: &str) -> Option<(Vec<LocalConverseToolCall>, String)> {
    recover_prose_tool_calls_detailed(text).ok()
}

/// Same as [`recover_prose_tool_calls`], but names the check that declined so the
/// caller can say WHY. Structural only -- key names, never values.
fn recover_prose_tool_calls_detailed(
    text: &str,
) -> Result<(Vec<LocalConverseToolCall>, String), String> {
    let mut calls = Vec::new();
    let mut remainder = String::new();
    let mut rest = text;
    while let Some(start) = rest.find(TOOL_CALL_OPEN) {
        remainder.push_str(&rest[..start]);
        let after = &rest[start + TOOL_CALL_OPEN.len()..];
        let end = after.find(TOOL_CALL_CLOSE).ok_or("unterminated")?;
        let raw = after[..end].trim();
        let payload: Value = serde_json::from_str(raw).map_err(|_| {
            // ⚠ Shape, not content: every alphanumeric becomes `x`, so punctuation and
            // structure survive and any owner text does not. That is enough to tell a
            // single-quoted Python dict from a `<function=..>` wrapper from truncation.
            let shape: String = raw
                .chars()
                .map(|c| if c.is_alphanumeric() { 'x' } else { c })
                .collect();
            format!("payload is not JSON; shape={shape}")
        })?;
        let object = payload.as_object().ok_or("payload is not an object")?;
        let keys: Vec<&str> = object.keys().map(String::as_str).collect();
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| format!("no usable name; keys={keys:?}"))?;
        let arguments = match object.get("arguments") {
            Some(Value::Object(arguments)) => Value::Object(arguments.clone()),
            // Some servers double-encode the arguments exactly as the structured
            // field does; accept that spelling and nothing looser.
            Some(Value::String(encoded)) => {
                let parsed: Value =
                    serde_json::from_str(encoded).map_err(|_| "arguments string is not JSON")?;
                if !parsed.is_object() {
                    return Err("arguments string is not an object".to_owned());
                }
                parsed
            }
            _ => return Err(format!("no usable arguments; keys={keys:?}")),
        };
        calls.push(LocalConverseToolCall {
            id: format!("recovered-{}", calls.len()),
            name: name.to_owned(),
            arguments,
        });
        rest = &after[end + TOOL_CALL_CLOSE.len()..];
    }
    if calls.is_empty() {
        return Err("no blocks".to_owned());
    }
    remainder.push_str(rest);
    Ok((calls, remainder.trim().to_owned()))
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

    /// A tool call the server left as prose is still a tool call.
    ///
    /// Measured on the founder's journal: every `cogitate` probe on the SPP lane
    /// failed `tool_call_synthesized_as_prose`, which fires only when the content
    /// holds `<tool_call>` and `tool_calls` is empty -- the model answered and the
    /// server passed the markup through. Thinking was down for that alone.
    #[test]
    fn a_tool_call_left_in_prose_is_recovered() {
        let parsed = parse_converse_response(&json!({
            "choices": [{
                "message": {"content": "<tool_call>\n{\"name\": \"emit_final\", \"arguments\": {\"content\": \"OK\"}}\n</tool_call>"},
                "finish_reason": "stop"
            }]
        }))
        .expect("a well-formed prose tool call is recoverable");
        assert_eq!(parsed.finish_reason, "tool_calls");
        assert_eq!(parsed.tool_calls.len(), 1);
        assert_eq!(parsed.tool_calls[0].name, "emit_final");
        assert_eq!(parsed.tool_calls[0].arguments, json!({"content": "OK"}));
        assert!(
            !parsed.tool_calls[0].id.is_empty(),
            "a tool call needs an id"
        );
        assert_eq!(parsed.text, "", "the markup must not be left in the text");
    }

    #[test]
    fn recovered_prose_tool_calls_keep_surrounding_text_and_double_encoded_arguments() {
        let parsed = parse_converse_response(&json!({
            "choices": [{
                "message": {"content": "thinking<tool_call>{\"name\": \"t\", \"arguments\": \"{\\\"a\\\": 1}\"}</tool_call>done"},
                "finish_reason": "stop"
            }]
        }))
        .expect("double-encoded arguments match the structured spelling");
        assert_eq!(parsed.tool_calls[0].arguments, json!({"a": 1}));
        assert_eq!(parsed.text, "thinkingdone");
    }

    /// 🔒 Negative twins. Recovery must stay conservative: a malformed block keeps the
    /// original refusal, because guessing at a tool call is worse than failing.
    #[test]
    fn malformed_prose_tool_calls_still_refuse() {
        for content in [
            // unterminated block
            "<tool_call>{\"name\": \"t\", \"arguments\": {}}",
            // not JSON
            "<tool_call>not json</tool_call>",
            // missing name
            "<tool_call>{\"arguments\": {}}</tool_call>",
            // empty name
            "<tool_call>{\"name\": \"\", \"arguments\": {}}</tool_call>",
            // arguments not an object
            "<tool_call>{\"name\": \"t\", \"arguments\": 5}</tool_call>",
            // double-encoded arguments that do not decode to an object
            "<tool_call>{\"name\": \"t\", \"arguments\": \"5\"}</tool_call>",
            // one good block and one bad one: recover nothing
            "<tool_call>{\"name\": \"t\", \"arguments\": {}}</tool_call><tool_call>bad</tool_call>",
        ] {
            assert_eq!(
                parse_converse_response(&json!({
                    "choices": [{"message": {"content": content}, "finish_reason": "stop"}]
                })),
                Err(LocalConverseError::ToolCallSynthesizedAsProse),
                "must refuse: {content}"
            );
        }
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
    fn parser_rejects_synthesized_tool_call_prose_only_on_stop() {
        let marker = "<tool_call>{\"name\":\"weather\"}</tool_call>";
        assert_eq!(
            parse_converse_response(&json!({
                "choices": [{"message": {"content": marker}, "finish_reason": "stop"}],
            })),
            Err(LocalConverseError::ToolCallSynthesizedAsProse)
        );
        for finish_reason in ["max_tokens", "content_filter"] {
            let response = parse_converse_response(&json!({
                "choices": [{"message": {"content": marker}, "finish_reason": finish_reason}],
            }))
            .expect("non-stop marker is ordinary text");
            assert_eq!(response.finish_reason, finish_reason);
        }
        let response = parse_converse_response(&json!({
            "choices": [{"message": {"content": "ordinary text"}, "finish_reason": "stop"}],
        }))
        .expect("ordinary stop text");
        assert_eq!(response.finish_reason, "stop");
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
