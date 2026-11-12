// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::super::{
    BoxedTerminateFn, CommandLaunchRequest, Disposition, HostedLaunchProvenance, InspectResult,
    LaunchError, LaunchedProcessIdentity, ManagedLaunchRequest, ProcessBirth, ProcessInstance,
    ProcessInstanceSource, ProcessTreeSnapshot, SERVICE_SHUTDOWN_TIMEOUT, SpawnError,
    SystemProcessInstanceSource, TerminationEvidence, TerminationOutcome,
    require_managed_process_capability,
};
#[cfg(any(test, feature = "test-hooks"))]
use super::super::{HostedAdmissionTestFault, hosted_admission_test_fault};
use super::spawn::{ManagedProcess, discharge_if_reaped, remove_launch_only_environment};
use crate::lifecycle::{
    AdmissionIdentity, AdmissionIntent, AdmissionResult, AdmissionResultState, AdmissionTicket,
    Discharger, HOSTED_GENERATION_ENV, HOSTED_LAUNCH_ID_ENV, HOSTED_LAUNCHER_PID_ENV,
    HOSTED_PARENT_LAUNCH_ID_ENV, ParentLossAdmissionError, ParentLossGeneration, ParentLossLedger,
    ParentLossPhase, discharge_holding_lock, discharge_replaced_service_roots,
    read_parent_loss_admission_acknowledgement, write_parent_loss_admission_intent,
    write_parent_loss_admission_result, write_parent_loss_admission_spawn_identity,
};
use solstone_core_journal_io::{FileLock, LockOptions, hold_lock};

#[allow(clippy::large_enum_variant)]
pub(crate) enum GenerationChildError {
    /// No live child. Includes pre-spawn Admission (ledger open, admission lock,
    /// "launch rejected after seal", "missing active generation") and
    /// SpawnError::ExactInstanceUnavailable (root killed and its reap confirmed).
    Unavailable(LaunchError),
    /// This value owns the live authority, including a child whose reap could
    /// not be confirmed. Nothing has terminated it.
    Live(LaunchAuthority, LaunchError),
}

#[cfg(any(test, feature = "test-hooks"))]
type BoxedScriptedTerminateFn = Box<dyn FnMut(Duration) -> Result<(), LaunchError> + Send>;

#[allow(clippy::large_enum_variant)]
enum Inner {
    #[cfg(any(test, feature = "test-hooks"))]
    Scripted {
        pid: u32,
        poll_fn: Box<dyn FnMut() -> io::Result<Option<i32>> + Send>,
        terminate_fn: BoxedScriptedTerminateFn,
    },
    Managed(ManagedProcess),
    Raw {
        child: Child,
        terminate_fn: BoxedTerminateFn,
        exact_identity: Option<LaunchedProcessIdentity>,
        admission: Option<AdmissionTicket>,
    },
}

/// Retained termination authority for one launched child.
pub struct LaunchAuthority {
    inner: Option<Inner>,
    disposition: Disposition,
    resources: Vec<std::sync::Arc<dyn Send + Sync>>,
}

impl fmt::Debug for LaunchAuthority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("LaunchAuthority");
        if self.inner.is_some() {
            debug.field("pid", &self.pid());
        }
        debug.field("disposition", &self.disposition).finish()
    }
}

