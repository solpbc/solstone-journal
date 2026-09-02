// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only schema and folds for per-day thinking health logs.

mod backlog;
mod catchup_state;
mod change_detection;
mod completion;
mod data_state;
mod error;
mod event;
mod grep_compile;
mod loader;
mod progress;
mod read;
mod safe_text;
mod scan;
mod segment_state;
mod source;
mod sync_copy;
mod terminal;
mod types;
mod vocabulary;

pub use backlog::{daily_failure_capped, read_backlog_view};
pub use catchup_state::{
    read_backoff_summary, read_segment_repair_attempted, read_segment_repair_summary,
};
pub use change_detection::{detect_segment_change, resolve_predecessor};
pub use completion::{
    blocked_segment_keys, classify_segment_completion, lookup_segment_progress,
    segment_fully_sensed, segment_fully_thought, segment_requires_processing,
};
pub use data_state::derive_modality_state;
pub use error::HealthError;
pub use event::{EventPayload, HealthEvent, RunLogRecord};
pub use grep_compile::{GrepCompileError, GrepPattern, compile_grep_pattern, decimal_digit_value};
pub use loader::{
    BoundedStderr, STDERR_LIMIT, classify_loader_failure, read_bounded_stderr, unresolved_library,
};
pub use progress::read_segment_progress;
pub use safe_text::{
    sanitize_for_terminal, sanitize_os_bytes_for_terminal, sanitize_os_bytes_for_terminal_bounded,
    sanitize_str_for_terminal_bounded, unsafe_ranges,
};
pub use scan::{DaySegment, ScanResult, TimeRange, scan_day};
pub use segment_state::{find_segment_dir, read_segment_data_state};
pub use source::{
    FilesystemHealthLogSource, FilesystemSegmentSource, HealthLogSource, SegmentSource,
    day_is_complete,
};
pub use sync_copy::{
    ADMISSION_WAIT_UNVERIFIABLE_COPY, HEARTBEAT_WITHOUT_WAIT_MARKER_COPY, SyncRescanDiagnosis,
    describe_sync_rescan, format_admission_waiting_copy, format_sync_scan_failure_copy,
};
pub use terminal::{
    is_floor_talent_capped, read_completed_since, read_completed_units,
    read_daily_deterministic_failures, read_terminal_states,
};
pub use types::{
    BacklogDay, BacklogError, BacklogUnit, BacklogView, BackoffSummary, CappedDailySummary,
    CappedDailyUnit, CompletedUnit, CompletionActivity, CompletionSegment, CompletionsSince,
    DailyUnit, DataStateMap, DeterministicFailure, FoldRead, SegmentBlocker,
    SegmentBlockerDimension, SegmentCompletion, SegmentIdentity, SegmentInput, SegmentProgress,
    SegmentRepairSummary, TerminalEvent, TerminalState, TerminalUnit, ThoughtVerdict,
};
pub use vocabulary::{
    BACKLOG_DEFAULT_WINDOW, BACKLOG_STATE_COMPLETE, BACKLOG_STATE_PENDING, BACKLOG_STATE_STUCK,
    BACKLOG_STATE_UNKNOWN, BODY_CARD_STREAMS, CAP, DETERMINISTIC_FAILURE_REASON_CODES, DataState,
    MIN_SPAN_MS, MODALITY_INPUT_AGED_MS, NO_SENSE_COMPLETE_AGED_MS, REASON_CATCHUP_BACKOFF,
    REASON_CORRUPT_RAW, REASON_FAILING_STEP, REASON_SEGMENT_REPAIR_DEGRADED,
    REASON_SEGMENT_REPAIR_PROGRESSING, REASON_SEGMENT_REPAIR_STUCK, REASON_SEGMENT_REPAIR_UNKNOWN,
    SEGMENT_FLOOR_TALENTS, SEGMENT_NO_PROCESSING_MODALITIES, SEGMENT_NONGATING_TALENTS,
    SEGMENT_REPAIR_STATUS_DEGRADED, SEGMENT_REPAIR_STATUS_PROGRESSING, SEGMENT_REPAIR_STATUS_STUCK,
    SEGMENT_REPAIR_STATUS_UNKNOWN, SEGMENT_SUPERSEDED_TALENTS, SENSED_TERMINAL_STATES,
    STUCK_FAIL_THRESHOLD, WHY_CORRUPT_RAW, WHY_FAILED, WHY_NEVER_ATTEMPTED,
    WHY_NO_SENSE_COMPLETE_AGED, WHY_SENSED_NOT_THOUGHT,
};
