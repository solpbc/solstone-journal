// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The retention executor's entry point — **the seam that makes the crate reachable.**
//!
//! # Why this exists
//!
//! `solstone-core-retention` holds every removal of the owner's media, is fully
//! tested, and until this binary was **referenced by nothing but workspace
//! configuration.** Native `sol` commands are HTTP clients that call the Python
//! service, so they replace a CLI layer rather than logic; the logic that deletes
//! owner media lives in Python routes; and Python's only path to Rust in this repo is
//! executing a Rust binary. So a binary is the seam.
//!
//! ⚠ The pattern is worth naming: **a converted crate is not converted until something
//! reaches it.** A sibling crate had the identical gap —
//! `solstone-core-observer-delete`, "owner-authorized deletion of location-data source
//! files", called by nothing. It was deleted rather than wired, because the
//! segment-scoped ruling of 2026-08-05 forbids the partial delete it implemented. ⛔
//! Nothing in the gate set detects a crate that compiles, passes its tests, and is
//! reachable from nowhere.
//!
//! # The contract this offers its caller
//!
//! - **Always prints one JSON object on stdout**, success or failure, so a caller
//!   never has to parse prose. The receipt is the crate's own `Outcome`, serialized.
//! - **Exit code distinguishes the three outcomes a caller must handle
//!   differently**: everything removed, something refused, or the run halted. ⛔ Not
//!   a boolean: "some of what you asked for did not happen" is the case the reference
//!   implementation collapsed into success, and it is the one an owner must be told
//!   about.
//! - **Removes first, then tells the index.** The ordering is the crate's, and it is
//!   preserved here because this binary is the only place both halves are available:
//!   the executor cannot depend on a database without defeating the `IndexNotify`
//!   boundary, so the composition lives at the seam rather than inside either side.
//!
//! # ⛔ It cannot ask what time it is
//!
//! `--at` and, for the sweep, `--today`/`--now` are required arguments. The crate
//! forbids itself the clock so a verdict is reproducible from a receipt; a binary
//! that silently supplied `now` would hand that property back.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::{DateTime, NaiveDate, Utc};
use solstone_core_indexer_store::db::prune_by_paths;
use solstone_core_retention::content::{ClosedHandlerSet, JournalMedia};
use solstone_core_retention::door::{
    compact_log, notify_index, recover, release_raw, remove_logs, remove_segments,
};
use solstone_core_retention::eligibility::{RawRelease, resolve};
use solstone_core_retention::logs::{
    CLASSES, COMPACTABLE, Compaction, EntryKind, Kept, LogPlan, LogPolicy, day_key,
    plan as plan_logs, plan_compactions,
};
use solstone_core_retention::notify::{IndexNotify, NotifyError, PruneCounts};
use solstone_core_retention::policy::Policy;
use solstone_core_retention::receipt::{Outcome, RemovedPath, Target};
use solstone_core_retention::scan::scan_segment;
use solstone_core_retention::sweep::{Plan, execute as execute_sweep, plan as plan_sweep};
use solstone_core_retention::tombstone::RemovalReason;

/// Everything removed, and the index told.
const EXIT_OK: u8 = 0;
/// Usage error: nothing was attempted.
const EXIT_USAGE: u8 = 2;
/// The run completed and something was refused. ⛔ A distinct code, because a caller
/// that treats this as success reports a deletion that did not happen.
const EXIT_REFUSED: u8 = 3;
/// The run halted part way. Some targets were never reached.
const EXIT_HALTED: u8 = 4;

#[derive(Clone, serde::Serialize)]
struct PruneError {
    class: String,
    path: String,
    day: Option<String>,
    reason: String,
    hint: Option<String>,
}

#[derive(Default, serde::Serialize)]
struct ClassCounts {
    planned_files: u64,
    planned_dirs: u64,
    planned_bytes: u64,
    removed_files: u64,
    removed_dirs: u64,
    removed_bytes: u64,
    skipped: u64,
    errors: Vec<PruneError>,
}

#[derive(Default, serde::Serialize)]
struct DayCounts {
    planned_files: u64,
    planned_dirs: u64,
    planned_bytes: u64,
    removed_files: u64,
    removed_dirs: u64,
    removed_bytes: u64,
}

