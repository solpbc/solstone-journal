// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::time::{Duration, SystemTime};

use jiff::Timestamp;
use jiff::civil::DateTime;
use jiff::tz::Offset;
use serde_json::{Value, json};

use crate::args::parse_args_for;
use crate::command::{CommandContext, CommandOutput};
use crate::decode::decode_response;
use crate::error::{ClientError, service_down_for};
use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};
use solstone_core_cli_copy::{MessageDetail, Value as CopyValue, display_for, render_for};

const VALID_ROLES: &[&str] = &["", "phone", "observer"];
// `subscribing` ends the command, not the turn-on: the journal keeps waiting
// for the purchase on its own.
const PRIVATE_LINK_TERMINAL_PHASES: &[&str] = &[
    "enabled",
    "revoked",
    "error",
    "subscribing",
    "needs_subscription",
];

const LOCAL_NETWORK_ROUTE: &str = "/app/network/api/local-network";
const LOCAL_NETWORK_OPEN_ROUTE: &str = "/app/network/local-network/open";
const LOCAL_NETWORK_CLOSE_ROUTE: &str = "/app/network/local-network/close";

#[must_use]
pub fn authorized_clients(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let devices = match devices(ctx) {
        Ok(devices) => devices,
        Err(error) => return link_error(locale, error),
    };
    if devices.is_empty() {
        return stdout_line(render_for(
            locale,
            "journal.cli.client.network.clients.empty",
            &[],
        ));
    }
    let lines = devices
        .iter()
        .map(|device| {
            let label = display_label(device);
            display_for(
                locale,
                "journal.cli.client.network.clients.seen",
                [
                    ("fingerprint", &string_field(device, "fingerprint")),
                    ("label", &label),
                    ("when", &relative_time_field(ctx, device, "last_seen_at")),
                ],
            )
        })
        .collect::<Vec<_>>();
    stdout(lines)
}

#[must_use]
pub fn list(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let devices = match devices(ctx) {
        Ok(devices) => devices,
        Err(error) => return link_error(locale, error),
    };
    if devices.is_empty() {
        return stdout_line(render_for(
            locale,
            "journal.cli.client.network.devices.empty",
            &[],
        ));
    }
    let mut lines = Vec::new();
    append_device_section(
        ctx,
        &mut lines,
        &render_for(
            locale,
            "journal.cli.client.network.linked.systems.heading",
            &[],
        ),
        &devices,
    );
    stdout(lines)
}

#[must_use]
pub fn observer_pause(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    stdout_line(render_for(
        locale,
        "journal.cli.client.network.observer.pause",
        &[],
    ))
}

