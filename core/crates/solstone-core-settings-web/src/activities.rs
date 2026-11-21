// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_convey_http::window_catalog::{ENGLISH, resolve_table, text};

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

pub async fn defaults(locale: &str) -> Response {
    json_response(json!({"activities": default_records(locale)}))
}

pub async fn for_facet(
    locale: &str,
    journal_root: PathBuf,
    Path(facet_name): Path<String>,
) -> Response {
    if facets::facet(&journal_root, &facet_name).is_none() {
        return facet_not_found(locale);
    }
    let defaults = default_records(locale);
    let mut activities =
        match solstone_core_facets::read_activity_definitions(&journal_root, &facet_name) {
            Ok(rows) => rows
                .into_iter()
                .map(|row| attached_record(locale, row))
                .collect::<Vec<_>>(),
            Err(_) => return activity_settings_unavailable(locale),
        };
    if activities.is_empty() {
        activities = defaults.iter().cloned().map(default_for_facet).collect();
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
    for default in defaults
        .iter()
        .filter(|activity| activity["always_on"].as_bool() == Some(true))
        .cloned()
    {
        if !existing.iter().any(|id| default["id"].as_str() == Some(id)) {
            activities.push(default_for_facet(default));
        }
    }
    json_response(json!({"activities": activities, "defaults": defaults}))
}

pub async fn add(
    locale: &str,
    journal_root: PathBuf,
    Path(facet_name): Path<String>,
    body: Bytes,
) -> Response {
    if facets::facet(&journal_root, &facet_name).is_none() {
        return facet_not_found(locale);
    }
    let JsonBody::Value(Value::Object(data)) = json_body(body) else {
        return missing_request_body(locale);
    };
    if data.is_empty() {
        return missing_request_body(locale);
    }
    let id = data
        .get("id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| data.get("name").and_then(Value::as_str).map(slug))
        .unwrap_or_default();
    if id.is_empty() {
        return missing_required_field(
            locale,
            text(
                locale,
                "journal.window.settings.activities.detail.either.id.or.name.is.required",
            ),
        );
    }
    let known = raw_default_records()
        .into_iter()
        .any(|record| record["id"].as_str() == Some(&id));
    if !known && data.get("name").and_then(Value::as_str).is_none() {
        return missing_required_field(
            locale,
            text(
                locale,
                "journal.window.settings.activities.detail.name.is.required.for.custom.activities",
            ),
        );
    }
    if data
        .get("priority")
        .is_some_and(|value| !matches!(value.as_str(), Some("high" | "normal" | "low")))
    {
        return invalid_config_value(
            locale,
            text(
                locale,
                "journal.window.settings.activities.detail.invalid.priority",
            ),
        );
    }
    if let Some(icon) = data.get("icon")
        && (!icon.is_string()
            || (!icon.as_str().is_some_and(str::is_empty) && icons::svg(icon.as_str()).is_none()))
    {
        return invalid_config_value(
            locale,
            text(
                locale,
                "journal.window.settings.activities.detail.icon.must.be.a.lucide.name",
            ),
        );
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
    // On a facet that shows rows it doesn't store, the facet stores what it
    // shows first: a default it shows takes its own place, and anything new
    // goes after what the facet stores.
    match solstone_core_facets::add_activity_attaching(
        &journal_root,
        &facet_name,
        Value::Object(row),
        |rows| drawn_to_store(rows, &id),
        |rows, attached| never_written(rows, attached, false),
    ) {
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
                return settings_operation_failed(locale);
            }
            let mut response =
                json_response(json!({"success":true,"activity":attached_record(locale, activity)}));
            *response.status_mut() = StatusCode::CREATED;
            response
        }
        Err(_) => settings_operation_failed(locale),
    }
}

pub async fn update(
    locale: &str,
    journal_root: PathBuf,
    Path((facet_name, activity_id)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    if facets::facet(&journal_root, &facet_name).is_none() {
        return facet_not_found(locale);
    };
    let JsonBody::Value(Value::Object(data)) = json_body(body) else {
        return missing_request_body(locale);
    };
    if data.is_empty() {
        return missing_request_body(locale);
    }
    if data
        .get("priority")
        .is_some_and(|value| !matches!(value.as_str(), Some("high" | "normal" | "low")))
    {
        return invalid_config_value(
            locale,
            text(
                locale,
                "journal.window.settings.activities.detail.invalid.priority",
            ),
        );
    }
    if let Some(icon) = data.get("icon")
        && (!icon.is_string()
            || (!icon.as_str().is_some_and(str::is_empty) && icons::svg(icon.as_str()).is_none()))
    {
        return invalid_config_value(
            locale,
            text(
                locale,
                "journal.window.settings.activities.detail.icon.must.be.a.lucide.name",
            ),
        );
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
    // A change to a default the facet shows but doesn't store: the facet
    // stores what it shows first, so nothing it shows goes (`drawn_to_store`).
    let priority_only = updates.len() == 1 && updates.contains_key("priority");
    let result = solstone_core_facets::update_activity_attaching(
        &journal_root,
        &facet_name,
        &activity_id,
        &updates,
        |rows| drawn_to_store(rows, &activity_id),
        |rows, attached| never_written(rows, attached, priority_only),
    );
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
                return settings_operation_failed(locale);
            }
            json_response(json!({"success":true,"activity":attached_record(locale, activity)}))
        }
        Ok(None) => activity_not_found(locale),
        Err(_) => settings_operation_failed(locale),
    }
}

