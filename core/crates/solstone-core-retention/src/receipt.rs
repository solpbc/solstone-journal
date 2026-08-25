// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What a removal run reports, and the path type that proves a removal happened.
//!
//! Both removal verbs report against the same unit -- a segment -- so one
//! [`TargetOutcome`] serves the whole-segment verb and the raw-release verb
//! alike. The raw-release verb operates on individual files, and groups them by
//! the segment each was proven in.

use serde::{Deserialize, Serialize};

/// One segment a removal run acted on. The unit BOTH verbs report against.
///
/// ⛔ `Ord`/`PartialOrd` are deliberately absent. They are what makes a
/// `BTreeMap<Target, _>` buildable, and a keyed collection silently collapses a
/// duplicate target -- so a run given the same segment twice would report one
/// row where the caller supplied two, and an owner's receipt would disagree with
/// their request. `Vec` cannot do that.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Target {
    pub day: String,
    pub stream: String,
    /// The segment's **directory name**.
    ///
    /// ⛔ Never a key derived from it. The two differ: the journal's segment-key
    /// scan reads `093000_300` out of a directory named `093000_300_summary`, so
    /// a path rebuilt from a key misses the real directory entirely, and a lock
    /// keyed on one while a recovery pass keys on the other excludes nothing.
    pub dir: String,
}

/// A journal-relative path this crate removed **and verified absent**.
///
/// The type exists so that "tell the index about a removal that has not
/// happened" is unrepresentable rather than merely forbidden: the constructor is
/// private to this crate, and the only code that calls it is the removal door,
/// after confirming the path is gone.
///
/// ⛔ Deliberately `Serialize` and **not** `Deserialize`. Deriving `Deserialize`
/// generates a constructor *inside* this crate and thereby hands every
/// downstream crate a factory -- `from_str` would mint one for a file nothing
/// ever removed, and it would flow into the search-index prune. A privacy check
/// would still pass, because the field really is private. Nothing reads one of
/// these back: recovery reads the filesystem.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RemovedPath(String);

impl RemovedPath {
    /// Record a path as removed. ⛔ Callers must have confirmed it is absent.
    ///
    /// ⚠ Unused in production until the door module lands, and deliberately so:
    /// this wave ships the type that makes a removal claim provable, not the code
    /// that makes one. Without this allow, `-D warnings` reds the workspace clippy
    /// gate for every crate in it.
    #[allow(
        dead_code,
        reason = "the only production caller is the door module, added in a later wave"
    )]
    pub(crate) fn confirmed(path: String) -> Self {
        Self(path)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One entry that did not complete, and why, in language an owner can read.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NotRemoved {
    /// The journal-relative entry this concerns.
    pub entry: String,
    pub reason: String,
    /// Where a half-finished segment was staged, when recovery is needed.
    pub staged: Option<String>,
}

/// A durable removal completed, but a required follow-up publication did not.
///
/// This is deliberately separate from [`NotRemoved`]: the owner's content is
/// already gone and must never be described as rolled back or refused.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PostCommitFailure {
    pub entry: String,
    pub reason: String,
}

/// The run itself stopped before reaching every target -- a lock timeout, an
/// abort, a budget.
///
/// ⛔ Never a single target's failure. A target that failed is a
/// [`TargetOutcome`] row; collapsing one into a run-level field is how a
/// forty-target removal reports one of its two failures.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RunHalt {
    pub reason: String,
}

/// What happened to one segment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TargetOutcome {
    pub target: Target,
    /// Every path removed from this segment, each verified absent.
    pub removed: Vec<RemovedPath>,
    /// Every entry that did not complete. Empty means the target did.
    ///
    /// 🔴 Plural, and that is load-bearing. The raw-release verb groups N proven
    /// files into one of these, so a single slot would name one of two files
    /// failing for different reasons in the same segment. Structured per-item
    /// results: one entry cannot discard a sibling's.
    pub not_removed: Vec<NotRemoved>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_commit_failure: Option<PostCommitFailure>,
}

/// One removal run. Always complete, whatever went wrong.
///
/// 🔴 The verbs return this and never a `Result`. `?` is only legal in a
/// function returning `Result`/`Option`, so a verb returning `Outcome` cannot
/// lose it by propagating an error out from under the caller -- which is exactly
/// how the reference implementation tells an owner an irreversible removal
/// failed after it had already completed.
///
/// ⚠ `#[must_use]` is not decoration. `Result` carries it implicitly, so a bare
/// return type would *delete* a compiler guard against the owner never seeing
/// this at all.
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Outcome {
    /// One row per target **reached**, in the order supplied.
    ///
    /// ⛔ May be shorter than the request when `halted` is set; never longer and
    /// never reordered. A consumer must not assume this matches the request's
    /// length.
    pub targets: Vec<TargetOutcome>,
    /// The run stopped before reaching every target. The rows above are still
    /// accurate as far as they go. A row describes a target the run reached;
    /// `halted` describes the target it could not start and every requested
    /// target after it.
    pub halted: Option<RunHalt>,
}

impl Outcome {
    /// A run that reached nothing, because it stopped first.
    pub fn halted_before_start(reason: String) -> Self {
        Self {
            targets: Vec::new(),
            halted: Some(RunHalt { reason }),
        }
    }

    /// Whether any target reported an entry it could not complete.
    pub fn has_failures(&self) -> bool {
        self.targets
            .iter()
            .any(|target| !target.not_removed.is_empty() || target.post_commit_failure.is_some())
    }

