// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use chrono::Local;
use thiserror::Error;

use crate::partition::partition_for;

use super::events::{OutputStream, ProcessEvent, ProcessEventSink};
use super::log::DailyLogWriter;
use super::signal_aware_exit_code;
use super::terminate::{TerminationError, TerminationOutcome, terminate};

#[derive(Debug, Error)]
pub enum SpawnError {
    #[error("cannot spawn an empty command")]
    EmptyCommand,
    #[error("failed to prepare operational log: {0}")]
    Log(#[source] io::Error),
    #[error("failed to spawn child: {0}")]
    Spawn(#[source] io::Error),
}

/// Inputs owned by the caller rather than process-global state.
#[derive(Clone)]
pub struct SpawnOptions {
    pub journal_root: PathBuf,
    pub reference: String,
    pub day: Option<String>,
    pub sink: Option<Arc<dyn ProcessEventSink>>,
    pub environment: BTreeMap<OsString, OsString>,
}

/// A child process with journal-system operational logs and bounded cleanup.
pub struct ManagedProcess {
    child: Child,
    name: String,
    cmd: Vec<String>,
    reference: String,
    started_at: Instant,
    log_writer: Arc<Mutex<DailyLogWriter>>,
    drains: Vec<JoinHandle<()>>,
    sink: Option<Arc<dyn ProcessEventSink>>,
    exit_emitted: bool,
}

impl ManagedProcess {
    pub fn spawn(cmd: Vec<String>, options: SpawnOptions) -> Result<Self, SpawnError> {
        if cmd.is_empty() {
            return Err(SpawnError::EmptyCommand);
        }
        let name = partition_for(&cmd).as_str().to_owned();
        let writer = DailyLogWriter::new(
            &options.journal_root,
            &options.reference,
            &name,
            options.day,
        )
        .map_err(SpawnError::Log)?;
        let log_path = writer.path();
        let writer = Arc::new(Mutex::new(writer));

        let mut command = Command::new(&cmd[0]);
        command
            .args(&cmd[1..])
            .envs(&options.environment)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().map_err(SpawnError::Spawn)?;
        let pid = child.id();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        emit(
            &options.sink,
            ProcessEvent::Spawned {
                reference: options.reference.clone(),
                name: name.clone(),
                pid,
                cmd: cmd.clone(),
                log_path,
            },
        );

        let mut drains = Vec::new();
        if let Some(stdout) = stdout {
            drains.push(spawn_drain(
                stdout,
                OutputStream::Stdout,
                Arc::clone(&writer),
                options.sink.clone(),
                options.reference.clone(),
                name.clone(),
                pid,
            ));
        }
        if let Some(stderr) = stderr {
            drains.push(spawn_drain(
                stderr,
                OutputStream::Stderr,
                Arc::clone(&writer),
                options.sink.clone(),
                options.reference.clone(),
                name.clone(),
                pid,
            ));
        }

        Ok(Self {
            child,
            name,
            cmd,
            reference: options.reference,
            started_at: Instant::now(),
            log_writer: writer,
            drains,
            sink: options.sink,
            exit_emitted: false,
        })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Return the child's process group on platforms that support process groups.
    pub fn pgid(&self) -> io::Result<i32> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            let pid = i32::try_from(self.child.id())
                .map_err(|_| io::Error::other("invalid child pid"))?;
            nix::unistd::getpgid(Some(nix::unistd::Pid::from_raw(pid)))
                .map(nix::unistd::Pid::as_raw)
                .map_err(io::Error::other)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "process groups unavailable on this platform",
            ))
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn cmd(&self) -> &[String] {
        &self.cmd
    }

    pub fn poll(&mut self) -> io::Result<Option<i32>> {
        self.child
            .try_wait()
            .map(|status| status.map(|value| signal_aware_exit_code(&value)))
    }

    pub fn wait(&mut self) -> io::Result<i32> {
        self.child
            .wait()
            .map(|status| signal_aware_exit_code(&status))
    }

    pub fn terminate(&mut self, timeout: Duration) -> Result<TerminationOutcome, TerminationError> {
        terminate(&mut self.child, timeout)
    }

    pub fn log_path(&self) -> PathBuf {
        self.log_writer
            .lock()
            .expect("log writer lock poisoned")
            .path()
    }

    /// Join drains and emit the exit event after a child exits.
    ///
    /// This is a no-op while the child is alive; call it again after `wait` or
    /// `poll` observes the exit so joining an open pipe cannot block forever.
    pub fn cleanup(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            return;
        }
        for drain in self.drains.drain(..) {
            let _ = drain.join();
        }
        if self.exit_emitted {
            return;
        }
        let exit_code = self
            .child
            .try_wait()
            .ok()
            .flatten()
            .map(|status| signal_aware_exit_code(&status));
        let log_path = self.log_path();
        emit(
            &self.sink,
            ProcessEvent::Exited {
                reference: self.reference.clone(),
                name: self.name.clone(),
                pid: self.child.id(),
                exit_code,
                duration: self.started_at.elapsed(),
                cmd: self.cmd.clone(),
                log_path,
            },
        );
        self.exit_emitted = true;
    }
}

impl Drop for ManagedProcess {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn spawn_drain<R>(
    reader: R,
    stream: OutputStream,
    writer: Arc<Mutex<DailyLogWriter>>,
    sink: Option<Arc<dyn ProcessEventSink>>,
    reference: String,
    name: String,
    pid: u32,
) -> JoinHandle<()>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let clean = line.trim_end_matches('\n').to_owned();
                    let formatted = format_log_line(&name, stream, &clean);
                    if let Ok(mut writer) = writer.lock() {
                        writer.write(&formatted);
                    }
                    emit(
                        &sink,
                        ProcessEvent::Line {
                            reference: reference.clone(),
                            name: name.clone(),
                            pid,
                            stream,
                            line: clean,
                        },
                    );
                }
            }
        }
    })
}

fn format_log_line(name: &str, stream: OutputStream, line: &str) -> String {
    let stream = match stream {
        OutputStream::Stdout => "stdout",
        OutputStream::Stderr => "stderr",
    };
    format!(
        "{} [{name}:{stream}] {line}\n",
        Local::now().format("%Y-%m-%dT%H:%M:%S")
    )
}

fn emit(sink: &Option<Arc<dyn ProcessEventSink>>, event: ProcessEvent) {
    if let Some(sink) = sink {
        sink.emit(event);
    }
}
