// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use serde_json::{Value, json};

use crate::command::{CommandContext, CommandOutput};
use crate::decode::decode_response;
use crate::error::{ClientError, SERVICE_DOWN_MESSAGE};
use crate::json_format::json_pretty_ascii;
use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};

const AI_KEY_ENV_VARS: &[&str] = &["GOOGLE_API_KEY", "ANTHROPIC_API_KEY", "OPENAI_API_KEY"];
const PROVIDERS: &[&str] = &["anthropic", "google", "openai", "local"];
const CONFIDENTIAL_TERMINAL_PHASES: &[&str] = &[
    "not_verified",
    "needs_subscription",
    "revoked",
    "repair_needed",
    "early_access",
];
const CONFIDENTIAL_RECHECK_WAIT_SECONDS: f64 = 15.0;
const CONFIDENTIAL_RECHECK_POLL_INTERVAL_SECONDS: f64 = 5.0;
const CONFIDENTIAL_RECHECK_TIMEOUT_GUIDANCE: &str = "no new confidential attestation result was observed within the wait; run solstone call thinking confidential status.";
const CONFIDENTIAL_RECHECK_NOT_STARTED_GUIDANCE: &str =
    "refresh was not started; run solstone call thinking confidential status.";
const CONFIDENTIAL_RECHECK_POST_REFUSED_GUIDANCE: &str =
    "no accepted refresh to wait for; run solstone call thinking confidential status.";
const CONFIDENTIAL_RECHECK_READ_FAILED_GUIDANCE: &str = "refresh was accepted, but no completed result could be read; run solstone call thinking confidential status.";

#[must_use]
pub fn confidential_status(ctx: CommandContext<'_>) -> CommandOutput {
    match get_confidential_state(ctx) {
        Ok(state) => {
            let operation_phase = state
                .get("confidential_operation")
                .and_then(Value::as_object)
                .and_then(|operation| operation.get("phase"))
                .cloned()
                .unwrap_or(Value::Null);
            stdout_json_value(&json!({
                "confidential_enabled": state.get("confidential_enabled").cloned().unwrap_or(Value::Null),
                "confidential_provenance_configured": state.get("confidential_provenance_configured").cloned().unwrap_or(Value::Null),
                "confidential_operation_phase": operation_phase,
                "confidential_attestation": state.get("confidential_attestation").cloned().unwrap_or(Value::Null),
            }))
        }
        Err(error) => thinking_error(error),
    }
}

#[must_use]
pub fn confidential_enable(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &["--wait-seconds", "--poll-interval"], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error, 1),
    };
    let wait_seconds = match parse_float_option(parsed.value("--wait-seconds"), 900.0) {
        Ok(value) => value,
        Err(error) => return stderr(error, 1),
    };
    let poll_interval = match parse_float_option(parsed.value("--poll-interval"), 1.0) {
        Ok(value) => value,
        Err(error) => return stderr(error, 1),
    };
    let response = match post_confidential_action(ctx, "/app/thinking/api/confidential/enable") {
        Ok(response) => response,
        Err(error) => return thinking_error(error),
    };
    let mut out = String::new();
    maybe_echo_portal(
        &mut out,
        response.get("operation"),
        "continue in browser \u{2192}",
    );
    let (state, phase, outcome) =
        match poll_confidential_until_terminal(ctx, wait_seconds, poll_interval) {
            Ok(result) => result,
            Err(error) => return thinking_error_preserving_stdout(out, error),
        };
    render_confidential_terminal(&mut out, &state, phase.as_deref());
    if (outcome == "terminal" && phase.as_deref() == Some("not_verified"))
        || outcome == "swept_configured"
    {
        push_line(
            &mut out,
            "next: solstone call thinking confidential recheck",
        );
        return CommandOutput::success(out);
    }
    if outcome == "timeout" {
        push_line(
            &mut out,
            "operation continues server-side; solstone call thinking confidential status shows its progress.",
        );
    } else if outcome == "swept_unconfigured" {
        push_line(
            &mut out,
            "operation ended without enabling confidential processing; check solstone call thinking confidential status.",
        );
    }
    CommandOutput {
        stdout: out,
        stderr: String::new(),
        exit: 1,
    }
}

