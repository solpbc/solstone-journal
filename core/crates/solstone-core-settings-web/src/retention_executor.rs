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
use solstone_core_convey_http::window_catalog::{strings, text};

pub(crate) const BINARY: &str = "solstone-retention";
const OVERRIDE: &str = "SOLSTONE_RETENTION_BIN";
const TIMEOUT: Duration = Duration::from_secs(60);
const STDOUT_LIMIT: usize = 1024 * 1024;
const STDERR_LIMIT: usize = 64 * 1024;

#[derive(Debug)]
pub(crate) enum ExecutorError {
    Unavailable(String),
    Refused(Refused),
}

impl ExecutorError {
    pub(crate) fn detail(self, locale: &str) -> String {
        match self {
            Self::Unavailable(reason) => reason,
            Self::Refused(refused) => refused.summary(locale),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Refused(pub(crate) Value);

impl Refused {
    fn summary(&self, locale: &str) -> String {
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
            text(
                locale,
                "journal.window.settings.retention.executor.refused.without.entry",
            )
        } else {
            entries.join("; ")
        }
    }
}

pub(crate) async fn marks(locale: &'static str, journal: PathBuf) -> Result<Value, ExecutorError> {
    invoke(
        locale,
        vec![
            "marks".to_owned(),
            "--journal".to_owned(),
            journal.display().to_string(),
        ],
    )
    .await
}

pub(crate) async fn mark(
    locale: &'static str,
    journal: PathBuf,
    today: String,
    now: String,
    policy: String,
) -> Result<Value, ExecutorError> {
    invoke(
        locale,
        vec![
            "mark".to_owned(),
            "--journal".to_owned(),
            journal.display().to_string(),
            "--today".to_owned(),
            today,
            "--now".to_owned(),
            now,
            "--policy".to_owned(),
            policy,
        ],
    )
    .await
}

pub(crate) async fn prune_logs(
    locale: &'static str,
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
    invoke(locale, args).await
}

async fn invoke(locale: &'static str, args: Vec<String>) -> Result<Value, ExecutorError> {
    tokio::task::spawn_blocking(move || run(locale, args))
        .await
        .map_err(|_| {
            ExecutorError::Unavailable(text(
                locale,
                "journal.window.settings.retention.executor.join.failed",
            ))
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

pub(crate) fn binary(locale: &str) -> Result<PathBuf, ExecutorError> {
    if let Ok(override_path) = env::var(OVERRIDE)
        && !override_path.is_empty()
    {
        let path = PathBuf::from(&override_path);
        if !executable(&path) {
            return Err(ExecutorError::Unavailable(strings(
                locale,
                "journal.window.settings.retention.executor.override",
                [("variable", OVERRIDE), ("path", &override_path)],
            )));
        }
        return Ok(path);
    }
    // The tool ships beside the running executable; on Windows PATH is not
    // known to include that directory, so look there first.
    #[cfg(windows)]
    if let Some(path) = env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.parent()
                .map(|dir| dir.join(format!("{BINARY}{}", env::consts::EXE_SUFFIX)))
        })
        .filter(|path| executable(path))
    {
        return Ok(path);
    }
    for directory in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
        let path = directory.join(format!("{BINARY}{}", env::consts::EXE_SUFFIX));
        if executable(&path) {
            return Ok(path);
        }
    }
    Err(ExecutorError::Unavailable(strings(
        locale,
        "journal.window.settings.retention.executor.path.missing",
        [("binary", BINARY), ("variable", OVERRIDE)],
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

fn run(locale: &str, args: Vec<String>) -> Result<Value, ExecutorError> {
    // This is PATH-based, unlike the speakers helper, because it matches the Python reference.
    let path = binary(locale)?;
    let mut child = Command::new(path)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            ExecutorError::Unavailable(strings(
                locale,
                "journal.window.settings.retention.executor.could.not.run",
                [("detail", &error.to_string())],
            ))
        })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        ExecutorError::Unavailable(text(
            locale,
            "journal.window.settings.retention.executor.stdout.unavailable",
        ))
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        ExecutorError::Unavailable(text(
            locale,
            "journal.window.settings.retention.executor.stderr.unavailable",
        ))
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
                text(locale, "journal.window.settings.retention.executor.timeout")
            } else {
                text(
                    locale,
                    "journal.window.settings.retention.executor.excess.output",
                )
            }));
        }
        if let Some(status) = child.try_wait().map_err(|error| {
            ExecutorError::Unavailable(strings(
                locale,
                "journal.window.settings.retention.executor.could.not.run",
                [("detail", &error.to_string())],
            ))
        })? {
            let stdout = stdout_reader
                .join()
                .map_err(|_| {
                    ExecutorError::Unavailable(text(
                        locale,
                        "journal.window.settings.retention.executor.stdout.failed",
                    ))
                })?
                .map_err(ExecutorError::Unavailable)?;
            let stderr = stderr_reader
                .join()
                .map_err(|_| {
                    ExecutorError::Unavailable(text(
                        locale,
                        "journal.window.settings.retention.executor.stderr.failed",
                    ))
                })?
                .map_err(ExecutorError::Unavailable)?;
            let receipt: Value = serde_json::from_slice(&stdout).map_err(|_| {
                ExecutorError::Unavailable(strings(
                    locale,
                    "journal.window.settings.retention.executor.receipt.unreadable",
                    [
                        ("code", &status.code().unwrap_or(-1).to_string()),
                        (
                            "detail",
                            &String::from_utf8_lossy(&stderr).trim().if_empty(&text(
                                locale,
                                "journal.window.settings.retention.executor.no.stderr",
                            )),
                        ),
                    ],
                ))
            })?;
            if !receipt.is_object() {
                return Err(ExecutorError::Unavailable(text(
                    locale,
                    "journal.window.settings.retention.executor.receipt.not.object",
                )));
            }
            return match status.code() {
                Some(0) => Ok(receipt),
                Some(3 | 4) => Err(ExecutorError::Refused(Refused(receipt))),
                code => Err(ExecutorError::Unavailable(strings(
                    locale,
                    "journal.window.settings.retention.executor.receipt.rejected",
                    [
                        ("code", &code.unwrap_or(-1).to_string()),
                        (
                            "receipt",
                            &receipt.get("error").unwrap_or(&receipt).to_string(),
                        ),
                    ],
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
