// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! How old a segment is, by each anchor the policy can name.
//!
//! # ⛔ This crate cannot ask what time it is
//!
//! Every function here takes the current instant as an argument, and
//! `tests/architecture.rs` bans the clock from the crate's sources outright. A
//! retention engine that reads the clock inside its decision has a decision no test
//! can pin and no receipt can reproduce: the same segment and the same policy must
//! always yield the same verdict for a given instant.
//!
//! # Both anchors round toward keeping
//!
//! A period is a promise to keep for **at least** that long, so every rounding here
//! is toward *younger*. Two different arithmetics get there, because the two sources
//! are different kinds of thing:
//!
//! - **Captured** comes from the day directory, which is a local calendar date with
//!   no time of day the engine can trust. A segment captured at 23:59 has had ~0
//!   elapsed time by the next calendar day, so the age is measured from the **end**
//!   of the captured day. `Days(7)` therefore holds for 7–8 days rather than 6–7.
//! - **Processed** comes from the record's `attempted_at`, a real instant, so the
//!   elapsed time is exact and simply truncated to whole days.
//!
//! # ⛔ Why `attempted_at` is a sound processed anchor
//!
//! It names when the attempt *started*, which sounds like the wrong end. It is sound
//! here for a reason specific to this caller: nothing reaches an age question until
//! the record has already passed terminal proof, so the attempt provably finished.
//! On a retry the field is restamped **forward**, which makes content look younger
//! and holds it longer; and a terminal record is not retried, so its value is
//! stable. It is written beside the data it describes and never re-derived.
//!
//! ⚠ Its one imprecision — the attempt's own duration, minutes at most — makes
//! content look *older*, and is contained by the policy's minimum-age floor, which
//! is anchored on the immutable captured date.

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value;

use crate::policy::SegmentAge;

/// The day-directory format, matching the reference's `%Y%m%d`.
const DAY_FORMAT: &str = "%Y%m%d";

/// The record field naming when the processing attempt began.
const ATTEMPTED_AT: &str = "attempted_at";

/// Whole days since the **end** of the captured day.
///
/// `None` when the day directory is not a date, which fails closed: an
/// unparseable day is not a young segment, it is an unmeasurable one.
pub fn captured_age_days(day: &str, today: NaiveDate) -> Option<u32> {
    let captured = NaiveDate::parse_from_str(day, DAY_FORMAT).ok()?;
    let elapsed = today.signed_duration_since(captured).num_days();
    // Measured from the end of the captured day, and clamped at zero so a
    // future-dated directory -- clock skew, or a hand-made one -- reads as the
    // youngest possible segment rather than an absurdly old one.
    Some(u32::try_from(elapsed.saturating_sub(1).max(0)).unwrap_or(u32::MAX))
}

/// Whole days since the processing attempt began, from the record.
///
/// `None` when the record is absent, carries no `attempted_at`, or carries one that
/// is not an RFC 3339 instant. All three fail closed.
pub fn processed_age_days(record: Option<&Value>, now: DateTime<Utc>) -> Option<u32> {
    let stamped = record?.get(ATTEMPTED_AT)?.as_str()?;
    let attempted = DateTime::parse_from_rfc3339(stamped).ok()?;
    let elapsed = now
        .signed_duration_since(attempted.with_timezone(&Utc))
        .num_days();
    Some(u32::try_from(elapsed.max(0)).unwrap_or(u32::MAX))
}

