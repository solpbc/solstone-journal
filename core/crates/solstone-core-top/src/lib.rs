// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native, deterministic activity-manager state and presentation.

mod r#loop;
mod process;
mod production;
mod reduce;
mod render;
mod state;

pub use r#loop::{
    TopBrainSource, TopClock, TopInput, TopLoopError, TopReceiveTransport, TopTerminal,
    run_top_with,
};
pub use process::{
    ProcessBirth, ProcessIdentity, ProcessObserver, ProcessSample, ProcessUnavailableReason,
    platform_observer,
};
pub use production::{
    ProductionCallosum, ProductionReceive, ProductionTerminal, TerminalOwner, TerminalOwnerError,
    TerminalSyscalls, run_top_with_outer_panic_cleanup,
};
pub use reduce::{
    ReductionDisposition, ReductionEffects, ReductionSample, STATUS_TIMEOUT_SECONDS, TopMalformed,
    TopMalformedKind, TopRoute, apply_receive_event, cleanup_processes, reduce_envelope,
};
pub use render::{
    AnsiTopStyle, FrameSample, MAX_FRAME_OPS, PlainTopStyle, TopRenderOp, TopStyle, TrustedToken,
    format_log_age, format_runtime, format_uptime, render_frame, render_ops,
    transform_trusted_render,
};
pub use state::{BrainHealthState, DomainContinuity, DomainRecovery, TopState};

/// Run the native interactive command with real terminal, journal, and
/// Callosum adapters.
pub fn run(verbose: bool, debug: bool) -> Result<(), String> {
    production::run(verbose, debug)
}
