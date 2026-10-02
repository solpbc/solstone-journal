// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! ChatGPT plan Responses API generation.

use std::io::Read;
use std::path::Path;
use std::time::Duration;

use serde_json::{Map, Value, json};
use solstone_core_chatgpt_auth::{ChatGptAuthManager, ChatGptCredential, CredentialError};
use solstone_core_generate::GenerateRequest;

use crate::endpoint::EndpointTransportError;
use crate::openai::{
    OpenAiGenerated, OpenAiResult, RequestBodyMode, capture_provider_detail,
    is_context_window_error, parse_response, request_body, request_timeout,
};
use crate::thinking::{byo_thinking, openai_efforts, shared_ceiling};

const OPENAI_BASE_URL: &str = "https://api.openai.com";
const OPENAI_RESPONSES_PATH: &str = "/v1/responses";
const MAX_BUFFER_BYTES: usize = 4 * 1024 * 1024; // 4 MiB

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatGptFailure {
    pub reason_code: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChatGptResult {
    Generated(OpenAiGenerated),
    Failed(ChatGptFailure),
}

pub(crate) enum ChatGptPost {
    Body { status: u16, body: String },
    Stream { reader: Box<dyn Read + Send> },
}

pub(crate) trait ResponsesTransport {
    fn post(
        &mut self,
        base_url: &str,
        path: &str,
        body: &Value,
        bearer: &str,
        timeout: Duration,
    ) -> Result<ChatGptPost, EndpointTransportError>;
}

#[derive(Default)]
pub struct UreqResponsesTransport;

impl ResponsesTransport for UreqResponsesTransport {
    fn post(
        &mut self,
        base_url: &str,
        path: &str,
        body: &Value,
        bearer: &str,
        timeout: Duration,
    ) -> Result<ChatGptPost, EndpointTransportError> {
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
            .header("Authorization", &format!("Bearer {bearer}"))
            .send(serde_json::to_string(body).expect("JSON value serializes"))
            .map_err(classify_ureq_error)?;
        let status = response.status().as_u16();
        if (200..300).contains(&status) {
            Ok(ChatGptPost::Stream {
                reader: Box::new(response.into_body().into_reader()),
            })
        } else {
            let body = response
                .into_body()
                .read_to_string()
                .map_err(classify_ureq_error)?;
            Ok(ChatGptPost::Body { status, body })
        }
    }
}

/// Generate text using the owner's ChatGPT plan through the Responses API.
/// ChatGPT plan Responses generation can make up to two credential calls of
/// up to 75s each, on top of the request timeouts.
pub fn chatgpt_generate(
    request: &GenerateRequest,
    config: &Map<String, Value>,
    journal: &Path,
) -> ChatGptResult {
    let auth = ChatGptAuthManager::with_default_transport(journal.to_path_buf());
    let mut transport = UreqResponsesTransport;
    chatgpt_generate_with(request, config, &auth, &mut transport)
}

pub(crate) fn chatgpt_generate_with<C: ChatGptCredential, T: ResponsesTransport>(
    request: &GenerateRequest,
    config: &Map<String, Value>,
    credential: &C,
    transport: &mut T,
) -> ChatGptResult {
    chatgpt_generate_with_lookup(
        request,
        config,
        credential,
        transport,
        crate::overrides::non_blank_process_env,
    )
}

fn chatgpt_generate_with_lookup<C: ChatGptCredential, T: ResponsesTransport>(
    request: &GenerateRequest,
    config: &Map<String, Value>,
    credential: &C,
    transport: &mut T,
    env: impl Fn(&str) -> Option<String>,
) -> ChatGptResult {
    let env = &env;
    let Some(model) = crate::overrides::configured_model_with(config, env) else {
        return failure("model_missing");
    };
    let base_url = crate::overrides::configured_base_url_with(config, OPENAI_BASE_URL, env);
    let thinking = byo_thinking(config);
    let efforts = openai_efforts(thinking);

    let mut bearer = match credential.access_token() {
        Ok(token) => token,
        Err(err) => return failure_from_credential_error(err),
    };
    let mut sent_bearers = vec![bearer.clone()];

    let mut step = 0;
    let mut token_retried = false;
    while step < efforts.len() {
        let body = request_body(
            request,
            &model,
            thinking,
            efforts[step],
            RequestBodyMode::ChatGptPlan,
        );
        let post_result = transport.post(
            &base_url,
            OPENAI_RESPONSES_PATH,
            &body,
            &bearer,
            request_timeout(request.timeout_s, crate::thinking::cloud_room(thinking)),
        );
        let post = match post_result {
            Ok(post) => post,
            Err(EndpointTransportError::Connection) => return failure("network_unreachable"),
            Err(EndpointTransportError::Capacity) => return failure("provider_unavailable"),
            Err(EndpointTransportError::ClosedBeforeResponse | EndpointTransportError::Other) => {
                return failure("provider_response_invalid");
            }
        };

        match post {
            ChatGptPost::Body {
                status,
                body: body_str,
            } => {
                let parsed_val = serde_json::from_str::<Value>(&body_str).unwrap_or(Value::Null);
                let extracted = extract_error_fields(&parsed_val);

                // Row 0: 400 only, step down if param == "reasoning.effort"
                if status == 400
                    && extracted.param.as_deref() == Some("reasoning.effort")
                    && step + 1 < efforts.len()
                {
                    step += 1;
                    continue;
                }

                // Row 1: subscription_sharing_invalid_user calls mark_token_rejected and returns sign-in required without refresh
                let norm_code = extracted
                    .code
                    .as_deref()
                    .map(|c| c.replace("_v2_", "_").replace("_v2", ""));
                if norm_code.as_deref() == Some("subscription_sharing_invalid_user") {
                    let _ = credential.mark_token_rejected(&bearer);
                    let detail = capture_detail_scrubbed(&body_str, &sent_bearers);
                    return ChatGptResult::Failed(ChatGptFailure {
                        reason_code: Some("chatgpt_sign_in_required".to_owned()),
                        detail,
                    });
                }

                // Row 10: 401 refresh
                if status == 401 && !token_retried {
                    token_retried = true;
                    match credential.access_token_after_rejection(&bearer) {
                        Ok(new_token) => {
                            bearer = new_token.clone();
                            sent_bearers.push(new_token);
                            continue;
                        }
                        Err(err) => return failure_from_credential_error(err),
                    }
                }
                if status == 401 && token_retried {
                    let _ = credential.mark_token_rejected(&bearer);
                    let detail = capture_detail_scrubbed(&body_str, &sent_bearers);
                    return ChatGptResult::Failed(ChatGptFailure {
                        reason_code: Some("chatgpt_sign_in_required".to_owned()),
                        detail,
                    });
                }

                let reason_code =
                    classify_http_error(status, &body_str, &extracted, credential, &bearer);
                let detail = capture_detail_scrubbed(&body_str, &sent_bearers);
                return ChatGptResult::Failed(ChatGptFailure {
                    reason_code: Some(reason_code.to_owned()),
                    detail,
                });
            }
            ChatGptPost::Stream { reader } => {
                return parse_stream(
                    reader,
                    request,
                    &model,
                    thinking,
                    credential,
                    &bearer,
                    &sent_bearers,
                );
            }
        }
    }

    failure("provider_response_invalid")
}

#[derive(Default, Debug)]
struct ExtractedError {
    code: Option<String>,
    param: Option<String>,
}

fn extract_error_fields(val: &Value) -> ExtractedError {
    let mut code: Option<String> = None;
    let mut code_frozen_by_string_error = false;
    let mut param: Option<String> = None;

    let mut current = val;
    for _ in 0..4 {
        let Some(obj) = current.as_object() else {
            break;
        };

        if !code_frozen_by_string_error && code.is_none() {
            if let Some(s) = obj.get("error").and_then(Value::as_str) {
                code = Some(s.to_owned());
                code_frozen_by_string_error = true;
            } else if let Some(c) = obj.get("code").and_then(Value::as_str) {
                code = Some(c.to_owned());
            }
        }

        if let Some(p) = obj.get("param").and_then(Value::as_str) {
            param = Some(p.to_owned());
        }

        if let Some(err_obj) = obj.get("error").filter(|v| v.is_object()) {
            current = err_obj;
        } else if let Some(det_obj) = obj.get("detail").filter(|v| v.is_object()) {
            current = det_obj;
        } else {
            break;
        }
    }

    ExtractedError { code, param }
}

fn classify_http_error<C: ChatGptCredential>(
    status: u16,
    body_str: &str,
    extracted: &ExtractedError,
    credential: &C,
    bearer: &str,
) -> &'static str {
    let norm_code = extracted
        .code
        .as_deref()
        .map(|c| c.replace("_v2_", "_").replace("_v2", ""));
    if let Some(c) = norm_code.as_deref() {
        if c == "subscription_sharing_invalid_user" {
            let _ = credential.mark_token_rejected(bearer);
            return "chatgpt_sign_in_required";
        }
        if c == "subscription_sharing_user_not_eligible"
            || c == "subscription_sharing_client_not_enabled"
            || c == "subscription_sharing_unsupported_capability"
        {
            return "chatgpt_not_eligible";
        }
        if c == "usage_limit_reached" || c == "rate_limit_exceeded" {
            return "chatgpt_usage_limit";
        }
        if c.starts_with("chatpass") {
            return "provider_request_rejected";
        }
        if c == "model_not_found" {
            return "model_not_found";
        }
        if c == "server_error" {
            return "provider_unavailable";
        }
    }

    if status == 404 {
        return "model_not_found";
    }
    if (500..=599).contains(&status) {
        return "provider_unavailable";
    }
    if status == 429 {
        return "chatgpt_usage_limit";
    }
    if is_context_window_error(body_str) {
        return "context_window_exceeded";
    }
    if status == 403 {
        let trimmed = body_str.trim_start();
        if extracted.code.is_none()
            && (trimmed.starts_with('<') || trimmed.to_ascii_lowercase().starts_with("<!doctype"))
        {
            return "chatgpt_not_eligible";
        }
        return "provider_request_rejected";
    }
    if status == 400 {
        return "provider_request_rejected";
    }
    "provider_response_invalid"
}

