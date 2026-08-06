// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Execute approved raw-release marks after proving them again from disk.

use std::collections::BTreeSet;
use std::path::Path;

use chrono::{DateTime, NaiveDate, Utc};
use solstone_core_journal_io::{LockOptions, hold_lock};

use crate::Policy;
use crate::age::segment_age;
use crate::content::{ClosedHandlerSet, JournalMedia};
use crate::door::release_raw;
use crate::eligibility::{RawRelease, resolve};
use crate::marks::{
    Approval, Failure, Mark, MarkId, MarkState, load, record_failure, resolve as resolve_mark,
};
use crate::receipt::{NotRemoved, Outcome, TargetOutcome};
use crate::scan::scan_segment;

/// Execute the explicitly named marks. Preflight errors remove nothing.
pub fn remove_marked(
    journal: &Path,
    ids: &[MarkId],
    policy: &Policy,
    today: NaiveDate,
    now: DateTime<Utc>,
    at: &str,
) -> Result<Outcome, String> {
    if ids.is_empty() {
        return Err("at least one --mark is required".to_owned());
    }
    let mut unique = BTreeSet::new();
    if ids.iter().any(|id| !unique.insert(id.as_str())) {
        return Err("the same mark was named more than once".to_owned());
    }
    let register = load(journal).map_err(|error| error.to_string())?;
    let mut marks = Vec::new();
    for id in ids {
        let Some(mark) = register.marks.get(id) else {
            return Err(format!(
                "no mark named `{}` exists; run `marks` to see current marks — an id changes whenever its proposal's file list changes",
                id.as_str()
            ));
        };
        if mark.class.axes().1 != Approval::Required {
            return Err(format!("mark `{}` does not require approval", id.as_str()));
        }
        if matches!(mark.state, MarkState::Failed(_)) {
            return Err(format!("mark `{}` has a recorded failure", id.as_str()));
        }
        marks.push((id.clone(), mark.clone()));
    }

    let mut outcome = Outcome {
        targets: Vec::new(),
        halted: None,
    };
    for (id, mark) in marks {
        outcome
            .targets
            .push(remove_one(journal, &id, &mark, policy, today, now, at)?);
    }
    Ok(outcome)
}

fn remove_one(
    journal: &Path,
    id: &MarkId,
    mark: &Mark,
    policy: &Policy,
    today: NaiveDate,
    now: DateTime<Utc>,
    at: &str,
) -> Result<TargetOutcome, String> {
    let target = &mark.target;
    let live = crate::layout::segment_rel(&target.day, &target.stream, &target.dir);
    let (row, complete, staged) = {
        let _segment_lock = hold_lock(journal.join(&live), LockOptions::default())
            .map_err(|error| format!("another process is working on this segment ({error})"))?;
        let found = scan_segment(&journal.join(&live), &ClosedHandlerSet, &JournalMedia);
        let records = found
            .iter()
            .map(|item| item.sidecar.record.as_ref())
            .collect::<Vec<_>>();
        let eligibility = policy.evaluate(
            &target.stream,
            segment_age(&target.day, &records, today, now),
        );
        if !eligibility.is_eligible() {
            return Ok(refused(
                mark,
                &format!(
                    "the current retention policy no longer proposes this release: {eligibility:?}"
                ),
            ));
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
                reason: "this file was proven releasable but is not named in this mark's proposal, so it is left in place".to_owned(),
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
                    reason: "this file was named in the proposal but is no longer present"
                        .to_owned(),
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
                || row.not_removed.iter().any(|item| {
                    item.reason == "this file was named in the proposal but is no longer present"
                        && item.entry == rel
                })
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
        resolve_mark(journal, id).map_err(|error| error.to_string())?;
    } else if let Some((staged, reason)) = staged {
        record_failure(
            journal,
            mark.class,
            target,
            &mark.proposal.names,
            Failure {
                at: at.to_owned(),
                reason,
                staged: Some(staged),
            },
            at,
        )
        .map_err(|error| error.to_string())?;
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