#[must_use]
pub fn confidential_recheck(ctx: CommandContext<'_>) -> CommandOutput {
    let baseline = match get_confidential_state(ctx) {
        Ok(state) => state,
        Err(error) => return thinking_error(error),
    };
    let baseline_attestation = baseline
        .get("confidential_attestation")
        .cloned()
        .unwrap_or(Value::Null);
    let baseline_observed_at = parse_attestation_observed_at(&baseline_attestation);
    let response = match post_confidential_action(ctx, "/app/thinking/api/confidential/recheck") {
        Ok(response) => response,
        Err(error) => {
            return thinking_error_with_guidance(error, CONFIDENTIAL_RECHECK_POST_REFUSED_GUIDANCE);
        }
    };
    let build_payload = |attestation: Value| {
        let mut payload = json!({
            "ok": response.get("ok").cloned().unwrap_or(Value::Null),
            "attestation": attestation,
        });
        if let Some(error) = response.get("error")
            && let Some(object) = payload.as_object_mut()
        {
            object.insert("error".to_string(), error.clone());
        }
        payload
    };

    // Only explicit ok=false means the request was accepted but no refresh was started;
    // missing/null ok preserves the existing permissive behavior and proceeds to wait.
    if response.get("ok").and_then(Value::as_bool) == Some(false) {
        let payload = build_payload(baseline_attestation);
        return stdout_json_with_guidance(&payload, CONFIDENTIAL_RECHECK_NOT_STARTED_GUIDANCE, 1);
    }

    let (state, outcome) =
        match poll_confidential_recheck_until_complete(ctx, baseline_observed_at.as_ref()) {
            Ok(result) => result,
            Err(error) => {
                return thinking_error_with_guidance(
                    error,
                    CONFIDENTIAL_RECHECK_READ_FAILED_GUIDANCE,
                );
            }
        };
    let payload = build_payload(
        state
            .get("confidential_attestation")
            .cloned()
            .unwrap_or(Value::Null),
    );
    if outcome == "completed" {
        stdout_json_value(&payload)
    } else {
        stdout_json_with_guidance(&payload, CONFIDENTIAL_RECHECK_TIMEOUT_GUIDANCE, 1)
    }
}

#[must_use]
pub fn confidential_disable(ctx: CommandContext<'_>) -> CommandOutput {
    match post_confidential_action(ctx, "/app/thinking/api/confidential/disable") {
        Ok(response) => stdout_json_value(&json!({
            "result": response.get("result").cloned().unwrap_or_else(|| json!({})),
        })),
        Err(error) => thinking_error(error),
    }
}

#[must_use]
pub fn keys_show(ctx: CommandContext<'_>) -> CommandOutput {
    match get_keys(ctx) {
        Ok(response) => stdout_json_value(&json!({
            "api_keys": response.get("api_keys").cloned().unwrap_or_else(|| json!({})),
            "env": response.get("env").cloned().unwrap_or_else(|| json!({})),
            "key_validation": response.get("key_validation").cloned().unwrap_or_else(|| json!({})),
        })),
        Err(error) => thinking_error(error),
    }
}

#[must_use]
pub fn keys_set(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error, 1),
    };
    let Some(env_var) = parsed.positionals.first() else {
        return stderr("Error: missing argument ENV_VAR", 1);
    };
    let Some(value) = parsed.positionals.get(1) else {
        return stderr("Error: missing argument VALUE", 1);
    };
    if let Err(error) = validate_env_var(env_var) {
        return stderr(error, 1);
    }
    match request_json(
        ctx,
        HttpMethod::Put,
        "/app/thinking/api/keys",
        vec![],
        Some(json!({"env_var": env_var, "value": value})),
    ) {
        Ok(response) => {
            let provider = env_provider(env_var);
            stdout_json_value(&json!({
                "env_var": env_var,
                "set": true,
                "validation": response.get("key_validation").and_then(|item| item.get(provider)).cloned().unwrap_or(Value::Null),
            }))
        }
        Err(error) => invalid_config_detail_error(error),
    }
}

