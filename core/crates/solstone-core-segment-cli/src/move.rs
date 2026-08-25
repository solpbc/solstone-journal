// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use serde_json::Value;
use solstone_core_indexer_store::{
    db::StreamPruneCounts,
    scan::{RescanFileStatus, rescan_file},
};
use solstone_core_segment::{
    DEFAULT_STREAM, DirEntryKind, LockOptions, Relocation, RelocationOutcome, RelocationRefusal,
    RepairOutcome, available_segment_key, list_dir_entries, read_text, relocate_segment,
    touch_stream_health_marker,
};

use crate::index::{SegmentIndexStatus, read_segment_index};
use crate::location::SegmentLocation;
use crate::read::{checks, read_marker, render_checks, successors};

#[derive(Debug)]
pub(crate) struct MovePlan {
    pub(crate) source: SegmentLocation,
    pub(crate) destination: SegmentLocation,
    pub(crate) marker: Value,
    pub(crate) marker_stream: String,
    pub(crate) successors: Vec<SegmentLocation>,
    pub(crate) events: u64,
    pub(crate) index: SegmentIndexStatus,
}

#[derive(Debug)]
pub(crate) enum MoveRefusal {
    Message(String),
}

impl MoveRefusal {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Message(message) => message,
        }
    }
}

/// The seam this crate orchestrates over.
///
/// The four durable journal writes of a move are one call, owned by the segment
/// write door. What remains here is index work, which this crate owns outright.
pub(crate) trait SegmentOperations {
    fn relocate(&self, relocation: &Relocation<'_>)
    -> Result<RelocationOutcome, RelocationRefusal>;
    fn prune(&self, journal: &Path, rel: &str) -> Result<Option<StreamPruneCounts>, String>;
    fn rescan(&self, journal: &Path, file: &Path) -> Result<RescanFileStatus, String>;
    fn touch_health(&self, journal: &Path, day: &str) -> Result<(), String>;
}

pub(crate) struct NativeOperations;

impl SegmentOperations for NativeOperations {
    fn relocate(
        &self,
        relocation: &Relocation<'_>,
    ) -> Result<RelocationOutcome, RelocationRefusal> {
        relocate_segment(relocation)
    }

    fn prune(&self, journal: &Path, rel: &str) -> Result<Option<StreamPruneCounts>, String> {
        solstone_core_indexer_store::db::prune_by_paths(journal, &[rel])
            .map_err(|error| error.to_string())
    }

    fn rescan(&self, journal: &Path, file: &Path) -> Result<RescanFileStatus, String> {
        rescan_file(journal, file).map_err(|error| error.to_string())
    }

    fn touch_health(&self, journal: &Path, day: &str) -> Result<(), String> {
        touch_stream_health_marker(journal, day).map_err(|error| error.to_string())
    }
}

pub(crate) fn build_plan(
    journal: &Path,
    source: SegmentLocation,
    to_day: &str,
    to_time: Option<&str>,
) -> Result<MovePlan, MoveRefusal> {
    if !to_day.bytes().all(|byte| byte.is_ascii_digit()) || to_day.len() != 8 {
        return Err(MoveRefusal::Message(format!(
            "Invalid --to-day format: {to_day} (expected YYYYMMDD)"
        )));
    }
    let mut destination_segment = source.segment.clone();
    if let Some(time) = to_time {
        if time.len() != 6 || !time.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(MoveRefusal::Message(format!(
                "Invalid --to-time format: {time} (expected HHMMSS)"
            )));
        }
        let Some((_, duration)) = source.segment.split_once('_') else {
            return Err(MoveRefusal::Message(
                "Source segment key has no duration".to_owned(),
            ));
        };
        destination_segment = format!("{time}_{duration}");
    }
    if source.day == to_day && source.segment == destination_segment {
        return Err(MoveRefusal::Message(
            "Source and destination are the same".to_owned(),
        ));
    }
    let mut destination =
        SegmentLocation::resolve(journal, to_day, &source.stream, &destination_segment)
            .map_err(|error| MoveRefusal::Message(error.to_string()))?;
    if destination.path.exists() {
        if to_time.is_none() {
            return Err(MoveRefusal::Message(format!(
                "Segment {destination_segment} already exists on {to_day}/{}. Use --to-time to specify an alternate time.",
                source.stream
            )));
        }
        let parent = journal.join(&destination.parent_rel);
        let available = available_segment_key(&parent, &destination_segment, 100)
            .map_err(|error| MoveRefusal::Message(error.to_string()))?;
        let Some(available) = available else {
            return Err(MoveRefusal::Message(format!(
                "No available segment slot near {destination_segment} on {to_day}"
            )));
        };
        destination = SegmentLocation::resolve(journal, to_day, &source.stream, &available)
            .map_err(|error| MoveRefusal::Message(error.to_string()))?;
    }
    let Some(marker) = read_marker(&source.path)
        .map_err(|_| MoveRefusal::Message("No stream.json in source segment".to_owned()))?
    else {
        return Err(MoveRefusal::Message(
            "No stream.json in source segment".to_owned(),
        ));
    };
    let marker_stream = marker
        .get("stream")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            MoveRefusal::Message(format!(
                "Stream mismatch: path says '{}' but stream.json says '{:?}'",
                source.stream,
                marker.get("stream")
            ))
        })?
        .to_owned();
    if source.stream != DEFAULT_STREAM && marker_stream != source.stream {
        return Err(MoveRefusal::Message(format!(
            "Stream mismatch: path says '{}' but stream.json says '{marker_stream}'",
            source.stream
        )));
    }
    let events = read_text(source.path.join("events.jsonl"), String::new())
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count() as u64;
    let index = read_segment_index(journal, &source.index_rel);
    let successors = successors(journal, &source.day, &marker_stream, &source.segment)
        .map_err(MoveRefusal::Message)?;
    Ok(MovePlan {
        source,
        destination,
        marker,
        marker_stream,
        successors,
        events,
        index,
    })
}

