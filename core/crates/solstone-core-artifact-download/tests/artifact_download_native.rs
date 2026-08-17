// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::File;

use solstone_core_artifact_download::clear_macos_quarantine;

#[cfg(target_os = "macos")]
#[test]
fn clearing_quarantine_is_recursive_and_idempotent() {
    let directory = tempfile::tempdir().unwrap();
    let nested = directory.path().join("nested");
    std::fs::create_dir(&nested).unwrap();
    let asset = nested.join("asset");
    File::create(&asset).unwrap();
    let written = std::process::Command::new("xattr")
        .args(["-w", "com.apple.quarantine", "0083;test"])
        .arg(&asset)
        .status()
        .unwrap();
    assert!(written.success());
    assert!(
        std::process::Command::new("xattr")
            .args(["-p", "com.apple.quarantine"])
            .arg(&asset)
            .output()
            .unwrap()
            .status
            .success()
    );

    clear_macos_quarantine(directory.path()).unwrap();
    assert!(
        !std::process::Command::new("xattr")
            .args(["-p", "com.apple.quarantine"])
            .arg(&asset)
            .output()
            .unwrap()
            .status
            .success()
    );
    clear_macos_quarantine(directory.path()).unwrap();
}

#[cfg(not(target_os = "macos"))]
#[test]
fn clearing_quarantine_is_a_no_op() {
    let directory = tempfile::tempdir().unwrap();
    let asset = directory.path().join("asset");
    File::create(&asset).unwrap();
    clear_macos_quarantine(directory.path()).unwrap();
    assert!(asset.is_file());
}
