// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! When the owner's raw originals become eligible for release.
//!
//! # One predicate with a parameterised anchor, not three modes
//!
//! The reference offers three modes — keep, after N days, once processing
//! completes — and a fourth is always about to be needed ("a week *after*
//! processing"). Every mature retention engine converged on one shape instead:
//!
//! ```text
//! eligible  ⟺  age_by(anchor) >= period          period == 0 means keep forever
//! ```
//!
//! Then *keep* is `period: 0`, *after N days* is `{captured, N}`, *once processing
//! completes* is `{processed, 0}`, and *a week after processing* is
//! `{processed, 7d}` for free. Zero-means-forever also makes "forever" the default
//! value of the same field, so there is no variant anyone can forget to handle.
//!
//! # ⛔ A missing anchor is never eligible
//!
//! If the anchor a rule names has no value, the segment is **not** eligible. That
//! sounds obvious and one major engine does the opposite: it substitutes a
//! different anchor when the named one is null. Here that behaviour would delete
//! recordings that were never processed.
//!
//! # ⛔ Precedence is declared, and resolves toward keeping
//!
//! Object-storage engines resolve overlapping rules toward the *shorter* retention,
//! because their cost model is a storage bill. This resolves toward the **longer**,
//! because the thing at stake is the owner's only copy of a recording. An explicit
//! priority decides between rules; nothing is derived from rule shape.

use serde::{Deserialize, Serialize};

/// A duration in whole days. Zero means forever.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Days(pub u32);

impl Days {
    /// Whether this period ever elapses.
    ///
    /// ⛔ Zero is *forever*, not *immediately*. Reading it the other way would make
    /// an unset field the most destructive setting available.
    pub fn keeps_forever(self) -> bool {
        self.0 == 0
    }
}

/// What a rule measures age from.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    /// When the segment's content was recorded.
    Captured,
    /// When processing of the segment finished.
    ///
    /// ⚠ Must be a write-once, non-decreasing value on disk. If it can move
    /// backwards, a rule that has already fired can un-fire; if it can be rewritten,
    /// the value that authorised a deletion is not the value that recorded it.
    Processed,
}

/// One retention rule.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Rule {
    pub anchor: Anchor,
    pub period: Days,
    /// Higher wins when several rules match. Author-declared, ⛔ never derived from
    /// the rules' shapes — a reader must be able to tell which rule applies without
    /// simulating the engine.
    pub priority: i32,
}

impl Rule {
    /// Keep forever. The default for anything unstated.
    pub fn keep() -> Self {
        Self {
            anchor: Anchor::Captured,
            period: Days(0),
            priority: 0,
        }
    }
}

/// What a segment offers a rule to measure against.
#[derive(Clone, Copy, Debug, Default)]
pub struct SegmentAge {
    /// Whole days since the content was recorded.
    pub since_captured: Option<u32>,
    /// Whole days since processing finished, if it has.
    pub since_processed: Option<u32>,
}

/// Why a segment is or is not eligible, in terms a receipt can carry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Eligibility {
    Eligible {
        anchor: Anchor,
        age_days: u32,
        period: Days,
    },
    /// The matching rule keeps this forever.
    KeptForever,
    /// Not yet old enough.
    TooYoung {
        anchor: Anchor,
        age_days: u32,
        period: Days,
    },
    /// ⛔ The anchor the rule names has no value on this segment.
    AnchorMissing { anchor: Anchor },
}

impl Eligibility {
    pub fn is_eligible(self) -> bool {
        matches!(self, Self::Eligible { .. })
    }
}

/// Decide one segment against one rule.
pub fn evaluate(rule: Rule, age: SegmentAge) -> Eligibility {
    if rule.period.keeps_forever() {
        return Eligibility::KeptForever;
    }
    let measured = match rule.anchor {
        Anchor::Captured => age.since_captured,
        Anchor::Processed => age.since_processed,
    };
    // ⛔ Fail closed. Never substitute the other anchor: that is how an engine
    // deletes content it was told to measure by something it does not have.
    let Some(age_days) = measured else {
        return Eligibility::AnchorMissing {
            anchor: rule.anchor,
        };
    };
    if age_days >= rule.period.0 {
        Eligibility::Eligible {
            anchor: rule.anchor,
            age_days,
            period: rule.period,
        }
    } else {
        Eligibility::TooYoung {
            anchor: rule.anchor,
            age_days,
            period: rule.period,
        }
    }
}

