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
use super::launch_control::{AdmittedWindowsLaunch, LaunchControl, signal_stop};
use crate::process::SERVICE_SHUTDOWN_TIMEOUT;

/// Run a native sibling with the current standard handles and retain its Job
/// until the entire owned tree is quiescent. An admitted CLI hop reissues its
/// exact provenance, grants and latched stop through a fresh launch transaction.
/// The returned status is the child's status; admission/cleanup failure is Err.
pub fn forward_windows_native_command(
    program: &OsStr,
    arguments: &[OsString],
    admitted: Option<&AdmittedWindowsLaunch>,
) -> io::Result<i32> {
    let mut command = Vec::with_capacity(arguments.len() + 1);
    command.push(program.to_owned());
    command.extend_from_slice(arguments);
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
    let mut resources = BoundedHelperResources::new();
    if let Some(incoming) = admitted {
        resources.retain(std::sync::Arc::new(incoming.read_file_grants().to_vec()));
    }
    let reservation = Reservation::track_until(Instant::now() + JOB_HARD_STOP_TIMEOUT)
        .map_err(io::Error::other)?;
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
