// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! OpenAI Responses API generation.

use std::time::Duration;
use std::{collections::BTreeSet, sync::LazyLock};

use regex::Regex;
use serde_json::{Map, Value, json};
use solstone_core_generate::{ContentPart, GenerateRequest};
use solstone_core_local::HttpResponse;

use crate::endpoint::EndpointTransportError;
use crate::schema_prep::prepare_provider_schema;
use crate::token_budget::generate_token_budget;
use crate::{
    ConverseFailure, ConverseMessage, ConverseToolCall, ConverseToolSpec, ConverseTurn,
    NON_RESPONSIVE_RAW_OUTPUT_CAP_CHARS,
};

const OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";
const OPENAI_BASE_URL: &str = "https://api.openai.com";
const OPENAI_RESPONSES_PATH: &str = "/v1/responses";
const DEFAULT_MODEL: &str = "gpt-5.4-mini";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const OPENAI_EFFORT_SUFFIXES: &[&str] = &["-none", "-low", "-medium", "-high", "-xhigh"];
const CONTEXT_WINDOW_PATTERNS: &[&str] = &[
    "prompt is too long",
    "maximum context length",
    "context window",
    "context length",
    "too many tokens",
    "exceeds the available context size",
];

static SCHEMA_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9_-]{1,64}$").expect("valid OpenAI schema-name regex"));

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiFailure {
    pub reason_code: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenAiGenerated {
    pub text: String,
    pub model: String,
    pub usage: Value,
    pub finish_reason: String,
    pub thinking: Option<Value>,
    pub raw_response_snippet: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OpenAiResult {
    Generated(OpenAiGenerated),
    Failed(OpenAiFailure),
}

pub type OpenAiTurn = ConverseTurn;
pub type OpenAiConverseFailure = ConverseFailure;

#[derive(Debug, Clone, PartialEq)]
pub enum OpenAiConverseResult {
    Turn(Box<OpenAiTurn>),
    Failed(OpenAiConverseFailure),
}

pub trait OpenAiTransport {
    fn post_json(
        &mut self,
        base_url: &str,
        path: &str,
        body: &Value,
        api_key: &str,
        timeout: Duration,
    ) -> Result<HttpResponse, EndpointTransportError>;
}

#[derive(Default)]
pub struct UreqOpenAiTransport;

impl OpenAiTransport for UreqOpenAiTransport {
    fn post_json(
        &mut self,
        base_url: &str,
        path: &str,
        body: &Value,
        api_key: &str,
        timeout: Duration,
    ) -> Result<HttpResponse, EndpointTransportError> {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(timeout))
            .timeout_recv_response(Some(timeout))
            .timeout_recv_body(Some(timeout))
            .timeout_global(Some(timeout))
            .build();
        let agent = ureq::Agent::new_with_config(config);
        let response = agent
            .post(&format!("{base_url}{path}"))
            .header("Content-Type", "application/json")
            .header("Authorization", &format!("Bearer {api_key}"))
            .send(serde_json::to_string(body).expect("JSON value serializes"))
            .map_err(classify_ureq_error)?;
        let status = response.status().as_u16();
        let body = response
            .into_body()
            .read_to_string()
            .map_err(classify_ureq_error)?;
        Ok(HttpResponse { status, body })
    }
}

pub fn openai_generate(request: &GenerateRequest, config: &Map<String, Value>) -> OpenAiResult {
    let mut transport = UreqOpenAiTransport;
    openai_generate_with(request, config, &mut transport)
}

pub fn openai_converse(
    request: &GenerateRequest,
    messages: &[ConverseMessage],
    tools: &[ConverseToolSpec],
    config: &Map<String, Value>,
) -> OpenAiConverseResult {
    let mut transport = UreqOpenAiTransport;
    openai_converse_with(request, messages, tools, config, &mut transport)
}

fn openai_converse_with<T: OpenAiTransport>(
    request: &GenerateRequest,
    messages: &[ConverseMessage],
    tools: &[ConverseToolSpec],
    config: &Map<String, Value>,
    transport: &mut T,
) -> OpenAiConverseResult {
    let Some(api_key) = configured_api_key(config) else {
        return converse_failure("provider_key_missing");
    };
    let model = configured_model(config);
    let base_url = crate::overrides::configured_base_url(config, OPENAI_BASE_URL);
    let body = converse_request_body(request, messages, tools, &model);
    let response = match transport.post_json(
        &base_url,
        OPENAI_RESPONSES_PATH,
        &body,
        &api_key,
        request_timeout(request.timeout_s),
    ) {
        Ok(response) => response,
        Err(EndpointTransportError::Connection) => return converse_failure("network_unreachable"),
        Err(EndpointTransportError::Capacity) => return converse_failure("provider_unavailable"),
        Err(EndpointTransportError::Other) => return converse_failure("provider_response_invalid"),
    };
    if !(200..300).contains(&response.status) {
        let reason_code = classify_http_failure(response.status, &response.body);
        let (retryable, blocking) = crate::converse::converse_failure_flags(reason_code);
        let detail = capture_provider_detail(&response.body, &api_key);
        return OpenAiConverseResult::Failed(ConverseFailure {
            reason_code: reason_code.to_owned(),
            retryable,
            blocking,
            detail,
        });
    }
    let offered = tools.iter().map(|tool| tool.name.clone()).collect();
    parse_converse_response(&response.body, &offered)
}

fn openai_generate_with<T: OpenAiTransport>(
    request: &GenerateRequest,
    config: &Map<String, Value>,
    transport: &mut T,
) -> OpenAiResult {
    openai_generate_with_lookup(
        request,
        config,
        transport,
        crate::overrides::non_blank_process_env,
    )
}

fn openai_generate_with_lookup<T: OpenAiTransport>(
    request: &GenerateRequest,
    config: &Map<String, Value>,
    transport: &mut T,
    env: impl Fn(&str) -> Option<String>,
) -> OpenAiResult {
    let env = &env;
    let Some(api_key) = crate::overrides::configured_api_key_with(config, OPENAI_API_KEY_ENV, env)
    else {
        return failure("provider_key_missing");
    };
    let model = crate::overrides::configured_model_with(config, DEFAULT_MODEL, env);
    let base_url = crate::overrides::configured_base_url_with(config, OPENAI_BASE_URL, env);
    let body = request_body(request, &model);
    let response = match transport.post_json(
        &base_url,
        OPENAI_RESPONSES_PATH,
        &body,
        &api_key,
        request_timeout(request.timeout_s),
    ) {
        Ok(response) => response,
        Err(EndpointTransportError::Connection) => return failure("network_unreachable"),
        Err(EndpointTransportError::Capacity) => return failure("provider_unavailable"),
        Err(EndpointTransportError::Other) => return failure("provider_response_invalid"),
    };
    if !(200..300).contains(&response.status) {
        let reason_code = classify_http_failure(response.status, &response.body);
        let detail = capture_provider_detail(&response.body, &api_key);
        return OpenAiResult::Failed(OpenAiFailure {
            reason_code: Some(reason_code.to_owned()),
            detail,
        });
    }
    parse_response(&response.body, &api_key)
}

fn configured_api_key(config: &Map<String, Value>) -> Option<String> {
    crate::overrides::configured_api_key(config, OPENAI_API_KEY_ENV)
}

fn configured_model(config: &Map<String, Value>) -> String {
    crate::overrides::configured_model(config, DEFAULT_MODEL)
}

fn request_body(request: &GenerateRequest, model: &str) -> Value {
    let content = request
        .contents
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } => json!({"type": "input_text", "text": text}),
            ContentPart::Image { mime_type, data } => {
                json!({"type": "input_image", "image_url": format!("data:{mime_type};base64,{data}")})
            }
        })
        .collect::<Vec<_>>();
    let mut input = Vec::new();
    if let Some(system) = &request.system_instruction {
        input.push(json!({
            "role": "system",
            "content": [{"type": "input_text", "text": system}],
        }));
    }
    input.push(json!({"role": "user", "content": content}));
    let mut body = json!({
        "model": strip_effort_suffix(model),
        "max_output_tokens": generate_token_budget(
            "openai",
            request.max_output_tokens,
            request.thinking_budget,
        ),
        "input": input,
    });
    if let Some(schema) = prepare_provider_schema(request.json_schema.as_ref(), "openai") {
        body["text"] = json!({
            "format": {
                "type": "json_schema",
                "name": schema_name(request.json_schema.as_ref()),
                "schema": schema,
                "strict": true,
            }
        });
    } else if request.json_output {
        body["text"] = json!({"format": {"type": "json_object"}});
    }
    body
}

