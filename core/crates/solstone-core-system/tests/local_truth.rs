use std::sync::Arc;

use serde_json::json;
use solstone_core_local::Platform;
use solstone_core_local::install::{archive, manifest, pins};
use solstone_core_system::provider_runtime::{
    LocalLaunchConfig, LocalRuntimeShared, LocalTruthConfig, LocalTruthSeam, ProviderFence,
    ProviderName, ProviderRuntimeState, ReasonCode, RuntimePhase, TruthObservationSeam,
};

fn fence(attempt: u32) -> ProviderFence {
    ProviderFence {
        incarnation: "test".into(),
        generation: 0,
        fingerprint: None,
        attempt,
    }
}

fn ready_journal() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("solstone-local-truth-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let cache = pins::cache_root(&root);
    let runtime = cache.join("bin/aarch64-apple-darwin/b10068");
    let model = cache.join("models/local__qwen3.5-4b");
    std::fs::create_dir_all(&runtime).expect("runtime directory");
    std::fs::create_dir_all(&model).expect("model directory");
    std::fs::write(runtime.join("llama-server"), b"#!/bin/sh\nexit 0\n").expect("runtime");
    archive::make_executable(&runtime.join("llama-server")).expect("executable runtime");
    std::fs::write(model.join("Qwen3.5-4B-Q4_K_M.gguf"), b"model").expect("model");
    std::fs::write(model.join("mmproj-F16.gguf"), b"projector").expect("projector");
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

fn observe(
    root: &std::path::Path,
    attempt: u32,
) -> (
    solstone_core_system::provider_runtime::ProviderTruthObservation,
    Arc<LocalRuntimeShared>,
) {
    let shared = Arc::new(LocalRuntimeShared::default());
    let mut seam = LocalTruthSeam::with_config(
        shared.clone(),
        LocalTruthConfig {
            journal_path: root.into(),
            platform: Platform::Darwin,
            nvidia_probe: None,
            vulkan_devices: Vec::new(),
        },
    );
    let state = ProviderRuntimeState::new(ProviderName::Local);
    let fence = fence(attempt);
    seam.dispatch_truth(&state, &fence);
    let observation = shared.wait_for_truth_result(&fence);
    (observation, shared)
}

#[test]
fn ac11_truth_fingerprint_is_stable_and_retired_macos_models_resolve_to_native_4b() {
    let root = ready_journal();
    let (first, _) = observe(&root, 1);
    let (second, _) = observe(&root, 2);
    assert_eq!(first.phase, RuntimePhase::Starting);
    assert_eq!(
        first.reason_code,
        Some(ReasonCode::known("launch-requested"))
    );
    assert_eq!(first.desired_fingerprint, second.desired_fingerprint);
    std::fs::create_dir_all(root.join("config")).expect("config directory");
    std::fs::write(
        root.join("config/journal.json"),
        json!({"providers":{"active":{"provider":"local","model":"gemma-4-26b-a4b-it-mlx-4bit"}}})
            .to_string(),
    )
    .expect("legacy config");
    let (legacy, shared) = observe(&root, 3);
    assert_eq!(legacy.desired_fingerprint, first.desired_fingerprint);
    let launch = shared
        .launch_request_for(&legacy.desired_fingerprint)
        .expect("native launch request");
    let LocalLaunchConfig::Metal {
        common,
        binary_path,
        ..
    } = launch
    else {
        panic!("Darwin must use Metal")
    };
    assert_eq!(common.model_id, "local/qwen3.5-4b");
    assert_eq!(
        common.desired_fingerprint_json["binary_path"],
        binary_path.as_deref().expect("binary path")
    );
    assert_eq!(
        common.desired_fingerprint_json["projector_path"],
        common.mmproj_path.as_deref().expect("projector path")
    );
    assert!(
        common.desired_fingerprint_json["artifact_target_fingerprint_sha256"]
            .as_str()
            .is_some_and(|value| value.len() == 64)
    );
    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn ac11_truth_unavailable_on_missing_journal() {
    let root = std::env::temp_dir().join("solstone-local-truth-missing");
    let _ = std::fs::remove_dir_all(&root);
    let (result, _) = observe(&root, 3);
    assert_eq!(result.phase, RuntimePhase::StateUnavailable);
    assert_eq!(
        result.reason_code,
        Some(ReasonCode::known("truth-observation-failed"))
    );
    assert!(result.boot_required);
}
