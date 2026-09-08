// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Retained failure state for the existing bounded helper owner.

use std::fmt;
use std::io;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock, TryLockError};
use std::time::{Duration, Instant};

use super::super::ProcessInstance;
use super::bounded::{BoundedHelperError, BoundedHelperResources, CaptureError};
use super::job_process::{NativeLaunchError, UnfinalizedWindowsJob, WindowsJobProcess};

const RESERVED: u8 = 0;
const PENDING: u8 = 1;
const COMPLETE: u8 = 2;
static PENDING_COUNT: AtomicUsize = AtomicUsize::new(0);
static INVOCATIONS: Mutex<Vec<Arc<Invocation>>> = Mutex::new(Vec::new());

#[cfg(feature = "test-hooks")]
thread_local! {
    static OBSERVATION_FAULT: std::cell::RefCell<Option<HelperCleanupObservationFault>> = const { std::cell::RefCell::new(None) };
}

/// Native test instrument: withholds completion observations on a genuinely
/// launched owner. It grants no handle/identity and cannot synthesize a failure.
#[cfg(feature = "test-hooks")]
#[derive(Clone, Debug)]
pub struct HelperCleanupObservationFault(Arc<std::sync::atomic::AtomicBool>);
#[cfg(feature = "test-hooks")]
impl HelperCleanupObservationFault {
    pub fn new() -> Self {
        Self(Arc::new(std::sync::atomic::AtomicBool::new(true)))
    }
    pub fn release(&self) {
        self.0.store(false, Ordering::Release);
    }
    fn active(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
#[cfg(feature = "test-hooks")]
impl Default for HelperCleanupObservationFault {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "test-hooks")]
pub fn run_bounded_helper_with_observation_fault_for_test(
    request: super::bounded::BoundedHelperRequest,
    fault: &HelperCleanupObservationFault,
) -> Result<super::bounded::BoundedHelperOutput, BoundedHelperFailure> {
    struct Restore(Option<HelperCleanupObservationFault>);
    impl Drop for Restore {
        fn drop(&mut self) {
            OBSERVATION_FAULT.with(|slot| {
                slot.replace(self.0.take());
            });
        }
    }
    let _restore = Restore(OBSERVATION_FAULT.with(|slot| slot.replace(Some(fault.clone()))));
    super::bounded::run_bounded_helper(request)
}

pub(super) fn current_observation_fault_active() -> bool {
    #[cfg(feature = "test-hooks")]
    {
        OBSERVATION_FAULT.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_some_and(HelperCleanupObservationFault::active)
        })
    }
    #[cfg(not(feature = "test-hooks"))]
    {
        false
    }
}

struct Invocation {
    phase: AtomicU8,
    identity: OnceLock<Option<ProcessInstance>>,
    process_id: OnceLock<Option<u32>>,
    helper_admission_blocker: bool,
    #[cfg(feature = "test-hooks")]
    observation_fault: Option<HelperCleanupObservationFault>,
    payload: OnceLock<Mutex<Option<PendingPayload>>>,
}

impl Invocation {
    fn observation_unavailable(&self) -> bool {
        #[cfg(feature = "test-hooks")]
        {
            self.observation_fault
                .as_ref()
                .is_some_and(HelperCleanupObservationFault::active)
        }
        #[cfg(not(feature = "test-hooks"))]
        {
            false
        }
    }
}

struct PendingPayload {
    owner: PendingNativeOwner,
    io: HelperIo,
    _resources: BoundedHelperResources,
}

