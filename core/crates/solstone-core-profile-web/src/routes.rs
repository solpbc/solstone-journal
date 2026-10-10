// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Axum transport for the native profile CLI contract.

use std::path::PathBuf;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, FixedOffset, Utc};
use serde::Deserialize;
use solstone_core_convey_http::envelope::error_envelope;

use crate::error::ProfileError;
use crate::pagination::parse_pagination;
use crate::profile;
use crate::types::ActiveCollection;

#[derive(Clone)]
pub(crate) struct RouteState {
    pub(crate) journal_root: PathBuf,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FullQuery {
    facets: Option<String>,
    include_mentions: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CadenceQuery {
    include_mentions: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ActiveQuery {
    window_days: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

/// The current instant in the journal's owner zone: activity days are the
/// owner's days.
fn local_now(journal: &std::path::Path) -> DateTime<FixedOffset> {
    Utc::now()
        .with_timezone(&solstone_core_journal_config::owner_zone(journal))
        .fixed_offset()
}

pub(crate) async fn full(
    State(state): State<RouteState>,
    Path(name): Path<String>,
    Query(query): Query<FullQuery>,
) -> Response {
    match profile::full(
        &state.journal_root,
        &name,
        parse_facets(query.facets.as_deref()).as_deref(),
        truthy(query.include_mentions.as_deref()),
        local_now(&state.journal_root),
    ) {
        Ok(Some(profile)) => Json(profile).into_response(),
        Ok(None) => entity_not_found(&name),
        Err(error) => internal_error(error),
    }
}

pub(crate) async fn brief(State(state): State<RouteState>, Path(name): Path<String>) -> Response {
    match profile::brief(&state.journal_root, &name, local_now(&state.journal_root)) {
        Ok(Some(profile)) => Json(profile).into_response(),
        Ok(None) => entity_not_found(&name),
        Err(error) => internal_error(error),
    }
}

pub(crate) async fn cadence(
    State(state): State<RouteState>,
    Path(name): Path<String>,
    Query(query): Query<CadenceQuery>,
) -> Response {
    match profile::cadence(
        &state.journal_root,
        &name,
        truthy(query.include_mentions.as_deref()),
        local_now(&state.journal_root),
    ) {
        Ok(Some(cadence)) => Json(cadence).into_response(),
        Ok(None) => entity_not_found(&name),
        Err(error) => internal_error(error),
    }
}

pub(crate) async fn active(
    State(state): State<RouteState>,
    Query(query): Query<ActiveQuery>,
) -> Response {
    let window_days = match parse_window_days(query.window_days.as_deref()) {
        Ok(window_days) => window_days,
        Err(detail) => return invalid_request(detail),
    };
    let pagination = parse_pagination(query.limit.as_deref(), query.offset.as_deref());
    match profile::list_active(
        &state.journal_root,
        window_days,
        local_now(&state.journal_root),
    ) {
        Ok(items) => {
            let total = items.len();
            let items = items
                .into_iter()
                .skip(pagination.offset)
                .take(pagination.limit)
                .collect();
            Json(ActiveCollection { items, total }).into_response()
        }
        Err(error) => internal_error(error),
    }
}

fn truthy(value: Option<&str>) -> bool {
    matches!(
        value
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

fn parse_facets(value: Option<&str>) -> Option<Vec<String>> {
    value
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .filter(|facets| !facets.is_empty())
}

fn parse_window_days(value: Option<&str>) -> Result<i64, &'static str> {
    let window_days = match value {
        None => 30,
        Some(value) => value
            .parse::<i64>()
            .map_err(|_| "window_days must be an integer")?,
    };
    if window_days <= 0 {
        return Err("window_days must be positive");
    }
    Ok(window_days)
}

fn entity_not_found(name: &str) -> Response {
    error_envelope(
        "entity_not_found",
        "that entity couldn't be found.",
        format!("no entity named '{name}'"),
        StatusCode::NOT_FOUND,
    )
    .into_response()
}

fn invalid_request(detail: &str) -> Response {
    error_envelope(
        "invalid_request_value",
        "one of those values couldn't be used.",
        detail,
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

fn internal_error(error: ProfileError) -> Response {
    log::error!("profile route failed: {error}");
    error_envelope(
        "profile_unavailable",
        "that profile couldn't be loaded.",
        "profile unavailable",
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .into_response()
}

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode, header},
    };
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use crate::test_support::{journal, write_json, write_jsonl};

    async fn send_request(
        router: Router,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(uri);
        if body.is_some() {
            request = request.header(header::CONTENT_TYPE, "application/json");
        }
        let body = body
            .map(|body| Body::from(serde_json::to_vec(&body).expect("request JSON")))
            .unwrap_or_else(Body::empty);
        let response = router
            .oneshot(request.body(body).expect("request"))
            .await
            .expect("response");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, body)
    }

    #[tokio::test]
    async fn profile_routes_return_data_without_retired_ledger_fields() {
        let temporary = journal();
        write_json(
            temporary.path(),
            "facets/work/facet.json",
            json!({"id":"00000000-0000-4000-8000-000000000001","name":"work","muted":false}),
        );
        write_json(
            temporary.path(),
            "entities/pat/entity.json",
            json!({"id":"pat","name":"Pat","type":"Person"}),
        );
        write_jsonl(
            temporary.path(),
            "facets/work/activities/20260401.jsonl",
            &[json!({
                "id": "meeting_1",
                "created_at": 100,
                "participation": [{"entity_id": "pat", "role": "attendee"}]
            })],
        );

        let router = crate::routes(temporary.path().to_path_buf());
        let (status, body) = send_request(router, "GET", "/api/profile/pat", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["entity_id"], "pat");
        assert_eq!(body["name"], "Pat");
        // Verify retired ledger fields are absent
        assert!(body.get("open_with_them").is_none());
        assert!(body.get("closed_with_them_30d").is_none());
        assert!(body.get("decisions_involving_them").is_none());

        let router = crate::routes(temporary.path().to_path_buf());
        let (status, brief) = send_request(router, "GET", "/api/profile/pat/brief", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(brief["entity_id"], "pat");
        assert!(brief.get("open_loop_count").is_none());
        assert!(brief.get("decisions_count_30d").is_none());

        let router = crate::routes(temporary.path().to_path_buf());
        let (status, cadence) = send_request(router, "GET", "/api/profile/pat/cadence", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(cadence.get("recent_interactions_count_30d").is_some());

        let router = crate::routes(temporary.path().to_path_buf());
        let (status, active) = send_request(router, "GET", "/api/profiles/active", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(active.get("items").is_some());
    }

    #[tokio::test]
    async fn profile_routes_return_404_for_unknown_entity() {
        let temporary = journal();
        let router = crate::routes(temporary.path().to_path_buf());
        let (status, body) = send_request(router, "GET", "/api/profile/unknown", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["reason_code"], "entity_not_found");
    }
}
