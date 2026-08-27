// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_callosum::{
    CallosumConnectionPhase, CallosumEnvelope, CallosumGapReason, CallosumReceiveEvent,
};
use std::collections::VecDeque;

use solstone_core_top::{
    FrameSample, PlainTopStyle, ProcessBirth, ProcessIdentity, ProcessObserver, ProcessSample,
    ReductionDisposition, ReductionSample, TopState, apply_receive_event, cleanup_processes,
    reduce_envelope, render_frame,
};

const FIXTURE: &str = include_str!("../../../fixtures/top_reference.json");

struct Observer;
impl ProcessObserver for Observer {
    fn sample(&mut self, pid: u32, _now: f64) -> ProcessSample {
        if pid == 102 {
            ProcessSample::Missing
        } else {
            ProcessSample::Live {
                identity: ProcessIdentity {
                    pid,
                    birth: ProcessBirth::LinuxStartTicks(1),
                },
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
        let result = reduce_envelope(
            &mut state,
            &envelope(&entry["event"]),
            &ReductionSample::fixture(1_800_000_000.0 + index as f64 * 7.0, "2027-01-15T08:00:00"),
            &mut observer,
        );
        assert!(matches!(result, ReductionDisposition::Applied(_)));
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
        let name = case["name"].as_str().unwrap();
        match name {
            "unknown-tract" => {
                assert_eq!(result, ReductionDisposition::Ignored);
                assert_eq!(state.malformed_events, 0);
            }
            "logs-line-wrong-stream" => {
                assert!(matches!(result, ReductionDisposition::Malformed(_)));
                assert_eq!(state.fixture_value(), TopState::default().fixture_value());
                assert_eq!(state.malformed_events, 1);
            }
            "observe-duration-wrong-type" => {
                assert!(matches!(result, ReductionDisposition::Malformed(_)));
                assert_eq!(state.fixture_value(), TopState::default().fixture_value());
                assert_eq!(state.malformed_events, 1);
            }
            "think-completed-defaults" => {
                assert!(matches!(result, ReductionDisposition::Malformed(_)));
                assert_eq!(state.fixture_value(), TopState::default().fixture_value());
                assert_eq!(state.malformed_events, 1);
            }
            _ => {
                assert!(
                    matches!(result, ReductionDisposition::Malformed(_)),
                    "{name}"
                );
                assert_eq!(
                    state.fixture_value(),
                    TopState::default().fixture_value(),
                    "{name}"
                );
                assert_eq!(state.malformed_events, 1, "{name}");
            }
        }
    }
}

#[test]
fn malformed_cases_each_have_a_same_route_valid_twin() {
    let twins = [
        serde_json::json!({"tract":"other","event":"status"}),
        serde_json::json!({"tract":"supervisor","event":"status","services":[],"crashed":[],"queues":{}}),
        serde_json::json!({"tract":"supervisor","event":"restarting","service":"ok"}),
        serde_json::json!({"tract":"supervisor","event":"started","service":"ok"}),
        serde_json::json!({"tract":"supervisor","event":"stopped","service":"ok"}),
        serde_json::json!({"tract":"supervisor","event":"queue","command":"x","queued":1}),
        serde_json::json!({"tract":"supervisor","event":"queue","command":"x","queued":1}),
        serde_json::json!({"tract":"logs","event":"exec","name":"n","ref":"r","pid":1}),
        serde_json::json!({"tract":"logs","event":"line","ref":"r","name":"n","pid":1,"stream":"stdout","line":"x"}),
        serde_json::json!({"tract":"observe","event":"status","mode":"idle"}),
        serde_json::json!({"tract":"observe","event":"observed","day":"d","segment":"s","duration":1}),
        serde_json::json!({"tract":"think","event":"completed","success":1,"failed":0,"duration_ms":1,"failed_names":[]}),
    ];
    for value in twins {
        let mut state = TopState::default();
        let result = reduce_envelope(
            &mut state,
            &envelope(&value),
            &ReductionSample::fixture(0.0, "x"),
            &mut Observer,
        );
        if value["tract"] == "other" {
            assert_eq!(result, ReductionDisposition::Ignored);
        } else {
            assert!(matches!(result, ReductionDisposition::Applied(_)));
        }
    }
}

#[test]
fn identity_and_order_twins_preserve_distinct_refs_and_service_order() {
    let mut state = TopState::default();
    let mut observer = Observer;
    for value in [
        serde_json::json!({"tract":"supervisor","event":"status","services":[{"name":"svc-10","ref":"r10","pid":10,"uptime_seconds":0},{"name":"svc-2","ref":"r2","pid":2,"uptime_seconds":0}],"crashed":[],"queues":{}}),
        serde_json::json!({"tract":"logs","event":"exec","ref":"a","name":"same","pid":3}),
        serde_json::json!({"tract":"logs","event":"exec","ref":"b","name":"same","pid":4}),
    ] {
        let result = reduce_envelope(
            &mut state,
            &envelope(&value),
            &ReductionSample::fixture(0.0, "x"),
            &mut observer,
        );
        assert!(matches!(result, ReductionDisposition::Applied(_)));
    }
    assert_eq!(state.services[0]["name"], "svc-10");
    assert_eq!(state.services[1]["name"], "svc-2");
    assert!(state.running_tasks.contains_key("a") && state.running_tasks.contains_key("b"));
}

#[test]
fn supervisor_status_preserves_crash_phase_for_rendering() {
    let mut state = TopState::default();
    let mut observer = Observer;
    let result = reduce_envelope(
        &mut state,
        &envelope(&serde_json::json!({
            "tract":"supervisor",
            "event":"status",
            "services":[],
            "crashed":[{"name":"convey","restart_attempts":5,"phase":"backoff"}],
            "queues":{}
        })),
        &ReductionSample::fixture(0.0, "x"),
        &mut observer,
    );
    assert!(matches!(result, ReductionDisposition::Applied(_)));
    assert_eq!(state.crashed[0]["phase"], "backoff");
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
    );
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
            identity: ProcessIdentity {
                pid: 55,
                birth: ProcessBirth::LinuxStartTicks(1),
            },
            rss_bytes: 11 * 1_048_576,
            cpu_percent: 12.0,
        },
        ProcessSample::Live {
            identity: ProcessIdentity {
                pid: 55,
                birth: ProcessBirth::LinuxStartTicks(1),
            },
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
    );
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
    );
    for value in [
        serde_json::json!({"tract":"supervisor","event":"status","services":[],"crashed":[],"queues":{}}),
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
        );
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
