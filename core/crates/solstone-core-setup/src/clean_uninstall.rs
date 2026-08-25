// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The deliberately narrow destructive branch of `journal setup`.

use std::fs;
use std::path::{Path, PathBuf};

use solstone_core_installation_identity::{
    ArtifactBindingEvidence, CleanUninstallPlan, GuardFields,
};

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
    pub plan: CleanUninstallPlan,
    pub artifact_evidence: ArtifactBindingEvidence,
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
    for path in [&wrappers.solstone, &wrappers.journal] {
        lines.push(format!(
            "  [{:<7}] wrapper: {}",
            marker(path),
            path.display()
        ));
    }
    lines.extend([
        format!(
            "  [{:<7}] config: {}",
            if context.plan.remove_owner_config {
                marker(&context.config_path)
            } else {
                "retain"
            },
            context.config_path.display()
        ),
        format!(
            "  [{:<7}] manifest: {}",
            if context.plan.remove_journal_manifest {
                marker(&context.manifest_path)
            } else {
                "retain"
            },
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
        Some(wrappers.solstone),
        Some(wrappers.journal),
        context
            .plan
            .remove_owner_config
            .then(|| context.config_path.clone()),
        context
            .plan
            .remove_journal_manifest
            .then(|| context.manifest_path.clone()),
    ]
    .iter()
    .flatten()
    .any(|path| present(path))
}
fn child_failure_reason(output: &crate::steps::CommandOutput) -> String {
    let mut reason = format!("service uninstall exited {}", output.exit_code);
    if output.timed_out {
        reason.push_str(" (timed out)");
    }
    let details = if output.stderr.trim().is_empty() {
        output.stdout.as_str()
    } else {
        output.stderr.as_str()
    };
    if let Some(line) = details.lines().map(str::trim).find(|line| !line.is_empty()) {
        reason.push_str(": ");
        reason.push_str(line);
    }
    reason
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
    if !artifact_evidence_matches_plan(context) {
        return result(
            "service",
            CleanUninstallState::Skipped,
            path,
            Some("service or wrapper guard does not match this installation, not removing".into()),
        );
    }
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
            Some(child_failure_reason(&output)),
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
    if !artifact_evidence_matches_plan(context) {
        return result(
            "wrapper",
            CleanUninstallState::Skipped,
            Some(paths.0.clone()),
            Some("wrapper guard does not match this installation, not removing".into()),
        );
    }
    if matches!(
        &context.artifact_evidence,
        ArtifactBindingEvidence::Guarded(_)
    ) {
        let existed = present(&paths.0) || present(&paths.1);
        for path in [&paths.0, &paths.1] {
            if present(path)
                && let Err(error) = fs::remove_file(path)
            {
                return result(
                    "wrapper",
                    CleanUninstallState::Failed,
                    Some(path.clone()),
                    Some(error.to_string()),
                );
            }
        }
        return result(
            "wrapper",
            if existed {
                CleanUninstallState::Removed
            } else {
                CleanUninstallState::AlreadyAbsent
            },
            Some(paths.0.clone()),
            None,
        );
    }
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

fn artifact_evidence_matches_plan(context: &CleanUninstallContext<'_>) -> bool {
    match &context.artifact_evidence {
        ArtifactBindingEvidence::Fresh => true,
        ArtifactBindingEvidence::Guarded(fields) => {
            *fields == GuardFields::from_binding(&context.plan.binding)
        }
        ArtifactBindingEvidence::LegacyUnguarded
        | ArtifactBindingEvidence::Foreign
        | ArtifactBindingEvidence::Malformed
        | ArtifactBindingEvidence::Ambiguous => false,
    }
}

fn remove_if_last(name: &'static str, path: PathBuf, remove: bool) -> CleanUninstallStepResult {
    if remove {
        remove_path(name, path)
    } else {
        result(
            name,
            CleanUninstallState::Skipped,
            Some(path),
            Some("retained for another adopted installation".into()),
        )
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
            exit_code: 2,
            message: "not a tty; rerun with --yes to proceed non-interactively (cancelled)".into(),
            results: Vec::new(),
        };
    }
    if !context.yes && !(context.confirm)() {
        return CleanUninstallOutcome {
            exit_code: 1,
            message: "cancelled".into(),
            results: Vec::new(),
        };
    }
    let service_result = remove_service(context, service);
    if service_result.state == CleanUninstallState::Failed {
        let leftover = "not run because service uninstall failed";
        return CleanUninstallOutcome {
            exit_code: 1,
            message: "clean uninstall stopped: service uninstall failed; wrappers, config, and manifest were left in place".into(),
            results: vec![
                service_result,
                result(
                    "wrapper",
                    CleanUninstallState::Skipped,
                    Some(wrappers.solstone),
                    Some(leftover.into()),
                ),
                result(
                    "config",
                    CleanUninstallState::Skipped,
                    Some(context.config_path.clone()),
                    Some(leftover.into()),
                ),
                result(
                    "manifest",
                    CleanUninstallState::Skipped,
                    Some(context.manifest_path.clone()),
                    Some(leftover.into()),
                ),
            ],
        };
    }
    let results = vec![
        service_result,
        remove_wrappers(context, &(wrappers.solstone, wrappers.journal)),
        remove_if_last(
            "config",
            context.config_path.clone(),
            context.plan.remove_owner_config,
        ),
        remove_if_last(
            "manifest",
            context.manifest_path.clone(),
            context.plan.remove_journal_manifest,
        ),
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
    use crate::identity_evidence::gather_artifact_evidence;
    use crate::manifest::manifest_path;
    use crate::user_config::{config_path, write_user_config};
    use crate::wrapper::{render_wrapper, wrapper_paths};
    use solstone_core_installation_identity::{
        CleanUninstallRequest, LegacyManifestEvidence, OwnerBase, PlatformTag,
        SetupAdmissionRequest, admit_clean_uninstall, admit_setup, journal_token_from_path,
        root_token_from_path,
    };
    use std::collections::VecDeque;
    use std::ffi::OsString;

    fn plan() -> CleanUninstallPlan {
        let binding = solstone_core_installation_identity::InstallationBinding {
            namespace: solstone_core_installation_identity::NamespaceName::parse(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            )
            .expect("namespace"),
            id: solstone_core_installation_identity::InstallationId::parse(
                "00112233445566778899aabbccddeeff",
            )
            .expect("id"),
            generation: solstone_core_installation_identity::Generation::new(1)
                .expect("generation"),
            platform: solstone_core_installation_identity::PlatformTag::Linux,
            root_token: solstone_core_installation_identity::RootToken::from_raw_absolute(
                b"/install/clean".to_vec(),
            )
            .expect("root"),
            journal_token: solstone_core_installation_identity::JournalToken::from_raw_absolute(
                b"/journal".to_vec(),
            )
            .expect("journal"),
        };
        CleanUninstallPlan {
            binding,
            remove_owner_config: true,
            remove_journal_manifest: true,
            already_tombstoned: false,
        }
    }

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
            plan: plan(),
            artifact_evidence: ArtifactBindingEvidence::Fresh,
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
        fs::write(context.home_dir.join(".local/bin/solstone"), "foreign").unwrap();
        let cancelled = run_clean_uninstall(&mut context);
        assert_eq!(cancelled.exit_code, 2);
        assert_eq!(
            cancelled.message,
            "not a tty; rerun with --yes to proceed non-interactively (cancelled)"
        );
    }

    #[test]
    fn interactive_decline_is_a_nonzero_cancel() {
        let root = root("decline");
        let home = root.join("home");
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        fs::write(home.join(".local/bin/solstone"), "foreign").unwrap();
        let mut runner = Runner(VecDeque::new());
        let mut confirm = || false;
        let mut context = CleanUninstallContext {
            journal_path: root.join("journal"),
            home_dir: home,
            config_path: root.join("config.toml"),
            manifest_path: root.join("journal/health/setup-state.json"),
            plan: plan(),
            artifact_evidence: ArtifactBindingEvidence::Fresh,
            curdir: root.join("repo"),
            executable_dir: root.join("bin"),
            yes: false,
            stdin_is_tty: true,
            confirm: &mut confirm,
            runner: &mut runner,
        };
        let outcome = run_clean_uninstall(&mut context);
        assert_eq!(outcome.exit_code, 1);
        assert_eq!(outcome.message, "cancelled");
        assert!(outcome.results.is_empty());
    }
    #[test]
    fn foreign_wrapper_is_skipped_with_the_measured_reason() {
        let root = root("foreign");
        let home = root.join("home");
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        fs::write(home.join(".local/bin/solstone"), "foreign").unwrap();
        let mut runner = Runner(VecDeque::from([0]));
        let mut confirm = || true;
        let mut context = CleanUninstallContext {
            journal_path: root.join("journal"),
            home_dir: home,
            config_path: root.join("config.toml"),
            manifest_path: root.join("journal/health/setup-state.json"),
            plan: plan(),
            artifact_evidence: ArtifactBindingEvidence::Fresh,
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
    struct RunnerWithOutput {
        exits: VecDeque<i32>,
        stderr: String,
    }
    impl CommandRunner for RunnerWithOutput {
        fn run(
            &mut self,
            _request: &CommandRequest,
        ) -> Result<crate::steps::CommandOutput, String> {
            Ok(crate::steps::CommandOutput {
                exit_code: self.exits.pop_front().unwrap_or(0),
                stdout: String::new(),
                stderr: self.stderr.clone(),
                timed_out: false,
            })
        }
    }

    #[test]
    fn service_failure_stops_before_wrappers_and_names_the_child() {
        let root = root("order");
        let home = root.join("home");
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        let runtime = root.join("bin");
        fs::create_dir_all(&runtime).unwrap();
        let guard = GuardFields::from_binding(&plan().binding);
        for binary in ["solstone", "journal"] {
            fs::write(
                home.join(".local/bin").join(binary),
                crate::wrapper::render_wrapper(
                    binary,
                    Path::new("/journal"),
                    &runtime.join(binary),
                    &guard,
                ),
            )
            .unwrap();
        }
        let config = root.join("config.toml");
        let manifest = root.join("journal/health/setup-state.json");
        fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        fs::write(&config, "x").unwrap();
        fs::write(&manifest, "x").unwrap();
        let mut runner = RunnerWithOutput {
            exits: VecDeque::from([7]),
            stderr:
                "error: launchd accepted the unload request, but the service is still present\n"
                    .into(),
        };
        let mut confirm = || true;
        let mut context = CleanUninstallContext {
            journal_path: root.join("journal"),
            home_dir: home.clone(),
            config_path: config.clone(),
            manifest_path: manifest.clone(),
            plan: plan(),
            artifact_evidence: ArtifactBindingEvidence::Fresh,
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
        assert_eq!(
            outcome
                .results
                .iter()
                .map(|result| result.state)
                .collect::<Vec<_>>(),
            [
                CleanUninstallState::Failed,
                CleanUninstallState::Skipped,
                CleanUninstallState::Skipped,
                CleanUninstallState::Skipped,
            ]
        );
        assert_eq!(
            outcome.results[0].reason.as_deref(),
            Some(
                "service uninstall exited 7: error: launchd accepted the unload request, but the service is still present"
            )
        );
        assert!(
            outcome.message.contains("left in place"),
            "{}",
            outcome.message
        );
        assert!(home.join(".local/bin/solstone").exists());
        assert!(home.join(".local/bin/journal").exists());
        assert!(config.exists());
        assert!(manifest.exists());
    }

    #[test]
    fn matching_guarded_wrappers_are_removed_even_when_their_binary_is_no_longer_current() {
        let root = root("guarded-wrapper");
        let home = root.join("home");
        let runtime = root.join("new-bin");
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        fs::create_dir_all(&runtime).unwrap();
        let guard = GuardFields::from_binding(&plan().binding);
        for binary in ["solstone", "journal"] {
            fs::write(
                home.join(".local/bin").join(binary),
                crate::wrapper::render_wrapper(
                    binary,
                    Path::new("/journal"),
                    &root.join("old-bin").join(binary),
                    &guard,
                ),
            )
            .unwrap();
        }
        let mut runner = Runner(VecDeque::from([0]));
        let mut confirm = || true;
        let mut context = CleanUninstallContext {
            journal_path: root.join("journal"),
            home_dir: home.clone(),
            config_path: root.join("config.toml"),
            manifest_path: root.join("journal/health/setup-state.json"),
            plan: plan(),
            artifact_evidence: ArtifactBindingEvidence::Guarded(guard),
            curdir: root.join("repo"),
            executable_dir: runtime,
            yes: true,
            stdin_is_tty: false,
            confirm: &mut confirm,
            runner: &mut runner,
        };
        let outcome = run_clean_uninstall(&mut context);
        assert_eq!(outcome.exit_code, 0);
        assert!(!home.join(".local/bin/solstone").exists());
        assert!(!home.join(".local/bin/journal").exists());
    }

    fn setup_request(
        owner: OwnerBase,
        install_root: &Path,
        journal: &Path,
    ) -> SetupAdmissionRequest {
        SetupAdmissionRequest {
            owner,
            root_token: root_token_from_path(install_root).expect("root token"),
            journal_token: journal_token_from_path(journal).expect("journal token"),
            journal_is_explicit: true,
            legacy_manifest: LegacyManifestEvidence::Absent,
            artifacts: ArtifactBindingEvidence::Fresh,
        }
    }

    fn clean_context<'a>(
        home: &Path,
        root: &Path,
        plan: CleanUninstallPlan,
        artifact_evidence: ArtifactBindingEvidence,
        runner: &'a mut Runner,
        confirm: &'a mut dyn FnMut() -> bool,
    ) -> CleanUninstallContext<'a> {
        let journal_path = plan.binding.journal_token.to_path_buf();
        CleanUninstallContext {
            manifest_path: manifest_path(&journal_path),
            journal_path,
            home_dir: home.to_path_buf(),
            config_path: config_path(home),
            plan,
            artifact_evidence,
            curdir: root.to_path_buf(),
            executable_dir: root.join("bin"),
            yes: true,
            stdin_is_tty: false,
            confirm,
            runner,
        }
    }

    #[test]
    fn two_roots_clean_in_either_order_preserving_foreign_wrappers_and_shared_state() {
        for (same_journal, first_is_a) in [false, true].into_iter().flat_map(|same_journal| {
            [true, false].map(move |first_is_a| (same_journal, first_is_a))
        }) {
            let name = match (same_journal, first_is_a) {
                (false, true) => "different-a-first",
                (false, false) => "different-b-first",
                (true, true) => "shared-a-first",
                (true, false) => "shared-b-first",
            };
            let root = root(name);
            let home = root.join("home");
            let install_a = root.join("install-a");
            let install_b = root.join("install-b");
            let journal_a = root.join("journal-a");
            let journal_b = if same_journal {
                journal_a.clone()
            } else {
                root.join("journal-b")
            };
            fs::create_dir_all(&home).unwrap();
            fs::create_dir_all(&install_a).unwrap();
            fs::create_dir_all(&install_b).unwrap();
            let owner = OwnerBase::at_home(home.clone(), PlatformTag::current()).unwrap();
            let binding_a = admit_setup(setup_request(owner.clone(), &install_a, &journal_a))
                .unwrap()
                .binding()
                .clone();
            let binding_b = admit_setup(setup_request(owner.clone(), &install_b, &journal_b))
                .unwrap()
                .binding()
                .clone();
            assert_ne!(binding_a.namespace, binding_b.namespace);
            assert_ne!(binding_a.id, binding_b.id);

            let manifest_a = manifest_path(&journal_a);
            let manifest_b = manifest_path(&journal_b);
            fs::create_dir_all(manifest_a.parent().unwrap()).unwrap();
            fs::write(&manifest_a, "manifest-a").unwrap();
            if manifest_b != manifest_a {
                fs::create_dir_all(manifest_b.parent().unwrap()).unwrap();
                fs::write(&manifest_b, "manifest-b").unwrap();
            }
            write_user_config(&config_path(&home), &journal_b.to_string_lossy()).unwrap();

            let (first, second) = if first_is_a {
                (&binding_a, &binding_b)
            } else {
                (&binding_b, &binding_a)
            };
            let first_manifest = manifest_path(&first.journal_token.to_path_buf());
            let second_manifest = manifest_path(&second.journal_token.to_path_buf());
            let paths = wrapper_paths(&home);
            fs::create_dir_all(paths.solstone.parent().unwrap()).unwrap();
            let second_guard = GuardFields::from_binding(second);
            let second_solstone = render_wrapper(
                "solstone",
                &second.journal_token.to_path_buf(),
                &root.join("bin/solstone"),
                &second_guard,
            );
            let second_journal = render_wrapper(
                "journal",
                &second.journal_token.to_path_buf(),
                &root.join("bin/journal"),
                &second_guard,
            );
            fs::write(&paths.solstone, &second_solstone).unwrap();
            fs::write(&paths.journal, &second_journal).unwrap();

            let first_evidence = gather_artifact_evidence(&home, &first.namespace);
            assert_eq!(first_evidence, ArtifactBindingEvidence::Foreign);
            let first_session = admit_clean_uninstall(CleanUninstallRequest {
                owner: owner.clone(),
                root_token: first.root_token.clone(),
                artifacts: first_evidence.clone(),
            })
            .expect("an unambiguous guard for the other root is preserved, not refused");
            let first_plan = first_session.plan().clone();
            assert!(!first_plan.remove_owner_config);
            assert_eq!(first_plan.remove_journal_manifest, !same_journal);
            let mut first_runner = Runner(VecDeque::new());
            let mut confirm = || true;
            let first_outcome = run_clean_uninstall(&mut clean_context(
                &home,
                &root,
                first_plan,
                first_evidence,
                &mut first_runner,
                &mut confirm,
            ));
            assert_eq!(first_outcome.exit_code, 0);
            assert_eq!(first_outcome.results[1].state, CleanUninstallState::Skipped);
            first_session.commit_tombstone().unwrap();

            assert_eq!(
                fs::read_to_string(&paths.solstone).unwrap(),
                second_solstone
            );
            assert_eq!(fs::read_to_string(&paths.journal).unwrap(), second_journal);
            assert!(config_path(&home).exists());
            assert!(
                second_manifest.exists(),
                "the other root's manifest must survive"
            );
            assert_eq!(first_manifest.exists(), same_journal);

            let second_evidence = gather_artifact_evidence(&home, &second.namespace);
            assert_eq!(
                second_evidence,
                ArtifactBindingEvidence::Guarded(GuardFields::from_binding(second))
            );
            let second_session = admit_clean_uninstall(CleanUninstallRequest {
                owner: owner.clone(),
                root_token: second.root_token.clone(),
                artifacts: second_evidence.clone(),
            })
            .expect("remaining root admission");
            let second_plan = second_session.plan().clone();
            assert!(second_plan.remove_owner_config);
            assert!(second_plan.remove_journal_manifest);
            let mut second_runner = Runner(VecDeque::from([0]));
            let mut confirm = || true;
            let second_outcome = run_clean_uninstall(&mut clean_context(
                &home,
                &root,
                second_plan.clone(),
                second_evidence,
                &mut second_runner,
                &mut confirm,
            ));
            assert_eq!(second_outcome.exit_code, 0);
            second_session.commit_tombstone().unwrap();

            assert!(!paths.solstone.exists());
            assert!(!paths.journal.exists());
            assert!(!config_path(&home).exists());
            assert!(!manifest_b.exists());
            assert!(!manifest_a.exists());

            let retry = admit_clean_uninstall(CleanUninstallRequest {
                owner,
                root_token: second.root_token.clone(),
                artifacts: ArtifactBindingEvidence::Fresh,
            })
            .expect("a completed uninstall is idempotently admissible");
            assert!(retry.plan().already_tombstoned);
            assert_eq!(
                retry.plan().binding.generation,
                second_plan.binding.generation
            );
            let mut retry_runner = Runner(VecDeque::new());
            let mut confirm = || true;
            let retry_outcome = run_clean_uninstall(&mut clean_context(
                &home,
                &root,
                retry.plan().clone(),
                ArtifactBindingEvidence::Fresh,
                &mut retry_runner,
                &mut confirm,
            ));
            assert_eq!(retry_outcome.exit_code, 0);
            assert!(retry_outcome.results.is_empty());
            retry.commit_tombstone().unwrap();
        }
    }

    #[test]
    fn confirmation_inventory_marks_managed_paths_and_preserves_owner_data() {
        let root = root("confirmation");
        let home = root.join("home");
        let config = root.join("config.toml");
        let manifest = root.join("journal/health/setup-state.json");
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        fs::write(home.join(".local/bin/solstone"), "managed").unwrap();
        fs::write(&config, "journal = \"x\"\n").unwrap();
        let mut runner = Runner(VecDeque::new());
        let mut confirm = || true;
        let context = CleanUninstallContext {
            journal_path: root.join("journal"),
            home_dir: home.clone(),
            config_path: config.clone(),
            manifest_path: manifest.clone(),
            plan: plan(),
            artifact_evidence: ArtifactBindingEvidence::Fresh,
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
