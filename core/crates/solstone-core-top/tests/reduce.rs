// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_callosum::{
    CallosumConnectionPhase, CallosumEnvelope, CallosumGapReason, CallosumReceiveEvent,
};
use std::collections::VecDeque;

use solstone_core_top::{
    FrameSample, PlainTopStyle, ProcessObserver, ProcessSample, ReductionSample, TopReduceError,
    TopState, apply_receive_event, cleanup_processes, reduce_envelope, render_frame,
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

#[test]
fn new_connection_generation_invalidates_stale_supervisor_state_until_status() {
    let mut state = TopState {
        services: vec![serde_json::json!({"name":"stale", "pid":1})],
        continuity: solstone_core_top::DomainContinuity {
            generation: 1,
            ..solstone_core_top::DomainContinuity::default()
        },
        ..TopState::default()
    };
    apply_receive_event(
        &mut state,
        &CallosumReceiveEvent::Continuity {
            generation: 2,
            epoch: 2,
            phase: CallosumConnectionPhase::Gapped {
                reason: CallosumGapReason::Disconnected,
                dropped_count: 1,
            },
        },
        &ReductionSample::fixture(0.0, "x"),
        &mut Observer,
    )
    .unwrap();
    assert!(state.services.is_empty());
    assert!(state.continuity.supervisor.is_incomplete());
}

#[test]
fn cleanup_refreshes_live_task_metrics_on_every_cycle() {
    struct SequenceObserver(VecDeque<ProcessSample>);
    impl ProcessObserver for SequenceObserver {
        fn sample(&mut self, _: u32, _: f64) -> ProcessSample {
            self.0.pop_front().expect("one sample per cleanup cycle")
        }
    }

    let mut state = TopState {
        running_tasks: [(
            "task".into(),
            serde_json::json!({"ref":"task", "name":"backup", "pid":55}),
        )]
        .into(),
        task_started_at: [("task".into(), 0.0)].into(),
        ..TopState::default()
    };
    let mut observer = SequenceObserver(VecDeque::from([
        ProcessSample::Live {
            rss_bytes: 11 * 1_048_576,
            cpu_percent: 12.0,
        },
        ProcessSample::Live {
            rss_bytes: 47 * 1_048_576,
            cpu_percent: 73.0,
        },
    ]));
    cleanup_processes(
        &mut state,
        &ReductionSample::fixture(5.0, "x"),
        &mut observer,
    );
    cleanup_processes(
        &mut state,
        &ReductionSample::fixture(10.0, "x"),
        &mut observer,
    );

    assert_eq!(state.memory_cache.get(&55), Some(&(47 * 1_048_576)));
    assert_eq!(state.cpu_cache.get(&55), Some(&73.0));
    let rendered = render_frame(
        &state,
        FrameSample {
            wall_seconds: 10.0,
            monotonic_seconds: 10.0,
        },
        120,
        &PlainTopStyle,
    );
    assert!(rendered.contains("     47     73"));
}

#[test]
fn domain_gaps_render_then_clear_on_fresh_domain_events() {
    let mut state = TopState {
        continuity: solstone_core_top::DomainContinuity {
            generation: 1,
            ..solstone_core_top::DomainContinuity::default()
        },
        ..TopState::default()
    };
    let mut observer = Observer;
    apply_receive_event(
        &mut state,
        &CallosumReceiveEvent::Continuity {
            generation: 2,
            epoch: 2,
            phase: CallosumConnectionPhase::Gapped {
                reason: CallosumGapReason::Disconnected,
                dropped_count: 1,
            },
        },
        &ReductionSample::fixture(0.0, "x"),
        &mut observer,
    )
    .unwrap();
    assert!(
        state.continuity.tasks.is_incomplete()
            && state.continuity.observe.is_incomplete()
            && state.continuity.think.is_incomplete()
    );
    let rendered = render_frame(&state, FrameSample::default(), 120, &PlainTopStyle);
    assert_eq!(rendered.matches("(reconnecting)").count(), 4);

    apply_receive_event(
        &mut state,
        &CallosumReceiveEvent::Continuity {
            generation: 2,
            epoch: 2,
            phase: CallosumConnectionPhase::Connected,
        },
        &ReductionSample::fixture(1.0, "x"),
        &mut observer,
    )
    .unwrap();
    for value in [
        serde_json::json!({"tract":"supervisor","event":"status","services":[]}),
        serde_json::json!({"tract":"logs","event":"exec","ref":"task","name":"backup","pid":3}),
        serde_json::json!({"tract":"observe","event":"status","mode":"idle"}),
        serde_json::json!({"tract":"think","event":"started"}),
    ] {
        apply_receive_event(
            &mut state,
            &CallosumReceiveEvent::Envelope {
                generation: 2,
                epoch: 2,
                envelope: envelope(&value),
            },
            &ReductionSample::fixture(1.0, "x"),
            &mut observer,
        )
        .unwrap();
    }
    assert!(!state.continuity.supervisor.is_incomplete());
    assert!(
        state.continuity.tasks.is_incomplete()
            && state.continuity.observe.is_incomplete()
            && state.continuity.think.is_incomplete()
    );
    let rendered = render_frame(&state, FrameSample::default(), 120, &PlainTopStyle);
    assert_eq!(rendered.matches("(reconnecting)").count(), 3);
}
