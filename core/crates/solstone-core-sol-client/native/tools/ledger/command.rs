// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};

use crate::command::{CommandContext, CommandOutput};
use crate::decode::decode_response;
use crate::error::{ClientError, SERVICE_DOWN_MESSAGE};
use crate::json_format::json_pretty_ascii;
use crate::pagination::paginate_collection;
use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};

const LEDGER_ITEM_NOT_FOUND: &str = "ledger_item_not_found";
const ACTIVITIES_BUSY: &str = "activities_busy";
const ACTIVITIES_BUSY_MESSAGE: &str =
    "I couldn't update activities right now because they were busy. Try again in a moment.";

#[must_use]
pub fn list(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[
            "--state",
            "--owner",
            "--counterparty",
            "--age-days-gte",
            "--closed-since",
            "--top",
            "--sort",
            "--facets",
        ],
        &["--json"],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    if let Some(sort) = parsed.value("--sort")
        && !matches!(sort, "age_days_desc" | "opened_at_desc" | "closed_at_desc")
    {
        return typer_bad_parameter(
            "call ledger list",
            "",
            "sort must be one of age_days_desc, opened_at_desc, closed_at_desc",
        );
    }
    let top = match parsed.value("--top").map(parse_usize).transpose() {
        Ok(value) => value,
        Err(error) => return stderr(error),
    };
    let mut params = vec![QueryParam::single(
        "state",
        parsed.value("--state").unwrap_or("open"),
    )];
    push_param(&mut params, "owner", parsed.value("--owner"));
    push_param(&mut params, "counterparty", parsed.value("--counterparty"));
    push_param(&mut params, "age_days_gte", parsed.value("--age-days-gte"));
    push_param(&mut params, "closed_since", parsed.value("--closed-since"));
    push_param(&mut params, "sort", parsed.value("--sort"));
    push_param(&mut params, "facets", parsed.value("--facets"));

    let items = match paginate_collection(ctx.transport, "/api/ledger", params, top) {
        Ok(items) => items,
        Err(error) => return ledger_error(error, None),
    };
    if parsed.has_flag("--json") {
        return stdout_json(&Value::Array(items));
    }
    render_items(&items)
}

#[must_use]
pub fn decisions(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &["--owner", "--since", "--involving", "--top", "--facets"],
        &["--json"],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let top = match parsed.value("--top").map(parse_usize).transpose() {
        Ok(value) => value,
        Err(error) => return stderr(error),
    };
    let mut params = Vec::new();
    push_param(&mut params, "owner", parsed.value("--owner"));
    push_param(&mut params, "since", parsed.value("--since"));
    push_param(&mut params, "involving", parsed.value("--involving"));
    push_param(&mut params, "facets", parsed.value("--facets"));

    let items = match paginate_collection(ctx.transport, "/api/ledger/decisions", params, top) {
        Ok(items) => items,
        Err(error) => return ledger_error(error, None),
    };
    if parsed.has_flag("--json") {
        return stdout_json(&Value::Array(items));
    }
    render_decisions(&items)
}

