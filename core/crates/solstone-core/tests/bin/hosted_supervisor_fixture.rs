// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Test-only hosted supervisor process. A real fixture is required because
//! direct-parent birth identity and parent death cannot be represented by an
//! in-process mock without losing the OS relationship under test.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

#[cfg(target_os = "macos")]
use std::collections::BTreeMap;
#[cfg(target_os = "macos")]
use std::ffi::OsString;
#[cfg(target_os = "macos")]
use std::net::TcpListener;
#[cfg(target_os = "macos")]
use std::time::Duration;

use solstone_core::supervisor::{
    SupervisorHostOutcome, receipt::write_hosted_supervisor_receipt, run_hosted,
};
use solstone_core_cli::SupervisorOptions;
use solstone_core_system::lifecycle::{
    CoordinatorBootstrap, DeclaredParent, HostedServiceKind, ParentLossCoordinator,
    arm_parent_loss_coordinator_termination_guard,
};
#[cfg(target_os = "macos")]
use solstone_core_system::lifecycle::{
    HostedServiceParentRuntime, HostedServiceShutdownEvidence, acknowledge_hosted_child_admission,
    admit_hosted_service_parent,
};
#[cfg(target_os = "macos")]
use solstone_core_system::process::{
    BoxedTerminateFn, CommandLaunchRequest, Disposition, LaunchAuthority, LaunchError,
    launch_command_hosted,
};
use solstone_core_system::process::{ProcessBirth, ProcessInstance};

#[cfg(target_os = "macos")]
const DARWIN_PARENT_LIFETIME_FIXTURE_ENV: &str = "SOLSTONE_DARWIN_PARENT_LIFETIME_FIXTURE";
#[cfg(target_os = "macos")]
const DARWIN_PARENT_LIFETIME_MODE_ENV: &str = "SOLSTONE_DARWIN_PARENT_LIFETIME_MODE";

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
        hosted_parent: false,
    }
}

fn run_hosted_fixture(
    journal: PathBuf,
    outcome: PathBuf,
    nonce: String,
    parent: DeclaredParent,
) -> ExitCode {
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
    if let Err(error) = write_hosted_supervisor_receipt(&outcome, &nonce, &result) {
        eprintln!("hosted fixture: outcome write failed: {error}");
        return ExitCode::from(75);
    }
    match result {
        SupervisorHostOutcome::OrderlyShutdown { .. }
        | SupervisorHostOutcome::ForcedShutdownAfterGraceTimeout { .. }
        | SupervisorHostOutcome::ParentLost { .. } => ExitCode::SUCCESS,
        SupervisorHostOutcome::LifecycleShutdownFailed { .. } => ExitCode::from(70),
        SupervisorHostOutcome::Refused { .. } => ExitCode::from(75),
    }
}

fn run_launcher(journal: PathBuf, child_pid: PathBuf, outcome: PathBuf, nonce: String) -> ExitCode {
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
        .arg(nonce)
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
        Ok(status) if status.code() == Some(70) => ExitCode::from(70),
        Ok(_) => ExitCode::from(75),
        Err(error) => {
            eprintln!("hosted fixture: child wait failed: {error}");
            ExitCode::from(75)
        }
    }
}

