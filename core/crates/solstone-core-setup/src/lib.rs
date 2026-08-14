// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Foundational types for the native `journal setup` port.

use std::env;
use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
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
use clean_uninstall::{
    CleanUninstallContext, clean_uninstall_confirmation_lines, clean_uninstall_has_managed_paths,
    clean_uninstall_refusal, run_clean_uninstall,
};
use events::{EventSink, JsonlEmitter};
use manifest::manifest_path;
use solstone_core_journal::{detect_checkout_root, resolve_installation_root_from_executable_dir};
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
    print!("proceed? [y/N]: ");
    let _ = io::stdout().flush();
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
    seams: Seams,
) -> ExitCode {
    let current_dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let stdin_is_tty = io::stdin().is_terminal();
    let stdout_is_tty = io::stdout().is_terminal();
    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    run_owner_setup_with_io(
        args,
        home_dir,
        executable_dir,
        current_dir,
        stdin_is_tty,
        stdout_is_tty,
        seams,
        &mut stdout,
        &mut stderr,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_owner_setup_with_io<W: Write, E: Write>(
    args: SetupArgs,
    home_dir: PathBuf,
    executable_dir: PathBuf,
    current_dir: PathBuf,
    stdin_is_tty: bool,
    stdout_is_tty: bool,
    mut seams: Seams,
    stdout: &mut W,
    stderr: &mut E,
) -> ExitCode {
    let project_root = resolve_installation_root_from_executable_dir(&executable_dir)
        .unwrap_or_else(|| executable_dir.clone());
    let is_source_checkout = detect_checkout_root(&project_root).is_some();
    let resolution = ResolutionContext {
        home_dir: home_dir.clone(),
        current_dir: current_dir.clone(),
        journal_env: env::var("SOLSTONE_JOURNAL").ok(),
        journal_variant_env: env::var("JOURNAL_VARIANT").ok(),
        is_source_checkout,
    };
    if args.clean_uninstall {
        if let Some(message) = clean_uninstall_refusal(&args) {
            let _ = writeln!(stderr, "{message}");
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
            stdin_is_tty,
            confirm: seams.confirm_clean_uninstall.as_mut(),
            runner: seams.runner.as_mut(),
        };
        if !clean.yes && clean.stdin_is_tty && clean_uninstall_has_managed_paths(&clean) {
            for line in clean_uninstall_confirmation_lines(&clean) {
                let _ = writeln!(stdout, "{line}");
            }
        }
        let outcome = run_clean_uninstall(&mut clean);
        // This is the one irreversible path in the verb: it removes the
        // service unit, both managed wrappers, the owner's user config and the
        // manifest inside their journal. A bare count tells an owner that
        // something was skipped or failed without telling them WHICH artifact
        // or WHERE -- and the skip is the case that matters most, because it
        // is how an owner-authored alias survives. Narrate each step.
        let total = outcome.results.len();
        for (index, result) in outcome.results.iter().enumerate() {
            let step = index + 1;
            let _ = writeln!(
                stdout,
                "[step {step}/{total}] running {} uninstall...",
                result.name
            );
            let detail = match (&result.path, &result.reason) {
                (_, Some(reason)) => reason.clone(),
                (Some(path), None) => path.display().to_string(),
                (None, None) => String::new(),
            };
            if detail.is_empty() {
                let _ = writeln!(
                    stdout,
                    "[step {step}/{total}] {} {}",
                    result.state.as_str(),
                    result.name
                );
            } else {
                let _ = writeln!(
                    stdout,
                    "[step {step}/{total}] {} {}: {detail}",
                    result.state.as_str(),
                    result.name
                );
            }
        }
        let _ = writeln!(stdout, "{}", outcome.message);
        return ExitCode::from(outcome.exit_code as u8);
    }
    let resolved = resolve_setup(&args, &resolution);
    let mode = resolve_mode(&args, stdin_is_tty, stdout_is_tty);
    if !args.jsonl && resolved.should_short_circuit() {
        let plan_context = SetupContext {
            args: &args,
            resolved: &resolved,
            mode,
            home_dir: home_dir.clone(),
            config_path: config_path(&home_dir),
            journal_path: resolved.journal_path.clone(),
            current_dir: resolution.current_dir.clone(),
            project_root: project_root.clone(),
            install_bin_dir: executable_dir.clone(),
            manifest_path: manifest_path(&resolved.journal_path),
            stdin_is_tty,
            stdout_is_tty,
            now: utc_now,
            runner: seams.runner.as_mut(),
            prompt: seams.prompt.as_mut(),
            events: None,
            wrapper_backup_dir: None,
            service_ops: seams.service_ops.as_mut(),
            already_keeps_journal_probe: seams.already_keeps_journal_probe,
            is_macos: cfg!(target_os = "macos"),
            check_report_builder: seams.check_report_builder.as_ref(),
        };
        for line in render_plan(&plan_context, args.dry_run) {
            let _ = writeln!(stdout, "{line}");
        }
    }
    let outcome = {
        let mut jsonl = args.jsonl.then(|| JsonlEmitter::new(&mut *stdout));
        let mut context = SetupContext {
            args: &args,
            resolved: &resolved,
            mode,
            home_dir: home_dir.clone(),
            config_path: config_path(&home_dir),
            journal_path: resolved.journal_path.clone(),
            current_dir: resolution.current_dir.clone(),
            project_root,
            install_bin_dir: executable_dir,
            manifest_path: manifest_path(&resolved.journal_path),
            stdin_is_tty,
            stdout_is_tty,
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
        run_setup(&mut context, &step_specs())
    };
    if let Some(dead_end) = outcome.dead_end {
        if args.jsonl
            && let (Some(step_name), Some(error_code)) = (dead_end.step_name, dead_end.error_code)
        {
            let mut emitter = JsonlEmitter::new(&mut *stdout);
            let _ = emitter.emit(
                events::EventType::StepFailed,
                &utc_now(),
                serde_json::Map::from_iter([
                    ("step".into(), serde_json::json!(step_name.as_str())),
                    ("duration_ms".into(), serde_json::json!(0)),
                    (
                        "error".into(),
                        serde_json::json!({
                            "code": error_code,
                            "message": dead_end.message,
                            "details": "",
                            "exit_code": outcome.exit_code,
                        }),
                    ),
                ]),
            );
            let _ = emitter.emit(
                events::EventType::SetupCompleted,
                &utc_now(),
                serde_json::Map::from_iter([
                    ("status".into(), serde_json::json!("failed")),
                    ("failed_step".into(), serde_json::json!(step_name.as_str())),
                    ("duration_ms".into(), serde_json::json!(outcome.duration_ms)),
                ]),
            );
        } else {
            let _ = writeln!(stderr, "{}", dead_end.message);
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::parse_args_at;
    use crate::steps::{CommandOutput, CommandRequest};
    use std::collections::VecDeque;
    use std::fs;
    use std::path::Path;

    struct Runner(VecDeque<CommandOutput>);
    impl CommandRunner for Runner {
        fn run(&mut self, _request: &CommandRequest) -> Result<CommandOutput, String> {
            Ok(self.0.pop_front().unwrap_or(CommandOutput {
                exit_code: 0,
                stdout: "{}".into(),
                stderr: String::new(),
                timed_out: false,
            }))
        }
    }

    struct Service;
    impl ServiceOps for Service {
        fn is_installed(
            &mut self,
            _runner: &mut dyn CommandRunner,
            _journal: &Path,
        ) -> Result<bool, String> {
            Ok(false)
        }
        fn health_check(
            &mut self,
            _runner: &mut dyn CommandRunner,
            _journal: &Path,
        ) -> Result<bool, String> {
            Ok(false)
        }
        fn restart(
            &mut self,
            _runner: &mut dyn CommandRunner,
            _journal: &Path,
        ) -> Result<(), String> {
            Ok(())
        }
        fn up(&mut self, _runner: &mut dyn CommandRunner, _journal: &Path) -> Result<i32, String> {
            Ok(0)
        }
    }

    struct Check;
    impl CheckReportBuilder for Check {
        fn local_provider_blocked(&self, _journal: &Path) -> bool {
            false
        }
    }

    struct Prompt;
    impl ExistingJournalPrompt for Prompt {
        fn accept_existing_journal(&mut self, _path: &Path) -> Result<bool, String> {
            Ok(false)
        }
    }

    fn no_probe(_context: &SetupContext<'_>) -> Result<bool, String> {
        Ok(false)
    }

    fn seams(outputs: Vec<CommandOutput>) -> Seams {
        Seams {
            runner: Box::new(Runner(outputs.into())),
            service_ops: Box::new(Service),
            check_report_builder: Box::new(Check),
            already_keeps_journal_probe: no_probe,
            prompt: Box::new(Prompt),
            confirm_clean_uninstall: Box::new(|| true),
        }
    }

    fn root(name: &str) -> PathBuf {
        let root = PathBuf::from("/var/tmp").join(format!(
            "solstone-core-setup-lib-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn parsed(values: &[String], current_dir: &Path) -> SetupArgs {
        parse_args_at(
            &values
                .iter()
                .cloned()
                .map(OsString::from)
                .collect::<Vec<_>>(),
            current_dir,
        )
        .unwrap()
    }

    #[test]
    fn owner_boundary_prints_dead_end_or_emits_terminal_jsonl_events() {
        let root = root("dead-end");
        let home = root.join("home");
        let cwd = root.join("cwd");
        fs::create_dir_all(&cwd).unwrap();
        let journal = root.join("journal-file");
        fs::write(&journal, "not a directory").unwrap();
        let doctor = CommandOutput {
            exit_code: 0,
            stdout: "{}".into(),
            stderr: String::new(),
            timed_out: false,
        };
        let args = parsed(
            &[
                "--yes".into(),
                "--journal".into(),
                journal.display().to_string(),
            ],
            &cwd,
        );
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = run_owner_setup_with_io(
            args,
            home.clone(),
            root.join("bin"),
            cwd.clone(),
            false,
            false,
            seams(vec![doctor]),
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, ExitCode::from(2));
        assert_eq!(
            String::from_utf8(stderr).unwrap(),
            format!(
                "expected a directory at {}; got a regular file. Re-run with --journal <other-path>.\n",
                journal.display()
            )
        );

        let jsonl_doctor = CommandOutput {
            exit_code: 0,
            stdout: concat!(
                "{\"event\":\"doctor.started\"}\n",
                "{\"event\":\"check.completed\"}\n",
                "{\"event\":\"doctor.completed\",\"status\":\"ok\"}\n"
            )
            .into(),
            stderr: String::new(),
            timed_out: false,
        };
        let args = parsed(
            &[
                "--yes".into(),
                "--jsonl".into(),
                "--journal".into(),
                journal.display().to_string(),
            ],
            &cwd,
        );
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = run_owner_setup_with_io(
            args,
            home,
            root.join("bin"),
            cwd,
            false,
            false,
            seams(vec![jsonl_doctor]),
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, ExitCode::from(2));
        assert!(stderr.is_empty());
        let events = String::from_utf8(stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        let tail = &events[events.len() - 2..];
        assert_eq!(tail[0]["event"], "step.failed");
        assert_eq!(tail[0]["step"], "journal");
        assert_eq!(tail[0]["duration_ms"], 0);
        assert_eq!(tail[0]["error"]["code"], "journal_dir_invalid");
        assert_eq!(tail[1]["event"], "setup.completed");
        assert_eq!(tail[1]["status"], "failed");
        assert_eq!(tail[1]["failed_step"], "journal");
    }

    #[test]
    fn wrapper_worktree_detection_uses_executable_install_root_not_caller_directory() {
        let root = root("worktree-root");
        let executable_dir = root.join(".venv/bin");
        let home = root.join("home");
        let caller_dir = root.join("unrelated-caller");
        fs::create_dir_all(&executable_dir).unwrap();
        fs::create_dir_all(&caller_dir).unwrap();
        fs::create_dir_all(root.join("solstone")).unwrap();
        fs::write(
            root.join("pyproject.toml"),
            "[project]\nname = \"fixture\"\n",
        )
        .unwrap();
        fs::write(root.join(".git"), "gitdir: elsewhere\n").unwrap();
        let args = parsed(
            &[
                "--yes".into(),
                "--skip-models".into(),
                "--skip-skills".into(),
                "--skip-service".into(),
            ],
            &caller_dir,
        );
        let doctor = CommandOutput {
            exit_code: 0,
            stdout: "{}".into(),
            stderr: String::new(),
            timed_out: false,
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = run_owner_setup_with_io(
            args,
            home.clone(),
            executable_dir,
            caller_dir,
            false,
            false,
            seams(vec![doctor]),
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, ExitCode::SUCCESS);
        assert!(!home.join(".local/bin/sol").exists());
        assert!(!home.join(".local/bin/journal").exists());
    }

    /// Clean-uninstall is the one irreversible path in the verb, and a bare
    /// count cannot tell an owner WHICH artifact was skipped or failed. The
    /// skip line in particular is how an owner-authored alias announces that
    /// it survived, so it is the one that must reach them.
    #[test]
    fn clean_uninstall_narrates_every_step_not_only_the_count() {
        let root = root("clean-narration");
        let executable_dir = root.join(".venv/bin");
        let home = root.join("home");
        fs::create_dir_all(&executable_dir).unwrap();
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        fs::write(home.join(".local/bin/sol"), "#!/bin/sh\necho owner\n").unwrap();
        let args = parsed(&["--clean-uninstall".into(), "--yes".into()], &root);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        // The journal resolves from the sandboxed HOME rather than from an
        // environment variable: `set_var` is process-global and racy under the
        // default test runner, and this assertion is about narration, not
        // about which journal was chosen.
        let _ = run_owner_setup_with_io(
            args,
            home.clone(),
            executable_dir,
            root.clone(),
            false,
            false,
            seams(vec![CommandOutput {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
            }]),
            &mut stdout,
            &mut stderr,
        );
        let text = String::from_utf8(stdout).unwrap();
        for step in 1..=4 {
            assert!(
                text.contains(&format!("[step {step}/4] running ")),
                "step {step} header missing from:\n{text}"
            );
        }
        assert!(
            text.contains("skipped wrapper: alias is not a managed symlink, not removing"),
            "the owner-authored alias must announce that it survived:\n{text}"
        );
        assert!(
            text.contains("clean uninstall complete:"),
            "summary missing:\n{text}"
        );
        assert!(
            home.join(".local/bin/sol").exists(),
            "an owner-authored alias must never be removed"
        );
    }
}
