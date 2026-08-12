// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native `journal restart-convey` protocol.
//!
//! The protocol is deliberately split from the Unix-socket adapter so a test
//! can drive its causal ordering with a controlled peer.  A successful result
//! requires the restart's first correlated start event and a freshly replaced
//! `health/convey.port`; the port file is written only after Convey binds.

use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use solstone_core_callosum::CallosumEnvelope;

const LOG_RULE: &str = "------------------------------------------------------------";
static NEXT_RESTART_ID: AtomicU64 = AtomicU64::new(1);

/// Parsed runtime options for the native restart operation.
#[derive(Debug, Clone, Copy)]
pub struct RestartConveyOptions {
    /// The parsed Python-compatible float timeout.  `inf` waits indefinitely;
    /// `nan` and non-positive values expire immediately, as Python's loop does.
    pub timeout: f64,
    pub verbose: bool,
}

/// Completed restart output for the owner-facing command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestartConveyReport {
    stdout: String,
    stderr: String,
}

impl RestartConveyReport {
    #[must_use]
    pub fn stdout(&self) -> &str {
        &self.stdout
    }

    #[must_use]
    pub fn stderr(&self) -> &str {
        &self.stderr
    }
}

/// Native restart failure with the complete owner-facing output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestartConveyError {
    stdout: String,
    stderr: String,
}

impl RestartConveyError {
    #[must_use]
    pub fn stdout(&self) -> &str {
        &self.stdout
    }

    #[must_use]
    pub fn stderr(&self) -> &str {
        &self.stderr
    }
}

/// The minimal transport boundary used by the restart protocol and its tests.
pub trait RestartTransport {
    fn send_restart(&mut self, restart_id: &str) -> Result<(), String>;
    fn start_timer(&mut self);
    fn elapsed(&mut self) -> Duration;
    fn next_event(&mut self, timeout: Duration) -> Result<Option<CallosumEnvelope>, String>;
}

/// Run against the journal's Callosum socket.
pub fn restart_convey(
    journal_root: &Path,
    options: RestartConveyOptions,
) -> Result<RestartConveyReport, RestartConveyError> {
    let socket = journal_root.join("health/callosum.sock");
    let transport = UnixRestartTransport::connect(&socket)
        .map_err(|error| failure_connect(journal_root, options.timeout, &socket, error))?;
    restart_convey_with_transport(journal_root, options, transport)
}

