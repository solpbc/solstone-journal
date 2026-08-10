// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::process::Command;

#[test]
fn direct_binary_exports_then_imports_a_segment() {
    let source = tempfile::tempdir().expect("source journal");
    let destination = tempfile::tempdir().expect("destination journal");
    let segment = source.path().join("chronicle/20260203/audio/120000_30");
    fs::create_dir_all(&segment).expect("segment");
    fs::write(segment.join("stream.json"), b"stream").expect("stream");
    fs::write(segment.join("device.json"), b"device").expect("device");
    let archive = source.path().join("transfer.tgz");
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
            source.path().to_str().expect("source path"),
        ])
        .output()
        .expect("export command");
    assert!(
        export.status.success(),
        "{}",
        String::from_utf8_lossy(&export.stderr)
    );

    let import = Command::new(binary)
        .args([
            "transfer",
            "import",
            "--archive",
            archive.to_str().expect("archive path"),
            "--journal",
            destination.path().to_str().expect("destination path"),
        ])
        .output()
        .expect("import command");
    assert!(
        import.status.success(),
        "{}",
        String::from_utf8_lossy(&import.stderr)
    );
    assert_eq!(
        fs::read(
            destination
                .path()
                .join("chronicle/20260203/audio/120000_30/device.json")
        )
        .expect("imported file"),
        b"device"
    );
}

#[test]
fn export_refusals_for_missing_or_empty_day_exit_two() {
    let journal = tempfile::tempdir().expect("journal");
    let output = journal.path().join("archive.tgz");
    let binary = env!("CARGO_BIN_EXE_solstone-core");
    for day in ["20260203", "20260204"] {
        if day == "20260204" {
            fs::create_dir_all(journal.path().join("chronicle").join(day)).expect("empty day");
        }
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
    }
}
