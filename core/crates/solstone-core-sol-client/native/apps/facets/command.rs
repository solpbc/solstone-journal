// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};
use solstone_core_cli_copy::{display_for, render_for};

use crate::command::{CommandContext, CommandOutput};
use crate::decode::decode_response;
use crate::error::ClientError;
use crate::json_format::json_pretty_utf8;
use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};

#[must_use]
pub fn list_candidates(ctx: CommandContext<'_>) -> CommandOutput {
    let (parsed, locale) = match parse_args(ctx, &[("--status", None)], &["--json"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let payload = match request_json(
        locale,
        ctx,
        HttpMethod::Get,
        "/app/curation/api/facet/candidates",
        vec![],
        None,
    ) {
        Ok(payload) => payload,
        Err(error) => return facets_error(locale, error),
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
        return crate::agent_output::literal_output(stdout_json(&Value::Array(rows)));
    }
    if rows.is_empty() {
        return stdout_line(render_for(locale, "journal.cli.sol.call.facets.empty", &[]));
    }
    let mut lines = Vec::new();
    for row in &rows {
        lines.push(display_for(
            locale,
            "journal.cli.sol.call.facets.candidate",
            [
                (
                    "name",
                    &crate::agent_output::literal_links(&display_or(row.get("name"), "")),
                ),
                ("status", &display_or(row.get("status"), "")),
                ("count", &display_value(row.get("count"))),
                ("day", &display_or(row.get("last_surfaced"), "")),
            ],
        ));
    }
    stdout(lines)
}

#[must_use]
pub fn accept(ctx: CommandContext<'_>) -> CommandOutput {
    let (parsed, locale) = match parse_args(ctx, &[], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(name_key) = parsed.positionals.first() else {
        return crate::args::typer_missing_for(
            locale,
            "call facets accept",
            "call facets accept [OPTIONS] NAME_KEY",
            crate::args::Missing::Argument("NAME_KEY"),
        );
    };
    let result = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/curation/api/facet/accept",
        vec![],
        Some(json!({"name_key": name_key})),
    ) {
        Ok(result) => result,
        Err(error) => return facets_error(locale, error),
    };
    stdout_line(render_result(locale, &result, "accept", name_key))
}

#[must_use]
pub fn dismiss(ctx: CommandContext<'_>) -> CommandOutput {
    let (parsed, locale) = match parse_args(ctx, &[], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(name_key) = parsed.positionals.first() else {
        return crate::args::typer_missing_for(
            locale,
            "call facets dismiss",
            "call facets dismiss [OPTIONS] NAME_KEY",
            crate::args::Missing::Argument("NAME_KEY"),
        );
    };
    let result = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/curation/api/facet/dismiss",
        vec![],
        Some(json!({"name_key": name_key})),
    ) {
        Ok(result) => result,
        Err(error) => return facets_error(locale, error),
    };
    stdout_line(render_result(locale, &result, "dismiss", name_key))
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
    ctx: CommandContext<'_>,
    options: &[(&str, Option<&str>)],
    flags: &[&str],
) -> Result<(ParsedArgs, &'static str), String> {
    let args = ctx.args;
    let locale = ctx.human_locale_matching(|option| canonical_option(option, options).is_some());
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
                return Err(crate::args::option_argument_for(locale, token));
            };
            parsed
                .values
                .push((canonical.to_string(), value.to_string()));
        } else if flags.contains(&token.as_str()) {
            parsed.flags.push(token.clone());
        } else if token.starts_with('-') {
            return Err(crate::args::unknown_option_for(locale, token));
        } else {
            parsed.positionals.push(token.clone());
        }
        index += 1;
    }
    Ok((parsed, locale))
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
    locale: &str,
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
        headers: crate::transport::locale_headers(locale),
        policy: TimeoutPolicy::Api,
    })?;
    decode_response(&response)
}

fn render_result(locale: &str, result: &Value, action: &str, name_key: &str) -> String {
    let status = result.get("status").and_then(Value::as_str);
    match status {
        Some("accepted") => display_for(
            locale,
            "journal.cli.sol.call.facets.accepted",
            [
                ("name_key", &name_key),
                ("facet", &display_value(result.get("facet_slug"))),
            ],
        ),
        Some("dismissed") => display_for(
            locale,
            "journal.cli.sol.call.facets.dismissed",
            [("name_key", &name_key)],
        ),
        Some("already_accepted") => display_for(
            locale,
            "journal.cli.sol.call.facets.already.accepted",
            [("name_key", &name_key)],
        ),
        Some("already_dismissed") => display_for(
            locale,
            "journal.cli.sol.call.facets.already.dismissed",
            [("name_key", &name_key)],
        ),
        other => display_for(
            locale,
            "journal.cli.sol.call.facets.action",
            [
                ("action", &action),
                ("name_key", &name_key),
                ("result", &other.unwrap_or("None")),
            ],
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

fn facets_error(locale: &str, error: ClientError) -> CommandOutput {
    match error {
        ClientError::Unreachable { .. } => stderr(crate::error::service_down_for(locale)),
        other => stderr(other.message_for(locale)),
    }
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
