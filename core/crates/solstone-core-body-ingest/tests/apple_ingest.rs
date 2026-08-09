// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

use rusqlite::Connection;
use serde_json::json;
use solstone_core_body_ingest::{
    AppleImportOptions, BodyIngestErrorKind, detect_apple_source, preview_apple, save_apple,
};
use solstone_core_body_rebuild::{BodyRebuildErrorKind, rebuild_body_store};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-body-apple-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fixture(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join(relative)
}

fn write_approval(journal: &Path, decision: &str) {
    let path = journal.join("imports/_approvals/health_import_preflight.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        serde_json::to_vec(&json!({
            "schema": "solstone.health_import_preflight.v1",
            "checklist_version": "solstone.health_import_preflight.checklist.v3",
            "approved_by": "Synthetic Owner",
            "approved_at": "2026-08-09T00:00:00Z",
            "journal_root": journal.canonicalize().unwrap(),
            "approved_importers": ["apple_health"],
            "replication_destinations": {
                "time_machine": {"decision": "approved", "notes": "synthetic"},
                "icloud": {"decision": "excluded", "notes": "synthetic"},
                "solbase": {"decision": "excluded", "notes": "synthetic"},
                "hosted_backup": {"decision": "excluded", "notes": "synthetic"},
                "other": {"decision": "excluded", "notes": "synthetic"}
            },
            "raw_retention": {
                "decision": decision,
                "notes": "synthetic",
                "unparsed_sensitive_modalities_acknowledged": true
            },
            "requires_per_run_confirmation": true,
            "no_real_health_data_in_artifact": true
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn directory_and_zip_preview_match_and_save_publishes_rebuildable_native_history() {
    let directory = fixture("tests/fixtures/importers/health/apple_health_synthetic");
    let zip = fixture("tests/fixtures/importers/health/apple_health_synthetic.zip");
    assert!(detect_apple_source(&directory).unwrap());
    assert!(detect_apple_source(&zip).unwrap());
    let directory_preview = preview_apple(&directory, None, None).unwrap();
    let zip_preview = preview_apple(&zip, None, None).unwrap();
    assert_eq!(directory_preview.rows(), zip_preview.rows());
    assert_eq!(directory_preview.days(), zip_preview.days());
    assert!(directory_preview.rows() > 0);

    let temporary = TempDir::new();
    let journal = temporary.path().join("journal");
    fs::create_dir(&journal).unwrap();
    write_approval(&journal, "retain_parsed");
    let report = save_apple(
        &zip,
        &journal,
        &AppleImportOptions {
            confirm_body_save: true,
            ..AppleImportOptions::default()
        },
    )
    .unwrap();

    assert_eq!(report.rows(), directory_preview.rows());
    let import = journal.join("imports").join(report.bundle_id().unwrap());
    for relative in [
        "body-bundle.json",
        "body-ledger.jsonl",
        "body-raw-inventory.jsonl",
        "manifest.json",
        "raw/export.xml",
    ] {
        assert!(import.join(relative).is_file(), "missing {relative}");
    }
    let connection = Connection::open(journal.join("imports/health-dedupe.sqlite")).unwrap();
    let rows: i64 = connection
        .query_row("SELECT COUNT(*) FROM health_dedupe", [], |row| row.get(0))
        .unwrap();
    assert_eq!(u64::try_from(rows).unwrap(), report.rows());
    drop(connection);
    fs::remove_file(journal.join("imports/health-dedupe.sqlite")).unwrap();

    let skipped = save_apple(
        &zip,
        &journal,
        &AppleImportOptions {
            confirm_body_save: true,
            ..AppleImportOptions::default()
        },
    )
    .unwrap();
    assert!(skipped.skipped());
    assert_eq!(skipped.bundle_id(), report.bundle_id());
    let repaired = Connection::open(journal.join("imports/health-dedupe.sqlite")).unwrap();
    let repaired_rows: i64 = repaired
        .query_row("SELECT COUNT(*) FROM health_dedupe", [], |row| row.get(0))
        .unwrap();
    assert_eq!(u64::try_from(repaired_rows).unwrap(), report.rows());
    drop(repaired);

    let forced = save_apple(
        &zip,
        &journal,
        &AppleImportOptions {
            confirm_body_save: true,
            force: true,
            ..AppleImportOptions::default()
        },
    )
    .unwrap();
    assert!(!forced.skipped());
    assert_ne!(forced.bundle_id(), report.bundle_id());
}

#[test]
fn retained_raw_is_digest_bound_and_rebuild_refuses_tampering() {
    let temporary = TempDir::new();
    let journal = temporary.path().join("journal");
    fs::create_dir(&journal).unwrap();
    write_approval(&journal, "retain_parsed");
    let source = fixture("tests/fixtures/importers/health/apple_health_synthetic.zip");
    let report = save_apple(
        &source,
        &journal,
        &AppleImportOptions {
            confirm_body_save: true,
            ..AppleImportOptions::default()
        },
    )
    .unwrap();
    let bundle = journal.join("imports").join(report.bundle_id().unwrap());
    fs::write(bundle.join("raw/export.xml"), b"tampered synthetic raw\n").unwrap();

    let error = rebuild_body_store(&journal).expect_err("tampered raw must fail closed");
    assert_eq!(error.kind(), BodyRebuildErrorKind::NativeReplay);
    assert_eq!(error.stage(), "raw_inventory_mismatch");
}

#[test]
fn retention_and_complete_source_inventory_participate_in_quiet_run_identity() {
    let temporary = TempDir::new();
    let journal = temporary.path().join("journal");
    let source = temporary.path().join("apple-export");
    fs::create_dir(&journal).unwrap();
    fs::create_dir_all(source.join("apple_health_export")).unwrap();
    fs::copy(
        fixture(
            "tests/fixtures/importers/health/apple_health_synthetic/apple_health_export/export.xml",
        ),
        source.join("apple_health_export/export.xml"),
    )
    .unwrap();
    fs::write(source.join("auxiliary.txt"), b"synthetic auxiliary v1\n").unwrap();

    write_approval(&journal, "discard");
    let discarded = save_apple(
        &source,
        &journal,
        &AppleImportOptions {
            confirm_body_save: true,
            ..AppleImportOptions::default()
        },
    )
    .unwrap();

    write_approval(&journal, "retain_complete");
    let retained = save_apple(
        &source,
        &journal,
        &AppleImportOptions {
            confirm_body_save: true,
            ..AppleImportOptions::default()
        },
    )
    .unwrap();
    assert!(!retained.skipped());
    assert_ne!(retained.bundle_id(), discarded.bundle_id());
    assert_eq!(
        fs::read(
            journal
                .join("imports")
                .join(retained.bundle_id().unwrap())
                .join("raw/auxiliary.txt")
        )
        .unwrap(),
        b"synthetic auxiliary v1\n"
    );

    fs::write(source.join("auxiliary.txt"), b"synthetic auxiliary v2\n").unwrap();
    let changed = save_apple(
        &source,
        &journal,
        &AppleImportOptions {
            confirm_body_save: true,
            ..AppleImportOptions::default()
        },
    )
    .unwrap();
    assert!(!changed.skipped());
    assert_ne!(changed.bundle_id(), retained.bundle_id());
    assert_eq!(
        fs::read(
            journal
                .join("imports")
                .join(changed.bundle_id().unwrap())
                .join("raw/auxiliary.txt")
        )
        .unwrap(),
        b"synthetic auxiliary v2\n"
    );
}

#[test]
fn dtd_export_and_inclusive_date_window_are_supported_without_writes_in_preview() {
    let directory = fixture("tests/fixtures/importers/health/apple_health_synthetic_dtd");
    let preview = preview_apple(&directory, Some("2026-04-11"), Some("2026-04-11")).unwrap();
    assert!(preview.rows() > 0);
    assert_eq!(preview.days(), ["20260411"]);
}

#[test]
fn gate_fails_before_creating_an_import_bundle() {
    let temporary = TempDir::new();
    let journal = temporary.path().join("journal");
    fs::create_dir(&journal).unwrap();
    let source = fixture("tests/fixtures/importers/health/apple_health_synthetic");
    let error = save_apple(&source, &journal, &AppleImportOptions::default()).unwrap_err();
    assert_eq!(error.stage(), "per_run_confirmation_missing");
    assert!(!journal.join("imports").exists());
}

#[cfg(unix)]
#[test]
fn intermediate_source_symlinks_are_refused_before_preview_or_snapshot() {
    use std::os::unix::fs::symlink;

    let temporary = TempDir::new();
    let journal = temporary.path().join("journal");
    let source = temporary.path().join("source");
    let outside = temporary.path().join("outside/apple_health_export");
    fs::create_dir(&journal).unwrap();
    fs::create_dir(&source).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::copy(
        fixture(
            "tests/fixtures/importers/health/apple_health_synthetic/apple_health_export/export.xml",
        ),
        outside.join("export.xml"),
    )
    .unwrap();
    symlink(&outside, source.join("apple_health_export")).unwrap();

    let detection = detect_apple_source(&source).expect_err("detection must reject the symlink");
    assert_eq!(detection.stage(), "source_symlink");
    let preview = preview_apple(&source, None, None).expect_err("preview must reject the symlink");
    assert_eq!(preview.stage(), "source_symlink");

    write_approval(&journal, "retain_parsed");
    let save = save_apple(
        &source,
        &journal,
        &AppleImportOptions {
            confirm_body_save: true,
            ..AppleImportOptions::default()
        },
    )
    .expect_err("save must reject the symlink");
    assert_eq!(save.stage(), "source_symlink");
    assert!(fs::read_dir(journal.join("imports")).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .as_encoded_bytes()
            .starts_with(b"body-")
    }));
}

