// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};

use solstone_core_system_health::{
    DataStateMap, FilesystemHealthLogSource, SegmentIdentity, SegmentInput, SegmentProgress,
    TerminalEvent, TerminalUnit, ThoughtVerdict, blocked_segment_keys, classify_segment_completion,
    is_floor_talent_capped, lookup_segment_progress, read_completed_since, read_completed_units,
    read_daily_deterministic_failures, read_segment_progress, read_terminal_states,
    segment_fully_sensed, segment_fully_thought, segment_requires_processing,
};

static NEXT_ROOT: AtomicUsize = AtomicUsize::new(0);

fn root() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "system-health-test-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn write(root: &std::path::Path, day: &str, file: &str, lines: &[&str]) {
    let dir = root.join("chronicle").join(day).join("health");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(file), lines.join("\n") + "\n").unwrap();
}

#[test]
fn terminal_fold_uses_global_sorted_file_encounter_order_and_survives_malformed() {
    let root = root();
    write(
        &root,
        "20990201",
        "001.jsonl",
        &[
            "{bad",
            r#"{"event":"talent.complete","ts":4,"mode":"daily","name":"alpha"}"#,
        ],
    );
    write(
        &root,
        "20990201",
        "999.jsonl",
        &[
            r#"{"event":"talent.fail","ts":4,"mode":"daily","name":"alpha","reason_code":"no_output"}"#,
        ],
    );
    let source = FilesystemHealthLogSource::new(&root);
    let states = read_terminal_states(&source, "20990201", false).unwrap();
    assert_eq!(states.malformed_line_count, 1);
    let state = states
        .value
        .get(&TerminalUnit {
            mode: "daily".into(),
            name: "alpha".into(),
            facet: None,
            stream: None,
            segment: None,
            activity: None,
        })
        .unwrap();
    assert_eq!(state.latest_event, TerminalEvent::Fail);
    assert_eq!(
        read_completed_units(&source, "20990201")
            .unwrap()
            .value
            .len(),
        0
    );
}

