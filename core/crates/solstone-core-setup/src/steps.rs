// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Ordered setup steps and the native run loop.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Map, Value, json};
use solstone_core_check::{Severity, gather_host_inputs};
use solstone_core_installation_identity::{GuardFields, SetupAdmission, service_guard_environment};
use solstone_core_journal_config::get_journal_config_path;
use solstone_core_journal_config_write::{JournalConfigMutation, mutate_journal_config};
use solstone_core_journal_io::{LockOptions, legacy_log_alias::cleanup_legacy_log_aliases};

use crate::args::{ResolvedSetup, SetupArgs, SetupMode, resolve_expanded_path};
use crate::events::{ErrorCode, EventSink, EventType, SkipReason, StepName};
use crate::manifest::{SetupManifest, can_skip, prior_steps, read_manifest, write_manifest};
use crate::user_config::{read_user_config, write_user_config};
#[cfg(not(windows))]
use crate::wrapper::{
    WrapperEnvironment, WrapperError, ensure_user_bin_on_path, provision_wrappers,
};
use crate::wrapper::{is_live_app_owned_child_launcher, wrapper_paths};

const LOCAL_MODEL: &str = "local/qwen3.5-4b";
const LOCAL_INSTALL_HINT: &str = "journal install-provider local";
const SOL_ALREADY_KEEPS_JOURNAL_NARRATION: &str = "solstone on this Mac already keeps this journal, so setup did not install a background launcher.\nRun journal doctor if something looks wrong.";

pub const ALL_STEP_NAMES: [StepName; 8] = [
    StepName::Doctor,
    StepName::Journal,
    StepName::InstallModels,
    StepName::SkillsUser,
    StepName::SkillsJournal,
    StepName::Wrapper,
    StepName::Service,
    StepName::Brain,
];

/// Steps whose failure is reported but does not halt the run.
///
/// `InstallModels` belongs here because it runs third of eight, ahead of
/// `Wrapper`, `Service` and `Brain`. Halting there leaves a brand-new owner with
/// no wrappers, no unit and no service — no journal at all — because one
/// *optional* inference asset did not verify. Continuing leaves them a working
/// journal missing one capability, which `journal doctor` reports and
/// `journal install-models` re-runs.
///
/// ⚠ This does not hide the failure: the aggregate below still returns the
/// step's own non-zero exit code and emits `SetupCompleted{status: "failed"}`.
/// Setup reports failure and still finishes building something the owner can use.
const CONTINUE_AFTER_FAILURE: [StepName; 3] = [
    StepName::InstallModels,
    StepName::SkillsUser,
    StepName::SkillsJournal,
];
const DOCTOR_TIMEOUT_SECONDS: u64 = 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRequest {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

pub trait CommandRunner {
    fn run(&mut self, request: &CommandRequest) -> Result<CommandOutput, String>;
}

/// Service predicates stay independently fakeable while production uses native siblings.
pub trait ServiceOps {
    fn is_installed(
        &mut self,
        runner: &mut dyn CommandRunner,
        journal: &Path,
    ) -> Result<bool, String>;
    fn health_check(
        &mut self,
        runner: &mut dyn CommandRunner,
        journal: &Path,
    ) -> Result<bool, String>;
    fn restart(&mut self, runner: &mut dyn CommandRunner, journal: &Path) -> Result<(), String>;
    fn up(&mut self, runner: &mut dyn CommandRunner, journal: &Path) -> Result<i32, String>;
}

pub struct NativeServiceOps {
    pub journal_bin: PathBuf,
}

impl NativeServiceOps {
    fn run(&self, runner: &mut dyn CommandRunner, args: &[&str]) -> Result<CommandOutput, String> {
        runner.run(&CommandRequest {
            program: self.journal_bin.clone(),
            args: args.iter().map(|value| (*value).to_owned()).collect(),
            timeout_seconds: None,
        })
    }
}

impl ServiceOps for NativeServiceOps {
    fn is_installed(
        &mut self,
        runner: &mut dyn CommandRunner,
        _journal: &Path,
    ) -> Result<bool, String> {
        self.run(runner, &["service", "status"])
            .map(|output| output.exit_code == 0)
    }
    fn health_check(
        &mut self,
        runner: &mut dyn CommandRunner,
        _journal: &Path,
    ) -> Result<bool, String> {
        self.run(runner, &["health"])
            .map(|output| output.exit_code == 0)
    }
    fn restart(&mut self, runner: &mut dyn CommandRunner, _journal: &Path) -> Result<(), String> {
        let output = self.run(runner, &["service", "restart"])?;
        if output.exit_code == 0 {
            Ok(())
        } else {
            Err(format!("service restart exited {}", output.exit_code))
        }
    }
    fn up(&mut self, runner: &mut dyn CommandRunner, _journal: &Path) -> Result<i32, String> {
        self.run(runner, &["up"]).map(|output| output.exit_code)
    }
}

pub trait CheckReportBuilder {
    fn local_provider_blocked(&self, journal: &Path) -> bool;
}

pub struct NativeCheckReportBuilder;

impl CheckReportBuilder for NativeCheckReportBuilder {
    fn local_provider_blocked(&self, journal: &Path) -> bool {
        let report = solstone_core_check::build_check_report(&gather_host_inputs(
            journal,
            env!("CARGO_PKG_VERSION"),
        ));
        report.overall == Severity::Blocked
    }
}

/// Production subprocess runner. Native setup's callers select sibling commands.
pub struct ProcessCommandRunner;

impl CommandRunner for ProcessCommandRunner {
    fn run(&mut self, request: &CommandRequest) -> Result<CommandOutput, String> {
        let mut child = Command::new(&request.program)
            .args(&request.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| error.to_string())?;
        let timed_out = if let Some(timeout_seconds) = request.timeout_seconds {
            let deadline = Instant::now() + Duration::from_secs(timeout_seconds);
            loop {
                if child
                    .try_wait()
                    .map_err(|error| error.to_string())?
                    .is_some()
                {
                    break false;
                }
                if Instant::now() >= deadline {
                    child.kill().map_err(|error| error.to_string())?;
                    break true;
                }
                thread::sleep(Duration::from_millis(10));
            }
        } else {
            false
        };
        let output = child
            .wait_with_output()
            .map_err(|error| error.to_string())?;
        Ok(CommandOutput {
            exit_code: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            timed_out,
        })
    }
}

pub trait ExistingJournalPrompt {
    fn accept_existing_journal(&mut self, path: &Path) -> Result<bool, String>;
}

pub struct NoPrompt;

impl ExistingJournalPrompt for NoPrompt {
    fn accept_existing_journal(&mut self, _path: &Path) -> Result<bool, String> {
        Ok(false)
    }
}

pub struct SetupContext<'a> {
    pub args: &'a SetupArgs,
    pub resolved: &'a ResolvedSetup,
    pub mode: SetupMode,
    pub home_dir: PathBuf,
    pub config_path: PathBuf,
    pub journal_path: PathBuf,
    pub current_dir: PathBuf,
    pub project_root: PathBuf,
    pub install_bin_dir: PathBuf,
    pub manifest_path: PathBuf,
    pub stdin_is_tty: bool,
    pub stdout_is_tty: bool,
    pub now: fn() -> String,
    pub runner: &'a mut dyn CommandRunner,
    pub prompt: &'a mut dyn ExistingJournalPrompt,
    pub events: Option<&'a mut dyn EventSink>,
    pub wrapper_backup_dir: Option<PathBuf>,
    pub service_ops: &'a mut dyn ServiceOps,
    pub already_keeps_journal_probe: fn(&SetupContext<'_>) -> Result<bool, String>,
    pub is_macos: bool,
    pub check_report_builder: &'a dyn CheckReportBuilder,
    /// Holds the owner-wide and namespace leases across every mutating setup step.
    pub installation_admission: Option<SetupAdmission>,
    /// Guard-drifted artifacts that must be reconciled even after a prior successful step.
    pub identity_guard_repair_steps: Vec<StepName>,
    /// Exact V1 launchers admitted together with a provider-less schema-v1 manifest.
    pub legacy_replacement: bool,
}

impl<'a> SetupContext<'a> {
    #[must_use]
    pub fn jsonl(&self) -> bool {
        self.args.jsonl
    }

    fn emit(&mut self, event: EventType, fields: Map<String, Value>) {
        if let Some(sink) = self.events.as_deref_mut() {
            let _ = sink.emit(event, &(self.now)(), fields);
        }
    }

    fn forward_line(&mut self, line: &str) {
        if let Some(sink) = self.events.as_deref_mut() {
            let _ = sink.forward_line(line);
        }
    }

    fn sol_already_keeps_journal(&self) -> bool {
        if !self.is_macos {
            return false;
        }
        match (self.already_keeps_journal_probe)(self) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("warning: could not inspect solstone launcher ownership: {error}");
                false
            }
        }
    }
}

