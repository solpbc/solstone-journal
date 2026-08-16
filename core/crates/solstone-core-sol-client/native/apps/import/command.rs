// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};

use crate::command::{CommandContext, CommandOutput};
use crate::decode::decode_response;
use crate::error::{ClientError, SERVICE_DOWN_MESSAGE};
use crate::json_format::json_compact_utf8;
use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};

const JOURNAL_SOURCE_PROBLEM: &str = "journal_source_problem";
const IMPORT_NOT_FOUND: &str = "import_not_found";
const INVALID_REQUEST_VALUE: &str = "invalid_request_value";
const SOURCE_NOT_FOUND_SUFFIX: &str = "not found. Check available sources in ~/.local/share/solstone/app-storage/import/journal_sources/.";

#[must_use]
pub fn list_staged(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &["--source", "--area"], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(source) = parsed.value("--source") else {
        return stderr("Error: option --source is required.");
    };
    let mut params = Vec::new();
    push_param(&mut params, "area", parsed.value("--area"));
    let body = match request_json(
        ctx,
        HttpMethod::Get,
        &format!("/app/import/api/journal-sources/{source}/staged"),
        params,
        None,
    ) {
        Ok(body) => body,
        Err(error) => return import_error(error, source, true),
    };
    let items = body
        .as_object()
        .and_then(|object| object.get("items"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let stdout = items
        .iter()
        .map(json_compact_utf8)
        .collect::<Vec<_>>()
        .join("\n");
    if stdout.is_empty() {
        CommandOutput::success(String::new())
    } else {
        CommandOutput::success(format!("{stdout}\n"))
    }
}

#[must_use]
pub fn resolve_config(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &["--source"], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(field) = parsed.positionals.first() else {
        return stderr("Error: missing argument FIELD");
    };
    let Some(action) = parsed.positionals.get(1) else {
        return stderr("Error: missing argument ACTION");
    };
    let Some(source) = parsed.value("--source") else {
        return stderr("Error: option --source is required.");
    };
    match request_json(
        ctx,
        HttpMethod::Post,
        &format!("/app/import/api/journal-sources/{source}/resolve-config"),
        vec![],
        Some(json!({"field": field, "action": action})),
    ) {
        Ok(_body) => stdout_line(format!(
            "Resolved config field {field} with action {action}."
        )),
        Err(error) => import_error(error, source, false),
    }
}

#[must_use]
pub fn resolve_config_all(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &["--source", "--category"], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(source) = parsed.value("--source") else {
        return stderr("Error: option --source is required.");
    };
    let Some(category) = parsed.value("--category") else {
        return stderr("Error: option --category is required.");
    };
    match request_json(
        ctx,
        HttpMethod::Post,
        &format!("/app/import/api/journal-sources/{source}/resolve-config-all"),
        vec![],
        Some(json!({"category": category})),
    ) {
        Ok(body) => stdout_line(format!(
            "Applied {} {category} config field(s).",
            body.get("count").and_then(Value::as_i64).unwrap_or(0)
        )),
        Err(error) => import_error(error, source, false),
    }
}

#[must_use]
pub fn resolve_entity(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &["--source", "--target"], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(source_id) = parsed.positionals.first() else {
        return stderr("Error: missing argument SOURCE_ID");
    };
    let Some(action) = parsed.positionals.get(1) else {
        return stderr("Error: missing argument ACTION");
    };
    let Some(source) = parsed.value("--source") else {
        return stderr("Error: option --source is required.");
    };
    let target = parsed
        .value("--target")
        .map(|value| Value::String(value.to_string()))
        .unwrap_or(Value::Null);
    match request_json(
        ctx,
        HttpMethod::Post,
        &format!("/app/import/api/journal-sources/{source}/resolve-entity"),
        vec![],
        Some(json!({"source_id": source_id, "action": action, "target": target})),
    ) {
        Ok(_body) if action == "merge" => stdout_line(format!(
            "Merged {source_id} into {}.",
            parsed.value("--target").unwrap_or("None")
        )),
        Ok(body) if action == "create" => stdout_line(format!(
            "Created entity {} from {source_id}.",
            body.get("target_id")
                .and_then(Value::as_str)
                .unwrap_or("None")
        )),
        Ok(_body) => stdout_line(format!("Skipped staged entity {source_id}.")),
        Err(error) => import_error(error, source, false),
    }
}

#[must_use]
pub fn resolve_staged_facet(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &["--source"], &["--apply", "--skip"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(staged_file) = parsed.positionals.first() else {
        return stderr("Error: missing argument STAGED_FILE");
    };
    if parsed.has_flag("--apply") == parsed.has_flag("--skip") {
        return stderr("Error: Exactly one of --apply or --skip is required.");
    }
    let Some(source) = parsed.value("--source") else {
        return stderr("Error: option --source is required.");
    };
    let mode = if parsed.has_flag("--apply") {
        "apply"
    } else {
        "skip"
    };
    match request_json(
        ctx,
        HttpMethod::Post,
        &format!("/app/import/api/journal-sources/{source}/resolve-facet"),
        vec![],
        Some(json!({"staged_file": staged_file, "mode": mode})),
    ) {
        Ok(_body) if mode == "apply" => {
            stdout_line(format!("Applied staged facet file {staged_file}."))
        }
        Ok(_body) => stdout_line(format!("Skipped staged facet file {staged_file}.")),
        Err(error) => import_error(error, source, false),
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

fn push_param(params: &mut Vec<QueryParam>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        params.push(QueryParam::single(key, value));
    }
}

fn import_error(error: ClientError, source: &str, list_staged: bool) -> CommandOutput {
    match error {
        ClientError::Unreachable { .. } => stderr(SERVICE_DOWN_MESSAGE),
        other if other.reason_code() == Some(JOURNAL_SOURCE_PROBLEM) => stderr(format!(
            "Error: Import source '{source}' {SOURCE_NOT_FOUND_SUFFIX}"
        )),
        other if list_staged && other.reason_code() == Some(INVALID_REQUEST_VALUE) => {
            stderr("Error: Area must be one of: entities, facets, config.")
        }
        other
            if other.reason_code() == Some(IMPORT_NOT_FOUND)
                || other.reason_code() == Some(INVALID_REQUEST_VALUE) =>
        {
            stderr(format!(
                "Error: {}",
                other.detail().unwrap_or_else(|| other.message())
            ))
        }
        other => stderr(other.message()),
    }
}

fn stdout_line(value: impl AsRef<str>) -> CommandOutput {
    CommandOutput::success(format!("{}\n", value.as_ref()))
}

fn stderr(value: impl AsRef<str>) -> CommandOutput {
    CommandOutput::failure(format!("{}\n", value.as_ref()), 1)
}
