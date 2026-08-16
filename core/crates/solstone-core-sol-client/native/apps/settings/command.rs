// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Number, Value, json};

use crate::command::{CommandContext, CommandOutput};
use crate::decode::decode_response;
use crate::error::{ClientError, SERVICE_DOWN_MESSAGE};
use crate::json_format::json_pretty_ascii;
use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};

const API_KEY_ENV_VARS: &[&str] = &["PLAUD_ACCESS_TOKEN"];
const NO_PROCESSING_OPTIONS_ERROR: &str = "error: provide at least one of \
--mode/--window-start/--window-end/--time-window/--display-powersave";

#[must_use]
pub fn convey_status(ctx: CommandContext<'_>) -> CommandOutput {
    match request_json(
        ctx,
        HttpMethod::Get,
        "/app/settings/api/convey/status",
        None,
    ) {
        Ok(value) => stdout_line(string_value(value.get("status_text"))),
        Err(error) => settings_error(error),
    }
}

#[must_use]
pub fn show(ctx: CommandContext<'_>) -> CommandOutput {
    match get_config(ctx) {
        Ok(config) => {
            let mut summary = Map::new();
            summary.insert(
                "identity".to_string(),
                object_field(&config, "identity").unwrap_or_else(empty_object),
            );
            summary.insert(
                "transcribe".to_string(),
                object_field(&config, "transcribe").unwrap_or_else(empty_object),
            );
            summary.insert(
                "observe".to_string(),
                object_field(&config, "observe").unwrap_or_else(empty_object),
            );
            summary.insert("keys".to_string(), Value::Object(key_status(&config)));
            stdout_json(&Value::Object(summary))
        }
        Err(error) => settings_error(error),
    }
}

#[must_use]
pub fn identity_show(ctx: CommandContext<'_>) -> CommandOutput {
    match get_config(ctx) {
        Ok(config) => stdout_json(&object_field(&config, "identity").unwrap_or_else(empty_object)),
        Err(error) => settings_error(error),
    }
}

#[must_use]
pub fn identity_set(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[
            "--name",
            "--preferred",
            "--bio",
            "--timezone",
            "--pronouns",
            "--add-email",
            "--remove-email",
            "--add-alias",
            "--remove-alias",
        ],
        &[],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let config = match get_config(ctx) {
        Ok(config) => config,
        Err(error) => return settings_error(error),
    };
    let identity = object_field(&config, "identity").unwrap_or_else(empty_object);
    let mut data = Map::new();
    push_string_value(&mut data, "name", parsed.value("--name"));
    push_string_value(&mut data, "preferred", parsed.value("--preferred"));
    push_string_value(&mut data, "bio", parsed.value("--bio"));
    push_string_value(&mut data, "timezone", parsed.value("--timezone"));
    if let Some(pronouns) = parsed.value("--pronouns") {
        match serde_json::from_str::<Value>(pronouns) {
            Ok(value) => {
                data.insert("pronouns".to_string(), value);
            }
            Err(_error) => return stderr("Invalid JSON in pronouns"),
        }
    }
    if parsed.value("--add-email").is_some() || parsed.value("--remove-email").is_some() {
        let mut emails = string_array_field(&identity, "email_addresses");
        if let Some(email) = parsed.value("--add-email")
            && !emails.iter().any(|item| item == email)
        {
            emails.push(email.to_string());
        }
        if let Some(remove) = parsed.value("--remove-email") {
            emails.retain(|email| email != remove);
        }
        data.insert(
            "email_addresses".to_string(),
            Value::Array(emails.into_iter().map(Value::String).collect()),
        );
    }
    if parsed.value("--add-alias").is_some() || parsed.value("--remove-alias").is_some() {
        let mut aliases = string_array_field(&identity, "aliases");
        if let Some(alias) = parsed.value("--add-alias")
            && !aliases.iter().any(|item| item == alias)
        {
            aliases.push(alias.to_string());
        }
        if let Some(remove) = parsed.value("--remove-alias") {
            aliases.retain(|alias| alias != remove);
        }
        data.insert(
            "aliases".to_string(),
            Value::Array(aliases.into_iter().map(Value::String).collect()),
        );
    }
    match post_config(
        ctx,
        "identity",
        Some(Value::Object(data)),
        None,
        Value::Null,
    ) {
        Ok(response) => stdout_json(
            &response
                .get("config")
                .and_then(|config| object_field(config, "identity"))
                .unwrap_or_else(empty_object),
        ),
        Err(error) => settings_error(error),
    }
}