#[must_use]
pub fn pair(ctx: CommandContext<'_>) -> CommandOutput {
    let options: &[&str] = &["--device-label", "--as", "--timeout"];
    let locale = ctx.human_locale_with_options(options);
    let parsed = match parse_args_for(locale, ctx.args, options, &["--no-wait"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error, 1),
    };
    let device_label = parsed.value("--device-label").unwrap_or("");
    let role = parsed.value("--as");
    if let Some(role) = role
        && !VALID_ROLES.contains(&role)
    {
        return stderr(
            render_for(locale, "journal.cli.client.network.role.invalid", &[]),
            2,
        );
    }
    let timeout_seconds = match parsed.value("--timeout") {
        Some(value) => match value.parse::<i64>() {
            Ok(value) => value,
            Err(_error) => {
                return stderr(
                    render_for(locale, "journal.cli.client.network.timeout.integer", &[]),
                    1,
                );
            }
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
                return stderr(
                    render_for(
                        locale,
                        "journal.cli.client.network.cli.pair.no.lan.address",
                        &[],
                    ),
                    1,
                );
            }
            if error.reason_code() == Some("local_network_closed") {
                return stderr(
                    render_for(
                        locale,
                        "journal.cli.client.network.cli.pair.local.network.closed",
                        &[],
                    ),
                    1,
                );
            }
            return link_error(locale, error);
        }
    };
    let nonce = string_field_value(&response, "nonce");
    let pair_link = string_field_value(&response, "pair_link");
    let ca_fp = string_field_value(&response, "ca_fingerprint");
    let mut out = String::new();
    push_line(
        &mut out,
        display_for(
            locale,
            "journal.cli.client.network.pair.link",
            [("link", &pair_link)],
        ),
    );
    push_line(
        &mut out,
        render_for(locale, "journal.cli.client.network.cli.pair.join.hint", &[]),
    );
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
        display_for(
            locale,
            "journal.cli.client.network.pair.fingerprint",
            [("fingerprint", &ca_fp)],
        ),
    );
    if !device_label.is_empty() {
        push_line(
            &mut out,
            display_for(
                locale,
                "journal.cli.client.network.pair.device",
                [("device", &device_label)],
            ),
        );
    }
    if parsed.has_flag("--no-wait") {
        return CommandOutput::success(out);
    }
    push_line(&mut out, "");
    push_line(
        &mut out,
        render_for(locale, "journal.cli.client.network.pair.wait", &[]),
    );

    let before_response = match request_json(
        ctx,
        HttpMethod::Get,
        "/app/network/api/devices",
        vec![],
        None,
    ) {
        Ok(response) => response,
        Err(error) => return link_error_preserving_stdout(locale, out, error, 1),
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
            Err(error) => return link_error_preserving_stdout(locale, out, error, 1),
        };
        let current = devices_from_body(&devices_response);
        let new_entries = current
            .iter()
            .filter(|device| !before.contains(&string_field(device, "fingerprint")))
            .collect::<Vec<_>>();
        if let Some(entry) = new_entries.last() {
            let label = display_label(entry);
            push_line(
                &mut out,
                display_for(
                    locale,
                    "journal.cli.client.network.pair.paired",
                    [("label", &label)],
                ),
            );
            push_line(
                &mut out,
                display_for(
                    locale,
                    "journal.cli.client.network.pair.result.fingerprint",
                    [("fingerprint", &string_field(entry, "fingerprint"))],
                ),
            );
            push_line(
                &mut out,
                display_for(
                    locale,
                    "journal.cli.client.network.pair.result.date",
                    [("date", &string_field(entry, "paired_at"))],
                ),
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
            Err(error) => return link_error_preserving_stdout(locale, out, error, 1),
        };
        if nonce_status
            .get("used")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            push_line(
                &mut out,
                render_for(locale, "journal.cli.client.network.pair.complete", &[]),
            );
            return CommandOutput::success(out);
        }
    }
    push_line(
        &mut out,
        render_for(locale, "journal.cli.client.network.pair.expired", &[]),
    );
    CommandOutput {
        stdout: out,
        stderr: String::new(),
        exit: 2,
    }
}

#[must_use]
pub fn private_link_setup(ctx: CommandContext<'_>) -> CommandOutput {
    let options: &[&str] = &["--wait-seconds", "--poll-interval"];
    let locale = ctx.human_locale_with_options(options);
    let parsed = match parse_args_for(locale, ctx.args, options, &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error, 1),
    };
    let wait_seconds = match parse_float_option(locale, parsed.value("--wait-seconds"), 900.0) {
        Ok(value) => value,
        Err(error) => return stderr(error, 1),
    };
    let poll_interval = match parse_float_option(locale, parsed.value("--poll-interval"), 1.0) {
        Ok(value) => value,
        Err(error) => return stderr(error, 1),
    };
    let mut out = String::new();
    push_line(
        &mut out,
        render_for(
            locale,
            "journal.cli.client.network.private.link.setting.up",
            &[],
        ),
    );
    let response = match post_private_link(locale, ctx, "/app/network/private-link/enable") {
        Ok(response) => response,
        Err(error) => return link_error(locale, error),
    };
    if let Some(operation) = response.get("operation").and_then(Value::as_object)
        && let Some(portal_url) = operation.get("portal_url").and_then(Value::as_str)
    {
        push_line(
            &mut out,
            display_for(
                locale,
                "!journal.cli.client.network.portal.approve",
                [("url", &portal_url)],
            ),
        );
    }
    match poll_private_link_until_terminal(locale, ctx, wait_seconds, poll_interval) {
        Ok((status, phase, guidance)) => {
            private_link_terminal_output(locale, out, &status, phase, guidance)
        }
        Err(error) => link_error_preserving_stdout(locale, out, error, 1),
    }
}

