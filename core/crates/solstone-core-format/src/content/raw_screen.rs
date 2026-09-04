// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::NaiveDate;
use std::collections::{BTreeMap, BTreeSet};
use std::iter::Peekable;
use std::str::Chars;

use serde_json::{Map, Value, json};

use super::{JsonObject, ProducedChunks, ScreenTalentRawScreen, json_truthy, recorded_chunk};
use crate::segment::{is_date_key, segment_parse};

const TMUX_PROJECTION_GUIDE: &str = "## Tmux change encoding\n\nEach Tmux observation advances visible pane state scoped by session, window id, and pane id. `snapshot` sets the complete pane to `lines` joined with newline characters. `splice` replaces `delete_count` lines at zero-based `start_line` with `lines`. `unchanged` preserves the prior pane; `disappeared` removes it. Geometry and active state apply to this observation. Links preserve visible labels and targets as data.";

pub(super) fn render(rel: &str, records: &[JsonObject]) -> ProducedChunks {
    let mut skipped = 0usize;
    let mut frames = Vec::new();
    for (index, record) in records.iter().enumerate() {
        if index == 0 && !record.contains_key("timestamp") && record.contains_key("raw") {
            continue;
        }
        if record.contains_key("timestamp") {
            frames.push(record);
        } else {
            skipped += 1;
        }
    }
    frames.sort_by(|left, right| {
        timestamp_seconds(left)
            .partial_cmp(&timestamp_seconds(right))
            .expect("timestamp_seconds always returns a finite value")
    });

    let (base_timestamp, base_hour, base_minute, base_second) = base_time(rel);
    let chunks = frames
        .into_iter()
        .map(|frame| render_frame(base_timestamp, base_hour, base_minute, base_second, frame))
        .collect();

    ProducedChunks {
        chunks,
        agent_override: Some("screen".to_string()),
        header: Some(screen_header(rel)),
        error: (skipped > 0)
            .then(|| format!("Skipped {skipped} entries missing 'timestamp' field in {rel}")),
        warnings: Vec::new(),
    }
}

