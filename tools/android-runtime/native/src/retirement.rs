// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::stop_diagnostics::Phase;
use std::any::Any;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, TryLockError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// The Android process owns this executor independently of every serving Runtime.
/// Keeping the actual executor alive is necessary while a native join is pending.
static EXECUTOR: OnceLock<Result<Arc<Executor>, ()>> = OnceLock::new();

#[cfg(all(test, feature = "full-tests"))]
thread_local! {
    static SCHEDULING_PANIC: std::cell::RefCell<Option<Box<dyn Any + Send>>> = const { std::cell::RefCell::new(None) };
}

struct Executor {
    runtime: tokio::runtime::Runtime,
    current: Mutex<Option<Arc<Record>>>,
}

impl Executor {
    fn new() -> Result<Arc<Self>, ()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .build()
            .map_err(|_| ())?;
        Ok(Arc::new(Self {
            runtime,
            current: Mutex::new(None),
        }))
    }
}

enum Task {
    Unscheduled,
    Running(tokio::task::JoinHandle<()>),
    Finished(Result<(), tokio::task::JoinError>),
}

struct Record {
    // These are original owners, retained even if a local observer/slot is lost.
    _owner: Arc<dyn Any + Send + Sync>,
    _root: Arc<dyn Any + Send + Sync>,
    worker: Mutex<Option<JoinHandle<()>>>,
    task: Mutex<Task>,
    panics: Mutex<Vec<Box<dyn Any + Send>>>,
    failed: AtomicBool,
    owner_retired: AtomicBool,
    joined: AtomicBool,
}

pub(super) struct Retirement {
    executor: Arc<Executor>,
    record: Arc<Record>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Pending,
    Stopped,
    Failed,
}

/// Production start consults process custody, independently of the SERVING slot.
pub(super) fn admission() -> Outcome {
    match EXECUTOR.get() {
        None => Outcome::Stopped,
        Some(Err(())) => Outcome::Failed,
        Some(Ok(executor)) => collect(executor),
    }
}

pub(super) fn inspect() -> Outcome {
    match EXECUTOR.get() {
        None => Outcome::Stopped,
        Some(Err(())) => Outcome::Failed,
        Some(Ok(executor)) => inspect_executor(executor),
    }
}

fn inspect_executor(executor: &Executor) -> Outcome {
    let current = match executor.current.try_lock() {
        Ok(current) => current,
        Err(TryLockError::WouldBlock) => return Outcome::Pending,
        Err(TryLockError::Poisoned(_)) => return Outcome::Failed,
    };
    let Some(record) = current.as_ref() else {
        return Outcome::Stopped;
    };
    inspect_record(record)
}

fn inspect_record(record: &Record) -> Outcome {
    if record.failed.load(Ordering::Acquire) {
        return Outcome::Failed;
    }
    let panics = match record.panics.try_lock() {
        Ok(panics) => panics,
        Err(TryLockError::WouldBlock) => return Outcome::Pending,
        Err(TryLockError::Poisoned(_)) => return Outcome::Failed,
    };
    if !panics.is_empty() {
        return Outcome::Failed;
    }
    drop(panics);

    if record.joined.load(Ordering::Acquire) {
        return Outcome::Stopped;
    }

    let task = match record.task.try_lock() {
        Ok(task) => task,
        Err(TryLockError::WouldBlock) => return Outcome::Pending,
        Err(TryLockError::Poisoned(_)) => return Outcome::Failed,
    };

    let finished = match &*task {
        Task::Unscheduled => return Outcome::Pending,
        Task::Running(handle) => {
            if !handle.is_finished() {
                return Outcome::Pending;
            }
            true
        }
        Task::Finished(Err(_)) => return Outcome::Failed,
        Task::Finished(Ok(())) => true,
    };
    drop(task);

    if finished {
        let worker = match record.worker.try_lock() {
            Ok(worker) => worker,
            Err(TryLockError::WouldBlock) => return Outcome::Pending,
            Err(TryLockError::Poisoned(_)) => return Outcome::Failed,
        };
        let panics = match record.panics.try_lock() {
            Ok(panics) => panics,
            Err(TryLockError::WouldBlock) => return Outcome::Pending,
            Err(TryLockError::Poisoned(_)) => return Outcome::Failed,
        };
        if worker.is_some() || !panics.is_empty() || record.failed.load(Ordering::Acquire) {
            return Outcome::Failed;
        }
        if !record.owner_retired.load(Ordering::Acquire) {
            return Outcome::Failed;
        }
        return Outcome::Stopped;
    }

    Outcome::Pending
}