#[cfg(unix)]
#[test]
fn top_level_fifo_archive_is_refused_without_blocking() {
    use nix::sys::stat::Mode;
    use nix::unistd::mkfifo;

    let temporary = TempDir::new();
    let source = temporary.path().join("synthetic.zip");
    mkfifo(&source, Mode::S_IRUSR | Mode::S_IWUSR).unwrap();

    let detection = detect_apple_source(&source).expect_err("FIFO detection must fail closed");
    assert_eq!(detection.stage(), "source_symlink");
    let preview = preview_apple(&source, None, None).expect_err("FIFO preview must fail");
    assert_eq!(preview.stage(), "archive");
}

#[test]
fn concurrent_non_force_saves_publish_one_immutable_bundle() {
    let temporary = TempDir::new();
    let journal = temporary.path().join("journal");
    fs::create_dir(&journal).unwrap();
    write_approval(&journal, "retain_parsed");
    let source = fixture("tests/fixtures/importers/health/apple_health_synthetic.zip");
    let barrier = Arc::new(Barrier::new(3));
    let mut threads = Vec::new();
    for _ in 0..2 {
        let barrier = Arc::clone(&barrier);
        let journal = journal.clone();
        let source = source.clone();
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            save_apple(
                &source,
                &journal,
                &AppleImportOptions {
                    confirm_body_save: true,
                    ..AppleImportOptions::default()
                },
            )
            .unwrap()
        }));
    }
    barrier.wait();
    let reports = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(reports.iter().filter(|report| report.skipped()).count(), 1);
    assert_eq!(reports[0].bundle_id(), reports[1].bundle_id());
    let bundle_count = fs::read_dir(journal.join("imports"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("body-"))
        .count();
    assert_eq!(bundle_count, 1);
}

