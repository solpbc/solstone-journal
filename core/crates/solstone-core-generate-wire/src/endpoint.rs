// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bring-your-own local endpoint generation.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::{Map, Value, json};
use solstone_core_generate::{ContentPart, GenerateRequest};
use solstone_core_local::admission::{
    AdmissionError, LocalSlotPermit, acquire_local_slot, admission_dir,
};
use solstone_core_local::{
    ByoEndpoint, HttpResponse, InputBudget, RequestBudget, Usage, build_messages,
    build_request_body, count_image_parts, estimate_tokens, fit_contents, parse_response,
    serialized_message_text, served_window_from_models_response,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const ENDPOINT_MODELS_TIMEOUT: Duration = Duration::from_millis(2_500);
pub const ENDPOINT_SERVED_WINDOW_CACHE_TTL: Duration = Duration::from_secs(300);
const SERVED_CONTEXT_WINDOW_MIN_TOKENS: u32 = 2_048;
const SAFETY_MARGIN_TOKENS: u32 = 256;
const MIN_COMPLETION_TOKENS: u32 = 256;
const ESTIMATED_IMAGE_TOKENS: u32 = 2_500;
const RECLAMP_SLACK_TOKENS: u32 = 16;
const COMPLETION_ANCHOR: &str = "tokens for the completion";
const CONTEXT_WINDOW_PATTERNS: &[&str] = &[
    "exceeds the available context size",
    "context size has been exceeded",
    "exceeds the context window",
    "maximum context length",
    "longer than the model's context length",
    "context length exceeded",
];

static LIMIT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"maximum context length of\s+(?P<limit>\d+)\s+tokens").expect("valid limit regex")
});
static INPUT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?P<input>\d+)\s+tokens?\s+from\s+the\s+input\s+messages?\s+and\s+\d+\s+tokens?\s+for\s+the\s+completion")
        .expect("valid input regex")
});

type ServedWindowCache = HashMap<(String, String), (Option<u32>, Instant)>;

