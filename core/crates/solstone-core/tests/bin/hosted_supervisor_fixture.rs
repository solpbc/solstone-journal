// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Test-only hosted supervisor process. A real fixture is required because
//! direct-parent birth identity and parent death cannot be represented by an
//! in-process mock without losing the OS relationship under test.

use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

use solstone_core::supervisor::{SupervisorHostOutcome, run_hosted};
use solstone_core_cli::SupervisorOptions;
use solstone_core_system::lifecycle::DeclaredParent;
use solstone_core_system::process::{ProcessBirth, ProcessInstance};

fn options() -> SupervisorOptions {
    SupervisorOptions {
        port: 0,
        journal_override: None,
        no_daily: false,
        no_schedule: true,
        no_convey: false,
        no_cortex: false,
        no_spl: false,
        remote: None,
        direct_port: None,
    }
}

fn run_hosted_fixture(journal: PathBuf, outcome: PathBuf, parent: DeclaredParent) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("hosted fixture: runtime failed: {error}");
            return ExitCode::from(75);
        }
    };
    let result = runtime.block_on(run_hosted(&journal, options(), Some(parent)));
    if let Err(error) = std::fs::write(&outcome, format!("{result:?}\n")) {
        eprintln!("hosted fixture: outcome write failed: {error}");
        return ExitCode::from(75);
    }
    match result {
        SupervisorHostOutcome::OrderlyShutdown { .. }
        | SupervisorHostOutcome::ForcedShutdownAfterGraceTimeout { .. }
        | SupervisorHostOutcome::ParentLost { .. } => ExitCode::SUCCESS,
        SupervisorHostOutcome::Refused { .. } => ExitCode::from(75),
    }
}

fn run_launcher(journal: PathBuf, child_pid: PathBuf, outcome: PathBuf) -> ExitCode {
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => {
            eprintln!("hosted fixture: current executable failed: {error}");
            return ExitCode::from(75);
        }
    };
    let mut child = match Command::new(executable)
        .arg("host")
        .arg(journal)
        .arg(outcome)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            eprintln!("hosted fixture: host spawn failed: {error}");
            return ExitCode::from(75);
        }
    };
    if let Err(error) = std::fs::write(&child_pid, child.id().to_string()) {
        eprintln!("hosted fixture: child pid write failed: {error}");
        return ExitCode::from(75);
    }
    match child.wait() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::from(75),
        Err(error) => {
            eprintln!("hosted fixture: child wait failed: {error}");
            ExitCode::from(75)
        }
    }
}

fn main() -> ExitCode {
    let mut args = std::env::args_os();
    let _program = args.next();
    let Some(mode) = args.next() else {
        return ExitCode::from(64);
    };
    let Some(journal) = args.next().map(PathBuf::from) else {
        return ExitCode::from(64);
    };
    match mode.to_string_lossy().as_ref() {
        "host" => {
            let Some(outcome) = args.next().map(PathBuf::from) else {
                return ExitCode::from(64);
            };
            if args.next().is_some() {
                return ExitCode::from(64);
            }
            let parent = match DeclaredParent::capture_current() {
                Ok(parent) => parent,
                Err(error) => {
                    eprintln!("hosted fixture: parent declaration failed: {error:?}");
                    return ExitCode::from(75);
                }
            };
            run_hosted_fixture(journal, outcome, parent)
        }
        "host-with-parent" => {
            let Some(outcome) = args.next().map(PathBuf::from) else {
                return ExitCode::from(64);
            };
            let Some(pid) = args
                .next()
                .and_then(|value| value.into_string().ok())
                .and_then(|value| value.parse().ok())
            else {
                return ExitCode::from(64);
            };
            let Some(start_ticks) = args
                .next()
                .and_then(|value| value.into_string().ok())
                .and_then(|value| value.parse().ok())
            else {
                return ExitCode::from(64);
            };
            if args.next().is_some() {
                return ExitCode::from(64);
            }
            run_hosted_fixture(
                journal,
                outcome,
                DeclaredParent::from_instance(ProcessInstance {
                    pid,
                    birth: ProcessBirth::linux(start_ticks, 1, 100),
                }),
            )
        }
        "launcher" => {
            let Some(child_pid) = args.next().map(PathBuf::from) else {
                return ExitCode::from(64);
            };
            let Some(outcome) = args.next().map(PathBuf::from) else {
                return ExitCode::from(64);
            };
            if args.next().is_some() {
                return ExitCode::from(64);
            }
            run_launcher(journal, child_pid, outcome)
        }
        _ => ExitCode::from(64),
    }
}
