// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What the app shows for the journal's run state, read from
//! `journal service __app-status` and from whether the journal answers on its
//! loopback port.

use serde::{Deserialize, Serialize};

pub const APP_STATUS_SCHEMA: &str = "solstone-journal-app-status-v1";

/// One reading of `journal service __app-status`.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ServiceStatus {
    pub schema: String,
    /// `None` until a journal is set up on this PC.
    pub journal: Option<String>,
    pub installed: bool,
    /// The owner's run intent, recorded on the scheduled task: a stop clears it.
    pub wants_running: bool,
    /// The scheduled task has a running instance.
    pub running: bool,
    /// The journal's supervisor reports ready.
    pub ready: bool,
    pub starts_at_sign_in: bool,
    pub port: Option<u16>,
}

impl ServiceStatus {
    /// Parse the last JSON line the command printed.
    pub fn parse(stdout: &str) -> Result<Self, String> {
        let line = stdout
            .lines()
            .rev()
            .find(|line| line.trim_start().starts_with('{'))
            .ok_or_else(|| "the journal reported no status".to_owned())?;
        let status: Self = serde_json::from_str(line).map_err(|error| error.to_string())?;
        if status.schema != APP_STATUS_SCHEMA {
            return Err(format!("unexpected status schema {}", status.schema));
        }
        Ok(status)
    }

    pub fn is_set_up(&self) -> bool {
        self.journal.is_some() && self.installed
    }
}

/// The run state the window shows, in the Mac app's words where they apply.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunDisplay {
    Starting,
    Running,
    Stopped,
    NotRunning,
}

/// The run state from one reading. An action the window has in flight
/// (starting, stopping) outranks this, and the window tracks that itself.
#[must_use]
pub fn run_display(status: &ServiceStatus, answering: bool) -> RunDisplay {
    if answering && status.ready {
        RunDisplay::Running
    } else if !status.wants_running {
        RunDisplay::Stopped
    } else if status.running {
        // The task runs and the journal has not answered yet: it is coming up.
        RunDisplay::Starting
    } else {
        RunDisplay::NotRunning
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(wants_running: bool, running: bool, ready: bool) -> ServiceStatus {
        ServiceStatus {
            schema: APP_STATUS_SCHEMA.to_owned(),
            journal: Some(r"C:\Users\owner\journal".to_owned()),
            installed: true,
            wants_running,
            running,
            ready,
            starts_at_sign_in: true,
            port: Some(5015),
        }
    }

    #[test]
    fn a_ready_journal_that_answers_is_running() {
        assert_eq!(
            run_display(&status(true, true, true), true),
            RunDisplay::Running
        );
    }

    #[test]
    fn a_ready_marker_without_an_answer_is_not_yet_running() {
        assert_eq!(
            run_display(&status(true, true, true), false),
            RunDisplay::Starting
        );
    }

    #[test]
    fn the_owners_stop_reads_as_stopped_and_a_dead_resident_does_not() {
        assert_eq!(
            run_display(&status(false, false, false), false),
            RunDisplay::Stopped
        );
        assert_eq!(
            run_display(&status(true, false, false), false),
            RunDisplay::NotRunning
        );
    }

    #[test]
    fn parses_the_last_json_line_and_refuses_another_schema() {
        let parsed = ServiceStatus::parse(
            "noise\n{\"schema\":\"solstone-journal-app-status-v1\",\"journal\":null,\"installed\":false,\"wants_running\":false,\"running\":false,\"ready\":false,\"starts_at_sign_in\":false,\"port\":null}\n",
        )
        .unwrap();
        assert!(!parsed.is_set_up());
        assert!(ServiceStatus::parse("{\"schema\":\"other\"}").is_err());
        assert!(ServiceStatus::parse("").is_err());
    }
}
