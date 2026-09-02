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
    let usage = crate::generate::extract_usage(data);
    // The model called a tool; the server left the markup in content.
    if tool_calls.is_empty() && finish_reason == "stop" && text.contains(TOOL_CALL_OPEN) {
        return match recover_prose_tool_calls(&text) {
            // `remainder` is the prose surrounding the block. Dropping it loses any
            // answer the model wrote alongside the call, so it is preserved.
            Ok((calls, remainder)) => Ok(LocalConverseResponse {
                text: remainder,
                tool_calls: calls,
                finish_reason: "tool_calls".to_owned(),
                usage,
            }),
            // Structural facts only -- never content, which in real use is owner text.
            // A bare `tool_call_synthesized_as_prose` says nothing about WHY recovery
            // declined, and "unterminated" vs "not JSON" vs "missing name" decides the fix.
            Err(why) => {
                log::debug!("tool-call prose recovery declined: {why}");
                Err(LocalConverseError::ToolCallSynthesizedAsProse)
            }
        };
    }
    Ok(LocalConverseResponse {
        text,
        tool_calls,
        finish_reason,
        usage,
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

/// Alphanumeric → `x` so structure survives and owner text does not.
fn mask_payload_shape(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_alphanumeric() { 'x' } else { c })
        .collect()
}

/// Qwen's `<function=` / `<parameter=` spelling. Strict: stray content, empty
/// names, unterminated tags, and duplicate parameters are refused. Non-string
/// JSON parameter values keep their type; everything else stays the raw trimmed
/// string. A zero-parameter call yields `{}`.
fn parse_xml_tool_call(raw: &str) -> Result<(String, Value), &'static str> {
    let rest = raw
        .trim()
        .strip_prefix("<function=")
        .ok_or("not a function block")?;
    let (name, rest) = rest.split_once('>').ok_or("unterminated function tag")?;
    let name = name.trim();
    if name.is_empty() {
        return Err("empty function name");
    }
    let body = rest
        .trim()
        .strip_suffix("</function>")
        .ok_or("missing closing function tag")?;
    let mut arguments = Map::new();
    let mut cursor = body.trim();
    while !cursor.is_empty() {
        let after_open = cursor
            .strip_prefix("<parameter=")
            .ok_or("unexpected content inside a function block")?;
        let (key, after_key) = after_open
            .split_once('>')
            .ok_or("unterminated parameter tag")?;
        let key = key.trim();
        if key.is_empty() {
            return Err("empty parameter name");
        }
        let (value, remainder) = after_key
            .split_once("</parameter>")
            .ok_or("missing closing parameter tag")?;
        let value = value.trim();
        let typed = match serde_json::from_str::<Value>(value) {
            Ok(Value::String(_)) | Err(_) => Value::String(value.to_owned()),
            Ok(other) => other,
        };
        if arguments.insert(key.to_owned(), typed).is_some() {
            return Err("duplicate parameter");
        }
        cursor = remainder.trim();
    }
    Ok((name.to_owned(), Value::Object(arguments)))
}

/// JSON `{name, arguments}` spelling. Missing `arguments` defaults to `{}`; a
/// present non-object (including a JSON string) fails closed.
fn parse_json_tool_call(raw: &str) -> Result<(String, Value), String> {
    let payload: Value = serde_json::from_str(raw)
        .map_err(|_| format!("payload is not JSON; shape={}", mask_payload_shape(raw)))?;
    let object = payload.as_object().ok_or("payload is not an object")?;
    let keys: Vec<String> = object.keys().map(|key| mask_payload_shape(key)).collect();
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| format!("no usable name; keys={keys:?}"))?;
    let arguments = match object.get("arguments") {
        Some(Value::Object(arguments)) => Value::Object(arguments.clone()),
        None => Value::Object(Map::new()),
        // Some servers double-encode the arguments exactly as the structured field
        // does. Accept that spelling and nothing looser -- it must still decode to
        // an object.
        Some(Value::String(encoded)) => {
            let parsed: Value =
                serde_json::from_str(encoded).map_err(|_| "arguments string is not JSON")?;
            if !parsed.is_object() {
                return Err("arguments string is not an object".to_owned());
            }
            parsed
        }
        Some(_) => return Err("arguments is not an object".to_owned()),
    };
    Ok((name.to_owned(), arguments))
}

/// XML spelling first (the native Qwen form); JSON only if that fails.
fn parse_prose_tool_call_payload(raw: &str) -> Result<(String, Value), String> {
    if let Ok((name, arguments)) = parse_xml_tool_call(raw) {
        return Ok((name, arguments));
    }
    parse_json_tool_call(raw)
}