fn classify_stream_error<C: ChatGptCredential>(
    err_val: &Value,
    credential: &C,
    bearer: &str,
) -> &'static str {
    let extracted = extract_error_fields(err_val);
    let norm_code = extracted
        .code
        .as_deref()
        .map(|c| c.replace("_v2_", "_").replace("_v2", ""));
    match norm_code.as_deref() {
        Some("subscription_sharing_invalid_user") => {
            let _ = credential.mark_token_rejected(bearer);
            "chatgpt_sign_in_required"
        }
        Some(
            "subscription_sharing_user_not_eligible"
            | "subscription_sharing_client_not_enabled"
            | "subscription_sharing_unsupported_capability",
        ) => "chatgpt_not_eligible",
        Some("usage_limit_reached" | "rate_limit_exceeded") => "chatgpt_usage_limit",
        Some(c) if c.starts_with("chatpass") => "provider_request_rejected",
        Some("model_not_found") => "model_not_found",
        Some("server_error") => "provider_unavailable",
        _ => "provider_response_invalid",
    }
}

fn failure_from_credential_error(err: CredentialError) -> ChatGptResult {
    match err {
        CredentialError::SignInRequired => ChatGptResult::Failed(ChatGptFailure {
            reason_code: Some("chatgpt_sign_in_required".to_owned()),
            detail: Some(err.to_string()),
        }),
        CredentialError::NotEligible => ChatGptResult::Failed(ChatGptFailure {
            reason_code: Some("chatgpt_not_eligible".to_owned()),
            detail: None,
        }),
        CredentialError::Network => ChatGptResult::Failed(ChatGptFailure {
            reason_code: Some("network_unreachable".to_owned()),
            detail: None,
        }),
        CredentialError::Unavailable => ChatGptResult::Failed(ChatGptFailure {
            reason_code: Some("provider_unavailable".to_owned()),
            detail: None,
        }),
        CredentialError::Busy => ChatGptResult::Failed(ChatGptFailure {
            reason_code: Some("pipeline_unavailable".to_owned()),
            detail: Some("Busy".to_owned()),
        }),
        CredentialError::Io(kind) => ChatGptResult::Failed(ChatGptFailure {
            reason_code: Some("pipeline_unavailable".to_owned()),
            detail: Some(kind),
        }),
        CredentialError::GrantNotSaved => ChatGptResult::Failed(ChatGptFailure {
            reason_code: Some("pipeline_unavailable".to_owned()),
            detail: Some("GrantNotSaved".to_owned()),
        }),
        CredentialError::Storage(_) => ChatGptResult::Failed(ChatGptFailure {
            reason_code: Some("pipeline_unavailable".to_owned()),
            detail: Some(err.to_string()),
        }),
        CredentialError::Refused(maybe_code) => {
            if let Some(code) = maybe_code {
                let norm = code.replace("_v2_", "_").replace("_v2", "");
                let reason_code = match norm.as_str() {
                    "subscription_sharing_invalid_user" => "chatgpt_sign_in_required",
                    "subscription_sharing_user_not_eligible"
                    | "subscription_sharing_client_not_enabled"
                    | "subscription_sharing_unsupported_capability" => "chatgpt_not_eligible",
                    "usage_limit_reached" | "rate_limit_exceeded" => "chatgpt_usage_limit",
                    _ => "provider_response_invalid",
                };
                ChatGptResult::Failed(ChatGptFailure {
                    reason_code: Some(reason_code.to_owned()),
                    detail: Some(code),
                })
            } else {
                ChatGptResult::Failed(ChatGptFailure {
                    reason_code: Some("provider_response_invalid".to_owned()),
                    detail: None,
                })
            }
        }
        CredentialError::Malformed(field) => ChatGptResult::Failed(ChatGptFailure {
            reason_code: Some("provider_response_invalid".to_owned()),
            detail: field,
        }),
    }
}

