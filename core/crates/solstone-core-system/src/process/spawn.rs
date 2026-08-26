// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
#[cfg(debug_assertions)]
use std::ffi::OsStr;
use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use chrono::Local;
use thiserror::Error;

use crate::partition::partition_for;

use super::events::{OutputStream, ProcessEvent, ProcessEventSink};
use super::log::DailyLogWriter;
use super::pdeathsig::apply_parent_death_kill;
use super::signal_aware_exit_code;
use super::terminate::{
    DRAIN_JOIN_TIMEOUT, SERVICE_SHUTDOWN_TIMEOUT, TerminationError, TerminationOutcome,
    signal_exact_instance, terminate, terminate_exact_instance,
};
use super::{InspectResult, ProcessInstance, ProcessInstanceSource, SystemProcessInstanceSource};

#[derive(Debug, Error)]
pub enum SpawnError {
    #[error("cannot spawn an empty command")]
    EmptyCommand,
    #[error("failed to prepare operational log: {0}")]
    Log(#[source] io::Error),
    #[error("failed to spawn child: {0}")]
    Spawn(#[source] io::Error),
    #[error("failed to capture birth-bound identity for spawned pid {pid}")]
    ExactInstanceUnavailable { pid: u32 },
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
    termination_mode: TerminationMode,
}

#[derive(Debug, Clone, Copy)]
enum TerminationMode {
    Legacy,
    Exact(ProcessInstance),
    ExactExited,
}

impl ManagedProcess {
    pub fn spawn(cmd: Vec<String>, options: SpawnOptions) -> Result<Self, SpawnError> {
        Self::spawn_with_mode(cmd, options, false)
    }

    pub fn spawn_exact(cmd: Vec<String>, options: SpawnOptions) -> Result<Self, SpawnError> {
        Self::spawn_with_mode(cmd, options, true)
    }

    fn spawn_with_mode(
        cmd: Vec<String>,
        options: SpawnOptions,
        exact: bool,
    ) -> Result<Self, SpawnError> {
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
        apply_parent_death_kill(&mut command);
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

        let mut process = Self {
            child,
            name,
            cmd,
            reference: options.reference,
            started_at: Instant::now(),
            log_writer: writer,
            drains,
            sink: options.sink,
            exit_emitted: false,
            termination_mode: TerminationMode::Legacy,
        };
        if exact {
            delay_exact_identity_observation_for_test(&options.environment);
            let source = SystemProcessInstanceSource;
            let observation = if force_exact_identity_unavailable_for_test(&options.environment) {
                InspectResult::Unverifiable
            } else {
                source.inspect(pid)
            };
            match observation {
                InspectResult::Present { instance, .. } => {
                    process.termination_mode = TerminationMode::Exact(instance);
                }
                _ if process
                    .child
                    .try_wait()
                    .map_err(SpawnError::Spawn)?
                    .is_some() =>
                {
                    // A short-lived child can exit before its birth-bound identity is
                    // observed. It has already been reaped, so preserving its real
                    // exit status is safer than reporting a false spawn failure.
                    process.termination_mode = TerminationMode::ExactExited;
                }
                _ => {
                    // `Child` retains the original spawned process handle. A direct
                    // kill followed by wait cannot widen to a reused PID or process
                    // group, unlike the legacy Drop route.
                    let _ = process.child.kill();
                    let _ = process.child.wait();
                    return Err(SpawnError::ExactInstanceUnavailable { pid });
                }
            }
        }
        Ok(process)
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
        match self.termination_mode {
            TerminationMode::Legacy => terminate(&mut self.child, timeout),
            TerminationMode::Exact(expected) => {
                let source = SystemProcessInstanceSource;
                terminate_exact_instance(&mut self.child, expected, timeout, &source)
            }
            TerminationMode::ExactExited => self.exact_exited_outcome(),
        }
    }

    pub fn terminate_exact(
        &mut self,
        timeout: Duration,
    ) -> Result<TerminationOutcome, TerminationError> {
        match self.termination_mode {
            TerminationMode::Exact(expected) => {
                let source = SystemProcessInstanceSource;
                terminate_exact_instance(&mut self.child, expected, timeout, &source)
            }
            TerminationMode::ExactExited => self.exact_exited_outcome(),
            TerminationMode::Legacy => Err(TerminationError::ExactInstanceUnavailable),
        }
    }

    pub fn signal_exact(
        &mut self,
        signal: nix::sys::signal::Signal,
    ) -> Result<(), TerminationError> {
        match self.termination_mode {
            TerminationMode::Exact(expected) => {
                let source = SystemProcessInstanceSource;
                signal_exact_instance(expected, signal, &source)
            }
            TerminationMode::Legacy | TerminationMode::ExactExited => {
                Err(TerminationError::ExactInstanceUnavailable)
            }
        }
    }

    pub fn log_path(&self) -> PathBuf {
        self.log_writer
            .lock()
            .expect("log writer lock poisoned")
            .path()
    }

    fn exact_exited_outcome(&mut self) -> Result<TerminationOutcome, TerminationError> {
        let Some(status) = self.child.try_wait()? else {
            return Err(TerminationError::ExactInstanceUnavailable);
        };
        Ok(TerminationOutcome::Graceful {
            exit_code: Some(signal_aware_exit_code(&status)),
        })
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
            join_drain_bounded(drain);
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

/// Delay exact identity observation only for a debug-test process that opts in
/// through its explicitly supplied child environment. This deterministically
/// covers the short-lived-child race without adding a production control path.
fn delay_exact_identity_observation_for_test(_environment: &BTreeMap<OsString, OsString>) {
    #[cfg(debug_assertions)]
    {
        const DELAY_ENV: &str = "SOLSTONE_TEST_EXACT_SPAWN_INSPECT_DELAY_MS";
        let Some(delay) = _environment
            .get(OsStr::new(DELAY_ENV))
            .and_then(|value| value.to_str())
            .and_then(|value| value.parse::<u64>().ok())
        else {
            return;
        };
        thread::sleep(Duration::from_millis(delay));
    }
}

/// Force the live-but-unverifiable exact-spawn branch only for an explicitly
/// opted-in debug test process.
fn force_exact_identity_unavailable_for_test(environment: &BTreeMap<OsString, OsString>) -> bool {
    #[cfg(debug_assertions)]
    {
        environment
            .get(OsStr::new("SOLSTONE_TEST_EXACT_SPAWN_FORCE_UNVERIFIABLE"))
            .is_some_and(|value| value == "1")
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = environment;
        false
    }
}

impl Drop for ManagedProcess {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = match self.termination_mode {
                TerminationMode::Legacy => self.terminate(SERVICE_SHUTDOWN_TIMEOUT),
                TerminationMode::Exact(_) | TerminationMode::ExactExited => {
                    self.terminate_exact(SERVICE_SHUTDOWN_TIMEOUT)
                }
            };
        }
        self.cleanup();
    }
}

fn join_drain_bounded(handle: JoinHandle<()>) {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = handle.join();
        let _ = tx.send(());
    });
    if rx.recv_timeout(DRAIN_JOIN_TIMEOUT).is_err() {
        eprintln!("managed process: drain join exceeded DRAIN_JOIN_TIMEOUT; detaching");
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
