// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Conservative no-owned-process facade for non-Unix targets.

use std::fmt;
use std::io;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Output};
use std::time::{Duration, Instant};

use super::{
    BoxedTerminateFn, CommandLaunchRequest, DescendantObservationFailure,
    DescendantTerminationOutcome, Disposition, HostedLaunchProvenance, InspectResult,
    InstanceCensus, InstanceVerdict, LaunchError, LaunchedProcessIdentity, ManagedLaunchRequest,
    ProcessInstance, ProcessInstanceSource, SignalKind, SpawnError, SpawnOptions,
    SystemProcessInstanceSource, TerminationError, TerminationOutcome,
    require_managed_process_capability,
};

#[cfg(any(windows, test))]
mod command_line;
#[cfg(any(windows, test))]
mod environment;
#[cfg(any(windows, test))]
mod identity;
#[cfg(any(windows, test))]
mod launch_spec;
#[cfg(any(windows, test))]
mod path_list;
#[cfg(any(windows, test))]
mod resolve;
#[cfg(any(windows, test))]
mod user_path;

#[cfg(windows)]
pub(crate) use identity::current_windows_process_instance;
#[cfg(all(windows, feature = "test-hooks"))]
pub use identity::windows_filetime_value_from_raw_for_test;

#[cfg(not(unix))]
impl ProcessInstanceSource for SystemProcessInstanceSource {
    fn inspect(&self, _pid: u32) -> InspectResult {
        #[cfg(target_os = "ios")]
        {
            return InspectResult::Unverifiable;
        }
        #[cfg(not(target_os = "ios"))]
        InspectResult::Unverifiable
    }

    fn census(&self) -> InstanceCensus {
        #[cfg(target_os = "ios")]
        {
            return InstanceCensus::Incomplete(Vec::new());
        }
        #[cfg(not(target_os = "ios"))]
        InstanceCensus::Incomplete(Vec::new())
    }

    fn observe(&self, expected: &ProcessInstance) -> InstanceVerdict {
        #[cfg(windows)]
        {
            return identity::verdict_from_windows_probe(
                expected,
                identity::sample_windows_process_with(
                    &identity::SystemWindowsProcessApi,
                    expected.pid,
                ),
            );
        }
        #[cfg(not(windows))]
        {
            let _ = expected;
            InstanceVerdict::Unverifiable
        }
    }
}

/// No Windows-owned process can exist until a later implementation supplies
/// a birth-bound containment primitive.
pub enum ManagedProcess {}

impl ManagedProcess {
    pub fn spawn(_cmd: Vec<String>, _options: SpawnOptions) -> Result<Self, SpawnError> {
        require_managed_process_capability()
            .map_err(|needed| SpawnError::CapabilityUnavailable { needed })?;
        unreachable!("non-Unix managed-process capability unexpectedly available")
    }

    pub fn spawn_exact(_cmd: Vec<String>, _options: SpawnOptions) -> Result<Self, SpawnError> {
        require_managed_process_capability()
            .map_err(|needed| SpawnError::CapabilityUnavailable { needed })?;
        unreachable!("non-Unix managed-process capability unexpectedly available")
    }

    pub fn pid(&self) -> u32 {
        match *self {}
    }

    pub fn pgid(&self) -> io::Result<i32> {
        match *self {}
    }

    pub fn name(&self) -> &str {
        match *self {}
    }

    pub fn cmd(&self) -> &[String] {
        match *self {}
    }

    pub fn poll(&mut self) -> io::Result<Option<i32>> {
        match *self {}
    }

    pub fn wait(&mut self) -> io::Result<i32> {
        match *self {}
    }

    pub fn terminate(
        &mut self,
        _timeout: Duration,
    ) -> Result<TerminationOutcome, TerminationError> {
        match *self {}
    }

    pub fn terminate_exact(
        &mut self,
        _timeout: Duration,
    ) -> Result<TerminationOutcome, TerminationError> {
        match *self {}
    }

    pub fn terminate_exact_until(
        &mut self,
        _deadline: Instant,
    ) -> Result<TerminationOutcome, TerminationError> {
        match *self {}
    }

    pub fn detach_after_bounded_shutdown(&mut self) {
        match *self {}
    }

    pub fn signal_exact(&mut self, _signal: SignalKind) -> Result<(), TerminationError> {
        match *self {}
    }

    pub fn log_path(&self) -> std::path::PathBuf {
        match *self {}
    }

    pub fn cleanup(&mut self) {
        match *self {}
    }

    pub fn cleanup_until(&mut self, _deadline: Instant) -> bool {
        match *self {}
    }
}

pub enum LaunchAuthority {}

impl fmt::Debug for LaunchAuthority {
    fn fmt(&self, _formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {}
    }
}

