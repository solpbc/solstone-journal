// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The sol tool's command contract on Windows: output capture, exit mapping,
//! and a timeout or root exit that leaves no descendant behind.
#![cfg(windows)]

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use solstone_core_cogitate_tools::sol_execution_test_hooks::run_with_timeout;

/// A bare name: resolution must find `powershell.exe` on PATH.
fn powershell(script: &str) -> Vec<String> {
    vec![
        "powershell".to_owned(),
        "-NoProfile".to_owned(),
        "-NonInteractive".to_owned(),
        "-Command".to_owned(),
        script.to_owned(),
    ]
}

fn start_descendant(receipt: &Path, window: &str) -> String {
    format!(
        "$p = Start-Process -PassThru {window} -FilePath powershell -ArgumentList '-NoProfile','-Command','Start-Sleep 120'; \
         Set-Content -NoNewline -Path '{}' -Value $p.Id",
        receipt.display()
    )
}

fn process_is_live(pid: u32) -> bool {
    let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .expect("run tasklist");
    assert!(output.status.success(), "tasklist failed");
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}

fn assert_descendant_exited(receipt: &Path) {
    let pid = fs::read_to_string(receipt)
        .expect("read descendant receipt")
        .trim()
        .parse::<u32>()
        .expect("descendant receipt is a PID");
    let deadline = Instant::now() + Duration::from_secs(5);
    while process_is_live(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        !process_is_live(pid),
        "receipt-bearing descendant {pid} survived Job cleanup"
    );
}

#[test]
fn timeout_preserves_partial_output_and_stops_the_whole_job() {
    let root = tempfile::tempdir().expect("create process fixture root");
    let receipt = root.path().join("descendant.pid");
    let script = format!(
        "{}; [Console]::Out.Write('partial'); [Console]::Out.Flush(); \
         [Console]::Error.Write('error'); [Console]::Error.Flush(); Start-Sleep 120",
        start_descendant(&receipt, "-WindowStyle Hidden")
    );
    let started = Instant::now();
    let actual = run_with_timeout(&powershell(&script), root.path(), Duration::from_secs(15))
        .expect("command handling");
    assert!(started.elapsed() < Duration::from_secs(45));
    assert!(actual.is_error);
    assert_eq!(
        actual.text,
        "stdout:\npartial\n\nstderr:\nerror\n\ntimeout: command exceeded 30s"
    );
    assert_descendant_exited(&receipt);
}

#[test]
fn exited_root_stops_a_descendant_that_may_hold_the_output_pipes() {
    let root = tempfile::tempdir().expect("create process fixture root");
    let receipt = root.path().join("descendant.pid");
    let script = format!(
        "{}; [Console]::Out.Write('root')",
        start_descendant(&receipt, "-NoNewWindow")
    );
    let started = Instant::now();
    let actual = run_with_timeout(&powershell(&script), root.path(), Duration::from_secs(60))
        .expect("collect exited root output");
    assert!(started.elapsed() < Duration::from_secs(45));
    assert!(!actual.is_error);
    assert_eq!(actual.text, "stdout:\nroot");
    assert_descendant_exited(&receipt);
}

#[test]
fn real_command_preserves_cwd_output_and_exit_mapping() {
    let root = tempfile::Builder::new()
        .prefix("solstone-cogitate-command-contract-")
        .tempdir()
        .expect("create command fixture root");
    let actual = run_with_timeout(
        &powershell(
            "[Console]::Out.Write((Get-Location).ProviderPath); [Console]::Error.Write('error'); exit 7",
        ),
        root.path(),
        Duration::from_secs(30),
    )
    .expect("collect command output");
    assert!(actual.is_error);
    let leaf = root
        .path()
        .file_name()
        .expect("fixture root has a leaf")
        .to_string_lossy();
    assert!(
        actual.text.starts_with("stdout:\n") && actual.text.contains(leaf.as_ref()),
        "{}",
        actual.text
    );
    assert!(
        actual.text.ends_with("\n\nstderr:\nerror\n\nexit_code: 7"),
        "{}",
        actual.text
    );
}

#[test]
fn missing_command_is_command_not_found() {
    let actual = run_with_timeout(
        &["solstone-no-such-command".to_owned()],
        Path::new("."),
        Duration::from_secs(5),
    )
    .expect("command handling");
    assert!(actual.is_error);
    assert_eq!(actual.text, "command_not_found: solstone-no-such-command");
}

/// Cortex stops a timed-out or cancelled talent through the same facade:
/// `terminate()` retires the whole Job and never calls the terminate closure.
#[test]
fn facade_stop_retires_descendants_without_the_terminate_closure() {
    use solstone_core_system::process::{
        BoxedTerminateFn, CommandLaunchRequest, Disposition, launch_command,
    };

    let root = tempfile::tempdir().expect("create process fixture root");
    let receipt = root.path().join("descendant.pid");
    let script = format!(
        "{}; Start-Sleep 120",
        start_descendant(&receipt, "-WindowStyle Hidden")
    );
    let terminate: BoxedTerminateFn =
        Box::new(|_child, _timeout| panic!("Windows stop must not use the terminate closure"));
    let mut authority = launch_command(
        Disposition::IndependentBoundedHelper {
            timeout: Duration::from_secs(120),
        },
        CommandLaunchRequest {
            read_file_grants: Vec::new(),
            program: Path::new(&std::env::var_os("SystemRoot").expect("SystemRoot"))
                .join(r"System32\WindowsPowerShell\v1.0\powershell.exe")
                .into_os_string(),
            arguments: powershell(&script)[1..].iter().map(Into::into).collect(),
            environment: Default::default(),
            current_dir: Some(root.path().to_path_buf()),
            process_group: true,
            stdin_piped: false,
            stdout_piped: false,
            stderr_piped: false,
        },
        terminate,
    )
    .expect("launch owned command");
    let deadline = Instant::now() + Duration::from_secs(60);
    while !receipt.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(receipt.exists(), "descendant receipt never appeared");
    // Set-Content may still be writing; wait for a complete PID.
    std::thread::sleep(Duration::from_millis(500));
    authority
        .terminate(Duration::from_secs(10))
        .expect("stop the owned Job");
    // Cortex's reaper keeps polling after a stop; the stopped Job reports exit.
    authority.wait().expect("observe the stopped Job");
    assert_descendant_exited(&receipt);
}
