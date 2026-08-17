// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use solstone_core_cogitate_tools::sol_execution_test_hooks::run_with_timeout;

fn shell(script: &str, extra: &[String]) -> Vec<String> {
    let mut argv = vec!["/bin/sh".to_owned(), "-c".to_owned(), script.to_owned()];
    argv.extend_from_slice(extra);
    argv
}

fn assert_process_exited(pid: i32) {
    let deadline = Instant::now() + Duration::from_secs(1);
    while nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err(),
        "receipt-bearing descendant {pid} survived group cleanup"
    );
}

#[test]
fn timeout_preserves_partial_output_and_cleans_the_process_group() {
    let root = tempfile::Builder::new()
        .prefix("solstone-cogitate-timeout-")
        .tempdir()
        .expect("create process fixture root");
    let receipt = root.path().join("descendant.pid");
    let argv = shell(
        "sleep 5 & child=$!; printf '%s' \"$child\" > \"$1\"; printf partial; printf error >&2; sleep 5",
        &[
            "solstone-cogitate-timeout".to_owned(),
            receipt.display().to_string(),
        ],
    );
    let started = Instant::now();
    let actual =
        run_with_timeout(&argv, root.path(), Duration::from_millis(50)).expect("command handling");
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(actual.is_error);
    assert_eq!(
        actual.text,
        "stdout:\npartial\n\nstderr:\nerror\n\ntimeout: command exceeded 30s"
    );
    let descendant = fs::read_to_string(&receipt)
        .expect("read descendant receipt")
        .parse::<i32>()
        .expect("descendant receipt is a PID");
    assert_process_exited(descendant);
}

#[test]
fn exited_root_cleans_a_descendant_that_holds_the_output_pipes() {
    let root = tempfile::Builder::new()
        .prefix("solstone-cogitate-root-exit-")
        .tempdir()
        .expect("create process fixture root");
    let receipt = root.path().join("descendant.pid");
    let argv = shell(
        "sleep 5 & child=$!; printf '%s' \"$child\" > \"$1\"; printf root",
        &[
            "solstone-cogitate-root-exit".to_owned(),
            receipt.display().to_string(),
        ],
    );
    let started = Instant::now();
    let actual = run_with_timeout(&argv, root.path(), Duration::from_secs(2))
        .expect("collect exited root output");
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!actual.is_error);
    assert_eq!(actual.text, "stdout:\nroot");

    let descendant = fs::read_to_string(&receipt)
        .expect("read descendant receipt")
        .parse::<i32>()
        .expect("descendant receipt is a PID");
    assert_process_exited(descendant);
}

#[test]
fn real_command_preserves_cwd_environment_output_and_exit_mapping() {
    let root = tempfile::Builder::new()
        .prefix("solstone-cogitate-command-contract-")
        .tempdir()
        .expect("create command fixture root");
    let inherited_home = env::var("HOME").expect("test host provides HOME");
    let argv = shell(
        "printf 'cwd=%s\\npath=%s\\nhome=%s' \"$PWD\" \"${PATH:+set}\" \"$HOME\"; printf error >&2; exit 7",
        &[],
    );
    let actual = run_with_timeout(&argv, root.path(), Duration::from_secs(2))
        .expect("collect command output");
    assert!(actual.is_error);
    assert_eq!(
        actual.text,
        format!(
            "stdout:\ncwd={}\npath=set\nhome={}\n\nstderr:\nerror\n\nexit_code: 7",
            root.path().display(),
            inherited_home
        )
    );
}

#[test]
fn real_command_fully_captures_then_presentation_truncates_each_stream() {
    let argv = shell(
        "i=0; while [ \"$i\" -lt 6001 ]; do printf x; printf y >&2; i=$((i + 1)); done",
        &[],
    );
    let actual = run_with_timeout(&argv, Path::new("."), Duration::from_secs(2))
        .expect("collect bounded large output");
    assert!(!actual.is_error);
    assert_eq!(
        actual.text,
        format!(
            "stdout:\n{}\n... [truncated]\n\nstderr:\n{}\n... [truncated]",
            "x".repeat(6000),
            "y".repeat(6000)
        )
    );
}
