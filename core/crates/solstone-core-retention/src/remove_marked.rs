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
    Failure, Mark, MarkId, PreflightMarks, load as load_marks, record_failure,
    resolve as resolve_mark,
};
use crate::receipt::{NotRemoved, Outcome, RunHalt, TargetOutcome};
use crate::scan::scan_segment;
use crate::{Eligibility, Policy};

const TOO_YOUNG: &str = "this one isn't old enough to delete yet.";
const KEPT_FOREVER: &str = "this one is kept indefinitely.";
const ANCHOR_MISSING: &str = "there's no date on this one, so it can't be deleted.";
const NOT_ON_REMOVAL_LIST: &str =
    "this file was proven releasable but is not on the removal list, so it is left in place";
const NO_LONGER_PRESENT: &str = "this file was on the removal list but is no longer present";
const NO_LONGER_ON_REMOVAL_LIST: &str =
    "this entry is no longer on the removal list, so nothing was removed";

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
        let register = match load_marks(journal) {
            Ok(register) => register,
            Err(error) => return Ok(refused(mark, &error.to_string())),
        };
        if register.marks.get(id) != Some(mark) {
            return Ok(refused(mark, NO_LONGER_ON_REMOVAL_LIST));
        }
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

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    reason = "test setup and assertions use concise infallible helpers"
)]
mod tests {
    use std::fs;

    use chrono::TimeZone;

    use super::*;
    use crate::marks::{Proposal, RemovalClass, decline, preflight, reconcile};

    #[test]
    fn a_mark_declined_after_preflight_is_not_unlinked() {
        let journal = tempfile::tempdir().unwrap();
        let target = crate::Target {
            day: "20260701".to_owned(),
            stream: "field.audio".to_owned(),
            dir: "070000_17".to_owned(),
        };
        let segment = journal.path().join(crate::layout::segment_rel(
            &target.day,
            &target.stream,
            &target.dir,
        ));
        fs::create_dir_all(&segment).unwrap();
        let raw = segment.join("audio.flac");
        fs::write(&raw, b"the owner's originals").unwrap();
        let proposal = Proposal {
            bytes: 1,
            reason: "test approval".to_owned(),
            names: vec!["audio.flac".to_owned()],
        };
        let register = reconcile(
            journal.path(),
            RemovalClass::PolicyRawRelease,
            &[(target.clone(), proposal.clone())],
            "first",
        )
        .unwrap();
        let id = register.marks.keys().next().unwrap().clone();
        let marks = preflight(journal.path(), std::slice::from_ref(&id)).unwrap();
        decline(journal.path(), &id).unwrap();
        let policy = Policy {
            default_rule: crate::Rule {
                anchor: crate::Anchor::Captured,
                period: Some(crate::Days(1)),
                priority: 0,
            },
            enabled: true,
            ..Policy::default()
        };
        let today = NaiveDate::from_ymd_opt(2026, 8, 6).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 8, 6, 0, 0, 0).single().unwrap();
        let mut register_errors = Vec::new();
        let outcome = remove_marked(
            journal.path(),
            &marks,
            &policy,
            today,
            now,
            "2026-08-06T00:00:00Z",
            &mut register_errors,
        );

        assert!(raw.exists());
        assert!(register_errors.is_empty());
        assert_eq!(outcome.targets.len(), 1);
        assert_eq!(outcome.targets[0].removed, Vec::new());
        assert_eq!(outcome.targets[0].not_removed.len(), 1);
        assert_eq!(
            outcome.targets[0].not_removed[0].reason,
            NO_LONGER_ON_REMOVAL_LIST
        );
    }
}
