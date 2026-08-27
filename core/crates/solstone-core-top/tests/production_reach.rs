// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use serde_json::json;
use solstone_core_callosum::{CallosumConnectionPhase, CallosumEnvelope, CallosumReceiveEvent};
use solstone_core_top::{
    ProcessObserver, ProcessSample, TopBrainSource, TopClock, TopInput, TopReceiveTransport,
    TopRenderOp, TopState, TopTerminal, run_top_with_outer_panic_cleanup,
};

const EXPECTED_TRACE: &[&str] = &[
    "receive.start",
    "clock.monotonic",
    "terminal.enter",
    "clock.wall",
    "clock.monotonic",
    "brain.inspect",
    "clock.wall",
    "clock.monotonic",
    "clock.datetime",
    "receive.next",
    "receive.next",
    "observer.sample",
    "receive.next",
    "terminal.width",
    "terminal.render",
    "terminal.input",
    "terminal.restore",
    "receive.stop",
];

#[derive(Clone)]
struct Trace {
    rows: Rc<RefCell<Vec<String>>>,
    silenced: Option<&'static str>,
}

impl Trace {
    fn new(silenced: Option<&'static str>) -> Self {
        Self {
            rows: Rc::new(RefCell::new(Vec::new())),
            silenced,
        }
    }

    fn record(&self, seam: &'static str, row: &'static str) {
        if self.silenced != Some(seam) {
            self.rows.borrow_mut().push(row.to_owned());
        }
    }

    fn rows(&self) -> Vec<String> {
        self.rows.borrow().clone()
    }
}

struct Clock(Trace);

impl TopClock for Clock {
    fn wall_seconds(&self) -> f64 {
        self.0.record("clock", "clock.wall");
        100.0
    }

    fn monotonic_seconds(&self) -> f64 {
        self.0.record("clock", "clock.monotonic");
        100.0
    }

    fn datetime(&self) -> serde_json::Value {
        self.0.record("clock", "clock.datetime");
        json!({"datetime":"fixture"})
    }

    fn sleep(&mut self, _: f64) -> Result<(), String> {
        self.0.record("clock", "clock.sleep");
        Ok(())
    }
}

struct Terminal {
    trace: Trace,
    inputs: VecDeque<TopInput>,
}

impl TopTerminal for Terminal {
    fn enter(&mut self) -> Result<(), String> {
        self.trace.record("terminal", "terminal.enter");
        Ok(())
    }

    fn restore(&mut self) -> Result<(), String> {
        self.trace.record("terminal", "terminal.restore");
        Ok(())
    }

    fn width(&mut self) -> Result<usize, String> {
        self.trace.record("terminal", "terminal.width");
        Ok(120)
    }

    fn render(&mut self, _: &[TopRenderOp]) -> Result<(), String> {
        self.trace.record("terminal", "terminal.render");
        Ok(())
    }

    fn input(&mut self, _: f64) -> Result<TopInput, String> {
        self.trace.record("terminal", "terminal.input");
        Ok(self.inputs.pop_front().unwrap_or(TopInput::Quit))
    }
}

struct Receive {
    trace: Trace,
    events: VecDeque<CallosumReceiveEvent>,
}

impl TopReceiveTransport for Receive {
    fn start(&mut self) -> Result<(), String> {
        self.trace.record("receive", "receive.start");
        Ok(())
    }

    fn next(&mut self) -> Result<Option<CallosumReceiveEvent>, String> {
        self.trace.record("receive", "receive.next");
        Ok(self.events.pop_front())
    }

    fn stop(&mut self) -> Result<(), String> {
        self.trace.record("receive", "receive.stop");
        Ok(())
    }
}

struct Observer(Trace);

impl ProcessObserver for Observer {
    fn sample(&mut self, _: u32, _: f64) -> ProcessSample {
        self.0.record("observer", "observer.sample");
        ProcessSample::Missing
    }
}

struct Brain(Trace);

impl TopBrainSource for Brain {
    fn inspect(&mut self) -> Result<serde_json::Value, String> {
        self.0.record("brain", "brain.inspect");
        Ok(json!({"lines":["ready"]}))
    }
}

fn run_recording_composition(silenced: Option<&'static str>) -> Vec<String> {
    let trace = Trace::new(silenced);
    let events = VecDeque::from([
        CallosumReceiveEvent::Continuity {
            generation: 1,
            epoch: 1,
            phase: CallosumConnectionPhase::Connected,
        },
        CallosumReceiveEvent::Envelope {
            generation: 1,
            epoch: 1,
            envelope: CallosumEnvelope {
                tract: "supervisor".to_owned(),
                event: "status".to_owned(),
                ts: None,
                extra: serde_json::Map::from_iter([
                    (
                        "services".to_owned(),
                        json!([{"name":"convey","ref":"convey-1","pid":1,"uptime_seconds":0}]),
                    ),
                    ("crashed".to_owned(), json!([])),
                    ("queues".to_owned(), json!({})),
                ]),
            },
        },
    ]);
    let mut state = TopState::default();
    let mut clock = Clock(trace.clone());
    let mut terminal = Terminal {
        trace: trace.clone(),
        inputs: VecDeque::from([TopInput::Quit]),
    };
    let mut receive = Receive {
        trace: trace.clone(),
        events,
    };
    let mut observer = Observer(trace.clone());
    let mut brain = Brain(trace.clone());
    run_top_with_outer_panic_cleanup(
        &mut state,
        &mut clock,
        &mut terminal,
        &mut receive,
        &mut observer,
        &mut brain,
    )
    .expect("recording composition should finish normally");
    trace.rows()
}

fn matches_full_trace(rows: &[String]) -> bool {
    rows.iter()
        .map(String::as_str)
        .eq(EXPECTED_TRACE.iter().copied())
}

#[test]
fn production_composition_reaches_every_owned_boundary_in_order() {
    assert!(matches_full_trace(&run_recording_composition(None)));

    // This reuses the identical full-trace predicate for each test-only
    // no-op seam. Removing any owner boundary makes the proof fail.
    for seam in ["clock", "terminal", "receive", "observer", "brain"] {
        assert!(
            !matches_full_trace(&run_recording_composition(Some(seam))),
            "silencing {seam} unexpectedly satisfied the production trace"
        );
    }
}
