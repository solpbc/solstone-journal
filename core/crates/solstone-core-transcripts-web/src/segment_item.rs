// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::sync::Arc;

use crate::AppState;
use axum::Json;
use axum::extract::{Path as RoutePath, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use solstone_core_format::content::segment_items;

/// Owner-session reader. Every request resolves the same typed publication used by search.
pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    RoutePath((day, key, id, part)): RoutePath<(String, String, String, String)>,
) -> Response {
    let root = state.journal_root.clone();
    solstone_core_convey_http::owner_read::spawn_blocking_response(
        solstone_core_convey_http::owner_read::OwnerReadRole::TranscriptsSegment,
        move || {
            read(&root, &day, &key, &id, &part)
                .unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
        },
    )
    .await
}

fn read(root: &Path, day: &str, key: &str, id: &str, part: &str) -> Option<Response> {
    if !crate::day::valid_day(day)
        || solstone_core_format::segment::segment_key(key).as_deref() != Some(key)
        || !segment_items::valid_id(id)
    {
        return None;
    }
    let origin = format!("chronicle/{day}/import.email/{key}");
    let segment = root.join(&origin);
    let file = segment_items::published_item(&segment, id)?;
    let name = file.file_name()?.to_str()?;
    let item = format!("{origin}/items/{id}");
    let rel = format!("{item}/{name}");
    if !segment_items::is_published_path(&file, &rel) {
        return None;
    }
    let has_regular = |name: &str| {
        file.parent()
            .and_then(|dir| std::fs::symlink_metadata(dir.join(name)).ok())
            .is_some_and(|meta| meta.is_file())
    };
    let visual = matches!(name, "image_transcript.md" | "screenshot_transcript.md");
    match part {
        "text" => {
            let bytes = segment_items::read_file(root, &rel, 32 * 1024 * 1024)?;
            let text = String::from_utf8(bytes).ok()?;
            Some(Json(json!({
                "text":text, "kind":solstone_core_format::content::kinds::agent_indexed_permission(None, &rel)?.as_str(),
                "preview":visual && has_regular("preview.jpg"), "original":has_regular("original.bin")
            })).into_response())
        }
        "preview" if visual => {
            let bytes =
                segment_items::read_file(root, &format!("{item}/preview.jpg"), 16 * 1024 * 1024)?;
            Some(
                (
                    [
                        (header::CONTENT_TYPE, "image/jpeg"),
                        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                    ],
                    bytes,
                )
                    .into_response(),
            )
        }
        "original" => {
            let bytes =
                segment_items::read_file(root, &format!("{item}/original.bin"), 64 * 1024 * 1024)?;
            Some(
                (
                    [
                        (header::CONTENT_TYPE, "application/octet-stream"),
                        (header::CONTENT_DISPOSITION, "attachment"),
                        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                    ],
                    bytes,
                )
                    .into_response(),
            )
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn owner_reads_typed_item_and_requested_original_until_parent_is_deleted() {
        let root = tempfile::tempdir().unwrap();
        let id = "a".repeat(64);
        let seg = root.path().join("chronicle/20261118/import.email/101600_0");
        let item = seg.join("items").join(&id);
        fs::create_dir_all(&item).unwrap();
        fs::write(seg.join("conversation_transcript.jsonl"), "{}\n").unwrap();
        fs::write(item.join("document_transcript.md"), "a plan").unwrap();
        fs::write(item.join("original.bin"), b"%PDF-").unwrap();
        let response = |part| read(root.path(), "20261118", "101600_0", &id, part);
        assert!(response("text").is_some());
        assert_eq!(
            response("original").unwrap().headers()[header::CONTENT_DISPOSITION],
            "attachment"
        );
        assert!(response("preview").is_none());
        assert!(response("../original.bin").is_none());
        let staging = seg.with_file_name(".removing_101600_0");
        fs::create_dir(&staging).unwrap();
        assert!(response("original").is_none());
        fs::remove_dir(staging).unwrap();
        fs::write(seg.join("tombstone.json"), b"{}").unwrap();
        assert!(response("text").is_none());
        assert!(response("original").is_none());
    }
}
