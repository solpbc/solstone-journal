// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[path = "support/corpus.rs"]
mod corpus;

use std::collections::BTreeMap;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use solstone_core_system::catchup::{
    SegmentRepairOutcome, record_daily_catchup_progress, record_segment_repair_attempt,
    record_segment_repair_outcome,
};
use solstone_core_system_health::{
    CAP, DETERMINISTIC_FAILURE_REASON_CODES, DataStateMap, FilesystemHealthLogSource, FoldRead,
    HealthLogSource, MIN_SPAN_MS, SEGMENT_FLOOR_TALENTS, SEGMENT_NO_PROCESSING_MODALITIES,
    SEGMENT_NONGATING_TALENTS, SEGMENT_SUPERSEDED_TALENTS, SegmentBlockerDimension,
    SegmentIdentity, SegmentInput, TerminalEvent, ThoughtVerdict, blocked_segment_keys,
    classify_segment_completion, is_floor_talent_capped, lookup_segment_progress,
    read_completed_since, read_completed_units, read_daily_deterministic_failures,
    read_segment_progress, read_terminal_states, segment_fully_sensed, segment_fully_thought,
    segment_requires_processing,
};

static NEXT_ROOT: AtomicUsize = AtomicUsize::new(0);

fn python(root: &std::path::Path, day: &str) -> Value {
    let repository = corpus::repository_root();
    let executable = repository.join(".venv/bin/python3");
    let executable = executable
        .is_file()
        .then_some(executable)
        .unwrap_or_else(|| "python3".into());
    let output = Command::new(executable)
        .arg(repository.join(
            "core/crates/solstone-core-system-health/tests/support/python_pipeline_health_oracle.py",
        ))
        .env("SOLSTONE_JOURNAL", root)
        .env("SOLSTONE_REPO_ROOT", &repository)
        .env("ORACLE_DAY", day)
        .env("ORACLE_SINCE_MS", corpus::SINCE_MS.to_string())
        .output()
        .expect("start Python oracle");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn python_writer(root: &std::path::Path, day: &str) -> Value {
    let repository = corpus::repository_root();
    let executable = repository.join(".venv/bin/python3");
    let executable = executable
        .is_file()
        .then_some(executable)
        .unwrap_or_else(|| "python3".into());
    let output = Command::new(executable)
        .arg(repository.join(
            "core/crates/solstone-core-system-health/tests/support/python_catchup_writer_oracle.py",
        ))
        .env("SOLSTONE_JOURNAL", root)
        .env("SOLSTONE_REPO_ROOT", &repository)
        .env("ORACLE_DAY", day)
        .output()
        .expect("start Python writer oracle");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn copy_tree(source: &std::path::Path, destination: &std::path::Path) {
    std::fs::create_dir_all(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn catchup_writers_match_the_python_oracle_on_a_disposable_fixture_copy() {
    let repository = corpus::repository_root();
    let source = repository.join("tests/fixtures/journal");
    let python_root = tempfile::tempdir().unwrap();
    let native_root = tempfile::tempdir().unwrap();
    copy_tree(&source, python_root.path());
    copy_tree(&source, native_root.path());
    let day = "20250101";
    let expected = python_writer(python_root.path(), day);
    record_daily_catchup_progress(native_root.path(), day, 1, 2);
    record_segment_repair_attempt(native_root.path(), day, 1.0);
    record_segment_repair_outcome(
        native_root.path(),
        day,
        SegmentRepairOutcome {
            success: false,
            timed_out: true,
            timeout_seconds: Some(3.0),
            ended_at: 4.0,
            cleared: Some(1),
            remaining: Some(2),
        },
    );
    let actual: Value = serde_json::from_slice(
        &std::fs::read(native_root.path().join("health/catchup-state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(actual, expected);
}

fn terminal_states(
    value: &BTreeMap<
        solstone_core_system_health::TerminalUnit,
        solstone_core_system_health::TerminalState,
    >,
) -> Value {
    Value::Array(
        value
            .iter()
            .map(|(unit, state)| {
                json!({
                    "unit": {
                        "mode": unit.mode,
                        "name": unit.name,
                        "facet": unit.facet,
                        "stream": unit.stream,
                        "segment": unit.segment,
                        "activity": unit.activity,
                    },
                    "state": {
                        "latest_event": match state.latest_event {
                            TerminalEvent::Complete => "complete",
                            TerminalEvent::Fail => "fail",
                        },
                        "latest_ts": state.latest_ts,
                        "last_real_complete_ts": state.last_real_complete_ts,
                        "trailing_fail_count": state.trailing_fail_count,
                        "deterministic_fail_count": state.deterministic_fail_count,
                        "last_fail_ts": state.last_fail_ts,
                        "use_id": state.use_id,
                        "state": state.state,
                        "reason_code": state.reason_code,
                        "provider": state.provider,
                        "model": state.model,
                        "oldest_trailing_fail_ts": state.oldest_trailing_fail_ts,
                    },
                })
            })
            .collect(),
    )
}

fn completed_units(
    value: &FoldRead<std::collections::BTreeSet<solstone_core_system_health::CompletedUnit>>,
) -> Value {
    Value::Array(
        value
            .value
            .iter()
            .map(|unit| json!({"mode": unit.mode, "name": unit.name, "facet": unit.facet}))
            .collect(),
    )
}

fn completed_since(value: &solstone_core_system_health::CompletionsSince) -> Value {
    json!({
        "segments": value.segments.iter().map(|item| json!({
            "stream": item.stream, "segment": item.segment, "ts": item.ts,
        })).collect::<Vec<_>>(),
        "activities": value.activities.iter().map(|item| json!({
            "facet": item.facet, "activity": item.activity, "ts": item.ts,
        })).collect::<Vec<_>>(),
    })
}

fn daily_failures(
    value: &BTreeMap<
        solstone_core_system_health::DailyUnit,
        solstone_core_system_health::DeterministicFailure,
    >,
) -> Value {
    Value::Array(
        value
            .iter()
            .map(|(unit, failure)| {
                json!({
                    "name": unit.name,
                    "facet": unit.facet,
                    "count": failure.count,
                    "reason_code": failure.reason_code,
                })
            })
            .collect(),
    )
}

fn segment_progress(
    value: &BTreeMap<
        solstone_core_system_health::SegmentIdentity,
        solstone_core_system_health::SegmentProgress,
    >,
) -> Value {
    Value::Array(
        value
            .iter()
            .map(|(identity, progress)| {
                json!({
                    "stream": identity.stream,
                    "segment": identity.segment,
                    "sensed": progress.sensed,
                    "density": progress.density,
                    "change_class": progress.change_class,
                    "dispatched": progress.dispatched,
                    "completed": progress.completed,
                    "unconfigured": progress.unconfigured,
                    "capped_by_skip": progress.capped_by_skip,
                })
            })
            .collect(),
    )
}

fn pure_segments() -> Vec<SegmentInput> {
    [
        ("progress", "screen", "analyzed"),
        ("not-sensed", "screen", "pending"),
        ("browser-only", "browser", "analyzed"),
    ]
    .into_iter()
    .map(|(key, modality, state)| SegmentInput {
        key: key.to_owned(),
        stream: "default".to_owned(),
        data_state: DataStateMap(BTreeMap::from([(modality.to_owned(), state.to_owned())])),
    })
    .collect()
}

fn thought_verdict(value: ThoughtVerdict) -> Value {
    match value {
        ThoughtVerdict::Complete => json!([true, Value::Null]),
        ThoughtVerdict::NoSenseComplete => json!([false, "no_sense_complete"]),
        ThoughtVerdict::Floor(name) => json!([false, format!("floor:{name}")]),
        ThoughtVerdict::Dispatched(name) => json!([false, format!("dispatched:{name}")]),
    }
}

fn pure_completion(
    segments: &[SegmentInput],
    progress: &BTreeMap<SegmentIdentity, solstone_core_system_health::SegmentProgress>,
) -> Value {
    let completion = classify_segment_completion(segments, progress);
    let thought = segment_fully_thought(lookup_segment_progress(progress, "default", "progress"));
    json!({
        "fully_sensed": segments.iter().map(|segment| segment_fully_sensed(&segment.data_state)).collect::<Vec<_>>(),
        "requires_processing": segments.iter().map(segment_requires_processing).collect::<Vec<_>>(),
        "thought": thought_verdict(thought),
        "lookup_found": lookup_segment_progress(progress, "default", "progress").is_some(),
        "classification": {
            "blockers": completion.blockers.iter().map(|blocker| json!({
                "segment": blocker.segment,
                "dimension": match blocker.dimension {
                    SegmentBlockerDimension::NotSensed => "not_sensed",
                    SegmentBlockerDimension::NotThought => "not_thought",
                },
                "detail": blocker.detail,
            })).collect::<Vec<_>>(),
            "not_sensed": completion.not_sensed,
            "not_thought": completion.not_thought,
            "total": completion.total,
            "capped": completion.capped,
            "exhausted": completion.exhausted,
        },
        "blocked": blocked_segment_keys(segments, progress).iter().map(|identity| json!({
            "stream": identity.stream, "segment": identity.segment,
        })).collect::<Vec<_>>(),
    })
}

#[test]
fn synthesized_corpus_matches_python_folds_and_vocabulary() {
    let root = std::env::temp_dir().join(format!(
        "system-health-oracle-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    corpus::write_corpus(&root);
    let source = FilesystemHealthLogSource::new(&root);
    let oracle = python(&root, corpus::CURRENT_DAY);

    let terminals = read_terminal_states(&source, corpus::CURRENT_DAY, false).unwrap();
    assert_eq!(terminals.malformed_line_count, 1);
    assert_eq!(oracle["terminal_states"], terminal_states(&terminals.value));

    let completed = read_completed_units(&source, corpus::CURRENT_DAY).unwrap();
    assert_eq!(oracle["completed_units"], completed_units(&completed));

    let since = read_completed_since(&source, corpus::CURRENT_DAY, corpus::SINCE_MS).unwrap();
    assert_eq!(since.malformed_line_count, 1);
    assert_eq!(oracle["completed_since"], completed_since(&since.value));

    let daily = read_daily_deterministic_failures(&source, corpus::CURRENT_DAY).unwrap();
    assert_eq!(
        oracle["daily_deterministic_failures"],
        daily_failures(&daily.value)
    );

    let progress = read_segment_progress(&source, corpus::CURRENT_DAY).unwrap();
    assert_eq!(progress.malformed_line_count, 1);
    assert_eq!(
        oracle["segment_progress"],
        segment_progress(&progress.value)
    );
    assert_eq!(
        oracle["pure_completion"],
        pure_completion(&pure_segments(), &progress.value)
    );

    assert_eq!(
        oracle["floor_caps"],
        json!({
            "cap_true": is_floor_talent_capped(
                &source, corpus::CURRENT_DAY, Some("default"), "cap-true", "documents"
            ).unwrap().value,
            "cap_short": is_floor_talent_capped(
                &source, corpus::CURRENT_DAY, Some("default"), "cap-short", "documents"
            ).unwrap().value,
        })
    );

    assert_eq!(oracle["floor"], json!(SEGMENT_FLOOR_TALENTS));
    assert_eq!(oracle["nongating"], json!(SEGMENT_NONGATING_TALENTS));
    let mut no_processing = SEGMENT_NO_PROCESSING_MODALITIES.to_vec();
    no_processing.sort_unstable();
    assert_eq!(oracle["no_processing"], json!(no_processing));
    assert_eq!(
        oracle["superseded"],
        json!({"entities":"entities:detection"})
    );
    assert_eq!(oracle["cap"], json!(CAP));
    assert_eq!(oracle["min_span_ms"], MIN_SPAN_MS);
    let mut deterministic = DETERMINISTIC_FAILURE_REASON_CODES.to_vec();
    deterministic.sort_unstable();
    assert_eq!(oracle["deterministic"], json!(deterministic));
    assert_eq!(
        SEGMENT_SUPERSEDED_TALENTS,
        &[("entities", "entities:detection")]
    );
}

/// The repository fixture has no health JSONL; this is intentionally only an empty-result smoke leg.
#[test]
fn real_fixture_health_logs_are_empty_on_both_sides() {
    let root = corpus::repository_root().join("tests/fixtures/journal");
    let source = FilesystemHealthLogSource::new(&root);
    assert!(source.health_log_paths("20250101").unwrap().is_empty());
    assert!(
        read_terminal_states(&source, "20250101", false)
            .unwrap()
            .value
            .is_empty()
    );
    assert_eq!(python(&root, "20250101")["terminal_states"], json!([]));
}
