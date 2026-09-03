// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use axum::response::Response;
use serde_json::{Map, Value, json};

use crate::http::json_response;

mod backup_copy {
    include!(concat!(env!("OUT_DIR"), "/backup_copy.rs"));
}

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
        (name, json!({"raw_media": policy.and_then(|values| values.get("raw_media")).cloned().unwrap_or(json!("keep")), "raw_media_days": policy.and_then(|values| values.get("raw_media_days")).cloned().unwrap_or_else(|| raw_media_days.clone())}))
    }).collect::<Map<_, _>>();
    let logs = retention
        .and_then(|values| values.get("journal_logs"))
        .and_then(Value::as_object);
    json_response(json!({
        "summary": {"raw_media_bytes": summary.raw_media_bytes, "raw_media_human": summary.raw_media_human(), "derived_bytes": summary.derived_bytes, "derived_human": summary.derived_human(), "total_segments": summary.total_segments, "segments_with_raw": summary.segments_with_raw, "segments_purged": summary.segments_purged},
        "retention": {"raw_media": raw_media, "raw_media_days": raw_media_days, "per_stream": per_stream, "journal_logs": {"enabled": logs.and_then(|values| values.get("enabled")).cloned().unwrap_or(json!(true)), "days": logs.and_then(|values| values.get("days")).cloned().unwrap_or(json!(30))}},
        "streams": streams(&journal_root),
        "warnings": storage_warnings(&summary, retention, &config, disk_percent(&journal_root)),
    }))
}

fn streams(journal_root: &std::path::Path) -> Vec<Value> {
    solstone_core_segment::list_stream_records_tolerant(journal_root)
        .ok()
        .map(|listing| listing.records)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, state)| json!({"name": state.get("name").and_then(Value::as_str).unwrap_or("")}))
        .collect()
}

fn storage_warnings(
    summary: &solstone_core_retention::StorageSummary,
    retention: Option<&Map<String, Value>>,
    config: &Map<String, Value>,
    disk_percent: Option<f64>,
) -> Vec<Value> {
    let retention = retention.cloned().unwrap_or_default();
    let keep_mode = retention
        .get("raw_media")
        .and_then(Value::as_str)
        .unwrap_or("keep")
        == "keep";
    let nudge = " your journal is set to always retain original media, so nothing is added to the list automatically.";
    let mut warnings = Vec::new();
    let disk_threshold = retention
        .get("storage_warning_disk_percent")
        .and_then(Value::as_f64)
        .or_else(|| {
            retention
                .get("storage_warning_disk_percent")
                .is_none()
                .then_some(80.0)
        });
    if let (Some(threshold), Some(current)) = (disk_threshold, disk_percent)
        && current >= threshold
    {
        let mut message = format!(
            "disk is {current}% full (threshold: {threshold}%). you can adjust your retention settings, or build the list to see what original media is ready for removal."
        );
        if keep_mode {
            message.push_str(nudge);
        }
        warnings.push(json!({"level": "warning", "type": "disk_percent", "message": message, "current": current, "threshold": threshold}));
    }
    if let Some(threshold) = retention
        .get("storage_warning_raw_media_gb")
        .and_then(Value::as_f64)
    {
        let current = round(summary.raw_media_bytes as f64 / 1024_f64.powi(3), 2);
        if current >= threshold {
            let mut message = format!(
                "raw media is {current} GB (threshold: {threshold} GB). you can adjust your retention settings, or build the list to see what original media is ready for removal."
            );
            if keep_mode {
                message.push_str(nudge);
            }
            warnings.push(json!({"level": "warning", "type": "raw_media_gb", "message": message, "current": current, "threshold": threshold}));
        }
    }
    let backup = config.get("backup").and_then(Value::as_object);
    let offload = backup
        .and_then(|backup| backup.get("offload"))
        .and_then(Value::as_object);
    let last_offload = backup
        .and_then(|backup| backup.get("last_offload"))
        .and_then(Value::as_object);
    if offload
        .and_then(|offload| offload.get("enabled"))
        .and_then(Value::as_bool)
        == Some(true)
        && last_offload
            .and_then(|offload| offload.get("status"))
            .and_then(Value::as_str)
            == Some("stalled")
    {
        let reason = last_offload
            .and_then(|offload| offload.get("reason"))
            .and_then(Value::as_str);
        let labels = backup_constants()
            .remove("OFFLOAD_STALL_REASON_LABELS")
            .and_then(|value| value.as_object().cloned())
            .expect("generated offload stall labels");
        let message = [
            backup_constants()
                .remove("OFFLOAD_STALLED_LEAD")
                .and_then(|value| value.as_str().map(ToOwned::to_owned))
                .expect("generated offload stalled lead"),
            reason
                .and_then(|reason| labels.get(reason))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .unwrap_or_default(),
        ]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
        warnings.push(json!({"level": "warning", "type": "offload_stalled", "message": message, "current": null, "threshold": null}));
    }
    warnings
}

