// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};

use crate::command::{CommandContext, CommandOutput};
use crate::decode::decode_response;
use crate::error::{ClientError, SERVICE_DOWN_MESSAGE};
use crate::json_format::json_pretty_ascii;
use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};

const LOG_PAGE_SIZE: usize = 100;

#[must_use]
pub fn status(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let state = match request_json(
        ctx,
        HttpMethod::Get,
        "/app/awareness/api/state",
        vec![],
        None,
    ) {
        Ok(state) => state,
        Err(error) => return awareness_error(error),
    };
    if !truthy(&state) {
        return stdout_line("No awareness state yet.");
    }
    if let Some(section) = parsed.positionals.first() {
        let Some(value) = state
            .as_object()
            .and_then(|object| object.get(section))
            .filter(|value| !value.is_null())
        else {
            return stdout_line(format!("No '{section}' state."));
        };
        return stdout_json(value);
    }
    stdout_json(&state)
}

#[must_use]
pub fn imports(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[("--record", Some("-r"))],
        &["--declined", "--nudge"],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let result = if let Some(record) = parsed.value("--record").filter(|record| !record.is_empty())
    {
        request_json(
            ctx,
            HttpMethod::Post,
            "/app/awareness/api/imports",
            vec![],
            Some(json!({"record": record})),
        )
    } else if parsed.has_flag("--declined") {
        request_json(
            ctx,
            HttpMethod::Post,
            "/app/awareness/api/imports",
            vec![],
            Some(json!({"declined": true})),
        )
    } else if parsed.has_flag("--nudge") {
        request_json(
            ctx,
            HttpMethod::Post,
            "/app/awareness/api/imports",
            vec![],
            Some(json!({"nudge": true})),
        )
    } else {
        request_json(
            ctx,
            HttpMethod::Get,
            "/app/awareness/api/imports",
            vec![],
            None,
        )
    };
    match result {
        Ok(value) => stdout_json(&value),
        Err(error) => awareness_error(error),
    }
}

#[must_use]
pub fn log(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[("--key", Some("-k")), ("--data", Some("-d"))],
        &[],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(kind) = parsed.positionals.first() else {
        return missing_argument("call awareness log", "KIND");
    };
    let message = parsed.positionals.get(1).cloned();
    let data = if let Some(data) = parsed.value("--data") {
        if data.is_empty() {
            Value::Null
        } else {
            match serde_json::from_str::<Value>(data) {
                Ok(value) => value,
                Err(_error) => return stderr("Error: --data must be valid JSON"),
            }
        }
    } else {
        Value::Null
    };
    let key = parsed
        .value("--key")
        .map(|value| Value::String(value.to_string()))
        .unwrap_or(Value::Null);
    let message = message.map(Value::String).unwrap_or(Value::Null);
    match request_json(
        ctx,
        HttpMethod::Post,
        "/app/awareness/api/log",
        vec![],
        Some(json!({"kind": kind, "key": key, "message": message, "data": data})),
    ) {
        Ok(value) => stdout_json(&value),
        Err(error) => awareness_error(error),
    }
}

#[must_use]
pub fn log_read(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[("--kind", Some("-k")), ("--limit", Some("-n"))],
        &[],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let limit = match parsed.value("--limit") {
        Some(value) => match value.parse::<isize>() {
            Ok(value) => value,
            Err(_error) => return stderr("Error: option --limit requires an integer."),
        },
        None => 0,
    };
    let mut entries: Vec<Value> = Vec::new();
    let mut offset = 0_usize;
    loop {
        let mut params = vec![
            QueryParam::single("limit", LOG_PAGE_SIZE.to_string()),
            QueryParam::single("offset", offset.to_string()),
        ];
        if let Some(day) = parsed.positionals.first() {
            params.push(QueryParam::single("day", day));
        }
        if let Some(kind) = parsed.value("--kind") {
            params.push(QueryParam::single("kind", kind));
        }
        let body = match request_json(ctx, HttpMethod::Get, "/app/awareness/api/log", params, None)
        {
            Ok(body) => body,
            Err(error) => return awareness_error(error),
        };
        let items = body
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let total = body.get("total").and_then(Value::as_i64).unwrap_or(0);
        entries.extend(items.iter().cloned());
        offset += items.len();
        if items.is_empty() || (entries.len() as i64) >= total {
            break;
        }
    }
    if limit > 0 && entries.len() > limit as usize {
        entries = entries.split_off(entries.len() - limit as usize);
    }
    if entries.is_empty() {
        return stdout_line("No entries found.");
    }
    stdout_json(&Value::Array(entries))
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

fn parse_args(
    args: &[String],
    options: &[(&str, Option<&str>)],
    flags: &[&str],
) -> Result<ParsedArgs, String> {
    let mut parsed = ParsedArgs::default();
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        if let Some((name, value)) = token.split_once('=')
            && let Some(canonical) = canonical_option(name, options)
        {
            parsed
                .values
                .push((canonical.to_string(), value.to_string()));
        } else if let Some(canonical) = canonical_option(token, options) {
            index += 1;
            let Some(value) = args.get(index) else {
                return Err(format!("Error: option {token} requires an argument."));
            };
            parsed
                .values
                .push((canonical.to_string(), value.to_string()));
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

fn canonical_option<'a>(token: &str, options: &'a [(&'a str, Option<&'a str>)]) -> Option<&'a str> {
    options.iter().find_map(|(long, short)| {
        if token == *long || short.is_some_and(|short| token == short) {
            Some(*long)
        } else {
            None
        }
    })
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

fn stdout_json(value: &Value) -> CommandOutput {
    CommandOutput::success(format!("{}\n", json_pretty_ascii(value)))
}

fn stdout_line(value: impl AsRef<str>) -> CommandOutput {
    CommandOutput::success(format!("{}\n", value.as_ref()))
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

fn awareness_error(error: ClientError) -> CommandOutput {
    match error {
        ClientError::Unreachable { .. } => stderr(SERVICE_DOWN_MESSAGE),
        other => stderr(other.message()),
    }
}

fn missing_argument(command: &str, name: &str) -> CommandOutput {
    let message = format!("Missing argument '{name}'.");
    let spaces = " ".repeat(77_usize.saturating_sub(message.chars().count()));
    stderr_with_exit(
        format!(
            "Usage: {command} [OPTIONS] {name} [MESSAGE]\n\
Try '{command} --help' for help.\n\
╭─ Error ──────────────────────────────────────────────────────────────────────╮\n\
│ {message}{spaces}│\n\
╰──────────────────────────────────────────────────────────────────────────────╯\n"
        ),
        2,
    )
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64() != Some(0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
    }
}
