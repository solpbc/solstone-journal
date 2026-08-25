// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable relocation of a segment directory to a new day or key.
//!
//! A segment move is a segment write, so the mutation lives behind this door
//! rather than in the CLI that plans and renders it. Four durable steps happen
//! here: the directory move, `events.jsonl` re-stamping, the successor marker
//! repoint, and the stream-tail repair. Planning, index maintenance, and
//! post-move verification stay with the caller.
//!
//! The directory move is the pivot. It either happens or nothing does, so its
//! failure is a refusal. Every later step can fail on its own without unwinding
//! the move, so each reports separately and the caller decides what that means.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use solstone_core_journal_io::{
    AtomicWriteOptions, DirEntryKind, JsonWriteOptions, LockOptions, PathOrDay, atomic_replace,
    ensure_directory, find_available_segment, iter_segments, list_dir_entries, read_text,
    remove_dir_all, remove_file, rename_within, write_json,
};

use crate::segment_dir::list_days;
use crate::stream_repair::{MarkerTail, RepairOutcome, repair_stream_tail_from_markers};

/// A relocation step that could not be completed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelocationError(String);

impl RelocationError {
    /// Describe a relocation step failure.
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for RelocationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for RelocationError {}

/// One end of a relocation: where a segment directory sits on disk.
#[derive(Clone, Debug)]
pub struct RelocationEnd {
    /// The `YYYYMMDD` day directory holding the segment.
    pub day: String,
    /// The `HHMMSS_LEN` segment key.
    pub segment: String,
    /// The absolute segment directory.
    pub path: PathBuf,
    /// The journal-relative segment directory.
    pub disk_rel: String,
    /// The journal-relative parent of the segment directory.
    pub parent_rel: String,
}

/// A planned relocation, ready to be applied.
pub struct Relocation<'a> {
    /// The journal root both ends are relative to.
    pub journal: &'a Path,
    /// Where the segment is now.
    pub source: &'a RelocationEnd,
    /// Where the segment is going.
    pub destination: &'a RelocationEnd,
    /// The successor whose chain marker points at `source`, when the caller's
    /// plan resolved exactly one. An absent or ambiguous chain is the caller's
    /// policy to report; this door patches only what it is handed.
    pub successor: Option<&'a RelocationEnd>,
    /// The stream named by the source marker, whose tail is repaired.
    pub stream: &'a str,
    /// The source marker sequence carried onto the destination.
    pub sequence: u64,
    /// How the stream record is locked during the tail repair.
    pub locks: LockOptions,
}

/// Why a relocation did not move the directory. Nothing was written.
#[derive(Debug)]
pub enum RelocationRefusal {
    /// The destination appeared between planning and the move.
    DestinationExists,
    /// The directory move itself failed.
    Failed(RelocationError),
}

/// What each durable step of an applied relocation did.
///
/// The directory move succeeded. Every field records a step taken after it.
#[derive(Debug)]
pub struct RelocationOutcome {
    /// Re-stamped `events.jsonl` object lines, or why none were re-stamped.
    pub events: Result<u64, RelocationError>,
    /// The successor marker repoint, when a successor was supplied.
    pub successor: Option<Result<(), RelocationError>>,
    /// The stream-tail repair.
    pub tail: RepairOutcome,
    /// Chronicle days whose stream structure was durably changed by this
    /// relocation. The caller advances health only for this exact set.
    pub mutated_days: BTreeSet<String>,
}

/// Choose an unused segment key under `parent`, claiming nothing.
///
/// Planning must know the key the move will land on before anything is
/// written, so the search lives with the mutation that consumes it rather than
/// with the caller that renders it.
pub fn available_segment_key(
    parent: &Path,
    candidate: &str,
    max_attempts: usize,
) -> Result<Option<String>, RelocationError> {
    find_available_segment(parent, candidate, max_attempts)
        .map_err(|error| RelocationError(error.to_string()))
}

/// Move a segment directory and re-author the identity that pointed at it.
pub fn relocate_segment(
    relocation: &Relocation<'_>,
) -> Result<RelocationOutcome, RelocationRefusal> {
    move_directory(relocation)?;
    let destination = relocation.destination;
    let mut mutated_days = BTreeSet::from([relocation.source.day.clone(), destination.day.clone()]);
    let events = rewrite_events(destination);
    let successor = relocation
        .successor
        .map(|end| patch_successor(end, &destination.day, &destination.segment));
    if successor.as_ref().is_some_and(Result::is_ok)
        && let Some(successor) = relocation.successor
    {
        mutated_days.insert(successor.day.clone());
    }
    let tail = repair_stream_tail_from_markers(
        relocation.journal,
        relocation.stream,
        &MarkerTail {
            last_day: &destination.day,
            last_segment: &destination.segment,
            max_seq: relocation.sequence,
        },
        relocation.locks,
    );
    Ok(RelocationOutcome {
        events,
        successor,
        tail,
        mutated_days,
    })
}

