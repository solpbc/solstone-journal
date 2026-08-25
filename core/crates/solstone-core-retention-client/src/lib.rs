// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded invocations of the native retention executor.
//!
//! The client caps named removal requests before it starts the executor. Its
//! timeout is a lock-contention bound, not proof that a permitted removal can
//! complete in that time. Its receipt cap covers
//! [`ASSUMED_MAX_NAMES_PER_MARK`] realistic paths per mark, but that assumption
//! is not enforced on persisted proposals. A proposal can contain an unbounded
//! list of names, so one legal in-cap mark can still exceed the output cap. That
//! case is reported as [`ClientError::OutcomeUnknown`], never as a run that did
//! not start.

use std::env;
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

pub use solstone_core_retention::Target;
pub use solstone_core_retention::layout::stream_rel;
pub use solstone_core_retention::marks::Mark;
pub use solstone_core_retention::marks::MarkState;
pub use solstone_core_retention::marks::Proposal;
pub use solstone_core_retention::marks::RemovalClass;
pub use solstone_core_retention::policy::Policy;
pub use solstone_core_retention::policy::policy_from_retention;
pub use solstone_core_retention::policy::policy_would_release;
pub use solstone_core_retention::summary::human_bytes;

const BINARY: &str = "solstone-retention";
const OVERRIDE: &str = "SOLSTONE_RETENTION_BIN";
const LOCK_WAIT_SECONDS: u64 = 10;
const PER_TARGET_OPERATION_ALLOWANCE_SECONDS: u64 = 5;
const STDERR_BYTES: usize = 64 * 1024;
const FIXED_RECEIPT_BYTES: usize = 256;

/// Maximum mark IDs accepted for one `remove-marked` invocation.
pub const MAX_REMOVE_MARK_IDS: usize = 32;

/// Headroom for a mark's released names: 256 times the fixture maximum of two.
///
/// This is deliberately an assumption rather than register validation. A mark
/// with more names remains valid data; its receipt can exceed the cap and is
/// reported as [`ClientError::OutcomeUnknown`].
const ASSUMED_MAX_NAMES_PER_MARK: usize = 512;

/// JSON bytes outside the `removed` paths in the measured target row.
const FIXED_TARGET_OUTCOME_BYTES: usize = 266;
/// JSON bytes for the longest observed releasable path, including its quotes.
const SERIALIZED_REMOVED_PATH_BYTES: usize = 51;
/// JSON bytes between adjacent paths in `removed`.
const REMOVED_PATH_SEPARATOR_BYTES: usize = 1;

/// Measured serialized size of one assumed-capacity target row, pinned by a test.
const PER_TARGET_BYTES: usize = FIXED_TARGET_OUTCOME_BYTES
    .saturating_add(ASSUMED_MAX_NAMES_PER_MARK.saturating_mul(SERIALIZED_REMOVED_PATH_BYTES))
    .saturating_add(
        ASSUMED_MAX_NAMES_PER_MARK
            .saturating_sub(1)
            .saturating_mul(REMOVED_PATH_SEPARATOR_BYTES),
    );

/// A parsed refusal receipt returned by the executor.
#[derive(Clone, Debug)]
pub struct Refused {
    receipt: Value,
    summary: String,
}

impl Refused {
    fn receipt(receipt: Value) -> Self {
        let summary = refusal_summary(&receipt);
        Self { receipt, summary }
    }

    /// The refusal receipt.
    pub fn receipt_value(&self) -> &Value {
        &self.receipt
    }

    /// Owner-readable detail when the receipt named a refusal reason.
    pub fn summary(&self) -> &str {
        &self.summary
    }
}

/// What prevented a client invocation from returning a successful receipt.
///
/// [`Self::BinaryUnavailable`] and [`Self::RequestTooLarge`] mean nothing ran.
/// [`Self::OutcomeUnknown`] means the executor may have run without a
/// trustworthy outcome. [`Self::Refused`] means it ran and returned a readable
/// refusal receipt.
#[derive(Debug)]
pub enum ClientError {
    /// The executor could not be resolved, so it did not run.
    BinaryUnavailable(String),
    /// The request exceeded the local cap, so the executor did not run.
    RequestTooLarge(String),
    /// The executor may have run, but no trustworthy outcome is available.
    OutcomeUnknown(String),
    /// The executor ran and returned a refusal receipt.
    Refused(Refused),
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BinaryUnavailable(reason)
            | Self::RequestTooLarge(reason)
            | Self::OutcomeUnknown(reason) => formatter.write_str(reason),
            Self::Refused(refused) => formatter.write_str(refused.summary()),
        }
    }
}

