// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::super::{
    BoxedTerminateFn, CommandLaunchRequest, Disposition, HostedLaunchProvenance, InspectResult,
    LaunchError, LaunchedProcessIdentity, ManagedLaunchRequest, ProcessInstanceSource,
    SERVICE_SHUTDOWN_TIMEOUT, SpawnError, SystemProcessInstanceSource,
    require_managed_process_capability,
};
#[cfg(any(test, feature = "test-hooks"))]
use super::super::{HostedAdmissionTestFault, hosted_admission_test_fault};
use super::spawn::ManagedProcess;
use super::terminate::terminate_exact_instance;
use crate::lifecycle::{
    AdmissionIdentity, AdmissionIntent, AdmissionResult, AdmissionResultState,
    HOSTED_GENERATION_ENV, HOSTED_LAUNCH_ID_ENV, HOSTED_PARENT_LAUNCH_ID_ENV, ParentLossLedger,
    ParentLossPhase, read_parent_loss_admission_acknowledgement,
    write_parent_loss_admission_intent, write_parent_loss_admission_result,
};
use solstone_core_journal_io::{LockOptions, hold_lock};

enum Inner {
    Managed(ManagedProcess),
    Raw {
        child: Child,
        terminate_fn: BoxedTerminateFn,
        exact_identity: Option<LaunchedProcessIdentity>,
    },
}

/// Retained termination authority for one launched child.
pub struct LaunchAuthority {
    inner: Option<Inner>,
    disposition: Disposition,
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
    pub fn pid(&self) -> u32 {
        match self.inner() {
            Inner::Managed(process) => process.pid(),
            Inner::Raw { child, .. } => child.id(),
        }
    }

    pub fn disposition(&self) -> &Disposition {
        &self.disposition
    }

    /// The PID/birth/UID sample retained by an exact managed launch.
    pub fn exact_identity(&self) -> Option<LaunchedProcessIdentity> {
        match self.inner() {
            Inner::Managed(process) => process.exact_identity(),
            Inner::Raw { exact_identity, .. } => *exact_identity,
        }
    }

    /// Bind a raw authority to the exact identity sampled by its launch
    /// boundary.  An explicitly-unowned coordinator is sampled outside this
    /// module, then uses this binding for exact cleanup on bootstrap failure.
    pub fn bind_exact_identity(
        &mut self,
        identity: LaunchedProcessIdentity,
    ) -> Result<(), LaunchError> {
        match self.inner_mut() {
            Inner::Raw { exact_identity, .. } => {
                *exact_identity = Some(identity);
                Ok(())
            }
            Inner::Managed(_) => Err(LaunchError::CapabilityUnavailable {
                needed: "raw launch identity binding",
            }),
        }
    }

    pub fn poll(&mut self) -> io::Result<Option<i32>> {
        match self.inner_mut() {
            Inner::Managed(process) => process.poll(),
            Inner::Raw { child, .. } => child
                .try_wait()
                .map(|status| status.map(|value| super::super::signal_aware_exit_code(&value))),
        }
    }

    pub fn wait(&mut self) -> io::Result<i32> {
        match self.inner_mut() {
            Inner::Managed(process) => process.wait(),
            Inner::Raw { child, .. } => child
                .wait()
                .map(|status| super::super::signal_aware_exit_code(&status)),
        }
    }

    pub fn terminate(&mut self, timeout: Duration) -> Result<(), LaunchError> {
        match self.inner_mut() {
            Inner::Managed(process) => process
                .terminate(timeout)
                .map(|_| ())
                .map_err(|error| LaunchError::Terminate(io::Error::other(error))),
            Inner::Raw {
                child,
                terminate_fn,
                ..
            } => {
                let signaled = terminate_fn(child, timeout);
                let waited = child.wait().map(|_| ()).map_err(LaunchError::Terminate);
                match (signaled, waited) {
                    (Ok(()), Ok(())) => Ok(()),
                    (Err(error), _) => Err(error),
                    (Ok(()), Err(error)) => Err(error),
                }
            }
        }
    }

    pub fn terminate_exact(&mut self, timeout: Duration) -> Result<(), LaunchError> {
        match self.inner_mut() {
            Inner::Managed(process) => process
                .terminate_exact(timeout)
                .map(|_| ())
                .map_err(|error| LaunchError::Terminate(io::Error::other(error))),
            Inner::Raw {
                child,
                exact_identity: Some(identity),
                ..
            } => terminate_exact_instance(
                child,
                identity.instance,
                timeout,
                &SystemProcessInstanceSource,
            )
            .map(|_| ())
            .map_err(|error| LaunchError::Terminate(io::Error::other(error))),
            Inner::Raw { .. } => Err(LaunchError::CapabilityUnavailable {
                needed: "birth-bound process termination",
            }),
        }
    }

