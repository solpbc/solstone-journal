// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;

use crate::error::GrabFailure;
use crate::reader::{MissingVideoCause, ScreenStatus};

#[derive(Clone, Debug, Default)]
pub(crate) struct HumanHints {
    pub screen_statuses: Vec<ScreenStatus>,
    pub missing_video_cause: Option<MissingVideoCause>,
    pub frame_error_texts: Vec<Option<String>>,
    pub single_frame_error_text: Option<String>,
}

fn level_4_footer(locale: &str) -> String {
    solstone_core_cli_copy::render_for(
        locale,
        "journal.cli.grab.level4.footer",
        &[
            (
                "inspect_command",
                solstone_core_cli_copy::Value::String(
                    "solstone journal grab <day> <stream> <segment> <screen> <id>",
                ),
            ),
            (
                "save_one_command",
                solstone_core_cli_copy::Value::String(
                    "solstone journal grab <day> <stream> <segment> <screen> <id> --out PATH",
                ),
            ),
            (
                "save_many_command",
                solstone_core_cli_copy::Value::String(
                    "solstone journal grab <day> <stream> <segment> <screen> <id1>,<id2>,... --out PATH",
                ),
            ),
            ("line_break", solstone_core_cli_copy::Value::String("\n")),
        ],
    )
}

/// The save-mode footer for a screen whose video is gone.
///
/// ⛔ The reason comes from the typed cause (or English fallback), which names a
/// cause only from a recorded deletion. A missing reason says the video is gone
/// and nothing more.
fn missing_video_footer_in(
    locale: &str,
    summary: &serde_json::Map<String, Value>,
    cause: Option<MissingVideoCause>,
) -> String {
    let reason = if let Some(cause) = cause {
        cause.render_reason_in(locale)
    } else if let Some(reason) = summary.get("video_missing_reason").and_then(Value::as_str) {
        reason.to_owned()
    } else {
        MissingVideoCause::Unknown.render_reason_in(locale)
    };
    solstone_core_cli_copy::render_for(
        locale,
        "journal.cli.grab.level4.missing.video",
        &[
            ("reason", solstone_core_cli_copy::Value::String(&reason)),
            (
                "command",
                solstone_core_cli_copy::Value::String(
                    "solstone journal grab <day> <stream> <segment> <screen> <id>",
                ),
            ),
            ("line_break", solstone_core_cli_copy::Value::String("\n")),
        ],
    )
}

#[cfg(test)]
fn missing_video_footer(summary: &serde_json::Map<String, Value>) -> String {
    missing_video_footer_in(solstone_core_cli_copy::ENGLISH, summary, None)
}

pub(crate) fn raw_frame_error_text(frame: &Value) -> Option<String> {
    let error = frame
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let line = error.lines().next().unwrap_or_default().trim();
    if line.is_empty() {
        return None;
    }
    Some(if line.chars().count() <= 60 {
        line.to_owned()
    } else {
        format!("{}...", line.chars().take(57).collect::<String>())
    })
}

pub(crate) fn frame_notes(frame: &Value) -> String {
    match raw_frame_error_text(frame) {
        Some(text) => format!("error: {text}"),
        None => String::new(),
    }
}

fn format_frame_note_in(locale: &str, text: &str) -> String {
    solstone_core_cli_copy::render_for(
        locale,
        "journal.cli.grab.frame.note.error",
        &[("text", solstone_core_cli_copy::Value::String(text))],
    )
}

fn header_label(locale: &str, column: &str) -> String {
    let id = match column {
        "day" => "journal.cli.grab.header.day",
        "streams" => "journal.cli.grab.header.streams",
        "segments" => "journal.cli.grab.header.segments",
        "screens" => "journal.cli.grab.header.screens",
        "frames_analyzed" => "journal.cli.grab.header.frames.analyzed",
        "stream" => "journal.cli.grab.header.stream",
        "segment" => "journal.cli.grab.header.segment",
        "start" => "journal.cli.grab.header.start",
        "end" => "journal.cli.grab.header.end",
        "screen" => "journal.cli.grab.header.screen",
        "position" => "journal.cli.grab.header.position",
        "connector" => "journal.cli.grab.header.connector",
        "status" => "journal.cli.grab.header.status",
        "frame_id" => "journal.cli.grab.header.frame.id",
        "timestamp" => "journal.cli.grab.header.timestamp",
        "abs_time" => "journal.cli.grab.header.abs.time",
        "primary" => "journal.cli.grab.header.primary",
        "notes" => "journal.cli.grab.header.notes",
        other => return other.to_owned(),
    };
    solstone_core_cli_copy::render_for(locale, id, &[])
}