enum PendingNativeOwner {
    Running(WindowsJobProcess),
    Shared {
        owner: Arc<Mutex<WindowsJobProcess>>,
        identity: ProcessInstance,
    },
    Unfinalized(UnfinalizedWindowsJob),
}
impl PendingNativeOwner {
    fn identity(&self) -> Option<ProcessInstance> {
        match self {
            Self::Running(owner) => Some(owner.identity()),
            Self::Shared { identity, .. } => Some(*identity),
            Self::Unfinalized(_) => None,
        }
    }
    fn process_id(&self) -> Option<u32> {
        match self {
            Self::Running(owner) => Some(owner.identity().pid),
            Self::Shared { identity, .. } => Some(identity.pid),
            Self::Unfinalized(owner) => owner.process_id(),
        }
    }
    fn is_quiescent(&self) -> io::Result<bool> {
        match self {
            Self::Running(owner) => owner.is_quiescent(),
            Self::Shared { owner, .. } => lock_native_owner_until(owner, None)?.is_quiescent(),
            Self::Unfinalized(owner) => owner.is_quiescent(),
        }
    }
    fn hard_stop_until(&mut self, deadline: Instant) -> io::Result<()> {
        match self {
            Self::Running(owner) => owner.hard_stop_until(deadline).map(|_| ()),
            Self::Shared { owner, .. } => lock_native_owner_until(owner, Some(deadline))?
                .hard_stop_until(deadline)
                .map(|_| ()),
            Self::Unfinalized(owner) => owner.hard_stop_until(deadline),
        }
    }
}

pub(super) fn lock_native_owner_until(
    owner: &Mutex<WindowsJobProcess>,
    deadline: Option<Instant>,
) -> io::Result<std::sync::MutexGuard<'_, WindowsJobProcess>> {
    loop {
        match owner.try_lock() {
            Ok(owner) => return Ok(owner),
            Err(TryLockError::Poisoned(_)) => {
                return Err(io::Error::other(
                    "native owner lock poisoned; cleanup remains retained",
                ));
            }
            Err(TryLockError::WouldBlock) => {
                let Some(deadline) = deadline else {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "native owner is being observed",
                    ));
                };
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "native owner lock deadline elapsed",
                    ));
                }
                std::thread::sleep(
                    Duration::from_millis(1)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
        }
    }
}

pub(super) struct Completion<T> {
    receiver: Receiver<T>,
    settled: bool,
}

impl<T> Completion<T> {
    pub(super) fn unstarted() -> Self {
        let (_sender, receiver) = std::sync::mpsc::channel();
        Self {
            receiver,
            settled: true,
        }
    }

    pub(super) fn new(receiver: Receiver<T>) -> Self {
        Self {
            receiver,
            settled: false,
        }
    }

    pub(super) fn try_recv(&mut self) -> Result<T, TryRecvError> {
        let result = self.receiver.try_recv();
        if !matches!(result, Err(TryRecvError::Empty)) {
            self.settled = true;
        }
        result
    }

    fn observe(&mut self) {
        if !self.settled {
            let _ = self.try_recv();
        }
    }

    fn drain_until(&mut self, deadline: Instant) {
        if self.settled {
            return;
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return;
        };
        if !matches!(
            self.receiver.recv_timeout(remaining),
            Err(RecvTimeoutError::Timeout)
        ) {
            self.settled = true;
        }
    }
}

pub(super) struct HelperIo {
    pub(super) workers: Vec<std::thread::JoinHandle<()>>,
    pub(super) stdin: Completion<io::Result<()>>,
    pub(super) stdout: Completion<Result<Vec<u8>, CaptureError>>,
    pub(super) stderr: Completion<Result<Vec<u8>, CaptureError>>,
}

impl HelperIo {
    pub(super) fn unstarted() -> Self {
        Self {
            workers: Vec::new(),
            stdin: Completion::unstarted(),
            stdout: Completion::unstarted(),
            stderr: Completion::unstarted(),
        }
    }

    pub(super) fn observe(&mut self) {
        settle_workers(&mut self.workers);
        self.stdin.observe();
        self.stdout.observe();
        self.stderr.observe();
    }

    pub(super) fn settled(&self) -> bool {
        self.workers.is_empty() && self.stdin.settled && self.stdout.settled && self.stderr.settled
    }

    pub(super) fn drain_until(&mut self, deadline: Instant) {
        self.stdin.drain_until(deadline);
        self.stdout.drain_until(deadline);
        self.stderr.drain_until(deadline);
        drain_workers_until(&mut self.workers, deadline);
    }
}