#[must_use]
pub fn keys_clear(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error, 1),
    };
    let Some(env_var) = parsed.positionals.first() else {
        return stderr("Error: missing argument ENV_VAR", 1);
    };
    if let Err(error) = validate_env_var(env_var) {
        return stderr(error, 1);
    }
    match request_json(
        ctx,
        HttpMethod::Put,
        "/app/thinking/api/keys",
        vec![],
        Some(json!({"env_var": env_var, "value": ""})),
    ) {
        Ok(_response) => stdout_json_value(&json!({"env_var": env_var, "cleared": true})),
        Err(error) => thinking_error(error),
    }
}

#[must_use]
pub fn keys_validate(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &["--cache-result"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error, 1),
    };
    let method = if parsed.has_flag("--cache-result") {
        HttpMethod::Post
    } else {
        HttpMethod::Get
    };
    match request_json(ctx, method, "/app/thinking/api/validate-keys", vec![], None) {
        Ok(response) => stdout_json_value(&json!({
            "key_validation": response.get("key_validation").cloned().unwrap_or_else(|| json!({})),
        })),
        Err(error) => thinking_error(error),
    }
}

#[must_use]
pub fn providers_show(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &["--human"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error, 1),
    };
    let response = match get_providers(ctx) {
        Ok(response) => response,
        Err(error) => return thinking_error(error),
    };
    if parsed.has_flag("--human") {
        return render_providers_human(&response);
    }
    stdout_json_value(&json!({
        "providers": response.get("providers").cloned().unwrap_or_else(|| json!([])),
        "provider_status": response.get("provider_status").cloned().unwrap_or_else(|| json!({})),
        "active_lane": response.get("active_lane").cloned().unwrap_or_else(|| json!({})),
        "active": response.get("active").cloned().unwrap_or_else(|| json!({})),
        "local_override": response.get("local_override").cloned().unwrap_or_else(|| json!({})),
        "api_keys": response.get("api_keys").cloned().unwrap_or_else(|| json!({})),
        "key_validation": response.get("key_validation").cloned().unwrap_or_else(|| json!({})),
    }))
}

#[must_use]
pub fn providers_set_active(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &["--provider", "--model"], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error, 1),
    };
    let Some(provider) = parsed.value("--provider") else {
        return stderr("Error: option --provider is required.", 1);
    };
    if let Err(error) = validate_provider(provider) {
        return stderr(error, 1);
    }
    let model = parsed.value("--model");
    let lane = if provider == "local" {
        if model.is_some() {
            return stderr("--model is only valid for cloud providers.", 1);
        }
        let response = match get_providers(ctx) {
            Ok(response) => response,
            Err(error) => return thinking_error(error),
        };
        if truthy(
            response
                .get("local_override")
                .and_then(|item| item.get("enabled")),
        ) {
            "byo"
        } else {
            "local"
        }
    } else {
        "byo"
    };
    let mut body = json!({"lane": lane, "provider": provider});
    if let Some(model) = model
        && let Some(object) = body.as_object_mut()
    {
        object.insert("model".to_string(), Value::String(model.to_string()));
    }
    match request_json(
        ctx,
        HttpMethod::Post,
        "/app/thinking/api/providers",
        vec![],
        Some(body),
    ) {
        Ok(response) => stdout_json_value(
            response
                .get("active")
                .unwrap_or(&Value::Object(Default::default())),
        ),
        Err(error) => invalid_config_detail_error(error),
    }
}