pub(super) fn wait_for_process(timeout: Duration) -> Outcome {
    let deadline = Instant::now()
        .checked_add(timeout)
        .unwrap_or_else(Instant::now);
    loop {
        let outcome = admission();
        if outcome != Outcome::Pending {
            return outcome;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Outcome::Pending;
        }
        thread::park_timeout(remaining.min(Duration::from_millis(10)));
    }
}

fn collect(executor: &Arc<Executor>) -> Outcome {
    let mut current = match executor.current.try_lock() {
        Ok(current) => current,
        Err(TryLockError::WouldBlock) => return Outcome::Pending,
        Err(TryLockError::Poisoned(_)) => return Outcome::Failed,
    };
    let Some(record) = current.as_ref() else {
        return Outcome::Stopped;
    };
    let outcome = observe(record);
    if outcome == Outcome::Stopped {
        *current = None;
    }
    outcome
}

fn observe(record: &Record) -> Outcome {
    if record.failed.load(Ordering::Acquire) {
        return Outcome::Failed;
    }
    if record.joined.load(Ordering::Acquire) {
        return Outcome::Stopped;
    }
    let mut task = match record.task.try_lock() {
        Ok(task) => task,
        Err(TryLockError::WouldBlock) => return Outcome::Pending,
        Err(TryLockError::Poisoned(_)) => {
            record.failed.store(true, Ordering::Release);
            return Outcome::Failed;
        }
    };
    if let Task::Running(handle) = &mut *task {
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        if let std::task::Poll::Ready(result) = std::pin::Pin::new(handle).poll(&mut context) {
            // Keep actual cancellation/panic errors in the owned record.
            *task = Task::Finished(result);
        }
    }
    match &*task {
        Task::Unscheduled | Task::Running(_) => Outcome::Pending,
        Task::Finished(Err(_)) => {
            record.failed.store(true, Ordering::Release);
            Outcome::Failed
        }
        Task::Finished(Ok(())) => {
            let worker = match record.worker.try_lock() {
                Ok(worker) => worker,
                Err(TryLockError::WouldBlock) => return Outcome::Pending,
                Err(TryLockError::Poisoned(_)) => {
                    record.failed.store(true, Ordering::Release);
                    return Outcome::Failed;
                }
            };
            let panics = match record.panics.try_lock() {
                Ok(panics) => panics,
                Err(TryLockError::WouldBlock) => return Outcome::Pending,
                Err(TryLockError::Poisoned(_)) => {
                    record.failed.store(true, Ordering::Release);
                    return Outcome::Failed;
                }
            };
            if worker.is_some()
                || !panics.is_empty()
                || !record.owner_retired.load(Ordering::Acquire)
                || record.failed.load(Ordering::Acquire)
            {
                record.failed.store(true, Ordering::Release);
                return Outcome::Failed;
            }
            record.joined.store(true, Ordering::Release);
            Outcome::Stopped
        }
    }
}

impl Retirement {
    pub(super) fn start<T, G, Q>(owner: T, root: Arc<G>, shutdown: Q) -> Result<Self, T>
    where
        T: Send + 'static,
        G: Any + Send + Sync,
        Q: FnOnce(&T) -> Result<(), ()> + Send + 'static,
    {
        let executor = match EXECUTOR.get_or_init(Executor::new) {
            Ok(executor) => Arc::clone(executor),
            Err(()) => return Err(owner),
        };
        Self::start_on(executor, owner, root, shutdown)
    }

    #[cfg(all(test, feature = "full-tests"))]
    pub(super) fn start_isolated<T, G, Q>(owner: T, root: Arc<G>, shutdown: Q) -> Result<Self, T>
    where
        T: Send + 'static,
        G: Any + Send + Sync,
        Q: FnOnce(&T) -> Result<(), ()> + Send + 'static,
    {
        Self::start_on(Executor::new().unwrap(), owner, root, shutdown)
    }

