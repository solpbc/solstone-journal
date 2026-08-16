// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};

use crate::command::{CommandContext, CommandOutput};
use crate::decode::decode_response;
use crate::error::{ClientError, SERVICE_DOWN_MESSAGE};
use crate::json_format::json_pretty_ascii;
use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};

const DEFAULT_MAX_BYTES: usize = 16_384;

#[must_use]
pub fn read(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[
            "--start",
            "--length",
            "--segment",
            "--segments",
            "--stream",
            "--max",
        ],
        &[
            "--full",
            "--raw",
            "--transcripts",
            "--audio",
            "--percepts",
            "--screen",
            "--agents",
        ],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let day = match resolve_day(parsed.positionals.first().map(String::as_str), ctx) {
        Ok(day) => day,
        Err(output) => return output,
    };
    let segment = parsed
        .value("--segment")
        .map(str::to_string)
        .or_else(|| env_value(ctx, "SOL_SEGMENT"));
    let stream = parsed
        .value("--stream")
        .map(str::to_string)
        .or_else(|| env_value(ctx, "SOL_STREAM"));
    let max_bytes = match parsed.value("--max") {
        Some(value) => match value.parse::<usize>() {
            Ok(value) => value,
            Err(_error) => return stderr("Error: option --max requires an integer."),
        },
        None => DEFAULT_MAX_BYTES,
    };

    if parsed.has_flag("--full") && parsed.has_flag("--raw") {
        return stderr("Error: Cannot use --full and --raw together.");
    }

    let transcript_flag = parsed.has_flag("--transcripts") || parsed.has_flag("--audio");
    let percept_flag = parsed.has_flag("--percepts") || parsed.has_flag("--screen");
    let agent_flag = parsed.has_flag("--agents");
    if (parsed.has_flag("--full") || parsed.has_flag("--raw"))
        && (transcript_flag || percept_flag || agent_flag)
    {
        return stderr("Error: Cannot mix --full/--raw with individual source flags.");
    }

    let (include_transcripts, include_percepts, include_agents) = if parsed.has_flag("--full") {
        (true, true, true)
    } else if parsed.has_flag("--raw") {
        (true, true, false)
    } else if transcript_flag || percept_flag || agent_flag {
        (transcript_flag, percept_flag, agent_flag)
    } else {
        (true, false, true)
    };

    let range_selected = parsed.value("--start").is_some() || parsed.value("--length").is_some();
    let mode_count = usize::from(segment.is_some())
        + usize::from(parsed.value("--segments").is_some())
        + usize::from(range_selected);
    if mode_count > 1 {
        return stderr("Error: Cannot mix --segment, --segments, and --start/--length.");
    }

    let mut params = vec![
        QueryParam::single("transcripts", if include_transcripts { "1" } else { "0" }),
        QueryParam::single("percepts", if include_percepts { "1" } else { "0" }),
        QueryParam::single("agents", if include_agents { "1" } else { "0" }),
    ];
    if range_selected {
        let start = parsed.value("--start").unwrap_or("000000");
        let end = if let Some(length) = parsed.value("--length") {
            match add_minutes_hhmmss(start, length) {
                Ok(end) => end,
                Err(error) => return stderr(error),
            }
        } else {
            "235959".to_string()
        };
        params.push(QueryParam::single("end", end));
        params.push(QueryParam::single("start", start));
    } else if let Some(segments) = parsed.value("--segments") {
        params.push(QueryParam::single("segments", segments));
        if let Some(stream) = stream.as_deref() {
            params.push(QueryParam::single("stream", stream));
        }
    } else if let Some(segment) = segment.as_deref() {
        params.push(QueryParam::single("segment", segment));
        if let Some(stream) = stream.as_deref() {
            params.push(QueryParam::single("stream", stream));
        }
    }

    let body = match request_json(
        ctx,
        HttpMethod::Get,
        &format!("/app/transcripts/api/read/{day}"),
        params,
        None,
    ) {
        Ok(body) => body,
        Err(error) => return transcripts_error(error),
    };
    let Some(markdown) = body.get("markdown").and_then(Value::as_str) else {
        return stderr(crate::error::MALFORMED_RESPONSE_MESSAGE);
    };
    truncated_output(markdown, max_bytes)
}

