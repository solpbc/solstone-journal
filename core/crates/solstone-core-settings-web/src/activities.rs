// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;

use axum::{extract::Path, response::Response};
use serde_json::{Value, json};

use crate::{
    facets,
    http::{facet_not_found, json_response},
    icons,
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
