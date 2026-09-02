// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Public managed-process facade over atomic Windows Job enrollment.

use std::io::{self, BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Output};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use chrono::Local;

use crate::partition::partition_for;

use super::super::events::{OutputStream, ProcessEvent, ProcessEventSink};
use super::super::log::DailyLogWriter;
use super::super::{
    BoxedTerminateFn, CommandLaunchRequest, DRAIN_JOIN_TIMEOUT, DescendantObservationFailure,
    DescendantTerminationOutcome, Disposition, HostedLaunchProvenance, LaunchError,
    LaunchedProcessIdentity, ManagedLaunchRequest, ProcessInstance, ProcessInstanceSource,
    SERVICE_SHUTDOWN_TIMEOUT, SignalKind, SpawnError, SpawnOptions, TerminationError,
    TerminationOutcome,
};
use super::job_process::{WindowsJobProcess, launch_windows_job_process};

/// A process whose root entered an unnamed kill-on-close Job atomically at
/// `CreateProcessW`, together with the ordinary managed-process log contract.
pub struct ManagedProcess {
    owner: WindowsJobProcess,
    name: String,
    cmd: Vec<String>,
    reference: String,
    started_at: Instant,
    log_writer: Arc<Mutex<DailyLogWriter>>,
    drains: Vec<JoinHandle<()>>,
    sink: Option<Arc<dyn ProcessEventSink>>,
    exit_emitted: bool,
    instance: ProcessInstance,
    exact_identity: Option<LaunchedProcessIdentity>,
    bounded_shutdown_detached: bool,
}

impl ManagedProcess {
    pub fn spawn(cmd: Vec<String>, options: SpawnOptions) -> Result<Self, SpawnError> {
        Self::spawn_with_mode(cmd, options, false)
    }

    /// The Windows primitive captures a birth-bound instance while the root is
    /// atomically enrolled in its Job. Windows does not have the Unix UID that
    /// the legacy `LaunchedProcessIdentity` also carries, so `exact_identity()`
    /// remains absent until a caller with a platform identity binds one.
    pub fn spawn_exact(cmd: Vec<String>, options: SpawnOptions) -> Result<Self, SpawnError> {
        Self::spawn_with_mode(cmd, options, true)
    }

    fn spawn_with_mode(
        cmd: Vec<String>,
        options: SpawnOptions,
        _exact: bool,
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

        let mut owner =
            launch_windows_job_process(&cmd, &options.environment).map_err(SpawnError::Spawn)?;
        let instance = owner.identity();
        let pid = instance.pid;
        let (stdout, stderr) = owner.take_output_files();

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

        let drains = vec![
            spawn_drain(
                stdout,
                OutputStream::Stdout,
                Arc::clone(&writer),
                options.sink.clone(),
                options.reference.clone(),
                name.clone(),
                pid,
            ),
            spawn_drain(
                stderr,
                OutputStream::Stderr,
                Arc::clone(&writer),
                options.sink.clone(),
                options.reference.clone(),
                name.clone(),
                pid,
            ),
        ];

        Ok(Self {
            owner,
            name,
            cmd,
            reference: options.reference,
            started_at: Instant::now(),
            log_writer: writer,
            drains,
            sink: options.sink,
            exit_emitted: false,
            instance,
            exact_identity: None,
            bounded_shutdown_detached: false,
        })
    }

    pub fn pid(&self) -> u32 {
        self.instance.pid
    }

    pub fn exact_identity(&self) -> Option<LaunchedProcessIdentity> {
        self.exact_identity
    }

