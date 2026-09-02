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
enum CaptureError {
    TooLarge,
    Io,
}

#[cfg(windows)]
fn capture_stream<R>(
    mut reader: R,
    limit: usize,
) -> std::sync::mpsc::Receiver<Result<Vec<u8>, CaptureError>>
where
    R: io::Read + Send + 'static,
{
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
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
        let _ = sender.send(result);
    });
    receiver
}

#[cfg(windows)]
fn write_input(
    mut writer: std::fs::File,
    input: Vec<u8>,
) -> std::sync::mpsc::Receiver<io::Result<()>> {
    use std::io::Write;

    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = writer.write_all(&input);
        let _ = sender.send(result);
    });
    receiver
}

#[cfg(windows)]
fn drain_receiver<T>(receiver: &std::sync::mpsc::Receiver<T>, deadline: std::time::Instant) {
    let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
        return;
    };
    let _ = receiver.recv_timeout(remaining);
}

#[cfg(windows)]
fn stop_and_drain(
    owner: &mut super::job_process::WindowsJobProcess,
    stdin: &std::sync::mpsc::Receiver<io::Result<()>>,
    stdout: &std::sync::mpsc::Receiver<Result<Vec<u8>, CaptureError>>,
    stderr: &std::sync::mpsc::Receiver<Result<Vec<u8>, CaptureError>>,
) -> bool {
    use super::super::DRAIN_JOIN_TIMEOUT;

    let drain_deadline = std::time::Instant::now() + DRAIN_JOIN_TIMEOUT;
    if !owner.is_quiescent().unwrap_or(false) {
        let _ = owner.hard_stop_until(drain_deadline);
    }
    drain_receiver(stdin, drain_deadline);
    drain_receiver(stdout, drain_deadline);
    drain_receiver(stderr, drain_deadline);
    owner.is_quiescent().unwrap_or(false)
}

/// Run one package-rooted helper under an atomic, bounded Windows Job scope.
///
/// The returned bytes are deliberately opaque: the dependency owner must
/// validate its own versioned response protocol before acting on them.
#[cfg(windows)]
pub fn run_bounded_helper(
    request: BoundedHelperRequest,
) -> Result<BoundedHelperOutput, BoundedHelperError> {
    use std::sync::mpsc::TryRecvError;
    use std::time::Instant;

    use super::job::JobResourceLimits;
    use super::job_process::{WindowsJobLaunchOptions, launch_windows_job_process_with_options};

    let canonical = canonicalize_request(&request)?;
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
    let mut owner = launch_windows_job_process_with_options(
        &command,
        &environment,
        WindowsJobLaunchOptions {
            current_directory: Some(&canonical.current_directory),
            resource_limits,
            exact_environment: true,
            retain_parent_stdin: true,
        },
    )
    .map_err(|_| BoundedHelperError::LaunchFailed)?;
    let identity = owner.identity();
    let stdin = owner
        .take_input_file()
        .ok_or(BoundedHelperError::LaunchFailed)?;
    let (stdout, stderr) = owner.take_output_files();
    let stdin = write_input(stdin, request.stdin);
    let stdout = capture_stream(stdout, request.budget.stdout_limit_bytes);
    let stderr = capture_stream(stderr, request.budget.stderr_limit_bytes);
    let deadline = Instant::now() + request.budget.timeout;
    let mut stdin_complete = false;
    let mut stdout_result = None;
    let mut stderr_result = None;
    let mut exit_code = None;

    loop {
        if !stdin_complete {
            match stdin.try_recv() {
                Ok(Ok(())) => stdin_complete = true,
                Ok(Err(_)) | Err(TryRecvError::Disconnected) => {
                    let quiescent = stop_and_drain(&mut owner, &stdin, &stdout, &stderr);
                    return Err(BoundedHelperError::InputWriteFailed {
                        identity,
                        quiescent,
                    });
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if stdout_result.is_none() {
            match stdout.try_recv() {
                Ok(result) => stdout_result = Some(result),
                Err(TryRecvError::Disconnected) => {
                    let quiescent = stop_and_drain(&mut owner, &stdin, &stdout, &stderr);
                    return Err(BoundedHelperError::OutputReadFailed {
                        stream: "stdout",
                        identity,
                        quiescent,
                    });
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if stderr_result.is_none() {
            match stderr.try_recv() {
                Ok(result) => stderr_result = Some(result),
                Err(TryRecvError::Disconnected) => {
                    let quiescent = stop_and_drain(&mut owner, &stdin, &stdout, &stderr);
                    return Err(BoundedHelperError::OutputReadFailed {
                        stream: "stderr",
                        identity,
                        quiescent,
                    });
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
                    let quiescent = stop_and_drain(&mut owner, &stdin, &stdout, &stderr);
                    return Err(BoundedHelperError::OutputLimitExceeded {
                        stream,
                        identity,
                        quiescent,
                    });
                }
                Some(Err(CaptureError::Io)) => {
                    let quiescent = stop_and_drain(&mut owner, &stdin, &stdout, &stderr);
                    return Err(BoundedHelperError::OutputReadFailed {
                        stream,
                        identity,
                        quiescent,
                    });
                }
                Some(Ok(_)) | None => {}
            }
        }

        if exit_code.is_none() {
            match owner.poll() {
                Ok(Some(code)) => exit_code = Some(code),
                Ok(None) => {}
                Err(_) => {
                    let quiescent = stop_and_drain(&mut owner, &stdin, &stdout, &stderr);
                    return Err(BoundedHelperError::ProcessObservationFailed {
                        identity,
                        quiescent,
                    });
                }
            }
        }
        if let (Some(exit_code), Some(Ok(_)), Some(Ok(_)), true) = (
            exit_code,
            stdout_result.as_ref(),
            stderr_result.as_ref(),
            stdin_complete,
        ) {
            if !owner.is_quiescent().unwrap_or(false) {
                let _ = stop_and_drain(&mut owner, &stdin, &stdout, &stderr);
                return Err(BoundedHelperError::JobNotQuiescent { identity });
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
            let quiescent = stop_and_drain(&mut owner, &stdin, &stdout, &stderr);
            return Err(BoundedHelperError::DeadlineExceeded {
                identity,
                quiescent,
            });
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
        resource_limits: Some(BoundedHelperResourceLimits {
            cpu_rate_per_10_000: 2_500,
            committed_memory_bytes: 512 * 1024 * 1024,
        }),
    })
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
        Err(BoundedHelperError::OutputLimitExceeded {
            stream: "stdout",
            quiescent: true,
            ..
        }) => {}
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
        Err(BoundedHelperError::DeadlineExceeded {
            quiescent: true, ..
        }) => {}
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
