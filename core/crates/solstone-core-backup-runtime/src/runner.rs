// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read};
use std::os::fd::BorrowedFd;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use nix::fcntl::{FcntlArg, FdFlag, fcntl};
use serde_json::Value;
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

impl SystemToolRunner {
    fn run_child(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        let mut command = Command::new(&request.program);
        command.args(&request.argv).env_clear().envs(&request.env);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let mut stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let stdout_reader = thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).map(|_| bytes)
        });
        let stderr_reader = thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).map(|_| bytes)
        });
        let start = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break (status, false);
            }
            if request
                .timeout
                .is_some_and(|timeout| start.elapsed() >= timeout)
            {
                child.kill()?;
                break (child.wait()?, true);
            }
            thread::sleep(Duration::from_millis(5));
        };
        let stdout = stdout_reader
            .join()
            .map_err(|_| io::Error::other("stdout reader panicked"))??;
        let stderr = stderr_reader
            .join()
            .map_err(|_| io::Error::other("stderr reader panicked"))??;
        Ok(ToolOutput {
            returncode: if status.1 {
                124
            } else {
                status.0.code().unwrap_or(1)
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
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

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
            Path::new("restic"),
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
            program: "restic".into(),
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
    #[cfg(unix)]
    #[test]
    fn real_fixture_process_observes_whitelisted_environment() {
        let directory = tempfile::tempdir().unwrap();
        let fixture = directory.path().join("fixture");
        fs::write(
            &fixture,
            "#!/bin/sh\nprintf '%s' \"$RESTIC_REPOSITORY:$RESTIC_PASSWORD:$LEAK\"\n",
        )
        .unwrap();
        fs::set_permissions(&fixture, fs::Permissions::from_mode(0o755)).unwrap();
        let result = run_restic(
            &SystemToolRunner,
            &[],
            "repo",
            "password",
            &fixture,
            None,
            false,
            None,
            Some(Duration::from_secs(1)),
            &[],
        )
        .unwrap();
        assert_eq!(result.stdout, "repo:[redacted]:");
    }

    #[cfg(unix)]
    #[test]
    fn timeout_scrubs_partial_output_and_passes_live_key_fd() {
        use std::io::Write;
        use std::os::fd::{AsFd, AsRawFd};
        let directory = tempfile::tempdir().unwrap();
        let fixture = directory.path().join("fixture");
        fs::write(
            &fixture,
            "#!/bin/sh\ncat /dev/fd/$1\nprintf ' PASSWORD' >&2\nsleep 1\n",
        )
        .unwrap();
        fs::set_permissions(&fixture, fs::Permissions::from_mode(0o755)).unwrap();
        let (reader, writer) = nix::unistd::pipe().unwrap();
        let mut writer = std::fs::File::from(writer);
        writer.write_all(b"PIPE_KEY").unwrap();
        drop(writer);
        let fd = reader.as_raw_fd();
        let result = run_restic(
            &SystemToolRunner,
            &[fd.to_string()],
            "repo",
            "PASSWORD",
            &fixture,
            None,
            true,
            None,
            Some(Duration::from_millis(200)),
            &[reader.as_fd()],
        )
        .unwrap();
        assert_eq!(result.returncode, 124);
        assert!(result.stdout.contains("PIPE_KEY"));
        assert_eq!(result.stderr, " [redacted]");
        assert_eq!(result.json, None);
    }
}
