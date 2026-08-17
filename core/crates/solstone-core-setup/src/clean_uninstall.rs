// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The deliberately narrow destructive branch of `journal setup`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::args::SetupArgs;
use crate::steps::{CommandRequest, CommandRunner, service_artifact_path};
use crate::wrapper::{AliasState, WrapperEnvironment, uninstall_wrappers, wrapper_paths};

pub const CLEAN_UNINSTALL_STEP_NAMES: [&str; 4] = ["service", "wrapper", "config", "manifest"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanUninstallState {
    Removed,
    AlreadyAbsent,
    Skipped,
    Failed,
}

impl CleanUninstallState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Removed => "removed",
            Self::AlreadyAbsent => "already-absent",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanUninstallStepResult {
    pub name: &'static str,
    pub state: CleanUninstallState,
    pub path: Option<PathBuf>,
    pub reason: Option<String>,
}

pub struct CleanUninstallContext<'a> {
    pub journal_path: PathBuf,
    pub home_dir: PathBuf,
    pub config_path: PathBuf,
    pub manifest_path: PathBuf,
    pub curdir: PathBuf,
    pub executable_dir: PathBuf,
    pub yes: bool,
    pub stdin_is_tty: bool,
    pub confirm: &'a mut dyn FnMut() -> bool,
    pub runner: &'a mut dyn CommandRunner,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanUninstallOutcome {
    pub exit_code: i32,
    pub message: String,
    pub results: Vec<CleanUninstallStepResult>,
}

#[must_use]
pub fn clean_uninstall_refusal(args: &SetupArgs) -> Option<String> {
    if args.jsonl {
        return Some("JSONL output is not supported for --clean-uninstall in this version.".into());
    }
    let mut incompatible = Vec::new();
    if args.journal.is_some() {
        incompatible.push("--journal");
    }
    if args.port_supplied() {
        incompatible.push("--port");
    }
    if args.variant_supplied() {
        incompatible.push("--variant");
    }
    if args.step_timeout_seconds_supplied() {
        incompatible.push("--step-timeout-seconds");
    }
    if args.dry_run {
        incompatible.push("--dry-run");
    }
    if args.explain {
        incompatible.push("--explain");
    }
    if args.skip_models {
        incompatible.push("--skip-models");
    }
    if args.skip_brain {
        incompatible.push("--skip-brain");
    }
    if args.skip_skills {
        incompatible.push("--skip-skills");
    }
    if args.skip_service {
        incompatible.push("--skip-service");
    }
    if args.skip_wrapper {
        incompatible.push("--skip-wrapper");
    }
    if args.accept_existing_journal {
        incompatible.push("--accept-existing-journal");
    }
    if args.force {
        incompatible.push("--force");
    }
    (!incompatible.is_empty()).then(|| {
        format!(
            "--clean-uninstall cannot be combined with {}",
            incompatible.join(", ")
        )
    })
}

fn present(path: &Path) -> bool {
    path.exists() || path.is_symlink()
}
/// Owner-facing inventory shown before destructive confirmation.
#[must_use]
pub fn clean_uninstall_confirmation_lines(context: &CleanUninstallContext<'_>) -> Vec<String> {
    let service = service_artifact_path(&context.home_dir);
    let wrappers = wrapper_paths(&context.home_dir);
    let marker = |path: &Path| if present(path) { "present" } else { "absent" };
    let mut lines = vec![
        "journal setup --clean-uninstall will remove these runtime artifacts:".into(),
        String::new(),
    ];
    if let Some(path) = service {
        lines.push(format!(
            "  [{:<7}] service: {}",
            marker(&path),
            path.display()
        ));
    }
    for path in [&wrappers.sol, &wrappers.journal] {
        lines.push(format!(
            "  [{:<7}] wrapper: {}",
            marker(path),
            path.display()
        ));
    }
    lines.extend([
        format!(
            "  [{:<7}] config: {}",
            marker(&context.config_path),
            context.config_path.display()
        ),
        format!(
            "  [{:<7}] manifest: {}",
            marker(&context.manifest_path),
            context.manifest_path.display()
        ),
        String::new(),
        "will not remove:".into(),
        format!("  - journal directory: {}", context.journal_path.display()),
        "  - /Applications/solstone.app".into(),
        "  - ~/Library/Application Support/solstone/".into(),
        "  - macOS microphone or screen recording permissions".into(),
        "  - a leftover pip, uv or pipx journal install".into(),
        String::new(),
    ]);
    lines
}

