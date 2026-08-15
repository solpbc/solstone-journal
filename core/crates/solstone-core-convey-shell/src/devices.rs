// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native Devices workspace and observer-management routes.

use std::cmp::Reverse;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use solstone_core_facets::append_action_log;
use solstone_core_observer::store::record::ObserverRecord;
use solstone_core_observer::store::reload::{find_observer, load_observers};
use solstone_core_observer::{ObserverCommand, ObserverError, execute, system_now_ms};

use crate::JournalRoot;
use crate::asset_response;
use crate::refusal::AppNotConverted;

const ACTIVE_THRESHOLD_MS: i64 = 30_000;
const STALE_THRESHOLD_MS: i64 = 120_000;
const FUTURE_CLOCK_DRIFT_TOLERANCE_MS: i64 = 300_000;
const OBSERVER_PROTOCOL_VERSION: i64 = 2;

// Mirrors `solstone/apps/observer/routes.py` lines 122-132. Keep this native
// vocabulary explicit; build-time Python parsing is intentionally not used.
const OBSERVER_STATE_LABELS: [(&str, &str); 4] = [
    ("connected", "connected"),
    ("stale", "not reporting"),
    ("disconnected", "offline"),
    ("revoked", "removed"),
];

const OBSERVER_ENTRY_FIELDS: [&str; 17] = [
    "prefix",
    "name",
    "created_at",
    "last_seen",
    "last_segment",
    "enabled",
    "revoked",
    "revoked_at",
    "stats",
    "live",
    "last_chat_request_at",
    "state",
    "group",
    "elapsed_ms",
    "clock_skew",
    "label",
    "failing",
];

pub(crate) async fn shell() -> Response {
    asset_response("/static/shell.html")
}

pub(crate) async fn workspace() -> Response {
    asset_response("/app/devices/workspace")
}

pub(crate) async fn list(Extension(root): Extension<Arc<JournalRoot>>) -> Response {
    let now_ms = system_now_ms();
    let records = match load_observers(&root.0) {
        Ok(records) => records,
        Err(error) => {
            log::warn!("devices list could not load observer records: {error}");
            return settings_operation_failed("Failed to load observers");
        }
    };
    let mut observers = records
        .iter()
        .map(|record| observer_json(record, now_ms))
        .collect::<Vec<_>>();
    observers.sort_by_key(observer_sort_key);
    Json(json!({
        "thresholds": {"active_ms": ACTIVE_THRESHOLD_MS, "stale_ms": STALE_THRESHOLD_MS},
        "labels": {"live": "live"},
        "observers": observers,
    }))
    .into_response()
}

pub(crate) async fn delete(
    Extension(root): Extension<Arc<JournalRoot>>,
    Path(prefix): Path<String>,
) -> Response {
    let record = match find_observer(&root.0, &prefix) {
        Ok(Some(record)) => record,
        Ok(None) => return paired_device_not_found(),
        Err(error) => {
            log::warn!("devices delete could not load observer {prefix}: {error}");
            return settings_operation_failed("Failed to revoke device");
        }
    };
    if record.revoked() {
        return pl_revoked(StatusCode::CONFLICT, "Device already revoked");
    }
    match execute(
        &root.0,
        ObserverCommand::Revoke {
            identifier: prefix,
            json: true,
        },
        system_now_ms(),
    ) {
        Ok(_) => Json(json!({"status": "ok"})).into_response(),
        Err(ObserverError::AlreadyRevoked(_)) => {
            pl_revoked(StatusCode::CONFLICT, "Device already revoked")
        }
        Err(ObserverError::NotFound(_) | ObserverError::InvalidIdentifier) => {
            paired_device_not_found()
        }
        Err(error) => {
            log::warn!("devices delete could not revoke observer: {error}");
            settings_operation_failed("Failed to revoke device")
        }
    }
}

