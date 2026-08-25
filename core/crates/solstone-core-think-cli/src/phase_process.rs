// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owned process boundary for whole-day phases without cooperative cancellation.

use std::collections::BTreeMap;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use solstone_core_system::process::{CAP_TERMINATION_TIMEOUT, ManagedProcess, SpawnOptions};

pub(crate) enum PhaseProcessOutcome {
    Exited(i32),
    TimedOut { cleanup_error: Option<String> },
    Failed(String),
}

pub(crate) trait PhaseProcessRunner {
    fn run(
        &self,
        phase: &'static str,
        command: Vec<String>,
        journal: &Path,
        day: &str,
        timeout: Duration,
    ) -> PhaseProcessOutcome;
}

pub(crate) struct NativePhaseProcessRunner;

impl PhaseProcessRunner for NativePhaseProcessRunner {
    fn run(
        &self,
        phase: &'static str,
        command: Vec<String>,
        journal: &Path,
        day: &str,
        timeout: Duration,
    ) -> PhaseProcessOutcome {
        #[cfg(not(test))]
        {
            run_owned_process(phase, command, journal, day, timeout)
        }
        #[cfg(test)]
        {
            let _ = (phase, command, journal, day, timeout);
            PhaseProcessOutcome::Exited(0)
        }
    }
}

pub(crate) fn journal_command(arguments: &[&str]) -> Result<Vec<String>, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not resolve journal executable: {error}"))?;
    let executable = executable
        .to_str()
        .ok_or_else(|| "journal executable path is not UTF-8".to_owned())?;
    Ok(std::iter::once(executable.to_owned())
        .chain(arguments.iter().map(|argument| (*argument).to_owned()))
        .collect())
}

fn run_owned_process(
    phase: &'static str,
    command: Vec<String>,
    journal: &Path,
    day: &str,
    timeout: Duration,
) -> PhaseProcessOutcome {
    let reference = format!("daily-{day}-{phase}-{}", std::process::id());
    let mut process = match ManagedProcess::spawn(
        command,
        SpawnOptions {
            journal_root: journal.to_path_buf(),
            reference,
            day: Some(day.to_owned()),
            sink: None,
            environment: BTreeMap::new(),
        },
    ) {
        Ok(process) => process,
        Err(error) => return PhaseProcessOutcome::Failed(error.to_string()),
    };
    let deadline = Instant::now() + timeout;
    loop {
        match process.poll() {
            Ok(Some(code)) => {
                process.cleanup();
                return PhaseProcessOutcome::Exited(code);
            }
            Ok(None) => {}
            Err(error) => return PhaseProcessOutcome::Failed(error.to_string()),
        }
        if Instant::now() >= deadline {
            let cleanup_error = process
                .terminate(CAP_TERMINATION_TIMEOUT)
                .err()
                .map(|error| error.to_string());
            process.cleanup();
            return PhaseProcessOutcome::TimedOut { cleanup_error };
        }
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[cfg(unix)]
    #[test]
    fn timeout_terminates_and_reaps_before_returning() {
        let journal = tempdir().unwrap();
        let late_write = journal.path().join("late-write");
        let command = vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            format!("sleep 0.2; touch '{}'", late_write.display()),
        ];

        let outcome = run_owned_process(
            "fixture",
            command,
            journal.path(),
            "20260813",
            Duration::from_millis(20),
        );

        assert!(matches!(outcome, PhaseProcessOutcome::TimedOut { .. }));
        thread::sleep(Duration::from_millis(250));
        assert!(
            !late_write.exists(),
            "terminated phase mutated after return"
        );
    }

    #[cfg(unix)]
    #[test]
    fn normal_exit_is_reported_after_reap() {
        let journal = tempdir().unwrap();
        let outcome = run_owned_process(
            "fixture",
            vec!["/bin/sh".to_owned(), "-c".to_owned(), "exit 7".to_owned()],
            journal.path(),
            "20260813",
            Duration::from_secs(1),
        );

        assert!(matches!(outcome, PhaseProcessOutcome::Exited(7)));
    }
}
