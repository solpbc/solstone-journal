// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use serde_json::{Value, json};
use solstone_core_system_health::{
    DataStateMap, FilesystemHealthLogSource, FoldRead, SegmentBlockerDimension, SegmentIdentity,
    SegmentInput, TerminalEvent, ThoughtVerdict, blocked_segment_keys, classify_segment_completion,
    is_floor_talent_capped, lookup_segment_progress, read_completed_since, read_completed_units,
    read_daily_deterministic_failures, read_segment_progress, read_terminal_states,
    segment_fully_sensed, segment_fully_thought, segment_requires_processing,
};

use super::corpus;

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
fn synthesized_corpus_folds_match_inlined_expectations() {
    let root = tempfile::tempdir().unwrap();
    corpus::write_corpus(root.path());
    let source = FilesystemHealthLogSource::new(root.path());

    let terminals = read_terminal_states(&source, corpus::CURRENT_DAY, false).unwrap();
    assert_eq!(
        terminals.malformed_line_count, 1,
        "terminal_states malformed_line_count"
    );
    let expected_terminals: Value = serde_json::from_str(TERMINAL_STATES).unwrap();
    assert_eq!(
        terminal_states(&terminals.value),
        expected_terminals,
        "terminal_states"
    );

    let completed = read_completed_units(&source, corpus::CURRENT_DAY).unwrap();
    assert_eq!(
        completed_units(&completed),
        serde_json::from_str::<Value>(COMPLETED_UNITS).unwrap(),
        "completed_units"
    );

    let since = read_completed_since(&source, corpus::CURRENT_DAY, corpus::SINCE_MS).unwrap();
    assert_eq!(
        since.malformed_line_count, 1,
        "completed_since malformed_line_count"
    );
    assert_eq!(
        completed_since(&since.value),
        serde_json::from_str::<Value>(COMPLETED_SINCE).unwrap(),
        "completed_since"
    );

    let daily = read_daily_deterministic_failures(&source, corpus::CURRENT_DAY).unwrap();
    assert_eq!(
        daily_failures(&daily.value),
        serde_json::from_str::<Value>(DAILY_FAILURES).unwrap(),
        "daily_deterministic_failures"
    );

    let progress = read_segment_progress(&source, corpus::CURRENT_DAY).unwrap();
    assert_eq!(
        progress.malformed_line_count, 1,
        "segment_progress malformed_line_count"
    );
    assert_eq!(
        segment_progress(&progress.value),
        serde_json::from_str::<Value>(SEGMENT_PROGRESS).unwrap(),
        "segment_progress"
    );
    assert_eq!(
        pure_completion(&pure_segments(), &progress.value),
        serde_json::from_str::<Value>(PURE_COMPLETION).unwrap(),
        "pure_completion"
    );
    assert_eq!(
        json!({
            "cap_true": is_floor_talent_capped(
                &source, corpus::CURRENT_DAY, Some("default"), "cap-true", "documents"
            ).unwrap().value,
            "cap_short": is_floor_talent_capped(
                &source, corpus::CURRENT_DAY, Some("default"), "cap-short", "documents"
            ).unwrap().value,
        }),
        json!({"cap_true": true, "cap_short": false}),
        "floor_caps"
    );
}