#[must_use]
pub fn set_local_endpoint(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &["--url", "--model", "--credential"], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error, 1),
    };
    let Some(url) = parsed.value("--url") else {
        return stderr("Error: option --url is required.", 1);
    };
    let Some(model) = parsed.value("--model") else {
        return stderr("Error: option --model is required.", 1);
    };
    let mut body = json!({"endpoint_url": url, "served_model_id": model});
    if let Some(credential) = parsed.value("--credential")
        && let Some(object) = body.as_object_mut()
    {
        object.insert(
            "credential".to_string(),
            Value::String(credential.to_string()),
        );
    }
    match request_json(
        ctx,
        HttpMethod::Post,
        "/app/thinking/api/local/endpoint",
        vec![],
        Some(body),
    ) {
        Ok(response) => stdout_json_value(response.get("local_endpoint").unwrap_or(&response)),
        Err(error) => thinking_error(error),
    }
}

#[must_use]
pub fn clear_local_endpoint(ctx: CommandContext<'_>) -> CommandOutput {
    match request_json(
        ctx,
        HttpMethod::Delete,
        "/app/thinking/api/local/endpoint",
        vec![],
        None,
    ) {
        Ok(response) => stdout_json_value(response.get("local_endpoint").unwrap_or(&response)),
        Err(error) => thinking_error(error),
    }
}

#[must_use]
pub fn local_readiness(ctx: CommandContext<'_>) -> CommandOutput {
    local_provider_status(ctx)
}

#[must_use]
pub fn local_status(ctx: CommandContext<'_>) -> CommandOutput {
    local_provider_status(ctx)
}

#[must_use]
pub fn local_availability(ctx: CommandContext<'_>) -> CommandOutput {
    request_model_query(ctx, HttpMethod::Get, "/app/thinking/api/local/availability")
}

#[must_use]
pub fn local_bootstrap(ctx: CommandContext<'_>) -> CommandOutput {
    request_model_query(ctx, HttpMethod::Post, "/app/thinking/api/local/bootstrap")
}

#[must_use]
pub fn local_bootstrap_status(ctx: CommandContext<'_>) -> CommandOutput {
    request_model_query(
        ctx,
        HttpMethod::Get,
        "/app/thinking/api/local/bootstrap/status",
    )
}

#[must_use]
pub fn local_models(ctx: CommandContext<'_>) -> CommandOutput {
    match request_json(
        ctx,
        HttpMethod::Get,
        "/app/thinking/api/local/models",
        vec![],
        None,
    ) {
        Ok(response) => stdout_json_value(&response),
        Err(error) => thinking_error(error),
    }
}

fn local_provider_status(ctx: CommandContext<'_>) -> CommandOutput {
    match request_json(
        ctx,
        HttpMethod::Get,
        "/app/thinking/api/providers/local/status",
        vec![],
        None,
    ) {
        Ok(response) => stdout_json_value(&response),
        Err(error) => thinking_error(error),
    }
}

fn request_model_query(ctx: CommandContext<'_>, method: HttpMethod, route: &str) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &["--model"], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error, 1),
    };
    let params = parsed
        .value("--model")
        .map(|model| vec![QueryParam::single("model", model)])
        .unwrap_or_default();
    match request_json(ctx, method, route, params, None) {
        Ok(response) => stdout_json_value(&response),
        Err(error) => thinking_error(error),
    }
}

fn get_providers(ctx: CommandContext<'_>) -> Result<Value, ClientError> {
    request_json(
        ctx,
        HttpMethod::Get,
        "/app/thinking/api/providers",
        vec![],
        None,
    )
}

fn get_keys(ctx: CommandContext<'_>) -> Result<Value, ClientError> {
    request_json(ctx, HttpMethod::Get, "/app/thinking/api/keys", vec![], None)
}

fn get_confidential_state(ctx: CommandContext<'_>) -> Result<Value, ClientError> {
    let response = get_providers(ctx)?;
    Ok(response
        .get("active_lane")
        .cloned()
        .unwrap_or_else(|| json!({})))
}

fn post_confidential_action(ctx: CommandContext<'_>, route: &str) -> Result<Value, ClientError> {
    request_json(ctx, HttpMethod::Post, route, vec![], None).map_err(action_error)
}

