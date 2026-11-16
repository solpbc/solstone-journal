// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A screen carried across the segment boundary.
//!
//! Each segment records each screen as its own video, and a video's first frame used to
//! be kept and described no matter what. A screen that has not changed since the previous
//! segment was described again from scratch. Instead, the winnow starts from the previous
//! segment's last kept frame of the same screen, and the text gate starts from that
//! frame's stored lines. When the first frame matches, the previous segment's row for that
//! frame is copied in its place, so an unchanged screen costs no model calls.
//!
//! Every condition here falls back to describing the video on its own. A fallback costs
//! the saving, never the description.

use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use solstone_core_format::segment::segment_key;
use solstone_core_journal_io::DEFAULT_STREAM;
use solstone_core_processing_record::vocab;

use crate::phone::PhoneScreen;
use crate::screen_ocr::{self, CoverageState, RawOccurrence};

/// What the previous segment left for one screen.
pub(crate) struct Carry {
    /// The hash of the previous segment's last kept frame.
    pub(crate) reference: u64,
    /// That frame's lines, as its private screen-text sidecar stored them.
    pub(crate) occurrences: Vec<RawOccurrence>,
    row: Value,
    origin: Value,
}

impl Carry {
    /// The previous segment's row, standing in for this video's first frame.
    ///
    /// The copy is whole, so every reader of the row works unchanged. `carried_from` names
    /// the segment and frame that were described, never an intermediate copy.
    pub(crate) fn row(&self, frame_id: u64, timestamp: f64) -> Value {
        let mut row = self.row.clone();
        row["frame_id"] = json!(frame_id);
        row["timestamp"] = json!(timestamp);
        row["requests"] = json!([]);
        row["carried_from"] = self.origin.clone();
        row
    }
}

/// The carry for `video`, when its true predecessor described the same screen and kept
/// what the carry needs. `None` describes the video on its own, as before.
pub(crate) fn resolve(journal: &Path, video: &Path, recipe_id: &str) -> Option<Carry> {
    if crate::decode::is_still(video) {
        return None;
    }
    let stem = video.file_stem()?.to_str()?;
    let segment_dir = video.parent()?;
    let segment = segment_dir.file_name()?.to_str()?;
    let stream_dir = segment_dir.parent()?;
    let day = crate::notify::day_for_path(video)?;
    let stream = if stream_dir.file_name()?.to_str()? == day {
        DEFAULT_STREAM
    } else if stream_dir.parent()?.file_name()?.to_str()? == day {
        stream_dir.file_name()?.to_str()?
    } else {
        return None;
    };
    if stream.starts_with("import.") {
        return None;
    }
    // Same day and stream, the segment just before, with a short enough gap. Never the
    // last segment processed: backfill and reprocessing run out of order.
    let predecessor =
        solstone_core_system_health::resolve_predecessor(journal, &day, Some(stream), segment)?;
    let predecessor = predecessor.get("segment")?.as_str()?;
    let (header, rows) = read_screen(&stream_dir.join(predecessor).join(format!("{stem}.jsonl")))?;
    let record = header.get("_solstone_processing")?;
    if record.get("state").and_then(Value::as_str) != Some(vocab::STATE_ANALYZED)
        || PhoneScreen::from_segment_meta(&header).is_some()
        || header
            .get("raw")
            .and_then(Value::as_str)
            .is_none_or(|raw| crate::decode::is_still(Path::new(raw)))
    {
        return None;
    }
    let reference = u64::from_str_radix(header.get("last_hash")?.as_str()?, 16).ok()?;
    let row = rows
        .into_iter()
        .max_by_key(|row| row.get("frame_id").and_then(Value::as_u64))?;
    if !crate::merge::is_analyzed(&row) || row.get("error").is_some() {
        return None;
    }
    // A carried row's text lives where it was described; `carried_from` always names that
    // segment, so this is at most one step back.
    let (origin_segment, origin_row) = match row.get("carried_from") {
        Some(origin) => {
            let segment = origin
                .get("segment")?
                .as_str()
                .filter(|segment| segment_key(segment).as_deref() == Some(*segment))?;
            let frame_id = origin.get("frame_id")?.as_u64()?;
            let (_, rows) = read_screen(&stream_dir.join(segment).join(format!("{stem}.jsonl")))?;
            let described = rows
                .into_iter()
                .find(|row| row.get("frame_id").and_then(Value::as_u64) == Some(frame_id))?;
            // The described row must still be the one that was copied.
            if described.get("analysis") != row.get("analysis") {
                return None;
            }
            (segment.to_owned(), described)
        }
        None => (predecessor.to_owned(), row.clone()),
    };
    let frame_id = origin_row.get("frame_id")?.as_u64()?;
    let offset = screen_ocr::segment_offset_millis(origin_row.get("timestamp")?.as_f64()?);
    let occurrences = screen_ocr::load_segment_sidecars(&stream_dir.join(&origin_segment))
        .into_iter()
        .filter(|sidecar| {
            sidecar.recipe_id == recipe_id
                && Path::new(&sidecar.source_name)
                    .file_stem()
                    .is_some_and(|name| name == stem)
        })
        .find_map(|sidecar| {
            sidecar.frames.into_iter().find(|frame| {
                frame.frame_id == frame_id
                    && frame.segment_offset_ms == offset
                    && matches!(frame.state, CoverageState::Success | CoverageState::Empty)
            })
        })?
        .occurrences;
    let origin = json!({"segment": origin_segment, "frame_id": frame_id});
    Some(Carry {
        reference,
        occurrences,
        row,
        origin,
    })
}

/// A screen output's header and rows; `None` when any line is not a JSON object.
fn read_screen(path: &Path) -> Option<(Value, Vec<Value>)> {
    let contents = fs::read_to_string(path).ok()?;
    let mut lines = contents.lines().map(serde_json::from_str::<Value>);
    let header = lines.next()?.ok().filter(Value::is_object)?;
    let rows = lines
        .map(|line| line.ok().filter(Value::is_object))
        .collect::<Option<Vec<_>>>()?;
    Some((header, rows))
}