#[test]
fn reader_behaviors_cover_cadence_daily_failures_progress_and_floor_cap() {
    let root = root();
    let failures = (0..5).map(|index| format!(r#"{{"event":"talent.fail","ts":{},"mode":"segment","stream":"default","segment":"s","name":"documents"}}"#, index * 1_800_000)).collect::<Vec<_>>();
    let lines = failures.iter().map(String::as_str).chain([
        r#"{"event":"talent.complete","ts":11,"mode":"daily","name":"daily"}"#,
        r#"{"event":"talent.fail","ts":12,"mode":"daily","name":"bad","reason_code":"no_output"}"#,
        r#"{"event":"sense.complete","ts":1,"mode":"segment","stream":"default","segment":"s","density":"active"}"#,
        r#"{"event":"talent.skip","ts":9000000,"mode":"segment","stream":"default","segment":"s","name":"documents","reason":"capped"}"#,
    ]).collect::<Vec<_>>();
    write(&root, "20990202", "001.jsonl", &lines);
    write(
        &root,
        "20990201",
        "001.jsonl",
        &[
            r#"{"event":"talent.complete","ts":10,"mode":"segment","stream":"default","segment":"prior","name":"entities"}"#,
        ],
    );
    let source = FilesystemHealthLogSource::new(&root);
    assert!(
        is_floor_talent_capped(&source, "20990202", Some("default"), "s", "documents")
            .unwrap()
            .value
    );
    assert_eq!(
        read_daily_deterministic_failures(&source, "20990202")
            .unwrap()
            .value
            .len(),
        1
    );
    assert_eq!(
        read_completed_since(&source, "20990202", 9)
            .unwrap()
            .value
            .segments[0]
            .segment,
        "prior"
    );
    let progress = read_segment_progress(&source, "20990202").unwrap().value;
    assert_eq!(
        progress[&SegmentIdentity {
            stream: Some("default".into()),
            segment: "s".into()
        }]
            .density
            .as_deref(),
        Some("active")
    );
    assert!(
        progress[&SegmentIdentity {
            stream: Some("default".into()),
            segment: "s".into(),
        }]
            .capped_by_skip
            .contains("documents")
    );
    let completion = classify_segment_completion(
        &[SegmentInput {
            key: "s".into(),
            stream: "default".into(),
            data_state: DataStateMap(BTreeMap::from([("screen".into(), "analyzed".into())])),
        }],
        &progress,
    );
    assert_eq!(completion.capped, 1);
}

#[test]
fn floor_cap_requires_a_long_unbroken_trailing_failure_streak() {
    let root = root();
    write(
        &root,
        "20990202",
        "001.jsonl",
        &[
            r#"{"event":"talent.fail","ts":1,"mode":"segment","stream":"default","segment":"short","name":"documents"}"#,
            r#"{"event":"talent.fail","ts":2,"mode":"segment","stream":"default","segment":"short","name":"documents"}"#,
            r#"{"event":"talent.fail","ts":3,"mode":"segment","stream":"default","segment":"short","name":"documents"}"#,
            r#"{"event":"talent.fail","ts":4,"mode":"segment","stream":"default","segment":"short","name":"documents"}"#,
            r#"{"event":"talent.fail","ts":5,"mode":"segment","stream":"default","segment":"short","name":"documents"}"#,
            r#"{"event":"talent.fail","ts":0,"mode":"segment","stream":"default","segment":"reset","name":"documents"}"#,
            r#"{"event":"talent.fail","ts":1800000,"mode":"segment","stream":"default","segment":"reset","name":"documents"}"#,
            r#"{"event":"talent.fail","ts":3600000,"mode":"segment","stream":"default","segment":"reset","name":"documents"}"#,
            r#"{"event":"talent.fail","ts":5400000,"mode":"segment","stream":"default","segment":"reset","name":"documents"}"#,
            r#"{"event":"talent.fail","ts":7200000,"mode":"segment","stream":"default","segment":"reset","name":"documents"}"#,
            r#"{"event":"talent.complete","ts":7200001,"mode":"segment","stream":"default","segment":"reset","name":"documents"}"#,
            r#"{"event":"talent.fail","ts":9000000,"mode":"segment","stream":"default","segment":"reset","name":"documents"}"#,
        ],
    );
    let source = FilesystemHealthLogSource::new(&root);
    assert!(
        !is_floor_talent_capped(&source, "20990202", Some("default"), "short", "documents")
            .unwrap()
            .value
    );
    assert!(
        !is_floor_talent_capped(&source, "20990202", Some("default"), "reset", "documents")
            .unwrap()
            .value
    );
}

#[test]
fn pure_completion_functions_preserve_precedence_and_stream_fallback() {
    let pending = SegmentInput {
        key: "pending".into(),
        stream: "one".into(),
        data_state: DataStateMap(BTreeMap::from([("screen".into(), "pending".into())])),
    };
    let complete = SegmentInput {
        key: "complete".into(),
        stream: "one".into(),
        data_state: DataStateMap(BTreeMap::from([("screen".into(), "analyzed".into())])),
    };
    assert!(!segment_fully_sensed(&pending.data_state));
    assert!(segment_requires_processing(&pending));
    let mut progress = BTreeMap::new();
    progress.insert(
        SegmentIdentity {
            stream: None,
            segment: "complete".into(),
        },
        SegmentProgress {
            sensed: true,
            density: Some("idle".into()),
            ..SegmentProgress::default()
        },
    );
    assert_eq!(
        segment_fully_thought(lookup_segment_progress(&progress, "one", "complete")),
        ThoughtVerdict::Complete
    );
    let completion = classify_segment_completion(&[pending.clone(), complete.clone()], &progress);
    assert_eq!(
        (
            completion.not_sensed,
            completion.not_thought,
            completion.total
        ),
        (1, 0, 2)
    );
    assert_eq!(
        blocked_segment_keys(&[pending], &progress),
        BTreeSet::from([SegmentIdentity {
            stream: Some("one".into()),
            segment: "pending".into()
        }])
    );
}

fn analyzed(key: &str) -> SegmentInput {
    SegmentInput {
        key: key.to_owned(),
        stream: "default".into(),
        data_state: DataStateMap(BTreeMap::from([("screen".into(), "analyzed".into())])),
    }
}

fn progress_for(root: &std::path::Path, day: &str) -> BTreeMap<SegmentIdentity, SegmentProgress> {
    read_segment_progress(&FilesystemHealthLogSource::new(root), day)
        .unwrap()
        .value
}

fn thought_for(
    progress: &BTreeMap<SegmentIdentity, SegmentProgress>,
    segment: &str,
) -> ThoughtVerdict {
    segment_fully_thought(lookup_segment_progress(progress, "default", segment))
}

#[test]
fn latest_unmatched_dispatch_is_not_completed_even_after_an_older_complete() {
    let root = root();
    write(
        &root,
        "20990202",
        "001.jsonl",
        &[
            r#"{"event":"sense.complete","ts":1,"mode":"segment","stream":"default","segment":"s","density":"active"}"#,
            r#"{"event":"talent.dispatch","ts":2,"mode":"segment","stream":"default","segment":"s","name":"documents","use_id":"old"}"#,
            r#"{"event":"talent.complete","ts":3,"mode":"segment","stream":"default","segment":"s","name":"documents","use_id":"old"}"#,
            r#"{"event":"talent.dispatch","ts":4,"mode":"segment","stream":"default","segment":"s","name":"documents","use_id":"new"}"#,
        ],
    );
    let progress = progress_for(&root, "20990202");
    let row = &progress[&SegmentIdentity {
        stream: Some("default".into()),
        segment: "s".into(),
    }];
    assert!(row.dispatched.contains("documents"));
    assert!(!row.completed.contains("documents"));
    assert_eq!(
        thought_for(&progress, "s"),
        ThoughtVerdict::Floor("documents".into())
    );
}

#[test]
fn matching_new_use_id_restores_completed_and_a_third_id_does_not() {
    let root = root();
    write(
        &root,
        "20990202",
        "001.jsonl",
        &[
            r#"{"event":"sense.complete","ts":1,"mode":"segment","stream":"default","segment":"s","density":"active"}"#,
            r#"{"event":"talent.dispatch","ts":2,"mode":"segment","stream":"default","segment":"s","name":"documents","use_id":"old"}"#,
            r#"{"event":"talent.complete","ts":3,"mode":"segment","stream":"default","segment":"s","name":"documents","use_id":"old"}"#,
            r#"{"event":"talent.dispatch","ts":4,"mode":"segment","stream":"default","segment":"s","name":"documents","use_id":"new"}"#,
            r#"{"event":"talent.complete","ts":5,"mode":"segment","stream":"default","segment":"s","name":"documents","use_id":"other"}"#,
        ],
    );
    let progress = progress_for(&root, "20990202");
    assert!(
        !progress[&SegmentIdentity {
            stream: Some("default".into()),
            segment: "s".into(),
        }]
            .completed
            .contains("documents")
    );
    assert_eq!(
        thought_for(&progress, "s"),
        ThoughtVerdict::Floor("documents".into())
    );

    write(
        &root,
        "20990203",
        "001.jsonl",
        &[
            r#"{"event":"sense.complete","ts":1,"mode":"segment","stream":"default","segment":"s","density":"active"}"#,
            r#"{"event":"talent.dispatch","ts":2,"mode":"segment","stream":"default","segment":"s","name":"documents","use_id":"old"}"#,
            r#"{"event":"talent.complete","ts":3,"mode":"segment","stream":"default","segment":"s","name":"documents","use_id":"old"}"#,
            r#"{"event":"talent.dispatch","ts":4,"mode":"segment","stream":"default","segment":"s","name":"documents","use_id":"new"}"#,
            r#"{"event":"talent.complete","ts":5,"mode":"segment","stream":"default","segment":"s","name":"documents","use_id":"new"}"#,
        ],
    );
    let restored = progress_for(&root, "20990203");
    assert!(
        restored[&SegmentIdentity {
            stream: Some("default".into()),
            segment: "s".into(),
        }]
            .completed
            .contains("documents")
    );
    assert_eq!(thought_for(&restored, "s"), ThoughtVerdict::Complete);
}

#[test]
fn missing_use_id_matches_by_order_and_complete_without_dispatch_still_counts() {
    let root = root();
    write(
        &root,
        "20990202",
        "001.jsonl",
        &[
            r#"{"event":"sense.complete","ts":1,"mode":"segment","stream":"default","segment":"legacy","density":"active"}"#,
            r#"{"event":"talent.dispatch","ts":2,"mode":"segment","stream":"default","segment":"legacy","name":"documents"}"#,
            r#"{"event":"talent.complete","ts":3,"mode":"segment","stream":"default","segment":"legacy","name":"documents"}"#,
            r#"{"event":"talent.complete","ts":4,"mode":"segment","stream":"default","segment":"orphan","name":"documents"}"#,
        ],
    );
    let progress = progress_for(&root, "20990202");
    assert!(
        progress[&SegmentIdentity {
            stream: Some("default".into()),
            segment: "legacy".into(),
        }]
            .completed
            .contains("documents")
    );
    assert_eq!(thought_for(&progress, "legacy"), ThoughtVerdict::Complete);
    assert!(
        progress[&SegmentIdentity {
            stream: Some("default".into()),
            segment: "orphan".into(),
        }]
            .completed
            .contains("documents")
    );
}

#[test]
fn fold_verdicts_keep_nongating_superseded_unconfigured_capped_idle_and_redundant() {
    let root = root();
    write(
        &root,
        "20990202",
        "001.jsonl",
        &[
            r#"{"event":"sense.complete","ts":1,"mode":"segment","stream":"default","segment":"gating","density":"active"}"#,
            r#"{"event":"talent.dispatch","ts":2,"mode":"segment","stream":"default","segment":"gating","name":"documents","use_id":"d1"}"#,
            r#"{"event":"sense.complete","ts":3,"mode":"segment","stream":"default","segment":"nongating","density":"active"}"#,
            r#"{"event":"talent.complete","ts":4,"mode":"segment","stream":"default","segment":"nongating","name":"documents"}"#,
            r#"{"event":"talent.dispatch","ts":5,"mode":"segment","stream":"default","segment":"nongating","name":"entities:detection","use_id":"e1"}"#,
            r#"{"event":"sense.complete","ts":6,"mode":"segment","stream":"default","segment":"superseded","density":"active"}"#,
            r#"{"event":"talent.complete","ts":7,"mode":"segment","stream":"default","segment":"superseded","name":"documents"}"#,
            r#"{"event":"talent.dispatch","ts":8,"mode":"segment","stream":"default","segment":"superseded","name":"entities"}"#,
            r#"{"event":"talent.complete","ts":9,"mode":"segment","stream":"default","segment":"superseded","name":"entities:detection"}"#,
            r#"{"event":"sense.complete","ts":10,"mode":"segment","stream":"default","segment":"unconfigured","density":"active"}"#,
            r#"{"event":"talent.skip","ts":11,"mode":"segment","stream":"default","segment":"unconfigured","name":"documents","reason":"no_config"}"#,
            r#"{"event":"sense.complete","ts":12,"mode":"segment","stream":"default","segment":"capped","density":"active"}"#,
            r#"{"event":"talent.skip","ts":13,"mode":"segment","stream":"default","segment":"capped","name":"documents","reason":"capped"}"#,
            r#"{"event":"sense.complete","ts":14,"mode":"segment","stream":"default","segment":"idle","density":"idle"}"#,
            r#"{"event":"talent.dispatch","ts":15,"mode":"segment","stream":"default","segment":"idle","name":"documents","use_id":"idle-1"}"#,
            r#"{"event":"sense.complete","ts":16,"mode":"segment","stream":"default","segment":"redundant","density":"active"}"#,
            r#"{"event":"sense.change_detect","ts":17,"mode":"segment","stream":"default","segment":"redundant","change_class":"redundant"}"#,
            r#"{"event":"talent.dispatch","ts":18,"mode":"segment","stream":"default","segment":"redundant","name":"documents","use_id":"red-1"}"#,
        ],
    );
    let progress = progress_for(&root, "20990202");
    assert_eq!(
        thought_for(&progress, "gating"),
        ThoughtVerdict::Floor("documents".into())
    );
    assert_eq!(
        thought_for(&progress, "nongating"),
        ThoughtVerdict::Complete
    );
    assert_eq!(
        thought_for(&progress, "superseded"),
        ThoughtVerdict::Complete
    );
    assert_eq!(
        thought_for(&progress, "unconfigured"),
        ThoughtVerdict::Complete
    );
    assert_eq!(thought_for(&progress, "capped"), ThoughtVerdict::Complete);
    assert_eq!(thought_for(&progress, "idle"), ThoughtVerdict::Complete);
    assert_eq!(
        thought_for(&progress, "redundant"),
        ThoughtVerdict::Complete
    );

    let gating = analyzed("gating");
    assert!(segment_requires_processing(&gating));
    assert!(segment_fully_sensed(&gating.data_state));
    let classification = classify_segment_completion(&[gating], &progress);
    assert_eq!(classification.not_thought, 1);
}

#[test]
fn capped_by_skip_anchors_to_latest_dispatch_like_completed() {
    let root = root();
    write(
        &root,
        "20990202",
        "001.jsonl",
        &[
            r#"{"event":"sense.complete","ts":1,"mode":"segment","stream":"default","segment":"stale","density":"active"}"#,
            r#"{"event":"talent.skip","ts":2,"mode":"segment","stream":"default","segment":"stale","name":"documents","reason":"capped","use_id":"old"}"#,
            r#"{"event":"talent.dispatch","ts":3,"mode":"segment","stream":"default","segment":"stale","name":"documents","use_id":"new"}"#,
            r#"{"event":"talent.complete","ts":4,"mode":"segment","stream":"default","segment":"stale","name":"documents","use_id":"other"}"#,
            r#"{"event":"sense.complete","ts":5,"mode":"segment","stream":"default","segment":"order-cap","density":"active"}"#,
            r#"{"event":"talent.dispatch","ts":6,"mode":"segment","stream":"default","segment":"order-cap","name":"documents","use_id":"new"}"#,
            r#"{"event":"talent.skip","ts":7,"mode":"segment","stream":"default","segment":"order-cap","name":"documents","reason":"capped"}"#,
            r#"{"event":"sense.complete","ts":8,"mode":"segment","stream":"default","segment":"order-dispatch","density":"active"}"#,
            r#"{"event":"talent.dispatch","ts":9,"mode":"segment","stream":"default","segment":"order-dispatch","name":"documents"}"#,
            r#"{"event":"talent.skip","ts":10,"mode":"segment","stream":"default","segment":"order-dispatch","name":"documents","reason":"capped","use_id":"new"}"#,
            r#"{"event":"sense.complete","ts":11,"mode":"segment","stream":"default","segment":"mismatch","density":"active"}"#,
            r#"{"event":"talent.dispatch","ts":12,"mode":"segment","stream":"default","segment":"mismatch","name":"documents","use_id":"new"}"#,
            r#"{"event":"talent.skip","ts":13,"mode":"segment","stream":"default","segment":"mismatch","name":"documents","reason":"capped","use_id":"other"}"#,
            r#"{"event":"sense.complete","ts":14,"mode":"segment","stream":"default","segment":"fail-after","density":"active"}"#,
            r#"{"event":"talent.dispatch","ts":15,"mode":"segment","stream":"default","segment":"fail-after","name":"documents","use_id":"new"}"#,
            r#"{"event":"talent.skip","ts":16,"mode":"segment","stream":"default","segment":"fail-after","name":"documents","reason":"capped","use_id":"new"}"#,
            r#"{"event":"talent.fail","ts":17,"mode":"segment","stream":"default","segment":"fail-after","name":"documents","use_id":"new"}"#,
            r#"{"event":"sense.complete","ts":18,"mode":"segment","stream":"default","segment":"complete-after","density":"active"}"#,
            r#"{"event":"talent.dispatch","ts":19,"mode":"segment","stream":"default","segment":"complete-after","name":"documents","use_id":"new"}"#,
            r#"{"event":"talent.skip","ts":20,"mode":"segment","stream":"default","segment":"complete-after","name":"documents","reason":"capped","use_id":"new"}"#,
            r#"{"event":"talent.complete","ts":21,"mode":"segment","stream":"default","segment":"complete-after","name":"documents","use_id":"new"}"#,
        ],
    );
    let progress = progress_for(&root, "20990202");
    let row = |segment: &str| {
        &progress[&SegmentIdentity {
            stream: Some("default".into()),
            segment: segment.into(),
        }]
    };

    assert!(!row("stale").capped_by_skip.contains("documents"));
    assert!(!row("stale").completed.contains("documents"));
    assert_eq!(
        thought_for(&progress, "stale"),
        ThoughtVerdict::Floor("documents".into())
    );

    assert!(row("order-cap").capped_by_skip.contains("documents"));
    assert!(!row("order-cap").completed.contains("documents"));
    assert_eq!(
        thought_for(&progress, "order-cap"),
        ThoughtVerdict::Complete
    );

    assert!(row("order-dispatch").capped_by_skip.contains("documents"));
    assert!(!row("order-dispatch").completed.contains("documents"));
    assert_eq!(
        thought_for(&progress, "order-dispatch"),
        ThoughtVerdict::Complete
    );

    assert!(!row("mismatch").capped_by_skip.contains("documents"));
    assert!(!row("mismatch").completed.contains("documents"));
    assert_eq!(
        thought_for(&progress, "mismatch"),
        ThoughtVerdict::Floor("documents".into())
    );

    assert!(!row("fail-after").capped_by_skip.contains("documents"));
    assert!(!row("fail-after").completed.contains("documents"));
    assert_eq!(
        thought_for(&progress, "fail-after"),
        ThoughtVerdict::Floor("documents".into())
    );

    assert!(!row("complete-after").capped_by_skip.contains("documents"));
    assert!(row("complete-after").completed.contains("documents"));
    assert_eq!(
        thought_for(&progress, "complete-after"),
        ThoughtVerdict::Complete
    );
}

#[test]
fn optional_recommendation_withdrawal_is_stream_scoped_and_redispatch_restores_work() {
    let root = root();
    for name in ["screen", "speaker_attribution"] {
        let rows = [
            serde_json::json!({"event":"sense.complete","ts":1,"mode":"segment","stream":"default","segment":name,"density":"active"}),
            serde_json::json!({"event":"talent.complete","ts":2,"mode":"segment","stream":"default","segment":name,"name":"documents"}),
            serde_json::json!({"event":"talent.dispatch","ts":3,"mode":"segment","stream":"default","segment":name,"name":name,"use_id":"old"}),
            serde_json::json!({"event":"talent.fail","ts":4,"mode":"segment","stream":"default","segment":name,"name":name,"use_id":"old"}),
            serde_json::json!({"event":"talent.skip","ts":5,"mode":"segment","stream":"another","segment":name,"name":name,"reason":"not_recommended"}),
        ].map(|row| row.to_string());
        write(
            &root,
            "20990202",
            &format!("{name}.jsonl"),
            &rows.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        assert_eq!(
            thought_for(&progress_for(&root, "20990202"), name),
            ThoughtVerdict::Dispatched(name.to_owned())
        );
        let skip = serde_json::json!({"event":"talent.skip","ts":6,"mode":"segment","stream":"default","segment":name,"name":name,"reason":"not_recommended"}).to_string();
        write(&root, "20990202", &format!("{name}-skip.jsonl"), &[&skip]);
        let progress = progress_for(&root, "20990202");
        assert_eq!(thought_for(&progress, name), ThoughtVerdict::Complete);
        let row = lookup_segment_progress(&progress, "default", name).unwrap();
        assert!(!row.completed.contains(name));
        assert!(!row.capped_by_skip.contains(name));
        let late = serde_json::json!({"event":"talent.fail","ts":7,"mode":"segment","stream":"default","segment":name,"name":name,"use_id":"old"}).to_string();
        write(&root, "20990202", &format!("{name}-late.jsonl"), &[&late]);
        assert_eq!(
            thought_for(&progress_for(&root, "20990202"), name),
            ThoughtVerdict::Complete
        );
        let retry = serde_json::json!({"event":"talent.dispatch","ts":8,"mode":"segment","stream":"default","segment":name,"name":name,"use_id":"new"}).to_string();
        write(&root, "20990202", &format!("{name}-retry.jsonl"), &[&retry]);
        assert_eq!(
            thought_for(&progress_for(&root, "20990202"), name),
            ThoughtVerdict::Dispatched(name.to_owned())
        );
    }
}

#[test]
fn not_recommended_does_not_waive_required_or_unknown_talents() {
    let root = root();
    for name in ["documents", "unknown"] {
        let rows = [
            serde_json::json!({"event":"sense.complete","ts":1,"mode":"segment","stream":"default","segment":name,"density":"active"}),
            serde_json::json!({"event":"talent.dispatch","ts":2,"mode":"segment","stream":"default","segment":name,"name":name}),
            serde_json::json!({"event":"talent.skip","ts":3,"mode":"segment","stream":"default","segment":name,"name":name,"reason":"not_recommended"}),
        ].map(|row| row.to_string());
        write(
            &root,
            "20990202",
            &format!("{name}.jsonl"),
            &rows.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        let progress = progress_for(&root, "20990202");
        assert!(
            lookup_segment_progress(&progress, "default", name)
                .unwrap()
                .dispatched
                .contains(name)
        );
        assert_ne!(thought_for(&progress, name), ThoughtVerdict::Complete);
    }
}
