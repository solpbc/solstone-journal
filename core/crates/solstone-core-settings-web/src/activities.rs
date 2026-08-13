// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;

use axum::{body::Bytes, extract::Path, http::StatusCode, response::Response};
use serde_json::{Map, Value, json};

use crate::{
    facets,
    http::{
        activity_not_found, facet_not_found, invalid_config_value, json_response,
        missing_request_body, missing_required_field, settings_operation_failed,
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
    let mut activities = read_attached(&journal_root, &facet_name);
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
                json_response(json!({"success":true,"activity":public_record(activity)}));
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
    match solstone_core_facets::update_activity(&journal_root, &facet_name, &activity_id, &updates)
    {
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
            json_response(json!({"success":true,"activity":public_record(activity)}))
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
    value
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
        .trim_matches('-')
        .to_owned()
}

fn read_attached(journal_root: &std::path::Path, facet_name: &str) -> Vec<Value> {
    let path = journal_root
        .join("facets")
        .join(facet_name)
        .join("activities/activities.jsonl");
    fs::read_to_string(path)
        .ok()
        .map(|text| {
            text.lines()
                .filter(|line| !line.trim().is_empty())
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .map(public_record)
                .collect()
        })
        .unwrap_or_default()
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
    serde_json::from_str::<Vec<Value>>(default_activities::JSON)
        .expect("generated default activities")
        .into_iter()
        .map(public_record)
        .collect()
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
    use axum::{body::to_bytes, http::Request};
    use serde_json::Value;
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
}