#[test]
fn retry_removes_crash_orphans_before_publishing_body_history() {
    let temporary = TempDir::new();
    let journal = temporary.path().join("journal");
    let imports = journal.join("imports");
    fs::create_dir_all(&imports).unwrap();
    write_approval(&journal, "retain_parsed");
    let stale = [
        ".tmp-apple-source-00000000000000000000000000000000",
        "..tmp-apple-source-11111111111111111111111111111111.staging.42_7.tmp",
        ".body-01J9ZK2F5M7Q8R3S4T6V0W1X2Y.staging.42_8.tmp",
    ];
    for name in stale {
        fs::create_dir(imports.join(name)).unwrap();
        fs::write(imports.join(name).join("owner-body-sentinel"), b"synthetic").unwrap();
    }

    let report = save_apple(
        &fixture("tests/fixtures/importers/health/apple_health_synthetic.zip"),
        &journal,
        &AppleImportOptions {
            confirm_body_save: true,
            ..AppleImportOptions::default()
        },
    )
    .unwrap();
    assert!(report.bundle_id().is_some());
    for name in stale {
        assert!(
            !imports.join(name).exists(),
            "stale residue survived: {name}"
        );
    }
}

#[cfg(unix)]
#[test]
fn complete_retention_rejects_backslash_names_before_raw_publication() {
    let temporary = TempDir::new();
    let journal = temporary.path().join("journal");
    let source = temporary.path().join("apple-export");
    fs::create_dir(&journal).unwrap();
    fs::create_dir(&source).unwrap();
    fs::copy(
        fixture(
            "tests/fixtures/importers/health/apple_health_synthetic/apple_health_export/export.xml",
        ),
        source.join("export.xml"),
    )
    .unwrap();
    fs::write(source.join("..\\outside"), b"synthetic sentinel").unwrap();
    write_approval(&journal, "retain_complete");

    let error = save_apple(
        &source,
        &journal,
        &AppleImportOptions {
            confirm_body_save: true,
            ..AppleImportOptions::default()
        },
    )
    .unwrap_err();
    assert_eq!(error.stage(), "source_name");
    assert!(!journal.join("imports/body-escape").exists());
    assert!(fs::read_dir(journal.join("imports")).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .as_encoded_bytes()
            .starts_with(b"body-")
    }));
}

#[test]
fn canonical_row_over_replay_limit_is_refused_before_bundle_publication() {
    let temporary = TempDir::new();
    let journal = temporary.path().join("journal");
    let source = temporary.path().join("apple-export");
    fs::create_dir(&journal).unwrap();
    fs::create_dir(&source).unwrap();
    let source_name = "é".repeat(180_000);
    fs::write(
        source.join("export.xml"),
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<HealthData>
  <Record type="HKQuantityTypeIdentifierStepCount" sourceName="{source_name}" unit="count" value="1" startDate="2026-01-02 08:00:00 -0700" endDate="2026-01-02 08:01:00 -0700"/>
</HealthData>
"#,
        ),
    )
    .unwrap();
    write_approval(&journal, "discard");

    let error = save_apple(
        &source,
        &journal,
        &AppleImportOptions {
            confirm_body_save: true,
            ..AppleImportOptions::default()
        },
    )
    .unwrap_err();
    assert_eq!(error.kind(), BodyIngestErrorKind::Source);
    assert_eq!(error.stage(), "row_frame_limit");
    assert!(fs::read_dir(journal.join("imports")).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .as_encoded_bytes()
            .starts_with(b"body-")
    }));
}
