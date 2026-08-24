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
        AdmitOutcome, Authorization, ClaimAdmission, ConvergenceError, DayKey, Delivery,
        GrantOutcome, GrantRequestSelector, GrantState, OperationId, OwnerBinding, OwnerRevoke,
        Preflight, Refusal, StoreVerdict, TargetScope, TerminalOutcome, TransactionClass,
        WriterFamily, check_initialized, preflight,
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

    /// Prepared owner for a fresh external operation over the admitted days,
    /// with an empty grant-request selector. The harness drives the same public
    /// mint the store's own tests use; there is no test-only feature.
    fn prepared_owner(
        admitted: &solstone_core_journal_convergence::Admitted,
    ) -> Result<OwnerBinding, ConvergenceError> {
        let operation = OperationId::generate()?;
        let selector = GrantRequestSelector::empty(admitted.days())?;
        OwnerBinding::prepare(
            admitted,
            &operation,
            TransactionClass::AdvanceDirty,
            &selector,
        )
    }

    fn admit_proof(
        held: &solstone_core_journal_convergence::HeldDays<'_>,
        owner: &OwnerBinding,
    ) -> Result<ClaimAdmission, ConvergenceError> {
        match ClaimAdmission::admit(held, owner)? {
            AdmitOutcome::Proof(proof) => Ok(proof),
            AdmitOutcome::ExistingLink => Err(ConvergenceError::Refused(Refusal::ReusedAuthority)),
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

    fn one_request() -> [(&'static str, WriterFamily, TargetScope); 1] {
        [("20260823", WriterFamily::Think, TargetScope::Chronicle)]
    }

    fn two_requests() -> [(&'static str, WriterFamily, TargetScope); 2] {
        [
            ("20260823", WriterFamily::Think, TargetScope::Chronicle),
            ("20260823", WriterFamily::Observe, TargetScope::Entities),
        ]
    }

    fn commit_selector(
        admitted: &solstone_core_journal_convergence::Admitted,
        selector: GrantRequestSelector,
    ) -> (OperationId, GrantRequestSelector) {
        let operation = OperationId::generate().unwrap();
        let owner = OwnerBinding::prepare(
            admitted,
            &operation,
            TransactionClass::AdvanceDirty,
            &selector,
        )
        .unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
        let receipt = held.continue_with(proof).unwrap().commit().unwrap();
        assert_eq!(receipt.outcome, TerminalOutcome::Committed);
        drop(held);
        (operation, selector)
    }

    fn commit_nonempty(
        admitted: &solstone_core_journal_convergence::Admitted,
    ) -> (OperationId, GrantRequestSelector) {
        let selector = GrantRequestSelector::try_new(admitted.days(), one_request()).unwrap();
        commit_selector(admitted, selector)
    }

    fn reopen_admitted(
        temporary: &TempDir,
        days: &[&str],
    ) -> solstone_core_journal_convergence::Admitted {
        let root = JournalRoot::open(&temporary.journal_path()).unwrap();
        let set = match preflight(days.iter().copied()).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("expected days"),
        };
        set.admit(root).unwrap()
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
        let owner = prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
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
        let owner_a = prepared_owner(&admitted_a).unwrap();
        let held = admitted_a.begin(owner_a).unwrap();
        let started = Instant::now();
        let owner_b = prepared_owner(&admitted_b).unwrap();
        let other = thread::spawn(move || admitted_b.begin(owner_b).map(drop));
        let got = other.join().expect("thread");
        assert!(got.is_ok());
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(held);
    }

    #[test]
    fn harness_lineage_and_authority() {
        let (_temporary, admitted) = open_admitted("lineage", &["20260823"]);
        let owner = prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
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
        let owner = prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
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
        let owner = prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
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
        let owner = prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
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
        let owner = prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
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
        let owner = prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
        let permit = held.continue_with(proof).unwrap();
        let receipt = permit.commit().unwrap();
        assert_eq!(receipt.outcome, TerminalOutcome::Committed);
        assert_eq!(receipt.serial, 1);
    }

    #[test]
    fn ac9_awaiting_owner_decision() {
        let (temporary, admitted) = open_admitted("awaiting", &["20260823"]);
        let owner = prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
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
        let owner = prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
        let permit = held.continue_with(proof).unwrap();
        let _ = permit;
        let first = held.snapshot(&sample_day()).unwrap();
        assert_eq!(first.record_revision, 1);
        let successor = admit_proof(&held, held.owner()).unwrap();
        held.advance_dirty(successor).unwrap();
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

    #[test]
    fn ac9_clearance() {
        let (_temporary, admitted) = open_admitted("clearance", &["20260823"]);
        let owner = prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
        let permit = held.continue_with(proof).unwrap();
        permit.commit().unwrap();
        drop(held);
        let owner = prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
        held.continue_with(proof).unwrap();
        let snapshot = held.snapshot(&sample_day()).unwrap();
        assert_eq!(snapshot.record_revision, 2);
        assert_eq!(snapshot.dirty_generation, 2);
    }

    #[test]
    fn ac9_external_activation_reports_nonempty_committed() {
        let (_temporary, admitted) = open_admitted("ac9-activation", &["20260823"]);
        let (operation, selector) = commit_nonempty(&admitted);
        assert_eq!(
            admitted.grant_state(&operation, &selector).unwrap(),
            GrantState::Outcome(GrantOutcome::NonemptyCommitted)
        );
        assert!(matches!(
            admitted.deliver_grants(&operation, &selector).unwrap(),
            Delivery::Ready(tokens) if tokens.len() == 1
        ));
    }

    #[test]
    fn ac9_external_redelivery_matches_after_restart() {
        let (temporary, admitted) = open_admitted("ac9-redelivery", &["20260823"]);
        let (operation, selector) = commit_nonempty(&admitted);
        let first = admitted.deliver_grants(&operation, &selector).unwrap();
        let first_hex = first.tokens()[0].as_hex().to_owned();
        drop(first);
        drop(admitted);

        let resumed = reopen_admitted(&temporary, &["20260823"]);
        let second = resumed.deliver_grants(&operation, &selector).unwrap();
        let token = &second.tokens()[0];
        assert_eq!(token.as_hex(), first_hex);
        let day = DayKey::parse(token.day()).unwrap();
        let lease = resumed.grant_lease().unwrap();
        assert!(matches!(
            lease
                .authorize(
                    &operation,
                    &selector,
                    token.as_hex(),
                    &day,
                    token.writer_family(),
                    token.target_scope(),
                )
                .unwrap(),
            Authorization::Granted(_)
        ));
    }

    #[test]
    fn ac9_external_lease_authorization_rejects_forged_and_wrong_target_bytes() {
        let (_temporary, admitted) = open_admitted("ac9-lease", &["20260823"]);
        let (operation, selector) = commit_nonempty(&admitted);
        let delivery = admitted.deliver_grants(&operation, &selector).unwrap();
        let token = &delivery.tokens()[0];
        let token_hex = token.as_hex().to_owned();
        let day = DayKey::parse(token.day()).unwrap();
        let family = token.writer_family();
        let scope = token.target_scope();
        drop(delivery);
        let lease = admitted.grant_lease().unwrap();
        assert!(matches!(
            lease
                .authorize(&operation, &selector, &token_hex, &day, family, scope)
                .unwrap(),
            Authorization::Granted(_)
        ));
        assert!(matches!(
            lease
                .authorize(&operation, &selector, &"00".repeat(32), &day, family, scope)
                .unwrap(),
            Authorization::Denied { .. }
        ));
        assert!(matches!(
            lease
                .authorize(
                    &operation,
                    &selector,
                    &token_hex,
                    &day,
                    family,
                    TargetScope::Entities,
                )
                .unwrap(),
            Authorization::Denied { .. }
        ));
    }

    #[test]
    fn ac9_external_owner_revocation_is_independent_for_disjoint_days() {
        let (temporary, admitted_a) = open_admitted("ac9-owner-revoke", &["20260823"]);
        let admitted_b = reopen_admitted(&temporary, &["20260824"]);
        let (operation_a, selector_a) = commit_nonempty(&admitted_a);
        let selector_b = GrantRequestSelector::try_new(
            admitted_b.days(),
            [("20260824", WriterFamily::Think, TargetScope::Chronicle)],
        )
        .unwrap();
        let (operation_b, selector_b) = commit_selector(&admitted_b, selector_b);

        assert_eq!(
            admitted_a.revoke_owner(&operation_a, &selector_a).unwrap(),
            OwnerRevoke::Revoked
        );
        assert!(matches!(
            admitted_a
                .deliver_grants(&operation_a, &selector_a)
                .unwrap(),
            Delivery::Denied { .. }
        ));
        assert!(matches!(
            admitted_b
                .deliver_grants(&operation_b, &selector_b)
                .unwrap(),
            Delivery::Ready(_)
        ));
    }

    #[test]
    fn ac9_external_member_revocation_leaves_sibling_authorized() {
        let (_temporary, admitted) = open_admitted("ac9-member-revoke", &["20260823"]);
        let selector = GrantRequestSelector::try_new(admitted.days(), two_requests()).unwrap();
        let (operation, selector) = commit_selector(&admitted, selector);
        let delivery = admitted.deliver_grants(&operation, &selector).unwrap();
        let sibling = delivery
            .tokens()
            .iter()
            .find(|token| token.writer_family() == WriterFamily::Observe)
            .unwrap();
        let sibling_hex = sibling.as_hex().to_owned();
        let sibling_day = DayKey::parse(sibling.day()).unwrap();
        let sibling_family = sibling.writer_family();
        let sibling_scope = sibling.target_scope();
        drop(delivery);

        assert_eq!(
            admitted
                .revoke_grant(
                    &operation,
                    &selector,
                    &sample_day(),
                    WriterFamily::Think,
                    TargetScope::Chronicle,
                )
                .unwrap(),
            solstone_core_journal_convergence::GrantRevoke::Revoked
        );
        let lease = admitted.grant_lease().unwrap();
        assert!(matches!(
            lease
                .authorize(
                    &operation,
                    &selector,
                    &sibling_hex,
                    &sibling_day,
                    sibling_family,
                    sibling_scope,
                )
                .unwrap(),
            Authorization::Granted(_)
        ));
    }

    #[test]
    fn ac9_external_pruning_requires_the_member_own_later_dirty_generation() {
        let (_temporary, admitted) = open_admitted("ac9-prune", &["20260823"]);
        let (operation, selector) = commit_nonempty(&admitted);
        let day = sample_day();
        assert!(
            !admitted
                .grant_pruned(
                    &operation,
                    &selector,
                    &day,
                    WriterFamily::Think,
                    TargetScope::Chronicle,
                )
                .unwrap()
        );
        admitted
            .revoke_grant(
                &operation,
                &selector,
                &day,
                WriterFamily::Think,
                TargetScope::Chronicle,
            )
            .unwrap();
        assert!(
            !admitted
                .grant_pruned(
                    &operation,
                    &selector,
                    &day,
                    WriterFamily::Think,
                    TargetScope::Chronicle,
                )
                .unwrap()
        );

        let successor = GrantRequestSelector::empty(admitted.days()).unwrap();
        let _ = commit_selector(&admitted, successor);
        admitted
            .revoke_grant(
                &operation,
                &selector,
                &day,
                WriterFamily::Think,
                TargetScope::Chronicle,
            )
            .unwrap();
        assert!(
            admitted
                .grant_pruned(
                    &operation,
                    &selector,
                    &day,
                    WriterFamily::Think,
                    TargetScope::Chronicle,
                )
                .unwrap()
        );
    }

    #[test]
    fn ac9_external_restart_pending_reports_its_only_recovery() {
        let (temporary, admitted) = open_admitted("ac9-pending-restart", &["20260823"]);
        let operation = OperationId::generate().unwrap();
        let selector = GrantRequestSelector::try_new(admitted.days(), one_request()).unwrap();
        let owner = OwnerBinding::prepare(
            &admitted,
            &operation,
            TransactionClass::AdvanceDirty,
            &selector,
        )
        .unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
        held.continue_with(proof).unwrap();
        drop(held);
        drop(admitted);

        let resumed = reopen_admitted(&temporary, &["20260823"]);
        let report = resumed.inspect().unwrap();
        let awaiting = report.awaiting().expect("named recovery");
        assert_eq!(
            awaiting.stage(),
            solstone_core_journal_convergence::AwaitingStage::AfterProjection
        );
        assert!(report.terminal_outcome().is_none());
        let same_owner = OwnerBinding::prepare(
            &resumed,
            &operation,
            TransactionClass::AdvanceDirty,
            &selector,
        )
        .unwrap();
        let held = resumed.begin(same_owner).unwrap();
        assert!(matches!(
            ClaimAdmission::admit(&held, held.owner()).unwrap(),
            AdmitOutcome::ExistingLink
        ));
    }
}
