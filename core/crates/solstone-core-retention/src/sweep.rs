// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The scheduled pass: the feature that has never run.
//!
//! The reference has a policy, a predicate and a `purge()`. It has no scheduler, no
//! maintenance routine and no timer, so nothing has ever called it. This is the
//! composition that would — and it is deliberately two halves.
//!
//! # 🔴 Planning is separate from executing, and planning touches nothing
//!
//! [`plan`] reads the chronicle and returns a value. It removes nothing, renames
//! nothing, writes nothing, and takes no lock. [`execute`] takes that value and
//! performs it. Three things follow, and each is the reason:
//!
//! 1. **The owner can be shown what a destructive scheduled job would do** before it
//!    is armed. A retention engine whose first observable act is a deletion cannot
//!    earn trust, and this one is being armed against a corpus that already exists.
//! 2. **The decision is a testable value.** Every property below is asserted against
//!    a plan, not inferred from a filesystem after the fact.
//! 3. **A refusal is data, not an absence.** Every segment the pass declines is in
//!    the plan with its reason. A sweep that reported only its deletions would make
//!    "nothing was eligible" and "the scan silently skipped everything"
//!    indistinguishable — which is exactly how a path bug hides.
//!
//! # ⛔ Two independent gates, in this order
//!
//! The policy decides *when*; the release predicate decides *whether there is proof*.
//! The policy is consulted first because it is cheap, but neither can overrule the
//! other: an eligible-by-age segment with no proof is held, and a proven segment that
//! is too young is held. A policy can only ever make the engine delete **less** than
//! the proof allows.
//!
//! # ⚠ What this pass will not do
//!
//! It releases proven raw originals. It never removes a segment — that verb exists,
//! and it answers to the owner, not to a timer. Nothing here can reach it.

use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate, Utc};
use solstone_core_journal_io::paths::{PathOrDay, day_dirs, iter_segments};

use crate::class::{classify, partition_empty_audio};
use crate::content::{HandlerRegistry, MediaClassifier};
use crate::door::{EvidenceTally, release_raw};
use crate::eligibility::{Blocker, FoundContent, ProvenRaw, RawRelease, resolve};
use crate::policy::{Eligibility, Policy};
use crate::receipt::{Outcome, Target};
use crate::scan::scan_segment;

/// A segment the pass would release raw from, and why it may.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub target: Target,
    /// The policy's verdict, carrying the anchor and the age it measured.
    pub eligibility: Eligibility,
    pub proven: Vec<ProvenRaw>,
}

impl Candidate {
    /// Bytes this candidate would reclaim.
    pub fn bytes(&self) -> u64 {
        self.proven
            .iter()
            .map(ProvenRaw::size)
            .fold(0u64, u64::saturating_add)
    }
}

/// Why a segment was not a candidate.
///
/// ⛔ Every variant is a *reason*, not a failure. A sweep over a healthy journal is
/// mostly skips, and each one has to be explicable to the owner.
#[derive(Clone, Debug)]
pub enum Skip {
    /// The segment holds no owner media, so there is nothing to release.
    NoMedia,
    /// The policy declined: kept forever, too young, or the anchor is missing.
    Policy(Eligibility),
    /// The policy allowed it and the release predicate did not.
    Held(Vec<Blocker>),
}

/// One skipped segment with its reason.
#[derive(Clone, Debug)]
pub struct Skipped {
    pub target: Target,
    pub reason: Skip,
}

/// What a pass would do, before it does anything.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    pub candidates: Vec<Candidate>,
    pub skipped: Vec<Skipped>,
    /// Days whose directory could not be listed. ⚠ Named rather than dropped: an
    /// unreadable day is the one case where a smaller plan is not good news.
    pub unreadable_days: Vec<String>,
    /// The chronicle root is missing or is not a directory.
    pub chronicle_unavailable: bool,
    /// Segment directories whose names are not UTF-8, so they cannot fill [`Target`].
    pub unrepresentable_segments: Vec<PathBuf>,
}

impl Plan {
    /// Segments examined, whatever the outcome.
    pub fn examined(&self) -> usize {
        self.candidates
            .len()
            .saturating_add(self.skipped.len())
            .saturating_add(self.unrepresentable_segments.len())
    }

    /// Bytes the plan would reclaim.
    pub fn bytes(&self) -> u64 {
        self.candidates
            .iter()
            .map(Candidate::bytes)
            .fold(0u64, u64::saturating_add)
    }

    /// Files the plan would release.
    pub fn files(&self) -> usize {
        self.candidates
            .iter()
            .map(|candidate| candidate.proven.len())
            .sum()
    }
}

