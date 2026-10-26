// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::OpenOptions;
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use chrono::Utc;
use serde_json::{Map, Value};
use solstone_core_callosum::CallosumSocketConnection;
use solstone_core_journal_io::cortex_use::{
    CortexUseOperation, format_cortex_use_fatal, format_cortex_use_summary,
};
use solstone_core_system::lifecycle::{
    HostedServiceParentRuntime, HostedServiceShutdownEvidence, ParentLossReason,
};
use thiserror::Error;
use tokio::time::{Duration, MissedTickBehavior};

use crate::process::{cancel_worker, spawn_worker};
use crate::renewal::{Now, RenewalHandle, RenewalService, RenewalWorkerStart};
use crate::state::{CortexState, Outbound};
use crate::storage::CortexStore;

pub struct CallableRoots<'a> {
    pub talent_root: &'a Path,
    pub apps_root: &'a Path,
    pub templates_dir: &'a Path,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallableUse {
    pub use_id: String,
    pub completed: bool,
}

pub struct CallableCortex {
    journal: PathBuf,
    store: CortexStore,
}
impl CallableCortex {
    pub fn open(journal: &Path) -> Result<Self, String> {
        let store = CortexStore::new(journal.to_path_buf()).map_err(|e| e.to_string())?;
        store
            .recover()
            .map_err(|fatal| format_cortex_use_fatal(CortexUseOperation::Recovery, fatal))?;
        Ok(Self {
            journal: journal.to_path_buf(),
            store,
        })
    }
    pub fn execute(
        &self,
        roots: CallableRoots<'_>,
        request: serde_json::Map<String, Value>,
        generate: &dyn solstone_core_generate::GenerateExecutor,
        now_ms: i64,
        cancel: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<CallableUse, String> {
        execute_callable_with_store(
            &self.store,
            &self.journal,
            roots,
            request,
            generate,
            now_ms,
            cancel,
        )
    }
}

pub fn execute_callable_request(
    journal: &Path,
    roots: CallableRoots<'_>,
    request: serde_json::Map<String, serde_json::Value>,
    generate: &dyn solstone_core_generate::GenerateExecutor,
    now_ms: i64,
    cancel: &(dyn Fn() -> bool + Send + Sync),
) -> Result<CallableUse, String> {
    if cancel() {
        return Err("thinking cancelled".into());
    }
    let store = CortexStore::new(journal.to_path_buf()).map_err(|e| e.to_string())?;
    execute_callable_with_store(&store, journal, roots, request, generate, now_ms, cancel)
}

#[allow(clippy::too_many_arguments)]
fn execute_callable_with_store(
    store: &CortexStore,
    journal: &Path,
    roots: CallableRoots<'_>,
    mut request: serde_json::Map<String, Value>,
    generate: &dyn solstone_core_generate::GenerateExecutor,
    now_ms: i64,
    cancel: &(dyn Fn() -> bool + Send + Sync),
) -> Result<CallableUse, String> {
    if cancel() {
        return Err("thinking cancelled".into());
    }
    let use_id = match request.get("use_id").and_then(Value::as_str) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => {
            let id = solstone_core_journal_io::cortex_use::allocate_cortex_use_id(journal, now_ms)
                .map_err(|e| e.to_string())?;
            let s = id.to_string();
            request.insert("use_id".to_string(), Value::String(s.clone()));
            s
        }
    };

