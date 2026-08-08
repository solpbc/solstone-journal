// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod descendants;
mod events;
mod log;
mod restart;
mod spawn;
mod terminate;

use std::process::ExitStatus;

pub use descendants::{Descendant, ProcessTreeSnapshot};
pub use events::{OutputStream, ProcessEvent, ProcessEventSink};
pub use log::DailyLogWriter;
pub use restart::{RestartPolicy, TEMPFAIL_DELAY, describe_exit, exit_status_for_code};
pub use spawn::{ManagedProcess, SpawnError, SpawnOptions};
pub use terminate::{
    CAP_TERMINATION_TIMEOUT, KILL_REAP_GRACE, SERVICE_SHUTDOWN_TIMEOUT,
    TASK_QUEUE_SHUTDOWN_TIMEOUT, TerminationError, TerminationOutcome,
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