    fn start_on<T, G, Q>(
        executor: Arc<Executor>,
        owner: T,
        root: Arc<G>,
        shutdown: Q,
    ) -> Result<Self, T>
    where
        T: Send + 'static,
        G: Any + Send + Sync,
        Q: FnOnce(&T) -> Result<(), ()> + Send + 'static,
    {
        if collect(&executor) != Outcome::Stopped {
            return Err(owner);
        }
        let mut current = match executor.current.try_lock() {
            Ok(current) if current.is_none() => current,
            _ => return Err(owner),
        };
        let owner = Arc::new(Mutex::new(Some(owner)));
        let record = Arc::new(Record {
            _owner: owner.clone(),
            _root: root,
            worker: Mutex::new(None),
            task: Mutex::new(Task::Unscheduled),
            panics: Mutex::new(Vec::new()),
            failed: AtomicBool::new(false),
            owner_retired: AtomicBool::new(false),
            joined: AtomicBool::new(false),
        });
        *current = Some(Arc::clone(&record));
        // No owner can execute shutdown before its native handle is registered.
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        let work_owner = Arc::clone(&owner);
        let work_record = Arc::clone(&record);
        let worker = thread::Builder::new()
            .name("journal-retire".into())
            .spawn(move || {
                if receive.recv().is_err() {
                    return;
                }
                let mut owner = work_owner.lock().unwrap_or_else(|p| p.into_inner());
                // Borrow the original MobileServing for pre-Drained shutdown.
                // An error/panic leaves its actual Runtime/routes in this cell.
                let result = catch_unwind(AssertUnwindSafe(|| {
                    Phase::RetirementCallback
                        .run(|| shutdown(owner.as_ref().expect("registered retirement owner")))
                }));
                match result {
                    Ok(Ok(())) => {
                        let value = owner.take().expect("quiescent retirement owner");
                        drop(owner);
                        Phase::OwnerDrop.run(|| drop(value));
                        work_record.owner_retired.store(true, Ordering::Release);
                    }
                    Ok(Err(())) => work_record.failed.store(true, Ordering::Release),
                    Err(payload) => {
                        work_record
                            .panics
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .push(payload);
                        work_record.failed.store(true, Ordering::Release);
                    }
                }
            });
        let worker = match worker {
            Ok(worker) => worker,
            Err(_) => {
                *current = None;
                return Err(owner
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .take()
                    .unwrap());
            }
        };
        *record.worker.lock().unwrap_or_else(|p| p.into_inner()) = Some(worker);
        let join_record = Arc::clone(&record);
        let scheduled = catch_unwind(AssertUnwindSafe(|| {
            #[cfg(all(test, feature = "full-tests"))]
            SCHEDULING_PANIC.with(|fault| {
                if let Some(payload) = fault.borrow_mut().take() {
                    std::panic::resume_unwind(payload);
                }
            });
            executor.runtime.spawn_blocking(move || {
                let worker = join_record
                    .worker
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .take();
                if let Some(worker) = worker {
                    if let Err(payload) = worker.join() {
                        join_record
                            .panics
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .push(payload);
                        join_record.failed.store(true, Ordering::Release);
                    }
                } else {
                    join_record.failed.store(true, Ordering::Release);
                }
            })
        }));
        match scheduled {
            Ok(task) => {
                *record.task.lock().unwrap_or_else(|p| p.into_inner()) = Task::Running(task)
            }
            Err(payload) => {
                record
                    .panics
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(payload);
                record.failed.store(true, Ordering::Release);
                // A native worker is already registered. Keep the exact original
                // owner and native handle in process custody on scheduling failure.
                drop(send);
                drop(current);
                return Ok(Self { executor, record });
            }
        }
        if send.send(()).is_err() {
            record.failed.store(true, Ordering::Release);
            drop(current);
            return Ok(Self { executor, record });
        }
        drop(current);
        Ok(Self { executor, record })
    }

