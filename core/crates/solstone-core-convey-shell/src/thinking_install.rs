// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Process composition for owner-requested installs. The install domain's lease
//! arbitrates writers; the process boundary owns launch, identity and reaping.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;
use solstone_core_local::install::{lease, status};
use solstone_core_system::process::{
    Disposition, InstanceVerdict, ManagedLaunchRequest, ProcessInstance, ProcessInstanceSource,
    SignalKind, SpawnOptions, SystemProcessInstanceSource, launch_managed_request,
    signal_exact_instance,
};

static ADMISSION: Mutex<()> = Mutex::new(());
const ADMISSION_TIMEOUT: Duration = Duration::from_secs(5);
const STOP_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) fn start(journal: &Path, model: &str) -> Result<Value, String> {
    let _guard = ADMISSION
        .lock()
        .map_err(|_| "installer admission unavailable")?;
    // Recheck under the admission mutex. Other CLI processes still arbitrate via
    // the OS lease, so an independent winner is reported from persisted status.
    let current = status::read_status(journal, "local").map_err(|e| e.to_string())?;
    if lease::is_held(journal, "local").map_err(|e| e.to_string())? {
        if status::is_in_flight(&current.install_state) {
            return Ok(solstone_core_thinking::local::bootstrap_status(
                journal, model,
            ));
        }
        return Err("installer lease is busy".into());
    }
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let parent = executable
        .parent()
        .ok_or("installer directory unavailable")?;
    let binary = solstone_core_journal_cli::sibling_native_in_dir(parent, "solstone-core")
        .map_err(|e| e.to_string())?;
    launch_installer(journal, model, &binary, ADMISSION_TIMEOUT)
}

fn launch_installer(
    journal: &Path,
    model: &str,
    binary: &Path,
    admission_timeout: Duration,
) -> Result<Value, String> {
    // Linux arms the installer's parent-death SIGKILL against the *thread* that
    // forked it, so that thread has to outlive the child. The portal reaches here
    // from a Tokio blocking worker, which is reaped after its idle keep-alive:
    // forking there killed the installer seconds after admission and the next
    // status read rendered `failed` / `install_interrupted`. One dedicated thread
    // forks, admits, and then reaps, so the fork's parent thread and the child's
    // reaper are the same thread by construction.
    let (admitted, admission) = std::sync::mpsc::sync_channel(1);
    let journal = journal.to_owned();
    let model = model.to_owned();
    let binary = binary.to_owned();
    std::thread::Builder::new()
        .name("local-install".into())
        .spawn(move || admit_installer(&journal, &model, &binary, admission_timeout, &admitted))
        .map_err(|e| e.to_string())?;
    admission
        .recv()
        .map_err(|_| "installer admission unavailable".to_owned())?
}

