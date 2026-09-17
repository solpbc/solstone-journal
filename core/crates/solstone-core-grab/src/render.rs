// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;

use crate::error::GrabFailure;

const LEVEL_4_FOOTER: &str = "Inspect:    journal grab <day> <stream> <segment> <screen> <id>\nSave one:   journal grab <day> <stream> <segment> <screen> <id> --out PATH\nSave many:  journal grab <day> <stream> <segment> <screen> <id1>,<id2>,... --out PATH\n\nHow extraction works:\n  Decoding walks the video linearly from frame 0 — seeking is unsafe at the\n  1 Hz capture rate. Cost is dominated by the highest requested frame_id, not\n  the count. Asking for ids 7,12,23 costs the same as asking for 23 alone.\n  Prefer batch mode when you want more than one frame from the same screen.";
const LEVEL_4_MISSING_VIDEO_TAIL: &str = "Frame metadata above is still readable.\n\nInspect: journal grab <day> <stream> <segment> <screen> <id>";

/// The save-mode footer for a screen whose video is gone.
///
/// ⛔ The reason comes from the payload, which names a cause only from a
/// recorded deletion. A missing reason says the video is gone and nothing more.
fn missing_video_footer(summary: &serde_json::Map<String, Value>) -> String {
    let reason = summary
        .get("video_missing_reason")
        .and_then(Value::as_str)
        .unwrap_or("the original video is no longer in your journal");
    format!("Save mode unavailable: {reason}.\n{LEVEL_4_MISSING_VIDEO_TAIL}")
}

pub(crate) fn frame_notes(frame: &Value) -> String {
    let error = frame
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let line = error.lines().next().unwrap_or_default().trim();
    if line.is_empty() {
        return String::new();
    }
    let text = if line.chars().count() <= 60 {
        line.to_owned()
    } else {
        format!("{}...", line.chars().take(57).collect::<String>())
    };
    format!("error: {text}")
}

pub(crate) fn print_table(columns: &[&str], rows: &[Value]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let widths: Vec<_> = columns
        .iter()
        .map(|column| {
            rows.iter().fold(column.chars().count(), |width, row| {
                width.max(cell(row, column).chars().count())
            })
        })
        .collect();
    let mut output = String::new();
    append_row(
        &mut output,
        columns
            .iter()
            .zip(&widths)
            .map(|(column, width)| (column.to_string(), *width)),
    );
    append_row(
        &mut output,
        widths.iter().map(|width| ("-".repeat(*width), *width)),
    );
    for row in rows {
        append_row(
            &mut output,
            columns
                .iter()
                .zip(&widths)
                .map(|(column, width)| (cell(row, column), *width)),
        );
    }
    output
}