#[cfg(test)]
pub(crate) fn print_table(columns: &[&str], rows: &[Value]) -> String {
    let headers: Vec<String> = columns.iter().map(|column| (*column).to_owned()).collect();
    print_table_with_headers(columns, &headers, rows, |_, _, row, column| {
        cell(row, column)
    })
}

fn print_table_in(
    locale: &str,
    columns: &[&str],
    rows: &[Value],
    cell_fn: impl Fn(usize, usize, &Value, &str) -> String,
) -> String {
    let headers: Vec<String> = columns
        .iter()
        .map(|column| header_label(locale, column))
        .collect();
    print_table_with_headers(columns, &headers, rows, cell_fn)
}

fn print_table_with_headers(
    columns: &[&str],
    headers: &[String],
    rows: &[Value],
    cell_fn: impl Fn(usize, usize, &Value, &str) -> String,
) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let rendered_rows: Vec<Vec<String>> = rows
        .iter()
        .enumerate()
        .map(|(row_idx, row)| {
            columns
                .iter()
                .enumerate()
                .map(|(col_idx, column)| cell_fn(row_idx, col_idx, row, column))
                .collect()
        })
        .collect();
    let widths: Vec<_> = headers
        .iter()
        .enumerate()
        .map(|(col_idx, header)| {
            rendered_rows
                .iter()
                .fold(header.chars().count(), |width, row| {
                    width.max(row[col_idx].chars().count())
                })
        })
        .collect();
    let mut output = String::new();
    append_row(
        &mut output,
        headers
            .iter()
            .zip(&widths)
            .map(|(header, width)| (header.clone(), *width)),
    );
    append_row(
        &mut output,
        widths.iter().map(|width| ("-".repeat(*width), *width)),
    );
    for row in rendered_rows {
        append_row(&mut output, row.into_iter().zip(widths.iter().copied()));
    }
    output
}

fn next_hint(locale: &str, command: &str) -> String {
    solstone_core_cli_copy::render_for(
        locale,
        "journal.cli.grab.next",
        &[("command", solstone_core_cli_copy::Value::String(command))],
    )
}

