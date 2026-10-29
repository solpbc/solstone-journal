// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The same source-derived prerequisites for discovery and daily-only execution.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::{DaySegment, SegmentIdentity, SegmentProgress};

fn directory(journal: &Path, day: &str, segment: &DaySegment) -> PathBuf {
    match segment.stream_layout {
        solstone_core_journal_io::SegmentLayout::Direct => {
            journal.join("chronicle").join(day).join(&segment.key)
        }
        solstone_core_journal_io::SegmentLayout::Named => journal
            .join("chronicle")
            .join(day)
            .join(&segment.stream)
            .join(&segment.key),
    }
}

/// How far a segment's work has come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentReadiness {
    /// Its thinking covers its input and its saved outputs are published.
    Ready,
    /// Its thinking covers its input; only the publication of its saved
    /// outputs is owed, which needs no model call.
    Unpublished,
    /// Its input is still being sensed or its thinking is not current.
    Owed,
}

pub fn segment_readiness(
    journal: &Path,
    day: &str,
    segment: &DaySegment,
    progress: &BTreeMap<SegmentIdentity, SegmentProgress>,
) -> Result<SegmentReadiness, String> {
    if segment.stream == "mcp.agent" {
        return Ok(SegmentReadiness::Ready);
    }
    if !crate::segment_requires_processing(&segment.clone().into()) {
        return Ok(SegmentReadiness::Ready);
    }
    let directory = directory(journal, day, segment);
    let newest = crate::newest_segment_input_ms(&directory).map_err(|error| error.to_string())?;
    if !crate::segment_fully_sensed(&segment.data_state)
        || !crate::segment_thinking_is_current(
            crate::lookup_segment_progress(progress, &segment.stream, &segment.key),
            newest,
        )
    {
        return Ok(SegmentReadiness::Owed);
    }
    Ok(
        if solstone_core_system::segment_publication::is_current(journal, &directory, newest)? {
            SegmentReadiness::Ready
        } else {
            SegmentReadiness::Unpublished
        },
    )
}

pub fn segment_work_ready(
    journal: &Path,
    day: &str,
    segment: &DaySegment,
    progress: &BTreeMap<SegmentIdentity, SegmentProgress>,
) -> Result<bool, String> {
    Ok(segment_readiness(journal, day, segment, progress)? == SegmentReadiness::Ready)
}

/// A day whose set-aside recordings are at most 1/20 of its recorded time is
/// effectively complete, so its daily work runs without them. When the held
/// work later finishes, its outputs change the day's evidence, and the day's
/// outputs are made again.
pub const SET_ASIDE_CLOSE_SHARE: u64 = 20;

/// A segment's recorded length from its `HHMMSS_LEN` key, at least one second
/// so that no recording weighs nothing.
pub fn segment_seconds(key: &str) -> u64 {
    key.rsplit_once('_')
        .and_then(|(_, length)| length.parse::<u64>().ok())
        .unwrap_or(0)
        .max(1)
}

/// Whether every step a sensed segment's thinking still waits on is set
/// aside: held after repeated failures, with no cause to retry on its current
/// input. Read-only; discovery records the holds. Anything unreadable, or any
/// step that is not held, is ordinary work.
pub fn segment_thinking_set_aside(
    journal: &Path,
    day: &str,
    segment: &DaySegment,
    progress: &BTreeMap<SegmentIdentity, SegmentProgress>,
) -> Result<bool, String> {
    use solstone_core_system::work_retry::{self, Unit};
    if !crate::segment_fully_sensed(&segment.data_state) {
        return Ok(false);
    }
    let thought = crate::lookup_segment_progress(progress, &segment.stream, &segment.key);
    let pending = crate::segment_pending_talents(thought);
    if pending.is_empty()
        || pending.iter().any(|name| {
            name == "facet_routing" || crate::SEGMENT_FLOOR_TALENTS.contains(&name.as_str())
        })
    {
        return Ok(false);
    }
    let newest = crate::newest_segment_input_ms(&directory(journal, day, segment))
        .map_err(|error| error.to_string())?;
    let read_current = thought
        .and_then(|progress| progress.sense_input_ms)
        .is_some_and(|read| newest.is_none_or(|newest| newest < read));
    if !pending.contains("sense") && !read_current {
        return Ok(false);
    }
    let direct = matches!(
        segment.stream_layout,
        solstone_core_journal_io::SegmentLayout::Direct
    );
    for name in pending {
        let unit = Unit::Segment {
            day: day.into(),
            stream: segment.stream.clone(),
            segment: segment.key.clone(),
            talent: name,
            direct,
        };
        let Some(record) = work_retry::read(journal, &unit.id()?)?.filter(|record| record.held)
        else {
            return Ok(false);
        };
        let Ok(input) = work_retry::input::current(journal, &unit) else {
            return Ok(false);
        };
        if record
            .permission(&input, work_retry::DECLARED_FIXES)
            .is_some()
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The directories of a day's thought segments whose saved outputs are not
/// yet published, or `None` while any of its segments still owes thinking,
/// apart from a set-aside gap within `SET_ASIDE_CLOSE_SHARE`. Daily work
/// publishes these itself rather than waiting for them.
pub fn day_unpublished_segments(journal: &Path, day: &str) -> Result<Option<Vec<PathBuf>>, String> {
    let (_, _, segments) = crate::scan_day(
        &crate::FilesystemSegmentSource,
        journal,
        day,
        chrono::Utc::now(),
    )
    .map_err(|error| error.to_string())?;
    let progress =
        crate::read_segment_progress(&crate::FilesystemHealthLogSource::new(journal), day)
            .map_err(|error| error.to_string())?;
    if progress.malformed_line_count != 0 {
        return Err("segment progress contains unreadable records".into());
    }
    let mut unpublished = Vec::new();
    let mut recorded_seconds = 0_u64;
    let mut set_aside_seconds = 0_u64;
    for segment in segments {
        let seconds = if segment.stream == "mcp.agent" {
            0
        } else {
            segment_seconds(&segment.key)
        };
        recorded_seconds += seconds;
        match segment_readiness(journal, day, &segment, &progress.value)? {
            SegmentReadiness::Ready => {}
            SegmentReadiness::Unpublished => unpublished.push(directory(journal, day, &segment)),
            SegmentReadiness::Owed
                if segment_thinking_set_aside(journal, day, &segment, &progress.value)? =>
            {
                set_aside_seconds += seconds;
            }
            SegmentReadiness::Owed => return Ok(None),
        }
    }
    if set_aside_seconds * SET_ASIDE_CLOSE_SHARE > recorded_seconds {
        return Ok(None);
    }
    Ok(Some(unpublished))
}

pub fn day_segments_ready(journal: &Path, day: &str) -> Result<bool, String> {
    Ok(day_unpublished_segments(journal, day)?.is_some_and(|unpublished| unpublished.is_empty()))
}