#[derive(serde::Serialize)]
struct CompactionCounts {
    exists: bool,
    planned: bool,
    rewritten: bool,
    lines_total: usize,
    lines_dropped: usize,
    lines_kept: usize,
    undateable_kept: usize,
    bytes_before: u64,
    bytes_after: u64,
    errors: Vec<PruneError>,
}

/// The real search index, behind the boundary the executor addresses it through.
struct RealIndex<'a> {
    journal: &'a Path,
}

impl IndexNotify for RealIndex<'_> {
    fn paths_removed(&self, removed: &[RemovedPath]) -> Result<PruneCounts, NotifyError> {
        let rels: Vec<&str> = removed.iter().map(RemovedPath::as_str).collect();
        match prune_by_paths(self.journal, &rels) {
            Ok(Some(counts)) => Ok(PruneCounts {
                chunks: counts.chunks,
                files: counts.files,
            }),
            Ok(None) => Ok(PruneCounts::default()),
            Err(error) => Err(NotifyError {
                reason: format!("the search index could not be updated: {error}"),
            }),
        }
    }
}

/// A parsed command line: flags with values, repeatable.
struct Args {
    values: Vec<(String, String)>,
}

impl Args {
    fn parse(raw: &[String]) -> Result<Self, String> {
        let mut values = Vec::new();
        let mut index = 0usize;
        while index < raw.len() {
            let flag = raw
                .get(index)
                .ok_or_else(|| "argument list ended unexpectedly".to_owned())?;
            if !flag.starts_with("--") {
                return Err(format!("expected a --flag, found `{flag}`"));
            }
            let Some(value) = raw.get(index.saturating_add(1)) else {
                return Err(format!("`{flag}` needs a value"));
            };
            values.push((flag.clone(), value.clone()));
            index = index.saturating_add(2);
        }
        Ok(Self { values })
    }

    fn one(&self, flag: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(name, _)| name == flag)
            .map(|(_, value)| value.as_str())
    }

    fn all(&self, flag: &str) -> Vec<&str> {
        self.values
            .iter()
            .filter(|(name, _)| name == flag)
            .map(|(_, value)| value.as_str())
            .collect()
    }

    fn required(&self, flag: &str) -> Result<&str, String> {
        self.one(flag).ok_or_else(|| format!("{flag} is required"))
    }

    fn has(&self, flag: &str) -> bool {
        self.values.iter().any(|(name, value)| {
            name == flag && !matches!(value, value if value == "false" || value == "0")
        })
    }
}

/// Print one JSON object and return an exit code.
fn emit(body: serde_json::Value, code: u8) -> ExitCode {
    println!("{body}");
    ExitCode::from(code)
}

fn fail(message: &str) -> ExitCode {
    emit(
        serde_json::json!({ "ok": false, "error": message }),
        EXIT_USAGE,
    )
}

/// The exit code an outcome implies.
fn code_for(outcome: &Outcome) -> u8 {
    if outcome.halted.is_some() {
        return EXIT_HALTED;
    }
    if outcome
        .targets
        .iter()
        .any(|target| !target.not_removed.is_empty())
    {
        return EXIT_REFUSED;
    }
    EXIT_OK
}

/// `DAY/STREAM/DIR`, with the default stream spelled out.
///
/// ⛔ Spelled out rather than omitted: an empty component would make two different
/// segments parse the same, and the caller naming a segment is the one place the
/// stream is unambiguous.
fn parse_segment(spec: &str) -> Result<Target, String> {
    let parts: Vec<&str> = spec.split('/').collect();
    let [day, stream, dir] = parts.as_slice() else {
        return Err(format!(
            "`{spec}` is not DAY/STREAM/DIR (name the default stream `_default`)"
        ));
    };
    if day.is_empty() || stream.is_empty() || dir.is_empty() {
        return Err(format!("`{spec}` has an empty component"));
    }
    if [day, stream, dir]
        .iter()
        .any(|part| **part == "." || **part == ".." || part.contains('\\'))
    {
        return Err(format!("`{spec}` names a traversal component"));
    }
    Ok(Target {
        day: (*day).to_owned(),
        stream: (*stream).to_owned(),
        dir: (*dir).to_owned(),
    })
}

