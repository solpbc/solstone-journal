// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::diff_matcher::{OpcodeTag, SequenceMatcher};
use super::python_json::{DumpOptions, python_dumps};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};

/// Python splitlines(keepends=True), including CRLF and Unicode separators.
pub(crate) fn split_lines_keepends(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if matches!(
            c,
            '\n' | '\r'
                | '\u{b}'
                | '\u{c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        ) {
            let mut end = i + c.len_utf8();
            if c == '\r' && chars.peek().is_some_and(|(_, c)| *c == '\n') {
                let (j, c) = chars.next().unwrap();
                end = j + c.len_utf8();
            }
            lines.push(text[start..end].to_owned());
            start = end;
        }
    }
    if start < text.len() {
        lines.push(text[start..].to_owned());
    }
    lines
}

pub(crate) fn keys(frame: &Value) -> Result<Vec<(String, String, String)>, &'static str> {
    let tmux = frame
        .get("content")
        .and_then(|c| c.get("tmux"))
        .and_then(Value::as_object)
        .ok_or("missing content.tmux")?;

    if tmux.get("session").is_none() {
        return Err("missing session");
    }
    let window = tmux
        .get("window")
        .and_then(Value::as_object)
        .ok_or("missing window object")?;
    if window.get("id").is_none() {
        return Err("missing window id");
    }
    let panes = tmux
        .get("panes")
        .and_then(Value::as_array)
        .ok_or("missing panes array")?;
    for p in panes {
        if p.get("id").is_none() {
            return Err("missing pane id");
        }
    }

    let session = python_dumps(
        tmux.get("session").unwrap(),
        DumpOptions {
            ensure_ascii: false,
            compact: false,
            sort_keys: true,
        },
    );
    let window_id = python_dumps(
        window.get("id").unwrap(),
        DumpOptions {
            ensure_ascii: false,
            compact: false,
            sort_keys: false,
        },
    );

    let mut result = Vec::new();
    for p in panes {
        let pane_id = python_dumps(p.get("id").unwrap(), DumpOptions::default());
        result.push((session.clone(), window_id.clone(), pane_id));
    }

    let mut seen = HashSet::new();
    for k in &result {
        if !seen.insert(k.clone()) {
            return Err("ambiguous pane identity");
        }
    }
    Ok(result)
}

/// Marks a record where every pane and roster restarts from complete text.
pub(crate) const RESTART_KEY: &str = "restart";

