// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows Task Scheduler service management.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use solstone_core_cli::{ServiceAction, ServiceInstallationGuardArguments};
use solstone_core_installation_identity::{
    GuardFields, OwnerBase, journal_token_from_path, load_installation_binding, owner_base,
    parse_service_guard_environment, root_token_from_path,
};
use solstone_core_journal::resolve_identity_root_from_executable_dir;
use solstone_core_service_unit::{
    WindowsServiceAction, WindowsTaskDefinition, WindowsTaskInput, encode_windows_task_xml,
    parse_windows_task_xml, render_windows_task_xml,
};
use solstone_core_system::lifecycle::wait_ready;
mod native_process;
mod task_scheduler;
use task_scheduler::{Operation, Snapshot, TaskInstance};

use crate::resolve_process_journal_path;

const READY_TIMEOUT: Duration = Duration::from_secs(120);
const STOP_TIMEOUT: Duration = Duration::from_secs(40);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

pub(crate) fn run(action: ServiceAction) -> ExitCode {
    // Explicit service entry may settle this process's failed independent launches;
    // unrelated pending work does not prevent an OS-manager control operation.
    let _ = solstone_core_system::process::retry_windows_launch_cleanup_until(
        Instant::now() + solstone_core_system::process::DRAIN_JOIN_TIMEOUT,
    );
    match action {
        ServiceAction::Install {
            port,
            installation_guard,
        } => run_install_action(port.canonical_decimal(), installation_guard),
        ServiceAction::Uninstall => run_uninstall_action(),
        ServiceAction::Start => run_start_action(),
        ServiceAction::Stop => run_stop_action(),
        ServiceAction::Restart { if_installed } => run_restart_action(if_installed),
        ServiceAction::Status => run_status(),
        ServiceAction::Up => run_up(),
        ServiceAction::Down => run_down(),
        ServiceAction::Logs { .. } => unreachable!("logs handled by service_logs"),
    }
}

struct ServiceContext {
    owner: OwnerBase,
    journal: PathBuf,
    sid: String,
    guard: GuardFields,
    task_path: String,
    public_journal_exe: PathBuf,
}

fn resolve_context() -> Result<ServiceContext, ExitCode> {
    resolve_context_at(None)
}

fn resolve_context_at(selected: Option<&Path>) -> Result<ServiceContext, ExitCode> {
    let journal = match selected {
        Some(path) => path.to_path_buf(),
        None => match resolve_process_journal_path() {
            Ok(line) => line.path,
            Err(error) => {
                crate::eprint_journal_path_error(error);
                return Err(ExitCode::from(1));
            }
        },
    };
    let owner = match owner_base() {
        Ok(owner) => owner,
        Err(error) => {
            eprintln!("could not locate owner base: {error}");
            return Err(ExitCode::from(1));
        }
    };
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("could not inspect current executable: {error}");
            return Err(ExitCode::from(1));
        }
    };
    let exe_dir = exe.parent().unwrap_or_else(|| Path::new("."));
    let root = match resolve_identity_root_from_executable_dir(exe_dir) {
        Some(root) => root,
        None => {
            eprintln!("could not resolve identity root from executable directory");
            return Err(ExitCode::from(1));
        }
    };
    let root_token = match root_token_from_path(&root) {
        Ok(token) => token,
        Err(error) => {
            eprintln!("could not resolve root token: {error}");
            return Err(ExitCode::from(1));
        }
    };
    let binding = match load_installation_binding(&owner, &root_token) {
        Ok(binding) => binding,
        Err(error) => {
            eprintln!("could not load installation binding: {error}");
            return Err(ExitCode::from(1));
        }
    };

    if journal_token_from_path(&journal).ok().as_ref() != Some(&binding.journal_token) {
        eprintln!("selected journal differs from the saved installation binding");
        return Err(ExitCode::from(1));
    }
    let sid = match solstone_core_callosum::windows::sid::current_user_sid() {
        Ok(sid) => sid,
        Err(error) => {
            eprintln!("could not retrieve user SID: {error}");
            return Err(ExitCode::from(1));
        }
    };
    let installation_id = binding.id.as_hex();
    let task_path = format!(r"\solstone-{sid}\{installation_id}");
    let public_journal_exe = exe_dir.join("journal.exe");
    if !public_journal_exe.is_file() {
        eprintln!("the installed journal.exe facade is unavailable");
        return Err(ExitCode::from(1));
    }

    Ok(ServiceContext {
        owner,
        journal,
        sid,
        guard: GuardFields::from_binding(&binding),
        task_path,
        public_journal_exe,
    })
}

