// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_cli_copy::{display_for, render_for};

use crate::args::parse_args_for;
use crate::command::{CommandContext, CommandOutput};
use crate::decode::decode_response;
use crate::error::{ClientError, service_down_for};
use crate::json_format::json_compact_ascii;
use crate::pagination::paginate_collection;
use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};

const ENTITY_NOT_FOUND: &str = "entity_not_found";

#[must_use]
pub fn brief(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_args_for(locale, ctx.args, &[], &["--json"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(name) = parsed.positionals.first() else {
        return missing_argument(locale, "call profile brief", "NAME");
    };
    let profile = match request_json(
        ctx,
        &format!("/api/profile/{}/brief", quote_path(name)),
        vec![],
    ) {
        Ok(profile) => profile,
        Err(error) => return profile_error(locale, error, name),
    };
    if parsed.has_flag("--json") {
        return crate::agent_output::literal_output(stdout_compact_json(&profile));
    }
    let profile = crate::agent_output::literal_body(profile);
    stdout(vec![
        display_for(
            locale,
            "journal.cli.sol.profile.entity.id",
            [("value", &field(&profile, "entity_id"))],
        ),
        display_for(
            locale,
            "journal.cli.sol.profile.name",
            [("value", &field(&profile, "name"))],
        ),
        display_for(
            locale,
            "journal.cli.sol.profile.type",
            [("value", &field(&profile, "type"))],
        ),
        display_for(
            locale,
            "journal.cli.sol.profile.description",
            [("value", &field(&profile, "description"))],
        ),
        display_for(
            locale,
            "journal.cli.sol.profile.last.seen",
            [("value", &field(&profile, "last_seen"))],
        ),
    ])
}

#[must_use]
pub fn cadence(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_args_for(locale, ctx.args, &[], &["--include-mentions", "--json"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(name) = parsed.positionals.first() else {
        return missing_argument(locale, "call profile cadence", "NAME");
    };
    let mut params = Vec::new();
    if parsed.has_flag("--include-mentions") {
        params.push(QueryParam::single("include_mentions", "true"));
    }
    let cadence = match request_json(
        ctx,
        &format!("/api/profile/{}/cadence", quote_path(name)),
        params,
    ) {
        Ok(cadence) => cadence,
        Err(error) => return profile_error(locale, error, name),
    };
    if parsed.has_flag("--json") {
        return stdout_compact_json(&cadence);
    }
    stdout(vec![
        display_for(
            locale,
            "journal.cli.sol.profile.interactions",
            [("value", &field(&cadence, "recent_interactions_count_30d"))],
        ),
        display_for(
            locale,
            "journal.cli.sol.profile.last.seen",
            [("value", &field(&cadence, "last_seen"))],
        ),
        display_for(
            locale,
            "journal.cli.sol.profile.interval",
            [("value", &field(&cadence, "avg_interval_days"))],
        ),
        display_for(
            locale,
            "journal.cli.sol.profile.quiet",
            [("value", &field(&cadence, "gone_quiet_since"))],
        ),
    ])
}

#[must_use]
pub fn full(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &["--facets"];
    let locale = ctx.human_locale_with_options(value_options);
    let parsed = match parse_args_for(
        locale,
        ctx.args,
        value_options,
        &["--include-mentions", "--json"],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(name) = parsed.positionals.first() else {
        return missing_argument(locale, "call profile full", "NAME");
    };
    let mut params = Vec::new();
    if let Some(facets) = parsed.value("--facets") {
        params.push(QueryParam::single("facets", facets));
    }
    if parsed.has_flag("--include-mentions") {
        params.push(QueryParam::single("include_mentions", "true"));
    }
    let profile = match request_json(ctx, &format!("/api/profile/{}", quote_path(name)), params) {
        Ok(profile) => profile,
        Err(error) => return profile_error(locale, error, name),
    };
    if parsed.has_flag("--json") {
        return crate::agent_output::literal_output(stdout_compact_json(&profile));
    }
    stdout(render_full(
        locale,
        &crate::agent_output::literal_body(profile),
    ))
}

#[must_use]
pub fn list_active(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = solstone_core_cli_copy::ENGLISH;
    let parsed = match parse_args_for(locale, ctx.args, &["--window-days"], &["--json"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let window_days = parsed.value("--window-days").unwrap_or("30");
    let ids = match paginate_collection(
        ctx.transport,
        "/api/profiles/active",
        vec![QueryParam::single("window_days", window_days)],
        None,
    ) {
        Ok(ids) => ids,
        Err(error) => return profile_list_error(locale, error),
    };
    if parsed.has_flag("--json") {
        return stdout_compact_json(&Value::Array(ids));
    }
    let lines = ids
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        CommandOutput::success(String::new())
    } else {
        stdout(lines)
    }
}

fn request_json(
    ctx: CommandContext<'_>,
    path: &str,
    params: Vec<QueryParam>,
) -> Result<Value, ClientError> {
    let response = ctx.transport.request(ApiRequest {
        method: HttpMethod::Get,
        path: path.to_string(),
        params,
        json: None,
        headers: vec![],
        policy: TimeoutPolicy::Api,
    })?;
    decode_response(&response)
}

fn render_full(locale: &str, profile: &Value) -> Vec<String> {
    let facets_label = profile
        .get("facets")
        .and_then(Value::as_array)
        .map(|items| {
            if items.is_empty() {
                "-".to_string()
            } else {
                items
                    .iter()
                    .map(display_value)
                    .collect::<Vec<_>>()
                    .join(",")
            }
        })
        .unwrap_or_else(|| "-".to_string());
    let cadence = &profile["cadence"];
    // `blocked` and `detached_facets` are reported by the API rather than filtered
    // there (settled 2026-09-03), so this renderer -- a caller -- is where
    // they have to become visible. Dropping them here would expose the status on
    // the wire and hide it from every agent reading the default output.
    let detached_label = profile
        .get("detached_facets")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(display_value)
                .collect::<Vec<_>>()
                .join(",")
        })
        .filter(|label| !label.is_empty());
    let mut lines = vec![
        display_for(
            locale,
            "journal.cli.sol.profile.identity",
            [
                ("name", &field(profile, "name")),
                ("kind", &field(profile, "type")),
                ("facets", &facets_label),
                ("self_value", &field(profile, "is_self")),
                ("hidden", &field(profile, "blocked")),
            ],
        ),
        String::new(),
        render_for(locale, "journal.cli.sol.profile.cadence", &[]),
        display_for(
            locale,
            "journal.cli.sol.profile.full.last.seen",
            [("value", &field(cadence, "last_seen"))],
        ),
        display_for(
            locale,
            "journal.cli.sol.profile.full.interactions",
            [("value", &field(cadence, "recent_interactions_count_30d"))],
        ),
        display_for(
            locale,
            "journal.cli.sol.profile.full.interval",
            [("value", &field(cadence, "avg_interval_days"))],
        ),
        display_for(
            locale,
            "journal.cli.sol.profile.full.quiet",
            [("value", &field(cadence, "gone_quiet_since"))],
        ),
    ];
    if let Some(detached_label) = detached_label {
        lines.insert(
            1,
            display_for(
                locale,
                "journal.cli.sol.profile.detached",
                [("detached_label", &detached_label)],
            ),
        );
    }
    lines
}

fn quote_path(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut output = String::new();
    for byte in value.as_bytes() {
        if matches!(
            byte,
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~'
        ) {
            output.push(char::from(*byte));
        } else {
            output.push('%');
            output.push(char::from(HEX[(byte >> 4) as usize]));
            output.push(char::from(HEX[(byte & 0x0F) as usize]));
        }
    }
    output
}

fn field(item: &Value, name: &str) -> String {
    display_value(&item[name])
}

fn display_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Null => "None".to_string(),
        other => other.to_string(),
    }
}

fn stdout_compact_json(value: &Value) -> CommandOutput {
    CommandOutput::success(format!("{}\n", json_compact_ascii(value)))
}

fn stdout(lines: Vec<String>) -> CommandOutput {
    CommandOutput::success(format!("{}\n", lines.join("\n")))
}

fn stderr(value: impl AsRef<str>) -> CommandOutput {
    CommandOutput::failure(format!("{}\n", value.as_ref()), 1)
}

fn profile_error(locale: &str, error: ClientError, name: &str) -> CommandOutput {
    match error {
        ClientError::Unreachable { .. } => stderr(service_down_for(locale)),
        other if other.reason_code() == Some(ENTITY_NOT_FOUND) || other.status() == Some(404) => {
            stderr(display_for(
                locale,
                "journal.cli.sol.profile.not.found",
                [("name", &name)],
            ))
        }
        other => stderr(
            other
                .detail_for(locale)
                .unwrap_or_else(|| other.message_for(locale)),
        ),
    }
}

fn profile_list_error(locale: &str, error: ClientError) -> CommandOutput {
    match error {
        ClientError::Unreachable { .. } => stderr(service_down_for(locale)),
        other => stderr(
            other
                .detail_for(locale)
                .unwrap_or_else(|| other.message_for(locale)),
        ),
    }
}

fn missing_argument(locale: &str, command: &str, name: &str) -> CommandOutput {
    crate::args::typer_missing_for(
        locale,
        command,
        &format!("{command} [OPTIONS] {name}"),
        crate::args::Missing::Argument(name),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;
    use crate::command::CommandContext;
    use crate::seam::{ExpectedHttpCall, ScriptedHttpTransport};
    use crate::transport::{ApiRequest, HttpMethod, HttpResponse, TimeoutPolicy};

    fn test_ctx<'a>(
        args: &'a [String],
        transport: &'a ScriptedHttpTransport,
    ) -> CommandContext<'a> {
        let env: &'a BTreeMap<String, String> = Box::leak(Box::new(BTreeMap::new()));
        CommandContext {
            args,
            env,
            stdin: "".into(),
            transport,
            clock: None,
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
            link_pairing: None,
            link_serve: None,
            link_status_probe: None,
        }
    }

    #[test]
    fn profile_brief_renders_fields() {
        let transport = ScriptedHttpTransport::new(vec![ExpectedHttpCall::Request {
            expected: ApiRequest {
                method: HttpMethod::Get,
                path: "/api/profile/ada/brief".to_string(),
                params: vec![],
                json: None,
                headers: vec![],
                policy: TimeoutPolicy::Api,
            },
            result: Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: json!({
                    "entity_id": "ada",
                    "name": "Ada",
                    "type": "person",
                    "description": "Mathematician",
                    "last_seen": "20261010"
                })
                .to_string()
                .into_bytes(),
                policy: TimeoutPolicy::Api,
            }),
        }]);

        let args = vec!["ada".to_string()];
        let output = brief(test_ctx(&args, &transport));
        transport.assert_done();

        assert_eq!(output.exit, 0);
        assert_eq!(
            output.stdout,
            "entity_id: ada\nname: Ada\ntype: person\ndescription: Mathematician\nlast_seen: 20261010\n"
        );
    }

    #[test]
    fn profile_full_renders_cadence_and_facets() {
        let profile = json!({
            "name": "Ada",
            "type": "person",
            "facets": ["work"],
            "is_self": false,
            "blocked": false,
            "cadence": {
                "last_seen": "20261010",
                "recent_interactions_count_30d": 2,
                "avg_interval_days": 3.0,
                "gone_quiet_since": null
            }
        });
        let lines = render_full("en-US", &profile);
        assert_eq!(
            lines,
            vec![
                "Ada · person · facets=work · self=False · hidden=False".to_string(),
                String::new(),
                "Cadence:".to_string(),
                "  last_seen: 20261010".to_string(),
                "  recent_interactions_count_30d: 2".to_string(),
                "  avg_interval_days: 3.0".to_string(),
                "  gone_quiet_since: None".to_string(),
            ]
        );
    }
}
