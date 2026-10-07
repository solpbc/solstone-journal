// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The deliberately narrow destructive branch of `journal setup`.

use std::fs;
use std::path::{Path, PathBuf};

use solstone_core_installation_identity::{
    ArtifactBindingEvidence, CleanUninstallPlan, CleanupSkip, CleanupTargetDecision,
    CleanupTargetKind, GuardFields, InstallationJournalRecord, LifecycleState, OwnerBase,
    PlatformTag, ProtectedJournals, RootToken, identity_writes_overlap, may_remove_cleanup_target,
    read_installation_journal_census, same_protected_place,
};

use crate::args::SetupArgs;
use crate::steps::{CommandRequest, CommandRunner, service_artifact_path, solstone_executable};
use crate::wrapper::{AliasState, WrapperEnvironment, uninstall_wrappers, wrapper_paths};

pub const CLEAN_UNINSTALL_STEP_NAMES: [&str; 8] = [
    "service",
    "wrapper",
    "config",
    "manifest",
    "rclone",
    "package-receipt",
    "setup-backups",
    "user-skill",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanUninstallState {
    Removed,
    AlreadyAbsent,
    Preserved,
    Skipped,
    Failed,
}

impl CleanUninstallState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Removed => "removed",
            Self::AlreadyAbsent => "already-absent",
            Self::Preserved => "preserved",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanUninstallMark {
    None,
    ProtectedJournal,
    NoOwnedPath,
    AnotherInstallation,
    RegistryUnreadable,
    Foreign,
    NotRun,
}

impl CleanUninstallMark {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::ProtectedJournal => "ProtectedJournal",
            Self::NoOwnedPath => "NoOwnedPath",
            Self::AnotherInstallation => "AnotherInstallation",
            Self::RegistryUnreadable => "RegistryUnreadable",
            Self::Foreign => "Foreign",
            Self::NotRun => "NotRun",
        }
    }
}

/// What the parent does with its installation-identity hold around the service child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityHold {
    /// Just before the service child starts.
    Release,
    /// Once the service child has succeeded, before anything else is removed.
    Reacquire,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanUninstallStepResult {
    pub name: &'static str,
    pub state: CleanUninstallState,
    pub path: Option<PathBuf>,
    pub reason: Option<String>,
    pub mark: CleanUninstallMark,
}

pub struct CleanUninstallContext<'a> {
    pub journal_path: PathBuf,
    pub home_dir: PathBuf,
    pub config_path: PathBuf,
    pub manifest_path: PathBuf,
    pub plan: CleanUninstallPlan,
    pub protected_journals: ProtectedJournals,
    pub registry_known: bool,
    pub platform: PlatformTag,
    pub bundled_user_skill: PathBuf,
    pub artifact_evidence: ArtifactBindingEvidence,
    pub curdir: PathBuf,
    pub executable_dir: PathBuf,
    pub yes: bool,
    pub stdin_is_tty: bool,
    pub confirm: &'a mut dyn FnMut() -> bool,
    pub runner: &'a mut dyn CommandRunner,
    /// Windows service commands reload the binding under the identity locks, so a parent
    /// that kept holding them would wait on its own child until the child timed out.
    /// A failed reacquire stops the uninstall before the wrappers, config and manifest.
    pub identity_hold: &'a mut dyn FnMut(IdentityHold) -> Result<(), String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanUninstallOutcome {
    pub exit_code: i32,
    pub message: String,
    pub results: Vec<CleanUninstallStepResult>,
    pub journal_path: Option<PathBuf>,
    pub already_complete: bool,
}

