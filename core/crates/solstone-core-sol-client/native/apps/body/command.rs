// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_cli_copy::{display_for, render_for};

use crate::args::parse_args_for;
use crate::command::{CommandContext, CommandOutput};
use crate::decode::decode_response;
use crate::error::ClientError;
use crate::json_format::sorted_json_pretty_ascii;
use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};

#[must_use]
pub fn status(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_args_for(locale, ctx.args, &[], &["--json"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let payload = match request_json(locale, ctx, "/app/body/api/status", vec![]) {
        Ok(payload) => payload,
        Err(error) => return body_error(locale, error),
    };
    if parsed.has_flag("--json") {
        return crate::agent_output::literal_output(stdout_json(&payload));
    }
    let Some(object) = payload.as_object() else {
        return stderr(render_for(
            locale,
            "journal.cli.sol.call.body.unreadable",
            &[],
        ));
    };
    let imports = object.get("imports").and_then(Value::as_object);
    let imported = ["new", "old"]
        .iter()
        .filter_map(|group| imports.and_then(|imports| imports.get(*group)))
        .filter_map(Value::as_array)
        .map(Vec::len)
        .sum::<usize>();
    let days = object.get("days").and_then(Value::as_i64).unwrap_or(0);
    let mut lines = vec![
        display_for(
            locale,
            "journal.cli.sol.call.body.imports",
            [("imported", &imported)],
        ),
        display_for(locale, "journal.cli.sol.call.body.days", [("days", &days)]),
    ];
    if let Some(coverage) = object.get("coverage").and_then(Value::as_object)
        && let Some(start) = coverage.get("start").and_then(Value::as_str)
    {
        let end = coverage.get("end").and_then(Value::as_str).unwrap_or("");
        lines.push(display_for(
            locale,
            "journal.cli.sol.call.body.coverage",
            [("start", &start), ("end", &end)],
        ));
    }
    stdout(lines)
}

#[must_use]
pub fn day(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_args_for(locale, ctx.args, &[], &["--json"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(day_value) = parsed.positionals.first() else {
        return crate::args::typer_missing_for(
            locale,
            "call body day",
            "call body day [OPTIONS] DAY_VALUE",
            crate::args::Missing::Argument("DAY_VALUE"),
        );
    };
    let payload = match request_json(
        locale,
        ctx,
        &format!("/app/body/api/day/{day_value}"),
        vec![],
    ) {
        Ok(payload) => payload,
        Err(error) => return body_error(locale, error),
    };
    if parsed.has_flag("--json") {
        return crate::agent_output::literal_output(stdout_json(&payload));
    }
    let Some(object) = payload.as_object() else {
        return stderr(render_for(
            locale,
            "journal.cli.sol.call.body.unreadable",
            &[],
        ));
    };
    let sources = object
        .get("sources")
        .and_then(Value::as_array)
        .map(|sources| {
            sources
                .iter()
                .filter_map(|source| source.get("name").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let mut lines = vec![
        display_for(
            locale,
            "journal.cli.sol.call.body.day",
            [("day", &string_field(object.get("day"), day_value))],
        ),
        display_for(
            locale,
            "journal.cli.sol.call.body.entries",
            [("count", &display_or_zero(object.get("entry_total")))],
        ),
    ];
    if !sources.is_empty() {
        lines.push(display_for(
            locale,
            "journal.cli.sol.call.body.sources",
            [("sources", &sources)],
        ));
    }
    stdout(lines)
}

#[must_use]
pub fn window(ctx: CommandContext<'_>) -> CommandOutput {
    let options = ["--from", "--to"];
    let locale = ctx.human_locale_with_options(&options);
    let parsed = match parse_args_for(locale, ctx.args, &options, &["--json"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(from_value) = parsed.value("--from") else {
        return crate::args::typer_missing_for(
            locale,
            "call body window",
            "call body window [OPTIONS]",
            crate::args::Missing::Option("--from"),
        );
    };
    let Some(to_value) = parsed.value("--to") else {
        return crate::args::typer_missing_for(
            locale,
            "call body window",
            "call body window [OPTIONS]",
            crate::args::Missing::Option("--to"),
        );
    };
    let payload = match request_json(
        locale,
        ctx,
        "/app/body/api/window",
        vec![
            QueryParam::single("from", from_value),
            QueryParam::single("to", to_value),
        ],
    ) {
        Ok(payload) => payload,
        Err(error) => return body_error(locale, error),
    };
    if parsed.has_flag("--json") {
        return crate::agent_output::literal_output(stdout_json(&payload));
    }
    let Some(object) = payload.as_object() else {
        return stderr(render_for(
            locale,
            "journal.cli.sol.call.body.unreadable",
            &[],
        ));
    };
    let mut lines = vec![
        display_for(
            locale,
            "journal.cli.sol.call.body.range",
            [
                ("start", &display_value(object.get("from"))),
                ("end", &display_value(object.get("to"))),
            ],
        ),
        display_for(
            locale,
            "journal.cli.sol.call.body.entries",
            [("count", &display_or_zero(object.get("entry_total")))],
        ),
    ];
    if let Some(signals) = object.get("signals").and_then(Value::as_array) {
        for signal in signals {
            lines.push(format!(
                "{}: {}",
                display_value(signal.get("label")),
                display_value(signal.get("count_label"))
            ));
        }
    }
    stdout(lines)
}

fn request_json(
    locale: &str,
    ctx: CommandContext<'_>,
    path: &str,
    params: Vec<QueryParam>,
) -> Result<Value, ClientError> {
    let response = ctx.transport.request(ApiRequest {
        method: HttpMethod::Get,
        path: path.to_string(),
        params,
        json: None,
        headers: crate::transport::locale_headers(locale),
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

fn body_error(locale: &str, error: ClientError) -> CommandOutput {
    match error {
        ClientError::Unreachable { .. } => stderr(crate::error::service_down_for(locale)),
        _ => stderr(error.message_for(locale)),
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