fn task_error(error: impl std::fmt::Display) -> ExitCode {
    eprintln!("journal: {error}");
    ExitCode::from(1)
}

fn inspect_task(ctx: &ServiceContext, deadline: Instant) -> Result<Snapshot, ExitCode> {
    task_scheduler::execute_until(
        &ctx.sid,
        &ctx.guard.id.as_hex(),
        Operation::Inspect,
        deadline,
    )
    .map_err(task_error)
}

fn validate_task(
    ctx: &ServiceContext,
    snapshot: &Snapshot,
) -> Result<WindowsTaskDefinition, ExitCode> {
    let xml = snapshot
        .validation_xml
        .as_deref()
        .filter(|_| snapshot.present)
        .ok_or_else(|| task_error("service task is not installed"))?;
    let definition = parse_windows_task_xml(xml).map_err(task_error)?;
    if definition.principal_sid != ctx.sid
        || definition.command
            != ctx
                .public_journal_exe
                .to_str()
                .ok_or_else(|| task_error("installed command contains invalid text"))?
        || definition.working_directory
            != ctx
                .journal
                .to_str()
                .ok_or_else(|| task_error("journal path contains invalid text"))?
        || definition.action.journal != definition.working_directory
        || definition.action.guard != ctx.guard
    {
        return Err(task_error(
            "service task differs from the current installation binding",
        ));
    }
    Ok(definition)
}

