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

    use std::collections::BTreeMap;

    use solstone_core_journal_convergence::{
        ClaimAdmission, ConvergenceError, DayKey, OwnerBinding, Preflight, Refusal, StoreVerdict,
        TerminalOutcome, check_initialized, preflight,
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

    fn open_admitted(
        name: &str,
        days: &[&str],
    ) -> (TempDir, solstone_core_journal_convergence::Admitted) {
        let temporary = TempDir::new(name);
        let journal = temporary.journal_path();
        fs::create_dir(&journal).unwrap();
        let root = JournalRoot::open(&journal).unwrap();
        let set = match preflight(days.iter().copied()).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("expected days"),
        };
        let admitted = set.admit(root).unwrap();
        (temporary, admitted)
    }

    fn sample_day() -> DayKey {
        DayKey::parse("20260823").unwrap()
    }

    fn snapshot_tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
        let mut entries = BTreeMap::new();
        snapshot_walk(root, root, &mut entries);
        entries
    }

    fn snapshot_walk(root: &Path, dir: &Path, entries: &mut BTreeMap<String, Vec<u8>>) {
        let listing = match fs::read_dir(dir) {
            Ok(listing) => listing,
            Err(_) => return,
        };
        for entry in listing.flatten() {
            let path = entry.path();
            let rel = path
                .strip_prefix(root)
                .expect("child of root")
                .to_string_lossy()
                .replace('\\', "/");
            if path.is_dir() {
                entries.insert(rel, Vec::new());
                snapshot_walk(root, &path, entries);
            } else if let Ok(bytes) = fs::read(&path) {
                entries.insert(rel, bytes);
            }
        }
    }

    #[test]
    fn harness_preflight_begin_continue() {
        let (temporary, admitted) = open_admitted("topology", &["20260823"]);
        assert!(check_initialized(&JournalRoot::open(&temporary.journal_path()).unwrap()).unwrap());
        let owner = OwnerBinding::issue_from_base(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = ClaimAdmission::issue_from_base(&held, held.owner()).unwrap();
        held.continue_with(proof).unwrap();
        let snapshot = held.snapshot(&sample_day()).unwrap();
        assert_eq!(snapshot.record_revision, 1);
        assert_eq!(
            snapshot.first_transition_serial,
            snapshot.dirty_by_transition_serial
        );
    }

    #[test]
    fn harness_lock_order_disjoint_days() {
        let (temporary, admitted_a) = open_admitted("locks", &["20260823"]);
        let root_b = JournalRoot::open(&temporary.journal_path()).unwrap();
        let set_b = match preflight(["20260824"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        let admitted_b = set_b
            .admit(root_b)
            .unwrap()
            .with_lock_timeout(Duration::from_millis(80));
        let owner_a = OwnerBinding::issue_from_base(&admitted_a).unwrap();
        let held = admitted_a.begin(owner_a).unwrap();
        let started = Instant::now();
        let owner_b = OwnerBinding::issue_from_base(&admitted_b).unwrap();
        let other = thread::spawn(move || admitted_b.begin(owner_b).map(drop));
        let got = other.join().expect("thread");
        assert!(got.is_ok());
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(held);
    }

    #[test]
    fn harness_lineage_and_authority() {
        let (_temporary, admitted) = open_admitted("lineage", &["20260823"]);
        let owner = OwnerBinding::issue_from_base(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = ClaimAdmission::issue_from_base(&held, held.owner()).unwrap();
        held.continue_with(proof).unwrap();
        let snapshot = held.snapshot(&sample_day()).unwrap();
        assert_eq!(snapshot.record_revision, 1);
        let error = held.proceed();
        assert!(
            error.is_ok() || matches!(error, Err(ConvergenceError::Refused(Refusal::CleanupOnly)))
        );
    }

    #[test]
    fn harness_artifact_loss_head() {
        let (temporary, admitted) = open_admitted("loss-head", &["20260823"]);
        let owner = OwnerBinding::issue_from_base(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = ClaimAdmission::issue_from_base(&held, held.owner()).unwrap();
        held.continue_with(proof).unwrap();
        fs::remove_file(
            temporary
                .journal_path()
                .join("health/convergence/days/20260823.head.json"),
        )
        .unwrap();
        assert!(held.snapshot(&sample_day()).is_err());
    }

    #[test]
    fn harness_artifact_loss_tail_witness() {
        let (temporary, admitted) = open_admitted("loss-witness", &["20260823"]);
        let owner = OwnerBinding::issue_from_base(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = ClaimAdmission::issue_from_base(&held, held.owner()).unwrap();
        held.continue_with(proof).unwrap();
        fs::remove_file(
            temporary
                .journal_path()
                .join("health/convergence/days/20260823.rev.1.wit.json"),
        )
        .unwrap();
        assert!(held.snapshot(&sample_day()).is_err());
    }

    #[test]
    fn harness_restart_reopens_from_durable_bytes() {
        let (temporary, admitted) = open_admitted("restart", &["20260823"]);
        let owner = OwnerBinding::issue_from_base(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = ClaimAdmission::issue_from_base(&held, held.owner()).unwrap();
        held.continue_with(proof).unwrap();
        let first = held.snapshot(&sample_day()).unwrap();
        drop(held);
        drop(admitted);
        let root = JournalRoot::open(&temporary.journal_path()).unwrap();
        let set = match preflight(["20260823"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        let admitted = set.admit(root).unwrap();
        let owner = OwnerBinding::issue_from_base(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = ClaimAdmission::issue_from_base(&held, held.owner()).unwrap();
        let error = held.continue_with(proof).unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Refused(Refusal::Busy)),
            "{error:?}"
        );
        let _ = first;
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

    #[test]
    fn harness_empty_preflight() {
        assert!(matches!(
            preflight::<[&str; 0], &str>([]).unwrap(),
            Preflight::Empty
        ));
    }

    #[test]
    fn ac9_live_permit_commit() {
        let (_temporary, admitted) = open_admitted("permit", &["20260823"]);
        let owner = OwnerBinding::issue_from_base(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = ClaimAdmission::issue_from_base(&held, held.owner()).unwrap();
        let permit = held.continue_with(proof).unwrap();
        let receipt = permit.commit().unwrap();
        assert_eq!(receipt.outcome, TerminalOutcome::Committed);
        assert_eq!(receipt.serial, 1);
    }

    #[test]
    fn ac9_awaiting_owner_decision() {
        let (temporary, admitted) = open_admitted("awaiting", &["20260823"]);
        let owner = OwnerBinding::issue_from_base(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = ClaimAdmission::issue_from_base(&held, held.owner()).unwrap();
        held.continue_with(proof).unwrap();
        drop(held);
        let before = snapshot_tree(&temporary.journal_path());
        let report = admitted.inspect().unwrap();
        assert!(report.awaiting().is_some());
        assert!(report.terminal_outcome().is_none());
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
    }

    #[test]
    fn ac9_supersession() {
        let (_temporary, admitted) = open_admitted("supersede", &["20260823"]);
        let owner = OwnerBinding::issue_from_base(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = ClaimAdmission::issue_from_base(&held, held.owner()).unwrap();
        let permit = held.continue_with(proof).unwrap();
        let _ = permit;
        let first = held.snapshot(&sample_day()).unwrap();
        assert_eq!(first.record_revision, 1);
        held.advance_dirty().unwrap();
        let second = held.snapshot(&sample_day()).unwrap();
        assert!(second.record_revision > first.record_revision);
        assert!(second.dirty_generation > first.dirty_generation);
        drop(held);
        let verdict = admitted
            .inspect_proposed(&sample_day(), first.record_revision)
            .unwrap();
        match verdict {
            StoreVerdict::HeadedDescendant {
                head_revision,
                proposed_revision,
            } => {
                assert_eq!(proposed_revision, first.record_revision);
                assert_eq!(head_revision, second.record_revision);
            }
            other => panic!("expected headed descendant, got {other:?}"),
        }
        let report = admitted.inspect().unwrap();
        assert!(report.awaiting().is_some());
        assert!(report.terminal_outcome().is_none());
    }
}
