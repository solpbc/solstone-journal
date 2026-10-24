// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The local provider's single-process macOS migration policy. The ordinary
//! coordinator still owns every launch, admission and stop.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};
use solstone_core_local::install::macos_mlx_migration::{self, MigrationState};

use crate::queue::{LocalAnalysisPause, TaskQueue};

use super::launch::LocalLaunchConfig;
use super::local_follow::{LAUNCH_BACKOFF_STEPS, LocalFollow, PERSISTED_BACKOFF};
use super::model::{ProviderRuntimeState, RuntimePhase};
use super::store::{LocalRuntimeShared, RuntimeClock};

const IDLE_WAIT_SECONDS: f64 = 120.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MacosRuntimeChoice {
    Metal,
    Mlx,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Attempt {
    #[default]
    Predecessor,
    Mlx,
    Returning,
}

#[derive(Default)]
struct Memory {
    attempt: Attempt,
    predecessor_attempt_finished: bool,
    pause: Option<LocalAnalysisPause>,
    drain_started: Option<f64>,
    next_attempt: f64,
    failures: usize,
    next_cleanup: f64,
    cleanup_failures: usize,
}

pub(super) struct MacosMlxHandover {
    journal: PathBuf,
    clock: Arc<dyn RuntimeClock>,
    queue: Option<TaskQueue>,
    memory: Mutex<Memory>,
}

impl MacosMlxHandover {
    pub(super) fn new(
        journal: PathBuf,
        clock: Arc<dyn RuntimeClock>,
        queue: Option<TaskQueue>,
    ) -> Self {
        Self {
            journal,
            clock,
            queue,
            memory: Mutex::new(Memory::default()),
        }
    }

    fn release(memory: &mut Memory) {
        memory.pause = None;
        memory.drain_started = None;
    }

    fn failed(&self, memory: &mut Memory) {
        let wait = LAUNCH_BACKOFF_STEPS[memory.failures.min(LAUNCH_BACKOFF_STEPS.len() - 1)];
        memory.failures = memory.failures.saturating_add(1);
        memory.next_attempt = self.clock.monotonic_seconds() + wait.as_secs_f64();
        memory.attempt = Attempt::Returning;
    }

    /// Runs on the supervisor thread after ordinary reconciliation.
    /// Only a coordinator-admitted, fingerprint-matching Ready child can commit.
    pub(super) fn advance(&self, state: &ProviderRuntimeState, shared: &LocalRuntimeShared) {
        let mut memory = self.memory.lock().expect("macOS migration state");
        let config = solstone_core_journal_config::read_journal_config(&self.journal)
            .ok()
            .and_then(|read| read.config);
        let active = config.as_ref().is_some_and(bundled_active);
        if !active || state.shutdown_requested {
            Self::release(&mut memory);
            memory.attempt = Attempt::Predecessor;
            memory.predecessor_attempt_finished = false;
            return;
        }
        let record = match macos_mlx_migration::read(&self.journal) {
            Ok(record) => record,
            Err(error) => {
                log::warn!("macOS runtime migration authority is unavailable: {error}");
                Self::release(&mut memory);
                return;
            }
        };
        let launch = shared.launch_request_for(&state.desired_fingerprint);
        // Once a pre-commit return is chosen, finish it through the ordinary
        // coordinator. A later tick of the old Ready state cannot recommit.
        if record != MigrationState::Committed
            && memory.attempt == Attempt::Returning
            && matches!(launch, Some(LocalLaunchConfig::Mlx { .. }))
        {
            return;
        }
        let admitted = state.latest_phase == RuntimePhase::Ready
            && shared.admitted_generation_matches(state.generation, &state.desired_fingerprint);
        if admitted && matches!(launch, Some(LocalLaunchConfig::Mlx { .. })) {
            if let Err(error) = macos_mlx_migration::commit(&self.journal) {
                log::warn!("macOS runtime migration could not commit: {error}");
                // Atomic publication can report an error after publishing. The
                // record, rather than that error alone, decides whether return
                // to the installed predecessor is still authorized.
                if matches!(
                    macos_mlx_migration::read(&self.journal),
                    Ok(MigrationState::Absent | MigrationState::InProgress)
                ) && memory.attempt == Attempt::Mlx
                {
                    self.failed(&mut memory);
                }
                return;
            }
            Self::release(&mut memory);
            let now = self.clock.monotonic_seconds();
            if now >= memory.next_cleanup {
                match macos_mlx_migration::retire_predecessor(&self.journal) {
                    Ok(_) => {
                        memory.cleanup_failures = 0;
                        memory.next_cleanup = now + PERSISTED_BACKOFF.as_secs_f64();
                    }
                    Err(error) => {
                        log::warn!("macOS predecessor cache retirement will retry: {error}");
                        let schedule = super::model::PROVIDER_CLEANUP_RETRY_SCHEDULE_SECONDS;
                        memory.next_cleanup =
                            now + schedule[memory.cleanup_failures.min(schedule.len() - 1)];
                        memory.cleanup_failures = memory.cleanup_failures.saturating_add(1);
                    }
                }
            }
        } else if admitted && matches!(launch, Some(LocalLaunchConfig::Metal { .. })) {
            memory.predecessor_attempt_finished = true;
            if memory.attempt == Attempt::Returning {
                memory.attempt = Attempt::Predecessor;
                Self::release(&mut memory);
            }
        } else if record != MigrationState::Committed
            && matches!(launch, Some(LocalLaunchConfig::Metal { .. }))
            && state.start.is_none()
            && state.retry.attempt_count as usize
                >= super::model::PROVIDER_RETRY_SCHEDULE_SECONDS.len()
        {
            // Installed files alone do not guarantee the old backend can
            // start. Its ordinary finite failure budget must not pin the Mac
            // to an unusable predecessor when MLX is available.
            memory.predecessor_attempt_finished = true;
            if memory.attempt == Attempt::Returning {
                memory.attempt = Attempt::Predecessor;
                Self::release(&mut memory);
            }
        } else if record != MigrationState::Committed
            && memory.attempt == Attempt::Mlx
            && matches!(launch, Some(LocalLaunchConfig::Mlx { .. }))
            && state.start.is_none()
            && (matches!(
                state.latest_phase,
                RuntimePhase::Failed
                    | RuntimePhase::ArtifactNotReady
                    | RuntimePhase::HostBlocked
                    | RuntimePhase::StateCorrupt
                    | RuntimePhase::StateUnavailable
            ) || state.retry.attempt_count as usize
                >= super::model::PROVIDER_RETRY_SCHEDULE_SECONDS.len()
                || (!admitted
                    && shared
                        .warmed_generation_matches(state.generation, &state.desired_fingerprint)))
        {
            self.failed(&mut memory);
        }
    }

    pub(super) fn choose(
        &self,
        config: &Map<String, Value>,
        predecessor: &Value,
        mlx: &Value,
        follow: Option<&LocalFollow>,
    ) -> Result<MacosRuntimeChoice, String> {
        let record = macos_mlx_migration::read(&self.journal).map_err(|error| error.to_string())?;
        if record == MigrationState::Committed {
            if mlx["ready"] != true
                && let Some(follow) = follow
            {
                follow.observe_migration(&self.journal, config, self.clock.as_ref());
            }
            return Ok(MacosRuntimeChoice::Mlx);
        }
        let active = bundled_active(config);
        if predecessor["ready"] != true {
            if mlx["ready"] != true
                && active
                && let Some(follow) = follow
            {
                follow.observe_migration(&self.journal, config, self.clock.as_ref());
            }
            return Ok(MacosRuntimeChoice::Mlx);
        }
        let mut memory = self.memory.lock().expect("macOS migration state");
        if !active {
            Self::release(&mut memory);
            memory.attempt = Attempt::Predecessor;
            memory.predecessor_attempt_finished = false;
            return Ok(MacosRuntimeChoice::Metal);
        }
        if memory.attempt == Attempt::Mlx {
            if mlx["ready"] != true {
                self.failed(&mut memory);
                return Ok(MacosRuntimeChoice::Metal);
            }
            return Ok(MacosRuntimeChoice::Mlx);
        }
        if memory.attempt == Attempt::Returning {
            return Ok(MacosRuntimeChoice::Metal);
        }
        if macos_mlx_migration::start(&self.journal).map_err(|error| error.to_string())?
            == MigrationState::Committed
        {
            return Ok(MacosRuntimeChoice::Mlx);
        }
        if mlx["ready"] != true {
            Self::release(&mut memory);
            if let Some(follow) = follow {
                follow.observe_migration(&self.journal, config, self.clock.as_ref());
            }
            return Ok(MacosRuntimeChoice::Metal);
        }
        // On restart, restore the predecessor first. Its ordinary admission
        // or exhausted start budget precedes a new idle swap attempt.
        if !memory.predecessor_attempt_finished {
            return Ok(MacosRuntimeChoice::Metal);
        }
        if solstone_core_local::install::lease::is_held(&self.journal, "local")
            .map_err(|error| error.to_string())?
        {
            Self::release(&mut memory);
            return Ok(MacosRuntimeChoice::Metal);
        }
        let now = self.clock.monotonic_seconds();
        if now < memory.next_attempt {
            return Ok(MacosRuntimeChoice::Metal);
        }
        let Some(queue) = self.queue.as_ref() else {
            return Ok(MacosRuntimeChoice::Metal);
        };
        if memory.pause.is_none() {
            memory.pause = Some(queue.pause_local_analysis());
            memory.drain_started = Some(now);
        }
        if memory.pause.as_ref().expect("pause installed").is_idle() {
            memory.attempt = Attempt::Mlx;
            memory.predecessor_attempt_finished = false;
            return Ok(MacosRuntimeChoice::Mlx);
        }
        if now - memory.drain_started.expect("pause has a start") >= IDLE_WAIT_SECONDS {
            Self::release(&mut memory);
            memory.next_attempt = now + PERSISTED_BACKOFF.as_secs_f64();
            log::info!("macOS runtime migration deferred while queued thinking is busy");
        }
        Ok(MacosRuntimeChoice::Metal)
    }
}

pub(super) fn bundled_active(config: &Map<String, Value>) -> bool {
    config
        .get("providers")
        .and_then(|value| value.get("active"))
        .and_then(|value| value.get("provider"))
        .and_then(Value::as_str)
        == Some("local")
        && !matches!(
            solstone_core_local::endpoint::resolve_local_endpoint(config),
            solstone_core_local::endpoint::LocalEndpointResolution::Byo(_)
        )
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::super::launch::LocalLaunchCommon;
    use super::super::model::{ProviderFence, ProviderName};
    use super::super::store::ReadyProcessLookup;
    use super::*;
    use crate::queue::{ProcessState, ProcessStateProbe, TaskQueueOptions};
    use serde_json::json;
    use std::{collections::BTreeMap, path::Path, time::Duration};

    #[derive(Default)]
    struct Clock(Mutex<f64>);
    impl RuntimeClock for Clock {
        fn monotonic_seconds(&self) -> f64 {
            *self.0.lock().unwrap()
        }
        fn now_utc_rfc3339(&self) -> String {
            (chrono::DateTime::parse_from_rfc3339("2026-10-23T00:00:00Z").unwrap()
                + chrono::Duration::seconds(self.monotonic_seconds() as i64))
            .to_rfc3339()
        }
        fn sleep(&self, _: Duration) {
            panic!("policy must not sleep")
        }
    }
    impl Clock {
        fn at(&self, time: f64) {
            *self.0.lock().unwrap() = time;
        }
    }
    struct NoHostProbe;
    impl ProcessStateProbe for NoHostProbe {
        fn state(&self, _: u32) -> ProcessState {
            panic!("no host process observation")
        }
    }
    fn options(journal: &Path) -> TaskQueueOptions {
        TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.into(),
            cap_resolver: Arc::new(crate::cap::DefaultCapResolver::default()),
            process_state_probe: Arc::new(NoHostProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        }
    }
    fn fixture() -> (tempfile::TempDir, Map<String, Value>, Arc<Clock>) {
        let journal = tempfile::tempdir().unwrap();
        std::fs::create_dir(journal.path().join("config")).unwrap();
        let config = json!({"providers":{"active":{"provider":"local"}}, "processing":{"gate":{"time_window":{"enabled":false},"display_powersave":{"enabled":false}}}});
        std::fs::write(
            journal.path().join("config/journal.json"),
            config.to_string(),
        )
        .unwrap();
        (
            journal,
            config.as_object().unwrap().clone(),
            Arc::new(Clock::default()),
        )
    }
    fn launch(mlx: bool) -> LocalLaunchConfig {
        let common = LocalLaunchCommon {
            desired_fingerprint_json: json!({}),
            desired_fingerprint_sha256: "candidate".into(),
            model_id: "local/qwen3.5-4b".into(),
            model_path: "fixture".into(),
            mmproj_path: None,
        };
        if mlx {
            LocalLaunchConfig::Mlx {
                common,
                binary_path: None,
                fixture_pins: None,
                unified_memory_mib: None,
            }
        } else {
            LocalLaunchConfig::Metal {
                common,
                binary_path: None,
                unified_memory_mib: None,
            }
        }
    }
    fn ready(shared: &LocalRuntimeShared, state: &mut ProviderRuntimeState, mlx: bool) {
        shared.record_launch_request(state.desired_fingerprint.clone(), launch(mlx));
        let fence = ProviderFence {
            incarnation: "fixture".into(),
            generation: state.generation,
            fingerprint: state.desired_fingerprint.clone(),
            attempt: state.retry.attempt_count,
        };
        shared.accept_ready(&fence);
        state.latest_phase = RuntimePhase::Ready;
    }

    fn admit_predecessor(manager: &MacosMlxHandover, shared: &LocalRuntimeShared) {
        let mut state = ProviderRuntimeState::new(ProviderName::Local);
        state.desired_fingerprint = Some("predecessor".into());
        ready(shared, &mut state, false);
        manager.advance(&state, shared);
    }

    #[test]
    fn download_failure_keeps_predecessor_and_automatically_retries_beyond_launch_budget() {
        let (journal, config, clock) = fixture();
        let model =
            solstone_core_local::install::pins::cache_root(journal.path()).join("models/installed");
        std::fs::create_dir_all(&model).unwrap();
        std::fs::write(
            solstone_core_local::install::manifest::artifact_manifest_path(&model),
            b"{}",
        )
        .unwrap();
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls = count.clone();
        let follow = LocalFollow::new(Arc::new(move |_| {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err("download failed".into())
        }));
        let manager = MacosMlxHandover::new(journal.path().into(), clock.clone(), None);
        let yes = json!({"ready":true});
        let no = json!({"ready":false});
        // The installer and runtime retries use the existing escalating backoff,
        // including after more failures than a single runtime launch budget.
        let mut time = 0.0;
        for attempt in 0..9 {
            clock.at(time);
            assert_eq!(
                manager.choose(&config, &yes, &no, Some(&follow)).unwrap(),
                MacosRuntimeChoice::Metal
            );
            assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), attempt + 1);
            assert_eq!(
                manager.choose(&config, &yes, &no, Some(&follow)).unwrap(),
                MacosRuntimeChoice::Metal
            );
            assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), attempt + 1);
            time += LAUNCH_BACKOFF_STEPS[attempt.min(3)].as_secs_f64();
        }
        assert_eq!(
            macos_mlx_migration::read(journal.path()).unwrap(),
            MigrationState::InProgress
        );
        assert!(manager.memory.lock().unwrap().pause.is_none());
        assert!(model.exists());
    }

    #[test]
    fn busy_queue_defers_without_interrupting_then_swaps_after_a_later_idle_boundary() {
        use crate::queue::ExecutionRequest;
        use crate::request::{BusTaskRequest, TaskArgv};
        use std::sync::{Barrier, mpsc};
        let (journal, config, clock) = fixture();
        let release = Arc::new(Barrier::new(2));
        let worker_release = release.clone();
        let (started, received) = mpsc::channel();
        let (finished, completed) = mpsc::channel();
        let queue = TaskQueue::with_callable_worker(
            options(journal.path()),
            Arc::new(move |_, context| {
                started.send(()).unwrap();
                worker_release.wait();
                assert!(
                    !context.is_cancelled(),
                    "migration must never interrupt work"
                );
                finished.send(()).unwrap();
                0
            }),
        );
        queue.submit(ExecutionRequest::Bus(BusTaskRequest {
            cmd: TaskArgv::from_wire(vec!["journal".into(), "describe".into(), "segment".into()])
                .unwrap(),
            reference: "in-flight-describe".into(),
            day: None,
            scheduler_name: None,
            queue_if_active_cmd_differs: false,
            priority: None,
            daily_catchup_provenance: None,
        }));
        received.recv_timeout(Duration::from_secs(5)).unwrap();
        let manager =
            MacosMlxHandover::new(journal.path().into(), clock.clone(), Some(queue.clone()));
        admit_predecessor(&manager, &LocalRuntimeShared::default());
        let yes = json!({"ready":true});
        assert_eq!(
            manager.choose(&config, &yes, &yes, None).unwrap(),
            MacosRuntimeChoice::Metal
        );
        clock.at(IDLE_WAIT_SECONDS);
        assert_eq!(
            manager.choose(&config, &yes, &yes, None).unwrap(),
            MacosRuntimeChoice::Metal
        );
        assert!(manager.memory.lock().unwrap().pause.is_none());
        release.wait();
        completed.recv_timeout(Duration::from_secs(5)).unwrap();
        queue.join_callable_workers().unwrap();
        clock.at(IDLE_WAIT_SECONDS + PERSISTED_BACKOFF.as_secs_f64() - 1.0);
        assert_eq!(
            manager.choose(&config, &yes, &yes, None).unwrap(),
            MacosRuntimeChoice::Metal
        );
        clock.at(IDLE_WAIT_SECONDS + PERSISTED_BACKOFF.as_secs_f64());
        assert_eq!(
            manager.choose(&config, &yes, &yes, None).unwrap(),
            MacosRuntimeChoice::Mlx
        );
        assert_eq!(
            macos_mlx_migration::read(journal.path()).unwrap(),
            MigrationState::InProgress
        );
        assert!(manager.memory.lock().unwrap().pause.is_some());
    }

    #[test]
    fn failed_first_mlx_launch_returns_to_verified_predecessor_and_later_rearms() {
        let (journal, config, clock) = fixture();
        let queue = TaskQueue::new(options(journal.path()));
        let manager = MacosMlxHandover::new(journal.path().into(), clock.clone(), Some(queue));
        let shared = LocalRuntimeShared::default();
        admit_predecessor(&manager, &shared);
        let yes = json!({"ready":true});
        let mut state = ProviderRuntimeState::new(ProviderName::Local);
        state.desired_fingerprint = Some("candidate".into());
        let mut time = 0.0;
        for attempt in 0..8 {
            clock.at(time);
            assert_eq!(
                manager.choose(&config, &yes, &yes, None).unwrap(),
                MacosRuntimeChoice::Mlx
            );
            shared.record_launch_request(state.desired_fingerprint.clone(), launch(true));
            // Truth dispatch may already have replaced Failed by Observing.
            state.latest_phase = RuntimePhase::Observing;
            state.retry.attempt_count =
                super::super::model::PROVIDER_RETRY_SCHEDULE_SECONDS.len() as u32;
            manager.advance(&state, &shared);
            assert_eq!(
                manager.choose(&config, &yes, &yes, None).unwrap(),
                MacosRuntimeChoice::Metal
            );
            state.generation += 1;
            ready(&shared, &mut state, false);
            manager.advance(&state, &shared);
            assert!(manager.memory.lock().unwrap().pause.is_none());
            assert_eq!(
                manager.choose(&config, &yes, &yes, None).unwrap(),
                MacosRuntimeChoice::Metal
            );
            assert_eq!(
                macos_mlx_migration::read(journal.path()).unwrap(),
                MigrationState::InProgress
            );
            time += LAUNCH_BACKOFF_STEPS[attempt.min(3)].as_secs_f64();
        }
    }

    #[test]
    fn only_matching_admitted_ready_commits_and_restart_never_selects_predecessor_afterward() {
        let (journal, config, clock) = fixture();
        let queue = TaskQueue::new(options(journal.path()));
        let manager = MacosMlxHandover::new(journal.path().into(), clock.clone(), Some(queue));
        let yes = json!({"ready":true});
        assert_eq!(
            manager.choose(&config, &yes, &yes, None).unwrap(),
            MacosRuntimeChoice::Metal
        );
        let shared = LocalRuntimeShared::default();
        admit_predecessor(&manager, &shared);
        assert_eq!(
            manager.choose(&config, &yes, &yes, None).unwrap(),
            MacosRuntimeChoice::Mlx
        );
        let mut state = ProviderRuntimeState::new(ProviderName::Local);
        state.generation = 1;
        state.desired_fingerprint = Some("candidate".into());
        shared.record_launch_request(state.desired_fingerprint.clone(), launch(true));
        state.latest_phase = RuntimePhase::Ready;
        manager.advance(&state, &shared);
        assert_eq!(
            macos_mlx_migration::read(journal.path()).unwrap(),
            MigrationState::InProgress
        );
        ready(&shared, &mut state, true);
        state.generation += 1;
        manager.advance(&state, &shared);
        assert_eq!(
            macos_mlx_migration::read(journal.path()).unwrap(),
            MigrationState::InProgress
        );
        state.generation -= 1;
        manager.advance(&state, &shared);
        assert_eq!(
            macos_mlx_migration::read(journal.path()).unwrap(),
            MigrationState::Committed
        );
        assert!(manager.memory.lock().unwrap().pause.is_none());
        let restarted = MacosMlxHandover::new(journal.path().into(), clock, None);
        assert_eq!(
            restarted
                .choose(&config, &yes, &json!({"ready":false}), None)
                .unwrap(),
            MacosRuntimeChoice::Mlx
        );
        std::fs::write(
            journal.path().join("config/macos-mlx-migration.json"),
            b"broken",
        )
        .unwrap();
        assert!(restarted.choose(&config, &yes, &yes, None).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn ready_publication_or_commit_failure_returns_before_the_one_way_decision() {
        use std::os::unix::fs::PermissionsExt;
        for publication_failure in [true, false] {
            let (journal, config, clock) = fixture();
            let queue = TaskQueue::new(options(journal.path()));
            let manager = MacosMlxHandover::new(journal.path().into(), clock.clone(), Some(queue));
            let shared = LocalRuntimeShared::default();
            admit_predecessor(&manager, &shared);
            let yes = json!({"ready":true});
            assert_eq!(
                manager.choose(&config, &yes, &yes, None).unwrap(),
                MacosRuntimeChoice::Mlx
            );
            let mut state = ProviderRuntimeState::new(ProviderName::Local);
            state.generation = 1;
            state.desired_fingerprint = Some("candidate".into());
            ready(&shared, &mut state, true);
            let directory = journal.path().join("config");
            if publication_failure {
                shared.invalidate_launch_credentials();
                state.latest_phase = RuntimePhase::StateUnavailable;
            } else {
                std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o500))
                    .unwrap();
            }
            manager.advance(&state, &shared);
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
            manager.advance(&state, &shared);
            assert_eq!(
                macos_mlx_migration::read(journal.path()).unwrap(),
                MigrationState::InProgress
            );
            assert_eq!(
                manager.choose(&config, &yes, &yes, None).unwrap(),
                MacosRuntimeChoice::Metal
            );
            assert!(manager.memory.lock().unwrap().pause.is_some());
            admit_predecessor(&manager, &shared);
            assert!(manager.memory.lock().unwrap().pause.is_none());
            clock.at(PERSISTED_BACKOFF.as_secs_f64());
            assert_eq!(
                manager.choose(&config, &yes, &yes, None).unwrap(),
                MacosRuntimeChoice::Mlx
            );
        }
    }

    #[test]
    fn unusable_predecessor_cannot_permanently_prevent_an_idle_mlx_attempt() {
        let (journal, config, clock) = fixture();
        let queue = TaskQueue::new(options(journal.path()));
        let manager = MacosMlxHandover::new(journal.path().into(), clock, Some(queue));
        let shared = LocalRuntimeShared::default();
        let yes = json!({"ready":true});
        assert_eq!(
            manager.choose(&config, &yes, &yes, None).unwrap(),
            MacosRuntimeChoice::Metal
        );
        let mut state = ProviderRuntimeState::new(ProviderName::Local);
        state.desired_fingerprint = Some("predecessor".into());
        shared.record_launch_request(state.desired_fingerprint.clone(), launch(false));
        state.latest_phase = RuntimePhase::Observing;
        state.retry.attempt_count =
            super::super::model::PROVIDER_RETRY_SCHEDULE_SECONDS.len() as u32;
        manager.advance(&state, &shared);
        assert_eq!(
            manager.choose(&config, &yes, &yes, None).unwrap(),
            MacosRuntimeChoice::Mlx
        );
        assert_eq!(
            macos_mlx_migration::read(journal.path()).unwrap(),
            MigrationState::InProgress
        );
    }

    #[test]
    fn ordinary_coordinator_composes_download_drain_stop_ready_commit_and_precommit_return() {
        use super::super::model::{
            LaunchOutcomeStatus, ManagedProcess, ProbeStatus, ProviderLaunchOutcome,
            ProviderProbeOutcome, ProviderRuntimeNow, ProviderStopCleanupOutcome,
            ProviderTruthObservation, ReasonCode, StopCleanupStatus,
        };
        use super::super::seams::{RetryToken, RuntimeStore, RuntimeStoreError};
        use super::super::{
            InMemoryRuntimeStore, NoopWorkers, ProviderRuntimeCoordinator, ReconcileContext,
            VecEventSink,
        };
        struct Store<'a> {
            inner: InMemoryRuntimeStore,
            shared: &'a LocalRuntimeShared,
            reject_new_ready: bool,
        }
        impl RuntimeStore for Store<'_> {
            fn admit_ready(&mut self, fence: &ProviderFence) {
                self.shared.accept_ready(fence);
            }
            fn revoke_ready(&mut self) {
                self.shared.invalidate_launch_credentials();
            }
            fn read_retry_token(
                &mut self,
                provider: ProviderName,
            ) -> Result<Option<RetryToken>, RuntimeStoreError> {
                self.inner.read_retry_token(provider)
            }
            fn consume_retry_token(
                &mut self,
                provider: ProviderName,
                token: &str,
            ) -> Result<(), RuntimeStoreError> {
                self.inner.consume_retry_token(provider, token)
            }
            fn publish_state(
                &mut self,
                state: &ProviderRuntimeState,
            ) -> Result<(), RuntimeStoreError> {
                if state.latest_phase != RuntimePhase::Ready {
                    self.shared.invalidate_launch_credentials();
                }
                if state.latest_phase == RuntimePhase::Ready
                    && state.desired_fingerprint.as_deref() == Some("new")
                    && self.reject_new_ready
                {
                    self.reject_new_ready = false;
                    self.shared.invalidate_launch_credentials();
                    return Err(RuntimeStoreError::Unavailable);
                }
                self.inner.publish_state(state)
            }
        }
        for failure in ["none", "launch-budget", "ready-publication"] {
            let (journal, config, clock) = fixture();
            let queue = TaskQueue::new(options(journal.path()));
            let manager = MacosMlxHandover::new(journal.path().into(), clock.clone(), Some(queue));
            let shared = LocalRuntimeShared::default();
            let mut store = Store {
                inner: InMemoryRuntimeStore::default(),
                shared: &shared,
                reject_new_ready: failure == "ready-publication",
            };
            let coordinator = ProviderRuntimeCoordinator::new();
            let mut state = ProviderRuntimeState::new(ProviderName::Local);
            let mut processes: Vec<ManagedProcess> = Vec::new();
            let mut truth = NoopWorkers;
            let mut lifecycle = NoopWorkers;
            let mut probe = NoopWorkers;
            let mut sink = VecEventSink::default();
            let mut events = Vec::new();
            let mut old_stopped = false;
            let mut returned = false;
            let mut fail_starts = if failure == "launch-budget" {
                super::super::model::PROVIDER_RETRY_SCHEDULE_SECONDS.len()
            } else {
                0
            };
            let model = solstone_core_local::install::pins::model_directory(
                journal.path(),
                "local/qwen3.5-4b",
            );
            std::fs::create_dir_all(&model).unwrap();
            std::fs::write(model.join("Qwen3.5-4B-Q4_K_M.gguf"), b"old fixture model").unwrap();
            for tick in 0..500 {
                let time = clock.monotonic_seconds();
                if let Some(pending) = state.truth.as_mut()
                    && pending.result.is_none()
                {
                    let candidate = json!({"ready":tick >= 20});
                    let choice = manager
                        .choose(&config, &json!({"ready":true}), &candidate, None)
                        .unwrap();
                    let mlx = choice == MacosRuntimeChoice::Mlx;
                    let fingerprint = if mlx { "new" } else { "old" };
                    shared.record_launch_request(Some(fingerprint.into()), launch(mlx));
                    pending.result = Some(ProviderTruthObservation {
                        provider: ProviderName::Local,
                        phase: RuntimePhase::Starting,
                        reason_code: Some(ReasonCode::known("launch-requested")),
                        desired_fingerprint: Some(fingerprint.into()),
                        has_plan: true,
                        boot_required: true,
                        detail: None,
                    });
                }
                if let Some(pending) = state.stop_cleanup.as_mut()
                    && pending.result.is_none()
                {
                    let stopped = state
                        .pending_stop_request
                        .as_ref()
                        .expect("owned stop request")
                        .managed
                        .clone();
                    assert_eq!(
                        processes.iter().filter(|process| process.running).count(),
                        1
                    );
                    events.push(format!("stop-{}", stopped.id));
                    if stopped.id == "old" {
                        old_stopped = true;
                    }
                    pending.result = Some(ProviderStopCleanupOutcome {
                        status: StopCleanupStatus::Stopped,
                        reason_code: ReasonCode::known("cleanup-succeeded"),
                        managed: Some(stopped),
                    });
                }
                if let Some(pending) = state.start.as_mut()
                    && pending.result.is_none()
                {
                    assert!(
                        !processes.iter().any(|process| process.running),
                        "a replacement never starts beside its predecessor"
                    );
                    let mlx = pending.fence.fingerprint.as_deref() == Some("new");
                    if mlx {
                        assert!(old_stopped, "predecessor exit precedes every MLX start");
                    }
                    let id = if mlx { "new" } else { "old" };
                    events.push(format!("start-{id}"));
                    let failed = mlx && fail_starts > 0;
                    if failed {
                        fail_starts -= 1;
                    }
                    let managed = (!failed).then(|| ManagedProcess {
                        id: id.into(),
                        name: "local".into(),
                        pid: 12345,
                        running: true,
                        fence: Some(pending.fence.clone()),
                    });
                    pending.result = Some(ProviderLaunchOutcome {
                        status: if failed {
                            LaunchOutcomeStatus::LaunchFailed
                        } else {
                            LaunchOutcomeStatus::Ready
                        },
                        reason_code: ReasonCode::known(if failed {
                            "launch-failed"
                        } else {
                            "probe-ready"
                        }),
                        managed,
                    });
                }
                if let Some(pending) = state.probe.as_mut()
                    && pending.result.is_none()
                {
                    pending.result = Some(ProviderProbeOutcome {
                        status: ProbeStatus::Ready,
                        reason_code: ReasonCode::known("probe-ready"),
                    });
                }
                coordinator.reconcile(
                    ProviderRuntimeNow {
                        monotonic_seconds: time,
                    },
                    &mut state,
                    &mut processes,
                    &mut ReconcileContext {
                        truth: &mut truth,
                        lifecycle: &mut lifecycle,
                        probe: &mut probe,
                        store: &mut store,
                        sink: &mut sink,
                        gate: None,
                    },
                );
                manager.advance(&state, &shared);
                assert!(processes.iter().filter(|process| process.running).count() <= 1);
                if tick == 19 {
                    assert_eq!(state.latest_phase, RuntimePhase::Ready);
                    assert_eq!(state.desired_fingerprint.as_deref(), Some("old"));
                    assert!(shared.launch_credentials().is_none());
                    assert!(
                        shared.admitted_generation_matches(
                            state.generation,
                            &state.desired_fingerprint
                        )
                    );
                    assert!(model.exists());
                    assert!(!events.iter().any(|event| event == "start-new"));
                }
                if failure != "none"
                    && old_stopped
                    && state.latest_phase == RuntimePhase::Ready
                    && state.desired_fingerprint.as_deref() == Some("old")
                    && !returned
                {
                    assert_eq!(
                        macos_mlx_migration::read(journal.path()).unwrap(),
                        MigrationState::InProgress
                    );
                    assert!(
                        model.exists(),
                        "precommit failure preserves the predecessor files"
                    );
                    assert!(manager.memory.lock().unwrap().pause.is_none());
                    returned = true;
                    clock.at(manager.memory.lock().unwrap().next_attempt);
                } else {
                    clock.at(time + 1.0);
                }
                if macos_mlx_migration::read(journal.path()).unwrap() == MigrationState::Committed {
                    assert_eq!(state.latest_phase, RuntimePhase::Ready);
                    assert_eq!(state.desired_fingerprint.as_deref(), Some("new"));
                    assert!(shared.launch_credentials().is_none());
                    assert!(
                        shared.admitted_generation_matches(
                            state.generation,
                            &state.desired_fingerprint
                        )
                    );
                    assert!(!model.exists());
                    break;
                }
            }
            assert_eq!(
                macos_mlx_migration::read(journal.path()).unwrap(),
                MigrationState::Committed,
                "{failure}: {events:?}"
            );
            assert_eq!(returned, failure != "none");
            assert_eq!(events.first().map(String::as_str), Some("start-old"));
        }
    }
}
