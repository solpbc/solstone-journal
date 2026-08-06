// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The scheduled pass, over real journal trees.
//!
//! ⚠ These live in `tests/` rather than beside the code because
//! `tests/architecture.rs` forbids any `src/` module from naming a removal or rename
//! primitive, and a filesystem bed has to tear itself down. That guard is worth more
//! than the convenience of co-location: it is what keeps the removal surface one
//! named file.

#![allow(
    clippy::disallowed_methods,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "bed setup and teardown; the crate-wide bans exist to constrain the \
              production verbs, and they fired here, which is them working"
)]

use std::fs;
use std::path::PathBuf;

use solstone_core_processing_record::vocab;

use chrono::{DateTime, NaiveDate, Utc};
use solstone_core_retention::content::{ClosedHandlerSet, JournalMedia};
use solstone_core_retention::policy::{Anchor, Days, Eligibility, Policy, Rule};
use solstone_core_retention::sweep::{Plan, Skip, execute, plan};

struct Bed {
    root: PathBuf,
}

impl Bed {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "retention-sweep-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("a clock")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("a bed");
        Self { root }
    }

    /// A segment with proven-releasable audio.
    fn proven_segment(&self, day: &str, stream: Option<&str>, dir: &str, stamp: &str) -> u64 {
        let path = self.segment_path(day, stream, dir);
        fs::create_dir_all(&path).expect("a segment");
        let raw = b"the owner's recording";
        fs::write(path.join("audio.flac"), raw).expect("raw");
        let header = serde_json::json!({
            "segment": dir,
            "_solstone_processing": {
                "schema": vocab::SCHEMA,
                "state": vocab::STATE_ANALYZED,
                "reason_code": vocab::REASON_OK,
                "handler": vocab::HANDLER_TRANSCRIBE,
                "attempted_at": stamp,
                "input_size": raw.len(),
            }
        });
        fs::write(
            path.join("audio.jsonl"),
            format!("{header}\n{{\"start\": 0.0, \"text\": \"hello\"}}\n"),
        )
        .expect("sidecar");
        raw.len() as u64
    }

    fn segment_path(&self, day: &str, stream: Option<&str>, dir: &str) -> PathBuf {
        let day_dir = self.root.join("chronicle").join(day);
        match stream {
            Some(stream) => day_dir.join(stream).join(dir),
            None => day_dir.join(dir),
        }
    }

    fn plan(&self, policy: &Policy, today: &str, now: &str) -> Plan {
        plan(
            &self.root,
            policy,
            &ClosedHandlerSet,
            &JournalMedia,
            NaiveDate::parse_from_str(today, "%Y-%m-%d").expect("a date"),
            DateTime::parse_from_rfc3339(now)
                .expect("an instant")
                .with_timezone(&Utc),
        )
    }
}

fn teardown(bed: &Bed) {
    std::fs::remove_dir_all(&bed.root).expect("teardown");
}

fn armed(rule: Rule) -> Policy {
    Policy {
        default_rule: rule,
        enabled: true,
        ..Policy::default()
    }
}

fn captured_after(days: u32) -> Policy {
    armed(Rule {
        anchor: Anchor::Captured,
        period: Some(Days(days)),
        priority: 0,
    })
}

/// 🔴 The default policy is off, so a plan over a real journal is empty.
#[test]
fn the_default_policy_proposes_nothing_at_all() {
    let bed = Bed::new("default-off");
    bed.proven_segment(
        "20250101",
        Some("field.audio"),
        "070000_17",
        "2025-01-01T00:00:00Z",
    );

    let built = bed.plan(&Policy::default(), "2026-08-05", "2026-08-05T00:00:00Z");
    assert!(built.candidates.is_empty(), "{built:?}");
    assert_eq!(built.examined(), 1, "and it still examined the segment");
    assert_eq!(built.bytes(), 0);
    assert!(matches!(
        built.skipped[0].reason,
        Skip::Policy(Eligibility::KeptForever)
    ));
    teardown(&bed);
}