#[must_use]
pub fn scan(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let day = match resolve_day(parsed.positionals.first().map(String::as_str), ctx) {
        Ok(day) => day,
        Err(output) => return output,
    };
    let data = match request_json(
        ctx,
        HttpMethod::Get,
        &format!("/app/transcripts/api/day/{day}"),
        vec![],
        None,
    ) {
        Ok(data) => data,
        Err(error) => return transcripts_error(error),
    };
    let audio = array_field(&data, "audio");
    let screen = array_field(&data, "screen");
    let segments = array_field(&data, "segments");
    let mut pending_slots: Vec<(String, String, Vec<String>)> = Vec::new();
    for segment in segments {
        if segment
            .get("data_state")
            .and_then(|state| state.get("audio"))
            .and_then(Value::as_str)
            == Some("pending")
        {
            let Some(start) = segment.get("start").and_then(Value::as_str) else {
                continue;
            };
            let (slot_start, slot_end) = pending_slot_range(start);
            if let Some((_start, _end, starts)) =
                pending_slots
                    .iter_mut()
                    .find(|(existing_start, existing_end, _starts)| {
                        *existing_start == slot_start && *existing_end == slot_end
                    })
            {
                starts.push(start.to_string());
            } else {
                pending_slots.push((slot_start, slot_end, vec![start.to_string()]));
            }
        }
    }
    for (_start, _end, starts) in &mut pending_slots {
        starts.sort();
    }

    let mut lines = vec!["Transcripts:".to_string()];
    if audio.is_empty() {
        lines.push("  (none)".to_string());
    } else {
        for range in audio {
            let start = field(&range, "start");
            let end = field(&range, "end");
            let mut starts = pending_slots
                .iter()
                .filter(|(slot_start, slot_end, _starts)| {
                    slot_overlaps_range((slot_start, slot_end), (&start, &end))
                })
                .flat_map(|(_slot_start, _slot_end, starts)| starts.iter().cloned())
                .collect::<Vec<_>>();
            starts.sort();
            let mut line = format!("  {start} - {end}");
            if !starts.is_empty() {
                line.push_str(&format!(" ({})", format_pending_scan_note(&starts)));
            }
            lines.push(line);
        }
    }
    lines.push("Percepts:".to_string());
    if screen.is_empty() {
        lines.push("  (none)".to_string());
    } else {
        for range in screen {
            lines.push(format!(
                "  {} - {}",
                field(&range, "start"),
                field(&range, "end")
            ));
        }
    }
    stdout(lines)
}

#[must_use]
pub fn segments(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let day = match resolve_day(parsed.positionals.first().map(String::as_str), ctx) {
        Ok(day) => day,
        Err(output) => return output,
    };
    let data = match request_json(
        ctx,
        HttpMethod::Get,
        &format!("/app/transcripts/api/segments/{day}"),
        vec![],
        None,
    ) {
        Ok(data) => data,
        Err(error) => return transcripts_error(error),
    };
    let segments = array_field(&data, "segments");
    if segments.is_empty() {
        return stdout_line("No segments.");
    }
    stdout(
        segments
            .iter()
            .map(|segment| {
                let types = segment
                    .get("types")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                format!(
                    "{}  {} - {}  [{}]",
                    field(segment, "key"),
                    field(segment, "start"),
                    field(segment, "end"),
                    types
                )
            })
            .collect(),
    )
}

