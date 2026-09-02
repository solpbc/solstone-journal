// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use solstone_core_system::lifecycle::{ShutdownDisposition, ShutdownDriver};
use solstone_core_system::process::SERVICE_SHUTDOWN_TIMEOUT;
use solstone_core_system::provider_runtime::{
    ProviderStopCleanupRequest, ReasonCode, RuntimePhase,
};

use super::runtime::SupervisorState;

/// The coordinator normally polls every 25 ms. This cap makes graceful
/// shutdown deterministic when healthy without retaining a supervisor that
/// needs to exit because its coordinator is wedged or unavailable.
const PARENT_LOSS_RETIRE_ACK_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) trait BoundedShutdownDiagnosticSink: Send + Sync {
    fn emit(&self, service: &str, message: &str) -> io::Result<()>;
}

pub(super) struct StderrBoundedShutdownDiagnosticSink;

impl BoundedShutdownDiagnosticSink for StderrBoundedShutdownDiagnosticSink {
    fn emit(&self, _: &str, message: &str) -> io::Result<()> {
        let stderr = io::stderr();
        let mut stderr = stderr.lock();
        writeln!(stderr, "{message}")
    }
}

pub(crate) struct SupervisorShutdownDriver {
    pub state: SupervisorState,
    pub runtime: tokio::runtime::Handle,
    parent_loss_bounded: bool,
    diagnostic_sink: Arc<dyn BoundedShutdownDiagnosticSink>,
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
        if let Some(coordinator) = self.state.parent_loss_coordinator.as_ref() {
            match coordinator.write_retire_expected(&self.state.journal) {
                Ok(()) => match coordinator.wait_for_retire_expected_ack(
                    &self.state.journal,
                    PARENT_LOSS_RETIRE_ACK_TIMEOUT,
                ) {
                    Ok(true) => {}
                    Ok(false) => {
                        // The coordinator remains the sole terminal authority.
                        // Continue shutdown rather than converting a bounded
                        // acknowledgement wait into an indefinite supervisor.
                        eprintln!(
                            "supervisor: parent-loss retirement acknowledgement timed out; continuing shutdown"
                        );
                    }
                    Err(error) => {
                        eprintln!(
                            "supervisor: could not read parent-loss retirement acknowledgement: {error}; continuing shutdown"
                        );
                    }
                },
                Err(error) => {
                    // A normal shutdown cannot authorize graceful retirement
                    // without the coordinator's private-generation material.
                    eprintln!(
                        "supervisor: could not request expected parent-loss retirement: {error}"
                    );
                    disposition = ShutdownDisposition::ForcedAfterGraceTimeout;
                }
            }
        }
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
        diagnostic_sink: Arc<dyn BoundedShutdownDiagnosticSink>,
    ) -> Self {
        Self {
            state,
            runtime,
            parent_loss_bounded,
            diagnostic_sink,
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
                    let service = app.service.as_str();
                    let message = format!(
                        "supervisor: failed to terminate {service} during bounded shutdown: {error}"
                    );
                    let _ = self.diagnostic_sink.emit(service, &message);
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

#[cfg(all(test, unix))]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::io;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::path::Path;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    use solstone_core_callosum::{CallosumSocketConnection, CallosumSocketServer};
    use solstone_core_local::plan::Platform;
    use solstone_core_system::cap::DefaultCapResolver;
    use solstone_core_system::lifecycle::{
        ShutdownDisposition, ShutdownPhase, ShutdownRegime, shutdown,
    };
    use solstone_core_system::process::{ManagedProcess, SpawnOptions, TerminationError};
    use solstone_core_system::provider_runtime::{
        FileRuntimeStore, LocalLifecycleSeam, LocalProbeSeam, LocalRuntimeShared, LocalTruthConfig,
        LocalTruthSeam, ParakeetLifecycleSeam, ParakeetProbeSeam, ParakeetRuntimeShared,
        ParakeetTruthConfig, ParakeetTruthSeam, ProviderName, ProviderRuntimeCoordinator,
        ProviderRuntimeState, RuntimeClock, SystemRuntimeClock, WedgeState,
    };
    use solstone_core_system::queue::{SystemProcessStateProbe, TaskQueue, TaskQueueOptions};
    use tempfile::TempDir;

    use super::super::runtime::{
        AppService, DailyState, FlushState, LocalProvider, ManagedAppProcess, ParakeetProvider,
        SupervisorState, SupervisorTiming,
    };
    use super::{BoundedShutdownDiagnosticSink, SupervisorShutdownDriver};

    #[derive(Default)]
    struct RecordingSink {
        events: Mutex<Vec<(String, String)>>,
        fail: bool,
    }

    impl RecordingSink {
        fn failed() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
                fail: true,
            }
        }

        fn events(&self) -> Vec<(String, String)> {
            self.events.lock().expect("recording sink lock").clone()
        }
    }

    impl BoundedShutdownDiagnosticSink for RecordingSink {
        fn emit(&self, service: &str, message: &str) -> io::Result<()> {
            self.events
                .lock()
                .expect("recording sink lock")
                .push((service.to_owned(), message.to_owned()));
            if self.fail {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "injected broken stderr pipe",
                ));
            }
            Ok(())
        }
    }

    struct Fixture {
        driver: SupervisorShutdownDriver,
        _journal: TempDir,
        child_pids: Vec<u32>,
    }

    impl Fixture {
        fn cleanup(&mut self) {
            for pid in &self.child_pids {
                let _ = kill(
                    Pid::from_raw(i32::try_from(*pid).expect("child pid fits i32")),
                    Signal::SIGKILL,
                );
            }
            for app in &mut self.driver.state.app_processes {
                if let Some(process) = app.process.as_mut() {
                    let _ = process.wait();
                    process.cleanup();
                }
            }
        }
    }

    async fn fixture(
        services: &[AppService],
        diagnostic_sink: Arc<dyn BoundedShutdownDiagnosticSink>,
    ) -> Fixture {
        let journal = TempDir::new().expect("temporary journal");
        let socket_path = journal.path().join("callosum.sock");
        let server = Arc::new(
            CallosumSocketServer::bind(&socket_path)
                .await
                .expect("callosum server"),
        );
        let connection = CallosumSocketConnection::new(&socket_path, serde_json::Map::new());
        let queue = TaskQueue::new(TaskQueueOptions {
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(DefaultCapResolver::new(Duration::from_secs(1))),
            process_state_probe: Arc::new(SystemProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
        });
        let (local, parakeet) = stopped_providers(journal.path());
        let mut app_processes = Vec::new();
        let mut child_pids = Vec::new();
        for service in services {
            let mut app =
                ManagedAppProcess::new(*service, true, journal.path(), None, None, 0, false);
            let process = ManagedProcess::spawn(
                vec!["/bin/sleep".to_owned(), "60".to_owned()],
                SpawnOptions {
                    journal_root: journal.path().to_path_buf(),
                    reference: format!("shutdown-test-{}", service.as_str()),
                    day: None,
                    sink: None,
                    environment: BTreeMap::new(),
                },
            )
            .expect("legacy managed app process");
            child_pids.push(process.pid());
            app.process = Some(process);
            app_processes.push(app);
        }
        let state = SupervisorState {
            journal: journal.path().to_path_buf(),
            is_remote_mode: false,
            no_daily: true,
            server,
            connection,
            queue,
            last_sync_snapshot: None,
            stale_heartbeats: Vec::new(),
            shutdown_started: AtomicBool::new(false),
            started: Instant::now(),
            scheduler: None,
            recorded_schedule_completions: BTreeSet::new(),
            app_processes,
            local,
            parakeet,
            flush: FlushState::default(),
            daily: DailyState { last_day: None },
            last_retry_expiry_drain: Instant::now(),
            wedge: WedgeState::default(),
            timing: SupervisorTiming {
                tick_interval: Duration::from_secs(1),
                status_interval: Duration::from_secs(5),
            },
            parent_loss_coordinator: None,
            sense_child_environment: BTreeMap::new(),
        };
        Fixture {
            driver: SupervisorShutdownDriver::new(
                state,
                tokio::runtime::Handle::current(),
                true,
                diagnostic_sink,
            ),
            _journal: journal,
            child_pids,
        }
    }

    fn stopped_providers(journal: &Path) -> (LocalProvider, ParakeetProvider) {
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
            lifecycle: LocalLifecycleSeam::new(local_shared.clone(), clock.clone()),
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
            lifecycle: ParakeetLifecycleSeam::new(parakeet_shared.clone(), clock.clone()),
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

    fn run_shutdown(mut fixture: Fixture) -> solstone_core_system::lifecycle::ShutdownReport {
        let result = catch_unwind(AssertUnwindSafe(|| {
            shutdown(&mut fixture.driver, ShutdownRegime::ParentLossBounded)
        }));
        fixture.cleanup();
        result.expect("bounded shutdown must not panic")
    }

    fn expected_message(service: AppService) -> String {
        format!(
            "supervisor: failed to terminate {} during bounded shutdown: {}",
            service.as_str(),
            TerminationError::ExactInstanceUnavailable
        )
    }

    fn assert_forced_stop_children(report: &solstone_core_system::lifecycle::ShutdownReport) {
        assert_eq!(
            report.phases,
            vec![
                ShutdownPhase::ReapManagedStarted,
                ShutdownPhase::ReapManagedCompleted,
                ShutdownPhase::DrainTasksStarted,
                ShutdownPhase::DrainTasksCompleted,
                ShutdownPhase::StopChildrenStarted,
                ShutdownPhase::StopChildrenCompleted,
                ShutdownPhase::JoinBusStarted,
                ShutdownPhase::JoinBusCompleted,
            ]
        );
        assert_eq!(
            report.disposition,
            ShutdownDisposition::ForcedAfterGraceTimeout
        );
        assert_eq!(
            report.forced_phase,
            Some(ShutdownPhase::StopChildrenCompleted)
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn bounded_shutdown_ignores_failed_diagnostic_write_and_preserves_report() {
        let sink = Arc::new(RecordingSink::failed());
        let diagnostic_sink: Arc<dyn BoundedShutdownDiagnosticSink> = sink.clone();
        let report = run_shutdown(fixture(&[AppService::Spl], diagnostic_sink).await);

        assert_eq!(
            sink.events(),
            vec![("spl".to_owned(), expected_message(AppService::Spl))]
        );
        assert_forced_stop_children(&report);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn bounded_shutdown_emits_each_failure_when_diagnostics_succeed_or_fail() {
        let services = [AppService::Cortex, AppService::Spl];
        let expected_events = vec![
            ("spl".to_owned(), expected_message(AppService::Spl)),
            ("cortex".to_owned(), expected_message(AppService::Cortex)),
        ];

        let usable_sink = Arc::new(RecordingSink::default());
        let usable_diagnostic_sink: Arc<dyn BoundedShutdownDiagnosticSink> = usable_sink.clone();
        let usable_report = run_shutdown(fixture(&services, usable_diagnostic_sink).await);
        assert_eq!(usable_sink.events(), expected_events);
        assert_forced_stop_children(&usable_report);

        let failed_sink = Arc::new(RecordingSink::failed());
        let failed_diagnostic_sink: Arc<dyn BoundedShutdownDiagnosticSink> = failed_sink.clone();
        let failed_report = run_shutdown(fixture(&services, failed_diagnostic_sink).await);
        assert_eq!(failed_sink.events(), expected_events);
        assert_eq!(failed_report.disposition, usable_report.disposition);
        assert_eq!(failed_report.forced_phase, usable_report.forced_phase);
        assert_eq!(failed_report.phases, usable_report.phases);
    }
}
