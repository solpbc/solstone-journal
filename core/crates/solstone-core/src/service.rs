// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native owner for background-service lifecycle.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use solstone_core::installation_context::installation_recovery_copy;
use solstone_core_cli::{ServiceAction, ServiceInstallationGuardArguments};
use solstone_core_installation_identity::{
    GuardFields, OwnerBase, PlatformTag, load_installation_binding,
    parse_service_guard_environment, root_token_from_path,
};
use solstone_core_journal::resolve_identity_root_from_executable_dir;
use solstone_core_journal_io::{
    DetailedAtomicOutcome, acquire_existing_parent_lock, atomic_replace_detailed,
};
use solstone_core_service_unit::{
    build_service_environment, render_launchd_plist, render_systemd_unit,
};
use solstone_core_system::lifecycle::{clear_readiness, wait_ready};
use solstone_core_system::process::SystemProcessInstanceSource;
use solstone_core_system_health::{SyncRescanDiagnosis, describe_sync_rescan};

use crate::{discover_binary_home, resolve_process_journal_path};

const LABEL: &str = "org.solpbc.solstone";
const UNIT: &str = "solstone.service";
pub(crate) const LOCK_NAME: &str = ".solstone-service.lock";
const UNIT_LIMIT: u64 = 1024 * 1024;
const OUTPUT_LIMIT: u64 = 256 * 1024;
const OBSERVATION_TIMEOUT: Duration = Duration::from_secs(15);
const START_TIMEOUT: Duration = Duration::from_secs(130);
const STOP_TIMEOUT: Duration = Duration::from_secs(40);
const MUTATION_TIMEOUT: Duration = Duration::from_secs(60);
const READY_TIMEOUT: Duration = Duration::from_secs(120);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const SERVICE_SYNC_DIAGNOSTIC_FILENAME: &str = "service-diagnostic.check";
const READY_TIMEOUT_MESSAGE: &str = "service did not become ready within 120 seconds";
const SERVICE_GUARD_ENVIRONMENT_NAMES: [&str; 4] = [
    "SOLSTONE_INSTALLATION_NAMESPACE",
    "SOLSTONE_INSTALLATION_ID",
    "SOLSTONE_INSTALLATION_GENERATION",
    "SOLSTONE_INSTALLATION_JOURNAL_TOKEN",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Platform {
    Linux,
    Darwin,
}

#[derive(Debug)]
pub(crate) enum UnitTruth {
    Absent,
    Managed(UnitSnapshot),
    Foreign,
    Unknown(String),
}

#[derive(Clone, Debug)]
pub(crate) struct UnitSnapshot {
    device: u64,
    inode: u64,
    bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeTruth {
    Absent,
    Managed {
        active: bool,
    },
    /// The registration exists but is not the one journal manages. The payload names
    /// which check rejected it, because the bare refusal is unactionable: an operator
    /// who added a drop-in deliberately cannot tell that from a corrupted unit, and
    /// journal correctly declines to overwrite either.
    Foreign(&'static str),
    Unknown(String),
}

struct CommandResult {
    code: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

pub fn run(action: ServiceAction) -> ExitCode {
    match run_inner(action) {
        Ok(code) => code,
        Err(message) => {
            if is_final_sync_diagnosis(&message) || is_installation_recovery_diagnosis(&message) {
                eprintln!("{message}");
            } else {
                eprintln!("error: {}", safe(&message));
            }
            ExitCode::from(1)
        }
    }
}

fn run_inner(action: ServiceAction) -> Result<ExitCode, String> {
    let platform = platform()?;
    let home = discover_binary_home().map_err(|error| format!("service home: {error:?}"))?;
    match action {
        ServiceAction::Install {
            port,
            installation_guard,
        } => {
            let guard = resolve_installation_guard(installation_guard, || {
                load_existing_installation_guard(&home)
            })?;
            install(platform, &home, port.canonical_decimal(), &guard).map(ExitCode::from)
        }
        ServiceAction::Uninstall => uninstall(platform, &home).map(ExitCode::from),
        ServiceAction::Start => start(platform, &home).map(ExitCode::from),
        ServiceAction::Stop | ServiceAction::Down => stop(platform, &home).map(ExitCode::from),
        ServiceAction::Restart { if_installed } => {
            restart(platform, &home, if_installed).map(ExitCode::from)
        }
        ServiceAction::Status => status(platform, &home),
        ServiceAction::Up => up(platform, &home),
        ServiceAction::Logs { .. } => unreachable!("logs dispatch is handled by main"),
    }
}

fn resolve_installation_guard(
    arguments: Option<ServiceInstallationGuardArguments>,
    load_existing: impl FnOnce() -> Result<GuardFields, String>,
) -> Result<GuardFields, String> {
    match arguments {
        Some(arguments) => parse_installation_guard(arguments),
        None => load_existing().map_err(service_install_recovery),
    }
}

fn load_existing_installation_guard(home: &Path) -> Result<GuardFields, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not resolve the solstone executable: {error}"))?;
    let executable_dir = executable
        .parent()
        .ok_or_else(|| "the solstone executable has no containing directory".to_owned())?;
    let root = resolve_identity_root_from_executable_dir(executable_dir).ok_or_else(|| {
        format!(
            "could not find the solstone installation from {}",
            executable_dir.display()
        )
    })?;
    let root_token = root_token_from_path(&root)
        .map_err(|error| format!("could not verify the solstone installation: {error}"))?;
    let owner = OwnerBase::at_home(home.to_path_buf(), PlatformTag::current())
        .map_err(|error| format!("could not verify the solstone setup storage: {error}"))?;
    let binding = load_installation_binding(&owner, &root_token)
        .map_err(|error| format!("could not verify the saved installation: {error}"))?;
    Ok(GuardFields::from_binding(&binding))
}

/// Whether this process inherited the complete installation guard for the
/// executable and owner that launched it. Invalid, partial, stale, or missing
/// values deliberately remain indistinguishable from an unguarded foreground
/// invocation.
pub(crate) fn current_process_has_matching_installation_guard() -> bool {
    let environment = SERVICE_GUARD_ENVIRONMENT_NAMES
        .into_iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| (name.to_owned(), value))
        })
        .collect();
    environment_matches_installation_guard(
        &environment,
        discover_binary_home,
        load_existing_installation_guard,
    )
}

fn environment_matches_installation_guard<Discover, Load, DiscoverError, LoadError>(
    environment: &BTreeMap<String, String>,
    discover_home: Discover,
    load_guard: Load,
) -> bool
where
    Discover: FnOnce() -> Result<PathBuf, DiscoverError>,
    Load: FnOnce(&Path) -> Result<GuardFields, LoadError>,
{
    let Ok(Some(inherited)) = parse_service_guard_environment(environment) else {
        return false;
    };
    let Ok(home) = discover_home() else {
        return false;
    };
    load_guard(&home).is_ok_and(|expected| inherited == expected)
}

fn service_install_recovery(detail: String) -> String {
    installation_recovery_copy(&detail)
}

pub(crate) fn platform() -> Result<Platform, String> {
    if cfg!(target_os = "linux") {
        Ok(Platform::Linux)
    } else if cfg!(target_os = "macos") {
        Ok(Platform::Darwin)
    } else {
        Err("service management is supported only on linux and mac".to_owned())
    }
}

/// Per-user unit path (`systemd --user` / LaunchAgents). The loopback port the
/// unit starts is machine-wide and shared across logins; do not derive a
/// per-user port from this.
pub(crate) fn unit_path(platform: Platform, home: &Path) -> PathBuf {
    match platform {
        Platform::Linux => home.join(".config/systemd/user").join(UNIT),
        Platform::Darwin => home
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist")),
    }
}

fn parse_installation_guard(
    arguments: ServiceInstallationGuardArguments,
) -> Result<GuardFields, String> {
    let environment = BTreeMap::from([
        (
            "SOLSTONE_INSTALLATION_NAMESPACE".to_owned(),
            arguments.namespace,
        ),
        ("SOLSTONE_INSTALLATION_ID".to_owned(), arguments.id),
        (
            "SOLSTONE_INSTALLATION_GENERATION".to_owned(),
            arguments.generation,
        ),
        (
            "SOLSTONE_INSTALLATION_JOURNAL_TOKEN".to_owned(),
            arguments.journal_token,
        ),
    ]);
    parse_service_guard_environment(&environment)
        .map_err(|error| {
            format!("service install: invalid installation identity arguments: {error}")
        })?
        .ok_or_else(|| "service install requires installation identity arguments".to_owned())
}

fn install(platform: Platform, home: &Path, port: &str, guard: &GuardFields) -> Result<u8, String> {
    let _lock = service_lock(home)?;
    let journal = resolve_process_journal_path()
        .map_err(|_| "service install: could not resolve journal".to_owned())?
        .path;
    if platform == Platform::Darwin {
        cleanup_stale_launchd(home)?;
    }
    let target = unit_path(platform, home);
    let initial = classify_unit(platform, &target)?;
    let upgrade_legacy_lock = platform == Platform::Linux
        && matches!(
            &initial,
            UnitTruth::Managed(snapshot) if !systemd_unit_has_installation_guard(&snapshot.bytes)
        );
    if matches!(initial, UnitTruth::Foreign | UnitTruth::Unknown(_)) {
        return Err(truth_error("install", &target, &initial));
    }
    ensure_health_dir(&journal)?;
    let runtime = observe_runtime(platform, &target)?;
    if matches!(runtime, RuntimeTruth::Foreign(_) | RuntimeTruth::Unknown(_)) {
        return Err(runtime_error("install", runtime));
    }

    let parent = target
        .parent()
        .ok_or_else(|| "service unit has no parent".to_owned())?;
    ensure_real_dir_chain(home, parent)?;
    let launcher = home.join(".local/bin/journal");
    let launcher_metadata = fs::metadata(&launcher)
        .map_err(|error| format!("service launcher is unavailable: {error}"))?;
    if !launcher_metadata.is_file() || launcher_metadata.permissions().mode() & 0o111 == 0 {
        return Err("service launcher is not an executable regular file".to_owned());
    }
    let runtime_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .map(|dir| version_independent_runtime_dir(&dir))
        .ok_or_else(|| "service install: current executable has no directory".to_owned())?;
    let environment = build_service_environment(
        path_text(home)?,
        std::env::var("PATH").ok().as_deref(),
        path_text(&runtime_dir)?,
        guard,
    );
    let bytes = match platform {
        Platform::Linux => {
            render_systemd_unit(&environment, path_text(&launcher)?, port).into_bytes()
        }
        Platform::Darwin => render_launchd_plist(&environment, path_text(&launcher)?, port),
    };

    if matches!(runtime, RuntimeTruth::Managed { .. }) {
        remove_runtime_registration(platform, &target)?;
    }
    revalidate_initial(platform, &target, &initial)?;
    if upgrade_legacy_lock {
        upgrade_legacy_supervisor_lock(&journal)?;
    }
    require_published(atomic_replace_detailed(&target, &bytes, 0o644))?;
    println!("wrote {}", path_display(&target));

    match platform {
        Platform::Linux => {
            require_success(
                run_fixed(systemctl(&["--user", "daemon-reload"]), MUTATION_TIMEOUT)?,
                "reload systemd",
            )?;
            require_success(
                run_fixed(systemctl(&["--user", "enable", UNIT]), MUTATION_TIMEOUT)?,
                "enable service",
            )?;
            println!("service enabled");
        }
        Platform::Darwin => {
            let uid = nix::unistd::Uid::effective().as_raw();
            clear_readiness(&journal)
                .map_err(|error| format!("could not clear the service's ready state: {error}"))?;
            require_success(
                run_fixed(
                    launchctl(&["bootstrap", &format!("gui/{uid}"), path_text(&target)?]),
                    START_TIMEOUT,
                )?,
                "load service",
            )?;
            println!("service loaded into launchd");
        }
    }
    Ok(0)
}

