// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::time::{Duration, SystemTime};

use chrono::{DateTime, NaiveDateTime, Utc};
use serde_json::{Value, json};

use crate::command::{CommandContext, CommandOutput};
use crate::decode::decode_response;
use crate::error::{ClientError, SERVICE_DOWN_MESSAGE};
use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};

const VALID_ROLES: &[&str] = &["", "phone", "observer", "peer"];
const LINKED_SYSTEMS_HEADING: &str = "Linked systems:";
const PEERS_HEADING: &str = "Peers:";
const PRIVATE_LINK_TERMINAL_PHASES: &[&str] =
    &["enabled", "revoked", "error", "needs_subscription"];
const PRIVATE_LINK_SETTING_UP: &str = "setting up your private network...";
const PRIVATE_LINK_SETUP_SUCCESS: &str =
    "your private network is on. your devices can reach home from anywhere.";
const PRIVATE_LINK_SETUP_FAILED: &str = "couldn't finish setting up your private network.";
const PRIVATE_LINK_PORTAL_CTA: &str = "continue to approve →";
const PRIVATE_LINK_NEEDS_SUBSCRIPTION: &str = "your private network needs an active subscription before it can turn on. \
your consent is saved; set one up, then enable your private network again:";
const PRIVATE_LINK_DISABLE_SUCCESS: &str =
    "your private network is off. devices connect directly again.";
const PRIVATE_LINK_DISABLE_FAILED: &str =
    "couldn't turn off your private network — it's still on. try again.";
const PRIVATE_LINK_NEEDS_REPAIR: &str = "your private network needs setting up again.";
const CLI_PAIR_LINK_LABEL: &str = "pair-link";
const CLI_PAIR_JOIN_HINT: &str = "link this device with:";
const CLI_PAIR_CA_FINGERPRINT_LABEL: &str = "CA fingerprint";
const CLI_PAIR_NO_LAN_ADDRESS: &str = "can't start pairing — your journal isn't reachable on a network address \
yet. turn on your private network to pair from anywhere, or connect this \
device to your home network.";

#[must_use]
pub fn authorized_clients(ctx: CommandContext<'_>) -> CommandOutput {
    let devices = match devices(ctx) {
        Ok(devices) => devices,
        Err(error) => return link_error(error),
    };
    if devices.is_empty() {
        return stdout_line("No authorized clients.");
    }
    let lines = devices
        .iter()
        .map(|device| {
            let label = display_label(device);
            format!(
                "{}  {}  last seen {}",
                string_field(device, "fingerprint"),
                label,
                relative_time_field(ctx, device, "last_seen_at")
            )
        })
        .collect::<Vec<_>>();
    stdout(lines)
}

#[must_use]
pub fn list(ctx: CommandContext<'_>) -> CommandOutput {
    let devices = match devices(ctx) {
        Ok(devices) => devices,
        Err(error) => return link_error(error),
    };
    if devices.is_empty() {
        return stdout_line("No devices linked yet.");
    }
    let mut linked = Vec::new();
    let mut peers = Vec::new();
    for device in devices {
        if string_field(&device, "role") == "peer" {
            peers.push(device);
        } else {
            linked.push(device);
        }
    }
    let mut lines = Vec::new();
    append_device_section(ctx, &mut lines, LINKED_SYSTEMS_HEADING, &linked);
    append_device_section(ctx, &mut lines, PEERS_HEADING, &peers);
    stdout(lines)
}

#[must_use]
pub fn observer_pause(_ctx: CommandContext<'_>) -> CommandOutput {
    stdout_line("observer-pause is not yet available.")
}

