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
    // A bounded owner operation, not a hosted service generation. Managed launch
    // retains exact identity, canonical operational logs and child-tree cleanup.
    let mut child = launch_managed_request(
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
    )
    .map_err(|e| e.to_string())?;
    let identity = child
        .exact_identity()
        .ok_or("installer identity unavailable")?
        .instance;
    let deadline = Instant::now() + admission_timeout;
    loop {
        let exit = child.poll().map_err(|e| e.to_string())?;
        let current = status::read_status(journal, "local").map_err(|e| e.to_string())?;
        let held = lease::is_held(journal, "local").map_err(|e| e.to_string())?;
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
            let payload = solstone_core_thinking::local::bootstrap_status(journal, model);
            // Moving the authority keeps Drop cleanup armed even if thread creation
            // fails. Successful admission always has a waiter that reaps the child.
            std::thread::Builder::new()
                .name("local-install-wait".into())
                .spawn(move || {
                    let _ = child.wait();
                })
                .map_err(|e| e.to_string())?;
            return Ok(payload);
        }
        if let Some(code) = exit {
            if code == 0 && !held && current.install_state == "installed" {
                return Ok(solstone_core_thinking::local::bootstrap_status(
                    journal, model,
                ));
            }
            if held && status::is_in_flight(&current.install_state) && current.attempt_id.is_some()
            {
                return Ok(solstone_core_thinking::local::bootstrap_status(
                    journal, model,
                ));
            }
            return Err(format!("installer exited before admission ({code})"));
        }
        if Instant::now() >= deadline {
            child
                .terminate_exact(STOP_TIMEOUT)
                .map_err(|e| format!("installer admission cleanup failed: {e}"))?;
            return Err("installer admission timed out".into());
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
    mut signal: impl FnMut(SignalKind) -> Result<(), String>,
) -> Result<(), String> {
    for kind in [SignalKind::Terminate, SignalKind::Kill] {
        match source.observe(&expected) {
            InstanceVerdict::NotSameOrExited => return Ok(()),
            InstanceVerdict::Unverifiable => {
                return Err("installer process observation unavailable".into());
            }
            InstanceVerdict::SameLive { .. } => signal(kind)?,
        }
        let deadline = Instant::now() + STOP_TIMEOUT;
        loop {
            match source.observe(&expected) {
                InstanceVerdict::NotSameOrExited => return Ok(()),
                InstanceVerdict::Unverifiable => {
                    return Err("installer process observation unavailable".into());
                }
                InstanceVerdict::SameLive { .. } => {}
            }
            if Instant::now() >= deadline {
                break;
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
