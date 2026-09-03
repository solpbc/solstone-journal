// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bundled local generate request, response, and context-budget logic.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::LoopbackAddr;
use crate::admission::{AdmissionError, acquire_local_slot, admission_dir};
use crate::connect::{ConnectInput, ConnectOutcome, ConnectedServer, connect};
use crate::fixture::local_generate;
use crate::plan::Platform;
use crate::tier::{
    CAPABLE_CONTEXT_TOKENS, CAPABLE_PARALLEL_SLOTS, FLOOR_CONTEXT_TOKENS, FLOOR_PARALLEL_SLOTS,
};

const SAFETY_MARGIN_TOKENS: u32 = 256;
const ESTIMATED_IMAGE_TOKENS: u32 = 2_500;
const OUTPUT_RESERVE_DIVISOR: u32 = 4;
const MIN_COMPLETION_TOKENS: u32 = 256;
const TOKENIZE_TIMEOUT: Duration = Duration::from_secs(5);
const TRUNCATION_MARKER: &str = "[earlier input truncated to fit the on-device model's context]";
const LOCAL_SCHEMA_MAX_ITEMS: u64 = 192;
const CAPACITY_EXHAUSTED_MESSAGE: &str =
    "The local model was busy and could not finish this request. Try again in a moment.";
const CONTEXT_PATTERNS: &[&str] = &[
    "exceeds the available context size",
    "context size has been exceeded",
    "exceeds the context window",
    "maximum context length",
    "longer than the model's context length",
    "context length exceeded",
];

type RebuildContents = Box<dyn Fn(String) -> Value>;

#[derive(Debug, Clone, PartialEq)]
pub struct GenerateError {
    pub reason_code: String,
    pub detail: String,
}