fn uninstall(platform: Platform, home: &Path) -> Result<u8, String> {
    let _lock = service_lock(home)?;
    let target = unit_path(platform, home);
    let initial = classify_unit(platform, &target)?;
    let runtime = observe_runtime(platform, &target)?;
    if matches!(runtime, RuntimeTruth::Foreign(_) | RuntimeTruth::Unknown(_)) {
        return Err(runtime_error("uninstall", runtime));
    }
    if matches!(initial, UnitTruth::Foreign | UnitTruth::Unknown(_)) {
        return Err(truth_error("uninstall", &target, &initial));
    }
    if matches!(initial, UnitTruth::Absent) {
        return match runtime {
            RuntimeTruth::Absent => {
                println!("service was not installed");
                Ok(0)
            }
            RuntimeTruth::Managed { .. } => {
                remove_runtime_registration(platform, &target)?;
                println!("removed stale service registration");
                Ok(0)
            }
            other => Err(runtime_error("uninstall", other)),
        };
    }
    let UnitTruth::Managed(snapshot) = initial else {
        unreachable!("foreign and unknown unit truth returned above")
    };
    verify_snapshot(platform, &target, &snapshot)?;
    match platform {
        Platform::Linux => {
            if matches!(runtime, RuntimeTruth::Managed { .. }) {
                require_success(
                    run_fixed(systemctl(&["--user", "stop", UNIT]), STOP_TIMEOUT)?,
                    "stop service",
                )?;
                require_success(
                    run_fixed(systemctl(&["--user", "disable", UNIT]), MUTATION_TIMEOUT)?,
                    "disable service",
                )?;
            }
        }
        Platform::Darwin if matches!(runtime, RuntimeTruth::Managed { .. }) => {
            let uid = nix::unistd::Uid::effective().as_raw();
            require_success(
                run_fixed(
                    launchctl(&["bootout", &format!("gui/{uid}/{LABEL}")]),
                    STOP_TIMEOUT,
                )?,
                "request launchd unload",
            )?;
            wait_runtime_absent(platform, &target)?;
        }
        Platform::Darwin => {}
    }
    verify_snapshot(platform, &target, &snapshot)?;
    fs::remove_file(&target).map_err(|error| format!("remove service unit: {error}"))?;
    if platform == Platform::Linux {
        require_success(
            run_fixed(systemctl(&["--user", "daemon-reload"]), MUTATION_TIMEOUT)?,
            "reload systemd",
        )?;
    }
    println!("removed {}", path_display(&target));
    Ok(0)
}

fn remove_runtime_registration(platform: Platform, target: &Path) -> Result<(), String> {
    match platform {
        Platform::Linux => {
            require_success(
                run_fixed(systemctl(&["--user", "stop", UNIT]), STOP_TIMEOUT)?,
                "stop stale service",
            )?;
            require_success(
                run_fixed(systemctl(&["--user", "disable", UNIT]), MUTATION_TIMEOUT)?,
                "disable stale service",
            )?;
            wait_runtime_quiescent(platform, target)
        }
        Platform::Darwin => {
            let uid = nix::unistd::Uid::effective().as_raw();
            require_success(
                run_fixed(
                    launchctl(&["bootout", &format!("gui/{uid}/{LABEL}")]),
                    STOP_TIMEOUT,
                )?,
                "unload stale service",
            )?;
            wait_runtime_absent(platform, target)
        }
    }
}

fn start(platform: Platform, home: &Path) -> Result<u8, String> {
    let _lock = service_lock(home)?;
    let target = unit_path(platform, home);
    require_managed(platform, &target)?;
    require_owned_runtime(platform, &target, true)?;
    let journal = resolved_journal()?;
    clear_readiness(&journal)
        .map_err(|error| format!("could not clear the service's ready state: {error}"))?;
    let result = match platform {
        Platform::Linux => run_fixed(systemctl(&["--user", "start", UNIT]), START_TIMEOUT)?,
        Platform::Darwin => {
            let uid = nix::unistd::Uid::effective().as_raw();
            run_fixed(
                launchctl(&["kickstart", &format!("gui/{uid}/{LABEL}")]),
                START_TIMEOUT,
            )?
        }
    };
    require_success(result, "start service")?;
    println!("service started");
    Ok(0)
}

fn stop(platform: Platform, home: &Path) -> Result<u8, String> {
    let _lock = service_lock(home)?;
    let target = unit_path(platform, home);
    let unit = classify_unit(platform, &target)?;
    if matches!(unit, UnitTruth::Foreign | UnitTruth::Unknown(_)) {
        return Err(truth_error("stop", &target, &unit));
    }
    let runtime = observe_runtime(platform, &target)?;
    if stop_requires_manager(&unit, runtime)? {
        let result = match platform {
            Platform::Linux => run_fixed(systemctl(&["--user", "stop", UNIT]), STOP_TIMEOUT)?,
            Platform::Darwin => {
                let uid = nix::unistd::Uid::effective().as_raw();
                run_fixed(
                    launchctl(&["kill", "SIGTERM", &format!("gui/{uid}/{LABEL}")]),
                    STOP_TIMEOUT,
                )?
            }
        };
        require_success(result, "stop service")?;
    }
    if let Ok(journal) = resolved_journal() {
        clear_readiness(&journal).map_err(|error| {
            format!("service stopped, but its ready state could not be cleared: {error}")
        })?;
    }
    println!("service stopped");
    Ok(0)
}

fn stop_requires_manager(unit: &UnitTruth, runtime: RuntimeTruth) -> Result<bool, String> {
    match runtime {
        RuntimeTruth::Foreign(_) | RuntimeTruth::Unknown(_) => Err(runtime_error("stop", runtime)),
        RuntimeTruth::Absent if matches!(unit, UnitTruth::Absent) => Err(not_installed()),
        RuntimeTruth::Absent | RuntimeTruth::Managed { active: false } => Ok(false),
        RuntimeTruth::Managed { active: true } => Ok(true),
    }
}

fn restart(platform: Platform, home: &Path, if_installed: bool) -> Result<u8, String> {
    let _lock = service_lock(home)?;
    let target = unit_path(platform, home);
    let unit = classify_unit(platform, &target)?;
    match &unit {
        UnitTruth::Absent => {}
        UnitTruth::Managed(_) => {}
        other => return Err(truth_error("restart", &target, other)),
    }
    let runtime = require_owned_runtime(platform, &target, true)?;
    if matches!(unit, UnitTruth::Absent) && matches!(runtime, RuntimeTruth::Absent) {
        return if if_installed {
            Ok(0)
        } else {
            Err(not_installed())
        };
    }
    let journal = resolved_journal()?;
    clear_readiness(&journal)
        .map_err(|error| format!("could not clear the service's ready state: {error}"))?;
    let result = match platform {
        Platform::Linux => run_fixed(systemctl(&["--user", "restart", UNIT]), START_TIMEOUT)?,
        Platform::Darwin => {
            let uid = nix::unistd::Uid::effective().as_raw();
            if matches!(runtime, RuntimeTruth::Managed { active: true }) {
                let kill = run_fixed(
                    launchctl(&["kill", "SIGTERM", &format!("gui/{uid}/{LABEL}")]),
                    STOP_TIMEOUT,
                )?;
                require_success(kill, "stop service before restart")?;
            }
            run_fixed(
                launchctl(&["kickstart", &format!("gui/{uid}/{LABEL}")]),
                START_TIMEOUT,
            )?
        }
    };
    require_success(result, "restart service")?;
    if wait_ready(&journal, READY_TIMEOUT, POLL_INTERVAL).is_none() {
        return Err(ready_timeout_message(&journal));
    }
    println!("service restarted");
    Ok(0)
}

fn status(platform: Platform, home: &Path) -> Result<ExitCode, String> {
    let target = unit_path(platform, home);
    match classify_unit(platform, &target)? {
        UnitTruth::Absent => {
            println!("service: not installed");
            println!("run 'journal setup' or 'journal service install' to install it.");
            print_no_supervisor_sync_diagnosis();
            Ok(ExitCode::from(1))
        }
        UnitTruth::Managed(_) => {
            println!("service: installed");
            match observe_runtime(platform, &target)? {
                RuntimeTruth::Managed { active: true } => {
                    println!(
                        "state: running ({})",
                        if platform == Platform::Linux {
                            "systemd"
                        } else {
                            "launchd"
                        }
                    );
                    println!();
                    return Ok(crate::health::run(false, false));
                }
                RuntimeTruth::Managed { active: false } | RuntimeTruth::Absent => {
                    println!("state: stopped");
                    print_no_supervisor_sync_diagnosis();
                }
                other => return Err(runtime_error("status", other)),
            }
            Ok(ExitCode::SUCCESS)
        }
        other => Err(truth_error("status", &target, &other)),
    }
}

fn up(platform: Platform, home: &Path) -> Result<ExitCode, String> {
    let target = unit_path(platform, home);
    require_managed(platform, &target)?;
    match observe_runtime(platform, &target)? {
        RuntimeTruth::Absent | RuntimeTruth::Managed { active: false } => {
            start(platform, home)?;
        }
        RuntimeTruth::Managed { active: true } => {}
        other => return Err(runtime_error("up", other)),
    }
    let journal = resolved_journal()?;
    if wait_ready(&journal, READY_TIMEOUT, POLL_INTERVAL).is_none() {
        return Err(ready_timeout_message(&journal));
    }
    let _ = status(platform, home)?;
    Ok(ExitCode::SUCCESS)
}

fn resolved_journal() -> Result<PathBuf, String> {
    resolve_process_journal_path()
        .map(|line| line.path)
        .map_err(|_| "could not resolve journal".to_owned())
}

fn ready_timeout_message(journal: &Path) -> String {
    match sync_rescan_diagnosis(journal) {
        Some(message) => message,
        None => READY_TIMEOUT_MESSAGE.to_owned(),
    }
}

