// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::{Duration, NaiveDate};
use serde_json::Value;

use crate::command::{CommandContext, CommandOutput};
use crate::decode::decode_response;
use crate::error::{ClientError, SERVICE_DOWN_MESSAGE};
use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};

#[must_use]
pub fn summary(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &["--day"], &["--json"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let mut params = Vec::new();
    if let Some(day) = parsed.value("--day") {
        params.push(QueryParam::single("day", day));
    }
    let report = match request_json(ctx, "/api/health/summary", params) {
        Ok(report) => report,
        Err(error) => return health_error(error),
    };
    if parsed.has_flag("--json") {
        stdout_json(&report)
    } else {
        stdout(render_summary(&report))
    }
}

#[must_use]
pub fn full(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &["--day"], &["--json"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let mut params = Vec::new();
    if let Some(day) = parsed.value("--day") {
        params.push(QueryParam::single("day", day));
    }
    let report = match request_json(ctx, "/api/health/full", params) {
        Ok(report) => report,
        Err(error) => return health_error(error),
    };
    if parsed.has_flag("--json") {
        stdout_json(&report)
    } else {
        stdout(render_full(&report))
    }
}

#[must_use]
pub fn for_range(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &["--day-from", "--day-to"], &["--json"]) {
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
    let report = match request_json(ctx, "/api/health/range", params) {
        Ok(report) => report,
        Err(error) => return health_error(error),
    };
    if parsed.has_flag("--json") {
        stdout_json(&report)
    } else {
        stdout(render_full(&report))
    }
}

#[must_use]
pub fn pipeline(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &["--day"], &["--yesterday"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    if parsed.value("--day").is_some() && parsed.has_flag("--yesterday") {
        return stderr("--day and --yesterday are mutually exclusive");
    }
    let target = if let Some(day) = parsed.value("--day") {
        day.to_string()
    } else if parsed.has_flag("--yesterday") {
        yesterday(ctx.today)
    } else {
        ctx.today.to_string()
    };
    let summary = match request_json(
        ctx,
        "/api/health/pipeline",
        vec![QueryParam::single("day", target)],
    ) {
        Ok(summary) => summary,
        Err(error) => return health_error(error),
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
            return Err(format!("Error: unexpected argument {token}."));
        }
        index += 1;
    }
    Ok(parsed)
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

fn yesterday(today: &str) -> String {
    NaiveDate::parse_from_str(today, "%Y%m%d")
        .map(|date| (date - Duration::days(1)).format("%Y%m%d").to_string())
        .unwrap_or_else(|_error| today.to_string())
}

fn render_summary(report: &Value) -> Vec<String> {
    let capture = &report["capture_health"];
    let synthesis = &report["synthesis_health"];
    let consumer_signal = &report["consumer_signal"];
    let mut lines = Vec::new();
    let range = report["range"]
        .as_array()
        .expect("health range is an array");
    lines.push(format!(
        "Range: {} -> {}",
        display_value(&range[0]),
        display_value(&range[1])
    ));
    lines.push("Capture".to_string());
    lines.push(format!(
        "  hours_with_capture: {}",
        display_value(&capture["hours_with_capture"])
    ));
    lines.push(format!(
        "  hours_total: {}",
        display_value(&capture["hours_total"])
    ));
    lines.push(format!(
        "  coverage_ratio: {}",
        dash(&capture["coverage_ratio"])
    ));
    lines.push(format!(
        "  facets_with_recent_capture: {}",
        join_values(&capture["facets_with_recent_capture"])
    ));
    lines.push(format!(
        "  facets_silent_24h: {}",
        join_values(&capture["facets_silent_24h"])
    ));
    lines.push(format!(
        "  last_segment_at: {}",
        dash(&capture["last_segment_at"])
    ));
    lines.push("Synthesis".to_string());
    lines.push(format!(
        "  activities_count: {}",
        display_value(&synthesis["activities_count"])
    ));
    lines.push(format!(
        "  activities_with_participation: {}",
        display_value(&synthesis["activities_with_participation"])
    ));
    lines.push(format!(
        "  activities_with_story: {}",
        display_value(&synthesis["activities_with_story"])
    ));
    lines.push(format!(
        "  activities_user_edited: {}",
        display_value(&synthesis["activities_user_edited"])
    ));
    lines.push(format!(
        "  activities_anticipated_unfilled: {}",
        display_value(&synthesis["activities_anticipated_unfilled"])
    ));
    lines.push(format!(
        "  talent_run_failures_24h: {}",
        dash(&synthesis["talent_run_failures_24h"])
    ));
    lines.push(format!(
        "  talent_degraded_outputs_24h: {}",
        dash(&synthesis["talent_degraded_outputs_24h"])
    ));
    lines.push(format!(
        "  indexer_last_rebuild_at: {}",
        dash(&synthesis["indexer_last_rebuild_at"])
    ));
    render_backlog(&mut lines, &report["segment_backlog"]);
    lines.push("Consumer Signals".to_string());
    lines.push(format!(
        "  profile_entities_total: {}",
        display_value(&consumer_signal["profile_entities_total"])
    ));
    if let Some(brain) = report.get("brain_health").and_then(Value::as_object)
        && let Some(lines_value) = brain.get("lines").and_then(Value::as_array)
        && !lines_value.is_empty()
    {
        for line in lines_value {
            lines.push(display_value(line));
        }
    }
    lines.push("Notes".to_string());
    let notes = report["notes"]
        .as_array()
        .expect("health notes is an array");
    if notes.is_empty() {
        lines.push("  none".to_string());
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

fn render_full(report: &Value) -> Vec<String> {
    let mut lines = render_summary(report);
    let silent = report["capture_health"]["facets_silent_24h"]
        .as_array()
        .expect("silent facets is an array");
    if silent.is_empty() {
        return lines;
    }
    lines.push("Silent Facet Detail".to_string());
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

fn render_backlog(lines: &mut Vec<String>, backlog: &Value) {
    let n = backlog["not_thought"].as_i64().expect("backlog count");
    let m = backlog["days_with_backlog"].as_i64().expect("backlog days");
    let seg_word = if n == 1 { "segment" } else { "segments" };
    let day_word = if m == 1 { "day" } else { "days" };
    let errors = truthy(&backlog["errors"]);
    if errors && n > 0 {
        lines.push(format!(
            "  at least {n} {seg_word} across {m} {day_word} awaiting thinking (status incomplete)"
        ));
    } else if errors {
        lines.push("  Segment analysis status unavailable".to_string());
    } else if n > 0 {
        lines.push(format!(
            "  {n} {seg_word} across {m} {day_word} awaiting thinking"
        ));
    }
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

fn health_error(error: ClientError) -> CommandOutput {
    match error {
        ClientError::Unreachable { .. } => stderr(SERVICE_DOWN_MESSAGE),
        _ => stderr(error.detail().unwrap_or_else(|| error.message())),
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
            stdin: "",
            today: "20260723",
            transport: &transport,
            clock: None,
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
            link_pairing: None,
            link_serve: None,
        });

        assert_eq!(
            output,
            CommandOutput {
                stdout: String::new(),
                stderr: "journal isn't running. start it with 'journal up' and retry.\n"
                    .to_string(),
                exit: 1,
            }
        );
        transport.assert_done();
    }

    #[test]
    fn health_error_renderer_preserves_variant_details() {
        assert_eq!(
            health_error(ClientError::unreachable(Some("io: x".to_string()))),
            CommandOutput {
                stdout: String::new(),
                stderr: "journal isn't running. start it with 'journal up' and retry.\n"
                    .to_string(),
                exit: 1,
            }
        );
        assert_eq!(
            health_error(ClientError::timeout(Some("D".to_string()))),
            CommandOutput {
                stdout: String::new(),
                stderr: "D\n".to_string(),
                exit: 1,
            }
        );
        assert_eq!(
            health_error(ClientError::ReasonRejected {
                status: 400,
                error: "invalid request".to_string(),
                reason_code: Some("invalid_request_value".to_string()),
                detail: Some("day must be YYYYMMDD".to_string()),
                payload: Box::new(serde_json::Value::Null),
            }),
            CommandOutput {
                stdout: String::new(),
                stderr: "day must be YYYYMMDD\n".to_string(),
                exit: 1,
            }
        );
        assert_eq!(
            health_error(ClientError::MalformedSuccess { status: Some(200) }),
            CommandOutput {
                stdout: String::new(),
                stderr: "I couldn't read the journal response.\n".to_string(),
                exit: 1,
            }
        );
        assert_eq!(
            health_error(ClientError::UnreadableServerError { status: Some(500) }),
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
                error: "I couldn't build your journal health report.".to_string(),
                reason_code: Some("health_report_failed".to_string()),
                detail: Some("health report unavailable".to_string()),
                payload: Box::new(serde_json::Value::Null),
            }),
        }]);
        let output = pipeline(CommandContext {
            args: &args,
            env: &env,
            stdin: "",
            today: "20260723",
            transport: &transport,
            clock: None,
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
            link_pairing: None,
            link_serve: None,
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
}
