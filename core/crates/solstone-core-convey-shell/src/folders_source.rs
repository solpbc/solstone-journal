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
    let link_ctx = read_device_link_context(journal);
    let (segments_by_id, backfill_by_id) = scan_folder_segments(journal);
    let mut folders = Vec::with_capacity(states.len());
    for state in states {
        let (device_id, last_added_at, folder_state) = resolve_device_and_state(&state, &link_ctx);
        let device_label = link_ctx
            .labels
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
            state: folder_state,
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

#[cfg(not(feature = "host"))]
fn primary_device_and_time(state: &FolderState) -> (String, Option<String>) {
    let best = state.devices.iter().max_by(|(cid_a, ts_a), (cid_b, ts_b)| {
        parse_ts(ts_a)
            .cmp(&parse_ts(ts_b))
            .then_with(|| ts_a.cmp(ts_b))
            .then_with(|| cid_a.cmp(cid_b))
    });
    match best {
        Some((cid, ts)) => (cid.clone(), Some(ts.clone())),
        None => (String::new(), state.last_synced_at.clone()),
    }
}

fn parse_ts(raw: &str) -> Option<jiff::Timestamp> {
    raw.parse::<jiff::Timestamp>().ok()
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
struct DeviceLinkContext {
    labels: BTreeMap<String, String>,
    authorized: Option<BTreeSet<String>>,
    declarations: BTreeMap<String, solstone_core_sol_link::source_declarations::Declaration>,
}

#[cfg(feature = "host")]
fn read_device_link_context(journal: &Path) -> DeviceLinkContext {
    let mut labels = BTreeMap::new();
    let authorized = match solstone_core_sol_link::ledger::read_authorized_clients(
        &journal.join("link/authorized_clients.json"),
    ) {
        solstone_core_sol_link::ledger::AuthorizedClientsRead::Present(clients) => {
            let mut set = BTreeSet::new();
            for entry in clients {
                let trimmed = entry.device_label.trim();
                if !trimmed.is_empty() {
                    labels.insert(entry.fingerprint.clone(), trimmed.to_owned());
                }
                set.insert(entry.fingerprint);
            }
            Some(set)
        }
        _ => None,
    };
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
    let declarations =
        solstone_core_sol_link::source_declarations::read(journal).unwrap_or_default();
    DeviceLinkContext {
        labels,
        authorized,
        declarations,
    }
}

#[cfg(feature = "host")]
fn declared_state_word(state: &solstone_core_sol_link::source_declarations::State) -> &'static str {
    use solstone_core_sol_link::source_declarations::State;
    match state {
        State::On => "on",
        State::Paused => "paused",
        State::NeedsAttention => "needs_attention",
    }
}

#[cfg(feature = "host")]
fn find_declared_folder<'a>(
    decl: &'a solstone_core_sol_link::source_declarations::Declaration,
    folder_id: &str,
) -> Option<&'a solstone_core_sol_link::source_declarations::State> {
    decl.sources
        .iter()
        .find(|entry| entry.source == "folder" && entry.detail.as_deref() == Some(folder_id))
        .map(|entry| &entry.state)
}

