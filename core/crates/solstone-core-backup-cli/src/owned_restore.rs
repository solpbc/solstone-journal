// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::sync::Arc;

use solstone_core_backup_runtime::DirectReadFinishState;
use solstone_core_offload::cli_restore::{
    CliRestoreSettlement, OwnedRestoreCliRun, run_cli_restore,
};

use super::{CliRun, ProductionClock, offload_restore_json, offload_restore_result, render_json};

/// The complete native outcome remains alive while output is borrowed.
#[must_use = "Retain the whole restore result through printing and exit"]
pub struct OwnedRestoreCliOutcome {
    run: OwnedRestoreCliRun,
    output: CliRun,
}

impl std::fmt::Debug for OwnedRestoreCliOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnedRestoreCliOutcome")
            .field("run", &self.run)
            .field("exit_code", &self.output.exit_code)
            .finish()
    }
}

impl OwnedRestoreCliOutcome {
    pub fn run(&self) -> &OwnedRestoreCliRun {
        &self.run
    }

    pub fn output(&self) -> (&str, &str, i32) {
        (
            &self.output.stdout,
            &self.output.stderr,
            self.output.exit_code,
        )
    }
}

pub(super) fn run(journal: &Path, day: Option<&str>, json_output: bool) -> OwnedRestoreCliOutcome {
    let run = run_cli_restore(journal, Arc::new(ProductionClock), day);
    let output = render(&run, json_output);
    OwnedRestoreCliOutcome { run, output }
}

fn settlement(run: &OwnedRestoreCliRun) -> &'static str {
    match run.settlement() {
        CliRestoreSettlement::NotStarted => "not_started",
        CliRestoreSettlement::Settled => "settled",
        CliRestoreSettlement::Refused => "refused",
        CliRestoreSettlement::Held => "held",
        CliRestoreSettlement::Unknown => "unknown",
    }
}

fn runtime(run: &OwnedRestoreCliRun) -> Option<&'static str> {
    run.runtime().map(|state| match state {
        DirectReadFinishState::Completed => "completed",
        DirectReadFinishState::Held => "held",
        DirectReadFinishState::Unknown => "unknown",
    })
}

fn render(run: &OwnedRestoreCliRun, json_output: bool) -> CliRun {
    let mut output = if json_output {
        let mut value = offload_restore_json(run.result());
        value["settlement"] = settlement(run).into();
        value["runtime"] = runtime(run).into();
        render_json(Ok::<_, String>(value))
    } else {
        let mut output = offload_restore_result(run.result().clone(), false);
        output.stdout.push_str(&format!(
            "settlement={} runtime={}\n",
            settlement(run),
            runtime(run).unwrap_or("none")
        ));
        output
    };
    let unfinished = matches!(
        run.settlement(),
        CliRestoreSettlement::Held | CliRestoreSettlement::Unknown
    ) || run
        .runtime()
        .is_some_and(|state| state != DirectReadFinishState::Completed);
    let succeeded = run.completed()
        || (run.settlement() == CliRestoreSettlement::NotStarted && run.result().status == "no_op");
    output.exit_code = i32::from(!succeeded);
    if unfinished {
        output.stderr.push_str(&super::copy::text(
            "journal.cli.backup.offload.restore.unfinished",
        ));
        output.stderr.push('\n');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CliOutcome, owned_restore_options, run_cli};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn well_formed_restore_selects_the_owned_path_on_supported_platforms() {
        for (given, expected) in [
            (vec!["offload", "restore", "--all"], Some((None, false))),
            (
                vec!["--verbose", "offload", "restore", "20261009", "--json"],
                Some((Some("20261009".to_owned()), true)),
            ),
            (
                vec!["offload", "restore", "--", "20261009"],
                Some((Some("20261009".to_owned()), false)),
            ),
        ] {
            let expected = expected.filter(|_| {
                cfg!(any(
                    target_os = "linux",
                    target_os = "macos",
                    target_os = "windows"
                ))
            });
            assert_eq!(owned_restore_options(&args(&given)), expected, "{given:?}");
        }
        for given in [
            vec!["offload", "restore"],
            vec!["offload", "restore", "--all", "20261009"],
            vec!["offload", "restore", "20260230"],
            vec!["offload", "restore", "--help"],
            vec!["offload", "restore", "--wat"],
            vec!["offload", "restore", "20261009", "20261010"],
        ] {
            assert_eq!(owned_restore_options(&args(&given)), None, "{given:?}");
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    #[test]
    fn nothing_offloaded_succeeds_and_a_missing_journal_refuses() {
        let journal = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(journal.path().join("chronicle")).unwrap();
        let run = run_cli(&args(&["offload", "restore", "--all"]), journal.path());
        let CliOutcome::Restore(owned) = &run else {
            panic!("whole restore outcome required")
        };
        assert_eq!(owned.run().settlement(), CliRestoreSettlement::NotStarted);
        let (stdout, stderr, code) = run.output();
        assert_eq!(code, 0, "{stdout}{stderr}");
        assert!(stdout.contains("status=no_op"), "{stdout}");
        assert!(
            stdout.contains("settlement=not_started runtime=none"),
            "{stdout}"
        );
        assert!(stderr.is_empty());

        let missing = journal.path().join("missing");
        let run = run_cli(
            &args(&["offload", "restore", "20261009", "--json"]),
            &missing,
        );
        let (stdout, _, code) = run.output();
        assert_eq!(code, 1);
        let value: serde_json::Value = serde_json::from_str(stdout).unwrap();
        assert_eq!(value["status"], "refused");
        assert_eq!(value["settlement"], "not_started");
        assert!(value["runtime"].is_null());
        assert!(!missing.exists());
    }

    #[test]
    fn whole_cli_result_is_send_static() {
        fn closed<T: Send + 'static>() {}
        closed::<OwnedRestoreCliOutcome>();
    }
}
