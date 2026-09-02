// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Anthropic Messages API generation.

use std::{collections::BTreeSet, time::Duration};

use serde_json::{Map, Value, json};
use solstone_core_generate::{ContentPart, GenerateRequest};
use solstone_core_local::HttpResponse;

use crate::endpoint::EndpointTransportError;
use crate::token_budget::generate_token_budget;
use crate::{
    ConverseFailure, ConverseMessage, ConverseToolCall, ConverseToolSpec, ConverseTurn,
    NON_RESPONSIVE_RAW_OUTPUT_CAP_CHARS,
};

const ANTHROPIC_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
const ANTHROPIC_MESSAGES_PATH: &str = "/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MODEL: &str = "claude-sonnet-4-6";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const CONTEXT_WINDOW_PATTERNS: &[&str] = &[
    "prompt is too long",
    "maximum context length",
    "context window",
    "context length",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnthropicFailure {
    pub reason_code: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AnthropicGenerated {
    pub text: String,
    pub model: String,
    pub usage: Value,
    pub finish_reason: String,
    pub thinking: Option<Value>,
    pub raw_response_snippet: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AnthropicResult {
    Generated(AnthropicGenerated),
    Failed(AnthropicFailure),
}

pub type AnthropicTurn = ConverseTurn;
pub type AnthropicConverseFailure = ConverseFailure;

#[derive(Debug, Clone, PartialEq)]
pub enum AnthropicConverseResult {
    Turn(Box<AnthropicTurn>),
    Failed(AnthropicConverseFailure),
}

pub trait AnthropicTransport {
    fn post_json(
        &mut self,
        base_url: &str,
        path: &str,
        body: &Value,
        api_key: &str,
        anthropic_version: &str,
        timeout: Duration,
    ) -> Result<HttpResponse, EndpointTransportError>;
}

#[derive(Default)]
pub struct UreqAnthropicTransport;

impl AnthropicTransport for UreqAnthropicTransport {
    fn post_json(
        &mut self,
        base_url: &str,
        path: &str,
        body: &Value,
        api_key: &str,
        anthropic_version: &str,
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
            .header("x-api-key", api_key)
            .header("anthropic-version", anthropic_version)
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

pub fn anthropic_generate(
    request: &GenerateRequest,
    config: &Map<String, Value>,
) -> AnthropicResult {
    let mut transport = UreqAnthropicTransport;
    anthropic_generate_with(request, config, &mut transport)
}

pub fn anthropic_converse(
    request: &GenerateRequest,
    messages: &[ConverseMessage],
    tools: &[ConverseToolSpec],
    config: &Map<String, Value>,
) -> AnthropicConverseResult {
    let mut transport = UreqAnthropicTransport;
    anthropic_converse_with(request, messages, tools, config, &mut transport)
}

fn anthropic_converse_with<T: AnthropicTransport>(
    request: &GenerateRequest,
    messages: &[ConverseMessage],
    tools: &[ConverseToolSpec],
    config: &Map<String, Value>,
    transport: &mut T,
) -> AnthropicConverseResult {
    let Some(api_key) = configured_api_key(config) else {
        return converse_failure("provider_key_missing");
    };
    let model = configured_model(config);
    let base_url = crate::overrides::configured_base_url(config, ANTHROPIC_BASE_URL);
    let body = converse_request_body(request, messages, tools, &model);
    let response = match transport.post_json(
        &base_url,
        ANTHROPIC_MESSAGES_PATH,
        &body,
        &api_key,
        ANTHROPIC_VERSION,
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
        return AnthropicConverseResult::Failed(ConverseFailure {
            reason_code: reason_code.to_owned(),
            retryable,
            blocking,
            detail,
        });
    }
    let offered = tools.iter().map(|tool| tool.name.clone()).collect();
    parse_converse_response(&response.body, &offered)
}

fn anthropic_generate_with<T: AnthropicTransport>(
    request: &GenerateRequest,
    config: &Map<String, Value>,
    transport: &mut T,
) -> AnthropicResult {
    anthropic_generate_with_lookup(
        request,
        config,
        transport,
        crate::overrides::non_blank_process_env,
    )
}

fn anthropic_generate_with_lookup<T: AnthropicTransport>(
    request: &GenerateRequest,
    config: &Map<String, Value>,
    transport: &mut T,
    env: impl Fn(&str) -> Option<String>,
) -> AnthropicResult {
    let env = &env;
    let Some(api_key) =
        crate::overrides::configured_api_key_with(config, ANTHROPIC_API_KEY_ENV, env)
    else {
        return failure("provider_key_missing");
    };
    let model = crate::overrides::configured_model_with(config, DEFAULT_MODEL, env);
    let base_url = crate::overrides::configured_base_url_with(config, ANTHROPIC_BASE_URL, env);
    let body = request_body(request, &model);
    let response = match transport.post_json(
        &base_url,
        ANTHROPIC_MESSAGES_PATH,
        &body,
        &api_key,
        ANTHROPIC_VERSION,
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
        return AnthropicResult::Failed(AnthropicFailure {
            reason_code: Some(reason_code.to_owned()),
            detail,
        });
    }
    parse_response(&response.body, &api_key)
}

fn configured_api_key(config: &Map<String, Value>) -> Option<String> {
    crate::overrides::configured_api_key(config, ANTHROPIC_API_KEY_ENV)
}

fn configured_model(config: &Map<String, Value>) -> String {
    crate::overrides::configured_model(config, DEFAULT_MODEL)
}

fn request_body(request: &GenerateRequest, model: &str) -> Value {
    let content = request
        .contents
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } => json!({"type": "text", "text": text}),
            ContentPart::Image { mime_type, data } => {
                json!({"type": "image", "source": {"type": "base64", "media_type": mime_type, "data": data}})
            }
        })
        .collect::<Vec<_>>();
    let mut body = json!({
        "model": model,
        "max_tokens": generate_token_budget("anthropic", request.max_output_tokens, request.thinking_budget),
        "messages": [{"role": "user", "content": content}],
    });
    if let Some(system) = &request.system_instruction {
        body["system"] = Value::String(system.clone());
    }
    if let Some(budget) = request.thinking_budget.filter(|budget| *budget > 0) {
        body["thinking"] = json!({"type": "enabled", "budget_tokens": budget});
    } else if model_supports_temperature(model) {
        body["temperature"] = json!(request.temperature);
    }
    body
}

fn converse_request_body(
    request: &GenerateRequest,
    messages: &[ConverseMessage],
    tools: &[ConverseToolSpec],
    model: &str,
) -> Value {
    let messages = messages
        .iter()
        .map(|message| match message {
            ConverseMessage::User { text } => json!({
                "role": "user",
                "content": [{"type": "text", "text": text}],
            }),
            ConverseMessage::Assistant { text, tool_calls } => {
                let mut content = Vec::new();
                if !text.is_empty() {
                    content.push(json!({"type": "text", "text": text}));
                }
                content.extend(tool_calls.iter().map(|call| {
                    json!({"type": "tool_use", "id": call.id, "name": call.name, "input": call.arguments})
                }));
                json!({"role": "assistant", "content": content})
            }
            ConverseMessage::ToolResult {
                tool_call_id,
                tool_name: _,
                output,
            } => json!({
                "role": "user",
                "content": [{"type": "tool_result", "tool_use_id": tool_call_id, "content": output}],
            }),
        })
        .collect::<Vec<_>>();
    let mut body = json!({
        "model": model,
        "max_tokens": generate_token_budget("anthropic", request.max_output_tokens, request.thinking_budget),
        "messages": messages,
        "tools": tools.iter().map(|tool| json!({
            "name": tool.name,
            "description": tool.description,
            "input_schema": tool.parameters,
        })).collect::<Vec<_>>(),
    });
    if let Some(system) = &request.system_instruction {
        body["system"] = Value::String(system.clone());
    }
    if let Some(budget) = request.thinking_budget.filter(|budget| *budget > 0) {
        body["thinking"] = json!({"type": "enabled", "budget_tokens": budget});
    } else if model_supports_temperature(model) {
        body["temperature"] = json!(request.temperature);
    }
    body
}

// Models that reject the `temperature` parameter (Anthropic API error:
// "temperature is deprecated for this model"). This list is manually
// reconciled against the `model_tiers` catalog in
// solstone-core-thinking/src/providers.rs and must be revisited whenever
// that catalog changes.
fn model_supports_temperature(model: &str) -> bool {
    !matches!(model, "claude-opus-4-7" | "claude-sonnet-5")
}

fn request_timeout(timeout_s: Option<f64>) -> Duration {
    timeout_s
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .map(Duration::from_secs_f64)
        .unwrap_or(DEFAULT_TIMEOUT)
}

fn parse_response(body: &str, secret: &str) -> AnthropicResult {
    let raw_snippet = capture_provider_detail(body, secret);
    let Ok(body) = serde_json::from_str::<Value>(body) else {
        return failure("provider_response_invalid");
    };
    let Some(content) = body.get("content").and_then(Value::as_array) else {
        return failure("provider_response_invalid");
    };
    let Some(model) = body.get("model").and_then(Value::as_str) else {
        return failure("provider_response_invalid");
    };
    let Some(stop_reason) = body.get("stop_reason").and_then(Value::as_str) else {
        return failure("provider_response_invalid");
    };
    let Some(provider_usage) = body.get("usage").and_then(Value::as_object) else {
        return failure("provider_response_invalid");
    };
    let Some(_) = provider_usage.get("input_tokens").and_then(Value::as_u64) else {
        return failure("provider_response_invalid");
    };
    let Some(_) = provider_usage.get("output_tokens").and_then(Value::as_u64) else {
        return failure("provider_response_invalid");
    };

    let mut text = String::new();
    let mut thinking = None;
    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                let Some(value) = block.get("text").and_then(Value::as_str) else {
                    return failure("provider_response_invalid");
                };
                text.push_str(value);
            }
            Some("thinking") if thinking.is_none() => thinking = Some(block.clone()),
            _ => {}
        }
    }
    let usage = usage_from_provider(provider_usage);
    AnthropicResult::Generated(AnthropicGenerated {
        text,
        model: model.to_owned(),
        usage,
        finish_reason: match stop_reason {
            "end_turn" | "stop_sequence" => "stop".to_owned(),
            "max_tokens" => "max_tokens".to_owned(),
            other => other.to_owned(),
        },
        thinking,
        raw_response_snippet: raw_snippet,
    })
}

