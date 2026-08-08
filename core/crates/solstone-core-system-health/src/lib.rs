// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only schema and folds for per-day thinking health logs.

mod completion;
mod error;
mod event;
mod progress;
mod read;
mod source;
mod terminal;
mod types;
mod vocabulary;

pub use completion::{
    blocked_segment_keys, classify_segment_completion, lookup_segment_progress,
    segment_fully_sensed, segment_fully_thought, segment_requires_processing,
};
pub use error::HealthError;
pub use event::{EventPayload, HealthEvent, RunLogRecord};
pub use progress::read_segment_progress;
pub use source::{FilesystemHealthLogSource, HealthLogSource};
pub use terminal::{
    is_floor_talent_capped, read_completed_since, read_completed_units,
    read_daily_deterministic_failures, read_terminal_states,
};
pub use types::{
    CompletedUnit, CompletionActivity, CompletionSegment, CompletionsSince, DailyUnit,
    DataStateMap, DeterministicFailure, FoldRead, SegmentBlocker, SegmentBlockerDimension,
    SegmentCompletion, SegmentIdentity, SegmentInput, SegmentProgress, TerminalEvent,
    TerminalState, TerminalUnit, ThoughtVerdict,
};
pub use vocabulary::{
    CAP, DETERMINISTIC_FAILURE_REASON_CODES, MIN_SPAN_MS, SEGMENT_FLOOR_TALENTS,
    SEGMENT_NO_PROCESSING_MODALITIES, SEGMENT_NONGATING_TALENTS, SEGMENT_SUPERSEDED_TALENTS,
    SENSED_TERMINAL_STATES,
};