impl std::error::Error for ClientError {}

/// List the current removal register.
pub fn marks(journal: PathBuf) -> Result<Value, ClientError> {
    run(vec![
        "marks".to_owned(),
        "--journal".to_owned(),
        journal.display().to_string(),
    ])
}

/// Execute named, already-approved marks with a caller-supplied policy and instant.
pub fn remove_marked(
    journal: PathBuf,
    today: String,
    now: String,
    policy: String,
    mark_ids: &[String],
) -> Result<Value, ClientError> {
    if mark_ids.len() > MAX_REMOVE_MARK_IDS {
        return Err(ClientError::RequestTooLarge(format!(
            "at most {MAX_REMOVE_MARK_IDS} mark IDs may be removed at once"
        )));
    }
    let mut args = vec![
        "remove-marked".to_owned(),
        "--journal".to_owned(),
        journal.display().to_string(),
        "--today".to_owned(),
        today,
        "--now".to_owned(),
        now,
        "--policy".to_owned(),
        policy,
    ];
    for id in mark_ids {
        args.extend(["--mark".to_owned(), id.clone()]);
    }
    run(args)
}

/// Drop one pending approval without starting a removal run.
pub fn decline(journal: PathBuf, mark_id: String) -> Result<Value, ClientError> {
    run(vec![
        "decline".to_owned(),
        "--journal".to_owned(),
        journal.display().to_string(),
        "--mark".to_owned(),
        mark_id,
    ])
}

/// Finish staged deletions a previous run left behind.
///
/// Recover is journal-wide, not keyed on a mark. `stdout_limit()` is sized from
/// [`MAX_REMOVE_MARK_IDS`], so a journal with many staged directories can overflow
/// it and surface as [`ClientError::OutcomeUnknown`] — that is honest, not a bug.
pub fn recover(journal: PathBuf, at: String) -> Result<Value, ClientError> {
    run(vec![
        "recover".to_owned(),
        "--journal".to_owned(),
        journal.display().to_string(),
        "--at".to_owned(),
        at,
        "--did".to_owned(),
        "owner".to_owned(),
        "--reason".to_owned(),
        "owner".to_owned(),
    ])
}

fn child_timeout() -> Duration {
    let per_target = LOCK_WAIT_SECONDS.saturating_add(PER_TARGET_OPERATION_ALLOWANCE_SECONDS);
    Duration::from_secs(per_target.saturating_mul(MAX_REMOVE_MARK_IDS as u64))
}

fn stdout_limit() -> usize {
    MAX_REMOVE_MARK_IDS
        .saturating_mul(PER_TARGET_BYTES)
        .saturating_add(FIXED_RECEIPT_BYTES)
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

fn binary() -> Result<PathBuf, ClientError> {
    resolve_binary(env::var_os(OVERRIDE), env::var_os("PATH"))
}

fn resolve_binary(
    override_path: Option<std::ffi::OsString>,
    path: Option<std::ffi::OsString>,
) -> Result<PathBuf, ClientError> {
    if let Some(override_path) = override_path.filter(|value| !value.is_empty()) {
        let path = PathBuf::from(&override_path);
        if !executable(&path) {
            return Err(ClientError::BinaryUnavailable(format!(
                "{OVERRIDE} points at {}, which is not an executable file",
                path.display()
            )));
        }
        return Ok(path);
    }
    for directory in env::split_paths(&path.unwrap_or_default()) {
        let path = directory.join(BINARY);
        if executable(&path) {
            return Ok(path);
        }
    }
    Err(ClientError::BinaryUnavailable(format!(
        "{BINARY} is not on PATH; set {OVERRIDE} to an executable file"
    )))
}

fn drain<R: Read + Send + 'static>(
    mut reader: R,
    limit: usize,
    sender: mpsc::Sender<()>,
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
            if output.len().saturating_add(count) > limit {
                match sender.send(()) {
                    Ok(()) | Err(_) => {}
                }
                return Err("output exceeded its cap".to_owned());
            }
            output.extend_from_slice(&buffer[..count]);
        }
    })
}