    pub(super) fn wait(&mut self, timeout: Duration) -> Outcome {
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);
        loop {
            let result = observe(&self.record);
            if result != Outcome::Pending {
                if result == Outcome::Stopped {
                    let _ = collect(&self.executor);
                }
                return result;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Outcome::Pending;
            }
            thread::park_timeout(remaining.min(Duration::from_millis(10)));
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::mpsc;

    // The allocated TLS collection owns the real guard through native thread exit.
    thread_local! { static TLS: RefCell<Vec<HeldTls>> = RefCell::new(Vec::with_capacity(1)); }

    struct HeldTls {
        entered: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
    }
    impl Drop for HeldTls {
        fn drop(&mut self) {
            self.entered.send(()).unwrap();
            self.release.recv_timeout(Duration::from_secs(10)).unwrap();
        }
    }

    struct Release(Option<mpsc::Sender<()>>);
    impl Drop for Release {
        fn drop(&mut self) {
            if let Some(release) = self.0.take() {
                let _ = release.send(());
            }
        }
    }

    #[test]
    fn actual_join_includes_tls_and_registry_survives_observer_loss() {
        let executor = Executor::new().unwrap();
        let root = Arc::new(());
        let weak = Arc::downgrade(&root);
        let (entered, observed) = mpsc::channel();
        let (release, held) = mpsc::channel();
        let mut release = Release(Some(release));
        let mut retirement = Retirement::start_on(executor.clone(), (), root, move |_| {
            TLS.with(|tls| {
                tls.borrow_mut().push(HeldTls {
                    entered,
                    release: held,
                });
            });
            Ok(())
        })
        .ok()
        .unwrap();
        observed.recv_timeout(Duration::from_secs(5)).unwrap();
        let started = Instant::now();
        assert_eq!(retirement.wait(Duration::from_millis(20)), Outcome::Pending);
        assert!(started.elapsed() < Duration::from_millis(250));
        assert!(weak.upgrade().is_some());
        drop(retirement);
        assert_eq!(collect(&executor), Outcome::Pending);
        assert!(Retirement::start_on(executor.clone(), (), Arc::new(()), |_| Ok(())).is_err());
        release.0.take().unwrap().send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while collect(&executor) == Outcome::Pending && Instant::now() < deadline {
            thread::park_timeout(Duration::from_millis(10));
        }
        assert_eq!(collect(&executor), Outcome::Stopped);
        assert!(weak.upgrade().is_none());
        let mut next = Retirement::start_on(executor, (), Arc::new(()), |_| Ok(()))
            .ok()
            .unwrap();
        assert_eq!(next.wait(Duration::from_secs(5)), Outcome::Stopped);
    }

    #[test]
    fn held_owner_destructor_keeps_native_join_pending() {
        struct Owner {
            entered: mpsc::Sender<()>,
            release: mpsc::Receiver<()>,
        }
        impl Drop for Owner {
            fn drop(&mut self) {
                self.entered.send(()).unwrap();
                self.release.recv_timeout(Duration::from_secs(10)).unwrap();
            }
        }
        let executor = Executor::new().unwrap();
        let (entered, observed) = mpsc::channel();
        let (release, held) = mpsc::channel();
        let mut release = Release(Some(release));
        let mut retirement = Retirement::start_on(
            executor.clone(),
            Owner {
                entered,
                release: held,
            },
            Arc::new(()),
            |_| Ok(()),
        )
        .ok()
        .unwrap();
        observed.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(retirement.wait(Duration::from_millis(20)), Outcome::Pending);
        assert_eq!(collect(&executor), Outcome::Pending);
        release.0.take().unwrap().send(()).unwrap();
        assert_eq!(retirement.wait(Duration::from_secs(5)), Outcome::Stopped);
    }

    #[test]
    fn canceled_queued_join_keeps_original_native_handle_and_root() {
        let executor = Executor::new().unwrap();
        let (occupied, observed) = mpsc::channel();
        let (unblock, blocked) = mpsc::channel();
        let mut unblock = Release(Some(unblock));
        let pool = executor.runtime.spawn_blocking(move || {
            occupied.send(()).unwrap();
            blocked.recv_timeout(Duration::from_secs(10)).unwrap();
        });
        observed.recv_timeout(Duration::from_secs(5)).unwrap();
        let root = Arc::new(());
        let identity = Arc::downgrade(&root);
        let (entered, observed) = mpsc::channel();
        let (release, held) = mpsc::channel();
        let mut release = Release(Some(release));
        let mut retirement = Retirement::start_on(executor.clone(), (), root, move |_| {
            TLS.with(|tls| {
                tls.borrow_mut().push(HeldTls {
                    entered,
                    release: held,
                });
            });
            Ok(())
        })
        .ok()
        .unwrap();
        observed.recv_timeout(Duration::from_secs(5)).unwrap();
        {
            let task = retirement.record.task.lock().unwrap();
            let Task::Running(task) = &*task else {
                panic!("actual queued join task absent")
            };
            task.abort();
        }
        // A queued blocking task is canceled when the occupied pool can service it.
        unblock.0.take().unwrap().send(()).unwrap();
        executor.runtime.block_on(pool).unwrap();
        assert_eq!(retirement.wait(Duration::from_secs(5)), Outcome::Failed);
        assert!(retirement.record.worker.lock().unwrap().is_some());
        assert!(identity.upgrade().is_some());
        drop(retirement);
        assert_eq!(collect(&executor), Outcome::Failed);
        release.0.take().unwrap().send(()).unwrap();
        assert_eq!(collect(&executor), Outcome::Failed);
        std::mem::forget(executor);
    }

    #[test]
    fn held_record_cannot_extend_observation_deadline() {
        let executor = Executor::new().unwrap();
        let (release, held) = mpsc::channel();
        let mut release = Release(Some(release));
        let mut retirement = Retirement::start_on(executor, (), Arc::new(()), move |_| {
            held.recv_timeout(Duration::from_secs(10)).unwrap();
            Ok(())
        })
        .ok()
        .unwrap();
        let record = Arc::clone(&retirement.record);
        let (entered, observed) = mpsc::channel();
        let (unlock, locked) = mpsc::channel();
        let mut unlock = Release(Some(unlock));
        let holder = thread::spawn(move || {
            let _task = record.task.lock().unwrap();
            entered.send(()).unwrap();
            locked.recv_timeout(Duration::from_secs(10)).unwrap();
        });
        observed.recv_timeout(Duration::from_secs(5)).unwrap();
        let started = Instant::now();
        assert_eq!(retirement.wait(Duration::from_millis(20)), Outcome::Pending);
        assert!(started.elapsed() < Duration::from_millis(250));
        unlock.0.take().unwrap().send(()).unwrap();
        holder.join().unwrap();
        release.0.take().unwrap().send(()).unwrap();
        assert_eq!(retirement.wait(Duration::from_secs(5)), Outcome::Stopped);
    }

    #[test]
    fn failed_borrowed_shutdown_keeps_original_owner_and_root() {
        struct Owner(Arc<AtomicBool>);
        impl Drop for Owner {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let root = Arc::new(());
        let weak = Arc::downgrade(&root);
        let mut retirement = Retirement::start_isolated(Owner(dropped.clone()), root, |_| Err(()))
            .ok()
            .unwrap();
        let executor = retirement.executor.clone();
        assert_eq!(retirement.wait(Duration::from_secs(5)), Outcome::Failed);
        assert_eq!(retirement.wait(Duration::ZERO), Outcome::Failed);
        drop(retirement);
        assert_eq!(collect(&executor), Outcome::Failed);
        assert!(!dropped.load(Ordering::Acquire));
        assert!(weak.upgrade().is_some());
        assert!(Retirement::start_on(executor.clone(), (), Arc::new(()), |_| Ok(())).is_err());
        // Match production's process-lifetime custody for the intentional failure.
        // This test makes no successful retirement/teardown claim.
        std::mem::forget(executor);
    }

    #[test]
    fn scheduling_panic_keeps_registered_owner_handle_root_and_payload() {
        struct Owner(Arc<AtomicBool>);
        impl Drop for Owner {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        struct Payload(Arc<AtomicBool>);
        impl Drop for Payload {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
                panic!("opaque scheduling payload must remain owned");
            }
        }
        let executor = Executor::new().unwrap();
        let root = Arc::new(());
        let weak = Arc::downgrade(&root);
        let dropped = Arc::new(AtomicBool::new(false));
        let payload_dropped = Arc::new(AtomicBool::new(false));
        SCHEDULING_PANIC
            .with(|fault| *fault.borrow_mut() = Some(Box::new(Payload(payload_dropped.clone()))));
        let mut retirement =
            Retirement::start_on(executor.clone(), Owner(dropped.clone()), root, |_| {
                panic!("unscheduled owner must not execute")
            })
            .ok()
            .unwrap();
        assert_eq!(retirement.wait(Duration::ZERO), Outcome::Failed);
        assert!(retirement.record.worker.lock().unwrap().is_some());
        assert_eq!(retirement.record.panics.lock().unwrap().len(), 1);
        assert!(!dropped.load(Ordering::Acquire));
        assert!(!payload_dropped.load(Ordering::Acquire));
        drop(retirement);
        assert_eq!(collect(&executor), Outcome::Failed);
        assert!(weak.upgrade().is_some());
        assert!(Retirement::start_on(executor.clone(), (), Arc::new(()), |_| Ok(())).is_err());
        std::mem::forget(executor);
    }

    #[test]
    fn opaque_owner_destructor_panic_is_retained_by_actual_native_join() {
        struct Payload(Arc<AtomicBool>);
        impl Drop for Payload {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
                panic!("opaque native-join payload must remain owned");
            }
        }
        struct Owner(Option<Payload>);
        impl Drop for Owner {
            fn drop(&mut self) {
                std::panic::panic_any(self.0.take().unwrap());
            }
        }
        let executor = Executor::new().unwrap();
        let root = Arc::new(());
        let weak = Arc::downgrade(&root);
        let dropped = Arc::new(AtomicBool::new(false));
        let mut retirement = Retirement::start_on(
            executor.clone(),
            Owner(Some(Payload(dropped.clone()))),
            root,
            |_| Ok(()),
        )
        .ok()
        .unwrap();
        assert_eq!(retirement.wait(Duration::from_secs(5)), Outcome::Failed);
        assert_eq!(retirement.wait(Duration::ZERO), Outcome::Failed);
        assert_eq!(retirement.record.panics.lock().unwrap().len(), 1);
        assert!(retirement.record.worker.lock().unwrap().is_none());
        assert!(!retirement.record.owner_retired.load(Ordering::Acquire));
        assert!(!dropped.load(Ordering::Acquire));
        drop(retirement);
        assert_eq!(collect(&executor), Outcome::Failed);
        assert!(weak.upgrade().is_some());
        std::mem::forget(executor);
    }
}

#[cfg(test)]
mod inspect_tests {
    use super::*;

