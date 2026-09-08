// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Caller-owned protocol streams with retained native Job authority.

use std::fs::File;
use std::io::{self, Read};
use std::os::windows::io::OwnedHandle;
use std::os::windows::process::ExitStatusExt;
use std::process::{ExitStatus, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use super::job_process::{
    JOB_HARD_STOP_TIMEOUT, WindowsJobLaunchOptions, WindowsJobProcess,
    launch_windows_native_job_process,
};
use super::launch_control::{LaunchControl, signal_stop};
use crate::process::{CommandLaunchRequest, Disposition, HostedLaunchProvenance, LaunchError};

pub(super) struct CommandProcess {
    owner: Arc<Mutex<WindowsJobProcess>>,
    timer_cancel: Option<mpsc::Sender<()>>,
    timer: Option<std::thread::JoinHandle<()>>,
    expired: Arc<AtomicBool>,
    identity: crate::process::ProcessInstance,
    reservation: Option<super::bounded_cleanup::Reservation>,
    resources: super::bounded::BoundedHelperResources,
    io: super::bounded_cleanup::HelperIo,
    streams: [Option<File>; 3],
    stop: Option<OwnedHandle>,
    deadline: Option<Instant>,
    bounded_cleanup_attempted: bool,
    exact_identity: Option<crate::process::LaunchedProcessIdentity>,
}

impl CommandProcess {
    pub(super) fn launch(
        disposition: &Disposition,
        mut request: CommandLaunchRequest,
        provenance: Option<HostedLaunchProvenance>,
    ) -> Result<Self, LaunchError> {
        if provenance.is_none() && !request.read_file_grants.is_empty() {
            return Err(LaunchError::Admission(
                "command grants require authenticated provenance".into(),
            ));
        }
        if matches!(disposition, Disposition::IndependentBoundedHelper { timeout } if timeout.is_zero())
        {
            return Err(LaunchError::Admission(
                "command timeout must be greater than zero".into(),
            ));
        }
        let started = Instant::now();
        let deadline = match disposition {
            Disposition::IndependentBoundedHelper { timeout } => Some(
                started
                    .checked_add(*timeout)
                    .ok_or_else(|| LaunchError::Admission("command timeout overflows".into()))?,
            ),
            _ => None,
        };
        let provenance = provenance.map(|mut provenance| {
            if let Some(deadline) = deadline {
                provenance.acknowledgement_timeout = provenance
                    .acknowledgement_timeout
                    .min(deadline.saturating_duration_since(Instant::now()));
            }
            provenance
        });
        let control = provenance
            .as_ref()
            .map(|provenance| LaunchControl::prepare(provenance, &mut request.environment))
            .transpose()
            .map_err(|error| LaunchError::Admission(error.to_string()))?;
        let piped = [
            request.stdin_piped,
            request.stdout_piped,
            request.stderr_piped,
        ];
        let mut command = vec![request.program];
        command.extend(request.arguments);
        let mut resources = super::bounded::BoundedHelperResources::new();
        resources.retain(Arc::new(request.read_file_grants.clone()));
        let reservation = super::bounded_cleanup::Reservation::track_until(
            deadline.unwrap_or_else(|| Instant::now() + JOB_HARD_STOP_TIMEOUT),
        )
        .map_err(|error| LaunchError::Spawn(io::Error::other(error)))?;
        let mut owner = match launch_windows_native_job_process(
            &command,
            &request.environment,
            WindowsJobLaunchOptions {
                current_directory: request.current_dir.as_deref(),
                retain_parent_stdin: piped[0],
                null_stdio: piped.map(|piped| !piped),
                ..WindowsJobLaunchOptions::default()
            },
        ) {
            Ok(owner) => owner,
            Err(failure) => {
                return Err(LaunchError::Spawn(io::Error::other(
                    reservation.launch_failure(failure, resources),
                )));
            }
        };
        let stop = match control
            .map(|control| control.admit(&owner, &request.read_file_grants, None))
            .transpose()
        {
            Ok(stop) => stop,
            Err(error) => {
                return Err(LaunchError::Spawn(io::Error::other(
                    reservation.independent_failure(owner, error.to_string(), resources),
                )));
            }
        };
        let identity = owner.identity();
        let streams = owner.take_command_files(piped);
        let mut process = Self {
            owner: Arc::new(Mutex::new(owner)),
            streams,
            stop,
            deadline,
            bounded_cleanup_attempted: false,
            exact_identity: None,
            timer_cancel: None,
            timer: None,
            expired: Arc::new(AtomicBool::new(false)),
            identity,
            reservation: Some(reservation),
            resources,
            io: super::bounded_cleanup::HelperIo::unstarted(),
        };
        // The complete owner is installed before fallible timer creation.
        // Timer and extracted stdio share in-process Job ownership only.
        if let Some(deadline) = deadline {
            let (cancel, receiver) = mpsc::channel();
            let owner = process.owner.clone();
            let expired = process.expired.clone();
            match std::thread::Builder::new()
                .name("command-deadline".into())
                .spawn(move || {
                    if receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                        == Err(mpsc::RecvTimeoutError::Timeout)
                    {
                        expired.store(true, Ordering::SeqCst);
                        let cleanup_deadline = Instant::now() + JOB_HARD_STOP_TIMEOUT;
                        let result = super::bounded_cleanup::lock_native_owner_until(
                            &owner,
                            Some(cleanup_deadline),
                        )
                        .and_then(|mut owner| owner.hard_stop_until(cleanup_deadline));
                        if let Err(error) = result {
                            eprintln!("command deadline cleanup incomplete: {error}");
                        }
                    }
                }) {
                Ok(timer) => {
                    process.timer_cancel = Some(cancel);
                    process.timer = Some(timer);
                }
                Err(error) => {
                    let failure = process
                        .retain_cleanup(error.to_string(), Instant::now() + JOB_HARD_STOP_TIMEOUT);
                    return Err(LaunchError::Spawn(io::Error::other(failure)));
                }
            }
        }
        Ok(process)
    }

    fn owner_until(
        &self,
        deadline: Option<Instant>,
    ) -> io::Result<std::sync::MutexGuard<'_, WindowsJobProcess>> {
        super::bounded_cleanup::lock_native_owner_until(&self.owner, deadline)
    }

    fn retain_cleanup(
        &mut self,
        detail: String,
        deadline: Instant,
    ) -> super::bounded_cleanup::BoundedHelperFailure {
        self.timer_cancel.take();
        if let Some(timer) = self.timer.take() {
            self.io.workers.push(timer);
        }
        // Close unextracted pipe ends before waiting. Extracted files carry no
        // native Job authority and remain under their caller's I/O contract.
        self.streams = [None, None, None];
        if !self.bounded_cleanup_attempted {
            if let Ok(mut owner) = self.owner_until(Some(deadline)) {
                if owner.is_quiescent().ok() != Some(true) && Instant::now() < deadline {
                    let _ = owner.hard_stop_until(deadline);
                }
            }
        }
        self.io.drain_until(deadline);
        self.reservation
            .take()
            .expect("command reservation transferred once")
            .shared_failure(
                self.owner.clone(),
                self.identity,
                std::mem::replace(&mut self.io, super::bounded_cleanup::HelperIo::unstarted()),
                std::mem::take(&mut self.resources),
                detail,
            )
    }

    pub(super) fn pid(&self) -> u32 {
        self.identity.pid
    }

    pub(super) fn exact_identity(&self) -> Option<crate::process::LaunchedProcessIdentity> {
        self.exact_identity
    }

    pub(super) fn bind_exact_identity(
        &mut self,
        identity: crate::process::LaunchedProcessIdentity,
    ) -> io::Result<()> {
        if identity.instance != self.identity {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "command process identity mismatch",
            ));
        }
        self.exact_identity = Some(identity);
        Ok(())
    }

    pub(super) fn take_stream(&mut self, index: usize) -> Option<File> {
        self.streams[index].take()
    }

    pub(super) fn poll(&mut self) -> io::Result<Option<i32>> {
        if self.expired.load(Ordering::SeqCst)
            || self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.bounded_cleanup_attempted = true;
            let cleanup_deadline = Instant::now() + JOB_HARD_STOP_TIMEOUT;
            self.owner_until(Some(cleanup_deadline))?
                .hard_stop_until(cleanup_deadline)?;
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "command deadline elapsed; owned Job reaped",
            ));
        }
        let mut owner = match self.owner_until(None) {
            Ok(owner) => owner,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(None),
            Err(error) => return Err(error),
        };
        if let Some(code) = owner.poll()? {
            if !owner.is_quiescent()? {
                owner.hard_stop_until(Instant::now() + JOB_HARD_STOP_TIMEOUT)?;
            }
            // Cancel the deadline only after the complete tree has exited.
            drop(owner);
            self.timer_cancel.take();
            return Ok(Some(code));
        }
        Ok(None)
    }

    pub(super) fn wait(&mut self) -> io::Result<i32> {
        loop {
            if let Some(code) = self.poll()? {
                return Ok(code);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub(super) fn terminate_until(&mut self, deadline: Instant) -> io::Result<()> {
        self.bounded_cleanup_attempted = true;
        if self.owner_until(Some(deadline))?.is_quiescent()? {
            return Ok(());
        }
        if let Some(stop) = &self.stop {
            signal_stop(stop)?;
            let grace = deadline
                .saturating_duration_since(Instant::now())
                .saturating_sub(JOB_HARD_STOP_TIMEOUT);
            let grace_deadline = self.deadline.map_or(Instant::now() + grace, |runtime| {
                runtime.min(Instant::now() + grace)
            });
            while Instant::now() < grace_deadline {
                if self.owner_until(Some(deadline))?.is_quiescent()? {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        self.owner_until(Some(deadline))?
            .hard_stop_until(deadline)
            .map(|_| ())
    }

    pub(super) fn cleanup(&mut self) {
        let _ = self.cleanup_until(Instant::now() + crate::process::DRAIN_JOIN_TIMEOUT);
    }

    pub(super) fn cleanup_until(&mut self, deadline: Instant) -> bool {
        // Root exit alone cannot cancel the timer or release launch resources.
        // Include owner-lock contention in the caller's one cleanup deadline.
        let quiescent = self
            .owner_until(Some(deadline))
            .and_then(|owner| owner.is_quiescent())
            .ok()
            == Some(true);
        if !quiescent {
            return false;
        }
        self.timer_cancel.take();
        if let Some(timer) = self.timer.take() {
            self.io.workers.push(timer);
        }
        self.io.drain_until(deadline);
        if !self.io.settled() {
            return false;
        }
        // Only unextracted streams are ours. Extracted Files remain with callers.
        self.streams = [None, None, None];
        self.resources = Default::default();
        true
    }

    pub(super) fn detach_after_bounded_shutdown(&mut self) {
        self.bounded_cleanup_attempted = true;
    }

    pub(super) fn output(mut self) -> Result<Output, LaunchError> {
        self.streams[0].take();
        let (sender, receiver) = mpsc::channel();
        let receiver = Arc::new(Mutex::new(receiver));
        self.resources.retain(receiver.clone());
        for index in [1, 2] {
            let sender = sender.clone();
            let file = self.streams[index].take();
            match std::thread::Builder::new()
                .name("command-output".into())
                .spawn(move || {
                    let mut bytes = Vec::new();
                    let result = match file {
                        Some(mut file) => file.read_to_end(&mut bytes).map(|_| bytes),
                        None => Ok(bytes),
                    };
                    // File has closed before completion is published. The actual
                    // thread handle additionally proves all closure state settled.
                    let _ = sender.send((index, result));
                }) {
                Ok(worker) => self.io.workers.push(worker),
                Err(error) => return Err(self.output_failure(error)),
            }
        }
        drop(sender);
        let code = match self.wait() {
            Ok(code) => code,
            Err(error) => return Err(self.output_failure(error)),
        };
        let drain_deadline = Instant::now() + crate::process::DRAIN_JOIN_TIMEOUT;
        let mut streams = [Vec::new(), Vec::new()];
        for _ in 0..2 {
            let received = receiver
                .lock()
                .expect("command capture receiver has one consumer")
                .recv_timeout(drain_deadline.saturating_duration_since(Instant::now()));
            match received {
                Ok((index, Ok(bytes))) => streams[index - 1] = bytes,
                Ok((_, Err(error))) => return Err(self.output_failure(error)),
                Err(_) => {
                    return Err(self.output_failure(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "command output drain incomplete",
                    )));
                }
            }
        }
        self.io.drain_until(drain_deadline);
        if !self.io.settled() {
            return Err(self.output_failure(io::Error::new(
                io::ErrorKind::TimedOut,
                "command output workers have not settled",
            )));
        }
        let [stdout, stderr] = streams;
        Ok(Output {
            status: ExitStatus::from_raw(code as u32),
            stdout,
            stderr,
        })
    }

    fn output_failure(&mut self, error: io::Error) -> LaunchError {
        let deadline = if self.bounded_cleanup_attempted {
            Instant::now()
        } else {
            Instant::now() + JOB_HARD_STOP_TIMEOUT
        };
        LaunchError::Terminate(io::Error::other(
            self.retain_cleanup(error.to_string(), deadline),
        ))
    }
}

impl Drop for CommandProcess {
    fn drop(&mut self) {
        if self.reservation.is_none() {
            return;
        }
        let deadline = if self.bounded_cleanup_attempted {
            Instant::now()
        } else {
            Instant::now() + JOB_HARD_STOP_TIMEOUT
        };
        let _ = self.retain_cleanup(
            "command owner dropped before cleanup completed".into(),
            deadline,
        );
    }
}
