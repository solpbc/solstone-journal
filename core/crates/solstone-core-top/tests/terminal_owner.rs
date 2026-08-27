// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_top::{
    ProcessObserver, ProcessSample, TerminalOwner, TerminalOwnerError, TerminalSyscalls,
    TopBrainSource, TopClock, TopInput, TopReceiveTransport, TopRenderOp, TopState, TopTerminal,
    run_top_with_outer_panic_cleanup,
};

#[derive(Default)]
struct RecordingSyscalls {
    calls: Vec<&'static str>,
    stdin_tty: bool,
    stdout_tty: bool,
    fail_enable_raw: bool,
    fail_enter_alt: bool,
    fail_hide_cursor: bool,
    fail_width: bool,
    fail_reset_style: bool,
    fail_reset_attributes: bool,
    fail_show_cursor: bool,
    fail_leave_alt: bool,
    fail_disable_raw: bool,
    fail_write_ops: bool,
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
    fn stdin_is_tty(&mut self) -> bool {
        self.calls.push("stdin-tty");
        self.stdin_tty
    }

    fn stdout_is_tty(&mut self) -> bool {
        self.calls.push("stdout-tty");
        self.stdout_tty
    }

    fn enable_raw_mode(&mut self) -> Result<(), String> {
        self.calls.push("enable-raw");
        (!self.fail_enable_raw)
            .then_some(())
            .ok_or_else(|| "raw".to_owned())
    }

    fn disable_raw_mode(&mut self) -> Result<(), String> {
        self.calls.push("disable-raw");
        (!self.fail_disable_raw)
            .then_some(())
            .ok_or_else(|| "disable-raw".to_owned())
    }

    fn enter_alt_screen(&mut self) -> Result<(), String> {
        self.calls.push("enter-alt");
        (!self.fail_enter_alt)
            .then_some(())
            .ok_or_else(|| "alt".to_owned())
    }

    fn leave_alt_screen(&mut self) -> Result<(), String> {
        self.calls.push("leave-alt");
        (!self.fail_leave_alt)
            .then_some(())
            .ok_or_else(|| "leave-alt".to_owned())
    }

    fn hide_cursor(&mut self) -> Result<(), String> {
        self.calls.push("hide-cursor");
        (!self.fail_hide_cursor)
            .then_some(())
            .ok_or_else(|| "hide".to_owned())
    }

    fn show_cursor(&mut self) -> Result<(), String> {
        self.calls.push("show-cursor");
        (!self.fail_show_cursor)
            .then_some(())
            .ok_or_else(|| "show".to_owned())
    }

    fn reset_style(&mut self) -> Result<(), String> {
        self.calls.push("reset-style");
        (!self.fail_reset_style)
            .then_some(())
            .ok_or_else(|| "reset".to_owned())
    }

    fn reset_attributes(&mut self) -> Result<(), String> {
        self.calls.push("reset-attributes");
        (!self.fail_reset_attributes)
            .then_some(())
            .ok_or_else(|| "attributes".to_owned())
    }

    fn stdout_width(&mut self) -> Result<usize, String> {
        self.calls.push("width");
        (!self.fail_width)
            .then_some(80)
            .ok_or_else(|| "width".to_owned())
    }

    fn write_ops(&mut self, _: &[TopRenderOp]) -> Result<(), String> {
        self.calls.push("write-ops");
        (!self.fail_write_ops)
            .then_some(())
            .ok_or_else(|| "write".to_owned())
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
            "enable-raw",
            "enter-alt",
            "hide-cursor",
            "width",
            "reset-attributes",
            "reset-style",
            "show-cursor",
            "leave-alt",
            "disable-raw",
        ]
    );
}

#[test]
fn terminal_owner_classifies_no_tty_before_any_mutation() {
    let mut stdin = TerminalOwner::new(RecordingSyscalls::default());
    assert_eq!(stdin.enter(), Err(TerminalOwnerError::StdinNotTty));
    assert_eq!(stdin.syscalls().calls, ["stdin-tty"]);

    let mut stdout = TerminalOwner::new(RecordingSyscalls {
        stdin_tty: true,
        ..RecordingSyscalls::default()
    });
    assert_eq!(stdout.enter(), Err(TerminalOwnerError::StdoutNotTty));
    assert_eq!(stdout.syscalls().calls, ["stdin-tty", "stdout-tty"]);
}

#[test]
fn terminal_owner_cleans_up_exactly_acquired_raw_mode_on_alt_screen_failure() {
    let mut owner = TerminalOwner::new(RecordingSyscalls {
        fail_enter_alt: true,
        ..RecordingSyscalls::ready()
    });
    let error = owner.enter().unwrap_err();
    assert!(matches!(error, TerminalOwnerError::Screen(_)), "{error:?}");
    assert!(
        !error.to_string().contains("cleanup:"),
        "unused teardown must not invent cleanup diagnostics: {error}"
    );
    assert_eq!(
        owner.syscalls().calls,
        [
            "stdin-tty",
            "stdout-tty",
            "enable-raw",
            "enter-alt",
            "reset-attributes",
            "reset-style",
            "leave-alt",
            "disable-raw",
        ]
    );
}

