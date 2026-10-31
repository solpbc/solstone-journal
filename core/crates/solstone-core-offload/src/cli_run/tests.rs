// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountClock(AtomicUsize);
impl Clock for CountClock {
    fn now_unix(&self) -> i64 {
        self.0.fetch_add(1, Ordering::Relaxed);
        1_791_713_600
    }
    fn iso_week(&self) -> u8 {
        41
    }
}

#[test]
fn dry_run_refuses_before_root_clock_and_preparation() {
    let clock = Arc::new(CountClock(AtomicUsize::new(0)));
    let run = run_cli_offload(
        Path::new("/does-not-exist/offload-cli"),
        clock.clone(),
        true,
    );
    assert!(matches!(
        run.prelaunch_refusal(),
        Some(CliPrelaunchRefusal::DryRun)
    ));
    assert_eq!(clock.0.load(Ordering::Relaxed), 0);
    assert!(run.preparation_custody().is_none());
    assert!(run.observed().planning_phase.is_none());
    assert_ne!(run.output().2, 0);
}

#[test]
fn finite_closed_cli_outcome_is_send_static() {
    fn assert_closed<T: Send + 'static>() {}
    assert_closed::<OwnedOffloadCliRun>();
}

#[test]
fn mutation_projection_keeps_publication_and_independent_uncertainty() {
    use solstone_core_retention::mark_mutation::{ConditionalResult, RegisterUncertainty};
    use solstone_core_retention::marks::Register;
    let outcome =
        solstone_core_retention::mark_mutation::BoundMutationOutcome::PublishedUncertain {
            candidate: ConditionalResult {
                status: solstone_core_retention::mark_mutation::MutationStatus::Applied,
                register: Register::empty(),
            },
            uncertainty: RegisterUncertainty {
                durability: Some(io::Error::other("synthetic fsync failure")),
                observation: Some(io::Error::other("synthetic final-name failure")),
                ..RegisterUncertainty::default()
            },
        };
    let fact = report::mutation(&outcome);
    assert!(fact.published);
    assert!(fact.durability_uncertain);
    assert!(fact.observation_uncertain);
    assert_eq!(
        fact.status,
        solstone_core_retention::mark_mutation::MutationStatus::Applied
    );
}

#[cfg(all(target_os = "linux", feature = "full-tests"))]
mod retained {
    use super::*;

    #[test]
    fn preparation_unwind_retains_original_activity_through_rendering() {
        let temp = tempfile::tempdir().unwrap();
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        PANIC_AT.with(|cut| cut.set(Some(CliUnwindPhase::Preparation)));
        let clock = Arc::new(CountClock(AtomicUsize::new(0)));
        let run = run_cli_offload(temp.path(), clock.clone(), false);
        assert_eq!(
            run.phase(),
            CliRunPhase::Unknown(CliUnwindPhase::Preparation)
        );
        let custody = run.preparation_custody().unwrap();
        assert_eq!(custody.journal_root().identity(), root.identity());
        assert!(Arc::ptr_eq(
            custody.owned_clock(),
            &(clock as Arc<dyn Clock + Send + Sync>)
        ));
        custody.revalidate().unwrap();
        assert!(
            solstone_core_journal_io::activity::ActivityLease::try_acquire(root.clone()).is_err()
        );
        assert_ne!(run.output().2, 0);
        drop(run);
        let _next = solstone_core_journal_io::activity::ActivityLease::try_acquire(root).unwrap();
    }

    #[test]
    fn a_paused_backup_skips_offload_before_preparation() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("config")).unwrap();
        std::fs::create_dir_all(temp.path().join("chronicle")).unwrap();
        std::fs::write(
            temp.path().join("config/journal.json"),
            r#"{"backup":{"enabled":true,"mode":"byo","paused_at":1769990400,"offload":{"enabled":true,"budget_bytes":1,"floor_bytes":1}}}"#,
        )
        .unwrap();
        let clock = Arc::new(CountClock(AtomicUsize::new(0)));
        let run = run_cli_offload(temp.path(), clock.clone(), false);
        assert_eq!(run.phase(), CliRunPhase::Skipped);
        assert_eq!(
            run.output(),
            ("offload_status=skipped reason=backup_paused\n", "", 0)
        );
        assert!(run.preparation_custody().is_none());
    }

    #[test]
    fn offload_off_skips_before_preparation_and_retires_an_earlier_reservation() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("config")).unwrap();
        std::fs::create_dir_all(temp.path().join("chronicle")).unwrap();
        std::fs::write(
            temp.path().join("config/journal.json"),
            r#"{"backup":{"enabled":true,"mode":"byo"}}"#,
        )
        .unwrap();
        // The empty root reservation a held run leaves behind; recovery retires it.
        std::fs::create_dir(temp.path().join(".tmp-offload-earlier")).unwrap();
        let clock = Arc::new(CountClock(AtomicUsize::new(0)));
        let run = run_cli_offload(temp.path(), clock.clone(), false);
        assert_eq!(run.phase(), CliRunPhase::Skipped);
        assert_eq!(
            run.output(),
            ("offload_status=skipped reason=offload_off\n", "", 0)
        );
        assert_eq!(clock.0.load(Ordering::Relaxed), 0);
        assert!(run.preparation_custody().is_none());
        let stages = std::fs::read_dir(temp.path())
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".tmp-offload-")
            })
            .count();
        assert_eq!(stages, 0);
    }
}

#[test]
fn missing_root_or_unsupported_platform_refuses_before_clock_and_preparation() {
    let clock = Arc::new(CountClock(AtomicUsize::new(0)));
    let run = run_cli_offload(
        Path::new("/does-not-exist/offload-cli"),
        clock.clone(),
        false,
    );
    assert_eq!(run.phase(), CliRunPhase::Prelaunch);
    assert!(
        matches!(
            run.prelaunch_refusal(),
            Some(CliPrelaunchRefusal::Root(_))
                if PRODUCER_SUPPORTED
        ) || matches!(
            run.prelaunch_refusal(),
            Some(CliPrelaunchRefusal::UnsupportedPlatform)
                if !PRODUCER_SUPPORTED
        )
    );
    assert_eq!(clock.0.load(Ordering::Relaxed), 0);
    assert!(run.preparation_custody().is_none());
    assert!(run.observed().planning_phase.is_none());
    assert!(run.runtime().is_none());
    assert!(run.binding_errors().is_empty());
    assert_ne!(run.output().2, 0);
}
