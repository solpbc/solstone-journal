// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_callosum::CallosumReceiveEvent;
use solstone_core_system_health::sanitize_for_terminal;

use crate::{
    BrainHealthState, FrameSample, ProcessObserver, ReductionSample, TopRenderOp, TopState,
    apply_receive_event, cleanup_processes, render_ops,
};

/// Input accepted by the native top loop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TopInput {
    Up,
    Down,
    Quit,
    Interrupt,
    EndOfFile,
    None,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum TopLoopError {
    #[error("terminal failure: {0}")]
    Terminal(String),
    #[error("transport failure: {0}")]
    Transport(String),
}

impl TopLoopError {
    fn with_cleanup(self, diagnostics: Vec<String>) -> Self {
        if diagnostics.is_empty() {
            return self;
        }
        let diagnostics = diagnostics.join("; ");
        match self {
            Self::Terminal(primary) => Self::Terminal(format!("{primary}; cleanup: {diagnostics}")),
            Self::Transport(primary) => {
                Self::Transport(format!("{primary}; cleanup: {diagnostics}"))
            }
        }
    }
}

/// Fully injected terminal seam; no test needs a TTY.
pub trait TopTerminal {
    fn enter(&mut self) -> Result<(), String>;
    fn restore(&mut self) -> Result<(), String>;
    fn width(&mut self) -> Result<usize, String>;
    fn render(&mut self, ops: &[TopRenderOp]) -> Result<(), String>;
    fn input(&mut self, timeout_seconds: f64) -> Result<TopInput, String>;
}
pub trait TopReceiveTransport {
    fn start(&mut self) -> Result<(), String>;
    fn next(&mut self) -> Result<Option<CallosumReceiveEvent>, String>;
    fn stop(&mut self) -> Result<(), String>;
}
pub trait TopClock {
    fn wall_seconds(&self) -> f64;
    fn monotonic_seconds(&self) -> f64;
    fn datetime(&self) -> serde_json::Value;
    fn sleep(&mut self, seconds: f64) -> Result<(), String>;
}
/// Read-only brain projection seam, refreshed without a live Python process.
pub trait TopBrainSource {
    fn inspect(&mut self) -> Result<serde_json::Value, String>;
}

/// Run one deterministic event-loop session, cleaning up only acquired
/// resources. Cleanup is best effort and cannot overwrite the primary error.
pub fn run_top_with(
    state: &mut TopState,
    clock: &mut dyn TopClock,
    terminal: &mut dyn TopTerminal,
    receive: &mut dyn TopReceiveTransport,
    observer: &mut dyn ProcessObserver,
    brain: &mut dyn TopBrainSource,
) -> Result<(), TopLoopError> {
    receive.start().map_err(TopLoopError::Transport)?;
    let mut entered = false;
    let mut last_cleanup = clock.monotonic_seconds();
    let outcome: Result<(), TopLoopError> = (|| {
        terminal.enter().map_err(TopLoopError::Terminal)?;
        entered = true;
        state.brain_health_state = BrainHealthState::Checking;
        refresh_brain(
            state,
            brain,
            clock.wall_seconds(),
            clock.monotonic_seconds(),
        );
        loop {
            let frame_wall = clock.wall_seconds();
            let frame_monotonic = clock.monotonic_seconds();
            let frame_datetime = clock.datetime();
            while let Some(event) = receive.next().map_err(TopLoopError::Transport)? {
                let sample = ReductionSample {
                    wall_seconds: frame_wall,
                    monotonic_seconds: frame_monotonic,
                    wall_datetime: frame_datetime.clone(),
                };
                let effects = apply_receive_event(state, &event, &sample, observer);
                if effects.refresh_brain {
                    refresh_brain(state, brain, sample.wall_seconds, sample.monotonic_seconds);
                }
            }
            let width = terminal.width().map_err(TopLoopError::Terminal)?;
            let ops = render_ops(
                state,
                FrameSample {
                    wall_seconds: frame_wall,
                    monotonic_seconds: frame_monotonic,
                },
                width,
            );
            terminal.render(&ops).map_err(TopLoopError::Terminal)?;
            match terminal.input(0.2).map_err(TopLoopError::Terminal)? {
                TopInput::Quit | TopInput::Interrupt | TopInput::EndOfFile => break,
                TopInput::Up => state.selected = state.selected.saturating_sub(1),
                TopInput::Down => {
                    state.selected =
                        (state.selected + 1).min(state.services.len().saturating_sub(1))
                }
                TopInput::None => {}
            }
            let now = clock.monotonic_seconds();
            if now - last_cleanup >= 5.0 {
                cleanup_processes(
                    state,
                    &ReductionSample {
                        wall_seconds: clock.wall_seconds(),
                        monotonic_seconds: now,
                        wall_datetime: clock.datetime(),
                    },
                    observer,
                );
                last_cleanup = now;
            }
            if now - state.brain_health_state.observed_at_monotonic() >= 30.0 {
                refresh_brain(state, brain, frame_wall, now);
            }
            clock.sleep(0.1).map_err(TopLoopError::Transport)?;
        }
        Ok(())
    })();
    let restore = entered
        .then(|| terminal.restore())
        .transpose()
        .err()
        .map(|error| format!("terminal restore: {error}"));
    let stop = receive
        .stop()
        .err()
        .map(|error| format!("transport stop: {error}"));
    let cleanup = [restore, stop].into_iter().flatten().collect::<Vec<_>>();
    match outcome {
        Err(error) => Err(error.with_cleanup(cleanup)),
        Ok(()) if cleanup.is_empty() => Ok(()),
        Ok(()) => Err(TopLoopError::Terminal(format!(
            "cleanup: {}",
            cleanup.join("; ")
        ))),
    }
}

