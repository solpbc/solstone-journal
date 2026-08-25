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

#[cfg(all(unix, not(target_os = "macos")))]
use std::ffi::OsStr;
use std::fs;
use std::path::PathBuf;

use solstone_core_processing_record::vocab;

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{Map, Value, json};
use solstone_core_retention::content::{ClosedHandlerSet, JournalMedia};
use solstone_core_retention::marks::{Proposal, RemovalClass, load, reconcile};
use solstone_core_retention::policy::{
    Anchor, Days, Eligibility, Policy, Rule, policy_from_retention,
};
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

    fn empty_terminal_file(
        &self,
        day: &str,
        stream: Option<&str>,
        dir: &str,
        name: &str,
        record: Value,
    ) -> PathBuf {
        let path = self.segment_path(day, stream, dir);
        fs::create_dir_all(&path).expect("a segment");
        let raw = b"raw";
        let file = path.join(name);
        fs::write(&file, raw).expect("raw");
        let mut header = json!({"segment": dir});
        header
            .as_object_mut()
            .expect("header")
            .insert("_solstone_processing".to_owned(), record);
        let sidecar = name.rsplit_once('.').expect("extension").0;
        fs::write(path.join(format!("{sidecar}.jsonl")), format!("{header}\n")).expect("sidecar");
        file
    }

    fn empty_terminal_segment(
        &self,
        day: &str,
        stream: Option<&str>,
        dir: &str,
        stamp: &str,
    ) -> PathBuf {
        self.empty_terminal_file(
            day,
            stream,
            dir,
            "audio.flac",
            json!({
                "schema": vocab::SCHEMA,
                "state": vocab::STATE_EMPTY,
                "reason_code": vocab::REASON_NO_DECODABLE_AUDIO,
                "handler": vocab::HANDLER_TRANSCRIBE,
                "attempted_at": stamp,
                "input_size": 3,
            }),
        )
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

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn unrepresentable_segments_are_named_in_the_plan() {
    use std::os::unix::ffi::OsStrExt;

    let bed = Bed::new("unrepresentable");
    bed.proven_segment(
        "20250101",
        Some("field.audio"),
        "070000_17",
        "2025-01-01T00:00:00Z",
    );
    fs::create_dir_all(
        bed.root
            .join("chronicle/20250101")
            .join(OsStr::from_bytes(b"080000_17\xff")),
    )
    .expect("non-utf8 segment");

    let built = bed.plan(&captured_after(1), "2026-08-05", "2026-08-05T00:00:00Z");
    assert_eq!(built.unrepresentable_segments.len(), 1);
    assert!(
        built.unrepresentable_segments[0]
            .file_name()
            .is_some_and(|name| name.as_bytes() == b"080000_17\xff")
    );
    assert!(built.examined() >= 2);
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

fn retention_object(value: Value) -> Map<String, Value> {
    value.as_object().cloned().expect("retention object")
}

fn keep_journal_policy() -> Policy {
    policy_from_retention(&retention_object(json!({"raw_media": "keep"})))
}

fn empty_record(stamp: &str, extra: Value) -> Value {
    let mut record = json!({
        "schema": vocab::SCHEMA,
        "state": vocab::STATE_EMPTY,
        "reason_code": vocab::REASON_NO_DECODABLE_AUDIO,
        "handler": vocab::HANDLER_TRANSCRIBE,
        "attempted_at": stamp,
        "input_size": 3,
    });
    if let Some(fields) = extra.as_object() {
        record
            .as_object_mut()
            .expect("record")
            .extend(fields.clone());
    }
    record
}

#[test]
fn empty_audio_on_a_keep_journal_is_a_candidate() {
    let bed = Bed::new("empty-keep");
    bed.empty_terminal_segment(
        "20260701",
        Some("field.audio"),
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    bed.empty_terminal_file(
        "20260701",
        Some("field.audio"),
        "080000_17",
        "clip.wav",
        empty_record("2026-07-01T00:00:00Z", json!({})),
    );
    bed.empty_terminal_file(
        "20260701",
        Some("field.audio"),
        "080000_17",
        "other.flac",
        empty_record("2026-07-01T00:00:00Z", json!({})),
    );

    let built = bed.plan(&keep_journal_policy(), "2026-08-05", "2026-08-05T00:00:00Z");
    assert_eq!(built.candidates.len(), 2, "{built:?}");
    teardown(&bed);
}

#[test]
fn empty_audio_class_exclusions() {
    const STAMP: &str = "2026-07-01T00:00:00Z";
    struct Case {
        name: &'static str,
        seed: fn(&Bed),
        policy: Policy,
        expect_candidate: bool,
    }
    let cases = [
        Case {
            name: "analyzed",
            seed: |bed| {
                bed.proven_segment("20260701", Some("field.audio"), "070000_17", STAMP);
            },
            policy: keep_journal_policy(),
            expect_candidate: false,
        },
        Case {
            name: "class-keep",
            seed: |bed| {
                bed.empty_terminal_segment("20260701", Some("field.audio"), "070000_17", STAMP);
            },
            policy: policy_from_retention(&retention_object(
                json!({"raw_media": "keep", "empty_audio": "keep"}),
            )),
            expect_candidate: false,
        },
        Case {
            name: "days-too-young",
            seed: |bed| {
                bed.empty_terminal_segment("20260804", Some("field.audio"), "070000_17", STAMP);
            },
            policy: policy_from_retention(&retention_object(json!({
                "raw_media": "keep",
                "empty_audio": "days",
                "empty_audio_days": 7,
            }))),
            expect_candidate: false,
        },
        Case {
            name: "days-old-enough",
            seed: |bed| {
                bed.empty_terminal_segment("20260701", Some("field.audio"), "070000_17", STAMP);
            },
            policy: policy_from_retention(&retention_object(json!({
                "raw_media": "keep",
                "empty_audio": "days",
                "empty_audio_days": 7,
            }))),
            expect_candidate: true,
        },
        Case {
            name: "failed",
            seed: |bed| {
                bed.empty_terminal_file(
                    "20260701",
                    Some("field.audio"),
                    "070000_17",
                    "audio.flac",
                    json!({
                        "schema": vocab::SCHEMA,
                        "state": vocab::STATE_FAILED,
                        "reason_code": vocab::REASON_CORRUPT_INPUT,
                        "handler": vocab::HANDLER_TRANSCRIBE,
                        "attempted_at": STAMP,
                        "input_size": 3,
                    }),
                );
            },
            policy: keep_journal_policy(),
            expect_candidate: false,
        },
        Case {
            name: "wrong-reason",
            seed: |bed| {
                bed.empty_terminal_file(
                    "20260701",
                    Some("field.audio"),
                    "070000_17",
                    "audio.flac",
                    empty_record(STAMP, json!({"reason_code": "ok"})),
                );
            },
            policy: keep_journal_policy(),
            expect_candidate: false,
        },
        Case {
            name: "backfill",
            seed: |bed| {
                bed.empty_terminal_file(
                    "20260701",
                    Some("field.audio"),
                    "070000_17",
                    "audio.flac",
                    empty_record(STAMP, json!({"source": "backfill"})),
                );
            },
            policy: keep_journal_policy(),
            expect_candidate: false,
        },
    ];

    for case in cases {
        let bed = Bed::new(&format!("excl-{}", case.name));
        (case.seed)(&bed);
        let built = bed.plan(&case.policy, "2026-08-05", "2026-08-05T00:00:00Z");
        assert_eq!(
            !built.candidates.is_empty(),
            case.expect_candidate,
            "{}: {built:?}",
            case.name
        );
        teardown(&bed);
    }
}

#[test]
fn empty_audio_candidate_ignores_raw_media_minimum_days() {
    let bed = Bed::new("empty-floor");
    bed.empty_terminal_segment(
        "20260803",
        Some("field.audio"),
        "070000_17",
        "2026-08-03T00:00:00Z",
    );
    let policy = policy_from_retention(&retention_object(json!({
        "raw_media": "keep",
        "raw_media_minimum_days": 30,
    })));
    let built = bed.plan(&policy, "2026-08-05", "2026-08-05T00:00:00Z");
    assert_eq!(built.candidates.len(), 1, "{built:?}");
    teardown(&bed);
}

#[test]
fn mark_pass_records_empty_audio_and_removes_nothing() {
    let bed = Bed::new("empty-mark");
    let raw = bed.empty_terminal_segment(
        "20260701",
        Some("field.audio"),
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    let policy = keep_journal_policy();
    let built = bed.plan(&policy, "2026-08-05", "2026-08-05T00:00:00Z");
    assert_eq!(built.candidates.len(), 1, "{built:?}");
    let proposals: Vec<_> = built
        .candidates
        .iter()
        .map(|candidate| {
            (
                candidate.target.clone(),
                Proposal {
                    bytes: candidate.bytes(),
                    reason: "empty-audio class".to_owned(),
                    names: candidate
                        .proven
                        .iter()
                        .map(|item| item.name().to_owned())
                        .collect(),
                },
            )
        })
        .collect();
    let register = reconcile(
        &bed.root,
        RemovalClass::PolicyRawRelease,
        &proposals,
        DateTime::parse_from_rfc3339("2026-08-05T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
    )
    .expect("reconcile");
    assert_eq!(register.marks.len(), 1);
    assert!(raw.exists());
    let loaded = load(&bed.root).expect("load");
    assert_eq!(loaded.marks.len(), 1);
    assert!(
        loaded
            .marks
            .values()
            .all(|mark| mark.class == RemovalClass::PolicyRawRelease)
    );
    teardown(&bed);
}

fn write_sibling(bed: &Bed, name: &str, record: Option<Value>, analysis_row: bool) -> PathBuf {
    let path = bed.segment_path("20260701", Some("field.audio"), "070000_17");
    fs::create_dir_all(&path).expect("segment");
    let raw = b"sibling";
    let file = path.join(name);
    fs::write(&file, raw).expect("sibling raw");
    if let Some(record) = record {
        let mut header = json!({"segment": "070000_17"});
        header
            .as_object_mut()
            .expect("header")
            .insert("_solstone_processing".to_owned(), record);
        let stem = name.rsplit_once('.').expect("extension").0;
        let body = if analysis_row {
            format!("{header}\n{{\"start\": 0.0, \"text\": \"hello\"}}\n")
        } else {
            format!("{header}\n")
        };
        fs::write(path.join(format!("{stem}.jsonl")), body).expect("sidecar");
    }
    file
}

fn proven_names(plan: &Plan) -> Vec<String> {
    plan.candidates
        .iter()
        .flat_map(|candidate| candidate.proven.iter().map(|item| item.name().to_owned()))
        .collect()
}

fn analyzed_record(stamp: &str, extra: Value) -> Value {
    let mut record = json!({
        "schema": vocab::SCHEMA,
        "state": vocab::STATE_ANALYZED,
        "reason_code": vocab::REASON_OK,
        "handler": vocab::HANDLER_TRANSCRIBE,
        "attempted_at": stamp,
        "input_size": 7,
    });
    if let Some(fields) = extra.as_object() {
        record
            .as_object_mut()
            .expect("record")
            .extend(fields.clone());
    }
    record
}

#[test]
fn mixed_sibling_releases_only_the_empty_file() {
    let bed = Bed::new("mixed-sibling");
    bed.empty_terminal_segment(
        "20260701",
        Some("field.audio"),
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    let extra = write_sibling(
        &bed,
        "extra.flac",
        Some(analyzed_record("2026-07-01T00:00:00Z", json!({}))),
        true,
    );
    let built = bed.plan(&keep_journal_policy(), "2026-08-05", "2026-08-05T00:00:00Z");
    assert_eq!(built.candidates.len(), 1, "{built:?}");
    assert_eq!(proven_names(&built), ["audio.flac"]);
    match built.candidates[0].eligibility {
        Eligibility::Eligible { anchor, period, .. } => {
            assert_eq!(anchor, Anchor::Processed);
            assert_eq!(period, Days(0));
        }
        other => panic!("expected processed-immediate eligibility, got {other:?}"),
    }
    assert!(extra.exists());
    teardown(&bed);
}

#[test]
fn empty_audio_releases_independently_of_ordinary_siblings() {
    const STAMP: &str = "2026-07-01T00:00:00Z";
    struct Case {
        name: &'static str,
        seed_sibling: fn(&Bed),
        policy: Policy,
    }
    let processed = policy_from_retention(&retention_object(json!({"raw_media": "processed"})));
    let cases = [
        Case {
            name: "incomplete",
            seed_sibling: |bed| {
                write_sibling(bed, "extra.flac", None, false);
            },
            policy: processed.clone(),
        },
        Case {
            name: "failed",
            seed_sibling: |bed| {
                write_sibling(
                    bed,
                    "extra.flac",
                    Some(json!({
                        "schema": vocab::SCHEMA,
                        "state": vocab::STATE_FAILED,
                        "reason_code": vocab::REASON_CORRUPT_INPUT,
                        "handler": vocab::HANDLER_TRANSCRIBE,
                        "attempted_at": STAMP,
                        "input_size": 7,
                    })),
                    false,
                );
            },
            policy: processed.clone(),
        },
        Case {
            name: "unprovable-image",
            seed_sibling: |bed| {
                write_sibling(bed, "photo.png", None, false);
            },
            policy: processed,
        },
        Case {
            name: "kept-forever",
            seed_sibling: |bed| {
                write_sibling(
                    bed,
                    "extra.flac",
                    Some(analyzed_record(STAMP, json!({}))),
                    true,
                );
            },
            policy: keep_journal_policy(),
        },
        Case {
            name: "backfill-lookalike",
            seed_sibling: |bed| {
                write_sibling(
                    bed,
                    "extra.flac",
                    Some(empty_record(STAMP, json!({"source": "backfill"}))),
                    false,
                );
            },
            policy: keep_journal_policy(),
        },
        Case {
            name: "wrong-reason-lookalike",
            seed_sibling: |bed| {
                write_sibling(
                    bed,
                    "extra.flac",
                    Some(empty_record(STAMP, json!({"reason_code": "ok"}))),
                    false,
                );
            },
            policy: keep_journal_policy(),
        },
    ];

    for case in cases {
        let bed = Bed::new(&format!("indep-{}", case.name));
        bed.empty_terminal_segment("20260701", Some("field.audio"), "070000_17", STAMP);
        (case.seed_sibling)(&bed);
        let built = bed.plan(&case.policy, "2026-08-05", "2026-08-05T00:00:00Z");
        assert_eq!(
            proven_names(&built),
            ["audio.flac"],
            "{}: {built:?}",
            case.name
        );
        teardown(&bed);
    }
}

#[test]
fn empty_audio_age_ignores_an_unstamped_ordinary_sibling() {
    let bed = Bed::new("empty-age-unstamped");
    bed.empty_terminal_segment(
        "20260701",
        Some("field.audio"),
        "070000_17",
        "2026-07-01T00:00:00Z",
    );
    let mut record = analyzed_record("2026-07-01T00:00:00Z", json!({}));
    record
        .as_object_mut()
        .expect("record")
        .remove("attempted_at");
    write_sibling(&bed, "extra.flac", Some(record), true);
    let built = bed.plan(&keep_journal_policy(), "2026-08-05", "2026-08-05T00:00:00Z");
    assert_eq!(proven_names(&built), ["audio.flac"], "{built:?}");
    teardown(&bed);
}
