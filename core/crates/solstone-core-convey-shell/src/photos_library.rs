// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! HTTP routes for the Photos Library source (phase 1).
//!
//! All routes are strictly gated behind `photos_library.enabled == Some(true)`
//! in `config/journal.json` (OFF by default until owner screens and copy land).
//!
//! - Device channel (`/app/devices/photos-library/*`): paired devices only
//!   (`AccessBasis::LinkedDevice`). Every connection is initiated by the app.
//! - Dashboard channel (`/app/sources/photos/api/*`): owner state, range choice,
//!   widen, pause/resume, turn-off (removes holding area only), landed photo
//!   listing, and chosen-set trash.

use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Extension, Query};
use axum::http::{HeaderMap, StatusCode, header::USER_AGENT};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_photos_library::keep::{
    CopyMetadataOverride, TrashSegmentRef, decode_copy_jpeg_base64, ingest_photo_copy,
    trash_chosen_photos,
};
use solstone_core_photos_library::types::{RangeSpec, ScanAssetRow, SmartAlbumCatalogEntry};
use solstone_core_photos_library::{
    choose_range, dashboard_state, lease_wanted, list_landed_photos, mark_wanted_non_copy,
    photos_library_enabled, scan_batch, scan_changes, scan_finish, scan_start, set_paused,
    turn_off, update_device_status, widen_range,
};

use crate::JournalRoot;

const SCAN_BODY_LIMIT: usize = 8 * 1024 * 1024;
const COPY_BODY_LIMIT: usize = 4 * 1024 * 1024;

pub(crate) fn router() -> Router {
    Router::new()
        // Device channel (client-initiated; LinkedDevice only)
        .route(
            "/app/devices/photos-library/scan/start",
            post(device_scan_start).layer(DefaultBodyLimit::max(SCAN_BODY_LIMIT)),
        )
        .route(
            "/app/devices/photos-library/scan/batch",
            post(device_scan_batch).layer(DefaultBodyLimit::max(SCAN_BODY_LIMIT)),
        )
        .route(
            "/app/devices/photos-library/scan/finish",
            post(device_scan_finish).layer(DefaultBodyLimit::max(512 * 1024)),
        )
        .route(
            "/app/devices/photos-library/scan/changes",
            post(device_scan_changes).layer(DefaultBodyLimit::max(SCAN_BODY_LIMIT)),
        )
        .route(
            "/app/devices/photos-library/status",
            post(device_status).layer(DefaultBodyLimit::max(64 * 1024)),
        )
        .route("/app/devices/photos-library/wanted", get(device_wanted))
        .route(
            "/app/devices/photos-library/copy",
            post(device_copy).layer(DefaultBodyLimit::max(COPY_BODY_LIMIT)),
        )
        // Dashboard channel (locked contract with VPX)
        .route("/app/sources/photos/api/state", get(api_state))
        .route(
            "/app/sources/photos/api/choose-range",
            post(api_choose_range),
        )
        .route("/app/sources/photos/api/widen-range", post(api_widen_range))
        .route("/app/sources/photos/api/pause", post(api_pause))
        .route("/app/sources/photos/api/resume", post(api_resume))
        .route("/app/sources/photos/api/turn-off", post(api_turn_off))
        .route("/app/sources/photos/api/photos", get(api_photos))
        .route("/app/sources/photos/api/trash", post(api_trash))
}

fn disabled_response() -> Response {
    error_envelope(
        "photos_library_disabled",
        "Not Found",
        "The photos library source is not enabled on this journal.",
        StatusCode::NOT_FOUND,
    )
    .into_response()
}

fn device_required() -> Response {
    error_envelope(
        "linked_device_required",
        "Forbidden",
        "Send photo library requests from a paired device.",
        StatusCode::FORBIDDEN,
    )
    .into_response()
}