/// The whole composition: an old, fully-proven segment is a candidate, and
/// executing the plan releases exactly its raw while the derived output stays.
#[test]
fn an_old_proven_segment_is_released_and_its_derived_output_survives() {
    let bed = Bed::new("release");
    let size = bed.proven_segment(
        "20260701",
        Some("field.audio"),
        "070000_17",
        "2026-07-01T00:00:00Z",
    );

    let built = bed.plan(&captured_after(7), "2026-08-05", "2026-08-05T00:00:00Z");
    assert_eq!(built.candidates.len(), 1, "{built:?}");
    assert_eq!(built.files(), 1);
    assert_eq!(built.bytes(), size);

    let segment = bed.segment_path("20260701", Some("field.audio"), "070000_17");
    assert!(
        segment.join("audio.flac").exists(),
        "planning removed nothing"
    );

    let (outcome, tally) = execute(&bed.root, &built);
    assert!(outcome.halted.is_none(), "{outcome:?}");
    assert_eq!(outcome.targets.len(), 1);
    assert!(outcome.targets[0].not_removed.is_empty(), "{outcome:?}");
    assert_eq!(tally.on_record, 1);
    assert!(!segment.join("audio.flac").exists(), "the raw is released");
    assert!(
        segment.join("audio.jsonl").exists(),
        "the derived output is what the owner keeps"
    );
    teardown(&bed);
}

/// 🔴 The default-stream layout, which the four-component path builder broke.
#[test]
fn a_default_stream_segment_is_released_rather_than_silently_skipped() {
    let bed = Bed::new("default-stream");
    bed.proven_segment("20260701", None, "070000_17", "2026-07-01T00:00:00Z");

    let built = bed.plan(&captured_after(7), "2026-08-05", "2026-08-05T00:00:00Z");
    assert_eq!(built.candidates.len(), 1, "{built:?}");
    assert_eq!(
        built.candidates[0].target.stream,
        solstone_core_retention::layout::default_stream()
    );
    assert_eq!(
        built.candidates[0].proven[0].rel(),
        "chronicle/20260701/070000_17/audio.flac",
        "the default stream contributes no path component"
    );

    let (outcome, _) = execute(&bed.root, &built);
    assert!(
        outcome.targets[0].not_removed.is_empty(),
        "a four-component path would have reported entry-missing here: {outcome:?}"
    );
    assert!(
        !bed.segment_path("20260701", None, "070000_17")
            .join("audio.flac")
            .exists()
    );
    teardown(&bed);
}

/// ⛔ The directory name, not the key scanned out of it.
#[test]
fn a_suffixed_directory_name_is_addressed_by_its_name() {
    let bed = Bed::new("suffixed");
    bed.proven_segment(
        "20260701",
        Some("field.audio"),
        "093000_300_summary",
        "2026-07-01T00:00:00Z",
    );

    let built = bed.plan(&captured_after(7), "2026-08-05", "2026-08-05T00:00:00Z");
    assert_eq!(built.candidates.len(), 1, "{built:?}");
    assert_eq!(
        built.candidates[0].target.dir, "093000_300_summary",
        "the key `093000_300` addresses a directory that does not exist"
    );

    let (outcome, _) = execute(&bed.root, &built);
    assert!(outcome.targets[0].not_removed.is_empty(), "{outcome:?}");
    teardown(&bed);
}

/// A young segment is skipped with the age it was measured at.
#[test]
fn a_young_segment_is_skipped_and_says_how_young() {
    let bed = Bed::new("young");
    bed.proven_segment(
        "20260804",
        Some("field.audio"),
        "070000_17",
        "2026-08-04T00:00:00Z",
    );

    let built = bed.plan(&captured_after(7), "2026-08-05", "2026-08-05T00:00:00Z");
    assert!(built.candidates.is_empty());
    match &built.skipped[0].reason {
        Skip::Policy(Eligibility::TooYoung {
            anchor,
            age_days,
            period,
        }) => {
            assert_eq!(*anchor, Anchor::Captured);
            assert_eq!(*age_days, 0);
            assert_eq!(*period, Days(7));
        }
        other => panic!("{other:?}"),
    }
    teardown(&bed);
}

/// ⛔ The two gates are independent: age cannot overrule missing proof.
#[test]
fn an_ancient_segment_without_proof_is_still_held() {
    let bed = Bed::new("no-proof");
    let path = bed.segment_path("20200101", Some("field.audio"), "070000_17");
    fs::create_dir_all(&path).expect("a segment");
    fs::write(path.join("audio.flac"), b"the owner's recording").expect("raw");
    // No sidecar at all: nothing consumed these bytes.

    let built = bed.plan(&captured_after(7), "2026-08-05", "2026-08-05T00:00:00Z");
    assert!(
        built.candidates.is_empty(),
        "six years old and unproven is still held: {built:?}"
    );
    assert!(matches!(built.skipped[0].reason, Skip::Held(_)));
    assert!(path.join("audio.flac").exists());
    teardown(&bed);
}

