// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Queue-idle maintenance policy. The ordinary coordinator owns replacement.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use solstone_core_local::generate::{GenerateTransport, UreqTransport};

use crate::queue::{LocalAnalysisPause, TaskQueue};

use super::model::{ManagedProcess, ProviderRuntimeState, RuntimePhase};
use super::store::{LocalRuntimeShared, ReadyProcess, ReadyProcessLookup, RuntimeClock};

const REQUEST_LIMIT: u64 = 2000;
const IDLE_SECONDS: f64 = 60.0;
const DRAIN_SECONDS: f64 = 120.0;
const SAMPLE_SECONDS: f64 = 5.0;

#[derive(Clone, Copy, Debug, Deserialize)]
pub(super) struct MaintenanceSnapshot {
    pid: u32,
    footprint_bytes: Option<u64>,
    activity_generation: u64,
    idle: bool,
}

pub(super) fn read_snapshot(process: &ReadyProcess) -> Result<MaintenanceSnapshot, String> {
    let response = UreqTransport.get(
        &format!("http://127.0.0.1:{}", process.port),
        "/maintenance",
        Duration::from_secs(1),
        None,
    )?;
    if response.status != 200 {
        return Err(format!("maintenance HTTP status {}", response.status));
    }
    let snapshot: MaintenanceSnapshot =
        serde_json::from_str(&response.body).map_err(|error| error.to_string())?;
    if snapshot.pid != process.pid {
        return Err("maintenance observation does not match the owned helper".into());
    }
    Ok(snapshot)
}

pub(super) fn capture_ready_baseline(process: &ReadyProcess) -> Option<MaintenanceSnapshot> {
    match read_snapshot(process) {
        Ok(snapshot) => {
            log::info!(
                "MLX Ready request baseline: pid={} requests={} footprint_bytes={:?}",
                process.pid,
                snapshot.activity_generation,
                snapshot.footprint_bytes
            );
            Some(snapshot)
        }
        Err(error) => {
            log::warn!(
                "MLX maintenance unavailable for this admission: pid={} baseline observation failed: {error}",
                process.pid
            );
            None
        }
    }
}

#[derive(Default)]
struct IdleTrigger {
    quiet: Option<(u64, f64)>,
}

impl IdleTrigger {
    fn observe(
        &mut self,
        now: f64,
        baseline: u64,
        request_limit: u64,
        snapshot: MaintenanceSnapshot,
    ) -> bool {
        if !snapshot.idle || snapshot.activity_generation <= baseline {
            self.quiet = None;
            return false;
        }
        let (_, quiet_since) = *self
            .quiet
            .get_or_insert((snapshot.activity_generation, now));
        if self
            .quiet
            .is_some_and(|(generation, _)| generation != snapshot.activity_generation)
        {
            self.quiet = Some((snapshot.activity_generation, now));
            return false;
        }
        now - quiet_since >= IDLE_SECONDS
            && snapshot.activity_generation.saturating_sub(baseline) >= request_limit
    }

    fn unchanged(&self, snapshot: MaintenanceSnapshot) -> bool {
        snapshot.idle
            && self
                .quiet
                .is_some_and(|(generation, _)| generation == snapshot.activity_generation)
    }
}

#[derive(Default)]
struct Memory {
    tracked: Option<ManagedProcess>,
    trigger: IdleTrigger,
    next_sample: f64,
    pause: Option<LocalAnalysisPause>,
    drain_started: Option<f64>,
    replacing: bool,
    stop_requested_at: Option<Instant>,
}

pub(super) struct MlxRecycle {
    queue: TaskQueue,
    clock: Arc<dyn RuntimeClock>,
    memory: Mutex<Memory>,
    request_limit: u64,
}

impl MlxRecycle {
    pub(super) fn new(queue: TaskQueue, clock: Arc<dyn RuntimeClock>) -> Self {
        let request_limit = REQUEST_LIMIT;
        // Qualification can lower only the threshold, never the idle/drain guards.
        // This environment seam is absent from ordinary production builds.
        #[cfg(feature = "test-hooks")]
        let request_limit = match std::env::var("SOLSTONE_TEST_MLX_RECYCLE_REQUESTS") {
            Ok(value) => value
                .parse::<u64>()
                .ok()
                .filter(|value| *value > 0 && *value <= REQUEST_LIMIT)
                .expect(
                    "positive test MLX recycle request count no larger than the production constant",
                ),
            Err(std::env::VarError::NotPresent) => request_limit,
            Err(error) => panic!("invalid test MLX recycle threshold: {error}"),
        };
        Self {
            queue,
            clock,
            memory: Mutex::new(Memory::default()),
            request_limit,
        }
    }

