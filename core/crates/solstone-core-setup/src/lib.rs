// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Foundational types for the native `journal setup` port.

use std::env;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::{Command, ExitCode};

pub mod args;
pub mod clean_uninstall;
pub mod events;
pub mod identity_evidence;
mod legacy_launcher;
pub mod manifest;
pub mod steps;
pub mod user_config;
pub mod wrapper;

use args::{ResolutionContext, SetupArgs, resolve_mode, resolve_setup};
use clean_uninstall::{
    CleanUninstallContext, clean_uninstall_confirmation_lines, clean_uninstall_has_managed_paths,
    clean_uninstall_refusal, run_clean_uninstall,
};
use events::{EventSink, JsonlEmitter, StepName};
use identity_evidence::{
    gather_artifact_evidence, gather_setup_artifact_evidence, wrapper_targets_drifted,
};
use manifest::{legacy_manifest_evidence, manifest_path};
use solstone_core_installation_identity::{
    ArtifactBindingEvidence, CleanUninstallRequest, CleanUninstallSession, IdentityError,
    JournalToken, OwnerBase, PlatformTag, SetupAdmission, SetupAdmissionRequest,
    admit_clean_uninstall, admit_setup, admit_setup_with_effective_journal_validator,
    journal_token_from_path, load_installation_binding, namespace_name, root_token_from_path,
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

struct SetupIdentityAdmission {
    admission: SetupAdmission,
    repair_steps: Vec<StepName>,
    legacy_replacement: bool,
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

fn package_install_receipt_path(home_dir: &std::path::Path) -> PathBuf {
    home_dir.join(".local/share/solstone/package-install-receipt")
}

/// Write the package route's owner receipt after setup succeeds.
///
/// Packages deliberately have no maintainer scripts, so `/usr/bin` setup is
/// the first owner-scoped step that can record the installed route. This file
/// is diagnostic dispatch data only; package database state remains the route
/// authority.
fn write_package_install_receipt(
    home_dir: &std::path::Path,
    executable_dir: &std::path::Path,
) -> io::Result<()> {
    if !package_database_owns_journal(executable_dir) {
        return Ok(());
    }
    write_owned_package_install_receipt(home_dir)
}

fn package_database_owns_journal(executable_dir: &std::path::Path) -> bool {
    if executable_dir != std::path::Path::new("/usr/bin") {
        return false;
    }
    let journal = "/usr/bin/journal";
    if let Ok(output) = Command::new("/usr/bin/dpkg-query")
        .args(["-S", journal])
        .output()
        && output.status.success()
        && dpkg_query_names_journal_package(&String::from_utf8_lossy(&output.stdout), journal)
    {
        return true;
    }
    Command::new("/usr/bin/rpm")
        .args(["-qf", "--qf", "%{NAME}\n", journal])
        .output()
        .is_ok_and(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).trim() == "solstone-journal"
        })
}

fn dpkg_query_names_journal_package(output: &str, journal: &str) -> bool {
    output.lines().any(|line| {
        line.rsplit_once(": ").is_some_and(|(package, path)| {
            path == journal && package.split(':').next() == Some("solstone-journal")
        })
    })
}

fn write_owned_package_install_receipt(home_dir: &std::path::Path) -> io::Result<()> {
    let path = package_install_receipt_path(home_dir);
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("package receipt has no parent"))?;
    let mut directory = home_dir.to_path_buf();
    for component in [".local", "share", "solstone"] {
        directory.push(component);
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(io::Error::other(format!(
                    "package receipt directory is not a real directory: {}",
                    directory.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&directory)?;
            }
            Err(error) => return Err(error),
        }
    }
    debug_assert_eq!(directory, parent);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(io::Error::other(
                "package receipt target is not a regular file",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let architecture = match env::consts::ARCH {
        "x86_64" => "linux-x86_64",
        "aarch64" => "linux-aarch64",
        other => other,
    };
    // `/usr/bin` selects the package-route receipt location, but it does not
    // prove which package manager (if any) owns this executable. Keep every
    // unobserved provenance field conservative; the package database remains
    // authoritative when the installer later detects a package route.
    let contents = format!(
        "schema_version=1\njournal_version={}\nlane=unknown\norigin=unknown\narchitecture={}\ninstaller_revision=unknown\nroute=package\nsignature_verification=unverified\n",
        env!("CARGO_PKG_VERSION"),
        architecture,
    );
    for attempt in 0..16_u8 {
        let partial = parent.join(format!(
            ".package-install-receipt-{}-{attempt}",
            std::process::id()
        ));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        if let Err(error) = file
            .write_all(contents.as_bytes())
            .and_then(|()| file.sync_all())
            .and_then(|()| fs::rename(&partial, &path))
        {
            let _ = fs::remove_file(partial);
            return Err(error);
        }
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate package receipt staging file",
    ))
}

fn identity_error_code(error: &IdentityError) -> events::ErrorCode {
    match error {
        IdentityError::Io { .. } => events::ErrorCode::InstallationIdentityUnavailable,
        _ => events::ErrorCode::InstallationIdentityRefused,
    }
}

