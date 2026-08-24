// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::time::Duration;

use solstone_core_journal_io::{
    BoundParentLock, ObjectIdentity, acquire_existing_parent_lock_bound,
};

use crate::error::{ConvergenceError, DurableRole, Refusal, random_hex};
use crate::init::StoreDirs;
use crate::layout::{DayKey, REGISTRY_LOCK, TOPOLOGY_LOCK, day_lock_name, require_nonempty_unique};

pub(crate) const LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const LOCK_POLL: Duration = Duration::from_millis(20);

pub(crate) struct TopologyGuard {
    _lock: BoundParentLock,
    #[cfg(test)]
    _observer: crate::access::LockObserverToken,
}

#[allow(dead_code)]
pub(crate) fn hold_topology(dirs: &StoreDirs) -> Result<TopologyGuard, ConvergenceError> {
    hold_topology_with_timeout(dirs, LOCK_TIMEOUT)
}

pub(crate) fn hold_topology_with_timeout(
    dirs: &StoreDirs,
    timeout: Duration,
) -> Result<TopologyGuard, ConvergenceError> {
    let guard = acquire_existing_parent_lock_bound(
        &dirs.convergence,
        OsStr::new(TOPOLOGY_LOCK),
        timeout,
        LOCK_POLL,
    )
    .map_err(|error| map_lock_error("acquire topology lock", DurableRole::TopologyLock, error))?;
    Ok(TopologyGuard {
        _lock: guard,
        #[cfg(test)]
        _observer: crate::access::LockObserverToken::new(crate::access::ObservedLock::Topology),
    })
}

pub(crate) struct RegistryGuard {
    _lock: BoundParentLock,
    #[cfg(test)]
    _observer: crate::access::LockObserverToken,
}

pub(super) fn hold_registry_with_timeout(
    dirs: &StoreDirs,
    timeout: Duration,
) -> Result<RegistryGuard, ConvergenceError> {
    let guard = acquire_existing_parent_lock_bound(
        &dirs.convergence,
        OsStr::new(REGISTRY_LOCK),
        timeout,
        LOCK_POLL,
    )
    .map_err(|error| map_lock_error("acquire registry lock", DurableRole::RegistryLock, error))?;
    Ok(RegistryGuard {
        _lock: guard,
        #[cfg(test)]
        _observer: crate::access::LockObserverToken::new(crate::access::ObservedLock::Registry),
    })
}

fn map_lock_error(
    operation: &'static str,
    role: DurableRole,
    error: solstone_core_journal_io::ExistingParentLockError,
) -> ConvergenceError {
    if matches!(
        error,
        solstone_core_journal_io::ExistingParentLockError::Timeout(_)
    ) {
        return ConvergenceError::Refused(Refusal::Busy);
    }
    ConvergenceError::Io {
        operation,
        role,
        source: std::io::Error::other(error.to_string()),
    }
}

/// Opaque ordered day-lock-set proof. Not `Clone`. Drop releases flocks.
pub struct DayLockSet {
    _locks: Vec<BoundParentLock>,
    days: BTreeSet<DayKey>,
    journal_id: String,
    root_id: String,
    object_identity: ObjectIdentity,
    instance: String,
    #[cfg(test)]
    _observer: crate::access::LockObserverToken,
}

impl DayLockSet {
    #[allow(dead_code)]
    pub fn days(&self) -> &BTreeSet<DayKey> {
        &self.days
    }

    pub(crate) fn instance(&self) -> &str {
        &self.instance
    }

    #[allow(dead_code)]
    pub(crate) fn journal_id(&self) -> &str {
        &self.journal_id
    }

    #[allow(dead_code)]
    pub(crate) fn root_id(&self) -> &str {
        &self.root_id
    }

    pub(crate) fn contains(&self, day: &DayKey) -> bool {
        self.days.contains(day)
    }