fn is_final_sync_diagnosis(message: &str) -> bool {
    message.starts_with("Installation: needs attention\n")
}

fn is_installation_recovery_diagnosis(message: &str) -> bool {
    message.starts_with("this installation couldn't be verified.\n")
}

fn sync_rescan_diagnosis(journal: &Path) -> Option<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |duration| duration.as_secs_f64());
    let process_source = SystemProcessInstanceSource;
    match describe_sync_rescan(
        journal,
        SERVICE_SYNC_DIAGNOSTIC_FILENAME,
        now,
        &process_source,
    ) {
        SyncRescanDiagnosis::Clean(_) => None,
        SyncRescanDiagnosis::Waiting(message)
        | SyncRescanDiagnosis::HeartbeatNeedsAttention(message)
        | SyncRescanDiagnosis::AdmissionWaitNeedsAttention(message)
        | SyncRescanDiagnosis::Unsafe(message) => Some(message),
    }
}

fn print_no_supervisor_sync_diagnosis() {
    if let Ok(journal) = resolved_journal()
        && let Some(message) = sync_rescan_diagnosis(&journal)
    {
        println!("{message}");
    }
}

pub(crate) fn service_lock(
    home: &Path,
) -> Result<solstone_core_journal_io::ExistingParentLock, String> {
    acquire_existing_parent_lock(
        home,
        OsStr::new(LOCK_NAME),
        Duration::from_secs(10),
        Duration::from_millis(50),
    )
    .map_err(|error| format!("service lock: {error}"))
}

pub(crate) fn classify_unit(platform: Platform, path: &Path) -> Result<UnitTruth, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(UnitTruth::Absent);
        }
        Err(error) => return Ok(UnitTruth::Unknown(format!("metadata: {error}"))),
    };
    if !metadata.file_type().is_file() {
        return Ok(UnitTruth::Foreign);
    }
    if metadata.len() > UNIT_LIMIT {
        return Ok(UnitTruth::Unknown("unit exceeds 1 MiB".to_owned()));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    match File::open(path).and_then(|file| file.take(UNIT_LIMIT + 1).read_to_end(&mut bytes)) {
        Ok(_) if bytes.len() as u64 <= UNIT_LIMIT => {}
        Ok(_) => return Ok(UnitTruth::Unknown("unit exceeds 1 MiB".to_owned())),
        Err(error) => return Ok(UnitTruth::Unknown(format!("read: {error}"))),
    }
    let Some(launchers) = expected_launchers(platform, path) else {
        return Ok(UnitTruth::Unknown(
            "unit path has no trusted home ancestor".to_owned(),
        ));
    };
    let managed = match platform {
        Platform::Linux => launchers
            .iter()
            .any(|launcher| systemd_managed(&bytes, launcher)),
        Platform::Darwin => launchers
            .iter()
            .any(|launcher| launchd_managed(&bytes, launcher, LABEL)),
    };
    Ok(if managed {
        UnitTruth::Managed(UnitSnapshot {
            device: metadata.dev(),
            inode: metadata.ino(),
            bytes,
        })
    } else {
        UnitTruth::Foreign
    })
}

fn systemd_managed(bytes: &[u8], launcher: &Path) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let sections = text
        .lines()
        .filter(|line| line.starts_with('['))
        .collect::<Vec<_>>();
    if sections != ["[Unit]", "[Service]", "[Install]"] {
        return false;
    }
    let mut singleton = BTreeMap::new();
    let mut environment = BTreeMap::new();
    let allowed = BTreeSet::from([
        "Description",
        "After",
        "StartLimitIntervalSec",
        "StartLimitBurst",
        "Type",
        "TimeoutStartSec",
        "ExecStart",
        "Restart",
        "RestartSec",
        "KillMode",
        "TimeoutStopSec",
        "LimitNOFILE",
        "StandardOutput",
        "StandardError",
        "WantedBy",
    ]);
    for line in text
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('['))
    {
        let Some((key, value)) = line.split_once('=') else {
            return false;
        };
        if key == "Environment" {
            let value = value.trim_matches('"');
            let Some((name, value)) = value.split_once('=') else {
                return false;
            };
            if !matches!(
                name,
                "HOME"
                    | "PATH"
                    | "PYTHONUNBUFFERED"
                    | "_SOLSTONE_JOURNAL_OVERRIDE"
                    | "ANTHROPIC_API_KEY"
                    | "GOOGLE_API_KEY"
                    | "OPENAI_API_KEY"
                    | "PLAUD_ACCESS_TOKEN"
                    | "REVAI_ACCESS_TOKEN"
                    | "SOLSTONE_INSTALLATION_NAMESPACE"
                    | "SOLSTONE_INSTALLATION_ID"
                    | "SOLSTONE_INSTALLATION_GENERATION"
                    | "SOLSTONE_INSTALLATION_JOURNAL_TOKEN"
            ) || environment.insert(name, value).is_some()
            {
                return false;
            }
        } else if !allowed.contains(key) || singleton.insert(key, value).is_some() {
            return false;
        }
    }
    let Some(exec) = singleton.get("ExecStart") else {
        return false;
    };
    let service_type = singleton.get("Type").copied();
    let safety_fields = match (
        singleton.get("KillMode").copied(),
        singleton.get("TimeoutStopSec").copied(),
    ) {
        (Some("control-group"), Some("30")) => true,
        (None, None) => {
            service_type == Some("simple") && systemd_unit_is_legacy_supervisor(exec, launcher)
        }
        _ => false,
    };
    singleton.get("Description") == Some(&"Solstone Supervisor")
        && singleton.get("After") == Some(&"default.target")
        && systemd_unit_exec_matches(exec, launcher)
        && environment.contains_key("HOME")
        && environment.contains_key("PATH")
        && service_guard_is_valid(&environment)
        && environment
            .get("PYTHONUNBUFFERED")
            .is_none_or(|value| *value == "1")
        && matches!(service_type, Some("notify" | "simple"))
        && singleton
            .get("StartLimitIntervalSec")
            .is_none_or(|value| *value == "120")
        && singleton
            .get("StartLimitBurst")
            .is_none_or(|value| *value == "10")
        && singleton.contains_key("StartLimitIntervalSec")
            == singleton.contains_key("StartLimitBurst")
        && singleton
            .get("TimeoutStartSec")
            .is_none_or(|value| *value == "120")
        && singleton.get("Restart") == Some(&"on-failure")
        && singleton.get("RestartSec") == Some(&"5")
        && safety_fields
        && singleton
            .get("LimitNOFILE")
            .is_none_or(|value| *value == "4096")
        && singleton.get("WantedBy") == Some(&"default.target")
        && systemd_logs_are_managed(&singleton)
}

fn service_guard_is_valid(environment: &BTreeMap<&str, &str>) -> bool {
    let mut guard = BTreeMap::new();
    for name in [
        "SOLSTONE_INSTALLATION_NAMESPACE",
        "SOLSTONE_INSTALLATION_ID",
        "SOLSTONE_INSTALLATION_GENERATION",
        "SOLSTONE_INSTALLATION_JOURNAL_TOKEN",
    ] {
        if let Some(value) = environment.get(name) {
            guard.insert(name.to_owned(), (*value).to_owned());
        }
    }
    parse_service_guard_environment(&guard).is_ok()
}

fn systemd_unit_has_installation_guard(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    text.lines()
        .filter_map(|line| line.strip_prefix("Environment="))
        .map(|value| value.trim_matches('"'))
        .any(|value| value.starts_with("SOLSTONE_INSTALLATION_NAMESPACE="))
}

/// V1 created `health/supervisor.lock` as an ordinary 0644 advisory-lock
/// file. Once its exact managed unit is quiescent, narrow that known legacy
/// entry to the native runtime's 0600 contract without following or replacing
/// it. Other modes and namespace shapes remain fail-closed.
fn upgrade_legacy_supervisor_lock(journal: &Path) -> Result<(), String> {
    let path = journal.join("health/supervisor.lock");
    let observed = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("inspect legacy supervisor lock: {error}")),
    };
    if !observed.file_type().is_file() {
        return Err("legacy supervisor lock is not a regular file".to_owned());
    }
    let observed_mode = observed.permissions().mode() & 0o7777;
    if observed_mode == 0o600 {
        return Ok(());
    }
    if observed_mode != 0o644
        || observed.uid() != nix::unistd::Uid::effective().as_raw()
        || observed.nlink() != 1
    {
        return Err(format!(
            "legacy supervisor lock cannot be upgraded safely (mode {observed_mode:o})"
        ));
    }

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|error| format!("open legacy supervisor lock: {error}"))?;
    let opened = file
        .metadata()
        .map_err(|error| format!("stat legacy supervisor lock: {error}"))?;
    if !opened.file_type().is_file()
        || opened.dev() != observed.dev()
        || opened.ino() != observed.ino()
        || opened.uid() != observed.uid()
        || opened.nlink() != 1
        || opened.permissions().mode() & 0o7777 != 0o644
    {
        return Err("legacy supervisor lock changed during upgrade".to_owned());
    }
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("restrict legacy supervisor lock: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("sync legacy supervisor lock: {error}"))?;
    let upgraded = file
        .metadata()
        .map_err(|error| format!("verify legacy supervisor lock: {error}"))?;
    if upgraded.permissions().mode() & 0o7777 != 0o600 {
        return Err("legacy supervisor lock mode did not converge to 600".to_owned());
    }
    let named = fs::symlink_metadata(&path)
        .map_err(|error| format!("revalidate legacy supervisor lock: {error}"))?;
    if !named.file_type().is_file()
        || named.dev() != upgraded.dev()
        || named.ino() != upgraded.ino()
        || named.uid() != upgraded.uid()
        || named.nlink() != 1
        || named.permissions().mode() & 0o7777 != 0o600
    {
        return Err("legacy supervisor lock changed after upgrade".to_owned());
    }
    Ok(())
}

fn systemd_unit_exec_matches(value: &str, launcher: &Path) -> bool {
    systemd_unit_arguments(value, launcher)
        .is_some_and(|arguments| managed_command_tail(&arguments[1..]))
}

fn systemd_unit_is_legacy_supervisor(value: &str, launcher: &Path) -> bool {
    systemd_unit_arguments(value, launcher)
        .is_some_and(|arguments| arguments.get(1).is_some_and(|verb| verb == "supervisor"))
}

fn systemd_unit_arguments(value: &str, launcher: &Path) -> Option<Vec<String>> {
    let expected = launcher.to_str()?;
    if let Some(tail) = value
        .strip_prefix(expected)
        .and_then(|tail| tail.strip_prefix(' '))
    {
        let mut arguments = vec![expected.to_owned()];
        arguments.extend(tail.split_whitespace().map(str::to_owned));
        return Some(arguments);
    }
    parse_systemd_words(value)
        .filter(|arguments| arguments.first().is_some_and(|value| value == expected))
}

