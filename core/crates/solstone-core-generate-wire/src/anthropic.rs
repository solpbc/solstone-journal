// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Anthropic Messages API generation.

use std::time::Duration;

use serde_json::{Map, Value, json};
use solstone_core_generate::{ContentPart, GenerateRequest};
use solstone_core_local::HttpResponse;

use crate::endpoint::EndpointTransportError;
use crate::token_budget::generate_token_budget;

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
}

#[derive(Debug, Clone, PartialEq)]
pub struct AnthropicGenerated {
    pub text: String,
    pub model: String,
    pub usage: Value,
    pub finish_reason: String,
    pub thinking: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AnthropicResult {
    Generated(AnthropicGenerated),
    Failed(AnthropicFailure),
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

fn anthropic_generate_with<T: AnthropicTransport>(
    request: &GenerateRequest,
    config: &Map<String, Value>,
    transport: &mut T,
) -> AnthropicResult {
    let Some(api_key) = configured_api_key(config) else {
        return failure("provider_key_missing");
    };
    let model = configured_model(config);
    let body = request_body(request, &model);
    let response = match transport.post_json(
        ANTHROPIC_BASE_URL,
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
        return failure(classify_http_failure(response.status, &response.body));
    }
    parse_response(&response.body)
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

fn model_supports_temperature(model: &str) -> bool {
    model != "claude-opus-4-7"
}

fn request_timeout(timeout_s: Option<f64>) -> Duration {
    timeout_s
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .map(Duration::from_secs_f64)
        .unwrap_or(DEFAULT_TIMEOUT)
}

fn parse_response(body: &str) -> AnthropicResult {
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
    let Some(input_tokens) = provider_usage.get("input_tokens").and_then(Value::as_u64) else {
        return failure("provider_response_invalid");
    };
    let Some(output_tokens) = provider_usage.get("output_tokens").and_then(Value::as_u64) else {
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
    let mut usage = Map::new();
    usage.insert("input_tokens".into(), Value::from(input_tokens));
    usage.insert("output_tokens".into(), Value::from(output_tokens));
    if let Some(tokens) = provider_usage
        .get("output_tokens_details")
        .and_then(Value::as_object)
        .and_then(|details| details.get("thinking_tokens"))
        .and_then(Value::as_u64)
        .filter(|tokens| *tokens != 0)
    {
        usage.insert("reasoning_tokens".into(), Value::from(tokens));
    }
    AnthropicResult::Generated(AnthropicGenerated {
        text,
        model: model.to_owned(),
        usage: Value::Object(usage),
        finish_reason: match stop_reason {
            "end_turn" | "stop_sequence" => "stop".to_owned(),
            "max_tokens" => "max_tokens".to_owned(),
            other => other.to_owned(),
        },
        thinking,
    })
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

fn failure(reason_code: &str) -> AnthropicResult {
    AnthropicResult::Failed(AnthropicFailure {
        reason_code: Some(reason_code.to_owned()),
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
    use std::{
        fs,
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    use solstone_core_generate::{
        ContentPart, GenerateResponse, ReasonCodeValue, encode_one_shot_response,
    };

    use super::*;
    use crate::{LaneOutcome, ProviderResultView, assess_provider_result, refusal_for};

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
        if std::env::var_os("SOLSTONE_ANTHROPIC_PROCESS_ENV_CHILD").is_none() {
            let status = Command::new(std::env::current_exe().unwrap())
                .arg("--exact")
                .arg("anthropic::tests::process_env_key_is_ignored_when_config_key_is_absent")
                .env("SOLSTONE_ANTHROPIC_PROCESS_ENV_CHILD", "1")
                .env(ANTHROPIC_API_KEY_ENV, "process-only-secret")
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }
        // The child process has a real API-key environment value, but the arm resolves only config.
        let mut transport = StubTransport::default();
        let result = anthropic_generate_with(&request(), &config(None, None), &mut transport);
        assert_eq!(
            result,
            AnthropicResult::Failed(AnthropicFailure {
                reason_code: Some("provider_key_missing".into())
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
        if std::env::var_os("SOLSTONE_ANTHROPIC_OVERRIDE_CHILD").is_none() {
            let status = Command::new(std::env::current_exe().unwrap())
                .arg("--exact")
                .arg("anthropic::tests::override_key_is_transport_only_and_not_emitted")
                .env("SOLSTONE_ANTHROPIC_OVERRIDE_CHILD", "1")
                .env(crate::overrides::API_KEY_OVERRIDE_ENV, "override-secret")
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }

        let mut transport = StubTransport {
            responses: vec![Ok(success_response())],
            ..Default::default()
        };
        let generated = generated(anthropic_generate_with(
            &request(),
            &config(None, None),
            &mut transport,
        ));
        assert_eq!(transport.api_keys, ["override-secret"]);
        assert!(
            !serde_json::to_vec(&transport.posts[0])
                .unwrap()
                .windows(b"override-secret".len())
                .any(|window| window == b"override-secret")
        );

        let journal = std::env::temp_dir().join(format!(
            "solstone-anthropic-override-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let assessment = assess_provider_result(ProviderResultView {
            journal_path: &journal,
            context: "test.generate",
            model: &generated.model,
            text: &generated.text,
            finish_reason: &generated.finish_reason,
            usage: &generated.usage,
            json_output: false,
            enforce_responsiveness: false,
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
    fn blank_configured_key_refuses_with_fixture_detail_before_post() {
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
        assert_eq!(refusal.detail, "fixture provider-response-invalid");
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
        );
        assert_eq!(
            generated(with_thinking).thinking,
            Some(json!({"type": "thinking", "thinking": "plan"}))
        );
        assert!(
            generated(parse_response(&success_response().body))
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
}
