// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Stopping intent for one import attempt.
//!
//! The flag is process-wide only through an `Arc`. Each worker thread installs
//! its own guard. A thread with no guard is not stopped, so callers that never
//! install one keep the previous behavior.
//!
//! [`WorkStopGuard`] restores thread-local state and is `!Send` and `!Sync`.
//! A spawned worker that installs or publishes does not read the entry
//! thread's local state. It holds the same [`WorkStop`] and checks that.
//! [`WorkStop::linked`] also observes a parent flag and a deadline. Those are
//! read when a check runs. Nothing waits on a timer.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

thread_local! {
    static ACTIVE: RefCell<Option<Arc<StopState>>> = const { RefCell::new(None) };
}

#[derive(Debug)]
struct StopState {
    flag: AtomicBool,
    parent: Option<Arc<AtomicBool>>,
    deadline: Option<Instant>,
}

fn stop_requested(state: &StopState) -> bool {
    state.flag.load(Ordering::Acquire)
        || state
            .parent
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Acquire))
        || state
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
}

/// Shared stopping intent. `request` does not wait for native work to return.
#[derive(Clone, Debug)]
pub struct WorkStop {
    state: Arc<StopState>,
}

impl WorkStop {
    #[must_use]
    pub fn new() -> Self {
        Self::from_state(StopState {
            flag: AtomicBool::new(false),
            parent: None,
            deadline: None,
        })
    }

    /// Observe a parent cancellation flag and an optional deadline.
    ///
    /// [`WorkStop::is_requested`] is true when this stop was requested, `parent`
    /// is set, or `deadline` is in the past. The deadline is read at each
    /// check. There is no timer thread.
    #[must_use]
    pub fn linked(parent: Arc<AtomicBool>, deadline: Option<Instant>) -> Self {
        Self::from_state(StopState {
            flag: AtomicBool::new(false),
            parent: Some(parent),
            deadline,
        })
    }

    fn from_state(state: StopState) -> Self {
        Self {
            state: Arc::new(state),
        }
    }

    pub fn request(&self) {
        self.state.flag.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_requested(&self) -> bool {
        stop_requested(&self.state)
    }
}

impl Default for WorkStop {
    fn default() -> Self {
        Self::new()
    }
}

/// Installs `stop` on this thread until the guard drops.
///
/// The guard is `!Send` and `!Sync`. Moving it would restore another thread's
/// local slot. Pass [`WorkStop`] itself into a worker that installs or publishes.
pub struct WorkStopGuard {
    previous: Option<Arc<StopState>>,
    _thread: PhantomData<*const ()>,
}

impl WorkStopGuard {
    #[must_use]
    pub fn install(stop: &WorkStop) -> Self {
        let previous = ACTIVE.with(|slot| slot.replace(Some(Arc::clone(&stop.state))));
        Self {
            previous,
            _thread: PhantomData,
        }
    }
}

impl Drop for WorkStopGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        ACTIVE.with(|slot| {
            slot.replace(previous);
        });
    }
}

/// Whether this thread may start another unit of import work.
///
/// A thread that has not installed a [`WorkStopGuard`] continues. Cancellation
/// is therefore invisible to code that runs outside an attempt.
#[must_use]
pub fn work_continues() -> bool {
    ACTIVE.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_none_or(|state| !stop_requested(state))
    })
}

/// The stop installed on this thread, if any.
///
/// Capture this on the thread that installed the guard, then move it into a
/// spawned worker. The worker checks [`WorkStop::is_requested`] rather than
/// this thread's local slot.
#[must_use]
pub fn installed_stop() -> Option<WorkStop> {
    ACTIVE.with(|slot| {
        slot.borrow().as_ref().map(|state| WorkStop {
            state: Arc::clone(state),
        })
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    fn assert_not_send<T>() {
        trait Probe<A> {
            fn probe() {}
        }
        impl<T: ?Sized> Probe<()> for T {}
        impl<T: ?Sized + Send> Probe<u8> for T {}
        let _ = <T as Probe<_>>::probe;
    }

    fn assert_not_sync<T>() {
        trait Probe<A> {
            fn probe() {}
        }
        impl<T: ?Sized> Probe<()> for T {}
        impl<T: ?Sized + Sync> Probe<u8> for T {}
        let _ = <T as Probe<_>>::probe;
    }

    #[test]
    fn guard_stays_on_its_thread_and_the_stop_can_move() {
        assert_send_sync::<WorkStop>();
        assert_not_send::<WorkStopGuard>();
        assert_not_sync::<WorkStopGuard>();
    }

    #[test]
    fn linked_parent_and_deadline_are_visible_at_the_check() {
        let parent = Arc::new(AtomicBool::new(false));
        let later = Instant::now() + Duration::from_secs(3_600);
        let stop = WorkStop::linked(Arc::clone(&parent), Some(later));
        assert!(!stop.is_requested());
        let _guard = WorkStopGuard::install(&stop);
        assert!(work_continues());
        parent.store(true, Ordering::Release);
        assert!(stop.is_requested());
        assert!(!work_continues());

        let expired = WorkStop::linked(
            Arc::new(AtomicBool::new(false)),
            Some(Instant::now() - Duration::from_secs(1)),
        );
        assert!(expired.is_requested());
        let _expired_guard = WorkStopGuard::install(&expired);
        assert!(!work_continues());
    }
}