fn systemd_logs_are_managed(singleton: &BTreeMap<&str, &str>) -> bool {
    match (
        singleton.get("StandardOutput"),
        singleton.get("StandardError"),
    ) {
        (None, None) => true,
        (Some(output), Some(error)) => {
            output.starts_with("append:")
                && output.ends_with("/health/service.log")
                && (*error == *output || *error == "inherit")
        }
        _ => false,
    }
}

fn launchd_managed(bytes: &[u8], launcher: &Path, expected_label: &str) -> bool {
    let Ok(value) = plist::Value::from_reader_xml(bytes) else {
        return false;
    };
    let Some(dict) = value.as_dictionary() else {
        return false;
    };
    let allowed = BTreeSet::from([
        "EnvironmentVariables",
        "KeepAlive",
        "Label",
        "ProgramArguments",
        "RunAtLoad",
        "SoftResourceLimits",
        "StandardErrorPath",
        "StandardOutPath",
    ]);
    let keys = dict.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if !keys.is_subset(&allowed)
        || !BTreeSet::from([
            "EnvironmentVariables",
            "KeepAlive",
            "Label",
            "ProgramArguments",
            "RunAtLoad",
        ])
        .is_subset(&keys)
    {
        return false;
    }
    let Some(arguments) = dict
        .get("ProgramArguments")
        .and_then(plist::Value::as_array)
    else {
        return false;
    };
    let args = arguments
        .iter()
        .map(plist::Value::as_string)
        .collect::<Option<Vec<_>>>();
    let Some(args) = args else {
        return false;
    };
    let environment = dict
        .get("EnvironmentVariables")
        .and_then(plist::Value::as_dictionary);
    let env_keys = environment.map(|env| env.keys().map(String::as_str).collect::<BTreeSet<_>>());
    let allowed_env = BTreeSet::from([
        "HOME",
        "PATH",
        "PYTHONUNBUFFERED",
        "_SOLSTONE_JOURNAL_OVERRIDE",
        "ANTHROPIC_API_KEY",
        "GOOGLE_API_KEY",
        "OPENAI_API_KEY",
        "PLAUD_ACCESS_TOKEN",
        "REVAI_ACCESS_TOKEN",
        "SOLSTONE_INSTALLATION_NAMESPACE",
        "SOLSTONE_INSTALLATION_ID",
        "SOLSTONE_INSTALLATION_GENERATION",
        "SOLSTONE_INSTALLATION_JOURNAL_TOKEN",
    ]);
    let keep_alive = dict.get("KeepAlive").and_then(plist::Value::as_dictionary);
    let limits = dict
        .get("SoftResourceLimits")
        .and_then(plist::Value::as_dictionary);
    dict.get("Label").and_then(plist::Value::as_string) == Some(expected_label)
        && args.len() >= 2
        && path_text(launcher).is_ok_and(|expected| args[0] == expected)
        && managed_command_tail(&args[1..])
        && env_keys.is_some_and(|keys| {
            keys.is_subset(&allowed_env) && keys.contains("HOME") && keys.contains("PATH")
        })
        && environment.is_some_and(launchd_service_guard_is_valid)
        && environment
            .and_then(|env| env.get("PYTHONUNBUFFERED"))
            .is_none_or(|value| value == &plist::Value::String("1".to_owned()))
        && environment.is_some_and(|env| env.values().all(|value| value.as_string().is_some()))
        && dict.get("RunAtLoad").and_then(plist::Value::as_boolean) == Some(true)
        && launchd_keep_alive_is_managed(dict.get("KeepAlive"), keep_alive)
        && limits.is_none_or(|value| {
            value.len() == 1
                && value.get("NumberOfFiles") == Some(&plist::Value::Integer(4096_i64.into()))
        })
        && launchd_logs_are_managed(dict)
}

fn launchd_service_guard_is_valid(environment: &plist::Dictionary) -> bool {
    let mut guard = BTreeMap::new();
    for name in [
        "SOLSTONE_INSTALLATION_NAMESPACE",
        "SOLSTONE_INSTALLATION_ID",
        "SOLSTONE_INSTALLATION_GENERATION",
        "SOLSTONE_INSTALLATION_JOURNAL_TOKEN",
    ] {
        if let Some(value) = environment.get(name) {
            let Some(value) = value.as_string() else {
                return false;
            };
            guard.insert(name.to_owned(), value.to_owned());
        }
    }
    parse_service_guard_environment(&guard).is_ok()
}

fn launchd_keep_alive_is_managed(
    value: Option<&plist::Value>,
    dictionary: Option<&plist::Dictionary>,
) -> bool {
    value.and_then(plist::Value::as_boolean) == Some(true)
        || dictionary.is_some_and(|value| {
            value.len() == 1 && value.get("SuccessfulExit") == Some(&plist::Value::Boolean(false))
        })
}

fn launchd_logs_are_managed(dict: &plist::Dictionary) -> bool {
    let stdout = dict
        .get("StandardOutPath")
        .and_then(plist::Value::as_string);
    let stderr = dict
        .get("StandardErrorPath")
        .and_then(plist::Value::as_string);
    match (stdout, stderr) {
        (None, None) => true,
        (Some(stdout), Some(stderr)) => {
            (stdout.ends_with("/health/service.log") && stdout == stderr)
                || (stdout.ends_with("/health/launchd-stdout.log")
                    && stderr.ends_with("/health/launchd-stderr.log"))
        }
        _ => false,
    }
}

fn cleanup_stale_launchd(home: &Path) -> Result<(), String> {
    let directory = home.join("Library/LaunchAgents");
    let metadata = match fs::symlink_metadata(&directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("inspect launchd directory: {error}")),
    };
    if !metadata.file_type().is_dir() {
        return Err("launchd directory is not a real directory".to_owned());
    }
    let canonical = directory.join(format!("{LABEL}.plist"));
    let mut candidates = fs::read_dir(&directory)
        .map_err(|error| format!("scan launchd directory: {error}"))?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| format!("scan launchd entry: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    candidates.sort();
    for candidate in candidates {
        if candidate != canonical && !stale_launchd_name(&candidate) {
            continue;
        }
        let snapshot = read_unit_snapshot(&candidate)
            .map_err(|error| format!("inspect stale launchd unit: {error}"))?;
        let value = plist::Value::from_reader_xml(snapshot.bytes.as_slice())
            .map_err(|error| format!("parse stale launchd unit: {error}"))?;
        let dictionary = value
            .as_dictionary()
            .ok_or_else(|| "stale launchd unit is not a dictionary".to_owned())?;
        let label = dictionary
            .get("Label")
            .and_then(plist::Value::as_string)
            .ok_or_else(|| "stale launchd unit has no label".to_owned())?;
        if label != LABEL && !label.starts_with(&format!("{LABEL}.")) {
            return Err("stale launchd unit has a foreign label".to_owned());
        }
        let arguments = dictionary
            .get("ProgramArguments")
            .and_then(plist::Value::as_array);
        let program = dictionary.get("Program").and_then(plist::Value::as_string);
        if arguments.is_some() && program.is_some() {
            return Err("stale launchd unit has conflicting program fields".to_owned());
        }
        let launcher = arguments
            .and_then(|arguments| arguments.first())
            .and_then(plist::Value::as_string)
            .or(program)
            .map(PathBuf::from)
            .ok_or_else(|| "stale launchd unit has no launcher".to_owned())?;
        let current_launchers = [
            home.join(".local/bin/journal"),
            home.join(".local/bin/solstone"),
            home.join(".local/bin/sol"),
        ];
        if candidate == canonical && current_launchers.contains(&launcher) {
            continue;
        }
        let program_only = arguments.is_none();
        if !matches!(
            launcher.file_name().and_then(OsStr::to_str),
            Some("journal" | "solstone" | "sol")
        ) || !stale_launchd_managed(dictionary, label, &launcher)
        {
            return Err("stale launchd unit is foreign or unrecognized".to_owned());
        }
        let launchers = [launcher.clone(), launcher.clone()];
        match observe_launchd_registration(label, &candidate, &launchers, program_only)? {
            RuntimeTruth::Absent => {}
            RuntimeTruth::Managed { .. } => {
                let uid = nix::unistd::Uid::effective().as_raw();
                require_success(
                    run_fixed(
                        launchctl(&["bootout", &format!("gui/{uid}"), path_text(&candidate)?]),
                        STOP_TIMEOUT,
                    )?,
                    "unload stale launchd service",
                )?;
                wait_launchd_absent(label, &candidate, &launchers, program_only)?;
            }
            other => return Err(runtime_error("clean stale launchd service", other)),
        }
        let actual = read_unit_snapshot(&candidate)
            .map_err(|error| format!("recheck stale launchd unit: {error}"))?;
        if !same_snapshot(&snapshot, &actual) {
            return Err(format!(
                "stale launchd plist changed during cleanup; preserved {}",
                path_display(&candidate)
            ));
        }
        fs::remove_file(&candidate)
            .map_err(|error| format!("remove stale launchd unit: {error}"))?;
        println!("removed stale launchd plist {}", path_display(&candidate));
    }
    Ok(())
}

fn stale_launchd_managed(dictionary: &plist::Dictionary, label: &str, launcher: &Path) -> bool {
    let allowed = BTreeSet::from([
        "EnvironmentVariables",
        "KeepAlive",
        "Label",
        "Program",
        "ProgramArguments",
        "RunAtLoad",
        "SoftResourceLimits",
        "StandardErrorPath",
        "StandardOutPath",
    ]);
    if !dictionary.keys().all(|key| allowed.contains(key.as_str()))
        || dictionary.get("Label").and_then(plist::Value::as_string) != Some(label)
    {
        return false;
    }
    match (
        dictionary
            .get("ProgramArguments")
            .and_then(plist::Value::as_array),
        dictionary.get("Program").and_then(plist::Value::as_string),
    ) {
        (Some(arguments), None) => {
            let arguments = arguments
                .iter()
                .map(plist::Value::as_string)
                .collect::<Option<Vec<_>>>();
            arguments.is_some_and(|arguments| {
                arguments.first().copied() == launcher.to_str()
                    && managed_command_tail(&arguments[1..])
            })
        }
        (None, Some(program)) => Some(program) == launcher.to_str(),
        _ => false,
    }
}

fn stale_launchd_name(path: &Path) -> bool {
    let Some(name) = path.file_name() else {
        return false;
    };
    let bytes = name.as_encoded_bytes();
    bytes.starts_with(b"org.solpbc.solstone.") && bytes.ends_with(b".plist")
}

