// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{self, Read};
use std::path::Path;

use serde_json::{Map, Value};

use crate::{HostedBinding, get_keys, save_hosted_binding, status_view};

pub const USAGE: &str = "usage: journal backup <command> [options]\n";
const MAX_JSON_STDIN_BYTES: usize = 1024 * 1024; // Keep in lockstep with solstone-core main.rs.

#[derive(Debug, PartialEq, Eq)]
pub struct CliRun {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

pub fn run_cli(args: &[String], journal: &Path) -> CliRun {
    let args = args
        .iter()
        .filter(|arg| !matches!(arg.as_str(), "-v" | "--verbose" | "-d" | "--debug"))
        .cloned()
        .collect::<Vec<_>>();
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        return success(USAGE.to_owned());
    }
    match args.as_slice() {
        [command] if command == "status" => render_json(status_view(journal).map(Value::Object)),
        [destination, command] if destination == "destination" && command == "show" => {
            match status_view(journal) {
                Ok(view) => render_json(Ok(view
                    .get("destination")
                    .cloned()
                    .expect("status has destination"))),
                Err(error) => runtime_error(error.to_string()),
            }
        }
        [destination, command] if destination == "destination" && command == "set-hosted" => {
            set_hosted(journal)
        }
        [key, command] if key == "recovery-key" && command == "show" => recovery_key_show(journal),
        _ => usage_error(&args.join(" ")),
    }
}

fn set_hosted(journal: &Path) -> CliRun {
    let payload = match read_stdin_json() {
        Ok(payload) => payload,
        Err(message) => return runtime_error(message),
    };
    let field = |name: &'static str| -> Result<String, String> {
        payload
            .get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("Missing {name}."))
    };
    let binding = match (
        field("broker_endpoint"),
        field("account_id"),
        field("instance_id"),
        field("bucket"),
        field("prefix"),
        field("broker_token"),
    ) {
        (
            Ok(broker_endpoint),
            Ok(account_id),
            Ok(instance_id),
            Ok(bucket),
            Ok(prefix),
            Ok(broker_token),
        ) => HostedBinding {
            broker_endpoint,
            account_id,
            instance_id,
            bucket,
            prefix,
            broker_token,
        },
        (Err(error), _, _, _, _, _)
        | (_, Err(error), _, _, _, _)
        | (_, _, Err(error), _, _, _)
        | (_, _, _, Err(error), _, _)
        | (_, _, _, _, Err(error), _)
        | (_, _, _, _, _, Err(error)) => return runtime_error(error),
    };
    if let Err(error) = save_hosted_binding(journal, &binding) {
        return runtime_error(error.to_string());
    }
    render_json(Ok(Value::Object(Map::from_iter([
        (
            "broker_endpoint".into(),
            Value::String(binding.broker_endpoint),
        ),
        ("account_id".into(), Value::String(binding.account_id)),
        ("instance_id".into(), Value::String(binding.instance_id)),
        ("bucket".into(), Value::String(binding.bucket)),
        ("prefix".into(), Value::String(binding.prefix)),
        ("bound".into(), Value::Bool(true)),
    ]))))
}

fn recovery_key_show(journal: &Path) -> CliRun {
    let keys = match get_keys(journal) {
        Ok(Some(keys)) => keys,
        Ok(None) => return runtime_error("No recovery key is set.".to_owned()),
        Err(error) => return runtime_error(error.to_string()),
    };
    let display = match crate::format_recovery_key_display(&keys.recovery_key) {
        Ok(display) => display,
        Err(error) => return runtime_error(error.to_string()),
    };
    let groups = display.split(' ').collect::<Vec<_>>();
    success(
        (0..groups.len())
            .step_by(4)
            .map(|index| format!("{}\n", groups[index..index + 4].join(" ")))
            .collect(),
    )
}

fn read_stdin_json() -> Result<Map<String, Value>, String> {
    let mut bytes = Vec::new();
    let result = io::stdin()
        .lock()
        .take((MAX_JSON_STDIN_BYTES + 1) as u64)
        .read_to_end(&mut bytes);
    if result.is_err() {
        return Err("stdin I/O error.".to_owned());
    }
    if bytes.len() > MAX_JSON_STDIN_BYTES {
        return Err("stdin request exceeds the JSON input limit.".to_owned());
    }
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Err("expected JSON object on stdin.".to_owned());
    }
    let value = serde_json::from_slice::<Value>(&bytes)
        .map_err(|error| format!("invalid JSON on stdin: {error}"))?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| "expected JSON object on stdin.".to_owned())
}

fn render_json(result: Result<Value, crate::BackupError>) -> CliRun {
    match result {
        Ok(mut value) => {
            sort_json_keys(&mut value);
            match serde_json::to_string_pretty(&value) {
                Ok(rendered) => success(format!("{rendered}\n")),
                Err(_) => runtime_error("could not render JSON output.".to_owned()),
            }
        }
        Err(error) => runtime_error(error.to_string()),
    }
}
fn sort_json_keys(value: &mut Value) {
    match value {
        Value::Object(object) => {
            let mut fields = std::mem::take(object).into_iter().collect::<Vec<_>>();
            fields.sort_by(|left, right| left.0.cmp(&right.0));
            for (_, value) in &mut fields {
                sort_json_keys(value);
            }
            *object = Map::from_iter(fields);
        }
        Value::Array(values) => {
            for value in values {
                sort_json_keys(value);
            }
        }
        _ => {}
    }
}
fn success(stdout: String) -> CliRun {
    CliRun {
        stdout,
        stderr: String::new(),
        exit_code: 0,
    }
}
fn runtime_error(message: String) -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr: format!("Error: {message}\n"),
        exit_code: 1,
    }
}
fn usage_error(arguments: &str) -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr: format!("{USAGE}journal backup: error: unrecognized arguments: {arguments}\n"),
        exit_code: 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_uses_owned_usage() {
        let output = run_cli(&["wat".into()], Path::new("/unused"));
        assert_eq!(output.exit_code, 2);
        assert_eq!(
            output.stderr,
            "usage: journal backup <command> [options]\njournal backup: error: unrecognized arguments: wat\n"
        );
    }
}
