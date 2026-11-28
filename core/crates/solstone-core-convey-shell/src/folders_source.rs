// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Dashboard summary route (`GET /app/sources/api/folders`) for watched folders.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use axum::extract::Extension;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use serde_json::Value;
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_ingest::folder::{FolderState, list_synced_folders};
use solstone_core_segment::is_activity_only_folder_dir;

use crate::JournalRoot;

pub(crate) fn router() -> Router {
    Router::new().route("/app/sources/api/folders", get(api_folders))
}

#[derive(Debug, Serialize)]
struct FoldersResponse {
    folders: Vec<FolderSummaryRow>,
}

#[derive(Debug, Serialize)]
struct FolderSummaryRow {
    id: String,
    name: String,
    device_id: String,
    device_label: String,
    state: &'static str,
    facet: Option<FacetBadge>,
    kinds: Vec<String>,
    items_count: u64,
    segments_count: u64,
    last_added_at: Option<String>,
    backfill_active: bool,
}

#[derive(Debug, Serialize)]
struct FacetBadge {
    name: String,
    title: String,
    color: Option<String>,
    icon: Option<String>,
    emoji: Option<String>,
}

async fn api_folders(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(basis): Extension<AccessBasis>,
) -> Response {
    match basis {
        AccessBasis::Localhost | AccessBasis::LinkedDevice { .. } => {}
        AccessBasis::PairingPeer { .. } => {
            return error_envelope(
                "owner_required",
                "Forbidden",
                "Open this from your journal or a paired device to view folders.",
                StatusCode::FORBIDDEN,
            )
            .into_response();
        }
    }
    match tokio::task::spawn_blocking(move || build_folders_response(&root.0)).await {
        Ok(Ok(payload)) => Json(payload).into_response(),
        Ok(Err(detail)) => error_envelope(
            "folders_read_failed",
            "Internal Server Error",
            detail,
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response(),
        Err(_) => error_envelope(
            "folders_read_failed",
            "Internal Server Error",
            "folders read task failed",
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response(),
    }
}

fn build_folders_response(journal: &Path) -> Result<FoldersResponse, String> {
    let states = list_synced_folders(journal).map_err(|error| error.to_string())?;
    if states.is_empty() {
        return Ok(FoldersResponse {
            folders: Vec::new(),
        });
    }
    let device_labels = read_device_labels(journal);
    let (segments_by_id, backfill_by_id) = scan_folder_segments(journal);
    let mut folders = Vec::with_capacity(states.len());
    for state in states {
        let (device_id, last_added_at) = primary_device_and_time(&state);
        let device_label = device_labels
            .get(&device_id)
            .cloned()
            .unwrap_or_else(|| "this device".to_owned());
        let facet = state
            .facet
            .as_deref()
            .and_then(|slug| read_facet_badge(journal, slug));
        let segments_count = segments_by_id.get(&state.id).copied().unwrap_or(0);
        let backfill_active = backfill_by_id.contains(&state.id);
        folders.push(FolderSummaryRow {
            id: state.id,
            name: state.name,
            device_id,
            device_label,
            state: "on",
            facet,
            kinds: state.kinds,
            items_count: state.event_count,
            segments_count,
            last_added_at,
            backfill_active,
        });
    }
    Ok(FoldersResponse { folders })
}

fn primary_device_and_time(state: &FolderState) -> (String, Option<String>) {
    let best = state
        .devices
        .iter()
        .max_by(|(cid_a, ts_a), (cid_b, ts_b)| ts_a.cmp(ts_b).then_with(|| cid_a.cmp(cid_b)));
    match best {
        Some((cid, ts)) => (cid.clone(), Some(ts.clone())),
        None => (String::new(), state.last_synced_at.clone()),
    }
}

fn read_facet_badge(journal: &Path, slug: &str) -> Option<FacetBadge> {
    let declaration = solstone_core_facets::observe_facet_declaration(journal, slug)
        .ok()
        .flatten()?;
    let obj = declaration.value().as_object()?;
    if obj.contains_key("id")
        && !obj
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(solstone_core_facets::is_well_formed_facet_id)
    {
        return None;
    }
    let title = obj
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| slug.to_owned());
    let color = obj.get("color").and_then(Value::as_str).map(str::to_owned);
    let icon = obj.get("icon").and_then(Value::as_str).map(str::to_owned);
    let emoji = obj.get("emoji").and_then(Value::as_str).map(str::to_owned);
    Some(FacetBadge {
        name: slug.to_owned(),
        title,
        color,
        icon,
        emoji,
    })
}

#[cfg(feature = "host")]
fn read_device_labels(journal: &Path) -> BTreeMap<String, String> {
    let mut labels = BTreeMap::new();
    if let solstone_core_sol_link::ledger::AuthorizedClientsRead::Present(clients) =
        solstone_core_sol_link::ledger::read_authorized_clients(
            &journal.join("link/authorized_clients.json"),
        )
    {
        for entry in clients {
            let trimmed = entry.device_label.trim();
            if !trimmed.is_empty() {
                labels.insert(entry.fingerprint, trimmed.to_owned());
            }
        }
    }
    if let Ok(descriptions) =
        solstone_core_sol_link::client_description_store::read_descriptions(journal)
    {
        for (cid, desc) in descriptions {
            let preferred = desc
                .owner_label
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .or_else(|| {
                    desc.reported
                        .as_ref()
                        .and_then(|reported| reported.name.as_deref())
                        .map(str::trim)
                        .filter(|v| !v.is_empty())
                });
            if let Some(label) = preferred {
                labels.insert(cid, label.to_owned());
            }
        }
    }
    labels
}

#[cfg(not(feature = "host"))]
fn read_device_labels(_journal: &Path) -> BTreeMap<String, String> {
    BTreeMap::new()
}

/// Count live folder segments per `folder_id` and report which folders still
/// have content-bearing past-day segments waiting on Band 4.
fn scan_folder_segments(journal: &Path) -> (BTreeMap<String, u64>, BTreeSet<String>) {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut backfill: BTreeSet<String> = BTreeSet::new();
    let today_key = solstone_core_journal_config::owner_day(journal, jiff::Timestamp::now());
    let Ok(day_entries) = std::fs::read_dir(journal.join("chronicle")) else {
        return (counts, backfill);
    };
    for day_entry in day_entries.flatten() {
        let day_path = day_entry.path();
        if !day_path.is_dir() {
            continue;
        }
        let Some(day_name) = day_entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if day_name.len() != 8 || !day_name.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let is_past = day_name.as_str() < today_key.as_str();
        let Ok(entries) = std::fs::read_dir(&day_path) else {
            continue;
        };
        for stream_entry in entries.flatten() {
            let stream_path = stream_entry.path();
            if !stream_path.is_dir() || stream_entry.file_name().to_str() == Some("health") {
                continue;
            }
            let Ok(segs) = std::fs::read_dir(&stream_path) else {
                continue;
            };
            for seg_entry in segs.flatten() {
                let seg_path = seg_entry.path();
                if !seg_path.is_dir() {
                    continue;
                }
                let activity_path = seg_path.join("talents/folder_activity.json");
                if !activity_path.is_file() {
                    continue;
                }
                let Ok(raw) = std::fs::read_to_string(&activity_path) else {
                    continue;
                };
                let Ok(value) = serde_json::from_str::<Value>(&raw) else {
                    continue;
                };
                let Some(folder_id) = value.get("folder_id").and_then(Value::as_str) else {
                    continue;
                };
                *counts.entry(folder_id.to_owned()).or_insert(0) += 1;
                if is_past
                    && !is_activity_only_folder_dir(&seg_path)
                    && !seg_path.join("talents/sense.json").is_file()
                    && !seg_path.join("talents/sense.md").is_file()
                {
                    backfill.insert(folder_id.to_owned());
                }
            }
        }
    }
    (counts, backfill)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn folders_api_lists_synced_folders_with_facet_counts_and_backfill_flag() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        solstone_core_facets::create_facet(root, "work", "Work", "", "#2E6F9E", "", None).unwrap();
        std::fs::create_dir_all(root.join("config/folders/0123456789abcdef0123456789abcdef"))
            .unwrap();
        std::fs::write(
            root.join("config/folders/0123456789abcdef0123456789abcdef/folder.json"),
            serde_json::json!({
                "v": 1,
                "id": "downloads-1",
                "name": "Downloads",
                "facet": "work",
                "kinds": ["notes", "documents", "other"],
                "devices": { "sha256:1111": "2026-11-27T17:42:00Z" },
                "last_synced_at": "2026-11-27T17:42:00Z",
                "event_count": 12
            })
            .to_string(),
        )
        .unwrap();
        let seg = root.join("chronicle/20200110/laptop_dl/140000_300");
        std::fs::create_dir_all(seg.join("talents")).unwrap();
        std::fs::write(seg.join("note_transcript.md"), "# note\n").unwrap();
        std::fs::write(
            seg.join("talents/folder_activity.json"),
            serde_json::json!({
                "v": 1,
                "folder_id": "downloads-1",
                "folder_name": "Downloads",
                "events": []
            })
            .to_string(),
        )
        .unwrap();

        let app = router()
            .layer(Extension(Arc::new(JournalRoot(root.to_path_buf()))))
            .layer(Extension(AccessBasis::Localhost));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/app/sources/api/folders")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["folders"].as_array().unwrap().len(), 1);
        let row = &body["folders"][0];
        assert_eq!(row["name"], "Downloads");
        assert_eq!(row["facet"]["name"], "work");
        assert_eq!(row["items_count"], 12);
        assert_eq!(row["segments_count"], 1);
        assert_eq!(row["backfill_active"], true);
        assert_eq!(row["state"], "on");
    }
}
