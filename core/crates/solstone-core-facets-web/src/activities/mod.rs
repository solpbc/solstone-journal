// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native activities API routes.

use std::path::{Path, PathBuf};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path as RoutePath, RawQuery, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Map, Value, json};
use solstone_core_entity::{
    EntityResolutionOutcome, load_all_journal_entities, record_entity_resolution,
};
use solstone_core_facets::{ActivityRecord, AppendOutcome};
use solstone_core_format::content::{ChatLabels, Family, produce_chunks_by_shape};

use crate::{Clock, http};

pub fn routes(root: PathBuf, clock: Clock) -> Router {
    Router::new()
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

async fn list(
    State((root, _)): State<(PathBuf, Clock)>,
    RoutePath(day): RoutePath<String>,
    RawQuery(query): RawQuery,
) -> Response {
    if let Some(response) = session_response(&root) {
        return response;
    }
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
    if let Some(response) = session_response(&root) {
        return response;
    }
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
    if let Some(response) = session_response(&root) {
        return response;
    }
    let body = object_body(&body);
    let title = string(&body, "title").trim().to_owned();
    let source = body
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or("user")
        .to_owned();
    if title.is_empty() {
        return invalid("title must not be empty");
    }
    if !matches!(source.as_str(), "user" | "cogitate") {
        return invalid("source must be 'user' or 'cogitate'");
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
            value_to_string(value),
            vec![Value::String(value_to_string(value))],
        ),
        _ => (
            format!("user_{}", clock.now().and_utc().timestamp_millis()),
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
        json!(clock.now().and_utc().timestamp_millis()),
    );
    record.insert("source".to_owned(), json!(source));
    record.insert("hidden".to_owned(), json!(false));
    let mut fields = vec!["activity", "title", "description", "details", "source"];
    if participation_provided {
        record.insert("participation".to_owned(), Value::Array(participation));
        fields.push("participation");
    }
    record.insert("edits".to_owned(), json!([{"timestamp":clock.now().and_utc().format("%Y-%m-%dT%H:%M:%SZ").to_string(),"actor":if source == "cogitate" { "cogitate:activities" } else { "cli:create" },"fields":fields,"note":"created"}]));
    match solstone_core_facets::append_activity_record(&root, &facet, &day, record) {
        Ok(AppendOutcome::Written(record)) => {
            let _ = solstone_core_facets::append_action_log_for_day(
                &root,
                Some(&facet),
                "call",
                "agent",
                "activity_create",
                json!({"id":span_id,"activity":activity,"source":source}),
                &day,
            );
            Json(payload(record)).into_response()
        }
        Ok(AppendOutcome::AlreadyExists) => already_exists(span_id),
        Err(error) => store_error(error),
    }
}

async fn update(
    State((root, clock)): State<(PathBuf, Clock)>,
    RoutePath((day, span_id)): RoutePath<(String, String)>,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Response {
    mutate_update(&root, &clock, day, span_id, query, body).await
}
async fn mute(
    State((root, clock)): State<(PathBuf, Clock)>,
    RoutePath((day, span_id)): RoutePath<(String, String)>,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Response {
    set_hidden(&root, &clock, day, span_id, query, body, true)
}
async fn unmute(
    State((root, clock)): State<(PathBuf, Clock)>,
    RoutePath((day, span_id)): RoutePath<(String, String)>,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Response {
    set_hidden(&root, &clock, day, span_id, query, body, false)
}

async fn mutate_update(
    root: &Path,
    clock: &Clock,
    day: String,
    span_id: String,
    query: Option<String>,
    body: Bytes,
) -> Response {
    if let Some(response) = session_response(root) {
        return response;
    }
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
    let note = value_to_string(body.get("note").unwrap_or(&Value::Null));
    let timestamp = clock
        .now()
        .and_utc()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    match solstone_core_facets::update_activity_record(
        root,
        &facet,
        &day,
        &span_id,
        patch,
        "cli:update",
        &note,
        &timestamp,
    ) {
        Ok(Some(record)) => {
            let _ = solstone_core_facets::append_action_log_for_day(
                root,
                Some(&facet),
                "call",
                "agent",
                "activity_update",
                json!({"id":span_id,"fields":patch.keys().collect::<Vec<_>>() }),
                &day,
            );
            Json(payload(record)).into_response()
        }
        Ok(None) | Err(solstone_core_facets::ActivityRecordStoreError::MissingDayFile { .. }) => {
            not_found(span_id)
        }
        Err(error) => store_error(error),
    }
}

fn set_hidden(
    root: &Path,
    clock: &Clock,
    day: String,
    span_id: String,
    query: Option<String>,
    body: Bytes,
    hidden: bool,
) -> Response {
    if let Some(response) = session_response(root) {
        return response;
    }
    let facet = query_value(query.as_deref(), "facet").unwrap_or_default();
    if facet.is_empty() {
        return not_found(span_id);
    }
    let body = object_body(&body);
    let reason = body.get("reason").and_then(Value::as_str);
    let timestamp = clock
        .now()
        .and_utc()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    match solstone_core_facets::set_activity_hidden(
        root,
        &facet,
        &day,
        &span_id,
        hidden,
        if hidden { "cli:mute" } else { "cli:unmute" },
        reason,
        &timestamp,
    ) {
        Ok(Some(record)) => {
            let action = if hidden {
                "activity_mute"
            } else {
                "activity_unmute"
            };
            let _ = solstone_core_facets::append_action_log_for_day(
                root,
                Some(&facet),
                "call",
                "agent",
                action,
                json!({"id":span_id,"reason":reason}),
                &day,
            );
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
    let entities = load_all_journal_entities(root)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|entity| entity.resolution_entity())
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
    let markdown = produce_chunks_by_shape(
        Family::Activity,
        None,
        std::slice::from_ref(&record),
        &ChatLabels::default(),
    )
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
    record.get(key).map(value_to_string).unwrap_or_default()
}
fn value_to_string(value: &Value) -> String {
    value
        .as_str()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| value.to_string())
}
fn invalid(detail: &str) -> Response {
    http::error(
        "activity_invalid",
        "I couldn't use that activity setting.",
        detail.to_owned(),
        StatusCode::BAD_REQUEST,
    )
}
fn not_found(detail: String) -> Response {
    http::error(
        "activity_not_found",
        "I couldn't find that activity in the facet.",
        detail,
        StatusCode::NOT_FOUND,
    )
}
fn already_exists(detail: String) -> Response {
    http::error(
        "activity_already_exists",
        "I couldn't create that activity because it already exists.",
        detail,
        StatusCode::CONFLICT,
    )
}
fn internal(detail: String) -> Response {
    http::error(
        "internal_error",
        "I couldn't complete that request.",
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
            "Activities are busy; try again.",
            "activities are busy; try again".to_owned(),
            StatusCode::SERVICE_UNAVAILABLE,
        )
    } else {
        internal(error.to_string())
    }
}
fn session_response(root: &Path) -> Option<Response> {
    let path = root.join("config/journal.json");
    if !path.exists() {
        return Some(
            axum::http::Response::builder()
                .status(StatusCode::FOUND)
                .header(header::LOCATION, "/init")
                .body(axum::body::Body::empty())
                .expect("redirect"),
        );
    }
    if std::fs::read_to_string(path)
        .ok()
        .is_some_and(|contents| serde_json::from_str::<Value>(&contents).is_err())
    {
        return Some(http::error(
            "corrupt_config",
            "I couldn't read the journal configuration.",
            "journal configuration is corrupt".to_owned(),
            StatusCode::INTERNAL_SERVER_ERROR,
        ));
    }
    None
}
