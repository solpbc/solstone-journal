// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use solstone_core_backup::archive_admission::ArchiveCancellation;
#[cfg(windows)]
use solstone_core_backup::archive_readback::{
    WindowsArchiveCleanup, WindowsArchiveExecutor, WindowsArchiveRequest, WindowsExecutionOutput,
};
#[cfg(windows)]
use solstone_core_distribution::windows_payload::{WINDOWS_RCLONE_WORKER, WINDOWS_RESTIC_WORKER};
#[cfg(windows)]
use std::sync::Arc;
use thiserror::Error;

/// Execution authority captured by resolution and retained for each operation.
#[derive(Clone, Copy, Debug)]
pub enum ResticInvocation<'a> {
    Executable(&'a Path),
    #[cfg(target_os = "ios")]
    Callable(&'a crate::ios_callable::AdmittedIosBackupAuthority),
}

impl<'a> ResticInvocation<'a> {
    /// Executable-only operations must refuse a callable capability explicitly.
    pub fn executable_path(self) -> Option<&'a Path> {
        match self {
            Self::Executable(path) => Some(path),
            #[cfg(target_os = "ios")]
            Self::Callable(_) => None,
        }
    }
}

impl<'a> From<&'a Path> for ResticInvocation<'a> {
    fn from(path: &'a Path) -> Self {
        Self::Executable(path)
    }
}

impl<'a> From<&'a std::path::PathBuf> for ResticInvocation<'a> {
    fn from(path: &'a std::path::PathBuf) -> Self {
        Self::Executable(path.as_path())
    }
}

/// An inherited OS handle explicitly passed to a backup child process.
///
/// Windows backup execution does not support inherited handles; keeping this
/// target-shaped type lets ordinary, descriptor-free restic requests retain a
/// uniform request contract while that unsupported path fails closed.
#[cfg(unix)]
pub type PassedHandle<'a> = std::os::fd::BorrowedFd<'a>;
#[cfg(windows)]
pub type PassedHandle<'a> = std::os::windows::io::BorrowedHandle<'a>;
#[cfg(not(any(unix, windows)))]
#[derive(Clone, Copy)]
pub struct PassedHandle<'a>(std::marker::PhantomData<&'a ()>);

#[derive(Clone)]
pub struct ToolRequest<'a> {
    pub program: OsString,
    pub argv: Vec<OsString>,
    pub env: BTreeMap<OsString, OsString>,
    pub timeout: Option<Duration>,
    pub pass_fds: Vec<PassedHandle<'a>>,
    /// Windows-only protected input. Unix refuses `Some` and retains descriptor transport.
    pub stdin: Option<Vec<u8>>,
    #[cfg(windows)]
    pub resources: solstone_core_system::process::BoundedHelperResources,
}
impl fmt::Debug for ToolRequest<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("ToolRequest");
        #[cfg(windows)]
        debug.field("resources", &self.resources.len());
        debug
            .field("program", &self.program)
            .field("argv", &self.argv)
            .field("env", &"<redacted>")
            .field("timeout", &self.timeout)
            .field("pass_fds", &self.pass_fds.len())
            .field("stdin", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub returncode: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}
impl fmt::Debug for ToolOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ToolOutput")
            .field("returncode", &self.returncode)
            .field("stdout", &"<redacted>")
            .field("stderr", &"<redacted>")
            .finish()
    }
}

pub trait ToolRunner {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput>;

    fn run_cancellable(
        &self,
        _request: &ToolRequest<'_>,
        _cancellation: &ArchiveCancellation,
    ) -> io::Result<ToolOutput> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "cancellable tool execution is unsupported",
        ))
    }

    fn cancellation(&self) -> Option<&ArchiveCancellation> {
        None
    }

    #[cfg(target_os = "ios")]
    fn ios_journal_root(&self) -> Option<&std::sync::Arc<solstone_core_journal_io::JournalRoot>> {
        None
    }

    /// The admitted target of the whole-journal restore this runner serves, if any.
    #[cfg(target_os = "ios")]
    fn ios_restore_target(&self) -> Option<&crate::ios_callable::RestoreTarget> {
        None
    }
}

#[derive(Debug, Default)]
pub struct SystemToolRunner;

// restic's -o flag parses CSV before rclone splits the program shell string.
// Encode only after program admission, preserving that exact logical value.
fn encode_restic_csv_option(option: &str) -> String {
    format!("\"{}\"", option.replace('"', "\"\""))
}

/// The argv a process receives. The quoted `rclone.program` option must reach
/// restic as one CSV field: a bare `"` in it is a parse error and the backup
/// never starts.
#[cfg(unix)]
fn spawn_argv(argv: &[OsString]) -> Vec<OsString> {
    argv.iter()
        .map(|arg| match arg.to_str() {
            Some(text) if text.starts_with("rclone.program=") => {
                OsString::from(encode_restic_csv_option(text))
            }
            _ => arg.clone(),
        })
        .collect()
}

