// SPDX-License-Identifier: AGPL-3.0-only
//! Native process-tree containment, isolated by the full-tests Cargo feature.
#![cfg(windows)]

use solstone_core_repository_contracts::windows_suite::{spawn_process_tree, wait_bounded};
use std::process::Command;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

#[allow(unsafe_code)]
fn prove_descendant_cleanup(parent_waits: bool) {
    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("descendant.pid");
    let release = temp.path().join("parent.release");
    let script = temp.path().join("parent.ps1");
    std::fs::write(&script, concat!(
        "$ErrorActionPreference='Stop'\r\n",
        "$p=Start-Process $env:ComSpec -ArgumentList '/d /c ping -n 100 127.0.0.1 >nul' -PassThru\r\n",
        "[IO.File]::WriteAllText(($env:DESCENDANT_PID+'.tmp'),[string]$p.Id)\r\n",
        "[IO.File]::Move(($env:DESCENDANT_PID+'.tmp'),$env:DESCENDANT_PID)\r\n",
        "while(!(Test-Path -LiteralPath $env:PARENT_RELEASE)){Start-Sleep -Milliseconds 10}\r\n"
    )).unwrap();
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script)
        .env("DESCENDANT_PID", &pid_file)
        .env("PARENT_RELEASE", &release);
    let mut tree = spawn_process_tree(&mut command).unwrap();
    let start = Instant::now();
    while !pid_file.exists() && start.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(10));
    }
    let pid: u32 = std::fs::read_to_string(&pid_file)
        .expect("descendant must start")
        .parse()
        .unwrap();
    // Hold the actual lifetime, so PID reuse cannot satisfy the cleanup proof.
    // SAFETY: request only synchronization on the measured PID, then retain
    // that handle through cleanup and close it once the wait completes.
    let descendant = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    assert!(!descendant.is_null(), "descendant handle must open");
    if !parent_waits {
        std::fs::write(&release, b"release").unwrap();
    }
    let result = wait_bounded(
        &mut tree,
        Duration::from_secs(if parent_waits { 1 } else { 20 }),
        Instant::now,
        std::thread::sleep,
    );
    if parent_waits {
        assert_eq!(result.unwrap_err(), "timed out");
    } else {
        assert!(result.unwrap().success());
    }
    drop(tree);
    let stopped = unsafe { WaitForSingleObject(descendant, 5000) };
    unsafe {
        CloseHandle(descendant);
    }
    assert_eq!(
        stopped, WAIT_OBJECT_0,
        "descendant lifetime survived cleanup"
    );
}

#[test]
fn timeout_terminates_the_native_descendant_lifetime() {
    prove_descendant_cleanup(true);
}

#[test]
fn parent_completion_terminates_the_native_descendant_lifetime() {
    prove_descendant_cleanup(false);
}