fn action_error(error: ClientError) -> ClientError {
    if matches!(
        error.reason_code(),
        Some("invalid_operation_for_state" | "service_busy")
    ) && error.detail().is_some()
    {
        return ClientError::ReasonRejected {
            status: error.status().unwrap_or(400),
            error: error.detail().unwrap_or(error.message()).to_string(),
            reason_code: error.reason_code().map(str::to_string),
            detail: error.detail().map(str::to_string),
            payload: Box::new(Value::Null),
        };
    }
    error
}

fn parse_attestation_observed_at(attestation: &Value) -> Option<DateTime<FixedOffset>> {
    let value = attestation.get("observed_at").and_then(Value::as_str)?;
    DateTime::parse_from_rfc3339(value).ok()
}

fn attestation_observed_at_newer(
    attestation: &Value,
    baseline: Option<&DateTime<FixedOffset>>,
) -> bool {
    let Some(current) = parse_attestation_observed_at(attestation) else {
        return false;
    };
    let Some(baseline) = baseline else {
        return true;
    };
    current > *baseline
}

fn poll_confidential_recheck_until_complete(
    ctx: CommandContext<'_>,
    baseline_observed_at: Option<&DateTime<FixedOffset>>,
) -> Result<(Value, &'static str), ClientError> {
    let deadline = monotonic_seconds(ctx) + CONFIDENTIAL_RECHECK_WAIT_SECONDS;
    let interval = Duration::from_secs_f64(CONFIDENTIAL_RECHECK_POLL_INTERVAL_SECONDS);
    let mut saw_verifying = false;
    loop {
        let state = get_confidential_state(ctx)?;
        let attestation = state
            .get("confidential_attestation")
            .unwrap_or(&Value::Null);
        let attestation_state = attestation.get("state").and_then(Value::as_str);
        if attestation_state == Some("verifying") {
            saw_verifying = true;
        } else {
            let known_terminal = matches!(
                attestation_state,
                Some("off" | "inactive" | "verified" | "unreachable" | "failed" | "stale")
            );
            if (saw_verifying && known_terminal)
                || (!saw_verifying
                    && attestation_observed_at_newer(attestation, baseline_observed_at))
            {
                return Ok((state, "completed"));
            }
        }
        if monotonic_seconds(ctx) >= deadline {
            return Ok((state, "timeout"));
        }
        sleep(ctx, interval);
    }
}

fn poll_confidential_until_terminal(
    ctx: CommandContext<'_>,
    wait_seconds: f64,
    poll_interval: f64,
) -> Result<(Value, Option<String>, &'static str), ClientError> {
    let deadline = monotonic_seconds(ctx) + wait_seconds.max(0.0);
    let interval = poll_interval.max(0.0);
    loop {
        let state = get_confidential_state(ctx)?;
        let Some(operation) = state
            .get("confidential_operation")
            .and_then(Value::as_object)
        else {
            let outcome = if state
                .get("confidential_provenance_configured")
                .and_then(Value::as_bool)
                == Some(true)
            {
                "swept_configured"
            } else {
                "swept_unconfigured"
            };
            return Ok((state, None, outcome));
        };
        let phase = operation
            .get("phase")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if CONFIDENTIAL_TERMINAL_PHASES.contains(&phase.as_str()) {
            return Ok((state, Some(phase), "terminal"));
        }
        if monotonic_seconds(ctx) >= deadline {
            return Ok((state, None, "timeout"));
        }
        if interval > 0.0 {
            sleep(ctx, Duration::from_secs_f64(interval));
        }
    }
}