pub(crate) fn encode(
    frames: &[Value],
    anchor_seconds: f64,
    include_removed: bool,
) -> Result<Vec<Value>, &'static str> {
    if anchor_seconds <= 0.0 {
        return Err("positive anchor interval required");
    }

    let mut state: HashMap<(String, String, String), Vec<String>> = HashMap::new();
    let mut last_anchor: HashMap<(String, String, String), f64> = HashMap::new();
    let mut visible: HashSet<(String, String, String)> = HashSet::new();
    let mut result = Vec::new();
    let mut previous_time: Option<f64> = None;
    let mut rosters: HashMap<String, Value> = HashMap::new();

    for frame in frames {
        let time = frame
            .get("timestamp")
            .and_then(Value::as_f64)
            .ok_or("missing timestamp")?;
        if let Some(prev) = previous_time
            && time < prev
        {
            return Err("source frame order is not monotonic");
        }
        previous_time = Some(time);

        let mut metadata = frame.clone();
        let frame_keys = keys(frame)?;
        let mut updates = Vec::new();
        // Anchors recur together: every pane and roster restarts from complete
        // text, so a reader that starts at a restart needs nothing earlier.
        let restart = result.is_empty()
            || frame_keys.iter().any(|key| {
                visible.contains(key)
                    && last_anchor
                        .get(key)
                        .is_some_and(|at| time - at >= anchor_seconds)
            });
        if restart {
            visible.clear();
            rosters.clear();
        }

        let tmux = metadata
            .get_mut("content")
            .and_then(|c| c.get_mut("tmux"))
            .and_then(Value::as_object_mut)
            .ok_or("invalid tmux metadata")?;

        let session_key = python_dumps(
            tmux.get("session").unwrap(),
            DumpOptions {
                ensure_ascii: false,
                compact: false,
                sort_keys: true,
            },
        );

        let has_windows = tmux.contains_key("windows");
        let windows_val = tmux.shift_remove("windows");
        let roster = json!({
            "present": has_windows,
            "value": windows_val,
        });

        let mut roster_update = if rosters.get(&session_key) == Some(&roster) {
            json!({"mode": "unchanged"})
        } else {
            let mut u = Map::new();
            u.insert("mode".into(), json!("full"));
            u.insert("present".into(), json!(has_windows));
            u.insert("value".into(), roster["value"].clone());
            Value::Object(u)
        };

        if let Some(prev_roster) = rosters.get(&session_key)
            && roster_update["mode"] == "full"
            && prev_roster["present"] == true
            && roster["present"] == true
            && roster["value"].is_array()
            && prev_roster["value"].is_array()
        {
            let cur_arr = roster["value"].as_array().unwrap();
            let prev_arr = prev_roster["value"].as_array().unwrap();
            if cur_arr.len() == prev_arr.len()
                && cur_arr
                    .iter()
                    .zip(prev_arr.iter())
                    .all(|(a, b)| a.get("id").is_some() && a.get("id") == b.get("id"))
            {
                let mut replacements = Vec::new();
                for (i, val) in cur_arr.iter().enumerate() {
                    if val != &prev_arr[i] {
                        replacements.push(json!({
                            "index": i,
                            "window": val,
                        }));
                    }
                }
                let candidate = json!({
                    "mode": "replace_windows",
                    "replacements": replacements,
                });
                let dump_opts = DumpOptions {
                    ensure_ascii: false,
                    compact: false,
                    sort_keys: false,
                };
                if python_dumps(&candidate, dump_opts).chars().count()
                    < python_dumps(&roster_update, dump_opts).chars().count()
                {
                    roster_update = candidate;
                }
            }
        }
        rosters.insert(session_key, roster);

        let panes = tmux
            .get_mut("panes")
            .and_then(Value::as_array_mut)
            .ok_or("invalid panes")?;

        for (key, pane_val) in frame_keys.iter().zip(panes.iter_mut()) {
            let pane_obj = pane_val.as_object_mut().ok_or("invalid pane")?;
            let text_val = pane_obj
                .shift_remove("content")
                .ok_or("nontext pane requires full native fallback")?;
            let text = text_val
                .as_str()
                .ok_or("nontext pane requires full native fallback")?;
            let lines = split_lines_keepends(text);
            let prior = state.get(key);

            let mut update;
            let last_t = last_anchor.get(key).copied().unwrap_or(time);
            if !visible.contains(key) || (time - last_t) >= anchor_seconds {
                update = json!({"mode": "full", "text": text});
                last_anchor.insert(key.clone(), time);
            } else if prior == Some(&lines) {
                update = json!({"mode": "unchanged"});
            } else if prior.map_or(0, |p| p.len()).max(lines.len()) > 2000 {
                update = json!({"mode": "full", "text": text});
                last_anchor.insert(key.clone(), time);
            } else {
                let p = prior.unwrap();
                let mut matcher = SequenceMatcher::new(p, &lines);
                let opcodes = matcher.get_opcodes();
                let mut edits = Vec::new();
                for op in opcodes {
                    if op.tag != OpcodeTag::Equal {
                        let mut edit = Map::new();
                        edit.insert("start".into(), json!(op.i1));
                        edit.insert("stop".into(), json!(op.i2));
                        if include_removed {
                            edit.insert("removed".into(), json!(p[op.i1..op.i2]));
                        }
                        edit.insert("inserted".into(), json!(lines[op.j1..op.j2]));
                        edits.push(Value::Object(edit));
                    }
                }
                update = json!({"mode": "edits", "edits": edits});
                let dump_opts = DumpOptions {
                    ensure_ascii: false,
                    compact: false,
                    sort_keys: false,
                };
                let full_cand = json!({"mode": "full", "text": text});
                if python_dumps(&update, dump_opts).chars().count()
                    >= python_dumps(&full_cand, dump_opts).chars().count()
                {
                    update = full_cand;
                    last_anchor.insert(key.clone(), time);
                }
            }
            state.insert(key.clone(), lines);
            updates.push(update);
        }

        let mut record = json!({
            "frame_metadata": metadata,
            "roster_update": roster_update,
            "pane_updates": updates,
        });
        if restart {
            record[RESTART_KEY] = Value::Bool(true);
        }
        result.push(record);
        visible = frame_keys.into_iter().collect();
    }

    Ok(result)
}

