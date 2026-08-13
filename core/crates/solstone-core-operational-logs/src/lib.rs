// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only support for journal operational service logs: immutable readers,
//! one-shot collection and rendering, an injected follower, and CPython-compatible count parsing.

mod collect;
mod count;
mod error;
mod follow;
mod read;
mod render;

#[cfg(test)]
mod fixture;

pub use collect::{HealthLogsQuery, collect_health_logs};
pub use count::{
    CanonicalInteger, CountParseError, ParsedCount, ServicePort, parse_health_log_count,
    parse_integer_text, parse_service_port, python_int_whitespace_ranges,
};
pub use error::{CollectError, EnumerationError, HealthDirectoryProbeError, OrdinaryTailError};
pub use follow::{
    FollowFatalError, FollowFs, FollowReadError, FollowReader, FollowState, FollowTickContext,
    InitialDiscovery, StdFollowFs, TickOutcome, discover_initial, run_follow, tick,
};
pub use read::{
    DayLogDirectoryOps, DayLogEntry, HealthDirectoryState, ProbeKind, ProbeOps,
    StdDayLogDirectoryOps, StdProbeOps, StdTailFileOpener, TailFile, TailFileOpener,
    list_day_log_symlinks, probe_health_directory, tail_ordinary_text, tail_reverse_text,
};
pub use render::render_collected;
