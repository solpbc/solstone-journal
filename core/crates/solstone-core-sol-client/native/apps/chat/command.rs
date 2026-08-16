// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};

use crate::command::{CommandContext, CommandOutput};
use crate::decode::decode_response;
use crate::error::{ClientError, SERVICE_DOWN_MESSAGE};
use crate::json_format::json_compact_ascii;
use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};

#[must_use]
pub fn start(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[
            "--summary",
            "--message",
            "--category",
            "--dedupe",
            "--dedupe-window",
            "--since-ts",
            "--trigger-talent",
        ],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let summary = parsed.value("--summary").unwrap_or_default();
    let message = parsed.value("--message");
    let category = parsed.value("--category").unwrap_or_default();
    let dedupe = parsed.value("--dedupe").unwrap_or_default();
    let dedupe_window = parsed.value("--dedupe-window");
    let since_ts = parsed
        .value("--since-ts")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    let trigger_talent = parsed.value("--trigger-talent").unwrap_or_default();

    if summary.trim().is_empty() {
        return stderr("Error: summary is required");
    }
    if summary.trim().chars().count() > 80 {
        return stderr("Error: summary must be 80 characters or fewer");
    }
    if message.is_some_and(|value| value.trim().chars().count() > 500) {
        return stderr("Error: message must be 500 characters or fewer");
    }
    if dedupe.trim().is_empty() {
        return stderr("Error: dedupe is required");
    }
    if trigger_talent.trim().is_empty() {
        return stderr("Error: trigger_talent is required");
    }
    if since_ts <= 0 {
        return stderr("Error: since_ts must be positive");
    }

    let response = match request_json(
        ctx,
        "/api/chat/start",
        Some(json!({
            "summary": summary,
            "message": message.map(str::to_string),
            "category": category,
            "dedupe": dedupe,
            "dedupe_window": dedupe_window.map(str::to_string),
            "since_ts": since_ts,
            "trigger_talent": trigger_talent,
        })),
    ) {
        Ok(response) => response,
        Err(error) => return chat_error(error),
    };
    stdout_line(json_compact_ascii(&response))
}

#[derive(Debug, Default)]
struct ParsedArgs {
    values: Vec<(String, String)>,
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
}

fn parse_args(args: &[String], options: &[&str]) -> Result<ParsedArgs, String> {
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
    json: Option<Value>,
) -> Result<Value, ClientError> {
    let response = ctx.transport.request(ApiRequest {
        method: HttpMethod::Post,
        path: path.to_string(),
        params: Vec::<QueryParam>::new(),
        json,
        policy: TimeoutPolicy::Api,
        headers: Vec::new(),
    })?;
    decode_response(&response)
}

fn chat_error(error: ClientError) -> CommandOutput {
    match error {
        ClientError::Unreachable { .. } => stderr(SERVICE_DOWN_MESSAGE),
        other => stderr(other.message()),
    }
}

fn stdout_line(value: impl AsRef<str>) -> CommandOutput {
    CommandOutput::success(format!("{}\n", value.as_ref()))
}

fn stderr(value: impl AsRef<str>) -> CommandOutput {
    CommandOutput::failure(format!("{}\n", value.as_ref()), 1)
}