fn scrub_secrets(mut text: String, secrets: &[String]) -> String {
    for secret in secrets {
        if !secret.is_empty() {
            text = text.replace(secret, "[REDACTED]");
        }
    }
    text
}

fn capture_detail_scrubbed(body: &str, secrets: &[String]) -> Option<String> {
    let scrubbed = scrub_secrets(body.to_owned(), secrets);
    capture_provider_detail(&scrubbed, "")
}

struct SseParser<R: Read> {
    reader: R,
    buffer: Vec<u8>,
    pos: usize,
    len: usize,
    prev_was_cr: bool,
    pending_bytes: Vec<u8>,
}

impl<R: Read> SseParser<R> {
    fn new(reader: R) -> Self {
        Self {
            reader,
            buffer: vec![0u8; 8192],
            pos: 0,
            len: 0,
            prev_was_cr: false,
            pending_bytes: Vec::new(),
        }
    }

    fn next_line(&mut self) -> Result<Option<String>, EndpointTransportError> {
        loop {
            if self.pos >= self.len {
                match self.reader.read(&mut self.buffer) {
                    Ok(0) => {
                        if self.pending_bytes.is_empty() {
                            return Ok(None);
                        } else {
                            let bytes = std::mem::take(&mut self.pending_bytes);
                            let s = String::from_utf8(bytes)
                                .map_err(|_| EndpointTransportError::Other)?;
                            return Ok(Some(s));
                        }
                    }
                    Ok(n) => {
                        self.pos = 0;
                        self.len = n;
                    }
                    Err(e) => {
                        let ureq_err = ureq::Error::from(e);
                        return Err(classify_ureq_error(ureq_err));
                    }
                }
            }

            while self.pos < self.len {
                let byte = self.buffer[self.pos];
                self.pos += 1;

                if self.prev_was_cr {
                    self.prev_was_cr = false;
                    if byte == b'\n' {
                        continue;
                    }
                }

                if byte == b'\n' {
                    let bytes = std::mem::take(&mut self.pending_bytes);
                    let s = String::from_utf8(bytes).map_err(|_| EndpointTransportError::Other)?;
                    return Ok(Some(s));
                } else if byte == b'\r' {
                    self.prev_was_cr = true;
                    let bytes = std::mem::take(&mut self.pending_bytes);
                    let s = String::from_utf8(bytes).map_err(|_| EndpointTransportError::Other)?;
                    return Ok(Some(s));
                } else {
                    self.pending_bytes.push(byte);
                    if self.pending_bytes.len() > MAX_BUFFER_BYTES {
                        return Err(EndpointTransportError::Other);
                    }
                }
            }
        }
    }
}

enum TerminalEvent {
    Completed(Value),
    Incomplete(Value),
    Failed(Value),
    Error(Value),
}

struct StreamAccumulator {
    delta_events: u64,
    delta_bytes: u64,
    output_bound_limit: u64,
    output_bound_tripped: bool,
    completed_items: Vec<String>,
    current_item_text: String,
    current_item_has_deltas: bool,
}

impl StreamAccumulator {
    fn new(output_bound_limit: u64) -> Self {
        Self {
            delta_events: 0,
            delta_bytes: 0,
            output_bound_limit,
            output_bound_tripped: false,
            completed_items: Vec::new(),
            current_item_text: String::new(),
            current_item_has_deltas: false,
        }
    }

    fn process_event(&mut self, event_str: &str) -> Result<Option<TerminalEvent>, &'static str> {
        let Ok(val) = serde_json::from_str::<Value>(event_str) else {
            return Err("provider_response_invalid");
        };
        if !val.is_object() {
            return Err("provider_response_invalid");
        }
        let event_type = val.get("type").and_then(Value::as_str).unwrap_or("");
        match event_type {
            "response.output_item.added" => {
                if self.current_item_has_deltas && !self.current_item_text.is_empty() {
                    self.completed_items
                        .push(std::mem::take(&mut self.current_item_text));
                }
                self.current_item_has_deltas = false;
            }
            "response.output_text.delta" => {
                if let Some(delta) = val.get("delta").and_then(Value::as_str) {
                    self.delta_events += 1;
                    self.delta_bytes += delta.len() as u64;
                    self.current_item_text.push_str(delta);
                    self.current_item_has_deltas = true;

                    let effective_count = self.delta_events.max(self.delta_bytes.div_ceil(5));
                    if effective_count > self.output_bound_limit {
                        self.output_bound_tripped = true;
                        log::warn!("ChatGPT client stopped at the output bound");
                    }
                }
            }
            "response.output_item.done" => {
                let mut text_from_item = String::new();
                if let Some(content) = val
                    .get("item")
                    .and_then(Value::as_object)
                    .and_then(|item| item.get("content"))
                    .and_then(Value::as_array)
                {
                    for block in content {
                        if let (Some("output_text"), Some(t)) = (
                            block.get("type").and_then(Value::as_str),
                            block.get("text").and_then(Value::as_str),
                        ) {
                            text_from_item.push_str(t);
                        }
                    }
                }
                if text_from_item.is_empty() && !self.current_item_text.is_empty() {
                    text_from_item = std::mem::take(&mut self.current_item_text);
                } else {
                    self.current_item_text.clear();
                }
                self.completed_items.push(text_from_item);
                self.current_item_has_deltas = false;
            }
            "response.completed" => {
                return Ok(Some(TerminalEvent::Completed(val)));
            }
            "response.incomplete" => {
                return Ok(Some(TerminalEvent::Incomplete(val)));
            }
            "response.failed" => {
                return Ok(Some(TerminalEvent::Failed(val)));
            }
            "error" => {
                return Ok(Some(TerminalEvent::Error(val)));
            }
            _ => {}
        }
        Ok(None)
    }

    fn finish_text(mut self) -> String {
        if self.current_item_has_deltas && !self.current_item_text.is_empty() {
            self.completed_items.push(self.current_item_text);
        }
        self.completed_items.join("")
    }
}

