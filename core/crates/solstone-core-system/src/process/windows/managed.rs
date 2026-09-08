// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Public managed-process facade over atomic Windows Job enrollment.

use std::io::{self, BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Output};
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
use super::job::JobResourceLimits;
use super::job_process::{
    JOB_HARD_STOP_TIMEOUT, WindowsJobLaunchOptions, WindowsJobProcess, launch_windows_job_process,
    launch_windows_job_process_with_options,
};

/// A process whose root entered an unnamed kill-on-close Job atomically at
/// `CreateProcessW`, together with the ordinary managed-process log contract.
pub struct ManagedProcess {
    owner: Option<WindowsJobProcess>,
    reservation: Option<super::bounded_cleanup::Reservation>,
    resources: super::bounded::BoundedHelperResources,
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
    stop_event: Option<std::os::windows::io::OwnedHandle>,
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

    /// Start a retained provider from an already-admitted package boundary.
    ///
    /// The caller supplies only canonical, package-owned paths and a complete
    /// environment. This method deliberately remains crate-private: public
    /// consumers use the validating independent-provider authority rather
    /// than composing a Job launch from arbitrary paths or inherited state.
    pub(crate) fn spawn_package_owned(
        cmd: Vec<String>,
        options: SpawnOptions,
        current_directory: &std::path::Path,
        resource_limits: Option<(u32, usize)>,
    ) -> Result<Self, SpawnError> {
        if cmd.is_empty() {
            return Err(SpawnError::EmptyCommand);
        }
        let (name, writer, log_path) = prepare_managed_log(&cmd, &options)?;
        let reservation = super::bounded_cleanup::Reservation::track_until(
            Instant::now() + JOB_HARD_STOP_TIMEOUT,
        )
        .map_err(|error| SpawnError::Spawn(io::Error::other(error)))?;
        let owner = match launch_windows_job_process_with_options(
            &cmd,
            &options.environment,
            WindowsJobLaunchOptions {
                current_directory: Some(current_directory),
                resource_limits: resource_limits.map(
                    |(cpu_rate_per_10_000, committed_memory_bytes)| JobResourceLimits {
                        cpu_rate_per_10_000,
                        committed_memory_bytes,
                    },
                ),
                exact_environment: true,
                retain_parent_stdin: false,
                null_stdio: [false; 3],
            },
        ) {
            Ok(owner) => owner,
            Err(failure) => {
                return Err(SpawnError::Spawn(io::Error::other(
                    reservation.launch_failure(failure, Default::default()),
                )));
            }
        };
        Self::from_owned_job(
            cmd,
            options,
            owner,
            name,
            writer,
            log_path,
            reservation,
            Default::default(),
        )
    }