    /// Terminate a managed child without opening a wait beyond `deadline`.
    pub(crate) fn terminate_exact_until(&mut self, deadline: Instant) -> Result<(), LaunchError> {
        match self.inner_mut() {
            Inner::Managed(process) => process
                .terminate_exact_until(deadline)
                .map(|_| ())
                .map_err(|error| LaunchError::Terminate(io::Error::other(error))),
            Inner::Raw { .. } => Err(LaunchError::CapabilityUnavailable {
                needed: "birth-bound managed process termination",
            }),
        }
    }

    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        match self.inner_mut() {
            Inner::Managed(_) => None,
            Inner::Raw { child, .. } => child.stdin.take(),
        }
    }

    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        match self.inner_mut() {
            Inner::Managed(_) => None,
            Inner::Raw { child, .. } => child.stdout.take(),
        }
    }

    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        match self.inner_mut() {
            Inner::Managed(_) => None,
            Inner::Raw { child, .. } => child.stderr.take(),
        }
    }

    pub fn wait_with_output(mut self) -> Result<Output, LaunchError> {
        match self.inner.take() {
            Some(Inner::Managed(process)) => {
                drop(process);
                Err(LaunchError::OutputUnavailable)
            }
            Some(Inner::Raw { child, .. }) => {
                child.wait_with_output().map_err(LaunchError::Terminate)
            }
            None => Err(LaunchError::OutputUnavailable),
        }
    }

    pub fn cleanup(&mut self) {
        if let Inner::Managed(process) = self.inner_mut() {
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
            None => Err(LaunchError::OutputUnavailable),
        }
    }

    /// Clean up a managed child only while the caller's shared deadline remains.
    pub(crate) fn cleanup_until(&mut self, deadline: Instant) -> bool {
        match self.inner_mut() {
            Inner::Managed(process) => process.cleanup_until(deadline),
            Inner::Raw { child, .. } => child.try_wait().ok().flatten().is_some(),
        }
    }

    /// Mark a managed authority so drop cannot begin a new service-length
    /// termination window after a bounded shutdown expires.
    pub(crate) fn detach_after_bounded_shutdown(&mut self) {
        if let Some(Inner::Managed(process)) = self.inner.as_mut() {
            process.detach_after_bounded_shutdown();
        }
    }

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
            ..
        }) = self.inner.take()
        else {
            return;
        };
        if child.try_wait().ok().flatten().is_none() {
            let _ = terminate_fn(&mut child, SERVICE_SHUTDOWN_TIMEOUT);
            let _ = child.wait();
        }
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
    let ledger = ParentLossLedger::open(&provenance.journal)
        .map_err(|error| LaunchError::Admission(error.to_string()))?;
    let lock_path = ledger.admission_lock_path(provenance.generation);
    let _lock = hold_lock(
        &lock_path,
        LockOptions {
            timeout: provenance.acknowledgement_timeout,
            poll_interval: Duration::from_millis(10),
            mode: Some(0o600),
        },
    )
    .map_err(|error| LaunchError::Admission(error.to_string()))?;
    let active = ledger
        .active_generation()
        .map_err(|error| LaunchError::Admission(error.to_string()))?
        .ok_or_else(|| LaunchError::Admission("missing active generation".to_owned()))?;
    if active.generation != provenance.generation || active.phase != ParentLossPhase::Admitting {
        return Err(LaunchError::Admission(
            "hosted launch rejected after seal".to_owned(),
        ));
    }
    let intent = AdmissionIntent::new(
        provenance.generation,
        provenance.launch_id.clone(),
        provenance.service,
        provenance.parent_launch_id.clone(),
    );
    write_parent_loss_admission_intent(&provenance.journal, &intent)
        .map_err(|error| LaunchError::Admission(error.to_string()))?;
    inject_hosted_provenance(&mut request.environment, &provenance);
    let mut authority = launch(disposition, move || spawn_command(request), terminate_fn)?;
    let source = SystemProcessInstanceSource;
    let identity = match source.inspect(authority.pid()) {
        InspectResult::Present { instance, uid, .. } => LaunchedProcessIdentity { instance, uid },
        InspectResult::Absent | InspectResult::Unverifiable => {
            let _ = authority.terminate(Duration::from_secs(2));
            return Err(LaunchError::Admission(
                "exact launch identity unavailable".to_owned(),
            ));
        }
    };
    authority.bind_exact_identity(identity)?;
    finish_hosted_admission(&mut authority, identity, &provenance)?;
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

