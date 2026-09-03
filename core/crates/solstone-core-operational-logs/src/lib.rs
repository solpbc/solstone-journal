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

pub use collect::{
    HealthLogsQuery, SourceTailSnapshot, collect_health_logs, collect_source_tail_snapshot,
};
pub use count::{
    CanonicalInteger, CountParseError, ParsedCount, ServicePort, parse_health_log_count,
    parse_integer_text, parse_service_port, python_int_whitespace_ranges,
};
pub use error::CollectError;
pub use follow::{FollowFatalError, run_follow, run_follow_from_snapshot};
pub use read::{StdTailFileOpener, TailFileOpener, tail_reverse_text};
pub use render::{normalize_raw_stream, render_collected, render_raw_stream};