fn install_task(ctx: &ServiceContext, port: u16) -> Result<(), ExitCode> {
    let deadline = Instant::now() + STOP_TIMEOUT;
    let before = inspect_task(ctx, deadline)?;
    if before.present {
        validate_task(ctx, &before)?;
    }
    let journal_display = ctx
        .journal
        .to_str()
        .ok_or_else(|| task_error("journal path contains invalid text"))?;
    let command = ctx
        .public_journal_exe
        .to_str()
        .ok_or_else(|| task_error("installed command contains invalid text"))?;
    let action = WindowsServiceAction {
        port,
        journal: journal_display.to_owned(),
        guard: ctx.guard.clone(),
    };
    let xml = render_windows_task_xml(&WindowsTaskInput {
        principal_sid: &ctx.sid,
        command,
        action: &action,
        working_directory: journal_display,
    })
    .map_err(task_error)?;
    let operation = if before.present {
        Operation::Update {
            before: &before,
            xml: &xml,
        }
    } else {
        Operation::Create { xml: &xml }
    };
    let after =
        task_scheduler::execute_until(&ctx.sid, &ctx.guard.id.as_hex(), operation, deadline)
            .map_err(task_error)?;
    let installed = validate_task(ctx, &after)?;
    if installed.action != action {
        return Err(task_error(
            "installed task action did not match its readback",
        ));
    }
    // Preserve the setup artifact only after actual scheduler readback succeeds.
    // It is never authorization to replace/delete the registered task.
    let provider = ctx.owner.path();
    let directory = provider
        .ancestors()
        .nth(2)
        .ok_or_else(|| task_error("installation provider location is unavailable"))?
        .join("journal-service");
    fs::create_dir_all(&directory).map_err(task_error)?;
    fs::write(
        directory.join(format!("{}.xml", ctx.guard.namespace)),
        encode_windows_task_xml(
            after
                .xml
                .as_deref()
                .ok_or_else(|| task_error("task XML readback missing"))?,
        )
        .map_err(task_error)?,
    )
    .map_err(task_error)?;
    Ok(())
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadyForwarder {
    instance: solstone_core_system::process::ProcessInstance,
    launch_id: String,
}

struct RetainedTaskRun {
    selected: TaskInstance,
    supervisor_instance: solstone_core_system::process::ProcessInstance,
    supervisor: native_process::RetainedProcess,
    forwarder: native_process::RetainedProcess,
}

fn same_task_profile(left: &Snapshot, right: &Snapshot) -> bool {
    left.present
        && right.present
        && left.xml == right.xml
        && left.task_sddl == right.task_sddl
        && left.folder_sddl == right.folder_sddl
}

fn retain_task_run(
    ctx: &ServiceContext,
    before: &Snapshot,
    selected_guid: &str,
    deadline: Instant,
) -> Result<RetainedTaskRun, ExitCode> {
    let marker = wait_ready(
        &ctx.journal,
        deadline.saturating_duration_since(Instant::now()),
        POLL_INTERVAL,
    )
    .ok_or_else(|| task_error("service startup or abnormal shutdown has no verified run target"))?;
    let supervisor_instance: solstone_core_system::process::ProcessInstance =
        serde_json::from_value(
            marker
                .extra
                .get("windows_process_instance")
                .cloned()
                .ok_or_else(|| task_error("service readiness has no exact native identity"))?,
        )
        .map_err(task_error)?;
    let current_instance: solstone_core_system::process::ProcessInstance = serde_json::from_slice(
        &fs::read(ctx.journal.join("health/supervisor.process_instance")).map_err(task_error)?,
    )
    .map_err(task_error)?;
    if marker.pid != supervisor_instance.pid || current_instance != supervisor_instance {
        return Err(task_error("service readiness identity changed"));
    }
    let root: ReadyForwarder = serde_json::from_value(
        marker
            .extra
            .get("windows_task_forwarder")
            .cloned()
            .ok_or_else(|| task_error("service readiness has no installed forwarder admission"))?,
    )
    .map_err(task_error)?;
    if root.launch_id.len() != 48
        || !root.launch_id.bytes().all(|byte| byte.is_ascii_hexdigit())
        || root.instance.pid == supervisor_instance.pid
    {
        return Err(task_error("service forwarder admission is invalid"));
    }
    let current = inspect_task(ctx, deadline)?;
    validate_task(ctx, &current)?;
    if !same_task_profile(before, &current) || current.instances.len() != 1 {
        return Err(task_error("service task changed during run admission"));
    }
    let selected = current.instances[0].clone();
    if selected.guid != selected_guid
        || selected.engine_pid != root.instance.pid
        || selected.current_action != "journal-supervisor"
    {
        return Err(task_error(
            "service scheduler instance does not match its admitted forwarder",
        ));
    }
    let supervisor =
        native_process::RetainedProcess::open(supervisor_instance).map_err(task_error)?;
    let forwarder = native_process::RetainedProcess::open(root.instance).map_err(task_error)?;
    if supervisor.exit_code().map_err(task_error)?.is_some()
        || forwarder.exit_code().map_err(task_error)?.is_some()
    {
        return Err(task_error("service run exited during admission"));
    }
    let after = inspect_task(ctx, deadline)?;
    validate_task(ctx, &after)?;
    if !same_task_profile(&current, &after) || after.instances != [selected.clone()] {
        return Err(task_error(
            "service task instance changed while retaining native handles",
        ));
    }
    Ok(RetainedTaskRun {
        selected,
        supervisor_instance,
        supervisor,
        forwarder,
    })
}

fn start_task(ctx: &ServiceContext) -> Result<(), ExitCode> {
    let deadline = Instant::now() + READY_TIMEOUT;
    let before = inspect_task(ctx, deadline)?;
    validate_task(ctx, &before)?;
    let selected_guid = match before.instances.as_slice() {
        [] => {
            let started = task_scheduler::execute_until(
                &ctx.sid,
                &ctx.guard.id.as_hex(),
                Operation::Run { before: &before },
                deadline,
            )
            .map_err(task_error)?;
            validate_task(ctx, &started)?;
            if !same_task_profile(&before, &started) {
                return Err(task_error("service task changed during start"));
            }
            started
                .run_instance
                .ok_or_else(|| task_error("scheduler returned no task run instance"))?
                .guid
        }
        [instance] => instance.guid.clone(),
        _ => return Err(task_error("cannot identify one running service task")),
    };
    let _run = retain_task_run(ctx, &before, &selected_guid, deadline)?;
    println!("your journal is ready");
    Ok(())
}

fn stop_task(ctx: &ServiceContext) -> Result<(), ExitCode> {
    let deadline = Instant::now() + STOP_TIMEOUT;
    let before = inspect_task(ctx, deadline)?;
    if !before.present {
        return Err(task_error(
            "service task is absent and its forwarder cleanup cannot be verified",
        ));
    }
    validate_task(ctx, &before)?;
    let selected = match before.instances.as_slice() {
        [instance] => instance,
        [] => {
            return Err(task_error(
                "scheduler is idle and its forwarder cleanup cannot be verified",
            ));
        }
        _ => return Err(task_error("cannot identify one running service task")),
    };
    let run = retain_task_run(ctx, &before, &selected.guid, deadline)?;
    let frame = serde_json::json!({"tract":"supervisor", "event":"service_stop", "target":run.supervisor_instance,
        "guard":solstone_core_installation_identity::service_guard_environment(&ctx.guard)});
    let mut line = serde_json::to_string(&frame).map_err(task_error)?;
    line.push('\n');
    solstone_core_callosum::CallosumOneShotSender::new(
        ctx.journal.join("health/callosum.sock"),
        deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_secs(3)),
    )
    .send_line(&line)
    .map_err(task_error)?;
    loop {
        if Instant::now() >= deadline {
            return Err(task_error(
                "service stop deadline elapsed; cleanup remains unverified",
            ));
        }
        let supervisor_exit = run.supervisor.exit_code().map_err(task_error)?;
        let forwarder_exit = run.forwarder.exit_code().map_err(task_error)?;
        if supervisor_exit.is_some_and(|code| code != 0)
            || forwarder_exit.is_some_and(|code| code != 0)
        {
            return Err(task_error(
                "service exited abnormally; cleanup remains unverified",
            ));
        }
        if supervisor_exit == Some(0) && forwarder_exit == Some(0) {
            let after = inspect_task(ctx, deadline)?;
            validate_task(ctx, &after)?;
            if !same_task_profile(&before, &after) {
                return Err(task_error("service task changed during stop"));
            }
            if after.instances.is_empty() && after.state == Some(3) {
                println!("background service stopped");
                return Ok(());
            }
            if after.instances.len() != 1 || after.instances[0].guid != run.selected.guid {
                return Err(task_error("service task instance changed during stop"));
            }
        }
        std::thread::sleep(POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())));
    }
}