fn converse_request_body(
    request: &GenerateRequest,
    messages: &[ConverseMessage],
    tools: &[ConverseToolSpec],
    model: &str,
) -> Value {
    let mut input = Vec::new();
    if let Some(system) = &request.system_instruction {
        input.push(json!({
            "role": "system",
            "content": [{"type": "input_text", "text": system}],
        }));
    }
    for message in messages {
        match message {
            ConverseMessage::User { text } => input.push(json!({
                "role": "user",
                "content": [{"type": "input_text", "text": text}],
            })),
            ConverseMessage::Assistant { text, tool_calls } => {
                if !text.is_empty() {
                    input.push(json!({
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": text}],
                    }));
                }
                input.extend(tool_calls.iter().map(|call| {
                    json!({
                        "type": "function_call",
                        "call_id": call.id,
                        "name": call.name,
                        "arguments": call.arguments.to_string(),
                    })
                }));
            }
            ConverseMessage::ToolResult {
                tool_call_id,
                tool_name: _,
                output,
            } => input.push(json!({
                "type": "function_call_output",
                "call_id": tool_call_id,
                "output": output,
            })),
        }
    }
    json!({
        "model": strip_effort_suffix(model),
        "max_output_tokens": generate_token_budget(
            "openai",
            request.max_output_tokens,
            request.thinking_budget,
        ),
        "input": input,
        "tools": tools.iter().map(|tool| json!({
            "type": "function",
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.parameters,
        })).collect::<Vec<_>>(),
    })
}

fn strip_effort_suffix(model: &str) -> &str {
    OPENAI_EFFORT_SUFFIXES
        .iter()
        .find_map(|suffix| model.strip_suffix(suffix))
        .unwrap_or(model)
}

fn schema_name(schema: Option<&Value>) -> &str {
    schema
        .and_then(Value::as_object)
        .and_then(|schema| schema.get("title"))
        .and_then(Value::as_str)
        .filter(|title| SCHEMA_NAME_RE.is_match(title))
        .unwrap_or("response")
}

fn request_timeout(timeout_s: Option<f64>) -> Duration {
    timeout_s
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .map(Duration::from_secs_f64)
        .unwrap_or(DEFAULT_TIMEOUT)
}

fn parse_response(body: &str, secret: &str) -> OpenAiResult {
    let raw_snippet = capture_provider_detail(body, secret);
    let Ok(body) = serde_json::from_str::<Value>(body) else {
        return failure("provider_response_invalid");
    };
    let Some(output) = body.get("output").and_then(Value::as_array) else {
        return failure("provider_response_invalid");
    };
    let Some(model) = body
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
    else {
        return failure("provider_response_invalid");
    };
    let usage = match response_usage(&body) {
        Ok(usage) => usage,
        Err(()) => return failure("provider_response_invalid"),
    };

    let mut text = String::new();
    for item in output {
        let Some(content) = item.get("content") else {
            continue;
        };
        let Some(content) = content.as_array() else {
            return failure("provider_response_invalid");
        };
        for block in content {
            if block.get("type").and_then(Value::as_str) == Some("output_text") {
                let Some(value) = block.get("text").and_then(Value::as_str) else {
                    return failure("provider_response_invalid");
                };
                text.push_str(value);
            }
        }
    }
    OpenAiResult::Generated(OpenAiGenerated {
        text,
        model: model.to_owned(),
        usage,
        finish_reason: normalize_finish_reason(&body),
        thinking: None,
        raw_response_snippet: raw_snippet,
    })
}