/// The configured policy: a default, per-stream overrides, and a floor.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Policy {
    /// Applies to any stream without its own rule.
    pub default_rule: Rule,
    /// Rules for named streams, highest priority first when several match.
    #[serde(default)]
    pub per_stream: Vec<(String, Rule)>,
    /// 🔴 No rule may release raw younger than this, whatever it says.
    ///
    /// The backstop against a misconfigured `{processed, 0}` reaching content the
    /// owner has never seen. ⛔ Enforced in code below, not in a comment.
    #[serde(default)]
    pub minimum_age: Days,
    /// 🔴 The destructive path is OFF unless explicitly armed.
    ///
    /// ⚠ This is also where the reference effectively is today — its policy has no
    /// runner at all — so defaulting to off preserves current behaviour rather than
    /// changing it silently at a version boundary.
    #[serde(default)]
    pub enabled: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            default_rule: Rule::keep(),
            per_stream: Vec::new(),
            minimum_age: Days(0),
            enabled: false,
        }
    }
}

impl Policy {
    /// The rule that governs a stream.
    ///
    /// A per-stream rule **shadows** the default entirely rather than merging with
    /// it: partial merging across scopes is where override semantics stop being
    /// explicable.
    pub fn rule_for(&self, stream: &str) -> Rule {
        self.per_stream
            .iter()
            .filter(|(name, _)| name == stream)
            .map(|(_, rule)| *rule)
            .max_by_key(|rule| rule.priority)
            .unwrap_or(self.default_rule)
    }

