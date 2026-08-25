// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsString;
use std::fs;
use std::time::Duration;

use nix::errno::Errno;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use solstone_core_import_web::{MetadataCommandOutcome, MetadataCommandPlan, run_metadata_command};

const STUB: &str = env!("CARGO_BIN_EXE_solstone-import-web-metadata-stub");
const COMPLETION_TIMEOUT: Duration = Duration::from_secs(1);
const STALL_TIMEOUT: Duration = Duration::from_millis(100);
const AUTHORED_STDOUT: &[u8] = br#"[{"CreateDate":"2026:08:01 12:34:56"}]"#;

fn plan(args: impl IntoIterator<Item = impl Into<OsString>>) -> MetadataCommandPlan {
    plan_with_timeout(args, COMPLETION_TIMEOUT)
}

fn plan_with_timeout(
    args: impl IntoIterator<Item = impl Into<OsString>>,
    timeout: Duration,
) -> MetadataCommandPlan {
    MetadataCommandPlan {
        program: STUB.into(),
        args: args.into_iter().map(Into::into).collect(),
        path: "photo.jpg".into(),
        timeout,
    }
}

#[test]
fn success_emits_authored_exiftool_json() {
    assert_eq!(
        run_metadata_command(&plan(["success"])),
        MetadataCommandOutcome::Completed(AUTHORED_STDOUT.to_vec())
    );
}

#[test]
fn malformed_exits_zero_with_non_json() {
    assert_eq!(
        run_metadata_command(&plan(["malformed"])),
        MetadataCommandOutcome::Completed(b"not-json".to_vec())
    );
}

#[test]
fn unavailable_is_non_zero_exit() {
    assert_eq!(
        run_metadata_command(&plan(["unavailable"])),
        MetadataCommandOutcome::Unavailable
    );
}

#[test]
fn stall_times_out_after_marker_and_is_reaped() {
    let dir = tempfile::TempDir::new().unwrap();
    let marker = dir.path().join("started");
    let outcome = run_metadata_command(&plan_with_timeout(
        ["stall".into(), marker.as_os_str().to_os_string()],
        STALL_TIMEOUT,
    ));
    assert_eq!(outcome, MetadataCommandOutcome::TimedOut);
    let pid = fs::read_to_string(&marker)
        .expect("the child writes its PID before stalling")
        .parse::<i32>()
        .expect("the marker contains a process ID");
    assert_eq!(
        kill(Pid::from_raw(pid), None),
        Err(Errno::ESRCH),
        "the timed-out child must be killed and reaped before the runner returns"
    );
}