/// Run with an explicit peer.  This is public so protocol tests do not require
/// a live journal or a spawned Convey process.
pub fn restart_convey_with_transport<T: RestartTransport>(
    journal_root: &Path,
    options: RestartConveyOptions,
    mut transport: T,
) -> Result<RestartConveyReport, RestartConveyError> {
    let before_inode = port_inode(journal_root);
    let restart_id = next_restart_id();
    let stdout = owner_stdout(journal_root, options.timeout);
    let mut stderr = String::new();
    if options.verbose {
        stderr.push_str("Connecting to Callosum...\nSending restart request to supervisor...\n");
    }
    transport
        .send_restart(&restart_id)
        .map_err(|_| failure_emit(stdout.clone()))?;
    transport.start_timer();
    let mut saw_start = false;
    let mut waiting_announced = false;
    let mut logs = Vec::new();
    loop {
        if saw_start && let Some(port) = fresh_port(journal_root, before_inode) {
            if options.verbose {
                if !waiting_announced {
                    stderr.push_str(&waiting_message(options.timeout));
                }
                stderr.push_str("Waiting for Flask to bind port...\n");
                stderr.push_str(&format!(
                    "Restarted in {:.1}s\n",
                    transport.elapsed().as_secs_f64()
                ));
            }
            return Ok(RestartConveyReport {
                stdout: format!("{stdout}Convey running at http://localhost:{port}/\n"),
                stderr,
            });
        }
        let elapsed = transport.elapsed();
        // Not `elapsed >= timeout`: `nan` must expire immediately, as Python's
        // `while elapsed < timeout` loop does.
        if !matches!(
            elapsed.as_secs_f64().partial_cmp(&options.timeout),
            Some(std::cmp::Ordering::Less)
        ) {
            if options.verbose && !waiting_announced {
                stderr.push_str(&waiting_message(options.timeout));
            }
            return Err(failure_timeout(
                options.timeout,
                &logs,
                options.verbose,
                stdout,
                stderr,
            ));
        }
        let event = transport
            .next_event(Duration::from_millis(50))
            .map_err(|_| failure_emit(stdout.clone()))?;
        let Some(event) = event else {
            if options.verbose && !waiting_announced {
                stderr.push_str(&waiting_message(options.timeout));
                waiting_announced = true;
            }
            continue;
        };
        if matching_restart(&event, &restart_id) {
            if event.tract == "logs" && event.event == "line" {
                let line = format_log(&event);
                if options.verbose {
                    stderr.push_str(&line);
                    stderr.push('\n');
                }
                logs.push(line);
                continue;
            }
            if event.tract == "supervisor" && event.event == "stopped" {
                if options.verbose {
                    stderr.push_str(&format!(
                        "Convey stopped (exit code: {})\n",
                        event
                            .extra
                            .get("exit_code")
                            .and_then(Value::as_i64)
                            .unwrap_or_default()
                    ));
                }
                continue;
            }
            if event.tract == "supervisor" && event.event == "restarting" {
                if options.verbose {
                    stderr.push_str("Restarting convey service...\n");
                }
                continue;
            }
            if event.tract == "supervisor" && event.event == "started" {
                if saw_start {
                    return Err(failure_crashed(&logs, options.verbose, stdout, stderr));
                }
                saw_start = true;
                if options.verbose {
                    stderr.push_str(&format!(
                        "Convey started (pid: {}, ref: {})\n",
                        event
                            .extra
                            .get("pid")
                            .and_then(Value::as_u64)
                            .unwrap_or_default(),
                        event
                            .extra
                            .get("ref")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                    ));
                }
            }
        }
    }
}

fn owner_stdout(journal_root: &Path, timeout: f64) -> String {
    format!(
        "Journal: {}\nTimeout: {}s\n",
        journal_root.display(),
        format_timeout(timeout)
    )
}

fn format_timeout(timeout: f64) -> String {
    if timeout.is_nan() {
        "nan".to_owned()
    } else if timeout.is_infinite() {
        if timeout.is_sign_positive() {
            "inf".to_owned()
        } else {
            "-inf".to_owned()
        }
    } else if timeout.fract() == 0.0 {
        format!("{timeout:.1}")
    } else {
        timeout.to_string()
    }
}

fn waiting_message(timeout: f64) -> String {
    format!(
        "Waiting for restart (timeout: {}s)...\n",
        format_timeout(timeout)
    )
}

fn next_restart_id() -> String {
    format!(
        "native-restart-{}-{}",
        std::process::id(),
        NEXT_RESTART_ID.fetch_add(1, Ordering::Relaxed)
    )
}

fn matching_restart(event: &CallosumEnvelope, restart_id: &str) -> bool {
    event.extra.get("restart_id").and_then(Value::as_str) == Some(restart_id)
        && (event.tract == "logs"
            || event.extra.get("service").and_then(Value::as_str) == Some("convey"))
}

fn port_inode(journal_root: &Path) -> Option<u64> {
    fs::metadata(journal_root.join("health/convey.port"))
        .ok()
        .map(|metadata| metadata.ino())
}

fn fresh_port(journal_root: &Path, before_inode: Option<u64>) -> Option<u16> {
    let path = journal_root.join("health/convey.port");
    let metadata = fs::metadata(&path).ok()?;
    if before_inode.is_some_and(|inode| inode == metadata.ino()) {
        return None;
    }
    let port = fs::read_to_string(path).ok()?.trim().parse::<u16>().ok()?;
    (port != 0).then_some(port)
}

fn failure_connect(
    journal_root: &Path,
    timeout: f64,
    socket: &Path,
    error: String,
) -> RestartConveyError {
    RestartConveyError {
        stdout: owner_stdout(journal_root, timeout),
        stderr: format!(
            "ERROR: Failed to connect to Callosum socket {}: {error}\n\nERROR: Convey service failed to restart\n",
            socket.display()
        ),
    }
}

