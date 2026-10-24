// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::LazyLock;

pub(crate) static CSI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").unwrap());
pub(crate) static MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*[›❯][\s\u{00a0}]*\S").unwrap());

pub(crate) fn select(frames: &[Value], max_bytes: usize, max_scopes: usize) -> Vec<Value> {
    let mut sorted_frames = frames.to_vec();
    sorted_frames.sort_by(|a, b| {
        let ta = a["timestamp"].as_f64().unwrap_or(0.0);
        let tb = b["timestamp"].as_f64().unwrap_or(0.0);
        ta.partial_cmp(&tb).unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut latest: BTreeMap<(String, String, String), Value> = BTreeMap::new();

    for frame in &sorted_frames {
        let ts = frame["timestamp"].clone();
        let tmux = &frame["content"]["tmux"];
        let session = tmux["session"].as_str().unwrap_or("").to_owned();
        let window_id = tmux["window"]["id"].as_str().unwrap_or("").to_owned();
        if let Some(panes) = tmux["panes"].as_array() {
            for pane in panes {
                let pane_id = pane["id"].as_str().unwrap_or("").to_owned();
                let text = pane["content"].as_str().unwrap_or("");
                let plain = CSI.replace_all(text, "");
                let lines = super::state_codec::split_lines_keepends(&plain);
                let marked: Vec<&str> = lines
                    .iter()
                    .map(String::as_str)
                    .filter(|s| MARKER.is_match(s) && !s.contains("Ask Codex to do anything"))
                    .collect();
                if marked.is_empty() {
                    continue;
                }
                let last_marked = marked.last().unwrap().trim_end();
                if last_marked.ends_with('…') || last_marked.ends_with("...") {
                    continue;
                }
                let scope_tuple = (session.clone(), window_id.clone(), pane_id.clone());
                let sha256 = format!("{:x}", Sha256::digest(text.as_bytes()));
                latest.insert(
                    scope_tuple,
                    json!({
                        "timestamp": ts,
                        "scope": [session, window_id, pane_id],
                        "text": text,
                        "sha256": sha256,
                    }),
                );
            }
        }
    }

    let mut rows: Vec<Value> = latest.into_values().collect();
    rows.sort_by(|a, b| {
        let ta = a["timestamp"].as_f64().unwrap_or(0.0);
        let tb = b["timestamp"].as_f64().unwrap_or(0.0);
        ta.partial_cmp(&tb).unwrap_or_else(|| {
            let sa = a["scope"].as_array().unwrap();
            let sb = b["scope"].as_array().unwrap();
            let ssa: (Option<&str>, Option<&str>, Option<&str>) =
                (sa[0].as_str(), sa[1].as_str(), sa[2].as_str());
            let ssb: (Option<&str>, Option<&str>, Option<&str>) =
                (sb[0].as_str(), sb[1].as_str(), sb[2].as_str());
            ssa.cmp(&ssb)
        })
    });

    let total_bytes: usize = rows
        .iter()
        .map(|r| r["text"].as_str().unwrap_or("").len())
        .sum();
    if rows.len() > max_scopes || total_bytes > max_bytes {
        return Vec::new();
    }
    rows
}
