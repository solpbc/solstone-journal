// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::process::Command;

#[test]
fn direct_binary_transfer_export_and_import_are_tombstoned_without_archive_io() {
    let journal = tempfile::tempdir().expect("journal");
    let archive = journal.path().join("transfer.tgz");
    let binary = env!("CARGO_BIN_EXE_solstone-core");

    let export = Command::new(binary)
        .args([
            "transfer",
            "export",
            "--day",
            "20260203",
            "--output",
            archive.to_str().expect("archive path"),
            "--journal",
            journal.path().to_str().expect("journal path"),
        ])
        .output()
        .expect("export command");
    assert_eq!(export.status.code(), Some(2));
    assert!(export.stdout.is_empty());
    assert!(String::from_utf8_lossy(&export.stderr).contains("journal archive export"));
    assert!(!archive.exists(), "tombstone must not create an archive");

    let import = Command::new(binary)
        .args([
            "transfer",
            "import",
            "--archive",
            archive.to_str().expect("archive path"),
            "--journal",
            journal.path().to_str().expect("journal path"),
        ])
        .output()
        .expect("import command");
    assert_eq!(import.status.code(), Some(2));
    assert!(import.stdout.is_empty());
    assert!(String::from_utf8_lossy(&import.stderr).contains("journal archive merge"));
}

#[test]
fn transfer_export_tombstone_replaces_former_argument_validation() {
    let journal = tempfile::tempdir().expect("journal");
    let output = journal.path().join("archive.tgz");
    let binary = env!("CARGO_BIN_EXE_solstone-core");
    for day in ["20260203", "20260204"] {
        let result = Command::new(binary)
            .args([
                "transfer",
                "export",
                "--day",
                day,
                "--output",
                output.to_str().expect("output path"),
                "--journal",
                journal.path().to_str().expect("journal path"),
            ])
            .output()
            .expect("export command");
        assert_eq!(
            result.status.code(),
            Some(2),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(String::from_utf8_lossy(&result.stderr).contains("journal archive export"));
    }
    assert!(!output.exists(), "tombstone must not create an archive");
}

#[test]
fn direct_binary_transfer_import_dry_run_is_tombstoned_without_reading_archive() {
    let journal = tempfile::tempdir().expect("journal");
    let archive = journal.path().join("missing.tgz");
    let binary = env!("CARGO_BIN_EXE_solstone-core");

    let import = Command::new(binary)
        .args([
            "transfer",
            "import",
            "--archive",
            archive.to_str().expect("archive path"),
            "--dry-run",
            "--journal",
            journal.path().to_str().expect("journal path"),
        ])
        .output()
        .expect("import command");
    assert_eq!(import.status.code(), Some(2));
    assert!(import.stdout.is_empty());
    assert!(String::from_utf8_lossy(&import.stderr).contains("journal archive merge"));
    assert!(
        !archive.exists(),
        "tombstone must not read or create an archive"
    );
}