// Join only after completion is observed. Retain unfinished handles, including
// during deadline expiry; no detached waiter can outlive the retained record.
pub(super) fn settle_workers(workers: &mut Vec<std::thread::JoinHandle<()>>) {
    let mut index = 0;
    while index < workers.len() {
        if workers[index].is_finished() {
            let _ = workers.swap_remove(index).join();
        } else {
            index += 1;
        }
    }
}

pub(super) fn drain_workers_until(
    workers: &mut Vec<std::thread::JoinHandle<()>>,
    deadline: Instant,
) {
    loop {
        settle_workers(workers);
        if workers.is_empty() || Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(
            Duration::from_millis(1).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

/// One actual helper Job and its inputs whose cleanup was incomplete at return.
/// Cloning this handle does not duplicate any native process or Job authority.
#[derive(Clone)]
pub struct BoundedHelperCleanup {
    identity: Option<ProcessInstance>,
    process_id: Option<u32>,
    state: Arc<Invocation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HelperCleanupStatus {
    Quiescent,
    Pending,
}

impl fmt::Debug for BoundedHelperCleanup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoundedHelperCleanup")
            .field("identity", &self.identity)
            .field("process_id", &self.process_id)
            .field(
                "completed",
                &(self.state.phase.load(Ordering::Acquire) == COMPLETE),
            )
            .finish()
    }
}

impl BoundedHelperCleanup {
    pub fn identity(&self) -> Option<ProcessInstance> {
        self.identity
    }
    pub fn process_id(&self) -> Option<u32> {
        self.process_id
    }

    /// Nonblocking observation only: no wait, retry of termination, or PID reopen.
    pub fn observe(&self) -> HelperCleanupStatus {
        self.progress(None)
    }

    /// One bounded cleanup attempt on the original owner. Lock contention and
    /// all stream stages share the supplied deadline and existing cleanup cap.
    pub fn retry_until(&self, deadline: Instant) -> HelperCleanupStatus {
        self.progress(Some(
            deadline.min(Instant::now() + super::super::DRAIN_JOIN_TIMEOUT),
        ))
    }

    fn progress(&self, deadline: Option<Instant>) -> HelperCleanupStatus {
        if self.state.phase.load(Ordering::Acquire) == COMPLETE {
            return HelperCleanupStatus::Quiescent;
        }
        if self.state.observation_unavailable() {
            return HelperCleanupStatus::Pending;
        }
        let Some(payload) = self.state.payload.get() else {
            return HelperCleanupStatus::Pending;
        };
        let mut guard = loop {
            match payload.try_lock() {
                Ok(guard) => break guard,
                Err(TryLockError::Poisoned(_)) => return HelperCleanupStatus::Pending,
                Err(TryLockError::WouldBlock) => {
                    let Some(deadline) = deadline else {
                        return HelperCleanupStatus::Pending;
                    };
                    if Instant::now() >= deadline {
                        return HelperCleanupStatus::Pending;
                    }
                    std::thread::sleep(
                        Duration::from_millis(1)
                            .min(deadline.saturating_duration_since(Instant::now())),
                    );
                }
            }
        };
        let Some(retained) = guard.as_mut() else {
            return if self.state.phase.load(Ordering::Acquire) == COMPLETE {
                HelperCleanupStatus::Quiescent
            } else {
                HelperCleanupStatus::Pending
            };
        };
        let mut quiescent = retained.owner.is_quiescent().ok() == Some(true);
        if let Some(deadline) = deadline {
            if !quiescent && Instant::now() < deadline {
                let _ = retained.owner.hard_stop_until(deadline);
            }
            retained.io.drain_until(deadline);
            quiescent = retained.owner.is_quiescent().ok() == Some(true);
        } else {
            retained.io.observe();
        }
        if !quiescent || !retained.io.settled() {
            return HelperCleanupStatus::Pending;
        }
        // Clear the actual payload, not just the registry reference: a completed
        // error Arc must no longer keep the last generation/input owner alive.
        let completed = guard.take().expect("retained payload was observed above");
        drop(completed);
        self.state.phase.store(COMPLETE, Ordering::Release);
        if self.state.helper_admission_blocker {
            PENDING_COUNT.fetch_sub(1, Ordering::SeqCst);
        }
        HelperCleanupStatus::Quiescent
    }
}

#[derive(Debug)]
pub enum HelperAdmissionStatus {
    Ready,
    Pending(Vec<BoundedHelperCleanup>),
    Contended,
}

fn snapshot(helper_only: bool) -> HelperAdmissionStatus {
    let mut registry = match INVOCATIONS.try_lock() {
        Ok(registry) => registry,
        Err(_) => return HelperAdmissionStatus::Contended,
    };
    registry.retain(|state| state.phase.load(Ordering::Acquire) != COMPLETE);
    if helper_only && PENDING_COUNT.load(Ordering::SeqCst) == 0 {
        return HelperAdmissionStatus::Ready;
    }
    let pending = registry
        .iter()
        .filter(|state| {
            state.helper_admission_blocker == helper_only
                && state.phase.load(Ordering::Acquire) == PENDING
        })
        .filter_map(|state| {
            state
                .identity
                .get()
                .cloned()
                .map(|identity| BoundedHelperCleanup {
                    identity,
                    process_id: state.process_id.get().copied().flatten(),
                    state: state.clone(),
                })
        })
        .collect::<Vec<_>>();
    if pending.is_empty() && helper_only {
        HelperAdmissionStatus::Contended
    } else if pending.is_empty() {
        HelperAdmissionStatus::Ready
    } else {
        HelperAdmissionStatus::Pending(pending)
    }
}

pub fn observe_bounded_helper_admission() -> HelperAdmissionStatus {
    observe_class(true)
}

/// Observe failed independent launches only; never waits, stops or gates a service.
pub fn observe_windows_launch_cleanup() -> HelperAdmissionStatus {
    observe_class(false)
}

fn observe_class(helper_only: bool) -> HelperAdmissionStatus {
    let pending = match snapshot(helper_only) {
        HelperAdmissionStatus::Pending(pending) => pending,
        other => return other,
    };
    for cleanup in pending {
        let _ = cleanup.observe();
    }
    snapshot(helper_only)
}

pub fn retry_bounded_helper_admission_until(deadline: Instant) -> HelperAdmissionStatus {
    retry_class(true, deadline)
}

/// Explicit recovery for failed independent launches. A remaining failure does
/// not refuse unrelated service launch. No registry lock is held through waits.
pub fn retry_windows_launch_cleanup_until(deadline: Instant) -> HelperAdmissionStatus {
    retry_class(false, deadline)
}

fn retry_class(helper_only: bool, deadline: Instant) -> HelperAdmissionStatus {
    let deadline = deadline.min(Instant::now() + super::super::DRAIN_JOIN_TIMEOUT);
    let pending = loop {
        match snapshot(helper_only) {
            HelperAdmissionStatus::Contended if Instant::now() < deadline => {
                std::thread::sleep(
                    Duration::from_millis(1)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            HelperAdmissionStatus::Pending(pending) => break pending,
            other => return other,
        }
    };
    for cleanup in pending {
        if Instant::now() >= deadline {
            break;
        }
        let _ = cleanup.retry_until(deadline);
    }
    snapshot(helper_only)
}

/// Private construction guarantees incomplete outcomes have real ownership.
#[derive(Debug)]
pub struct BoundedHelperFailure {
    cause: BoundedHelperError,
    cleanup: Option<BoundedHelperCleanup>,
    blockers: Vec<BoundedHelperCleanup>,
}

impl BoundedHelperFailure {
    pub fn cause(&self) -> &BoundedHelperError {
        &self.cause
    }
    pub fn cleanup(&self) -> Option<&BoundedHelperCleanup> {
        self.cleanup.as_ref()
    }
    pub fn blockers(&self) -> &[BoundedHelperCleanup] {
        &self.blockers
    }

    pub(super) fn prelaunch(cause: BoundedHelperError) -> Self {
        // This constructor is private and refuses all postlaunch reasons.
        assert!(!matches!(
            cause,
            BoundedHelperError::LaunchFinalizationFailed { .. }
                | BoundedHelperError::IoWorkerStartFailed { .. }
                | BoundedHelperError::InputWriteFailed { .. }
                | BoundedHelperError::OutputLimitExceeded { .. }
                | BoundedHelperError::OutputReadFailed { .. }
                | BoundedHelperError::DeadlineExceeded { .. }
                | BoundedHelperError::ProcessObservationFailed { .. }
                | BoundedHelperError::JobNotQuiescent { .. }
        ));
        Self {
            cause,
            cleanup: None,
            blockers: Vec::new(),
        }
    }
}

impl fmt::Display for BoundedHelperFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.cause)?;
        if let Some(cleanup) = &self.cleanup {
            write!(f, "; cleanup was incomplete at return: {cleanup:?}")?;
        }
        if !self.blockers.is_empty() {
            write!(
                f,
                "; prior helper cleanup blocks admission: {:?}",
                self.blockers
            )?;
        }
        Ok(())
    }
}
impl std::error::Error for BoundedHelperFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}

