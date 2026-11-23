// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded recognizers and turn parsers for structured chat and transcript exports
//! (JSON, CSV, TSV, and HTML).
//!
//! Triage rules and the generic text importer share these parsers so a structured
//! file is admitted only when it is both recognized as a known export format and
//! readable into bounded conversation turns.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use jiff::Timestamp;
use jiff::civil::{Date, DateTime, Time};
use jiff::tz::Offset;
use serde_json::Value;

/// Maximum byte size of a structured export file inspected or imported.
pub const MAX_STRUCTURED_EXPORT_BYTES: usize = 32 * 1024 * 1024;
/// Maximum JSON container nesting depth (`{` / `[`) allowed before `serde_json` parses.
pub const MAX_JSON_DEPTH: usize = 32;
/// Maximum number of turns/segments/messages accepted from a single export file.
pub const MAX_TURNS: usize = 200_000;
/// Maximum number of rows accepted in a CSV or TSV export.
pub const MAX_CSV_ROWS: usize = 200_000;
/// Maximum number of columns accepted per CSV row.
pub const MAX_CSV_COLS: usize = 16;
/// Maximum byte length of a single CSV/TSV cell or spoken transcript segment.
pub const MAX_CELL_BYTES: usize = 8 * 1024;
/// Maximum byte length of a single spoken transcript turn (aligned with caption row cap).
pub const MAX_SPOKEN_TURN_BYTES: usize = 8 * 1024;
/// Maximum byte length of a single written chat message body.
pub const MAX_WRITTEN_TURN_BYTES: usize = 1024 * 1024;
/// Maximum byte length of a speaker label.
pub const MAX_SPEAKER_BYTES: usize = 256;
/// Maximum element nesting depth allowed in Telegram Desktop HTML.
pub const MAX_HTML_DEPTH: usize = 64;
/// Maximum total tag count scanned in Telegram Desktop HTML.
pub const MAX_HTML_TAGS: usize = 2_000_000;
/// Maximum byte length of a single `<...>` HTML tag.
pub const MAX_HTML_TAG_BYTES: usize = 16 * 1024;
/// Maximum attribute count parsed on a single HTML tag.
pub const MAX_HTML_ATTRS: usize = 32;
/// Maximum relative recording offset accepted in seconds (30 days).
const MAX_OFFSET_SECONDS: u64 = 86_400 * 30;
/// Earliest accepted Unix epoch timestamp (1970-01-01T00:00:00Z).
const MIN_UNIX_SECONDS: i64 = 0;
/// Latest accepted Unix epoch timestamp (2100-01-01T00:00:00Z).
const MAX_UNIX_SECONDS: i64 = 4_102_444_800;

/// Timing carried by one parsed structured export turn.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum StructuredTiming {
    Sent(Timestamp),
    NoSendInstant,
    Caption { start: String, end: String },
}

/// One turn parsed from a structured export.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StructuredTurn {
    pub(crate) speaker: Option<String>,
    pub(crate) text: String,
    pub(crate) offset: u64,
    pub(crate) timing: StructuredTiming,
}

/// Whether `rule_id` is a structured-export rule admitted for `extension`.
pub(crate) fn rule_matches_extension(rule_id: &str, extension: &str) -> bool {
    match extension.to_ascii_lowercase().as_str() {
        "json" => matches!(
            rule_id,
            "whisper_json"
                | "macwhisper_json"
                | "podcast_transcript_json"
                | "telegram_json"
                | "slack_json"
        ),
        "csv" => rule_id == "fireflies_csv",
        "tsv" => rule_id == "whisper_tsv",
        "html" => rule_id == "telegram_html",
        _ => false,
    }
}

