// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native port of `solstone/apps/observer/prune.py`: duplicate observer
//! segment pruning. Every irreversible step (the directory removal) is
//! guarded by refusal gates that fail closed rather than guess, and by the
//! append-before-delete ordering in `apply::execute_plan`.

mod apply;
mod attribution;
mod chain;
mod history;
mod identity;
mod marker;
mod plan;
mod types;

use std::collections::BTreeSet;
use std::path::Path;

use chrono::{Duration, NaiveDate};
use solstone_core_journal_io::{SegmentIdentityError, check_record_identities};
use solstone_core_segment::{list_days, list_segments};

pub use attribution::observer_prefix_for_stream;
pub use history::{
    HistoryPruneFailure, HistoryPruneReport, has_history_for_stream, remove_history_rows_for_stream,
};
pub use types::{PruneCandidate, PruneGroup, PruneResult, Refusal, SegmentAnalysis};

/// The mutually-exclusive `--day` / `--day-range` / `--all` selector.
#[derive(Debug, Clone)]
pub enum DaySelector {
    Day(String),
    DayRange(String, String),
    All,
}

/// Resolve the day selector to a sorted list of `YYYYMMDD` days.
pub fn resolve_prune_days(journal: &Path, selector: &DaySelector) -> Result<Vec<String>, String> {
    match selector {
        DaySelector::Day(day) => {
            validate_day(day)?;
            Ok(vec![day.clone()])
        }
        DaySelector::DayRange(start_text, end_text) => {
            let start = validate_day(start_text)?;
            let end = validate_day(end_text)?;
            if end < start {
                return Err("--day-range end must be on or after start".to_owned());
            }
            let mut days = Vec::new();
            let mut current = start;
            while current <= end {
                days.push(current.format("%Y%m%d").to_string());
                current += Duration::days(1);
            }
            Ok(days)
        }
        DaySelector::All => {
            let mut days: Vec<String> = list_days(journal)
                .map_err(|error| error.to_string())?
                .into_iter()
                .map(|(day, _)| day)
                .collect();
            days.sort();
            Ok(days)
        }
    }
}

fn validate_day(day: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(day, "%Y%m%d").map_err(|_| format!("invalid day: {day}"))
}

fn selected_listed_segments(
    journal: &Path,
    days: &[String],
    stream: Option<&str>,
) -> Vec<solstone_core_journal_io::Segment> {
    let mut listed = Vec::new();
    for day in days {
        let Ok(segments) = list_segments(journal, day) else {
            continue;
        };
        for segment in segments {
            if let Some(filter) = stream
                && !segment.stream().matches(filter)
            {
                continue;
            }
            listed.push(segment);
        }
    }
    listed
}

fn identity_preflight(
    journal: &Path,
    days: &[String],
    stream: Option<&str>,
) -> Result<(), Refusal> {
    check_record_identities(&selected_listed_segments(journal, days, stream)).map_err(
        |error: SegmentIdentityError| {
            Refusal::new(
                "prune",
                "segment-identity",
                None::<String>,
                error.to_string(),
            )
        },
    )?;
    Ok(())
}

fn selected_streams(
    journal: &Path,
    days: &[String],
    stream: Option<&str>,
) -> Result<BTreeSet<String>, Refusal> {
    let mut streams = BTreeSet::new();
    for day in days {
        let Ok(segments) = solstone_core_segment::list_segments(journal, day) else {
            continue;
        };
        for segment in segments {
            if let Some(filter) = stream
                && !segment.stream().matches(filter)
            {
                continue;
            }
            let identity = segment.record_identity().map_err(|error| {
                Refusal::new(
                    "prune",
                    "segment-identity",
                    None::<String>,
                    error.to_string(),
                )
            })?;
            streams.insert(identity.stream.to_owned());
        }
    }
    if let Some(stream) = stream {
        streams.insert(stream.to_owned());
    }
    Ok(streams)
}

/// Repair dangling `prev_segment` pointers already justified by pruned
/// history rows left over from an interrupted run, before planning anything
/// new. This is what makes a crashed-mid-group prune converge on rerun.
fn repair_crash_leftovers(
    journal: &Path,
    days: &[String],
    stream: Option<&str>,
) -> (Vec<Refusal>, u64, BTreeSet<String>) {
    let streams = match selected_streams(journal, days, stream) {
        Ok(streams) => streams,
        Err(refusal) => return (vec![refusal], 0, BTreeSet::new()),
    };
    let mut refusals = Vec::new();
    let mut repaired = 0u64;
    let mut mutated_days = BTreeSet::new();
    for stream_name in streams {
        let (stream_refusals, count, stream_days) =
            chain::repair_stream_chain(journal, &stream_name, &Default::default(), false);
        refusals.extend(stream_refusals);
        repaired += count;
        mutated_days.extend(stream_days);
    }
    (refusals, repaired, mutated_days)
}

