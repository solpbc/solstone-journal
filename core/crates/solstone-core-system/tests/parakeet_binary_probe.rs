// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::time::{Duration, Instant};

use solstone_core_system::provider_runtime::{ParakeetCppReadiness, probe_parakeet_cpp_binary};
use tempfile::tempdir;

#[test]
fn binary_probe_classifies_openmp_loader_failure() {
    let root = tempdir().unwrap();
    let binary = root.path().join("parakeet-server");
    fs::write(
        &binary,
        "#!/bin/sh\necho 'libgomp.so.1 missing' >&2\nexit 1\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&binary).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&binary, permissions).unwrap();
    let readiness = probe_parakeet_cpp_binary(&binary, Duration::from_secs(1));
    assert!(
        matches!(
            readiness,
            ParakeetCppReadiness::OpenMpRuntimeUnavailable { .. }
        ),
        "unexpected readiness: {readiness:?}"
    );
}

#[test]
fn binary_probe_reaps_a_timed_out_child() {
    let root = tempdir().unwrap();
    let binary = root.path().join("parakeet-server");
    let pid = root.path().join("pid");
    fs::write(
        &binary,
        format!("#!/bin/sh\necho $$ > {}\nsleep 60\n", pid.display()),
    )
    .unwrap();
    let mut permissions = fs::metadata(&binary).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&binary, permissions).unwrap();

    let readiness = probe_parakeet_cpp_binary(&binary, Duration::from_secs(1));
    assert!(
        matches!(readiness, ParakeetCppReadiness::BinaryUnstartable { .. }),
        "unexpected readiness: {readiness:?}"
    );
    // ⚠ The child writes its pid and then sleeps; the probe's 1 s timeout can elapse
    // before the shell has flushed that write, so reading it straight away raced and
    // panicked with `NotFound` under parallel suite load. Wait for the file instead of
    // assuming it is there -- and fail with a sentence rather than an `unwrap`.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !pid.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let child_pid = fs::read_to_string(&pid).unwrap_or_else(|error| {
        panic!("the probed child must have recorded its pid before the probe reaped it: {error}; readiness: {readiness:?}")
    });
    let status = Command::new("sh")
        .args(["-c", &format!("kill -0 {} 2>/dev/null", child_pid.trim())])
        .status()
        .unwrap();
    assert!(!status.success(), "timed-out child must be reaped");
}
