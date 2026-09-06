// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;

use crate::command::{CommandContext, CommandOutput};
use crate::decode::decode_response;
use crate::error::{ClientError, SERVICE_DOWN_MESSAGE};
use crate::json_format::sorted_json_pretty_ascii;
use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};

const BODY_RESPONSE_ERROR: &str = "the response from the body app couldn't be read.";

#[must_use]
pub fn status(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &["--json"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let payload = match request_json(ctx, "/app/body/api/status", vec![]) {
        Ok(payload) => payload,
        Err(error) => return body_error(error),
    };
    if parsed.has_flag("--json") {
        return stdout_json(&payload);
    }
    let Some(object) = payload.as_object() else {
        return stderr(BODY_RESPONSE_ERROR);
    };
    let imports = object
        .get("imports")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let entries = object
        .get("normalized")
        .and_then(Value::as_object)
        .and_then(|normalized| normalized.get("total"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let mut lines = vec![format!("imports: {imports}"), format!("entries: {entries}")];
    if let Some(coverage) = object.get("coverage_window").and_then(Value::as_object)
        && let Some(start) = coverage.get("start").and_then(Value::as_str)
    {
        let end = coverage.get("end").and_then(Value::as_str).unwrap_or("");
        lines.push(format!("coverage: {start} to {end}"));
    }
    stdout(lines)
}

#[must_use]
pub fn day(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &["--json"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(day_value) = parsed.positionals.first() else {
        return typer_missing_day_value();
    };
    let payload = match request_json(ctx, &format!("/app/body/api/day/{day_value}"), vec![]) {
        Ok(payload) => payload,
        Err(error) => return body_error(error),
    };
    if parsed.has_flag("--json") {
        return stdout_json(&payload);
    }
    let Some(object) = payload.as_object() else {
        return stderr(BODY_RESPONSE_ERROR);
    };
    let glucose = object.get("glucose").and_then(Value::as_object);
    stdout(vec![
        format!("day: {}", string_field(object.get("day"), day_value)),
        format!("entries: {}", display_or_zero(object.get("entry_total"))),
        format!(
            "glucose: count={} min={} max={} mean={} unit={}",
            display_or_zero(glucose.and_then(|item| item.get("count"))),
            display_value(glucose.and_then(|item| item.get("min"))),
            display_value(glucose.and_then(|item| item.get("max"))),
            display_value(glucose.and_then(|item| item.get("mean"))),
            display_value(glucose.and_then(|item| item.get("unit"))),
        ),
    ])
}

#[must_use]
pub fn window(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &["--from", "--to"], &["--json"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(from_value) = parsed.value("--from") else {
        return typer_missing_window_option("--from");
    };
    let Some(to_value) = parsed.value("--to") else {
        return typer_missing_window_option("--to");
    };
    let payload = match request_json(
        ctx,
        "/app/body/api/window",
        vec![
            QueryParam::single("from", from_value),
            QueryParam::single("to", to_value),
        ],
    ) {
        Ok(payload) => payload,
        Err(error) => return body_error(error),
    };
    if parsed.has_flag("--json") {
        return stdout_json(&payload);
    }
    let Some(object) = payload.as_object() else {
        return stderr(BODY_RESPONSE_ERROR);
    };
    let mut lines = vec![
        format!(
            "window: {} to {}",
            display_value(object.get("from")),
            display_value(object.get("to"))
        ),
        format!("entries: {}", display_or_zero(object.get("entry_total"))),
    ];
    if let Some(brief) = object.get("brief_label")
        && truthy(brief)
    {
        lines.push(display_value(Some(brief)));
    }
    stdout(lines)
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

fn stdout_json(value: &Value) -> CommandOutput {
    CommandOutput::success(format!("{}\n", sorted_json_pretty_ascii(value)))
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

fn typer_missing_day_value() -> CommandOutput {
    stderr_with_exit(
        "Usage: call body day [OPTIONS] DAY_VALUE\n\
Try 'call body day --help' for help.\n\
╭─ Error ──────────────────────────────────────────────────────────────────────╮\n\
│ Missing argument 'DAY_VALUE'.                                                │\n\
╰──────────────────────────────────────────────────────────────────────────────╯\n",
        2,
    )
}

fn typer_missing_window_option(option: &str) -> CommandOutput {
    let message = match option {
        "--from" => {
            "│ Missing option '--from'.                                                     │"
        }
        "--to" => {
            "│ Missing option '--to'.                                                       │"
        }
        _ => unreachable!("unsupported body window option"),
    };
    stderr_with_exit(
        format!(
            "Usage: call body window [OPTIONS]\n\
Try 'call body window --help' for help.\n\
╭─ Error ──────────────────────────────────────────────────────────────────────╮\n\
{message}\n\
╰──────────────────────────────────────────────────────────────────────────────╯\n"
        ),
        2,
    )
}

fn body_error(error: ClientError) -> CommandOutput {
    match error {
        ClientError::Unreachable { .. } => stderr(SERVICE_DOWN_MESSAGE),
        _ => stderr(error.message()),
    }
}

fn string_field(value: Option<&Value>, default: &str) -> String {
    value
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| default.to_string())
}

fn display_or_zero(value: Option<&Value>) -> String {
    value.map_or_else(|| "0".to_string(), |item| display_value(Some(item)))
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

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::String(value) => !value.is_empty(),
        _ => true,
    }
}
