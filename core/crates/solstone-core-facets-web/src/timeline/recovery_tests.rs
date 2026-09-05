// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use chrono::{NaiveDate, TimeZone, Utc};
use serde_json::{Value, json};
use solstone_core_generate::{GenerateRequest, GeneratedResponse};
use solstone_core_maintenance::{
    MaintenanceServices, RollupPicker, RollupPickerError, TimelineServices, registry,
    run_cli_with_timeline, timezone::HostTimezoneSource,
};
use solstone_core_system_health::{TimelineDivergenceDiagnosis, diagnose_timeline_divergence};
use solstone_core_timeline::{
    AttemptOutcome, AttemptStateV1, CURRENT_SCHEMA_VERSION, CurationRecordV1, MasterTimelineV1,
    PublishedArtifactV1, SegmentBindingV1, SegmentSummaryV1, SegmentTimelineV1, TimelineKind,
    TimelineRecordV1, day_timeline_path, master_subject_key, publish_segment_timeline,
    timeline_record_path,
};
use tower::ServiceExt;

use super::routes;
use crate::clock::Clock;

const DAYS: [&str; 5] = ["20260501", "20260502", "20260503", "20260504", "20260505"];
const SEGMENT_COUNT: usize = 82;
const MISSING_COUNT: usize = 20;
const LEGACY_COUNT: usize = 62;
const TOP: usize = 100;

struct UtcHost;

impl HostTimezoneSource for UtcHost {
    fn usable_iana_key(&self) -> Option<String> {
        Some("UTC".to_owned())
    }
}

struct NeverPicker;

impl RollupPicker for NeverPicker {
    fn pick(&self, _: &GenerateRequest) -> Result<GeneratedResponse, RollupPickerError> {
        panic!("the recovery fixture uses --top {TOP}; no curation generation is expected")
    }
}

#[derive(Clone)]
struct FixtureSegment {
    binding: SegmentBindingV1,
    missing: bool,
}

#[tokio::test]
async fn recovery_fixture_converges_legacy_segment_population_to_verified_owner_truth() {
    let journal = tempfile::tempdir().expect("temporary recovery journal");
    let segments = build_recovery_fixture(journal.path());
    assert_eq!(segments.len(), SEGMENT_COUNT);
    assert_eq!(
        segments.iter().filter(|segment| segment.missing).count(),
        MISSING_COUNT
    );
    assert_eq!(
        segments.iter().filter(|segment| !segment.missing).count(),
        LEGACY_COUNT
    );
    let corrupt_log = journal
        .path()
        .join("health/think/runs/fixture_segments.jsonl");
    let corrupt_log_bytes = fs::read(&corrupt_log).expect("corrupted run log exists");
    let stale_master = fs::read(journal.path().join("timeline.json")).expect("stale master exists");

    let mut named_missing = 0;
    let mut named_legacy = 0;
    for day in DAYS {
        let result = run_timeline(
            journal.path(),
            &[
                "run",
                "timeline:rollup-day",
                "--day",
                day,
                "--commit",
                "--top",
                "100",
            ],
        );
        assert_eq!(result.exit_code, 1, "{day}: {result:?}");
        named_missing += result.stderr.matches("missing (").count();
        named_legacy += result.stderr.matches("wrong_shape (").count();
        assert!(
            !day_timeline_path(journal.path(), day).exists(),
            "failed day scan must not publish a partial day artifact"
        );
    }
    assert_eq!(named_missing, MISSING_COUNT);
    assert_eq!(named_legacy, LEGACY_COUNT);
    assert_eq!(
        fs::read(journal.path().join("timeline.json")).expect("stale master remains"),
        stale_master,
        "failed segment scans must not overwrite the last master artifact"
    );

    repair_segment_population(journal.path(), &segments);
    for day in DAYS {
        let result = run_timeline(
            journal.path(),
            &[
                "run",
                "timeline:rollup-day",
                "--day",
                day,
                "--commit",
                "--top",
                "100",
            ],
        );
        assert_eq!(result.exit_code, 0, "{day}: {result:?}");
        assert!(day_timeline_path(journal.path(), day).is_file());
    }
    let master = run_timeline(
        journal.path(),
        &["run", "timeline:rollup-master", "--commit", "--top", "100"],
    );
    assert_eq!(master.exit_code, 0, "{master:?}");
    let recovered_master: MasterTimelineV1 = serde_json::from_slice(
        &fs::read(journal.path().join("timeline.json")).expect("recovered master reads"),
    )
    .expect("recovered master parses");
    assert_ne!(recovered_master.source_digest, "obsolete-master-artifact");
    assert_eq!(recovered_master.months["202605"].days.len(), DAYS.len());

    let overview = api_payload(journal.path(), "/app/timeline/api/overview").await;
    assert_eq!(overview["status"], "current");
    assert_eq!(overview["data_through"], DAYS.last().copied().unwrap());
    for day in DAYS {
        let payload = api_payload(journal.path(), &format!("/app/timeline/api/day/{day}")).await;
        assert_eq!(payload["status"], "current", "{day}");
    }
    assert_eq!(
        diagnose_timeline_divergence(
            journal.path(),
            Utc.with_ymd_and_hms(2026, 5, 6, 12, 0, 0).unwrap(),
        )
        .expect("Doctor diagnosis"),
        TimelineDivergenceDiagnosis::Clean
    );
    assert_eq!(
        fs::read(&corrupt_log).expect("corrupted run log remains isolated"),
        corrupt_log_bytes,
        "timeline recovery must neither read nor rewrite unrelated run logs"
    );
}

