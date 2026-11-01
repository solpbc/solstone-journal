// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;

#[test]
fn offload_report_keeps_uncertain_effects_and_unavailable_totals() {
    let failures = vec!["private-path credential-bearing diagnostic".into()];
    let value = ledger(
        [OffloadLedgerWrite::Recorded {
            effect: OffloadLedgerEffect::Attempted { written: Some(7) },
            durability: [
                OffloadLedgerSync::Confirmed,
                OffloadLedgerSync::Uncertain,
                OffloadLedgerSync::Confirmed,
                OffloadLedgerSync::Uncertain,
            ],
            confirmation: OffloadLedgerSync::Uncertain,
            failures: &failures,
        }]
        .into_iter(),
    );
    assert_eq!(value[0]["effect"], "attempted");
    assert_eq!(value[0]["bytes"], 7);
    assert_eq!(value[0]["confirmation"], "uncertain");
    assert_eq!(value[0]["durability"][0], "confirmed");
    assert_eq!(value[0]["durability"][1], "uncertain");
    assert_eq!(value[0]["failures"][0]["detail_available"], true);
    assert!(!value.to_string().contains("credential-bearing"));
    let planning = planning_fields(
        OffloadPlanningPhase::OffersPublished,
        None,
        None,
        None,
        None,
        None,
    );
    assert_eq!(planning["phase"], "offers_published");
    for field in ["budget_bytes", "local", "decision", "initial_projection"] {
        assert!(planning[field].is_null(), "{field}");
    }
    assert!(planning.get("budget_met").is_none());
}

#[test]
fn native_archive_refusal_stage_is_per_failure_and_never_settles_effects() {
    use solstone_core_backup::archive_admission::NativeArchiveStartStage;
    use solstone_core_backup_runtime::ArchiveError;
    let failures = [
        Some(NativeArchiveStartStage::StateBinding),
        None,
        Some(NativeArchiveStartStage::SourceRoot),
    ]
    .map(|stage| {
        Err(ArchiveFailure {
            diagnostic: None,
            error: ArchiveError::NativeStart { status: 1, stage },
            remote_effects: ArchiveRemoteEffects::NotAttempted,
        })
    });
    let report = archives(&failures);
    assert_eq!(
        report[0]["native_start"],
        json!({"status":1,"refusal_stage":"state_binding"})
    );
    assert!(report[1]["native_start"].is_null());
    assert_eq!(
        report[2]["native_start"],
        json!({"status":1,"refusal_stage":"source_root"})
    );
    for row in report.as_array().unwrap() {
        assert_eq!(row["reason"], "archive start refused (status 1)");
        assert_eq!(row["remote_effects"], json!({"state":"not_attempted"}));
        assert!(row["diagnostic"].is_null());
    }
    assert!(NativeArchiveStartStage::from_code(0).is_none());
    assert!(NativeArchiveStartStage::from_code(u32::MAX).is_none());
}

#[test]
fn failed_archive_stderr_reaches_report_bounded_and_redacted_without_settling_effects() {
    use solstone_core_backup::archive_readback::{
        ArchiveCommandDiagnostic, DirectArchiveEffects, DirectArchiveFailure, ReadbackError,
    };
    use std::{collections::BTreeMap, ffi::OsString};
    let environment = BTreeMap::from([
        (
            OsString::from("RESTIC_PASSWORD"),
            OsString::from("fixture-password"),
        ),
        (
            OsString::from("AWS_SECRET_ACCESS_KEY"),
            OsString::from("fixture-secret"),
        ),
        (
            OsString::from("RCLONE_CONFIG_SPB_SESSION_TOKEN"),
            OsString::from("fixture-session"),
        ),
        (
            OsString::from("RCLONE_CONFIG_SPB_TYPE"),
            OsString::from("s3"),
        ),
    ]);
    let stderr = format!(
        "Fatal: s3 command refused fixture-password fixture-secret fixture-session {}",
        "\u{03b1}".repeat(4096)
    );
    let diagnostic =
        ArchiveCommandDiagnostic::from_stderr(stderr.as_bytes(), &environment).unwrap();
    let direct = DirectArchiveFailure {
        reason: ReadbackError::ToolFailed,
        effects: DirectArchiveEffects::Attempted {
            returncode: Some(1),
            observed_snapshot_id: None,
        },
        diagnostic: Some(diagnostic),
    };
    let report = archives(&[Err(ArchiveFailure::from(direct))]);
    let message = report[0]["diagnostic"]["stderr"].as_str().unwrap();
    assert!(message.starts_with("Fatal: s3 command refused <redacted> <redacted> <redacted>"));
    assert!(message.len() <= 4096);
    assert!(
        !message.contains("fixture-password")
            && !message.contains("fixture-secret")
            && !message.contains("fixture-session")
    );
    assert_eq!(report[0]["diagnostic"]["truncated"], true);
    assert_eq!(
        report[0]["remote_effects"],
        json!({"state":"attempted","returncode":1,"observed_snapshot_id":null})
    );
    let missing = DirectArchiveFailure {
        reason: ReadbackError::ToolFailed,
        effects: DirectArchiveEffects::NotAttempted,
        diagnostic: None,
    };
    assert!(archives(&[Err(ArchiveFailure::from(missing))])[0]["diagnostic"].is_null());
    assert!(ArchiveCommandDiagnostic::from_stderr(b"", &environment).is_none());
}
