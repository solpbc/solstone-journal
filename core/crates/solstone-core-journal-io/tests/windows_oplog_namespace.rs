// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(windows)]

use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::Path;
use std::process::Command;

use solstone_core_journal_io::JournalRoot;
use solstone_core_journal_io::operational_log::{
    OplogNamespacePrimitive, admit_day_health_directory, run_with_oplog_namespace_barrier,
};
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

const DAY: &str = "20260901";

fn chronicle_path(root: &Path) -> std::path::PathBuf {
    root.join("chronicle")
}

fn day_path(root: &Path) -> std::path::PathBuf {
    chronicle_path(root).join(DAY)
}

fn health_path(root: &Path) -> std::path::PathBuf {
    day_path(root).join("health")
}

fn create_junction(link: &Path, target: &Path) {
    let output = Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .expect("launch cmd.exe for native junction fixture");
    assert!(
        output.status.success(),
        "create junction fixture {} -> {}: status={} stdout={} stderr={}",
        link.display(),
        target.display(),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn after_day_junction_health_is_unsafe() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let root = temporary.path().to_path_buf();
    let target = root.join("junction-target");
    fs::create_dir(&target).unwrap();
    let error = run_with_oplog_namespace_barrier(
        OplogNamespacePrimitive::AfterDay,
        {
            let root = root.clone();
            let target = target.clone();
            move || {
                create_junction(&health_path(&root), &target);
            }
        },
        || admit_day_health_directory(JournalRoot::open(&root).unwrap(), DAY),
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "oplog_namespace_health_unsafe");
    assert!(day_path(&root).is_dir());
    assert_ne!(
        fs::symlink_metadata(health_path(&root))
            .unwrap()
            .file_attributes()
            & FILE_ATTRIBUTE_REPARSE_POINT,
        0
    );
    assert!(
        fs::read_dir(&target).unwrap().next().is_none(),
        "admit must not create through the planted health junction"
    );
}

#[test]
#[ignore = "source-origin marker for the native Windows gate"]
fn journal_win_ci_windows_oplog_namespace_marker() {
    println!("JOURNAL_WIN_CI_TARGET_WINDOWS_OPLOG_NAMESPACE=executed/pass");
}