fn move_directory(relocation: &Relocation<'_>) -> Result<(), RelocationRefusal> {
    let journal = relocation.journal;
    let destination = relocation.destination;
    ensure_directory(&journal.join(&destination.parent_rel))
        .map_err(|error| RelocationRefusal::Failed(RelocationError(error.to_string())))?;
    // The caller's collision check can become stale while it reads markers and
    // the index. This narrows that window to check-to-syscall, but cannot
    // make rename atomic: rename(2) has no no-replace mode through this seam.
    if destination.path.exists() {
        return Err(RelocationRefusal::DestinationExists);
    }
    rename_within(journal, &relocation.source.disk_rel, &destination.disk_rel)
        .map_err(|error| RelocationRefusal::Failed(RelocationError(error.to_string())))
}

/// Re-stamp `day` and `segment` on every object line, leaving the rest alone.
///
/// Non-object and unparsable lines survive byte-for-byte: a move must not be a
/// silent opportunity to normalize content the mover does not understand.
fn rewrite_events(destination: &RelocationEnd) -> Result<u64, RelocationError> {
    let path = destination.path.join("events.jsonl");
    let text =
        read_text(&path, String::new()).map_err(|error| RelocationError(error.to_string()))?;
    if text.is_empty() {
        return Ok(0);
    }
    let mut lines = text.split('\n').collect::<Vec<_>>();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let mut rewritten = Vec::with_capacity(lines.len());
    let mut count = 0;
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            rewritten.push(line.to_owned());
            continue;
        }
        match serde_json::from_str::<Value>(trimmed) {
            Ok(mut value) if value.is_object() => {
                let object = value.as_object_mut().expect("object checked");
                object.insert("day".to_owned(), Value::String(destination.day.clone()));
                object.insert(
                    "segment".to_owned(),
                    Value::String(destination.segment.clone()),
                );
                rewritten.push(serde_json::to_string(&value).expect("value serializes"));
                count += 1;
            }
            Ok(_) | Err(_) => rewritten.push(line.to_owned()),
        }
    }
    let replacement = if rewritten.is_empty() {
        String::new()
    } else {
        format!("{}\n", rewritten.join("\n"))
    };
    atomic_replace(path, replacement.as_bytes(), AtomicWriteOptions::default())
        .map_err(|error| RelocationError(error.to_string()))?;
    Ok(count)
}

fn patch_successor(
    successor: &RelocationEnd,
    day: &str,
    segment: &str,
) -> Result<(), RelocationError> {
    let path = successor.path.join("stream.json");
    let text = read_text(&path, String::new())
        .map_err(|_| RelocationError::new("could not read marker"))?;
    let mut marker = serde_json::from_str::<Value>(&text)
        .map_err(|_| RelocationError::new("could not read marker"))?;
    let object = marker
        .as_object_mut()
        .ok_or_else(|| RelocationError::new("could not read marker"))?;
    object.insert("prev_day".to_owned(), Value::String(day.to_owned()));
    object.insert("prev_segment".to_owned(), Value::String(segment.to_owned()));
    write_json(path, &marker, JsonWriteOptions::default())
        .map_err(|error| RelocationError(error.to_string()))
}

/// Sidecar names the restructure migration treats as non-content.
///
/// Deliberately the three names Python's `solstone/think/segment_files.py`
/// defines, not this crate's wider seven-name [`crate::RESERVED_SEGMENT_FILENAMES`].
/// The wider set would classify a segment holding only `device.json`,
/// `events.jsonl`, or `tombstone.json` as empty and delete it, which the
/// migration being ported never does.
const RESTRUCTURE_NON_CONTENT_NAMES: [&str; 3] = ["stream.json", "ingest.json", "ingest.json.lock"];

/// Segment-root JSON outputs that always belong under the segment's `agents/`.
const KNOWN_SEGMENT_AGENT_JSON: [&str; 3] = ["activity_state.json", "facets.json", "speakers.json"];

/// What one day-to-stream restructure run found and did.
///
/// A missing marker is reported rather than returned as an error: the caller
/// prints the migration's refusal text and exits nonzero, and `Err` stays
/// reserved for genuine I/O failures.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SegmentRestructureReport {
    /// The journal already nests every segment under a stream directory.
    pub already_restructured: bool,
    /// Non-empty segments found as direct day children.
    pub total: usize,
    /// Empty segment directories found as direct day children.
    pub empty: usize,
    /// Non-empty direct-child segments with no usable `stream.json` marker.
    ///
    /// Any nonzero count means nothing was moved or removed.
    pub missing_markers: usize,
    /// Segments moved (or, when `dry_run`, planned).
    pub moved: usize,
    /// Empty segment directories removed (or planned).
    pub removed: usize,
    /// Distinct stream directories the moved segments landed in.
    pub streams: usize,
    /// Post-move nested segment count, absent on a dry run or a refusal.
    pub verified: Option<usize>,
    /// Whether the run planned only.
    pub dry_run: bool,
}

/// What one agent-layout migration run moved, deduplicated, and skipped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentLayoutMigrationReport {
    /// Files moved (or, when `dry_run`, planned).
    pub moved: usize,
    /// Sources removed because the destination already held identical bytes.
    pub cleaned: usize,
    /// Files left alone: unrecognized, or a differing destination already there.
    pub skipped: usize,
    /// Individual moves that failed without stopping the run.
    pub errors: usize,
}

