// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use chrono::{DateTime, Local};
use serde_json::Value;

use crate::CliRun;
use crate::args::LogOptions;
use crate::runs;

fn event_detail(event: &Value, etype: &str) -> String {
    let string = |key| {
        event
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    match etype {
        "request" => string("prompt"),
        "start" => format!("{} \"{}\"", string("model"), string("prompt")),
        "thinking" => event
            .get("summary")
            .and_then(Value::as_str)
            .filter(|summary| !summary.is_empty())
            .or_else(|| {
                event
                    .get("content")
                    .and_then(Value::as_str)
                    .filter(|content| !content.is_empty())
            })
            .unwrap_or_default()
            .to_owned(),
        "tool_start" => {
            let tool = string("tool");
            let Some(args) = event.get("args").and_then(Value::as_object) else {
                return tool;
            };
            let parts = args
                .iter()
                .map(|(key, value)| {
                    format!("{key}={}", solstone_core_format::json_compact_ascii(value))
                })
                .collect::<Vec<_>>();
            format!("{tool}({})", parts.join(", "))
        }
        "tool_end" => format!("{} → {}", string("tool"), string("result")),
        "talent_updated" => string("talent"),
        "finish" => {
            let result = string("result");
            let Some(usage) = event.get("usage").and_then(Value::as_object) else {
                return result;
            };
            if usage.is_empty() {
                return result;
            }
            let input = usage
                .get("input_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let output = usage
                .get("output_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            format!("{result} [{input}in/{output}out]")
        }
        "error" => string("error"),
        "info" => string("message"),
        _ => String::new(),
    }
}

fn format_event_line(event: &Value, full: bool) -> String {
    let timestamp = event.get("ts").and_then(Value::as_i64).unwrap_or(0);
    let local = DateTime::from_timestamp_millis(timestamp)
        .unwrap()
        .with_timezone(&Local);
    let time = local.format("%H:%M:%S");
    let etype = event.get("event").and_then(Value::as_str).unwrap_or("?");
    let label = match etype {
        "thinking" => "think",
        "tool_start" => "tool",
        "talent_updated" => "updated",
        _ => etype,
    };
    let mut detail = event_detail(event, etype);
    if full {
        detail = detail.replace('\n', "\\n");
    } else {
        detail = detail.replace('\n', " ");
        if detail.chars().count() > 76 {
            detail = format!("{}…", detail.chars().take(75).collect::<String>());
        }
    }
    format!("{time}.{:03}  {label:<8}  {detail}", timestamp % 1_000)
}

pub(crate) fn run_log(talents_dir: &Path, options: &LogOptions) -> CliRun {
    let Some(run_file) = runs::find_run_file(talents_dir, &options.id) else {
        return CliRun {
            stdout: String::new(),
            stderr: format!("Talent run not found: {}\n", options.id),
            exit_code: 1,
        };
    };
    let text = match fs::read_to_string(&run_file) {
        Ok(text) => text,
        Err(error) => {
            return CliRun {
                stdout: String::new(),
                stderr: format!("failed to read {}: {error}\n", run_file.display()),
                exit_code: 1,
            };
        }
    };
    if options.json {
        return CliRun {
            stdout: text,
            stderr: String::new(),
            exit_code: 0,
        };
    }
    let mut output = String::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if !event.is_object() {
            continue;
        }
        output.push_str(&format_event_line(&event, options.full));
        output.push('\n');
    }
    CliRun {
        stdout: output,
        stderr: String::new(),
        exit_code: 0,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn options(id: &str, json: bool, full: bool) -> LogOptions {
        LogOptions {
            id: id.to_owned(),
            json,
            full,
        }
    }

    fn event(event: &str, fields: Value) -> Value {
        let mut fields = fields.as_object().cloned().expect("object");
        fields.insert("event".to_owned(), Value::String(event.to_owned()));
        Value::Object(fields)
    }

    #[test]
    fn event_details_match_every_supported_event() {
        assert_eq!(
            event_detail(
                &event("request", serde_json::json!({"prompt":"synthetic request"})),
                "request"
            ),
            "synthetic request"
        );
        assert_eq!(
            event_detail(
                &event(
                    "start",
                    serde_json::json!({"model":"test-model","prompt":"begin"})
                ),
                "start"
            ),
            "test-model \"begin\""
        );
        assert_eq!(
            event_detail(
                &event(
                    "thinking",
                    serde_json::json!({"summary":"brief","content":"long"})
                ),
                "thinking"
            ),
            "brief"
        );
        assert_eq!(
            event_detail(
                &event("thinking", serde_json::json!({"content":"fallback"})),
                "thinking"
            ),
            "fallback"
        );
        assert_eq!(
            event_detail(
                &event(
                    "tool_start",
                    serde_json::json!({"tool":"probe","args":{"first":"café","second":2}})
                ),
                "tool_start"
            ),
            "probe(first=\"caf\\u00e9\", second=2)"
        );
        assert_eq!(
            event_detail(
                &event(
                    "tool_end",
                    serde_json::json!({"tool":"probe","result":"done"})
                ),
                "tool_end"
            ),
            "probe → done"
        );
        assert_eq!(
            event_detail(
                &event("talent_updated", serde_json::json!({"talent":"synthetic"})),
                "talent_updated"
            ),
            "synthetic"
        );
        assert_eq!(
            event_detail(
                &event(
                    "finish",
                    serde_json::json!({"result":"done","usage":{"input_tokens":3,"output_tokens":5}})
                ),
                "finish"
            ),
            "done [3in/5out]"
        );
        assert_eq!(
            event_detail(
                &event("finish", serde_json::json!({"result":"done"})),
                "finish"
            ),
            "done"
        );
        assert_eq!(
            event_detail(
                &event("error", serde_json::json!({"error":"synthetic failure"})),
                "error"
            ),
            "synthetic failure"
        );
        assert_eq!(
            event_detail(
                &event("info", serde_json::json!({"message":"captured stdout"})),
                "info"
            ),
            "captured stdout"
        );
        assert_eq!(
            event_detail(&event("other", serde_json::json!({})), "other"),
            ""
        );
    }

    #[test]
    fn empty_tool_start_args_keep_parens() {
        assert_eq!(
            event_detail(
                &event("tool_start", serde_json::json!({"tool":"probe","args":{}})),
                "tool_start"
            ),
            "probe()"
        );
    }

    #[test]
    fn formats_labels_timestamp_truncation_and_full_details() {
        let boundary = "x".repeat(76);
        let timestamp = 1_700_000_000_123_i64;
        let expected_time = DateTime::from_timestamp_millis(timestamp)
            .unwrap()
            .with_timezone(&Local)
            .format("%H:%M:%S")
            .to_string();
        let thinking = event(
            "thinking",
            serde_json::json!({"ts":timestamp,"content":boundary}),
        );
        assert_eq!(
            format_event_line(&thinking, false),
            format!("{expected_time}.123  think     {}", "x".repeat(76))
        );

        let longer = event(
            "thinking",
            serde_json::json!({"ts":timestamp,"content":"y".repeat(77)}),
        );
        let line = format_event_line(&longer, false);
        let detail = line.rsplit("  ").next().expect("detail");
        assert_eq!(detail.chars().count(), 76);
        assert_eq!(detail, format!("{}…", "y".repeat(75)));
        assert_eq!(detail.chars().last(), Some('…'));

        let tool = event(
            "tool_start",
            serde_json::json!({"ts":timestamp,"tool":"probe","args":{}}),
        );
        assert_eq!(
            format_event_line(&tool, false),
            format!("{expected_time}.123  tool      probe()")
        );

        let full = event(
            "talent_updated",
            serde_json::json!({"ts":timestamp,"talent":"row-one\nrow-two"}),
        );
        assert_eq!(
            format_event_line(&full, true),
            format!("{expected_time}.123  updated   row-one\\nrow-two")
        );
        let passthrough = event("custom", serde_json::json!({"ts":timestamp}));
        assert!(format_event_line(&passthrough, false).contains("  custom    "));
    }

    #[test]
    fn raw_json_preserves_malformed_lines_while_rendered_mode_skips_them() {
        let root = tempfile::tempdir().expect("tempdir");
        let talents = root.path().join("talents");
        fs::create_dir_all(talents.join("synthetic")).expect("talent directory");
        let raw = "{\"event\":\"request\",\"use_id\":\"run-z\",\"ts\":1700000000123,\"prompt\":\"synthetic\"}\nnot-json\n";
        fs::write(talents.join("synthetic/run-z.jsonl"), raw).expect("run");
        let json = run_log(&talents, &options("run-z", true, false));
        assert_eq!(
            json,
            CliRun {
                stdout: raw.to_owned(),
                stderr: String::new(),
                exit_code: 0
            }
        );
        let rendered = run_log(&talents, &options("run-z", false, false));
        assert_eq!(rendered.exit_code, 0, "{}", rendered.stderr);
        assert_eq!(rendered.stdout.lines().count(), 1);
        assert!(!rendered.stdout.contains("not-json"));
    }

    #[test]
    fn missing_run_has_reference_error() {
        let root = tempfile::tempdir().expect("tempdir");
        let output = run_log(
            &root.path().join("talents"),
            &options("missing-run", false, false),
        );
        assert_eq!(
            output,
            CliRun {
                stdout: String::new(),
                stderr: "Talent run not found: missing-run\n".to_owned(),
                exit_code: 1,
            }
        );
    }
}