fn delete_task(ctx: &ServiceContext) -> Result<(), ExitCode> {
    let deadline = Instant::now() + STOP_TIMEOUT;
    let before = inspect_task(ctx, deadline)?;
    if !before.present {
        return Ok(());
    }
    validate_task(ctx, &before)?;
    let after = task_scheduler::execute_until(
        &ctx.sid,
        &ctx.guard.id.as_hex(),
        Operation::Delete { before: &before },
        deadline,
    )
    .map_err(task_error)?;
    if after.present {
        return Err(task_error("service task deletion was not observed"));
    }
    Ok(())
}

fn run_install_action(port: &str, supplied: Option<ServiceInstallationGuardArguments>) -> ExitCode {
    let ctx = match resolve_context() {
        Ok(ctx) => ctx,
        Err(code) => return code,
    };
    if let Some(supplied) = supplied {
        let environment = BTreeMap::from([
            (
                "SOLSTONE_INSTALLATION_NAMESPACE".to_owned(),
                supplied.namespace,
            ),
            ("SOLSTONE_INSTALLATION_ID".to_owned(), supplied.id),
            (
                "SOLSTONE_INSTALLATION_GENERATION".to_owned(),
                supplied.generation,
            ),
            (
                "SOLSTONE_INSTALLATION_JOURNAL_TOKEN".to_owned(),
                supplied.journal_token,
            ),
        ]);
        if parse_service_guard_environment(&environment)
            .ok()
            .flatten()
            .as_ref()
            != Some(&ctx.guard)
        {
            eprintln!("service installation guard differs from the saved binding");
            return ExitCode::from(1);
        }
    }
    let Ok(port) = port.parse::<u16>() else {
        return ExitCode::from(1);
    };
    match install_task(&ctx, port) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

fn run_uninstall_action() -> ExitCode {
    let ctx = match resolve_context() {
        Ok(ctx) => ctx,
        Err(code) => return code,
    };
    if let Err(code) = stop_task(&ctx) {
        return code;
    }
    match delete_task(&ctx) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

fn run_start_action() -> ExitCode {
    let ctx = match resolve_context() {
        Ok(ctx) => ctx,
        Err(code) => return code,
    };
    match start_task(&ctx) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

fn run_stop_action() -> ExitCode {
    let ctx = match resolve_context() {
        Ok(ctx) => ctx,
        Err(code) => return code,
    };
    match stop_task(&ctx) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

fn run_restart_action(if_installed: bool) -> ExitCode {
    let ctx = match resolve_context() {
        Ok(ctx) => ctx,
        Err(code) => return code,
    };
    let installed = match inspect_task(&ctx, Instant::now() + STOP_TIMEOUT) {
        Ok(snapshot) => snapshot.present,
        Err(code) => return code,
    };
    if !installed {
        if if_installed {
            return ExitCode::SUCCESS;
        } else {
            eprintln!("service task is not installed");
            return ExitCode::from(1);
        }
    }
    if let Err(code) = stop_task(&ctx) {
        return code;
    }
    match start_task(&ctx) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

fn run_up() -> ExitCode {
    let ctx = match resolve_context() {
        Ok(ctx) => ctx,
        Err(code) => return code,
    };
    let before = match inspect_task(&ctx, Instant::now() + STOP_TIMEOUT) {
        Ok(snapshot) => snapshot,
        Err(code) => return code,
    };
    if before.present {
        if let Err(code) = validate_task(&ctx, &before) {
            return code;
        }
    } else if let Err(code) = install_task(&ctx, 5015) {
        return code;
    }
    match start_task(&ctx) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

fn run_down() -> ExitCode {
    let ctx = match resolve_context() {
        Ok(ctx) => ctx,
        Err(code) => return code,
    };
    match stop_task(&ctx) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

fn run_status() -> ExitCode {
    let ctx = match resolve_context() {
        Ok(ctx) => ctx,
        Err(code) => return code,
    };
    let snapshot = match inspect_task(&ctx, Instant::now() + STOP_TIMEOUT) {
        Ok(snapshot) => snapshot,
        Err(code) => return code,
    };
    if !snapshot.present {
        println!("background service is not installed");
        return ExitCode::from(3);
    }
    if let Err(code) = validate_task(&ctx, &snapshot) {
        return code;
    }
    let ready = solstone_core_system::lifecycle::readiness_is_valid(&ctx.journal);
    println!("Task: {}", ctx.task_path);
    println!(
        "Supervisor readiness: {}",
        if ready { "ready" } else { "not ready" }
    );
    ExitCode::SUCCESS
}

/// Recognize the exact installed action before logger/runtime initialization.
/// A task guard never authorizes a speakers-generation borrow.
pub(crate) struct AdmittedTaskAction {
    pub(crate) arguments: Vec<OsString>,
    pub(crate) journal: PathBuf,
    pub(crate) launch: solstone_core_system::process::InstalledTaskLaunchRequest,
}

pub(crate) fn admit_task_action(args: &[OsString]) -> Result<Option<AdmittedTaskAction>, String> {
    let declared_task = args.iter().any(|arg| arg == "--windows-service");
    let supervisor_guard = args.first().is_some_and(|arg| arg == "supervisor")
        && args
            .iter()
            .any(|arg| arg.to_string_lossy().starts_with("--installation-"));
    if !declared_task && !supervisor_guard {
        return Ok(None);
    }
    let arguments = args
        .iter()
        .map(|arg| {
            arg.to_str()
                .map(str::to_owned)
                .ok_or_else(|| "windows service action contains non-Unicode text".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let action = WindowsServiceAction::parse(&arguments).map_err(str::to_owned)?;
    let context = resolve_context_at(Some(Path::new(&action.journal)))
        .map_err(|_| "windows service binding unavailable".to_owned())?;
    if action.guard != context.guard {
        return Err("windows service action differs from the complete saved binding".to_owned());
    }
    Ok(Some(AdmittedTaskAction {
        arguments: arguments[..4].iter().map(OsString::from).collect(),
        journal: context.journal.clone(),
        launch: solstone_core_system::process::InstalledTaskLaunchRequest {
            journal: context.journal,
            guard: context.guard,
            arguments,
            acknowledgement_timeout: Duration::from_secs(3),
        },
    }))
}
