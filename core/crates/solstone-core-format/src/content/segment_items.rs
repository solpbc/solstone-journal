// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Independently readable imported items that share their enclosing segment's facets.
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::kinds::Kind;

pub const ITEMS_DIR: &str = "items";

/// Occurrence identity, independent of a filename, payload digest or filesystem path.
pub fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

const FILES: [&str; 4] = [
    "document_transcript.md",
    "note_transcript.md",
    "image_transcript.md",
    "screenshot_transcript.md",
];

/// Return the ID and transcript filename for day-rooted or chronicle-relative paths.
pub fn coordinate(relative: &str) -> Option<(&str, &str)> {
    let mut parts = relative
        .strip_prefix("chronicle/")
        .unwrap_or(relative)
        .split('/');
    let day = parts.next()?;
    let stream = parts.next()?;
    let segment = parts.next()?;
    let items = parts.next()?;
    let id = parts.next()?;
    let file = parts.next()?;
    (parts.next().is_none()
        && stream == "import.email"
        && items == ITEMS_DIR
        && crate::segment::is_date_key(day)
        && crate::segment::segment_key(segment).as_deref() == Some(segment)
        && valid_id(id)
        && FILES.contains(&file))
    .then_some((id, file))
}

/// Return only complete, unambiguous item publications. A staging directory or a
/// symlink at any item level is invisible, including to a reader following a saved ID.
pub fn published_item(segment: &Path, id: &str) -> Option<PathBuf> {
    if !valid_id(id) || segment.parent()?.file_name()? != "import.email" {
        return None;
    }
    let items = segment.join(ITEMS_DIR);
    let dir = items.join(id);
    if !fs::symlink_metadata(&items).ok()?.is_dir() || !fs::symlink_metadata(&dir).ok()?.is_dir() {
        return None;
    }
    let mut files = FILES
        .iter()
        .map(|name| dir.join(name))
        .filter(|p| fs::symlink_metadata(p).is_ok_and(|meta| meta.is_file()));
    let file = files.next()?;
    files.next().is_none().then_some(file)
}

/// An exchange with readable turns and no committed or staged owner deletion.
pub fn live_parent(segment: &Path) -> bool {
    let Some(name) = segment.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let staged = segment.with_file_name(format!("{}{name}", crate::segment::STAGED_REMOVAL_PREFIX));
    let missing = |p| fs::symlink_metadata(p).is_err_and(|e| e.kind() == io::ErrorKind::NotFound);
    fs::symlink_metadata(segment).is_ok_and(|m| m.is_dir())
        && fs::symlink_metadata(segment.join("conversation_transcript.jsonl"))
            .is_ok_and(|m| m.is_file())
        && missing(segment.join("tombstone.json"))
        && missing(staged)
}

pub fn list(segment: &Path) -> io::Result<Vec<PathBuf>> {
    if segment
        .parent()
        .and_then(Path::file_name)
        .is_none_or(|stream| stream != "import.email")
        || !live_parent(segment)
    {
        return Ok(Vec::new());
    }
    let items = segment.join(ITEMS_DIR);
    match fs::symlink_metadata(&items) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
        Ok(meta) if !meta.is_dir() => return Ok(Vec::new()),
        _ => {}
    }
    let mut paths = Vec::new();
    for entry in fs::read_dir(items)? {
        let entry = entry?;
        if let Some(path) = entry
            .file_name()
            .to_str()
            .and_then(|id| published_item(segment, id))
        {
            paths.push(path)
        }
    }
    paths.sort();
    Ok(paths)
}

/// Resolve only a live, unambiguously published typed item. The source segment tombstone
/// governs retained typed outputs even while the index is catching up.
pub fn is_published_path(path: &Path, rel: &str) -> bool {
    let Some((id, _)) = coordinate(rel) else {
        return false;
    };
    let Some(segment) = path.ancestors().nth(3) else {
        return false;
    };
    live_parent(segment) && published_item(segment, id).as_deref() == Some(path)
}

/// Read through the journal's no-follow, bounded file reader.
pub fn read_file(root: &Path, rel: &str, maximum: usize) -> Option<Vec<u8>> {
    let root = solstone_core_journal_io::JournalRoot::open(root).ok()?;
    solstone_core_journal_io::read_relative_file_bounded(&root, Path::new(rel), maximum)
        .ok()?
        .map(|o| o.bytes)
}

/// Coordinates for an item attached to a particular turn, including a parent
/// already processed through another reply. Never follow a partial coordinate.
pub fn item_origin<'a>(
    attachment: &'a serde_json::Value,
    day: &'a str,
    segment: &'a str,
) -> Option<(&'a str, &'a str, &'a str)> {
    let id = attachment.get("item_ref")?.as_str()?;
    let (day, segment) = match (attachment.get("item_day"), attachment.get("item_segment")) {
        (None, None) => (day, segment),
        (Some(day), Some(segment)) => (day.as_str()?, segment.as_str()?),
        _ => return None,
    };
    (valid_id(id)
        && crate::segment::is_date_key(day)
        && crate::segment::segment_key(segment).as_deref() == Some(segment))
    .then_some((day, segment, id))
}

pub(super) const fn spec(pattern: &'static str, kind: Kind) -> super::FilePattern {
    use super::*;
    FilePattern {
        pattern,
        root: PatternRoot::DayRooted,
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        disposition: IndexDisposition::Admitted {
            // The durable classification table stores its historical bucket.
            // Effective search and fetch grants come from the settled kind.
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        kind: KindAssignment::Settled(kind),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT,
        condition: FileCondition::Path,
        segment: None,
    }
}