pub(super) struct Reservation {
    state: Arc<Invocation>,
    published: bool,
}

impl Reservation {
    pub(super) fn acquire(deadline: Instant) -> Result<Self, BoundedHelperFailure> {
        match retry_bounded_helper_admission_until(deadline) {
            HelperAdmissionStatus::Ready => {}
            HelperAdmissionStatus::Pending(blockers) => {
                return Err(BoundedHelperFailure {
                    cause: BoundedHelperError::CleanupPending,
                    cleanup: None,
                    blockers,
                });
            }
            HelperAdmissionStatus::Contended => {
                return Err(BoundedHelperFailure::prelaunch(
                    BoundedHelperError::CleanupContended,
                ));
            }
        }
        Self::register_until(deadline, true)
    }

    pub(super) fn track_until(deadline: Instant) -> Result<Self, BoundedHelperFailure> {
        let _ = retry_windows_launch_cleanup_until(deadline);
        Self::register_until(deadline, false)
    }

    fn register_until(
        deadline: Instant,
        helper_admission_blocker: bool,
    ) -> Result<Self, BoundedHelperFailure> {
        let mut registry = loop {
            if Instant::now() >= deadline {
                return Err(BoundedHelperFailure::prelaunch(
                    BoundedHelperError::AdmissionDeadlineExceeded,
                ));
            }
            match INVOCATIONS.try_lock() {
                Ok(registry) => break registry,
                Err(TryLockError::Poisoned(_)) => {
                    return Err(BoundedHelperFailure::prelaunch(
                        BoundedHelperError::CleanupContended,
                    ));
                }
                Err(TryLockError::WouldBlock) => std::thread::sleep(
                    Duration::from_millis(1)
                        .min(deadline.saturating_duration_since(Instant::now())),
                ),
            }
        };
        // This single SeqCst read is admission's linearization point. A failure
        // published afterward belongs to concurrent work already admitted here.
        if helper_admission_blocker && PENDING_COUNT.load(Ordering::SeqCst) != 0 {
            drop(registry);
            return Err(match snapshot(true) {
                HelperAdmissionStatus::Pending(blockers) => BoundedHelperFailure {
                    cause: BoundedHelperError::CleanupPending,
                    cleanup: None,
                    blockers,
                },
                _ => BoundedHelperFailure::prelaunch(BoundedHelperError::CleanupContended),
            });
        }
        let state = Arc::new(Invocation {
            phase: AtomicU8::new(RESERVED),
            identity: OnceLock::new(),
            process_id: OnceLock::new(),
            helper_admission_blocker,
            #[cfg(feature = "test-hooks")]
            observation_fault: OBSERVATION_FAULT.with(|slot| slot.borrow().clone()),
            payload: OnceLock::new(),
        });
        registry.retain(|state| state.phase.load(Ordering::Acquire) != COMPLETE);
        registry.push(state.clone());
        Ok(Self {
            state,
            published: false,
        })
    }