fn failure_emit(stdout: String) -> RestartConveyError {
    RestartConveyError {
        stdout,
        stderr:
            "ERROR: Failed to send restart request\n\nERROR: Convey service failed to restart\n"
                .to_owned(),
    }
}

fn failure_timeout(
    timeout: f64,
    logs: &[String],
    verbose: bool,
    stdout: String,
    mut stderr: String,
) -> RestartConveyError {
    stderr.push_str(&format!(
        "ERROR: Timeout waiting for convey to restart ({}s)\n\nERROR: Convey service failed to restart\n",
        format_timeout(timeout)
    ));
    if !verbose {
        append_logs(&mut stderr, logs);
    }
    RestartConveyError { stdout, stderr }
}

fn failure_crashed(
    logs: &[String],
    verbose: bool,
    stdout: String,
    mut stderr: String,
) -> RestartConveyError {
    stderr.push_str("ERROR: Convey crashed and restarted (attempt 2)\n\nERROR: Convey service failed to restart\n");
    if !verbose {
        append_logs(&mut stderr, logs);
    }
    RestartConveyError { stdout, stderr }
}

fn append_logs(output: &mut String, logs: &[String]) {
    if logs.is_empty() {
        return;
    }
    output.push_str("\nCollected logs:\n");
    output.push_str(LOG_RULE);
    output.push('\n');
    for line in logs {
        output.push_str(line);
        output.push('\n');
    }
    output.push_str(LOG_RULE);
    output.push('\n');
}

fn format_log(event: &CallosumEnvelope) -> String {
    let stream = match event.extra.get("stream").and_then(Value::as_str) {
        Some("stderr") => "ERR",
        _ => "OUT",
    };
    let timestamp = event
        .ts
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
        .map(|timestamp| timestamp.with_timezone(&chrono::Local))
        .unwrap_or_else(chrono::Local::now)
        .format("%H:%M:%S");
    format!(
        "[{timestamp}] [{stream}] {}",
        event
            .extra
            .get("line")
            .and_then(Value::as_str)
            .unwrap_or_default()
    )
}

struct UnixRestartTransport {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
    started: Option<Instant>,
}

impl UnixRestartTransport {
    fn connect(socket: &Path) -> Result<Self, String> {
        let stream = UnixStream::connect(socket).map_err(|error| error.to_string())?;
        let reader = BufReader::new(stream.try_clone().map_err(|error| error.to_string())?);
        Ok(Self {
            stream,
            reader,
            started: None,
        })
    }
}

impl RestartTransport for UnixRestartTransport {
    fn send_restart(&mut self, restart_id: &str) -> Result<(), String> {
        let envelope = CallosumEnvelope {
            tract: "supervisor".to_owned(),
            event: "restart".to_owned(),
            ts: None,
            extra: Map::from_iter([
                ("service".to_owned(), json!("convey")),
                ("restart_id".to_owned(), json!(restart_id)),
            ]),
        };
        let mut bytes = serde_json::to_vec(&envelope).map_err(|error| error.to_string())?;
        bytes.push(b'\n');
        self.stream
            .write_all(&bytes)
            .map_err(|error| error.to_string())?;
        self.stream.flush().map_err(|error| error.to_string())
    }

    fn start_timer(&mut self) {
        self.started = Some(Instant::now());
    }

    fn elapsed(&mut self) -> Duration {
        self.started
            .map_or(Duration::ZERO, |started| started.elapsed())
    }

    fn next_event(&mut self, timeout: Duration) -> Result<Option<CallosumEnvelope>, String> {
        self.reader
            .get_ref()
            .set_read_timeout(Some(timeout))
            .map_err(|error| error.to_string())?;
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => Ok(None),
            Ok(_) => serde_json::from_str(line.trim_end())
                .map(Some)
                .map_err(|error| error.to_string()),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                Ok(None)
            }
            Err(error) => Err(error.to_string()),
        }
    }
}