/// The supervisor fixture is itself the hosted supervisor executable, so it
/// must route the coordinator's hidden sibling verb back into the production
/// coordinator state machine. This preserves the normal `current_exe()`
/// launch shape used by `bootstrap_parent_loss_coordinator`.
fn run_parent_loss_coordinator(mut args: impl Iterator<Item = std::ffi::OsString>) -> ExitCode {
    if let Err(error) = arm_parent_loss_coordinator_termination_guard() {
        eprintln!("hosted fixture: coordinator could not block SIGTERM: {error}");
        return ExitCode::from(75);
    }
    let mut supervisor = None;
    let mut enabled = None;
    let mut supervisor_heartbeat_filename = None;
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--supervisor-json") => {
                supervisor = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .and_then(|value| serde_json::from_str::<ProcessInstance>(&value).ok());
            }
            Some("--enabled-json") => {
                enabled = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .and_then(|value| serde_json::from_str::<Vec<HostedServiceKind>>(&value).ok());
            }
            Some("--supervisor-heartbeat") => {
                supervisor_heartbeat_filename =
                    args.next().and_then(|value| value.into_string().ok());
            }
            _ => {}
        }
    }
    let (Some(supervisor), Some(enabled), Some(supervisor_heartbeat_filename)) =
        (supervisor, enabled, supervisor_heartbeat_filename)
    else {
        eprintln!("hosted fixture: invalid coordinator bootstrap arguments");
        return ExitCode::from(64);
    };
    let Some(journal) = std::env::var_os("SOLSTONE_JOURNAL").map(PathBuf::from) else {
        eprintln!("hosted fixture: coordinator has no journal");
        return ExitCode::from(75);
    };
    let mut capability = Vec::new();
    if let Err(error) = std::io::stdin().read_to_end(&mut capability) {
        eprintln!("hosted fixture: coordinator capability read failed: {error}");
        return ExitCode::from(75);
    }
    if capability.len() < 32 {
        eprintln!("hosted fixture: coordinator capability is missing");
        return ExitCode::from(64);
    }
    let (coordinator, _) = match ParentLossCoordinator::bootstrap(CoordinatorBootstrap {
        journal,
        supervisor,
        enabled,
        supervisor_heartbeat_filename,
        capability,
    }) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("hosted fixture: coordinator bootstrap failed: {error}");
            return ExitCode::from(75);
        }
    };
    match coordinator.run() {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("hosted fixture: coordinator failed: {error}");
            ExitCode::from(75)
        }
    }
}

#[cfg(target_os = "macos")]
fn darwin_parent_lifetime_fixture_enabled() -> bool {
    std::env::var(DARWIN_PARENT_LIFETIME_FIXTURE_ENV).as_deref() == Ok("1")
}

#[cfg(target_os = "macos")]
fn darwin_parent_lifetime_hostile_mode() -> bool {
    std::env::var(DARWIN_PARENT_LIFETIME_MODE_ENV).as_deref() == Ok("hostile-late-spawner")
}

#[cfg(target_os = "macos")]
fn run_darwin_hosted_service(
    kind: HostedServiceKind,
    marker: PathBuf,
    convey_port: Option<u16>,
) -> ExitCode {
    let Some(journal) = std::env::var_os("SOLSTONE_JOURNAL").map(PathBuf::from) else {
        eprintln!("hosted fixture: Darwin service has no journal");
        return ExitCode::from(75);
    };
    let parent = match admit_hosted_service_parent(&journal, kind) {
        Ok(Some(parent)) => parent,
        Ok(None) => {
            eprintln!("hosted fixture: Darwin service was not marked hosted");
            return ExitCode::from(75);
        }
        Err(error) => {
            eprintln!("hosted fixture: Darwin service admission failed: {error}");
            return ExitCode::from(75);
        }
    };
    let listener = match convey_port {
        Some(port) => match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => Some(listener),
            Err(error) => {
                eprintln!("hosted fixture: Darwin Convey listener failed: {error}");
                return ExitCode::from(75);
            }
        },
        None => None,
    };
    let _cortex_descendants = if kind == HostedServiceKind::Cortex {
        match spawn_darwin_cortex_descendants(
            &journal,
            &parent,
            darwin_parent_lifetime_hostile_mode(),
        ) {
            Ok(descendants) => Some(descendants),
            Err(error) => {
                eprintln!("hosted fixture: Darwin Cortex descendants failed: {error}");
                return ExitCode::from(75);
            }
        }
    } else {
        None
    };
    if let Err(error) = std::fs::write(&marker, std::process::id().to_string()) {
        eprintln!("hosted fixture: Darwin service readiness failed: {error}");
        return ExitCode::from(75);
    }

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("hosted fixture: Darwin service runtime failed: {error}");
            return ExitCode::from(75);
        }
    };
    runtime.block_on(parent.await_parent_loss());
    // The only fixture listener is Convey's test port. Dropping it is
    // infallible and happens before the coordination witness is recorded.
    drop(listener);
    match parent.finish_parent_loss(HostedServiceShutdownEvidence {
        listener_stopped: true,
        service_runner_stopped: true,
        operational_artifacts_cleaned: true,
    }) {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("hosted fixture: Darwin service coordination failed: {error}");
            ExitCode::from(70)
        }
    }
}