    /// Every path this run removed, across all targets.
    pub fn removed_paths(&self) -> impl Iterator<Item = &RemovedPath> {
        self.targets.iter().flat_map(|target| &target.removed)
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "test code; the crate-level denials exist to constrain the verbs"
)]
mod tests {
    use super::*;

    fn target(dir: &str) -> Target {
        Target {
            day: "20260805".to_owned(),
            stream: "field.audio".to_owned(),
            dir: dir.to_owned(),
        }
    }

    fn failed(entry: &str, reason: &str) -> NotRemoved {
        NotRemoved {
            entry: entry.to_owned(),
            reason: reason.to_owned(),
            staged: None,
        }
    }

    /// Two targets failing for different reasons must BOTH be readable.
    ///
    /// This is the shape a single `Option<RunHalt>` could not represent: a
    /// forty-segment removal in which two segments fail names one of them.
    #[test]
    fn two_targets_failing_differently_are_both_reported() {
        let outcome = Outcome {
            targets: vec![
                TargetOutcome {
                    target: target("070000_17"),
                    removed: vec![RemovedPath::confirmed("a.flac".to_owned())],
                    not_removed: Vec::new(),
                    post_commit_failure: None,
                },
                TargetOutcome {
                    target: target("070100_17"),
                    removed: Vec::new(),
                    not_removed: vec![failed("b.flac", "the file is a directory")],
                    post_commit_failure: None,
                },
                TargetOutcome {
                    target: target("070200_17"),
                    removed: Vec::new(),
                    not_removed: vec![failed("c.flac", "permission denied reading the entry")],
                    post_commit_failure: None,
                },
            ],
            halted: None,
        };

        assert_eq!(outcome.targets.len(), 3);
        assert_eq!(outcome.targets[0].removed.len(), 1);
        let second = &outcome.targets[1].not_removed[0].reason;
        let third = &outcome.targets[2].not_removed[0].reason;
        assert_ne!(
            second, third,
            "both failures must be independently readable"
        );
        assert!(outcome.has_failures());
        assert!(
            outcome.halted.is_none(),
            "a target failure is not a run halt"
        );
    }

    /// The same target twice yields TWO rows.
    ///
    /// A keyed collection satisfies the test above with three distinct targets
    /// while silently collapsing a duplicate. This is what separates `Vec` from
    /// `BTreeMap<Target, _>`, and it is why `Target` has no `Ord`.
    #[test]
    fn a_duplicate_target_yields_two_rows() {
        let outcome = Outcome {
            targets: vec![
                TargetOutcome {
                    target: target("070000_17"),
                    removed: vec![RemovedPath::confirmed("a.flac".to_owned())],
                    not_removed: Vec::new(),
                    post_commit_failure: None,
                },
                TargetOutcome {
                    target: target("070000_17"),
                    removed: Vec::new(),
                    not_removed: vec![failed("a.flac", "already removed")],
                    post_commit_failure: None,
                },
            ],
            halted: None,
        };
        assert_eq!(outcome.targets.len(), 2);
        assert_eq!(outcome.targets[0].target, outcome.targets[1].target);
    }

    /// One TargetOutcome must hold two failures for the SAME segment.
    ///
    /// The raw-release verb groups N proven files into one row, so an `Option`
    /// here would name one of two files failing for different reasons -- the
    /// same defect as above, one level down.
    #[test]
    fn one_target_holds_two_entry_failures() {
        let outcome = TargetOutcome {
            target: target("070000_17"),
            removed: vec![RemovedPath::confirmed("kept.flac".to_owned())],
            not_removed: vec![
                failed("audio.flac", "permission denied reading the entry"),
                failed("video.mp4", "the read failed part way through"),
            ],
            post_commit_failure: None,
        };
        assert_eq!(outcome.not_removed.len(), 2);
        assert_ne!(outcome.not_removed[0].reason, outcome.not_removed[1].reason);
        assert_eq!(outcome.not_removed[0].entry, "audio.flac");
        assert_eq!(outcome.not_removed[1].entry, "video.mp4");
    }

    /// A run may carry a halt AND per-target failures at once, and the emitted
    /// JSON must distinguish them.
    #[test]
    fn a_halt_and_a_target_failure_are_both_emitted() {
        let outcome = Outcome {
            targets: vec![TargetOutcome {
                target: target("070000_17"),
                removed: Vec::new(),
                not_removed: vec![failed("a.flac", "the file is a directory")],
                post_commit_failure: None,
            }],
            halted: Some(RunHalt {
                reason: "another process holds this segment".to_owned(),
            }),
        };
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(json.contains("another process holds this segment"));
        assert!(json.contains("the file is a directory"));
    }

    #[test]
    fn a_target_keeps_its_directory_name_when_the_key_would_differ() {
        // The journal's segment-key scan reads `093000_300` out of this name.
        let held = target("093000_300_summary");
        assert_eq!(held.dir, "093000_300_summary");
    }

    #[test]
    fn halted_before_start_reaches_nothing() {
        let outcome = Outcome::halted_before_start("the journal is locked".to_owned());
        assert!(outcome.targets.is_empty());
        assert!(outcome.halted.is_some());
        assert!(!outcome.has_failures());
        assert_eq!(outcome.removed_paths().count(), 0);
    }
}
