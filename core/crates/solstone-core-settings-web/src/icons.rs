// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashMap;
use std::sync::LazyLock;

use axum::{extract::Query, response::Response};
use serde_json::{Value, json};

use crate::http::json_response;

static ICONS: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../solstone/convey/static/icons/lucide.json"
    )))
    .expect("embedded Lucide catalogue")
});
static TAGS: LazyLock<HashMap<String, Vec<String>>> = LazyLock::new(|| {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../solstone/convey/static/icons/lucide-tags.json"
    )))
    .expect("embedded Lucide tags")
});
static EMOJI: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../solstone/convey/static/icons/emoji-lucide.json"
    )))
    .expect("embedded emoji icon map")
});

pub fn svg(name: Option<&str>, emoji: &str) -> Option<String> {
    name.and_then(|name| ICONS.get(name)).cloned().or_else(|| {
        (!emoji.is_empty())
            .then(|| EMOJI.get(emoji).and_then(|name| ICONS.get(name)).cloned())
            .flatten()
    })
}

pub async fn search(Query(query): Query<HashMap<String, String>>) -> Response {
    let needle = query
        .get("q")
        .map_or("", |value| value.trim())
        .to_ascii_lowercase();
    let limit = query
        .get("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(80)
        .clamp(1, 200);
    let mut rows: Vec<(u8, usize, String)> = ICONS
        .keys()
        .filter_map(|name| rank(name, &needle).map(|rank| (rank, name.len(), name.clone())))
        .collect();
    rows.sort();
    json_response(
        json!({"icons": rows.into_iter().take(limit).map(|(_, _, name)| json!({"name": name, "svg": ICONS[&name]})).collect::<Vec<Value>>() }),
    )
}

fn rank(name: &str, needle: &str) -> Option<u8> {
    if needle.is_empty() {
        return Some(0);
    }
    let tokens: Vec<&str> = name.split('-').collect();
    if name == needle {
        return Some(0);
    }
    if tokens.first() == Some(&needle) {
        return Some(1);
    }
    if tokens.contains(&needle) {
        return Some(2);
    }
    if name.starts_with(needle) {
        return Some(3);
    }
    if name.contains(needle) {
        return Some(4);
    }
    let tags = TAGS.get(name).map(Vec::as_slice).unwrap_or(&[]);
    if tags
        .iter()
        .any(|tag| tag == needle || tag.split_whitespace().any(|word| word == needle))
    {
        return Some(5);
    }
    tags.iter().any(|tag| tag.contains(needle)).then_some(6)
}
