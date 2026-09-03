// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use solstone_core_timeline::{
    ArtifactCurrentness, DayTimelineV1, MasterTimelineV1, SegmentBindingV1, SegmentTimelineV1,
    TimelineError, day_subject_key, day_timeline_path, evaluate_artifact_currentness,
    master_subject_key, master_timeline_path, segment_subject_key, segment_timeline_path,
    validate_day_timeline, validate_master_timeline, validate_segment_timeline,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TimelineStatus {
    Current,
    Stale,
    Missing,
}

impl TimelineStatus {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Stale => "stale",
            Self::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ArtifactOutcome {
    Current,
    Missing,
    Unreadable,
    Malformed,
    Invalid,
    StateMissing,
    StateUnavailable,
    DigestMismatch,
}

impl ArtifactOutcome {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Missing => "missing",
            Self::Unreadable => "unreadable",
            Self::Malformed => "malformed",
            Self::Invalid => "invalid",
            Self::StateMissing => "state_missing",
            Self::StateUnavailable => "state_unavailable",
            Self::DigestMismatch => "digest_mismatch",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct ArtifactProjection<T> {
    pub(super) value: Option<T>,
    pub(super) status: TimelineStatus,
    pub(super) outcome: ArtifactOutcome,
}

pub(super) fn master(root: &Path) -> ArtifactProjection<MasterTimelineV1> {
    read_artifact(
        &master_timeline_path(root),
        master_subject_key(),
        |value: &MasterTimelineV1| &value.source_digest,
        |value: &MasterTimelineV1| value.generated_at_ms,
        |text| {
            let value = serde_json::from_str(text)?;
            validate_master_timeline(&value)?;
            Ok(value)
        },
        root,
    )
}

pub(super) fn day(root: &Path, day: &str) -> ArtifactProjection<DayTimelineV1> {
    read_artifact(
        &day_timeline_path(root, day),
        &day_subject_key(day),
        |value: &DayTimelineV1| &value.source_digest,
        |value: &DayTimelineV1| value.generated_at_ms,
        |text| {
            let value = serde_json::from_str(text)?;
            validate_day_timeline(&value)?;
            Ok(value)
        },
        root,
    )
}

pub(super) fn segment(
    root: &Path,
    binding: &SegmentBindingV1,
) -> ArtifactProjection<SegmentTimelineV1> {
    let path = match segment_timeline_path(root, binding) {
        Ok(path) => path,
        Err(_) => return stale_without_value(ArtifactOutcome::Invalid),
    };
    read_artifact(
        &path,
        &segment_subject_key(binding),
        |value: &SegmentTimelineV1| &value.input_digest,
        |value: &SegmentTimelineV1| value.generated_at_ms,
        |text| {
            let value = serde_json::from_str(text)?;
            validate_segment_timeline(&value)?;
            Ok(value)
        },
        root,
    )
}

fn read_artifact<T>(
    path: &Path,
    subject: &str,
    digest: impl Fn(&T) -> &str,
    generated_at_ms: impl Fn(&T) -> i64,
    parse: impl FnOnce(&str) -> Result<T, TimelineError>,
    root: &Path,
) -> ArtifactProjection<T> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ArtifactProjection {
                value: None,
                status: TimelineStatus::Missing,
                outcome: ArtifactOutcome::Missing,
            };
        }
        Err(_) => return stale_without_value(ArtifactOutcome::Unreadable),
    };
    let value = match parse(&text) {
        Ok(value) => value,
        Err(TimelineError::Serde(_)) => return stale_without_value(ArtifactOutcome::Malformed),
        Err(_) => return stale_without_value(ArtifactOutcome::Invalid),
    };
    let outcome = match evaluate_artifact_currentness(
        root,
        subject,
        digest(&value),
        generated_at_ms(&value),
        &text,
    ) {
        Ok(ArtifactCurrentness::Current) => ArtifactOutcome::Current,
        Ok(ArtifactCurrentness::Stale) => ArtifactOutcome::DigestMismatch,
        Ok(ArtifactCurrentness::Missing) => ArtifactOutcome::StateMissing,
        Err(_) => ArtifactOutcome::StateUnavailable,
    };
    ArtifactProjection {
        value: Some(value),
        status: if outcome == ArtifactOutcome::Current {
            TimelineStatus::Current
        } else {
            TimelineStatus::Stale
        },
        outcome,
    }
}

fn stale_without_value<T>(outcome: ArtifactOutcome) -> ArtifactProjection<T> {
    ArtifactProjection {
        value: None,
        status: TimelineStatus::Stale,
        outcome,
    }
}
