// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::time::Duration;

use solstone_core_import_web::{MetadataCommandOutcome, MetadataCommandPlan, run_metadata_command};

const STUB: &str = env!("CARGO_BIN_EXE_solstone-import-web-metadata-stub");
const AUTHORED_TIMEOUT: Duration = Duration::from_millis(100);
const AUTHORED_STDOUT: &[u8] = br#"[{"CreateDate":"2026:08:01 12:34:56"}]"#;

fn plan(args: &[&str]) -> MetadataCommandPlan {
    MetadataCommandPlan {
        program: STUB.into(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        path: "photo.jpg".into(),
        timeout: AUTHORED_TIMEOUT,
    }
}

#[test]
fn success_emits_authored_exiftool_json() {
    assert_eq!(
        run_metadata_command(&plan(&["success"])),
        MetadataCommandOutcome::Completed(AUTHORED_STDOUT.to_vec())
    );
}

#[test]
fn malformed_exits_zero_with_non_json() {
    assert_eq!(
        run_metadata_command(&plan(&["malformed"])),
        MetadataCommandOutcome::Completed(b"not-json".to_vec())
    );
}

#[test]
fn unavailable_is_non_zero_exit() {
    assert_eq!(
        run_metadata_command(&plan(&["unavailable"])),
        MetadataCommandOutcome::Unavailable
    );
}

#[test]
fn stall_times_out_after_marker_and_is_reaped() {
    let dir = tempfile::TempDir::new().unwrap();
    let marker = dir.path().join("started");
    let outcome = run_metadata_command(&plan(&["stall", marker.to_str().unwrap()]));
    assert_eq!(outcome, MetadataCommandOutcome::TimedOut);
    assert!(marker.exists());
}
