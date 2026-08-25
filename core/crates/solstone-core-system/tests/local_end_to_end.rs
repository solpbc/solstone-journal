// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(target_os = "linux")]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use serde_json::json;
use solstone_core_local::install::{archive, manifest, pins};
use solstone_core_local::nvidia::NvidiaProbe;
use solstone_core_local::{ArtifactTrust, Platform};
use solstone_core_system::provider_runtime::{
    FileRuntimeStore, LocalLaunchCommon, LocalLaunchConfig, LocalLifecycleSeam, LocalProbeSeam,
    LocalRuntimeShared, LocalTruthConfig, LocalTruthSeam, ManagedProcess, ProviderName,
    ProviderRuntimeCoordinator, ProviderRuntimeNow, ProviderRuntimeState,
    ProviderStopCleanupRequest, ReasonCode, ReconcileContext, RuntimeClock, RuntimePhase,
    VecEventSink,
};

const FIXTURE: &str = env!("CARGO_BIN_EXE_solstone-system-test-child");

/// A synthetic probe, never the real one: this test drives a CUDA launch to
/// `Ready`, so calling `probe_nvidia_gpu()` would make the assertion a
/// statement about the host's graphics card rather than about the coordinator.
fn nvidia() -> NvidiaProbe {
    NvidiaProbe {
        schema: "solstone-local-nvidia-probe-v1".into(),
        detected: true,
        gpu_index: Some(0),
        gpu_name: Some("test GPU".into()),
        compute_cap: Some("8.9".into()),
        arch: Some("sm_89".into()),
        driver_cuda_major: Some(13),
        vram_mib: Some(16_000),
        unified_memory_mib: None,
        probe_error: None,
    }
}

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
fn journal() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("solstone-local-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let cache = pins::cache_root(&root);
    let runtime = cache.join("bin/aarch64-apple-darwin/b10068");
    let model = cache.join("models/local__qwen3.5-4b");
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::create_dir_all(&model).unwrap();
    std::fs::write(runtime.join("llama-server"), b"#!/bin/sh\nexit 0\n").unwrap();
    archive::make_executable(&runtime.join("llama-server")).unwrap();
    std::fs::write(model.join("Qwen3.5-4B-Q4_K_M.gguf"), b"model").unwrap();
    std::fs::write(model.join("mmproj-F16.gguf"), b"projector").unwrap();
    let runtime_manifest = manifest::build_manifest(
        "local",
        "llama-server-vulkan",
        "test",
        json!({"pin_identity":pins::vulkan_identity("aarch64-apple-darwin").unwrap()}),
        manifest::runtime_inventory(&runtime, &[]).unwrap(),
        None,
        None,
    )
    .unwrap();
    manifest::write_manifest(
        &manifest::artifact_manifest_path(&runtime),
        &runtime_manifest,
    )
    .unwrap();
    let model_manifest = manifest::build_manifest(
        "local",
        "local-model",
        "test",
        json!({"pin_identity":pins::model_identity("local/qwen3.5-4b").unwrap()}),
        manifest::inventory_for_tree(&model, "model").unwrap(),
        None,
        None,
    )
    .unwrap();
    manifest::write_manifest(&manifest::artifact_manifest_path(&model), &model_manifest).unwrap();
    root
}
fn pump(
    c: &ProviderRuntimeCoordinator,
    n: ProviderRuntimeNow,
    s: &mut ProviderRuntimeState,
    p: &mut Vec<ManagedProcess>,
    sh: &LocalRuntimeShared,
    x: &mut ReconcileContext<'_>,
) {
    if let Some(f) = s.truth.as_mut()
        && f.result.is_none()
    {
        f.result = Some(sh.wait_for_truth_result(&f.fence));
    }
    if let Some(f) = s.start.as_mut()
        && f.result.is_none()
    {
        f.result = Some(sh.wait_for_launch_result(&f.fence));
    }
    if let Some(f) = s.stop_cleanup.as_mut()
        && f.result.is_none()
    {
        f.result = Some(sh.wait_for_stop_cleanup_result(&f.fence));
    }
    if let Some(f) = s.probe.as_mut()
        && f.result.is_none()
    {
        f.result = Some(sh.wait_for_probe_result(&f.fence));
    }
    c.reconcile(n, s, p, x);
}
#[test]
fn ac18_real_coordinator_seams_and_store() {
    let journal = journal();
    let shared = Arc::new(LocalRuntimeShared::default());
    let clock: Arc<dyn RuntimeClock> = Arc::new(TestClock {
        millis: AtomicU64::new(0),
    });
    let mut truth = LocalTruthSeam::with_config(
        shared.clone(),
        LocalTruthConfig {
            journal_path: journal.clone(),
            platform: Platform::Darwin,
            nvidia_probe: None,
            vulkan_devices: vec![],
        },
    );
    let mut lifecycle = LocalLifecycleSeam::with_timeouts(
        shared.clone(),
        clock.clone(),
        Duration::from_secs(5),
        Duration::from_millis(1),
        Duration::from_secs(1),
    );
    let mut probe = LocalProbeSeam::new(shared.clone(), journal.clone());
    let mut store =
        FileRuntimeStore::new(journal.clone(), ProviderName::Local, shared.clone(), clock);
    let mut sink = VecEventSink::default();
    let c = ProviderRuntimeCoordinator::new();
    let mut s = ProviderRuntimeState::new(ProviderName::Local);
    let mut ps = vec![];
    let n = ProviderRuntimeNow {
        monotonic_seconds: 0.0,
    };
    {
        let mut x = ReconcileContext {
            truth: &mut truth,
            lifecycle: &mut lifecycle,
            probe: &mut probe,
            store: &mut store,
            sink: &mut sink,
            gate: None,
        };
        c.reconcile(n, &mut s, &mut ps, &mut x);
    }
    let f = s.truth.as_ref().unwrap().fence.clone();
    s.truth.as_mut().unwrap().result = Some(shared.wait_for_truth_result(&f));
    let fp = s
        .truth
        .as_ref()
        .unwrap()
        .result
        .as_ref()
        .unwrap()
        .desired_fingerprint
        .clone()
        .unwrap();
    shared.record_launch_request(
        Some(fp.clone()),
        LocalLaunchConfig::Cuda {
            common: LocalLaunchCommon {
                desired_fingerprint_json: json!({"provider":"local","stub":true}),
                desired_fingerprint_sha256: fp,
                model_id: "local/test".into(),
                model_path: "test-ready".into(),
                mmproj_path: None,
            },
            binary_path: Some(FIXTURE.into()),
            lib_dir: None,
            nvidia_probe: nvidia(),
            cuda_embedded_arch_set: vec!["sm_89".into()],
            cuda_min_driver_version: 1,
            cuda_artifact_trust: ArtifactTrust::Trusted,
            cuda_persisted_installed_cuda_target: false,
        },
    );
    for _ in 0..6 {
        let mut x = ReconcileContext {
            truth: &mut truth,
            lifecycle: &mut lifecycle,
            probe: &mut probe,
            store: &mut store,
            sink: &mut sink,
            gate: None,
        };
        pump(&c, n, &mut s, &mut ps, &shared, &mut x);
        if s.latest_phase == RuntimePhase::Ready {
            break;
        }
    }
    assert_eq!(s.latest_phase, RuntimePhase::Ready);
    assert_eq!(ps.len(), 1);
    let port = journal.join("health/local.port");
    let health: serde_json::Value = serde_json::from_slice(
        &std::fs::read(journal.join("health/providers/runtime/local.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(health["phase"], "ready");
    assert!(port.exists());
    let published_port = std::fs::read_to_string(&port)
        .expect("port file")
        .trim()
        .parse::<u16>()
        .expect("port number");
    assert_eq!(
        health["process"]["port"].as_u64(),
        Some(u64::from(published_port))
    );
    assert!(health["incarnation"].is_string());
    assert!(health["generation"].is_u64());
    assert!(health["attempt"].is_u64());
    let mut probed_ready = false;
    for _ in 0..4 {
        let mut x = ReconcileContext {
            truth: &mut truth,
            lifecycle: &mut lifecycle,
            probe: &mut probe,
            store: &mut store,
            sink: &mut sink,
            gate: None,
        };
        pump(&c, n, &mut s, &mut ps, &shared, &mut x);
        if s.latest_reason_code == Some(ReasonCode::known("probe-ready")) {
            probed_ready = true;
            break;
        }
    }
    assert!(
        probed_ready,
        "expected a probe-ready reason code, got {:?}",
        s.latest_reason_code
    );
    s.pending_stop_request = Some(ProviderStopCleanupRequest {
        managed: ps[0].clone(),
        reason_code: ReasonCode::known("intent-removed"),
        target_phase: RuntimePhase::Stopped,
        target_reason_code: Some(ReasonCode::known("cleanup-succeeded")),
        admission_exclusive: false,
        orphaned_start_outcome: false,
    });
    for _ in 0..4 {
        let mut x = ReconcileContext {
            truth: &mut truth,
            lifecycle: &mut lifecycle,
            probe: &mut probe,
            store: &mut store,
            sink: &mut sink,
            gate: None,
        };
        pump(&c, n, &mut s, &mut ps, &shared, &mut x);
        if s.latest_phase == RuntimePhase::Stopped {
            break;
        }
    }
    assert_eq!(s.latest_phase, RuntimePhase::Stopped);
    assert!(!port.exists());
    let _ = std::fs::remove_dir_all(journal);
}
