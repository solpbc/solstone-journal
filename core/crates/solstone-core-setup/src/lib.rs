// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Foundational types for the native `journal setup` port.

use std::env;
use std::ffi::OsString;
use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::process::ExitCode;

pub mod args;
pub mod clean_uninstall;
pub mod events;
pub mod manifest;
pub mod steps;
pub mod user_config;
pub mod wrapper;

use args::{
    ResolutionContext, SetupArgs, resolve_clean_uninstall_journal, resolve_mode, resolve_setup,
};
use clean_uninstall::{CleanUninstallContext, clean_uninstall_refusal, run_clean_uninstall};
use events::{EventSink, JsonlEmitter};
use manifest::manifest_path;
use steps::{
    CheckReportBuilder, CommandRunner, ExistingJournalPrompt, NativeCheckReportBuilder,
    NativeServiceOps, ProcessCommandRunner, ServiceOps, SetupContext,
    native_already_keeps_journal_probe, render_plan, run_setup, step_specs,
};
use user_config::config_path;

pub struct Seams {
    pub runner: Box<dyn CommandRunner>,
    pub service_ops: Box<dyn ServiceOps>,
    pub check_report_builder: Box<dyn CheckReportBuilder>,
    pub already_keeps_journal_probe: fn(&SetupContext<'_>) -> Result<bool, String>,
    pub prompt: Box<dyn ExistingJournalPrompt>,
    pub confirm_clean_uninstall: Box<dyn FnMut() -> bool>,
}

struct TerminalPrompt;
impl ExistingJournalPrompt for TerminalPrompt {
    fn accept_existing_journal(&mut self, path: &std::path::Path) -> Result<bool, String> {
        eprint!(
            "{} already contains journal data; proceed? [y/N]: ",
            path.display()
        );
        let mut answer = String::new();
        io::stdin()
            .read_line(&mut answer)
            .map_err(|error| error.to_string())?;
        Ok(matches!(
            answer.trim().to_ascii_lowercase().as_str(),
            "y" | "yes"
        ))
    }
}

fn terminal_confirm() -> bool {
    eprint!("proceed? [y/N]: ");
    let mut answer = String::new();
    io::stdin().read_line(&mut answer).is_ok()
        && matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

fn utc_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn run_owner_setup(
    args: SetupArgs,
    home_dir: PathBuf,
    executable_dir: PathBuf,
    mut seams: Seams,
) -> ExitCode {
    let current_dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let resolution = ResolutionContext {
        home_dir: home_dir.clone(),
        current_dir: current_dir.clone(),
        journal_env: env::var("SOLSTONE_JOURNAL").ok(),
        journal_variant_env: env::var("SOLSTONE_JOURNAL_VARIANT").ok(),
        is_source_checkout: false,
    };
    if args.clean_uninstall {
        if let Some(message) = clean_uninstall_refusal(&args) {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
        let journal = resolve_clean_uninstall_journal(&resolution);
        let mut clean = CleanUninstallContext {
            journal_path: journal.clone(),
            home_dir: home_dir.clone(),
            config_path: config_path(&home_dir),
            manifest_path: manifest_path(&journal),
            curdir: current_dir,
            executable_dir,
            yes: args.yes,
            stdin_is_tty: io::stdin().is_terminal(),
            confirm: seams.confirm_clean_uninstall.as_mut(),
            runner: seams.runner.as_mut(),
        };
        let outcome = run_clean_uninstall(&mut clean);
        println!("{}", outcome.message);
        return ExitCode::from(outcome.exit_code as u8);
    }
    let resolved = resolve_setup(&args, &resolution);
    let mode = resolve_mode(&args, io::stdin().is_terminal(), io::stdout().is_terminal());
    let mut jsonl = args.jsonl.then(|| JsonlEmitter::new(io::stdout()));
    let mut context = SetupContext {
        args: &args,
        resolved: &resolved,
        mode,
        home_dir: home_dir.clone(),
        config_path: config_path(&home_dir),
        journal_path: resolved.journal_path.clone(),
        current_dir: resolution.current_dir.clone(),
        project_root: resolution.current_dir,
        install_bin_dir: executable_dir,
        manifest_path: manifest_path(&resolved.journal_path),
        stdin_is_tty: io::stdin().is_terminal(),
        stdout_is_tty: io::stdout().is_terminal(),
        now: utc_now,
        runner: seams.runner.as_mut(),
        prompt: seams.prompt.as_mut(),
        events: jsonl.as_mut().map(|emitter| emitter as &mut dyn EventSink),
        wrapper_backup_dir: None,
        service_ops: seams.service_ops.as_mut(),
        already_keeps_journal_probe: seams.already_keeps_journal_probe,
        is_macos: cfg!(target_os = "macos"),
        check_report_builder: seams.check_report_builder.as_ref(),
    };
    if !args.jsonl && resolved.should_short_circuit() {
        for line in render_plan(&context, args.dry_run) {
            println!("{line}");
        }
    }
    let outcome = run_setup(&mut context, &step_specs());
    ExitCode::from(outcome.exit_code as u8)
}

pub fn run_owner_args(
    argv: &[OsString],
    home_dir: PathBuf,
    executable_dir: PathBuf,
    seams: Seams,
) -> ExitCode {
    match args::parse_args_at(
        argv,
        &env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    ) {
        Ok(args) => run_owner_setup(args, home_dir, executable_dir, seams),
        Err(error) => {
            eprint!("{}", args::USAGE);
            eprintln!("journal setup: error: {}", error.0);
            ExitCode::from(2)
        }
    }
}

pub fn run_owner_setup_native(args: SetupArgs) -> ExitCode {
    let home_dir = env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let executable_dir = env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    run_owner_setup(
        args,
        home_dir,
        executable_dir.clone(),
        Seams {
            runner: Box::new(ProcessCommandRunner),
            service_ops: Box::new(NativeServiceOps {
                journal_bin: executable_dir.join("journal"),
            }),
            check_report_builder: Box::new(NativeCheckReportBuilder),
            already_keeps_journal_probe: native_already_keeps_journal_probe,
            prompt: Box::new(TerminalPrompt),
            confirm_clean_uninstall: Box::new(terminal_confirm),
        },
    )
}
