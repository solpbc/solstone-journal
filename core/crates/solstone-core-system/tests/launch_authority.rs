// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use solstone_core_system::process::{Disposition, LaunchError, launch, launch_with};

fn process_is_gone(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return true;
    };
    matches!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH)
    )
}

fn wait_until_gone(pid: u32) {
    for _ in 0..200 {
        if process_is_gone(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("pid {pid} still alive");
}

fn kill_child(child: &mut Child, _: Duration) -> Result<(), LaunchError> {
    child.kill().map_err(LaunchError::Terminate)
}

fn recording_kill(
    flag: &Arc<AtomicBool>,
) -> Box<dyn FnMut(&mut Child, Duration) -> Result<(), LaunchError> + Send> {
    let flag = Arc::clone(flag);
    Box::new(move |child: &mut Child, timeout| {
        flag.store(true, Ordering::SeqCst);
        kill_child(child, timeout)
    })
}

fn spawn_sleep(seconds: &str) -> io::Result<Child> {
    Command::new("/bin/sleep").arg(seconds).spawn()
}

fn unwrap_launch_err<T>(result: Result<T, LaunchError>, what: &str) -> LaunchError {
    match result {
        Err(error) => error,
        Ok(_) => panic!("{what}: expected error"),
    }
}

#[test]
fn ac3_failed_confirm_reaps_the_spawned_child() {
    let terminated = Arc::new(AtomicBool::new(false));
    let result = launch_with(
        Disposition::IndependentLongLived,
        || spawn_sleep("5"),
        recording_kill(&terminated),
        |_| Ok(()),
        |_| Err(io::Error::other("forced confirm failure")),
    );
    let error = unwrap_launch_err(result, "confirm");
    let LaunchError::ConfirmationFailed { pid, .. } = error else {
        panic!("expected ConfirmationFailed, got {error:?}");
    };
    assert!(terminated.load(Ordering::SeqCst));
    wait_until_gone(pid);
}

#[test]
fn drop_reaps_a_raw_child_through_terminate_fn() {
    let terminated = Arc::new(AtomicBool::new(false));
    let authority = launch(
        Disposition::InheritedParentScope,
        || spawn_sleep("30"),
        recording_kill(&terminated),
    )
    .expect("launch sleep");
    let pid = authority.pid();
    drop(authority);
    assert!(terminated.load(Ordering::SeqCst));
    wait_until_gone(pid);
}

#[test]
fn inherited_scope_wait_with_output_returns_child_stdout() {
    let output = launch(
        Disposition::InheritedParentScope,
        || {
            Command::new("/bin/echo")
                .arg("ok")
                .stdout(Stdio::piped())
                .spawn()
        },
        Box::new(kill_child),
    )
    .expect("launch echo")
    .wait_with_output()
    .expect("wait");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "ok");
}