    pub(super) fn launch_failure(
        self,
        failure: NativeLaunchError,
        resources: BoundedHelperResources,
    ) -> BoundedHelperFailure {
        match failure {
            NativeLaunchError::BeforeCreate(_) => {
                BoundedHelperFailure::prelaunch(BoundedHelperError::LaunchFailed)
            }
            NativeLaunchError::AfterCreate { source, mut owner } => {
                if !self.state.observation_unavailable() {
                    let _ =
                        owner.hard_stop_until(Instant::now() + super::super::DRAIN_JOIN_TIMEOUT);
                }
                let cause = BoundedHelperError::LaunchFinalizationFailed {
                    detail: source.to_string(),
                    process_id: owner.process_id(),
                };
                self.finish_failure(
                    cause,
                    PendingNativeOwner::Unfinalized(owner),
                    HelperIo::unstarted(),
                    resources,
                )
            }
        }
    }

    pub(super) fn failure(
        self,
        cause: BoundedHelperError,
        owner: WindowsJobProcess,
        io: HelperIo,
        resources: BoundedHelperResources,
    ) -> BoundedHelperFailure {
        self.finish_failure(cause, PendingNativeOwner::Running(owner), io, resources)
    }

    pub(super) fn independent_failure(
        self,
        mut owner: WindowsJobProcess,
        detail: String,
        resources: BoundedHelperResources,
    ) -> BoundedHelperFailure {
        let _ = owner.hard_stop_until(Instant::now() + super::super::DRAIN_JOIN_TIMEOUT);
        let cause = BoundedHelperError::LaunchFinalizationFailed {
            detail,
            process_id: Some(owner.identity().pid),
        };
        self.failure(cause, owner, HelperIo::unstarted(), resources)
    }