fn stop(
    child: &mut Child,
    stdout_reader: thread::JoinHandle<Result<Vec<u8>, String>>,
    stderr_reader: thread::JoinHandle<Result<Vec<u8>, String>>,
) {
    match child.kill() {
        Ok(()) | Err(_) => {}
    }
    match child.wait() {
        Ok(_) | Err(_) => {}
    }
    match stdout_reader.join() {
        Ok(_) | Err(_) => {}
    }
    match stderr_reader.join() {
        Ok(_) | Err(_) => {}
    }
}

fn terminate(child: &mut Child) {
    match child.kill() {
        Ok(()) | Err(_) => {}
    }
    match child.wait() {
        Ok(_) | Err(_) => {}
    }
}

fn run(args: Vec<String>) -> Result<Value, ClientError> {
    let path = binary()?;
    let mut child = Command::new(path)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            ClientError::OutcomeUnknown(format!("the retention tool could not start: {error}"))
        })?;
    let Some(stdout) = child.stdout.take() else {
        terminate(&mut child);
        return Err(ClientError::OutcomeUnknown(
            "the retention tool started without stdout".to_owned(),
        ));
    };
    let Some(stderr) = child.stderr.take() else {
        terminate(&mut child);
        return Err(ClientError::OutcomeUnknown(
            "the retention tool started without stderr".to_owned(),
        ));
    };
    let (sender, receiver) = mpsc::channel();
    let stdout_reader = drain(stdout, stdout_limit(), sender.clone());
    let stderr_reader = drain(stderr, STDERR_BYTES, sender);
    let deadline = Instant::now() + child_timeout();
    loop {
        if receiver.try_recv().is_ok() {
            stop(&mut child, stdout_reader, stderr_reader);
            return Err(ClientError::OutcomeUnknown(
                "the retention tool produced too much output".to_owned(),
            ));
        }
        if Instant::now() >= deadline {
            stop(&mut child, stdout_reader, stderr_reader);
            return Err(ClientError::OutcomeUnknown(format!(
                "the retention tool did not finish within {}s",
                child_timeout().as_secs()
            )));
        }
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(error) => {
                stop(&mut child, stdout_reader, stderr_reader);
                return Err(ClientError::OutcomeUnknown(format!(
                    "the retention tool could not be observed: {error}"
                )));
            }
        };
        let Some(status) = status else {
            thread::sleep(Duration::from_millis(10));
            continue;
        };
        let stdout = stdout_reader.join().map_err(|_| {
            ClientError::OutcomeUnknown("the stdout reader did not finish".to_owned())
        })?;
        let stderr = stderr_reader.join().map_err(|_| {
            ClientError::OutcomeUnknown("the stderr reader did not finish".to_owned())
        })?;
        let stdout = stdout.map_err(ClientError::OutcomeUnknown)?;
        let stderr = stderr.map_err(ClientError::OutcomeUnknown)?;
        let receipt = parse_receipt(&stdout)?;
        return match status.code() {
            Some(0) => Ok(receipt),
            Some(3 | 4) => Err(ClientError::Refused(Refused::receipt(receipt))),
            code => Err(ClientError::OutcomeUnknown(format!(
                "the retention tool exited {}: {}",
                code.unwrap_or(-1),
                String::from_utf8_lossy(&stderr).trim()
            ))),
        };
    }
}

fn parse_receipt(bytes: &[u8]) -> Result<Value, ClientError> {
    let receipt: Value = serde_json::from_slice(bytes).map_err(|error| {
        ClientError::OutcomeUnknown(format!(
            "the retention tool produced no readable receipt: {error}"
        ))
    })?;
    if receipt.is_object() {
        Ok(receipt)
    } else {
        Err(ClientError::OutcomeUnknown(
            "the retention tool receipt was not an object".to_owned(),
        ))
    }
}

