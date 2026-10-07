use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use solstone_core_local::VulkanDevice;
use solstone_core_local::install::{archive, manifest, pins};
use solstone_core_local::nvidia::NvidiaProbe;
use solstone_core_local::plan::{PlanOutcome, plan};
use solstone_core_system::provider_runtime::{
    LocalHost, LocalLaunchConfig, LocalRuntimeShared, LocalTruthConfig, LocalTruthSeam,
    ProviderFence, ProviderName, ProviderRuntimeState, ReasonCode, RuntimePhase,
    TruthObservationSeam,
};
use solstone_core_system::vulkan_observe::VulkanObservation;

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
    let (release, _, _, _) = pins::vulkan_pin("aarch64-apple-darwin").expect("Darwin runtime pin");
    let runtime = cache.join("bin").join("aarch64-apple-darwin").join(release);
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
    let (observation, shared, _) = observe_with(
        root,
        LocalHost::Darwin,
        None,
        VulkanObservation {
            devices: Vec::new(),
            succeeded: true,
        },
        attempt,
    );
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
    platform: LocalHost,
    nvidia_probe: Option<NvidiaProbe>,
    vulkan: VulkanObservation,
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
            windows_package: None,
            journal_path: root.into(),
            platform,
            arch: "x86_64",
            nvidia_probe,
            vulkan,
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
        LocalHost::Linux,
        Some(undetected_probe()),
        VulkanObservation {
            devices: vec![hardware],
            succeeded: true,
        },
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
    let (observation, shared, _) = observe_with(
        &root,
        LocalHost::Linux,
        Some(undetected_probe()),
        VulkanObservation {
            devices,
            succeeded: true,
        },
        1,
    );
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
        LocalHost::Linux,
        Some(undetected_probe()),
        VulkanObservation {
            devices: Vec::new(),
            succeeded: true,
        },
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

#[test]
fn windows_host_reports_gpu_or_package_status() {
    let root = var_tmp("windows-host-tag-blocks");
    let hardware = VulkanDevice {
        index: 0,
        name: "NVIDIA RTX".into(),
        device_type: Some(1),
        vram_mib: 16_384,
    };
    // Succeeded vulkan with missing package -> package-unavailable
    let (observation, shared, _) = observe_with(
        &root,
        LocalHost::Windows,
        Some(undetected_probe()),
        VulkanObservation {
            devices: vec![hardware],
            succeeded: true,
        },
        1,
    );
    assert_eq!(observation.phase, RuntimePhase::HostBlocked);
    assert_eq!(
        observation.reason_code.as_ref().map(ReasonCode::as_str),
        Some("package-unavailable")
    );
    assert!(
        shared
            .launch_request_for(&observation.desired_fingerprint)
            .is_none()
    );

    // Empty or failed vulkan -> gpu-unavailable
    for obs in [
        VulkanObservation {
            devices: Vec::new(),
            succeeded: true,
        },
        VulkanObservation {
            devices: Vec::new(),
            succeeded: false,
        },
    ] {
        let (observation, shared, _) =
            observe_with(&root, LocalHost::Windows, Some(undetected_probe()), obs, 1);
        assert_eq!(observation.phase, RuntimePhase::HostBlocked);
        assert_eq!(
            observation.reason_code.as_ref().map(ReasonCode::as_str),
            Some("gpu-unavailable")
        );
        assert!(
            shared
                .launch_request_for(&observation.desired_fingerprint)
                .is_none()
        );
    }
    let _ = std::fs::remove_dir_all(root);
}