    pub(super) fn bind_exact_identity(
        &mut self,
        identity: LaunchedProcessIdentity,
    ) -> io::Result<()> {
        if identity.instance != self.instance {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "provided exact identity does not match the Job-enrolled root",
            ));
        }
        self.exact_identity = Some(identity);
        Ok(())
    }

    pub fn pgid(&self) -> io::Result<i32> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Windows owner scopes use Job Objects, not process groups",
        ))
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn cmd(&self) -> &[String] {
        &self.cmd
    }

    pub fn poll(&mut self) -> io::Result<Option<i32>> {
        self.owner.poll()
    }

    pub fn wait(&mut self) -> io::Result<i32> {
        self.owner.wait()
    }

    pub fn terminate(&mut self, timeout: Duration) -> Result<TerminationOutcome, TerminationError> {
        self.terminate_until(Instant::now() + timeout)
    }

    pub fn terminate_exact(
        &mut self,
        timeout: Duration,
    ) -> Result<TerminationOutcome, TerminationError> {
        self.terminate_until(Instant::now() + timeout)
    }

    pub fn terminate_exact_until(
        &mut self,
        deadline: Instant,
    ) -> Result<TerminationOutcome, TerminationError> {
        self.terminate_until(deadline)
    }

    fn terminate_until(
        &mut self,
        deadline: Instant,
    ) -> Result<TerminationOutcome, TerminationError> {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Windows Job termination deadline already elapsed",
            )
            .into());
        }
        if self.owner.is_quiescent()? {
            return Ok(TerminationOutcome::Graceful {
                exit_code: self.owner.poll()?,
            });
        }
        let exit_code = self.owner.hard_stop_until(deadline)?;
        Ok(TerminationOutcome::EscalatedAndReaped {
            exit_code: Some(exit_code),
        })
    }

    /// Prevent Drop from opening another bounded termination window after the
    /// caller has exhausted its own deadline.
    pub fn detach_after_bounded_shutdown(&mut self) {
        self.bounded_shutdown_detached = true;
        self.drains.clear();
    }

    pub fn signal_exact(&mut self, _signal: SignalKind) -> Result<(), TerminationError> {
        Err(TerminationError::ExactInstanceUnavailable)
    }

    pub fn log_path(&self) -> PathBuf {
        self.log_writer
            .lock()
            .expect("log writer lock poisoned")
            .path()
    }

    pub fn cleanup(&mut self) {
        if self.owner.poll().ok().flatten().is_none() {
            return;
        }
        for drain in self.drains.drain(..) {
            join_drain_bounded(drain);
        }
        self.emit_exit();
    }

    pub fn cleanup_until(&mut self, deadline: Instant) -> bool {
        if self.owner.poll().ok().flatten().is_none() {
            return false;
        }
        let mut completed = true;
        for drain in self.drains.drain(..) {
            completed &= join_drain_until(drain, deadline);
        }
        self.emit_exit();
        completed
    }

    fn emit_exit(&mut self) {
        if self.exit_emitted {
            return;
        }
        emit(
            &self.sink,
            ProcessEvent::Exited {
                reference: self.reference.clone(),
                name: self.name.clone(),
                pid: self.pid(),
                exit_code: self.owner.poll().ok().flatten(),
                duration: self.started_at.elapsed(),
                cmd: self.cmd.clone(),
                log_path: self.log_path(),
            },
        );
        self.exit_emitted = true;
    }
}

impl Drop for ManagedProcess {
    fn drop(&mut self) {
        if !self.bounded_shutdown_detached && self.owner.is_quiescent().ok() != Some(true) {
            let _ = self.owner.hard_stop();
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

fn join_drain_until(handle: JoinHandle<()>, deadline: Instant) -> bool {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return false;
    }
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = handle.join();
        let _ = tx.send(());
    });
    rx.recv_timeout(remaining).is_ok()
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

/// Retained authority over an atomically Job-contained managed process.
pub struct LaunchAuthority {
    process: ManagedProcess,
    disposition: Disposition,
}

impl std::fmt::Debug for LaunchAuthority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LaunchAuthority")
            .field("pid", &self.pid())
            .field("disposition", &self.disposition)
            .finish()
    }
}

impl LaunchAuthority {
    pub fn pid(&self) -> u32 {
        self.process.pid()
    }

    pub fn disposition(&self) -> &Disposition {
        &self.disposition
    }

    pub fn exact_identity(&self) -> Option<LaunchedProcessIdentity> {
        self.process.exact_identity()
    }

    pub fn bind_exact_identity(
        &mut self,
        identity: LaunchedProcessIdentity,
    ) -> Result<(), LaunchError> {
        self.process
            .bind_exact_identity(identity)
            .map_err(|source| LaunchError::ConfirmationFailed {
                pid: self.pid(),
                source,
            })
    }

    pub fn poll(&mut self) -> io::Result<Option<i32>> {
        self.process.poll()
    }

    pub fn wait(&mut self) -> io::Result<i32> {
        self.process.wait()
    }

    pub fn terminate(&mut self, timeout: Duration) -> Result<(), LaunchError> {
        self.process
            .terminate(timeout)
            .map(|_| ())
            .map_err(|error| LaunchError::Terminate(io::Error::other(error)))
    }

    pub fn terminate_exact(&mut self, timeout: Duration) -> Result<(), LaunchError> {
        self.process
            .terminate_exact(timeout)
            .map(|_| ())
            .map_err(|error| LaunchError::Terminate(io::Error::other(error)))
    }

    pub(crate) fn terminate_exact_until(&mut self, deadline: Instant) -> Result<(), LaunchError> {
        self.process
            .terminate_exact_until(deadline)
            .map(|_| ())
            .map_err(|error| LaunchError::Terminate(io::Error::other(error)))
    }

    /// Managed Windows launches own and drain their standard handles; raw child
    /// pipe escape hatches are deliberately unavailable.
    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        None
    }

    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        None
    }

    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        None
    }

    pub fn wait_with_output(mut self) -> Result<Output, LaunchError> {
        let _ = self.terminate(SERVICE_SHUTDOWN_TIMEOUT);
        Err(LaunchError::OutputUnavailable)
    }

    pub fn cleanup(&mut self) {
        self.process.cleanup();
    }

    pub fn relinquish_explicitly_unowned(self) -> Result<(), LaunchError> {
        if !matches!(self.disposition, Disposition::ExplicitlyUnowned { .. }) {
            return Err(LaunchError::NotExplicitlyUnowned);
        }
        Err(LaunchError::Admission(
            "Windows managed launch cannot relinquish its required Job authority".to_owned(),
        ))
    }

    pub fn into_managed(self) -> Result<ManagedProcess, LaunchError> {
        Ok(self.process)
    }

    pub(crate) fn cleanup_until(&mut self, deadline: Instant) -> bool {
        self.process.cleanup_until(deadline)
    }

    pub(crate) fn detach_after_bounded_shutdown(&mut self) {
        self.process.detach_after_bounded_shutdown();
    }
}

