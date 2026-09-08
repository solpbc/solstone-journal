// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::Path;
use std::time::Duration;

#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::io::Read;
#[cfg(unix)]
use std::os::fd::{AsFd, OwnedFd};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(unix)]
use std::process::{Command, Stdio};
#[cfg(unix)]
use std::thread;
#[cfg(unix)]
use std::time::Instant;

#[cfg(unix)]
use nix::errno::Errno;
#[cfg(unix)]
use nix::fcntl::{FcntlArg, FdFlag, fcntl};
#[cfg(unix)]
use nix::sys::signal::{Signal, killpg};
#[cfg(unix)]
use nix::sys::wait::waitpid;
#[cfg(unix)]
use nix::unistd::{Pid, pipe};
use serde_json::Value;
#[cfg(windows)]
use solstone_core_distribution::windows_payload::{WINDOWS_RCLONE_WORKER, WINDOWS_RESTIC_WORKER};
#[cfg(unix)]
use solstone_core_system::process::{Disposition, LaunchAuthority, LaunchError, launch};
use thiserror::Error;

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
}
impl fmt::Debug for ToolRequest<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ToolRequest")
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
}

#[derive(Debug, Default)]
pub struct SystemToolRunner;

// restic's -o flag parses CSV before rclone splits the program shell string.
// Encode only after program admission, preserving that exact logical value.
#[cfg(any(windows, test))]
fn encode_restic_csv_option(option: &str) -> String {
    format!("\"{}\"", option.replace('"', "\"\""))
}

#[cfg(unix)]
impl ToolRunner for SystemToolRunner {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        if request.stdin.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "stdin not supported on unix tool runner",
            ));
        }
        let mut restored = Vec::with_capacity(request.pass_fds.len());
        for &fd in &request.pass_fds {
            let flags = fcntl(fd, FcntlArg::F_GETFD).map_err(io::Error::other)?;
            let flags = FdFlag::from_bits_truncate(flags);
            if flags.contains(FdFlag::FD_CLOEXEC) {
                fcntl(fd, FcntlArg::F_SETFD(flags & !FdFlag::FD_CLOEXEC))
                    .map_err(io::Error::other)?;
                restored.push((fd, flags));
            }
        }
        let result = self.run_child(request);
        for (fd, flags) in restored {
            let _ = fcntl(fd, FcntlArg::F_SETFD(flags));
        }
        result
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

        let (bin_dir, package_root) = crate::windows_tool::resolve_package_bin_and_root()?;
        let payload =
            solstone_core_distribution::windows_payload::verify_windows_payload(&package_root)
                .map_err(|error| {
                    io::Error::new(io::ErrorKind::PermissionDenied, error.to_string())
                })?;

        let missing_tool = || {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "backup tool is not declared in the admitted payload",
            )
        };
        let admitted_restic = payload
            .declared_path(WINDOWS_RESTIC_WORKER)
            .ok_or_else(missing_tool)?;
        let admitted_rclone = payload
            .declared_path(WINDOWS_RCLONE_WORKER)
            .ok_or_else(missing_tool)?;
        let canonical_program = Path::new(&request.program).canonicalize()?;
        let canonical_restic = admitted_restic.canonicalize()?;
        let canonical_rclone = admitted_rclone.canonicalize()?;
        if canonical_program != canonical_restic && canonical_program != canonical_rclone {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unadmitted backup executable",
            ));
        }
        let arguments = request
            .argv
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
        let uses_rclone = request
            .env
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

        let budget = solstone_core_system::process::BoundedHelperBudget {
            stdin_limit_bytes: 64 * 1024,
            stdout_limit_bytes: 64 * 1024 * 1024,
            stderr_limit_bytes: 16 * 1024 * 1024,
            timeout,
        };

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
            environment: request.env.clone(),
            stdin: request.stdin.clone().unwrap_or_default(),
            budget,
            resource_limits: None,
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
            Err(solstone_core_system::process::BoundedHelperError::DeadlineExceeded {
                quiescent: true,
                ..
            }) => Ok(ToolOutput {
                returncode: 124,
                stdout: Vec::new(),
                stderr: Vec::new(),
            }),
            Err(solstone_core_system::process::BoundedHelperError::DeadlineExceeded {
                quiescent: false,
                ..
            }) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "child process exceeded deadline and failed to quiesce",
            )),
            Err(solstone_core_system::process::BoundedHelperError::JobNotQuiescent { .. }) => {
                Err(io::Error::other("job object processes failed to terminate"))
            }
            Err(other) => Err(io::Error::other(other.to_string())),
        }
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

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Observe,
    GroupCleanup,
    CloseOwned,
    Reap,
    CollectReaders,
}

#[cfg(unix)]
trait SessionIo {
    fn observe_exit_without_reap(&mut self) -> io::Result<bool>;
    fn group_cleanup(&mut self) -> io::Result<()>;
    fn close_owned_endpoints(&mut self);
    fn reap_root(&mut self) -> io::Result<i32>;
    fn collect_readers(&mut self) -> io::Result<(Vec<u8>, Vec<u8>)>;
}

