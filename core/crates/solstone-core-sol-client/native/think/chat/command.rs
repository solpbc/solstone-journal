// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::time::Duration;

use serde_json::{Map, Value};

use crate::command::{CommandContext, CommandOutput};
use crate::decode::decode_response;
use crate::error::{ClientError, SERVICE_DOWN_MESSAGE};
use crate::seam::ChatInput;
use crate::transport::{ApiRequest, HttpMethod, TimeoutPolicy};

const QUEUED_MESSAGE: &str = "Sol is busy right now — your message is queued.";
const LIVE_PROGRESS_UNAVAILABLE_MESSAGE: &str = "Live progress was unavailable.";
const LOST_CONTACT_MESSAGE: &str =
    "sol: Lost contact with Sol before it finished — check 'journal doctor'.";
const EMPTY_ANSWER_MESSAGE: &str = "sol: Sol returned an empty answer.";
const MALFORMED_RESPONSE_MESSAGE: &str = "I couldn't read the chat response.";
const COMPOSING_MESSAGE: &str = "Composing your answer…";
const CHAT_LIVENESS_THINKING: &str = "sol is thinking…";
const HELP: &str = "usage: solstone chat [-h] [--facet FACET] [-v] [-d] [message ...]\n\nChat with your journal\n\npositional arguments:\n  message        Chat message\n\noptions:\n  -h, --help     show this help message and exit\n  --facet FACET  Facet context\n  -v, --verbose  Enable verbose output\n  -d, --debug    Enable debug logging\n";
const POLL_SECONDS: u64 = 2;
const IDLE_CEILING_SECONDS: u64 = 240;

#[must_use]
pub fn chat(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args) {
        Ok(parsed) => parsed,
        Err(error) => return argparse_error(error),
    };
    if parsed.help || parsed.message.is_empty() {
        return CommandOutput::success(HELP);
    }

    let Some(clock) = ctx.clock else {
        return stderr_with_exit("sol: native chat runtime is unavailable.", 1);
    };
    let Some(event_source) = ctx.chat_events else {
        return stderr_with_exit("sol: native chat runtime is unavailable.", 1);
    };
    let mut sse_ended = event_source.open(ctx.transport).is_err();
    let mut out = Output::default();
    let posted = match post_chat(ctx, &parsed.message, parsed.facet.as_deref()) {
        Ok(posted) => posted,
        Err(PostError::Unreachable) => return stderr_with_exit(SERVICE_DOWN_MESSAGE, 1),
        Err(PostError::Client(error)) => return stderr_with_exit(render_post_error(&error), 1),
        Err(PostError::Malformed) => {
            return stderr_with_exit(format!("sol: {MALFORMED_RESPONSE_MESSAGE}"), 1);
        }
    };
    if posted.queued {
        out.emit(QUEUED_MESSAGE);
    }

    let mut state = ChatState::new(posted.use_id);
    let mut last_event_at = clock.monotonic();
    loop {
        match event_source.next(Duration::from_secs(POLL_SECONDS), clock) {
            ChatInput::SseEvent(event) => {
                match handle_event(&mut state, &mut out, &event, parsed.verbose) {
                    EventResult::Continue => {}
                    EventResult::Observed => last_event_at = clock.monotonic(),
                    EventResult::Interrupted => return stderr_with_exit("\nInterrupted.", 1),
                    EventResult::Terminal => break,
                }
            }
            ChatInput::SseEnded => sse_ended = true,
            ChatInput::Interrupted => return stderr_with_exit("\nInterrupted.", 1),
            ChatInput::PollTick => {
                if state.terminal.is_some() {
                    break;
                }
                let idle = clock.monotonic().saturating_sub(last_event_at);
                if sse_ended || idle >= Duration::from_secs(IDLE_CEILING_SECONDS) {
                    if let Some(terminal) = session_terminal(ctx, &state.use_id) {
                        out.emit(LIVE_PROGRESS_UNAVAILABLE_MESSAGE);
                        state.terminal = Some(terminal);
                        break;
                    }
                    if idle >= Duration::from_secs(IDLE_CEILING_SECONDS) {
                        state.terminal = Some(Terminal::LostContact);
                        break;
                    }
                }
            }
        }
    }

    finish_terminal(out, state.terminal)
}

#[derive(Debug, Default)]
struct ParsedArgs {
    message: String,
    facet: Option<String>,
    verbose: bool,
    help: bool,
}

fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    let mut parsed = ParsedArgs::default();
    let mut message = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        if token == "-h" || token == "--help" {
            parsed.help = true;
        } else if token == "-v" || token == "--verbose" {
            parsed.verbose = true;
        } else if token == "-d" || token == "--debug" {
        } else if let Some(value) = token.strip_prefix("--facet=") {
            parsed.facet = Some(value.to_string());
        } else if token == "--facet" {
            index += 1;
            let Some(value) = args.get(index) else {
                return Err("argument --facet: expected one argument".to_string());
            };
            parsed.facet = Some(value.clone());
        } else if token.starts_with('-') {
            return Err(format!("unrecognized arguments: {token}"));
        } else {
            message.push(token.clone());
        }
        index += 1;
    }
    parsed.message = message.join(" ").trim().to_string();
    Ok(parsed)
}

fn argparse_error(error: String) -> CommandOutput {
    CommandOutput::failure(format!("{HELP}solstone chat: error: {error}\n"), 2)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PostedChat {
    use_id: String,
    queued: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum PostError {
    Unreachable,
    Client(ClientError),
    Malformed,
}

fn post_chat(
    ctx: CommandContext<'_>,
    message: &str,
    facet: Option<&str>,
) -> Result<PostedChat, PostError> {
    let mut payload = Map::new();
    payload.insert("message".to_string(), Value::String(message.to_string()));
    if let Some(facet) = facet.filter(|value| !value.is_empty()) {
        payload.insert("facet".to_string(), Value::String(facet.to_string()));
    }
    let response = ctx
        .transport
        .request(ApiRequest {
            method: HttpMethod::Post,
            path: "/api/chat".to_string(),
            params: vec![],
            json: Some(Value::Object(payload)),
            headers: vec![],
            policy: TimeoutPolicy::ChatPost,
        })
        .map_err(|error| match error {
            ClientError::Unreachable { .. } => PostError::Unreachable,
            other => PostError::Client(other),
        })?;
    let data = decode_response(&response).map_err(|error| match error {
        ClientError::MalformedSuccess { .. } => PostError::Malformed,
        other => PostError::Client(other),
    })?;
    let Some(object) = data.as_object() else {
        return Err(PostError::Malformed);
    };
    let use_id = string_or_empty(object.get("use_id")).trim().to_string();
    if use_id.is_empty() {
        return Err(PostError::Malformed);
    }
    Ok(PostedChat {
        use_id,
        queued: truthy(object.get("queued")),
    })
}

#[derive(Debug, Default)]
struct Output {
    stderr: String,
}

impl Output {
    fn emit(&mut self, line: &str) {
        self.stderr.push_str(line);
        self.stderr.push('\n');
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Terminal {
    Finish(String),
    Error { reason: String, provider: String },
    LostContact,
}

#[derive(Debug)]
struct ChatState {
    use_id: String,
    terminal: Option<Terminal>,
    last_progress: Option<String>,
}

impl ChatState {
    fn new(use_id: String) -> Self {
        Self {
            use_id,
            terminal: None,
            last_progress: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EventResult {
    Continue,
    Observed,
    Terminal,
    Interrupted,
}

fn handle_event(
    state: &mut ChatState,
    out: &mut Output,
    value: &Value,
    verbose: bool,
) -> EventResult {
    let Some(message) = value.as_object() else {
        return EventResult::Continue;
    };
    if event_name(message) == Some("__keyboard_interrupt__") {
        return EventResult::Interrupted;
    }

    let event_tract = str_field(message, "tract");
    let event = message.get("event").and_then(Value::as_str);

    if event_tract == Some("cortex")
        && message.get("chat_proxy").and_then(Value::as_bool) == Some(true)
        && str_field(message, "use_id") == Some(state.use_id.as_str())
    {
        if event == Some("finish") {
            state.terminal = Some(Terminal::Finish(string_or_empty(message.get("result"))));
            return EventResult::Terminal;
        }
        if event == Some("error") {
            state.terminal = Some(Terminal::Error {
                reason: string_or_empty(message.get("error")).if_empty("unknown"),
                provider: string_or_empty(message.get("provider")),
            });
            return EventResult::Terminal;
        }
        render_event_progress(state, out, message, verbose);
        return EventResult::Observed;
    }

    if event_tract != Some("chat") {
        return EventResult::Continue;
    }

    if str_field(message, "use_id") == Some(state.use_id.as_str()) {
        if event == Some("sol_message") && truthy(message.get("requested_target")) {
            render_event_progress(state, out, message, verbose);
        }
        return EventResult::Observed;
    }

    if event == Some("sol_message") && is_fold_terminal_message(message, &state.use_id) {
        state.terminal = Some(Terminal::Finish(string_or_empty(message.get("text"))));
        return EventResult::Terminal;
    }

    if event == Some("talent_finished") {
        render_event_progress(state, out, message, verbose);
        return EventResult::Observed;
    }

    EventResult::Continue
}

fn render_event_progress(
    state: &mut ChatState,
    out: &mut Output,
    message: &Map<String, Value>,
    verbose: bool,
) {
    emit_progress(state, out, render_progress(message, false));
    if verbose {
        emit_progress(state, out, render_progress(message, true));
    }
}

fn emit_progress(state: &mut ChatState, out: &mut Output, line: Option<String>) {
    let Some(line) = line else {
        return;
    };
    if state.last_progress.as_deref() == Some(line.as_str()) {
        return;
    }
    state.last_progress = Some(line.clone());
    out.emit(&line);
}

fn render_progress(message: &Map<String, Value>, verbose: bool) -> Option<String> {
    let tract = str_field(message, "tract");
    let event = event_name(message);
    if verbose {
        if tract == Some("cortex") && event == Some("start") {
            let provider = collapse_whitespace(message.get("provider")).if_empty("unknown");
            let model = collapse_whitespace(message.get("model")).if_empty("unknown");
            return Some(format!("Provider: {provider}; model: {model}"));
        }
        if tract == Some("cortex") && event == Some("thinking") {
            let summary = collapse_whitespace(message.get("summary"));
            if !summary.is_empty() {
                return Some(format!("Thinking: {}", truncate(&summary, 200)));
            }
        }
        if tract == Some("cortex") && event == Some("tool_end") {
            return Some(format!("· {} done", tool_name(message)));
        }
        return None;
    }

    if tract == Some("cortex") {
        if event == Some("start") || event == Some("thinking") {
            return Some(CHAT_LIVENESS_THINKING.to_string());
        }
        if event == Some("tool_start") {
            return Some(format!("· {}", tool_name(message)));
        }
        return None;
    }

    if tract == Some("chat") && event == Some("sol_message") {
        let target = message.get("requested_target").and_then(Value::as_str)?;
        let label = talent_label_for(target)?;
        return Some(format!(
            "{label}{}",
            task_suffix(message.get("requested_task"))
        ));
    }

    if tract == Some("chat") && event == Some("talent_finished") {
        return Some(COMPOSING_MESSAGE.to_string());
    }
    None
}

fn session_terminal(ctx: CommandContext<'_>, use_id: &str) -> Option<Terminal> {
    let response = ctx
        .transport
        .request(ApiRequest {
            method: HttpMethod::Get,
            path: "/api/chat/session".to_string(),
            params: vec![],
            json: None,
            headers: vec![],
            policy: TimeoutPolicy::ChatPost,
        })
        .ok()?;
    let data = decode_response(&response).ok()?;
    let latest = data
        .as_object()
        .and_then(|object| object.get("latest_sol_message"))
        .and_then(Value::as_object)?;
    let same_use_id = str_field(latest, "use_id") == Some(use_id);
    if !same_use_id && !is_fold_terminal_message(latest, use_id) {
        return None;
    }
    if latest
        .get("requested_target")
        .is_some_and(|value| !value.is_null())
    {
        return None;
    }
    Some(Terminal::Finish(string_or_empty(latest.get("text"))))
}

fn finish_terminal(out: Output, terminal: Option<Terminal>) -> CommandOutput {
    match terminal.unwrap_or(Terminal::LostContact) {
        Terminal::LostContact => with_stderr(out, LOST_CONTACT_MESSAGE, 1),
        Terminal::Finish(result) => {
            if result.trim().is_empty() {
                with_stderr(out, EMPTY_ANSWER_MESSAGE, 1)
            } else {
                CommandOutput {
                    stdout: format!("{result}\n"),
                    stderr: out.stderr,
                    exit: 0,
                }
            }
        }
        Terminal::Error { reason, provider } => with_stderr(
            out,
            format!("sol: {}", chat_view_message(&reason, &provider)),
            1,
        ),
    }
}

fn with_stderr(mut out: Output, line: impl AsRef<str>, exit: i32) -> CommandOutput {
    out.emit(line.as_ref());
    CommandOutput {
        stdout: String::new(),
        stderr: out.stderr,
        exit,
    }
}

fn stderr_with_exit(line: impl AsRef<str>, exit: i32) -> CommandOutput {
    CommandOutput::failure(format!("{}\n", line.as_ref()), exit)
}

fn render_post_error(error: &ClientError) -> String {
    let mut lines = vec![format!("sol: {}", error.message())];
    if let Some(detail) = error
        .detail()
        .map(str::trim)
        .filter(|detail| !detail.is_empty())
    {
        lines.push(format!("sol: {detail}"));
    }
    lines.join("\n")
}

fn str_field<'a>(message: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    message.get(key).and_then(Value::as_str)
}

fn event_name(message: &Map<String, Value>) -> Option<&str> {
    str_field(message, "event").or_else(|| str_field(message, "kind"))
}

fn origin_logical_use_id(message: &Map<String, Value>) -> &str {
    message
        .get("origin")
        .and_then(Value::as_object)
        .and_then(|origin| origin.get("logical_use_id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn is_fold_terminal_message(message: &Map<String, Value>, use_id: &str) -> bool {
    message.get("requested_target").is_none_or(Value::is_null)
        && origin_logical_use_id(message) == use_id
}

fn string_or_empty(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Null) | None => String::new(),
        Some(Value::Bool(value)) => {
            if *value {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        Some(other) => other.to_string(),
    }
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_f64().is_some_and(|number| number != 0.0),
        Some(Value::String(value)) => !value.is_empty(),
        Some(Value::Array(value)) => !value.is_empty(),
        Some(Value::Object(value)) => !value.is_empty(),
    }
}

fn collapse_whitespace(value: Option<&Value>) -> String {
    string_or_empty(value)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    text.chars().take(limit - 1).collect::<String>() + "…"
}

fn task_suffix(value: Option<&Value>) -> String {
    let collapsed = collapse_whitespace(value);
    if collapsed.is_empty() {
        String::new()
    } else {
        format!(" ({})", truncate(&collapsed, 100))
    }
}

fn tool_name(message: &Map<String, Value>) -> String {
    collapse_whitespace(message.get("tool")).if_empty("unknown")
}

fn talent_label_for(target: &str) -> Option<&'static str> {
    match target {
        "read" => Some("reading your journal…"),
        "exec" => Some("Making that change…"),
        "support" => Some("Reaching solstone support…"),
        _ => None,
    }
}

fn chat_view_message(reason: &str, provider: &str) -> String {
    let Some(template) = readiness_summary(reason) else {
        return reason.to_string();
    };
    if reason == "unknown" {
        if let Some(display) = display_name(provider) {
            return format!("something went wrong with {display}");
        }
        return template.to_string();
    }
    template.replace("{provider}", display_name(provider).unwrap_or(provider))
}

fn display_name(provider: &str) -> Option<&'static str> {
    match provider {
        "google" => Some("Gemini"),
        "openai" => Some("OpenAI"),
        "anthropic" => Some("Anthropic"),
        "local" => Some("Local"),
        _ => None,
    }
}

fn readiness_summary(reason: &str) -> Option<&'static str> {
    match reason {
        "thinking_engine_not_chosen" => Some("no thinking engine is chosen yet"),
        "provider_key_missing" => {
            Some("{provider} needs credentials before it can read your screen descriptions")
        }
        "ram_insufficient" => Some("the local model needs more memory than this machine has"),
        "gpu_unavailable" => Some("local models need GPU acceleration on this computer"),
        "gpu_probe_failed" => Some("local GPU check couldn't finish"),
        "local_artifact_proof_unavailable" => Some("local provider files could not be verified"),
        "local_model_missing" | "model_missing" | "binary_missing" => {
            Some("local model setup is not finished")
        }
        "install_busy" => Some("local model setup is already running"),
        "local_model_installing" => Some("local model setup is finishing"),
        "local_model_loading" | "local_model_not_ready" => Some("the local model is starting up"),
        "local_server_unhealthy" => Some("the local model isn't responding"),
        "local_endpoint_unreachable" => {
            Some("The inference endpoint you configured could not be reached.")
        }
        "local_endpoint_contract_failed" => {
            Some("The configured endpoint did not respond in the expected format.")
        }
        "unsupported_platform" => Some("this machine is not supported for local model setup"),
        "host_unfit" => Some("this computer doesn't meet the local model's requirements"),
        "unsupported_model" => Some("this local model is not supported"),
        "sha256_mismatch" | "archive_path_traversal" | "cuda_runtime_incomplete" => {
            Some("local model setup could not be verified")
        }
        "provider_key_invalid" => Some("your {provider} key didn't validate"),
        "model_not_found" => Some("{provider} doesn't offer this model to this key"),
        "provider_quota_exceeded" => Some("your {provider} quota is spent"),
        "provider_request_rejected" => {
            Some("the provider refused a request sol sent; this is a defect in sol")
        }
        "network_unreachable" => Some("I couldn't reach the network"),
        "provider_response_invalid" => Some(
            "{provider}'s response didn't match the expected shape — try rephrasing or asking something more specific.",
        ),
        "provider_unavailable" => Some("{provider} is having trouble right now"),
        "chat_pipeline_unavailable" => Some("the chat pipeline isn't ready yet"),
        "chat_timeout" => Some("chat took too long"),
        "local_queue_timeout" => Some("the local model was busy and couldn't start in time"),
        "local_capacity_exhausted" => {
            Some("the local model was busy and could not finish this request")
        }
        "context_window_exceeded" => Some("the conversation grew too long to finish"),
        "context_budget_exceeded" => Some("the request was too long for the local model"),
        "incomplete_json_length" | "incomplete_text_length" => {
            Some("the answer ran out of room before it finished")
        }
        "max_turns_exhausted" => Some("this took too many steps to finish"),
        "no_output" => Some("I didn't get a response"),
        "token_budget_exceeded" => Some("this run reached its resource budget before finishing"),
        "wall_clock_exceeded" => Some("this run took too long to finish"),
        "unknown" => Some("chat had trouble"),
        _ => None,
    }
}

trait IfEmpty {
    fn if_empty(self, fallback: &str) -> String;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.to_string()
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::command::CommandOutput;
    use crate::seam::{
        ExpectedHttpCall, FakeClock, ScriptedChatEventSource, ScriptedHttpTransport,
    };
    use crate::transport::{ApiRequest, HttpMethod, SseRequest, TimeoutPolicy};
    use serde_json::json;

    #[test]
    fn chat_help_matches_argparse_surface() {
        assert!(HELP.starts_with("usage: solstone chat "));
        assert!(HELP.contains("--facet FACET  Facet context"));
    }

    #[test]
    fn progress_dedupes_consecutive_lines() {
        let mut state = ChatState::new("u1".to_string());
        let mut out = Output::default();
        let event = json!({
            "tract": "cortex",
            "event": "thinking",
            "chat_proxy": true,
            "use_id": "u1"
        });
        let object = event.as_object().expect("event object");
        render_event_progress(&mut state, &mut out, object, false);
        render_event_progress(&mut state, &mut out, object, false);
        assert_eq!(out.stderr, "sol is thinking…\n");
    }

    #[test]
    fn provider_readiness_unknown_provider_matches_python_chat_view() {
        assert_eq!(
            chat_view_message("provider_unavailable", "openai"),
            "OpenAI is having trouble right now"
        );
        assert_eq!(
            chat_view_message("provider_request_rejected", "google"),
            "the provider refused a request sol sent; this is a defect in sol"
        );
        assert_eq!(chat_view_message("new_reason", "openai"), "new_reason");
        assert_eq!(
            chat_view_message("unknown", "google"),
            "something went wrong with Gemini"
        );
    }

    #[test]
    fn chat_unreachable_post_renders_service_down_message() {
        let args = vec!["service".to_string()];
        let env = BTreeMap::new();
        let clock = FakeClock::at_unix(0);
        let events = ScriptedChatEventSource::new(vec![]);
        let transport = ScriptedHttpTransport::new(vec![
            ExpectedHttpCall::Sse {
                expected: SseRequest {
                    path: "/sse/events".to_string(),
                    policy: TimeoutPolicy::SseOpen,
                },
                chunks: vec![b": open\n\n".to_vec()],
            },
            ExpectedHttpCall::Request {
                expected: ApiRequest {
                    method: HttpMethod::Post,
                    path: "/api/chat".to_string(),
                    params: vec![],
                    json: Some(json!({"message": "service"})),
                    headers: vec![],
                    policy: TimeoutPolicy::ChatPost,
                },
                result: Err(ClientError::unreachable(Some(
                    "connection refused".to_string(),
                ))),
            },
        ]);
        let output = chat(CommandContext {
            args: &args,
            env: &env,
            stdin: "",
            today: "20260723",
            transport: &transport,
            clock: Some(&clock),
            chat_events: Some(&events),
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
            link_pairing: None,
            link_serve: None,
        });

        assert_eq!(
            output,
            CommandOutput {
                stdout: String::new(),
                stderr: "sol: solstone isn't running. Start it with 'journal up' and retry.\n"
                    .to_string(),
                exit: 1,
            }
        );
        transport.assert_done();
    }
}
