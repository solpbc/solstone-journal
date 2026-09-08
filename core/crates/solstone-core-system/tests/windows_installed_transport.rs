// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Actual installed-root pipe controls, run from an admitted staged installation.
//! Task Scheduler and public journal entry are separate acceptance subjects.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use solstone_core_installation_identity::{
    Generation, GuardFields, InstallationId, NamespaceName, journal_token_from_path,
    load_installation_binding, owner_base, root_token_from_path,
};
use solstone_core_system::process::{
    BoundedHelperBudget, BoundedHelperRequest, BoundedHelperResources, HelperAdmissionStatus,
    HelperCleanupStatus, InstalledTaskLaunchRequest, LaunchError, ProcessBirth, ProcessInstance,
    forward_windows_installed_task, receive_windows_installed_task_launch, receive_windows_launch,
    retry_windows_launch_cleanup_until, run_bounded_helper,
};
use windows_sys::Win32::Foundation::{FILETIME, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    WaitForSingleObject,
};

const JOURNAL: &str = "SOLSTONE_TEST_INSTALLED_TRANSPORT_JOURNAL";
const MODE: &str = "SOLSTONE_TEST_INSTALLED_TRANSPORT_MODE";
const FIXTURE: &str = "SOLSTONE_TEST_INSTALLED_TRANSPORT_FIXTURE";
const FORWARDER: &str = "installed_transport::installed_transport_forwarder";
const RECEIVER: &str = "installed_transport::installed_transport_receiver";
const REFUSAL: &str = "launch variant or installed action mismatch";