#[must_use]
pub fn speakers(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &["--json"]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(day) = parsed.positionals.first() else {
        return stderr("Error: missing argument DAY");
    };
    let Some(stream) = parsed.positionals.get(1) else {
        return stderr("Error: missing argument STREAM");
    };
    let Some(segment) = parsed.positionals.get(2) else {
        return stderr("Error: missing argument SEGMENT");
    };
    let payload = match request_json(
        ctx,
        HttpMethod::Get,
        &format!("/app/transcripts/api/segment/{day}/{stream}/{segment}"),
        vec![],
        None,
    ) {
        Ok(payload) => payload,
        Err(error) => return transcripts_error(error),
    };
    let rows = speaker_rows(&payload);
    let result = speaker_result(day, stream, segment, &payload, &rows);
    if parsed.has_flag("--json") {
        return stdout_line(json_pretty_ascii(&result));
    }

    let mut lines = vec![format!("Speakers for {day}/{stream}/{segment}:")];
    if rows.is_empty() {
        lines.push("  (none)".to_string());
    } else {
        for row in &rows {
            let marker = if row.get("actionable").and_then(Value::as_bool) == Some(true) {
                "*"
            } else {
                "-"
            };
            let (speaker_name, confidence) = row_speaker_text(row);
            lines.push(format!(
                "  {marker} #{} {} {} {} [{}] {}",
                display_value(row.get("sentence_id")),
                field(row, "speaker_source"),
                field(row, "time"),
                speaker_name,
                confidence,
                field(row, "text")
            ));
        }
    }
    lines.push(String::new());
    lines.push(
        "* actionable: sol call speakers correct <day> <stream> <segment> <source> <sentence-id> <new-speaker>"
            .to_string(),
    );
    lines.push(
        "- not actionable: sol call speakers tag-owner <day> <stream> <segment> <source> <sentence-id>"
            .to_string(),
    );
    stdout(lines)
}

#[must_use]
pub fn stats(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(month) = parsed.positionals.first() else {
        return stderr("Error: missing argument MONTH");
    };
    let day_totals = match request_json(
        ctx,
        HttpMethod::Get,
        &format!("/app/transcripts/api/stats/{month}"),
        vec![],
        None,
    ) {
        Ok(day_totals) => day_totals,
        Err(error) => return transcripts_error(error),
    };
    let mut days = day_totals
        .as_object()
        .map(|object| object.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    days.sort();
    let mut output = Vec::new();
    let mut days_with_data = 0_usize;
    for day in days {
        let ranges = match request_json(
            ctx,
            HttpMethod::Get,
            &format!("/app/transcripts/api/ranges/{day}"),
            vec![],
            None,
        ) {
            Ok(ranges) => ranges,
            Err(error) => return transcripts_error_preserving_stdout(error, &output),
        };
        let n_transcripts = array_field(&ranges, "audio").len();
        let n_percepts = array_field(&ranges, "screen").len();
        days_with_data += 1;
        output.push(format!(
            "{day}  transcripts:{n_transcripts} percepts:{n_percepts}"
        ));
    }
    if days_with_data == 0 {
        return stdout_line(format!("No data for {month}."));
    }
    output.push(String::new());
    output.push(format!("Total: {days_with_data} days with data"));
    stdout(output)
}

#[derive(Debug, Default)]
struct ParsedArgs {
    positionals: Vec<String>,
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
            parsed.positionals.push(token.clone());
        }
        index += 1;
    }
    Ok(parsed)
}

fn request_json(
    ctx: CommandContext<'_>,
    method: HttpMethod,
    path: &str,
    params: Vec<QueryParam>,
    json: Option<Value>,
) -> Result<Value, ClientError> {
    let response = ctx.transport.request(ApiRequest {
        method,
        path: path.to_string(),
        params,
        json,
        headers: vec![],
        policy: TimeoutPolicy::Api,
    })?;
    decode_response(&response)
}

