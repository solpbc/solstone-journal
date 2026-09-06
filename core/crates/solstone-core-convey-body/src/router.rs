// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Path as RoutePath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::error_envelope;

use crate::trends::trends_response;
use crate::{BodyStoreHealthVerdict, read_body_store_health, read_health_dedupe_stats};

/// Build the mergeable read-only Body API route surface.
pub fn api_router(journal_root: impl AsRef<Path>) -> Router {
    Router::new()
        .route("/app/body/api/index", get(index_route))
        .route("/app/body/api/stats/{month}", get(stats_route))
        .route("/app/body/api/trends", get(trends_route))
        .route("/app/body/api/day/{day}", get(crate::day::day_route))
        .route("/app/body/api/status", get(crate::archive::status_route))
        .route("/app/body/api/recent", get(crate::archive::recent_route))
        .route("/app/body/api/window", get(crate::window::window_route))
        .with_state(Arc::new(journal_root.as_ref().to_path_buf()))
}

async fn trends_route(State(root): State<Arc<PathBuf>>) -> Response {
    Json(trends_response(root)).into_response()
}

async fn index_route(State(root): State<Arc<PathBuf>>) -> Response {
    let stats = match ready_stats(&root) {
        Ok(stats) => stats,
        Err(error) => return unavailable_response(error),
    };
    Json(index_payload(
        stats
            .as_ref()
            .map_or(&BTreeMap::new(), |stats| &stats.by_day),
    ))
    .into_response()
}

async fn stats_route(
    State(root): State<Arc<PathBuf>>,
    RoutePath(month): RoutePath<String>,
) -> Response {
    let stats = match ready_stats(&root) {
        Ok(stats) => stats,
        Err(error) => return unavailable_response(error),
    };
    if !is_month(&month) {
        return error_envelope(
            "invalid_request_value",
            "one of those values couldn't be used.",
            "Invalid month",
            StatusCode::BAD_REQUEST,
        )
        .into_response();
    }
    let days = stats
        .as_ref()
        .map(|stats| &stats.by_day)
        .into_iter()
        .flatten()
        .filter(|(day, _)| day.len() >= 6 && day[..6] == month)
        .map(|(day, count)| (day.clone(), *count))
        .collect::<BTreeMap<_, _>>();
    Json(days).into_response()
}

/// The health reader determines whether aggregate-derived serving is safe.
/// Torn/rebuilding/error states use one 503 JSON shape on both routes.
pub(crate) fn ready_stats(
    root: &Path,
) -> Result<Option<Arc<crate::HealthDedupeStats>>, StoreError> {
    match read_body_store_health(root) {
        Ok(BodyStoreHealthVerdict::Healthy(_)) | Ok(BodyStoreHealthVerdict::FirstRun(_)) => {
            read_health_dedupe_stats(root).map_err(|error| StoreError::Read(error.to_string()))
        }
        Ok(BodyStoreHealthVerdict::Rebuilding(reason))
        | Ok(BodyStoreHealthVerdict::Torn(reason)) => Err(StoreError::Verdict(reason)),
        Err(error) => Err(StoreError::Read(error.to_string())),
    }
}

#[derive(Debug)]
pub(crate) enum StoreError {
    Verdict(crate::BodyStoreHealthReason),
    Read(String),
    ShardUnreadable(String),
}

