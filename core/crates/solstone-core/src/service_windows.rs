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
    ArtifactBindingEvidence, CleanUninstallPlan, CleanUninstallRequest, CleanupSkip,
    CleanupTargetDecision, CleanupTargetKind, GuardFields, IdentityError, OwnerBase, PlatformTag,
    ProtectedJournals, admit_clean_uninstall, journal_token_from_path, load_installation_binding,
    may_remove_cleanup_target, owner_base, parse_service_guard_environment, root_token_from_path,
};
use solstone_core_journal::resolve_identity_root_from_executable_dir;
use solstone_core_service_unit::{
    WindowsServiceAction, WindowsTaskDefinition, WindowsTaskInput, WindowsTaskProfile,
    encode_windows_task_xml, parse_windows_task_xml, render_windows_task_xml,
    windows_service_update_plan,
};
use solstone_core_setup::clean_uninstall::{
    CleanUninstallMark, CleanUninstallPreflight, CleanUninstallPreflightState, CleanUninstallState,
    CleanUninstallStepResult, clean_uninstall_preflight,
};
use solstone_core_setup::user_config::config_path;
use solstone_core_system::lifecycle::wait_ready;
mod native_process;
mod sign_in_resume;
mod task_scheduler;
use task_scheduler::{Operation, Snapshot, TaskInstance};

use crate::{discover_binary_home, resolve_process_journal_path};

const READY_TIMEOUT: Duration = Duration::from_secs(120);
/// A public stop waits out the supervisor's own worst-case standard shutdown
/// (all hosted children sharing one stop budget), then the forwarder's Job
/// drain and a Scheduler readback, before it may call cleanup unverified. The
/// former fixed 40 s sat below that ceiling, so a clean shutdown that was
/// still in progress was reported as a failed stop.
const STOP_TIMEOUT: Duration = Duration::from_secs(
    solstone_core_system::lifecycle::standard_shutdown_ceiling().as_secs() + 15,
);
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
        } => run_install_action(
            port.as_ref()
                .map(|port| port.canonical_decimal().to_owned()),
            installation_guard,
        ),
        ServiceAction::Uninstall => run_uninstall_action(),
        ServiceAction::Start => run_start_action(),
        ServiceAction::Stop => run_stop_action(),
        ServiceAction::Restart { if_installed } => run_restart_action(if_installed),
        ServiceAction::Status => run_status(),
        ServiceAction::Up => run_up(),
        ServiceAction::Down => run_down(),
        ServiceAction::ResumeAfterUpdate => run_resume_after_update(),
        ServiceAction::BeforeUninstall => run_before_uninstall(),
        ServiceAction::AppStatus => run_app_status(),
        ServiceAction::SignIn { on } => run_sign_in(on),
        ServiceAction::Logs { .. } => unreachable!("logs handled by service_logs"),
    }
}

struct ServiceContext {
    owner: OwnerBase,
    journal: PathBuf,
    sid: String,
    guard: GuardFields,
    task_path: String,
    public_solstone_exe: PathBuf,
    /// This command's Task Scheduler worker; it ends with the context.
    scheduler: task_scheduler::ControlSession,
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
    context_for_journal(journal).map_err(|error| {
        eprintln!("{error}");
        ExitCode::from(1)
    })
}

/// The service context for `journal`, or the one-line reason there is none.
fn context_for_journal(journal: PathBuf) -> Result<ServiceContext, String> {
    let owner = owner_base().map_err(|error| format!("could not locate owner base: {error}"))?;
    let exe = std::env::current_exe()
        .map_err(|error| format!("could not inspect current executable: {error}"))?;
    let exe_dir = exe.parent().unwrap_or_else(|| Path::new("."));
    let root = resolve_identity_root_from_executable_dir(exe_dir)
        .ok_or_else(|| "could not resolve identity root from executable directory".to_owned())?;
    let root_token = root_token_from_path(&root)
        .map_err(|error| format!("could not resolve root token: {error}"))?;
    let binding = load_installation_binding(&owner, &root_token)
        .map_err(|error| format!("could not load installation binding: {error}"))?;

    if journal_token_from_path(&journal).ok().as_ref() != Some(&binding.journal_token) {
        return Err("selected journal differs from the saved installation binding".to_owned());
    }
    let sid = solstone_core_callosum::windows::sid::current_user_sid()
        .map_err(|error| format!("could not retrieve user SID: {error}"))?;
    let installation_id = binding.id.as_hex();
    let task_path = format!(r"\solstone-{sid}\{installation_id}");
    let public_solstone_exe = exe_dir.join("solstone.exe");
    if !public_solstone_exe.is_file() {
        return Err("the installed solstone.exe facade is unavailable".to_owned());
    }

    Ok(ServiceContext {
        owner,
        journal,
        sid,
        guard: GuardFields::from_binding(&binding),
        task_path,
        public_solstone_exe,
        scheduler: task_scheduler::ControlSession::default(),
    })
}

/// How long `journal doctor` waits on the Task Scheduler for one readback.
const DOCTOR_INSPECT_TIMEOUT: Duration = Duration::from_secs(10);

/// `journal doctor`'s reading of this installation's service registration.
///
/// The doctor asks the same question of every platform -- is a service
/// registered, and does it run this install? -- and on Windows only this
/// command's Task Scheduler client can answer it. Nothing is printed: every
/// failure comes back as the reason the registration could not be read.
pub(crate) fn doctor_registration(
    context: &solstone_core_doctor::context::CheckContext,
) -> solstone_core_doctor::context::WindowsServiceRegistration {
    use solstone_core_doctor::context::WindowsServiceRegistration;

    let ctx = match context_for_journal(context.journal_path.clone()) {
        Ok(ctx) => ctx,
        Err(reason) => return WindowsServiceRegistration::Unreadable(reason),
    };
    let snapshot = match task_scheduler::execute_until(
        &ctx.scheduler,
        &ctx.sid,
        &ctx.guard.id.as_hex(),
        Operation::Inspect,
        Instant::now() + DOCTOR_INSPECT_TIMEOUT,
    ) {
        Ok(snapshot) => snapshot,
        Err(error) => return WindowsServiceRegistration::Unreadable(error.to_string()),
    };
    if !snapshot.present {
        return WindowsServiceRegistration::Absent;
    }
    let Some(xml) = snapshot.validation_xml.as_deref() else {
        return WindowsServiceRegistration::Unreadable(
            "the registered task has no readable definition".to_owned(),
        );
    };
    let definition = match parse_windows_task_xml(xml) {
        Ok(definition) => definition,
        Err(error) => return WindowsServiceRegistration::Unreadable(error.to_string()),
    };
    let expected = ctx.public_solstone_exe.display().to_string();
    let legacy_expected = ctx
        .public_solstone_exe
        .with_file_name("journal.exe")
        .display()
        .to_string();
    let journal = ctx.journal.display().to_string();
    let mismatch = if definition.command != expected && definition.command != legacy_expected {
        Some(format!(
            "{} is registered, expected {expected}",
            definition.command
        ))
    } else if definition.principal_sid != ctx.sid {
        Some("it is registered to another account".to_owned())
    } else if definition.working_directory != journal || definition.action.journal != journal {
        Some(format!(
            "it runs the journal at {}, expected {journal}",
            definition.action.journal
        ))
    } else if definition.action.guard != ctx.guard {
        Some("it belongs to a different installation of the journal".to_owned())
    } else {
        None
    };
    WindowsServiceRegistration::Present {
        command: definition.command,
        mismatch,
    }
}