/// A segment's age by every anchor, from its day and its records.
///
/// ⛔ `records` must hold one entry per media file the segment carries, in the same
/// sense the release predicate uses: every file whose raw would be released. The
/// processed anchor is the age of the **most recent** attempt among them, and is
/// absent if *any* file cannot supply one.
///
/// Both rules follow from what the anchor has to mean. A segment is not finished
/// processing until its last file is, so the newest attempt governs — taking the
/// oldest would release a segment as soon as its first file finished. And one
/// unmeasurable file makes the segment's processing age unmeasurable, rather than
/// letting the measurable files answer for it.
pub fn segment_age(
    day: &str,
    records: &[Option<&Value>],
    today: NaiveDate,
    now: DateTime<Utc>,
) -> SegmentAge {
    let mut youngest: Option<u32> = None;
    let mut every_file_answered = true;
    for record in records {
        match processed_age_days(*record, now) {
            Some(age) => {
                youngest = Some(youngest.map_or(age, |current: u32| current.min(age)));
            }
            None => every_file_answered = false,
        }
    }
    SegmentAge {
        since_captured: captured_age_days(day, today),
        // ⛔ An empty list answers nothing. A segment with no media files has no
        // processing age, and must not inherit the vacuous truth of an empty fold.
        since_processed: if every_file_answered && !records.is_empty() {
            youngest
        } else {
            None
        },
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

    fn date(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
    }

    fn instant(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn record(stamp: &str) -> Value {
        serde_json::json!({ "attempted_at": stamp })
    }

    /// The end-of-day anchor, stated as the promise it keeps.
    #[test]
    fn a_captured_period_holds_for_at_least_its_length() {
        // Captured some time on the 4th; the engine does not know when.
        assert_eq!(captured_age_days("20260804", date("2026-08-04")), Some(0));
        assert_eq!(captured_age_days("20260804", date("2026-08-05")), Some(0));
        assert_eq!(captured_age_days("20260804", date("2026-08-06")), Some(1));
        // A seven-day rule first fires on the 12th, by which point between 7 and 8
        // days have really elapsed -- never fewer than 7.
        assert_eq!(captured_age_days("20260804", date("2026-08-11")), Some(6));
        assert_eq!(captured_age_days("20260804", date("2026-08-12")), Some(7));
    }

    /// ⛔ The naive reading would release a 23:59 capture a day early.
    #[test]
    fn the_start_of_day_reading_would_break_the_promise() {
        let captured = date("2026-08-04");
        let fires_on = date("2026-08-11");
        let naive = fires_on.signed_duration_since(captured).num_days();
        assert_eq!(naive, 7, "the start-of-day reading calls this seven days");
        assert_eq!(
            captured_age_days("20260804", fires_on),
            Some(6),
            "and a 23:59 capture has had six days and one minute"
        );
    }

    #[test]
    fn date_arithmetic_crosses_months_and_a_leap_year() {
        assert_eq!(captured_age_days("20260131", date("2026-02-02")), Some(1));
        // 2028 is a leap year: 29 February exists, so the span is one day longer.
        assert_eq!(captured_age_days("20280228", date("2028-03-01")), Some(1));
        assert_eq!(captured_age_days("20270228", date("2027-03-01")), Some(0));
    }

    #[test]
    fn an_unparseable_day_is_unmeasurable_not_young() {
        for day in [
            "",
            "2026-08-04",
            "20261301",
            "20260230",
            "nonsense",
            "2026080",
        ] {
            assert_eq!(captured_age_days(day, date("2026-08-05")), None, "{day:?}");
        }
    }

    /// A future day holds under every period rather than reading as ancient.
    #[test]
    fn a_future_day_reads_as_the_youngest_possible_segment() {
        assert_eq!(captured_age_days("20260901", date("2026-08-05")), Some(0));
    }

    #[test]
    fn the_processed_anchor_truncates_elapsed_time() {
        let now = instant("2026-08-12T00:00:00Z");
        assert_eq!(
            processed_age_days(Some(&record("2026-08-05T00:00:00Z")), now),
            Some(7)
        );
        // Six days and twenty-three hours is six days, not seven.
        assert_eq!(
            processed_age_days(Some(&record("2026-08-05T01:00:00Z")), now),
            Some(6)
        );
    }

    #[test]
    fn a_non_utc_offset_is_honoured_rather_than_ignored() {
        let now = instant("2026-08-12T00:00:00Z");
        // Same instant, written with an offset.
        assert_eq!(
            processed_age_days(Some(&record("2026-08-05T02:00:00+02:00")), now),
            Some(7)
        );
    }

    #[test]
    fn every_unreadable_processed_anchor_fails_closed() {
        let now = instant("2026-08-12T00:00:00Z");
        assert_eq!(processed_age_days(None, now), None, "no record");
        for value in [
            serde_json::json!({}),
            serde_json::json!({ "attempted_at": null }),
            serde_json::json!({ "attempted_at": 1_754_352_000 }),
            serde_json::json!({ "attempted_at": "" }),
            serde_json::json!({ "attempted_at": "2026-08-05" }),
            serde_json::json!({ "attempted_at": "yesterday" }),
        ] {
            assert_eq!(processed_age_days(Some(&value), now), None, "{value}");
        }
    }

    /// A restamped attempt makes content younger, never older.
    #[test]
    fn a_retry_can_only_hold_content_longer() {
        let now = instant("2026-08-12T00:00:00Z");
        let first = processed_age_days(Some(&record("2026-08-05T00:00:00Z")), now).unwrap();
        let retried = processed_age_days(Some(&record("2026-08-09T00:00:00Z")), now).unwrap();
        assert!(retried < first, "{retried} must be younger than {first}");
    }

    #[test]
    fn a_stamp_in_the_future_reads_as_just_processed() {
        let now = instant("2026-08-12T00:00:00Z");
        assert_eq!(
            processed_age_days(Some(&record("2026-09-01T00:00:00Z")), now),
            Some(0)
        );
    }

    /// ⛔ The newest attempt governs the segment.
    #[test]
    fn the_segment_is_as_young_as_its_most_recent_attempt() {
        let old = record("2026-08-05T00:00:00Z");
        let recent = record("2026-08-11T00:00:00Z");
        let age = segment_age(
            "20260701",
            &[Some(&old), Some(&recent)],
            date("2026-08-12"),
            instant("2026-08-12T00:00:00Z"),
        );
        assert_eq!(
            age.since_processed,
            Some(1),
            "the segment is one day processed, not seven"
        );
        assert_eq!(age.since_captured, Some(41));
    }

    /// ⛔ One unmeasurable file makes the segment unmeasurable.
    #[test]
    fn a_single_file_without_a_stamp_removes_the_processed_anchor() {
        let stamped = record("2026-08-05T00:00:00Z");
        let legacy = serde_json::json!({ "state": "analyzed" });
        let age = segment_age(
            "20260701",
            &[Some(&stamped), Some(&legacy)],
            date("2026-08-12"),
            instant("2026-08-12T00:00:00Z"),
        );
        assert_eq!(age.since_processed, None);
        assert_eq!(
            age.since_captured,
            Some(41),
            "and the captured anchor still answers, so a captured rule still works"
        );
    }

    /// ⛔ An empty fold must not answer for a segment with no media.
    #[test]
    fn a_segment_with_no_media_files_has_no_processed_age() {
        let age = segment_age(
            "20260701",
            &[],
            date("2026-08-12"),
            instant("2026-08-12T00:00:00Z"),
        );
        assert_eq!(
            age.since_processed, None,
            "min over an empty set is not zero days processed"
        );
    }

    /// The two anchors are independent: either can answer without the other.
    #[test]
    fn the_anchors_do_not_substitute_for_each_other() {
        let stamped = record("2026-08-11T00:00:00Z");
        let unmeasurable_day = segment_age(
            "nonsense",
            &[Some(&stamped)],
            date("2026-08-12"),
            instant("2026-08-12T00:00:00Z"),
        );
        assert_eq!(unmeasurable_day.since_captured, None);
        assert_eq!(unmeasurable_day.since_processed, Some(1));
    }
}