fn read_unit_snapshot(path: &Path) -> Result<UnitSnapshot, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| format!("metadata: {error}"))?;
    if !metadata.file_type().is_file() {
        return Err("entry is not a regular file".to_owned());
    }
    if metadata.len() > UNIT_LIMIT {
        return Err("unit exceeds 1 MiB".to_owned());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .and_then(|file| file.take(UNIT_LIMIT + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("read: {error}"))?;
    if bytes.len() as u64 > UNIT_LIMIT {
        return Err("unit exceeds 1 MiB".to_owned());
    }
    Ok(UnitSnapshot {
        device: metadata.dev(),
        inode: metadata.ino(),
        bytes,
    })
}

fn parse_systemd_words(value: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut characters = value.chars().peekable();
    let mut quoted = false;
    while let Some(character) = characters.next() {
        match character {
            '"' => quoted = !quoted,
            '\\' => {
                let escaped = characters.next()?;
                match escaped {
                    'n' => word.push('\n'),
                    'r' => word.push('\r'),
                    't' => word.push('\t'),
                    'x' => word.push(char::from_u32(parse_hex(&mut characters, 2)?)?),
                    'u' => word.push(char::from_u32(parse_hex(&mut characters, 4)?)?),
                    other => word.push(other),
                }
            }
            '$' if characters.peek() == Some(&'$') => {
                characters.next();
                word.push('$');
            }
            '%' if characters.peek() == Some(&'%') => {
                characters.next();
                word.push('%');
            }
            character if character.is_whitespace() && !quoted => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            character => word.push(character),
        }
    }
    if quoted {
        return None;
    }
    if !word.is_empty() {
        words.push(word);
    }
    Some(words)
}

fn systemd_drop_ins_are_trusted(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    let Some(paths) = parse_systemd_words(value) else {
        return false;
    };
    !paths.is_empty()
        && paths
            .iter()
            .all(|path| trusted_systemd_vendor_drop_in(Path::new(path)))
}

fn trusted_systemd_vendor_drop_in(path: &Path) -> bool {
    let vendor_directory = Path::new("/usr/lib/systemd/user/service.d");
    if path.parent() != Some(vendor_directory)
        || path.extension() != Some(OsStr::new("conf"))
        || path.file_stem().is_none()
    {
        return false;
    }
    let trusted_directories = [
        Path::new("/"),
        Path::new("/usr"),
        Path::new("/usr/lib"),
        Path::new("/usr/lib/systemd"),
        Path::new("/usr/lib/systemd/user"),
        vendor_directory,
    ];
    if !trusted_directories
        .iter()
        .all(|directory| trusted_root_owned_directory(directory))
    {
        return false;
    }
    let Ok(file) = fs::symlink_metadata(path) else {
        return false;
    };
    file.file_type().is_file() && file.uid() == 0 && file.permissions().mode() & 0o022 == 0
}

fn trusted_root_owned_directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.file_type().is_dir()
            && metadata.uid() == 0
            && metadata.permissions().mode() & 0o022 == 0
    })
}

fn parse_hex(
    characters: &mut std::iter::Peekable<impl Iterator<Item = char>>,
    digits: usize,
) -> Option<u32> {
    let mut value = 0_u32;
    for _ in 0..digits {
        value = value.checked_mul(16)?;
        value = value.checked_add(characters.next()?.to_digit(16)?)?;
    }
    Some(value)
}

pub(crate) fn observe_runtime(
    platform: Platform,
    canonical: &Path,
) -> Result<RuntimeTruth, String> {
    let launchers = expected_launchers(platform, canonical)
        .ok_or_else(|| "service unit has no trusted home ancestor".to_owned())?;
    match platform {
        Platform::Linux => {
            let result = run_fixed(
                systemctl(&[
                    "--user",
                    "show",
                    UNIT,
                    "--property=Id,LoadState,ActiveState,SubState,FragmentPath,SourcePath,DropInPaths,ExecStart,UnitFileState",
                    "--no-pager",
                ]),
                OBSERVATION_TIMEOUT,
            )?;
            classify_systemd_runtime(&result, canonical, &launchers)
        }
        Platform::Darwin => observe_launchd_registration(LABEL, canonical, &launchers, false),
    }
}

fn classify_systemd_runtime(
    result: &CommandResult,
    canonical: &Path,
    launchers: &[PathBuf],
) -> Result<RuntimeTruth, String> {
    if result.code != 0 || !result.stderr.is_empty() {
        return Ok(RuntimeTruth::Unknown(result_message(result)));
    }
    let text = std::str::from_utf8(&result.stdout)
        .map_err(|_| "systemd observation was not UTF-8".to_owned())?;
    let mut values = BTreeMap::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            return Ok(RuntimeTruth::Unknown(
                "malformed systemd property".to_owned(),
            ));
        };
        if values.insert(key, value).is_some() {
            return Ok(RuntimeTruth::Unknown(format!(
                "duplicate systemd property: {key}"
            )));
        }
    }
    let absent_keys = BTreeSet::from([
        "Id",
        "LoadState",
        "ActiveState",
        "SubState",
        "FragmentPath",
        "SourcePath",
        "DropInPaths",
        "UnitFileState",
    ]);
    let Some(id) = values.get("Id") else {
        return Ok(RuntimeTruth::Unknown(
            "incomplete systemd property set".to_owned(),
        ));
    };
    let Some(load_state) = values.get("LoadState") else {
        return Ok(RuntimeTruth::Unknown(
            "incomplete systemd property set".to_owned(),
        ));
    };
    if *id != UNIT {
        return Ok(RuntimeTruth::Foreign(
            "a different unit answers to this name",
        ));
    }
    if *load_state == "not-found" {
        return if values.keys().copied().collect::<BTreeSet<_>>() == absent_keys
            && values["ActiveState"] == "inactive"
            && values["SubState"] == "dead"
            && values["FragmentPath"].is_empty()
            && values["SourcePath"].is_empty()
            && values["DropInPaths"].is_empty()
            && values["UnitFileState"].is_empty()
        {
            Ok(RuntimeTruth::Absent)
        } else {
            Ok(RuntimeTruth::Unknown(
                "conflicting absent systemd state".to_owned(),
            ))
        };
    }
    let loaded_keys = BTreeSet::from([
        "Id",
        "LoadState",
        "ActiveState",
        "SubState",
        "FragmentPath",
        "SourcePath",
        "DropInPaths",
        "ExecStart",
        "UnitFileState",
    ]);
    if values.keys().copied().collect::<BTreeSet<_>>() != loaded_keys {
        return Ok(RuntimeTruth::Unknown(
            "incomplete systemd property set".to_owned(),
        ));
    }
    // Report which check rejected the unit, in the order an operator would fix them.
    if !systemd_drop_ins_are_trusted(values["DropInPaths"]) {
        return Ok(RuntimeTruth::Foreign(
            "the unit carries a drop-in journal did not write",
        ));
    }
    if values["FragmentPath"] != path_text(canonical)? {
        return Ok(RuntimeTruth::Foreign(
            "the unit file is not the one journal manages",
        ));
    }
    if !launchers
        .iter()
        .any(|launcher| systemd_runtime_exec_matches(values["ExecStart"], launcher))
    {
        return Ok(RuntimeTruth::Foreign(
            "the unit's ExecStart is not a journal launcher",
        ));
    }
    if values["LoadState"] != "loaded"
        || !values["SourcePath"].is_empty()
        || !matches!(values["UnitFileState"], "enabled" | "disabled" | "static")
    {
        return Ok(RuntimeTruth::Foreign("the unit is in an unexpected state"));
    }
    let active = match (values["ActiveState"], values["SubState"]) {
        ("active", "running") => true,
        ("inactive", "dead") | ("failed", "failed") => false,
        ("activating", "start")
        | ("activating", "start-pre")
        | ("deactivating", "stop")
        | ("deactivating", "stop-sigterm") => false,
        _ => {
            return Ok(RuntimeTruth::Unknown(
                "unrecognized systemd active/substate pair".to_owned(),
            ));
        }
    };
    Ok(RuntimeTruth::Managed { active })
}

fn systemd_runtime_exec_matches(value: &str, launcher: &Path) -> bool {
    let Some(expected) = launcher.to_str() else {
        return false;
    };
    let Some(value) = value
        .strip_prefix("{ ")
        .and_then(|value| value.strip_suffix(" }"))
    else {
        return false;
    };
    let allowed = BTreeSet::from([
        "path",
        "argv[]",
        "ignore_errors",
        "start_time",
        "stop_time",
        "pid",
        "code",
        "status",
    ]);
    let mut fields = BTreeMap::new();
    for field in value.split(" ; ") {
        let Some((name, value)) = field.split_once('=') else {
            return false;
        };
        if !allowed.contains(name) || fields.insert(name, value).is_some() {
            return false;
        }
    }
    let Some(arguments) = fields
        .get("argv[]")
        .and_then(|value| parse_systemd_words(value))
    else {
        return false;
    };
    fields.get("path") == Some(&expected)
        && arguments.first().is_some_and(|value| value == expected)
        && managed_command_tail(&arguments[1..])
        && fields.get("ignore_errors") == Some(&"no")
}

fn observe_launchd_registration(
    label: &str,
    canonical: &Path,
    launchers: &[PathBuf],
    program_only: bool,
) -> Result<RuntimeTruth, String> {
    let uid = nix::unistd::Uid::effective().as_raw();
    let result = run_fixed(
        launchctl(&["print", &format!("gui/{uid}/{label}")]),
        OBSERVATION_TIMEOUT,
    )?;
    let absent =
        format!("Bad request.\nCould not find service \"{label}\" in domain for user gui: {uid}\n");
    if result.code == 113 && result.stdout.is_empty() && result.stderr == absent.as_bytes() {
        return Ok(RuntimeTruth::Absent);
    }
    if result.code != 0 || !result.stderr.is_empty() {
        return Ok(RuntimeTruth::Unknown(result_message(&result)));
    }
    classify_launchd_runtime(&result, label, canonical, launchers, uid, program_only)
}

fn classify_launchd_runtime(
    result: &CommandResult,
    label: &str,
    canonical: &Path,
    launchers: &[PathBuf],
    uid: u32,
    program_only: bool,
) -> Result<RuntimeTruth, String> {
    if result.code != 0 || !result.stderr.is_empty() {
        return Ok(RuntimeTruth::Unknown(result_message(result)));
    }
    let text = std::str::from_utf8(&result.stdout)
        .map_err(|_| "launchd observation was not UTF-8".to_owned())?;
    if !text.starts_with(&format!("gui/{uid}/{label} = {{\n"))
        || field(text, "path") != Some(path_text(canonical)?)
        || field(text, "type") != Some("LaunchAgent")
    {
        return Ok(RuntimeTruth::Foreign(
            "the launch agent is not the one journal manages",
        ));
    }
    let Some(arguments) = launchd_arguments(text) else {
        return Ok(RuntimeTruth::Unknown(
            "launchd omitted or malformed arguments".to_owned(),
        ));
    };
    if !launchers.iter().any(|launcher| {
        field(text, "program") == launcher.to_str()
            && arguments.first().copied() == launcher.to_str()
    }) || !(managed_command_tail(&arguments[1..]) || (program_only && arguments.len() == 1))
    {
        return Ok(RuntimeTruth::Foreign(
            "the launch agent's program is not a journal launcher",
        ));
    }
    match field(text, "state") {
        Some("running") => Ok(RuntimeTruth::Managed { active: true }),
        Some(_) => Ok(RuntimeTruth::Managed { active: false }),
        None => Ok(RuntimeTruth::Unknown(
            "launchd omitted the runtime state".to_owned(),
        )),
    }
}

