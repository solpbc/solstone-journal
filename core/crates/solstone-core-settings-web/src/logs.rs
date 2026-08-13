// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use axum::{
    extract::{Path as AxumPath, Query},
    response::Response,
};
use serde_json::{Value, json};

use crate::http::json_response;

pub async fn journal(
    journal_root: PathBuf,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    json_response(read_logs(
        &journal_root.join("config/actions"),
        query.get("cursor").map(String::as_str),
    ))
}

pub async fn facet(
    journal_root: PathBuf,
    AxumPath(facet_name): AxumPath<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    json_response(read_logs(
        &journal_root.join("facets").join(facet_name).join("logs"),
        query.get("cursor").map(String::as_str),
    ))
}

fn read_logs(directory: &Path, cursor: Option<&str>) -> Value {
    let Ok(entries) = fs::read_dir(directory) else {
        return json!({"day": null, "entries": [], "next_cursor": null});
    };
    let mut files = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            valid_log_name(&name).then(|| (name[..8].to_owned(), entry.path()))
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| right.0.cmp(&left.0));
    if let Some(cursor) = cursor {
        files.retain(|(day, _)| day.as_str() < cursor);
    }
    let Some((day, path)) = files.first() else {
        return json!({"day": null, "entries": [], "next_cursor": null});
    };
    let mut rows = fs::read_to_string(path)
        .ok()
        .map(|text| {
            text.lines()
                .filter(|line| !line.trim().is_empty())
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    rows.reverse();
    json!({"day": day, "entries": rows, "next_cursor": files.get(1).map(|(next, _)| next)})
}

fn valid_log_name(name: &str) -> bool {
    name.len() == 14
        && name.ends_with(".jsonl")
        && name[..8].bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use axum::{body::to_bytes, http::Request};
    use serde_json::Value;
    use tower::ServiceExt;

    use crate::test_support::{populated_root, shell_router};

    #[tokio::test]
    async fn ac9_journal_and_facet_log_paging_match_measured_table() {
        let root = populated_root();
        let router = shell_router(root.path());
        for (path, day, actions, next) in [
            (
                "/app/settings/api/logs",
                Some("20260813"),
                vec!["identity_update"],
                Some("20260812"),
            ),
            (
                "/app/settings/api/logs?cursor=20260899",
                Some("20260813"),
                vec!["identity_update"],
                Some("20260812"),
            ),
            (
                "/app/settings/api/logs?cursor=20260812",
                Some("20260811"),
                vec!["probe_2", "probe_1", "probe_0"],
                Some("20260810"),
            ),
            (
                "/app/settings/api/logs?cursor=20260811",
                Some("20260810"),
                vec!["probe_1", "probe_0"],
                None,
            ),
            ("/app/settings/api/logs?cursor=20260810", None, vec![], None),
            (
                "/app/settings/api/facet/work-life/logs",
                Some("20260813"),
                vec!["activity_add", "activity_add", "facet_create"],
                Some("20260812"),
            ),
            (
                "/app/settings/api/facet/work-life/logs?cursor=20260814",
                Some("20260813"),
                vec!["activity_add", "activity_add", "facet_create"],
                Some("20260812"),
            ),
            (
                "/app/settings/api/facet/work-life/logs?cursor=20260812",
                Some("20260811"),
                vec!["probe_2", "probe_1", "probe_0"],
                Some("20260810"),
            ),
            (
                "/app/settings/api/facet/work-life/logs?cursor=20260810",
                None,
                vec![],
                None,
            ),
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
            let body: Value = serde_json::from_slice(
                &to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("body"),
            )
            .expect("JSON");
            assert_eq!(body["day"].as_str(), day);
            assert_eq!(body["next_cursor"].as_str(), next);
            assert_eq!(
                body["entries"]
                    .as_array()
                    .expect("entries")
                    .iter()
                    .filter_map(|row| row["action"].as_str())
                    .collect::<Vec<_>>(),
                actions
            );
        }
    }
}
