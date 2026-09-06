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
            activity_value_string(value),
            vec![Value::String(activity_value_string(value))],
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
            if let Err(error) = solstone_core_facets::append_action_log_for_day(
                root,
                Some(&facet),
                "call",
                "agent",
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

fn set_hidden(
    root: &Path,
    clock: &Clock,
    day: String,
    span_id: String,
    query: Option<String>,
    body: Bytes,
    hidden: bool,
) -> Response {
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
            if let Err(error) = solstone_core_facets::append_action_log_for_day(
                root,
                Some(&facet),
                "call",
                "agent",
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
        "that request couldn't be completed.",
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
                "source must be 'user' or 'cogitate'",
            ),
            (
                json!({"title":"x","activity":"missing"}),
                "activity_not_found",
                "missing",
            ),
            (
                json!({"title":"x","source":"bad","activity":"missing"}),
                "activity_invalid",
                "source must be 'user' or 'cogitate'",
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
        assert_eq!(refused["detail"], "source must be 'user' or 'cogitate'");
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
}
