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
use solstone_core_indexer_store::RetentionIndex;
use solstone_core_retention::content::{ClosedHandlerSet, JournalMedia};
use solstone_core_retention::door::{
    compact_log, notify_index, recover, release_raw, remove_logs, remove_segments,
};
use solstone_core_retention::eligibility::{RawRelease, resolve};
use solstone_core_retention::logs::{
    CLASSES, COMPACTABLE, Compaction, EntryKind, Kept, LogPlan, LogPolicy, day_key,
    plan as plan_logs, plan_compactions,
};
use solstone_core_retention::marks::{
    Failure, MarkId, Proposal, RemovalClass, decline, load, preflight, reconcile,
    reconcile_recovered, record_failure, resolve_offload, upsert_offload,
};
use solstone_core_retention::policy::Policy;
use solstone_core_retention::receipt::{Outcome, Target};
use solstone_core_retention::remove_marked::remove_marked;
use solstone_core_retention::scan::scan_segment;
use solstone_core_retention::sweep::{Plan, Skip, execute as execute_sweep, plan as plan_sweep};
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
    if outcome.has_failures() {
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
    let verb = extra
        .get("verb")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let index = RetentionIndex::new(journal);
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
            "verb": verb,
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
    let at = match parse_rfc3339_flag(args, "--at") {
        Ok(value) => value,
        Err(error) => return fail(&error),
    };
    let at_stamp = at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let cid = match args.required("--did") {
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
    let outcome = remove_segments(&journal, &targets, &at_stamp, reason, cid);
    // An integration test cannot cleanly fail only the register write after a real
    // removal. Preserve the completed outcome even if that secondary write fails.
    let mut register_errors = Vec::new();
    for target_outcome in &outcome.targets {
        let Some(failure) = target_outcome.not_removed.iter().find_map(|item| {
            item.staged.as_ref().map(|staged| Failure {
                at: String::new(),
                reason: item.reason.clone(),
                staged: Some(staged.clone()),
            })
        }) else {
            continue;
        };
        if let Err(error) = record_failure(
            &journal,
            RemovalClass::OwnerSegmentRemoval,
            &target_outcome.target,
            &[],
            failure,
            at,
        ) {
            register_errors.push(error.to_string());
        }
    }
    finish(
        &journal,
        outcome,
        serde_json::json!({
            "verb": "remove-segments",
            "register_error": (!register_errors.is_empty()).then(|| register_errors.join("; ")),
        }),
    )
}

fn run_recover(args: &Args) -> ExitCode {
    let journal = match args.required("--journal") {
        Ok(value) => PathBuf::from(value),
        Err(error) => return fail(&error),
    };
    let at = match parse_rfc3339_flag(args, "--at") {
        Ok(value) => value,
        Err(error) => return fail(&error),
    };
    let at_stamp = at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let cid = match args.required("--did") {
        Ok(value) => value,
        Err(error) => return fail(&error),
    };
    let reason = match reason_for(args.one("--reason")) {
        Ok(reason) => reason,
        Err(error) => return fail(&error),
    };
    let outcome = recover(&journal, &at_stamp, reason, cid);
    if let Err(error) = reconcile_recovered(&journal) {
        return emit(
            serde_json::json!({ "ok": false, "verb": "recover", "error": error.to_string() }),
            EXIT_REFUSED,
        );
    }
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
    let at = match parse_rfc3339_flag(args, "--at") {
        Ok(value) => value,
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
    // Raw-file release has no `.removing_` rename, so door::release_raw cannot
    // currently produce a staged row. Keep this defensive registration path for a
    // future door change, and preserve the completed outcome if its register write fails.
    let mut register_errors = Vec::new();
    for target_outcome in &outcome.targets {
        let Some(failure) = target_outcome.not_removed.iter().find_map(|item| {
            item.staged.as_ref().map(|staged| Failure {
                at: String::new(),
                reason: item.reason.clone(),
                staged: Some(staged.clone()),
            })
        }) else {
            continue;
        };
        let mut names = proven
            .iter()
            .filter(|item| {
                item.day() == target_outcome.target.day
                    && item.stream() == target_outcome.target.stream
                    && item.dir() == target_outcome.target.dir
            })
            .map(|item| item.name().to_owned())
            .collect::<Vec<_>>();
        names.sort();
        if let Err(error) = record_failure(
            &journal,
            RemovalClass::OwnerRawRelease,
            &target_outcome.target,
            &names,
            failure,
            at,
        ) {
            register_errors.push(error.to_string());
        }
    }
    finish(
        &journal,
        outcome,
        serde_json::json!({
            "verb": "release-raw",
            "held": held,
            "evidence": { "on_record": tally.on_record, "on_legacy_rows": tally.on_legacy_rows },
            "register_error": (!register_errors.is_empty()).then(|| register_errors.join("; ")),
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
        "skipped_segments": plan
            .skipped
            .iter()
            .map(|skipped| match &skipped.reason {
                Skip::NoMedia => serde_json::json!({
                    "day": skipped.target.day,
                    "stream": skipped.target.stream,
                    "dir": skipped.target.dir,
                    "reason": "no_media",
                }),
                Skip::Policy(eligibility) => serde_json::json!({
                    "day": skipped.target.day,
                    "stream": skipped.target.stream,
                    "dir": skipped.target.dir,
                    "reason": "policy",
                    "eligibility": eligibility,
                }),
                Skip::Held(blockers) => serde_json::json!({
                    "day": skipped.target.day,
                    "stream": skipped.target.stream,
                    "dir": skipped.target.dir,
                    "reason": "held",
                    "blockers": blockers,
                }),
            })
            .collect::<Vec<serde_json::Value>>(),
        "unreadable_days": plan.unreadable_days,
        "unrepresentable_segments": plan
            .unrepresentable_segments
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>(),
        "chronicle_unavailable": plan.chronicle_unavailable,
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

fn add_skip(classes: &mut BTreeMap<String, ClassCounts>, class: &str) {
    if let Some(counts) = classes.get_mut(class) {
        counts.skipped = counts.skipped.saturating_add(1);
    }
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
        if matches!(retained.reason, Kept::Exempt | Kept::ContentNotFullyOld) {
            add_skip(&mut classes, retained.class);
        }
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
    if args.has("--execute") {
        return fail("unknown flag `--execute`; use --force");
    }
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
    if !args.has("--force") {
        return emit(
            serde_json::json!({ "ok": true, "verb": "sweep", "executed": false, "plan": plan_json(&plan) }),
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

fn parse_policy(args: &Args) -> Result<Policy, String> {
    match args.one("--policy") {
        None => Ok(Policy::default()),
        Some(text) => serde_json::from_str::<Policy>(text)
            .map_err(|error| format!("--policy is not a retention policy: {error}")),
    }
}

fn parse_today(args: &Args) -> Result<NaiveDate, String> {
    args.required("--today").and_then(|value| {
        NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .map_err(|_| format!("--today must be YYYY-MM-DD, not `{value}`"))
    })
}

fn parse_rfc3339_flag(args: &Args, flag: &str) -> Result<DateTime<Utc>, String> {
    args.required(flag).and_then(|value| {
        DateTime::parse_from_rfc3339(value)
            .map(|when| when.with_timezone(&Utc))
            .map_err(|_| format!("{flag} must be an RFC 3339 instant, not `{value}`"))
    })
}

fn parse_now(args: &Args) -> Result<DateTime<Utc>, String> {
    parse_rfc3339_flag(args, "--now")
}

/// Report a command-specific argument refusal without losing the verb identifier.
fn verb_fail(verb: &str, error: &str) -> ExitCode {
    emit(
        serde_json::json!({ "ok": false, "verb": verb, "error": error }),
        EXIT_USAGE,
    )
}

fn run_mark(args: &Args) -> ExitCode {
    let journal = match args.required("--journal") {
        Ok(value) => PathBuf::from(value),
        Err(error) => return verb_fail("mark", &error),
    };
    let today = match parse_today(args) {
        Ok(value) => value,
        Err(error) => return verb_fail("mark", &error),
    };
    let now = match parse_now(args) {
        Ok(value) => value,
        Err(error) => return verb_fail("mark", &error),
    };
    let policy = match parse_policy(args) {
        Ok(value) => value,
        Err(error) => return verb_fail("mark", &error),
    };
    let plan = plan_sweep(
        &journal,
        &policy,
        &ClosedHandlerSet,
        &JournalMedia,
        today,
        now,
    );
    if plan.chronicle_unavailable {
        return emit(
            serde_json::json!({
                "ok": false,
                "verb": "mark",
                "error": "the journal chronicle directory is unavailable",
            }),
            EXIT_REFUSED,
        );
    }
    let mut proposals = plan
        .candidates
        .iter()
        .map(|candidate| {
            let mut names = candidate
                .proven
                .iter()
                .map(|item| item.name().to_owned())
                .collect::<Vec<_>>();
            names.sort();
            (
                candidate.target.clone(),
                Proposal {
                    bytes: candidate.bytes(),
                    reason: format!("policy eligibility: {:?}", candidate.eligibility),
                    names,
                },
            )
        })
        .collect::<Vec<_>>();
    if !plan.unreadable_days.is_empty() {
        let register = match load(&journal) {
            Ok(register) => register,
            Err(error) => {
                return emit(
                    serde_json::json!({ "ok": false, "verb": "mark", "error": error.to_string() }),
                    EXIT_REFUSED,
                );
            }
        };
        proposals.extend(
            register
                .marks
                .values()
                .filter(|mark| {
                    mark.class == RemovalClass::PolicyRawRelease
                        && plan.unreadable_days.contains(&mark.target.day)
                })
                .map(|mark| (mark.target.clone(), mark.proposal.clone())),
        );
    }
    match reconcile(&journal, RemovalClass::PolicyRawRelease, &proposals, now) {
        Ok(register) => emit(
            serde_json::json!({ "ok": true, "verb": "mark", "marks": register, "plan": plan_json(&plan) }),
            EXIT_OK,
        ),
        Err(error) => emit(
            serde_json::json!({ "ok": false, "verb": "mark", "error": error.to_string() }),
            EXIT_REFUSED,
        ),
    }
}

fn run_marks(args: &Args) -> ExitCode {
    let journal = match args.required("--journal") {
        Ok(value) => PathBuf::from(value),
        Err(error) => return verb_fail("marks", &error),
    };
    match load(&journal) {
        Ok(register) => emit(
            serde_json::json!({ "ok": true, "verb": "marks", "marks": register }),
            EXIT_OK,
        ),
        Err(error) => emit(
            serde_json::json!({ "ok": false, "verb": "marks", "error": error.to_string() }),
            EXIT_REFUSED,
        ),
    }
}

fn run_mark_offload(args: &Args) -> ExitCode {
    let journal = match args.required("--journal") {
        Ok(value) => PathBuf::from(value),
        Err(error) => return verb_fail("mark-offload", &error),
    };
    let day = match args.required("--day") {
        Ok(value) => value,
        Err(error) => return verb_fail("mark-offload", &error),
    };
    let dir = match args.required("--dir") {
        Ok(value) => value,
        Err(error) => return verb_fail("mark-offload", &error),
    };
    let reason = match args.required("--reason") {
        Ok(value) => value,
        Err(error) => return verb_fail("mark-offload", &error),
    };
    let now = match parse_now(args) {
        Ok(value) => value,
        Err(error) => return verb_fail("mark-offload", &error),
    };
    let stream = args.one("--stream").unwrap_or("_default");
    let mut names = args
        .all("--file")
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if names.is_empty() {
        return verb_fail("mark-offload", "at least one --file is required");
    }
    names.sort();
    if names.windows(2).any(|names| names[0] == names[1]) {
        return verb_fail("mark-offload", "--file names must be unique");
    }
    let target = Target {
        day: day.to_owned(),
        stream: stream.to_owned(),
        dir: dir.to_owned(),
    };
    let found = scan_segment(
        &journal.join(crate_layout_segment_rel(&target)),
        &ClosedHandlerSet,
        &JournalMedia,
    );
    let mut bytes = 0u64;
    for name in &names {
        let Some(item) = found.iter().find(|item| item.name.as_str() == name) else {
            return emit(
                serde_json::json!({
                    "ok": false,
                    "verb": "mark-offload",
                    "error": format!("`{name}` is not a present owner-media file in this segment"),
                }),
                EXIT_REFUSED,
            );
        };
        bytes = bytes.saturating_add(item.size);
    }
    match upsert_offload(&journal, &target, names, bytes, reason.to_owned(), now) {
        Ok(register) => emit(
            serde_json::json!({ "ok": true, "verb": "mark-offload", "marks": register }),
            EXIT_OK,
        ),
        Err(error) => emit(
            serde_json::json!({ "ok": false, "verb": "mark-offload", "error": error.to_string() }),
            EXIT_REFUSED,
        ),
    }
}

fn run_resolve_offload(args: &Args) -> ExitCode {
    let journal = match args.required("--journal") {
        Ok(value) => PathBuf::from(value),
        Err(error) => return verb_fail("resolve-offload", &error),
    };
    let day = match args.required("--day") {
        Ok(value) => value,
        Err(error) => return verb_fail("resolve-offload", &error),
    };
    let dir = match args.required("--dir") {
        Ok(value) => value,
        Err(error) => return verb_fail("resolve-offload", &error),
    };
    let stream = args.one("--stream").unwrap_or("_default");
    let mut names = args
        .all("--file")
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if names.is_empty() {
        return verb_fail("resolve-offload", "at least one --file is required");
    }
    names.sort();
    if names.windows(2).any(|names| names[0] == names[1]) {
        return verb_fail("resolve-offload", "--file names must be unique");
    }
    let target = Target {
        day: day.to_owned(),
        stream: stream.to_owned(),
        dir: dir.to_owned(),
    };
    match resolve_offload(&journal, &target, &names) {
        Ok(register) => emit(
            serde_json::json!({ "ok": true, "verb": "resolve-offload", "marks": register }),
            EXIT_OK,
        ),
        Err(error) => emit(
            serde_json::json!({ "ok": false, "verb": "resolve-offload", "error": error.to_string() }),
            EXIT_REFUSED,
        ),
    }
}

fn run_remove_marked(args: &Args) -> ExitCode {
    let journal = match args.required("--journal") {
        Ok(value) => PathBuf::from(value),
        Err(error) => return verb_fail("remove-marked", &error),
    };
    if let Some((flag, _)) = args.values.iter().find(|(flag, _)| {
        flag != "--journal"
            && flag != "--today"
            && flag != "--now"
            && flag != "--policy"
            && flag != "--mark"
    }) {
        return verb_fail("remove-marked", &format!("unknown flag `{flag}`"));
    }
    let today = match parse_today(args) {
        Ok(value) => value,
        Err(error) => return verb_fail("remove-marked", &error),
    };
    let now = match parse_now(args) {
        Ok(value) => value,
        Err(error) => return verb_fail("remove-marked", &error),
    };
    let policy = match parse_policy(args) {
        Ok(value) => value,
        Err(error) => return verb_fail("remove-marked", &error),
    };
    let ids = match args
        .all("--mark")
        .into_iter()
        .map(|value| MarkId::parse(value).ok_or_else(|| format!("`{value}` is not a mark ID")))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(value) => value,
        Err(error) => return verb_fail("remove-marked", &error),
    };
    let marks = match preflight(&journal, &ids) {
        Ok(marks) => marks,
        Err(error) => {
            return emit(
                serde_json::json!({ "ok": false, "verb": "remove-marked", "error": error.to_string() }),
                EXIT_REFUSED,
            );
        }
    };
    let mut register_errors = Vec::new();
    let outcome = remove_marked(&journal, &marks, &policy, today, now, &mut register_errors);
    finish(
        &journal,
        outcome,
        serde_json::json!({
            "verb": "remove-marked",
            "register_error": (!register_errors.is_empty()).then(|| register_errors.join("; ")),
        }),
    )
}

fn run_decline(args: &Args) -> ExitCode {
    let journal = match args.required("--journal") {
        Ok(value) => PathBuf::from(value),
        Err(error) => return verb_fail("decline", &error),
    };
    if let Some((flag, _)) = args
        .values
        .iter()
        .find(|(flag, _)| flag != "--journal" && flag != "--mark")
    {
        return verb_fail("decline", &format!("unknown flag `{flag}`"));
    }
    let ids = args.all("--mark");
    let [id] = ids.as_slice() else {
        return verb_fail("decline", "exactly one --mark is required");
    };
    let id = match MarkId::parse(id) {
        Some(id) => id,
        None => return verb_fail("decline", &format!("`{id}` is not a mark ID")),
    };
    match decline(&journal, &id) {
        Ok(register) => emit(
            serde_json::json!({ "ok": true, "verb": "decline", "marks": register }),
            EXIT_OK,
        ),
        Err(error) => emit(
            serde_json::json!({ "ok": false, "verb": "decline", "error": error.to_string() }),
            EXIT_REFUSED,
        ),
    }
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
            serde_json::json!({ "ok": true, "verb": "prune-logs", "executed": false, "plan": described }),
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
  release-raw     --journal P --at ISO --segment DAY/STREAM/DIR [--segment ...]
  recover         --journal P --at ISO --did ID [--reason owner|policy]
  sweep           --journal P --today YYYY-MM-DD --now ISO [--policy JSON] [--force true]
  prune-logs      --journal P --today YYYY-MM-DD --days N [--execute true]
  mark            --journal P --today YYYY-MM-DD --now ISO [--policy JSON]
  marks           --journal P
  mark-offload    --journal P --day DAY [--stream STREAM] --dir DIR --file NAME [--file ...] --reason REF --now ISO
  resolve-offload --journal P --day DAY [--stream STREAM] --dir DIR --file NAME [--file ...]
  remove-marked   --journal P --today YYYY-MM-DD --now ISO [--policy JSON] --mark ID [--mark ...]
  decline         --journal P --mark ID

--force executes the sweep. Use it only with the owner's express consent.
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
        "mark" => run_mark(&args),
        "marks" => run_marks(&args),
        "mark-offload" => run_mark_offload(&args),
        "resolve-offload" => run_resolve_offload(&args),
        "remove-marked" => run_remove_marked(&args),
        "decline" => run_decline(&args),
        other => fail(&format!("unknown verb `{other}`")),
    }
}
