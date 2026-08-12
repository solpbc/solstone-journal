// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use solstone_core_import::{OuraConnectRequest, connect_oura};

#[test]
fn connect_routes_to_the_native_owner_before_any_retired_file_import_path() {
    let error = connect_oura(&OuraConnectRequest {
        journal_root: PathBuf::from("relative-journal"),
        timeout_seconds: 1,
    })
    .unwrap_err();
    assert_eq!(error.stage(), "target_journal_not_absolute");
}
