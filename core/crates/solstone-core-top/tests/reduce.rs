// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_callosum::CallosumEnvelope;
use solstone_core_top::{
    ProcessObserver, ProcessSample, ReductionSample, TopReduceError, TopState, reduce_envelope,
};

const FIXTURE: &str = include_str!("../../../fixtures/top_reference.json");

struct Observer;
impl ProcessObserver for Observer {
    fn sample(&mut self, pid: u32, _now: f64) -> ProcessSample {
        if pid == 102 {
            ProcessSample::Missing
        } else {
            ProcessSample::Live {
                rss_bytes: 10 * 1024 * 1024,
                cpu_percent: 12.4,
            }
        }
    }
}

fn envelope(value: &Value) -> CallosumEnvelope {
    serde_json::from_value(value.clone()).unwrap()
}

#[test]
fn retained_valid_events_reduce_in_order_without_singleton_shortcuts() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let mut state = TopState {
        selected: 99,
        ..TopState::default()
    };
    let mut observer = Observer;
    for (index, entry) in fixture["events"].as_array().unwrap().iter().enumerate() {
        reduce_envelope(
            &mut state,
            &envelope(&entry["event"]),
            &ReductionSample::fixture(1_800_000_000.0 + index as f64 * 7.0, "2027-01-15T08:00:00"),
            &mut observer,
        )
        .unwrap();
    }
    assert_eq!(state.services.len(), 2);
    assert_eq!(
        state.command_queues.get("backup"),
        Some(&serde_json::json!(4))
    );
    assert_eq!(state.recent_segments.len(), 3);
    assert_eq!(state.recent_segments[0][1], "004");
    assert!(state.think_last_completed.contains_key("duration_ms"));
    assert!(!state.think_running);
}

#[test]
fn retained_malformed_cases_are_classified_without_coercion() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    for case in fixture["malformed_events"].as_array().unwrap() {
        let mut state = TopState::default();
        let mut observer = Observer;
        let result = reduce_envelope(
            &mut state,
            &envelope(&case["event"]),
            &ReductionSample::fixture(1_800_000_000.0, "2027-01-15T08:00:00"),
            &mut observer,
        );
        match case["name"].as_str().unwrap() {
            "supervisor-service-missing-pid" => {
                assert_eq!(result.unwrap_err(), TopReduceError::MissingServicePid)
            }
            "supervisor-services-wrong-type" => {
                assert_eq!(result.unwrap_err(), TopReduceError::ServicesWrongType)
            }
            "supervisor-queues-wrong-type" => {
                assert_eq!(result.unwrap_err(), TopReduceError::QueuesWrongType)
            }
            "queue-wrong-count" => {
                assert_eq!(result.unwrap_err(), TopReduceError::QueueCountWrongType)
            }
            "logs-line-wrong-stream" => {
                result.unwrap();
                assert_eq!(state.last_log_lines["r"][1], 7);
            }
            "observe-duration-wrong-type" => {
                result.unwrap();
                assert_eq!(state.recent_segments[0][2], "sixty");
            }
            "think-completed-defaults" => assert!(result.unwrap().refresh_brain),
            _ => assert!(result.is_ok(), "{}", case["name"]),
        }
    }
}

#[test]
fn malformed_cases_each_have_a_same_route_valid_twin() {
    let twins = [
        serde_json::json!({"tract":"other","event":"status"}),
        serde_json::json!({"tract":"supervisor","event":"status","services":[],"crashed":[]}),
        serde_json::json!({"tract":"supervisor","event":"status","services":[{"name":"ok","pid":1}]}),
        serde_json::json!({"tract":"supervisor","event":"status","services":[]}),
        serde_json::json!({"tract":"supervisor","event":"status","queues":{}}),
        serde_json::json!({"tract":"supervisor","event":"queue","command":"x","queued":1}),
        serde_json::json!({"tract":"supervisor","event":"queue","command":"x","queued":1}),
        serde_json::json!({"tract":"logs","event":"exec","name":"n","ref":"r","pid":1}),
        serde_json::json!({"tract":"logs","event":"line","ref":"r","line":"x"}),
        serde_json::json!({"tract":"logs","event":"line","ref":"r","stream":"stdout","line":"x"}),
        serde_json::json!({"tract":"observe","event":"observed","day":"d","segment":"s"}),
        serde_json::json!({"tract":"observe","event":"observed","day":"d","segment":"s","duration":1}),
        serde_json::json!({"tract":"think","event":"completed"}),
    ];
    for value in twins {
        let mut state = TopState::default();
        reduce_envelope(
            &mut state,
            &envelope(&value),
            &ReductionSample::fixture(0.0, "x"),
            &mut Observer,
        )
        .unwrap();
    }
}

#[test]
fn identity_and_order_twins_preserve_distinct_refs_and_service_order() {
    let mut state = TopState::default();
    let mut observer = Observer;
    for value in [
        serde_json::json!({"tract":"supervisor","event":"status","services":[{"name":"svc-10","pid":10},{"name":"svc-2","pid":2}]}),
        serde_json::json!({"tract":"logs","event":"exec","ref":"a","name":"same","pid":3}),
        serde_json::json!({"tract":"logs","event":"exec","ref":"b","name":"same","pid":4}),
    ] {
        reduce_envelope(
            &mut state,
            &envelope(&value),
            &ReductionSample::fixture(0.0, "x"),
            &mut observer,
        )
        .unwrap();
    }
    assert_eq!(state.services[0]["name"], "svc-10");
    assert_eq!(state.services[1]["name"], "svc-2");
    assert!(state.running_tasks.contains_key("a") && state.running_tasks.contains_key("b"));
}