/// Screen-talent-only projection for canonical tmux producer envelopes.
///
/// A record that does not satisfy the complete typed envelope stays on the
/// ordinary renderer. This is deliberately fail-open to the lossless generic
/// representation: a producer revision or malformed record may cost context,
/// but it cannot silently lose owner evidence.
pub(super) fn render_for_screen_talent(rel: &str, records: &[JsonObject]) -> ScreenTalentRawScreen {
    let mut pane_state = BTreeMap::<PaneKey, String>::new();
    let mut visible_panes = BTreeMap::<WindowKey, BTreeSet<String>>::new();
    let mut skipped = 0usize;
    let mut frames = Vec::new();
    for (source_order, record) in records.iter().enumerate() {
        if source_order == 0 && !record.contains_key("timestamp") && record.contains_key("raw") {
            continue;
        }
        if record.contains_key("timestamp") {
            frames.push((source_order, record));
        } else {
            skipped += 1;
        }
    }
    frames.sort_by(|(left_order, left), (right_order, right)| {
        timestamp_seconds(left)
            .partial_cmp(&timestamp_seconds(right))
            .expect("timestamp_seconds always returns a finite value")
            .then_with(|| left_order.cmp(right_order))
    });
    let has_tmux_projection = frames
        .iter()
        .any(|(_, frame)| canonical_tmux(frame).is_some());

    let (base_timestamp, base_hour, base_minute, base_second) = base_time(rel);
    let mut tmux_chunk_indices = Vec::new();
    let chunks = frames
        .into_iter()
        .enumerate()
        .map(|(chunk_index, (_, frame))| {
            let Some(tmux) = canonical_tmux(frame) else {
                return render_frame(base_timestamp, base_hour, base_minute, base_second, frame);
            };
            tmux_chunk_indices.push(chunk_index);
            let offset_ms = timestamp_millis(frame);
            let heading = timestamp_heading(base_hour, base_minute, base_second, offset_ms);
            let window_key = WindowKey {
                session: tmux.session.to_owned(),
                window_id: tmux.window.id.to_owned(),
            };
            let now_visible = tmux
                .panes
                .iter()
                .map(|pane| pane.id.to_owned())
                .collect::<BTreeSet<_>>();
            let previously_visible = visible_panes
                .insert(window_key.clone(), now_visible.clone())
                .unwrap_or_default();
            let mut pane_changes = Vec::new();

            for pane_id in previously_visible.difference(&now_visible) {
                pane_state.remove(&PaneKey {
                    window: window_key.clone(),
                    pane_id: pane_id.clone(),
                });
                pane_changes.push(json!({"id": pane_id, "op": "disappeared"}));
            }
            for pane in tmux.panes {
                let key = PaneKey {
                    window: window_key.clone(),
                    pane_id: pane.id.to_owned(),
                };
                let SanitizedTerminal { text, links } = sanitize_terminal(pane.content);
                let operation = match pane_state.insert(key, text.clone()) {
                    None => json!({"op":"snapshot", "lines": split_lines(&text)}),
                    Some(previous) if previous == text => json!({"op":"unchanged"}),
                    Some(previous) => line_splice(&previous, &text),
                };
                let mut projected = Map::from_iter([
                    ("id".to_owned(), json!(pane.id)),
                    ("index".to_owned(), json!(pane.index)),
                    ("active".to_owned(), json!(pane.active)),
                    (
                        "geometry".to_owned(),
                        json!([pane.left, pane.top, pane.width, pane.height]),
                    ),
                ]);
                projected.extend(
                    operation
                        .as_object()
                        .expect("pane operation is an object")
                        .clone(),
                );
                if !links.is_empty() {
                    projected.insert("links".to_owned(), json!(links));
                }
                pane_changes.push(Value::Object(projected));
            }
            let projected = json!({
                "session": tmux.session,
                "window": {
                    "id": tmux.window.id,
                    "index": tmux.window.index,
                    "name": tmux.window.name,
                },
                "panes": pane_changes,
            });
            let content = format!(
                "{heading}\n\n**Tmux observation:**\n\n```json\n{}\n```\n",
                serde_json::to_string(&projected).expect("tmux projection is JSON")
            );
            recorded_chunk(content, base_timestamp.saturating_add(offset_ms), frame)
        })
        .collect();

    ScreenTalentRawScreen {
        chunks,
        agent_override: Some("screen".to_string()),
        header: Some(if has_tmux_projection {
            format!("{}\n\n{TMUX_PROJECTION_GUIDE}", screen_header(rel))
        } else {
            screen_header(rel)
        }),
        error: (skipped > 0)
            .then(|| format!("Skipped {skipped} entries missing 'timestamp' field in {rel}")),
        warnings: Vec::new(),
        tmux_chunk_indices,
    }
}

fn render_frame(
    base_timestamp: i64,
    base_hour: i64,
    base_minute: i64,
    base_second: i64,
    frame: &JsonObject,
) -> crate::content::IndexChunk {
    let offset_ms = timestamp_millis(frame);
    let heading = timestamp_heading(base_hour, base_minute, base_second, offset_ms);
    let mut lines = vec![heading, String::new()];
    if let Some(analysis) = frame.get("analysis").and_then(Value::as_object) {
        let category = analysis
            .get("primary")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        lines.push(format!("**Category:** {category}"));
        lines.push(String::new());
        if let Some(description) = analysis
            .get("visual_description")
            .and_then(Value::as_str)
            .filter(|description| !description.is_empty())
        {
            lines.push(description.to_owned());
            lines.push(String::new());
        }
    }
    if let Some(content) = frame.get("content").and_then(Value::as_object) {
        for (category, value) in content {
            if !json_truthy(Some(value)) {
                continue;
            }
            let formatted = format_category(category, value);
            if !formatted.is_empty() {
                lines.push(formatted);
            }
        }
    }
    recorded_chunk(
        lines.join("\n"),
        base_timestamp.saturating_add(offset_ms),
        frame,
    )
}

