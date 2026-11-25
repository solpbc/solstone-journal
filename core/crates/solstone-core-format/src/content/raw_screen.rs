// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use jiff::tz::Offset;
use std::collections::{BTreeMap, BTreeSet};
use std::iter::Peekable;
use std::str::Chars;

use serde_json::{Map, Value, json};
use solstone_core_window_catalog::{ENGLISH, screen_category_label, strings, text};

use super::{
    JsonObject, ProducedChunks, ScreenTalentRawScreen, json_truthy, recorded_chunk,
    recorded_chunk_for_locale,
};
use crate::segment::{is_date_key, segment_parse};

const TMUX_PROJECTION_GUIDE: &str = "## Tmux change encoding\n\nEach Tmux change advances visible pane state scoped by session, window id, and pane id. `snapshot` sets the complete pane to `lines` joined with newline characters. `splice` replaces `delete_count` lines at zero-based `start_line` with `lines`. `unchanged` preserves the prior pane; `disappeared` removes it. Geometry and active state apply to this change. Links preserve visible labels and targets as data.";

#[allow(dead_code)]
pub(super) fn render(rel: &str, records: &[JsonObject]) -> ProducedChunks {
    render_for_locale(rel, records, ENGLISH)
}

pub(super) fn render_for_locale(rel: &str, records: &[JsonObject], locale: &str) -> ProducedChunks {
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
        .map(|frame| {
            render_frame(
                base_timestamp,
                base_hour,
                base_minute,
                base_second,
                frame,
                locale,
            )
        })
        .collect();

    ProducedChunks {
        chunks,
        agent_override: Some("screen".to_string()),
        header: Some(screen_header_with_capture(rel, records)),
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
                return render_frame(
                    base_timestamp,
                    base_hour,
                    base_minute,
                    base_second,
                    frame,
                    ENGLISH,
                );
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
                    "name": super::screen_text::redact_inline(tmux.window.name),
                },
                "panes": pane_changes,
            });
            let content = format!(
                "{heading}\n\n**Tmux window:**\n\n```json\n{}\n```\n",
                serde_json::to_string(&projected).expect("tmux projection is JSON")
            );
            recorded_chunk(content, base_timestamp.saturating_add(offset_ms), frame)
        })
        .collect();

    ScreenTalentRawScreen {
        chunks,
        agent_override: Some("screen".to_string()),
        header: Some(if has_tmux_projection {
            format!(
                "{}\n\n{TMUX_PROJECTION_GUIDE}",
                screen_header_with_capture(rel, records)
            )
        } else {
            screen_header_with_capture(rel, records)
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
    locale: &str,
) -> crate::content::IndexChunk {
    let offset_ms = timestamp_millis(frame);
    let heading = timestamp_heading(base_hour, base_minute, base_second, offset_ms);
    let mut lines = vec![heading, String::new()];
    if let Some(analysis) = frame.get("analysis").and_then(Value::as_object) {
        let category = match analysis.get("primary").and_then(Value::as_str) {
            Some(primary) if locale != ENGLISH => {
                screen_category_label(locale, primary).unwrap_or_else(|| primary.to_owned())
            }
            Some(primary) => primary.to_owned(),
            None => text(locale, "journal.window.content.raw.screen.unknown"),
        };
        lines.push(strings(
            locale,
            "journal.window.content.raw.screen.category",
            [("category", category.as_str())],
        ));
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
    if let Some(objects) = frame
        .get("detections")
        .and_then(|detections| crate::objects::objects_line_for_locale(detections, locale))
    {
        lines.push(objects);
        lines.push(String::new());
    }
    if let Some(content) = frame.get("content").and_then(Value::as_object) {
        for (category, value) in content {
            if !json_truthy(Some(value)) {
                continue;
            }
            let mut view;
            let value = if category == "tmux" {
                view = value.clone();
                super::omit_tmux_capture_fields(&mut view);
                sanitize_tmux_value(&mut view);
                &view
            } else {
                value
            };
            let formatted = format_category(category, value, locale);
            if !formatted.is_empty() {
                lines.push(formatted);
            }
        }
    }
    recorded_chunk_for_locale(
        lines.join("\n"),
        base_timestamp.saturating_add(offset_ms),
        frame,
        locale,
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
    let capture_fields = super::TMUX_PANE_CAPTURE_FIELDS
        .iter()
        .filter(|key| pane.contains_key(**key))
        .count();
    if pane.len() != 8 + capture_fields {
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

fn sanitize_tmux_value(tmux: &mut Value) {
    if let Some(panes) = tmux.get_mut("panes").and_then(Value::as_array_mut) {
        for pane in panes.iter_mut().filter_map(Value::as_object_mut) {
            if let Some(Value::String(content)) = pane.get_mut("content") {
                *content = sanitize_terminal(content).text;
            }
            if let Some(Value::Array(links)) = pane.get_mut("links") {
                for link in links.iter_mut().filter_map(Value::as_object_mut) {
                    for field in ["label", "target"] {
                        if let Some(Value::String(text)) = link.get_mut(field) {
                            *text = super::screen_text::redact_inline(text);
                        }
                    }
                }
            }
        }
    }
    if let Some(Value::String(name)) = tmux
        .get_mut("window")
        .and_then(Value::as_object_mut)
        .and_then(|window| window.get_mut("name"))
    {
        *name = super::screen_text::redact_inline(name);
    }
    if let Some(windows) = tmux.get_mut("windows").and_then(Value::as_array_mut) {
        for window in windows.iter_mut().filter_map(Value::as_object_mut) {
            if let Some(Value::String(name)) = window.get_mut("name") {
                *name = super::screen_text::redact_inline(name);
            }
        }
    }
}

fn sanitize_terminal(content: &str) -> SanitizedTerminal {
    let mut output = String::new();
    let mut links = Vec::new();
    let mut active_link: Option<(String, usize)> = None;
    let mut conceal = false;
    let mut characters = content.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\u{1b}' => match characters.next() {
                Some('[') => consume_csi(&mut characters, &mut conceal),
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
            '\u{9b}' => consume_csi(&mut characters, &mut conceal),
            '\u{9d}' => {
                let command = consume_control_string(&mut characters, true);
                update_osc_link(&command, &output, &mut active_link, &mut links);
            }
            '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}' => {
                consume_control_string(&mut characters, false);
            }
            '\n' => output.push('\n'),
            '\t' if !conceal => output.push('\t'),
            _ if !character.is_control() && !conceal => output.push(character),
            _ => {}
        }
    }
    if let Some((target, start)) = active_link {
        push_osc_link(&mut links, &output[start..], &target);
    }
    SanitizedTerminal {
        text: super::screen_text::redact_text_preserving_lines(&output),
        links,
    }
}

fn consume_csi(characters: &mut Peekable<Chars<'_>>, conceal: &mut bool) {
    let mut params = String::new();
    let mut final_byte = None;
    for parameter in characters.by_ref() {
        if ('\u{40}'..='\u{7e}').contains(&parameter) {
            final_byte = Some(parameter);
            break;
        }
        params.push(parameter);
    }
    if final_byte == Some('m')
        && params
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b';' | b':'))
    {
        if params.is_empty() {
            *conceal = false;
            return;
        }
        let mut parts = params.split(';');
        while let Some(part) = parts.next() {
            let (head, has_subparams) = match part.split_once(':') {
                Some((head, _)) => (head, true),
                None => (part, false),
            };
            let code = head.parse::<u32>().unwrap_or(0);
            match code {
                0 | 28 => *conceal = false,
                8 => *conceal = true,
                38 | 48 | 58 if !has_subparams => {
                    match parts.next().and_then(|next| next.parse::<u32>().ok()) {
                        Some(5) => {
                            parts.next();
                        }
                        Some(2) => {
                            parts.nth(2);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
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

fn push_osc_link(links: &mut Vec<Value>, label: &str, target: &str) {
    links.push(json!({
        "label": super::screen_text::redact_inline(label),
        "target": super::screen_text::redact_inline(target),
    }));
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
            push_osc_link(links, &output[start..], &target);
        }
    } else if let Some((target, start)) = active_link.replace((uri.to_owned(), output.len())) {
        push_osc_link(links, &output[start..], &target);
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
        .and_then(solstone_core_time::parse_day)
        .and_then(|day| {
            Offset::UTC
                .to_timestamp(day.to_datetime(times.clock()?))
                .ok()
        })
        .map(|time| time.as_millisecond())
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

/// The screen header, with a note on what recorded it when its capture
/// metadata names a phone or tablet. A phone recording's own "Screen Sharing"
/// panel and a picture-in-picture call read as sharing to a call otherwise.
fn screen_header_with_capture(rel: &str, records: &[JsonObject]) -> String {
    let header = screen_header(rel);
    let Some(meta) = records
        .first()
        .filter(|record| !record.contains_key("timestamp") && record.contains_key("raw"))
    else {
        return header;
    };
    let device = match meta.get("platform").and_then(Value::as_str) {
        Some("ios") => "iPhone or iPad",
        Some("android") => "Android phone or tablet",
        _ => return header,
    };
    let mut note = format!(
        "{header}\n\nThese frames come from a screen recording the owner made on their own {device}. \
         A screen-sharing or screen-recording panel in them is that device recording its own screen, \
         not the owner sharing their screen to a call. An app under a small picture-in-picture call \
         window is what the owner was looking at, not something shared in the call."
    );
    let sources = meta.get("sources").and_then(Value::as_array);
    if sources.is_some_and(|sources| {
        !sources.is_empty()
            && !sources
                .iter()
                .any(|source| source.as_str() == Some("audio"))
    }) {
        note.push_str(" No sound was recorded with it, so nothing in it was heard.");
    }
    note
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

fn format_category(category: &str, content: &Value, locale: &str) -> String {
    if category == "meeting"
        && let Some(content) = content.as_object()
    {
        return format_meeting(content, locale);
    }
    let heading = if locale == ENGLISH {
        python_title(category)
    } else {
        screen_category_label(locale, category).unwrap_or_else(|| python_title(category))
    };
    match content {
        Value::String(content) => {
            format!("**{heading}:**\n\n{}\n", content.trim())
        }
        Value::Object(content) => format!(
            "**{heading}:**\n\n```json\n{}\n```\n",
            serde_json::to_string_pretty(content).unwrap_or_default()
        ),
        _ => String::new(),
    }
}

fn format_meeting(content: &Map<String, Value>, locale: &str) -> String {
    let unknown = text(locale, "journal.window.content.raw.screen.unknown");
    let unknown_title = text(locale, "journal.window.content.raw.screen.unknown.title");
    let platform = content
        .get("platform")
        .and_then(Value::as_str)
        .unwrap_or(unknown.as_str());
    let mut lines = vec![
        strings(
            locale,
            "journal.window.content.raw.screen.meeting",
            [("platform", platform)],
        ),
        String::new(),
    ];
    if let Some(participants) = content.get("participants").and_then(Value::as_array)
        && !participants.is_empty()
    {
        lines.push(text(
            locale,
            "journal.window.content.raw.screen.participants",
        ));
        for participant in participants {
            let Some(participant) = participant.as_object() else {
                continue;
            };
            let name = participant
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(unknown_title.as_str());
            let status = participant
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or(unknown.as_str());
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
        let share_heading = match presenter {
            Some(presenter) => strings(
                locale,
                "journal.window.content.raw.screen.share.by",
                [("presenter", presenter)],
            ),
            None => text(locale, "journal.window.content.raw.screen.share"),
        };
        lines.push(share_heading);
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
    fn a_phone_recording_says_what_recorded_it_and_whether_anything_was_heard() {
        let frame = record(json!(1.0), "a call");
        let header = |meta: Value| {
            let records = [meta.as_object().unwrap().clone(), frame.clone()];
            (
                render(
                    "20261026/device_mobile-segment/170248_300/screen.jsonl",
                    &records,
                )
                .header
                .unwrap(),
                render_for_screen_talent(
                    "20261026/device_mobile-segment/170248_300/screen.jsonl",
                    &records,
                )
                .header
                .unwrap(),
            )
        };
        let (shared, talent) =
            header(json!({"raw":"screen.mp4","platform":"ios","sources":["screencast"]}));
        assert_eq!(shared, talent);
        assert!(
            talent.starts_with("# Frame Analyses\n\nThese frames come from a screen recording")
        );
        assert!(talent.contains("iPhone or iPad"));
        assert!(talent.contains("not the owner sharing their screen to a call"));
        assert!(talent.ends_with("nothing in it was heard."));

        let (_, with_sound) = header(
            json!({"raw":"screen.mp4","platform":"android","sources":["screencast","audio"]}),
        );
        assert!(with_sound.contains("Android phone or tablet"));
        assert!(!with_sound.contains("heard"));

        let (_, desktop) = header(json!({"raw":"screen.mp4","platform":"macos"}));
        assert_eq!(desktop, "# Frame Analyses");
    }

    #[test]
    fn qualified_detections_render_as_an_objects_line_after_the_description() {
        let frame = json!({
            "timestamp": 0.0,
            "analysis": {"primary": "media", "visual_description": "A town hall stream."},
            "detections": {
                "source": "screen",
                "objects": [
                    {"class_name": "person", "score": 0.8},
                    {"class_name": "person", "score": 0.7},
                    {"class_name": "laptop", "score": 0.9},
                    {"class_name": "person", "score": 0.2}
                ]
            }
        })
        .as_object()
        .expect("frame")
        .clone();
        let produced = render("20260304/workstation/090000_300/screen.jsonl", &[frame]);
        assert_eq!(
            produced.chunks[0].content,
            "### 09:00:00\n\n**Category:** media\n\nA town hall stream.\n\n**Objects:** person ×2\n"
        );

        let mut unqualified = produced.chunks[0].source.clone().expect("source");
        unqualified["detections"]["objects"] = json!([{"class_name": "tv", "score": 0.9}]);
        let produced = render(
            "20260304/workstation/090000_300/screen.jsonl",
            &[unqualified],
        );
        assert!(!produced.chunks[0].content.contains("**Objects:**"));
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
        assert_eq!(fixture.len(), 724);
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
    fn capture_metadata_preserves_native_projection_and_unknown_fields_fall_back() {
        let legacy: Value = serde_json::from_str(include_str!(
            "../../tests/data/golden/tmux-observer-envelope-main.jsonl"
        ))
        .unwrap();
        let mut extended = legacy.clone();
        for pane in extended["content"]["tmux"]["panes"].as_array_mut().unwrap() {
            for (key, value) in [
                ("cursor_x", json!(12)),
                ("cursor_y", json!(3)),
                ("cursor_visible", json!(true)),
                ("alternate_on", json!(false)),
                ("pane_in_mode", json!(false)),
            ] {
                pane[key] = value;
            }
        }
        let render = |frames: &[Value]| {
            render_for_screen_talent(
                "20260903/device/102159_300/tmux_0_screen.jsonl",
                &frames
                    .iter()
                    .map(|v| v.as_object().unwrap().clone())
                    .collect::<Vec<_>>(),
            )
        };
        let generic_old = super::render(
            "20260903/device/102159_300/tmux_0_screen.jsonl",
            &[legacy.as_object().unwrap().clone()],
        );
        let generic_new = super::render(
            "20260903/device/102159_300/tmux_0_screen.jsonl",
            &[extended.as_object().unwrap().clone()],
        );
        assert_eq!(generic_old.chunks[0].content, generic_new.chunks[0].content);
        let old = render(&[legacy.clone(), legacy.clone()]);
        let new = render(&[extended.clone(), extended.clone()]);
        let mixed = render(&[legacy, extended.clone()]);
        for view in [&new, &mixed] {
            assert_eq!(view.tmux_chunk_indices, old.tmux_chunk_indices);
            assert_eq!(view.header, old.header);
            assert_eq!(
                view.chunks.iter().map(|c| &c.content).collect::<Vec<_>>(),
                old.chunks.iter().map(|c| &c.content).collect::<Vec<_>>()
            );
        }
        extended["content"]["tmux"]["panes"][0]["unrecognized"] = json!(true);
        let unknown = render(&[extended]);
        assert!(unknown.tmux_chunk_indices.is_empty());
        assert!(unknown.chunks[0].content.contains("unrecognized"));
    }

    #[test]
    fn near_tmux_records_stay_on_lossless_generic_projection() {
        let valid = tmux_record(0.0, "@7", json!([pane("%1", "owner text\n")]));
        let mut cases = Vec::new();

        let mut wrong_primary = valid.clone();
        wrong_primary["analysis"]["primary"] = json!("terminal");
        wrong_primary["analysis"]["visual_description"] =
            json!("## Tmux change encoding\n### 10:22:00\n**Tmux window:**\n```json");
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

    #[test]
    fn concealed_sgr_text_and_credentials_are_redacted_in_structured_and_fallback_paths() {
        let jwt =
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjMifQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        let pane_body = format!(
            concat!(
                "visible \x1b[8mhidden_zero\x1b[0m after0\n",
                "keep \x1b[8mhidden_empty\x1b[m after_empty\n",
                "keep \x1b[1;8mhidden_28\x1b[28m after28\n",
                "\x1b[38;5;8mgrey_visible\x1b[0m \x1b[38:2::10:20:8mtruecolor_visible\x1b[0m\n",
                "$ vault unlock --password hunter2 --token mytok123 --api-key sk-proj-abc123DEF456ghi789\n",
                "$ curl -H 'Authorization: Bearer {jwt}' https://api.example.test\n",
                "keys ghp_q8XzT3vLm9Kp2RwN7sYbF4cHdJ6aUeG1oVi5 AKIAIOSFODNN7EXAMPLE\n",
                "\x1b]8;;https://example.test/auth?access_token=linksecret&view=1\x1b\\open --token labelsecret\x1b]8;;\x1b\\\n",
                "commit dbc2c5b8cf11395ac2842f300ea6f52f05c8216d ok\n"
            ),
            jwt = jwt
        );
        let canonical = tmux_record(0.0, "@7", json!([pane("%1", &pane_body)]));
        let structured = render_for_screen_talent(
            "20260903/device/102159_300/tmux_0_screen.jsonl",
            std::slice::from_ref(&canonical),
        );
        let structured_text = &structured.chunks[0].content;
        for leaked in [
            "hidden_zero",
            "hidden_empty",
            "hidden_28",
            "hunter2",
            "mytok123",
            "sk-proj-abc123DEF456ghi789",
            jwt,
            "ghp_q8XzT3vLm9Kp2RwN7sYbF4cHdJ6aUeG1oVi5",
            "AKIAIOSFODNN7EXAMPLE",
            "linksecret",
            "labelsecret",
        ] {
            assert!(
                !structured_text.contains(leaked),
                "structured path leaked {leaked}: {structured_text}"
            );
        }
        assert!(structured_text.contains("visible  after0"));
        assert!(structured_text.contains("grey_visible"));
        assert!(structured_text.contains("truecolor_visible"));
        assert!(structured_text.contains("dbc2c5b8cf11395ac2842f300ea6f52f05c8216d"));
        let proj = projected_json(structured_text);
        assert_eq!(
            proj["panes"][0]["links"][0]["target"],
            "https://example.test/auth?access_token=[REDACTED]&view=1"
        );
        assert_eq!(
            proj["panes"][0]["links"][0]["label"],
            "open --token [REDACTED]"
        );

        // Generic fallback in render_for_screen_talent (extra unrecognized pane field)
        // and render_for_locale both sanitize & redact tmux pane content too.
        let mut fallback_record = canonical;
        fallback_record["content"]["tmux"]["panes"][0]["unrecognized"] = json!(true);
        let fallback = render_for_screen_talent(
            "20260903/device/102159_300/tmux_0_screen.jsonl",
            std::slice::from_ref(&fallback_record),
        );
        assert!(fallback.tmux_chunk_indices.is_empty());
        let generic = render(
            "20260903/device/102159_300/tmux_0_screen.jsonl",
            &[fallback_record],
        );
        for rendered in [&fallback.chunks[0].content, &generic.chunks[0].content] {
            for leaked in [
                "hidden_zero",
                "hidden_empty",
                "hidden_28",
                "hunter2",
                "mytok123",
                "sk-proj-abc123DEF456ghi789",
                jwt,
                "ghp_q8XzT3vLm9Kp2RwN7sYbF4cHdJ6aUeG1oVi5",
                "AKIAIOSFODNN7EXAMPLE",
                "linksecret",
                "labelsecret",
            ] {
                assert!(
                    !rendered.contains(leaked),
                    "fallback path leaked {leaked}: {rendered}"
                );
            }
            assert!(rendered.contains("grey_visible"));
            assert!(rendered.contains("dbc2c5b8cf11395ac2842f300ea6f52f05c8216d"));
        }
    }
}
