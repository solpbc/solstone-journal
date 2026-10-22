// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native activities API routes.

use std::path::{Path, PathBuf};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path as RoutePath, RawQuery, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::Utc;
use serde_json::{Map, Value, json};
use solstone_core_entity::{
    EntityResolutionEntity, EntityResolutionOutcome, record_entity_resolution,
};
use solstone_core_facets::{
    ActivityRecord, AppendOutcome, activity_value_or_empty, activity_value_string,
    read_detected_entities,
};
use solstone_core_format::content::{Family, produce_chunks_by_shape};

use crate::{Clock, http};

mod view;

pub(crate) const ACTOR_HEADER: &str = "x-solstone-actor";
pub(crate) const ACTOR_OWNER: &str = "owner";

pub fn routes(root: PathBuf, clock: Clock) -> Router {
    Router::new()
        .route("/app/activities/workspace", get(activities_workspace_route))
        .route("/app/activities/", get(activities_today_redirect))
        .route("/app/activities/{day}", get(activities_day_shell))
        .route("/app/activities/api/day/{day}/view", get(day_view))
        .route("/app/activities/api/index", get(activities_index))
        .route("/app/activities/api/stats/{month}", get(activities_stats))
        .route(
            "/app/activities/api/day/{day}/records",
            get(list).post(create),
        )
        .route(
            "/app/activities/api/day/{day}/record/{span_id}",
            get(record),
        )
        .route(
            "/app/activities/api/day/{day}/record/{span_id}/update",
            post(update),
        )
        .route(
            "/app/activities/api/day/{day}/record/{span_id}/mute",
            post(mute),
        )
        .route(
            "/app/activities/api/day/{day}/record/{span_id}/unmute",
            post(unmute),
        )
        .with_state((root, clock))
}

async fn activities_workspace_route() -> Response {
    crate::assets::activities_workspace()
}

fn today(clock: &Clock) -> String {
    clock.now().date_naive().format("%Y%m%d").to_string()
}

async fn activities_today_redirect(
    State((_, clock)): State<(PathBuf, Clock)>,
    RawQuery(query): RawQuery,
) -> Response {
    let day = today(&clock);
    let location = match query {
        Some(q) if !q.is_empty() => format!("/app/activities/{day}?{q}"),
        _ => format!("/app/activities/{day}"),
    };
    Response::builder()
        .status(StatusCode::FOUND)
        .header(axum::http::header::LOCATION, location)
        .body(axum::body::Body::empty())
        .expect("redirect response builds")
        .into_response()
}

async fn activities_day_shell(RoutePath(day): RoutePath<String>) -> Response {
    if crate::segments::is_day(&day) {
        crate::assets::shell()
    } else {
        invalid_day_not_found()
    }
}

fn invalid_day_not_found() -> Response {
    http::error(
        "invalid_day",
        "that day couldn't be used.",
        "Day not found".to_owned(),
        StatusCode::NOT_FOUND,
    )
}

async fn list(
    State((root, _)): State<(PathBuf, Clock)>,
    RoutePath(day): RoutePath<String>,
    RawQuery(query): RawQuery,
) -> Response {
    let facet = query_value(query.as_deref(), "facet").filter(|facet| !facet.is_empty());
    let include_hidden = query_value(query.as_deref(), "include_hidden").as_deref() == Some("1");
    let facets = match facet {
        Some(facet) => vec![facet],
        None => match solstone_core_facets::list_declared_facet_names(&root) {
            Ok(names) => names,
            Err(error) => return internal(error.to_string()),
        },
    };
    let mut items = Vec::new();
    for facet in facets {
        match solstone_core_facets::load_activity_records(&root, &facet, &day, include_hidden) {
            Ok(records) => items.extend(records.into_iter().map(|mut record| {
                record.insert("facet".to_owned(), Value::String(facet.clone()));
                record.insert("day".to_owned(), Value::String(day.clone()));
                payload(record)
            })),
            Err(error) => return internal(error.to_string()),
        }
    }
    Json(json!({"items":items})).into_response()
}

async fn record(
    State((root, _)): State<(PathBuf, Clock)>,
    RoutePath((day, span_id)): RoutePath<(String, String)>,
    RawQuery(query): RawQuery,
) -> Response {
    let facet = query_value(query.as_deref(), "facet").unwrap_or_default();
    if facet.is_empty() {
        return not_found(span_id);
    }
    match solstone_core_facets::get_activity_record(&root, &facet, &day, &span_id) {
        Ok(Some(value)) => Json(payload(value)).into_response(),
        Ok(None) | Err(solstone_core_facets::ActivityRecordStoreError::MissingDayFile { .. }) => {
            not_found(span_id)
        }
        Err(error) => internal(error.to_string()),
    }
}

