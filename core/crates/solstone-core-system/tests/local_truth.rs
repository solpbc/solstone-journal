use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use solstone_core_local::install::{archive, manifest, pins};
use solstone_core_local::nvidia::NvidiaProbe;
use solstone_core_local::plan::{PlanOutcome, plan};
use solstone_core_local::{Platform, VulkanDevice};
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
    let (observation, shared, _) = observe_with(root, Platform::Darwin, None, Vec::new(), attempt);
    (observation, shared)
}

fn undetected_probe() -> NvidiaProbe {
    NvidiaProbe {
        schema: "solstone-local-nvidia-probe-v1".into(),
        detected: false,
        gpu_index: None,
        gpu_name: None,
        compute_cap: None,
        arch: None,
        driver_cuda_major: None,
        vram_mib: None,
        unified_memory_mib: None,
        probe_error: None,
    }
}

fn var_tmp(name: &str) -> PathBuf {
    let path = PathBuf::from("/var/tmp").join(format!(
        "solstone-local-truth-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("var tmp journal");
    path
}

fn write_linux_runtime_tree(root: &Path, unflattened: bool) {
    let key = pins::platform_key();
    let (release, _, _, _) = pins::vulkan_pin(&key).expect("vulkan pin for host platform");
    let cache = pins::cache_root(root);
    let runtime = cache.join("bin").join(&key).join(release);
    let model = cache.join("models/local__qwen3.5-4b");
    std::fs::create_dir_all(&runtime).expect("runtime directory");
    std::fs::create_dir_all(&model).expect("model directory");
    std::fs::write(runtime.join("llama-server"), b"#!/bin/sh\nexit 0\n").expect("runtime");
    archive::make_executable(&runtime.join("llama-server")).expect("executable runtime");
    if unflattened {
        let nested = runtime.join("llama-b10068");
        std::fs::create_dir_all(&nested).expect("unflattened lib dir");
        std::fs::write(nested.join("libllama-server-impl.so"), b"library").expect("nested lib");
    }
    std::fs::write(model.join("Qwen3.5-4B-Q4_K_M.gguf"), b"model").expect("model");
    std::fs::write(model.join("mmproj-F16.gguf"), b"projector").expect("projector");
    let runtime_manifest = manifest::build_manifest(
        "local",
        "llama-server-vulkan",
        "test",
        json!({"pin_identity":pins::vulkan_identity(&key).unwrap()}),
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
}

fn observe_with(
    root: &Path,
    platform: Platform,
    nvidia_probe: Option<NvidiaProbe>,
    vulkan_devices: Vec<VulkanDevice>,
    attempt: u32,
) -> (
    solstone_core_system::provider_runtime::ProviderTruthObservation,
    Arc<LocalRuntimeShared>,
    ProviderRuntimeState,
) {
    let shared = Arc::new(LocalRuntimeShared::default());
    let mut seam = LocalTruthSeam::with_config(
        shared.clone(),
        LocalTruthConfig {
            journal_path: root.into(),
            platform,
            nvidia_probe,
            vulkan_devices,
        },
    );
    let state = ProviderRuntimeState::new(ProviderName::Local);
    let fence = fence(attempt);
    seam.dispatch_truth(&state, &fence);
    let observation = shared.wait_for_truth_result(&fence);
    (observation, shared, state)
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

#[test]
fn ac1_linux_cloud_model_starts_local_vulkan_and_sets_ld_library_path() {
    let root = var_tmp("ac1-linux-cloud-model");
    write_linux_runtime_tree(&root, true);
    std::fs::create_dir_all(root.join("config")).expect("config directory");
    std::fs::write(
        root.join("config/journal.json"),
        json!({"providers":{"active":{"provider":"google","model":"google/gemini-3.5-flash"}}})
            .to_string(),
    )
    .expect("cloud-active journal config");
    let hardware = VulkanDevice {
        index: 0,
        name: "Intel Arc".into(),
        device_type: Some(1),
        vram_mib: 8_192,
    };
    let (observation, shared, state) = observe_with(
        &root,
        Platform::Linux,
        Some(undetected_probe()),
        vec![hardware],
        1,
    );
    assert_eq!(
        (
            observation.phase,
            observation.reason_code.as_ref().map(ReasonCode::as_str)
        ),
        (RuntimePhase::Starting, Some("launch-requested"))
    );
    let launch = shared
        .launch_request_for(&observation.desired_fingerprint)
        .expect("linux vulkan launch request");
    let LocalLaunchConfig::Vulkan { common, .. } = &launch else {
        panic!("Linux Vulkan hardware must request a Vulkan launch");
    };
    assert_eq!(common.model_id, "local/qwen3.5-4b");
    let input = launch.assemble_plan_input(&state, 4010);
    assert!(input.lib_dir.is_none(), "AC1 plans with lib_dir unset");
    let planned = match plan(input) {
        PlanOutcome::Launch(plan) => *plan,
        PlanOutcome::Rejected { reason } => panic!("expected launch plan: {reason}"),
    };
    let ld_library_path = planned
        .extra_env
        .get("LD_LIBRARY_PATH")
        .expect("unflattened Vulkan tree must set LD_LIBRARY_PATH");
    assert!(
        ld_library_path.contains("llama-b10068"),
        "LD_LIBRARY_PATH={ld_library_path} must contain the nested llama-b10068 dir"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ac4_software_first_prefers_intel_over_llvmpipe() {
    let root = var_tmp("ac4-software-first");
    write_linux_runtime_tree(&root, false);
    let devices = vec![
        VulkanDevice {
            index: 0,
            name: "llvmpipe".into(),
            device_type: Some(4),
            vram_mib: 8_192,
        },
        VulkanDevice {
            index: 1,
            name: "Intel".into(),
            device_type: Some(1),
            vram_mib: 8_192,
        },
    ];
    let (observation, shared, _) =
        observe_with(&root, Platform::Linux, Some(undetected_probe()), devices, 1);
    assert_eq!(observation.phase, RuntimePhase::Starting);
    assert_eq!(
        observation.reason_code.as_ref().map(ReasonCode::as_str),
        Some("launch-requested")
    );
    let launch = shared
        .launch_request_for(&observation.desired_fingerprint)
        .expect("linux vulkan launch request");
    let LocalLaunchConfig::Vulkan {
        selected_gpu_index,
        selected_gpu_name,
        ..
    } = launch
    else {
        panic!("Linux Vulkan hardware must request a Vulkan launch");
    };
    assert_eq!(
        (selected_gpu_index, selected_gpu_name.as_str()),
        (1, "Intel")
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ac9_manifest_missing_maps_to_manifest_missing_reason() {
    let root = var_tmp("ac9-manifest-missing");
    let (observation, _, _) = observe_with(
        &root,
        Platform::Linux,
        Some(undetected_probe()),
        Vec::new(),
        1,
    );
    assert_eq!(observation.phase, RuntimePhase::ArtifactNotReady);
    assert_eq!(
        observation.reason_code.as_ref().map(ReasonCode::as_str),
        Some("manifest-missing"),
        "inspect reason_code=manifest_missing must map to runtime reason manifest-missing"
    );
    let _ = std::fs::remove_dir_all(root);
}