/// Read up to `MAX_STRUCTURED_EXPORT_BYTES` from `path`.
/// Returns `Ok(None)` when the file exceeds the cap.
pub(crate) fn read_bounded_bytes(path: &Path) -> Result<Option<Vec<u8>>, std::io::Error> {
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() > MAX_STRUCTURED_EXPORT_BYTES as u64 {
        return Ok(None);
    }
    let file = File::open(path)?;
    let mut bytes = Vec::with_capacity((metadata.len() as usize).min(64 * 1024));
    file.take(MAX_STRUCTURED_EXPORT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_STRUCTURED_EXPORT_BYTES {
        return Ok(None);
    }
    Ok(Some(bytes))
}

/// Decode a structured export as strict UTF-8 (allowing a UTF-8 BOM, rejecting NUL bytes).
pub(crate) fn decode_structured_utf8(bytes: &[u8]) -> Option<&str> {
    if bytes.len() > MAX_STRUCTURED_EXPORT_BYTES {
        return None;
    }
    let raw = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let text = std::str::from_utf8(raw).ok()?;
    (!text.contains('\0')).then_some(text)
}

fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

fn format_cue_ms(ms: u64) -> String {
    let hours = ms / 3_600_000;
    let minutes = (ms / 60_000) % 60;
    let seconds = (ms / 1_000) % 60;
    let millis = ms % 1_000;
    format!("{hours:02}:{minutes:02}:{seconds:02}.{millis:03}")
}

fn seconds_to_ms(secs: f64) -> Option<u64> {
    if !secs.is_finite() || secs < 0.0 || secs > MAX_OFFSET_SECONDS as f64 {
        return None;
    }
    let ms = (secs * 1_000.0).round();
    if !ms.is_finite() || ms < 0.0 {
        return None;
    }
    Some(ms as u64)
}

fn clean_speaker(raw: Option<&str>) -> Option<Option<String>> {
    let Some(raw) = raw else {
        return Some(None);
    };
    let trimmed = raw.trim();
    if trimmed.len() > MAX_SPEAKER_BYTES {
        return None;
    }
    Some((!trimmed.is_empty()).then(|| trimmed.to_owned()))
}

/// Fast single-pass byte scan verifying JSON container nesting never exceeds `max_depth`.
fn json_within_depth(text: &str, max_depth: usize) -> bool {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for &byte in text.as_bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else {
            match byte {
                b'"' => in_string = true,
                b'{' | b'[' => {
                    depth += 1;
                    if depth > max_depth {
                        return false;
                    }
                }
                b'}' | b']' => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
    }
    !in_string && depth == 0
}

fn parse_bounded_json(text: &str, expected_start: u8) -> Option<Value> {
    if text.len() > MAX_STRUCTURED_EXPORT_BYTES {
        return None;
    }
    let trimmed = strip_bom(text).trim();
    if trimmed.as_bytes().first().copied() != Some(expected_start) {
        return None;
    }
    if !json_within_depth(trimmed, MAX_JSON_DEPTH) {
        return None;
    }
    serde_json::from_str::<Value>(trimmed).ok()
}

fn is_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    bytes.iter().enumerate().all(|(idx, &byte)| {
        if matches!(idx, 8 | 13 | 18 | 23) {
            byte == b'-'
        } else {
            byte.is_ascii_hexdigit()
        }
    })
}

/// OpenAI Whisper CLI `.tsv` output (`start\tend\ttext` with millisecond integer offsets).
pub(crate) fn parse_whisper_tsv(text: &str) -> Option<Vec<StructuredTurn>> {
    if text.len() > MAX_STRUCTURED_EXPORT_BYTES {
        return None;
    }
    let mut lines = strip_bom(text)
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.trim().is_empty());
    if lines.next()?.trim() != "start\tend\ttext" {
        return None;
    }
    let mut turns = Vec::new();
    for line in lines {
        if turns.len() >= MAX_CSV_ROWS || line.len() > MAX_CELL_BYTES {
            return None;
        }
        let mut parts = line.splitn(3, '\t');
        let start_ms = parts.next()?.trim().parse::<u64>().ok()?;
        let end_ms = parts.next()?.trim().parse::<u64>().ok()?;
        let body = parts.next()?.trim();
        if end_ms < start_ms
            || start_ms / 1_000 > MAX_OFFSET_SECONDS
            || end_ms / 1_000 > MAX_OFFSET_SECONDS
            || body.is_empty()
            || body.len() > MAX_SPOKEN_TURN_BYTES
        {
            return None;
        }
        turns.push(StructuredTurn {
            speaker: None,
            text: body.to_owned(),
            offset: start_ms / 1_000,
            timing: StructuredTiming::Caption {
                start: format_cue_ms(start_ms),
                end: format_cue_ms(end_ms),
            },
        });
    }
    (!turns.is_empty()).then_some(turns)
}

/// OpenAI Whisper CLI `.json` output (`text`, `language`, `segments` with second offsets).
pub(crate) fn parse_whisper_json(text: &str) -> Option<Vec<StructuredTurn>> {
    let root = parse_bounded_json(text, b'{')?;
    let obj = root.as_object()?;
    if !obj.get("text").is_some_and(Value::is_string)
        || !obj.get("language").is_some_and(Value::is_string)
    {
        return None;
    }
    let segments = obj.get("segments").and_then(Value::as_array)?;
    if segments.is_empty() || segments.len() > MAX_TURNS {
        return None;
    }
    // Segments with UUID string IDs belong to MacWhisper's millisecond JSON schema.
    if segments
        .iter()
        .any(|seg| seg.get("id").and_then(Value::as_str).is_some_and(is_uuid))
    {
        return None;
    }
    let mut turns = Vec::with_capacity(segments.len().min(1024));
    for seg in segments {
        let seg_obj = seg.as_object()?;
        let start_ms = seconds_to_ms(seg_obj.get("start")?.as_f64()?)?;
        let end_ms = seconds_to_ms(seg_obj.get("end")?.as_f64()?)?;
        if end_ms < start_ms {
            return None;
        }
        let body = seg_obj.get("text")?.as_str()?.trim();
        if body.len() > MAX_SPOKEN_TURN_BYTES {
            return None;
        }
        let speaker = match seg_obj.get("speaker") {
            None | Some(Value::Null) => None,
            Some(Value::String(raw)) => clean_speaker(Some(raw))?,
            Some(_) => return None,
        };
        if !body.is_empty() {
            turns.push(StructuredTurn {
                speaker,
                text: body.to_owned(),
                offset: start_ms / 1_000,
                timing: StructuredTiming::Caption {
                    start: format_cue_ms(start_ms),
                    end: format_cue_ms(end_ms),
                },
            });
        }
    }
    (!turns.is_empty()).then_some(turns)
}

