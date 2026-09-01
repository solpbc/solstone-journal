// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Google Gemini generateContent generation.

use std::{collections::BTreeSet, time::Duration};

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

const GOOGLE_API_KEY_ENV: &str = "GOOGLE_API_KEY";
const GOOGLE_BASE_URL: &str = "https://generativelanguage.googleapis.com";
const DEFAULT_MODEL: &str = "gemini-3.5-flash";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
// Google does not publish stable context-window error text, so this is the
// same best-effort heuristic used by the other provider arms.
const CONTEXT_WINDOW_PATTERNS: &[&str] = &[
    "prompt is too long",
    "maximum context length",
    "context window",
    "context length",
    "too many tokens",
    "exceeds the available context size",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleFailure {
    pub reason_code: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GoogleGenerated {
    pub text: String,
    pub model: String,
    pub usage: Value,
    pub finish_reason: String,
    pub thinking: Option<Value>,
    pub raw_response_snippet: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GoogleResult {
    Generated(GoogleGenerated),
    Failed(GoogleFailure),
}

pub type GoogleTurn = ConverseTurn;
pub type GoogleConverseFailure = ConverseFailure;

#[derive(Debug, Clone, PartialEq)]
pub enum GoogleConverseResult {
    Turn(Box<GoogleTurn>),
    Failed(GoogleConverseFailure),
}

pub trait GoogleTransport {
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
pub struct UreqGoogleTransport;

impl GoogleTransport for UreqGoogleTransport {
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
            .header("x-goog-api-key", api_key)
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

pub fn google_generate(request: &GenerateRequest, config: &Map<String, Value>) -> GoogleResult {
    let mut transport = UreqGoogleTransport;
    google_generate_with(request, config, &mut transport)
}

pub fn google_converse(
    request: &GenerateRequest,
    messages: &[ConverseMessage],
    tools: &[ConverseToolSpec],
    config: &Map<String, Value>,
) -> GoogleConverseResult {
    let mut transport = UreqGoogleTransport;
    google_converse_with(request, messages, tools, config, &mut transport)
}

fn google_converse_with<T: GoogleTransport>(
    request: &GenerateRequest,
    messages: &[ConverseMessage],
    tools: &[ConverseToolSpec],
    config: &Map<String, Value>,
    transport: &mut T,
) -> GoogleConverseResult {
    let Some(api_key) = configured_api_key(config) else {
        return converse_failure("provider_key_missing");
    };
    let model = configured_model(config);
    let base_url = crate::overrides::configured_base_url(config, GOOGLE_BASE_URL);
    let path = format!("/v1beta/models/{model}:generateContent");
    let body = converse_request_body(request, messages, tools);
    let response = match transport.post_json(
        &base_url,
        &path,
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
        return GoogleConverseResult::Failed(ConverseFailure {
            reason_code: reason_code.to_owned(),
            retryable,
            blocking,
            detail,
        });
    }
    let offered = tools.iter().map(|tool| tool.name.clone()).collect();
    parse_converse_response(&response.body, &offered)
}

fn google_generate_with<T: GoogleTransport>(
    request: &GenerateRequest,
    config: &Map<String, Value>,
    transport: &mut T,
) -> GoogleResult {
    google_generate_with_lookup(
        request,
        config,
        transport,
        crate::overrides::non_blank_process_env,
    )
}

fn google_generate_with_lookup<T: GoogleTransport>(
    request: &GenerateRequest,
    config: &Map<String, Value>,
    transport: &mut T,
    env: impl Fn(&str) -> Option<String>,
) -> GoogleResult {
    let env = &env;
    let Some(api_key) = crate::overrides::configured_api_key_with(config, GOOGLE_API_KEY_ENV, env)
    else {
        return failure("provider_key_missing");
    };
    let model = crate::overrides::configured_model_with(config, DEFAULT_MODEL, env);
    let base_url = crate::overrides::configured_base_url_with(config, GOOGLE_BASE_URL, env);
    let path = format!("/v1beta/models/{model}:generateContent");
    let body = request_body(request, &model);
    let response = match transport.post_json(
        &base_url,
        &path,
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
        return GoogleResult::Failed(GoogleFailure {
            reason_code: Some(reason_code.to_owned()),
            detail,
        });
    }
    parse_response(&response.body, &api_key)
}

fn configured_api_key(config: &Map<String, Value>) -> Option<String> {
    crate::overrides::configured_api_key(config, GOOGLE_API_KEY_ENV)
}

fn configured_model(config: &Map<String, Value>) -> String {
    crate::overrides::configured_model(config, DEFAULT_MODEL)
}

fn request_body(request: &GenerateRequest, _model: &str) -> Value {
    let parts = request
        .contents
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } => json!({"text": text}),
            ContentPart::Image { mime_type, data } => {
                json!({"inlineData": {"mimeType": mime_type, "data": data}})
            }
        })
        .collect::<Vec<_>>();
    let mut body = json!({
        "contents": [{"role": "user", "parts": parts}],
        "generationConfig": {
            "temperature": request.temperature,
            "maxOutputTokens": generate_token_budget(
                "google",
                request.max_output_tokens,
                request.thinking_budget,
            ),
        },
    });
    if let Some(system) = &request.system_instruction {
        body["systemInstruction"] = json!({"parts": [{"text": system}]});
    }
    if let Some(budget) = request.thinking_budget {
        body["generationConfig"]["thinkingConfig"] = json!({"thinkingBudget": budget});
    }
    if let Some(schema) = prepare_provider_schema(request.json_schema.as_ref(), "google") {
        body["generationConfig"]["responseJsonSchema"] = schema;
        body["generationConfig"]["responseMimeType"] = json!("application/json");
    } else if request.json_output {
        body["generationConfig"]["responseMimeType"] = json!("application/json");
    }
    body
}

/// Reshape a tool parameter schema into the subset Gemini accepts.
///
/// Function declarations take a restricted OpenAPI 3.0 Schema; `additionalProperties`
/// is not part of it, and Gemini rejects the whole request with HTTP 400
/// INVALID_ARGUMENT -- one violation per declaration -- rather than ignoring the
/// key. Anthropic and OpenAI accept the same schema unchanged, so the narrowing
/// belongs here on the way out rather than in the shared tool definitions.
fn gemini_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .filter(|(key, _)| key.as_str() != "additionalProperties")
                .map(|(key, value)| (key.clone(), gemini_schema(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(gemini_schema).collect()),
        other => other.clone(),
    }
}

fn converse_request_body(
    request: &GenerateRequest,
    messages: &[ConverseMessage],
    tools: &[ConverseToolSpec],
) -> Value {
    let contents =
        messages
            .iter()
            .map(|message| match message {
                ConverseMessage::User { text } => {
                    json!({"role": "user", "parts": [{"text": text}]})
                }
                ConverseMessage::Assistant { text, tool_calls } => {
                    // Multi-turn Gemini 3 tool history needs thoughtSignature replay;
                    // this generic assistant representation does not carry it yet.
                    let mut parts = Vec::new();
                    if !text.is_empty() {
                        parts.push(json!({"text": text}));
                    }
                    parts.extend(tool_calls.iter().map(|call| json!({
                    "functionCall": {"id": call.id, "name": call.name, "args": call.arguments},
                })));
                    json!({"role": "model", "parts": parts})
                }
                ConverseMessage::ToolResult {
                    tool_call_id,
                    tool_name,
                    output,
                } => json!({
                    "role": "user",
                    "parts": [{"functionResponse": {
                        "id": tool_call_id,
                        "name": tool_name,
                        "response": {"result": output},
                    }}],
                }),
            })
            .collect::<Vec<_>>();
    let mut body = json!({
        "contents": contents,
        "tools": [{"functionDeclarations": tools.iter().map(|tool| json!({
            "name": tool.name,
            "description": tool.description,
            "parameters": gemini_schema(&tool.parameters),
        })).collect::<Vec<_>>() }],
        "generationConfig": {
            "temperature": request.temperature,
            "maxOutputTokens": generate_token_budget(
                "google",
                request.max_output_tokens,
                request.thinking_budget,
            ),
        },
    });
    if let Some(system) = &request.system_instruction {
        body["systemInstruction"] = json!({"parts": [{"text": system}]});
    }
    if let Some(budget) = request.thinking_budget {
        body["generationConfig"]["thinkingConfig"] = json!({"thinkingBudget": budget});
    }
    body
}

fn request_timeout(timeout_s: Option<f64>) -> Duration {
    timeout_s
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .map(Duration::from_secs_f64)
        .unwrap_or(DEFAULT_TIMEOUT)
}

fn parse_response(body: &str, secret: &str) -> GoogleResult {
    let raw_snippet = capture_provider_detail(body, secret);
    let Ok(body) = serde_json::from_str::<Value>(body) else {
        return failure("provider_response_invalid");
    };
    let Some(candidates) = body.get("candidates").and_then(Value::as_array) else {
        return failure("provider_response_invalid");
    };
    let Some(candidate) = candidates.first() else {
        return failure("provider_response_invalid");
    };
    let Some(parts) = candidate
        .get("content")
        .and_then(Value::as_object)
        .and_then(|content| content.get("parts"))
        .and_then(Value::as_array)
    else {
        return failure("provider_response_invalid");
    };
    let Some(model) = body
        .get("modelVersion")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
    else {
        return failure("provider_response_invalid");
    };
    let usage = match response_usage(&body, model) {
        Ok(usage) => usage,
        Err(()) => return failure("provider_response_invalid"),
    };
    let mut text = String::new();
    for part in parts {
        if let Some(value) = part.get("text").and_then(Value::as_str) {
            text.push_str(value);
        }
    }
    GoogleResult::Generated(GoogleGenerated {
        text,
        model: model.to_owned(),
        usage,
        finish_reason: normalize_finish_reason(candidate),
        thinking: None,
        raw_response_snippet: raw_snippet,
    })
}

fn parse_converse_response(body: &str, offered: &BTreeSet<String>) -> GoogleConverseResult {
    let Ok(body) = serde_json::from_str::<Value>(body) else {
        return converse_failure("provider_response_invalid");
    };
    let Some(candidates) = body.get("candidates").and_then(Value::as_array) else {
        return converse_failure("provider_response_invalid");
    };
    let Some(candidate) = candidates.first() else {
        return converse_failure("provider_response_invalid");
    };
    let Some(parts) = candidate
        .get("content")
        .and_then(Value::as_object)
        .and_then(|content| content.get("parts"))
        .and_then(Value::as_array)
    else {
        return converse_failure("provider_response_invalid");
    };
    let Some(model) = body
        .get("modelVersion")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
    else {
        return converse_failure("provider_response_invalid");
    };
    let usage = match response_usage(&body, model) {
        Ok(usage) => usage,
        Err(()) => return converse_failure("provider_response_invalid"),
    };
    let mut text = String::new();
    let mut function_parts = Vec::new();
    for part in parts {
        if let Some(value) = part.get("text").and_then(Value::as_str) {
            text.push_str(value);
        }
        if let Some(function_call) = part.get("functionCall") {
            function_parts.push(function_call);
        }
    }
    let finish_reason = normalize_finish_reason(candidate);
    if finish_reason == "max_tokens" {
        return GoogleConverseResult::Turn(Box::new(ConverseTurn {
            text,
            tool_calls: Vec::new(),
            finish_reason,
            usage,
            model: model.to_owned(),
            thinking: None,
        }));
    }
    if finish_reason == "malformed_function_call" {
        return converse_failure("tool_call_arguments_invalid");
    }
    let mut tool_calls = Vec::new();
    for (position, function_call) in function_parts.into_iter().enumerate() {
        let Some(name) = function_call
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
        else {
            return converse_failure("tool_call_arguments_invalid");
        };
        // Gemini assigns no call id -- verified against the live endpoint, a real
        // tool call carries only `name` and `args`. The runtime needs an id to
        // correlate a call with its result, and the outbound `functionResponse`
        // echoes whatever we choose, so synthesize one that stays unique across
        // parallel calls to the same tool within a turn.
        let id = function_call
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("{name}-{position}"));
        let id = id.as_str();
        let Some(arguments) = function_call.get("args") else {
            return converse_failure("tool_call_arguments_invalid");
        };
        if !arguments.is_object() {
            return converse_failure("tool_call_arguments_invalid");
        }
        tool_calls.push(ConverseToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments: arguments.clone(),
            not_offered: !offered.contains(name),
        });
    }
    GoogleConverseResult::Turn(Box::new(ConverseTurn {
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

fn response_usage(body: &Value, model: &str) -> Result<Value, ()> {
    let Some(usage) = body.get("usageMetadata") else {
        return Ok(Value::Object(Map::new()));
    };
    let Some(usage) = usage.as_object() else {
        return Err(());
    };
    let mut normalized = Map::new();
    copy_usage_number(usage, "promptTokenCount", "input_tokens", &mut normalized)?;
    copy_usage_number(
        usage,
        "candidatesTokenCount",
        "output_tokens",
        &mut normalized,
    )?;
    copy_usage_number(usage, "totalTokenCount", "total_tokens", &mut normalized)?;
    copy_nonzero_usage_number(
        usage,
        "thoughtsTokenCount",
        "reasoning_tokens",
        &mut normalized,
    )?;
    copy_nonzero_usage_number(
        usage,
        "cachedContentTokenCount",
        "cached_tokens",
        &mut normalized,
    )?;
    if normalized
        .values()
        .all(|value| value.as_u64().is_none_or(|value| value == 0))
    {
        return Ok(Value::Object(Map::new()));
    }
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

fn copy_nonzero_usage_number(
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
    if value != 0 {
        normalized.insert(target.to_owned(), Value::from(value));
    }
    Ok(())
}

fn normalize_finish_reason(candidate: &Value) -> String {
    let Some(reason) = candidate
        .get("finishReason")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
    else {
        return unknown_finish_reason().to_owned();
    };
    match reason.to_ascii_uppercase().as_str() {
        "STOP" => "stop".to_owned(),
        "MAX_TOKENS" => "max_tokens".to_owned(),
        "SAFETY" => "content_filter".to_owned(),
        // Preserve future Gemini reasons in normalized form for contract sanitization.
        _ => reason.to_ascii_lowercase(),
    }
}

fn classify_http_failure(status: u16, body: &str) -> &'static str {
    match status {
        401 | 403 => "provider_key_invalid",
        429 => "provider_quota_exceeded",
        400 if is_context_window_error(body) => "context_window_exceeded",
        // Bare Gemini INVALID_ARGUMENT errors reject the request, not its response.
        400 => "provider_request_rejected",
        500..=599 => "provider_unavailable",
        // Other HTTP status classes do not establish a valid provider response.
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

fn failure(reason_code: &str) -> GoogleResult {
    GoogleResult::Failed(GoogleFailure {
        reason_code: Some(reason_code.to_owned()),
        detail: None,
    })
}

fn converse_failure(reason_code: &str) -> GoogleConverseResult {
    let (retryable, blocking) = crate::converse::converse_failure_flags(reason_code);
    GoogleConverseResult::Failed(ConverseFailure {
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

fn unknown_finish_reason() -> &'static str {
    solstone_core_generate::contract()["response"]["finish_reason_unknown"]
        .as_str()
        .expect("generate contract carries the unknown finish reason")
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
        base_urls: Vec<String>,
        paths: Vec<String>,
        api_keys: Vec<String>,
    }

    impl GoogleTransport for StubTransport {
        fn post_json(
            &mut self,
            base_url: &str,
            path: &str,
            body: &Value,
            api_key: &str,
            _timeout: Duration,
        ) -> Result<HttpResponse, EndpointTransportError> {
            self.posts.push(body.clone());
            self.base_urls.push(base_url.to_owned());
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
            env.insert(GOOGLE_API_KEY_ENV.into(), Value::String(key.into()));
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
            "modelVersion": "gemini-response-model",
            "candidates": [{
                "content": {"parts": [{"text": "done"}]},
                "finishReason": "STOP",
            }],
            "usageMetadata": {
                "promptTokenCount": 12,
                "candidatesTokenCount": 34,
                "totalTokenCount": 46,
            },
        })
    }

    fn generated(result: GoogleResult) -> GoogleGenerated {
        match result {
            GoogleResult::Generated(success) => success,
            GoogleResult::Failed(failure) => panic!("unexpected failure: {failure:?}"),
        }
    }

    fn parsed(body: Value) -> GoogleGenerated {
        generated(parse_response(&body.to_string(), ""))
    }

    fn temp_journal() -> std::path::PathBuf {
        crate::validation::isolated_journal_dir("google")
    }

    #[test]
    fn google_budget_sums_thinking_in_request_body() {
        let mut request = request();
        request.thinking_budget = Some(500);
        assert_eq!(
            request_body(&request, DEFAULT_MODEL)["generationConfig"]["maxOutputTokens"],
            4_500
        );
    }

    #[test]
    fn google_budget_clamps_in_request_body() {
        let mut request = request();
        request.max_output_tokens = 65_000;
        request.thinking_budget = Some(1_000);
        assert_eq!(
            request_body(&request, DEFAULT_MODEL)["generationConfig"]["maxOutputTokens"],
            65_535
        );
    }

    #[test]
    fn temperature_is_always_present_for_google() {
        let mut request = request();
        request.thinking_budget = Some(5_000);
        assert_eq!(
            request_body(&request, DEFAULT_MODEL)["generationConfig"]["temperature"],
            json!(request.temperature)
        );
    }

    #[test]
    fn thinking_budget_uses_google_thinking_config_shape() {
        for (budget, expected) in [
            (Some(0), Some(json!({"thinkingBudget": 0}))),
            (Some(5_000), Some(json!({"thinkingBudget": 5_000}))),
            (None, None),
        ] {
            let mut request = request();
            request.thinking_budget = budget;
            let config = &request_body(&request, DEFAULT_MODEL)["generationConfig"];
            assert_eq!(config.get("thinkingConfig"), expected.as_ref());
        }
    }

    #[test]
    fn google_schema_is_reduced_before_embedding() {
        let mut request = request();
        request.json_schema = Some(json!({
            "type": "array",
            "minLength": 1,
            "maxLength": 8,
            "maxItems": 4,
            "minItems": 1,
            "minimum": 2,
            "maximum": 9,
            "items": {"type": "string", "maxLength": 2, "minimum": 0},
            "properties": {"nested": {"type": "string", "minLength": 1, "maximum": 3}},
        }));
        let config = &request_body(&request, DEFAULT_MODEL)["generationConfig"];
        let schema = &config["responseJsonSchema"];
        assert_eq!(config["responseMimeType"], "application/json");
        assert!(schema.get("minLength").is_none());
        assert!(schema.get("maxLength").is_none());
        assert!(schema.get("maxItems").is_none());
        assert_eq!(schema["minItems"], 1);
        assert_eq!(schema["minimum"], 2);
        assert_eq!(schema["maximum"], 9);
        assert!(schema["items"].get("maxLength").is_none());
        assert_eq!(schema["items"]["minimum"], 0);
        assert!(schema["properties"]["nested"].get("minLength").is_none());
        assert_eq!(schema["properties"]["nested"]["maximum"], 3);
    }

    #[test]
    fn json_output_without_schema_uses_only_response_mime_type() {
        let mut request = request();
        request.json_output = true;
        let config = &request_body(&request, DEFAULT_MODEL)["generationConfig"];
        assert_eq!(config["responseMimeType"], "application/json");
        assert!(config.get("responseJsonSchema").is_none());
    }

    #[test]
    fn request_posts_to_literal_generate_content_path() {
        let mut transport = StubTransport {
            responses: vec![Ok(response(successful_body()))],
            ..Default::default()
        };
        let _ = google_generate_with(
            &request(),
            &config(Some("configured-secret"), None),
            &mut transport,
        );
        assert_eq!(
            transport.base_urls,
            vec!["https://generativelanguage.googleapis.com".to_owned()]
        );
        assert_eq!(
            transport.paths,
            vec!["/v1beta/models/gemini-3.5-flash:generateContent".to_owned()]
        );
        assert_eq!(transport.api_keys, vec!["configured-secret".to_owned()]);
    }

    #[test]
    fn request_uses_google_content_and_system_shapes() {
        let mut request = request();
        request.contents.push(ContentPart::Image {
            mime_type: "image/png".into(),
            data: "encoded".into(),
        });
        let body = request_body(&request, DEFAULT_MODEL);
        assert_eq!(body["contents"][0]["parts"][0], json!({"text": "hello"}));
        assert_eq!(
            body["contents"][0]["parts"][1],
            json!({"inlineData": {"mimeType": "image/png", "data": "encoded"}})
        );
        assert_eq!(
            body["systemInstruction"],
            json!({"parts": [{"text": "system"}]})
        );
    }

    #[test]
    fn multiple_parts_are_concatenated() {
        let mut body = successful_body();
        body["candidates"][0]["content"]["parts"] = json!([
            {"text": "first "},
            {"inlineData": {"mimeType": "image/png", "data": "ignored"}},
            {"text": "second"},
        ]);
        assert_eq!(parsed(body).text, "first second");
    }

    #[test]
    fn empty_candidates_is_provider_response_invalid() {
        let result = parse_response(
            &json!({
                "candidates": [],
                "promptFeedback": {"blockReason": "SAFETY"},
            })
            .to_string(),
            "",
        );
        assert_eq!(
            result,
            GoogleResult::Failed(GoogleFailure {
                reason_code: Some("provider_response_invalid".into()),
                detail: None,
            })
        );
    }

    #[test]
    fn finish_reasons_are_normalized() {
        for (reason, expected) in [
            (Some("STOP"), "stop"),
            (Some("MAX_TOKENS"), "max_tokens"),
            (Some("SAFETY"), "content_filter"),
            (Some("RECITATION"), "recitation"),
            (Some("  Other  "), "other"),
            (None, unknown_finish_reason()),
        ] {
            let mut body = successful_body();
            let candidate = body["candidates"][0].as_object_mut().unwrap();
            if let Some(reason) = reason {
                candidate.insert("finishReason".into(), json!(reason));
            } else {
                candidate.remove("finishReason");
            }
            assert_eq!(parsed(body).finish_reason, expected);
        }
    }

    #[test]
    fn usage_metadata_uses_google_field_names() {
        let mut body = successful_body();
        body["usageMetadata"] = json!({
            "promptTokenCount": 2,
            "candidatesTokenCount": 3,
            "totalTokenCount": 5,
            "thoughtsTokenCount": 6,
            "cachedContentTokenCount": 4,
        });
        let usage = parsed(body).usage;
        assert_eq!(
            usage,
            json!({
                "input_tokens": 2,
                "output_tokens": 3,
                "total_tokens": 5,
                "reasoning_tokens": 6,
                "cached_tokens": 4,
                "model_version": "gemini-response-model",
            })
        );
        assert!(usage.get("cache_creation_tokens").is_none());
    }

    #[test]
    fn all_zero_or_absent_usage_is_empty() {
        let journal = temp_journal();
        let mut zero = successful_body();
        zero["usageMetadata"] = json!({
            "promptTokenCount": 0,
            "candidatesTokenCount": 0,
            "totalTokenCount": 0,
            "thoughtsTokenCount": 0,
            "cachedContentTokenCount": 0,
        });
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

        let mut absent = successful_body();
        absent.as_object_mut().unwrap().remove("usageMetadata");
        assert_eq!(parsed(absent).usage, json!({}));

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
                    status: 403,
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
                Ok(HttpResponse {
                    status: 400,
                    body: "{}".into(),
                }),
                "provider_request_rejected",
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
            let GoogleResult::Failed(failure) = google_generate_with(
                &request(),
                &config(Some("configured-secret"), None),
                &mut transport,
            ) else {
                panic!("case must fail");
            };
            assert_eq!(failure.reason_code.as_deref(), Some(expected_code));
            let refusal = refusal_for(&LaneOutcome::GoogleFailure(failure), "google", None);
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
        let result = google_generate_with_lookup(
            &request(),
            &config(None, None),
            &mut transport,
            crate::overrides::lookup_leaks_conventional_keys,
        );
        assert_eq!(
            result,
            GoogleResult::Failed(GoogleFailure {
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
                google_generate_with(&request(), &config(key, None), &mut transport),
                GoogleResult::Failed(GoogleFailure {
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
        let GoogleResult::Failed(failure) =
            google_generate_with(&request(), &config(Some(credential), None), &mut transport)
        else {
            panic!("server error must fail");
        };
        let refusal = refusal_for(&LaneOutcome::GoogleFailure(failure), "google", None);
        assert!(!refusal.detail.contains(credential));
        assert_eq!(transport.api_keys, [credential]);
    }

    #[test]
    fn non_context_window_http_error_body_reaches_refusal_detail() {
        let body = r#"{"error":{"message":"invalid temperature distinctive-400-google"}}"#;
        let mut transport = StubTransport {
            responses: vec![Ok(HttpResponse {
                status: 400,
                body: body.to_owned(),
            })],
            ..Default::default()
        };
        let GoogleResult::Failed(failure) = google_generate_with(
            &request(),
            &config(Some("configured-secret"), None),
            &mut transport,
        ) else {
            panic!("400 must fail");
        };
        assert_eq!(failure.detail.as_deref(), Some(body));
        let refusal = refusal_for(&LaneOutcome::GoogleFailure(failure), "google", None);
        assert_eq!(refusal.detail, body);
        assert!(!refusal.detail.contains("fixture"));
        assert_ne!(refusal.detail, crate::refusal::LIVE_PROVIDER_FAILURE_DETAIL);
    }

    #[test]
    fn blank_extracted_text_keeps_distinctive_raw_snippet_on_refusal() {
        let mut body = successful_body();
        body["candidates"] = json!([{
            "content": {"parts": [{"text": ""}]},
            "finishReason": "STOP",
        }]);
        body["distinctive"] = json!("blank-visible-google-xyz");
        let generated = parsed(body);
        assert!(generated.text.trim().is_empty());
        let snippet = generated
            .raw_response_snippet
            .as_deref()
            .expect("raw snippet");
        assert!(snippet.contains("blank-visible-google-xyz"));
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
            "google",
            None,
        );
        assert!(refusal.detail.contains("blank-visible-google-xyz"));
        assert!(!refusal.detail.contains("fixture"));
        let _ = fs::remove_dir_all(journal);
    }

    /// Gemini refuses `additionalProperties` inside a function declaration.
    ///
    /// Its declarations take a restricted OpenAPI 3.0 Schema subset; sending the key
    /// yields HTTP 400 INVALID_ARGUMENT, one violation per declaration. Verified
    /// against the live endpoint 2026-08-10, where it blocked every cogitate talent
    /// on Google before a single token was spent. Anthropic and OpenAI accept the
    /// same schema, so the narrowing belongs in this arm.
    #[test]
    fn converse_tool_schemas_omit_keys_gemini_rejects() {
        let tools = vec![ConverseToolSpec {
            name: "read_file".to_owned(),
            description: "read".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "nested": {
                        "type": "object",
                        "properties": {"deep": {"type": "string"}},
                        "additionalProperties": false
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        }];
        let body = converse_request_body(&request(), &[], &tools);
        assert!(
            !body.to_string().contains("additionalProperties"),
            "gemini rejects additionalProperties anywhere in a declaration"
        );
        let declared = &body["tools"][0]["functionDeclarations"][0]["parameters"];
        assert_eq!(declared["properties"]["path"]["type"], "string");
        assert_eq!(
            declared["properties"]["nested"]["properties"]["deep"]["type"],
            "string"
        );
        assert_eq!(declared["required"][0], "path");
    }

    /// Gemini assigns no call id; requiring one refused every real tool call.
    ///
    /// A live response is `{"name": ..., "args": {...}}` with no `id` -- verified
    /// 2026-08-10. Every other fixture in this file supplies one, so the arm's own
    /// tests measured a shape the provider never sends.
    #[test]
    fn converse_synthesizes_a_call_id_when_gemini_omits_one() {
        let offered = ["weather".to_owned()].into_iter().collect();
        let GoogleConverseResult::Turn(turn) = parse_converse_response(
            &json!({
                "modelVersion":"gemini",
                "candidates":[{"finishReason":"STOP","content":{"parts":[
                    {"functionCall":{"name":"weather","args":{"city":"Denver"}}},
                    {"functionCall":{"name":"weather","args":{"city":"Boulder"}}}
                ]}}]
            })
            .to_string(),
            &offered,
        ) else {
            panic!("an id-less gemini tool call must parse")
        };
        assert_eq!(turn.tool_calls.len(), 2);
        assert_eq!(turn.tool_calls[0].name, "weather");
        assert_eq!(turn.tool_calls[0].arguments["city"], "Denver");
        assert_ne!(
            turn.tool_calls[0].id, turn.tool_calls[1].id,
            "parallel calls to one tool need distinct ids to correlate results"
        );
        assert!(!turn.tool_calls[0].id.is_empty());
    }

    #[test]
    fn converse_body_and_tool_turns_follow_gemini_shapes() {
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
        let body = converse_request_body(&request(), &messages, &tools);
        assert_eq!(
            crate::converse::canonical_json(&body),
            crate::converse::canonical_json(&json!({
                "contents":[
                    {"role":"user","parts":[{"text":"ask"}]},
                    {"role":"model","parts":[{"text":"working"},{"functionCall":{"id":"call-1","name":"weather","args":{"city":"Denver"}}}]},
                    {"role":"user","parts":[{"functionResponse":{"id":"call-1","name":"weather","response":{"result":"sunny"}}}]}
                ],
                "tools":[{"functionDeclarations":[{"name":"weather","description":"weather","parameters":{"type":"object"}}]}],
                "generationConfig":{"temperature":0.3,"maxOutputTokens":4000},
                "systemInstruction":{"parts":[{"text":"system"}]}
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
                &tools
            ))
        );

        let offered = ["weather".to_owned()].into_iter().collect();
        let GoogleConverseResult::Turn(turn) = parse_converse_response(&json!({
            "modelVersion":"gemini", "usageMetadata":{"promptTokenCount":2,"candidatesTokenCount":3,"totalTokenCount":5,"thoughtsTokenCount":1},
            "candidates":[{"finishReason":"STOP","content":{"parts":[{"text":"before"},{"functionCall":{"id":"call-1","name":"weather","args":{"city":"Denver"}}}]}}]
        }).to_string(), &offered) else { panic!("tool turn expected") };
        assert_eq!(turn.finish_reason, "tool_calls");
        assert_eq!(turn.text, "before");
        assert!(!turn.tool_calls[0].not_offered);
        assert_eq!(
            turn.usage,
            json!({"input_tokens":2,"output_tokens":3,"total_tokens":5,"reasoning_tokens":1,"model_version":"gemini"})
        );
        let GoogleResult::Generated(generated) = parse_response(
            &json!({
                "modelVersion":"gemini", "usageMetadata":{"promptTokenCount":2,"candidatesTokenCount":3,"totalTokenCount":5,"thoughtsTokenCount":1},
                "candidates":[{"finishReason":"STOP","content":{"parts":[{"text":"before"},{"functionCall":{"id":"call-1","name":"weather","args":{"city":"Denver"}}}]}}]
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
        let GoogleConverseResult::Turn(zero_input) = parse_converse_response(
            &json!({
                "modelVersion":"gemini","usageMetadata":{"promptTokenCount":0,"candidatesTokenCount":1},"candidates":[{"finishReason":"STOP","content":{"parts":[]}}]
            })
            .to_string(),
            &offered,
        ) else {
            panic!("zero input turn expected")
        };
        let GoogleConverseResult::Turn(absent_input) = parse_converse_response(
            &json!({
                "modelVersion":"gemini","usageMetadata":{"candidatesTokenCount":1},"candidates":[{"finishReason":"STOP","content":{"parts":[]}}]
            })
            .to_string(),
            &offered,
        ) else {
            panic!("absent input turn expected")
        };
        assert_eq!(zero_input.usage["input_tokens"], 0);
        assert!(absent_input.usage.get("input_tokens").is_none());
        let GoogleConverseResult::Turn(unoffered) = parse_converse_response(&json!({
            "modelVersion":"gemini","candidates":[{"finishReason":"STOP","content":{"parts":[{"functionCall":{"id":"c","name":"other","args":{}}}]}}]
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
        let GoogleConverseResult::Failed(invalid) = parse_converse_response(&json!({
            "modelVersion":"gemini","candidates":[{"finishReason":"STOP","content":{"parts":[{"functionCall":{"id":"c","name":"weather","args":[]}}]}}]
        }).to_string(), &offered) else { panic!("invalid expected") };
        assert_eq!(invalid.reason_code, "tool_call_arguments_invalid");
        assert!(invalid.retryable && !invalid.blocking);
        let GoogleConverseResult::Failed(missing) = parse_converse_response(&json!({
            "modelVersion":"gemini","candidates":[{"finishReason":"STOP","content":{"parts":[{"functionCall":{}}]}}]
        }).to_string(), &offered) else { panic!("missing calls expected") };
        assert_eq!(missing.reason_code, "tool_call_arguments_invalid");
        let GoogleConverseResult::Failed(mixed_malformed) = parse_converse_response(
            &json!({
                "modelVersion":"gemini","candidates":[{"finishReason":"STOP","content":{"parts":[
                    {"functionCall":{"id":"valid","name":"weather","args":{}}},
                    {"functionCall":{"id":"malformed","args":{}}}
                ]}}]
            })
            .to_string(),
            &offered,
        ) else {
            panic!("mixed malformed calls must fail")
        };
        assert_eq!(mixed_malformed.reason_code, "tool_call_arguments_invalid");
        let GoogleConverseResult::Failed(malformed) = parse_converse_response(&json!({
            "modelVersion":"gemini","candidates":[{"finishReason":"MALFORMED_FUNCTION_CALL","content":{"parts":[]}}]
        }).to_string(), &offered) else { panic!("invalid expected") };
        assert_eq!(malformed.reason_code, "tool_call_arguments_invalid");
        let GoogleConverseResult::Turn(truncated) = parse_converse_response(&json!({
            "modelVersion":"gemini","candidates":[{"finishReason":"MAX_TOKENS","content":{"parts":[{"functionCall":{"id":"c","name":"weather","args":{}}}]}}]
        }).to_string(), &offered) else { panic!("truncated expected") };
        assert_eq!(truncated.finish_reason, "max_tokens");
        assert!(truncated.tool_calls.is_empty());
        let GoogleConverseResult::Turn(text_only) = parse_converse_response(&json!({
            "modelVersion":"gemini","candidates":[{"finishReason":"STOP","content":{"parts":[{"text":"plain text"}]}}]
        }).to_string(), &offered) else { panic!("text turn expected") };
        assert_eq!(text_only.text, "plain text");
        assert!(text_only.tool_calls.is_empty());
    }

    #[test]
    fn converse_http_failures_reuse_google_classification() {
        let mut transport = StubTransport {
            responses: vec![Ok(HttpResponse {
                status: 429,
                body: "{}".into(),
            })],
            ..Default::default()
        };
        let GoogleConverseResult::Failed(failure) = google_converse_with(
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