#[must_use]
pub fn pair(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &["--device-label", "--as", "--timeout"],
        &["--no-wait"],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error, 1),
    };
    let device_label = parsed.value("--device-label").unwrap_or("");
    let role = parsed.value("--as");
    if let Some(role) = role
        && !VALID_ROLES.contains(&role)
    {
        return stderr("invalid role; expected one of: phone, observer, peer", 2);
    }
    let timeout_seconds = match parsed.value("--timeout") {
        Some(value) => match value.parse::<i64>() {
            Ok(value) => value,
            Err(_error) => return stderr("Error: option --timeout requires an integer.", 1),
        },
        None => 300,
    };
    let mut payload = serde_json::Map::new();
    payload.insert(
        "device_label".to_string(),
        Value::String(device_label.to_string()),
    );
    if let Some(role) = role {
        payload.insert("role".to_string(), Value::String(role.to_string()));
    }
    let response = match request_json(
        ctx,
        HttpMethod::Post,
        "/app/network/pair-start",
        vec![],
        Some(Value::Object(payload)),
    ) {
        Ok(response) => response,
        Err(error) => {
            if error.reason_code() == Some("pairing_request_invalid") {
                return stderr(CLI_PAIR_NO_LAN_ADDRESS, 1);
            }
            return link_error(error);
        }
    };
    let nonce = string_field_value(&response, "nonce");
    let pair_link = string_field_value(&response, "pair_link");
    let ca_fp = string_field_value(&response, "ca_fingerprint");
    let mut out = String::new();
    push_line(&mut out, format!("{CLI_PAIR_LINK_LABEL}: {pair_link}"));
    push_line(&mut out, CLI_PAIR_JOIN_HINT);
    let mut join_cmd = vec![
        "solstone".to_string(),
        "link".to_string(),
        "join".to_string(),
        "--code".to_string(),
        pair_link,
    ];
    if !device_label.is_empty() {
        join_cmd.push("--label".to_string());
        join_cmd.push(device_label.to_string());
    }
    push_line(&mut out, format!("  {}", shlex_join(&join_cmd)));
    push_line(
        &mut out,
        format!("{CLI_PAIR_CA_FINGERPRINT_LABEL}: sha256:{ca_fp}"),
    );
    if !device_label.is_empty() {
        let suffix = if role == Some("peer") { " (peer)" } else { "" };
        push_line(&mut out, format!("Device: {device_label}{suffix}"));
    }
    if parsed.has_flag("--no-wait") {
        return CommandOutput::success(out);
    }
    push_line(&mut out, "");
    push_line(&mut out, "Waiting for linked system…");

    let before_response = match request_json(
        ctx,
        HttpMethod::Get,
        "/app/network/api/devices",
        vec![],
        None,
    ) {
        Ok(response) => response,
        Err(error) => return link_error_preserving_stdout(out, error, 1),
    };
    let before = devices_from_body(&before_response)
        .iter()
        .map(|device| string_field(device, "fingerprint"))
        .collect::<Vec<_>>();
    let deadline = wall_seconds(ctx) + timeout_seconds.max(0) as f64;
    while wall_seconds(ctx) < deadline {
        sleep(ctx, Duration::from_secs(1));
        let devices_response = match request_json(
            ctx,
            HttpMethod::Get,
            "/app/network/api/devices",
            vec![],
            None,
        ) {
            Ok(response) => response,
            Err(error) => return link_error_preserving_stdout(out, error, 1),
        };
        let current = devices_from_body(&devices_response);
        let new_entries = current
            .iter()
            .filter(|device| !before.contains(&string_field(device, "fingerprint")))
            .collect::<Vec<_>>();
        if let Some(entry) = new_entries.last() {
            let suffix = if string_field(entry, "role") == "peer" {
                " (peer)"
            } else {
                ""
            };
            let label = display_label(entry);
            push_line(&mut out, format!("Paired: {label}{suffix}"));
            push_line(
                &mut out,
                format!("  fingerprint: {}", string_field(entry, "fingerprint")),
            );
            push_line(
                &mut out,
                format!("  paired_at:   {}", string_field(entry, "paired_at")),
            );
            return CommandOutput::success(out);
        }
        let nonce_status = match request_json(
            ctx,
            HttpMethod::Get,
            "/app/network/api/pair/nonce-status",
            vec![QueryParam::single("nonce", &nonce)],
            None,
        ) {
            Ok(response) => response,
            Err(error) => return link_error_preserving_stdout(out, error, 1),
        };
        if nonce_status
            .get("used")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            push_line(
                &mut out,
                "Pair request completed; device should appear in `solstone call link list`.",
            );
            return CommandOutput::success(out);
        }
    }
    push_line(&mut out, "Timed out. Pair code expired.");
    CommandOutput {
        stdout: out,
        stderr: String::new(),
        exit: 2,
    }
}