#[derive(Debug, Clone, PartialEq)]
pub struct EndpointGenerated {
    pub text: String,
    pub model: String,
    pub usage: Option<Usage>,
    pub finish_reason: String,
    pub input_budget: Option<InputBudget>,
    pub request_budget: Option<RequestBudget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointFailure {
    pub reason_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EndpointResult {
    Generated(EndpointGenerated),
    Failed(EndpointFailure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverflowDecision {
    Retry(u32),
    Budget,
    Context,
    Contract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointTransportError {
    Connection,
    Capacity,
    Other,
}

pub trait EndpointTransport {
    fn get(
        &mut self,
        base_url: &str,
        path: &str,
        credential: Option<&str>,
        timeout: Duration,
    ) -> Result<HttpResponse, EndpointTransportError>;

    fn post_json(
        &mut self,
        base_url: &str,
        path: &str,
        body: &Value,
        credential: Option<&str>,
        timeout: Duration,
    ) -> Result<HttpResponse, EndpointTransportError>;
}

#[derive(Default)]
pub struct UreqEndpointTransport;

impl EndpointTransport for UreqEndpointTransport {
    fn get(
        &mut self,
        base_url: &str,
        path: &str,
        credential: Option<&str>,
        timeout: Duration,
    ) -> Result<HttpResponse, EndpointTransportError> {
        endpoint_request("get", base_url, path, None, credential, timeout)
    }

    fn post_json(
        &mut self,
        base_url: &str,
        path: &str,
        body: &Value,
        credential: Option<&str>,
        timeout: Duration,
    ) -> Result<HttpResponse, EndpointTransportError> {
        endpoint_request("post", base_url, path, Some(body), credential, timeout)
    }
}

#[derive(Default)]
pub struct EndpointRuntime {
    served_windows: Mutex<ServedWindowCache>,
}

pub fn endpoint_generate(
    request: &GenerateRequest,
    journal_path: &Path,
    endpoint: &ByoEndpoint,
    config: &Map<String, Value>,
    runtime: &EndpointRuntime,
) -> EndpointResult {
    let mut transport = UreqEndpointTransport;
    endpoint_generate_with(
        request,
        journal_path,
        endpoint,
        config,
        runtime,
        &mut transport,
        Instant::now(),
    )
}

fn endpoint_generate_with<T: EndpointTransport>(
    request: &GenerateRequest,
    journal_path: &Path,
    endpoint: &ByoEndpoint,
    config: &Map<String, Value>,
    runtime: &EndpointRuntime,
    transport: &mut T,
    now: Instant,
) -> EndpointResult {
    let max_tokens = match u32::try_from(request.max_output_tokens) {
        Ok(value) => value,
        Err(_) => return failure("provider_response_invalid"),
    };
    let served_window = runtime.resolve_served_window(endpoint, config, transport, now);
    let mut prepared = match prepare_endpoint_request(request, endpoint, max_tokens, served_window)
    {
        Ok(prepared) => prepared,
        Err(reason_code) => return failure(reason_code),
    };
    let timeout = request_timeout(request.timeout_s);
    let started = Instant::now();
    let Some(admission_timeout) = remaining_timeout(started, timeout) else {
        return failure("local_capacity_exhausted");
    };
    if admission_timeout.is_zero() {
        return failure("local_capacity_exhausted");
    }
    let response = {
        let _permit = match acquire_endpoint_slot(
            journal_path,
            endpoint,
            request.exclusive_admission,
            admission_timeout,
        ) {
            Ok(permit) => permit,
            Err(reason_code) => return failure(reason_code),
        };
        let mut attempt = 0;
        loop {
            let Some(remaining) = remaining_timeout(started, timeout) else {
                return failure("local_capacity_exhausted");
            };
            if remaining.is_zero() {
                return failure("local_capacity_exhausted");
            }
            let response = match endpoint_post(endpoint, &prepared.body, remaining, transport) {
                Ok(response) => response,
                Err(reason_code) => return failure(reason_code),
            };
            if response.status != 400 {
                break response;
            }
            match endpoint_overflow_decision(&response.body, served_window, attempt) {
                OverflowDecision::Retry(new_max_tokens) => {
                    prepared.body["max_tokens"] = json!(new_max_tokens);
                    if let Some(request_budget) = prepared.request_budget.as_mut() {
                        request_budget.clamped_max_tokens = new_max_tokens;
                    }
                    attempt += 1;
                }
                OverflowDecision::Budget => return failure("context_budget_exceeded"),
                OverflowDecision::Context => return failure("context_window_exceeded"),
                OverflowDecision::Contract => return failure("local_endpoint_contract_failed"),
            }
        }
    };
    if !(200..300).contains(&response.status) {
        return failure("provider_response_invalid");
    }
    let body = match serde_json::from_str::<Value>(&response.body) {
        Ok(body) => body,
        Err(_) => return failure("provider_response_invalid"),
    };
    let parsed = match parse_response(&body) {
        Ok(parsed) => parsed,
        Err(_) => return failure("provider_response_invalid"),
    };
    EndpointResult::Generated(EndpointGenerated {
        text: parsed.text,
        model: endpoint.served_model_id.clone(),
        usage: parsed.usage,
        finish_reason: parsed.finish_reason,
        input_budget: prepared.input_budget,
        request_budget: prepared.request_budget,
    })
}

struct PreparedEndpointRequest {
    body: Value,
    input_budget: Option<InputBudget>,
    request_budget: Option<RequestBudget>,
}

fn prepare_endpoint_request(
    request: &GenerateRequest,
    endpoint: &ByoEndpoint,
    max_tokens: u32,
    served_window: Option<u32>,
) -> Result<PreparedEndpointRequest, &'static str> {
    let contents = request_contents(request);
    let (contents, input_budget, request_budget, max_tokens) = match served_window {
        None => (contents, None, None, max_tokens),
        Some(window) => {
            let mut count = estimate_tokens;
            let (fitted_contents, input_budget) = fit_contents(
                &contents,
                request.system_instruction.as_deref(),
                max_tokens,
                window,
                &mut count,
            )
            .map_err(|_| "context_budget_exceeded")?;
            let messages = build_messages(&fitted_contents, request.system_instruction.as_deref());
            let estimated_prompt_tokens = estimate_tokens(&serialized_message_text(&messages));
            let image_tokens =
                ESTIMATED_IMAGE_TOKENS.saturating_mul(count_image_parts(&fitted_contents));
            let room = window
                .saturating_sub(estimated_prompt_tokens)
                .saturating_sub(image_tokens)
                .saturating_sub(SAFETY_MARGIN_TOKENS);
            if room < MIN_COMPLETION_TOKENS {
                return Err("context_budget_exceeded");
            }
            let clamped_max_tokens = max_tokens.min(room);
            let request_budget = RequestBudget {
                window,
                slots: endpoint
                    .parallel_slots
                    .expect("non-confidential BYO endpoint lanes have configured parallel slots"),
                estimated_prompt_tokens,
                image_tokens,
                clamped_max_tokens,
                requested_max_output_tokens: max_tokens,
            };
            (
                fitted_contents,
                input_budget,
                Some(request_budget),
                clamped_max_tokens,
            )
        }
    };
    Ok(PreparedEndpointRequest {
        body: build_request_body(
            &endpoint.served_model_id,
            build_messages(&contents, request.system_instruction.as_deref()),
            request.temperature,
            max_tokens,
            request.json_output,
            request.json_schema.as_ref(),
            false,
        ),
        input_budget,
        request_budget,
    })
}

fn acquire_endpoint_slot(
    journal_path: &Path,
    endpoint: &ByoEndpoint,
    exclusive_admission: bool,
    timeout: Duration,
) -> Result<LocalSlotPermit, &'static str> {
    acquire_local_slot(
        &admission_dir(journal_path),
        endpoint
            .parallel_slots
            .expect("non-confidential BYO endpoint lanes have configured parallel slots"),
        Some(timeout),
        exclusive_admission,
    )
    .map_err(|error| match error {
        AdmissionError::Timeout => "local_queue_timeout",
        AdmissionError::Io(_) => "provider_response_invalid",
    })
}

fn endpoint_post<T: EndpointTransport>(
    endpoint: &ByoEndpoint,
    body: &Value,
    timeout: Duration,
    transport: &mut T,
) -> Result<HttpResponse, &'static str> {
    transport
        .post_json(
            &endpoint.base_url,
            "/v1/chat/completions",
            body,
            endpoint.credential.as_deref(),
            timeout,
        )
        .map_err(|error| match error {
            EndpointTransportError::Connection => "local_endpoint_unreachable",
            EndpointTransportError::Capacity => "local_capacity_exhausted",
            EndpointTransportError::Other => "provider_response_invalid",
        })
}

fn remaining_timeout(started: Instant, timeout: Duration) -> Option<Duration> {
    timeout.checked_sub(started.elapsed())
}

pub fn endpoint_overflow_decision(
    body_text: &str,
    served_window: Option<u32>,
    attempt: u32,
) -> OverflowDecision {
    let body = body_text.to_ascii_lowercase();
    if body.contains(COMPLETION_ANCHOR) {
        let limit = LIMIT_RE
            .captures(&body)
            .and_then(|captures| captures.name("limit"))
            .and_then(|capture| capture.as_str().parse::<u32>().ok())
            .or(served_window);
        let input = INPUT_RE
            .captures(&body)
            .and_then(|captures| captures.name("input"))
            .and_then(|capture| capture.as_str().parse::<u32>().ok());
        if let (Some(limit), Some(input)) = (limit, input) {
            let new_max_tokens = limit
                .saturating_sub(input)
                .saturating_sub(RECLAMP_SLACK_TOKENS);
            if attempt == 0 && new_max_tokens >= MIN_COMPLETION_TOKENS {
                return OverflowDecision::Retry(new_max_tokens);
            }
            return if attempt == 0 {
                OverflowDecision::Budget
            } else {
                OverflowDecision::Context
            };
        }
    }
    if CONTEXT_WINDOW_PATTERNS
        .iter()
        .any(|pattern| body.contains(pattern))
    {
        OverflowDecision::Context
    } else {
        OverflowDecision::Contract
    }
}

impl EndpointRuntime {
    fn resolve_served_window<T: EndpointTransport>(
        &self,
        endpoint: &ByoEndpoint,
        config: &Map<String, Value>,
        transport: &mut T,
        now: Instant,
    ) -> Option<u32> {
        if let Some(window) = configured_served_context_window(config) {
            return Some(window);
        }
        let key = (endpoint.base_url.clone(), endpoint.served_model_id.clone());
        if let Some(value) = self
            .served_windows
            .lock()
            .expect("endpoint served-window cache lock poisoned")
            .get(&key)
            .filter(|(_, cached_at)| {
                now.checked_duration_since(*cached_at)
                    .is_some_and(|age| age < ENDPOINT_SERVED_WINDOW_CACHE_TTL)
            })
            .map(|(value, _)| *value)
        {
            return value;
        }
        let value = discover_served_window(endpoint, transport);
        self.served_windows
            .lock()
            .expect("endpoint served-window cache lock poisoned")
            .insert(key, (value, now));
        value
    }
}

fn configured_served_context_window(config: &Map<String, Value>) -> Option<u32> {
    config
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get("local"))
        .and_then(Value::as_object)
        .and_then(|local| local.get("served_context_window"))
        .and_then(Value::as_u64)
        .and_then(|window| u32::try_from(window).ok())
        .filter(|window| *window >= SERVED_CONTEXT_WINDOW_MIN_TOKENS)
}

fn discover_served_window<T: EndpointTransport>(
    endpoint: &ByoEndpoint,
    transport: &mut T,
) -> Option<u32> {
    let response = transport
        .get(
            &endpoint.base_url,
            "/v1/models",
            endpoint.credential.as_deref(),
            ENDPOINT_MODELS_TIMEOUT,
        )
        .ok()?;
    if !(200..300).contains(&response.status) {
        return None;
    }
    let body = serde_json::from_str(&response.body).ok()?;
    served_window_from_models_response(&body, &endpoint.served_model_id)
}

fn request_contents(request: &GenerateRequest) -> Value {
    Value::Array(
        request
            .contents
            .iter()
            .map(|content| match content {
                ContentPart::Text { text } => Value::String(text.clone()),
                ContentPart::Image { mime_type, data } => {
                    json!({"type": "image", "mime_type": mime_type, "data": data})
                }
            })
            .collect(),
    )
}

fn request_timeout(timeout_s: Option<f64>) -> Duration {
    timeout_s
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .map(Duration::from_secs_f64)
        .unwrap_or(DEFAULT_TIMEOUT)
}

fn failure(reason_code: &str) -> EndpointResult {
    EndpointResult::Failed(EndpointFailure {
        reason_code: Some(reason_code.to_owned()),
    })
}

fn endpoint_request(
    method: &str,
    base_url: &str,
    path: &str,
    body: Option<&Value>,
    credential: Option<&str>,
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
    let url = format!("{base_url}{path}");
    let response = match (method, body) {
        ("get", None) => {
            let mut request = agent.get(&url);
            if let Some(credential) = credential {
                request = request.header("Authorization", &format!("Bearer {credential}"));
            }
            request.call()
        }
        ("post", Some(body)) => {
            let mut request = agent.post(&url).header("Content-Type", "application/json");
            if let Some(credential) = credential {
                request = request.header("Authorization", &format!("Bearer {credential}"));
            }
            request.send(serde_json::to_string(body).expect("JSON value serializes"))
        }
        _ => unreachable!("endpoint transport uses GET or JSON POST"),
    }
    .map_err(classify_ureq_error)?;
    let status = response.status().as_u16();
    let body = response
        .into_body()
        .read_to_string()
        .map_err(classify_ureq_error)?;
    Ok(HttpResponse { status, body })
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
    use std::io::Read;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar};
    use std::thread;