fn write_current_model(root: &Path) {
    let model = pins::cache_root(root).join("models/local__qwen3.5-4b");
    std::fs::create_dir_all(&model).expect("model directory");
    std::fs::write(model.join("Qwen3.5-4B-Q4_K_M.gguf"), b"model").expect("model");
    std::fs::write(model.join("mmproj-F16.gguf"), b"projector").expect("projector");
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

fn write_config(root: &Path, config: serde_json::Value) {
    std::fs::create_dir_all(root.join("config")).expect("config directory");
    std::fs::write(root.join("config/journal.json"), config.to_string()).expect("config");
}

/// Observes truth `observations` times through one seam whose installer only
/// records the journals it was asked to install into.
fn follow_launches(root: &Path, platform: LocalHost, observations: u32) -> Vec<PathBuf> {
    let launched = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorder = launched.clone();
    let shared = Arc::new(LocalRuntimeShared::default());
    let mut seam = LocalTruthSeam::with_config(
        shared.clone(),
        LocalTruthConfig {
            windows_package: None,
            journal_path: root.into(),
            platform,
            arch: "x86_64",
            nvidia_probe: Some(undetected_probe()),
            vulkan: VulkanObservation {
                devices: Vec::new(),
                succeeded: true,
            },
        },
    )
    .with_installer(Arc::new(move |journal: &Path| {
        recorder.lock().unwrap().push(journal.to_path_buf());
        Ok(())
    }));
    let state = ProviderRuntimeState::new(ProviderName::Local);
    for attempt in 1..=observations {
        let fence = fence(attempt);
        seam.dispatch_truth(&state, &fence);
        let observation = shared.wait_for_truth_result(&fence);
        assert_eq!(observation.phase, RuntimePhase::ArtifactNotReady);
    }
    launched.lock().unwrap().clone()
}

#[test]
fn an_installed_model_with_its_runtime_behind_the_pin_starts_the_installer_once() {
    for (name, platform) in [
        ("follow-linux", LocalHost::Linux),
        ("follow-darwin", LocalHost::Darwin),
    ] {
        let root = var_tmp(name);
        write_current_model(&root);
        write_config(&root, json!({"providers":{"active":{"provider":"local"}}}));
        assert_eq!(
            follow_launches(&root, platform, 2),
            vec![root.clone()],
            "{name}"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}

#[test]
fn the_installer_is_not_started_for_an_owner_who_does_not_think_locally() {
    for (name, config) in [
        ("follow-no-config", None),
        (
            "follow-cloud-active",
            Some(json!({"providers":{"active":{"provider":"openai"}}})),
        ),
        (
            "follow-own-endpoint",
            Some(json!({"providers":{"active":{"provider":"local"},
                "local":{"endpoint_url":"http://127.0.0.1:9","served_model_id":"m"}}})),
        ),
    ] {
        let root = var_tmp(name);
        write_current_model(&root);
        if let Some(config) = config {
            write_config(&root, config);
        }
        let launched = follow_launches_any_phase(&root);
        assert!(launched.is_empty(), "{name}: {launched:?}");
        let _ = std::fs::remove_dir_all(root);
    }
    let root = var_tmp("follow-never-installed");
    write_config(&root, json!({"providers":{"active":{"provider":"local"}}}));
    assert!(follow_launches(&root, LocalHost::Linux, 1).is_empty());
    let _ = std::fs::remove_dir_all(root);
}

fn follow_launches_any_phase(root: &Path) -> Vec<PathBuf> {
    let launched = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorder = launched.clone();
    let shared = Arc::new(LocalRuntimeShared::default());
    let mut seam = LocalTruthSeam::with_config(
        shared.clone(),
        LocalTruthConfig {
            windows_package: None,
            journal_path: root.into(),
            platform: LocalHost::Linux,
            arch: "x86_64",
            nvidia_probe: Some(undetected_probe()),
            vulkan: VulkanObservation {
                devices: Vec::new(),
                succeeded: true,
            },
        },
    )
    .with_installer(Arc::new(move |journal: &Path| {
        recorder.lock().unwrap().push(journal.to_path_buf());
        Ok(())
    }));
    let state = ProviderRuntimeState::new(ProviderName::Local);
    let fence = fence(1);
    seam.dispatch_truth(&state, &fence);
    launched.lock().unwrap().clone()
}

fn windows_test_package() -> solstone_core_local::install::windows_engine::WindowsLlamaPackage {
    solstone_core_local::install::windows_engine::WindowsLlamaPackage {
        package_root: PathBuf::from(r"C:\test\package"),
        engine: PathBuf::from(r"C:\test\package\bin\llama-server.exe"),
        loader: PathBuf::from(r"C:\test\package\bin\vulkan-1.dll"),
        probe: PathBuf::from(r"C:\test\package\bin\solstone-vulkan-probe.exe"),
        engine_sha256: "0".repeat(64),
        loader_sha256: "1".repeat(64),
        probe_sha256: "2".repeat(64),
    }
}

fn write_stale_model(root: &Path) {
    let model = pins::cache_root(root).join("models/local__qwen3.5-4b");
    std::fs::create_dir_all(&model).expect("model directory");
    std::fs::write(model.join("Qwen3.5-4B-Q4_K_M.gguf"), b"model").expect("model");
    std::fs::write(model.join("mmproj-F16.gguf"), b"projector").expect("projector");
    let mut identity = pins::model_identity("local/qwen3.5-4b").expect("model pin");
    if let Some(obj) = identity.as_object_mut() {
        obj.insert(
            "filename".into(),
            serde_json::Value::String("other.gguf".into()),
        );
    }
    let model_manifest = manifest::build_manifest(
        "local",
        "local-model",
        "test",
        json!({ "pin_identity": identity }),
        manifest::inventory_for_tree(&model, "model").unwrap(),
        None,
        None,
    )
    .unwrap();
    manifest::write_manifest(&manifest::artifact_manifest_path(&model), &model_manifest).unwrap();
}

fn write_install_status(root: &Path, status: &solstone_core_local::install::status::InstallStatus) {
    let path = solstone_core_local::install::status::status_path(root, "local");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("status parent directory");
    }
    let bytes = serde_json::to_vec_pretty(status).expect("serialize status");
    std::fs::write(path, bytes).expect("write status");
}

fn old_test_status() -> solstone_core_local::install::status::InstallStatus {
    solstone_core_local::install::status::InstallStatus {
        schema_version: 1,
        provider: "local".into(),
        revision: 1,
        install_state: "failed".into(),
        attempt_id: Some("attempt-old".into()),
        target_fingerprint_json: None,
        target_fingerprint_sha256: None,
        started_at: Some("2026-09-01T00:00:00Z".into()),
        last_transition_at: Some("2026-09-01T00:00:00Z".into()),
        last_progress_at: Some("2026-09-01T00:00:00Z".into()),
        completed_at: Some("2026-09-01T00:00:00Z".into()),
        progress_bytes_received: None,
        progress_bytes_total: None,
        install_error: Some("old failure".into()),
        error_code: Some("download_failed".into()),
        owner: None,
    }
}

fn setup_windows_follow_fixture(root: &Path) {
    write_stale_model(root);
    write_config(
        root,
        json!({ "providers": { "active": { "provider": "local" } } }),
    );
    write_install_status(root, &old_test_status());
}

#[test]
fn windows_follow_launches_once_when_the_model_pin_moves() {
    let root = var_tmp("windows-follow-pin-move");
    setup_windows_follow_fixture(&root);
    let pkg = windows_test_package();

    let readiness = solstone_core_local::install::readiness::inspect_local_present_with_package(
        serde_json::Map::from_iter([
            (
                "journal".into(),
                serde_json::Value::String(root.display().to_string()),
            ),
            (
                "model_id".into(),
                serde_json::Value::String("local/qwen3.5-4b".into()),
            ),
            ("backend".into(), serde_json::Value::String("vulkan".into())),
            (
                "artifact_key".into(),
                serde_json::Value::String("x86_64-windows".into()),
            ),
        ]),
        Some(pkg.clone()),
    );
    assert_eq!(readiness["proof"]["binary"]["status"], "ready");
    assert_eq!(readiness["proof"]["binary"]["reason_code"], "ready");
    assert_eq!(
        readiness["proof"]["model"]["status"],
        "missing-or-mismatched"
    );
    assert_eq!(
        readiness["proof"]["model"]["reason_code"],
        "manifest_pin_mismatch"
    );

    let launched = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorder = launched.clone();
    let shared = Arc::new(LocalRuntimeShared::default());
    let mut seam = LocalTruthSeam::with_config(
        shared.clone(),
        LocalTruthConfig {
            windows_package: Some(pkg),
            journal_path: root.clone(),
            platform: LocalHost::Windows,
            arch: "x86_64",
            nvidia_probe: Some(undetected_probe()),
            vulkan: VulkanObservation {
                devices: vec![VulkanDevice {
                    index: 0,
                    name: "Integrated GPU".into(),
                    device_type: Some(1),
                    vram_mib: 4096,
                }],
                succeeded: true,
            },
        },
    )
    .with_installer(Arc::new(move |journal: &Path| {
        recorder.lock().unwrap().push(journal.to_path_buf());
        Ok(())
    }));

    let state = ProviderRuntimeState::new(ProviderName::Local);
    for attempt in 1..=2 {
        let fence = fence(attempt);
        seam.dispatch_truth(&state, &fence);
        let obs = shared.wait_for_truth_result(&fence);
        assert_eq!(obs.phase, RuntimePhase::ArtifactNotReady);
        assert_eq!(
            obs.reason_code.as_ref().map(ReasonCode::as_str),
            Some("artifact-stale")
        );
    }

    assert_eq!(launched.lock().unwrap().len(), 1);
    assert_eq!(launched.lock().unwrap()[0], root);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn windows_follow_launches_once_for_an_abandoned_install() {
    let root = var_tmp("windows-follow-abandoned");
    setup_windows_follow_fixture(&root);
    let mut status = old_test_status();
    status.install_state = "downloading".into();
    status.attempt_id = Some("attempt-old".into());
    status.error_code = None;
    write_install_status(&root, &status);

    let pkg = windows_test_package();
    let launched = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorder = launched.clone();
    let shared = Arc::new(LocalRuntimeShared::default());
    let mut seam = LocalTruthSeam::with_config(
        shared.clone(),
        LocalTruthConfig {
            windows_package: Some(pkg),
            journal_path: root.clone(),
            platform: LocalHost::Windows,
            arch: "x86_64",
            nvidia_probe: Some(undetected_probe()),
            vulkan: VulkanObservation {
                devices: vec![VulkanDevice {
                    index: 0,
                    name: "Integrated GPU".into(),
                    device_type: Some(1),
                    vram_mib: 4096,
                }],
                succeeded: true,
            },
        },
    )
    .with_installer(Arc::new(move |journal: &Path| {
        recorder.lock().unwrap().push(journal.to_path_buf());
        Ok(())
    }));

    let state = ProviderRuntimeState::new(ProviderName::Local);
    let fence = fence(1);
    seam.dispatch_truth(&state, &fence);
    let obs = shared.wait_for_truth_result(&fence);
    assert_eq!(obs.phase, RuntimePhase::ArtifactNotReady);
    assert_eq!(
        obs.reason_code.as_ref().map(ReasonCode::as_str),
        Some("install-in-progress")
    );
    assert_eq!(launched.lock().unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn windows_follow_does_not_launch() {
    fn run_case(case_name: &str, modify: impl FnOnce(&Path, &mut LocalTruthConfig)) {
        let root = var_tmp(&format!("win-follow-hold-{case_name}"));
        setup_windows_follow_fixture(&root);
        let pkg = windows_test_package();

        let mut config = LocalTruthConfig {
            windows_package: Some(pkg),
            journal_path: root.clone(),
            platform: LocalHost::Windows,
            arch: "x86_64",
            nvidia_probe: Some(undetected_probe()),
            vulkan: VulkanObservation {
                devices: vec![VulkanDevice {
                    index: 0,
                    name: "Integrated GPU".into(),
                    device_type: Some(1),
                    vram_mib: 4096,
                }],
                succeeded: true,
            },
        };
        modify(&root, &mut config);

        let launched = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = launched.clone();
        let shared = Arc::new(LocalRuntimeShared::default());
        let mut seam = LocalTruthSeam::with_config(shared.clone(), config).with_installer(
            Arc::new(move |journal: &Path| {
                recorder.lock().unwrap().push(journal.to_path_buf());
                Ok(())
            }),
        );

        let state = ProviderRuntimeState::new(ProviderName::Local);
        let fence = fence(1);
        seam.dispatch_truth(&state, &fence);
        let _ = shared.wait_for_truth_result(&fence);

        assert!(
            launched.lock().unwrap().is_empty(),
            "case '{case_name}' unexpectedly launched installer: {:?}",
            launched.lock().unwrap()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    // Cancelled status
    run_case("cancelled-status", |root, _| {
        let mut status = old_test_status();
        status.install_state = "failed".into();
        status.error_code = Some("install_cancelled".into());
        write_install_status(root, &status);
    });

    // Cancelled status with fresh seam
    run_case("cancelled-status-fresh-seam", |root, _| {
        let mut status = old_test_status();
        status.install_state = "failed".into();
        status.error_code = Some("install_cancelled".into());
        write_install_status(root, &status);
    });

    // Held lease
    run_case("held-lease", |root, _| {
        let guard =
            solstone_core_local::install::lease::acquire(root, "local").expect("acquire lease");
        std::mem::forget(guard); // keep lease held across observation
    });

    // Activity inside floor
    run_case("recent-activity", |root, _| {
        let mut status = old_test_status();
        let now = chrono::Utc::now().to_rfc3339();
        status.last_progress_at = Some(now.clone());
        status.last_transition_at = Some(now);
        write_install_status(root, &status);
    });

    // No model manifest file
    run_case("no-model-manifest", |root, _| {
        let manifest_path = manifest::artifact_manifest_path(
            &pins::cache_root(root).join("models/local__qwen3.5-4b"),
        );
        let _ = std::fs::remove_file(manifest_path);
    });

    // Active provider not local
    run_case("active-provider-openai", |root, _| {
        write_config(
            root,
            json!({ "providers": { "active": { "provider": "openai" } } }),
        );
    });

    // BYO endpoint
    run_case("byo-endpoint", |root, _| {
        write_config(
            root,
            json!({
                "providers": {
                    "active": { "provider": "local" },
                    "local": {
                        "endpoint_url": "http://127.0.0.1:9999",
                        "served_model_id": "custom-model"
                    }
                }
            }),
        );
    });

    // Damaged manifest
    run_case("damaged-manifest", |root, _| {
        let manifest_path = manifest::artifact_manifest_path(
            &pins::cache_root(root).join("models/local__qwen3.5-4b"),
        );
        std::fs::write(manifest_path, b"[]").expect("write malformed manifest");
    });

    // Unreadable manifest
    run_case("unreadable-manifest", |root, _| {
        use std::os::unix::fs::PermissionsExt;
        let manifest_path = manifest::artifact_manifest_path(
            &pins::cache_root(root).join("models/local__qwen3.5-4b"),
        );
        std::fs::set_permissions(manifest_path, std::fs::Permissions::from_mode(0o000))
            .expect("set mode 0000");
    });

    // Host blocked: bad arch
    run_case("bad-arch", |_, config| {
        config.arch = "aarch64";
    });

    // Host blocked: empty vulkan
    run_case("empty-vulkan", |_, config| {
        config.vulkan.devices.clear();
    });

    // Host blocked: windows_package None
    run_case("missing-package", |_, config| {
        config.windows_package = None;
    });
}
