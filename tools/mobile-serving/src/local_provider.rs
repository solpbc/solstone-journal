// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! App-owned use of the existing Local coordinator and credential fences.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use serde_json::{Map, json};
use solstone_core_callosum::CallosumSocketConnection;
use solstone_core_journal_io::JournalRoot;
use solstone_core_system::provider_runtime::{
    FileRuntimeStore, LocalLifecycleSeam, LocalProbeSeam, LocalReadySideEffect, LocalRuntimeShared,
    LocalTruthSeam, ManagedProcess, ProviderName, ProviderRuntimeCoordinator, ProviderRuntimeNow,
    ProviderRuntimeState, ProviderStopCleanupRequest, ReasonCode, ReconcileContext, RuntimeClock,
    RuntimePhase, SystemRuntimeClock, VecEventSink,
};
use tokio::sync::watch;

struct Owner {
    shared: Arc<LocalRuntimeShared>,
    root: Arc<JournalRoot>,
}

type Registry = BTreeMap<PathBuf, Weak<Owner>>;
static OWNERS: OnceLock<Mutex<Registry>> = OnceLock::new();

fn registry() -> &'static Mutex<Registry> {
    OWNERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn current(journal: &Path) -> Option<Arc<Owner>> {
    registry().lock().ok()?.get(journal)?.upgrade()
}

pub(crate) fn request_stop(journal: &Path) {
    if let Some(owner) = current(journal) {
        owner.shared.close_launch_credentials();
    }
}

/// Every operation refreshes from the admitted generation. The token is never
/// read from a port file or retained across worker replacement/Stop.
pub(crate) fn authority(journal: &Path) -> Option<solstone_core_local::LocalInferenceAuthority> {
    let owner = Arc::downgrade(&current(journal)?);
    Some(solstone_core_local::LocalInferenceAuthority::new(
        0,
        0,
        String::new(),
        Some(Box::new(move |deadline| {
            loop {
                let owner = owner
                    .upgrade()
                    .ok_or_else(|| "local provider owner retired".to_owned())?;
                if owner.shared.launch_credentials_closed()
                    || owner.root.revalidate_canonical_binding().is_err()
                    || Instant::now() >= deadline
                {
                    return Err("local provider credentials unavailable".into());
                }
                if let Some((generation, port, token)) = owner.shared.launch_credentials() {
                    let token = String::from_utf8(token).map_err(|error| error.to_string())?;
                    return Ok((generation, port, token));
                }
                drop(owner);
                std::thread::sleep(Duration::from_millis(25));
            }
        })),
    ))
}

