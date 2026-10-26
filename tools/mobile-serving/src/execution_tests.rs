// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use axum::body::{Body, to_bytes};
use axum::http::Request;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;
use tower::ServiceExt;

#[derive(Default)]
struct Model {
    seen: Mutex<Vec<String>>,
    refuse: AtomicBool,
    hold: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

async fn completion(State(model): State<Arc<Model>>, Json(body): Json<Value>) -> Response {
    let properties = &body["response_format"]["json_schema"]["schema"]["properties"];
    let (name, output) = if properties.get("role").is_some() {
        ("linked_presence", json!({"role":"with"}))
    } else if properties.get("points").is_some() {
        (
            "linked_share",
            json!({"points":[{"ids":["u1"],"point":"asked Jane for her opinion"}]}),
        )
    } else if properties.get("summary").is_some() {
        (
            "linked_share_activity",
            json!({"summary":"They discussed the proposal."}),
        )
    } else {
        panic!("unexpected production completion: {body}")
    };
    assert_eq!(body["temperature"], 0.0);
    assert_eq!(body["model"], "preview-fixture");
    model.seen.lock().unwrap().push(name.into());
    if model.hold.load(Ordering::Acquire) {
        model.entered.notify_one();
        model.release.notified().await;
    }
    if model.refuse.load(Ordering::Acquire) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":{"message":"synthetic refusal"}})),
        )
            .into_response();
    }
    Json(json!({"model":"preview-fixture", "choices":[{"message":{"content":output.to_string()},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":8,"completion_tokens":8,"total_tokens":16}})).into_response()
}

async fn model_server(model: Arc<Model>) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route(
            "/v1/models",
            get(|| async {
                Json(json!({"data":[{"id":"preview-fixture","context_window":32768}]}))
            }),
        )
        .route("/v1/chat/completions", post(completion))
        .with_state(model);
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}"), task)
}

fn write(root: &Path, path: &str, value: impl AsRef<[u8]>) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, value).unwrap();
}

fn fixture(endpoint: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "config/journal.json",
        json!({
            "identity":{"name":"Sam Rivera","preferred":"Sam","timezone":"UTC"},
            "providers":{"active":{"provider":"local","model":"preview-fixture"},
                "local":{"endpoint_url":endpoint,"served_model_id":"preview-fixture"}}
        })
        .to_string(),
    );
    write(
        root.path(),
        "entities/sam_rivera/entity.json",
        json!({"id":"sam_rivera","name":"Sam Rivera","type":"Person","is_principal":true})
            .to_string(),
    );
    write(
        root.path(),
        "entities/jane_doe/entity.json",
        json!({"id":"jane_doe","name":"Jane Doe","type":"Person"}).to_string(),
    );
    write(root.path(), "facets/work/facet.json", r#"{"title":"Work"}"#);
    write(root.path(), "facets/work/activities/20261012.jsonl", json!({"id":"meeting_120000_300","activity":"meeting","title":"private title not sent","segments":["120000_300"],"stream":"voice"}).to_string()+"\n");
    let rows = [
        json!({"_solstone_processing":{"state":"analyzed"}}),
        json!({"start":"00:00:01","speaker":1,"text":"Jane, what do you think?"}),
        json!({"start":"00:00:02","speaker":2,"text":"I like the proposal."}),
    ];
    write(
        root.path(),
        "chronicle/20261012/voice/120000_300/audio.jsonl",
        rows.iter()
            .map(|row| row.to_string() + "\n")
            .collect::<String>(),
    );
    root
}

fn payload() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../core/payload")
}

async fn request(router: &Router, path: &str, body: Value) -> (StatusCode, Value) {
    routed_request(router, "POST", path, body).await
}

