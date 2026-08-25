// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Execute approved raw-release marks after proving them again from disk.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use chrono::{DateTime, NaiveDate, Utc};
use solstone_core_journal_io::{LockOptions, hold_lock};

use crate::age::segment_age;
use crate::class::{classify, partition_empty_audio};
use crate::content::{ClosedHandlerSet, JournalMedia};
use crate::door::release_raw;
use crate::eligibility::{Blocker, FoundContent, ProvenRaw, RawRelease, resolve};
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
    register_errors: &mut Vec<String>,
) -> Outcome {
    let mut context = RemovalContext {
        policy,
        today,
        now,
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
        let found_names = found
            .iter()
            .map(|item| item.name.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        let (empty, ordinary) = partition_empty_audio(found, &ClosedHandlerSet);
        let empty_side = decide_side(&empty, target, context);
        let ordinary_side = decide_side(&ordinary, target, context);
        let mut side_refusal = BTreeMap::new();
        record_side_refusal(&empty, &empty_side, &mut side_refusal);
        record_side_refusal(&ordinary, &ordinary_side, &mut side_refusal);
        let ready = ready_union(empty_side, ordinary_side);
        let proved_names = ready
            .iter()
            .map(|item| item.name().to_owned())
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
            post_commit_failure: None,
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
        for name in &mark.proposal.names {
            if found_names.contains(name.as_str())
                && !proved_names.contains(name)
                && let Some(reason) = side_refusal.get(name)
            {
                row.not_removed.push(NotRemoved {
                    entry: crate::layout::content_rel(
                        &target.day,
                        &target.stream,
                        &target.dir,
                        name,
                    ),
                    reason: reason.clone(),
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
                at: String::new(),
                reason,
                staged: Some(staged),
            },
            context.now,
        )
    {
        context.register_errors.push(error.to_string());
    }
    Ok(row)
}

enum SideOutcome {
    None,
    Policy(Eligibility),
    Held(Vec<Blocker>),
    Proved(Vec<ProvenRaw>),
}

fn decide_side(
    side: &[FoundContent],
    target: &crate::Target,
    context: &RemovalContext<'_>,
) -> SideOutcome {
    if side.is_empty() {
        return SideOutcome::None;
    }
    let records = side
        .iter()
        .map(|item| item.sidecar.record.as_ref())
        .collect::<Vec<_>>();
    let verdict = context.policy.evaluate(
        &target.stream,
        segment_age(&target.day, &records, context.today, context.now),
        classify(side, &ClosedHandlerSet),
    );
    if !verdict.is_eligible() {
        return SideOutcome::Policy(verdict);
    }
    match resolve(
        &ClosedHandlerSet,
        &JournalMedia,
        &target.day,
        &target.stream,
        &target.dir,
        side,
    ) {
        RawRelease::Releasable(ready) if ready.is_empty() => SideOutcome::None,
        RawRelease::Releasable(ready) => SideOutcome::Proved(ready),
        RawRelease::Held(blockers) => SideOutcome::Held(blockers),
    }
}

fn ready_union(empty: SideOutcome, ordinary: SideOutcome) -> Vec<ProvenRaw> {
    let mut ready = match empty {
        SideOutcome::Proved(proven) => proven,
        _ => Vec::new(),
    };
    if let SideOutcome::Proved(proven) = ordinary {
        ready.extend(proven);
    }
    ready
}

fn record_side_refusal(
    side: &[FoundContent],
    outcome: &SideOutcome,
    side_refusal: &mut BTreeMap<String, String>,
) {
    let reason = match outcome {
        SideOutcome::Policy(Eligibility::TooYoung { .. }) => TOO_YOUNG.to_owned(),
        SideOutcome::Policy(Eligibility::KeptForever) => KEPT_FOREVER.to_owned(),
        SideOutcome::Policy(Eligibility::AnchorMissing { .. }) => ANCHOR_MISSING.to_owned(),
        SideOutcome::Policy(Eligibility::Eligible { .. })
        | SideOutcome::None
        | SideOutcome::Proved(_) => {
            return;
        }
        SideOutcome::Held(blockers) => format!(
            "the current processing proof no longer permits this release: {}",
            blockers
                .iter()
                .map(|blocker| blocker.name())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    for item in side {
        side_refusal.insert(item.name.as_str().to_owned(), reason.clone());
    }
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
        post_commit_failure: None,
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
    use std::path::Path;

    use chrono::TimeZone;

    use super::*;
    use crate::marks::{Proposal, RemovalClass, decline, preflight, reconcile};
    use crate::policy::policy_from_retention;
    use serde_json::json;

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
            crate::marks::mark_at("first"),
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

    fn target() -> crate::Target {
        crate::Target {
            day: "20260701".to_owned(),
            stream: "field.audio".to_owned(),
            dir: "070000_17".to_owned(),
        }
    }

    fn keep_journal_product_policy() -> Policy {
        policy_from_retention(json!({"raw_media": "keep"}).as_object().unwrap())
    }

    fn seed_empty_audio(journal: &Path, target: &crate::Target) -> std::path::PathBuf {
        let segment = journal.join(crate::layout::segment_rel(
            &target.day,
            &target.stream,
            &target.dir,
        ));
        fs::create_dir_all(&segment).unwrap();
        let raw = b"raw";
        fs::write(segment.join("audio.flac"), raw).unwrap();
        let header = json!({
            "segment": &target.dir,
            "_solstone_processing": {
                "schema": "solstone.processing.v1",
                "state": "empty",
                "reason_code": "no_decodable_audio",
                "handler": "transcribe",
                "attempted_at": "2026-07-01T00:00:00Z",
                "input_size": raw.len(),
            }
        });
        fs::write(segment.join("audio.jsonl"), format!("{header}\n")).unwrap();
        segment
    }

    fn seed_analyzed_sibling(segment: &Path, name: &str) {
        let raw = b"sibling";
        fs::write(segment.join(name), raw).unwrap();
        let stem = name.rsplit_once('.').unwrap().0;
        let header = json!({
            "segment": "070000_17",
            "_solstone_processing": {
                "schema": "solstone.processing.v1",
                "state": "analyzed",
                "reason_code": "ok",
                "handler": "transcribe",
                "attempted_at": "2026-07-01T00:00:00Z",
                "input_size": raw.len(),
            }
        });
        fs::write(
            segment.join(format!("{stem}.jsonl")),
            format!("{header}\n{{\"start\":0.0,\"text\":\"x\"}}\n"),
        )
        .unwrap();
    }

    fn approve(
        journal: &Path,
        names: Vec<String>,
        policy: &Policy,
    ) -> (crate::Outcome, Vec<String>) {
        let target = target();
        let proposal = Proposal {
            bytes: 1,
            reason: "test approval".to_owned(),
            names,
        };
        let register = reconcile(
            journal,
            RemovalClass::PolicyRawRelease,
            &[(target, proposal)],
            crate::marks::mark_at("first"),
        )
        .unwrap();
        let id = register.marks.keys().next().unwrap().clone();
        let marks = preflight(journal, std::slice::from_ref(&id)).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 8, 6).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 8, 6, 0, 0, 0).single().unwrap();
        let mut register_errors = Vec::new();
        let outcome = remove_marked(journal, &marks, policy, today, now, &mut register_errors);
        (outcome, register_errors)
    }

    #[test]
    fn an_unnamed_ordinary_sibling_does_not_block_or_appear_on_an_empty_audio_mark() {
        let journal = tempfile::tempdir().unwrap();
        let target = target();
        let segment = seed_empty_audio(journal.path(), &target);
        seed_analyzed_sibling(&segment, "extra.flac");
        let audio = segment.join("audio.flac");
        let extra = segment.join("extra.flac");
        let policy = keep_journal_product_policy();
        let (outcome, register_errors) =
            approve(journal.path(), vec!["audio.flac".to_owned()], &policy);

        assert!(!audio.exists());
        assert!(extra.exists());
        assert!(register_errors.is_empty());
        assert_eq!(outcome.targets.len(), 1);
        assert_eq!(outcome.targets[0].removed.len(), 1);
        assert!(
            outcome.targets[0].removed[0]
                .as_str()
                .ends_with("audio.flac")
        );
        assert_eq!(outcome.targets[0].not_removed, Vec::new());
    }

    #[test]
    fn a_mark_naming_both_sides_releases_only_the_eligible_file() {
        let journal = tempfile::tempdir().unwrap();
        let target = target();
        let segment = seed_empty_audio(journal.path(), &target);
        seed_analyzed_sibling(&segment, "extra.flac");
        let audio = segment.join("audio.flac");
        let extra = segment.join("extra.flac");
        let policy = keep_journal_product_policy();
        let (outcome, register_errors) = approve(
            journal.path(),
            vec!["audio.flac".to_owned(), "extra.flac".to_owned()],
            &policy,
        );

        assert!(!audio.exists());
        assert!(extra.exists());
        assert!(register_errors.is_empty());
        assert_eq!(outcome.targets[0].removed.len(), 1);
        assert!(
            outcome.targets[0].removed[0]
                .as_str()
                .ends_with("audio.flac")
        );
        assert_eq!(outcome.targets[0].not_removed.len(), 1);
        assert!(
            outcome.targets[0].not_removed[0]
                .entry
                .ends_with("extra.flac")
        );
        assert_eq!(outcome.targets[0].not_removed[0].reason, KEPT_FOREVER);
    }

    #[test]
    fn a_named_file_that_is_no_longer_empty_terminal_is_refused_per_file() {
        let journal = tempfile::tempdir().unwrap();
        let target = target();
        let segment = seed_empty_audio(journal.path(), &target);
        let audio = segment.join("audio.flac");
        seed_analyzed_sibling(&segment, "audio.flac");
        let policy = keep_journal_product_policy();
        let (outcome, register_errors) =
            approve(journal.path(), vec!["audio.flac".to_owned()], &policy);

        assert!(audio.exists());
        assert!(register_errors.is_empty());
        assert_eq!(outcome.targets[0].removed, Vec::new());
        assert_eq!(outcome.targets[0].not_removed.len(), 1);
        assert!(
            outcome.targets[0].not_removed[0]
                .entry
                .ends_with("audio.flac")
        );
        assert_eq!(outcome.targets[0].not_removed[0].reason, KEPT_FOREVER);
    }
}