pub(crate) fn unavailable_response(error: StoreError) -> Response {
    let (reason_code, detail) = match error {
        StoreError::Verdict(reason) => (
            format!("body_store_{}", reason.as_str()),
            reason.as_str().to_owned(),
        ),
        StoreError::Read(detail) => ("body_store_unavailable".to_owned(), detail),
        StoreError::ShardUnreadable(detail) => ("body_store_shard_unreadable".to_owned(), detail),
    };
    error_envelope(
        reason_code,
        "the body store couldn't be read.",
        detail,
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .into_response()
}

fn is_month(value: &str) -> bool {
    value.len() == 6 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn index_payload(by_day: &BTreeMap<String, u64>) -> Value {
    let mut months = BTreeMap::new();
    let days = by_day
        .iter()
        .filter(|(day, total)| day.len() >= 6 && **total > 0)
        .map(|(day, total)| {
            *months.entry(day[..6].to_owned()).or_insert(0) += *total;
            day.clone()
        })
        .collect::<Vec<_>>();
    let coverage = days
        .first()
        .zip(days.last())
        .map(|(start, end)| json!({"start": start, "end": end}));
    json!({"coverage": coverage, "months": months})
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use serde_json::{Map, Value, json};
    use tower::ServiceExt;

    use super::{api_router, index_payload};
    use crate::{
        BodyAggregateSeed, BodyJournalSeed, BodySeedBundle, BodySeedManifest,
        read_health_dedupe_stats, seed_body_journal,
    };

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "solstone-convey-body-router-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("temporary root creates");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn row(day: &str, index: usize) -> Map<String, Value> {
        json!({
            "dedupe_key": format!("row-{day}-{index}"),
            "record_type": "synthetic_type",
            "start_date": format!("{}-{}-{}T00:00:00Z", &day[..4], &day[4..6], &day[6..8]),
        })
        .as_object()
        .expect("row is object")
        .clone()
    }

    fn seed(root: &Path, counts: &BTreeMap<String, u64>) {
        let mut shards = BTreeMap::<String, Vec<Map<String, Value>>>::new();
        for (day, count) in counts {
            let month = format!("{}-{}", &day[..4], &day[4..6]);
            for index in 0..*count as usize {
                shards
                    .entry(month.clone())
                    .or_default()
                    .push(row(day, index));
            }
        }
        seed_body_journal(
            root,
            &BodyJournalSeed {
                dates: BTreeSet::new(),
                day_summaries: BTreeMap::new(),
                bundles: vec![BodySeedBundle {
                    import_id: "synthetic-body".to_owned(),
                    source_family: "apple_health".to_owned(),
                    manifest: BodySeedManifest::Present {
                        source_type: Some("apple_health".to_owned()),
                        entry_count: Some(counts.values().sum()),
                        extra: Map::new(),
                    },
                    shards,
                }],
                aggregate: BodyAggregateSeed::Direct,
                journal_config: None,
            },
        )
        .expect("journal seeds");
    }

    async fn get(root: &Path, path: &str) -> (StatusCode, Value) {
        let response = api_router(root)
            .oneshot(
                Request::get(path)
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body reads");
        (
            status,
            serde_json::from_slice(&body).expect("response JSON parses"),
        )
    }

    #[test]
    fn index_fold_drops_zero_days_before_months_and_coverage() {
        let by_day = BTreeMap::from([
            ("20260101".to_owned(), 0),
            ("20260203".to_owned(), 2),
            ("20260204".to_owned(), 3),
        ]);
        assert_eq!(
            index_payload(&by_day),
            json!({
                "coverage": {"start": "20260203", "end": "20260204"},
                "months": {"202602": 5}
            })
        );
    }

    #[tokio::test]
    async fn routes_fold_seeded_days_and_first_run_is_empty() {
        let first_run = TempDir::new();
        assert_eq!(
            get(&first_run.0, "/app/body/api/index").await,
            (StatusCode::OK, json!({"coverage": null, "months": {}}))
        );
        assert_eq!(
            get(&first_run.0, "/app/body/api/stats/202608").await,
            (StatusCode::OK, json!({}))
        );

        let temporary = TempDir::new();
        seed(
            &temporary.0,
            &BTreeMap::from([("20260731".to_owned(), 2), ("20260801".to_owned(), 3)]),
        );
        assert_eq!(
            get(&temporary.0, "/app/body/api/index").await.1,
            json!({
                "coverage": {"start": "20260731", "end": "20260801"},
                "months": {"202607": 2, "202608": 3}
            })
        );
        assert_eq!(
            get(&temporary.0, "/app/body/api/stats/202608").await.1,
            json!({"20260801": 3})
        );
        assert_eq!(
            get(&temporary.0, "/app/body/api/stats/999999").await,
            (StatusCode::OK, json!({}))
        );
        assert_eq!(
            get(&temporary.0, "/app/body/api/stats/nope").await,
            (
                StatusCode::BAD_REQUEST,
                json!({
                    "error": "one of those values couldn't be used.",
                    "reason_code": "invalid_request_value",
                    "detail": "Invalid month"
                })
            )
        );
    }

    #[tokio::test]
    async fn corpus_counts_reproduce_index_and_august_stats() {
        let corpus_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("fixtures/convey_body_corpus.json");
        let corpus: Value = serde_json::from_slice(&fs::read(corpus_path).expect("corpus reads"))
            .expect("corpus parses");
        let counts = corpus["journal"]["dedupe_day_counts_by_start_date"]
            .as_object()
            .expect("day counts are object")
            .iter()
            .map(|(day, count)| (day.clone(), count.as_u64().expect("count is u64")))
            .collect::<BTreeMap<_, _>>();
        let temporary = TempDir::new();
        seed(&temporary.0, &counts);

        for (rule, path) in [
            ("/app/body/api/index", "/app/body/api/index"),
            ("/app/body/api/stats/<month>", "/app/body/api/stats/202608"),
        ] {
            let expected = corpus["cases"]["fixed"]
                .as_array()
                .expect("fixed cases are array")
                .iter()
                .find(|case| case["rule"] == rule)
                .and_then(|case| case.get("json"))
                .expect("fixture case has JSON");
            let (status, actual) = get(&temporary.0, path).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert_eq!(&actual, expected, "{path}");
        }
    }

    #[tokio::test]
    async fn torn_store_refuses_both_routes() {
        let temporary = TempDir::new();
        seed(&temporary.0, &BTreeMap::from([("20260801".to_owned(), 1)]));
        read_health_dedupe_stats(&temporary.0)
            .expect("stats read succeeds")
            .expect("aggregate exists");
        fs::remove_file(temporary.0.join("imports/health-dedupe.sqlite"))
            .expect("aggregate removes");
        for path in ["/app/body/api/index", "/app/body/api/stats/202608"] {
            assert!(!get(&temporary.0, path).await.0.is_success(), "{path}");
        }
    }
}
