// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsStr;

use crate::error::{ConvergenceError, DurableRole, Refusal, random_hex};
use crate::init::load_allocator;
use crate::layout::{DayKey, adoption_name};
use crate::schema::{
    Adoption, SCHEMA_VERSION, now_rfc3339, read_json, replace_json, write_json_exclusive,
};
use crate::store::ConvergenceStore;

pub(crate) fn bump_serial(
    store: &ConvergenceStore,
    dirs: &crate::init::StoreDirs,
) -> Result<u64, ConvergenceError> {
    let mut allocator = load_allocator(dirs)?;
    if allocator.journal_id != store.journal_id() || allocator.root_id != store.root_id() {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::Allocator,
        });
    }
    if allocator.exhausted || allocator.next_serial == u64::MAX {
        allocator.exhausted = true;
        replace_json(
            &dirs.convergence,
            OsStr::new(crate::layout::ALLOCATOR),
            &allocator,
        )?;
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
    Ok(serial)
}

pub(crate) fn ensure_adoption(
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
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use crate::init::open_store_dirs;
    use crate::layout::DayKey;
    use crate::test_support::{admit_days, continue_ok, initialized_store};

    #[test]
    fn allocate_releases_topology_before_return() {
        let (temporary, admitted) = admit_days("alloc-release", &["20260823"]);
        let _held = continue_ok(&admitted);
        let root_b =
            solstone_core_journal_io::JournalRoot::open(&temporary.journal_path()).unwrap();
        let set_b = match crate::preflight::preflight(["20260824"]).unwrap() {
            crate::preflight::Preflight::Ready(set) => set,
            crate::preflight::Preflight::Empty => panic!("days"),
        };
        let admitted_b = set_b.admit(root_b).unwrap();
        let started = std::time::Instant::now();
        let _held_b = continue_ok(&admitted_b);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
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
    fn exhaustion_is_refused() {
        let (_temporary, admitted) = admit_days("exhaust", &["20260823"]);
        let dirs = open_store_dirs(admitted.store().root()).unwrap().unwrap();
        let mut allocator = load_allocator(&dirs).unwrap();
        allocator.next_serial = u64::MAX;
        allocator.exhausted = false;
        replace_json(
            &dirs.convergence,
            OsStr::new(crate::layout::ALLOCATOR),
            &allocator,
        )
        .unwrap();
        let owner = crate::owner::OwnerBinding::issue_from_base(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = crate::owner::ClaimAdmission::issue_from_base(&held, held.owner()).unwrap();
        let error = held.continue_with(proof).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::Exhausted)
        ));
    }

    #[test]
    fn unissued_serial_recurs_after_crash_before_allocator_write() {
        let (temporary, admitted) = admit_days("unissued", &["20260823"]);
        let allocator_path = temporary
            .journal_path()
            .join("health/convergence/allocator.json");
        let genesis = std::fs::read(&allocator_path).unwrap();
        let dirs = open_store_dirs(admitted.store().root()).unwrap().unwrap();
        let first = bump_serial(admitted.store(), &dirs).unwrap();
        assert_eq!(first, 1);
        std::fs::write(&allocator_path, genesis).unwrap();
        let again = bump_serial(admitted.store(), &dirs).unwrap();
        assert_eq!(again, 1);
    }

    #[test]
    fn issued_serial_advances_after_allocator_write() {
        let (_temporary, admitted) = admit_days("issued", &["20260823"]);
        let dirs = open_store_dirs(admitted.store().root()).unwrap().unwrap();
        let first = bump_serial(admitted.store(), &dirs).unwrap();
        let second = bump_serial(admitted.store(), &dirs).unwrap();
        assert_eq!(first, 1);
        assert_eq!(second, 2);
    }

    #[test]
    fn grafted_lineage_is_unknown() {
        let (temporary, admitted) = admit_days("graft-adopt", &["20260823"]);
        let held = continue_ok(&admitted);
        let path = temporary
            .journal_path()
            .join("health/convergence/days/20260823.adopt.json");
        let mut adoption: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        adoption["journal_id"] = serde_json::Value::String("other".into());
        std::fs::write(&path, serde_json::to_vec(&adoption).unwrap()).unwrap();
        assert!(matches!(
            held.inspect_day(&DayKey::parse("20260823").unwrap())
                .unwrap_err(),
            ConvergenceError::Unknown { .. }
        ));
    }
}