fn refusal_summary(receipt: &Value) -> String {
    let entries = receipt
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
        receipt
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("the retention tool refused without naming an entry.")
            .to_owned()
    } else {
        entries.join("; ")
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    reason = "test setup and assertions use concise infallible helpers"
)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use solstone_core_retention::receipt::{NotRemoved, TargetOutcome};

    use super::*;

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn temp_path(name: &str) -> PathBuf {
        PathBuf::from("/var/tmp").join(format!(
            "retention-client-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn resolution_uses_an_executable_override_before_path() {
        let directory = temp_path("override");
        fs::create_dir(&directory).unwrap();
        let binary = directory.join(BINARY);
        fs::write(&binary, b"#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let resolved = resolve_binary(Some(binary.clone().into_os_string()), None).unwrap();
        assert_eq!(resolved, binary);
        match fs::remove_dir_all(directory) {
            Ok(()) | Err(_) => {}
        }
    }

    #[test]
    fn resolution_refuses_a_non_executable_override() {
        let error =
            resolve_binary(Some(PathBuf::from("missing").into_os_string()), None).unwrap_err();
        assert!(matches!(error, ClientError::BinaryUnavailable(_)));
    }

    #[test]
    fn error_partition_is_exhaustive() {
        fn nothing_ran(error: ClientError) -> bool {
            match error {
                ClientError::BinaryUnavailable(_) => true,
                ClientError::RequestTooLarge(_) => true,
                ClientError::OutcomeUnknown(_) => false,
                ClientError::Refused(_) => false,
            }
        }

        assert!(nothing_ran(ClientError::BinaryUnavailable(
            "missing".to_owned()
        )));
        assert!(nothing_ran(ClientError::RequestTooLarge(
            "too many marks".to_owned()
        )));
        assert!(!nothing_ran(ClientError::OutcomeUnknown(
            "unknown".to_owned()
        )));
        assert!(!nothing_ran(ClientError::Refused(Refused::receipt(
            serde_json::json!({ "ok": false, "error": "refused" }),
        ))));
    }

    #[test]
    fn receipt_parser_requires_an_object() {
        assert_eq!(parse_receipt(br#"{"ok":true}"#).unwrap()["ok"], true);
        assert!(matches!(
            parse_receipt(br#"[]"#),
            Err(ClientError::OutcomeUnknown(_))
        ));
    }

    #[test]
    fn remove_marked_refuses_an_over_cap_request_before_spawn() {
        let ids = (0..MAX_REMOVE_MARK_IDS.saturating_add(1))
            .map(|number| number.to_string())
            .collect::<Vec<_>>();
        let error = remove_marked(
            PathBuf::from("journal"),
            "2026-08-06".to_owned(),
            "2026-08-06T00:00:00Z".to_owned(),
            "{}".to_owned(),
            &ids,
        )
        .unwrap_err();
        assert!(matches!(error, ClientError::RequestTooLarge(_)));
    }

    #[test]
    fn per_target_bytes_is_measured_from_an_assumed_capacity_target_outcome() {
        let long_path = "chronicle/20260520/default/094000_300/screen.webm";
        let row = TargetOutcome {
            target: Target {
                day: "20260520".to_owned(),
                stream: "default".to_owned(),
                dir: "094000_300".to_owned(),
            },
            removed: Vec::new(),
            not_removed: vec![NotRemoved {
                entry: long_path.to_owned(),
                reason: "your retention settings don't release these originals yet. they aren't old enough.".to_owned(),
                staged: None,
            }],
            post_commit_failure: None,
        };
        let mut receipt_row = serde_json::to_value(row).unwrap();
        let removed = (0..ASSUMED_MAX_NAMES_PER_MARK)
            .map(|_| Value::String(long_path.to_owned()))
            .collect();
        receipt_row["removed"] = Value::Array(removed);
        let serialized = serde_json::to_vec(&receipt_row).unwrap();
        assert_eq!(serialized.len(), PER_TARGET_BYTES);
        assert_eq!(PER_TARGET_BYTES, 26_889);
        assert_eq!(child_timeout().as_secs(), 480);
        assert!(
            child_timeout()
                >= Duration::from_secs(
                    (MAX_REMOVE_MARK_IDS as u64).saturating_mul(LOCK_WAIT_SECONDS)
                ),
            "the child timeout must cover lock waiting for every accepted mark"
        );
        assert_eq!(stdout_limit(), 860_704);
    }
}
