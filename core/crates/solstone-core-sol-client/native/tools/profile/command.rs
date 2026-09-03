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
    // there (founder ruling 2026-09-03), so this renderer -- a caller -- is where
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
                field(item, "action"),
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
    if truthy(item.get("counterparty")) {
        format!(
            "{}: {} -> {}",
            field(item, "owner"),
            field(item, "summary"),
            field(item, "counterparty")
        )
    } else {
        format!("{}: {}", field(item, "owner"), field(item, "summary"))
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