fn parse_stream<R: Read, C: ChatGptCredential>(
    reader: R,
    request: &GenerateRequest,
    model_name: &str,
    thinking: crate::thinking::Thinking,
    credential: &C,
    current_bearer: &str,
    sent_bearers: &[String],
) -> ChatGptResult {
    let mut parser = SseParser::new(reader);
    let mut data_lines = Vec::new();
    let mut total_event_data_bytes = 0usize;

    let ceiling = shared_ceiling(request.max_output_tokens, thinking);
    let output_bound_limit = (ceiling * 5).div_ceil(4);
    let mut acc = StreamAccumulator::new(output_bound_limit);
    let mut terminal_event: Option<TerminalEvent> = None;

    loop {
        let line = match parser.next_line() {
            Ok(Some(l)) => l,
            Ok(None) => break,
            Err(EndpointTransportError::Capacity) => return failure("provider_unavailable"),
            Err(EndpointTransportError::Connection) => return failure("network_unreachable"),
            Err(EndpointTransportError::ClosedBeforeResponse | EndpointTransportError::Other) => {
                return failure("provider_response_invalid");
            }
        };

        if line.is_empty() {
            if !data_lines.is_empty() {
                let event_str = data_lines.join("\n");
                data_lines.clear();
                total_event_data_bytes = 0;
                if event_str.trim() != "[DONE]" {
                    match acc.process_event(&event_str) {
                        Ok(Some(terminal)) => {
                            terminal_event = Some(terminal);
                            break;
                        }
                        Ok(None) => {
                            if acc.output_bound_tripped {
                                break;
                            }
                        }
                        Err(code) => return failure(code),
                    }
                }
            }
            continue;
        }

        if line.starts_with(':') {
            continue;
        }

        if let Some(data) = line.strip_prefix("data:") {
            let data = data.strip_prefix(' ').unwrap_or(data);
            total_event_data_bytes += data.len();
            if total_event_data_bytes > MAX_BUFFER_BYTES {
                return failure("provider_response_invalid");
            }
            data_lines.push(data.to_string());
        }
    }

    if !data_lines.is_empty() && terminal_event.is_none() && !acc.output_bound_tripped {
        let event_str = data_lines.join("\n");
        if event_str.trim() != "[DONE]" {
            match acc.process_event(&event_str) {
                Ok(Some(terminal)) => {
                    terminal_event = Some(terminal);
                }
                Ok(None) => {}
                Err(code) => return failure(code),
            }
        }
    }

    if acc.output_bound_tripped {
        let assembled = acc.finish_text();
        let synth = json!({
            "model": model_name,
            "status": "incomplete",
            "incomplete_details": { "reason": "max_output_tokens" },
            "output": [{ "type": "message", "content": [{ "type": "output_text", "text": assembled }] }]
        });
        let scrubbed = scrub_secrets(synth.to_string(), sent_bearers);
        return match parse_response(&scrubbed, "") {
            OpenAiResult::Generated(g) => ChatGptResult::Generated(g),
            OpenAiResult::Failed(f) => ChatGptResult::Failed(ChatGptFailure {
                reason_code: f.reason_code,
                detail: f.detail,
            }),
        };
    }

    match terminal_event {
        Some(TerminalEvent::Completed(val)) => {
            // A comment next to the assembler says a parser that reads response.output on response.completed yields ""
            let assembled = acc.finish_text();
            let resp_obj = val.get("response").and_then(Value::as_object);
            let model = resp_obj
                .and_then(|r| r.get("model"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or(model_name);
            let mut synth = json!({
                "model": model,
                "status": "completed",
                "output": [{ "type": "message", "content": [{ "type": "output_text", "text": assembled }] }]
            });
            if let Some(usage) = resp_obj.and_then(|r| r.get("usage")) {
                synth["usage"] = usage.clone();
            }
            let scrubbed = scrub_secrets(synth.to_string(), sent_bearers);
            match parse_response(&scrubbed, "") {
                OpenAiResult::Generated(g) => ChatGptResult::Generated(g),
                OpenAiResult::Failed(f) => ChatGptResult::Failed(ChatGptFailure {
                    reason_code: f.reason_code,
                    detail: f.detail,
                }),
            }
        }
        Some(TerminalEvent::Incomplete(val)) => {
            let assembled = acc.finish_text();
            let resp_obj = val.get("response").and_then(Value::as_object);
            let model = resp_obj
                .and_then(|r| r.get("model"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or(model_name);
            let status = resp_obj
                .and_then(|r| r.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("incomplete");
            let mut synth = json!({
                "model": model,
                "status": status,
                "output": [{ "type": "message", "content": [{ "type": "output_text", "text": assembled }] }]
            });
            if let Some(details) = resp_obj.and_then(|r| r.get("incomplete_details")) {
                synth["incomplete_details"] = details.clone();
            }
            if let Some(usage) = resp_obj.and_then(|r| r.get("usage")) {
                synth["usage"] = usage.clone();
            }
            let scrubbed = scrub_secrets(synth.to_string(), sent_bearers);
            match parse_response(&scrubbed, "") {
                OpenAiResult::Generated(g) => ChatGptResult::Generated(g),
                OpenAiResult::Failed(f) => ChatGptResult::Failed(ChatGptFailure {
                    reason_code: f.reason_code,
                    detail: f.detail,
                }),
            }
        }
        Some(TerminalEvent::Failed(val)) | Some(TerminalEvent::Error(val)) => {
            let err_val = if let Some(resp) = val.get("response") {
                resp
            } else {
                &val
            };
            let reason_code = classify_stream_error(err_val, credential, current_bearer);
            let detail = capture_detail_scrubbed(&val.to_string(), sent_bearers);
            ChatGptResult::Failed(ChatGptFailure {
                reason_code: Some(reason_code.to_owned()),
                detail,
            })
        }
        None => failure("provider_response_invalid"),
    }
}

fn failure(reason_code: &str) -> ChatGptResult {
    ChatGptResult::Failed(ChatGptFailure {
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

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use solstone_core_generate::ContentPart;

    use super::*;
    use crate::{
        ProviderResultView, SanitizedFinishReason, ValidationFailure, assess_provider_result,
    };

    struct MockCredential {
        token: String,
        rejection_token: Result<String, CredentialError>,
        marked_rejected: AtomicBool,
        rejection_calls: AtomicUsize,
    }

    impl MockCredential {
        fn new(token: &str) -> Self {
            Self {
                token: token.to_string(),
                rejection_token: Ok(format!("{token}-refreshed")),
                marked_rejected: AtomicBool::new(false),
                rejection_calls: AtomicUsize::new(0),
            }
        }
    }

    impl ChatGptCredential for MockCredential {
        fn access_token(&self) -> Result<String, CredentialError> {
            Ok(self.token.clone())
        }

        fn access_token_after_rejection(&self, _rejected: &str) -> Result<String, CredentialError> {
            self.rejection_calls.fetch_add(1, Ordering::SeqCst);
            self.rejection_token.clone()
        }

        fn mark_token_rejected(&self, _rejected: &str) -> Result<(), CredentialError> {
            self.marked_rejected.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    struct MockTransport {
        responses: Vec<Result<ChatGptPost, EndpointTransportError>>,
        bodies: Vec<Value>,
        bearers: Vec<String>,
    }

    impl MockTransport {
        fn new(responses: Vec<Result<ChatGptPost, EndpointTransportError>>) -> Self {
            Self {
                responses,
                bodies: Vec::new(),
                bearers: Vec::new(),
            }
        }
    }

    impl ResponsesTransport for MockTransport {
        fn post(
            &mut self,
            _base_url: &str,
            _path: &str,
            body: &Value,
            bearer: &str,
            _timeout: Duration,
        ) -> Result<ChatGptPost, EndpointTransportError> {
            self.bodies.push(body.clone());
            self.bearers.push(bearer.to_string());
            if self.responses.is_empty() {
                panic!("ran out of mock responses");
            }
            self.responses.remove(0)
        }
    }

    fn sample_request() -> GenerateRequest {
        GenerateRequest {
            id: Some("req-1".to_string()),
            context: "test".to_string(),
            contents: vec![ContentPart::Text {
                text: "Hello".to_string(),
            }],
            system_instruction: Some("Be helpful".to_string()),
            temperature: 0.3,
            max_output_tokens: 100,
            timeout_s: None,
            json_output: false,
            json_schema: None,
            enforce_responsiveness: true,
            attempt_index: 0,
            exclusive_admission: false,
            transport_retries: None,
        }
    }

    fn sample_config() -> Map<String, Value> {
        let mut map = Map::new();
        map.insert(
            "providers".to_string(),
            json!({
                "active": { "provider": "chatgpt", "model": "gpt-test" },
                "chatgpt": { "model": "gpt-test" }
            }),
        );
        map
    }

    // Acceptance 1: request_body ChatGptPlan mode
    #[test]
    fn acceptance_1_request_body_chatgpt_plan() {
        let mut req = sample_request();
        req.system_instruction = Some("System instructions".to_string());
        req.json_schema = Some(
            json!({"type": "object", "properties": {"a": {"type": "string"}}, "additionalProperties": false}),
        );
        let body = request_body(
            &req,
            "gpt-test-high",
            crate::thinking::Thinking::Off,
            Some("none"),
            RequestBodyMode::ChatGptPlan,
        );
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], false);
        assert!(body.get("max_output_tokens").is_none());
        assert_eq!(body["instructions"], "System instructions");
        assert_eq!(body["reasoning"]["effort"], "none");
        assert_eq!(body["text"]["format"]["type"], "json_schema");
        assert_eq!(body["model"], "gpt-test-high");
        if let Some(input) = body.get("input").and_then(Value::as_array) {
            for item in input {
                assert_ne!(item.get("role").and_then(Value::as_str), Some("system"));
            }
        }
    }

    // Acceptance 2: assemble measured sample
    #[test]
    fn acceptance_2_assemble_measured_sample() {
        let sse = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"plan says hello\"}\n\ndata: {\"type\":\"response.output_item.done\",\"item\":{\"id\":\"msg_test\",\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"plan says hello\"}]}}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_test\",\"model\":\"gpt-test\",\"output\":[],\"usage\":{\"input_tokens\":3,\"output_tokens\":4,\"total_tokens\":7}}}\n\n";
        let req = sample_request();
        let cred = MockCredential::new("plan-token");
        let res = parse_stream(
            Cursor::new(sse.as_bytes()),
            &req,
            "gpt-test",
            crate::thinking::Thinking::Off,
            &cred,
            "plan-token",
            &["plan-token".to_string()],
        );
        match res {
            ChatGptResult::Generated(g) => {
                assert_eq!(g.text, "plan says hello");
                assert_eq!(g.model, "gpt-test");
                assert_eq!(g.finish_reason, "stop");
                assert_eq!(g.usage["input_tokens"], 3);
                assert_eq!(g.usage["output_tokens"], 4);
                assert_eq!(g.usage["total_tokens"], 7);
            }
            ChatGptResult::Failed(f) => panic!("expected Generated, got {f:?}"),
        }

        // A wrong parser that reads completed.response.output gets ""
        let val: Value = serde_json::from_str(r#"{"id":"resp_test","model":"gpt-test","output":[],"usage":{"input_tokens":3,"output_tokens":4,"total_tokens":7}}"#).unwrap();
        let wrong_text = val
            .get("output")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|i| i.get("text").and_then(Value::as_str))
                    .collect::<String>()
            })
            .unwrap_or_default();
        assert_eq!(wrong_text, "");
    }

    // Acceptance 3: completed text byte-identical, CRLF, CR, comments, split multibyte, error paths
    #[test]
    fn acceptance_3_documented_only_sse_framing_and_errors() {
        let req = sample_request();
        let cred = MockCredential::new("plan-token");

        // CRLF, CR, [DONE], : comment
        let sse_mixed = ": comment line\r\ndata: {\"type\":\"response.output_item.done\",\"item\":{\"content\":[{\"type\":\"output_text\",\"text\":\"plan says hello\"}]}}\r\rdata: [DONE]\n\ndata: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-test\"}}\n\n";
        let res = parse_stream(
            Cursor::new(sse_mixed.as_bytes()),
            &req,
            "gpt-test",
            crate::thinking::Thinking::Off,
            &cred,
            "plan-token",
            &["plan-token".to_string()],
        );
        match res {
            ChatGptResult::Generated(g) => assert_eq!(g.text, "plan says hello"),
            ChatGptResult::Failed(f) => panic!("expected Generated, got {f:?}"),
        }

        // Split multi-byte character (U+00E9, C3 A9)
        struct SplitReader {
            chunks: Vec<Vec<u8>>,
            index: usize,
        }
        impl Read for SplitReader {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.index >= self.chunks.len() {
                    return Ok(0);
                }
                let chunk = &self.chunks[self.index];
                self.index += 1;
                let n = chunk.len().min(buf.len());
                buf[..n].copy_from_slice(&chunk[..n]);
                Ok(n)
            }
        }

        let part1 = b"data: {\"type\":\"response.output_item.done\",\"item\":{\"content\":[{\"type\":\"output_text\",\"text\":\"caf\xC3".to_vec();
        let part2 = b"\xA9\"}]}}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-test\"}}\n\n".to_vec();
        let split_reader = SplitReader {
            chunks: vec![part1, part2],
            index: 0,
        };
        let res = parse_stream(
            split_reader,
            &req,
            "gpt-test",
            crate::thinking::Thinking::Off,
            &cred,
            "plan-token",
            &["plan-token".to_string()],
        );
        match res {
            ChatGptResult::Generated(g) => assert_eq!(g.text, "café"),
            ChatGptResult::Failed(f) => panic!("expected Generated, got {f:?}"),
        }

        // Non-object payload
        let non_obj_sse = "data: \"not an object\"\n\n";
        let res = parse_stream(
            Cursor::new(non_obj_sse.as_bytes()),
            &req,
            "gpt-test",
            crate::thinking::Thinking::Off,
            &cred,
            "plan-token",
            &["plan-token".to_string()],
        );
        assert_eq!(
            res,
            ChatGptResult::Failed(ChatGptFailure {
                reason_code: Some("provider_response_invalid".to_string()),
                detail: None
            })
        );

        // EOF before terminal
        let eof_sse = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n";
        let res = parse_stream(
            Cursor::new(eof_sse.as_bytes()),
            &req,
            "gpt-test",
            crate::thinking::Thinking::Off,
            &cred,
            "plan-token",
            &["plan-token".to_string()],
        );
        assert_eq!(
            res,
            ChatGptResult::Failed(ChatGptFailure {
                reason_code: Some("provider_response_invalid".to_string()),
                detail: None
            })
        );
    }

    // Acceptance 4: documented_only response.failed, error event, response.incomplete
    #[test]
    fn acceptance_4_documented_only_stream_terminals() {
        let req = sample_request();
        let cred = MockCredential::new("plan-token");

        // response.failed with usage_limit_reached
        let failed_sse = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"usage_limit_reached\"}}}\n\n";
        let res = parse_stream(
            Cursor::new(failed_sse.as_bytes()),
            &req,
            "gpt-test",
            crate::thinking::Thinking::Off,
            &cred,
            "plan-token",
            &["plan-token".to_string()],
        );
        assert_eq!(
            res,
            ChatGptResult::Failed(ChatGptFailure {
                reason_code: Some("chatgpt_usage_limit".to_string()),
                detail: Some("{\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"usage_limit_reached\"}}}".to_string())
            })
        );

        // error event with subscription_sharing_user_not_eligible
        let error_sse = "data: {\"type\":\"error\",\"error\":{\"code\":\"subscription_sharing_user_not_eligible\"}}\n\n";
        let res = parse_stream(
            Cursor::new(error_sse.as_bytes()),
            &req,
            "gpt-test",
            crate::thinking::Thinking::Off,
            &cred,
            "plan-token",
            &["plan-token".to_string()],
        );
        assert_eq!(
            res,
            ChatGptResult::Failed(ChatGptFailure {
                reason_code: Some("chatgpt_not_eligible".to_string()),
                detail: Some("{\"type\":\"error\",\"error\":{\"code\":\"subscription_sharing_user_not_eligible\"}}".to_string())
            })
        );

        // response.incomplete with running delta text
        let inc_sse = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial thought\"}\n\ndata: {\"type\":\"response.incomplete\",\"response\":{\"model\":\"gpt-test\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n";
        let res = parse_stream(
            Cursor::new(inc_sse.as_bytes()),
            &req,
            "gpt-test",
            crate::thinking::Thinking::Off,
            &cred,
            "plan-token",
            &["plan-token".to_string()],
        );
        match res {
            ChatGptResult::Generated(g) => {
                assert_eq!(g.text, "partial thought");
                assert_eq!(g.finish_reason, "max_tokens");
            }
            ChatGptResult::Failed(f) => panic!("expected Generated, got {f:?}"),
        }
    }

    // Acceptance 5: output bound trips and does not read again
    #[test]
    fn acceptance_5_output_bound_trip_read_count() {
        struct ChunkCountReader {
            chunks: Vec<String>,
            index: usize,
            reads: usize,
        }
        impl Read for ChunkCountReader {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                self.reads += 1;
                if self.index >= self.chunks.len() {
                    return Ok(0);
                }
                let chunk = self.chunks[self.index].as_bytes();
                self.index += 1;
                let n = chunk.len().min(buf.len());
                buf[..n].copy_from_slice(&chunk[..n]);
                Ok(n)
            }
        }

        let mut req = sample_request();
        req.max_output_tokens = 10;
        let ceiling = shared_ceiling(req.max_output_tokens, crate::thinking::Thinking::Off);
        let limit = (ceiling * 5).div_ceil(4);

        let mut chunks = Vec::new();
        for _ in 0..(limit + 5) {
            chunks.push(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\"}\n\n".to_string(),
            );
        }
        chunks.push(
            "data: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-test\"}}\n\n"
                .to_string(),
        );

        let mut reader = ChunkCountReader {
            chunks,
            index: 0,
            reads: 0,
        };
        let cred = MockCredential::new("plan-token");
        let res = parse_stream(
            &mut reader,
            &req,
            "gpt-test",
            crate::thinking::Thinking::Off,
            &cred,
            "plan-token",
            &["plan-token".to_string()],
        );

        match res {
            ChatGptResult::Generated(g) => {
                assert_eq!(g.finish_reason, "max_tokens");
                assert_eq!(g.model, "gpt-test");
                let view = ProviderResultView {
                    journal_path: std::path::Path::new("/unused"),
                    context: "test",
                    model: &g.model,
                    text: &g.text,
                    finish_reason: &g.finish_reason,
                    usage: &g.usage,
                    json_output: true,
                    enforce_responsiveness: false,
                    raw_response_snippet: g.raw_response_snippet.as_deref(),
                    thinking_seen: g.thinking.is_some(),
                };
                let assessment = assess_provider_result(view);
                assert_eq!(
                    assessment.failure,
                    Some(ValidationFailure::IncompleteJson {
                        finish_reason: SanitizedFinishReason::MaxTokens,
                    })
                );
            }
            ChatGptResult::Failed(f) => panic!("expected Generated, got {f:?}"),
        }

        assert!(reader.reads <= (limit as usize) + 3);
    }

    // Acceptance 6: row 0 step down on reasoning.effort 400
    #[test]
    fn acceptance_6_step_down_on_effort_400() {
        let cred = MockCredential::new("plan-token");
        let stream_bytes = "data: {\"type\":\"response.output_item.done\",\"item\":{\"content\":[{\"type\":\"output_text\",\"text\":\"hello\"}]}}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-test\"}}\n\n";
        let responses = vec![
            Ok(ChatGptPost::Body {
                status: 400,
                body: r#"{"error":{"param":"reasoning.effort","message":"unsupported"}}"#
                    .to_string(),
            }),
            Ok(ChatGptPost::Stream {
                reader: Box::new(Cursor::new(stream_bytes.as_bytes())),
            }),
        ];
        let mut transport = MockTransport::new(responses);
        let req = sample_request();
        let config = sample_config();

        let result = chatgpt_generate_with(&req, &config, &cred, &mut transport);
        match result {
            ChatGptResult::Generated(g) => {
                assert_eq!(g.text, "hello");
                assert_eq!(transport.bodies.len(), 2);
            }
            ChatGptResult::Failed(f) => panic!("expected Generated, got {f:?}"),
        }
    }

    // Acceptance 7: rows 00 to 15
    #[test]
    fn acceptance_7_row_00_through_15() {
        let cred = MockCredential::new("plan-token");

        // row 00: 400 with param reasoning.effort
        let ext = extract_error_fields(&json!({"error": {"param": "reasoning.effort"}}));
        assert_eq!(ext.param.as_deref(), Some("reasoning.effort"));

        // row 01: documented_only subscription_sharing_invalid_user
        let r1 = classify_http_error(
            400,
            r#"{"error":{"code":"subscription_sharing_invalid_user"}}"#,
            &extract_error_fields(&json!({"error":{"code":"subscription_sharing_invalid_user"}})),
            &cred,
            "plan-token",
        );
        assert_eq!(r1, "chatgpt_sign_in_required");
        assert!(cred.marked_rejected.load(Ordering::SeqCst));
        cred.marked_rejected.store(false, Ordering::SeqCst);

        // row 02: documented_only subscription_sharing_user_not_eligible
        let r2 = classify_http_error(
            400,
            "",
            &extract_error_fields(&json!({"code":"subscription_sharing_user_not_eligible"})),
            &cred,
            "plan-token",
        );
        assert_eq!(r2, "chatgpt_not_eligible");
        assert!(!cred.marked_rejected.load(Ordering::SeqCst));

        // row 03: documented_only subscription_sharing_client_not_enabled & v2
        let r3_1 = classify_http_error(
            400,
            "",
            &extract_error_fields(&json!({"code":"subscription_sharing_client_not_enabled"})),
            &cred,
            "plan-token",
        );
        assert_eq!(r3_1, "chatgpt_not_eligible");
        assert!(!cred.marked_rejected.load(Ordering::SeqCst));

        let r3_2 = classify_http_error(
            400,
            "",
            &extract_error_fields(&json!({"code":"subscription_sharing_v2_client_not_enabled"})),
            &cred,
            "plan-token",
        );
        assert_eq!(r3_2, "chatgpt_not_eligible");
        assert!(!cred.marked_rejected.load(Ordering::SeqCst));

        // row 04: documented_only subscription_sharing_unsupported_capability
        let r4 = classify_http_error(
            400,
            "",
            &extract_error_fields(&json!({"code":"subscription_sharing_unsupported_capability"})),
            &cred,
            "plan-token",
        );
        assert_eq!(r4, "chatgpt_not_eligible");
        assert!(!cred.marked_rejected.load(Ordering::SeqCst));

        // row 05: documented_only usage_limit_reached / rate_limit_exceeded & string error precedence
        let r5_1 = classify_http_error(
            400,
            "",
            &extract_error_fields(&json!({"code":"usage_limit_reached"})),
            &cred,
            "plan-token",
        );
        assert_eq!(r5_1, "chatgpt_usage_limit");

        let r5_2 = classify_http_error(
            400,
            "",
            &extract_error_fields(&json!({"code":"rate_limit_exceeded"})),
            &cred,
            "plan-token",
        );
        assert_eq!(r5_2, "chatgpt_usage_limit");

        let ext_str_err = extract_error_fields(&json!({
            "error": "usage_limit_reached",
            "code": "other"
        }));
        let r5_3 = classify_http_error(400, "", &ext_str_err, &cred, "plan-token");
        assert_eq!(r5_3, "chatgpt_usage_limit");

        // row 06: documented_only chatpass prefix (v1 and v2)
        let r6 = classify_http_error(
            400,
            "",
            &extract_error_fields(&json!({"code":"chatpass_scope_denied"})),
            &cred,
            "plan-token",
        );
        assert_eq!(r6, "provider_request_rejected");

        let r6_v2 = classify_http_error(
            400,
            "",
            &extract_error_fields(&json!({"code":"chatpass_v2_scope_not_authorized"})),
            &cred,
            "plan-token",
        );
        assert_eq!(r6_v2, "provider_request_rejected");

        // row 07: 404
        let r7 = classify_http_error(404, "", &ExtractedError::default(), &cred, "plan-token");
        assert_eq!(r7, "model_not_found");

        // row 08: 500 & server_error code
        let r8 = classify_http_error(500, "", &ExtractedError::default(), &cred, "plan-token");
        assert_eq!(r8, "provider_unavailable");

        let r8_code = classify_http_error(
            400,
            "",
            &extract_error_fields(&json!({"code":"server_error"})),
            &cred,
            "plan-token",
        );
        assert_eq!(r8_code, "provider_unavailable");

        // row 09: 429
        let r9 = classify_http_error(429, "", &ExtractedError::default(), &cred, "plan-token");
        assert_eq!(r9, "chatgpt_usage_limit");

        // row 11: context window
        let r11 = classify_http_error(
            400,
            r#"{"error":{"message":"maximum context length is 8192 tokens"}}"#,
            &ExtractedError::default(),
            &cred,
            "plan-token",
        );
        assert_eq!(r11, "context_window_exceeded");

        // row 12: 403 HTML
        let r12 = classify_http_error(
            403,
            "<html><body>cloudflare challenge</body></html>",
            &ExtractedError::default(),
            &cred,
            "plan-token",
        );
        assert_eq!(r12, "chatgpt_not_eligible");

        // row 13: 403 other
        let r13 = classify_http_error(
            403,
            r#"{"detail":"unknown_x"}"#,
            &extract_error_fields(&json!({"detail":"unknown_x"})),
            &cred,
            "plan-token",
        );
        assert_eq!(r13, "provider_request_rejected");

        // row 14: 400 other (max_output_tokens)
        let r14 = classify_http_error(
            400,
            r#"{"detail":"Unsupported parameter: max_output_tokens"}"#,
            &extract_error_fields(&json!({"detail":"Unsupported parameter: max_output_tokens"})),
            &cred,
            "plan-token",
        );
        assert_eq!(r14, "provider_request_rejected");

        // row 15: anything else
        let r15 = classify_http_error(
            200,
            "strange body",
            &ExtractedError::default(),
            &cred,
            "plan-token",
        );
        assert_eq!(r15, "provider_response_invalid");

        // Precedence twins
        // 400 with both subscription_sharing_unsupported_capability and param reasoning.effort
        let ext_twin = extract_error_fields(&json!({
            "error": {
                "code": "subscription_sharing_unsupported_capability",
                "param": "reasoning.effort"
            }
        }));
        assert_eq!(ext_twin.param.as_deref(), Some("reasoning.effort"));

        // 403 + model_not_found is model_not_found
        let r_twin2 = classify_http_error(
            403,
            "",
            &extract_error_fields(&json!({"code":"model_not_found"})),
            &cred,
            "plan-token",
        );
        assert_eq!(r_twin2, "model_not_found");

        // 400 with nested detail: { param: "reasoning.effort", code: "subscription_sharing_unsupported_capability" }
        // tested through chatgpt_generate_with steps down effort and posts a 2nd time
        let stream_bytes = "data: {\"type\":\"response.output_item.done\",\"item\":{\"content\":[{\"type\":\"output_text\",\"text\":\"stepped down\"}]}}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-test\"}}\n\n";
        let responses = vec![
            Ok(ChatGptPost::Body {
                status: 400,
                body: r#"{"detail":{"param":"reasoning.effort","code":"subscription_sharing_unsupported_capability"}}"#
                    .to_string(),
            }),
            Ok(ChatGptPost::Stream {
                reader: Box::new(Cursor::new(stream_bytes.as_bytes())),
            }),
        ];
        let mut transport = MockTransport::new(responses);
        let req = sample_request();
        let config = sample_config();
        let step_res = chatgpt_generate_with(&req, &config, &cred, &mut transport);
        match step_res {
            ChatGptResult::Generated(g) => {
                assert_eq!(g.text, "stepped down");
                assert_eq!(transport.bodies.len(), 2);
            }
            ChatGptResult::Failed(f) => panic!("expected Generated after step-down, got {f:?}"),
        }
    }

    // Acceptance 8: 401 retry and second 401 mark rejected
    #[test]
    fn acceptance_8_second_401_marks_rejected() {
        let cred = MockCredential::new("plan-token-1");
        let responses = vec![
            Ok(ChatGptPost::Body {
                status: 401,
                body: r#"{"error":"unauthorized"}"#.to_string(),
            }),
            Ok(ChatGptPost::Body {
                status: 401,
                body: r#"{"error":"unauthorized again"}"#.to_string(),
            }),
        ];
        let mut transport = MockTransport::new(responses);
        let req = sample_request();
        let config = sample_config();

        let res = chatgpt_generate_with(&req, &config, &cred, &mut transport);
        assert_eq!(
            res,
            ChatGptResult::Failed(ChatGptFailure {
                reason_code: Some("chatgpt_sign_in_required".to_string()),
                detail: Some(r#"{"error":"unauthorized again"}"#.to_string())
            })
        );
        assert_eq!(transport.bearers.len(), 2);
        assert!(cred.marked_rejected.load(Ordering::SeqCst));
    }

    // Acceptance 9: CredentialError mapping
    #[test]
    fn acceptance_9_credential_error_mapping_zero_posts() {
        let storage_err = CredentialError::Storage(std::path::PathBuf::from("/test/path"));
        let res = failure_from_credential_error(storage_err);
        match res {
            ChatGptResult::Failed(f) => {
                assert_eq!(f.reason_code, Some("pipeline_unavailable".to_string()));
                assert!(
                    f.detail
                        .as_ref()
                        .unwrap()
                        .contains("journal thinking chatgpt sign-out --forget")
                );
            }
            _ => panic!("expected failed"),
        }

        let io_err = CredentialError::Io("PermissionDenied".to_string());
        let res_io = failure_from_credential_error(io_err);
        match res_io {
            ChatGptResult::Failed(f) => {
                assert_eq!(f.reason_code, Some("pipeline_unavailable".to_string()));
                assert!(!f.detail.as_ref().unwrap().contains("sign-out"));
            }
            _ => panic!("expected failed"),
        }
    }

    // Acceptance 10: Environment keys injected, bearer recorded is credential token
    #[test]
    fn acceptance_10_bearer_uses_credential_token_ignoring_env_keys() {
        let cred = MockCredential::new("plan-token-real");
        let stream_bytes = "data: {\"type\":\"response.output_item.done\",\"item\":{\"content\":[{\"type\":\"output_text\",\"text\":\"hi\"}]}}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-test\"}}\n\n";
        let responses = vec![Ok(ChatGptPost::Stream {
            reader: Box::new(Cursor::new(stream_bytes.as_bytes())),
        })];
        let mut transport = MockTransport::new(responses);
        let req = sample_request();
        let config = sample_config();

        let env_lookup = |key: &str| match key {
            "OPENAI_API_KEY" => Some("sk-openai-key".to_string()),
            "ANTHROPIC_API_KEY" => Some("sk-anthropic-key".to_string()),
            "GOOGLE_API_KEY" => Some("sk-google-key".to_string()),
            "SOLSTONE_GENERATE_API_KEY_OVERRIDE" => Some("sk-override".to_string()),
            _ => None,
        };

        let result = chatgpt_generate_with_lookup(&req, &config, &cred, &mut transport, env_lookup);
        match result {
            ChatGptResult::Generated(_) => {
                assert_eq!(transport.bearers.len(), 1);
                assert_eq!(transport.bearers[0], "plan-token-real");
            }
            ChatGptResult::Failed(f) => panic!("expected Generated, got {f:?}"),
        }
    }

    // Acceptance 11: Usage limit failure with keys set, resolve_lane is LaneOutcome::ChatGpt
    #[test]
    fn acceptance_11_usage_limit_and_lane_outcome() {
        let cred = MockCredential::new("plan-token");
        let responses = vec![Ok(ChatGptPost::Body {
            status: 429,
            body: r#"{"error":{"code":"usage_limit_reached"}}"#.to_string(),
        })];
        let mut transport = MockTransport::new(responses);
        let req = sample_request();
        let config = sample_config();

        let env_lookup = |key: &str| match key {
            "OPENAI_API_KEY" => Some("sk-openai-key".to_string()),
            _ => None,
        };

        let result = chatgpt_generate_with_lookup(&req, &config, &cred, &mut transport, env_lookup);
        assert_eq!(
            result,
            ChatGptResult::Failed(ChatGptFailure {
                reason_code: Some("chatgpt_usage_limit".to_string()),
                detail: Some(r#"{"error":{"code":"usage_limit_reached"}}"#.to_string())
            })
        );
        assert_eq!(transport.bodies.len(), 1);

        let (_, lane) = crate::lane::resolve_lane_with(&config, env_lookup);
        assert_eq!(lane, crate::lane::LaneOutcome::ChatGpt);
    }
}
