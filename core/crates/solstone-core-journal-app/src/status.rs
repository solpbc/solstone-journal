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
    /// Another account on this computer is running its journal.
    #[serde(default)]
    pub other_account_journal: bool,
}

impl ServiceStatus {
    /// Parse the last JSON line the command printed.
    pub fn parse(stdout: &str) -> Result<Self, String> {
        let line = stdout
            .lines()
            .rev()
            .find(|line| line.trim_start().starts_with('{'))
            .ok_or_else(|| crate::i18n::text("error.status.missing"))?;
        let status: Self = serde_json::from_str(line).map_err(|error| error.to_string())?;
        if status.schema != APP_STATUS_SCHEMA {
            return Err(crate::i18n::strings(
                "error.status.schema",
                &[("schema", &status.schema)],
            ));
        }
        Ok(status)
    }

    pub fn is_set_up(&self) -> bool {
        self.journal.is_some() && self.installed
    }
}

/// The journal's port from one status read, read once more when the read
/// fails. A journal that is set up reports its port whether or not it runs, so
/// a failed read is a slow or interrupted query rather than a journal that is
/// down, and one of them must not leave the window saying the journal isn't
/// answering. A read that succeeds is never repeated.
pub fn bound_port(
    mut read: impl FnMut() -> Result<ServiceStatus, String>,
    pause: impl FnOnce(),
) -> Option<u16> {
    read()
        .or_else(|_| {
            pause();
            read()
        })
        .ok()
        .and_then(|status| status.port)
}

/// The run state the window shows, in the Mac app's words where they apply.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunDisplay {
    Starting,
    Running,
    Stopped,
    NotRunning,
    OtherAccount,
}

/// The run state from one reading. An action the window has in flight
/// (starting, stopping) outranks this, and the window tracks that itself.
#[must_use]
pub fn run_display(status: &ServiceStatus, answering: bool) -> RunDisplay {
    if status.other_account_journal {
        RunDisplay::OtherAccount
    } else if answering && status.ready {
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
            other_account_journal: false,
        }
    }

    #[test]
    fn another_accounts_journal_outranks_every_other_reading() {
        let mut held = status(true, true, true);
        held.other_account_journal = true;
        assert_eq!(run_display(&held, true), RunDisplay::OtherAccount);
        assert_eq!(run_display(&held, false), RunDisplay::OtherAccount);
    }

    #[test]
    fn an_older_journal_that_does_not_say_means_no_other_account() {
        let line = format!(
            r#"{{"schema":"{APP_STATUS_SCHEMA}","journal":null,"installed":false,"wants_running":false,"running":false,"ready":false,"starts_at_sign_in":false,"port":null}}"#
        );
        assert!(!ServiceStatus::parse(&line).unwrap().other_account_journal);
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

    #[test]
    fn one_failed_status_read_is_read_again_before_the_port_is_given_up() {
        let mut reads = vec![Ok(status(true, true, true)), Err("stalled".to_owned())];
        let mut paused = false;
        let port = bound_port(|| reads.pop().unwrap(), || paused = true);
        assert_eq!(port, Some(5015));
        assert!(paused && reads.is_empty());

        let mut reads = vec![Err("stalled".to_owned()), Err("stalled".to_owned())];
        assert_eq!(bound_port(|| reads.pop().unwrap(), || {}), None);
        assert!(reads.is_empty());

        let not_set_up = ServiceStatus {
            port: None,
            ..status(false, false, false)
        };
        let mut reads = vec![Ok(not_set_up)];
        assert_eq!(bound_port(|| reads.pop().unwrap(), || panic!("no pause")), None);
    }
}