/// Remove, then tell the index, then report both.
fn finish(journal: &Path, outcome: Outcome, extra: serde_json::Value) -> ExitCode {
    let code = code_for(&outcome);
    let index = RealIndex { journal };
    // ⚠ A failed notification does not undo a removal and must not be reported as
    // one. It is a separate field, and it does not change the exit code: the files
    // are gone either way, and a stale index row is self-announcing.
    let notified = match notify_index(&index, &outcome) {
        Ok(counts) => serde_json::json!({
            "ok": true, "chunks": counts.chunks, "files": counts.files
        }),
        Err(error) => serde_json::json!({ "ok": false, "error": error.reason }),
    };
    let receipt = serde_json::to_value(&outcome)
        .unwrap_or_else(|error| serde_json::json!({ "unserializable": error.to_string() }));
    emit(
        serde_json::json!({
            "ok": code == EXIT_OK,
            "outcome": receipt,
            "index": notified,
            "detail": extra,
        }),
        code,
    )
}

fn reason_for(name: Option<&str>) -> Result<RemovalReason, String> {
    match name.unwrap_or("owner") {
        "owner" => Ok(RemovalReason::OwnerSegmentDelete),
        "policy" => Ok(RemovalReason::RetentionPolicy),
        other => Err(format!(
            "--reason must be `owner` or `policy`, not `{other}`"
        )),
    }
}

fn run_remove_segments(args: &Args) -> ExitCode {
    let journal = match args.required("--journal") {
        Ok(value) => PathBuf::from(value),
        Err(error) => return fail(&error),
    };
    let at = match args.required("--at") {
        Ok(value) => value,
        Err(error) => return fail(&error),
    };
    let did = match args.required("--did") {
        Ok(value) => value,
        Err(error) => return fail(&error),
    };
    let reason = match reason_for(args.one("--reason")) {
        Ok(reason) => reason,
        Err(error) => return fail(&error),
    };
    let specs = args.all("--segment");
    if specs.is_empty() {
        return fail("at least one --segment DAY/STREAM/DIR is required");
    }
    let mut targets = Vec::new();
    for spec in specs {
        match parse_segment(spec) {
            Ok(target) => targets.push(target),
            Err(error) => return fail(&error),
        }
    }
    let outcome = remove_segments(&journal, &targets, at, reason, did);
    finish(
        &journal,
        outcome,
        serde_json::json!({ "verb": "remove-segments" }),
    )
}

fn run_recover(args: &Args) -> ExitCode {
    let journal = match args.required("--journal") {
        Ok(value) => PathBuf::from(value),
        Err(error) => return fail(&error),
    };
    let at = match args.required("--at") {
        Ok(value) => value,
        Err(error) => return fail(&error),
    };
    let did = match args.required("--did") {
        Ok(value) => value,
        Err(error) => return fail(&error),
    };
    let reason = match reason_for(args.one("--reason")) {
        Ok(reason) => reason,
        Err(error) => return fail(&error),
    };
    let outcome = recover(&journal, at, reason, did);
    finish(&journal, outcome, serde_json::json!({ "verb": "recover" }))
}

/// Release proven raw originals from named segments, keeping every derived output.
///
/// ⛔ Takes segments, not files. The unit of the decision is the segment even though the
/// unit of the removal is the file: one unprovable file holds the whole segment, because
/// a partially-released segment is a shape no reader expects and derived frames with no
/// record of their own depend on riding the segment's verdict.
///
/// ⚠ Proof is re-derived from disk here. A caller cannot assert that a file is
/// releasable -- it can only name where to look.
fn run_release_raw(args: &Args) -> ExitCode {
    let journal = match args.required("--journal") {
        Ok(value) => PathBuf::from(value),
        Err(error) => return fail(&error),
    };
    let specs = args.all("--segment");
    if specs.is_empty() {
        return fail("at least one --segment DAY/STREAM/DIR is required");
    }
    let mut targets = Vec::new();
    for spec in specs {
        match parse_segment(spec) {
            Ok(target) => targets.push(target),
            Err(error) => return fail(&error),
        }
    }

    let mut proven = Vec::new();
    let mut held = Vec::new();
    for target in &targets {
        let segment = journal.join(crate_layout_segment_rel(target)).to_path_buf();
        let found = scan_segment(&segment, &ClosedHandlerSet, &JournalMedia);
        if found.is_empty() {
            continue;
        }
        match resolve(
            &ClosedHandlerSet,
            &JournalMedia,
            &target.day,
            &target.stream,
            &target.dir,
            &found,
        ) {
            RawRelease::Releasable(mut ready) => proven.append(&mut ready),
            RawRelease::Held(blockers) => held.push(serde_json::json!({
                "day": target.day,
                "stream": target.stream,
                "dir": target.dir,
                "blockers": blockers
                    .iter()
                    .map(|blocker| blocker.name().to_owned())
                    .collect::<Vec<String>>(),
            })),
        }
    }

    let (outcome, tally) = release_raw(&journal, &proven);
    finish(
        &journal,
        outcome,
        serde_json::json!({
            "verb": "release-raw",
            "held": held,
            "evidence": { "on_record": tally.on_record, "on_legacy_rows": tally.on_legacy_rows },
        }),
    )
}

