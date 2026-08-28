// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod authority;
mod descendants;
mod events;
mod instance;
mod log;
mod macos_proc;
mod observation;
mod pdeathsig;
mod restart;
mod spawn;
mod terminate;

use std::process::ExitStatus;

pub use authority::{
    BoxedTerminateFn, CommandLaunchRequest, Disposition, HostedLaunchProvenance, LaunchAuthority,
    LaunchError, ManagedLaunchRequest, launch, launch_command, launch_command_hosted,
    launch_managed, launch_managed_hosted, launch_managed_request, launch_managed_with,
    launch_with,
};
#[cfg(any(test, feature = "test-hooks"))]
pub use authority::{HostedAdmissionTestFault, set_hosted_admission_test_fault};
pub use descendants::{Descendant, ProcessTreeSnapshot};
pub use events::{OutputStream, ProcessEvent, ProcessEventSink};
#[cfg(target_os = "linux")]
pub(crate) use instance::hold_while_instance_live;
#[cfg(target_os = "macos")]
pub(crate) use instance::macos_sweep_table;
pub use instance::{
    CensusRow, ExecutionState, InspectResult, InstanceCensus, InstanceVerdict, ProcessBirth,
    ProcessInstance, ProcessInstanceSource, SystemProcessInstanceSource,
};
pub use log::DailyLogWriter;
pub use observation::{ProcessObservation, ProcessObservationTuple, classify_process_observation};
pub use pdeathsig::apply_parent_death_kill;
pub use restart::{
    EXIT_TEMPFAIL, RestartPolicy, STRUGGLING_THRESHOLD, TEMPFAIL_DELAY, describe_exit,
    exit_status_for_code,
};
pub use spawn::{LaunchedProcessIdentity, ManagedProcess, SpawnError, SpawnOptions};
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) use terminate::signal_pid;
pub use terminate::{
    CAP_TERMINATION_TIMEOUT, DRAIN_JOIN_TIMEOUT, DescendantObservationFailure,
    DescendantTerminationOutcome, KILL_REAP_GRACE, SERVICE_SHUTDOWN_TIMEOUT,
    TASK_QUEUE_SHUTDOWN_TIMEOUT, TerminationError, TerminationOutcome, signal_exact_instance,
    terminate, terminate_descendants_exact, terminate_exact_instance,
};

/// Match Python `Popen.returncode`: normal exits are non-negative and signals
/// are represented by their negative signal number.
pub(crate) fn signal_aware_exit_code(status: &ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;

        -status.signal().unwrap_or(0)
    }
    #[cfg(not(unix))]
    {
        -1
    }
}
