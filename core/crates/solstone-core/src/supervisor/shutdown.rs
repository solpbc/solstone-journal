// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use solstone_core_system::lifecycle::ShutdownDriver;
use solstone_core_system::process::SERVICE_SHUTDOWN_TIMEOUT;
use solstone_core_system::provider_runtime::{
    ProviderStopCleanupRequest, ReasonCode, RuntimePhase,
};

use super::runtime::SupervisorState;

pub(crate) struct SupervisorShutdownDriver<'a> {
    pub state: SupervisorState,
    pub runtime: &'a tokio::runtime::Runtime,
}
impl ShutdownDriver for SupervisorShutdownDriver<'_> {
    fn reap_managed(&mut self, _: Duration) {
        self.state.reap_managed();
    }
    fn drain_tasks(&mut self, _: Duration) {
        if !self.state.shutdown_started.swap(true, Ordering::AcqRel) {
            let _ = self.state.queue.shutdown();
        }
    }
    fn stop_children(&mut self, cap: Option<Duration>) {
        for app in &mut self.state.app_processes {
            app.enabled = false;
            app.restart_at = None;
        }
        for app in self.state.app_processes.iter_mut().rev() {
            let Some(process) = app.process.as_mut() else {
                continue;
            };
            if let Err(error) = process.terminate(SERVICE_SHUTDOWN_TIMEOUT) {
                eprintln!(
                    "supervisor: failed to terminate {} during shutdown: {error}",
                    app.service.as_str()
                );
            }
            process.cleanup();
        }
        request_stop(&mut self.state.local.state, &self.state.local.processes);
        request_stop(
            &mut self.state.parakeet.state,
            &self.state.parakeet.processes,
        );
        let deadline = cap.map(|value| Instant::now() + value);
        while provider_running(&self.state) && deadline.is_none_or(|limit| Instant::now() < limit) {
            super::tick::reconcile_providers(&mut self.state);
            std::thread::sleep(Duration::from_millis(10));
        }
        self.state.reap_managed();
    }
    fn join_bus(&mut self, _: Duration) {
        self.runtime.block_on(async {
            self.state.connection.stop().await;
            self.state.server.stop().await;
        });
    }
}

fn request_stop(
    state: &mut solstone_core_system::provider_runtime::ProviderRuntimeState,
    processes: &[solstone_core_system::provider_runtime::ManagedProcess],
) {
    if state.pending_stop_request.is_some()
        || matches!(
            state.latest_phase,
            RuntimePhase::Stopped | RuntimePhase::NotDesired
        )
    {
        return;
    }
    if let Some(managed) = processes.iter().find(|process| process.running).cloned() {
        state.pending_stop_request = Some(ProviderStopCleanupRequest {
            managed,
            reason_code: ReasonCode::known("intent-removed"),
            target_phase: RuntimePhase::Stopped,
            target_reason_code: Some(ReasonCode::known("cleanup-succeeded")),
            admission_exclusive: false,
            orphaned_start_outcome: false,
        });
    }
}

fn provider_running(state: &SupervisorState) -> bool {
    state.local.processes.iter().any(|process| process.running)
        || state
            .parakeet
            .processes
            .iter()
            .any(|process| process.running)
}