    use serde_json::json;

    use super::*;

    static NEXT_JOURNAL: AtomicUsize = AtomicUsize::new(0);

    #[derive(Default)]
    struct StubTransport {
        get_result: Option<Result<HttpResponse, EndpointTransportError>>,
        post_result: Option<Result<HttpResponse, EndpointTransportError>>,
        post_results: Vec<Result<HttpResponse, EndpointTransportError>>,
        get_calls: usize,
        posts: Vec<Value>,
        get_credentials: Vec<Option<String>>,
        post_credentials: Vec<Option<String>>,
    }

    impl EndpointTransport for StubTransport {
        fn get(
            &mut self,
            _base_url: &str,
            _path: &str,
            credential: Option<&str>,
            _timeout: Duration,
        ) -> Result<HttpResponse, EndpointTransportError> {
            self.get_calls += 1;
            self.get_credentials.push(credential.map(str::to_owned));
            self.get_result
                .clone()
                .unwrap_or(Err(EndpointTransportError::Other))
        }

        fn post_json(
            &mut self,
            _base_url: &str,
            _path: &str,
            body: &Value,
            credential: Option<&str>,
            _timeout: Duration,
        ) -> Result<HttpResponse, EndpointTransportError> {
            self.posts.push(body.clone());
            self.post_credentials.push(credential.map(str::to_owned));
            if !self.post_results.is_empty() {
                return self.post_results.remove(0);
            }
            self.post_result
                .clone()
                .unwrap_or(Err(EndpointTransportError::Other))
        }
    }