    /// Decide a segment, floor included.
    pub fn evaluate(&self, stream: &str, age: SegmentAge) -> Eligibility {
        if !self.enabled {
            return Eligibility::KeptForever;
        }
        let verdict = evaluate(self.rule_for(stream), age);
        // The floor is applied after the rule, so a rule can never undercut it.
        if let Eligibility::Eligible {
            anchor,
            age_days,
            // ⚠ Deliberately discarded: the verdict below reports the FLOOR's
            // period, because the floor is what blocks. Reporting the rule's would
            // tell an owner a number that does not explain the outcome.
            period: _,
        } = verdict
            && !self.minimum_age.keeps_forever()
            && age_days < self.minimum_age.0
        {
            return Eligibility::TooYoung {
                anchor,
                age_days,
                period: self.minimum_age,
            };
        }
        verdict
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

    fn armed(default_rule: Rule) -> Policy {
        Policy {
            default_rule,
            enabled: true,
            ..Policy::default()
        }
    }

    #[test]
    fn zero_means_forever_not_immediately() {
        assert!(Days(0).keeps_forever());
        assert!(!Days(1).keeps_forever());
        let verdict = evaluate(
            Rule::keep(),
            SegmentAge {
                since_captured: Some(9_999),
                since_processed: Some(9_999),
            },
        );
        assert_eq!(verdict, Eligibility::KeptForever);
    }

    /// The four reference behaviours, expressed in the one shape.
    #[test]
    fn one_shape_expresses_every_mode() {
        let age = SegmentAge {
            since_captured: Some(30),
            since_processed: Some(2),
        };
        // keep
        assert_eq!(evaluate(Rule::keep(), age), Eligibility::KeptForever);
        // after N days
        assert!(
            evaluate(
                Rule {
                    anchor: Anchor::Captured,
                    period: Days(7),
                    priority: 0
                },
                age
            )
            .is_eligible()
        );
        // once processing completes
        assert!(
            evaluate(
                Rule {
                    anchor: Anchor::Processed,
                    period: Days(1),
                    priority: 0
                },
                age
            )
            .is_eligible()
        );
        // a week after processing -- the mode the reference cannot express
        assert!(
            !evaluate(
                Rule {
                    anchor: Anchor::Processed,
                    period: Days(7),
                    priority: 0
                },
                age
            )
            .is_eligible()
        );
    }

    /// ⛔ A missing anchor is never eligible, and never falls back.
    #[test]
    fn a_missing_anchor_fails_closed_rather_than_substituting() {
        let unprocessed = SegmentAge {
            since_captured: Some(9_999),
            since_processed: None,
        };
        let verdict = evaluate(
            Rule {
                anchor: Anchor::Processed,
                period: Days(1),
                priority: 0,
            },
            unprocessed,
        );
        assert_eq!(
            verdict,
            Eligibility::AnchorMissing {
                anchor: Anchor::Processed
            },
            "a very old but unprocessed segment must NOT be released by a \
             processed-anchored rule"
        );
        assert!(!verdict.is_eligible());
    }

    #[test]
    fn the_destructive_path_is_off_until_armed() {
        let old = SegmentAge {
            since_captured: Some(9_999),
            since_processed: Some(9_999),
        };
        let rule = Rule {
            anchor: Anchor::Captured,
            period: Days(1),
            priority: 0,
        };
        let mut policy = Policy {
            default_rule: rule,
            ..Policy::default()
        };
        assert_eq!(
            policy.evaluate("field.audio", old),
            Eligibility::KeptForever
        );
        policy.enabled = true;
        assert!(policy.evaluate("field.audio", old).is_eligible());
    }

    /// 🔴 The floor cannot be undercut by any rule.
    #[test]
    fn the_minimum_age_overrides_a_shorter_rule() {
        let policy = Policy {
            default_rule: Rule {
                anchor: Anchor::Processed,
                period: Days(0),
                priority: 0,
            },
            minimum_age: Days(30),
            enabled: true,
            ..Policy::default()
        };
        // period 0 on `processed` would be "as soon as processing finishes" -- but
        // Days(0) is forever, so express the misconfiguration as period 1.
        let policy = Policy {
            default_rule: Rule {
                anchor: Anchor::Processed,
                period: Days(1),
                priority: 0,
            },
            ..policy
        };
        let fresh = SegmentAge {
            since_captured: Some(2),
            since_processed: Some(2),
        };
        assert!(
            !policy.evaluate("field.audio", fresh).is_eligible(),
            "the floor must hold against a rule that would release immediately"
        );
        let old = SegmentAge {
            since_captured: Some(60),
            since_processed: Some(60),
        };
        assert!(policy.evaluate("field.audio", old).is_eligible());
    }

    /// A per-stream rule shadows the default entirely.
    #[test]
    fn a_per_stream_rule_shadows_the_default_rather_than_merging() {
        let policy = Policy {
            default_rule: Rule {
                anchor: Anchor::Captured,
                period: Days(1),
                priority: 0,
            },
            per_stream: vec![(
                "field.audio".to_owned(),
                Rule {
                    anchor: Anchor::Captured,
                    period: Days(0),
                    priority: 0,
                },
            )],
            enabled: true,
            ..Policy::default()
        };
        let old = SegmentAge {
            since_captured: Some(9_999),
            since_processed: Some(9_999),
        };
        assert_eq!(
            policy.evaluate("field.audio", old),
            Eligibility::KeptForever,
            "the stream's own rule keeps it, and the default does not leak through"
        );
        assert!(policy.evaluate("field.screen", old).is_eligible());
    }

    /// Declared priority decides, and the longer retention wins a tie of intent.
    #[test]
    fn declared_priority_decides_between_matching_rules() {
        let policy = Policy {
            default_rule: Rule::keep(),
            per_stream: vec![
                (
                    "field.audio".to_owned(),
                    Rule {
                        anchor: Anchor::Captured,
                        period: Days(1),
                        priority: 1,
                    },
                ),
                (
                    "field.audio".to_owned(),
                    Rule {
                        anchor: Anchor::Captured,
                        period: Days(0),
                        priority: 5,
                    },
                ),
            ],
            enabled: true,
            ..Policy::default()
        };
        let old = SegmentAge {
            since_captured: Some(9_999),
            since_processed: Some(9_999),
        };
        assert_eq!(
            policy.evaluate("field.audio", old),
            Eligibility::KeptForever,
            "the higher-priority rule wins, and here it keeps"
        );
    }

    #[test]
    fn a_verdict_carries_what_it_measured() {
        let policy = armed(Rule {
            anchor: Anchor::Captured,
            period: Days(7),
            priority: 0,
        });
        match policy.evaluate(
            "field.audio",
            SegmentAge {
                since_captured: Some(3),
                since_processed: None,
            },
        ) {
            Eligibility::TooYoung {
                anchor,
                age_days,
                period,
            } => {
                assert_eq!(anchor, Anchor::Captured);
                assert_eq!(age_days, 3);
                assert_eq!(period, Days(7));
            }
            other => panic!("expected TooYoung, got {other:?}"),
        }
    }
}