#[cfg(unix)]
fn push_step(trace: &mut Option<&mut Vec<Step>>, step: Step) {
    if let Some(trace) = trace {
        trace.push(step);
    }
}

#[cfg(unix)]
fn complete_session<S: SessionIo>(
    session: &mut S,
    mut trace: Option<&mut Vec<Step>>,
) -> io::Result<(i32, Vec<u8>, Vec<u8>)> {
    push_step(&mut trace, Step::Observe);
    session.observe_exit_without_reap()?;
    push_step(&mut trace, Step::GroupCleanup);
    let cleanup_err = session.group_cleanup().err();
    push_step(&mut trace, Step::CloseOwned);
    session.close_owned_endpoints();
    push_step(&mut trace, Step::Reap);
    let reaped = session.reap_root();
    if let Some(err) = cleanup_err {
        // If killpg failed with EPERM, descendants can still hold the
        // pipe write ends; joining would reintroduce the unbounded hang
        // this supervisor exists to remove.
        return Err(err);
    }
    let status = reaped?;
    push_step(&mut trace, Step::CollectReaders);
    let (stdout, stderr) = session.collect_readers()?;
    Ok((status, stdout, stderr))
}

#[cfg(unix)]
struct GroupGuard {
    pgid: Pid,
    reaped: bool,
}

#[cfg(unix)]
impl Drop for GroupGuard {
    fn drop(&mut self) {
        if !self.reaped {
            // Drop cannot return an error. Signal first, then reap.
            // This is the only path that best-effort swallows these
            // failures; returning paths never do.
            let _ = kill_group(self.pgid);
            let _ = waitpid(self.pgid, None);
        }
    }
}

#[cfg(unix)]
type OutputReader = thread::JoinHandle<io::Result<Vec<u8>>>;

#[cfg(unix)]
struct SpawnedChild {
    // Drop order is declaration order and is load-bearing: GroupGuard
    // (kill group), parent write ends (EOF), LaunchAuthority, then detached readers.
    guard: GroupGuard,
    stdout_w: Option<OwnedFd>,
    stderr_w: Option<OwnedFd>,
    authority: LaunchAuthority,
    readers: Option<(OutputReader, OutputReader)>,
}

#[cfg(unix)]
impl SessionIo for SpawnedChild {
    fn observe_exit_without_reap(&mut self) -> io::Result<bool> {
        observe_exit_without_reap(self.guard.pgid)
    }

    fn group_cleanup(&mut self) -> io::Result<()> {
        kill_group(self.guard.pgid)
    }

    fn close_owned_endpoints(&mut self) {
        drop(self.stdout_w.take());
        drop(self.stderr_w.take());
    }

    fn reap_root(&mut self) -> io::Result<i32> {
        self.guard.reaped = true;
        self.authority.wait()
    }

    fn collect_readers(&mut self) -> io::Result<(Vec<u8>, Vec<u8>)> {
        let (stdout_reader, stderr_reader) = self.readers.take().expect("readers");
        let stdout = stdout_reader
            .join()
            .map_err(|_| io::Error::other("stdout reader panicked"))??;
        let stderr = stderr_reader
            .join()
            .map_err(|_| io::Error::other("stderr reader panicked"))??;
        Ok((stdout, stderr))
    }
}

#[cfg(unix)]
fn kill_group(pgid: Pid) -> io::Result<()> {
    match killpg(pgid, Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        // Darwin reports EPERM for an empty process group whose leader has
        // already exited. Do not hide EPERM while that leader is still live.
        #[cfg(target_os = "macos")]
        Err(Errno::EPERM) if matches!(nix::unistd::getpgid(Some(pgid)), Err(Errno::ESRCH)) => {
            Ok(())
        }
        Err(err) => Err(io::Error::from(err)),
    }
}

#[cfg(unix)]
fn observe_exit_without_reap(pid: Pid) -> io::Result<bool> {
    let pid = rustix::process::Pid::from_raw(pid.as_raw())
        .ok_or_else(|| io::Error::other("invalid child pid"))?;
    rustix::process::waitid(
        rustix::process::WaitId::Pid(pid),
        rustix::process::WaitIdOptions::EXITED
            | rustix::process::WaitIdOptions::NOHANG
            | rustix::process::WaitIdOptions::NOWAIT,
    )
    .map(|status| status.is_some())
    .map_err(io::Error::from)
}

#[cfg(unix)]
fn set_cloexec(fd: PassedHandle<'_>) -> io::Result<()> {
    let flags = fcntl(fd, FcntlArg::F_GETFD).map_err(io::Error::from)?;
    let flags = FdFlag::from_bits_truncate(flags);
    fcntl(fd, FcntlArg::F_SETFD(flags | FdFlag::FD_CLOEXEC)).map_err(io::Error::from)?;
    Ok(())
}