/// The non-bypassable hosted child boundary.  It writes an immutable intent
/// before spawn, captures exact PID/birth/UID, waits for the child-owned
/// acknowledgement, and exact-reaps a child if admission cannot complete.
pub fn launch_managed_hosted(
    disposition: Disposition,
    mut request: ManagedLaunchRequest,
    provenance: HostedLaunchProvenance,
) -> Result<LaunchAuthority, LaunchError> {
    let ledger = ParentLossLedger::open(&provenance.journal)
        .map_err(|error| LaunchError::Admission(error.to_string()))?;
    let lock_path = ledger.admission_lock_path(provenance.generation);
    let _lock = hold_lock(
        &lock_path,
        LockOptions {
            timeout: provenance.acknowledgement_timeout,
            poll_interval: Duration::from_millis(10),
            mode: Some(0o600),
        },
    )
    .map_err(|error| LaunchError::Admission(error.to_string()))?;
    let active = ledger
        .active_generation()
        .map_err(|error| LaunchError::Admission(error.to_string()))?
        .ok_or_else(|| LaunchError::Admission("missing active generation".to_owned()))?;
    if active.generation != provenance.generation || active.phase != ParentLossPhase::Admitting {
        return Err(LaunchError::Admission(
            "hosted launch rejected after seal".to_owned(),
        ));
    }
    let intent = AdmissionIntent::new(
        provenance.generation,
        provenance.launch_id.clone(),
        provenance.service,
        provenance.parent_launch_id.clone(),
    );
    write_parent_loss_admission_intent(&provenance.journal, &intent)
        .map_err(|error| LaunchError::Admission(error.to_string()))?;
    inject_hosted_provenance(&mut request.options.environment, &provenance);
    let mut authority = launch_managed_request(disposition, request)?;
    let identity = authority
        .exact_identity()
        .ok_or_else(|| LaunchError::Admission("exact launch identity unavailable".to_owned()))?;
    finish_hosted_admission(&mut authority, identity, &provenance)?;
    Ok(authority)
}

fn inject_hosted_provenance(
    environment: &mut BTreeMap<OsString, OsString>,
    provenance: &HostedLaunchProvenance,
) {
    environment.insert(
        OsString::from(HOSTED_GENERATION_ENV),
        OsString::from(provenance.generation.to_string()),
    );
    environment.insert(
        OsString::from(HOSTED_LAUNCH_ID_ENV),
        OsString::from(provenance.launch_id.clone()),
    );
    environment.insert(
        OsString::from("SOLSTONE_JOURNAL"),
        provenance.journal.as_os_str().to_os_string(),
    );
    if let Some(parent_launch_id) = provenance.parent_launch_id.as_ref() {
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
) -> Result<(), LaunchError> {
    let expected = AdmissionIdentity {
        generation: provenance.generation,
        launch_id: provenance.launch_id.clone(),
        instance: identity.instance,
        uid: identity.uid,
        parent_launch_id: provenance.parent_launch_id.clone(),
    };
    let deadline = Instant::now() + provenance.acknowledgement_timeout;
    loop {
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
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Ok(None) | Err(_) => break,
        }
    }
    let result_state = match terminate_rejected_hosted_child(authority) {
        Ok(exit_code) => AdmissionResultState::RejectedAndReaped { exit_code },
        Err(error) => AdmissionResultState::RejectedUnreaped {
            detail: error.to_string(),
        },
    };
    let result = AdmissionResult {
        schema: 1,
        identity: Some(expected),
        state: result_state,
    };
    let _ = write_parent_loss_admission_result(
        &provenance.journal,
        provenance.generation,
        &provenance.launch_id,
        &result,
    );
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
        }),
        disposition,
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
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::super::super::SpawnOptions;
    use crate::lifecycle::{
        AdmissionResult, AdmissionResultState, HostedServiceKind, ParentLossLedger,
        ParentLossPhase, acknowledge_parent_loss_admission, write_parent_loss_admission_result,
    };
    use crate::process::{InstanceVerdict, ProcessBirth, ProcessInstance};

    fn process_is_gone(pid: u32) -> bool {
        let Ok(pid) = i32::try_from(pid) else {
            return true;
        };
        matches!(
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
            Err(nix::errno::Errno::ESRCH)
        )
    }

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
        let result_path = ledger
            .generation_path(generation)
            .join("admissions/unacknowledged-child/result.json");
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

        acknowledge_parent_loss_admission(&bed.root, identity.clone())
            .expect("late child acknowledgement may be recorded but not trusted");
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
            Err(crate::lifecycle::ParentLossAdmissionError::Conflict { .. })
        ));
        assert!(matches!(
            ledger
                .active_generation()
                .expect("active generation")
                .expect("active")
                .phase,
            ParentLossPhase::Admitting
        ));
    }
}