/// MacWhisper structured `.json` export (`text`, `segments` with UUID IDs, `speaker`, or `words`).
pub(crate) fn parse_macwhisper_json(text: &str) -> Option<Vec<StructuredTurn>> {
    let root = parse_bounded_json(text, b'{')?;
    let obj = root.as_object()?;
    if !obj.get("text").is_some_and(Value::is_string) {
        return None;
    }
    let segments = obj.get("segments").and_then(Value::as_array)?;
    if segments.is_empty() || segments.len() > MAX_TURNS {
        return None;
    }
    let any_uuid_id = segments
        .iter()
        .any(|seg| seg.get("id").and_then(Value::as_str).is_some_and(is_uuid));
    // Keep mutually exclusive with `whisper_json` when `language` is present without UUID IDs.
    if obj.get("language").is_some_and(Value::is_string) && !any_uuid_id {
        return None;
    }
    let mut has_macwhisper_marker = any_uuid_id;
    let all_integer_bounds = segments.iter().all(|seg| {
        seg.get("start").is_some_and(Value::is_u64) && seg.get("end").is_some_and(Value::is_u64)
    });
    let millis_units = any_uuid_id && all_integer_bounds;

    let mut turns = Vec::with_capacity(segments.len().min(1024));
    for seg in segments {
        let seg_obj = seg.as_object()?;
        let (start_ms, end_ms) = if millis_units {
            let s = seg_obj.get("start")?.as_u64()?;
            let e = seg_obj.get("end")?.as_u64()?;
            if e < s || s / 1_000 > MAX_OFFSET_SECONDS || e / 1_000 > MAX_OFFSET_SECONDS {
                return None;
            }
            (s, e)
        } else {
            let s = seconds_to_ms(seg_obj.get("start")?.as_f64()?)?;
            let e = seconds_to_ms(seg_obj.get("end")?.as_f64()?)?;
            if e < s {
                return None;
            }
            (s, e)
        };
        let body = seg_obj.get("text")?.as_str()?.trim();
        if body.len() > MAX_SPOKEN_TURN_BYTES {
            return None;
        }
        let speaker = match seg_obj.get("speaker") {
            None | Some(Value::Null) => None,
            Some(Value::String(raw)) => {
                has_macwhisper_marker = true;
                clean_speaker(Some(raw))?
            }
            Some(_) => return None,
        };
        if let Some(words) = seg_obj.get("words") {
            let words = words.as_array()?;
            if !words.is_empty()
                && words.iter().all(|w| {
                    w.get("start").is_some_and(Value::is_number)
                        && w.get("end").is_some_and(Value::is_number)
                        && (w.get("text").is_some_and(Value::is_string)
                            || w.get("word").is_some_and(Value::is_string))
                })
            {
                has_macwhisper_marker = true;
            }
        }
        if !body.is_empty() {
            turns.push(StructuredTurn {
                speaker,
                text: body.to_owned(),
                offset: start_ms / 1_000,
                timing: StructuredTiming::Caption {
                    start: format_cue_ms(start_ms),
                    end: format_cue_ms(end_ms),
                },
            });
        }
    }
    (has_macwhisper_marker && !turns.is_empty()).then_some(turns)
}

fn parse_numeric_or_string_seconds(value: &Value) -> Option<u64> {
    let secs = match value {
        Value::Number(num) => num.as_f64()?,
        Value::String(raw) => raw.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    seconds_to_ms(secs)
}

/// Podcasting 2.0 JSON transcript (`version`, `segments[].{startTime, endTime, body, speaker}`).
pub(crate) fn parse_podcast_transcript_json(text: &str) -> Option<Vec<StructuredTurn>> {
    let root = parse_bounded_json(text, b'{')?;
    let obj = root.as_object()?;
    let version = obj.get("version").and_then(Value::as_str)?;
    if version.is_empty()
        || version.len() > 32
        || !version.contains('.')
        || !version
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return None;
    }
    let segments = obj.get("segments").and_then(Value::as_array)?;
    if segments.is_empty() || segments.len() > MAX_TURNS {
        return None;
    }

    struct Pending {
        speaker: Option<String>,
        start_ms: u64,
        end_ms: u64,
        raw_body: String,
    }

    let mut pending: Vec<Pending> = Vec::with_capacity(segments.len().min(1024));
    for seg in segments {
        let seg_obj = seg.as_object()?;
        let start_ms = parse_numeric_or_string_seconds(seg_obj.get("startTime")?)?;
        let end_ms = parse_numeric_or_string_seconds(seg_obj.get("endTime")?)?;
        if end_ms < start_ms {
            return None;
        }
        let body = seg_obj.get("body")?.as_str()?;
        let speaker = match seg_obj.get("speaker") {
            None | Some(Value::Null) => None,
            Some(Value::String(raw)) => clean_speaker(Some(raw))?,
            Some(_) => return None,
        };
        if let Some(last) = pending.last_mut()
            && last.speaker == speaker
            && start_ms <= last.end_ms.saturating_add(1)
            && (body.trim().is_empty()
                || last
                    .raw_body
                    .chars()
                    .last()
                    .is_some_and(char::is_whitespace))
        {
            if last.raw_body.len() + body.len() > MAX_SPOKEN_TURN_BYTES {
                return None;
            }
            last.raw_body.push_str(body);
            last.end_ms = last.end_ms.max(end_ms);
            continue;
        }
        if body.trim().is_empty() {
            continue;
        }
        if body.len() > MAX_SPOKEN_TURN_BYTES {
            return None;
        }
        pending.push(Pending {
            speaker,
            start_ms,
            end_ms,
            raw_body: body.to_owned(),
        });
    }

    let mut turns = Vec::with_capacity(pending.len());
    for item in pending {
        let trimmed = item.raw_body.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.len() > MAX_SPOKEN_TURN_BYTES {
            return None;
        }
        turns.push(StructuredTurn {
            speaker: item.speaker,
            text: trimmed.to_owned(),
            offset: item.start_ms / 1_000,
            timing: StructuredTiming::Caption {
                start: format_cue_ms(item.start_ms),
                end: format_cue_ms(item.end_ms),
            },
        });
    }
    (!turns.is_empty()).then_some(turns)
}

fn parse_unix_seconds(value: &Value) -> Option<Timestamp> {
    let secs = match value {
        Value::String(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty()
                || trimmed.len() > 12
                || !trimmed.bytes().all(|byte| byte.is_ascii_digit())
            {
                return None;
            }
            trimmed.parse::<i64>().ok()?
        }
        Value::Number(num) => num.as_i64()?,
        _ => return None,
    };
    if !(MIN_UNIX_SECONDS..=MAX_UNIX_SECONDS).contains(&secs) {
        return None;
    }
    Timestamp::from_second(secs).ok()
}

fn looks_like_telegram_iso_date(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    bytes.len() >= 19
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
        && bytes[10] == b'T'
        && bytes[11..13].iter().all(u8::is_ascii_digit)
        && bytes[13] == b':'
        && bytes[14..16].iter().all(u8::is_ascii_digit)
        && bytes[16] == b':'
        && bytes[17..19].iter().all(u8::is_ascii_digit)
}