fn timestamp_heading(base_hour: i64, base_minute: i64, base_second: i64, offset_ms: i64) -> String {
    let clock_ms = (base_hour * 3_600_000 + base_minute * 60_000 + base_second * 1_000)
        .saturating_add(offset_ms)
        .rem_euclid(86_400_000);
    let hour = clock_ms / 3_600_000;
    let minute = (clock_ms / 60_000).rem_euclid(60);
    let second = (clock_ms / 1_000).rem_euclid(60);
    let millisecond = clock_ms.rem_euclid(1_000);
    if millisecond == 0 {
        format!("### {hour:02}:{minute:02}:{second:02}")
    } else {
        format!("### {hour:02}:{minute:02}:{second:02}.{millisecond:03}")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct WindowKey {
    session: String,
    window_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PaneKey {
    window: WindowKey,
    pane_id: String,
}

struct TmuxEnvelope<'a> {
    session: &'a str,
    window: TmuxWindow<'a>,
    panes: Vec<TmuxPane<'a>>,
}

struct TmuxWindow<'a> {
    id: &'a str,
    index: u64,
    name: &'a str,
}

struct TmuxPane<'a> {
    id: &'a str,
    index: u64,
    left: i64,
    top: i64,
    width: u64,
    height: u64,
    active: bool,
    content: &'a str,
}

fn canonical_tmux(frame: &JsonObject) -> Option<TmuxEnvelope<'_>> {
    if frame
        .get("analysis")
        .and_then(Value::as_object)
        .and_then(|analysis| analysis.get("primary"))
        .and_then(Value::as_str)
        != Some("tmux")
    {
        return None;
    }
    let content = frame.get("content")?.as_object()?;
    if content.len() != 1 {
        return None;
    }
    let tmux = content.get("tmux")?.as_object()?;
    if tmux.len() != 4 {
        return None;
    }
    let session = nonempty_string(tmux.get("session"))?;
    let window_object = tmux.get("window")?.as_object()?;
    let window = parse_window(window_object, false)?;
    let windows = tmux.get("windows")?.as_array()?;
    let mut window_ids = BTreeSet::new();
    let mut active_windows = 0usize;
    let mut current_active_windows = 0usize;
    for roster_window in windows {
        let roster_object = roster_window.as_object()?;
        let roster = parse_window(roster_object, true)?;
        if !window_ids.insert(roster.id) {
            return None;
        }
        if roster_object.get("active").and_then(Value::as_bool) == Some(true) {
            active_windows += 1;
            if roster.id == window.id && roster.index == window.index && roster.name == window.name
            {
                current_active_windows += 1;
            }
        }
    }
    if active_windows != 1 || current_active_windows != 1 {
        return None;
    }
    let panes = tmux
        .get("panes")?
        .as_array()?
        .iter()
        .map(parse_pane)
        .collect::<Option<Vec<_>>>()?;
    if panes
        .iter()
        .map(|pane| pane.id)
        .collect::<BTreeSet<_>>()
        .len()
        != panes.len()
    {
        return None;
    }
    Some(TmuxEnvelope {
        session,
        window,
        panes,
    })
}

fn parse_window<'a>(window: &'a Map<String, Value>, roster: bool) -> Option<TmuxWindow<'a>> {
    if window.len() != usize::from(roster) + 3 {
        return None;
    }
    if roster && !window.get("active").is_some_and(Value::is_boolean) {
        return None;
    }
    Some(TmuxWindow {
        id: nonempty_string(window.get("id"))?,
        index: window.get("index")?.as_u64()?,
        name: window.get("name")?.as_str()?,
    })
}

fn parse_pane(value: &Value) -> Option<TmuxPane<'_>> {
    let pane = value.as_object()?;
    if pane.len() != 8 {
        return None;
    }
    Some(TmuxPane {
        id: nonempty_string(pane.get("id"))?,
        index: pane.get("index")?.as_u64()?,
        left: pane.get("left")?.as_i64()?,
        top: pane.get("top")?.as_i64()?,
        width: pane.get("width")?.as_u64()?,
        height: pane.get("height")?.as_u64()?,
        active: pane.get("active")?.as_bool()?,
        content: pane.get("content")?.as_str()?,
    })
}

fn nonempty_string(value: Option<&Value>) -> Option<&str> {
    value?.as_str().filter(|value| !value.is_empty())
}

fn split_lines(text: &str) -> Vec<&str> {
    text.split('\n').collect()
}

