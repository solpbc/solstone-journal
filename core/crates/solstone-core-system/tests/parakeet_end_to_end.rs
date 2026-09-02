// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Proves AC1: Parakeet launches behind the same `LifecycleSeam` trait Local
//! implements and reaches `Ready` with a fixture executable, no real model,
//! using the real seams/store (no mocks) exactly like
//! `local_end_to_end.rs`'s `ac18_real_coordinator_seams_and_store`.
//!
//! `ParakeetTruthSeam` is wired through the native supervisor. This remains a
//! deliberate lower-level lifecycle regression: it seeds `ProviderRuntimeState`
//! directly and uses `NoopWorkers` so it can prove lifecycle, probe, and the
//! durable store reach `Ready` without driving a truth dispatch.

use std::collections::BTreeMap;
#[cfg(windows)]
use std::io::{Read, Write};
#[cfg(windows)]
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use solstone_core_system::provider_runtime::{
    FileRuntimeStore, NoopWorkers, ParakeetLaunchConfig, ParakeetLifecycleSeam, ParakeetPlacement,
    ParakeetProbeSeam, ParakeetRuntimeShared, ProviderName, ProviderRuntimeCoordinator,
    ProviderRuntimeNow, ProviderRuntimeState, ProviderStopCleanupRequest, ReasonCode,
    ReconcileContext, RuntimeClock, RuntimePhase, VecEventSink,
};

const FIXTURE: &str = env!("CARGO_BIN_EXE_solstone-system-test-child");

struct TestClock {
    millis: AtomicU64,
}
impl RuntimeClock for TestClock {
    fn now_utc_rfc3339(&self) -> String {
        "2026-08-09T12:00:00+00:00".into()
    }
    fn monotonic_seconds(&self) -> f64 {
        self.millis.load(Ordering::Relaxed) as f64 / 1_000.0
    }
    fn sleep(&self, duration: Duration) {
        self.millis.fetch_add(
            u64::try_from(duration.as_millis()).unwrap_or(1).max(1),
            Ordering::Relaxed,
        );
        thread::yield_now();
    }
}

