// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Axum transport for the native profile CLI contract.

use std::path::PathBuf;

use axum::{
    Json,
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, FixedOffset, Local, Utc};
use serde::Deserialize;
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_facets::LedgerCloseState;

use crate::error::ProfileError;
use crate::ledger_fold::{self, LedgerListQuery, LedgerState};
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

#[derive(Debug, Deserialize)]
pub(crate) struct CloseBody {
    #[serde(default)]
    pub(crate) note: Option<String>,
    #[serde(default)]
    pub(crate) as_state: Option<String>,
}

/// The current instant with the local offset: activity days are local days.
fn local_now() -> DateTime<FixedOffset> {
    Local::now().fixed_offset()
}

fn list_all_query() -> LedgerListQuery {
    LedgerListQuery {
        state: LedgerState::All,
        owner: None,
        counterparty: None,
        age_days_gte: None,
        closed_since: None,
        top: None,
        sort: None,
        facets: None,
    }
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
        local_now(),
    ) {
        Ok(Some(profile)) => Json(profile).into_response(),
        Ok(None) => entity_not_found(&name),
        Err(error) => internal_error(error),
    }
}

pub(crate) async fn brief(State(state): State<RouteState>, Path(name): Path<String>) -> Response {
    match profile::brief(&state.journal_root, &name, local_now()) {
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
        local_now(),
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
    match profile::list_active(&state.journal_root, window_days, local_now()) {
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

pub(crate) async fn item(State(state): State<RouteState>, Path(item_id): Path<String>) -> Response {
    match ledger_fold::list(&state.journal_root, Utc::now(), list_all_query()) {
        Ok(items) => {
            if let Some(item) = items.into_iter().find(|item| item.id == item_id) {
                Json(item).into_response()
            } else {
                ledger_item_unknown()
            }
        }
        Err(error) => internal_error(error),
    }
}

pub(crate) async fn close(
    State(state): State<RouteState>,
    Path(item_id): Path<String>,
    body: Bytes,
) -> Response {
    let payload = match serde_json::from_slice::<CloseBody>(&body) {
        Ok(payload) => payload,
        Err(_) => return invalid_request("the body could not be read."),
    };

    let as_state_target = match payload.as_state.as_deref() {
        None | Some("closed") => LedgerCloseState::Closed,
        Some("dropped") => LedgerCloseState::Dropped,
        Some(_) => return invalid_request("as_state must be closed or dropped."),
    };
    let state_str = as_state_target.as_str();

    let note_str = match payload.note {
        Some(note) if !note.trim().is_empty() => note,
        _ => return invalid_request("a note is required."),
    };

    let now = Utc::now();
    let items = match ledger_fold::list(&state.journal_root, now, list_all_query()) {
        Ok(items) => items,
        Err(error) => return internal_error(error),
    };

    let Some(item) = items.into_iter().find(|item| item.id == item_id) else {
        return ledger_item_unknown();
    };

    let Some(source) = item
        .sources
        .iter()
        .rev()
        .find(|source| source.field == "commitments")
    else {
        return ledger_target_missing();
    };

    if let Err(error) = solstone_core_facets::append_ledger_close(
        &state.journal_root,
        &source.facet,
        &source.day,
        &source.activity_id,
        &item_id,
        as_state_target,
        &note_str,
        now,
    ) {
        return store_error(error);
    }

    let items_after = match ledger_fold::list(&state.journal_root, now, list_all_query()) {
        Ok(items) => items,
        Err(error) => return internal_error(error),
    };

    let Some(item_after) = items_after.into_iter().find(|item| item.id == item_id) else {
        return ledger_target_missing();
    };

    let confirmed = item_after.state == state_str;
    Json(serde_json::json!({
        "item": item_after,
        "confirmed": confirmed,
    }))
    .into_response()
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

fn ledger_item_unknown() -> Response {
    error_envelope(
        "ledger_item_unknown",
        "that item was not found.",
        "that item was not found. run solstone call profile full <name> for current ids.",
        StatusCode::NOT_FOUND,
    )
    .into_response()
}

fn ledger_target_missing() -> Response {
    error_envelope(
        "ledger_target_missing",
        "that item changed.",
        "that item changed. re-read the profile and try again.",
        StatusCode::NOT_FOUND,
    )
    .into_response()
}

fn ledger_busy() -> Response {
    error_envelope(
        "ledger_busy",
        "the close could not be saved right now because it was busy. try again in a moment.",
        "the close could not be saved right now because it was busy. try again in a moment.",
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .into_response()
}

pub(crate) fn store_error(error: solstone_core_facets::ActivityRecordStoreError) -> Response {
    match error {
        solstone_core_facets::ActivityRecordStoreError::MissingRecord { .. }
        | solstone_core_facets::ActivityRecordStoreError::MissingDayFile { .. } => {
            ledger_target_missing()
        }
        solstone_core_facets::ActivityRecordStoreError::Lock(
            solstone_core_journal_io::LockError::Timeout(_),
        ) => ledger_busy(),
        other => internal_error(ProfileError::internal(other)),
    }
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
    use std::time::Duration;

    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode, header},
    };
    use chrono::TimeZone;
    use serde_json::{Value, json};
    use solstone_core_facets::{LedgerCloseState, append_ledger_close};
    use solstone_core_journal_io::{LockError, LockTimeout};
    use tower::ServiceExt;

    use super::*;
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

    #[test]
    fn ledger_close_direct_fold_updates_profile_full_closed_and_dropped() {
        let temporary = journal();
        let clock = Utc.with_ymd_and_hms(2026, 9, 29, 15, 0, 0).unwrap();
        let fixed_now = clock.fixed_offset();

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
                "id": "commit_1",
                "created_at": 100,
                "commitments": [{
                    "owner": "Owner",
                    "owner_entity_id": "owner",
                    "counterparty": "Pat",
                    "counterparty_entity_id": "pat",
                    "action": "send report"
                }]
            })],
        );

        let items_before = ledger_fold::list(temporary.path(), clock, list_all_query()).unwrap();
        assert_eq!(items_before.len(), 1);
        let item_id = items_before[0].id.clone();

        append_ledger_close(
            temporary.path(),
            "work",
            "20260401",
            "commit_1",
            &item_id,
            LedgerCloseState::Closed,
            "closed note",
            clock,
        )
        .expect("append close");

        let items_closed = ledger_fold::list(temporary.path(), clock, list_all_query()).unwrap();
        assert_eq!(items_closed.len(), 1);
        assert_eq!(items_closed[0].state, "closed");
        assert_eq!(items_closed[0].closed_at, Some(clock.timestamp_millis()));
        assert_ne!(items_closed[0].closed_at, Some(100));

        let profile_closed = profile::full(temporary.path(), "pat", None, false, fixed_now)
            .unwrap()
            .expect("profile exists");
        assert_eq!(profile_closed.open_with_them.len(), 0);
        assert_eq!(profile_closed.closed_with_them_30d.len(), 1);
        assert_eq!(profile_closed.closed_with_them_30d[0].id, item_id);
        assert_eq!(profile_closed.closed_with_them_30d[0].state, "closed");
        assert_eq!(
            profile_closed.closed_with_them_30d[0].closed_at,
            Some(clock.timestamp_millis())
        );

        append_ledger_close(
            temporary.path(),
            "work",
            "20260401",
            "commit_1",
            &item_id,
            LedgerCloseState::Dropped,
            "dropped note",
            clock,
        )
        .expect("append drop");

        let items_dropped = ledger_fold::list(temporary.path(), clock, list_all_query()).unwrap();
        assert_eq!(items_dropped.len(), 1);
        assert_eq!(items_dropped[0].state, "dropped");

        let profile_dropped = profile::full(temporary.path(), "pat", None, false, fixed_now)
            .unwrap()
            .expect("profile exists");
        assert_eq!(profile_dropped.open_with_them.len(), 0);
        assert_eq!(profile_dropped.closed_with_them_30d.len(), 0);
    }

    #[tokio::test]
    async fn ledger_close_post_happy_path_closed() {
        let temporary = journal();
        write_json(
            temporary.path(),
            "facets/work/facet.json",
            json!({"id":"00000000-0000-4000-8000-000000000001","name":"work","muted":false}),
        );
        write_jsonl(
            temporary.path(),
            "facets/work/activities/20260401.jsonl",
            &[json!({
                "id": "c1",
                "created_at": 100,
                "commitments": [{"owner":"Owner","action":"send report"}]
            })],
        );

        let router = crate::routes(temporary.path().to_path_buf());
        let items = ledger_fold::list(temporary.path(), Utc::now(), list_all_query()).unwrap();
        let item_id = &items[0].id;

        let (status, body) = send_request(
            router,
            "POST",
            &format!("/api/ledger/{item_id}/close"),
            Some(json!({"note":"done","as_state":"closed"})),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["confirmed"], true);
        assert_eq!(body["item"]["state"], "closed");
        assert_eq!(body["item"]["id"], *item_id);
    }

    #[tokio::test]
    async fn ledger_close_absent_as_state_defaults_to_closed() {
        let temporary = journal();
        write_json(
            temporary.path(),
            "facets/work/facet.json",
            json!({"id":"00000000-0000-4000-8000-000000000001","name":"work","muted":false}),
        );
        write_jsonl(
            temporary.path(),
            "facets/work/activities/20260401.jsonl",
            &[json!({
                "id": "c1",
                "created_at": 100,
                "commitments": [{"owner":"Owner","action":"send report"}]
            })],
        );

        let router = crate::routes(temporary.path().to_path_buf());
        let items = ledger_fold::list(temporary.path(), Utc::now(), list_all_query()).unwrap();
        let item_id = &items[0].id;

        let (status, body) = send_request(
            router,
            "POST",
            &format!("/api/ledger/{item_id}/close"),
            Some(json!({"note":"done"})),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["confirmed"], true);
        assert_eq!(body["item"]["state"], "closed");
        assert_eq!(body["item"]["id"], *item_id);
    }

    #[tokio::test]
    async fn ledger_close_post_closed_then_dropped_then_closed_ends_closed() {
        let temporary = journal();
        write_json(
            temporary.path(),
            "facets/work/facet.json",
            json!({"id":"00000000-0000-4000-8000-000000000001","name":"work","muted":false}),
        );
        write_jsonl(
            temporary.path(),
            "facets/work/activities/20260401.jsonl",
            &[json!({
                "id": "c1",
                "created_at": 100,
                "commitments": [{"owner":"Owner","action":"send report"}]
            })],
        );

        let items = ledger_fold::list(temporary.path(), Utc::now(), list_all_query()).unwrap();
        let item_id = &items[0].id;

        let router1 = crate::routes(temporary.path().to_path_buf());
        let (s1, b1) = send_request(
            router1,
            "POST",
            &format!("/api/ledger/{item_id}/close"),
            Some(json!({"note":"closed 1","as_state":"closed"})),
        )
        .await;
        assert_eq!(s1, StatusCode::OK);
        assert_eq!(b1["confirmed"], true);

        let router2 = crate::routes(temporary.path().to_path_buf());
        let (s2, b2) = send_request(
            router2,
            "POST",
            &format!("/api/ledger/{item_id}/close"),
            Some(json!({"note":"dropped 2","as_state":"dropped"})),
        )
        .await;
        assert_eq!(s2, StatusCode::OK);
        assert_eq!(b2["confirmed"], true);
        assert_eq!(b2["item"]["state"], "dropped");

        let router3 = crate::routes(temporary.path().to_path_buf());
        let (s3, b3) = send_request(
            router3,
            "POST",
            &format!("/api/ledger/{item_id}/close"),
            Some(json!({"note":"closed 3","as_state":"closed"})),
        )
        .await;
        assert_eq!(s3, StatusCode::OK);
        assert_eq!(b3["confirmed"], true);
        assert_eq!(b3["item"]["state"], "closed");
    }

    #[tokio::test]
    async fn ledger_close_story_closure_then_owner_close_lands_on_commitments_row() {
        let temporary = journal();
        write_json(
            temporary.path(),
            "facets/work/facet.json",
            json!({"id":"00000000-0000-4000-8000-000000000001","name":"work","muted":false}),
        );
        write_jsonl(
            temporary.path(),
            "facets/work/activities/20260401.jsonl",
            &[
                json!({
                    "id": "commit_row",
                    "created_at": 100,
                    "commitments": [{"owner":"Owner","owner_entity_id":"owner","counterparty":"Pat","counterparty_entity_id":"pat","action":"send report"}]
                }),
                json!({
                    "id": "closure_row",
                    "created_at": 200,
                    "closures": [{"owner_entity_id":"owner","counterparty_entity_id":"pat","action":"send the report"}]
                }),
            ],
        );

        let items = ledger_fold::list(temporary.path(), Utc::now(), list_all_query()).unwrap();
        let item_id = &items[0].id;

        let router = crate::routes(temporary.path().to_path_buf());
        let (status, body) = send_request(
            router,
            "POST",
            &format!("/api/ledger/{item_id}/close"),
            Some(json!({"note":"manual confirm","as_state":"closed"})),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["confirmed"], true);

        let day_path = temporary
            .path()
            .join("facets/work/activities/20260401.jsonl");
        let lines = std::fs::read_to_string(day_path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(lines[0]["id"], "commit_row");
        assert_eq!(lines[0]["edits"].as_array().unwrap().len(), 1);
        assert_eq!(
            lines[0]["edits"][0]["fields"],
            serde_json::json!(["ledger_close"])
        );
        assert_eq!(lines[1]["id"], "closure_row");
        assert!(lines[1].get("edits").is_none());
    }

    #[tokio::test]
    async fn ledger_close_future_override_returns_unconfirmed_and_stores_edit() {
        let temporary = journal();
        write_json(
            temporary.path(),
            "facets/work/facet.json",
            json!({"id":"00000000-0000-4000-8000-000000000001","name":"work","muted":false}),
        );
        write_jsonl(
            temporary.path(),
            "facets/work/activities/20260401.jsonl",
            &[json!({
                "id": "commit_1",
                "created_at": 100,
                "commitments": [{"owner":"Owner","owner_entity_id":"owner","counterparty":"Pat","counterparty_entity_id":"pat","action":"send report"}],
                "edits": [{
                    "timestamp": "2099-01-01T00:00:00.000Z",
                    "actor": "owner:ledger_close",
                    "fields": ["ledger_close"],
                    "note": "future closed",
                    "ledger_close": {"item_id":"03b382d6f35ed848","as_state":"closed"}
                }]
            })],
        );

        let router = crate::routes(temporary.path().to_path_buf());
        let (status, body) = send_request(
            router,
            "POST",
            "/api/ledger/03b382d6f35ed848/close",
            Some(json!({"note":"try drop","as_state":"dropped"})),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["confirmed"], false);
        assert_eq!(body["item"]["state"], "closed");

        let day_path = temporary
            .path()
            .join("facets/work/activities/20260401.jsonl");
        let record: Value =
            serde_json::from_str(&std::fs::read_to_string(day_path).unwrap()).unwrap();
        assert_eq!(record["edits"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn ledger_close_muted_facet_precedence_lands_on_visible_row() {
        let temporary = journal();
        write_json(
            temporary.path(),
            "facets/visible/facet.json",
            json!({"id":"00000000-0000-4000-8000-000000000001","name":"visible","muted":false}),
        );
        write_json(
            temporary.path(),
            "facets/muted/facet.json",
            json!({"id":"00000000-0000-4000-8000-000000000002","name":"muted","muted":true}),
        );
        write_jsonl(
            temporary.path(),
            "facets/visible/activities/20260401.jsonl",
            &[json!({
                "id": "vis_1",
                "created_at": 100,
                "commitments": [{"owner":"Owner","action":"send report"}]
            })],
        );
        write_jsonl(
            temporary.path(),
            "facets/muted/activities/20260401.jsonl",
            &[json!({
                "id": "mut_1",
                "created_at": 200,
                "commitments": [{"owner":"Owner","action":"send report"}]
            })],
        );

        let muted_bytes_before = std::fs::read(
            temporary
                .path()
                .join("facets/muted/activities/20260401.jsonl"),
        )
        .unwrap();

        let items = ledger_fold::list(temporary.path(), Utc::now(), list_all_query()).unwrap();
        let item_id = &items[0].id;

        let router = crate::routes(temporary.path().to_path_buf());
        let (status, body) = send_request(
            router,
            "POST",
            &format!("/api/ledger/{item_id}/close"),
            Some(json!({"note":"close visible","as_state":"closed"})),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["confirmed"], true);

        let muted_bytes_after = std::fs::read(
            temporary
                .path()
                .join("facets/muted/activities/20260401.jsonl"),
        )
        .unwrap();
        assert_eq!(muted_bytes_before, muted_bytes_after);

        let vis_content = std::fs::read_to_string(
            temporary
                .path()
                .join("facets/visible/activities/20260401.jsonl"),
        )
        .unwrap();
        let vis_record: Value = serde_json::from_str(&vis_content).unwrap();
        assert_eq!(vis_record["edits"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn ledger_close_sibling_and_different_action_commitments_stay_open() {
        let temporary = journal();
        write_json(
            temporary.path(),
            "facets/work/facet.json",
            json!({"id":"00000000-0000-4000-8000-000000000001","name":"work","muted":false}),
        );
        write_jsonl(
            temporary.path(),
            "facets/work/activities/20260401.jsonl",
            &[
                json!({
                    "id": "row_1",
                    "created_at": 100,
                    "commitments": [
                        {"owner":"Owner","counterparty":"Pat","action":"send report"},
                        {"owner":"Owner","counterparty":"Pat","action":"buy coffee"}
                    ]
                }),
                json!({
                    "id": "row_2",
                    "created_at": 200,
                    "commitments": [{"owner":"Owner","counterparty":"Pat","action":"schedule meeting"}]
                }),
            ],
        );

        let items = ledger_fold::list(temporary.path(), Utc::now(), list_all_query()).unwrap();
        assert_eq!(items.len(), 3);
        let target_item = items
            .iter()
            .find(|item| item.action == "send report")
            .unwrap();
        let target_id = target_item.id.clone();

        let router = crate::routes(temporary.path().to_path_buf());
        let (status, body) = send_request(
            router,
            "POST",
            &format!("/api/ledger/{target_id}/close"),
            Some(json!({"note":"close report","as_state":"closed"})),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["confirmed"], true);

        let items_after =
            ledger_fold::list(temporary.path(), Utc::now(), list_all_query()).unwrap();
        let buy_coffee = items_after
            .iter()
            .find(|item| item.action == "buy coffee")
            .unwrap();
        let schedule = items_after
            .iter()
            .find(|item| item.action == "schedule meeting")
            .unwrap();
        assert_eq!(buy_coffee.state, "open");
        assert_eq!(schedule.state, "open");
    }

    #[tokio::test]
    async fn ledger_close_unknown_id_returns_404_and_preserves_file() {
        let temporary = journal();
        write_json(
            temporary.path(),
            "facets/work/facet.json",
            json!({"id":"00000000-0000-4000-8000-000000000001","name":"work","muted":false}),
        );
        let day_path = temporary
            .path()
            .join("facets/work/activities/20260401.jsonl");
        write_jsonl(
            temporary.path(),
            "facets/work/activities/20260401.jsonl",
            &[json!({
                "id": "c1",
                "created_at": 100,
                "commitments": [{"owner":"Owner","action":"send report"}]
            })],
        );
        let bytes_before = std::fs::read(&day_path).unwrap();

        let router = crate::routes(temporary.path().to_path_buf());
        let (status, body) = send_request(
            router,
            "POST",
            "/api/ledger/unknown12345678/close",
            Some(json!({"note":"note","as_state":"closed"})),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["reason_code"], "ledger_item_unknown");
        assert_eq!(body["error"], "that item was not found.");
        assert_eq!(
            body["detail"],
            "that item was not found. run solstone call profile full <name> for current ids."
        );
        assert_eq!(std::fs::read(&day_path).unwrap(), bytes_before);
    }

    #[tokio::test]
    async fn ledger_close_validation_rejects_blank_note_and_bad_state_and_malformed_body() {
        let temporary = journal();
        write_json(
            temporary.path(),
            "facets/work/facet.json",
            json!({"id":"00000000-0000-4000-8000-000000000001","name":"work","muted":false}),
        );
        write_jsonl(
            temporary.path(),
            "facets/work/activities/20260401.jsonl",
            &[json!({
                "id": "c1",
                "created_at": 100,
                "commitments": [{"owner":"Owner","action":"send report"}]
            })],
        );

        let items = ledger_fold::list(temporary.path(), Utc::now(), list_all_query()).unwrap();
        let item_id = &items[0].id;

        // Missing note key
        let r0 = crate::routes(temporary.path().to_path_buf());
        let (s0, b0) = send_request(
            r0,
            "POST",
            &format!("/api/ledger/{item_id}/close"),
            Some(json!({"as_state":"closed"})),
        )
        .await;
        assert_eq!(s0, StatusCode::BAD_REQUEST);
        assert_eq!(b0["reason_code"], "invalid_request_value");
        assert_eq!(b0["detail"], "a note is required.");

        // Blank note
        let r1 = crate::routes(temporary.path().to_path_buf());
        let (s1, b1) = send_request(
            r1,
            "POST",
            &format!("/api/ledger/{item_id}/close"),
            Some(json!({"note":"","as_state":"closed"})),
        )
        .await;
        assert_eq!(s1, StatusCode::BAD_REQUEST);
        assert_eq!(b1["reason_code"], "invalid_request_value");
        assert_eq!(b1["detail"], "a note is required.");

        // Whitespace-only note
        let r2 = crate::routes(temporary.path().to_path_buf());
        let (s2, b2) = send_request(
            r2,
            "POST",
            &format!("/api/ledger/{item_id}/close"),
            Some(json!({"note":"   \t\n  ","as_state":"closed"})),
        )
        .await;
        assert_eq!(s2, StatusCode::BAD_REQUEST);
        assert_eq!(b2["reason_code"], "invalid_request_value");
        assert_eq!(b2["detail"], "a note is required.");

        // Capitalized as_state
        let r3 = crate::routes(temporary.path().to_path_buf());
        let (s3, b3) = send_request(
            r3,
            "POST",
            &format!("/api/ledger/{item_id}/close"),
            Some(json!({"note":"valid note","as_state":"Closed"})),
        )
        .await;
        assert_eq!(s3, StatusCode::BAD_REQUEST);
        assert_eq!(b3["reason_code"], "invalid_request_value");
        assert_eq!(b3["detail"], "as_state must be closed or dropped.");

        // Invalid JSON body
        let router_raw = crate::routes(temporary.path().to_path_buf());
        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/ledger/{item_id}/close"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("not valid json"))
            .unwrap();
        let resp = router_raw.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let b4: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(b4["reason_code"], "invalid_request_value");
        assert_eq!(b4["detail"], "the body could not be read.");
    }

    #[tokio::test]
    async fn ledger_close_store_error_mapper_maps_timeout_missing_and_unexpected() {
        // Lock timeout
        let resp_lock = store_error(solstone_core_facets::ActivityRecordStoreError::Lock(
            LockError::Timeout(LockTimeout {
                path: "activities.jsonl".into(),
                timeout: Duration::from_millis(1),
            }),
        ));
        let (parts, body) = resp_lock.into_parts();
        let body: Value =
            serde_json::from_slice(&to_bytes(body, usize::MAX).await.unwrap()).unwrap();
        assert_eq!(parts.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["reason_code"], "ledger_busy");
        assert_eq!(
            body["error"],
            "the close could not be saved right now because it was busy. try again in a moment."
        );
        assert_eq!(
            body["detail"],
            "the close could not be saved right now because it was busy. try again in a moment."
        );

        // Missing record
        let resp_missing = store_error(
            solstone_core_facets::ActivityRecordStoreError::MissingRecord {
                facet: "work".into(),
                day: "20260401".into(),
                record_id: "rec".into(),
            },
        );
        let (parts_m, body_m) = resp_missing.into_parts();
        let body_m: Value =
            serde_json::from_slice(&to_bytes(body_m, usize::MAX).await.unwrap()).unwrap();
        assert_eq!(parts_m.status, StatusCode::NOT_FOUND);
        assert_eq!(body_m["reason_code"], "ledger_target_missing");
        assert_eq!(body_m["error"], "that item changed.");
        assert_eq!(
            body_m["detail"],
            "that item changed. re-read the profile and try again."
        );

        // Other store error (DestinationMuted)
        let resp_other = store_error(
            solstone_core_facets::ActivityRecordStoreError::DestinationMuted {
                facet: "muted".into(),
            },
        );
        let (parts_o, _body_o) = resp_other.into_parts();
        assert!(!parts_o.status.is_success());
        assert_eq!(parts_o.status, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn ledger_close_get_item_returns_open_and_dropped_without_mutating_file() {
        let temporary = journal();
        write_json(
            temporary.path(),
            "facets/work/facet.json",
            json!({"id":"00000000-0000-4000-8000-000000000001","name":"work","muted":false}),
        );
        let day_path = temporary
            .path()
            .join("facets/work/activities/20260401.jsonl");
        write_jsonl(
            temporary.path(),
            "facets/work/activities/20260401.jsonl",
            &[
                json!({
                    "id": "open_row",
                    "created_at": 100,
                    "commitments": [{"owner":"Owner","action":"open task"}]
                }),
                json!({
                    "id": "dropped_row",
                    "created_at": 200,
                    "commitments": [{"owner":"Owner","action":"dropped task"}],
                }),
            ],
        );

        let items = ledger_fold::list(temporary.path(), Utc::now(), list_all_query()).unwrap();
        let open_id = items
            .iter()
            .find(|i| i.action == "open task")
            .unwrap()
            .id
            .clone();
        let dropped_id = items
            .iter()
            .find(|i| i.action == "dropped task")
            .unwrap()
            .id
            .clone();

        append_ledger_close(
            temporary.path(),
            "work",
            "20260401",
            "dropped_row",
            &dropped_id,
            LedgerCloseState::Dropped,
            "dropped",
            Utc::now(),
        )
        .expect("append drop");

        let bytes_before = std::fs::read(&day_path).unwrap();

        let router = crate::routes(temporary.path().to_path_buf());
        let (s_open, b_open) =
            send_request(router, "GET", &format!("/api/ledger/{open_id}"), None).await;
        assert_eq!(s_open, StatusCode::OK);
        assert_eq!(b_open["id"], open_id);
        assert_eq!(b_open["state"], "open");

        let router2 = crate::routes(temporary.path().to_path_buf());
        let (s_drop, b_drop) =
            send_request(router2, "GET", &format!("/api/ledger/{dropped_id}"), None).await;
        assert_eq!(s_drop, StatusCode::OK);
        assert_eq!(b_drop["id"], dropped_id);
        assert_eq!(b_drop["state"], "dropped");

        assert_eq!(std::fs::read(&day_path).unwrap(), bytes_before);
    }
}
