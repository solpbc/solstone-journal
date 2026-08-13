// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;

use axum::response::Response;
use serde_json::{Map, Value, json};

use crate::http::json_response;

pub async fn get(journal_root: PathBuf) -> Response {
    let summary = solstone_core_retention::compute_storage_summary(&journal_root);
    let config = solstone_core_journal_config::read_journal_config(&journal_root)
        .expect("session gate handled corrupt config")
        .config
        .unwrap_or_default();
    let retention = config.get("retention").and_then(Value::as_object);
    let raw_media = retention
        .and_then(|values| values.get("raw_media"))
        .cloned()
        .unwrap_or(json!("keep"));
    let raw_media_days = retention
        .and_then(|values| values.get("raw_media_days"))
        .cloned()
        .unwrap_or(Value::Null);
    let per_stream = retention.and_then(|values| values.get("per_stream")).and_then(Value::as_object).cloned().unwrap_or_default().into_iter().map(|(name, policy)| {
        let policy = policy.as_object();
        (name, json!({"raw_media": policy.and_then(|values| values.get("raw_media")).cloned().unwrap_or(json!("keep")), "raw_media_days": policy.and_then(|values| values.get("raw_media_days")).cloned().unwrap_or(Value::Null)}))
    }).collect::<Map<_, _>>();
    let logs = retention
        .and_then(|values| values.get("journal_logs"))
        .and_then(Value::as_object);
    json_response(json!({
        "summary": {"raw_media_bytes": summary.raw_media_bytes, "raw_media_human": summary.raw_media_human(), "derived_bytes": summary.derived_bytes, "derived_human": summary.derived_human(), "total_segments": summary.total_segments, "segments_with_raw": summary.segments_with_raw, "segments_purged": summary.segments_purged},
        "retention": {"raw_media": raw_media, "raw_media_days": raw_media_days, "per_stream": per_stream, "journal_logs": {"enabled": logs.and_then(|values| values.get("enabled")).cloned().unwrap_or(json!(true)), "days": logs.and_then(|values| values.get("days")).cloned().unwrap_or(Value::Null)}},
        "streams": streams(&journal_root), "warnings": [],
    }))
}

fn streams(journal_root: &std::path::Path) -> Vec<Value> {
    let Ok(entries) = fs::read_dir(journal_root.join("streams")) else {
        return Vec::new();
    };
    let mut names = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".json").map(ToOwned::to_owned)
        })
        .collect::<Vec<_>>();
    names.sort();
    names
        .into_iter()
        .map(|name| json!({"name": name}))
        .collect()
}
