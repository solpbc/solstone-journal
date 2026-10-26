// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Stopping intent for one import attempt.
//!
//! The flag is process-wide only through an `Arc`. Each worker thread installs
//! its own guard. A thread with no guard is not stopped, so callers that never
//! install one keep the previous behavior.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

thread_local! {
    static ACTIVE: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

/// Shared stopping intent. `request` does not wait for native work to return.
#[derive(Clone, Debug)]
pub struct WorkStop {
    flag: Arc<AtomicBool>,
}

impl WorkStop {
    #[must_use]
    pub fn new() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn request(&self) {
        self.flag.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_requested(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }
}

impl Default for WorkStop {
    fn default() -> Self {
        Self::new()
    }
}

/// Installs `stop` on this thread until the guard drops.
pub struct WorkStopGuard {
    previous: Option<Arc<AtomicBool>>,
}

impl WorkStopGuard {
    #[must_use]
    pub fn install(stop: &WorkStop) -> Self {
        let previous = ACTIVE.with(|slot| slot.replace(Some(Arc::clone(&stop.flag))));
        Self { previous }
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
            .is_none_or(|flag| !flag.load(Ordering::Acquire))
    })
}