fn resolve_day(arg: Option<&str>, ctx: CommandContext<'_>) -> Result<String, CommandOutput> {
    if let Some(day) = arg.filter(|day| !day.is_empty()) {
        return Ok(day.to_string());
    }
    if let Some(day) = env_value(ctx, "SOL_DAY").filter(|day| !day.is_empty()) {
        return Ok(day);
    }
    Err(stderr(
        "Error: day is required (pass as argument or set SOL_DAY).",
    ))
}

fn env_value(ctx: CommandContext<'_>, key: &str) -> Option<String> {
    ctx.env.get(key).cloned()
}

fn add_minutes_hhmmss(start: &str, length: &str) -> Result<String, String> {
    let minutes = length
        .parse::<i64>()
        .map_err(|_error| "Error: option --length requires an integer.".to_string())?;
    if start.len() != 6 {
        return Err("Error: start time must be HHMMSS.".to_string());
    }
    let hour = start[0..2]
        .parse::<i64>()
        .map_err(|_error| "Error: start time must be HHMMSS.".to_string())?;
    let minute = start[2..4]
        .parse::<i64>()
        .map_err(|_error| "Error: start time must be HHMMSS.".to_string())?;
    let second = start[4..6]
        .parse::<i64>()
        .map_err(|_error| "Error: start time must be HHMMSS.".to_string())?;
    let mut total = hour * 3600 + minute * 60 + second + minutes * 60;
    total = ((total % 86_400) + 86_400) % 86_400;
    Ok(format!(
        "{:02}{:02}{:02}",
        total / 3600,
        (total % 3600) / 60,
        total % 60
    ))
}

fn truncated_output(text: &str, max_bytes: usize) -> CommandOutput {
    let encoded = text.as_bytes();
    if max_bytes > 0 && encoded.len() > max_bytes {
        let mut end = max_bytes;
        while std::str::from_utf8(&encoded[..end]).is_err() {
            end -= 1;
        }
        let truncated = std::str::from_utf8(&encoded[..end]).expect("valid prefix");
        return CommandOutput {
            stdout: format!("{truncated}\n"),
            stderr: format!(
                "[truncated: {} bytes total, --max {}]\n",
                comma_usize(encoded.len()),
                comma_usize(max_bytes)
            ),
            exit: 0,
        };
    }
    stdout_line(text)
}

fn comma_usize(value: usize) -> String {
    let raw = value.to_string();
    let mut output = String::new();
    for (index, ch) in raw.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            output.push(',');
        }
        output.push(ch);
    }
    output.chars().rev().collect()
}

fn pending_slot_range(start: &str) -> (String, String) {
    let hour = start[0..2].parse::<i64>().unwrap_or(0);
    let minute = start[3..5].parse::<i64>().unwrap_or(0);
    let slot_minute = minute - (minute % 15);
    let mut end_hour = hour;
    let mut end_minute = slot_minute + 15;
    if end_minute >= 60 {
        end_hour = (end_hour + 1) % 24;
        end_minute -= 60;
    }
    (
        format!("{hour:02}:{slot_minute:02}"),
        format!("{end_hour:02}:{end_minute:02}"),
    )
}

fn format_pending_scan_note(starts: &[String]) -> String {
    let noun = if starts.len() == 1 {
        "segment"
    } else {
        "segments"
    };
    format!("{} {noun} pending at {}", starts.len(), starts.join(", "))
}

fn slot_overlaps_range(slot: (&str, &str), range: (&str, &str)) -> bool {
    fn to_min(value: &str) -> i64 {
        let (hour, minute) = value.split_once(':').unwrap_or(("0", "0"));
        hour.parse::<i64>().unwrap_or(0) * 60 + minute.parse::<i64>().unwrap_or(0)
    }
    let slot_start = to_min(slot.0);
    let slot_end = to_min(slot.1);
    let range_start = to_min(range.0);
    let range_end = to_min(range.1);
    slot_start < range_end && slot_end > range_start
}

