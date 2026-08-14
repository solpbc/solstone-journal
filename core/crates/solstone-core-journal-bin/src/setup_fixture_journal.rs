// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Test-only native sibling used by the setup process-contract harness.

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use solstone_core_setup::steps::{
    CheckReportBuilder, ExistingJournalPrompt, NativeServiceOps, ProcessCommandRunner, SetupContext,
};
use solstone_core_setup::{Seams, run_owner_args};

struct AvailableCheck;
impl CheckReportBuilder for AvailableCheck {
    fn local_provider_blocked(&self, _journal: &Path) -> bool {
        false
    }
}
struct AcceptPrompt;
impl ExistingJournalPrompt for AcceptPrompt {
    fn accept_existing_journal(&mut self, _path: &Path) -> Result<bool, String> {
        Ok(true)
    }
}
fn no_probe(_context: &SetupContext<'_>) -> Result<bool, String> {
    Ok(false)
}

fn main() -> ExitCode {
    let argv = env::args_os().skip(1).collect::<Vec<_>>();
    if argv.first().is_some_and(|argument| argument == "setup") {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let bin_dir = env::var_os("SETUP_FIXTURE_BIN_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                env::current_exe()
                    .ok()
                    .and_then(|path| path.parent().map(Path::to_path_buf))
                    .unwrap_or_else(|| PathBuf::from("."))
            });
        return run_owner_args(
            &argv[1..],
            home,
            bin_dir.clone(),
            Seams {
                runner: Box::new(ProcessCommandRunner),
                service_ops: Box::new(NativeServiceOps {
                    journal_bin: bin_dir.join("journal"),
                }),
                check_report_builder: Box::new(AvailableCheck),
                already_keeps_journal_probe: no_probe,
                prompt: Box::new(AcceptPrompt),
                confirm_clean_uninstall: Box::new(|| true),
            },
        );
    }
    fake_sibling(&argv)
}

fn fake_sibling(argv: &[OsString]) -> ExitCode {
    if argv.first().is_some_and(|argument| argument == "doctor") {
        if argv.iter().any(|argument| argument == "--jsonl") {
            println!("{{\"event\":\"doctor.started\"}}");
            println!("{{\"event\":\"check.completed\"}}");
            println!("{{\"event\":\"doctor.completed\",\"status\":\"ok\"}}");
        } else {
            println!("{{}}");
        }
    }
    ExitCode::SUCCESS
}
