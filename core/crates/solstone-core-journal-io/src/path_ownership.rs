// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Which thread holds which path's canonical lock, and an in-process gate for
//! writers that may not create a sidecar beside their target.
//!
//! [`crate::hold_lock`] is not reentrant: its advisory lock belongs to an open
//! file, so a second acquisition on the same thread waits for the first and
//! times out. Callers that already hold a path's lock and then append to it, or
//! read it through a healing reader, would deadlock on themselves. Exclusive
//! sidecar locks therefore record their holder here, and the canonical JSONL
//! writers and healers reuse a lock this thread already holds instead of taking
//! it again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};
use std::thread::{self, ThreadId};

#[derive(Debug, Clone, Copy)]
struct Holder {
    thread: ThreadId,
    token: u64,
}

fn owners() -> MutexGuard<'static, HashMap<PathBuf, Holder>> {
    static OWNERS: OnceLock<Mutex<HashMap<PathBuf, Holder>>> = OnceLock::new();
    OWNERS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The key two spellings of one path share: absolute, with `.` components gone.
pub(crate) fn ownership_key(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// A registered exclusive hold; dropping it forgets the holder.
#[derive(Debug)]
pub(crate) struct OwnershipToken {
    key: PathBuf,
    token: u64,
}

impl Drop for OwnershipToken {
    fn drop(&mut self) {
        let mut owners = owners();
        if owners
            .get(&self.key)
            .is_some_and(|holder| holder.token == self.token)
        {
            owners.remove(&self.key);
        }
    }
}

/// Record that the current thread holds `path`'s exclusive canonical lock.
pub(crate) fn register_exclusive(path: &Path) -> OwnershipToken {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let token = NEXT.fetch_add(1, Ordering::Relaxed);
    let key = ownership_key(path);
    owners().insert(
        key.clone(),
        Holder {
            thread: thread::current().id(),
            token,
        },
    );
    OwnershipToken { key, token }
}

/// Whether the current thread already holds `path`'s exclusive canonical lock.
pub fn current_thread_holds_lock(path: impl AsRef<Path>) -> bool {
    let key = ownership_key(path.as_ref());
    owners()
        .get(&key)
        .is_some_and(|holder| holder.thread == thread::current().id())
}

#[derive(Default)]
struct GateState {
    held: HashMap<PathBuf, (ThreadId, usize)>,
}

fn gate() -> &'static (Mutex<GateState>, Condvar) {
    static GATE: OnceLock<(Mutex<GateState>, Condvar)> = OnceLock::new();
    GATE.get_or_init(Default::default)
}

/// Exclusive use of one path among this process's threads. Reentrant on the
/// thread that holds it; dropping the outermost guard admits the next waiter.
#[derive(Debug)]
pub struct PathGateGuard {
    key: PathBuf,
}

impl Drop for PathGateGuard {
    fn drop(&mut self) {
        let (state, released) = gate();
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((_, depth)) = state.held.get_mut(&self.key) {
            *depth -= 1;
            if *depth == 0 {
                state.held.remove(&self.key);
                released.notify_all();
            }
        }
    }
}

/// Serialize this process's writers of `path` without creating anything on disk.
///
/// For writers whose target sits where a lock sidecar may not go (a segment
/// directory, a census-bounded namespace) and that no durable reader rewrites,
/// so the only coordination they need is among their own threads.
pub fn hold_path_gate(path: impl AsRef<Path>) -> PathGateGuard {
    let key = ownership_key(path.as_ref());
    let me = thread::current().id();
    let (state, released) = gate();
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    loop {
        match state.held.get_mut(&key) {
            None => {
                state.held.insert(key.clone(), (me, 1));
                return PathGateGuard { key };
            }
            Some((holder, depth)) if *holder == me => {
                *depth += 1;
                return PathGateGuard { key };
            }
            Some(_) => {
                state = released
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_names_only_the_holding_thread_and_ends_with_the_token() {
        let path = Path::new("/journal/./awareness/20260101.jsonl");
        assert!(!current_thread_holds_lock(path));
        let token = register_exclusive(path);
        assert!(current_thread_holds_lock(
            "/journal/awareness/20260101.jsonl"
        ));
        let other =
            thread::spawn(|| current_thread_holds_lock("/journal/awareness/20260101.jsonl"))
                .join()
                .unwrap();
        assert!(!other);
        drop(token);
        assert!(!current_thread_holds_lock(path));
    }

    #[test]
    fn a_stale_token_does_not_forget_a_newer_holder() {
        let path = Path::new("/journal/stale-token.jsonl");
        let first = register_exclusive(path);
        let second = register_exclusive(path);
        drop(first);
        assert!(current_thread_holds_lock(path));
        drop(second);
        assert!(!current_thread_holds_lock(path));
    }

    #[test]
    fn the_gate_is_reentrant_on_its_thread() {
        let path = PathBuf::from("/journal/reentrant-gate.jsonl");
        let outer = hold_path_gate(&path);
        let inner = hold_path_gate("/journal/./reentrant-gate.jsonl");
        drop(outer);
        drop(inner);
        let _again = hold_path_gate(&path);
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn the_gate_excludes_other_threads_until_the_outermost_guard_drops() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicUsize;
        use std::time::Duration;

        let path = PathBuf::from("/journal/gate.jsonl");
        let outer = hold_path_gate(&path);
        let inner = hold_path_gate(&path);
        let entered = Arc::new(AtomicUsize::new(0));
        let waiter = {
            let entered = Arc::clone(&entered);
            let path = path.clone();
            thread::spawn(move || {
                let _guard = hold_path_gate(&path);
                entered.fetch_add(1, Ordering::SeqCst);
            })
        };
        drop(inner);
        thread::sleep(Duration::from_millis(20));
        assert_eq!(entered.load(Ordering::SeqCst), 0);
        drop(outer);
        waiter.join().unwrap();
        assert_eq!(entered.load(Ordering::SeqCst), 1);
    }
}
