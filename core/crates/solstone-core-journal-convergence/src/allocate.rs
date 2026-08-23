// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsStr;

use crate::error::{ConvergenceError, DurableRole, Refusal, map_root_error, random_hex};
use crate::init::{load_allocator, open_store_dirs};
use crate::layout::{DayKey, adoption_name};
use crate::lock::{AllocationProof, DayLockSet, hold_topology};
use crate::schema::{
    Adoption, SCHEMA_VERSION, now_rfc3339, read_json, replace_json, write_json_exclusive,
};
use crate::store::ConvergenceStore;

impl ConvergenceStore {
    /// Issue a serial under a brief topology lock. Adoption is created first.
    pub fn allocate(&self, days: &DayLockSet) -> Result<AllocationProof, ConvergenceError> {
        self.revalidate()?;
        days.matches(self.journal_id(), self.root_id(), self.object_identity())?;
        let dirs = open_store_dirs(self.root())?
            .ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
        let topology = hold_topology(&dirs)?;
        self.root().revalidate().map_err(map_root_error)?;
        for day in days.days() {
            ensure_adoption(self, &dirs, day)?;
        }
        let mut allocator = load_allocator(&dirs)?;
        if allocator.journal_id != self.journal_id() || allocator.root_id != self.root_id() {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::Allocator,
            });
        }
        if allocator.exhausted || allocator.next_serial == u64::MAX {
            allocator.exhausted = true;
            let _ = replace_json(
                &dirs.convergence,
                OsStr::new(crate::layout::ALLOCATOR),
                &allocator,
            );
            return Err(ConvergenceError::Refused(Refusal::Exhausted));
        }
        let serial = allocator.next_serial;
        allocator.next_serial = allocator
            .next_serial
            .checked_add(1)
            .ok_or(ConvergenceError::Refused(Refusal::Exhausted))?;
        replace_json(
            &dirs.convergence,
            OsStr::new(crate::layout::ALLOCATOR),
            &allocator,
        )?;
        drop(topology);
        Ok(AllocationProof::new(serial, days))
    }
}

fn ensure_adoption(
    store: &ConvergenceStore,
    dirs: &crate::init::StoreDirs,
    day: &DayKey,
) -> Result<Adoption, ConvergenceError> {
    let name = adoption_name(day);
    if let Some(existing) = read_json::<Adoption>(&dirs.days, &name, DurableRole::Adoption)? {
        crate::schema::require_ids(
            store.journal_id(),
            store.root_id(),
            &existing.journal_id,
            &existing.root_id,
        )?;
        crate::schema::require_day(day, &existing.day)?;
        return Ok(existing);
    }
    let adoption = Adoption {
        schema_version: SCHEMA_VERSION,
        journal_id: store.journal_id().to_owned(),
        root_id: store.root_id().to_owned(),
        adoption_id: random_hex()?,
        day: day.as_str().to_owned(),
        auxiliary_time: now_rfc3339(),
    };
    write_json_exclusive(&dirs.days, &name, &adoption, DurableRole::Adoption)?;
    Ok(adoption)
}

pub(crate) fn load_adoption(
    dirs: &crate::init::StoreDirs,
    day: &DayKey,
) -> Result<Option<Adoption>, ConvergenceError> {
    read_json(&dirs.days, &adoption_name(day), DurableRole::Adoption)
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use crate::layout::DayKey;
    use crate::test_support::initialized_store;

    #[test]
    fn allocate_releases_topology_before_return() {
        let (_temporary, store) = initialized_store();
        let day = DayKey::parse("20260823").unwrap();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        let _proof = store.allocate(&locks).unwrap();
        let other = DayKey::parse("20260824").unwrap();
        store.acquire_days(&[other]).unwrap();
    }

    #[test]
    fn allocator_carries_journal_root_ids() {
        let (_temporary, store) = initialized_store();
        let dirs = open_store_dirs(store.root()).unwrap().unwrap();
        let allocator = load_allocator(&dirs).unwrap();
        assert_eq!(allocator.journal_id, store.journal_id());
        assert_eq!(allocator.root_id, store.root_id());
        assert_eq!(allocator.next_serial, 1);
    }

    #[test]
    fn intervening_advance_refuses_stale_proof() {
        let (_temporary, store) = initialized_store();
        let day = DayKey::parse("20260823").unwrap();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        let first = store.allocate(&locks).unwrap();
        let _second = store.allocate(&locks).unwrap();
        let proposal = store
            .propose(&locks, &day, crate::OrdinaryIntent::AdvanceDirty)
            .unwrap();
        let mut authority = crate::OrdinaryAuthority::bind(proposal, first).unwrap();
        let error = store.publish(&locks, &day, &mut authority).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::InterveningAdvance)
        ));
    }

    #[test]
    fn exhaustion_is_refused() {
        let (_temporary, store) = initialized_store();
        let dirs = open_store_dirs(store.root()).unwrap().unwrap();
        let mut allocator = load_allocator(&dirs).unwrap();
        allocator.next_serial = u64::MAX;
        allocator.exhausted = false;
        replace_json(
            &dirs.convergence,
            OsStr::new(crate::layout::ALLOCATOR),
            &allocator,
        )
        .unwrap();
        let day = DayKey::parse("20260823").unwrap();
        let locks = store.acquire_days(&[day]).unwrap();
        let error = store.allocate(&locks).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::Exhausted)
        ));
    }

    #[test]
    fn unissued_serial_recurs_after_crash_before_allocator_write() {
        let (temporary, store) = initialized_store();
        let allocator_path = temporary
            .journal_path()
            .join("health/convergence/allocator.json");
        let genesis = std::fs::read(&allocator_path).unwrap();
        let day = DayKey::parse("20260823").unwrap();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        let _first = store.allocate(&locks).unwrap();
        std::fs::write(&allocator_path, genesis).unwrap();
        let again = store.allocate(&locks).unwrap();
        assert_eq!(again.serial(), 1);
    }

    #[test]
    fn issued_serial_advances_after_allocator_write() {
        let (_temporary, store) = initialized_store();
        let day = DayKey::parse("20260823").unwrap();
        let locks = store.acquire_days(&[day]).unwrap();
        let first = store.allocate(&locks).unwrap();
        let second = store.allocate(&locks).unwrap();
        assert_eq!(first.serial(), 1);
        assert_eq!(second.serial(), 2);
    }

    #[test]
    fn grafted_lineage_is_unknown() {
        let (temporary, store) = initialized_store();
        let day = DayKey::parse("20260823").unwrap();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        store.allocate(&locks).unwrap();
        let path = temporary
            .journal_path()
            .join("health/convergence/days/20260823.adopt.json");
        let mut adoption: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        adoption["journal_id"] = serde_json::Value::String("other".into());
        std::fs::write(&path, serde_json::to_vec(&adoption).unwrap()).unwrap();
        assert!(matches!(
            store.load_day(&locks, &day).unwrap_err(),
            ConvergenceError::Unknown { .. }
        ));
    }
}
