// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;

use crate::command::{CommandContext, CommandOutput};
use crate::decode::decode_response;
use crate::error::{ClientError, SERVICE_DOWN_MESSAGE};
use crate::json_format::json_compact_ascii;
use crate::pagination::paginate_collection;
use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};

const ENTITY_NOT_FOUND: &str = "entity_not_found";
/// Follows what the owner said, by their recognized voice. Without it an item
/// has no voice evidence, which is not the same as being someone else's.
const SAID_BY_YOU: &str = "said by you";

#[must_use]
pub fn brief(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &["--json"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(name) = parsed.positionals.first() else {
        return missing_argument("call profile brief", "NAME");
    };
    let profile = match request_json(
        ctx,
        &format!("/api/profile/{}/brief", quote_path(name)),
        vec![],
    ) {
        Ok(profile) => profile,
        Err(error) => return profile_error(error, name),
    };
    if parsed.has_flag("--json") {
        return stdout_compact_json(&profile);
    }
    stdout(vec![
        format!("entity_id: {}", field(&profile, "entity_id")),
        format!("name: {}", field(&profile, "name")),
        format!("type: {}", field(&profile, "type")),
        format!("description: {}", field(&profile, "description")),
        format!("last_seen: {}", field(&profile, "last_seen")),
        format!("open_loop_count: {}", field(&profile, "open_loop_count")),
        format!(
            "decisions_count_30d: {}",
            field(&profile, "decisions_count_30d")
        ),
    ])
}

#[must_use]
pub fn cadence(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &["--include-mentions", "--json"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(name) = parsed.positionals.first() else {
        return missing_argument("call profile cadence", "NAME");
    };
    let mut params = Vec::new();
    if parsed.has_flag("--include-mentions") {
        params.push(QueryParam::single("include_mentions", "true"));
    }
    let cadence = match request_json(
        ctx,
        &format!("/api/profile/{}/cadence", quote_path(name)),
        params,
    ) {
        Ok(cadence) => cadence,
        Err(error) => return profile_error(error, name),
    };
    if parsed.has_flag("--json") {
        return stdout_compact_json(&cadence);
    }
    stdout(vec![
        format!(
            "recent_interactions_count_30d: {}",
            field(&cadence, "recent_interactions_count_30d")
        ),
        format!("last_seen: {}", field(&cadence, "last_seen")),
        format!(
            "avg_interval_days: {}",
            field(&cadence, "avg_interval_days")
        ),
        format!("gone_quiet_since: {}", field(&cadence, "gone_quiet_since")),
    ])
}

#[must_use]
pub fn full(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &["--facets"], &["--include-mentions", "--json"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(name) = parsed.positionals.first() else {
        return missing_argument("call profile full", "NAME");
    };
    let mut params = Vec::new();
    if let Some(facets) = parsed.value("--facets") {
        params.push(QueryParam::single("facets", facets));
    }
    if parsed.has_flag("--include-mentions") {
        params.push(QueryParam::single("include_mentions", "true"));
    }
    let profile = match request_json(ctx, &format!("/api/profile/{}", quote_path(name)), params) {
        Ok(profile) => profile,
        Err(error) => return profile_error(error, name),
    };
    if parsed.has_flag("--json") {
        return stdout_compact_json(&profile);
    }
    stdout(render_full(&profile))
}

#[must_use]
pub fn list_active(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &["--window-days"], &["--json"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let window_days = parsed.value("--window-days").unwrap_or("30");
    let ids = match paginate_collection(
        ctx.transport,
        "/api/profiles/active",
        vec![QueryParam::single("window_days", window_days)],
        None,
    ) {
        Ok(ids) => ids,
        Err(error) => return profile_list_error(error),
    };
    if parsed.has_flag("--json") {
        return stdout_compact_json(&Value::Array(ids));
    }
    let lines = ids
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        CommandOutput::success(String::new())
    } else {
        stdout(lines)
    }
}

#[must_use]
pub fn item(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(item_id) = parsed.positionals.first() else {
        return missing_argument("call profile item", "ITEM_ID");
    };
    let item = match request_json(ctx, &format!("/api/ledger/{}", quote_path(item_id)), vec![]) {
        Ok(item) => item,
        Err(error) => return client_error_output(error),
    };
    stdout(vec![format!(
        "{} is {}.",
        field(&item, "id"),
        with_said_by_you(&item, field(&item, "state"))
    )])
}

#[must_use]
pub fn close(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &["--note"], &["--dropped", "--dry-run"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(item_id) = parsed.positionals.first() else {
        return missing_argument("call profile close", "ITEM_ID");
    };
    let requested_state = if parsed.has_flag("--dropped") {
        "dropped"
    } else {
        "closed"
    };

    if parsed.has_flag("--dry-run") {
        let item = match request_json(ctx, &format!("/api/ledger/{}", quote_path(item_id)), vec![])
        {
            Ok(item) => item,
            Err(error) => return client_error_output(error),
        };
        let current_state = field(&item, "state");
        return stdout(vec![format!(
            "{item_id} is {current_state}. a close would mark it {requested_state}."
        )]);
    }

    let Some(note) = parsed.value("--note") else {
        return stderr("a note is required. add --note \"...\".");
    };
    if note.trim().is_empty() {
        return stderr("a note is required. add --note \"...\".");
    }

    let response = match ctx.transport.request(ApiRequest {
        method: HttpMethod::Post,
        path: format!("/api/ledger/{}/close", quote_path(item_id)),
        params: vec![],
        json: Some(serde_json::json!({
            "note": note,
            "as_state": requested_state,
        })),
        headers: vec![],
        policy: TimeoutPolicy::Api,
    }) {
        Ok(response) => response,
        Err(error) => return client_error_output(error),
    };

    let decoded = match decode_response(&response) {
        Ok(decoded) => decoded,
        Err(error) => return client_error_output(error),
    };

    let confirmed = decoded
        .get("confirmed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let item = &decoded["item"];
    let id = field(item, "id");
    let state = field(item, "state");
    if confirmed {
        stdout(vec![format!("{id} is {state}.")])
    } else {
        stderr(format!(
            "saved {id} as {requested_state}, but it is still {state}."
        ))
    }
}

#[derive(Debug, Default)]
struct ParsedArgs {
    positionals: Vec<String>,
    values: Vec<(String, String)>,
    flags: Vec<String>,
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

fn request_json(
    ctx: CommandContext<'_>,
    path: &str,
    params: Vec<QueryParam>,
) -> Result<Value, ClientError> {
    let response = ctx.transport.request(ApiRequest {
        method: HttpMethod::Get,
        path: path.to_string(),
        params,
        json: None,
        headers: vec![],
        policy: TimeoutPolicy::Api,
    })?;
    decode_response(&response)
}

fn render_full(profile: &Value) -> Vec<String> {
    let facets_label = profile
        .get("facets")
        .and_then(Value::as_array)
        .map(|items| {
            if items.is_empty() {
                "-".to_string()
            } else {
                items
                    .iter()
                    .map(display_value)
                    .collect::<Vec<_>>()
                    .join(",")
            }
        })
        .unwrap_or_else(|| "-".to_string());
    let cadence = &profile["cadence"];
    // `blocked` and `detached_facets` are reported by the API rather than filtered
    // there (settled 2026-09-03), so this renderer -- a caller -- is where
    // they have to become visible. Dropping them here would expose the status on
    // the wire and hide it from every agent reading the default output.
    let detached_label = profile
        .get("detached_facets")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(display_value)
                .collect::<Vec<_>>()
                .join(",")
        })
        .filter(|label| !label.is_empty());
    let mut lines = vec![
        format!(
            "{} \u{00b7} {} \u{00b7} facets={} \u{00b7} self={} \u{00b7} blocked={}",
            field(profile, "name"),
            field(profile, "type"),
            facets_label,
            field(profile, "is_self"),
            field(profile, "blocked")
        ),
        String::new(),
        "Cadence:".to_string(),
        format!("  last_seen: {}", field(cadence, "last_seen")),
        format!(
            "  recent_interactions_count_30d: {}",
            field(cadence, "recent_interactions_count_30d")
        ),
        format!(
            "  avg_interval_days: {}",
            field(cadence, "avg_interval_days")
        ),
        format!("  gone_quiet_since: {}", field(cadence, "gone_quiet_since")),
        String::new(),
        "Open loops".to_string(),
    ];
    if let Some(detached_label) = detached_label {
        lines.insert(1, format!("detached facets: {detached_label}"));
    }
    let open = profile
        .get("open_with_them")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if open.is_empty() {
        lines.push("No open loops.".to_string());
    } else {
        lines.extend(render_item_table(open, false));
    }
    lines.push(String::new());
    lines.push("Closed 30d".to_string());
    let closed = profile
        .get("closed_with_them_30d")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if closed.is_empty() {
        lines.push("No closed items.".to_string());
    } else {
        lines.extend(render_closed_table(closed));
    }
    lines.push(String::new());
    lines.push("Decisions".to_string());
    let decisions = profile
        .get("decisions_involving_them")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if decisions.is_empty() {
        lines.push("No decisions.".to_string());
    } else {
        lines.extend(render_decisions_table(decisions));
    }
    lines
}

fn render_item_table(items: &[Value], include_closed_at: bool) -> Vec<String> {
    let headers = if include_closed_at {
        vec!["id", "state", "age_days", "summary", "when", "closed_at"]
    } else {
        vec!["id", "state", "age_days", "summary", "when"]
    };
    let rows = items
        .iter()
        .map(|item| {
            let mut row = vec![
                field(item, "id"),
                field(item, "state"),
                field(item, "age_days"),
                item_summary(item),
                field_or_empty(item, "when"),
            ];
            if include_closed_at {
                row.push(field_or_empty(item, "closed_at"));
            }
            row
        })
        .collect::<Vec<_>>();
    render_table(&headers, &rows)
}

fn render_closed_table(items: &[Value]) -> Vec<String> {
    let rows = items
        .iter()
        .map(|item| {
            vec![
                field(item, "id"),
                field_or_empty(item, "closed_at"),
                item_summary(item),
            ]
        })
        .collect::<Vec<_>>();
    render_table(&["id", "closed_at", "summary"], &rows)
}

fn render_decisions_table(items: &[Value]) -> Vec<String> {
    let rows = items
        .iter()
        .map(|item| {
            vec![
                field(item, "id"),
                field(item, "day"),
                field(item, "owner"),
                with_said_by_you(item, field(item, "action")),
                field(item, "context"),
            ]
        })
        .collect::<Vec<_>>();
    render_table(&["id", "day", "owner", "action", "context"], &rows)
}

fn render_table(headers: &[&str], rows: &[Vec<String>]) -> Vec<String> {
    let widths = headers
        .iter()
        .enumerate()
        .map(|(index, header)| {
            rows.iter()
                .map(|row| row[index].chars().count())
                .chain([header.chars().count()])
                .max()
                .unwrap_or(0)
        })
        .collect::<Vec<_>>();
    let mut lines = vec![
        headers
            .iter()
            .enumerate()
            .map(|(index, header)| pad(header, widths[index]))
            .collect::<Vec<_>>()
            .join("  "),
        widths
            .iter()
            .map(|width| "-".repeat(*width))
            .collect::<Vec<_>>()
            .join("  "),
    ];
    for row in rows {
        lines.push(
            row.iter()
                .enumerate()
                .map(|(index, cell)| pad(cell, widths[index]))
                .collect::<Vec<_>>()
                .join("  "),
        );
    }
    lines
}

fn item_summary(item: &Value) -> String {
    let summary = if truthy(item.get("counterparty")) {
        format!(
            "{}: {} -> {}",
            field(item, "owner"),
            field(item, "summary"),
            field(item, "counterparty")
        )
    } else {
        format!("{}: {}", field(item, "owner"), field(item, "summary"))
    };
    with_said_by_you(item, summary)
}

fn with_said_by_you(item: &Value, text: String) -> String {
    if item.get("owner_evidence").and_then(Value::as_str) == Some("voice") {
        format!("{text}; {SAID_BY_YOU}")
    } else {
        text
    }
}

fn quote_path(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut output = String::new();
    for byte in value.as_bytes() {
        if matches!(
            byte,
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~'
        ) {
            output.push(char::from(*byte));
        } else {
            output.push('%');
            output.push(char::from(HEX[(byte >> 4) as usize]));
            output.push(char::from(HEX[(byte & 0x0F) as usize]));
        }
    }
    output
}

fn field(item: &Value, name: &str) -> String {
    display_value(&item[name])
}

fn field_or_empty(item: &Value, name: &str) -> String {
    if truthy(item.get(name)) {
        field(item, name)
    } else {
        String::new()
    }
}

fn display_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Null => "None".to_string(),
        other => other.to_string(),
    }
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Null) | None => false,
        Some(Value::Bool(value)) => *value,
        Some(Value::String(value)) => !value.is_empty(),
        Some(Value::Array(value)) => !value.is_empty(),
        Some(Value::Object(value)) => !value.is_empty(),
        Some(Value::Number(number)) => number.as_i64() != Some(0),
    }
}

