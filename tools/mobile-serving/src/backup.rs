// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, json};
use solstone_core_backup::archive_admission::ArchiveCancellation;
use solstone_core_backup_runtime::{
    hosted_runtime::{HttpError, HttpRequest, HttpResponse, HttpTransport},
    runner::{ToolOutput, ToolRequest, ToolRunner},
};
use solstone_core_backup_web::BackupWebDeps;
#[cfg(feature = "processing")]
use solstone_core_callosum::CallosumOneShotSender;
use solstone_core_callosum::{
    CallosumConnectionPhase, CallosumEnvelope, CallosumReceiveEvent, CallosumSocketConnection,
};
use solstone_core_journal_io::JournalRoot;
use solstone_core_maintenance::{CliRun, EmbeddedBackupRoutine};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tokio::{
    sync::{oneshot, watch},
    task::JoinHandle,
};

static NEXT_START: AtomicU64 = AtomicU64::new(1);

#[cfg(all(
    test,
    feature = "full-tests",
    unix,
    not(any(target_os = "android", target_os = "ios"))
))]
#[path = "backup_current_effect_tests.rs"]
mod current_effect_tests;

fn routine(envelope: &CallosumEnvelope) -> Option<EmbeddedBackupRoutine> {
    if envelope.tract != "supervisor" || envelope.event != "request" {
        return None;
    }
    let command = envelope.extra.get("cmd")?.as_array()?;
    if command.len() != 5
        || command[..4]
            != [
                json!("solstone"),
                json!("journal"),
                json!("maintenance"),
                json!("run"),
            ]
    {
        return None;
    }
    match command[4].as_str()? {
        "backup:run" => Some(EmbeddedBackupRoutine::Run),
        "backup:verify" => Some(EmbeddedBackupRoutine::Verify),
        "backup:prune" => Some(EmbeddedBackupRoutine::Prune),
        _ => None,
    }
}

/// One terminal disposition for an embedded backup maintenance run.
///
/// Bus submit, permit release, and task completion are not this disposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackupDisposition {
    Success,
    Refusal,
    Failure,
}

/// Classify the current `CliRun` only.
///
/// A stored `last_backup`, `last_verification`, or `last_prune` status is
/// history. It does not change this run.
pub(crate) fn classify_backup_run(run: &CliRun, root_bound: bool) -> BackupDisposition {
    if !root_bound {
        return BackupDisposition::Refusal;
    }
    if run
        .stdout
        .split_whitespace()
        .any(|token| token == "reason=refused")
    {
        return BackupDisposition::Refusal;
    }
    if run.exit_code == 0 {
        BackupDisposition::Success
    } else {
        BackupDisposition::Failure
    }
}

pub(crate) fn backup_terminal(root: &JournalRoot, run: &CliRun) -> BackupDisposition {
    classify_backup_run(run, root.revalidate_canonical_binding().is_ok())
}

pub(crate) fn log_backup_terminal(disposition: BackupDisposition, exit_code: i32) {
    match disposition {
        BackupDisposition::Success => {}
        BackupDisposition::Refusal => {
            log::warn!("embedded backup maintenance refused");
        }
        BackupDisposition::Failure => {
            log::warn!("embedded backup maintenance failed (exit {exit_code})");
        }
    }
}

/// The worker's typed result for one backup request.
///
/// `disposition` is this result. Emitting it on the bus is a separate report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackupSettlement {
    pub(crate) reference: String,
    pub(crate) routine: EmbeddedBackupRoutine,
    pub(crate) disposition: BackupDisposition,
    pub(crate) exit_code: i32,
}

/// Admission and overlap are outcomes of the request. They are not a log line
/// in place of a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BackupRequestOutcome {
    Settled(BackupSettlement),
    AdmissionRefused {
        reference: String,
        routine: EmbeddedBackupRoutine,
    },
    WorkerActive {
        reference: String,
        routine: EmbeddedBackupRoutine,
    },
}

pub(crate) fn settlement_from_run(
    root: &JournalRoot,
    routine: EmbeddedBackupRoutine,
    run: &CliRun,
    reference: &str,
) -> BackupSettlement {
    let disposition = backup_terminal(root, run);
    BackupSettlement {
        reference: reference.to_owned(),
        routine,
        exit_code: exit_code_for(disposition, run.exit_code),
        disposition,
    }
}

fn exit_code_for(disposition: BackupDisposition, exit_code: i32) -> i32 {
    match disposition {
        BackupDisposition::Success => 0,
        BackupDisposition::Refusal | BackupDisposition::Failure => {
            if exit_code == 0 {
                1
            } else {
                exit_code
            }
        }
    }
}

pub(crate) fn outcome_exit(outcome: &BackupRequestOutcome) -> i32 {
    match outcome {
        BackupRequestOutcome::Settled(settlement) => settlement.exit_code,
        BackupRequestOutcome::AdmissionRefused { .. }
        | BackupRequestOutcome::WorkerActive { .. } => 1,
    }
}

pub(crate) fn log_backup_outcome(outcome: &BackupRequestOutcome) {
    match outcome {
        BackupRequestOutcome::Settled(settlement) => {
            log_backup_terminal(settlement.disposition, settlement.exit_code);
        }
        BackupRequestOutcome::AdmissionRefused { .. } => {
            log::warn!("embedded backup request refused by operation admission");
        }
        BackupRequestOutcome::WorkerActive { .. } => {
            log::warn!("embedded backup request refused while a maintenance worker is active");
        }
    }
}

