// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Execute approved raw-release marks after proving them again from disk.

use std::collections::BTreeSet;
use std::path::Path;

use chrono::{DateTime, NaiveDate, Utc};
use solstone_core_journal_io::{LockOptions, hold_lock};

use crate::age::segment_age;
use crate::content::{ClosedHandlerSet, JournalMedia};
use crate::door::release_raw;
use crate::eligibility::{RawRelease, resolve};
use crate::marks::{
    Failure, Mark, MarkId, PreflightMarks, record_failure, resolve as resolve_mark,
};
use crate::receipt::{NotRemoved, Outcome, RunHalt, TargetOutcome};
use crate::scan::scan_segment;
use crate::{Eligibility, Policy};

const TOO_YOUNG: &str =
    "your retention settings don't release these originals yet. they aren't old enough.";
const KEPT_FOREVER: &str = "your retention settings keep these originals indefinitely.";
const ANCHOR_MISSING: &str =
    "i don't have a record of when these originals are from, so i can't release them.";
const NOT_ON_REMOVAL_LIST: &str =
    "this file was proven releasable but is not on the removal list, so it is left in place";
const NO_LONGER_PRESENT: &str = "this file was on the removal list but is no longer present";

struct RemovalContext<'a> {
    policy: &'a Policy,
    today: NaiveDate,
    now: DateTime<Utc>,
    at: &'a str,
    register_errors: &'a mut Vec<String>,
}

/// Execute marks already proven valid by preflight.
///
/// A row describes a target the run reached; `halted` describes the target it
/// could not start and every requested target after it.
pub fn remove_marked(
    journal: &Path,
    marks: &PreflightMarks,
    policy: &Policy,
    today: NaiveDate,
    now: DateTime<Utc>,
    at: &str,
    register_errors: &mut Vec<String>,
) -> Outcome {
    let mut context = RemovalContext {
        policy,
        today,
        now,
        at,
        register_errors,
    };
    let mut outcome = Outcome {
        targets: Vec::new(),
        halted: None,
    };
    for (index, (id, mark)) in marks.as_slice().iter().enumerate() {
        match remove_one(journal, id, mark, &mut context) {
            Ok(row) => outcome.targets.push(row),
            Err(()) => {
                let remaining = marks.as_slice().len().saturating_sub(index);
                let reason = format!(
                    "i couldn't start on the originals for {} because something else is using them. the rest of the removal list wasn't attempted ({remaining} remaining).",
                    id.as_str()
                );
                if outcome.targets.is_empty() {
                    return Outcome::halted_before_start(reason);
                }
                outcome.halted = Some(RunHalt { reason });
                break;
            }
        }
    }
    outcome
}

fn remove_one(
    journal: &Path,
    id: &MarkId,
    mark: &Mark,
    context: &mut RemovalContext<'_>,
) -> Result<TargetOutcome, ()> {
    let target = &mark.target;
    let live = crate::layout::segment_rel(&target.day, &target.stream, &target.dir);
    let (row, complete, staged) = {
        let _segment_lock =
            hold_lock(journal.join(&live), LockOptions::default()).map_err(|_| ())?;
        let found = scan_segment(&journal.join(&live), &ClosedHandlerSet, &JournalMedia);
        let records = found
            .iter()
            .map(|item| item.sidecar.record.as_ref())
            .collect::<Vec<_>>();
        let eligibility = context.policy.evaluate(
            &target.stream,
            segment_age(&target.day, &records, context.today, context.now),
        );
        match eligibility {
            Eligibility::Eligible { .. } => {}
            Eligibility::TooYoung { .. } => return Ok(refused(mark, TOO_YOUNG)),
            Eligibility::KeptForever => return Ok(refused(mark, KEPT_FOREVER)),
            Eligibility::AnchorMissing { .. } => return Ok(refused(mark, ANCHOR_MISSING)),
        }
        let ready = match resolve(
            &ClosedHandlerSet,
            &JournalMedia,
            &target.day,
            &target.stream,
            &target.dir,
            &found,
        ) {
            RawRelease::Releasable(ready) => ready,
            RawRelease::Held(blockers) => {
                return Ok(refused(
                    mark,
                    &format!(
                        "the current processing proof no longer permits this release: {}",
                        blockers
                            .iter()
                            .map(|blocker| blocker.name())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            }
        };
        let found_names = found
            .iter()
            .map(|item| item.name.as_str())
            .collect::<BTreeSet<_>>();
        let desired = mark
            .proposal
            .names
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let not_approved = ready
            .iter()
            .filter(|item| !desired.contains(item.name()))
            .map(|item| item.name().to_owned())
            .collect::<Vec<_>>();
        let ready = ready
            .into_iter()
            .filter(|item| desired.contains(item.name()))
            .collect::<Vec<_>>();
        let (partial, _) = release_raw(journal, &ready);
        let mut row = partial.targets.into_iter().next().unwrap_or(TargetOutcome {
            target: target.clone(),
            removed: Vec::new(),
            not_removed: Vec::new(),
        });
        for name in not_approved {
            row.not_removed.push(NotRemoved {
                entry: crate::layout::content_rel(&target.day, &target.stream, &target.dir, &name),
                reason: NOT_ON_REMOVAL_LIST.to_owned(),
                staged: None,
            });
        }
        for name in &mark.proposal.names {
            if !found_names.contains(name.as_str()) {
                row.not_removed.push(NotRemoved {
                    entry: crate::layout::content_rel(
                        &target.day,
                        &target.stream,
                        &target.dir,
                        name,
                    ),
                    reason: NO_LONGER_PRESENT.to_owned(),
                    staged: None,
                });
            }
        }
        let removed = row
            .removed
            .iter()
            .map(|path| path.as_str())
            .collect::<BTreeSet<_>>();
        let accounted = mark.proposal.names.iter().all(|name| {
            let rel = crate::layout::content_rel(&target.day, &target.stream, &target.dir, name);
            removed.contains(rel.as_str())
                || row
                    .not_removed
                    .iter()
                    .any(|item| item.reason == NO_LONGER_PRESENT && item.entry == rel)
        });
        let complete = accounted && !row.not_removed.iter().any(|item| item.staged.is_some());
        let staged = row.not_removed.iter().find_map(|item| {
            item.staged
                .as_ref()
                .map(|path| (path.to_owned(), item.reason.to_owned()))
        });
        (row, complete, staged)
    };
    // Segment lock, then register lock, always: marks owns the second lock internally.
    if complete {
        if let Err(error) = resolve_mark(journal, id) {
            context.register_errors.push(error.to_string());
        }
    } else if let Some((staged, reason)) = staged
        && let Err(error) = record_failure(
            journal,
            mark.class,
            target,
            &mark.proposal.names,
            Failure {
                at: context.at.to_owned(),
                reason,
                staged: Some(staged),
            },
            context.at,
        )
    {
        context.register_errors.push(error.to_string());
    }
    Ok(row)
}

fn refused(mark: &Mark, reason: &str) -> TargetOutcome {
    TargetOutcome {
        target: mark.target.clone(),
        removed: Vec::new(),
        not_removed: vec![NotRemoved {
            entry: crate::layout::segment_rel(
                &mark.target.day,
                &mark.target.stream,
                &mark.target.dir,
            ),
            reason: reason.to_owned(),
            staged: None,
        }],
    }
}