/// A segment's journal-relative path, through the crate's one path builder.
fn crate_layout_segment_rel(target: &Target) -> String {
    solstone_core_retention::layout::segment_rel(&target.day, &target.stream, &target.dir)
}

/// A plan's shape as a caller needs to read it.
fn plan_json(plan: &Plan) -> serde_json::Value {
    serde_json::json!({
        "examined": plan.examined(),
        "candidates": plan.candidates.len(),
        "files": plan.files(),
        "bytes": plan.bytes(),
        "skipped": plan.skipped.len(),
        "unreadable_days": plan.unreadable_days,
        "segments": plan
            .candidates
            .iter()
            .map(|candidate| serde_json::json!({
                "day": candidate.target.day,
                "stream": candidate.target.stream,
                "dir": candidate.target.dir,
                "bytes": candidate.bytes(),
                "files": candidate
                    .proven
                    .iter()
                    .map(|proven| proven.rel())
                    .collect::<Vec<String>>(),
            }))
            .collect::<Vec<serde_json::Value>>(),
    })
}

fn count_target(class: &mut ClassCounts, day: &mut DayCounts, kind: EntryKind, bytes: u64) {
    match kind {
        EntryKind::File => {
            class.planned_files = class.planned_files.saturating_add(1);
            day.planned_files = day.planned_files.saturating_add(1);
        }
        EntryKind::Directory => {
            class.planned_dirs = class.planned_dirs.saturating_add(1);
            day.planned_dirs = day.planned_dirs.saturating_add(1);
        }
    }
    class.planned_bytes = class.planned_bytes.saturating_add(bytes);
    day.planned_bytes = day.planned_bytes.saturating_add(bytes);
}

fn count_removed(class: &mut ClassCounts, day: &mut DayCounts, kind: EntryKind, bytes: u64) {
    match kind {
        EntryKind::File => {
            class.removed_files = class.removed_files.saturating_add(1);
            day.removed_files = day.removed_files.saturating_add(1);
        }
        EntryKind::Directory => {
            class.removed_dirs = class.removed_dirs.saturating_add(1);
            day.removed_dirs = day.removed_dirs.saturating_add(1);
        }
    }
    class.removed_bytes = class.removed_bytes.saturating_add(bytes);
    day.removed_bytes = day.removed_bytes.saturating_add(bytes);
}

fn add_prune_error(
    classes: &mut BTreeMap<String, ClassCounts>,
    errors: &mut Vec<PruneError>,
    class: &str,
    path: &str,
    day: Option<String>,
    reason: String,
) {
    let error = PruneError {
        class: class.to_owned(),
        path: path.to_owned(),
        day,
        reason,
        hint: None,
    };
    if let Some(counts) = classes.get_mut(class) {
        counts.skipped = counts.skipped.saturating_add(1);
        counts.errors.push(error.clone());
    }
    errors.push(error);
}

