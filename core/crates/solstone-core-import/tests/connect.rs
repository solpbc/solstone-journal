// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;

use solstone_core_import::{ImportError, connect_backend};

#[test]
fn oura_route_reaches_native_config_validation_before_oauth() {
    let temporary = tempfile::tempdir().unwrap();
    let journal = temporary.path().canonicalize().unwrap();

    let error = connect_backend(&journal, "oura").unwrap_err();

    assert!(matches!(
        error,
        ImportError::Refusal {
            kind: "journal_config_missing",
            exit_code: 65,
            ..
        }
    ));
}

#[test]
fn non_oura_refusal_has_no_file_side_effects() {
    let temporary = tempfile::tempdir().unwrap();
    let journal = temporary.path().canonicalize().unwrap();
    let before = directory_names(&journal);

    let error = connect_backend(&journal, "audio").unwrap_err();

    assert!(matches!(
        error,
        ImportError::Refusal {
            kind: "unsupported_connect_backend",
            ..
        }
    ));
    assert_eq!(directory_names(&journal), before);
    assert!(!journal.join("imports").exists());
}

fn directory_names(directory: &std::path::Path) -> Vec<String> {
    let mut names = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}