pub(crate) async fn key(
    Extension(root): Extension<Arc<JournalRoot>>,
    Path(prefix): Path<String>,
) -> Response {
    let record = match find_observer(&root.0, &prefix) {
        Ok(Some(record)) => record,
        Ok(None) => return paired_device_not_found(),
        Err(error) => {
            log::warn!("devices key could not load observer {prefix}: {error}");
            return settings_operation_failed("Failed to read device");
        }
    };
    if record.revoked() {
        return pl_revoked(StatusCode::FORBIDDEN, "key unavailable — device revoked");
    }
    let name = record.name().unwrap_or_default();
    if let Err(error) = append_action_log(
        &root.0,
        None,
        "app",
        "observer",
        "observer_key_view",
        json!({"name": name, "key_prefix": prefix}),
    ) {
        log::warn!("devices key could not append audit action: {error}");
        return settings_operation_failed("Failed to record key view");
    }
    Json(json!({
        "key": record.key(),
        "name": name,
        "ingest_url": "/app/observer/ingest",
        "protocol_version": OBSERVER_PROTOCOL_VERSION,
    }))
    .into_response()
}

pub(crate) async fn create_retired() -> Response {
    error_response(
        StatusCode::GONE,
        "operation_no_longer_available",
        "I couldn't finish because that action is no longer available.",
        "Devices are no longer created by hand. A device registers itself when you pair it.",
    )
}

// Temporary death-condition routes: remove these explicit observer wire
// refusals, and this comment, when the real native handlers are mounted. Do
// not replace them with a blanket observer wildcard. Bare `/app/observer/ingest`
// is deliberately excluded: its reference endpoint is POST-only, so there is
// no GET refusal to preserve.
pub(crate) async fn observer_wire_refusal() -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(AppNotConverted::new("observer")),
    )
        .into_response()
}

fn observer_json(record: &ObserverRecord, now_ms: i64) -> Value {
    let freshness = freshness(record.last_seen(), record.revoked(), now_ms);
    let enabled = record.enabled().unwrap_or(true);
    let failing = record.ingest_rejection().is_some() && !record.revoked() && enabled;
    let display_name = registration_display_name(record);
    let source_label = display_name.and_then(|_| registration_source_label(record));
    let presented_name = display_name.map_or_else(
        || record.name().unwrap_or_default().to_owned(),
        |display_name| match source_label {
            Some(source_label) => format!("{display_name} · {source_label}"),
            None => display_name.to_owned(),
        },
    );
    let mut value = Map::from_iter([
        ("prefix".to_owned(), json!(record.prefix())),
        ("name".to_owned(), json!(presented_name)),
        (
            "created_at".to_owned(),
            json!(record.created_at().unwrap_or(0)),
        ),
        (
            "last_seen".to_owned(),
            record.last_seen().map_or(Value::Null, Value::from),
        ),
        (
            "last_segment".to_owned(),
            record
                .last_segment()
                .map_or(Value::Null, |value| json!(value)),
        ),
        ("enabled".to_owned(), json!(enabled)),
        ("revoked".to_owned(), json!(record.revoked())),
        (
            "revoked_at".to_owned(),
            record.revoked_at().map_or(Value::Null, Value::from),
        ),
        (
            "stats".to_owned(),
            json!(record.stats().cloned().unwrap_or_default()),
        ),
        // Replace these nulls when a native live-connection source is mounted
        // and can determine device liveness.
        ("live".to_owned(), Value::Null),
        ("last_chat_request_at".to_owned(), Value::Null),
        ("state".to_owned(), json!(freshness.state)),
        ("group".to_owned(), json!(freshness.group)),
        (
            "elapsed_ms".to_owned(),
            freshness.elapsed_ms.map_or(Value::Null, Value::from),
        ),
        ("clock_skew".to_owned(), json!(freshness.clock_skew)),
        ("label".to_owned(), json!(state_label(freshness.state))),
        ("failing".to_owned(), json!(failing)),
    ]);
    debug_assert_eq!(value.len(), OBSERVER_ENTRY_FIELDS.len());
    if let Some(display_name) = display_name {
        value.insert("display_name".to_owned(), json!(display_name));
    }
    if let Some(source_label) = source_label {
        value.insert("source_label".to_owned(), json!(source_label));
    }
    if let Some(rejection) = failing.then(|| record.ingest_rejection()).flatten() {
        value.insert(
            "ingest_rejection".to_owned(),
            json!({
                "reason_code": rejection.get("reason_code"),
                "active_count": rejection.get("active_count"),
                "first_ts": rejection.get("first_ts"),
                "latest_ts": rejection.get("latest_ts"),
                "summary": rejection.get("summary"),
                "stream": rejection.get("stream"),
                "version": rejection.get("version"),
            }),
        );
    }
    Value::Object(value)
}