/// Admit maintenance, hold the permit until the embedded run returns, and
/// classify that run. The scheduled adapter and the callosum consumer both
/// use this function.
///
/// `task` is the admitted queue checkpoint for this one request. It is not the
/// shared backup-route cancellation token, and failing it does not cancel that
/// lifetime.
pub(crate) fn settle_embedded_backup(
    root: &JournalRoot,
    routine: EmbeddedBackupRoutine,
    deps: &BackupWebDeps,
    reference: &str,
    task: &Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
) -> BackupRequestOutcome {
    if root.revalidate_canonical_binding().is_err() {
        return BackupRequestOutcome::Settled(BackupSettlement {
            reference: reference.to_owned(),
            routine,
            disposition: BackupDisposition::Refusal,
            exit_code: 1,
        });
    }
    let _restore_use = match solstone_core_journal_io::whole_journal_restore::begin_target_use(
        root.canonical_path(),
    ) {
        Ok(guard) => guard,
        Err(_) => {
            return BackupRequestOutcome::AdmissionRefused {
                reference: reference.to_owned(),
                routine,
            };
        }
    };
    let permit = match deps.lifetime().admit_maintenance() {
        Ok(permit) => permit,
        Err(_) => {
            return BackupRequestOutcome::AdmissionRefused {
                reference: reference.to_owned(),
                routine,
            };
        }
    };
    let runner = CancelledRunner {
        inner: deps.runner.as_ref(),
        cancellation: permit.cancellation(),
        task,
    };
    let http = CancelledHttp {
        inner: deps.http.as_ref(),
        cancellation: permit.cancellation(),
        task,
    };
    let run = solstone_core_backup::with_backup_task_checkpoint(Arc::clone(task), || {
        solstone_core_maintenance::run_embedded_backup(
            root.canonical_path(),
            routine,
            &runner,
            &http,
            &deps.executable,
        )
    });
    // The permit stays alive until `run` has returned.
    drop(permit);
    BackupRequestOutcome::Settled(settlement_from_run(root, routine, &run, reference))
}

struct CancelledRunner<'a> {
    inner: &'a dyn ToolRunner,
    cancellation: &'a ArchiveCancellation,
    task: &'a Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
}

impl ToolRunner for CancelledRunner<'_> {
    fn run(&self, request: &ToolRequest<'_>) -> std::io::Result<ToolOutput> {
        if let Err(cause) = (self.task)() {
            return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, cause));
        }
        let child =
            ArchiveCancellation::new_child(self.cancellation).with_task(Arc::clone(self.task));
        self.inner.run_cancellable(request, &child)
    }
    fn run_cancellable(
        &self,
        request: &ToolRequest<'_>,
        _: &ArchiveCancellation,
    ) -> std::io::Result<ToolOutput> {
        self.run(request)
    }
    fn cancellation(&self) -> Option<&ArchiveCancellation> {
        Some(self.cancellation)
    }

    #[cfg(target_os = "ios")]
    fn ios_journal_root(&self) -> Option<&Arc<JournalRoot>> {
        self.inner.ios_journal_root()
    }
}

struct CancelledHttp<'a> {
    inner: &'a dyn HttpTransport,
    cancellation: &'a ArchiveCancellation,
    task: &'a Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
}

impl HttpTransport for CancelledHttp<'_> {
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        if (self.task)().is_err() {
            return Err(HttpError::Other);
        }
        let child =
            ArchiveCancellation::new_child(self.cancellation).with_task(Arc::clone(self.task));
        self.inner.execute_cancellable(request, &child)
    }
    fn execute_cancellable(
        &self,
        request: &HttpRequest,
        _: &ArchiveCancellation,
    ) -> Result<HttpResponse, HttpError> {
        self.execute(request)
    }
}

pub(super) async fn run(
    root: Arc<JournalRoot>,
    deps: BackupWebDeps,
    stop: watch::Receiver<bool>,
    ready: oneshot::Sender<()>,
) -> Result<(), tokio::task::JoinError> {
    run_with_executor(root, deps, stop, ready, {
        let task: Arc<dyn Fn() -> Result<(), String> + Send + Sync> = Arc::new(|| Ok(()));
        Arc::new(move |root, routine, deps, reference| {
            settle_embedded_backup(root, routine, deps, reference, &task)
        })
    })
    .await
}

type Executor = Arc<
    dyn Fn(&JournalRoot, EmbeddedBackupRoutine, &BackupWebDeps, &str) -> BackupRequestOutcome
        + Send
        + Sync,
>;

pub(crate) fn routine_label(routine: EmbeddedBackupRoutine) -> &'static str {
    match routine {
        EmbeddedBackupRoutine::Run => "backup:run",
        EmbeddedBackupRoutine::Verify => "backup:verify",
        EmbeddedBackupRoutine::Prune => "backup:prune",
    }
}

fn request_reference(envelope: &CallosumEnvelope, marker: u64, sequence: u64) -> String {
    envelope
        .extra
        .get("ref")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("embedded-backup-{marker}-{sequence}"))
}

/// Report a finished request. The bus accept bit is not the disposition.
fn report_terminal(connection: &CallosumSocketConnection, outcome: &BackupRequestOutcome) {
    log_backup_outcome(outcome);
    let _accepted = connection.emit("supervisor", "stopped", terminal_fields(outcome));
}

fn terminal_fields(outcome: &BackupRequestOutcome) -> Map<String, serde_json::Value> {
    let (reference, routine, exit_code) = match outcome {
        BackupRequestOutcome::Settled(settlement) => (
            settlement.reference.as_str(),
            settlement.routine,
            settlement.exit_code,
        ),
        BackupRequestOutcome::AdmissionRefused { reference, routine }
        | BackupRequestOutcome::WorkerActive { reference, routine } => {
            (reference.as_str(), *routine, 1)
        }
    };
    Map::from_iter([
        ("service".into(), json!("backup")),
        ("ref".into(), json!(reference)),
        ("routine".into(), json!(routine_label(routine))),
        ("exit_code".into(), json!(exit_code)),
    ])
}

