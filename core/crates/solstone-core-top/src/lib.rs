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
    run_top_with, run_top_with_in,
};
pub use process::{
    ProcessBirth, ProcessIdentity, ProcessObserver, ProcessSample, ProcessUnavailableReason,
    platform_observer,
};
pub use production::{
    ProductionCallosum, ProductionReceive, ProductionTerminal, TerminalOwner, TerminalOwnerError,
    TerminalSyscalls, run_top_with_outer_panic_cleanup, run_top_with_outer_panic_cleanup_in,
};
pub use reduce::{
    ReductionDisposition, ReductionEffects, ReductionSample, STATUS_TIMEOUT_SECONDS, TopMalformed,
    TopMalformedKind, TopRoute, apply_receive_event, cleanup_processes, reduce_envelope,
};
pub use render::{
    AnsiTopStyle, FrameSample, MAX_FRAME_OPS, PlainTopStyle, TopRenderOp, TopStyle, TrustedToken,
    format_log_age, format_runtime, format_uptime, render_frame, render_frame_in, render_ops,
    render_ops_in, transform_trusted_render,
};
pub use state::{BrainHealthState, DomainContinuity, DomainRecovery, TopState};

/// Run the native interactive command with real terminal, journal, and
/// Callosum adapters using the invoking terminal's locale.
pub fn run(verbose: bool, debug: bool) -> Result<(), String> {
    let locale =
        solstone_core_cli_copy::terminal_locale().unwrap_or(solstone_core_cli_copy::ENGLISH);
    run_in(locale, verbose, debug)
}

/// Run the native interactive command with real adapters in an explicit locale.
pub fn run_in(locale: &str, verbose: bool, debug: bool) -> Result<(), String> {
    production::run_in(locale, verbose, debug)
}