fn reject_empty_or_unowned(disposition: &Disposition) -> Result<(), LaunchError> {
    match disposition {
        Disposition::ExplicitlyUnowned { reason } if reason.is_empty() => {
            Err(LaunchError::EmptyUnownedReason)
        }
        Disposition::ExplicitlyUnowned { .. } => Err(LaunchError::Admission(
            "an explicitly unowned request cannot use the Windows managed Job facade".to_owned(),
        )),
        _ => Ok(()),
    }
}

fn raw_launch_unavailable() -> LaunchError {
    LaunchError::CapabilityUnavailable {
        needed: "Windows atomic managed launch request",
    }
}

pub fn launch<F>(
    _disposition: Disposition,
    _spawn: F,
    _terminate_fn: BoxedTerminateFn,
) -> Result<LaunchAuthority, LaunchError>
where
    F: FnOnce() -> io::Result<Child>,
{
    Err(raw_launch_unavailable())
}

pub fn launch_managed<F>(disposition: Disposition, spawn: F) -> Result<LaunchAuthority, LaunchError>
where
    F: FnOnce() -> Result<ManagedProcess, SpawnError>,
{
    reject_empty_or_unowned(&disposition)?;
    let process = spawn().map_err(LaunchError::SpawnManaged)?;
    Ok(LaunchAuthority {
        process,
        disposition,
    })
}

pub fn launch_with<F, Cap, Conf>(
    _disposition: Disposition,
    _spawn: F,
    _terminate_fn: BoxedTerminateFn,
    _capability: Cap,
    _confirm: Conf,
) -> Result<LaunchAuthority, LaunchError>
where
    F: FnOnce() -> io::Result<Child>,
    Cap: FnOnce(&Disposition) -> Result<(), LaunchError>,
    Conf: FnOnce(u32) -> io::Result<()>,
{
    Err(raw_launch_unavailable())
}

pub fn launch_managed_with<F, Cap>(
    disposition: Disposition,
    spawn: F,
    capability: Cap,
) -> Result<LaunchAuthority, LaunchError>
where
    F: FnOnce() -> Result<ManagedProcess, SpawnError>,
    Cap: FnOnce(&Disposition) -> Result<(), LaunchError>,
{
    reject_empty_or_unowned(&disposition)?;
    capability(&disposition)?;
    let process = spawn().map_err(LaunchError::SpawnManaged)?;
    Ok(LaunchAuthority {
        process,
        disposition,
    })
}

pub fn launch_command(
    _disposition: Disposition,
    _request: CommandLaunchRequest,
    _terminate_fn: BoxedTerminateFn,
) -> Result<LaunchAuthority, LaunchError> {
    Err(raw_launch_unavailable())
}

pub fn launch_command_hosted(
    _disposition: Disposition,
    _request: CommandLaunchRequest,
    _provenance: HostedLaunchProvenance,
    _terminate_fn: BoxedTerminateFn,
) -> Result<LaunchAuthority, LaunchError> {
    Err(raw_launch_unavailable())
}

pub fn launch_managed_request(
    disposition: Disposition,
    request: ManagedLaunchRequest,
) -> Result<LaunchAuthority, LaunchError> {
    launch_managed(disposition, move || {
        ManagedProcess::spawn_exact(request.command, request.options)
    })
}

pub fn launch_managed_hosted(
    _disposition: Disposition,
    _request: ManagedLaunchRequest,
    _provenance: HostedLaunchProvenance,
) -> Result<LaunchAuthority, LaunchError> {
    Err(LaunchError::CapabilityUnavailable {
        needed: "Windows hosted launch admission",
    })
}

pub fn terminate_descendants_exact<F>(
    _root: ProcessInstance,
    _owner_uid: u32,
    _timeout: Duration,
    _source: &dyn ProcessInstanceSource,
    stop_service: F,
) -> Result<DescendantTerminationOutcome, DescendantObservationFailure>
where
    F: FnOnce(),
{
    stop_service();
    Err(DescendantObservationFailure::CensusIncomplete)
}

pub fn terminate(
    _child: &mut Child,
    _timeout: Duration,
) -> Result<TerminationOutcome, TerminationError> {
    Err(TerminationError::DescendantCoverageUnavailable)
}

pub fn terminate_exact_instance(
    _child: &mut Child,
    _expected: ProcessInstance,
    _timeout: Duration,
    _source: &dyn ProcessInstanceSource,
) -> Result<TerminationOutcome, TerminationError> {
    Err(TerminationError::DescendantCoverageUnavailable)
}

pub fn signal_exact_instance(
    _expected: ProcessInstance,
    _signal: SignalKind,
    _source: &dyn ProcessInstanceSource,
) -> Result<(), TerminationError> {
    Err(TerminationError::DescendantCoverageUnavailable)
}

pub fn apply_parent_death_kill(_command: &mut Command) {}
