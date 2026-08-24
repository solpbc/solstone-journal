// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The sole owner of resolver lock entry.
//!
//! A resolver holds its complete ordered day set, may briefly enter topology,
//! and only then enters the registry.  Keeping registry entry here makes the
//! no-global-with-registry rule executable rather than conventional.

use std::os::fd::OwnedFd;
use std::time::Duration;

use crate::claim::{ClaimView, mechanical_finalize};
use crate::error::ConvergenceError;
use crate::init::{StoreDirs, open_store_dirs};
use crate::layout::DayKey;
use crate::lock::{
    DayLockSet, LOCK_TIMEOUT, RegistryGuard, acquire_days_with_timeout, hold_registry_with_timeout,
};
use crate::preflight::Admitted;
use crate::store::{ConvergenceStore, LoadDay};

/// Live registry section.  It is intentionally constructible only here.
pub(crate) struct RegistrySection<'a> {
    _guard: RegistryGuard,
    dirs: &'a StoreDirs,
}

impl<'a> RegistrySection<'a> {
    pub(crate) fn registry(&self) -> &'a OwnedFd {
        &self.dirs.registry
    }
}

fn enter_registry_with_timeout(
    dirs: &StoreDirs,
    timeout: Duration,
) -> Result<RegistrySection<'_>, ConvergenceError> {
    let guard = hold_registry_with_timeout(dirs, timeout)?;
    Ok(RegistrySection {
        _guard: guard,
        dirs,
    })
}

/// Execute a bounded registry operation before any day lock has been taken.
/// This is used only by owner preparation.
pub(crate) fn with_registry_only<T>(
    dirs: &StoreDirs,
    operation: impl FnOnce(&RegistrySection<'_>) -> Result<T, ConvergenceError>,
) -> Result<T, ConvergenceError> {
    let section = enter_registry_with_timeout(dirs, LOCK_TIMEOUT)?;
    operation(&section)
}

/// A complete live day lease plus the descriptor set it was acquired from.
/// It deliberately exposes no registry guard and no topology guard.
pub(crate) struct ResolverAccess<'a> {
    admitted: &'a Admitted,
    dirs: StoreDirs,
    locks: DayLockSet,
}

impl<'a> ResolverAccess<'a> {
    pub(crate) fn acquire(admitted: &'a Admitted) -> Result<Self, ConvergenceError> {
        admitted.store.revalidate()?;
        let dirs = open_store_dirs(admitted.store.root())?.ok_or(
            crate::error::ConvergenceError::Refused(crate::error::Refusal::Uninitialized),
        )?;
        let locks = acquire_days_with_timeout(
            &dirs,
            admitted.days(),
            admitted.store.journal_id(),
            admitted.store.root_id(),
            admitted.store.object_identity(),
            admitted.lock_timeout(),
        )?;
        Ok(Self {
            admitted,
            dirs,
            locks,
        })
    }

    pub(crate) fn store(&self) -> &ConvergenceStore {
        &self.admitted.store
    }

    pub(crate) fn dirs(&self) -> &StoreDirs {
        &self.dirs
    }

    pub(crate) fn locks(&self) -> &DayLockSet {
        &self.locks
    }

    pub(crate) fn days(&self) -> &[DayKey] {
        self.admitted.days()
    }

    /// A registry operation can occur only while this access owns its day
    /// lease.  Topology is never retained by this type, so it cannot overlap.
    pub(crate) fn with_registry<T>(
        &self,
        operation: impl FnOnce(&RegistrySection<'_>) -> Result<T, ConvergenceError>,
    ) -> Result<T, ConvergenceError> {
        let section = enter_registry_with_timeout(&self.dirs, self.admitted.lock_timeout())?;
        operation(&section)
    }

    pub(crate) fn load_day(&self, day: &DayKey) -> Result<LoadDay, ConvergenceError> {
        self.admitted.store.load_day(&self.locks, day)
    }

    /// Mechanically publish the unique next claim head while the complete day
    /// set is held.  The global section ends before a caller may enter the
    /// registry, making the ordering available as one executable primitive.
    pub(crate) fn finalize_claim_head(&self) -> Result<ClaimView, ConvergenceError> {
        let topology =
            crate::lock::hold_topology_with_timeout(&self.dirs, self.admitted.lock_timeout())?;
        let view = mechanical_finalize(self.store(), &self.dirs)?;
        drop(topology);
        Ok(view)
    }
}

pub(crate) fn with_registry<T>(
    dirs: &StoreDirs,
    timeout: Duration,
    operation: impl FnOnce(&RegistrySection<'_>) -> Result<T, ConvergenceError>,
) -> Result<T, ConvergenceError> {
    let section = enter_registry_with_timeout(dirs, timeout)?;
    operation(&section)
}

#[cfg(test)]
pub(crate) fn hold_registry_for_test(
    dirs: &StoreDirs,
    timeout: Duration,
) -> Result<RegistrySection<'_>, ConvergenceError> {
    enter_registry_with_timeout(dirs, timeout)
}

#[cfg(test)]
mod observer {
    use std::cell::RefCell;

    #[derive(Default)]
    struct State {
        enabled: bool,
        days: usize,
        topology: usize,
        registry: usize,
        trace: Vec<&'static str>,
    }

    thread_local! {
        static STATE: RefCell<State> = RefCell::new(State::default());
    }

    #[derive(Clone, Copy)]
    pub(crate) enum Kind {
        Day,
        Topology,
        Registry,
    }

    pub(crate) struct Token {
        kind: Kind,
        enabled: bool,
        thread: std::thread::ThreadId,
    }

    impl Token {
        pub(crate) fn new(kind: Kind) -> Self {
            acquire(kind)
        }
    }

