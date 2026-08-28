// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(unix)]
use std::io;
#[cfg(unix)]
use std::process::{Child, Command, Stdio};
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use solstone_core_system::process::BoxedTerminateFn;
#[cfg(unix)]
use solstone_core_system::process::launch_managed_with;
#[cfg(unix)]
use solstone_core_system::process::{Disposition, LaunchError, launch, launch_with};

#[cfg(unix)]
fn process_is_gone(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return true;
    };
    matches!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH)
    )
}

#[cfg(unix)]
fn wait_until_gone(pid: u32) {
    for _ in 0..200 {
        if process_is_gone(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("pid {pid} still alive");
}

#[cfg(unix)]
fn kill_child(child: &mut Child, _: Duration) -> Result<(), LaunchError> {
    child.kill().map_err(LaunchError::Terminate)
}

#[cfg(unix)]
fn recording_kill(flag: &Arc<AtomicBool>) -> BoxedTerminateFn {
    let flag = Arc::clone(flag);
    Box::new(move |child: &mut Child, timeout| {
        flag.store(true, Ordering::SeqCst);
        kill_child(child, timeout)
    })
}

#[cfg(unix)]
fn spawn_sleep(seconds: &str) -> io::Result<Child> {
    Command::new("/bin/sleep").arg(seconds).spawn()
}

#[cfg(unix)]
fn unwrap_launch_err<T>(result: Result<T, LaunchError>, what: &str) -> LaunchError {
    match result {
        Err(error) => error,
        Ok(_) => panic!("{what}: expected error"),
    }
}

#[cfg(unix)]
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

#[cfg(unix)]
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

#[cfg(unix)]
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

#[cfg(unix)]
fn dispositions() -> [Disposition; 4] {
    [
        Disposition::IndependentLongLived,
        Disposition::IndependentBoundedHelper {
            timeout: std::time::Duration::from_secs(1),
        },
        Disposition::InheritedParentScope,
        Disposition::ExplicitlyUnowned {
            reason: "caller owns no child".to_owned(),
        },
    ]
}

#[cfg(unix)]
fn assert_launch_capability<T>(result: Result<T, LaunchError>) {
    assert!(matches!(
        result,
        Err(LaunchError::CapabilityUnavailable {
            needed: "process-groups"
        })
    ));
}

#[cfg(unix)]
#[test]
fn capability_refusal_precedes_all_injected_launch_spawns() {
    for disposition in dispositions() {
        assert_launch_capability(launch_with(
            disposition.clone(),
            || panic!("raw spawn closure must not run"),
            Box::new(|_, _| panic!("terminate closure must not run")),
            |_| {
                Err(LaunchError::CapabilityUnavailable {
                    needed: "process-groups",
                })
            },
            |_| panic!("confirmation closure must not run"),
        ));
        assert_launch_capability(launch_managed_with(
            disposition,
            || panic!("managed spawn closure must not run"),
            |_| {
                Err(LaunchError::CapabilityUnavailable {
                    needed: "process-groups",
                })
            },
        ));
    }
}
