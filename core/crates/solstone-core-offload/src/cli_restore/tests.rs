// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;

struct FixedClock;
impl Clock for FixedClock {
    fn now_unix(&self) -> i64 {
        1_760_000_000
    }
    fn iso_week(&self) -> u8 {
        41
    }
}

#[test]
fn whole_cli_restore_result_is_send_static() {
    fn closed<T: Send + 'static>() {}
    closed::<OwnedRestoreCliRun>();
}

#[test]
fn settlement_detail_retains_the_transaction_cause_and_actual_counts() {
    let source = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
    for transaction in [None, Some("Repair(CatalogChanged)")] {
        let mut report = base("degraded", "restore_held", None);
        report.files_restored = 1;
        report.bytes_restored = 3;
        report.reason_detail = transaction.map(str::to_owned);
        let result = with_settlement_detail(
            report,
            RestoreFinishRefusal::Native {
                file: Some(0),
                reason: "unsettled_transaction",
                source: Some(&source),
                readback: None,
            },
        );
        assert_eq!(result.status, "degraded");
        assert_eq!(result.reason.as_deref(), Some("restore_held"));
        assert_eq!(result.files_restored, 1);
        assert_eq!(result.bytes_restored, 3);
        let detail = result.reason_detail.as_deref().unwrap();
        assert!(detail.contains("unsettled_transaction"));
        assert!(detail.contains("PermissionDenied"));
        if let Some(transaction) = transaction {
            assert!(detail.starts_with(transaction));
        }
    }
}

#[test]
fn a_missing_journal_starts_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing-journal");
    let run = run_cli_restore(&missing, Arc::new(FixedClock), Some("20261009"));
    assert_eq!(run.settlement(), CliRestoreSettlement::NotStarted);
    assert_eq!(run.result().status, "refused");
    let expected = if cfg!(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows"
    )) {
        "segment_identity"
    } else {
        "restore_held"
    };
    assert_eq!(run.result().reason.as_deref(), Some(expected));
    assert!(run.runtime().is_none());
    assert!(!run.completed());
    assert!(!missing.exists());
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn nothing_offloaded_is_a_no_op_without_activity() {
    let journal = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(journal.path().join("chronicle")).unwrap();
    for day in [None, Some("20261009")] {
        let run = run_cli_restore(journal.path(), Arc::new(FixedClock), day);
        assert_eq!(run.settlement(), CliRestoreSettlement::NotStarted);
        assert_eq!(run.result().status, "no_op");
        assert_eq!(run.result().reason.as_deref(), Some("nothing_to_restore"));
        assert_eq!(
            run.result().scope,
            if day.is_some() { "day" } else { "all" }
        );
        assert!(!journal.path().join("backup-activity.lock").exists());
    }
}