    let name = request
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "request missing name".to_string())?;

    if cancel() {
        return Err("thinking cancelled".into());
    }
    let (active_path, identity) = match store.claim_checked(name, &use_id, &request, cancel) {
        Ok(Some((path, identity))) => (path, identity),
        Ok(None) => return Err(format!("cortex use {use_id} for {name} is already claimed")),
        Err(err) => return Err(err.to_string()),
    };

    let runtime_paths = solstone_core_talent_runtime::prepare::RuntimePaths {
        talent_root: roots.talent_root.to_path_buf(),
        apps_root: roots.apps_root.to_path_buf(),
        templates_dir: roots.templates_dir.to_path_buf(),
    };
    let execution_context = solstone_core_talent_runtime::ExecutionContext {
        journal: journal.to_path_buf(),
    };

    let mut writer = Vec::new();
    let outcome = solstone_core_talent_runtime::execute_request(
        request.clone(),
        &runtime_paths,
        &execution_context,
        generate,
        &mut writer,
        &solstone_core_generate::Cancellation::from_fn(cancel),
    );
    if cancel()
        || matches!(
            outcome,
            solstone_core_talent_runtime::RuntimeOutcome::Cancelled
        )
    {
        return Ok(CallableUse {
            use_id,
            completed: false,
        });
    }
    // The talent wire deliberately omits caller context. Restore that context
    // at the Cortex boundary, just as the subprocess owner does for its bus.
    let mut terminal = Vec::new();
    solstone_core_talent_runtime::emit_outcome(&mut terminal, outcome);
    let mut finish = None;
    for line in terminal
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let mut event: Map<String, Value> =
            serde_json::from_slice(line).map_err(|e| e.to_string())?;
        event.insert("name".into(), Value::String(name.to_owned()));
        event.insert("use_id".into(), Value::String(use_id.clone()));
        if let Some(day) = request.get("day") {
            event.insert("day".into(), day.clone());
        }
        if event.get("event").and_then(Value::as_str) == Some("finish") {
            finish = Some(event.clone());
        }
        serde_json::to_writer(&mut writer, &event).map_err(|e| e.to_string())?;
        writer.push(b'\n');
    }

    if !writer.is_empty() {
        let mut file = OpenOptions::new()
            .append(true)
            .open(&active_path)
            .map_err(|e| e.to_string())?;
        if cancel() {
            return Ok(CallableUse {
                use_id,
                completed: false,
            });
        }
        file.write_all(&writer).map_err(|e| e.to_string())?;
    }

    if cancel() {
        return Ok(CallableUse {
            use_id,
            completed: false,
        });
    }

    let completed = store.complete_checked(&use_id, name, identity, Some(&request), cancel);
    if completed
        && !cancel()
        && let Some(mut fields) = finish
    {
        // Publish only after durable completion. Startup scan covers a missed
        // best-effort bus send; no result body is needed by subscribers.
        fields.retain(|key, _| matches!(key.as_str(), "name" | "use_id" | "day" | "skip_reason"));
        let envelope = solstone_core_callosum::CallosumEnvelope {
            tract: "cortex".into(),
            event: "finish".into(),
            ts: Some(now_ms),
            extra: fields,
        };
        if let Ok(mut line) = serde_json::to_string(&envelope) {
            line.push('\n');
            let _ = solstone_core_callosum::CallosumOneShotSender::new(
                journal.join("health/callosum.sock"),
                Duration::from_secs(1),
            )
            .send_line(&line);
        }
    }
    Ok(CallableUse { use_id, completed })
}

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
    #[error("cortex recovery failed: {0}")]
    Recovery(String),
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
            Self::Recovery(_) => "recovery",
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
    let recovery = store.recover().map_err(|fatal| {
        let diagnostic = format_cortex_use_fatal(CortexUseOperation::Recovery, fatal);
        log::error!("{diagnostic}");
        CortexServiceError::Recovery(diagnostic)
    })?;
    if let Some(diagnostic) =
        format_cortex_use_summary(CortexUseOperation::Recovery, recovery.refusals())
    {
        log::warn!("{diagnostic}");
    }
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
        _ = parent.await_retire_expected_request() => ShutdownMode::Immediate,
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
                        serde_json::json!(["solstone", "journal", "brain", "refresh"]),
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
        assert_eq!(
            CortexServiceError::Recovery("failed".into()).class(),
            "recovery"
        );
        let text = error.to_string();
        assert!(
            text.contains(&bin.display().to_string()),
            "InstallationRoot Display must carry the diagnostic: {text}"
        );
        assert!(text.contains("pyproject.toml"));
        assert_ne!(error.class(), "runtime");
    }

    fn mock_generated_response() -> solstone_core_generate::GenerateResponse {
        solstone_core_generate::GenerateResponse::Generated(Box::new(
            solstone_core_generate::GeneratedResponse {
                id: None,
                text: "{}".into(),
                model: "test-model".into(),
                usage: serde_json::json!({}),
                finish_reason: "stop".into(),
                thinking: None,
                schema_validation: None,
                input_budget: None,
                request_budget: None,
                inference: None,
                hints_applied: Vec::new(),
            },
        ))
    }

    struct TestFixture {
        _root: tempfile::TempDir,
        journal: PathBuf,
        talent_root: PathBuf,
        apps_root: PathBuf,
        templates_dir: PathBuf,
    }

    impl TestFixture {
        fn new(talent_name: &str) -> Self {
            let root = tempfile::tempdir().unwrap();
            let journal = root.path().join("journal");
            let talent_root = root.path().join("talent");
            let apps_root = root.path().join("apps");
            let templates_dir = root.path().join("templates");

            std::fs::create_dir_all(&journal).unwrap();
            std::fs::create_dir_all(journal.join("config")).unwrap();
            std::fs::write(
                journal.join("config/journal.json"),
                r#"{"providers":{"active":{"provider":"test","model":"test-model"}}}"#,
            )
            .unwrap();

            std::fs::create_dir_all(&talent_root).unwrap();
            std::fs::create_dir_all(&apps_root).unwrap();
            std::fs::create_dir_all(&templates_dir).unwrap();

            std::fs::write(
                talent_root.join(format!("{talent_name}.md")),
                "{\n\"max_output_tokens\":1024\n}\nworker fixture",
            )
            .unwrap();

            Self {
                _root: root,
                journal,
                talent_root,
                apps_root,
                templates_dir,
            }
        }

        fn roots(&self) -> CallableRoots<'_> {
            CallableRoots {
                talent_root: &self.talent_root,
                apps_root: &self.apps_root,
                templates_dir: &self.templates_dir,
            }
        }
    }

    #[test]
    fn two_calls_with_same_now_ms_and_no_use_id_produce_increasing_ids() {
        let fixture = TestFixture::new("call_talent");
        let now_ms = 1_700_000_000_000;
        let generate =
            |_req: &solstone_core_generate::GenerateRequest| Ok(mock_generated_response());
        let cancel = || false;

        let req1 = serde_json::Map::from_iter([
            ("name".into(), Value::String("call_talent".into())),
            ("prompt".into(), Value::String("call1".into())),
        ]);
        let res1 = execute_callable_request(
            &fixture.journal,
            fixture.roots(),
            req1,
            &generate,
            now_ms,
            &cancel,
        )
        .unwrap();

        let req2 = serde_json::Map::from_iter([
            ("name".into(), Value::String("call_talent".into())),
            ("prompt".into(), Value::String("call2".into())),
        ]);
        let res2 = execute_callable_request(
            &fixture.journal,
            fixture.roots(),
            req2,
            &generate,
            now_ms,
            &cancel,
        )
        .unwrap();

        assert!(res1.completed);
        assert!(res2.completed);
        let id1: i64 = res1.use_id.parse().unwrap();
        let id2: i64 = res2.use_id.parse().unwrap();
        assert!(id2 > id1, "ids must be strictly increasing: {id2} > {id1}");

        let talents_dir = fixture.journal.join("talents/call_talent");
        assert!(talents_dir.join(format!("{}.jsonl", res1.use_id)).is_file());
        assert!(
            !talents_dir
                .join(format!("{}_active.jsonl", res1.use_id))
                .exists()
        );
        assert!(talents_dir.join(format!("{}.jsonl", res2.use_id)).is_file());
        assert!(
            !talents_dir
                .join(format!("{}_active.jsonl", res2.use_id))
                .exists()
        );
    }

    #[test]
    fn cancelled_use_is_recovered_only_by_a_new_owner() {
        let fixture = TestFixture::new("call_talent");
        let owner = CallableCortex::open(&fixture.journal).unwrap();
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        let generate = |_: &solstone_core_generate::GenerateRequest| {
            cancelled.store(true, std::sync::atomic::Ordering::Release);
            Ok(mock_generated_response())
        };
        let first = owner
            .execute(
                fixture.roots(),
                Map::from_iter([
                    ("name".into(), Value::String("call_talent".into())),
                    ("prompt".into(), Value::String("first".into())),
                ]),
                &generate,
                1_700_000_000_000,
                &|| cancelled.load(std::sync::atomic::Ordering::Acquire),
            )
            .unwrap();
        assert!(!first.completed);
        let directory = fixture.journal.join("talents/call_talent");
        let active = directory.join(format!("{}_active.jsonl", first.use_id));
        let terminal = directory.join(format!("{}.jsonl", first.use_id));
        assert!(active.is_file());
        assert!(!terminal.exists());
        drop(owner);
        let restarted = CallableCortex::open(&fixture.journal).unwrap();
        assert!(!active.exists());
        assert!(terminal.is_file());
        let second = restarted
            .execute(
                fixture.roots(),
                Map::from_iter([
                    ("name".into(), Value::String("call_talent".into())),
                    ("prompt".into(), Value::String("second".into())),
                ]),
                &|_: &solstone_core_generate::GenerateRequest| Ok(mock_generated_response()),
                1_700_000_000_001,
                &|| false,
            )
            .unwrap();
        assert!(second.completed);
        assert_ne!(first.use_id, second.use_id);
    }

    #[test]
    fn completed_use_second_call_with_same_use_id_refuses_and_does_not_call_generate() {
        let fixture = TestFixture::new("call_talent");
        let now_ms = 1_700_000_000_000;
        let counter1 = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter1_clone = Arc::clone(&counter1);
        let generate1 = move |_req: &solstone_core_generate::GenerateRequest| {
            counter1_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(mock_generated_response())
        };
        let cancel = || false;

        let req1 = serde_json::Map::from_iter([
            ("name".into(), Value::String("call_talent".into())),
            ("prompt".into(), Value::String("call1".into())),
        ]);
        let res1 = execute_callable_request(
            &fixture.journal,
            fixture.roots(),
            req1,
            &generate1,
            now_ms,
            &cancel,
        )
        .unwrap();

        assert!(res1.completed);
        assert_eq!(counter1.load(std::sync::atomic::Ordering::SeqCst), 1);
        let term_path = fixture
            .journal
            .join("talents/call_talent")
            .join(format!("{}.jsonl", res1.use_id));
        let bytes_before = std::fs::read(&term_path).unwrap();

        let counter2 = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter2_clone = Arc::clone(&counter2);
        let generate2 = move |_req: &solstone_core_generate::GenerateRequest| {
            counter2_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(mock_generated_response())
        };
        let req2 = serde_json::Map::from_iter([
            ("name".into(), Value::String("call_talent".into())),
            ("use_id".into(), Value::String(res1.use_id.clone())),
            ("prompt".into(), Value::String("call2".into())),
        ]);
        let res2 = execute_callable_request(
            &fixture.journal,
            fixture.roots(),
            req2,
            &generate2,
            now_ms,
            &cancel,
        );

        assert!(
            res2.is_err(),
            "second call with same use_id must return Err"
        );
        assert_eq!(
            counter2.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "generate must not be called on second call"
        );
        let bytes_after = std::fs::read(&term_path).unwrap();
        assert_eq!(
            bytes_before, bytes_after,
            "terminal bytes must be unchanged"
        );
    }

    #[test]
    fn generate_error_or_refusal_does_not_retry_and_leaves_terminal_record() {
        let fixture = TestFixture::new("call_talent");
        let now_ms = 1_700_000_000_000;
        let cancel = || false;

        // Part A: GenerateExecutor returns Err
        let counter_err = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter_err_clone = Arc::clone(&counter_err);
        let generate_err = move |_req: &solstone_core_generate::GenerateRequest| {
            counter_err_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(solstone_core_generate::ClientError::Resolve(
                "simulated transport error".into(),
            ))
        };
        let req1 = serde_json::Map::from_iter([
            ("name".into(), Value::String("call_talent".into())),
            ("prompt".into(), Value::String("err".into())),
        ]);
        let res1 = execute_callable_request(
            &fixture.journal,
            fixture.roots(),
            req1,
            &generate_err,
            now_ms,
            &cancel,
        )
        .unwrap();

        assert_eq!(
            counter_err.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "exactly one invocation on Err"
        );
        assert!(res1.completed);
        let term1 = fixture
            .journal
            .join("talents/call_talent")
            .join(format!("{}.jsonl", res1.use_id));
        assert!(
            term1.is_file(),
            "terminal record must exist for Err when cancel is false"
        );

        // Part B: GenerateExecutor returns RefusedResponse
        let counter_refused = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter_refused_clone = Arc::clone(&counter_refused);
        let generate_refused = move |_req: &solstone_core_generate::GenerateRequest| {
            counter_refused_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(solstone_core_generate::GenerateResponse::Refused(
                solstone_core_generate::RefusedResponse {
                    id: None,
                    reason: solstone_core_generate::RefusalReason::Unknown,
                    reason_code: None,
                    retryable: false,
                    blocking: false,
                    reset_at_ms: None,
                    provider: Some("test".into()),
                    detail: "quota exhausted".into(),
                },
            ))
        };
        let req2 = serde_json::Map::from_iter([
            ("name".into(), Value::String("call_talent".into())),
            ("prompt".into(), Value::String("refused".into())),
        ]);
        let res2 = execute_callable_request(
            &fixture.journal,
            fixture.roots(),
            req2,
            &generate_refused,
            now_ms + 100,
            &cancel,
        )
        .unwrap();

        assert_eq!(
            counter_refused.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "exactly one invocation on Refused"
        );
        assert!(res2.completed);
        let term2 = fixture
            .journal
            .join("talents/call_talent")
            .join(format!("{}.jsonl", res2.use_id));
        assert!(
            term2.is_file(),
            "terminal record must exist for Refused when cancel is false"
        );
    }
    #[cfg(feature = "full-tests")]
    #[test]
    fn callable_owner_keeps_a_live_use_while_another_use_completes() {
        let fixture = TestFixture::new("call_talent");
        let owner = Arc::new(CallableCortex::open(&fixture.journal).unwrap());
        let (entered_send, entered) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let released = Mutex::new(released);
        let first_owner = Arc::clone(&owner);
        let talent = fixture.talent_root.clone();
        let apps = fixture.apps_root.clone();
        let templates = fixture.templates_dir.clone();
        let first = thread::spawn(move || {
            let generate = move |_: &solstone_core_generate::GenerateRequest| {
                entered_send.send(()).unwrap();
                released.lock().unwrap().recv().unwrap();
                Ok(mock_generated_response())
            };
            first_owner
                .execute(
                    CallableRoots {
                        talent_root: &talent,
                        apps_root: &apps,
                        templates_dir: &templates,
                    },
                    Map::from_iter([
                        ("name".into(), Value::String("call_talent".into())),
                        ("prompt".into(), Value::String("first".into())),
                    ]),
                    &generate,
                    1_700_000_000_000,
                    &|| false,
                )
                .unwrap()
        });
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        let directory = fixture.journal.join("talents/call_talent");
        let active = std::fs::read_dir(&directory)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .ends_with("_active.jsonl")
            })
            .unwrap();
        let second = owner
            .execute(
                fixture.roots(),
                Map::from_iter([
                    ("name".into(), Value::String("call_talent".into())),
                    ("prompt".into(), Value::String("second".into())),
                ]),
                &|_: &solstone_core_generate::GenerateRequest| Ok(mock_generated_response()),
                1_700_000_000_001,
                &|| false,
            )
            .unwrap();
        assert!(second.completed);
        assert!(
            active.is_file(),
            "another live use must not be restart-recovered"
        );
        assert!(
            !std::fs::read_to_string(&active)
                .unwrap()
                .contains("interrupted by")
        );
        release.send(()).unwrap();
        assert!(first.join().unwrap().completed);
        assert!(!active.exists());
    }

    #[test]
    fn callable_cancellation_during_generate_leaves_no_finish_or_terminal_success() {
        let fixture = TestFixture::new("call_talent");
        let owner = CallableCortex::open(&fixture.journal).unwrap();
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        let generate = |_: &solstone_core_generate::GenerateRequest| {
            cancelled.store(true, std::sync::atomic::Ordering::Release);
            Ok(mock_generated_response())
        };
        let result = owner
            .execute(
                fixture.roots(),
                Map::from_iter([
                    ("name".into(), Value::String("call_talent".into())),
                    ("prompt".into(), Value::String("cancel".into())),
                ]),
                &generate,
                1_700_000_000_000,
                &|| cancelled.load(std::sync::atomic::Ordering::Acquire),
            )
            .unwrap();
        assert!(!result.completed);
        let directory = fixture.journal.join("talents/call_talent");
        assert!(!directory.join(format!("{}.jsonl", result.use_id)).exists());
        let active =
            std::fs::read_to_string(directory.join(format!("{}_active.jsonl", result.use_id)))
                .unwrap();
        assert!(
            !active
                .lines()
                .any(|line| serde_json::from_str::<Value>(line)
                    .is_ok_and(|v| v["event"] == "finish"))
        );
        let positive = owner
            .execute(
                fixture.roots(),
                Map::from_iter([
                    ("name".into(), Value::String("call_talent".into())),
                    ("prompt".into(), Value::String("positive".into())),
                ]),
                &|_: &solstone_core_generate::GenerateRequest| Ok(mock_generated_response()),
                1_700_000_000_001,
                &|| false,
            )
            .unwrap();
        assert!(positive.completed);
        assert!(
            directory
                .join(format!("{}.jsonl", positive.use_id))
                .is_file()
        );
    }
}
