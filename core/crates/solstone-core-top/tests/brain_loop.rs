// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::VecDeque;

use serde_json::json;
use solstone_core_callosum::{CallosumConnectionPhase, CallosumEnvelope, CallosumReceiveEvent};
use solstone_core_top::{
    BrainHealthState, ProcessObserver, ProcessSample, TopBrainSource, TopClock, TopInput,
    TopReceiveTransport, TopRenderOp, TopState, TopTerminal, run_top_with,
};

struct Clock {
    wall: f64,
    monotonic: f64,
}
impl TopClock for Clock {
    fn wall_seconds(&self) -> f64 {
        self.wall
    }
    fn monotonic_seconds(&self) -> f64 {
        self.monotonic
    }
    fn datetime(&self) -> serde_json::Value {
        json!({"datetime":"fixture"})
    }
    fn sleep(&mut self, _: f64) -> Result<(), String> {
        self.wall += 30.0;
        self.monotonic += 30.0;
        Ok(())
    }
}
#[derive(Default)]
struct Terminal {
    inputs: VecDeque<TopInput>,
    frames: Vec<String>,
}
impl TopTerminal for Terminal {
    fn enter(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn restore(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn width(&mut self) -> Result<usize, String> {
        Ok(120)
    }
    fn render(&mut self, ops: &[TopRenderOp]) -> Result<(), String> {
        self.frames.push(
            ops.iter()
                .map(|op| match op {
                    TopRenderOp::Style(token) => token.spelling().to_owned(),
                    TopRenderOp::Print(text) => text.clone(),
                })
                .collect(),
        );
        Ok(())
    }
    fn input(&mut self, _: f64) -> Result<TopInput, String> {
        Ok(self.inputs.pop_front().unwrap_or(TopInput::Quit))
    }
}
struct Receive {
    events: VecDeque<Option<CallosumReceiveEvent>>,
}
impl TopReceiveTransport for Receive {
    fn start(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn next(&mut self) -> Result<Option<CallosumReceiveEvent>, String> {
        Ok(self.events.pop_front().flatten())
    }
    fn stop(&mut self) -> Result<(), String> {
        Ok(())
    }
}
struct Observer;
impl ProcessObserver for Observer {
    fn sample(&mut self, _: u32, _: f64) -> ProcessSample {
        ProcessSample::Missing
    }
}
struct Brain(VecDeque<Result<serde_json::Value, String>>);
impl TopBrainSource for Brain {
    fn inspect(&mut self) -> Result<serde_json::Value, String> {
        self.0.pop_front().expect("scheduled inspection")
    }
}

#[test]
fn brain_failures_are_nonfatal_and_later_success_recovers_only_brain() {
    let completed = CallosumReceiveEvent::Envelope {
        generation: 1,
        epoch: 1,
        envelope: CallosumEnvelope {
            tract: "think".to_owned(),
            event: "completed".to_owned(),
            ts: None,
            extra: serde_json::Map::from_iter([
                ("success".to_owned(), json!(1)),
                ("failed".to_owned(), json!(0)),
                ("duration_ms".to_owned(), json!(1)),
                ("failed_names".to_owned(), json!([])),
            ]),
        },
    };
    let mut state = TopState::default();
    state.continuity.generation = 1;
    state.continuity.epoch = 1;
    state.continuity.connection = CallosumConnectionPhase::Connected;
    state.cpu_cache.insert(9, 17.0);
    let continuity = state.continuity.clone();
    let process_cache = state.cpu_cache.clone();
    let mut terminal = Terminal {
        inputs: VecDeque::from([
            TopInput::None,
            TopInput::None,
            TopInput::None,
            TopInput::None,
            TopInput::Quit,
        ]),
        ..Terminal::default()
    };
    let mut receive = Receive {
        events: VecDeque::from([
            None,
            Some(CallosumReceiveEvent::Continuity {
                generation: 1,
                epoch: 1,
                phase: CallosumConnectionPhase::Connected,
            }),
            Some(completed),
            None,
        ]),
    };
    let mut brain = Brain(VecDeque::from([
        Err("\x1b[31mhostile".to_owned()),
        Err("completion failure".to_owned()),
        Err("periodic failure".to_owned()),
        Ok(json!({"lines":["Brain Health — OK"]})),
    ]));
    run_top_with(
        &mut state,
        &mut Clock {
            wall: 0.0,
            monotonic: 0.0,
        },
        &mut terminal,
        &mut receive,
        &mut Observer,
        &mut brain,
    )
    .unwrap();
    assert!(matches!(
        state.brain_health_state,
        BrainHealthState::Available { .. }
    ));
    assert_eq!(state.continuity.generation, continuity.generation);
    assert_eq!(state.continuity.epoch, continuity.epoch);
    assert_eq!(state.continuity.supervisor, continuity.supervisor);
    assert_eq!(state.continuity.tasks, continuity.tasks);
    assert_eq!(state.continuity.observe, continuity.observe);
    assert_eq!(state.cpu_cache, process_cache);
    assert!(
        terminal
            .frames
            .iter()
            .any(|frame| frame.contains("(status unavailable)"))
    );
    assert!(
        terminal
            .frames
            .iter()
            .any(|frame| frame.contains("\\x1b[31mhostile"))
    );
    assert!(
        terminal
            .frames
            .iter()
            .all(|frame| !frame.contains("\x1b[31mhostile"))
    );
    assert!(
        terminal
            .frames
            .iter()
            .all(|frame| frame.starts_with("<HOME><CLEAR>"))
    );
}