#[must_use]
pub fn private_link_disable(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let response = match post_private_link(locale, ctx, "/app/network/private-link/disable") {
        Ok(response) => response,
        Err(error) => {
            if error.reason_code() == Some("service_operation_failed") {
                return stderr(
                    MessageDetail::window("journal.window.network.private.link.disable.failed", [])
                        .render(locale),
                    1,
                );
            }
            return link_error(locale, error);
        }
    };
    let state = response
        .get("status")
        .and_then(Value::as_object)
        .and_then(|status| status.get("state"))
        .and_then(Value::as_str);
    if state == Some("not_enabled") {
        // "devices connect directly again" is false while the journal is
        // closed to devices on the network.
        let closed = request_json(ctx, HttpMethod::Get, LOCAL_NETWORK_ROUTE, vec![], None)
            .ok()
            .and_then(|body| {
                body.get("listening_on")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .is_some_and(|listening| listening == "this_pc");
        return stdout_line(if closed {
            MessageDetail::window(
                "journal.window.network.private.link.disable.success.closed",
                [],
            )
            .render(locale)
        } else {
            MessageDetail::window("journal.window.network.private.link.disable.success", [])
                .render(locale)
        });
    }
    stderr(
        MessageDetail::window("journal.window.network.private.link.needs.repair", [])
            .render(locale),
        1,
    )
}

#[must_use]
pub fn private_link_status(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let status = match request_json(
        ctx,
        HttpMethod::Get,
        "/app/network/api/private-link",
        vec![],
        None,
    ) {
        Ok(status) => status,
        Err(error) => return link_error(locale, error),
    };
    stdout(render_private_link_status(locale, &status))
}

#[must_use]
pub fn local_network_status(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_args_for(locale, ctx.args, &[], &["--json"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error, 2),
    };
    match request_json(ctx, HttpMethod::Get, LOCAL_NETWORK_ROUTE, vec![], None) {
        Ok(body) => local_network_output(locale, &body, parsed.has_flag("--json"), false),
        Err(error) => link_error(locale, error),
    }
}

#[must_use]
pub fn local_network_open(ctx: CommandContext<'_>) -> CommandOutput {
    local_network_set(ctx, LOCAL_NETWORK_OPEN_ROUTE, true)
}

#[must_use]
pub fn local_network_close(ctx: CommandContext<'_>) -> CommandOutput {
    local_network_set(ctx, LOCAL_NETWORK_CLOSE_ROUTE, false)
}

fn local_network_set(ctx: CommandContext<'_>, route: &str, opening: bool) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_args_for(locale, ctx.args, &[], &["--json"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error, 2),
    };
    match request_json(ctx, HttpMethod::Post, route, vec![], Some(json!({}))) {
        Ok(body) => local_network_output(locale, &body, parsed.has_flag("--json"), opening),
        Err(ClientError::Unreachable { .. }) => stderr(service_down_for(locale), 1),
        Err(error) if error.reason_code() == Some("agents_on_network") => stderr(
            MessageDetail::window("journal.window.network.local.network.agents.on", [])
                .render(locale),
            1,
        ),
        Err(_) => stderr(
            MessageDetail::window("journal.window.network.local.network.failed", []).render(locale),
            1,
        ),
    }
}

fn local_network_output(
    locale: &str,
    body: &Value,
    json_output: bool,
    opening: bool,
) -> CommandOutput {
    if json_output {
        return stdout_line(body.to_string());
    }
    // Agents on your network holds the door open only when the owner's own
    // choice is closed; an owner who chose open stays open after turning it off.
    if body.get("listening_on").and_then(Value::as_str) == Some("local_network")
        && body.get("source").and_then(Value::as_str) == Some("agents_on_network")
    {
        return stdout(vec![
            MessageDetail::window("journal.window.network.local.network.agents.on", [])
                .render(locale),
        ]);
    }
    let mut lines = vec![match body.get("listening_on").and_then(Value::as_str) {
        Some("local_network") => {
            render_for(locale, "journal.cli.client.network.local.network.open", &[])
        }
        Some("this_pc") => render_for(
            locale,
            "journal.cli.client.network.local.network.this.pc",
            &[],
        ),
        _ => MessageDetail::window("journal.window.network.local.network.not.reachable", [])
            .render(locale),
    }];
    if opening && truthy(body.get("windows_asks")) {
        lines.push(render_for(
            locale,
            "journal.cli.client.network.local.network.windows.asks",
            &[],
        ));
    }
    stdout(lines)
}

#[must_use]
pub fn status(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let state = match request_json(
        ctx,
        HttpMethod::Get,
        "/app/network/api/status",
        vec![],
        None,
    ) {
        Ok(state) => state,
        Err(error) => return link_error(locale, error),
    };
    let private_link = match request_json(
        ctx,
        HttpMethod::Get,
        "/app/network/api/private-link",
        vec![],
        None,
    ) {
        Ok(private_link) => private_link,
        Err(error) => return link_error(locale, error),
    };
    let devices = match devices(ctx) {
        Ok(devices) => devices,
        Err(error) => return link_error(locale, error),
    };
    let mut lines = Vec::new();
    if let Some(instance_id) = state
        .get("instance_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        let mark = match crate::mark_text::mark_from_jid(instance_id) {
            Ok(mark) => mark,
            Err(_) => {
                return stderr(
                    render_for(locale, "journal.cli.client.network.mark.unavailable", &[]),
                    1,
                );
            }
        };
        let spec = mark.to_render_spec();
        let spoken = crate::mark_text::spoken_mark(&spec);
        lines.push(spoken);
        lines.push(display_for(
            locale,
            "journal.cli.client.network.journal.id",
            [("id", &instance_id)],
        ));
    } else {
        lines.push(absent_instance_id_line(ctx));
    }
    lines.push(match state.get("relay_url") {
        None | Some(Value::Null) => render_for(
            locale,
            "journal.cli.client.network.status.relay.absent",
            &[],
        ),
        Some(value) => {
            let url = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            display_for(
                locale,
                "journal.cli.client.network.status.relay",
                [("url", &url)],
            )
        }
    });
    lines.push(if truthy(state.get("enrolled")) {
        render_for(locale, "journal.cli.client.network.status.enrolled", &[])
    } else {
        render_for(locale, "journal.cli.client.network.status.unenrolled", &[])
    });
    let posture = if private_link.get("posture").and_then(Value::as_str) == Some("spl") {
        render_for(locale, "journal.cli.client.network.reach.private", &[])
    } else {
        render_for(locale, "journal.cli.client.network.reach.direct", &[])
    };
    lines.push(posture);
    lines.push(
        match private_link
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("")
        {
            "enabled" => render_for(
                locale,
                "journal.cli.client.network.status.network.enabled",
                &[],
            ),
            "not_enabled" => render_for(
                locale,
                "journal.cli.client.network.status.network.not.enabled",
                &[],
            ),
            "inconsistent" => render_for(
                locale,
                "journal.cli.client.network.status.network.inconsistent",
                &[],
            ),
            "" => render_for(
                locale,
                "journal.cli.client.network.status.network.unknown",
                &[],
            ),
            other => display_for(
                locale,
                "journal.cli.client.network.status.network.other",
                [("state", &other)],
            ),
        },
    );
    lines.push(display_for(
        locale,
        "journal.cli.client.network.devices.total",
        [("total", &devices.len())],
    ));
    lines.push(render_for(
        locale,
        "journal.cli.client.network.listen.state",
        &[],
    ));
    stdout(lines)
}