fn parse_converse_response(body: &str, offered: &BTreeSet<String>) -> OpenAiConverseResult {
    let Ok(body) = serde_json::from_str::<Value>(body) else {
        return converse_failure("provider_response_invalid");
    };
    let Some(output) = body.get("output").and_then(Value::as_array) else {
        return converse_failure("provider_response_invalid");
    };
    let Some(model) = body
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
    else {
        return converse_failure("provider_response_invalid");
    };
    let usage = match response_usage(&body) {
        Ok(usage) => usage,
        Err(()) => return converse_failure("provider_response_invalid"),
    };
    let mut text = String::new();
    let mut function_items = Vec::new();
    for item in output {
        if item.get("type").and_then(Value::as_str) == Some("function_call") {
            function_items.push(item);
            continue;
        }
        let Some(content) = item.get("content") else {
            continue;
        };
        let Some(content) = content.as_array() else {
            return converse_failure("provider_response_invalid");
        };
        for block in content {
            if block.get("type").and_then(Value::as_str) == Some("output_text") {
                let Some(value) = block.get("text").and_then(Value::as_str) else {
                    return converse_failure("provider_response_invalid");
                };
                text.push_str(value);
            }
        }
    }
    let finish_reason = normalize_finish_reason(&body);
    if body.get("status").and_then(Value::as_str).map(str::trim) == Some("incomplete") {
        return OpenAiConverseResult::Turn(Box::new(ConverseTurn {
            text,
            tool_calls: Vec::new(),
            finish_reason,
            usage,
            model: model.to_owned(),
            thinking: None,
        }));
    }
    let mut tool_calls = Vec::new();
    for item in function_items {
        let Some(id) = item
            .get("call_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            return converse_failure("tool_call_arguments_invalid");
        };
        let Some(name) = item
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
        else {
            return converse_failure("tool_call_arguments_invalid");
        };
        let Some(arguments) = item.get("arguments").and_then(Value::as_str) else {
            return converse_failure("tool_call_arguments_invalid");
        };
        let Ok(arguments) = serde_json::from_str::<Value>(arguments) else {
            return converse_failure("tool_call_arguments_invalid");
        };
        if !arguments.is_object() {
            return converse_failure("tool_call_arguments_invalid");
        }
        tool_calls.push(ConverseToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments,
            not_offered: !offered.contains(name),
        });
    }
    OpenAiConverseResult::Turn(Box::new(ConverseTurn {
        text,
        finish_reason: if !tool_calls.is_empty() {
            "tool_calls".to_owned()
        } else {
            finish_reason
        },
        tool_calls,
        usage,
        model: model.to_owned(),
        thinking: None,
    }))
}

fn response_usage(body: &Value) -> Result<Value, ()> {
    let Some(usage) = body.get("usage") else {
        return Ok(Value::Object(Map::new()));
    };
    let Some(usage) = usage.as_object() else {
        return Err(());
    };
    let mut normalized = Map::new();
    copy_usage_number(usage, "input_tokens", "input_tokens", &mut normalized)?;
    copy_usage_number(usage, "output_tokens", "output_tokens", &mut normalized)?;
    copy_usage_number(usage, "total_tokens", "total_tokens", &mut normalized)?;
    copy_nested_usage_number(
        usage,
        "input_tokens_details",
        "cached_tokens",
        "cached_tokens",
        &mut normalized,
    )?;
    copy_nested_usage_number(
        usage,
        "output_tokens_details",
        "reasoning_tokens",
        "reasoning_tokens",
        &mut normalized,
    )?;
    if normalized
        .values()
        .all(|value| value.as_u64().is_none_or(|value| value == 0))
    {
        return Ok(Value::Object(Map::new()));
    }
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .ok_or(())?;
    normalized.insert("model_version".into(), Value::String(model.to_owned()));
    Ok(Value::Object(normalized))
}

fn copy_usage_number(
    usage: &Map<String, Value>,
    source: &str,
    target: &str,
    normalized: &mut Map<String, Value>,
) -> Result<(), ()> {
    let Some(value) = usage.get(source) else {
        return Ok(());
    };
    let Some(value) = value.as_u64() else {
        return Err(());
    };
    normalized.insert(target.to_owned(), Value::from(value));
    Ok(())
}

fn copy_nested_usage_number(
    usage: &Map<String, Value>,
    details_name: &str,
    source: &str,
    target: &str,
    normalized: &mut Map<String, Value>,
) -> Result<(), ()> {
    let Some(details) = usage.get(details_name) else {
        return Ok(());
    };
    let Some(details) = details.as_object() else {
        return Err(());
    };
    copy_usage_number(details, source, target, normalized)
}