fn extract_telegram_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            if text.len() > MAX_WRITTEN_TURN_BYTES {
                return None;
            }
            Some(text.clone())
        }
        Value::Array(parts) => {
            let mut out = String::new();
            for part in parts {
                let piece = match part {
                    Value::String(s) => s.as_str(),
                    Value::Object(obj) => {
                        if !obj.get("type").is_some_and(Value::is_string) {
                            return None;
                        }
                        obj.get("text")?.as_str()?
                    }
                    _ => return None,
                };
                if out.len() + piece.len() > MAX_WRITTEN_TURN_BYTES {
                    return None;
                }
                out.push_str(piece);
            }
            Some(out)
        }
        _ => None,
    }
}

/// Telegram Desktop single-chat JSON export (`result.json` from Export chat history).
/// Whole-account multi-chat exports (`chats.list` / `personal_information`) are refused.
pub(crate) fn parse_telegram_json(text: &str) -> Option<Vec<StructuredTurn>> {
    let root = parse_bounded_json(text, b'{')?;
    let obj = root.as_object()?;
    if obj.contains_key("chats") || obj.contains_key("personal_information") {
        return None;
    }
    let root_type = match obj.get("type") {
        None => None,
        Some(Value::String(s)) => Some(s.as_str()),
        Some(_) => return None,
    };
    let has_telegram_root_type = matches!(
        root_type,
        Some(
            "personal_chat"
                | "bot_chat"
                | "private_group"
                | "private_supergroup"
                | "public_supergroup"
                | "private_channel"
                | "public_channel"
                | "saved_messages"
        )
    );
    let messages = obj.get("messages").and_then(Value::as_array)?;
    if messages.is_empty() || messages.len() > MAX_TURNS {
        return None;
    }
    let mut saw_telegram_marker = has_telegram_root_type;
    let mut turns = Vec::with_capacity(messages.len().min(1024));
    for message in messages {
        let msg = message.as_object()?;
        let kind = msg.get("type").and_then(Value::as_str)?;
        if !matches!(kind, "message" | "service") {
            return None;
        }
        let timing = if let Some(unix) = msg.get("date_unixtime") {
            saw_telegram_marker = true;
            StructuredTiming::Sent(parse_unix_seconds(unix)?)
        } else {
            let date = msg.get("date").and_then(Value::as_str)?;
            if !looks_like_telegram_iso_date(date) {
                return None;
            }
            if let Ok(ts) = date.parse::<Timestamp>() {
                StructuredTiming::Sent(ts)
            } else {
                StructuredTiming::NoSendInstant
            }
        };
        if kind == "service" {
            continue;
        }
        if msg.get("from_id").is_some_and(Value::is_string)
            || msg.get("text_entities").is_some_and(Value::is_array)
        {
            saw_telegram_marker = true;
        }
        let has_sender_field = msg.contains_key("from") || msg.contains_key("from_id");
        if !has_sender_field {
            return None;
        }
        let from_str = match msg.get("from") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.as_str()),
            Some(_) => return None,
        };
        let from_id_str = match msg.get("from_id") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.as_str()),
            Some(_) => return None,
        };
        let speaker = match clean_speaker(from_str)? {
            Some(name) => Some(name),
            None => clean_speaker(from_id_str)?,
        };
        let body = extract_telegram_text(msg.get("text")?)?;
        if body.trim().is_empty() {
            continue;
        }
        turns.push(StructuredTurn {
            speaker,
            text: body,
            offset: 0,
            timing,
        });
    }
    (saw_telegram_marker && !turns.is_empty()).then_some(turns)
}

fn parse_slack_ts(raw: &str) -> Option<Timestamp> {
    let (secs_part, micros_part) = raw.split_once('.')?;
    if !(9..=12).contains(&secs_part.len())
        || micros_part.len() != 6
        || !secs_part.bytes().all(|byte| byte.is_ascii_digit())
        || !micros_part.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let secs = secs_part.parse::<i64>().ok()?;
    let micros = micros_part.parse::<i32>().ok()?;
    if !(MIN_UNIX_SECONDS..=MAX_UNIX_SECONDS).contains(&secs) {
        return None;
    }
    Timestamp::new(secs, micros * 1_000).ok()
}

/// Slack channel export JSON (top-level array of Slack message objects).
pub(crate) fn parse_slack_json(text: &str) -> Option<Vec<StructuredTurn>> {
    let root = parse_bounded_json(text, b'[')?;
    let items = root.as_array()?;
    if items.is_empty() || items.len() > MAX_TURNS {
        return None;
    }
    let mut saw_slack_marker = false;
    let mut turns = Vec::with_capacity(items.len().min(1024));
    for item in items {
        let obj = item.as_object()?;
        let kind = obj.get("type").and_then(Value::as_str)?;
        let ts = parse_slack_ts(obj.get("ts").and_then(Value::as_str)?)?;
        if kind != "message" {
            continue;
        }
        if obj.get("user").is_some_and(Value::is_string)
            || obj.get("bot_id").is_some_and(Value::is_string)
            || obj.get("client_msg_id").is_some_and(Value::is_string)
            || obj.get("team").is_some_and(Value::is_string)
            || obj.contains_key("blocks")
            || obj.get("user_profile").is_some_and(Value::is_object)
        {
            saw_slack_marker = true;
        }
        let body = match obj.get("text") {
            Some(Value::String(s)) => s.as_str(),
            None if obj.contains_key("files")
                || obj.contains_key("attachments")
                || obj.contains_key("subtype") =>
            {
                ""
            }
            _ => return None,
        };
        if body.len() > MAX_WRITTEN_TURN_BYTES {
            return None;
        }
        if body.trim().is_empty() {
            continue;
        }
        let profile = obj.get("user_profile").and_then(Value::as_object);
        let candidate = profile
            .and_then(|p| p.get("display_name").and_then(Value::as_str))
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                profile
                    .and_then(|p| p.get("real_name").and_then(Value::as_str))
                    .filter(|s| !s.trim().is_empty())
            })
            .or_else(|| {
                profile
                    .and_then(|p| p.get("name").and_then(Value::as_str))
                    .filter(|s| !s.trim().is_empty())
            })
            .or_else(|| {
                obj.get("user")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
            })
            .or_else(|| {
                obj.get("username")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
            })
            .or_else(|| {
                obj.get("bot_id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
            });
        let speaker = clean_speaker(candidate)?;
        turns.push(StructuredTurn {
            speaker,
            text: body.to_owned(),
            offset: 0,
            timing: StructuredTiming::Sent(ts),
        });
    }
    (saw_slack_marker && !turns.is_empty()).then_some(turns)
}

