// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Advisory duplicate suppression for the heartbeat command's numeric PID file.
//! This cannot establish heartbeat identity or authorize process control.

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

pub(crate) fn recorded_pid_may_be_running(pid: u32) -> io::Result<bool> {
    if pid == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid heartbeat PID",
        ));
    }
    // SAFETY: requests only observation rights; the returned handle is not inheritable.
    #[allow(unsafe_code)]
    let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if raw.is_null() {
        let error = io::Error::last_os_error();
        return match error.raw_os_error().map(|code| code as u32) {
            // As with Unix EPERM, suppress duplicates without claiming verified identity.
            Some(ERROR_ACCESS_DENIED) => Ok(true),
            Some(ERROR_INVALID_PARAMETER) => Ok(false),
            _ => Err(error),
        };
    }
    // SAFETY: OpenProcess transferred this newly owned, non-null handle.
    #[allow(unsafe_code)]
    let process = unsafe { OwnedHandle::from_raw_handle(raw) };
    // SAFETY: the owned process handle remains open throughout this zero-timeout wait.
    #[allow(unsafe_code)]
    match unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } {
        WAIT_TIMEOUT => Ok(true),
        WAIT_OBJECT_0 => Ok(false),
        WAIT_FAILED => Err(io::Error::last_os_error()),
        value => Err(io::Error::other(format!(
            "unexpected process wait result {value}"
        ))),
    }
}