/// Move every flat day-child segment under its marker's stream directory.
///
/// Preflight is total: when any non-empty direct-child segment lacks a usable
/// marker the run reports that count and changes nothing at all, so a partially
/// tagged journal can never be half-restructured.
pub fn restructure_segments_by_stream(
    journal: &Path,
    dry_run: bool,
) -> Result<SegmentRestructureReport, RelocationError> {
    let mut report = SegmentRestructureReport {
        dry_run,
        ..SegmentRestructureReport::default()
    };
    let days = list_days(journal).map_err(|error| RelocationError(error.to_string()))?;
    if days.is_empty() {
        return Ok(report);
    }

    let mut plan = Vec::new();
    let mut nested_seen = false;
    let mut flat_seen = false;
    for (_, day_dir) in &days {
        let (direct, nested) = split_day_segments(journal, day_dir)?;
        nested_seen |= !nested.is_empty();
        flat_seen |= !direct.is_empty();
        for segment in direct {
            if is_empty_segment(&segment)? {
                report.empty += 1;
                plan.push((segment, None));
                continue;
            }
            report.total += 1;
            let stream = marker_stream(&segment)?;
            if stream.is_none() {
                report.missing_markers += 1;
            }
            plan.push((segment, stream));
        }
    }

    // Python returns early only when nesting exists and nothing is still flat.
    if !flat_seen && nested_seen {
        report.already_restructured = true;
        return Ok(report);
    }
    if report.total == 0 {
        return Ok(report);
    }
    if report.missing_markers > 0 {
        return Ok(report);
    }

    let mut streams = BTreeSet::new();
    for (segment, stream) in plan {
        let Some(stream) = stream else {
            if !dry_run {
                remove_dir_all(journal, &relative(journal, &segment)?)
                    .map_err(|error| RelocationError(error.to_string()))?;
            }
            report.removed += 1;
            continue;
        };
        let parent = segment
            .parent()
            .ok_or_else(|| RelocationError::new("segment directory has no day parent"))?;
        let name = segment
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| RelocationError::new("segment directory has no name"))?;
        streams.insert(stream.clone());
        if !dry_run {
            ensure_directory(&parent.join(&stream))
                .map_err(|error| RelocationError(error.to_string()))?;
            // Python's `shutil.move` performs no destination pre-check here;
            // the restructure runs against a flat layout it has just proven
            // has no stream directory of this name holding this key.
            rename_within(
                journal,
                &relative(journal, &segment)?,
                &format!("{}/{stream}/{name}", relative(journal, parent)?),
            )
            .map_err(|error| RelocationError(error.to_string()))?;
        }
        report.moved += 1;
    }
    report.streams = streams.len();

    if !dry_run {
        let mut nested = 0;
        for (_, day_dir) in &days {
            nested += split_day_segments(journal, day_dir)?.1.len();
        }
        report.verified = Some(nested);
    }
    Ok(report)
}

/// Move legacy agent outputs into the `agents/` layout.
///
/// An occupied destination is never overwritten: identical bytes retire the
/// source, differing bytes leave both in place and are counted as skipped.
pub fn migrate_agent_layout(
    journal: &Path,
    dry_run: bool,
) -> Result<AgentLayoutMigrationReport, RelocationError> {
    let mut report = AgentLayoutMigrationReport::default();
    let facets = facet_names(journal)?;
    for (_, day_dir) in list_days(journal).map_err(|error| RelocationError(error.to_string()))? {
        let mut segments = iter_segments(journal, PathOrDay::Directory(&day_dir))
            .map_err(|error| RelocationError(error.to_string()))?
            .into_iter()
            .map(|segment| segment.path().to_path_buf())
            .collect::<Vec<_>>();
        segments.sort();
        for segment in segments {
            migrate_segment_outputs(journal, &segment, &facets, dry_run, &mut report)?;
        }
        migrate_daily_faceted_outputs(journal, &day_dir, &facets, dry_run, &mut report)?;
    }
    Ok(report)
}

/// Move one segment's root markdown and known JSON outputs into `agents/`.
fn migrate_segment_outputs(
    journal: &Path,
    segment: &Path,
    facets: &BTreeSet<String>,
    dry_run: bool,
    report: &mut AgentLayoutMigrationReport,
) -> Result<(), RelocationError> {
    let agents = segment.join("agents");
    let mut markdown = Vec::new();
    let mut json = Vec::new();
    for entry in list_dir_entries(segment).map_err(|error| RelocationError(error.to_string()))? {
        if entry.kind != DirEntryKind::File {
            continue;
        }
        let name = entry.name.to_string_lossy().into_owned();
        if name.ends_with(".md") {
            markdown.push(name);
        } else if name.ends_with(".json") {
            json.push(name);
        }
    }
    markdown.sort();
    json.sort();

    for name in markdown {
        move_file(
            journal,
            &segment.join(&name),
            &agents.join(&name),
            dry_run,
            report,
        )?;
    }
    for name in json {
        if KNOWN_SEGMENT_AGENT_JSON.contains(&name.as_str()) {
            move_file(
                journal,
                &segment.join(&name),
                &agents.join(&name),
                dry_run,
                report,
            )?;
            continue;
        }
        if let Some(facet) = name
            .strip_prefix("activity_state_")
            .and_then(|rest| rest.strip_suffix(".json"))
            .filter(|facet| facets.contains(*facet))
        {
            move_file(
                journal,
                &segment.join(&name),
                &agents.join(facet).join("activity_state.json"),
                dry_run,
                report,
            )?;
            continue;
        }
        report.skipped += 1;
    }
    Ok(())
}

