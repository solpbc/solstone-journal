// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Terminal-only segments, sampled to fit the floor window for the talents that
//! read them, Sense and Screen (docs/THINK.md § Terminal segments).
//!
//! Each tmux session's captures become entries: each pane's screen when first
//! seen, the lines that later appeared on it, each line once, and the input the
//! owner typed. The entries are then fitted to the budget the floor window
//! leaves after everything else in the request, so the source is bounded by
//! design and reads the same on every backend. The result takes the place of
//! the screen captures in the talent's ordinary request, which runs, validates and
//! publishes like any other.
mod cursor;
mod diff_matcher;
#[cfg(all(test, feature = "full-tests"))]
mod preparation_tests;
mod sample;
mod screen;

use crate::{ExecutionContext, PreparedTalent};
use serde_json::{Value, json};

/// Typed input kept per segment, newest first.
const TYPED_BUDGET_BYTES: usize = 6000;
/// Room for the request's message framing and tokenizer boundary effects
/// between separately counted parts.
const FRAMING_TOKENS: u32 = 256;
/// Distinct terminal contexts listed, the most observed first.
const CONTEXT_ROWS: usize = 30;
/// Desktop-activity fields that locate a record rather than describe it.
const CONTEXT_PLUMBING: [&str; 9] = [
    "v", "t", "method", "ts", "end_ts", "start_ms", "end_ms", "stream", "path",
];

/// The capture's desktop activity, each distinct context once: what the
/// terminal reported about the focused pane (its title, working directory,
/// program, window), in order of first appearance, with how often it was seen.
fn desktop_context(records: &[serde_json::Map<String, Value>]) -> String {
    let mut rows: Vec<(String, usize)> = Vec::new();
    for record in records {
        let mut fields: Vec<(&String, &Value)> = record
            .iter()
            .filter(|(key, value)| !CONTEXT_PLUMBING.contains(&key.as_str()) && !value.is_null())
            .collect();
        fields.sort_by_key(|(key, _)| (key.as_str() != "kind", key.as_str()));
        let row = fields
            .iter()
            .map(|(key, value)| match value {
                Value::String(text) => format!("{key} \"{text}\""),
                other => format!("{key} {other}"),
            })
            .collect::<Vec<_>>()
            .join(" · ");
        match rows.iter_mut().find(|(known, _)| *known == row) {
            Some((_, seen)) => *seen += 1,
            None => rows.push((row, 1)),
        }
    }
    let mut kept: Vec<usize> = (0..rows.len()).collect();
    kept.sort_by_key(|&i| (std::cmp::Reverse(rows[i].1), i));
    kept.truncate(CONTEXT_ROWS);
    kept.sort_unstable();
    let mut text = format!(
        "Terminal focus reported with the capture, each distinct context once, in order of first appearance ({} observations):\n",
        records.len()
    );
    for i in &kept {
        let (row, seen) = &rows[*i];
        text.push_str(&format!("- {row} ({seen}×)\n"));
    }
    if rows.len() > kept.len() {
        text.push_str(&format!(
            "[{} less frequent contexts left out]\n",
            rows.len() - kept.len()
        ));
    }
    text
}

