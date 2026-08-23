// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::VecDeque;

use serde_json::json;
use solstone_core_callosum::{CallosumConnectionPhase, CallosumEnvelope, CallosumReceiveEvent};
use solstone_core_top::{
    RestartEnqueueResult, RestartIdError, RestartIdSource, RestartPhase, RestartRequestError,
    RestartRequestOutcome, SessionRestartIds, TopBrainSource, TopClock, TopInput,
    TopReceiveTransport, TopRenderOp, TopRestartTransport, TopState, TopTerminal,
    acknowledge_restart, advance_restart_attempts, request_restart, run_top_with,
};

struct Transport {
    generation: u64,
    epoch: u64,
    ids: SessionRestartIds,
    results: VecDeque<RestartEnqueueResult>,
    emitted: Vec<(String, String)>,
}

impl Transport {
    fn fixed(nonce: u8) -> Self {
        Self {
            generation: 7,
            epoch: 9,
            ids: SessionRestartIds::with_nonce(4242, [nonce; 16]),
            results: VecDeque::new(),
            emitted: Vec::new(),
        }
    }
}

impl TopRestartTransport for Transport {
    fn emit_restart(&mut self, service: &str, restart_id: &str) -> RestartEnqueueResult {
        self.emitted
            .push((service.to_owned(), restart_id.to_owned()));
        self.results
            .pop_front()
            .unwrap_or(RestartEnqueueResult::Enqueued)
    }

    fn current_generation(&self) -> u64 {
        self.generation
    }

    fn current_epoch(&self) -> u64 {
        self.epoch
    }

    fn restart_ids(&mut self) -> &mut dyn RestartIdSource {
        &mut self.ids
    }
}

fn app_state() -> TopState {
    TopState {
        services: vec![json!({"name":"convey"})],
        ..TopState::default()
    }
}

fn emitted(state: &mut TopState, transport: &mut Transport, now: f64) -> String {
    let RestartRequestOutcome::Emitted { restart_id } =
        request_restart(state, "convey", now, transport)
    else {
        panic!("restart should be emitted")
    };
    restart_id
}

#[test]
fn session_restart_ids_are_distinct_and_never_reused() {
    let mut first = SessionRestartIds::with_nonce(77, [0x01; 16]);
    let mut second = SessionRestartIds::with_nonce(77, [0x02; 16]);
    assert_eq!(
        first.next_restart_id().unwrap(),
        "top-v1-77-01010101010101010101010101010101-1"
    );
    assert_eq!(
        first.next_restart_id().unwrap(),
        "top-v1-77-01010101010101010101010101010101-2"
    );
    assert_eq!(
        second.next_restart_id().unwrap(),
        "top-v1-77-02020202020202020202020202020202-1"
    );
}

#[test]
fn enqueue_rejection_and_sequence_exhaustion_are_visible() {
    let mut state = app_state();
    let mut transport = Transport::fixed(0x11);
    transport.results =
        VecDeque::from([RestartEnqueueResult::Full, RestartEnqueueResult::Enqueued]);
    let first = request_restart(&mut state, "convey", 0.0, &mut transport);
    assert!(matches!(
        first,
        RestartRequestOutcome::Failed {
            restart_id: Some(ref id),
            error: RestartRequestError::QueueFull,
        } if id.ends_with("-1")
    ));
    let second = emitted(&mut state, &mut transport, 1.0);
    assert!(second.ends_with("-2"));
    assert_eq!(transport.emitted.len(), 2);

    for result in [
        RestartEnqueueResult::Closed,
        RestartEnqueueResult::TransportError,
    ] {
        let mut state = app_state();
        let mut transport = Transport::fixed(0x22);
        transport.results.push_back(result.clone());
        assert!(matches!(
            request_restart(&mut state, "convey", 0.0, &mut transport),
            RestartRequestOutcome::Failed {
                restart_id: Some(_),
                ..
            }
        ));
        assert_eq!(transport.ids.next_sequence(), 2);
    }

    let mut state = app_state();
    let mut unavailable = Transport::fixed(0x33);
    unavailable.ids = SessionRestartIds::unavailable(4242);
    assert_eq!(
        request_restart(&mut state, "convey", 0.0, &mut unavailable),
        RestartRequestOutcome::Failed {
            restart_id: None,
            error: RestartRequestError::Id(RestartIdError::EntropyUnavailable),
        }
    );
    assert_eq!(unavailable.ids.next_sequence(), 1);
    assert!(unavailable.emitted.is_empty());

    let mut state = app_state();
    let mut exhausted = Transport::fixed(0x44);
    exhausted.ids = SessionRestartIds::with_nonce_and_sequence(4242, [0x44; 16], u64::MAX);
    assert_eq!(
        request_restart(&mut state, "convey", 0.0, &mut exhausted),
        RestartRequestOutcome::Failed {
            restart_id: None,
            error: RestartRequestError::Id(RestartIdError::SequenceExhausted),
        }
    );
    assert!(exhausted.emitted.is_empty());
}