/// The status route gives no instance id both before a mark is locked in and
/// when the link state can't be read. The identity route tells the two apart.
fn absent_instance_id_line(ctx: CommandContext<'_>) -> String {
    let locale = ctx.human_locale();
    let identity = request_json(
        ctx,
        HttpMethod::Get,
        "/app/network/api/identity",
        vec![],
        None,
    );
    let no_identity_yet =
        identity.is_ok_and(|body| body.get("availability").and_then(Value::as_str) == Some("none"));
    if no_identity_yet {
        render_for(locale, "journal.cli.client.network.mark.none", &[])
    } else {
        render_for(locale, "journal.cli.client.network.mark.unavailable", &[])
    }
}

#[must_use]
pub fn unpair(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_args_for(locale, ctx.args, &[], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error, 1),
    };
    let Some(target) = parsed.positionals.first() else {
        return stderr(
            display_for(
                locale,
                "journal.cli.client.missing.argument",
                [("argument", &"TARGET")],
            ),
            1,
        );
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
        Ok(_) => stdout_line(render_for(
            locale,
            "journal.cli.client.network.unpaired",
            &[],
        )),
        Err(error) => {
            if error.reason_code() == Some("paired_device_not_found") {
                let message = if target.starts_with("sha256:") {
                    display_for(
                        locale,
                        "journal.cli.client.network.device.fingerprint.absent",
                        [("fingerprint", &target)],
                    )
                } else {
                    display_for(
                        locale,
                        "journal.cli.client.network.device.label.absent",
                        [("label", &python_repr(target))],
                    )
                };
                return CommandOutput {
                    stdout: format!("{message}\n"),
                    stderr: String::new(),
                    exit: 1,
                };
            }
            if let Some(promoted) = promote_invalid_state_detail(&error) {
                return link_error(locale, promoted);
            }
            link_error(locale, error)
        }
    }
}