impl LaunchAuthority {
    pub(crate) fn retain_resource<T: Send + Sync + 'static>(
        &mut self,
        resource: std::sync::Arc<T>,
    ) {
        self.resources.push(resource);
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn scripted(
        pid: u32,
        poll_fn: impl FnMut() -> io::Result<Option<i32>> + Send + 'static,
        terminate_fn: impl FnMut(Duration) -> Result<(), LaunchError> + Send + 'static,
    ) -> Self {
        Self {
            inner: Some(Inner::Scripted {
                pid,
                poll_fn: Box::new(poll_fn),
                terminate_fn: Box::new(terminate_fn),
            }),
            disposition: Disposition::IndependentLongLived,
            resources: Vec::new(),
        }
    }

    pub(crate) fn from_managed(process: ManagedProcess, disposition: Disposition) -> Self {
        Self {
            inner: Some(Inner::Managed(process)),
            disposition,
            resources: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn without_process(disposition: Disposition) -> Self {
        Self {
            inner: None,
            disposition,
            resources: Vec::new(),
        }
    }

    pub fn pid(&self) -> u32 {
        match self.inner.as_ref() {
            Some(Inner::Managed(process)) => process.pid(),
            Some(Inner::Raw { child, .. }) => child.id(),
            #[cfg(any(test, feature = "test-hooks"))]
            Some(Inner::Scripted { pid, .. }) => *pid,
            None => 1,
        }
    }

    pub fn disposition(&self) -> &Disposition {
        &self.disposition
    }

    /// The PID/birth/UID sample retained by an exact managed launch.
    pub fn exact_identity(&self) -> Option<LaunchedProcessIdentity> {
        match self.inner.as_ref() {
            Some(Inner::Managed(process)) => process.exact_identity(),
            Some(Inner::Raw { exact_identity, .. }) => *exact_identity,
            #[cfg(any(test, feature = "test-hooks"))]
            Some(Inner::Scripted { .. }) => None,
            None => Some(LaunchedProcessIdentity {
                instance: ProcessInstance {
                    pid: 1,
                    birth: ProcessBirth::unknown(),
                },
                uid: nix::unistd::getuid().as_raw(),
            }),
        }
    }

    /// The birth-bound instance retained by the launch, without Unix credentials.
    pub fn process_instance(&self) -> Option<crate::process::ProcessInstance> {
        self.exact_identity().map(|identity| identity.instance)
    }

    /// Bind a raw authority to the exact identity sampled by its launch
    /// boundary.  An explicitly-unowned coordinator is sampled outside this
    /// module, then uses this binding for exact cleanup on bootstrap failure.
    pub fn bind_exact_identity(
        &mut self,
        identity: LaunchedProcessIdentity,
    ) -> Result<(), LaunchError> {
        match self.inner.as_mut() {
            Some(Inner::Raw { exact_identity, .. }) => {
                *exact_identity = Some(identity);
                Ok(())
            }
            Some(Inner::Managed(_)) => Err(LaunchError::CapabilityUnavailable {
                needed: "raw launch identity binding",
            }),
            #[cfg(any(test, feature = "test-hooks"))]
            Some(Inner::Scripted { .. }) => Ok(()),
            None => Ok(()),
        }
    }

    /// Bind this child to its admission entry, discharged once this owner
    /// observes the child reaped.
    pub(crate) fn set_admission_ticket(&mut self, ticket: AdmissionTicket) {
        match self.inner.as_mut() {
            Some(Inner::Managed(process)) => process.set_admission_ticket(ticket),
            Some(Inner::Raw { admission, .. }) => *admission = Some(ticket),
            #[cfg(any(test, feature = "test-hooks"))]
            Some(Inner::Scripted { .. }) => {}
            None => {}
        }
    }

    fn discharge_raw_if_reaped(&mut self) {
        if let Some(Inner::Raw {
            child, admission, ..
        }) = self.inner.as_mut()
        {
            discharge_if_reaped(child, admission);
        }
    }

    pub fn poll(&mut self) -> io::Result<Option<i32>> {
        let status = self.poll_inner();
        self.discharge_raw_if_reaped();
        status
    }

    fn poll_inner(&mut self) -> io::Result<Option<i32>> {
        match self.inner.as_mut() {
            Some(Inner::Managed(process)) => process.poll(),
            Some(Inner::Raw { child, .. }) => child
                .try_wait()
                .map(|status| status.map(|value| super::super::signal_aware_exit_code(&value))),
            #[cfg(any(test, feature = "test-hooks"))]
            Some(Inner::Scripted { poll_fn, .. }) => poll_fn(),
            None => Ok(Some(-1)),
        }
    }

    pub fn wait(&mut self) -> io::Result<i32> {
        let status = self.wait_inner();
        self.discharge_raw_if_reaped();
        status
    }

    fn wait_inner(&mut self) -> io::Result<i32> {
        match self.inner.as_mut() {
            Some(Inner::Managed(process)) => process.wait(),
            Some(Inner::Raw { child, .. }) => child
                .wait()
                .map(|status| super::super::signal_aware_exit_code(&status)),
            #[cfg(any(test, feature = "test-hooks"))]
            Some(Inner::Scripted { poll_fn, .. }) => loop {
                if let Some(code) = poll_fn()? {
                    return Ok(code);
                }
                thread::sleep(Duration::from_millis(10));
            },
            None => Ok(-1),
        }
    }

    pub fn terminate(&mut self, timeout: Duration) -> Result<(), LaunchError> {
        let result = self.terminate_inner(timeout);
        self.discharge_raw_if_reaped();
        result
    }

    fn terminate_inner(&mut self, timeout: Duration) -> Result<(), LaunchError> {
        match self.inner.as_mut() {
            Some(Inner::Managed(process)) => process
                .terminate(timeout)
                .map(|_| ())
                .map_err(|error| LaunchError::Terminate(io::Error::other(error))),
            Some(Inner::Raw {
                child,
                terminate_fn,
                ..
            }) => {
                let signaled = terminate_fn(child, timeout);
                let waited = child.wait().map(|_| ()).map_err(LaunchError::Terminate);
                match (signaled, waited) {
                    (Ok(()), Ok(())) => Ok(()),
                    (Err(error), _) => Err(error),
                    (Ok(()), Err(error)) => Err(error),
                }
            }
            #[cfg(any(test, feature = "test-hooks"))]
            Some(Inner::Scripted { terminate_fn, .. }) => terminate_fn(timeout),
            None => Ok(()),
        }
    }

    pub fn terminate_exact(&mut self, timeout: Duration) -> Result<(), LaunchError> {
        #[cfg(any(test, feature = "test-hooks"))]
        if let Some(Inner::Scripted { terminate_fn, .. }) = self.inner.as_mut() {
            return terminate_fn(timeout);
        }
        let evidence = self.terminate_exact_evidence(timeout);
        evidence
            .result
            .map(|_| ())
            .map_err(|error| LaunchError::Terminate(io::Error::other(error)))
    }

    pub(crate) fn terminate_exact_evidence(&mut self, timeout: Duration) -> TerminationEvidence {
        let evidence = self.terminate_exact_evidence_inner(timeout);
        self.discharge_raw_if_reaped();
        evidence
    }

    fn terminate_exact_evidence_inner(&mut self, timeout: Duration) -> TerminationEvidence {
        match self.inner.as_mut() {
            Some(Inner::Managed(process)) => process.terminate_exact_evidence(timeout),
            Some(Inner::Raw {
                child,
                exact_identity: Some(identity),
                ..
            }) => {
                let source = SystemProcessInstanceSource;
                super::terminate::terminate_exact_instance_with_snapshot(
                    child,
                    identity.instance,
                    timeout,
                    &source,
                )
            }
            Some(Inner::Raw { .. }) => TerminationEvidence {
                result: Err(super::super::TerminationError::ExactInstanceUnavailable),
                snapshot: None,
            },
            #[cfg(any(test, feature = "test-hooks"))]
            Some(Inner::Scripted { .. }) => TerminationEvidence {
                result: Err(super::super::TerminationError::ExactInstanceUnavailable),
                snapshot: None,
            },
            None => TerminationEvidence {
                result: Ok(TerminationOutcome::Graceful {
                    exit_code: Some(-1),
                }),
                snapshot: None,
            },
        }
    }

    /// Terminate a managed child without opening a wait beyond `deadline`.
    #[allow(dead_code)]
    pub(crate) fn terminate_exact_until(&mut self, deadline: Instant) -> Result<(), LaunchError> {
        #[cfg(any(test, feature = "test-hooks"))]
        if let Some(Inner::Scripted { terminate_fn, .. }) = self.inner.as_mut() {
            return terminate_fn(Duration::ZERO);
        }
        let evidence = self.terminate_exact_until_evidence(deadline);
        evidence
            .result
            .map(|_| ())
            .map_err(|error| LaunchError::Terminate(io::Error::other(error)))
    }

    pub(crate) fn terminate_exact_until_evidence(
        &mut self,
        deadline: Instant,
    ) -> TerminationEvidence {
        let evidence = self.terminate_exact_until_evidence_inner(deadline);
        self.discharge_raw_if_reaped();
        evidence
    }

    fn terminate_exact_until_evidence_inner(&mut self, deadline: Instant) -> TerminationEvidence {
        match self.inner.as_mut() {
            Some(Inner::Managed(process)) => process.terminate_exact_until_evidence(deadline),
            Some(Inner::Raw {
                child,
                exact_identity: Some(identity),
                ..
            }) => {
                let source = SystemProcessInstanceSource;
                super::terminate::terminate_exact_instance_until_with_snapshot(
                    child,
                    identity.instance,
                    deadline,
                    &source,
                )
            }
            Some(Inner::Raw { .. }) => TerminationEvidence {
                result: Err(super::super::TerminationError::ExactInstanceUnavailable),
                snapshot: None,
            },
            #[cfg(any(test, feature = "test-hooks"))]
            Some(Inner::Scripted { .. }) => TerminationEvidence {
                result: Err(super::super::TerminationError::ExactInstanceUnavailable),
                snapshot: None,
            },
            None => TerminationEvidence {
                result: Ok(TerminationOutcome::Graceful {
                    exit_code: Some(-1),
                }),
                snapshot: None,
            },
        }
    }

    pub(crate) fn last_termination_snapshot(&self) -> Option<&ProcessTreeSnapshot> {
        match self.inner.as_ref() {
            Some(Inner::Managed(process)) => process.last_termination_snapshot(),
            _ => None,
        }
    }

    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        match self.inner.as_mut() {
            #[cfg(any(test, feature = "test-hooks"))]
            Some(Inner::Scripted { .. }) => None,
            Some(Inner::Managed(_)) | None => None,
            Some(Inner::Raw { child, .. }) => child.stdin.take(),
        }
    }

    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        match self.inner.as_mut() {
            #[cfg(any(test, feature = "test-hooks"))]
            Some(Inner::Scripted { .. }) => None,
            Some(Inner::Managed(_)) | None => None,
            Some(Inner::Raw { child, .. }) => child.stdout.take(),
        }
    }

    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        match self.inner.as_mut() {
            #[cfg(any(test, feature = "test-hooks"))]
            Some(Inner::Scripted { .. }) => None,
            Some(Inner::Managed(_)) | None => None,
            Some(Inner::Raw { child, .. }) => child.stderr.take(),
        }
    }

    pub fn wait_with_output(mut self) -> Result<Output, LaunchError> {
        match self.inner.take() {
            Some(Inner::Managed(process)) => {
                drop(process);
                Err(LaunchError::OutputUnavailable)
            }
            Some(Inner::Raw {
                child, admission, ..
            }) => {
                let output = child.wait_with_output().map_err(LaunchError::Terminate)?;
                // The output's status is the reap.
                if let Some(ticket) = admission {
                    let _ = ticket.discharge_reaped();
                }
                Ok(output)
            }
            #[cfg(any(test, feature = "test-hooks"))]
            Some(Inner::Scripted { .. }) => Err(LaunchError::OutputUnavailable),
            None => Err(LaunchError::OutputUnavailable),
        }
    }

    pub fn cleanup(&mut self) {
        if let Some(Inner::Managed(process)) = self.inner.as_mut() {
            process.cleanup();
        }
    }

    /// Consume the authority without terminating its child.  This is limited
    /// to the documented raw coordinator escape hatch; all ordinary raw
    /// authorities retain Drop-based termination.
    pub fn relinquish_explicitly_unowned(mut self) -> Result<(), LaunchError> {
        if !matches!(self.disposition, Disposition::ExplicitlyUnowned { .. }) {
            return Err(LaunchError::NotExplicitlyUnowned);
        }
        let _ = self.inner.take();
        Ok(())
    }

    /// Consume a managed authority after the admission boundary has completed.
    /// Raw children intentionally cannot escape through this conversion.
    pub fn into_managed(mut self) -> Result<ManagedProcess, LaunchError> {
        match self.inner.take() {
            Some(Inner::Managed(process)) => Ok(process),
            Some(raw @ Inner::Raw { .. }) => {
                self.inner = Some(raw);
                Err(LaunchError::OutputUnavailable)
            }
            #[cfg(any(test, feature = "test-hooks"))]
            Some(scripted @ Inner::Scripted { .. }) => {
                self.inner = Some(scripted);
                Err(LaunchError::OutputUnavailable)
            }
            None => Err(LaunchError::OutputUnavailable),
        }
    }

    /// Clean up a managed child only while the caller's shared deadline remains.
    pub(crate) fn cleanup_until(&mut self, deadline: Instant) -> bool {
        match self.inner_mut() {
            Inner::Managed(process) => process.cleanup_until(deadline),
            Inner::Raw {
                child, admission, ..
            } => {
                discharge_if_reaped(child, admission);
                child.try_wait().ok().flatten().is_some()
            }
            #[cfg(any(test, feature = "test-hooks"))]
            Inner::Scripted { poll_fn, .. } => poll_fn().ok().flatten().is_some(),
        }
    }

    /// Mark a managed authority so drop cannot begin a new service-length
    /// termination window after a bounded shutdown expires.
    pub(crate) fn detach_after_bounded_shutdown(&mut self) {
        match self.inner.as_mut() {
            Some(Inner::Managed(process)) => process.detach_after_bounded_shutdown(),
            // A detached child is never discharged.
            Some(Inner::Raw { admission, .. }) => *admission = None,
            _ => {}
        }
    }

    #[allow(dead_code)]
    fn inner(&self) -> &Inner {
        self.inner.as_ref().expect("launch authority inner")
    }

    fn inner_mut(&mut self) -> &mut Inner {
        self.inner.as_mut().expect("launch authority inner")
    }
}

impl Drop for LaunchAuthority {
    fn drop(&mut self) {
        let Some(Inner::Raw {
            mut child,
            mut terminate_fn,
            mut admission,
            ..
        }) = self.inner.take()
        else {
            return;
        };
        if child.try_wait().ok().flatten().is_none() {
            let _ = terminate_fn(&mut child, SERVICE_SHUTDOWN_TIMEOUT);
            let _ = child.wait();
        }
        discharge_if_reaped(&mut child, &mut admission);
    }
}

/// Launch a raw child after capability and post-spawn confirmation checks.
pub fn launch<F>(
    disposition: Disposition,
    spawn: F,
    terminate_fn: BoxedTerminateFn,
) -> Result<LaunchAuthority, LaunchError>
where
    F: FnOnce() -> io::Result<Child>,
{
    launch_with(
        disposition,
        spawn,
        terminate_fn,
        production_capability_probe,
        production_confirm,
    )
}

/// Construct and launch a raw command entirely inside the authority boundary.
pub fn launch_command(
    disposition: Disposition,
    request: CommandLaunchRequest,
    terminate_fn: BoxedTerminateFn,
) -> Result<LaunchAuthority, LaunchError> {
    launch(disposition, move || spawn_command(request), terminate_fn)
}

