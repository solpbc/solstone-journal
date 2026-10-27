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
    let (router, worker, _settings_sync) = routes_and_worker(root, Some(payload()), stopped);
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
async fn source_recurrence_controls_legacy_aliases_and_preserves_modified_custom_commands() {
    use chrono::TimeZone;
    use solstone_core_system::request::ScheduledRequest;
    use solstone_core_system::schedule::{ScheduleEngine, ScheduleNow, ScheduleSubmissionSink};
    #[derive(Default)]
    struct Sink(Mutex<Vec<ScheduledRequest>>);
    impl ScheduleSubmissionSink for Sink {
        fn submit(&self, request: ScheduledRequest) -> bool {
            self.0.lock().unwrap().push(request);
            true
        }
    }
    let fixture = fixture("http://127.0.0.1:1");
    let path = fixture.path().join("config/schedules.json");
    let custom =
        json!({"cmd":["solstone","journal","heartbeat"],"every":"daily","extra":"owner custom"});
    let original = json!({
        "daily_time":"00:15", "weekly_time":"03:15",
        "sync:plaud":{"cmd":["solstone","journal","importer","--sync","plaud","--save","--dry-run"],"every":"hourly","max_runtime":"10m","extra":"kept"},
        "owner:source:plaud":{"cmd":["journal","importer","--sync","plaud","--save"],"every":"weekly","enabled":true},
        "sync:obsidian":custom,
        "owner:source:obsidian":{"cmd":["journal","importer","--sync","obsidian","--save","--path","/retained/owner/vault"],"every":"hourly"},
        "custom":custom
    });
    std::fs::write(&path, original.to_string()).unwrap();
    let before = std::fs::read(&path).unwrap();
    let root = Arc::new(JournalRoot::open(fixture.path()).unwrap());
    let (stop, stopped) = watch::channel(false);
    let (router, worker, _settings_sync) = routes_and_worker(root, Some(payload()), stopped);
    let worker = tokio::spawn(worker);
    let plaud = "/api/journal/recurrences/source/plaud";
    let (status, observed) = routed_request(&router, "GET", plaud, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{observed}");
    assert_eq!(
        observed["enabled"], true,
        "legacy missing-enabled is active"
    );
    assert_eq!(observed["entries"].as_array().unwrap().len(), 2);
    let (_, audio) = routed_request(
        &router,
        "GET",
        "/api/journal/recurrences/source/audio",
        json!({}),
    )
    .await;
    assert_eq!(audio["enabled"], false);
    assert_eq!(audio["entries"], json!([]));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "status never synthesizes a source recurrence"
    );
    let (status, changed) =
        routed_request(&router, "PUT", plaud, json!({"every":"1m","enabled":true})).await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(
        changed["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["active"] == true)
            .count(),
        1
    );
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["sync:plaud"]["cmd"], original["sync:plaud"]["cmd"]);
    assert_eq!(saved["sync:plaud"]["max_runtime"], "10m");
    assert_eq!(saved["sync:plaud"]["extra"], "kept");
    assert_eq!(saved["owner:source:plaud"]["enabled"], false);
    assert_eq!(saved["custom"], custom);
    let now = chrono::Utc.with_ymd_and_hms(2030, 1, 1, 12, 0, 0).unwrap();
    let (mut engine, _) = ScheduleEngine::init(
        &path,
        fixture.path().join("health/scheduler.json"),
        ScheduleNow::in_zone(now, chrono_tz::UTC),
    )
    .unwrap();
    let sink = Sink::default();
    engine
        .check(
            ScheduleNow::in_zone(now + chrono::Duration::minutes(1), chrono_tz::UTC),
            &sink,
        )
        .unwrap();
    let emitted = {
        let requests = sink.0.lock().unwrap();
        requests
            .iter()
            .filter(|r| {
                r.scheduler_name
                    .as_deref()
                    .is_some_and(|n| n == "sync:plaud" || n == "owner:source:plaud")
            })
            .count()
    };
    assert_eq!(
        emitted, 1,
        "the actual scheduler submits only one owned source recurrence"
    );
    let obsidian = "/api/journal/recurrences/source/obsidian";
    let (_, observed) = routed_request(&router, "GET", obsidian, json!({})).await;
    assert_eq!(observed["enabled"], true);
    assert_eq!(observed["blocked_names"], json!(["sync:obsidian"]));
    let (status, disabled) = routed_request(
        &router,
        "PUT",
        obsidian,
        json!({"every":"daily","enabled":false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{disabled}");
    assert_eq!(disabled["enabled"], false);
    assert_eq!(
        routed_request(&router, "DELETE", obsidian, json!({}))
            .await
            .0,
        StatusCode::OK
    );
    let (status, removed) = routed_request(&router, "DELETE", plaud, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert_eq!(removed["enabled"], false);
    let mut saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(saved.get("sync:plaud").is_none() && saved.get("owner:source:plaud").is_none());
    assert!(saved.get("owner:source:obsidian").is_none());
    assert_eq!(saved["sync:obsidian"], custom);
    assert_eq!(saved["custom"], custom);
    assert_eq!(saved["daily_time"], "00:15");
    assert_eq!(saved["weekly_time"], "03:15");
    saved["sync:plaud"] = custom.clone();
    saved["owner:source:plaud"] = custom.clone();
    std::fs::write(&path, saved.to_string()).unwrap();
    let custom_bytes = std::fs::read(&path).unwrap();
    assert_eq!(
        routed_request(
            &router,
            "PUT",
            plaud,
            json!({"every":"hourly","enabled":true})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        routed_request(&router, "DELETE", plaud, json!({})).await.0,
        StatusCode::OK
    );
    assert_eq!(
        routed_request(
            &router,
            "GET",
            "/api/journal/recurrences/source/oura",
            json!({})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        custom_bytes,
        "custom reserved-name commands are preserved"
    );
    stop.send_replace(true);
    worker.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_preview_routes_run_all_three_maintained_talents_and_return_source_refs() {
    let model = Arc::new(Model::default());
    let (endpoint, server) = model_server(Arc::clone(&model)).await;
    let fixture = fixture(&endpoint);
    let before = std::fs::read(fixture.path().join("config/journal.json")).unwrap();
    let root = Arc::new(JournalRoot::open(fixture.path()).unwrap());
    let (stop, stopped) = watch::channel(false);
    let (router, worker, _settings_sync) = routes_and_worker(root, Some(payload()), stopped);
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
    let (router, worker, _settings_sync) = routes_and_worker(root, Some(payload()), stopped);
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

#[test]
fn owner_settings_and_source_cards_share_alias_choices_audit_and_root_refusal() {
    fn call(server: &crate::MobileServing, method: &str, path: &str, body: Value) -> (u16, Value) {
        let response = server
            .request_owner(method, path, &body.to_string())
            .unwrap();
        (
            response.status,
            serde_json::from_str(&response.body).unwrap_or_else(|error| {
                panic!(
                    "{method} {path}: status={} body={:?}: {error}",
                    response.status, response.body
                )
            }),
        )
    }
    let _serial = crate::foreground::TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let fixture = fixture("http://127.0.0.1:1");
    let schedules = fixture.path().join("config/schedules.json");
    let custom = json!({"cmd":["owner-custom-command"],"every":"weekly","extra":"kept"});
    let original = json!({
        "daily_time":"00:15", "weekly_time":"03:15",
        "sync:plaud":{"cmd":["solstone","journal","importer","--sync","plaud","--save","--dry-run"],"every":"weekly","extra":"kept","max_runtime":"10m"},
        "owner:source:plaud":{"cmd":["journal","importer","--sync","plaud","--save"],"every":"hourly","enabled":true},
        "sync:obsidian":custom,
        "owner:source:obsidian":{"cmd":["journal","importer","--sync","obsidian","--save"],"every":"daily","enabled":true},
        "custom":custom
    });
    std::fs::write(&schedules, original.to_string()).unwrap();
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let std::net::SocketAddr::V4(address) = probe.local_addr().unwrap() else {
        unreachable!()
    };
    drop(probe);
    let server = crate::MobileServing::start(fixture.path().to_path_buf(), address).unwrap();
    let (code, proposal) = call(&server, "GET", "/init/mark", json!({}));
    assert_eq!(code, 200, "{proposal}");
    assert_eq!(
        call(
            &server,
            "POST",
            "/init/mark/lock",
            json!({"proposal_id":proposal["proposal_id"]})
        )
        .0,
        200
    );
    let (code, finalized) = call(&server, "POST", "/init/finalize", json!({}));
    assert_eq!(code, 200, "{finalized}");
    let settings = "/app/settings/api/sync";
    let plaud = "/api/journal/recurrences/source/plaud";
    let (code, legacy) = call(&server, "GET", settings, json!({}));
    assert_eq!(code, 200, "{legacy}");
    let (code, source) = call(&server, "GET", plaud, json!({}));
    assert_eq!(code, 200, "{source}");
    assert_eq!(legacy["plaud"]["enabled"], source["enabled"]);
    assert_eq!(
        legacy["plaud"]["enabled"], true,
        "missing-enabled legacy entry is active"
    );
    assert_eq!(
        legacy["obsidian"]["enabled"], true,
        "owner-only alias is visible to Settings"
    );
    assert_eq!(
        legacy["obsidian"]["blocked_names"],
        json!(["sync:obsidian"])
    );
    assert_eq!(
        call(
            &server,
            "PUT",
            settings,
            json!({"plaud":{"enabled":"wrong"}})
        )
        .0,
        400
    );
    let (code, disabled) = call(
        &server,
        "PUT",
        settings,
        json!({"plaud":{"enabled":false},"obsidian":{"enabled":false}}),
    );
    assert_eq!(code, 200, "{disabled}");
    assert_eq!(disabled["plaud"]["enabled"], false);
    assert_eq!(disabled["obsidian"]["enabled"], false);
    assert_eq!(call(&server, "GET", plaud, json!({})).1["enabled"], false);
    assert_eq!(
        call(&server, "PUT", plaud, json!({"every":"5m","enabled":true})).0,
        200
    );
    assert_eq!(
        call(&server, "GET", settings, json!({})).1["plaud"]["enabled"],
        true
    );
    assert_eq!(
        call(&server, "PUT", settings, json!({"plaud":{"enabled":false}})).0,
        200
    );
    let (code, enabled) = call(&server, "PUT", settings, json!({"plaud":{"enabled":true}}));
    assert_eq!(code, 200, "{enabled}");
    let source = call(&server, "GET", plaud, json!({})).1;
    assert_eq!(
        source["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["active"] == true)
            .count(),
        1
    );
    let saved: Value = serde_json::from_slice(&std::fs::read(&schedules).unwrap()).unwrap();
    assert_eq!(
        saved["sync:plaud"]["every"], "5m",
        "Settings preserves cadence selected by source card"
    );
    assert_eq!(saved["sync:plaud"]["cmd"], original["sync:plaud"]["cmd"]);
    assert_eq!(saved["sync:plaud"]["max_runtime"], "10m");
    assert_eq!(saved["sync:plaud"]["extra"], "kept");
    assert_eq!(saved["sync:obsidian"], custom);
    assert_eq!(saved["custom"], custom);
    assert_eq!(saved["daily_time"], "00:15");
    assert_eq!(saved["weekly_time"], "03:15");
    assert!(
        contains_action(fixture.path(), "sync_update"),
        "same existing Settings audit is retained"
    );
    assert_eq!(call(&server, "DELETE", plaud, json!({})).0, 200);
    let removed = call(&server, "GET", settings, json!({})).1;
    assert_eq!(removed["plaud"]["enabled"], false);
    assert_eq!(removed["plaud"]["configured"], false);
    assert_eq!(
        call(&server, "PUT", settings, json!({"plaud":{"enabled":true}})).0,
        200
    );
    let mut saved: Value = serde_json::from_slice(&std::fs::read(&schedules).unwrap()).unwrap();
    assert_eq!(
        saved["sync:plaud"]["every"], "hourly",
        "only explicit enable creates the maintained Settings recurrence"
    );
    assert!(saved.get("owner:source:plaud").is_none());
    saved["sync:plaud"] = custom.clone();
    saved["owner:source:plaud"] = custom.clone();
    std::fs::write(&schedules, saved.to_string()).unwrap();
    let custom_bytes = std::fs::read(&schedules).unwrap();
    assert_eq!(
        call(
            &server,
            "PUT",
            settings,
            json!({"plaud":{"enabled":true},"obsidian":{"enabled":true}})
        )
        .0,
        409
    );
    assert_eq!(
        std::fs::read(&schedules).unwrap(),
        custom_bytes,
        "conflicting multi-source update is atomic and custom commands stay unchanged"
    );
    let retained = tempfile::tempdir().unwrap();
    std::fs::rename(fixture.path(), retained.path().join("original")).unwrap();
    std::fs::create_dir(fixture.path()).unwrap();
    for path in [settings, plaud] {
        assert!(server.request_owner("GET", path, "{}").is_err());
        assert!(server.request_owner("PUT", path, "{}").is_err());
    }
    server.shutdown_for_retirement().unwrap();
    drop(server);
    let mut entries = Vec::new();
    let mut pending = vec![fixture.path().to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            entries.push((
                path.strip_prefix(fixture.path()).unwrap().to_owned(),
                metadata.len(),
                if metadata.is_file() {
                    std::fs::read_to_string(&path).ok()
                } else {
                    None
                },
            ));
            if metadata.is_dir() {
                pending.push(path);
            }
        }
    }
    if !entries.is_empty() {
        let replacement = fixture.keep();
        let original = retained.keep();
        panic!(
            "replacement root receives no schedule, action, lock or producer write: entries={entries:?}; replacement={}; original={}",
            replacement.display(),
            original.display()
        );
    }
}

fn contains_action(root: &Path, action: &str) -> bool {
    std::fs::read_dir(root.join("config/actions"))
        .unwrap()
        .any(|entry| {
            std::fs::read_to_string(entry.unwrap().path())
                .unwrap()
                .lines()
                .any(|line| {
                    let value: Value = serde_json::from_str(line).unwrap();
                    value["action"] == action
                })
        })
}

#[cfg(all(unix, feature = "in-process-pdf"))]
#[test]
fn authenticated_source_recurrence_executes_saved_connection_and_obeys_edits() {
    use crate::foreground::ForegroundProcessingHost;
    use crate::import_route::{
        ConfiguredSources, SavedFolderProvider, SavedFolderRequest, SourceCopyError,
        packaged_import_factory_with_sources,
    };
    use solstone_core_import::{
        BackendName, SyncState, WorkStop, begin_folder, write_sync_state_bound,
    };
    use solstone_core_maintenance::{
        registry::routines,
        schedule_sync::{expected_entry, schedule_name},
    };
    use solstone_core_system::schedule::ScheduleNow;
    use std::sync::atomic::AtomicUsize;

    struct Provider {
        identity: String,
        calls: AtomicUsize,
    }
    impl SavedFolderProvider for Provider {
        fn copy(
            &self,
            request: SavedFolderRequest<'_>,
            destination: &Path,
        ) -> Result<(), SourceCopyError> {
            assert_eq!(request.backend, BackendName::Obsidian);
            assert_eq!(request.source_identity, self.identity);
            assert_eq!(request.access_material, "synthetic-owner-grant");
            assert!(!request.stop.is_requested());
            let call = self.calls.fetch_add(1, Ordering::AcqRel) + 1;
            let note = destination.join("note.md");
            std::fs::write(
                &note,
                format!("# Notes\n\ncobalt scheduled source run {call}\n"),
            )
            .unwrap();
            let modified: chrono::DateTime<chrono::Utc> = "2026-10-14T09:00:00Z".parse().unwrap();
            std::fs::File::open(note)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(
                    std::time::SystemTime::UNIX_EPOCH
                        + Duration::from_secs(modified.timestamp().try_into().unwrap())
                        + Duration::from_secs(call as u64),
                ))
                .unwrap();
            Ok(())
        }
    }
    fn call(server: &crate::MobileServing, method: &str, body: Value) -> (u16, Value) {
        let response = server
            .request_owner(
                method,
                "/api/journal/recurrences/source/obsidian",
                &body.to_string(),
            )
            .unwrap();
        (
            response.status,
            serde_json::from_str(&response.body).unwrap(),
        )
    }
    fn completed(host: &ForegroundProcessingHost, name: &str) -> usize {
        host.queue()
            .history()
            .iter()
            .filter(|record| {
                record.scheduler_name.as_deref() == Some(name) && record.exit_status == "ok"
            })
            .count()
    }
    fn wait_completed(host: &ForegroundProcessingHost, name: &str, expected: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while completed(host, name) < expected {
            assert!(
                std::time::Instant::now() < deadline,
                "source did not complete: {:?}",
                host.queue().history()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        host.queue().join_callable_workers().unwrap();
    }
    let _serial = crate::foreground::TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let journal = fixture("http://127.0.0.1:1");
    let config_path = journal.path().join("config/journal.json");
    let mut config: Value = serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
    config["setup"] = json!({"completed_at":1});
    config["processing"] = json!({"history_confirmed":true});
    std::fs::write(&config_path, config.to_string()).unwrap();
    let mut schedules = serde_json::Map::from_iter([
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
        schedules.insert(name.into(), json!({"enabled":false}));
    }
    for routine in routines() {
        let mut entry = expected_entry(routine);
        entry["enabled"] = json!(false);
        schedules.insert(schedule_name(routine.id), entry);
    }
    let custom =
        json!({"cmd":["owner-custom-command"],"every":"weekly","enabled":false,"extra":"kept"});
    schedules.insert("custom".into(), custom.clone());
    write(
        journal.path(),
        "config/schedules.json",
        Value::Object(schedules).to_string(),
    );
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let std::net::SocketAddr::V4(address) = probe.local_addr().unwrap() else {
        unreachable!()
    };
    drop(probe);
    let server = Arc::new(
        crate::MobileServing::start_with_payload(journal.path().to_path_buf(), address, payload())
            .unwrap(),
    );
    let retained = tempfile::tempdir().unwrap();
    std::fs::write(
        retained.path().join("note.md"),
        "# Wrong\n\nwrong retained source\n",
    )
    .unwrap();
    let mut saved = SyncState::empty(BackendName::Obsidian);
    let identity = begin_folder(&mut saved, "Notes", true, Some("synthetic-owner-grant")).unwrap();
    saved
        .root_mut()
        .insert("source_path".into(), json!(retained.path()));
    write_sync_state_bound(&server.root, &saved, &WorkStop::new()).unwrap();
    let provider = Arc::new(Provider {
        identity,
        calls: AtomicUsize::new(0),
    });
    let cache = tempfile::tempdir().unwrap();
    // This format never invokes the PDF worker.
    let library = cache.path().join("unused-pdfium.so");
    std::fs::write(&library, b"notes do not load PDFium").unwrap();
    let sources = Arc::new(ConfiguredSources::new(
        cache.path().to_path_buf(),
        provider.clone(),
    ));
    let host = ForegroundProcessingHost::open_with_imports(
        Arc::clone(&server.root),
        &payload(),
        Arc::new(|| false),
        Arc::new(AtomicBool::new(false)),
        server.backup_dependencies(),
        packaged_import_factory_with_sources(library, sources),
        false,
        false,
    )
    .unwrap();
    host.stop_discovery_for_test();
    let config_before = std::fs::read(&config_path).unwrap();
    let now = chrono::Utc::now();
    assert_eq!(call(&server, "GET", json!({})).1["enabled"], false);
    let (status, created) = call(&server, "PUT", json!({"every":"1m","enabled":true}));
    assert_eq!(status, 200, "{created}");
    assert_eq!(created["entries"].as_array().unwrap().len(), 1);
    let name = created["entries"][0]["name"].as_str().unwrap().to_owned();
    assert!(
        created["entries"][0]["entry"]["cmd"]
            .as_array()
            .unwrap()
            .iter()
            .all(|arg| arg.as_str() != Some("--path"))
    );
    let report = host
        .check_schedule(ScheduleNow::in_zone(
            now + chrono::Duration::minutes(1),
            chrono_tz::UTC,
        ))
        .unwrap();
    assert!(report.submitted.iter().any(|submitted| submitted == &name));
    host.queue().resume_processing();
    wait_completed(&host, &name, 1);
    let read = server
        .request_owner(
            "GET",
            "/app/transcripts/api/read/20261014?transcripts=1",
            "",
        )
        .unwrap();
    assert_eq!(read.status, 200);
    assert!(
        read.body.contains("cobalt scheduled source run 1"),
        "{}",
        read.body
    );
    assert!(!read.body.contains("wrong retained source"));
    assert_eq!(provider.calls.load(Ordering::Acquire), 1);
    server.index().unwrap(); // Explicit indexing; automatic indexing is not proved.
    let search = server
        .request_owner("GET", "/app/search/api/search?q=cobalt", "")
        .unwrap();
    assert_eq!(search.status, 200, "{}", search.body);
    let search: Value = serde_json::from_str(&search.body).unwrap();
    assert!(
        search["days"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|day| day["results"].as_array().unwrap())
            .any(|hit| hit["day"] == "20261014"
                && hit["text"].as_str().is_some_and(|text| text
                    .replace("<strong>", "")
                    .replace("</strong>", "")
                    .contains("cobalt scheduled source run 1"))),
        "{search}"
    );
    let (status, changed) = call(&server, "PUT", json!({"every":"5m","enabled":true}));
    assert_eq!(status, 200, "{changed}");
    let report = host
        .check_schedule(ScheduleNow::in_zone(
            now + chrono::Duration::minutes(2),
            chrono_tz::UTC,
        ))
        .unwrap();
    assert!(!report.submitted.iter().any(|submitted| submitted == &name));
    host.queue().join_callable_workers().unwrap();
    assert_eq!(
        provider.calls.load(Ordering::Acquire),
        1,
        "cadence change prevents the old one-minute invocation"
    );
    let report = host
        .check_schedule(ScheduleNow::in_zone(
            now + chrono::Duration::minutes(8),
            chrono_tz::UTC,
        ))
        .unwrap();
    assert!(report.submitted.iter().any(|submitted| submitted == &name));
    host.queue().resume_processing();
    wait_completed(&host, &name, 2);
    let read = server
        .request_owner(
            "GET",
            "/app/transcripts/api/read/20261014?transcripts=1",
            "",
        )
        .unwrap();
    assert_eq!(read.status, 200);
    assert!(
        read.body.contains("cobalt scheduled source run 2"),
        "{}",
        read.body
    );
    assert_eq!(call(&server, "DELETE", json!({})).0, 200);
    let report = host
        .check_schedule(ScheduleNow::in_zone(
            now + chrono::Duration::minutes(9),
            chrono_tz::UTC,
        ))
        .unwrap();
    assert!(!report.submitted.iter().any(|submitted| submitted == &name));
    host.queue().join_callable_workers().unwrap();
    assert_eq!(
        provider.calls.load(Ordering::Acquire),
        2,
        "removed recurrence cannot invoke the retained source"
    );
    let raw: Value = serde_json::from_slice(
        &std::fs::read(journal.path().join("config/schedules.json")).unwrap(),
    )
    .unwrap();
    assert!(raw.get("owner:source:obsidian").is_none() && raw.get("sync:obsidian").is_none());
    assert_eq!(raw["custom"], custom);
    assert_eq!(raw["daily_time"], "00:15");
    assert_eq!(raw["weekly_time"], "03:15");
    assert_eq!(std::fs::read(&config_path).unwrap(), config_before);
    host.shutdown().unwrap();
    server.shutdown_for_retirement().unwrap();
}

#[cfg(all(unix, feature = "in-process-pdf"))]
#[test]
fn authenticated_source_read_sync_and_status_use_the_saved_connection() {
    use crate::foreground::ForegroundProcessingHost;
    use crate::import_route::{
        ConfiguredSources, SavedFolderProvider, SavedFolderRequest, SourceCopyError,
        packaged_import_factory_with_sources,
    };
    use solstone_core_import::{
        BackendName, SyncState, WorkStop, begin_folder, write_sync_state_bound,
    };
    use solstone_core_maintenance::{
        registry::routines,
        schedule_sync::{expected_entry, schedule_name},
    };
    use std::sync::{Condvar, atomic::AtomicUsize};

    struct Provider {
        identity: String,
        calls: AtomicUsize,
        entered: std::sync::mpsc::Sender<()>,
        gate: Arc<(Mutex<bool>, Condvar)>,
    }
    impl SavedFolderProvider for Provider {
        fn copy(
            &self,
            request: SavedFolderRequest<'_>,
            destination: &Path,
        ) -> Result<(), SourceCopyError> {
            assert_eq!(request.backend, BackendName::Obsidian);
            assert_eq!(request.source_identity, self.identity);
            assert_eq!(request.access_material, "synthetic-sync-grant");
            self.calls.fetch_add(1, Ordering::AcqRel);
            self.entered.send(()).unwrap();
            let (lock, changed) = &*self.gate;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = changed.wait(released).unwrap();
            }
            drop(released);
            assert!(!request.stop.is_requested());
            let note = destination.join("note.md");
            std::fs::write(&note, "# Notes\n\ncobalt explicit source sync\n").unwrap();
            let modified: chrono::DateTime<chrono::Utc> = "2026-10-14T09:00:00Z".parse().unwrap();
            std::fs::File::open(note)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(
                    std::time::SystemTime::UNIX_EPOCH
                        + Duration::from_secs(modified.timestamp().try_into().unwrap()),
                ))
                .unwrap();
            Ok(())
        }
    }
    struct Release(Arc<(Mutex<bool>, Condvar)>);
    impl Drop for Release {
        fn drop(&mut self) {
            let (lock, changed) = &*self.0;
            *lock.lock().unwrap() = true;
            changed.notify_all();
        }
    }
    struct ReleaseRestore(PathBuf);
    impl Drop for ReleaseRestore {
        fn drop(&mut self) {
            solstone_core_journal_io::whole_journal_restore::release_target_for_restore(&self.0);
        }
    }
    fn call(server: &crate::MobileServing, method: &str, path: &str, body: &str) -> (u16, Value) {
        let response = server.request_owner(method, path, body).unwrap();
        (
            response.status,
            serde_json::from_str(&response.body).unwrap_or_else(|error| {
                panic!(
                    "{method} {path} status={} body={:?}: {error}",
                    response.status, response.body
                )
            }),
        )
    }

    let _serial = crate::foreground::TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let journal = fixture("http://127.0.0.1:1");
    let config_path = journal.path().join("config/journal.json");
    let mut config: Value = serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
    config["setup"] = json!({"completed_at":1});
    config["processing"] = json!({"history_confirmed":true});
    config["env"] = json!({"PLAUD_ACCESS_TOKEN":"synthetic-readiness-token"});
    std::fs::write(&config_path, config.to_string()).unwrap();
    let mut schedules = serde_json::Map::from_iter([
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
        schedules.insert(name.into(), json!({"enabled":false}));
    }
    for routine in routines() {
        let mut entry = expected_entry(routine);
        entry["enabled"] = json!(false);
        schedules.insert(schedule_name(routine.id), entry);
    }
    write(
        journal.path(),
        "config/schedules.json",
        Value::Object(schedules).to_string(),
    );
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let std::net::SocketAddr::V4(address) = probe.local_addr().unwrap() else {
        unreachable!()
    };
    drop(probe);
    let server =
        crate::MobileServing::start_with_payload(journal.path().to_path_buf(), address, payload())
            .unwrap();
    let retained = tempfile::tempdir().unwrap();
    std::fs::write(retained.path().join("note.md"), "wrong retained source\n").unwrap();
    let mut saved = SyncState::empty(BackendName::Obsidian);
    let identity = begin_folder(
        &mut saved,
        "Selected notes",
        true,
        Some("synthetic-sync-grant"),
    )
    .unwrap();
    saved
        .root_mut()
        .insert("source_path".into(), json!(retained.path()));
    write_sync_state_bound(&server.root, &saved, &WorkStop::new()).unwrap();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (entered_send, entered) = std::sync::mpsc::channel();
    let provider = Arc::new(Provider {
        identity: identity.clone(),
        calls: AtomicUsize::new(0),
        entered: entered_send,
        gate: Arc::clone(&gate),
    });
    let cache = tempfile::tempdir().unwrap();
    let library = cache.path().join("unused-pdfium.so");
    std::fs::write(&library, b"notes never load PDFium").unwrap();
    let sources = Arc::new(ConfiguredSources::new(
        cache.path().to_path_buf(),
        provider.clone(),
    ));
    let host = ForegroundProcessingHost::open_with_imports(
        Arc::clone(&server.root),
        &payload(),
        Arc::new(|| false),
        Arc::new(AtomicBool::new(false)),
        server.backup_dependencies(),
        packaged_import_factory_with_sources(library, sources),
        false,
        false,
    )
    .unwrap();
    host.stop_discovery_for_test();
    // This guard is created after the host, so a failed assertion releases the
    // actual provider before the host's joined shutdown can run.
    let release = Release(Arc::clone(&gate));
    let schedule_path = journal.path().join("config/schedules.json");
    let schedule_before = std::fs::read(&schedule_path).unwrap();
    let config_before = std::fs::read(&config_path).unwrap();
    let resource = "/api/journal/sources/obsidian";
    let run = "/api/journal/sources/obsidian/sync";
    let (status, connection) = call(&server, "GET", resource, "");
    assert_eq!(status, 200, "{connection}");
    assert_eq!(connection["connection"]["source_identity"], identity);
    assert_eq!(connection["connection"]["folder_name"], "Selected notes");
    assert_eq!(connection["connection"]["access_state"], "ready");
    assert!(!connection.to_string().contains("synthetic-sync-grant"));
    assert!(
        !connection
            .to_string()
            .contains(&retained.path().to_string_lossy().into_owned())
    );
    let audio = call(&server, "GET", "/api/journal/sources/audio", "");
    assert_eq!(audio.0, 200);
    assert!(audio.1["connection"].is_null());
    let plaud = call(&server, "GET", "/api/journal/sources/plaud", "");
    assert_eq!(plaud.0, 200);
    assert_eq!(plaud.1["available"], true);
    assert!(!plaud.1.to_string().contains("synthetic-readiness-token"));
    for source in ["oura", "apple", "shell"] {
        assert_eq!(
            call(
                &server,
                "GET",
                &format!("/api/journal/sources/{source}"),
                ""
            )
            .0,
            400
        );
        assert_eq!(
            call(
                &server,
                "POST",
                &format!("/api/journal/sources/{source}/sync"),
                ""
            )
            .0,
            400
        );
    }
    assert_eq!(
        call(&server, "POST", run, r#"{"args":["--path","/unselected"]}"#).0,
        400
    );
    for request_id in ["", "not/a/reference", &"x".repeat(65)] {
        assert_eq!(
            call(
                &server,
                "POST",
                run,
                &json!({"request_id":request_id}).to_string()
            )
            .0,
            400
        );
    }
    let request_id = "fixture-client-sync-01";
    let status_path = format!("{run}/{request_id}");
    let run_body = json!({"request_id":request_id}).to_string();
    let before_post = call(&server, "GET", &status_path, "");
    assert_eq!(before_post.0, 200);
    assert_eq!(before_post.1["request_id"], request_id);
    assert_eq!(before_post.1["state"], "unknown");
    assert_eq!(
        provider.calls.load(Ordering::Acquire),
        0,
        "status never starts work"
    );
    let (lost_response, response_sink) = std::os::unix::net::UnixStream::pair().unwrap();
    response_sink.set_nonblocking(true).unwrap();
    let descriptor: std::os::fd::OwnedFd = response_sink.into();
    let lost_head = server
        .open_owner_response(
            crate::next_owner_request_id(),
            "POST",
            run,
            &[("content-type".into(), "application/json".into())],
            run_body.as_bytes().to_vec(),
            std::fs::File::from(descriptor),
        )
        .unwrap();
    // Lose both app reply channels before learning any server reference. The
    // retained owner response task still hands this request to the same worker.
    drop(lost_head);
    drop(lost_response);
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let active = call(&server, "GET", &status_path, "");
    assert_eq!(active.0, 200);
    assert_eq!(active.1["terminal"], false);
    assert!(
        matches!(active.1["state"].as_str(), Some("accepted" | "running")),
        "{}",
        active.1
    );
    assert_eq!(active.1["request_id"], request_id);
    let reference = active.1["reference"].as_str().unwrap().to_owned();
    assert!(reference.starts_with("owner-source-obsidian-request-"));
    let duplicate = call(&server, "POST", run, &run_body);
    assert_eq!(duplicate.0, 202);
    assert_eq!(duplicate.1["request_id"], request_id);
    assert_eq!(duplicate.1["reference"], reference);
    assert_eq!(duplicate.1["reused"], true);
    assert_eq!(duplicate.1["submitted"], false);
    drop(release);
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let terminal = loop {
        let (status, body) = call(&server, "GET", &status_path, "");
        assert_eq!(status, 200, "{body}");
        if body["terminal"] == true {
            break body;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "sync has no terminal: {body}; {:?}",
            host.queue().history()
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    host.queue().join_callable_workers().unwrap();
    assert_eq!(terminal["exit_status"], "ok");
    assert_eq!(terminal["request_id"], request_id);
    let duplicate_completed = call(&server, "POST", run, &run_body);
    assert_eq!(duplicate_completed.0, 202);
    assert_eq!(duplicate_completed.1["reused"], true);
    assert_eq!(duplicate_completed.1["reference"], reference);
    assert_eq!(provider.calls.load(Ordering::Acquire), 1);
    let read = server
        .request_owner(
            "GET",
            "/app/transcripts/api/read/20261014?transcripts=1",
            "",
        )
        .unwrap();
    assert_eq!(read.status, 200);
    assert!(
        read.body.contains("cobalt explicit source sync"),
        "{}",
        read.body
    );
    assert!(!read.body.contains("wrong retained source"));
    assert_eq!(
        std::fs::read(&schedule_path).unwrap(),
        schedule_before,
        "sync-now creates no recurrence or new default"
    );
    assert_eq!(std::fs::read(&config_path).unwrap(), config_before);
    let unknown = call(
        &server,
        "GET",
        "/api/journal/sources/audio/sync/fixture-client-sync-01",
        "",
    );
    assert_eq!(unknown.0, 200);
    assert_eq!(unknown.1["state"], "unknown");
    assert_eq!(unknown.1["terminal"], false);
    let state_path = journal.path().join("imports/obsidian.json");
    let valid_state = std::fs::read(&state_path).unwrap();
    std::fs::write(&state_path, b"{unreadable state").unwrap();
    assert_eq!(call(&server, "GET", resource, "").0, 422);
    assert_eq!(std::fs::read(&state_path).unwrap(), b"{unreadable state");
    std::fs::write(&state_path, valid_state).unwrap();
    let valid_state = std::fs::read(&state_path).unwrap();
    let legacy = br#"{"source_path":"/legacy/catalogue-only","files":{"kept.md":{"catalogue_fact":"preserved"}}}"#;
    std::fs::write(&state_path, legacy).unwrap();
    let legacy_connection = call(&server, "GET", resource, "");
    assert_eq!(legacy_connection.0, 200, "{legacy_connection:?}");
    assert!(legacy_connection.1["connection"].is_null());
    assert_eq!(std::fs::read(&state_path).unwrap(), legacy);
    let partial = br#"{"files":{},"remembered":true}"#;
    std::fs::write(&state_path, partial).unwrap();
    assert_eq!(call(&server, "GET", resource, "").0, 422);
    assert_eq!(std::fs::read(&state_path).unwrap(), partial);
    std::fs::write(&state_path, valid_state).unwrap();
    solstone_core_journal_io::whole_journal_restore::hold_target_for_restore(journal.path());
    let hold = ReleaseRestore(journal.path().to_path_buf());
    for (method, path) in [("GET", resource), ("POST", run)] {
        let refusal = server.request_owner(method, path, "").unwrap();
        assert_eq!(refusal.status, 302);
        assert!(refusal.body.contains("href=\"/init\""), "{}", refusal.body);
    }
    assert_eq!(provider.calls.load(Ordering::Acquire), 1);
    drop(hold);
    assert_eq!(call(&server, "GET", resource, "").0, 200);
    host.shutdown().unwrap();
    assert_eq!(call(&server, "POST", run, &run_body).0, 503);
    assert_eq!(call(&server, "GET", &status_path, "").0, 503);
    assert_eq!(
        host.queue()
            .history()
            .iter()
            .filter(|row| row.reference == reference)
            .count(),
        1,
        "duplicate delivery must create only one queue invocation"
    );
    assert_eq!(provider.calls.load(Ordering::Acquire), 1);
    let original = tempfile::tempdir().unwrap();
    std::fs::rename(journal.path(), original.path().join("original")).unwrap();
    std::fs::create_dir(journal.path()).unwrap();
    assert!(server.request_owner("GET", resource, "").is_err());
    assert!(server.request_owner("POST", run, "").is_err());
    server.shutdown_for_retirement().unwrap();
    assert!(
        std::fs::read_dir(journal.path()).unwrap().next().is_none(),
        "replacement root must remain an empty tree"
    );
}