fn line_splice(previous: &str, current: &str) -> Value {
    let previous = split_lines(previous);
    let current = split_lines(current);
    let prefix = previous
        .iter()
        .zip(&current)
        .take_while(|(left, right)| left == right)
        .count();
    let suffix = previous[prefix..]
        .iter()
        .rev()
        .zip(current[prefix..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    json!({
        "op": "splice",
        "start_line": prefix,
        "delete_count": previous.len() - prefix - suffix,
        "lines": current[prefix..current.len() - suffix],
    })
}

#[derive(Debug, PartialEq, Eq)]
struct SanitizedTerminal {
    text: String,
    links: Vec<Value>,
}

fn sanitize_terminal(content: &str) -> SanitizedTerminal {
    let mut output = String::new();
    let mut links = Vec::new();
    let mut active_link: Option<(String, usize)> = None;
    let mut characters = content.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\u{1b}' => match characters.next() {
                Some('[') => consume_csi(&mut characters),
                Some(']') => {
                    let command = consume_control_string(&mut characters, true);
                    update_osc_link(&command, &output, &mut active_link, &mut links);
                }
                Some('P' | 'X' | '^' | '_') => {
                    consume_control_string(&mut characters, false);
                }
                Some(_intermediate @ '\u{20}'..='\u{2f}') => {
                    consume_escape_sequence(&mut characters);
                }
                Some(_) | None => {}
            },
            '\u{9b}' => consume_csi(&mut characters),
            '\u{9d}' => {
                let command = consume_control_string(&mut characters, true);
                update_osc_link(&command, &output, &mut active_link, &mut links);
            }
            '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}' => {
                consume_control_string(&mut characters, false);
            }
            '\n' | '\t' => output.push(character),
            _ if !character.is_control() => output.push(character),
            _ => {}
        }
    }
    if let Some((target, start)) = active_link {
        links.push(json!({"label": &output[start..], "target": target}));
    }
    SanitizedTerminal {
        text: output,
        links,
    }
}

fn consume_csi(characters: &mut Peekable<Chars<'_>>) {
    for parameter in characters.by_ref() {
        if ('\u{40}'..='\u{7e}').contains(&parameter) {
            break;
        }
    }
}

fn consume_escape_sequence(characters: &mut Peekable<Chars<'_>>) {
    for value in characters.by_ref() {
        if ('\u{30}'..='\u{7e}').contains(&value) {
            break;
        }
    }
}

fn consume_control_string(characters: &mut Peekable<Chars<'_>>, bell_terminates: bool) -> String {
    let mut command = String::new();
    while let Some(value) = characters.next() {
        if value == '\u{9c}' || (bell_terminates && value == '\u{7}') {
            break;
        }
        if value == '\u{1b}' && characters.peek() == Some(&'\\') {
            characters.next();
            break;
        }
        command.push(value);
    }
    command
}

fn update_osc_link(
    command: &str,
    output: &str,
    active_link: &mut Option<(String, usize)>,
    links: &mut Vec<Value>,
) {
    let Some(uri) = command
        .strip_prefix("8;")
        .and_then(|value| value.split_once(';').map(|(_, uri)| uri))
    else {
        return;
    };
    if uri.is_empty() {
        if let Some((target, start)) = active_link.take() {
            links.push(json!({"label": &output[start..], "target": target}));
        }
    } else {
        if let Some((target, start)) = active_link.replace((uri.to_owned(), output.len())) {
            links.push(json!({"label": &output[start..], "target": target}));
        }
    }
}

fn base_time(rel: &str) -> (i64, i64, i64, i64) {
    let Some(times) = segment_parse(rel) else {
        return (0, 0, 0, 0);
    };
    let hour = i64::from(times.hour);
    let minute = i64::from(times.minute);
    let second = i64::from(times.second);
    let base_timestamp = rel
        .split('/')
        .find(|part| is_date_key(part))
        .and_then(|day| NaiveDate::parse_from_str(day, "%Y%m%d").ok())
        .and_then(|day| {
            day.and_hms_opt(times.hour.into(), times.minute.into(), times.second.into())
        })
        .map(|time| time.and_utc().timestamp_millis())
        .unwrap_or(0);
    (base_timestamp, hour, minute, second)
}

fn timestamp_seconds(frame: &JsonObject) -> f64 {
    frame
        .get("timestamp")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .unwrap_or(0.0)
}

fn timestamp_millis(frame: &JsonObject) -> i64 {
    (timestamp_seconds(frame) * 1_000.0).round() as i64
}