/// Strict, bounded RFC 4180 CSV parser.
fn parse_bounded_csv(text: &str) -> Option<Vec<Vec<String>>> {
    if text.len() > MAX_STRUCTURED_EXPORT_BYTES {
        return None;
    }
    let bytes = strip_bom(text).as_bytes();
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut idx = 0usize;

    while idx < bytes.len() {
        if rows.len() >= MAX_CSV_ROWS || row.len() >= MAX_CSV_COLS {
            return None;
        }
        let mut cell = Vec::new();
        if bytes[idx] == b'"' {
            idx += 1;
            let mut closed = false;
            while idx < bytes.len() {
                if bytes[idx] == b'"' {
                    if bytes.get(idx + 1) == Some(&b'"') {
                        if cell.len() >= MAX_CELL_BYTES {
                            return None;
                        }
                        cell.push(b'"');
                        idx += 2;
                    } else {
                        idx += 1;
                        closed = true;
                        break;
                    }
                } else {
                    if cell.len() >= MAX_CELL_BYTES {
                        return None;
                    }
                    cell.push(bytes[idx]);
                    idx += 1;
                }
            }
            if !closed {
                return None;
            }
            row.push(String::from_utf8(cell).ok()?);
            match bytes.get(idx).copied() {
                Some(b',') => {
                    idx += 1;
                    if idx == bytes.len() {
                        if row.len() >= MAX_CSV_COLS {
                            return None;
                        }
                        row.push(String::new());
                        rows.push(std::mem::take(&mut row));
                    }
                }
                Some(b'\r') => {
                    idx += 1;
                    if bytes.get(idx) == Some(&b'\n') {
                        idx += 1;
                    }
                    rows.push(std::mem::take(&mut row));
                }
                Some(b'\n') => {
                    idx += 1;
                    rows.push(std::mem::take(&mut row));
                }
                None => {
                    rows.push(std::mem::take(&mut row));
                }
                Some(_) => return None,
            }
        } else {
            let start = idx;
            while idx < bytes.len() && !matches!(bytes[idx], b',' | b'\r' | b'\n') {
                if bytes[idx] == b'"' || idx - start >= MAX_CELL_BYTES {
                    return None;
                }
                idx += 1;
            }
            let value = std::str::from_utf8(&bytes[start..idx]).ok()?;
            row.push(value.to_owned());
            match bytes.get(idx).copied() {
                Some(b',') => {
                    idx += 1;
                    if idx == bytes.len() {
                        if row.len() >= MAX_CSV_COLS {
                            return None;
                        }
                        row.push(String::new());
                        rows.push(std::mem::take(&mut row));
                    }
                }
                Some(b'\r') => {
                    idx += 1;
                    if bytes.get(idx) == Some(&b'\n') {
                        idx += 1;
                    }
                    rows.push(std::mem::take(&mut row));
                }
                Some(b'\n') => {
                    idx += 1;
                    rows.push(std::mem::take(&mut row));
                }
                None => {
                    rows.push(std::mem::take(&mut row));
                }
                Some(_) => unreachable!(),
            }
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
    Some(rows)
}

fn parse_clock_ms(raw: &str) -> Option<u64> {
    let trimmed = raw.trim();
    let (clock, frac_ms) = match trimmed.split_once('.') {
        Some((c, f)) => {
            if f.is_empty() || f.len() > 3 || !f.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let val = f.parse::<u64>().ok()?;
            let ms = match f.len() {
                1 => val * 100,
                2 => val * 10,
                3 => val,
                _ => return None,
            };
            (c, ms)
        }
        None => (trimmed, 0),
    };
    let parts: Vec<_> = clock.split(':').collect();
    let seconds = match parts.as_slice() {
        [m, s] => {
            if !(1..=2).contains(&m.len())
                || s.len() != 2
                || !m.bytes().all(|b| b.is_ascii_digit())
                || !s.bytes().all(|b| b.is_ascii_digit())
            {
                return None;
            }
            let m = m.parse::<u64>().ok()?;
            let s = s.parse::<u64>().ok()?;
            if m >= 60 || s >= 60 {
                return None;
            }
            m * 60 + s
        }
        [h, m, s] => {
            if !(1..=3).contains(&h.len())
                || m.len() != 2
                || s.len() != 2
                || !h.bytes().all(|b| b.is_ascii_digit())
                || !m.bytes().all(|b| b.is_ascii_digit())
                || !s.bytes().all(|b| b.is_ascii_digit())
            {
                return None;
            }
            let h = h.parse::<u64>().ok()?;
            let m = m.parse::<u64>().ok()?;
            let s = s.parse::<u64>().ok()?;
            if m >= 60 || s >= 60 {
                return None;
            }
            h.checked_mul(3600)?.checked_add(m * 60 + s)?
        }
        _ => return None,
    };
    if seconds > MAX_OFFSET_SECONDS {
        return None;
    }
    seconds.checked_mul(1_000)?.checked_add(frac_ms)
}

/// Fireflies CSV transcript export (`Speaker, Start Time, End Time, Sentence`).
pub(crate) fn parse_fireflies_csv(text: &str) -> Option<Vec<StructuredTurn>> {
    let rows = parse_bounded_csv(text)?;
    if rows.len() < 2 {
        return None;
    }
    let header = &rows[0];
    if header.len() != 4 {
        return None;
    }
    if !header[0].trim().eq_ignore_ascii_case("speaker")
        || !header[1].trim().eq_ignore_ascii_case("start time")
        || !header[2].trim().eq_ignore_ascii_case("end time")
        || !header[3].trim().eq_ignore_ascii_case("sentence")
    {
        return None;
    }
    let mut turns = Vec::with_capacity(rows.len().saturating_sub(1).min(1024));
    for row in &rows[1..] {
        if row.len() == 1 && row[0].trim().is_empty() {
            continue;
        }
        if row.len() != 4 {
            return None;
        }
        let speaker = clean_speaker(Some(&row[0]))?;
        let start_raw = row[1].trim();
        let end_raw = row[2].trim();
        let start_ms = parse_clock_ms(start_raw)?;
        let end_ms = parse_clock_ms(end_raw)?;
        if end_ms < start_ms {
            return None;
        }
        let body = row[3].trim();
        if body.len() > MAX_SPOKEN_TURN_BYTES {
            return None;
        }
        if !body.is_empty() {
            turns.push(StructuredTurn {
                speaker,
                text: body.to_owned(),
                offset: start_ms / 1_000,
                timing: StructuredTiming::Caption {
                    start: start_raw.to_owned(),
                    end: end_raw.to_owned(),
                },
            });
        }
    }
    (!turns.is_empty()).then_some(turns)
}

fn is_void_html_tag(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

fn has_css_class(class_attr: &str, needle: &str) -> bool {
    class_attr.split_ascii_whitespace().any(|c| c == needle)
}

/// Decode common HTML entities in a single pass with a strict output cap.
/// Unknown named entities are preserved verbatim without recursion.
fn append_decoded_html(raw: &str, out: &mut String, limit: usize) -> bool {
    let bytes = raw.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'&'
            && let Some(rel_semi) = bytes[i + 1..bytes.len().min(i + 16)]
                .iter()
                .position(|&b| b == b';')
        {
            let entity = &raw[i + 1..i + 1 + rel_semi];
            let decoded: Option<char> = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                    u32::from_str_radix(&entity[2..], 16)
                        .ok()
                        .filter(|&cp| cp != 0)
                        .and_then(char::from_u32)
                }
                _ if entity.starts_with('#') => entity[1..]
                    .parse::<u32>()
                    .ok()
                    .filter(|&cp| cp != 0)
                    .and_then(char::from_u32),
                _ => None,
            };
            if let Some(ch) = decoded {
                if out.len() + ch.len_utf8() > limit {
                    return false;
                }
                out.push(ch);
                i += rel_semi + 2;
                continue;
            }
        }
        let next = raw[i..].chars().next().expect("valid UTF-8 char");
        if out.len() + next.len_utf8() > limit {
            return false;
        }
        out.push(next);
        i += next.len_utf8();
    }
    true
}

