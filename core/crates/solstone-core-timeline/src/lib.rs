// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Versioned timeline artifacts and their durable publication primitives.

mod binding;
mod error;
mod fingerprint;
mod locks;
mod schema;
mod state;
mod store;

pub use binding::{
    discover_day_segment_bindings, origin_for_binding, resolve_segment_binding, segment_directory,
};
pub use error::{InvalidSelectionReason, TimelineCurationStage, TimelineError};
pub use fingerprint::curation_input_digest;
pub use locks::{
    TimelineLockRequest, TimelineLockSet, TimelineLockSubject, acquire_timeline_locks,
    segment_attempt_lock_name,
};
pub use schema::{
    CURRENT_SCHEMA_VERSION, CurationContentPartV1, CurationRecordV1, CurationRequestV1,
    DayTimelineV1, GenerationProvenanceV1, HourTimelineV1, MasterTimelineV1, MonthTimelineEntryV1,
    MonthTimelineV1, SegmentBindingV1, SegmentSelectorV1, SegmentSummaryV1, SegmentTimelineV1,
    TimelineEntryV1, TimelineKind, validate_day_timeline, validate_master_timeline,
    validate_segment_binding, validate_segment_timeline,
};
pub use state::{
    ArtifactStateV1, AttemptOutcome, AttemptStateV1, MAX_DIAGNOSTIC_DETAIL_BYTES, TimelineStateV1,
    bounded_diagnostic_detail, load_timeline_state, record_artifact_published,
    record_attempt_outcome, record_attempt_started, save_timeline_state, timeline_state_path,
    update_timeline_state,
};
pub use store::{
    publish_continuation_summary, publish_segment_timeline, segment_subject_key,
    segment_timeline_path,
};