#[test]
fn only_exact_lifecycle_correlation_advances_active_overlay() {
    let mut state = app_state();
    let mut transport = Transport::fixed(0x55);
    let id = emitted(&mut state, &mut transport, 0.0);
    for (service, restart_id, generation, epoch, event) in [
        ("convey", Some(id.as_str()), 7, 9, "restart"),
        ("sense", Some(id.as_str()), 7, 9, "restarting"),
        ("convey", Some("other"), 7, 9, "restarting"),
        ("convey", Some(id.as_str()), 6, 9, "restarting"),
        ("convey", Some(id.as_str()), 7, 10, "restarting"),
    ] {
        assert!(
            acknowledge_restart(
                &mut state, service, restart_id, generation, epoch, event, 1.0
            )
            .is_none()
        );
        assert_eq!(
            state.restart_attempts["convey"].phase,
            RestartPhase::Pending
        );
    }
    assert_eq!(
        acknowledge_restart(&mut state, "convey", Some(&id), 7, 9, "restarting", 1.0)
            .unwrap()
            .phase,
        RestartPhase::Restarting
    );
    assert_eq!(
        acknowledge_restart(&mut state, "convey", Some(&id), 7, 9, "stopped", 2.0)
            .unwrap()
            .phase,
        RestartPhase::Stopped
    );
    assert_eq!(
        acknowledge_restart(&mut state, "convey", Some(&id), 7, 9, "started", 3.0)
            .unwrap()
            .phase,
        RestartPhase::Started
    );
}

#[test]
fn stopped_restarting_started_and_timeout_transitions_preserve_origin_deadline() {
    let mut state = app_state();
    let mut transport = Transport::fixed(0x66);
    let id = emitted(&mut state, &mut transport, 0.0);
    acknowledge_restart(&mut state, "convey", Some(&id), 7, 9, "stopped", 2.0);
    assert_eq!(
        state.restart_attempts["convey"].started_deadline,
        Some(12.0)
    );
    acknowledge_restart(&mut state, "convey", Some(&id), 7, 9, "restarting", 6.0);
    assert_eq!(
        state.restart_attempts["convey"].started_deadline,
        Some(12.0)
    );
    assert_eq!(
        acknowledge_restart(&mut state, "convey", Some(&id), 7, 9, "started", 11.0)
            .unwrap()
            .phase,
        RestartPhase::Started
    );

    let mut state = app_state();
    let mut transport = Transport::fixed(0x67);
    let id = emitted(&mut state, &mut transport, 0.0);
    acknowledge_restart(&mut state, "convey", Some(&id), 7, 9, "stopped", 2.0);
    assert_eq!(
        advance_restart_attempts(&mut state, 12.0)[0].phase,
        RestartPhase::Failed(solstone_core_top::RestartFailure::RestartTimedOut)
    );

    let mut state = app_state();
    let mut transport = Transport::fixed(0x68);
    let id = emitted(&mut state, &mut transport, 0.0);
    acknowledge_restart(&mut state, "convey", Some(&id), 7, 9, "restarting", 2.0);
    acknowledge_restart(&mut state, "convey", Some(&id), 7, 9, "stopped", 3.0);
    assert_eq!(
        state.restart_attempts["convey"].started_deadline,
        Some(12.0)
    );
    assert_eq!(
        advance_restart_attempts(&mut state, 12.0)[0].phase,
        RestartPhase::Failed(solstone_core_top::RestartFailure::RestartTimedOut)
    );
}

#[test]
fn drain_before_deadline_accepts_boundary_ack_and_rejects_late_ack() {
    let mut state = app_state();
    let mut transport = Transport::fixed(0x77);
    let id = emitted(&mut state, &mut transport, 0.0);
    // Owner-loop order is receive reduction before this deadline call.
    acknowledge_restart(&mut state, "convey", Some(&id), 7, 9, "restarting", 5.0);
    assert!(advance_restart_attempts(&mut state, 5.0).is_empty());
    acknowledge_restart(&mut state, "convey", Some(&id), 7, 9, "started", 15.0);
    assert!(advance_restart_attempts(&mut state, 15.0).is_empty());
    assert_eq!(
        state.restart_attempts["convey"].phase,
        RestartPhase::Started
    );

    let mut state = app_state();
    let mut transport = Transport::fixed(0x78);
    let id = emitted(&mut state, &mut transport, 0.0);
    assert_eq!(
        advance_restart_attempts(&mut state, 5.0)[0].phase,
        RestartPhase::Interrupted
    );
    assert!(
        acknowledge_restart(&mut state, "convey", Some(&id), 7, 9, "restarting", 5.1).is_none()
    );
    let retry = emitted(&mut state, &mut transport, 5.1);
    assert!(retry.ends_with("-2"));
    assert_eq!(
        request_restart(&mut state, "convey", 5.2, &mut transport),
        RestartRequestOutcome::Rejected
    );
}