/// The log plan and its line-compaction companions in the executor's stable receipt.
///
/// The bridge must not recreate the class table or date parser, so both the planned and
/// completed totals stay beside the plan that made the decision.
fn log_plan_json(
    journal: &Path,
    plan: &LogPlan,
    compactions: &[Compaction],
    outcome: Option<&Outcome>,
    executed: bool,
) -> serde_json::Value {
    let mut classes: BTreeMap<String, ClassCounts> = CLASSES
        .iter()
        .map(|class| (class.name.to_owned(), ClassCounts::default()))
        .collect();
    let mut days: BTreeMap<String, DayCounts> = BTreeMap::new();
    let mut errors = Vec::new();
    let failures: BTreeMap<String, String> = outcome
        .into_iter()
        .flat_map(|done| done.targets.iter())
        .flat_map(|target| target.not_removed.iter())
        .map(|failure| (failure.entry.clone(), failure.reason.clone()))
        .collect();

    for target in &plan.prunable {
        let day_name = day_key(target.day());
        let Some(class) = classes.get_mut(target.class()) else {
            continue;
        };
        let day = days.entry(day_name).or_default();
        count_target(class, day, target.kind(), target.bytes());
        if executed {
            if let Some(reason) = failures.get(target.rel()) {
                add_prune_error(
                    &mut classes,
                    &mut errors,
                    target.class(),
                    target.rel(),
                    Some(day_key(target.day())),
                    reason.clone(),
                );
            } else if let (Some(class), Some(day)) = (
                classes.get_mut(target.class()),
                days.get_mut(&day_key(target.day())),
            ) {
                count_removed(class, day, target.kind(), target.bytes());
            }
        }
    }

    for retained in &plan.retained {
        let (day, reason) = match &retained.reason {
            Kept::Undateable => (
                None,
                Some("the entry's retention date could not be determined".to_owned()),
            ),
            Kept::ContentMalformed { day, detail } => (
                Some(day_key(*day)),
                Some(format!("malformed talent day-index row: {detail}")),
            ),
            Kept::TooYoung(_) | Kept::Exempt | Kept::NotAMatch | Kept::ContentNotFullyOld => {
                (None, None)
            }
        };
        if let Some(reason) = reason {
            add_prune_error(
                &mut classes,
                &mut errors,
                retained.class,
                &retained.rel,
                day,
                reason,
            );
        }
    }

    let compaction_rows = COMPACTABLE
        .iter()
        .map(|log| {
            let planned = compactions
                .iter()
                .find(|planned| planned.name() == log.name);
            let path = journal.join(log.rel);
            let failure = failures.get(log.rel).map(|reason| PruneError {
                class: log.name.to_owned(),
                path: log.rel.to_owned(),
                day: None,
                reason: reason.clone(),
                hint: None,
            });
            if let Some(error) = &failure {
                errors.push(error.clone());
            }
            let counts = CompactionCounts {
                exists: path.exists(),
                planned: planned.is_some(),
                rewritten: executed && planned.is_some() && failure.is_none(),
                lines_total: planned.map_or(0, |item| item.lines_total),
                lines_dropped: planned.map_or(0, |item| item.lines_dropped),
                lines_kept: planned.map_or(0, Compaction::lines_kept),
                undateable_kept: planned.map_or(0, |item| item.undateable_kept),
                bytes_before: planned.map_or(0, |item| item.bytes_before),
                bytes_after: planned.map_or(0, |item| item.bytes_after),
                errors: failure.into_iter().collect(),
            };
            (log.name, counts)
        })
        .collect::<BTreeMap<&str, CompactionCounts>>();

    serde_json::json!({
        "examined": plan.examined(),
        "prunable": plan.prunable.len(),
        "bytes": plan.bytes(),
        "retained": plan.retained.len(),
        "absent_classes": plan.absent_classes,
        "by_class": classes,
        "by_day": days,
        "errors": errors,
        "compactions": compaction_rows,
    })
}