fn task_error(error: impl std::fmt::Display) -> ExitCode {
    eprintln!("journal: {error}");
    ExitCode::from(1)
}

fn inspect_task(ctx: &ServiceContext, deadline: Instant) -> Result<Snapshot, ExitCode> {
    task_scheduler::execute_until(
        &ctx.scheduler,
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
    let command_matches = definition.command
        == ctx
            .public_solstone_exe
            .to_str()
            .ok_or_else(|| task_error("installed command contains invalid text"))?
        || definition.command
            == ctx
                .public_solstone_exe
                .with_file_name("journal.exe")
                .to_str()
                .ok_or_else(|| task_error("installed command contains invalid text"))?;
    if definition.principal_sid != ctx.sid
        || !command_matches
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

/// The port a first installation takes when nothing names one.
const DEFAULT_SERVICE_PORT: u16 = 5015;

fn install_task(ctx: &ServiceContext, requested_port: Option<u16>) -> Result<(), ExitCode> {
    let deadline = Instant::now() + STOP_TIMEOUT;
    let before = inspect_task(ctx, deadline)?;
    let registered = if before.present {
        Some(validate_task(ctx, &before)?)
    } else {
        None
    };
    // An explicit `--port` wins. Without one, keep the port the registration
    // already carries: re-registering an established journal on the 5015
    // default moved it off its own port and exited 0 -- measured here as
    // `"supervisor" "6123"` becoming `"supervisor" "5015"`.
    let port = requested_port.unwrap_or_else(|| {
        registered
            .as_ref()
            .map_or(DEFAULT_SERVICE_PORT, |definition| definition.action.port)
    });
    let journal_display = ctx
        .journal
        .to_str()
        .ok_or_else(|| task_error("journal path contains invalid text"))?;
    let command = ctx
        .public_solstone_exe
        .to_str()
        .ok_or_else(|| task_error("installed command contains invalid text"))?;
    let action = WindowsServiceAction {
        port,
        journal: journal_display.to_owned(),
        guard: ctx.guard.clone(),
    };
    let enabled = registered.as_ref().is_none_or(|definition| {
        registered_run_intent(ctx, definition, !before.instances.is_empty())
    });
    // The owner's sign-in choice is kept the way the port is; a first
    // installation starts at sign-in.
    let starts_at_sign_in = registered
        .as_ref()
        .is_none_or(|definition| definition.starts_at_sign_in);
    let xml = render_windows_task_xml(&WindowsTaskInput {
        principal_sid: &ctx.sid,
        command,
        action: &action,
        working_directory: journal_display,
        enabled,
        starts_at_sign_in,
    })
    .map_err(task_error)?;
    // The Scheduler accepts an update only against an idle task, so a
    // registration that is running is stopped first and started again after
    // the new profile reads back -- and one the owner had already stopped
    // stays stopped. Each phase proves itself before the next begins, and a
    // failed phase returns nonzero carrying the operation's own diagnostic.
    let plan = windows_service_update_plan(before.instances.len());
    let stopped;
    let after = if before.present {
        let current = if plan.stop_before_update {
            stop_task(ctx)?;
            let idle = inspect_task(ctx, Instant::now() + STOP_TIMEOUT)?;
            let idle_definition = validate_task(ctx, &idle)?;
            // The stop records itself as a disabled registration, which is the
            // only difference the update may find; anything else is a change
            // someone else made.
            if !same_task_security(&before, &idle)
                || registered.as_ref().is_none_or(|definition| {
                    definition.action != idle_definition.action
                        || definition.profile != idle_definition.profile
                        || definition.starts_at_sign_in != idle_definition.starts_at_sign_in
                        || idle_definition.enabled
                })
            {
                // The owner is left with a stopped service and no update, so
                // the line says both, in the same words the rest of this
                // surface uses for the thing that was stopped.
                return Err(task_error(
                    "background support for your journal was stopped so it could be updated. \
                     its registration with windows changed first, so the update didn't happen, \
                     and background support is stopped now.\n\
                     run `solstone journal service status` to check it, then `solstone journal service install` \
                     again.",
                ));
            }
            stopped = idle;
            &stopped
        } else {
            &before
        };
        task_scheduler::execute_until(
            &ctx.scheduler,
            &ctx.sid,
            &ctx.guard.id.as_hex(),
            Operation::Update {
                before: current,
                xml: &xml,
            },
            Instant::now() + STOP_TIMEOUT,
        )
        .map_err(task_error)?
    } else {
        task_scheduler::execute_until(
            &ctx.scheduler,
            &ctx.sid,
            &ctx.guard.id.as_hex(),
            Operation::Create { xml: &xml },
            Instant::now() + STOP_TIMEOUT,
        )
        .map_err(task_error)?
    };
    let installed = validate_task(ctx, &after)?;
    if installed.action != action || installed.starts_at_sign_in != starts_at_sign_in {
        return Err(task_error(
            "installed task action did not match its readback",
        ));
    }
    // Preserve the setup artifact only after actual scheduler readback succeeds.
    // It is never authorization to replace/delete the registered task.
    // Save the natively normalized profile that was just validated, not the
    // raw Scheduler readback: raw XML embeds the registration ACL and spells
    // the trigger principal as an account name, and the strict profile parser
    // that later reads this artifact (setup evidence) performs no native
    // normalization, so a raw artifact refuses every later setup as malformed.
    let directory = task_artifact_directory(&ctx.owner)
        .ok_or_else(|| task_error("installation provider location is unavailable"))?;
    fs::create_dir_all(&directory).map_err(task_error)?;
    fs::write(
        directory.join(format!("{}.xml", ctx.guard.namespace)),
        encode_windows_task_xml(
            after
                .validation_xml
                .as_deref()
                .ok_or_else(|| task_error("task XML readback missing"))?,
        )
        .map_err(task_error)?,
    )
    .map_err(task_error)?;
    if plan.start_after_update {
        start_task(ctx)?;
    }
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
    same_task_security(left, right) && left.xml == right.xml
}

fn same_task_security(left: &Snapshot, right: &Snapshot) -> bool {
    left.present
        && right.present
        && left.task_sddl == right.task_sddl
        && left.folder_sddl == right.folder_sddl
}

/// Whether the owner wants this registration running, read before it is
/// replaced.
///
/// A current profile carries the answer: `service stop` disables it. A running
/// task is wanted by definition. A legacy (logon-only) registration came from
/// a build whose stop left the task enabled, so an idle legacy task is only
/// wanted if its resident was taken down rather than stopped: an orderly stop
/// or sign-out clears the supervisor's lifecycle markers, while an update or a
/// kill leaves them on disk with no live process behind them.
fn registered_run_intent(
    ctx: &ServiceContext,
    definition: &WindowsTaskDefinition,
    running: bool,
) -> bool {
    running
        || (definition.enabled
            && (definition.profile == WindowsTaskProfile::Current
                || resident_was_taken_down(&ctx.journal)))
}

fn resident_was_taken_down(journal: &Path) -> bool {
    let health = journal.join("health");
    [
        "supervisor.ready",
        "supervisor.pid",
        "supervisor.process_instance",
    ]
    .iter()
    .any(|marker| health.join(marker).exists())
        && !solstone_core_system::lifecycle::readiness_is_valid(journal)
}

/// Record the owner's run intent on the registration and return its readback.
fn set_task_enabled(
    ctx: &ServiceContext,
    before: &Snapshot,
    enabled: bool,
    deadline: Instant,
) -> Result<Snapshot, ExitCode> {
    let after = task_scheduler::execute_until(
        &ctx.scheduler,
        &ctx.sid,
        &ctx.guard.id.as_hex(),
        Operation::SetEnabled { before, enabled },
        deadline,
    )
    .map_err(task_error)?;
    let definition = validate_task(ctx, &after)?;
    if definition.enabled != enabled || !same_task_security(before, &after) {
        return Err(task_error("service task changed while recording its state"));
    }
    Ok(after)
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
    .ok_or_else(|| {
        task_error(
            "your journal didn't report ready, so the start can't be confirmed. run `solstone journal service logs` to see why",
        )
    })?;
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
    solstone_core_setup::refuse_journal_in_program_folder(&ctx.journal, &ctx.public_solstone_exe)
        .map_err(task_error)?;
    let deadline = Instant::now() + READY_TIMEOUT;
    let mut before = inspect_task(ctx, deadline)?;
    if !validate_task(ctx, &before)?.enabled {
        // A stopped service is a disabled registration; starting it is the
        // owner taking that back, so every trigger resumes with it.
        set_task_enabled(ctx, &before, true, deadline)?;
        before = inspect_task(ctx, deadline)?;
        validate_task(ctx, &before)?;
    }
    let selected_guid = match before.instances.as_slice() {
        [] => {
            let started = task_scheduler::execute_until(
                &ctx.scheduler,
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
    sign_in_resume::clear(&ctx.owner.path(), &ctx.guard.id.as_hex()).map_err(task_error)?;
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
    // Disable first, so the five-minute recovery trigger cannot race the stop.
    // A public stop arms a separate hidden sign-in resume before reaching here.
    let before = if validate_task(ctx, &before)?.enabled {
        set_task_enabled(ctx, &before, false, deadline)?
    } else {
        before
    };
    let selected = match before.instances.as_slice() {
        [instance] => instance,
        [] => {
            println!("background service stopped");
            return Ok(());
        }
        _ => return Err(task_error("cannot identify one running service task")),
    };
    let run = retain_task_run(ctx, &before, &selected.guid, deadline)?;
    let frame = serde_json::json!({"tract":"supervisor", "event":"service_stop", "target":run.supervisor_instance,
        "guard":solstone_core_installation_identity::service_guard_environment(&ctx.guard)});
    let mut line = serde_json::to_string(&frame).map_err(task_error)?;
    line.push('\n');
    // The one-shot pipe handshake is bounded to a few seconds and a busy
    // resident can miss that window (observed once as "transport unavailable"
    // while the tree was healthy), so the stop request retries. A readiness
    // marker proves the supervisor process exists, not that its Callosum
    // listener has finished coming up -- measured up to and past 30 s on a
    // `restart` issued shortly after `start` -- so this retries against the
    // same command deadline the rest of `stop_task` already uses, rather than
    // a separate, tighter sub-window with no basis in the resident's own
    // worst-case startup latency.
    loop {
        let attempt = solstone_core_callosum::CallosumOneShotSender::new(
            ctx.journal.join("health/callosum.sock"),
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(3)),
        )
        .send_line(&line);
        match attempt {
            Ok(()) => break,
            Err(error) => {
                if Instant::now() >= deadline {
                    return Err(task_error(error));
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
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
            if after.instances.is_empty() && after.state == Some(1) {
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
        &ctx.scheduler,
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

const BEFORE_UNINSTALL_FALLBACK_TIMEOUT: Duration = Duration::from_secs(20);

fn clean_hook_preflight() -> Result<
    (
        PathBuf,
        OwnerBase,
        solstone_core_installation_identity::RootToken,
        CleanUninstallPreflight,
    ),
    ExitCode,
> {
    let home =
        discover_binary_home().map_err(|error| task_error(format!("service home: {error:?}")))?;
    let owner = owner_base().map_err(task_error)?;
    let executable = std::env::current_exe().map_err(task_error)?;
    let executable_dir = executable.parent().unwrap_or_else(|| Path::new("."));
    let root = resolve_identity_root_from_executable_dir(executable_dir)
        .unwrap_or_else(|| executable_dir.to_path_buf());
    let root_token = root_token_from_path(&root).map_err(task_error)?;
    let preflight = clean_uninstall_preflight(&owner, &root_token, &config_path(&home))
        .map_err(|error| task_error(format!("clean uninstall refused: {error}")))?;
    Ok((home, owner, root_token, preflight))
}

fn admitted_clean_session(
    owner: &OwnerBase,
    root_token: &solstone_core_installation_identity::RootToken,
    ctx: &ServiceContext,
) -> Result<solstone_core_installation_identity::CleanUninstallSession, ExitCode> {
    admit_clean_uninstall(CleanUninstallRequest {
        owner: owner.clone(),
        root_token: root_token.clone(),
        artifacts: ArtifactBindingEvidence::Guarded(ctx.guard.clone()),
    })
    .map_err(task_error)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UninstallIdentityHold {
    Release,
    Reacquire,
}

/// Stops and removes the service task with the installation identity hold released.
///
/// A running supervisor admits `service_stop` only after loading the installation
/// binding under the owner identity lock, so a stop requested while this process
/// holds a clean-uninstall admission waits until its timeout and leaves the task
/// running. Release the hold for the stop and removal, then admit again (the caller
/// requires the same plan) before anything else is removed, as setup's clean
/// uninstall does around this command. A failure names the step that failed;
/// nothing after it runs.
fn remove_task_with_identity_released(
    hold: &mut dyn FnMut(UninstallIdentityHold) -> Result<(), String>,
    stop: &mut dyn FnMut() -> Result<(), String>,
    delete: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), (&'static str, String)> {
    hold(UninstallIdentityHold::Release).map_err(|reason| ("identity", reason))?;
    stop().map_err(|reason| ("task", reason))?;
    delete().map_err(|reason| ("task", reason))?;
    hold(UninstallIdentityHold::Reacquire).map_err(|reason| ("identity", reason))
}

fn admitted_protected_journals(
    preflight: &CleanUninstallPreflight,
    plan: &CleanUninstallPlan,
) -> ProtectedJournals {
    let mut protected = preflight.protected_journals.clone();
    for journal in &plan.protected_journals {
        protected.insert(journal.to_path_buf());
    }
    protected
}

fn saved_task_artifact_paths(ctx: &ServiceContext) -> Result<(PathBuf, PathBuf), ExitCode> {
    let directory = task_artifact_directory(&ctx.owner)
        .ok_or_else(|| task_error("installation provider location is unavailable"))?;
    Ok((
        directory.join(format!("{}.xml", ctx.guard.namespace)),
        directory.join(format!("{}.after-update.json", ctx.guard.namespace)),
    ))
}

fn task_artifact_directory(owner: &OwnerBase) -> Option<PathBuf> {
    owner
        .path()
        .ancestors()
        .nth(2)
        .map(|parent| parent.join("journal-service"))
}

fn journal_app_paths() -> Option<[PathBuf; 4]> {
    let state_dir = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("solstone-journal");
    Some([
        state_dir.join("journal-app.json"),
        state_dir.join("journal-mark.ico"),
        state_dir.join("journal-app-webview"),
        state_dir.join("journal-app.json.tmp"),
    ])
}

fn decision_mark(decision: CleanupTargetDecision) -> CleanUninstallMark {
    match decision {
        CleanupTargetDecision::Remove => CleanUninstallMark::None,
        CleanupTargetDecision::Skip(CleanupSkip::ProtectedJournal) => {
            CleanUninstallMark::ProtectedJournal
        }
        CleanupTargetDecision::Skip(CleanupSkip::AnotherInstallation) => {
            CleanUninstallMark::AnotherInstallation
        }
        CleanupTargetDecision::Skip(CleanupSkip::RegistryUnreadable) => {
            CleanUninstallMark::RegistryUnreadable
        }
    }
}

fn hook_step(
    name: &'static str,
    state: CleanUninstallState,
    path: Option<PathBuf>,
    mark: CleanUninstallMark,
    reason: Option<String>,
) -> CleanUninstallStepResult {
    CleanUninstallStepResult {
        name,
        state,
        path,
        reason,
        mark,
    }
}

fn remove_hook_file(
    name: &'static str,
    path: PathBuf,
    kind: CleanupTargetKind,
    last_installation: bool,
    registry_known: bool,
    protected: &ProtectedJournals,
    platform: PlatformTag,
) -> CleanUninstallStepResult {
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return hook_step(
                name,
                CleanUninstallState::AlreadyAbsent,
                Some(path),
                CleanUninstallMark::None,
                None,
            );
        }
        Err(error) => {
            return hook_step(
                name,
                CleanUninstallState::Failed,
                Some(path),
                CleanUninstallMark::None,
                Some(error.to_string()),
            );
        }
        Ok(_) => {}
    }
    let decision = may_remove_cleanup_target(
        &path,
        kind,
        last_installation,
        registry_known,
        protected,
        platform,
    );
    if let CleanupTargetDecision::Skip(skip) = decision {
        return hook_step(
            name,
            CleanUninstallState::Preserved,
            Some(path),
            decision_mark(CleanupTargetDecision::Skip(skip)),
            Some(format!("{skip:?}")),
        );
    }
    match fs::remove_file(&path) {
        Ok(()) => hook_step(
            name,
            CleanUninstallState::Removed,
            Some(path),
            CleanUninstallMark::None,
            None,
        ),
        Err(error) => hook_step(
            name,
            CleanUninstallState::Failed,
            Some(path),
            CleanUninstallMark::None,
            Some(error.to_string()),
        ),
    }
}

fn remove_hook_directory(
    name: &'static str,
    path: PathBuf,
    last_installation: bool,
    registry_known: bool,
    protected: &ProtectedJournals,
    platform: PlatformTag,
) -> CleanUninstallStepResult {
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return hook_step(
                name,
                CleanUninstallState::AlreadyAbsent,
                Some(path),
                CleanUninstallMark::None,
                None,
            );
        }
        Err(error) => {
            return hook_step(
                name,
                CleanUninstallState::Failed,
                Some(path),
                CleanUninstallMark::None,
                Some(error.to_string()),
            );
        }
        Ok(_) => {}
    }
    let decision = may_remove_cleanup_target(
        &path,
        CleanupTargetKind::Shared,
        last_installation,
        registry_known,
        protected,
        platform,
    );
    if let CleanupTargetDecision::Skip(skip) = decision {
        return hook_step(
            name,
            CleanUninstallState::Preserved,
            Some(path),
            decision_mark(CleanupTargetDecision::Skip(skip)),
            Some(format!("{skip:?}")),
        );
    }
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => hook_step(
            name,
            CleanUninstallState::AlreadyAbsent,
            Some(path),
            CleanUninstallMark::None,
            None,
        ),
        Err(error) => hook_step(
            name,
            CleanUninstallState::Failed,
            Some(path),
            CleanUninstallMark::None,
            Some(error.to_string()),
        ),
        Ok(metadata) if metadata.file_type().is_symlink() => hook_step(
            name,
            CleanUninstallState::Preserved,
            Some(path),
            CleanUninstallMark::Foreign,
            Some("Foreign".into()),
        ),
        Ok(metadata) if !metadata.is_dir() => hook_step(
            name,
            CleanUninstallState::Preserved,
            Some(path),
            CleanUninstallMark::Foreign,
            Some("Foreign".into()),
        ),
        Ok(_) => match fs::remove_dir_all(&path) {
            Ok(()) => hook_step(
                name,
                CleanUninstallState::Removed,
                Some(path),
                CleanUninstallMark::None,
                None,
            ),
            Err(error) => hook_step(
                name,
                CleanUninstallState::Failed,
                Some(path),
                CleanUninstallMark::None,
                Some(error.to_string()),
            ),
        },
    }
}

fn no_binding_hook_results(home: &Path) -> Vec<CleanUninstallStepResult> {
    let mut results = vec![
        ("task", None),
        ("task-xml", None),
        ("after-update-receipt", None),
        ("config", Some(config_path(home))),
        ("resume-script", None),
    ]
    .into_iter()
    .map(|(name, path)| {
        hook_step(
            name,
            CleanUninstallState::Preserved,
            path,
            CleanUninstallMark::Foreign,
            Some("NoAdoptedBinding".into()),
        )
    })
    .collect::<Vec<_>>();
    if let Some(paths) = journal_app_paths() {
        results.extend(paths.into_iter().enumerate().map(|(index, path)| {
            hook_step(
                [
                    "journal-app-prefs",
                    "journal-app-icon",
                    "journal-app-webview",
                    "journal-app-prefs-temp",
                ][index],
                CleanUninstallState::Preserved,
                Some(path),
                CleanUninstallMark::Foreign,
                Some("NoAdoptedBinding".into()),
            )
        }));
    } else {
        results.extend(
            [
                "journal-app-prefs",
                "journal-app-icon",
                "journal-app-webview",
                "journal-app-prefs-temp",
            ]
            .into_iter()
            .map(|name| {
                hook_step(
                    name,
                    CleanUninstallState::Preserved,
                    None,
                    CleanUninstallMark::NoOwnedPath,
                    Some("NoAdoptedBinding".into()),
                )
            }),
        );
    }
    results
}

fn print_uninstall_report(
    journal: Option<&Path>,
    status: &str,
    results: &[CleanUninstallStepResult],
) {
    if let Some(journal) = journal {
        eprintln!("journal: {}", journal.display());
    }
    eprintln!("before-uninstall: {status}");
    for result in results {
        let path = result.path.as_deref().map_or_else(
            || "<no owned path>".to_owned(),
            |path| path.display().to_string(),
        );
        eprintln!(
            "{}: {} [{}] {}",
            result.name,
            result.state.as_str(),
            result.mark.as_str(),
            path
        );
    }
}

fn before_uninstall_deadline() -> Result<Instant, String> {
    let Some(value) = std::env::var_os("SOLSTONE_BEFORE_UNINSTALL_DEADLINE_UNIX_MS") else {
        return Ok(Instant::now() + BEFORE_UNINSTALL_FALLBACK_TIMEOUT);
    };
    let unix_millis = value
        .to_str()
        .ok_or("before-uninstall deadline is not Unicode")?
        .parse::<u64>()
        .map_err(|_| "before-uninstall deadline is invalid")?;
    let wall_deadline = std::time::UNIX_EPOCH
        .checked_add(Duration::from_millis(unix_millis))
        .ok_or("before-uninstall deadline is out of range")?;
    let remaining = wall_deadline
        .duration_since(std::time::SystemTime::now())
        .unwrap_or_default();
    Instant::now()
        .checked_add(remaining)
        .ok_or_else(|| "before-uninstall deadline is out of range".to_owned())
}

fn hook_cleanup_inventory(
    ctx: &ServiceContext,
    home: &Path,
    include_config: bool,
) -> Vec<(&'static str, Option<PathBuf>)> {
    let (task_xml, after_update) = saved_task_artifact_paths(ctx)
        .map(|(task_xml, receipt)| (Some(task_xml), Some(receipt)))
        .unwrap_or((None, None));
    let resume =
        sign_in_resume::cleanup_script_path(&ctx.owner.path(), &ctx.guard.id.as_hex()).ok();
    let mut steps = vec![
        ("task-xml", task_xml),
        ("after-update-receipt", after_update),
        ("resume-script", resume),
    ];
    if include_config {
        steps.push(("config", Some(config_path(home))));
    }
    if let Some(paths) = journal_app_paths() {
        steps.extend([
            ("journal-app-prefs", Some(paths[0].clone())),
            ("journal-app-icon", Some(paths[1].clone())),
            ("journal-app-webview", Some(paths[2].clone())),
            ("journal-app-prefs-temp", Some(paths[3].clone())),
        ]);
    } else {
        steps.extend([
            ("journal-app-prefs", None),
            ("journal-app-icon", None),
            ("journal-app-webview", None),
            ("journal-app-prefs-temp", None),
        ]);
    }
    steps
}

fn stop_after_failed_cleanup_step(
    journal: Option<&Path>,
    mut results: Vec<CleanUninstallStepResult>,
    name: &'static str,
    path: Option<PathBuf>,
    reason: impl std::fmt::Display,
    ctx: &ServiceContext,
    home: &Path,
    include_config: bool,
) -> ExitCode {
    results.push(hook_step(
        name,
        CleanUninstallState::Failed,
        path,
        CleanUninstallMark::None,
        Some(reason.to_string()),
    ));
    let steps = hook_cleanup_inventory(ctx, home, include_config);
    let skip_from = if name == "task" {
        0
    } else {
        steps
            .iter()
            .position(|(step_name, _)| *step_name == name)
            .map_or(steps.len(), |index| index + 1)
    };
    results.extend(steps.into_iter().skip(skip_from).map(|(name, path)| {
        hook_step(
            name,
            CleanUninstallState::Skipped,
            path,
            CleanUninstallMark::NotRun,
            Some("NotRun".into()),
        )
    }));
    print_uninstall_report(journal, "failed", &results);
    ExitCode::from(1)
}

fn remove_app_paths(
    results: &mut Vec<CleanUninstallStepResult>,
    last_installation: bool,
    registry_known: bool,
    protected: &ProtectedJournals,
    platform: PlatformTag,
) {
    let Some(paths) = journal_app_paths() else {
        results.extend(
            [
                "journal-app-prefs",
                "journal-app-icon",
                "journal-app-webview",
                "journal-app-prefs-temp",
            ]
            .into_iter()
            .map(|name| {
                hook_step(
                    name,
                    CleanUninstallState::Preserved,
                    None,
                    CleanUninstallMark::NoOwnedPath,
                    Some("NoOwnedPath".into()),
                )
            }),
        );
        return;
    };
    for (index, (name, path)) in [
        ("journal-app-prefs", paths[0].clone()),
        ("journal-app-icon", paths[1].clone()),
        ("journal-app-webview", paths[2].clone()),
        ("journal-app-prefs-temp", paths[3].clone()),
    ]
    .into_iter()
    .enumerate()
    {
        let result = if index == 2 {
            remove_hook_directory(
                name,
                path,
                last_installation,
                registry_known,
                protected,
                platform,
            )
        } else {
            remove_hook_file(
                name,
                path,
                CleanupTargetKind::Shared,
                last_installation,
                registry_known,
                protected,
                platform,
            )
        };
        let failed = result.state == CleanUninstallState::Failed;
        results.push(result);
        if failed {
            for (name, path) in [
                ("journal-app-prefs", Some(paths[0].clone())),
                ("journal-app-icon", Some(paths[1].clone())),
                ("journal-app-webview", Some(paths[2].clone())),
                ("journal-app-prefs-temp", Some(paths[3].clone())),
            ]
            .into_iter()
            .skip(index + 1)
            {
                results.push(hook_step(
                    name,
                    CleanUninstallState::Skipped,
                    path,
                    CleanUninstallMark::NotRun,
                    Some("NotRun".into()),
                ));
            }
            return;
        }
    }
}

fn run_install_action(
    port: Option<String>,
    supplied: Option<ServiceInstallationGuardArguments>,
) -> ExitCode {
    let ctx = match resolve_context() {
        Ok(ctx) => ctx,
        Err(code) => return code,
    };
    if let Err(error) = solstone_core_setup::refuse_journal_in_program_folder(
        &ctx.journal,
        &ctx.public_solstone_exe,
    ) {
        eprintln!("{error}");
        return ExitCode::from(1);
    }
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
    // `parse_service_port` accepts integer text without imposing a machine
    // range, so a value that got this far can still be out of range for a port.
    // It used to exit 1 with nothing on stderr, which is the same silence
    // `service install <port>` used to have: the owner sees a failed command
    // and no reason. The refusal is the locked one for an invalid `--port`
    // value, so this adds no new owner-facing string.
    let port = match port.as_deref().map(|text| (text, text.parse::<u16>())) {
        Some((_, Ok(port))) => Some(port),
        Some((text, Err(_))) => {
            eprintln!(
                "error: invalid port '{}'",
                solstone_core_system_health::sanitize_str_for_terminal_bounded(text)
            );
            return ExitCode::from(1);
        }
        None => None,
    };
    match install_task(&ctx, port) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

fn run_uninstall_action() -> ExitCode {
    let (home, owner, root_token, preflight) = match clean_hook_preflight() {
        Ok(value) => value,
        Err(code) => return code,
    };
    if preflight.state == CleanUninstallPreflightState::AlreadyComplete {
        print_uninstall_report(preflight.journal_path.as_deref(), "already complete", &[]);
        return ExitCode::SUCCESS;
    }
    if preflight.state == CleanUninstallPreflightState::NoBinding || preflight.root_record.is_none()
    {
        return task_error("clean uninstall requires an adopted installation identity");
    }
    let Some(journal) = preflight.journal_path.as_deref() else {
        return task_error("adopted installation has no journal path");
    };
    let ctx = match context_for_journal(journal.to_path_buf()) {
        Ok(ctx) => ctx,
        Err(error) => return task_error(error),
    };
    let session = match admitted_clean_session(&owner, &root_token, &ctx) {
        Ok(session) => session,
        Err(code) => return code,
    };
    let plan = session.plan().clone();
    let protected_journals = admitted_protected_journals(&preflight, &plan);
    let platform = owner.platform();
    let mut results = Vec::new();
    let mut session = Some(session);
    let removed = remove_task_with_identity_released(
        &mut |hold| match hold {
            UninstallIdentityHold::Release => {
                session = None;
                Ok(())
            }
            UninstallIdentityHold::Reacquire => {
                let again = admitted_clean_session(&owner, &root_token, &ctx)
                    .map_err(|code| format!("ReadmissionFailed: {code:?}"))?;
                if again.plan() != &plan {
                    return Err("PlanChangedAfterTaskRemoval".into());
                }
                session = Some(again);
                Ok(())
            }
        },
        &mut || stop_task(&ctx).map_err(|code| format!("TaskStopFailed: {code:?}")),
        &mut || delete_task(&ctx).map_err(|code| format!("TaskDeleteFailed: {code:?}")),
    );
    if let Err((name, reason)) = removed {
        return stop_after_failed_cleanup_step(
            Some(journal),
            results,
            name,
            None,
            reason,
            &ctx,
            &home,
            false,
        );
    }
    results.push(hook_step(
        "task",
        CleanUninstallState::Removed,
        None,
        CleanUninstallMark::None,
        None,
    ));
    let (task_xml, after_update) = match saved_task_artifact_paths(&ctx) {
        Ok(paths) => paths,
        Err(code) => {
            return stop_after_failed_cleanup_step(
                Some(journal),
                results,
                "task-xml",
                None,
                format!("TaskArtifactPathsUnavailable: {code:?}"),
                &ctx,
                &home,
                false,
            );
        }
    };
    for (name, path) in [
        ("task-xml", task_xml),
        ("after-update-receipt", after_update),
    ] {
        let result = remove_hook_file(
            name,
            path,
            CleanupTargetKind::PerInstall,
            false,
            preflight.registry_known,
            &protected_journals,
            platform,
        );
        if result.state == CleanUninstallState::Failed {
            return stop_after_failed_cleanup_step(
                Some(journal),
                results,
                result.name,
                result.path,
                result.reason.unwrap_or_else(|| "CleanupFailed".into()),
                &ctx,
                &home,
                false,
            );
        }
        results.push(result);
    }
    let id = ctx.guard.id.as_hex();
    let resume_path = match sign_in_resume::cleanup_script_path(&ctx.owner.path(), &id) {
        Ok(path) => path,
        Err(error) => {
            return stop_after_failed_cleanup_step(
                Some(journal),
                results,
                "resume-script",
                None,
                error,
                &ctx,
                &home,
                false,
            );
        }
    };
    if let Err(error) = sign_in_resume::clear_run_value(&id) {
        return stop_after_failed_cleanup_step(
            Some(journal),
            results,
            "resume-script",
            Some(resume_path),
            format!("RunValueClearFailed: {error}"),
            &ctx,
            &home,
            false,
        );
    }
    let resume_result = remove_hook_file(
        "resume-script",
        resume_path,
        CleanupTargetKind::PerInstall,
        false,
        preflight.registry_known,
        &protected_journals,
        platform,
    );
    if resume_result.state == CleanUninstallState::Failed {
        return stop_after_failed_cleanup_step(
            Some(journal),
            results,
            resume_result.name,
            resume_result.path,
            resume_result
                .reason
                .unwrap_or_else(|| "CleanupFailed".into()),
            &ctx,
            &home,
            false,
        );
    }
    results.push(resume_result);
    remove_app_paths(
        &mut results,
        plan.remove_owner_config,
        preflight.registry_known,
        &protected_journals,
        platform,
    );
    let failed = results
        .iter()
        .any(|result| result.state == CleanUninstallState::Failed);
    print_uninstall_report(
        Some(journal),
        if failed { "failed" } else { "complete" },
        &results,
    );
    if failed {
        return ExitCode::from(1);
    }
    drop(session);
    ExitCode::SUCCESS
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
    if let Err(code) = prepare_sign_in_resume(&ctx) {
        return code;
    }
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

/// A public stop comes back at the owner's next sign-in only when the
/// registration starts at sign-in; otherwise it holds until the owner starts
/// the journal again. An absent task is armed as before: the stop that
/// follows refuses it, and the resume script removes itself when it finds no
/// task.
fn prepare_sign_in_resume(ctx: &ServiceContext) -> Result<(), ExitCode> {
    let before = inspect_task(ctx, Instant::now() + STOP_TIMEOUT)?;
    if before.present && !validate_task(ctx, &before)?.starts_at_sign_in {
        return sign_in_resume::clear(&ctx.owner.path(), &ctx.guard.id.as_hex())
            .map_err(task_error);
    }
    sign_in_resume::arm(&ctx.owner.path(), &ctx.guard.id.as_hex(), &ctx.task_path)
        .map_err(task_error)
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
    } else if let Err(code) = install_task(&ctx, None) {
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
    if let Err(code) = prepare_sign_in_resume(&ctx) {
        return code;
    }
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
    let definition = match validate_task(&ctx, &snapshot) {
        Ok(definition) => definition,
        Err(code) => return code,
    };
    let ready = solstone_core_system::lifecycle::readiness_is_valid(&ctx.journal);
    println!("Task: {}", ctx.task_path);
    println!(
        "Supervisor readiness: {}",
        if ready { "ready" } else { "not ready" }
    );
    if !definition.enabled {
        println!(
            "{}",
            if definition.starts_at_sign_in {
                STOPPED_STATUS_COPY
            } else {
                STOPPED_UNTIL_STARTED_STATUS_COPY
            }
        );
    }
    ExitCode::SUCCESS
}

/// The one status line a stopped service adds until the next sign-in.
const STOPPED_STATUS_COPY: &str =
    "Stopped: background support for your journal starts again when you next sign in.";
/// The same line when signing in does not start the journal.
const STOPPED_UNTIL_STARTED_STATUS_COPY: &str = "Stopped: background support for your journal stays off until you start it or open the journal app.";

const APP_STATUS_SCHEMA: &str = "solstone-journal-app-status-v1";

/// One JSON line for the Windows journal app to poll. It exits 0 with
/// `"journal": null` when this installation has not been set up yet, so the
/// app can tell that apart from a failure, which exits nonzero with the
/// reason on stderr. One inspect, so one Task Scheduler worker.
fn run_app_status() -> ExitCode {
    match installation_binding_absent() {
        Ok(true) => {
            println!(
                "{}",
                serde_json::json!({
                    "schema": APP_STATUS_SCHEMA,
                    "journal": null,
                    "installed": false,
                    "wants_running": false,
                    "running": false,
                    "ready": false,
                    "starts_at_sign_in": false,
                    "port": null,
                })
            );
            return ExitCode::SUCCESS;
        }
        Ok(false) => {}
        Err(code) => return code,
    }
    let ctx = match resolve_context() {
        Ok(ctx) => ctx,
        Err(code) => return code,
    };
    let snapshot = match inspect_task(&ctx, Instant::now() + STOP_TIMEOUT) {
        Ok(snapshot) => snapshot,
        Err(code) => return code,
    };
    let definition = if snapshot.present {
        match validate_task(&ctx, &snapshot) {
            Ok(definition) => Some(definition),
            Err(code) => return code,
        }
    } else {
        None
    };
    let Some(journal) = ctx.journal.to_str() else {
        return task_error("journal path contains invalid text");
    };
    println!(
        "{}",
        serde_json::json!({
            "schema": APP_STATUS_SCHEMA,
            "journal": journal,
            "installed": definition.is_some(),
            "wants_running": definition.as_ref().is_some_and(|definition| definition.enabled),
            "running": !snapshot.instances.is_empty(),
            "ready": solstone_core_system::lifecycle::readiness_is_valid(&ctx.journal),
            "starts_at_sign_in": definition
                .as_ref()
                .is_some_and(|definition| definition.starts_at_sign_in),
            "port": definition.as_ref().map(|definition| definition.action.port),
        })
    );
    ExitCode::SUCCESS
}

/// Set whether signing in starts the journal, from the Windows journal app.
fn run_sign_in(on: bool) -> ExitCode {
    let ctx = match resolve_context() {
        Ok(ctx) => ctx,
        Err(code) => return code,
    };
    match set_sign_in(&ctx, on) {
        Ok(()) => {
            println!("{}", serde_json::json!({ "starts_at_sign_in": on }));
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}

/// Flip both triggers without touching a running journal or the owner's run
/// intent, then keep the sign-in resume consistent with the switch: a journal
/// the owner stopped comes back at the next sign-in only while the switch is
/// on, as it would have had it been on when the owner stopped it.
fn set_sign_in(ctx: &ServiceContext, on: bool) -> Result<(), ExitCode> {
    let deadline = Instant::now() + STOP_TIMEOUT;
    let before = inspect_task(ctx, deadline)?;
    let registered = validate_task(ctx, &before)?;
    let enabled = if registered.starts_at_sign_in == on {
        registered.enabled
    } else {
        // Rendered from the registration itself, so only the switch differs.
        // A legacy registration (always on) is moved to the current profile
        // the first time it is switched off, as an install would move it.
        let xml = render_windows_task_xml(&WindowsTaskInput {
            principal_sid: &registered.principal_sid,
            command: &registered.command,
            action: &registered.action,
            working_directory: &registered.working_directory,
            enabled: registered.enabled,
            starts_at_sign_in: on,
        })
        .map_err(task_error)?;
        let after = task_scheduler::execute_until(
            &ctx.scheduler,
            &ctx.sid,
            &ctx.guard.id.as_hex(),
            Operation::SetSignIn {
                before: &before,
                on,
                xml: &xml,
            },
            deadline,
        )
        .map_err(task_error)?;
        let changed = validate_task(ctx, &after)?;
        if changed.starts_at_sign_in != on
            || changed.enabled != registered.enabled
            || changed.action != registered.action
            || changed.principal_sid != registered.principal_sid
            || changed.command != registered.command
            || changed.working_directory != registered.working_directory
            || !same_task_security(&before, &after)
        {
            return Err(task_error("service task changed while recording its state"));
        }
        changed.enabled
    };
    if !on {
        sign_in_resume::clear(&ctx.owner.path(), &ctx.guard.id.as_hex()).map_err(task_error)
    } else if !enabled {
        sign_in_resume::arm(&ctx.owner.path(), &ctx.guard.id.as_hex(), &ctx.task_path)
            .map_err(task_error)
    } else {
        Ok(())
    }
}

/// Velopack's before-uninstall hook has 30 seconds. Deleting a registration
/// leaves an already-running instance alone; Velopack then sweeps the install
/// root and stops that process itself. This removes the recovery trigger before
/// the files it points at disappear, without waiting for a full service stop.
fn run_before_uninstall() -> ExitCode {
    let (home, owner, root_token, preflight) = match clean_hook_preflight() {
        Ok(value) => value,
        Err(code) => return code,
    };
    let journal = preflight.journal_path.clone();
    match preflight.state {
        CleanUninstallPreflightState::AlreadyComplete => {
            print_uninstall_report(journal.as_deref(), "already complete", &[]);
            return ExitCode::SUCCESS;
        }
        CleanUninstallPreflightState::NoBinding => {
            let results = no_binding_hook_results(&home);
            print_uninstall_report(journal.as_deref(), "NoAdoptedBinding", &results);
            return ExitCode::SUCCESS;
        }
        CleanUninstallPreflightState::Proceed => {}
    }
    if preflight.root_record.is_none() {
        return task_error("installation identity census is unreadable");
    }
    let Some(journal_path) = journal.as_deref() else {
        return task_error("adopted installation has no journal path");
    };
    let ctx = match context_for_journal(journal_path.to_path_buf()) {
        Ok(ctx) => ctx,
        Err(error) => return task_error(error),
    };
    let session = match admitted_clean_session(&owner, &root_token, &ctx) {
        Ok(session) => session,
        Err(code) => return code,
    };
    let plan = session.plan().clone();
    let protected_journals = admitted_protected_journals(&preflight, &plan);
    let deadline = match before_uninstall_deadline() {
        Ok(deadline) => deadline,
        Err(error) => {
            return stop_after_failed_cleanup_step(
                journal.as_deref(),
                Vec::new(),
                "task",
                None,
                error,
                &ctx,
                &home,
                true,
            );
        }
    };
    if Instant::now() >= deadline {
        return stop_after_failed_cleanup_step(
            journal.as_deref(),
            Vec::new(),
            "task",
            None,
            "DeadlineElapsed",
            &ctx,
            &home,
            true,
        );
    }
    let mut results = Vec::new();
    let before = match inspect_task(&ctx, deadline) {
        Ok(before) => before,
        Err(code) => {
            return stop_after_failed_cleanup_step(
                journal.as_deref(),
                results,
                "task",
                None,
                format!("TaskInspectFailed: {code:?}"),
                &ctx,
                &home,
                true,
            );
        }
    };
    if before.present {
        if let Err(code) = validate_task(&ctx, &before) {
            return stop_after_failed_cleanup_step(
                journal.as_deref(),
                results,
                "task",
                None,
                format!("TaskGuardMismatch: {code:?}"),
                &ctx,
                &home,
                true,
            );
        }
        let after = match task_scheduler::execute_until(
            &ctx.scheduler,
            &ctx.sid,
            &ctx.guard.id.as_hex(),
            Operation::DeleteBeforeUninstall { before: &before },
            deadline,
        ) {
            Ok(after) => after,
            Err(error) => {
                return stop_after_failed_cleanup_step(
                    journal.as_deref(),
                    results,
                    "task",
                    None,
                    error,
                    &ctx,
                    &home,
                    true,
                );
            }
        };
        if after.present {
            return stop_after_failed_cleanup_step(
                journal.as_deref(),
                results,
                "task",
                None,
                "TaskRemainedRegistered",
                &ctx,
                &home,
                true,
            );
        }
        results.push(hook_step(
            "task",
            CleanUninstallState::Removed,
            None,
            CleanUninstallMark::None,
            None,
        ));
    } else {
        results.push(hook_step(
            "task",
            CleanUninstallState::AlreadyAbsent,
            None,
            CleanUninstallMark::None,
            None,
        ));
    }

    let (task_xml, after_update) = match saved_task_artifact_paths(&ctx) {
        Ok(paths) => paths,
        Err(code) => {
            return stop_after_failed_cleanup_step(
                journal.as_deref(),
                results,
                "task-xml",
                None,
                format!("TaskArtifactPathsUnavailable: {code:?}"),
                &ctx,
                &home,
                true,
            );
        }
    };
    for (name, path) in [
        ("task-xml", task_xml),
        ("after-update-receipt", after_update),
    ] {
        let result = remove_hook_file(
            name,
            path,
            CleanupTargetKind::PerInstall,
            false,
            preflight.registry_known,
            &protected_journals,
            owner.platform(),
        );
        if result.state == CleanUninstallState::Failed {
            return stop_after_failed_cleanup_step(
                journal.as_deref(),
                results,
                result.name,
                result.path,
                result.reason.unwrap_or_else(|| "CleanupFailed".into()),
                &ctx,
                &home,
                true,
            );
        }
        results.push(result);
    }
    let id = ctx.guard.id.as_hex();
    let resume_path = match sign_in_resume::cleanup_script_path(&ctx.owner.path(), &id) {
        Ok(path) => path,
        Err(error) => {
            return stop_after_failed_cleanup_step(
                journal.as_deref(),
                results,
                "resume-script",
                None,
                error,
                &ctx,
                &home,
                true,
            );
        }
    };
    if let Err(error) = sign_in_resume::clear_run_value(&id) {
        return stop_after_failed_cleanup_step(
            journal.as_deref(),
            results,
            "resume-script",
            Some(resume_path),
            format!("RunValueClearFailed: {error}"),
            &ctx,
            &home,
            true,
        );
    }
    let resume_result = remove_hook_file(
        "resume-script",
        resume_path,
        CleanupTargetKind::PerInstall,
        false,
        preflight.registry_known,
        &protected_journals,
        owner.platform(),
    );
    if resume_result.state == CleanUninstallState::Failed {
        return stop_after_failed_cleanup_step(
            journal.as_deref(),
            results,
            resume_result.name,
            resume_result.path,
            resume_result
                .reason
                .unwrap_or_else(|| "CleanupFailed".into()),
            &ctx,
            &home,
            true,
        );
    }
    results.push(resume_result);
    let config_result = remove_hook_file(
        "config",
        config_path(&home),
        CleanupTargetKind::Shared,
        plan.remove_owner_config,
        preflight.registry_known,
        &protected_journals,
        owner.platform(),
    );
    if config_result.state == CleanUninstallState::Failed {
        return stop_after_failed_cleanup_step(
            journal.as_deref(),
            results,
            config_result.name,
            config_result.path,
            config_result
                .reason
                .unwrap_or_else(|| "CleanupFailed".into()),
            &ctx,
            &home,
            true,
        );
    }
    results.push(config_result);
    remove_app_paths(
        &mut results,
        plan.remove_owner_config,
        preflight.registry_known,
        &protected_journals,
        owner.platform(),
    );
    let failed = results
        .iter()
        .any(|result| result.state == CleanUninstallState::Failed);
    print_uninstall_report(
        journal.as_deref(),
        if failed { "failed" } else { "complete" },
        &results,
    );
    if failed {
        return ExitCode::from(1);
    }
    match session.commit_tombstone() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => task_error(error),
    }
}

/// Whether this install root has no saved binding: `journal setup` has not
/// run for it yet, or it has already been uninstalled.
fn installation_binding_absent() -> Result<bool, ExitCode> {
    let owner = owner_base().map_err(task_error)?;
    let exe = std::env::current_exe().map_err(task_error)?;
    let root =
        resolve_identity_root_from_executable_dir(exe.parent().unwrap_or_else(|| Path::new(".")))
            .ok_or_else(|| task_error("could not resolve uninstall root"))?;
    let token = root_token_from_path(&root).map_err(task_error)?;
    match load_installation_binding(&owner, &token) {
        Ok(_) => Ok(false),
        Err(IdentityError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(true)
        }
        Err(error) => Err(task_error(error)),
    }
}

/// The step an update or reinstall owes the resident it took down.
///
/// 🔴 Velopack stops every process under the app directory to apply an update,
/// and a Setup run over an existing install does the same, and nothing started
/// the resident again: the task sat Ready with stale lifecycle markers until
/// the owner ran `journal service start` (measured on Windows 11 after
/// `Update.exe apply` and after a Setup over an installed build). Velopack
/// runs its hooks from the *new* build, which starts this action detached.
/// It moves the registration to the current profile (re-registering an
/// already-current one changes nothing) and starts the service if the owner
/// wants it running. A service the owner stopped stays stopped.
fn run_resume_after_update() -> ExitCode {
    let Ok(ctx) = resolve_context() else {
        // A first install has no binding yet: setup registers the service.
        return ExitCode::SUCCESS;
    };
    let outcome = resume_after_update(&ctx);
    record_after_update(
        &ctx,
        match &outcome {
            Ok(outcome) => *outcome,
            Err(_) => "failed",
        },
    );
    match outcome {
        Ok(_) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

fn resume_after_update(ctx: &ServiceContext) -> Result<&'static str, ExitCode> {
    if !inspect_task(ctx, Instant::now() + STOP_TIMEOUT)?.present {
        return Ok("not-installed");
    }
    install_task(ctx, None)?;
    let after = inspect_task(ctx, Instant::now() + STOP_TIMEOUT)?;
    if !validate_task(ctx, &after)?.enabled {
        return Ok("stopped");
    }
    start_task(ctx)?;
    Ok("started")
}

/// Nobody reads a detached hook's output, so its outcome is kept beside the
/// saved task profile, where support and the Windows proofs can find it.
fn record_after_update(ctx: &ServiceContext, outcome: &str) {
    let Some(directory) = task_artifact_directory(&ctx.owner) else {
        return;
    };
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let record = serde_json::json!({
        "schema": "solstone-windows-after-update-v1",
        "outcome": outcome,
        "at_unix_seconds": at,
        "version": env!("CARGO_PKG_VERSION"),
    });
    let _ = fs::write(
        directory.join(format!("{}.after-update.json", ctx.guard.namespace)),
        record.to_string(),
    );
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

#[cfg(test)]
mod uninstall_identity_hold_tests {
    use super::{UninstallIdentityHold, remove_task_with_identity_released};
    use std::cell::{Cell, RefCell};

    #[test]
    fn a_running_task_is_stopped_and_removed_while_the_identity_hold_is_released() {
        let held = Cell::new(true);
        let order = RefCell::new(Vec::new());
        let result = remove_task_with_identity_released(
            &mut |hold| {
                order.borrow_mut().push(format!("{hold:?}"));
                held.set(hold == UninstallIdentityHold::Reacquire);
                Ok(())
            },
            // The supervisor's stop admission loads the binding under the same lock.
            &mut || {
                order.borrow_mut().push("stop".into());
                if held.get() {
                    Err("stop waited on the identity lock".into())
                } else {
                    Ok(())
                }
            },
            &mut || {
                order.borrow_mut().push("delete".into());
                if held.get() {
                    Err("delete ran under the identity lock".into())
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(result, Ok(()));
        assert_eq!(*order.borrow(), ["Release", "stop", "delete", "Reacquire"]);
        assert!(held.get());
    }

    #[test]
    fn a_failed_stop_or_a_refused_readmission_names_its_step_and_runs_nothing_after_it() {
        let deleted = Cell::new(false);
        let stopped = remove_task_with_identity_released(
            &mut |_| Ok(()),
            &mut || Err("TaskStopFailed".into()),
            &mut || {
                deleted.set(true);
                Ok(())
            },
        );
        assert_eq!(stopped, Err(("task", "TaskStopFailed".to_string())));
        assert!(!deleted.get());

        let readmitted = remove_task_with_identity_released(
            &mut |hold| match hold {
                UninstallIdentityHold::Release => Ok(()),
                UninstallIdentityHold::Reacquire => Err("PlanChangedAfterTaskRemoval".into()),
            },
            &mut || Ok(()),
            &mut || Ok(()),
        );
        assert_eq!(
            readmitted,
            Err(("identity", "PlanChangedAfterTaskRemoval".to_string()))
        );
    }
}