#[cfg(feature = "host")]
fn resolve_device_and_state(
    folder: &FolderState,
    ctx: &DeviceLinkContext,
) -> (String, Option<String>, &'static str) {
    let is_authorized = |cid: &str| ctx.authorized.as_ref().is_none_or(|set| set.contains(cid));

    // 1a. Prefer an authorized device already recorded on this folder that declares it.
    let declared_from_synced = folder
        .devices
        .iter()
        .filter(|(cid, _)| is_authorized(cid))
        .filter_map(|(cid, ts)| {
            let decl = ctx.declarations.get(cid)?;
            let st = find_declared_folder(decl, &folder.id)?;
            Some((cid, ts, st))
        })
        .max_by(|(cid_a, ts_a, _), (cid_b, ts_b, _)| {
            parse_ts(ts_a)
                .cmp(&parse_ts(ts_b))
                .then_with(|| ts_a.cmp(ts_b))
                .then_with(|| cid_a.cmp(cid_b))
        });
    if let Some((cid, ts, st)) = declared_from_synced {
        return (cid.clone(), Some(ts.clone()), declared_state_word(st));
    }

    // 1b. Handle device rekey: another currently authorized device declares this folder_id.
    let declared_from_any_authorized = ctx
        .declarations
        .iter()
        .filter(|(cid, _)| is_authorized(cid))
        .filter_map(|(cid, decl)| {
            let st = find_declared_folder(decl, &folder.id)?;
            Some((cid, decl.declared_at.as_str(), st))
        })
        .max_by(|(cid_a, ts_a, _), (cid_b, ts_b, _)| {
            parse_ts(ts_a)
                .cmp(&parse_ts(ts_b))
                .then_with(|| ts_a.cmp(ts_b))
                .then_with(|| cid_a.cmp(cid_b))
        });
    if let Some((cid, _, st)) = declared_from_any_authorized {
        let last_added = folder
            .devices
            .get(cid)
            .cloned()
            .or_else(|| folder.last_synced_at.clone());
        return (cid.clone(), last_added, declared_state_word(st));
    }

    // 2. No authorized device explicitly declares this folder_id.
    let best_authorized = folder
        .devices
        .iter()
        .filter(|(cid, _)| is_authorized(cid))
        .max_by(|(cid_a, ts_a), (cid_b, ts_b)| {
            parse_ts(ts_a)
                .cmp(&parse_ts(ts_b))
                .then_with(|| ts_a.cmp(ts_b))
                .then_with(|| cid_a.cmp(cid_b))
        });
    let (device_id, last_added_at) = match best_authorized {
        Some((cid, ts)) => (cid.clone(), Some(ts.clone())),
        None => {
            let fallback = folder
                .devices
                .iter()
                .max_by(|(cid_a, ts_a), (cid_b, ts_b)| {
                    parse_ts(ts_a)
                        .cmp(&parse_ts(ts_b))
                        .then_with(|| ts_a.cmp(ts_b))
                        .then_with(|| cid_a.cmp(cid_b))
                })
                .map(|(cid, ts)| (cid.clone(), Some(ts.clone())))
                .unwrap_or_else(|| (String::new(), folder.last_synced_at.clone()));
            if ctx.authorized.is_some() {
                return (fallback.0, fallback.1, "off");
            }
            fallback
        }
    };

    let Some(decl) = ctx.declarations.get(&device_id) else {
        return (device_id, last_added_at, "on");
    };
    if decl.sources.len() >= solstone_core_sol_link::source_declarations::MAX_ENTRIES {
        return (device_id, last_added_at, "on");
    }
    let omitted_after_sync = match (
        parse_ts(&decl.declared_at),
        last_added_at.as_deref().and_then(parse_ts),
    ) {
        (Some(declared_ts), Some(synced_ts)) => declared_ts >= synced_ts,
        (Some(_), None) => true,
        (None, _) => false,
    };
    let resolved = if omitted_after_sync { "off" } else { "on" };
    (device_id, last_added_at, resolved)
}

#[cfg(not(feature = "host"))]
struct DeviceLinkContext {
    labels: BTreeMap<String, String>,
}

#[cfg(not(feature = "host"))]
fn read_device_link_context(_journal: &Path) -> DeviceLinkContext {
    DeviceLinkContext {
        labels: BTreeMap::new(),
    }
}

