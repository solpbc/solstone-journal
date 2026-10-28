// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use solstone_core_speaker_resolve::discovery_helper::drive_discovery_cluster_helper_after_ready;

const TIMEOUT: Duration = Duration::from_millis(500);
const TERMINATE_GRACE: Duration = Duration::from_millis(1500);

struct HelperGuard {
    _dir: tempfile::TempDir,
}

impl HelperGuard {
    fn script(body: &str) -> (Self, std::path::PathBuf, std::path::PathBuf) {
        let dir = tempfile::TempDir::new_in("/var/tmp").expect("helper dir");
        let receipt = dir.path().join("receipt");
        let path = dir.path().join("helper");
        let script = format!(
            "#!/bin/sh\nRECEIPT='{}'\nprintf 'start %s\\n' \"$$\" >> \"$RECEIPT\"\n{body}\n",
            receipt.display()
        );
        fs::write(&path, script).expect("helper");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("mode");
        (Self { _dir: dir }, path, receipt)
    }
}

fn start_pid(receipt: &str) -> i32 {
    receipt
        .lines()
        .find_map(|line| line.strip_prefix("start ")?.parse().ok())
        .expect("start pid")
}

fn wait_ready(receipt: &std::path::Path) -> Result<Instant, String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if fs::read_to_string(receipt).is_ok_and(|text| text.lines().any(|line| line == "ready")) {
            return Ok(Instant::now());
        }
        if Instant::now() >= deadline {
            return Err("helper did not become ready".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn drive_ready(
    helper: &std::path::Path,
    receipt: &std::path::Path,
    timeout: Duration,
    grace: Duration,
    limit: usize,
) -> (Result<Vec<i64>, (String, String)>, Duration) {
    let mut ready_at = None;
    let result = drive_discovery_cluster_helper_after_ready(helper, timeout, grace, limit, || {
        ready_at = Some(wait_ready(receipt)?);
        Ok(())
    });
    (result, ready_at.expect("helper readiness").elapsed())
}

#[test]
fn oversized_stdout_is_rejected_while_the_helper_is_running() {
    let (_guard, helper, receipt) = HelperGuard::script(
        "cat >/dev/null\nhead -c 1048577 /dev/zero\nprintf 'ready\\n' >> \"$RECEIPT\"\nwhile :; do :; done",
    );
    let timeout = Duration::from_secs(5);
    let (result, elapsed) = drive_ready(
        &helper,
        &receipt,
        timeout,
        Duration::from_millis(20),
        1024 * 1024,
    );
    assert_eq!(result.unwrap_err().1, "stdout-too-large");
    assert!(
        elapsed < Duration::from_secs(1),
        "rejection must be output-driven, not timeout-driven: {elapsed:?} vs {timeout:?}"
    );
}

#[test]
fn timeout_terminates_a_responsive_helper() {
    let (_guard, helper, receipt) = HelperGuard::script(
        "trap 'printf \"term %s\\n\" \"$$\" >> \"$RECEIPT\"; exit 0' TERM\ncat >/dev/null\nprintf 'ready\\n' >> \"$RECEIPT\"\nwhile :; do :; done",
    );
    let (result, elapsed) = drive_ready(&helper, &receipt, TIMEOUT, TERMINATE_GRACE, 1024);
    assert_eq!(result.unwrap_err().1, "timeout");
    let text = fs::read_to_string(receipt).expect("receipt");
    assert!(text.contains("term "), "{text}");
    // kill() is only reached after terminate_and_reap's grace poll expires
    // (`Instant::now() + grace`), so finishing before timeout + grace proves
    // SIGKILL was not reached.
    assert!(
        elapsed < TIMEOUT + TERMINATE_GRACE,
        "TERM-only exit must finish before the grace poll can reach kill(): {elapsed:?}"
    );
}

#[test]
fn timeout_kills_after_grace_when_term_is_ignored() {
    let (_guard, helper, receipt) = HelperGuard::script(
        "trap 'printf \"term %s\\n\" \"$$\" >> \"$RECEIPT\"' TERM\ncat >/dev/null\nprintf 'ready\\n' >> \"$RECEIPT\"\nwhile :; do :; done",
    );
    let (result, elapsed) = drive_ready(&helper, &receipt, TIMEOUT, TERMINATE_GRACE, 1024);
    assert_eq!(result.unwrap_err().1, "timeout");
    let text = fs::read_to_string(receipt).expect("receipt");
    assert!(text.contains("start "), "{text}");
    assert!(text.contains("term "), "{text}");
    // Ignoring TERM forces the grace poll to expire (`now < deadline`) before
    // kill()+wait(), so elapsed cannot be shorter than timeout + grace.
    assert!(
        elapsed >= TIMEOUT + TERMINATE_GRACE,
        "ignored TERM must wait the full grace before KILL: {elapsed:?}"
    );
    let pid = start_pid(&text);
    assert_eq!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH),
        "reaped child must not answer signal 0"
    );
}