#[must_use]
pub fn keys_show(ctx: CommandContext<'_>) -> CommandOutput {
    match get_config(ctx) {
        Ok(config) => stdout_json(&Value::Object(key_status(&config))),
        Err(error) => settings_error(error),
    }
}

#[must_use]
pub fn keys_set(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(env_var) = parsed.positionals.first() else {
        return stderr("Error: missing argument ENV_VAR");
    };
    let Some(value) = parsed.positionals.get(1) else {
        return stderr("Error: missing argument VALUE");
    };
    if !valid_env_var(env_var) {
        return invalid_env_var(env_var);
    }
    match post_config(
        ctx,
        "env",
        None,
        Some(env_var),
        Value::String(value.clone()),
    ) {
        Ok(response) => {
            let mut payload = Map::new();
            payload.insert("env_var".to_string(), Value::String(env_var.clone()));
            payload.insert("set".to_string(), Value::Bool(true));
            payload.insert(
                "validation".to_string(),
                response
                    .get("key_validation")
                    .and_then(|validation| validation.get("plaud"))
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            stdout_json(&Value::Object(payload))
        }
        Err(error) => settings_error(error),
    }
}

#[must_use]
pub fn keys_clear(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(env_var) = parsed.positionals.first() else {
        return stderr("Error: missing argument ENV_VAR");
    };
    if !valid_env_var(env_var) {
        return invalid_env_var(env_var);
    }
    match post_config(
        ctx,
        "env",
        None,
        Some(env_var),
        Value::String(String::new()),
    ) {
        Ok(_response) => {
            let mut payload = Map::new();
            payload.insert("env_var".to_string(), Value::String(env_var.clone()));
            payload.insert("cleared".to_string(), Value::Bool(true));
            stdout_json(&Value::Object(payload))
        }
        Err(error) => settings_error(error),
    }
}

#[must_use]
pub fn keys_validate(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[("--cache-result", None)]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let method = if parsed.bool_value("--cache-result").unwrap_or(false) {
        HttpMethod::Post
    } else {
        HttpMethod::Get
    };
    match request_json(ctx, method, "/app/settings/api/validate-keys", None) {
        Ok(response) => {
            let mut payload = Map::new();
            payload.insert(
                "key_validation".to_string(),
                object_field(&response, "key_validation").unwrap_or_else(empty_object),
            );
            stdout_json(&Value::Object(payload))
        }
        Err(error) => settings_error(error),
    }
}

#[must_use]
pub fn observer_show(ctx: CommandContext<'_>) -> CommandOutput {
    match request_json(ctx, HttpMethod::Get, "/app/settings/api/observe", None) {
        Ok(value) => stdout_json(&value),
        Err(error) => settings_error(error),
    }
}

#[must_use]
pub fn observer_set(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &["--capture-interval"],
        &[("--enabled", Some("--no-enabled"))],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let current = match request_json(ctx, HttpMethod::Get, "/app/settings/api/observe", None) {
        Ok(current) => current,
        Err(error) => return settings_error(error),
    };
    let defaults = current
        .get("defaults")
        .and_then(|defaults| defaults.get("tmux"))
        .cloned()
        .unwrap_or_else(empty_object);
    let mut tmux = Map::new();
    if let Some(capture_interval) = parsed.value("--capture-interval") {
        let Ok(capture_interval_value) = capture_interval.parse::<i64>() else {
            return stderr("Error: option --capture-interval requires an integer.");
        };
        let min_value = int_field(&defaults, "capture_interval_min").unwrap_or(1);
        let max_value = int_field(&defaults, "capture_interval_max").unwrap_or(60);
        if capture_interval_value < min_value || capture_interval_value > max_value {
            return stderr(format!(
                "tmux.capture_interval must be an integer between {min_value} and {max_value}"
            ));
        }
        tmux.insert(
            "capture_interval".to_string(),
            Value::Number(Number::from(capture_interval_value)),
        );
    }
    if let Some(enabled) = parsed.bool_value("--enabled") {
        tmux.insert("enabled".to_string(), Value::Bool(enabled));
    }
    match request_json(
        ctx,
        HttpMethod::Post,
        "/app/settings/api/observe",
        Some(json!({"tmux": Value::Object(tmux)})),
    ) {
        Ok(response) => stdout_json(&object_field(&response, "tmux").unwrap_or_else(empty_object)),
        Err(error) => settings_error(error),
    }
}

#[must_use]
pub fn processing_show(ctx: CommandContext<'_>) -> CommandOutput {
    match request_json(ctx, HttpMethod::Get, "/app/settings/api/processing", None) {
        Ok(value) => stdout_json(&value),
        Err(error) => settings_error(error),
    }
}

#[must_use]
pub fn processing_set(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &["--mode", "--window-start", "--window-end"],
        &[
            ("--time-window", Some("--no-time-window")),
            ("--display-powersave", Some("--no-display-powersave")),
        ],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let mut data = Map::new();
    push_string_value(&mut data, "mode", parsed.value("--mode"));
    let mut time_window = Map::new();
    push_string_value(&mut time_window, "start", parsed.value("--window-start"));
    push_string_value(&mut time_window, "end", parsed.value("--window-end"));
    if let Some(value) = parsed.bool_value("--time-window") {
        time_window.insert("enabled".to_string(), Value::Bool(value));
    }
    let mut gate = Map::new();
    if !time_window.is_empty() {
        gate.insert("time_window".to_string(), Value::Object(time_window));
    }
    if let Some(value) = parsed.bool_value("--display-powersave") {
        gate.insert("display_powersave".to_string(), json!({"enabled": value}));
    }
    if !gate.is_empty() {
        data.insert("gate".to_string(), Value::Object(gate));
    }
    if data.is_empty() {
        return stderr(NO_PROCESSING_OPTIONS_ERROR);
    }
    match post_config(
        ctx,
        "processing",
        Some(Value::Object(data)),
        None,
        Value::Null,
    ) {
        Ok(response) => stdout_json(
            &response
                .get("config")
                .and_then(|config| object_field(config, "processing"))
                .unwrap_or_else(empty_object),
        ),
        Err(error) if error.reason_code() == Some("invalid_config_value") => {
            if let Some(detail) = error.detail() {
                stderr(detail)
            } else {
                settings_error(error)
            }
        }
        Err(error) => settings_error(error),
    }
}

#[must_use]
pub fn transcribe_show(ctx: CommandContext<'_>) -> CommandOutput {
    match request_json(ctx, HttpMethod::Get, "/app/settings/api/transcribe", None) {
        Ok(response) => {
            let mut payload = Map::new();
            payload.insert(
                "backends".to_string(),
                response
                    .get("backends")
                    .cloned()
                    .unwrap_or_else(empty_array),
            );
            payload.insert(
                "api_keys".to_string(),
                object_field(&response, "api_keys").unwrap_or_else(empty_object),
            );
            payload.insert(
                "config".to_string(),
                object_field(&response, "config").unwrap_or_else(empty_object),
            );
            stdout_json(&Value::Object(payload))
        }
        Err(error) => settings_error(error),
    }
}

#[must_use]
pub fn transcribe_set_backend(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(backend) = parsed.positionals.first() else {
        return stderr("Error: missing argument BACKEND");
    };
    let response = match request_json(ctx, HttpMethod::Get, "/app/settings/api/transcribe", None) {
        Ok(response) => response,
        Err(error) => return settings_error(error),
    };
    let mut valid = response
        .get("backends")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("name").and_then(Value::as_str))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    valid.sort();
    if !valid.iter().any(|item| item == backend) {
        return stderr(format!(
            "Invalid backend: {backend}. Must be one of: {}",
            valid.join(", ")
        ));
    }
    match post_config(
        ctx,
        "transcribe",
        Some(json!({"backend": backend})),
        None,
        Value::Null,
    ) {
        Ok(response) => stdout_json(
            &response
                .get("config")
                .and_then(|config| object_field(config, "transcribe"))
                .unwrap_or_else(empty_object),
        ),
        Err(error) => settings_error(error),
    }
}