pub(crate) fn decode(records: &[Value]) -> Result<Vec<Value>, &'static str> {
    let mut state: HashMap<(String, String, String), Vec<String>> = HashMap::new();
    let mut result = Vec::new();
    let mut rosters: HashMap<String, Value> = HashMap::new();

    for record in records {
        let mut frame = record
            .get("frame_metadata")
            .cloned()
            .ok_or("missing frame_metadata")?;
        let tmux = frame
            .get_mut("content")
            .and_then(|c| c.get_mut("tmux"))
            .and_then(Value::as_object_mut)
            .ok_or("missing content.tmux")?;

        let session_key = python_dumps(
            tmux.get("session").unwrap(),
            DumpOptions {
                ensure_ascii: false,
                compact: false,
                sort_keys: true,
            },
        );

        let roster_update = record.get("roster_update").ok_or("missing roster_update")?;
        let mode = roster_update
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("");

        if mode == "full" {
            rosters.insert(
                session_key.clone(),
                json!({
                    "present": roster_update["present"],
                    "value": roster_update["value"],
                }),
            );
        } else if mode == "replace_windows" && rosters.contains_key(&session_key) {
            let mut val = rosters[&session_key]["value"].clone();
            let arr = val.as_array_mut().ok_or("invalid roster value array")?;
            if let Some(replacements) = roster_update.get("replacements").and_then(Value::as_array)
            {
                for change in replacements {
                    let idx = change["index"].as_u64().ok_or("invalid index")? as usize;
                    if idx < arr.len() {
                        arr[idx] = change["window"].clone();
                    }
                }
            }
            rosters.insert(session_key.clone(), json!({"present": true, "value": val}));
        } else if mode != "unchanged" || !rosters.contains_key(&session_key) {
            return Err("unknown roster mode or missing prior roster");
        }

        if rosters[&session_key]["present"] == true {
            tmux.insert("windows".into(), rosters[&session_key]["value"].clone());
        }

        let frame_keys = keys(&frame)?;
        let tmux = frame["content"]["tmux"]
            .as_object_mut()
            .ok_or("missing tmux")?;
        let pane_updates = record
            .get("pane_updates")
            .and_then(Value::as_array)
            .ok_or("missing pane_updates")?;
        if frame_keys.len() != pane_updates.len() {
            return Err("pane update count differs from metadata");
        }

        let panes = tmux
            .get_mut("panes")
            .and_then(Value::as_array_mut)
            .ok_or("invalid panes")?;

        for ((key, pane_val), update) in frame_keys.iter().zip(panes.iter_mut()).zip(pane_updates) {
            let mode = update.get("mode").and_then(Value::as_str).unwrap_or("");
            let lines: Vec<String>;
            if mode == "full" {
                let text = update
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or("missing full text")?;
                lines = split_lines_keepends(text);
            } else if (mode == "unchanged" || mode == "edits") && state.contains_key(key) {
                let prior = state.get(key).unwrap();
                if mode == "unchanged" {
                    lines = prior.clone();
                } else {
                    let mut reconstructed = Vec::new();
                    let mut cursor = 0;
                    let edits = update
                        .get("edits")
                        .and_then(Value::as_array)
                        .ok_or("missing edits")?;
                    for edit in edits {
                        let start = edit["start"].as_u64().ok_or("invalid start")? as usize;
                        let stop = edit["stop"].as_u64().ok_or("invalid stop")? as usize;
                        if !(cursor <= start && start <= stop && stop <= prior.len()) {
                            return Err("edit does not bind to exact prior pane lines");
                        }
                        if let Some(removed) = edit.get("removed").and_then(Value::as_array) {
                            let removed_strs: Vec<&str> =
                                removed.iter().filter_map(Value::as_str).collect();
                            let prior_slice: Vec<&str> =
                                prior[start..stop].iter().map(|s| s.as_str()).collect();
                            if removed_strs != prior_slice {
                                return Err("edit does not bind to exact prior pane lines");
                            }
                        }
                        for line in &prior[cursor..start] {
                            reconstructed.push(line.clone());
                        }
                        if let Some(inserted) = edit.get("inserted").and_then(Value::as_array) {
                            for line in inserted {
                                if let Some(s) = line.as_str() {
                                    reconstructed.push(s.to_owned());
                                }
                            }
                        }
                        cursor = stop;
                    }
                    for line in &prior[cursor..] {
                        reconstructed.push(line.clone());
                    }
                    lines = reconstructed;
                }
            } else {
                return Err("unknown mode or missing prior pane state");
            }

            state.insert(key.clone(), lines.clone());
            pane_val
                .as_object_mut()
                .unwrap()
                .insert("content".into(), json!(lines.concat()));
        }

        result.push(frame);
    }

    Ok(result)
}