#[must_use]
pub fn get(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &["--json"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(item_id) = parsed.positionals.first() else {
        return missing_argument("call ledger get", "ITEM_ID");
    };
    let item = match request_json(
        ctx,
        HttpMethod::Get,
        &format!("/api/ledger/{item_id}"),
        vec![],
        None,
    ) {
        Ok(item) => item,
        Err(error) => return ledger_error(error, Some(item_id)),
    };
    if parsed.has_flag("--json") {
        return stdout_json(&Value::Array(vec![item]));
    }
    render_items(&[item])
}

#[must_use]
pub fn close(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &["--note", "--as"], &["--json"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(item_id) = parsed.positionals.first() else {
        return missing_argument("call ledger close", "ITEM_ID");
    };
    let Some(note) = parsed.value("--note") else {
        return missing_option("call ledger close", "--note");
    };
    let as_state = parsed.value("--as").unwrap_or("closed");
    if !matches!(as_state, "closed" | "dropped") {
        return typer_bad_parameter(
            "call ledger close",
            "ITEM_ID",
            "as_state must be 'closed' or 'dropped'",
        );
    }
    let item = match request_json(
        ctx,
        HttpMethod::Post,
        &format!("/api/ledger/{item_id}/close"),
        vec![],
        Some(json!({"note": note, "as_state": as_state})),
    ) {
        Ok(item) => item,
        Err(error) => return ledger_error(error, Some(item_id)),
    };
    if parsed.has_flag("--json") {
        return stdout_json(&Value::Array(vec![item]));
    }
    render_items(&[item])
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

fn render_items(items: &[Value]) -> CommandOutput {
    if items.is_empty() {
        return stdout_line("No ledger items found.");
    }
    let mut rows = Vec::new();
    for item in items {
        rows.push(vec![
            field(item, "id"),
            field(item, "state"),
            field(item, "age_days"),
            item_summary(item),
            field_or_empty(item, "when"),
            field(item, "opened_at"),
            field_or_empty(item, "closed_at"),
        ]);
    }
    stdout(render_table(
        &[
            "id",
            "state",
            "age_days",
            "summary",
            "when",
            "opened_at",
            "closed_at",
        ],
        &rows,
    ))
}

fn render_decisions(items: &[Value]) -> CommandOutput {
    if items.is_empty() {
        return stdout_line("No decisions found.");
    }
    let mut rows = Vec::new();
    for item in items {
        rows.push(vec![
            field(item, "id"),
            field(item, "day"),
            field(item, "owner"),
            field(item, "action"),
            field(item, "context"),
        ]);
    }
    stdout(render_table(
        &["id", "day", "owner", "action", "context"],
        &rows,
    ))
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

fn field(item: &Value, name: &str) -> String {
    display_value(item.get(name))
}

fn field_or_empty(item: &Value, name: &str) -> String {
    if truthy(item.get(name)) {
        field(item, name)
    } else {
        String::new()
    }
}

fn display_value(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Bool(true)) => "True".to_string(),
        Some(Value::Bool(false)) => "False".to_string(),
        Some(Value::Null) | None => "None".to_string(),
        Some(other) => other.to_string(),
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

fn push_param(params: &mut Vec<QueryParam>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        params.push(QueryParam::single(key, value));
    }
}

fn parse_usize(value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_error| format!("Error: invalid integer value: {value}"))
}

fn stdout_json(value: &Value) -> CommandOutput {
    CommandOutput::success(format!("{}\n", json_pretty_ascii(value)))
}

fn stdout_line(value: impl AsRef<str>) -> CommandOutput {
    stdout(vec![value.as_ref().to_string()])
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

fn ledger_error(error: ClientError, item_id: Option<&str>) -> CommandOutput {
    match error {
        ClientError::Unreachable { .. } => stderr(SERVICE_DOWN_MESSAGE),
        other if other.reason_code() == Some(LEDGER_ITEM_NOT_FOUND) => stderr(format!(
            "ledger item not found: {}",
            item_id.unwrap_or("None")
        )),
        other if other.reason_code() == Some(ACTIVITIES_BUSY) => stderr(ACTIVITIES_BUSY_MESSAGE),
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

fn missing_option(command: &str, name: &str) -> CommandOutput {
    let message = format!("Missing option '{name}'.");
    let spaces = " ".repeat(77_usize.saturating_sub(message.chars().count()));
    stderr_with_exit(
        format!(
            "Usage: {command} [OPTIONS] ITEM_ID\n\
Try '{command} --help' for help.\n\
╭─ Error ──────────────────────────────────────────────────────────────────────╮\n\
│ {message}{spaces}│\n\
╰──────────────────────────────────────────────────────────────────────────────╯\n"
        ),
        2,
    )
}

fn typer_bad_parameter(command: &str, usage_args: &str, message: &str) -> CommandOutput {
    let message = format!("Invalid value: {message}");
    let lines = wrap_panel_text(&message, 65);
    let usage_args = if usage_args.is_empty() {
        String::new()
    } else {
        format!(" {usage_args}")
    };
    let body = lines
        .iter()
        .map(|line| {
            let spaces = " ".repeat(77_usize.saturating_sub(line.chars().count()));
            format!("│ {line}{spaces}│")
        })
        .collect::<Vec<_>>()
        .join("\n");
    stderr_with_exit(
        format!(
            "Usage: {command} [OPTIONS]{usage_args}\n\
Try '{command} --help' for help.\n\
╭─ Error ──────────────────────────────────────────────────────────────────────╮\n\
{body}\n\
╰──────────────────────────────────────────────────────────────────────────────╯\n"
        ),
        2,
    )
}

fn wrap_panel_text(message: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in message.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
        } else if current.chars().count() + 1 + word.chars().count() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(current);
            current = word.to_string();
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn pad(value: &str, width: usize) -> String {
    format!("{value:<width$}")
}
