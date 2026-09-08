// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One-shot, package-rooted Windows helper ownership.
//!
//! This is intentionally separate from [`super::managed::ManagedProcess`]: a
//! service owner drains into managed logs, while a helper owner returns one
//! bounded protocol response to its immediate caller.

use std::collections::BTreeMap;
use std::ffi::OsString;
#[cfg(windows)]
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;

use super::super::ProcessInstance;
#[cfg(windows)]
use super::bounded_cleanup::{BoundedHelperFailure, Completion, HelperIo, Reservation};

/// Keeps existing caller-owned resources alive through helper Job and I/O cleanup.
/// This bag neither validates nor transfers native authority.
#[cfg(windows)]
#[derive(Clone, Default)]
pub struct BoundedHelperResources {
    owners: Vec<std::sync::Arc<dyn Send + Sync + 'static>>,
}

#[cfg(windows)]
impl BoundedHelperResources {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn retain<T: Send + Sync + 'static>(&mut self, owner: std::sync::Arc<T>) {
        self.owners.push(owner);
    }
    pub fn extend(&mut self, resources: &Self) {
        self.owners.extend(resources.owners.iter().cloned());
    }
    pub fn len(&self) -> usize {
        self.owners.len()
    }
    pub fn is_empty(&self) -> bool {
        self.owners.is_empty()
    }
}

#[cfg(windows)]
impl std::fmt::Debug for BoundedHelperResources {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoundedHelperResources")
            .field("owners", &self.len())
            .finish()
    }
}

/// Explicit resource limits installed on a helper Job before the helper's
/// first instruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoundedHelperResourceLimits {
    pub cpu_rate_per_10_000: u32,
    pub committed_memory_bytes: usize,
}

/// Every byte and time limit for one helper protocol exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoundedHelperBudget {
    pub timeout: Duration,
    pub stdin_limit_bytes: usize,
    pub stdout_limit_bytes: usize,
    pub stderr_limit_bytes: usize,
}

/// One package-rooted helper invocation.
///
/// `environment` is the complete child environment: the launch never copies
/// the parent's variables. It must carry a nonempty `SystemRoot` entry, while
/// `PATH` is refused so it cannot choose executable or DLL code. The launcher
/// writes one empty `PATH` entry itself: Windows otherwise supplies a process
/// `PATH` when that key is omitted from a custom block.
#[derive(Debug)]
#[cfg_attr(not(windows), allow(dead_code))]
pub struct BoundedHelperRequest {
    pub package_root: PathBuf,
    pub executable: PathBuf,
    pub current_directory: PathBuf,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<OsString, OsString>,
    pub stdin: Vec<u8>,
    pub budget: BoundedHelperBudget,
    pub resource_limits: Option<BoundedHelperResourceLimits>,
    #[cfg(windows)]
    pub resources: BoundedHelperResources,
}

/// The complete, bounded result returned to a helper protocol parser.
#[derive(Debug, Eq, PartialEq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub struct BoundedHelperOutput {
    pub identity: ProcessInstance,
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// `true` only after the owner observed zero active processes in the Job.
    pub quiescent: bool,
}

