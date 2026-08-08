// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value, json};

use crate::command::{CommandContext, CommandOutput};
use crate::decode::decode_response;
use crate::error::{ClientError, SERVICE_DOWN_MESSAGE};
use crate::json_format::json_pretty_ascii;
use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};

#[must_use]
pub fn agents(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse(ctx.args, &["--day", "-d", "--segment", "-s"], &[]) {
        Ok(value) => value,
        Err(error) => return stderr(error),
    };
    let mut query = query_values(
        &parsed,
        &[
            ("--day", "day"),
            ("-d", "day"),
            ("--segment", "segment"),
            ("-s", "segment"),
        ],
    );
    if let Some(day) = parsed.positionals.first() {
        query.push(QueryParam::single("day", day));
    }
    get(ctx, "/app/search/api/agents", query)
}

#[must_use]
pub fn facet_create(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse(
        ctx.args,
        &["--emoji", "--color", "--description", "--icon"],
        &["--consent"],
    ) {
        Ok(value) => value,
        Err(error) => return stderr(error),
    };
    let Some(title) = parsed.positionals.first() else {
        return stderr("Error: missing argument TITLE");
    };
    post(
        ctx,
        HttpMethod::Post,
        "/app/settings/api/facet",
        Some(
            json!({"title": title, "emoji": parsed.value("--emoji").unwrap_or("📦"), "color": parsed.value("--color").unwrap_or("#667eea"), "description": parsed.value("--description").unwrap_or(""), "icon": parsed.value("--icon").unwrap_or(""), "consent": parsed.flag("--consent")}),
        ),
    )
}

#[must_use]
pub fn facet_delete(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse(ctx.args, &[], &["--yes", "--consent"]) {
        Ok(value) => value,
        Err(error) => return stderr(error),
    };
    let Some(name) = parsed.positionals.first() else {
        return stderr("Error: missing argument NAME");
    };
    if !parsed.flag("--yes") {
        return stderr("Error: --yes is required to delete a facet.");
    }
    post(
        ctx,
        HttpMethod::Delete,
        &format!("/app/settings/api/facet/{name}"),
        Some(json!({"consent": true})),
    )
}

#[must_use]
pub fn facet_mute(ctx: CommandContext<'_>) -> CommandOutput {
    facet_muted(ctx, true)
}
#[must_use]
pub fn facet_unmute(ctx: CommandContext<'_>) -> CommandOutput {
    facet_muted(ctx, false)
}

#[must_use]
pub fn facet_rename(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse(ctx.args, &[], &["--consent"]) {
        Ok(value) => value,
        Err(error) => return stderr(error),
    };
    let (Some(name), Some(new_name)) = (parsed.positionals.first(), parsed.positionals.get(1))
    else {
        return stderr("Error: NAME and NEW_NAME are required");
    };
    post(
        ctx,
        HttpMethod::Post,
        &format!("/app/settings/api/facet/{name}/rename"),
        Some(json!({"new_name": new_name, "consent": parsed.flag("--consent")})),
    )
}

#[must_use]
pub fn facet_show(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse(ctx.args, &[], &[]) {
        Ok(value) => value,
        Err(error) => return stderr(error),
    };
    let Some(name) = parsed.positionals.first() else {
        return stderr("Error: missing argument NAME");
    };
    get(ctx, &format!("/app/settings/api/facet/{name}"), vec![])
}

#[must_use]
pub fn facet_update(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse(
        ctx.args,
        &["--title", "--description", "--emoji", "--color", "--icon"],
        &[],
    ) {
        Ok(value) => value,
        Err(error) => return stderr(error),
    };
    let Some(name) = parsed.positionals.first() else {
        return stderr("Error: missing argument NAME");
    };
    let mut body = Map::new();
    for (option, key) in [
        ("--title", "title"),
        ("--description", "description"),
        ("--emoji", "emoji"),
        ("--color", "color"),
        ("--icon", "icon"),
    ] {
        if let Some(value) = parsed.value(option) {
            body.insert(key.to_string(), Value::String(value.to_string()));
        }
    }
    if body.is_empty() {
        return stderr("Error: provide at least one field to update.");
    }
    post(
        ctx,
        HttpMethod::Put,
        &format!("/app/settings/api/facet/{name}"),
        Some(Value::Object(body)),
    )
}

