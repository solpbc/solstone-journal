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
pub mod identity_evidence;
pub mod manifest;
pub mod steps;
pub mod user_config;
pub mod wrapper;

use args::{ResolutionContext, SetupArgs, resolve_mode, resolve_setup};
use clean_uninstall::{
    CleanUninstallContext, clean_uninstall_confirmation_lines, clean_uninstall_has_managed_paths,
    clean_uninstall_refusal, run_clean_uninstall,
};
use events::{EventSink, JsonlEmitter};
use identity_evidence::gather_artifact_evidence;
use manifest::{legacy_manifest_evidence, manifest_path};
use solstone_core_installation_identity::{
    ArtifactBindingEvidence, CleanUninstallRequest, CleanUninstallSession, IdentityError,
    OwnerBase, PlatformTag, SetupAdmission, SetupAdmissionRequest, admit_clean_uninstall,
    admit_setup, journal_token_from_path, namespace_name, root_token_from_path,
};
use solstone_core_journal::{
    resolve_checkout_root_from_executable_dir, resolve_identity_root_from_executable_dir,
    resolve_installation_root_from_executable_dir,
};
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

fn identity_error_code(error: &IdentityError) -> events::ErrorCode {
    match error {
        IdentityError::Io { .. } => events::ErrorCode::InstallationIdentityUnavailable,
        _ => events::ErrorCode::InstallationIdentityRefused,
    }
}

fn report_identity_failure<W: Write>(
    jsonl: bool,
    stdout: &mut W,
    stderr: &mut impl Write,
    error: &IdentityError,
) -> ExitCode {
    let code = identity_error_code(error);
    let message = format!(
        "installation identity admission failed: {error}. Repair the managed wrapper/service artifacts or identity storage, then re-run `journal setup`."
    );
    if jsonl {
        let mut emitter = JsonlEmitter::new(stdout);
        let _ = emitter.emit(
            events::EventType::StepFailed,
            &utc_now(),
            serde_json::Map::from_iter([
                ("step".into(), serde_json::json!("identity")),
                ("duration_ms".into(), serde_json::json!(0)),
                (
                    "error".into(),
                    serde_json::json!({
                        "code": code,
                        "message": message,
                        "details": error.to_string(),
                        "exit_code": 2,
                    }),
                ),
            ]),
        );
        let _ = emitter.emit(
            events::EventType::SetupCompleted,
            &utc_now(),
            serde_json::Map::from_iter([
                ("status".into(), serde_json::json!("failed")),
                ("failed_step".into(), serde_json::json!("identity")),
                ("duration_ms".into(), serde_json::json!(0)),
            ]),
        );
    } else {
        let _ = writeln!(stderr, "{message}");
    }
    ExitCode::from(2)
}

fn resolve_identity_root(
    executable_dir: &std::path::Path,
    project_root: &std::path::Path,
) -> PathBuf {
    resolve_identity_root_from_executable_dir(executable_dir)
        .unwrap_or_else(|| project_root.to_path_buf())
}

fn admit_setup_identity(
    home_dir: &std::path::Path,
    executable_dir: &std::path::Path,
    project_root: &std::path::Path,
    resolved: &args::ResolvedSetup,
) -> Result<SetupAdmission, IdentityError> {
    let root = resolve_identity_root(executable_dir, project_root);
    let root_token = root_token_from_path(&root)?;
    let namespace = namespace_name(PlatformTag::current(), &root_token);
    let artifacts = gather_artifact_evidence(home_dir, &namespace);
    let manifest = legacy_manifest_evidence(&manifest_path(&resolved.journal_path));
    if !home_dir.exists()
        && matches!(artifacts, ArtifactBindingEvidence::Fresh)
        && matches!(
            manifest,
            solstone_core_installation_identity::LegacyManifestEvidence::Absent
        )
    {
        std::fs::create_dir_all(home_dir).map_err(|source| IdentityError::Io {
            operation: "create setup home directory",
            source,
        })?;
    }
    admit_setup(SetupAdmissionRequest {
        owner: OwnerBase::at_home(home_dir.to_path_buf(), PlatformTag::current())?,
        root_token,
        journal_token: journal_token_from_path(&resolved.journal_path)?,
        journal_is_explicit: matches!(resolved.journal_source.as_str(), "cli" | "env"),
        legacy_manifest: manifest,
        artifacts,
    })
}