/// ⛔ And a still image holds its whole segment, however old.
#[test]
fn an_image_holds_its_segment_because_no_handler_claims_it() {
    let bed = Bed::new("image");
    bed.proven_segment(
        "20200101",
        Some("field.audio"),
        "070000_17",
        "2020-01-01T00:00:00Z",
    );
    let path = bed.segment_path("20200101", Some("field.audio"), "070000_17");
    fs::write(path.join("photo.png"), b"an image").expect("image");

    let built = bed.plan(&captured_after(7), "2026-08-05", "2026-08-05T00:00:00Z");
    assert!(built.candidates.is_empty(), "{built:?}");
    match &built.skipped[0].reason {
        Skip::Held(blockers) => {
            assert!(
                blockers.iter().any(|b| b.name() == "photo.png"),
                "{blockers:?}"
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(
        path.join("audio.flac").exists(),
        "one unprovable file holds the proven one too"
    );
    teardown(&bed);
}

/// A processed-anchored rule works, and expresses what the reference cannot.
#[test]
fn a_week_after_processing_is_expressible_and_measured_from_the_record() {
    let bed = Bed::new("processed");
    // Captured long ago, processed yesterday.
    bed.proven_segment(
        "20260101",
        Some("field.audio"),
        "070000_17",
        "2026-08-04T00:00:00Z",
    );
    let policy = armed(Rule {
        anchor: Anchor::Processed,
        period: Some(Days(7)),
        priority: 0,
    });

    let built = bed.plan(&policy, "2026-08-05", "2026-08-05T00:00:00Z");
    assert!(
        built.candidates.is_empty(),
        "processed yesterday is not a week ago, however old the capture: {built:?}"
    );

    let later = bed.plan(&policy, "2026-08-12", "2026-08-12T00:00:00Z");
    assert_eq!(later.candidates.len(), 1, "{later:?}");
    teardown(&bed);
}

/// ⛔ A missing processed anchor holds; it does not fall back to captured.
#[test]
fn a_legacy_segment_holds_under_a_processed_rule_and_releases_under_a_captured_one() {
    let bed = Bed::new("legacy");
    let path = bed.segment_path("20200101", Some("field.audio"), "070000_17");
    fs::create_dir_all(&path).expect("a segment");
    fs::write(path.join("audio.flac"), b"the owner's recording").expect("raw");
    // Read old: analysis rows, no record -- so no `attempted_at` anywhere.
    fs::write(
        path.join("audio.jsonl"),
        b"{\"segment\": \"070000_17\"}\n{\"start\": 0.0, \"text\": \"hello\"}\n",
    )
    .expect("sidecar");

    let processed = armed(Rule {
        anchor: Anchor::Processed,
        period: Some(Days(7)),
        priority: 0,
    });
    let held = bed.plan(&processed, "2026-08-05", "2026-08-05T00:00:00Z");
    assert!(held.candidates.is_empty(), "{held:?}");
    assert!(matches!(
        held.skipped[0].reason,
        Skip::Policy(Eligibility::AnchorMissing {
            anchor: Anchor::Processed
        })
    ));

    // The captured anchor still answers, and the legacy evidence still proves.
    let released = bed.plan(&captured_after(7), "2026-08-05", "2026-08-05T00:00:00Z");
    assert_eq!(released.candidates.len(), 1, "{released:?}");
    let (_, tally) = execute(&bed.root, &released);
    assert_eq!(tally.on_legacy_rows, 1, "and the receipt says how it knew");
    teardown(&bed);
}

/// A per-stream rule reaches the sweep, and shadows the default.
#[test]
fn a_per_stream_rule_governs_only_its_own_stream() {
    let bed = Bed::new("per-stream");
    bed.proven_segment(
        "20260701",
        Some("field.audio"),
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    bed.proven_segment(
        "20260701",
        Some("field.screen"),
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    let policy = Policy {
        default_rule: Rule::keep(),
        per_stream: vec![(
            "field.audio".to_owned(),
            Rule {
                anchor: Anchor::Captured,
                period: Some(Days(7)),
                priority: 0,
            },
        )],
        enabled: true,
        ..Policy::default()
    };

    let built = bed.plan(&policy, "2026-08-05", "2026-08-05T00:00:00Z");
    assert_eq!(built.candidates.len(), 1, "{built:?}");
    assert_eq!(built.candidates[0].target.stream, "field.audio");
    assert!(
        built
            .skipped
            .iter()
            .any(|s| s.target.stream == "field.screen"
                && matches!(s.reason, Skip::Policy(Eligibility::KeptForever))),
        "{built:?}"
    );
    teardown(&bed);
}

/// ⛔ The floor overrides a rule that would release sooner.
#[test]
fn the_minimum_age_holds_content_a_rule_would_have_released() {
    let bed = Bed::new("floor");
    bed.proven_segment(
        "20260801",
        Some("field.audio"),
        "070000_17",
        "2026-08-01T00:00:00Z",
    );
    let policy = Policy {
        default_rule: Rule {
            anchor: Anchor::Captured,
            period: Some(Days(1)),
            priority: 0,
        },
        minimum_age: Days(30),
        enabled: true,
        ..Policy::default()
    };

    let built = bed.plan(&policy, "2026-08-05", "2026-08-05T00:00:00Z");
    assert!(built.candidates.is_empty(), "{built:?}");
    match &built.skipped[0].reason {
        Skip::Policy(Eligibility::TooYoung { period, .. }) => {
            assert_eq!(
                *period,
                Days(30),
                "the verdict names the floor that blocked"
            );
        }
        other => panic!("{other:?}"),
    }
    teardown(&bed);
}

/// A segment mid-removal is invisible to the sweep.
#[test]
fn a_staged_directory_is_not_examined() {
    let bed = Bed::new("staged");
    bed.proven_segment(
        "20260701",
        Some("field.audio"),
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    let stream = bed.root.join("chronicle/20260701/field.audio");
    fs::rename(
        stream.join("070000_17"),
        stream.join(solstone_core_retention::staging::staged_name("070000_17")),
    )
    .expect("stage it");

    let built = bed.plan(&captured_after(7), "2026-08-05", "2026-08-05T00:00:00Z");
    assert_eq!(
        built.examined(),
        0,
        "a segment being removed is not a retention candidate: {built:?}"
    );
    teardown(&bed);
}

/// A journal with no chronicle plans nothing and does not fail.
#[test]
fn an_empty_journal_plans_nothing() {
    let bed = Bed::new("empty");
    let built = bed.plan(&captured_after(7), "2026-08-05", "2026-08-05T00:00:00Z");
    assert_eq!(built.examined(), 0);
    assert!(built.unreadable_days.is_empty());
    assert_eq!(built.bytes(), 0);
    teardown(&bed);
}

/// Executing an empty plan is legal and touches nothing.
#[test]
fn executing_an_empty_plan_removes_nothing() {
    let bed = Bed::new("empty-exec");
    bed.proven_segment(
        "20260701",
        Some("field.audio"),
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    let (outcome, tally) = execute(&bed.root, &Plan::default());
    assert!(outcome.targets.is_empty(), "{outcome:?}");
    assert_eq!(tally.on_record, 0);
    assert!(
        bed.segment_path("20260701", Some("field.audio"), "070000_17")
            .join("audio.flac")
            .exists()
    );
    teardown(&bed);
}

/// Several days and streams, in a deterministic order.
#[test]
fn a_plan_is_ordered_and_covers_every_day() {
    let bed = Bed::new("many");
    for day in ["20260703", "20260701", "20260702"] {
        bed.proven_segment(
            day,
            Some("field.audio"),
            "070000_17",
            "2026-07-01T00:00:00Z",
        );
    }
    let built = bed.plan(&captured_after(7), "2026-08-05", "2026-08-05T00:00:00Z");
    let days: Vec<&str> = built
        .candidates
        .iter()
        .map(|candidate| candidate.target.day.as_str())
        .collect();
    assert_eq!(days, vec!["20260701", "20260702", "20260703"]);
    assert_eq!(built.files(), 3);
    teardown(&bed);
}

/// A segment with no media is reported, not dropped.
#[test]
fn a_segment_holding_only_derived_output_is_reported_as_having_no_media() {
    let bed = Bed::new("no-media");
    let path = bed.segment_path("20260701", Some("field.audio"), "070000_17");
    fs::create_dir_all(&path).expect("a segment");
    fs::write(path.join("audio.jsonl"), b"{\"segment\": \"x\"}\n").expect("derived");

    let built = bed.plan(&captured_after(7), "2026-08-05", "2026-08-05T00:00:00Z");
    assert!(built.candidates.is_empty());
    assert_eq!(built.examined(), 1, "reported rather than dropped");
    assert!(matches!(built.skipped[0].reason, Skip::NoMedia));
    teardown(&bed);
}