#[must_use]
pub fn facets(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse(ctx.args, &[], &["--all"]) {
        Ok(value) => value,
        Err(error) => return stderr(error),
    };
    let query = if parsed.flag("--all") {
        vec![QueryParam::single("all", "true")]
    } else {
        vec![]
    };
    get(ctx, "/app/settings/api/facets", query)
}

#[must_use]
pub fn import(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse(ctx.args, &[], &[]) {
        Ok(value) => value,
        Err(error) => return stderr(error),
    };
    let Some(id) = parsed.positionals.first() else {
        return stderr("Error: missing argument ID");
    };
    get(ctx, &format!("/app/import/api/{id}"), vec![])
}

#[must_use]
pub fn imports(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse(ctx.args, &["--limit", "-n", "--source", "-s"], &["--json"]) {
        Ok(value) => value,
        Err(error) => return stderr(error),
    };
    get(
        ctx,
        "/app/import/api/list",
        query_values(
            &parsed,
            &[
                ("--limit", "per_page"),
                ("-n", "per_page"),
                ("--source", "source"),
                ("-s", "source"),
            ],
        ),
    )
}

#[must_use]
pub fn news(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse(
        ctx.args,
        &["--facet", "-f", "--day", "-d", "--limit", "-n", "--cursor"],
        &[],
    ) {
        Ok(value) => value,
        Err(error) => return stderr(error),
    };
    let facet = parsed
        .value("--facet")
        .or_else(|| parsed.value("-f"))
        .or_else(|| parsed.positionals.first().map(String::as_str));
    let Some(facet) = facet else {
        return stderr("Error: facet is required");
    };
    get(
        ctx,
        &format!("/app/news/api/facet/{facet}"),
        query_values(
            &parsed,
            &[
                ("--day", "day"),
                ("-d", "day"),
                ("--limit", "limit"),
                ("-n", "limit"),
                ("--cursor", "cursor"),
            ],
        ),
    )
}

#[must_use]
pub fn read(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse(
        ctx.args,
        &["--day", "-d", "--segment", "-s", "--max", "--path"],
        &[],
    ) {
        Ok(value) => value,
        Err(error) => return stderr(error),
    };
    let mut query = query_values(
        &parsed,
        &[
            ("--day", "day"),
            ("-d", "day"),
            ("--segment", "segment"),
            ("-s", "segment"),
            ("--max", "max_bytes"),
            ("--path", "path"),
        ],
    );
    if let Some(agent) = parsed.positionals.first() {
        query.push(QueryParam::single("agent", agent));
    }
    get(ctx, "/app/search/api/read", query)
}

#[must_use]
pub fn retention_config(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse(ctx.args, &["--mode", "--days", "--stream"], &["--clear"]) {
        Ok(value) => value,
        Err(error) => return stderr(error),
    };
    if parsed.values.is_empty() && !parsed.flag("--clear") {
        return get(ctx, "/app/settings/api/storage", vec![]);
    }
    let mut body = Map::new();
    if parsed.flag("--clear") {
        body.insert("per_stream".to_string(), json!({}));
    }
    let mode = parsed.value("--mode");
    let days = parsed.value("--days");
    if let Some(stream) = parsed.value("--stream") {
        body.insert("per_stream".to_string(), json!({stream: {"raw_media": mode, "raw_media_days": days.and_then(|value| value.parse::<i64>().ok())}}));
    } else {
        if let Some(value) = mode {
            body.insert("raw_media".to_string(), Value::String(value.to_string()));
        }
        if let Some(value) = days.and_then(|value| value.parse::<i64>().ok()) {
            body.insert("raw_media_days".to_string(), Value::from(value));
        }
    }
    post(
        ctx,
        HttpMethod::Put,
        "/app/settings/api/storage",
        Some(Value::Object(body)),
    )
}