fn run_sweep(args: &Args) -> ExitCode {
    let journal = match args.required("--journal") {
        Ok(value) => PathBuf::from(value),
        Err(error) => return fail(&error),
    };
    let today = match args.required("--today").and_then(|value| {
        NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .map_err(|_| format!("--today must be YYYY-MM-DD, not `{value}`"))
    }) {
        Ok(value) => value,
        Err(error) => return fail(&error),
    };
    let now = match args.required("--now").and_then(|value| {
        DateTime::parse_from_rfc3339(value)
            .map(|when| when.with_timezone(&Utc))
            .map_err(|_| format!("--now must be an RFC 3339 instant, not `{value}`"))
    }) {
        Ok(value) => value,
        Err(error) => return fail(&error),
    };
    let policy = match args.one("--policy") {
        None => Policy::default(),
        Some(text) => match serde_json::from_str::<Policy>(text) {
            Ok(policy) => policy,
            Err(error) => return fail(&format!("--policy is not a retention policy: {error}")),
        },
    };

    let plan = plan_sweep(
        &journal,
        &policy,
        &ClosedHandlerSet,
        &JournalMedia,
        today,
        now,
    );
    // ⛔ Planning is the default. A destructive pass must be asked for.
    if !args.has("--execute") {
        return emit(
            serde_json::json!({ "ok": true, "executed": false, "plan": plan_json(&plan) }),
            EXIT_OK,
        );
    }
    let (outcome, tally) = execute_sweep(&journal, &plan);
    finish(
        &journal,
        outcome,
        serde_json::json!({
            "verb": "sweep",
            "executed": true,
            "plan": plan_json(&plan),
            "evidence": { "on_record": tally.on_record, "on_legacy_rows": tally.on_legacy_rows },
        }),
    )
}

fn run_prune_logs(args: &Args) -> ExitCode {
    let journal = match args.required("--journal") {
        Ok(value) => PathBuf::from(value),
        Err(error) => return fail(&error),
    };
    let today = match args.required("--today").and_then(|value| {
        NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .map_err(|_| format!("--today must be YYYY-MM-DD, not `{value}`"))
    }) {
        Ok(value) => value,
        Err(error) => return fail(&error),
    };
    let days = match args.required("--days").and_then(|value| {
        value
            .parse::<u32>()
            .map_err(|_| format!("--days must be a whole number, not `{value}`"))
    }) {
        Ok(value) => value,
        Err(error) => return fail(&error),
    };
    let policy = LogPolicy {
        days,
        enabled: true,
    };
    let plan = plan_logs(&journal, &policy, today);
    let compactions = plan_compactions(&journal, &policy, today);
    let described = log_plan_json(&journal, &plan, &compactions, None, false);
    if !args.has("--execute") {
        return emit(
            serde_json::json!({ "ok": true, "executed": false, "plan": described }),
            EXIT_OK,
        );
    }
    let mut outcome = remove_logs(&journal, &plan.prunable);
    for compaction in &compactions {
        let compacted = compact_log(&journal, compaction);
        outcome.targets.extend(compacted.targets);
        if outcome.halted.is_none() {
            outcome.halted = compacted.halted;
        }
    }
    let described = log_plan_json(&journal, &plan, &compactions, Some(&outcome), true);
    finish(
        &journal,
        outcome,
        serde_json::json!({ "verb": "prune-logs", "executed": true, "plan": described }),
    )
}

const USAGE: &str = "\
solstone-retention — the retention executor

  remove-segments --journal P --at ISO --did ID --segment DAY/STREAM/DIR [--segment ...] \
[--reason owner|policy]
  release-raw     --journal P --segment DAY/STREAM/DIR [--segment ...]
  recover         --journal P --at ISO --did ID [--reason owner|policy]
  sweep           --journal P --today YYYY-MM-DD --now ISO [--policy JSON] [--execute true]
  prune-logs      --journal P --today YYYY-MM-DD --days N [--execute true]

Always prints one JSON object. Exit 0 all removed, 2 usage, 3 something refused, \
4 run halted.
Name the default stream `_default`; it contributes no path component.
";

fn main() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let Some(verb) = raw.first() else {
        eprint!("{USAGE}");
        return ExitCode::from(EXIT_USAGE);
    };
    if verb == "--help" || verb == "-h" || verb == "help" {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let args = match Args::parse(raw.get(1..).unwrap_or_default()) {
        Ok(args) => args,
        Err(error) => return fail(&error),
    };
    match verb.as_str() {
        "remove-segments" => run_remove_segments(&args),
        "release-raw" => run_release_raw(&args),
        "recover" => run_recover(&args),
        "sweep" => run_sweep(&args),
        "prune-logs" => run_prune_logs(&args),
        other => fail(&format!("unknown verb `{other}`")),
    }
}
