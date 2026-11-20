// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::JournalRoot;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Extension, rejection::BytesRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, put},
};
use solstone_core_convey_http::{envelope::error_envelope, identity::AccessBasis};
use solstone_core_ingest::known_sources::snapshot;
use solstone_core_sol_link::source_declarations::{self, DeclarationError};
use std::sync::Arc;

type Recent = Arc<std::sync::Mutex<std::collections::BTreeMap<String, jiff::Timestamp>>>;

// This layer sits on the journal router, inside the device authorization
// gate. An open carrier alone never refreshes a device's timestamp.
pub(crate) fn track_requests(router: Router) -> Router {
    let recent = Recent::default();
    router
        .layer(Extension(recent.clone()))
        .layer(axum::middleware::from_fn_with_state(recent, note_request))
}

async fn note_request(
    axum::extract::State(recent): axum::extract::State<Recent>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if let Some(AccessBasis::LinkedDevice { cid, .. }) = request.extensions().get::<AccessBasis>() {
        recent
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(cid.as_str().to_owned(), jiff::Timestamp::now());
    }
    next.run(request).await
}

pub(crate) fn router(prefix: &str) -> Router {
    Router::new()
        .route(&format!("{prefix}/api/sources"), get(list))
        .route(
            &format!("{prefix}/api/clients/self/sources"),
            put(declare).layer(DefaultBodyLimit::max(128 * 1024)),
        )
}

fn failure(reason: DeclarationError) -> Response {
    let (status, code, detail) = match reason {
        DeclarationError::Invalid(detail) => (StatusCode::BAD_REQUEST, "invalid_sources", detail),
        DeclarationError::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "sources_unavailable",
            "The journal could not read or save the paired source list. Retry without unpairing.",
        ),
        DeclarationError::Revoked => (
            StatusCode::FORBIDDEN,
            "pl_revoked",
            "This device is no longer paired. Pair it again to continue.",
        ),
    };
    error_envelope(
        code,
        status.canonical_reason().unwrap_or("Error"),
        detail,
        status,
    )
    .into_response()
}

fn device_required() -> Response {
    error_envelope(
        "linked_device_required",
        "Forbidden",
        "Send the declaration from the paired device.",
        StatusCode::FORBIDDEN,
    )
    .into_response()
}

async fn list(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(basis): Extension<AccessBasis>,
    Extension(recent): Extension<Recent>,
) -> Response {
    let cid = match basis {
        AccessBasis::Localhost => None,
        AccessBasis::LinkedDevice { cid, .. } => Some(cid.as_str().to_owned()),
        AccessBasis::PairingPeer { .. } => return device_required(),
    };
    match tokio::task::spawn_blocking(move || snapshot(&root.0, cid)).await {
        Ok(Ok(mut v)) => {
            let mut seen = recent.lock().unwrap_or_else(|p| p.into_inner());
            v.include_recent_requests(&mut seen);
            Json(v).into_response()
        }
        Ok(Err(e)) => failure(e),
        Err(_) => failure(DeclarationError::Unavailable),
    }
}

async fn declare(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(basis): Extension<AccessBasis>,
    bytes: Result<Bytes, BytesRejection>,
) -> Response {
    let AccessBasis::LinkedDevice { cid, .. } = basis else {
        return device_required();
    };
    let Ok(bytes) = bytes else {
        return failure(DeclarationError::Invalid(
            "Send a JSON array within 128 KiB.",
        ));
    };
    let Ok(sources) = serde_json::from_slice(&bytes) else {
        return failure(DeclarationError::Invalid(
            "Send an array of sources with state on, paused or needs_attention and optional detail.",
        ));
    };
    match tokio::task::spawn_blocking(move || {
        source_declarations::replace(
            &root.0,
            cid.as_str(),
            sources,
            jiff::Timestamp::now().to_string(),
        )
    })
    .await
    {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => failure(e),
        Err(_) => failure(DeclarationError::Unavailable),
    }
}