    pub(crate) fn matches(
        &self,
        journal_id: &str,
        root_id: &str,
        identity: ObjectIdentity,
    ) -> Result<(), ConvergenceError> {
        if self.journal_id != journal_id || self.root_id != root_id {
            return Err(ConvergenceError::Refused(Refusal::WrongLineage));
        }
        if self.object_identity != identity {
            return Err(ConvergenceError::Changed {
                what: crate::error::ChangedWhat::Root,
            });
        }
        Ok(())
    }
}

#[allow(dead_code)]
pub(crate) fn acquire_days(
    dirs: &StoreDirs,
    days: &[DayKey],
    journal_id: &str,
    root_id: &str,
    object_identity: ObjectIdentity,
) -> Result<DayLockSet, ConvergenceError> {
    acquire_days_with_timeout(
        dirs,
        days,
        journal_id,
        root_id,
        object_identity,
        LOCK_TIMEOUT,
    )
}

pub(crate) fn acquire_days_with_timeout(
    dirs: &StoreDirs,
    days: &[DayKey],
    journal_id: &str,
    root_id: &str,
    object_identity: ObjectIdentity,
    timeout: Duration,
) -> Result<DayLockSet, ConvergenceError> {
    require_nonempty_unique(days)?;
    let mut ordered = days.to_vec();
    ordered.sort();
    let mut locks = Vec::with_capacity(ordered.len());
    #[cfg(test)]
    let observer = crate::access::LockObserverToken::new(crate::access::ObservedLock::Day);
    for day in &ordered {
        let name = day_lock_name(day);
        let guard = acquire_existing_parent_lock_bound(&dirs.days, &name, timeout, LOCK_POLL)
            .map_err(|error| map_lock_error("acquire day lock", DurableRole::DayLock, error))?;
        locks.push(guard);
    }
    Ok(DayLockSet {
        _locks: locks,
        days: ordered.into_iter().collect(),
        journal_id: journal_id.to_owned(),
        root_id: root_id.to_owned(),
        object_identity,
        instance: random_hex()?,
        #[cfg(test)]
        _observer: observer,
    })
}