#[must_use]
pub fn private_link_setup(ctx: CommandContext<'_>) -> CommandOutput {
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
    let mut out = String::new();
    push_line(&mut out, PRIVATE_LINK_SETTING_UP);
    let response = match post_private_link(ctx, "/app/network/private-link/enable") {
        Ok(response) => response,
        Err(error) => return link_error(error),
    };
    if let Some(operation) = response.get("operation").and_then(Value::as_object)
        && let Some(portal_url) = operation.get("portal_url").and_then(Value::as_str)
    {
        push_line(&mut out, format!("{PRIVATE_LINK_PORTAL_CTA} {portal_url}"));
    }
    match poll_private_link_until_terminal(ctx, wait_seconds, poll_interval) {
        Ok((status, phase, guidance)) => {
            private_link_terminal_output(out, &status, phase, guidance)
        }
        Err(error) => link_error_preserving_stdout(out, error, 1),
    }
}

#[must_use]
pub fn private_link_disable(ctx: CommandContext<'_>) -> CommandOutput {
    let response = match post_private_link(ctx, "/app/network/private-link/disable") {
        Ok(response) => response,
        Err(error) => {
            if error.reason_code() == Some("service_operation_failed") {
                return stderr(PRIVATE_LINK_DISABLE_FAILED, 1);
            }
            return link_error(error);
        }
    };
    let state = response
        .get("status")
        .and_then(Value::as_object)
        .and_then(|status| status.get("state"))
        .and_then(Value::as_str);
    if state == Some("not_enabled") {
        return stdout_line(PRIVATE_LINK_DISABLE_SUCCESS);
    }
    stderr(PRIVATE_LINK_NEEDS_REPAIR, 1)
}

#[must_use]
pub fn private_link_status(ctx: CommandContext<'_>) -> CommandOutput {
    let status = match request_json(
        ctx,
        HttpMethod::Get,
        "/app/network/api/private-link",
        vec![],
        None,
    ) {
        Ok(status) => status,
        Err(error) => return link_error(error),
    };
    stdout(render_private_link_status(&status))
}

#[must_use]
pub fn status(ctx: CommandContext<'_>) -> CommandOutput {
    let state = match request_json(
        ctx,
        HttpMethod::Get,
        "/app/network/api/status",
        vec![],
        None,
    ) {
        Ok(state) => state,
        Err(error) => return link_error(error),
    };
    let private_link = match request_json(
        ctx,
        HttpMethod::Get,
        "/app/network/api/private-link",
        vec![],
        None,
    ) {
        Ok(private_link) => private_link,
        Err(error) => return link_error(error),
    };
    let devices = match devices(ctx) {
        Ok(devices) => devices,
        Err(error) => return link_error(error),
    };
    let mut lines = Vec::new();
    if state.get("instance_id").is_none_or(Value::is_null) {
        lines.push("Instance ID:   (not provisioned — pair a device to provision)".to_string());
        lines.push("Home label:    (not provisioned)".to_string());
    } else {
        lines.push(format!(
            "Instance ID:   {}",
            display_value(state.get("instance_id"))
        ));
        lines.push(format!(
            "Home label:    {}",
            display_value(state.get("home_label"))
        ));
    }
    lines.push(format!(
        "Relay URL:     {}",
        display_value(state.get("relay_url"))
    ));
    lines.push(format!(
        "Enrolled:      {}",
        if truthy(state.get("enrolled")) {
            "yes"
        } else {
            "no"
        }
    ));
    let posture = if private_link.get("posture").and_then(Value::as_str) == Some("spl") {
        "private network"
    } else {
        "direct"
    };
    lines.push(format!("Reach posture: {posture}"));
    lines.push(format!(
        "Private network: {}",
        private_link_state_label(private_link.get("state"))
    ));
    lines.push(format!("Paired devices: {}", devices.len()));
    lines
        .push("Listen-WS state: (query convey /app/network/api/status for live state)".to_string());
    stdout(lines)
}