#[cfg(target_os = "macos")]
struct DarwinCortexDescendants {
    _authorities: Vec<LaunchAuthority>,
}

#[cfg(target_os = "macos")]
fn spawn_darwin_cortex_descendants(
    journal: &std::path::Path,
    parent: &HostedServiceParentRuntime,
    hostile_late_spawner: bool,
) -> Result<DarwinCortexDescendants, String> {
    let health = journal.join("health");
    let talent = health.join("darwin-talent-worker.pid");
    let late_spawner = health.join("darwin-late-spawner.pid");
    let late_child = health.join("darwin-late-descendant.pid");
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut authorities = vec![launch_darwin_fixture_child(
        &executable,
        if hostile_late_spawner {
            "darwin-term-resistant"
        } else {
            "darwin-cooperative-child"
        },
        vec![talent.as_os_str().to_os_string()],
        parent.child_launch_provenance("fixture-cortex-talent".to_owned()),
    )?];
    if hostile_late_spawner {
        authorities.push(launch_darwin_fixture_child(
            &executable,
            "darwin-late-spawner",
            vec![
                late_spawner.as_os_str().to_os_string(),
                late_child.as_os_str().to_os_string(),
            ],
            parent.child_launch_provenance("fixture-cortex-late-spawner".to_owned()),
        )?);
        wait_for_fixture_paths(&[talent, late_spawner])?;
    } else {
        wait_for_fixture_paths(&[talent])?;
    }
    Ok(DarwinCortexDescendants {
        _authorities: authorities,
    })
}

#[cfg(target_os = "macos")]
fn launch_darwin_fixture_child(
    executable: &std::path::Path,
    mode: &str,
    arguments: Vec<OsString>,
    provenance: solstone_core_system::process::HostedLaunchProvenance,
) -> Result<LaunchAuthority, String> {
    let request = CommandLaunchRequest {
        program: executable.as_os_str().to_os_string(),
        arguments: std::iter::once(OsString::from(mode))
            .chain(arguments)
            .collect(),
        environment: BTreeMap::new(),
        current_dir: None,
        process_group: false,
        stdin_piped: false,
        stdout_piped: false,
        stderr_piped: false,
    };
    launch_command_hosted(
        Disposition::IndependentBoundedHelper {
            timeout: Duration::from_secs(30),
        },
        request,
        provenance,
        darwin_fixture_child_terminate(),
    )
    .map_err(|error| error.to_string())
}

#[cfg(target_os = "macos")]
fn darwin_fixture_child_terminate() -> BoxedTerminateFn {
    Box::new(|child, _| child.kill().map_err(LaunchError::Terminate))
}

#[cfg(target_os = "macos")]
fn wait_for_fixture_paths(paths: &[PathBuf]) -> Result<(), String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if paths.iter().all(|path| {
            std::fs::read_to_string(path)
                .ok()
                .and_then(|contents| contents.trim().parse::<u32>().ok())
                .is_some()
        }) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    Err("Darwin child fixture did not become ready".to_owned())
}

#[cfg(target_os = "macos")]
fn block_sigterm() -> nix::sys::signal::SigSet {
    let mut signals = nix::sys::signal::SigSet::empty();
    signals.add(nix::sys::signal::Signal::SIGTERM);
    signals
        .thread_block()
        .expect("Darwin fixture blocks SIGTERM");
    signals
}

