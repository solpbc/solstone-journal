// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Test-only external consumer of `solstone-core-journal-convergence`.
//!
//! This crate has no production callsite. It exists so the public store API
//! can be driven from outside the store crate with no test-only feature.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    use solstone_core_journal_convergence::{
        AllocationProof, ConvergenceError, ConvergenceStore, DayKey, LoadDay, OrdinaryAuthority,
        OrdinaryIntent, PendingKind, PublishOutcome, Refusal, check_initialized, initialize,
        validate_day_set,
    };
    use solstone_core_journal_io::JournalRoot;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = PathBuf::from("/var/tmp").join(format!(
                "sjc-harness-{name}-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("create /var/tmp harness directory");
            Self { path }
        }

        fn journal_path(&self) -> PathBuf {
            self.path.join("journal")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn open_store(name: &str) -> (TempDir, ConvergenceStore) {
        let temporary = TempDir::new(name);
        let journal = temporary.journal_path();
        fs::create_dir(&journal).unwrap();
        let root = JournalRoot::open(&journal).unwrap();
        initialize(&root).unwrap();
        let store = ConvergenceStore::open(root).unwrap();
        (temporary, store)
    }

    fn sample_day() -> DayKey {
        DayKey::parse("20260823").unwrap()
    }

    fn dirty(
        store: &ConvergenceStore,
        locks: &solstone_core_journal_convergence::DayLockSet,
        day: &DayKey,
    ) {
        let proof = store.allocate(locks).unwrap();
        let proposal = store
            .propose(locks, day, OrdinaryIntent::AdvanceDirty)
            .unwrap();
        let mut authority = OrdinaryAuthority::bind(proposal, proof).unwrap();
        match store.publish(locks, day, &mut authority).unwrap() {
            PublishOutcome::Published { .. } => {}
            other => panic!("{other:?}"),
        }
    }

    fn days_dir(temporary: &TempDir) -> PathBuf {
        temporary.journal_path().join("health/convergence/days")
    }

    #[test]
    fn harness_topology_allocate_propose_bind_publish() {
        let (temporary, store) = open_store("topology");
        assert!(check_initialized(&JournalRoot::open(&temporary.journal_path()).unwrap()).unwrap());
        let day = sample_day();
        validate_day_set(std::slice::from_ref(&day)).unwrap();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        let proof: AllocationProof = store.allocate(&locks).unwrap();
        let proposal = store
            .propose(&locks, &day, OrdinaryIntent::AdvanceDirty)
            .unwrap();
        let mut authority = OrdinaryAuthority::bind(proposal, proof).unwrap();
        store.publish(&locks, &day, &mut authority).unwrap();
        match store.load_day(&locks, &day).unwrap() {
            LoadDay::Published(snapshot) => {
                assert_eq!(snapshot.record_revision, 1);
                assert_eq!(
                    snapshot.first_transition_serial,
                    snapshot.dirty_by_transition_serial
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn harness_lock_order_disjoint_days() {
        let (temporary, store_a) = open_store("locks");
        let root_b = JournalRoot::open(&temporary.journal_path()).unwrap();
        let store_b = ConvergenceStore::open(root_b).unwrap();
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
    fn harness_lineage_and_authority() {
        let (_temporary, store) = open_store("lineage");
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        dirty(&store, &locks, &day);
        match store.load_day(&locks, &day).unwrap() {
            LoadDay::Published(snapshot) => {
                assert_eq!(snapshot.first_transition_serial, 1);
                assert_eq!(snapshot.dirty_by_transition_serial, 2);
                assert_eq!(snapshot.dirty_generation, 2);
            }
            other => panic!("{other:?}"),
        }
        let proof = store.allocate(&locks).unwrap();
        let proposal = store
            .propose(&locks, &day, OrdinaryIntent::AdvanceDirty)
            .unwrap();
        let mut authority = OrdinaryAuthority::bind(proposal, proof).unwrap();
        store.publish(&locks, &day, &mut authority).unwrap();
        let error = store.publish(&locks, &day, &mut authority).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::ReusedAuthority)
        ));
    }

    #[test]
    fn harness_artifact_loss_head() {
        let (temporary, store) = open_store("loss-head");
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        fs::remove_file(days_dir(&temporary).join("20260823.head.json")).unwrap();
        assert!(matches!(
            store.load_day(&locks, &day).unwrap_err(),
            ConvergenceError::Unknown { .. }
        ));
    }

    #[test]
    fn harness_artifact_loss_tail_witness() {
        let (temporary, store) = open_store("loss-witness");
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        dirty(&store, &locks, &day);
        fs::remove_file(days_dir(&temporary).join("20260823.rev.2.wit.json")).unwrap();
        assert!(matches!(
            store.load_day(&locks, &day).unwrap_err(),
            ConvergenceError::Unknown { .. }
        ));
    }

    #[test]
    fn harness_artifact_loss_record() {
        let (temporary, store) = open_store("loss-record");
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        dirty(&store, &locks, &day);
        fs::remove_file(
            temporary
                .journal_path()
                .join("health/convergence/records/20260823/record.json"),
        )
        .unwrap();
        match store.load_day(&locks, &day) {
            Err(ConvergenceError::Unknown { .. })
            | Ok(LoadDay::PublicationPending {
                kind: PendingKind::HeadAheadOfRecord,
            }) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn harness_restart_reopens_from_durable_bytes() {
        let (temporary, store) = open_store("restart");
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
            LoadDay::Published(snapshot) => {
                assert_eq!(snapshot.digest, first.digest);
                assert_eq!(snapshot.record_revision, first.record_revision);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn harness_temp_is_under_var_tmp() {
        let temporary = TempDir::new("prefix");
        assert!(temporary.path.starts_with(Path::new("/var/tmp")));
        assert!(
            temporary
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("sjc-")
        );
    }
}