fn journal() -> PathBuf {
    let root = std::env::temp_dir().join(format!("solstone-parakeet-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// The production Windows path requires an executable and model under the
/// signed package root. This fixture creates the same containment shape and
/// only returns 200 after the native provider sends its launch capability.
#[cfg(windows)]
fn fixture_launch_paths(
    journal: &std::path::Path,
) -> (PathBuf, PathBuf, Option<PathBuf>, BTreeMap<String, String>) {
    let package = journal.join("signed-fixture-package");
    let bin = package.join("bin");
    let models = package.join("models");
    std::fs::create_dir_all(&bin).expect("fixture package bin");
    std::fs::create_dir_all(&models).expect("fixture package models");
    let binary = bin.join("parakeet-server.exe");
    std::fs::copy(FIXTURE, &binary).expect("copy fixture into package bin");
    let model = models.join("model.bin");
    std::fs::write(&model, "test-ready-auth").expect("write fixture model marker");
    (
        binary,
        model,
        Some(package),
        BTreeMap::from([("PARAKEET_ATT_CONTEXT".to_owned(), "128".to_owned())]),
    )
}

#[cfg(not(windows))]
fn fixture_launch_paths(
    _: &std::path::Path,
) -> (PathBuf, PathBuf, Option<PathBuf>, BTreeMap<String, String>) {
    (
        PathBuf::from(FIXTURE),
        PathBuf::from("test-ready"),
        None,
        BTreeMap::new(),
    )
}

#[cfg(windows)]
fn unauthenticated_health_is_refused(port: u16) {
    let mut stream =
        TcpStream::connect(("127.0.0.1", port)).expect("connect unauthenticated health");
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .expect("send unauthenticated health");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read refused health");
    assert!(response.starts_with("HTTP/1.1 401"));
}

fn pump(
    coordinator: &ProviderRuntimeCoordinator,
    now: ProviderRuntimeNow,
    state: &mut ProviderRuntimeState,
    processes: &mut Vec<solstone_core_system::provider_runtime::ManagedProcess>,
    shared: &ParakeetRuntimeShared,
    context: &mut ReconcileContext<'_>,
) {
    if let Some(in_flight) = state.start.as_mut()
        && in_flight.result.is_none()
    {
        in_flight.result = Some(shared.wait_for_launch_result(&in_flight.fence));
    }
    if let Some(in_flight) = state.stop_cleanup.as_mut()
        && in_flight.result.is_none()
    {
        in_flight.result = Some(shared.wait_for_stop_cleanup_result(&in_flight.fence));
    }
    if let Some(in_flight) = state.probe.as_mut()
        && in_flight.result.is_none()
    {
        in_flight.result = Some(shared.wait_for_probe_result(&in_flight.fence));
    }
    coordinator.reconcile(now, state, processes, context);
}

#[test]
fn parakeet_launches_through_the_real_lifecycle_seam_and_reaches_ready() {
    let journal = journal();
    let (binary_path, model_path, package_root, env_updates) = fixture_launch_paths(&journal);
    let shared = Arc::new(ParakeetRuntimeShared::default());
    let clock: Arc<dyn RuntimeClock> = Arc::new(TestClock {
        millis: AtomicU64::new(0),
    });
    let mut truth = NoopWorkers;
    let mut lifecycle = ParakeetLifecycleSeam::with_timeouts(
        shared.clone(),
        clock.clone(),
        Duration::from_secs(5),
        Duration::from_millis(1),
        Duration::from_secs(1),
    );
    let mut probe = ParakeetProbeSeam::new(shared.clone(), journal.clone());
    let mut store = FileRuntimeStore::new(
        journal.clone(),
        ProviderName::Parakeet,
        shared.clone(),
        clock,
    );
    let mut sink = VecEventSink::default();
    let coordinator = ProviderRuntimeCoordinator::new();

    let mut state = ProviderRuntimeState::new(ProviderName::Parakeet);
    state.desired_fingerprint = Some("desired-a".to_owned());
    state.has_plan = true;
    state.latest_phase = RuntimePhase::Starting;
    state.next_truth_at = 100.0;
    state.next_probe_at = 100.0;

    shared.record_launch_request(
        Some("desired-a".to_owned()),
        ParakeetLaunchConfig {
            binary_backend: "cpu".to_owned(),
            env_updates,
            gpu_index: None,
            binary_path,
            model_path,
            package_root,
            journal_path: journal.clone(),
            threads: 4,
            desired_fingerprint_json: "{}".to_owned(),
            desired_fingerprint_sha256: "desired-a".to_owned(),
            placement: ParakeetPlacement::Cpu,
        },
    );

    let mut processes = vec![];
    let now = ProviderRuntimeNow {
        monotonic_seconds: 0.0,
    };
    for _ in 0..6 {
        let mut context = ReconcileContext {
            truth: &mut truth,
            lifecycle: &mut lifecycle,
            probe: &mut probe,
            store: &mut store,
            sink: &mut sink,
            gate: None,
        };
        pump(
            &coordinator,
            now,
            &mut state,
            &mut processes,
            &shared,
            &mut context,
        );
        if state.latest_phase == RuntimePhase::Ready {
            break;
        }
    }

    assert_eq!(state.latest_phase, RuntimePhase::Ready);
    assert_eq!(processes.len(), 1);

    let health: serde_json::Value = serde_json::from_slice(
        &std::fs::read(journal.join("health/providers/runtime/parakeet.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(health["phase"], "ready");
    assert_eq!(health["provider"], "parakeet");

    // Parakeet's owned-port file is a distinct service name from its
    // provider name -- health/parakeet-cpp.port, not health/parakeet.port.
    let port_path = journal.join("health/parakeet-cpp.port");
    assert!(port_path.exists());
    let published_port = std::fs::read_to_string(&port_path)
        .expect("port file")
        .trim()
        .parse::<u16>()
        .expect("port number");
    assert_eq!(
        health["process"]["port"].as_u64(),
        Some(u64::from(published_port))
    );
    assert_eq!(health["process"]["name"], "parakeet-server");
    #[cfg(windows)]
    unauthenticated_health_is_refused(published_port);

    state.pending_stop_request = Some(ProviderStopCleanupRequest {
        managed: processes[0].clone(),
        reason_code: ReasonCode::known("intent-removed"),
        target_phase: RuntimePhase::Stopped,
        target_reason_code: Some(ReasonCode::known("cleanup-succeeded")),
        admission_exclusive: false,
        orphaned_start_outcome: false,
    });
    for _ in 0..4 {
        let mut context = ReconcileContext {
            truth: &mut truth,
            lifecycle: &mut lifecycle,
            probe: &mut probe,
            store: &mut store,
            sink: &mut sink,
            gate: None,
        };
        pump(
            &coordinator,
            now,
            &mut state,
            &mut processes,
            &shared,
            &mut context,
        );
        if state.latest_phase == RuntimePhase::Stopped {
            break;
        }
    }
    assert_eq!(state.latest_phase, RuntimePhase::Stopped);
    assert!(processes.is_empty());
    assert!(!port_path.exists());

    let _ = std::fs::remove_dir_all(journal);
}