/// Raw-command counterpart to [`launch_managed_hosted`]. This is required for
/// descendants that exchange data over stdio, such as Cortex talent workers.
pub fn launch_command_hosted(
    disposition: Disposition,
    mut request: CommandLaunchRequest,
    provenance: HostedLaunchProvenance,
    terminate_fn: BoxedTerminateFn,
) -> Result<LaunchAuthority, LaunchError> {
    let admitted = admit_under_lock(
        &provenance.journal,
        Some(provenance.generation),
        provenance.acknowledgement_timeout,
    )?;
    write_launch_intent(&admitted, &hosted_intent(&provenance))?;
    inject_hosted_provenance(&mut request.environment, &provenance);
    let mut authority = match launch(disposition, move || spawn_command(request), terminate_fn) {
        Ok(authority) => authority,
        Err(err) => {
            record_spawn_failed(&admitted, &provenance.launch_id, &err, left_no_child(&err))?;
            return Err(err);
        }
    };
    let source = SystemProcessInstanceSource;
    let identity = match source.inspect(authority.pid()) {
        InspectResult::Present { instance, uid, .. } => LaunchedProcessIdentity { instance, uid },
        InspectResult::Absent | InspectResult::Unverifiable => {
            let detail = match authority.terminate(Duration::from_secs(2)) {
                Ok(()) => "exact launch identity unavailable".to_owned(),
                Err(error) => error.to_string(),
            };
            record_unidentified_reject(&mut authority, &admitted, &provenance.launch_id, detail)?;
            return Err(LaunchError::Admission(
                "exact launch identity unavailable".to_owned(),
            ));
        }
    };
    authority.bind_exact_identity(identity)?;
    finish_hosted_admission(&mut authority, identity, &provenance, Some(&admitted))?;
    authority.set_admission_ticket(admitted.ticket(&provenance.launch_id));
    Ok(authority)
}

/// Wrap an already-atomic `ManagedProcess::spawn` after the same pre-spawn checks.
pub fn launch_managed<F>(disposition: Disposition, spawn: F) -> Result<LaunchAuthority, LaunchError>
where
    F: FnOnce() -> Result<ManagedProcess, SpawnError>,
{
    launch_managed_with(disposition, spawn, production_capability_probe)
}

/// Launch one exact managed process from declarative inputs.
pub fn launch_managed_request(
    disposition: Disposition,
    request: ManagedLaunchRequest,
) -> Result<LaunchAuthority, LaunchError> {
    launch_managed(disposition, move || {
        ManagedProcess::spawn_exact(request.command, request.options)
    })
}

/// [`launch_managed_request`] that hands back a child the exact spawn could
/// not prove reaped. `Err((None, _))` means no process is left.
#[allow(clippy::result_large_err)]
fn launch_managed_keeping(
    disposition: Disposition,
    request: ManagedLaunchRequest,
) -> Result<LaunchAuthority, (Option<LaunchAuthority>, LaunchError)> {
    reject_empty_unowned_reason(&disposition).map_err(|error| (None, error))?;
    production_capability_probe(&disposition).map_err(|error| (None, error))?;
    match ManagedProcess::spawn_exact_keeping(request.command, request.options) {
        Ok(process) => Ok(LaunchAuthority::from_managed(process, disposition)),
        Err((process, error)) => Err((
            process.map(|process| LaunchAuthority::from_managed(process, disposition)),
            LaunchError::SpawnManaged(error),
        )),
    }
}

/// The non-bypassable hosted child boundary.  It writes an immutable intent
/// before spawn, captures exact PID/birth/UID, waits for the child-owned
/// acknowledgement, and exact-reaps a child if admission cannot complete.
pub fn launch_managed_hosted(
    disposition: Disposition,
    mut request: ManagedLaunchRequest,
    provenance: HostedLaunchProvenance,
) -> Result<LaunchAuthority, LaunchError> {
    let admitted = admit_under_lock(
        &provenance.journal,
        Some(provenance.generation),
        provenance.acknowledgement_timeout,
    )?;
    write_launch_intent(&admitted, &hosted_intent(&provenance))?;
    inject_hosted_provenance(&mut request.options.environment, &provenance);
    let mut authority = match launch_managed_keeping(disposition, request) {
        Ok(authority) => authority,
        Err((child, err)) => {
            record_spawn_failed(&admitted, &provenance.launch_id, &err, child.is_none())?;
            drop(child);
            return Err(err);
        }
    };
    if let Some(identity) = authority.exact_identity() {
        finish_hosted_admission(&mut authority, identity, &provenance, Some(&admitted))?;
        authority.set_admission_ticket(admitted.ticket(&provenance.launch_id));
        return Ok(authority);
    }
    match authority.poll() {
        Ok(Some(_)) => {
            record_unidentified_reject(
                &mut authority,
                &admitted,
                &provenance.launch_id,
                String::new(),
            )?;
            Ok(authority)
        }
        Ok(None) => {
            let detail = match authority.terminate_exact(Duration::from_secs(2)) {
                Ok(()) => "exact launch identity unavailable".to_owned(),
                Err(err) => err.to_string(),
            };
            record_unidentified_reject(&mut authority, &admitted, &provenance.launch_id, detail)?;
            Err(LaunchError::Admission(
                "exact launch identity unavailable".to_owned(),
            ))
        }
        Err(error) => {
            record_unidentified_admission_result(
                &provenance.journal,
                provenance.generation,
                &provenance.launch_id,
                AdmissionResultState::RejectedUnreaped {
                    detail: error.to_string(),
                },
            )?;
            Err(LaunchError::Admission(format!(
                "failed to poll early-exited child: {error}"
            )))
        }
    }
}

/// A queue task worker or other generation child. With `inject_provenance`
/// (a journal binary), the child gets the hosted environment and records
/// its own identity at process entry, before any work.
#[allow(clippy::result_large_err)]
pub(crate) fn launch_managed_generation_child(
    disposition: Disposition,
    journal: &Path,
    launch_id: String,
    mut request: ManagedLaunchRequest,
    inject_provenance: bool,
) -> Result<LaunchAuthority, GenerationChildError> {
    let admitted = admit_under_lock(journal, None, GENERATION_CHILD_LOCK_TIMEOUT)
        .map_err(GenerationChildError::Unavailable)?;
    let intent = AdmissionIntent::new(admitted.generation, launch_id.clone(), None, None);
    write_launch_intent(&admitted, &intent).map_err(GenerationChildError::Unavailable)?;
    if inject_provenance {
        inject_hosted_environment(
            &mut request.options.environment,
            journal,
            admitted.generation,
            &launch_id,
            None,
        );
    }

    let mut authority = match launch_managed_keeping(disposition, request) {
        Ok(authority) => authority,
        Err((Some(mut authority), err)) => {
            authority.set_admission_ticket(admitted.ticket(&launch_id));
            return Err(GenerationChildError::Live(authority, err));
        }
        Err((None, err)) => {
            let _ = record_spawn_failed(&admitted, &launch_id, &err, true);
            return Err(GenerationChildError::Unavailable(err));
        }
    };

    match finish_generation_child_admission(&mut authority, &admitted, &launch_id, false) {
        Ok(admitted_child) => {
            if admitted_child {
                authority.set_admission_ticket(admitted.ticket(&launch_id));
            }
            Ok(authority)
        }
        Err(err) => {
            authority.set_admission_ticket(admitted.ticket(&launch_id));
            Err(GenerationChildError::Live(authority, err))
        }
    }
}

/// Record the exact identity of a generation child. Returns `false` when the
/// child was already reaped inside the launch, in which case its entry has
/// been discharged and there is nothing left to reap.
fn finish_generation_child_admission(
    authority: &mut LaunchAuthority,
    admitted: &Admitted,
    launch_id: &str,
    retire: bool,
) -> Result<bool, LaunchError> {
    let journal = admitted.journal.as_path();
    let generation = admitted.generation;
    let source = SystemProcessInstanceSource;
    let identity = match authority.exact_identity() {
        Some(identity) => identity,
        None => match source.inspect(authority.pid()) {
            InspectResult::Present { instance, uid, .. } => {
                let id = LaunchedProcessIdentity { instance, uid };
                authority.bind_exact_identity(id)?;
                id
            }
            InspectResult::Absent | InspectResult::Unverifiable => match authority.poll() {
                Ok(Some(exit_code)) => {
                    record_unidentified_admission_result(
                        journal,
                        generation,
                        launch_id,
                        AdmissionResultState::RejectedAndReaped {
                            exit_code: Some(exit_code),
                        },
                    )?;
                    admitted.discharge(launch_id);
                    return Ok(false);
                }
                Ok(None) => {
                    if retire {
                        let _ = authority.terminate_exact(Duration::from_secs(2));
                    }
                    return Err(LaunchError::Admission(
                        "exact launch identity unavailable".to_owned(),
                    ));
                }
                Err(error) => {
                    if retire {
                        let _ = authority.terminate_exact(Duration::from_secs(2));
                    }
                    return Err(LaunchError::Admission(format!(
                        "failed to poll child without an exact identity: {error}"
                    )));
                }
            },
        },
    };

    let expected = AdmissionIdentity {
        generation,
        launch_id: launch_id.to_owned(),
        instance: identity.instance,
        uid: identity.uid,
        parent_launch_id: None,
    };
    write_parent_loss_admission_spawn_identity(journal, &expected)
        .map_err(|error| LaunchError::Admission(error.to_string()))?;

    let result = AdmissionResult {
        schema: 1,
        identity: Some(expected),
        state: AdmissionResultState::Admitted,
    };
    write_parent_loss_admission_result(journal, generation, launch_id, &result)
        .map_err(|error| LaunchError::Admission(error.to_string()))?;

    Ok(true)
}