/// Move `agents/<topic>_<facet>.<ext>` into `agents/<facet>/<topic>.<ext>`.
fn migrate_daily_faceted_outputs(
    journal: &Path,
    day_dir: &Path,
    facets: &BTreeSet<String>,
    dry_run: bool,
    report: &mut AgentLayoutMigrationReport,
) -> Result<(), RelocationError> {
    let agents = day_dir.join("agents");
    if !agents.is_dir() {
        return Ok(());
    }
    // Longest facet name first so `notes_work_travel` cannot be claimed by the
    // shorter `travel` when `work_travel` also exists.
    let mut ordered = facets.iter().cloned().collect::<Vec<_>>();
    ordered.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));

    let mut names = list_dir_entries(&agents)
        .map_err(|error| RelocationError(error.to_string()))?
        .into_iter()
        .filter(|entry| entry.kind == DirEntryKind::File)
        .map(|entry| entry.name.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();

    for name in names {
        let Some((stem, extension)) = split_agent_extension(&name) else {
            continue;
        };
        let matched = ordered.iter().find_map(|facet| {
            stem.strip_suffix(&format!("_{facet}"))
                .filter(|topic| !topic.is_empty())
                .map(|topic| (facet.as_str(), topic))
        });
        let Some((facet, topic)) = matched else {
            report.skipped += 1;
            continue;
        };
        move_file(
            journal,
            &agents.join(&name),
            &agents.join(facet).join(format!("{topic}{extension}")),
            dry_run,
            report,
        )?;
    }
    Ok(())
}

/// Split a `.md`/`.json` filename into stem and dotted extension.
fn split_agent_extension(name: &str) -> Option<(&str, &str)> {
    [".md", ".json"]
        .into_iter()
        .find_map(|extension| name.strip_suffix(extension).map(|stem| (stem, extension)))
}

/// Move one file, retiring an identical source and never overwriting.
fn move_file(
    journal: &Path,
    source: &Path,
    destination: &Path,
    dry_run: bool,
    report: &mut AgentLayoutMigrationReport,
) -> Result<(), RelocationError> {
    if destination.exists() {
        let identical = match (fs::read(source), fs::read(destination)) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        };
        if identical {
            if !dry_run {
                remove_file(journal, &relative(journal, source)?)
                    .map_err(|error| RelocationError(error.to_string()))?;
            }
            report.cleaned += 1;
        } else {
            report.skipped += 1;
        }
        return Ok(());
    }
    if dry_run {
        report.moved += 1;
        return Ok(());
    }
    let outcome = destination
        .parent()
        .ok_or_else(|| RelocationError::new("destination has no parent"))
        .and_then(|parent| {
            ensure_directory(parent).map_err(|error| RelocationError(error.to_string()))
        })
        .and_then(|()| {
            rename_within(
                journal,
                &relative(journal, source)?,
                &relative(journal, destination)?,
            )
            .map_err(|error| RelocationError(error.to_string()))
        });
    // One unmovable file is counted and stepped over, exactly as the migration
    // being ported does; the rest of the journal still migrates.
    match outcome {
        Ok(()) => report.moved += 1,
        Err(_) => report.errors += 1,
    }
    Ok(())
}

/// Every facet directory name, or none when the journal has no `facets/`.
fn facet_names(journal: &Path) -> Result<BTreeSet<String>, RelocationError> {
    Ok(list_dir_entries(&journal.join("facets"))
        .map_err(|error| RelocationError(error.to_string()))?
        .into_iter()
        .filter(|entry| entry.kind == DirEntryKind::Directory)
        .map(|entry| entry.name.to_string_lossy().into_owned())
        .collect())
}

/// Split one day's segments into direct children and stream-nested ones.
fn split_day_segments(
    journal: &Path,
    day_dir: &Path,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>), RelocationError> {
    let mut direct = Vec::new();
    let mut nested = Vec::new();
    for segment in iter_segments(journal, PathOrDay::Directory(day_dir))
        .map_err(|error| RelocationError(error.to_string()))?
    {
        if segment.path().parent() == Some(day_dir) {
            direct.push(segment.path().to_path_buf());
        } else {
            nested.push(segment.path().to_path_buf());
        }
    }
    direct.sort();
    nested.sort();
    Ok((direct, nested))
}

/// Whether a segment directory holds no content file.
fn is_empty_segment(segment: &Path) -> Result<bool, RelocationError> {
    Ok(!list_dir_entries(segment)
        .map_err(|error| RelocationError(error.to_string()))?
        .into_iter()
        .any(|entry| {
            entry.kind == DirEntryKind::File
                && !RESTRUCTURE_NON_CONTENT_NAMES.contains(&entry.name.to_string_lossy().as_ref())
        }))
}