#[tokio::test]
async fn day_api_currentness_follows_changed_and_incomplete_native_inputs() {
    let journal = tempfile::tempdir().unwrap();
    let segments = build_recovery_fixture(journal.path());
    repair_segment_population(journal.path(), &segments);
    let day = DAYS[0];
    let publish = [
        "run",
        "timeline:rollup-day",
        "--day",
        day,
        "--commit",
        "--top",
        "100",
    ];
    let preview = ["run", "timeline:rollup-day", "--day", day, "--top", "100"];
    assert_eq!(run_timeline(journal.path(), &publish).exit_code, 0);
    let route = format!("/app/timeline/api/day/{day}");
    let before = api_payload(journal.path(), &route).await;
    assert_eq!(before["status"], "current");
    let artifact_path = day_timeline_path(journal.path(), day);
    let artifact = fs::read(&artifact_path).unwrap();

    let binding = SegmentBindingV1 {
        day: day.to_owned(),
        stream: "_default".to_owned(),
        segment: "090000_300".to_owned(),
    };
    let source = journal
        .path()
        .join("chronicle")
        .join(day)
        .join("090000_300/talents/activity.md");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, "New activity after the day publication.\n").unwrap();
    assert_eq!(run_timeline(journal.path(), &preview).exit_code, 1);
    let incomplete = api_payload(journal.path(), &route).await;
    assert_eq!(incomplete["status"], "stale");
    assert_eq!(incomplete["artifact_outcome"], "source_unavailable");
    assert_eq!(incomplete["day_top"], before["day_top"]);
    assert_eq!(fs::read(&artifact_path).unwrap(), artifact);

    repair_segment_population(
        journal.path(),
        &[FixtureSegment {
            binding,
            missing: true,
        }],
    );
    let native = run_timeline(journal.path(), &preview);
    assert_eq!(native.exit_code, 0);
    assert!(native.stdout.contains("[stale]"));
    let stale = api_payload(journal.path(), &route).await;
    assert_eq!(stale["status"], "stale");
    assert_eq!(stale["artifact_outcome"], "digest_mismatch");
    assert_eq!(fs::read(&artifact_path).unwrap(), artifact);

    assert_eq!(run_timeline(journal.path(), &publish).exit_code, 0);
    assert_eq!(
        api_payload(journal.path(), &route).await["status"],
        "current"
    );
    let source_bytes = fs::read(&source).unwrap();
    fs::write(&source, "Changed activity after segment publication.\n").unwrap();
    assert_eq!(run_timeline(journal.path(), &preview).exit_code, 1);
    assert_eq!(api_payload(journal.path(), &route).await["status"], "stale");
    fs::write(&source, source_bytes).unwrap();
    assert_eq!(
        api_payload(journal.path(), &route).await["status"],
        "current"
    );
}