fn request_shutdown(state: &mut ProviderRuntimeState, processes: &[ManagedProcess]) {
    state.shutdown_requested = true;
    state.start_cancelled = true;
    state.has_plan = false;
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

fn harvest(shared: &LocalRuntimeShared, state: &mut ProviderRuntimeState) {
    if let Some(work) = state.truth.as_mut()
        && work.result.is_none()
        && let Some(result) = shared.take_truth_result(&work.fence)
    {
        work.result = Some(result);
    }
    if let Some(work) = state.start.as_mut()
        && work.result.is_none()
        && let Some(result) = shared.take_launch_result(&work.fence)
    {
        work.result = Some(result);
    }
    if let Some(work) = state.stop_cleanup.as_mut()
        && work.result.is_none()
        && let Some(result) = shared.take_stop_cleanup_result(&work.fence)
    {
        work.result = Some(result);
    }
    if let Some(work) = state.probe.as_mut()
        && work.result.is_none()
        && let Some(result) = shared.take_probe_result(&work.fence)
    {
        work.result = Some(result);
    }
    // The coordinator deliberately ignores truth/probe after shutdown. Retire
    // those completed observations here, while still awaiting an actual result.
    if state.shutdown_requested {
        if state
            .truth
            .as_ref()
            .is_some_and(|work| work.result.is_some())
        {
            state.truth = None;
        }
        if state
            .probe
            .as_ref()
            .is_some_and(|work| work.result.is_some())
        {
            state.probe = None;
        }
    }
}

fn settled(
    state: &ProviderRuntimeState,
    processes: &[ManagedProcess],
    shared: &LocalRuntimeShared,
) -> bool {
    state.start.is_none()
        && state.stop_cleanup.is_none()
        && state.truth.is_none()
        && state.probe.is_none()
        && state.pending_stop_request.is_none()
        && state.orphaned_stop_requests.is_empty()
        && !processes.iter().any(|process| process.running)
        && !shared.has_owned_authority()
        && shared.reap_finished_workers()
}

/// A missing factory on desktop keeps host-only serving tests and existing BYO
/// operation unchanged. Real mobile hosts always reconcile their package state.
pub(crate) fn start(
    root: Arc<JournalRoot>,
    stop: watch::Receiver<bool>,
) -> Result<Option<impl std::future::Future<Output = ()> + Send + 'static>, String> {
    if !cfg!(any(target_os = "android", target_os = "ios"))
        && solstone_core_local::embedded::factory().is_none()
    {
        return Ok(None);
    }
    let shared = Arc::new(LocalRuntimeShared::default());
    shared.retain_bound_root(Arc::clone(&root));
    let owner = Arc::new(Owner {
        shared: Arc::clone(&shared),
        root,
    });
    {
        let mut registry = registry()
            .lock()
            .map_err(|_| "local provider registry unavailable")?;
        if registry
            .get(owner.root.canonical_path())
            .and_then(Weak::upgrade)
            .is_some()
        {
            return Err("local provider owner is still retained".into());
        }
        registry.insert(
            owner.root.canonical_path().to_path_buf(),
            Arc::downgrade(&owner),
        );
    }
    Ok(Some(run(owner, stop)))
}