#[must_use]
pub fn retention_list(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse(ctx.args, &["--stream"], &[]) {
        Ok(value) => value,
        Err(error) => return stderr(error),
    };
    post(
        ctx,
        HttpMethod::Post,
        "/app/settings/api/storage/purge",
        Some(json!({"stream_filter": parsed.value("--stream")})),
    )
}

#[must_use]
pub fn search(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse(
        ctx.args,
        &[
            "--query",
            "-q",
            "--limit",
            "-n",
            "--offset",
            "--day",
            "-d",
            "--day-from",
            "--day-to",
            "--facet",
            "-f",
            "--agent",
            "-a",
            "--stream",
            "--time-bucket",
        ],
        &["--json"],
    ) {
        Ok(value) => value,
        Err(error) => return stderr(error),
    };
    let mut query = query_values(
        &parsed,
        &[
            ("--query", "q"),
            ("-q", "q"),
            ("--limit", "limit"),
            ("-n", "limit"),
            ("--offset", "offset"),
            ("--day", "day"),
            ("-d", "day"),
            ("--day-from", "day_from"),
            ("--day-to", "day_to"),
            ("--facet", "facet"),
            ("-f", "facet"),
            ("--agent", "agent"),
            ("-a", "agent"),
            ("--stream", "stream"),
            ("--time-bucket", "time_bucket"),
        ],
    );
    if !query.iter().any(|item| item.key == "q")
        && let Some(value) = parsed.positionals.first()
    {
        query.push(QueryParam::single("q", value));
    }
    get(ctx, "/app/search/api/search", query)
}

#[must_use]
pub fn storage_summary(ctx: CommandContext<'_>) -> CommandOutput {
    get(ctx, "/app/settings/api/storage", vec![])
}

fn facet_muted(ctx: CommandContext<'_>, muted: bool) -> CommandOutput {
    let parsed = match parse(ctx.args, &[], &[]) {
        Ok(value) => value,
        Err(error) => return stderr(error),
    };
    let Some(name) = parsed.positionals.first() else {
        return stderr("Error: missing argument NAME");
    };
    post(
        ctx,
        HttpMethod::Put,
        &format!("/app/settings/api/facet/{name}"),
        Some(json!({"muted": muted})),
    )
}

#[derive(Default)]
struct Parsed {
    values: Vec<(String, String)>,
    flags: Vec<String>,
    positionals: Vec<String>,
}
impl Parsed {
    fn value(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
    fn flag(&self, name: &str) -> bool {
        self.flags.iter().any(|value| value == name)
    }
}
fn parse(args: &[String], options: &[&str], flags: &[&str]) -> Result<Parsed, String> {
    let mut parsed = Parsed::default();
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
fn query_values(parsed: &Parsed, values: &[(&str, &str)]) -> Vec<QueryParam> {
    values
        .iter()
        .filter_map(|(option, key)| {
            parsed
                .value(option)
                .map(|value| QueryParam::single(*key, value))
        })
        .collect()
}
fn get(ctx: CommandContext<'_>, path: &str, params: Vec<QueryParam>) -> CommandOutput {
    request(ctx, HttpMethod::Get, path, params, None)
}
fn post(
    ctx: CommandContext<'_>,
    method: HttpMethod,
    path: &str,
    json: Option<Value>,
) -> CommandOutput {
    request(ctx, method, path, vec![], json)
}
fn request(
    ctx: CommandContext<'_>,
    method: HttpMethod,
    path: &str,
    params: Vec<QueryParam>,
    json: Option<Value>,
) -> CommandOutput {
    match ctx
        .transport
        .request(ApiRequest {
            method,
            path: path.to_string(),
            params,
            json,
            headers: vec![],
            policy: TimeoutPolicy::Api,
        })
        .and_then(|response| decode_response(&response))
    {
        Ok(value) => CommandOutput::success(format!("{}\n", json_pretty_ascii(&value))),
        Err(ClientError::Unreachable { .. }) => stderr(SERVICE_DOWN_MESSAGE),
        Err(error) => stderr(error.message()),
    }
}
fn stderr(value: impl AsRef<str>) -> CommandOutput {
    CommandOutput::failure(format!("{}\n", value.as_ref()), 1)
}
