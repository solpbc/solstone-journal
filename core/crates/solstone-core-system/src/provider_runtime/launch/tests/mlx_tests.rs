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
fn selected_mlx_failure_does_not_reach_installer_and_other_hosts_ignore_selector() {
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
        |_, _, _| Ok(MacosRuntimeChoice::Mlx),
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
            |_, _, _| panic!("non-Darwin host consulted the migration policy"),
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