fn screen_header(rel: &str) -> String {
    let stem = rel
        .rsplit('/')
        .next()
        .unwrap_or(rel)
        .strip_suffix(".jsonl")
        .unwrap_or(rel);
    let (position, connector) = parse_screen_filename(stem);
    if position == "unknown" || connector == "unknown" {
        "# Frame Analyses".to_string()
    } else {
        format!("# Frame Analyses ({position} - {connector})")
    }
}

fn parse_screen_filename(filename: &str) -> (&str, &str) {
    let Some(prefix) = filename.strip_suffix("_screen") else {
        return ("unknown", "unknown");
    };
    let Some((position, connector)) = prefix.rsplit_once('_') else {
        return ("unknown", "unknown");
    };
    let position_valid = !position.is_empty()
        && position
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-');
    let connector_valid = !connector.is_empty()
        && connector
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    if position_valid && connector_valid {
        (position, connector)
    } else {
        ("unknown", "unknown")
    }
}

fn format_category(category: &str, content: &Value) -> String {
    if category == "meeting"
        && let Some(content) = content.as_object()
    {
        return format_meeting(content);
    }
    match content {
        Value::String(content) => {
            format!("**{}:**\n\n{}\n", python_title(category), content.trim())
        }
        Value::Object(content) => format!(
            "**{}:**\n\n```json\n{}\n```\n",
            python_title(category),
            serde_json::to_string_pretty(content).unwrap_or_default()
        ),
        _ => String::new(),
    }
}

fn format_meeting(content: &Map<String, Value>) -> String {
    let mut lines = vec![
        format!(
            "**Meeting** ({})",
            content
                .get("platform")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        ),
        String::new(),
    ];
    if let Some(participants) = content.get("participants").and_then(Value::as_array)
        && !participants.is_empty()
    {
        lines.push("**Participants:**".to_string());
        for participant in participants {
            let Some(participant) = participant.as_object() else {
                continue;
            };
            let name = participant
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Unknown");
            let status = participant
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let video = participant
                .get("video")
                .is_some_and(|value| json_truthy(Some(value)));
            lines.push(format!(
                "- {} {name} ({status})",
                if video { "📹" } else { "🔇" }
            ));
        }
        lines.push(String::new());
    }
    if let Some(screen_share) = content.get("screen_share").and_then(Value::as_object)
        && !screen_share.is_empty()
    {
        let presenter = screen_share.get("presenter").and_then(Value::as_str);
        let description = screen_share
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("");
        let formatted_text = screen_share
            .get("formatted_text")
            .and_then(Value::as_str)
            .unwrap_or("");
        let presenter = presenter
            .map(|value| format!(" by {value}"))
            .unwrap_or_default();
        lines.push(format!("**Screen Share{presenter}:**"));
        if !description.is_empty() {
            lines.push(format!("*{description}*"));
        }
        lines.push(String::new());
        if !formatted_text.is_empty() {
            lines.push(formatted_text.trim().to_string());
            lines.push(String::new());
        }
    }
    lines.join("\n")
}

