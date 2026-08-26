// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use solstone_core_system::lifecycle::{ShutdownDisposition, ShutdownDriver};
use solstone_core_system::process::SERVICE_SHUTDOWN_TIMEOUT;
use solstone_core_system::provider_runtime::{
    ProviderStopCleanupRequest, ReasonCode, RuntimePhase,
};

use super::runtime::SupervisorState;

pub(crate) struct SupervisorShutdownDriver {
    pub state: SupervisorState,
    pub runtime: tokio::runtime::Handle,
    parent_loss_bounded: bool,
}
impl ShutdownDriver for SupervisorShutdownDriver {
    fn reap_managed(&mut self, cap: Duration) -> ShutdownDisposition {
        if self.parent_loss_bounded {
            let deadline = Instant::now() + cap;
            return if self.state.reap_managed_until(deadline) {
                ShutdownDisposition::Orderly
            } else {
                ShutdownDisposition::ForcedAfterGraceTimeout
            };
        }
        self.state.reap_managed();
        ShutdownDisposition::Orderly
    }
    fn drain_tasks(&mut self, cap: Duration) -> ShutdownDisposition {
        if self.parent_loss_bounded {
            if !self.state.shutdown_started.swap(true, Ordering::AcqRel)
                && self.state.queue.shutdown_until(Instant::now() + cap).forced
            {
                return ShutdownDisposition::ForcedAfterGraceTimeout;
            }
            return ShutdownDisposition::Orderly;
        }
        if !self.state.shutdown_started.swap(true, Ordering::AcqRel)
            && self.state.queue.shutdown().forced
        {
            return ShutdownDisposition::ForcedAfterGraceTimeout;
        }
        ShutdownDisposition::Orderly
    }
    fn stop_children(&mut self, cap: Option<Duration>) -> ShutdownDisposition {
        if self.parent_loss_bounded {
            return self.stop_children_until(cap);
        }
        let mut disposition = ShutdownDisposition::Orderly;
        for app in &mut self.state.app_processes {
            app.enabled = false;
            app.restart_at = None;
        }
        for app in self.state.app_processes.iter_mut().rev() {
            let Some(process) = app.process.as_mut() else {
                continue;
            };
            if let Err(error) = process.terminate_exact(SERVICE_SHUTDOWN_TIMEOUT) {
                if matches!(
                    error,
                    solstone_core_system::process::TerminationError::ParentGraceTimeout
                ) {
                    disposition = ShutdownDisposition::ForcedAfterGraceTimeout;
                }
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
        if provider_running(&self.state) {
            disposition = ShutdownDisposition::ForcedAfterGraceTimeout;
        }
        self.state.reap_managed();
        disposition
    }
    fn join_bus(&mut self, cap: Duration) -> ShutdownDisposition {
        if self.parent_loss_bounded {
            let result = tokio::task::block_in_place(|| {
                self.runtime.block_on(async {
                    tokio::time::timeout(cap, async {
                        self.state.connection.stop().await;
                        self.state.server.stop().await;
                    })
                    .await
                })
            });
            return if result.is_ok() {
                ShutdownDisposition::Orderly
            } else {
                ShutdownDisposition::ForcedAfterGraceTimeout
            };
        }
        tokio::task::block_in_place(|| {
            self.runtime.block_on(async {
                self.state.connection.stop().await;
                self.state.server.stop().await;
            });
        });
        ShutdownDisposition::Orderly
    }
}

impl SupervisorShutdownDriver {
    pub(super) fn new(
        state: SupervisorState,
        runtime: tokio::runtime::Handle,
        parent_loss_bounded: bool,
    ) -> Self {
        Self {
            state,
            runtime,
            parent_loss_bounded,
        }
    }

    fn stop_children_until(&mut self, cap: Option<Duration>) -> ShutdownDisposition {
        let Some(cap) = cap else {
            return ShutdownDisposition::ForcedAfterGraceTimeout;
        };
        let deadline = Instant::now() + cap;
        let mut disposition = ShutdownDisposition::Orderly;
        for app in &mut self.state.app_processes {
            app.enabled = false;
            app.restart_at = None;
        }
        for app in self.state.app_processes.iter_mut().rev() {
            let Some(process) = app.process.as_mut() else {
                continue;
            };
            let result = process.terminate_exact_until(deadline);
            if result.is_err() || !process.cleanup_until(deadline) {
                disposition = ShutdownDisposition::ForcedAfterGraceTimeout;
                process.detach_after_bounded_shutdown();
                if let Err(error) = result {
                    eprintln!(
                        "supervisor: failed to terminate {} during bounded shutdown: {error}",
                        app.service.as_str()
                    );
                }
            }
        }
        request_stop(&mut self.state.local.state, &self.state.local.processes);
        request_stop(
            &mut self.state.parakeet.state,
            &self.state.parakeet.processes,
        );
        while provider_running(&self.state) && Instant::now() < deadline {
            super::tick::reconcile_providers(&mut self.state);
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            std::thread::sleep(remaining.min(Duration::from_millis(10)));
        }
        if provider_running(&self.state) {
            disposition = ShutdownDisposition::ForcedAfterGraceTimeout;
        }
        if !self.state.reap_managed_until(deadline) {
            disposition = ShutdownDisposition::ForcedAfterGraceTimeout;
        }
        disposition
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