pub async fn remove(
    locale: &str,
    journal_root: PathBuf,
    Path((facet_name, activity_id)): Path<(String, String)>,
    _body: Bytes,
) -> Response {
    if facets::facet(&journal_root, &facet_name).is_none() {
        return facet_not_found(locale);
    };
    if always_on_records()
        .iter()
        .any(|row| row["id"].as_str() == Some(&activity_id))
    {
        return crate::http::activity_protected(locale);
    }
    match solstone_core_facets::remove_activity_attaching(
        &journal_root,
        &facet_name,
        &activity_id,
        |rows| drawn_to_store(rows, &activity_id),
        |rows, attached| never_written(rows, attached, false),
    ) {
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
                return settings_operation_failed(locale);
            }
            json_response(json!({"success":true}))
        }
        Ok(false) => activity_not_found(locale),
        Err(_) => settings_operation_failed(locale),
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

/// The rows a facet stores before a write to `id`, which it doesn't store, so
/// that it shows the same activities in the same order afterwards: the one
/// rule every activity write takes. With no rows it shows every default: all
/// of them, as bare rows, whatever `id` is (a new activity goes after them; an
/// id it doesn't show finds no row, and nothing is written). With rows it
/// shows them and then each `always_on` default they lack, in the defaults'
/// order: those up to and including `id`, and none when `id` isn't one.
fn drawn_to_store(rows: &[Value], id: &str) -> Vec<Value> {
    let bare = |record: &Value| json!({"id": record["id"]});
    if rows.is_empty() {
        return raw_default_records().iter().map(bare).collect();
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

/// "The same as never written": a write that leaves every default, bare, in
/// the defaults' order stores what a facet that never wrote stores, so the
/// facet follows the defaults as they change. It reaches a write that stored
/// what the facet shows (`attached`), and a priority pick, whose Normal puts
/// back an earlier pick's attach. A stored activity's own write is as stored.
fn never_written(rows: &[Value], attached: bool, priority_only: bool) -> bool {
    (attached || priority_only) && only_bare_defaults(rows)
}

/// Rows that are every default, bare, in the defaults' order: a facet that
/// stores them shows what one that stores nothing shows.
fn only_bare_defaults(rows: &[Value]) -> bool {
    let defaults = raw_default_records();
    rows.len() == defaults.len()
        && rows.iter().zip(&defaults).all(|(row, record)| {
            row.as_object()
                .is_some_and(|row| row.len() == 1 && row.get("id") == record.get("id"))
        })
}

fn default_records(locale: &str) -> Vec<Value> {
    stored_default_records(locale)
        .into_iter()
        .map(public_record)
        .collect()
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
    default_records(ENGLISH)
}

fn stored_default_records(locale: &str) -> Vec<Value> {
    let mut records = serde_json::from_str::<Vec<Value>>(default_activities::JSON)
        .expect("generated default activities");
    for record in &mut records {
        resolve_table(locale, &mut record["name"]);
        // The editor and writers still receive the source description.
        resolve_table(ENGLISH, &mut record["description"]);
    }
    records
}

/// Project a stored facet row for the page. Attaching a default stores only its
/// id and the owner's overrides, so the default's name, description and icons
/// are filled back in, and a missing priority reads as normal.
fn attached_record(locale: &str, row: Value) -> Value {
    let Value::Object(stored) = row else {
        return public_record(row);
    };
    let id = stored.get("id").and_then(Value::as_str).unwrap_or_default();
    let default = stored_default_records(locale)
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
    record
        .as_object_mut()
        .expect("activity record object")
        .insert(
            "icon_svg".to_owned(),
            icons::svg(icon.as_deref())
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

    /// A facet that stores `text`, as one whose rows were added on an earlier
    /// release would (the page offers no chip on a facet that stores none).
    fn seed(root: &tempfile::TempDir, facet: &str, text: &str) {
        let path = rows_file(root, facet);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
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
        seed(&root, "home", "{\"id\": \"coding\"}\n");
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

        seed(&root, "rows", "{\"id\": \"coding\"}\n");
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
        seed(&root, "rows", "{\"id\": \"coding\"}\n");
        let rows_before = stored(&root, "rows");
        for (facet, id, body) in [
            ("home", "no_such_activity", json!({"priority": "high"})),
            ("rows", "no_such_activity", json!({"priority": "high"})),
            ("rows", "writing", json!({"priority": "high"})),
            ("home", "no_such_activity", json!({"description": "mine"})),
            ("rows", "no_such_activity", json!({"instructions": "mine"})),
            ("rows", "writing", json!({"description": "mine"})),
            (
                "rows",
                "writing",
                json!({"priority": "high", "description": "mine"}),
            ),
        ] {
            let (status, answer) = put(&router, facet, id, body.clone()).await;
            assert_eq!(status, 404, "{facet} {id} {body}");
            assert_eq!(answer["reason_code"], "activity_not_found");
        }
        for (facet, id) in [
            ("home", "no_such_activity"),
            ("rows", "no_such_activity"),
            ("rows", "writing"),
        ] {
            let (status, answer) = delete(&router, facet, id).await;
            assert_eq!(status, 404, "remove {facet} {id}");
            assert_eq!(answer["reason_code"], "activity_not_found");
        }
        assert_eq!(stored(&root, "home"), None);
        assert_eq!(stored(&root, "rows"), rows_before);
    }

    async fn delete(router: &axum::Router, facet: &str, id: &str) -> (u16, Value) {
        send(
            router,
            Request::delete(format!("/app/settings/api/facet/{facet}/activities/{id}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
    }

    async fn post(router: &axum::Router, facet: &str, body: Value) -> (u16, Value) {
        send(
            router,
            Request::post(format!("/app/settings/api/facet/{facet}/activities"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).expect("JSON")))
                .expect("request"),
        )
        .await
    }

    fn bare_defaults() -> String {
        super::raw_default_records()
            .iter()
            .map(|record| format!("{{\"id\": {}}}\n", record["id"]))
            .collect()
    }

    // A facet's other writes on a default it shows but doesn't store, and a
    // custom add there: it keeps showing what it showed.

    #[tokio::test]
    async fn a_custom_add_on_a_facet_that_stores_none_keeps_every_default_and_comes_after_them() {
        let root = facet_root(&["home"]);
        let router = crate::routes(root.path().to_path_buf());
        let before = shown(&router, "home").await;
        let (status, body) = post(
            &router,
            "home",
            json!({"name": "Gardening", "description": "the beds", "emoji": "🌱"}),
        )
        .await;
        assert_eq!(status, 201, "{body}");
        let after = shown(&router, "home").await;
        assert_eq!(after.len(), 27);
        assert_eq!(&after[..26], &before[..], "the 26, as they were, in order");
        assert_eq!(after[26]["id"], "gardening");
        assert_eq!(after[26]["name"], "Gardening");
        assert_eq!(body["activity"], after[26]);
        assert_eq!(
            stored(&root, "home").expect("stored"),
            format!(
                "{}{}\n",
                bare_defaults(),
                r#"{"id": "gardening", "custom": true, "name": "Gardening", "description": "the beds", "emoji": "🌱"}"#
            )
        );
    }

    #[tokio::test]
    async fn a_custom_named_like_a_default_on_a_facet_that_stores_none_keeps_every_default() {
        for (name, id, description) in [
            ("Reading", "reading", ""),
            ("Doctor appointment", "doctor_appointment", "mine"),
        ] {
            let root = facet_root(&["home"]);
            let router = crate::routes(root.path().to_path_buf());
            let before = shown(&router, "home").await;
            let (status, body) = post(
                &router,
                "home",
                json!({"name": name, "description": description, "emoji": "📚"}),
            )
            .await;
            assert_eq!(status, 201, "{name}: {body}");
            assert_eq!(body["activity"]["id"], id);
            let after = shown(&router, "home").await;
            assert_eq!(ids(&after), ids(&before), "{name}: the same 26, in order");
            if description.is_empty() {
                assert_eq!(stored(&root, "home"), None, "{name}: nothing to keep");
                assert_eq!(after, before);
            } else {
                let mine = after.iter().find(|a| a["id"] == id).expect("shown");
                assert_eq!(mine["description"], description);
                let expected = bare_defaults().replace(
                    &format!("{{\"id\": \"{id}\"}}\n"),
                    &format!("{{\"id\": \"{id}\", \"description\": \"{description}\"}}\n"),
                );
                assert_eq!(stored(&root, "home").expect("stored"), expected);
            }
        }
    }

    #[tokio::test]
    async fn a_default_s_description_instructions_and_remove_are_kept_in_a_facet_that_stores_none()
    {
        for (id, body) in [
            ("coding", json!({"description": "mine"})),
            ("meeting", json!({"description": "mine"})),
            ("call", json!({"instructions": "listen for names"})),
        ] {
            let root = facet_root(&["home"]);
            let router = crate::routes(root.path().to_path_buf());
            let before = shown(&router, "home").await;
            let (status, answer) = put(&router, "home", id, body.clone()).await;
            assert_eq!(status, 200, "{id} {body}: {answer}");
            let after = shown(&router, "home").await;
            assert_eq!(ids(&after), ids(&before), "{id}: the same, in order");
            for (was, now) in before.iter().zip(&after) {
                if was["id"] == id {
                    let mut expected = was.clone();
                    for (key, value) in body.as_object().unwrap() {
                        expected[key] = value.clone();
                    }
                    assert_eq!(now, &expected);
                    assert_eq!(&answer["activity"], now);
                } else {
                    assert_eq!(now, was, "{}", was["id"]);
                }
            }
            assert_eq!(stored(&root, "home").expect("stored").lines().count(), 26);
        }
        let root = facet_root(&["home"]);
        let router = crate::routes(root.path().to_path_buf());
        let before = shown(&router, "home").await;
        let (status, _) = delete(&router, "home", "travel").await;
        assert_eq!(status, 200);
        let after = shown(&router, "home").await;
        let expected = before
            .iter()
            .filter(|a| a["id"] != "travel")
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(after, expected, "the rest, as they were, in order");
        assert_eq!(
            stored(&root, "home").expect("stored"),
            bare_defaults().replace("{\"id\": \"travel\"}\n", "")
        );
        let (status, answer) = delete(&router, "home", "meeting").await;
        assert_eq!(status, 400);
        assert_eq!(answer["reason_code"], "activity_protected");
    }

    #[tokio::test]
    async fn an_always_on_default_s_description_and_instructions_are_kept_in_a_facet_with_rows() {
        for (id, body, file) in [
            (
                "email",
                json!({"description": "mine"}),
                "{\"id\": \"coding\"}\n{\"id\": \"meeting\"}\n{\"id\": \"email\", \"description\": \"mine\"}\n",
            ),
            (
                "messaging",
                json!({"instructions": "mine"}),
                "{\"id\": \"coding\"}\n{\"id\": \"meeting\"}\n{\"id\": \"email\"}\n{\"id\": \"messaging\", \"instructions\": \"mine\"}\n",
            ),
        ] {
            let root = facet_root(&["rows"]);
            let router = crate::routes(root.path().to_path_buf());
            seed(&root, "rows", "{\"id\": \"coding\"}\n");
            let before = shown(&router, "rows").await;
            assert_eq!(ids(&before), ["coding", "meeting", "email", "messaging"]);
            let (status, _) = put(&router, "rows", id, body.clone()).await;
            assert_eq!(status, 200, "{id}");
            let after = shown(&router, "rows").await;
            assert_eq!(ids(&after), ids(&before), "{id}: keeps its place");
            assert_eq!(stored(&root, "rows").expect("stored"), file);
        }
        let root = facet_root(&["rows"]);
        let router = crate::routes(root.path().to_path_buf());
        seed(&root, "rows", "{\"id\": \"coding\"}\n");
        let file = stored(&root, "rows");
        let (status, answer) = delete(&router, "rows", "email").await;
        assert_eq!(status, 400);
        assert_eq!(answer["reason_code"], "activity_protected");
        assert_eq!(stored(&root, "rows"), file);
    }

    #[tokio::test]
    async fn a_write_that_leaves_a_facet_as_never_written_stores_nothing() {
        let root = facet_root(&["home"]);
        let router = crate::routes(root.path().to_path_buf());
        let never = shown(&router, "home").await;
        for (id, body) in [
            ("coding", json!({"description": ""})),
            ("call", json!({"instructions": ""})),
            ("meeting", json!({"priority": "normal"})),
            ("travel", json!({"name": "Trips", "emoji": "x"})),
        ] {
            let (status, answer) = put(&router, "home", id, body.clone()).await;
            assert_eq!(status, 200, "{id} {body}: {answer}");
            assert_eq!(stored(&root, "home"), None, "{id} {body}: nothing written");
        }
        let (status, _) = post(&router, "home", json!({"id": "reading"})).await;
        assert_eq!(status, 201);
        assert_eq!(
            stored(&root, "home"),
            None,
            "a default's add: nothing written"
        );
        assert_eq!(shown(&router, "home").await, never);
        // Once stored, an edit put back leaves what never editing leaves.
        put(&router, "home", "coding", json!({"description": "mine"})).await;
        assert_eq!(stored(&root, "home").expect("stored").lines().count(), 26);
        put(&router, "home", "coding", json!({"priority": "high"})).await;
        put(&router, "home", "coding", json!({"priority": "normal"})).await;
        assert!(stored(&root, "home").is_some_and(|text| !text.is_empty()));
    }

    #[tokio::test]
    async fn a_stored_activity_s_add_and_remove_write_as_before() {
        let root = facet_root(&["rows"]);
        let router = crate::routes(root.path().to_path_buf());
        let path = rows_file(&root, "rows");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = format!(
            "\n{}{}\n\n",
            bare_defaults(),
            r#"{"id":"x","custom":true,"name":"X","description":""}"#
        );
        std::fs::write(&path, &file).unwrap();
        // A stored id's add answers it and writes nothing.
        for body in [
            json!({"id": "coding"}),
            json!({"name": "X", "description": "new"}),
        ] {
            let (status, _) = post(&router, "rows", body).await;
            assert_eq!(status, 201);
            assert_eq!(stored(&root, "rows").as_deref(), Some(file.as_str()));
        }
        // A custom's remove leaves the 26 stored, as before.
        let (status, _) = delete(&router, "rows", "x").await;
        assert_eq!(status, 200);
        assert_eq!(stored(&root, "rows").expect("stored"), bare_defaults());
        // A removed default's chip adds it at the end of the rows.
        delete(&router, "rows", "travel").await;
        attach(&router, "rows", "travel").await;
        let ids_now = ids(&shown(&router, "rows").await)
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert_eq!(ids_now.last().map(String::as_str), Some("travel"));
        assert_eq!(stored(&root, "rows").expect("stored").lines().count(), 26);
        // A stored default's description edit writes as before.
        put(&router, "rows", "travel", json!({"description": ""})).await;
        assert_eq!(stored(&root, "rows").expect("stored").lines().count(), 26);
    }

    #[tokio::test]
    async fn the_record_store_allows_what_the_facet_shows_after_each_write() {
        let candidates = super::raw_default_records()
            .iter()
            .map(|record| record["id"].as_str().unwrap().to_owned())
            .chain(["gardening".to_owned(), "no_such_activity".to_owned()])
            .collect::<Vec<_>>();
        let allowed = |root: &tempfile::TempDir| {
            candidates
                .iter()
                .filter(|id| {
                    solstone_core_facets::activity_is_available(root.path(), "home", id)
                        .expect("read")
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        let root = facet_root(&["home"]);
        let router = crate::routes(root.path().to_path_buf());
        let defaults = allowed(&root);
        assert_eq!(defaults.len(), 26);
        post(&router, "home", json!({"name": "Gardening"})).await;
        let mut plus = defaults.clone();
        plus.push("gardening".to_owned());
        assert_eq!(allowed(&root), plus, "a custom add");
        let root = facet_root(&["home"]);
        let router = crate::routes(root.path().to_path_buf());
        put(&router, "home", "coding", json!({"description": "mine"})).await;
        assert_eq!(allowed(&root), defaults, "a description");
        let root = facet_root(&["home"]);
        let router = crate::routes(root.path().to_path_buf());
        delete(&router, "home", "travel").await;
        let less = defaults
            .iter()
            .filter(|id| *id != "travel")
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(allowed(&root), less, "a remove");
    }

    #[tokio::test]
    async fn a_stored_activity_s_update_writes_as_before() {
        let root = facet_root(&["rows"]);
        let router = crate::routes(root.path().to_path_buf());
        seed(&root, "rows", "{\"id\": \"coding\"}\n");
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