#[cfg(test)]
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::thread;
    use std::time::{Duration, Instant};

    use crate::error::{ConvergenceError, Refusal};
    use crate::init::open_store_dirs;
    use crate::layout::DayKey;
    use crate::test_support::initialized_store;

    #[test]
    fn lock_files_never_unlinked_on_drop() {
        let (temporary, store) = initialized_store();
        let day = DayKey::parse("20260823").unwrap();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        let path = temporary
            .journal_path()
            .join("health/convergence/days/20260823.lock");
        assert!(path.exists());
        drop(locks);
        assert!(path.exists());
    }

    #[test]
    fn busy_a_does_not_block_disjoint_b() {
        let (temporary, store_a) = initialized_store();
        let root_b =
            solstone_core_journal_io::JournalRoot::open(&temporary.journal_path()).unwrap();
        let store_b = crate::store::ConvergenceStore::open(root_b).unwrap();
        let day_a = DayKey::parse("20260823").unwrap();
        let day_b = DayKey::parse("20260824").unwrap();
        let held = store_a.acquire_days(&[day_a]).unwrap();
        let started = Instant::now();
        let other = thread::spawn(move || store_b.acquire_days(&[day_b]));
        let got = other.join().expect("thread");
        assert!(got.is_ok());
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(held);
    }

    #[test]
    fn two_descriptors_contend_on_same_day_lock() {
        let (temporary, store_a) = initialized_store();
        let root_b =
            solstone_core_journal_io::JournalRoot::open(&temporary.journal_path()).unwrap();
        let store_b = crate::store::ConvergenceStore::open(root_b).unwrap();
        let day = DayKey::parse("20260823").unwrap();
        let held = store_a.acquire_days(std::slice::from_ref(&day)).unwrap();
        let started = Instant::now();
        let result = store_b.acquire_days(&[day]);
        assert!(result.is_err());
        assert!(started.elapsed() >= Duration::from_millis(50));
        drop(held);
    }

    #[test]
    fn proof_bound_to_lock_set_instance() {
        let (_temporary, admitted) = crate::test_support::admit_days("proof-inst", &["20260823"]);
        let owner = crate::test_support::prepared_owner(&admitted).unwrap();
        let held = admitted.begin(owner).unwrap();
        let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
        assert_eq!(proof.instance(), held.lock_set().instance());
    }

    #[test]
    fn proof_refused_after_relock_same_days() {
        let (_temporary, admitted) = crate::test_support::admit_days("stale-proof", &["20260823"]);
        let owner = crate::test_support::prepared_owner(&admitted).unwrap();
        let held = admitted.begin(owner).unwrap();
        let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
        drop(held);
        let owner = crate::test_support::prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let error = held.continue_with(proof).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::StaleLease)
        ));
    }

    #[test]
    fn allocate_requires_live_day_lock_set() {
        let (_temporary_a, admitted_a) = crate::test_support::admit_days("live-a", &["20260823"]);
        let (_temporary_b, admitted_b) = crate::test_support::admit_days("live-b", &["20260823"]);
        let owner_a = crate::test_support::prepared_owner(&admitted_a).unwrap();
        let error = admitted_b.begin(owner_a).unwrap_err();
        assert!(
            matches!(
                error,
                ConvergenceError::Refused(Refusal::WrongLineage) | ConvergenceError::Changed { .. }
            ),
            "{error:?}"
        );
    }

    #[test]
    fn proof_consumed_by_bind_cannot_bind_twice() {
        let (_temporary, admitted) =
            crate::test_support::admit_days("consume-twice", &["20260823"]);
        let owner = crate::test_support::prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let mut proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
        proof.consume().unwrap();
        let error = held.continue_with(proof).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::ReusedAuthority)
        ));
    }

    #[test]
    fn registry_lock_contention_is_busy() {
        let (temporary, store_a) = initialized_store();
        let root_b =
            solstone_core_journal_io::JournalRoot::open(&temporary.journal_path()).unwrap();
        let store_b = crate::store::ConvergenceStore::open(root_b).unwrap();
        let dirs_a = open_store_dirs(store_a.root()).unwrap().unwrap();
        let dirs_b = open_store_dirs(store_b.root()).unwrap().unwrap();
        let held = crate::access::hold_registry_for_test(&dirs_a, Duration::from_secs(2)).unwrap();
        let started = Instant::now();
        let result = crate::access::hold_registry_for_test(&dirs_b, Duration::from_millis(80));
        assert!(matches!(
            result,
            Err(ConvergenceError::Refused(Refusal::Busy))
        ));
        assert!(started.elapsed() >= Duration::from_millis(50));
        drop(held);
    }

    #[test]
    fn held_registry_does_not_block_disjoint_day() {
        let (temporary, store_a) = initialized_store();
        let root_b =
            solstone_core_journal_io::JournalRoot::open(&temporary.journal_path()).unwrap();
        let store_b = crate::store::ConvergenceStore::open(root_b).unwrap();
        let dirs_a = open_store_dirs(store_a.root()).unwrap().unwrap();
        let day = DayKey::parse("20260823").unwrap();
        let held = crate::access::hold_registry_for_test(&dirs_a, Duration::from_secs(2)).unwrap();
        let started = Instant::now();
        let other = thread::spawn(move || store_b.acquire_days(&[day]));
        let got = other.join().expect("thread");
        assert!(got.is_ok());
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(held);
    }

    #[test]
    fn held_day_does_not_block_registry() {
        let (temporary, store_a) = initialized_store();
        let root_b =
            solstone_core_journal_io::JournalRoot::open(&temporary.journal_path()).unwrap();
        let store_b = crate::store::ConvergenceStore::open(root_b).unwrap();
        let dirs_b = open_store_dirs(store_b.root()).unwrap().unwrap();
        let day = DayKey::parse("20260823").unwrap();
        let held = store_a.acquire_days(std::slice::from_ref(&day)).unwrap();
        let started = Instant::now();
        let other = thread::spawn(move || {
            crate::access::hold_registry_for_test(&dirs_b, Duration::from_millis(80)).map(|_| ())
        });
        let got = other.join().expect("thread");
        assert!(got.is_ok());
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(held);
    }
}
