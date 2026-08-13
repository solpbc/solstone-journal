// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native, deterministic activity-manager state and presentation.

mod r#loop;
mod process;
mod production;
mod reduce;
mod render;
mod restart;
mod state;

pub use r#loop::{
    TopBrainSource, TopClock, TopInput, TopLoopError, TopReceiveTransport, TopTerminal,
    run_top_with,
};
pub use process::{ProcessObserver, ProcessSample, ProcessUnavailableReason, platform_observer};
pub use reduce::{
    ReductionDisposition, ReductionEffects, ReductionSample, STATUS_TIMEOUT_SECONDS, TopMalformed,
    TopMalformedKind, TopRoute, apply_receive_event, cleanup_processes, reduce_envelope,
};
pub use render::{
    FrameSample, PlainTopStyle, TopStyle, format_log_age, format_runtime, format_uptime,
    render_frame,
};
pub use restart::{
    RestartAttempt, RestartFailure, RestartPhase, RestartRequestOutcome, RestartTransition,
    TopRestartError, TopRestartTransport, acknowledge_restart, advance_restart_attempts,
    fail_discontinuous_restarts, request_restart,
};
pub use state::{DomainContinuity, DomainRecovery, TopState};

/// Run the native interactive command with real terminal, journal, and
/// Callosum adapters.
pub fn run(verbose: bool, debug: bool) -> Result<(), String> {
    production::run(verbose, debug)
}
