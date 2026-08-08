// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod descendants;
mod events;
mod log;
mod restart;
mod spawn;
mod terminate;

pub use descendants::{Descendant, ProcessTreeSnapshot};
pub use events::{OutputStream, ProcessEvent, ProcessEventSink};
pub use log::DailyLogWriter;
pub use restart::{RestartPolicy, TEMPFAIL_DELAY, describe_exit, exit_status_for_code};
pub use spawn::{ManagedProcess, SpawnOptions};
pub use terminate::{
    CAP_TERMINATION_TIMEOUT, DescendantCoverage, KILL_REAP_GRACE, SERVICE_SHUTDOWN_TIMEOUT,
    TASK_QUEUE_SHUTDOWN_TIMEOUT, TerminationError, TerminationOutcome,
};
