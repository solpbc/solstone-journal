// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bring-your-own local endpoint generation.

use std::collections::{BTreeSet, HashMap};
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
    ByoEndpoint, HttpResponse, InputBudget, LocalConverseError, LocalConverseRequest,
    RequestBudget, Usage, build_converse_request_body, build_messages, build_request_body,
    count_image_parts, estimate_tokens, fit_contents, parse_converse_response, parse_response,
    serialized_message_text, served_window_from_models_response,
};
use solstone_core_spp_ratls::AttestationStateStore;

use crate::{ConverseFailure, ConverseMessage, ConverseToolCall, ConverseToolSpec, ConverseTurn};

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

pub type EndpointConverseResult = Result<Box<ConverseTurn>, ConverseFailure>;

pub(crate) struct EndpointConverseCall<'a> {
    pub request: &'a GenerateRequest,
    pub messages: &'a [ConverseMessage],
    pub tools: &'a [ConverseToolSpec],
    pub journal_path: &'a Path,
    pub endpoint: &'a ByoEndpoint,
    pub config: &'a Map<String, Value>,
    pub runtime: &'a EndpointRuntime,
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

pub fn endpoint_converse(
    request: &GenerateRequest,
    messages: &[ConverseMessage],
    tools: &[ConverseToolSpec],
    journal_path: &Path,
    endpoint: &ByoEndpoint,
    config: &Map<String, Value>,
    runtime: &EndpointRuntime,
) -> EndpointConverseResult {
    let mut transport = UreqEndpointTransport;
    endpoint_converse_with(
        EndpointConverseCall {
            request,
            messages,
            tools,
            journal_path,
            endpoint,
            config,
            runtime,
        },
        &mut transport,
        Instant::now(),
    )
}