#[must_use]
pub fn clean_uninstall_has_managed_paths(context: &CleanUninstallContext<'_>) -> bool {
    let wrappers = wrapper_paths(&context.home_dir);
    [
        service_artifact_path(&context.home_dir),
        Some(wrappers.sol),
        Some(wrappers.journal),
        Some(context.config_path.clone()),
        Some(context.manifest_path.clone()),
    ]
    .iter()
    .flatten()
    .any(|path| present(path))
}
fn result(
    name: &'static str,
    state: CleanUninstallState,
    path: Option<PathBuf>,
    reason: Option<String>,
) -> CleanUninstallStepResult {
    CleanUninstallStepResult {
        name,
        state,
        path,
        reason,
    }
}

fn remove_path(name: &'static str, path: PathBuf) -> CleanUninstallStepResult {
    if !present(&path) {
        return result(name, CleanUninstallState::AlreadyAbsent, Some(path), None);
    }
    match fs::remove_file(&path) {
        Ok(()) => result(name, CleanUninstallState::Removed, Some(path), None),
        Err(error) => result(
            name,
            CleanUninstallState::Failed,
            Some(path),
            Some(error.to_string()),
        ),
    }
}

fn remove_service(
    context: &mut CleanUninstallContext<'_>,
    path: Option<PathBuf>,
) -> CleanUninstallStepResult {
    let existed = path.as_ref().is_some_and(|path| present(path));
    let output = context.runner.run(&CommandRequest {
        program: context.executable_dir.join("journal"),
        args: vec!["service".into(), "uninstall".into()],
        timeout_seconds: None,
    });
    match output {
        Err(error) => result("service", CleanUninstallState::Failed, path, Some(error)),
        Ok(output) if output.exit_code != 0 => result(
            "service",
            CleanUninstallState::Failed,
            path,
            Some(format!("service uninstall exited {}", output.exit_code)),
        ),
        Ok(_) if existed => match path.as_ref().map(fs::remove_file) {
            Some(Ok(())) => result("service", CleanUninstallState::Removed, path, None),
            Some(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                result("service", CleanUninstallState::Removed, path, None)
            }
            Some(Err(error)) => result(
                "service",
                CleanUninstallState::Failed,
                path,
                Some(error.to_string()),
            ),
            None => unreachable!(),
        },
        Ok(_) => result("service", CleanUninstallState::AlreadyAbsent, path, None),
    }
}

fn remove_wrappers(
    context: &CleanUninstallContext<'_>,
    paths: &(PathBuf, PathBuf),
) -> CleanUninstallStepResult {
    let existed = present(&paths.0) || present(&paths.1);
    let environment = WrapperEnvironment {
        home_dir: context.home_dir.clone(),
        curdir: context.curdir.clone(),
        executable_dir: context.executable_dir.clone(),
        backup_dir: None,
    };
    match uninstall_wrappers(&environment) {
        Ok(()) => result(
            "wrapper",
            if existed {
                CleanUninstallState::Removed
            } else {
                CleanUninstallState::AlreadyAbsent
            },
            Some(paths.0.clone()),
            None,
        ),
        Err((AliasState::Worktree, _)) => result(
            "wrapper",
            CleanUninstallState::Skipped,
            Some(paths.0.clone()),
            Some("refusing to act from a git worktree".into()),
        ),
        Err((AliasState::CrossRepo, target)) => result(
            "wrapper",
            CleanUninstallState::Skipped,
            Some(paths.0.clone()),
            Some(format!(
                "alias points at {}, not removing",
                target.map_or_else(|| "unknown".into(), |path| path.display().to_string())
            )),
        ),
        Err((AliasState::Dangling, target)) => result(
            "wrapper",
            CleanUninstallState::Skipped,
            Some(paths.0.clone()),
            Some(format!(
                "alias is dangling (target {} missing), not removing",
                target.map_or_else(|| "unknown".into(), |path| path.display().to_string())
            )),
        ),
        Err((AliasState::Foreign, _)) => result(
            "wrapper",
            CleanUninstallState::Skipped,
            Some(paths.0.clone()),
            Some("alias is not a managed symlink, not removing".into()),
        ),
        Err((state, _)) => result(
            "wrapper",
            CleanUninstallState::Failed,
            Some(paths.0.clone()),
            Some(format!("unexpected alias state: {state:?}")),
        ),
    }
}