/// Parse Telegram Desktop HTML `title="DD.MM.YYYY HH:MM:SS UTC+HH:MM"`.
fn parse_telegram_html_title_time(title: &str) -> Option<StructuredTiming> {
    let parts: Vec<_> = title.split_ascii_whitespace().collect();
    if parts.len() < 2 || parts.len() > 3 {
        return None;
    }
    let mut d = parts[0].split('.');
    let day: i8 = d.next()?.parse().ok()?;
    let month: i8 = d.next()?.parse().ok()?;
    let year: i16 = d.next()?.parse().ok()?;
    if d.next().is_some() {
        return None;
    }
    let date = Date::new(year, month, day).ok()?;
    let mut t = parts[1].split(':');
    let hour: i8 = t.next()?.parse().ok()?;
    let minute: i8 = t.next()?.parse().ok()?;
    let second: i8 = t.next()?.parse().ok()?;
    if t.next().is_some() {
        return None;
    }
    let time = Time::new(hour, minute, second, 0).ok()?;
    let dt = DateTime::new(
        date.year(),
        date.month(),
        date.day(),
        time.hour(),
        time.minute(),
        time.second(),
        0,
    )
    .ok()?;
    let Some(zone_part) = parts.get(2) else {
        return Some(StructuredTiming::NoSendInstant);
    };
    let rest = zone_part.strip_prefix("UTC")?;
    if rest.is_empty() {
        let ts = Offset::UTC.to_timestamp(dt).ok()?;
        return Some(StructuredTiming::Sent(ts));
    }
    let (sign, digits) = if let Some(r) = rest.strip_prefix('+') {
        (1i32, r)
    } else {
        let r = rest.strip_prefix('-')?;
        (-1i32, r)
    };
    let (oh, om) = if let Some((h, m)) = digits.split_once(':') {
        (h.parse::<i32>().ok()?, m.parse::<i32>().ok()?)
    } else if digits.len() == 4 {
        (
            digits[..2].parse::<i32>().ok()?,
            digits[2..].parse::<i32>().ok()?,
        )
    } else if (1..=2).contains(&digits.len()) {
        (digits.parse::<i32>().ok()?, 0)
    } else {
        return None;
    };
    if oh > 23 || om > 59 {
        return None;
    }
    let offset = Offset::from_seconds(sign * (oh * 3600 + om * 60)).ok()?;
    let ts = offset.to_timestamp(dt).ok()?;
    Some(StructuredTiming::Sent(ts))
}

struct ParsedTag<'a> {
    name: String,
    closing: bool,
    self_closing: bool,
    class_attr: Option<String>,
    title_attr: Option<String>,
    _marker: std::marker::PhantomData<&'a str>,
}

