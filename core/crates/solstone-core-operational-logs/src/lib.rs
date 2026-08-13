// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Immutable readers for journal operational service logs.

mod error;
mod read;

#[cfg(test)]
mod fixture;

pub use error::{EnumerationError, HealthDirectoryProbeError, OrdinaryTailError};
pub use read::{
    DayLogDirectoryOps, DayLogEntry, HealthDirectoryState, ProbeKind, ProbeOps,
    StdDayLogDirectoryOps, StdProbeOps, StdTailFileOpener, TailFile, TailFileOpener,
    list_day_log_symlinks, probe_health_directory, tail_ordinary_text, tail_reverse_text,
};