fn launchd_arguments(text: &str) -> Option<Vec<&str>> {
    let tail = text.split_once("\targuments = {\n")?.1;
    let block = tail.split_once("\t}\n")?.0;
    let arguments = block
        .lines()
        .map(|line| line.strip_prefix("\t\t"))
        .collect::<Option<Vec<_>>>()?;
    (!arguments.is_empty()).then_some(arguments)
}

fn managed_command_tail(arguments: &[impl AsRef<str>]) -> bool {
    match arguments {
        [verb, port]
            if matches!(verb.as_ref(), "start" | "supervisor")
                && solstone_core_operational_logs::parse_service_port(port.as_ref()).is_ok() =>
        {
            true
        }
        [verb] if verb.as_ref() == "supervisor" => true,
        _ => false,
    }
}

fn expected_launchers(platform: Platform, unit: &Path) -> Option<[PathBuf; 3]> {
    let home = match platform {
        Platform::Linux => unit.ancestors().nth(4),
        Platform::Darwin => unit.ancestors().nth(3),
    }?;
    Some([
        home.join(".local/bin/journal"),
        home.join(".local/bin/solstone"),
        home.join(".local/bin/sol"),
    ])
}

fn wait_launchd_absent(
    label: &str,
    target: &Path,
    launchers: &[PathBuf],
    program_only: bool,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match observe_launchd_registration(label, target, launchers, program_only)? {
            RuntimeTruth::Absent => return Ok(()),
            RuntimeTruth::Managed { .. } | RuntimeTruth::Unknown(_)
                if Instant::now() < deadline =>
            {
                thread::sleep(POLL_INTERVAL);
            }
            RuntimeTruth::Managed { .. } => {
                return Err(
                    "launchd accepted the unload request, but the service is still present"
                        .to_owned(),
                );
            }
            other => return Err(runtime_error("verify unload", other)),
        }
    }
}

fn wait_runtime_absent(platform: Platform, target: &Path) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match observe_runtime(platform, target)? {
            RuntimeTruth::Absent => return Ok(()),
            RuntimeTruth::Managed { .. } | RuntimeTruth::Unknown(_)
                if Instant::now() < deadline =>
            {
                thread::sleep(POLL_INTERVAL);
            }
            RuntimeTruth::Managed { .. } => {
                return Err(
                    "launchd accepted the unload request, but the service is still present"
                        .to_owned(),
                );
            }
            other => return Err(runtime_error("verify unload", other)),
        }
    }
}

fn wait_runtime_quiescent(platform: Platform, target: &Path) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match observe_runtime(platform, target)? {
            RuntimeTruth::Absent => return Ok(()),
            RuntimeTruth::Managed { active: false } if platform == Platform::Linux => return Ok(()),
            RuntimeTruth::Managed { .. } | RuntimeTruth::Unknown(_)
                if Instant::now() < deadline =>
            {
                thread::sleep(POLL_INTERVAL);
            }
            RuntimeTruth::Managed { .. } => {
                return Err("the previous service did not stop before replacement".to_owned());
            }
            other => return Err(runtime_error("verify service stop", other)),
        }
    }
}

fn require_owned_runtime(
    platform: Platform,
    target: &Path,
    allow_absent: bool,
) -> Result<RuntimeTruth, String> {
    let truth = observe_runtime(platform, target)?;
    match truth {
        RuntimeTruth::Managed { .. } => Ok(truth),
        RuntimeTruth::Absent if allow_absent => Ok(truth),
        other => Err(runtime_error("operate", other)),
    }
}

fn require_managed(platform: Platform, path: &Path) -> Result<UnitSnapshot, String> {
    match classify_unit(platform, path)? {
        UnitTruth::Managed(snapshot) => Ok(snapshot),
        UnitTruth::Absent => Err(not_installed()),
        other => Err(truth_error("operate", path, &other)),
    }
}

fn revalidate_initial(platform: Platform, path: &Path, initial: &UnitTruth) -> Result<(), String> {
    match (initial, classify_unit(platform, path)?) {
        (UnitTruth::Absent, UnitTruth::Absent) => Ok(()),
        (UnitTruth::Managed(expected), UnitTruth::Managed(actual))
            if same_snapshot(expected, &actual) =>
        {
            Ok(())
        }
        _ => Err("service unit changed before publication; nothing was written".to_owned()),
    }
}

fn verify_snapshot(platform: Platform, path: &Path, expected: &UnitSnapshot) -> Result<(), String> {
    match classify_unit(platform, path)? {
        UnitTruth::Managed(actual) if same_snapshot(expected, &actual) => Ok(()),
        _ => Err("service unit changed during operation; preserved replacement".to_owned()),
    }
}

fn same_snapshot(left: &UnitSnapshot, right: &UnitSnapshot) -> bool {
    left.device == right.device && left.inode == right.inode && left.bytes == right.bytes
}

fn ensure_real_dir_chain(home: &Path, target: &Path) -> Result<(), String> {
    let relative = target
        .strip_prefix(home)
        .map_err(|_| "service directory escaped home".to_owned())?;
    let mut current = home.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            return Err("service directory contains unsafe component".to_owned());
        };
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                return Err(format!(
                    "unsafe service directory: {}",
                    path_display(&current)
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&current)
                    .map_err(|source| format!("create service directory: {source}"))?;
            }
            Err(error) => return Err(format!("inspect service directory: {error}")),
        }
    }
    Ok(())
}

fn ensure_health_dir(journal: &Path) -> Result<(), String> {
    match fs::symlink_metadata(journal) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => return Err("service journal root is not a real directory".to_owned()),
        Err(error) => return Err(format!("inspect service journal root: {error}")),
    }
    let health = journal.join("health");
    match fs::symlink_metadata(&health) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err("service health path is not a real directory".to_owned()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&health)
            .map_err(|source| format!("create service health directory: {source}")),
        Err(error) => Err(format!("inspect service health directory: {error}")),
    }
}

fn require_published(
    result: Result<DetailedAtomicOutcome, solstone_core_journal_io::DetailedAtomicError>,
) -> Result<(), String> {
    match result {
        Ok(DetailedAtomicOutcome::Published) => Ok(()),
        Ok(other) => Err(format!(
            "service unit published with uncertain durability: {other:?}"
        )),
        Err(error) => Err(format!("service unit publication failed: {error}")),
    }
}

fn systemctl(args: &[&str]) -> Command {
    let mut command = Command::new("/usr/bin/systemctl");
    command
        .args(args)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
    command
}

fn launchctl(args: &[&str]) -> Command {
    let mut command = Command::new("/bin/launchctl");
    command
        .args(args)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
    command
}

fn run_fixed(mut command: Command, timeout: Duration) -> Result<CommandResult, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("service command spawn failed: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "service stdout unavailable".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "service stderr unavailable".to_owned())?;
    let out_thread = thread::spawn(move || read_bounded(stdout));
    let err_thread = thread::spawn(move || read_bounded(stderr));
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = out_thread.join();
                let _ = err_thread.join();
                return Err("service command timed out".to_owned());
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = out_thread.join();
                let _ = err_thread.join();
                return Err(format!("service command wait failed: {error}"));
            }
        }
    };
    let stdout = out_thread
        .join()
        .map_err(|_| "service stdout reader panicked".to_owned())??;
    let stderr = err_thread
        .join()
        .map_err(|_| "service stderr reader panicked".to_owned())??;
    Ok(CommandResult {
        code: status.code().unwrap_or(128 + status.signal().unwrap_or(0)),
        stdout,
        stderr,
    })
}

fn read_bounded(reader: impl Read) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(OUTPUT_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("service command output failed: {error}"))?;
    if bytes.len() as u64 > OUTPUT_LIMIT {
        Err("service command output exceeded 256 KiB".to_owned())
    } else {
        Ok(bytes)
    }
}

fn require_success(result: CommandResult, operation: &str) -> Result<(), String> {
    if result.code == 0 {
        Ok(())
    } else {
        Err(format!("{operation} failed: {}", result_message(&result)))
    }
}

fn result_message(result: &CommandResult) -> String {
    format!(
        "exit={} stdout={} stderr={}",
        result.code,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    )
}

fn field<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    text.lines()
        .find_map(|line| line.strip_prefix(&format!("\t{name} = ")))
}

fn truth_error(operation: &str, path: &Path, truth: &UnitTruth) -> String {
    let detail = match truth {
        UnitTruth::Foreign => "not recognized as a service file created by journal".to_owned(),
        UnitTruth::Unknown(cause) => format!("could not verify the service file: {cause}"),
        UnitTruth::Absent => "absent".to_owned(),
        UnitTruth::Managed(_) => "managed".to_owned(),
    };
    format!(
        "service {operation} refused {}: {detail}",
        path_display(path)
    )
}

fn runtime_error(operation: &str, truth: RuntimeTruth) -> String {
    match truth {
        RuntimeTruth::Foreign(reason) => format!(
            "service {operation} refused: {reason}. journal will not overwrite it. \
             If that is deliberate, finish the {operation} yourself -- on Linux: \
             `systemctl --user daemon-reload && systemctl --user restart solstone.service` \
             -- then confirm the running build with \
             `readlink /proc/$(systemctl --user show solstone.service -p MainPID --value)/exe`"
        ),
        RuntimeTruth::Unknown(cause) => {
            format!("service {operation} could not check the service registration: {cause}")
        }
        _ => format!("service {operation} runtime state was unexpected"),
    }
}

fn not_installed() -> String {
    "service not installed. run 'journal service install' first.".to_owned()
}

fn path_text(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| "service path is not UTF-8".to_owned())
}

fn path_display(path: &Path) -> String {
    solstone_core_system_health::sanitize_os_bytes_for_terminal(path.as_os_str().as_encoded_bytes())
}

fn safe(value: &str) -> String {
    solstone_core_system_health::sanitize_for_terminal(value)
}

