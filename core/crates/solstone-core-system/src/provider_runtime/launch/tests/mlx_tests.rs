// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(unix, feature = "full-tests"))]

use super::*;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};

fn fixture() -> (
    tempfile::TempDir,
    PathBuf,
    PathBuf,
    &'static [pins::MlxFilePin],
) {
    let root = tempfile::tempdir().unwrap();
    let helper = root.path().join("mlx-helper");
    fs::write(&helper, b"#!/bin/sh\nexec /bin/sleep 30\n").unwrap();
    make_file_executable(&helper);
    let resources = root.path().join("mlx-swift_Cmlx.bundle/Contents/Resources");
    fs::create_dir_all(&resources).unwrap();
    fs::write(resources.join("default.metallib"), b"fixture shader").unwrap();
    let model = root.path().join("model");
    fs::create_dir(&model).unwrap();
    fs::write(model.join("config.json"), b"fixture").unwrap();
    let pins = Box::leak(Box::new([pins::MlxFilePin {
        name: "config.json",
        size: 7,
        sha256: Box::leak(
            solstone_core_local::install::fingerprint::sha256("fixture").into_boxed_str(),
        ),
    }]));
    (root, helper, model, pins)
}

fn config(journal: &Path, platform: LocalHost) -> LocalTruthConfig {
    LocalTruthConfig {
        journal_path: journal.into(),
        platform,
        arch: "x86_64",
        nvidia_probe: None,
        vulkan: crate::vulkan_observe::VulkanObservation {
            devices: vec![],
            succeeded: false,
        },
        windows_package: None,
    }
}