impl GenerateError {
    fn into_failure(self) -> GenerateFailure {
        failure_record(&self.reason_code, self.detail, None)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerateInput {
    pub schema: String,
    pub journal_path: String,
    pub bind_address: LoopbackAddr,
    pub default_model_id: String,
    pub platform: Platform,
    pub contents: Value,
    pub system_instruction: Option<String>,
    pub temperature: f64,
    pub max_output_tokens: u32,
    pub json_output: bool,
    pub json_schema: Option<Value>,
    pub timeout_s: Option<f64>,
    pub exclusive_admission: bool,
    pub attempt_index: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InputBudget {
    pub clipped: bool,
    pub dropped_chars: usize,
    pub dropped_entries: usize,
    pub budget_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RequestBudget {
    pub window: u32,
    pub slots: u32,
    pub estimated_prompt_tokens: u32,
    pub image_tokens: u32,
    pub clamped_max_tokens: u32,
    pub requested_max_output_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ServerInference {
    pub prompt_eval_ms: Option<f64>,
    pub generation_ms: Option<f64>,
    pub server_total_ms: Option<f64>,
    pub prompt_tokens: Option<f64>,
    pub generated_tokens: Option<f64>,
    pub prompt_cached_tokens: Option<f64>,
    pub selected_slot: Option<f64>,
    pub prompt_cache_state: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Inference {
    pub profile: String,
    pub serving_capacity: u32,
    pub capacity_source: String,
    pub admission_slot: Option<u32>,
    pub queue_wait_ms: f64,
    pub client_total_ms: f64,
    pub retry_index: Option<u32>,
    pub outcome: String,
    pub finish_reason: Option<String>,
    pub reason_code: Option<String>,
    pub timed_out: bool,
    pub cancelled: bool,
    pub server: Option<ServerInference>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GenerateSuccess {
    pub schema: String,
    pub outcome: String,
    pub text: String,
    pub model: String,
    pub usage: Option<Usage>,
    pub finish_reason: String,
    pub input_budget: Option<InputBudget>,
    pub request_budget: RequestBudget,
    pub inference: Inference,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GenerateFailure {
    pub schema: String,
    pub outcome: String,
    pub reason_code: Option<String>,
    pub detail: String,
    pub inference: Option<Inference>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum GenerateResult {
    Success(GenerateSuccess),
    Failure(GenerateFailure),
}

#[derive(Debug, Clone, PartialEq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// Transport seam for deterministic unit tests and the future one-record CLI.
pub trait GenerateTransport {
    fn get(
        &mut self,
        base_url: &str,
        path: &str,
        timeout: Duration,
    ) -> Result<HttpResponse, String>;
    fn post_json(
        &mut self,
        base_url: &str,
        path: &str,
        body: &Value,
        timeout: Duration,
    ) -> Result<HttpResponse, String>;
}

#[derive(Default)]
pub struct UreqTransport;

impl GenerateTransport for UreqTransport {
    fn get(
        &mut self,
        base_url: &str,
        path: &str,
        timeout: Duration,
    ) -> Result<HttpResponse, String> {
        request("get", base_url, path, None, timeout)
    }

    fn post_json(
        &mut self,
        base_url: &str,
        path: &str,
        body: &Value,
        timeout: Duration,
    ) -> Result<HttpResponse, String> {
        request("post", base_url, path, Some(body), timeout)
    }
}

/// Execute the bundled local request using the production readiness probe.
pub fn generate(input: GenerateInput) -> GenerateResult {
    let mut transport = UreqTransport;
    generate_with(input, &mut transport, connect)
}

/// Execute with injected readiness and HTTP seams.
pub fn generate_with<T, F>(input: GenerateInput, transport: &mut T, connector: F) -> GenerateResult
where
    T: GenerateTransport,
    F: FnOnce(ConnectInput) -> ConnectOutcome,
{
    let contract = local_generate();
    if input.schema != contract.schema_identifiers.input {
        return failure(
            "model_not_ready",
            "unsupported local generate input schema".into(),
            None,
        );
    }

    let started = Instant::now();
    let timeout = input
        .timeout_s
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .map(Duration::from_secs_f64)
        .unwrap_or(Duration::from_secs(120));

    let connect_input = ConnectInput {
        schema: "solstone-local-connect-input-v1".into(),
        journal_path: input.journal_path.clone(),
        bind_address: input.bind_address,
        default_model_id: input.default_model_id.clone(),
        platform: input.platform,
    };
    let server = match connector(connect_input) {
        ConnectOutcome::Ready { server } => server,
        ConnectOutcome::Loading { reason } => return failure("model_loading", reason, None),
        ConnectOutcome::NotReady { reason } | ConnectOutcome::Failed { reason } => {
            return failure("model_not_ready", reason, None);
        }
    };

    let window = resolve_context_window(&input, &server, transport);
    let prepared = match prepare_bundled_request(&input, &server, window, |text| {
        count_tokens(transport, &server.base_url, text)
    }) {
        Ok(prepared) => prepared,
        Err(error) => return GenerateResult::Failure(error.into_failure()),
    };
    let context_for = |admission_slot, queue_wait_ms, timed_out| InferenceContext {
        server: &server,
        started,
        admission_slot,
        queue_wait_ms,
        attempt_index: input.attempt_index,
        timed_out,
    };

    let permit = match remaining_timeout(started, timeout).and_then(|remaining| {
        acquire_local_slot(
            &admission_dir(std::path::Path::new(&input.journal_path)),
            server.parallel_slots,
            Some(remaining),
            input.exclusive_admission,
        )
        .map_err(|error| match error {
            AdmissionError::Timeout => RemainingTimeout::AdmissionTimeout,
            AdmissionError::Io(error) => RemainingTimeout::AdmissionError(error.to_string()),
        })
    }) {
        Ok(permit) => permit,
        Err(RemainingTimeout::AdmissionTimeout) => {
            return failure_with_inference(
                Some("admission_timeout"),
                "Local inference queue exceeded its request deadline.".into(),
                context_for(None, elapsed_ms(started), true),
                "error",
                None,
                None,
            );
        }
        Err(RemainingTimeout::AdmissionError(detail)) => {
            return failure_with_inference(
                None,
                detail,
                context_for(None, elapsed_ms(started), false),
                "error",
                None,
                None,
            );
        }
        Err(RemainingTimeout::PostError(detail)) => {
            return failure_with_inference(
                None,
                detail,
                context_for(None, elapsed_ms(started), false),
                "error",
                None,
                None,
            );
        }
    };
    let admission_slot = permit.slot_index;
    let queue_wait_ms = permit.queue_wait_ms;
    let inference_context = context_for(Some(admission_slot), queue_wait_ms, false);
    // Hold the permit through interpret and at most one empty_completion retry
    // POST, then release before returning the result.
    let mut post = || {
        post_completion(
            transport,
            &server.base_url,
            &prepared.body,
            started,
            timeout,
        )
    };
    let fail_post = |error: RemainingTimeout| match error {
        RemainingTimeout::AdmissionTimeout => failure_with_inference(
            Some("admission_timeout"),
            "Local inference request exceeded its deadline before posting.".into(),
            context_for(Some(admission_slot), queue_wait_ms, true),
            "error",
            None,
            None,
        ),
        RemainingTimeout::PostError(detail) | RemainingTimeout::AdmissionError(detail) => {
            failure_with_inference(None, detail, inference_context, "error", None, None)
        }
    };
    let response = match post() {
        Ok(response) => response,
        Err(error) => return fail_post(error),
    };
    let interpreted = match interpret_completion(&response) {
        CompletionInterpretation::Failed(CompletionError {
            reason_code: Some(reason),
            ..
        }) if reason == "empty_completion" => match post() {
            Ok(retry) => interpret_completion(&retry),
            Err(error) => return fail_post(error),
        },
        other => other,
    };
    drop(permit);

    let (parsed_response, server_fields) = match interpreted {
        CompletionInterpretation::Ready {
            parsed,
            server_fields,
        } => (parsed, server_fields),
        CompletionInterpretation::Failed(CompletionError {
            reason_code,
            detail,
            server_fields,
        }) => {
            return failure_with_inference(
                reason_code.as_deref(),
                detail,
                inference_context,
                "error",
                None,
                server_fields,
            );
        }
    };

    GenerateResult::Success(GenerateSuccess {
        schema: contract.schema_identifiers.result.clone(),
        outcome: outcome("success"),
        text: parsed_response.text,
        model: input.default_model_id,
        usage: parsed_response.usage,
        finish_reason: parsed_response.finish_reason.clone(),
        input_budget: prepared.input_budget,
        request_budget: prepared.request_budget,
        inference: inference(
            inference_context,
            "success",
            Some(parsed_response.finish_reason),
            None,
            server_fields,
        ),
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreparedRequest {
    pub body: Value,
    pub input_budget: Option<InputBudget>,
    pub request_budget: RequestBudget,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextWindow {
    pub window: u32,
    pub slots: u32,
}

/// Build a bundled request from an injected tokenizer, without I/O other than that tokenizer.
pub fn prepare_bundled_request<F>(
    input: &GenerateInput,
    server: &ConnectedServer,
    context: ContextWindow,
    mut count: F,
) -> Result<PreparedRequest, GenerateError>
where
    F: FnMut(&str) -> u32,
{
    let image_tokens = ESTIMATED_IMAGE_TOKENS.saturating_mul(count_image_parts(&input.contents));
    let required = SAFETY_MARGIN_TOKENS + MIN_COMPLETION_TOKENS;
    if context.window.saturating_sub(image_tokens) < required || image_tokens > context.window {
        return Err(failure_error(
            "context_image_overflow",
            "Local request image content exceeds the local model context window.".into(),
        ));
    }
    let effective_window = context.window - image_tokens;
    let (fitted_contents, input_budget) = fit_contents(
        &input.contents,
        input.system_instruction.as_deref(),
        input.max_output_tokens,
        effective_window,
        &mut count,
    )?;
    let messages = build_messages(&fitted_contents, input.system_instruction.as_deref());
    let estimated_prompt_tokens = count(&serialized_message_text(&messages));
    let room = context
        .window
        .saturating_sub(estimated_prompt_tokens)
        .saturating_sub(image_tokens)
        .saturating_sub(SAFETY_MARGIN_TOKENS);
    if context.window < estimated_prompt_tokens + image_tokens + SAFETY_MARGIN_TOKENS
        || room < MIN_COMPLETION_TOKENS
    {
        return Err(failure_error(
            "context_fitted_overflow",
            "Local request prompt and image content exceed the local model context window.".into(),
        ));
    }
    let clamped_max_tokens = input.max_output_tokens.min(room);
    let request_budget = RequestBudget {
        window: context.window,
        slots: context.slots,
        estimated_prompt_tokens,
        image_tokens,
        clamped_max_tokens,
        requested_max_output_tokens: input.max_output_tokens,
    };
    Ok(PreparedRequest {
        body: build_request_body(
            &server.served_model_id,
            messages,
            input.temperature,
            clamped_max_tokens,
            input.json_output,
            input.json_schema.as_ref(),
            true,
        ),
        input_budget,
        request_budget,
    })
}

pub fn build_request_body(
    model: &str,
    messages: Vec<Value>,
    temperature: f64,
    max_tokens: u32,
    json_output: bool,
    json_schema: Option<&Value>,
    include_qwen_sampling_controls: bool,
) -> Value {
    let mut body = Map::new();
    body.insert("model".into(), Value::String(model.into()));
    body.insert("messages".into(), Value::Array(messages));
    body.insert("temperature".into(), json!(temperature));
    body.insert("max_tokens".into(), json!(max_tokens));
    body.insert("stream".into(), Value::Bool(false));
    if include_qwen_sampling_controls {
        body.insert(
            "chat_template_kwargs".into(),
            json!({"enable_thinking": false}),
        );
        body.insert("top_p".into(), json!(0.8));
        body.insert("top_k".into(), json!(20));
        body.insert("min_p".into(), json!(0.0));
        body.insert("presence_penalty".into(), json!(1.5));
    }
    if let Some(schema) = json_schema {
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
    } else if json_output {
        body.insert("response_format".into(), json!({"type": "json_object"}));
    }
    Value::Object(body)
}

pub fn build_messages(contents: &Value, system_instruction: Option<&str>) -> Vec<Value> {
    let mut messages = Vec::new();
    if let Some(instruction) = system_instruction.filter(|instruction| !instruction.is_empty()) {
        messages.push(json!({"role": "system", "content": instruction}));
    }
    match contents {
        Value::String(text) => messages.push(json!({"role": "user", "content": text})),
        Value::Array(items)
            if items
                .first()
                .and_then(Value::as_object)
                .is_some_and(|item| item.contains_key("role")) =>
        {
            for item in items {
                let item = item.as_object().expect("role-list item checked above");
                let role = item
                    .get("role")
                    .map(python_text)
                    .unwrap_or_else(|| "user".into());
                let content = item
                    .get("content")
                    .map(message_content)
                    .unwrap_or_else(|| Value::String(String::new()));
                messages.push(json!({"role": role, "content": content}));
            }
        }
        Value::Array(_) => {
            messages.push(json!({"role": "user", "content": message_content(contents)}))
        }
        value => messages.push(json!({"role": "user", "content": python_text(value)})),
    }
    messages
}

pub fn prepare_local_schema(schema: &Value) -> Value {
    let mut prepared = schema.clone();
    prepare_schema_node(&mut prepared);
    prepared
}

fn prepare_schema_node(node: &mut Value) {
    match node {
        Value::Object(object) => {
            for key in ["pattern", "minLength", "maxLength", "x-truncate"] {
                object.remove(key);
            }
            let array = matches!(object.get("type"), Some(Value::String(kind)) if kind == "array")
                || matches!(object.get("type"), Some(Value::Array(kinds)) if kinds.iter().any(|kind| kind == "array"));
            if array && !object.contains_key("maxItems") {
                object.insert("maxItems".into(), json!(LOCAL_SCHEMA_MAX_ITEMS));
            }
            for (key, value) in object.iter_mut() {
                if key != "const" && key != "enum" {
                    prepare_schema_node(value);
                }
            }
        }
        Value::Array(values) => values.iter_mut().for_each(prepare_schema_node),
        _ => {}
    }
}

fn post_completion<T: GenerateTransport>(
    transport: &mut T,
    base_url: &str,
    body: &Value,
    started: Instant,
    timeout: Duration,
) -> Result<HttpResponse, RemainingTimeout> {
    remaining_timeout(started, timeout).and_then(|remaining| {
        transport
            .post_json(base_url, "/v1/chat/completions", body, remaining)
            .map_err(RemainingTimeout::PostError)
    })
}

struct CompletionError {
    reason_code: Option<String>,
    detail: String,
    server_fields: Option<ServerInference>,
}

enum CompletionInterpretation {
    Ready {
        parsed: ParsedResponse,
        server_fields: Option<ServerInference>,
    },
    Failed(CompletionError),
}

fn interpret_completion(response: &HttpResponse) -> CompletionInterpretation {
    let parsed = serde_json::from_str::<Value>(&response.body);
    let server_fields = parsed.as_ref().ok().map(server_inference);
    if !(200..300).contains(&response.status) {
        let body_lower = response.body.to_ascii_lowercase();
        let reason_code = if contains_context_pattern(&body_lower) {
            if parsed
                .as_ref()
                .ok()
                .and_then(bundled_error_type)
                .is_some_and(|kind| kind == "exceed_context_size_error")
            {
                Some("context_server_overflow")
            } else {
                Some("capacity_exhausted")
            }
        } else {
            None
        };
        let detail = match reason_code {
            Some("context_server_overflow") => {
                "Local request exceeded the model context window after fitting.".into()
            }
            Some("capacity_exhausted") => CAPACITY_EXHAUSTED_MESSAGE.into(),
            _ => format!("Local server returned HTTP {}.", response.status),
        };
        return CompletionInterpretation::Failed(CompletionError {
            reason_code: reason_code.map(str::to_owned),
            detail,
            server_fields,
        });
    }
    let data = match parsed {
        Ok(data) => data,
        Err(error) => {
            return CompletionInterpretation::Failed(CompletionError {
                reason_code: None,
                detail: format!("Local model response was not valid JSON: {error}"),
                server_fields: None,
            });
        }
    };
    match parse_response(&data) {
        Ok(parsed_response) => CompletionInterpretation::Ready {
            parsed: parsed_response,
            server_fields,
        },
        Err((reason_code, detail)) => CompletionInterpretation::Failed(CompletionError {
            reason_code: Some(reason_code),
            detail,
            server_fields,
        }),
    }
}

pub fn parse_response(data: &Value) -> Result<ParsedResponse, (String, String)> {
    let choices = data
        .get("choices")
        .and_then(Value::as_array)
        .ok_or_else(|| ("response_invalid".into(), "No response from model.".into()))?;
    if choices.is_empty() {
        return Err(("empty_completion".into(), "No response from model.".into()));
    }
    let choice = choices[0].as_object().ok_or_else(|| {
        (
            "response_invalid".into(),
            "Malformed model response.".into(),
        )
    })?;
    let text = choice
        .get("message")
        .and_then(Value::as_object)
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .into();
    let finish_reason = normalize_finish_reason(choice.get("finish_reason"))?;
    Ok(ParsedResponse {
        text,
        usage: extract_usage(data),
        finish_reason,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedResponse {
    pub text: String,
    pub usage: Option<Usage>,
    pub finish_reason: String,
}

pub fn normalize_finish_reason(raw: Option<&Value>) -> Result<String, (String, String)> {
    let raw = raw
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            (
                "response_invalid".into(),
                "Local model response did not include a finish reason.".into(),
            )
        })?;
    let reason = raw.to_ascii_lowercase();
    let normalized = match reason.as_str() {
        "stop" => "stop",
        "length" | "max_tokens" => "max_tokens",
        "content_filter" => "content_filter",
        "tool_calls" | "function_call" => {
            return Err((
                "response_invalid".into(),
                format!("Local model returned unsupported finish reason: {reason}"),
            ));
        }
        _ => {
            return Err((
                "response_invalid".into(),
                format!("Local model returned unknown finish reason: {reason}"),
            ));
        }
    };
    if local_generate()
        .finish_reasons
        .iter()
        .any(|value| value == normalized)
    {
        Ok(normalized.into())
    } else {
        Err((
            "response_invalid".into(),
            "Local result fixture has no finish reason.".into(),
        ))
    }
}

pub fn compute_input_budget(max_output_tokens: u32, window: u32) -> u32 {
    window.saturating_sub(
        max_output_tokens.min(window / OUTPUT_RESERVE_DIVISOR) + SAFETY_MARGIN_TOKENS,
    )
}

pub fn split_entries(block: &str) -> Vec<String> {
    let mut entries = Vec::new();
    let mut current = String::new();
    for line in block.split_inclusive('\n') {
        if is_entry_header(line) && !current.is_empty() {
            entries.push(std::mem::take(&mut current));
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        entries.push(current);
    }
    entries
}

/// Longest character-suffix of `entry` whose token count fits `available`.
///
/// Returns `None` when not even one character fits.
fn fit_entry_tail<F>(entry: &str, available: u32, count: &mut F) -> Option<String>
where
    F: FnMut(&str) -> u32,
{
    if available == 0 {
        return None;
    }
    let characters = entry.chars().collect::<Vec<_>>();
    let (mut low, mut high) = (0_usize, characters.len());
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        let candidate = characters[characters.len() - middle..]
            .iter()
            .collect::<String>();
        if count(&candidate) <= available {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    (low > 0).then(|| characters[characters.len() - low..].iter().collect())
}

pub fn fit_contents<F>(
    contents: &Value,
    system_instruction: Option<&str>,
    max_output_tokens: u32,
    window: u32,
    count: &mut F,
) -> Result<(Value, Option<InputBudget>), GenerateError>
where
    F: FnMut(&str) -> u32,
{
    let (block, preserved, rebuild): (&str, Vec<String>, RebuildContents) = match contents {
        Value::String(block) => (
            block,
            vec![system_instruction.unwrap_or_default().into()],
            Box::new(Value::String),
        ),
        Value::Array(items) if items.first().is_some_and(Value::is_string) => {
            let block = items[0].as_str().expect("first item checked");
            let mut preserved = vec![system_instruction.unwrap_or_default().into()];
            preserved.extend(
                items[1..]
                    .iter()
                    .filter(|item| image_part(item).is_none())
                    .map(python_text),
            );
            let trailing = items[1..].to_vec();
            (
                block,
                preserved,
                Box::new(move |fitted| {
                    let mut output = Vec::with_capacity(trailing.len() + 1);
                    output.push(Value::String(fitted));
                    output.extend(trailing.clone());
                    Value::Array(output)
                }),
            )
        }
        _ => return Ok((contents.clone(), None)),
    };
    let budget = compute_input_budget(max_output_tokens, window);
    let preserved_tokens = preserved
        .iter()
        .filter(|text| !text.is_empty())
        .map(|text| count(text))
        .sum::<u32>();
    if preserved_tokens >= budget {
        return Err(failure_error(
            "context_preserved_overflow",
            "Local request system instruction and preserved prompt content exceed the model context window.".into(),
        ));
    }
    let entries = dedup_adjacent(split_entries(block));
    let fitted_block = entries.concat();
    if preserved_tokens + count(&fitted_block) <= budget {
        return Ok((rebuild(fitted_block), None));
    }
    let marker = format!("{TRUNCATION_MARKER}\n\n");
    let available = budget
        .saturating_sub(preserved_tokens)
        .saturating_sub(count(&marker));
    let mut running = 0;
    let mut kept_reversed = Vec::new();
    for entry in entries.iter().rev() {
        let tokens = count(entry);
        if running + tokens > available {
            break;
        }
        kept_reversed.push(entry.clone());
        running += tokens;
    }
    kept_reversed.reverse();
    // Entries are kept whole, so a block with no `##` headers is one entry and
    // nothing fits: the prompt would reduce to the truncation marker alone and the
    // model would answer confidently about no content at all. Keep the tail of the
    // most recent entry instead -- the part a reader would keep if forced to cut.
    // Entries are kept whole, so a block with no `##` headers is one entry and
    // nothing fits: the prompt would reduce to the truncation marker alone and the
    // model would answer confidently about no content at all. Keep the tail of the
    // most recent entry instead -- the part a reader would keep if forced to cut.
    let mut partial_dropped_chars = 0;
    if kept_reversed.is_empty()
        && let Some(last) = entries.last()
        && let Some(tail) = fit_entry_tail(last, available, count)
    {
        partial_dropped_chars = last.chars().count() - tail.chars().count();
        kept_reversed.push(tail);
    }
    let dropped_entries = entries.len() - kept_reversed.len();
    let dropped_chars = entries[..dropped_entries]
        .iter()
        .map(|entry| entry.chars().count())
        .sum::<usize>()
        + partial_dropped_chars;
    let mut new_block = marker;
    new_block.push_str(&kept_reversed.concat());
    Ok((
        rebuild(new_block),
        Some(InputBudget {
            clipped: true,
            dropped_chars,
            dropped_entries,
            budget_tokens: budget,
        }),
    ))
}

fn resolve_context_window<T: GenerateTransport>(
    input: &GenerateInput,
    server: &ConnectedServer,
    transport: &mut T,
) -> ContextWindow {
    // Keep ConnectOutcome's established wire shape unchanged: readiness and capacity come from
    // connect(), while this one extra /props read supplies n_ctx for budget fitting.
    if let Ok(response) = transport.get(&server.base_url, "/props", Duration::from_secs(1))
        && response.status == 200
        && let Ok(props) = serde_json::from_str::<Value>(&response.body)
        && let Some(context) = props_context(&props)
    {
        let slots = props_slots(&props).unwrap_or(CAPABLE_PARALLEL_SLOTS);
        return ContextWindow {
            window: context / slots,
            slots,
        };
    }
    let sidecar =
        std::fs::read_to_string(std::path::Path::new(&input.journal_path).join("health/local.ctx"))
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
            .filter(|context| *context > 0);
    if let Some(window) = sidecar {
        let slots = slots_from_launched_tier(window).unwrap_or(FLOOR_PARALLEL_SLOTS);
        return ContextWindow { window, slots };
    }
    ContextWindow {
        window: FLOOR_CONTEXT_TOKENS,
        slots: FLOOR_PARALLEL_SLOTS,
    }
}

fn count_tokens<T: GenerateTransport>(transport: &mut T, base_url: &str, text: &str) -> u32 {
    let response = transport.post_json(
        base_url,
        "/tokenize",
        &json!({"content": text}),
        TOKENIZE_TIMEOUT,
    );
    response
        .ok()
        .filter(|response| (200..300).contains(&response.status))
        .and_then(|response| serde_json::from_str::<Value>(&response.body).ok())
        .and_then(|body| {
            body.get("tokens")
                .and_then(Value::as_array)
                .map(|tokens| tokens.len())
        })
        .and_then(|tokens| u32::try_from(tokens).ok())
        .unwrap_or_else(|| estimate_tokens(text))
}

pub fn estimate_tokens(text: &str) -> u32 {
    u32::try_from(text.chars().count().div_ceil(3)).unwrap_or(u32::MAX)
}

fn request(
    method: &str,
    base_url: &str,
    path: &str,
    body: Option<&Value>,
    timeout: Duration,
) -> Result<HttpResponse, String> {
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
        ("get", None) => agent.get(&url).call(),
        ("post", Some(body)) => agent
            .post(&url)
            .header("Content-Type", "application/json")
            .send(serde_json::to_string(body).expect("JSON value serializes")),
        _ => unreachable!("generate transport uses GET or JSON POST"),
    }
    .map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let body = response
        .into_body()
        .read_to_string()
        .map_err(|error| error.to_string())?;
    Ok(HttpResponse { status, body })
}

fn props_context(props: &Value) -> Option<u32> {
    props
        .get("n_ctx")
        .or_else(|| {
            props
                .get("default_generation_settings")
                .and_then(Value::as_object)
                .and_then(|settings| settings.get("n_ctx"))
        })
        .and_then(value_u32)
        .filter(|value| *value > 0)
}

fn props_slots(props: &Value) -> Option<u32> {
    props
        .get("total_slots")
        .and_then(value_u32)
        .filter(|value| *value > 0)
}

fn value_u32(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

fn slots_from_launched_tier(context: u32) -> Option<u32> {
    match context {
        FLOOR_CONTEXT_TOKENS => Some(FLOOR_PARALLEL_SLOTS),
        CAPABLE_CONTEXT_TOKENS => Some(CAPABLE_PARALLEL_SLOTS),
        _ => None,
    }
}

fn message_content(value: &Value) -> Value {
    if contains_image(value) {
        Value::Array(content_parts(value))
    } else if value.is_string() {
        value.clone()
    } else if let Value::Array(values) = value {
        Value::String(
            values
                .iter()
                .map(python_text)
                .collect::<Vec<_>>()
                .join("\n"),
        )
    } else {
        Value::String(python_text(value))
    }
}

fn content_parts(value: &Value) -> Vec<Value> {
    if let Some((mime_type, data)) = image_part(value) {
        return vec![
            json!({"type": "image_url", "image_url": {"url": format!("data:{mime_type};base64,{data}")}}),
        ];
    }
    if let Value::Array(values) = value {
        return values.iter().flat_map(content_parts).collect();
    }
    vec![json!({"type": "text", "text": python_text(value)})]
}

fn contains_image(value: &Value) -> bool {
    image_part(value).is_some()
        || match value {
            Value::Object(object) => object.values().any(contains_image),
            Value::Array(values) => values.iter().any(contains_image),
            _ => false,
        }
}

fn image_part(value: &Value) -> Option<(&str, &str)> {
    let object = value.as_object()?;
    (object.get("type")?.as_str()? == "image").then_some((
        object.get("mime_type")?.as_str()?,
        object.get("data")?.as_str()?,
    ))
}

pub fn count_image_parts(value: &Value) -> u32 {
    u32::from(image_part(value).is_some())
        + match value {
            Value::Object(object) => object.values().map(count_image_parts).sum(),
            Value::Array(values) => values.iter().map(count_image_parts).sum(),
            _ => 0,
        }
}

pub fn serialized_message_text(messages: &[Value]) -> String {
    let mut text = Vec::new();
    for message in messages {
        match message.get("content") {
            Some(Value::String(content)) => text.push(content.clone()),
            Some(Value::Array(parts)) => text.extend(parts.iter().filter_map(|part| {
                (part.get("type") == Some(&Value::String("text".into())))
                    .then(|| part.get("text").and_then(Value::as_str).map(str::to_owned))
                    .flatten()
            })),
            _ => {}
        }
    }
    text.join("\n")
}

pub(crate) fn extract_usage(data: &Value) -> Option<Usage> {
    let usage = data.get("usage")?.as_object()?;
    let input_tokens = integer_or_zero(usage.get("prompt_tokens"));
    let output_tokens = integer_or_zero(usage.get("completion_tokens"));
    let total_tokens = usage
        .get("total_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(input_tokens + output_tokens);
    let cached_tokens = usage
        .get("prompt_tokens_details")
        .and_then(Value::as_object)
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_u64)
        .filter(|tokens| *tokens != 0);
    Some(Usage {
        input_tokens,
        output_tokens,
        total_tokens,
        cached_tokens,
    })
}

pub(crate) fn integer_or_zero(value: Option<&Value>) -> u64 {
    value.and_then(Value::as_u64).unwrap_or(0)
}

fn bundled_error_type(data: &Value) -> Option<&str> {
    data.get("error")?.as_object()?.get("type")?.as_str()
}

fn contains_context_pattern(text: &str) -> bool {
    CONTEXT_PATTERNS
        .iter()
        .any(|pattern| text.contains(pattern))
}

fn server_inference(data: &Value) -> ServerInference {
    let timings = data.get("timings").and_then(Value::as_object);
    let usage = data.get("usage").and_then(Value::as_object);
    let prompt_details = usage
        .and_then(|usage| usage.get("prompt_tokens_details"))
        .and_then(Value::as_object);
    let cached = timings
        .and_then(|timings| number(timings.get("cache_n")))
        .or_else(|| prompt_details.and_then(|details| number(details.get("cached_tokens"))));
    let selected_slot = number(data.get("id_slot"))
        .or_else(|| number(data.get("slot_id")))
        .or_else(|| timings.and_then(|timings| number(timings.get("slot_id"))));
    let prompt_eval_ms = timings.and_then(|timings| number(timings.get("prompt_ms")));
    let generation_ms = timings.and_then(|timings| number(timings.get("predicted_ms")));
    ServerInference {
        prompt_eval_ms,
        generation_ms,
        server_total_ms: (prompt_eval_ms.is_some() || generation_ms.is_some())
            .then(|| prompt_eval_ms.unwrap_or(0.0) + generation_ms.unwrap_or(0.0)),
        prompt_tokens: usage
            .and_then(|usage| number(usage.get("prompt_tokens")))
            .or_else(|| timings.and_then(|timings| number(timings.get("prompt_n")))),
        generated_tokens: usage
            .and_then(|usage| number(usage.get("completion_tokens")))
            .or_else(|| timings.and_then(|timings| number(timings.get("predicted_n")))),
        prompt_cached_tokens: cached,
        selected_slot,
        prompt_cache_state: if cached.is_some_and(|cached| cached > 0.0) {
            "warm".into()
        } else if cached.is_some() {
            "cold".into()
        } else {
            "unknown".into()
        },
    }
}

fn number(value: Option<&Value>) -> Option<f64> {
    value.and_then(|value| match value {
        Value::Number(number) => number.as_f64(),
        _ => None,
    })
}

fn failure(reason_code: &str, detail: String, inference: Option<Inference>) -> GenerateResult {
    GenerateResult::Failure(failure_record(reason_code, detail, inference))
}

fn failure_error(reason_code: &str, detail: String) -> GenerateError {
    debug_assert!(local_generate().reason_codes.contains_key(reason_code));
    GenerateError {
        reason_code: reason_code.into(),
        detail,
    }
}

fn failure_record(
    reason_code: &str,
    detail: String,
    inference: Option<Inference>,
) -> GenerateFailure {
    debug_assert!(local_generate().reason_codes.contains_key(reason_code));
    GenerateFailure {
        schema: local_generate().schema_identifiers.result.clone(),
        outcome: outcome("failure"),
        reason_code: Some(reason_code.into()),
        detail,
        inference,
    }
}

#[derive(Clone, Copy)]
struct InferenceContext<'a> {
    server: &'a ConnectedServer,
    started: Instant,
    admission_slot: Option<u32>,
    queue_wait_ms: f64,
    attempt_index: u32,
    timed_out: bool,
}

fn failure_with_inference(
    reason_code: Option<&str>,
    detail: String,
    context: InferenceContext<'_>,
    inference_outcome: &str,
    finish_reason: Option<String>,
    server_fields: Option<ServerInference>,
) -> GenerateResult {
    if let Some(reason_code) = reason_code {
        debug_assert!(local_generate().reason_codes.contains_key(reason_code));
    }
    GenerateResult::Failure(GenerateFailure {
        schema: local_generate().schema_identifiers.result.clone(),
        outcome: outcome("failure"),
        reason_code: reason_code.map(str::to_owned),
        detail,
        inference: Some(inference(
            context,
            inference_outcome,
            finish_reason,
            reason_code.map(str::to_owned),
            server_fields,
        )),
    })
}

fn inference(
    context: InferenceContext<'_>,
    outcome_value: &str,
    finish_reason: Option<String>,
    reason_code: Option<String>,
    server_fields: Option<ServerInference>,
) -> Inference {
    Inference {
        profile: context.server.profile.clone(),
        serving_capacity: context.server.parallel_slots,
        capacity_source: context.server.capacity_source.clone(),
        admission_slot: context.admission_slot,
        queue_wait_ms: context.queue_wait_ms,
        client_total_ms: elapsed_ms(context.started),
        retry_index: Some(context.attempt_index),
        outcome: outcome_value.into(),
        finish_reason,
        reason_code,
        timed_out: context.timed_out,
        cancelled: false,
        server: server_fields,
    }
}

enum RemainingTimeout {
    AdmissionTimeout,
    AdmissionError(String),
    PostError(String),
}

fn remaining_timeout(started: Instant, timeout: Duration) -> Result<Duration, RemainingTimeout> {
    timeout
        .checked_sub(started.elapsed())
        .filter(|remaining| !remaining.is_zero())
        .ok_or(RemainingTimeout::AdmissionTimeout)
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1_000.0
}

fn outcome(name: &str) -> String {
    local_generate()
        .outcomes
        .iter()
        .find(|value| value.as_str() == name)
        .cloned()
        .expect("local generate fixture must define required outcome")
}

fn dedup_adjacent(entries: Vec<String>) -> Vec<String> {
    let mut deduped = Vec::new();
    for entry in entries {
        if deduped.last() == Some(&entry) {
            continue;
        }
        deduped.push(entry);
    }
    deduped
}

fn is_entry_header(line: &str) -> bool {
    let mut chars = line.chars();
    let (first, second, third, fourth) = (chars.next(), chars.next(), chars.next(), chars.next());
    first == Some('#')
        && second == Some('#')
        && (third.is_some_and(char::is_whitespace)
            || (third == Some('#') && fourth.is_some_and(char::is_whitespace)))
}

fn python_text(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => "None".into(),
        Value::Bool(value) => if *value { "True" } else { "False" }.into(),
        Value::Number(value) => value.to_string(),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(python_text)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(object) => format!(
            "{{{}}}",
            object
                .iter()
                .map(|(key, value)| format!("'{key}': {}", python_text(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(contents: Value) -> GenerateInput {
        GenerateInput {
            schema: local_generate().schema_identifiers.input.clone(),
            journal_path: "/unused".into(),
            bind_address: LoopbackAddr::IPV4_LOOPBACK,
            default_model_id: "default".into(),
            platform: Platform::Linux,
            contents,
            system_instruction: None,
            temperature: 0.3,
            max_output_tokens: 512,
            json_output: false,
            json_schema: None,
            timeout_s: None,
            exclusive_admission: false,
            attempt_index: 0,
        }
    }

    fn server() -> ConnectedServer {
        ConnectedServer {
            model_id: "default".into(),
            served_model_id: "served-model".into(),
            port: 8080,
            base_url: "http://127.0.0.1:8080".into(),
            parallel_slots: 2,
            capacity_source: "props".into(),
            profile: "capable".into(),
        }
    }

    #[test]
    fn fixture_vocabulary_is_loaded_from_local_contract() {
        let fixture = local_generate();
        assert_eq!(fixture.schema_version, 1);
        assert_eq!(
            fixture.reason_codes.get("context_image_overflow"),
            Some(&"context_budget_exceeded".into())
        );
        assert!(
            fixture
                .reference_sources
                .iter()
                .any(|source| source.ends_with("local.py"))
        );
        assert_eq!(fixture.prompt_cache_states, ["warm", "cold", "unknown"]);
    }

    #[test]
    fn request_bodies_match_recorded_python_reference() {
        let text = build_request_body(
            "served-model",
            build_messages(&json!("Hello"), None),
            0.3,
            512,
            false,
            None,
            true,
        );
        assert_eq!(
            serde_json::to_string(&text).unwrap(),
            r#"{"model":"served-model","messages":[{"role":"user","content":"Hello"}],"temperature":0.3,"max_tokens":512,"stream":false,"chat_template_kwargs":{"enable_thinking":false},"top_p":0.8,"top_k":20,"min_p":0.0,"presence_penalty":1.5}"#
        );

        let image = json!(["look", {"type":"image","mime_type":"image/png","data":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4AWP4z8AAAAMBAQDbfS68AAAAAElFTkSuQmCC"}]);
        let image_body = build_request_body(
            "served-model",
            build_messages(&image, None),
            0.2,
            256,
            false,
            None,
            true,
        );
        assert_eq!(
            serde_json::to_string(&image_body).unwrap(),
            r#"{"model":"served-model","messages":[{"role":"user","content":[{"type":"text","text":"look"},{"type":"image_url","image_url":{"url":"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4AWP4z8AAAAMBAQDbfS68AAAAAElFTkSuQmCC"}}]}],"temperature":0.2,"max_tokens":256,"stream":false,"chat_template_kwargs":{"enable_thinking":false},"top_p":0.8,"top_k":20,"min_p":0.0,"presence_penalty":1.5}"#
        );

        let schema = json!({"type":"object","properties":{"tags":{"type":"array","items":{"type":"string","pattern":"^[a-z]+$","minLength":2,"maxLength":10},"x-truncate":true},"literal":{"const":{"pattern":"must-stay"}}}});
        let schema_body = build_request_body(
            "served-model",
            build_messages(&json!("schema"), None),
            0.5,
            128,
            true,
            Some(&schema),
            true,
        );
        assert_eq!(
            serde_json::to_string(&schema_body).unwrap(),
            r#"{"model":"served-model","messages":[{"role":"user","content":"schema"}],"temperature":0.5,"max_tokens":128,"stream":false,"chat_template_kwargs":{"enable_thinking":false},"top_p":0.8,"top_k":20,"min_p":0.0,"presence_penalty":1.5,"response_format":{"type":"json_schema","json_schema":{"name":"local_schema","schema":{"type":"object","properties":{"tags":{"type":"array","items":{"type":"string"},"maxItems":192},"literal":{"const":{"pattern":"must-stay"}}}},"strict":true}}}"#
        );
    }

    #[test]
    fn request_body_qwen_controls_require_bundled_or_confidential() {
        let qwen_fields = [
            "chat_template_kwargs",
            "top_p",
            "top_k",
            "min_p",
            "presence_penalty",
        ];
        let non_confidential = build_request_body(
            "served-model",
            build_messages(&json!("Hello"), None),
            0.3,
            512,
            false,
            Some(&json!({"type": "object"})),
            false,
        );
        let non_confidential = non_confidential.as_object().unwrap();
        for field in [
            "model",
            "messages",
            "temperature",
            "max_tokens",
            "stream",
            "response_format",
        ] {
            assert!(non_confidential.contains_key(field), "missing {field}");
        }
        for field in qwen_fields {
            assert!(!non_confidential.contains_key(field), "unexpected {field}");
        }

        // Whether a lane sets this flag is the caller's mapping and is asserted
        // in the endpoint arm, which is where `is_confidential` is in scope.
        let gated = build_request_body(
            "served-model",
            build_messages(&json!("Hello"), None),
            0.3,
            512,
            false,
            None,
            true,
        );
        let gated = gated.as_object().unwrap();
        for field in qwen_fields {
            assert!(gated.contains_key(field), "missing {field}");
        }
    }

    #[test]
    fn finish_reason_normalization_matches_reference() {
        for (raw, expected) in [
            ("stop", "stop"),
            ("length", "max_tokens"),
            ("max_tokens", "max_tokens"),
            ("content_filter", "content_filter"),
        ] {
            assert_eq!(
                normalize_finish_reason(Some(&json!(raw))),
                Ok(expected.into())
            );
        }
        for raw in ["tool_calls", "function_call", "other"] {
            assert!(
                matches!(normalize_finish_reason(Some(&json!(raw))), Err((reason, _)) if reason == "response_invalid")
            );
        }
        assert!(
            matches!(normalize_finish_reason(Some(&json!("  "))), Err((reason, _)) if reason == "response_invalid")
        );
    }

    #[test]
    fn empty_content_with_valid_finish_reason_is_successful() {
        let parsed =
            parse_response(&json!({"choices":[{"message":{"content":""},"finish_reason":"stop"}]}))
                .unwrap();
        assert_eq!(parsed.text, "");
        assert_eq!(parsed.finish_reason, "stop");
        assert!(
            matches!(parse_response(&json!({"choices":[{"message":{"content":""},"finish_reason":""}]})), Err((reason, _)) if reason == "response_invalid")
        );
    }

    #[test]
    fn truncation_marker_and_fitted_contents_match_reference() {
        assert_eq!(
            TRUNCATION_MARKER,
            "[earlier input truncated to fit the on-device model's context]"
        );
        let source = format!("## old\n{}\n## newest\nb\n", "a".repeat(100));
        let (fitted, budget) = fit_contents(&json!(source), None, 256, 448, &mut |text| {
            u32::try_from(text.len()).unwrap()
        })
        .unwrap();
        assert_eq!(
            fitted,
            json!(format!("{TRUNCATION_MARKER}\n\n## newest\nb\n"))
        );
        assert_eq!(budget.unwrap().dropped_entries, 1);
    }

    /// A block with no `##` headers is a single entry, so keeping entries whole
    /// leaves nothing and the prompt collapses to the marker alone. The model then
    /// answers confidently about no content. Keep the tail of the entry instead.
    #[test]
    fn an_oversized_headerless_block_keeps_its_tail_rather_than_only_the_marker() {
        let source = format!("{}TAIL-KEEP-ME", "a".repeat(8_000));
        // One char per token keeps the arithmetic checkable: budget is
        // 2048 - (min(256, 2048/4) + 256) = 1536, less the marker.
        let (fitted, budget) = fit_contents(&json!(source), None, 256, 2_048, &mut |text| {
            u32::try_from(text.len()).unwrap()
        })
        .unwrap();

        let text = fitted.as_str().expect("fitted block is a string");
        let marker = format!("{TRUNCATION_MARKER}\n\n");
        assert!(text.starts_with(&marker));
        // The newest characters survive rather than the whole entry being dropped.
        assert!(text.ends_with("TAIL-KEEP-ME"), "tail was dropped: {text:?}");
        let kept = text.len() - marker.len();
        let available = 1_536 - marker.len();
        assert!(
            kept > available / 2 && kept <= available,
            "kept {kept} of {available} available"
        );
        let budget = budget.expect("clipped");
        assert!(budget.clipped);
        assert_eq!(budget.dropped_chars, source.chars().count() - kept);
    }

    #[test]
    fn fit_contents_excludes_only_direct_trailing_image_parts_from_preserved_budget() {
        let data = "x".repeat(4_096);
        let image = json!({"type":"image","mime_type":"image/png","data":data.clone()});
        let mut count = |text: &str| u32::try_from(text.len()).unwrap();

        let contents = json!(["short", image]);
        let (fitted, budget) = fit_contents(&contents, None, 256, 1_024, &mut count).unwrap();
        assert_eq!(fitted, contents);
        assert_eq!(fitted[1]["data"], data);
        assert_eq!(budget, None);

        let contents = json!(["short", image, "ordinary sibling"]);
        let (fitted, budget) = fit_contents(&contents, None, 256, 1_024, &mut count).unwrap();
        assert_eq!(fitted, contents);
        assert_eq!(budget, None);

        let contents = json!(["short", image, data]);
        assert!(
            matches!(fit_contents(&contents, None, 256, 1_024, &mut count), Err(GenerateError { reason_code, .. }) if reason_code == "context_preserved_overflow")
        );
    }

    #[test]
    fn fit_contents_counts_oversized_non_image_objects_as_preserved_content() {
        let data = "x".repeat(4_096);
        let contents = json!([
            "short",
            {"type":"other","mime_type":"image/png","data":data},
        ]);
        let mut count = |text: &str| u32::try_from(text.len()).unwrap();

        assert!(
            matches!(fit_contents(&contents, None, 256, 1_024, &mut count), Err(GenerateError { reason_code, .. }) if reason_code == "context_preserved_overflow")
        );
    }

    #[test]
    fn bundled_text_first_image_uses_only_the_image_token_estimate() {
        let data = "x".repeat(7_000);
        let request = input(json!([
            "short",
            {"type":"image","mime_type":"image/png","data":data.clone()},
        ]));
        let prepared = prepare_bundled_request(
            &request,
            &server(),
            ContextWindow {
                window: 10_000,
                slots: 1,
            },
            |text| u32::try_from(text.len()).unwrap(),
        )
        .unwrap();

        assert_eq!(prepared.request_budget.image_tokens, ESTIMATED_IMAGE_TOKENS);
        assert_eq!(
            serde_json::to_string(&prepared.body).unwrap(),
            format!(
                r#"{{"model":"served-model","messages":[{{"role":"user","content":[{{"type":"text","text":"short"}},{{"type":"image_url","image_url":{{"url":"data:image/png;base64,{data}"}}}}]}}],"temperature":0.3,"max_tokens":512,"stream":false,"chat_template_kwargs":{{"enable_thinking":false}},"top_p":0.8,"top_k":20,"min_p":0.0,"presence_penalty":1.5}}"#
            )
        );
    }

    #[test]
    fn clamps_completion_and_refuses_fitted_overflow() {
        let mut request = input(json!("short"));
        request.max_output_tokens = 2_000;
        let prepared = prepare_bundled_request(
            &request,
            &server(),
            ContextWindow {
                window: 1_000,
                slots: 1,
            },
            |_| 100,
        )
        .unwrap();
        assert_eq!(prepared.request_budget.clamped_max_tokens, 644);
        assert_eq!(prepared.body["max_tokens"], 644);

        let overflowing = prepare_bundled_request(
            &input(json!("prompt")),
            &server(),
            ContextWindow {
                window: 600,
                slots: 1,
            },
            |_| 400,
        );
        assert!(
            matches!(overflowing, Err(GenerateError { reason_code, .. }) if reason_code == "context_fitted_overflow")
        );
    }

    #[test]
    fn image_and_fitted_overflows_are_distinct() {
        let image = json!([{"type":"image","mime_type":"image/png","data":"x"}]);
        let image_error = prepare_bundled_request(
            &input(image),
            &server(),
            ContextWindow {
                window: 750,
                slots: 1,
            },
            |_| 1,
        );
        let fitted_error = prepare_bundled_request(
            &input(json!("prompt")),
            &server(),
            ContextWindow {
                window: 600,
                slots: 1,
            },
            |_| 400,
        );
        let image_code = match image_error {
            Err(error) => error.reason_code,
            other => panic!("expected image refusal: {other:?}"),
        };
        let fitted_code = match fitted_error {
            Err(error) => error.reason_code,
            other => panic!("expected fitted refusal: {other:?}"),
        };
        assert_eq!(image_code, "context_image_overflow");
        assert_eq!(fitted_code, "context_fitted_overflow");
        assert_ne!(image_code, fitted_code);
        assert_ne!(image_code, "context_server_overflow");
        assert_ne!(fitted_code, "capacity_exhausted");
    }

    #[test]
    fn parseable_shape_errors_are_classified_but_invalid_json_is_not() {
        assert!(
            matches!(parse_response(&json!({"choices":"bad"})), Err((reason, _)) if reason == "response_invalid")
        );
        assert!(serde_json::from_str::<Value>("not json").is_err());
    }

    #[test]
    fn empty_choices_array_is_empty_completion() {
        assert_eq!(
            parse_response(&json!({"choices": []})),
            Err(("empty_completion".into(), "No response from model.".into()))
        );
    }

    struct ScriptedTransport {
        completions: std::collections::VecDeque<Result<HttpResponse, String>>,
        completion_posts: usize,
    }

    impl ScriptedTransport {
        fn new(completions: impl IntoIterator<Item = Result<HttpResponse, String>>) -> Self {
            Self {
                completions: completions.into_iter().collect(),
                completion_posts: 0,
            }
        }
    }

    impl GenerateTransport for ScriptedTransport {
        fn get(
            &mut self,
            _base_url: &str,
            _path: &str,
            _timeout: Duration,
        ) -> Result<HttpResponse, String> {
            Err("scripted transport has no /props".into())
        }

        fn post_json(
            &mut self,
            _base_url: &str,
            path: &str,
            _body: &Value,
            _timeout: Duration,
        ) -> Result<HttpResponse, String> {
            if path != "/v1/chat/completions" {
                return Err("scripted transport has no tokenize".into());
            }
            self.completion_posts += 1;
            self.completions
                .pop_front()
                .unwrap_or_else(|| Err("no more scripted completion posts".into()))
        }
    }

    fn ok_http(body: Value) -> Result<HttpResponse, String> {
        Ok(HttpResponse {
            status: 200,
            body: body.to_string(),
        })
    }

    fn generate_against(
        completions: impl IntoIterator<Item = Result<HttpResponse, String>>,
    ) -> (GenerateResult, usize) {
        let root = tempfile::tempdir().expect("journal");
        let mut request = input(json!("Hello"));
        request.journal_path = root.path().to_str().expect("utf-8 journal path").to_owned();
        let mut transport = ScriptedTransport::new(completions);
        let result = generate_with(request, &mut transport, |_| ConnectOutcome::Ready {
            server: server(),
        });
        (result, transport.completion_posts)
    }

    #[test]
    fn empty_completion_retries_once_and_can_succeed() {
        let (result, posts) = generate_against([
            ok_http(json!({"choices": []})),
            ok_http(json!({
                "choices": [{"message": {"content": "hello"}, "finish_reason": "stop"}]
            })),
        ]);
        let GenerateResult::Success(success) = result else {
            panic!("expected success after retry, got {result:?}");
        };
        assert_eq!(success.text, "hello");
        assert_eq!(success.finish_reason, "stop");
        assert_eq!(posts, 2);
    }

    #[test]
    fn empty_completion_retry_that_is_also_empty_is_final() {
        let (result, posts) = generate_against([
            ok_http(json!({"choices": []})),
            ok_http(json!({"choices": []})),
        ]);
        let GenerateResult::Failure(failure) = result else {
            panic!("expected failure after two empty completions, got {result:?}");
        };
        assert_eq!(failure.reason_code.as_deref(), Some("empty_completion"));
        assert_eq!(posts, 2);
    }

    #[test]
    fn non_array_choices_does_not_retry() {
        let (result, posts) = generate_against([ok_http(json!({"choices": "bad"}))]);
        let GenerateResult::Failure(failure) = result else {
            panic!("expected response_invalid without retry, got {result:?}");
        };
        assert_eq!(failure.reason_code.as_deref(), Some("response_invalid"));
        assert_eq!(posts, 1);
    }
}
