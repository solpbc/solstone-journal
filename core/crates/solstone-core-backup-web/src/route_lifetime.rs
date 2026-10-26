// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::time::{Duration, Instant};

use axum::response::Response;
use solstone_core_backup::archive_admission::ArchiveCancellation;
use solstone_core_backup_runtime::runner::{ToolOutput, ToolRequest, ToolRunner};

use crate::operation::{self, AdmitWrite, SharedOperationSlot, WriteGuard};
use crate::restore_prepare::SharedRestorePrepare;

#[cfg(all(test, feature = "full-tests"))]
#[path = "route_lifetime_tests.rs"]
mod route_lifetime_tests;

/// Coordination handle for backup route lifetime and teardown drain.
///
/// Clones share one record. A new `BackupWebDeps` is a new lifetime.
/// This lifetime does not drain supervisor maintenance (`backup:run` from `backup_now`,
/// `backup:verify` from `offload_enable`).
#[derive(Clone)]
pub struct BackupRouteLifetime {
    pub(crate) operations: SharedOperationSlot,
    pub(crate) restore_prepare: SharedRestorePrepare,
}

/// The outcome of draining a backup route lifetime toward quiescence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackupRouteDrain {
    /// This lifetime is closed and every owned thread and admitted write has finished.
    Drained,
    /// The deadline passed, work is still in flight, or the lifetime is not closed.
    /// `Pending` keeps ownership and is retryable.
    Pending,
    /// A joined thread panicked or join failure was recorded. `Failure` stays `Failure` on retry.
    Failure,
}

impl BackupRouteLifetime {
    /// Signal stop across all operations in this lifetime without waiting.
    ///
    /// `request_stop` does not wait and does not touch the process-global archive flag.
    pub fn request_stop(&self) {
        self.operations.request_stop();
        if let Ok(mut prepare_guard) = self.restore_prepare.try_lock() {
            *prepare_guard = None;
        }
    }