fn arguments(selector: &str) -> Vec<String> {
    [
        "--ignored",
        "--exact",
        selector,
        "--show-output",
        "--test-threads=2",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn request() -> InstalledTaskLaunchRequest {
    let journal = PathBuf::from(std::env::var_os(JOURNAL).expect("explicit fixture journal"));
    let exe = std::env::current_exe().expect("fixture executable");
    let root = exe
        .parent()
        .and_then(solstone_core_journal::resolve_identity_root_from_executable_dir)
        .expect("staged installation root");
    let binding = load_installation_binding(
        &owner_base().expect("actual owner base"),
        &root_token_from_path(&root).expect("root token"),
    )
    .expect("actual setup-created binding");
    assert_eq!(
        journal_token_from_path(&journal).unwrap(),
        binding.journal_token
    );
    InstalledTaskLaunchRequest {
        journal,
        guard: GuardFields::from_binding(&binding),
        arguments: arguments(RECEIVER),
        acknowledgement_timeout: Duration::from_secs(3),
    }
}

fn fixture() -> PathBuf {
    PathBuf::from(std::env::var_os(FIXTURE).expect("private fixture path"))
}

fn await_file(path: &Path, deadline: Instant) -> io::Result<Vec<u8>> {
    loop {
        match fs::read(path) {
            Ok(bytes) => return Ok(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    }
}

fn birth(handle: &impl AsRawHandle) -> io::Result<ProcessBirth> {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: caller retains a process handle and all output storage is valid.
    #[allow(unsafe_code)]
    if unsafe {
        GetProcessTimes(
            handle.as_raw_handle(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(ProcessBirth::windows(
        (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime),
    ))
}

fn open_observer(expected: ProcessInstance) -> io::Result<OwnedHandle> {
    // SAFETY: this is a noninheritable, read-only observation handle, never kill authority.
    #[allow(unsafe_code)]
    let raw = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            expected.pid,
        )
    };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful OpenProcess returns one newly owned handle.
    #[allow(unsafe_code)]
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    if birth(&handle)? != expected.birth {
        return Err(io::Error::other(
            "descendant birth differs from retained child",
        ));
    }
    Ok(handle)
}

fn is_signalled(handle: &impl AsRawHandle) -> io::Result<bool> {
    // SAFETY: caller retains a valid process handle; zero timeout is observation only.
    #[allow(unsafe_code)]
    match unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) } {
        WAIT_OBJECT_0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        _ => Err(io::Error::last_os_error()),
    }
}

fn publish(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension("pending");
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, path)
}

#[test]
#[ignore = "child of the staged native installed transport receipt"]
fn installed_transport_receiver() {
    let mode = std::env::var(MODE).expect("transport mode");
    let mut expected = request();
    match mode.as_str() {
        "valid" | "hosted-entry" => {}
        "argv" => expected.arguments.push("changed".into()),
        "journal" => expected.journal.push("different journal"),
        "namespace" => {
            let candidate = NamespaceName::parse(&"1".repeat(64)).unwrap();
            expected.guard.namespace = if expected.guard.namespace == candidate {
                NamespaceName::parse(&"2".repeat(64)).unwrap()
            } else {
                candidate
            };
        }
        "id" => {
            let candidate = InstallationId::parse(&"1".repeat(32)).unwrap();
            expected.guard.id = if expected.guard.id == candidate {
                InstallationId::parse(&"2".repeat(32)).unwrap()
            } else {
                candidate
            };
        }
        "generation" => {
            expected.guard.generation = Generation::new(if expected.guard.generation.get() == 1 {
                2
            } else {
                1
            })
            .unwrap()
        }
        "journal-token" => {
            expected.guard.journal_token =
                journal_token_from_path(&expected.journal.join("different journal")).unwrap()
        }
        _ => panic!("unknown transport mode"),
    }
    let admission = if mode == "hosted-entry" {
        receive_windows_launch().map(|_| None)
    } else {
        receive_windows_installed_task_launch(&expected).map(Some)
    };
    if mode != "valid" {
        assert!(
            matches!(admission, Err(LaunchError::Admission(ref message)) if message == REFUSAL),
            "actual entry refusal: {admission:?}"
        );
        publish(&fixture().join("refusal"), REFUSAL.as_bytes()).unwrap();
        println!("JOURNAL_WIN_CI_INSTALLED_RECEIVER={mode}:REFUSED");
        return;
    }
    let _admission = admission.expect("real installed pipe admission");
    let executable = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .join("solstone-system-test-child.exe");
    assert!(executable.is_file(), "exact staged system child");
    let phase = fixture();
    let ready = phase.join("descendant-ready");
    let never_release = phase.join("never-release");
    assert!(!never_release.exists());
    // This fixture-only raw child deliberately inherits the receiver's real Job.
    // No new Job or generation holder may mask inner forwarder containment.
    let child = Command::new(executable)
        .arg("ready-wait")
        .arg(&ready)
        .arg(&never_release)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("inherited-Job descendant");
    let identity = ProcessInstance {
        pid: child.id(),
        birth: birth(&child).unwrap(),
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    await_file(&ready, deadline).expect("descendant running");
    assert!(!is_signalled(&child).unwrap());
    publish(
        &phase.join("descendant.json"),
        &serde_json::to_vec(&identity).unwrap(),
    )
    .unwrap();
    await_file(&phase.join("observer-retained"), deadline)
        .expect("forwarder retained exact handle");
    assert!(
        !is_signalled(&child).unwrap(),
        "descendant must still require Job cleanup"
    );
    assert!(!never_release.exists());
    drop(child); // original Job remains the sole termination authority, not this handle
    println!("JOURNAL_WIN_CI_INSTALLED_RECEIVER=valid:ADMITTED");
}

#[test]
#[ignore = "child of the staged native installed transport receipt"]
fn installed_transport_forwarder() {
    assert!(std::env::var_os("SOL_WINDOWS_LAUNCH").is_none());
    let mode = std::env::var(MODE).expect("transport mode");
    let request = request();
    let deadline = Instant::now() + Duration::from_secs(15);
    let retained = Arc::new(OnceLock::new());
    let observer = if mode == "valid" {
        let retained = retained.clone();
        let phase = fixture();
        Some(
            std::thread::Builder::new()
                .name("installed-observer".into())
                .spawn(move || -> io::Result<()> {
                    let identity: ProcessInstance = serde_json::from_slice(&await_file(
                        &phase.join("descendant.json"),
                        deadline,
                    )?)?;
                    let mut stale = identity;
                    stale.birth = ProcessBirth::windows(
                        stale
                            .birth
                            .windows_filetime()
                            .ok_or_else(|| io::Error::other("missing native birth"))?
                            ^ 1,
                    );
                    if open_observer(stale).is_ok() {
                        return Err(io::Error::other(
                            "same-PID stale-birth observer was accepted",
                        ));
                    }
                    let handle = open_observer(identity)?;
                    if is_signalled(&handle)? {
                        return Err(io::Error::other("descendant already exited"));
                    }
                    retained
                        .set(handle)
                        .map_err(|_| io::Error::other("duplicate observer"))?;
                    publish(&phase.join("observer-retained"), b"retained")
                })
                .expect("start observer before native launch"),
        )
    } else {
        None
    };
    let result =
        forward_windows_installed_task(std::env::current_exe().unwrap().as_os_str(), &request);
    // This observation happens immediately after INNER forward returns, before
    // joining the observer, registry recovery or outer fixture Job cleanup.
    let inner_descendant_settled = retained.get().map(is_signalled).transpose();
    let cleanup = retry_windows_launch_cleanup_until(deadline);
    let observer_result = observer.map(|thread| thread.join());
    assert!(
        matches!(cleanup, HelperAdmissionStatus::Ready),
        "original launch cleanup: {cleanup:?}"
    );
    if let Some(observer_result) = observer_result {
        observer_result.unwrap().unwrap();
    }
    if mode == "valid" {
        assert_eq!(result.unwrap(), 0, "actual admitted receiver exit");
        assert_eq!(
            inner_descendant_settled.unwrap(),
            Some(true),
            "inner forwarder left a descendant"
        );
        assert!(!fixture().join("never-release").exists());
    } else {
        assert!(result.is_err(), "entry refusal must fail admission");
        assert_eq!(
            fs::read(fixture().join("refusal")).unwrap(),
            REFUSAL.as_bytes()
        );
    }
    println!("JOURNAL_WIN_CI_INSTALLED_FORWARDER={mode}:PASS");
}

#[test]
#[ignore = "native staged installation and setup-created owner binding required"]
fn windows_installed_transport_receipt() {
    let bound = request(); // fail prerequisites before creating or launching a fixture
    let executable = std::env::current_exe().unwrap();
    let bin = executable.parent().unwrap().to_path_buf();
    let deadline = Instant::now() + Duration::from_secs(100);
    for mode in [
        "valid",
        "hosted-entry",
        "argv",
        "journal",
        "namespace",
        "id",
        "generation",
        "journal-token",
    ] {
        let phase = Arc::new(tempfile::tempdir().unwrap());
        let mut resources = BoundedHelperResources::new();
        resources.retain(phase.clone());
        let output = run_bounded_helper(BoundedHelperRequest {
            executable: executable.clone(),
            package_root: bin.clone(),
            current_directory: bin.clone(),
            arguments: arguments(FORWARDER),
            environment: BTreeMap::from([
                (
                    OsString::from("SystemRoot"),
                    std::env::var_os("SystemRoot").unwrap(),
                ),
                (JOURNAL.into(), bound.journal.as_os_str().to_owned()),
                (MODE.into(), mode.into()),
                (FIXTURE.into(), phase.path().as_os_str().to_owned()),
            ]),
            stdin: Vec::new(),
            resource_limits: None,
            resources,
            budget: BoundedHelperBudget {
                timeout: deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_secs(20)),
                stdin_limit_bytes: 1,
                stdout_limit_bytes: 65536,
                stderr_limit_bytes: 65536,
            },
        });
        let output = match output {
            Ok(output) => output,
            Err(failure) => {
                let settled = failure
                    .cleanup()
                    .map(|cleanup| cleanup.retry_until(deadline));
                // The original registry still owns phase/resources if this fails;
                // never delete the fixture or label an incomplete owner reaped.
                assert!(
                    settled.is_none_or(|state| state == HelperCleanupStatus::Quiescent),
                    "outer cleanup remains pending: {failure}"
                );
                panic!("outer transport fixture failed after cleanup: {failure}");
            }
        };
        assert!(output.quiescent);
        assert_eq!(
            output.exit_code,
            0,
            "forwarder {mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        for selector in [FORWARDER, RECEIVER] {
            assert_eq!(
                stdout
                    .lines()
                    .filter(|line| *line == format!("test {selector} ... ok"))
                    .count(),
                1,
                "{stdout}"
            );
        }
        assert_eq!(
            stdout
                .lines()
                .filter(|line| line.starts_with("test result: ok. 1 passed; 0 failed; 0 ignored;"))
                .count(),
            2,
            "{stdout}"
        );
        assert_eq!(
            stdout
                .lines()
                .filter(|line| *line == format!("JOURNAL_WIN_CI_INSTALLED_FORWARDER={mode}:PASS"))
                .count(),
            1,
            "{stdout}"
        );
    }
    println!("JOURNAL_WIN_CI_INSTALLED_TRANSPORT=PASS");
}