#[cfg(all(test, feature = "full-tests"))]
thread_local! {
    /// Lets a test see the request as preparation built it before sampling.
    static SKIP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `HH:MM:SS` for a capture `offset` seconds after the segment start.
pub(super) fn clock(base_ms: i64, offset: f64) -> String {
    let value = (base_ms + (offset * 1000.0).round() as i64).rem_euclid(86_400_000);
    format!(
        "{:02}:{:02}:{:02}",
        value / 3_600_000,
        (value / 60_000) % 60,
        (value / 1_000) % 60
    )
}

/// Replace a terminal-only segment's screen captures with their sampled
/// reading, and return what was read and left out. Any other segment, or one
/// whose sources cannot be bound to the bytes that were rendered, keeps the
/// ordinary request.
pub(crate) fn prepare(prepared: &mut PreparedTalent, context: &ExecutionContext) -> Option<Value> {
    #[cfg(all(test, feature = "full-tests"))]
    if SKIP.with(std::cell::Cell::get) {
        return None;
    }
    if !matches!(prepared.name.as_str(), "sense" | "screen")
        || prepared.config.contains_key("messages")
        || prepared.config.contains_key("hook")
        || prepared.config.contains_key("span")
        || prepared.config.contains_key("activity")
    {
        return None;
    }
    let counts = prepared.config.get("source_counts")?;
    if counts["transcripts"] != 0
        || counts["talents"] != 0
        || counts["percepts"].as_u64()? == 0
        || prepared.config.get("_memory_sources")? != &json!([])
    {
        return None;
    }
    let sources = prepared
        .entry_sources
        .iter()
        .map(Option::as_ref)
        .collect::<Option<Vec<_>>>()?;
    if sources.len() != counts["percepts"].as_u64()? as usize {
        return None;
    }
    let transcript = prepared.config.get("transcript")?.as_str()?.to_owned();
    let base_ms = base_ms(prepared)?;
    let day = prepared.config.get("day")?.as_str()?;
    let segment = prepared.config.get("segment")?.as_str()?;
    let stream = prepared.config.get("stream").and_then(Value::as_str);
    let mut screens: Vec<(String, String, Vec<Value>)> = Vec::new();
    let mut contexts: Vec<(String, String)> = Vec::new();
    for source in sources {
        let mut expected = context.journal.join("chronicle").join(day);
        if let Some(stream) = &source.stream {
            expected = expected.join(stream);
        }
        expected = expected.join(segment).join(&source.file);
        if source.path != expected
            || source.path.file_name()?.to_str()? != source.file
            || source.unparsed_lines != 0
            || stream.is_some_and(|stream| source.stream.as_deref() != Some(stream))
        {
            return None;
        }
        if source.file.ends_with("_desktop_activity.jsonl")
            && !source.records.is_empty()
            && source.records.iter().all(|record| {
                record.get("v").and_then(Value::as_u64) == Some(1)
                    && record.get("t").and_then(Value::as_str) == Some("desktop_activity")
                    && record.get("method").and_then(Value::as_str) == Some("tmux")
                    && matches!(
                        record.get("kind").and_then(Value::as_str),
                        Some("terminal" | "availability")
                    )
            })
        {
            let rendered = source
                .chunks
                .iter()
                .map(|(chunk, _)| chunk.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            let rendered = rendered.trim();
            if rendered.is_empty() || transcript.matches(rendered).count() != 1 {
                return None;
            }
            contexts.push((rendered.to_owned(), desktop_context(&source.records)));
            continue;
        }
        let mut frames: Vec<Value> = source
            .records
            .iter()
            .filter(|r| r.contains_key("timestamp"))
            .cloned()
            .map(Value::Object)
            .collect();
        if frames.is_empty()
            || frames.len() != source.chunks.len()
            || frames.iter().any(|f| !valid_frame(f))
            || frames
                .iter()
                .zip(&source.chunks)
                .any(|(f, (_, r))| f.as_object() != Some(r))
        {
            return None;
        }
        let rendered = source
            .chunks
            .iter()
            .map(|(s, _)| s.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if rendered.is_empty() || transcript.matches(&rendered).count() != 1 {
            return None;
        }
        frames.sort_by(|a, b| {
            a["timestamp"]
                .as_f64()
                .unwrap_or(0.0)
                .total_cmp(&b["timestamp"].as_f64().unwrap_or(0.0))
        });
        let name = frames[0]["content"]["tmux"]["session"]
            .as_str()
            .unwrap_or("")
            .to_owned();
        screens.push((rendered, name, frames));
    }
    if screens.is_empty() {
        return None;
    }
    // Everything but the screen captures is read whole; the captures get the rest.
    let max_output_tokens = prepared
        .config
        .get("max_output_tokens")
        .and_then(Value::as_u64)
        .and_then(|tokens| u32::try_from(tokens).ok())?;
    let floor = solstone_core_local::generate::compute_input_budget(
        max_output_tokens,
        solstone_core_local::plan::LOCAL_MIN_CONTEXT_TOKENS,
    );
    let mut transcript = transcript;
    for (rendered, context) in &contexts {
        transcript = transcript.replacen(rendered, context.trim_end(), 1);
    }
    let mut rest = transcript.clone();
    for (rendered, _, _) in &screens {
        rest = rest.replacen(rendered, "", 1);
    }
    let instruction = prepared
        .config
        .get("user_instruction")
        .and_then(Value::as_str)
        .unwrap_or("");
    let fixed = solstone_core_tokens::count(&rest)
        + solstone_core_tokens::count(instruction)
        + FRAMING_TOKENS;
    let budget = floor.checked_sub(fixed)?;
    let typed_budget = TYPED_BUDGET_BYTES / screens.len();
    let sessions: Vec<screen::Session> = screens
        .iter()
        .map(|(_, name, frames)| screen::read(name, frames, typed_budget))
        .collect();
    let fit = sample::fit(&sessions, base_ms, budget, &|text| {
        solstone_core_tokens::count(text)
    });
    let mut text = transcript;
    for ((rendered, _, _), reading) in screens.iter().zip(&fit.texts) {
        text = text.replacen(rendered, reading.trim_end(), 1);
    }
    prepared
        .config
        .insert("transcript".to_owned(), Value::String(text));
    // Spelling views bind speech; a terminal-only segment has none.
    prepared.config.remove(crate::product_names::VIEW_KEY);
    // The reading fits one request, so Screen no longer splits the segment.
    prepared.config.remove("_screen_batch_cuts");
    Some(json!({
        "event": "terminal_reading",
        "talent": prepared.name,
        "branch": "sampled",
        "sessions": sessions.len(),
        "contexts": contexts.len(),
        "captures": sessions.iter().map(|s| s.captures).sum::<usize>(),
        "typed": sessions.iter().flat_map(|s| &s.items).filter(|i| i.kind == screen::Kind::Typed).count(),
        "typed_left_out": sessions.iter().map(|s| s.typed_left_out).sum::<usize>(),
        "entries": fit.entries,
        "entries_left_out": fit.entries_left_out,
        "lines": fit.lines,
        "lines_shown": fit.lines_shown,
        "line_cap": fit.line_cap,
        "tokens": fit.tokens,
        "budget": budget,
    }))
}

fn valid_frame(frame: &Value) -> bool {
    let tmux = &frame["content"]["tmux"];
    frame["timestamp"].as_f64().is_some_and(f64::is_finite)
        && tmux.is_object()
        && tmux["session"].as_str().is_some_and(|s| !s.is_empty())
        && tmux["window"]["id"].as_str().is_some_and(|s| !s.is_empty())
        && tmux["panes"].as_array().is_some_and(|ps| {
            !ps.is_empty()
                && ps.iter().all(|p| {
                    p["id"].as_str().is_some_and(|s| !s.is_empty()) && p["content"].is_string()
                })
        })
}

fn base_ms(prepared: &PreparedTalent) -> Option<i64> {
    let segment = prepared.config.get("segment")?.as_str()?;
    let clock = segment.split('_').next()?;
    if clock.len() != 6 || !clock.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let h: i64 = clock[..2].parse().ok()?;
    let m: i64 = clock[2..4].parse().ok()?;
    let s: i64 = clock[4..].parse().ok()?;
    if h >= 24 || m >= 60 || s >= 60 {
        return None;
    }
    Some((h * 3600 + m * 60 + s) * 1000)
}
