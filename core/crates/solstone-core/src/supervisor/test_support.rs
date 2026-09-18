// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared `SupervisorState` test fixtures.
//!
//! Split out because more than one handler test needs a real
//! `SupervisorState` — not a stand-in — without paying for a real spawn:
//! `stopped_providers` builds the `local`/`parakeet` fields at rest, and a
//! caller that also wants `queue.submit` to stay in-memory (no dispatch, no
//! child process) constructs its `TaskQueue` with `ready: false` and reads
//! `queue.contains_reference` back rather than driving the queue's worker
//! loop.

#![cfg(all(test, unix))]

use std::path::Path;
use std::sync::Arc;

use solstone_core_local::plan::Platform;
use solstone_core_system::provider_runtime::{
    FileRuntimeStore, LocalLifecycleSeam, LocalProbeSeam, LocalRuntimeShared, LocalTruthConfig,
    LocalTruthSeam, ParakeetLifecycleSeam, ParakeetProbeSeam, ParakeetRuntimeShared,
    ParakeetTruthConfig, ParakeetTruthSeam, ProviderName, ProviderRuntimeCoordinator,
    ProviderRuntimeState, RuntimeClock, SystemRuntimeClock,
};

use super::runtime::{LocalProvider, ParakeetProvider};

/// Local and Parakeet providers in their at-rest, nothing-running,
/// nothing-spawned state — enough to satisfy every `SupervisorState` field
/// that touches them without booting either runtime.
pub(super) fn stopped_providers(journal: &Path) -> (LocalProvider, ParakeetProvider) {
    let clock: Arc<dyn RuntimeClock> = Arc::new(SystemRuntimeClock::default());
    let local_shared = Arc::new(LocalRuntimeShared::default());
    let local = LocalProvider {
        coordinator: ProviderRuntimeCoordinator::new(),
        shared: local_shared.clone(),
        truth: LocalTruthSeam::with_config(
            local_shared.clone(),
            LocalTruthConfig {
                journal_path: journal.to_path_buf(),
                platform: if cfg!(target_os = "macos") {
                    Platform::Darwin
                } else {
                    Platform::Linux
                },
                nvidia_probe: None,
                vulkan_devices: Vec::new(),
            },
        ),
        lifecycle: LocalLifecycleSeam::new(local_shared.clone(), clock.clone())
            .with_journal(journal),
        probe: LocalProbeSeam::new(local_shared.clone(), journal),
        store: FileRuntimeStore::new(
            journal,
            ProviderName::Local,
            local_shared.clone(),
            clock.clone(),
        ),
        state: ProviderRuntimeState::new(ProviderName::Local),
        processes: Vec::new(),
        launch_recorded_for: None,
        fixture_launch: None,
    };
    let parakeet_shared = Arc::new(ParakeetRuntimeShared::default());
    let parakeet = ParakeetProvider {
        coordinator: ProviderRuntimeCoordinator::new(),
        shared: parakeet_shared.clone(),
        truth: ParakeetTruthSeam::with_config(
            parakeet_shared.clone(),
            ParakeetTruthConfig {
                journal_path: journal.to_path_buf(),
                remote_mode: false,
                platform: std::env::consts::OS.to_owned(),
                machine: std::env::consts::ARCH.to_owned(),
                vulkan_devices: Vec::new(),
            },
        ),
        lifecycle: ParakeetLifecycleSeam::new(parakeet_shared.clone(), clock.clone())
            .with_journal(journal),
        probe: ParakeetProbeSeam::new(parakeet_shared.clone(), journal),
        store: FileRuntimeStore::new(
            journal,
            ProviderName::Parakeet,
            parakeet_shared.clone(),
            clock,
        ),
        state: ProviderRuntimeState::new(ProviderName::Parakeet),
        processes: Vec::new(),
    };
    (local, parakeet)
}