fn refresh_brain(
    state: &mut TopState,
    brain: &mut dyn TopBrainSource,
    wall_seconds: f64,
    monotonic_seconds: f64,
) {
    match brain.inspect() {
        Ok(health) => {
            state.brain_health = Some(health);
            state.brain_health_ts = wall_seconds;
            state.brain_health_state = BrainHealthState::Available {
                observed_at_monotonic: monotonic_seconds,
            };
        }
        Err(error) => {
            state.brain_health = None;
            state.brain_health_state = BrainHealthState::Unavailable {
                message: sanitize_for_terminal(&error),
                observed_at_monotonic: monotonic_seconds,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    struct Clock;
    impl TopClock for Clock {
        fn wall_seconds(&self) -> f64 {
            0.
        }
        fn monotonic_seconds(&self) -> f64 {
            0.
        }
        fn datetime(&self) -> serde_json::Value {
            serde_json::json!({"datetime":"x"})
        }
        fn sleep(&mut self, _: f64) -> Result<(), String> {
            Ok(())
        }
    }
    #[derive(Default)]
    struct Terminal {
        entered: bool,
        restored: bool,
        input: VecDeque<TopInput>,
    }
    impl TopTerminal for Terminal {
        fn enter(&mut self) -> Result<(), String> {
            self.entered = true;
            Ok(())
        }
        fn restore(&mut self) -> Result<(), String> {
            self.restored = true;
            Ok(())
        }
        fn width(&mut self) -> Result<usize, String> {
            Ok(80)
        }
        fn render(&mut self, _: &[TopRenderOp]) -> Result<(), String> {
            Ok(())
        }
        fn input(&mut self, _: f64) -> Result<TopInput, String> {
            Ok(self.input.pop_front().unwrap_or(TopInput::Quit))
        }
    }
    #[derive(Default)]
    struct Receive {
        started: bool,
        stopped: bool,
    }
    impl TopReceiveTransport for Receive {
        fn start(&mut self) -> Result<(), String> {
            self.started = true;
            Ok(())
        }
        fn next(&mut self) -> Result<Option<CallosumReceiveEvent>, String> {
            Ok(None)
        }
        fn stop(&mut self) -> Result<(), String> {
            self.stopped = true;
            Ok(())
        }
    }
    struct Observer;
    impl ProcessObserver for Observer {
        fn sample(&mut self, _: u32, _: f64) -> crate::ProcessSample {
            crate::ProcessSample::Missing
        }
    }
    struct Brain;
    impl TopBrainSource for Brain {
        fn inspect(&mut self) -> Result<serde_json::Value, String> {
            Ok(serde_json::json!({"headline":"ok"}))
        }
    }
    #[test]
    fn cleanup_occurs_after_acquisition() {
        let mut state = TopState::default();
        let mut clock = Clock;
        let mut terminal = Terminal {
            input: VecDeque::from([TopInput::Quit]),
            ..Terminal::default()
        };
        let mut receive = Receive::default();
        assert!(
            run_top_with(
                &mut state,
                &mut clock,
                &mut terminal,
                &mut receive,
                &mut Observer,
                &mut Brain
            )
            .is_ok()
        );
        assert!(terminal.entered && terminal.restored && receive.started && receive.stopped);
    }

    #[derive(Default)]
    struct FailTerminal {
        enter: bool,
        fail_render_at: Option<usize>,
        render_calls: usize,
        fail_width_at: Option<usize>,
        width_calls: usize,
        fail_input: Option<&'static str>,
        fail_restore: bool,
        input_none: bool,
        inputs: VecDeque<TopInput>,
        restored: bool,
    }
    impl TopTerminal for FailTerminal {
        fn enter(&mut self) -> Result<(), String> {
            if self.enter {
                Err("enter".into())
            } else {
                Ok(())
            }
        }
        fn restore(&mut self) -> Result<(), String> {
            self.restored = true;
            if self.fail_restore {
                Err("restore".into())
            } else {
                Ok(())
            }
        }
        fn width(&mut self) -> Result<usize, String> {
            self.width_calls += 1;
            if self.fail_width_at == Some(self.width_calls) {
                Err("width".into())
            } else {
                Ok(80)
            }
        }
        fn render(&mut self, _: &[TopRenderOp]) -> Result<(), String> {
            self.render_calls += 1;
            if self.fail_render_at == Some(self.render_calls) {
                Err("render".into())
            } else {
                Ok(())
            }
        }
        fn input(&mut self, _: f64) -> Result<TopInput, String> {
            if let Some(message) = self.fail_input {
                Err(message.into())
            } else if let Some(input) = self.inputs.pop_front() {
                Ok(input)
            } else if self.input_none {
                Ok(TopInput::None)
            } else {
                Ok(TopInput::Quit)
            }
        }
    }
    #[derive(Default)]
    struct FailReceive {
        fail_start: bool,
        fail_next: bool,
        fail_stop: bool,
        stopped: bool,
        events: VecDeque<CallosumReceiveEvent>,
    }
    impl TopReceiveTransport for FailReceive {
        fn start(&mut self) -> Result<(), String> {
            (!self.fail_start)
                .then_some(())
                .ok_or_else(|| "start".into())
        }
        fn next(&mut self) -> Result<Option<CallosumReceiveEvent>, String> {
            if self.fail_next {
                Err("next".into())
            } else {
                Ok(self.events.pop_front())
            }
        }
        fn stop(&mut self) -> Result<(), String> {
            self.stopped = true;
            (!self.fail_stop).then_some(()).ok_or_else(|| "stop".into())
        }
    }
    struct FailClock {
        fail_sleep: bool,
        now: f64,
        sleep_calls: usize,
    }
    impl TopClock for FailClock {
        fn wall_seconds(&self) -> f64 {
            self.now
        }
        fn monotonic_seconds(&self) -> f64 {
            self.now
        }
        fn datetime(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        fn sleep(&mut self, _: f64) -> Result<(), String> {
            if self.fail_sleep {
                Err("sleep".into())
            } else {
                self.sleep_calls += 1;
                self.now += 5.0;
                Ok(())
            }
        }
    }
    struct FailBrain {
        fail: bool,
    }
    impl TopBrainSource for FailBrain {
        fn inspect(&mut self) -> Result<serde_json::Value, String> {
            (!self.fail)
                .then_some(serde_json::json!({}))
                .ok_or_else(|| "brain".into())
        }
    }

    #[test]
    fn retained_loop_failure_shapes_acquire_and_cleanup_correctly() {
        for name in [
            "normal-periodic",
            "normal-q",
            "normal-ctrl-c",
            "normal-ctrl-d",
            "key-up",
            "key-down",
            "event-success",
            "context-error",
            "initial-render-error",
            "input-error",
            "input-poll-error",
            "input-read-error",
            "event-error",
            "cleanup-error",
            "later-render-error",
            "initial-width-error",
            "later-width-error",
            "sleep-error",
            "stop-error",
        ] {
            let mut state = TopState::default();
            if name == "normal-periodic" {
                state.running_tasks.insert(
                    "periodic".into(),
                    serde_json::json!({"name":"periodic", "pid":99}),
                );
            }
            if matches!(name, "key-up" | "key-down") {
                state.services = vec![
                    serde_json::json!({"name":"one", "pid":101, "ref":"one", "uptime_seconds":1}),
                    serde_json::json!({"name":"two", "pid":201, "ref":"two", "uptime_seconds":1}),
                ];
                state.selected = usize::from(name == "key-up");
            }
            let mut clock = FailClock {
                fail_sleep: name == "sleep-error",
                now: 0.0,
                sleep_calls: 0,
            };
            let mut terminal = FailTerminal {
                fail_render_at: match name {
                    "initial-render-error" => Some(1),
                    "later-render-error" => Some(2),
                    _ => None,
                },
                fail_width_at: match name {
                    "initial-width-error" => Some(1),
                    "later-width-error" => Some(2),
                    _ => None,
                },
                fail_input: match name {
                    "input-error" => Some("input"),
                    "input-poll-error" => Some("poll-sentinel"),
                    "input-read-error" => Some("read-sentinel"),
                    _ => None,
                },
                fail_restore: name == "cleanup-error",
                input_none: name == "sleep-error",
                inputs: match name {
                    "normal-periodic" => {
                        VecDeque::from([TopInput::None, TopInput::None, TopInput::Quit])
                    }
                    "later-render-error" | "later-width-error" => {
                        VecDeque::from([TopInput::None, TopInput::Quit])
                    }
                    "key-up" => VecDeque::from([TopInput::Up, TopInput::Quit]),
                    "key-down" => VecDeque::from([TopInput::Down, TopInput::Quit]),
                    _ => VecDeque::new(),
                },
                ..FailTerminal::default()
            };
            let mut receive = FailReceive {
                fail_start: name == "context-error",
                fail_next: name == "event-error",
                fail_stop: name == "stop-error",
                events: if name == "event-success" {
                    VecDeque::from([
                        CallosumReceiveEvent::Continuity {
                            generation: 1,
                            epoch: 1,
                            phase: solstone_core_callosum::CallosumConnectionPhase::Connected,
                        },
                        CallosumReceiveEvent::Envelope {
                            generation: 1,
                            epoch: 1,
                            envelope: solstone_core_callosum::CallosumEnvelope {
                                tract: "supervisor".into(),
                                event: "queue".into(),
                                ts: None,
                                extra: serde_json::Map::from_iter([
                                    ("command".into(), serde_json::json!("health")),
                                    ("queued".into(), serde_json::json!(2)),
                                ]),
                            },
                        },
                    ])
                } else {
                    VecDeque::new()
                },
                ..FailReceive::default()
            };
            let mut brain = FailBrain { fail: false };
            let result = run_top_with(
                &mut state,
                &mut clock,
                &mut terminal,
                &mut receive,
                &mut Observer,
                &mut brain,
            );
            if name == "context-error" {
                assert!(!terminal.restored, "{name}");
                assert!(!receive.stopped, "{name}");
            } else {
                assert!(terminal.restored, "{name}");
                assert!(receive.stopped, "{name}");
            }
            if matches!(
                name,
                "normal-periodic"
                    | "normal-q"
                    | "normal-ctrl-c"
                    | "normal-ctrl-d"
                    | "key-up"
                    | "key-down"
                    | "event-success"
            ) {
                assert!(result.is_ok(), "{name}: {result:?}");
            } else {
                assert!(result.is_err(), "{name}");
            }
            match name {
                "input-poll-error" => assert!(
                    result
                        .as_ref()
                        .unwrap_err()
                        .to_string()
                        .contains("poll-sentinel"),
                    "{name}: {result:?}"
                ),
                "input-read-error" => assert!(
                    result
                        .as_ref()
                        .unwrap_err()
                        .to_string()
                        .contains("read-sentinel"),
                    "{name}: {result:?}"
                ),
                "key-up" => assert_eq!(state.selected, 0, "{name}"),
                "key-down" => assert_eq!(state.selected, 1, "{name}"),
                "event-success" => assert_eq!(
                    state.command_queues.get("health"),
                    Some(&serde_json::json!(2))
                ),
                "normal-periodic" => {
                    assert!(clock.sleep_calls >= 2, "{name}");
                    assert!(state.finished_tasks.contains_key("periodic"), "{name}");
                }
                _ => {}
            }
        }
    }

    #[test]
    fn down_clamps_at_the_last_service() {
        let mut state = TopState {
            services: vec![
                serde_json::json!({"name":"one"}),
                serde_json::json!({"name":"two"}),
            ],
            selected: 1,
            ..TopState::default()
        };
        let mut clock = Clock;
        let mut terminal = Terminal {
            input: VecDeque::from([TopInput::Down, TopInput::Quit]),
            ..Terminal::default()
        };
        let mut receive = Receive::default();
        run_top_with(
            &mut state,
            &mut clock,
            &mut terminal,
            &mut receive,
            &mut Observer,
            &mut Brain,
        )
        .unwrap();
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn resize_is_observed_by_re_rendering_with_current_width() {
        struct ResizeTerminal {
            widths: VecDeque<usize>,
            renders: usize,
        }
        impl TopTerminal for ResizeTerminal {
            fn enter(&mut self) -> Result<(), String> {
                Ok(())
            }
            fn restore(&mut self) -> Result<(), String> {
                Ok(())
            }
            fn width(&mut self) -> Result<usize, String> {
                Ok(self.widths.pop_front().unwrap_or(120))
            }
            fn render(&mut self, _: &[TopRenderOp]) -> Result<(), String> {
                self.renders += 1;
                Ok(())
            }
            fn input(&mut self, _: f64) -> Result<TopInput, String> {
                if self.renders < 2 {
                    Ok(TopInput::None)
                } else {
                    Ok(TopInput::Quit)
                }
            }
        }
        let mut terminal = ResizeTerminal {
            widths: VecDeque::from([40, 120]),
            renders: 0,
        };
        let mut state = TopState::default();
        let mut clock = Clock;
        let mut receive = Receive::default();
        let mut brain = Brain;
        run_top_with(
            &mut state,
            &mut clock,
            &mut terminal,
            &mut receive,
            &mut Observer,
            &mut brain,
        )
        .unwrap();
        assert_eq!(terminal.renders, 2);
    }
}