#[cfg(unix)]
fn disk_percent(journal_root: &std::path::Path) -> Option<f64> {
    let stats = nix::sys::statvfs::statvfs(journal_root).ok()?;
    let total = stats.blocks() as f64;
    (total > 0.0).then(|| round((total - stats.blocks_free() as f64) / total * 100.0, 1))
}

#[cfg(windows)]
fn disk_percent(journal_root: &std::path::Path) -> Option<f64> {
    let space = solstone_core_journal_io::windows_disk_space(journal_root).ok()?;
    let total = space.total_bytes as f64;
    (total > 0.0).then(|| round((total - space.available_bytes as f64) / total * 100.0, 1))
}

#[cfg(not(any(unix, windows)))]
fn disk_percent(_journal_root: &std::path::Path) -> Option<f64> {
    None
}

fn round(value: f64, places: i32) -> f64 {
    let scale = 10_f64.powi(places);
    (value * scale).round() / scale
}

fn backup_constants() -> Map<String, Value> {
    serde_json::from_str(backup_copy::COPY_JSON).expect("generated backup copy constants")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::{Map, Value, json};
    use tempfile::TempDir;

    use super::{storage_warnings, streams};

    #[test]
    fn storage_warnings_cover_disk_raw_media_and_stalled_offload() {
        let summary = solstone_core_retention::StorageSummary {
            raw_media_bytes: 1024_u64.pow(3),
            ..Default::default()
        };
        let config: Map<String, Value> = serde_json::from_value(json!({
            "retention": {"raw_media": "keep", "storage_warning_disk_percent": 80, "storage_warning_raw_media_gb": 1},
            "backup": {"offload": {"enabled": true}, "last_offload": {"status": "stalled", "reason": "locked"}},
        }))
        .expect("config object");
        let warnings = storage_warnings(
            &summary,
            config.get("retention").and_then(Value::as_object),
            &config,
            Some(87.3),
        );
        assert_eq!(warnings.len(), 3);
        assert_eq!(warnings[0]["type"], "disk_percent");
        assert_eq!(warnings[1]["type"], "raw_media_gb");
        assert_eq!(warnings[2]["type"], "offload_stalled");
        assert_eq!(warnings[2]["current"], Value::Null);
        assert_eq!(warnings[2]["threshold"], Value::Null);
    }

    #[test]
    fn streams_skip_malformed_records() {
        let temporary = TempDir::new().expect("temporary journal");
        let directory = temporary.path().join("streams");
        fs::create_dir_all(&directory).expect("streams directory");
        fs::write(directory.join("good.json"), r#"{"name":"good"}"#).expect("valid stream");
        fs::write(directory.join("broken.json"), "not JSON").expect("invalid stream");
        assert_eq!(streams(temporary.path()), vec![json!({"name": "good"})]);
    }
}
