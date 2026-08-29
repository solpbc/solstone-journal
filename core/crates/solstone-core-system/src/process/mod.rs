// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod common;
mod events;
mod log;
mod observation;
#[cfg(unix)]
#[path = "unix/mod.rs"]
mod platform;
#[cfg(not(unix))]
#[path = "windows/mod.rs"]
mod platform;
mod restart;
#[cfg(all(unix, test))]
#[path = "windows/identity.rs"]
mod windows_identity_tests;

#[cfg(unix)]
use std::process::ExitStatus;

pub use common::CensusRow;
#[cfg(any(test, feature = "test-hooks"))]
pub(crate) use common::hosted_admission_test_fault;
pub(crate) use common::require_managed_process_capability;
#[cfg(windows)]
pub(crate) use common::windows_filetime_epoch_seconds;
pub use common::{
    BoxedTerminateFn, CAP_TERMINATION_TIMEOUT, CommandLaunchRequest, DRAIN_JOIN_TIMEOUT,
    Descendant, DescendantObservationFailure, DescendantTerminationOutcome, Disposition,
    ExecutionState, HostedLaunchProvenance, InspectResult, InstanceCensus, InstanceVerdict,
    KILL_REAP_GRACE, LaunchError, LaunchedProcessIdentity, ManagedLaunchRequest, ProcessBirth,
    ProcessInstance, ProcessInstanceSource, ProcessTreeSnapshot, SERVICE_SHUTDOWN_TIMEOUT,
    SignalKind, SpawnError, SpawnOptions, SystemProcessInstanceSource, TASK_QUEUE_SHUTDOWN_TIMEOUT,
    TerminationError, TerminationOutcome,
};
#[cfg(any(test, feature = "test-hooks"))]
pub use common::{HostedAdmissionTestFault, set_hosted_admission_test_fault};
pub use events::{OutputStream, ProcessEvent, ProcessEventSink};
pub use log::DailyLogWriter;
pub use observation::{ProcessObservation, ProcessObservationTuple, classify_process_observation};
#[cfg(windows)]
pub(crate) use platform::current_windows_process_instance;
#[cfg(target_os = "linux")]
pub(crate) use platform::hold_while_instance_live;
#[cfg(target_os = "macos")]
pub(crate) use platform::macos_sweep_table;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) use platform::signal_pid;
pub use platform::{
    LaunchAuthority, ManagedProcess, apply_parent_death_kill, launch, launch_command,
    launch_command_hosted, launch_managed, launch_managed_hosted, launch_managed_request,
    launch_managed_with, launch_with, signal_exact_instance, terminate,
    terminate_descendants_exact, terminate_exact_instance,
};
pub use restart::{
    EXIT_TEMPFAIL, RestartPolicy, STRUGGLING_THRESHOLD, TEMPFAIL_DELAY, describe_exit,
    exit_status_for_code,
};

/// Match Python `Popen.returncode`: normal exits are non-negative and signals
/// are represented by their negative signal number.
#[cfg(unix)]
pub(crate) fn signal_aware_exit_code(status: &ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    use std::os::unix::process::ExitStatusExt;

    -status.signal().unwrap_or(0)
}