async fn run(owner: Arc<Owner>, mut stop: watch::Receiver<bool>) {
    let shared = Arc::clone(&owner.shared);
    let root = &owner.root;
    let clock: Arc<dyn RuntimeClock> = Arc::new(SystemRuntimeClock::default());
    let coordinator = ProviderRuntimeCoordinator::new();
    let mut truth = LocalTruthSeam::new(Arc::clone(&shared), root.canonical_path());
    let mut lifecycle = LocalLifecycleSeam::new(Arc::clone(&shared), Arc::clone(&clock))
        .with_journal(root.canonical_path());
    let mut probe = LocalProbeSeam::new(Arc::clone(&shared), root.canonical_path());
    let mut store = FileRuntimeStore::new(
        root.canonical_path(),
        ProviderName::Local,
        shared.clone(),
        Arc::clone(&clock),
    );
    let mut state = ProviderRuntimeState::new(ProviderName::Local);
    let mut processes = Vec::new();
    let mut connection = CallosumSocketConnection::new(
        root.canonical_path().join("health/callosum.sock"),
        Map::new(),
    );
    connection.start();
    let mut connection_open = true;
    let mut stop_open = true;
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if *stop.borrow()
            || shared.launch_credentials_closed()
            || root.revalidate_canonical_binding().is_err()
        {
            shared.close_launch_credentials();
            request_shutdown(&mut state, &processes);
        }
        harvest(&shared, &mut state);
        shared.reap_finished_workers();
        let now = ProviderRuntimeNow {
            monotonic_seconds: clock.monotonic_seconds(),
        };
        let mut sink = VecEventSink::default();
        if !state.shutdown_requested
            && let Some(exited) = shared.observe_exited_current_process(&processes, Instant::now())
        {
            coordinator
                .observe_process_exit(now, &mut state, &processes, &exited, &mut store, &mut sink);
        }
        coordinator.reconcile(
            now,
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
        for effect in store.take_ready_side_effects() {
            let LocalReadySideEffect::RefreshBrain {
                expected_fingerprint_sha256,
            } = effect;
            connection.emit(
                "supervisor",
                "request",
                Map::from_iter([
                    (
                        "cmd".into(),
                        json!([
                            "solstone",
                            "journal",
                            "brain",
                            "refresh",
                            "--json",
                            "--expected-fingerprint",
                            expected_fingerprint_sha256
                        ]),
                    ),
                    (
                        "ref".into(),
                        json!(format!("embedded-local-ready-{}", state.generation)),
                    ),
                ]),
            );
        }
        if state.shutdown_requested && settled(&state, &processes, &shared) {
            break;
        }
        tokio::select! {
            _ = tick.tick() => {},
            changed = stop.changed(), if stop_open => {
                if changed.is_err() {
                    stop_open = false;
                    shared.close_launch_credentials();
                }
            },
            received = connection.next_event(), if connection_open => {
                if received.is_none() {
                    connection_open = false;
                    shared.close_launch_credentials();
                }
            },
        }
    }
    shared.close_launch_credentials();
    connection.stop().await;
    let mut registry = registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if registry
        .get(root.canonical_path())
        .and_then(Weak::upgrade)
        .is_some_and(|current| Arc::ptr_eq(&current, &owner))
    {
        registry.remove(root.canonical_path());
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use solstone_core_system::provider_runtime::{ProviderFence, ReadyProcessLookup};

    fn publish(shared: &LocalRuntimeShared, generation: u64, port: u16, token: &[u8]) {
        let fence = ProviderFence {
            incarnation: "mobile-authority-test".into(),
            generation,
            fingerprint: Some(format!("generation-{generation}")),
            attempt: 0,
        };
        shared.stage_launch_credentials(&fence, port, token.to_vec());
        shared.accept_ready(&fence);
    }

    #[test]
    fn operations_refresh_the_admitted_generation_and_old_authority_cannot_follow_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let root = Arc::new(JournalRoot::open(directory.path()).unwrap());
        let path = root.canonical_path().to_owned();
        let owner = Arc::new(Owner {
            shared: Arc::new(LocalRuntimeShared::default()),
            root: Arc::clone(&root),
        });
        registry()
            .lock()
            .unwrap()
            .insert(path.clone(), Arc::downgrade(&owner));
        let mut auth = authority(&path).unwrap();
        publish(&owner.shared, 1, 8123, b"first");
        auth.begin_operation(Instant::now() + Duration::from_secs(1))
            .unwrap();
        assert_eq!(
            (auth.generation(), auth.port(), auth.token()),
            (1, 8123, "first")
        );
        publish(&owner.shared, 2, 8124, b"second");
        auth.begin_operation(Instant::now() + Duration::from_secs(1))
            .unwrap();
        assert_eq!(
            (auth.generation(), auth.port(), auth.token()),
            (2, 8124, "second")
        );
        request_stop(&path);
        // Even a launch completion racing after Stop cannot reopen credentials.
        publish(&owner.shared, 3, 8125, b"late");
        assert!(
            auth.begin_operation(Instant::now() + Duration::from_secs(1))
                .is_err()
        );
        let replacement = Arc::new(Owner {
            shared: Arc::new(LocalRuntimeShared::default()),
            root,
        });
        registry()
            .lock()
            .unwrap()
            .insert(path.clone(), Arc::downgrade(&replacement));
        publish(&replacement.shared, 4, 8126, b"replacement");
        let mut new_auth = authority(&path).unwrap();
        new_auth
            .begin_operation(Instant::now() + Duration::from_secs(1))
            .unwrap();
        assert_eq!(new_auth.generation(), 4);
        assert!(
            auth.begin_operation(Instant::now() + Duration::from_secs(1))
                .is_err()
        );
        drop(owner);
        assert!(
            auth.begin_operation(Instant::now() + Duration::from_secs(1))
                .is_err()
        );
        registry().lock().unwrap().remove(&path);
    }
}