    pub(super) fn shared_failure(
        self,
        owner: Arc<Mutex<WindowsJobProcess>>,
        identity: ProcessInstance,
        io: HelperIo,
        resources: BoundedHelperResources,
        detail: String,
    ) -> BoundedHelperFailure {
        self.finish_failure(
            BoundedHelperError::LaunchFinalizationFailed {
                detail,
                process_id: Some(identity.pid),
            },
            PendingNativeOwner::Shared { owner, identity },
            io,
            resources,
        )
    }

    fn finish_failure(
        mut self,
        cause: BoundedHelperError,
        owner: PendingNativeOwner,
        mut io: HelperIo,
        resources: BoundedHelperResources,
    ) -> BoundedHelperFailure {
        io.observe();
        if !self.state.observation_unavailable()
            && owner.is_quiescent().ok() == Some(true)
            && io.settled()
        {
            return BoundedHelperFailure {
                cause,
                cleanup: None,
                blockers: Vec::new(),
            };
        }
        let identity = owner.identity();
        let process_id = owner.process_id();
        let _ = self.state.identity.set(identity);
        let _ = self.state.process_id.set(process_id);
        let initialized = self.state.payload.set(Mutex::new(Some(PendingPayload {
            owner,
            io,
            _resources: resources,
        })));
        assert!(
            initialized.is_ok(),
            "a reservation publishes its payload once"
        );
        if self.state.helper_admission_blocker {
            PENDING_COUNT.fetch_add(1, Ordering::SeqCst);
        }
        self.state.phase.store(PENDING, Ordering::Release);
        self.published = true;
        let cleanup = BoundedHelperCleanup {
            identity,
            process_id,
            state: self.state.clone(),
        };
        eprintln!("native launch cleanup incomplete: {cleanup:?}");
        BoundedHelperFailure {
            cause,
            cleanup: Some(cleanup),
            blockers: Vec::new(),
        }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.published {
            self.state.phase.store(COMPLETE, Ordering::Release);
        }
    }
}