fn admit_clean_identity(
    home_dir: &std::path::Path,
    executable_dir: &std::path::Path,
    project_root: &std::path::Path,
) -> Result<CleanUninstallSession, IdentityError> {
    let root = resolve_identity_root(executable_dir, project_root);
    let root_token = root_token_from_path(&root)?;
    let namespace = namespace_name(PlatformTag::current(), &root_token);
    admit_clean_uninstall(CleanUninstallRequest {
        owner: OwnerBase::at_home(home_dir.to_path_buf(), PlatformTag::current())?,
        root_token,
        artifacts: gather_artifact_evidence(home_dir, &namespace),
    })
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
    // Setup's `project_root` is a REPOSITORY root wherever there is one: the
    // wrapper step joins `.venv/bin` onto it and reads `.git` beside it to
    // recognise a worktree, and neither is true of the payload root the
    // installation resolver returns inside a checkout. Asking for the checkout
    // first restores the value this had when the two roots were the same
    // directory, in all three layouts.
    let project_root = resolve_checkout_root_from_executable_dir(&executable_dir)
        .or_else(|| resolve_installation_root_from_executable_dir(&executable_dir))
        .unwrap_or_else(|| executable_dir.clone());
    // Asked by name rather than derived from `project_root`, so this keeps
    // answering the right question if that fallback chain ever changes.
    let is_source_checkout = resolve_checkout_root_from_executable_dir(&executable_dir).is_some();
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
        let clean_session = match admit_clean_identity(&home_dir, &executable_dir, &project_root) {
            Ok(session) => session,
            Err(error) => return report_identity_failure(false, stdout, stderr, &error),
        };
        let clean_plan = clean_session.plan().clone();
        let journal = clean_plan.binding.journal_token.to_path_buf();
        let artifact_evidence = gather_artifact_evidence(&home_dir, &clean_plan.binding.namespace);
        let mut clean = CleanUninstallContext {
            journal_path: journal.clone(),
            home_dir: home_dir.clone(),
            config_path: config_path(&home_dir),
            manifest_path: manifest_path(&journal),
            plan: clean_plan,
            artifact_evidence,
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
        if outcome.exit_code == 0
            && let Err(error) = clean_session.commit_tombstone()
        {
            return report_identity_failure(false, stdout, stderr, &error);
        }
        return ExitCode::from(outcome.exit_code as u8);
    }
    let resolved = resolve_setup(&args, &resolution);
    let mode = resolve_mode(&args, stdin_is_tty, stdout_is_tty);
    let mut effective_resolved = resolved.clone();
    let admission = if resolved.should_short_circuit() {
        None
    } else {
        let admission =
            match admit_setup_identity(&home_dir, &executable_dir, &project_root, &resolved) {
                Ok(admission) => admission,
                Err(error) => return report_identity_failure(args.jsonl, stdout, stderr, &error),
            };
        let effective_journal = admission.effective_journal().to_path_buf();
        if effective_journal != effective_resolved.journal_path {
            effective_resolved.journal_path = effective_journal.clone();
            if let Some(serde_json::Value::Object(journal)) =
                effective_resolved.args_resolved.get_mut("journal")
            {
                journal.insert("value".into(), serde_json::json!(effective_journal));
            }
        }
        Some(admission)
    };
    if !args.jsonl && resolved.should_short_circuit() {
        let plan_context = SetupContext {
            args: &args,
            resolved: &effective_resolved,
            mode,
            home_dir: home_dir.clone(),
            config_path: config_path(&home_dir),
            journal_path: effective_resolved.journal_path.clone(),
            current_dir: resolution.current_dir.clone(),
            project_root: project_root.clone(),
            install_bin_dir: executable_dir.clone(),
            manifest_path: manifest_path(&effective_resolved.journal_path),
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
            installation_admission: None,
        };
        for line in render_plan(&plan_context, args.dry_run) {
            let _ = writeln!(stdout, "{line}");
        }
        let _ = writeln!(
            stdout,
            "identity admission: would run before mutating setup steps"
        );
    }
    let outcome = {
        let mut jsonl = args.jsonl.then(|| JsonlEmitter::new(&mut *stdout));
        let mut context = SetupContext {
            args: &args,
            resolved: &effective_resolved,
            mode,
            home_dir: home_dir.clone(),
            config_path: config_path(&home_dir),
            journal_path: effective_resolved.journal_path.clone(),
            current_dir: resolution.current_dir.clone(),
            project_root,
            install_bin_dir: executable_dir,
            manifest_path: manifest_path(&effective_resolved.journal_path),
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
            installation_admission: admission,
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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    struct CountingRunner(Arc<AtomicUsize>);
    impl CommandRunner for CountingRunner {
        fn run(&mut self, _request: &CommandRequest) -> Result<CommandOutput, String> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(CommandOutput {
                exit_code: 0,
                stdout: "{}".into(),
                stderr: String::new(),
                timed_out: false,
            })
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
        root.canonicalize().unwrap()
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
        fs::create_dir_all(root.join("bin")).unwrap();
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
        fs::write(
            root.join("pyproject.toml"),
            "[project]\nname = \"fixture\"\n",
        )
        .unwrap();
        fs::write(root.join(".git"), "gitdir: elsewhere\n").unwrap();
        // A checkout is recognised by its payload root carrying the three
        // layout anchors, not by a `solstone` package directory.
        for anchor in [
            solstone_core_journal::LAYOUT_BUNDLE_ANCHOR,
            solstone_core_journal::LAYOUT_LAYOUT_ANCHOR,
            solstone_core_journal::LAYOUT_TEMPLATE_ANCHOR,
        ] {
            let path = root
                .join(solstone_core_journal::CHECKOUT_PAYLOAD_ROOT)
                .join(anchor);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, anchor).unwrap();
        }
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
        assert!(!home.join(".local/bin/solstone").exists());
        assert!(!home.join(".local/bin/journal").exists());
    }

    /// Clean-uninstall is the one irreversible path in the verb, and a bare
    /// count cannot tell an owner WHICH artifact was skipped or failed. The
    /// skip line in particular is how an owner-authored alias announces that
    /// it survived, so it is the one that must reach them.
    #[test]
    fn clean_uninstall_refuses_before_destructive_steps_without_an_identity() {
        let root = root("clean-narration");
        let executable_dir = root.join(".venv/bin");
        let home = root.join("home");
        fs::create_dir_all(&executable_dir).unwrap();
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        fs::write(home.join(".local/bin/solstone"), "#!/bin/sh\necho owner\n").unwrap();
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
        assert!(stdout.is_empty());
        assert!(
            String::from_utf8(stderr)
                .expect("stderr")
                .contains("installation identity admission failed")
        );
        assert!(
            home.join(".local/bin/solstone").exists(),
            "an owner-authored alias must never be removed"
        );
    }

    #[test]
    fn dry_run_and_explain_do_not_create_identity_storage() {
        for flag in ["--dry-run", "--explain"] {
            let root = root(flag.trim_start_matches("--"));
            let home = root.join("home");
            let executable_dir = root.join("bin");
            let journal = root.join("journal");
            fs::create_dir_all(&executable_dir).unwrap();
            let args = parsed(
                &[
                    flag.into(),
                    "--journal".into(),
                    journal.display().to_string(),
                ],
                &root,
            );
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            assert_eq!(
                run_owner_setup_with_io(
                    args,
                    home.clone(),
                    executable_dir,
                    root.clone(),
                    false,
                    false,
                    seams(Vec::new()),
                    &mut stdout,
                    &mut stderr,
                ),
                ExitCode::SUCCESS
            );
            assert!(stderr.is_empty());
            assert!(
                !home
                    .join(".local/share/solstone/installation-identity")
                    .exists()
            );
        }
    }

    #[test]
    fn malformed_artifacts_refuse_before_any_setup_command_runs() {
        let root = root("identity-refusal");
        let home = root.join("home");
        let executable_dir = root.join("bin");
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        fs::create_dir_all(&executable_dir).unwrap();
        fs::write(home.join(".local/bin/solstone"), "owner-authored wrapper").unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let args = parsed(
            &[
                "--yes".into(),
                "--journal".into(),
                root.join("journal").display().to_string(),
            ],
            &root,
        );
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = run_owner_setup_with_io(
            args,
            home.clone(),
            executable_dir,
            root,
            false,
            false,
            Seams {
                runner: Box::new(CountingRunner(calls.clone())),
                service_ops: Box::new(Service),
                check_report_builder: Box::new(Check),
                already_keeps_journal_probe: no_probe,
                prompt: Box::new(Prompt),
                confirm_clean_uninstall: Box::new(|| true),
            },
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, ExitCode::from(2));
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert!(stdout.is_empty());
        assert!(
            String::from_utf8(stderr)
                .unwrap()
                .contains("installation identity admission failed")
        );
        assert!(
            !home
                .join(".local/share/solstone/installation-identity")
                .exists()
        );
    }

    #[test]
    fn native_setup_seam_publishes_an_adopted_identity_record() {
        let root = root("identity-e2e");
        let home = root.join("home");
        let executable_dir = root.join("bin");
        let journal = root.join("journal");
        fs::create_dir_all(&executable_dir).unwrap();
        let args = parsed(
            &[
                "--yes".into(),
                "--journal".into(),
                journal.display().to_string(),
                "--skip-models".into(),
                "--skip-skills".into(),
                "--skip-service".into(),
                "--skip-brain".into(),
            ],
            &root,
        );
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let doctor = CommandOutput {
            exit_code: 0,
            stdout: "{}".into(),
            stderr: String::new(),
            timed_out: false,
        };
        assert_eq!(
            run_owner_setup_with_io(
                args,
                home.clone(),
                executable_dir.clone(),
                root,
                false,
                false,
                seams(vec![doctor]),
                &mut stdout,
                &mut stderr,
            ),
            ExitCode::SUCCESS
        );
        assert!(stderr.is_empty());
        let root_token = root_token_from_path(&executable_dir).unwrap();
        let namespace = namespace_name(PlatformTag::current(), &root_token);
        let record = home
            .join(".local/share/solstone/installation-identity/v1/namespaces")
            .join(namespace.as_hex())
            .join("record");
        assert!(
            fs::read_to_string(record)
                .unwrap()
                .contains("state=adopted\n")
        );
    }

    #[test]
    fn entrypoint_preserves_an_implicit_journal_and_updates_an_explicit_one() {
        assert!(
            std::env::var_os("SOLSTONE_JOURNAL").is_none(),
            "this resolution test requires no process journal override"
        );
        let root = root("identity-journal-source");
        let home = root.join("home");
        let executable_dir = root.join("bin");
        let journal_one = root.join("journal-one");
        let journal_two = root.join("journal-two");
        fs::create_dir_all(&executable_dir).unwrap();

        let setup = |journal: Option<&Path>| {
            let mut values = vec![
                "--yes".to_owned(),
                "--skip-models".to_owned(),
                "--skip-skills".to_owned(),
                "--skip-service".to_owned(),
                "--skip-brain".to_owned(),
            ];
            if let Some(journal) = journal {
                values.extend(["--journal".to_owned(), journal.display().to_string()]);
            }
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            assert_eq!(
                run_owner_setup_with_io(
                    parsed(&values, &root),
                    home.clone(),
                    executable_dir.clone(),
                    root.clone(),
                    false,
                    false,
                    seams(vec![CommandOutput {
                        exit_code: 0,
                        stdout: "{}".into(),
                        stderr: String::new(),
                        timed_out: false,
                    }]),
                    &mut stdout,
                    &mut stderr,
                ),
                ExitCode::SUCCESS
            );
            assert!(stderr.is_empty());
        };

        setup(Some(&journal_one));
        let root_token = root_token_from_path(&executable_dir).unwrap();
        let namespace = namespace_name(PlatformTag::current(), &root_token);
        let record_path = home
            .join(".local/share/solstone/installation-identity/v1/namespaces")
            .join(namespace.as_hex())
            .join("record");
        let first_bytes = fs::read(&record_path).unwrap();
        let first = solstone_core_installation_identity::decode_record(&first_bytes).unwrap();
        assert_eq!(first.journal_token.to_path_buf(), journal_one);

        // Simulate another root changing the owner-wide selection.  This invocation
        // has neither a CLI journal nor an environment override, so its config value
        // must remain implicit and cannot replace this root's adopted journal.
        crate::user_config::write_user_config(&config_path(&home), &journal_two.to_string_lossy())
            .unwrap();
        setup(None);
        let implicit_bytes = fs::read(&record_path).unwrap();
        let implicit = solstone_core_installation_identity::decode_record(&implicit_bytes).unwrap();
        assert_eq!(implicit.journal_token.to_path_buf(), journal_one);
        let implicit_manifest = crate::manifest::read_manifest(&manifest_path(&journal_one))
            .expect("implicit rerun writes its manifest at the adopted journal");
        assert_eq!(
            implicit_manifest.args_resolved["journal"]["value"],
            serde_json::json!(journal_one)
        );
        assert_eq!(
            implicit_manifest.args_resolved["journal"]["source"],
            "config"
        );
        assert_eq!(implicit_bytes, first_bytes);

        setup(Some(&journal_two));
        let explicit_bytes = fs::read(&record_path).unwrap();
        let explicit = solstone_core_installation_identity::decode_record(&explicit_bytes).unwrap();
        assert_eq!(explicit.journal_token.to_path_buf(), journal_two);
        assert_ne!(
            explicit_bytes, implicit_bytes,
            "journal update must refresh checksum bytes"
        );
        assert_ne!(
            explicit_bytes
                .split(|byte| *byte == b'\n')
                .find(|line| line.starts_with(b"checksum=")),
            implicit_bytes
                .split(|byte| *byte == b'\n')
                .find(|line| line.starts_with(b"checksum="))
        );
    }
}
