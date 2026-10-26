// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Synthetic producer composition and checked-retirement evidence.

use super::*;
use axum::Json;
use axum::extract::State;
use chrono::Timelike;
use serde_json::json;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

fn write(root: &Path, relative: &str, bytes: impl AsRef<[u8]>) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

async fn until(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("effectful producer checkpoint");
}

fn state(root: &Path, week: &str) -> Option<String> {
    let bytes = std::fs::read(root.join(format!("health/weekly-heads-up/{week}.json"))).ok()?;
    serde_json::from_slice::<Value>(&bytes).ok()?["state"]
        .as_str()
        .map(str::to_owned)
}

fn week(root: &Path, day: &str, finished: bool) {
    write(
        root,
        &format!("reflections/weekly/{day}.json"),
        json!({
            "memories":[{"text":"private synthetic memory sentinel"}]
        })
        .to_string(),
    );
    if finished {
        complete_week(root, day);
    }
}

fn complete_week(root: &Path, day: &str) {
    use solstone_core_cortex::{CallableCortex, CallableRoots};
    use solstone_core_generate::{GenerateRequest, GenerateResponse, GeneratedResponse};
    let payload = root.join("synthetic-payload");
    write(
        &payload,
        "talent/weekly_reflection.md",
        r#"{
"type":"generate",
"max_output_tokens":128
}
synthetic completion"#,
    );
    let generate = |_: &GenerateRequest| {
        Ok(GenerateResponse::Generated(Box::new(GeneratedResponse {
            id: None,
            text: "{}".into(),
            model: "test-model".into(),
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
    let owner = CallableCortex::open(root).unwrap();
    let completed = owner
        .execute(
            CallableRoots {
                talent_root: &payload.join("talent"),
                apps_root: &payload.join("apps"),
                templates_dir: &payload.join("templates"),
            },
            serde_json::Map::from_iter([
                ("name".into(), json!("weekly_reflection")),
                ("day".into(), json!(day)),
                ("prompt".into(), json!("synthetic completed week")),
            ]),
            &generate,
            chrono::Utc::now().timestamp_millis(),
            &|| false,
        )
        .unwrap();
    assert!(completed.completed);
    let log = std::fs::read_to_string(root.join(format!(
        "talents/weekly_reflection/{}.jsonl",
        completed.use_id
    )))
    .unwrap();
    assert!(
        log.lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .any(|event| event["event"] == "finish"
                && event["name"] == "weekly_reflection"
                && event["day"] == day),
        "actual callable completion log: {log}"
    );
}

struct Receiver {
    instance_id: String,
    deliveries: Mutex<Vec<Value>>,
    count: AtomicUsize,
    hold: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

async fn enroll(State(receiver): State<Arc<Receiver>>) -> Json<Value> {
    Json(
        json!({"token":"synthetic-bearer","token_type":"Bearer","instance_id":receiver.instance_id}),
    )
}

async fn dispatch(State(receiver): State<Arc<Receiver>>, Json(body): Json<Value>) -> Json<Value> {
    assert_eq!(body["devices"][0]["token"], "0123456789abcdef");
    assert!(
        !body
            .to_string()
            .contains("private synthetic memory sentinel")
    );
    receiver.deliveries.lock().unwrap().push(body);
    receiver.count.fetch_add(1, Ordering::AcqRel);
    if receiver.hold.load(Ordering::Acquire) {
        receiver.entered.notify_one();
        receiver.release.notified().await;
    }
    Json(json!({"results":[{"token":"0123456789abcdef","outcome":"sent"}]}))
}

async fn weekly_fixture() -> (
    tempfile::TempDir,
    String,
    Arc<Receiver>,
    tokio::task::JoinHandle<()>,
) {
    let root = tempfile::tempdir().unwrap();
    // Put the controlled journal inside the existing delivery window at the
    // real test clock; no test changes the product's window or its clock.
    let offset = 12_i32 - chrono::Utc::now().hour() as i32;
    let zone = format!("Etc/GMT{:+}", -offset);
    write(root.path(), "config/journal.json", json!({"identity":{"timezone":zone},"providers":{"active":{"provider":"test","model":"test-model"}}}).to_string());
    let ca = solstone_core_sol_link::ca::generate_ca().unwrap();
    let id = solstone_core_sol_link::ca::jid_from_spki(ca.spki_der()).unwrap();
    write(root.path(), "link/ca/cert.pem", ca.certificate_pem());
    write(root.path(), "link/ca/private.pem", ca.private_key_pem());
    write(
        root.path(),
        "link/state.json",
        json!({"instance_id":id,"home_label":"synthetic"}).to_string(),
    );
    let cid = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let registered_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut ledger = solstone_core_sol_link::ledger::AuthorizationLedger::new(root.path());
    ledger
        .add(solstone_core_sol_link::ledger::ClientEntry::new(
            cid,
            "synthetic",
            "2026-01-01T00:00:00Z",
            "synthetic-instance",
            solstone_core_sol_link::ledger::ClientRole::Roleless,
        ))
        .unwrap();
    write(
        root.path(),
        "config/push-registry.json",
        json!({"version":2,"devices":[{
            "platform":"ios","cid":cid,"device_token":"0123456789abcdef",
            "bundle_id":"com.solstone.test","environment":"development",
            "push_key":"KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio","registered_at":registered_at
        }]})
        .to_string(),
    );
    write(
        root.path(),
        "config/push-capabilities.json",
        json!({"version":1,"grants":[{
            "platform":"ios","cid":cid,"identity":"0123456789abcdef",
            "push_key":"KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio","registered_at":registered_at
        }]})
        .to_string(),
    );
    solstone_core_sol_link::committed::load_committed_identity(root.path())
        .expect("synthetic committed push identity");
    let receiver = Arc::new(Receiver {
        instance_id: id,
        deliveries: Mutex::new(Vec::new()),
        count: AtomicUsize::new(0),
        hold: AtomicBool::new(false),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = Router::new()
        .route("/reach/push/relay-token", post(enroll))
        .route("/push/dispatch", post(dispatch))
        .with_state(Arc::clone(&receiver));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (root, format!("http://{address}"), receiver, server)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embedded_weekly_catches_up_resumes_pending_and_retry_and_obeys_current_recipients() {
    let (root, portal, receiver, server) = weekly_fixture().await;
    week(root.path(), "20260802", false);
    write(
        root.path(),
        "talents/weekly_reflection/legacy.jsonl",
        concat!(
            "{\"name\":\"weekly_reflection\",\"day\":\"20260802\",\"prompt\":\"synthetic legacy request\"}\n",
            "{\"event\":\"finish\",\"output\":\"{}\",\"disposition\":\"NoOutput\"}\n"
        ),
    );
    for (day, status) in [("20260809", "pending"), ("20260816", "retry")] {
        week(root.path(), day, false);
        write(root.path(), &format!("health/weekly-heads-up/{day}.json"), json!({
            "version":1,"week":day,"state":status,"due":"2026-01-01T00:00:00Z","reselect":true,"failed":[]
        }).to_string());
    }
    let bus = solstone_core_callosum::CallosumSocketServer::bind(
        root.path().join("health/callosum.sock"),
    )
    .await
    .unwrap();
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(weekly_heads_up::subscribe_weekly_heads_up_until_stopped(
        Arc::new(solstone_core_journal_io::JournalRoot::open(root.path()).unwrap()),
        portal.clone(),
        stopped,
        Arc::new(|| true),
    ));
    let ready = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if ["20260802", "20260809", "20260816"]
                .iter()
                .all(|day| state(root.path(), day).as_deref() == Some("done"))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(
        ready.is_ok(),
        "weekly states: {:?}; deliveries: {}",
        ["20260802", "20260809", "20260816"].map(|day| std::fs::read_to_string(
            root.path()
                .join(format!("health/weekly-heads-up/{day}.json"))
        )),
        receiver.count.load(Ordering::Acquire)
    );
    assert_eq!(receiver.count.load(Ordering::Acquire), 3);
    until(|| bus.client_count() > 0).await;
    week(root.path(), "20260823", false);
    // Exercise the actual callable owner, including its durable completion
    // row and bus publication, rather than synthesizing a finish envelope.
    complete_week(root.path(), "20260823");
    until(|| state(root.path(), "20260823").as_deref() == Some("done")).await;
    assert_eq!(receiver.count.load(Ordering::Acquire), 4);
    stop.send_replace(true);
    task.await.unwrap();
    let (stop, stopped) = tokio::sync::watch::channel(false);
    // A newly completed week while stopped is caught up on reopen. Consent
    // has since been removed, so the current registry prevents a delivery.
    week(root.path(), "20260830", true);
    write(
        root.path(),
        "config/push-registry.json",
        r#"{"version":2,"devices":[]}"#,
    );
    let task = tokio::spawn(weekly_heads_up::subscribe_weekly_heads_up_until_stopped(
        Arc::new(solstone_core_journal_io::JournalRoot::open(root.path()).unwrap()),
        portal,
        stopped,
        Arc::new(|| true),
    ));
    until(|| state(root.path(), "20260830").as_deref() == Some("done")).await;
    assert_eq!(
        receiver.count.load(Ordering::Acquire),
        4,
        "reopen must neither duplicate nor send after opt-out"
    );
    stop.send_replace(true);
    task.await.unwrap();
    bus.stop().await;
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn weekly_completion_during_startup_delivery_is_not_lost() {
    let (root, portal, receiver, server) = weekly_fixture().await;
    week(root.path(), "20260802", true);
    receiver.hold.store(true, Ordering::Release);
    let bus = solstone_core_callosum::CallosumSocketServer::bind(
        root.path().join("health/callosum.sock"),
    )
    .await
    .unwrap();
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(weekly_heads_up::subscribe_weekly_heads_up_until_stopped(
        Arc::new(solstone_core_journal_io::JournalRoot::open(root.path()).unwrap()),
        portal,
        stopped,
        Arc::new(|| true),
    ));
    tokio::time::timeout(Duration::from_secs(10), receiver.entered.notified())
        .await
        .unwrap();
    // The startup scan already selected its completed logs and is blocked in
    // the first delivery. A real second completion must survive that overlap.
    week(root.path(), "20260809", true);
    receiver.hold.store(false, Ordering::Release);
    receiver.release.notify_one();
    until(|| state(root.path(), "20260809").as_deref() == Some("done")).await;
    assert_eq!(state(root.path(), "20260802").as_deref(), Some("done"));
    assert_eq!(receiver.count.load(Ordering::Acquire), 2);
    stop.send_replace(true);
    task.await.unwrap();
    bus.stop().await;
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn weekly_stop_retains_an_active_delivery_and_closed_startup_runs_nothing() {
    let (root, portal, receiver, server) = weekly_fixture().await;
    week(root.path(), "20260802", true);
    receiver.hold.store(true, Ordering::Release);
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let mut task = tokio::spawn(weekly_heads_up::subscribe_weekly_heads_up_until_stopped(
        Arc::new(solstone_core_journal_io::JournalRoot::open(root.path()).unwrap()),
        portal.clone(),
        stopped,
        Arc::new(|| true),
    ));
    tokio::time::timeout(Duration::from_secs(10), receiver.entered.notified())
        .await
        .unwrap();
    stop.send_replace(true);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut task)
            .await
            .is_err(),
        "blocking delivery must remain owned through Stop"
    );
    receiver.release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state(root.path(), "20260802").as_deref(), Some("done"));
    week(root.path(), "20260809", true);
    weekly_heads_up::subscribe_weekly_heads_up_until_stopped(
        Arc::new(solstone_core_journal_io::JournalRoot::open(root.path()).unwrap()),
        portal,
        stop.subscribe(),
        Arc::new(|| true),
    )
    .await;
    assert!(state(root.path(), "20260809").is_none());
    assert_eq!(receiver.count.load(Ordering::Acquire), 1);
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embedded_binding_starts_location_reconciliation_and_joins_both_producers() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "config/journal.json",
        r#"{"identity":{"timezone":"UTC"}}"#,
    );
    write(
        root.path(),
        "chronicle/20261001/phone/140000_300/location.jsonl",
        concat!(
            "{\"accuracy\":\"full\",\"gap\":false,\"kind\":\"location\",\"platform\":\"ios\",\"schema\":\"solstone.location.segment/1\",\"source\":\"location\",\"tier\":\"full\"}\n",
            "{\"h_acc\":10,\"lat\":39.7555,\"lon\":-105.2211,\"schema\":\"solstone.location.fix/1\",\"stationary\":false,\"t\":\"2026-10-01T14:00:10Z\"}\n"
        ),
    );
    let bind = || async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let SocketAddr::V4(address) = address else {
            unreachable!()
        };
        bind_embedded_device_door(root.path().to_path_buf(), address)
            .await
            .unwrap()
    };
    let handle = bind().await;
    let output = root.path().join("chronicle/20261001/location.md");
    until(|| {
        output.exists()
            && root
                .path()
                .join("chronicle/20261001/health/location.json")
                .exists()
    })
    .await;
    let content = std::fs::read(&output).unwrap();
    assert!(!content.is_empty());
    assert!(
        root.path()
            .join("chronicle/20261001/health/location.json")
            .exists()
    );
    handle.shutdown_and_wait().await;
    handle.shutdown_and_wait().await;
    assert!(handle.location_task.lock().await.is_none());
    assert!(handle.weekly_heads_up_task.lock().await.is_none());
    let reopened = bind().await;
    reopened.shutdown_and_wait().await;
    assert_eq!(
        std::fs::read(output).unwrap(),
        content,
        "unchanged marker must stay idempotent"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn weekly_recovery_drains_delivery_and_rescans_restored_completion_without_bus() {
    use crate::recovery_drain::{relinquish_admitted_work, resume_after_whole_journal_restore};
    let (root, portal, receiver, server) = weekly_fixture().await;
    let consent = std::fs::read(root.path().join("config/push-registry.json")).unwrap();
    let retained = tempfile::tempdir().unwrap();
    write(
        retained.path(),
        "config/journal.json",
        std::fs::read(root.path().join("config/journal.json")).unwrap(),
    );
    week(retained.path(), "20260809", true);
    week(root.path(), "20260802", true);
    receiver.hold.store(true, Ordering::Release);
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(weekly_heads_up::subscribe_weekly_heads_up_until_stopped(
        Arc::new(solstone_core_journal_io::JournalRoot::open(root.path()).unwrap()),
        portal,
        stopped,
        Arc::new(|| true),
    ));
    tokio::time::timeout(Duration::from_secs(10), receiver.entered.notified())
        .await
        .unwrap();
    let target = root.path().to_path_buf();
    let mut drain = tokio::task::spawn_blocking(move || relinquish_admitted_work(&target));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut drain)
            .await
            .is_err()
    );
    receiver.hold.store(false, Ordering::Release);
    receiver.release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), drain)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(state(root.path(), "20260802").as_deref(), Some("done"));
    // Copy a real maintained completion produced in a separate retained
    // journal. No completion bus exists here; only recovery resume can rescan it.
    write(
        root.path(),
        "reflections/weekly/20260809.json",
        std::fs::read(retained.path().join("reflections/weekly/20260809.json")).unwrap(),
    );
    for entry in std::fs::read_dir(retained.path().join("talents/weekly_reflection")).unwrap() {
        let entry = entry.unwrap();
        write(
            root.path(),
            &format!(
                "talents/weekly_reflection/{}",
                entry.file_name().to_str().unwrap()
            ),
            std::fs::read(entry.path()).unwrap(),
        );
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(state(root.path(), "20260809").is_none());
    assert_eq!(receiver.count.load(Ordering::Acquire), 1);
    assert!(!task.is_finished());
    resume_after_whole_journal_restore(root.path());
    until(|| state(root.path(), "20260809").as_deref() == Some("done")).await;
    assert_eq!(receiver.count.load(Ordering::Acquire), 2);
    assert_eq!(
        std::fs::read(root.path().join("config/push-registry.json")).unwrap(),
        consent
    );
    let target = root.path().to_path_buf();
    tokio::task::spawn_blocking(move || relinquish_admitted_work(&target))
        .await
        .unwrap()
        .unwrap();
    stop.send_replace(true);
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    resume_after_whole_journal_restore(root.path());
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn map_recovery_joins_admitted_pass_rebuilds_unchanged_source_and_stops_while_paused() {
    use crate::recovery_drain::{relinquish_admitted_work, resume_after_whole_journal_restore};
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "config/journal.json",
        r#"{"identity":{"timezone":"UTC"}}"#,
    );
    write(
        root.path(),
        "chronicle/20261001/phone/140000_300/location.jsonl",
        concat!(
            "{\"accuracy\":\"full\",\"gap\":false,\"kind\":\"location\",\"platform\":\"ios\",\"schema\":\"solstone.location.segment/1\",\"source\":\"location\",\"tier\":\"full\"}\n",
            "{\"h_acc\":10,\"lat\":39.7555,\"lon\":-105.2211,\"schema\":\"solstone.location.fix/1\",\"stationary\":false,\"t\":\"2026-10-01T14:00:10Z\"}\n"
        ),
    );
    let binding = Arc::new(solstone_core_journal_io::JournalRoot::open(root.path()).unwrap());
    let bridge = recovery_watch::RecoveryWatch::new("map-recovery-proof", Arc::clone(&binding));
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let entered = Arc::new(tokio::sync::Notify::new());
    let notify = Arc::clone(&entered);
    let (release, released) = std::sync::mpsc::channel();
    let released = Mutex::new(released);
    let calls = AtomicUsize::new(0);
    let active = Arc::new(move || {
        // The first call is the async loop preflight; the second is the
        // actual blocking pass, after admission acquired its target lease.
        if calls.fetch_add(1, Ordering::AcqRel) == 1 {
            notify.notify_one();
            released.lock().unwrap().recv().unwrap();
        }
        true
    });
    let task = tokio::spawn(solstone_core_map_web::run_maintenance_until_stopped(
        binding,
        stopped,
        active,
        bridge.subscribe(),
    ));
    tokio::time::timeout(Duration::from_secs(10), entered.notified())
        .await
        .unwrap();
    let output = root.path().join("chronicle/20261001/location.md");
    let target = root.path().to_path_buf();
    let mut drain = tokio::task::spawn_blocking(move || relinquish_admitted_work(&target));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut drain)
            .await
            .is_err()
    );
    assert!(!output.exists());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), drain)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!output.exists(), "held pass must relinquish publication");
    resume_after_whole_journal_restore(root.path());
    until(|| output.is_file()).await;
    let content = std::fs::read(&output).unwrap();
    assert!(!content.is_empty());
    // Drain again after the reconciler has remembered this exact source.
    let target = root.path().to_path_buf();
    tokio::task::spawn_blocking(move || relinquish_admitted_work(&target))
        .await
        .unwrap()
        .unwrap();
    std::fs::remove_file(&output).unwrap();
    resume_after_whole_journal_restore(root.path());
    until(|| output.is_file()).await;
    assert_eq!(std::fs::read(&output).unwrap(), content);
    let target = root.path().to_path_buf();
    tokio::task::spawn_blocking(move || relinquish_admitted_work(&target))
        .await
        .unwrap()
        .unwrap();
    stop.send_replace(true);
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    resume_after_whole_journal_restore(root.path());
    drop(bridge);
}