    #[test]
    fn inspect_on_finished_record_is_idempotent_and_retains_executor() {
        let executor = Executor::new().unwrap();
        let record = Arc::new(Record {
            _owner: Arc::new(()),
            _root: Arc::new(()),
            worker: Mutex::new(None),
            task: Mutex::new(Task::Finished(Ok(()))),
            panics: Mutex::new(Vec::new()),
            failed: AtomicBool::new(false),
            owner_retired: AtomicBool::new(true),
            joined: AtomicBool::new(false),
        });
        *executor.current.lock().unwrap() = Some(record.clone());

        assert_eq!(inspect_executor(&executor), Outcome::Stopped);
        // Idempotent: repeated inspect produces the same outcome
        assert_eq!(inspect_executor(&executor), Outcome::Stopped);
        // Retains executor record in current, unlike collect() which would clear it
        let current = executor.current.try_lock().unwrap();
        assert!(current.is_some());
        drop(current);

        match &*record.task.lock().unwrap() {
            Task::Finished(Ok(())) => {}
            _ => panic!("task state must be preserved"),
        }
        assert!(!record.failed.load(Ordering::Acquire));
    }

    #[test]
    fn inspect_on_running_record_reports_pending_without_mutating() {
        let executor = Executor::new().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = executor.runtime.spawn(async move {
            let _ = rx.recv();
        });
        let record = Arc::new(Record {
            _owner: Arc::new(()),
            _root: Arc::new(()),
            worker: Mutex::new(None),
            task: Mutex::new(Task::Running(handle)),
            panics: Mutex::new(Vec::new()),
            failed: AtomicBool::new(false),
            owner_retired: AtomicBool::new(false),
            joined: AtomicBool::new(false),
        });
        *executor.current.lock().unwrap() = Some(record.clone());

        assert_eq!(inspect_executor(&executor), Outcome::Pending);
        assert_eq!(inspect_executor(&executor), Outcome::Pending);

        // Verify record and task state were not mutated
        let current = executor.current.try_lock().unwrap();
        assert!(current.is_some());
        drop(current);

        assert!(!record.failed.load(Ordering::Acquire));
        assert!(!record.joined.load(Ordering::Acquire));
        match &*record.task.lock().unwrap() {
            Task::Running(_) => {}
            _ => panic!("task state must remain running without mutation"),
        }

        let _ = tx.send(());
    }
}