/// Decide what a pass would release. **Reads only.**
///
/// `today` is the local calendar date the captured anchor is measured against and
/// `now` the instant the processed anchor is measured against; both are arguments
/// because this crate is forbidden to read the clock.
pub fn plan(
    journal: &Path,
    policy: &Policy,
    registry: &dyn HandlerRegistry,
    classifier: &dyn MediaClassifier,
    today: NaiveDate,
    now: DateTime<Utc>,
) -> Plan {
    let mut built = Plan::default();
    if !crate::layout::chronicle_root_is_dir(journal) {
        built.chronicle_unavailable = true;
        return built;
    }
    // A non-directory chronicle root is deterministic to exercise; permission-based
    // unreadability is not when tests run as root. Per-day failures remain named below.
    let Ok(days) = day_dirs(journal) else {
        built.chronicle_unavailable = true;
        return built;
    };
    // Deterministic order, so two plans over one journal compare.
    let mut days: Vec<(String, std::path::PathBuf)> = days.into_iter().collect();
    days.sort_by(|left, right| left.0.cmp(&right.0));

    for (day, day_dir) in days {
        let Ok(segments) = iter_segments(journal, PathOrDay::Directory(&day_dir)) else {
            built.unreadable_days.push(day);
            continue;
        };
        for segment in segments {
            // ⛔ The directory NAME, never `segment.key()`. The key is the
            // `HHMMSS_LEN` scanned out of the name and the two differ whenever a
            // name carries a suffix; a path built from the key addresses a
            // different directory, or none.
            let Some(identity) = segment.record_identity() else {
                built
                    .unrepresentable_segments
                    .push(segment.path().to_path_buf());
                continue;
            };
            let target = Target {
                day: day.clone(),
                stream: identity.stream.to_owned(),
                dir: identity.name.to_owned(),
            };

            let found = scan_segment(segment.path(), registry, classifier);
            if found.is_empty() {
                built.skipped.push(Skipped {
                    target,
                    reason: Skip::NoMedia,
                });
                continue;
            }

            let (empty, ordinary) = partition_empty_audio(found, registry);
            let empty_side = decide_side(&empty, registry, classifier, policy, &target, today, now);
            let ordinary_side =
                decide_side(&ordinary, registry, classifier, policy, &target, today, now);
            match merge_sides(empty_side, ordinary_side) {
                SideMerge::Candidate {
                    eligibility,
                    proven,
                } => built.candidates.push(Candidate {
                    target,
                    eligibility,
                    proven,
                }),
                SideMerge::Skipped(reason) => built.skipped.push(Skipped { target, reason }),
            }
        }
    }
    built
}

enum SideOutcome {
    None,
    Policy(Eligibility),
    Held(Vec<Blocker>),
    Proved {
        eligibility: Eligibility,
        proven: Vec<ProvenRaw>,
    },
}

enum SideMerge {
    Candidate {
        eligibility: Eligibility,
        proven: Vec<ProvenRaw>,
    },
    Skipped(Skip),
}

fn decide_side(
    side: &[FoundContent],
    registry: &dyn HandlerRegistry,
    classifier: &dyn MediaClassifier,
    policy: &Policy,
    target: &Target,
    today: NaiveDate,
    now: DateTime<Utc>,
) -> SideOutcome {
    if side.is_empty() {
        return SideOutcome::None;
    }
    let records: Vec<Option<&serde_json::Value>> = side
        .iter()
        .map(|item| item.sidecar.record.as_ref())
        .collect();
    let age = crate::age::segment_age(&target.day, &records, today, now);
    let class = classify(side, registry);
    let verdict = policy.evaluate(&target.stream, age, class);
    if !verdict.is_eligible() {
        return SideOutcome::Policy(verdict);
    }
    match resolve(
        registry,
        classifier,
        &target.day,
        &target.stream,
        &target.dir,
        side,
    ) {
        RawRelease::Releasable(proven) if proven.is_empty() => SideOutcome::None,
        RawRelease::Releasable(proven) => SideOutcome::Proved {
            eligibility: verdict,
            proven,
        },
        RawRelease::Held(blockers) => SideOutcome::Held(blockers),
    }
}

fn merge_sides(empty: SideOutcome, ordinary: SideOutcome) -> SideMerge {
    let (empty_eligibility, mut proven, empty_blockers, empty_policy) = take_side(empty);
    let (ordinary_eligibility, ordinary_proven, ordinary_blockers, ordinary_policy) =
        take_side(ordinary);
    proven.extend(ordinary_proven);
    let mut blockers = empty_blockers.unwrap_or_default();
    if let Some(held) = ordinary_blockers {
        blockers.extend(held);
    }
    if !proven.is_empty() {
        let eligibility = match empty_eligibility.or(ordinary_eligibility) {
            Some(eligibility) => eligibility,
            None => return SideMerge::Skipped(Skip::NoMedia),
        };
        return SideMerge::Candidate {
            eligibility,
            proven,
        };
    }
    if !blockers.is_empty() {
        return SideMerge::Skipped(Skip::Held(blockers));
    }
    if let Some(eligibility) = empty_policy.or(ordinary_policy) {
        return SideMerge::Skipped(Skip::Policy(eligibility));
    }
    SideMerge::Skipped(Skip::NoMedia)
}

fn take_side(
    outcome: SideOutcome,
) -> (
    Option<Eligibility>,
    Vec<ProvenRaw>,
    Option<Vec<Blocker>>,
    Option<Eligibility>,
) {
    match outcome {
        SideOutcome::None => (None, Vec::new(), None, None),
        SideOutcome::Policy(verdict) => (None, Vec::new(), None, Some(verdict)),
        SideOutcome::Held(blockers) => (None, Vec::new(), Some(blockers), None),
        SideOutcome::Proved {
            eligibility,
            proven,
        } => (Some(eligibility), proven, None, None),
    }
}

/// Perform a plan.
///
/// ⛔ Takes a [`Plan`] rather than re-deciding, so what is executed is what was
/// shown. It calls exactly one verb — [`release_raw`] — and there is no path from
/// here to segment removal.
///
/// ⚠ A plan is a snapshot. `release_raw` re-proves containment and existence per
/// file at the moment it acts, so a stale plan loses files rather than removing the
/// wrong ones.
///
/// ⛔ No `#[must_use]` here, and none is needed: the returned tuple already carries
/// it from [`Outcome`], so discarding this receipt is a compile error either way.
/// Verified by control, not assumed.
pub fn execute(journal: &Path, plan: &Plan) -> (Outcome, EvidenceTally) {
    let proven: Vec<ProvenRaw> = plan
        .candidates
        .iter()
        .flat_map(|candidate| candidate.proven.iter().cloned())
        .collect();
    release_raw(journal, &proven)
}