fn python_title(value: &str) -> String {
    let mut result = String::new();
    let mut capitalize = true;
    for character in value.chars() {
        if character.is_alphanumeric() {
            if capitalize {
                result.extend(character.to_uppercase());
            } else {
                result.extend(character.to_lowercase());
            }
            capitalize = false;
        } else {
            result.push(character);
            capitalize = true;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;
    use crate::content::OccurrenceTimeMs;

    fn record(timestamp: Value, marker: &str) -> JsonObject {
        json!({"timestamp": timestamp, "content": {"marker": marker}})
            .as_object()
            .expect("record object")
            .clone()
    }

    fn tmux_record(timestamp: f64, window_id: &str, panes: Value) -> JsonObject {
        json!({
            "frame_id": 1,
            "timestamp": timestamp,
            "requests": [],
            "analysis": {
                "visual_description": "derived description must not be repeated",
                "primary": "tmux",
                "secondary": "none",
                "overlap": false
            },
            "content": {"tmux": {
                "session": "owner/session",
                "window": {"id": window_id, "index": 2, "name": "dev"},
                "windows": [
                    {"id": window_id, "index": 2, "name": "dev", "active": true},
                    {"id": "@roster-only", "index": 8, "name": "omit me", "active": false}
                ],
                "panes": panes
            }}
        })
        .as_object()
        .expect("tmux frame")
        .clone()
    }

    fn pane(id: &str, content: &str) -> Value {
        json!({
            "id": id,
            "index": 0,
            "left": 0,
            "top": 0,
            "width": 80,
            "height": 24,
            "active": true,
            "content": content
        })
    }

    fn projected_json(content: &str) -> Value {
        let json = content
            .split_once("```json\n")
            .and_then(|(_, rest)| rest.split_once("\n```"))
            .map(|(json, _)| json)
            .expect("projected JSON fence");
        serde_json::from_str(json).expect("valid projected JSON")
    }

    #[test]
    fn fractional_offsets_are_sorted_numerically_and_rendered_to_milliseconds() {
        let records = vec![
            record(json!(1.5), "late"),
            record(json!(0.75), "early"),
            record(json!(1), "middle"),
        ];
        let produced = render("20260304/workstation/090000_300/screen.jsonl", &records);

        assert_eq!(produced.chunks.len(), 3);
        assert!(produced.chunks[0].content.starts_with("### 09:00:00.750\n"));
        assert!(produced.chunks[0].content.contains("early"));
        assert!(produced.chunks[1].content.starts_with("### 09:00:01\n"));
        assert!(produced.chunks[1].content.contains("middle"));
        assert!(produced.chunks[2].content.starts_with("### 09:00:01.500\n"));
        assert!(produced.chunks[2].content.contains("late"));
        assert_eq!(
            produced
                .chunks
                .iter()
                .map(|chunk| chunk.occurrence_time_ms)
                .collect::<Vec<_>>(),
            vec![
                Some(OccurrenceTimeMs(1_772_614_800_750)),
                Some(OccurrenceTimeMs(1_772_614_801_000)),
                Some(OccurrenceTimeMs(1_772_614_801_500)),
            ]
        );
    }

    #[test]
    fn equal_numeric_offsets_preserve_source_row_order() {
        let records = vec![record(json!(1.5), "first"), record(json!(1.500), "second")];
        let produced = render("20260304/workstation/090000_300/screen.jsonl", &records);

        assert!(produced.chunks[0].content.contains("first"));
        assert!(produced.chunks[1].content.contains("second"));
    }

    #[test]
    fn signed_zero_offsets_are_equal_and_preserve_source_row_order() {
        let records = vec![
            record(json!(0.0), "positive-zero-first"),
            record(json!(-0.0), "negative-zero-second"),
        ];
        let produced = render("20260304/workstation/090000_300/screen.jsonl", &records);

        assert!(produced.chunks[0].content.contains("positive-zero-first"));
        assert!(produced.chunks[1].content.contains("negative-zero-second"));
    }

    #[test]
    fn producer_golden_projects_by_typed_envelope_not_stream_name() {
        let fixture = include_str!("../../tests/data/golden/tmux-observer-envelope-main.jsonl");
        assert_eq!(fixture.as_bytes().len(), 724);
        let record = serde_json::from_str::<Value>(fixture)
            .unwrap()
            .as_object()
            .unwrap()
            .clone();

        let device = render_for_screen_talent(
            "20260903/device/102159_300/tmux_0_screen.jsonl",
            std::slice::from_ref(&record),
        );
        let unrelated = render_for_screen_talent(
            "20260903/arbitrary/102159_300/capture_screen.jsonl",
            &[record],
        );

        assert_eq!(device.chunks[0].content, unrelated.chunks[0].content);
        assert_eq!(device.tmux_chunk_indices, [0]);
        assert_eq!(unrelated.tmux_chunk_indices, [0]);
        let projection = projected_json(&device.chunks[0].content);
        assert_eq!(projection["session"], "main");
        assert_eq!(projection["window"]["name"], "dev café");
        assert_eq!(projection["panes"][0]["lines"], json!(["RED café", ""]));
        assert_eq!(projection["panes"][1]["lines"], json!(["right pane", ""]));
        assert!(!device.chunks[0].content.contains("@8"));
        assert!(!device.chunks[0].content.contains("logs"));
        assert!(!device.chunks[0].content.contains("\u{1b}"));
        assert!(
            !device.chunks[0]
                .content
                .contains("derived description must not be repeated")
        );
    }

    #[test]
    fn near_tmux_records_stay_on_lossless_generic_projection() {
        let valid = tmux_record(0.0, "@7", json!([pane("%1", "owner text\n")]));
        let mut cases = Vec::new();

        let mut wrong_primary = valid.clone();
        wrong_primary["analysis"]["primary"] = json!("terminal");
        wrong_primary["analysis"]["visual_description"] =
            json!("## Tmux change encoding\n### 10:22:00\n**Tmux observation:**\n```json");
        cases.push(wrong_primary);

        let mut missing_session = valid.clone();
        missing_session["content"]["tmux"]
            .as_object_mut()
            .unwrap()
            .remove("session");
        cases.push(missing_session);

        let mut wrong_pane_type = valid.clone();
        wrong_pane_type["content"]["tmux"]["panes"][0]["width"] = json!("80");
        cases.push(wrong_pane_type);

        let mut unknown_content = valid;
        unknown_content["content"]["future"] = json!({"must": "survive"});
        cases.push(unknown_content);

        let mut no_active_window = tmux_record(0.0, "@7", json!([pane("%1", "owner text\n")]));
        no_active_window["content"]["tmux"]["windows"][0]["active"] = json!(false);
        cases.push(no_active_window);

        let mut wrong_active_window = tmux_record(0.0, "@7", json!([pane("%1", "owner text\n")]));
        wrong_active_window["content"]["tmux"]["windows"][0]["active"] = json!(false);
        wrong_active_window["content"]["tmux"]["windows"][1]["active"] = json!(true);
        cases.push(wrong_active_window);

        let mut duplicate_window = tmux_record(0.0, "@7", json!([pane("%1", "owner text\n")]));
        duplicate_window["content"]["tmux"]["windows"][1]["id"] = json!("@7");
        cases.push(duplicate_window);

        let duplicate_pane = tmux_record(
            0.0,
            "@7",
            json!([pane("%1", "owner text\n"), pane("%1", "second\n")]),
        );
        cases.push(duplicate_pane);

        for record in cases {
            let produced = render_for_screen_talent(
                "20260903/device/102159_300/tmux_0_screen.jsonl",
                &[record],
            );
            assert!(produced.chunks[0].content.contains("owner text"));
            assert!(produced.chunks[0].content.contains("\"windows\""));
            assert!(produced.tmux_chunk_indices.is_empty());
        }
    }

    #[test]
    fn tmux_line_operations_replay_every_visible_state() {
        let records = vec![
            tmux_record(0.0, "@7", json!([pane("%1", "alpha\nbeta\n")])),
            tmux_record(
                1.25,
                "@7",
                json!([pane(
                    "%1",
                    "alpha\nBETA\n\u{1b}]8;;https://example.test/a?B=1\u{1b}\\link label\u{1b}]8;;\u{7}\n",
                )]),
            ),
            tmux_record(2.0, "@7", json!([])),
            tmux_record(3.0, "@8", json!([pane("%2", "other window\n")])),
            tmux_record(4.0, "@7", json!([pane("%1", "restored\n")])),
        ];
        let produced =
            render_for_screen_talent("20260903/device/102159_300/tmux_0_screen.jsonl", &records);
        assert_eq!(produced.chunks.len(), records.len());

        let mut states = BTreeMap::<String, Vec<String>>::new();
        let expected = [
            [(
                "%1".to_owned(),
                vec!["alpha".to_owned(), "beta".to_owned(), String::new()],
            )]
            .into_iter()
            .collect(),
            [(
                "%1".to_owned(),
                vec![
                    "alpha".to_owned(),
                    "BETA".to_owned(),
                    "link label".to_owned(),
                    String::new(),
                ],
            )]
            .into_iter()
            .collect(),
            BTreeMap::new(),
        ];

        for (index, chunk) in produced.chunks.iter().take(3).enumerate() {
            let projection = projected_json(&chunk.content);
            for pane in projection["panes"].as_array().unwrap() {
                let id = pane["id"].as_str().unwrap().to_owned();
                match pane["op"].as_str().unwrap() {
                    "snapshot" => {
                        states.insert(
                            id,
                            pane["lines"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|line| line.as_str().unwrap().to_owned())
                                .collect(),
                        );
                    }
                    "splice" => {
                        let lines = states.get_mut(&id).unwrap();
                        let start = pane["start_line"].as_u64().unwrap() as usize;
                        let delete = pane["delete_count"].as_u64().unwrap() as usize;
                        let replacement = pane["lines"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|line| line.as_str().unwrap().to_owned());
                        lines.splice(start..start + delete, replacement);
                    }
                    "unchanged" => {}
                    "disappeared" => {
                        states.remove(&id);
                    }
                    operation => panic!("unknown operation {operation}"),
                }
            }
            assert_eq!(states, expected[index]);
        }

        let changed = projected_json(&produced.chunks[1].content);
        assert_eq!(changed["panes"][0]["op"], "splice");
        assert_eq!(changed["panes"][0]["links"][0]["label"], "link label");
        assert_eq!(
            changed["panes"][0]["links"][0]["target"],
            "https://example.test/a?B=1"
        );
        assert_eq!(
            projected_json(&produced.chunks[3].content)["window"]["id"],
            "@8"
        );
        assert_eq!(
            projected_json(&produced.chunks[4].content)["panes"][0]["op"],
            "snapshot"
        );
    }

    #[test]
    fn unchanged_tmux_viewport_still_emits_a_timed_observation() {
        let records = vec![
            tmux_record(0.0, "@7", json!([pane("%1", "same\n")])),
            tmux_record(1.5, "@7", json!([pane("%1", "same\n")])),
        ];
        let produced =
            render_for_screen_talent("20260903/device/102159_300/tmux_0_screen.jsonl", &records);

        assert_eq!(produced.chunks.len(), 2);
        assert_eq!(
            projected_json(&produced.chunks[1].content)["panes"][0]["op"],
            "unchanged"
        );
        assert!(produced.chunks[1].content.starts_with("### 10:22:00.500\n"));
    }

    #[test]
    fn tmux_projection_explains_the_stateful_encoding() {
        let record = tmux_record(0.0, "@7", json!([pane("%1", "same\n")]));
        let produced =
            render_for_screen_talent("20260903/device/102159_300/tmux_0_screen.jsonl", &[record]);
        let header = produced.header.unwrap();

        assert!(header.contains("scoped by session, window id, and pane id"));
        for operation in ["`snapshot`", "`splice`", "`unchanged`", "`disappeared`"] {
            assert!(header.contains(operation));
        }
        assert!(header.contains("zero-based `start_line`"));
    }

    #[test]
    fn terminal_control_strings_and_c1_forms_never_become_visible_text() {
        let sanitized = sanitize_terminal(concat!(
            "café",
            "\u{1b}Pprivate-dcs\u{1b}\\",
            "\u{1b}Xprivate-sos\u{1b}\\",
            "\u{1b}^private-pm\u{1b}\\",
            "\u{1b}_private-apc\u{1b}\\",
            "\u{90}private-c1-dcs\u{9c}",
            "\u{98}private-c1-sos\u{9c}",
            "\u{9e}private-c1-pm\u{9c}",
            "\u{9f}private-c1-apc\u{9c}",
            "\u{9b}31mRED\u{9b}0m",
            "\u{9d}8;;https://example.test/λ\u{9c}labelλ\u{9d}8;;\u{9c}",
            "\u{1b}(Bdone"
        ));

        assert_eq!(sanitized.text, "caféREDlabelλdone");
        assert_eq!(
            sanitized.links,
            vec![json!({"label":"labelλ", "target":"https://example.test/λ"})]
        );
        for hidden in ["private", "31m", "8;;", "(B"] {
            assert!(!sanitized.text.contains(hidden));
        }
    }

    #[test]
    fn a_new_osc_link_opener_closes_and_preserves_the_previous_link() {
        let sanitized = sanitize_terminal(concat!(
            "\u{1b}]8;;https://a.test/λ\u{1b}\\label α",
            "\u{1b}]8;;https://b.test/μ\u{1b}\\label β",
            "\u{1b}]8;;\u{1b}\\"
        ));

        assert_eq!(sanitized.text, "label αlabel β");
        assert_eq!(
            sanitized.links,
            vec![
                json!({"label":"label α", "target":"https://a.test/λ"}),
                json!({"label":"label β", "target":"https://b.test/μ"}),
            ]
        );
    }
}