/// Recover `<tool_call>` blocks a server left in message content.
/// All-or-nothing: one malformed block refuses the whole response rather than
/// guessing.
///
/// Returns the recovered calls together with the prose that surrounded them. The
/// model often answers *and* calls a tool in one turn; discarding that text loses
/// the answer.
fn recover_prose_tool_calls(text: &str) -> Result<(Vec<LocalConverseToolCall>, String), String> {
    let mut calls = Vec::new();
    let mut remainder = String::new();
    let mut rest = text;
    while let Some(start) = rest.find(TOOL_CALL_OPEN) {
        remainder.push_str(&rest[..start]);
        let after = &rest[start + TOOL_CALL_OPEN.len()..];
        let end = after
            .find(TOOL_CALL_CLOSE)
            .ok_or_else(|| "unterminated".to_owned())?;
        let raw = after[..end].trim();
        let (name, arguments) = parse_prose_tool_call_payload(raw)?;
        calls.push(LocalConverseToolCall {
            id: format!("recovered-{}", calls.len()),
            name,
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

    /// The exact bytes the founder's SPP lane returns.
    ///
    /// Captured verbatim 2026-09-01 via a shape dump: Qwen 3.5 writes an XML-ish
    /// block, not JSON, and SGLang without a tool-call parser passes it through.

    /// 🔒 Negative twins for the XML spelling -- strict, never guessed at.

    /// 🔒 Negative twins. Recovery must stay conservative: a malformed block keeps the
    /// original refusal, because guessing at a tool call is worse than failing.

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

    fn prose_response(
        content: &str,
        finish_reason: &str,
    ) -> Result<LocalConverseResponse, LocalConverseError> {
        parse_converse_response(&json!({
            "choices": [{"message": {"content": content}, "finish_reason": finish_reason}],
        }))
    }

    #[test]
    fn missing_arguments_key_recovers_as_empty_object() {
        let parsed = prose_response("<tool_call>{\"name\":\"weather\"}</tool_call>", "stop")
            .expect("missing arguments defaults to {}");
        assert_eq!(parsed.finish_reason, "tool_calls");
        assert_eq!(parsed.text, "");
        assert_eq!(parsed.tool_calls.len(), 1);
        assert_eq!(parsed.tool_calls[0].id, "recovered-0");
        assert_eq!(parsed.tool_calls[0].name, "weather");
        assert_eq!(parsed.tool_calls[0].arguments, json!({}));
    }

    #[test]
    fn parser_treats_tool_call_marker_as_ordinary_text_when_not_stop() {
        let marker = "<tool_call>{\"name\":\"weather\"}</tool_call>";
        for finish_reason in ["max_tokens", "content_filter"] {
            let response =
                prose_response(marker, finish_reason).expect("non-stop marker is ordinary text");
            assert_eq!(response.finish_reason, finish_reason);
            assert_eq!(response.text, marker);
            assert!(response.tool_calls.is_empty());
        }
        let response = prose_response("ordinary text", "stop").expect("ordinary stop text");
        assert_eq!(response.finish_reason, "stop");
        assert_eq!(response.text, "ordinary text");
        assert!(response.tool_calls.is_empty());
    }

    #[test]
    fn a_json_tool_call_left_in_prose_is_recovered() {
        let parsed = prose_response(
            "<tool_call>\n{\"name\": \"emit_final\", \"arguments\": {\"content\": \"OK\"}}\n</tool_call>",
            "stop",
        )
        .expect("well-formed JSON prose is recoverable");
        assert_eq!(parsed.finish_reason, "tool_calls");
        assert_eq!(parsed.text, "");
        assert_eq!(parsed.tool_calls.len(), 1);
        assert_eq!(parsed.tool_calls[0].id, "recovered-0");
        assert_eq!(parsed.tool_calls[0].name, "emit_final");
        assert_eq!(parsed.tool_calls[0].arguments, json!({"content": "OK"}));
    }

    #[test]
    fn a_qwen_xml_tool_call_left_in_prose_is_recovered() {
        let parsed = prose_response(
            "<tool_call>\n<function=emit_final>\n<parameter=content>\nOK\n</parameter>\n</function>\n</tool_call>",
            "stop",
        )
        .expect("Qwen XML prose is recoverable");
        assert_eq!(parsed.finish_reason, "tool_calls");
        assert_eq!(parsed.text, "");
        assert_eq!(parsed.tool_calls.len(), 1);
        assert_eq!(parsed.tool_calls[0].id, "recovered-0");
        assert_eq!(parsed.tool_calls[0].name, "emit_final");
        assert_eq!(parsed.tool_calls[0].arguments, json!({"content": "OK"}));
    }

    #[test]
    fn non_object_arguments_still_refuse() {
        for content in [
            "<tool_call>{\"name\": \"t\", \"arguments\": 5}</tool_call>",
            "<tool_call>{\"name\": \"t\", \"arguments\": []}</tool_call>",
            "<tool_call>{\"name\": \"t\", \"arguments\": true}</tool_call>",
            "<tool_call>{\"name\": \"t\", \"arguments\": null}</tool_call>",
            // A string that is not JSON, and one that decodes to a non-object, both refuse.
            "<tool_call>{\"name\": \"t\", \"arguments\": \"not json\"}</tool_call>",
            "<tool_call>{\"name\": \"t\", \"arguments\": \"[1,2]\"}</tool_call>",
        ] {
            assert_eq!(
                prose_response(content, "stop"),
                Err(LocalConverseError::ToolCallSynthesizedAsProse),
                "must refuse: {content}"
            );
        }
        // ...but a string that decodes to an object is a spelling real servers emit
        // (the structured `tool_calls` field double-encodes the same way), so it is
        // accepted. Refusing it cost a live conversation lane its tool calls.
        let parsed = prose_response(
            "<tool_call>{\"name\": \"t\", \"arguments\": \"{\\\"a\\\":1}\"}</tool_call>",
            "stop",
        )
        .expect("double-encoded object arguments are accepted");
        assert_eq!(parsed.tool_calls[0].arguments, json!({"a": 1}));
    }

    #[test]
    fn recovered_prose_tool_calls_keep_the_surrounding_text() {
        let parsed = prose_response(
            "thinking<tool_call>{\"name\":\"t\",\"arguments\":{}}</tool_call>done",
            "stop",
        )
        .expect("surrounding prose is preserved");
        // The model often answers *and* calls a tool in one turn. Discarding the
        // prose loses the answer, so the remainder is returned as the text.
        assert_eq!(parsed.text, "thinkingdone");
        assert_eq!(parsed.tool_calls[0].name, "t");
        assert_eq!(parsed.tool_calls[0].arguments, json!({}));
    }

    #[test]
    fn malformed_block_refuses_the_whole_response() {
        assert_eq!(
            prose_response(
                "<tool_call>{\"name\": \"t\", \"arguments\": {}}</tool_call><tool_call>bad</tool_call>",
                "stop",
            ),
            Err(LocalConverseError::ToolCallSynthesizedAsProse)
        );
    }

    #[test]
    fn multiple_recovered_calls_share_one_id_counter() {
        let parsed = prose_response(
            "<tool_call>\n<function=emit_final>\n<parameter=content>\nOK\n</parameter>\n</function>\n</tool_call><tool_call>{\"name\": \"t\", \"arguments\": {}}</tool_call>",
            "stop",
        )
        .expect("mixed XML then JSON recover together");
        assert_eq!(parsed.tool_calls.len(), 2);
        assert_eq!(parsed.tool_calls[0].id, "recovered-0");
        assert_eq!(parsed.tool_calls[0].name, "emit_final");
        assert_eq!(parsed.tool_calls[1].id, "recovered-1");
        assert_eq!(parsed.tool_calls[1].name, "t");
        assert_eq!(parsed.text, "");
    }

    #[test]
    fn mask_payload_shape_replaces_alphanumerics_only() {
        assert_eq!(mask_payload_shape("abc123"), "xxxxxx");
        assert_eq!(
            mask_payload_shape("<function=emit_final>"),
            "<xxxxxxxx=xxxx_xxxxx>"
        );
    }

    #[test]
    fn malformed_prose_tool_calls_still_refuse() {
        for content in [
            "<tool_call>{\"name\": \"t\", \"arguments\": {}}",
            "<tool_call>not json</tool_call>",
            "<tool_call>{\"arguments\": {}}</tool_call>",
            "<tool_call>{\"name\": \"\", \"arguments\": {}}</tool_call>",
            "<tool_call></tool_call>",
        ] {
            assert_eq!(
                prose_response(content, "stop"),
                Err(LocalConverseError::ToolCallSynthesizedAsProse),
                "must refuse: {content}"
            );
        }
    }

    #[test]
    fn xml_tool_calls_carry_types_and_allow_no_parameters() {
        assert_eq!(
            parse_xml_tool_call(
                "<function=go>\n<parameter=count>\n3\n</parameter>\n<parameter=on>\ntrue\n</parameter>\n<parameter=note>\nhello there\n</parameter>\n</function>"
            ),
            Ok((
                "go".to_owned(),
                json!({"count": 3, "on": true, "note": "hello there"})
            ))
        );
        assert_eq!(
            parse_xml_tool_call("<function=ping>\n</function>"),
            Ok(("ping".to_owned(), json!({})))
        );
    }

    #[test]
    fn malformed_xml_tool_calls_are_refused() {
        for raw in [
            "<function=>\n</function>",
            "<function=go\n</function>",
            "<function=go>\n<parameter=a>\nx\n</parameter>",
            "<function=go>\nstray\n</function>",
            "<function=go>\n<parameter=a\nx\n</parameter>\n</function>",
            "<function=go>\n<parameter=a>\nx\n</function>",
            "<function=go>\n<parameter=a>\n1\n</parameter>\n<parameter=a>\n2\n</parameter>\n</function>",
        ] {
            assert!(parse_xml_tool_call(raw).is_err(), "must refuse: {raw}");
        }
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
