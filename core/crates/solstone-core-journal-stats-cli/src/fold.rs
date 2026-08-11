// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Composition over the system-health segment scan.

use std::path::Path;

use chrono::{DateTime, Utc};
use solstone_core_system_health::{
    HealthLogSource, SegmentInput, SegmentSource, classify_segment_completion,
    read_segment_progress, scan_day as scan_health_day,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SegmentFoldOutcome {
    pub(crate) transcript_ranges: u64,
    pub(crate) percept_ranges: u64,
    pub(crate) browser_segments: u64,
    pub(crate) segments_pending_think: u64,
    pub(crate) segment_fold_failed: bool,
}

pub(crate) fn fold_segments<S: SegmentSource, H: HealthLogSource>(
    segment_source: &S,
    health_source: &H,
    journal_root: &Path,
    day: &str,
    now: DateTime<Utc>,
) -> SegmentFoldOutcome {
    let Ok((audio_ranges, screen_ranges, segments)) =
        scan_health_day(segment_source, journal_root, day, now)
    else {
        return failed();
    };

    let Ok(progress) = read_segment_progress(health_source, day) else {
        return failed();
    };

    let browser_segments = segments
        .iter()
        .filter(|segment| {
            segment
                .types
                .iter()
                .any(|segment_type| segment_type == "browser")
        })
        .count()
        .try_into()
        .unwrap_or(u64::MAX);
    let inputs: Vec<SegmentInput> = segments.into_iter().map(Into::into).collect();
    let completion = classify_segment_completion(&inputs, &progress.value);

    SegmentFoldOutcome {
        transcript_ranges: audio_ranges.len().try_into().unwrap_or(u64::MAX),
        percept_ranges: screen_ranges.len().try_into().unwrap_or(u64::MAX),
        browser_segments,
        segments_pending_think: completion.not_thought.try_into().unwrap_or(u64::MAX),
        segment_fold_failed: false,
    }
}

fn failed() -> SegmentFoldOutcome {
    SegmentFoldOutcome {
        segment_fold_failed: true,
        ..SegmentFoldOutcome::default()
    }
}