fn normalize_finish_reason(body: &Value) -> String {
    let raw = match body.get("status").and_then(Value::as_str).map(str::trim) {
        Some("completed") => "stop".to_owned(),
        Some("incomplete") => body
            .get("incomplete_details")
            .and_then(Value::as_object)
            .and_then(|details| details.get("reason"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .unwrap_or("max_output_tokens")
            .to_owned(),
        Some("failed") => "error".to_owned(),
        Some(status) if !status.is_empty() => status.to_owned(),
        // Read from the contract rather than held as a literal: the value
        // collides with the `unknown` refusal reason, and holding a copy is
        // what the vocabulary guard exists to prevent.
        _ => unknown_finish_reason().to_owned(),
    };
    match raw.trim().to_ascii_lowercase().as_str() {
        "end_turn" | "stop_sequence" => "stop".to_owned(),
        "length" | "max_output_tokens" => "max_tokens".to_owned(),
        normalized => normalized.to_owned(),
    }
}

fn classify_http_failure(status: u16, body: &str) -> &'static str {
    match status {
        401 => "provider_key_invalid",
        429 => "provider_quota_exceeded",
        400 if is_context_window_error(body) => "context_window_exceeded",
        400 => "provider_request_rejected",
        500..=599 => "provider_unavailable",
        _ => "provider_response_invalid",
    }
}

fn is_context_window_error(body: &str) -> bool {
    let Ok(body) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    body.get("error")
        .and_then(Value::as_object)
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .map(|message| {
            let message = message.to_ascii_lowercase();
            CONTEXT_WINDOW_PATTERNS
                .iter()
                .any(|pattern| message.contains(pattern))
        })
        .unwrap_or(false)
}

fn capture_provider_detail(body: &str, secret: &str) -> Option<String> {
    if body.is_empty() {
        return None;
    }
    let scrubbed = if secret.is_empty() {
        body.to_owned()
    } else {
        body.replace(secret, "[redacted]")
    };
    Some(
        scrubbed
            .chars()
            .take(NON_RESPONSIVE_RAW_OUTPUT_CAP_CHARS)
            .collect(),
    )
}

fn failure(reason_code: &str) -> OpenAiResult {
    OpenAiResult::Failed(OpenAiFailure {
        reason_code: Some(reason_code.to_owned()),
        detail: None,
    })
}

fn converse_failure(reason_code: &str) -> OpenAiConverseResult {
    let (retryable, blocking) = crate::converse::converse_failure_flags(reason_code);
    OpenAiConverseResult::Failed(ConverseFailure {
        reason_code: reason_code.to_owned(),
        retryable,
        blocking,
        detail: None,
    })
}

fn classify_ureq_error(error: ureq::Error) -> EndpointTransportError {
    match error {
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed | ureq::Error::Io(_) => {
            EndpointTransportError::Connection
        }
        ureq::Error::Timeout(ureq::Timeout::Resolve | ureq::Timeout::Connect) => {
            EndpointTransportError::Connection
        }
        ureq::Error::Timeout(_) => EndpointTransportError::Capacity,
        _ => EndpointTransportError::Other,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use solstone_core_generate::{ContentPart, ReasonCodeValue};

    use super::*;
    use crate::{
        LaneOutcome, ProviderResultView, ValidationFailure, assess_provider_result, refusal_for,
    };

    #[derive(Default)]
    struct StubTransport {
        responses: Vec<Result<HttpResponse, EndpointTransportError>>,
        posts: Vec<Value>,
        paths: Vec<String>,
        api_keys: Vec<String>,
    }

    impl OpenAiTransport for StubTransport {
        fn post_json(
            &mut self,
            _base_url: &str,
            path: &str,
            body: &Value,
            api_key: &str,
            _timeout: Duration,
        ) -> Result<HttpResponse, EndpointTransportError> {
            self.posts.push(body.clone());
            self.paths.push(path.to_owned());
            self.api_keys.push(api_key.to_owned());
            self.responses.remove(0)
        }
    }

    fn request() -> GenerateRequest {
        GenerateRequest {
            id: Some("request".into()),
            context: "context".into(),
            contents: vec![ContentPart::Text {
                text: "hello".into(),
            }],
            system_instruction: Some("system".into()),
            temperature: 0.3,
            max_output_tokens: 4_000,
            thinking_budget: None,
            timeout_s: None,
            json_output: false,
            json_schema: None,
            enforce_responsiveness: false,
            attempt_index: 0,
            exclusive_admission: false,
            transport_retries: None,
        }
    }

    fn config(key: Option<&str>, model: Option<&str>) -> Map<String, Value> {
        let mut env = Map::new();
        if let Some(key) = key {
            env.insert(OPENAI_API_KEY_ENV.into(), Value::String(key.into()));
        }
        let mut active = Map::new();
        if let Some(model) = model {
            active.insert("model".into(), Value::String(model.into()));
        }
        let mut providers = Map::new();
        providers.insert("active".into(), Value::Object(active));
        let mut config = Map::new();
        config.insert("env".into(), Value::Object(env));
        config.insert("providers".into(), Value::Object(providers));
        config
    }

    fn response(body: Value) -> HttpResponse {
        HttpResponse {
            status: 200,
            body: body.to_string(),
        }
    }

    fn successful_body() -> Value {
        json!({
            "model": "gpt-response-model",
            "status": "completed",
            "output": [{"content": [{"type": "output_text", "text": "done"}]}],
            "usage": {"input_tokens": 12, "output_tokens": 34, "total_tokens": 46},
        })
    }

    fn generated(result: OpenAiResult) -> OpenAiGenerated {
        match result {
            OpenAiResult::Generated(success) => success,
            OpenAiResult::Failed(failure) => panic!("unexpected failure: {failure:?}"),
        }
    }

    fn parsed(body: Value) -> OpenAiGenerated {
        generated(parse_response(&body.to_string(), ""))
    }

    fn temp_journal() -> std::path::PathBuf {
        crate::validation::isolated_journal_dir("openai")
    }

    #[test]
    fn completed_status_normalizes_to_stop() {
        assert_eq!(parsed(successful_body()).finish_reason, "stop");
    }

    #[test]
    fn incomplete_status_defaults_to_max_output_tokens() {
        for details in [json!({"reason": "max_output_tokens"}), json!({})] {
            let mut body = successful_body();
            body["status"] = json!("incomplete");
            body["incomplete_details"] = details;
            assert_eq!(parsed(body).finish_reason, "max_tokens");
        }
    }

    #[test]
    fn failed_status_normalizes_to_error() {
        let mut body = successful_body();
        body["status"] = json!("failed");
        assert_eq!(parsed(body).finish_reason, "error");
    }

    #[test]
    fn unknown_status_is_trimmed_lowercased_and_preserved() {
        let mut body = successful_body();
        body["status"] = json!("  Future_Status  ");
        assert_eq!(parsed(body).finish_reason, "future_status");
    }

    #[test]
    fn missing_or_blank_status_becomes_unknown() {
        let mut missing = successful_body();
        missing.as_object_mut().unwrap().remove("status");
        assert_eq!(parsed(missing).finish_reason, unknown_finish_reason());

        let mut blank = successful_body();
        blank["status"] = json!("  ");
        assert_eq!(parsed(blank).finish_reason, unknown_finish_reason());
    }

    #[test]
    fn finish_reason_ignores_absent_choices_key_and_uses_status() {
        let body = successful_body();
        assert!(body.get("choices").is_none());
        assert_eq!(parsed(body).finish_reason, "stop");
    }

    #[test]
    fn request_posts_to_literal_responses_path() {
        let mut transport = StubTransport {
            responses: vec![Ok(response(successful_body()))],
            ..Default::default()
        };
        let _ = openai_generate_with(
            &request(),
            &config(Some("configured-secret"), None),
            &mut transport,
        );
        assert_eq!(transport.paths, vec!["/v1/responses".to_owned()]);
    }

    #[test]
    fn multiple_output_text_blocks_are_concatenated() {
        let mut body = successful_body();
        body["output"] = json!([
            {"content": [{"type": "output_text", "text": "first "}, {"type": "output_text", "text": "second"}]},
            {"content": [{"type": "output_text", "text": " third"}]},
        ]);
        assert_eq!(parsed(body).text, "first second third");
    }

    #[test]
    fn real_output_text_block_extracts_text_despite_extra_fields() {
        let mut body = successful_body();
        body["output"] = json!([{
            "content": [{
                "type": "output_text",
                "annotations": [],
                "logprobs": [],
                "text": "Paris",
            }],
        }]);
        body["usage"]["output_tokens_details"] = json!({"reasoning_tokens": 0});
        assert_eq!(parsed(body).text, "Paris");
    }

    #[test]
    fn converse_real_output_text_block_extracts_text_despite_extra_fields() {
        let offered = BTreeSet::new();
        let OpenAiConverseResult::Turn(turn) = parse_converse_response(
            &json!({
                "model": "gpt",
                "status": "completed",
                "usage": {
                    "input_tokens": 12,
                    "output_tokens": 34,
                    "total_tokens": 46,
                    "output_tokens_details": {"reasoning_tokens": 0}
                },
                "output": [{
                    "content": [{
                        "type": "output_text",
                        "annotations": [],
                        "logprobs": [],
                        "text": "Paris"
                    }]
                }]
            })
            .to_string(),
            &offered,
        ) else {
            panic!("text turn expected")
        };
        assert_eq!(turn.text, "Paris");
    }

    #[test]
    fn legacy_output_text_key_without_type_yields_empty_text() {
        let mut body = successful_body();
        body["output"] = json!([{"content": [{"output_text": "x"}]}]);
        assert_eq!(parsed(body).text, "");
    }

    #[test]
    fn json_schema_uses_text_format_and_never_response_format() {
        let mut request = request();
        request.json_schema = Some(json!({"title": "Answer", "type": "object"}));
        let body = request_body(&request, "gpt-5.4-mini");
        assert_eq!(body["text"]["format"]["type"], "json_schema");
        assert_eq!(body["text"]["format"]["name"], "Answer");
        assert_eq!(
            body["text"]["format"]["schema"],
            request.json_schema.unwrap()
        );
        assert_eq!(body["text"]["format"]["strict"], true);
        assert!(body.get("response_format").is_none());
    }

    #[test]
    fn schema_name_accepts_only_documented_title_vocabulary() {
        assert_eq!(
            schema_name(Some(&json!({"title": "valid-name_123"}))),
            "valid-name_123"
        );
        assert_eq!(
            schema_name(Some(&json!({"title": "not valid"}))),
            "response"
        );
        assert_eq!(
            schema_name(Some(&json!({"title": "a".repeat(65)}))),
            "response"
        );
    }

    #[test]
    fn json_output_uses_text_json_object_format() {
        let mut request = request();
        request.json_output = true;
        let body = request_body(&request, "gpt-5.4-mini");
        assert_eq!(body["text"]["format"]["type"], "json_object");
        assert!(body.get("response_format").is_none());
    }

    #[test]
    fn model_effort_suffix_is_removed_before_request() {
        for suffix in OPENAI_EFFORT_SUFFIXES {
            let body = request_body(&request(), &format!("gpt-5{suffix}"));
            assert_eq!(body["model"], "gpt-5");
            assert!(body.get("reasoning").is_none());
        }
        let body = request_body(&request(), "gpt-5");
        assert_eq!(body["model"], "gpt-5");
        assert!(body.get("reasoning").is_none());
    }

    #[test]
    fn suffix_match_is_exact_not_prefix_gpt_5_turbo_unchanged() {
        let body = request_body(&request(), "gpt-5-turbo");
        assert_eq!(body["model"], "gpt-5-turbo");
    }

    #[test]
    fn request_never_sends_temperature() {
        assert!(
            request_body(&request(), "gpt-5.4-mini")
                .get("temperature")
                .is_none()
        );
    }

    #[test]
    fn thinking_budget_is_ignored_and_generated_thinking_is_none() {
        let mut request = request();
        request.thinking_budget = Some(5_000);
        let body = request_body(&request, "gpt-5.4-mini");
        assert!(body.get("thinking").is_none());
        assert_eq!(body["max_output_tokens"], 4_000);
        assert!(parsed(successful_body()).thinking.is_none());
    }

    #[test]
    fn openai_schema_is_reduced_before_embedding() {
        let mut request = request();
        request.json_schema = Some(json!({
            "type": "array",
            "minLength": 1,
            "maxLength": 8,
            "maxItems": 4,
            "minimum": 2,
            "maximum": 9,
        }));
        let schema = &request_body(&request, "gpt-5.4-mini")["text"]["format"]["schema"];
        assert!(schema.get("minLength").is_none());
        assert!(schema.get("maxLength").is_none());
        assert_eq!(schema["maxItems"], 4);
        assert_eq!(schema["minimum"], 2);
        assert_eq!(schema["maximum"], 9);
    }

    #[test]
    fn responses_usage_uses_responses_field_names() {
        let mut body = successful_body();
        body["usage"] = json!({
            "input_tokens": 2,
            "output_tokens": 3,
            "total_tokens": 5,
            "input_tokens_details": {"cached_tokens": 4},
            "output_tokens_details": {"reasoning_tokens": 6},
        });
        let usage = parsed(body).usage;
        assert_eq!(
            usage,
            json!({
                "input_tokens": 2,
                "output_tokens": 3,
                "total_tokens": 5,
                "cached_tokens": 4,
                "reasoning_tokens": 6,
                "model_version": "gpt-response-model",
            })
        );
        assert!(usage.get("cache_creation_tokens").is_none());
    }

    #[test]
    fn usage_adds_model_version_only_after_nonzero_usage() {
        let mut nonzero = successful_body();
        nonzero["usage"] = json!({"input_tokens": 1});
        assert_eq!(parsed(nonzero).usage["model_version"], "gpt-response-model");

        let mut zero = successful_body();
        zero["usage"] = json!({"input_tokens": 0, "output_tokens": 0});
        assert_eq!(parsed(zero).usage, json!({}));
    }

    #[test]
    fn all_zero_or_absent_usage_is_empty() {
        let journal = temp_journal();
        let mut zero = successful_body();
        zero["usage"] = json!({"input_tokens": 0, "output_tokens": 0, "total_tokens": 0});
        let zero = parsed(zero);
        let assessment = assess_provider_result(ProviderResultView {
            journal_path: &journal,
            context: "test.generate",
            model: &zero.model,
            text: &zero.text,
            finish_reason: &zero.finish_reason,
            usage: &zero.usage,
            json_output: false,
            enforce_responsiveness: false,
            raw_response_snippet: None,
        });
        assert!(assessment.token_log_error.is_none());
        assert!(!journal.join("tokens").exists());

        let nonzero = parsed(successful_body());
        let assessment = assess_provider_result(ProviderResultView {
            journal_path: &journal,
            context: "test.generate",
            model: &nonzero.model,
            text: &nonzero.text,
            finish_reason: &nonzero.finish_reason,
            usage: &nonzero.usage,
            json_output: false,
            enforce_responsiveness: false,
            raw_response_snippet: None,
        });
        assert!(assessment.token_log_error.is_none());
        let files = fs::read_dir(journal.join("tokens")).unwrap().count();
        assert_eq!(files, 1);
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn http_and_transport_failures_map_to_fixture_codes() {
        let cases = [
            (
                Ok(HttpResponse {
                    status: 401,
                    body: "{}".into(),
                }),
                "provider_key_invalid",
                true,
            ),
            (
                Ok(HttpResponse {
                    status: 429,
                    body: "{}".into(),
                }),
                "provider_quota_exceeded",
                true,
            ),
            (
                Ok(HttpResponse {
                    status: 400,
                    body: json!({"error": {"message": "maximum context length exceeded"}})
                        .to_string(),
                }),
                "context_window_exceeded",
                false,
            ),
            (
                Err(EndpointTransportError::Connection),
                "network_unreachable",
                false,
            ),
        ];
        for (response, expected_code, expected_blocking) in cases {
            let mut transport = StubTransport {
                responses: vec![response],
                ..Default::default()
            };
            let OpenAiResult::Failed(failure) = openai_generate_with(
                &request(),
                &config(Some("configured-secret"), None),
                &mut transport,
            ) else {
                panic!("case must fail");
            };
            assert_eq!(failure.reason_code.as_deref(), Some(expected_code));
            let refusal = refusal_for(&LaneOutcome::OpenAiFailure(failure), "openai", None);
            assert_eq!(
                refusal.reason_code.as_ref().map(ReasonCodeValue::as_wire),
                Some(expected_code)
            );
            assert_eq!(refusal.blocking, expected_blocking);
        }
    }

    #[test]
    fn process_environment_key_is_ignored_when_config_key_is_absent() {
        let mut transport = StubTransport::default();
        let result = openai_generate_with_lookup(
            &request(),
            &config(None, None),
            &mut transport,
            crate::overrides::lookup_leaks_conventional_keys,
        );
        assert_eq!(
            result,
            OpenAiResult::Failed(OpenAiFailure {
                reason_code: Some("provider_key_missing".into()),
                detail: None,
            })
        );
        assert!(transport.posts.is_empty());
    }

    #[test]
    fn missing_or_blank_configured_key_makes_no_request() {
        for key in [None, Some("  \t")] {
            let mut transport = StubTransport::default();
            assert_eq!(
                openai_generate_with(&request(), &config(key, None), &mut transport),
                OpenAiResult::Failed(OpenAiFailure {
                    reason_code: Some("provider_key_missing".into()),
                    detail: None,
                })
            );
            assert!(transport.posts.is_empty());
        }
    }

    #[test]
    fn provider_error_body_never_reaches_refusal_detail() {
        let credential = "configured-secret";
        let mut transport = StubTransport {
            responses: vec![Ok(HttpResponse {
                status: 500,
                body: format!("provider echoed {credential}"),
            })],
            ..Default::default()
        };
        let OpenAiResult::Failed(failure) =
            openai_generate_with(&request(), &config(Some(credential), None), &mut transport)
        else {
            panic!("server error must fail");
        };
        let refusal = refusal_for(&LaneOutcome::OpenAiFailure(failure), "openai", None);
        assert!(!refusal.detail.contains(credential));
        assert_eq!(transport.api_keys, [credential]);
    }

    #[test]
    fn non_context_window_http_error_body_reaches_refusal_detail() {
        let body = r#"{"error":{"message":"invalid temperature distinctive-400-openai"}}"#;
        let mut transport = StubTransport {
            responses: vec![Ok(HttpResponse {
                status: 400,
                body: body.to_owned(),
            })],
            ..Default::default()
        };
        let OpenAiResult::Failed(failure) = openai_generate_with(
            &request(),
            &config(Some("configured-secret"), None),
            &mut transport,
        ) else {
            panic!("400 must fail");
        };
        assert_eq!(failure.detail.as_deref(), Some(body));
        let refusal = refusal_for(&LaneOutcome::OpenAiFailure(failure), "openai", None);
        assert_eq!(refusal.detail, body);
        assert!(!refusal.detail.contains("fixture"));
        assert_ne!(refusal.detail, crate::refusal::LIVE_PROVIDER_FAILURE_DETAIL);
    }

    #[test]
    fn captured_http_error_body_truncates_to_512_utf8_characters() {
        let body = format!("I cannot {}", "界".repeat(600));
        let mut transport = StubTransport {
            responses: vec![Ok(HttpResponse {
                status: 500,
                body: body.clone(),
            })],
            ..Default::default()
        };
        let OpenAiResult::Failed(failure) = openai_generate_with(
            &request(),
            &config(Some("configured-secret"), None),
            &mut transport,
        ) else {
            panic!("server error must fail");
        };
        let detail = failure.detail.expect("captured body");
        assert_eq!(detail.chars().count(), NON_RESPONSIVE_RAW_OUTPUT_CAP_CHARS);
        assert!(detail.starts_with("I cannot "));
        assert!(!detail.contains("fixture"));
    }

    #[test]
    fn blank_extracted_text_keeps_distinctive_raw_snippet_on_refusal() {
        let mut body = successful_body();
        body["output"] = json!([{"content": [{"type": "output_text", "text": ""}]}]);
        body["distinctive"] = json!("blank-visible-openai-xyz");
        let generated = parsed(body);
        assert!(generated.text.trim().is_empty());
        let snippet = generated
            .raw_response_snippet
            .as_deref()
            .expect("raw snippet");
        assert!(snippet.contains("blank-visible-openai-xyz"));
        let journal = temp_journal();
        let assessment = assess_provider_result(ProviderResultView {
            journal_path: &journal,
            context: "test.generate",
            model: &generated.model,
            text: &generated.text,
            finish_reason: &generated.finish_reason,
            usage: &generated.usage,
            json_output: false,
            enforce_responsiveness: false,
            raw_response_snippet: generated.raw_response_snippet.as_deref(),
        });
        assert_eq!(
            assessment.failure,
            Some(ValidationFailure::ProviderResponseInvalid {
                raw_response_snippet: generated.raw_response_snippet.clone(),
            })
        );
        let refusal = refusal_for(
            &LaneOutcome::ValidationFailure(assessment.failure.unwrap()),
            "openai",
            None,
        );
        assert!(refusal.detail.contains("blank-visible-openai-xyz"));
        assert!(!refusal.detail.contains("fixture"));
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn converse_body_and_tool_turns_follow_responses_shapes() {
        let messages = vec![
            ConverseMessage::User { text: "ask".into() },
            ConverseMessage::Assistant {
                text: "working".into(),
                tool_calls: vec![ConverseToolCall {
                    id: "call-1".into(),
                    name: "weather".into(),
                    arguments: json!({"city":"Denver"}),
                    not_offered: false,
                }],
            },
            ConverseMessage::ToolResult {
                tool_call_id: "call-1".into(),
                tool_name: "weather".into(),
                output: "sunny".into(),
            },
        ];
        let tools = vec![ConverseToolSpec {
            name: "weather".into(),
            description: "weather".into(),
            parameters: json!({"type":"object"}),
        }];
        let body = converse_request_body(&request(), &messages, &tools, "gpt");
        assert_eq!(
            crate::converse::canonical_json(&body),
            crate::converse::canonical_json(&json!({
                "model":"gpt", "max_output_tokens":4000,
                "input":[
                    {"role":"system","content":[{"type":"input_text","text":"system"}]},
                    {"role":"user","content":[{"type":"input_text","text":"ask"}]},
                    {"role":"assistant","content":[{"type":"output_text","text":"working"}]},
                    {"type":"function_call","call_id":"call-1","name":"weather","arguments":"{\"city\":\"Denver\"}"},
                    {"type":"function_call_output","call_id":"call-1","output":"sunny"}
                ],
                "tools":[{"type":"function","name":"weather","description":"weather","parameters":{"type":"object"}}]
            }))
        );
        let mut without_tools = body.clone();
        without_tools.as_object_mut().unwrap().remove("tools");
        assert_ne!(
            crate::converse::canonical_json(&body),
            crate::converse::canonical_json(&without_tools)
        );
        assert_ne!(
            crate::converse::canonical_json(&body),
            crate::converse::canonical_json(&converse_request_body(
                &request(),
                &messages.into_iter().rev().collect::<Vec<_>>(),
                &tools,
                "gpt"
            ))
        );

        let offered = ["weather".to_owned()].into_iter().collect();
        let OpenAiConverseResult::Turn(turn) = parse_converse_response(&json!({
            "model":"gpt", "status":"completed", "usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5,"output_tokens_details":{"reasoning_tokens":1}},
            "output":[{"content":[{"type":"output_text","text":"before"}]},{"id":"fc","call_id":"call-1","type":"function_call","name":"weather","arguments":"{\"city\":\"Denver\"}"}]
        }).to_string(), &offered) else { panic!("tool turn expected") };
        assert_eq!(turn.finish_reason, "tool_calls");
        assert_eq!(turn.text, "before");
        assert!(!turn.tool_calls[0].not_offered);
        assert_eq!(
            turn.usage,
            json!({"input_tokens":2,"output_tokens":3,"total_tokens":5,"reasoning_tokens":1,"model_version":"gpt"})
        );
        assert_eq!(
            crate::validation::sanitize_finish_reason(&turn.finish_reason),
            crate::SanitizedFinishReason::ToolCalls
        );
        let OpenAiResult::Generated(generated) = parse_response(
            &json!({
                "model":"gpt", "status":"completed", "usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5,"output_tokens_details":{"reasoning_tokens":1}},
                "output":[{"content":[{"type":"output_text","text":"before"}]},{"id":"fc","call_id":"call-1","type":"function_call","name":"weather","arguments":"{\"city\":\"Denver\"}"}]
            })
            .to_string(),
            "",
        ) else {
            panic!("generated response expected")
        };
        assert_eq!(
            crate::validation::usage_for_log(&turn.usage),
            crate::validation::usage_for_log(&generated.usage)
        );
        let OpenAiConverseResult::Turn(zero_cache) = parse_converse_response(
            &json!({
                "model":"gpt","status":"completed","usage":{"input_tokens":1,"output_tokens":1,"input_tokens_details":{"cached_tokens":0}},"output":[]
            })
            .to_string(),
            &offered,
        ) else {
            panic!("zero cache turn expected")
        };
        let OpenAiConverseResult::Turn(absent_cache) = parse_converse_response(
            &json!({
                "model":"gpt","status":"completed","usage":{"input_tokens":1,"output_tokens":1},"output":[]
            })
            .to_string(),
            &offered,
        ) else {
            panic!("absent cache turn expected")
        };
        assert_eq!(zero_cache.usage["cached_tokens"], 0);
        assert!(absent_cache.usage.get("cached_tokens").is_none());

        let OpenAiConverseResult::Turn(unoffered) = parse_converse_response(&json!({
            "model":"gpt","status":"completed","usage":{},"output":[{"type":"function_call","call_id":"c","name":"other","arguments":"{}"}]
        }).to_string(), &offered) else { panic!("tool turn expected") };
        assert!(unoffered.tool_calls[0].not_offered);
        let assessment = assess_provider_result(ProviderResultView {
            journal_path: &temp_journal(),
            context: "test.converse",
            model: &unoffered.model,
            text: &unoffered.text,
            finish_reason: &unoffered.finish_reason,
            usage: &unoffered.usage,
            json_output: false,
            enforce_responsiveness: false,
            raw_response_snippet: None,
        });
        assert_eq!(assessment.failure, None);
        let OpenAiConverseResult::Failed(invalid) = parse_converse_response(&json!({
            "model":"gpt","status":"completed","usage":{},"output":[{"type":"function_call","call_id":"c","name":"weather","arguments":"not json"}]
        }).to_string(), &offered) else { panic!("invalid expected") };
        assert_eq!(invalid.reason_code, "tool_call_arguments_invalid");
        assert!(invalid.retryable && !invalid.blocking);
        let OpenAiConverseResult::Failed(missing) = parse_converse_response(&json!({
            "model":"gpt","status":"completed","usage":{},"output":[{"type":"function_call","arguments":"{}"}]
        }).to_string(), &offered) else { panic!("missing calls expected") };
        assert_eq!(missing.reason_code, "tool_call_arguments_invalid");
        let OpenAiConverseResult::Failed(mixed_malformed) = parse_converse_response(
            &json!({
                "model":"gpt","status":"completed","usage":{},"output":[
                    {"type":"function_call","call_id":"valid","name":"weather","arguments":"{}"},
                    {"type":"function_call","call_id":"malformed","arguments":"{}"}
                ]
            })
            .to_string(),
            &offered,
        ) else {
            panic!("mixed malformed calls must fail")
        };
        assert_eq!(mixed_malformed.reason_code, "tool_call_arguments_invalid");
        let OpenAiConverseResult::Turn(truncated) = parse_converse_response(&json!({
            "model":"gpt","status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"usage":{},"output":[{"type":"function_call","call_id":"c","name":"weather","arguments":"{"}]
        }).to_string(), &offered) else { panic!("truncated expected") };
        assert_eq!(truncated.finish_reason, "max_tokens");
        assert!(truncated.tool_calls.is_empty());
        let OpenAiConverseResult::Turn(text_only) = parse_converse_response(&json!({
            "model":"gpt","status":"completed","usage":{},"output":[{"content":[{"type":"output_text","text":"plain text"}]}]
        }).to_string(), &offered) else { panic!("text turn expected") };
        assert_eq!(text_only.text, "plain text");
        assert!(text_only.tool_calls.is_empty());
    }

    #[test]
    fn converse_http_failures_reuse_openai_classification() {
        let mut transport = StubTransport {
            responses: vec![Ok(HttpResponse {
                status: 429,
                body: "{}".into(),
            })],
            ..Default::default()
        };
        let OpenAiConverseResult::Failed(failure) = openai_converse_with(
            &request(),
            &[],
            &[],
            &config(Some("configured-secret"), None),
            &mut transport,
        ) else {
            panic!("failure expected")
        };
        assert_eq!(failure.reason_code, "provider_quota_exceeded");
        assert!(failure.blocking);
    }
}

#[cfg(test)]
mod vocabulary_tests {
    #[test]
    fn openai_production_never_reads_choices_shape() {
        let production = include_str!("openai.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("OpenAI module has a production prefix");
        for member in ["choices", "prompt_tokens", "completion_tokens"] {
            let quoted_member = format!("\"{member}\"");
            assert!(
                !production.contains(&quoted_member),
                "OpenAI production must not use Chat Completions primitive {member:?}"
            );
        }
    }
}

fn unknown_finish_reason() -> &'static str {
    solstone_core_generate::contract()["response"]["finish_reason_unknown"]
        .as_str()
        .expect("generate contract carries the unknown finish reason")
}