/// A fail-closed helper launch or completion result.
#[derive(Debug, Error, Eq, PartialEq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum BoundedHelperError {
    #[error("earlier bounded helper cleanup blocks admission")]
    CleanupPending,
    #[error("bounded helper cleanup admission is contended")]
    CleanupContended,
    #[error("bounded helper admission did not complete before its deadline")]
    AdmissionDeadlineExceeded,
    #[error("bounded helper timeout must be nonzero")]
    ZeroTimeout,
    #[error("bounded helper {stream} byte limit must be nonzero")]
    ZeroLimit { stream: &'static str },
    #[error("bounded helper input exceeds its declared byte limit")]
    InputLimitExceeded,
    #[error("bounded helper environment must contain a nonempty SystemRoot")]
    MissingSystemRoot,
    #[error("bounded helper environment may not contain PATH")]
    PathEnvironmentRefused,
    #[error("bounded helper package root could not be canonicalized")]
    PackageRootUnavailable,
    #[error("bounded helper executable could not be canonicalized")]
    ExecutableUnavailable,
    #[error("bounded helper executable is outside its package root")]
    ExecutableOutsidePackage,
    #[error("bounded helper executable is not a regular file")]
    ExecutableNotFile,
    #[error("bounded helper current directory could not be canonicalized")]
    CurrentDirectoryUnavailable,
    #[error("bounded helper current directory is outside its package root")]
    CurrentDirectoryOutsidePackage,
    #[error("bounded helper current directory is not a directory")]
    CurrentDirectoryNotDirectory,
    #[error("bounded helper path cannot be represented by the managed Windows command boundary")]
    PathNotRepresentable,
    #[error("bounded helper failed before an owned child identity was available")]
    LaunchFailed,
    #[error("native launch finalization failed: {detail}")]
    LaunchFinalizationFailed {
        detail: String,
        process_id: Option<u32>,
    },
    #[error("bounded helper {stream} I/O worker could not start")]
    IoWorkerStartFailed {
        stream: &'static str,
        identity: ProcessInstance,
        quiescent: bool,
    },
    #[error("bounded helper input writer failed")]
    InputWriteFailed {
        identity: ProcessInstance,
        quiescent: bool,
    },
    #[error("bounded helper {stream} output exceeded its declared byte limit")]
    OutputLimitExceeded {
        stream: &'static str,
        identity: ProcessInstance,
        quiescent: bool,
    },
    #[error("bounded helper {stream} output reader failed")]
    OutputReadFailed {
        stream: &'static str,
        identity: ProcessInstance,
        quiescent: bool,
    },
    #[error("bounded helper did not complete before its deadline")]
    DeadlineExceeded {
        identity: ProcessInstance,
        quiescent: bool,
    },
    #[error("bounded helper process observation failed")]
    ProcessObservationFailed {
        identity: ProcessInstance,
        quiescent: bool,
    },
    #[error("bounded helper completion did not establish Job quiescence")]
    JobNotQuiescent { identity: ProcessInstance },
}

fn validate_request_shape(request: &BoundedHelperRequest) -> Result<(), BoundedHelperError> {
    if request.budget.timeout.is_zero() {
        return Err(BoundedHelperError::ZeroTimeout);
    }
    for (stream, limit) in [
        ("stdin", request.budget.stdin_limit_bytes),
        ("stdout", request.budget.stdout_limit_bytes),
        ("stderr", request.budget.stderr_limit_bytes),
    ] {
        if limit == 0 {
            return Err(BoundedHelperError::ZeroLimit { stream });
        }
    }
    if request.stdin.len() > request.budget.stdin_limit_bytes {
        return Err(BoundedHelperError::InputLimitExceeded);
    }

    let mut has_system_root = false;
    for (key, value) in &request.environment {
        let key = key.to_string_lossy();
        if key.eq_ignore_ascii_case("path") {
            return Err(BoundedHelperError::PathEnvironmentRefused);
        }
        if key.eq_ignore_ascii_case("systemroot") && !value.is_empty() {
            has_system_root = true;
        }
    }
    if !has_system_root {
        return Err(BoundedHelperError::MissingSystemRoot);
    }
    Ok(())
}

#[cfg(windows)]
struct CanonicalHelperRequest {
    executable: String,
    current_directory: PathBuf,
}

