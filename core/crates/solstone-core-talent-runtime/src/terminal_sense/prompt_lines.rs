// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::prompt_attention::{CSI, MARKER, select};
use super::state_codec::split_lines_keepends;
use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

static STOP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:[⎿●•◦─━┌└╭╰│]|GPT-\S|\? for shortcuts|⚠|⏵)").unwrap());

pub(crate) fn excerpts(frames: &[Value], max_bytes: usize) -> Vec<Value> {
    let mut result = Vec::new();
    for screen in select(frames, 24000, 4) {
        let raw = screen["text"].as_str().unwrap_or("");
        let lines = split_lines_keepends(raw);
        let plain: Vec<String> = lines
            .iter()
            .map(|s| {
                CSI.replace_all(s, "")
                    .trim_end_matches(['\r', '\n'])
                    .to_owned()
            })
            .collect();
        let marked: Vec<usize> = plain
            .iter()
            .enumerate()
            .filter(|(_, s)| MARKER.is_match(s) && !s.contains("Ask Codex to do anything"))
            .map(|(i, _)| i)
            .collect();
        if marked.is_empty() {
            continue;
        }
        let lo = *marked.last().unwrap();
        let mut hi = lo + 1;
        while hi < lines.len() {
            let s = &plain[hi];
            if s.trim().is_empty()
                || MARKER.is_match(s)
                || STOP.is_match(s)
                || (!s.starts_with("  ") && !s.starts_with('\t'))
            {
                break;
            }
            hi += 1;
        }

        let text = lines[lo..hi].concat();
        let start_byte: usize = lines[..lo].iter().map(|s| s.len()).sum();
        let end_byte = start_byte + text.len();
        let excerpt_sha = format!("{:x}", Sha256::digest(text.as_bytes()));

        let mut row = screen.as_object().cloned().unwrap_or_default();
        row.shift_remove("text");
        row.insert("start_line".into(), json!(lo));
        row.insert("end_line_exclusive".into(), json!(hi));
        row.insert("start_utf8_byte".into(), json!(start_byte));
        row.insert("end_utf8_byte".into(), json!(end_byte));
        row.insert("text".into(), json!(text));
        row.insert("excerpt_sha256".into(), json!(excerpt_sha));

        result.push(Value::Object(row));
    }

    let total: usize = result
        .iter()
        .map(|r| r["text"].as_str().unwrap_or("").len())
        .sum();
    if total <= max_bytes {
        result
    } else {
        Vec::new()
    }
}
