// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::progressive_frames::{expand, groups};
use super::readable_changes as display;
use super::state_codec::{decode as restore_states, encode as encode_states};
use serde_json::{Map, Value, json};

pub(crate) const INTRO: &str = super::instructions::RAW_CHANGES_INTRO;
const ROSTER_EMITTED: &str = "Global window rosters are emitted initially and on change.\n";
const ROSTER_OMITTED: &str = "The global inactive-window roster is omitted.\n";

pub(crate) fn prepare(
    frames: &[Value],
    keep_roster: bool,
) -> Result<(Vec<Value>, Vec<Value>), &'static str> {
    let mut visible = Vec::new();
    for f in frames {
        let mut analysis = Map::new();
        if let Some(a) = f.get("analysis").and_then(Value::as_object) {
            for (k, v) in a {
                if matches!(k.as_str(), "primary" | "visual_description") {
                    analysis.insert(k.clone(), v.clone());
                }
            }
        }
        let mut tmux = f["content"]["tmux"].clone();
        // Capture-only fields must not change prefix grouping or model input.
        // Keep the source records and the lower-level lossless codec untouched.
        solstone_core_format::content::omit_tmux_capture_fields(&mut tmux);
        if !keep_roster && let Some(obj) = tmux.as_object_mut() {
            obj.shift_remove("windows");
        }
        visible.push(json!({
            "timestamp": f["timestamp"],
            "analysis": analysis,
            "content": {"tmux": tmux},
        }));
    }

    let grouped = groups(&visible);
    let latest_frames: Vec<Value> = grouped.iter().map(|g| g.latest.clone()).collect();
    let mut records = encode_states(&latest_frames, 60.0, true)?;

    for (record, group) in records.iter_mut().zip(grouped.iter()) {
        record
            .as_object_mut()
            .unwrap()
            .insert("prefix_history".into(), Value::Array(group.earlier.clone()));
    }

    Ok((visible, records))
}

pub(crate) fn restore(records: &[Value]) -> Result<Vec<Value>, &'static str> {
    let mut work = records.to_vec();
    let mut histories = Vec::new();
    for r in &mut work {
        let h = r
            .as_object_mut()
            .unwrap()
            .shift_remove("prefix_history")
            .ok_or("missing prefix_history")?;
        histories.push(h.as_array().cloned().unwrap_or_default());
    }

    let frames = restore_states(&work)?;
    let mut grouped = Vec::new();
    for (f, h) in frames.into_iter().zip(histories) {
        grouped.push(super::progressive_frames::Group {
            latest: f,
            earlier: h,
        });
    }
    Ok(expand(&grouped))
}

pub(crate) fn encode(records: &[Value], base_ms: i64) -> String {
    let text = display::encode(records, base_ms);
    assert!(text.starts_with(display::INTRO));
    let roster = records
        .iter()
        .any(|r| r["roster_update"]["present"].as_bool() == Some(true));
    let availability = if roster {
        ROSTER_EMITTED
    } else {
        ROSTER_OMITTED
    };
    format!("{INTRO}{availability}{}", &text[display::INTRO.len()..])
}

pub(crate) fn decode(text: &str, base_ms: i64) -> Result<Vec<Value>, &'static str> {
    if !text.starts_with(INTRO) {
        return Err("missing native-byte timeline guide");
    }
    let (_, timeline) = split_guide(text).ok_or("missing roster availability")?;
    let display_text = format!("{}{timeline}", display::INTRO);
    display::decode(&display_text, base_ms)
}

/// Split an encoded timeline into its reading guide and its observations. The
/// guide travels with the preserved instructions, so a fitted timeline stays readable.
pub(crate) fn split_guide(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix(INTRO)?;
    let availability = [ROSTER_EMITTED, ROSTER_OMITTED]
        .into_iter()
        .find(|line| rest.starts_with(line))?;
    Some(text.split_at(INTRO.len() + availability.len()))
}
