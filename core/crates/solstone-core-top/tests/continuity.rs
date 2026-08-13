// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_callosum::{
    CallosumConnectionPhase, CallosumEnvelope, CallosumGapReason, CallosumReceiveEvent,
};
use solstone_core_top::{
    DomainRecovery, ProcessObserver, ProcessSample, ReductionSample, TopState, apply_receive_event,
};

#[derive(Default)]
struct Observer {
    calls: usize,
}

impl ProcessObserver for Observer {
    fn sample(&mut self, _: u32, _: f64) -> ProcessSample {
        self.calls += 1;
        ProcessSample::Missing
    }
}

fn sample() -> ReductionSample {
    ReductionSample::fixture(10.0, "2027-01-15T08:00:00")
}

fn envelope(
    tract: &str,
    event: &str,
    extra: serde_json::Map<String, serde_json::Value>,
) -> CallosumEnvelope {
    CallosumEnvelope {
        tract: tract.to_owned(),
        event: event.to_owned(),
        ts: None,
        extra,
    }
}

fn connected(state: &mut TopState, observer: &mut Observer) {
    apply_receive_event(
        state,
        &CallosumReceiveEvent::Continuity {
            generation: 1,
            epoch: 1,
            phase: CallosumConnectionPhase::Connected,
        },
        &sample(),
        observer,
    );
}

fn gap(state: &mut TopState, observer: &mut Observer) {
    apply_receive_event(
        state,
        &CallosumReceiveEvent::Continuity {
            generation: 1,
            epoch: 2,
            phase: CallosumConnectionPhase::Gapped {
                reason: CallosumGapReason::Disconnected,
                dropped_count: 1,
            },
        },
        &sample(),
        observer,
    );
}

#[test]
fn owner_rejects_wrong_generation_or_epoch_before_reduction() {
    let mut state = TopState::default();
    let mut observer = Observer::default();
    connected(&mut state, &mut observer);
    let before = state.clone();
    for (generation, epoch) in [(0, 1), (1, 0), (2, 1)] {
        apply_receive_event(
            &mut state,
            &CallosumReceiveEvent::Envelope {
                generation,
                epoch,
                envelope: envelope(
                    "logs",
                    "exec",
                    serde_json::Map::from_iter([
                        ("ref".into(), serde_json::json!("r")),
                        ("name".into(), serde_json::json!("n")),
                        ("pid".into(), serde_json::json!(7)),
                    ]),
                ),
            },
            &sample(),
            &mut observer,
        );
    }
    assert_eq!(state.running_tasks, before.running_tasks);
    assert_eq!(observer.calls, 0);
    assert_eq!(state.continuity.rejected_receive_events, 3);
}

#[test]
fn supervisor_snapshot_recovers_only_supervisor_domain() {
    let mut state = TopState::default();
    let mut observer = Observer::default();
    connected(&mut state, &mut observer);
    gap(&mut state, &mut observer);
    apply_receive_event(
        &mut state,
        &CallosumReceiveEvent::Continuity {
            generation: 1,
            epoch: 2,
            phase: CallosumConnectionPhase::Connected,
        },
        &sample(),
        &mut observer,
    );
    apply_receive_event(
        &mut state,
        &CallosumReceiveEvent::Envelope {
            generation: 1,
            epoch: 2,
            envelope: envelope(
                "supervisor",
                "status",
                serde_json::Map::from_iter([
                    ("services".into(), serde_json::json!([])),
                    ("crashed".into(), serde_json::json!([])),
                    ("queues".into(), serde_json::json!({})),
                ]),
            ),
        },
        &sample(),
        &mut observer,
    );
    assert_eq!(state.continuity.supervisor, DomainRecovery::Complete);
    assert!(state.continuity.tasks.is_incomplete());
    assert!(state.continuity.observe.is_incomplete());
    assert!(state.continuity.think.is_incomplete());
}

#[test]
fn gap_marks_domains_independently_and_evidence_does_not_complete_them() {
    let mut state = TopState::default();
    let mut observer = Observer::default();
    connected(&mut state, &mut observer);
    gap(&mut state, &mut observer);
    assert!(state.continuity.supervisor.is_incomplete());
    assert!(state.continuity.tasks.is_incomplete());
    assert!(state.continuity.observe.is_incomplete());
    assert!(state.continuity.think.is_incomplete());
    apply_receive_event(
        &mut state,
        &CallosumReceiveEvent::Continuity {
            generation: 1,
            epoch: 2,
            phase: CallosumConnectionPhase::Connected,
        },
        &sample(),
        &mut observer,
    );
    for (tract, event, extra) in [
        (
            "logs",
            "exec",
            serde_json::Map::from_iter([
                ("ref".into(), serde_json::json!("r")),
                ("name".into(), serde_json::json!("n")),
                ("pid".into(), serde_json::json!(3)),
            ]),
        ),
        (
            "observe",
            "status",
            serde_json::Map::from_iter([("mode".into(), serde_json::json!("idle"))]),
        ),
        ("think", "started", serde_json::Map::new()),
    ] {
        apply_receive_event(
            &mut state,
            &CallosumReceiveEvent::Envelope {
                generation: 1,
                epoch: 2,
                envelope: envelope(tract, event, extra),
            },
            &sample(),
            &mut observer,
        );
    }
    assert_eq!(
        state.continuity.tasks,
        DomainRecovery::Incomplete {
            post_gap_evidence: true
        }
    );
    assert_eq!(
        state.continuity.observe,
        DomainRecovery::Incomplete {
            post_gap_evidence: true
        }
    );
    assert_eq!(
        state.continuity.think,
        DomainRecovery::Incomplete {
            post_gap_evidence: true
        }
    );
}

#[test]
fn observe_extension_only_row_is_byte_equivalent_noop_after_gap() {
    let mut state = TopState::default();
    let mut observer = Observer::default();
    connected(&mut state, &mut observer);
    gap(&mut state, &mut observer);
    apply_receive_event(
        &mut state,
        &CallosumReceiveEvent::Continuity {
            generation: 1,
            epoch: 2,
            phase: CallosumConnectionPhase::Connected,
        },
        &sample(),
        &mut observer,
    );
    let before = state.clone();
    apply_receive_event(
        &mut state,
        &CallosumReceiveEvent::Envelope {
            generation: 1,
            epoch: 2,
            envelope: envelope(
                "observe",
                "status",
                serde_json::Map::from_iter([("producer_health".into(), serde_json::json!("ok"))]),
            ),
        },
        &sample(),
        &mut observer,
    );
    assert_eq!(state, before);
}