#[test]
fn never_installed_mlx_failure_does_not_install_and_other_hosts_do_not_inspect_mlx() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("config")).unwrap();
    fs::write(
        root.path().join("config/journal.json"),
        br#"{"providers":{"active":{"provider":"local"}}}"#,
    )
    .unwrap();
    let launches = Arc::new(AtomicUsize::new(0));
    let count = launches.clone();
    let follow = LocalFollow::new(Arc::new(move |_: &Path| {
        count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    let observed = observe_truth_with_mlx(
        &LocalRuntimeShared::default(),
        &config(root.path(), LocalHost::Darwin),
        Some(&follow),
        || json!({"ready":false,"detail":"missing fixture"}),
    );
    assert_eq!(observed.phase, RuntimePhase::ArtifactNotReady);
    assert!(!observed.has_plan);
    assert_eq!(launches.load(Ordering::SeqCst), 0);
    for platform in [LocalHost::Linux, LocalHost::Windows] {
        observe_truth_with_mlx(
            &LocalRuntimeShared::default(),
            &config(root.path(), platform),
            None,
            || panic!("non-Darwin host inspected MLX"),
        );
    }
}

#[test]
fn mlx_prelaunch_checks_bytes_coordinates_and_holds_the_model_directory() {
    let (root, helper, model, pins) = fixture();
    use crate::lifecycle::{HostedServiceKind, ParentLossLedger};
    use crate::process::{InspectResult, ProcessInstanceSource, SystemProcessInstanceSource};
    let InspectResult::Present {
        instance: owner, ..
    } = SystemProcessInstanceSource.inspect(std::process::id())
    else {
        panic!("test process identity unavailable");
    };
    let ledger = ParentLossLedger::open(root.path()).unwrap();
    let active = ledger
        .reserve_generation(owner, [HostedServiceKind::Sense])
        .unwrap();
    ledger.initialize_record(&active).unwrap();
    ledger
        .persist_coordinator_identity(active.generation, owner)
        .unwrap();
    ledger.mark_admitting(active.generation, owner).unwrap();
    let shared = LocalRuntimeShared::default();
    let readiness = mlx_candidate::inspect_fixture(&helper, &model, pins, false);
    let observed = observe_mlx_readiness(&shared, readiness);
    assert!(observed.has_plan);
    let mut launch = shared
        .launch_request_for(&observed.desired_fingerprint)
        .unwrap();
    if let LocalLaunchConfig::Mlx { fixture_pins, .. } = &mut launch {
        *fixture_pins = Some(pins);
    } else {
        panic!("MLX launch was not selected");
    }
    let mut state = ProviderRuntimeState::new(ProviderName::Local);
    state.desired_fingerprint = observed.desired_fingerprint.clone();
    let fence = ProviderFence {
        incarnation: "fixture".into(),
        generation: 1,
        fingerprint: observed.desired_fingerprint,
        attempt: 1,
    };
    let start = |launch: &LocalLaunchConfig| {
        start_local_unix(
            &shared,
            &TestClock::default(),
            launch,
            &state,
            &fence,
            Some(root.path()),
            Duration::ZERO,
            Duration::ZERO,
        )
    };
    // Valid, unchanged bytes must reach the real spawn authority.
    let valid = start(&launch);
    assert_eq!(valid.status, LaunchOutcomeStatus::WarmupTimeout);
    let child = valid.managed.unwrap();
    assert!(solstone_core_artifact_download::generation::hold_write(&model).is_err());
    let mut authority = shared.take_child(&child.id).unwrap();
    authority.terminate(Duration::from_secs(5)).unwrap();
    drop(authority);
    assert!(solstone_core_artifact_download::generation::hold_write(&model).is_ok());
    // Presence can still be true, but changed bytes must never reach spawn.
    fs::write(model.join("config.json"), b"corrupt").unwrap();
    let corrupt = start(&launch);
    assert_eq!(corrupt.status, LaunchOutcomeStatus::LaunchFailed);
    assert_eq!(corrupt.reason_code.as_str(), "artifact-stale");
    assert!(corrupt.managed.is_none());
    fs::write(model.join("config.json"), b"fixture").unwrap();
    let other = root.path().join("other-helper");
    fs::copy(&helper, &other).unwrap();
    if let LocalLaunchConfig::Mlx { binary_path, .. } = &mut launch {
        *binary_path = Some(other.display().to_string());
    }
    let changed = start(&launch);
    assert_eq!(changed.reason_code.as_str(), "artifact-stale");
    assert!(changed.managed.is_none());
    let other_model = root.path().join("other-model");
    fs::create_dir(&other_model).unwrap();
    fs::copy(model.join("config.json"), other_model.join("config.json")).unwrap();
    if let LocalLaunchConfig::Mlx {
        common,
        binary_path,
        ..
    } = &mut launch
    {
        *binary_path = Some(helper.display().to_string());
        common.model_path = other_model.display().to_string();
    }
    let changed = start(&launch);
    assert_eq!(changed.reason_code.as_str(), "artifact-stale");
    assert!(changed.managed.is_none());
}

fn prior_llama_install(root: &Path) {
    use solstone_core_local::install::{archive, manifest};
    let cache = pins::cache_root(root);
    let (release, _, _, _) = pins::vulkan_pin("aarch64-apple-darwin").expect("Darwin runtime pin");
    let runtime = cache.join("bin").join("aarch64-apple-darwin").join(release);
    let model = cache.join("models/local__qwen3.5-4b");
    fs::create_dir_all(&runtime).expect("runtime directory");
    fs::create_dir_all(&model).expect("model directory");
    fs::write(runtime.join("llama-server"), b"#!/bin/sh\nexit 0\n").expect("runtime");
    archive::make_executable(&runtime.join("llama-server")).expect("executable runtime");
    fs::write(model.join("Qwen3.5-4B-Q4_K_M.gguf"), b"model").expect("model");
    fs::write(model.join("mmproj-F16.gguf"), b"projector").expect("projector");
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
}

fn active_journal(root: &Path, provider: &str) {
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/journal.json"),
        json!({"providers":{"active":{"provider":provider,"model":"gemma-4-26b-a4b-it-mlx-4bit"}}})
            .to_string(),
    )
    .unwrap();
}

// Presence mode inspects sizes/control records, never weight contents. Sparse
// files exercise that real path without downloading or qualifying a model.
fn present_mlx(journal: &Path) {
    let directory = pins::mlx_model_directory(journal);
    fs::create_dir_all(&directory).unwrap();
    for pin in pins::MLX_QWEN_PINS.iter() {
        fs::File::create(directory.join(pin.name))
            .unwrap()
            .set_len(pin.size)
            .unwrap();
    }
    fs::write(
        directory.join(".generation.json"),
        serde_json::to_vec(&pins::mlx_model_generation()).unwrap(),
    )
    .unwrap();
    assert_eq!(
        mlx_candidate::inspect_present_default(journal)["ready"],
        true
    );
}

#[test]
fn darwin_uses_mlx_and_preserves_legacy_files_regardless_of_old_migration_marker() {
    let _helper = mlx_candidate::MlxHelperTestGuard::new();
    let root = tempfile::tempdir().unwrap();
    prior_llama_install(root.path());
    active_journal(root.path(), "local");
    let input = Map::from_iter([("journal".into(), json!(root.path()))]);
    assert_eq!(
        solstone_core_local::install::metal_candidate::inspect_present_with(
            &input,
            "aarch64-apple-darwin"
        )
        .unwrap()["ready"],
        true
    );
    present_mlx(root.path());
    let shared = LocalRuntimeShared::default();
    let mut first = None;
    for marker in [
        None,
        Some(r#"{"phase":"preparing"}"#),
        Some(r#"{"phase":"committed"}"#),
        Some("not json"),
    ] {
        let path = root.path().join("config/macos-mlx-migration.json");
        if let Some(marker) = marker {
            fs::write(path, marker).unwrap();
        }
        let observed = observe_truth(&shared, &config(root.path(), LocalHost::Darwin), None);
        assert_eq!(observed.phase, RuntimePhase::Starting);
        assert!(observed.has_plan);
        if let Some(first) = &first {
            assert_eq!(&observed.desired_fingerprint, first);
        } else {
            first = Some(observed.desired_fingerprint.clone());
        }
        let launch = shared
            .launch_request_for(&observed.desired_fingerprint)
            .unwrap();
        let LocalLaunchConfig::Mlx { common, .. } = launch else {
            panic!("Darwin must use MLX")
        };
        assert_eq!(common.model_id, "local/qwen3.5-4b");
        assert!(common.mmproj_path.is_none());
        assert_eq!(
            fs::read(
                pins::cache_root(root.path())
                    .join("models/local__qwen3.5-4b/Qwen3.5-4B-Q4_K_M.gguf")
            )
            .unwrap(),
            b"model"
        );
    }
    // A present but damaged sealed MLX cannot turn the valid old engine into a fallback.
    fs::remove_file(pins::mlx_model_directory(root.path()).join(pins::MLX_QWEN_PINS[0].name))
        .unwrap();
    let observed = observe_truth(&shared, &config(root.path(), LocalHost::Darwin), None);
    assert_eq!(observed.phase, RuntimePhase::ArtifactNotReady);
    assert!(!observed.has_plan);
    assert!(
        shared
            .launch_request_for(&observed.desired_fingerprint)
            .is_none()
    );
}

#[test]
fn darwin_follow_acquires_mlx_and_retries_failed_acquisition_after_restart_backoff() {
    use solstone_core_local::install::status;
    let _helper = mlx_candidate::MlxHelperTestGuard::new();
    let root = tempfile::tempdir().unwrap();
    prior_llama_install(root.path());
    active_journal(root.path(), "local");
    let readiness = mlx_candidate::inspect_present_default(root.path());
    assert_eq!(readiness["reason_code"], "manifest_missing");
    let launches = Arc::new(AtomicUsize::new(0));
    let count = launches.clone();
    let launcher: crate::provider_runtime::local_follow::LocalInstallerLauncher =
        Arc::new(move |_: &Path| {
            count.fetch_add(1, Ordering::SeqCst);
            Err("fixture acquisition failed".into())
        });
    let observe = |follow: &LocalFollow| {
        let observed = observe_truth(
            &LocalRuntimeShared::default(),
            &config(root.path(), LocalHost::Darwin),
            Some(follow),
        );
        assert_eq!(observed.phase, RuntimePhase::ArtifactNotReady);
        assert!(!observed.has_plan);
    };
    let follow = LocalFollow::new(launcher.clone());
    observe(&follow);
    observe(&follow);
    assert_eq!(launches.load(Ordering::SeqCst), 1);
    let current = status::read_status(root.path(), "local").unwrap();
    let failed =
        status::transition(current, "failed", Some("download_failed".into()), None).unwrap();
    status::write_status(root.path(), failed.clone()).unwrap();
    observe(&LocalFollow::new(launcher.clone()));
    assert_eq!(
        launches.load(Ordering::SeqCst),
        1,
        "persisted backoff survives restart"
    );
    let mut expired = failed;
    let old = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
    expired.last_transition_at = Some(old.clone());
    expired.last_progress_at = Some(old.clone());
    expired.completed_at = Some(old);
    fs::write(
        status::status_path(root.path(), "local"),
        serde_json::to_vec(&expired).unwrap(),
    )
    .unwrap();
    observe(&LocalFollow::new(launcher.clone()));
    assert_eq!(
        launches.load(Ordering::SeqCst),
        2,
        "eligible restart retries the ordinary installer"
    );
    expired.error_code = Some("install_cancelled".into());
    fs::write(
        status::status_path(root.path(), "local"),
        serde_json::to_vec(&expired).unwrap(),
    )
    .unwrap();
    observe(&LocalFollow::new(launcher.clone()));
    assert_eq!(launches.load(Ordering::SeqCst), 2);
    active_journal(root.path(), "openai");
    let inactive = observe_truth(
        &LocalRuntimeShared::default(),
        &config(root.path(), LocalHost::Darwin),
        Some(&LocalFollow::new(launcher.clone())),
    );
    assert_eq!(inactive.phase, RuntimePhase::NotDesired);
    assert!(!inactive.has_plan);
    assert_eq!(launches.load(Ordering::SeqCst), 2);
    let never = tempfile::tempdir().unwrap();
    active_journal(never.path(), "local");
    observe_truth(
        &LocalRuntimeShared::default(),
        &config(never.path(), LocalHost::Darwin),
        Some(&LocalFollow::new(launcher)),
    );
    assert_eq!(launches.load(Ordering::SeqCst), 2);
}

fn obsolete_gguf(journal: &Path) -> PathBuf {
    use solstone_core_artifact_download::generation::{Generation, Member};
    let generation = Generation::new(
        "local-model",
        vec![Member {
            file: "old.gguf".into(),
            sha256: solstone_core_local::install::fingerprint::sha256("obsolete"),
            size_bytes: 8,
        }],
    )
    .unwrap();
    let directory = generation.directory(&pins::cache_root(journal).join("models"));
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("old.gguf"), b"obsolete").unwrap();
    generation.seal(&directory).unwrap();
    directory
}

#[test]
fn mlx_ready_follow_does_not_run_the_gguf_collector() {
    let witness = tempfile::tempdir().unwrap();
    let collectible = obsolete_gguf(witness.path());
    pins::collect_model_generations(witness.path());
    assert!(
        !collectible.exists(),
        "fixture must expose the old Ready cleanup"
    );
    let _helper = mlx_candidate::MlxHelperTestGuard::new();
    let root = tempfile::tempdir().unwrap();
    active_journal(root.path(), "local");
    present_mlx(root.path());
    let preserved = obsolete_gguf(root.path());
    let control = fs::read(preserved.join(".generation.json")).unwrap();
    let follow = LocalFollow::new(Arc::new(|_| panic!("ready MLX must not install")));
    for _ in 0..2 {
        let observed = observe_truth(
            &LocalRuntimeShared::default(),
            &config(root.path(), LocalHost::Darwin),
            Some(&follow),
        );
        assert_eq!(observed.phase, RuntimePhase::Starting);
        assert_eq!(fs::read(preserved.join("old.gguf")).unwrap(), b"obsolete");
        assert_eq!(
            fs::read(preserved.join(".generation.json")).unwrap(),
            control
        );
    }
}