fn owner_required() -> Response {
    error_envelope(
        "owner_required",
        "Forbidden",
        "Sign in to the journal to manage photos.",
        StatusCode::FORBIDDEN,
    )
    .into_response()
}

fn bad_request(detail: impl Into<String>) -> Response {
    error_envelope(
        "invalid_request",
        "Bad Request",
        detail.into(),
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

fn internal_error(detail: impl Into<String>) -> Response {
    error_envelope(
        "photos_library_error",
        "Internal Server Error",
        detail.into(),
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .into_response()
}

fn require_device(basis: &AccessBasis) -> Result<String, Box<Response>> {
    match basis {
        AccessBasis::LinkedDevice { cid, .. } => Ok(cid.as_str().to_owned()),
        AccessBasis::Localhost | AccessBasis::PairingPeer { .. } => {
            Err(Box::new(device_required()))
        }
    }
}

fn require_owner_or_device(basis: &AccessBasis) -> Result<(), Box<Response>> {
    match basis {
        AccessBasis::Localhost | AccessBasis::LinkedDevice { .. } => Ok(()),
        AccessBasis::PairingPeer { .. } => Err(Box::new(owner_required())),
    }
}

#[derive(Deserialize)]
struct ScanStartBody {
    #[serde(default)]
    device_name: String,
    #[serde(default)]
    device_kind: String,
    #[serde(default = "default_authorized")]
    access_state: String,
    #[serde(default)]
    total_items: usize,
    #[serde(default)]
    smart_albums: Vec<SmartAlbumCatalogEntry>,
}

fn default_authorized() -> String {
    "authorized".to_owned()
}

async fn device_scan_start(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(basis): Extension<AccessBasis>,
    Json(body): Json<ScanStartBody>,
) -> Response {
    if !photos_library_enabled(&root.0) {
        return disabled_response();
    }
    let cid = match require_device(&basis) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let name = if body.device_name.trim().is_empty() {
        cid.clone()
    } else {
        body.device_name
    };
    match tokio::task::spawn_blocking(move || {
        scan_start(
            &root.0,
            &cid,
            &name,
            &body.device_kind,
            &body.access_state,
            body.total_items,
            body.smart_albums,
        )
    })
    .await
    {
        Ok(Ok(())) => Json(json!({ "ok": true })).into_response(),
        Ok(Err(e)) => bad_request(e),
        Err(_) => internal_error("scan start task failed"),
    }
}

#[derive(Deserialize)]
struct ScanBatchBody {
    batch_index: u32,
    rows: Vec<ScanAssetRow>,
    #[serde(default)]
    items_checked: Option<usize>,
}

async fn device_scan_batch(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(basis): Extension<AccessBasis>,
    Json(body): Json<ScanBatchBody>,
) -> Response {
    if !photos_library_enabled(&root.0) {
        return disabled_response();
    }
    let cid = match require_device(&basis) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    match tokio::task::spawn_blocking(move || {
        scan_batch(
            &root.0,
            &cid,
            body.batch_index,
            body.rows,
            body.items_checked,
        )
    })
    .await
    {
        Ok(Ok(accepted)) => Json(json!({ "ok": true, "accepted": accepted })).into_response(),
        Ok(Err(e)) => bad_request(e),
        Err(_) => internal_error("scan batch task failed"),
    }
}

#[derive(Deserialize)]
struct ScanFinishBody {
    #[serde(default)]
    items_checked: Option<usize>,
    #[serde(default)]
    earliest_item_at: Option<String>,
    #[serde(default)]
    access_state: Option<String>,
    #[serde(default)]
    icloud_waiting_count: Option<u64>,
}

async fn device_scan_finish(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(basis): Extension<AccessBasis>,
    Json(body): Json<ScanFinishBody>,
) -> Response {
    if !photos_library_enabled(&root.0) {
        return disabled_response();
    }
    let cid = match require_device(&basis) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    match tokio::task::spawn_blocking(move || {
        scan_finish(
            &root.0,
            &cid,
            body.items_checked,
            body.earliest_item_at,
            body.access_state.as_deref(),
            body.icloud_waiting_count,
        )
    })
    .await
    {
        Ok(Ok(summary)) => Json(json!({ "ok": true, "summary": summary })).into_response(),
        Ok(Err(e)) => error_envelope(
            "smart_album_contract_failed",
            "Unprocessable Entity",
            e,
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .into_response(),
        Err(_) => internal_error("scan finish task failed"),
    }
}

#[derive(Deserialize)]
struct ScanChangesBody {
    #[serde(default)]
    added_or_updated: Vec<ScanAssetRow>,
    #[serde(default)]
    removed_ids: Vec<String>,
    #[serde(default)]
    smart_albums: Option<Vec<SmartAlbumCatalogEntry>>,
    #[serde(default)]
    access_state: Option<String>,
    #[serde(default)]
    icloud_waiting_count: Option<u64>,
}

async fn device_scan_changes(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(basis): Extension<AccessBasis>,
    Json(body): Json<ScanChangesBody>,
) -> Response {
    if !photos_library_enabled(&root.0) {
        return disabled_response();
    }
    let cid = match require_device(&basis) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    match tokio::task::spawn_blocking(move || {
        scan_changes(
            &root.0,
            &cid,
            body.added_or_updated,
            body.removed_ids,
            body.smart_albums,
            body.access_state.as_deref(),
            body.icloud_waiting_count,
        )
    })
    .await
    {
        Ok(Ok(summary)) => Json(json!({ "ok": true, "summary": summary })).into_response(),
        Ok(Err(e)) => error_envelope(
            "smart_album_contract_failed",
            "Unprocessable Entity",
            e,
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .into_response(),
        Err(_) => internal_error("scan changes task failed"),
    }
}

#[derive(Deserialize)]
struct DeviceStatusBody {
    #[serde(default)]
    device_name: Option<String>,
    #[serde(default)]
    device_kind: Option<String>,
    #[serde(default)]
    access_state: Option<String>,
    #[serde(default)]
    icloud_waiting_count: Option<u64>,
    #[serde(default)]
    scan_paused: Option<bool>,
}

async fn device_status(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(basis): Extension<AccessBasis>,
    Json(body): Json<DeviceStatusBody>,
) -> Response {
    if !photos_library_enabled(&root.0) {
        return disabled_response();
    }
    let cid = match require_device(&basis) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    match tokio::task::spawn_blocking(move || {
        update_device_status(
            &root.0,
            &cid,
            body.device_name.as_deref(),
            body.device_kind.as_deref(),
            body.access_state.as_deref(),
            body.icloud_waiting_count,
            body.scan_paused,
        )
    })
    .await
    {
        Ok(Ok(())) => Json(json!({ "ok": true })).into_response(),
        Ok(Err(e)) => bad_request(e),
        Err(_) => internal_error("device status task failed"),
    }
}

#[derive(Deserialize)]
struct WantedQuery {
    #[serde(default = "default_wanted_limit")]
    limit: usize,
}

const fn default_wanted_limit() -> usize {
    20
}

async fn device_wanted(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(basis): Extension<AccessBasis>,
    Query(query): Query<WantedQuery>,
) -> Response {
    if !photos_library_enabled(&root.0) {
        return disabled_response();
    }
    let cid = match require_device(&basis) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    match tokio::task::spawn_blocking(move || lease_wanted(&root.0, &cid, query.limit, None)).await
    {
        Ok(Ok(resp)) => Json(resp).into_response(),
        Ok(Err(e)) => internal_error(e),
        Err(_) => internal_error("wanted list task failed"),
    }
}

#[derive(Deserialize)]
struct CopyUploadBody {
    asset_id: String,
    #[serde(default)]
    cloud_id: Option<String>,
    /// `"ok"` (default when `jpeg_base64` is provided), `"gone"`, or `"icloud_waiting"`.
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    jpeg_base64: Option<String>,
    #[serde(default)]
    metadata: Option<CopyMetadataOverride>,
}

async fn device_copy(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(basis): Extension<AccessBasis>,
    Json(body): Json<CopyUploadBody>,
) -> Response {
    if !photos_library_enabled(&root.0) {
        return disabled_response();
    }
    let cid = match require_device(&basis) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    match body.status.as_deref() {
        Some("gone" | "icloud_waiting") => {
            let status_str = body.status.unwrap_or_default();
            match tokio::task::spawn_blocking(move || {
                mark_wanted_non_copy(&root.0, &cid, &body.asset_id, &status_str)
            })
            .await
            {
                Ok(Ok(())) => Json(json!({ "ok": true })).into_response(),
                Ok(Err(e)) => bad_request(e),
                Err(_) => internal_error("mark non-copy task failed"),
            }
        }
        None | Some("ok") => {
            let Some(b64) = body.jpeg_base64 else {
                return bad_request("missing jpeg_base64 payload");
            };
            let bytes = match decode_copy_jpeg_base64(&b64) {
                Ok(b) => b,
                Err(e) => return bad_request(e),
            };
            match tokio::task::spawn_blocking(move || {
                ingest_photo_copy(
                    &root.0,
                    &cid,
                    &body.asset_id,
                    body.cloud_id.as_deref(),
                    &bytes,
                    body.metadata.as_ref(),
                )
            })
            .await
            {
                Ok(Ok(outcome)) => Json(outcome).into_response(),
                Ok(Err(e)) => bad_request(e),
                Err(_) => internal_error("photo copy ingest task failed"),
            }
        }
        Some(other) => bad_request(format!("unsupported copy status: {other}")),
    }
}

async fn api_state(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(basis): Extension<AccessBasis>,
) -> Response {
    if !photos_library_enabled(&root.0) {
        return disabled_response();
    }
    if let Err(r) = require_owner_or_device(&basis) {
        return *r;
    }
    match tokio::task::spawn_blocking(move || dashboard_state(&root.0)).await {
        Ok(Ok(state)) => Json(state).into_response(),
        Ok(Err(e)) => internal_error(e),
        Err(_) => internal_error("photos state task failed"),
    }
}

#[derive(Deserialize)]
struct RangeBody {
    range: RangeSpec,
}

async fn api_choose_range(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(basis): Extension<AccessBasis>,
    Json(body): Json<RangeBody>,
) -> Response {
    if !photos_library_enabled(&root.0) {
        return disabled_response();
    }
    if let Err(r) = require_owner_or_device(&basis) {
        return *r;
    }
    match tokio::task::spawn_blocking(move || choose_range(&root.0, &body.range)).await {
        Ok(Ok(state)) => Json(state).into_response(),
        Ok(Err(e)) => bad_request(e),
        Err(_) => internal_error("choose range task failed"),
    }
}

async fn api_widen_range(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(basis): Extension<AccessBasis>,
    Json(body): Json<RangeBody>,
) -> Response {
    if !photos_library_enabled(&root.0) {
        return disabled_response();
    }
    if let Err(r) = require_owner_or_device(&basis) {
        return *r;
    }
    match tokio::task::spawn_blocking(move || widen_range(&root.0, &body.range)).await {
        Ok(Ok(state)) => Json(state).into_response(),
        Ok(Err(e)) => bad_request(e),
        Err(_) => internal_error("widen range task failed"),
    }
}

async fn api_pause(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(basis): Extension<AccessBasis>,
) -> Response {
    if !photos_library_enabled(&root.0) {
        return disabled_response();
    }
    if let Err(r) = require_owner_or_device(&basis) {
        return *r;
    }
    match tokio::task::spawn_blocking(move || set_paused(&root.0, true)).await {
        Ok(Ok(state)) => Json(state).into_response(),
        Ok(Err(e)) => internal_error(e),
        Err(_) => internal_error("pause task failed"),
    }
}

async fn api_resume(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(basis): Extension<AccessBasis>,
) -> Response {
    if !photos_library_enabled(&root.0) {
        return disabled_response();
    }
    if let Err(r) = require_owner_or_device(&basis) {
        return *r;
    }
    match tokio::task::spawn_blocking(move || set_paused(&root.0, false)).await {
        Ok(Ok(state)) => Json(state).into_response(),
        Ok(Err(e)) => internal_error(e),
        Err(_) => internal_error("resume task failed"),
    }
}

async fn api_turn_off(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(basis): Extension<AccessBasis>,
) -> Response {
    if !photos_library_enabled(&root.0) {
        return disabled_response();
    }
    if let Err(r) = require_owner_or_device(&basis) {
        return *r;
    }
    match tokio::task::spawn_blocking(move || turn_off(&root.0)).await {
        Ok(Ok(state)) => Json(state).into_response(),
        Ok(Err(e)) => internal_error(e),
        Err(_) => internal_error("turn off task failed"),
    }
}

#[derive(Deserialize)]
struct PhotosListQuery {
    #[serde(default)]
    month: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

async fn api_photos(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(basis): Extension<AccessBasis>,
    Query(query): Query<PhotosListQuery>,
) -> Response {
    if !photos_library_enabled(&root.0) {
        return disabled_response();
    }
    if let Err(r) = require_owner_or_device(&basis) {
        return *r;
    }
    match tokio::task::spawn_blocking(move || {
        list_landed_photos(
            &root.0,
            query.month.as_deref(),
            query.cursor.as_deref(),
            query.limit,
        )
    })
    .await
    {
        Ok(Ok(page)) => Json(page).into_response(),
        Ok(Err(e)) => internal_error(e),
        Err(_) => internal_error("photos listing task failed"),
    }
}

#[derive(Deserialize)]
struct TrashBody {
    segments: Vec<TrashSegmentRef>,
}

async fn api_trash(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(basis): Extension<AccessBasis>,
    headers: HeaderMap,
    Json(body): Json<TrashBody>,
) -> Response {
    if !photos_library_enabled(&root.0) {
        return disabled_response();
    }
    if let Err(r) = require_owner_or_device(&basis) {
        return *r;
    }
    let ua = headers
        .get(USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    match tokio::task::spawn_blocking(move || {
        trash_chosen_photos(&root.0, &body.segments, ua.as_deref())
    })
    .await
    {
        Ok(Ok(result)) => Json(result).into_response(),
        Ok(Err(e)) => bad_request(e),
        Err(_) => internal_error("photos trash task failed"),
    }
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use solstone_core_convey_http::identity::{Carrier, LinkedDeviceCid};
    use tower::ServiceExt;

    use super::*;

    fn test_app(root: std::path::PathBuf, basis: AccessBasis) -> Router {
        router()
            .layer(Extension(Arc::new(JournalRoot(root))))
            .layer(Extension(basis))
    }

    async fn send_json(
        app: &Router,
        method: &str,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        let req = if let Some(val) = body {
            builder = builder.header("content-type", "application/json");
            builder
                .body(Body::from(serde_json::to_vec(&val).unwrap()))
                .unwrap()
        } else {
            builder.body(Body::empty()).unwrap()
        };
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let parsed = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, parsed)
    }

    #[tokio::test]
    async fn routes_return_404_when_flag_is_off_and_enforce_device_basis_when_on() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();

        // Default OFF -> 404 on both dashboard and device routes.
        let owner_app = test_app(root.clone(), AccessBasis::Localhost);
        let (st, _) = send_json(&owner_app, "GET", "/app/sources/photos/api/state", None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);

        let dev_basis = AccessBasis::LinkedDevice {
            carrier: Carrier::Direct,
            cid: LinkedDeviceCid::try_from(
                "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            )
            .unwrap(),
            leaf_spki: Vec::new(),
        };
        let dev_app = test_app(root.clone(), dev_basis);
        let (st, _) = send_json(&dev_app, "GET", "/app/devices/photos-library/wanted", None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);

        // Turn ON flag -> Localhost refused on device routes, allowed on dashboard routes.
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config/journal.json"),
            r#"{"photos_library":{"enabled":true}}"#,
        )
        .unwrap();

        let (st, _) = send_json(
            &owner_app,
            "GET",
            "/app/devices/photos-library/wanted",
            None,
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);

        let (st, body) = send_json(&owner_app, "GET", "/app/sources/photos/api/state", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["source"], "photos");
        assert_eq!(body["state"], "ready_to_set_up");

        // LinkedDevice drives full scan -> choose-range -> wanted -> pause -> resume -> turn-off.
        let (st, _) = send_json(
            &dev_app,
            "POST",
            "/app/devices/photos-library/scan/start",
            Some(json!({
                "device_name": "Test Phone",
                "device_kind": "iphone",
                "access_state": "authorized",
                "total_items": 1,
                "smart_albums": [
                    {"id": "sa1", "subtype": 1000000220, "title": "Captured by Me", "count": 1},
                    {"id": "sa2", "subtype": 1000000218, "title": "Recently Saved", "count": 0}
                ]
            })),
        )
        .await;
        assert_eq!(st, StatusCode::OK);

        let (st, _) = send_json(
            &dev_app,
            "POST",
            "/app/devices/photos-library/scan/batch",
            Some(json!({
                "batch_index": 0,
                "items_checked": 1,
                "rows": [{
                    "id": "photo-1",
                    "capturedAt": "2026-11-19T12:00:00Z",
                    "mediaType": "photo",
                    "albums": [{"id": "sa1", "kind": "smart", "subtype": 1000000220}]
                }]
            })),
        )
        .await;
        assert_eq!(st, StatusCode::OK);

        let (st, _) = send_json(
            &dev_app,
            "POST",
            "/app/devices/photos-library/scan/finish",
            Some(json!({"items_checked": 1, "access_state": "authorized"})),
        )
        .await;
        assert_eq!(st, StatusCode::OK);

        // Before range choice -> wanted list is empty!
        let (st, wanted) =
            send_json(&dev_app, "GET", "/app/devices/photos-library/wanted", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(wanted["wanted"].as_array().unwrap().len(), 0);

        // Owner chooses "all" -> wanted list exposes photo-1.
        let (st, chosen) = send_json(
            &owner_app,
            "POST",
            "/app/sources/photos/api/choose-range",
            Some(json!({"range": "all"})),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(chosen["choice"]["range"], "all");

        let (st, wanted) =
            send_json(&dev_app, "GET", "/app/devices/photos-library/wanted", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(wanted["wanted"].as_array().unwrap().len(), 1);
        assert_eq!(wanted["wanted"][0]["asset_id"], "photo-1");

        // Pause stops wanted leases; resume restores them; turn-off removes holding area.
        let (st, paused) =
            send_json(&owner_app, "POST", "/app/sources/photos/api/pause", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(paused["state"], "paused");
        let (st, wanted) =
            send_json(&dev_app, "GET", "/app/devices/photos-library/wanted", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(wanted["paused"], true);
        assert_eq!(wanted["wanted"].as_array().unwrap().len(), 0);

        let (st, resumed) =
            send_json(&owner_app, "POST", "/app/sources/photos/api/resume", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(resumed["state"], "on");

        let (st, off) =
            send_json(&owner_app, "POST", "/app/sources/photos/api/turn-off", None).await;
        assert_eq!(st, StatusCode::OK);
        assert!(!root.join("config/photos-library").exists());
        assert!(off["state"] == "ready_to_set_up" || off["state"] == "off");
    }
}