/// The setup artifacts an owner must clear to recover from a refused admission.
///
/// ⚠ Every path here is created by `journal setup` and rebuilt by it. 🔒 None of
/// them holds the owner's journal, and saying so in the copy is load-bearing: an
/// owner who is not certain of that will not run the commands, and this refusal
/// is otherwise a dead end.
///
/// 🔴 The identity entry is deliberately `installation-identity/v1/namespaces/
/// <namespace>` and NEVER its parent. That directory is **owner-wide**: it holds
/// one namespace entry per installation for this user, so removing the parent
/// would destroy a second, healthy installation's record along with this one.
/// When the namespace cannot be resolved, the identity entry is omitted rather
/// than widened -- an incomplete remedy is recoverable, a destructive one is not.
fn identity_recovery_paths(
    home_dir: &std::path::Path,
    namespace: Option<&str>,
) -> Vec<(PathBuf, &'static str)> {
    let wrappers = wrapper::wrapper_paths(home_dir);
    // The `rm` flag is fixed per entry rather than probed from the filesystem, so
    // the printed remedy is the same on every host and cannot change under us
    // between rendering the message and the owner running it.
    let mut paths = vec![(wrappers.journal, "-f"), (wrappers.solstone, "-f")];
    if let Some(service) = steps::service_artifact_path(home_dir) {
        paths.push((service, "-f"));
    }
    if let Some(namespace) = namespace {
        paths.push((
            installation_identity_dir(home_dir)
                .join("v1")
                .join("namespaces")
                .join(namespace),
            "-rf",
        ));
    }
    paths
}

fn installation_identity_dir(home_dir: &std::path::Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home_dir.join("Library/Application Support/solstone/installation-identity")
    } else {
        home_dir.join(".local/share/solstone/installation-identity")
    }
}

/// This installation's identity namespace, derived exactly as admission derives it.
fn setup_identity_namespace(
    executable_dir: &std::path::Path,
    project_root: &std::path::Path,
) -> Option<String> {
    let root = resolve_identity_root(executable_dir, project_root);
    let root_token = root_token_from_path(&root).ok()?;
    Some(namespace_name(PlatformTag::current(), &root_token).to_string())
}