#[cfg(unix)]
impl ToolRunner for SystemToolRunner {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        run_unix_tool(request, None)
    }

    fn run_cancellable(
        &self,
        request: &ToolRequest<'_>,
        cancellation: &ArchiveCancellation,
    ) -> io::Result<ToolOutput> {
        run_unix_tool(request, Some(cancellation))
    }
}

#[cfg(unix)]
fn run_unix_tool(
    request: &ToolRequest<'_>,
    cancellation: Option<&ArchiveCancellation>,
) -> io::Result<ToolOutput> {
    if request.stdin.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "stdin not supported on unix tool runner",
        ));
    }
    let request = solstone_core_backup::bounded_child::Request {
        program: request.program.clone(),
        argv: spawn_argv(&request.argv),
        env: request.env.clone(),
        timeout: request.timeout,
        pass_fds: request.pass_fds.clone(),
    };
    let output = match cancellation {
        Some(cancellation) => {
            solstone_core_backup::bounded_child::run_cancellable(&request, cancellation)
        }
        None => solstone_core_backup::bounded_child::run(&request),
    }?;
    Ok(ToolOutput {
        returncode: output.returncode,
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

#[cfg(windows)]
#[allow(clippy::too_many_arguments)] // Mirrors the independent process-boundary inputs below.
fn launch_windows_helper(
    program: &Path,
    argv: &[OsString],
    env: &BTreeMap<OsString, OsString>,
    timeout: Duration,
    stdin: Option<Vec<u8>>,
    resources: solstone_core_system::process::BoundedHelperResources,
    stdout_limit_bytes: usize,
    stdout_sink: Option<solstone_core_system::process::BoundedStdoutSink>,
    cancellation: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
) -> io::Result<ToolOutput> {
    let (bin_dir, package_root) = crate::windows_tool::resolve_package_bin_and_root()?;
    let payload =
        solstone_core_distribution::windows_payload::verify_windows_payload(&package_root)
            .map_err(|error| io::Error::new(io::ErrorKind::PermissionDenied, error.to_string()))?;

    let refused_tool = |error: solstone_core_distribution::windows_payload::WindowsPayloadError| {
        io::Error::new(io::ErrorKind::PermissionDenied, error.to_string())
    };
    let admitted_restic = payload
        .declared_path(WINDOWS_RESTIC_WORKER)
        .map_err(refused_tool)?;
    let admitted_rclone = payload
        .declared_path(WINDOWS_RCLONE_WORKER)
        .map_err(refused_tool)?;
    let canonical_program = program.canonicalize()?;
    let canonical_restic = admitted_restic.canonicalize()?;
    let canonical_rclone = admitted_rclone.canonicalize()?;
    if canonical_program != canonical_restic && canonical_program != canonical_rclone {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unadmitted backup executable",
        ));
    }
    let arguments = argv
        .iter()
        .map(|arg| {
            arg.to_str().map(str::to_owned).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "backup argument is not Unicode",
                )
            })
        })
        .collect::<io::Result<Vec<_>>>()?;
    let mut program_options = 0;
    let mut args_options = 0;
    for arg in &arguments {
        if let Some(value) = arg.strip_prefix("rclone.program=") {
            let unquoted = value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .filter(|value| !value.contains('"'))
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "unadmitted rclone program option",
                    )
                })?;
            if Path::new(unquoted).canonicalize()? != canonical_rclone {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "unadmitted rclone executable",
                ));
            }
            program_options += 1;
        } else if let Some(value) = arg.strip_prefix("rclone.args=") {
            if value != "serve restic --stdio --append-only --config NUL" {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "unadmitted rclone arguments",
                ));
            }
            args_options += 1;
        }
    }
    let uses_rclone = env
        .get(std::ffi::OsStr::new("RESTIC_REPOSITORY"))
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.starts_with("rclone:"));
    if (uses_rclone && (program_options != 1 || args_options != 1))
        || program_options > 1
        || args_options > 1
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "rclone transport requires the declared append-only configuration",
        ));
    }

    if timeout.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "timeout must be non-zero",
        ));
    }

    let budget = solstone_core_system::process::BoundedHelperBudget {
        stdin_limit_bytes: 64 * 1024,
        stdout_limit_bytes: stdout_limit_bytes.max(1),
        stderr_limit_bytes: 16 * 1024 * 1024,
        timeout,
    };

    let mut environment = env.clone();
    // The native runner needs the Windows root; restic also uses the owner's
    // temporary directory. Keep credential and command settings explicit.
    for name in ["SystemRoot", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(name).filter(|value| !value.is_empty()) {
            environment.entry(name.into()).or_insert(value);
        }
    }
    let bounded_request = solstone_core_system::process::BoundedHelperRequest {
        package_root,
        executable: canonical_program,
        arguments: arguments
            .into_iter()
            .map(|argument| {
                if argument.starts_with("rclone.program=") {
                    encode_restic_csv_option(&argument)
                } else {
                    argument
                }
            })
            .collect(),
        current_directory: bin_dir,
        environment,
        stdin: stdin.unwrap_or_default(),
        budget,
        resource_limits: None,
        resources,
        stdout_sink,
        cancellation,
    };

    match solstone_core_system::process::run_bounded_helper(bounded_request) {
        Ok(output) if output.quiescent => Ok(ToolOutput {
            returncode: output.exit_code,
            stdout: output.stdout,
            stderr: output.stderr,
        }),
        Ok(_) => Err(io::Error::other(
            "backup helper did not establish quiescence",
        )),
        Err(failure)
            if matches!(
                failure.cause(),
                solstone_core_system::process::BoundedHelperError::DeadlineExceeded {
                    quiescent: true,
                    ..
                }
            ) && failure.cleanup().is_none() =>
        {
            Ok(ToolOutput {
                returncode: 124,
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
        }
        Err(failure) => {
            let kind = if matches!(
                failure.cause(),
                solstone_core_system::process::BoundedHelperError::DeadlineExceeded { .. }
            ) {
                io::ErrorKind::TimedOut
            } else {
                io::ErrorKind::Other
            };
            Err(io::Error::new(kind, failure))
        }
    }
}

#[cfg(windows)]
impl ToolRunner for SystemToolRunner {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        if !request.pass_fds.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "passed file descriptors/handles are unsupported on windows",
            ));
        }

        let timeout = match request.timeout {
            Some(d) if d.is_zero() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "timeout must be non-zero",
                ));
            }
            Some(d) => d,
            None => Duration::from_secs(48 * 60 * 60),
        };

        launch_windows_helper(
            Path::new(&request.program),
            &request.argv,
            &request.env,
            timeout,
            request.stdin.clone(),
            request.resources.clone(),
            64 * 1024 * 1024,
            None,
            None,
        )
    }

    fn run_cancellable(
        &self,
        request: &ToolRequest<'_>,
        cancellation: &ArchiveCancellation,
    ) -> io::Result<ToolOutput> {
        if !request.pass_fds.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "passed file descriptors/handles are unsupported on windows",
            ));
        }

        let timeout = match request.timeout {
            Some(d) if d.is_zero() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "timeout must be non-zero",
                ));
            }
            Some(d) => d,
            None => Duration::from_secs(48 * 60 * 60),
        };

        let cancel_fn = Some(Arc::new({
            let cancellation = cancellation.clone();
            move || cancellation.is_cancelled()
        }) as Arc<dyn Fn() -> bool + Send + Sync>);

        launch_windows_helper(
            Path::new(&request.program),
            &request.argv,
            &request.env,
            timeout,
            request.stdin.clone(),
            request.resources.clone(),
            64 * 1024 * 1024,
            None,
            cancel_fn,
        )
    }
}