pub(crate) fn endpoint_converse_with<T: EndpointTransport>(
    call: EndpointConverseCall<'_>,
    transport: &mut T,
    now: Instant,
) -> EndpointConverseResult {
    let EndpointConverseCall {
        request,
        messages,
        tools,
        journal_path,
        endpoint,
        config,
        runtime,
    } = call;
    let max_tokens = match u32::try_from(request.max_output_tokens) {
        Ok(value) => value,
        Err(_) => return converse_failure("provider_response_invalid"),
    };
    let served_window = runtime.resolve_served_window(endpoint, config, transport, now);
    // 🔴 A confidential endpoint used to REFUSE here when no served window was
    // resolved:
    // `None if endpoint.is_confidential => converse_failure("context_budget_exceeded")`.
    //
    // `AttestedEndpointTransport::get` returns `Err` unconditionally and says so in
    // its own comment -- "model discovery is optional ... it must not issue an
    // unaudited second request over the one-shot channel". So for a confidential
    // endpoint `resolve_served_window` can only ever return `None` unless
    // `providers.local.served_context_window` is configured. Discovery was declared
    // optional and its absence was then fatal, in exactly the case where it is
    // guaranteed absent -- thinking was down on the SPP lane for every owner.
    //
    // ⚠ `None` is not a licence to overflow: it means no CLIENT-side fitting, which
    // is what the non-confidential BYO path already does, and the server still
    // enforces its own window. ✅ Client-side fitting remains available by setting
    // `providers.local.served_context_window`, which `resolve_served_window` reads first.
    let input_budget_tokens = served_window
        .map(|window| solstone_core_local::generate::compute_input_budget(max_tokens, window));
    let message_values = converse_messages_to_value(messages);
    let tool_values = converse_tools_to_value(tools);
    let local_request = LocalConverseRequest {
        model: &endpoint.served_model_id,
        system_instruction: request.system_instruction.as_deref(),
        messages: &message_values,
        tools: &tool_values,
        temperature: request.temperature,
        max_tokens,
        json_output: request.json_output,
        json_schema: request.json_schema.as_ref(),
        // Bundled and confidential lanes use llama-server-specific controls;
        // arbitrary BYO endpoints may not support them.
        include_qwen_sampling_controls: endpoint.is_bundled || endpoint.is_confidential,
    };
    let mut body = match build_converse_request_body(&local_request, input_budget_tokens) {
        Ok(body) => body,
        Err(LocalConverseError::ContextBudgetExceeded) => {
            return converse_failure("context_budget_exceeded");
        }
        Err(_) => return converse_failure("provider_response_invalid"),
    };
    let timeout = request_timeout(request.timeout_s);
    let started = Instant::now();
    let Some(admission_timeout) = remaining_timeout(started, timeout) else {
        return converse_failure("local_capacity_exhausted");
    };
    if admission_timeout.is_zero() {
        return converse_failure("local_capacity_exhausted");
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
                Err(reason_code) => return converse_failure(reason_code),
            }
        };
        let mut attempt = 0;
        loop {
            let Some(remaining) = remaining_timeout(started, timeout) else {
                return converse_failure("local_capacity_exhausted");
            };
            if remaining.is_zero() {
                return converse_failure("local_capacity_exhausted");
            }
            let response = match endpoint_post(endpoint, &body, remaining, transport) {
                Ok(response) => response,
                Err(reason_code) => return converse_failure(reason_code),
            };
            if response.status != 400 {
                break response;
            }
            match endpoint_overflow_decision(&response.body, served_window, attempt) {
                OverflowDecision::Retry(new_max_tokens) => {
                    body["max_tokens"] = json!(new_max_tokens);
                    attempt += 1;
                }
                OverflowDecision::Budget => return converse_failure("context_budget_exceeded"),
                OverflowDecision::Context => return converse_failure("context_window_exceeded"),
                OverflowDecision::Contract => {
                    return converse_failure("local_endpoint_contract_failed");
                }
            }
        }
    };
    if !(200..300).contains(&response.status) {
        return converse_failure("provider_response_invalid");
    }
    let response_body = match serde_json::from_str::<Value>(&response.body) {
        Ok(body) => body,
        Err(_) => return converse_failure("provider_response_invalid"),
    };
    let parsed = match parse_converse_response(&response_body) {
        Ok(parsed) => parsed,
        Err(LocalConverseError::ResponseInvalid | LocalConverseError::ContextBudgetExceeded) => {
            return converse_failure("provider_response_invalid");
        }
        Err(LocalConverseError::ToolCallsMissing) => return converse_failure("tool_calls_missing"),
        Err(LocalConverseError::ToolCallArgumentsInvalid) => {
            return converse_failure("tool_call_arguments_invalid");
        }
        Err(LocalConverseError::ToolCallSynthesizedAsProse) => {
            return converse_failure("tool_call_synthesized_as_prose");
        }
    };
    let offered = tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<BTreeSet<_>>();
    let tool_calls = parsed
        .tool_calls
        .into_iter()
        .map(|call| {
            let not_offered = !offered.contains(call.name.as_str());
            ConverseToolCall {
                id: call.id,
                name: call.name,
                arguments: call.arguments,
                not_offered,
            }
        })
        .collect();
    let usage = match parsed.usage {
        Some(usage) => match serde_json::to_value(usage) {
            Ok(usage) => usage,
            Err(_) => return converse_failure("provider_response_invalid"),
        },
        None => json!({}),
    };
    Ok(Box::new(ConverseTurn {
        text: parsed.text,
        tool_calls,
        finish_reason: parsed.finish_reason,
        usage,
        model: endpoint.served_model_id.clone(),
        thinking: None,
    }))
}

/// Deterministic transport injection for downstream library tests.
///
/// This surface is absent from normal builds and deliberately exposes only the
/// existing endpoint call, not endpoint internals.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub mod test_support {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    pub fn endpoint_converse_with_transport<T: EndpointTransport>(
        request: &GenerateRequest,
        messages: &[ConverseMessage],
        tools: &[ConverseToolSpec],
        journal_path: &Path,
        endpoint: &ByoEndpoint,
        config: &Map<String, Value>,
        runtime: &EndpointRuntime,
        transport: &mut T,
        now: Instant,
    ) -> EndpointConverseResult {
        endpoint_converse_with(
            EndpointConverseCall {
                request,
                messages,
                tools,
                journal_path,
                endpoint,
                config,
                runtime,
            },
            transport,
            now,
        )
    }
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
                // Confidential calls create a fresh attested channel, not a
                // shared local endpoint slot; this only records budget metadata.
                slots: endpoint.parallel_slots.unwrap_or(1),
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
            // Bundled generate never reaches this endpoint path. Only the
            // confidential lane's directly attested channel is distinguished
            // from a plain BYO endpoint here.
            endpoint.is_confidential,
        ),
        input_budget,
        request_budget,
    })
}