#[cfg(feature = "processing")]
pub(crate) fn report_scheduled_terminal(
    root: &JournalRoot,
    outcome: &BackupRequestOutcome,
) -> Result<(), solstone_core_callosum::CallosumOneShotError> {
    if root.revalidate_canonical_binding().is_err() {
        log::warn!("embedded backup terminal report refused a changed root");
        return Err(solstone_core_callosum::CallosumOneShotError::Unavailable);
    }
    let mut fields = terminal_fields(outcome);
    fields.insert("tract".into(), json!("supervisor"));
    fields.insert("event".into(), json!("stopped"));
    match serde_json::to_string(&fields) {
        Ok(line) => {
            let result = CallosumOneShotSender::new(
                root.canonical_path().join("health/callosum.sock"),
                std::time::Duration::from_secs(2),
            )
            .send_line_confirmed(&format!("{line}\n"));
            if let Err(error) = &result {
                log::warn!("embedded backup terminal report failed: {error}");
            }
            result
        }
        Err(error) => {
            log::warn!("embedded backup terminal report encoding failed: {error}");
            Err(solstone_core_callosum::CallosumOneShotError::Unavailable)
        }
    }
}

async fn run_with_executor(
    root: Arc<JournalRoot>,
    deps: BackupWebDeps,
    mut stop: watch::Receiver<bool>,
    ready: oneshot::Sender<()>,
    executor: Executor,
) -> Result<(), tokio::task::JoinError> {
    let mut connection = CallosumSocketConnection::new(
        root.canonical_path().join("health/callosum.sock"),
        Map::new(),
    );
    connection.start();
    let marker = NEXT_START.fetch_add(1, Ordering::Relaxed);
    let mut ready = Some(ready);
    let mut worker: Option<JoinHandle<BackupRequestOutcome>> = None;
    let mut sequence = 0_u64;
    loop {
        if *stop.borrow() {
            break;
        }
        tokio::select! {
            _ = stop.changed() => break,
            finished = async {
                match worker.as_mut() {
                    Some(worker) => Some(worker.await),
                    None => std::future::pending().await,
                }
            } => {
                match finished {
                    Some(Ok(outcome)) => report_terminal(&connection, &outcome),
                    Some(Err(error)) => {
                        deps.lifetime().request_stop();
                        connection.stop().await;
                        // Return the actual error and opaque payload to the original
                        // app owner. A completed permit is not successful retirement.
                        return Err(error);
                    }
                    None => {}
                }
                worker = None;
            },
            event = connection.next_event() => {
                let Some(event) = event else { break };
                let envelope = match event {
                    CallosumReceiveEvent::Continuity { phase: CallosumConnectionPhase::Connected, .. } => {
                        // Reflection proves registration in the broadcast set before
                        // the app can accept an owner backup request.
                        connection.emit("supervisor", "status", Map::from_iter([
                            ("service".into(), json!("backup")),
                            ("embedded_backup_start".into(), json!(marker)),
                        ]));
                        continue;
                    },
                    CallosumReceiveEvent::Continuity { .. } => continue,
                    CallosumReceiveEvent::Envelope { envelope, .. } => envelope,
                };
                if envelope.tract == "supervisor" && envelope.event == "status"
                    && envelope.extra.get("embedded_backup_start") == Some(&json!(marker))
                {
                    if let Some(ready) = ready.take() { let _ = ready.send(()); }
                    continue;
                }
                if ready.is_some() { continue; }
                let Some(routine) = routine(&envelope) else { continue };
                sequence = sequence.wrapping_add(1);
                let reference = request_reference(&envelope, marker, sequence);
                if worker.is_some() {
                    report_terminal(
                        &connection,
                        &BackupRequestOutcome::WorkerActive { reference, routine },
                    );
                    continue;
                }
                let root = Arc::clone(&root);
                let deps = deps.clone();
                let executor = Arc::clone(&executor);
                worker = Some(tokio::task::spawn_blocking(move || {
                    executor(&root, routine, &deps, &reference)
                }));
            },
        }
    }
    deps.lifetime().request_stop();
    let result = match worker {
        Some(worker) => match worker.await {
            Ok(outcome) => {
                report_terminal(&connection, &outcome);
                Ok(())
            }
            Err(error) => Err(error),
        },
        None => Ok(()),
    };
    connection.stop().await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    fn request(command: Value) -> CallosumEnvelope {
        serde_json::from_value(json!({"tract":"supervisor","event":"request","cmd":command}))
            .unwrap()
    }
    #[test]
    fn only_the_normal_backup_maintenance_requests_are_admitted() {
        let prefix = ["solstone", "journal", "maintenance", "run"];
        for (name, expected) in [
            ("backup:run", EmbeddedBackupRoutine::Run),
            ("backup:verify", EmbeddedBackupRoutine::Verify),
            ("backup:prune", EmbeddedBackupRoutine::Prune),
        ] {
            let mut command = prefix.to_vec();
            command.push(name);
            assert_eq!(routine(&request(json!(command))), Some(expected));
        }
        for command in [
            json!([
                "solstone",
                "journal",
                "maintenance",
                "run",
                "backup:run",
                "--extra"
            ]),
            json!([
                "solstone",
                "journal",
                "maintenance",
                "run",
                "backup:offload"
            ]),
            json!(["other", "journal", "maintenance", "run", "backup:run"]),
            json!(null),
        ] {
            assert_eq!(routine(&request(command)), None);
        }
        let mut other = request(json!([
            "solstone",
            "journal",
            "maintenance",
            "run",
            "backup:run"
        ]));
        other.event = "status".into();
        assert_eq!(routine(&other), None);
    }

    fn run(exit_code: i32, stdout: &str) -> CliRun {
        CliRun {
            stdout: stdout.to_owned(),
            stderr: String::new(),
            exit_code,
        }
    }

    #[test]
    fn backup_disposition_follows_the_current_run() {
        assert_eq!(
            classify_backup_run(&run(0, ""), true),
            BackupDisposition::Success
        );
        assert_eq!(
            classify_backup_run(&run(0, "backup: skipped\n"), true),
            BackupDisposition::Success
        );
        assert_eq!(
            classify_backup_run(&run(1, "reason=refused"), true),
            BackupDisposition::Refusal
        );
        assert_eq!(
            classify_backup_run(&run(0, "reason=refused"), true),
            BackupDisposition::Refusal
        );
        assert_eq!(
            classify_backup_run(&run(1, "reason=missing"), true),
            BackupDisposition::Failure
        );
        assert_eq!(
            classify_backup_run(&run(0, "backup: skipped"), false),
            BackupDisposition::Refusal
        );
    }

    #[test]
    fn stored_status_does_not_override_the_current_run() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("config")).unwrap();
        std::fs::write(root.path().join("config/journal.json"), "{}").unwrap();
        solstone_core_backup::record_backup_result(
            root.path(),
            "error",
            json!(null),
            json!(null),
            json!("tool"),
            None,
            None,
        )
        .unwrap();
        let journal = JournalRoot::open(root.path()).unwrap();
        let failed = settlement_from_run(
            &journal,
            EmbeddedBackupRoutine::Run,
            &run(1, ""),
            "run-failed",
        );
        assert_eq!(failed.disposition, BackupDisposition::Failure);
        assert_ne!(failed.exit_code, 0);
        let skipped = settlement_from_run(
            &journal,
            EmbeddedBackupRoutine::Run,
            &run(0, "backup: skipped\n"),
            "run-skipped",
        );
        assert_eq!(skipped.disposition, BackupDisposition::Success);
        assert_eq!(skipped.exit_code, 0);
        solstone_core_backup::record_backup_result(
            root.path(),
            "ok",
            json!(null),
            json!(null),
            json!(null),
            None,
            None,
        )
        .unwrap();
        for status_run in [
            run(1, ""),
            run(1, "status=ok"),
            run(1, "status=skipped"),
            run(1, "status=cleared_superseded"),
        ] {
            let settlement = settlement_from_run(
                &journal,
                EmbeddedBackupRoutine::Run,
                &status_run,
                "run-not-success",
            );
            assert_eq!(settlement.disposition, BackupDisposition::Failure);
            assert_ne!(settlement.exit_code, 0);
        }
        solstone_core_backup::record_prune_result(root.path(), "ok", json!(null), json!(null))
            .unwrap();
        let refusal = CliRun {
            stdout: "backup prune: error reason=refused detail=prune is not available\n".to_owned(),
            stderr: String::new(),
            exit_code: 1,
        };
        let settled = settlement_from_run(
            &journal,
            EmbeddedBackupRoutine::Prune,
            &refusal,
            "prune-refused",
        );
        assert_eq!(settled.routine, EmbeddedBackupRoutine::Prune);
        assert_eq!(settled.reference, "prune-refused");
        assert_eq!(settled.disposition, BackupDisposition::Refusal);
        assert_ne!(settled.exit_code, 0);
        assert_eq!(
            outcome_exit(&BackupRequestOutcome::AdmissionRefused {
                reference: "busy".into(),
                routine: EmbeddedBackupRoutine::Run,
            }),
            1
        );
        assert_eq!(
            outcome_exit(&BackupRequestOutcome::WorkerActive {
                reference: "active".into(),
                routine: EmbeddedBackupRoutine::Verify,
            }),
            1
        );
    }

    #[cfg(feature = "full-tests")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reflected_consumer_retains_real_maintenance_until_completion_and_returns_panic() {
        use std::{
            sync::{Mutex, atomic::AtomicBool},
            time::{Duration, Instant},
        };
        for (panic, stop_before_release) in [(false, true), (true, true), (true, false)] {
            struct Payload(Arc<AtomicBool>);
            impl Drop for Payload {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::Release);
                    panic!("opaque worker panic payload must remain owned");
                }
            }
            let directory = tempfile::tempdir().unwrap();
            let root = Arc::new(JournalRoot::open(directory.path()).unwrap());
            let socket = root.canonical_path().join("health/callosum.sock");
            let bus = solstone_core_callosum::CallosumSocketServer::bind(socket.clone())
                .await
                .unwrap();
            let mut observer = CallosumSocketConnection::new(socket, Map::new());
            observer.start();
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    match observer.next_event().await {
                        Some(CallosumReceiveEvent::Continuity {
                            phase: CallosumConnectionPhase::Connected,
                            ..
                        }) => {
                            observer.emit(
                                "supervisor",
                                "status",
                                Map::from_iter([("backup_test_observer".into(), json!(true))]),
                            );
                        }
                        Some(CallosumReceiveEvent::Envelope { envelope, .. })
                            if envelope.extra.get("backup_test_observer") == Some(&json!(true)) =>
                        {
                            break;
                        }
                        _ => {}
                    }
                }
            })
            .await
            .unwrap();
            let deps = BackupWebDeps::new(root.canonical_path().to_path_buf());
            let lifetime = deps.lifetime();
            let (entered, entry) = oneshot::channel();
            let entered = Mutex::new(Some(entered));
            let (release, held) = std::sync::mpsc::channel();
            let held = Mutex::new(held);
            let dropped = Arc::new(AtomicBool::new(false));
            let payload_dropped = dropped.clone();
            let (stop, stopping) = watch::channel(false);
            let (ready, registered) = oneshot::channel();
            let mut consumer = tokio::spawn(run_with_executor(
                root.clone(),
                deps,
                stopping,
                ready,
                Arc::new(move |root, routine, deps, reference| {
                    assert_eq!(routine, EmbeddedBackupRoutine::Run);
                    let permit = deps.lifetime().admit_maintenance().unwrap();
                    root.revalidate_canonical_binding().unwrap();
                    entered.lock().unwrap().take().unwrap().send(()).unwrap();
                    held.lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(10))
                        .unwrap();
                    assert_eq!(permit.cancellation().is_cancelled(), stop_before_release);
                    if panic {
                        std::panic::panic_any(Payload(payload_dropped.clone()));
                    }
                    BackupRequestOutcome::Settled(BackupSettlement {
                        reference: reference.to_owned(),
                        routine,
                        disposition: BackupDisposition::Success,
                        exit_code: 0,
                    })
                }),
            ));
            tokio::time::timeout(Duration::from_secs(5), registered)
                .await
                .unwrap()
                .unwrap();
            observer.emit(
                "supervisor",
                "request",
                Map::from_iter([(
                    "cmd".into(),
                    json!(["solstone", "journal", "maintenance", "run", "backup:run"]),
                )]),
            );
            tokio::time::timeout(Duration::from_secs(5), entry)
                .await
                .unwrap()
                .unwrap();
            if stop_before_release {
                stop.send(true).unwrap();
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(25), &mut consumer)
                    .await
                    .is_err()
            );
            assert_eq!(
                lifetime.drain(
                    &tokio::runtime::Handle::current(),
                    Instant::now() + Duration::from_millis(25)
                ),
                solstone_core_backup_web::BackupRouteDrain::Pending
            );
            if stop_before_release {
                assert!(lifetime.admit_maintenance().is_err());
            }
            release.send(()).unwrap();
            let result = tokio::time::timeout(Duration::from_secs(5), consumer)
                .await
                .unwrap()
                .unwrap();
            if panic {
                let failure = result.unwrap_err();
                assert!(failure.is_panic());
                let payload = failure.into_panic();
                assert!(Arc::ptr_eq(
                    &payload
                        .downcast_ref::<Payload>()
                        .expect("exact injected opaque payload")
                        .0,
                    &dropped
                ));
                assert!(!dropped.load(Ordering::Acquire));
                // Model the sticky original-owner failure custody; never discard
                // this opaque payload while reporting a shutdown result.
                std::mem::forget(payload);
            } else {
                result.unwrap();
            }
            assert!(lifetime.admit_maintenance().is_err());
            assert_eq!(
                lifetime.drain(
                    &tokio::runtime::Handle::current(),
                    Instant::now() + Duration::from_secs(1)
                ),
                solstone_core_backup_web::BackupRouteDrain::Drained
            );
            observer.stop().await;
            bus.stop().await;
        }
    }

    #[cfg(feature = "full-tests")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn backup_busy_worker_does_not_start_a_second_tool() {
        use std::sync::{Mutex, atomic::AtomicU64};
        use std::time::Duration;

        let directory = tempfile::tempdir().unwrap();
        let root = Arc::new(JournalRoot::open(directory.path()).unwrap());
        let socket = root.canonical_path().join("health/callosum.sock");
        let bus = solstone_core_callosum::CallosumSocketServer::bind(socket.clone())
            .await
            .unwrap();
        let mut observer = CallosumSocketConnection::new(socket, Map::new());
        observer.start();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match observer.next_event().await {
                    Some(CallosumReceiveEvent::Continuity {
                        phase: CallosumConnectionPhase::Connected,
                        ..
                    }) => {
                        observer.emit(
                            "supervisor",
                            "status",
                            Map::from_iter([("backup_busy_observer".into(), json!(true))]),
                        );
                    }
                    Some(CallosumReceiveEvent::Envelope { envelope, .. })
                        if envelope.extra.get("backup_busy_observer") == Some(&json!(true)) =>
                    {
                        break;
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        let deps = BackupWebDeps::new(root.canonical_path().to_path_buf());
        let calls = Arc::new(AtomicU64::new(0));
        let calls_executor = Arc::clone(&calls);
        let (entered, entry) = oneshot::channel();
        let entered = Mutex::new(Some(entered));
        let (release, held) = std::sync::mpsc::channel();
        let held = Mutex::new(held);
        let (stop, stopping) = watch::channel(false);
        let (ready, registered) = oneshot::channel();
        let consumer = tokio::spawn(run_with_executor(
            root.clone(),
            deps,
            stopping,
            ready,
            Arc::new(move |_root, routine, _deps, reference| {
                assert_eq!(routine, EmbeddedBackupRoutine::Run);
                calls_executor.fetch_add(1, Ordering::SeqCst);
                entered.lock().unwrap().take().unwrap().send(()).unwrap();
                held.lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap();
                BackupRequestOutcome::Settled(BackupSettlement {
                    reference: reference.to_owned(),
                    routine,
                    disposition: BackupDisposition::Success,
                    exit_code: 0,
                })
            }),
        ));
        tokio::time::timeout(Duration::from_secs(5), registered)
            .await
            .unwrap()
            .unwrap();
        observer.emit(
            "supervisor",
            "request",
            Map::from_iter([(
                "cmd".into(),
                json!(["solstone", "journal", "maintenance", "run", "backup:run"]),
            )]),
        );
        tokio::time::timeout(Duration::from_secs(5), entry)
            .await
            .unwrap()
            .unwrap();
        observer.emit(
            "supervisor",
            "request",
            Map::from_iter([(
                "cmd".into(),
                json!(["solstone", "journal", "maintenance", "run", "backup:run"]),
            )]),
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match observer.next_event().await {
                    Some(CallosumReceiveEvent::Envelope { envelope, .. })
                        if envelope.tract == "supervisor"
                            && envelope.event == "stopped"
                            && envelope.extra.get("service") == Some(&json!("backup"))
                            && envelope.extra.get("exit_code") == Some(&json!(1)) =>
                    {
                        break;
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        release.send(()).unwrap();
        stop.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(5), consumer)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        observer.stop().await;
        bus.stop().await;
    }

    struct CountingRunner {
        calls: Arc<AtomicU64>,
    }

    impl ToolRunner for CountingRunner {
        fn run(&self, _: &ToolRequest<'_>) -> std::io::Result<ToolOutput> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput {
                returncode: 0,
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
        }

        fn run_cancellable(
            &self,
            _: &ToolRequest<'_>,
            _: &ArchiveCancellation,
        ) -> std::io::Result<ToolOutput> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput {
                returncode: 0,
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
        }
    }

    struct CountingHttp {
        calls: Arc<AtomicU64>,
    }

    impl HttpTransport for CountingHttp {
        fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: Vec::new(),
            })
        }

        fn execute_cancellable(
            &self,
            _: &HttpRequest,
            _: &ArchiveCancellation,
        ) -> Result<HttpResponse, HttpError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: Vec::new(),
            })
        }
    }

    fn empty_tool_request() -> ToolRequest<'static> {
        ToolRequest {
            program: std::ffi::OsString::from("restic"),
            argv: Vec::new(),
            env: std::collections::BTreeMap::new(),
            timeout: None,
            pass_fds: Vec::new(),
            stdin: None,
            #[cfg(windows)]
            resources: Default::default(),
        }
    }

    fn empty_http_request() -> HttpRequest {
        HttpRequest {
            method: "GET".into(),
            url: "https://backup.example/settle".into(),
            headers: Vec::new(),
            body: Vec::new(),
            timeout: std::time::Duration::from_secs(1),
        }
    }

    fn failing_task() -> Arc<dyn Fn() -> Result<(), String> + Send + Sync> {
        Arc::new(|| Err("stopped".to_owned()))
    }

    #[test]
    fn backup_admission_refusal_does_not_call_the_tool() {
        let directory = tempfile::tempdir().unwrap();
        let root = JournalRoot::open(directory.path()).unwrap();
        let mut deps = BackupWebDeps::new(root.canonical_path().to_path_buf());
        let calls = Arc::new(AtomicU64::new(0));
        deps.runner = Arc::new(CountingRunner {
            calls: Arc::clone(&calls),
        });
        // A closed lifetime is the admission refusal. A second write permit is
        // not one: IdleOnly refuses a live archive or operation, not another write.
        deps.lifetime().request_stop();
        let task = failing_task();
        let outcome = settle_embedded_backup(
            &root,
            EmbeddedBackupRoutine::Run,
            &deps,
            "admission-refused",
            &task,
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            outcome,
            BackupRequestOutcome::AdmissionRefused {
                reference: "admission-refused".into(),
                routine: EmbeddedBackupRoutine::Run,
            }
        );
    }

    #[test]
    fn backup_task_checkpoint_blocks_the_runner() {
        let runner = CountingRunner {
            calls: Arc::new(AtomicU64::new(0)),
        };
        let cancellation = ArchiveCancellation::new();
        let task = failing_task();
        let cancelled = super::CancelledRunner {
            inner: &runner,
            cancellation: &cancellation,
            task: &task,
        };
        let error = cancelled.run(&empty_tool_request()).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert_eq!(runner.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn backup_http_checkpoint_blocks_the_transport() {
        let http = CountingHttp {
            calls: Arc::new(AtomicU64::new(0)),
        };
        let cancellation = ArchiveCancellation::new();
        let task = failing_task();
        let cancelled = super::CancelledHttp {
            inner: &http,
            cancellation: &cancellation,
            task: &task,
        };
        let error = cancelled.execute(&empty_http_request()).unwrap_err();
        assert_eq!(error, HttpError::Other);
        assert_eq!(http.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn backup_task_checkpoint_blocks_the_writer() {
        let directory = tempfile::tempdir().unwrap();
        let destination = solstone_core_backup::Destination {
            repository: "s3:repo".into(),
            backend: "s3".into(),
            credentials: json!({"access_key_id": "access", "secret_access_key": "secret"})
                .as_object()
                .unwrap()
                .clone(),
        };
        solstone_core_backup::set_destination(directory.path(), &destination).unwrap();
        solstone_core_backup::generate_and_store_keys(directory.path()).unwrap();
        solstone_core_backup::set_enabled(directory.path(), true).unwrap();
        let config_path = directory.path().join("config/journal.json");
        let before = std::fs::read(&config_path).unwrap();
        let root = JournalRoot::open(directory.path()).unwrap();
        let mut deps = BackupWebDeps::new(root.canonical_path().to_path_buf());
        let calls = Arc::new(AtomicU64::new(0));
        deps.runner = Arc::new(CountingRunner {
            calls: Arc::clone(&calls),
        });
        let task = failing_task();
        let outcome = settle_embedded_backup(
            &root,
            EmbeddedBackupRoutine::Run,
            &deps,
            "writer-fence",
            &task,
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(std::fs::read(&config_path).unwrap(), before);
        match outcome {
            BackupRequestOutcome::Settled(settlement) => {
                assert_ne!(settlement.disposition, BackupDisposition::Success);
                assert_ne!(settlement.exit_code, 0);
            }
            other => panic!("expected a settled refusal, got {other:?}"),
        }
    }

    fn seed_backup(journal: &std::path::Path) {
        let destination = solstone_core_backup::Destination {
            repository: "s3:repo".into(),
            backend: "s3".into(),
            credentials: json!({"access_key_id": "access", "secret_access_key": "secret"})
                .as_object()
                .unwrap()
                .clone(),
        };
        solstone_core_backup::set_destination(journal, &destination).unwrap();
        solstone_core_backup::generate_and_store_keys(journal).unwrap();
        solstone_core_backup::set_enabled(journal, true).unwrap();
    }

    #[test]
    fn backup_owner_records_last_backup_under_a_healthy_task() {
        let directory = tempfile::tempdir().unwrap();
        seed_backup(directory.path());
        let task: Arc<dyn Fn() -> Result<(), String> + Send + Sync> = Arc::new(|| Ok(()));
        solstone_core_backup::with_backup_task_checkpoint(task, || {
            solstone_core_backup::record_backup_result(
                directory.path(),
                "ok",
                json!(1_700_000_000),
                json!("snap-positive"),
                Value::Null,
                None,
                None,
            )
            .unwrap();
        });
        let saved: Value = serde_json::from_slice(
            &std::fs::read(directory.path().join("config/journal.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(saved["backup"]["last_backup"]["status"], "ok");
        assert_eq!(
            saved["backup"]["last_backup"]["snapshot_id"],
            "snap-positive"
        );
    }

    #[cfg(feature = "full-tests")]
    struct PollingRunner {
        calls: Arc<AtomicU64>,
        entered: Arc<std::sync::atomic::AtomicBool>,
    }

    #[cfg(feature = "full-tests")]
    impl ToolRunner for PollingRunner {
        fn run(&self, request: &ToolRequest<'_>) -> std::io::Result<ToolOutput> {
            self.run_cancellable(request, &ArchiveCancellation::new())
        }

        fn run_cancellable(
            &self,
            _: &ToolRequest<'_>,
            cancellation: &ArchiveCancellation,
        ) -> std::io::Result<ToolOutput> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered
                .store(true, std::sync::atomic::Ordering::Release);
            while !cancellation.is_cancelled() {
                std::thread::yield_now();
            }
            Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "stopped",
            ))
        }
    }

    #[cfg(feature = "full-tests")]
    struct PollingHttp {
        calls: Arc<AtomicU64>,
        entered: Arc<std::sync::atomic::AtomicBool>,
    }

    #[cfg(feature = "full-tests")]
    impl HttpTransport for PollingHttp {
        fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
            self.execute_cancellable(request, &ArchiveCancellation::new())
        }

        fn execute_cancellable(
            &self,
            _: &HttpRequest,
            cancellation: &ArchiveCancellation,
        ) -> Result<HttpResponse, HttpError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered
                .store(true, std::sync::atomic::Ordering::Release);
            while !cancellation.is_cancelled() {
                std::thread::yield_now();
            }
            Err(HttpError::Other)
        }
    }

    #[cfg(feature = "full-tests")]
    fn wait_until(mut ready: impl FnMut() -> bool) {
        for _ in 0..1_000_000 {
            if ready() {
                return;
            }
            std::thread::yield_now();
        }
        assert!(ready());
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn backup_runner_stops_when_the_task_fails_during_the_poll() {
        use std::sync::atomic::AtomicBool;

        let parent = ArchiveCancellation::new();
        let sibling = ArchiveCancellation::new_child(&parent);
        let entered = Arc::new(AtomicBool::new(false));
        let fail = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicU64::new(0));
        let runner = PollingRunner {
            calls: Arc::clone(&calls),
            entered: Arc::clone(&entered),
        };
        let fail_flag = Arc::clone(&fail);
        let task: Arc<dyn Fn() -> Result<(), String> + Send + Sync> = Arc::new(move || {
            if fail_flag.load(Ordering::Acquire) {
                Err("stopped".to_owned())
            } else {
                Ok(())
            }
        });
        let writer_task = Arc::clone(&task);
        let parent_for_worker = parent.clone();
        let task_for_worker = Arc::clone(&task);
        let worker = std::thread::spawn(move || {
            let cancelled = super::CancelledRunner {
                inner: &runner,
                cancellation: &parent_for_worker,
                task: &task_for_worker,
            };
            cancelled.run(&empty_tool_request())
        });
        wait_until(|| entered.load(Ordering::Acquire));
        assert!(!parent.is_cancelled());
        assert!(!sibling.is_cancelled());
        fail.store(true, Ordering::Release);
        let error = worker.join().unwrap().unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!parent.is_cancelled());
        assert!(!sibling.is_cancelled());

        let directory = tempfile::tempdir().unwrap();
        seed_backup(directory.path());
        let config_path = directory.path().join("config/journal.json");
        let before = std::fs::read(&config_path).unwrap();
        let write = solstone_core_backup::with_backup_task_checkpoint(writer_task, || {
            solstone_core_backup::record_backup_result(
                directory.path(),
                "ok",
                json!(1),
                json!("should-not-land"),
                Value::Null,
                None,
                None,
            )
        });
        assert!(write.is_err());
        assert_eq!(std::fs::read(&config_path).unwrap(), before);
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn backup_http_stops_when_the_task_fails_during_the_poll() {
        use std::sync::atomic::AtomicBool;

        let parent = ArchiveCancellation::new();
        let sibling = ArchiveCancellation::new_child(&parent);
        let entered = Arc::new(AtomicBool::new(false));
        let fail = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicU64::new(0));
        let http = PollingHttp {
            calls: Arc::clone(&calls),
            entered: Arc::clone(&entered),
        };
        let fail_flag = Arc::clone(&fail);
        let task: Arc<dyn Fn() -> Result<(), String> + Send + Sync> = Arc::new(move || {
            if fail_flag.load(Ordering::Acquire) {
                Err("stopped".to_owned())
            } else {
                Ok(())
            }
        });
        let parent_for_worker = parent.clone();
        let task_for_worker = Arc::clone(&task);
        let worker = std::thread::spawn(move || {
            let cancelled = super::CancelledHttp {
                inner: &http,
                cancellation: &parent_for_worker,
                task: &task_for_worker,
            };
            cancelled.execute(&empty_http_request())
        });
        wait_until(|| entered.load(Ordering::Acquire));
        assert!(!parent.is_cancelled());
        assert!(!sibling.is_cancelled());
        fail.store(true, Ordering::Release);
        let error = worker.join().unwrap().unwrap_err();
        assert_eq!(error, HttpError::Other);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!parent.is_cancelled());
        assert!(!sibling.is_cancelled());
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn backup_child_stops_when_the_root_is_replaced_during_the_poll() {
        use std::sync::atomic::AtomicBool;

        let parent_dir = tempfile::tempdir().unwrap();
        let journal = parent_dir.path().join("journal");
        std::fs::create_dir(&journal).unwrap();
        seed_backup(&journal);
        let config_path = journal.join("config/journal.json");
        let before = std::fs::read(&config_path).unwrap();
        let bound = JournalRoot::open(&journal).unwrap();
        let parent = ArchiveCancellation::new();
        let sibling = ArchiveCancellation::new_child(&parent);
        let entered = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicU64::new(0));
        let runner = PollingRunner {
            calls: Arc::clone(&calls),
            entered: Arc::clone(&entered),
        };
        let task: Arc<dyn Fn() -> Result<(), String> + Send + Sync> = Arc::new(move || {
            bound
                .revalidate_canonical_binding()
                .map_err(|error| error.to_string())
        });
        let writer_task = Arc::clone(&task);
        let parent_for_worker = parent.clone();
        let task_for_worker = Arc::clone(&task);
        let worker = std::thread::spawn(move || {
            let cancelled = super::CancelledRunner {
                inner: &runner,
                cancellation: &parent_for_worker,
                task: &task_for_worker,
            };
            cancelled.run(&empty_tool_request())
        });
        wait_until(|| entered.load(Ordering::Acquire));
        assert!(!parent.is_cancelled());
        assert!(!sibling.is_cancelled());
        let parked = parent_dir.path().join("parked");
        std::fs::rename(&journal, &parked).unwrap();
        std::fs::create_dir(&journal).unwrap();
        let error = worker.join().unwrap().unwrap_err();
        assert_eq!(
            std::fs::read(parked.join("config/journal.json")).unwrap(),
            before
        );
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert!(!parent.is_cancelled());
        assert!(!sibling.is_cancelled());
        let write = solstone_core_backup::with_backup_task_checkpoint(writer_task, || {
            solstone_core_backup::record_backup_result(
                &journal,
                "ok",
                json!(1),
                json!("should-not-land"),
                Value::Null,
                None,
                None,
            )
        });
        assert!(write.is_err());
        assert!(!journal.join("config/journal.json").exists());
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn backup_child_stops_when_the_task_deadline_expires_during_the_poll() {
        use std::sync::atomic::AtomicBool;
        use std::time::{Duration, Instant};

        let parent = ArchiveCancellation::new();
        let sibling = ArchiveCancellation::new_child(&parent);
        let entered = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicU64::new(0));
        let runner = PollingRunner {
            calls: Arc::clone(&calls),
            entered: Arc::clone(&entered),
        };
        let control = solstone_core_system::queue::CallableTaskContext {
            cancelled: Arc::new(AtomicBool::new(false)),
            priority: Default::default(),
            deadline: Instant::now() + Duration::from_millis(80),
        };
        let task: Arc<dyn Fn() -> Result<(), String> + Send + Sync> = Arc::new(move || {
            if control.is_cancelled() {
                Err("deadline".to_owned())
            } else {
                Ok(())
            }
        });
        let writer_task = Arc::clone(&task);
        let parent_for_worker = parent.clone();
        let task_for_worker = Arc::clone(&task);
        let worker = std::thread::spawn(move || {
            let cancelled = super::CancelledRunner {
                inner: &runner,
                cancellation: &parent_for_worker,
                task: &task_for_worker,
            };
            cancelled.run(&empty_tool_request())
        });
        wait_until(|| entered.load(Ordering::Acquire));
        assert!(!parent.is_cancelled());
        assert!(!sibling.is_cancelled());
        let error = worker.join().unwrap().unwrap_err();
        assert!(!parent.is_cancelled());
        assert!(!sibling.is_cancelled());
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let directory = tempfile::tempdir().unwrap();
        seed_backup(directory.path());
        let config_path = directory.path().join("config/journal.json");
        let before = std::fs::read(&config_path).unwrap();
        let write = solstone_core_backup::with_backup_task_checkpoint(writer_task, || {
            solstone_core_backup::record_backup_result(
                directory.path(),
                "ok",
                json!(1),
                json!("should-not-land"),
                Value::Null,
                None,
                None,
            )
        });
        assert!(write.is_err());
        assert_eq!(std::fs::read(&config_path).unwrap(), before);
    }
}
