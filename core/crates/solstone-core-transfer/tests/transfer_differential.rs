// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;
use std::process::Command;

use solstone_core_transfer::{ExportRequest, ImportRequest, export, import};

fn write_file(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().expect("parent")).expect("parent directories");
    fs::write(path, bytes).expect("file");
}

fn python() -> std::path::PathBuf {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root");
    repository.join(".venv/bin/python")
}

fn run_python(journal: &Path, output: &Path, script: &str) {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root");
    let result = Command::new(python())
        .arg("-c")
        .arg(script)
        .env("PYTHONPATH", repository)
        .env("SOLSTONE_JOURNAL", journal)
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .env("TRANSFER_ARCHIVE", output)
        .output()
        .expect("Python reference");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn native_export_and_import_match_the_python_v1_contract() {
    let source = tempfile::tempdir().expect("source");
    let segment = source.path().join("chronicle/20260203/audio/120000_30");
    write_file(&segment.join("stream.json"), b"stream");
    write_file(&segment.join("device.json"), b"device");

    let native_archive = source.path().join("native.tgz");
    export(
        source.path(),
        ExportRequest {
            day: "20260203".to_owned(),
            output: native_archive.clone(),
        },
    )
    .expect("native export");
    let python_archive = source.path().join("python.tgz");
    run_python(
        source.path(),
        &python_archive,
        "from pathlib import Path; from solstone.observe.transfer import create_archive; create_archive('20260203', Path(__import__('os').environ['TRANSFER_ARCHIVE']))",
    );

    let native_manifest = manifest_without_volatile(&native_archive);
    let python_manifest = manifest_without_volatile(&python_archive);
    assert_eq!(native_manifest, python_manifest);

    let python_destination = tempfile::tempdir().expect("Python destination");
    run_python(
        python_destination.path(),
        &native_archive,
        "from pathlib import Path; from solstone.observe.transfer import import_archive; import_archive(Path(__import__('os').environ['TRANSFER_ARCHIVE']))",
    );
    assert_eq!(
        fs::read(
            python_destination
                .path()
                .join("chronicle/20260203/audio/120000_30/device.json")
        )
        .expect("Python imported native file"),
        b"device"
    );

    let destination = tempfile::tempdir().expect("destination");
    import(
        destination.path(),
        ImportRequest {
            archive: python_archive,
            dry_run: false,
        },
    )
    .expect("native imports Python archive");
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

fn manifest_without_volatile(path: &Path) -> serde_json::Value {
    let file = fs::File::open(path).expect("archive");
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let mut entries = archive.entries().expect("entries");
    let mut entry = entries.next().expect("manifest entry").expect("entry");
    let mut value: serde_json::Value = serde_json::from_reader(&mut entry).expect("manifest JSON");
    value
        .as_object_mut()
        .expect("manifest object")
        .remove("created_at");
    value
        .as_object_mut()
        .expect("manifest object")
        .remove("host");
    value
}