#[cfg(unix)]
fn pipe_cloexec() -> io::Result<(OwnedFd, OwnedFd)> {
    let (read, write) = pipe().map_err(io::Error::from)?;
    set_cloexec(read.as_fd())?;
    set_cloexec(write.as_fd())?;
    Ok((read, write))
}

#[cfg(unix)]
impl SystemToolRunner {
    fn run_child(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        let (stdout_r, stdout_w) = pipe_cloexec()?;
        let (stderr_r, stderr_w) = pipe_cloexec()?;
        let mut command = Command::new(&request.program);
        command.args(&request.argv).env_clear().envs(&request.env);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout_w.try_clone()?))
            .stderr(Stdio::from(stderr_w.try_clone()?));
        command.process_group(0);
        let authority = launch(
            Disposition::IndependentBoundedHelper {
                timeout: request.timeout.unwrap_or(Duration::MAX),
            },
            move || command.spawn(),
            Box::new(|child, _timeout| {
                let Ok(raw) = i32::try_from(child.id()) else {
                    return child.kill().map_err(LaunchError::Terminate);
                };
                match killpg(Pid::from_raw(raw), Signal::SIGKILL) {
                    Ok(()) | Err(Errno::ESRCH) => Ok(()),
                    Err(err) => Err(LaunchError::Terminate(io::Error::from(err))),
                }
            }),
        )
        .map_err(|error| match error {
            LaunchError::Spawn(inner) => inner,
            other => io::Error::other(other),
        })?;
        // Command is moved into the spawn closure and dropped when launch()
        // finishes spawning, closing its Stdio write-end clones so
        // read_to_end sees EOF.
        // pids fit in i32 on every target this crate builds; cast so no
        // `?` can return between spawn and an armed GroupGuard.
        let pgid = Pid::from_raw(authority.pid() as i32);
        let mut session = SpawnedChild {
            guard: GroupGuard {
                pgid,
                reaped: false,
            },
            stdout_w: Some(stdout_w),
            stderr_w: Some(stderr_w),
            authority,
            readers: None,
        };
        let stdout_reader = thread::spawn(move || {
            let mut stdout = File::from(stdout_r);
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).map(|_| bytes)
        });
        let stderr_reader = thread::spawn(move || {
            let mut stderr = File::from(stderr_r);
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).map(|_| bytes)
        });
        session.readers = Some((stdout_reader, stderr_reader));
        let start = Instant::now();
        let timed_out = loop {
            if session.observe_exit_without_reap()? {
                break false;
            }
            if request
                .timeout
                .is_some_and(|timeout| start.elapsed() >= timeout)
            {
                break true;
            }
            thread::sleep(Duration::from_millis(5));
        };
        let (status, stdout, stderr) = complete_session(&mut session, None)?;
        Ok(ToolOutput {
            returncode: if timed_out {
                124
            } else if status >= 0 {
                status
            } else {
                1
            },
            stdout,
            stderr,
        })
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
pub fn run_restic(
    runner: &dyn ToolRunner,
    args: &[String],
    repository: &str,
    password: &str,
    restic_path: &Path,
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
pub(crate) fn run_restic_with_stdin(
    runner: &dyn ToolRunner,
    args: &[String],
    repository: &str,
    password: &str,
    restic_path: &Path,
    backend_env: Option<&BTreeMap<String, Option<String>>>,
    json: bool,
    max_repack_size: Option<&str>,
    timeout: Option<Duration>,
    pass_fds: &[PassedHandle<'_>],
    stdin: Option<Vec<u8>>,
) -> Result<ResticResult, RunnerError> {
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
    guard_argv(&argv, &secrets)?;
    let output = runner
        .run(&ToolRequest {
            program: restic_path.as_os_str().to_os_string(),
            argv: argv.iter().map(OsString::from).collect(),
            env,
            timeout,
            pass_fds: pass_fds.to_vec(),
            stdin,
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

    #[cfg(unix)]
    #[test]
    fn complete_session_records_observe_cleanup_close_reap_order() {
        struct Fake;
        impl SessionIo for Fake {
            fn observe_exit_without_reap(&mut self) -> io::Result<bool> {
                Ok(true)
            }
            fn group_cleanup(&mut self) -> io::Result<()> {
                Ok(())
            }
            fn close_owned_endpoints(&mut self) {}
            fn reap_root(&mut self) -> io::Result<i32> {
                Ok(0)
            }
            fn collect_readers(&mut self) -> io::Result<(Vec<u8>, Vec<u8>)> {
                Ok((Vec::new(), Vec::new()))
            }
        }
        let mut log = Vec::new();
        complete_session(&mut Fake, Some(&mut log)).unwrap();
        assert_eq!(
            log,
            [
                Step::Observe,
                Step::GroupCleanup,
                Step::CloseOwned,
                Step::Reap,
                Step::CollectReaders
            ]
        );
    }
}