#[cfg(not(feature = "host"))]
fn resolve_device_and_state(
    folder: &FolderState,
    _ctx: &DeviceLinkContext,
) -> (String, Option<String>, &'static str) {
    let (device_id, last_added_at) = primary_device_and_time(folder);
    (device_id, last_added_at, "on")
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
    // Older journals may have no binding for a stream. Keep reading those,
    // but avoid walking every recording segment on each sources-page visit.
    let other_streams: BTreeSet<_> = solstone_core_segment::list_stream_bindings(journal)
        .unwrap_or_default()
        .into_iter()
        .filter(|record| !solstone_core_format::folder::is_folder_source_name(&record.source))
        .map(|record| record.name)
        .collect();
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
            let name = stream_entry.file_name();
            if !stream_path.is_dir()
                || name
                    .to_str()
                    .is_some_and(|s| s == "health" || other_streams.contains(s))
            {
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

        let other = solstone_core_segment::bind_named_stream(
            root,
            "20200110",
            "140000_300",
            "another",
            "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            "activity",
            &Default::default(),
        )
        .unwrap();
        let unrelated = root
            .join("chronicle/20200110")
            .join(other.stream)
            .join("140000_300/talents");
        std::fs::create_dir_all(&unrelated).unwrap();
        std::fs::copy(
            seg.join("talents/folder_activity.json"),
            unrelated.join("folder_activity.json"),
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

    #[cfg(feature = "host")]
    fn write_folder_state(
        root: &Path,
        dir_key: &str,
        id: &str,
        name: &str,
        devices: &[(&str, &str)],
        last_synced_at: &str,
    ) {
        let dir = root.join("config/folders").join(dir_key);
        std::fs::create_dir_all(&dir).unwrap();
        let dev_map: serde_json::Map<String, Value> = devices
            .iter()
            .map(|(cid, ts)| ((*cid).to_owned(), Value::String((*ts).to_owned())))
            .collect();
        std::fs::write(
            dir.join("folder.json"),
            serde_json::json!({
                "v": 1,
                "id": id,
                "name": name,
                "facet": null,
                "kinds": ["notes", "documents"],
                "devices": dev_map,
                "last_synced_at": last_synced_at,
                "event_count": 3
            })
            .to_string(),
        )
        .unwrap();
    }

    #[cfg(feature = "host")]
    fn authorize_device(root: &Path, cid: &str, label: &str) {
        use solstone_core_sol_link::ledger::{AuthorizationLedger, ClientEntry, ClientRole};
        let entry = ClientEntry::new(
            cid,
            label,
            "2026-11-20T00:00:00Z",
            "instance",
            ClientRole::Roleless,
        );
        AuthorizationLedger::new(root).add(entry).unwrap();
    }

    #[cfg(feature = "host")]
    async fn fetch_folders_json(root: &Path) -> Value {
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
        serde_json::from_slice(&bytes).unwrap()
    }

    #[cfg(feature = "host")]
    #[tokio::test]
    async fn folders_api_reflects_declared_states_same_name_ids_and_omitted_off() {
        use solstone_core_sol_link::source_declarations::{self, Source, State};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cid = "sha256:1111111111111111111111111111111111111111111111111111111111111111";
        authorize_device(root, cid, "work laptop");

        // Two folders with identical display name ("Notes"), distinct IDs, plus paused/attention/off.
        write_folder_state(
            root,
            "f1",
            "notes-1111",
            "Notes",
            &[(cid, "2026-11-27T17:00:00Z")],
            "2026-11-27T17:00:00Z",
        );
        write_folder_state(
            root,
            "f2",
            "notes-2222",
            "Notes",
            &[(cid, "2026-11-27T17:00:00Z")],
            "2026-11-27T17:00:00Z",
        );
        write_folder_state(
            root,
            "f3",
            "downloads-3333",
            "Downloads",
            &[(cid, "2026-11-27T17:00:00Z")],
            "2026-11-27T17:00:00Z",
        );
        write_folder_state(
            root,
            "f4",
            "archive-4444",
            "Archive",
            &[(cid, "2026-11-27T17:00:00Z")],
            "2026-11-27T17:00:00Z",
        );

        source_declarations::replace(
            root,
            cid,
            vec![
                Source {
                    source: "folder".to_owned(),
                    state: State::On,
                    detail: Some("notes-1111".to_owned()),
                },
                Source {
                    source: "folder".to_owned(),
                    state: State::Paused,
                    detail: Some("notes-2222".to_owned()),
                },
                Source {
                    source: "folder".to_owned(),
                    state: State::NeedsAttention,
                    detail: Some("downloads-3333".to_owned()),
                },
            ],
            "2026-11-27T17:05:00Z".to_owned(),
        )
        .unwrap();

        let body = fetch_folders_json(root).await;
        let by_id = |id: &str| {
            body["folders"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"] == id)
                .unwrap_or_else(|| panic!("missing folder {id}"))
                .clone()
        };
        assert_eq!(by_id("notes-1111")["state"], "on");
        assert_eq!(by_id("notes-2222")["state"], "paused");
        assert_eq!(by_id("downloads-3333")["state"], "needs_attention");
        assert_eq!(by_id("archive-4444")["state"], "off");

        // Clearing all sources with [] moves every synced folder to off.
        source_declarations::replace(root, cid, vec![], "2026-11-27T17:10:00Z".to_owned()).unwrap();
        let cleared = fetch_folders_json(root).await;
        for row in cleared["folders"].as_array().unwrap() {
            assert_eq!(row["state"], "off", "{row}");
        }
    }

    #[cfg(feature = "host")]
    #[tokio::test]
    async fn folders_api_handles_64_cap_saturation_stale_vs_inflight_unpair_and_rekey() {
        use solstone_core_sol_link::source_declarations::{self, MAX_ENTRIES, Source, State};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cid = "sha256:2222222222222222222222222222222222222222222222222222222222222222";
        let old_cid = "sha256:3333333333333333333333333333333333333333333333333333333333333333";
        let new_cid = "sha256:4444444444444444444444444444444444444444444444444444444444444444";
        let quiet_cid = "sha256:5555555555555555555555555555555555555555555555555555555555555555";
        authorize_device(root, cid, "saturated box");
        authorize_device(root, new_cid, "rekeyed laptop");
        authorize_device(root, quiet_cid, "dormant box");

        // 1. Saturated 64-entry snapshot omits folder-overflow: stays "on", not falsely "off".
        write_folder_state(
            root,
            "sat",
            "folder-overflow",
            "Overflow",
            &[(cid, "2026-11-27T17:00:00Z")],
            "2026-11-27T17:00:00Z",
        );
        let full_entries: Vec<Source> = (0..MAX_ENTRIES)
            .map(|idx| Source {
                source: "folder".to_owned(),
                state: State::On,
                detail: Some(format!("folder-{idx:03}")),
            })
            .collect();
        source_declarations::replace(root, cid, full_entries, "2026-11-27T17:05:00Z".to_owned())
            .unwrap();

        // 2. Unpaired device with no other authorized declarant resolves to "off".
        write_folder_state(
            root,
            "unpaired",
            "folder-unpaired",
            "OldDesk",
            &[(old_cid, "2026-11-27T17:00:00Z")],
            "2026-11-27T17:00:00Z",
        );

        // 3. Rekeyed device: recorded under old_cid, now declared paused by authorized new_cid.
        write_folder_state(
            root,
            "rekey",
            "folder-rekey",
            "RekeyDocs",
            &[(old_cid, "2026-11-27T16:00:00Z")],
            "2026-11-27T16:00:00Z",
        );
        source_declarations::replace(
            root,
            new_cid,
            vec![Source {
                source: "folder".to_owned(),
                state: State::Paused,
                detail: Some("folder-rekey".to_owned()),
            }],
            "2026-11-27T17:05:00Z".to_owned(),
        )
        .unwrap();

        // 4. In-flight race (folder synced after last declaration omitting it) stays "on";
        //    dormant old declaration (60 days ago) keeps declared state with no staleness timeout.
        write_folder_state(
            root,
            "inflight",
            "folder-inflight",
            "Fresh",
            &[(quiet_cid, "2026-11-27T17:00:00Z")],
            "2026-11-27T17:00:00Z",
        );
        write_folder_state(
            root,
            "dormant",
            "folder-dormant",
            "Dormant",
            &[(quiet_cid, "2026-09-20T12:00:00Z")],
            "2026-09-20T12:00:00Z",
        );
        source_declarations::replace(
            root,
            quiet_cid,
            vec![Source {
                source: "folder".to_owned(),
                state: State::Paused,
                detail: Some("folder-dormant".to_owned()),
            }],
            "2026-09-25T12:00:00Z".to_owned(),
        )
        .unwrap();

        let body = fetch_folders_json(root).await;
        let by_id = |id: &str| {
            body["folders"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"] == id)
                .unwrap_or_else(|| panic!("missing folder {id}"))
                .clone()
        };
        assert_eq!(by_id("folder-overflow")["state"], "on");
        assert_eq!(by_id("folder-unpaired")["state"], "off");
        assert_eq!(by_id("folder-rekey")["state"], "paused");
        assert_eq!(by_id("folder-rekey")["device_id"], new_cid);
        assert_eq!(by_id("folder-rekey")["device_label"], "rekeyed laptop");
        assert_eq!(by_id("folder-inflight")["state"], "on");
        assert_eq!(by_id("folder-dormant")["state"], "paused");
    }
}