pub fn launch_generation_child<F>(
    disposition: Disposition,
    journal: &Path,
    launch_id: String,
    spawn: F,
    terminate_fn: BoxedTerminateFn,
) -> Result<LaunchAuthority, LaunchError>
where
    F: FnOnce() -> io::Result<Child>,
{
    let admitted = admit_under_lock(journal, None, GENERATION_CHILD_LOCK_TIMEOUT)?;
    let intent = AdmissionIntent::new(admitted.generation, launch_id.clone(), None, None);
    write_launch_intent(&admitted, &intent)?;

    let mut authority = match launch(disposition, spawn, terminate_fn) {
        Ok(authority) => authority,
        Err(err) => {
            record_spawn_failed(&admitted, &launch_id, &err, left_no_child(&err))?;
            return Err(err);
        }
    };

    if finish_generation_child_admission(&mut authority, &admitted, &launch_id, true)? {
        authority.set_admission_ticket(admitted.ticket(&launch_id));
    }
    Ok(authority)
}

const GENERATION_CHILD_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// One launch's hold on its generation's admission lock, taken before the
/// pointer was read and found naming this generation `Admitting`. The seal
/// takes the same lock, so nothing this launch writes can follow the seal.
struct Admitted {
    journal: PathBuf,
    ledger: ParentLossLedger,
    generation: ParentLossGeneration,
    lock: FileLock,
}

impl Admitted {
    fn ticket(&self, launch_id: &str) -> AdmissionTicket {
        AdmissionTicket {
            journal: self.journal.clone(),
            generation: self.generation,
            launch_id: launch_id.to_owned(),
        }
    }

    /// Discharge an entry with no process left to reap, under the held lock.
    fn discharge(&self, launch_id: &str) {
        let _ = discharge_holding_lock(
            &self.ledger,
            self.generation,
            launch_id,
            Discharger::Launcher,
            &self.lock,
        );
    }
}

/// The launch gate shared by every Unix launch path: take the admission
/// lock, then read the pointer, then require `generation` (the active one
/// when `None`) to be active and `Admitting`.
fn admit_under_lock(
    journal: &Path,
    generation: Option<ParentLossGeneration>,
    timeout: Duration,
) -> Result<Admitted, LaunchError> {
    let refuse = |detail: String| LaunchError::Admission(detail);
    let ledger = ParentLossLedger::open(journal).map_err(|error| refuse(error.to_string()))?;
    let read_pointer = || {
        ledger
            .active_generation()
            .map_err(|error| refuse(error.to_string()))?
            .ok_or_else(|| refuse("missing active generation".to_owned()))
    };
    let generation = match generation {
        Some(generation) => generation,
        None => read_pointer()?.generation,
    };
    let lock = hold_lock(
        ledger.admission_lock_path(generation),
        LockOptions {
            timeout,
            poll_interval: Duration::from_millis(10),
            mode: Some(0o600),
        },
    )
    .map_err(|error| refuse(error.to_string()))?;
    let active = read_pointer()?;
    if active.generation != generation || active.phase != ParentLossPhase::Admitting {
        return Err(refuse("launch rejected after seal".to_owned()));
    }
    Ok(Admitted {
        journal: journal.to_path_buf(),
        ledger,
        generation,
        lock,
    })
}

fn hosted_intent(provenance: &HostedLaunchProvenance) -> AdmissionIntent {
    AdmissionIntent::new(
        provenance.generation,
        provenance.launch_id.clone(),
        provenance.service,
        provenance.parent_launch_id.clone(),
    )
}

/// Write a launch's intent. A service launch then discharges the roots it
/// replaces, still under the same lock.
fn write_launch_intent(admitted: &Admitted, intent: &AdmissionIntent) -> Result<(), LaunchError> {
    write_parent_loss_admission_intent(&admitted.journal, intent)
        .map_err(|error| LaunchError::Admission(error.to_string()))?;
    if let Some(service) = intent.service {
        discharge_replaced_service_roots(
            &admitted.ledger,
            admitted.generation,
            &intent.launch_id,
            service,
            &SystemProcessInstanceSource,
            &admitted.lock,
        );
    }
    Ok(())
}

/// Whether a failed raw launch left no child behind. A confirmation failure
/// spawned one and did not prove its reap.
fn left_no_child(error: &LaunchError) -> bool {
    !matches!(error, LaunchError::ConfirmationFailed { .. })
}

/// Record a launch that failed at spawn, discharging it when no child is left.
fn record_spawn_failed(
    admitted: &Admitted,
    launch_id: &str,
    error: &LaunchError,
    no_child: bool,
) -> Result<(), LaunchError> {
    record_unidentified_admission_result(
        &admitted.journal,
        admitted.generation,
        launch_id,
        AdmissionResultState::SpawnFailed {
            detail: error.to_string(),
        },
    )?;
    if no_child {
        admitted.discharge(launch_id);
    }
    Ok(())
}

/// Record an unidentified child the launch rejected. Only a confirmed reap
/// status records `RejectedAndReaped` and discharges the entry.
fn record_unidentified_reject(
    authority: &mut LaunchAuthority,
    admitted: &Admitted,
    launch_id: &str,
    detail: String,
) -> Result<(), LaunchError> {
    let (state, reaped) = match authority.poll() {
        Ok(Some(exit_code)) => (
            AdmissionResultState::RejectedAndReaped {
                exit_code: Some(exit_code),
            },
            true,
        ),
        Ok(None) => (AdmissionResultState::RejectedUnreaped { detail }, false),
        Err(error) => (
            AdmissionResultState::RejectedUnreaped {
                detail: error.to_string(),
            },
            false,
        ),
    };
    record_unidentified_admission_result(&admitted.journal, admitted.generation, launch_id, state)?;
    if reaped {
        admitted.discharge(launch_id);
    }
    Ok(())
}

fn record_unidentified_admission_result(
    journal: &Path,
    generation: ParentLossGeneration,
    launch_id: &str,
    state: AdmissionResultState,
) -> Result<(), LaunchError> {
    let result = AdmissionResult {
        schema: 1,
        identity: None,
        state,
    };
    write_parent_loss_admission_result(journal, generation, launch_id, &result)
        .map_err(|error| LaunchError::Admission(error.to_string()))
}

fn inject_hosted_provenance(
    environment: &mut BTreeMap<OsString, OsString>,
    provenance: &HostedLaunchProvenance,
) {
    inject_hosted_environment(
        environment,
        &provenance.journal,
        provenance.generation,
        &provenance.launch_id,
        provenance.parent_launch_id.as_deref(),
    );
}

fn inject_hosted_environment(
    environment: &mut BTreeMap<OsString, OsString>,
    journal: &Path,
    generation: ParentLossGeneration,
    launch_id: &str,
    parent_launch_id: Option<&str>,
) {
    environment.insert(
        OsString::from(HOSTED_GENERATION_ENV),
        OsString::from(generation.to_string()),
    );
    environment.insert(
        OsString::from(HOSTED_LAUNCH_ID_ENV),
        OsString::from(launch_id),
    );
    environment.insert(
        OsString::from("SOLSTONE_JOURNAL"),
        journal.as_os_str().to_os_string(),
    );
    // The child checks it still belongs to this process before working.
    environment.insert(
        OsString::from(HOSTED_LAUNCHER_PID_ENV),
        OsString::from(std::process::id().to_string()),
    );
    if let Some(parent_launch_id) = parent_launch_id {
        environment.insert(
            OsString::from(HOSTED_PARENT_LAUNCH_ID_ENV),
            OsString::from(parent_launch_id),
        );
    }
}

fn finish_hosted_admission(
    authority: &mut LaunchAuthority,
    identity: LaunchedProcessIdentity,
    provenance: &HostedLaunchProvenance,
    admitted: Option<&Admitted>,
) -> Result<(), LaunchError> {
    let expected = AdmissionIdentity {
        generation: provenance.generation,
        launch_id: provenance.launch_id.clone(),
        instance: identity.instance,
        uid: identity.uid,
        parent_launch_id: provenance.parent_launch_id.clone(),
    };
    write_parent_loss_admission_spawn_identity(&provenance.journal, &expected)
        .map_err(|error| LaunchError::Admission(error.to_string()))?;
    let deadline = Instant::now() + provenance.acknowledgement_timeout;
    loop {
        if Instant::now() >= deadline {
            break;
        }
        match read_parent_loss_admission_acknowledgement(
            &provenance.journal,
            provenance.generation,
            &provenance.launch_id,
        ) {
            Ok(Some(ack)) if ack.identity == expected => {
                let result = AdmissionResult {
                    schema: 1,
                    identity: Some(expected),
                    state: AdmissionResultState::Admitted,
                };
                write_parent_loss_admission_result(
                    &provenance.journal,
                    provenance.generation,
                    &provenance.launch_id,
                    &result,
                )
                .map_err(|error| LaunchError::Admission(error.to_string()))?;
                return Ok(());
            }
            Ok(Some(_)) => break,
            Ok(None) | Err(ParentLossAdmissionError::Json(_)) => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => break,
        }
    }
    let (result_state, reaped) = match terminate_rejected_hosted_child(authority) {
        Ok(exit_code) => (
            AdmissionResultState::RejectedAndReaped { exit_code },
            exit_code.is_some(),
        ),
        Err(error) => (
            AdmissionResultState::RejectedUnreaped {
                detail: error.to_string(),
            },
            false,
        ),
    };
    let result = AdmissionResult {
        schema: 1,
        identity: Some(expected),
        state: result_state,
    };
    let recorded = write_parent_loss_admission_result(
        &provenance.journal,
        provenance.generation,
        &provenance.launch_id,
        &result,
    );
    if let (Ok(()), true, Some(admitted)) = (recorded, reaped, admitted) {
        admitted.discharge(&provenance.launch_id);
    }
    Err(LaunchError::Admission(
        "hosted child did not complete matching admission acknowledgement".to_owned(),
    ))
}