#[cfg(target_os = "macos")]
fn run_darwin_term_resistant(ready: PathBuf) -> ExitCode {
    if let Err(error) = acknowledge_darwin_fixture_child_admission() {
        eprintln!("hosted fixture: Darwin resistant child admission failed: {error}");
        return ExitCode::from(75);
    }
    let _signals = block_sigterm();
    if let Err(error) = std::fs::write(ready, std::process::id().to_string()) {
        eprintln!("hosted fixture: Darwin resistant child readiness failed: {error}");
        return ExitCode::from(75);
    }
    std::thread::sleep(std::time::Duration::from_secs(30));
    ExitCode::SUCCESS
}

#[cfg(target_os = "macos")]
fn run_darwin_cooperative_child(ready: PathBuf) -> ExitCode {
    if let Err(error) = acknowledge_darwin_fixture_child_admission() {
        eprintln!("hosted fixture: Darwin cooperative child admission failed: {error}");
        return ExitCode::from(75);
    }
    let _signals = block_sigterm();
    if let Err(error) = std::fs::write(ready, std::process::id().to_string()) {
        eprintln!("hosted fixture: Darwin cooperative child readiness failed: {error}");
        return ExitCode::from(75);
    }
    // Remain live through the mandatory post-TERM census, then cooperate.
    // This proves the service tracks a known child without manufacturing an
    // ambiguous Darwin census in the positive completion case.
    match _signals.wait() {
        Ok(nix::sys::signal::Signal::SIGTERM) => ExitCode::SUCCESS,
        Ok(signal) => {
            eprintln!("hosted fixture: Darwin cooperative child received unexpected {signal:?}");
            ExitCode::from(75)
        }
        Err(error) => {
            eprintln!("hosted fixture: Darwin cooperative child wait failed: {error}");
            ExitCode::from(75)
        }
    }
}

#[cfg(target_os = "macos")]
fn run_darwin_late_spawner(ready: PathBuf, late_child: PathBuf) -> ExitCode {
    if let Err(error) = acknowledge_darwin_fixture_child_admission() {
        eprintln!("hosted fixture: Darwin late-spawner admission failed: {error}");
        return ExitCode::from(75);
    }
    let signals = block_sigterm();
    if let Err(error) = std::fs::write(ready, std::process::id().to_string()) {
        eprintln!("hosted fixture: Darwin late-spawner readiness failed: {error}");
        return ExitCode::from(75);
    }
    match signals.wait() {
        Ok(nix::sys::signal::Signal::SIGTERM) => {
            let executable = match std::env::current_exe() {
                Ok(executable) => executable,
                Err(error) => {
                    eprintln!("hosted fixture: Darwin late-spawner executable failed: {error}");
                    return ExitCode::from(75);
                }
            };
            if let Err(error) = Command::new(executable)
                .arg("darwin-term-resistant")
                .arg(&late_child)
                .env_remove("SOL_PARENT_LOSS_GENERATION")
                .env_remove("SOL_PARENT_LOSS_LAUNCH_ID")
                .env_remove("SOL_PARENT_LOSS_PARENT_LAUNCH_ID")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                eprintln!("hosted fixture: Darwin late descendant failed: {error}");
                return ExitCode::from(75);
            }
            if let Err(error) = wait_for_fixture_paths(&[late_child]) {
                eprintln!("hosted fixture: Darwin late descendant readiness failed: {error}");
                return ExitCode::from(75);
            }
            // The unadmitted grandchild exists before Cortex crashes. This
            // gives the coordinator a real reparented process plus a missing
            // Cortex witness: it must refuse the sealed generation at its
            // bounded deadline rather than mistake PGID membership for proof.
            let cortex = nix::unistd::getppid();
            if cortex.as_raw() <= 1 {
                eprintln!("hosted fixture: Darwin late-spawner has no Cortex parent");
                return ExitCode::from(75);
            }
            if let Err(error) = nix::sys::signal::kill(cortex, nix::sys::signal::Signal::SIGKILL) {
                eprintln!("hosted fixture: Darwin Cortex crash failed: {error}");
                return ExitCode::from(75);
            }
            std::thread::sleep(std::time::Duration::from_secs(30));
            ExitCode::SUCCESS
        }
        Ok(signal) => {
            eprintln!("hosted fixture: Darwin late-spawner received unexpected {signal:?}");
            ExitCode::from(75)
        }
        Err(error) => {
            eprintln!("hosted fixture: Darwin late-spawner wait failed: {error}");
            ExitCode::from(75)
        }
    }
}