/// Rewrite `<root>/versions/<version>/bin` to `<root>/current/bin`.
///
/// 🔴 This unit's `PATH` is written once, at setup, and read forever after. Baking the
/// *resolved* version directory into it means every PATH-resolved subprocess keeps
/// running whichever build was current when setup last succeeded -- and setup refuses
/// whenever the unit carries a user drop-in, so "last succeeded" can be many deploys
/// ago. Measured on the founder's journal 2026-09-02: the supervisor was running a new
/// build while `think` and `transcribe` workers, spawned through `PATH`, were still
/// executing a binary from two deploys earlier, silently missing every fix in between.
///
/// Naming `current/bin` instead makes the unit version-independent, so a deploy that
/// only flips the symlink is enough and a stale unit can no longer pin an old build.
/// A directory that is not in the versioned layout (a dev build, a test tree) is
/// returned unchanged.
pub(crate) fn version_independent_runtime_dir(runtime_dir: &Path) -> PathBuf {
    let mut components: Vec<_> = runtime_dir.components().collect();
    // Expect the tail to be `versions/<version>/bin`.
    if components.len() < 3 {
        return runtime_dir.to_path_buf();
    }
    let bin = components.pop().expect("checked length");
    let _version = components.pop().expect("checked length");
    let versions = components.pop().expect("checked length");
    if bin.as_os_str() != "bin" || versions.as_os_str() != "versions" {
        return runtime_dir.to_path_buf();
    }
    let mut stable: PathBuf = components.iter().collect();
    stable.push("current");
    stable.push("bin");
    stable
}

#[cfg(test)]
mod tests {
    /// The unit's `PATH` is written once and read forever. A resolved version
    /// directory in it pins every PATH-resolved subprocess to whichever build was
    /// current when setup last succeeded -- and setup refuses while a user drop-in
    /// exists, so that can be several deploys stale.
    #[test]
    fn the_service_path_names_current_not_a_resolved_version() {
        use super::version_independent_runtime_dir;
        assert_eq!(
            version_independent_runtime_dir(Path::new(
                "/home/owner/.local/solstone-journal/versions/2.0.0-7568daa6e1c9/bin"
            )),
            Path::new("/home/owner/.local/solstone-journal/current/bin")
        );
        // A tree that is not the versioned layout is left alone.
        for untouched in [
            "/home/owner/.local/bin",
            "/usr/bin",
            "/home/owner/src/solstone/target/debug",
        ] {
            assert_eq!(
                version_independent_runtime_dir(Path::new(untouched)),
                Path::new(untouched),
                "{untouched}"
            );
        }
    }

    use super::*;

    #[test]
    fn clean_sync_rescan_preserves_ready_timeout_message() {
        let journal = tempfile::tempdir_in("/var/tmp").unwrap();

        assert_eq!(ready_timeout_message(journal.path()), READY_TIMEOUT_MESSAGE);
        assert!(!journal.path().join("health").exists());
    }

    #[test]
    fn final_sync_diagnoses_are_not_wrapped_or_sanitized_again() {
        assert!(is_final_sync_diagnosis(
            "Installation: needs attention\nyour journal contains an item that can't be checked safely."
        ));
        assert!(is_final_sync_diagnosis(
            "Installation: needs attention\nstartup status couldn't be verified."
        ));
        assert!(!is_final_sync_diagnosis(
            "Installation: waiting\na recent heartbeat from another run is present."
        ));
        assert!(!is_final_sync_diagnosis(READY_TIMEOUT_MESSAGE));
    }

    fn test_guard() -> GuardFields {
        parse_service_guard_environment(&BTreeMap::from([
            (
                "SOLSTONE_INSTALLATION_NAMESPACE".to_owned(),
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned(),
            ),
            (
                "SOLSTONE_INSTALLATION_ID".to_owned(),
                "0123456789abcdef0123456789abcdef".to_owned(),
            ),
            (
                "SOLSTONE_INSTALLATION_GENERATION".to_owned(),
                "1".to_owned(),
            ),
            (
                "SOLSTONE_INSTALLATION_JOURNAL_TOKEN".to_owned(),
                "2f6a6f75726e616c".to_owned(),
            ),
        ]))
        .unwrap()
        .unwrap()
    }

    fn guarded_environment() -> BTreeMap<String, String> {
        let mut environment = BTreeMap::from([
            ("HOME".to_owned(), "/home/owner".to_owned()),
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("PYTHONUNBUFFERED".to_owned(), "1".to_owned()),
        ]);
        let guard = test_guard();
        environment.extend(solstone_core_installation_identity::service_guard_environment(&guard));
        environment
    }

    #[test]
    fn direct_service_install_loads_the_existing_guard() {
        let expected = test_guard();
        let actual = resolve_installation_guard(None, || Ok(expected.clone())).unwrap();

        assert_eq!(actual, expected);
    }

    #[test]
    fn direct_service_install_offers_setup_when_the_existing_guard_is_unavailable() {
        let error = resolve_installation_guard(None, || {
            Err("the saved installation record is unreadable".to_owned())
        })
        .unwrap_err();

        assert_eq!(
            error,
            installation_recovery_copy("the saved installation record is unreadable")
        );
    }

    #[test]
    fn matching_inherited_guard_activates_service_capture() {
        let expected = test_guard();
        let environment = solstone_core_installation_identity::service_guard_environment(&expected);
        assert!(environment_matches_installation_guard(
            &environment,
            || Ok::<_, ()>(PathBuf::from("/home/owner")),
            |home| {
                assert_eq!(home, Path::new("/home/owner"));
                Ok::<_, ()>(expected)
            },
        ));
    }

    #[test]
    fn partial_inherited_guard_does_not_activate_service_capture() {
        let environment =
            solstone_core_installation_identity::service_guard_environment(&test_guard());
        for missing_mask in 1..(1 << SERVICE_GUARD_ENVIRONMENT_NAMES.len()) {
            let mut partial = environment.clone();
            for (index, name) in SERVICE_GUARD_ENVIRONMENT_NAMES.into_iter().enumerate() {
                if missing_mask & (1 << index) != 0 {
                    partial.remove(name);
                }
            }
            assert!(!environment_matches_installation_guard(
                &partial,
                || -> Result<PathBuf, ()> { panic!("partial guard must not resolve a home") },
                |_| -> Result<GuardFields, ()> { panic!("partial guard must not load a binding") },
            ));
        }
    }

    #[test]
    fn malformed_inherited_guard_does_not_activate_service_capture() {
        let mut environment =
            solstone_core_installation_identity::service_guard_environment(&test_guard());
        environment.insert(
            "SOLSTONE_INSTALLATION_GENERATION".to_owned(),
            "not-a-generation".to_owned(),
        );
        assert!(!environment_matches_installation_guard(
            &environment,
            || -> Result<PathBuf, ()> { panic!("malformed guard must not resolve a home") },
            |_| -> Result<GuardFields, ()> { panic!("malformed guard must not load a binding") },
        ));
    }

    #[test]
    fn mismatched_inherited_guard_does_not_activate_service_capture() {
        let expected = test_guard();
        let environment = solstone_core_installation_identity::service_guard_environment(&expected);
        for (name, value) in [
            ("SOLSTONE_INSTALLATION_GENERATION", "2"),
            ("SOLSTONE_INSTALLATION_JOURNAL_TOKEN", "3f6a6f75726e616c"),
        ] {
            let mut mismatched = environment.clone();
            mismatched.insert(name.to_owned(), value.to_owned());
            assert!(!environment_matches_installation_guard(
                &mismatched,
                || Ok::<_, ()>(PathBuf::from("/home/owner")),
                |_| Ok::<_, ()>(expected.clone()),
            ));
        }
    }

    #[test]
    fn unavailable_installation_binding_does_not_activate_service_capture() {
        let environment =
            solstone_core_installation_identity::service_guard_environment(&test_guard());
        assert!(!environment_matches_installation_guard(
            &environment,
            || Ok::<_, ()>(PathBuf::from("/home/owner")),
            |_| Err::<GuardFields, _>(()),
        ));
    }

    #[test]
    fn unit_truth_accepts_current_renderers_and_rejects_extensions() {
        let environment = BTreeMap::from([
            ("HOME".to_owned(), "/home/owner".to_owned()),
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("PYTHONUNBUFFERED".to_owned(), "1".to_owned()),
        ]);
        let systemd = render_systemd_unit(&environment, "/home/owner/.local/bin/journal", "5015");
        let launcher = Path::new("/home/owner/.local/bin/journal");
        assert!(systemd_managed(systemd.as_bytes(), launcher));
        assert!(!systemd_managed(
            systemd
                .replace("KillMode=control-group\n", "")
                .replace("TimeoutStopSec=30\n", "")
                .as_bytes(),
            launcher,
        ));
        assert!(!systemd_managed(
            systemd.replace("KillMode=control-group\n", "").as_bytes(),
            launcher,
        ));
        assert!(!systemd_managed(
            format!("{systemd}ExecStartPre=/bin/false\n").as_bytes(),
            launcher,
        ));
        assert!(!systemd_managed(
            systemd
                .replace("KillMode=control-group", "KillMode=process")
                .as_bytes(),
            launcher,
        ));
        let plist = render_launchd_plist(&environment, "/home/owner/.local/bin/journal", "5015");
        assert!(launchd_managed(&plist, launcher, LABEL));
        let hostile_launcher = Path::new("/tmp/foreign/journal");
        assert!(!launchd_managed(&plist, hostile_launcher, LABEL));
    }

    #[test]
    fn guarded_units_are_managed_only_with_a_complete_valid_guard() {
        let environment = guarded_environment();
        let launcher = Path::new("/home/owner/.local/bin/journal");
        let systemd = render_systemd_unit(&environment, path_text(launcher).unwrap(), "5015");
        assert!(systemd_managed(systemd.as_bytes(), launcher));
        assert!(systemd_unit_has_installation_guard(systemd.as_bytes()));
        assert!(!systemd_managed(
            systemd
                .replace(
                    "Environment=SOLSTONE_INSTALLATION_ID=0123456789abcdef0123456789abcdef\n",
                    "",
                )
                .as_bytes(),
            launcher,
        ));
        let plist = render_launchd_plist(&environment, path_text(launcher).unwrap(), "5015");
        assert!(launchd_managed(&plist, launcher, LABEL));
        let mut partial = plist::Value::from_reader_xml(plist.as_slice()).unwrap();
        partial
            .as_dictionary_mut()
            .unwrap()
            .get_mut("EnvironmentVariables")
            .unwrap()
            .as_dictionary_mut()
            .unwrap()
            .remove("SOLSTONE_INSTALLATION_ID");
        let mut partial_bytes = Vec::new();
        partial.to_writer_xml(&mut partial_bytes).unwrap();
        assert!(!launchd_managed(&partial_bytes, launcher, LABEL));
    }