#[must_use]
pub fn unpair(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error, 1),
    };
    let Some(target) = parsed.positionals.first() else {
        return stderr("Error: missing argument TARGET", 1);
    };
    let payload = if target.starts_with("sha256:") {
        json!({"fingerprint": target})
    } else {
        json!({"device_label": target})
    };
    match request_json(
        ctx,
        HttpMethod::Post,
        "/app/network/unpair",
        vec![],
        Some(payload),
    ) {
        Ok(_) => stdout_line("Unpaired."),
        Err(error) => {
            if error.reason_code() == Some("paired_device_not_found") {
                let message = if target.starts_with("sha256:") {
                    format!("No paired device with fingerprint {target}")
                } else {
                    format!("No paired device with label {}", python_repr(target))
                };
                return CommandOutput {
                    stdout: format!("{message}\n"),
                    stderr: String::new(),
                    exit: 1,
                };
            }
            if let Some(promoted) = promote_invalid_state_detail(&error) {
                return link_error(promoted);
            }
            link_error(error)
        }
    }
}

fn devices(ctx: CommandContext<'_>) -> Result<Vec<Value>, ClientError> {
    let body = request_json(
        ctx,
        HttpMethod::Get,
        "/app/network/api/devices",
        vec![],
        None,
    )?;
    Ok(devices_from_body(&body))
}

