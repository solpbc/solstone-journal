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
use solstone_core_spp_ratls::AttestationStateStore;

use crate::NON_RESPONSIVE_RAW_OUTPUT_CAP_CHARS;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const ENDPOINT_MODELS_TIMEOUT: Duration = Duration::from_millis(2_500);
pub const ENDPOINT_SERVED_WINDOW_CACHE_TTL: Duration = Duration::from_secs(300);
const SERVED_CONTEXT_WINDOW_MIN_TOKENS: u32 = 2_048;
const SAFETY_MARGIN_TOKENS: u32 = 256;
const MIN_COMPLETION_TOKENS: u32 = 256;
const ESTIMATED_IMAGE_TOKENS: u32 = 2_500;
const RECLAMP_SLACK_TOKENS: u32 = 16;
/// How many times a context refusal may re-fit the prompt against a smaller window.
const MAX_CONTEXT_REFITS: u32 = 4;
pub const UNKNOWN_WINDOW_TOKENS: u32 = 32_768;
pub const CONFIDENTIAL_WINDOW_TOKENS: u32 = 262_144;
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
    pub detail: Option<String>,
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
    attestation_state: AttestationStateStore,
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

pub(crate) fn endpoint_generate_with<T: EndpointTransport>(
    request: &GenerateRequest,
    journal_path: &Path,
    endpoint: &ByoEndpoint,
    config: &Map<String, Value>,
    runtime: &EndpointRuntime,
    transport: &mut T,
    now: Instant,
) -> EndpointResult {
    let Some(max_tokens) = completion_ceiling(request, endpoint, config) else {
        return failure("provider_response_invalid");
    };
    let served_window = runtime.resolve_served_window(endpoint, config, transport, now);
    let mut prepared = match prepare_endpoint_request(request, endpoint, max_tokens, served_window)
    {
        Ok(prepared) => prepared,
        Err(reason_code) => return failure(reason_code),
    };
    let timeout = request_timeout(request.timeout_s, thinking_headroom(endpoint, config));
    let started = Instant::now();
    let Some(admission_timeout) = remaining_timeout(started, timeout) else {
        return failure("local_capacity_exhausted");
    };
    if admission_timeout.is_zero() {
        return failure("local_capacity_exhausted");
    }
    let response = {
        let _permit = if endpoint.is_confidential {
            None
        } else {
            match acquire_endpoint_slot(
                journal_path,
                endpoint,
                request.exclusive_admission,
                admission_timeout,
            ) {
                Ok(permit) => Some(permit),
                Err(reason_code) => return failure(reason_code),
            }
        };
        let mut refits = 0u32;
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
            // This preserves the frozen parser oracle. A context refusal means the
            // CLIENT-side fit under-counted the prompt, so re-clamping `max_tokens`
            // alone would resend byte-identical input and cannot make it fit. Halving
            // the window handed to `prepare_endpoint_request` is what actually trims
            // the INPUT -- see `MAX_CONTEXT_REFITS` for the bounded policy.
            let overflow = endpoint_overflow_decision(&response.body, Some(served_window), refits);
            match overflow {
                // A detailed refusal with less than the minimum completion room is
                // still recoverable for one-shot generation: unlike converse, this
                // path owns a trimmable input block and can refit it to make room.
                OverflowDecision::Retry(_)
                | OverflowDecision::Context
                | OverflowDecision::Budget => {}
                OverflowDecision::Contract => {
                    let rejection = terminal_request_rejection(&response.body);
                    return EndpointResult::Failed(EndpointFailure {
                        reason_code: Some("provider_request_rejected".to_owned()),
                        detail: Some(rejection.detail.to_owned()),
                    });
                }
            }
            if refits >= MAX_CONTEXT_REFITS {
                return failure("context_window_exceeded");
            }
            refits += 1;
            prepared = match prepare_endpoint_request(
                request,
                endpoint,
                max_tokens,
                served_window >> refits,
            ) {
                Ok(prepared) => prepared,
                Err(reason_code) => return failure(reason_code),
            };
        }
    };
    let secret = endpoint.credential.as_deref().unwrap_or("");
    if !(200..300).contains(&response.status) {
        let detail = capture_provider_detail(&response.body, secret);
        return EndpointResult::Failed(EndpointFailure {
            reason_code: Some(non_success_reason(endpoint, response.status).to_owned()),
            detail,
        });
    }
    let body = match serde_json::from_str::<Value>(&response.body) {
        Ok(body) => body,
        Err(_) => {
            let detail = capture_provider_detail(&response.body, secret);
            return EndpointResult::Failed(EndpointFailure {
                reason_code: Some("provider_response_invalid".to_owned()),
                detail,
            });
        }
    };
    let parsed = match parse_response(&body) {
        Ok(parsed) => parsed,
        Err(_) => {
            let detail = capture_provider_detail(&response.body, secret);
            return EndpointResult::Failed(EndpointFailure {
                reason_code: Some("provider_response_invalid".to_owned()),
                detail,
            });
        }
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

/// The completion ceiling asked of an endpoint: the talent's own visible budget,
/// plus the owner's thinking room on their own endpoint. No thinking field is
/// ever sent there, so the room is for a model that thinks on its own; bundled and
/// confidential run with thinking off and get none.
fn completion_ceiling(
    request: &GenerateRequest,
    endpoint: &ByoEndpoint,
    config: &Map<String, Value>,
) -> Option<u32> {
    u32::try_from(
        request
            .max_output_tokens
            .saturating_add(thinking_headroom(endpoint, config)),
    )
    .ok()
}

fn thinking_headroom(endpoint: &ByoEndpoint, config: &Map<String, Value>) -> u64 {
    if endpoint.is_bundled || endpoint.is_confidential {
        0
    } else {
        crate::thinking::endpoint_headroom(crate::thinking::byo_thinking(config))
    }
}

fn prepare_endpoint_request(
    request: &GenerateRequest,
    endpoint: &ByoEndpoint,
    max_tokens: u32,
    window: u32,
) -> Result<PreparedEndpointRequest, &'static str> {
    let contents = request_contents(request);
    let mut count = estimate_tokens;
    let (contents, input_budget) = fit_contents(
        &contents,
        request.system_instruction.as_deref(),
        max_tokens,
        window,
        &mut count,
    )
    .map_err(|_| "context_budget_exceeded")?;
    let messages = build_messages(&contents, request.system_instruction.as_deref());
    let estimated_prompt_tokens = estimate_tokens(&serialized_message_text(&messages));
    let image_tokens = ESTIMATED_IMAGE_TOKENS.saturating_mul(count_image_parts(&contents));
    let room = window
        .saturating_sub(estimated_prompt_tokens)
        .saturating_sub(image_tokens)
        .saturating_sub(SAFETY_MARGIN_TOKENS);
    if room < MIN_COMPLETION_TOKENS {
        return Err("context_budget_exceeded");
    }
    let clamped_max_tokens = max_tokens.min(room);
    let request_budget = Some(RequestBudget {
        window,
        // Confidential calls create a fresh attested channel, not a
        // shared local endpoint slot; this only records budget metadata.
        slots: endpoint.parallel_slots.unwrap_or(1),
        estimated_prompt_tokens,
        image_tokens,
        clamped_max_tokens,
        requested_max_output_tokens: max_tokens,
    });
    let max_tokens = clamped_max_tokens;
    Ok(PreparedEndpointRequest {
        body: build_request_body(
            &endpoint.served_model_id,
            build_messages(&contents, request.system_instruction.as_deref()),
            request.temperature,
            max_tokens,
            request.json_output,
            request.json_schema.as_ref(),
            // Bundled generate never reaches this endpoint path. Only the
            // confidential lane's directly attested channel is distinguished
            // from a plain BYO endpoint here.
            endpoint.is_confidential,
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
        // Confidential calls have no shared local admission resource.
        endpoint.parallel_slots.unwrap_or(1),
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

pub(crate) const REQUEST_REJECTED_DETAIL: &str = "the local endpoint rejected the request";
pub(crate) const REQUEST_REJECTED_SCHEMA_DETAIL: &str =
    "the local endpoint rejected the structured-output schema";
pub(crate) const DIAGNOSTIC_CLASS_SCHEMA: &str = "structured_output_schema_rejection";
pub(crate) const DIAGNOSTIC_CLASS_GENERIC: &str = "request_rejected";
const SCHEMA_REJECTION_PREFIX: &str = "Failed to compile json grammar:";

pub(crate) struct RequestRejection {
    pub(crate) diagnostic_class: &'static str,
    pub(crate) detail: &'static str,
}

pub(crate) fn terminal_request_rejection(body: &str) -> RequestRejection {
    let rejection = if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(body) {
        let is_bad_request = map.get("type").and_then(Value::as_str) == Some("BadRequestError");
        let is_400 = map.get("code").and_then(Value::as_u64) == Some(400);
        let is_schema_prefix = map
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|msg| msg.starts_with(SCHEMA_REJECTION_PREFIX));
        if is_bad_request && is_400 && is_schema_prefix {
            RequestRejection {
                diagnostic_class: DIAGNOSTIC_CLASS_SCHEMA,
                detail: REQUEST_REJECTED_SCHEMA_DETAIL,
            }
        } else {
            RequestRejection {
                diagnostic_class: DIAGNOSTIC_CLASS_GENERIC,
                detail: REQUEST_REJECTED_DETAIL,
            }
        }
    } else {
        RequestRejection {
            diagnostic_class: DIAGNOSTIC_CLASS_GENERIC,
            detail: REQUEST_REJECTED_DETAIL,
        }
    };

    log::warn!(
        "{}",
        serde_json::json!({
            "status": 400,
            "provider": "local",
            "diagnostic_class": rejection.diagnostic_class,
        })
    );

    rejection
}

impl EndpointRuntime {
    pub(crate) fn attestation_state(&self) -> &AttestationStateStore {
        &self.attestation_state
    }

    fn resolve_served_window<T: EndpointTransport>(
        &self,
        endpoint: &ByoEndpoint,
        config: &Map<String, Value>,
        transport: &mut T,
        now: Instant,
    ) -> u32 {
        if endpoint.is_confidential {
            return CONFIDENTIAL_WINDOW_TOKENS;
        }
        // The owner's configured window is authoritative for their endpoint. The
        // bundled lane puts its own running server's window in this slot of its
        // private config copy before calling, so an owner value never reaches it.
        if let Some(window) = configured_served_context_window(config) {
            return window;
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
            return value.unwrap_or(UNKNOWN_WINDOW_TOKENS);
        }
        let value = discover_served_window(endpoint, transport);
        self.served_windows
            .lock()
            .expect("endpoint served-window cache lock poisoned")
            .insert(key, (value, now));
        value.unwrap_or(UNKNOWN_WINDOW_TOKENS)
    }
}

pub(crate) fn configured_served_context_window(config: &Map<String, Value>) -> Option<u32> {
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

/// The caller's timeout, else the lane default plus time for any thinking room.
fn request_timeout(timeout_s: Option<f64>, thinking_room: u64) -> Duration {
    timeout_s
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .map(Duration::from_secs_f64)
        .unwrap_or(DEFAULT_TIMEOUT + crate::thinking::thinking_time(thinking_room))
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

/// The reason for a non-success reply from the endpoint. An owner-supplied
/// endpoint that answers 401 is refusing the credential, which the owner can fix,
/// so it is named as a key problem, the same as the OpenAI and Anthropic presets:
/// a refused key when one is set, a missing one when none is. The bundled and
/// confidential lanes keep the generic reason: their credential is ours.
fn non_success_reason(endpoint: &ByoEndpoint, status: u16) -> &'static str {
    if status != 401 || endpoint.is_bundled || endpoint.is_confidential {
        return "provider_response_invalid";
    }
    if endpoint.credential.is_some() {
        "provider_key_invalid"
    } else {
        "provider_key_missing"
    }
}

fn failure(reason_code: &str) -> EndpointResult {
    EndpointResult::Failed(EndpointFailure {
        reason_code: Some(reason_code.to_owned()),
        detail: None,
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
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar};
    use std::thread;

    use serde_json::json;
    use solstone_core_generate::ReasonCodeValue;

    use super::*;

    static NEXT_JOURNAL: AtomicUsize = AtomicUsize::new(0);

    #[derive(Default)]
    struct GateState {
        current: u32,
        peak: u32,
        release: bool,
        records: Vec<(Value, Duration, bool)>,
    }

    struct AdmissionGate {
        inner: Mutex<GateState>,
        entered: Condvar,
        released: Condvar,
    }

    struct GateEntry<'a> {
        gate: &'a AdmissionGate,
    }

    impl Drop for GateEntry<'_> {
        fn drop(&mut self) {
            let mut state = match self.gate.inner.lock() {
                Ok(state) => state,
                Err(poisoned) => poisoned.into_inner(),
            };
            state.current = state.current.saturating_sub(1);
            self.gate.entered.notify_all();
        }
    }

    struct ReleaseOnDrop {
        gate: Arc<AdmissionGate>,
    }

    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            let mut state = match self.gate.inner.lock() {
                Ok(state) => state,
                Err(poisoned) => poisoned.into_inner(),
            };
            state.release = true;
            self.gate.released.notify_all();
        }
    }

    #[derive(Clone, Default)]
    struct StubTransport {
        get_script: Vec<Result<HttpResponse, EndpointTransportError>>,
        post_script: Vec<Result<HttpResponse, EndpointTransportError>>,
        get_calls: usize,
        posts: Vec<Value>,
        get_credentials: Vec<Option<String>>,
        post_credentials: Vec<Option<String>>,
        gate: Option<Arc<AdmissionGate>>,
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
            if !self.get_script.is_empty() {
                return self.get_script.remove(0);
            }
            Err(EndpointTransportError::Other)
        }

        fn post_json(
            &mut self,
            _base_url: &str,
            _path: &str,
            body: &Value,
            credential: Option<&str>,
            timeout: Duration,
        ) -> Result<HttpResponse, EndpointTransportError> {
            self.posts.push(body.clone());
            self.post_credentials.push(credential.map(str::to_owned));
            let _entry = self.gate.as_ref().map(|gate| {
                {
                    let mut state = gate.inner.lock().expect("admission gate lock");
                    state.current += 1;
                    state.peak = state.peak.max(state.current);
                    let after_release = state.release;
                    state.records.push((body.clone(), timeout, after_release));
                    if state.current == 2 {
                        gate.entered.notify_all();
                    }
                }
                let entry = GateEntry { gate };
                {
                    let mut state = gate.inner.lock().expect("admission gate lock");
                    while !state.release {
                        state = gate.released.wait(state).expect("admission gate wait");
                    }
                }
                entry
            });
            if !self.post_script.is_empty() {
                return self.post_script.remove(0);
            }
            Err(EndpointTransportError::Other)
        }
    }

    struct TestLogger;
    static LOGGER: TestLogger = TestLogger;
    static LOGGER_INIT: std::sync::Once = std::sync::Once::new();
    // Each line keeps the thread that logged it, so a test reads only its own warnings
    // even when other tests log at the same time.
    type CapturedLine = (std::thread::ThreadId, String);
    static LOGS: std::sync::OnceLock<std::sync::Mutex<Vec<CapturedLine>>> =
        std::sync::OnceLock::new();
    static WARN_TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    impl log::Log for TestLogger {
        fn enabled(&self, metadata: &log::Metadata) -> bool {
            metadata.level() <= log::Level::Warn
        }
        fn log(&self, record: &log::Record) {
            if self.enabled(record.metadata())
                && let Ok(mut logs) = LOGS
                    .get_or_init(|| std::sync::Mutex::new(Vec::new()))
                    .lock()
            {
                logs.push((std::thread::current().id(), record.args().to_string()));
            }
        }
        fn flush(&self) {}
    }

    fn install_warn_capture() -> std::sync::MutexGuard<'static, ()> {
        let guard = WARN_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        LOGGER_INIT.call_once(|| {
            let _ = log::set_logger(&LOGGER);
            log::set_max_level(log::LevelFilter::Warn);
        });
        LOGS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        guard
    }

    fn captured_warns() -> Vec<String> {
        let current = std::thread::current().id();
        LOGS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(thread, _)| *thread == current)
            .map(|(_, line)| line.clone())
            .collect()
    }

    fn endpoint(base_url: &str) -> ByoEndpoint {
        ByoEndpoint {
            base_url: base_url.to_owned(),
            served_model_id: "served".into(),
            credential: None,
            parallel_slots: Some(1),
            is_confidential: false,
            is_bundled: false,
        }
    }

    const QWEN_SAMPLING_FIELDS: [&str; 5] = [
        "chat_template_kwargs",
        "top_p",
        "top_k",
        "min_p",
        "presence_penalty",
    ];

    #[test]
    fn qwen_sampling_controls_follow_the_endpoint_lane_flags() {
        for (is_confidential, expected) in [(false, false), (true, true)] {
            let mut endpoint = endpoint("http://endpoint");
            endpoint.is_confidential = is_confidential;
            let journal = journal_path();
            let request = request(None);
            let runtime = EndpointRuntime::default();
            let config = served_window_config();
            let mut transport = StubTransport {
                post_script: vec![Ok(response())],
                ..Default::default()
            };
            endpoint_generate_with(
                &request,
                &journal,
                &endpoint,
                &config,
                &runtime,
                &mut transport,
                Instant::now(),
            );
            let body = transport.posts.remove(0);
            for field in QWEN_SAMPLING_FIELDS {
                assert_eq!(
                    body.get(field).is_some(),
                    expected,
                    "is_confidential={is_confidential}: {field}"
                );
            }
            assert!(body.get("model").is_some(), "model is always present");
            let _ = std::fs::remove_dir_all(journal);
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

    fn models_response(model_id: &str, max_model_len: u32) -> HttpResponse {
        HttpResponse {
            status: 200,
            body: json!({"data": [{"id": model_id, "max_model_len": max_model_len}]}).to_string(),
        }
    }

    fn wait_ticket_names(journal: &Path) -> BTreeSet<String> {
        std::fs::read_dir(admission_dir(journal))
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .filter(|name| name.starts_with("wait-") && name.ends_with(".ticket"))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn zero_or_absent_request_timeout_uses_the_default() {
        for timeout_s in [None, Some(0.0)] {
            assert_eq!(request_timeout(timeout_s, 0), DEFAULT_TIMEOUT);
        }
    }

    #[test]
    fn discovery_failure_does_not_block_generation_and_fits_the_baseline_window() {
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let mut transport = StubTransport {
            get_script: vec![Err(EndpointTransportError::Other)],
            post_script: vec![Ok(response())],
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
        assert_eq!(
            generated.request_budget.map(|budget| budget.window),
            Some(UNKNOWN_WINDOW_TOKENS)
        );
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
        let mut wide = request(None);
        wide.max_output_tokens = 4_000;
        let mut transport = StubTransport {
            get_script: vec![
                Ok(models_response("served", 4096)),
                Ok(models_response("served", 8192)),
                Ok(models_response("other", 2048)),
                Ok(models_response("served", 4096)),
            ],
            post_script: vec![
                Ok(response()),
                Ok(response()),
                Ok(response()),
                Ok(response()),
                Ok(response()),
                Ok(response()),
            ],
            ..Default::default()
        };
        let endpoint_a = endpoint("http://endpoint-a");
        let endpoint_b = endpoint("http://endpoint-b");
        let mut endpoint_other = endpoint("http://endpoint-a");
        endpoint_other.served_model_id = "other".into();

        assert!(matches!(
            endpoint_generate_with(
                &wide,
                &journal,
                &endpoint_a,
                &Map::new(),
                &runtime,
                &mut transport,
                now,
            ),
            EndpointResult::Generated(_)
        ));
        assert_eq!(transport.get_calls, 1);
        let max_tokens_am = transport.posts[0]["max_tokens"].clone();

        assert!(matches!(
            endpoint_generate_with(
                &wide,
                &journal,
                &endpoint_a,
                &Map::new(),
                &runtime,
                &mut transport,
                now,
            ),
            EndpointResult::Generated(_)
        ));
        assert_eq!(transport.get_calls, 1);
        assert_eq!(transport.posts[1]["max_tokens"], max_tokens_am);

        assert!(matches!(
            endpoint_generate_with(
                &wide,
                &journal,
                &endpoint_b,
                &Map::new(),
                &runtime,
                &mut transport,
                now,
            ),
            EndpointResult::Generated(_)
        ));
        assert_eq!(transport.get_calls, 2);
        let max_tokens_bm = transport.posts[2]["max_tokens"].clone();
        assert_ne!(max_tokens_bm, max_tokens_am);

        assert!(matches!(
            endpoint_generate_with(
                &wide,
                &journal,
                &endpoint_other,
                &Map::new(),
                &runtime,
                &mut transport,
                now,
            ),
            EndpointResult::Generated(_)
        ));
        assert_eq!(transport.get_calls, 3);
        let max_tokens_an = transport.posts[3]["max_tokens"].clone();
        assert_ne!(max_tokens_an, max_tokens_am);
        assert_ne!(max_tokens_an, max_tokens_bm);

        assert!(matches!(
            endpoint_generate_with(
                &wide,
                &journal,
                &endpoint_a,
                &Map::new(),
                &runtime,
                &mut transport,
                now,
            ),
            EndpointResult::Generated(_)
        ));
        assert_eq!(transport.get_calls, 3);
        assert_eq!(transport.posts[4]["max_tokens"], max_tokens_am);

        let expired = now
            .checked_add(ENDPOINT_SERVED_WINDOW_CACHE_TTL)
            .and_then(|instant| instant.checked_add(Duration::from_nanos(1)))
            .expect("served-window TTL fits Instant");
        assert!(matches!(
            endpoint_generate_with(
                &wide,
                &journal,
                &endpoint_a,
                &Map::new(),
                &runtime,
                &mut transport,
                expired,
            ),
            EndpointResult::Generated(_)
        ));
        assert_eq!(transport.get_calls, 4);
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn served_window_text_first_image_is_not_counted_as_preserved_text() {
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let data = "x".repeat(12_000);
        let mut request = request(None);
        request.contents = vec![
            ContentPart::Text {
                text: "short".into(),
            },
            ContentPart::Image {
                mime_type: "image/png".into(),
                data: data.clone(),
            },
        ];
        let mut transport = StubTransport {
            post_script: vec![Ok(response())],
            ..Default::default()
        };
        let result = endpoint_generate_with(
            &request,
            &journal,
            &endpoint("http://endpoint"),
            &served_window_config(),
            &runtime,
            &mut transport,
            Instant::now(),
        );
        let EndpointResult::Generated(generated) = result else {
            panic!("served-window image request must succeed");
        };

        assert_eq!(transport.posts.len(), 1);
        assert_eq!(
            transport.posts[0]["messages"][0]["content"][1]["image_url"]["url"],
            json!(format!("data:image/png;base64,{data}"))
        );
        assert_eq!(
            generated
                .request_budget
                .expect("served-window request budget")
                .image_tokens,
            ESTIMATED_IMAGE_TOKENS
        );
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn an_endpoint_refusing_the_owners_credential_is_a_key_problem() {
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let keyed = ByoEndpoint {
            credential: Some("owner-key".into()),
            ..endpoint("http://endpoint")
        };
        let bundled = ByoEndpoint {
            is_bundled: true,
            ..keyed.clone()
        };
        let cases = [
            (keyed.clone(), 401, "provider_key_invalid"),
            (endpoint("http://endpoint"), 401, "provider_key_missing"),
            (keyed.clone(), 403, "provider_response_invalid"),
            (keyed, 500, "provider_response_invalid"),
            (bundled, 401, "provider_response_invalid"),
        ];
        for (target, status, expected) in cases {
            let mut transport = StubTransport {
                post_script: vec![Ok(HttpResponse {
                    status,
                    body: r#"{"error":{"message":"Authentication failed"}}"#.into(),
                })],
                ..Default::default()
            };
            let result = endpoint_generate_with(
                &request(None),
                &journal,
                &target,
                &served_window_config(),
                &runtime,
                &mut transport,
                Instant::now(),
            );
            let EndpointResult::Failed(failed) = result else {
                panic!("status {status} must refuse");
            };
            assert_eq!(
                failed.reason_code.as_deref(),
                Some(expected),
                "status {status}"
            );
        }
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn invalid_endpoint_response_uses_contract_provider_response_invalid() {
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let mut transport = StubTransport {
            post_script: vec![Ok(HttpResponse {
                status: 200,
                body: "{}".into(),
            })],
            ..Default::default()
        };
        let result = endpoint_generate_with(
            &request(None),
            &journal,
            &endpoint("http://endpoint"),
            &served_window_config(),
            &runtime,
            &mut transport,
            Instant::now(),
        );
        let EndpointResult::Failed(failed) = result else {
            panic!("empty object must refuse");
        };
        assert_eq!(
            failed.reason_code.as_deref(),
            Some("provider_response_invalid")
        );
        let refusal =
            crate::refusal_for(&crate::LaneOutcome::EndpointFailure(failed), "local", None);
        assert_eq!(refusal.detail, "{}");
        let _ = std::fs::remove_dir_all(journal);
    }

    /// A context refusal is the endpoint telling us the client-side fit under-counted
    /// the prompt, so generate re-fits it against a halved window and re-posts.
    /// Re-clamping `max_tokens` alone would resend byte-identical input.
    #[test]
    fn detailed_context_overflow_refits_the_prompt_for_generate() {
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let overflow = "maximum context length of 1000 tokens: 600 tokens from the input messages and 400 tokens for the completion";
        let mut transport = StubTransport {
            post_script: vec![Ok(bad_request(overflow)), Ok(response())],
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
        assert_eq!(transport.posts[0]["max_tokens"], 64);
        let _ = std::fs::remove_dir_all(journal);
    }

    /// JSON-heavy requests can require more than one shrink because the client
    /// estimator does not see all provider-side schema and escaping overhead.
    #[test]
    fn repeated_context_refusals_keep_shrinking_until_the_endpoint_accepts() {
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let overflow = "maximum context length of 1000 tokens: 600 tokens from the input messages and 400 tokens for the completion";
        let mut transport = StubTransport {
            post_script: vec![
                Ok(bad_request(overflow)),
                Ok(bad_request(overflow)),
                Ok(response()),
            ],
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
        assert_eq!(transport.posts.len(), 3);
        let first = transport.posts[0]["messages"].to_string().len();
        let second = transport.posts[1]["messages"].to_string().len();
        let third = transport.posts[2]["messages"].to_string().len();
        assert!(third <= second && second <= first);
        let _ = std::fs::remove_dir_all(journal);
    }

    /// The retry sequence is still finite when every progressively smaller body
    /// is refused by the endpoint.
    #[test]
    fn context_refit_exhaustion_is_terminal_without_an_extra_post() {
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let overflow = "request exceeds the context window";
        let config = json!({"providers":{"local":{"served_context_window":65_536}}})
            .as_object()
            .unwrap()
            .clone();
        let mut transport = StubTransport {
            post_script: (0..=MAX_CONTEXT_REFITS)
                .map(|_| Ok(bad_request(overflow)))
                .collect(),
            ..Default::default()
        };
        assert_eq!(
            endpoint_generate_with(
                &request(None),
                &journal,
                &endpoint("http://endpoint"),
                &config,
                &runtime,
                &mut transport,
                Instant::now(),
            ),
            failure("context_window_exceeded")
        );
        assert_eq!(transport.posts.len(), MAX_CONTEXT_REFITS as usize + 1);
        let _ = std::fs::remove_dir_all(journal);
    }

    /// The point of the refit is that the INPUT gets smaller. A prompt far larger
    /// than the window is trimmed by the first fit and trimmed again by the refit.
    #[test]
    fn the_refit_actually_shrinks_the_prompt() {
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let mut oversized = request(None);
        // A completion budget the fitter can actually leave room for: the shared
        // `request()` helper asks for 64, below `MIN_COMPLETION_TOKENS`, so any
        // trimmed prompt would be refused before it was ever posted.
        oversized.max_output_tokens = 512;
        oversized.contents = vec![ContentPart::Text {
            text: "lorem ipsum dolor sit amet ".repeat(4_000),
        }];
        let mut transport = StubTransport {
            post_script: vec![
                Ok(bad_request("request exceeds the context window")),
                Ok(response()),
            ],
            ..Default::default()
        };
        assert!(matches!(
            endpoint_generate_with(
                &oversized,
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
        let first = transport.posts[0]["messages"].to_string().len();
        let second = transport.posts[1]["messages"].to_string().len();
        assert!(
            second < first,
            "refit must shrink the prompt: first={first} second={second}"
        );
        let _ = std::fs::remove_dir_all(journal);
    }

    fn no_window_models() -> Result<HttpResponse, EndpointTransportError> {
        // A models payload carrying no `max_model_len` leaves the window unknown.
        Ok(HttpResponse {
            status: 200,
            body: json!({"data": [{"id": "served"}]}).to_string(),
        })
    }

    /// Bedrock's shape: no usable `/models`, and a day far larger than the
    /// model's window. The input is fitted to the 32k baseline instead of
    /// failing on the endpoint.
    #[test]
    fn an_unknown_window_fits_input_and_clamps_the_reply_against_32k() {
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let mut long_day = request(None);
        long_day.max_output_tokens = 2_048;
        long_day.contents = vec![ContentPart::Text {
            text: "the quick brown fox jumps over the lazy dog. ".repeat(20_000),
        }];
        let mut transport = StubTransport {
            get_script: vec![no_window_models()],
            post_script: vec![Ok(response())],
            ..Default::default()
        };
        let EndpointResult::Generated(generated) = endpoint_generate_with(
            &long_day,
            &journal,
            &endpoint("http://endpoint"),
            &Map::new(),
            &runtime,
            &mut transport,
            Instant::now(),
        ) else {
            panic!("an unknown window must still generate");
        };
        assert_eq!(transport.posts.len(), 1);
        let budget = generated.request_budget.expect("budget recorded");
        assert_eq!(budget.window, UNKNOWN_WINDOW_TOKENS);
        assert!(generated.input_budget.expect("input fitted").clipped);
        let sent = estimate_tokens(&transport.posts[0]["messages"].to_string());
        assert!(sent < UNKNOWN_WINDOW_TOKENS, "sent {sent} tokens");
        assert_eq!(transport.posts[0]["max_tokens"], 2_048);
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn the_owner_thinking_budget_is_headroom_on_their_endpoint_only() {
        let runtime = EndpointRuntime::default();
        let config = json!({"providers": {"byo_thinking_budget": 8_192}})
            .as_object()
            .unwrap()
            .clone();
        let mut owner = StubTransport {
            get_script: vec![no_window_models()],
            post_script: vec![Ok(response())],
            ..Default::default()
        };
        let journal = journal_path();
        let _ = endpoint_generate_with(
            &request(None),
            &journal,
            &endpoint("http://endpoint"),
            &config,
            &runtime,
            &mut owner,
            Instant::now(),
        );
        assert_eq!(owner.posts[0]["max_tokens"], 64 + 8_192);
        // No thinking field ever goes on this wire.
        for field in [
            "reasoning",
            "reasoning_effort",
            "thinking",
            "chat_template_kwargs",
        ] {
            assert!(owner.posts[0].get(field).is_none(), "{field}");
        }
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn too_small_completion_room_refits_input_for_generate() {
        let _guard = install_warn_capture();
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let mut oversized = request(None);
        oversized.max_output_tokens = 512;
        oversized.contents = vec![ContentPart::Text {
            text: "lorem ipsum dolor sit amet ".repeat(4_000),
        }];
        let overflow = "maximum context length of 1000 tokens: 800 tokens from the input messages and 400 tokens for the completion";
        let mut transport = StubTransport {
            post_script: vec![Ok(bad_request(overflow)), Ok(response())],
            ..Default::default()
        };
        assert!(matches!(
            endpoint_generate_with(
                &oversized,
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
        let first = transport.posts[0]["messages"].to_string().len();
        let second = transport.posts[1]["messages"].to_string().len();
        assert!(second < first);
        for warn in captured_warns() {
            if let Ok(v) = serde_json::from_str::<Value>(&warn) {
                assert!(v.get("diagnostic_class").is_none());
            }
        }
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn contract_400s_are_not_retried() {
        let _guard = install_warn_capture();
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let mut transport = StubTransport {
            post_script: vec![Ok(bad_request("unexpected endpoint response"))],
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
            EndpointResult::Failed(EndpointFailure {
                reason_code: Some("provider_request_rejected".to_owned()),
                detail: Some(REQUEST_REJECTED_DETAIL.to_owned()),
            })
        );
        assert_eq!(transport.posts.len(), 1);
        let warns = captured_warns();
        assert_eq!(warns.len(), 1);
        let warn_val: Value = serde_json::from_str(&warns[0]).unwrap();
        assert_eq!(warn_val["status"], 400);
        assert_eq!(warn_val["provider"], "local");
        assert_eq!(warn_val["diagnostic_class"], DIAGNOSTIC_CLASS_GENERIC);
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn unrecognized_non_context_400_is_provider_request_rejected() {
        let _guard = install_warn_capture();
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let mut transport = StubTransport {
            post_script: vec![Ok(bad_request("unexpected endpoint response"))],
            ..Default::default()
        };
        let result = endpoint_generate_with(
            &request(None),
            &journal,
            &endpoint("http://endpoint"),
            &served_window_config(),
            &runtime,
            &mut transport,
            Instant::now(),
        );
        let EndpointResult::Failed(failure) = result else {
            panic!("expected failed endpoint result, got {result:?}");
        };
        assert_eq!(
            failure.reason_code,
            Some("provider_request_rejected".to_owned())
        );
        assert_eq!(failure.detail, Some(REQUEST_REJECTED_DETAIL.to_owned()));
        assert_eq!(transport.posts.len(), 1);

        let refusal =
            crate::refusal_for(&crate::LaneOutcome::EndpointFailure(failure), "local", None);
        assert_eq!(
            refusal.reason_code.as_ref().map(ReasonCodeValue::as_wire),
            Some("provider_request_rejected")
        );
        assert!(refusal.retryable);
        assert!(!refusal.blocking);
        assert_eq!(refusal.detail, REQUEST_REJECTED_DETAIL);

        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn credentials_are_threaded_and_provider_text_never_reaches_refusal_detail() {
        let _guard = install_warn_capture();
        let credential = "endpoint-secret";
        for configured in [Some(credential), None] {
            let runtime = EndpointRuntime::default();
            let journal = journal_path();
            let mut endpoint = endpoint("http://endpoint");
            endpoint.credential = configured.map(str::to_owned);
            let request = request(None);
            let mut transport = StubTransport {
                get_script: vec![Ok(HttpResponse {
                    status: 200,
                    body: json!({"data": []}).to_string(),
                })],
                post_script: vec![
                    Ok(response()),
                    Ok(bad_request(&format!("invalid credential {credential}"))),
                ],
                ..Default::default()
            };
            let EndpointResult::Generated(generated) = endpoint_generate_with(
                &request,
                &journal,
                &endpoint,
                &Map::new(),
                &runtime,
                &mut transport,
                Instant::now(),
            ) else {
                panic!("credentialed generate must succeed before the refusal probe");
            };
            let EndpointResult::Failed(failed) = endpoint_generate_with(
                &request,
                &journal,
                &endpoint,
                &Map::new(),
                &runtime,
                &mut transport,
                Instant::now(),
            ) else {
                panic!("plain 400 must refuse");
            };
            let warns = captured_warns();
            for warn in &warns {
                assert!(!warn.contains(credential));
            }
            let refusal = crate::refusal_for(
                &crate::LaneOutcome::EndpointFailure(failed.clone()),
                "local",
                None,
            );
            assert_eq!(refusal.detail, REQUEST_REJECTED_DETAIL);
            assert!(!refusal.detail.contains(credential));
            assert_eq!(
                transport.get_credentials,
                vec![configured.map(str::to_owned)]
            );
            assert_eq!(
                transport.post_credentials,
                vec![configured.map(str::to_owned), configured.map(str::to_owned)]
            );
            for body in &transport.posts {
                let serialized = body.to_string();
                assert!(!serialized.contains(credential));
            }
            assert!(!format!("{failed:?}").contains(credential));
            assert!(!format!("{refusal:?}").contains(credential));
            let usage = generated
                .usage
                .as_ref()
                .and_then(|usage| serde_json::to_value(usage).ok())
                .unwrap_or_else(|| json!({}));
            crate::record_generate_usage(
                &journal,
                &generated.model,
                &request.context,
                &crate::usage_for_log(&usage),
                None,
            )
            .expect("token log write");
            let tokens = journal.join("tokens");
            let files: Vec<_> = std::fs::read_dir(&tokens)
                .expect("tokens directory")
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
                .collect();
            assert_eq!(files.len(), 1);
            let log = std::fs::read_to_string(&files[0]).expect("read token log");
            assert!(!log.contains(credential));
            let _ = std::fs::remove_dir_all(journal);
        }
    }

    #[test]
    fn refusal_for_preserves_request_rejected_detail() {
        for (detail, expected_detail) in [
            (REQUEST_REJECTED_DETAIL, REQUEST_REJECTED_DETAIL),
            (
                REQUEST_REJECTED_SCHEMA_DETAIL,
                REQUEST_REJECTED_SCHEMA_DETAIL,
            ),
        ] {
            let failure = EndpointFailure {
                reason_code: Some("provider_request_rejected".to_owned()),
                detail: Some(detail.to_owned()),
            };
            let refusal =
                crate::refusal_for(&crate::LaneOutcome::EndpointFailure(failure), "local", None);
            assert_eq!(
                refusal.reason_code.as_ref().map(ReasonCodeValue::as_wire),
                Some("provider_request_rejected")
            );
            assert_eq!(refusal.detail, expected_detail);
        }
    }

    #[test]
    fn endpoint_generate_schema_400_sets_schema_diagnostic_and_detail() {
        let _guard = install_warn_capture();
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let schema_body = json!({
            "type": "BadRequestError",
            "code": 400,
            "message": "Failed to compile json grammar: syntax error in schema",
        })
        .to_string();
        let mut transport = StubTransport {
            post_script: vec![Ok(bad_request(&schema_body))],
            ..Default::default()
        };
        let result = endpoint_generate_with(
            &request(None),
            &journal,
            &endpoint("http://endpoint"),
            &served_window_config(),
            &runtime,
            &mut transport,
            Instant::now(),
        );
        let EndpointResult::Failed(failure) = result else {
            panic!("expected endpoint failure, got {result:?}");
        };
        assert_eq!(
            failure.reason_code,
            Some("provider_request_rejected".to_owned())
        );
        assert_eq!(
            failure.detail,
            Some(REQUEST_REJECTED_SCHEMA_DETAIL.to_owned())
        );
        assert_eq!(transport.posts.len(), 1);
        let warns = captured_warns();
        assert_eq!(warns.len(), 1);
        let warn_val: Value = serde_json::from_str(&warns[0]).unwrap();
        let obj = warn_val.as_object().unwrap();
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, vec!["diagnostic_class", "provider", "status"]);
        assert_eq!(warn_val["status"], 400);
        assert_eq!(warn_val["provider"], "local");
        assert_eq!(warn_val["diagnostic_class"], DIAGNOSTIC_CLASS_SCHEMA);
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn endpoint_generate_schema_matrix_tests() {
        const SENTINELS: &[&str] = &[
            "SENTINEL_CRED_9f3a",
            "SENTINEL_PROVIDER_e1d4",
            "SENTINEL_OWNER_b7c2",
        ];
        let cases = [
            (
                json!({
                    "type": "BadRequestError",
                    "code": 400,
                    "message": "Failed to compile json grammar: syntax error in schema",
                })
                .to_string(),
                DIAGNOSTIC_CLASS_SCHEMA,
                REQUEST_REJECTED_SCHEMA_DETAIL,
            ),
            (
                json!({
                    "type": "OtherError",
                    "code": 400,
                    "message": "Failed to compile json grammar: syntax error",
                })
                .to_string(),
                DIAGNOSTIC_CLASS_GENERIC,
                REQUEST_REJECTED_DETAIL,
            ),
            (
                json!({
                    "type": "BadRequestError",
                    "code": 500,
                    "message": "Failed to compile json grammar: syntax error",
                })
                .to_string(),
                DIAGNOSTIC_CLASS_GENERIC,
                REQUEST_REJECTED_DETAIL,
            ),
            (
                json!({
                    "type": "BadRequestError",
                    "code": 400,
                    "message": "Some other error",
                })
                .to_string(),
                DIAGNOSTIC_CLASS_GENERIC,
                REQUEST_REJECTED_DETAIL,
            ),
            (
                json!({
                    "type": "BadRequestError",
                    "code": "400",
                    "message": "Failed to compile json grammar: syntax error",
                })
                .to_string(),
                DIAGNOSTIC_CLASS_GENERIC,
                REQUEST_REJECTED_DETAIL,
            ),
            (
                json!({
                    "type": "BadRequestError",
                    "code": 400,
                    "message": "prefix Failed to compile json grammar: nope",
                })
                .to_string(),
                DIAGNOSTIC_CLASS_GENERIC,
                REQUEST_REJECTED_DETAIL,
            ),
            (
                format!(
                    "The schema and grammar failed to compile due to {} and {} and {}",
                    SENTINELS[0], SENTINELS[1], SENTINELS[2]
                ),
                DIAGNOSTIC_CLASS_GENERIC,
                REQUEST_REJECTED_DETAIL,
            ),
            (
                "plain text error".to_owned(),
                DIAGNOSTIC_CLASS_GENERIC,
                REQUEST_REJECTED_DETAIL,
            ),
        ];

        for (body, expected_diag, expected_detail) in cases {
            let _guard = install_warn_capture();
            let runtime = EndpointRuntime::default();
            let journal = journal_path();
            let mut transport = StubTransport {
                post_script: vec![Ok(bad_request(&body))],
                ..Default::default()
            };
            let result = endpoint_generate_with(
                &request(None),
                &journal,
                &endpoint("http://endpoint"),
                &served_window_config(),
                &runtime,
                &mut transport,
                Instant::now(),
            );
            let EndpointResult::Failed(failure) = result else {
                panic!("expected endpoint failure for body: {body}");
            };
            assert_eq!(
                failure.reason_code,
                Some("provider_request_rejected".to_owned())
            );
            assert_eq!(failure.detail, Some(expected_detail.to_owned()));
            let warns = captured_warns();
            assert_eq!(warns.len(), 1, "expected 1 warn for body: {body}");
            let warn_val: Value = serde_json::from_str(&warns[0]).unwrap();
            let obj = warn_val.as_object().unwrap();
            let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
            keys.sort();
            assert_eq!(keys, vec!["diagnostic_class", "provider", "status"]);
            assert_eq!(warn_val["status"], 400);
            assert_eq!(warn_val["provider"], "local");
            assert_eq!(warn_val["diagnostic_class"], expected_diag);

            if body.contains("SENTINEL") {
                for sentinel in SENTINELS {
                    assert!(!failure.detail.as_deref().unwrap_or("").contains(sentinel));
                    assert!(!warns[0].contains(sentinel));
                }
            }
            let _ = std::fs::remove_dir_all(journal);
        }
    }

    #[test]
    fn refused_connection_is_endpoint_unreachable() {
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        assert_eq!(
            endpoint_generate_with(
                &request(None),
                &journal,
                &endpoint("http://endpoint"),
                &served_window_config(),
                &runtime,
                &mut StubTransport {
                    post_script: vec![Err(EndpointTransportError::Connection)],
                    ..Default::default()
                },
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
            post_script: vec![Err(EndpointTransportError::Other)],
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
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        assert_eq!(
            endpoint_generate_with(
                &request(None),
                &journal,
                &endpoint("http://endpoint"),
                &served_window_config(),
                &runtime,
                &mut StubTransport {
                    post_script: vec![Err(EndpointTransportError::Capacity)],
                    ..Default::default()
                },
                Instant::now(),
            ),
            failure("local_capacity_exhausted")
        );
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn confidential_endpoint_does_not_create_a_local_admission_directory() {
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let endpoint = ByoEndpoint {
            parallel_slots: None,
            is_confidential: true,
            ..endpoint("http://endpoint")
        };
        let mut transport = StubTransport {
            post_script: vec![Ok(response())],
            ..Default::default()
        };

        assert!(matches!(
            endpoint_generate_with(
                &request(None),
                &journal,
                &endpoint,
                &served_window_config(),
                &runtime,
                &mut transport,
                Instant::now(),
            ),
            EndpointResult::Generated(_)
        ));
        assert!(!admission_dir(&journal).exists());
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
        let config = served_window_config();
        let timeout_s = Some(10.0);
        let gate = Arc::new(AdmissionGate {
            inner: Mutex::new(GateState::default()),
            entered: Condvar::new(),
            released: Condvar::new(),
        });
        let _release = ReleaseOnDrop {
            gate: Arc::clone(&gate),
        };
        let template = StubTransport {
            post_script: vec![Ok(response())],
            gate: Some(Arc::clone(&gate)),
            ..Default::default()
        };
        let mut workers = Vec::new();
        for label in ["w1", "w2"] {
            let journal = journal.clone();
            let endpoint = endpoint.clone();
            let mut request = request(timeout_s);
            request.contents = vec![ContentPart::Text { text: label.into() }];
            let config = config.clone();
            let mut transport = template.clone();
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
        {
            let mut state = gate.inner.lock().expect("admission gate lock");
            while state.current < 2 {
                state = gate.entered.wait(state).expect("admission gate wait");
            }
        }
        let before = wait_ticket_names(&journal);
        let journal_w3 = journal.clone();
        let endpoint_w3 = endpoint.clone();
        let mut request_w3 = request(timeout_s);
        request_w3.contents = vec![ContentPart::Text { text: "w3".into() }];
        let config_w3 = config.clone();
        let mut transport_w3 = template.clone();
        workers.push(thread::spawn(move || {
            endpoint_generate_with(
                &request_w3,
                &journal_w3,
                &endpoint_w3,
                &config_w3,
                &EndpointRuntime::default(),
                &mut transport_w3,
                Instant::now(),
            )
        }));
        while wait_ticket_names(&journal)
            .difference(&before)
            .next()
            .is_none()
        {
            thread::yield_now();
        }
        {
            let mut state = gate.inner.lock().expect("admission gate lock");
            state.release = true;
            gate.released.notify_all();
        }
        for worker in workers {
            assert!(matches!(
                worker.join().expect("join endpoint worker"),
                EndpointResult::Generated(_)
            ));
        }
        let state = gate.inner.lock().expect("admission gate lock");
        assert_eq!(state.peak, 2, "peak admission depth: {}", state.peak);
        let after_release = |label: &str| {
            state
                .records
                .iter()
                .find(|(body, _, _)| body.to_string().contains(label))
                .map(|(_, _, after)| *after)
                .expect("labeled post")
        };
        assert!(!after_release("w1"));
        assert!(!after_release("w2"));
        assert!(after_release("w3"));
        let queued = state
            .records
            .iter()
            .find(|(body, _, _)| body.to_string().contains("w3"))
            .map(|(_, timeout, _)| *timeout)
            .expect("labeled post");
        assert!(
            queued < request_timeout(timeout_s, 0),
            "queued post timeout {queued:?} was not reduced from {:?}",
            request_timeout(timeout_s, 0)
        );
        let _ = std::fs::remove_dir_all(cleanup_journal);
    }

    #[test]
    fn transport_timeout_releases_admission_for_the_next_request() {
        let journal = journal_path();
        let config = served_window_config();
        let permit = acquire_local_slot(
            &admission_dir(&journal),
            1,
            Some(Duration::from_secs(2)),
            false,
        )
        .expect("held permit");
        assert_eq!(
            endpoint_generate_with(
                &request(Some(0.05)),
                &journal,
                &endpoint("http://endpoint"),
                &config,
                &EndpointRuntime::default(),
                &mut StubTransport {
                    post_script: vec![Ok(response())],
                    ..Default::default()
                },
                Instant::now(),
            ),
            failure("local_queue_timeout")
        );
        drop(permit);
        assert_eq!(
            endpoint_generate_with(
                &request(Some(10.0)),
                &journal,
                &endpoint("http://endpoint"),
                &config,
                &EndpointRuntime::default(),
                &mut StubTransport {
                    post_script: vec![Err(EndpointTransportError::Capacity)],
                    ..Default::default()
                },
                Instant::now(),
            ),
            failure("local_capacity_exhausted")
        );
        assert!(matches!(
            endpoint_generate_with(
                &request(Some(10.0)),
                &journal,
                &endpoint("http://endpoint"),
                &config,
                &EndpointRuntime::default(),
                &mut StubTransport {
                    post_script: vec![Ok(response())],
                    ..Default::default()
                },
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