/// Plan or execute observer duplicate pruning. Dry-run is the default and
/// performs zero writes. `--execute` re-derives everything from disk before
/// deleting anything -- the dry-run plan is never carried forward.
pub fn run_prune(
    journal: &Path,
    days: &[String],
    stream: Option<&str>,
    execute: bool,
    now_ms: i64,
) -> PruneResult {
    if execute {
        if let Err(refusal) = identity_preflight(journal, days, stream) {
            let mut result = PruneResult {
                execute: true,
                ..PruneResult::default()
            };
            result.refusals.push(refusal);
            return result;
        }
        let (recovery_refusals, repaired, repaired_days) =
            repair_crash_leftovers(journal, days, stream);
        let mut result = plan::plan(journal, days, stream);
        result.execute = true;
        result.crash_repaired = repaired;
        let recovery_found_refusals = !recovery_refusals.is_empty();
        let mut refusals = recovery_refusals;
        refusals.extend(result.refusals);
        result.refusals = refusals;
        if recovery_found_refusals {
            for day in &repaired_days {
                if let Err(error) = solstone_core_segment::touch_stream_health_marker(journal, day)
                {
                    apply::report_marker_failure(&mut result, day, &error);
                }
            }
            return result;
        }
        let groups = std::mem::take(&mut result.groups);
        apply::execute_plan(journal, &mut result, groups, now_ms, repaired_days);
        result
    } else {
        plan::plan(journal, days, stream)
    }
}

/// Format prune output for the operator CLI.
pub fn format_result(result: &PruneResult) -> String {
    let candidates: Vec<&PruneCandidate> = result
        .groups
        .iter()
        .flat_map(|group| &group.candidates)
        .collect();
    let deleted_keys: BTreeSet<(String, String, String)> = result
        .deleted
        .iter()
        .map(|candidate| {
            (
                candidate.analysis.day.clone(),
                candidate.analysis.stream.clone(),
                candidate.analysis.segment.clone(),
            )
        })
        .collect();
    let mut lines = vec![
        if result.execute {
            "device prune execute".to_owned()
        } else {
            "device prune dry-run".to_owned()
        },
        format!("groups: {}", result.groups.len()),
        format!("candidates: {}", candidates.len()),
        format!("deleted: {}", result.deleted.len()),
        format!("chain-repaired: {}", result.chain_repaired),
        format!("last-physical-copy: {}", result.last_physical_copy_count()),
        format!("refusals: {}", result.refusals.len()),
    ];
    if result.crash_repaired > 0 {
        lines.push(format!("crash-repaired: {}", result.crash_repaired));
    }
    for group in &result.groups {
        lines.push(format!(
            "group {}/{}/{}_*: canonical={} candidates={}",
            group.day,
            group.stream,
            group.start,
            group.canonical.segment,
            group.candidates.len()
        ));
        for candidate in &group.candidates {
            if !candidate.last_physical_copy {
                continue;
            }
            let key = (
                candidate.analysis.day.clone(),
                candidate.analysis.stream.clone(),
                candidate.analysis.segment.clone(),
            );
            let prefix = if deleted_keys.contains(&key) {
                "deleted"
            } else {
                "would-delete"
            };
            lines.push(format!(
                "  {prefix}: {} duplicate_of={} [last-physical-copy]",
                candidate.analysis.segment, group.canonical.segment
            ));
        }
    }
    if !result.index_errors.is_empty() {
        lines.push("index errors:".to_owned());
        for error in &result.index_errors {
            lines.push(format!("  {error}"));
        }
    }
    if !result.refusals.is_empty() {
        lines.push("refusals:".to_owned());
        for refusal in &result.refusals {
            let file_text = refusal.file.clone().unwrap_or_else(|| "(none)".to_owned());
            lines.push(format!(
                "  refused={} gate={} file={file_text} resolution={}",
                refusal.subject, refusal.gate, refusal.resolution
            ));
        }
    }
    lines.join("\n") + "\n"
}