fn devices_from_body(body: &Value) -> Vec<Value> {
    body.get("devices")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn append_device_section(
    ctx: CommandContext<'_>,
    lines: &mut Vec<String>,
    heading: &str,
    devices: &[Value],
) {
    if devices.is_empty() {
        return;
    }
    if !lines.is_empty() {
        lines.push(String::new());
    }
    lines.push(heading.to_string());
    for device in devices {
        let label = display_label(device);
        lines.push(format!(
            "- {} — added {} — last seen {} [{}]",
            label,
            relative_time_field(ctx, device, "paired_at"),
            relative_time_field(ctx, device, "last_seen_at"),
            string_field(device, "fingerprint_short"),
        ));
    }
}

fn post_private_link(ctx: CommandContext<'_>, path: &str) -> Result<Value, ClientError> {
    match request_json(ctx, HttpMethod::Post, path, vec![], None) {
        Ok(response) => Ok(response),
        Err(error) => {
            if let Some(promoted) = promote_invalid_state_detail(&error) {
                return Err(promoted);
            }
            if error.reason_code() == Some("service_busy") {
                return Err(ClientError::ReasonRejected {
                    status: error.status().unwrap_or(503),
                    error: error
                        .detail()
                        .unwrap_or("operation already running")
                        .to_string(),
                    reason_code: error.reason_code().map(str::to_string),
                    detail: error.detail().map(str::to_string),
                    payload: Box::new(Value::Null),
                });
            }
            Err(error)
        }
    }
}

fn promote_invalid_state_detail(error: &ClientError) -> Option<ClientError> {
    if error.reason_code() != Some("invalid_operation_for_state") || error.detail().is_none() {
        return None;
    }
    Some(ClientError::ReasonRejected {
        status: error.status().unwrap_or(400),
        error: error.detail().unwrap_or(error.message()).to_string(),
        reason_code: error.reason_code().map(str::to_string),
        detail: error.detail().map(str::to_string),
        payload: Box::new(Value::Null),
    })
}

fn poll_private_link_until_terminal(
    ctx: CommandContext<'_>,
    wait_seconds: f64,
    poll_interval: f64,
) -> Result<(Value, Option<String>, Option<String>), ClientError> {
    let deadline = monotonic_seconds(ctx) + wait_seconds.max(0.0);
    let interval = poll_interval.max(0.0);
    loop {
        let status = request_json(
            ctx,
            HttpMethod::Get,
            "/app/network/api/private-link",
            vec![],
            None,
        )?;
        let operation = status.get("operation").and_then(Value::as_object);
        let Some(operation) = operation else {
            return Ok((status, None, None));
        };
        let phase = operation
            .get("phase")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if PRIVATE_LINK_TERMINAL_PHASES.contains(&phase.as_str()) {
            let guidance = operation
                .get("guidance")
                .and_then(Value::as_str)
                .map(str::to_string);
            return Ok((status, Some(phase), guidance));
        }
        if monotonic_seconds(ctx) >= deadline {
            return Ok((
                status,
                Some("timeout".to_string()),
                Some("timed out waiting for your private network.".to_string()),
            ));
        }
        if interval > 0.0 {
            sleep(ctx, Duration::from_secs_f64(interval));
        }
    }
}

fn private_link_terminal_output(
    mut out: String,
    status: &Value,
    phase: Option<String>,
    guidance: Option<String>,
) -> CommandOutput {
    match phase.as_deref() {
        Some("enabled") => {
            push_line(&mut out, PRIVATE_LINK_SETUP_SUCCESS);
            CommandOutput::success(out)
        }
        Some("needs_subscription") => {
            push_line(&mut out, PRIVATE_LINK_NEEDS_SUBSCRIPTION);
            if let Some(url) = status
                .get("operation")
                .and_then(Value::as_object)
                .and_then(|operation| operation.get("subscribe_url"))
                .and_then(Value::as_str)
            {
                push_line(&mut out, url);
            }
            CommandOutput::success(out)
        }
        Some("revoked" | "error" | "timeout") => {
            let mut stderr_value = String::new();
            push_line(&mut stderr_value, PRIVATE_LINK_SETUP_FAILED);
            if let Some(guidance) = guidance {
                push_line(&mut stderr_value, guidance);
            }
            CommandOutput {
                stdout: out,
                stderr: stderr_value,
                exit: 1,
            }
        }
        _ => {
            for line in render_private_link_status(status) {
                push_line(&mut out, line);
            }
            CommandOutput::success(out)
        }
    }
}

fn render_private_link_status(status: &Value) -> Vec<String> {
    let posture = if status.get("posture").and_then(Value::as_str) == Some("spl") {
        "private network"
    } else {
        "direct"
    };
    let mut lines = vec![
        format!("posture: {posture}"),
        format!("state: {}", private_link_state_label(status.get("state"))),
        format!(
            "enrolled: {}",
            if truthy(status.get("enrolled")) {
                "yes"
            } else {
                "no"
            }
        ),
    ];
    if status.get("state").and_then(Value::as_str) == Some("enabled")
        && let Some(relay_url) = status.get("relay_url").and_then(Value::as_str)
    {
        lines.push(format!("relay URL: {relay_url}"));
    }
    if let Some(operation) = status.get("operation").and_then(Value::as_object) {
        if let Some(phase) = operation.get("phase").and_then(Value::as_str)
            && !phase.is_empty()
        {
            lines.push(format!("operation: {phase}"));
        }
        if let Some(guidance) = operation.get("guidance").and_then(Value::as_str)
            && !guidance.is_empty()
        {
            lines.push(guidance.to_string());
        }
    }
    lines
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

fn stdout(lines: Vec<String>) -> CommandOutput {
    CommandOutput::success(format!("{}\n", lines.join("\n")))
}

fn stdout_line(value: impl AsRef<str>) -> CommandOutput {
    CommandOutput::success(format!("{}\n", value.as_ref()))
}

fn stderr(value: impl AsRef<str>, exit: i32) -> CommandOutput {
    CommandOutput::failure(format!("{}\n", value.as_ref()), exit)
}

fn link_error(error: ClientError) -> CommandOutput {
    match error {
        ClientError::Unreachable { .. } => stderr(SERVICE_DOWN_MESSAGE, 1),
        _ => stderr(error.message(), 1),
    }
}

fn link_error_preserving_stdout(stdout: String, error: ClientError, exit: i32) -> CommandOutput {
    let stderr = match error {
        ClientError::Unreachable { .. } => format!("{SERVICE_DOWN_MESSAGE}\n"),
        _ => format!("{}\n", error.message()),
    };
    CommandOutput {
        stdout,
        stderr,
        exit,
    }
}

fn push_line(out: &mut String, value: impl AsRef<str>) {
    out.push_str(value.as_ref());
    out.push('\n');
}

fn string_field(object: &Value, key: &str) -> String {
    object
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn string_field_value(object: &Value, key: &str) -> String {
    object
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn display_label(device: &Value) -> String {
    let display = string_field(device, "display_label");
    if display.is_empty() {
        string_field(device, "device_label")
    } else {
        display
    }
}

fn display_value(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Null) | None => "None".to_string(),
        Some(value) => value.to_string(),
    }
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(value)) => *value,
        Some(Value::Null) | None => false,
        Some(Value::Number(number)) => number.as_i64().is_some_and(|value| value != 0),
        Some(Value::String(value)) => !value.is_empty(),
        Some(Value::Array(value)) => !value.is_empty(),
        Some(Value::Object(value)) => !value.is_empty(),
    }
}