#[test]
fn terminal_owner_cleans_up_raw_and_alt_on_cursor_hide_failure() {
    let mut owner = TerminalOwner::new(RecordingSyscalls {
        fail_hide_cursor: true,
        ..RecordingSyscalls::ready()
    });
    assert!(matches!(owner.enter(), Err(TerminalOwnerError::Screen(_))));
    assert_eq!(
        owner.syscalls().calls,
        [
            "stdin-tty",
            "stdout-tty",
            "enable-raw",
            "enter-alt",
            "hide-cursor",
            "reset-attributes",
            "reset-style",
            "show-cursor",
            "leave-alt",
            "disable-raw",
        ]
    );
}

#[test]
fn terminal_owner_cleans_up_full_acquisition_on_width_failure() {
    let mut owner = TerminalOwner::new(RecordingSyscalls {
        fail_width: true,
        ..RecordingSyscalls::ready()
    });
    assert!(matches!(owner.enter(), Err(TerminalOwnerError::Width(_))));
    assert!(owner.cleanup_diagnostics().is_empty());
    assert_eq!(
        owner.syscalls().calls,
        [
            "stdin-tty",
            "stdout-tty",
            "enable-raw",
            "enter-alt",
            "hide-cursor",
            "width",
            "reset-attributes",
            "reset-style",
            "show-cursor",
            "leave-alt",
            "disable-raw",
        ]
    );
}

#[test]
fn terminal_owner_preserves_primary_error_when_cleanup_also_fails() {
    let mut owner = TerminalOwner::new(RecordingSyscalls {
        fail_width: true,
        fail_reset_style: true,
        fail_disable_raw: true,
        ..RecordingSyscalls::ready()
    });
    let error = owner.enter().unwrap_err();
    assert!(matches!(error, TerminalOwnerError::Width(_)), "{error:?}");
    let message = error.to_string();
    assert!(
        message.starts_with("terminal width failed: width"),
        "{message}"
    );
    assert!(
        message.contains("cleanup: restore style: reset"),
        "{message}"
    );
    assert!(
        message.contains("restore raw mode: disable-raw"),
        "{message}"
    );
    assert_eq!(
        owner.syscalls().calls,
        [
            "stdin-tty",
            "stdout-tty",
            "enable-raw",
            "enter-alt",
            "hide-cursor",
            "width",
            "reset-attributes",
            "reset-style",
            "show-cursor",
            "leave-alt",
            "disable-raw",
        ]
    );
}

#[test]
fn terminal_owner_raw_mode_failure_inverts_the_attempted_raw_mode() {
    let mut owner = TerminalOwner::new(RecordingSyscalls {
        fail_enable_raw: true,
        ..RecordingSyscalls::ready()
    });
    let error = owner.enter().unwrap_err();
    assert!(matches!(error, TerminalOwnerError::Apply(_)), "{error:?}");
    assert_eq!(
        owner.syscalls().calls,
        ["stdin-tty", "stdout-tty", "enable-raw", "disable-raw"]
    );
}

#[test]
fn terminal_owner_write_ops_failure_is_primary_and_restore_still_runs() {
    let mut owner = TerminalOwner::new(RecordingSyscalls {
        fail_write_ops: true,
        ..RecordingSyscalls::ready()
    });
    owner.enter().unwrap();
    assert_eq!(
        owner.write_ops(&[TopRenderOp::Print("payload".to_owned())]),
        Err("write".to_owned())
    );
    owner.restore_once().unwrap();
    assert_eq!(
        owner.syscalls().calls,
        [
            "stdin-tty",
            "stdout-tty",
            "enable-raw",
            "enter-alt",
            "hide-cursor",
            "width",
            "write-ops",
            "reset-attributes",
            "reset-style",
            "show-cursor",
            "leave-alt",
            "disable-raw",
        ]
    );
}

#[test]
fn terminal_owner_teardown_continues_after_a_single_step_failure() {
    let mut owner = TerminalOwner::new(RecordingSyscalls {
        fail_show_cursor: true,
        ..RecordingSyscalls::ready()
    });
    owner.enter().unwrap();
    let error = owner.restore_once().unwrap_err();
    assert_eq!(error, "restore cursor: show");
    assert_eq!(
        owner.syscalls().calls,
        [
            "stdin-tty",
            "stdout-tty",
            "enable-raw",
            "enter-alt",
            "hide-cursor",
            "width",
            "reset-attributes",
            "reset-style",
            "show-cursor",
            "leave-alt",
            "disable-raw",
        ]
    );
}

#[test]
fn terminal_owner_teardown_continues_after_attribute_reset_failure() {
    let mut owner = TerminalOwner::new(RecordingSyscalls {
        fail_reset_attributes: true,
        ..RecordingSyscalls::ready()
    });
    owner.enter().unwrap();
    let error = owner.restore_once().unwrap_err();
    assert_eq!(error, "restore attributes: attributes");
    assert_eq!(
        owner.syscalls().calls,
        [
            "stdin-tty",
            "stdout-tty",
            "enable-raw",
            "enter-alt",
            "hide-cursor",
            "width",
            "reset-attributes",
            "reset-style",
            "show-cursor",
            "leave-alt",
            "disable-raw",
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
    fn render(&mut self, _: &[TopRenderOp]) -> Result<(), String> {
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
