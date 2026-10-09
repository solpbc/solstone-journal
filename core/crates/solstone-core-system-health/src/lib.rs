// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only schema and folds for per-day thinking health logs.

#[cfg(test)]
mod acceptance_tests;
mod backlog;
mod backlog_copy;
mod catchup_state;
mod change_detection;
mod completion;
mod data_state;
mod error;
mod event;
mod freshness;
mod grep_compile;
mod index_health;
mod indexing_observation;
mod loader;
mod not_yet;
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
pub use backlog_copy::backlog_day_reason_copy;
pub use catchup_state::{
    read_backoff_summary, read_daily_catchup_finished, read_segment_repair_attempted,
    read_segment_repair_summary,
};
pub use change_detection::{detect_segment_change, resolve_predecessor};
pub use completion::{
    blocked_segment_keys, classify_segment_completion, lookup_segment_progress,
    segment_fully_sensed, segment_fully_thought, segment_requires_processing,
    segment_thinking_is_current,
};
pub use data_state::derive_modality_state;
pub use error::HealthError;
pub use event::{EventPayload, HealthEvent, RunLogRecord};
pub use freshness::{
    BACKLOG_FRESHNESS_MAX_AGE_HOURS, BacklogStatusEvaluation, FUTURE_SKEW_TOLERANCE_MS,
    SummaryFreshness, UNFINISHED_TEMPLATE_MANY_DAYS, UNFINISHED_TEMPLATE_MANY_ONE_DAY,
    UNFINISHED_TEMPLATE_ONE, UnfinishedActivitiesAggregate, VERDICT_AGE_STALE_TEMPLATE,
    VERDICT_AGE_UNKNOWN, VERDICT_ALL_CAUGHT_UP, VERDICT_CAUGHT_UP, VERDICT_MIXED_PENDING_PLURAL,
    VERDICT_MIXED_PENDING_SINGULAR, VERDICT_MIXED_STUCK_PLURAL, VERDICT_MIXED_STUCK_SINGULAR,
    VERDICT_PENDING_ONLY_PLURAL, VERDICT_PENDING_ONLY_SINGULAR, VERDICT_STUCK_ONLY_PLURAL,
    VERDICT_STUCK_ONLY_SINGULAR, VERDICT_UNCLEAR_NOW, aggregate_unfinished_activities,
    aggregate_unfinished_from_days, evaluate_backlog_status, format_summary_age,
    parse_summary_time, select_unfinished_template, summary_freshness,
};
pub use grep_compile::{GrepCompileError, GrepPattern, compile_grep_pattern, decimal_digit_value};
pub use index_health::{
    INDEX_TEXT_BUILDING, INDEX_TEXT_CLASSIFICATION_STALLED, INDEX_TEXT_FAILED_FILES,
    INDEX_TEXT_NEWER_GENERATION, INDEX_TEXT_OK, INDEX_TEXT_REPAIR, INDEX_TEXT_UNREADABLE,
    IndexFailure, IndexHealth, IndexHealthState, behind_text, evaluate_index_health,
    evaluate_index_health_recent, evaluate_index_health_with, index_health_from, retained_text,
};
pub use indexing_observation::{
    IndexingAttemptObservation, IndexingDiagnostics, IndexingObservations,
    LIMIT_TOKEN_FAILURES_NOT_RETAINED_PAST_HISTORY, LIMIT_TOKEN_PUBLICATION_NOT_CRASH_ATOMIC,
    LIMIT_TOKEN_UNFINISHED_NO_DAY_INDEX, LIMIT_TOKEN_UNOBSERVED_OUTSIDE_DAYS,
    indexing_window_bounds, normalize_indexing_identity, read_indexing_observations,
};
pub use loader::{
    BoundedStderr, STDERR_LIMIT, classify_loader_failure, read_bounded_stderr, unresolved_library,
};
pub use not_yet::{
    NOT_YET_ENGINE, NOT_YET_FIRST_NIGHT, NOT_YET_SEARCH, NotYet, OVERNIGHT_WINDOW_END_HOUR,
    journal_not_yet, not_yet_evaluation, not_yet_rule, summary_not_yet,
};
pub use progress::{read_pending_facet_routing, read_segment_progress};
pub use safe_text::{
    sanitize_for_terminal, sanitize_os_bytes_for_terminal, sanitize_os_bytes_for_terminal_bounded,
    sanitize_str_for_terminal_bounded, unsafe_ranges,
};
pub use scan::{
    DaySegment, LocationSegmentRow, ScanResult, TimeRange, UnclaimedImageState,
    holds_location_file, is_location_only, list_location_only_segments, newest_segment_input_ms,
    scan_day, unclaimed_image_state,
};
pub use segment_state::{find_segment_dir, read_segment_data_state};
pub use source::{
    FilesystemHealthLogSource, FilesystemSegmentSource, HealthLogSource, SegmentSource,
    day_is_complete, day_is_complete_with,
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
    DailyUnit, DataStateMap, DeterministicFailure, FoldRead, IndexerPhase, SegmentBlocker,
    SegmentBlockerDimension, SegmentCompletion, SegmentIdentity, SegmentInput, SegmentProgress,
    SegmentRepairSummary, TerminalEvent, TerminalState, TerminalUnit, ThoughtVerdict,
    UnfinishedActivities,
};
pub use vocabulary::{
    BACKLOG_DEFAULT_WINDOW, BACKLOG_STATE_COMPLETE, BACKLOG_STATE_PENDING, BACKLOG_STATE_STUCK,
    BACKLOG_STATE_UNKNOWN, CAP, DETERMINISTIC_FAILURE_REASON_CODES, DataState, MIN_SPAN_MS,
    MODALITY_INPUT_AGED_MS, NO_SENSE_COMPLETE_AGED_MS, REASON_CATCHUP_BACKOFF, REASON_CORRUPT_RAW,
    REASON_FAILING_STEP, REASON_SEGMENT_REPAIR_DEGRADED, REASON_SEGMENT_REPAIR_PROGRESSING,
    REASON_SEGMENT_REPAIR_STUCK, REASON_SEGMENT_REPAIR_UNKNOWN, SEGMENT_FLOOR_TALENTS,
    SEGMENT_NO_PROCESSING_MODALITIES, SEGMENT_NONGATING_TALENTS, SEGMENT_REPAIR_STATUS_DEGRADED,
    SEGMENT_REPAIR_STATUS_PROGRESSING, SEGMENT_REPAIR_STATUS_STUCK, SEGMENT_REPAIR_STATUS_UNKNOWN,
    SEGMENT_SUPERSEDED_TALENTS, SENSED_TERMINAL_STATES, STUCK_FAIL_THRESHOLD, WHY_CORRUPT_RAW,
    WHY_FAILED, WHY_NEVER_ATTEMPTED, WHY_NO_SENSE_COMPLETE_AGED, WHY_SENSED_NOT_THOUGHT,
};