#[cfg(target_os = "macos")]
fn acknowledge_darwin_fixture_child_admission() -> Result<(), String> {
    let Some(journal) = std::env::var_os("SOLSTONE_JOURNAL").map(PathBuf::from) else {
        return Ok(());
    };
    acknowledge_hosted_child_admission(&journal).map_err(|error| error.to_string())
}

#[cfg(target_os = "macos")]
fn run_darwin_lookalike(port: u16, ready: PathBuf) -> ExitCode {
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("hosted fixture: Darwin lookalike listener failed: {error}");
            return ExitCode::from(75);
        }
    };
    if let Err(error) = std::fs::write(ready, std::process::id().to_string()) {
        eprintln!("hosted fixture: Darwin lookalike readiness failed: {error}");
        return ExitCode::from(75);
    }
    let _listener = listener;
    std::thread::sleep(std::time::Duration::from_secs(30));
    ExitCode::SUCCESS
}

#[cfg(target_os = "macos")]
fn run_darwin_control(ready: PathBuf) -> ExitCode {
    if let Err(error) = std::fs::write(ready, std::process::id().to_string()) {
        eprintln!("hosted fixture: Darwin control readiness failed: {error}");
        return ExitCode::from(75);
    }
    std::thread::sleep(std::time::Duration::from_secs(30));
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let mut args = std::env::args_os();
    let _program = args.next();
    let Some(mode) = args.next() else {
        return ExitCode::from(64);
    };
    match mode.to_string_lossy().as_ref() {
        "__parent-loss-coordinator" => run_parent_loss_coordinator(args),
        "host" => {
            let Some(journal) = args.next().map(PathBuf::from) else {
                return ExitCode::from(64);
            };
            let Some(outcome) = args.next().map(PathBuf::from) else {
                return ExitCode::from(64);
            };
            let Some(nonce) = args.next().and_then(|value| value.into_string().ok()) else {
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
            run_hosted_fixture(journal, outcome, nonce, parent)
        }
        "host-with-parent" => {
            let Some(journal) = args.next().map(PathBuf::from) else {
                return ExitCode::from(64);
            };
            let Some(outcome) = args.next().map(PathBuf::from) else {
                return ExitCode::from(64);
            };
            let Some(nonce) = args.next().and_then(|value| value.into_string().ok()) else {
                return ExitCode::from(64);
            };
            let Some(pid) = args
                .next()
                .and_then(|value| value.into_string().ok())
                .and_then(|value| value.parse::<u32>().ok())
            else {
                return ExitCode::from(64);
            };
            let Some(start_ticks) = args
                .next()
                .and_then(|value| value.into_string().ok())
                .and_then(|value| value.parse::<u64>().ok())
            else {
                return ExitCode::from(64);
            };
            if args.next().is_some() {
                return ExitCode::from(64);
            }
            run_hosted_fixture(
                journal,
                outcome,
                nonce,
                DeclaredParent::from_instance(ProcessInstance {
                    pid,
                    birth: ProcessBirth::linux(start_ticks, 1, 100),
                }),
            )
        }
        "launcher" => {
            let Some(journal) = args.next().map(PathBuf::from) else {
                return ExitCode::from(64);
            };
            let Some(child_pid) = args.next().map(PathBuf::from) else {
                return ExitCode::from(64);
            };
            let Some(outcome) = args.next().map(PathBuf::from) else {
                return ExitCode::from(64);
            };
            let Some(nonce) = args.next().and_then(|value| value.into_string().ok()) else {
                return ExitCode::from(64);
            };
            if args.next().is_some() {
                return ExitCode::from(64);
            }
            run_launcher(journal, child_pid, outcome, nonce)
        }
        "ready-sleep" => {
            let Some(marker) = args.next().map(PathBuf::from) else {
                return ExitCode::from(64);
            };
            let Some(port) = args
                .next()
                .and_then(|value| value.into_string().ok())
                .and_then(|value| value.parse::<u16>().ok())
            else {
                return ExitCode::from(64);
            };
            if args.next().is_some() {
                return ExitCode::from(64);
            }
            #[cfg(target_os = "macos")]
            if darwin_parent_lifetime_fixture_enabled() {
                return run_darwin_hosted_service(HostedServiceKind::Convey, marker, Some(port));
            }
            let _ = (marker, port);
            ExitCode::from(64)
        }
        "ready-park" => {
            let Some(marker) = args.next().map(PathBuf::from) else {
                return ExitCode::from(64);
            };
            if args.next().is_some() {
                return ExitCode::from(64);
            }
            #[cfg(target_os = "macos")]
            if darwin_parent_lifetime_fixture_enabled() {
                let kind = match marker.file_name().and_then(|name| name.to_str()) {
                    Some("fixture-sense.marker") => HostedServiceKind::Sense,
                    Some("fixture-cortex.marker") => HostedServiceKind::Cortex,
                    Some("fixture-spl.marker") => HostedServiceKind::Spl,
                    _ => return ExitCode::from(64),
                };
                return run_darwin_hosted_service(kind, marker, None);
            }
            let _ = marker;
            ExitCode::from(64)
        }
        "darwin-term-resistant" => {
            let Some(ready) = args.next().map(PathBuf::from) else {
                return ExitCode::from(64);
            };
            if args.next().is_some() {
                return ExitCode::from(64);
            }
            #[cfg(target_os = "macos")]
            return run_darwin_term_resistant(ready);
            #[cfg(not(target_os = "macos"))]
            {
                let _ = ready;
                ExitCode::from(64)
            }
        }
        "darwin-cooperative-child" => {
            let Some(ready) = args.next().map(PathBuf::from) else {
                return ExitCode::from(64);
            };
            if args.next().is_some() {
                return ExitCode::from(64);
            }
            #[cfg(target_os = "macos")]
            return run_darwin_cooperative_child(ready);
            #[cfg(not(target_os = "macos"))]
            {
                let _ = ready;
                ExitCode::from(64)
            }
        }
        "darwin-late-spawner" => {
            let Some(ready) = args.next().map(PathBuf::from) else {
                return ExitCode::from(64);
            };
            let Some(late_child) = args.next().map(PathBuf::from) else {
                return ExitCode::from(64);
            };
            if args.next().is_some() {
                return ExitCode::from(64);
            }
            #[cfg(target_os = "macos")]
            return run_darwin_late_spawner(ready, late_child);
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (ready, late_child);
                ExitCode::from(64)
            }
        }
        "darwin-lookalike" => {
            let Some(port) = args
                .next()
                .and_then(|value| value.into_string().ok())
                .and_then(|value| value.parse::<u16>().ok())
            else {
                return ExitCode::from(64);
            };
            let Some(ready) = args.next().map(PathBuf::from) else {
                return ExitCode::from(64);
            };
            if args.next().is_some() {
                return ExitCode::from(64);
            }
            #[cfg(target_os = "macos")]
            return run_darwin_lookalike(port, ready);
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (port, ready);
                ExitCode::from(64)
            }
        }
        "darwin-control" => {
            let Some(ready) = args.next().map(PathBuf::from) else {
                return ExitCode::from(64);
            };
            if args.next().is_some() {
                return ExitCode::from(64);
            }
            #[cfg(target_os = "macos")]
            return run_darwin_control(ready);
            #[cfg(not(target_os = "macos"))]
            {
                let _ = ready;
                ExitCode::from(64)
            }
        }
        _ => ExitCode::from(64),
    }
}
