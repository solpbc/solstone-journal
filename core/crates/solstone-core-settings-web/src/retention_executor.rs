// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded client for the native retention sibling binary.

use std::{
    env,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;

const BINARY: &str = "solstone-retention";
const OVERRIDE: &str = "SOLSTONE_RETENTION_BIN";
const TIMEOUT: Duration = Duration::from_secs(60);
const STDOUT_LIMIT: usize = 1024 * 1024;
const STDERR_LIMIT: usize = 64 * 1024;

#[derive(Debug)]
pub(crate) enum ExecutorError {
    Unavailable(String),
    Refused(Refused),
}

impl std::fmt::Display for ExecutorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(reason) => formatter.write_str(reason),
            Self::Refused(refused) => formatter.write_str(&refused.summary()),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Refused(pub(crate) Value);

impl Refused {
    pub(crate) fn summary(&self) -> String {
        let entries = self
            .0
            .get("outcome")
            .and_then(|value| value.get("targets"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .flat_map(|target| {
                target
                    .get("not_removed")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .map(|entry| {
                format!(
                    "{}: {}",
                    entry.get("entry").and_then(Value::as_str).unwrap_or("?"),
                    entry.get("reason").and_then(Value::as_str).unwrap_or("?")
                )
            })
            .collect::<Vec<_>>();
        if entries.is_empty() {
            "the retention tool refused without naming an entry.".to_owned()
        } else {
            entries.join("; ")
        }
    }
}

pub(crate) async fn marks(journal: PathBuf) -> Result<Value, ExecutorError> {
    invoke(vec![
        "marks".to_owned(),
        "--journal".to_owned(),
        journal.display().to_string(),
    ])
    .await
}

pub(crate) async fn mark(
    journal: PathBuf,
    today: String,
    now: String,
    policy: String,
) -> Result<Value, ExecutorError> {
    invoke(vec![
        "mark".to_owned(),
        "--journal".to_owned(),
        journal.display().to_string(),
        "--today".to_owned(),
        today,
        "--now".to_owned(),
        now,
        "--policy".to_owned(),
        policy,
    ])
    .await
}

pub(crate) async fn prune_logs(
    journal: PathBuf,
    today: String,
    days: String,
    dry_run: bool,
) -> Result<Value, ExecutorError> {
    let mut args = vec![
        "prune-logs".to_owned(),
        "--journal".to_owned(),
        journal.display().to_string(),
        "--today".to_owned(),
        today,
        "--days".to_owned(),
        days,
    ];
    if !dry_run {
        args.extend(["--execute".to_owned(), "true".to_owned()]);
    }
    invoke(args).await
}

async fn invoke(args: Vec<String>) -> Result<Value, ExecutorError> {
    tokio::task::spawn_blocking(move || run(args))
        .await
        .map_err(|_| {
            ExecutorError::Unavailable(
                "the retention tool could not run: task join failed.".to_owned(),
            )
        })?
}

fn executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn binary() -> Result<PathBuf, ExecutorError> {
    if let Ok(override_path) = env::var(OVERRIDE)
        && !override_path.is_empty()
    {
        let path = PathBuf::from(&override_path);
        if !executable(&path) {
            return Err(ExecutorError::Unavailable(format!(
                "{OVERRIDE} points at {override_path}, which is not an executable file"
            )));
        }
        return Ok(path);
    }
    for directory in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
        let path = directory.join(BINARY);
        if executable(&path) {
            return Ok(path);
        }
    }
    Err(ExecutorError::Unavailable(format!(
        "{BINARY} is not on PATH. Every removal of the owner's media goes through it, so nothing is deleted without it. Install the core binaries, or set {OVERRIDE} to an absolute path."
    )))
}

fn drain<R: Read + Send + 'static>(
    mut reader: R,
    limit: usize,
    sender: mpsc::Sender<String>,
    name: &'static str,
) -> thread::JoinHandle<Result<Vec<u8>, String>> {
    thread::spawn(move || {
        let mut output = Vec::new();
        let mut buffer = [0_u8; 8192];
        loop {
            let count = reader
                .read(&mut buffer)
                .map_err(|error| error.to_string())?;
            if count == 0 {
                return Ok(output);
            }
            if output.len() + count > limit {
                let error = format!("{name}-too-large");
                let _ = sender.send(error.clone());
                return Err(error);
            }
            output.extend_from_slice(&buffer[..count]);
        }
    })
}

fn run(args: Vec<String>) -> Result<Value, ExecutorError> {
    // This is PATH-based, unlike the speakers helper, because it matches the Python reference.
    let path = binary()?;
    let mut child = Command::new(path)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            ExecutorError::Unavailable(format!("the retention tool could not run: {error}."))
        })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        ExecutorError::Unavailable(
            "the retention tool could not run: stdout unavailable.".to_owned(),
        )
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        ExecutorError::Unavailable(
            "the retention tool could not run: stderr unavailable.".to_owned(),
        )
    })?;
    let (sender, receiver) = mpsc::channel();
    let stdout_reader = drain(stdout, STDOUT_LIMIT, sender.clone(), "stdout");
    let stderr_reader = drain(stderr, STDERR_LIMIT, sender, "stderr");
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if receiver.try_recv().is_ok() || Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(ExecutorError::Unavailable(if Instant::now() >= deadline {
                "the retention tool did not finish within 60s.".to_owned()
            } else {
                "the retention tool produced too much output.".to_owned()
            }));
        }
        if let Some(status) = child.try_wait().map_err(|error| {
            ExecutorError::Unavailable(format!("the retention tool could not run: {error}."))
        })? {
            let stdout = stdout_reader
                .join()
                .map_err(|_| {
                    ExecutorError::Unavailable(
                        "the retention tool could not run: stdout reader failed.".to_owned(),
                    )
                })?
                .map_err(ExecutorError::Unavailable)?;
            let stderr = stderr_reader
                .join()
                .map_err(|_| {
                    ExecutorError::Unavailable(
                        "the retention tool could not run: stderr reader failed.".to_owned(),
                    )
                })?
                .map_err(ExecutorError::Unavailable)?;
            let receipt: Value = serde_json::from_slice(&stdout).map_err(|_| {
                ExecutorError::Unavailable(format!(
                    "the retention tool produced no readable receipt (exit {}): {}.",
                    status.code().unwrap_or(-1),
                    String::from_utf8_lossy(&stderr)
                        .trim()
                        .if_empty("<no stderr>")
                ))
            })?;
            if !receipt.is_object() {
                return Err(ExecutorError::Unavailable(
                    "the retention tool's receipt was not an object.".to_owned(),
                ));
            }
            return match status.code() {
                Some(0) => Ok(receipt),
                Some(3 | 4) => Err(ExecutorError::Refused(Refused(receipt))),
                code => Err(ExecutorError::Unavailable(format!(
                    "the retention tool was rejected (exit {}): {}",
                    code.unwrap_or(-1),
                    receipt.get("error").unwrap_or(&receipt)
                ))),
            };
        }
        thread::sleep(Duration::from_millis(10));
    }
}

trait Empty {
    fn if_empty(self, fallback: &str) -> String;
}
impl Empty for &str {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.to_owned()
        } else {
            self.to_owned()
        }
    }
}