pub(crate) fn render(payload: &Value) -> Result<String, GrabFailure> {
    let level = payload
        .get("level")
        .and_then(Value::as_str)
        .ok_or_else(|| GrabFailure::runtime("unsupported output level"))?;
    let data = payload
        .get("data")
        .ok_or_else(|| GrabFailure::runtime("unsupported output level"))?;
    let mut output = String::new();
    match level {
        "0" => {
            output.push_str(&print_table(
                &["day", "streams", "segments", "screens", "frames_analyzed"],
                array(data, "days")?,
            ));
            output.push_str("\nNext: journal grab <day>\n");
        }
        "1" => {
            output.push_str(&print_table(
                &["stream", "segments", "screens", "frames_analyzed"],
                array(data, "streams")?,
            ));
            output.push_str("\nNext: journal grab <day> <stream>\n");
        }
        "2" => {
            output.push_str(&print_table(
                &["segment", "start", "end", "screens", "frames_analyzed"],
                array(data, "segments")?,
            ));
            output.push_str("\nNext: journal grab <day> <stream> <segment>\n");
        }
        "3" => {
            output.push_str(&print_table(
                &[
                    "screen",
                    "position",
                    "connector",
                    "frames_analyzed",
                    "status",
                ],
                array(data, "screens")?,
            ));
            output.push_str("\nNext: journal grab <day> <stream> <segment> <screen>\n");
        }
        "4" => {
            let summary = data
                .get("summary")
                .and_then(Value::as_object)
                .ok_or_else(|| GrabFailure::runtime("invalid level 4 payload"))?;
            if summary.get("legacy_schema").and_then(Value::as_bool) == Some(true) {
                output.push_str("0 frames analyzed: file uses pre-frame_id schema\n");
            } else if summary.get("frames_analyzed").and_then(Value::as_i64) == Some(0)
                && array(data, "frames")?.is_empty()
            {
                output.push_str("No qualified frames in this screen's analysis.\n");
            } else {
                output.push_str(&print_table(
                    &["frame_id", "timestamp", "abs_time", "primary", "notes"],
                    array(data, "frames")?,
                ));
                output.push('\n');
                if summary.get("video_present").and_then(Value::as_bool) == Some(true) {
                    output.push_str(LEVEL_4_FOOTER);
                } else {
                    output.push_str(&missing_video_footer(summary));
                }
                output.push('\n');
            }
        }
        "5a" => {
            let scope = payload
                .get("scope")
                .and_then(Value::as_object)
                .ok_or_else(|| GrabFailure::runtime("invalid level 5a payload"))?;
            let source = data
                .get("source")
                .and_then(Value::as_object)
                .ok_or_else(|| GrabFailure::runtime("invalid level 5a payload"))?;
            let computed = data
                .get("computed")
                .and_then(Value::as_object)
                .ok_or_else(|| GrabFailure::runtime("invalid level 5a payload"))?;
            for (label, value) in [
                ("Screen", scope.get("screen")),
                ("JSONL", source.get("jsonl")),
                ("Video", source.get("video")),
                ("Frame", scope.get("frame_id")),
                ("Time", computed.get("abs_time")),
            ] {
                output.push_str(&format!("{label}: {}\n", display(value)));
            }
            if let Some(notes) = computed
                .get("notes")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                output.push_str(&format!("Notes: {notes}\n"));
            }
            output.push('\n');
            output.push_str(
                &serde_json::to_string_pretty(data.get("frame").unwrap())
                    .expect("serializable payload"),
            );
            output.push_str("\n\nSave: journal grab <day> <stream> <segment> <screen> <id> --out PATH\nBatch: journal grab <day> <stream> <segment> <screen> <id1>,<id2>,... --out PATH\n");
        }
        "5b" | "5c" => {
            for item in array(data, "saved")? {
                output.push_str(&format!("saved {}\n", display(item.get("path"))));
            }
        }
        _ => {
            return Err(GrabFailure::runtime(format!(
                "unsupported output level: {level}"
            )));
        }
    }
    Ok(output)
}

fn append_row(output: &mut String, values: impl Iterator<Item = (String, usize)>) {
    let values: Vec<_> = values.collect();
    output.push_str(
        &values
            .iter()
            .map(|(value, width)| {
                format!(
                    "{value}{}",
                    " ".repeat(width.saturating_sub(value.chars().count()))
                )
            })
            .collect::<Vec<_>>()
            .join("  "),
    );
    output.push('\n');
}

fn cell(row: &Value, key: &str) -> String {
    display(row.get(key))
}
fn display(value: Option<&Value>) -> String {
    value
        .map(|value| match value {
            Value::String(value) => value.clone(),
            Value::Null => "None".to_owned(),
            Value::Bool(value) => if *value { "True" } else { "False" }.to_owned(),
            _ => value.to_string(),
        })
        .unwrap_or_default()
}
fn array<'a>(value: &'a Value, key: &str) -> Result<&'a [Value], GrabFailure> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| GrabFailure::runtime("invalid render payload"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{frame_notes, missing_video_footer, print_table};

    #[test]
    fn table_uses_python_widths_and_empty_is_silent() {
        assert_eq!(print_table(&["name", "n"], &[]), "");
        assert_eq!(
            print_table(&["name", "n"], &[json!({"name":"a", "n":12})]),
            "name  n \n----  --\na     12\n"
        );
    }

    #[test]
    fn the_missing_video_footer_carries_the_payload_reason() {
        let with_reason = json!({"video_present": false, "video_missing_reason": "you deleted the original video after your backup copied it"});
        assert!(
            missing_video_footer(with_reason.as_object().unwrap()).starts_with(
                "Save mode unavailable: you deleted the original video after your backup copied it.\n"
            )
        );
        // A payload with no reason says the video is gone and nothing more.
        let without = json!({"video_present": false});
        assert!(
            missing_video_footer(without.as_object().unwrap()).starts_with(
                "Save mode unavailable: the original video is no longer in your journal.\n"
            )
        );
        assert!(
            missing_video_footer(without.as_object().unwrap())
                .contains("Frame metadata above is still readable.")
        );
    }

    #[test]
    fn notes_use_first_line_and_exact_boundary() {
        assert_eq!(
            frame_notes(&json!({"error":"  first\nsecond"})),
            "error: first"
        );
        assert_eq!(
            frame_notes(&json!({"error":"x".repeat(60)})),
            format!("error: {}", "x".repeat(60))
        );
        assert_eq!(
            frame_notes(&json!({"error":"x".repeat(61)})),
            format!("error: {}...", "x".repeat(57))
        );
    }
}