    #[test]
    fn legacy_supervisor_lock_upgrade_is_exact_and_fail_closed() {
        let root = tempfile::tempdir_in("/var/tmp").unwrap();
        let journal = root.path().join("journal");
        let health = journal.join("health");
        fs::create_dir_all(&health).unwrap();
        let lock = health.join("supervisor.lock");

        upgrade_legacy_supervisor_lock(&journal).unwrap();
        fs::write(&lock, b"").unwrap();
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o644)).unwrap();
        upgrade_legacy_supervisor_lock(&journal).unwrap();
        assert_eq!(
            fs::metadata(&lock).unwrap().permissions().mode() & 0o777,
            0o600
        );

        fs::set_permissions(&lock, fs::Permissions::from_mode(0o640)).unwrap();
        let error = upgrade_legacy_supervisor_lock(&journal).unwrap_err();
        assert!(error.contains("cannot be upgraded safely"));
        assert_eq!(
            fs::metadata(&lock).unwrap().permissions().mode() & 0o777,
            0o640
        );

        fs::set_permissions(&lock, fs::Permissions::from_mode(0o4644)).unwrap();
        let error = upgrade_legacy_supervisor_lock(&journal).unwrap_err();
        assert!(error.contains("cannot be upgraded safely"));
        assert_eq!(
            fs::metadata(&lock).unwrap().permissions().mode() & 0o7777,
            0o4644
        );

        let outside = root.path().join("outside.lock");
        fs::write(&outside, b"").unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o644)).unwrap();
        fs::remove_file(&lock).unwrap();
        std::os::unix::fs::symlink(&outside, &lock).unwrap();
        let error = upgrade_legacy_supervisor_lock(&journal).unwrap_err();
        assert!(error.contains("not a regular file"));
        assert_eq!(
            fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    #[test]
    fn unit_truth_derives_launcher_and_unbounded_port_values() {
        let launcher = Path::new("/home/owner space/$runtime%/.local/bin/journal");
        let environment = BTreeMap::from([
            ("HOME".to_owned(), "/home/owner space/$runtime%".to_owned()),
            ("PATH".to_owned(), "/runtime:/usr/bin:/bin".to_owned()),
            ("PYTHONUNBUFFERED".to_owned(), "1".to_owned()),
        ]);
        let port = "123456789012345678901234567890";
        let systemd = render_systemd_unit(&environment, path_text(launcher).unwrap(), port);
        assert!(systemd_managed(systemd.as_bytes(), launcher));
        let plist = render_launchd_plist(&environment, path_text(launcher).unwrap(), port);
        assert!(launchd_managed(&plist, launcher, LABEL));
    }

    #[test]
    fn historical_wrappers_and_units_remain_managed() {
        let sol = Path::new("/home/owner/.local/bin/solstone");
        let legacy_systemd = concat!(
            "[Unit]\n",
            "Description=Solstone Supervisor\n",
            "After=default.target\n",
            "\n[Service]\n",
            "Type=simple\n",
            "Environment=HOME=/home/owner\n",
            "Environment=PATH=/usr/bin:/bin\n",
            "ExecStart=/home/owner/.local/bin/solstone supervisor 5015\n",
            "Restart=on-failure\n",
            "RestartSec=5\n",
            "\n[Install]\n",
            "WantedBy=default.target\n",
        );
        assert!(systemd_managed(legacy_systemd.as_bytes(), sol));
        assert!(!systemd_unit_has_installation_guard(
            legacy_systemd.as_bytes()
        ));

        let environment = BTreeMap::from([
            ("HOME".to_owned(), "/home/owner".to_owned()),
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("PYTHONUNBUFFERED".to_owned(), "1".to_owned()),
        ]);
        let current = render_launchd_plist(&environment, "/home/owner/.local/bin/journal", "5015");
        let historical = String::from_utf8(current)
            .unwrap()
            .replace(
                "/home/owner/.local/bin/journal",
                "/home/owner/.local/bin/solstone",
            )
            .replace("<string>start</string>", "<string>supervisor</string>");
        assert!(launchd_managed(historical.as_bytes(), sol, LABEL));
    }

    #[test]
    fn runtime_truth_requires_complete_managed_identity() {
        let canonical = Path::new("/home/owner/.config/systemd/user/solstone.service");
        let launchers = [
            PathBuf::from("/home/owner/.local/bin/journal"),
            PathBuf::from("/home/owner/.local/bin/solstone"),
            PathBuf::from("/home/owner/.local/bin/sol"),
        ];
        let absent = CommandResult {
            code: 0,
            stdout: concat!(
                "Id=solstone.service\n",
                "LoadState=not-found\n",
                "ActiveState=inactive\n",
                "SubState=dead\n",
                "FragmentPath=\n",
                "SourcePath=\n",
                "DropInPaths=\n",
                "UnitFileState=\n",
            )
            .as_bytes()
            .to_vec(),
            stderr: Vec::new(),
        };
        assert_eq!(
            classify_systemd_runtime(&absent, canonical, &launchers).unwrap(),
            RuntimeTruth::Absent
        );

        let loaded_stdout = concat!(
            "Id=solstone.service\n",
            "LoadState=loaded\n",
            "ActiveState=active\n",
            "SubState=running\n",
            "FragmentPath=/home/owner/.config/systemd/user/solstone.service\n",
            "SourcePath=\n",
            "DropInPaths=\n",
            "ExecStart={ path=/home/owner/.local/bin/solstone ; argv[]=/home/owner/.local/bin/solstone supervisor 5015 ; ignore_errors=no }\n",
            "UnitFileState=enabled\n",
        );
        let loaded = CommandResult {
            code: 0,
            stdout: loaded_stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        };
        assert_eq!(
            classify_systemd_runtime(&loaded, canonical, &launchers).unwrap(),
            RuntimeTruth::Managed { active: true }
        );
        let legacy_sol = CommandResult {
            code: 0,
            stdout: loaded_stdout
                .replace("/.local/bin/solstone", "/.local/bin/sol")
                .into_bytes(),
            stderr: Vec::new(),
        };
        assert_eq!(
            classify_systemd_runtime(&legacy_sol, canonical, &launchers).unwrap(),
            RuntimeTruth::Managed { active: true }
        );
        let near_twin = CommandResult {
            code: 0,
            stdout: loaded_stdout
                .replace("/.local/bin/solstone", "/.local/bin/sol-old")
                .into_bytes(),
            stderr: Vec::new(),
        };
        assert!(matches!(
            classify_systemd_runtime(&near_twin, canonical, &launchers).unwrap(),
            RuntimeTruth::Foreign(_)
        ));
        let drop_in = CommandResult {
            code: 0,
            stdout: loaded_stdout
                .replace("DropInPaths=\n", "DropInPaths=/tmp/foreign.conf\n")
                .into_bytes(),
            stderr: Vec::new(),
        };
        assert!(matches!(
            classify_systemd_runtime(&drop_in, canonical, &launchers).unwrap(),
            RuntimeTruth::Foreign(_)
        ));
        assert!(!trusted_systemd_vendor_drop_in(Path::new(
            "/etc/systemd/user/service.d/foreign.conf"
        )));
        assert!(!trusted_systemd_vendor_drop_in(Path::new(
            "/usr/lib/systemd/user/service.d/nested/foreign.conf"
        )));
        assert!(!trusted_systemd_vendor_drop_in(Path::new(
            "/usr/lib/systemd/user/service.d/foreign.txt"
        )));
        assert!(!trusted_root_owned_directory(Path::new("/tmp")));
    }

    #[test]
    fn stop_plan_preserves_absent_and_runtime_ownership_failures() {
        assert_eq!(
            stop_requires_manager(&UnitTruth::Absent, RuntimeTruth::Absent).unwrap_err(),
            "service not installed. run 'journal service install' first."
        );
        // The refusal now names which check rejected the unit and how to finish the
        // operation by hand -- the bare message was unactionable.
        let refusal = stop_requires_manager(
            &UnitTruth::Absent,
            RuntimeTruth::Foreign("the unit carries a drop-in journal did not write"),
        )
        .unwrap_err();
        assert!(
            refusal.contains("the unit carries a drop-in journal did not write"),
            "{refusal}"
        );
        assert!(refusal.contains("systemctl --user restart"), "{refusal}");
        let managed_unit = UnitTruth::Managed(UnitSnapshot {
            device: 1,
            inode: 2,
            bytes: Vec::new(),
        });
        assert!(!stop_requires_manager(&managed_unit, RuntimeTruth::Absent).unwrap());
        assert!(
            !stop_requires_manager(&UnitTruth::Absent, RuntimeTruth::Managed { active: false },)
                .unwrap()
        );
        assert!(
            stop_requires_manager(&UnitTruth::Absent, RuntimeTruth::Managed { active: true },)
                .unwrap()
        );
    }

    #[test]
    fn launchd_runtime_requires_path_program_and_arguments() {
        let canonical = Path::new("/home/owner/Library/LaunchAgents/org.solpbc.solstone.plist");
        let launchers = [
            PathBuf::from("/home/owner/.local/bin/journal"),
            PathBuf::from("/home/owner/.local/bin/solstone"),
        ];
        let stdout = concat!(
            "gui/501/org.solpbc.solstone = {\n",
            "\tpath = /home/owner/Library/LaunchAgents/org.solpbc.solstone.plist\n",
            "\ttype = LaunchAgent\n",
            "\tstate = running\n",
            "\tprogram = /home/owner/.local/bin/solstone\n",
            "\targuments = {\n",
            "\t\t/home/owner/.local/bin/solstone\n",
            "\t\tsupervisor\n",
            "\t\t5015\n",
            "\t}\n",
            "}\n",
        );
        let managed = CommandResult {
            code: 0,
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        };
        assert_eq!(
            classify_launchd_runtime(&managed, LABEL, canonical, &launchers, 501, false).unwrap(),
            RuntimeTruth::Managed { active: true }
        );
        let foreign = CommandResult {
            code: 0,
            stdout: stdout.replace("\t\t5015\n", "\t\t--foreign\n").into_bytes(),
            stderr: Vec::new(),
        };
        assert!(matches!(
            classify_launchd_runtime(&foreign, LABEL, canonical, &launchers, 501, false).unwrap(),
            RuntimeTruth::Foreign(_)
        ));
        let pending = CommandResult {
            code: 0,
            stdout: stdout
                .replace("\tstate = running\n", "\tstate = pending\n")
                .into_bytes(),
            stderr: Vec::new(),
        };
        assert_eq!(
            classify_launchd_runtime(&pending, LABEL, canonical, &launchers, 501, false).unwrap(),
            RuntimeTruth::Managed { active: false }
        );
    }

    #[test]
    fn stale_launchd_truth_accepts_retained_program_shapes() {
        let launcher = Path::new("/old/checkout/.venv/bin/solstone");
        for value in [
            plist::Value::Dictionary(plist::Dictionary::from_iter([
                ("Label".to_owned(), plist::Value::String(LABEL.to_owned())),
                (
                    "ProgramArguments".to_owned(),
                    plist::Value::Array(vec![
                        plist::Value::String(path_text(launcher).unwrap().to_owned()),
                        plist::Value::String("supervisor".to_owned()),
                        plist::Value::String("5015".to_owned()),
                    ]),
                ),
            ])),
            plist::Value::Dictionary(plist::Dictionary::from_iter([
                ("Label".to_owned(), plist::Value::String(LABEL.to_owned())),
                (
                    "Program".to_owned(),
                    plist::Value::String(path_text(launcher).unwrap().to_owned()),
                ),
            ])),
        ] {
            assert!(stale_launchd_managed(
                value.as_dictionary().unwrap(),
                LABEL,
                launcher
            ));
        }
    }
}