fn parse_html_tag(raw_inside: &str) -> Option<ParsedTag<'_>> {
    let trimmed = raw_inside.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (closing, rest) = if let Some(r) = trimmed.strip_prefix('/') {
        (true, r.trim_start())
    } else {
        (false, trimmed)
    };
    let (self_closing, rest) = if !closing && let Some(r) = rest.strip_suffix('/') {
        (true, r.trim_end())
    } else {
        (false, rest)
    };
    let name_end = rest
        .find(|c: char| c.is_ascii_whitespace() || c == '/')
        .unwrap_or(rest.len());
    let name = &rest[..name_end];
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return None;
    }
    let lower_name = name.to_ascii_lowercase();
    let mut class_attr = None;
    let mut title_attr = None;
    if !closing {
        let mut cursor = &rest.as_bytes()[name_end..];
        let mut attr_count = 0usize;
        while !cursor.is_empty() {
            while let Some(&b) = cursor.first() {
                if b.is_ascii_whitespace() {
                    cursor = &cursor[1..];
                } else {
                    break;
                }
            }
            if cursor.is_empty() {
                break;
            }
            attr_count += 1;
            if attr_count > MAX_HTML_ATTRS {
                return None;
            }
            let key_end = cursor
                .iter()
                .position(|&b| b == b'=' || b.is_ascii_whitespace())
                .unwrap_or(cursor.len());
            let key = std::str::from_utf8(&cursor[..key_end]).ok()?;
            cursor = &cursor[key_end..];
            while let Some(&b) = cursor.first() {
                if b.is_ascii_whitespace() {
                    cursor = &cursor[1..];
                } else {
                    break;
                }
            }
            let value = if cursor.first() == Some(&b'=') {
                cursor = &cursor[1..];
                while let Some(&b) = cursor.first() {
                    if b.is_ascii_whitespace() {
                        cursor = &cursor[1..];
                    } else {
                        break;
                    }
                }
                match cursor.first().copied() {
                    Some(quote @ (b'"' | b'\'')) => {
                        cursor = &cursor[1..];
                        let end = cursor.iter().position(|&b| b == quote)?;
                        let v = std::str::from_utf8(&cursor[..end]).ok()?;
                        cursor = &cursor[end + 1..];
                        Some(v)
                    }
                    Some(_) => {
                        let end = cursor
                            .iter()
                            .position(u8::is_ascii_whitespace)
                            .unwrap_or(cursor.len());
                        let v = std::str::from_utf8(&cursor[..end]).ok()?;
                        cursor = &cursor[end..];
                        Some(v)
                    }
                    None => None,
                }
            } else {
                None
            };
            if let Some(val) = value {
                if key.eq_ignore_ascii_case("class") {
                    let mut decoded = String::new();
                    if !append_decoded_html(val, &mut decoded, MAX_HTML_TAG_BYTES) {
                        return None;
                    }
                    class_attr = Some(decoded);
                } else if key.eq_ignore_ascii_case("title") {
                    let mut decoded = String::new();
                    if !append_decoded_html(val, &mut decoded, MAX_HTML_TAG_BYTES) {
                        return None;
                    }
                    title_attr = Some(decoded);
                }
            }
        }
    }
    Some(ParsedTag {
        name: lower_name,
        closing,
        self_closing,
        class_attr,
        title_attr,
        _marker: std::marker::PhantomData,
    })
}