fn registration_display_name(record: &ObserverRecord) -> Option<&str> {
    let label = record.value().get("label")?.as_str()?.trim();
    if label.is_empty()
        || label.eq_ignore_ascii_case("watch")
        || label.eq_ignore_ascii_case("omi pendant")
    {
        return None;
    }
    Some(label)
}

fn registration_source_label(record: &ObserverRecord) -> Option<&'static str> {
    match record.value().get("stream_type")?.as_str()? {
        "mobile" => Some("mobile"),
        "omi" => Some("Omi"),
        "watch" => Some("Watch"),
        _ => None,
    }
}

fn observer_sort_key(value: &Value) -> (u8, bool, Reverse<i64>, String) {
    let group_rank = match value["group"].as_str() {
        Some("active") => 0,
        Some("stale") => 1,
        _ => 2,
    };
    let last_seen = value["last_seen"].as_i64();
    (
        group_rank,
        last_seen.is_none(),
        Reverse(last_seen.unwrap_or(0)),
        value["prefix"].as_str().unwrap_or_default().to_owned(),
    )
}

#[derive(Clone, Copy)]
struct Freshness {
    state: &'static str,
    group: &'static str,
    elapsed_ms: Option<i64>,
    clock_skew: bool,
}

fn freshness(last_seen: Option<i64>, revoked: bool, now_ms: i64) -> Freshness {
    if revoked {
        return Freshness {
            state: "revoked",
            group: "inactive",
            elapsed_ms: None,
            clock_skew: false,
        };
    }
    let Some(last_seen) = last_seen else {
        return Freshness {
            state: "disconnected",
            group: "inactive",
            elapsed_ms: None,
            clock_skew: false,
        };
    };
    let elapsed = now_ms - last_seen;
    if elapsed < -FUTURE_CLOCK_DRIFT_TOLERANCE_MS {
        Freshness {
            state: "disconnected",
            group: "inactive",
            elapsed_ms: Some(elapsed),
            clock_skew: true,
        }
    } else if elapsed < 0 {
        Freshness {
            state: "connected",
            group: "active",
            elapsed_ms: Some(0),
            clock_skew: false,
        }
    } else if elapsed < ACTIVE_THRESHOLD_MS {
        Freshness {
            state: "connected",
            group: "active",
            elapsed_ms: Some(elapsed),
            clock_skew: false,
        }
    } else if elapsed < STALE_THRESHOLD_MS {
        Freshness {
            state: "stale",
            group: "stale",
            elapsed_ms: Some(elapsed),
            clock_skew: false,
        }
    } else {
        Freshness {
            state: "disconnected",
            group: "inactive",
            elapsed_ms: Some(elapsed),
            clock_skew: false,
        }
    }
}

fn state_label(state: &str) -> &'static str {
    OBSERVER_STATE_LABELS
        .iter()
        .find_map(|(name, label)| (*name == state).then_some(*label))
        .expect("freshness state has a label")
}

fn error_response(status: StatusCode, reason_code: &str, error: &str, detail: &str) -> Response {
    (
        status,
        Json(json!({"error": error, "reason_code": reason_code, "detail": detail})),
    )
        .into_response()
}

fn paired_device_not_found() -> Response {
    error_response(
        StatusCode::NOT_FOUND,
        "paired_device_not_found",
        "I couldn't find that paired device.",
        "Device not found",
    )
}

fn pl_revoked(status: StatusCode, detail: &str) -> Response {
    error_response(
        status,
        "pl_revoked",
        "I couldn't use that paired device because it was revoked.",
        detail,
    )
}