#[cfg(windows)]
struct WindowsArchiveCleanupOwner(solstone_core_system::process::BoundedHelperCleanup);

#[cfg(windows)]
impl WindowsArchiveCleanup for WindowsArchiveCleanupOwner {
    fn quiescent(&self) -> bool {
        self.0.observe() == solstone_core_system::process::HelperCleanupStatus::Quiescent
    }
}

#[cfg(windows)]
fn map_windows_helper_result(result: io::Result<ToolOutput>) -> WindowsExecutionOutput {
    match result {
        Ok(output) => WindowsExecutionOutput {
            returncode: output.returncode,
            stdout: output.stdout,
            stderr: output.stderr,
            cleanup: None,
        },
        Err(error) => {
            let cleanup = error
                .get_ref()
                .and_then(|error| {
                    error.downcast_ref::<solstone_core_system::process::BoundedHelperFailure>()
                })
                .and_then(|failure| failure.cleanup().cloned())
                .map(|cleanup| {
                    Arc::new(WindowsArchiveCleanupOwner(cleanup)) as Arc<dyn WindowsArchiveCleanup>
                });
            WindowsExecutionOutput {
                returncode: 1,
                stdout: Vec::new(),
                stderr: error.to_string().into_bytes(),
                cleanup,
            }
        }
    }
}