async fn routed_request(
    router: &Router,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_execution_queue_lease_survives_client_loss_until_retirement() {
    use solstone_core_convey_shell::recovery_drain::{
        relinquish_admitted_work, resume_after_whole_journal_restore,
    };
    let fixture = fixture("http://127.0.0.1:1");
    let root = Arc::new(JournalRoot::open(fixture.path()).unwrap());
    let bridge = solstone_core_convey_shell::recovery_watch::RecoveryWatch::new(
        "execution-queue-proof",
        Arc::clone(&root),
    );
    let (_stop, stopped) = watch::channel(false);
    let (sender, mut receiver) = mpsc::channel(1);
    let router = schedules::routes(OwnerIngress {
        sender,
        root: Arc::downgrade(&root),
        stop: stopped,
    });
    let client = tokio::spawn(async move {
        routed_request(
            &router,
            "PUT",
            "/api/journal/recurrences/think/queued",
            json!({"args":["--help"],"every":"hourly","enabled":false}),
        )
        .await
    });
    let queued = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    client.abort();
    assert!(client.await.unwrap_err().is_cancelled());
    assert!(queued.cancelled.load(Ordering::Acquire));
    let target = fixture.path().to_path_buf();
    let mut drain = tokio::task::spawn_blocking(move || relinquish_admitted_work(&target));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut drain)
            .await
            .is_err()
    );
    drop(queued);
    tokio::time::timeout(Duration::from_secs(5), drain)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!fixture.path().join("config/schedules.json").exists());
    resume_after_whole_journal_restore(fixture.path());
    drop(bridge);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recovery_drains_lost_client_native_preview_and_reuses_routes_for_real_mutation() {
    use solstone_core_convey_shell::recovery_drain::{
        relinquish_admitted_work, resume_after_whole_journal_restore,
    };
    let model = Arc::new(Model::default());
    model.hold.store(true, Ordering::Release);
    let (endpoint, server) = model_server(Arc::clone(&model)).await;
    let fixture = fixture(&endpoint);
    let root = Arc::new(JournalRoot::open(fixture.path()).unwrap());
    let weak = Arc::downgrade(&root);
    let (stop, stopped) = watch::channel(false);
    let (router, worker) = routes_and_worker(root, Some(payload()), stopped);
    let worker = tokio::spawn(worker);
    let client_router = router.clone();
    let client = tokio::spawn(async move {
        request(
            &client_router,
            "/api/journal/share/match",
            json!({"person":"jane_doe","day":"20261012"}),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(10), model.entered.notified())
        .await
        .unwrap();
    client.abort();
    assert!(client.await.unwrap_err().is_cancelled());
    let target = fixture.path().to_path_buf();
    let mut drain = tokio::task::spawn_blocking(move || relinquish_admitted_work(&target));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut drain)
            .await
            .is_err()
    );
    assert!(whole_journal_restore_blocks(fixture.path()));
    let mutation = json!({"args":["--help"],"every":"hourly","enabled":false});
    assert_eq!(
        routed_request(
            &router,
            "PUT",
            "/api/journal/recurrences/think/resumed",
            mutation.clone()
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(!fixture.path().join("config/schedules.json").exists());
    model.hold.store(false, Ordering::Release);
    model.release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), drain)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        !worker.is_finished(),
        "temporary recovery must preserve the worker"
    );
    resume_after_whole_journal_restore(fixture.path());
    let (status, response) = request(
        &router,
        "/api/journal/share/match",
        json!({"person":"jane_doe","day":"20261012"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["result"][0]["basis"], "spoken_with");
    let (status, response) = routed_request(
        &router,
        "PUT",
        "/api/journal/recurrences/think/resumed",
        mutation,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let saved: Value = serde_json::from_slice(
        &std::fs::read(fixture.path().join("config/schedules.json")).unwrap(),
    )
    .unwrap();
    assert!(saved.to_string().contains("owner:think:resumed"));
    stop.send_replace(true);
    tokio::time::timeout(Duration::from_secs(5), worker)
        .await
        .unwrap()
        .unwrap();
    assert!(
        weak.upgrade().is_none(),
        "retained routes must not retain a stopped root"
    );
    assert_eq!(
        request(
            &router,
            "/api/journal/share/match",
            json!({"person":"jane_doe","day":"20261012"})
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_preview_routes_run_all_three_maintained_talents_and_return_source_refs() {
    let model = Arc::new(Model::default());
    let (endpoint, server) = model_server(Arc::clone(&model)).await;
    let fixture = fixture(&endpoint);
    let before = std::fs::read(fixture.path().join("config/journal.json")).unwrap();
    let root = Arc::new(JournalRoot::open(fixture.path()).unwrap());
    let (stop, stopped) = watch::channel(false);
    let (router, worker) = routes_and_worker(root, Some(payload()), stopped);
    let worker = tokio::spawn(worker);
    let (status, matches) = request(
        &router,
        "/api/journal/share/match",
        json!({"person":"jane_doe","day":"20261012"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{matches}");
    assert_eq!(matches["result"][0]["activity_id"], "meeting_120000_300");
    assert_eq!(matches["result"][0]["basis"], "spoken_with");
    let (status, package) = request(&router, "/api/journal/share/package", json!({"person":"jane_doe","day":"20261012","facet":"work","activity":"meeting_120000_300"})).await;
    assert_eq!(status, StatusCode::OK, "{package}");
    assert_eq!(package["result"]["available"], true, "{package}");
    assert_eq!(package["result"]["segments"][0]["segment"], "120000_300");
    assert_eq!(
        package["result"]["segments"][0]["points"][0]["ids"],
        json!(["u1"])
    );
    assert!(!package.to_string().contains("private title not sent"));
    for name in ["linked_presence", "linked_share", "linked_share_activity"] {
        assert!(
            model.seen.lock().unwrap().iter().any(|seen| seen == name),
            "{name}"
        );
    }
    assert_eq!(
        std::fs::read(fixture.path().join("config/journal.json")).unwrap(),
        before
    );
    assert!(!fixture.path().join("talents").exists());
    assert!(!fixture.path().join("chronicle/20261012/talents").exists());
    model.refuse.store(true, Ordering::Release);
    let (status, failed) = request(
        &router,
        "/api/journal/share/match",
        json!({"person":"jane_doe","day":"20261012"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{failed}");
    assert_eq!(failed["error"], "preview_failed");
    stop.send_replace(true);
    tokio::time::timeout(Duration::from_secs(5), worker)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        request(
            &router,
            "/api/journal/share/match",
            json!({"person":"jane_doe","day":"20261012"})
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn checked_stop_joins_a_preview_already_inside_the_provider() {
    let model = Arc::new(Model::default());
    model.hold.store(true, Ordering::Release);
    let (endpoint, server) = model_server(Arc::clone(&model)).await;
    let fixture = fixture(&endpoint);
    let root = Arc::new(JournalRoot::open(fixture.path()).unwrap());
    let (stop, stopped) = watch::channel(false);
    let (router, worker) = routes_and_worker(root, Some(payload()), stopped);
    let mut worker = tokio::spawn(worker);
    let request = tokio::spawn(async move {
        request(
            &router,
            "/api/journal/share/match",
            json!({"person":"jane_doe","day":"20261012"}),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(10), model.entered.notified())
        .await
        .unwrap();
    stop.send_replace(true);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut worker)
            .await
            .is_err(),
        "active blocking work must remain owned through Stop"
    );
    model.release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), worker)
        .await
        .unwrap()
        .unwrap();
    let (status, body) = request.await.unwrap();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(
        model.seen.lock().unwrap().len(),
        1,
        "Stop prevents another completion"
    );
    server.abort();
}

#[test]
fn partial_non_match_keeps_the_shared_warning() {
    use solstone_core_generate::{GenerateRequest, GeneratedResponse};
    let fixture = fixture("http://127.0.0.1:1");
    let root = JournalRoot::open(fixture.path()).unwrap();
    let generate = |_: &GenerateRequest| {
        Ok(GenerateResponse::Generated(Box::new(GeneratedResponse {
            id: None,
            text: r#"{"role":"about"}"#.into(),
            model: "test-model".into(),
            usage: json!({}),
            finish_reason: "stop".into(),
            thinking: None,
            schema_validation: Some(json!({"valid":true})),
            input_budget: Some(json!({"clipped":true,"dropped_entries":2,"dropped_chars":31})),
            request_budget: None,
            inference: None,
            hints_applied: vec![],
        })))
    };
    let (status, response) = execute_preview(
        &root,
        &payload(),
        &["match", "--person", "jane_doe", "--day", "20261012"].map(OsString::from),
        false,
        &generate,
        &|| false,
    );
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["result"], json!([]));
    assert_eq!(
        response["warnings"],
        json!([solstone_core_format::content::PARTIAL_INPUT_NOTICE])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(
    clippy::await_holding_lock,
    reason = "the foreground singleton uses one test serialization authority"
)]
async fn typed_owner_think_and_recurrence_reach_the_real_queue_and_preserve_choices() {
    let _lock = crate::foreground::TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    use crate::foreground::ForegroundProcessingHost;
    use solstone_core_generate::GeneratedResponse;
    use solstone_core_maintenance::{
        registry::routines,
        schedule_sync::{expected_entry, schedule_name},
    };
    use solstone_core_system::schedule::ScheduleNow;
    let temp = fixture("http://127.0.0.1:1");
    let config_path = temp.path().join("config/journal.json");
    let mut config: Value = serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
    config["retention"] = json!({"journal_logs":{"enabled":true,"days":30}});
    config["processing"] = json!({"history_confirmed":true});
    config["providers"] = json!({"active":{"provider":"openai","model":"fixture"}});
    config["setup"] = json!({"completed_at":chrono::Utc::now().timestamp_millis()});
    std::fs::write(&config_path, config.to_string()).unwrap();
    let payload = temp.path().join("payload");
    for directory in [
        "solstone/talent",
        "solstone/apps",
        "solstone/think/templates",
    ] {
        std::fs::create_dir_all(payload.join(directory)).unwrap();
    }
    write(
        &payload,
        "solstone/talent/custom_fixture.md",
        concat!(
            "{\n\"type\":\"generate\",\n\"schedule\":\"activity\",\n\"activities\":[\"meeting\"],\n\"priority\":1,\n",
            "\"output\":\"md\",\n\"max_output_tokens\":128\n}\ncustom fixture\n"
        ),
    );
    write(
        &payload,
        "solstone/talent/sense.md",
        concat!(
            "{\n\"type\":\"generate\",\n\"schedule\":\"segment\",\n\"priority\":0,\n",
            "\"output\":\"json\",\n\"max_output_tokens\":128\n}\nAnalyze synthetic segment\n"
        ),
    );
    let segment = "chronicle/20261012/voice/120000_300";
    write(
        temp.path(),
        &format!("{segment}/talents/sense.json"),
        r#"{"density":"active","activity_summary":"synthetic active segment","content_type":"writing"}"#,
    );
    let mut raw = serde_json::Map::from_iter([
        ("daily_time".into(), json!("00:15")),
        ("weekly_time".into(), json!("03:15")),
    ]);
    for name in [
        "heartbeat",
        "weekly-agents",
        "cadence",
        "brain",
        "facet-candidates",
        "rebuild-edges",
        "search-catch-all",
    ] {
        raw.insert(name.into(), json!({"enabled":false}));
    }
    for routine in routines() {
        let mut entry = expected_entry(routine);
        entry["enabled"] = json!(false);
        raw.insert(schedule_name(routine.id), entry);
    }
    write(
        temp.path(),
        "config/schedules.json",
        Value::Object(raw).to_string(),
    );
    let service_root = temp.path().to_path_buf();
    let service_payload = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../core/payload");
    let service = tokio::task::spawn_blocking(move || {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let std::net::SocketAddr::V4(address) = probe.local_addr().unwrap() else {
            unreachable!()
        };
        drop(probe);
        CheckedServing(Some(Arc::new(
            crate::MobileServing::start_with_payload(service_root, address, service_payload)
                .unwrap(),
        )))
    })
    .await
    .unwrap();
    let config_before = std::fs::read(temp.path().join("config/journal.json")).unwrap();
    let args = json!([
        "--day",
        "20261012",
        "--activity",
        "meeting_120000_300",
        "--facet",
        "work",
        "--refresh"
    ]);
    // A live serving bus is insufficient: no foreground owner means no admission.
    assert_eq!(
        owner_request(&service, "POST", "/api/journal/think", json!({"args":args}))
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let (status, inspection) = owner_request(
        &service,
        "POST",
        "/api/journal/think",
        json!({"args":["--help"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{inspection}");
    assert!(inspection["stdout"].as_str().unwrap().contains("--updated"));
    let (status, inspection) = owner_request(
        &service,
        "POST",
        "/api/journal/think",
        json!({"args":["--updated"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{inspection}");
    assert_eq!(inspection["exit_code"], 0);
    let outside = tempfile::tempdir().unwrap();
    let outside_day = outside.path().join("escape-day");
    let invalid_days = [
        outside_day.to_string_lossy().into_owned(),
        "../../escape-day".to_owned(),
        "2026-10-12".to_owned(),
    ];
    let schedules_before = std::fs::read(temp.path().join("config/schedules.json")).unwrap();
    for day in &invalid_days {
        let request = json!({"args":["--day",day,"--cadence"]});
        assert_eq!(
            owner_request(&service, "POST", "/api/journal/think", request)
                .await
                .0,
            StatusCode::BAD_REQUEST,
            "invalid journal coordinate {day}"
        );
        assert_eq!(
            owner_request(
                &service,
                "PUT",
                "/api/journal/recurrences/think/invalid_day",
                json!({"args":["--day",day,"--cadence"],"every":"5m","enabled":true})
            )
            .await
            .0,
            StatusCode::BAD_REQUEST,
            "invalid recurring journal coordinate {day}"
        );
    }
    assert_eq!(
        std::fs::read(temp.path().join("config/schedules.json")).unwrap(),
        schedules_before
    );
    assert!(!outside_day.exists());
    let generate = |_: &GenerateRequest| {
        Ok(GenerateResponse::Generated(Box::new(GeneratedResponse {
            id: None,
            text: json!({"density":"active","content_type":"writing","activity_summary":"typed owner fixture result",
                "entities":[],"facets":[],"speculative_facet":null,"meeting_detected":false,"speakers":[],
                "recommend":{"screen_record":false,"speaker_attribution":false},"emotional_register":"neutral","topics":[],"quotes":[]}).to_string(),
            model: "fixture".into(),
            usage: json!({}),
            finish_reason: "stop".into(),
            thinking: None,
            schema_validation: None,
            input_budget: None,
            request_budget: None,
            inference: None,
            hints_applied: vec![],
        })))
    };
    use chrono::TimeZone;
    // The actual worker keeps its real clock. Only the scheduler's existing
    // test seam receives a future clock, so every control would be due if
    // cached disabled/deleted work were still admitted.
    let now = chrono::Utc.with_ymd_and_hms(2030, 1, 1, 12, 0, 0).unwrap();
    let host = ForegroundProcessingHost::open_with_executor(
        temp.path(),
        &payload,
        ScheduleNow::in_zone(now, chrono_tz::UTC),
        Arc::new(generate),
        solstone_core_backup_web::BackupWebDeps::new(temp.path().to_path_buf()),
    )
    .unwrap();
    host.stop_discovery_for_test();
    // Bus work and the API share this hosted selector; even a directly queued
    // invalid coordinate must stop before constructing a path or using Generate.
    for day in &invalid_days {
        assert_eq!(
            host.execute_worker_command(&[
                "think".to_owned(),
                "--day".to_owned(),
                day.clone(),
                "--cadence".to_owned(),
            ]),
            2
        );
    }
    assert!(!outside_day.exists());
    let (status, response) =
        owner_request(&service, "POST", "/api/journal/think", json!({"args":args})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{response}");
    let output = temp
        .path()
        .join("facets/work/activities/20261012/meeting_120000_300/custom_fixture.md");
    let completed = tokio::time::timeout(Duration::from_secs(15), async {
        while !output.exists()
            || !host.queue().history().iter().any(|record| {
                record.command.iter().any(|arg| arg == "--refresh") && record.exit_status == "ok"
            })
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    let logs = std::fs::read_dir(temp.path().join("talents/custom_fixture"))
        .ok()
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| (entry.file_name(), std::fs::read_to_string(entry.path())))
                .collect::<Vec<_>>()
        });
    assert!(
        completed.is_ok(),
        "actual owner queue history: {:?}; status: {:?}; callable logs: {logs:?}",
        host.queue().history(),
        host.queue()
            .collect_status_snapshot(std::time::Instant::now())
    );
    assert!(
        std::fs::read_to_string(&output)
            .unwrap()
            .contains("typed owner fixture result")
    );
    // A real canonical oplog, created by its owner and old enough under the
    // explicit saved policy. A second old writer keeps its live lease.
    use solstone_core_journal_io::operational_log::{OplogFormat, create_oplog_at};
    use std::io::Write;
    let old_at = chrono::Utc
        .with_ymd_and_hms(2020, 1, 1, 12, 0, 0)
        .unwrap()
        .fixed_offset();
    let mut old = create_oplog_at(
        JournalRoot::open(temp.path()).unwrap(),
        "think",
        "api-fixture",
        OplogFormat::Jsonl,
        old_at,
    )
    .unwrap();
    old.write_all(b"{\"event\":\"completed\"}\n").unwrap();
    let old_path = temp
        .path()
        .join("chronicle/20200101/health")
        .join(old.leaf_name());
    assert!(old_path.exists());
    drop(old);
    let mut held = create_oplog_at(
        JournalRoot::open(temp.path()).unwrap(),
        "think",
        "active-fixture",
        OplogFormat::Jsonl,
        old_at,
    )
    .unwrap();
    held.write_all(b"{\"event\":\"started\"}\n").unwrap();
    let held_path = temp
        .path()
        .join("chronicle/20200101/health")
        .join(held.leaf_name());
    let (status, prune) = owner_request(
        &service,
        "POST",
        "/api/journal/maintenance/health:prune-logs",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{prune}");
    let reference = prune["reference"].as_str().unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !host
            .queue()
            .history()
            .iter()
            .any(|record| record.reference == reference && record.exit_status == "ok")
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        !old_path.exists(),
        "eligible released operational log must be pruned through actual owner dispatch"
    );
    assert!(
        held_path.exists(),
        "live writer lease must preserve its old log"
    );
    drop(held);
    let path = "/api/journal/recurrences/think/custom_fixture";
    let mutate = |method: &str, body: Value| {
        let service = Arc::clone(&service);
        let method = method.to_owned();
        async move { owner_request(&service, &method, path, body).await }
    };
    let (status, value) = mutate(
        "PUT",
        json!({"args":args,"every":"1m","enabled":true,"max_runtime":"30m"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    host.check_schedule(ScheduleNow::in_zone(
        now + chrono::Duration::minutes(1),
        chrono_tz::UTC,
    ))
    .unwrap();
    host.queue().resume_processing();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let saved: Value = match std::fs::read(temp.path().join("health/scheduler.json")) {
                Ok(bytes) => serde_json::from_slice(&bytes).unwrap(),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({}),
                Err(error) => panic!("scheduler state read failed: {error}"),
            };
            if saved["owner:think:custom_fixture"]["last_status"] == "success" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let (status, value) = mutate(
        "PUT",
        json!({"args":args,"every":"weekly","enabled":false,"max_runtime":"10m"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let completed = host
        .queue()
        .history()
        .iter()
        .filter(|record| record.scheduler_name.as_deref() == Some("owner:think:custom_fixture"))
        .count();
    host.check_schedule(ScheduleNow::in_zone(
        now + chrono::Duration::minutes(10),
        chrono_tz::UTC,
    ))
    .unwrap();
    host.queue().join_callable_workers().unwrap();
    assert_eq!(
        host.queue()
            .history()
            .iter()
            .filter(|record| record.scheduler_name.as_deref() == Some("owner:think:custom_fixture"))
            .count(),
        completed,
        "disabled recurrence must stop at the next tick within the same hour"
    );
    let saved: Value =
        serde_json::from_slice(&std::fs::read(temp.path().join("config/schedules.json")).unwrap())
            .unwrap();
    assert_eq!(saved["owner:think:custom_fixture"]["enabled"], false);
    assert_eq!(saved["owner:think:custom_fixture"]["every"], "weekly");
    assert_eq!(mutate("DELETE", json!({})).await.0, StatusCode::OK);
    host.check_schedule(ScheduleNow::in_zone(
        now + chrono::Duration::minutes(20),
        chrono_tz::UTC,
    ))
    .unwrap();
    host.queue().join_callable_workers().unwrap();
    assert_eq!(
        host.queue()
            .history()
            .iter()
            .filter(|record| record.scheduler_name.as_deref() == Some("owner:think:custom_fixture"))
            .count(),
        completed
    );
    assert_eq!(
        mutate(
            "PUT",
            json!({"args":args,"every":"9223372036854775807m","enabled":true,"max_runtime":"30m"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        mutate(
            "PUT",
            json!({"args":args,"every":"5m","enabled":true,"max_runtime":"18446744073709551615s"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let saved: Value =
        serde_json::from_slice(&std::fs::read(temp.path().join("config/schedules.json")).unwrap())
            .unwrap();
    assert!(saved.get("owner:think:custom_fixture").is_none());
    assert_eq!(saved["daily_time"], "00:15");
    assert_eq!(saved["weekly_time"], "03:15");
    assert_eq!(
        std::fs::read(temp.path().join("config/journal.json")).unwrap(),
        config_before
    );
    assert_eq!(
        owner_request(
            &service,
            "POST",
            "/api/journal/think",
            json!({"args":["sh","-c","anything"]})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    host.shutdown().unwrap();
    assert_eq!(
        owner_request(&service, "POST", "/api/journal/think", json!({"args":args}))
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let second = ForegroundProcessingHost::open_with_executor(
        temp.path(),
        &payload,
        ScheduleNow::in_zone(chrono::Utc::now(), chrono_tz::UTC),
        Arc::new(generate),
        solstone_core_backup_web::BackupWebDeps::new(temp.path().to_path_buf()),
    )
    .unwrap();
    host.request_stop();
    host.shutdown().unwrap();
    drop(host);
    assert!(
        ForegroundProcessingHost::open_with_executor(
            temp.path(),
            &payload,
            ScheduleNow::in_zone(chrono::Utc::now(), chrono_tz::UTC),
            Arc::new(generate),
            solstone_core_backup_web::BackupWebDeps::new(temp.path().to_path_buf())
        )
        .is_err(),
        "an old host must not release the reopened generation's singleton"
    );
    assert_eq!(
        owner_request(&service, "POST", "/api/journal/think", json!({"args":args}))
            .await
            .0,
        StatusCode::ACCEPTED,
        "old repeated Stop/Drop must not revoke the reopened queue"
    );
    second.shutdown().unwrap();
    tokio::task::spawn_blocking(move || {
        service.shutdown_for_retirement().unwrap();
        drop(service);
    })
    .await
    .unwrap();
}

async fn owner_request(
    service: &Arc<crate::MobileServing>,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let service = Arc::clone(service);
    let method = method.to_owned();
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        let response = service
            .request_owner(&method, &path, &body.to_string())
            .unwrap();
        (
            StatusCode::from_u16(response.status).unwrap(),
            serde_json::from_str(&response.body).unwrap_or_else(|error| {
                panic!(
                    "owner response {method} {path} status {}: {error}: {}",
                    response.status, response.body
                )
            }),
        )
    })
    .await
    .unwrap()
}

// MobileServing is a synchronous native owner. Retire and drop its runtime on
// a synchronous thread even when an assertion unwinds this async composition.
struct CheckedServing(Option<Arc<crate::MobileServing>>);
impl std::ops::Deref for CheckedServing {
    type Target = Arc<crate::MobileServing>;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().unwrap()
    }
}
impl Drop for CheckedServing {
    fn drop(&mut self) {
        if let Some(owner) = self.0.take() {
            let result = std::thread::spawn(move || {
                let result = owner.shutdown_for_retirement();
                drop(owner);
                result
            })
            .join();
            if !matches!(result, Ok(Ok(()))) {
                eprintln!("synthetic serving retirement failed: {result:?}");
            }
        }
    }
}