pub(crate) fn render_in(
    locale: &str,
    payload: &Value,
    hints: &HumanHints,
) -> Result<String, GrabFailure> {
    let level = payload
        .get("level")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            GrabFailure::runtime(solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.grab.error.unsupported.level",
                &[],
            ))
        })?;
    let data = payload.get("data").ok_or_else(|| {
        GrabFailure::runtime(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.grab.error.unsupported.level",
            &[],
        ))
    })?;
    let mut output = String::new();
    match level {
        "0" => {
            output.push_str(&print_table_in(
                locale,
                &["day", "streams", "segments", "screens", "frames_analyzed"],
                array(locale, data, "days")?,
                |_, _, row, column| cell(row, column),
            ));
            output.push('\n');
            output.push_str(&next_hint(locale, "solstone journal grab <day>"));
            output.push('\n');
        }
        "1" => {
            output.push_str(&print_table_in(
                locale,
                &["stream", "segments", "screens", "frames_analyzed"],
                array(locale, data, "streams")?,
                |_, _, row, column| cell(row, column),
            ));
            output.push('\n');
            output.push_str(&next_hint(locale, "solstone journal grab <day> <stream>"));
            output.push('\n');
        }
        "2" => {
            output.push_str(&print_table_in(
                locale,
                &["segment", "start", "end", "screens", "frames_analyzed"],
                array(locale, data, "segments")?,
                |_, _, row, column| cell(row, column),
            ));
            output.push('\n');
            output.push_str(&next_hint(
                locale,
                "solstone journal grab <day> <stream> <segment>",
            ));
            output.push('\n');
        }
        "3" => {
            output.push_str(&print_table_in(
                locale,
                &[
                    "screen",
                    "position",
                    "connector",
                    "frames_analyzed",
                    "status",
                ],
                array(locale, data, "screens")?,
                |row_idx, _, row, column| {
                    if column == "status"
                        && let Some(status) = hints.screen_statuses.get(row_idx)
                    {
                        status.render_in(locale)
                    } else {
                        cell(row, column)
                    }
                },
            ));
            output.push('\n');
            output.push_str(&next_hint(
                locale,
                "solstone journal grab <day> <stream> <segment> <screen>",
            ));
            output.push('\n');
        }
        "4" => {
            let summary = data
                .get("summary")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    GrabFailure::runtime(solstone_core_cli_copy::render_for(
                        locale,
                        "journal.cli.grab.error.invalid.level4",
                        &[],
                    ))
                })?;
            if summary.get("legacy_schema").and_then(Value::as_bool) == Some(true) {
                output.push_str(&solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.grab.level4.legacy",
                    &[],
                ));
                output.push('\n');
            } else if summary.get("frames_analyzed").and_then(Value::as_i64) == Some(0)
                && array(locale, data, "frames")?.is_empty()
            {
                output.push_str(&solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.grab.level4.empty",
                    &[],
                ));
                output.push('\n');
            } else {
                output.push_str(&print_table_in(
                    locale,
                    &["frame_id", "timestamp", "abs_time", "primary", "notes"],
                    array(locale, data, "frames")?,
                    |row_idx, _, row, column| {
                        if column == "notes"
                            && let Some(Some(text)) = hints.frame_error_texts.get(row_idx)
                        {
                            format_frame_note_in(locale, text)
                        } else {
                            cell(row, column)
                        }
                    },
                ));
                output.push('\n');
                if summary.get("video_present").and_then(Value::as_bool) == Some(true) {
                    output.push_str(&level_4_footer(locale));
                } else {
                    output.push_str(&missing_video_footer_in(
                        locale,
                        summary,
                        hints.missing_video_cause,
                    ));
                }
                output.push('\n');
            }
        }
        "5a" => {
            let scope = payload
                .get("scope")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    GrabFailure::runtime(solstone_core_cli_copy::render_for(
                        locale,
                        "journal.cli.grab.error.invalid.level5a",
                        &[],
                    ))
                })?;
            let source = data
                .get("source")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    GrabFailure::runtime(solstone_core_cli_copy::render_for(
                        locale,
                        "journal.cli.grab.error.invalid.level5a",
                        &[],
                    ))
                })?;
            let computed = data
                .get("computed")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    GrabFailure::runtime(solstone_core_cli_copy::render_for(
                        locale,
                        "journal.cli.grab.error.invalid.level5a",
                        &[],
                    ))
                })?;
            for (id, value) in [
                ("journal.cli.grab.level5a.screen", scope.get("screen")),
                ("journal.cli.grab.level5a.jsonl", source.get("jsonl")),
                ("journal.cli.grab.level5a.video", source.get("video")),
                ("journal.cli.grab.level5a.frame", scope.get("frame_id")),
                ("journal.cli.grab.level5a.time", computed.get("abs_time")),
            ] {
                let formatted = display(value);
                output.push_str(&solstone_core_cli_copy::render_for(
                    locale,
                    id,
                    &[("value", solstone_core_cli_copy::Value::String(&formatted))],
                ));
                output.push('\n');
            }
            let rendered_notes = if let Some(text) = &hints.single_frame_error_text {
                Some(format_frame_note_in(locale, text))
            } else {
                computed
                    .get("notes")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
            };
            if let Some(notes) = rendered_notes {
                output.push_str(&solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.grab.level5a.notes",
                    &[("value", solstone_core_cli_copy::Value::String(&notes))],
                ));
                output.push('\n');
            }
            output.push('\n');
            output.push_str(
                &serde_json::to_string_pretty(data.get("frame").unwrap())
                    .expect("serializable payload"),
            );
            output.push_str("\n\n");
            output.push_str(&solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.grab.level5a.footer",
                &[
                    (
                        "save_command",
                        solstone_core_cli_copy::Value::String(
                            "solstone journal grab <day> <stream> <segment> <screen> <id> --out PATH",
                        ),
                    ),
                    (
                        "batch_command",
                        solstone_core_cli_copy::Value::String(
                            "solstone journal grab <day> <stream> <segment> <screen> <id1>,<id2>,... --out PATH",
                        ),
                    ),
                    ("line_break", solstone_core_cli_copy::Value::String("\n")),
                ],
            ));
            output.push('\n');
        }
        "5b" | "5c" => {
            for item in array(locale, data, "saved")? {
                let path = display(item.get("path"));
                output.push_str(&solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.grab.saved",
                    &[("path", solstone_core_cli_copy::Value::String(&path))],
                ));
                output.push('\n');
            }
        }
        _ => {
            return Err(GrabFailure::runtime(solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.grab.error.unsupported.level.named",
                &[("level", solstone_core_cli_copy::Value::String(level))],
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
fn array<'a>(locale: &str, value: &'a Value, key: &str) -> Result<&'a [Value], GrabFailure> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| {
            GrabFailure::runtime(solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.grab.error.invalid.render.payload",
                &[],
            ))
        })
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