    pub(crate) fn initialize() {
        STATE.with(|state| {
            *state.borrow_mut() = State {
                enabled: true,
                ..State::default()
            };
        });
    }

    pub(crate) fn acquire(kind: Kind) -> Token {
        let enabled = STATE.with(|state| {
            let mut state = state.borrow_mut();
            if !state.enabled {
                return false;
            }
            match kind {
                Kind::Day if state.registry != 0 => panic!("day acquisition while registry held"),
                Kind::Topology if state.registry != 0 => {
                    panic!("topology acquisition while registry held")
                }
                Kind::Registry if state.topology != 0 => panic!("registry/global overlap"),
                _ => {}
            }
            match kind {
                Kind::Day => {
                    state.days += 1;
                    state.trace.push("day");
                }
                Kind::Topology => {
                    state.topology += 1;
                    state.trace.push("topology");
                }
                Kind::Registry => {
                    state.registry += 1;
                    state.trace.push("registry");
                }
            }
            true
        });
        Token {
            kind,
            enabled,
            thread: std::thread::current().id(),
        }
    }

    impl Drop for Token {
        fn drop(&mut self) {
            if !self.enabled || self.thread != std::thread::current().id() {
                return;
            }
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                match self.kind {
                    Kind::Day => state.days -= 1,
                    Kind::Topology => state.topology -= 1,
                    Kind::Registry => state.registry -= 1,
                }
            });
        }
    }

    pub(crate) fn trace() -> Vec<&'static str> {
        STATE.with(|state| state.borrow().trace.clone())
    }
}

#[cfg(test)]
pub(crate) use observer::{Kind as ObservedLock, Token as LockObserverToken};

#[cfg(test)]
pub(crate) fn initialize_lock_trace() {
    observer::initialize();
}

#[cfg(test)]
pub(crate) fn lock_trace() -> Vec<&'static str> {
    observer::trace()
}

#[cfg(test)]
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use crate::claim::mechanical_finalize;
    use crate::lock::{acquire_days_with_timeout, hold_topology_with_timeout};
    use crate::test_support::{admit_days, continue_ok};

    fn trace_claim_path(admitted: &Admitted) {
        initialize_lock_trace();
        let access = ResolverAccess::acquire(admitted).unwrap();
        {
            let topology =
                hold_topology_with_timeout(access.dirs(), admitted.lock_timeout()).unwrap();
            let _view = mechanical_finalize(access.store(), access.dirs()).unwrap();
            drop(topology);
        }
        access.with_registry(|_| Ok(())).unwrap();
        assert_eq!(lock_trace(), vec!["day", "topology", "registry"]);
    }

    #[test]
    fn normal_day_global_registry_trace_is_ordered() {
        let (_temporary, admitted) = admit_days("access-trace", &["20260823"]);
        initialize_lock_trace();
        let access = ResolverAccess::acquire(&admitted).unwrap();
        {
            let topology =
                hold_topology_with_timeout(access.dirs(), admitted.lock_timeout()).unwrap();
            drop(topology);
        }
        access.with_registry(|_| Ok(())).unwrap();
        assert_eq!(lock_trace(), vec!["day", "topology", "registry"]);
    }

    #[test]
    fn registry_only_trace_has_no_day_or_global() {
        let (_temporary, admitted) = admit_days("access-registry-only", &["20260823"]);
        let dirs = open_store_dirs(admitted.store().root()).unwrap().unwrap();
        initialize_lock_trace();
        with_registry_only(&dirs, |_| Ok(())).unwrap();
        assert_eq!(lock_trace(), vec!["registry"]);
    }

    #[test]
    fn claim_read_path_releases_global_before_registry() {
        let (_temporary, admitted) = admit_days("access-claim-read", &["20260823"]);
        trace_claim_path(&admitted);
    }

    #[test]
    fn claim_head_recovery_path_releases_global_before_registry() {
        let (_temporary, admitted) = admit_days("access-claim-head", &["20260823"]);
        let held = continue_ok(&admitted);
        drop(held);
        trace_claim_path(&admitted);
    }

    #[test]
    fn observer_rejects_registry_global_overlap() {
        let (_temporary, admitted) = admit_days("access-overlap", &["20260823"]);
        initialize_lock_trace();
        let dirs = open_store_dirs(admitted.store().root()).unwrap().unwrap();
        let topology = hold_topology_with_timeout(&dirs, admitted.lock_timeout()).unwrap();
        let result =
            std::panic::catch_unwind(|| with_registry(&dirs, admitted.lock_timeout(), |_| Ok(())));
        drop(topology);
        assert!(result.is_err());
    }

    #[test]
    fn observer_rejects_day_wait_while_registry_held() {
        let (_temporary, admitted) = admit_days("access-day", &["20260823"]);
        initialize_lock_trace();
        let dirs = open_store_dirs(admitted.store().root()).unwrap().unwrap();
        let section = enter_registry_with_timeout(&dirs, admitted.lock_timeout()).unwrap();
        let result = std::panic::catch_unwind(|| {
            acquire_days_with_timeout(
                &dirs,
                admitted.days(),
                admitted.store().journal_id(),
                admitted.store().root_id(),
                admitted.store().object_identity(),
                admitted.lock_timeout(),
            )
        });
        drop(section);
        assert!(result.is_err());
    }

    #[test]
    fn observer_trace_is_thread_local() {
        initialize_lock_trace();
        let parent = lock_trace();
        let child = std::thread::spawn(|| {
            initialize_lock_trace();
            let token = LockObserverToken::new(ObservedLock::Day);
            let trace = lock_trace();
            drop(token);
            trace
        })
        .join()
        .unwrap();
        assert_eq!(parent, lock_trace());
        assert_eq!(child, vec!["day"]);
    }
}