#[derive(Debug, Default)]
struct ParsedArgs {
    positionals: Vec<String>,
    values: Vec<(String, String)>,
    bools: Vec<(String, bool)>,
}

impl ParsedArgs {
    fn value(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .rev()
            .find(|(key, _value)| key == name)
            .map(|(_key, value)| value.as_str())
    }

    fn bool_value(&self, name: &str) -> Option<bool> {
        self.bools
            .iter()
            .rev()
            .find(|(key, _value)| key == name)
            .map(|(_key, value)| *value)
    }
}

fn parse_args(
    args: &[String],
    options: &[&str],
    bool_options: &[(&str, Option<&str>)],
) -> Result<ParsedArgs, String> {
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
        } else if let Some((canonical, value)) = bool_option_value(token, bool_options) {
            parsed.bools.push((canonical.to_string(), value));
        } else if token.starts_with('-') {
            return Err(format!("Error: unknown option {token}."));
        } else {
            parsed.positionals.push(token.clone());
        }
        index += 1;
    }
    Ok(parsed)
}

fn bool_option_value<'a>(
    token: &str,
    options: &'a [(&'a str, Option<&'a str>)],
) -> Option<(&'a str, bool)> {
    options.iter().find_map(|(positive, negative)| {
        if token == *positive {
            Some((*positive, true))
        } else if negative.is_some_and(|negative| token == negative) {
            Some((*positive, false))
        } else {
            None
        }
    })
}

