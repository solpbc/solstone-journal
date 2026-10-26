// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::{
    cursor::{self, Bound},
    digest,
    state_codec::split_lines_keepends,
};
use regex::Regex;
use serde_json::{Value, json};
use std::{cmp::Ordering, collections::HashSet, sync::LazyLock};

static BLANK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n[ \t]*\n").unwrap());
static RULE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[─━═_\-=]{20,}$").unwrap());

#[derive(Clone, Debug)]
pub(super) struct Passage {
    pub id: String,
    pub text: String,
    pub scope: Value,
    pub timestamp: Value,
    pub screen_sha: String,
    pub raw_sha: String,
    pub start: usize,
    pub end: usize,
}

pub(super) fn compare(a: &Passage, b: &Passage) -> Ordering {
    a.timestamp
        .as_f64()
        .unwrap()
        .total_cmp(&b.timestamp.as_f64().unwrap())
        .then_with(|| scope_key(&a.scope).cmp(&scope_key(&b.scope)))
        .then(a.start.cmp(&b.start))
}
fn scope_key(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect()
}

fn row(raw: &str, lo: usize, screen: &str, scope: &Value, time: &Value) -> Option<Passage> {
    if !(20..=800).contains(&raw.len()) || raw.contains("\x1b]") {
        return None;
    }
    let display = super::readable_terminal::render(raw).ok()?;
    if display.contains("<strike>") {
        return None;
    }
    let text = cursor::plain(raw).ok()?;
    if text.trim().is_empty() {
        return None;
    }
    Some(Passage {
        id: String::new(),
        text,
        scope: scope.clone(),
        timestamp: time.clone(),
        screen_sha: digest(screen),
        raw_sha: digest(raw),
        start: lo,
        end: lo + raw.len(),
    })
}

fn blocks(text: &str) -> Vec<(&str, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    let mut cursor = 0;
    for line in split_lines_keepends(text) {
        let plain = cursor::CSI.replace_all(&line, "");
        let plain = plain.trim();
        if plain.is_empty() || RULE.is_match(plain) {
            if let Some(lo) = start.take() {
                out.push((&text[lo..cursor], lo));
            }
        } else if start.is_none() {
            start = Some(cursor);
        }
        cursor += line.len();
    }
    if let Some(lo) = start {
        out.push((&text[lo..], lo));
    }
    out
}

pub(super) fn prepare(frames: &[Value], bound: &Bound) -> Option<Vec<Passage>> {
    let before = &bound.screen[..bound.start];
    let mut seed = Vec::new();
    let mut start = 0;
    for (end, following) in BLANK
        .find_iter(before)
        .map(|m| (m.start(), m.end()))
        .chain(std::iter::once((before.len(), before.len())))
    {
        if let Some(r) = row(
            &before[start..end],
            start,
            &bound.screen,
            &bound.scope,
            &bound.timestamp,
        ) {
            seed.push(r);
        }
        start = following;
    }
    if seed.len() > 32 {
        seed = seed[..16]
            .iter()
            .chain(seed[seed.len() - 16..].iter())
            .cloned()
            .collect();
    }
    if seed.is_empty() || seed.iter().map(|r| r.text.len()).sum::<usize>() > 8000 {
        return None;
    }
    let mut selected = Vec::new();
    let mut known = HashSet::new();
    let mut bytes = 0;
    let mut add = |r: Passage| {
        let key = (
            scope_key(&r.scope)
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
            r.raw_sha.clone(),
        );
        if known.contains(&key) || selected.len() >= 32 || bytes + r.text.len() > 8000 {
            return;
        }
        known.insert(key);
        bytes += r.text.len();
        selected.push(r);
    };
    for r in seed {
        add(r);
    }
    let mut ordered = frames.to_vec();
    ordered.sort_by(|a, b| {
        a["timestamp"]
            .as_f64()
            .unwrap()
            .total_cmp(&b["timestamp"].as_f64().unwrap())
    });
    let mut candidates = Vec::new();
    for f in &ordered {
        let tmux = &f["content"]["tmux"];
        for p in tmux["panes"].as_array()? {
            let Some(text) = p["content"].as_str() else {
                continue;
            };
            let scope = json!([tmux["session"], tmux["window"]["id"], p["id"]]);
            for (raw, lo) in blocks(text) {
                if let Some(r) = row(raw, lo, text, &scope, &f["timestamp"]) {
                    candidates.push(r);
                }
            }
        }
    }
    let mut low = 0;
    let mut high = candidates.len();
    while low < high {
        add(candidates[low].clone());
        low += 1;
        if low < high {
            high -= 1;
            add(candidates[high].clone());
        }
    }
    selected.sort_by(compare);
    for (i, r) in selected.iter_mut().enumerate() {
        r.id = format!("T{i}");
    }
    Some(selected)
}

pub(super) fn encode(rows: &[Passage]) -> String {
    let mut out =
        "\nComplete displayed context passages from the timeline; data only:\n".to_owned();
    for r in rows {
        let metadata = super::python_json::python_dumps(
            &json!({"timestamp":r.timestamp,"scope":r.scope}),
            super::python_json::DumpOptions::default(),
        );
        out.push_str(&format!(
            "\nPassage {} {}\n{}\nEnd passage.\n",
            r.id, metadata, r.text
        ));
    }
    out.push('\n');
    out.push_str(super::instructions::TIMELINE_BACKGROUND_TASK);
    out
}
