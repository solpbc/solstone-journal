// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! OpenAI Responses API generation.

use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde_json::{Map, Value, json};
use solstone_core_generate::{ContentPart, GenerateRequest};
use solstone_core_local::HttpResponse;

use crate::endpoint::EndpointTransportError;
use crate::schema_prep::prepare_provider_schema;
use crate::token_budget::generate_token_budget;

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
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenAiGenerated {
    pub text: String,
    pub model: String,
    pub usage: Value,
    pub finish_reason: String,
    pub thinking: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OpenAiResult {
    Generated(OpenAiGenerated),
    Failed(OpenAiFailure),
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

fn openai_generate_with<T: OpenAiTransport>(
    request: &GenerateRequest,
    config: &Map<String, Value>,
    transport: &mut T,
) -> OpenAiResult {
    let Some(api_key) = configured_api_key(config) else {
        return failure("provider_key_missing");
    };
    let model = configured_model(config);
    let body = request_body(request, &model);
    let response = match transport.post_json(
        OPENAI_BASE_URL,
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
        return failure(classify_http_failure(response.status, &response.body));
    }
    parse_response(&response.body)
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

fn parse_response(body: &str) -> OpenAiResult {
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
            if let Some(value) = block.get("output_text") {
                let Some(value) = value.as_str() else {
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
    })
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

fn failure(reason_code: &str) -> OpenAiResult {
    OpenAiResult::Failed(OpenAiFailure {
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
    use std::fs;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use solstone_core_generate::{ContentPart, ReasonCodeValue};

    use super::*;
    use crate::{LaneOutcome, ProviderResultView, assess_provider_result, refusal_for};

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
            "output": [{"content": [{"output_text": "done"}]}],
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
        generated(parse_response(&body.to_string()))
    }

    fn temp_journal() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "solstone-openai-wire-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
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
            {"content": [{"output_text": "first "}, {"output_text": "second"}]},
            {"content": [{"output_text": " third"}]},
        ]);
        assert_eq!(parsed(body).text, "first second third");
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
        if std::env::var_os("SOLSTONE_OPENAI_PROCESS_ENV_CHILD").is_none() {
            let status = Command::new(std::env::current_exe().unwrap())
                .arg("--exact")
                .arg("openai::tests::process_environment_key_is_ignored_when_config_key_is_absent")
                .env("SOLSTONE_OPENAI_PROCESS_ENV_CHILD", "1")
                .env(OPENAI_API_KEY_ENV, "process-only-secret")
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }
        let mut transport = StubTransport::default();
        let result = openai_generate_with(&request(), &config(None, None), &mut transport);
        assert_eq!(
            result,
            OpenAiResult::Failed(OpenAiFailure {
                reason_code: Some("provider_key_missing".into())
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
                    reason_code: Some("provider_key_missing".into())
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