fn parse_converse_response(body: &str, offered: &BTreeSet<String>) -> AnthropicConverseResult {
    let Ok(body) = serde_json::from_str::<Value>(body) else {
        return converse_failure("provider_response_invalid");
    };
    let Some(content) = body.get("content").and_then(Value::as_array) else {
        return converse_failure("provider_response_invalid");
    };
    let Some(model) = body.get("model").and_then(Value::as_str) else {
        return converse_failure("provider_response_invalid");
    };
    let Some(stop_reason) = body.get("stop_reason").and_then(Value::as_str) else {
        return converse_failure("provider_response_invalid");
    };
    let Some(provider_usage) = body.get("usage").and_then(Value::as_object) else {
        return converse_failure("provider_response_invalid");
    };
    if provider_usage
        .get("input_tokens")
        .and_then(Value::as_u64)
        .is_none()
        || provider_usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .is_none()
    {
        return converse_failure("provider_response_invalid");
    }

    let mut text = String::new();
    let mut thinking = None;
    let mut tool_blocks = Vec::new();
    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                let Some(value) = block.get("text").and_then(Value::as_str) else {
                    return converse_failure("provider_response_invalid");
                };
                text.push_str(value);
            }
            Some("thinking") if thinking.is_none() => thinking = Some(block.clone()),
            Some("tool_use") => tool_blocks.push(block),
            _ => {}
        }
    }
    let usage = usage_from_provider(provider_usage);
    if stop_reason == "max_tokens" {
        return AnthropicConverseResult::Turn(Box::new(ConverseTurn {
            text,
            tool_calls: Vec::new(),
            finish_reason: "max_tokens".to_owned(),
            usage,
            model: model.to_owned(),
            thinking,
        }));
    }

    let mut tool_calls = Vec::new();
    for block in tool_blocks {
        let Some(id) = block
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            return converse_failure("tool_call_arguments_invalid");
        };
        let Some(name) = block
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
        else {
            return converse_failure("tool_call_arguments_invalid");
        };
        let Some(arguments) = block.get("input").filter(|value| value.is_object()) else {
            return converse_failure("tool_call_arguments_invalid");
        };
        tool_calls.push(ConverseToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments: arguments.clone(),
            not_offered: !offered.contains(name),
            thought_signature: None,
        });
    }
    if stop_reason == "tool_use" && tool_calls.is_empty() {
        return converse_failure("tool_calls_missing");
    }
    let finish_reason = if tool_calls.is_empty() {
        match stop_reason {
            "end_turn" | "stop_sequence" => "stop".to_owned(),
            other => other.to_owned(),
        }
    } else {
        "tool_calls".to_owned()
    };
    AnthropicConverseResult::Turn(Box::new(ConverseTurn {
        text,
        tool_calls,
        finish_reason,
        usage,
        model: model.to_owned(),
        thinking,
    }))
}

