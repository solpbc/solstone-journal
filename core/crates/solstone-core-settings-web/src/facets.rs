// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use axum::{
    body::Bytes,
    extract::{Path, Query},
    http::StatusCode,
    response::Response,
};
use serde_json::{Map, Value, json};

use crate::{
    http::{
        facet_not_found, invalid_config_value, invalid_request_value, json_response,
        missing_request_body, missing_required_field, settings_operation_failed,
    },
    icons,
    request_body::{JsonBody, json_body},
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
    let Some(mut config) = facet(&journal_root, &facet_name) else {
        return facet_not_found();
    };
    let config = config.as_object_mut().expect("facet config object");
    config
        .entry("muted".to_owned())
        .or_insert(Value::Bool(false));
    config.insert(
        "path".to_owned(),
        Value::String(
            journal_root
                .join("facets")
                .join(&facet_name)
                .display()
                .to_string(),
        ),
    );
    json_response(json!({"facet": facet_name, "config": config}))
}

pub async fn create(journal_root: PathBuf, body: Bytes) -> Response {
    let JsonBody::Value(Value::Object(data)) = json_body(body) else {
        return missing_request_body();
    };
    let Some(title) = data
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return missing_required_field("Title is required");
    };
    let slug = title
        .to_ascii_lowercase()
        .chars()
        .map(|value| {
            if value.is_ascii_lowercase() || value.is_ascii_digit() {
                value
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_owned();
    if slug.is_empty()
        || !slug
            .chars()
            .next()
            .is_some_and(|value| value.is_ascii_lowercase())
    {
        return invalid_request_value("Title must start with a letter.");
    }
    if facet(&journal_root, &slug).is_some() {
        return invalid_config_value("invalid or existing facet title");
    }
    let emoji = data.get("emoji").and_then(Value::as_str).unwrap_or("📦");
    let color = data
        .get("color")
        .and_then(Value::as_str)
        .unwrap_or("#667eea");
    let description = data
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let icon = data.get("icon").and_then(Value::as_str).map(str::trim);
    if data.get("icon").is_some()
        && (icon.is_none()
            || icon.is_some_and(|icon| !icon.is_empty() && icons::svg(Some(icon), "").is_none()))
    {
        return invalid_request_value("unknown Lucide icon");
    }
    let consent = data
        .get("consent")
        .is_some_and(|value| value.as_bool() == Some(true));
    match solstone_core_facets::create_facet(
        &journal_root,
        &slug,
        title,
        description,
        color,
        emoji,
        icon,
    ) {
        Ok(()) => {
            let mut params = [
                ("title".to_owned(), json!(title)),
                ("emoji".to_owned(), json!(emoji)),
                ("color".to_owned(), json!(color)),
                ("description".to_owned(), json!(description)),
            ]
            .into_iter()
            .collect::<Map<_, _>>();
            if let Some(icon) = icon.filter(|icon| !icon.is_empty()) {
                params.insert("icon".to_owned(), json!(icon));
            }
            if consent {
                params.insert("consent".to_owned(), Value::Bool(true));
            }
            if solstone_core_facets::append_action_log(
                &journal_root,
                Some(&slug),
                "call",
                "agent",
                "facet_create",
                Value::Object(params),
            )
            .is_err()
            {
                return settings_operation_failed();
            }
            let mut config = [
                ("title".to_owned(), json!(title)),
                ("description".to_owned(), json!(description)),
                ("color".to_owned(), json!(color)),
                ("emoji".to_owned(), json!(emoji)),
            ]
            .into_iter()
            .collect::<Map<_, _>>();
            if let Some(icon) = icon.filter(|icon| !icon.is_empty()) {
                config.insert("icon".to_owned(), json!(icon));
            }
            let mut response = json_response(json!({"success":true,"facet":slug,"config":config}));
            *response.status_mut() = StatusCode::CREATED;
            response
        }
        Err(_) => settings_operation_failed(),
    }
}

pub async fn update(
    journal_root: PathBuf,
    Path(facet_name): Path<String>,
    body: Bytes,
) -> Response {
    let JsonBody::Value(Value::Object(data)) = json_body(body) else {
        return missing_request_body();
    };
    if data.is_empty() {
        return missing_request_body();
    }
    let Some(current) =
        facet(&journal_root, &facet_name).and_then(|value| value.as_object().cloned())
    else {
        return facet_not_found();
    };
    let title = data
        .get("title")
        .and_then(Value::as_str)
        .or_else(|| current.get("title").and_then(Value::as_str))
        .unwrap_or(&facet_name);
    let description = data
        .get("description")
        .and_then(Value::as_str)
        .or_else(|| current.get("description").and_then(Value::as_str))
        .unwrap_or("");
    let color = data
        .get("color")
        .and_then(Value::as_str)
        .or_else(|| current.get("color").and_then(Value::as_str))
        .unwrap_or("");
    let emoji = data
        .get("emoji")
        .and_then(Value::as_str)
        .or_else(|| current.get("emoji").and_then(Value::as_str))
        .unwrap_or("");
    let icon = data
        .get("icon")
        .and_then(Value::as_str)
        .or_else(|| current.get("icon").and_then(Value::as_str));
    if data.get("icon").is_some()
        && (data.get("icon").and_then(Value::as_str).is_none()
            || data
                .get("icon")
                .and_then(Value::as_str)
                .is_some_and(|icon| !icon.is_empty() && icons::svg(Some(icon), "").is_none()))
    {
        return invalid_request_value("unknown Lucide icon");
    }
    let mut changed_fields = Map::new();
    for (key, value) in [
        ("title", data.get("title")),
        ("description", data.get("description")),
        ("color", data.get("color")),
        ("emoji", data.get("emoji")),
        ("icon", data.get("icon")),
    ] {
        if let Some(value) = value
            && current.get(key) != Some(value)
        {
            changed_fields.insert(
                key.to_owned(),
                json!({"old": current.get(key), "new": value}),
            );
        }
    }
    if solstone_core_facets::update_facet(
        &journal_root,
        &facet_name,
        title,
        description,
        color,
        emoji,
        icon,
    )
    .is_err()
    {
        return settings_operation_failed();
    }
    if !changed_fields.is_empty()
        && solstone_core_facets::append_action_log(
            &journal_root,
            Some(&facet_name),
            "call",
            "agent",
            "facet_update",
            json!({"changed_fields": changed_fields}),
        )
        .is_err()
    {
        return settings_operation_failed();
    }
    if let Some(muted) = data.get("muted").and_then(Value::as_bool)
        && solstone_core_facets::set_facet_muted(&journal_root, &facet_name, muted).is_err()
    {
        return settings_operation_failed();
    }
    if let Some(muted) = data.get("muted").and_then(Value::as_bool)
        && current
            .get("muted")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            != muted
        && solstone_core_facets::append_action_log(
            &journal_root,
            Some(&facet_name),
            "call",
            "agent",
            if muted { "facet_mute" } else { "facet_unmute" },
            json!({"muted": muted}),
        )
        .is_err()
    {
        return settings_operation_failed();
    }
    let config = facet(&journal_root, &facet_name).unwrap_or(Value::Null);
    json_response(json!({"success":true,"facet":facet_name,"config":config}))
}

pub async fn delete(
    journal_root: PathBuf,
    Path(facet_name): Path<String>,
    body: Bytes,
) -> Response {
    let JsonBody::Value(Value::Object(data)) = json_body(body) else {
        return missing_request_body();
    };
    let Some(consent) = data.get("consent") else {
        return missing_required_field("consent is required");
    };
    if consent != &Value::Bool(true) {
        return invalid_request_value("consent must be true");
    }
    if facet(&journal_root, &facet_name).is_none() {
        return facet_not_found();
    }
    if solstone_core_facets::append_action_log(
        &journal_root,
        None,
        "call",
        "agent",
        "facet_delete",
        json!({"name": facet_name, "consent": true}),
    )
    .is_err()
    {
        return settings_operation_failed();
    }
    match solstone_core_facets::delete_facet(&journal_root, &facet_name) {
        Ok(true) => json_response(json!({"success":true,"facet":facet_name})),
        Ok(false) => facet_not_found(),
        Err(_) => settings_operation_failed(),
    }
}

pub async fn rename(
    journal_root: PathBuf,
    Path(facet_name): Path<String>,
    body: Bytes,
) -> Response {
    let JsonBody::Value(Value::Object(data)) = json_body(body) else {
        return missing_request_body();
    };
    let Some(new_name) = data
        .get("new_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return missing_required_field("new_name is required");
    };
    if !data.get("consent").is_none_or(Value::is_boolean) {
        return invalid_config_value("consent must be boolean");
    }
    match solstone_core_facets::rename_facet(&journal_root, &facet_name, new_name) {
        Ok(_) => {
            let mut params = [
                ("old_name".to_owned(), json!(facet_name)),
                ("new_name".to_owned(), json!(new_name)),
            ]
            .into_iter()
            .collect::<Map<_, _>>();
            if data
                .get("consent")
                .is_some_and(|value| value.as_bool() == Some(true))
            {
                params.insert("consent".to_owned(), Value::Bool(true));
            }
            if solstone_core_facets::append_action_log(
                &journal_root,
                Some(new_name),
                "call",
                "agent",
                "facet_rename",
                Value::Object(params),
            )
            .is_err()
            {
                return settings_operation_failed();
            }
            json_response(json!({"success":true,"facet":new_name}))
        }
        Err(_) => settings_operation_failed(),
    }
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
