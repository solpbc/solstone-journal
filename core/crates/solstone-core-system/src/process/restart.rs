// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::time::Duration;

/// Fixed retry delay for the temporary session-not-ready exit path.
pub const TEMPFAIL_DELAY: Duration = Duration::from_secs(15);
/// The Python scheduler's distinct no-input outcome.
pub const EXIT_EMPTY: i32 = 66;
/// The Python session-not-ready exit code.
pub const EXIT_TEMPFAIL: i32 = 75;
/// Consecutive short-uptime exits before a service is reported as struggling.
pub const STRUGGLING_THRESHOLD: usize = 5;

/// Restart backoff state for long-lived services only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RestartPolicy {
    attempts: usize,
    unsuccessful_starts: usize,
}

impl RestartPolicy {
    const SCHEDULE: [Duration; 3] = [
        Duration::from_secs(0),
        Duration::from_secs(1),
        Duration::from_secs(5),
    ];

    pub fn attempts(&self) -> usize {
        self.attempts
    }

    pub fn unsuccessful_starts(&self) -> usize {
        self.unsuccessful_starts
    }

    pub fn next_delay(&mut self) -> Duration {
        let delay = Self::SCHEDULE[self.attempts.min(Self::SCHEDULE.len() - 1)];
        self.attempts = self.attempts.saturating_add(1);
        delay
    }

    pub fn reset_attempts(&mut self) {
        self.attempts = 0;
    }

    pub fn reset_unsuccessful_starts(&mut self) {
        self.unsuccessful_starts = 0;
    }

    /// Apply the supervisor's tempfail bypass and sixty-second uptime gate.
    pub fn decide_after_exit(&mut self, exit_code: i32, uptime: Duration) -> Duration {
        if uptime >= Duration::from_secs(60) {
            self.unsuccessful_starts = 0;
        }
        if exit_code == EXIT_TEMPFAIL {
            if uptime < Duration::from_secs(60) {
                self.unsuccessful_starts = self.unsuccessful_starts.saturating_add(1);
            }
            return TEMPFAIL_DELAY;
        }
        if uptime >= Duration::from_secs(60) {
            self.reset_attempts();
            return self.next_delay();
        }
        self.unsuccessful_starts = self.unsuccessful_starts.saturating_add(1);
        self.next_delay()
    }
}

/// Render a process result using Python-compatible signal descriptions.
pub fn describe_exit(return_code: i32) -> String {
    if return_code >= 0 {
        return format!("exit {return_code}");
    }
    let signal_number = -return_code;
    #[cfg(unix)]
    {
        match nix::sys::signal::Signal::try_from(signal_number) {
            Ok(signal) => format!("exit {return_code} / {signal:?}"),
            Err(_) => format!("exit {return_code} / signal {signal_number}"),
        }
    }
    #[cfg(not(unix))]
    {
        format!("exit {return_code} / signal {signal_number}")
    }
}

/// `_record_scheduler_completion` writes this status to `health/scheduler.json`.
/// Any other exit-code mapping would break that catchup-ledger contract; timeout
/// remains caller-owned and is never an exit-code result.
pub fn exit_status_for_code(exit_code: i32) -> &'static str {
    if exit_code == 0 {
        "ok"
    } else if exit_code == EXIT_EMPTY {
        "empty"
    } else {
        "error"
    }
}