fn get_config(ctx: CommandContext<'_>) -> Result<Value, ClientError> {
    request_json(ctx, HttpMethod::Get, "/app/settings/api/config", None)
}

fn post_config(
    ctx: CommandContext<'_>,
    section: &str,
    data: Option<Value>,
    key: Option<&str>,
    value: Value,
) -> Result<Value, ClientError> {
    let mut body = Map::new();
    body.insert("section".to_string(), Value::String(section.to_string()));
    if let Some(data) = data {
        body.insert("data".to_string(), data);
    }
    if let Some(key) = key {
        body.insert("key".to_string(), Value::String(key.to_string()));
        body.insert("value".to_string(), value);
    }
    request_json(
        ctx,
        HttpMethod::Post,
        "/app/settings/api/config",
        Some(Value::Object(body)),
    )
}

fn request_json(
    ctx: CommandContext<'_>,
    method: HttpMethod,
    path: &str,
    json: Option<Value>,
) -> Result<Value, ClientError> {
    let response = ctx.transport.request(ApiRequest {
        method,
        path: path.to_string(),
        params: Vec::<QueryParam>::new(),
        json,
        headers: vec![],
        policy: TimeoutPolicy::Api,
    })?;
    decode_response(&response)
}

fn settings_error(error: ClientError) -> CommandOutput {
    match error {
        ClientError::Unreachable { .. } => stderr(SERVICE_DOWN_MESSAGE),
        _ => stderr(error.message()),
    }
}

fn invalid_env_var(env_var: &str) -> CommandOutput {
    stderr(format!(
        "Invalid env var: {env_var}. Must be one of: {}",
        API_KEY_ENV_VARS.join(", ")
    ))
}

fn valid_env_var(env_var: &str) -> bool {
    API_KEY_ENV_VARS.contains(&env_var)
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

fn push_string_value(object: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        object.insert(key.to_string(), Value::String(value.to_string()));
    }
}

fn key_status(config: &Value) -> Map<String, Value> {
    let env_config = config.get("env").and_then(Value::as_object);
    let mut output = Map::new();
    for key in API_KEY_ENV_VARS {
        let value = env_config
            .and_then(|env_config| env_config.get(*key))
            .is_some_and(python_truthy);
        output.insert((*key).to_string(), Value::Bool(value));
    }
    output
}

fn object_field(value: &Value, key: &str) -> Option<Value> {
    value
        .get(key)
        .and_then(Value::as_object)
        .map(|object| Value::Object(object.clone()))
}

fn string_array_field(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn int_field(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(Value::as_i64)
}

fn string_value(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or("").to_string()
}

fn empty_object() -> Value {
    Value::Object(Map::new())
}

fn empty_array() -> Value {
    Value::Array(Vec::new())
}

fn python_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_i64().is_none_or(|value| value != 0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}