#[cfg(windows)]
fn canonicalize_request(
    request: &BoundedHelperRequest,
) -> Result<CanonicalHelperRequest, BoundedHelperError> {
    validate_request_shape(request)?;
    let package_root = std::fs::canonicalize(&request.package_root)
        .map_err(|_| BoundedHelperError::PackageRootUnavailable)?;
    let executable = std::fs::canonicalize(&request.executable)
        .map_err(|_| BoundedHelperError::ExecutableUnavailable)?;
    if !executable.starts_with(&package_root) {
        return Err(BoundedHelperError::ExecutableOutsidePackage);
    }
    if !executable.is_file() {
        return Err(BoundedHelperError::ExecutableNotFile);
    }
    let current_directory = std::fs::canonicalize(&request.current_directory)
        .map_err(|_| BoundedHelperError::CurrentDirectoryUnavailable)?;
    if !current_directory.starts_with(&package_root) {
        return Err(BoundedHelperError::CurrentDirectoryOutsidePackage);
    }
    if !current_directory.is_dir() {
        return Err(BoundedHelperError::CurrentDirectoryNotDirectory);
    }
    let executable = executable
        .into_os_string()
        .into_string()
        .map_err(|_| BoundedHelperError::PathNotRepresentable)?;
    Ok(CanonicalHelperRequest {
        executable,
        current_directory,
    })
}

#[cfg(windows)]
#[derive(Debug)]
pub(super) enum CaptureError {
    TooLarge,
    Io,
}