fn settings_operation_failed(detail: &str) -> Response {
    error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        "settings_operation_failed",
        "I couldn't save those settings.",
        detail,
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    const INGEST_REJECTION_FIELDS: [&str; 7] = [
        "reason_code",
        "active_count",
        "first_ts",
        "latest_ts",
        "summary",
        "stream",
        "version",
    ];

    struct EstablishedJournal(PathBuf);

    impl EstablishedJournal {
        fn new() -> Self {
            let path = Self::temporary_path();
            fs::create_dir(&path).expect("journal root");
            fs::create_dir(path.join("config")).expect("config directory");
            fs::write(
                path.join("config/journal.json"),
                br#"{"setup":{"completed_at":1767225600}}"#,
            )
            .expect("journal config");
            Self(path)
        }

        fn unestablished() -> Self {
            let path = Self::temporary_path();
            fs::create_dir(&path).expect("journal root");
            Self(path)
        }

        fn temporary_path() -> PathBuf {
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            std::env::temp_dir().join(format!(
                "solstone-devices-{}-{nanos}-{sequence}",
                std::process::id()
            ))
        }

        fn observer(&self, key: &str, name: &str, last_seen: Option<i64>, revoked: bool) {
            let directory = self.0.join("apps/observer/observers");
            fs::create_dir_all(&directory).expect("observer directory");
            let prefix = key.chars().take(8).collect::<String>();
            fs::write(
                directory.join(format!("{prefix}.json")),
                json!({
                    "key": key,
                    "name": name,
                    "created_at": 1,
                    "last_seen": last_seen,
                    "revoked": revoked,
                    "enabled": true,
                    "stats": {"segments_received": 0, "bytes_received": 0},
                })
                .to_string(),
            )
            .expect("observer record");
        }

        fn failing_observer(&self, key: &str) {
            let directory = self.0.join("apps/observer/observers");
            fs::create_dir_all(&directory).expect("observer directory");
            let prefix = key.chars().take(8).collect::<String>();
            fs::write(
                directory.join(format!("{prefix}.json")),
                json!({
                    "key": key,
                    "name": "failing",
                    "created_at": 1,
                    "last_seen": system_now_ms(),
                    "enabled": true,
                    "health": {"ingest_rejection": {
                        "reason_code": "ingest_rejected",
                        "active_count": 1,
                        "first_ts": 10,
                        "latest_ts": 20,
                        "summary": "bad segment",
                        "stream": "screen",
                        "version": "2",
                        "segment": "not exposed",
                    }},
                })
                .to_string(),
            )
            .expect("failing observer record");
        }
    }

    impl Drop for EstablishedJournal {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    async fn request(app: axum::Router, request: Request<Body>) -> (StatusCode, Value) {
        let response = app.oneshot(request).await.expect("router responds");
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        let value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        (status, value)
    }

    #[tokio::test]
    async fn routes_project_manage_and_keep_the_observer_ingest_wire_literal() {
        let journal = EstablishedJournal::new();
        journal.observer("abcdefgh-key", "phone", None, false);
        let app = crate::router(journal.0.clone());

        let root = app
            .clone()
            .oneshot(Request::get("/app/devices/").body(Body::empty()).unwrap())
            .await
            .expect("root response");
        assert_eq!(root.status(), StatusCode::OK);
        let workspace = app
            .clone()
            .oneshot(
                Request::get("/app/devices/workspace")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("workspace response");
        assert_eq!(workspace.status(), StatusCode::OK);

        let (status, listed) = request(
            app.clone(),
            Request::get("/app/devices/api/list")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let row = &listed["observers"][0];
        let actual_fields = row
            .as_object()
            .expect("row object")
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let expected_fields = OBSERVER_ENTRY_FIELDS.into_iter().collect::<BTreeSet<_>>();
        assert_eq!(actual_fields, expected_fields);
        let corpus: Value =
            serde_json::from_str(include_str!("../../../fixtures/convey_devices_corpus.json"))
                .expect("devices corpus parses");
        let environment_native = &corpus["environment_native"];
        assert_eq!(row["live"], environment_native["live"]);
        assert_eq!(
            row["last_chat_request_at"],
            environment_native["last_chat_request_at"]
        );
        assert!(row["failing"].is_boolean());

        let (status, key) = request(
            app.clone(),
            Request::get("/app/devices/api/abcdefgh/key")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(key["ingest_url"], "/app/observer/ingest");
        assert_eq!(key["protocol_version"], OBSERVER_PROTOCOL_VERSION);
        let actions = fs::read_dir(journal.0.join("config/actions"))
            .expect("key view action directory")
            .map(|entry| entry.expect("action entry").path())
            .collect::<Vec<_>>();
        assert_eq!(actions.len(), 1, "one key request appends one audit action");
        let action: Value =
            serde_json::from_str(&fs::read_to_string(&actions[0]).expect("key view action record"))
                .expect("action JSON");
        assert_eq!(action["source"], "app");
        assert_eq!(action["actor"], "observer");
        assert_eq!(action["action"], "observer_key_view");
        assert_eq!(
            action["params"],
            json!({"name": "phone", "key_prefix": "abcdefgh"})
        );

        let (status, deleted) = request(
            app.clone(),
            Request::delete("/app/devices/api/abcdefgh")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(deleted["status"], "ok");
        let (status, second) = request(
            app,
            Request::delete("/app/devices/api/abcdefgh")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(second["reason_code"], "pl_revoked");
    }

    #[test]
    fn corpus_freshness_and_sorting_match_the_captured_contract() {
        let corpus: Value =
            serde_json::from_str(include_str!("../../../fixtures/convey_devices_corpus.json"))
                .expect("devices corpus parses");
        assert_eq!(
            corpus["freshness"]
                .as_array()
                .expect("freshness rows")
                .len(),
            24
        );
        assert_eq!(
            freshness(Some(2_000_000 - ACTIVE_THRESHOLD_MS), false, 2_000_000).state,
            "stale"
        );
        assert_eq!(
            freshness(Some(2_000_000 - STALE_THRESHOLD_MS), false, 2_000_000).state,
            "disconnected"
        );
        assert!(
            !freshness(
                Some(2_000_000 + FUTURE_CLOCK_DRIFT_TOLERANCE_MS),
                false,
                2_000_000,
            )
            .clock_skew
        );

        let now = 2_000_000;
        let mut rows = [
            observer_json(&record("aaaa0000-key", Some(now - 1_000), false), now),
            observer_json(&record("bbbb0000-key", Some(now - 1_000), true), now),
            observer_json(&record("cccc0000-key", Some(now - 600_000), false), now),
            observer_json(&record("dddd0000-key", Some(now - 60_000), false), now),
            observer_json(&record("eeee0000-key", None, true), now),
        ];
        rows.sort_by_key(observer_sort_key);
        assert_eq!(
            rows.map(|row| row["prefix"].as_str().unwrap().to_owned()),
            ["aaaa0000", "dddd0000", "bbbb0000", "cccc0000", "eeee0000"],
            "the two revoked rows stay on opposite sides of the offline row"
        );
    }

    #[test]
    fn registration_labels_project_readable_distinct_sources_without_mutating_records() {
        let rows = [
            ("mobile", "mobile", "Jeremie’s iPhone · mobile"),
            ("omi", "Omi", "Jeremie’s iPhone · Omi"),
            ("watch", "Watch", "Jeremie’s iPhone · Watch"),
        ]
        .map(|(stream_type, source_label, name)| {
            let record = ObserverRecord::from_value(json!({
                "key": format!("{stream_type}0-key"),
                "name": format!("iphone-{stream_type}-technical"),
                "label": "  Jeremie’s iPhone  ",
                "stream_type": stream_type,
                "enabled": true,
            }))
            .expect("record");
            let stored = record.value().clone();
            let row = observer_json(&record, 2_000_000);
            assert_eq!(
                record.value(),
                &stored,
                "projection must not mutate storage"
            );
            assert_eq!(row["display_name"], "Jeremie’s iPhone");
            assert_eq!(row["source_label"], source_label);
            assert_eq!(row["name"], name);
            row
        });

        assert_eq!(rows[0]["prefix"], "mobile0-");
        assert_eq!(rows[1]["prefix"], "omi0-key");
        assert_eq!(rows[2]["prefix"], "watch0-k");
    }

    #[test]
    fn legacy_registration_labels_preserve_technical_names() {
        for label in [
            Value::Null,
            json!(""),
            json!("  "),
            json!("watch"),
            json!("omi pendant"),
        ] {
            let record = ObserverRecord::from_value(json!({
                "key": "abcdefgh-key",
                "name": "iphone-technical-id",
                "label": label,
                "stream_type": "watch",
                "enabled": true,
            }))
            .expect("record");
            let row = observer_json(&record, 2_000_000);
            assert_eq!(row["name"], "iphone-technical-id");
            assert!(row.get("display_name").is_none());
            assert!(row.get("source_label").is_none());
        }
    }

    #[tokio::test]
    async fn list_serializes_a_seeded_failing_record_without_its_segment() {
        let journal = EstablishedJournal::new();
        journal.failing_observer("failing0-key");
        let (status, listed) = request(
            crate::router(journal.0.clone()),
            Request::get("/app/devices/api/list")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let rejection = &listed["observers"][0]["ingest_rejection"];
        assert_eq!(listed["observers"][0]["failing"], true);
        let actual_fields = rejection
            .as_object()
            .expect("rejection object")
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let expected_fields = INGEST_REJECTION_FIELDS.into_iter().collect::<BTreeSet<_>>();
        assert_eq!(actual_fields.len(), INGEST_REJECTION_FIELDS.len());
        assert_eq!(actual_fields, expected_fields);
        assert!(rejection.get("segment").is_none());
    }

    #[tokio::test]
    async fn observer_wire_refusals_keep_the_json_contract_and_devices_stays_gated() {
        let journal = EstablishedJournal::new();
        let app = crate::router(journal.0.clone());
        let expected = serde_json::to_vec(&AppNotConverted::new("observer")).expect("refusal");
        for path in [
            "/app/observer/callosum",
            "/app/observer/ingest/manifest",
            "/app/observer/ingest/segments/20260101",
            "/app/observer/ingest/other",
        ] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .expect("wire refusal");
            assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED, "{path}");
            assert_eq!(
                to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap()
                    .as_ref(),
                expected.as_slice(),
                "{path}"
            );
        }

        let bare_ingest = app
            .clone()
            .oneshot(
                Request::get("/app/observer/ingest")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("bare ingest response");
        assert_eq!(bare_ingest.status(), StatusCode::NOT_FOUND);

        let unestablished = EstablishedJournal::unestablished();
        let app = crate::router(unestablished.0.clone());
        let observer = app
            .clone()
            .oneshot(
                Request::get("/app/observer/unlisted")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(observer.status(), StatusCode::NOT_FOUND);
        let devices = app
            .oneshot(
                Request::get("/app/devices/api/list")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(devices.status(), StatusCode::FOUND);
    }

    #[tokio::test]
    async fn retired_create_and_null_live_asset_are_explicit() {
        let journal = EstablishedJournal::new();
        let app = crate::router(journal.0.clone());
        let (status, refusal) = request(
            app.clone(),
            Request::post("/app/devices/api/create")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::GONE);
        assert_eq!(refusal["reason_code"], "operation_no_longer_available");
        let response = app
            .oneshot(
                Request::get("/app/devices/workspace")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(
            std::str::from_utf8(&body)
                .unwrap()
                .contains("observer.live === null")
        );
        assert!(
            std::str::from_utf8(&body)
                .unwrap()
                .contains("data-live-status=\"unavailable\"")
        );
        assert!(
            std::str::from_utf8(&body)
                .unwrap()
                .contains("live connection status unavailable")
        );
    }

    fn record(key: &str, last_seen: Option<i64>, revoked: bool) -> ObserverRecord {
        ObserverRecord::from_value(json!({
            "key": key,
            "name": key,
            "created_at": 1,
            "last_seen": last_seen,
            "revoked": revoked,
            "enabled": true,
            "stats": {},
        }))
        .expect("record")
    }
}
