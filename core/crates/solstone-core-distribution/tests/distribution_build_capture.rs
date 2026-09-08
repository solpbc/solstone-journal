// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(any(unix, windows))]

use std::fs;
#[cfg(windows)]
use std::path::PathBuf;
use std::process::Command;

use solstone_core_distribution::produce::windows_build::capture_build_command_for_test as capture_build_command;

#[cfg(windows)]
fn powershell(script: &str) -> Command {
    let path = PathBuf::from(std::env::var_os("SystemRoot").expect("Windows system root"))
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let mut command = Command::new(path);
    command.args(["-NoProfile", "-NonInteractive", "-Command", script]);
    command
}

#[cfg(any(unix, windows))]
#[test]
fn live_capture_preserves_raw_bytes_and_actual_nonzero_exit() {
    let root = tempfile::tempdir().unwrap();
    let stdout = root.path().join("stdout");
    let stderr = root.path().join("stderr");
    let exit = root.path().join("exit.json");
    #[cfg(unix)]
    let mut command = Command::new("sh");
    #[cfg(unix)]
    command.args([
        "-c",
        "printf '\\377\\000\\015\\012'; printf 'error\\015\\012' >&2; exit 17",
    ]);
    #[cfg(windows)]
    let mut command = powershell(
        "[Console]::OpenStandardOutput().Write([byte[]]@(255,0,13,10),0,4); [Console]::OpenStandardError().Write([byte[]]@(101,114,114,111,114,13,10),0,7); exit 17",
    );
    let status = capture_build_command(&mut command, &stdout, &stderr, &exit).unwrap();
    assert_eq!(status.code(), Some(17));
    assert_eq!(fs::read(stdout).unwrap(), [255, 0, 13, 10]);
    assert_eq!(fs::read(stderr).unwrap(), b"error\r\n");
    assert_eq!(fs::read(exit).unwrap(), b"17");
}

#[cfg(any(unix, windows))]
#[test]
fn live_capture_waits_for_inherited_writer_and_refuses_evidence_failure() {
    let root = tempfile::tempdir().unwrap();
    let stdout = root.path().join("stdout");
    let stderr = root.path().join("stderr");
    let exit = root.path().join("exit.json");
    #[cfg(unix)]
    let mut command = Command::new("sh");
    #[cfg(unix)]
    command.args(["-c", "(sleep 0.2; printf 'late') & exit 0"]);
    #[cfg(windows)]
    let mut command = powershell(
        r#"$p = New-Object Diagnostics.ProcessStartInfo; $p.FileName = Join-Path $PSHOME 'powershell.exe'; $p.UseShellExecute = $false; $p.Arguments = '-NoProfile -NonInteractive -Command "Start-Sleep -Milliseconds 300; [Console]::Write(''late'')"'; $child = [Diagnostics.Process]::Start($p); $child.Dispose(); exit 0"#,
    );
    let started = std::time::Instant::now();
    assert_eq!(
        capture_build_command(&mut command, &stdout, &stderr, &exit)
            .unwrap()
            .code(),
        Some(0)
    );
    assert!(started.elapsed() >= std::time::Duration::from_millis(150));
    assert_eq!(fs::read(&stdout).unwrap(), b"late");
    assert_eq!(fs::read(&exit).unwrap(), b"0");

    // Deliberately occupied evidence leaf: preserve original bytes, still
    // settle both streams, and refuse any successful capture result.
    #[cfg(unix)]
    let mut command = Command::new("sh");
    #[cfg(unix)]
    command.args(["-c", "printf 'out'; printf 'err' >&2; exit 17"]);
    #[cfg(windows)]
    let mut command = powershell("[Console]::Write('out'); [Console]::Error.Write('err'); exit 17");
    let error = capture_build_command(
        &mut command,
        &root.path().join("out2"),
        &root.path().join("err2"),
        &exit,
    )
    .unwrap_err();
    assert!(error.contains("persist actual Cargo exit"));
    assert!(error.contains("Some(17)"));
    assert_eq!(fs::read(exit).unwrap(), b"0");
    assert_eq!(fs::read(root.path().join("out2")).unwrap(), b"out");
    assert_eq!(fs::read(root.path().join("err2")).unwrap(), b"err");
}