fn stdout_compact_json(value: &Value) -> CommandOutput {
    CommandOutput::success(format!("{}\n", json_compact_ascii(value)))
}

fn stdout(lines: Vec<String>) -> CommandOutput {
    CommandOutput::success(format!("{}\n", lines.join("\n")))
}

fn stderr(value: impl AsRef<str>) -> CommandOutput {
    CommandOutput::failure(format!("{}\n", value.as_ref()), 1)
}

fn stderr_with_exit(value: impl Into<String>, exit: i32) -> CommandOutput {
    CommandOutput {
        stdout: String::new(),
        stderr: value.into(),
        exit,
    }
}

fn profile_error(error: ClientError, name: &str) -> CommandOutput {
    match error {
        ClientError::Unreachable { .. } => stderr(SERVICE_DOWN_MESSAGE),
        other if other.reason_code() == Some(ENTITY_NOT_FOUND) || other.status() == Some(404) => {
            stderr(format!("profile not found: {name}"))
        }
        other => stderr(other.detail().unwrap_or_else(|| other.message())),
    }
}

fn profile_list_error(error: ClientError) -> CommandOutput {
    match error {
        ClientError::Unreachable { .. } => stderr(SERVICE_DOWN_MESSAGE),
        other => stderr(other.detail().unwrap_or_else(|| other.message())),
    }
}