pub(crate) fn render_plan(plan: &MovePlan) -> String {
    let mut output = format!(
        "Move: {} -> {}\n  events.jsonl lines: {}\n",
        plan.source.token(),
        plan.destination.token(),
        plan.events
    );
    match plan.successors.as_slice() {
        [] => output.push_str("  successor to patch: (none - stream tail)\n"),
        [successor] => output.push_str(&format!("  successor to patch: {}\n", successor.token())),
        many => output.push_str(&format!(
            "  successor to patch: (ambiguous: {} successors)\n",
            many.len()
        )),
    }
    match &plan.index {
        SegmentIndexStatus::Ready { chunks, .. } => output.push_str(&format!("  index chunks: {chunks}\n")),
        SegmentIndexStatus::Unreadable { error } => output.push_str(&format!("  index read error: {error} (delete+reindex will be attempted; run: journal indexer --rescan)\n")),
        SegmentIndexStatus::Absent => {}
    }
    output.push_str(&format!(
        "  health markers: {}, {}\n",
        plan.source.day, plan.destination.day
    ));
    output
}

pub(crate) struct MoveExecution {
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) exit_code: i32,
}

pub(crate) fn execute_plan(
    journal: &Path,
    plan: &MovePlan,
    verbose: bool,
    locks: LockOptions,
    operations: &dyn SegmentOperations,
) -> MoveExecution {
    let mut stdout = "\nExecuting move...\n".to_owned();
    let mut stderr = String::new();
    let source = plan.source.relocation_end();
    let destination = plan.destination.relocation_end();
    // An ambiguous chain is a planning verdict, not a write: hand the door only
    // the successor it should patch and report the ambiguity here.
    let successor = match plan.successors.as_slice() {
        [successor] => Some(successor.relocation_end()),
        _ => None,
    };
    let outcome = match operations.relocate(&Relocation {
        journal,
        source: &source,
        destination: &destination,
        successor: successor.as_ref(),
        stream: &plan.marker_stream,
        sequence: plan.marker.get("seq").and_then(Value::as_u64).unwrap_or(0),
        locks,
    }) {
        Ok(outcome) => outcome,
        Err(RelocationRefusal::DestinationExists) => {
            return MoveExecution {
                stdout,
                stderr: format!(
                    "Destination {} already exists; no changes made\n",
                    plan.destination.token()
                ),
                exit_code: 1,
            };
        }
        Err(RelocationRefusal::Failed(error)) => {
            return MoveExecution {
                stdout,
                stderr: format!(
                    "step 1 directory move failed: {error}; source and index remain authoritative\n"
                ),
                exit_code: 3,
            };
        }
    };
    if verbose {
        stdout.push_str(&format!(
            "  created directory: {}\n",
            journal.join(&plan.destination.parent_rel).display()
        ));
    }
    stdout.push_str(&format!(
        "  moved directory: {} -> {}\n",
        plan.source.segment, plan.destination.segment
    ));
    let mut failures = PostMoveFailures::default();
    match outcome.events {
        Ok(count) if count > 0 => stdout.push_str(&format!(
            "  rewrote {count} events.jsonl lines (day: {}->{}, segment: {}->{})\n",
            plan.source.day, plan.destination.day, plan.source.segment, plan.destination.segment
        )),
        Ok(_) if verbose => stdout.push_str("  no events.jsonl to rewrite\n"),
        Ok(_) => {}
        Err(error) => failures.push(&mut stderr, 2, "events rewrite", error.to_string()),
    }
    match plan.successors.as_slice() {
        [] if verbose => stdout.push_str("  no successor to patch (stream tail)\n"),
        [] => {}
        [successor] => match outcome.successor {
            Some(Err(error)) => failures.push(&mut stderr, 3, "successor patch", error.to_string()),
            _ => stdout.push_str(&format!("  patched successor {}\n", successor.token())),
        },
        successors => failures.push(
            &mut stderr,
            3,
            "successor patch",
            format!(
                "ambiguous chain: {} successors left unpatched",
                successors.len()
            ),
        ),
    }
    match outcome.tail {
        RepairOutcome::Repaired | RepairOutcome::Unchanged(_) => {
            stdout.push_str(&format!("  rebuilt stream state: {}\n", plan.marker_stream))
        }
        RepairOutcome::NoRecord => failures.push(
            &mut stderr,
            4,
            "stream-tail repair",
            "no stream record; identity was not re-authored".to_owned(),
        ),
        RepairOutcome::Malformed => failures.push(
            &mut stderr,
            4,
            "stream-tail repair",
            "malformed stream record".to_owned(),
        ),
        RepairOutcome::Locked => failures.push(
            &mut stderr,
            4,
            "stream-tail repair",
            "could not lock stream record".to_owned(),
        ),
        RepairOutcome::WriteFailed => failures.push(
            &mut stderr,
            4,
            "stream-tail repair",
            "atomic publication failed".to_owned(),
        ),
    }
    if !matches!(plan.index, SegmentIndexStatus::Absent) {
        match operations.prune(journal, &plan.source.index_rel) {
            Ok(Some(counts)) if counts.chunks > 0 || counts.files > 0 || verbose => stdout
                .push_str(&format!(
                    "  deleted index rows: chunks={}, files={}\n",
                    counts.chunks, counts.files
                )),
            Ok(_) => {}
            Err(error) => failures.push(
                &mut stderr,
                5,
                "index prune",
                format!("{error}; run: journal indexer --rescan"),
            ),
        }
        let mut indexed = 0_u64;
        for file in files_recursive(&plan.destination.path) {
            match operations.rescan(journal, &file) {
                Ok(RescanFileStatus::Indexed { .. }) => indexed += 1,
                Ok(RescanFileStatus::Declined) => {}
                Err(error) => failures.push(
                    &mut stderr,
                    5,
                    "re-index",
                    format!("{error}; run: journal indexer --rescan"),
                ),
            }
        }
        stdout.push_str(&format!(
            "  re-indexed: {indexed} files at {}\n",
            plan.destination.index_rel
        ));
    } else if verbose {
        stdout.push_str("  index not available, skipping reindex\n");
    }
    let mutated_days = outcome.mutated_days;
    for day in &mutated_days {
        if let Err(error) = operations.touch_health(journal, day) {
            failures.push(&mut stderr, 6, "health-marker touch", error);
        }
    }
    stdout.push_str(&format!(
        "  touched health markers: {}\n",
        mutated_days.iter().cloned().collect::<Vec<_>>().join(", ")
    ));
    if verbose {
        stdout.push_str("    think will re-run daily talents on the touched days\n");
    }
    let results = checks(journal, &plan.destination);
    stdout.push('\n');
    stdout.push_str(&render_checks(&results));
    let passed = results.iter().filter(|check| check.passed).count();
    stdout.push_str(&format!("\n{passed}/{} checks passed\n", results.len()));
    MoveExecution {
        stdout,
        stderr,
        exit_code: if failures.any { 3 } else { 0 },
    }
}

#[derive(Default)]
struct PostMoveFailures {
    any: bool,
}

impl PostMoveFailures {
    fn push(&mut self, stderr: &mut String, step: u8, name: &str, detail: String) {
        self.any = true;
        stderr.push_str(&format!(
            "step {step} {name} inconsistent after move: {detail}\n"
        ));
    }
}

fn files_recursive(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_files(root, &mut files);
    files.sort();
    files
}

fn collect_files(root: &Path, files: &mut Vec<PathBuf>) {
    for entry in list_dir_entries(root).unwrap_or_default() {
        match entry.kind {
            DirEntryKind::File => files.push(entry.path),
            DirEntryKind::Directory => collect_files(&entry.path, files),
            DirEntryKind::Other => {}
        }
    }
}
