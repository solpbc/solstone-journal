// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Google Gemini generateContent generation.

use std::time::Duration;

use serde_json::{Map, Value, json};
use solstone_core_generate::{ContentPart, GenerateRequest};
use solstone_core_local::HttpResponse;

use crate::NON_RESPONSIVE_RAW_OUTPUT_CAP_CHARS;
use crate::endpoint::EndpointTransportError;
use crate::schema_prep::prepare_provider_schema;
use crate::thinking::{
    GoogleThinkingControl, Thinking, byo_thinking, google_budgets, google_ceiling, google_levels,
    google_refused_thinking, google_rejected_thinking_level,
    google_rejected_thinking_level_control,
};

const GOOGLE_API_KEY_ENV: &str = "GOOGLE_API_KEY";
const GOOGLE_BASE_URL: &str = "https://generativelanguage.googleapis.com";
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
    "exceeds the maximum number of tokens allowed",
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
    let Some(model) = crate::overrides::configured_model_with(config, env) else {
        return failure("model_missing");
    };
    let base_url = crate::overrides::configured_base_url_with(config, GOOGLE_BASE_URL, env);
    let path = format!("/v1beta/models/{model}:generateContent");
    let thinking = byo_thinking(config);
    let levels = google_levels(thinking);
    let budgets = google_budgets(thinking);
    let timeout = request_timeout(request.timeout_s, crate::thinking::cloud_room(thinking));
    enum Phase {
        Level(usize),
        Budget(usize),
    }
    let mut phase = Phase::Level(0);
    let response = loop {
        let control = match phase {
            Phase::Level(i) => GoogleThinkingControl::Level(levels[i]),
            Phase::Budget(i) => GoogleThinkingControl::Budget(budgets[i]),
        };
        let body = request_body(request, &model, thinking, control);
        let response = match transport.post_json(&base_url, &path, &body, &api_key, timeout) {
            Ok(response) => response,
            Err(EndpointTransportError::Connection) => return failure("network_unreachable"),
            Err(EndpointTransportError::Capacity) => return failure("provider_unavailable"),
            Err(EndpointTransportError::ClosedBeforeResponse) => {
                return failure("provider_response_invalid");
            }
            Err(EndpointTransportError::StatusExpired | EndpointTransportError::Other) => {
                return failure("provider_response_invalid");
            }
        };
        // A level value rejection tries the next level. A level-control refusal drops the
        // remaining levels and starts `google_budgets` at its first value. The bare invalid-argument
        // sentence steps only on the budget ladder.
        match phase {
            Phase::Level(i) => {
                if google_rejected_thinking_level(response.status, &response.body, levels[i]) {
                    if i + 1 < levels.len() {
                        phase = Phase::Level(i + 1);
                        continue;
                    } else {
                        break response;
                    }
                }
                if google_rejected_thinking_level_control(response.status, &response.body) {
                    phase = Phase::Budget(0);
                    continue;
                }
                break response;
            }
            Phase::Budget(i) => {
                let is_schema_or_content = serde_json::from_str::<Value>(&response.body)
                    .ok()
                    .and_then(|v| {
                        v.pointer("/error/message")
                            .and_then(Value::as_str)
                            .map(|msg| {
                                let msg = msg.to_ascii_lowercase();
                                msg.contains("schema") || msg.contains("content")
                            })
                    })
                    .unwrap_or(false);
                if is_schema_or_content || response.body.contains("API_KEY_INVALID") {
                    break response;
                }
                if i + 1 < budgets.len()
                    && google_refused_thinking(
                        response.status,
                        &response.body,
                        is_context_window_error(&response.body),
                    )
                {
                    phase = Phase::Budget(i + 1);
                    continue;
                }
                break response;
            }
        }
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

/// Build the smallest request every Gemini model accepts: contents, an output
/// ceiling, an optional system instruction, the JSON response format, and one
/// thinking control, level or budget. Sampling controls are never sent. The ceiling is
/// the talent's own visible budget plus the thinking room, which Gemini's total
/// cap may clamp but never below the visible part.
fn request_body(
    request: &GenerateRequest,
    _model: &str,
    thinking: Thinking,
    control: GoogleThinkingControl,
) -> Value {
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
    let thinking_config = match control {
        GoogleThinkingControl::Level(level) => json!({"thinkingLevel": level}),
        GoogleThinkingControl::Budget(budget) => json!({"thinkingBudget": budget}),
    };
    let mut body = json!({
        "contents": [{"role": "user", "parts": parts}],
        "generationConfig": {
            "maxOutputTokens": google_ceiling(request.max_output_tokens, thinking),
            "thinkingConfig": thinking_config,
        },
    });
    if let Some(system) = &request.system_instruction {
        body["systemInstruction"] = json!({"parts": [{"text": system}]});
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
/// The caller's timeout, else the lane default plus time for any thinking room.
fn request_timeout(timeout_s: Option<f64>, thinking_room: u64) -> Duration {
    timeout_s
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .map(Duration::from_secs_f64)
        .unwrap_or(DEFAULT_TIMEOUT + crate::thinking::thinking_time(thinking_room))
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
        400 if body.contains("API_KEY_INVALID") => "provider_key_invalid",
        404 => "model_not_found",
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
        timeouts: Vec<Duration>,
    }

    impl GoogleTransport for StubTransport {
        fn post_json(
            &mut self,
            base_url: &str,
            path: &str,
            body: &Value,
            api_key: &str,
            timeout: Duration,
        ) -> Result<HttpResponse, EndpointTransportError> {
            self.posts.push(body.clone());
            self.base_urls.push(base_url.to_owned());
            self.paths.push(path.to_owned());
            self.api_keys.push(api_key.to_owned());
            self.timeouts.push(timeout);
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
        active.insert(
            "model".into(),
            Value::String(model.unwrap_or("gemini-test-model").into()),
        );
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

    fn generate_with_recorded(
        budget: Option<u64>,
        req: &GenerateRequest,
        responses: Vec<Result<HttpResponse, EndpointTransportError>>,
    ) -> (GoogleResult, StubTransport) {
        let mut config = config(Some("configured-secret"), None);
        if let Some(budget) = budget {
            config["providers"]["byo_thinking_budget"] = json!(budget);
        }
        let original_config = config.clone();
        let mut transport = StubTransport {
            responses,
            ..Default::default()
        };
        let result = google_generate_with(req, &config, &mut transport);
        assert_eq!(
            config, original_config,
            "config map must be unchanged after call"
        );
        (result, transport)
    }

    fn post_with(budget: Option<u64>, responses: Vec<HttpResponse>) -> Vec<Value> {
        let (_result, transport) =
            generate_with_recorded(budget, &request(), responses.into_iter().map(Ok).collect());
        transport.posts
    }

    fn invalid_argument() -> HttpResponse {
        HttpResponse {
            status: 400,
            body: json!({"error": {"code": 400, "message": "Request contains an invalid argument.", "status": "INVALID_ARGUMENT"}})
                .to_string(),
        }
    }

    fn value_rejection(level: &str) -> HttpResponse {
        HttpResponse {
            status: 400,
            body: json!({"error": {"code": 400, "message": format!("Thinking level {level} is not supported for this model. Please retry with other thinking level."), "status": "INVALID_ARGUMENT"}}).to_string(),
        }
    }

    fn control_rejection() -> HttpResponse {
        HttpResponse {
            status: 400,
            body: json!({"error": {"code": 400, "message": "Thinking level is not supported for this model.", "status": "INVALID_ARGUMENT"}}).to_string(),
        }
    }

    fn schema_refusal() -> HttpResponse {
        HttpResponse {
            status: 400,
            body: json!({"error": {"code": 400, "message": "Invalid response schema.", "status": "INVALID_ARGUMENT"}}).to_string(),
        }
    }

    fn content_refusal() -> HttpResponse {
        HttpResponse {
            status: 400,
            body: json!({"error": {"code": 400, "message": "The submitted content violates the content policy.", "status": "INVALID_ARGUMENT"}}).to_string(),
        }
    }

    fn api_key_invalid() -> HttpResponse {
        HttpResponse {
            status: 400,
            body: json!({"error": {"code": 400, "message": "API key not valid. Please pass a valid API key.", "status": "INVALID_ARGUMENT", "details": [{"reason": "API_KEY_INVALID"}]}}).to_string(),
        }
    }

    #[test]
    fn thinking_off_asks_for_no_thinking_and_steps_up_only_when_refused() {
        let posts = post_with(
            None,
            vec![
                value_rejection("MINIMAL"),
                control_rejection(),
                invalid_argument(),
                invalid_argument(),
                response(successful_body()),
            ],
        );
        assert_eq!(posts.len(), 5);
        assert_eq!(
            posts[0]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "MINIMAL"
        );
        assert!(
            posts[0]["generationConfig"]["thinkingConfig"]
                .get("thinkingBudget")
                .is_none()
        );

        assert_eq!(
            posts[1]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "LOW"
        );
        assert!(
            posts[1]["generationConfig"]["thinkingConfig"]
                .get("thinkingBudget")
                .is_none()
        );

        assert_eq!(
            posts[2]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            0
        );
        assert!(
            posts[2]["generationConfig"]["thinkingConfig"]
                .get("thinkingLevel")
                .is_none()
        );

        assert_eq!(
            posts[3]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            128
        );
        assert!(
            posts[3]["generationConfig"]["thinkingConfig"]
                .get("thinkingLevel")
                .is_none()
        );

        assert_eq!(
            posts[4]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            512
        );
        assert!(
            posts[4]["generationConfig"]["thinkingConfig"]
                .get("thinkingLevel")
                .is_none()
        );

        for body in &posts {
            assert_eq!(body["generationConfig"]["maxOutputTokens"], 4_000 + 1_024);
            assert!(body["generationConfig"].get("temperature").is_none());
        }
    }

    #[test]
    fn an_owner_budget_is_sent_and_added_to_the_visible_budget() {
        let posts = post_with(
            Some(32_768),
            vec![
                control_rejection(),
                invalid_argument(),
                response(successful_body()),
            ],
        );
        assert_eq!(posts.len(), 3);
        assert_eq!(
            posts[0]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "HIGH"
        );
        assert!(
            posts[0]["generationConfig"]["thinkingConfig"]
                .get("thinkingBudget")
                .is_none()
        );

        assert_eq!(
            posts[1]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            32_768
        );
        assert!(
            posts[1]["generationConfig"]["thinkingConfig"]
                .get("thinkingLevel")
                .is_none()
        );

        // Gemini 2.5 Flash caps a thinking budget at 24,576.
        assert_eq!(
            posts[2]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            24_576
        );
        assert!(
            posts[2]["generationConfig"]["thinkingConfig"]
                .get("thinkingLevel")
                .is_none()
        );

        for body in &posts {
            assert_eq!(body["generationConfig"]["maxOutputTokens"], 4_000 + 32_768);
            assert!(body["generationConfig"].get("temperature").is_none());
        }
    }

    #[test]
    fn a_context_window_refusal_is_not_retried_with_another_budget() {
        let posts = post_with(
            None,
            vec![HttpResponse {
                status: 400,
                body: json!({"error": {"code": 400, "message": "The input token count (1200000) exceeds the maximum number of tokens allowed (1048576).", "status": "INVALID_ARGUMENT"}})
                    .to_string(),
            }],
        );
        assert_eq!(posts.len(), 1);
    }

    #[test]
    fn missing_model_is_refused_before_any_request() {
        let mut config = config(Some("configured-secret"), None);
        config["providers"]["active"]
            .as_object_mut()
            .expect("active is an object")
            .remove("model");
        let mut transport = StubTransport::default();
        let result = google_generate_with(&request(), &config, &mut transport);
        assert_eq!(
            result,
            GoogleResult::Failed(GoogleFailure {
                reason_code: Some("model_missing".into()),
                detail: None,
            })
        );
        assert!(transport.posts.is_empty());
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
        let config = &request_body(
            &request,
            "gemini-test-model",
            Thinking::Off,
            GoogleThinkingControl::Level("MINIMAL"),
        )["generationConfig"];
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
        let config = &request_body(
            &request,
            "gemini-test-model",
            Thinking::Off,
            GoogleThinkingControl::Level("MINIMAL"),
        )["generationConfig"];
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
            vec!["/v1beta/models/gemini-test-model:generateContent".to_owned()]
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
        let body = request_body(
            &request,
            "gemini-test-model",
            Thinking::Off,
            GoogleThinkingControl::Level("MINIMAL"),
        );
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
            thinking_seen: false,
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
            thinking_seen: false,
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
            thinking_seen: false,
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

    #[test]
    fn unknown_model_and_auth_failures_classify_distinctly() {
        assert_eq!(
            classify_http_failure(
                404,
                r#"{"error":{"code":404,"message":"models/solstone-key-check is not found","status":"NOT_FOUND"}}"#
            ),
            "model_not_found"
        );
        // Shape captured from the live Gemini API.
        assert_eq!(
            classify_http_failure(
                400,
                r#"{"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","status":"INVALID_ARGUMENT","details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"API_KEY_INVALID"}]}}"#
            ),
            "provider_key_invalid"
        );
        assert_eq!(
            classify_http_failure(400, r#"{"error":{"status":"INVALID_ARGUMENT"}}"#),
            "provider_request_rejected"
        );
    }

    #[test]
    fn first_post_sends_level_without_budget_for_all_configs() {
        for (budget, expected_level) in [
            (None, "MINIMAL"),
            (Some(0), "MINIMAL"),
            (Some(8_192), "LOW"),
            (Some(16_384), "MEDIUM"),
            (Some(32_768), "HIGH"),
        ] {
            let posts = post_with(budget, vec![response(successful_body())]);
            assert_eq!(posts.len(), 1);
            assert_eq!(
                posts[0]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
                expected_level
            );
            assert!(
                posts[0]["generationConfig"]["thinkingConfig"]
                    .get("thinkingBudget")
                    .is_none()
            );
        }
    }

    #[test]
    fn value_rejection_steps_to_next_level_and_exhaustion_stops_without_budgets() {
        // Off: MINIMAL value-rejection then LOW, no budget key
        let posts = post_with(
            None,
            vec![value_rejection("MINIMAL"), response(successful_body())],
        );
        assert_eq!(posts.len(), 2);
        assert_eq!(
            posts[0]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "MINIMAL"
        );
        assert!(
            posts[0]["generationConfig"]["thinkingConfig"]
                .get("thinkingBudget")
                .is_none()
        );
        assert_eq!(
            posts[1]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "LOW"
        );
        assert!(
            posts[1]["generationConfig"]["thinkingConfig"]
                .get("thinkingBudget")
                .is_none()
        );

        // 8192: value-rejection is one post (LOW) and GoogleResult::Failed, no budget
        let (result, transport) =
            generate_with_recorded(Some(8_192), &request(), vec![Ok(value_rejection("LOW"))]);
        assert_eq!(transport.posts.len(), 1);
        assert_eq!(
            transport.posts[0]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "LOW"
        );
        assert!(
            transport.posts[0]["generationConfig"]["thinkingConfig"]
                .get("thinkingBudget")
                .is_none()
        );
        assert!(matches!(result, GoogleResult::Failed(_)));

        // 16384: MEDIUM value-rejection's next post is LOW, not HIGH, and has no budget.
        // A following LOW value-rejection returns Failed and does not post a budget.
        let (result, transport) = generate_with_recorded(
            Some(16_384),
            &request(),
            vec![Ok(value_rejection("MEDIUM")), Ok(value_rejection("LOW"))],
        );
        assert_eq!(transport.posts.len(), 2);
        assert_eq!(
            transport.posts[0]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "MEDIUM"
        );
        assert!(
            transport.posts[0]["generationConfig"]["thinkingConfig"]
                .get("thinkingBudget")
                .is_none()
        );
        assert_eq!(
            transport.posts[1]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "LOW"
        );
        assert!(
            transport.posts[1]["generationConfig"]["thinkingConfig"]
                .get("thinkingBudget")
                .is_none()
        );
        assert!(matches!(result, GoogleResult::Failed(_)));

        // 32768: one HIGH value-rejection returns Failed in one post
        let (result, transport) =
            generate_with_recorded(Some(32_768), &request(), vec![Ok(value_rejection("HIGH"))]);
        assert_eq!(transport.posts.len(), 1);
        assert_eq!(
            transport.posts[0]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "HIGH"
        );
        assert!(
            transport.posts[0]["generationConfig"]["thinkingConfig"]
                .get("thinkingBudget")
                .is_none()
        );
        assert!(matches!(result, GoogleResult::Failed(_)));
    }

    #[test]
    fn control_refusal_starts_saved_budget_ladder_and_has_no_level() {
        // absent / 0 -> budgets 0, 128, 512
        let posts = post_with(None, vec![control_rejection(), response(successful_body())]);
        assert_eq!(posts.len(), 2);
        assert_eq!(
            posts[0]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "MINIMAL"
        );
        assert!(
            posts[0]["generationConfig"]["thinkingConfig"]
                .get("thinkingBudget")
                .is_none()
        );
        assert_eq!(
            posts[1]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            0
        );
        assert!(
            posts[1]["generationConfig"]["thinkingConfig"]
                .get("thinkingLevel")
                .is_none()
        );

        // 8192 -> budget 8192 only
        let posts = post_with(
            Some(8_192),
            vec![control_rejection(), response(successful_body())],
        );
        assert_eq!(posts.len(), 2);
        assert_eq!(
            posts[0]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "LOW"
        );
        assert!(
            posts[0]["generationConfig"]["thinkingConfig"]
                .get("thinkingBudget")
                .is_none()
        );
        assert_eq!(
            posts[1]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            8_192
        );
        assert!(
            posts[1]["generationConfig"]["thinkingConfig"]
                .get("thinkingLevel")
                .is_none()
        );

        // 16384 -> budget 16384 only (LOW is NOT posted)
        let posts = post_with(
            Some(16_384),
            vec![control_rejection(), response(successful_body())],
        );
        assert_eq!(posts.len(), 2);
        assert_eq!(
            posts[0]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "MEDIUM"
        );
        assert!(
            posts[0]["generationConfig"]["thinkingConfig"]
                .get("thinkingBudget")
                .is_none()
        );
        assert_eq!(
            posts[1]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            16_384
        );
        assert!(
            posts[1]["generationConfig"]["thinkingConfig"]
                .get("thinkingLevel")
                .is_none()
        );

        // 32768 -> budget 32768 then 24576
        let posts = post_with(
            Some(32_768),
            vec![
                control_rejection(),
                invalid_argument(),
                response(successful_body()),
            ],
        );
        assert_eq!(posts.len(), 3);
        assert_eq!(
            posts[0]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "HIGH"
        );
        assert!(
            posts[0]["generationConfig"]["thinkingConfig"]
                .get("thinkingBudget")
                .is_none()
        );
        assert_eq!(
            posts[1]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            32_768
        );
        assert!(
            posts[1]["generationConfig"]["thinkingConfig"]
                .get("thinkingLevel")
                .is_none()
        );
        assert_eq!(
            posts[2]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            24_576
        );
        assert!(
            posts[2]["generationConfig"]["thinkingConfig"]
                .get("thinkingLevel")
                .is_none()
        );
    }

    #[test]
    fn success_on_level_posts_once_and_never_posts_budget() {
        // Off: MINIMAL value-rejected, LOW then 200 -> exactly two posts, both levels, no budget
        let posts = post_with(
            None,
            vec![value_rejection("MINIMAL"), response(successful_body())],
        );
        assert_eq!(posts.len(), 2);
        assert_eq!(
            posts[0]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "MINIMAL"
        );
        assert!(
            posts[0]["generationConfig"]["thinkingConfig"]
                .get("thinkingBudget")
                .is_none()
        );
        assert_eq!(
            posts[1]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "LOW"
        );
        assert!(
            posts[1]["generationConfig"]["thinkingConfig"]
                .get("thinkingBudget")
                .is_none()
        );

        // 16384: first-try MEDIUM 200 is one post, level MEDIUM, no budget
        let posts = post_with(Some(16_384), vec![response(successful_body())]);
        assert_eq!(posts.len(), 1);
        assert_eq!(
            posts[0]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "MEDIUM"
        );
        assert!(
            posts[0]["generationConfig"]["thinkingConfig"]
                .get("thinkingBudget")
                .is_none()
        );
    }

    #[test]
    fn exhausted_ladders_produce_provider_request_rejected_with_last_body() {
        // Exhausted off, control refused on MINIMAL, then bare budget refusals:
        // posts: level MINIMAL, budget 0, budget 128, budget 512
        let last_body_off = invalid_argument().body;
        let (result, transport) = generate_with_recorded(
            None,
            &request(),
            vec![
                Ok(control_rejection()),
                Ok(invalid_argument()),
                Ok(invalid_argument()),
                Ok(invalid_argument()),
            ],
        );
        assert_eq!(transport.posts.len(), 4);
        assert_eq!(
            transport.posts[0]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "MINIMAL"
        );
        assert_eq!(
            transport.posts[1]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            0
        );
        assert_eq!(
            transport.posts[2]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            128
        );
        assert_eq!(
            transport.posts[3]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            512
        );
        let GoogleResult::Failed(failure) = result else {
            panic!("must fail");
        };
        let refusal = refusal_for(&LaneOutcome::GoogleFailure(failure), "google", None);
        assert_eq!(
            refusal.reason_code.as_ref().map(ReasonCodeValue::as_wire),
            Some("provider_request_rejected")
        );
        assert_eq!(refusal.detail, last_body_off);

        // Exhausted 32768: level HIGH, budget 32768, budget 24576
        let last_body_32k = invalid_argument().body;
        let (result, transport) = generate_with_recorded(
            Some(32_768),
            &request(),
            vec![
                Ok(control_rejection()),
                Ok(invalid_argument()),
                Ok(invalid_argument()),
            ],
        );
        assert_eq!(transport.posts.len(), 3);
        assert_eq!(
            transport.posts[0]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "HIGH"
        );
        assert_eq!(
            transport.posts[1]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            32_768
        );
        assert_eq!(
            transport.posts[2]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            24_576
        );
        let GoogleResult::Failed(failure) = result else {
            panic!("must fail");
        };
        let refusal = refusal_for(&LaneOutcome::GoogleFailure(failure), "google", None);
        assert_eq!(
            refusal.reason_code.as_ref().map(ReasonCodeValue::as_wire),
            Some("provider_request_rejected")
        );
        assert_eq!(refusal.detail, last_body_32k);

        // Longer off path: MINIMAL value-rejected, LOW control-refused, then budgets 0, 128, 512
        let (result, transport) = generate_with_recorded(
            None,
            &request(),
            vec![
                Ok(value_rejection("MINIMAL")),
                Ok(control_rejection()),
                Ok(invalid_argument()),
                Ok(invalid_argument()),
                Ok(invalid_argument()),
            ],
        );
        assert_eq!(transport.posts.len(), 5);
        assert_eq!(
            transport.posts[0]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "MINIMAL"
        );
        assert!(
            transport.posts[0]["generationConfig"]["thinkingConfig"]
                .get("thinkingBudget")
                .is_none()
        );
        assert_eq!(
            transport.posts[1]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "LOW"
        );
        assert!(
            transport.posts[1]["generationConfig"]["thinkingConfig"]
                .get("thinkingBudget")
                .is_none()
        );
        assert_eq!(
            transport.posts[2]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            0
        );
        assert!(
            transport.posts[2]["generationConfig"]["thinkingConfig"]
                .get("thinkingLevel")
                .is_none()
        );
        assert_eq!(
            transport.posts[3]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            128
        );
        assert!(
            transport.posts[3]["generationConfig"]["thinkingConfig"]
                .get("thinkingLevel")
                .is_none()
        );
        assert_eq!(
            transport.posts[4]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            512
        );
        assert!(
            transport.posts[4]["generationConfig"]["thinkingConfig"]
                .get("thinkingLevel")
                .is_none()
        );
        assert!(matches!(result, GoogleResult::Failed(_)));
    }

    #[test]
    fn terminal_errors_stop_in_single_post_and_stop_budget_ladder() {
        let terminal_cases = [
            (
                Ok(HttpResponse {
                    status: 401,
                    body: "{}".into(),
                }),
                "provider_key_invalid",
            ),
            (
                Ok(HttpResponse {
                    status: 403,
                    body: "{}".into(),
                }),
                "provider_key_invalid",
            ),
            (
                Ok(api_key_invalid()),
                "provider_key_invalid",
            ),
            (
                Ok(HttpResponse {
                    status: 429,
                    body: "{}".into(),
                }),
                "provider_quota_exceeded",
            ),
            (
                Ok(HttpResponse {
                    status: 500,
                    body: "{}".into(),
                }),
                "provider_unavailable",
            ),
            (
                Err(EndpointTransportError::Connection),
                "network_unreachable",
            ),
            (
                Ok(HttpResponse {
                    status: 400,
                    body: json!({"error": {"message": "The input token count (1200000) exceeds the maximum number of tokens allowed (1048576).", "status": "INVALID_ARGUMENT"}}).to_string(),
                }),
                "context_window_exceeded",
            ),
            (
                Ok(schema_refusal()),
                "provider_request_rejected",
            ),
            (
                Ok(content_refusal()),
                "provider_request_rejected",
            ),
        ];

        for (response, expected_code) in terminal_cases {
            let (result, transport) = generate_with_recorded(None, &request(), vec![response]);
            assert_eq!(transport.posts.len(), 1);
            let GoogleResult::Failed(failure) = result else {
                panic!("case must fail");
            };
            assert_eq!(failure.reason_code.as_deref(), Some(expected_code));
        }

        // After control refusal has entered budget ladder, explicit schema or content stops further budgets
        let (result, transport) = generate_with_recorded(
            None,
            &request(),
            vec![Ok(control_rejection()), Ok(schema_refusal())],
        );
        assert_eq!(transport.posts.len(), 2);
        assert_eq!(
            transport.posts[0]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "MINIMAL"
        );
        assert_eq!(
            transport.posts[1]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            0
        );
        assert!(matches!(result, GoogleResult::Failed(_)));
    }

    #[test]
    fn bare_invalid_argument_stops_on_level_and_steps_only_in_budget_phase() {
        // Bare INVALID_ARGUMENT on first level posts once and does not start budgets
        let (result, transport) =
            generate_with_recorded(None, &request(), vec![Ok(invalid_argument())]);
        assert_eq!(transport.posts.len(), 1);
        assert_eq!(
            transport.posts[0]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "MINIMAL"
        );
        assert!(
            transport.posts[0]["generationConfig"]["thinkingConfig"]
                .get("thinkingBudget")
                .is_none()
        );
        assert!(matches!(result, GoogleResult::Failed(_)));

        // The same bare body after a control refusal walks the remaining budget ladder
        let (result, transport) = generate_with_recorded(
            None,
            &request(),
            vec![
                Ok(control_rejection()),
                Ok(invalid_argument()),
                Ok(invalid_argument()),
                Ok(response(successful_body())),
            ],
        );
        assert_eq!(transport.posts.len(), 4);
        assert_eq!(
            transport.posts[0]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "MINIMAL"
        );
        assert_eq!(
            transport.posts[1]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            0
        );
        assert_eq!(
            transport.posts[2]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            128
        );
        assert_eq!(
            transport.posts[3]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            512
        );
        assert!(matches!(result, GoogleResult::Generated(_)));
    }

    #[test]
    fn sampling_omitted_and_ceilings_timeouts_clamp_stable() {
        let mut req = request();
        req.system_instruction = Some("system test".into());
        req.temperature = 0.3;
        req.json_schema = Some(json!({"type": "object"}));
        req.json_output = true;

        for (budget, expected_ceiling, expected_timeout_secs) in [
            (None, 5024, 136),
            (Some(0), 5024, 136),
            (Some(8_192), 12192, 248),
            (Some(16_384), 20384, 376),
            (Some(32_768), 36768, 632),
        ] {
            let responses = if budget == Some(8_192) || budget == Some(16_384) {
                vec![Ok(control_rejection()), Ok(response(successful_body()))]
            } else {
                vec![
                    Ok(control_rejection()),
                    Ok(invalid_argument()),
                    Ok(response(successful_body())),
                ]
            };
            let (result, transport) = generate_with_recorded(budget, &req, responses);
            assert!(matches!(result, GoogleResult::Generated(_)));
            assert!(transport.posts.len() >= 2);
            for post in &transport.posts {
                assert!(post.get("temperature").is_none());
                assert!(post.get("topP").is_none());
                assert!(post.get("topK").is_none());
                assert!(post.get("top_p").is_none());
                assert!(post.get("top_k").is_none());
                assert!(post["generationConfig"].get("temperature").is_none());
                assert!(post["generationConfig"].get("topP").is_none());
                assert!(post["generationConfig"].get("topK").is_none());
                assert!(post["generationConfig"].get("top_p").is_none());
                assert!(post["generationConfig"].get("top_k").is_none());

                assert_eq!(
                    post["generationConfig"]["maxOutputTokens"],
                    expected_ceiling
                );
                assert_eq!(post["contents"], transport.posts[0]["contents"]);
                assert_eq!(
                    post["systemInstruction"],
                    transport.posts[0]["systemInstruction"]
                );
                assert_eq!(
                    post["generationConfig"]["responseJsonSchema"],
                    transport.posts[0]["generationConfig"]["responseJsonSchema"]
                );
                assert_eq!(
                    post["generationConfig"]["responseMimeType"],
                    transport.posts[0]["generationConfig"]["responseMimeType"]
                );
            }
            for timeout in &transport.timeouts {
                assert_eq!(*timeout, Duration::from_secs(expected_timeout_secs));
            }
        }

        // Clamp tests with saved 32768
        let mut clamp_40k = req.clone();
        clamp_40k.max_output_tokens = 40_000;
        let (_result, transport_40k) = generate_with_recorded(
            Some(32_768),
            &clamp_40k,
            vec![Ok(control_rejection()), Ok(response(successful_body()))],
        );
        for post in &transport_40k.posts {
            assert_eq!(post["generationConfig"]["maxOutputTokens"], 65535);
        }

        let mut clamp_70k = req.clone();
        clamp_70k.max_output_tokens = 70_000;
        let (_result, transport_70k) = generate_with_recorded(
            Some(32_768),
            &clamp_70k,
            vec![Ok(control_rejection()), Ok(response(successful_body()))],
        );
        for post in &transport_70k.posts {
            assert_eq!(post["generationConfig"]["maxOutputTokens"], 70000);
        }

        // Explicit timeout_s: Some(45.0)
        let mut req_explicit_timeout = req.clone();
        req_explicit_timeout.timeout_s = Some(45.0);
        let (_result, transport_explicit) = generate_with_recorded(
            Some(32_768),
            &req_explicit_timeout,
            vec![Ok(control_rejection()), Ok(response(successful_body()))],
        );
        for timeout in &transport_explicit.timeouts {
            assert_eq!(*timeout, Duration::from_secs_f64(45.0));
        }
    }

    #[test]
    fn empty_text_validations_distinguish_thinking_consumed_from_invalid() {
        let mut body = successful_body();
        body["candidates"] = json!([{
            "content": {"parts": [{"text": ""}]},
            "finishReason": "MAX_TOKENS",
        }]);
        body["usageMetadata"] = json!({
            "promptTokenCount": 10,
            "candidatesTokenCount": 20,
            "totalTokenCount": 30,
            "thoughtsTokenCount": 15,
        });
        let (result, transport) =
            generate_with_recorded(Some(8_192), &request(), vec![Ok(response(body))]);
        assert_eq!(transport.posts.len(), 1);
        assert_eq!(
            transport.posts[0]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "LOW"
        );
        assert!(
            transport.posts[0]["generationConfig"]["thinkingConfig"]
                .get("thinkingBudget")
                .is_none()
        );
        let GoogleResult::Generated(success) = result else {
            panic!("must succeed as Generated");
        };
        let journal = temp_journal();
        let assessment = assess_provider_result(ProviderResultView {
            journal_path: &journal,
            context: "test.generate",
            model: &success.model,
            text: &success.text,
            finish_reason: &success.finish_reason,
            usage: &success.usage,
            json_output: false,
            enforce_responsiveness: false,
            raw_response_snippet: success.raw_response_snippet.as_deref(),
            thinking_seen: false,
        });
        assert_eq!(
            assessment.failure,
            Some(ValidationFailure::ThinkingConsumedBudget { json_output: false })
        );
        let _ = fs::remove_dir_all(journal);

        // Second case: empty text, finishReason STOP, no thoughts -> ProviderResponseInvalid
        let mut body_invalid = successful_body();
        body_invalid["candidates"] = json!([{
            "content": {"parts": [{"text": ""}]},
            "finishReason": "STOP",
        }]);
        body_invalid["usageMetadata"] = json!({
            "promptTokenCount": 10,
            "candidatesTokenCount": 0,
            "totalTokenCount": 10,
        });
        let (result_invalid, _) =
            generate_with_recorded(None, &request(), vec![Ok(response(body_invalid))]);
        let GoogleResult::Generated(success_invalid) = result_invalid else {
            panic!("must succeed as Generated");
        };
        let journal = temp_journal();
        let assessment_invalid = assess_provider_result(ProviderResultView {
            journal_path: &journal,
            context: "test.generate",
            model: &success_invalid.model,
            text: &success_invalid.text,
            finish_reason: &success_invalid.finish_reason,
            usage: &success_invalid.usage,
            json_output: false,
            enforce_responsiveness: false,
            raw_response_snippet: success_invalid.raw_response_snippet.as_deref(),
            thinking_seen: false,
        });
        assert!(matches!(
            assessment_invalid.failure,
            Some(ValidationFailure::ProviderResponseInvalid { .. })
        ));
        let _ = fs::remove_dir_all(journal);
    }
}