    fn spawn_with_mode(
        cmd: Vec<String>,
        options: SpawnOptions,
        _exact: bool,
    ) -> Result<Self, SpawnError> {
        if cmd.is_empty() {
            return Err(SpawnError::EmptyCommand);
        }
        let (name, writer, log_path) = prepare_managed_log(&cmd, &options)?;
        let reservation = super::bounded_cleanup::Reservation::track_until(
            Instant::now() + JOB_HARD_STOP_TIMEOUT,
        )
        .map_err(|error| SpawnError::Spawn(io::Error::other(error)))?;
        let owner = match launch_windows_job_process(&cmd, &options.environment) {
            Ok(owner) => owner,
            Err(failure) => {
                return Err(SpawnError::Spawn(io::Error::other(
                    reservation.launch_failure(failure, Default::default()),
                )));
            }
        };
        Self::from_owned_job(
            cmd,
            options,
            owner,
            name,
            writer,
            log_path,
            reservation,
            Default::default(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn from_owned_job(
        cmd: Vec<String>,
        options: SpawnOptions,
        owner: WindowsJobProcess,
        name: String,
        writer: Arc<Mutex<DailyLogWriter>>,
        log_path: PathBuf,
        reservation: super::bounded_cleanup::Reservation,
        mut resources: super::bounded::BoundedHelperResources,
    ) -> Result<Self, SpawnError> {
        let instance = owner.identity();
        resources.retain(writer.clone());
        // Install the complete owner before callbacks or fallible worker setup.
        // Unwind uses the same Drop transfer as ordinary incomplete cleanup.
        let mut process = Self {
            owner: Some(owner),
            reservation: Some(reservation),
            resources,
            name,
            cmd,
            reference: options.reference,
            started_at: Instant::now(),
            log_writer: writer,
            drains: Vec::with_capacity(2),
            sink: options.sink,
            exit_emitted: false,
            instance,
            exact_identity: None,
            bounded_shutdown_detached: false,
            stop_event: None,
        };
        let (stdout, stderr) = process.owner_mut().take_output_files();
        for (reader, stream) in [
            (stdout, OutputStream::Stdout),
            (stderr, OutputStream::Stderr),
        ] {
            match spawn_drain(
                reader,
                stream,
                process.log_writer.clone(),
                process.sink.clone(),
                process.reference.clone(),
                process.name.clone(),
                instance.pid,
            ) {
                Ok(worker) => process.drains.push(worker),
                Err(error) => {
                    let failure = process
                        .retain_cleanup(error.to_string(), Instant::now() + DRAIN_JOIN_TIMEOUT);
                    return Err(SpawnError::Spawn(io::Error::other(failure)));
                }
            }
        }
        emit(
            &process.sink,
            ProcessEvent::Spawned {
                reference: process.reference.clone(),
                name: process.name.clone(),
                pid: instance.pid,
                cmd: process.cmd.clone(),
                log_path,
            },
        );
        Ok(process)
    }

    fn owner(&self) -> &WindowsJobProcess {
        self.owner.as_ref().expect("managed owner retained")
    }
    fn owner_mut(&mut self) -> &mut WindowsJobProcess {
        self.owner.as_mut().expect("managed owner retained")
    }

    fn retain_cleanup(
        &mut self,
        detail: String,
        deadline: Instant,
    ) -> super::bounded_cleanup::BoundedHelperFailure {
        let mut owner = self.owner.take().expect("managed owner transferred once");
        if !self.bounded_shutdown_detached
            && owner.is_quiescent().ok() != Some(true)
            && Instant::now() < deadline
        {
            let _ = owner.hard_stop_until(deadline);
        }
        let mut io = super::bounded_cleanup::HelperIo::unstarted();
        io.workers = std::mem::take(&mut self.drains);
        io.drain_until(deadline);
        let cause = super::bounded::BoundedHelperError::LaunchFinalizationFailed {
            detail,
            process_id: Some(self.instance.pid),
        };
        self.reservation
            .take()
            .expect("managed reservation transferred once")
            .failure(cause, owner, io, std::mem::take(&mut self.resources))
    }

    /// Fault control for descendant-only capability lifetime receipts. This
    /// deliberately removes this successfully admitted launch's parent grant
    /// copies while preserving its original Job, stop, log and drain owners.
    /// Production builds have no resource-release escape.
    #[cfg(feature = "test-hooks")]
    pub fn release_parent_read_file_grants_for_test(&mut self) -> io::Result<()> {
        if self.stop_event.is_none()
            || self.resources.len() != 2
            || self.owner_mut().poll()?.is_some()
        {
            return Err(io::Error::other(
                "expected a live admitted managed test launch",
            ));
        }
        // launch_managed_hosted installs the grant Vec; from_owned_job adds
        // the log writer. This hook is not a generic resource-bag operation.
        let mut retained = super::bounded::BoundedHelperResources::new();
        retained.retain(self.log_writer.clone());
        self.resources = retained;
        Ok(())
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
        self.owner_mut().poll()
    }

    pub fn wait(&mut self) -> io::Result<i32> {
        self.owner_mut().wait()
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
        if self.owner().is_quiescent()? {
            return Ok(TerminationOutcome::Graceful {
                exit_code: self.owner_mut().poll()?,
            });
        }
        if let Some(stop) = self.stop_event.as_ref() {
            super::launch_control::signal_stop(stop)?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            let stop_deadline = Instant::now()
                + SERVICE_SHUTDOWN_TIMEOUT.min(remaining.saturating_sub(JOB_HARD_STOP_TIMEOUT));
            while Instant::now() < stop_deadline {
                if self.owner().is_quiescent()? {
                    return Ok(TerminationOutcome::Graceful {
                        exit_code: self.owner_mut().poll()?,
                    });
                }
                thread::sleep(Duration::from_millis(50));
            }
        }
        let exit_code = self.owner_mut().hard_stop_until(deadline)?;
        Ok(TerminationOutcome::EscalatedAndReaped {
            exit_code: Some(exit_code),
        })
    }

    /// Prevent Drop from opening another bounded termination window after the
    /// caller has exhausted its own deadline.
    pub fn detach_after_bounded_shutdown(&mut self) {
        self.bounded_shutdown_detached = true;
        // Drop transfers any unfinished drains and native owner to retained cleanup.
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
        let _ = self.cleanup_until(Instant::now() + DRAIN_JOIN_TIMEOUT);
    }

    pub fn cleanup_until(&mut self, deadline: Instant) -> bool {
        if self.owner().is_quiescent().ok() != Some(true) {
            return false;
        }
        super::bounded_cleanup::drain_workers_until(&mut self.drains, deadline);
        if !self.drains.is_empty() {
            return false;
        }
        self.emit_exit();
        // Explicitly release generation and log-resource copies on completion,
        // even when the managed facade remains alive for inspection.
        self.resources = Default::default();
        true
    }

    fn emit_exit(&mut self) {
        if self.exit_emitted {
            return;
        }
        let exit_code = self.owner_mut().poll().ok().flatten();
        emit(
            &self.sink,
            ProcessEvent::Exited {
                reference: self.reference.clone(),
                name: self.name.clone(),
                pid: self.pid(),
                exit_code,
                duration: self.started_at.elapsed(),
                cmd: self.cmd.clone(),
                log_path: self.log_path(),
            },
        );
        self.exit_emitted = true;
    }
}

fn prepare_managed_log(
    cmd: &[String],
    options: &SpawnOptions,
) -> Result<(String, Arc<Mutex<DailyLogWriter>>, PathBuf), SpawnError> {
    let name = partition_for(cmd).as_str().to_owned();
    let writer = DailyLogWriter::new(
        &options.journal_root,
        &options.reference,
        &name,
        options.day.clone(),
    )
    .map_err(SpawnError::Log)?;
    let log_path = writer.path();
    Ok((name, Arc::new(Mutex::new(writer)), log_path))
}

impl Drop for ManagedProcess {
    fn drop(&mut self) {
        if self.owner.is_none() {
            return;
        }
        let deadline = if self.bounded_shutdown_detached {
            Instant::now()
        } else {
            Instant::now() + DRAIN_JOIN_TIMEOUT
        };
        // The process-lifetime registry retains the original Job, drain handles,
        // and resources on incomplete cleanup, even when this error is dropped.
        let _ = self.retain_cleanup(
            "managed owner dropped before cleanup completed".into(),
            deadline,
        );
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
) -> io::Result<JoinHandle<()>>
where
    R: Read + Send + 'static,
{
    thread::Builder::new()
        .name("managed-output".into())
        .spawn(move || {
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

enum AuthorityProcess {
    Managed(ManagedProcess),
    Command(super::command::CommandProcess),
}

/// Retained authority over one atomic Job, independent of extracted stdio files.
pub struct LaunchAuthority {
    process: AuthorityProcess,
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
        match &self.process {
            AuthorityProcess::Managed(process) => process.pid(),
            AuthorityProcess::Command(process) => process.pid(),
        }
    }

    pub fn disposition(&self) -> &Disposition {
        &self.disposition
    }

    pub fn exact_identity(&self) -> Option<LaunchedProcessIdentity> {
        match &self.process {
            AuthorityProcess::Managed(process) => process.exact_identity(),
            AuthorityProcess::Command(process) => process.exact_identity(),
        }
    }

    pub fn bind_exact_identity(
        &mut self,
        identity: LaunchedProcessIdentity,
    ) -> Result<(), LaunchError> {
        let result = match &mut self.process {
            AuthorityProcess::Managed(process) => process.bind_exact_identity(identity),
            AuthorityProcess::Command(process) => process.bind_exact_identity(identity),
        };
        result.map_err(|source| LaunchError::ConfirmationFailed {
            pid: self.pid(),
            source,
        })
    }

    pub fn poll(&mut self) -> io::Result<Option<i32>> {
        match &mut self.process {
            AuthorityProcess::Managed(process) => process.poll(),
            AuthorityProcess::Command(process) => process.poll(),
        }
    }

    pub fn wait(&mut self) -> io::Result<i32> {
        match &mut self.process {
            AuthorityProcess::Managed(process) => process.wait(),
            AuthorityProcess::Command(process) => process.wait(),
        }
    }

    pub fn terminate(&mut self, timeout: Duration) -> Result<(), LaunchError> {
        self.terminate_exact_until(Instant::now() + timeout)
    }

    pub fn terminate_exact(&mut self, timeout: Duration) -> Result<(), LaunchError> {
        self.terminate(timeout)
    }

    pub(crate) fn terminate_exact_until(&mut self, deadline: Instant) -> Result<(), LaunchError> {
        match &mut self.process {
            AuthorityProcess::Managed(process) => process
                .terminate_exact_until(deadline)
                .map(|_| ())
                .map_err(|error| LaunchError::Terminate(io::Error::other(error))),
            AuthorityProcess::Command(process) => process
                .terminate_until(deadline)
                .map_err(LaunchError::Terminate),
        }
    }

    /// A command's synchronous, noninheritable parent pipe endpoint. Extracting
    /// it transfers only stream ownership; this authority retains the Job.
    pub fn take_stdin(&mut self) -> Option<std::fs::File> {
        self.take_stream(0)
    }
    pub fn take_stdout(&mut self) -> Option<std::fs::File> {
        self.take_stream(1)
    }
    pub fn take_stderr(&mut self) -> Option<std::fs::File> {
        self.take_stream(2)
    }

    fn take_stream(&mut self, index: usize) -> Option<std::fs::File> {
        match &mut self.process {
            AuthorityProcess::Command(process) => process.take_stream(index),
            AuthorityProcess::Managed(_) => None,
        }
    }

    pub fn wait_with_output(self) -> Result<Output, LaunchError> {
        match self.process {
            AuthorityProcess::Command(process) => process.output(),
            AuthorityProcess::Managed(_) => Err(LaunchError::OutputUnavailable),
        }
    }

    pub fn cleanup(&mut self) {
        match &mut self.process {
            AuthorityProcess::Managed(process) => process.cleanup(),
            AuthorityProcess::Command(process) => process.cleanup(),
        }
    }

    pub fn relinquish_explicitly_unowned(self) -> Result<(), LaunchError> {
        if !matches!(self.disposition, Disposition::ExplicitlyUnowned { .. }) {
            return Err(LaunchError::NotExplicitlyUnowned);
        }
        Err(LaunchError::Admission(
            "Windows launch cannot relinquish its required Job authority".to_owned(),
        ))
    }

    pub fn into_managed(self) -> Result<ManagedProcess, LaunchError> {
        match self.process {
            AuthorityProcess::Managed(process) => Ok(process),
            AuthorityProcess::Command(_) => Err(LaunchError::CapabilityUnavailable {
                needed: "managed operational logging",
            }),
        }
    }

    pub(crate) fn cleanup_until(&mut self, deadline: Instant) -> bool {
        match &mut self.process {
            AuthorityProcess::Managed(process) => process.cleanup_until(deadline),
            AuthorityProcess::Command(process) => process.cleanup_until(deadline),
        }
    }

    pub(crate) fn detach_after_bounded_shutdown(&mut self) {
        match &mut self.process {
            AuthorityProcess::Managed(process) => process.detach_after_bounded_shutdown(),
            AuthorityProcess::Command(process) => process.detach_after_bounded_shutdown(),
        }
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
        process: AuthorityProcess::Managed(process),
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
        process: AuthorityProcess::Managed(process),
        disposition,
    })
}

pub fn launch_command(
    disposition: Disposition,
    request: CommandLaunchRequest,
    _terminate_fn: BoxedTerminateFn,
) -> Result<LaunchAuthority, LaunchError> {
    reject_empty_or_unowned(&disposition)?;
    let process = super::command::CommandProcess::launch(&disposition, request, None)?;
    Ok(LaunchAuthority {
        process: AuthorityProcess::Command(process),
        disposition,
    })
}

pub fn launch_command_hosted(
    disposition: Disposition,
    request: CommandLaunchRequest,
    provenance: HostedLaunchProvenance,
    _terminate_fn: BoxedTerminateFn,
) -> Result<LaunchAuthority, LaunchError> {
    reject_empty_or_unowned(&disposition)?;
    let process = super::command::CommandProcess::launch(&disposition, request, Some(provenance))?;
    Ok(LaunchAuthority {
        process: AuthorityProcess::Command(process),
        disposition,
    })
}

pub fn launch_managed_request(
    disposition: Disposition,
    request: ManagedLaunchRequest,
) -> Result<LaunchAuthority, LaunchError> {
    if !request.read_file_grants.is_empty() {
        // Standalone Sense/Think owns its acquired generation too. Its native
        // children borrow through the same launch transaction as hosted work;
        // metadata in the environment never substitutes for a retained grant.
        let parent = super::identity::current_windows_process_instance()
            .map_err(|error| LaunchError::Admission(error.to_string()))?;
        let mut nonce = [0_u8; 24];
        getrandom::fill(&mut nonce).map_err(|error| LaunchError::Admission(error.to_string()))?;
        let provenance = HostedLaunchProvenance {
            journal: request.options.journal_root.clone(),
            generation: parent.birth.windows_filetime().ok_or_else(|| {
                LaunchError::Admission("Windows process birth is unavailable".into())
            })?,
            launch_id: nonce.iter().map(|byte| format!("{byte:02x}")).collect(),
            service: None,
            parent_launch_id: None,
            acknowledgement_timeout: Duration::from_secs(3),
        };
        return launch_managed_hosted(disposition, request, provenance);
    }
    launch_managed(disposition, move || {
        ManagedProcess::spawn_exact(request.command, request.options)
    })
}

pub fn launch_managed_hosted(
    disposition: Disposition,
    request: ManagedLaunchRequest,
    provenance: HostedLaunchProvenance,
) -> Result<LaunchAuthority, LaunchError> {
    reject_empty_or_unowned(&disposition)?;
    let mut options = request.options;
    let control =
        super::launch_control::LaunchControl::prepare(&provenance, &mut options.environment)
            .map_err(|error| LaunchError::Admission(error.to_string()))?;
    let (name, writer, log_path) =
        prepare_managed_log(&request.command, &options).map_err(LaunchError::SpawnManaged)?;
    let mut resources = super::bounded::BoundedHelperResources::new();
    resources.retain(Arc::new(request.read_file_grants.clone()));
    let reservation =
        super::bounded_cleanup::Reservation::track_until(Instant::now() + JOB_HARD_STOP_TIMEOUT)
            .map_err(|error| LaunchError::Spawn(io::Error::other(error)))?;
    let owner = match launch_windows_job_process(&request.command, &options.environment) {
        Ok(owner) => owner,
        Err(failure) => {
            return Err(LaunchError::Spawn(io::Error::other(
                reservation.launch_failure(failure, resources),
            )));
        }
    };
    let stop = match control.admit(&owner, &request.read_file_grants, None) {
        Ok(stop) => stop,
        Err(error) => {
            return Err(LaunchError::Spawn(io::Error::other(
                reservation.independent_failure(owner, error.to_string(), resources),
            )));
        }
    };
    let mut managed = ManagedProcess::from_owned_job(
        request.command,
        options,
        owner,
        name,
        writer,
        log_path,
        reservation,
        resources,
    )
    .map_err(LaunchError::SpawnManaged)?;
    managed.stop_event = Some(stop);
    Ok(LaunchAuthority {
        process: AuthorityProcess::Managed(managed),
        disposition,
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