    #[derive(Default)]
    struct ConcurrencyState {
        current: u32,
        peak: u32,
        post_timeouts: Vec<Duration>,
    }

    #[derive(Clone)]
    struct HoldingTransport {
        state: Arc<(Mutex<ConcurrencyState>, Condvar)>,
        hold: Duration,
    }

    impl EndpointTransport for HoldingTransport {
        fn get(
            &mut self,
            _base_url: &str,
            _path: &str,
            _credential: Option<&str>,
            _timeout: Duration,
        ) -> Result<HttpResponse, EndpointTransportError> {
            Err(EndpointTransportError::Other)
        }

        fn post_json(
            &mut self,
            _base_url: &str,
            _path: &str,
            _body: &Value,
            _credential: Option<&str>,
            timeout: Duration,
        ) -> Result<HttpResponse, EndpointTransportError> {
            let (state, started) = &*self.state;
            {
                let mut state = state.lock().expect("concurrency state lock");
                state.current += 1;
                state.peak = state.peak.max(state.current);
                state.post_timeouts.push(timeout);
                if state.current == 2 {
                    started.notify_all();
                }
            }
            thread::sleep(self.hold);
            state.lock().expect("concurrency state lock").current -= 1;
            Ok(response())
        }
    }

    fn endpoint(base_url: &str) -> ByoEndpoint {
        ByoEndpoint {
            base_url: base_url.to_owned(),
            served_model_id: "served".into(),
            credential: None,
            parallel_slots: Some(1),
            is_confidential: false,
        }
    }