fn usage_from_provider(provider_usage: &Map<String, Value>) -> Value {
    let mut usage = Map::new();
    for (source, target) in [
        ("input_tokens", "input_tokens"),
        ("output_tokens", "output_tokens"),
        ("cache_creation_input_tokens", "cache_creation_tokens"),
        ("cache_read_input_tokens", "cached_input_tokens"),
    ] {
        if let Some(tokens) = provider_usage.get(source).and_then(Value::as_u64) {
            usage.insert(target.into(), Value::from(tokens));
        }
    }
    if let Some(tokens) = provider_usage
        .get("output_tokens_details")
        .and_then(Value::as_object)
        .and_then(|details| details.get("thinking_tokens"))
        .and_then(Value::as_u64)
        .filter(|tokens| *tokens != 0)
    {
        usage.insert("reasoning_tokens".into(), Value::from(tokens));
    }
    Value::Object(usage)
}

fn classify_http_failure(status: u16, body: &str) -> &'static str {
    match status {
        401 => "provider_key_invalid",
        429 => "provider_quota_exceeded",
        400 if is_context_window_error(body) => "context_window_exceeded",
        400 => "provider_response_invalid",
        500..=599 => "provider_unavailable",
        _ => "provider_response_invalid",
    }
}

// This is intentionally a best-effort wording heuristic, matching the BYO endpoint arm's
// context-window handling. The provider's exact messages are not a stable wire contract.
fn is_context_window_error(body: &str) -> bool {
    let Ok(body) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    body.get("error")
        .and_then(Value::as_object)
        .filter(|error| error.get("type").and_then(Value::as_str) == Some("invalid_request_error"))
        .and_then(|error| error.get("message").and_then(Value::as_str))
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

fn failure(reason_code: &str) -> AnthropicResult {
    AnthropicResult::Failed(AnthropicFailure {
        reason_code: Some(reason_code.to_owned()),
        detail: None,
    })
}

fn converse_failure(reason_code: &str) -> AnthropicConverseResult {
    let (retryable, blocking) = crate::converse::converse_failure_flags(reason_code);
    AnthropicConverseResult::Failed(ConverseFailure {
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

    use solstone_core_generate::{
        ContentPart, GenerateResponse, ReasonCodeValue, encode_one_shot_response,
    };

    use super::*;
    use crate::{
        LaneOutcome, ProviderResultView, ValidationFailure, assess_provider_result, refusal_for,
    };

    #[derive(Default)]
    struct StubTransport {
        responses: Vec<Result<HttpResponse, EndpointTransportError>>,
        posts: Vec<Value>,
        api_keys: Vec<String>,
        versions: Vec<String>,
    }

    impl AnthropicTransport for StubTransport {
        fn post_json(
            &mut self,
            _base_url: &str,
            _path: &str,
            body: &Value,
            api_key: &str,
            anthropic_version: &str,
            _timeout: Duration,
        ) -> Result<HttpResponse, EndpointTransportError> {
            self.posts.push(body.clone());
            self.api_keys.push(api_key.to_owned());
            self.versions.push(anthropic_version.to_owned());
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
            env.insert(ANTHROPIC_API_KEY_ENV.into(), Value::String(key.into()));
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

    fn success_response() -> HttpResponse {
        HttpResponse {
            status: 200,
            body: json!({
                "content": [{"type": "text", "text": "done"}],
                "model": "claude-response-model",
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": 12,
                    "output_tokens": 34,
                    "output_tokens_details": {"thinking_tokens": 8}
                }
            })
            .to_string(),
        }
    }

    fn generated(result: AnthropicResult) -> AnthropicGenerated {
        match result {
            AnthropicResult::Generated(success) => success,
            AnthropicResult::Failed(failure) => panic!("unexpected failure: {failure:?}"),
        }
    }

    #[test]
    fn process_env_key_is_ignored_when_config_key_is_absent() {
        let mut transport = StubTransport::default();
        let result = anthropic_generate_with_lookup(
            &request(),
            &config(None, None),
            &mut transport,
            crate::overrides::lookup_leaks_conventional_keys,
        );
        assert_eq!(
            result,
            AnthropicResult::Failed(AnthropicFailure {
                reason_code: Some("provider_key_missing".into()),
                detail: None,
            })
        );
        assert!(transport.posts.is_empty());
    }

    #[test]
    fn configured_key_is_sent_once_as_x_api_key() {
        let mut transport = StubTransport {
            responses: vec![Ok(success_response())],
            ..Default::default()
        };
        let _ = anthropic_generate_with(
            &request(),
            &config(Some("configured-secret"), None),
            &mut transport,
        );
        assert_eq!(transport.api_keys, ["configured-secret"]);
        assert_eq!(transport.versions, [ANTHROPIC_VERSION]);
    }

    #[test]
    fn override_key_is_transport_only_and_not_emitted() {
        let mut transport = StubTransport {
            responses: vec![Ok(success_response())],
            ..Default::default()
        };
        let generated = generated(anthropic_generate_with_lookup(
            &request(),
            &config(None, None),
            &mut transport,
            crate::overrides::lookup_api_key_override,
        ));
        assert_eq!(transport.api_keys, ["override-secret"]);
        assert!(
            !serde_json::to_vec(&transport.posts[0])
                .unwrap()
                .windows(b"override-secret".len())
                .any(|window| window == b"override-secret")
        );

        let journal = crate::validation::isolated_journal_dir("anthropic-override");
        let assessment = assess_provider_result(ProviderResultView {
            journal_path: &journal,
            context: "test.generate",
            model: &generated.model,
            text: &generated.text,
            finish_reason: &generated.finish_reason,
            usage: &generated.usage,
            json_output: false,
            enforce_responsiveness: false,
            raw_response_snippet: None,
        });
        assert!(assessment.token_log_error.is_none());
        let token_log = fs::read_to_string(
            fs::read_dir(journal.join("tokens"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path(),
        )
        .unwrap();
        assert!(!token_log.contains("override-secret"));

        let diagnostic = encode_one_shot_response(&GenerateResponse::Refused(refusal_for(
            &LaneOutcome::AnthropicFailure(AnthropicFailure {
                reason_code: Some("provider_response_invalid".into()),
                detail: None,
            }),
            "anthropic",
            Some("request".into()),
        )))
        .unwrap()
        .into_bytes();
        assert!(
            !diagnostic
                .windows(b"override-secret".len())
                .any(|window| window == b"override-secret")
        );
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn blank_configured_key_refuses_before_post() {
        let mut transport = StubTransport::default();
        let result =
            anthropic_generate_with(&request(), &config(Some("  \t"), None), &mut transport);
        let AnthropicResult::Failed(failure) = result else {
            panic!("blank key must fail");
        };
        assert_eq!(failure.reason_code.as_deref(), Some("provider_key_missing"));
        assert!(transport.posts.is_empty());
        let refusal = refusal_for(
            &LaneOutcome::AnthropicFailure(failure),
            "anthropic",
            Some("request".into()),
        );
        assert_eq!(refusal.detail, crate::refusal::LIVE_PROVIDER_FAILURE_DETAIL);
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
        let AnthropicResult::Failed(failure) =
            anthropic_generate_with(&request(), &config(Some(credential), None), &mut transport)
        else {
            panic!("server error must fail");
        };
        let refusal = refusal_for(&LaneOutcome::AnthropicFailure(failure), "anthropic", None);
        assert!(!refusal.detail.contains(credential));
    }

    #[test]
    fn non_context_window_http_error_body_reaches_refusal_detail() {
        let body = r#"{"error":{"message":"invalid temperature distinctive-400-anthropic"}}"#;
        let mut transport = StubTransport {
            responses: vec![Ok(HttpResponse {
                status: 400,
                body: body.to_owned(),
            })],
            ..Default::default()
        };
        let AnthropicResult::Failed(failure) = anthropic_generate_with(
            &request(),
            &config(Some("configured-secret"), None),
            &mut transport,
        ) else {
            panic!("400 must fail");
        };
        assert_eq!(failure.detail.as_deref(), Some(body));
        let refusal = refusal_for(&LaneOutcome::AnthropicFailure(failure), "anthropic", None);
        assert_eq!(refusal.detail, body);
        assert!(!refusal.detail.contains("fixture"));
        assert_ne!(refusal.detail, crate::refusal::LIVE_PROVIDER_FAILURE_DETAIL);
    }

    #[test]
    fn blank_extracted_text_keeps_distinctive_raw_snippet_on_refusal() {
        let body = json!({
            "content": [{"type": "text", "text": ""}],
            "model": "claude-response-model",
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 12, "output_tokens": 34},
            "distinctive": "blank-visible-anthropic-xyz",
        });
        let generated = generated(parse_response(&body.to_string(), ""));
        assert!(generated.text.trim().is_empty());
        let snippet = generated
            .raw_response_snippet
            .as_deref()
            .expect("raw snippet");
        assert!(snippet.contains("blank-visible-anthropic-xyz"));
        let journal = crate::validation::isolated_journal_dir("anthropic-blank");
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
            "anthropic",
            None,
        );
        assert!(refusal.detail.contains("blank-visible-anthropic-xyz"));
        assert!(!refusal.detail.contains("fixture"));
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn successful_response_extracts_text_model_finish_reason_and_usage() {
        let mut transport = StubTransport {
            responses: vec![Ok(success_response())],
            ..Default::default()
        };
        let success = generated(anthropic_generate_with(
            &request(),
            &config(Some("configured-secret"), None),
            &mut transport,
        ));
        assert_eq!(success.text, "done");
        assert_eq!(success.model, "claude-response-model");
        assert_eq!(success.finish_reason, "stop");
        assert_eq!(
            success.usage,
            json!({"input_tokens": 12, "output_tokens": 34, "reasoning_tokens": 8})
        );
    }

    #[test]
    fn thinking_enabled_omits_temperature_and_sends_thinking_body() {
        let mut request = request();
        request.thinking_budget = Some(500);
        let mut transport = StubTransport {
            responses: vec![Ok(success_response())],
            ..Default::default()
        };
        let _ = anthropic_generate_with(
            &request,
            &config(Some("configured-secret"), None),
            &mut transport,
        );
        let body = &transport.posts[0];
        assert_eq!(
            body["thinking"],
            json!({"type": "enabled", "budget_tokens": 500})
        );
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn thinking_disabled_respects_model_temperature_capability() {
        let mut no_temperature = StubTransport {
            responses: vec![Ok(success_response())],
            ..Default::default()
        };
        let _ = anthropic_generate_with(
            &request(),
            &config(Some("configured-secret"), Some("claude-opus-4-7")),
            &mut no_temperature,
        );
        assert!(no_temperature.posts[0].get("temperature").is_none());

        let mut no_temperature_sonnet_5 = StubTransport {
            responses: vec![Ok(success_response())],
            ..Default::default()
        };
        let _ = anthropic_generate_with(
            &request(),
            &config(Some("configured-secret"), Some("claude-sonnet-5")),
            &mut no_temperature_sonnet_5,
        );
        assert!(
            no_temperature_sonnet_5.posts[0]
                .get("temperature")
                .is_none()
        );

        let mut temperature = StubTransport {
            responses: vec![Ok(success_response())],
            ..Default::default()
        };
        let _ = anthropic_generate_with(
            &request(),
            &config(Some("configured-secret"), Some("claude-sonnet-4-6")),
            &mut temperature,
        );
        assert_eq!(temperature.posts[0]["temperature"], json!(0.3));
    }

    #[test]
    fn thinking_content_blocks_are_preserved_and_optional() {
        let with_thinking = parse_response(
            &json!({
                "content": [
                    {"type": "thinking", "thinking": "plan"},
                    {"type": "text", "text": "done"}
                ],
                "model": "claude-response-model",
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 1, "output_tokens": 2}
            })
            .to_string(),
            "",
        );
        assert_eq!(
            generated(with_thinking).thinking,
            Some(json!({"type": "thinking", "thinking": "plan"}))
        );
        assert!(
            generated(parse_response(&success_response().body, ""))
                .thinking
                .is_none()
        );
    }

    #[test]
    fn failures_map_to_fixture_reason_codes_and_blocking_values() {
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
                    body: json!({"error": {"type": "invalid_request_error", "message": "prompt is too long"}}).to_string(),
                }),
                "context_window_exceeded",
                false,
            ),
            (
                Err(EndpointTransportError::Connection),
                "network_unreachable",
                false,
            ),
            (
                Ok(HttpResponse {
                    status: 500,
                    body: "{}".into(),
                }),
                "provider_unavailable",
                true,
            ),
        ];
        for (response, expected_code, expected_blocking) in cases {
            let mut transport = StubTransport {
                responses: vec![response],
                ..Default::default()
            };
            let AnthropicResult::Failed(failure) = anthropic_generate_with(
                &request(),
                &config(Some("configured-secret"), None),
                &mut transport,
            ) else {
                panic!("case must fail");
            };
            assert_eq!(failure.reason_code.as_deref(), Some(expected_code));
            let refusal = refusal_for(&LaneOutcome::AnthropicFailure(failure), "anthropic", None);
            assert_eq!(
                refusal.reason_code.as_ref().map(ReasonCodeValue::as_wire),
                Some(expected_code)
            );
            assert_eq!(refusal.blocking, expected_blocking);
        }
    }

    #[test]
    fn converse_body_and_tool_turns_follow_anthropic_shapes() {
        let messages = vec![
            ConverseMessage::User { text: "ask".into() },
            ConverseMessage::Assistant {
                text: "working".into(),
                tool_calls: vec![ConverseToolCall {
                    id: "call-1".into(),
                    name: "weather".into(),
                    arguments: json!({"city": "Denver"}),
                    not_offered: false,
                    thought_signature: None,
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
            parameters: json!({"type": "object"}),
        }];
        let body = converse_request_body(&request(), &messages, &tools, "model");
        assert_eq!(
            crate::converse::canonical_json(&body),
            crate::converse::canonical_json(&json!({
                "model": "model", "max_tokens": 4000, "temperature": 0.3,
                "system": "system", "tools": [{"name": "weather", "description": "weather", "input_schema": {"type":"object"}}],
                "messages": [
                    {"role":"user","content":[{"type":"text","text":"ask"}]},
                    {"role":"assistant","content":[{"type":"text","text":"working"},{"type":"tool_use","id":"call-1","name":"weather","input":{"city":"Denver"}}]},
                    {"role":"user","content":[{"type":"tool_result","tool_use_id":"call-1","content":"sunny"}]}
                ]
            }))
        );
        let mut without_tools = body.clone();
        without_tools.as_object_mut().unwrap().remove("tools");
        assert_ne!(
            crate::converse::canonical_json(&body),
            crate::converse::canonical_json(&without_tools)
        );
        let swapped = converse_request_body(
            &request(),
            &messages.into_iter().rev().collect::<Vec<_>>(),
            &tools,
            "model",
        );
        assert_ne!(
            crate::converse::canonical_json(&body),
            crate::converse::canonical_json(&swapped)
        );

        let offered = ["weather".to_owned()].into_iter().collect();
        let turn = parse_converse_response(&json!({
            "model":"model", "stop_reason":"tool_use",
            "usage":{"input_tokens":2,"output_tokens":3,"cache_creation_input_tokens":5,"cache_read_input_tokens":7,"output_tokens_details":{"thinking_tokens":11}},
            "content":[{"type":"text","text":"before"},{"type":"tool_use","id":"call-1","name":"weather","input":{"city":"Denver"}}]
        }).to_string(), &offered);
        let AnthropicConverseResult::Turn(turn) = turn else {
            panic!("tool turn expected")
        };
        assert_eq!(turn.text, "before");
        assert_eq!(turn.finish_reason, "tool_calls");
        assert!(!turn.tool_calls[0].not_offered);
        assert_eq!(
            turn.usage,
            json!({"input_tokens":2,"output_tokens":3,"cache_creation_tokens":5,"cached_input_tokens":7,"reasoning_tokens":11})
        );
        assert_eq!(
            crate::validation::sanitize_finish_reason(&turn.finish_reason),
            crate::SanitizedFinishReason::ToolCalls
        );
        let AnthropicResult::Generated(generated) = parse_response(
            &json!({
                "model":"model", "stop_reason":"tool_use",
                "usage":{"input_tokens":2,"output_tokens":3,"cache_creation_input_tokens":5,"cache_read_input_tokens":7,"output_tokens_details":{"thinking_tokens":11}},
                "content":[{"type":"text","text":"before"},{"type":"tool_use","id":"call-1","name":"weather","input":{"city":"Denver"}}]
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
        let cache_zero = usage_from_provider(
            json!({"input_tokens":1,"output_tokens":1,"cache_creation_input_tokens":0})
                .as_object()
                .unwrap(),
        );
        let cache_absent = usage_from_provider(
            json!({"input_tokens":1,"output_tokens":1})
                .as_object()
                .unwrap(),
        );
        assert_eq!(cache_zero["cache_creation_tokens"], 0);
        assert!(cache_absent.get("cache_creation_tokens").is_none());

        let unoffered = parse_converse_response(&json!({
            "model":"model", "stop_reason":"tool_use", "usage":{"input_tokens":0,"output_tokens":0},
            "content":[{"type":"tool_use","id":"call-2","name":"other","input":{}}]
        }).to_string(), &offered);
        let AnthropicConverseResult::Turn(unoffered) = unoffered else {
            panic!("tool turn expected")
        };
        assert!(unoffered.tool_calls[0].not_offered);
        let journal = std::env::temp_dir();
        let assessment = assess_provider_result(ProviderResultView {
            journal_path: &journal,
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

        for (body, code) in [
            (
                json!({"model":"model","stop_reason":"tool_use","usage":{"input_tokens":0,"output_tokens":0},"content":[]}),
                "tool_calls_missing",
            ),
            (
                json!({"model":"model","stop_reason":"tool_use","usage":{"input_tokens":0,"output_tokens":0},"content":[{"type":"tool_use","id":"call","name":"weather","input":[]}] }),
                "tool_call_arguments_invalid",
            ),
        ] {
            let AnthropicConverseResult::Failed(failure) =
                parse_converse_response(&body.to_string(), &offered)
            else {
                panic!("failure expected")
            };
            assert_eq!(failure.reason_code, code);
            assert!(failure.retryable && !failure.blocking);
        }
        let AnthropicConverseResult::Turn(truncated) = parse_converse_response(&json!({
            "model":"model","stop_reason":"max_tokens","usage":{"input_tokens":0,"output_tokens":0},
            "content":[{"type":"tool_use","id":"partial","name":"weather","input":{}}]
        }).to_string(), &offered) else { panic!("truncated turn expected") };
        assert_eq!(truncated.finish_reason, "max_tokens");
        assert!(truncated.tool_calls.is_empty());
        let AnthropicConverseResult::Turn(text_only) = parse_converse_response(&json!({
            "model":"model","stop_reason":"end_turn","usage":{"input_tokens":0,"output_tokens":0},
            "content":[{"type":"text","text":"plain text"}]
        }).to_string(), &offered) else { panic!("text turn expected") };
        assert_eq!(text_only.text, "plain text");
        assert!(text_only.tool_calls.is_empty());
    }

    #[test]
    fn converse_http_failures_reuse_anthropic_classification() {
        let mut transport = StubTransport {
            responses: vec![Ok(HttpResponse {
                status: 429,
                body: "{}".into(),
            })],
            ..Default::default()
        };
        let AnthropicConverseResult::Failed(failure) = anthropic_converse_with(
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