fn terminate_rejected_hosted_child(
    authority: &mut LaunchAuthority,
) -> Result<Option<i32>, LaunchError> {
    #[cfg(any(test, feature = "test-hooks"))]
    if hosted_admission_test_fault() == Some(HostedAdmissionTestFault::ExactReap) {
        return Err(LaunchError::Admission(
            "test hook forced exact reap failure".to_owned(),
        ));
    }
    authority.terminate_exact(Duration::from_secs(2))?;
    #[cfg(any(test, feature = "test-hooks"))]
    if hosted_admission_test_fault() == Some(HostedAdmissionTestFault::ExitProof) {
        return Err(LaunchError::Admission(
            "test hook forced exact exit-proof failure".to_owned(),
        ));
    }
    authority.poll().map_err(LaunchError::Terminate)
}

fn spawn_command(request: CommandLaunchRequest) -> io::Result<Child> {
    let mut command = Command::new(request.program);
    remove_launch_only_environment(&mut command);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    solstone_core_local_endpoint::prepare_child(&mut command, &request.environment);
    command.args(request.arguments).envs(request.environment);
    if let Some(current_dir) = request.current_dir {
        command.current_dir(current_dir);
    }
    command.stdin(if request.stdin_piped {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    command.stdout(if request.stdout_piped {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    command.stderr(if request.stderr_piped {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    if request.process_group {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
    }
    command.spawn()
}

pub fn launch_with<F, Cap, Conf>(
    disposition: Disposition,
    spawn: F,
    mut terminate_fn: BoxedTerminateFn,
    capability: Cap,
    confirm: Conf,
) -> Result<LaunchAuthority, LaunchError>
where
    F: FnOnce() -> io::Result<Child>,
    Cap: FnOnce(&Disposition) -> Result<(), LaunchError>,
    Conf: FnOnce(u32) -> io::Result<()>,
{
    reject_empty_unowned_reason(&disposition)?;
    capability(&disposition)?;
    let mut child = spawn().map_err(LaunchError::Spawn)?;
    if claims_independent_scope(&disposition) {
        let pid = child.id();
        if let Err(source) = confirm(pid) {
            let _ = terminate_fn(&mut child, SERVICE_SHUTDOWN_TIMEOUT);
            let _ = child.wait();
            return Err(LaunchError::ConfirmationFailed { pid, source });
        }
    }
    Ok(LaunchAuthority {
        inner: Some(Inner::Raw {
            child,
            terminate_fn,
            exact_identity: None,
            admission: None,
        }),
        disposition,
        resources: Vec::new(),
    })
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
    reject_empty_unowned_reason(&disposition)?;
    capability(&disposition)?;
    let process = spawn().map_err(LaunchError::SpawnManaged)?;
    Ok(LaunchAuthority {
        inner: Some(Inner::Managed(process)),
        disposition,
        resources: Vec::new(),
    })
}

fn reject_empty_unowned_reason(disposition: &Disposition) -> Result<(), LaunchError> {
    match disposition {
        Disposition::ExplicitlyUnowned { reason } if reason.trim().is_empty() => {
            Err(LaunchError::EmptyUnownedReason)
        }
        _ => Ok(()),
    }
}

const fn claims_independent_scope(disposition: &Disposition) -> bool {
    matches!(
        disposition,
        Disposition::IndependentLongLived | Disposition::IndependentBoundedHelper { .. }
    )
}

fn production_capability_probe(disposition: &Disposition) -> Result<(), LaunchError> {
    let _ = disposition;
    require_managed_process_capability()
        .map_err(|needed| LaunchError::CapabilityUnavailable { needed })
}

fn production_confirm(pid: u32) -> io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let pid = i32::try_from(pid).map_err(|_| io::Error::other("invalid child pid"))?;
        nix::unistd::getpgid(Some(nix::unistd::Pid::from_raw(pid)))
            .map(|_| ())
            .map_err(io::Error::other)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "process groups unavailable on this platform",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;
    #[cfg(all(test, feature = "full-tests"))]
    use std::fs::{File, OpenOptions};
    #[cfg(all(test, feature = "full-tests"))]
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::super::super::SpawnOptions;
    #[cfg(all(test, feature = "full-tests"))]
    use crate::lifecycle::{
        AdmissionAcknowledgement, AdmissionResult, AdmissionResultState, ParentLossPhase,
        acknowledge_parent_loss_admission, write_parent_loss_admission_result,
    };
    use crate::lifecycle::{HostedServiceKind, ParentLossLedger};
    #[cfg(all(test, feature = "full-tests"))]
    use crate::process::InstanceVerdict;
    use crate::process::{ProcessBirth, ProcessInstance};
    #[cfg(all(test, feature = "full-tests"))]
    use std::ffi::OsString;

    #[cfg(all(test, feature = "full-tests"))]
    fn process_is_gone(pid: u32) -> bool {
        let Ok(pid) = i32::try_from(pid) else {
            return true;
        };
        matches!(
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
            Err(nix::errno::Errno::ESRCH)
        )
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn wait_until_gone(pid: u32) {
        for _ in 0..200 {
            if process_is_gone(pid) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("pid {pid} still alive");
    }

    fn panic_if_spawned() -> io::Result<Child> {
        panic!("spawn must not run");
    }

    fn kill_child(child: &mut Child, _: Duration) -> Result<(), LaunchError> {
        child.kill().map_err(LaunchError::Terminate)
    }

    fn unwrap_launch_err<T>(result: Result<T, LaunchError>, what: &str) -> LaunchError {
        match result {
            Err(error) => error,
            Ok(_) => panic!("{what}: expected error"),
        }
    }

    fn instance(pid: u32, birth: u64) -> ProcessInstance {
        ProcessInstance {
            pid,
            birth: ProcessBirth::linux(birth, 1, 100),
        }
    }

    fn admitting_generation(root: &std::path::Path) -> (ParentLossLedger, u64, ProcessInstance) {
        let ledger = ParentLossLedger::open(root).expect("parent-loss ledger");
        let active = ledger
            .reserve_generation(instance(10, 1), [HostedServiceKind::Sense])
            .expect("generation reservation");
        ledger
            .initialize_record(&active)
            .expect("generation record");
        let coordinator = instance(20, 2);
        ledger
            .persist_coordinator_identity(active.generation, coordinator)
            .expect("coordinator identity");
        ledger
            .mark_admitting(active.generation, coordinator)
            .expect("admitting generation");
        (ledger, active.generation, coordinator)
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn admission_fixture(
        name: &str,
        launch_id: &str,
        timeout: Duration,
    ) -> (
        JournalBed,
        ParentLossLedger,
        HostedLaunchProvenance,
        LaunchAuthority,
        LaunchedProcessIdentity,
    ) {
        let bed = JournalBed::new(name);
        let (ledger, generation, _) = admitting_generation(&bed.root);
        let provenance = HostedLaunchProvenance {
            journal: bed.root.clone(),
            generation,
            launch_id: launch_id.to_owned(),
            service: None,
            parent_launch_id: Some("parent-service".to_owned()),
            acknowledgement_timeout: timeout,
        };
        let authority = launch_managed_request(
            Disposition::InheritedParentScope,
            ManagedLaunchRequest {
                command: vec!["/bin/sleep".to_owned(), "60".to_owned()],
                options: SpawnOptions {
                    journal_root: bed.root.clone(),
                    reference: launch_id.to_owned(),
                    day: None,
                    sink: None,
                    environment: BTreeMap::new(),
                },
            },
        )
        .expect("launch managed child");
        let identity = authority.exact_identity().expect("exact child identity");
        assert_eq!(authority.process_instance(), Some(identity.instance));
        (bed, ledger, provenance, authority, identity)
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn partial_acknowledgement(
        ledger: &ParentLossLedger,
        generation: ParentLossGeneration,
        launch_id: &str,
    ) -> (File, PathBuf) {
        let directory = ledger
            .generation_path(generation)
            .join("admissions")
            .join(launch_id);
        fs::create_dir_all(&directory).expect("acknowledgement directory");
        let path = directory.join("acknowledgement.json");
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("partial acknowledgement");
        (file, path)
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn complete_acknowledgement(mut file: File, identity: AdmissionIdentity) {
        let acknowledgement = AdmissionAcknowledgement {
            schema: 1,
            identity,
        };
        let mut bytes =
            serde_json::to_vec_pretty(&acknowledgement).expect("serialize acknowledgement");
        bytes.push(b'\n');
        file.write_all(&bytes).expect("write acknowledgement");
        file.sync_all().expect("sync acknowledgement");
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn wait_for_file(path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !path.exists() {
            assert!(Instant::now() < deadline, "timed out waiting for {path:?}");
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn admission_result(
        ledger: &ParentLossLedger,
        generation: ParentLossGeneration,
        launch_id: &str,
    ) -> AdmissionResult {
        let path = ledger
            .generation_path(generation)
            .join("admissions")
            .join(launch_id)
            .join("result.json");
        serde_json::from_slice(&fs::read(path).expect("admission result")).expect("result JSON")
    }

    struct JournalBed {
        root: PathBuf,
    }

    impl JournalBed {
        fn new(name: &str) -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let root = PathBuf::from("/var/tmp").join(format!("solstone-authority-{name}-{stamp}"));
            fs::create_dir_all(&root).expect("temporary journal");
            Self { root }
        }
    }

    impl Drop for JournalBed {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn outstanding(ledger: &ParentLossLedger, generation: ParentLossGeneration) -> Vec<String> {
        let Ok(listing) = fs::read_dir(ledger.generation_path(generation).join("admissions"))
        else {
            return Vec::new();
        };
        let mut names: Vec<String> = listing
            .map(|item| {
                item.expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    /// This test binary re-run as a child: the probe test below does the
    /// child's part when `PROBE_ENV` names a mode.
    #[cfg(all(test, feature = "full-tests"))]
    pub(crate) const PROBE_ENV: &str = "SOLSTONE_TEST_ADMISSION_PROBE";
    #[cfg(all(test, feature = "full-tests"))]
    pub(crate) const PROBE_REPORT_ENV: &str = "SOLSTONE_TEST_ADMISSION_PROBE_REPORT";
    #[cfg(all(test, feature = "full-tests"))]
    const PROBE_TEST: &str = "process::platform::authority::tests::admission_probe_child";

    #[cfg(all(test, feature = "full-tests"))]
    pub(crate) fn probe_arguments() -> Vec<String> {
        ["--exact", PROBE_TEST, "--nocapture", "--test-threads=1"]
            .map(str::to_owned)
            .to_vec()
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn probe_command(mode: &str) -> (Vec<String>, BTreeMap<OsString, OsString>) {
        let mut command = vec![
            std::env::current_exe()
                .expect("test binary")
                .display()
                .to_string(),
        ];
        command.extend(probe_arguments());
        let environment = BTreeMap::from([(OsString::from(PROBE_ENV), OsString::from(mode))]);
        (command, environment)
    }

    /// The child side of the spawn tests. A plain test run does nothing.
    /// `ack` records the child's own admission, as every journal binary does
    /// at process entry; `exec` reaches `ack` through a pid-preserving exec;
    /// `worker` also reports what it and its own children see.
    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn admission_probe_child() {
        let Some(mode) = std::env::var_os(PROBE_ENV) else {
            return;
        };
        std::process::exit(run_admission_probe(mode.to_str().unwrap_or_default()));
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn run_admission_probe(mode: &str) -> i32 {
        use std::os::unix::process::CommandExt;
        if mode == "exec" {
            let error = Command::new(std::env::current_exe().expect("test binary"))
                .args(probe_arguments())
                .env(PROBE_ENV, "ack")
                .exec();
            eprintln!("probe exec failed: {error}");
            return 71;
        }
        let Some(journal) = std::env::var_os("SOLSTONE_JOURNAL").map(PathBuf::from) else {
            return 72;
        };
        if mode == "orphan" || mode == "attached" {
            // Acknowledge only once the launcher has exited and this process
            // was reparented ("orphan"), or at once ("attached"), and report.
            let launcher: Option<i32> = std::env::var(HOSTED_LAUNCHER_PID_ENV)
                .ok()
                .and_then(|pid| pid.parse().ok());
            let until = Instant::now() + Duration::from_secs(10);
            while mode == "orphan"
                && Some(nix::unistd::getppid().as_raw()) == launcher
                && Instant::now() < until
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            let outcome = match crate::lifecycle::acknowledge_hosted_child_admission(&journal) {
                Ok(()) => "worked".to_owned(),
                Err(error) => format!("stopped: {error}"),
            };
            let written = std::env::var_os(PROBE_REPORT_ENV).map(|path| fs::write(path, outcome));
            return if matches!(written, Some(Ok(()))) {
                0
            } else {
                73
            };
        }
        if let Err(error) = crate::lifecycle::acknowledge_hosted_child_admission(&journal) {
            eprintln!("probe admission failed: {error}");
            return 70;
        }
        if mode != "worker" {
            return 0;
        }
        // Work starts here: the acknowledgement must already be durable.
        let generation: u64 = std::env::var(HOSTED_GENERATION_ENV)
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or_default();
        let launch_id = std::env::var(HOSTED_LAUNCH_ID_ENV).unwrap_or_default();
        let acked = read_parent_loss_admission_acknowledgement(&journal, generation, &launch_id)
            .ok()
            .flatten()
            .is_some_and(|ack| ack.identity.instance.pid == std::process::id());
        let count = |text: &str| {
            text.lines()
                .filter(|line| line.starts_with("SOL_PARENT_LOSS_"))
                .count()
        };
        let raw = launch_command(
            Disposition::InheritedParentScope,
            CommandLaunchRequest {
                program: OsString::from("/usr/bin/env"),
                arguments: Vec::new(),
                environment: BTreeMap::new(),
                current_dir: None,
                process_group: false,
                stdin_piped: false,
                stdout_piped: true,
                stderr_piped: false,
            },
            Box::new(|child, _| child.kill().map_err(LaunchError::Terminate)),
        )
        .and_then(LaunchAuthority::wait_with_output)
        .map(|output| count(&String::from_utf8_lossy(&output.stdout)));
        let managed_file = journal.join("probe-managed-env.txt");
        let managed = ManagedProcess::spawn(
            vec![
                "/bin/sh".to_owned(),
                "-c".to_owned(),
                format!("env > '{}'", managed_file.display()),
            ],
            super::super::super::SpawnOptions {
                journal_root: journal.clone(),
                reference: "probe-grandchild".to_owned(),
                day: None,
                sink: None,
                environment: BTreeMap::new(),
            },
        )
        .and_then(|mut process| process.wait().map_err(SpawnError::Spawn))
        .ok()
        .and_then(|_| fs::read_to_string(&managed_file).ok())
        .map(|text| count(&text));
        let report = format!(
            "own={}\nacked={}\nraw={}\nmanaged={}\n",
            u8::from(!launch_id.is_empty() && generation != 0),
            u8::from(acked),
            raw.map_or_else(|_| "error".to_owned(), |n| n.to_string()),
            managed.map_or_else(|| "error".to_owned(), |n| n.to_string()),
        );
        let written = std::env::var_os(PROBE_REPORT_ENV).map(|path| fs::write(path, report));
        if matches!(written, Some(Ok(()))) {
            0
        } else {
            73
        }
    }

    #[test]
    fn a_thousand_spawn_failures_leave_admissions_empty() {
        let bed = JournalBed::new("spawn-failures");
        let (ledger, generation, _) = admitting_generation(&bed.root);
        for index in 0..1_000 {
            let error = launch_generation_child(
                Disposition::InheritedParentScope,
                &bed.root,
                format!("fails-{index}"),
                || Err(io::Error::from(io::ErrorKind::NotFound)),
                Box::new(kill_child),
            )
            .expect_err("spawn fails");
            assert!(matches!(error, LaunchError::Spawn(_)));
        }
        for index in 0..10 {
            let result = launch_managed_generation_child(
                Disposition::IndependentBoundedHelper {
                    timeout: Duration::from_secs(5),
                },
                &bed.root,
                format!("managed-fails-{index}"),
                ManagedLaunchRequest {
                    command: vec!["/definitely/not/a-binary".to_owned()],
                    options: SpawnOptions {
                        journal_root: bed.root.clone(),
                        reference: "managed-fails".to_owned(),
                        day: None,
                        sink: None,
                        environment: BTreeMap::new(),
                    },
                },
                false,
            );
            assert!(matches!(result, Err(GenerationChildError::Unavailable(_))));
        }
        assert_eq!(outstanding(&ledger, generation), Vec::<String>::new());
        let discharged = fs::read_dir(ledger.generation_path(generation).join("discharged"))
            .expect("discharged")
            .count();
        assert_eq!(discharged, 1_010);
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn hosted(
        bed: &JournalBed,
        generation: u64,
        launch_id: &str,
        service: Option<HostedServiceKind>,
    ) -> HostedLaunchProvenance {
        HostedLaunchProvenance {
            journal: bed.root.clone(),
            generation,
            launch_id: launch_id.to_owned(),
            service,
            parent_launch_id: None,
            acknowledgement_timeout: Duration::from_secs(10),
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn managed_probe(bed: &JournalBed, mode: &str) -> ManagedLaunchRequest {
        let (command, environment) = probe_command(mode);
        ManagedLaunchRequest {
            command,
            options: SpawnOptions {
                journal_root: bed.root.clone(),
                reference: format!("probe-{mode}"),
                day: None,
                sink: None,
                environment,
            },
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn managed_command(bed: &JournalBed, command: &[&str]) -> ManagedLaunchRequest {
        ManagedLaunchRequest {
            command: command.iter().map(|part| (*part).to_owned()).collect(),
            options: SpawnOptions {
                journal_root: bed.root.clone(),
                reference: "plain".to_owned(),
                day: None,
                sink: None,
                environment: BTreeMap::new(),
            },
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn every_launch_path_discharges_the_child_it_reaps() {
        let bed = JournalBed::new("reap-discharges");
        let (ledger, generation, _) = admitting_generation(&bed.root);

        let mut raw = launch_generation_child(
            Disposition::InheritedParentScope,
            &bed.root,
            "raw-child".to_owned(),
            || Command::new("/usr/bin/true").spawn(),
            Box::new(kill_child),
        )
        .expect("raw generation child");
        assert_eq!(outstanding(&ledger, generation), ["raw-child"]);
        assert_eq!(raw.wait().expect("wait"), 0);
        assert_eq!(outstanding(&ledger, generation), Vec::<String>::new());

        let mut managed = launch_managed_generation_child(
            Disposition::IndependentBoundedHelper {
                timeout: Duration::from_secs(5),
            },
            &bed.root,
            "managed-child".to_owned(),
            managed_command(&bed, &["/usr/bin/true"]),
            false,
        )
        .unwrap_or_else(|_| panic!("managed generation child"));
        while managed.poll().expect("poll").is_none() {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(outstanding(&ledger, generation), Vec::<String>::new());

        let mut hosted_managed = launch_managed_hosted(
            Disposition::InheritedParentScope,
            managed_probe(&bed, "ack"),
            hosted(&bed, generation, "hosted-managed", None),
        )
        .expect("hosted managed child");
        assert_eq!(hosted_managed.wait().expect("wait"), 0);
        assert_eq!(outstanding(&ledger, generation), Vec::<String>::new());

        let (command, environment) = probe_command("ack");
        let hosted_raw = launch_command_hosted(
            Disposition::InheritedParentScope,
            CommandLaunchRequest {
                program: OsString::from(&command[0]),
                arguments: command[1..].iter().map(OsString::from).collect(),
                environment,
                current_dir: None,
                process_group: false,
                stdin_piped: false,
                stdout_piped: true,
                stderr_piped: true,
            },
            hosted(&bed, generation, "hosted-raw", None),
            Box::new(kill_child),
        )
        .expect("hosted raw child");
        assert!(
            hosted_raw
                .wait_with_output()
                .expect("output")
                .status
                .success()
        );
        assert_eq!(outstanding(&ledger, generation), Vec::<String>::new());

        let mut moved = launch_managed_hosted(
            Disposition::InheritedParentScope,
            managed_probe(&bed, "ack"),
            hosted(&bed, generation, "into-managed", None),
        )
        .expect("hosted child")
        .into_managed()
        .expect("managed process");
        assert_eq!(moved.wait().expect("wait"), 0);
        assert_eq!(outstanding(&ledger, generation), Vec::<String>::new());

        // A service root is the stop proof's roster: its reap never discharges it.
        let mut service = launch_managed_hosted(
            Disposition::InheritedParentScope,
            managed_probe(&bed, "ack"),
            hosted(
                &bed,
                generation,
                "service-root",
                Some(HostedServiceKind::Sense),
            ),
        )
        .expect("hosted service");
        assert_eq!(service.wait().expect("wait"), 0);
        drop(service);
        assert_eq!(outstanding(&ledger, generation), ["service-root"]);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn a_detached_child_is_never_discharged() {
        let bed = JournalBed::new("detached");
        let (ledger, generation, _) = admitting_generation(&bed.root);
        let mut authority = launch_managed_generation_child(
            Disposition::IndependentBoundedHelper {
                timeout: Duration::from_secs(5),
            },
            &bed.root,
            "detached-child".to_owned(),
            managed_command(&bed, &["/bin/sleep", "30"]),
            false,
        )
        .unwrap_or_else(|_| panic!("generation child"));
        authority.detach_after_bounded_shutdown();
        let pid = authority.pid();
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(i32::try_from(pid).expect("pid")),
            nix::sys::signal::Signal::SIGKILL,
        )
        .expect("kill");
        while authority.poll().expect("poll").is_none() {
            thread::sleep(Duration::from_millis(5));
        }
        authority.cleanup();
        drop(authority);
        assert_eq!(outstanding(&ledger, generation), ["detached-child"]);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn an_unconfirmed_reap_after_an_unverifiable_spawn_keeps_the_entry() {
        let bed = JournalBed::new("unconfirmed-reap");
        let (ledger, generation, _) = admitting_generation(&bed.root);
        let request = |wait_fails: bool| {
            let mut request = managed_command(&bed, &["/bin/sleep", "60"]);
            let environment = &mut request.options.environment;
            environment.insert(
                OsString::from("SOLSTONE_TEST_EXACT_SPAWN_FORCE_UNVERIFIABLE"),
                OsString::from("1"),
            );
            if wait_fails {
                environment.insert(
                    OsString::from("SOLSTONE_TEST_EXACT_SPAWN_FORCE_WAIT_FAILURE"),
                    OsString::from("1"),
                );
            }
            request
        };
        let disposition = || Disposition::IndependentBoundedHelper {
            timeout: Duration::from_secs(5),
        };

        // The child was killed and its reap confirmed: the launcher discharges it.
        let confirmed = launch_managed_generation_child(
            disposition(),
            &bed.root,
            "confirmed".to_owned(),
            request(false),
            false,
        );
        assert!(matches!(
            confirmed,
            Err(GenerationChildError::Unavailable(_))
        ));
        assert_eq!(outstanding(&ledger, generation), Vec::<String>::new());

        // The reap could not be confirmed: the entry stays, and the caller
        // keeps the child until its own reap discharges it.
        let unconfirmed = launch_managed_generation_child(
            disposition(),
            &bed.root,
            "unconfirmed".to_owned(),
            request(true),
            false,
        );
        let Err(GenerationChildError::Live(authority, _)) = unconfirmed else {
            panic!("an unconfirmed reap must hand the child back");
        };
        assert_eq!(outstanding(&ledger, generation), ["unconfirmed"]);
        drop(authority);
        assert_eq!(outstanding(&ledger, generation), Vec::<String>::new());
    }

    /// A task worker launched by a shell that stands in for its launcher. With
    /// `orphan`, the shell exits at once and the worker acknowledges after it
    /// was reparented; otherwise the shell waits for it. The worker reports
    /// whether it went on to work.
    #[cfg(all(test, feature = "full-tests"))]
    fn launcher_probe(name: &str, orphan: bool, skip_check: bool) -> String {
        let bed = JournalBed::new(name);
        let (_ledger, generation, _) = admitting_generation(&bed.root);
        let launch_id = format!("{name}-worker");
        write_parent_loss_admission_intent(
            &bed.root,
            &AdmissionIntent::new(generation, launch_id.clone(), None, None),
        )
        .expect("intent");
        let report = bed.root.join("launcher-report.txt");
        let script = if orphan {
            "SOL_PARENT_LOSS_LAUNCHER_PID=$$ \"$0\" \"$@\" &"
        } else {
            "SOL_PARENT_LOSS_LAUNCHER_PID=$$ \"$0\" \"$@\"; true"
        };
        let mut shell = Command::new("/bin/sh");
        shell
            .args(["-c", script])
            .arg(std::env::current_exe().expect("test binary"))
            .args(probe_arguments())
            .env(PROBE_ENV, if orphan { "orphan" } else { "attached" })
            .env(PROBE_REPORT_ENV, &report)
            .env("SOLSTONE_JOURNAL", &bed.root)
            .env(HOSTED_GENERATION_ENV, generation.to_string())
            .env(HOSTED_LAUNCH_ID_ENV, &launch_id)
            .stdout(std::process::Stdio::null());
        if skip_check {
            shell.env("SOLSTONE_TEST_SKIP_LAUNCHER_CHECK", "1");
        }
        assert!(shell.status().expect("launcher shell").success());
        let until = Instant::now() + Duration::from_secs(30);
        while !report.exists() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        fs::read_to_string(&report).expect("the worker reported")
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn a_worker_whose_launcher_died_before_it_acknowledged_stops_without_working() {
        let outcome = launcher_probe("launcher-gone", true, false);
        assert!(
            outcome.starts_with("stopped:") && outcome.contains("has exited"),
            "{outcome}"
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn removing_the_launcher_check_turns_the_orphan_test_red() {
        assert_eq!(launcher_probe("launcher-unchecked", true, true), "worked");
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn a_worker_whose_launcher_is_alive_proceeds() {
        assert_eq!(launcher_probe("launcher-alive", false, false), "worked");
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn a_hosted_service_reached_through_exec_still_admits() {
        let bed = JournalBed::new("exec-admits");
        let (ledger, generation, _) = admitting_generation(&bed.root);
        let mut authority = launch_managed_hosted(
            Disposition::InheritedParentScope,
            managed_probe(&bed, "exec"),
            hosted(
                &bed,
                generation,
                "exec-service",
                Some(HostedServiceKind::Sense),
            ),
        )
        .expect("an exec'd service acknowledges with the launch environment intact");
        let result = admission_result(&ledger, generation, "exec-service");
        assert_eq!(result.state, AdmissionResultState::Admitted);
        assert_eq!(
            result.identity.map(|identity| identity.instance.pid),
            Some(authority.pid())
        );
        assert_eq!(authority.wait().expect("wait"), 0);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn a_thousand_restarts_of_one_hosted_service_leave_one_root() {
        let bed = JournalBed::new("service-restarts");
        let (ledger, generation, _) = admitting_generation(&bed.root);
        let began = Instant::now();
        let mut last = String::new();
        for restart in 0..1_000 {
            last = format!("sense-{restart}");
            let mut authority = launch_managed_hosted(
                Disposition::InheritedParentScope,
                managed_probe(&bed, "ack"),
                hosted(&bed, generation, &last, Some(HostedServiceKind::Sense)),
            )
            .expect("service admitted");
            assert_eq!(authority.wait().expect("wait"), 0);
        }
        eprintln!("1,000 admitted service restarts took {:?}", began.elapsed());
        assert_eq!(outstanding(&ledger, generation), [last]);
    }

    #[test]
    fn ac1_four_dispositions_are_constructible_and_empty_unowned_reason_rejects_before_spawn() {
        let _ = Disposition::IndependentLongLived;
        let _ = Disposition::IndependentBoundedHelper {
            timeout: Duration::from_secs(1),
        };
        let _ = Disposition::InheritedParentScope;
        let _ = Disposition::ExplicitlyUnowned {
            reason: "host diagnostic".to_owned(),
        };

        for reason in ["", "   ", "\t\n"] {
            let spawned = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&spawned);
            let error = unwrap_launch_err(
                launch(
                    Disposition::ExplicitlyUnowned {
                        reason: reason.to_owned(),
                    },
                    move || {
                        flag.store(true, Ordering::SeqCst);
                        panic_if_spawned()
                    },
                    Box::new(kill_child),
                ),
                "empty unowned reason",
            );
            assert!(
                matches!(error, LaunchError::EmptyUnownedReason),
                "{error:?}"
            );
            assert!(!spawned.load(Ordering::SeqCst));

            let spawned = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&spawned);
            let error = unwrap_launch_err(
                launch_managed(
                    Disposition::ExplicitlyUnowned {
                        reason: reason.to_owned(),
                    },
                    move || {
                        flag.store(true, Ordering::SeqCst);
                        panic!("managed spawn must not run");
                    },
                ),
                "empty unowned reason",
            );
            assert!(
                matches!(error, LaunchError::EmptyUnownedReason),
                "{error:?}"
            );
            assert!(!spawned.load(Ordering::SeqCst));
        }
    }

    #[test]
    fn ac2_capability_failure_does_not_spawn_or_downgrade() {
        for disposition in [
            Disposition::IndependentLongLived,
            Disposition::IndependentBoundedHelper {
                timeout: Duration::from_secs(5),
            },
        ] {
            let spawned = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&spawned);
            let error = unwrap_launch_err(
                launch_with(
                    disposition,
                    move || {
                        flag.store(true, Ordering::SeqCst);
                        panic_if_spawned()
                    },
                    Box::new(kill_child),
                    |_| {
                        Err(LaunchError::CapabilityUnavailable {
                            needed: "process-groups",
                        })
                    },
                    |_| Ok(()),
                ),
                "capability",
            );
            assert!(
                matches!(
                    error,
                    LaunchError::CapabilityUnavailable {
                        needed: "process-groups"
                    }
                ),
                "{error:?}"
            );
            assert!(!spawned.load(Ordering::SeqCst));
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn launch_managed_delegates_pid_poll_and_terminate() {
        let bed = JournalBed::new("managed");
        let mut authority = launch_managed(Disposition::IndependentLongLived, || {
            ManagedProcess::spawn(
                vec!["/bin/sleep".to_owned(), "5".to_owned()],
                SpawnOptions {
                    journal_root: bed.root.clone(),
                    reference: "authority-managed".to_owned(),
                    day: None,
                    sink: None,
                    environment: BTreeMap::new(),
                },
            )
        })
        .expect("launch managed");
        let pid = authority.pid();
        assert!(pid > 1);
        assert!(matches!(authority.poll(), Ok(None)));
        authority
            .terminate(Duration::from_secs(2))
            .expect("terminate");
        wait_until_gone(pid);
        authority.cleanup();
    }

    #[test]
    fn seal_rejection_happens_before_spawn_or_service_work() {
        let bed = JournalBed::new("hosted-seal-rejection");
        let (ledger, generation, coordinator) = admitting_generation(&bed.root);
        ledger
            .seal(generation, coordinator)
            .expect("seal generation");
        let provenance = HostedLaunchProvenance {
            journal: bed.root.clone(),
            generation,
            launch_id: "sealed-launch".to_owned(),
            service: None,
            parent_launch_id: None,
            acknowledgement_timeout: Duration::from_millis(20),
        };
        let error = launch_managed_hosted(
            Disposition::InheritedParentScope,
            ManagedLaunchRequest {
                command: vec!["/definitely/not/a-child".to_owned()],
                options: SpawnOptions {
                    journal_root: bed.root.clone(),
                    reference: "sealed-launch".to_owned(),
                    day: None,
                    sink: None,
                    environment: BTreeMap::new(),
                },
            },
            provenance,
        )
        .expect_err("sealed generation rejects before invoking spawn");
        assert!(
            matches!(error, LaunchError::Admission(message) if message.contains("rejected after seal"))
        );
        assert!(
            !ledger
                .generation_path(generation)
                .join("admissions/sealed-launch/intent.json")
                .exists()
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn rejected_child_is_exactly_reaped_and_stale_ack_cannot_admit_it() {
        let bed = JournalBed::new("hosted-rejected-child");
        let (ledger, generation, _) = admitting_generation(&bed.root);
        let provenance = HostedLaunchProvenance {
            journal: bed.root.clone(),
            generation,
            launch_id: "unacknowledged-child".to_owned(),
            service: None,
            parent_launch_id: Some("parent-service".to_owned()),
            acknowledgement_timeout: Duration::from_millis(20),
        };
        let error = launch_managed_hosted(
            Disposition::InheritedParentScope,
            ManagedLaunchRequest {
                command: vec!["/bin/sleep".to_owned(), "60".to_owned()],
                options: SpawnOptions {
                    journal_root: bed.root.clone(),
                    reference: "unacknowledged-child".to_owned(),
                    day: None,
                    sink: None,
                    environment: BTreeMap::new(),
                },
            },
            provenance.clone(),
        )
        .expect_err("child without acknowledgement is rejected");
        assert!(matches!(error, LaunchError::Admission(_)));
        // A confirmed reap is discharged by the launcher.
        assert!(
            !ledger
                .generation_path(generation)
                .join("admissions/unacknowledged-child")
                .exists()
        );
        let result_path = ledger
            .generation_path(generation)
            .join("discharged/unacknowledged-child/result.json");
        let result: AdmissionResult =
            serde_json::from_slice(&std::fs::read(&result_path).expect("rejected child result"))
                .expect("result JSON");
        let identity = result.identity.clone().expect("fresh exact identity");
        assert!(matches!(
            result.state,
            AdmissionResultState::RejectedAndReaped { .. }
        ));
        assert!(matches!(
            SystemProcessInstanceSource.observe(&identity.instance),
            InstanceVerdict::NotSameOrExited
        ));

        // A late acknowledgement or result finds the entry discharged and
        // cannot recreate it.
        assert!(matches!(
            acknowledge_parent_loss_admission(&bed.root, identity.clone()),
            Err(crate::lifecycle::ParentLossAdmissionError::GenerationClosed)
        ));
        assert!(matches!(
            write_parent_loss_admission_result(
                &bed.root,
                generation,
                &provenance.launch_id,
                &AdmissionResult {
                    schema: 1,
                    identity: Some(identity),
                    state: AdmissionResultState::Admitted,
                },
            ),
            Err(crate::lifecycle::ParentLossAdmissionError::GenerationClosed)
        ));
        assert!(
            !ledger
                .generation_path(generation)
                .join("admissions/unacknowledged-child")
                .exists()
        );
        assert!(matches!(
            ledger
                .active_generation()
                .expect("active generation")
                .expect("active")
                .phase,
            ParentLossPhase::Admitting
        ));
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn incomplete_acknowledgement_completed_before_deadline_admits_child() {
        let launch_id = "partial-acknowledgement";
        let (_bed, ledger, provenance, mut authority, identity) = admission_fixture(
            "hosted-partial-acknowledgement",
            launch_id,
            Duration::from_secs(1),
        );
        let generation = provenance.generation;
        let expected = AdmissionIdentity {
            generation,
            launch_id: launch_id.to_owned(),
            instance: identity.instance,
            uid: identity.uid,
            parent_launch_id: provenance.parent_launch_id.clone(),
        };
        let (file, path) = partial_acknowledgement(&ledger, generation, launch_id);
        assert!(
            fs::read(&path)
                .expect("incomplete acknowledgement")
                .is_empty()
        );
        let spawn_identity_path = ledger
            .generation_path(generation)
            .join("admissions")
            .join(launch_id)
            .join("spawn-identity.json");
        let writer = thread::spawn(move || {
            wait_for_file(&spawn_identity_path);
            thread::sleep(Duration::from_millis(100));
            complete_acknowledgement(file, expected);
        });

        let finish = finish_hosted_admission(&mut authority, identity, &provenance, None);
        writer.join().expect("acknowledgement writer");
        finish.expect("partial acknowledgement admits before deadline");
        assert!(matches!(
            admission_result(&ledger, generation, launch_id).state,
            AdmissionResultState::Admitted
        ));
        assert!(matches!(authority.poll(), Ok(None)));
        authority
            .terminate(Duration::from_secs(2))
            .expect("terminate child");
        authority.cleanup();
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn acknowledgement_completed_after_deadline_does_not_admit_child() {
        let launch_id = "late-acknowledgement";
        let (_bed, ledger, provenance, mut authority, identity) = admission_fixture(
            "hosted-late-acknowledgement",
            launch_id,
            Duration::from_millis(50),
        );
        let generation = provenance.generation;
        let pid = authority.pid();
        let expected = AdmissionIdentity {
            generation,
            launch_id: launch_id.to_owned(),
            instance: identity.instance,
            uid: identity.uid,
            parent_launch_id: provenance.parent_launch_id.clone(),
        };
        let (file, path) = partial_acknowledgement(&ledger, generation, launch_id);
        assert!(
            fs::read(&path)
                .expect("incomplete acknowledgement")
                .is_empty()
        );
        let spawn_identity_path = ledger
            .generation_path(generation)
            .join("admissions")
            .join(launch_id)
            .join("spawn-identity.json");
        let writer = thread::spawn(move || {
            wait_for_file(&spawn_identity_path);
            thread::sleep(Duration::from_millis(100));
            complete_acknowledgement(file, expected);
        });

        let error = finish_hosted_admission(&mut authority, identity, &provenance, None)
            .expect_err("late acknowledgement rejects child");
        writer.join().expect("acknowledgement writer");
        assert!(matches!(error, LaunchError::Admission(_)));
        assert!(matches!(
            admission_result(&ledger, generation, launch_id).state,
            AdmissionResultState::RejectedAndReaped { .. }
        ));
        wait_until_gone(pid);
        authority.cleanup();
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn matching_acknowledgement_cannot_admit_after_deadline() {
        let launch_id = "expired-acknowledgement";
        let (_bed, ledger, provenance, mut authority, identity) =
            admission_fixture("hosted-expired-acknowledgement", launch_id, Duration::ZERO);
        let generation = provenance.generation;
        let pid = authority.pid();
        let expected = AdmissionIdentity {
            generation,
            launch_id: launch_id.to_owned(),
            instance: identity.instance,
            uid: identity.uid,
            parent_launch_id: provenance.parent_launch_id.clone(),
        };
        let (file, _) = partial_acknowledgement(&ledger, generation, launch_id);
        complete_acknowledgement(file, expected);

        let error = finish_hosted_admission(&mut authority, identity, &provenance, None)
            .expect_err("expired acknowledgement rejects child");
        assert!(matches!(error, LaunchError::Admission(_)));
        assert!(matches!(
            admission_result(&ledger, generation, launch_id).state,
            AdmissionResultState::RejectedAndReaped { .. }
        ));
        wait_until_gone(pid);
        authority.cleanup();
    }
}