/// macOS currently implements the app-owned-child signal; native supervisor conflict
/// state has no reusable typed seam yet and deliberately remains a conservative false.
pub fn native_already_keeps_journal_probe(context: &SetupContext<'_>) -> Result<bool, String> {
    let journal = wrapper_paths(&context.home_dir).journal;
    // solstone-macos still writes the app-owned-child marker at this path.
    // Do not derive it from WrapperPaths; this repo only reads that file.
    let macos_app_owned_sol = context.home_dir.join(".local/bin/sol");
    Ok([macos_app_owned_sol, journal]
        .iter()
        .all(|path| is_live_app_owned_child_launcher(path)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StepStatus {
    Ok,
    Skipped,
    Failed,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StepError {
    pub code: ErrorCode,
    pub message: String,
    pub details: String,
    pub exit_code: i32,
}

/// Wrapper provisioning warnings intentionally have a smaller payload than failures.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WrapperWarning {
    pub message: String,
    pub fix_hint: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BrainWarning {
    pub code: ErrorCode,
    pub message: String,
    pub details: String,
    pub exit_code: i32,
    pub fix_hint: String,
}

/// Python stores failure and warning payloads under the same `error` manifest key.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum StepErrorPayload {
    Failure(StepError),
    WrapperWarning(WrapperWarning),
    BrainWarning(BrainWarning),
}

impl StepErrorPayload {
    fn failure_exit_code(&self) -> Option<i32> {
        match self {
            Self::Failure(error) => Some(error.exit_code),
            Self::WrapperWarning(_) => None,
            Self::BrainWarning(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StepResult {
    pub name: StepName,
    pub status: StepStatus,
    pub paths: Vec<String>,
    pub started_at: String,
    pub finished_at: String,
    pub error: Option<StepErrorPayload>,
    pub reason: Option<String>,
}

impl StepResult {
    fn new(name: StepName, status: StepStatus, paths: Vec<PathBuf>, now: String) -> Self {
        Self {
            name,
            status,
            paths: paths
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            started_at: now.clone(),
            finished_at: now,
            error: None,
            reason: None,
        }
    }

    fn failed(name: StepName, paths: Vec<PathBuf>, now: String, error: StepError) -> Self {
        let mut result = Self::new(name, StepStatus::Failed, paths, now);
        result.error = Some(StepErrorPayload::Failure(error));
        result
    }

    fn brain_warning(name: StepName, now: String, warning: BrainWarning) -> Self {
        let mut result = Self::new(name, StepStatus::Warning, Vec::new(), now);
        result.error = Some(StepErrorPayload::BrainWarning(warning));
        result.reason = Some(SkipReason::LocalBootstrapDidNotStart.as_str().to_owned());
        result
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepExecutionError {
    DeadEnd {
        message: String,
        exit_code: i32,
        step_name: Option<StepName>,
        error_code: Option<ErrorCode>,
    },
    Unhandled {
        message: String,
    },
}

type Executor = fn(&mut SetupContext<'_>) -> Result<StepResult, StepExecutionError>;
type PlanRenderer = fn(&SetupContext<'_>) -> String;

#[derive(Clone, Copy)]
pub struct StepSpec {
    pub name: StepName,
    pub executor: Option<Executor>,
    pub plan: PlanRenderer,
}

/// One authoritative setup table: executor and plan body stay paired.
#[must_use]
pub fn step_specs() -> [StepSpec; 8] {
    [
        StepSpec {
            name: StepName::Doctor,
            executor: Some(step_doctor),
            plan: plan_doctor,
        },
        StepSpec {
            name: StepName::Journal,
            executor: Some(step_journal),
            plan: plan_journal,
        },
        StepSpec {
            name: StepName::InstallModels,
            executor: Some(step_install_models),
            plan: plan_install_models,
        },
        StepSpec {
            name: StepName::SkillsUser,
            executor: Some(step_skills_user),
            plan: plan_skills_user,
        },
        StepSpec {
            name: StepName::SkillsJournal,
            executor: Some(step_skills_journal),
            plan: plan_skills_journal,
        },
        StepSpec {
            name: StepName::Wrapper,
            executor: Some(step_wrapper),
            plan: plan_wrapper,
        },
        StepSpec {
            name: StepName::Service,
            executor: Some(step_service),
            plan: plan_service,
        },
        StepSpec {
            name: StepName::Brain,
            executor: Some(step_brain),
            plan: plan_brain,
        },
    ]
}

/// Native executors available in this implementation stage.
#[must_use]
pub fn implemented_step_specs() -> Vec<StepSpec> {
    step_specs()
        .into_iter()
        .filter(|spec| spec.executor.is_some())
        .collect()
}

/// Render the complete explain/dry-run plan without running a step.
#[must_use]
fn plan_value<'a>(resolved: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    resolved.get(key)?.as_object()?.get("value")
}

/// `  <key>: <value> (<source>)`, matching the reference's provenance line.
fn resolved_plan_line(resolved: &Map<String, Value>, key: &str) -> Option<String> {
    let entry = resolved.get(key)?.as_object()?;
    let value = entry.get("value")?;
    let source = entry.get("source")?.as_str()?;
    let rendered = match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    Some(format!("  {key}: {rendered} ({source})"))
}

pub fn render_plan(context: &SetupContext<'_>, dry_run: bool) -> Vec<String> {
    let heading = if dry_run {
        "setup dry-run:"
    } else {
        "setup plan:"
    };
    let mut lines = vec![
        heading.to_owned(),
        format!("  mode: {}", mode_name(context.mode)),
        format!(
            "  journal: {} ({})",
            context.journal_path.display(),
            context.resolved.journal_source
        ),
    ];
    // The plan exists to tell an owner what a run would do with the arguments
    // they just gave it, so every resolved value carries its provenance. An
    // owner who passes --port and is shown no port cannot tell whether it took
    // effect. The runtime already honours all of these; only the rendering was
    // short, which is the narrowing that is easiest to miss because nothing
    // fails.
    for key in ["port", "variant", "step_timeout_seconds"] {
        if let Some(line) = resolved_plan_line(&context.resolved.args_resolved, key) {
            lines.push(line);
        }
    }
    if let Some(value) =
        plan_value(&context.resolved.args_resolved, "is_source_checkout").and_then(Value::as_bool)
    {
        lines.push(format!(
            "  source checkout: {}",
            if value { "True" } else { "False" }
        ));
    }
    for (index, spec) in step_specs().iter().enumerate() {
        lines.push(format!(
            "[step {}/{}] {}",
            index + 1,
            ALL_STEP_NAMES.len(),
            spec.name.as_str()
        ));
        lines.push((spec.plan)(context));
    }
    lines
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    pub exit_code: i32,
    pub ran_steps: Vec<StepName>,
    pub dead_end: Option<DeadEndOutcome>,
    pub duration_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadEndOutcome {
    pub message: String,
    pub step_name: Option<StepName>,
    pub error_code: Option<ErrorCode>,
}

pub fn run_setup(context: &mut SetupContext<'_>, steps: &[StepSpec]) -> RunOutcome {
    let setup_started = Instant::now();
    if context.jsonl() {
        context.emit(
            EventType::SetupStarted,
            Map::from_iter([
                ("started_at".into(), json!((context.now)())),
                ("version".into(), json!(env!("CARGO_PKG_VERSION"))),
                ("mode".into(), json!(mode_name(context.mode))),
                (
                    "args_resolved".into(),
                    Value::Object(context.resolved.args_resolved.clone()),
                ),
            ]),
        );
    }
    if context.resolved.should_short_circuit() {
        if context.jsonl() {
            context.emit(
                EventType::SetupCompleted,
                Map::from_iter([
                    ("status".into(), json!("ok")),
                    (
                        "duration_ms".into(),
                        json!(setup_started.elapsed().as_millis()),
                    ),
                ]),
            );
        }
        return RunOutcome {
            exit_code: 0,
            ran_steps: Vec::new(),
            dead_end: None,
            duration_ms: setup_started.elapsed().as_millis(),
        };
    }

    let prior: BTreeMap<String, Map<String, Value>> = if context.args.force {
        Default::default()
    } else {
        read_manifest(&context.manifest_path).map_or_else(Default::default, |manifest| {
            prior_steps(&manifest)
                .into_iter()
                .map(|(name, step)| (name, step.clone()))
                .collect()
        })
    };
    narrate_prior_run(context, read_manifest(&context.manifest_path).as_ref());
    let mut manifest = SetupManifest::initial(
        (context.now)(),
        mode_name(context.mode).to_owned(),
        context.resolved.args_resolved.clone(),
    );
    let mut aggregate = Vec::new();
    let mut ran_steps = Vec::new();
    for (offset, spec) in steps.iter().enumerate() {
        let index = offset + 1;
        let step_started = Instant::now();
        let command = command_for_step(context, spec.name);
        if let Some(command) = &command {
            narrate(
                context,
                &format!(
                    "[step {index}/{}] running {}: {}",
                    ALL_STEP_NAMES.len(),
                    spec.name.as_str(),
                    command.join(" ")
                ),
            );
        } else {
            narrate(
                context,
                &format!(
                    "[step {index}/{}] running {}...",
                    ALL_STEP_NAMES.len(),
                    spec.name.as_str()
                ),
            );
        }
        let mut started_fields = Map::from_iter([
            ("step".into(), json!(spec.name.as_str())),
            ("index".into(), json!(index)),
            ("total".into(), json!(ALL_STEP_NAMES.len())),
        ]);
        if let Some(command) = command {
            started_fields.insert("command".into(), json!(command));
        }
        context.emit(EventType::StepStarted, started_fields);
        let result = if let Some(prior_step) = prior.get(spec.name.as_str()) {
            if can_skip(Some(prior_step))
                && !context.identity_guard_repair_steps.contains(&spec.name)
            {
                let paths = prior_step
                    .get("paths")
                    .and_then(Value::as_array)
                    .map_or_else(Vec::new, |paths| {
                        paths
                            .iter()
                            .filter_map(Value::as_str)
                            .map(PathBuf::from)
                            .collect()
                    });
                if spec.name == StepName::Service {
                    match resume_service(context, paths.clone()) {
                        Ok(Some(result)) => result,
                        Ok(None) => match spec
                            .executor
                            .expect("only implemented steps are runnable")(
                            context
                        ) {
                            Ok(result) => result,
                            Err(StepExecutionError::DeadEnd {
                                message,
                                exit_code,
                                step_name,
                                error_code,
                            }) => {
                                return dead_end_outcome(
                                    exit_code,
                                    ran_steps,
                                    message,
                                    step_name,
                                    error_code,
                                    setup_started,
                                );
                            }
                            Err(StepExecutionError::Unhandled { message }) => StepResult::failed(
                                spec.name,
                                Vec::new(),
                                (context.now)(),
                                StepError {
                                    code: ErrorCode::SetupUnhandledException,
                                    message,
                                    details: String::new(),
                                    exit_code: 1,
                                },
                            ),
                        },
                        Err(StepExecutionError::Unhandled { message }) => StepResult::failed(
                            spec.name,
                            Vec::new(),
                            (context.now)(),
                            StepError {
                                code: ErrorCode::SetupUnhandledException,
                                message,
                                details: String::new(),
                                exit_code: 1,
                            },
                        ),
                        Err(StepExecutionError::DeadEnd {
                            message,
                            exit_code,
                            step_name,
                            error_code,
                        }) => {
                            return dead_end_outcome(
                                exit_code,
                                ran_steps,
                                message,
                                step_name,
                                error_code,
                                setup_started,
                            );
                        }
                    }
                } else {
                    let mut skipped =
                        StepResult::new(spec.name, StepStatus::Skipped, paths, (context.now)());
                    skipped.reason = Some(SkipReason::PriorRunOk.as_str().to_owned());
                    skipped
                }
            } else {
                match spec.executor.expect("only implemented steps are runnable")(context) {
                    Ok(result) => result,
                    Err(StepExecutionError::DeadEnd {
                        message,
                        exit_code,
                        step_name,
                        error_code,
                    }) => {
                        return dead_end_outcome(
                            exit_code,
                            ran_steps,
                            message,
                            step_name,
                            error_code,
                            setup_started,
                        );
                    }
                    Err(StepExecutionError::Unhandled { message }) => StepResult::failed(
                        spec.name,
                        Vec::new(),
                        (context.now)(),
                        StepError {
                            code: ErrorCode::SetupUnhandledException,
                            message,
                            details: String::new(),
                            exit_code: 1,
                        },
                    ),
                }
            }
        } else {
            match spec.executor.expect("only implemented steps are runnable")(context) {
                Ok(result) => result,
                Err(StepExecutionError::DeadEnd {
                    message,
                    exit_code,
                    step_name,
                    error_code,
                }) => {
                    return dead_end_outcome(
                        exit_code,
                        ran_steps,
                        message,
                        step_name,
                        error_code,
                        setup_started,
                    );
                }
                Err(StepExecutionError::Unhandled { message }) => StepResult::failed(
                    spec.name,
                    Vec::new(),
                    (context.now)(),
                    StepError {
                        code: ErrorCode::SetupUnhandledException,
                        message,
                        details: String::new(),
                        exit_code: 1,
                    },
                ),
            }
        };
        ran_steps.push(spec.name);
        manifest
            .steps
            .push(serde_json::to_value(&result).expect("step result is serializable"));
        write_manifest(&context.manifest_path, &manifest);
        emit_step_result(context, &result, step_started.elapsed().as_millis());
        narrate_step_result(context, index, &result);
        if result.status == StepStatus::Failed {
            if CONTINUE_AFTER_FAILURE.contains(&result.name) {
                aggregate.push(result);
                continue;
            }
            let exit_code = result
                .error
                .as_ref()
                .and_then(StepErrorPayload::failure_exit_code)
                .unwrap_or(1);
            context.emit(
                EventType::SetupCompleted,
                Map::from_iter([
                    ("status".into(), json!("failed")),
                    ("failed_step".into(), json!(result.name.as_str())),
                    (
                        "duration_ms".into(),
                        json!(setup_started.elapsed().as_millis()),
                    ),
                ]),
            );
            return RunOutcome {
                exit_code,
                ran_steps,
                dead_end: None,
                duration_ms: setup_started.elapsed().as_millis(),
            };
        }
    }
    if let Some(first) = aggregate.first() {
        let exit_code = aggregate
            .iter()
            .filter_map(|result| {
                result
                    .error
                    .as_ref()
                    .and_then(StepErrorPayload::failure_exit_code)
            })
            .max()
            .unwrap_or(1);
        context.emit(
            EventType::SetupCompleted,
            Map::from_iter([
                ("status".into(), json!("failed")),
                ("failed_step".into(), json!(first.name.as_str())),
                (
                    "duration_ms".into(),
                    json!(setup_started.elapsed().as_millis()),
                ),
            ]),
        );
        return RunOutcome {
            exit_code,
            ran_steps,
            dead_end: None,
            duration_ms: setup_started.elapsed().as_millis(),
        };
    }
    manifest.completed_at = Some((context.now)());
    write_manifest(&context.manifest_path, &manifest);
    context.emit(
        EventType::SetupCompleted,
        Map::from_iter([
            ("status".into(), json!("ok")),
            (
                "duration_ms".into(),
                json!(setup_started.elapsed().as_millis()),
            ),
        ]),
    );
    narrate_success(context, &manifest);
    RunOutcome {
        exit_code: 0,
        ran_steps,
        dead_end: None,
        duration_ms: setup_started.elapsed().as_millis(),
    }
}

fn dead_end_outcome(
    exit_code: i32,
    ran_steps: Vec<StepName>,
    message: String,
    step_name: Option<StepName>,
    error_code: Option<ErrorCode>,
    setup_started: Instant,
) -> RunOutcome {
    RunOutcome {
        exit_code,
        ran_steps,
        dead_end: Some(DeadEndOutcome {
            message,
            step_name,
            error_code,
        }),
        duration_ms: setup_started.elapsed().as_millis(),
    }
}

fn narrate(context: &SetupContext<'_>, line: &str) {
    if !context.jsonl() {
        println!("{line}");
    }
}

fn narrate_error(context: &SetupContext<'_>, line: &str) {
    if !context.jsonl() {
        eprintln!("{line}");
    }
}

fn narrate_prior_run(context: &SetupContext<'_>, previous: Option<&SetupManifest>) {
    let Some(previous) = previous else {
        return;
    };
    if previous.completed_at.is_some() {
        let suffix = if context.args.force {
            "re-running all steps (--force)."
        } else {
            "verifying current state."
        };
        narrate(
            context,
            &format!(
                "journal setup last ran cleanly on {}; {suffix}",
                previous.completed_at.as_deref().unwrap_or_default()
            ),
        );
        if !context.args.force {
            narrate(context, "Use --force to re-run all steps unconditionally.");
        }
        return;
    }
    let failed = previous
        .steps
        .iter()
        .filter_map(Value::as_object)
        .filter(|step| step.get("status").and_then(Value::as_str) == Some("failed"))
        .filter_map(|step| step.get("name").and_then(Value::as_str))
        .collect::<Vec<_>>();
    if failed.is_empty() {
        return;
    }
    narrate(
        context,
        &format!(
            "journal setup last run on {} left these steps incomplete:",
            previous.started_at
        ),
    );
    for name in failed {
        narrate(context, &format!("  - {name} (failed)"));
    }
    narrate(
        context,
        "Re-running will verify state and re-run incomplete steps.",
    );
}

fn narrate_step_result(context: &SetupContext<'_>, index: usize, result: &StepResult) {
    if result.status == StepStatus::Skipped {
        narrate(
            context,
            &format!(
                "[step {index}/{}] skipped {}: {}",
                ALL_STEP_NAMES.len(),
                result.name.as_str(),
                result.reason.as_deref().unwrap_or("skipped")
            ),
        );
    } else if result.status == StepStatus::Failed {
        let message = match result.error.as_ref() {
            Some(StepErrorPayload::Failure(error)) => error.message.as_str(),
            _ => "step failed",
        };
        narrate_error(
            context,
            &format!("journal setup: {} failed: {message}", result.name.as_str()),
        );
    }
}

fn narrate_success(context: &SetupContext<'_>, manifest: &SetupManifest) {
    if context.jsonl() {
        return;
    }
    println!();
    println!("solstone is set up.");
    println!();
    let skipped_prior = manifest
        .steps
        .iter()
        .filter_map(Value::as_object)
        .filter(|step| step.get("reason").and_then(Value::as_str) == Some("prior_run_ok"))
        .count();
    let skipped_other = manifest
        .steps
        .iter()
        .filter_map(Value::as_object)
        .filter(|step| {
            step.get("status").and_then(Value::as_str) == Some("skipped")
                && step.get("reason").and_then(Value::as_str) != Some("prior_run_ok")
        })
        .count();
    println!(
        "{skipped_prior} of {} steps already done; ran {}",
        ALL_STEP_NAMES.len(),
        ALL_STEP_NAMES
            .len()
            .saturating_sub(skipped_prior + skipped_other)
    );
    println!();
    println!("artifacts:");
    let mut paths = Vec::new();
    for step in &manifest.steps {
        if let Some(items) = step.get("paths").and_then(Value::as_array) {
            for path in items.iter().filter_map(Value::as_str) {
                if !paths.iter().any(|seen: &String| seen == path) {
                    paths.push(path.to_owned());
                }
            }
        }
    }
    let manifest_path = context.manifest_path.to_string_lossy().into_owned();
    if !paths.contains(&manifest_path) {
        paths.push(manifest_path);
    }
    if paths.is_empty() {
        println!("  none");
    } else {
        for path in paths {
            println!("  {path}");
        }
    }
}

fn command_for_step(context: &SetupContext<'_>, name: StepName) -> Option<Vec<String>> {
    let journal = |args: Vec<String>| {
        std::iter::once(
            context
                .install_bin_dir
                .join("journal")
                .to_string_lossy()
                .into_owned(),
        )
        .chain(args)
        .collect::<Vec<_>>()
    };
    let sol = |args: Vec<String>| {
        std::iter::once(
            context
                .install_bin_dir
                .join("solstone")
                .to_string_lossy()
                .into_owned(),
        )
        .chain(args)
        .collect::<Vec<_>>()
    };
    match name {
        StepName::Doctor => Some(journal(vec![
            "doctor".into(),
            "--readiness".into(),
            if context.jsonl() { "--jsonl" } else { "--json" }.into(),
            "--port".into(),
            context.args.port.to_string(),
        ])),
        StepName::InstallModels => Some(journal(vec![
            "install-models".into(),
            "--variant".into(),
            context.args.variant.clone(),
        ])),
        StepName::SkillsUser => Some(sol(vec![
            "skills".into(),
            "install".into(),
            "--agent".into(),
            "all".into(),
        ])),
        StepName::SkillsJournal => Some(sol(vec![
            "skills".into(),
            "install".into(),
            "--project".into(),
            context.journal_path.to_string_lossy().into_owned(),
            "--agent".into(),
            "all".into(),
        ])),
        StepName::Service if !context.sol_already_keeps_journal() || context.args.skip_service => {
            Some(journal(vec![
                "service".into(),
                "install".into(),
                "--port".into(),
                context.args.port.to_string(),
            ]))
        }
        StepName::Brain => Some(sol(vec![
            "call".into(),
            "thinking".into(),
            "local".into(),
            "bootstrap".into(),
        ])),
        StepName::Journal | StepName::Wrapper | StepName::Service => None,
    }
}

fn emit_step_result(context: &mut SetupContext<'_>, result: &StepResult, duration_ms: u128) {
    let outcome = if result.status == StepStatus::Skipped {
        "skipped"
    } else {
        "ok"
    };
    match result.status {
        StepStatus::Failed => {
            let error = result.error.as_ref().expect("failed result has an error");
            context.emit(
                EventType::StepFailed,
                Map::from_iter([
                    ("step".into(), json!(result.name.as_str())),
                    ("duration_ms".into(), json!(duration_ms)),
                    (
                        "error".into(),
                        serde_json::to_value(error).expect("error serializable"),
                    ),
                ]),
            );
        }
        _ => {
            let mut fields = Map::from_iter([
                ("step".into(), json!(result.name.as_str())),
                ("outcome".into(), json!(outcome)),
                ("duration_ms".into(), json!(duration_ms)),
            ]);
            if let Some(reason) = &result.reason {
                fields.insert("reason".into(), json!(reason));
            }
            context.emit(EventType::StepCompleted, fields);
            if result.status == StepStatus::Warning {
                let (text, fix_hint) = match result.error.as_ref() {
                    Some(StepErrorPayload::WrapperWarning(warning)) => {
                        (warning.message.as_str(), warning.fix_hint.as_str())
                    }
                    Some(StepErrorPayload::BrainWarning(warning)) => {
                        (warning.message.as_str(), warning.fix_hint.as_str())
                    }
                    _ => ("step warning", ""),
                };
                context.emit(
                    EventType::StepWarning,
                    Map::from_iter([
                        ("step".into(), json!(result.name.as_str())),
                        (
                            "text".into(),
                            json!(text.chars().take(512).collect::<String>()),
                        ),
                        ("fix_hint".into(), json!(fix_hint)),
                    ]),
                );
            }
        }
    }
}

fn step_doctor(context: &mut SetupContext<'_>) -> Result<StepResult, StepExecutionError> {
    let request = CommandRequest {
        program: context.install_bin_dir.join("journal"),
        args: vec![
            "doctor".into(),
            "--readiness".into(),
            if context.jsonl() {
                "--jsonl".into()
            } else {
                "--json".into()
            },
            "--port".into(),
            context.args.port.to_string(),
        ],
        timeout_seconds: Some(DOCTOR_TIMEOUT_SECONDS),
    };
    let output = context
        .runner
        .run(&request)
        .map_err(|error| StepExecutionError::Unhandled { message: error })?;
    let now = (context.now)();
    if output.timed_out {
        return Ok(StepResult::failed(
            StepName::Doctor,
            Vec::new(),
            now,
            StepError {
                code: ErrorCode::DoctorTimeout,
                message: format!("doctor timed out after {DOCTOR_TIMEOUT_SECONDS}s"),
                details: output.stderr,
                exit_code: 1,
            },
        ));
    }
    if context.jsonl() {
        let mut completed = None;
        let mut advisories = Vec::new();
        for line in output.stdout.lines() {
            let stripped = line.trim();
            if stripped.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(stripped) else {
                emit_doctor_warning(context, stripped, "");
                continue;
            };
            let Some(event) = value.get("event").and_then(Value::as_str) else {
                emit_doctor_warning(context, stripped, "");
                continue;
            };
            if !matches!(
                event,
                "doctor.started" | "check.completed" | "doctor.completed"
            ) {
                emit_doctor_warning(context, stripped, "");
                continue;
            }
            context.forward_line(line);
            if event == "doctor.completed" {
                completed = Some(value);
            } else if event == "check.completed"
                && value.get("severity").and_then(Value::as_str) == Some("advisory")
                && matches!(
                    value.get("status").and_then(Value::as_str),
                    Some("warning" | "failed")
                )
            {
                let text = value
                    .get("detail")
                    .and_then(Value::as_str)
                    .or_else(|| value.get("name").and_then(Value::as_str))
                    .unwrap_or("")
                    .to_owned();
                let fix_hint = value
                    .get("fix")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                advisories.push((text, fix_hint));
            }
        }
        let Some(completed) = completed else {
            return Ok(StepResult::failed(
                StepName::Doctor,
                Vec::new(),
                now,
                StepError {
                    code: ErrorCode::DoctorJsonlIncomplete,
                    message: "doctor JSONL stream ended without doctor.completed".into(),
                    details: output.stderr,
                    exit_code: output.exit_code.max(1),
                },
            ));
        };
        if completed.get("status").and_then(Value::as_str) == Some("failed") {
            return Ok(StepResult::failed(
                StepName::Doctor,
                Vec::new(),
                now,
                StepError {
                    code: ErrorCode::DoctorFailed,
                    message: "doctor completed with status failed".into(),
                    details: output.stderr,
                    exit_code: output.exit_code.max(1),
                },
            ));
        }
        for (text, fix_hint) in advisories {
            emit_doctor_warning(context, &text, &fix_hint);
        }
        return Ok(StepResult::new(
            StepName::Doctor,
            StepStatus::Ok,
            Vec::new(),
            now,
        ));
    }
    if output.exit_code != 0 {
        return Ok(StepResult::failed(
            StepName::Doctor,
            Vec::new(),
            now,
            StepError {
                code: ErrorCode::DoctorFailed,
                message: "doctor blocker failed".into(),
                details: tail(&non_empty_output(&output)),
                exit_code: output.exit_code,
            },
        ));
    }
    if serde_json::from_str::<Value>(&output.stdout).is_err() {
        return Ok(StepResult::failed(
            StepName::Doctor,
            Vec::new(),
            now,
            StepError {
                code: ErrorCode::DoctorJsonlIncomplete,
                message: "doctor JSON parse failed".into(),
                details: tail(&output.stdout),
                exit_code: 1,
            },
        ));
    }
    Ok(StepResult::new(
        StepName::Doctor,
        StepStatus::Ok,
        Vec::new(),
        now,
    ))
}

fn emit_doctor_warning(context: &mut SetupContext<'_>, text: &str, fix_hint: &str) {
    context.emit(
        EventType::StepWarning,
        Map::from_iter([
            ("step".into(), json!(StepName::Doctor.as_str())),
            (
                "text".into(),
                json!(text.chars().take(512).collect::<String>()),
            ),
            ("fix_hint".into(), json!(fix_hint)),
        ]),
    );
}

fn step_journal(context: &mut SetupContext<'_>) -> Result<StepResult, StepExecutionError> {
    if context.journal_path.exists() && !context.journal_path.is_dir() {
        return Err(StepExecutionError::DeadEnd {
            message: format!(
                "expected a directory at {}; got a regular file. Re-run with --journal <other-path>.",
                context.journal_path.display()
            ),
            exit_code: 2,
            step_name: Some(StepName::Journal),
            error_code: Some(ErrorCode::JournalDirInvalid),
        });
    }
    let persisted = read_user_config(&context.config_path)
        .get("journal")
        .cloned()
        .unwrap_or_default();
    let persisted = persisted.trim();
    let persisted_matches = !persisted.is_empty()
        && paths_match(
            persisted,
            &context.journal_path,
            &context.home_dir,
            &context.current_dir,
        );
    let existing_journal = non_empty_journal(&context.journal_path);
    if existing_journal && !context.args.accept_existing_journal && !persisted_matches {
        if context.mode == SetupMode::NonInteractive {
            return Err(StepExecutionError::DeadEnd {
                message: existing_journal_message(&context.journal_path),
                exit_code: 2,
                step_name: Some(StepName::Journal),
                error_code: Some(ErrorCode::JournalExistingBlocked),
            });
        }
        let accepted = context
            .prompt
            .accept_existing_journal(&context.journal_path)
            .map_err(|message| StepExecutionError::Unhandled { message })?;
        if !accepted {
            return Err(StepExecutionError::DeadEnd {
                message: "setup aborted by user".into(),
                exit_code: 2,
                step_name: None,
                error_code: None,
            });
        }
    }
    fs::create_dir_all(&context.journal_path).map_err(|error| StepExecutionError::Unhandled {
        message: error.to_string(),
    })?;
    if !persisted_matches {
        write_user_config(
            &context.config_path,
            &context.journal_path.to_string_lossy(),
        )
        .map_err(|error| StepExecutionError::Unhandled {
            message: error.to_string(),
        })?;
    }
    ensure_journal_config(&context.journal_path)
        .map_err(|error| StepExecutionError::Unhandled { message: error })?;
    if existing_journal {
        cleanup_legacy_log_aliases(&context.journal_path).map_err(|error| {
            StepExecutionError::Unhandled {
                message: format!("legacy log alias cleanup failed: {error}"),
            }
        })?;
    }
    Ok(StepResult::new(
        StepName::Journal,
        StepStatus::Ok,
        vec![
            context.config_path.clone(),
            context.journal_path.clone(),
            get_journal_config_path(&context.journal_path),
        ],
        (context.now)(),
    ))
}

fn step_install_models(context: &mut SetupContext<'_>) -> Result<StepResult, StepExecutionError> {
    if context.args.skip_models {
        let mut result = StepResult::new(
            StepName::InstallModels,
            StepStatus::Skipped,
            Vec::new(),
            (context.now)(),
        );
        result.reason = Some(SkipReason::SkipModels.as_str().to_owned());
        return Ok(result);
    }
    let paths = model_paths(context);
    let output = context
        .runner
        .run(&CommandRequest {
            program: context.install_bin_dir.join("journal"),
            args: vec![
                "install-models".into(),
                "--variant".into(),
                context.args.variant.clone(),
            ],
            timeout_seconds: Some(context.args.step_timeout_seconds.max(0) as u64),
        })
        .map_err(|message| StepExecutionError::Unhandled { message })?;
    let now = (context.now)();
    if output.timed_out || output.exit_code != 0 {
        return Ok(subprocess_failure(
            StepName::InstallModels,
            paths,
            now,
            output,
            context.args.step_timeout_seconds,
        ));
    }
    Ok(StepResult::new(
        StepName::InstallModels,
        StepStatus::Ok,
        paths,
        now,
    ))
}

fn step_skills_user(context: &mut SetupContext<'_>) -> Result<StepResult, StepExecutionError> {
    let paths = vec![
        context.home_dir.join(".claude/skills/solstone/SKILL.md"),
        context.home_dir.join(".codex/skills/solstone/SKILL.md"),
        context.home_dir.join(".gemini/skills/solstone/SKILL.md"),
    ];
    run_skill_step(
        context,
        StepName::SkillsUser,
        paths,
        vec![
            "skills".into(),
            "install".into(),
            "--agent".into(),
            "all".into(),
        ],
    )
}

fn step_skills_journal(context: &mut SetupContext<'_>) -> Result<StepResult, StepExecutionError> {
    let paths = vec![
        context.journal_path.join(".claude/skills"),
        context.journal_path.join(".agents/skills"),
    ];
    run_skill_step(
        context,
        StepName::SkillsJournal,
        paths,
        vec![
            "skills".into(),
            "install".into(),
            "--project".into(),
            context.journal_path.to_string_lossy().into_owned(),
            "--agent".into(),
            "all".into(),
        ],
    )
}

fn step_wrapper(context: &mut SetupContext<'_>) -> Result<StepResult, StepExecutionError> {
    #[cfg(windows)]
    {
        let mut result = StepResult::new(
            StepName::Wrapper,
            StepStatus::Skipped,
            Vec::new(),
            (context.now)(),
        );
        result.reason = Some(SkipReason::WindowsPackageOwnsCommands.as_str().to_owned());
        return Ok(result);
    }
    #[cfg(not(windows))]
    {
        if context.args.skip_wrapper {
            let mut result = StepResult::new(
                StepName::Wrapper,
                StepStatus::Skipped,
                Vec::new(),
                (context.now)(),
            );
            result.reason = Some(SkipReason::SkipWrapper.as_str().to_owned());
            return Ok(result);
        }
        let environment = WrapperEnvironment {
            home_dir: context.home_dir.clone(),
            curdir: context.project_root.clone(),
            executable_dir: context.install_bin_dir.clone(),
            backup_dir: context.wrapper_backup_dir.clone(),
            legacy_replacement: context.legacy_replacement,
        };
        let paths = wrapper_paths(&context.home_dir);
        match provision_wrappers(
            &environment,
            &context.journal_path,
            context
                .installation_admission
                .as_ref()
                .expect("setup identity admission precedes mutating wrapper step")
                .binding(),
        ) {
            Ok(_) => {
                narrate(context, &ensure_user_bin_on_path(&context.home_dir));
                Ok(StepResult::new(
                    StepName::Wrapper,
                    StepStatus::Ok,
                    vec![paths.solstone, paths.journal],
                    (context.now)(),
                ))
            }
            Err(error) => Ok(StepResult::failed(
                StepName::Wrapper,
                Vec::new(),
                (context.now)(),
                StepError {
                    code: ErrorCode::WrapperProvisionFailed,
                    message: format!(
                        "could not provision the solstone/journal wrappers at {} ({}: {error})",
                        context.home_dir.join(".local/bin").display(),
                        std::any::type_name::<WrapperError>()
                            .rsplit("::")
                            .next()
                            .unwrap_or("WrapperError")
                    ),
                    details: "fix permissions on ~/.local/bin and re-run `journal setup`, or invoke solstone/journal directly from the runtime".into(),
                    exit_code: 1,
                },
            )),
        }
    }
}

/// The native service artifact shared by setup and clean-uninstall.
#[must_use]
pub(crate) fn service_artifact_path(home: &Path) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        Some(home.join("Library/LaunchAgents/org.solpbc.solstone.plist"))
    } else if cfg!(target_os = "linux") {
        Some(home.join(".config/systemd/user/solstone.service"))
    } else {
        None
    }
}

fn skipped_result(
    name: StepName,
    paths: Vec<PathBuf>,
    now: String,
    reason: SkipReason,
) -> StepResult {
    let mut result = StepResult::new(name, StepStatus::Skipped, paths, now);
    result.reason = Some(reason.as_str().to_owned());
    result
}

fn step_service(context: &mut SetupContext<'_>) -> Result<StepResult, StepExecutionError> {
    if context.args.skip_service {
        return Ok(skipped_result(
            StepName::Service,
            Vec::new(),
            (context.now)(),
            SkipReason::SkipService,
        ));
    }
    if context.sol_already_keeps_journal() {
        narrate(context, SOL_ALREADY_KEEPS_JOURNAL_NARRATION);
        return Ok(skipped_result(
            StepName::Service,
            Vec::new(),
            (context.now)(),
            SkipReason::SolAlreadyKeepsJournal,
        ));
    }
    let paths = service_artifact_path(&context.home_dir)
        .into_iter()
        .collect::<Vec<_>>();
    let guard = GuardFields::from_binding(
        context
            .installation_admission
            .as_ref()
            .expect("setup admission is retained through service provisioning")
            .binding(),
    );
    let service_guard = service_guard_environment(&guard);
    let output = context
        .runner
        .run(&CommandRequest {
            program: context.install_bin_dir.join("journal"),
            args: vec![
                "service".into(),
                "install".into(),
                "--port".into(),
                context.args.port.to_string(),
                "--installation-namespace".into(),
                service_guard["SOLSTONE_INSTALLATION_NAMESPACE"].clone(),
                "--installation-id".into(),
                service_guard["SOLSTONE_INSTALLATION_ID"].clone(),
                "--installation-generation".into(),
                service_guard["SOLSTONE_INSTALLATION_GENERATION"].clone(),
                "--installation-journal-token".into(),
                service_guard["SOLSTONE_INSTALLATION_JOURNAL_TOKEN"].clone(),
            ],
            timeout_seconds: None,
        })
        .map_err(|message| StepExecutionError::Unhandled { message })?;
    if output.exit_code != 0 {
        return Ok(subprocess_failure(
            StepName::Service,
            paths,
            (context.now)(),
            output,
            context.args.step_timeout_seconds,
        ));
    }
    // The Type=notify supervisor verifies the installed binding before it
    // announces readiness. Release setup's exclusive identity lease after the
    // guarded unit is published so the new process can perform that read.
    drop(context.installation_admission.take());
    let up = context
        .service_ops
        .up(context.runner, &context.journal_path)
        .map_err(|message| StepExecutionError::Unhandled { message })?;
    if up != 0 {
        return Ok(StepResult::failed(
            StepName::Service,
            paths,
            (context.now)(),
            StepError {
                code: ErrorCode::ServiceUpFailed,
                message: format!("service up failed (exit {up})"),
                details: String::new(),
                exit_code: 1,
            },
        ));
    }
    Ok(StepResult::new(
        StepName::Service,
        StepStatus::Ok,
        paths,
        (context.now)(),
    ))
}

/// Returns `Ok(None)` for the two paths that deliberately rerun service installation.
fn resume_service(
    context: &mut SetupContext<'_>,
    paths: Vec<PathBuf>,
) -> Result<Option<StepResult>, StepExecutionError> {
    if context.sol_already_keeps_journal() && !context.args.skip_service {
        narrate(context, SOL_ALREADY_KEEPS_JOURNAL_NARRATION);
        return Ok(Some(skipped_result(
            StepName::Service,
            Vec::new(),
            (context.now)(),
            SkipReason::SolAlreadyKeepsJournal,
        )));
    }
    let installed = context
        .service_ops
        .is_installed(context.runner, &context.journal_path)
        .map_err(|message| StepExecutionError::Unhandled { message })?;
    if !installed {
        return Ok(None);
    }
    let healthy = context
        .service_ops
        .health_check(context.runner, &context.journal_path)
        .map_err(|message| StepExecutionError::Unhandled { message })?;
    if healthy {
        return Ok(Some(skipped_result(
            StepName::Service,
            paths,
            (context.now)(),
            SkipReason::PriorRunOk,
        )));
    }
    // Re-publish the guarded unit before starting an unhealthy runtime. A
    // direct restart here would make the child wait on setup's identity lease.
    Ok(None)
}

fn brain_skip_reason(context: &SetupContext<'_>) -> SkipReason {
    if context
        .resolved
        .args_resolved
        .get("skip_brain")
        .and_then(Value::as_object)
        .and_then(|entry| entry.get("source"))
        .and_then(Value::as_str)
        == Some("cli:--skip-models")
    {
        SkipReason::SkipModelsImpliesSkipBrain
    } else {
        SkipReason::SkipBrain
    }
}

fn step_brain(context: &mut SetupContext<'_>) -> Result<StepResult, StepExecutionError> {
    let mutation = mutate_journal_config(&context.journal_path, LockOptions::default(), |config| {
        let malformed = || JournalConfigMutation {
            changed: false,
            value: Some(SkipReason::ProviderConfigUnexpectedShape),
        };
        if config.contains_key("providers") && !config["providers"].is_object() {
            return malformed();
        }
        let providers = config
            .entry("providers")
            .or_insert_with(|| Value::Object(Map::new()));
        let Some(providers) = providers.as_object_mut() else {
            return malformed();
        };
        let active = providers.get("active").filter(|value| !value.is_null());
        if active.is_some_and(|value| !value.is_object()) {
            return malformed();
        }
        if active
            .and_then(|value| value.get("provider"))
            .and_then(Value::as_str)
            .is_some_and(|provider| provider != "local")
        {
            return JournalConfigMutation {
                changed: false,
                value: Some(SkipReason::ProviderAlreadyConfigured),
            };
        }
        let local = json!({"provider":"local","model":LOCAL_MODEL});
        let changed = active != Some(&local);
        if changed {
            providers.insert("active".into(), local);
        }
        JournalConfigMutation {
            changed,
            value: None,
        }
    })
    .map_err(|error| StepExecutionError::Unhandled {
        message: error.to_string(),
    })?;
    if let Some(reason) = mutation.value {
        return Ok(skipped_result(
            StepName::Brain,
            Vec::new(),
            (context.now)(),
            reason,
        ));
    }
    if context.args.skip_brain {
        return Ok(skipped_result(
            StepName::Brain,
            Vec::new(),
            (context.now)(),
            brain_skip_reason(context),
        ));
    }
    if context.args.skip_service {
        return Ok(skipped_result(
            StepName::Brain,
            Vec::new(),
            (context.now)(),
            SkipReason::SkipService,
        ));
    }
    if context.sol_already_keeps_journal() {
        return Ok(skipped_result(
            StepName::Brain,
            Vec::new(),
            (context.now)(),
            SkipReason::SolAlreadyKeepsJournal,
        ));
    }
    if context
        .check_report_builder
        .local_provider_blocked(&context.journal_path)
    {
        return Ok(skipped_result(
            StepName::Brain,
            Vec::new(),
            (context.now)(),
            SkipReason::LocalProviderUnavailable,
        ));
    }
    let output = context
        .runner
        .run(&CommandRequest {
            program: context.install_bin_dir.join("solstone"),
            args: vec![
                "call".into(),
                "thinking".into(),
                "local".into(),
                "bootstrap".into(),
            ],
            timeout_seconds: Some(context.args.step_timeout_seconds.max(0) as u64),
        })
        .map_err(|message| StepExecutionError::Unhandled { message })?;
    if output.exit_code == 0 && !output.timed_out {
        return Ok(StepResult::new(
            StepName::Brain,
            StepStatus::Ok,
            Vec::new(),
            (context.now)(),
        ));
    }
    let failure = subprocess_failure(
        StepName::Brain,
        Vec::new(),
        (context.now)(),
        output,
        context.args.step_timeout_seconds,
    );
    let StepErrorPayload::Failure(error) = failure.error.expect("subprocess failure payload")
    else {
        unreachable!()
    };
    Ok(StepResult::brain_warning(
        StepName::Brain,
        (context.now)(),
        BrainWarning {
            code: error.code,
            message: error.message,
            details: error.details,
            exit_code: error.exit_code,
            fix_hint: LOCAL_INSTALL_HINT.into(),
        },
    ))
}

fn run_skill_step(
    context: &mut SetupContext<'_>,
    name: StepName,
    paths: Vec<PathBuf>,
    args: Vec<String>,
) -> Result<StepResult, StepExecutionError> {
    if context.args.skip_skills {
        let mut result = StepResult::new(name, StepStatus::Skipped, Vec::new(), (context.now)());
        result.reason = Some(SkipReason::SkipSkills.as_str().to_owned());
        return Ok(result);
    }
    let output = context
        .runner
        .run(&CommandRequest {
            program: context.install_bin_dir.join("solstone"),
            args,
            timeout_seconds: Some(context.args.step_timeout_seconds.max(0) as u64),
        })
        .map_err(|message| StepExecutionError::Unhandled { message })?;
    let now = (context.now)();
    if output.timed_out || output.exit_code != 0 {
        return Ok(subprocess_failure(
            name,
            paths,
            now,
            output,
            context.args.step_timeout_seconds,
        ));
    }
    Ok(StepResult::new(name, StepStatus::Ok, paths, now))
}

fn ensure_journal_config(journal: &Path) -> Result<(), String> {
    mutate_journal_config(journal, LockOptions::default(), |_config| {
        JournalConfigMutation {
            changed: false,
            value: (),
        }
    })
    .map(|_| ())
    .map_err(|error| error.to_string())
}

fn subprocess_failure(
    name: StepName,
    paths: Vec<PathBuf>,
    now: String,
    output: CommandOutput,
    timeout: i64,
) -> StepResult {
    let details = non_empty_output(&output);
    let error = if output.timed_out {
        StepError {
            code: ErrorCode::StepSubprocessTimeout,
            message: format!("{} step timed out after {timeout}s", name.as_str()),
            details: tail(&details),
            exit_code: 1,
        }
    } else {
        StepError {
            code: ErrorCode::StepSubprocessFailed,
            message: first_line(&output.stderr).unwrap_or_else(|| {
                format!(
                    "{} step exited with code {}",
                    name.as_str(),
                    output.exit_code
                )
            }),
            details: tail(&details),
            exit_code: output.exit_code,
        }
    };
    StepResult::failed(name, paths, now, error)
}

fn model_paths(context: &SetupContext<'_>) -> Vec<PathBuf> {
    if cfg!(target_os = "linux") {
        return solstone_core_system::provider_runtime::parakeet_cpp_artifacts(
            &context.journal_path,
            "linux",
            std::env::consts::ARCH,
        )
        .map_or_else(
            |_| Vec::new(),
            |artifacts| {
                vec![
                    artifacts.binary_cpu,
                    artifacts.binary_vulkan,
                    artifacts.model,
                ]
            },
        );
    }
    if cfg!(target_os = "macos") {
        return vec![
            solstone_core_journal_config::parakeet_coreml::parakeet_coreml_sentinel_path(
                &context.home_dir,
            ),
        ];
    }
    Vec::new()
}
fn non_empty_output(output: &CommandOutput) -> String {
    if output.stderr.is_empty() {
        output.stdout.clone()
    } else {
        output.stderr.clone()
    }
}
fn first_line(value: &str) -> Option<String> {
    value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
}
fn tail(value: &str) -> String {
    const LIMIT: usize = 8192;
    if value.len() <= LIMIT {
        value.to_owned()
    } else {
        String::from_utf8_lossy(&value.as_bytes()[value.len() - LIMIT..]).into_owned()
    }
}
fn mode_name(mode: SetupMode) -> &'static str {
    match mode {
        SetupMode::Interactive => "interactive",
        SetupMode::NonInteractive => "non_interactive",
        SetupMode::DryRun => "dry_run",
        SetupMode::Explain => "explain",
    }
}
fn paths_match(configured: &str, journal: &Path, home: &Path, current_dir: &Path) -> bool {
    resolve_expanded_path(configured, home, current_dir) == journal
}

fn existing_journal_message(path: &Path) -> String {
    [
        format!("journal setup: cannot proceed in non-interactive mode - {} already contains journal data.", path.display()),
        "Setup will not auto-claim an existing journal.".into(),
        String::new(),
        "Retry with one of:".into(),
        "  journal setup --accept-existing-journal".into(),
        "  journal setup --journal /path/to/new-journal --accept-existing-journal".into(),
        String::new(),
        "Interactive escape:".into(),
        "  journal setup".into(),
        String::new(),
        "Run 'journal setup --explain' for full step list.".into(),
    ]
    .join("\n")
}
fn non_empty_journal(path: &Path) -> bool {
    path.is_dir()
        && (path.join("config").is_dir()
            || fs::read_dir(path).is_ok_and(|entries| {
                entries.flatten().any(|entry| {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    name.ends_with(".jsonl")
                        || (entry.path().is_dir()
                            && name.len() == 8
                            && name.chars().all(|character| character.is_ascii_digit()))
                })
            }))
}
fn plan_doctor(context: &SetupContext<'_>) -> String {
    format!(
        "would run: {} doctor --readiness",
        context.install_bin_dir.join("journal").display()
    )
}
fn plan_journal(context: &SetupContext<'_>) -> String {
    format!("would write: {}", context.config_path.display())
}
fn plan_install_models(context: &SetupContext<'_>) -> String {
    format!(
        "would run: {} install-models --variant {}",
        context.install_bin_dir.join("journal").display(),
        context.args.variant
    )
}
fn plan_skills_user(context: &SetupContext<'_>) -> String {
    format!(
        "would run: {} skills install --agent all",
        context.install_bin_dir.join("solstone").display()
    )
}
fn plan_skills_journal(context: &SetupContext<'_>) -> String {
    format!(
        "would run: {} skills install --project {} --agent all",
        context.install_bin_dir.join("solstone").display(),
        context.journal_path.display()
    )
}
fn plan_wrapper(_context: &SetupContext<'_>) -> String {
    if cfg!(windows) {
        "would skip POSIX wrapper provisioning; the Windows package exposes the commands directly"
            .into()
    } else {
        "would provision managed solstone and journal wrappers in-process".into()
    }
}
fn plan_service(_context: &SetupContext<'_>) -> String {
    "would install and start the journal service".into()
}
fn plan_brain(_context: &SetupContext<'_>) -> String {
    "would set the local provider lane and bootstrap it".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{ResolutionContext, parse_args_at, resolve_mode, resolve_setup};
    use crate::events::{EventSink, EventType};
    use crate::manifest::manifest_path;
    use crate::user_config::config_path;
    use solstone_core_installation_identity::{
        ArtifactBindingEvidence, LegacyManifestEvidence, OwnerBase, PlatformTag,
        SetupAdmissionRequest, admit_setup, journal_token_from_path, root_token_from_path,
    };
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::ffi::OsString;
    use std::io;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    struct FakeRunner {
        outputs: VecDeque<Result<CommandOutput, String>>,
        requests: Vec<CommandRequest>,
    }
    impl FakeRunner {
        fn new(outputs: Vec<Result<CommandOutput, String>>) -> Self {
            Self {
                outputs: outputs.into(),
                requests: Vec::new(),
            }
        }
    }
    impl CommandRunner for FakeRunner {
        fn run(&mut self, request: &CommandRequest) -> Result<CommandOutput, String> {
            self.requests.push(request.clone());
            self.outputs.pop_front().unwrap_or_else(|| {
                Ok(CommandOutput {
                    exit_code: 0,
                    stdout: "{}".into(),
                    stderr: String::new(),
                    timed_out: false,
                })
            })
        }
    }
    struct Prompt(bool);
    impl ExistingJournalPrompt for Prompt {
        fn accept_existing_journal(&mut self, _path: &Path) -> Result<bool, String> {
            Ok(self.0)
        }
    }
    struct NoopServiceOps;
    impl ServiceOps for NoopServiceOps {
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
    struct NoopCheck;
    impl CheckReportBuilder for NoopCheck {
        fn local_provider_blocked(&self, _journal: &Path) -> bool {
            false
        }
    }
    static NOOP_CHECK: NoopCheck = NoopCheck;
    fn no_probe(_context: &SetupContext<'_>) -> Result<bool, String> {
        Ok(false)
    }
    fn keeps_journal(_context: &SetupContext<'_>) -> Result<bool, String> {
        Ok(true)
    }
    struct SequenceServiceOps {
        installed: VecDeque<bool>,
        healthy: VecDeque<bool>,
        restarts: usize,
        up: i32,
    }
    impl ServiceOps for SequenceServiceOps {
        fn is_installed(
            &mut self,
            _runner: &mut dyn CommandRunner,
            _journal: &Path,
        ) -> Result<bool, String> {
            Ok(self.installed.pop_front().unwrap_or(false))
        }
        fn health_check(
            &mut self,
            _runner: &mut dyn CommandRunner,
            _journal: &Path,
        ) -> Result<bool, String> {
            Ok(self.healthy.pop_front().unwrap_or(false))
        }
        fn restart(
            &mut self,
            _runner: &mut dyn CommandRunner,
            _journal: &Path,
        ) -> Result<(), String> {
            self.restarts += 1;
            Ok(())
        }
        fn up(&mut self, _runner: &mut dyn CommandRunner, _journal: &Path) -> Result<i32, String> {
            Ok(self.up)
        }
    }
    struct FailingServiceOps;
    impl ServiceOps for FailingServiceOps {
        fn is_installed(
            &mut self,
            _runner: &mut dyn CommandRunner,
            _journal: &Path,
        ) -> Result<bool, String> {
            Err("status spawn failed".into())
        }
        fn health_check(
            &mut self,
            _runner: &mut dyn CommandRunner,
            _journal: &Path,
        ) -> Result<bool, String> {
            unreachable!()
        }
        fn restart(
            &mut self,
            _runner: &mut dyn CommandRunner,
            _journal: &Path,
        ) -> Result<(), String> {
            unreachable!()
        }
        fn up(&mut self, _runner: &mut dyn CommandRunner, _journal: &Path) -> Result<i32, String> {
            unreachable!()
        }
    }
    #[derive(Default)]
    struct Recorder(RefCell<Vec<String>>);
    impl EventSink for Recorder {
        fn emit(
            &mut self,
            event: EventType,
            _timestamp: &str,
            _fields: Map<String, Value>,
        ) -> io::Result<()> {
            self.0.borrow_mut().push(event.as_str().into());
            Ok(())
        }
        fn forward_line(&mut self, line: &str) -> io::Result<()> {
            self.0.borrow_mut().push(line.into());
            Ok(())
        }
    }
    #[derive(Default)]
    struct FieldRecorder(RefCell<Vec<(EventType, Map<String, Value>)>>);
    impl EventSink for FieldRecorder {
        fn emit(
            &mut self,
            event: EventType,
            _timestamp: &str,
            fields: Map<String, Value>,
        ) -> io::Result<()> {
            self.0.borrow_mut().push((event, fields));
            Ok(())
        }
        fn forward_line(&mut self, _line: &str) -> io::Result<()> {
            Ok(())
        }
    }
    fn now() -> String {
        "2026-01-01T00:00:00Z".into()
    }
    fn fixture(name: &str, argv: &[&str]) -> (SetupArgs, ResolvedSetup, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "solstone-core-setup-steps-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("home")).unwrap();
        fs::create_dir_all(root.join("cwd")).unwrap();
        let resolution = ResolutionContext {
            home_dir: root.join("home"),
            current_dir: root.join("cwd"),
            journal_env: Some(root.join("journal").to_string_lossy().into_owned()),
            journal_variant_env: None,
            is_source_checkout: false,
        };
        let args = parse_args_at(
            &argv.iter().map(OsString::from).collect::<Vec<_>>(),
            &resolution.current_dir,
        )
        .unwrap();
        let resolved = resolve_setup(&args, &resolution);
        (args, resolved, root, resolution.home_dir)
    }
    fn context<'a>(
        args: &'a SetupArgs,
        resolved: &'a ResolvedSetup,
        root: &Path,
        home: &Path,
        runner: &'a mut dyn CommandRunner,
        prompt: &'a mut dyn ExistingJournalPrompt,
        events: Option<&'a mut dyn EventSink>,
    ) -> SetupContext<'a> {
        SetupContext {
            args,
            resolved,
            mode: resolve_mode(args, false, false),
            home_dir: home.to_path_buf(),
            config_path: config_path(home),
            journal_path: resolved.journal_path.clone(),
            current_dir: root.join("cwd"),
            project_root: root.join("repo"),
            install_bin_dir: root.join("bin"),
            manifest_path: manifest_path(&resolved.journal_path),
            stdin_is_tty: false,
            stdout_is_tty: false,
            now,
            runner,
            prompt,
            events,
            wrapper_backup_dir: Some(root.join("wrapper-backups")),
            service_ops: Box::leak(Box::new(NoopServiceOps)),
            already_keeps_journal_probe: no_probe,
            is_macos: false,
            check_report_builder: &NOOP_CHECK,
            installation_admission: {
                let identity_root = root.join("repo");
                fs::create_dir_all(&identity_root).expect("create identity root");
                Some(
                    admit_setup(SetupAdmissionRequest {
                        owner: OwnerBase::at_home(home.to_path_buf(), PlatformTag::Linux)
                            .expect("owner base"),
                        root_token: root_token_from_path(&identity_root).expect("root token"),
                        journal_token: journal_token_from_path(&resolved.journal_path)
                            .expect("journal token"),
                        journal_is_explicit: true,
                        legacy_manifest: LegacyManifestEvidence::Absent,
                        artifacts: ArtifactBindingEvidence::Fresh,
                    })
                    .expect("test setup admission"),
                )
            },
            identity_guard_repair_steps: Vec::new(),
            legacy_replacement: false,
        }
    }
    #[test]
    fn short_circuit_emits_only_setup_start_and_completion() {
        let (args, resolved, root, home) = fixture("short", &["--jsonl", "--explain"]);
        let mut runner = FakeRunner::new(Vec::new());
        let mut prompt = Prompt(false);
        let mut recorder = FieldRecorder::default();
        let outcome = run_setup(
            &mut context(
                &args,
                &resolved,
                &root,
                &home,
                &mut runner,
                &mut prompt,
                Some(&mut recorder),
            ),
            &implemented_step_specs(),
        );
        assert_eq!(outcome.exit_code, 0);
        assert!(outcome.ran_steps.is_empty());
        let events = recorder.0.borrow();
        assert_eq!(
            events.iter().map(|(event, _)| *event).collect::<Vec<_>>(),
            [EventType::SetupStarted, EventType::SetupCompleted]
        );
        assert_eq!(events[0].1["version"], env!("CARGO_PKG_VERSION"));
        assert!(events[1].1["duration_ms"].is_u64());
    }

    #[test]
    fn jsonl_step_events_include_the_documented_command_and_duration() {
        let (args, resolved, root, home) = fixture("jsonl-metadata", &["--jsonl"]);
        let mut runner = FakeRunner::new(vec![Ok(CommandOutput {
            exit_code: 0,
            stdout: concat!(
                "{\"event\":\"doctor.started\"}\n",
                "{\"event\":\"check.completed\"}\n",
                "{\"event\":\"doctor.completed\",\"status\":\"ok\"}\n"
            )
            .into(),
            stderr: String::new(),
            timed_out: false,
        })]);
        let mut prompt = Prompt(false);
        let mut recorder = FieldRecorder::default();
        let doctor = [StepSpec {
            name: StepName::Doctor,
            executor: Some(step_doctor),
            plan: plan_doctor,
        }];
        run_setup(
            &mut context(
                &args,
                &resolved,
                &root,
                &home,
                &mut runner,
                &mut prompt,
                Some(&mut recorder),
            ),
            &doctor,
        );
        let events = recorder.0.borrow();
        let started = events
            .iter()
            .find(|(event, _)| *event == EventType::StepStarted)
            .unwrap();
        assert_eq!(started.1["step"], "doctor");
        assert_eq!(started.1["command"][1], "doctor");
        let completed = events
            .iter()
            .find(|(event, _)| *event == EventType::StepCompleted)
            .unwrap();
        assert!(completed.1["duration_ms"].is_u64());
    }
    #[test]
    fn table_has_the_fixed_eight_step_order_and_complete_plan_shape() {
        let specs = step_specs();
        assert_eq!(ALL_STEP_NAMES, specs.map(|spec| spec.name));
        assert_eq!(implemented_step_specs().len(), 8);
    }
    #[test]
    fn service_resume_covers_all_five_branches_and_fallthroughs_execute_install() {
        let service_spec = [StepSpec {
            name: StepName::Service,
            executor: Some(step_service),
            plan: plan_service,
        }];
        let run =
            |name: &str, args: &[&str], installed: Vec<bool>, healthy: Vec<bool>, keep: bool| {
                let (args, resolved, root, home) = fixture(name, args);
                let prior_path = root.join("prior");
                fs::write(&prior_path, "present").unwrap();
                let mut manifest =
                    SetupManifest::initial(now(), "non_interactive".into(), Map::new());
                manifest
                    .steps
                    .push(json!({"name":"service","status":"ok","paths":[prior_path]}));
                write_manifest(&manifest_path(&resolved.journal_path), &manifest);
                let mut runner = FakeRunner::new(vec![Ok(CommandOutput {
                    exit_code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                    timed_out: false,
                })]);
                let mut prompt = Prompt(false);
                let mut ops = SequenceServiceOps {
                    installed: installed.into(),
                    healthy: healthy.into(),
                    restarts: 0,
                    up: 0,
                };
                let mut setup = context(
                    &args,
                    &resolved,
                    &root,
                    &home,
                    &mut runner,
                    &mut prompt,
                    None,
                );
                setup.service_ops = &mut ops;
                setup.is_macos = keep;
                setup.already_keeps_journal_probe = keeps_journal;
                let outcome = run_setup(&mut setup, &service_spec);
                (outcome, runner.requests.len(), ops.restarts)
            };
        let (outcome, requests, _) = run("service-solstone", &[], vec![], vec![], true);
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(requests, 0);
        let (outcome, requests, _) = run("service-missing", &[], vec![false], vec![], false);
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(requests, 1);
        let (outcome, requests, _) = run("service-healthy", &[], vec![true], vec![true], false);
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(requests, 0);
        let (outcome, requests, restarts) = run(
            "service-restarted",
            &[],
            vec![true],
            vec![false, true],
            false,
        );
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(requests, 1);
        assert_eq!(restarts, 0);
        let (outcome, requests, restarts) = run(
            "service-still-bad",
            &[],
            vec![true],
            vec![false, false],
            false,
        );
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(requests, 1);
        assert_eq!(restarts, 0);
        let (outcome, requests, _) =
            run("service-force", &["--force"], vec![true], vec![true], false);
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(requests, 1);
    }
    #[test]
    fn service_resume_predicate_error_becomes_an_unhandled_failure() {
        let (args, resolved, root, home) = fixture("service-resume-error", &[]);
        let path = root.join("prior");
        fs::write(&path, "present").unwrap();
        let mut manifest = SetupManifest::initial(now(), "non_interactive".into(), Map::new());
        manifest
            .steps
            .push(json!({"name":"service","status":"ok","paths":[path]}));
        write_manifest(&manifest_path(&resolved.journal_path), &manifest);
        let mut runner = FakeRunner::new(Vec::new());
        let mut prompt = Prompt(false);
        let mut ops = FailingServiceOps;
        let mut setup = context(
            &args,
            &resolved,
            &root,
            &home,
            &mut runner,
            &mut prompt,
            None,
        );
        setup.service_ops = &mut ops;
        let outcome = run_setup(
            &mut setup,
            &[StepSpec {
                name: StepName::Service,
                executor: Some(step_service),
                plan: plan_service,
            }],
        );
        assert_eq!(outcome.exit_code, 1);
        let written = read_manifest(&manifest_path(&resolved.journal_path)).unwrap();
        assert_eq!(
            written.steps[0]["error"]["code"],
            "setup_unhandled_exception"
        );
    }
    #[test]
    fn service_start_releases_the_setup_identity_lease_after_unit_publication() {
        let (args, resolved, root, home) = fixture("service-identity-release", &[]);
        let mut runner = FakeRunner::new(vec![Ok(CommandOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
        })]);
        let mut prompt = Prompt(false);
        let mut ops = SequenceServiceOps {
            installed: VecDeque::new(),
            healthy: VecDeque::new(),
            restarts: 0,
            up: 0,
        };
        let mut setup = context(
            &args,
            &resolved,
            &root,
            &home,
            &mut runner,
            &mut prompt,
            None,
        );
        setup.service_ops = &mut ops;

        let result = step_service(&mut setup).expect("service step");

        assert_eq!(result.status, StepStatus::Ok);
        assert!(setup.installation_admission.is_none());
        assert_eq!(runner.requests.len(), 1);
    }
    #[test]
    fn brain_mutates_local_provider_before_skipping_and_warning_keeps_its_distinct_shape() {
        let (args, resolved, root, home) = fixture("brain-skip", &["--skip-brain"]);
        let mut runner = FakeRunner::new(Vec::new());
        let mut prompt = Prompt(false);
        let skipped = step_brain(&mut context(
            &args,
            &resolved,
            &root,
            &home,
            &mut runner,
            &mut prompt,
            None,
        ))
        .unwrap();
        assert_eq!(skipped.status, StepStatus::Skipped);
        let config: Value = serde_json::from_slice(
            &fs::read(get_journal_config_path(&resolved.journal_path)).unwrap(),
        )
        .unwrap();
        assert_eq!(
            config["providers"]["active"],
            json!({"provider":"local","model":LOCAL_MODEL})
        );
        let (args, resolved, root, home) = fixture("brain-warning", &[]);
        let mut runner = FakeRunner::new(vec![Ok(CommandOutput {
            exit_code: 7,
            stdout: String::new(),
            stderr: "bootstrap failed\n".into(),
            timed_out: false,
        })]);
        let mut prompt = Prompt(false);
        let warning = step_brain(&mut context(
            &args,
            &resolved,
            &root,
            &home,
            &mut runner,
            &mut prompt,
            None,
        ))
        .unwrap();
        assert_eq!(warning.status, StepStatus::Warning);
        assert_eq!(
            warning.reason.as_deref(),
            Some(SkipReason::LocalBootstrapDidNotStart.as_str())
        );
        assert_eq!(
            serde_json::to_value(warning).unwrap()["error"],
            json!({"code":"step_subprocess_failed","message":"bootstrap failed","details":"bootstrap failed\n","exit_code":7,"fix_hint":LOCAL_INSTALL_HINT})
        );
    }
    #[test]
    fn brain_preserves_owner_provider_and_rejects_malformed_provider_shape() {
        for (name, config, reason) in [
            (
                "brain-owner",
                json!({"providers":{"active":{"provider":"remote"}}}),
                SkipReason::ProviderAlreadyConfigured,
            ),
            (
                "brain-malformed",
                json!({"providers":"bad"}),
                SkipReason::ProviderConfigUnexpectedShape,
            ),
        ] {
            let (args, resolved, root, home) = fixture(name, &[]);
            let config_path = get_journal_config_path(&resolved.journal_path);
            fs::create_dir_all(config_path.parent().unwrap()).unwrap();
            fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
            let mut runner = FakeRunner::new(Vec::new());
            let mut prompt = Prompt(false);
            let result = step_brain(&mut context(
                &args,
                &resolved,
                &root,
                &home,
                &mut runner,
                &mut prompt,
                None,
            ))
            .unwrap();
            assert_eq!(result.status, StepStatus::Skipped);
            assert_eq!(result.reason.as_deref(), Some(reason.as_str()));
            assert_eq!(
                serde_json::from_slice::<Value>(&fs::read(config_path).unwrap()).unwrap(),
                config
            );
        }
    }
    #[test]
    fn wrapper_shell_path_refusal_is_failed_and_worktree_still_provisions_path() {
        let (args, resolved, root, home) = fixture("wrapper-warning", &[]);
        fs::create_dir_all(root.join("repo")).unwrap();
        let mut runner = FakeRunner::new(Vec::new());
        let mut prompt = Prompt(false);
        let mut setup = context(
            &args,
            &resolved,
            &root,
            &home,
            &mut runner,
            &mut prompt,
            None,
        );
        setup.journal_path = root.join("bad$journal");
        let warning = step_wrapper(&mut setup).unwrap();
        assert_eq!(warning.status, StepStatus::Failed);
        assert_eq!(
            serde_json::to_value(&warning).unwrap()["error"],
            json!({
                "code": "wrapper_provision_failed",
                "message": format!(
                    "could not provision the solstone/journal wrappers at {} (WrapperError: journal path contains shell-active character '$': {:?})",
                    home.join(".local/bin").display(),
                    root.join("bad$journal").to_string_lossy(),
                ),
                "details": "fix permissions on ~/.local/bin and re-run `journal setup`, or invoke solstone/journal directly from the runtime",
                "exit_code": 1,
            })
        );
        fs::write(root.join("repo/.git"), "gitdir: x").unwrap();
        fs::write(home.join(".bashrc"), "# rc\n").unwrap();
        setup.journal_path = root.join("journal");
        let okay = step_wrapper(&mut setup).unwrap();
        assert_eq!(okay.status, StepStatus::Ok);
        assert!(!home.join(".local/bin/solstone").exists());
        assert!(
            fs::read_to_string(home.join(".bashrc"))
                .unwrap()
                .contains(".local/bin")
        );
    }

    /// A degraded optional asset must not cost the owner their whole journal.
    ///
    /// `InstallModels` runs third of eight. When it halted the run, a brand-new
    /// install ended with no wrappers, no unit and no service — observed on the
    /// founder's machine, where setup aborted with
    /// `install_models failed: … CED assets are unavailable` and only
    /// `--skip-models` got through. Setup must finish building the journal and
    /// still report the failure.
    #[test]
    fn install_models_failure_does_not_halt_setup_before_the_journal_is_usable() {
        // The steps that actually make a journal usable run after this one.
        let models = ALL_STEP_NAMES
            .iter()
            .position(|name| *name == StepName::InstallModels)
            .expect("install_models is a setup step");
        for later in [StepName::Wrapper, StepName::Service, StepName::Brain] {
            let index = ALL_STEP_NAMES
                .iter()
                .position(|name| *name == later)
                .expect("step is in the canonical order");
            assert!(
                index > models,
                "{} runs after install_models, so halting there strands the owner",
                later.as_str()
            );
        }

        let (args, resolved, root, home) = fixture("models-continue", &[]);
        let mut runner = FakeRunner::new(vec![
            Ok(CommandOutput {
                exit_code: 0,
                stdout: "{}".into(),
                stderr: String::new(),
                timed_out: false,
            }),
            Ok(CommandOutput {
                exit_code: 9,
                stdout: String::new(),
                stderr: "CED assets are unavailable".into(),
                timed_out: false,
            }),
            Ok(CommandOutput {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
            }),
            Ok(CommandOutput {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
            }),
        ]);
        let mut prompt = Prompt(false);
        let specs = [
            StepSpec {
                name: StepName::Doctor,
                executor: Some(step_doctor),
                plan: plan_doctor,
            },
            StepSpec {
                name: StepName::InstallModels,
                executor: Some(step_install_models),
                plan: plan_install_models,
            },
            StepSpec {
                name: StepName::SkillsUser,
                executor: Some(step_skills_user),
                plan: plan_skills_user,
            },
            StepSpec {
                name: StepName::SkillsJournal,
                executor: Some(step_skills_journal),
                plan: plan_skills_journal,
            },
        ];
        let outcome = run_setup(
            &mut context(
                &args,
                &resolved,
                &root,
                &home,
                &mut runner,
                &mut prompt,
                None,
            ),
            &specs,
        );
        assert_eq!(
            outcome.ran_steps,
            vec![
                StepName::Doctor,
                StepName::InstallModels,
                StepName::SkillsUser,
                StepName::SkillsJournal
            ],
            "setup must keep going after a failed install_models"
        );
        // ...and must still report the failure rather than claiming success.
        assert_eq!(outcome.exit_code, 9);
    }

    #[test]
    fn skills_failures_continue_and_single_failure_uses_its_exit_code() {
        let (args, resolved, root, home) = fixture("aggregate", &[]);
        let mut runner = FakeRunner::new(vec![
            Ok(CommandOutput {
                exit_code: 0,
                stdout: "{}".into(),
                stderr: String::new(),
                timed_out: false,
            }),
            Ok(CommandOutput {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
            }),
            Ok(CommandOutput {
                exit_code: 7,
                stdout: String::new(),
                stderr: "user failed".into(),
                timed_out: false,
            }),
            Ok(CommandOutput {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
            }),
        ]);
        let mut prompt = Prompt(false);
        let specs = [
            StepSpec {
                name: StepName::Doctor,
                executor: Some(step_doctor),
                plan: plan_doctor,
            },
            StepSpec {
                name: StepName::InstallModels,
                executor: Some(step_install_models),
                plan: plan_install_models,
            },
            StepSpec {
                name: StepName::SkillsUser,
                executor: Some(step_skills_user),
                plan: plan_skills_user,
            },
            StepSpec {
                name: StepName::SkillsJournal,
                executor: Some(step_skills_journal),
                plan: plan_skills_journal,
            },
        ];
        let outcome = run_setup(
            &mut context(
                &args,
                &resolved,
                &root,
                &home,
                &mut runner,
                &mut prompt,
                None,
            ),
            &specs,
        );
        assert_eq!(outcome.exit_code, 7);
        assert_eq!(
            outcome.ran_steps,
            vec![
                StepName::Doctor,
                StepName::InstallModels,
                StepName::SkillsUser,
                StepName::SkillsJournal
            ]
        );
    }
    #[test]
    fn force_reruns_prior_ok_step() {
        let (args, resolved, root, home) = fixture("force", &["--force"]);
        fs::create_dir_all(&resolved.journal_path).unwrap();
        fs::write(resolved.journal_path.join("present"), "x").unwrap();
        let mut old = SetupManifest::initial(now(), "non_interactive".into(), Map::new());
        old.steps.push(
            json!({"name":"doctor","status":"ok","paths":[resolved.journal_path.join("present")]}),
        );
        write_manifest(&manifest_path(&resolved.journal_path), &old);
        let mut runner = FakeRunner::new(vec![Ok(CommandOutput {
            exit_code: 0,
            stdout: "{}".into(),
            stderr: String::new(),
            timed_out: false,
        })]);
        let mut prompt = Prompt(false);
        let specs = [StepSpec {
            name: StepName::Doctor,
            executor: Some(step_doctor),
            plan: plan_doctor,
        }];
        let outcome = run_setup(
            &mut context(
                &args,
                &resolved,
                &root,
                &home,
                &mut runner,
                &mut prompt,
                None,
            ),
            &specs,
        );
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(runner.requests.len(), 1);
    }

    #[test]
    fn prior_ok_step_is_skipped_without_force() {
        let (args, resolved, root, home) = fixture("skip", &[]);
        fs::create_dir_all(&resolved.journal_path).unwrap();
        fs::write(resolved.journal_path.join("present"), "x").unwrap();
        let mut old = SetupManifest::initial(now(), "non_interactive".into(), Map::new());
        old.steps.push(
            json!({"name":"doctor","status":"ok","paths":[resolved.journal_path.join("present")]}),
        );
        write_manifest(&manifest_path(&resolved.journal_path), &old);
        let mut runner = FakeRunner::new(Vec::new());
        let mut prompt = Prompt(false);
        let specs = [StepSpec {
            name: StepName::Doctor,
            executor: Some(step_doctor),
            plan: plan_doctor,
        }];
        let outcome = run_setup(
            &mut context(
                &args,
                &resolved,
                &root,
                &home,
                &mut runner,
                &mut prompt,
                None,
            ),
            &specs,
        );
        assert_eq!(outcome.exit_code, 0);
        assert!(runner.requests.is_empty());
    }

    #[test]
    fn unhandled_executor_error_is_a_fixed_exit_one_failure() {
        let (args, resolved, root, home) = fixture("unhandled", &[]);
        let mut runner = FakeRunner::new(Vec::new());
        let mut prompt = Prompt(false);
        let error = |_context: &mut SetupContext<'_>| {
            Err(StepExecutionError::Unhandled {
                message: "broken seam".into(),
            })
        };
        let specs = [StepSpec {
            name: StepName::Doctor,
            executor: Some(error),
            plan: plan_doctor,
        }];
        let outcome = run_setup(
            &mut context(
                &args,
                &resolved,
                &root,
                &home,
                &mut runner,
                &mut prompt,
                None,
            ),
            &specs,
        );
        assert_eq!(outcome.exit_code, 1);
        let manifest = read_manifest(&manifest_path(&resolved.journal_path)).unwrap();
        assert_eq!(
            manifest.steps[0]["error"]["code"],
            "setup_unhandled_exception"
        );
    }

    #[test]
    fn doctor_jsonl_forwards_only_the_documented_doctor_events() {
        let (args, resolved, root, home) = fixture("doctor-jsonl", &["--jsonl"]);
        let stdout = concat!(
            "{\"event\":\"doctor.started\"}\n",
            "not-json\n",
            "{\"event\":\"other\"}\n",
            "{\"event\":\"check.completed\"}\n",
            "{\"event\":\"doctor.completed\",\"status\":\"ok\"}\n"
        );
        let mut runner = FakeRunner::new(vec![Ok(CommandOutput {
            exit_code: 0,
            stdout: stdout.into(),
            stderr: String::new(),
            timed_out: false,
        })]);
        let mut prompt = Prompt(false);
        let mut recorder = Recorder::default();
        let result = step_doctor(&mut context(
            &args,
            &resolved,
            &root,
            &home,
            &mut runner,
            &mut prompt,
            Some(&mut recorder),
        ))
        .unwrap();
        assert_eq!(result.status, StepStatus::Ok);
        assert_eq!(recorder.0.borrow().len(), 5);
    }
    #[test]
    fn doctor_jsonl_emits_advisory_warning_fields() {
        let (args, resolved, root, home) = fixture("doctor-advisory", &["--jsonl"]);
        let stdout = concat!(
            "{\"event\":\"check.completed\",\"severity\":\"advisory\",\"status\":\"warning\",\"detail\":\"disk low\",\"fix\":\"free space\"}\n",
            "{\"event\":\"doctor.completed\",\"status\":\"ok\"}\n"
        );
        let mut runner = FakeRunner::new(vec![Ok(CommandOutput {
            exit_code: 0,
            stdout: stdout.into(),
            stderr: String::new(),
            timed_out: false,
        })]);
        let mut prompt = Prompt(false);
        let mut recorder = FieldRecorder::default();
        step_doctor(&mut context(
            &args,
            &resolved,
            &root,
            &home,
            &mut runner,
            &mut prompt,
            Some(&mut recorder),
        ))
        .unwrap();
        let events = recorder.0.borrow();
        let (_, fields) = events
            .iter()
            .find(|(event, _)| *event == EventType::StepWarning)
            .unwrap();
        assert_eq!(fields["step"], "doctor");
        assert_eq!(fields["text"], "disk low");
        assert_eq!(fields["fix_hint"], "free space");
    }
    #[test]
    fn journal_persisted_match_keeps_user_config_and_bootstraps_config() {
        let (args, resolved, root, home) = fixture("journal-match", &[]);
        fs::create_dir_all(config_path(&home).parent().unwrap()).unwrap();
        let existing = format!(
            "journal = \"{}\"\nother = \"keep\"\n",
            resolved.journal_path.display()
        );
        fs::write(config_path(&home), &existing).unwrap();
        let mut runner = FakeRunner::new(Vec::new());
        let mut prompt = Prompt(false);
        let result = step_journal(&mut context(
            &args,
            &resolved,
            &root,
            &home,
            &mut runner,
            &mut prompt,
            None,
        ))
        .unwrap();
        assert_eq!(result.status, StepStatus::Ok);
        assert_eq!(fs::read_to_string(config_path(&home)).unwrap(), existing);
        assert!(get_journal_config_path(&resolved.journal_path).is_file());
    }

    #[cfg(unix)]
    #[test]
    fn existing_journal_converges_retired_log_aliases() {
        let (args, resolved, root, home) =
            fixture("journal-legacy-alias", &["--accept-existing-journal"]);
        fs::create_dir_all(resolved.journal_path.join("config")).expect("existing journal config");
        let alias = resolved.journal_path.join("health/heartbeat.log");
        fs::create_dir_all(alias.parent().expect("health directory")).expect("health directory");
        symlink(
            "../chronicle/20240101/health/launch-1_heartbeat.log",
            &alias,
        )
        .expect("retired managed-process alias");
        assert!(
            non_empty_journal(&resolved.journal_path),
            "the pre-creation journal is classified as existing"
        );

        let mut runner = FakeRunner::new(Vec::new());
        let mut prompt = Prompt(false);
        let result = step_journal(&mut context(
            &args,
            &resolved,
            &root,
            &home,
            &mut runner,
            &mut prompt,
            None,
        ))
        .expect("existing journal setup");

        assert_eq!(result.status, StepStatus::Ok);
        assert!(
            alias.symlink_metadata().is_err(),
            "retired alias was removed"
        );
    }

    #[test]
    fn fresh_journal_does_not_create_legacy_alias_namespaces() {
        let (args, resolved, root, home) = fixture("journal-fresh-no-cleanup", &[]);
        assert!(!resolved.journal_path.exists(), "the journal starts absent");

        let mut runner = FakeRunner::new(Vec::new());
        let mut prompt = Prompt(false);
        step_journal(&mut context(
            &args,
            &resolved,
            &root,
            &home,
            &mut runner,
            &mut prompt,
            None,
        ))
        .expect("fresh journal setup");

        for namespace in ["health", "chronicle", "talents", "agents"] {
            assert!(
                !resolved.journal_path.join(namespace).exists(),
                "fresh setup does not create {namespace} for legacy cleanup"
            );
        }
    }

    #[test]
    fn journal_fresh_write_replaces_user_config_and_config_bootstrap_preserves_existing() {
        let (args, resolved, root, home) = fixture("journal-fresh", &["--accept-existing-journal"]);
        fs::create_dir_all(config_path(&home).parent().unwrap()).unwrap();
        fs::write(config_path(&home), "other = \"drop\"\n").unwrap();
        fs::create_dir_all(resolved.journal_path.join("config")).unwrap();
        let config = get_journal_config_path(&resolved.journal_path);
        fs::write(&config, "{\"known\":\"existing\"}").unwrap();
        let before = fs::read(&config).unwrap();
        let mut runner = FakeRunner::new(Vec::new());
        let mut prompt = Prompt(false);
        step_journal(&mut context(
            &args,
            &resolved,
            &root,
            &home,
            &mut runner,
            &mut prompt,
            None,
        ))
        .unwrap();
        assert_eq!(
            fs::read_to_string(config_path(&home)).unwrap(),
            format!("journal = \"{}\"\n", resolved.journal_path.display())
        );
        assert_eq!(fs::read(&config).unwrap(), before);
    }
    #[test]
    fn journal_dead_end_retains_prior_manifest_entries() {
        let (args, resolved, root, home) = fixture("dead-end", &[]);
        fs::create_dir_all(resolved.journal_path.join("config")).unwrap();
        let mut runner = FakeRunner::new(Vec::new());
        let mut prompt = Prompt(false);
        let ok = |context: &mut SetupContext<'_>| {
            Ok(StepResult::new(
                StepName::Doctor,
                StepStatus::Ok,
                Vec::new(),
                (context.now)(),
            ))
        };
        let specs = [
            StepSpec {
                name: StepName::Doctor,
                executor: Some(ok),
                plan: plan_doctor,
            },
            StepSpec {
                name: StepName::Journal,
                executor: Some(step_journal),
                plan: plan_journal,
            },
        ];
        let outcome = run_setup(
            &mut context(
                &args,
                &resolved,
                &root,
                &home,
                &mut runner,
                &mut prompt,
                None,
            ),
            &specs,
        );
        assert_eq!(outcome.exit_code, 2);
        let dead_end = outcome.dead_end.unwrap();
        assert_eq!(dead_end.step_name, Some(StepName::Journal));
        assert_eq!(dead_end.error_code, Some(ErrorCode::JournalExistingBlocked));
        assert_eq!(
            dead_end.message,
            existing_journal_message(&resolved.journal_path)
        );
        let manifest = read_manifest(&manifest_path(&resolved.journal_path)).unwrap();
        assert_eq!(manifest.steps.len(), 1);
        assert_eq!(manifest.completed_at, None);
    }

    #[test]
    fn journal_file_dead_end_carries_the_reference_error_code() {
        let (args, resolved, root, home) = fixture("journal-file", &[]);
        fs::create_dir_all(resolved.journal_path.parent().unwrap()).unwrap();
        fs::write(&resolved.journal_path, "not a directory").unwrap();
        let mut runner = FakeRunner::new(Vec::new());
        let mut prompt = Prompt(false);
        let error = step_journal(&mut context(
            &args,
            &resolved,
            &root,
            &home,
            &mut runner,
            &mut prompt,
            None,
        ))
        .unwrap_err();
        assert_eq!(
            error,
            StepExecutionError::DeadEnd {
                message: format!(
                    "expected a directory at {}; got a regular file. Re-run with --journal <other-path>.",
                    resolved.journal_path.display()
                ),
                exit_code: 2,
                step_name: Some(StepName::Journal),
                error_code: Some(ErrorCode::JournalDirInvalid)
            }
        );
    }

    #[test]
    fn tail_handles_a_multibyte_truncation_boundary() {
        let value = format!("{}😀{}", "a".repeat(8190), "b".repeat(8189));
        let output = tail(&value);
        assert!(output.is_char_boundary(0));
        assert!(!output.is_empty());
    }

    #[test]
    fn persisted_journal_match_trims_and_canonicalizes_existing_paths() {
        let (args, mut resolved, root, home) = fixture("canonical-match", &[]);
        let journal = root.join("journal/sub");
        fs::create_dir_all(&journal).unwrap();
        resolved.journal_path = journal.clone().canonicalize().unwrap();
        fs::create_dir_all(config_path(&home).parent().unwrap()).unwrap();
        fs::write(
            config_path(&home),
            format!(
                "journal = \"  {}/./sub/../sub  \"\n",
                root.join("journal").display()
            ),
        )
        .unwrap();
        let mut runner = FakeRunner::new(Vec::new());
        let mut prompt = Prompt(false);
        step_journal(&mut context(
            &args,
            &resolved,
            &root,
            &home,
            &mut runner,
            &mut prompt,
            None,
        ))
        .unwrap();
        assert!(runner.requests.is_empty());
        assert!(get_journal_config_path(&journal).is_file());
    }

    /// The plan is the surface whose whole job is telling an owner what a run
    /// would do with the arguments they just gave it, so every resolved value
    /// carries its provenance. This pins all four lines because the runtime
    /// honoured them while the rendering did not, and a narrowing that nothing
    /// fails on is the one that survives.
    #[test]
    fn plan_header_carries_every_resolved_value_with_its_source() {
        let (args, resolved, root, home) = fixture(
            "plan-header",
            &["--explain", "--port", "6000", "--variant", "cuda"],
        );
        let mut runner = FakeRunner::new(Vec::new());
        let mut prompt = Prompt(false);
        let context = context(
            &args,
            &resolved,
            &root,
            &home,
            &mut runner,
            &mut prompt,
            None,
        );
        let lines = render_plan(&context, false);

        assert_eq!(lines[0], "setup plan:");
        assert_eq!(lines[1], "  mode: explain");
        assert!(lines[2].starts_with("  journal: "), "{:?}", lines[2]);
        assert_eq!(lines[3], "  port: 6000 (cli)");
        assert_eq!(lines[4], "  variant: cuda (cli)");
        assert_eq!(lines[5], "  step_timeout_seconds: 1800 (default)");
        assert_eq!(lines[6], "  source checkout: False");
    }

    fn write_app_owned_child_marker(path: &Path, target: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let escaped = target.to_string_lossy().replace('\'', "'\\''");
        fs::write(
            path,
            format!("#!/bin/sh\n# managed-version: app-owned-child\nexec '{escaped}' \"$@\"\n"),
        )
        .unwrap();
    }

    #[test]
    fn native_probe_detects_macos_app_owned_child_marker_at_literal_sol_path() {
        let (args, resolved, root, home) = fixture("app-owned-child-probe", &[]);
        let runtime = root.join("runtime");
        fs::create_dir_all(&runtime).unwrap();
        let target = runtime.join("child");
        fs::write(&target, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        }
        write_app_owned_child_marker(&home.join(".local/bin/sol"), &target);
        write_app_owned_child_marker(&home.join(".local/bin/journal"), &target);
        let mut runner = FakeRunner::new(Vec::new());
        let mut prompt = Prompt(false);
        let setup = context(
            &args,
            &resolved,
            &root,
            &home,
            &mut runner,
            &mut prompt,
            None,
        );
        assert!(native_already_keeps_journal_probe(&setup).unwrap());
        assert!(!home.join(".local/bin/solstone").exists());
    }
}
