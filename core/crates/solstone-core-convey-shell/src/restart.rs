// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native `journal restart-convey` protocol.
//!
//! The protocol is deliberately split from the Unix-socket adapter so a test
//! can drive its causal ordering with a controlled peer.  A successful result
//! requires the restart's first correlated start event and a freshly replaced
//! `health/convey.port`; the port file is written only after Convey binds.

#[cfg(test)]
use std::collections::VecDeque;
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
    pub timeout: Duration,
    pub verbose: bool,
    pub debug: bool,
}

/// Completed restart output for the owner-facing command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestartConveyReport {
    pub output: String,
}

/// Native restart failure with the complete owner-facing output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestartConveyError {
    output: String,
}

impl RestartConveyError {
    #[must_use]
    pub fn output(&self) -> &str {
        &self.output
    }
}

/// The minimal transport boundary used by the restart protocol and its tests.
pub trait RestartTransport {
    fn send_restart(&mut self, restart_id: &str) -> Result<(), String>;
    fn next_event(&mut self, timeout: Duration) -> Result<Option<CallosumEnvelope>, String>;
}

/// Run against the journal's Callosum socket.
pub fn restart_convey(
    journal_root: &Path,
    options: RestartConveyOptions,
) -> Result<RestartConveyReport, RestartConveyError> {
    let socket = journal_root.join("health/callosum.sock");
    let transport =
        UnixRestartTransport::connect(&socket).map_err(|error| failure_connect(&socket, error))?;
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
    let mut output = String::new();
    if options.verbose {
        output.push_str("Connecting to Callosum...\nSending restart request to supervisor...\n");
    }
    transport.send_restart(&restart_id).map_err(failure_emit)?;
    if options.verbose {
        output.push_str("Restarting convey service...\n");
    }

    let started = Instant::now();
    let mut saw_start = false;
    let mut logs = Vec::new();
    loop {
        if saw_start {
            if let Some(port) = fresh_port(journal_root, before_inode) {
                output.push_str(&format!("Convey running at http://localhost:{port}\n"));
                return Ok(RestartConveyReport { output });
            }
        }
        let elapsed = started.elapsed();
        if elapsed >= options.timeout {
            return Err(failure_timeout(
                options.timeout,
                &logs,
                options.verbose,
                output,
            ));
        }
        let remaining = options.timeout.saturating_sub(elapsed);
        let wait = remaining.min(Duration::from_millis(50));
        let event = transport.next_event(wait).map_err(failure_emit)?;
        let Some(event) = event else {
            continue;
        };
        if matching_restart(&event, &restart_id) {
            if event.tract == "logs" && event.event == "line" {
                let line = format_log(&event);
                if options.verbose {
                    output.push_str(&line);
                    output.push('\n');
                }
                logs.push(line);
                continue;
            }
            if event.tract == "supervisor" && event.event == "stopped" {
                if options.verbose {
                    output.push_str(&format!(
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
            if event.tract == "supervisor" && event.event == "started" {
                if saw_start {
                    return Err(failure_crashed(&logs, options.verbose, output));
                }
                saw_start = true;
                if options.verbose {
                    output.push_str(&format!(
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

fn failure_connect(socket: &Path, error: String) -> RestartConveyError {
    RestartConveyError {
        output: format!(
            "ERROR: Failed to connect to Callosum socket {}: {error}\n\nERROR: Convey service failed to restart\n",
            socket.display()
        ),
    }
}

fn failure_emit(error: String) -> RestartConveyError {
    RestartConveyError {
        output: format!(
            "ERROR: Failed to send restart request: {error}\n\nERROR: Convey service failed to restart\n"
        ),
    }
}

fn failure_timeout(
    timeout: Duration,
    logs: &[String],
    verbose: bool,
    mut output: String,
) -> RestartConveyError {
    output.push_str(&format!(
        "ERROR: Timeout waiting for convey to restart ({:.1}s)\n\nERROR: Convey service failed to restart\n",
        timeout.as_secs_f64()
    ));
    if !verbose {
        append_logs(&mut output, logs);
    }
    RestartConveyError { output }
}

fn failure_crashed(logs: &[String], verbose: bool, mut output: String) -> RestartConveyError {
    output.push_str("ERROR: Convey crashed and restarted (attempt 2)\n\nERROR: Convey service failed to restart\n");
    if !verbose {
        append_logs(&mut output, logs);
    }
    RestartConveyError { output }
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
    format!(
        "[00:00:00] [{stream}] {}",
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
}

impl UnixRestartTransport {
    fn connect(socket: &Path) -> Result<Self, String> {
        let stream = UnixStream::connect(socket).map_err(|error| error.to_string())?;
        let reader = BufReader::new(stream.try_clone().map_err(|error| error.to_string())?);
        Ok(Self { stream, reader })
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

#[cfg(test)]
pub(crate) struct TestTransport {
    pub sent: Vec<String>,
    pub events: VecDeque<CallosumEnvelope>,
}

#[cfg(test)]
impl RestartTransport for TestTransport {
    fn send_restart(&mut self, restart_id: &str) -> Result<(), String> {
        self.sent.push(restart_id.to_owned());
        Ok(())
    }
    fn next_event(&mut self, _: Duration) -> Result<Option<CallosumEnvelope>, String> {
        Ok(self.events.pop_front())
    }
}