    fn release(memory: &mut Memory) {
        memory.pause = None;
        memory.drain_started = None;
        memory.trigger.quiet = None;
    }

    pub(super) fn cancel_request(&self) {
        *self.memory.lock().expect("MLX maintenance state") = Memory::default();
    }

    /// Called on the supervisor thread after reconciliation. Returning a process
    /// requests ordinary coordinator replacement; it never stops the child here.
    pub(super) fn advance(
        &self,
        state: &ProviderRuntimeState,
        processes: &[ManagedProcess],
        shared: &LocalRuntimeShared,
        active: bool,
    ) -> Option<ManagedProcess> {
        self.advance_with_sampler(state, processes, shared, active, read_snapshot)
    }

    fn advance_with_sampler(
        &self,
        state: &ProviderRuntimeState,
        processes: &[ManagedProcess],
        shared: &LocalRuntimeShared,
        active: bool,
        mut sample: impl FnMut(&ReadyProcess) -> Result<MaintenanceSnapshot, String>,
    ) -> Option<ManagedProcess> {
        let mut memory = self.memory.lock().expect("MLX maintenance state");
        if !active || state.shutdown_requested {
            *memory = Memory::default();
            return None;
        }
        if memory.replacing {
            let previous = memory.tracked.as_ref().expect("replacement has an owner");
            if previous.fence.as_ref().map(|fence| &fence.fingerprint)
                != Some(&state.desired_fingerprint)
            {
                *memory = Memory::default();
            } else if state.latest_phase == RuntimePhase::Ready
                && shared.admitted_generation_matches(state.generation, &state.desired_fingerprint)
                && processes.iter().any(|process| {
                    process.running
                        && process.fence != previous.fence
                        && process.fence.as_ref().is_some_and(|fence| {
                            fence.generation == state.generation
                                && fence.fingerprint == state.desired_fingerprint
                                && shared.ready_process_for_fence(fence).is_some_and(|ready| {
                                    ready.pid == process.pid && ready.process_id == process.id
                                })
                        })
                })
            {
                log::info!(
                    "MLX maintenance successor admitted Ready: old_pid={} downtime_seconds={:.3}",
                    previous.pid,
                    memory
                        .stop_requested_at
                        .expect("replacement start")
                        .elapsed()
                        .as_secs_f64()
                );
                *memory = Memory::default();
            }
            // Failed stops/starts retain the queue pause through ordinary retries.
            return None;
        }
        if state.latest_phase != RuntimePhase::Ready
            || !shared.admitted_generation_matches(state.generation, &state.desired_fingerprint)
        {
            Self::release(&mut memory);
            return None;
        }
        let Some(process) = processes.iter().find(|process| {
            process.running
                && process.fence.as_ref().is_some_and(|fence| {
                    fence.generation == state.generation
                        && fence.fingerprint == state.desired_fingerprint
                })
        }) else {
            Self::release(&mut memory);
            return None;
        };
        let Some((_ready_at, baseline)) =
            shared.mlx_ready_baseline(state.generation, &state.desired_fingerprint)
        else {
            Self::release(&mut memory);
            return None;
        };
        if memory.tracked.as_ref() != Some(process) {
            *memory = Memory {
                tracked: Some(process.clone()),
                ..Memory::default()
            };
        }
        let now = self.clock.monotonic_seconds();
        if memory.pause.is_some() {
            if now - memory.drain_started.expect("pause has a start") >= DRAIN_SECONDS {
                log::info!("MLX maintenance deferred while queued thinking is busy");
                Self::release(&mut memory);
                memory.next_sample = now + SAMPLE_SECONDS;
                return None;
            }
            if !memory.pause.as_ref().expect("pause installed").is_idle() {
                return None;
            }
        } else if now < memory.next_sample {
            return None;
        }
        memory.next_sample = now + SAMPLE_SECONDS;
        let Some(ready) =
            shared.ready_process_for_fence(process.fence.as_ref().expect("tracked fence"))
        else {
            Self::release(&mut memory);
            return None;
        };
        let snapshot = match sample(&ready) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                log::warn!("MLX maintenance observation deferred: {error}");
                Self::release(&mut memory);
                memory.next_sample = now + IDLE_SECONDS;
                return None;
            }
        };
        if !memory.trigger.observe(
            now,
            baseline.activity_generation,
            self.request_limit,
            snapshot,
        ) {
            if memory.pause.is_some() {
                Self::release(&mut memory);
            }
            return None;
        }
        if memory.pause.is_none() {
            memory.pause = Some(self.queue.pause_local_analysis());
            memory.drain_started = Some(now);
        }
        if !memory.pause.as_ref().expect("pause installed").is_idle() {
            return None;
        }
        // Recheck after atomically pausing queue admission and draining reservations.
        // Direct operator calls remain the explicitly accepted reset residual.
        let rechecked = match sample(&ready) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                log::warn!("MLX maintenance post-drain observation deferred: {error}");
                Self::release(&mut memory);
                return None;
            }
        };
        if !memory.trigger.unchanged(rechecked) {
            Self::release(&mut memory);
            return None;
        }
        memory.replacing = true;
        memory.stop_requested_at = Some(Instant::now());
        log::info!(
            "MLX maintenance requested: pid={} baseline_requests={} requests={} threshold_requests={} footprint_bytes={:?}",
            process.pid,
            baseline.activity_generation,
            rechecked.activity_generation,
            self.request_limit,
            rechecked.footprint_bytes
        );
        Some(process.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(requests: u64, idle: bool) -> MaintenanceSnapshot {
        MaintenanceSnapshot {
            pid: 42,
            footprint_bytes: None,
            activity_generation: requests,
            idle,
        }
    }

    #[test]
    fn production_threshold_is_exactly_2000_calls_since_ready() {
        let mut trigger = IdleTrigger::default();
        let baseline = 17;
        assert!(!trigger.observe(
            0.0,
            baseline,
            REQUEST_LIMIT,
            snapshot(baseline + 1999, true)
        ));
        assert!(!trigger.observe(
            60.0,
            baseline,
            REQUEST_LIMIT,
            snapshot(baseline + 1999, true)
        ));
        assert!(!trigger.observe(
            61.0,
            baseline,
            REQUEST_LIMIT,
            snapshot(baseline + 2000, true)
        ));
        assert!(trigger.observe(
            121.0,
            baseline,
            REQUEST_LIMIT,
            snapshot(baseline + 2000, true)
        ));
    }

    #[test]
    fn calls_require_sixty_seconds_of_unchanged_idle_without_a_ready_age_guard() {
        let mut trigger = IdleTrigger::default();
        assert!(!trigger.observe(0.0, 0, 2, snapshot(2, true)));
        assert!(!trigger.observe(59.0, 0, 2, snapshot(2, true)));
        assert!(trigger.observe(60.0, 0, 2, snapshot(2, true)));
        assert!(!trigger.observe(61.0, 0, 2, snapshot(3, true)));
        assert!(!trigger.unchanged(snapshot(2, true)));
        assert!(!trigger.unchanged(snapshot(3, false)));
        assert!(trigger.observe(121.0, 0, 2, snapshot(3, true)));
        assert!(!trigger.observe(122.0, 0, 2, snapshot(3, false)));
        assert!(!trigger.observe(123.0, 0, 2, snapshot(3, true)));
    }

    #[test]
    fn calls_at_or_below_the_admission_baseline_never_recycle() {
        let mut trigger = IdleTrigger::default();
        for now in [0.0, 60.0, 2000.0] {
            assert!(!trigger.observe(now, 2000, REQUEST_LIMIT, snapshot(0, true)));
            assert!(!trigger.observe(now, 2000, REQUEST_LIMIT, snapshot(2000, true)));
            assert!(!trigger.observe(now, 2000, REQUEST_LIMIT, snapshot(2001, true)));
        }
    }

    #[test]
    fn footprint_is_evidence_only_even_when_unavailable_or_zero() {
        let mut trigger = IdleTrigger::default();
        assert!(!trigger.observe(0.0, 0, REQUEST_LIMIT, snapshot(2000, true)));
        let mut observed = snapshot(2000, true);
        observed.footprint_bytes = Some(0);
        assert!(trigger.observe(60.0, 0, REQUEST_LIMIT, observed));
        observed.footprint_bytes = Some(u64::MAX);
        assert!(trigger.observe(60.0, 0, REQUEST_LIMIT, observed));
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod composed_tests {
    use super::super::model::{ProviderFence, ProviderName};
    use super::*;
    use crate::queue::{ExecutionRequest, ProcessState, ProcessStateProbe, TaskQueueOptions};
    use crate::request::{BusTaskRequest, TaskArgv};
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::mpsc;

    #[derive(Default)]
    struct Clock(Mutex<f64>);
    impl RuntimeClock for Clock {
        fn monotonic_seconds(&self) -> f64 {
            *self.0.lock().unwrap()
        }
        fn now_utc_rfc3339(&self) -> String {
            "2026-10-24T00:00:00Z".into()
        }
        fn sleep(&self, _: Duration) {
            panic!("maintenance must not sleep")
        }
    }
    impl Clock {
        fn at(&self, seconds: f64) {
            *self.0.lock().unwrap() = seconds;
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
    fn request() -> ExecutionRequest {
        ExecutionRequest::Bus(BusTaskRequest {
            cmd: TaskArgv::from_wire(vec!["journal".into(), "describe".into(), "segment".into()])
                .unwrap(),
            reference: "maintenance-in-flight-describe".into(),
            day: None,
            scheduler_name: None,
            queue_if_active_cmd_differs: false,
            priority: None,
            daily_catchup_provenance: None,
        })
    }
    fn admit(
        shared: &LocalRuntimeShared,
        state: &mut ProviderRuntimeState,
        pid: u32,
    ) -> ManagedProcess {
        let fence = ProviderFence {
            incarnation: "test".into(),
            generation: state.generation,
            fingerprint: Some("same-candidate".into()),
            attempt: 1,
        };
        state.latest_phase = RuntimePhase::Ready;
        state.desired_fingerprint = fence.fingerprint.clone();
        let process = ManagedProcess {
            id: format!("local:{pid}"),
            pid,
            name: "local".into(),
            running: true,
            fence: Some(fence.clone()),
        };
        shared.record_ready_process(
            &fence,
            ReadyProcess {
                process_id: process.id.clone(),
                process_name: process.name.clone(),
                pid,
                port: 1,
            },
        );
        shared.record_mlx_admission_for_test(
            &fence,
            Instant::now(),
            MaintenanceSnapshot {
                pid,
                footprint_bytes: None,
                activity_generation: 0,
                idle: true,
            },
        );
        process
    }
    fn observe(generation: u64) -> MaintenanceSnapshot {
        MaintenanceSnapshot {
            pid: 42,
            footprint_bytes: None,
            activity_generation: generation * REQUEST_LIMIT,
            idle: true,
        }
    }

    #[test]
    fn busy_queue_defers_and_successor_ready_alone_releases_pause() {
        let journal = tempfile::tempdir().unwrap();
        std::fs::create_dir(journal.path().join("config")).unwrap();
        std::fs::write(journal.path().join("config/journal.json"),
            r#"{"processing":{"gate":{"time_window":{"enabled":false},"display_powersave":{"enabled":false}}}}"#).unwrap();
        let (release, receive_release) = mpsc::channel();
        let receive_release = Mutex::new(receive_release);
        let (started, receive_started) = mpsc::channel();
        let (finished, receive_finished) = mpsc::channel();
        let queue = TaskQueue::with_callable_worker(
            options(journal.path()),
            Arc::new(move |_, context| {
                started.send(()).unwrap();
                receive_release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
                assert!(
                    !context.is_cancelled(),
                    "maintenance never interrupts queued analysis"
                );
                finished.send(()).unwrap();
                0
            }),
        );
        queue.submit(request());
        receive_started
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        let clock = Arc::new(Clock::default());
        let manager = MlxRecycle::new(queue.clone(), clock.clone());
        let shared = LocalRuntimeShared::default();
        let mut state = ProviderRuntimeState::new(ProviderName::Local);
        let child = admit(&shared, &mut state, 42);
        let children = [child.clone()];
        assert!(
            manager
                .advance_with_sampler(&state, &children, &shared, true, |_| Ok(observe(1)))
                .is_none()
        );
        clock.at(60.0);
        assert!(
            manager
                .advance_with_sampler(&state, &children, &shared, true, |_| Ok(observe(1)))
                .is_none()
        );
        assert!(manager.memory.lock().unwrap().pause.is_some());
        clock.at(180.0);
        assert!(
            manager
                .advance_with_sampler(&state, &children, &shared, true, |_| panic!(
                    "drain bound defers before sampling"
                ))
                .is_none()
        );
        assert!(manager.memory.lock().unwrap().pause.is_none());
        release.send(()).unwrap();
        // The explicit retirement join cancels workers; observe ordinary task
        // completion before invoking that cleanup helper.
        receive_finished
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        queue.join_callable_workers().unwrap();
        clock.at(185.0);
        assert!(
            manager
                .advance_with_sampler(&state, &children, &shared, true, |_| Ok(observe(1)))
                .is_none()
        );
        clock.at(245.0);
        assert_eq!(
            manager.advance_with_sampler(&state, &children, &shared, true, |_| Ok(observe(1))),
            Some(child)
        );
        state.generation += 1;
        for phase in [
            RuntimePhase::Stopping,
            RuntimePhase::CleanupFailed,
            RuntimePhase::Backoff,
            RuntimePhase::Failed,
        ] {
            state.latest_phase = phase;
            assert!(
                manager
                    .advance_with_sampler(&state, &children, &shared, true, |_| panic!(
                        "no sampling during replacement"
                    ))
                    .is_none()
            );
            assert!(manager.memory.lock().unwrap().pause.is_some());
        }
        state.latest_phase = RuntimePhase::Ready;
        assert!(
            manager
                .advance_with_sampler(&state, &children, &shared, true, |_| panic!(
                    "unadmitted Ready cannot resume"
                ))
                .is_none()
        );
        assert!(manager.memory.lock().unwrap().pause.is_some());
        let successor = admit(&shared, &mut state, 43);
        assert!(
            manager
                .advance_with_sampler(&state, &[successor], &shared, true, |_| panic!(
                    "Ready releases before another sample"
                ))
                .is_none()
        );
        assert!(manager.memory.lock().unwrap().pause.is_none());
        queue.shutdown();
    }

    #[test]
    fn activity_before_ready_is_not_activity_after_admission() {
        let journal = tempfile::tempdir().unwrap();
        let queue = TaskQueue::new(options(journal.path()));
        let clock = Arc::new(Clock::default());
        let manager = MlxRecycle::new(queue.clone(), clock.clone());
        let shared = LocalRuntimeShared::default();
        let mut state = ProviderRuntimeState::new(ProviderName::Local);
        let child = admit(&shared, &mut state, 42);
        shared.record_mlx_admission_for_test(
            child.fence.as_ref().unwrap(),
            Instant::now(),
            MaintenanceSnapshot {
                pid: 42,
                footprint_bytes: None,
                activity_generation: 7 * REQUEST_LIMIT,
                idle: true,
            },
        );
        let children = [child];
        for seconds in [0.0, 60.0, 120.0] {
            clock.at(seconds);
            assert!(
                manager
                    .advance_with_sampler(&state, &children, &shared, true, |_| Ok(observe(7)))
                    .is_none()
            );
        }
        clock.at(125.0);
        assert!(
            manager
                .advance_with_sampler(&state, &children, &shared, true, |_| Ok(observe(8)))
                .is_none()
        );
        clock.at(185.0);
        assert!(
            manager
                .advance_with_sampler(&state, &children, &shared, true, |_| Ok(observe(8)))
                .is_some()
        );
        manager.cancel_request();
        queue.shutdown();
    }

    #[test]
    fn activity_after_drain_cancels_and_interruption_releases_maintenance() {
        let journal = tempfile::tempdir().unwrap();
        let queue = TaskQueue::new(options(journal.path()));
        let clock = Arc::new(Clock::default());
        let manager = MlxRecycle::new(queue.clone(), clock.clone());
        let shared = LocalRuntimeShared::default();
        let mut state = ProviderRuntimeState::new(ProviderName::Local);
        let child = admit(&shared, &mut state, 42);
        let children = [child];
        assert!(
            manager
                .advance_with_sampler(&state, &children, &shared, true, |_| Ok(observe(1)))
                .is_none()
        );
        clock.at(60.0);
        let mut reads = 0;
        assert!(
            manager
                .advance_with_sampler(&state, &children, &shared, true, |_| {
                    reads += 1;
                    Ok(observe(if reads == 1 { 1 } else { 2 }))
                })
                .is_none()
        );
        assert_eq!(reads, 2);
        assert!(manager.memory.lock().unwrap().pause.is_none());
        clock.at(65.0);
        assert!(
            manager
                .advance_with_sampler(&state, &children, &shared, true, |_| Ok(observe(2)))
                .is_none()
        );
        clock.at(125.0);
        assert!(
            manager
                .advance_with_sampler(&state, &children, &shared, true, |_| Ok(observe(2)))
                .is_some()
        );
        assert!(manager.memory.lock().unwrap().pause.is_some());
        assert!(
            manager
                .advance_with_sampler(&state, &children, &shared, false, |_| panic!(
                    "inactive cancels before sampling"
                ))
                .is_none()
        );
        assert!(manager.memory.lock().unwrap().pause.is_none());
        clock.at(130.0);
        assert!(
            manager
                .advance_with_sampler(&state, &children, &shared, true, |_| Ok(observe(3)))
                .is_none()
        );
        clock.at(190.0);
        assert!(
            manager
                .advance_with_sampler(&state, &children, &shared, true, |_| Ok(observe(3)))
                .is_some()
        );
        state.shutdown_requested = true;
        assert!(
            manager
                .advance_with_sampler(&state, &children, &shared, true, |_| panic!(
                    "shutdown cancels before sampling"
                ))
                .is_none()
        );
        assert!(manager.memory.lock().unwrap().pause.is_none());
        queue.shutdown();
    }
}