/// Fork the installer, report the admission outcome, then reap it on this same
/// thread. Every exit path sends exactly one admission result.
fn admit_installer(
    journal: &Path,
    model: &str,
    binary: &Path,
    admission_timeout: Duration,
    admitted: &std::sync::mpsc::SyncSender<Result<Value, String>>,
) {
    // A bounded owner operation, not a hosted service generation. Managed launch
    // retains exact identity, canonical operational logs and child-tree cleanup.
    let launched = launch_managed_request(
        Disposition::IndependentBoundedHelper {
            timeout: STOP_TIMEOUT,
        },
        ManagedLaunchRequest {
            command: vec![
                binary.to_string_lossy().into_owned(),
                "install-provider".into(),
                "local".into(),
            ],
            options: SpawnOptions {
                journal_root: journal.to_owned(),
                reference: "local-install".into(),
                day: None,
                sink: None,
                environment: Default::default(),
            },
            #[cfg(windows)]
            read_file_grants: Vec::new(),
        },
    );
    let mut child = match launched {
        Ok(child) => child,
        Err(error) => {
            let _ = admitted.send(Err(error.to_string()));
            return;
        }
    };
    let Some(identity) = child.exact_identity().map(|launched| launched.instance) else {
        let _ = admitted.send(Err("installer identity unavailable".into()));
        return;
    };
    let deadline = Instant::now() + admission_timeout;
    loop {
        let exit = match child.poll() {
            Ok(exit) => exit,
            Err(error) => {
                let _ = admitted.send(Err(error.to_string()));
                return;
            }
        };
        let current = match status::read_status(journal, "local") {
            Ok(current) => current,
            Err(error) => {
                let _ = admitted.send(Err(error.to_string()));
                return;
            }
        };
        let held = match lease::is_held(journal, "local") {
            Ok(held) => held,
            Err(error) => {
                let _ = admitted.send(Err(error.to_string()));
                return;
            }
        };
        let owned = current
            .owner
            .clone()
            .and_then(|value| serde_json::from_value::<ProcessInstance>(value).ok())
            == Some(identity);
        if owned
            && current.attempt_id.is_some()
            && status::is_in_flight(&current.install_state)
            && held
        {
            let _ = admitted.send(Ok(solstone_core_thinking::local::bootstrap_status(
                journal, model,
            )));
            // Reaping here, rather than on a thread spawned for it, is what keeps
            // the child's parent thread alive for the whole install.
            let _ = child.wait();
            return;
        }
        if let Some(code) = exit {
            if code == 0 && !held && current.install_state == "installed" {
                let _ = admitted.send(Ok(solstone_core_thinking::local::bootstrap_status(
                    journal, model,
                )));
                return;
            }
            if held && status::is_in_flight(&current.install_state) && current.attempt_id.is_some()
            {
                let _ = admitted.send(Ok(solstone_core_thinking::local::bootstrap_status(
                    journal, model,
                )));
                return;
            }
            let _ = admitted.send(Err(format!("installer exited before admission ({code})")));
            return;
        }
        if Instant::now() >= deadline {
            let outcome = match child.terminate_exact(STOP_TIMEOUT) {
                Ok(()) => Err("installer admission timed out".to_owned()),
                Err(error) => Err(format!("installer admission cleanup failed: {error}")),
            };
            let _ = admitted.send(outcome);
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub(crate) fn stop(owner: &Value) -> Result<(), String> {
    let expected: ProcessInstance = serde_json::from_value(owner.clone())
        .map_err(|_| "installer process identity unavailable")?;
    if expected.pid == std::process::id() || expected.pid == 0 || !expected.birth.is_verifiable() {
        return Err("installer process identity invalid".into());
    }
    stop_with(expected, &SystemProcessInstanceSource, |signal| {
        signal_exact_instance(expected, signal, &SystemProcessInstanceSource)
            .map_err(|e| e.to_string())
    })
}

fn stop_with(
    expected: ProcessInstance,
    source: &dyn ProcessInstanceSource,
    signal: impl FnMut(SignalKind) -> Result<(), String>,
) -> Result<(), String> {
    stop_with_timeout(expected, source, signal, STOP_TIMEOUT)
}

fn stop_with_timeout(
    expected: ProcessInstance,
    source: &dyn ProcessInstanceSource,
    mut signal: impl FnMut(SignalKind) -> Result<(), String>,
    timeout: Duration,
) -> Result<(), String> {
    for kind in [SignalKind::Terminate, SignalKind::Kill] {
        match source.observe(&expected) {
            InstanceVerdict::NotSameOrExited => return Ok(()),
            InstanceVerdict::Unverifiable => {
                return Err("installer process observation unavailable".into());
            }
            InstanceVerdict::SameLive { .. } => signal(kind)?,
        }
        let deadline = Instant::now() + timeout;
        loop {
            match source.observe(&expected) {
                InstanceVerdict::NotSameOrExited => return Ok(()),
                InstanceVerdict::Unverifiable => {
                    // Reaping can remove the process between native inspector
                    // reads. Observe again within this wait, but uncertainty
                    // never authorizes another signal or a successful result.
                    if Instant::now() >= deadline {
                        return Err("installer process observation unavailable".into());
                    }
                }
                InstanceVerdict::SameLive { .. } => {
                    if Instant::now() >= deadline {
                        break;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    Err("installer is still running".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_system::process::{InspectResult, InstanceCensus, ProcessBirth};

    struct Observed(InspectResult);
    impl ProcessInstanceSource for Observed {
        fn inspect(&self, _: u32) -> InspectResult {
            self.0
        }
        fn census(&self) -> InstanceCensus {
            InstanceCensus::Incomplete(Vec::new())
        }
    }

    struct Observations(Mutex<std::collections::VecDeque<InspectResult>>);
    impl ProcessInstanceSource for Observations {
        fn inspect(&self, _: u32) -> InspectResult {
            self.0
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected observation")
        }
        fn census(&self) -> InstanceCensus {
            panic!("cancellation must inspect only its exact owner")
        }
    }

    #[test]
    fn cancellation_never_signals_reused_or_unverifiable_identity() {
        let expected = ProcessInstance {
            pid: 123,
            birth: ProcessBirth::linux(10, 100, 100),
        };
        let absent = Observed(InspectResult::Absent);
        assert!(
            stop_with(expected, &absent, |_| panic!(
                "absent process must not be signalled"
            ))
            .is_ok()
        );
        let unknown = Observed(InspectResult::Unverifiable);
        assert!(
            stop_with(expected, &unknown, |_| panic!(
                "unknown process must not be signalled"
            ))
            .is_err()
        );
        let reused = Observed(InspectResult::Present {
            instance: ProcessInstance {
                pid: 123,
                birth: ProcessBirth::linux(11, 100, 100),
            },
            uid: 1000,
            execution: solstone_core_system::process::ExecutionState::Running,
            ppid: None,
            pgid: None,
        });
        assert!(
            stop_with(expected, &reused, |_| panic!(
                "reused PID must not be signalled"
            ))
            .is_ok()
        );
        assert!(stop(&serde_json::json!({"pid": -1})).is_err());
        assert!(stop(&serde_json::json!({"pid": 4294967296_u64})).is_err());
    }

    #[test]
    fn cancellation_observes_exit_after_transient_post_signal_uncertainty() {
        let expected = ProcessInstance {
            pid: 123,
            birth: ProcessBirth::linux(10, 100, 100),
        };
        let source = Observations(Mutex::new(std::collections::VecDeque::from([
            InspectResult::Present {
                instance: expected,
                uid: 1000,
                execution: solstone_core_system::process::ExecutionState::Running,
                ppid: None,
                pgid: None,
            },
            // A concurrent waiter can reap between the Linux stat and uid reads.
            InspectResult::Unverifiable,
            InspectResult::Absent,
        ])));
        let mut signals = Vec::new();
        let result = stop_with(expected, &source, |kind| {
            signals.push(kind);
            Ok(())
        });
        assert!(result.is_ok(), "{result:?}");
        assert!(matches!(signals.as_slice(), [SignalKind::Terminate]));
        assert!(source.0.lock().unwrap().is_empty());
    }

    #[test]
    fn cancellation_deadline_requires_verified_liveness_before_escalation() {
        let expected = ProcessInstance {
            pid: 123,
            birth: ProcessBirth::linux(10, 100, 100),
        };
        let live = InspectResult::Present {
            instance: expected,
            uid: 1000,
            execution: solstone_core_system::process::ExecutionState::Running,
            ppid: None,
            pgid: None,
        };
        let unknown = Observations(Mutex::new(std::collections::VecDeque::from([
            live,
            InspectResult::Unverifiable,
        ])));
        let mut signals = Vec::new();
        let result = stop_with_timeout(
            expected,
            &unknown,
            |kind| {
                signals.push(kind);
                Ok(())
            },
            Duration::ZERO,
        );
        assert_eq!(
            result.unwrap_err(),
            "installer process observation unavailable"
        );
        assert!(matches!(signals.as_slice(), [SignalKind::Terminate]));
        assert!(unknown.0.lock().unwrap().is_empty());

        let still_live = Observations(Mutex::new(std::collections::VecDeque::from([
            live,
            live,
            live,
            InspectResult::Absent,
        ])));
        signals.clear();
        stop_with_timeout(
            expected,
            &still_live,
            |kind| {
                signals.push(kind);
                Ok(())
            },
            Duration::ZERO,
        )
        .unwrap();
        assert!(matches!(
            signals.as_slice(),
            [SignalKind::Terminate, SignalKind::Kill]
        ));
        assert!(still_live.0.lock().unwrap().is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn admitted_installer_is_observable_and_duplicate_start_reuses_its_attempt() {
        use std::os::unix::fs::PermissionsExt;
        let journal = tempfile::tempdir().unwrap();
        let script = journal.path().join("installer");
        let pid_path = journal.path().join("installer.pid");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nexec sleep 30\n",
                pid_path.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let root = journal.path().to_owned();
        let (release, released) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let identity = loop {
                if let Ok(text) = std::fs::read_to_string(&pid_path)
                    && let Ok(pid) = text.trim().parse()
                    && let InspectResult::Present { instance, .. } =
                        SystemProcessInstanceSource.inspect(pid)
                {
                    break instance;
                }
                assert!(Instant::now() < deadline, "fake installer did not start");
                std::thread::sleep(Duration::from_millis(10));
            };
            let _held = lease::acquire(&root, "local").unwrap().unwrap();
            status::begin(
                &root,
                "{}".into(),
                "target".into(),
                Some(serde_json::json!(identity)),
                "downloading",
            )
            .unwrap();
            released.recv_timeout(Duration::from_secs(10)).unwrap();
            stop(&serde_json::json!(identity)).unwrap();
        });
        let admitted = launch_installer(
            journal.path(),
            "local/qwen3.5-4b",
            &script,
            ADMISSION_TIMEOUT,
        )
        .unwrap();
        assert_eq!(admitted["install_state"], "downloading");
        assert!(admitted["attempt_id"].is_string());
        let duplicate = start(journal.path(), "local/qwen3.5-4b").unwrap();
        assert_eq!(duplicate["attempt_id"], admitted["attempt_id"]);
        release.send(()).unwrap();
        writer.join().unwrap();
        assert!(!lease::is_held(journal.path(), "local").unwrap());
    }

    #[test]
    #[cfg(unix)]
    fn an_admitted_installer_outlives_the_thread_that_requested_it() {
        // The portal requests the install from a Tokio blocking worker, which is
        // reaped once it has been idle past the pool's keep-alive. On Linux the
        // installer's parent-death SIGKILL is armed against the forking thread, so
        // a reaped requester used to take the installer with it: the lease was
        // released, the persisted state stayed in flight, and the next status read
        // rendered `failed` / `install_interrupted` seconds after admission.
        // Joining the requesting thread here terminates it exactly as the pool
        // would, without waiting out a keep-alive.
        use std::os::unix::fs::PermissionsExt;
        let journal = tempfile::tempdir().unwrap();
        let script = journal.path().join("installer");
        let pid_path = journal.path().join("installer.pid");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nexec sleep 30\n",
                pid_path.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let root = journal.path().to_owned();
        let (release, released) = std::sync::mpsc::channel();
        let pid_file = pid_path.clone();
        let writer = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let identity = loop {
                if let Ok(text) = std::fs::read_to_string(&pid_file)
                    && let Ok(pid) = text.trim().parse()
                    && let InspectResult::Present { instance, .. } =
                        SystemProcessInstanceSource.inspect(pid)
                {
                    break instance;
                }
                assert!(Instant::now() < deadline, "fake installer did not start");
                std::thread::sleep(Duration::from_millis(10));
            };
            let _held = lease::acquire(&root, "local").unwrap().unwrap();
            status::begin(
                &root,
                "{}".into(),
                "target".into(),
                Some(serde_json::json!(identity)),
                "downloading",
            )
            .unwrap();
            released.recv_timeout(Duration::from_secs(10)).unwrap();
            stop(&serde_json::json!(identity)).unwrap();
        });

        let requested = journal.path().to_owned();
        let requester = std::thread::spawn(move || {
            launch_installer(&requested, "local/qwen3.5-4b", &script, ADMISSION_TIMEOUT)
        });
        let admitted = requester.join().unwrap().unwrap();
        assert_eq!(admitted["install_state"], "downloading");

        // The requesting thread is gone. The installer must still be running.
        let pid: u32 = std::fs::read_to_string(&pid_path)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            assert!(
                matches!(
                    SystemProcessInstanceSource.inspect(pid),
                    InspectResult::Present { .. }
                ),
                "the installer died with the thread that requested it"
            );
            std::thread::sleep(Duration::from_millis(100));
        }

        release.send(()).unwrap();
        writer.join().unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn admission_timeout_terminates_and_reaps_the_started_installer() {
        use std::os::unix::fs::PermissionsExt;
        let journal = tempfile::tempdir().unwrap();
        let script = journal.path().join("installer");
        let pid_path = journal.path().join("installer.pid");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nexec sleep 30\n",
                pid_path.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let result = launch_installer(
            journal.path(),
            "local/qwen3.5-4b",
            &script,
            Duration::from_millis(250),
        );
        assert_eq!(result.unwrap_err(), "installer admission timed out");
        let pid: u32 = std::fs::read_to_string(pid_path)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(
            SystemProcessInstanceSource.inspect(pid),
            InspectResult::Absent
        );
    }
}
