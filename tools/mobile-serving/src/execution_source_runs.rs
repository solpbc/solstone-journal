// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Typed reads and explicit sync of existing owner source connections.

use super::*;
use axum::body::Bytes;
use axum::extract::Path;

pub(super) enum Request {
    Read(String),
    Sync { source: String, request_id: String },
    Status { source: String, request_id: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SyncRequest {
    request_id: String,
}

pub(super) fn routes(sender: OwnerIngress) -> Router {
    Router::new()
        .route("/api/journal/sources/{source}", get(read))
        .route("/api/journal/sources/{source}/sync", post(sync))
        .route(
            "/api/journal/sources/{source}/sync/{request_id}",
            get(status),
        )
        .with_state(sender)
}

async fn read(State(sender): State<OwnerIngress>, Path(source): Path<String>) -> Response {
    submit_operation(sender, Operation::SourceRun(Request::Read(source))).await
}

async fn sync(
    State(sender): State<OwnerIngress>,
    Path(source): Path<String>,
    body: Bytes,
) -> Response {
    let Ok(request) = serde_json::from_slice::<SyncRequest>(&body) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"source_request_invalid"})),
        )
            .into_response();
    };
    submit_operation(
        sender,
        Operation::SourceRun(Request::Sync {
            source,
            request_id: request.request_id,
        }),
    )
    .await
}

async fn status(
    State(sender): State<OwnerIngress>,
    Path((source, request_id)): Path<(String, String)>,
) -> Response {
    submit_operation(
        sender,
        Operation::SourceRun(Request::Status { source, request_id }),
    )
    .await
}

pub(super) fn execute(
    root: &Arc<JournalRoot>,
    request: Request,
    cancelled: &(dyn Fn() -> bool + Send + Sync),
) -> (StatusCode, Value) {
    let source = match &request {
        Request::Read(source) | Request::Sync { source, .. } | Request::Status { source, .. } => {
            source
        }
    };
    if schedules::source_command(source).is_none() {
        return (StatusCode::BAD_REQUEST, json!({"error":"source_unknown"}));
    }
    match &request {
        Request::Sync { request_id, .. } | Request::Status { request_id, .. }
            if crate::foreground::source_sync_reference(source, request_id).is_none() =>
        {
            return (
                StatusCode::BAD_REQUEST,
                json!({"error":"source_request_invalid"}),
            );
        }
        _ => {}
    }
    if cancelled() {
        return unavailable();
    }
    match request {
        Request::Read(source) => connection(root, &source),
        Request::Sync { source, request_id } => {
            match crate::foreground::submit_owner_source_sync(root, &source, &request_id) {
                Ok((reference, reused)) => (
                    StatusCode::ACCEPTED,
                    json!({"source":source,"request_id":request_id,"accepted":true,"submitted":!reused,"reused":reused,"reference":reference}),
                ),
                Err(_) => unavailable(),
            }
        }
        Request::Status { source, request_id } => {
            match crate::foreground::owner_source_sync_status(root, &source, &request_id) {
                Ok(Some(status)) => (StatusCode::OK, status),
                Ok(None) => (
                    StatusCode::OK,
                    json!({"source":source,"request_id":request_id,"reference":crate::foreground::source_sync_reference(&source,&request_id),"state":"unknown","terminal":false}),
                ),
                Err(_) => unavailable(),
            }
        }
    }
}

fn unavailable() -> (StatusCode, Value) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        json!({"error":"source_queue_unavailable"}),
    )
}

fn connection(root: &JournalRoot, source: &str) -> (StatusCode, Value) {
    if source == "plaud" {
        return match solstone_core_journal_config::read_journal_config_bound(root) {
            Ok(config) => (
                StatusCode::OK,
                json!({"source":source,"available":solstone_core_settings_web::source_sync_plaud_available(&config.config.unwrap_or_default())}),
            ),
            Err(_) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({"error":"source_state_unavailable"}),
            ),
        };
    }
    #[cfg(unix)]
    {
        use solstone_core_import::{
            BackendName, SyncStateRead, public_connection_status, read_sync_state_bound,
        };
        let backend = match source {
            "obsidian" => BackendName::Obsidian,
            "audio" => BackendName::Audio,
            _ => unreachable!("source whitelist checked before read"),
        };
        match read_sync_state_bound(root, backend) {
            SyncStateRead::Absent => (StatusCode::OK, json!({"source":source,"connection":null})),
            SyncStateRead::Loaded(state) => {
                let connection = match public_connection_status(&state) {
                    Ok(connection) => connection,
                    Err(_) => {
                        return (
                            StatusCode::UNPROCESSABLE_ENTITY,
                            json!({"error":"source_state_unavailable"}),
                        );
                    }
                };
                let connection = connection.map(|connection| {
                    json!({
                        "source_identity":connection.source_identity,
                        "folder_name":connection.folder_name,
                        "remembered":connection.remembered,
                        "access_state":connection.access_state.to_string(),
                    })
                });
                (
                    StatusCode::OK,
                    json!({"source":source,"connection":connection}),
                )
            }
            SyncStateRead::Unreadable { .. } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({"error":"source_state_unavailable"}),
            ),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = root;
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({"error":"source_state_unavailable"}),
        )
    }
}