impl LaunchAuthority {
    pub fn pid(&self) -> u32 {
        match *self {}
    }

    pub fn disposition(&self) -> &Disposition {
        match *self {}
    }

    pub fn exact_identity(&self) -> Option<LaunchedProcessIdentity> {
        match *self {}
    }

    pub fn bind_exact_identity(
        &mut self,
        _identity: LaunchedProcessIdentity,
    ) -> Result<(), LaunchError> {
        match *self {}
    }

    pub fn poll(&mut self) -> io::Result<Option<i32>> {
        match *self {}
    }

    pub fn wait(&mut self) -> io::Result<i32> {
        match *self {}
    }

    pub fn terminate(&mut self, _timeout: Duration) -> Result<(), LaunchError> {
        match *self {}
    }

    pub fn terminate_exact(&mut self, _timeout: Duration) -> Result<(), LaunchError> {
        match *self {}
    }

    pub(crate) fn terminate_exact_until(&mut self, _deadline: Instant) -> Result<(), LaunchError> {
        match *self {}
    }

    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        match *self {}
    }

    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        match *self {}
    }

    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        match *self {}
    }

    pub fn wait_with_output(self) -> Result<Output, LaunchError> {
        match self {}
    }

    pub fn cleanup(&mut self) {
        match *self {}
    }

    pub fn relinquish_explicitly_unowned(self) -> Result<(), LaunchError> {
        match self {}
    }

    pub fn into_managed(self) -> Result<ManagedProcess, LaunchError> {
        match self {}
    }

    pub(crate) fn cleanup_until(&mut self, _deadline: Instant) -> bool {
        match *self {}
    }

    pub(crate) fn detach_after_bounded_shutdown(&mut self) {
        match *self {}
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
    require_managed_process_capability()
        .map_err(|needed| LaunchError::CapabilityUnavailable { needed })?;
    unreachable!("non-Unix managed-process capability unexpectedly available")
}

pub fn launch_managed<F>(
    _disposition: Disposition,
    _spawn: F,
) -> Result<LaunchAuthority, LaunchError>
where
    F: FnOnce() -> Result<ManagedProcess, SpawnError>,
{
    require_managed_process_capability()
        .map_err(|needed| LaunchError::CapabilityUnavailable { needed })?;
    unreachable!("non-Unix managed-process capability unexpectedly available")
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
    require_managed_process_capability()
        .map_err(|needed| LaunchError::CapabilityUnavailable { needed })?;
    unreachable!("non-Unix managed-process capability unexpectedly available")
}

pub fn launch_managed_with<F, Cap>(
    _disposition: Disposition,
    _spawn: F,
    _capability: Cap,
) -> Result<LaunchAuthority, LaunchError>
where
    F: FnOnce() -> Result<ManagedProcess, SpawnError>,
    Cap: FnOnce(&Disposition) -> Result<(), LaunchError>,
{
    require_managed_process_capability()
        .map_err(|needed| LaunchError::CapabilityUnavailable { needed })?;
    unreachable!("non-Unix managed-process capability unexpectedly available")
}

pub fn launch_command(
    _disposition: Disposition,
    _request: CommandLaunchRequest,
    _terminate_fn: BoxedTerminateFn,
) -> Result<LaunchAuthority, LaunchError> {
    require_managed_process_capability()
        .map_err(|needed| LaunchError::CapabilityUnavailable { needed })?;
    unreachable!("non-Unix managed-process capability unexpectedly available")
}

pub fn launch_command_hosted(
    _disposition: Disposition,
    _request: CommandLaunchRequest,
    _provenance: HostedLaunchProvenance,
    _terminate_fn: BoxedTerminateFn,
) -> Result<LaunchAuthority, LaunchError> {
    require_managed_process_capability()
        .map_err(|needed| LaunchError::CapabilityUnavailable { needed })?;
    unreachable!("non-Unix managed-process capability unexpectedly available")
}

pub fn launch_managed_request(
    _disposition: Disposition,
    _request: ManagedLaunchRequest,
) -> Result<LaunchAuthority, LaunchError> {
    require_managed_process_capability()
        .map_err(|needed| LaunchError::CapabilityUnavailable { needed })?;
    unreachable!("non-Unix managed-process capability unexpectedly available")
}

pub fn launch_managed_hosted(
    _disposition: Disposition,
    _request: ManagedLaunchRequest,
    _provenance: HostedLaunchProvenance,
) -> Result<LaunchAuthority, LaunchError> {
    require_managed_process_capability()
        .map_err(|needed| LaunchError::CapabilityUnavailable { needed })?;
    unreachable!("non-Unix managed-process capability unexpectedly available")
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
