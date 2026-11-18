// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use axum::{body::Bytes, extract::Path, http::StatusCode, response::Response};
use serde_json::{Map, Value, json};

use crate::{
    facets,
    http::{
        activity_not_found, activity_settings_unavailable, facet_not_found, invalid_config_value,
        json_response, missing_request_body, missing_required_field, settings_operation_failed,
    },
    icons,
    request_body::{JsonBody, json_body},
};

mod default_activities {
    include!(concat!(env!("OUT_DIR"), "/default_activities.rs"));
}

pub async fn defaults() -> Response {
    json_response(json!({"activities": default_records()}))
}

pub async fn for_facet(journal_root: PathBuf, Path(facet_name): Path<String>) -> Response {
    if facets::facet(&journal_root, &facet_name).is_none() {
        return facet_not_found();
    }
    let mut activities =
        match solstone_core_facets::read_activity_definitions(&journal_root, &facet_name) {
            Ok(rows) => rows.into_iter().map(attached_record).collect::<Vec<_>>(),
            Err(_) => return activity_settings_unavailable(),
        };
    if activities.is_empty() {
        activities = default_records()
            .into_iter()
            .map(default_for_facet)
            .collect();
    }
    let existing: Vec<String> = activities
        .iter()
        .filter_map(|activity| {
            activity
                .get("id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .collect();
    for default in always_on_records() {
        if !existing.iter().any(|id| default["id"].as_str() == Some(id)) {
            activities.push(default_for_facet(default));
        }
    }
    json_response(json!({"activities": activities, "defaults": default_records()}))
}

pub async fn add(journal_root: PathBuf, Path(facet_name): Path<String>, body: Bytes) -> Response {
    if facets::facet(&journal_root, &facet_name).is_none() {
        return facet_not_found();
    }
    let JsonBody::Value(Value::Object(data)) = json_body(body) else {
        return missing_request_body();
    };
    if data.is_empty() {
        return missing_request_body();
    }
    let id = data
        .get("id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| data.get("name").and_then(Value::as_str).map(slug))
        .unwrap_or_default();
    if id.is_empty() {
        return missing_required_field("Either id or name is required");
    }
    let known = raw_default_records()
        .into_iter()
        .any(|record| record["id"].as_str() == Some(&id));
    if !known && data.get("name").and_then(Value::as_str).is_none() {
        return missing_required_field("name is required for custom activities");
    }
    if data
        .get("priority")
        .is_some_and(|value| !matches!(value.as_str(), Some("high" | "normal" | "low")))
    {
        return invalid_config_value("invalid priority");
    }
    if let Some(icon) = data.get("icon")
        && (!icon.is_string()
            || (!icon.as_str().is_some_and(str::is_empty)
                && icons::svg(icon.as_str(), "").is_none()))
    {
        return invalid_config_value("icon must be a Lucide name; send emoji in emoji");
    }
    let mut row = Map::new();
    row.insert("id".to_owned(), Value::String(id.clone()));
    if known {
        for key in ["description", "instructions"] {
            if let Some(value) = data
                .get(key)
                .filter(|value| !value.as_str().is_some_and(str::is_empty))
            {
                row.insert(key.to_owned(), value.clone());
            }
        }
        if let Some(value) = data
            .get("priority")
            .filter(|value| value.as_str() != Some("normal"))
        {
            row.insert("priority".to_owned(), value.clone());
        }
    } else {
        row.insert("custom".to_owned(), Value::Bool(true));
        row.insert(
            "name".to_owned(),
            data.get("name")
                .cloned()
                .unwrap_or_else(|| Value::String(id.replace('_', " "))),
        );
        row.insert(
            "description".to_owned(),
            data.get("description")
                .cloned()
                .unwrap_or_else(|| json!("")),
        );
        if let Some(value) = data
            .get("priority")
            .filter(|value| value.as_str() != Some("normal"))
        {
            row.insert("priority".to_owned(), value.clone());
        }
        for key in ["instructions", "emoji", "icon"] {
            if let Some(value) = data
                .get(key)
                .filter(|value| !value.as_str().is_some_and(str::is_empty))
            {
                row.insert(key.to_owned(), value.clone());
            }
        }
    }
    match solstone_core_facets::add_activity(&journal_root, &facet_name, Value::Object(row)) {
        Ok(activity) => {
            if solstone_core_facets::append_action_log(
                &journal_root,
                Some(&facet_name),
                "app",
                "settings",
                "activity_add",
                json!({"activity_id": id}),
            )
            .is_err()
            {
                return settings_operation_failed();
            }
            let mut response =
                json_response(json!({"success":true,"activity":attached_record(activity)}));
            *response.status_mut() = StatusCode::CREATED;
            response
        }
        Err(_) => settings_operation_failed(),
    }
}

pub async fn update(
    journal_root: PathBuf,
    Path((facet_name, activity_id)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    if facets::facet(&journal_root, &facet_name).is_none() {
        return facet_not_found();
    };
    let JsonBody::Value(Value::Object(data)) = json_body(body) else {
        return missing_request_body();
    };
    if data.is_empty() {
        return missing_request_body();
    }
    if data
        .get("priority")
        .is_some_and(|value| !matches!(value.as_str(), Some("high" | "normal" | "low")))
    {
        return invalid_config_value("invalid priority");
    }
    if let Some(icon) = data.get("icon")
        && (!icon.is_string()
            || (!icon.as_str().is_some_and(str::is_empty)
                && icons::svg(icon.as_str(), "").is_none()))
    {
        return invalid_config_value("icon must be a Lucide name; send emoji in emoji");
    }
    let updates = data
        .iter()
        .filter(|(key, _)| {
            [
                "description",
                "instructions",
                "priority",
                "name",
                "emoji",
                "icon",
            ]
            .contains(&key.as_str())
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Map<_, _>>();
    // A priority on a default the facet shows but doesn't store: the facet
    // stores what it shows first, so nothing it shows goes (`drawn_to_store`).
    let result = if updates.len() == 1 && updates.contains_key("priority") {
        solstone_core_facets::update_activity_attaching(
            &journal_root,
            &facet_name,
            &activity_id,
            &updates,
            |rows| drawn_to_store(rows, &activity_id),
            only_bare_defaults,
        )
    } else {
        solstone_core_facets::update_activity(&journal_root, &facet_name, &activity_id, &updates)
    };
    match result {
        Ok(Some(activity)) => {
            if solstone_core_facets::append_action_log(
                &journal_root,
                Some(&facet_name),
                "app",
                "settings",
                "activity_update",
                json!({"activity_id": activity_id, "updates": data}),
            )
            .is_err()
            {
                return settings_operation_failed();
            }
            json_response(json!({"success":true,"activity":attached_record(activity)}))
        }
        Ok(None) => activity_not_found(),
        Err(_) => settings_operation_failed(),
    }
}

pub async fn remove(
    journal_root: PathBuf,
    Path((facet_name, activity_id)): Path<(String, String)>,
    _body: Bytes,
) -> Response {
    if facets::facet(&journal_root, &facet_name).is_none() {
        return facet_not_found();
    };
    if always_on_records()
        .iter()
        .any(|row| row["id"].as_str() == Some(&activity_id))
    {
        return crate::http::activity_protected();
    }
    match solstone_core_facets::remove_activity(&journal_root, &facet_name, &activity_id) {
        Ok(true) => {
            if solstone_core_facets::append_action_log(
                &journal_root,
                Some(&facet_name),
                "app",
                "settings",
                "activity_remove",
                json!({"activity_id": activity_id}),
            )
            .is_err()
            {
                return settings_operation_failed();
            }
            json_response(json!({"success":true}))
        }
        Ok(false) => activity_not_found(),
        Err(_) => settings_operation_failed(),
    }
}

fn slug(value: &str) -> String {
    let slug = value
        .to_ascii_lowercase()
        .chars()
        .map(|value| {
            if value.is_ascii_alphanumeric() {
                value
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_owned();
    if slug.is_empty() {
        "activity".to_owned()
    } else {
        slug
    }
}

/// The rows a facet stores before a default it shows takes a change, so that
/// it shows the same activities in the same order afterwards. With no rows it
/// shows every default: all of them, as bare rows. With rows it shows them and
/// then each `always_on` default they lack, in the defaults' order: those up
/// to and including `id`. Nothing for an id the facet doesn't show.
fn drawn_to_store(rows: &[Value], id: &str) -> Vec<Value> {
    let bare = |record: &Value| json!({"id": record["id"]});
    if rows.is_empty() {
        let defaults = raw_default_records();
        if !defaults
            .iter()
            .any(|record| record["id"].as_str() == Some(id))
        {
            return Vec::new();
        }
        return defaults.iter().map(bare).collect();
    }
    let stored = |record: &Value| {
        rows.iter()
            .any(|row| row.get("id").and_then(Value::as_str) == record["id"].as_str())
    };
    let missing = always_on_records()
        .into_iter()
        .filter(|record| !stored(record))
        .collect::<Vec<_>>();
    match missing
        .iter()
        .position(|record| record["id"].as_str() == Some(id))
    {
        Some(at) => missing[..=at].iter().map(bare).collect(),
        None => Vec::new(),
    }
}

/// Rows that are every default, bare, in the defaults' order: a facet that
/// stores them shows what one that stores nothing shows, so it stores nothing
/// and follows the defaults as they change.
fn only_bare_defaults(rows: &[Value]) -> bool {
    let defaults = raw_default_records();
    rows.len() == defaults.len()
        && rows.iter().zip(&defaults).all(|(row, record)| {
            row.as_object()
                .is_some_and(|row| row.len() == 1 && row.get("id") == record.get("id"))
        })
}

fn default_records() -> Vec<Value> {
    raw_default_records()
}

fn always_on_records() -> Vec<Value> {
    raw_default_records()
        .into_iter()
        .filter(|activity| activity["always_on"].as_bool() == Some(true))
        .collect()
}

fn default_for_facet(mut record: Value) -> Value {
    let values = record.as_object_mut().expect("default activity object");
    values.insert("custom".to_owned(), Value::Bool(false));
    values
        .entry("priority".to_owned())
        .or_insert_with(|| json!("normal"));
    record
}

fn raw_default_records() -> Vec<Value> {
    stored_default_records()
        .into_iter()
        .map(public_record)
        .collect()
}

fn stored_default_records() -> Vec<Value> {
    serde_json::from_str::<Vec<Value>>(default_activities::JSON)
        .expect("generated default activities")
}

/// Project a stored facet row for the page. Attaching a default stores only its
/// id and the owner's overrides, so the default's name, description and icons
/// are filled back in, and a missing priority reads as normal.
fn attached_record(row: Value) -> Value {
    let Value::Object(stored) = row else {
        return public_record(row);
    };
    let id = stored.get("id").and_then(Value::as_str).unwrap_or_default();
    let default = stored_default_records()
        .into_iter()
        .find(|record| record["id"].as_str() == Some(id));
    let known = default.is_some();
    let mut record = match default {
        Some(Value::Object(default)) => default,
        _ => Map::new(),
    };
    let name = id.replace('_', " ");
    record.extend(stored);
    record.entry("custom").or_insert(Value::Bool(!known));
    record.entry("name").or_insert(Value::String(name));
    record.entry("description").or_insert_with(|| json!(""));
    record.entry("priority").or_insert_with(|| json!("normal"));
    public_record(Value::Object(record))
}

fn public_record(mut record: Value) -> Value {
    let icon = record
        .get("icon")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    // Python's public activity projection deliberately does not use emoji fallback.
    record
        .as_object_mut()
        .expect("activity record object")
        .insert(
            "icon_svg".to_owned(),
            icons::svg(icon.as_deref(), "")
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
    record
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use crate::test_support::{populated_root, shell_router};

    #[tokio::test]
    async fn ac8_work_life_activities_are_checked_by_id_and_icon_shape() {
        let root = populated_root();
        let response = shell_router(root.path())
            .oneshot(
                Request::get("/app/settings/api/facet/work-life/activities")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), 200);
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("JSON");
        let activities = body["activities"].as_array().expect("activities");
        assert_eq!(activities.len(), 5);
        let by_id = |id: &str| {
            activities
                .iter()
                .find(|activity| activity["id"] == id)
                .expect("activity id")
        };
        assert!(
            by_id("deep_work")["icon_svg"]
                .as_str()
                .expect("SVG")
                .contains("<svg")
        );
        assert!(by_id("standup")["icon_svg"].is_null());
        for id in ["meeting", "email", "messaging"] {
            let _ = by_id(id);
        }
    }

    #[tokio::test]
    async fn custom_activity_slugs_match_python_boundaries() {
        let root = crate::test_support::populated_root();
        let router = shell_router(root.path());
        for (name, expected_id) in [("  Deep Work  ", "deep_work"), ("!!!", "activity")] {
            let response = router
                .clone()
                .oneshot(
                    Request::post("/app/settings/api/facet/work-life/activities")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            serde_json::to_vec(&json!({"name": name})).expect("JSON"),
                        ))
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), 201);
            let body: Value = serde_json::from_slice(
                &to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("body"),
            )
            .expect("JSON");
            assert_eq!(body["activity"]["id"], expected_id);
        }
    }

    async fn send(router: &axum::Router, request: Request<Body>) -> (u16, Value) {
        let response = router.clone().oneshot(request).await.expect("response");
        let status = response.status().as_u16();
        let body = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("JSON");
        (status, body)
    }

    #[tokio::test]
    async fn attached_default_reads_back_with_its_name_icon_and_priority() {
        let root = populated_root();
        let router = shell_router(root.path());
        let url = "/app/settings/api/facet/work-life/activities";
        let (status, added) = send(
            &router,
            Request::post(url)
                .header("content-type", "application/json")
                .body(Body::from(r#"{"id":"coding"}"#))
                .expect("request"),
        )
        .await;
        assert_eq!(status, 201);
        assert!(
            added["activity"]["name"]
                .as_str()
                .is_some_and(|name| !name.is_empty())
        );
        for priority in ["high", "normal"] {
            let (status, _) = send(
                &router,
                Request::put(format!("{url}/coding"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({"priority": priority})).expect("JSON"),
                    ))
                    .expect("request"),
            )
            .await;
            assert_eq!(status, 200);
            let (_, body) = send(
                &router,
                Request::get(url).body(Body::empty()).expect("request"),
            )
            .await;
            let coding = body["activities"]
                .as_array()
                .expect("activities")
                .iter()
                .find(|activity| activity["id"] == "coding")
                .expect("attached default")
                .clone();
            assert!(coding["name"].as_str().is_some_and(|name| !name.is_empty()));
            assert!(
                coding["description"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty())
            );
            assert!(
                coding["icon_svg"]
                    .as_str()
                    .is_some_and(|svg| svg.contains("<svg"))
                    || coding["emoji"]
                        .as_str()
                        .is_some_and(|emoji| !emoji.is_empty())
            );
            assert_eq!(coding["priority"], priority);
            assert_eq!(coding["custom"], false);
        }
    }

    // A facet's priority on a default it shows but doesn't store.

    fn facet_root(facets: &[&str]) -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("journal");
        for facet in facets {
            let dir = root.path().join("facets").join(facet);
            std::fs::create_dir_all(&dir).expect("facet");
            std::fs::write(dir.join("facet.json"), r#"{"title":"Facet"}"#).expect("facet.json");
        }
        root
    }

    fn rows_file(root: &tempfile::TempDir, facet: &str) -> std::path::PathBuf {
        root.path()
            .join("facets")
            .join(facet)
            .join("activities/activities.jsonl")
    }

    fn stored(root: &tempfile::TempDir, facet: &str) -> Option<String> {
        std::fs::read_to_string(rows_file(root, facet)).ok()
    }

    async fn shown(router: &axum::Router, facet: &str) -> Vec<Value> {
        let (status, body) = send(
            router,
            Request::get(format!("/app/settings/api/facet/{facet}/activities"))
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        assert_eq!(status, 200);
        body["activities"].as_array().expect("activities").clone()
    }

    async fn put(router: &axum::Router, facet: &str, id: &str, body: Value) -> (u16, Value) {
        send(
            router,
            Request::put(format!("/app/settings/api/facet/{facet}/activities/{id}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).expect("JSON")))
                .expect("request"),
        )
        .await
    }

    async fn attach(router: &axum::Router, facet: &str, id: &str) {
        let (status, _) = send(
            router,
            Request::post(format!("/app/settings/api/facet/{facet}/activities"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({"id": id})).expect("JSON"),
                ))
                .expect("request"),
        )
        .await;
        assert_eq!(status, 201);
    }

    fn ids(records: &[Value]) -> Vec<&str> {
        records
            .iter()
            .map(|record| record["id"].as_str().expect("id"))
            .collect()
    }

    /// The same records, but for `id`'s priority.
    fn same_but(before: &[Value], after: &[Value], id: &str, priority: &str) {
        assert_eq!(ids(before), ids(after), "the same activities, in order");
        for (was, now) in before.iter().zip(after) {
            if was["id"] == id {
                let mut expected = was.clone();
                expected["priority"] = json!(priority);
                assert_eq!(now, &expected, "{id}");
            } else {
                assert_eq!(now, was, "{}", was["id"]);
            }
        }
    }

    #[tokio::test]
    async fn a_default_s_priority_is_kept_in_a_facet_that_stores_none() {
        for id in ["coding", "meeting"] {
            let root = facet_root(&["home"]);
            let router = crate::routes(root.path().to_path_buf());
            let before = shown(&router, "home").await;
            assert_eq!(before.len(), 26);
            let (status, body) = put(&router, "home", id, json!({"priority": "high"})).await;
            assert_eq!(status, 200, "{id}: {body}");
            assert_eq!(body["activity"]["priority"], "high");
            let after = shown(&router, "home").await;
            same_but(&before, &after, id, "high");
            assert_eq!(
                body["activity"],
                after[ids(&after).iter().position(|x| *x == id).unwrap()]
            );
            let lines = stored(&root, "home").expect("stored");
            assert_eq!(
                lines.lines().count(),
                26,
                "{id}: the facet stores what it shows"
            );
        }
    }

    #[tokio::test]
    async fn an_always_on_default_s_priority_is_kept_in_a_facet_with_rows_and_keeps_its_place() {
        let root = facet_root(&["home"]);
        let router = crate::routes(root.path().to_path_buf());
        attach(&router, "home", "coding").await;
        let before = shown(&router, "home").await;
        assert_eq!(ids(&before), ["coding", "meeting", "email", "messaging"]);
        let (status, _) = put(&router, "home", "email", json!({"priority": "high"})).await;
        assert_eq!(status, 200);
        same_but(&before, &shown(&router, "home").await, "email", "high");
        assert_eq!(
            stored(&root, "home").expect("stored"),
            "{\"id\": \"coding\"}\n{\"id\": \"meeting\"}\n{\"id\": \"email\", \"priority\": \"high\"}\n"
        );
    }

    #[tokio::test]
    async fn normal_again_leaves_what_never_picking_leaves() {
        let root = facet_root(&["home", "quiet", "rows"]);
        let router = crate::routes(root.path().to_path_buf());
        let never = shown(&router, "home").await;
        put(&router, "home", "coding", json!({"priority": "high"})).await;
        let (status, _) = put(&router, "home", "coding", json!({"priority": "normal"})).await;
        assert_eq!(status, 200);
        assert_eq!(
            stored(&root, "home").as_deref(),
            Some(""),
            "an empty file, as no file"
        );
        assert_eq!(shown(&router, "home").await, never);

        let (status, body) = put(&router, "quiet", "coding", json!({"priority": "normal"})).await;
        assert_eq!(status, 200);
        assert_eq!(body["activity"]["priority"], "normal");
        assert_eq!(stored(&root, "quiet"), None, "nothing written");

        attach(&router, "rows", "coding").await;
        let before = shown(&router, "rows").await;
        put(&router, "rows", "email", json!({"priority": "low"})).await;
        put(&router, "rows", "email", json!({"priority": "normal"})).await;
        assert_eq!(shown(&router, "rows").await, before);
        assert_eq!(
            stored(&root, "rows").expect("stored"),
            "{\"id\": \"coding\"}\n{\"id\": \"meeting\"}\n{\"id\": \"email\"}\n"
        );
    }

    #[tokio::test]
    async fn what_a_facet_doesn_t_show_still_answers_not_found_and_stores_nothing() {
        let root = facet_root(&["home", "rows"]);
        let router = crate::routes(root.path().to_path_buf());
        attach(&router, "rows", "coding").await;
        let rows_before = stored(&root, "rows");
        for (facet, id, body) in [
            ("home", "no_such_activity", json!({"priority": "high"})),
            ("rows", "no_such_activity", json!({"priority": "high"})),
            ("rows", "writing", json!({"priority": "high"})),
            ("home", "coding", json!({"description": "mine"})),
            (
                "home",
                "coding",
                json!({"priority": "high", "description": "mine"}),
            ),
        ] {
            let (status, answer) = put(&router, facet, id, body.clone()).await;
            assert_eq!(status, 404, "{facet} {id} {body}");
            assert_eq!(answer["reason_code"], "activity_not_found");
        }
        assert_eq!(stored(&root, "home"), None);
        assert_eq!(stored(&root, "rows"), rows_before);
    }

    #[tokio::test]
    async fn a_stored_activity_s_update_writes_as_before() {
        let root = facet_root(&["rows"]);
        let router = crate::routes(root.path().to_path_buf());
        attach(&router, "rows", "coding").await;
        put(&router, "rows", "coding", json!({"priority": "low"})).await;
        assert_eq!(
            stored(&root, "rows").expect("stored"),
            "{\"id\": \"coding\", \"priority\": \"low\"}\n"
        );
        put(&router, "rows", "coding", json!({"priority": "normal"})).await;
        assert_eq!(
            stored(&root, "rows").expect("stored"),
            "{\"id\": \"coding\"}\n"
        );
    }

    #[tokio::test]
    async fn the_record_store_allows_the_same_activities_after_the_facet_stores_what_it_shows() {
        let root = facet_root(&["home"]);
        let router = crate::routes(root.path().to_path_buf());
        let mut candidates = ids(&shown(&router, "home").await)
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        candidates.push("no_such_activity".to_owned());
        let allowed = |root: &tempfile::TempDir| {
            candidates
                .iter()
                .map(|id| {
                    solstone_core_facets::activity_is_available(root.path(), "home", id)
                        .expect("read")
                })
                .collect::<Vec<_>>()
        };
        let before = allowed(&root);
        put(&router, "home", "travel", json!({"priority": "high"})).await;
        assert_eq!(allowed(&root), before);
        assert_eq!(before.iter().filter(|x| **x).count(), 26);
    }

    #[test]
    fn settings_defaults_and_the_record_store_s_defaults_agree() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../../fixtures/activity_defaults.json"))
                .expect("fixture");
        let theirs = fixture["activities"]
            .as_array()
            .expect("activities")
            .iter()
            .map(|record| {
                (
                    record["id"].clone(),
                    record["always_on"].as_bool() == Some(true),
                )
            })
            .collect::<Vec<_>>();
        let ours = super::raw_default_records()
            .iter()
            .map(|record| {
                (
                    record["id"].clone(),
                    record["always_on"].as_bool() == Some(true),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(ours, theirs);
    }
}
