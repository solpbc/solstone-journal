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
use crate::layout::{DayKey, TOPOLOGY_LOCK, day_lock_name, validate_day_set};

const LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const LOCK_POLL: Duration = Duration::from_millis(20);

pub(crate) struct TopologyGuard {
    _lock: BoundParentLock,
}

pub(crate) fn hold_topology(dirs: &StoreDirs) -> Result<TopologyGuard, ConvergenceError> {
    let guard = acquire_existing_parent_lock_bound(
        &dirs.convergence,
        OsStr::new(TOPOLOGY_LOCK),
        LOCK_TIMEOUT,
        LOCK_POLL,
    )
    .map_err(|error| ConvergenceError::Io {
        operation: "acquire topology lock",
        role: DurableRole::TopologyLock,
        source: std::io::Error::other(error.to_string()),
    })?;
    Ok(TopologyGuard { _lock: guard })
}

/// Opaque ordered day-lock-set proof. Not `Clone`. Drop releases flocks.
pub struct DayLockSet {
    _locks: Vec<BoundParentLock>,
    days: BTreeSet<DayKey>,
    journal_id: String,
    root_id: String,
    object_identity: ObjectIdentity,
    instance: String,
}

impl DayLockSet {
    pub fn days(&self) -> &BTreeSet<DayKey> {
        &self.days
    }

    pub(crate) fn instance(&self) -> &str {
        &self.instance
    }

    pub(crate) fn journal_id(&self) -> &str {
        &self.journal_id
    }

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

/// Single-use allocation proof bound to a lock-set instance and issued serial.
#[derive(Debug)]
pub struct AllocationProof {
    serial: u64,
    days: BTreeSet<DayKey>,
    journal_id: String,
    root_id: String,
    instance: String,
}

impl AllocationProof {
    pub(crate) fn new(serial: u64, days: &DayLockSet) -> Self {
        Self {
            serial,
            days: days.days().clone(),
            journal_id: days.journal_id().to_owned(),
            root_id: days.root_id().to_owned(),
            instance: days.instance().to_owned(),
        }
    }

    pub(crate) fn serial(&self) -> u64 {
        self.serial
    }

    pub(crate) fn instance(&self) -> &str {
        &self.instance
    }

    pub(crate) fn days(&self) -> &BTreeSet<DayKey> {
        &self.days
    }

    pub(crate) fn journal_id(&self) -> &str {
        &self.journal_id
    }

    pub(crate) fn root_id(&self) -> &str {
        &self.root_id
    }
}

pub(crate) fn acquire_days(
    dirs: &StoreDirs,
    days: &[DayKey],
    journal_id: &str,
    root_id: &str,
    object_identity: ObjectIdentity,
) -> Result<DayLockSet, ConvergenceError> {
    validate_day_set(days)?;
    let mut locks = Vec::with_capacity(days.len());
    for day in days {
        let name = day_lock_name(day);
        let guard = acquire_existing_parent_lock_bound(&dirs.days, &name, LOCK_TIMEOUT, LOCK_POLL)
            .map_err(|error| ConvergenceError::Io {
                operation: "acquire day lock",
                role: DurableRole::DayLock,
                source: std::io::Error::other(error.to_string()),
            })?;
        locks.push(guard);
    }
    Ok(DayLockSet {
        _locks: locks,
        days: days.iter().cloned().collect(),
        journal_id: journal_id.to_owned(),
        root_id: root_id.to_owned(),
        object_identity,
        instance: random_hex()?,
    })
}

#[cfg(test)]
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::thread;
    use std::time::{Duration, Instant};

    use crate::error::{ConvergenceError, Refusal};
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
        let (_temporary, store) = initialized_store();
        let day = DayKey::parse("20260823").unwrap();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        let proof = store.allocate(&locks).unwrap();
        assert_eq!(proof.instance(), locks.instance());
    }

    #[test]
    fn proof_refused_after_relock_same_days() {
        let (_temporary, store) = initialized_store();
        let day = DayKey::parse("20260823").unwrap();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        let proof = store.allocate(&locks).unwrap();
        drop(locks);
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        let proposal = store
            .propose(&locks, &day, crate::OrdinaryIntent::AdvanceDirty)
            .unwrap();
        let error = crate::OrdinaryAuthority::bind(proposal, proof).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::StaleLease)
        ));
    }

    #[test]
    fn allocate_requires_live_day_lock_set() {
        let (temporary_a, store_a) = initialized_store();
        let (_temporary_b, store_b) = initialized_store();
        let day = DayKey::parse("20260823").unwrap();
        let locks = store_a.acquire_days(&[day]).unwrap();
        let error = store_b.allocate(&locks).unwrap_err();
        assert!(
            matches!(
                error,
                ConvergenceError::Refused(Refusal::WrongLineage) | ConvergenceError::Changed { .. }
            ),
            "{error:?}"
        );
        drop(temporary_a);
    }

    #[test]
    fn proof_consumed_by_bind_cannot_bind_twice() {
        let (_temporary, store) = initialized_store();
        let day = DayKey::parse("20260823").unwrap();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        let proof = store.allocate(&locks).unwrap();
        let proposal = store
            .propose(&locks, &day, crate::OrdinaryIntent::AdvanceDirty)
            .unwrap();
        let mut authority = crate::OrdinaryAuthority::bind(proposal, proof).unwrap();
        store.publish(&locks, &day, &mut authority).unwrap();
        let error = store.publish(&locks, &day, &mut authority).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::ReusedAuthority)
        ));
    }
}