pub fn run_clean_uninstall(context: &mut CleanUninstallContext<'_>) -> CleanUninstallOutcome {
    let service = service_artifact_path(&context.home_dir);
    let wrappers = wrapper_paths(&context.home_dir);
    if !clean_uninstall_has_managed_paths(context) {
        return CleanUninstallOutcome {
            exit_code: 0,
            message: "nothing to remove (all paths already absent)".into(),
            results: Vec::new(),
        };
    }
    if !context.yes && !context.stdin_is_tty {
        return CleanUninstallOutcome {
            exit_code: 0,
            message: "not a tty; rerun with --yes to proceed non-interactively (cancelled)".into(),
            results: Vec::new(),
        };
    }
    if !context.yes && !(context.confirm)() {
        return CleanUninstallOutcome {
            exit_code: 0,
            message: "cancelled".into(),
            results: Vec::new(),
        };
    }
    let results = vec![
        remove_service(context, service),
        remove_wrappers(context, &(wrappers.sol, wrappers.journal)),
        remove_path("config", context.config_path.clone()),
        remove_path("manifest", context.manifest_path.clone()),
    ];
    let counts = |state| {
        results
            .iter()
            .filter(|result| result.state == state)
            .count()
    };
    let message = format!(
        "clean uninstall complete: {} removed, {} already-absent, {} skipped, {} failed",
        counts(CleanUninstallState::Removed),
        counts(CleanUninstallState::AlreadyAbsent),
        counts(CleanUninstallState::Skipped),
        counts(CleanUninstallState::Failed)
    );
    CleanUninstallOutcome {
        exit_code: if counts(CleanUninstallState::Failed) == 0 {
            0
        } else {
            1
        },
        message,
        results,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::parse_args_at;
    use std::collections::VecDeque;
    use std::ffi::OsString;

    struct Runner(VecDeque<i32>);
    impl CommandRunner for Runner {
        fn run(
            &mut self,
            _request: &CommandRequest,
        ) -> Result<crate::steps::CommandOutput, String> {
            Ok(crate::steps::CommandOutput {
                exit_code: self.0.pop_front().unwrap_or(0),
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
            })
        }
    }
    fn root(name: &str) -> PathBuf {
        let root =
            PathBuf::from("/var/tmp").join(format!("solstone-clean-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }
    fn args(values: &[&str]) -> SetupArgs {
        parse_args_at(
            &values.iter().map(OsString::from).collect::<Vec<_>>(),
            Path::new("/var/tmp"),
        )
        .unwrap()
    }
    #[test]
    fn refusal_checks_jsonl_before_incompatible_flags_and_accepts_yes() {
        assert_eq!(
            clean_uninstall_refusal(&args(&["--clean-uninstall", "--jsonl", "--port", "5015"])),
            Some("JSONL output is not supported for --clean-uninstall in this version.".into())
        );
        assert_eq!(
            clean_uninstall_refusal(&args(&["--clean-uninstall", "--port", "5015", "--force"])),
            Some("--clean-uninstall cannot be combined with --port, --force".into())
        );
        assert_eq!(
            clean_uninstall_refusal(&args(&["--clean-uninstall", "--yes"])),
            None
        );
    }
    #[test]
    fn all_absent_and_no_tty_do_not_run_steps() {
        let root = root("early");
        let mut runner = Runner(VecDeque::new());
        let mut confirm = || true;
        let mut context = CleanUninstallContext {
            journal_path: root.join("journal"),
            home_dir: root.join("home"),
            config_path: root.join("config.toml"),
            manifest_path: root.join("journal/health/setup-state.json"),
            curdir: root.join("repo"),
            executable_dir: root.join("bin"),
            yes: false,
            stdin_is_tty: false,
            confirm: &mut confirm,
            runner: &mut runner,
        };
        assert_eq!(
            run_clean_uninstall(&mut context).message,
            "nothing to remove (all paths already absent)"
        );
        fs::create_dir_all(context.home_dir.join(".local/bin")).unwrap();
        fs::write(context.home_dir.join(".local/bin/sol"), "foreign").unwrap();
        assert_eq!(
            run_clean_uninstall(&mut context).message,
            "not a tty; rerun with --yes to proceed non-interactively (cancelled)"
        );
    }
    #[test]
    fn foreign_wrapper_is_skipped_with_the_measured_reason() {
        let root = root("foreign");
        let home = root.join("home");
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        fs::write(home.join(".local/bin/sol"), "foreign").unwrap();
        let mut runner = Runner(VecDeque::from([0]));
        let mut confirm = || true;
        let mut context = CleanUninstallContext {
            journal_path: root.join("journal"),
            home_dir: home,
            config_path: root.join("config.toml"),
            manifest_path: root.join("journal/health/setup-state.json"),
            curdir: root.join("repo"),
            executable_dir: root.join("bin"),
            yes: true,
            stdin_is_tty: false,
            confirm: &mut confirm,
            runner: &mut runner,
        };
        let outcome = run_clean_uninstall(&mut context);
        assert_eq!(
            outcome
                .results
                .iter()
                .find(|result| result.name == "wrapper")
                .unwrap()
                .reason
                .as_deref(),
            Some("alias is not a managed symlink, not removing")
        );
    }
    #[test]
    fn runs_the_four_fixed_steps_in_order_and_fails_for_service_failure() {
        let root = root("order");
        let home = root.join("home");
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        let runtime = root.join("bin");
        fs::create_dir_all(&runtime).unwrap();
        for binary in ["sol", "journal"] {
            fs::write(
                home.join(".local/bin").join(binary),
                crate::wrapper::render_wrapper(
                    binary,
                    Path::new("/journal"),
                    &runtime.join(binary),
                ),
            )
            .unwrap();
        }
        let config = root.join("config.toml");
        let manifest = root.join("journal/health/setup-state.json");
        fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        fs::write(&config, "x").unwrap();
        fs::write(&manifest, "x").unwrap();
        let mut runner = Runner(VecDeque::from([7]));
        let mut confirm = || true;
        let mut context = CleanUninstallContext {
            journal_path: root.join("journal"),
            home_dir: home,
            config_path: config,
            manifest_path: manifest,
            curdir: root.join("repo"),
            executable_dir: runtime,
            yes: true,
            stdin_is_tty: false,
            confirm: &mut confirm,
            runner: &mut runner,
        };
        let outcome = run_clean_uninstall(&mut context);
        assert_eq!(outcome.exit_code, 1);
        assert_eq!(
            outcome
                .results
                .iter()
                .map(|result| result.name)
                .collect::<Vec<_>>(),
            CLEAN_UNINSTALL_STEP_NAMES
        );
    }

    #[test]
    fn confirmation_inventory_marks_managed_paths_and_preserves_owner_data() {
        let root = root("confirmation");
        let home = root.join("home");
        let config = root.join("config.toml");
        let manifest = root.join("journal/health/setup-state.json");
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        fs::write(home.join(".local/bin/sol"), "managed").unwrap();
        fs::write(&config, "journal = \"x\"\n").unwrap();
        let mut runner = Runner(VecDeque::new());
        let mut confirm = || true;
        let context = CleanUninstallContext {
            journal_path: root.join("journal"),
            home_dir: home.clone(),
            config_path: config.clone(),
            manifest_path: manifest.clone(),
            curdir: root.join("repo"),
            executable_dir: root.join("bin"),
            yes: false,
            stdin_is_tty: true,
            confirm: &mut confirm,
            runner: &mut runner,
        };
        let lines = clean_uninstall_confirmation_lines(&context).join("\n");
        assert!(lines.contains("[present] wrapper: "));
        assert!(lines.contains(&format!("[present] config: {}", config.display())));
        assert!(lines.contains(&format!("[absent ] manifest: {}", manifest.display())));
        assert!(lines.contains(&format!(
            "  - journal directory: {}",
            context.journal_path.display()
        )));
        assert!(lines.contains("  - a leftover pip, uv or pipx journal install"));
    }
}