const TERMINAL_STATES: &str = r#"[
  {"state":{"deterministic_fail_count":0,"last_fail_ts":null,"last_real_complete_ts":450,"latest_event":"complete","latest_ts":450,"model":null,"oldest_trailing_fail_ts":null,"provider":null,"reason_code":null,"state":null,"trailing_fail_count":0,"use_id":null},"unit":{"activity":"meeting","facet":"work","mode":"activity","name":"summary","segment":null,"stream":null}},
  {"state":{"deterministic_fail_count":0,"last_fail_ts":null,"last_real_complete_ts":510,"latest_event":"complete","latest_ts":510,"model":null,"oldest_trailing_fail_ts":null,"provider":null,"reason_code":null,"state":null,"trailing_fail_count":0,"use_id":null},"unit":{"activity":null,"facet":null,"mode":"daily","name":"completed-daily","segment":null,"stream":null}},
  {"state":{"deterministic_fail_count":1,"last_fail_ts":500,"last_real_complete_ts":500,"latest_event":"fail","latest_ts":500,"model":null,"oldest_trailing_fail_ts":500,"provider":null,"reason_code":"no_output","state":null,"trailing_fail_count":1,"use_id":null},"unit":{"activity":null,"facet":"work","mode":"daily","name":"cross-file","segment":null,"stream":null}},
  {"state":{"deterministic_fail_count":1,"last_fail_ts":520,"last_real_complete_ts":null,"latest_event":"fail","latest_ts":520,"model":null,"oldest_trailing_fail_ts":520,"provider":null,"reason_code":"no_output","state":null,"trailing_fail_count":1,"use_id":null},"unit":{"activity":null,"facet":"work","mode":"daily","name":"daily-deterministic","segment":null,"stream":null}},
  {"state":{"deterministic_fail_count":0,"last_fail_ts":null,"last_real_complete_ts":100,"latest_event":"complete","latest_ts":900,"model":null,"oldest_trailing_fail_ts":null,"provider":null,"reason_code":null,"state":null,"trailing_fail_count":0,"use_id":null},"unit":{"activity":null,"facet":null,"mode":"segment","name":"documents","segment":"cadence","stream":"default"}},
  {"state":{"deterministic_fail_count":0,"last_fail_ts":9000004,"last_real_complete_ts":null,"latest_event":"fail","latest_ts":9000004,"model":null,"oldest_trailing_fail_ts":9000000,"provider":null,"reason_code":null,"state":null,"trailing_fail_count":5,"use_id":null},"unit":{"activity":null,"facet":null,"mode":"segment","name":"documents","segment":"cap-short","stream":"default"}},
  {"state":{"deterministic_fail_count":0,"last_fail_ts":8200000,"last_real_complete_ts":null,"latest_event":"fail","latest_ts":8200000,"model":null,"oldest_trailing_fail_ts":1000000,"provider":null,"reason_code":null,"state":null,"trailing_fail_count":5,"use_id":null},"unit":{"activity":null,"facet":null,"mode":"segment","name":"documents","segment":"cap-true","stream":"default"}},
  {"state":{"deterministic_fail_count":0,"last_fail_ts":null,"last_real_complete_ts":400,"latest_event":"complete","latest_ts":400,"model":null,"oldest_trailing_fail_ts":null,"provider":null,"reason_code":null,"state":null,"trailing_fail_count":0,"use_id":null},"unit":{"activity":null,"facet":null,"mode":"segment","name":"entities","segment":"current","stream":"default"}}
]"#;

const COMPLETED_UNITS: &str = r#"[{"facet":null,"mode":"daily","name":"completed-daily"}]"#;

const COMPLETED_SINCE: &str = r#"{
  "activities":[{"activity":"meeting","facet":"work","ts":450}],
  "segments":[{"segment":"prior","stream":"legacy","ts":300},{"segment":"current","stream":"default","ts":400}]
}"#;

const DAILY_FAILURES: &str = r#"[
  {"count":1,"facet":"work","name":"cross-file","reason_code":"no_output"},
  {"count":1,"facet":"work","name":"daily-deterministic","reason_code":"no_output"}
]"#;

const SEGMENT_PROGRESS: &str = r#"[
  {"capped_by_skip":[],"change_class":null,"completed":["documents"],"density":null,"dispatched":[],"segment":"cadence","sensed":false,"stream":"default","unconfigured":[]},
  {"capped_by_skip":[],"change_class":null,"completed":[],"density":null,"dispatched":[],"segment":"cap-short","sensed":false,"stream":"default","unconfigured":[]},
  {"capped_by_skip":[],"change_class":null,"completed":[],"density":null,"dispatched":[],"segment":"cap-true","sensed":false,"stream":"default","unconfigured":[]},
  {"capped_by_skip":[],"change_class":null,"completed":["entities"],"density":null,"dispatched":[],"segment":"current","sensed":false,"stream":"default","unconfigured":[]},
  {"capped_by_skip":["documents"],"change_class":"changed","completed":[],"density":"active","dispatched":["entities"],"segment":"progress","sensed":true,"stream":"default","unconfigured":["entities"]}
]"#;

const PURE_COMPLETION: &str = r#"{
  "blocked":[{"segment":"not-sensed","stream":"default"},{"segment":"progress","stream":"default"}],
  "classification":{"blockers":[{"detail":"dispatched:entities","dimension":"not_thought","segment":"progress"},{"detail":"screen=pending","dimension":"not_sensed","segment":"not-sensed"}],"capped":1,"exhausted":[],"not_sensed":1,"not_thought":1,"total":3},
  "fully_sensed":[true,false,true],
  "lookup_found":true,
  "requires_processing":[true,true,false],
  "thought":[false,"dispatched:entities"]
}"#;
