// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_cli_copy::{Value as CopyValue, display_for, render_for};

use crate::command::{CommandContext, CommandOutput, journal_today};
use crate::decode::decode_response;
use crate::error::{ClientError, service_down_for};
use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};

#[must_use]
pub fn summary(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &["--day"];
    let locale = ctx.human_locale_with_options(value_options);
    let parsed = match parse_args(locale, ctx.args, value_options, &["--json"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let mut params = Vec::new();
    if let Some(day) = parsed.value("--day") {
        params.push(QueryParam::single("day", day));
    }
    let mut report = match request_json(locale, ctx, "/api/health/summary", params) {
        Ok(report) => report,
        Err(error) => return health_error(locale, error),
    };
    prune_retired_consumer_signals(&mut report);
    if parsed.has_flag("--json") {
        stdout_json(&report)
    } else {
        stdout(render_summary(locale, &report))
    }
}

#[must_use]
pub fn full(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &["--day"];
    let locale = ctx.human_locale_with_options(value_options);
    let parsed = match parse_args(locale, ctx.args, value_options, &["--json"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let mut params = Vec::new();
    if let Some(day) = parsed.value("--day") {
        params.push(QueryParam::single("day", day));
    }
    let mut report = match request_json(locale, ctx, "/api/health/full", params) {
        Ok(report) => report,
        Err(error) => return health_error(locale, error),
    };
    prune_retired_consumer_signals(&mut report);
    if parsed.has_flag("--json") {
        stdout_json(&report)
    } else {
        stdout(render_full(locale, &report))
    }
}

#[must_use]
pub fn for_range(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &["--day-from", "--day-to"];
    let locale = ctx.human_locale_with_options(value_options);
    let parsed = match parse_args(locale, ctx.args, value_options, &["--json"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let mut params = Vec::new();
    if let Some(day_from) = parsed.value("--day-from") {
        params.push(QueryParam::single("day_from", day_from));
    }
    if let Some(day_to) = parsed.value("--day-to") {
        params.push(QueryParam::single("day_to", day_to));
    }
    let mut report = match request_json(locale, ctx, "/api/health/range", params) {
        Ok(report) => report,
        Err(error) => return health_error(locale, error),
    };
    prune_retired_consumer_signals(&mut report);
    if parsed.has_flag("--json") {
        stdout_json(&report)
    } else {
        stdout(render_full(locale, &report))
    }
}

#[must_use]
pub fn pipeline(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = solstone_core_cli_copy::ENGLISH;
    let parsed = match parse_args(locale, ctx.args, &["--day"], &["--yesterday"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    if parsed.value("--day").is_some() && parsed.has_flag("--yesterday") {
        return stderr("--day and --yesterday are mutually exclusive");
    }
    let target = if let Some(day) = parsed.value("--day") {
        day.to_string()
    } else {
        let today = match journal_today(ctx) {
            Ok(today) => today,
            Err(error) => return health_error(locale, error),
        };
        if parsed.has_flag("--yesterday") {
            yesterday(&today)
        } else {
            today
        }
    };
    let summary = match request_json(
        locale,
        ctx,
        "/api/health/pipeline",
        vec![QueryParam::single("day", target)],
    ) {
        Ok(summary) => summary,
        Err(error) => return health_error(locale, error),
    };
    stdout_json(&summary)
}

#[derive(Debug, Default)]
struct ParsedArgs {
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
    locale: &str,
    args: &[String],
    options: &[&str],
    flags: &[&str],
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
                return Err(display_for(
                    locale,
                    "journal.cli.client.option.value",
                    [("option", &token)],
                ));
            };
            parsed.values.push((token.clone(), value.clone()));
        } else if flags.contains(&token.as_str()) {
            parsed.flags.push(token.clone());
        } else if token.starts_with('-') {
            return Err(display_for(
                locale,
                "journal.cli.client.option.unknown",
                [("option", &token)],
            ));
        } else {
            return Err(display_for(
                locale,
                "journal.cli.sol.health.argument.unexpected",
                [("token", &token)],
            ));
        }
        index += 1;
    }
    Ok(parsed)
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

fn yesterday(today: &str) -> String {
    solstone_core_time::parse_day(today)
        .and_then(|date| date.yesterday().ok())
        .map_or_else(|| today.to_string(), solstone_core_time::day_key)
}

fn prune_retired_consumer_signals(report: &mut Value) {
    let Some(signals) = report
        .get_mut("consumer_signal")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    signals.remove("ledger_open_items_total");
    signals.remove("ledger_stale_items_count");
}

fn render_summary(locale: &str, report: &Value) -> Vec<String> {
    let capture = &report["capture_health"];
    let synthesis = &report["synthesis_health"];
    let consumer_signal = &report["consumer_signal"];
    let mut lines = Vec::new();
    let range = report["range"]
        .as_array()
        .expect("health range is an array");
    lines.push(display_for(
        locale,
        "journal.cli.sol.health.range",
        [
            ("start", &display_value(&range[0])),
            ("end", &display_value(&range[1])),
        ],
    ));
    lines.push(render_for(locale, "journal.cli.sol.health.intake", &[]));
    lines.push(display_for(
        locale,
        "journal.cli.sol.health.hours.with.capture",
        [("value", &display_value(&capture["hours_with_capture"]))],
    ));
    lines.push(display_for(
        locale,
        "journal.cli.sol.health.hours.total",
        [("value", &display_value(&capture["hours_total"]))],
    ));
    lines.push(display_for(
        locale,
        "journal.cli.sol.health.coverage.ratio",
        [("value", &dash(&capture["coverage_ratio"]))],
    ));
    lines.push(display_for(
        locale,
        "journal.cli.sol.health.facets.with.recent.capture",
        [(
            "value",
            &join_values(&capture["facets_with_recent_capture"]),
        )],
    ));
    lines.push(display_for(
        locale,
        "journal.cli.sol.health.facets.silent.past.day",
        [("value", &join_values(&capture["facets_silent_24h"]))],
    ));
    lines.push(display_for(
        locale,
        "journal.cli.sol.health.last.segment.at",
        [("value", &dash(&capture["last_segment_at"]))],
    ));
    lines.push(render_for(locale, "journal.cli.sol.health.synthesis", &[]));
    lines.push(display_for(
        locale,
        "journal.cli.sol.health.activities.count",
        [("value", &display_value(&synthesis["activities_count"]))],
    ));
    lines.push(display_for(
        locale,
        "journal.cli.sol.health.activities.with.participation",
        [(
            "value",
            &display_value(&synthesis["activities_with_participation"]),
        )],
    ));
    lines.push(display_for(
        locale,
        "journal.cli.sol.health.activities.with.story",
        [("value", &display_value(&synthesis["activities_with_story"]))],
    ));
    lines.push(display_for(
        locale,
        "journal.cli.sol.health.activities.user.edited",
        [(
            "value",
            &display_value(&synthesis["activities_user_edited"]),
        )],
    ));
    lines.push(display_for(
        locale,
        "journal.cli.sol.health.talent.run.failures.past.day",
        [("value", &dash(&synthesis["talent_run_failures_24h"]))],
    ));
    lines.push(display_for(
        locale,
        "journal.cli.sol.health.talent.degraded.outputs.past.day",
        [("value", &dash(&synthesis["talent_degraded_outputs_24h"]))],
    ));
    lines.push(display_for(
        locale,
        "journal.cli.sol.health.index.activity.at",
        [("value", &dash(&synthesis["index_activity_at"]))],
    ));
    if let Some(index) = synthesis.get("search_index").and_then(Value::as_object) {
        lines.push(display_for(
            locale,
            "journal.cli.sol.health.index",
            [
                ("state", &display_value(&index["state"])),
                ("stale", &display_value(&index["stale"])),
                ("missing", &display_value(&index["missing"])),
                ("orphaned", &display_value(&index["orphaned"])),
                ("failed", &display_value(&index["failed"])),
            ],
        ));
    }
    render_backlog(locale, &mut lines, &report["segment_backlog"]);
    lines.push(render_for(locale, "journal.cli.sol.health.consumer", &[]));
    lines.push(display_for(
        locale,
        "journal.cli.sol.health.profile.entities.total",
        [(
            "value",
            &display_value(&consumer_signal["profile_entities_total"]),
        )],
    ));
    if let Some(brain) = report.get("brain_health").and_then(Value::as_object)
        && let Some(lines_value) = brain.get("lines").and_then(Value::as_array)
        && !lines_value.is_empty()
    {
        for line in lines_value {
            lines.push(display_value(line));
        }
    }
    lines.push(render_for(locale, "journal.cli.sol.health.notes", &[]));
    let notes = report["notes"]
        .as_array()
        .expect("health notes is an array");
    if notes.is_empty() {
        lines.push(render_for(locale, "journal.cli.sol.health.none", &[]));
    } else {
        for note in notes {
            lines.push(format!(
                "  [{}] {}: {}",
                display_value(&note["severity"]),
                display_value(&note["category"]),
                display_value(&note["message"])
            ));
        }
    }
    lines
}

fn render_full(locale: &str, report: &Value) -> Vec<String> {
    let mut lines = render_summary(locale, report);
    let silent = report["capture_health"]["facets_silent_24h"]
        .as_array()
        .expect("silent facets is an array");
    if silent.is_empty() {
        return lines;
    }
    lines.push(render_for(locale, "journal.cli.sol.health.silent", &[]));
    let notes = report["notes"]
        .as_array()
        .expect("health notes is an array");
    for facet in silent {
        let facet = display_value(facet);
        let prefix = format!("{facet}:");
        for note in notes {
            let category = display_value(&note["category"]);
            let message = display_value(&note["message"]);
            if category == "capture" && message.starts_with(&prefix) {
                lines.push(format!(
                    "  {facet}: [{}] {message}",
                    display_value(&note["severity"])
                ));
            }
        }
    }
    lines
}

fn render_backlog(locale: &str, lines: &mut Vec<String>, backlog: &Value) {
    let not_thought = backlog["not_thought"].as_i64().unwrap_or(0);
    let not_sensed = backlog["not_sensed"].as_i64().unwrap_or(0);
    let m = backlog["days_with_backlog"].as_i64().unwrap_or(0);
    let errors = truthy(&backlog["errors"]);
    if not_thought == 0 && not_sensed == 0 {
        if errors {
            lines.push(render_for(
                locale,
                "journal.cli.sol.health.backlog.unavailable",
                &[],
            ));
        }
        return;
    }
    let (id, quantities): (&str, &[(&str, &str, i64)]) =
        match (not_thought > 0, not_sensed > 0, errors) {
            (true, false, false) => (
                "journal.cli.sol.health.backlog.thinking.days",
                &[
                    ("thinking", "thinking_quantity", not_thought),
                    ("days", "days_quantity", m),
                ],
            ),
            (true, false, true) => (
                "journal.cli.sol.health.backlog.thinking.days.incomplete",
                &[
                    ("thinking", "thinking_quantity", not_thought),
                    ("days", "days_quantity", m),
                ],
            ),
            (false, true, false) => (
                "journal.cli.sol.health.backlog.media.days",
                &[
                    ("media", "media_quantity", not_sensed),
                    ("days", "days_quantity", m),
                ],
            ),
            (false, true, true) => (
                "journal.cli.sol.health.backlog.media.days.incomplete",
                &[
                    ("media", "media_quantity", not_sensed),
                    ("days", "days_quantity", m),
                ],
            ),
            (true, true, false) => (
                "journal.cli.sol.health.backlog.thinking.media.days",
                &[
                    ("thinking", "thinking_quantity", not_thought),
                    ("media", "media_quantity", not_sensed),
                    ("days", "days_quantity", m),
                ],
            ),
            (true, true, true) => (
                "journal.cli.sol.health.backlog.thinking.media.days.incomplete",
                &[
                    ("thinking", "thinking_quantity", not_thought),
                    ("media", "media_quantity", not_sensed),
                    ("days", "days_quantity", m),
                ],
            ),
            (false, false, false) => (
                "journal.cli.sol.health.backlog.days",
                &[("days", "days_quantity", m)],
            ),
            (false, false, true) => (
                "journal.cli.sol.health.backlog.days.incomplete",
                &[("days", "days_quantity", m)],
            ),
        };
    let values: Vec<_> = quantities.iter().map(|(_, _, n)| n.to_string()).collect();
    let args: Vec<_> = quantities
        .iter()
        .zip(&values)
        .flat_map(|((name, selector, count), text)| {
            [
                (*name, CopyValue::String(text)),
                (
                    *selector,
                    crate::command::count_selector(u64::try_from(*count).unwrap_or(0)),
                ),
            ]
        })
        .collect();
    lines.push(render_for(locale, id, &args));
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_i64() != Some(0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn join_values(value: &Value) -> String {
    value
        .as_array()
        .expect("health list is an array")
        .iter()
        .map(display_value)
        .collect::<Vec<_>>()
        .join(", ")
}

fn dash(value: &Value) -> String {
    if value.is_null() {
        "—".to_string()
    } else {
        display_value(value)
    }
}

fn display_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Null => "None".to_string(),
        _ => value.to_string(),
    }
}

fn stdout_json(value: &Value) -> CommandOutput {
    let pretty = serde_json::to_string_pretty(value).expect("JSON output should serialize");
    stdout(vec![ensure_ascii(&pretty)])
}

fn ensure_ascii(value: &str) -> String {
    let mut output = String::new();
    for ch in value.chars() {
        if ch.is_ascii() {
            output.push(ch);
        } else {
            let codepoint = ch as u32;
            if codepoint <= 0xFFFF {
                output.push_str(&format!("\\u{codepoint:04x}"));
            } else {
                let adjusted = codepoint - 0x1_0000;
                let high = 0xD800 + (adjusted >> 10);
                let low = 0xDC00 + (adjusted & 0x3FF);
                output.push_str(&format!("\\u{high:04x}\\u{low:04x}"));
            }
        }
    }
    output
}

fn stdout(lines: Vec<String>) -> CommandOutput {
    CommandOutput::success(format!("{}\n", lines.join("\n")))
}

fn stderr(value: impl AsRef<str>) -> CommandOutput {
    CommandOutput::failure(format!("{}\n", value.as_ref()), 1)
}

fn health_error(locale: &str, error: ClientError) -> CommandOutput {
    match error {
        ClientError::Unreachable { .. } => stderr(service_down_for(locale)),
        _ => stderr(
            error
                .detail_for(locale)
                .unwrap_or_else(|| error.message_for(locale)),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::command::{CommandContext, CommandOutput};
    use crate::seam::{ExpectedHttpCall, ScriptedHttpTransport};
    use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};

    #[test]
    fn summary_unreachable_renders_service_down_message() {
        let args: Vec<String> = vec![];
        let env = BTreeMap::new();
        let transport = ScriptedHttpTransport::new(vec![ExpectedHttpCall::Request {
            expected: ApiRequest {
                method: HttpMethod::Get,
                path: "/api/health/summary".to_string(),
                params: vec![],
                json: None,
                headers: vec![],
                policy: TimeoutPolicy::Api,
            },
            result: Err(ClientError::unreachable(Some(
                "io: Connection refused".to_string(),
            ))),
        }]);
        let output = summary(CommandContext {
            args: &args,
            env: &env,
            stdin: "".into(),
            transport: &transport,
            clock: None,
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
            link_pairing: None,
            link_serve: None,
            link_status_probe: None,
        });

        assert_eq!(
            output,
            CommandOutput {
                stdout: String::new(),
                stderr: "journal isn't running. start it with 'solstone journal up' and retry.\n"
                    .to_string(),
                exit: 1,
            }
        );
        transport.assert_done();
    }

    #[test]
    fn pipeline_yesterday_is_the_day_before_the_journals_today() {
        let args = vec!["--yesterday".to_string()];
        let env = BTreeMap::new();
        let transport = ScriptedHttpTransport::new(vec![
            crate::command::shell_today_call("20261001"),
            ExpectedHttpCall::Request {
                expected: ApiRequest {
                    method: HttpMethod::Get,
                    path: "/api/health/pipeline".to_string(),
                    params: vec![QueryParam::single("day", "20260930")],
                    json: None,
                    headers: vec![],
                    policy: TimeoutPolicy::Api,
                },
                result: Err(ClientError::unreachable(None)),
            },
        ]);
        let output = pipeline(CommandContext {
            args: &args,
            env: &env,
            stdin: "".into(),
            transport: &transport,
            clock: None,
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
            link_pairing: None,
            link_serve: None,
            link_status_probe: None,
        });
        assert_eq!(output.exit, 1);
        transport.assert_done();
    }

    #[test]
    fn health_error_renderer_preserves_variant_details() {
        assert_eq!(
            health_error("en-US", ClientError::unreachable(Some("io: x".to_string()))),
            CommandOutput {
                stdout: String::new(),
                stderr: "journal isn't running. start it with 'solstone journal up' and retry.\n"
                    .to_string(),
                exit: 1,
            }
        );
        assert_eq!(
            health_error("en-US", ClientError::timeout(Some("D".to_string()))),
            CommandOutput {
                stdout: String::new(),
                stderr: "D\n".to_string(),
                exit: 1,
            }
        );
        assert_eq!(
            health_error(
                "en-US",
                ClientError::ReasonRejected {
                    status: 400,
                    error: "invalid request".to_string(),
                    reason_code: Some("invalid_request_value".to_string()),
                    detail: Some("day must be YYYYMMDD".to_string()),
                    payload: Box::new(serde_json::Value::Null),
                }
            ),
            CommandOutput {
                stdout: String::new(),
                stderr: "day must be YYYYMMDD\n".to_string(),
                exit: 1,
            }
        );
        assert_eq!(
            health_error("en-US", ClientError::MalformedSuccess { status: Some(200) }),
            CommandOutput {
                stdout: String::new(),
                stderr: "the journal response couldn't be read.\n".to_string(),
                exit: 1,
            }
        );
        assert_eq!(
            health_error(
                "en-US",
                ClientError::UnreadableServerError { status: Some(500) }
            ),
            CommandOutput {
                stdout: String::new(),
                stderr: "The journal returned an unreadable error.\n".to_string(),
                exit: 1,
            }
        );
    }

    #[test]
    fn pipeline_report_failed_renders_sanitized_detail() {
        // Native-only coverage: on a reason-coded `health_report_failed` 500,
        // native's pipeline renders the sanitized server detail. The installed
        // Python `pipeline` wrapper deliberately lets the exception surface with
        // empty stderr; that intentional divergence is why this case cannot be a
        // shared Python/native parity vector and lives here instead.
        let args: Vec<String> = vec!["--day".to_string(), "20260723".to_string()];
        let env = BTreeMap::new();
        let transport = ScriptedHttpTransport::new(vec![ExpectedHttpCall::Request {
            expected: ApiRequest {
                method: HttpMethod::Get,
                path: "/api/health/pipeline".to_string(),
                params: vec![QueryParam::single("day", "20260723")],
                json: None,
                headers: vec![],
                policy: TimeoutPolicy::Api,
            },
            result: Err(ClientError::ReasonRejected {
                status: 500,
                error: "your journal health report couldn't be built.".to_string(),
                reason_code: Some("health_report_failed".to_string()),
                detail: Some("health report unavailable".to_string()),
                payload: Box::new(serde_json::Value::Null),
            }),
        }]);
        let output = pipeline(CommandContext {
            args: &args,
            env: &env,
            stdin: "".into(),
            transport: &transport,
            clock: None,
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
            link_pairing: None,
            link_serve: None,
            link_status_probe: None,
        });

        assert_eq!(
            output,
            CommandOutput {
                stdout: String::new(),
                stderr: "health report unavailable\n".to_string(),
                exit: 1,
            }
        );
        transport.assert_done();
    }

    #[test]
    fn render_backlog_formats_various_backlog_states() {
        let mut lines = Vec::new();
        let backlog = serde_json::json!({
            "not_thought": 2,
            "not_sensed": 0,
            "days_with_backlog": 1,
            "errors": []
        });
        render_backlog("en-US", &mut lines, &backlog);
        assert_eq!(lines, vec!["  2 segments awaiting thinking across 1 day"]);

        lines.clear();
        let backlog = serde_json::json!({
            "not_thought": 0,
            "not_sensed": 3,
            "days_with_backlog": 2,
            "errors": []
        });
        render_backlog("en-US", &mut lines, &backlog);
        assert_eq!(
            lines,
            vec!["  3 segments waiting for media processing across 2 days"]
        );

        lines.clear();
        let backlog = serde_json::json!({
            "not_thought": 1,
            "not_sensed": 2,
            "days_with_backlog": 2,
            "errors": []
        });
        render_backlog("en-US", &mut lines, &backlog);
        assert_eq!(
            lines,
            vec![
                "  1 segment awaiting thinking, 2 segments waiting for media processing across 2 days"
            ]
        );
    }

    #[test]
    fn render_backlog_emits_when_only_sense_backlog_exists() {
        let mut lines = Vec::new();
        let backlog = serde_json::json!({
            "not_thought": 0,
            "not_sensed": 5,
            "days_with_backlog": 2,
            "errors": []
        });
        render_backlog("en-US", &mut lines, &backlog);
        assert_eq!(
            lines,
            vec!["  5 segments waiting for media processing across 2 days"]
        );
    }

    #[test]
    fn render_backlog_hedges_both_halves_when_errors_present() {
        let mut lines = Vec::new();
        let backlog = serde_json::json!({
            "not_thought": 3,
            "not_sensed": 4,
            "days_with_backlog": 1,
            "errors": ["some error"]
        });
        render_backlog("en-US", &mut lines, &backlog);
        // Exact, not `contains`: a containment assertion cannot tell which of the
        // two counts the bound actually reaches, which is the defect this covers.
        assert_eq!(
            lines,
            vec![
                "  at least 3 segments awaiting thinking, at least 4 segments waiting for media processing across 1 day (status incomplete)"
            ]
        );
    }
}
