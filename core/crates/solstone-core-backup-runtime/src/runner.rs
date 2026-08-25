// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::fcntl::{FcntlArg, FdFlag, fcntl};
use nix::sys::signal::{Signal, killpg};
use nix::sys::wait::waitpid;
use nix::unistd::{Pid, pipe};
use serde_json::Value;
use solstone_core_system::process::{Disposition, LaunchAuthority, LaunchError, launch};
use thiserror::Error;

#[derive(Clone)]
pub struct ToolRequest<'a> {
    pub program: OsString,
    pub argv: Vec<OsString>,
    pub env: BTreeMap<OsString, OsString>,
    pub timeout: Option<Duration>,
    pub pass_fds: Vec<BorrowedFd<'a>>,
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

impl ToolRunner for SystemToolRunner {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        let mut restored = Vec::with_capacity(request.pass_fds.len());
        #[cfg(unix)]
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
        #[cfg(unix)]
        for (fd, flags) in restored {
            let _ = fcntl(fd, FcntlArg::F_SETFD(flags));
        }
        result
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Observe,
    GroupCleanup,
    CloseOwned,
    Reap,
    CollectReaders,
}

trait SessionIo {
    fn observe_exit_without_reap(&mut self) -> io::Result<bool>;
    fn group_cleanup(&mut self) -> io::Result<()>;
    fn close_owned_endpoints(&mut self);
    fn reap_root(&mut self) -> io::Result<i32>;
    fn collect_readers(&mut self) -> io::Result<(Vec<u8>, Vec<u8>)>;
}

fn push_step(trace: &mut Option<&mut Vec<Step>>, step: Step) {
    if let Some(trace) = trace {
        trace.push(step);
    }
}

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

struct GroupGuard {
    pgid: Pid,
    reaped: bool,
}

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

type OutputReader = thread::JoinHandle<io::Result<Vec<u8>>>;

struct SpawnedChild {
    // Drop order is declaration order and is load-bearing: GroupGuard
    // (kill group), parent write ends (EOF), LaunchAuthority, then detached readers.
    guard: GroupGuard,
    stdout_w: Option<OwnedFd>,
    stderr_w: Option<OwnedFd>,
    authority: LaunchAuthority,
    readers: Option<(OutputReader, OutputReader)>,
}

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

fn set_cloexec(fd: BorrowedFd<'_>) -> io::Result<()> {
    let flags = fcntl(fd, FcntlArg::F_GETFD).map_err(io::Error::from)?;
    let flags = FdFlag::from_bits_truncate(flags);
    fcntl(fd, FcntlArg::F_SETFD(flags | FdFlag::FD_CLOEXEC)).map_err(io::Error::from)?;
    Ok(())
}

fn pipe_cloexec() -> io::Result<(OwnedFd, OwnedFd)> {
    let (read, write) = pipe().map_err(io::Error::from)?;
    set_cloexec(read.as_fd())?;
    set_cloexec(write.as_fd())?;
    Ok((read, write))
}

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

#[derive(Clone, Debug, PartialEq)]
pub struct ResticResult {
    pub returncode: i32,
    pub stdout: String,
    pub stderr: String,
    pub json: Option<Value>,
    pub argv: Vec<String>,
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
    pass_fds: &[BorrowedFd<'_>],
) -> Result<ResticResult, RunnerError> {
    if !is_explicit_program_path(restic_path) {
        return Err(RunnerError::BareProgram);
    }
    let (env, secrets) = child_env(repository, password, backend_env);
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
    for key in ["PATH", "HOME", "TMPDIR"] {
        if let Some(value) = env::var_os(key) {
            env.insert(key.into(), value);
        }
    }
    env.insert("RESTIC_REPOSITORY".into(), repository.into());
    env.insert("RESTIC_PASSWORD".into(), password.into());
    let mut secrets = vec![password.to_owned()];
    if let Some(backend_env) = backend_env {
        for (key, value) in backend_env {
            if let Some(value) = value {
                env.insert(key.into(), value.into());
                if !value.is_empty() {
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
        };
        let output = ToolOutput {
            returncode: 1,
            stdout: b"OUTPUT_SECRET".to_vec(),
            stderr: b"ERROR_SECRET".to_vec(),
        };

        let rendered = format!("{request:?}\n{output:?}");
        for secret in ["REQUEST_SECRET", "OUTPUT_SECRET", "ERROR_SECRET"] {
            assert!(!rendered.contains(secret));
        }
    }

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