/// Telegram Desktop HTML chat export (`messages.html`).
pub(crate) fn parse_telegram_html(text: &str) -> Option<Vec<StructuredTurn>> {
    if text.len() > MAX_STRUCTURED_EXPORT_BYTES {
        return None;
    }
    let text = strip_bom(text);
    // Fast pre-filter before scanning tags.
    if !text.contains("chat_page")
        || !text.contains("page_body")
        || !text.contains("history")
        || !text.contains("message")
    {
        return None;
    }

    let bytes = text.as_bytes();
    let mut i = 0usize;
    let mut tag_count = 0usize;
    let mut stack: Vec<String> = Vec::new();

    let mut page_depth: Option<usize> = None;
    let mut history_depth: Option<usize> = None;
    let mut msg_depth: Option<usize> = None;
    let mut skip_depth: Option<usize> = None;
    let mut sender_depth: Option<usize> = None;
    let mut text_depth: Option<usize> = None;

    let mut saw_page = false;
    let mut saw_history = false;
    let mut saw_default_msg = false;

    let mut last_sender: Option<String> = None;
    let mut current_sender: Option<String> = None;
    let mut current_time: Option<StructuredTiming> = None;
    let mut current_text: Option<String> = None;
    let mut sender_buf = String::new();
    let mut text_buf = String::new();
    let mut turns: Vec<StructuredTurn> = Vec::new();

    let pop_to = |stack: &mut Vec<String>, closing_name: &str| {
        if let Some(pos) = stack.iter().rposition(|name| name == closing_name) {
            stack.truncate(pos);
        }
    };

    while i < bytes.len() {
        if bytes[i] == b'<' {
            // Comment <!-- ... -->
            if text[i..].starts_with("<!--") {
                let end = text[i + 4..].find("-->")?;
                i += 4 + end + 3;
                continue;
            }
            // DOCTYPE / XML declaration
            if bytes.get(i + 1).is_some_and(|&b| b == b'!' || b == b'?') {
                let end = bytes[i + 1..bytes.len().min(i + 1 + MAX_HTML_TAG_BYTES)]
                    .iter()
                    .position(|&b| b == b'>')?;
                i += 1 + end + 1;
                continue;
            }
            // Find end of tag, respecting quoted attribute values and byte cap.
            let mut j = i + 1;
            let limit = bytes.len().min(i + MAX_HTML_TAG_BYTES);
            let mut quote: Option<u8> = None;
            let mut found_end = None;
            while j < limit {
                let b = bytes[j];
                if let Some(q) = quote {
                    if b == q {
                        quote = None;
                    }
                } else if b == b'"' || b == b'\'' {
                    quote = Some(b);
                } else if b == b'>' {
                    found_end = Some(j);
                    break;
                }
                j += 1;
            }
            let end = found_end?;
            tag_count += 1;
            if tag_count > MAX_HTML_TAGS {
                return None;
            }
            let tag = parse_html_tag(&text[i + 1..end])?;
            i = end + 1;

            // Skip raw content inside <script> and <style>.
            if !tag.closing && !tag.self_closing && matches!(tag.name.as_str(), "script" | "style")
            {
                let needle = if tag.name == "script" {
                    "</script>"
                } else {
                    "</style>"
                };
                let rest = &text[i..];
                let lower_end = rest
                    .as_bytes()
                    .windows(needle.len())
                    .position(|w| w.eq_ignore_ascii_case(needle.as_bytes()))?;
                i += lower_end + needle.len();
                continue;
            }

            if tag.closing {
                pop_to(&mut stack, &tag.name);
                let new_depth = stack.len();
                if let Some(d) = sender_depth
                    && new_depth < d
                {
                    sender_depth = None;
                    let cleaned = clean_speaker(Some(&sender_buf))?;
                    sender_buf.clear();
                    if cleaned.is_some() {
                        current_sender = cleaned.clone();
                        last_sender = cleaned;
                    }
                }
                if let Some(d) = text_depth
                    && new_depth < d
                {
                    text_depth = None;
                    let trimmed = text_buf.trim();
                    if !trimmed.is_empty() {
                        current_text = Some(trimmed.to_owned());
                    }
                    text_buf.clear();
                }
                if let Some(d) = skip_depth
                    && new_depth < d
                {
                    skip_depth = None;
                }
                if let Some(d) = msg_depth
                    && new_depth < d
                {
                    msg_depth = None;
                    if let Some(body) = current_text.take() {
                        if turns.len() >= MAX_TURNS {
                            return None;
                        }
                        turns.push(StructuredTurn {
                            speaker: current_sender.take(),
                            text: body,
                            offset: 0,
                            timing: current_time
                                .take()
                                .unwrap_or(StructuredTiming::NoSendInstant),
                        });
                    }
                    current_sender = None;
                    current_time = None;
                }
                if let Some(d) = history_depth
                    && new_depth < d
                {
                    history_depth = None;
                }
                if let Some(d) = page_depth
                    && new_depth < d
                {
                    page_depth = None;
                }
                continue;
            }

            // Open or void/self-closing tag.
            if tag.name == "br" && text_depth.is_some() && skip_depth.is_none() {
                if text_buf.len() >= MAX_WRITTEN_TURN_BYTES {
                    return None;
                }
                text_buf.push('\n');
                continue;
            }

            let is_void = tag.self_closing || is_void_html_tag(&tag.name);
            let cur_depth = if is_void {
                stack.len() + 1
            } else {
                if stack.len() >= MAX_HTML_DEPTH {
                    return None;
                }
                stack.push(tag.name.clone());
                stack.len()
            };

            let classes = tag.class_attr.as_deref().unwrap_or("");
            if !is_void && tag.name == "div" {
                if page_depth.is_none()
                    && has_css_class(classes, "page_body")
                    && has_css_class(classes, "chat_page")
                {
                    page_depth = Some(cur_depth);
                    saw_page = true;
                } else if page_depth.is_some()
                    && history_depth.is_none()
                    && has_css_class(classes, "history")
                {
                    history_depth = Some(cur_depth);
                    saw_history = true;
                } else if history_depth.is_some()
                    && msg_depth.is_none()
                    && skip_depth.is_none()
                    && has_css_class(classes, "message")
                {
                    if has_css_class(classes, "service") {
                        last_sender = None;
                        skip_depth = Some(cur_depth);
                    } else if has_css_class(classes, "default") {
                        msg_depth = Some(cur_depth);
                        saw_default_msg = true;
                        let joined = has_css_class(classes, "joined");
                        current_sender = if joined { last_sender.clone() } else { None };
                        current_time = None;
                        current_text = None;
                    }
                } else if msg_depth.is_some() && skip_depth.is_none() {
                    if has_css_class(classes, "userpic_wrap")
                        || has_css_class(classes, "media_wrap")
                        || has_css_class(classes, "reply_to")
                        || has_css_class(classes, "forwarded")
                        || has_css_class(classes, "reactions")
                        || has_css_class(classes, "signature")
                    {
                        skip_depth = Some(cur_depth);
                    } else if sender_depth.is_none()
                        && text_depth.is_none()
                        && has_css_class(classes, "from_name")
                    {
                        sender_depth = Some(cur_depth);
                        sender_buf.clear();
                    } else if sender_depth.is_none()
                        && text_depth.is_none()
                        && has_css_class(classes, "date")
                        && has_css_class(classes, "details")
                    {
                        if let Some(title) = tag.title_attr.as_deref()
                            && let Some(parsed) = parse_telegram_html_title_time(title)
                        {
                            current_time = Some(parsed);
                        }
                    } else if sender_depth.is_none()
                        && text_depth.is_none()
                        && has_css_class(classes, "text")
                        && current_text.is_none()
                    {
                        text_depth = Some(cur_depth);
                        text_buf.clear();
                    }
                }
            } else if !is_void
                && msg_depth.is_some()
                && skip_depth.is_none()
                && tag.name == "span"
                && has_css_class(classes, "reactions")
            {
                skip_depth = Some(cur_depth);
            }
        } else {
            let next_lt = bytes[i..]
                .iter()
                .position(|&b| b == b'<')
                .map_or(bytes.len(), |rel| i + rel);
            let slice = &text[i..next_lt];
            i = next_lt;
            if skip_depth.is_some() {
                continue;
            }
            if sender_depth.is_some() {
                if !append_decoded_html(slice, &mut sender_buf, MAX_SPEAKER_BYTES * 2) {
                    return None;
                }
            } else if text_depth.is_some()
                && !append_decoded_html(slice, &mut text_buf, MAX_WRITTEN_TURN_BYTES)
            {
                return None;
            }
        }
    }

    // Refuse truncated Telegram HTML where container blocks did not close.
    if page_depth.is_some()
        || history_depth.is_some()
        || msg_depth.is_some()
        || text_depth.is_some()
        || sender_depth.is_some()
    {
        return None;
    }

    (saw_page && saw_history && saw_default_msg && !turns.is_empty()).then_some(turns)
}