async fn create(
    State((root, clock)): State<(PathBuf, Clock)>,
    RoutePath(day): RoutePath<String>,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Response {
    let body = object_body(&body);
    let title = string(&body, "title").trim().to_owned();
    let source = body
        .get("source")
        .map(|value| activity_value_or_empty(Some(value)))
        .filter(|source| !source.is_empty())
        .unwrap_or_else(|| "user".to_owned());
    if title.is_empty() {
        return invalid("title must not be empty");
    }
    if source != "user" {
        return invalid("source must be 'user'");
    }
    let activity = string(&body, "activity").trim().to_owned();
    let facet = query_value(query.as_deref(), "facet").unwrap_or_default();
    match solstone_core_facets::activity_is_available(&root, &facet, &activity) {
        Ok(true) => {}
        Ok(false) => return not_found(activity),
        Err(error) => return internal(error.to_string()),
    }
    if facet.is_empty() {
        return invalid("facet must not be empty");
    }
    let (anchor, segments) = match body.get("since_segment") {
        Some(value) if !value.is_null() => (
            activity_value_string(value),
            vec![Value::String(activity_value_string(value))],
        ),
        _ => (
            format!("user_{}", clock.now().timestamp_millis()),
            Vec::new(),
        ),
    };
    let span_id = solstone_core_system::activity_state::make_activity_id(&activity, &anchor);
    let description = {
        let description = string(&body, "description").trim().to_owned();
        if description.is_empty() {
            title.clone()
        } else {
            description
        }
    };
    let participation_provided = body.contains_key("participation");
    let participation = if participation_provided {
        resolve_participation(&root, &facet, &day, &span_id, body.get("participation"))
    } else {
        Ok(Vec::new())
    };
    let participation = match participation {
        Ok(value) => value,
        Err(error) => return internal(error),
    };
    let mut record = Map::new();
    record.insert("id".to_owned(), json!(span_id));
    record.insert("activity".to_owned(), json!(activity));
    record.insert("title".to_owned(), json!(title));
    record.insert("description".to_owned(), json!(description));
    record.insert("details".to_owned(), json!(string(&body, "details")));
    record.insert("segments".to_owned(), Value::Array(segments));
    record.insert("active_entities".to_owned(), json!([]));
    record.insert(
        "created_at".to_owned(),
        json!(clock.now().timestamp_millis()),
    );
    record.insert("source".to_owned(), json!(source));
    record.insert("hidden".to_owned(), json!(false));
    let mut fields = vec!["activity", "title", "description", "details", "source"];
    if participation_provided {
        record.insert("participation".to_owned(), Value::Array(participation));
        fields.push("participation");
    }
    record.insert("edits".to_owned(), json!([{"timestamp":clock.now().with_timezone(&Utc).format("%Y-%m-%dT%H:%M:%SZ").to_string(),"actor":"cli:create","fields":fields,"note":"created"}]));
    match solstone_core_facets::append_activity_record(&root, &facet, &day, record) {
        Ok(AppendOutcome::Written(record)) => {
            if let Err(error) = solstone_core_facets::append_action_log_for_day(
                &root,
                Some(&facet),
                "call",
                "agent",
                "activity_create",
                json!({"id":span_id,"activity":activity,"source":source}),
                &day,
            ) {
                return internal(error.to_string());
            }
            Json(payload(record)).into_response()
        }
        Ok(AppendOutcome::AlreadyExists) => already_exists(span_id),
        Err(error) => store_error(error),
    }
}

async fn day_view(
    State((root, _)): State<(PathBuf, Clock)>,
    RoutePath(day): RoutePath<String>,
    RawQuery(query): RawQuery,
) -> Response {
    if !crate::segments::is_day(&day) {
        return invalid_day_not_found();
    }
    solstone_core_convey_http::owner_read::spawn_blocking_response(
        solstone_core_convey_http::owner_read::OwnerReadRole::ActivitiesDayView,
        move || match view::build_view(&root, &day, query.as_deref()) {
            Ok(val) => Json(val).into_response(),
            Err(err) => internal(err),
        },
    )
    .await
}

async fn activities_index(
    State((root, _)): State<(PathBuf, Clock)>,
    RawQuery(query): RawQuery,
) -> Response {
    solstone_core_convey_http::owner_read::spawn_blocking_response(
        solstone_core_convey_http::owner_read::OwnerReadRole::ActivitiesIndex,
        move || {
            let counts = view::compute_day_counts(&root, query.as_deref());
            Json(crate::date_nav::date_nav_index(&counts)).into_response()
        },
    )
    .await
}

async fn activities_stats(
    State((root, _)): State<(PathBuf, Clock)>,
    RoutePath(month): RoutePath<String>,
    RawQuery(query): RawQuery,
) -> Response {
    if !crate::segments::is_month(&month) {
        return http::error(
            "invalid_month",
            "that month couldn't be used.",
            "Invalid month format, expected YYYYMM".to_owned(),
            StatusCode::BAD_REQUEST,
        );
    }
    solstone_core_convey_http::owner_read::spawn_blocking_response(
        solstone_core_convey_http::owner_read::OwnerReadRole::ActivitiesMonthStats,
        move || {
            let counts = view::compute_day_counts(&root, query.as_deref());
            let filtered: std::collections::BTreeMap<String, usize> = counts
                .into_iter()
                .filter(|(day, _)| day.starts_with(&month))
                .collect();
            Json(filtered).into_response()
        },
    )
    .await
}

async fn update(
    State((root, clock)): State<(PathBuf, Clock)>,
    RoutePath((day, span_id)): RoutePath<(String, String)>,
    RawQuery(query): RawQuery,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> Response {
    let is_owner = headers.get(ACTOR_HEADER).and_then(|v| v.to_str().ok()) == Some(ACTOR_OWNER);
    let (actor, log_source, log_actor) = if is_owner {
        ("owner:update", "app", "activities")
    } else {
        ("cli:update", "call", "agent")
    };
    mutate_update(
        &root, &clock, day, span_id, query, body, actor, log_source, log_actor,
    )
    .await
}

async fn mute(
    State((root, clock)): State<(PathBuf, Clock)>,
    RoutePath((day, span_id)): RoutePath<(String, String)>,
    RawQuery(query): RawQuery,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> Response {
    let is_owner = headers.get(ACTOR_HEADER).and_then(|v| v.to_str().ok()) == Some(ACTOR_OWNER);
    let (actor, log_source, log_actor) = if is_owner {
        ("owner:mute", "app", "activities")
    } else {
        ("cli:mute", "call", "agent")
    };
    set_hidden(
        &root, &clock, day, span_id, query, body, true, actor, log_source, log_actor,
    )
}

async fn unmute(
    State((root, clock)): State<(PathBuf, Clock)>,
    RoutePath((day, span_id)): RoutePath<(String, String)>,
    RawQuery(query): RawQuery,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> Response {
    let is_owner = headers.get(ACTOR_HEADER).and_then(|v| v.to_str().ok()) == Some(ACTOR_OWNER);
    let (actor, log_source, log_actor) = if is_owner {
        ("owner:unmute", "app", "activities")
    } else {
        ("cli:unmute", "call", "agent")
    };
    set_hidden(
        &root, &clock, day, span_id, query, body, false, actor, log_source, log_actor,
    )
}

#[allow(clippy::too_many_arguments)]
async fn mutate_update(
    root: &Path,
    clock: &Clock,
    day: String,
    span_id: String,
    query: Option<String>,
    body: Bytes,
    actor: &str,
    log_source: &str,
    log_actor: &str,
) -> Response {
    let facet = query_value(query.as_deref(), "facet").unwrap_or_default();
    if facet.is_empty() {
        return not_found(span_id);
    }
    let body = object_body(&body);
    let Some(patch) = body.get("patch").and_then(Value::as_object) else {
        return invalid("patch must be a non-empty object");
    };
    if patch.is_empty() {
        return invalid("patch must not be empty");
    }
    let disallowed = patch
        .keys()
        .filter(|key| !matches!(key.as_str(), "title" | "description" | "details"))
        .cloned()
        .collect::<Vec<_>>();
    if !disallowed.is_empty() {
        return invalid(&format!(
            "patch contains disallowed fields: {}",
            disallowed.join(", ")
        ));
    }
    let note = activity_value_or_empty(body.get("note"));
    let timestamp = clock
        .now()
        .with_timezone(&Utc)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    match solstone_core_facets::update_activity_record(
        root, &facet, &day, &span_id, patch, actor, &note, &timestamp,
    ) {
        Ok(Some(record)) => {
            if let Err(error) = solstone_core_facets::append_action_log_for_day(
                root,
                Some(&facet),
                log_source,
                log_actor,
                "activity_update",
                json!({"id":span_id,"fields":patch.keys().collect::<Vec<_>>() }),
                &day,
            ) {
                return internal(error.to_string());
            }
            Json(payload(record)).into_response()
        }
        Ok(None) | Err(solstone_core_facets::ActivityRecordStoreError::MissingDayFile { .. }) => {
            not_found(span_id)
        }
        Err(error) => store_error(error),
    }
}

#[allow(clippy::too_many_arguments)]
fn set_hidden(
    root: &Path,
    clock: &Clock,
    day: String,
    span_id: String,
    query: Option<String>,
    body: Bytes,
    hidden: bool,
    actor: &str,
    log_source: &str,
    log_actor: &str,
) -> Response {
    let facet = query_value(query.as_deref(), "facet").unwrap_or_default();
    if facet.is_empty() {
        return not_found(span_id);
    }
    let body = object_body(&body);
    let reason = body.get("reason").and_then(Value::as_str);
    let timestamp = clock
        .now()
        .with_timezone(&Utc)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    match solstone_core_facets::set_activity_hidden(
        root, &facet, &day, &span_id, hidden, actor, reason, &timestamp,
    ) {
        Ok(Some(record)) => {
            let action = if hidden {
                "activity_mute"
            } else {
                "activity_unmute"
            };
            if let Err(error) = solstone_core_facets::append_action_log_for_day(
                root,
                Some(&facet),
                log_source,
                log_actor,
                action,
                json!({"id":span_id,"reason":reason}),
                &day,
            ) {
                return internal(error.to_string());
            }
            Json(payload(record)).into_response()
        }
        Ok(None) | Err(solstone_core_facets::ActivityRecordStoreError::MissingDayFile { .. }) => {
            not_found(span_id)
        }
        Err(error) => store_error(error),
    }
}

fn resolve_participation(
    root: &Path,
    facet: &str,
    day: &str,
    record_id: &str,
    raw: Option<&Value>,
) -> Result<Vec<Value>, String> {
    // Python routes.py:_resolve_participation_entity_ids calls
    // load_entities(facet=facet, day=day), i.e. this facet day's detected rows.
    let entities = read_detected_entities(root, facet, day)
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter_map(|entity| entity.as_object().cloned())
        .map(|entity| EntityResolutionEntity {
            id: entity.get("id").and_then(Value::as_str).map(str::to_owned),
            name: entity
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            aka: string_values(&entity, "aka"),
            emails: string_values(&entity, "emails"),
            blocked: entity
                .get("blocked")
                .is_some_and(solstone_core_facets::activity_value_truthy),
        })
        .collect::<Vec<_>>();
    raw.and_then(Value::as_array).cloned().unwrap_or_default().into_iter().map(|entry| {
        let mut entry = entry.as_object().cloned().unwrap_or_default();
        let query = string(&entry, "name");
        let resolution = record_entity_resolution(root, &query, &entities, json!({"kind":"facet","facet":facet}), json!({"lane":"apps.activities.create","facet":facet,"day":day,"record_id":record_id,"field":"participation.name"}), 90.0, false).map_err(|error| error.to_string())?;
        let entity_id = if resolution.outcome == EntityResolutionOutcome::Resolved { resolution.entity_index.and_then(|index| entities.get(index)).and_then(|entity| entity.id.clone()).map(Value::String).unwrap_or(Value::Null) } else { Value::Null };
        entry.insert("entity_id".to_owned(), entity_id);
        Ok(Value::Object(entry))
    }).collect::<Result<Vec<_>, String>>()
}

fn payload(record: ActivityRecord) -> Value {
    let markdown = produce_chunks_by_shape(Family::Activity, None, std::slice::from_ref(&record))
        .chunks
        .into_iter()
        .next()
        .map(|chunk| chunk.content)
        .unwrap_or_default();
    json!({"record":record,"markdown":markdown})
}
fn query_value(query: Option<&str>, wanted: &str) -> Option<String> {
    query.unwrap_or_default().split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        (key == wanted).then(|| value.to_owned())
    })
}
fn object_body(bytes: &[u8]) -> Map<String, Value> {
    serde_json::from_slice::<Value>(bytes)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}
fn string(record: &Map<String, Value>, key: &str) -> String {
    activity_value_or_empty(record.get(key))
}
fn string_values(record: &Map<String, Value>, key: &str) -> Vec<String> {
    record
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}
fn invalid(detail: &str) -> Response {
    http::error(
        "activity_invalid",
        "that activity setting couldn't be used.",
        detail.to_owned(),
        StatusCode::BAD_REQUEST,
    )
}
fn not_found(detail: String) -> Response {
    http::error(
        "activity_not_found",
        "that activity isn't in the facet.",
        detail,
        StatusCode::NOT_FOUND,
    )
}
fn already_exists(detail: String) -> Response {
    http::error(
        "activity_already_exists",
        "that activity couldn't be created because it already exists.",
        detail,
        StatusCode::CONFLICT,
    )
}
fn internal(detail: String) -> Response {
    http::error(
        "internal_error",
        "that request didn't finish.",
        detail,
        StatusCode::INTERNAL_SERVER_ERROR,
    )
}
fn store_error(error: solstone_core_facets::ActivityRecordStoreError) -> Response {
    if matches!(
        error,
        solstone_core_facets::ActivityRecordStoreError::Lock(
            solstone_core_journal_io::LockError::Timeout(_)
        )
    ) {
        http::error(
            "activities_busy",
            "activities couldn't be updated right now because they were busy. try again in a moment.",
            "activities couldn't be updated right now because they were busy. try again in a moment."
                .to_owned(),
            StatusCode::SERVICE_UNAVAILABLE,
        )
    } else {
        internal(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::{path::Path, time::Duration};

    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode, header},
    };
    use serde_json::{Value, json};
    use solstone_core_journal_io::{LockError, LockTimeout};
    use tower::ServiceExt;

    use super::*;
    use crate::test_support::{fixed_clock, later_clock, phase_root, write};

    fn gated(root: &Path, clock: Clock) -> Router {
        solstone_core_convey_shell::session_gate::apply_layer(
            routes(root.to_path_buf(), clock),
            root.to_path_buf(),
        )
    }

    async fn request(
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

    fn create_body(since_segment: Option<&str>) -> Value {
        let mut body = json!({"title":"Planning","activity":"meeting"});
        if let Some(since_segment) = since_segment {
            body["since_segment"] = Value::String(since_segment.to_owned());
        }
        body
    }

    fn day_path(root: &Path) -> std::path::PathBuf {
        root.join("facets/work/activities/20260510.jsonl")
    }

    #[tokio::test]
    async fn ac2a_create_guard_same_since_segment_refuses_second_create() {
        let root = phase_root("established_empty");
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let uri = "/app/activities/api/day/20260510/records?facet=work";
        let (first, _) = request(
            gated(root.path(), fixed_clock()),
            "POST",
            uri,
            Some(create_body(Some("111000_300"))),
        )
        .await;
        let (second, body) = request(
            gated(root.path(), fixed_clock()),
            "POST",
            uri,
            Some(create_body(Some("111000_300"))),
        )
        .await;
        assert_eq!(first, StatusCode::OK);
        assert_eq!(second, StatusCode::CONFLICT);
        assert_eq!(body["reason_code"], "activity_already_exists");
        assert_eq!(
            std::fs::read_to_string(day_path(root.path()))
                .expect("rows")
                .lines()
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn ac2b_create_guard_fixed_clock_refuses_second_create() {
        let root = phase_root("established_empty");
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let uri = "/app/activities/api/day/20260510/records?facet=work";
        let (first, _) = request(
            gated(root.path(), fixed_clock()),
            "POST",
            uri,
            Some(create_body(None)),
        )
        .await;
        let (second, body) = request(
            gated(root.path(), fixed_clock()),
            "POST",
            uri,
            Some(create_body(None)),
        )
        .await;
        assert_eq!(first, StatusCode::OK);
        assert_eq!(second, StatusCode::CONFLICT);
        assert_eq!(body["reason_code"], "activity_already_exists");
        assert_eq!(
            std::fs::read_to_string(day_path(root.path()))
                .expect("rows")
                .lines()
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn ac2c_create_guard_distinct_clock_readings_create_two_records() {
        let root = phase_root("established_empty");
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let uri = "/app/activities/api/day/20260510/records?facet=work";
        let (first, _) = request(
            gated(root.path(), fixed_clock()),
            "POST",
            uri,
            Some(create_body(None)),
        )
        .await;
        let (second, _) = request(
            gated(root.path(), later_clock()),
            "POST",
            uri,
            Some(create_body(None)),
        )
        .await;
        assert_eq!(first, StatusCode::OK);
        assert_eq!(second, StatusCode::OK);
        assert_eq!(
            std::fs::read_to_string(day_path(root.path()))
                .expect("rows")
                .lines()
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn created_and_edited_times_are_true_instants_west_of_utc() {
        let root = phase_root("established_empty");
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        // 18:00 at UTC-6 is midnight UTC the next day.
        let evening = Clock::new(|| {
            chrono::DateTime::parse_from_rfc3339("2026-05-10T18:00:00-06:00").expect("clock")
        });
        let uri = "/app/activities/api/day/20260510/records?facet=work";
        let (status, created) = request(
            gated(root.path(), evening.clone()),
            "POST",
            uri,
            Some(create_body(None)),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let instant = chrono::DateTime::parse_from_rfc3339("2026-05-11T00:00:00Z").unwrap();
        let created = &created["record"];
        assert_eq!(created["created_at"], instant.timestamp_millis());
        assert_eq!(created["edits"][0]["timestamp"], "2026-05-11T00:00:00Z");

        let id = created["id"].as_str().expect("id");
        let (status, updated) = request(
            gated(root.path(), evening),
            "POST",
            &format!("/app/activities/api/day/20260510/record/{id}/update?facet=work"),
            Some(json!({"patch":{"title":"Planning review"}})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{updated}");
        let edits = updated["record"]["edits"].as_array().expect("edits");
        assert_eq!(edits.last().unwrap()["timestamp"], "2026-05-11T00:00:00Z");
    }

    #[tokio::test]
    async fn ac3_list_hides_muted_unless_requested() {
        let root = phase_root("populated");
        let uri = "/app/activities/api/day/20260510/records?facet=work";
        let (_, visible) = request(gated(root.path(), fixed_clock()), "GET", uri, None).await;
        let (_, all) = request(
            gated(root.path(), fixed_clock()),
            "GET",
            &format!("{uri}&include_hidden=1"),
            None,
        )
        .await;
        assert_eq!(visible["items"].as_array().expect("items").len(), 1);
        assert_eq!(all["items"].as_array().expect("items").len(), 2);
        assert_eq!(all["items"][1]["record"]["hidden"], true);
    }

    #[tokio::test]
    async fn ac4_list_without_facet_orders_two_declared_facets() {
        let root = phase_root("populated");
        write(
            &root
                .path()
                .join("facets/personal/activities/20260510.jsonl"),
            "{\"id\":\"personal_1\",\"activity\":\"meeting\",\"title\":\"Personal\",\"description\":\"Personal\",\"hidden\":false}\n",
        );
        let (_, all) = request(
            gated(root.path(), fixed_clock()),
            "GET",
            "/app/activities/api/day/20260510/records?include_hidden=1",
            None,
        )
        .await;
        let ids = all["items"]
            .as_array()
            .expect("items")
            .iter()
            .map(|item| item["record"]["id"].as_str().expect("id"))
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            ["personal_1", "meeting_100000_300", "focus_103000_300"]
        );
        let (_, scoped) = request(
            gated(root.path(), fixed_clock()),
            "GET",
            "/app/activities/api/day/20260510/records?facet=work&include_hidden=1",
            None,
        )
        .await;
        assert_eq!(scoped["items"].as_array().expect("items").len(), 2);
    }

    #[tokio::test]
    async fn ac6_update_adds_one_named_edit_with_note() {
        let root = phase_root("populated");
        let uri = "/app/activities/api/day/20260510/record/meeting_100000_300/update?facet=work";
        let (status, body) = request(
            gated(root.path(), fixed_clock()),
            "POST",
            uri,
            Some(json!({"patch":{"title":"Retitled","details":"More"},"note":"corrected"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let edits = body["record"]["edits"].as_array().expect("edits");
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[1]["fields"], json!(["title", "details"]));
        assert_eq!(edits[1]["note"], "corrected");
        assert_eq!(body["record"]["title"], "Retitled");
    }

    #[tokio::test]
    async fn ac7_mute_then_unmute_each_append_an_edit() {
        let root = phase_root("populated");
        let mute_uri = "/app/activities/api/day/20260510/record/meeting_100000_300/mute?facet=work";
        let (muted, muted_body) = request(
            gated(root.path(), fixed_clock()),
            "POST",
            mute_uri,
            Some(json!({"reason":"quiet"})),
        )
        .await;
        let (unmuted, unmuted_body) = request(
            gated(root.path(), fixed_clock()),
            "POST",
            "/app/activities/api/day/20260510/record/meeting_100000_300/unmute?facet=work",
            Some(json!({"reason":"back"})),
        )
        .await;
        assert_eq!(muted, StatusCode::OK);
        assert_eq!(muted_body["record"]["hidden"], true);
        assert_eq!(
            muted_body["record"]["edits"]
                .as_array()
                .expect("edits")
                .len(),
            2
        );
        assert_eq!(unmuted, StatusCode::OK);
        assert_eq!(unmuted_body["record"]["hidden"], false);
        assert_eq!(
            unmuted_body["record"]["edits"]
                .as_array()
                .expect("edits")
                .len(),
            3
        );
    }

    #[tokio::test]
    async fn ac8_repeat_hidden_state_normalizes_once_then_suppresses_each_direction() {
        let root = phase_root("populated");
        write(
            &day_path(root.path()),
            "{\"id\":\"loose_mute\",\"activity\":\"meeting\",\"description\":\"Loose mute\",\"title\":false,\"details\":false,\"hidden\":false,\"edits\":{}}\n{\"id\":\"loose_unmute\",\"activity\":\"meeting\",\"description\":\"Loose unmute\",\"title\":false,\"details\":false,\"hidden\":true,\"edits\":{}}\n",
        );
        for (id, action, expected) in [
            ("loose_mute", "mute", true),
            ("loose_unmute", "unmute", false),
        ] {
            let uri = format!("/app/activities/api/day/20260510/record/{id}/{action}?facet=work");
            let (first, body) = request(
                gated(root.path(), fixed_clock()),
                "POST",
                &uri,
                Some(json!({})),
            )
            .await;
            assert_eq!(first, StatusCode::OK);
            assert_eq!(body["record"]["hidden"], expected);
            assert_eq!(
                body["record"]["title"],
                if expected {
                    "Loose mute"
                } else {
                    "Loose unmute"
                }
            );
            assert_eq!(body["record"]["details"], "");
            assert_eq!(body["record"]["edits"].as_array().expect("edits").len(), 1);
            let persisted =
                std::fs::read_to_string(day_path(root.path())).expect("normalized rows");
            let (second, repeated) = request(
                gated(root.path(), fixed_clock()),
                "POST",
                &uri,
                Some(json!({})),
            )
            .await;
            // An always-append port grows edits here; an early-return port cannot persist first-pass normalization.
            assert_eq!(second, StatusCode::OK);
            assert_eq!(
                repeated["record"]["edits"].as_array().expect("edits").len(),
                1
            );
            assert_eq!(
                std::fs::read_to_string(day_path(root.path())).expect("suppressed rows"),
                persisted
            );
        }
    }

    #[tokio::test]
    async fn ac9_missing_record_is_not_found_for_each_record_route() {
        let root = phase_root("populated");
        for (suffix, body) in [
            ("", None),
            ("/update", Some(json!({"patch":{"title":"new"}}))),
            ("/mute", Some(json!({}))),
            ("/unmute", Some(json!({}))),
        ] {
            let method = if suffix.is_empty() { "GET" } else { "POST" };
            let uri = format!("/app/activities/api/day/20260510/record/missing{suffix}?facet=work");
            let (status, response) =
                request(gated(root.path(), fixed_clock()), method, &uri, body).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{suffix}");
            assert_eq!(response["reason_code"], "activity_not_found");
            assert_eq!(response["detail"], "missing");
        }
    }

    #[tokio::test]
    async fn ac10_create_refusal_order_matches_python() {
        let root = phase_root("populated");
        let uri = "/app/activities/api/day/20260510/records?facet=work";
        for (body, code, detail) in [
            (
                json!({"activity":"missing"}),
                "activity_invalid",
                "title must not be empty",
            ),
            (
                json!({"title":"x","source":"bad","activity":"meeting"}),
                "activity_invalid",
                "source must be 'user'",
            ),
            (
                json!({"title":"x","activity":"missing"}),
                "activity_not_found",
                "missing",
            ),
            (
                json!({"title":"x","source":"bad","activity":"missing"}),
                "activity_invalid",
                "source must be 'user'",
            ),
        ] {
            let (_, response) =
                request(gated(root.path(), fixed_clock()), "POST", uri, Some(body)).await;
            assert_eq!(response["reason_code"], code);
            assert_eq!(response["detail"], detail);
        }
    }

    #[tokio::test]
    async fn ac14_gated_preinit_and_corrupt_config_do_not_create_activity_days() {
        let uri = "/app/activities/api/day/20260510/records?facet=work";
        let unestablished = phase_root("unestablished");
        let response = gated(unestablished.path(), fixed_clock())
            .oneshot(
                Request::post(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&create_body(None)).expect("body"),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(response.headers()[header::LOCATION], "/init");
        assert!(!day_path(unestablished.path()).exists());

        let corrupt = phase_root("corrupt");
        let (status, body) = request(
            gated(corrupt.path(), fixed_clock()),
            "POST",
            uri,
            Some(create_body(None)),
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["reason_code"], "corrupt_config");
        assert!(!day_path(corrupt.path()).exists());
    }

    #[tokio::test]
    async fn participation_uses_only_the_facet_day_detected_entities() {
        let root = phase_root("populated");
        write(
            &root.path().join("entities/outside/entity.json"),
            "{\"id\":\"outside\",\"name\":\"Outside Person\",\"type\":\"person\"}\n",
        );
        write(
            &root.path().join("facets/personal/entities/20260510.jsonl"),
            "{\"id\":\"outside\",\"name\":\"Outside Person\",\"type\":\"person\"}\n",
        );
        let (status, body) = request(
            gated(root.path(), fixed_clock()),
            "POST",
            "/app/activities/api/day/20260510/records?facet=work",
            Some(json!({"title":"Planning","activity":"meeting","participation":[{"name":"Outside Person"}]})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["record"]["participation"][0]["entity_id"], Value::Null);
    }

    #[tokio::test]
    async fn python_value_coercion_matches_create_and_update() {
        let root = phase_root("populated");
        let uri = "/app/activities/api/day/20260510/records?facet=work";
        let (_, refused) = request(
            gated(root.path(), fixed_clock()),
            "POST",
            uri,
            Some(json!({"title":"x","activity":"meeting","source":true})),
        )
        .await;
        assert_eq!(refused["detail"], "source must be 'user'");
        let (_, created) = request(
            gated(root.path(), fixed_clock()),
            "POST",
            uri,
            Some(json!({"title":"Fallback","activity":"meeting","description":false})),
        )
        .await;
        assert_eq!(created["record"]["description"], "Fallback");
        let (_, updated) = request(
            gated(root.path(), fixed_clock()),
            "POST",
            "/app/activities/api/day/20260510/record/meeting_100000_300/update?facet=work",
            Some(json!({"patch":{"details":"changed"},"note":null})),
        )
        .await;
        assert_eq!(updated["record"]["edits"][1]["note"], "");
    }

    #[tokio::test]
    async fn action_log_failures_propagate_after_the_record_write() {
        let root = phase_root("populated");
        write(&root.path().join("facets/work/logs"), "not a directory");
        let (status, response) = request(
            gated(root.path(), fixed_clock()),
            "POST",
            "/app/activities/api/day/20260510/records?facet=work",
            Some(create_body(Some("111000_300"))),
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(response["reason_code"], "internal_error");
        assert_eq!(
            std::fs::read_to_string(day_path(root.path()))
                .expect("record was written before the Python-equivalent log failure")
                .lines()
                .count(),
            3
        );
    }

    #[tokio::test]
    async fn lock_timeout_uses_the_canonical_activities_busy_reason() {
        let response = store_error(solstone_core_facets::ActivityRecordStoreError::Lock(
            LockError::Timeout(LockTimeout {
                path: "activities.jsonl".into(),
                timeout: Duration::from_millis(1),
            }),
        ));
        let (parts, body) = response.into_parts();
        let body = to_bytes(body, usize::MAX).await.expect("body");
        let body: Value = serde_json::from_slice(&body).expect("JSON");
        assert_eq!(parts.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["reason_code"], "activities_busy");
        assert_eq!(
            body["error"],
            "activities couldn't be updated right now because they were busy. try again in a moment."
        );
    }

    #[tokio::test]
    async fn activities_view_contract_and_spot_check() {
        let root = tempfile::Builder::new().tempdir_in("/var/tmp").unwrap();
        write(
            &root.path().join("config/journal.json"),
            r#"{"setup":{"completed_at":1767225600}}"#,
        );
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        solstone_core_facets::create_facet(root.path(), "home", "Home", "", "", "", None).unwrap();
        solstone_core_facets::create_facet(root.path(), "m", "Muted", "", "", "", None).unwrap();
        write(
            &root.path().join("facets/m/facet.json"),
            r#"{"name":"Muted","muted":true}"#,
        );

        // Spot-check chronicle directories under day 20260715:
        std::fs::create_dir_all(root.path().join("chronicle/20260715/s1/100000_300")).unwrap();
        std::fs::create_dir_all(root.path().join("chronicle/20260715/s2/100000_300")).unwrap();
        std::fs::create_dir_all(root.path().join("chronicle/20260715/s1/100200_300")).unwrap();

        // 1. Day 20260715:
        // work: 2 visible, 2 hidden
        // home: 1 visible (shared_item)
        // m: 2 visible
        // Unscoped visible count: 2 (work) + 1 (home) = 3
        // Unscoped hidden count: 2 (work) + 0 (home) = 2
        let work_rows = r#"{"id":"spot_s1","activity":"meeting","title":"Spot S1","segments":["100000_300"],"stream":"s1","hidden":false,"source":"user"}
{"id":"spot_s2","activity":"meeting","title":"Spot S2","segments":["100000_300"],"stream":"s2","hidden":false,"source":"user"}
{"id":"spot_hid","activity":"meeting","title":"Spot Hid","segments":["100200_300"],"stream":"s1","hidden":true,"source":"user"}
{"id":"shared_item","activity":"meeting","title":"Shared Work","hidden":true,"source":"user"}"#;
        write(
            &root.path().join("facets/work/activities/20260715.jsonl"),
            work_rows,
        );

        let home_rows = r#"{"id":"shared_item","activity":"meeting","title":"Shared Home","hidden":false,"source":"user"}"#;
        write(
            &root.path().join("facets/home/activities/20260715.jsonl"),
            home_rows,
        );

        let m_rows = r#"{"id":"m1","activity":"meeting","title":"M1","hidden":false,"source":"user"}
{"id":"m2","activity":"meeting","title":"M2","hidden":false,"source":"user"}"#;
        write(
            &root.path().join("facets/m/activities/20260715.jsonl"),
            m_rows,
        );

        // GET /app/activities/api/day/20260715/view
        let (status, res) = request(
            gated(root.path(), fixed_clock()),
            "GET",
            "/app/activities/api/day/20260715/view",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(res.get("items").is_some());
        assert!(res.get("facets").is_some());
        assert!(res.get("hidden_count").is_some());

        let items = res["items"].as_array().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(res["hidden_count"], 2);

        // Two declared facets, work and home, same activity id stored in both.
        // Hiding only the work copy removes only that item from the default list.
        assert!(
            items
                .iter()
                .any(|i| i["id"] == "shared_item" && i["facet"] == "home")
        );
        assert!(
            !items
                .iter()
                .any(|i| i["id"] == "shared_item" && i["facet"] == "work")
        );

        // Muted facet m is absent from default items and default hidden_count
        assert!(!items.iter().any(|i| i["facet"] == "m"));
        let facets = res["facets"].as_array().unwrap();
        let m_facet = facets.iter().find(|f| f["name"] == "m").unwrap();
        assert_eq!(m_facet["count"], 2);
        let work_facet = facets.iter().find(|f| f["name"] == "work").unwrap();
        assert_eq!(work_facet["count"], 2);
        let home_facet = facets.iter().find(|f| f["name"] == "home").unwrap();
        assert_eq!(home_facet["count"], 1);

        // GET .../view?facet=m lists those 2
        let (status_m, res_m) = request(
            gated(root.path(), fixed_clock()),
            "GET",
            "/app/activities/api/day/20260715/view?facet=m",
            None,
        )
        .await;
        assert_eq!(status_m, StatusCode::OK);
        assert_eq!(res_m["items"].as_array().unwrap().len(), 2);
        // facets[f].count equals the visible count ?facet=f would return, and does not change when request's own ?facet= is different
        let facets_m = res_m["facets"].as_array().unwrap();
        assert_eq!(
            facets_m.iter().find(|f| f["name"] == "work").unwrap()["count"],
            2
        );
        assert_eq!(
            facets_m.iter().find(|f| f["name"] == "home").unwrap()["count"],
            1
        );
        assert_eq!(
            facets_m.iter().find(|f| f["name"] == "m").unwrap()["count"],
            2
        );

        // With ?include_hidden=1: items length equals visible count plus hidden_count (3 + 2 = 5)
        let (status_all, res_all) = request(
            gated(root.path(), fixed_clock()),
            "GET",
            "/app/activities/api/day/20260715/view?include_hidden=1",
            None,
        )
        .await;
        assert_eq!(status_all, StatusCode::OK);
        let all_items = res_all["items"].as_array().unwrap();
        assert_eq!(all_items.len(), 5);
        assert_eq!(
            all_items.len(),
            items.len() + res["hidden_count"].as_u64().unwrap() as usize
        );
        assert!(
            all_items
                .iter()
                .any(|i| i["id"] == "shared_item" && i["facet"] == "home")
        );
        assert!(
            all_items
                .iter()
                .any(|i| i["id"] == "shared_item" && i["facet"] == "work")
        );

        // ?facet=work lists work's rows while that response's facets still shows home with home's real count
        let (status_w, res_w) = request(
            gated(root.path(), fixed_clock()),
            "GET",
            "/app/activities/api/day/20260715/view?facet=work",
            None,
        )
        .await;
        assert_eq!(status_w, StatusCode::OK);
        let work_items = res_w["items"].as_array().unwrap();
        assert_eq!(work_items.len(), 2);
        assert!(work_items.iter().all(|i| i["facet"] == "work"));
        let facets_w = res_w["facets"].as_array().unwrap();
        assert_eq!(
            facets_w.iter().find(|f| f["name"] == "home").unwrap()["count"],
            1
        );
        assert_eq!(
            facets_w.iter().find(|f| f["name"] == "work").unwrap()["count"],
            2
        );
        assert_eq!(
            facets_w.iter().find(|f| f["name"] == "m").unwrap()["count"],
            2
        );

        // Spot-check fixture assertions:
        // Visible s1 item: stream is "s1", concurrent is true, segment stream is "s1" (not null)
        let s1_item = items.iter().find(|i| i["id"] == "spot_s1").unwrap();
        assert_eq!(s1_item["stream"], "s1");
        assert_eq!(s1_item["concurrent"], true);
        assert_eq!(s1_item["segments"][0]["stream"], "s1");

        // With include_hidden=1, hidden row on s1 is concurrent false
        let hid_item = all_items.iter().find(|i| i["id"] == "spot_hid").unwrap();
        assert_eq!(hid_item["concurrent"], false);

        // 2. Day 20260716:
        // Field fixtures: hidden reasons, edits kinds, edited_by_owner, minute cases, malformed activities.jsonl
        std::fs::create_dir_all(root.path().join("chronicle/20260716/s1/100000_3600")).unwrap();
        std::fs::create_dir_all(root.path().join("chronicle/20260716/s1/101500_600")).unwrap();
        std::fs::create_dir_all(root.path().join("chronicle/20260716/s1/235000_1200")).unwrap();

        // Write malformed activities.jsonl under work:
        write(
            &root.path().join("facets/work/activities/activities.jsonl"),
            "{\"id\":\"project_review\",\"name\":\"Project Review\"}\n{not valid json\n",
        );

        let day16_rows = r#"{"id":"h_owner","activity":"project_review","title":"H Owner","hidden":true,"edits":[{"actor":"owner:mute","fields":["hidden"],"note":"muted"}]}
{"id":"h_cmd","activity":"team_sync","title":"H Cmd","hidden":true,"edits":[{"actor":"cli:mute","fields":["hidden"],"note":"cmd mute"}]}
{"id":"h_repl","activity":"project_review","title":"H Repl","hidden":true,"edits":[{"actor":"schedule","fields":["hidden"],"note":"superseded by new_plan"}]}
{"id":"h_canc","activity":"project_review","title":"H Canc","hidden":true,"cancelled":true}
{"id":"h_other","activity":"project_review","title":"H Other","hidden":true}
{"id":"h_canc_owner","activity":"project_review","title":"H Canc Owner","hidden":true,"cancelled":true,"edits":[{"actor":"schedule","fields":["cancelled"],"note":"cancelled by schedule"},{"actor":"owner:mute","fields":["hidden"],"note":"muted by owner"}]}
{"id":"v_open","activity":"project_review","title":"V Open","hidden":false,"source":"user"}
{"id":"v_plan","activity":"project_review","title":"V Plan","hidden":false,"source":"anticipated"}
{"id":"v_detected","activity":"project_review","title":"V Detected","hidden":false}
{"id":"v_other","activity":"project_review","title":"V Other","hidden":false,"source":"custom"}
{"id":"rec_edits","activity":"project_review","title":"Edits Rec","hidden":false,"source":"user","edits":[{"actor":"activity","note":"grew"},{"actor":"schedule","note":"created by schedule"},{"actor":"schedule","note":"superseded by x"},{"actor":"schedule","note":"updated by schedule"},{"actor":"owner:update","fields":["title"],"note":"updated"},{"actor":"owner:mute","fields":["hidden"],"note":"muted"},{"actor":"owner:unmute","fields":["hidden"],"note":"unmuted"},{"actor":"cli:update","fields":["title"],"note":"cli update"},{"actor":"cli:mute","fields":["hidden"],"note":"cli mute"},{"actor":"cli:unmute","fields":["hidden"],"note":"cli unmute"},{"actor":"cli:create","fields":["title"],"note":"cli create"},{"actor":"owner:other","note":"other"},{"actor":"schedule","note":"cancelled by schedule","fields":["cancelled"]},{"actor":"schedule","note":"not cancelled","fields":["title"]}]}
{"id":"rec_owner1","activity":"project_review","title":"Owner 1","hidden":false,"source":"user","edits":[{"actor":"cli:create","fields":["title"]}]}
{"id":"rec_owner2","activity":"project_review","title":"Owner 2","hidden":false,"source":"user","edits":[{"actor":"schedule","fields":["title"]}]}
{"id":"rec_owner3","activity":"project_review","title":"Owner 3","hidden":false,"source":"user","edits":[{"actor":"cli:update","fields":["title"]}]}
{"id":"rec_owner4","activity":"project_review","title":"Owner 4","hidden":false,"source":"user","edits":[{"actor":"owner:update","fields":["title"]}]}
{"id":"rec_owner5","activity":"project_review","title":"Owner 5","hidden":false,"source":"user","edits":[{"actor":"owner:update","fields":["description"]}]}
{"id":"rec_owner6","activity":"project_review","title":"Owner 6","hidden":false,"source":"user","edits":[{"actor":"owner:update","fields":["details"]}]}
{"id":"rec_owner7","activity":"project_review","title":"Owner 7","hidden":false,"source":"user","edits":[{"actor":"owner:update","fields":["hidden"]}]}
{"id":"plan_null_start","activity":"project_review","title":"Plan Null Start","hidden":false,"source":"anticipated","start":null,"end":"10:00:00"}
{"id":"plan_midnight","activity":"project_review","title":"Plan Midnight","hidden":false,"source":"anticipated","start":"23:00:00","end":"00:30:00"}
{"id":"plan_same","activity":"project_review","title":"Plan Same","hidden":false,"source":"anticipated","start":"09:00:00","end":"09:00:00"}
{"id":"det_midnight","activity":"project_review","title":"Det Midnight","hidden":false,"segments":["235000_1200"],"stream":"s1"}
{"id":"det_longer","activity":"project_review","title":"Det Longer","hidden":false,"segments":["100000_3600","101500_600"],"stream":"s1"}
{"id":"det_no_seg","activity":"project_review","title":"Det No Seg","hidden":false}
{"id":"story_and_people","activity":"project_review","title":"Story & People","hidden":false,"source":"user","story":{"body":"Discussed quarterly roadmap"},"participation":[{"name":"Carol","role":"attendee"},{"name":"Dave","role":"organizer"}]}"#;
        write(
            &root.path().join("facets/work/activities/20260716.jsonl"),
            day16_rows,
        );

        // GET .../view?include_hidden=1 on 20260716
        let (status16, res16) = request(
            gated(root.path(), fixed_clock()),
            "GET",
            "/app/activities/api/day/20260716/view?include_hidden=1",
            None,
        )
        .await;
        assert_eq!(status16, StatusCode::OK);
        let items16 = res16["items"].as_array().unwrap();

        // Malformed activities.jsonl: activity_label is activity id with '_' replaced by spaces
        let team_sync = items16.iter().find(|i| i["id"] == "h_cmd").unwrap();
        assert_eq!(team_sync["activity_label"], "team sync");
        let proj_rev = items16.iter().find(|i| i["id"] == "v_open").unwrap();
        assert_eq!(proj_rev["activity_label"], "project review");

        // hidden_reason assertions
        assert_eq!(
            items16.iter().find(|i| i["id"] == "h_owner").unwrap()["hidden_reason"],
            "owner"
        );
        assert_eq!(
            items16.iter().find(|i| i["id"] == "h_cmd").unwrap()["hidden_reason"],
            "command"
        );
        assert_eq!(
            items16.iter().find(|i| i["id"] == "h_repl").unwrap()["hidden_reason"],
            "other"
        );
        assert_eq!(
            items16.iter().find(|i| i["id"] == "h_canc").unwrap()["hidden_reason"],
            "cancelled"
        );
        assert_eq!(
            items16.iter().find(|i| i["id"] == "h_other").unwrap()["hidden_reason"],
            "other"
        );
        // A cancelled plan the owner later hid is "owner"
        assert_eq!(
            items16.iter().find(|i| i["id"] == "h_canc_owner").unwrap()["hidden_reason"],
            "owner"
        );
        // hidden_reason is absent when item is not hidden
        assert!(
            items16
                .iter()
                .find(|i| i["id"] == "v_open")
                .unwrap()
                .get("hidden_reason")
                .is_none()
        );

        // Retired plan records are never shown.
        for plan in ["v_plan", "plan_null_start", "plan_midnight", "plan_same"] {
            assert!(items16.iter().all(|i| i["id"] != plan), "{plan}");
        }

        // kind mapping
        assert_eq!(
            items16.iter().find(|i| i["id"] == "v_open").unwrap()["kind"],
            "added"
        );
        assert_eq!(
            items16.iter().find(|i| i["id"] == "v_detected").unwrap()["kind"],
            "detected"
        );
        assert_eq!(
            items16.iter().find(|i| i["id"] == "v_other").unwrap()["kind"],
            "other"
        );

        // edits[].kind mapping on rec_edits
        let edits = items16.iter().find(|i| i["id"] == "rec_edits").unwrap()["edits"]
            .as_array()
            .unwrap();
        assert_eq!(edits[0]["kind"], "grew");
        assert_eq!(edits[1]["kind"], "other");
        assert_eq!(edits[2]["kind"], "other");
        assert_eq!(edits[3]["kind"], "other");
        assert_eq!(edits[4]["kind"], "owner_update");
        assert_eq!(edits[5]["kind"], "owner_hide");
        assert_eq!(edits[6]["kind"], "owner_unhide");
        assert_eq!(edits[7]["kind"], "command_update");
        assert_eq!(edits[8]["kind"], "command_hide");
        assert_eq!(edits[9]["kind"], "command_unhide");
        assert_eq!(edits[10]["kind"], "command_create");
        assert_eq!(edits[11]["kind"], "other");
        assert!(edits[12].get("cancelled").is_none());
        assert!(edits[13].get("cancelled").is_none());

        // edited_by_owner
        assert_eq!(
            items16.iter().find(|i| i["id"] == "rec_owner1").unwrap()["edited_by_owner"],
            false
        );
        assert_eq!(
            items16.iter().find(|i| i["id"] == "rec_owner2").unwrap()["edited_by_owner"],
            false
        );
        assert_eq!(
            items16.iter().find(|i| i["id"] == "rec_owner3").unwrap()["edited_by_owner"],
            false
        );
        assert_eq!(
            items16.iter().find(|i| i["id"] == "rec_owner4").unwrap()["edited_by_owner"],
            true
        );
        assert_eq!(
            items16.iter().find(|i| i["id"] == "rec_owner5").unwrap()["edited_by_owner"],
            true
        );
        assert_eq!(
            items16.iter().find(|i| i["id"] == "rec_owner6").unwrap()["edited_by_owner"],
            true
        );
        assert_eq!(
            items16.iter().find(|i| i["id"] == "rec_owner7").unwrap()["edited_by_owner"],
            false
        );

        // Minute cases:
        let d_mid = items16.iter().find(|i| i["id"] == "det_midnight").unwrap();
        assert!(d_mid["end_min"].as_u64().unwrap() > 1440);

        let d_longer = items16.iter().find(|i| i["id"] == "det_longer").unwrap();
        assert_eq!(d_longer["start_min"], 600);
        assert_eq!(d_longer["end_min"], 660);

        let d_no_seg = items16.iter().find(|i| i["id"] == "det_no_seg").unwrap();
        assert!(d_no_seg["start_min"].is_null());
        assert!(d_no_seg["end_min"].is_null());

        // summary and people
        let sp = items16
            .iter()
            .find(|i| i["id"] == "story_and_people")
            .unwrap();
        assert_eq!(sp["summary"], "Discussed quarterly roadmap");
        assert_eq!(sp["people"], serde_json::json!(["Carol"]));

        // Empty day and ?facet= of an undeclared name return empty items, every declared facet at count 0, hidden_count 0
        let (status_emp, res_emp) = request(
            gated(root.path(), fixed_clock()),
            "GET",
            "/app/activities/api/day/20260717/view?facet=undeclared",
            None,
        )
        .await;
        assert_eq!(status_emp, StatusCode::OK);
        assert_eq!(res_emp["items"].as_array().unwrap().len(), 0);
        assert_eq!(res_emp["hidden_count"], 0);
        let emp_facets = res_emp["facets"].as_array().unwrap();
        for f in emp_facets {
            assert_eq!(f["count"], 0);
        }

        // An invalid day (2026-09-01, not eight digits) returns 404 with error invalid_day
        let (status_inv, res_inv) = request(
            gated(root.path(), fixed_clock()),
            "GET",
            "/app/activities/api/day/2026-09-01/view",
            None,
        )
        .await;
        assert_eq!(status_inv, StatusCode::NOT_FOUND);
        assert_eq!(res_inv["reason_code"], "invalid_day");
        assert_eq!(res_inv["detail"], "Day not found");
        assert_eq!(res_inv["error"], "that day couldn't be used.");
    }

    #[tokio::test]
    async fn activities_index_and_stats_contract() {
        let root = tempfile::Builder::new().tempdir_in("/var/tmp").unwrap();
        write(
            &root.path().join("config/journal.json"),
            r#"{"setup":{"completed_at":1767225600}}"#,
        );

        // Empty journal index and stats
        let (status_emp_idx, res_emp_idx) = request(
            gated(root.path(), fixed_clock()),
            "GET",
            "/app/activities/api/index",
            None,
        )
        .await;
        assert_eq!(status_emp_idx, StatusCode::OK);
        assert_eq!(
            res_emp_idx,
            serde_json::json!({"coverage": null, "months": {}})
        );

        let (status_emp_st, res_emp_st) = request(
            gated(root.path(), fixed_clock()),
            "GET",
            "/app/activities/api/stats/202603",
            None,
        )
        .await;
        assert_eq!(status_emp_st, StatusCode::OK);
        assert_eq!(res_emp_st, serde_json::json!({}));

        // Invalid month 2026-09 returns 400 invalid_month
        let (status_inv_m, res_inv_m) = request(
            gated(root.path(), fixed_clock()),
            "GET",
            "/app/activities/api/stats/2026-09",
            None,
        )
        .await;
        assert_eq!(status_inv_m, StatusCode::BAD_REQUEST);
        assert_eq!(res_inv_m["reason_code"], "invalid_month");
        assert_eq!(res_inv_m["detail"], "Invalid month format, expected YYYYMM");

        // Six-digit 202613 is 200 and {}
        let (status_13, res_13) = request(
            gated(root.path(), fixed_clock()),
            "GET",
            "/app/activities/api/stats/202613",
            None,
        )
        .await;
        assert_eq!(status_13, StatusCode::OK);
        assert_eq!(res_13, serde_json::json!({}));

        // Now populate facets work and quiet (muted)
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        solstone_core_facets::create_facet(root.path(), "quiet", "Quiet", "", "", "", None)
            .unwrap();
        write(
            &root.path().join("facets/quiet/facet.json"),
            r#"{"name":"Quiet","muted":true}"#,
        );

        // Non-day files in activities directory
        write(
            &root.path().join("facets/work/activities/activities.jsonl"),
            "{\"id\":\"act\"}\n",
        );
        write(
            &root.path().join("facets/work/activities/not_a_day.txt"),
            "not a day\n",
        );
        write(&root.path().join("facets/work/activities/.lock"), "");

        // 20260301: 1 visible, 1 hidden -> visible count 1
        let w_0301 = r#"{"id":"w1","hidden":false,"source":"user"}
{"id":"w2","hidden":true,"source":"user"}"#;
        write(
            &root.path().join("facets/work/activities/20260301.jsonl"),
            w_0301,
        );

        // 20260302: 1 hidden -> visible count 0 -> absent
        let w_0302 = r#"{"id":"w3","hidden":true,"source":"user"}"#;
        write(
            &root.path().join("facets/work/activities/20260302.jsonl"),
            w_0302,
        );

        // 20260405: 2 visible
        let w_0405 = r#"{"id":"w4","hidden":false,"source":"user"}
{"id":"w5","hidden":false,"source":"user"}"#;
        write(
            &root.path().join("facets/work/activities/20260405.jsonl"),
            w_0405,
        );

        // Future day 20990115: only a retired plan, so no activities
        let w_future = r#"{"id":"plan_future","hidden":false,"source":"anticipated","start":"10:00:00","end":"11:00:00"}"#;
        write(
            &root.path().join("facets/work/activities/20990115.jsonl"),
            w_future,
        );

        // Under quiet: 20260301 has 1 visible, 20260303 has 3 visible
        let q_0301 = r#"{"id":"q1","hidden":false,"source":"user"}"#;
        write(
            &root.path().join("facets/quiet/activities/20260301.jsonl"),
            q_0301,
        );
        let q_0303 = r#"{"id":"q2","hidden":false,"source":"user"}
{"id":"q3","hidden":false,"source":"user"}
{"id":"q4","hidden":false,"source":"user"}"#;
        write(
            &root.path().join("facets/quiet/activities/20260303.jsonl"),
            q_0303,
        );

        // Unscoped index:
        let (status_idx, res_idx) = request(
            gated(root.path(), fixed_clock()),
            "GET",
            "/app/activities/api/index",
            None,
        )
        .await;
        assert_eq!(status_idx, StatusCode::OK);
        assert!(res_idx.get("coverage").is_some());
        assert!(res_idx.get("months").is_some());
        let months = res_idx["months"].as_object().unwrap();
        // months keys are only YYYYMM
        for k in months.keys() {
            assert_eq!(k.len(), 6);
            assert!(k.chars().all(|c| c.is_ascii_digit()));
        }
        // Unscoped omits muted facet quiet
        assert!(months.contains_key("202603"));
        assert!(months.contains_key("202604"));
        assert!(!months.contains_key("209901"));

        // Unscoped stats for 202603: {YYYYMMDD: count} and nothing else
        let (status_st, res_st) = request(
            gated(root.path(), fixed_clock()),
            "GET",
            "/app/activities/api/stats/202603",
            None,
        )
        .await;
        assert_eq!(status_st, StatusCode::OK);
        assert_eq!(res_st, serde_json::json!({"20260301": 1}));

        // ?facet= limits both payloads to that facet, including muted facet
        let (status_q_st, res_q_st) = request(
            gated(root.path(), fixed_clock()),
            "GET",
            "/app/activities/api/stats/202603?facet=quiet",
            None,
        )
        .await;
        assert_eq!(status_q_st, StatusCode::OK);
        assert_eq!(res_q_st, serde_json::json!({"20260301": 1, "20260303": 3}));

        let (status_q_idx, res_q_idx) = request(
            gated(root.path(), fixed_clock()),
            "GET",
            "/app/activities/api/index?facet=quiet",
            None,
        )
        .await;
        assert_eq!(status_q_idx, StatusCode::OK);
        let q_months = res_q_idx["months"].as_object().unwrap();
        assert!(q_months.contains_key("202603"));
        assert!(!q_months.contains_key("202604"));
    }

    #[tokio::test]
    async fn activities_owner_marker_actor_and_audit_log() {
        let root = tempfile::Builder::new().tempdir_in("/var/tmp").unwrap();
        write(
            &root.path().join("config/journal.json"),
            r#"{"setup":{"completed_at":1767225600}}"#,
        );
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();

        // Seed record 1: id rec_owner
        let seed1 = r#"{"id":"rec_owner","activity":"meeting","title":"Seed Owner","description":"","details":"","segments":[],"active_entities":[],"created_at":1767225600000,"source":"user","hidden":false,"edits":[]}"#;
        write(
            &root.path().join("facets/work/activities/20260510.jsonl"),
            &format!("{seed1}\n"),
        );

        // Helper to send request with x-solstone-actor header
        let send_with_marker = |method: &'static str,
                                uri: String,
                                actor: Option<&'static str>,
                                body: Option<Value>| {
            let app = gated(root.path(), fixed_clock());
            async move {
                let mut req = Request::builder().method(method).uri(uri);
                if let Some(a) = actor {
                    req = req.header("x-solstone-actor", a);
                }
                if body.is_some() {
                    req = req.header(header::CONTENT_TYPE, "application/json");
                }
                let body_bytes = body
                    .map(|b| Body::from(serde_json::to_vec(&b).unwrap()))
                    .unwrap_or_else(Body::empty);
                let res = app.oneshot(req.body(body_bytes).unwrap()).await.unwrap();
                let status = res.status();
                let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
                let val: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                (status, val)
            }
        };

        // 1. With X-Solstone-Actor: owner
        let (status, res) = send_with_marker(
            "POST",
            "/app/activities/api/day/20260510/record/rec_owner/update?facet=work".into(),
            Some("owner"),
            Some(serde_json::json!({"patch":{"title":"Updated Owner"}})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(res["record"]["edits"][0]["actor"], "owner:update");

        let (status, res) = send_with_marker(
            "POST",
            "/app/activities/api/day/20260510/record/rec_owner/mute?facet=work".into(),
            Some("owner"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(res["record"]["edits"][1]["actor"], "owner:mute");

        let (status, res) = send_with_marker(
            "POST",
            "/app/activities/api/day/20260510/record/rec_owner/unmute?facet=work".into(),
            Some("owner"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(res["record"]["edits"][2]["actor"], "owner:unmute");

        // Action log check for owner calls: source "app", actor "activities"
        let logs_raw =
            std::fs::read_to_string(root.path().join("facets/work/logs/20260510.jsonl")).unwrap();
        let log_lines: Vec<Value> = logs_raw
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(log_lines.len(), 3);
        for entry in &log_lines[0..3] {
            assert_eq!(entry["source"], "app");
            assert_eq!(entry["actor"], "activities");
        }

        // 2. The same three routes with no header on a different record store cli:update, cli:mute, cli:unmute
        let seed2 = r#"{"id":"rec_cli","activity":"meeting","title":"Seed Cli","description":"","details":"","segments":[],"active_entities":[],"created_at":1767225600000,"source":"user","hidden":false,"edits":[]}"#;
        let _ = solstone_core_facets::append_activity_record(
            root.path(),
            "work",
            "20260510",
            serde_json::from_str(seed2).unwrap(),
        )
        .unwrap();

        let (status, res) = send_with_marker(
            "POST",
            "/app/activities/api/day/20260510/record/rec_cli/update?facet=work".into(),
            None,
            Some(serde_json::json!({"patch":{"title":"Updated Cli"}})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(res["record"]["edits"][0]["actor"], "cli:update");

        let (status, res) = send_with_marker(
            "POST",
            "/app/activities/api/day/20260510/record/rec_cli/mute?facet=work".into(),
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(res["record"]["edits"][1]["actor"], "cli:mute");

        let (status, res) = send_with_marker(
            "POST",
            "/app/activities/api/day/20260510/record/rec_cli/unmute?facet=work".into(),
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(res["record"]["edits"][2]["actor"], "cli:unmute");

        let logs_raw2 =
            std::fs::read_to_string(root.path().join("facets/work/logs/20260510.jsonl")).unwrap();
        let log_lines2: Vec<Value> = logs_raw2
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(log_lines2.len(), 6);
        for entry in &log_lines2[3..6] {
            assert_eq!(entry["source"], "call");
            assert_eq!(entry["actor"], "agent");
        }

        // 3. Create with header still stores cli:create and logs source "call", actor "agent"
        let (status, res) = send_with_marker(
            "POST",
            "/app/activities/api/day/20260510/records?facet=work".into(),
            Some("owner"),
            Some(serde_json::json!({"title":"Created With Header","activity":"meeting"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(res["record"]["edits"][0]["actor"], "cli:create");

        let logs_raw3 =
            std::fs::read_to_string(root.path().join("facets/work/logs/20260510.jsonl")).unwrap();
        let log_lines3: Vec<Value> = logs_raw3
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(log_lines3.len(), 7);
        assert_eq!(log_lines3[6]["source"], "call");
        assert_eq!(log_lines3[6]["actor"], "agent");

        // 4. Header value owner:update or Owner is the command path (cli:*)
        let seed3 = r#"{"id":"rec_bad_actor1","activity":"meeting","title":"Seed Bad 1","description":"","details":"","segments":[],"active_entities":[],"created_at":1767225600000,"source":"user","hidden":false,"edits":[]}"#;
        let _ = solstone_core_facets::append_activity_record(
            root.path(),
            "work",
            "20260510",
            serde_json::from_str(seed3).unwrap(),
        )
        .unwrap();
        let (_, res) = send_with_marker(
            "POST",
            "/app/activities/api/day/20260510/record/rec_bad_actor1/update?facet=work".into(),
            Some("owner:update"),
            Some(serde_json::json!({"patch":{"title":"Updated Bad 1"}})),
        )
        .await;
        assert_eq!(res["record"]["edits"][0]["actor"], "cli:update");

        let seed4 = r#"{"id":"rec_bad_actor2","activity":"meeting","title":"Seed Bad 2","description":"","details":"","segments":[],"active_entities":[],"created_at":1767225600000,"source":"user","hidden":false,"edits":[]}"#;
        let _ = solstone_core_facets::append_activity_record(
            root.path(),
            "work",
            "20260510",
            serde_json::from_str(seed4).unwrap(),
        )
        .unwrap();
        let (_, res) = send_with_marker(
            "POST",
            "/app/activities/api/day/20260510/record/rec_bad_actor2/update?facet=work".into(),
            Some("Owner"),
            Some(serde_json::json!({"patch":{"title":"Updated Bad 2"}})),
        )
        .await;
        assert_eq!(res["record"]["edits"][0]["actor"], "cli:update");
    }

    #[tokio::test]
    async fn activities_workspace_asset_contains_owner_constants() {
        let res = crate::assets::activities_workspace();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let body_str = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body_str.contains(ACTOR_HEADER));
        assert!(body_str.contains(ACTOR_OWNER));
    }

    #[tokio::test]
    async fn activities_today_redirect_preserves_query_and_uses_injected_clock() {
        let root = phase_root("established_empty");
        let dt = chrono::DateTime::parse_from_rfc3339("2026-10-15T23:30:00-07:00").unwrap();
        let clock = Clock::new(move || dt);
        let app = gated(root.path(), clock);

        // 302 redirect with query preserved
        let req = Request::builder()
            .method("GET")
            .uri("/app/activities/?facet=work")
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::FOUND);
        assert_eq!(
            res.headers().get(header::LOCATION).unwrap(),
            "/app/activities/20261015?facet=work"
        );

        // 302 redirect without query
        let req2 = Request::builder()
            .method("GET")
            .uri("/app/activities/")
            .body(Body::empty())
            .unwrap();
        let res2 = app.clone().oneshot(req2).await.unwrap();
        assert_eq!(res2.status(), StatusCode::FOUND);
        assert_eq!(
            res2.headers().get(header::LOCATION).unwrap(),
            "/app/activities/20261015"
        );

        // Shell route on valid day returns 200 shell
        let req3 = Request::builder()
            .method("GET")
            .uri("/app/activities/20261015")
            .body(Body::empty())
            .unwrap();
        let res3 = app.clone().oneshot(req3).await.unwrap();
        assert_eq!(res3.status(), StatusCode::OK);

        // Shell route on invalid day returns 404
        let req4 = Request::builder()
            .method("GET")
            .uri("/app/activities/notaday")
            .body(Body::empty())
            .unwrap();
        let res4 = app.clone().oneshot(req4).await.unwrap();
        assert_eq!(res4.status(), StatusCode::NOT_FOUND);
        let bytes4 = to_bytes(res4.into_body(), usize::MAX).await.unwrap();
        let body4: Value = serde_json::from_slice(&bytes4).unwrap();
        assert_eq!(body4["reason_code"], "invalid_day");

        // Workspace route returns 200 and contains data-date-nav-keep-search
        let req5 = Request::builder()
            .method("GET")
            .uri("/app/activities/workspace")
            .body(Body::empty())
            .unwrap();
        let res5 = app.oneshot(req5).await.unwrap();
        assert_eq!(res5.status(), StatusCode::OK);
        let bytes5 = to_bytes(res5.into_body(), usize::MAX).await.unwrap();
        let body5 = String::from_utf8(bytes5.to_vec()).unwrap();
        assert!(body5.contains("data-date-nav-keep-search"));
    }

    #[tokio::test]
    async fn activities_view_facets_carry_title_color_and_muted() {
        let root = tempfile::Builder::new().tempdir_in("/var/tmp").unwrap();
        write(
            &root.path().join("config/journal.json"),
            r#"{"setup":{"completed_at":1767225600}}"#,
        );
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "#123456", "", None)
            .unwrap();
        write(
            &root.path().join("facets/untitled/facet.json"),
            r#"{"title":"","color":""}"#,
        );
        write(
            &root.path().join("facets/quiet/facet.json"),
            r##"{"title":"Quiet","color":"#abcdef","muted":true,"id":"not-well-formed"}"##,
        );
        let seed = |id: &str| {
            format!(
                r#"{{"id":"{id}","activity":"meeting","title":"t","description":"","details":"","segments":[],"active_entities":[],"created_at":1767225600000,"source":"user","hidden":false,"edits":[]}}"#
            )
        };
        write(
            &root.path().join("facets/quiet/activities/20260510.jsonl"),
            &format!("{}\n", seed("quiet_one")),
        );
        write(
            &root.path().join("facets/work/activities/20260510.jsonl"),
            &format!("{}\n", seed("work_one")),
        );

        let (status, res) = request(
            gated(root.path(), fixed_clock()),
            "GET",
            "/app/activities/api/day/20260510/view",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let facets = res["facets"].as_array().expect("facets array");
        let entry = |name: &str| {
            facets
                .iter()
                .find(|f| f["name"] == name)
                .unwrap_or_else(|| panic!("{name} listed"))
                .clone()
        };
        let work = entry("work");
        assert_eq!(work["title"], "Work");
        assert_eq!(work["color"], "#123456");
        assert_eq!(work["muted"], false);
        assert_eq!(entry("untitled")["title"], "untitled");
        let quiet = entry("quiet");
        assert_eq!(quiet["muted"], true);
        assert_eq!(quiet["count"], 1);
        let ids: Vec<&str> = res["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["work_one"]);
        assert_eq!(res["hidden_count"], 0);

        let (_, stats) = request(
            gated(root.path(), fixed_clock()),
            "GET",
            "/app/activities/api/stats/202605",
            None,
        )
        .await;
        assert_eq!(stats["20260510"], 1);
    }

    #[tokio::test]
    async fn activities_edit_ordering_on_owner_muted_record() {
        let root = tempfile::Builder::new().tempdir_in("/var/tmp").unwrap();
        write(
            &root.path().join("config/journal.json"),
            r#"{"setup":{"completed_at":1767225600}}"#,
        );
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();

        let seed = r#"{"id":"rec_muted_order","activity":"meeting","title":"Seed Muted Order","description":"","details":"","segments":[],"active_entities":[],"created_at":1767225600000,"source":"user","hidden":false,"edits":[]}"#;
        write(
            &root.path().join("facets/work/activities/20260510.jsonl"),
            &format!("{seed}\n"),
        );

        // 1. Mute as owner
        let app = gated(root.path(), fixed_clock());
        let req = Request::builder()
            .method("POST")
            .uri("/app/activities/api/day/20260510/record/rec_muted_order/mute?facet=work")
            .header(ACTOR_HEADER, ACTOR_OWNER)
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // 2. Append schedule cancellation edit (fields: ["cancelled"])
        let day_file = root.path().join("facets/work/activities/20260510.jsonl");
        let raw = std::fs::read_to_string(&day_file).unwrap();
        let mut rec: Value = serde_json::from_str(raw.trim()).unwrap();
        rec["cancelled"] = json!(true);
        rec["edits"].as_array_mut().unwrap().push(json!({
            "timestamp": "2026-05-10T12:00:00Z",
            "actor": "schedule",
            "fields": ["cancelled"],
            "note": "cancelled"
        }));
        write(
            &day_file,
            &format!("{}\n", serde_json::to_string(&rec).unwrap()),
        );

        // View with include_hidden=1: hidden is true, hidden_reason is "owner"
        let (status, res) = request(
            gated(root.path(), fixed_clock()),
            "GET",
            "/app/activities/api/day/20260510/view?include_hidden=1",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let items = res["items"].as_array().unwrap();
        let item = items.iter().find(|i| i["id"] == "rec_muted_order").unwrap();
        assert_eq!(item["hidden"], true);
        assert_eq!(item["hidden_reason"], "owner");
        assert_eq!(item["cancelled"], true);

        // 3. Append schedule un-cancellation edit
        let raw2 = std::fs::read_to_string(&day_file).unwrap();
        let mut rec2: Value = serde_json::from_str(raw2.trim()).unwrap();
        rec2["cancelled"] = json!(false);
        rec2["edits"].as_array_mut().unwrap().push(json!({
            "timestamp": "2026-05-10T13:00:00Z",
            "actor": "schedule",
            "fields": ["cancelled"],
            "note": "uncancelled"
        }));
        write(
            &day_file,
            &format!("{}\n", serde_json::to_string(&rec2).unwrap()),
        );

        // View again: hidden is still true, hidden_reason is still "owner", cancelled is false
        let (status2, res2) = request(
            gated(root.path(), fixed_clock()),
            "GET",
            "/app/activities/api/day/20260510/view?include_hidden=1",
            None,
        )
        .await;
        assert_eq!(status2, StatusCode::OK);
        let items2 = res2["items"].as_array().unwrap();
        let item2 = items2
            .iter()
            .find(|i| i["id"] == "rec_muted_order")
            .unwrap();
        assert_eq!(item2["hidden"], true);
        assert_eq!(item2["hidden_reason"], "owner");
        assert_eq!(item2["cancelled"], false);
    }
}
