// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};

use crate::command::{CommandContext, CommandOutput};
use crate::decode::decode_response;
use crate::error::{ClientError, SERVICE_DOWN_MESSAGE};
use crate::json_format::json_pretty_ascii;
use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};

#[must_use]
pub fn set_name(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[("--status", Some("-s"))]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(name) = parsed.positionals.first() else {
        return missing_argument("call sol set-name", "NAME");
    };
    let status = parsed.value("--status").unwrap_or("chosen");
    post_json(
        ctx,
        "/app/thinking/api/set-name",
        json!({"name": name, "status": status}),
    )
}

#[must_use]
pub fn reset(ctx: CommandContext<'_>) -> CommandOutput {
    post_json(ctx, "/app/thinking/api/reset", Value::Null)
}

#[must_use]
pub fn set_owner(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[("--bio", Some("-b"))]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(name) = parsed.positionals.first() else {
        return missing_argument("call sol set-owner", "NAME");
    };
    let bio = parsed
        .value("--bio")
        .map(|value| Value::String(value.to_string()))
        .unwrap_or(Value::Null);
    post_json(
        ctx,
        "/app/thinking/api/set-owner",
        json!({"name": name, "bio": bio}),
    )
}

#[must_use]
pub fn sol_init(ctx: CommandContext<'_>) -> CommandOutput {
    post_json(ctx, "/app/thinking/api/sol-init", Value::Null)
}

#[derive(Debug, Default)]
struct ParsedArgs {
    positionals: Vec<String>,
    values: Vec<(String, String)>,
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

fn parse_args(args: &[String], options: &[(&str, Option<&str>)]) -> Result<ParsedArgs, String> {
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

fn post_json(ctx: CommandContext<'_>, path: &str, body: Value) -> CommandOutput {
    let json = if body.is_null() { None } else { Some(body) };
    let response = match ctx.transport.request(ApiRequest {
        method: HttpMethod::Post,
        path: path.to_string(),
        params: Vec::<QueryParam>::new(),
        json,
        headers: vec![],
        policy: TimeoutPolicy::Api,
    }) {
        Ok(response) => response,
        Err(error) => return sol_error(error),
    };
    match decode_response(&response) {
        Ok(payload) => stdout_json(&payload),
        Err(error) => sol_error(error),
    }
}

fn stdout_json(value: &Value) -> CommandOutput {
    CommandOutput::success(format!("{}\n", json_pretty_ascii(value)))
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

fn sol_error(error: ClientError) -> CommandOutput {
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
