// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use chrono::Utc;
use serde_json::{Map, Value};
use solstone_core_callosum::CallosumSocketConnection;
use solstone_core_system::lifecycle::{
    HostedServiceParentRuntime, HostedServiceShutdownEvidence, ParentLossReason,
};
use thiserror::Error;
use tokio::time::{Duration, MissedTickBehavior};

use crate::process::{cancel_worker, spawn_worker};
use crate::renewal::{Now, RenewalHandle, RenewalService, RenewalWorkerStart};
use crate::state::{CortexState, Outbound};
use crate::storage::CortexStore;

#[derive(Clone, Copy, Debug)]
pub struct CortexOptions {
    pub verbose: bool,
    pub debug: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShutdownMode {
    Immediate,
    Drain,
}

#[derive(Debug, Error)]
pub enum CortexServiceError {
    #[error("cortex runtime unavailable")]
    Runtime,
    #[error("could not inspect current executable: {0}")]
    CurrentExecutable(#[source] std::io::Error),
    #[error("{0}")]
    InstallationRoot(String),
    #[error("could not initialize cortex journal storage: {0}")]
    Storage(#[source] std::io::Error),
    #[error("hosted parent-loss coordination failed: {0}")]
    ParentLoss(String),
}

impl CortexServiceError {
    pub const fn class(&self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::CurrentExecutable(_) => "executable",
            Self::InstallationRoot(_) => "installation-root",
            Self::Storage(_) => "storage",
            Self::ParentLoss(_) => "parent-loss",
        }
    }
}

pub async fn run_native_service(
    journal: PathBuf,
    options: CortexOptions,
) -> Result<(), CortexServiceError> {
    run_native_service_with_hosted_parent(journal, options, None).await
}

/// Run Cortex with an optional birth-admitted hosted parent lifetime.
pub async fn run_native_service_with_hosted_parent(
    journal: PathBuf,
    options: CortexOptions,
    hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
) -> Result<(), CortexServiceError> {
    if options.verbose {
        eprintln!("cortex: starting native service");
    }
    if options.debug {
        eprintln!("cortex: debug diagnostics enabled");
    }
    let executable = std::env::current_exe().map_err(CortexServiceError::CurrentExecutable)?;
    let executable_dir = executable
        .parent()
        .map(PathBuf::from)
        .ok_or(CortexServiceError::Runtime)?;
    let (talent_root, apps_root, templates_dir) =
        package_roots_from_executable_dir(&executable_dir).ok_or_else(|| {
            CortexServiceError::InstallationRoot(
                solstone_core_journal::describe_installation_root_miss(&executable_dir),
            )
        })?;
    let connection =
        CallosumSocketConnection::new(journal.join("health/callosum.sock"), Map::new());
    let parent_loss = Arc::new(Mutex::new(None));
    let result = run_until(
        journal,
        connection,
        executable_dir,
        talent_root,
        apps_root,
        templates_dir,
        hosted_parent.clone(),
        shutdown_with_hosted_parent(hosted_parent.clone(), Arc::clone(&parent_loss)),
    )
    .await;
    let service_stopped = result.is_ok();
    if let Some(parent) = hosted_parent {
        let mut reason = parent_loss
            .lock()
            .expect("hosted parent-loss reason lock poisoned")
            .take();
        if reason.is_none() {
            reason = parent.await_parent_loss_or_retire_expected_request().await;
        }
        if reason.is_some() {
            parent
                .finish_parent_loss(HostedServiceShutdownEvidence {
                    // `run_until` returns `Ok` only after it has stopped the
                    // Callosum connection and its service runner.
                    listener_stopped: service_stopped,
                    service_runner_stopped: service_stopped,
                    // Cortex has no separate health artifact to withdraw.
                    operational_artifacts_cleaned: true,
                })
                .map_err(|error| CortexServiceError::ParentLoss(error.to_string()))?;
        }
    }
    result
}

pub(crate) fn package_roots_from_executable_dir(
    executable_dir: &Path,
) -> Option<(PathBuf, PathBuf, PathBuf)> {
    let root =
        solstone_core_journal::resolve_installation_root_from_executable_dir(executable_dir)?;
    let talent_root = root.join("solstone/talent");
    let apps_root = root.join("solstone/apps");
    let templates_dir = talent_root.parent()?.join("think/templates");
    Some((talent_root, apps_root, templates_dir))
}

#[allow(clippy::too_many_arguments)] // Service paths and hosted-parent ownership are independently injected.
pub async fn run_until<F>(
    journal: PathBuf,
    connection: CallosumSocketConnection,
    executable_dir: PathBuf,
    talent_root: PathBuf,
    apps_root: PathBuf,
    templates_dir: PathBuf,
    hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
    shutdown: F,
) -> Result<(), CortexServiceError>
where
    F: Future<Output = ShutdownMode> + Send + 'static,
{
    run_until_with(
        journal,
        connection,
        TalentExecutionPaths {
            executable_dir,
            talent_root,
            apps_root,
            templates_dir,
        },
        hosted_parent,
        shutdown,
        ServiceDependencies::production(),
    )
    .await
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ServiceLifecycle {
    BusConnected,
    StartupRefreshEmitted {
        tract: &'static str,
        event: String,
        fields: Map<String, Value>,
    },
    RenewalWorker,
    MainLoop,
}

type RenewalWorkerFactory = Arc<dyn Fn(RenewalHandle) -> RenewalWorkerStart + Send + Sync>;
type LifecycleObserver = Arc<dyn Fn(ServiceLifecycle) + Send + Sync>;

struct TalentExecutionPaths {
    executable_dir: PathBuf,
    talent_root: PathBuf,
    apps_root: PathBuf,
    templates_dir: PathBuf,
}

struct ServiceDependencies {
    now: Now,
    worker_factory: RenewalWorkerFactory,
    lifecycle: LifecycleObserver,
    worker_start_requests: usize,
}

impl ServiceDependencies {
    fn production() -> Self {
        Self {
            now: Arc::new(Utc::now),
            worker_factory: Arc::new(RenewalWorkerStart::production),
            lifecycle: Arc::new(|_| {}),
            worker_start_requests: 1,
        }
    }
}

async fn run_until_with<F>(
    journal: PathBuf,
    mut connection: CallosumSocketConnection,
    execution_paths: TalentExecutionPaths,
    hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
    shutdown: F,
    dependencies: ServiceDependencies,
) -> Result<(), CortexServiceError>
where
    F: Future<Output = ShutdownMode> + Send + 'static,
{
    let store = CortexStore::new(journal.clone()).map_err(CortexServiceError::Storage)?;
    // Recovery intentionally happens before this connection is started.
    store.recover();
    let (spawn_tx, spawn_rx) = mpsc::channel();
    let (cancel_tx, cancel_rx) = mpsc::channel();
    let (outbound_tx, outbound_rx) = mpsc::channel();
    let state = CortexState::new(store, spawn_tx, cancel_tx, outbound_tx.clone());
    let spawn_state = state.clone();
    thread::spawn(move || {
        spawn_worker(
            spawn_state,
            execution_paths.executable_dir,
            execution_paths.talent_root,
            execution_paths.apps_root,
            execution_paths.templates_dir,
            spawn_rx,
            hosted_parent,
        )
    });
    let cancel_state = state.clone();
    thread::spawn(move || cancel_worker(cancel_state, cancel_rx));
    connection.start();
    (dependencies.lifecycle)(ServiceLifecycle::BusConnected);
    let renewal_handle = RenewalHandle::production(journal, outbound_tx, dependencies.now.clone());
    let mut renewal = start_renewal(&renewal_handle, &dependencies);
    let mut status = tokio::time::interval(Duration::from_secs(5));
    let mut drain = tokio::time::interval(Duration::from_millis(10));
    status.set_missed_tick_behavior(MissedTickBehavior::Skip);
    drain.set_missed_tick_behavior(MissedTickBehavior::Skip);
    tokio::pin!(shutdown);
    let mut draining = false;
    (dependencies.lifecycle)(ServiceLifecycle::MainLoop);
    loop {
        tokio::select! {
            _ = status.tick() => state.status(state.queue_depth()),
            _ = drain.tick() => drain_outbound(&connection, &outbound_rx),
            mode = &mut shutdown, if !draining => {
                state.stop_accepting();
                match mode {
                    ShutdownMode::Immediate => {
                        for running in state.stop_immediately() {
                            let _ = running
                                .authority
                                .lock()
                                .expect("cortex authority lock poisoned")
                                .terminate(Duration::from_secs(10));
                        }
                        break;
                    }
                    ShutdownMode::Drain => draining = true,
                }
            },
            message = connection.next_message(), if !draining => match message {
                Some(message) => dispatch(&state, &renewal_handle, message.tract.as_str(), message.event.as_str(), message.extra),
                None => break,
            },
            _ = tokio::time::sleep(Duration::from_millis(20)), if draining && state.is_idle() => break,
        }
    }
    drain_outbound(&connection, &outbound_rx);
    renewal.stop();
    connection.stop().await;
    Ok(())
}

fn start_renewal(
    renewal_handle: &RenewalHandle,
    dependencies: &ServiceDependencies,
) -> RenewalService {
    if renewal_handle.startup_refresh_needed()
        && let Some(outbound) = renewal_handle.startup_refresh()
    {
        (dependencies.lifecycle)(ServiceLifecycle::StartupRefreshEmitted {
            tract: outbound.tract,
            event: outbound.event,
            fields: outbound.fields,
        });
    }
    let mut renewal = RenewalService::new((dependencies.worker_factory)(renewal_handle.clone()));
    for _ in 0..dependencies.worker_start_requests {
        if renewal.start_worker_once() {
            (dependencies.lifecycle)(ServiceLifecycle::RenewalWorker);
        }
    }
    renewal
}

pub(crate) fn dispatch(
    state: &CortexState,
    renewal: &RenewalHandle,
    tract: &str,
    event: &str,
    fields: Map<String, Value>,
) {
    match tract {
        "cortex" => match event {
            "request" => state.request(fields),
            "cancel" => state.queue_cancel(&fields),
            _ => {}
        },
        "supervisor" => renewal.handle_supervisor(event, &fields),
        _ => {}
    }
}

fn drain_outbound(connection: &CallosumSocketConnection, receiver: &mpsc::Receiver<Outbound>) {
    while let Ok(outbound) = receiver.try_recv() {
        let _ = connection.emit(outbound.tract, &outbound.event, outbound.fields);
    }
}

async fn shutdown_signal() -> ShutdownMode {
    #[cfg(unix)]
    {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! {
                _ = signal.recv() => return ShutdownMode::Immediate,
                _ = tokio::signal::ctrl_c() => return ShutdownMode::Drain,
            }
        }
    }
    let _ = tokio::signal::ctrl_c().await;
    ShutdownMode::Drain
}

async fn shutdown_with_hosted_parent(
    hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
    parent_loss: Arc<Mutex<Option<ParentLossReason>>>,
) -> ShutdownMode {
    let Some(parent) = hosted_parent else {
        return shutdown_signal().await;
    };
    tokio::select! {
        mode = shutdown_signal() => mode,
        reason = parent.await_parent_loss() => {
            *parent_loss
                .lock()
                .expect("hosted parent-loss reason lock poisoned") = Some(reason);
            ShutdownMode::Immediate
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use std::sync::{Arc, Mutex, mpsc};

    use super::*;

    fn renewal_handle(directory: &tempfile::TempDir) -> (RenewalHandle, mpsc::Receiver<Outbound>) {
        let (outbound, receiver) = mpsc::channel();
        (
            RenewalHandle::production(directory.path().to_path_buf(), outbound, Arc::new(test_now)),
            receiver,
        )
    }

    fn test_now() -> DateTime<Utc> {
        chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 8, 6, 12, 0, 0).unwrap()
    }

    fn test_dependencies(
        lifecycle: Arc<Mutex<Vec<ServiceLifecycle>>>,
        factory_calls: Arc<Mutex<usize>>,
        worker_start_requests: usize,
    ) -> ServiceDependencies {
        ServiceDependencies {
            now: Arc::new(test_now),
            worker_factory: Arc::new(move |handle| {
                *factory_calls.lock().unwrap() += 1;
                RenewalWorkerStart::test(handle, Arc::new(|_| true))
            }),
            lifecycle: Arc::new(move |entry| lifecycle.lock().unwrap().push(entry)),
            worker_start_requests,
        }
    }

    #[test]
    fn dispatch_filters_only_at_service_boundary() {
        let directory = tempfile::tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let (spawn_tx, _) = mpsc::channel();
        let (cancel_tx, _) = mpsc::channel();
        let (outbound_tx, _) = mpsc::channel();
        let state = CortexState::new(store, spawn_tx, cancel_tx, outbound_tx);
        let (renewal, _) = renewal_handle(&directory);
        let before = renewal.snapshot();
        dispatch(&state, &renewal, "other", "started", Map::new());
        assert_eq!(renewal.snapshot(), before);
    }

    #[test]
    fn service_starts_one_renewal_worker_once_even_when_start_requested_twice() {
        let directory = tempfile::tempdir().unwrap();
        let lifecycle = Arc::new(Mutex::new(Vec::new()));
        let factory_calls = Arc::new(Mutex::new(0));
        let (handle, _outbound) = renewal_handle(&directory);
        let mut renewal = start_renewal(
            &handle,
            &test_dependencies(lifecycle.clone(), factory_calls.clone(), 2),
        );
        renewal.stop();
        assert_eq!(
            lifecycle
                .lock()
                .unwrap()
                .iter()
                .filter(|entry| matches!(entry, ServiceLifecycle::RenewalWorker))
                .count(),
            1
        );
        assert_eq!(*factory_calls.lock().unwrap(), 1);
    }

    #[test]
    fn startup_refresh_is_emitted_before_the_single_renewal_worker_starts() {
        let directory = tempfile::tempdir().unwrap();
        let lifecycle = Arc::new(Mutex::new(Vec::new()));
        let factory_calls = Arc::new(Mutex::new(0));
        let (handle, _outbound) = renewal_handle(&directory);
        let mut renewal = start_renewal(
            &handle,
            &test_dependencies(lifecycle.clone(), factory_calls, 1),
        );
        renewal.stop();
        assert_eq!(
            &*lifecycle.lock().unwrap(),
            &[
                ServiceLifecycle::StartupRefreshEmitted {
                    tract: "supervisor",
                    event: "request".into(),
                    fields: Map::from_iter([(
                        "cmd".into(),
                        serde_json::json!(["journal", "brain", "refresh"]),
                    )]),
                },
                ServiceLifecycle::RenewalWorker,
            ]
        );
    }

    #[test]
    fn share_layout_resolves_and_fails_when_anchor_removed() {
        let directory = tempfile::tempdir().unwrap();
        let prefix = directory.path().join("tree");
        let bin = prefix.join("bin");
        let share = prefix.join("share");
        std::fs::create_dir_all(&bin).unwrap();
        for relative in [
            solstone_core_journal::LAYOUT_BUNDLE_ANCHOR,
            solstone_core_journal::LAYOUT_LAYOUT_ANCHOR,
            solstone_core_journal::LAYOUT_TEMPLATE_ANCHOR,
        ] {
            let path = share.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, relative).unwrap();
        }
        let (talent, apps, templates) = package_roots_from_executable_dir(&bin).unwrap();
        assert_eq!(talent, share.join("solstone/talent"));
        assert_eq!(apps, share.join("solstone/apps"));
        assert_eq!(templates, share.join("solstone/think/templates"));
        std::fs::remove_file(share.join(solstone_core_journal::LAYOUT_BUNDLE_ANCHOR)).unwrap();
        assert!(package_roots_from_executable_dir(&bin).is_none());
        let error = CortexServiceError::InstallationRoot(
            solstone_core_journal::describe_installation_root_miss(&bin),
        );
        assert_eq!(error.class(), "installation-root");
        assert_ne!(CortexServiceError::Runtime.class(), "installation-root");
        assert_eq!(
            CortexServiceError::CurrentExecutable(std::io::Error::other("inspect")).class(),
            "executable"
        );
        assert_eq!(
            CortexServiceError::Storage(std::io::Error::other("storage")).class(),
            "storage"
        );
        let text = error.to_string();
        assert!(
            text.contains(&bin.display().to_string()),
            "InstallationRoot Display must carry the diagnostic: {text}"
        );
        assert!(text.contains("pyproject.toml"));
        assert_ne!(error.class(), "runtime");
    }
}
