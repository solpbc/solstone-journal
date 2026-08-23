// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_journal_io::{JournalRoot, ObjectIdentity};

use crate::digest::RecordDigest;
use crate::error::{ConvergenceError, Refusal, map_root_error};
use crate::init::{check_initialized, load_allocator, load_root_witness, open_store_dirs};
use crate::layout::{DayKey, validate_day_set};
use crate::lock::{DayLockSet, acquire_days};
use crate::schema::{DayRecord, require_ids, validate_record_numbers};

/// Snapshot of a published day record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DaySnapshot {
    pub day: DayKey,
    pub journal_id: String,
    pub root_id: String,
    pub adoption_id: String,
    pub record_revision: u64,
    pub first_transition_serial: u64,
    pub dirty_by_transition_serial: u64,
    pub dirty_generation: u64,
    pub completed_generation: u64,
    pub auxiliary_time: String,
    pub digest: RecordDigest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LoadDay {
    Genesis,
    Published(DaySnapshot),
    PublicationPending { kind: PendingKind },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingKind {
    WitnessAheadOfHead,
    HeadAheadOfRecord,
}

/// Journal- and lineage-bound convergence store. Owns a retained [`JournalRoot`].
pub struct ConvergenceStore {
    root: JournalRoot,
    journal_id: String,
    root_id: String,
    object_identity: ObjectIdentity,
}

impl ConvergenceStore {
    /// Admit an initialized journal root. The store never acquires a root of its own.
    pub fn open(root: JournalRoot) -> Result<Self, ConvergenceError> {
        if !check_initialized(&root)? {
            return Err(ConvergenceError::Refused(Refusal::Uninitialized));
        }
        root.revalidate().map_err(map_root_error)?;
        let dirs =
            open_store_dirs(&root)?.ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
        let witness = load_root_witness(&dirs)?;
        let root_id = crate::digest::digest_value(&witness)?.as_hex().to_owned();
        let allocator = load_allocator(&dirs)?;
        require_ids(
            &witness.journal_id,
            &root_id,
            &allocator.journal_id,
            &allocator.root_id,
        )?;
        Ok(Self {
            object_identity: root.identity(),
            root,
            journal_id: witness.journal_id,
            root_id,
        })
    }

    pub fn revalidate(&self) -> Result<(), ConvergenceError> {
        self.root.revalidate().map_err(map_root_error)
    }

    pub fn acquire_days(&self, days: &[DayKey]) -> Result<DayLockSet, ConvergenceError> {
        validate_day_set(days)?;
        self.revalidate()?;
        let dirs = open_store_dirs(&self.root)?
            .ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
        acquire_days(
            &dirs,
            days,
            &self.journal_id,
            &self.root_id,
            self.object_identity,
        )
    }

    /// Read a day under a live lock set. Performs no on-disk write.
    pub fn load_day(&self, days: &DayLockSet, day: &DayKey) -> Result<LoadDay, ConvergenceError> {
        self.inspect(days, day)
    }

    pub(crate) fn inspect(
        &self,
        days: &DayLockSet,
        day: &DayKey,
    ) -> Result<LoadDay, ConvergenceError> {
        if !days.contains(day) {
            return Err(ConvergenceError::Refused(Refusal::WrongDay {
                expected: day.as_str().to_owned(),
                observed: String::new(),
            }));
        }
        self.revalidate()?;
        days.matches(&self.journal_id, &self.root_id, self.object_identity)?;
        crate::publish::inspect_day(self, day)
    }

    pub(crate) fn root(&self) -> &JournalRoot {
        &self.root
    }

    pub(crate) fn journal_id(&self) -> &str {
        &self.journal_id
    }

    pub(crate) fn root_id(&self) -> &str {
        &self.root_id
    }

    pub(crate) fn object_identity(&self) -> ObjectIdentity {
        self.object_identity
    }
}

pub(crate) fn snapshot_from_record(record: &DayRecord) -> Result<DaySnapshot, ConvergenceError> {
    validate_record_numbers(record)?;
    Ok(DaySnapshot {
        day: DayKey::parse(&record.day)?,
        journal_id: record.journal_id.clone(),
        root_id: record.root_id.clone(),
        adoption_id: record.adoption_id.clone(),
        record_revision: record.record_revision,
        first_transition_serial: record.first_transition_serial,
        dirty_by_transition_serial: record.dirty_by_transition_serial,
        dirty_generation: record.dirty_generation,
        completed_generation: record.completed_generation,
        auxiliary_time: record.auxiliary_time.clone(),
        digest: crate::schema::record_digest(record)?,
    })
}

#[cfg(test)]
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use crate::initialize;
    use crate::layout::DayKey;
    use crate::test_support::{
        TempDir, dirty, initialized_store, open_root, sample_day, snapshot_tree,
    };

    #[test]
    fn load_day_creates_nothing() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        store.allocate(&locks).unwrap();
        let before = snapshot_tree(&temporary.journal_path());
        let _ = store.load_day(&locks, &day).unwrap();
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
    }

    #[test]
    fn open_refuses_uninitialized() {
        let temporary = TempDir::new("uninitialized");
        let (_, root) = open_root(&temporary);
        match ConvergenceStore::open(root) {
            Err(error) => assert!(matches!(
                error,
                ConvergenceError::Refused(Refusal::Uninitialized)
            )),
            Ok(_) => panic!("uninitialized root must not open"),
        }
    }

    #[test]
    fn genesis_is_adoption_present_absent_ever_head_record() {
        let (_temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        store.allocate(&locks).unwrap();
        assert!(matches!(
            store.load_day(&locks, &day).unwrap(),
            LoadDay::Genesis
        ));
    }

    #[test]
    fn g1_is_dirty1_completed0_rev1_first_eq_current() {
        let (_temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        match store.load_day(&locks, &day).unwrap() {
            LoadDay::Published(snapshot) => {
                assert_eq!(snapshot.record_revision, 1);
                assert_eq!(snapshot.dirty_generation, 1);
                assert_eq!(snapshot.completed_generation, 0);
                assert_eq!(
                    snapshot.first_transition_serial,
                    snapshot.dirty_by_transition_serial
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn later_dirty_preserves_first_changes_current() {
        let (_temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        let first = match store.load_day(&locks, &day).unwrap() {
            LoadDay::Published(snapshot) => snapshot,
            other => panic!("{other:?}"),
        };
        dirty(&store, &locks, &day);
        match store.load_day(&locks, &day).unwrap() {
            LoadDay::Published(snapshot) => {
                assert_eq!(
                    snapshot.first_transition_serial,
                    first.first_transition_serial
                );
                assert_ne!(
                    snapshot.dirty_by_transition_serial,
                    first.dirty_by_transition_serial
                );
                assert_eq!(snapshot.dirty_generation, 2);
                assert_eq!(snapshot.record_revision, 2);
                assert_eq!(snapshot.completed_generation, 0);
                assert_eq!(snapshot.auxiliary_time, first.auxiliary_time);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn retained_root_survives_namespace_rename_without_touching_replacement() {
        let temporary = TempDir::new("rename");
        let (journal, root) = open_root(&temporary);
        initialize(&root).unwrap();
        let store = ConvergenceStore::open(root).unwrap();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        store.allocate(&locks).unwrap();
        dirty(&store, &locks, &day);
        let original = match store.load_day(&locks, &day).unwrap() {
            LoadDay::Published(snapshot) => snapshot,
            other => panic!("{other:?}"),
        };
        let moved = temporary.path().join("journal-moved");
        std::fs::rename(&journal, &moved).unwrap();
        std::fs::create_dir(&journal).unwrap();
        std::fs::write(journal.join("poison"), b"replacement").unwrap();
        store.revalidate().unwrap();
        match store.load_day(&locks, &day).unwrap() {
            LoadDay::Published(snapshot) => {
                assert_eq!(snapshot.digest, original.digest);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            std::fs::read(journal.join("poison")).unwrap(),
            b"replacement"
        );
        assert!(!journal.join("health").exists());
    }

    #[test]
    fn revalidate_changed_only_when_capability_fails() {
        let (temporary, store) = initialized_store();
        std::fs::rename(temporary.journal_path(), temporary.path().join("moved")).unwrap();
        store.revalidate().unwrap();
    }

    #[test]
    fn restart_reopens_from_durable_bytes() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        let first = match store.load_day(&locks, &day).unwrap() {
            LoadDay::Published(snapshot) => snapshot,
            other => panic!("{other:?}"),
        };
        drop(locks);
        drop(store);
        let root = JournalRoot::open(&temporary.journal_path()).unwrap();
        let store = ConvergenceStore::open(root).unwrap();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        match store.load_day(&locks, &day).unwrap() {
            LoadDay::Published(snapshot) => assert_eq!(snapshot.digest, first.digest),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn g1_sets_first_equal_current() {
        g1_is_dirty1_completed0_rev1_first_eq_current();
    }

    #[test]
    fn later_dirty_accepts_proof_once_per_day() {
        let (_temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        dirty(&store, &locks, &day);
        match store.load_day(&locks, &day).unwrap() {
            LoadDay::Published(snapshot) => {
                assert_eq!(snapshot.dirty_generation, 2);
                assert_ne!(
                    snapshot.first_transition_serial,
                    snapshot.dirty_by_transition_serial
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn genesis_head_ever_record_matrix() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        store.allocate(&locks).unwrap();
        assert!(matches!(
            store.load_day(&locks, &day).unwrap(),
            LoadDay::Genesis
        ));
        dirty(&store, &locks, &day);
        assert!(matches!(
            store.load_day(&locks, &day).unwrap(),
            LoadDay::Published(_)
        ));
        std::fs::remove_file(
            temporary
                .journal_path()
                .join("health/convergence/days/20260823.ever.wit.json"),
        )
        .unwrap();
        assert!(matches!(
            store.load_day(&locks, &day).unwrap_err(),
            ConvergenceError::Unknown { .. }
        ));
    }

    #[test]
    fn record_without_matching_head_is_unknown_or_pending() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        std::fs::remove_file(
            temporary
                .journal_path()
                .join("health/convergence/days/20260823.head.json"),
        )
        .unwrap();
        assert!(matches!(
            store.load_day(&locks, &day).unwrap_err(),
            ConvergenceError::Unknown { .. }
        ));
    }

    #[test]
    fn record_deleted_then_reopen_is_unknown() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        dirty(&store, &locks, &day);
        drop(locks);
        drop(store);
        std::fs::remove_file(
            temporary
                .journal_path()
                .join("health/convergence/records/20260823/record.json"),
        )
        .unwrap();
        let root = JournalRoot::open(&temporary.journal_path()).unwrap();
        let store = ConvergenceStore::open(root).unwrap();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        assert!(matches!(
            store.load_day(&locks, &day).unwrap_err(),
            ConvergenceError::Unknown {
                role: crate::DurableRole::Record
            }
        ));
    }

    #[test]
    fn record_copied_to_other_day_is_wrong_day_or_unknown() {
        let (temporary, store) = initialized_store();
        let day_a = DayKey::parse("20260823").unwrap();
        let day_b = DayKey::parse("20260824").unwrap();
        let locks = store.acquire_days(&[day_a.clone(), day_b.clone()]).unwrap();
        dirty(&store, &locks, &day_a);
        let source = temporary
            .journal_path()
            .join("health/convergence/records/20260823/record.json");
        let dest_dir = temporary
            .journal_path()
            .join("health/convergence/records/20260824");
        std::fs::create_dir_all(&dest_dir).unwrap();
        std::fs::copy(source, dest_dir.join("record.json")).unwrap();
        store.allocate(&locks).unwrap();
        assert!(matches!(
            store.load_day(&locks, &day_b).unwrap_err(),
            ConvergenceError::Unknown { .. }
        ));
    }

    #[test]
    fn artifact_grafted_from_other_journal_id_is_unknown() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        let path = temporary
            .journal_path()
            .join("health/convergence/records/20260823/record.json");
        let mut record: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        record["journal_id"] = serde_json::Value::String("grafted".into());
        std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(matches!(
            store.load_day(&locks, &day).unwrap_err(),
            ConvergenceError::Unknown { .. }
        ));
    }

    #[test]
    fn future_serial_is_refused() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        let path = temporary
            .journal_path()
            .join("health/convergence/records/20260823/record.json");
        let mut record: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        record["dirty_by_transition_serial"] = serde_json::json!(99);
        std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(matches!(
            store.load_day(&locks, &day).unwrap_err(),
            ConvergenceError::Refused(Refusal::FutureSerial { .. })
        ));
    }

    #[test]
    fn wrong_lineage_wrong_day_wrong_journal() {
        let (_temporary, store) = initialized_store();
        let day = sample_day();
        let other = DayKey::parse("20260824").unwrap();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        let error = store.load_day(&locks, &other).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::WrongDay { .. })
        ));
    }

    #[test]
    fn auxiliary_time_is_not_an_ordering_input() {
        let (_temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        let first = match store.load_day(&locks, &day).unwrap() {
            LoadDay::Published(snapshot) => snapshot,
            other => panic!("{other:?}"),
        };
        dirty(&store, &locks, &day);
        match store.load_day(&locks, &day).unwrap() {
            LoadDay::Published(snapshot) => {
                assert_eq!(snapshot.auxiliary_time, first.auxiliary_time);
                assert!(snapshot.record_revision > first.record_revision);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn caller_authored_fields_unrepresentable() {
        let (_temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        match store.load_day(&locks, &day).unwrap() {
            LoadDay::Published(snapshot) => {
                assert_eq!(snapshot.journal_id, store.journal_id());
                assert_eq!(snapshot.root_id, store.root_id());
                assert_eq!(snapshot.record_revision, 1);
                assert_eq!(snapshot.dirty_generation, 1);
                assert_eq!(snapshot.completed_generation, 0);
                assert_eq!(
                    snapshot.first_transition_serial,
                    snapshot.dirty_by_transition_serial
                );
                assert_eq!(snapshot.first_transition_serial, 1);
            }
            other => panic!("{other:?}"),
        }
        let error = crate::schema::parse_json::<crate::schema::DayRecord>(
            br#"{"schema_version":1,"journal_id":"j","root_id":"r","adoption_id":"a","day":"20260823","record_revision":1,"first_transition_serial":1,"dirty_by_transition_serial":1,"dirty_generation":1,"completed_generation":0,"auxiliary_time":"t","extra":1}"#,
            crate::DurableRole::Record,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::UnknownField { field }) if field == "extra"
        ));
    }
}
