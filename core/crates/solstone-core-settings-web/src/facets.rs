// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use axum::{
    extract::{Path, Query},
    response::Response,
};
use serde_json::{Value, json};

use crate::{
    http::{facet_not_found, json_response},
    icons,
};

pub async fn list(journal_root: PathBuf, Query(query): Query<HashMap<String, String>>) -> Response {
    let include_all = matches!(query.get("all").map(String::as_str), Some("true" | "1"));
    let mut rows = facet_entries(&journal_root);
    if !include_all {
        rows.retain(|(_, config)| {
            !config
                .get("muted")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        });
    }
    rows.sort_by_key(|(name, config)| {
        config
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or(name)
            .to_ascii_lowercase()
    });
    json_response(
        json!({"facets": rows.into_iter().map(|(name, config)| public_record(&name, &config)).collect::<Vec<_>>() }),
    )
}

pub async fn muted(journal_root: PathBuf) -> Response {
    let facets = facet_entries(&journal_root)
        .into_iter()
        .filter(|(_, config)| {
            config
                .get("muted")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .map(|(name, config)| public_record(&name, &config))
        .collect::<Vec<_>>();
    json_response(json!({"facets": facets}))
}

pub async fn get_one(journal_root: PathBuf, Path(facet_name): Path<String>) -> Response {
    let Some(config) = facet(&journal_root, &facet_name) else {
        return facet_not_found();
    };
    json_response(json!({"facet": facet_name, "config": config}))
}

pub(crate) fn facet(journal_root: &std::path::Path, name: &str) -> Option<Value> {
    let path = journal_root.join("facets").join(name).join("facet.json");
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn facet_entries(journal_root: &std::path::Path) -> Vec<(String, Value)> {
    let Ok(entries) = fs::read_dir(journal_root.join("facets")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            facet(journal_root, &name).map(|config| (name, config))
        })
        .collect()
}

fn public_record(name: &str, config: &Value) -> Value {
    let values = config.as_object().cloned().unwrap_or_default();
    let emoji = values.get("emoji").and_then(Value::as_str).unwrap_or("");
    let icon = values.get("icon").and_then(Value::as_str);
    json!({
        "name": name,
        "title": values.get("title").and_then(Value::as_str).unwrap_or(name),
        "color": values.get("color").and_then(Value::as_str).unwrap_or(""),
        "emoji": emoji,
        "icon": icon.unwrap_or(""),
        "icon_svg": icons::svg(icon, emoji),
        "muted": values.get("muted").and_then(Value::as_bool).unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use axum::{body::to_bytes, http::Request};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use crate::test_support::{populated_root, shell_router};

    #[tokio::test]
    async fn ac7_populated_facet_route_name_sets_are_distinct() {
        let root = populated_root();
        let router = shell_router(root.path());
        for (path, expected) in [
            (
                "/app/settings/api/facets",
                vec!["work-life", "zeta-project"],
            ),
            (
                "/app/settings/api/facets?all=true",
                vec!["muted-thing", "work-life", "zeta-project"],
            ),
            ("/app/settings/api/facets/muted", vec!["muted-thing"]),
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::get(path)
                        .body(axum::body::Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            let value: Value = serde_json::from_slice(
                &to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("body"),
            )
            .expect("JSON");
            let mut names = value["facets"]
                .as_array()
                .expect("facets")
                .iter()
                .filter_map(|row| row["name"].as_str())
                .collect::<Vec<_>>();
            names.sort_unstable();
            let mut expected = expected;
            expected.sort_unstable();
            assert_eq!(names, expected);
        }
    }

    #[tokio::test]
    async fn ac10_absent_facet_logs_are_empty_but_facet_and_activities_are_not_found() {
        let root = populated_root();
        let router = shell_router(root.path());
        let logs = router
            .clone()
            .oneshot(
                Request::get("/app/settings/api/facet/no-such-facet/logs")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(logs.status(), 200);
        let body: Value =
            serde_json::from_slice(&to_bytes(logs.into_body(), usize::MAX).await.expect("body"))
                .expect("JSON");
        assert_eq!(
            body,
            json!({"day": null, "entries": [], "next_cursor": null})
        );
        for path in [
            "/app/settings/api/facet/no-such-facet",
            "/app/settings/api/facet/no-such-facet/activities",
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::get(path)
                        .body(axum::body::Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), 404);
            let body: Value = serde_json::from_slice(
                &to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("body"),
            )
            .expect("JSON");
            assert_eq!(body["reason_code"], "facet_not_found");
        }
    }
}