#[cfg(all(test, feature = "test-hooks"))]
mod native_tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use solstone_core_system::process::{
        Disposition, HelperAdmissionStatus, ManagedLaunchRequest, ManagedProcess, OutputStream,
        ProcessEvent, ProcessEventSink, SpawnOptions, launch_managed_request,
        observe_windows_launch_cleanup, retry_windows_launch_cleanup_until,
    };

    const CHILD: &str = "heartbeat_pid_windows::native_tests::heartbeat_pid_exited_child";
    const CHILD_MARKER: &str = "JOURNAL_WIN_CI_HEARTBEAT_PID_CHILD=PASS";

    #[test]
    #[ignore = "child of the native advisory heartbeat PID receipt"]
    fn heartbeat_pid_exited_child() {
        println!("{CHILD_MARKER}");
    }

    // Independent raw observation distinguishes actual access denial and absence
    // from the advisory helper's intentionally coarser boolean result.
    fn raw_observation(pid: u32) -> Result<OwnedHandle, u32> {
        // SAFETY: observation rights only, noninheritable, no caller pointers.
        #[allow(unsafe_code)]
        let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if raw.is_null() {
            return Err(io::Error::last_os_error().raw_os_error().unwrap() as u32);
        }
        // SAFETY: OpenProcess transferred this newly owned non-null handle.
        #[allow(unsafe_code)]
        Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
    }

    fn signalled(process: &OwnedHandle) -> bool {
        // SAFETY: the original owned handle remains live across the zero wait.
        #[allow(unsafe_code)]
        let result = unsafe { WaitForSingleObject(process.as_raw_handle(), 0) };
        assert!(matches!(result, WAIT_OBJECT_0 | WAIT_TIMEOUT));
        result == WAIT_OBJECT_0
    }

    #[derive(Default)]
    struct Output(Mutex<(Vec<String>, usize, bool)>);

    impl ProcessEventSink for Output {
        fn emit(&self, event: ProcessEvent) {
            if let ProcessEvent::Line { stream, line, .. } = event {
                let mut state = self.0.lock().unwrap();
                let bytes = line.len().saturating_add(1);
                if bytes > (64 * 1024_usize).saturating_sub(state.1) {
                    state.2 = true;
                    return;
                }
                state.1 += bytes;
                if stream == OutputStream::Stdout {
                    state.0.push(line);
                }
            }
        }
    }

    #[test]
    #[ignore = "source-bound standard-owner native process controls required"]
    fn windows_heartbeat_pid_observation_receipt() {
        assert!(matches!(
            observe_windows_launch_cleanup(),
            HelperAdmissionStatus::Ready
        ));
        let denied_pid: u32 = std::env::var("SOLSTONE_TEST_HEARTBEAT_DENIED_PID")
            .expect("explicit read-only denied-process prerequisite")
            .parse()
            .expect("numeric denied PID");
        assert_ne!(denied_pid, 0);
        assert_ne!(denied_pid, std::process::id());
        assert_eq!(
            raw_observation(denied_pid).unwrap_err(),
            ERROR_ACCESS_DENIED
        );
        assert!(recorded_pid_may_be_running(denied_pid).unwrap());
        // The operator retains the prerequisite process object across this arm.
        // Recheck the raw error so the boolean cannot stand in for access denial.
        assert_eq!(
            raw_observation(denied_pid).unwrap_err(),
            ERROR_ACCESS_DENIED
        );
        assert!(recorded_pid_may_be_running(std::process::id()).unwrap());
        assert_eq!(
            recorded_pid_may_be_running(0).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );

        // Deliberately retain the exact fixture path on a failed cleanup. The
        // outer native driver owns failed-fixture reconciliation after process exit.
        let root = tempfile::Builder::new()
            .prefix("solstone-heartbeat-pid-")
            .tempdir()
            .unwrap()
            .keep();
        println!("JOURNAL_WIN_CI_HEARTBEAT_PID_FIXTURE={}", root.display());
        let deadline = Instant::now() + Duration::from_secs(30);
        let body_deadline = deadline - Duration::from_secs(10);
        let output = Arc::new(Output::default());
        let mut process: Option<ManagedProcess> = None;
        let mut retained_process = None;
        let mut exited_pid = None;
        let body = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut environment: BTreeMap<OsString, OsString> = BTreeMap::new();
            if let Some(system_root) = std::env::var_os("SystemRoot") {
                environment.insert("SystemRoot".into(), system_root);
            }
            process = Some(
                launch_managed_request(
                    Disposition::IndependentBoundedHelper {
                        timeout: body_deadline.saturating_duration_since(Instant::now()),
                    },
                    ManagedLaunchRequest {
                        command: vec![
                            std::env::current_exe().unwrap().to_str().unwrap().into(),
                            "--ignored".into(),
                            "--exact".into(),
                            CHILD.into(),
                            "--show-output".into(),
                        ],
                        options: SpawnOptions {
                            journal_root: root.clone(),
                            reference: format!("heartbeat-pid-{}", std::process::id()),
                            day: None,
                            sink: Some(output.clone()),
                            environment,
                        },
                        read_file_grants: Vec::new(),
                    },
                )
                .expect("launch original bounded child")
                .into_managed()
                .expect("retain original managed Job"),
            );
            let process = process.as_mut().unwrap();
            // The Job retains native identity internally. The optional public
            // domain identity is intentionally unbound for this pure helper.
            let pid = process.pid();
            // ManagedProcess already retains the original process object, so
            // this observation handle cannot race with reuse of its numeric PID.
            retained_process = Some(raw_observation(pid).expect("retain original child object"));
            loop {
                if let Some(status) = process.poll().unwrap() {
                    assert_eq!(status, 0, "actual child libtest exit");
                    break;
                }
                assert!(Instant::now() < body_deadline, "owned child deadline");
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(signalled(retained_process.as_ref().unwrap()));
            assert!(!recorded_pid_may_be_running(pid).unwrap());
            exited_pid = Some(pid);
        }));
        let mut settled = true;
        if let Some(process) = process.as_mut() {
            if process.poll().ok().flatten().is_none() {
                let _ = process.terminate_exact_until(deadline);
            }
            settled &= process.cleanup_until(deadline);
            process.detach_after_bounded_shutdown();
        }
        drop(process);
        settled &= matches!(
            retry_windows_launch_cleanup_until(deadline),
            HelperAdmissionStatus::Ready
        );
        drop(retained_process);
        assert!(
            settled,
            "original Job/I/O cleanup incomplete; fixture retained"
        );
        if let Err(panic) = body {
            std::panic::resume_unwind(panic);
        }
        let state = output.0.lock().unwrap();
        assert!(!state.2, "child output exceeded bound");
        for expected in [format!("test {CHILD} ... ok"), CHILD_MARKER.to_owned()] {
            assert_eq!(state.0.iter().filter(|line| **line == expected).count(), 1);
        }
        let summaries: Vec<_> = state
            .0
            .iter()
            .filter(|line| line.starts_with("test result:"))
            .collect();
        assert_eq!(summaries.len(), 1);
        assert!(summaries[0].starts_with("test result: ok. 1 passed; 0 failed; 0 ignored;"));
        drop(state);
        let pid = exited_pid.unwrap();
        // Absence is a distinct observed outcome after releasing every owned
        // handle. A reused PID or an externally retained object refuses this arm.
        assert_eq!(raw_observation(pid).unwrap_err(), ERROR_INVALID_PARAMETER);
        assert!(!recorded_pid_may_be_running(pid).unwrap());
        std::fs::remove_dir_all(&root).expect("remove only settled fixture");
        assert!(!root.exists());
        for label in [
            "live-self",
            "denied-open",
            "invalid-zero",
            "retained-exited",
            "absent",
            "cleanup",
        ] {
            println!("JOURNAL_WIN_CI_HEARTBEAT_PID={label}:PASS");
        }
        println!("JOURNAL_WIN_CI_HEARTBEAT_PID=PASS");
    }
}
