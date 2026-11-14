// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owned process boundary for whole-day phases without cooperative cancellation.

#[cfg(any(not(test), all(feature = "full-tests", unix)))]
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(any(not(test), all(feature = "full-tests", unix)))]
use std::thread;
use std::time::{Duration, Instant};

#[cfg(any(not(test), all(feature = "full-tests", unix)))]
use solstone_core_system::process::{CAP_TERMINATION_TIMEOUT, ManagedProcess, SpawnOptions};

pub(crate) enum PhaseProcessOutcome {
    Exited(i32),
    TimedOut {
        cleanup_error: Option<String>,
    },
    #[cfg(any(not(test), all(feature = "full-tests", unix)))]
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

/// In-process indexer and journal-stats phases for a hosted think context.
///
/// The desktop runner stays [`NativePhaseProcessRunner`]. This one does not spawn.
pub(crate) struct HostedPhaseRunner {
    pub(crate) index: Arc<dyn crate::context::IndexBoundary>,
    pub(crate) talent_root: PathBuf,
    pub(crate) apps_root: PathBuf,
    pub(crate) cancel_checkpoint: Arc<dyn Fn() -> bool + Send + Sync>,
}

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

impl PhaseProcessRunner for HostedPhaseRunner {
    fn run(
        &self,
        phase: &'static str,
        command: Vec<String>,
        journal: &Path,
        day: &str,
        timeout: Duration,
    ) -> PhaseProcessOutcome {
        let _ = (command, day);
        let deadline = Instant::now() + timeout;
        let admit = || {
            if (self.cancel_checkpoint)() {
                Err("cancelled".to_owned())
            } else if Instant::now() >= deadline {
                Err("phase deadline".to_owned())
            } else {
                Ok(())
            }
        };
        if admit().is_err() {
            return PhaseProcessOutcome::Exited(1);
        }
        match phase {
            "indexer" | "indexer_inputs" => {
                let checkpoint = || admit();
                match self.index.drain_pending(
                    journal,
                    phase == "indexer_inputs",
                    timeout,
                    &checkpoint,
                ) {
                    Ok(()) => PhaseProcessOutcome::Exited(0),
                    Err(_) => PhaseProcessOutcome::Exited(1),
                }
            }
            "journal_stats" => {
                let backlog = solstone_core_journal_stats_cli::FilesystemBacklogViewReader;
                let run = solstone_core_journal_stats_cli::run_cli_with_cache_checked(
                    &[],
                    journal,
                    chrono::Utc::now(),
                    &self.talent_root,
                    &self.apps_root,
                    &backlog,
                    &solstone_core_journal_stats_cli::FilesystemDocumentWriter,
                    &solstone_core_journal_stats_cli::FilesystemDayCacheWriter,
                    &admit,
                );
                if run.exit_code == 0 {
                    PhaseProcessOutcome::Exited(0)
                } else {
                    PhaseProcessOutcome::Exited(1)
                }
            }
            _ => PhaseProcessOutcome::Exited(1),
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

#[cfg(any(not(test), all(feature = "full-tests", unix)))]
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
mod hosted_tests {
    use super::*;

    struct TestHostIndex;
    impl crate::context::IndexBoundary for TestHostIndex {
        fn enqueue_path(&self, journal: &Path, path: &Path) -> Result<(), String> {
            solstone_core_indexer_store::todo::enqueue_path(journal, path)
                .map_err(|error| error.to_string())
        }
        fn drain_pending(
            &self,
            journal: &Path,
            force: bool,
            budget: Duration,
            checkpoint: &dyn Fn() -> Result<(), String>,
        ) -> Result<(), String> {
            let report = solstone_core_indexer_store::todo::drain_with_checkpoint(
                journal,
                solstone_core_indexer_store::todo::DrainOptions { force, budget },
                checkpoint,
            )
            .map_err(|error| error.to_string())?;
            if report.timed_out || report.failed != 0 {
                Err("pending".into())
            } else {
                Ok(())
            }
        }
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn hosted_stats_stop_after_native_staging_preserves_output() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path().to_path_buf();
        let destination = root.join("stats.json");
        let before = b"{\"sentinel\":\"existing\"}\n";
        std::fs::write(&destination, before).unwrap();
        std::fs::create_dir(root.join("tokens")).unwrap();
        std::fs::write(
            root.join("tokens/20260105.jsonl"),
            "{\"timestamp\":1,\"model\":\"model\",\"usage\":{\"input\":3}}\n",
        )
        .unwrap();
        let staged = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&staged);
        let watched = root.clone();
        let hosted = HostedPhaseRunner {
            index: Arc::new(TestHostIndex),
            talent_root: root.join("talents"),
            apps_root: root.join("apps"),
            cancel_checkpoint: Arc::new(move || {
                if observed.load(Ordering::Acquire) {
                    return true;
                }
                if std::fs::read_dir(&watched).unwrap().any(|entry| {
                    solstone_core_journal_io::is_publication_candidate_name(
                        &entry.unwrap().file_name(),
                    )
                }) {
                    observed.store(true, Ordering::Release);
                    return true;
                }
                false
            }),
        };
        let outcome = hosted.run(
            "journal_stats",
            Vec::new(),
            &root,
            "20260105",
            Duration::from_secs(10),
        );
        assert!(
            staged.load(Ordering::Acquire),
            "actual HostedPhaseRunner must reach the native stage"
        );
        assert!(matches!(outcome, PhaseProcessOutcome::Exited(1)));
        assert_eq!(std::fs::read(&destination).unwrap(), before);

        let admitted = HostedPhaseRunner {
            index: Arc::new(TestHostIndex),
            talent_root: root.join("talents"),
            apps_root: root.join("apps"),
            cancel_checkpoint: Arc::new(|| false),
        };
        let outcome = admitted.run(
            "journal_stats",
            Vec::new(),
            &root,
            "20260105",
            Duration::from_secs(10),
        );
        assert!(matches!(outcome, PhaseProcessOutcome::Exited(0)));
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(destination).unwrap()).unwrap();
        assert_eq!(saved["tokens"]["by_day"]["20260105"]["model"]["input"], 3);
    }

    fn runner(cancel: bool) -> HostedPhaseRunner {
        let root = PathBuf::from("payload");
        HostedPhaseRunner {
            index: Arc::new(TestHostIndex),
            talent_root: root.join("talent"),
            apps_root: root.join("apps"),
            cancel_checkpoint: Arc::new(move || cancel),
        }
    }

    #[test]
    fn cancelled_hosted_phases_do_not_write() {
        let journal = tempfile::tempdir().expect("journal");
        let runner = runner(true);
        for phase in ["indexer", "journal_stats"] {
            let outcome = runner.run(
                phase,
                Vec::new(),
                journal.path(),
                "20261018",
                Duration::from_secs(1),
            );
            assert!(matches!(outcome, PhaseProcessOutcome::Exited(1)));
        }
        assert!(!journal.path().join("indexer").exists());
        assert!(!journal.path().join("stats.json").exists());
        assert!(
            !journal
                .path()
                .join("chronicle")
                .read_dir()
                .map(|mut entries| entries.next().is_some())
                .unwrap_or(false)
        );
    }

    #[test]
    fn an_empty_hosted_drain_succeeds_without_creating_sqlite() {
        let journal = tempfile::tempdir().expect("journal");
        let outcome = runner(false).run(
            "indexer",
            Vec::new(),
            journal.path(),
            "20261018",
            Duration::from_secs(1),
        );
        assert!(matches!(outcome, PhaseProcessOutcome::Exited(0)));
        assert!(!solstone_core_indexer_store::db::db_path(journal.path()).exists());
    }

    #[test]
    fn open_hosted_journal_stats_writes_stats_json() {
        let journal = tempfile::tempdir().expect("journal");
        let outcome = runner(false).run(
            "journal_stats",
            Vec::new(),
            journal.path(),
            "20261018",
            Duration::from_secs(1),
        );
        assert!(matches!(outcome, PhaseProcessOutcome::Exited(0)));
        assert!(journal.path().join("stats.json").is_file());
    }

    #[test]
    fn unknown_hosted_phase_fails_closed() {
        let journal = tempfile::tempdir().expect("journal");
        let outcome = runner(false).run(
            "sense_batch",
            Vec::new(),
            journal.path(),
            "20261018",
            Duration::from_secs(1),
        );
        assert!(matches!(outcome, PhaseProcessOutcome::Exited(1)));
    }
}

// These drive real `/bin/sh` children, so `run_owned_process` is compiled under test
// only where they run.
#[cfg(all(test, feature = "full-tests", unix))]
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