impl CleanUninstallOutcome {
    #[must_use]
    pub fn already_complete(journal_path: Option<PathBuf>, platform: PlatformTag) -> Self {
        Self {
            exit_code: 0,
            message: "clean uninstall is already complete".into(),
            results: macos_preserved_results(platform),
            journal_path,
            already_complete: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CleanUninstallPreflightState {
    Proceed,
    NoBinding,
    AlreadyComplete,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanUninstallPreflight {
    pub state: CleanUninstallPreflightState,
    pub protected_journals: ProtectedJournals,
    pub registry_known: bool,
    pub journal_path: Option<PathBuf>,
    pub root_record: Option<InstallationJournalRecord>,
}

/// Read the config and identity registry without creating owner storage.
pub fn clean_uninstall_preflight(
    owner: &OwnerBase,
    root: &RootToken,
    config_file: &Path,
) -> Result<CleanUninstallPreflight, String> {
    let config_journal =
        crate::user_config::read_cleanup_journal(config_file).map_err(|error| error.to_string())?;
    let mut protected_journals = ProtectedJournals::new(owner.platform());
    if let Some(path) = &config_journal {
        protected_journals.insert(path.clone());
    }
    if identity_writes_overlap(&owner.path(), &protected_journals, owner.platform()) {
        return Err("identity storage overlaps a protected journal".into());
    }
    let census = read_installation_journal_census(owner, root, true)
        .map_err(|error| format!("installation identity census is unreadable: {error}"))?;
    let registry_known = census.registry_known;
    let records = census.records;
    let root_record = census.root_record;
    for record in &records {
        protected_journals.insert(record.journal_token.to_path_buf());
    }
    if let Some(record) = &root_record {
        let record_journal = record.journal_token.to_path_buf();
        if let Some(config_journal) = &config_journal
            && !same_protected_place(
                &record_journal,
                config_journal,
                owner.platform() == PlatformTag::Windows,
            )
        {
            return Err("config journal does not match the installation identity".into());
        }
        protected_journals.insert(record_journal);
    }
    if identity_writes_overlap(&owner.path(), &protected_journals, owner.platform()) {
        return Err("identity storage overlaps a protected journal".into());
    }
    if root_record
        .as_ref()
        .is_some_and(|record| record.lifecycle == LifecycleState::Prepared)
    {
        return Err("installation identity is prepared".into());
    }
    let state = match root_record.as_ref().map(|record| record.lifecycle) {
        Some(LifecycleState::Tombstoned) => CleanUninstallPreflightState::AlreadyComplete,
        Some(LifecycleState::Adopted) => CleanUninstallPreflightState::Proceed,
        Some(LifecycleState::Prepared) => unreachable!("prepared record returned above"),
        None if registry_known => CleanUninstallPreflightState::NoBinding,
        None => return Err("installation identity census is unreadable".into()),
    };
    let journal_path = root_record
        .as_ref()
        .map(|record| record.journal_token.to_path_buf())
        .or(config_journal);
    Ok(CleanUninstallPreflight {
        state,
        protected_journals,
        registry_known,
        journal_path,
        root_record,
    })
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
    if args.skip_path {
        incompatible.push("--skip-path");
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
        "solstone journal setup --clean-uninstall will remove these runtime artifacts:".into(),
        String::new(),
    ];
    if let Err(error) = &service {
        lines.push(format!("  service location unavailable: {error}"));
    }
    if let Ok(Some(path)) = service {
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
            "  [retain ] journal manifest: {}",
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
    let service = match service_artifact_path(&context.home_dir) {
        Ok(service) => service,
        Err(_) => return true, // Run must report this failure rather than claim nothing needs removal.
    };
    let policy = |path: &Path, kind, last| {
        may_remove_cleanup_target(
            path,
            kind,
            last,
            context.registry_known,
            &context.protected_journals,
            context.platform,
        ) == CleanupTargetDecision::Remove
    };
    let windows_managed = if context.platform == PlatformTag::Windows {
        let Some(task_xml) = service.as_deref() else {
            return true;
        };
        let Some(service_directory) = task_xml.parent() else {
            return true;
        };
        let Some(owner_directory) = service_directory.parent() else {
            return true;
        };
        let namespace = context.plan.binding.namespace.to_string();
        let after_update = task_xml.with_file_name(format!("{namespace}.after-update.json"));
        let resume_script = owner_directory.join(format!(
            "journal-resume-{}.vbs",
            context.plan.binding.id.as_hex()
        ));
        let per_install = [&after_update, &resume_script]
            .iter()
            .any(|path| present(path) && policy(path, CleanupTargetKind::PerInstall, false));
        let shared = std::env::var_os("LOCALAPPDATA").is_some_and(|local_app_data| {
            has_removable_windows_app_state(
                &PathBuf::from(local_app_data).join("solstone-journal"),
                context,
            )
        });
        per_install || shared
    } else {
        false
    };
    service.as_ref().is_some_and(|path| {
        present(path)
            && artifact_evidence_matches_plan(context)
            && policy(path, CleanupTargetKind::PerInstall, false)
    }) || [wrappers.solstone, wrappers.journal]
        .iter()
        .any(|path| present(path) && policy(path, CleanupTargetKind::PerInstall, false))
        || (context.plan.remove_owner_config
            && present(&context.config_path)
            && policy(&context.config_path, CleanupTargetKind::Shared, true))
        || (context.platform == PlatformTag::Linux && {
            let path = context.home_dir.join(".cache/solstone/rclone");
            present(&path) && policy(&path, CleanupTargetKind::Shared, true)
        })
        || {
            let path = crate::package_install_receipt_path(&context.home_dir);
            present(&path) && policy(&path, CleanupTargetKind::Shared, true)
        }
        || {
            let path = WrapperEnvironment {
                home_dir: context.home_dir.clone(),
                curdir: context.curdir.clone(),
                executable_dir: context.executable_dir.clone(),
                backup_dir: None,
                legacy_replacement: false,
            }
            .backup_dir();
            present(&path) && policy(&path, CleanupTargetKind::Shared, true)
        }
        || user_skill_paths(&context.home_dir)
            .iter()
            .any(|path| present(path) && policy(path, CleanupTargetKind::PerInstall, false))
        || windows_managed
}

fn has_removable_windows_app_state(
    state_directory: &Path,
    context: &CleanUninstallContext<'_>,
) -> bool {
    [
        "journal-app.json",
        "journal-app.json.tmp",
        "journal-mark.ico",
        "journal-app-webview",
    ]
    .iter()
    .any(|name| {
        let path = state_directory.join(name);
        present(&path)
            && may_remove_cleanup_target(
                &path,
                CleanupTargetKind::Shared,
                context.plan.remove_owner_config,
                context.registry_known,
                &context.protected_journals,
                context.platform,
            ) == CleanupTargetDecision::Remove
    })
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
        mark: CleanUninstallMark::None,
    }
}

fn marked_result(
    name: &'static str,
    state: CleanUninstallState,
    path: Option<PathBuf>,
    reason: Option<String>,
    mark: CleanUninstallMark,
) -> CleanUninstallStepResult {
    CleanUninstallStepResult {
        name,
        state,
        path,
        reason,
        mark,
    }
}

fn mark_for_decision(decision: CleanupTargetDecision) -> Option<CleanUninstallMark> {
    match decision {
        CleanupTargetDecision::Remove => None,
        CleanupTargetDecision::Skip(CleanupSkip::ProtectedJournal) => {
            Some(CleanUninstallMark::ProtectedJournal)
        }
        CleanupTargetDecision::Skip(CleanupSkip::AnotherInstallation) => {
            Some(CleanUninstallMark::AnotherInstallation)
        }
        CleanupTargetDecision::Skip(CleanupSkip::RegistryUnreadable) => {
            Some(CleanUninstallMark::RegistryUnreadable)
        }
    }
}

fn preserve_for_decision(
    name: &'static str,
    path: PathBuf,
    decision: CleanupTargetDecision,
) -> CleanUninstallStepResult {
    let mark = mark_for_decision(decision).unwrap_or(CleanUninstallMark::None);
    marked_result(
        name,
        CleanUninstallState::Preserved,
        Some(path),
        Some(format!("preserved: {mark:?}")),
        mark,
    )
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
    if let Err(reason) = (context.identity_hold)(IdentityHold::Release) {
        return result("service", CleanUninstallState::Failed, path, Some(reason));
    }
    let output = context.runner.run(&CommandRequest {
        program: solstone_executable(&context.executable_dir),
        args: vec!["journal".into(), "service".into(), "uninstall".into()],
        timeout_seconds: None,
    });
    match output {
        Err(error) => return result("service", CleanUninstallState::Failed, path, Some(error)),
        Ok(output) if output.exit_code != 0 => {
            return result(
                "service",
                CleanUninstallState::Failed,
                path,
                Some(child_failure_reason(&output)),
            );
        }
        Ok(_) => {}
    }
    if let Err(reason) = (context.identity_hold)(IdentityHold::Reacquire) {
        return result("service", CleanUninstallState::Failed, path, Some(reason));
    }
    if !existed {
        return result("service", CleanUninstallState::AlreadyAbsent, path, None);
    }
    let path_ref = path.as_ref().expect("existing service path");
    let decision = may_remove_cleanup_target(
        path_ref,
        CleanupTargetKind::PerInstall,
        false,
        context.registry_known,
        &context.protected_journals,
        context.platform,
    );
    if decision != CleanupTargetDecision::Remove {
        return preserve_for_decision("service", path.expect("existing service path"), decision);
    }
    match path.as_ref().map(fs::remove_file) {
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
        let mut protected = None;
        for path in [&paths.0, &paths.1] {
            let decision = may_remove_cleanup_target(
                path,
                CleanupTargetKind::PerInstall,
                false,
                context.registry_known,
                &context.protected_journals,
                context.platform,
            );
            if decision != CleanupTargetDecision::Remove {
                protected.get_or_insert_with(|| (path.clone(), decision));
                continue;
            }
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
        if let Some((path, decision)) = protected {
            return preserve_for_decision("wrapper", path, decision);
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
    let environment = WrapperEnvironment {
        home_dir: context.home_dir.clone(),
        curdir: context.curdir.clone(),
        executable_dir: context.executable_dir.clone(),
        backup_dir: None,
        legacy_replacement: false,
    };
    let existed = present(&paths.0) || present(&paths.1);
    let protected = [&paths.0, &paths.1]
        .iter()
        .find(|path| {
            may_remove_cleanup_target(
                path,
                CleanupTargetKind::PerInstall,
                false,
                context.registry_known,
                &context.protected_journals,
                context.platform,
            ) != CleanupTargetDecision::Remove
        })
        .copied()
        .cloned();
    match uninstall_wrappers(&environment, &context.protected_journals) {
        Ok(()) => match protected {
            Some(path) => preserve_for_decision(
                "wrapper",
                path.clone(),
                may_remove_cleanup_target(
                    &path,
                    CleanupTargetKind::PerInstall,
                    false,
                    context.registry_known,
                    &context.protected_journals,
                    context.platform,
                ),
            ),
            None => result(
                "wrapper",
                if existed {
                    CleanUninstallState::Removed
                } else {
                    CleanUninstallState::AlreadyAbsent
                },
                Some(paths.0.clone()),
                None,
            ),
        },
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

fn remove_file_target(
    name: &'static str,
    path: PathBuf,
    kind: CleanupTargetKind,
    last_installation: bool,
    context: &CleanUninstallContext<'_>,
) -> CleanUninstallStepResult {
    if !present(&path) {
        return result(name, CleanUninstallState::AlreadyAbsent, Some(path), None);
    }
    let decision = may_remove_cleanup_target(
        &path,
        kind,
        last_installation,
        context.registry_known,
        &context.protected_journals,
        context.platform,
    );
    if decision != CleanupTargetDecision::Remove {
        return preserve_for_decision(name, path, decision);
    }
    remove_path(name, path)
}

fn remove_shared_directory(
    name: &'static str,
    path: PathBuf,
    context: &CleanUninstallContext<'_>,
) -> CleanUninstallStepResult {
    if !present(&path) {
        return result(name, CleanUninstallState::AlreadyAbsent, Some(path), None);
    }
    let decision = may_remove_cleanup_target(
        &path,
        CleanupTargetKind::Shared,
        context.plan.remove_owner_config,
        context.registry_known,
        &context.protected_journals,
        context.platform,
    );
    if decision != CleanupTargetDecision::Remove {
        return preserve_for_decision(name, path, decision);
    }
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() => marked_result(
            name,
            CleanUninstallState::Preserved,
            Some(path),
            Some("preserved symlink".into()),
            CleanUninstallMark::Foreign,
        ),
        Ok(metadata) if metadata.is_dir() => match fs::remove_dir_all(&path) {
            Ok(()) => result(name, CleanUninstallState::Removed, Some(path), None),
            Err(error) => result(
                name,
                CleanUninstallState::Failed,
                Some(path),
                Some(error.to_string()),
            ),
        },
        Ok(_) => marked_result(
            name,
            CleanUninstallState::Preserved,
            Some(path),
            Some("preserved non-directory target".into()),
            CleanUninstallMark::Foreign,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            result(name, CleanUninstallState::AlreadyAbsent, Some(path), None)
        }
        Err(error) => result(
            name,
            CleanUninstallState::Failed,
            Some(path),
            Some(error.to_string()),
        ),
    }
}

fn remove_manifest(context: &CleanUninstallContext<'_>) -> CleanUninstallStepResult {
    marked_result(
        "manifest",
        CleanUninstallState::Preserved,
        Some(context.manifest_path.clone()),
        Some("journal manifest is preserved".into()),
        CleanUninstallMark::ProtectedJournal,
    )
}

fn user_skill_paths(home: &Path) -> [PathBuf; 3] {
    [
        home.join(".claude/skills/solstone"),
        home.join(".codex/skills/solstone"),
        home.join(".gemini/skills/solstone"),
    ]
}

fn remove_user_skill(
    path: PathBuf,
    context: &CleanUninstallContext<'_>,
) -> CleanUninstallStepResult {
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return result(
                "user-skill",
                CleanUninstallState::AlreadyAbsent,
                Some(path),
                None,
            );
        }
        Err(error) => {
            return result(
                "user-skill",
                CleanUninstallState::Failed,
                Some(path),
                Some(error.to_string()),
            );
        }
    };
    if !metadata.file_type().is_symlink() {
        return marked_result(
            "user-skill",
            CleanUninstallState::Preserved,
            Some(path),
            Some("user-authored skill content is preserved".into()),
            CleanUninstallMark::Foreign,
        );
    }
    let link_target = match fs::read_link(&path) {
        Ok(target) if target.is_absolute() => target,
        Ok(target) => path.parent().unwrap_or_else(|| Path::new(".")).join(target),
        Err(error) => {
            return result(
                "user-skill",
                CleanUninstallState::Failed,
                Some(path),
                Some(error.to_string()),
            );
        }
    };
    if !same_protected_place(
        &link_target,
        &context.bundled_user_skill,
        context.platform == PlatformTag::Windows,
    ) {
        return marked_result(
            "user-skill",
            CleanUninstallState::Preserved,
            Some(path),
            Some("symlink target is foreign".into()),
            CleanUninstallMark::Foreign,
        );
    }
    let decision = may_remove_cleanup_target(
        &path,
        CleanupTargetKind::PerInstall,
        false,
        context.registry_known,
        &context.protected_journals,
        context.platform,
    );
    if decision != CleanupTargetDecision::Remove {
        return preserve_for_decision("user-skill", path, decision);
    }
    match fs::remove_file(&path) {
        Ok(()) => result("user-skill", CleanUninstallState::Removed, Some(path), None),
        Err(error) => result(
            "user-skill",
            CleanUninstallState::Failed,
            Some(path),
            Some(error.to_string()),
        ),
    }
}

fn skipped_result(name: &'static str, path: Option<PathBuf>) -> CleanUninstallStepResult {
    marked_result(
        name,
        CleanUninstallState::Skipped,
        path,
        Some("step was not run after an earlier failure".into()),
        CleanUninstallMark::NotRun,
    )
}

fn outcome(
    context: &CleanUninstallContext<'_>,
    exit_code: i32,
    message: String,
    results: Vec<CleanUninstallStepResult>,
    already_complete: bool,
) -> CleanUninstallOutcome {
    CleanUninstallOutcome {
        exit_code,
        message,
        results,
        journal_path: Some(context.journal_path.clone()),
        already_complete,
    }
}

#[must_use]
pub fn macos_preserved_results(platform: PlatformTag) -> Vec<CleanUninstallStepResult> {
    if platform != PlatformTag::Macos {
        return Vec::new();
    }
    [
        "journal-preferences",
        "journal-handoff",
        "journal-app-support",
    ]
    .into_iter()
    .map(|name| {
        marked_result(
            name,
            CleanUninstallState::Preserved,
            None,
            Some("no owned path is known".into()),
            CleanUninstallMark::NoOwnedPath,
        )
    })
    .collect()
}

fn cleanup_step_inventory(
    context: &CleanUninstallContext<'_>,
    service_path: Option<PathBuf>,
) -> Vec<(&'static str, Option<PathBuf>)> {
    let wrappers = wrapper_paths(&context.home_dir);
    let backup_dir = WrapperEnvironment {
        home_dir: context.home_dir.clone(),
        curdir: context.curdir.clone(),
        executable_dir: context.executable_dir.clone(),
        backup_dir: None,
        legacy_replacement: false,
    }
    .backup_dir();
    vec![
        ("service", service_path),
        ("wrapper", Some(wrappers.solstone)),
        ("config", Some(context.config_path.clone())),
        ("manifest", Some(context.manifest_path.clone())),
        (
            "rclone",
            (context.platform == PlatformTag::Linux)
                .then(|| context.home_dir.join(".cache/solstone/rclone")),
        ),
        (
            "package-receipt",
            Some(crate::package_install_receipt_path(&context.home_dir)),
        ),
        ("setup-backups", Some(backup_dir)),
        (
            "user-skill",
            Some(user_skill_paths(&context.home_dir)[0].clone()),
        ),
        (
            "user-skill",
            Some(user_skill_paths(&context.home_dir)[1].clone()),
        ),
        (
            "user-skill",
            Some(user_skill_paths(&context.home_dir)[2].clone()),
        ),
    ]
}

fn stop_after_failed_step(
    context: &CleanUninstallContext<'_>,
    mut results: Vec<CleanUninstallStepResult>,
    failed_step_index: usize,
    service_path: Option<PathBuf>,
) -> CleanUninstallOutcome {
    results.extend(
        cleanup_step_inventory(context, service_path)
            .into_iter()
            .skip(failed_step_index + 1)
            .map(|(name, path)| skipped_result(name, path)),
    );
    outcome(
        context,
        1,
        "clean uninstall stopped after a failed step".into(),
        results,
        false,
    )
}

pub fn run_clean_uninstall(context: &mut CleanUninstallContext<'_>) -> CleanUninstallOutcome {
    let service = match service_artifact_path(&context.home_dir) {
        Ok(service) => service,
        Err(error) => {
            let results = vec![result(
                "service",
                CleanUninstallState::Failed,
                None,
                Some(error.to_string()),
            )];
            let mut failed = stop_after_failed_step(context, results, 0, None);
            failed.message = format!("service location unavailable: {error}");
            return failed;
        }
    };
    let wrappers = wrapper_paths(&context.home_dir);
    if !clean_uninstall_has_managed_paths(context) {
        let mut results = macos_preserved_results(context.platform);
        return outcome(
            context,
            0,
            "nothing to remove (all paths already absent)".into(),
            std::mem::take(&mut results),
            false,
        );
    }
    if !context.yes && !context.stdin_is_tty {
        return outcome(
            context,
            2,
            "not a tty; rerun with --yes to proceed non-interactively (cancelled)".into(),
            Vec::new(),
            false,
        );
    }
    if !context.yes && !(context.confirm)() {
        return outcome(context, 1, "cancelled".into(), Vec::new(), false);
    }
    let service_result = remove_service(context, service.clone());
    let service_failed = service_result.state == CleanUninstallState::Failed;
    let mut results = vec![service_result];
    if service_failed {
        return stop_after_failed_step(context, results, 0, service);
    }
    let wrapper_result = remove_wrappers(
        context,
        &(wrappers.solstone.clone(), wrappers.journal.clone()),
    );
    let wrapper_failed = wrapper_result.state == CleanUninstallState::Failed;
    results.push(wrapper_result);
    if wrapper_failed {
        return stop_after_failed_step(context, results, 1, service);
    }
    let config_result = remove_file_target(
        "config",
        context.config_path.clone(),
        CleanupTargetKind::Shared,
        context.plan.remove_owner_config,
        context,
    );
    let config_failed = config_result.state == CleanUninstallState::Failed;
    results.push(config_result);
    if config_failed {
        return stop_after_failed_step(context, results, 2, service);
    }
    let manifest_result = remove_manifest(context);
    let manifest_failed = manifest_result.state == CleanUninstallState::Failed;
    results.push(manifest_result);
    if manifest_failed {
        return stop_after_failed_step(context, results, 3, service);
    }
    if context.platform == PlatformTag::Linux {
        let rclone_result = remove_shared_directory(
            "rclone",
            context.home_dir.join(".cache/solstone/rclone"),
            context,
        );
        let rclone_failed = rclone_result.state == CleanUninstallState::Failed;
        results.push(rclone_result);
        if rclone_failed {
            return stop_after_failed_step(context, results, 4, service);
        }
    } else {
        results.push(marked_result(
            "rclone",
            CleanUninstallState::Preserved,
            None,
            Some("no owned path on this platform".into()),
            CleanUninstallMark::NoOwnedPath,
        ));
    }
    let package_result = remove_file_target(
        "package-receipt",
        crate::package_install_receipt_path(&context.home_dir),
        CleanupTargetKind::Shared,
        context.plan.remove_owner_config,
        context,
    );
    let package_failed = package_result.state == CleanUninstallState::Failed;
    results.push(package_result);
    if package_failed {
        return stop_after_failed_step(context, results, 5, service);
    }
    let backup_dir = WrapperEnvironment {
        home_dir: context.home_dir.clone(),
        curdir: context.curdir.clone(),
        executable_dir: context.executable_dir.clone(),
        backup_dir: None,
        legacy_replacement: false,
    }
    .backup_dir();
    let backups_result = remove_shared_directory("setup-backups", backup_dir, context);
    let backups_failed = backups_result.state == CleanUninstallState::Failed;
    results.push(backups_result);
    if backups_failed {
        return stop_after_failed_step(context, results, 6, service);
    }
    for (index, path) in user_skill_paths(&context.home_dir).into_iter().enumerate() {
        let skill_result = remove_user_skill(path, context);
        let skill_failed = skill_result.state == CleanUninstallState::Failed;
        results.push(skill_result);
        if skill_failed {
            return stop_after_failed_step(context, results, 7 + index, service);
        }
    }
    let failed = results
        .iter()
        .any(|result| result.state == CleanUninstallState::Failed);
    if !failed {
        results.extend(macos_preserved_results(context.platform));
    }
    let counts = |state| {
        results
            .iter()
            .filter(|result| result.state == state)
            .count()
    };
    let message = format!(
        "clean uninstall complete: {} removed, {} already-absent, {} preserved, {} skipped, {} failed",
        counts(CleanUninstallState::Removed),
        counts(CleanUninstallState::AlreadyAbsent),
        counts(CleanUninstallState::Preserved),
        counts(CleanUninstallState::Skipped),
        counts(CleanUninstallState::Failed)
    );
    outcome(context, i32::from(failed), message, results, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::parse_args_at;
    use crate::identity_evidence::gather_artifact_evidence;
    use crate::manifest::manifest_path;
    use crate::user_config::{config_path, write_user_config};
    use crate::wrapper::{WrapperCommand, render_wrapper, wrapper_paths};
    use solstone_core_installation_identity::{
        CleanUninstallRequest, LegacyManifestEvidence, OwnerBase, PlatformTag,
        SetupAdmissionRequest, admit_clean_uninstall, admit_setup, journal_token_from_path,
        root_token_from_path,
    };
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::ffi::OsString;
    use std::ops::Deref;
    use std::rc::Rc;

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
        let journal_token = binding.journal_token.clone();
        CleanUninstallPlan {
            binding,
            remove_owner_config: true,
            already_tombstoned: false,
            protected_journals: vec![journal_token],
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
    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new(name: &str) -> Self {
            let root = PathBuf::from("/var/tmp")
                .join(format!("solstone-clean-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }

    impl Deref for TestRoot {
        type Target = Path;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn root(name: &str) -> TestRoot {
        TestRoot::new(name)
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
            protected_journals: ProtectedJournals::new(PlatformTag::Linux),
            registry_known: true,
            platform: PlatformTag::Linux,
            bundled_user_skill: PathBuf::from("/bundled/solstone/talent/solstone"),
            artifact_evidence: ArtifactBindingEvidence::Fresh,
            curdir: root.join("repo"),
            executable_dir: root.join("bin"),
            yes: false,
            stdin_is_tty: false,
            confirm: &mut confirm,
            runner: &mut runner,
            identity_hold: &mut |_| Ok(()),
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
            protected_journals: ProtectedJournals::new(PlatformTag::Linux),
            registry_known: true,
            platform: PlatformTag::Linux,
            bundled_user_skill: PathBuf::from("/bundled/solstone/talent/solstone"),
            artifact_evidence: ArtifactBindingEvidence::Fresh,
            curdir: root.join("repo"),
            executable_dir: root.join("bin"),
            yes: false,
            stdin_is_tty: true,
            confirm: &mut confirm,
            runner: &mut runner,
            identity_hold: &mut |_| Ok(()),
        };
        let outcome = run_clean_uninstall(&mut context);
        assert_eq!(outcome.exit_code, 1);
        assert_eq!(outcome.message, "cancelled");
        assert!(outcome.results.is_empty());
    }
    struct RecordingRunner(Rc<RefCell<Vec<&'static str>>>, i32);
    impl CommandRunner for RecordingRunner {
        fn run(&mut self, request: &CommandRequest) -> Result<crate::steps::CommandOutput, String> {
            assert_eq!(request.args, ["journal", "service", "uninstall"]);
            self.0.borrow_mut().push("child");
            Ok(crate::steps::CommandOutput {
                exit_code: self.1,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
            })
        }
    }

    fn held_context<'a>(
        root: &Path,
        runner: &'a mut RecordingRunner,
        confirm: &'a mut dyn FnMut() -> bool,
        identity_hold: &'a mut dyn FnMut(IdentityHold) -> Result<(), String>,
    ) -> CleanUninstallContext<'a> {
        CleanUninstallContext {
            journal_path: root.join("journal"),
            home_dir: root.join("home"),
            config_path: root.join("config.toml"),
            manifest_path: root.join("journal/health/setup-state.json"),
            plan: plan(),
            protected_journals: ProtectedJournals::new(PlatformTag::Linux),
            registry_known: true,
            platform: PlatformTag::Linux,
            bundled_user_skill: PathBuf::from("/bundled/solstone/talent/solstone"),
            artifact_evidence: ArtifactBindingEvidence::Fresh,
            curdir: root.join("repo"),
            executable_dir: root.join("bin"),
            yes: true,
            stdin_is_tty: false,
            confirm,
            runner,
            identity_hold,
        }
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn temporary_preferences_alone_are_managed_only_when_removable() {
        let path = std::env::temp_dir().join(format!(
            "solstone-clean-temp-preferences-{}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        let root = TestRoot(path);
        let state = root.join("state");
        fs::create_dir(&state).unwrap();
        let temp = state.join("journal-app.json.tmp");
        fs::write(&temp, "temporary preferences").unwrap();
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut runner = RecordingRunner(events, 0);
        let mut confirm = || true;
        let mut hold = |_| Ok(());
        let mut context = held_context(&root, &mut runner, &mut confirm, &mut hold);
        context.platform = PlatformTag::Windows;
        context.protected_journals = ProtectedJournals::new(PlatformTag::Windows);
        assert!(has_removable_windows_app_state(&state, &context));
        context.plan.remove_owner_config = false;
        assert!(!has_removable_windows_app_state(&state, &context));
        context.plan.remove_owner_config = true;
        context.protected_journals.insert(state.clone());
        assert!(!has_removable_windows_app_state(&state, &context));
        assert_eq!(fs::read_to_string(temp).unwrap(), "temporary preferences");
    }

    // The Windows service child reloads the binding under the identity locks: the parent
    // must not hold them while it waits, and must hold them again before removing more.
    #[test]
    fn the_service_child_runs_while_the_identity_hold_is_released() {
        let root = root("hold-order");
        fs::write(root.join("config.toml"), "journal = \"x\"\n").unwrap();
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut runner = RecordingRunner(events.clone(), 0);
        let mut confirm = || true;
        let held = events.clone();
        let mut hold = move |hold: IdentityHold| {
            held.borrow_mut().push(match hold {
                IdentityHold::Release => "release",
                IdentityHold::Reacquire => "reacquire",
            });
            Ok(())
        };
        let outcome = run_clean_uninstall(&mut held_context(
            &root,
            &mut runner,
            &mut confirm,
            &mut hold,
        ));
        assert_eq!(*events.borrow(), ["release", "child", "reacquire"]);
        assert_eq!(outcome.exit_code, 0, "{outcome:?}");
        assert!(!root.join("config.toml").exists());
    }

    #[test]
    fn a_refused_reacquire_or_a_failed_child_removes_nothing_more() {
        for (child_exit, refuse) in [(0, true), (1, false)] {
            let root = root(&format!("hold-refused-{child_exit}"));
            fs::write(root.join("config.toml"), "journal = \"x\"\n").unwrap();
            let events = Rc::new(RefCell::new(Vec::new()));
            let mut runner = RecordingRunner(events.clone(), child_exit);
            let mut confirm = || true;
            let held = events.clone();
            let mut hold = move |hold: IdentityHold| match hold {
                IdentityHold::Release => {
                    held.borrow_mut().push("release");
                    Ok(())
                }
                IdentityHold::Reacquire => {
                    held.borrow_mut().push("reacquire");
                    if refuse {
                        Err("installation changed while its service was being removed".into())
                    } else {
                        Ok(())
                    }
                }
            };
            let outcome = run_clean_uninstall(&mut held_context(
                &root,
                &mut runner,
                &mut confirm,
                &mut hold,
            ));
            assert_eq!(outcome.exit_code, 1);
            assert_eq!(outcome.results[0].state, CleanUninstallState::Failed);
            assert!(
                outcome.results[1..]
                    .iter()
                    .all(|result| result.state == CleanUninstallState::Skipped)
            );
            assert!(root.join("config.toml").exists());
            let expected: &[&str] = if refuse {
                &["release", "child", "reacquire"]
            } else {
                &["release", "child"]
            };
            assert_eq!(*events.borrow(), expected);
        }
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
            protected_journals: ProtectedJournals::new(PlatformTag::Linux),
            registry_known: true,
            platform: PlatformTag::Linux,
            bundled_user_skill: PathBuf::from("/bundled/solstone/talent/solstone"),
            artifact_evidence: ArtifactBindingEvidence::Fresh,
            curdir: root.join("repo"),
            executable_dir: root.join("bin"),
            yes: true,
            stdin_is_tty: false,
            confirm: &mut confirm,
            runner: &mut runner,
            identity_hold: &mut |_| Ok(()),
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
        for (binary, command) in [
            ("solstone", WrapperCommand::Solstone),
            ("journal", WrapperCommand::Journal),
        ] {
            fs::write(
                home.join(".local/bin").join(binary),
                crate::wrapper::render_wrapper(
                    command,
                    Path::new("/journal"),
                    &runtime.join(binary),
                    &guard,
                )
                .unwrap(),
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
            protected_journals: ProtectedJournals::new(PlatformTag::Linux),
            registry_known: true,
            platform: PlatformTag::Linux,
            bundled_user_skill: PathBuf::from("/bundled/solstone/talent/solstone"),
            artifact_evidence: ArtifactBindingEvidence::Fresh,
            curdir: root.join("repo"),
            executable_dir: runtime,
            yes: true,
            stdin_is_tty: false,
            confirm: &mut confirm,
            runner: &mut runner,
            identity_hold: &mut |_| Ok(()),
        };
        let outcome = run_clean_uninstall(&mut context);
        assert_eq!(outcome.exit_code, 1);
        assert_eq!(
            outcome
                .results
                .iter()
                .map(|result| result.name)
                .collect::<Vec<_>>(),
            [
                "service",
                "wrapper",
                "config",
                "manifest",
                "rclone",
                "package-receipt",
                "setup-backups",
                "user-skill",
                "user-skill",
                "user-skill",
            ]
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
                CleanUninstallState::Skipped,
                CleanUninstallState::Skipped,
                CleanUninstallState::Skipped,
                CleanUninstallState::Skipped,
                CleanUninstallState::Skipped,
                CleanUninstallState::Skipped,
            ]
        );
        assert!(
            outcome.results[1..]
                .iter()
                .all(|result| result.mark == CleanUninstallMark::NotRun)
        );
        assert_eq!(
            outcome.results[0].reason.as_deref(),
            Some(
                "service uninstall exited 7: error: launchd accepted the unload request, but the service is still present"
            )
        );
        assert!(home.join(".local/bin/solstone").exists());
        assert!(home.join(".local/bin/journal").exists());
        assert!(config.exists());
        assert!(manifest.exists());
    }

    #[test]
    fn later_failure_stops_before_every_remaining_cleanup_step() {
        use std::os::unix::fs::symlink;

        let root = root("later-failure");
        let home = root.join("home");
        let runtime = root.join("bin");
        fs::create_dir_all(&runtime).unwrap();
        let config = root.join("config.toml");
        fs::create_dir(&config).unwrap();
        let package_receipt = crate::package_install_receipt_path(&home);
        fs::create_dir_all(package_receipt.parent().unwrap()).unwrap();
        fs::write(&package_receipt, "receipt").unwrap();
        let backups = home.join(".local/share/solstone/setup-backups");
        fs::create_dir_all(&backups).unwrap();
        let backup_canary = backups.join("canary");
        fs::write(&backup_canary, "backup").unwrap();
        let bundled = root.join("install/solstone/talent/solstone");
        fs::create_dir_all(&bundled).unwrap();
        let skill = home.join(".claude/skills/solstone");
        fs::create_dir_all(skill.parent().unwrap()).unwrap();
        symlink(&bundled, &skill).unwrap();

        let mut runner = RunnerWithOutput {
            exits: VecDeque::from([0]),
            stderr: String::new(),
        };
        let mut confirm = || true;
        let mut context = CleanUninstallContext {
            journal_path: root.join("journal"),
            home_dir: home,
            config_path: config.clone(),
            manifest_path: root.join("journal/health/setup-state.json"),
            plan: plan(),
            protected_journals: ProtectedJournals::new(PlatformTag::Linux),
            registry_known: true,
            platform: PlatformTag::Linux,
            bundled_user_skill: bundled,
            artifact_evidence: ArtifactBindingEvidence::Fresh,
            curdir: root.join("repo"),
            executable_dir: runtime,
            yes: true,
            stdin_is_tty: false,
            confirm: &mut confirm,
            runner: &mut runner,
            identity_hold: &mut |_| Ok(()),
        };

        let outcome = run_clean_uninstall(&mut context);
        assert_eq!(outcome.exit_code, 1);
        let config_index = outcome
            .results
            .iter()
            .position(|result| result.name == "config")
            .unwrap();
        assert_eq!(
            outcome.results[config_index].state,
            CleanUninstallState::Failed
        );
        assert!(outcome.results[config_index + 1..].iter().all(|result| {
            result.state == CleanUninstallState::Skipped
                && result.mark == CleanUninstallMark::NotRun
        }));
        assert!(config.is_dir());
        assert!(package_receipt.exists());
        assert!(backup_canary.exists());
        assert!(skill.is_symlink());
    }

    #[test]
    fn matching_guarded_wrappers_are_removed_even_when_their_binary_is_no_longer_current() {
        let root = root("guarded-wrapper");
        let home = root.join("home");
        let runtime = root.join("new-bin");
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        fs::create_dir_all(&runtime).unwrap();
        let guard = GuardFields::from_binding(&plan().binding);
        for (binary, command) in [
            ("solstone", WrapperCommand::Solstone),
            ("journal", WrapperCommand::Journal),
        ] {
            fs::write(
                home.join(".local/bin").join(binary),
                crate::wrapper::render_wrapper(
                    command,
                    Path::new("/journal"),
                    &root.join("old-bin").join(binary),
                    &guard,
                )
                .unwrap(),
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
            protected_journals: ProtectedJournals::new(PlatformTag::Linux),
            registry_known: true,
            platform: PlatformTag::Linux,
            bundled_user_skill: PathBuf::from("/bundled/solstone/talent/solstone"),
            artifact_evidence: ArtifactBindingEvidence::Guarded(guard),
            curdir: root.join("repo"),
            executable_dir: runtime,
            yes: true,
            stdin_is_tty: false,
            confirm: &mut confirm,
            runner: &mut runner,
            identity_hold: &mut |_| Ok(()),
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
            protected_journals: ProtectedJournals::new(PlatformTag::Linux),
            registry_known: true,
            platform: PlatformTag::Linux,
            bundled_user_skill: PathBuf::from("/bundled/solstone/talent/solstone"),
            artifact_evidence,
            curdir: root.to_path_buf(),
            executable_dir: root.join("bin"),
            yes: true,
            stdin_is_tty: false,
            confirm,
            runner,
            identity_hold: Box::leak(Box::new(|_| Ok(()))),
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
                WrapperCommand::Solstone,
                &second.journal_token.to_path_buf(),
                &root.join("bin/solstone"),
                &second_guard,
            )
            .unwrap();
            let second_journal = render_wrapper(
                WrapperCommand::Journal,
                &second.journal_token.to_path_buf(),
                &root.join("bin/journal"),
                &second_guard,
            )
            .unwrap();
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
            assert!(first_manifest.exists());

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
            assert!(manifest_b.exists());
            assert!(manifest_a.exists());

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
        let mut context = CleanUninstallContext {
            journal_path: root.join("journal"),
            home_dir: home.clone(),
            config_path: config.clone(),
            manifest_path: manifest.clone(),
            plan: plan(),
            protected_journals: ProtectedJournals::new(PlatformTag::Linux),
            registry_known: true,
            platform: PlatformTag::Linux,
            bundled_user_skill: PathBuf::from("/bundled/solstone/talent/solstone"),
            artifact_evidence: ArtifactBindingEvidence::Fresh,
            curdir: root.join("repo"),
            executable_dir: root.join("bin"),
            yes: false,
            stdin_is_tty: true,
            confirm: &mut confirm,
            runner: &mut runner,
            identity_hold: &mut |_| Ok(()),
        };
        let _ = clean_uninstall_confirmation_lines(&context);
        assert!(clean_uninstall_has_managed_paths(&context));
        fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        fs::write(&manifest, "manifest").unwrap();
        let outcome = run_clean_uninstall(&mut context);
        assert_eq!(outcome.journal_path, Some(context.journal_path.clone()));
        assert!(outcome.results.iter().any(|result| {
            result.name == "manifest"
                && result.state == CleanUninstallState::Preserved
                && result.mark == CleanUninstallMark::ProtectedJournal
                && result.path.as_deref() == Some(manifest.as_path())
        }));
        assert!(manifest.exists());
    }
}