#[cfg(all(windows, feature = "test-hooks"))]
thread_local! {
    static FAIL_IO_WORKER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

#[cfg(windows)]
fn spawn_io_worker(name: &str, work: impl FnOnce() + Send + 'static) -> io::Result<()> {
    #[cfg(feature = "test-hooks")]
    if FAIL_IO_WORKER.with(|remaining| match remaining.get() {
        Some(1) => {
            remaining.set(None);
            true
        }
        Some(value) => {
            remaining.set(Some(value - 1));
            false
        }
        None => false,
    }) {
        return Err(io::Error::other(
            "injected helper I/O worker creation failure",
        ));
    }
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(work)
        .map(|_join| ())
}

#[cfg(windows)]
fn capture_stream<R>(
    mut reader: R,
    limit: usize,
) -> io::Result<std::sync::mpsc::Receiver<Result<Vec<u8>, CaptureError>>>
where
    R: io::Read + Send + 'static,
{
    let (sender, receiver) = std::sync::mpsc::channel();
    spawn_io_worker("bounded-helper-output", move || {
        let mut output = Vec::new();
        let mut buffer = [0_u8; 8192];
        let result = loop {
            match reader.read(&mut buffer) {
                Ok(0) => break Ok(output),
                Ok(count) if output.len().saturating_add(count) > limit => {
                    break Err(CaptureError::TooLarge);
                }
                Ok(count) => output.extend_from_slice(&buffer[..count]),
                Err(_) => break Err(CaptureError::Io),
            }
        };
        drop(reader);
        let _ = sender.send(result);
    })?;
    Ok(receiver)
}

#[cfg(windows)]
fn write_input(
    writer: Option<std::fs::File>,
    input: Vec<u8>,
) -> io::Result<std::sync::mpsc::Receiver<io::Result<()>>> {
    use std::io::Write;

    let (sender, receiver) = std::sync::mpsc::channel();
    spawn_io_worker("bounded-helper-input", move || {
        let result = match writer {
            Some(mut writer) => {
                let result = writer.write_all(&input);
                drop(writer);
                result
            }
            None => Err(io::Error::other("owned helper stdin was unavailable")),
        };
        drop(input);
        let _ = sender.send(result);
    })?;
    Ok(receiver)
}

#[cfg(windows)]
fn stop_and_drain(owner: &mut super::job_process::WindowsJobProcess, io: &mut HelperIo) -> bool {
    if super::bounded_cleanup::current_observation_fault_active() {
        return false;
    }
    let drain_deadline = std::time::Instant::now() + super::super::DRAIN_JOIN_TIMEOUT;
    if !owner.is_quiescent().unwrap_or(false) {
        let _ = owner.hard_stop_until(drain_deadline);
    }
    io.drain_until(drain_deadline);
    io.observe();
    owner.is_quiescent().unwrap_or(false)
}

/// Run one package-rooted helper under an atomic, bounded Windows Job scope.
///
/// The returned bytes are deliberately opaque: the dependency owner must
/// validate its own versioned response protocol before acting on them.
#[cfg(windows)]
pub fn run_bounded_helper(
    request: BoundedHelperRequest,
) -> Result<BoundedHelperOutput, BoundedHelperFailure> {
    use std::sync::mpsc::TryRecvError;
    use std::time::Instant;

    use super::job::JobResourceLimits;
    use super::job_process::{WindowsJobLaunchOptions, launch_windows_job_process_with_options};

    let started = Instant::now();
    let canonical = canonicalize_request(&request).map_err(BoundedHelperFailure::prelaunch)?;
    let deadline = started.checked_add(request.budget.timeout).ok_or_else(|| {
        BoundedHelperFailure::prelaunch(BoundedHelperError::AdmissionDeadlineExceeded)
    })?;
    let reservation = Reservation::acquire(deadline)?;
    let mut command = Vec::with_capacity(request.arguments.len() + 1);
    command.push(canonical.executable);
    command.extend(request.arguments.iter().cloned());
    // A request may never choose PATH, but Windows exposes one when it is
    // omitted from a custom block. An explicit empty value makes the absence
    // of caller-controlled search directories observable to the child too.
    let mut environment = request.environment;
    environment.insert(OsString::from("PATH"), OsString::new());
    let resource_limits = request.resource_limits.map(|limits| JobResourceLimits {
        cpu_rate_per_10_000: limits.cpu_rate_per_10_000,
        committed_memory_bytes: limits.committed_memory_bytes,
    });
    let mut owner = match launch_windows_job_process_with_options(
        &command,
        &environment,
        WindowsJobLaunchOptions {
            current_directory: Some(&canonical.current_directory),
            resource_limits,
            exact_environment: true,
            retain_parent_stdin: true,
            null_stdio: [false; 3],
        },
    ) {
        Ok(owner) => owner,
        Err(failure) => return Err(reservation.launch_failure(failure, request.resources)),
    };
    let identity = owner.identity();
    let stdin = owner.take_input_file();
    let (stdout, stderr) = owner.take_output_files();
    // An unstarted slot means no I/O operation exists. A failed Builder::spawn
    // drops its closure and stream; previously started streams remain in `io`.
    let mut io = HelperIo::unstarted();
    match write_input(stdin, request.stdin) {
        Ok(receiver) => io.stdin = Completion::new(receiver),
        Err(_) => {
            drop(stdout);
            drop(stderr);
            let quiescent = stop_and_drain(&mut owner, &mut io);
            return Err(reservation.failure(
                BoundedHelperError::IoWorkerStartFailed {
                    stream: "stdin",
                    identity,
                    quiescent,
                },
                owner,
                io,
                request.resources,
            ));
        }
    }
    match capture_stream(stdout, request.budget.stdout_limit_bytes) {
        Ok(receiver) => io.stdout = Completion::new(receiver),
        Err(_) => {
            drop(stderr);
            let quiescent = stop_and_drain(&mut owner, &mut io);
            return Err(reservation.failure(
                BoundedHelperError::IoWorkerStartFailed {
                    stream: "stdout",
                    identity,
                    quiescent,
                },
                owner,
                io,
                request.resources,
            ));
        }
    }
    match capture_stream(stderr, request.budget.stderr_limit_bytes) {
        Ok(receiver) => io.stderr = Completion::new(receiver),
        Err(_) => {
            let quiescent = stop_and_drain(&mut owner, &mut io);
            return Err(reservation.failure(
                BoundedHelperError::IoWorkerStartFailed {
                    stream: "stderr",
                    identity,
                    quiescent,
                },
                owner,
                io,
                request.resources,
            ));
        }
    }
    if super::bounded_cleanup::current_observation_fault_active() {
        return Err(reservation.failure(
            BoundedHelperError::ProcessObservationFailed {
                identity,
                quiescent: false,
            },
            owner,
            io,
            request.resources,
        ));
    }
    let mut stdin_complete = false;
    let mut stdout_result = None;
    let mut stderr_result = None;
    let mut exit_code = None;

    loop {
        if !stdin_complete {
            match io.stdin.try_recv() {
                Ok(Ok(())) => stdin_complete = true,
                Ok(Err(_)) | Err(TryRecvError::Disconnected) => {
                    let quiescent = stop_and_drain(&mut owner, &mut io);
                    return Err(reservation.failure(
                        BoundedHelperError::InputWriteFailed {
                            identity,
                            quiescent,
                        },
                        owner,
                        io,
                        request.resources,
                    ));
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if stdout_result.is_none() {
            match io.stdout.try_recv() {
                Ok(result) => stdout_result = Some(result),
                Err(TryRecvError::Disconnected) => {
                    let quiescent = stop_and_drain(&mut owner, &mut io);
                    return Err(reservation.failure(
                        BoundedHelperError::OutputReadFailed {
                            stream: "stdout",
                            identity,
                            quiescent,
                        },
                        owner,
                        io,
                        request.resources,
                    ));
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if stderr_result.is_none() {
            match io.stderr.try_recv() {
                Ok(result) => stderr_result = Some(result),
                Err(TryRecvError::Disconnected) => {
                    let quiescent = stop_and_drain(&mut owner, &mut io);
                    return Err(reservation.failure(
                        BoundedHelperError::OutputReadFailed {
                            stream: "stderr",
                            identity,
                            quiescent,
                        },
                        owner,
                        io,
                        request.resources,
                    ));
                }
                Err(TryRecvError::Empty) => {}
            }
        }

        for (stream, result) in [
            ("stdout", stdout_result.as_ref()),
            ("stderr", stderr_result.as_ref()),
        ] {
            match result {
                Some(Err(CaptureError::TooLarge)) => {
                    let quiescent = stop_and_drain(&mut owner, &mut io);
                    return Err(reservation.failure(
                        BoundedHelperError::OutputLimitExceeded {
                            stream,
                            identity,
                            quiescent,
                        },
                        owner,
                        io,
                        request.resources,
                    ));
                }
                Some(Err(CaptureError::Io)) => {
                    let quiescent = stop_and_drain(&mut owner, &mut io);
                    return Err(reservation.failure(
                        BoundedHelperError::OutputReadFailed {
                            stream,
                            identity,
                            quiescent,
                        },
                        owner,
                        io,
                        request.resources,
                    ));
                }
                Some(Ok(_)) | None => {}
            }
        }

        if exit_code.is_none() {
            match owner.poll() {
                Ok(Some(code)) => exit_code = Some(code),
                Ok(None) => {}
                Err(_) => {
                    let quiescent = stop_and_drain(&mut owner, &mut io);
                    return Err(reservation.failure(
                        BoundedHelperError::ProcessObservationFailed {
                            identity,
                            quiescent,
                        },
                        owner,
                        io,
                        request.resources,
                    ));
                }
            }
        }
        if let (Some(exit_code), Some(Ok(_)), Some(Ok(_)), true) = (
            exit_code,
            stdout_result.as_ref(),
            stderr_result.as_ref(),
            stdin_complete,
        ) {
            if !owner.is_quiescent().unwrap_or(false) && !stop_and_drain(&mut owner, &mut io) {
                return Err(reservation.failure(
                    BoundedHelperError::JobNotQuiescent { identity },
                    owner,
                    io,
                    request.resources,
                ));
            }
            let captured_stdout = stdout_result
                .take()
                .expect("completed stdout result remains available")
                .expect("stdout errors returned before completion");
            let captured_stderr = stderr_result
                .take()
                .expect("completed stderr result remains available")
                .expect("stderr errors returned before completion");
            return Ok(BoundedHelperOutput {
                identity,
                exit_code,
                stdout: captured_stdout,
                stderr: captured_stderr,
                quiescent: true,
            });
        }
        if Instant::now() >= deadline {
            let quiescent = stop_and_drain(&mut owner, &mut io);
            return Err(reservation.failure(
                BoundedHelperError::DeadlineExceeded {
                    identity,
                    quiescent,
                },
                owner,
                io,
                request.resources,
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(all(windows, feature = "test-hooks"))]
fn receipt_fixture() -> Result<PathBuf, String> {
    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    current
        .parent()
        .and_then(|directory| directory.parent())
        .map(|directory| directory.join("solstone-system-test-child.exe"))
        .filter(|candidate| candidate.is_file())
        .ok_or_else(|| {
            "could not locate solstone-system-test-child.exe beside test artifacts".to_owned()
        })
}

#[cfg(all(windows, feature = "test-hooks"))]
fn receipt_request(
    fixture: &std::path::Path,
    mode: &str,
    arguments: &[&str],
    stdin: Vec<u8>,
    budget: BoundedHelperBudget,
) -> Result<BoundedHelperRequest, String> {
    let package_root = fixture
        .parent()
        .ok_or_else(|| "fixture has no package-root parent".to_owned())?
        .to_path_buf();
    let system_root = std::env::var_os("SystemRoot")
        .ok_or_else(|| "SystemRoot was unavailable for bounded helper receipt".to_owned())?;
    Ok(BoundedHelperRequest {
        package_root: package_root.clone(),
        executable: fixture.to_path_buf(),
        current_directory: package_root,
        arguments: std::iter::once(mode.to_owned())
            .chain(arguments.iter().map(|argument| (*argument).to_owned()))
            .collect(),
        environment: BTreeMap::from([(OsString::from("SystemRoot"), system_root)]),
        stdin,
        budget,
        resources: BoundedHelperResources::default(),
        resource_limits: Some(BoundedHelperResourceLimits {
            cpu_rate_per_10_000: 2_500,
            committed_memory_bytes: 512 * 1024 * 1024,
        }),
    })
}

// The request bag is the ONLY file-capability owner in this control. The child
// receives ordinary stdio only. This proves resource retention independently of
// the separate descendant generation-borrowing acceptance.
#[cfg(all(windows, feature = "test-hooks"))]
fn retained_resource_receipt_for_test(
    fixture: &std::path::Path,
    budget: BoundedHelperBudget,
) -> Result<(), String> {
    use super::bounded_cleanup::{
        HelperCleanupObservationFault, HelperCleanupStatus,
        run_bounded_helper_with_observation_fault_for_test,
    };
    use std::os::windows::fs::OpenOptionsExt;
    use std::sync::Arc;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    for stage in [
        "child-end-close",
        "primary-thread-close",
        "process-birth",
        "after-io",
    ] {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "solstone-helper-resource-{}-{nonce}-{stage}",
            std::process::id()
        ));
        let resource = Arc::new(
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .share_mode(0)
                .create_new(true)
                .open(&path)
                .map_err(|error| error.to_string())?,
        );
        let weak = Arc::downgrade(&resource);
        let mut request = receipt_request(fixture, "sleep", &[], Vec::new(), budget)?;
        request.resources.retain(resource); // no outer or child-owned duplicate
        let fault = HelperCleanupObservationFault::new();
        let result = super::job_process::with_finalization_fault(stage, || {
            run_bounded_helper_with_observation_fault_for_test(request, &fault)
        });
        let failure = result.err();
        let retained = (|| {
            let failure = failure
                .as_ref()
                .ok_or("resource control unexpectedly succeeded")?;
            let cleanup = failure
                .cleanup()
                .ok_or("resource control returned no native owner")?;
            if cleanup.process_id().is_none()
                || (stage != "after-io" && cleanup.identity().is_some())
                || cleanup.observe() != HelperCleanupStatus::Pending
                || weak.upgrade().is_none()
            {
                return Err(format!(
                    "{stage} did not retain unknown-birth native/resource authority"
                ));
            }
            if stage != "after-io"
                && !matches!(failure.cause(),
                BoundedHelperError::LaunchFinalizationFailed { detail, .. } if detail.contains(stage))
            {
                return Err(format!(
                    "{stage} did not preserve its actual launch failure"
                ));
            }
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .share_mode(0)
                .open(&path)
            {
                Err(error) if error.raw_os_error() == Some(32) => {}
                other => return Err(format!("{stage} sole-owner contention was {other:?}")),
            }
            Ok(())
        })();
        fault.release();
        let settled = failure
            .as_ref()
            .and_then(|failure| failure.cleanup())
            .map(|cleanup| cleanup.retry_until(Instant::now() + super::super::DRAIN_JOIN_TIMEOUT));
        let released = (|| {
            if settled != Some(HelperCleanupStatus::Quiescent) || weak.upgrade().is_some() {
                return Err(format!(
                    "{stage} original native owner/resources did not settle"
                ));
            }
            // Keep the completed Failure/Cleanup Arc alive across reacquisition.
            let reacquired = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .share_mode(0)
                .open(&path)
                .map_err(|error| format!("{stage} reacquisition failed: {error}"))?;
            drop(reacquired);
            Ok(())
        })();
        drop(failure);
        let removed = std::fs::remove_file(&path)
            .map_err(|error| format!("removing fixture {} failed: {error}", path.display()));
        retained?;
        released?;
        removed?;
    }
    Ok(())
}

/// Native receipt for the public bounded-helper authority, retained under the
/// existing Windows Job-owner selector so the source-bound host rail runs it.
#[cfg(all(windows, feature = "test-hooks"))]
pub(super) fn bounded_helper_receipt_for_test() -> Result<(), String> {
    let fixture = receipt_fixture()?;
    let budget = BoundedHelperBudget {
        timeout: Duration::from_secs(2),
        stdin_limit_bytes: 1024,
        stdout_limit_bytes: 1024,
        stderr_limit_bytes: 1024,
    };
    retained_resource_receipt_for_test(&fixture, budget)?;
    for (ordinal, stream) in [(1, "stdin"), (2, "stdout"), (3, "stderr")] {
        let request = receipt_request(&fixture, "sleep", &[], Vec::new(), budget)?;
        FAIL_IO_WORKER.with(|fault| fault.set(Some(ordinal)));
        let result = run_bounded_helper(request);
        FAIL_IO_WORKER.with(|fault| fault.set(None));
        match result {
            Err(failure)
                if matches!(failure.cause(), BoundedHelperError::IoWorkerStartFailed {
                stream: actual, quiescent: true, ..
            } if *actual == stream)
                    && failure.cleanup().is_none() => {}
            other => {
                return Err(format!(
                    "helper I/O worker {ordinal} failure receipt was {other:?}"
                ));
            }
        }
    }
    let result = run_bounded_helper(receipt_request(
        &fixture,
        "echo-stdin",
        &[],
        b"bounded-helper-input".to_vec(),
        budget,
    )?)
    .map_err(|error| error.to_string())?;
    if result.identity.pid == 0
        || result.identity.birth.windows_filetime().is_none()
        || !result.quiescent
        || result.exit_code != 0
        || result.stdout != b"bounded-helper-input"
        || !result.stderr.is_empty()
    {
        return Err(
            "bounded helper receipt did not retain exact identity, I/O, and Job quiescence"
                .to_owned(),
        );
    }

    let current_directory = run_bounded_helper(receipt_request(
        &fixture,
        "current-directory",
        &[],
        Vec::new(),
        budget,
    )?)
    .map_err(|error| error.to_string())?;
    let expected_current_directory = std::fs::canonicalize(
        fixture
            .parent()
            .ok_or_else(|| "fixture has no current-directory parent".to_owned())?,
    )
    .map_err(|error| error.to_string())?;
    let observed_current_directory = std::fs::canonicalize(
        String::from_utf8(current_directory.stdout)
            .map_err(|_| "current-directory receipt was not UTF-8".to_owned())?
            .trim_end(),
    )
    .map_err(|error| error.to_string())?;
    if observed_current_directory != expected_current_directory || !current_directory.quiescent {
        return Err(
            "bounded helper did not retain its explicit package-owned current directory".to_owned(),
        );
    }

    let environment = run_bounded_helper(receipt_request(
        &fixture,
        "environment-empty",
        &["PATH"],
        Vec::new(),
        budget,
    )?)
    .map_err(|error| error.to_string())?;
    if environment.stdout != b"empty\n" || !environment.quiescent {
        return Err(format!(
            "bounded helper did not receive its empty PATH boundary: stdout={:?}, quiescent={}",
            environment.stdout, environment.quiescent
        ));
    }

    let mut output_budget = budget;
    output_budget.stdout_limit_bytes = 8;
    match run_bounded_helper(receipt_request(
        &fixture,
        "write-stdout",
        &["9"],
        Vec::new(),
        output_budget,
    )?) {
        Err(failure)
            if failure.cleanup().is_none()
                && matches!(
                    failure.cause(),
                    BoundedHelperError::OutputLimitExceeded {
                        stream: "stdout",
                        quiescent: true,
                        ..
                    }
                ) => {}
        other => return Err(format!("bounded helper stdout cap receipt was {other:?}")),
    }

    let mut timeout_budget = budget;
    timeout_budget.timeout = Duration::from_millis(100);
    match run_bounded_helper(receipt_request(
        &fixture,
        "sleep",
        &[],
        Vec::new(),
        timeout_budget,
    )?) {
        Err(failure)
            if failure.cleanup().is_none()
                && matches!(
                    failure.cause(),
                    BoundedHelperError::DeadlineExceeded {
                        quiescent: true,
                        ..
                    }
                ) => {}
        other => return Err(format!("bounded helper deadline receipt was {other:?}")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> BoundedHelperRequest {
        BoundedHelperRequest {
            package_root: PathBuf::from("package"),
            executable: PathBuf::from("package/helper.exe"),
            current_directory: PathBuf::from("package"),
            arguments: Vec::new(),
            environment: BTreeMap::from([(
                OsString::from("SystemRoot"),
                OsString::from("C:\\Windows"),
            )]),
            stdin: Vec::new(),
            budget: BoundedHelperBudget {
                timeout: Duration::from_secs(1),
                stdin_limit_bytes: 1,
                stdout_limit_bytes: 1,
                stderr_limit_bytes: 1,
            },
            resource_limits: None,
            #[cfg(windows)]
            resources: BoundedHelperResources::default(),
        }
    }

    #[test]
    fn request_shape_requires_explicit_nonzero_budgets_and_system_root() {
        let mut candidate = request();
        candidate.budget.timeout = Duration::ZERO;
        assert_eq!(
            validate_request_shape(&candidate),
            Err(BoundedHelperError::ZeroTimeout)
        );

        let mut candidate = request();
        candidate.budget.stdout_limit_bytes = 0;
        assert_eq!(
            validate_request_shape(&candidate),
            Err(BoundedHelperError::ZeroLimit { stream: "stdout" })
        );

        let mut candidate = request();
        candidate.environment.clear();
        assert_eq!(
            validate_request_shape(&candidate),
            Err(BoundedHelperError::MissingSystemRoot)
        );
    }

    #[test]
    fn request_shape_refuses_input_overflow_and_path_environment() {
        let mut candidate = request();
        candidate.stdin = vec![1, 2];
        assert_eq!(
            validate_request_shape(&candidate),
            Err(BoundedHelperError::InputLimitExceeded)
        );

        let mut candidate = request();
        candidate
            .environment
            .insert(OsString::from("Path"), OsString::from("C:\\poison"));
        assert_eq!(
            validate_request_shape(&candidate),
            Err(BoundedHelperError::PathEnvironmentRefused)
        );
    }
}