#[cfg(windows)]
impl WindowsArchiveExecutor for SystemToolRunner {
    fn run_cancellable(
        &self,
        request: WindowsArchiveRequest<'_>,
        capture_limit: usize,
    ) -> WindowsExecutionOutput {
        let mut resources = solstone_core_system::process::BoundedHelperResources::new();
        resources.retain(Arc::new(request.retained));
        let cancellation = request.cancellation.map(|cancellation| {
            let cancellation = cancellation.clone();
            Arc::new(move || cancellation.is_cancelled()) as Arc<dyn Fn() -> bool + Send + Sync>
        });
        map_windows_helper_result(launch_windows_helper(
            request.program,
            request.argv,
            request.env,
            request.timeout,
            None,
            resources,
            capture_limit.clamp(1, 64 * 1024 * 1024),
            None,
            cancellation,
        ))
    }

    fn run_dump(
        &self,
        request: WindowsArchiveRequest<'_>,
        stage: std::fs::File,
        byte_bound: u64,
    ) -> WindowsExecutionOutput {
        let mut resources = solstone_core_system::process::BoundedHelperResources::new();
        resources.retain(Arc::new(request.retained));
        let cancellation = request.cancellation.map(|cancellation| {
            let cancellation = cancellation.clone();
            Arc::new(move || cancellation.is_cancelled()) as Arc<dyn Fn() -> bool + Send + Sync>
        });
        map_windows_helper_result(launch_windows_helper(
            request.program,
            request.argv,
            request.env,
            request.timeout,
            None,
            resources,
            64 * 1024 * 1024,
            Some(solstone_core_system::process::BoundedStdoutSink {
                file: stage,
                limit_bytes: byte_bound,
            }),
            cancellation,
        ))
    }
}

#[cfg(not(any(unix, windows)))]
impl ToolRunner for SystemToolRunner {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        let _ = request;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the backup child-process runner is unsupported on this platform",
        ))
    }
}

#[derive(Clone, PartialEq)]
pub struct ResticResult {
    pub returncode: i32,
    pub stdout: String,
    pub stderr: String,
    pub json: Option<Value>,
    pub argv: Vec<String>,
}
impl fmt::Debug for ResticResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResticResult")
            .field("returncode", &self.returncode)
            .field("stdout", &"<redacted>")
            .field("stderr", &"<redacted>")
            .field("json", &self.json)
            .field("argv", &self.argv)
            .finish()
    }
}

#[derive(Debug, Error)]
pub enum RunnerError {
    #[error("tool program must include a path separator")]
    BareProgram,
    #[error("restic --insecure-tls is forbidden")]
    InsecureTls,
    #[error("restic argv contains a secret")]
    SecretInArgv,
    #[error("restic process could not start")]
    Process(#[source] io::Error),
}

pub fn reason_for_returncode(returncode: i32) -> &'static str {
    match returncode {
        3 => "incomplete",
        10 => "repo_missing",
        11 => "locked",
        12 => "auth_failed",
        124 => "timeout",
        _ => "failed",
    }
}

pub fn select_summary(parsed: &Value) -> Option<&serde_json::Map<String, Value>> {
    match parsed {
        Value::Object(record)
            if record.get("message_type") == Some(&Value::String("summary".into())) =>
        {
            Some(record)
        }
        Value::Array(records) => records.iter().rev().find_map(|record| match record {
            Value::Object(record)
                if record.get("message_type") == Some(&Value::String("summary".into())) =>
            {
                Some(record)
            }
            _ => None,
        }),
        _ => None,
    }
}

pub(crate) fn is_explicit_program_path(path: &Path) -> bool {
    path.components().count() > 1
}