struct BoundaryClock(f64);

impl TopClock for BoundaryClock {
    fn wall_seconds(&self) -> f64 {
        self.0
    }

    fn monotonic_seconds(&self) -> f64 {
        self.0
    }

    fn datetime(&self) -> serde_json::Value {
        json!({"datetime":"fixture"})
    }

    fn sleep(&mut self, _: f64) -> Result<(), String> {
        Ok(())
    }
}

struct BoundaryTerminal;

impl TopTerminal for BoundaryTerminal {
    fn enter(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn restore(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn width(&mut self) -> Result<usize, String> {
        Ok(120)
    }

    fn render(&mut self, _: &[TopRenderOp]) -> Result<(), String> {
        Ok(())
    }

    fn input(&mut self, _: f64) -> Result<TopInput, String> {
        Ok(TopInput::Quit)
    }
}

struct BoundaryReceive(VecDeque<CallosumReceiveEvent>);

impl TopReceiveTransport for BoundaryReceive {
    fn start(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn next(&mut self) -> Result<Option<CallosumReceiveEvent>, String> {
        Ok(self.0.pop_front())
    }

    fn stop(&mut self) -> Result<(), String> {
        Ok(())
    }
}

struct BoundaryObserver;

impl solstone_core_top::ProcessObserver for BoundaryObserver {
    fn sample(&mut self, _: u32, _: f64) -> solstone_core_top::ProcessSample {
        solstone_core_top::ProcessSample::Missing
    }
}

struct BoundaryBrain;

impl TopBrainSource for BoundaryBrain {
    fn inspect(&mut self) -> Result<serde_json::Value, String> {
        Ok(json!({"lines":["ready"]}))
    }
}

fn lifecycle(event: &str, restart_id: &str) -> CallosumReceiveEvent {
    CallosumReceiveEvent::Envelope {
        generation: 7,
        epoch: 9,
        envelope: CallosumEnvelope {
            tract: "supervisor".to_owned(),
            event: event.to_owned(),
            ts: None,
            extra: serde_json::Map::from_iter([
                ("service".to_owned(), json!("convey")),
                ("restart_id".to_owned(), json!(restart_id)),
            ]),
        },
    }
}

#[test]
fn owner_loop_drains_boundary_lifecycle_before_restart_deadline() {
    let mut state = app_state();
    let mut transport = Transport::fixed(0x79);
    let id = emitted(&mut state, &mut transport, 0.0);
    let mut receive = BoundaryReceive(VecDeque::from([
        CallosumReceiveEvent::Continuity {
            generation: 7,
            epoch: 9,
            phase: CallosumConnectionPhase::Connected,
        },
        lifecycle("restarting", &id),
    ]));
    run_top_with(
        &mut state,
        &mut BoundaryClock(5.0),
        &mut BoundaryTerminal,
        &mut receive,
        &mut BoundaryObserver,
        &mut transport,
        &mut BoundaryBrain,
    )
    .unwrap();
    assert_eq!(
        state.restart_attempts["convey"].phase,
        RestartPhase::Restarting
    );

    let mut state = app_state();
    let mut transport = Transport::fixed(0x7a);
    let id = emitted(&mut state, &mut transport, 0.0);
    state.restart_attempts.get_mut("convey").unwrap().phase = RestartPhase::Restarting;
    state
        .restart_attempts
        .get_mut("convey")
        .unwrap()
        .started_deadline = Some(10.0);
    let mut receive = BoundaryReceive(VecDeque::from([
        CallosumReceiveEvent::Continuity {
            generation: 7,
            epoch: 9,
            phase: CallosumConnectionPhase::Connected,
        },
        lifecycle("started", &id),
    ]));
    run_top_with(
        &mut state,
        &mut BoundaryClock(10.0),
        &mut BoundaryTerminal,
        &mut receive,
        &mut BoundaryObserver,
        &mut transport,
        &mut BoundaryBrain,
    )
    .unwrap();
    assert_eq!(
        state.restart_attempts["convey"].phase,
        RestartPhase::Started
    );
}