fn private_link_state_label(value: Option<&Value>) -> String {
    match value.and_then(Value::as_str).unwrap_or("") {
        "enabled" => "enabled".to_string(),
        "not_enabled" => "not enabled".to_string(),
        "inconsistent" => "needs repair".to_string(),
        "" => "unknown".to_string(),
        other => other.to_string(),
    }
}

fn relative_time_field(ctx: CommandContext<'_>, object: &Value, key: &str) -> String {
    let Some(value) = object.get(key).and_then(Value::as_str) else {
        return "never".to_string();
    };
    if value.is_empty() {
        return "never".to_string();
    }
    let Ok(naive) = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%SZ") else {
        return value.to_string();
    };
    let then = DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc);
    let now = DateTime::<Utc>::from(clock_now(ctx));
    let seconds = (now.timestamp() - then.timestamp()).max(0);
    format!("{} ago", relative_time(seconds))
}

fn relative_time(seconds: i64) -> String {
    if seconds < 60 {
        return plural(seconds, "second");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return plural(minutes, "minute");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return plural(hours, "hour");
    }
    let days = hours / 24;
    if days < 7 {
        return plural(days, "day");
    }
    if days < 28 {
        return plural(days / 7, "week");
    }
    if days < 60 {
        return "1 month".to_string();
    }
    plural(days / 30, "month")
}

fn plural(value: i64, unit: &str) -> String {
    if value == 1 {
        format!("1 {unit}")
    } else {
        format!("{value} {unit}s")
    }
}

fn clock_now(ctx: CommandContext<'_>) -> SystemTime {
    ctx.clock.map_or_else(SystemTime::now, |clock| clock.now())
}

fn wall_seconds(ctx: CommandContext<'_>) -> f64 {
    clock_now(ctx)
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs_f64()
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

fn shlex_join(values: &[String]) -> String {
    values
        .iter()
        .map(|value| shell_quote(value))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'@' | b'%' | b'+' | b'=' | b':' | b',' | b'.' | b'/' | b'-'))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn python_repr(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}
