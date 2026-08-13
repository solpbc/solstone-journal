// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_top::{
    ProcessObserver, ProcessSample, RestartEnqueueResult, RestartIdSource, SessionRestartIds,
    TerminalOwner, TerminalOwnerError, TerminalSyscalls, TopBrainSource, TopClock, TopInput,
    TopReceiveTransport, TopRestartTransport, TopState, TopTerminal,
    run_top_with_outer_panic_cleanup,
};

#[derive(Default)]
struct RecordingSyscalls {
    calls: Vec<&'static str>,
    stdin_tty: bool,
    stdout_tty: bool,
    fail_apply_raw: bool,
    fail_width: bool,
    fail_screen_entry: bool,
}

impl RecordingSyscalls {
    fn ready() -> Self {
        Self {
            stdin_tty: true,
            stdout_tty: true,
            ..Self::default()
        }
    }
}

impl TerminalSyscalls for RecordingSyscalls {
    type Saved = u8;

    fn stdin_is_tty(&mut self) -> bool {
        self.calls.push("stdin-tty");
        self.stdin_tty
    }

    fn stdout_is_tty(&mut self) -> bool {
        self.calls.push("stdout-tty");
        self.stdout_tty
    }

    fn capture_stdin(&mut self) -> Result<Self::Saved, String> {
        self.calls.push("capture");
        Ok(1)
    }

    fn raw_mode(&mut self, _: &Self::Saved) -> Self::Saved {
        2
    }

    fn apply_stdin(&mut self, settings: &Self::Saved) -> Result<(), String> {
        self.calls
            .push(if *settings == 1 { "restore" } else { "apply" });
        (*settings != 2 || !self.fail_apply_raw)
            .then_some(())
            .ok_or_else(|| "apply".to_owned())
    }

    fn stdout_width(&mut self) -> Result<usize, String> {
        self.calls.push("width");
        (!self.fail_width)
            .then_some(80)
            .ok_or_else(|| "width".to_owned())
    }

    fn write_stdout(&mut self, bytes: &str) -> Result<(), String> {
        let entering = bytes == "\x1b[?1049h\x1b[?25l";
        self.calls.push(if entering {
            "enter-screen"
        } else {
            "leave-screen"
        });
        (!entering || !self.fail_screen_entry)
            .then_some(())
            .ok_or_else(|| "screen".to_owned())
    }
}

#[test]
fn terminal_owner_orders_descriptor_operations_and_restores_once() {
    let mut owner = TerminalOwner::new(RecordingSyscalls::ready());
    owner.enter().unwrap();
    owner.restore_once().unwrap();
    owner.restore_once().unwrap();
    assert_eq!(
        owner.syscalls().calls,
        [
            "stdin-tty",
            "stdout-tty",
            "capture",
            "apply",
            "width",
            "enter-screen",
            "restore",
            "leave-screen",
        ]
    );
}

#[test]
fn terminal_owner_conservatively_restores_after_screen_write_failure() {
    let mut owner = TerminalOwner::new(RecordingSyscalls {
        fail_screen_entry: true,
        ..RecordingSyscalls::ready()
    });
    assert!(matches!(owner.enter(), Err(TerminalOwnerError::Screen(_))));
    assert_eq!(
        owner.syscalls().calls,
        [
            "stdin-tty",
            "stdout-tty",
            "capture",
            "apply",
            "width",
            "enter-screen",
            "restore",
            "leave-screen",
        ]
    );
}

#[test]
fn terminal_owner_classifies_no_tty_and_partial_apply() {
    let mut stdin = TerminalOwner::new(RecordingSyscalls::default());
    assert_eq!(stdin.enter(), Err(TerminalOwnerError::StdinNotTty));

    let mut stdout = TerminalOwner::new(RecordingSyscalls {
        stdin_tty: true,
        ..RecordingSyscalls::default()
    });
    assert_eq!(stdout.enter(), Err(TerminalOwnerError::StdoutNotTty));

    let mut partial = TerminalOwner::new(RecordingSyscalls {
        fail_width: true,
        ..RecordingSyscalls::ready()
    });
    assert!(matches!(partial.enter(), Err(TerminalOwnerError::Width(_))));
    assert!(partial.cleanup_diagnostics().is_empty());
    assert_eq!(
        partial.syscalls().calls,
        [
            "stdin-tty",
            "stdout-tty",
            "capture",
            "apply",
            "width",
            "restore"
        ]
    );
}

struct PanicTerminal {
    entered: bool,
    restores: usize,
}
impl TopTerminal for PanicTerminal {
    fn enter(&mut self) -> Result<(), String> {
        self.entered = true;
        Ok(())
    }
    fn restore(&mut self) -> Result<(), String> {
        self.restores += 1;
        Ok(())
    }
    fn width(&mut self) -> Result<usize, String> {
        panic!("post-entry panic")
    }
    fn render(&mut self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn input(&mut self, _: f64) -> Result<TopInput, String> {
        Ok(TopInput::Quit)
    }
}
struct Receive {
    stops: usize,
}
impl TopReceiveTransport for Receive {
    fn start(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn next(&mut self) -> Result<Option<solstone_core_callosum::CallosumReceiveEvent>, String> {
        Ok(None)
    }
    fn stop(&mut self) -> Result<(), String> {
        self.stops += 1;
        Ok(())
    }
}
struct Clock;
impl TopClock for Clock {
    fn wall_seconds(&self) -> f64 {
        0.0
    }
    fn monotonic_seconds(&self) -> f64 {
        0.0
    }
    fn datetime(&self) -> serde_json::Value {
        serde_json::json!({"datetime":"x"})
    }
    fn sleep(&mut self, _: f64) -> Result<(), String> {
        Ok(())
    }
}
struct Observer;
impl ProcessObserver for Observer {
    fn sample(&mut self, _: u32, _: f64) -> ProcessSample {
        ProcessSample::Missing
    }
}
struct Restart(SessionRestartIds);
impl TopRestartTransport for Restart {
    fn emit_restart(&mut self, _: &str, _: &str) -> RestartEnqueueResult {
        RestartEnqueueResult::Enqueued
    }
    fn current_generation(&self) -> u64 {
        0
    }
    fn current_epoch(&self) -> u64 {
        0
    }
    fn restart_ids(&mut self) -> &mut dyn RestartIdSource {
        &mut self.0
    }
}
struct Brain;
impl TopBrainSource for Brain {
    fn inspect(&mut self) -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({"lines":["ok"]}))
    }
}

#[test]
fn production_outer_panic_boundary_restores_and_stops_once() {
    let mut terminal = PanicTerminal {
        entered: false,
        restores: 0,
    };
    let mut receive = Receive { stops: 0 };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = run_top_with_outer_panic_cleanup(
            &mut TopState::default(),
            &mut Clock,
            &mut terminal,
            &mut receive,
            &mut Observer,
            &mut Restart(SessionRestartIds::with_nonce(1, [0; 16])),
            &mut Brain,
        );
    }));
    assert!(outcome.is_err());
    assert!(terminal.entered);
    assert_eq!(terminal.restores, 1);
    assert_eq!(receive.stops, 1);
}

#[test]
fn production_terminal_has_no_process_or_helper_reach() {
    let source = include_str!("../src/production.rs");
    for forbidden in ["std::process", "Command::new", "\"stty\""] {
        assert!(
            !source.contains(forbidden),
            "forbidden terminal reach: {forbidden}"
        );
    }
}