/// The usable stream a segment's marker names, if it has one.
///
/// A marker naming a stream that is not a plain path component is reported as
/// unusable rather than moved: the migration must not be a way to write outside
/// the day directory.
fn marker_stream(segment: &Path) -> Result<Option<String>, RelocationError> {
    let text = read_text(segment.join("stream.json"), String::new())
        .map_err(|error| RelocationError(error.to_string()))?;
    let Ok(marker) = serde_json::from_str::<Value>(&text) else {
        return Ok(None);
    };
    Ok(marker
        .get("stream")
        .and_then(Value::as_str)
        .filter(|stream| crate::is_safe_stream_component(stream))
        .map(str::to_owned))
}

/// The journal-relative form of a path inside the journal.
fn relative(journal: &Path, path: &Path) -> Result<String, RelocationError> {
    path.strip_prefix(journal)
        .ok()
        .and_then(|rel| rel.to_str())
        .map(str::to_owned)
        .ok_or_else(|| RelocationError::new(format!("path is outside the journal: {path:?}")))
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;
    use crate::test_support::TempDir;

    struct Fixture {
        root: TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                root: TempDir::new(),
            }
        }

        fn path(&self) -> &Path {
            self.root.path()
        }

        fn end(&self, day: &str, stream: &str, segment: &str) -> RelocationEnd {
            let (disk_rel, parent_rel) = if stream == solstone_core_journal_io::DEFAULT_STREAM {
                (
                    format!("chronicle/{day}/{segment}"),
                    format!("chronicle/{day}"),
                )
            } else {
                (
                    format!("chronicle/{day}/{stream}/{segment}"),
                    format!("chronicle/{day}/{stream}"),
                )
            };
            RelocationEnd {
                day: day.to_owned(),
                segment: segment.to_owned(),
                path: self.path().join(&disk_rel),
                disk_rel,
                parent_rel,
            }
        }

        fn create(&self, end: &RelocationEnd, marker: Value) -> &Self {
            fs::create_dir_all(&end.path).expect("segment directory");
            fs::write(
                end.path.join("stream.json"),
                serde_json::to_string(&marker).expect("marker serializes"),
            )
            .expect("marker written");
            self
        }

        fn stream_state(&self, stream: &str, day: &str, segment: &str, seq: u64) -> PathBuf {
            let path = self.path().join("streams").join(format!("{stream}.json"));
            fs::create_dir_all(path.parent().expect("streams parent")).expect("streams directory");
            fs::write(
                &path,
                serde_json::to_string(&json!({
                    "name": stream, "kind": "capture", "host": "desk", "platform": "linux",
                    "created_at": 7, "last_day": day, "last_segment": segment, "seq": seq,
                    // Tail repair never rewrites identity keys; a legacy "did" stays "did".
                    "did": "device-1", "source": "microphone", "unknown": {"kept": true}
                }))
                .expect("state serializes"),
            )
            .expect("state written");
            path
        }
    }

    fn relocation<'a>(
        journal: &'a Path,
        source: &'a RelocationEnd,
        destination: &'a RelocationEnd,
        successor: Option<&'a RelocationEnd>,
        stream: &'a str,
        sequence: u64,
    ) -> Relocation<'a> {
        Relocation {
            journal,
            source,
            destination,
            successor,
            stream,
            sequence,
            locks: LockOptions::default(),
        }
    }

    #[test]
    fn all_four_durable_steps_land_and_preserve_unowned_marker_and_event_content() {
        let fixture = Fixture::new();
        let source = fixture.end("20260304", "_default", "090000_60");
        let destination = fixture.end("20260305", "_default", "090000_60");
        let successor = fixture.end("20260307", "_default", "100000_60");
        fixture.create(&source, json!({"stream": "workstation", "seq": 5}));
        fixture.create(
            &successor,
            json!({"stream": "workstation", "seq": 6, "prev_day": "20260304",
                "prev_segment": "090000_60", "unknown": "kept"}),
        );
        fs::write(
            source.path.join("events.jsonl"),
            "{\"day\":\"20260304\",\"segment\":\"090000_60\"}\nnot json\n\n",
        )
        .expect("events written");
        let state = fixture.stream_state("workstation", "20260304", "090000_60", 5);
        let before: Value =
            serde_json::from_str(&fs::read_to_string(&state).expect("state read")).expect("state");

        let outcome = relocate_segment(&relocation(
            fixture.path(),
            &source,
            &destination,
            Some(&successor),
            "workstation",
            5,
        ))
        .expect("relocation applies");

        assert!(!source.path.exists());
        assert!(destination.path.is_dir());
        assert_eq!(outcome.events, Ok(1));
        let events =
            fs::read_to_string(destination.path.join("events.jsonl")).expect("events read");
        assert!(events.contains("\"day\":\"20260305\""));
        assert!(events.contains("not json\n\n"));
        assert_eq!(outcome.successor, Some(Ok(())));
        let patched: Value = serde_json::from_str(
            &fs::read_to_string(successor.path.join("stream.json")).expect("marker read"),
        )
        .expect("marker");
        assert_eq!(patched["prev_day"], "20260305");
        assert_eq!(patched["prev_segment"], "090000_60");
        assert_eq!(patched["unknown"], "kept");
        assert!(matches!(
            outcome.tail,
            RepairOutcome::Repaired | RepairOutcome::Unchanged(_)
        ));
        let repaired: Value =
            serde_json::from_str(&fs::read_to_string(&state).expect("state read")).expect("state");
        // Tail repair patches last_day/last_segment/seq in place; "did" is preserved verbatim.
        for key in [
            "did",
            "source",
            "created_at",
            "kind",
            "host",
            "platform",
            "unknown",
        ] {
            assert_eq!(
                repaired.get(key),
                before.get(key),
                "{key} was not preserved"
            );
        }
        assert_eq!(repaired["last_day"], "20260305");
    }

    #[test]
    fn a_destination_appearing_after_planning_refuses_without_touching_either_segment() {
        let fixture = Fixture::new();
        let source = fixture.end("20260304", "work", "090000_60");
        let destination = fixture.end("20260305", "work", "090000_60");
        fixture.create(&source, json!({"stream": "work", "seq": 1}));
        fixture.create(&destination, json!({"sentinel": true}));
        let source_before = fs::read(source.path.join("stream.json")).expect("source read");

        let refusal = relocate_segment(&relocation(
            fixture.path(),
            &source,
            &destination,
            None,
            "work",
            1,
        ))
        .expect_err("collision refuses");

        assert!(matches!(refusal, RelocationRefusal::DestinationExists));
        assert!(source.path.is_dir());
        assert_eq!(
            fs::read(source.path.join("stream.json")).expect("source read"),
            source_before
        );
        let held: Value = serde_json::from_str(
            &fs::read_to_string(destination.path.join("stream.json")).expect("destination read"),
        )
        .expect("marker");
        assert_eq!(held["sentinel"], true);
    }

    #[test]
    fn a_missing_source_refuses_and_leaves_the_created_destination_parent_empty() {
        let fixture = Fixture::new();
        let source = fixture.end("20260304", "work", "090000_60");
        let destination = fixture.end("20260305", "work", "090000_60");

        let refusal = relocate_segment(&relocation(
            fixture.path(),
            &source,
            &destination,
            None,
            "work",
            1,
        ))
        .expect_err("absent source refuses");

        assert!(matches!(refusal, RelocationRefusal::Failed(_)));
        assert!(!destination.path.exists());
    }

    #[test]
    fn an_absent_successor_is_not_patched_and_a_malformed_one_reports_without_unwinding() {
        let fixture = Fixture::new();
        let source = fixture.end("20260304", "work", "090000_60");
        let destination = fixture.end("20260305", "work", "090000_60");
        fixture.create(&source, json!({"stream": "work", "seq": 1}));
        let outcome = relocate_segment(&relocation(
            fixture.path(),
            &source,
            &destination,
            None,
            "work",
            1,
        ))
        .expect("relocation applies");
        assert_eq!(outcome.successor, None);
        assert!(destination.path.is_dir());

        let fixture = Fixture::new();
        let source = fixture.end("20260304", "work", "090000_60");
        let destination = fixture.end("20260305", "work", "090000_60");
        let successor = fixture.end("20260306", "work", "100000_60");
        fixture.create(&source, json!({"stream": "work", "seq": 1}));
        fs::create_dir_all(&successor.path).expect("successor directory");
        fs::write(successor.path.join("stream.json"), "{ not json").expect("marker written");

        let outcome = relocate_segment(&relocation(
            fixture.path(),
            &source,
            &destination,
            Some(&successor),
            "work",
            1,
        ))
        .expect("relocation applies");

        assert_eq!(
            outcome.successor,
            Some(Err(RelocationError::new("could not read marker")))
        );
        assert!(destination.path.is_dir(), "the move is not unwound");
    }

    #[test]
    fn a_stream_without_a_record_reports_no_record_after_the_directory_has_moved() {
        let fixture = Fixture::new();
        let source = fixture.end("20260304", "work", "090000_60");
        let destination = fixture.end("20260305", "work", "090000_60");
        fixture.create(&source, json!({"stream": "work", "seq": 1}));

        let outcome = relocate_segment(&relocation(
            fixture.path(),
            &source,
            &destination,
            None,
            "work",
            1,
        ))
        .expect("relocation applies");

        assert_eq!(outcome.tail, RepairOutcome::NoRecord);
        assert!(destination.path.is_dir());
    }

    /// Build one flat day-child segment with an optional marker and content.
    fn flat_segment(root: &Path, day: &str, key: &str, stream: Option<&str>, content: bool) {
        let dir = root.join("chronicle").join(day).join(key);
        fs::create_dir_all(&dir).expect("segment directory");
        if let Some(stream) = stream {
            fs::write(
                dir.join("stream.json"),
                serde_json::to_string(&json!({"stream": stream, "seq": 1})).expect("marker"),
            )
            .expect("marker written");
        }
        if content {
            fs::write(dir.join("audio.jsonl"), "{\"kind\":\"audio\"}\n").expect("content written");
        }
    }

    #[test]
    fn every_flat_segment_moves_under_its_marker_stream_and_empty_ones_are_removed() {
        let fixture = Fixture::new();
        let root = fixture.path();
        flat_segment(root, "20260304", "090000_60", Some("work"), true);
        flat_segment(root, "20260304", "100000_60", Some("home"), true);
        flat_segment(root, "20260305", "110000_60", Some("work"), true);
        // Only a marker and no content file: this directory is empty.
        flat_segment(root, "20260305", "120000_60", Some("work"), false);

        let report = restructure_segments_by_stream(root, false).expect("restructure runs");

        assert_eq!(report.total, 3);
        assert_eq!(report.empty, 1);
        assert_eq!(report.moved, 3);
        assert_eq!(report.removed, 1);
        assert_eq!(report.streams, 2, "work and home");
        assert_eq!(report.verified, Some(3));
        assert_eq!(report.missing_markers, 0);
        for (day, stream, key) in [
            ("20260304", "work", "090000_60"),
            ("20260304", "home", "100000_60"),
            ("20260305", "work", "110000_60"),
        ] {
            assert!(
                root.join("chronicle")
                    .join(day)
                    .join(stream)
                    .join(key)
                    .is_dir(),
                "{day}/{stream}/{key} did not land"
            );
            assert!(!root.join("chronicle").join(day).join(key).exists());
        }
        assert!(!root.join("chronicle/20260305/120000_60").exists());
    }

    #[test]
    fn one_missing_marker_refuses_the_whole_run_before_anything_moves_or_is_removed() {
        let fixture = Fixture::new();
        let root = fixture.path();
        flat_segment(root, "20260304", "090000_60", Some("work"), true);
        flat_segment(root, "20260304", "100000_60", None, true);
        // An empty segment would otherwise have been deleted by this run.
        flat_segment(root, "20260304", "110000_60", Some("work"), false);

        let report = restructure_segments_by_stream(root, false).expect("restructure runs");

        assert_eq!(report.missing_markers, 1);
        assert_eq!(report.total, 2);
        assert_eq!(report.moved, 0);
        assert_eq!(report.removed, 0, "refusal precedes every deletion");
        assert_eq!(report.verified, None);
        for key in ["090000_60", "100000_60", "110000_60"] {
            assert!(
                root.join("chronicle/20260304").join(key).is_dir(),
                "{key} was disturbed by a refused run"
            );
        }
        assert!(!root.join("chronicle/20260304/work").exists());
    }

    #[test]
    fn a_marker_naming_an_unusable_stream_is_a_refusal_not_a_move_outside_the_day() {
        let fixture = Fixture::new();
        let root = fixture.path();
        flat_segment(root, "20260304", "090000_60", Some("../escaped"), true);

        let report = restructure_segments_by_stream(root, false).expect("restructure runs");

        assert_eq!(report.missing_markers, 1);
        assert_eq!(report.moved, 0);
        assert!(root.join("chronicle/20260304/090000_60").is_dir());
        assert!(!root.join("escaped").exists());
    }

    #[test]
    fn a_dry_run_plans_the_same_counts_and_leaves_the_flat_layout_intact() {
        let fixture = Fixture::new();
        let root = fixture.path();
        flat_segment(root, "20260304", "090000_60", Some("work"), true);
        flat_segment(root, "20260304", "100000_60", Some("work"), false);

        let report = restructure_segments_by_stream(root, true).expect("restructure plans");

        assert!(report.dry_run);
        assert_eq!(report.moved, 1);
        assert_eq!(report.removed, 1);
        assert_eq!(report.verified, None, "nothing to verify on a plan");
        assert!(root.join("chronicle/20260304/090000_60").is_dir());
        assert!(root.join("chronicle/20260304/100000_60").is_dir());
        assert!(!root.join("chronicle/20260304/work").exists());
    }

    #[test]
    fn an_already_nested_journal_reports_no_work_and_an_empty_one_reports_nothing() {
        let fixture = Fixture::new();
        let nested = fixture.path().join("chronicle/20260304/work/090000_60");
        fs::create_dir_all(&nested).expect("nested segment");
        fs::write(nested.join("audio.jsonl"), "{}\n").expect("content written");

        let report = restructure_segments_by_stream(fixture.path(), false).expect("runs");

        assert!(report.already_restructured);
        assert_eq!(report.moved, 0);
        assert_eq!(report.total, 0);

        let empty = Fixture::new();
        let report = restructure_segments_by_stream(empty.path(), false).expect("runs");
        assert_eq!(report, SegmentRestructureReport::default());
    }

    #[test]
    fn segment_and_daily_agent_outputs_move_into_the_agents_layout() {
        let fixture = Fixture::new();
        let root = fixture.path();
        fs::create_dir_all(root.join("facets/work")).expect("facet directory");
        fs::create_dir_all(root.join("facets/home")).expect("facet directory");
        let segment = root.join("chronicle/20260304/090000_60");
        fs::create_dir_all(&segment).expect("segment directory");
        fs::write(segment.join("summary.md"), "body").expect("markdown written");
        fs::write(segment.join("facets.json"), "{}").expect("known json written");
        fs::write(segment.join("activity_state_work.json"), "{\"a\":1}").expect("faceted written");
        fs::write(segment.join("activity_state_absent.json"), "{}").expect("unknown facet");
        fs::write(segment.join("ingest.json"), "{}").expect("unrelated json");
        let day_agents = root.join("chronicle/20260304/agents");
        fs::create_dir_all(&day_agents).expect("day agents directory");
        fs::write(day_agents.join("digest_work.md"), "digest").expect("faceted markdown");
        fs::write(day_agents.join("notes_home.json"), "{}").expect("faceted json");
        fs::write(day_agents.join("orphan.md"), "orphan").expect("unmatched markdown");

        let report = migrate_agent_layout(root, false).expect("migration runs");

        assert_eq!(report.moved, 5);
        assert_eq!(report.cleaned, 0);
        assert_eq!(
            report.skipped, 3,
            "the unknown-facet state JSON, `ingest.json`, and the unmatched daily file"
        );
        assert_eq!(report.errors, 0);
        assert_eq!(
            fs::read_to_string(segment.join("agents/summary.md")).expect("moved markdown"),
            "body"
        );
        assert!(segment.join("agents/facets.json").is_file());
        assert_eq!(
            fs::read_to_string(segment.join("agents/work/activity_state.json"))
                .expect("faceted state"),
            "{\"a\":1}"
        );
        assert!(!segment.join("summary.md").exists());
        assert!(
            segment.join("activity_state_absent.json").is_file(),
            "an unknown facet suffix is left alone"
        );
        assert!(segment.join("ingest.json").is_file());
        assert_eq!(
            fs::read_to_string(day_agents.join("work/digest.md")).expect("daily markdown"),
            "digest"
        );
        assert!(day_agents.join("home/notes.json").is_file());
        assert!(day_agents.join("orphan.md").is_file());
    }

    #[test]
    fn an_identical_destination_retires_the_source_and_a_differing_one_keeps_both() {
        let fixture = Fixture::new();
        let root = fixture.path();
        let segment = root.join("chronicle/20260304/090000_60");
        fs::create_dir_all(segment.join("agents")).expect("agents directory");
        fs::write(segment.join("same.md"), "shared").expect("source written");
        fs::write(segment.join("agents/same.md"), "shared").expect("identical destination");
        fs::write(segment.join("differs.md"), "source").expect("source written");
        fs::write(segment.join("agents/differs.md"), "destination").expect("other destination");

        let report = migrate_agent_layout(root, false).expect("migration runs");

        assert_eq!(report.cleaned, 1);
        assert_eq!(report.skipped, 1);
        assert_eq!(report.moved, 0);
        assert!(
            !segment.join("same.md").exists(),
            "identical source retired"
        );
        assert_eq!(
            fs::read_to_string(segment.join("differs.md")).expect("source retained"),
            "source"
        );
        assert_eq!(
            fs::read_to_string(segment.join("agents/differs.md")).expect("destination retained"),
            "destination",
            "a differing destination is never overwritten"
        );
    }

    #[test]
    fn an_agent_layout_dry_run_counts_the_same_work_and_writes_nothing() {
        let fixture = Fixture::new();
        let root = fixture.path();
        let segment = root.join("chronicle/20260304/090000_60");
        fs::create_dir_all(segment.join("agents")).expect("agents directory");
        fs::write(segment.join("summary.md"), "body").expect("source written");
        fs::write(segment.join("same.md"), "shared").expect("source written");
        fs::write(segment.join("agents/same.md"), "shared").expect("identical destination");

        let report = migrate_agent_layout(root, true).expect("migration plans");

        assert_eq!(report.moved, 1);
        assert_eq!(report.cleaned, 1);
        assert!(segment.join("summary.md").is_file());
        assert!(segment.join("same.md").is_file(), "no source was retired");
        assert!(!segment.join("agents/summary.md").exists());
    }

    #[test]
    fn the_longest_facet_name_claims_a_daily_file_whose_stem_ends_in_both() {
        let fixture = Fixture::new();
        let root = fixture.path();
        fs::create_dir_all(root.join("facets/travel")).expect("facet directory");
        fs::create_dir_all(root.join("facets/work_travel")).expect("facet directory");
        let day_agents = root.join("chronicle/20260304/agents");
        fs::create_dir_all(&day_agents).expect("day agents directory");
        fs::write(day_agents.join("notes_work_travel.md"), "notes").expect("faceted markdown");

        let report = migrate_agent_layout(root, false).expect("migration runs");

        assert_eq!(report.moved, 1);
        assert!(
            day_agents.join("work_travel/notes.md").is_file(),
            "the longer facet name wins the suffix"
        );
        assert!(!day_agents.join("travel/notes_work.md").exists());
    }

    #[test]
    fn key_search_returns_the_candidate_when_free_and_claims_nothing_either_way() {
        let fixture = Fixture::new();
        let parent = fixture.path().join("chronicle/20260305/work");
        fs::create_dir_all(&parent).expect("parent directory");

        assert_eq!(
            available_segment_key(&parent, "090000_60", 100).expect("search runs"),
            Some("090000_60".to_owned())
        );
        assert!(!parent.join("090000_60").exists(), "nothing was claimed");

        fs::create_dir_all(parent.join("090000_60")).expect("occupied key");
        let chosen = available_segment_key(&parent, "090000_60", 100)
            .expect("search runs")
            .expect("a free key exists");
        assert_ne!(chosen, "090000_60");
        assert!(!parent.join(&chosen).exists(), "nothing was claimed");

        assert!(available_segment_key(&parent, "not-a-key", 100).is_err());
    }
}