fn speaker_rows(payload: &Value) -> Vec<Value> {
    let mut rows = Vec::new();
    for chunk in array_field(payload, "chunks") {
        if chunk.get("type").and_then(Value::as_str) != Some("audio") {
            continue;
        }
        let label = chunk
            .get("speaker_label")
            .cloned()
            .filter(|value| !value.is_null())
            .unwrap_or(Value::Null);
        let mut row = Map::new();
        row.insert(
            "sentence_id".to_string(),
            chunk.get("sentence_id").cloned().unwrap_or(Value::Null),
        );
        row.insert(
            "speaker_source".to_string(),
            chunk.get("speaker_source").cloned().unwrap_or(Value::Null),
        );
        row.insert("time".to_string(), Value::String(field(&chunk, "time")));
        row.insert("text".to_string(), Value::String(field(&chunk, "markdown")));
        row.insert(
            "has_embedding".to_string(),
            Value::Bool(
                chunk
                    .get("has_embedding")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            ),
        );
        row.insert(
            "actionable".to_string(),
            Value::Bool(
                chunk
                    .get("speaker_actionable")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            ),
        );
        row.insert("speaker".to_string(), label);
        rows.push(Value::Object(row));
    }
    rows
}

fn speaker_result(
    day: &str,
    stream: &str,
    segment: &str,
    payload: &Value,
    rows: &[Value],
) -> Value {
    let mut result = Map::new();
    result.insert("day".to_string(), Value::String(day.to_string()));
    result.insert("stream".to_string(), Value::String(stream.to_string()));
    result.insert(
        "segment_key".to_string(),
        Value::String(segment.to_string()),
    );
    result.insert(
        "speaker_labels".to_string(),
        payload
            .get("speaker_labels")
            .cloned()
            .filter(|value| !value.is_null())
            .unwrap_or_else(|| Value::Object(Map::new())),
    );
    result.insert("sentences".to_string(), Value::Array(rows.to_vec()));
    Value::Object(result)
}

fn row_speaker_text(row: &Value) -> (String, String) {
    let label = row.get("speaker").unwrap_or(&Value::Null);
    if label.is_null()
        || label
            .get("confidence_state")
            .and_then(Value::as_str)
            .is_some_and(|value| value == "unknown")
    {
        return ("unknown voice".to_string(), "unknown".to_string());
    }
    (
        label
            .get("name")
            .or_else(|| label.get("entity_id"))
            .and_then(Value::as_str)
            .unwrap_or("unknown voice")
            .to_string(),
        label
            .get("confidence_state")
            .or_else(|| label.get("confidence"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
    )
}

fn array_field(value: &Value, key: &str) -> Vec<Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn field(item: &Value, key: &str) -> String {
    display_value(item.get(key))
}

fn display_value(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Bool(true)) => "True".to_string(),
        Some(Value::Bool(false)) => "False".to_string(),
        Some(Value::Null) | None => "None".to_string(),
        Some(other) => other.to_string(),
    }
}

fn transcripts_error(error: ClientError) -> CommandOutput {
    transcripts_error_preserving_stdout(error, &[])
}

fn transcripts_error_preserving_stdout(
    error: ClientError,
    stdout_lines: &[String],
) -> CommandOutput {
    let stderr = match error {
        ClientError::Unreachable { .. } => SERVICE_DOWN_MESSAGE.to_string(),
        other => other.message().to_string(),
    };
    CommandOutput {
        stdout: if stdout_lines.is_empty() {
            String::new()
        } else {
            format!("{}\n", stdout_lines.join("\n"))
        },
        stderr: format!("{stderr}\n"),
        exit: 1,
    }
}

fn stdout_line(value: impl AsRef<str>) -> CommandOutput {
    CommandOutput::success(format!("{}\n", value.as_ref()))
}

fn stdout(lines: Vec<String>) -> CommandOutput {
    CommandOutput::success(format!("{}\n", lines.join("\n")))
}

fn stderr(value: impl AsRef<str>) -> CommandOutput {
    CommandOutput::failure(format!("{}\n", value.as_ref()), 1)
}