fn converse_messages_to_value(messages: &[ConverseMessage]) -> Value {
    Value::Array(
        messages
            .iter()
            .map(|message| match message {
                ConverseMessage::User { text } => json!({"role": "user", "content": text}),
                ConverseMessage::Assistant { text, tool_calls } if tool_calls.is_empty() => {
                    json!({"role": "assistant", "content": text})
                }
                ConverseMessage::Assistant { text, tool_calls } => json!({
                    "role": "assistant",
                    "content": if text.is_empty() { Value::Null } else { Value::String(text.clone()) },
                    "tool_calls": tool_calls.iter().map(|call| json!({
                        "id": call.id,
                        "type": "function",
                        "function": {"name": call.name, "arguments": call.arguments.to_string()},
                    })).collect::<Vec<_>>(),
                }),
                ConverseMessage::ToolResult {
                    tool_call_id,
                    tool_name: _,
                    output,
                } => json!({"role": "tool", "tool_call_id": tool_call_id, "content": output}),
            })
            .collect(),
    )
}

fn converse_tools_to_value(tools: &[ConverseToolSpec]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    },
                })
            })
            .collect(),
    )
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

pub(crate) fn converse_failure(reason_code: &str) -> EndpointConverseResult {
    let (retryable, blocking) = crate::converse::converse_failure_flags(reason_code);
    Err(ConverseFailure {
        reason_code: reason_code.to_owned(),
        retryable,
        blocking,
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar};
    use std::thread;

    use serde_json::json;

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
        for (is_bundled, is_confidential, expected) in [
            (false, false, false),
            (true, false, true),
            (false, true, true),
        ] {
            let mut endpoint = endpoint("http://endpoint");
            endpoint.is_bundled = is_bundled;
            endpoint.is_confidential = is_confidential;
            let journal = journal_path();
            let request = request(None);
            let messages = [];
            let tools = [];
            let runtime = EndpointRuntime::default();
            let config = served_window_config();
            let mut transport = StubTransport {
                post_script: vec![Ok(response())],
                ..Default::default()
            };
            endpoint_converse_with(
                EndpointConverseCall {
                    request: &request,
                    messages: &messages,
                    tools: &tools,
                    journal_path: &journal,
                    endpoint: &endpoint,
                    config: &config,
                    runtime: &runtime,
                },
                &mut transport,
                Instant::now(),
            )
            .expect("converse request succeeds");
            let body = transport.posts.remove(0);
            for field in QWEN_SAMPLING_FIELDS {
                assert_eq!(
                    body.get(field).is_some(),
                    expected,
                    "is_bundled={is_bundled}, is_confidential={is_confidential}: {field}"
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
            assert_eq!(request_timeout(timeout_s), DEFAULT_TIMEOUT);
        }
    }

    #[test]
    fn discovery_failure_does_not_block_generation_or_add_budgets() {
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
        assert_eq!(refusal.detail, crate::refusal::LIVE_PROVIDER_FAILURE_DETAIL);
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn overflow_reclamps_once_then_generates() {
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
        assert_eq!(transport.posts[1]["max_tokens"], 384);
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn repeated_overflow_is_context_window_exceeded_without_a_third_post() {
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let overflow = "maximum context length of 1000 tokens: 600 tokens from the input messages and 400 tokens for the completion";
        let mut transport = StubTransport {
            post_script: vec![Ok(bad_request(overflow)), Ok(bad_request(overflow))],
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
            post_script: vec![Ok(bad_request(overflow))],
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
                post_script: vec![Ok(bad_request(body))],
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
            let refusal = crate::refusal_for(
                &crate::LaneOutcome::EndpointFailure(failed.clone()),
                "local",
                None,
            );
            assert_eq!(refusal.detail, crate::refusal::LIVE_PROVIDER_FAILURE_DETAIL);
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
                assert!(!body.to_string().contains(credential));
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
            queued < request_timeout(timeout_s),
            "queued post timeout {queued:?} was not reduced from {:?}",
            request_timeout(timeout_s)
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
    fn converse_tools() -> Vec<ConverseToolSpec> {
        vec![ConverseToolSpec {
            name: "weather".into(),
            description: "weather".into(),
            parameters: json!({"type": "object"}),
        }]
    }

    fn converse_response(name: &str) -> HttpResponse {
        HttpResponse {
            status: 200,
            body: json!({
                "choices": [{
                    "message": {
                        "content": "before",
                        "tool_calls": [{
                            "id": "call-1",
                            "type": "function",
                            "function": {"name": name, "arguments": "{\"city\":\"Denver\"}"},
                        }],
                    },
                    "finish_reason": "stop",
                }],
                "usage": {"prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5},
            })
            .to_string(),
        }
    }

    #[test]
    fn converse_posts_chat_completions_history_and_marks_unoffered_calls() {
        for (name, expected_not_offered) in [("weather", false), ("other", true)] {
            let runtime = EndpointRuntime::default();
            let journal = journal_path();
            let messages = vec![
                ConverseMessage::User { text: "ask".into() },
                ConverseMessage::Assistant {
                    text: String::new(),
                    tool_calls: vec![ConverseToolCall {
                        id: "prior-call".into(),
                        name: "weather".into(),
                        arguments: json!({"city": "Denver"}),
                        not_offered: false,
                    }],
                },
                ConverseMessage::ToolResult {
                    tool_call_id: "prior-call".into(),
                    tool_name: "weather".into(),
                    output: "sunny".into(),
                },
            ];
            let mut transport = StubTransport {
                post_script: vec![Ok(converse_response(name))],
                ..Default::default()
            };
            let turn = endpoint_converse_with(
                EndpointConverseCall {
                    request: &request(None),
                    messages: &messages,
                    tools: &converse_tools(),
                    journal_path: &journal,
                    endpoint: &endpoint("http://endpoint"),
                    config: &served_window_config(),
                    runtime: &runtime,
                },
                &mut transport,
                Instant::now(),
            )
            .expect("converse turn");
            assert_eq!(turn.text, "before");
            assert_eq!(turn.finish_reason, "tool_calls");
            assert_eq!(turn.tool_calls[0].not_offered, expected_not_offered);
            assert_eq!(turn.tool_calls[0].arguments, json!({"city": "Denver"}));
            let expected = json!({
                "model": "served",
                "messages": [
                    {"role": "user", "content": "ask"},
                    {"role": "assistant", "content": null, "tool_calls": [{
                        "id": "prior-call",
                        "type": "function",
                        "function": {"name": "weather", "arguments": "{\"city\":\"Denver\"}"},
                    }]},
                    {"role": "tool", "tool_call_id": "prior-call", "content": "sunny"},
                ],
                "tools": [{"type": "function", "function": {"name": "weather", "description": "weather", "parameters": {"type": "object"}}}],
                "temperature": 0.2,
                "max_tokens": 64,
                "stream": false,
            });
            assert_eq!(transport.posts, vec![expected.clone()]);
            let mut wrong = expected;
            wrong.as_object_mut().expect("body object").remove("tools");
            assert_ne!(transport.posts[0], wrong);
            let _ = std::fs::remove_dir_all(journal);
        }
    }

    #[test]
    fn converse_fits_growing_history_before_its_single_post() {
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let tools = converse_tools();
        let mut messages = Vec::new();
        for number in 0..10 {
            let id = format!("call-{number}");
            messages.push(ConverseMessage::Assistant {
                text: String::new(),
                tool_calls: vec![ConverseToolCall {
                    id: id.clone(),
                    name: "weather".into(),
                    arguments: json!({"note": "x".repeat(500)}),
                    not_offered: false,
                }],
            });
            messages.push(ConverseMessage::ToolResult {
                tool_call_id: id,
                tool_name: "weather".into(),
                output: "y".repeat(500),
            });
        }
        messages.push(ConverseMessage::User {
            text: "latest".into(),
        });
        let mut transport = StubTransport {
            post_script: vec![Ok(response())],
            ..Default::default()
        };
        let config = json!({"providers": {"local": {"served_context_window": 2048}}})
            .as_object()
            .expect("config object")
            .clone();
        endpoint_converse_with(
            EndpointConverseCall {
                request: &request(None),
                messages: &messages,
                tools: &tools,
                journal_path: &journal,
                endpoint: &endpoint("http://endpoint"),
                config: &config,
                runtime: &runtime,
            },
            &mut transport,
            Instant::now(),
        )
        .expect("converse turn");
        let posted = serde_json::to_string(&transport.posts[0]).expect("posted JSON");
        assert!(
            estimate_tokens(&posted)
                <= solstone_core_local::generate::compute_input_budget(64, 2048)
        );
        assert_eq!(transport.posts.len(), 1);
        assert!(
            messages.len()
                > transport.posts[0]["messages"]
                    .as_array()
                    .expect("messages")
                    .len()
        );
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn converse_fitting_evicts_tool_call_pairs_without_orphans() {
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let tools = converse_tools();
        let mut messages = Vec::new();
        for number in 0..5 {
            let id = format!("call-{number}");
            messages.push(ConverseMessage::Assistant {
                text: String::new(),
                tool_calls: vec![ConverseToolCall {
                    id: id.clone(),
                    name: "weather".into(),
                    arguments: json!({"note": "x".repeat(650)}),
                    not_offered: false,
                }],
            });
            messages.push(ConverseMessage::ToolResult {
                tool_call_id: id,
                tool_name: "weather".into(),
                output: "y".repeat(650),
            });
        }
        messages.push(ConverseMessage::User {
            text: "latest".into(),
        });
        let config = json!({"providers": {"local": {"served_context_window": 2048}}})
            .as_object()
            .expect("config object")
            .clone();
        let mut transport = StubTransport {
            post_script: vec![Ok(response())],
            ..Default::default()
        };
        endpoint_converse_with(
            EndpointConverseCall {
                request: &request(None),
                messages: &messages,
                tools: &tools,
                journal_path: &journal,
                endpoint: &endpoint("http://endpoint"),
                config: &config,
                runtime: &runtime,
            },
            &mut transport,
            Instant::now(),
        )
        .expect("converse turn");

        let mut assistant_call_ids = BTreeSet::new();
        let mut tool_result_ids = BTreeSet::new();
        for message in transport.posts[0]["messages"]
            .as_array()
            .expect("posted messages")
        {
            if message["role"] == "assistant" {
                for call in message["tool_calls"].as_array().into_iter().flatten() {
                    assistant_call_ids.insert(
                        call["id"]
                            .as_str()
                            .expect("assistant tool call ID")
                            .to_owned(),
                    );
                }
            }
            if message["role"] == "tool" {
                tool_result_ids.insert(
                    message["tool_call_id"]
                        .as_str()
                        .expect("tool result call ID")
                        .to_owned(),
                );
            }
        }
        assert!(!assistant_call_ids.is_empty());
        assert!(assistant_call_ids.len() < 5);
        assert_eq!(assistant_call_ids, tool_result_ids);
        assert_eq!(transport.posts.len(), 1);

        let unfittable = vec![
            ConverseMessage::Assistant {
                text: String::new(),
                tool_calls: vec![ConverseToolCall {
                    id: "huge".into(),
                    name: "weather".into(),
                    arguments: json!({"note": "x".repeat(20_000)}),
                    not_offered: false,
                }],
            },
            ConverseMessage::ToolResult {
                tool_call_id: "huge".into(),
                tool_name: "weather".into(),
                output: "y".repeat(20_000),
            },
        ];
        let mut blocked = StubTransport::default();
        let error = endpoint_converse_with(
            EndpointConverseCall {
                request: &request(None),
                messages: &unfittable,
                tools: &tools,
                journal_path: &journal,
                endpoint: &endpoint("http://endpoint"),
                config: &config,
                runtime: &runtime,
            },
            &mut blocked,
            Instant::now(),
        )
        .expect_err("unfittable pair must fail before transport");
        assert_eq!(error.reason_code, "context_budget_exceeded");
        assert!(blocked.posts.is_empty());
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn converse_and_one_shot_builders_share_wire_controls() {
        let schema = json!({"type": "object", "properties": {"answer": {"type": "string"}}});
        let one_shot = build_request_body(
            "served",
            vec![json!({"role": "user", "content": "ask"})],
            0.2,
            64,
            true,
            Some(&schema),
            true,
        );
        let messages = json!([{"role": "user", "content": "ask"}]);
        let tools = json!([]);
        let converse = build_converse_request_body(
            &LocalConverseRequest {
                model: "served",
                system_instruction: None,
                messages: &messages,
                tools: &tools,
                temperature: 0.2,
                max_tokens: 64,
                json_output: true,
                json_schema: Some(&schema),
                include_qwen_sampling_controls: true,
            },
            None,
        )
        .expect("converse request body");
        for field in [
            "temperature",
            "max_tokens",
            "stream",
            "response_format",
            "chat_template_kwargs",
            "top_p",
            "top_k",
            "min_p",
            "presence_penalty",
        ] {
            assert_eq!(one_shot[field], converse[field], "{field}");
        }
    }

    #[test]
    fn endpoint_converse_tool_calls_are_accepted_with_blank_text() {
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let messages = vec![ConverseMessage::User { text: "ask".into() }];
        let tools = converse_tools();
        let config = served_window_config();
        let mut transport = StubTransport {
            post_script: vec![Ok(converse_response("weather"))],
            ..Default::default()
        };
        let turn = endpoint_converse_with(
            EndpointConverseCall {
                request: &request(None),
                messages: &messages,
                tools: &tools,
                journal_path: &journal,
                endpoint: &endpoint("http://endpoint"),
                config: &config,
                runtime: &runtime,
            },
            &mut transport,
            Instant::now(),
        )
        .expect("converse turn");
        let assessment = crate::assess_provider_result(crate::ProviderResultView {
            journal_path: &journal,
            context: "test.converse",
            model: &turn.model,
            text: "",
            finish_reason: &turn.finish_reason,
            usage: &turn.usage,
            json_output: false,
            enforce_responsiveness: false,
        });
        assert_eq!(assessment.failure, None);
        let empty = json!({});
        let rejected = crate::assess_provider_result(crate::ProviderResultView {
            journal_path: &journal,
            context: "test.converse",
            model: &turn.model,
            text: "",
            finish_reason: "stop",
            usage: &empty,
            json_output: false,
            enforce_responsiveness: false,
        });
        assert_eq!(
            rejected.failure,
            Some(crate::ValidationFailure::ProviderResponseInvalid)
        );
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn converse_without_a_served_window_uses_the_plain_endpoint_fallback() {
        let runtime = EndpointRuntime::default();
        let journal = journal_path();
        let messages = vec![ConverseMessage::User { text: "ask".into() }];
        let mut transport = StubTransport {
            get_script: vec![Err(EndpointTransportError::Other)],
            post_script: vec![Ok(response())],
            ..Default::default()
        };
        assert!(
            endpoint_converse_with(
                EndpointConverseCall {
                    request: &request(None),
                    messages: &messages,
                    tools: &converse_tools(),
                    journal_path: &journal,
                    endpoint: &endpoint("http://endpoint"),
                    config: &Map::new(),
                    runtime: &runtime,
                },
                &mut transport,
                Instant::now(),
            )
            .is_ok()
        );
        assert_eq!(transport.get_calls, 1);
        assert_eq!(transport.posts.len(), 1);
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn converse_reason_codes_use_shared_failure_flags() {
        for reason_code in [
            "context_budget_exceeded",
            "provider_response_invalid",
            "tool_calls_missing",
            "tool_call_arguments_invalid",
            "tool_call_synthesized_as_prose",
            "local_endpoint_unreachable",
            "local_capacity_exhausted",
            "local_queue_timeout",
            "context_window_exceeded",
            "local_endpoint_contract_failed",
            "attestation_stale",
            "attestation_not_yet_verified",
            "attestation_failed",
        ] {
            let Err(failure) = converse_failure(reason_code) else {
                panic!("converse failure expected");
            };
            assert_eq!(failure.reason_code, reason_code);
        }
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
