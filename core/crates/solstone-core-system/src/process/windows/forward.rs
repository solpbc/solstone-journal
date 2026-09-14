// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The journal CLI's Windows process-replacement boundary.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io;
use std::time::{Duration, Instant};

use super::bounded::BoundedHelperResources;
use super::bounded_cleanup::Reservation;
use super::job_process::{JOB_HARD_STOP_TIMEOUT, launch_windows_forwarder};
use super::launch_control::{
    AdmittedWindowsLaunch, InstalledTaskLaunchRequest, LaunchControl, signal_stop,
};
use crate::process::SERVICE_SHUTDOWN_TIMEOUT;

/// Console session-end events (logoff, shutdown, console close) reach every
/// console process in the session, this forwarder included. Without a handler
/// the default one exits immediately, which closes the kill-on-close Job and
/// terminates the resident before it can clear its readiness and identity.
/// This handler instead latches a stop request for the forwarding loop and
/// never returns, exactly as tokio's Windows handler does for these three
/// events, so the system's own session-end grace is spent draining the tree.
mod session_end {
    use std::io;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use windows_sys::Win32::System::Console::{
        CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT, SetConsoleCtrlHandler,
    };

    static REQUESTED: AtomicBool = AtomicBool::new(false);
    static INSTALLED: OnceLock<io::Result<()>> = OnceLock::new();

    #[allow(unsafe_code)]
    unsafe extern "system" fn handler(control_type: u32) -> windows_sys::core::BOOL {
        match control_type {
            CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => {
                REQUESTED.store(true, Ordering::SeqCst);
                // Returning hands the process to the system for termination.
                // The forwarding loop finishes the tree and exits the process.
                loop {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            _ => 0,
        }
    }

    /// Install once per process; a later failure is reported, never retried.
    pub(super) fn install() -> io::Result<()> {
        INSTALLED
            .get_or_init(|| {
                // SAFETY: `handler` is a valid `HandlerRoutine` for the life of
                // the process and is never removed.
                #[allow(unsafe_code)]
                if unsafe { SetConsoleCtrlHandler(Some(handler), 1) } == 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            })
            .as_ref()
            .map(|()| ())
            .map_err(|error| io::Error::new(error.kind(), error.to_string()))
    }

    pub(super) fn requested() -> bool {
        REQUESTED.load(Ordering::SeqCst)
    }
}

/// Run a native sibling with the current standard handles and retain its Job
/// until the entire owned tree is quiescent. An admitted CLI hop reissues its
/// exact provenance, grants and latched stop through a fresh launch transaction.
/// The returned status is the child's status; admission/cleanup failure is Err.
pub fn forward_windows_native_command(
    program: &OsStr,
    arguments: &[OsString],
    admitted: Option<&AdmittedWindowsLaunch>,
) -> io::Result<i32> {
    let mut environment = BTreeMap::new();
    let control = admitted
        .map(|incoming| {
            let mut nonce = [0_u8; 24];
            getrandom::fill(&mut nonce).map_err(|error| io::Error::other(error.to_string()))?;
            let launch_id = nonce
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let mut provenance = incoming.child_launch_provenance(launch_id);
            // Forward the same service command through a new process identity.
            // Ordinary children use child_launch_provenance's service=None instead.
            provenance.service = incoming.service();
            LaunchControl::prepare(&provenance, &mut environment)
        })
        .transpose()?;
    forward(program, arguments, admitted, control, environment)
}

/// Forward the exact installed action with no hosted generation grants.
pub fn forward_windows_installed_task(
    program: &OsStr,
    request: &InstalledTaskLaunchRequest,
) -> io::Result<i32> {
    let mut environment = BTreeMap::new();
    let control = LaunchControl::prepare_installed(request, &mut environment)?;
    let arguments = request
        .arguments
        .iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
    forward(program, &arguments, None, Some(control), environment)
}

fn forward(
    program: &OsStr,
    arguments: &[OsString],
    admitted: Option<&AdmittedWindowsLaunch>,
    control: Option<LaunchControl>,
    environment: BTreeMap<OsString, OsString>,
) -> io::Result<i32> {
    let mut command = Vec::with_capacity(arguments.len() + 1);
    command.push(program.to_owned());
    command.extend_from_slice(arguments);
    let mut resources = BoundedHelperResources::new();
    if let Some(incoming) = admitted {
        resources.retain(std::sync::Arc::new(incoming.read_file_grants().to_vec()));
    }
    let reservation = Reservation::track_until(Instant::now() + JOB_HARD_STOP_TIMEOUT)
        .map_err(io::Error::other)?;
    // Best effort: a forwarder that cannot register still forwards; it merely
    // keeps the pre-existing immediate-exit behavior at session end.
    let _ = session_end::install();
    let mut owner = match launch_windows_forwarder(&command, &environment) {
        Ok(owner) => owner,
        Err(failure) => {
            return Err(io::Error::other(
                reservation.launch_failure(failure, resources),
            ));
        }
    };
    let outcome = (|| {
        let stop = match (control, admitted) {
            (Some(control), Some(incoming)) => {
                Some(control.admit(&owner, incoming.read_file_grants(), Some(incoming))?)
            }
            (Some(control), None) => Some(control.admit(&owner, &[], None)?),
            (None, None) => None,
            _ => return Err(io::Error::other("inconsistent forwarder admission")),
        };
        let mut stop_deadline = None;
        loop {
            if let Some(code) = owner.poll()? {
                // A root exit does not release the caller's grants while a
                // descendant still owns this generation or writes its output.
                if !owner.is_quiescent()? {
                    owner.hard_stop_until(Instant::now() + JOB_HARD_STOP_TIMEOUT)?;
                }
                return Ok(code);
            }
            if let Some(incoming) = admitted {
                if incoming.stop_requested()? && stop_deadline.is_none() {
                    signal_stop(
                        stop.as_ref()
                            .ok_or_else(|| io::Error::other("missing forwarding stop"))?,
                    )?;
                    stop_deadline = Some(Instant::now() + SERVICE_SHUTDOWN_TIMEOUT);
                }
            }
            if session_end::requested() && stop_deadline.is_none() {
                // Session end: ask the child to stop, then bound the drain.
                // An installed resident also receives the same console event
                // itself and clears its lifecycle artifacts on the way out.
                if let Some(stop) = stop.as_ref() {
                    signal_stop(stop)?;
                }
                stop_deadline = Some(Instant::now() + SERVICE_SHUTDOWN_TIMEOUT);
            }
            if stop_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return owner.hard_stop_until(Instant::now() + JOB_HARD_STOP_TIMEOUT);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    })();
    match outcome {
        Ok(code) => Ok(code),
        Err(error) => Err(io::Error::other(reservation.independent_failure(
            owner,
            error.to_string(),
            resources,
        ))),
    }
}