fn render_confidential_terminal(out: &mut String, state: &Value, phase: Option<&str>) {
    let empty = json!({});
    let operation = state.get("confidential_operation").unwrap_or(&empty);
    let attestation = state.get("confidential_attestation").unwrap_or(&empty);
    push_line(
        out,
        format!(
            "confidential_enabled: {}",
            display_value(state.get("confidential_enabled"))
        ),
    );
    push_line(
        out,
        format!(
            "confidential_provenance_configured: {}",
            display_value(state.get("confidential_provenance_configured"))
        ),
    );
    push_line(
        out,
        format!(
            "attestation_state: {}",
            display_value(attestation.get("state"))
        ),
    );
    push_line(
        out,
        format!(
            "attestation_reason: {}",
            display_value(attestation.get("reason"))
        ),
    );
    push_line(
        out,
        format!(
            "attestation_observed_at: {}",
            display_value(attestation.get("observed_at"))
        ),
    );
    push_line(
        out,
        format!(
            "attestation_expires_at: {}",
            display_value(attestation.get("expires_at"))
        ),
    );
    let operation_phase = phase
        .map(str::to_string)
        .or_else(|| {
            operation
                .get("phase")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    if !operation_phase.is_empty() {
        push_line(out, format!("operation: {operation_phase}"));
    }
    if let Some(guidance) = operation.get("guidance").and_then(Value::as_str) {
        push_line(out, guidance);
    }
    if let Some(subscribe_url) = operation.get("subscribe_url").and_then(Value::as_str) {
        push_line(out, format!("subscribe_url: {subscribe_url}"));
    }
}

fn render_providers_human(response: &Value) -> CommandOutput {
    let active = response.get("active_lane").unwrap_or(&Value::Null);
    let mut lines = vec![format!(
        "active lane: {}",
        display_value(active.get("lane"))
    )];
    if let Some(statuses) = response.get("provider_status").and_then(Value::as_object) {
        let mut keys = statuses.keys().collect::<Vec<_>>();
        keys.sort();
        for key in keys {
            let status = &statuses[key];
            let issues = status.get("issues").and_then(Value::as_array);
            let status_text = if let Some(first) = issues.and_then(|items| items.first()) {
                display_value(Some(first))
            } else if truthy(status.get("cogitate_ready")) || truthy(status.get("generate_ready")) {
                "ready".to_string()
            } else {
                "not ready".to_string()
            };
            lines.push(format!("{key}: {status_text}"));
        }
    }
    stdout(lines)
}

fn maybe_echo_portal(out: &mut String, operation: Option<&Value>, cta: &str) {
    if let Some(portal_url) = operation
        .and_then(Value::as_object)
        .and_then(|operation| operation.get("portal_url"))
        .and_then(Value::as_str)
    {
        push_line(out, format!("{cta} {portal_url}"));
    }
}

fn request_json(
    ctx: CommandContext<'_>,
    method: HttpMethod,
    path: &str,
    params: Vec<QueryParam>,
    json: Option<Value>,
) -> Result<Value, ClientError> {
    let response = ctx.transport.request(ApiRequest {
        method,
        path: path.to_string(),
        params,
        json,
        headers: vec![],
        policy: TimeoutPolicy::Api,
    })?;
    decode_response(&response)
}

#[derive(Debug, Default)]
struct ParsedArgs {
    values: Vec<(String, String)>,
    flags: Vec<String>,
    positionals: Vec<String>,
}

impl ParsedArgs {
    fn value(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .rev()
            .find(|(key, _value)| key == name)
            .map(|(_key, value)| value.as_str())
    }

    fn has_flag(&self, name: &str) -> bool {
        self.flags.iter().any(|flag| flag == name)
    }
}

fn parse_args(args: &[String], options: &[&str], flags: &[&str]) -> Result<ParsedArgs, String> {
    let mut parsed = ParsedArgs::default();
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        if let Some((name, value)) = token.split_once('=')
            && options.contains(&name)
        {
            parsed.values.push((name.to_string(), value.to_string()));
        } else if options.contains(&token.as_str()) {
            index += 1;
            let Some(value) = args.get(index) else {
                return Err(format!("Error: option {token} requires an argument."));
            };
            parsed.values.push((token.clone(), value.clone()));
        } else if flags.contains(&token.as_str()) {
            parsed.flags.push(token.clone());
        } else if token.starts_with('-') {
            return Err(format!("Error: unknown option {token}."));
        } else {
            parsed.positionals.push(token.clone());
        }
        index += 1;
    }
    Ok(parsed)
}

fn parse_float_option(value: Option<&str>, default: f64) -> Result<f64, String> {
    match value {
        Some(value) => value
            .parse::<f64>()
            .map_err(|_error| "Error: option requires a floating-point value.".to_string()),
        None => Ok(default),
    }
}

fn stdout_json_value(value: &Value) -> CommandOutput {
    CommandOutput::success(format!("{}\n", json_pretty_ascii(value)))
}

fn stdout_json_with_guidance(value: &Value, guidance: &str, exit: i32) -> CommandOutput {
    let mut stdout = stdout_json_value(value).stdout;
    push_line(&mut stdout, guidance);
    CommandOutput {
        stdout,
        stderr: String::new(),
        exit,
    }
}

fn stdout(lines: Vec<String>) -> CommandOutput {
    CommandOutput::success(format!("{}\n", lines.join("\n")))
}

fn stderr(value: impl AsRef<str>, exit: i32) -> CommandOutput {
    CommandOutput::failure(format!("{}\n", value.as_ref()), exit)
}

fn thinking_error(error: ClientError) -> CommandOutput {
    match error {
        ClientError::Unreachable { .. } => stderr(SERVICE_DOWN_MESSAGE, 1),
        _ => stderr(error.message(), 1),
    }
}

fn thinking_error_with_guidance(error: ClientError, guidance: &str) -> CommandOutput {
    let mut output = thinking_error(error);
    push_line(&mut output.stderr, guidance);
    output
}

fn thinking_error_preserving_stdout(stdout: String, error: ClientError) -> CommandOutput {
    let stderr = match error {
        ClientError::Unreachable { .. } => format!("{SERVICE_DOWN_MESSAGE}\n"),
        _ => format!("{}\n", error.message()),
    };
    CommandOutput {
        stdout,
        stderr,
        exit: 1,
    }
}

fn invalid_config_detail_error(error: ClientError) -> CommandOutput {
    if error.reason_code() == Some("invalid_config_value")
        && let Some(detail) = error.detail()
    {
        return stderr(detail, 1);
    }
    thinking_error(error)
}

fn push_line(out: &mut String, value: impl AsRef<str>) {
    out.push_str(value.as_ref());
    out.push('\n');
}

fn validate_env_var(env_var: &str) -> Result<(), String> {
    if AI_KEY_ENV_VARS.contains(&env_var) {
        Ok(())
    } else {
        Err(format!(
            "Invalid env var: {env_var}. Must be one of: {}",
            AI_KEY_ENV_VARS.join(", ")
        ))
    }
}

fn validate_provider(provider: &str) -> Result<(), String> {
    if PROVIDERS.contains(&provider) {
        Ok(())
    } else {
        Err(format!(
            "Invalid provider: {provider}. Must be one of: {}",
            PROVIDERS.join(", ")
        ))
    }
}

fn env_provider(env_var: &str) -> &str {
    match env_var {
        "GOOGLE_API_KEY" => "google",
        "ANTHROPIC_API_KEY" => "anthropic",
        "OPENAI_API_KEY" => "openai",
        _ => "",
    }
}

fn display_value(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Bool(true)) => "True".to_string(),
        Some(Value::Bool(false)) => "False".to_string(),
        Some(Value::Null) | None => "None".to_string(),
        Some(value) => value.to_string(),
    }
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_f64().is_some_and(|value| value != 0.0),
        Some(Value::String(value)) => !value.is_empty(),
        Some(Value::Array(value)) => !value.is_empty(),
        Some(Value::Object(value)) => !value.is_empty(),
        Some(Value::Null) | None => false,
    }
}

fn monotonic_seconds(ctx: CommandContext<'_>) -> f64 {
    ctx.clock
        .map(|clock| clock.monotonic())
        .unwrap_or(Duration::ZERO)
        .as_secs_f64()
}

fn sleep(ctx: CommandContext<'_>, duration: Duration) {
    if let Some(clock) = ctx.clock {
        clock.sleep(duration);
    } else {
        std::thread::sleep(duration);
    }
}
