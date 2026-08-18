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
//! eligible  ⟺  age_by(anchor) >= period          no period at all means keep forever
//! ```
//!
//! Then *keep* is no period, *after N days* is `{captured, N}`, *once processing
//! completes* is `{processed, 0 days}`, and *a week after processing* is
//! `{processed, 7 days}` for free. An **absent** period is the default value of the
//! field, so "forever" is what an unset setting means and there is no variant anyone
//! can forget to handle.
//!
//! ⚠ Absence and zero are deliberately different spellings. They were the same one
//! -- zero meant forever -- and the collision made `{processed, 0}` silently
//! `KeptForever`, so the mode this module documents as free did not work and no test
//! said so. It was found by mapping the reference's config onto this type.
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
use serde_json::{Map, Value};

use crate::class::MediaClass;

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
    /// How long to keep. ⛔ `None` is FOREVER; `Some(Days(0))` is "as soon as the
    /// anchor has a value", which is what makes *once processing completes*
    /// expressible as `{processed, Some(Days(0))}`.
    ///
    /// ⚠ This was a bare `Days` with zero meaning forever, and the two readings of
    /// zero collided: `evaluate` short-circuited on zero before it ever looked at the
    /// anchor, so `{processed, 0}` -- the mode this module's own documentation
    /// promised fell out for free -- was silently `KeptForever` and no test covered
    /// it. Absence and immediacy are different things and now have different
    /// spellings; `#[serde(default)]` keeps an unset field meaning forever.
    #[serde(default)]
    pub period: Option<Days>,
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
            period: None,
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
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
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
    // ⛔ Absence means forever. An immediate period (`Some(Days(0))`) must fall
    // through to the anchor check below, or *once processing completes* is
    // inexpressible.
    let Some(period) = rule.period else {
        return Eligibility::KeptForever;
    };
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
    if age_days >= period.0 {
        Eligibility::Eligible {
            anchor: rule.anchor,
            age_days,
            period,
        }
    } else {
        Eligibility::TooYoung {
            anchor: rule.anchor,
            age_days,
            period,
        }
    }
}

/// The configured policy: a default, per-stream overrides, a class rule, and a floor.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Policy {
    /// Applies to any stream without its own rule.
    pub default_rule: Rule,
    /// Rules for named streams, highest priority first when several match.
    #[serde(default)]
    pub per_stream: Vec<(String, Rule)>,
    /// Journal-global empty-audio class rule; `per_stream` does not name it.
    ///
    /// Wire omit (`--policy` JSON / `Policy::default`) is `Rule::keep()`. Journal
    /// omit (`policy_from_retention`) is `{processed, Some(Days(0))}`. The two
    /// defaults are opposite on purpose: a hand-written policy that does not name
    /// the class must not start releasing; a journal that has never heard of the
    /// class must get the product default (eligible once processed).
    #[serde(default = "Rule::keep")]
    pub empty_audio_rule: Rule,
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
            empty_audio_rule: Rule::keep(),
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

    /// Decide a segment. The minimum-age floor applies only to ordinary media.
    pub fn evaluate(&self, stream: &str, age: SegmentAge, class: MediaClass) -> Eligibility {
        if !self.enabled {
            return Eligibility::KeptForever;
        }
        match class {
            MediaClass::NoDecodableAudio => evaluate(self.empty_audio_rule, age),
            MediaClass::Ordinary => self.evaluate_ordinary(stream, age),
        }
    }

    fn evaluate_ordinary(&self, stream: &str, age: SegmentAge) -> Eligibility {
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

/// Project the journal retention object into the policy the removal engine evaluates.
///
/// An unrepresentable period becomes no period, which keeps recordings forever and is
/// therefore safer than choosing a finite duration. The minimum-age floor has the
/// opposite retaining direction: a larger floor keeps recordings longer, so a
/// fractional or oversized numeric floor rounds upward and saturates at the largest
/// representable whole-day value.
pub fn policy_from_retention(retention: &Map<String, Value>) -> Policy {
    let per_stream = retention
        .get("per_stream")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(name, value)| {
            value.as_object().map(|stream| {
                (
                    name.clone(),
                    rule_from_retention(
                        stream.get("raw_media").and_then(Value::as_str),
                        stream.get("raw_media_days"),
                    ),
                )
            })
        })
        .collect();
    Policy {
        default_rule: rule_from_retention(
            retention.get("raw_media").and_then(Value::as_str),
            retention.get("raw_media_days"),
        ),
        per_stream,
        empty_audio_rule: empty_audio_rule_from_retention(retention),
        minimum_age: minimum_age(retention.get("raw_media_minimum_days")),
        enabled: true,
    }
}