#[must_use]
pub fn set_label(ctx: CommandContext<'_>) -> CommandOutput {
    let options: &[&str] = &["--label"];
    let locale = ctx.human_locale_with_options(options);
    let parsed = match parse_args_for(locale, ctx.args, options, &["--clear"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error, 1),
    };
    let Some(cid) = parsed.positionals.first() else {
        return stderr(
            display_for(
                locale,
                "journal.cli.client.missing.argument",
                [("argument", &"CID")],
            ),
            1,
        );
    };
    let has_label = parsed.value("--label").is_some();
    let has_clear = parsed.has_flag("--clear");
    if has_label == has_clear {
        return stderr(
            render_for(locale, "journal.cli.client.network.label.options", &[]),
            1,
        );
    }
    let label_value = if has_clear {
        Value::Null
    } else {
        match parsed.value("--label") {
            Some(label) if !label.trim().is_empty() => Value::String(label.to_string()),
            _ => Value::Null,
        }
    };
    let payload = json!({"label": label_value});
    let route = format!("/app/network/api/clients/{cid}/label");
    match request_json(ctx, HttpMethod::Patch, &route, vec![], Some(payload)) {
        Ok(response) => stdout_line(match response.get("owner_label").and_then(Value::as_str) {
            Some(label) => display_for(
                locale,
                "journal.cli.client.network.device.label.set",
                [("label", &label)],
            ),
            None => render_for(
                locale,
                "journal.cli.client.network.device.label.cleared",
                &[],
            ),
        }),
        Err(error) => link_error(locale, error),
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
    let locale = ctx.human_locale();
    if devices.is_empty() {
        return;
    }
    if !lines.is_empty() {
        lines.push(String::new());
    }
    lines.push(heading.to_string());
    for device in devices {
        let label = display_label(device);
        lines.push(display_for(
            locale,
            "journal.cli.client.network.devices.seen",
            [
                ("label", &label),
                ("added", &relative_time_field(ctx, device, "paired_at")),
                ("seen", &relative_time_field(ctx, device, "last_seen_at")),
                ("fingerprint", &string_field(device, "fingerprint_short")),
            ],
        ));
    }
}

fn post_private_link(
    locale: &str,
    ctx: CommandContext<'_>,
    path: &str,
) -> Result<Value, ClientError> {
    match request_json(ctx, HttpMethod::Post, path, vec![], None) {
        Ok(response) => Ok(response),
        Err(error) => {
            if let Some(promoted) = promote_invalid_state_detail(&error) {
                return Err(promoted);
            }
            if error.reason_code() == Some("service_busy") {
                return Err(ClientError::ReasonRejected {
                    status: error.status().unwrap_or(503),
                    error: error.detail().map_or_else(
                        || {
                            MessageDetail::window("journal.window.network.operation.busy", [])
                                .render(locale)
                        },
                        str::to_owned,
                    ),
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
    locale: &str,
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
                Some(MessageDetail::window("journal.window.network.window.explanation.timed.out.waiting.for.your.private", []).render(locale)),
            ));
        }
        if interval > 0.0 {
            sleep(ctx, Duration::from_secs_f64(interval));
        }
    }
}

fn private_link_terminal_output(
    locale: &str,
    mut out: String,
    status: &Value,
    phase: Option<String>,
    guidance: Option<String>,
) -> CommandOutput {
    match phase.as_deref() {
        Some("enabled") => {
            push_line(
                &mut out,
                MessageDetail::window("journal.window.network.private.link.setup.success", [])
                    .render(locale),
            );
            CommandOutput::success(out)
        }
        Some(phase @ ("subscribing" | "needs_subscription")) => {
            push_line(
                &mut out,
                if phase == "subscribing" {
                    render_for(
                        locale,
                        "!journal.cli.client.network.private.link.subscribing",
                        &[],
                    )
                } else {
                    render_for(
                        locale,
                        "!journal.cli.client.network.private.link.needs.subscription",
                        &[],
                    )
                },
            );
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
            push_line(
                &mut stderr_value,
                MessageDetail::window("journal.window.network.private.link.setup.failed", [])
                    .render(locale),
            );
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
            for line in render_private_link_status(locale, status) {
                push_line(&mut out, line);
            }
            CommandOutput::success(out)
        }
    }
}

fn render_private_link_status(locale: &str, status: &Value) -> Vec<String> {
    let posture = if status.get("posture").and_then(Value::as_str) == Some("spl") {
        render_for(locale, "journal.cli.client.network.posture.private", &[])
    } else {
        render_for(locale, "journal.cli.client.network.posture.direct", &[])
    };
    let mut lines = vec![
        posture,
        match status.get("state").and_then(Value::as_str).unwrap_or("") {
            "enabled" => render_for(
                locale,
                "journal.cli.client.network.private.state.enabled",
                &[],
            ),
            "not_enabled" => render_for(
                locale,
                "journal.cli.client.network.private.state.not.enabled",
                &[],
            ),
            "inconsistent" => render_for(
                locale,
                "journal.cli.client.network.private.state.inconsistent",
                &[],
            ),
            "" => render_for(
                locale,
                "journal.cli.client.network.private.state.unknown",
                &[],
            ),
            other => display_for(
                locale,
                "journal.cli.client.network.private.state.other",
                [("state", &other)],
            ),
        },
        if truthy(status.get("enrolled")) {
            render_for(locale, "journal.cli.client.network.private.enrolled", &[])
        } else {
            render_for(locale, "journal.cli.client.network.private.unenrolled", &[])
        },
    ];
    if status.get("state").and_then(Value::as_str) == Some("enabled")
        && let Some(relay_url) = status.get("relay_url").and_then(Value::as_str)
    {
        lines.push(display_for(
            locale,
            "journal.cli.client.network.relay.url",
            [("url", &relay_url)],
        ));
    }
    if let Some(operation) = status.get("operation").and_then(Value::as_object) {
        if let Some(phase) = operation.get("phase").and_then(Value::as_str)
            && !phase.is_empty()
        {
            lines.push(display_for(
                locale,
                "journal.cli.client.network.operation",
                [("phase", &phase)],
            ));
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

fn parse_float_option(locale: &str, value: Option<&str>, default: f64) -> Result<f64, String> {
    match value {
        Some(value) => value
            .parse::<f64>()
            .map_err(|_error| render_for(locale, "journal.cli.client.network.option.float", &[])),
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

fn link_error(locale: &str, error: ClientError) -> CommandOutput {
    match error {
        ClientError::Unreachable { .. } => stderr(service_down_for(locale), 1),
        _ => stderr(error.message_for(locale), 1),
    }
}

fn link_error_preserving_stdout(
    locale: &str,
    stdout: String,
    error: ClientError,
    exit: i32,
) -> CommandOutput {
    let stderr = match error {
        ClientError::Unreachable { .. } => format!("{}\n", service_down_for(locale)),
        _ => format!("{}\n", error.message_for(locale)),
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

fn relative_time_field(ctx: CommandContext<'_>, object: &Value, key: &str) -> String {
    let Some(value) = object.get(key).and_then(Value::as_str) else {
        return MessageDetail::window("journal.window.network.delivery.checkin.never", [])
            .render(ctx.human_locale());
    };
    if value.is_empty() {
        return MessageDetail::window("journal.window.network.delivery.checkin.never", [])
            .render(ctx.human_locale());
    }
    let Some(then) = DateTime::strptime("%Y-%m-%dT%H:%M:%SZ", value)
        .ok()
        .and_then(|wall| Offset::UTC.to_timestamp(wall).ok())
    else {
        return value.to_string();
    };
    let now = Timestamp::try_from(clock_now(ctx)).expect("the clock is within the supported range");
    let seconds = (now.as_second() - then.as_second()).max(0);
    let days = seconds / 86400;
    let (id, count) = if seconds < 60 {
        ("journal.cli.client.network.time.second", seconds)
    } else if seconds < 3600 {
        ("journal.cli.client.network.time.minute", seconds / 60)
    } else if seconds < 86400 {
        ("journal.cli.client.network.time.hour", seconds / 3600)
    } else if days < 7 {
        ("journal.cli.client.network.time.day", days)
    } else if days < 28 {
        ("journal.cli.client.network.time.week", days / 7)
    } else {
        ("journal.cli.client.network.time.month", (days / 30).max(1))
    };
    render_for(
        ctx.human_locale(),
        id,
        &[("quantity", CopyValue::Cardinal(count))],
    )
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
