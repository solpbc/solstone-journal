// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};

use crate::command::{CommandContext, CommandOutput};
use crate::decode::decode_response;
use crate::error::{ClientError, SERVICE_DOWN_MESSAGE};
use crate::json_format::json_pretty_utf8;
use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};

#[must_use]
pub fn list_candidates(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[("--status", None)], &["--json"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let payload = match request_json(
        ctx,
        HttpMethod::Get,
        "/app/curation/api/facet/candidates",
        vec![],
        None,
    ) {
        Ok(payload) => payload,
        Err(error) => return facets_error(error),
    };
    let mut rows = payload
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if let Some(status) = parsed.value("--status") {
        rows.retain(|row| row.get("status").and_then(Value::as_str) == Some(status));
    }
    if parsed.has_flag("--json") {
        return stdout_json(&Value::Array(rows));
    }
    if rows.is_empty() {
        return stdout_line("No facet candidates found.");
    }
    let mut lines = Vec::new();
    for row in &rows {
        lines.push(format!(
            "{}  [{}]  count={}  last={}",
            display_or(row.get("name"), ""),
            display_or(row.get("status"), ""),
            display_value(row.get("count")),
            display_or(row.get("last_surfaced"), ""),
        ));
    }
    stdout(lines)
}

#[must_use]
pub fn accept(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(name_key) = parsed.positionals.first() else {
        return missing_argument("call facets accept", "NAME_KEY");
    };
    let result = match request_json(
        ctx,
        HttpMethod::Post,
        "/app/curation/api/facet/accept",
        vec![],
        Some(json!({"name_key": name_key})),
    ) {
        Ok(result) => result,
        Err(error) => return facets_error(error),
    };
    stdout_line(render_result(&result, "accept", name_key))
}

#[must_use]
pub fn dismiss(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(name_key) = parsed.positionals.first() else {
        return missing_argument("call facets dismiss", "NAME_KEY");
    };
    let result = match request_json(
        ctx,
        HttpMethod::Post,
        "/app/curation/api/facet/dismiss",
        vec![],
        Some(json!({"name_key": name_key})),
    ) {
        Ok(result) => result,
        Err(error) => return facets_error(error),
    };
    stdout_line(render_result(&result, "dismiss", name_key))
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

fn render_result(result: &Value, action: &str, name_key: &str) -> String {
    let status = result.get("status").and_then(Value::as_str);
    match status {
        Some("accepted") => format!(
            "Accepted facet candidate '{name_key}' as '{}'.",
            display_value(result.get("facet_slug"))
        ),
        Some("dismissed") => format!("Dismissed facet candidate '{name_key}'."),
        Some("already_accepted") => format!("Facet candidate '{name_key}' already accepted."),
        Some("already_dismissed") => format!("Facet candidate '{name_key}' already dismissed."),
        other => format!(
            "{action} result for '{name_key}': {}",
            other.unwrap_or("None")
        ),
    }
}

fn stdout_json(value: &Value) -> CommandOutput {
    CommandOutput::success(format!("{}\n", json_pretty_utf8(value)))
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

fn facets_error(error: ClientError) -> CommandOutput {
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
            "Usage: {command} [OPTIONS] {name}\n\
Try '{command} --help' for help.\n\
╭─ Error ──────────────────────────────────────────────────────────────────────╮\n\
│ {message}{spaces}│\n\
╰──────────────────────────────────────────────────────────────────────────────╯\n"
        ),
        2,
    )
}

fn display_or(value: Option<&Value>, default: &str) -> String {
    match value {
        Some(Value::String(value)) if !value.is_empty() => value.clone(),
        Some(Value::String(_)) | None => default.to_string(),
        Some(other) => display_value(Some(other)),
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