/// Whether at least one configured rule can release raw media.
pub fn policy_would_release(policy: &Policy) -> bool {
    policy.default_rule.period.is_some()
        || policy
            .per_stream
            .iter()
            .any(|(_, rule)| rule.period.is_some())
        || policy.empty_audio_rule.period.is_some()
}

fn empty_audio_rule_from_retention(retention: &Map<String, Value>) -> Rule {
    match retention.get("empty_audio").and_then(Value::as_str) {
        None => Rule {
            anchor: Anchor::Processed,
            period: Some(Days(0)),
            priority: 0,
        },
        Some(mode) => rule_from_retention(Some(mode), retention.get("empty_audio_days")),
    }
}

fn rule_from_retention(mode: Option<&str>, days: Option<&Value>) -> Rule {
    match mode {
        Some("days") => Rule {
            anchor: Anchor::Captured,
            period: period(days),
            priority: 0,
        },
        Some("processed") => Rule {
            anchor: Anchor::Processed,
            period: Some(Days(0)),
            priority: 0,
        },
        _ => Rule::keep(),
    }
}

fn period(value: Option<&Value>) -> Option<Days> {
    let ParsedDays::Integer(days) = parse_days(value)? else {
        return None;
    };
    (days > 0).then(|| Days(saturating_u32(days)))
}

fn minimum_age(value: Option<&Value>) -> Days {
    match parse_days(value) {
        Some(ParsedDays::Integer(days) | ParsedDays::Fractional(days)) if days > 0 => {
            Days(saturating_u32(days))
        }
        _ => Days(0),
    }
}

fn saturating_u32(days: i64) -> u32 {
    u32::try_from(days).unwrap_or(u32::MAX)
}

enum ParsedDays {
    Integer(i64),
    Fractional(i64),
}

