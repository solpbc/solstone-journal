// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use solstone_core_convey_shell::drive_discovery_cluster_helper;

struct HelperGuard {
    dir: tempfile::TempDir,
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
        (Self { dir }, path, receipt)
    }
}

impl Drop for HelperGuard {
    fn drop(&mut self) {
        let receipt = self.dir.path().join("receipt");
        if let Ok(text) = fs::read_to_string(receipt) {
            for line in text.lines() {
                if let Some(pid) = line.strip_prefix("start ")
                    && let Ok(pid) = pid.parse::<i32>()
                {
                    let _ = nix::sys::signal::kill(
                        nix::unistd::Pid::from_raw(pid),
                        nix::sys::signal::Signal::SIGKILL,
                    );
                }
            }
        }
    }
}

#[test]
fn oversized_stdout_is_rejected_while_the_helper_is_running() {
    let (_guard, helper, _receipt) =
        HelperGuard::script("cat >/dev/null\nhead -c 1048577 /dev/zero");
    let result = drive_discovery_cluster_helper(
        &helper,
        Duration::from_secs(1),
        Duration::from_millis(20),
        1024 * 1024,
    );
    assert_eq!(result.unwrap_err().1, "stdout-too-large");
}

#[test]
fn timeout_terminates_a_responsive_helper() {
    let (_guard, helper, receipt) = HelperGuard::script(
        "trap 'printf \"term %s\\n\" \"$$\" >> \"$RECEIPT\"; exit 0' TERM\ncat >/dev/null\nwhile :; do :; done",
    );
    let result = drive_discovery_cluster_helper(
        &helper,
        Duration::from_millis(20),
        Duration::from_millis(500),
        1024,
    );
    assert_eq!(result.unwrap_err().1, "timeout");
    let text = fs::read_to_string(receipt).expect("receipt");
    assert!(text.contains("term "), "{text}");
}

#[test]
fn timeout_kills_after_grace_when_term_is_ignored() {
    let (_guard, helper, receipt) =
        HelperGuard::script("trap '' TERM\ncat >/dev/null\nwhile :; do :; done");
    let result = drive_discovery_cluster_helper(
        &helper,
        Duration::from_millis(20),
        Duration::from_millis(30),
        1024,
    );
    assert_eq!(result.unwrap_err().1, "timeout");
    let text = fs::read_to_string(receipt).expect("receipt");
    assert!(text.contains("start "), "{text}");
    assert!(!text.contains("term "), "{text}");
}