    /// Wait for all in-flight work and worker threads to finish before `deadline`.
    ///
    /// Clones share one record; a new `BackupWebDeps` is a new lifetime; `drain` does not stop;
    /// not-closed is `Pending`; `Pending` keeps ownership and is retryable; `Failure` stays
    /// `Failure` on retry; this lifetime does not drain supervisor maintenance (`backup:run` from
    /// `backup_now`, `backup:verify` from `offload_enable`).
    /// `executor` belongs to the embedding app. Its runtime must stay alive
    /// through drain; runtime-pool shutdown is outside this router's lifetime.
    /// Native joins run in retained blocking tasks, including thread-local
    /// destructors, so they cannot extend this caller's deadline.
    pub fn drain(&self, executor: &tokio::runtime::Handle, deadline: Instant) -> BackupRouteDrain {
        let mut attempted_settlement = false;
        loop {
            let mut guard = match self.operations.try_lock() {
                Ok(guard) => guard,
                Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                    self.operations.finish_join(0, true);
                    poisoned.into_inner()
                }
                Err(std::sync::TryLockError::WouldBlock) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return BackupRouteDrain::Pending;
                    }
                    std::thread::park_timeout(Duration::from_millis(10).min(remaining));
                    continue;
                }
            };

            // Polling task completion never joins a native thread or waits for
            // the caller-owned executor pool's thread-local destructors.
            let mut i = 0;
            while i < guard.join_batches.len() {
                use std::future::Future;
                let ready = guard.join_batches[i].task.as_mut().and_then(|task| {
                    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
                    match std::pin::Pin::new(task).poll(&mut context) {
                        std::task::Poll::Ready(result) => Some(result),
                        std::task::Poll::Pending => None,
                    }
                });
                if let Some(result) = ready {
                    let mut batch = guard.join_batches.remove(i);
                    let empty = batch
                        .owners
                        .try_lock()
                        .is_ok_and(|owners| owners.is_empty());
                    let no_panics = batch
                        .panics
                        .try_lock()
                        .is_ok_and(|panics| panics.is_empty());
                    if result.is_err() || !empty || !no_panics {
                        batch.failure = result.err();
                        self.operations.finish_join(0, true);
                        guard.failed_joins.push(batch);
                    }
                } else {
                    i += 1;
                }
            }

            let mut finished = Vec::new();
            let mut i = 0;
            while i < guard.threads.len() {
                if guard.threads[i].is_finished() {
                    finished.push(guard.threads.remove(i));
                } else {
                    i += 1;
                }
            }
            finished.extend(guard.archive.take_finished_workers());
            if guard
                .settlement_worker
                .as_ref()
                .is_some_and(|worker| worker.is_finished())
            {
                finished.push(guard.settlement_worker.take().expect("finished settlement"));
            }
            let took_count = finished.len();

            if took_count > 0 {
                self.operations.begin_join(took_count);
                let owners = std::sync::Arc::new(std::sync::Mutex::new(finished));
                let panics = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
                let work_owners = owners.clone();
                let work_panics = panics.clone();
                let operations = self.operations.clone();
                let task = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    executor.spawn_blocking(move || {
                        let mut had_err = false;
                        loop {
                            let handle = work_owners
                                .lock()
                                .unwrap_or_else(|poison| poison.into_inner())
                                .pop();
                            let Some(handle) = handle else {
                                break;
                            };
                            if let Err(payload) = handle.join() {
                                had_err = true;
                                work_panics
                                    .lock()
                                    .unwrap_or_else(|poison| poison.into_inner())
                                    .push(payload);
                            }
                        }
                        operations.finish_join(took_count, had_err);
                    })
                }));
                let mut batch = operation::JoinBatch {
                    task: None,
                    owners,
                    panics,
                    failure: None,
                };
                match task {
                    Ok(task) => {
                        batch.task = Some(task);
                        guard.join_batches.push(batch);
                    }
                    Err(payload) => {
                        batch
                            .panics
                            .lock()
                            .unwrap_or_else(|poison| poison.into_inner())
                            .push(payload);
                        self.operations.finish_join(0, true);
                        guard.failed_joins.push(batch);
                    }
                }
                drop(guard);
                continue;
            }

            #[cfg(windows)]
            let cleanup_some = guard
                .current
                .as_ref()
                .is_some_and(|current| current.cleanup.is_some());
            #[cfg(not(windows))]
            let cleanup_some = false;

            let worker_active = guard
                .current
                .as_ref()
                .is_some_and(|current| current.worker_active);

            let prepared_pending = if guard.closed {
                match self.restore_prepare.try_lock() {
                    Ok(mut prepare) => {
                        *prepare = None;
                        false
                    }
                    Err(_) => true,
                }
            } else {
                false
            };
            if guard.closed
                && !attempted_settlement
                && guard.settlement_worker.is_none()
                && guard.threads.is_empty()
                && !self.operations.joining()
                && guard.join_batches.is_empty()
                && !guard.archive.workers_running()
                && !worker_active
                && guard.writes_in_flight == 0
                && (guard.archive.needs_settlement() || cleanup_some || prepared_pending)
            {
                let operations = std::sync::Arc::clone(&self.operations);
                let prepare = std::sync::Arc::clone(&self.restore_prepare);
                attempted_settlement = true;
                // Registration occurs while holding the mutex. On spawn failure
                // all original custody remains in the shared record.
                match std::thread::Builder::new()
                    .name("backup-route-settle".into())
                    .spawn(move || {
                        crate::archive_worker::settle_closed(&operations);
                        match prepare.lock() {
                            Ok(mut prepare) => *prepare = None,
                            Err(_) => operations.finish_join(0, true),
                        }
                    }) {
                    Ok(worker) => guard.settlement_worker = Some(worker),
                    Err(_) => self.operations.finish_join(0, true),
                }
            }

            let blocked = !guard.threads.is_empty()
                || guard.archive.is_busy()
                || guard.settlement_worker.is_some()
                || self.operations.joining()
                || !guard.join_batches.is_empty()
                || guard.writes_in_flight > 0
                || worker_active
                || cleanup_some
                || prepared_pending;

            let closed = guard.closed;
            let join_failure = self.operations.join_failed();

            if join_failure {
                return BackupRouteDrain::Failure;
            }

            if blocked {
                let now = Instant::now();
                if now >= deadline {
                    return BackupRouteDrain::Pending;
                }
                let remaining = deadline - now;
                let slice = Duration::from_millis(10).min(remaining);
                drop(guard);
                self.operations.wait_timeout(slice);
                continue;
            }

            drop(guard);

            if !closed {
                return BackupRouteDrain::Pending;
            }

            if join_failure {
                return BackupRouteDrain::Failure;
            }

            return BackupRouteDrain::Drained;
        }
    }

    pub fn admit_maintenance(&self) -> Result<MaintenancePermit, Response> {
        let write = operation::admit_write(&self.operations, AdmitWrite::Maintenance)?;
        let cancellation = match self.operations.lock() {
            Ok(guard) => guard.cancellation.clone(),
            Err(poisoned) => poisoned.into_inner().cancellation.clone(),
        };
        Ok(MaintenancePermit {
            _write: write,
            cancellation,
        })
    }
}

pub struct MaintenancePermit {
    _write: WriteGuard,
    cancellation: ArchiveCancellation,
}

impl MaintenancePermit {
    pub fn cancellation(&self) -> &ArchiveCancellation {
        &self.cancellation
    }
}

pub(crate) struct LifetimeRunner<'a> {
    inner: &'a dyn ToolRunner,
    cancellation: ArchiveCancellation,
}

impl<'a> LifetimeRunner<'a> {
    pub(crate) fn new(inner: &'a dyn ToolRunner, cancellation: ArchiveCancellation) -> Self {
        Self {
            inner,
            cancellation,
        }
    }
}

impl<'a> ToolRunner for LifetimeRunner<'a> {
    fn run(&self, request: &ToolRequest<'_>) -> std::io::Result<ToolOutput> {
        self.inner.run_cancellable(request, &self.cancellation)
    }

    fn run_cancellable(
        &self,
        request: &ToolRequest<'_>,
        cancellation: &ArchiveCancellation,
    ) -> std::io::Result<ToolOutput> {
        self.inner.run_cancellable(request, cancellation)
    }

    fn cancellation(&self) -> Option<&ArchiveCancellation> {
        Some(&self.cancellation)
    }

    #[cfg(target_os = "ios")]
    fn ios_journal_root(&self) -> Option<&std::sync::Arc<solstone_core_journal_io::JournalRoot>> {
        self.inner.ios_journal_root()
    }
}