    fn request(timeout_s: Option<f64>) -> GenerateRequest {
        GenerateRequest {
            id: None,
            context: "test.generate".into(),
            contents: vec![ContentPart::Text {
                text: "Hello".into(),
            }],
            system_instruction: None,
            temperature: 0.2,
            max_output_tokens: 64,
            thinking_budget: None,
            timeout_s,
            json_output: false,
            json_schema: None,
            enforce_responsiveness: false,
            attempt_index: 0,
            exclusive_admission: false,
            transport_retries: None,
        }
    }

    fn journal_path() -> std::path::PathBuf {
        let suffix = NEXT_JOURNAL.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "solstone-endpoint-wire-{}-{suffix}",
            std::process::id()
        ))
    }

    fn response() -> HttpResponse {
        HttpResponse {
            status: 200,
            body: json!({
                "choices": [{"message": {"content": "Done"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 3, "completion_tokens": 2, "total_tokens": 5},
            })
            .to_string(),
        }
    }

    fn bad_request(body: &str) -> HttpResponse {
        HttpResponse {
            status: 400,
            body: body.into(),
        }
    }

    fn served_window_config() -> Map<String, Value> {
        json!({"providers": {"local": {"served_context_window": 4096}}})
            .as_object()
            .unwrap()
            .clone()
    }

    #[test]
    fn zero_or_absent_request_timeout_uses_the_default() {
        for timeout_s in [None, Some(0.0)] {
            assert_eq!(request_timeout(timeout_s), DEFAULT_TIMEOUT);
        }
    }

    #[test]
    fn discovery_failure_does_not_block_generation_or_add_budgets() {
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let mut transport = StubTransport {
            get_result: Some(Err(EndpointTransportError::Other)),
            post_result: Some(Ok(response())),
            ..Default::default()
        };
        let result = endpoint_generate_with(
            &request(None),
            &journal,
            &endpoint("http://endpoint"),
            &Map::new(),
            &runtime,
            &mut transport,
            Instant::now(),
        );
        let EndpointResult::Generated(generated) = result else {
            panic!("discovery failure must not block generation");
        };
        assert_eq!(generated.model, "served");
        assert_eq!(generated.input_budget, None);
        assert_eq!(generated.request_budget, None);
        assert_eq!(transport.get_calls, 1);
        assert_eq!(transport.posts.len(), 1);
        for field in [
            "chat_template_kwargs",
            "top_p",
            "top_k",
            "min_p",
            "presence_penalty",
        ] {
            assert!(
                !transport.posts[0].get(field).is_some(),
                "unexpected {field}"
            );
        }
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn served_window_discovery_is_cached_within_ttl() {
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let now = Instant::now();
        let mut transport = StubTransport {
            get_result: Some(Ok(HttpResponse {
                status: 200,
                body: json!({"data": [{"id": "served", "max_model_len": 4096}]}).to_string(),
            })),
            post_result: Some(Ok(response())),
            ..Default::default()
        };
        for _ in 0..2 {
            assert!(matches!(
                endpoint_generate_with(
                    &request(None),
                    &journal,
                    &endpoint("http://endpoint"),
                    &Map::new(),
                    &runtime,
                    &mut transport,
                    now,
                ),
                EndpointResult::Generated(_)
            ));
        }
        assert_eq!(transport.get_calls, 1);
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn invalid_endpoint_response_uses_contract_provider_response_invalid() {
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let mut transport = StubTransport {
            post_result: Some(Ok(HttpResponse {
                status: 200,
                body: "{}".into(),
            })),
            ..Default::default()
        };
        assert_eq!(
            endpoint_generate_with(
                &request(None),
                &journal,
                &endpoint("http://endpoint"),
                &served_window_config(),
                &runtime,
                &mut transport,
                Instant::now(),
            ),
            failure("provider_response_invalid")
        );
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn overflow_reclamps_once_then_generates() {
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let overflow = "maximum context length of 1000 tokens: 600 tokens from the input messages and 400 tokens for the completion";
        let mut transport = StubTransport {
            post_results: vec![Ok(bad_request(overflow)), Ok(response())],
            ..Default::default()
        };
        assert!(matches!(
            endpoint_generate_with(
                &request(None),
                &journal,
                &endpoint("http://endpoint"),
                &served_window_config(),
                &runtime,
                &mut transport,
                Instant::now(),
            ),
            EndpointResult::Generated(_)
        ));
        assert_eq!(transport.posts.len(), 2);
        assert_eq!(transport.posts[1]["max_tokens"], 384);
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn repeated_overflow_is_context_window_exceeded_without_a_third_post() {
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let overflow = "maximum context length of 1000 tokens: 600 tokens from the input messages and 400 tokens for the completion";
        let mut transport = StubTransport {
            post_result: Some(Ok(bad_request(overflow))),
            ..Default::default()
        };
        assert_eq!(
            endpoint_generate_with(
                &request(None),
                &journal,
                &endpoint("http://endpoint"),
                &served_window_config(),
                &runtime,
                &mut transport,
                Instant::now(),
            ),
            failure("context_window_exceeded")
        );
        assert_eq!(transport.posts.len(), 2);
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn too_small_reclamp_is_context_budget_exceeded_without_retry() {
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let overflow = "maximum context length of 1000 tokens: 800 tokens from the input messages and 400 tokens for the completion";
        let mut transport = StubTransport {
            post_result: Some(Ok(bad_request(overflow))),
            ..Default::default()
        };
        assert_eq!(
            endpoint_generate_with(
                &request(None),
                &journal,
                &endpoint("http://endpoint"),
                &served_window_config(),
                &runtime,
                &mut transport,
                Instant::now(),
            ),
            failure("context_budget_exceeded")
        );
        assert_eq!(transport.posts.len(), 1);
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn context_and_contract_400s_are_not_retried() {
        for (body, reason_code) in [
            (
                "request exceeds the context window",
                "context_window_exceeded",
            ),
            (
                "unexpected endpoint response",
                "local_endpoint_contract_failed",
            ),
        ] {
            let runtime = EndpointRuntime::default();
            let journal = journal_path();
            let mut transport = StubTransport {
                post_result: Some(Ok(bad_request(body))),
                ..Default::default()
            };
            assert_eq!(
                endpoint_generate_with(
                    &request(None),
                    &journal,
                    &endpoint("http://endpoint"),
                    &served_window_config(),
                    &runtime,
                    &mut transport,
                    Instant::now(),
                ),
                failure(reason_code)
            );
            assert_eq!(transport.posts.len(), 1);
            let _ = std::fs::remove_dir_all(journal);
        }
    }

    #[test]
    fn credentials_are_threaded_and_provider_text_never_reaches_refusal_detail() {
        let credential = "endpoint-secret";
        for configured in [Some(credential), None] {
            let runtime = EndpointRuntime::default();
            let journal = journal_path();
            let mut endpoint = endpoint("http://endpoint");
            endpoint.credential = configured.map(str::to_owned);
            let mut transport = StubTransport {
                get_result: Some(Ok(HttpResponse {
                    status: 200,
                    body: json!({"data": []}).to_string(),
                })),
                post_result: Some(Ok(bad_request(&format!("invalid credential {credential}")))),
                ..Default::default()
            };
            let EndpointResult::Failed(failure) = endpoint_generate_with(
                &request(None),
                &journal,
                &endpoint,
                &Map::new(),
                &runtime,
                &mut transport,
                Instant::now(),
            ) else {
                panic!("plain 400 must refuse");
            };
            let refusal =
                crate::refusal_for(&crate::LaneOutcome::EndpointFailure(failure), "local", None);
            assert_eq!(refusal.detail, "fixture provider-response-invalid");
            assert!(!refusal.detail.contains(credential));
            assert_eq!(
                transport.get_credentials,
                vec![configured.map(str::to_owned)]
            );
            assert_eq!(
                transport.post_credentials,
                vec![configured.map(str::to_owned)]
            );
            let _ = std::fs::remove_dir_all(journal);
        }
    }

    #[test]
    fn refused_connection_is_endpoint_unreachable() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        assert_eq!(
            endpoint_generate_with(
                &request(Some(0.05)),
                &journal,
                &endpoint(&format!("http://{address}")),
                &served_window_config(),
                &runtime,
                &mut UreqEndpointTransport,
                Instant::now(),
            ),
            failure("local_endpoint_unreachable")
        );
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn other_post_failure_is_provider_response_invalid() {
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let mut transport = StubTransport {
            post_result: Some(Err(EndpointTransportError::Other)),
            ..Default::default()
        };
        assert_eq!(
            endpoint_generate_with(
                &request(None),
                &journal,
                &endpoint("http://endpoint"),
                &served_window_config(),
                &runtime,
                &mut transport,
                Instant::now(),
            ),
            failure("provider_response_invalid")
        );
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn response_timeout_after_connection_is_capacity_exhausted() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request);
            thread::sleep(Duration::from_millis(150));
        });
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        assert_eq!(
            endpoint_generate_with(
                &request(Some(0.05)),
                &journal,
                &endpoint(&format!("http://{address}")),
                &served_window_config(),
                &runtime,
                &mut UreqEndpointTransport,
                Instant::now(),
            ),
            failure("local_capacity_exhausted")
        );
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn admission_limits_concurrency_and_spends_queued_timeout() {
        let journal = journal_path();
        let cleanup_journal = journal.clone();
        let endpoint = ByoEndpoint {
            parallel_slots: Some(2),
            ..endpoint("http://endpoint")
        };
        let request = request(Some(10.0));
        let config = served_window_config();
        let state = Arc::new((Mutex::new(ConcurrencyState::default()), Condvar::new()));
        let transport = HoldingTransport {
            state: Arc::clone(&state),
            hold: Duration::from_millis(200),
        };
        let mut workers = Vec::new();
        for _ in 0..2 {
            let journal = journal.clone();
            let endpoint = endpoint.clone();
            let request = request.clone();
            let config = config.clone();
            let mut transport = transport.clone();
            workers.push(thread::spawn(move || {
                endpoint_generate_with(
                    &request,
                    &journal,
                    &endpoint,
                    &config,
                    &EndpointRuntime::default(),
                    &mut transport,
                    Instant::now(),
                )
            }));
        }
        let (state_lock, started) = &*state;
        let mut guard = state_lock.lock().expect("concurrency state lock");
        while guard.current < 2 {
            guard = started.wait(guard).expect("concurrency state wait");
        }
        drop(guard);
        let journal = journal.clone();
        let endpoint = endpoint.clone();
        let request = request.clone();
        let config = config.clone();
        let mut transport = transport.clone();
        workers.push(thread::spawn(move || {
            endpoint_generate_with(
                &request,
                &journal,
                &endpoint,
                &config,
                &EndpointRuntime::default(),
                &mut transport,
                Instant::now(),
            )
        }));
        for worker in workers {
            assert!(matches!(
                worker.join().expect("join endpoint worker"),
                EndpointResult::Generated(_)
            ));
        }
        let state = state_lock.lock().expect("concurrency state lock");
        assert!(state.peak <= 2, "peak admission depth: {}", state.peak);
        assert_eq!(state.post_timeouts.len(), 3);
        let queued_wait = state.post_timeouts[0]
            .checked_sub(state.post_timeouts[2])
            .expect("queued request receives less post timeout");
        assert!(
            queued_wait >= Duration::from_millis(150),
            "queued post timeout was only reduced by {queued_wait:?}"
        );
        let _ = std::fs::remove_dir_all(cleanup_journal);
    }

    #[test]
    fn transport_timeout_releases_admission_for_the_next_request() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request);
            thread::sleep(Duration::from_millis(150));
        });
        let journal = journal_path();
        let config = served_window_config();
        assert_eq!(
            endpoint_generate_with(
                &request(Some(0.05)),
                &journal,
                &endpoint(&format!("http://{address}")),
                &config,
                &EndpointRuntime::default(),
                &mut UreqEndpointTransport,
                Instant::now(),
            ),
            failure("local_capacity_exhausted")
        );
        server.join().unwrap();
        let mut transport = StubTransport {
            post_result: Some(Ok(response())),
            ..Default::default()
        };
        assert!(matches!(
            endpoint_generate_with(
                &request(Some(0.2)),
                &journal,
                &endpoint("http://endpoint"),
                &config,
                &EndpointRuntime::default(),
                &mut transport,
                Instant::now(),
            ),
            EndpointResult::Generated(_)
        ));
        let _ = std::fs::remove_dir_all(journal);
    }
}

#[cfg(test)]
mod vocabulary_tests {
    #[test]
    fn endpoint_production_uses_shared_response_and_models_primitives() {
        let production = include_str!("endpoint.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("endpoint module has a production prefix");
        for member in [
            "choices",
            "finish_reason",
            "prompt_tokens",
            "completion_tokens",
            "max_model_len",
        ] {
            let quoted_member = format!("\"{member}\"");
            assert!(
                !production.contains(&quoted_member),
                "endpoint production must use a shared primitive for {member:?}"
            );
        }
    }
}