fn missing_argument(command: &str, name: &str) -> CommandOutput {
    let message = format!("Missing argument '{name}'.");
    let spaces = " ".repeat(77_usize.saturating_sub(message.chars().count()));
    stderr_with_exit(
        format!(
            "Usage: {command} [OPTIONS] {name}\n\
Try '{command} --help' for help.\n\
╭─ Error ──────────────────────────────────────────────────────────────────────╮\n\
│ {message}{spaces}│\n\
╰──────────────────────────────────────────────────────────────────────────────╯\n"
        ),
        2,
    )
}

fn pad(value: &str, width: usize) -> String {
    format!("{value:<width$}")
}

fn client_error_output(error: ClientError) -> CommandOutput {
    match error {
        ClientError::Unreachable { .. } => stderr(SERVICE_DOWN_MESSAGE),
        other => stderr(other.detail().unwrap_or_else(|| other.message())),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;
    use crate::command::CommandContext;
    use crate::seam::{ExpectedHttpCall, ScriptedHttpTransport};
    use crate::transport::{ApiRequest, HttpMethod, HttpResponse, TimeoutPolicy};

    fn test_ctx<'a>(
        args: &'a [String],
        transport: &'a ScriptedHttpTransport,
    ) -> CommandContext<'a> {
        let env: &'a BTreeMap<String, String> = Box::leak(Box::new(BTreeMap::new()));
        CommandContext {
            args,
            env,
            stdin: "",
            transport,
            clock: None,
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
            link_pairing: None,
            link_serve: None,
            link_status_probe: None,
        }
    }

    #[test]
    fn profile_close_dry_run_only_gets_and_shows_would_write() {
        let transport = ScriptedHttpTransport::new(vec![ExpectedHttpCall::Request {
            expected: ApiRequest {
                method: HttpMethod::Get,
                path: "/api/ledger/item123".to_string(),
                params: vec![],
                json: None,
                headers: vec![],
                policy: TimeoutPolicy::Api,
            },
            result: Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: json!({"id": "item123", "state": "open"})
                    .to_string()
                    .into_bytes(),
                policy: TimeoutPolicy::Api,
            }),
        }]);

        let args = vec!["item123".to_string(), "--dry-run".to_string()];
        let output = close(test_ctx(&args, &transport));
        transport.assert_done();

        assert_eq!(output.exit, 0);
        assert_eq!(
            output.stdout,
            "item123 is open. a close would mark it closed.\n"
        );
    }

    #[test]
    fn profile_close_real_write_posts_and_confirms() {
        let transport = ScriptedHttpTransport::new(vec![ExpectedHttpCall::Request {
            expected: ApiRequest {
                method: HttpMethod::Post,
                path: "/api/ledger/item123/close".to_string(),
                params: vec![],
                json: Some(json!({"note": "sent", "as_state": "closed"})),
                headers: vec![],
                policy: TimeoutPolicy::Api,
            },
            result: Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: json!({
                    "confirmed": true,
                    "item": {"id": "item123", "state": "closed"}
                })
                .to_string()
                .into_bytes(),
                policy: TimeoutPolicy::Api,
            }),
        }]);

        let args = vec![
            "item123".to_string(),
            "--note".to_string(),
            "sent".to_string(),
        ];
        let output = close(test_ctx(&args, &transport));
        transport.assert_done();

        assert_eq!(output.exit, 0);
        assert_eq!(output.stdout, "item123 is closed.\n");
    }

    #[test]
    fn profile_close_dropped_flag_posts_dropped_state() {
        let transport = ScriptedHttpTransport::new(vec![ExpectedHttpCall::Request {
            expected: ApiRequest {
                method: HttpMethod::Post,
                path: "/api/ledger/item123/close".to_string(),
                params: vec![],
                json: Some(json!({"note": "dropping", "as_state": "dropped"})),
                headers: vec![],
                policy: TimeoutPolicy::Api,
            },
            result: Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: json!({
                    "confirmed": true,
                    "item": {"id": "item123", "state": "dropped"}
                })
                .to_string()
                .into_bytes(),
                policy: TimeoutPolicy::Api,
            }),
        }]);

        let args = vec![
            "item123".to_string(),
            "--dropped".to_string(),
            "--note".to_string(),
            "dropping".to_string(),
        ];
        let output = close(test_ctx(&args, &transport));
        transport.assert_done();

        assert_eq!(output.exit, 0);
        assert_eq!(output.stdout, "item123 is dropped.\n");
    }

    #[test]
    fn profile_close_unconfirmed_reports_did_not_take_effect() {
        let transport = ScriptedHttpTransport::new(vec![ExpectedHttpCall::Request {
            expected: ApiRequest {
                method: HttpMethod::Post,
                path: "/api/ledger/item123/close".to_string(),
                params: vec![],
                json: Some(json!({"note": "drop", "as_state": "dropped"})),
                headers: vec![],
                policy: TimeoutPolicy::Api,
            },
            result: Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: json!({
                    "confirmed": false,
                    "item": {"id": "item123", "state": "closed"}
                })
                .to_string()
                .into_bytes(),
                policy: TimeoutPolicy::Api,
            }),
        }]);

        let args = vec![
            "item123".to_string(),
            "--dropped".to_string(),
            "--note".to_string(),
            "drop".to_string(),
        ];
        let output = close(test_ctx(&args, &transport));
        transport.assert_done();

        assert_eq!(output.exit, 1);
        assert!(
            output
                .stderr
                .contains("saved item123 as dropped, but it is still ")
        );
    }

    #[test]
    fn profile_close_unknown_id_outputs_server_detail() {
        let transport = ScriptedHttpTransport::new(vec![ExpectedHttpCall::Request {
            expected: ApiRequest {
                method: HttpMethod::Get,
                path: "/api/ledger/item123".to_string(),
                params: vec![],
                json: None,
                headers: vec![],
                policy: TimeoutPolicy::Api,
            },
            result: Err(ClientError::ReasonRejected {
                status: 404,
                error: "that item was not found.".to_string(),
                reason_code: Some("ledger_item_unknown".to_string()),
                detail: Some("that item was not found. run solstone call profile full <name> for current ids.".to_string()),
                payload: Box::new(serde_json::Value::Null),
            }),
        }]);

        let args = vec!["item123".to_string(), "--dry-run".to_string()];
        let output = close(test_ctx(&args, &transport));
        transport.assert_done();

        assert_eq!(output.exit, 1);
        assert!(output.stderr.contains("that item was not found."));
    }

    #[test]
    fn profile_close_missing_or_blank_note_fails_without_http_call() {
        let transport = ScriptedHttpTransport::new(vec![]);
        let args = vec!["item123".to_string()];
        let output = close(test_ctx(&args, &transport));
        transport.assert_done();
        assert_eq!(output.exit, 1);
        assert_eq!(output.stderr, "a note is required. add --note \"...\".\n");

        let transport_blank = ScriptedHttpTransport::new(vec![]);
        let args_blank = vec![
            "item123".to_string(),
            "--note".to_string(),
            "   ".to_string(),
        ];
        let output_blank = close(test_ctx(&args_blank, &transport_blank));
        transport_blank.assert_done();
        assert_eq!(output_blank.exit, 1);
        assert_eq!(
            output_blank.stderr,
            "a note is required. add --note \"...\".\n"
        );
    }

    #[test]
    fn profile_close_missing_positional_item_id_exits_2() {
        let transport = ScriptedHttpTransport::new(vec![]);
        let args = vec![];
        let output = close(test_ctx(&args, &transport));
        transport.assert_done();
        assert_eq!(output.exit, 2);
        assert!(output.stderr.contains("ITEM_ID"));

        let transport_item = ScriptedHttpTransport::new(vec![]);
        let output_item = item(test_ctx(&args, &transport_item));
        transport_item.assert_done();
        assert_eq!(output_item.exit, 2);
        assert!(output_item.stderr.contains("ITEM_ID"));
    }

    #[test]
    fn what_the_owner_said_by_voice_is_marked_and_nothing_else_is() {
        let profile = json!({
            "name": "Pat", "type": "person", "facets": ["work"], "is_self": false,
            "blocked": false, "cadence": {},
            "open_with_them": [
                {"id": "said", "state": "open", "age_days": 1, "owner": "you",
                 "summary": "send the deck", "counterparty": "Pat", "owner_evidence": "voice"},
                {"id": "plain", "state": "open", "age_days": 4, "owner": "you",
                 "summary": "book the room", "counterparty": "Pat"}
            ],
            "closed_with_them_30d": [
                {"id": "done", "closed_at": 1, "owner": "you", "summary": "share notes",
                 "owner_evidence": "voice"}
            ],
            "decisions_involving_them": [
                {"id": "chose", "day": "20261003", "owner": "Pat", "action": "go with plan b"}
            ]
        });
        let lines = render_full(&profile);
        let row = |id: &str| {
            lines
                .iter()
                .find(|line| line.starts_with(id))
                .unwrap_or_else(|| panic!("row {id}"))
                .clone()
        };
        assert!(row("said").contains(SAID_BY_YOU));
        assert!(row("done").contains(SAID_BY_YOU));
        assert!(!row("plain").contains(SAID_BY_YOU));
        assert!(!row("chose").contains(SAID_BY_YOU));
    }

    #[test]
    fn profile_item_marks_what_the_owner_said_by_voice() {
        let transport = ScriptedHttpTransport::new(vec![ExpectedHttpCall::Request {
            expected: ApiRequest {
                method: HttpMethod::Get,
                path: "/api/ledger/item123".to_string(),
                params: vec![],
                json: None,
                headers: vec![],
                policy: TimeoutPolicy::Api,
            },
            result: Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: json!({"id": "item123", "state": "open", "owner_evidence": "voice"})
                    .to_string()
                    .into_bytes(),
                policy: TimeoutPolicy::Api,
            }),
        }]);

        let args = vec!["item123".to_string()];
        let output = item(test_ctx(&args, &transport));
        transport.assert_done();
        assert_eq!(output.exit, 0);
        assert!(output.stdout.contains(SAID_BY_YOU));
    }

    #[test]
    fn profile_close_item_success_renders_stdout() {
        let transport = ScriptedHttpTransport::new(vec![ExpectedHttpCall::Request {
            expected: ApiRequest {
                method: HttpMethod::Get,
                path: "/api/ledger/item123".to_string(),
                params: vec![],
                json: None,
                headers: vec![],
                policy: TimeoutPolicy::Api,
            },
            result: Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: json!({"id": "item123", "state": "open"})
                    .to_string()
                    .into_bytes(),
                policy: TimeoutPolicy::Api,
            }),
        }]);

        let args = vec!["item123".to_string()];
        let output = item(test_ctx(&args, &transport));
        transport.assert_done();
        assert_eq!(output.exit, 0);
        assert_eq!(output.stdout, "item123 is open.\n");
    }
}