#[allow(clippy::too_many_arguments)] // Mirrors restic's independent process-boundary inputs.
pub fn run_restic<'invocation>(
    runner: &dyn ToolRunner,
    args: &[String],
    repository: &str,
    password: &str,
    restic_path: impl Into<ResticInvocation<'invocation>>,
    backend_env: Option<&BTreeMap<String, Option<String>>>,
    json: bool,
    max_repack_size: Option<&str>,
    timeout: Option<Duration>,
    pass_fds: &[PassedHandle<'_>],
) -> Result<ResticResult, RunnerError> {
    run_restic_with_stdin(
        runner,
        args,
        repository,
        password,
        restic_path,
        backend_env,
        json,
        max_repack_size,
        timeout,
        pass_fds,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_restic_with_stdin<'invocation>(
    runner: &dyn ToolRunner,
    args: &[String],
    repository: &str,
    password: &str,
    restic_path: impl Into<ResticInvocation<'invocation>>,
    backend_env: Option<&BTreeMap<String, Option<String>>>,
    json: bool,
    max_repack_size: Option<&str>,
    timeout: Option<Duration>,
    pass_fds: &[PassedHandle<'_>],
    stdin: Option<Vec<u8>>,
) -> Result<ResticResult, RunnerError> {
    let invocation = restic_path.into();
    #[cfg(target_os = "ios")]
    if let ResticInvocation::Callable(authority) = invocation {
        return crate::ios_callable::run(
            authority,
            args,
            repository,
            password,
            backend_env,
            json,
            max_repack_size,
            timeout,
            pass_fds,
            stdin,
            runner.cancellation(),
            runner.ios_restore_target(),
        );
    }
    let restic_path = invocation.executable_path().ok_or_else(|| {
        RunnerError::Process(io::Error::new(
            io::ErrorKind::Unsupported,
            "executable backup authority required",
        ))
    })?;

    if !is_explicit_program_path(restic_path) {
        return Err(RunnerError::BareProgram);
    }
    let (env, mut secrets) = child_env(repository, password, backend_env);
    if let Some(ref stdin_bytes) = stdin {
        let text = String::from_utf8_lossy(stdin_bytes);
        let stripped = text.strip_suffix('\n').unwrap_or(&text);
        if !stripped.is_empty() {
            secrets.push(stripped.to_owned());
        }
    }
    let mut argv = args.to_vec();
    if json {
        argv.push("--json".into());
    }
    if let Some(size) = max_repack_size {
        argv.extend(["--max-repack-size".into(), size.into()]);
    }
    // Android app children have no desktop home/cache environment. Restic init
    // works without a cache, but key and backup operations otherwise fail while
    // opening one. Keep app execution explicit without inventing an ambient home
    // or writing an unadmitted cache outside the app's retained authorities.
    #[cfg(target_os = "android")]
    if !argv.iter().any(|arg| arg == "--no-cache") {
        argv.push("--no-cache".into());
    }
    guard_argv(&argv, &secrets)?;
    let output = runner
        .run(&ToolRequest {
            program: restic_path.as_os_str().to_os_string(),
            argv: argv.iter().map(OsString::from).collect(),
            env,
            timeout,
            pass_fds: pass_fds.to_vec(),
            stdin,
            #[cfg(windows)]
            resources: Default::default(),
        })
        .map_err(RunnerError::Process)?;
    let stdout = scrub(&String::from_utf8_lossy(&output.stdout), &secrets);
    let stderr = scrub(&String::from_utf8_lossy(&output.stderr), &secrets);
    let parsed = if json && output.returncode != 124 {
        parse_json(&stdout)
    } else {
        None
    };
    Ok(ResticResult {
        returncode: output.returncode,
        stdout,
        stderr,
        json: parsed,
        argv,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_restic_with_stdin_cancellable<'invocation>(
    runner: &dyn ToolRunner,
    args: &[String],
    repository: &str,
    password: &str,
    restic_path: impl Into<ResticInvocation<'invocation>>,
    backend_env: Option<&BTreeMap<String, Option<String>>>,
    json: bool,
    max_repack_size: Option<&str>,
    timeout: Option<Duration>,
    pass_fds: &[PassedHandle<'_>],
    stdin: Option<Vec<u8>>,
    cancellation: &ArchiveCancellation,
) -> Result<ResticResult, RunnerError> {
    let invocation = restic_path.into();
    #[cfg(target_os = "ios")]
    if let ResticInvocation::Callable(authority) = invocation {
        return crate::ios_callable::run(
            authority,
            args,
            repository,
            password,
            backend_env,
            json,
            max_repack_size,
            timeout,
            pass_fds,
            stdin,
            Some(cancellation),
            runner.ios_restore_target(),
        );
    }
    let restic_path = invocation.executable_path().ok_or_else(|| {
        RunnerError::Process(io::Error::new(
            io::ErrorKind::Unsupported,
            "executable backup authority required",
        ))
    })?;

    if !is_explicit_program_path(restic_path) {
        return Err(RunnerError::BareProgram);
    }
    let (env, mut secrets) = child_env(repository, password, backend_env);
    if let Some(ref stdin_bytes) = stdin {
        let text = String::from_utf8_lossy(stdin_bytes);
        let stripped = text.strip_suffix('\n').unwrap_or(&text);
        if !stripped.is_empty() {
            secrets.push(stripped.to_owned());
        }
    }
    let mut argv = args.to_vec();
    // Match ordinary Android restic calls: the app has no ambient HOME cache.
    #[cfg(target_os = "android")]
    if !argv.iter().any(|arg| arg == "--no-cache") {
        argv.push("--no-cache".into());
    }
    if json {
        argv.push("--json".into());
    }
    if let Some(size) = max_repack_size {
        argv.extend(["--max-repack-size".into(), size.into()]);
    }
    guard_argv(&argv, &secrets)?;
    let output = runner
        .run_cancellable(
            &ToolRequest {
                program: restic_path.as_os_str().to_os_string(),
                argv: argv.iter().map(OsString::from).collect(),
                env,
                timeout,
                pass_fds: pass_fds.to_vec(),
                stdin,
                #[cfg(windows)]
                resources: Default::default(),
            },
            cancellation,
        )
        .map_err(RunnerError::Process)?;
    let stdout = scrub(&String::from_utf8_lossy(&output.stdout), &secrets);
    let stderr = scrub(&String::from_utf8_lossy(&output.stderr), &secrets);
    let parsed = if json && output.returncode != 124 {
        parse_json(&stdout)
    } else {
        None
    };
    Ok(ResticResult {
        returncode: output.returncode,
        stdout,
        stderr,
        json: parsed,
        argv,
    })
}

pub fn child_env(
    repository: &str,
    password: &str,
    backend_env: Option<&BTreeMap<String, Option<String>>>,
) -> (BTreeMap<OsString, OsString>, Vec<String>) {
    let mut env = BTreeMap::new();
    #[cfg(unix)]
    {
        for key in ["PATH", "HOME", "TMPDIR"] {
            if let Some(value) = env::var_os(key) {
                env.insert(key.into(), value);
            }
        }
    }
    #[cfg(windows)]
    {
        if let Some(value) = env::var_os("SystemRoot").filter(|v| !v.is_empty()) {
            env.insert("SystemRoot".into(), value);
        }
        for key in ["TEMP", "TMP", "USERPROFILE", "LOCALAPPDATA"] {
            if let Some(value) = env::var_os(key).filter(|v| !v.is_empty()) {
                env.insert(key.into(), value);
            }
        }
    }
    env.insert("RESTIC_REPOSITORY".into(), repository.into());
    env.insert("RESTIC_PASSWORD".into(), password.into());
    let mut secrets = vec![password.to_owned()];
    if let Some(backend_env) = backend_env {
        for (key, value) in backend_env {
            if let Some(value) = value {
                env.insert(key.into(), value.into());
                // These exact hosted transport constants are public settings. Treating
                // them as secrets rejects paths containing "s3" and corrupts JSON booleans.
                // Unknown keys and unexpected values remain protected by default.
                let public_setting = matches!(
                    (key.as_str(), value.as_str()),
                    ("RCLONE_CONFIG_SPB_TYPE", "s3")
                        | ("RCLONE_CONFIG_SPB_PROVIDER", "Cloudflare")
                        | ("RCLONE_CONFIG_SPB_ENV_AUTH", "false")
                        | ("RCLONE_CONFIG_SPB_REGION", "auto")
                        | ("RCLONE_CONFIG_SPB_NO_CHECK_BUCKET", "true")
                        | ("TZ", "UTC")
                );
                if !value.is_empty() && !public_setting {
                    secrets.push(value.clone());
                }
            }
        }
    }
    (env, secrets)
}

pub fn guard_argv(argv: &[String], secrets: &[String]) -> Result<(), RunnerError> {
    if argv.iter().any(|arg| arg == "--insecure-tls") {
        return Err(RunnerError::InsecureTls);
    }
    if argv.iter().any(|arg| {
        secrets
            .iter()
            .filter(|secret| !secret.is_empty())
            .any(|secret| arg.contains(secret))
    }) {
        return Err(RunnerError::SecretInArgv);
    }
    Ok(())
}

fn scrub(value: &str, secrets: &[String]) -> String {
    secrets
        .iter()
        .filter(|secret| !secret.is_empty())
        .fold(value.to_owned(), |text, secret| {
            text.replace(secret, "[redacted]")
        })
}
fn parse_json(text: &str) -> Option<Value> {
    if text.trim().is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str(text) {
        return Some(value);
    }
    let lines = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    (!lines.is_empty()).then_some(Value::Array(lines))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct Fixture;
    impl ToolRunner for Fixture {
        fn run(&self, _: &ToolRequest) -> io::Result<ToolOutput> {
            Ok(ToolOutput {
                returncode: 0,
                stdout: b"{\"message_type\":\"summary\"}".to_vec(),
                stderr: b"PASSWORD BACKEND UNRELATED".to_vec(),
            })
        }
    }

    struct RecordingFixture {
        calls: Cell<u8>,
        program: std::cell::RefCell<Option<OsString>>,
    }
    impl ToolRunner for RecordingFixture {
        fn run(&self, request: &ToolRequest) -> io::Result<ToolOutput> {
            self.calls.set(self.calls.get() + 1);
            self.program.replace(Some(request.program.clone()));
            Ok(ToolOutput {
                returncode: 0,
                stdout: vec![],
                stderr: vec![],
            })
        }
    }

    #[cfg(unix)]
    #[test]
    fn unix_spawn_passes_the_rclone_program_option_as_one_csv_field() {
        let argv: Vec<OsString> = [
            "-o",
            r#"rclone.program="/opt/solstone app/lib/solstone-rclone/rclone""#,
            "-o",
            "rclone.args=serve restic --stdio --append-only --config /dev/null",
            "backup",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        let spawned = spawn_argv(&argv);
        assert_eq!(
            spawned[1],
            OsString::from(r#""rclone.program=""/opt/solstone app/lib/solstone-rclone/rclone""""#)
        );
        assert_eq!(spawned[0], argv[0]);
        assert_eq!(spawned[2..], argv[2..]);
    }

    #[test]
    fn restic_csv_option_preserves_shell_quotes_and_commas() {
        assert_eq!(
            encode_restic_csv_option(r#"rclone.program="C:\Owner café, local\rclone.exe""#),
            r#""rclone.program=""C:\Owner café, local\rclone.exe""""#
        );
        assert_eq!(
            encode_restic_csv_option(r#"rclone.program="\\?\C:\Owner's path\rclone.exe""#),
            r#""rclone.program=""\\?\C:\Owner's path\rclone.exe""""#
        );
    }

    #[test]
    fn guards_forbidden_tokens_and_secret_substrings() {
        assert!(matches!(
            guard_argv(&["--insecure-tls".into()], &[]),
            Err(RunnerError::InsecureTls)
        ));
        assert!(matches!(
            guard_argv(&["prefix-secret-suffix".into()], &["secret".into()]),
            Err(RunnerError::SecretInArgv)
        ));
    }
    #[test]
    fn refuses_bare_program_before_invoking_runner() {
        let runner = RecordingFixture {
            calls: Cell::new(0),
            program: std::cell::RefCell::new(None),
        };
        let result = run_restic(
            &runner,
            &["snapshots".into()],
            "repo",
            "password",
            Path::new("restic"),
            None,
            false,
            None,
            None,
            &[],
        );
        assert!(matches!(result, Err(RunnerError::BareProgram)));
        assert_eq!(runner.calls.get(), 0);

        run_restic(
            &runner,
            &["snapshots".into()],
            "repo",
            "password",
            Path::new("/fixture/bin/restic"),
            None,
            false,
            None,
            None,
            &[],
        )
        .unwrap();
        assert_eq!(runner.calls.get(), 1);
        assert_eq!(runner.program.take(), Some("/fixture/bin/restic".into()));
    }
    #[test]
    fn maps_all_reference_return_codes() {
        assert_eq!(
            [3, 10, 11, 12, 124, 9].map(reason_for_returncode),
            [
                "incomplete",
                "repo_missing",
                "locked",
                "auth_failed",
                "timeout",
                "failed"
            ]
        );
    }
    #[test]
    fn runner_whitelists_and_scrubs_only_active_secrets() {
        let mut backend = BTreeMap::new();
        backend.insert("BACKEND".into(), Some("BACKEND".into()));
        let result = run_restic(
            &Fixture,
            &["snapshots".into()],
            "repo",
            "PASSWORD",
            Path::new("/fixture/bin/restic"),
            Some(&backend),
            true,
            None,
            None,
            &[],
        )
        .unwrap();
        assert_eq!(result.stderr, "[redacted] [redacted] UNRELATED");
        let (environment, _) = child_env("repo", "PASSWORD", Some(&backend));
        assert!(environment.contains_key(&OsString::from("RESTIC_REPOSITORY")));
        assert!(!environment.contains_key(&OsString::from("AWS_SECRET_ACCESS_KEY")));
        assert!(select_summary(result.json.as_ref().unwrap()).is_some());
    }

    #[test]
    fn hosted_public_settings_allow_paths_and_json_while_credentials_stay_guarded() {
        let mut backend = BTreeMap::from([
            ("RCLONE_CONFIG_SPB_TYPE".into(), Some("s3".into())),
            (
                "RCLONE_CONFIG_SPB_PROVIDER".into(),
                Some("Cloudflare".into()),
            ),
            ("RCLONE_CONFIG_SPB_ENV_AUTH".into(), Some("false".into())),
            ("RCLONE_CONFIG_SPB_REGION".into(), Some("auto".into())),
            (
                "RCLONE_CONFIG_SPB_NO_CHECK_BUCKET".into(),
                Some("true".into()),
            ),
            (
                "RCLONE_CONFIG_SPB_ACCESS_KEY_ID".into(),
                Some("ACCESS".into()),
            ),
            (
                "RCLONE_CONFIG_SPB_SECRET_ACCESS_KEY".into(),
                Some("SECRET".into()),
            ),
            (
                "RCLONE_CONFIG_SPB_SESSION_TOKEN".into(),
                Some("TOKEN".into()),
            ),
            ("UNKNOWN_BACKEND_KEY".into(), Some("UNKNOWN_SECRET".into())),
        ]);
        let (_, secrets) = child_env("repository", "PASSWORD", Some(&backend));
        guard_argv(
            &["rclone.program=/tools/s3-auto-Cloudflare-true-false/rclone".into()],
            &secrets,
        )
        .unwrap();
        let output = r#"{"s3":true,"auto":false,"provider":"Cloudflare"}"#;
        assert_eq!(scrub(output, &secrets), output);
        for credential in ["PASSWORD", "ACCESS", "SECRET", "TOKEN", "UNKNOWN_SECRET"] {
            assert!(matches!(
                guard_argv(&[format!("path/{credential}/tool")], &secrets),
                Err(RunnerError::SecretInArgv)
            ));
            assert!(!scrub(credential, &secrets).contains(credential));
        }
        backend.insert(
            "RCLONE_CONFIG_SPB_TYPE".into(),
            Some("unexpected-sensitive-value".into()),
        );
        backend.insert("RCLONE_CONFIG_SPB_SESSION_TOKEN".into(), Some("s3".into()));
        let (_, secrets) = child_env("repository", "PASSWORD", Some(&backend));
        for value in ["unexpected-sensitive-value", "s3"] {
            assert!(matches!(
                guard_argv(&[value.into()], &secrets),
                Err(RunnerError::SecretInArgv)
            ));
        }
    }
    #[test]
    fn parses_jsonl_and_last_summary() {
        let parsed = parse_json(
            "{\"message_type\":\"summary\",\"a\":1}\n{\"message_type\":\"summary\",\"a\":2}\n",
        )
        .unwrap();
        assert_eq!(
            select_summary(&parsed).unwrap().get("a"),
            Some(&Value::from(2))
        );
        assert_eq!(parse_json("{\nnot-json"), None);
    }
    #[test]
    fn debug_redacts_process_environment_and_raw_output() {
        let request = ToolRequest {
            program: "/fixture/bin/restic".into(),
            argv: vec!["snapshots".into()],
            env: BTreeMap::from([("RESTIC_PASSWORD".into(), "REQUEST_SECRET".into())]),
            timeout: None,
            pass_fds: vec![],
            stdin: Some(b"STDIN_SECRET\n".to_vec()),
            #[cfg(windows)]
            resources: Default::default(),
        };
        let output = ToolOutput {
            returncode: 1,
            stdout: b"OUTPUT_SECRET".to_vec(),
            stderr: b"ERROR_SECRET".to_vec(),
        };
        let restic_result = ResticResult {
            returncode: 0,
            stdout: "RESULT_STDOUT_SECRET".into(),
            stderr: "RESULT_STDERR_SECRET".into(),
            json: None,
            argv: vec!["snapshots".into()],
        };

        let rendered = format!("{request:?}\n{output:?}\n{restic_result:?}");
        for secret in [
            "REQUEST_SECRET",
            "STDIN_SECRET",
            "OUTPUT_SECRET",
            "ERROR_SECRET",
            "RESULT_STDOUT_SECRET",
            "RESULT_STDERR_SECRET",
        ] {
            assert!(!rendered.contains(secret));
        }
    }

    #[test]
    fn run_restic_scrubs_stdin_secret() {
        struct SecretEchoFixture;
        impl ToolRunner for SecretEchoFixture {
            fn run(&self, request: &ToolRequest) -> io::Result<ToolOutput> {
                assert_eq!(
                    request.stdin.as_deref(),
                    Some(b"RECOVERY_SECRET\n".as_slice())
                );
                assert_eq!(
                    request.env.get(std::ffi::OsStr::new("RESTIC_PASSWORD")),
                    Some(&OsString::from("PASSWORD"))
                );
                assert!(request.pass_fds.is_empty());
                assert!(!format!("{request:?}").contains("RECOVERY_SECRET"));
                Ok(ToolOutput {
                    returncode: 0,
                    stdout: b"hello RECOVERY_SECRET world".to_vec(),
                    stderr: b"RECOVERY_SECRET diagnostic PASSWORD".to_vec(),
                })
            }
        }
        let result = run_restic_with_stdin(
            &SecretEchoFixture,
            &["key".into(), "add".into()],
            "repo",
            "PASSWORD",
            Path::new("/fixture/bin/restic"),
            None,
            false,
            None,
            None,
            &[],
            Some(b"RECOVERY_SECRET\n".to_vec()),
        )
        .unwrap();
        assert_eq!(result.stdout, "hello [redacted] world");
        assert_eq!(result.stderr, "[redacted] diagnostic [redacted]");
        assert!(!format!("{result:?}").contains("RECOVERY_SECRET"));
        assert!(!format!("{result:?}").contains("PASSWORD"));
    }

    #[cfg(unix)]
    #[test]
    fn unix_system_tool_runner_rejects_stdin() {
        let runner = SystemToolRunner;
        let request = ToolRequest {
            program: "/fixture/bin/restic".into(),
            argv: vec!["snapshots".into()],
            env: BTreeMap::new(),
            timeout: None,
            pass_fds: vec![],
            stdin: Some(b"data".to_vec()),
        };
        let err = runner.run(&request).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }
}
