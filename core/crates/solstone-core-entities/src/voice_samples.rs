// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    model::copy_text,
    photos,
    router::{RouterState, admitted},
};
use axum::{
    Json,
    extract::{Extension, Path as RoutePath, Query, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use serde_json::{Value, json};
use solstone_core_convey_http::coalesce::ResponseLocale;
use solstone_core_convey_http::window_catalog::{response_locale, text};
use solstone_core_convey_http::{envelope::error_envelope, identity::AccessBasis};
use solstone_core_entity::{
    VoiceprintRemoval, remove_voiceprints_by_key, try_load_entity_voiceprints_file,
};
use std::{path::Path, sync::Arc};

const PAGE_SIZE: usize = 50;

fn failure(locale: &str, detail: impl std::fmt::Display) -> Response {
    log::warn!("voice samples request failed: {detail}");
    error_envelope(
        "entity_operation_failed",
        copy_text(locale, "ENT_VOICE_ERROR"),
        detail.to_string(),
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .into_response()
}

pub(crate) fn list(root: &Path, id: &str, cursor: usize) -> Result<Value, String> {
    let id = photos::entity_id(root, id).map_err(|e| format!("{e:?}"))?;
    let archive = try_load_entity_voiceprints_file(root, &id).map_err(|e| e.to_string())?;
    let metadata = archive.map(|a| a.metadata).unwrap_or_default();
    let total = metadata.len();
    let mut items = Vec::new();
    for raw in metadata.iter().skip(cursor).take(PAGE_SIZE) {
        let value: Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
        items.push(json!({"key": URL_SAFE_NO_PAD.encode(raw), "day": value["day"], "stream": value["stream"], "stream_layout": value["stream_layout"], "segment_key": value["segment_key"], "source": value["source"], "sentence_id": value["sentence_id"]}));
    }
    let next = cursor.saturating_add(items.len());
    Ok(json!({"items": items, "total": total, "cursor": if next < total {Some(next)} else {None}}))
}

pub(crate) fn remove(root: &Path, id: &str, key: &str) -> Result<Value, String> {
    let _trust = solstone_core_entity::hold_entity_trust_lock(root).map_err(|e| e.to_string())?;
    let id = photos::entity_id(root, id).map_err(|e| format!("{e:?}"))?;
    let raw = URL_SAFE_NO_PAD.decode(key).map_err(|e| e.to_string())?;
    let metadata: Value = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
    let archive = try_load_entity_voiceprints_file(root, &id).map_err(|e| e.to_string())?;
    let Some(archive) = archive else {
        return Ok(json!({"removed": 0}));
    };
    let encoder = archive
        .envelope
        .encoder
        .unwrap_or(solstone_core_entity::EncoderIdentity {
            id: "unresolved".into(),
            sha256: "0".repeat(64),
            width: 256,
        });
    let report = remove_voiceprints_by_key(
        root,
        &id,
        &[VoiceprintRemoval {
            key: metadata.clone(),
            expected_metadata: Some(metadata),
        }],
        &encoder,
    )
    .map_err(|e| e.to_string())?;
    Ok(json!({"removed": report.removed_count}))
}

#[derive(Deserialize)]
pub(crate) struct Page {
    #[serde(default)]
    cursor: usize,
}

pub(crate) async fn panel(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(id): RoutePath<String>,
    Query(page): Query<Page>,
    locale: Option<Extension<ResponseLocale>>,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    let locale = response_locale(locale);
    match solstone_core_serving::seam::run_blocking(move || list(&root, &id, page.cursor)).await {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => failure(locale, e),
        Err(e) => failure(locale, e),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Remove {
    key: String,
}

pub(crate) async fn remove_route(
    Extension(b): Extension<AccessBasis>,
    State(root): State<Arc<RouterState>>,
    RoutePath(id): RoutePath<String>,
    locale: Option<Extension<ResponseLocale>>,
    request: Request,
) -> Response {
    if let Some(r) = admitted(&b) {
        return r;
    }
    let locale = response_locale(locale);
    let body = match axum::body::to_bytes(request.into_body(), 16 * 1024)
        .await
        .ok()
        .and_then(|b| serde_json::from_slice::<Remove>(&b).ok())
    {
        Some(body) => body,
        None => {
            return error_envelope(
                "invalid_request",
                copy_text(locale, "ENT_VOICE_ERROR"),
                text(locale, "journal.window.entities.voice.invalid.key"),
                StatusCode::BAD_REQUEST,
            )
            .into_response();
        }
    };
    match solstone_core_serving::seam::run_blocking(move || remove(&root, &id, &body.key)).await {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => failure(locale, e),
        Err(e) => failure(locale, e),
    }
}