fn build_recovery_fixture(journal: &Path) -> Vec<FixtureSegment> {
    let state = TimelineRecordV1 {
        schema_version: CURRENT_SCHEMA_VERSION,
        subject: master_subject_key().to_owned(),
        published: Some(PublishedArtifactV1 {
            input_digest: "obsolete-master-state".to_owned(),
            artifact_sha256: "obsolete-master-bytes".to_owned(),
            published_at_ms: 1,
        }),
        attempts: Vec::new(),
    };
    fs::write(
        timeline_record_path(journal, master_subject_key()).unwrap(),
        serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
    fs::write(
        journal.join("timeline.json"),
        serde_json::to_vec(&stale_master()).expect("stale master serializes"),
    )
    .expect("stale master writes");
    fs::create_dir_all(journal.join("health/think/runs")).expect("run-log parent creates");
    fs::write(
        journal.join("health/think/runs/fixture_segments.jsonl"),
        b"{\"complete\":true}\n{\"truncated\":\n",
    )
    .expect("corrupt run log writes");

    let counts = [17, 17, 16, 16, 16];
    let mut segments = Vec::with_capacity(SEGMENT_COUNT);
    let mut ordinal = 0;
    for (day, count) in DAYS.into_iter().zip(counts) {
        for minute in 0..count {
            let segment = format!("08{minute:02}00_300");
            let binding = SegmentBindingV1 {
                day: day.to_owned(),
                stream: "_default".to_owned(),
                segment: segment.clone(),
            };
            let path = journal.join("chronicle").join(day).join(&segment);
            fs::create_dir_all(path.join("talents")).expect("segment directory creates");
            fs::write(
                path.join("talents/activity.md"),
                format!("Recovered activity {ordinal}.\n"),
            )
            .expect("activity source writes");
            let missing = ordinal < MISSING_COUNT;
            if !missing {
                fs::write(
                    path.join("timeline.json"),
                    json!({"title": "legacy", "description": "pre-v1 timeline"}).to_string(),
                )
                .expect("legacy timeline writes");
            }
            segments.push(FixtureSegment { binding, missing });
            ordinal += 1;
        }
    }
    segments
}

fn stale_master() -> MasterTimelineV1 {
    MasterTimelineV1 {
        schema_version: CURRENT_SCHEMA_VERSION,
        kind: TimelineKind::Master,
        source_digest: "obsolete-master-artifact".to_owned(),
        generated_at_ms: 1,
        top_n: TOP,
        months: BTreeMap::new(),
        year_top: Vec::new(),
        year_curation: CurationRecordV1 {
            input_digest: "obsolete-master-artifact".to_owned(),
            candidate_count: 0,
            picks: Vec::new(),
            rationale: "stale fixture".to_owned(),
            error: None,
            provenance: None,
        },
    }
}

fn repair_segment_population(journal: &Path, segments: &[FixtureSegment]) {
    for (ordinal, segment) in segments.iter().enumerate() {
        let snapshot = solstone_core_timeline::resolve_activity_source(journal, &segment.binding)
            .expect("activity resolution")
            .expect("activity source");
        let source = solstone_core_timeline::SegmentSourceV1::GeneratedActivity {
            schema_version: solstone_core_timeline::SEGMENT_SOURCE_SCHEMA_VERSION,
            relative_path: snapshot.relative_path,
            sha256: snapshot.sha256,
        };
        let digest = solstone_core_timeline::segment_input_digest(&segment.binding, &source)
            .expect("segment source digest");
        let timeline = SegmentTimelineV1 {
            schema_version: CURRENT_SCHEMA_VERSION,
            kind: TimelineKind::Segment,
            binding: segment.binding.clone(),
            input_digest: digest.clone(),
            source: Some(source),
            generated_at_ms: 1_778_000_000_000 + ordinal as i64,
            summary: SegmentSummaryV1 {
                title: format!("Recovered {ordinal}"),
                description: "Recovered V1 segment timeline.".to_owned(),
                origin: format!("{}/{}", segment.binding.day, segment.binding.segment),
                continuation_of: None,
            },
            provenance: None,
        };
        publish_segment_timeline(
            journal,
            &timeline,
            AttemptStateV1 {
                attempt_id: format!("fixture-repair-{ordinal}"),
                input_digest: digest,
                started_at_ms: timeline.generated_at_ms,
                finished_at_ms: None,
                outcome: AttemptOutcome::Running,
                detail: String::new(),
            },
        )
        .expect("segment repair publishes verified V1 artifact");
    }
}

fn run_timeline(journal: &Path, arguments: &[&str]) -> solstone_core_maintenance::CliRun {
    let picker = NeverPicker;
    let host = UtcHost;
    let timeline = TimelineServices {
        now: Utc.with_ymd_and_hms(2026, 5, 6, 12, 0, 0).unwrap(),
        host_timezone: &host,
        picker: &picker,
    };
    let services = MaintenanceServices::new(registry::routines());
    run_cli_with_timeline(
        &arguments
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect::<Vec<_>>(),
        journal,
        &services,
        &timeline,
    )
}

async fn api_payload(journal: &Path, path: &str) -> Value {
    let clock = Clock::new(|| {
        NaiveDate::from_ymd_opt(2026, 5, 6)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
    });
    let response = routes(journal.to_path_buf(), clock)
        .oneshot(Request::get(path).body(Body::empty()).expect("request"))
        .await
        .expect("timeline route response");
    serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("timeline response body"),
    )
    .expect("timeline response JSON")
}