fn parse_days(value: Option<&Value>) -> Option<ParsedDays> {
    match value? {
        Value::Bool(value) => Some(ParsedDays::Integer(i64::from(*value))),
        Value::Number(number) => number
            .as_i64()
            .map(ParsedDays::Integer)
            .or_else(|| {
                number
                    .as_u64()
                    .map(|days| ParsedDays::Integer(i64::try_from(days).unwrap_or(i64::MAX)))
            })
            .or_else(|| number.as_f64().and_then(parse_float_days)),
        Value::String(value) => parse_integer_string(value).map(ParsedDays::Integer),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

fn parse_integer_string(value: &str) -> Option<i64> {
    let value = value.trim();
    if let Ok(days) = value.parse::<i64>() {
        return Some(days);
    }
    let (negative, digits) = if let Some(digits) = value.strip_prefix('-') {
        (true, digits)
    } else {
        (false, value.strip_prefix('+').unwrap_or(value))
    };
    (!digits.is_empty() && digits.bytes().all(|digit| digit.is_ascii_digit()))
        .then_some(if negative { i64::MIN } else { i64::MAX })
}

fn parse_float_days(days: f64) -> Option<ParsedDays> {
    if !days.is_finite() {
        return None;
    }
    let rounded = days.ceil();
    let rounded = if rounded >= i64::MAX as f64 {
        i64::MAX
    } else if rounded <= i64::MIN as f64 {
        i64::MIN
    } else {
        rounded as i64
    };
    if days.fract() == 0.0 {
        Some(ParsedDays::Integer(rounded))
    } else {
        Some(ParsedDays::Fractional(rounded))
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
    use crate::class::MediaClass;
    use serde_json::json;

    fn armed(default_rule: Rule) -> Policy {
        Policy {
            default_rule,
            enabled: true,
            ..Policy::default()
        }
    }

    fn retention(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn journal_retention_projection_handles_every_duration_shape() {
        for (value, period, minimum_age) in [
            (json!(7), Some(7), 7),
            (json!(-7), None, 0),
            (json!(0), None, 0),
            (json!(7.5), None, 8),
            (json!(-7.5), None, 0),
            (json!(true), Some(1), 1),
            (json!(false), None, 0),
            (json!("30"), Some(30), 30),
            (json!(" 30 "), Some(30), 30),
            (json!("ninety"), None, 0),
            (Value::Null, None, 0),
            (json!([]), None, 0),
            (json!({}), None, 0),
            (json!(u64::MAX), Some(u32::MAX), u32::MAX),
            (json!("999999999999999999999"), Some(u32::MAX), u32::MAX),
        ] {
            let policy = policy_from_retention(&retention(json!({
                "raw_media": "days",
                "raw_media_days": value.clone(),
                "raw_media_minimum_days": value,
            })));
            assert_eq!(policy.default_rule.period.map(|days| days.0), period);
            assert_eq!(policy.minimum_age, Days(minimum_age));
        }

        let policy = policy_from_retention(&retention(json!({"raw_media": "days"})));
        assert_eq!(policy.default_rule, Rule::keep());
        assert_eq!(policy.minimum_age, Days(0));
    }

    #[test]
    fn journal_retention_projection_preserves_modes_and_stream_overrides() {
        let processed = policy_from_retention(&retention(json!({"raw_media": "processed"})));
        assert_eq!(
            processed.default_rule,
            Rule {
                anchor: Anchor::Processed,
                period: Some(Days(0)),
                priority: 0,
            }
        );

        for mode in [
            json!("keep"),
            json!("unexpected"),
            json!(true),
            Value::Null,
            json!([]),
            json!({}),
        ] {
            let policy = policy_from_retention(&retention(json!({"raw_media": mode})));
            assert_eq!(policy.default_rule, Rule::keep());
        }

        let policy = policy_from_retention(&retention(json!({})));
        assert_eq!(policy.default_rule, Rule::keep());

        let policy = policy_from_retention(&retention(json!({
            "raw_media": "keep",
            "per_stream": {
                "audio": {"raw_media": "days", "raw_media_days": 7.5},
                "screen": {"raw_media": "processed"},
                "ignored": true,
            },
        })));
        assert_eq!(policy.per_stream.len(), 2);
        assert_eq!(policy.rule_for("audio"), Rule::keep());
        assert_eq!(
            policy.rule_for("screen"),
            Rule {
                anchor: Anchor::Processed,
                period: Some(Days(0)),
                priority: 0,
            }
        );

        let policy = policy_from_retention(&retention(json!({
            "raw_media": "days",
            "raw_media_days": 7,
            "per_stream": [],
        })));
        assert!(policy.per_stream.is_empty());
    }

    #[test]
    fn journal_retention_projection_identifies_releasing_rules() {
        let keep = policy_from_retention(&retention(
            json!({"raw_media": "keep", "empty_audio": "keep"}),
        ));
        assert!(!policy_would_release(&keep));

        let absent_class = policy_from_retention(&retention(json!({"raw_media": "keep"})));
        assert!(policy_would_release(&absent_class));

        let days = policy_from_retention(&retention(json!({
            "raw_media": "days",
            "raw_media_days": 1,
        })));
        assert!(policy_would_release(&days));

        let stream = policy_from_retention(&retention(json!({
            "raw_media": "keep",
            "per_stream": {"audio": {"raw_media": "processed"}},
        })));
        assert!(policy_would_release(&stream));
    }

    #[test]
    fn absent_empty_audio_keys_project_to_processed_immediate() {
        let policy = policy_from_retention(&retention(json!({"raw_media": "keep"})));
        assert_eq!(
            policy.empty_audio_rule,
            Rule {
                anchor: Anchor::Processed,
                period: Some(Days(0)),
                priority: 0,
            }
        );
    }

    #[test]
    fn policy_would_release_includes_empty_audio_rule() {
        assert!(policy_would_release(&policy_from_retention(&retention(
            json!({"raw_media": "keep"})
        ))));
        assert!(policy_would_release(&policy_from_retention(&retention(
            json!({"raw_media": "keep", "empty_audio": "processed"})
        ))));
        assert!(policy_would_release(&policy_from_retention(&retention(
            json!({"raw_media": "keep", "empty_audio": "days", "empty_audio_days": 7})
        ))));
        assert!(!policy_would_release(&policy_from_retention(&retention(
            json!({"raw_media": "keep", "empty_audio": "keep"})
        ))));
        assert!(!policy_would_release(&policy_from_retention(&retention(
            json!({"raw_media": "keep", "empty_audio": "days", "empty_audio_days": 0})
        ))));
    }

    #[test]
    fn empty_audio_class_skips_minimum_age_floor() {
        let policy = Policy {
            empty_audio_rule: Rule {
                anchor: Anchor::Processed,
                period: Some(Days(0)),
                priority: 0,
            },
            minimum_age: Days(30),
            enabled: true,
            ..Policy::default()
        };
        let young = SegmentAge {
            since_captured: Some(2),
            since_processed: Some(0),
        };
        assert!(
            policy
                .evaluate("field.audio", young, MediaClass::NoDecodableAudio)
                .is_eligible()
        );
        assert!(
            !policy
                .evaluate("field.audio", young, MediaClass::Ordinary)
                .is_eligible()
        );
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
                    period: Some(Days(7)),
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
                    period: Some(Days(1)),
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
                    period: Some(Days(7)),
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
                period: Some(Days(1)),
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
            period: Some(Days(1)),
            priority: 0,
        };
        let mut policy = Policy {
            default_rule: rule,
            ..Policy::default()
        };
        assert_eq!(
            policy.evaluate("field.audio", old, MediaClass::Ordinary),
            Eligibility::KeptForever
        );
        policy.enabled = true;
        assert!(
            policy
                .evaluate("field.audio", old, MediaClass::Ordinary)
                .is_eligible()
        );
    }

    /// 🔴 The floor cannot be undercut by any rule.
    /// 🔴 The mode this module documents as free, and which silently never fired.
    ///
    /// The reference offers *once processing completes*. It is spelled
    /// `{processed, 0 days}`, and until the period became an `Option` it collided with
    /// the spelling for *forever* -- `evaluate` short-circuited on zero before it ever
    /// consulted the anchor. Eight tests passed and none of them was this one.
    #[test]
    fn once_processing_completes_is_expressible_and_actually_fires() {
        let rule = Rule {
            anchor: Anchor::Processed,
            period: Some(Days(0)),
            priority: 0,
        };
        let processed = SegmentAge {
            since_captured: Some(0),
            since_processed: Some(0),
        };
        assert_eq!(
            evaluate(rule, processed),
            Eligibility::Eligible {
                anchor: Anchor::Processed,
                age_days: 0,
                period: Days(0),
            },
            "processing has finished, so a zero-day processed rule releases"
        );

        // ⛔ And it still fails closed when processing has NOT finished: an immediate
        // period is not a licence to release something with no processed anchor.
        let unprocessed = SegmentAge {
            since_captured: Some(9999),
            since_processed: None,
        };
        assert_eq!(
            evaluate(rule, unprocessed),
            Eligibility::AnchorMissing {
                anchor: Anchor::Processed
            },
        );
    }

    /// Absence and zero are different, and only absence keeps forever.
    #[test]
    fn an_absent_period_keeps_forever_and_a_zero_period_does_not() {
        let ancient = SegmentAge {
            since_captured: Some(9999),
            since_processed: Some(9999),
        };
        let keeps = Rule {
            anchor: Anchor::Captured,
            period: None,
            priority: 0,
        };
        assert_eq!(evaluate(keeps, ancient), Eligibility::KeptForever);
        assert_eq!(Rule::keep().period, None, "the default keeps");

        let immediate = Rule {
            period: Some(Days(0)),
            ..keeps
        };
        assert!(
            evaluate(immediate, ancient).is_eligible(),
            "a zero period is immediate, not forever"
        );
    }

    /// An unset period deserialises as forever, so a partial config cannot delete.
    #[test]
    fn a_rule_with_no_period_in_json_keeps_forever() {
        let rule: Rule = serde_json::from_str(r#"{"anchor": "captured", "priority": 0}"#)
            .expect("a rule with no period is valid");
        assert_eq!(rule.period, None);
        assert_eq!(
            evaluate(
                rule,
                SegmentAge {
                    since_captured: Some(9999),
                    since_processed: Some(9999),
                }
            ),
            Eligibility::KeptForever,
            "⛔ an omitted period must be the SAFEST setting, not the most destructive"
        );
    }

    #[test]
    fn the_minimum_age_overrides_a_shorter_rule() {
        // 🔴 `{processed, 0 days}` -- release as soon as processing finishes -- is
        // the exact misconfiguration the floor exists to catch, and it is now
        // expressible. 📌 This test previously built it, then immediately rebuilt the
        // policy with `Days(1)` under a comment explaining that `Days(0)` meant
        // forever. That comment was a written record of noticing the defect and
        // routing around it in a test rather than fixing it.
        let policy = Policy {
            default_rule: Rule {
                anchor: Anchor::Processed,
                period: Some(Days(0)),
                priority: 0,
            },
            minimum_age: Days(30),
            enabled: true,
            ..Policy::default()
        };
        let fresh = SegmentAge {
            since_captured: Some(2),
            since_processed: Some(2),
        };
        assert!(
            !policy
                .evaluate("field.audio", fresh, MediaClass::Ordinary)
                .is_eligible(),
            "the floor must hold against a rule that would release immediately"
        );
        let old = SegmentAge {
            since_captured: Some(60),
            since_processed: Some(60),
        };
        assert!(
            policy
                .evaluate("field.audio", old, MediaClass::Ordinary)
                .is_eligible()
        );
    }

    /// A per-stream rule shadows the default entirely.
    #[test]
    fn a_per_stream_rule_shadows_the_default_rather_than_merging() {
        let policy = Policy {
            default_rule: Rule {
                anchor: Anchor::Captured,
                period: Some(Days(1)),
                priority: 0,
            },
            per_stream: vec![(
                "field.audio".to_owned(),
                Rule {
                    anchor: Anchor::Captured,
                    // ⛔ `None`, not `Some(Days(0))`: this rule KEEPS.
                    period: None,
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
            policy.evaluate("field.audio", old, MediaClass::Ordinary),
            Eligibility::KeptForever,
            "the stream's own rule keeps it, and the default does not leak through"
        );
        assert!(
            policy
                .evaluate("field.screen", old, MediaClass::Ordinary)
                .is_eligible()
        );
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
                        period: Some(Days(1)),
                        priority: 1,
                    },
                ),
                (
                    "field.audio".to_owned(),
                    Rule {
                        anchor: Anchor::Captured,
                        // ⛔ `None`, not `Some(Days(0))`: this rule KEEPS.
                        period: None,
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
            policy.evaluate("field.audio", old, MediaClass::Ordinary),
            Eligibility::KeptForever,
            "the higher-priority rule wins, and here it keeps"
        );
    }

    #[test]
    fn a_verdict_carries_what_it_measured() {
        let policy = armed(Rule {
            anchor: Anchor::Captured,
            period: Some(Days(7)),
            priority: 0,
        });
        match policy.evaluate(
            "field.audio",
            SegmentAge {
                since_captured: Some(3),
                since_processed: None,
            },
            MediaClass::Ordinary,
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
