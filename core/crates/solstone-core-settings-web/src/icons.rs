// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashMap;
use std::sync::LazyLock;

use axum::{extract::Query, response::Response};
use serde_json::{Value, json};
use solstone_core_convey_http::icons as shared_icons;

use crate::http::json_response;

static TAGS: LazyLock<HashMap<String, Vec<String>>> = LazyLock::new(|| {
    serde_json::from_str(include_str!(
        "../../solstone-core-convey-shell/assets/static/icons/lucide-tags.json"
    ))
    .expect("embedded Lucide tags")
});

#[derive(Debug)]
struct EmojiEntry {
    emoji: String,
    label: String,
    tags: Vec<String>,
}

static EMOJIS: LazyLock<Vec<EmojiEntry>> = LazyLock::new(|| {
    let raw: Vec<(String, String, String)> = serde_json::from_str(include_str!(
        "../../solstone-core-convey-shell/assets/static/icons/emoji-tags.json"
    ))
    .expect("embedded emoji catalogue");
    raw.into_iter()
        .map(|(emoji, label, tags)| EmojiEntry {
            emoji,
            label,
            tags: tags.split_whitespace().map(str::to_owned).collect(),
        })
        .collect()
});

/// The named Lucide icon's SVG. An emoji is never turned into an icon: it is
/// shown as the owner chose it.
pub fn svg(name: Option<&str>) -> Option<String> {
    shared_icons::svg(name)
}

pub async fn search(Query(query): Query<HashMap<String, String>>) -> Response {
    let icons = shared_icons::catalogue();
    let needle = query
        .get("q")
        .map_or("", |value| value.trim())
        .to_ascii_lowercase();
    let limit = query
        .get("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(80)
        .clamp(1, 200);
    let mut rows: Vec<(u8, usize, String)> = icons
        .keys()
        .filter_map(|name| rank(name, &needle).map(|rank| (rank, name.len(), name.clone())))
        .collect();
    rows.sort();
    json_response(
        json!({"icons": rows.into_iter().take(limit).map(|(_, _, name)| json!({"name": name, "svg": icons[&name]})).collect::<Vec<Value>>() }),
    )
}

pub async fn search_emojis(Query(query): Query<HashMap<String, String>>) -> Response {
    let raw_q = query.get("q").map_or("", |value| value.trim());
    let needle = raw_q.to_ascii_lowercase();
    let limit = query
        .get("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(80)
        .clamp(1, 200);
    let mut rows: Vec<(u8, usize, &EmojiEntry)> = EMOJIS
        .iter()
        .enumerate()
        .filter_map(|(idx, entry)| rank_emoji(entry, raw_q, &needle).map(|rank| (rank, idx, entry)))
        .collect();
    rows.sort_by_key(|(rank, idx, _)| (*rank, *idx));
    json_response(json!({
        "emojis": rows
            .into_iter()
            .take(limit)
            .map(|(_, _, entry)| json!({"emoji": entry.emoji, "label": entry.label}))
            .collect::<Vec<Value>>()
    }))
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

fn rank_emoji(entry: &EmojiEntry, raw_q: &str, needle: &str) -> Option<u8> {
    if needle.is_empty() {
        return Some(0);
    }
    if entry.emoji == raw_q || entry.emoji.replace('\u{FE0F}', "") == raw_q.replace('\u{FE0F}', "")
    {
        return Some(0);
    }
    if entry.label == needle {
        return Some(1);
    }
    let words: Vec<&str> = entry.label.split_whitespace().collect();
    if words.first() == Some(&needle) {
        return Some(2);
    }
    if words.contains(&needle) {
        return Some(3);
    }
    if entry.label.starts_with(needle) {
        return Some(4);
    }
    if entry.label.contains(needle) {
        return Some(5);
    }
    if entry.tags.iter().any(|tag| tag == needle) {
        return Some(6);
    }
    entry
        .tags
        .iter()
        .any(|tag| tag.contains(needle))
        .then_some(7)
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use serde_json::Value;
    use tower::ServiceExt;

    use crate::test_support::{established_root, shell_router};

    #[tokio::test]
    async fn emoji_search_returns_curated_order_on_empty_query_and_matches_keywords() {
        let root = established_root();
        let router = shell_router(root.path());

        let empty = router
            .clone()
            .oneshot(
                Request::get("/app/settings/api/emojis?limit=12")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(empty.status(), 200);
        let empty_json: Value =
            serde_json::from_slice(&to_bytes(empty.into_body(), usize::MAX).await.expect("body"))
                .expect("JSON");
        let list = empty_json["emojis"].as_array().expect("emojis array");
        assert_eq!(list.len(), 12);
        assert_eq!(list[0]["emoji"], "💼");

        let hat = router
            .clone()
            .oneshot(
                Request::get("/app/settings/api/emojis?q=hat&limit=20")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(hat.status(), 200);
        let hat_json: Value =
            serde_json::from_slice(&to_bytes(hat.into_body(), usize::MAX).await.expect("body"))
                .expect("JSON");
        assert!(
            hat_json["emojis"]
                .as_array()
                .expect("emojis array")
                .iter()
                .any(|row| row["emoji"] == "🎩"),
            "hat search finds top hat"
        );

        let none = router
            .oneshot(
                Request::get("/app/settings/api/emojis?q=zzzznomatch")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(none.status(), 200);
        let none_json: Value =
            serde_json::from_slice(&to_bytes(none.into_body(), usize::MAX).await.expect("body"))
                .expect("JSON");
        assert_eq!(none_json["emojis"].as_array().expect("array").len(), 0);
    }
}