fn report_identity_failure<W: Write>(
    jsonl: bool,
    stdout: &mut W,
    stderr: &mut impl Write,
    error: &IdentityError,
    home_dir: &std::path::Path,
    namespace: Option<&str>,
) -> ExitCode {
    let code = identity_error_code(error);
    let recovery = identity_recovery_paths(home_dir, namespace);
    // Shape follows the locked recovery copy in
    // `vpx/design-system/journal-service-install-recovery-copy.md`: the owner-visible
    // line first, the internal reason under `details:`.
    //
    // ⚠ It deliberately DIVERGES from that lock's prescribed second line ("run
    // `journal setup` to check it. if setup finishes successfully, try again"),
    // because for this particular refusal that instruction is a loop -- the
    // admission is deterministic, so re-running setup fails identically. ⛔ And
    // `--clean-uninstall` is not the way out either: `admit_clean_uninstall` runs
    // against the same registry and answers "installation identity does not
    // exist" in exactly this state. An amendment adding a row for the
    // no-bootstrap-evidence state is filed with VPX; until it lands this string
    // and the lock are knowingly in tension, which is better than a loop.
    let steps = recovery
        .iter()
        .map(|(path, flag)| format!("\n    rm {flag} {}", path.display()))
        .collect::<String>();
    // ⚠ Platform-specific, and it must follow the same cfg as
    // `service_artifact_path` -- on macOS that path is a LaunchAgent plist, so a
    // `systemctl` line here would tell a Mac owner to run a command their system
    // does not have, next to an `rm` of a plist.
    let stop_service = if steps::service_artifact_path(home_dir).is_none() {
        ""
    } else if cfg!(target_os = "macos") {
        "\n    launchctl bootout gui/$(id -u)/org.solpbc.solstone"
    } else {
        "\n    systemctl --user disable --now solstone.service"
    };
    let message = format!(
        "this installation couldn't be verified.\n\ndetails: {error}\n\nto recover, stop the service, remove this installation's setup files, and run `journal setup` again:{stop_service}{steps}\n\nyour journal itself is untouched. none of these holds your memories."
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
                        "remedy": recovery
                            .iter()
                            .map(|(path, _)| path.display().to_string())
                            .collect::<Vec<_>>(),
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

fn allows_recognized_legacy_path_shadow(
    platform: PlatformTag,
    executable_dir: &std::path::Path,
    artifacts: &ArtifactBindingEvidence,
    legacy_transition: bool,
) -> bool {
    matches!(platform, PlatformTag::Linux)
        && executable_dir == std::path::Path::new("/usr/bin")
        && legacy_transition
        && matches!(artifacts, ArtifactBindingEvidence::LegacyUnguarded)
}

fn admit_setup_identity(
    home_dir: &std::path::Path,
    executable_dir: &std::path::Path,
    project_root: &std::path::Path,
    resolved: &args::ResolvedSetup,
    validate_wrapper_journal: bool,
) -> Result<SetupIdentityAdmission, IdentityError> {
    let root = resolve_identity_root(executable_dir, project_root);
    let root_token = root_token_from_path(&root)?;
    let namespace = namespace_name(PlatformTag::current(), &root_token);
    let manifest = legacy_manifest_evidence(&manifest_path(&resolved.journal_path));
    let owner = OwnerBase::at_home(home_dir.to_path_buf(), PlatformTag::current())?;
    let admitted_retry = load_installation_binding(&owner, &root_token).is_ok();
    let artifacts = gather_setup_artifact_evidence(
        home_dir,
        &namespace,
        admitted_retry
            || matches!(
                manifest,
                solstone_core_installation_identity::LegacyManifestEvidence::ValidProviderlessSchemaV1
            ),
    );
    if std::env::var_os("HOME").is_some_and(|home| std::path::Path::new(&home) == home_dir)
        && legacy_launcher::validate_effective_path(home_dir, project_root, executable_dir).is_err()
        && !allows_recognized_legacy_path_shadow(
            PlatformTag::current(),
            executable_dir,
            artifacts.artifacts(),
            artifacts.legacy_transition(),
        )
    {
        return Err(IdentityError::AdmissionRefused(
            "PATH resolves outside the V2 installation",
        ));
    }
    if !home_dir.exists()
        && matches!(artifacts.artifacts(), ArtifactBindingEvidence::Fresh)
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
    let request = SetupAdmissionRequest {
        owner,
        root_token,
        journal_token: journal_token_from_path(&resolved.journal_path)?,
        journal_is_explicit: matches!(resolved.journal_source.as_str(), "cli" | "env"),
        legacy_manifest: manifest,
        artifacts: artifacts.artifacts().clone(),
    };
    let admission = if validate_wrapper_journal {
        admit_setup_with_effective_journal_validator(request, validate_effective_wrapper_journal)?
    } else {
        admit_setup(request)?
    };
    let mut repair_steps = artifacts.repair_steps(admission.binding());
    if wrapper_targets_drifted(home_dir, executable_dir) {
        // A version swap since the last setup run: the wrapper's own
        // `SOL_BIN=` still names the old build even though identity admission
        // above found nothing wrong (same namespace, same guard). Force both
        // steps so the owner ends up on the build they just installed instead
        // of a wrapper and service silently left pointed at the old one.
        for step in [StepName::Wrapper, StepName::Service] {
            if !repair_steps.contains(&step) {
                repair_steps.push(step);
            }
        }
    }
    let legacy_replacement = artifacts.legacy_transition();
    Ok(SetupIdentityAdmission {
        admission,
        repair_steps,
        legacy_replacement,
    })
}

fn validate_effective_wrapper_journal(journal: &JournalToken) -> Result<(), IdentityError> {
    wrapper::validate_journal_path_for_wrapper(&journal.to_path_buf()).map_err(|_| {
        IdentityError::AdmissionRefused(
            "effective journal path cannot be represented safely in managed wrappers",
        )
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
    seams: Seams,
    stdout: &mut W,
    stderr: &mut E,
) -> ExitCode {
    run_owner_setup_with_io_with_resolution_env(
        args,
        home_dir,
        executable_dir,
        current_dir,
        env::var("SOLSTONE_JOURNAL").ok(),
        env::var("JOURNAL_VARIANT").ok(),
        stdin_is_tty,
        stdout_is_tty,
        seams,
        stdout,
        stderr,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_owner_setup_with_io_with_resolution_env<W: Write, E: Write>(
    args: SetupArgs,
    home_dir: PathBuf,
    executable_dir: PathBuf,
    current_dir: PathBuf,
    journal_env: Option<String>,
    journal_variant_env: Option<String>,
    stdin_is_tty: bool,
    stdout_is_tty: bool,
    mut seams: Seams,
    stdout: &mut W,
    stderr: &mut E,
) -> ExitCode {
    let receipt_executable_dir = executable_dir.clone();
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
        journal_env,
        journal_variant_env,
        is_source_checkout,
    };
    if args.clean_uninstall {
        if let Some(message) = clean_uninstall_refusal(&args) {
            let _ = writeln!(stderr, "{message}");
            return ExitCode::from(2);
        }
        // Computed before `executable_dir` moves into the clean-uninstall context below.
        let clean_namespace = setup_identity_namespace(&executable_dir, &project_root);
        let clean_session = match admit_clean_identity(&home_dir, &executable_dir, &project_root) {
            Ok(session) => session,
            Err(error) => {
                return report_identity_failure(
                    false,
                    stdout,
                    stderr,
                    &error,
                    &home_dir,
                    setup_identity_namespace(&executable_dir, &project_root).as_deref(),
                );
            }
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
            return report_identity_failure(
                false,
                stdout,
                stderr,
                &error,
                &home_dir,
                clean_namespace.as_deref(),
            );
        }
        return ExitCode::from(outcome.exit_code as u8);
    }
    let resolved = resolve_setup(&args, &resolution);
    let mode = resolve_mode(&args, stdin_is_tty, stdout_is_tty);
    if !resolved.should_short_circuit()
        && !args.skip_wrapper
        && let Err(error) = wrapper::validate_wrapper_pair(&resolved.journal_path, &executable_dir)
    {
        let _ = writeln!(stderr, "journal setup: refused before mutation: {error}");
        return ExitCode::from(1);
    }
    let mut effective_resolved = resolved.clone();
    let (admission, identity_guard_repair_steps, legacy_replacement) = if resolved
        .should_short_circuit()
    {
        (None, Vec::new(), false)
    } else {
        let identity_admission = match admit_setup_identity(
            &home_dir,
            &executable_dir,
            &project_root,
            &resolved,
            !args.skip_wrapper,
        ) {
            Ok(admission) => admission,
            Err(error) => {
                return report_identity_failure(
                    args.jsonl,
                    stdout,
                    stderr,
                    &error,
                    &home_dir,
                    setup_identity_namespace(&executable_dir, &project_root).as_deref(),
                );
            }
        };
        let effective_journal = identity_admission
            .admission
            .effective_journal()
            .to_path_buf();
        if effective_journal != effective_resolved.journal_path {
            effective_resolved.journal_path = effective_journal.clone();
            if let Some(serde_json::Value::Object(journal)) =
                effective_resolved.args_resolved.get_mut("journal")
            {
                journal.insert("value".into(), serde_json::json!(effective_journal));
            }
        }
        if !args.skip_wrapper
            && let Err(error) = wrapper::validate_wrapper_pair(&effective_journal, &executable_dir)
        {
            let _ = writeln!(stderr, "journal setup: refused before mutation: {error}");
            return ExitCode::from(1);
        }
        (
            Some(identity_admission.admission),
            identity_admission.repair_steps,
            identity_admission.legacy_replacement,
        )
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
            identity_guard_repair_steps: Vec::new(),
            legacy_replacement: false,
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
            identity_guard_repair_steps,
            legacy_replacement,
        };
        run_setup(&mut context, &step_specs())
    };
    if outcome.exit_code == 0
        && !args.dry_run
        && !args.explain
        && let Err(error) = write_package_install_receipt(&home_dir, &receipt_executable_dir)
    {
        let _ = writeln!(
            stderr,
            "journal setup: warning: package receipt was not written: {error}"
        );
    }
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
    let exit_code = if args.installer_transaction && outcome.exit_code != 0 {
        // Crossing into `run_setup` means setup may have mutated owner state.
        // The archive installer may roll back only failures returned before
        // this boundary; code 3 is its explicit leave-the-candidate marker.
        3
    } else {
        outcome.exit_code
    };
    ExitCode::from(exit_code as u8)
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
    use crate::identity_evidence::{gather_artifact_evidence, gather_wrapper_artifact_evidence};
    use crate::manifest::{SetupManifest, write_manifest};
    use crate::steps::{CommandOutput, CommandRequest, service_artifact_path};
    use crate::wrapper::{
        WrapperEnvironment, provision_wrappers, wrapper_lock, wrapper_paths,
        write_wrappers_atomically_with,
    };
    use solstone_core_installation_identity::{
        ArtifactBindingEvidence, GuardFields, LegacyManifestEvidence, LifecycleState, OwnerBase,
        PlatformTag, SetupAdmissionRequest, admit_setup, decode_record, encode_record,
        journal_token_from_path, namespace_name, root_token_from_path,
    };
    use std::collections::VecDeque;
    use std::fs;
    use std::io;
    use std::ops::Deref;
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

    struct ServiceArtifactRunner {
        artifact_path: PathBuf,
    }

    impl ServiceArtifactRunner {
        fn new(artifact_path: PathBuf) -> Self {
            Self { artifact_path }
        }
    }

    impl CommandRunner for ServiceArtifactRunner {
        fn run(&mut self, request: &CommandRequest) -> Result<CommandOutput, String> {
            if request
                .args
                .first()
                .is_some_and(|argument| argument == "service")
                && request
                    .args
                    .get(1)
                    .is_some_and(|argument| argument == "install")
            {
                let mut lines = Vec::new();
                for (argument, key) in [
                    (
                        "--installation-namespace",
                        "SOLSTONE_INSTALLATION_NAMESPACE",
                    ),
                    ("--installation-id", "SOLSTONE_INSTALLATION_ID"),
                    (
                        "--installation-generation",
                        "SOLSTONE_INSTALLATION_GENERATION",
                    ),
                    (
                        "--installation-journal-token",
                        "SOLSTONE_INSTALLATION_JOURNAL_TOKEN",
                    ),
                ] {
                    let value = request
                        .args
                        .iter()
                        .position(|value| value == argument)
                        .and_then(|index| request.args.get(index + 1))
                        .ok_or_else(|| format!("missing {argument} from service install"))?;
                    lines.push(format!("Environment=\"{key}={value}\""));
                }
                fs::create_dir_all(
                    self.artifact_path
                        .parent()
                        .expect("service artifact parent"),
                )
                .map_err(|error| error.to_string())?;
                fs::write(&self.artifact_path, lines.join("\n"))
                    .map_err(|error| error.to_string())?;
            }
            Ok(CommandOutput {
                exit_code: 0,
                stdout: "{}".into(),
                stderr: String::new(),
                timed_out: false,
            })
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

    struct HealthyService;
    impl ServiceOps for HealthyService {
        fn is_installed(
            &mut self,
            _runner: &mut dyn CommandRunner,
            _journal: &Path,
        ) -> Result<bool, String> {
            Ok(true)
        }
        fn health_check(
            &mut self,
            _runner: &mut dyn CommandRunner,
            _journal: &Path,
        ) -> Result<bool, String> {
            Ok(true)
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

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new(name: &str) -> Self {
            let root = PathBuf::from("/var/tmp").join(format!(
                "solstone-core-setup-lib-{name}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self(root.canonicalize().unwrap())
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
                "--installer-transaction".into(),
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
        assert_eq!(
            exit,
            ExitCode::from(3),
            "an installer transaction must mark every failure after entering the setup run loop as potentially mutating"
        );
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
            root.to_path_buf(),
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
                .contains("this installation couldn't be verified")
        );
        assert!(
            home.join(".local/bin/solstone").exists(),
            "an owner-authored alias must never be removed"
        );
    }

    /// A refusal that names no remedy is a dead end.
    ///
    /// Observed on the founder's machine 2026-08-31: the previous wording said
    /// only "Repair the managed wrapper/service artifacts or identity storage",
    /// and the sole route back was hand-removing paths nobody would guess.
    /// ⛔ `--clean-uninstall` is not the answer -- it runs the same admission and
    /// refuses in this exact state -- so the refusal has to carry the paths.
    #[test]
    fn a_refused_identity_admission_names_every_path_the_owner_must_clear() {
        let home = std::path::Path::new("/home/tester");
        let namespace = "a".repeat(64);
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit = report_identity_failure(
            false,
            &mut stdout,
            &mut stderr,
            &IdentityError::AdmissionRefused("existing artifacts have no valid bootstrap evidence"),
            home,
            Some(namespace.as_str()),
        );
        let text = String::from_utf8(stderr).expect("stderr");
        for (path, flag) in identity_recovery_paths(home, Some(namespace.as_str())) {
            assert!(
                text.contains(&format!("rm {flag} {}", path.display())),
                "the remedy must name `rm {flag} {}`; got:\n{text}",
                path.display()
            );
        }
        assert!(
            text.contains("run `journal setup` again"),
            "the remedy must say what to run afterwards; got:\n{text}"
        );
        // An owner who is not sure their journal is safe will not run the commands.
        assert!(
            text.contains("your journal itself is untouched"),
            "the remedy must state that the journal is untouched; got:\n{text}"
        );
        // Stopping the service first, so the removed unit is not left running --
        // and with this platform's own command. ⛔ A macOS owner must never be
        // handed `systemctl` next to an `rm` of a LaunchAgent plist.
        let (expected_stop, foreign_stop) = if cfg!(target_os = "macos") {
            ("launchctl bootout", "systemctl")
        } else {
            (
                "systemctl --user disable --now solstone.service",
                "launchctl",
            )
        };
        assert!(
            text.contains(expected_stop),
            "the remedy must stop the service with this platform's command; got:\n{text}"
        );
        assert!(
            !text.contains(foreign_stop),
            "the remedy must not name another platform's service manager; got:\n{text}"
        );
        assert_eq!(format!("{exit:?}"), format!("{:?}", ExitCode::from(2)));
    }

    /// 🔴 The remedy must never tell an owner to remove the owner-wide registry.
    ///
    /// `installation-identity/v1/namespaces/` holds one entry per installation
    /// for this user. Naming its parent would destroy a second, healthy
    /// installation's record along with the broken one.
    #[test]
    fn the_remedy_scopes_identity_removal_to_this_installation_only() {
        let home = std::path::Path::new("/home/tester");
        let namespace = "b".repeat(64);
        let registry = installation_identity_dir(home);

        let scoped = identity_recovery_paths(home, Some(namespace.as_str()));
        let identity: Vec<_> = scoped
            .iter()
            .filter(|(path, _)| path.starts_with(&registry))
            .collect();
        assert_eq!(identity.len(), 1, "exactly one identity path belongs here");
        assert_eq!(
            identity[0].0,
            registry.join("v1").join("namespaces").join(&namespace),
            "the identity path must be this installation's namespace directory"
        );
        for (path, _) in &scoped {
            assert_ne!(
                *path, registry,
                "the owner-wide identity registry must never be a removal target"
            );
        }

        // ...and when the namespace cannot be resolved, omit rather than widen.
        let unscoped = identity_recovery_paths(home, None);
        assert!(
            !unscoped.iter().any(|(path, _)| path.starts_with(&registry)),
            "an unresolvable namespace must drop the identity path, not broaden it"
        );
    }

    #[test]
    fn dry_run_and_explain_do_not_create_identity_storage() {
        for flag in ["--dry-run", "--explain"] {
            let root = root(flag.trim_start_matches("--"));
            let home = root.join("home");
            let executable_dir = if flag == "--explain" {
                PathBuf::from("/usr/bin")
            } else {
                root.join("bin")
            };
            let journal = root.join("journal");
            if executable_dir != Path::new("/usr/bin") {
                fs::create_dir_all(&executable_dir).unwrap();
            }
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
                    root.to_path_buf(),
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
                !OwnerBase::at_home(home.clone(), PlatformTag::current())
                    .expect("identity owner")
                    .path()
                    .exists()
            );
            assert!(
                !package_install_receipt_path(&home).exists(),
                "{flag} must not claim a package install completed"
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
            root.to_path_buf(),
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
                .contains("this installation couldn't be verified")
        );
        assert!(
            !OwnerBase::at_home(home.clone(), PlatformTag::current())
                .expect("identity owner")
                .path()
                .exists()
        );
    }

    #[test]
    fn unsafe_wrapper_input_refuses_before_identity_or_setup_mutation() {
        let root = root("wrapper-preflight");
        let home = root.join("home");
        let executable_dir = root.join("bin");
        fs::create_dir_all(&executable_dir).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let args = parsed(
            &[
                "--yes".into(),
                "--installer-transaction".into(),
                "--journal".into(),
                root.join("bad}journal").display().to_string(),
            ],
            &root,
        );
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = run_owner_setup_with_io(
            args,
            home.clone(),
            executable_dir,
            root.to_path_buf(),
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
        assert_eq!(exit, ExitCode::from(1));
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert!(stdout.is_empty());
        assert!(
            String::from_utf8(stderr)
                .unwrap()
                .contains("refused before mutation")
        );
        assert!(
            !OwnerBase::at_home(home, PlatformTag::current())
                .unwrap()
                .path()
                .exists()
        );
    }

    #[test]
    fn unsafe_prepared_journal_refuses_without_adopting_identity() {
        let root = root("wrapper-preflight-prepared");
        let home = root.join("home");
        let executable_dir = root.join("bin");
        let safe_journal = root.join("safe-journal");
        let unsafe_journal = root.join("old}journal");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&executable_dir).unwrap();

        let root_token = root_token_from_path(&executable_dir).unwrap();
        let namespace = namespace_name(PlatformTag::current(), &root_token);
        let owner = OwnerBase::at_home(home.clone(), PlatformTag::current()).unwrap();
        let initial = admit_setup(SetupAdmissionRequest {
            owner: owner.clone(),
            root_token,
            journal_token: journal_token_from_path(&safe_journal).unwrap(),
            journal_is_explicit: true,
            legacy_manifest: LegacyManifestEvidence::Absent,
            artifacts: ArtifactBindingEvidence::Fresh,
        })
        .unwrap();
        drop(initial);

        let namespace_path = owner.path().join("namespaces").join(namespace.as_hex());
        let record_path = namespace_path.join("record");
        let marker_path = namespace_path.join("adoption.marker");
        let mut record = decode_record(&fs::read(&record_path).unwrap()).unwrap();
        record.state = LifecycleState::Prepared;
        record.journal_token = journal_token_from_path(&unsafe_journal).unwrap();
        fs::write(&record_path, encode_record(&record).unwrap()).unwrap();
        fs::remove_file(&marker_path).unwrap();
        let record_before = fs::read(&record_path).unwrap();

        let calls = Arc::new(AtomicUsize::new(0));
        let args = parsed(
            &[
                "--yes".into(),
                "--journal".into(),
                safe_journal.display().to_string(),
            ],
            &root,
        );
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = run_owner_setup_with_io(
            args,
            home,
            executable_dir,
            root.to_path_buf(),
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
                .contains("cannot be represented safely")
        );
        assert_eq!(fs::read(&record_path).unwrap(), record_before);
        assert!(!marker_path.exists());
    }

    /// Negative twin for the version-swap fix: a wrapper that is
    /// well-formed and guarded, but bound to a genuinely different
    /// installation's namespace, must still refuse rather than be silently
    /// adopted. Recognising "current still points at this exact sibling
    /// version directory" (the fix) must not widen into recognising an
    /// unrelated installation's wrapper as this one's own.
    #[test]
    fn foreign_wrapper_from_a_different_installation_still_refuses() {
        let root = root("identity-foreign-refusal");
        let home = root.join("home");
        let executable_dir = root.join("bin");
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        fs::create_dir_all(&executable_dir).unwrap();

        let foreign_root = root.join("a-completely-different-installation");
        fs::create_dir_all(&foreign_root).unwrap();
        let foreign_root_token = root_token_from_path(&foreign_root).unwrap();
        let foreign_namespace = namespace_name(PlatformTag::current(), &foreign_root_token);
        let foreign_guard = GuardFields {
            namespace: foreign_namespace,
            id: solstone_core_installation_identity::InstallationId::parse(
                "00112233445566778899aabbccddeeff",
            )
            .unwrap(),
            generation: solstone_core_installation_identity::Generation::new(1).unwrap(),
            journal_token: journal_token_from_path(&root.join("journal")).unwrap(),
        };
        let wrapper_path = home.join(".local/bin/journal");
        let wrapper_before = crate::wrapper::render_wrapper(
            crate::wrapper::WrapperCommand::Journal,
            &root.join("journal"),
            &executable_dir.join("journal"),
            &foreign_guard,
        )
        .unwrap();
        fs::write(&wrapper_path, &wrapper_before).unwrap();

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
            root.to_path_buf(),
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
        assert_eq!(
            calls.load(Ordering::Relaxed),
            0,
            "no step may run after a refusal"
        );
        assert!(stdout.is_empty());
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(
            stderr.contains("this installation couldn't be verified"),
            "stderr: {stderr}"
        );
        assert!(
            stderr.contains("bound to a different installation"),
            "must name the foreign-binding reason rather than a generic refusal: {stderr}"
        );
        assert_eq!(
            fs::read_to_string(&wrapper_path).unwrap(),
            wrapper_before,
            "a refused admission must never rewrite the foreign wrapper it could not vouch for"
        );
    }

    #[test]
    fn path_shadow_admission_requires_a_recognized_legacy_transition() {
        assert!(allows_recognized_legacy_path_shadow(
            PlatformTag::Linux,
            std::path::Path::new("/usr/bin"),
            &ArtifactBindingEvidence::LegacyUnguarded,
            true,
        ));
        for artifacts in [
            ArtifactBindingEvidence::Fresh,
            ArtifactBindingEvidence::Malformed,
            ArtifactBindingEvidence::Ambiguous,
        ] {
            assert!(!allows_recognized_legacy_path_shadow(
                PlatformTag::Linux,
                std::path::Path::new("/usr/bin"),
                &artifacts,
                true,
            ));
        }
        assert!(!allows_recognized_legacy_path_shadow(
            PlatformTag::Linux,
            std::path::Path::new("/usr/bin"),
            &ArtifactBindingEvidence::LegacyUnguarded,
            false,
        ));
        assert!(!allows_recognized_legacy_path_shadow(
            PlatformTag::Macos,
            std::path::Path::new("/usr/bin"),
            &ArtifactBindingEvidence::LegacyUnguarded,
            true,
        ));
        assert!(!allows_recognized_legacy_path_shadow(
            PlatformTag::Linux,
            std::path::Path::new("/opt/solstone/bin"),
            &ArtifactBindingEvidence::LegacyUnguarded,
            true,
        ));
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
                root.to_path_buf(),
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
        let record = OwnerBase::at_home(home.clone(), PlatformTag::current())
            .expect("identity owner")
            .path()
            .join("namespaces")
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
                run_owner_setup_with_io_with_resolution_env(
                    parsed(&values, &root),
                    home.clone(),
                    executable_dir.clone(),
                    root.to_path_buf(),
                    None,
                    None,
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
        let record_path = OwnerBase::at_home(home.clone(), PlatformTag::current())
            .expect("identity owner")
            .path()
            .join("namespaces")
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

    #[test]
    fn plain_setup_repairs_the_service_guard_after_a_config_journal_change() {
        let root = root("identity-service-drift");
        let home = root.join("home");
        let executable_dir = root.join("bin");
        let journal_one = root.join("journal-one");
        let journal_two = root.join("journal-two");
        fs::create_dir_all(&executable_dir).expect("create executable directory");
        let service_path = service_artifact_path(&home).expect("linux service artifact");

        let initial_args = parsed(
            &[
                "--yes".into(),
                "--journal".into(),
                journal_one.display().to_string(),
                "--skip-models".into(),
                "--skip-skills".into(),
                "--skip-brain".into(),
            ],
            &root,
        );
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        assert_eq!(
            run_owner_setup_with_io_with_resolution_env(
                initial_args,
                home.clone(),
                executable_dir.clone(),
                root.to_path_buf(),
                None,
                None,
                false,
                false,
                Seams {
                    runner: Box::new(ServiceArtifactRunner::new(service_path.clone())),
                    service_ops: Box::new(HealthyService),
                    check_report_builder: Box::new(Check),
                    already_keeps_journal_probe: no_probe,
                    prompt: Box::new(Prompt),
                    confirm_clean_uninstall: Box::new(|| true),
                },
                &mut stdout,
                &mut stderr,
            ),
            ExitCode::SUCCESS
        );
        assert!(stderr.is_empty());

        let root_token = root_token_from_path(&executable_dir).expect("root token");
        let namespace = namespace_name(PlatformTag::current(), &root_token);
        let owner = OwnerBase::at_home(home.clone(), PlatformTag::current()).expect("owner");
        let updated = admit_setup(SetupAdmissionRequest {
            owner: owner.clone(),
            root_token: root_token.clone(),
            journal_token: journal_token_from_path(&journal_two).expect("second journal token"),
            journal_is_explicit: true,
            legacy_manifest: LegacyManifestEvidence::Absent,
            artifacts: gather_wrapper_artifact_evidence(&home, &namespace),
        })
        .expect("config-equivalent explicit admission");
        let expected = GuardFields::from_binding(updated.binding());
        provision_wrappers(
            &WrapperEnvironment {
                home_dir: home.clone(),
                curdir: executable_dir.clone(),
                executable_dir: executable_dir.clone(),
                backup_dir: Some(root.join("backups")),
                legacy_replacement: false,
            },
            &journal_two,
            updated.binding(),
        )
        .expect("config-equivalent wrapper rewrite");
        drop(updated);

        let paths = wrapper_paths(&home);
        let mut prior = SetupManifest::initial(
            "2026-01-01T00:00:00Z".into(),
            "non_interactive".into(),
            serde_json::Map::new(),
        );
        prior.completed_at = Some("2026-01-01T00:00:01Z".into());
        prior.steps.extend([
            serde_json::json!({
                "name":"wrapper",
                "status":"ok",
                "paths":[paths.solstone.clone(), paths.journal.clone()],
            }),
            serde_json::json!({
                "name":"service",
                "status":"ok",
                "paths":[service_path.clone()],
            }),
        ]);
        write_manifest(&manifest_path(&journal_two), &prior);

        let plain_args = parsed(
            &[
                "--yes".into(),
                "--skip-models".into(),
                "--skip-skills".into(),
                "--skip-brain".into(),
            ],
            &root,
        );
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        assert_eq!(
            run_owner_setup_with_io_with_resolution_env(
                plain_args,
                home.clone(),
                executable_dir,
                root.to_path_buf(),
                None,
                None,
                false,
                false,
                Seams {
                    runner: Box::new(ServiceArtifactRunner::new(service_path)),
                    service_ops: Box::new(HealthyService),
                    check_report_builder: Box::new(Check),
                    already_keeps_journal_probe: no_probe,
                    prompt: Box::new(Prompt),
                    confirm_clean_uninstall: Box::new(|| true),
                },
                &mut stdout,
                &mut stderr,
            ),
            ExitCode::SUCCESS
        );
        assert!(stderr.is_empty());
        assert_eq!(
            gather_artifact_evidence(&home, &namespace),
            ArtifactBindingEvidence::Guarded(expected)
        );
    }

    #[test]
    fn wrapper_write_failure_after_an_explicit_journal_update_is_retryable() {
        let root = root("identity-wrapper-retry");
        let home = root.join("home");
        let executable_dir = root.join("bin");
        let journal_one = root.join("journal-one");
        let journal_two = root.join("journal-two");
        fs::create_dir_all(&home).expect("create home directory");
        fs::create_dir_all(&executable_dir).expect("create executable directory");
        let root_token = root_token_from_path(&executable_dir).expect("root token");
        let namespace = namespace_name(PlatformTag::current(), &root_token);
        let owner = OwnerBase::at_home(home.clone(), PlatformTag::current()).expect("owner");
        let environment = WrapperEnvironment {
            home_dir: home.clone(),
            curdir: executable_dir.clone(),
            executable_dir: executable_dir.clone(),
            backup_dir: Some(root.join("backups")),
            legacy_replacement: false,
        };

        let initial = admit_setup(SetupAdmissionRequest {
            owner: owner.clone(),
            root_token: root_token.clone(),
            journal_token: journal_token_from_path(&journal_one).expect("first journal token"),
            journal_is_explicit: true,
            legacy_manifest: LegacyManifestEvidence::Absent,
            artifacts: ArtifactBindingEvidence::Fresh,
        })
        .expect("initial admission");
        provision_wrappers(&environment, &journal_one, initial.binding())
            .expect("initial wrapper write");
        drop(initial);

        let updated = admit_setup(SetupAdmissionRequest {
            owner: owner.clone(),
            root_token: root_token.clone(),
            journal_token: journal_token_from_path(&journal_two).expect("second journal token"),
            journal_is_explicit: true,
            legacy_manifest: LegacyManifestEvidence::Absent,
            artifacts: gather_wrapper_artifact_evidence(&home, &namespace),
        })
        .expect("explicit admission before wrapper write");
        let updated_guard = GuardFields::from_binding(updated.binding());
        let paths = wrapper_paths(&home);
        let _lock = wrapper_lock(&home).expect("wrapper lock");
        assert!(
            write_wrappers_atomically_with(
                &[
                    (
                        paths.solstone.clone(),
                        crate::wrapper::render_wrapper(
                            crate::wrapper::WrapperCommand::Solstone,
                            &journal_two,
                            &executable_dir.join("solstone"),
                            &updated_guard,
                        )
                        .unwrap(),
                    ),
                    (
                        paths.journal.clone(),
                        crate::wrapper::render_wrapper(
                            crate::wrapper::WrapperCommand::Journal,
                            &journal_two,
                            &executable_dir.join("journal"),
                            &updated_guard,
                        )
                        .unwrap(),
                    ),
                ],
                |_from, _to| Err(io::Error::other("injected wrapper replacement failure")),
            )
            .is_err()
        );
        drop(_lock);
        drop(updated);

        let retry = admit_setup(SetupAdmissionRequest {
            owner,
            root_token,
            journal_token: journal_token_from_path(&journal_two).expect("retry journal token"),
            journal_is_explicit: true,
            legacy_manifest: LegacyManifestEvidence::Absent,
            artifacts: gather_wrapper_artifact_evidence(&home, &namespace),
        })
        .expect("same-identity wrapper drift is retryable");
        assert_eq!(GuardFields::from_binding(retry.binding()), updated_guard);
        provision_wrappers(&environment, &journal_two, retry.binding())
            .expect("retry rewrites wrappers");
        assert_eq!(
            gather_wrapper_artifact_evidence(&home, &namespace),
            ArtifactBindingEvidence::Guarded(updated_guard)
        );
    }

    #[cfg(unix)]
    #[test]
    fn setup_after_current_flip_admits_old_sibling_wrappers_and_repoints_them() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let root = root("identity-version-swap-integration");
        let home = root.join("home");
        let prefix = root.join("prefix");
        let old_bin = prefix.join("versions/2.0.0-aaaaaaaaaaaa/bin");
        let new_bin = prefix.join("versions/2.0.1-bbbbbbbbbbbb/bin");
        let journal = root.join("journal");
        let service_path = service_artifact_path(&home).expect("linux service artifact");
        for bin in [&old_bin, &new_bin] {
            fs::create_dir_all(bin).unwrap();
            for name in ["journal", "solstone"] {
                let path = bin.join(name);
                fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        symlink("versions/2.0.0-aaaaaaaaaaaa", prefix.join("current")).unwrap();
        let setup_args = || {
            parsed(
                &[
                    "--yes".into(),
                    "--journal".into(),
                    journal.display().to_string(),
                    "--skip-models".into(),
                    "--skip-skills".into(),
                    "--skip-brain".into(),
                ],
                &root,
            )
        };
        let run = |args: SetupArgs, executable_dir: PathBuf| {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = run_owner_setup_with_io_with_resolution_env(
                args,
                home.clone(),
                executable_dir,
                root.to_path_buf(),
                None,
                None,
                false,
                false,
                Seams {
                    runner: Box::new(ServiceArtifactRunner::new(service_path.clone())),
                    service_ops: Box::new(HealthyService),
                    check_report_builder: Box::new(Check),
                    already_keeps_journal_probe: no_probe,
                    prompt: Box::new(Prompt),
                    confirm_clean_uninstall: Box::new(|| true),
                },
                &mut stdout,
                &mut stderr,
            );
            (exit, stderr)
        };

        let (initial, initial_stderr) = run(setup_args(), old_bin.clone());
        assert_eq!(initial, ExitCode::SUCCESS, "{initial_stderr:?}");
        fs::remove_file(prefix.join("current")).unwrap();
        symlink("versions/2.0.1-bbbbbbbbbbbb", prefix.join("current")).unwrap();

        let (upgraded, upgraded_stderr) = run(setup_args(), new_bin.clone());
        assert_eq!(
            upgraded,
            ExitCode::SUCCESS,
            "the post-flip setup must not classify its own old sibling wrappers as Foreign: {}",
            String::from_utf8_lossy(&upgraded_stderr)
        );
        for (path, command) in [
            (
                wrapper_paths(&home).journal,
                crate::wrapper::WrapperCommand::Journal,
            ),
            (
                wrapper_paths(&home).solstone,
                crate::wrapper::WrapperCommand::Solstone,
            ),
        ] {
            let content = fs::read_to_string(path).unwrap();
            let parsed = crate::wrapper::parse_wrapper(command, &content).unwrap();
            assert_eq!(parsed.sol_bin.parent(), Some(new_bin.as_path()));
        }
    }

    #[test]
    fn successful_package_setup_receipt_has_the_dispatch_fields() {
        let root = root("package-install-receipt");
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();
        write_owned_package_install_receipt(&home).unwrap();
        let receipt = fs::read_to_string(package_install_receipt_path(&home)).unwrap();
        for field in [
            "schema_version=1",
            concat!("journal_version=", env!("CARGO_PKG_VERSION")),
            "lane=unknown",
            "origin=unknown",
            "route=package",
            "installer_revision=unknown",
            "signature_verification=unverified",
        ] {
            assert!(receipt.lines().any(|line| line == field), "{field}");
        }
        assert!(
            receipt
                .lines()
                .any(|line| line.starts_with("architecture="))
        );

        let other_home = root.join("other-home");
        write_package_install_receipt(&other_home, &root.join("tree/bin")).unwrap();
        assert!(!package_install_receipt_path(&other_home).exists());

        let symlink_home = root.join("symlink-home");
        let outside = root.join("outside");
        fs::create_dir_all(&symlink_home).unwrap();
        fs::create_dir_all(&outside).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, symlink_home.join(".local")).unwrap();
            assert!(write_owned_package_install_receipt(&symlink_home).is_err());
            assert!(
                !outside
                    .join("share/solstone/package-install-receipt")
                    .exists()
            );
        }
    }

    #[test]
    fn package_database_output_requires_the_exact_journal_package_and_path() {
        assert!(dpkg_query_names_journal_package(
            "solstone-journal: /usr/bin/journal\n",
            "/usr/bin/journal"
        ));
        assert!(dpkg_query_names_journal_package(
            "solstone-journal:amd64: /usr/bin/journal\n",
            "/usr/bin/journal"
        ));
        assert!(!dpkg_query_names_journal_package(
            "another-package: /usr/bin/journal\n",
            "/usr/bin/journal"
        ));
        assert!(!dpkg_query_names_journal_package(
            "solstone-journal: /opt/journal\n",
            "/usr/bin/journal"
        ));
    }
}
