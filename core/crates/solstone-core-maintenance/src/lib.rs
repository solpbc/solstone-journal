// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native registry and command surface for recurring journal maintenance.

pub mod bodies;
mod parser;
pub mod registry;
pub mod schedule_sync;
pub mod timezone;

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Datelike, Utc};
use registry::{RoutineDescriptor, routines};
use solstone_core_artifact_download::{ByteDownload, UreqByteDownload};
use solstone_core_backup_runtime::{
    BackupServices, Clock, HttpTransport, NativeJournalMaintenance, SystemToolRunner,
    ToolInstallDirs, ToolRunner, UreqHttpTransport, prepare, resolve_operational_tools,
    resolve_tools,
};
use solstone_core_generate::{GenerateRequest, GenerateResponse, OneShotClient};
use solstone_core_offload::{OffloadResult, format_offload_result};

pub use parser::USAGE;

/// Captured command output for the aggregate journal dispatcher.
#[derive(PartialEq, Eq)]
pub struct CliRun {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

impl std::fmt::Debug for CliRun {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CliRun")
            .field("stdout", &"<redacted>")
            .field("stderr", &"<redacted>")
            .field("exit_code", &self.exit_code)
            .finish()
    }
}

/// Injectable dependencies for parser and routine-dispatch tests.
#[derive(Debug, Clone, Copy)]
pub struct MaintenanceServices<'a> {
    pub routines: &'a [RoutineDescriptor],
}

/// Injectable time and owner-timezone dependencies for health routines.
pub struct HealthServices<'a> {
    pub now: DateTime<Utc>,
    pub host_timezone: &'a dyn timezone::HostTimezoneSource,
}

/// Injectable generation dependencies for timeline rollup routines.
pub struct TimelineServices<'a> {
    pub now: DateTime<Utc>,
    pub host_timezone: &'a dyn timezone::HostTimezoneSource,
    pub picker: &'a dyn RollupPicker,
    pub model_resolver: &'a dyn GenerateModelResolver,
}

/// One model selection request for a timeline rollup.
pub trait RollupPicker: Sync {
    fn pick(&self, request: &GenerateRequest) -> Result<String, String>;
}

/// Resolve the configured generate-lane model without making a generation request.
pub trait GenerateModelResolver: Sync {
    fn resolve(
        &self,
        config: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<String, String>;
}

impl<'a> MaintenanceServices<'a> {
    pub const fn new(routines: &'a [RoutineDescriptor]) -> Self {
        Self { routines }
    }
}

/// Run the production maintenance command parser.
pub fn run_cli(args: &[String], journal: &Path) -> CliRun {
    let http = UreqHttpTransport;
    run_cli_with_deps(
        args,
        journal,
        &SystemToolRunner,
        &UreqByteDownload,
        &http,
        ToolInstallDirs::default(),
    )
}

fn run_cli_with_deps(
    args: &[String],
    journal: &Path,
    runner: &dyn ToolRunner,
    downloader: &dyn ByteDownload,
    http: &dyn HttpTransport,
    dirs: ToolInstallDirs<'_>,
) -> CliRun {
    let clock = ProductionClock;
    let restore_hooks = NativeJournalMaintenance;
    let host_timezone = timezone::ProductionHostTimezoneSource;
    let rollup_picker = ProductionRollupPicker;
    let model_resolver = ProductionGenerateModelResolver;
    let now = Utc::now();
    let placeholder = BackupServices {
        runner,
        http,
        clock: &clock,
        restic_path: None,
        rclone_path: None,
        version: env!("CARGO_PKG_VERSION"),
        journal_maintenance: &restore_hooks,
    };
    if is_bare_backup_run(args) {
        return run_admitted_backup(journal, &clock, runner, downloader, dirs, placeholder);
    }
    let maintenance_services = MaintenanceServices::new(routines());
    let health = HealthServices {
        now,
        host_timezone: &host_timezone,
    };
    let timeline = TimelineServices {
        now,
        host_timezone: &host_timezone,
        picker: &rollup_picker,
        model_resolver: &model_resolver,
    };
    match classify_maintenance_tool_resolution(args) {
        None => run_cli_with_all_services(
            args,
            journal,
            &maintenance_services,
            Some(&placeholder),
            Some(&health),
            Some(&timeline),
        ),
        Some(append_only) => {
            match resolve_operational_tools(runner, downloader, journal, append_only, dirs) {
                Ok(tools) => {
                    let backup_services = BackupServices {
                        restic_path: Some(&tools.restic_path),
                        rclone_path: tools.rclone_path.as_deref(),
                        ..placeholder
                    };
                    run_cli_with_all_services(
                        args,
                        journal,
                        &maintenance_services,
                        Some(&backup_services),
                        Some(&health),
                        Some(&timeline),
                    )
                }
                Err(reason) => format_backup_resolution_error(args, &reason),
            }
        }
    }
}

fn format_backup_resolution_error(args: &[String], reason: &str) -> CliRun {
    let id = classify_maintenance_routine_id(args);
    let line = match id.as_deref() {
        Some("backup:prune") => format!("backup prune: error reason={reason}"),
        Some("backup:verify") => format!("backup verify: error reason={reason}"),
        Some("backup:offload") => format_offload_result(&OffloadResult {
            status: "stalled".into(),
            reason: Some(reason.to_owned()),
            files_marked: 0,
            bytes_marked: 0,
            files_already_marked: 0,
            bytes_already_marked: 0,
            ran_out_of_markable_media: false,
            dry_run: false,
            reason_detail: None,
            details: vec![],
            recording_failure: None,
        }),
        _ => format!("backup: error reason={reason}"),
    };
    backup_routine_line(line)
}

fn run_admitted_backup(
    journal: &Path,
    clock: &dyn Clock,
    runner: &dyn ToolRunner,
    downloader: &dyn ByteDownload,
    dirs: ToolInstallDirs<'_>,
    placeholder: BackupServices<'_>,
) -> CliRun {
    match prepare(journal, clock) {
        Ok(capability) => match resolve_tools(&capability, runner, downloader, dirs) {
            Ok(tools) => {
                let services = BackupServices {
                    restic_path: Some(&tools.restic_path),
                    rclone_path: tools.rclone_path.as_deref(),
                    ..placeholder
                };
                bodies::backup::backup_run_result(capability.execute(&services))
            }
            Err(tool_error) => {
                bodies::backup::backup_run_result(capability.record_tool_error(clock, tool_error))
            }
        },
        Err(result) => bodies::backup::backup_run_result(result),
    }
}

fn backup_routine_line(line: String) -> CliRun {
    CliRun {
        stdout: format!("{line}\n"),
        stderr: String::new(),
        exit_code: 0,
    }
}

fn classify_maintenance_routine_id(args: &[String]) -> Option<String> {
    let args = strip_maintenance_global_flags(args);
    let rest = args.strip_prefix(&["run".to_owned()])?;
    rest.first().cloned()
}

fn is_bare_backup_run(args: &[String]) -> bool {
    let args = strip_maintenance_global_flags(args);
    if maintenance_has_help(&args) {
        return false;
    }
    let Some((command, rest)) = args.split_first() else {
        return false;
    };
    if command != "run" {
        return false;
    }
    let Some((id, routine_args)) = rest.split_first() else {
        return false;
    };
    let forwarded = routine_args
        .strip_prefix(&["--".to_owned()])
        .unwrap_or(routine_args);
    id == "backup:run" && forwarded.is_empty()
}

/// Keep in sync with `parser::run` / `parser::run_routine` backup id matching.
fn classify_maintenance_tool_resolution(args: &[String]) -> Option<bool> {
    let args = strip_maintenance_global_flags(args);
    if maintenance_has_help(&args) {
        return None;
    }
    let (command, rest) = args.split_first()?;
    if command != "run" {
        return None;
    }
    let (id, routine_args) = rest.split_first()?;
    let forwarded = routine_args
        .strip_prefix(&["--".to_owned()])
        .unwrap_or(routine_args);
    match id.as_str() {
        "backup:prune" if forwarded.is_empty() => Some(false),
        "backup:verify" if forwarded.is_empty() => Some(false),
        "backup:offload" if !forwarded.iter().any(|argument| argument != "--dry-run") => Some(true),
        _ => None,
    }
}

fn strip_maintenance_global_flags(args: &[String]) -> Vec<String> {
    let mut first_command = 0;
    while first_command < args.len()
        && matches!(
            args[first_command].as_str(),
            "-v" | "--verbose" | "-d" | "--debug"
        )
    {
        first_command += 1;
    }
    args[first_command..].to_vec()
}

fn maintenance_has_help(args: &[String]) -> bool {
    let end = args.iter().position(|argument| argument == "--");
    let options = end.map_or(args, |index| &args[..index]);
    options
        .iter()
        .any(|argument| matches!(argument.as_str(), "-h" | "--help"))
}

/// Run the maintenance parser with an injected registry.
pub fn run_cli_with(args: &[String], journal: &Path, services: &MaintenanceServices<'_>) -> CliRun {
    parser::run(args, journal, services, None, None, None)
}

/// Run the maintenance parser with an injected backup runtime service set.
pub fn run_cli_with_backup(
    args: &[String],
    journal: &Path,
    services: &MaintenanceServices<'_>,
    backup_services: &BackupServices<'_>,
) -> CliRun {
    parser::run(args, journal, services, Some(backup_services), None, None)
}

/// Run the maintenance parser with injected backup and health routine services.
pub fn run_cli_with_services(
    args: &[String],
    journal: &Path,
    services: &MaintenanceServices<'_>,
    backup_services: Option<&BackupServices<'_>>,
    health_services: Option<&HealthServices<'_>>,
) -> CliRun {
    parser::run(
        args,
        journal,
        services,
        backup_services,
        health_services,
        None,
    )
}

/// Run the maintenance parser with injected timeline services.
pub fn run_cli_with_timeline(
    args: &[String],
    journal: &Path,
    services: &MaintenanceServices<'_>,
    timeline_services: &TimelineServices<'_>,
) -> CliRun {
    parser::run(args, journal, services, None, None, Some(timeline_services))
}

/// Run the maintenance parser with every routine service set injected.
pub fn run_cli_with_all_services(
    args: &[String],
    journal: &Path,
    services: &MaintenanceServices<'_>,
    backup_services: Option<&BackupServices<'_>>,
    health_services: Option<&HealthServices<'_>>,
    timeline_services: Option<&TimelineServices<'_>>,
) -> CliRun {
    parser::run(
        args,
        journal,
        services,
        backup_services,
        health_services,
        timeline_services,
    )
}

struct ProductionRollupPicker;

impl RollupPicker for ProductionRollupPicker {
    fn pick(&self, request: &GenerateRequest) -> Result<String, String> {
        let client = OneShotClient::sibling().map_err(|error| format!("{error:?}"))?;
        match client
            .execute(request)
            .map_err(|error| format!("{error:?}"))?
        {
            GenerateResponse::Generated(response) => Ok(response.text),
            GenerateResponse::Refused(response) => Err(response.detail),
        }
    }
}

struct ProductionGenerateModelResolver;

impl GenerateModelResolver for ProductionGenerateModelResolver {
    fn resolve(
        &self,
        config: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<String, String> {
        solstone_core_brain::resolve_generate_model(config).map_err(|error| error.to_string())
    }
}

struct ProductionClock;

impl Clock for ProductionClock {
    fn now_unix(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
            .unwrap_or(0)
    }

    fn iso_week(&self) -> u8 {
        Utc::now().iso_week().week() as u8
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "the composed fixture owns its temporary journal and fake services"
)]
mod composed_tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::io;
    use std::path::Path;

    use chrono::{TimeZone, Utc};
    use serde_json::{Map, Value, json};
    use solstone_core_backup_runtime::hosted_runtime::HttpError;
    use solstone_core_backup_runtime::{
        BackupServices, Clock, HttpRequest, HttpResponse, HttpTransport, JournalMaintenance,
        JournalMaintenanceError, ToolOutput, ToolRequest, ToolRunner,
    };
    use solstone_core_generate::GenerateRequest;

    use super::{
        GenerateModelResolver, HealthServices, MaintenanceServices, RollupPicker, TimelineServices,
        registry, run_cli_with_all_services,
    };
    use crate::timezone::HostTimezoneSource;

    struct Runner(RefCell<VecDeque<ToolOutput>>);

    impl ToolRunner for Runner {
        fn run(&self, _: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            Ok(self.0.borrow_mut().pop_front().unwrap_or(ToolOutput {
                returncode: 0,
                stdout: Vec::new(),
                stderr: Vec::new(),
            }))
        }
    }

    struct Http;

    impl HttpTransport for Http {
        fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
            panic!("the composed maintenance fixture does not use HTTP")
        }
    }

    struct FixtureClock;

    impl Clock for FixtureClock {
        fn now_unix(&self) -> i64 {
            1_772_323_200
        }

        fn iso_week(&self) -> u8 {
            9
        }
    }

    struct Hooks;

    impl JournalMaintenance for Hooks {
        fn rebuild_body_history(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
            panic!("maintenance routines do not restore")
        }

        fn full_scan(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
            panic!("maintenance routines do not restore")
        }
    }

    struct Host;

    impl HostTimezoneSource for Host {
        fn usable_iana_key(&self) -> Option<String> {
            Some("UTC".to_owned())
        }
    }

    struct Picker;

    impl RollupPicker for Picker {
        fn pick(&self, _: &GenerateRequest) -> Result<String, String> {
            panic!("empty timeline fixtures must not generate")
        }
    }

    struct Model;

    impl GenerateModelResolver for Model {
        fn resolve(&self, _: &Map<String, Value>) -> Result<String, String> {
            Ok("fixture-model".to_owned())
        }
    }

    #[test]
    fn all_routines_compose_through_parser_registry_and_schedule_without_python() {
        let journal = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(journal.path().join("config")).unwrap();
        std::fs::create_dir_all(journal.path().join("chronicle")).unwrap();
        std::fs::write(
            journal.path().join("config/journal.json"),
            json!({
                "retention": {
                    "raw_media": "keep",
                    "journal_logs": {"enabled": false}
                }
            })
            .to_string(),
        )
        .unwrap();
        let runner = Runner(RefCell::new(VecDeque::new()));
        let http = Http;
        let clock = FixtureClock;
        let hooks = Hooks;
        let host = Host;
        let picker = Picker;
        let model = Model;
        let backup = BackupServices {
            runner: &runner,
            http: &http,
            clock: &clock,
            restic_path: Some(Path::new("/fixture/bin/restic")),
            rclone_path: None,
            version: "test",
            journal_maintenance: &hooks,
        };
        let now = Utc.with_ymd_and_hms(2026, 3, 2, 0, 0, 0).unwrap();
        let health = HealthServices {
            now,
            host_timezone: &host,
        };
        let timeline = TimelineServices {
            now,
            host_timezone: &host,
            picker: &picker,
            model_resolver: &model,
        };
        let services = MaintenanceServices::new(registry::routines());
        let run = |arguments: &[&str]| {
            run_cli_with_all_services(
                &arguments
                    .iter()
                    .map(|argument| (*argument).to_owned())
                    .collect::<Vec<_>>(),
                journal.path(),
                &services,
                Some(&backup),
                Some(&health),
                Some(&timeline),
            )
        };

        assert!(run(&["list"]).stdout.contains("backup:run"));
        assert_eq!(run(&["sync"]).exit_code, 0);

        for (id, witness, expected_exit) in [
            ("backup:run", "backup: skipped", 0),
            ("backup:prune", "backup prune:", 0),
            ("backup:verify", "backup verify:", 0),
            ("backup:offload", "backup offload:", 0),
            ("health:mark-raw", "new items: 0", 0),
            ("health:prune-logs", "prune-logs: disabled", 0),
            ("timeline:rollup-day", "no segment timeline.json", 66),
            ("timeline:rollup-master", "no day-level timeline.json", 66),
        ] {
            let mut args = vec!["run", id];
            if id == "backup:offload" {
                args.push("--dry-run");
            }
            if id == "timeline:rollup-day" {
                args.push("20260301");
            }
            let result = run(&args);
            assert_eq!(result.exit_code, expected_exit, "{id}: {result:?}");
            assert!(result.stdout.contains(witness), "{id}: {result:?}");
        }
    }
}

#[cfg(test)]
mod resolution_tests {
    use super::*;
    use serde_json::{Value, json};
    use solstone_core_artifact_download::{ByteDownload, ByteDownloadError};
    use solstone_core_backup::{
        Destination, HostedBinding, generate_and_store_keys, get_backup_config,
        record_backup_result, record_verification_result, save_hosted_binding, set_destination,
        set_enabled, set_mode, set_offload,
    };
    use solstone_core_backup_runtime::hosted_runtime::{HttpError, HttpRequest, HttpResponse};
    use solstone_core_backup_runtime::rclone_install::{
        RCLONE_SCHEMA_VERSION, RCLONE_TOOL, RCLONE_VERSION,
    };
    use solstone_core_backup_runtime::readiness::{
        RESTIC_SCHEMA_VERSION, RESTIC_TOOL, RESTIC_VERSION, binary_path, file_sha256,
        platform_info, sentinel_path,
    };
    use solstone_core_backup_runtime::{
        HttpTransport, ToolInstallDirs, ToolOutput, ToolRequest, ToolRunner,
        backup_journal_resolved_hook_armed, backup_path_resolution_attempts,
        install_backup_journal_resolved_hook, reset_backup_journal_resolved_hook,
        reset_backup_path_resolution_attempts,
    };
    use std::cell::{Cell, RefCell};
    use std::ffi::OsString;
    use std::fs;
    use std::io;
    #[cfg(unix)]
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::{Path, PathBuf};
    #[cfg(unix)]
    use std::rc::Rc;
    use std::sync::{LazyLock, Mutex};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    static CURRENT_DIRECTORY: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    struct CurrentDirectoryGuard(PathBuf);

    impl CurrentDirectoryGuard {
        fn change_to(path: &Path) -> Self {
            let original = std::env::current_dir().expect("working directory reads");
            std::env::set_current_dir(path).expect("working directory sets");
            Self(original)
        }
    }

    impl Drop for CurrentDirectoryGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
        }
    }

    struct RecordingRunner {
        programs: RefCell<Vec<OsString>>,
        argvs: RefCell<Vec<Vec<String>>>,
        on_first_call: RefCell<Option<Box<dyn FnOnce()>>>,
    }

    impl RecordingRunner {
        fn new() -> Self {
            Self {
                programs: RefCell::new(vec![]),
                argvs: RefCell::new(vec![]),
                on_first_call: RefCell::new(None),
            }
        }

        fn with_on_first_call(callback: impl FnOnce() + 'static) -> Self {
            Self {
                programs: RefCell::new(vec![]),
                argvs: RefCell::new(vec![]),
                on_first_call: RefCell::new(Some(Box::new(callback))),
            }
        }
    }

    impl ToolRunner for RecordingRunner {
        fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            if let Some(callback) = self.on_first_call.borrow_mut().take() {
                callback();
            }
            self.programs.borrow_mut().push(request.program.clone());
            self.argvs.borrow_mut().push(
                request
                    .argv
                    .iter()
                    .map(|value| value.to_string_lossy().into_owned())
                    .collect(),
            );
            let name = Path::new(&request.program)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            let version = request.argv.iter().any(|value| value == "version");
            let stdout = if version && name == RCLONE_TOOL {
                format!("rclone v{RCLONE_VERSION}\n")
            } else if version {
                format!("restic {RESTIC_VERSION}\n")
            } else {
                "{\"message_type\":\"summary\",\"snapshot_id\":\"snap\"}\n".to_owned()
            };
            Ok(ToolOutput {
                returncode: 0,
                stdout: stdout.into_bytes(),
                stderr: vec![],
            })
        }
    }

    struct PanicDownload;

    impl ByteDownload for PanicDownload {
        fn fetch(&self, _: &str, _: Duration) -> Result<Vec<u8>, ByteDownloadError> {
            panic!("must not download")
        }
    }

    struct FailingDownload {
        calls: Cell<u32>,
    }

    impl ByteDownload for FailingDownload {
        fn fetch(&self, _: &str, _: Duration) -> Result<Vec<u8>, ByteDownloadError> {
            self.calls.set(self.calls.get() + 1);
            Err(ByteDownloadError::Transport)
        }
    }

    struct UnusedHttp;

    impl HttpTransport for UnusedHttp {
        fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
            panic!("BYO must not fetch broker credentials")
        }
    }

    struct BrokerHttp {
        calls: Cell<u32>,
    }

    impl BrokerHttp {
        fn new() -> Self {
            Self {
                calls: Cell::new(0),
            }
        }
    }

    impl HttpTransport for BrokerHttp {
        fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
            self.calls.set(self.calls.get() + 1);
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: serde_json::to_vec(&json!({
                    "access_key_id": "access",
                    "secret_access_key": "secret",
                    "session_token": "token",
                    "endpoint": "https://example.invalid",
                    "expires_at": "2099-01-01T00:00:00Z",
                }))
                .unwrap(),
            })
        }
    }

    fn write_ready_restic(dir: &Path) -> PathBuf {
        let binary = binary_path(dir);
        fs::write(&binary, b"restic-fixture").unwrap();
        let digest = file_sha256(&binary).unwrap();
        let (os, arch) = platform_info().unwrap();
        fs::write(
            sentinel_path(dir),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": RESTIC_SCHEMA_VERSION,
                "tool": RESTIC_TOOL,
                "version": RESTIC_VERSION,
                "sha256": digest,
                "platform": {"os": os, "arch": arch},
                "binary_path": binary,
            }))
            .unwrap(),
        )
        .unwrap();
        binary
    }

    fn write_ready_rclone(dir: &Path) -> PathBuf {
        let binary = dir.join(RCLONE_TOOL);
        fs::write(&binary, b"rclone-fixture").unwrap();
        let digest = file_sha256(&binary).unwrap();
        let (os, arch) = platform_info().unwrap();
        fs::write(
            dir.join(".install-complete"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": RCLONE_SCHEMA_VERSION,
                "tool": RCLONE_TOOL,
                "version": RCLONE_VERSION,
                "sha256": digest,
                "platform": {"os": os, "arch": arch},
                "binary_path": binary,
            }))
            .unwrap(),
        )
        .unwrap();
        binary
    }

    fn dirs<'a>(restic: &'a Path, rclone: Option<&'a Path>) -> ToolInstallDirs<'a> {
        ToolInstallDirs {
            restic: Some(restic),
            rclone,
        }
    }

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn rclone_program(argvs: &[Vec<String>]) -> Option<String> {
        argvs.iter().find_map(|argv| {
            argv.windows(2).find_map(|pair| {
                pair[1]
                    .strip_prefix("rclone.program=")
                    .filter(|_| pair[0] == "-o")
                    .map(str::to_owned)
            })
        })
    }

    fn last_backup_reason(journal: &Path) -> Option<String> {
        get_backup_config(journal)
            .unwrap()
            .get("last_backup")
            .and_then(Value::as_object)
            .and_then(|backup| backup.get("error_reason"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    }

    fn last_backup_status(journal: &Path) -> Option<String> {
        get_backup_config(journal)
            .unwrap()
            .get("last_backup")
            .and_then(Value::as_object)
            .and_then(|backup| backup.get("status"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    }

    fn operated_journal_configured(
        broker_endpoint: &str,
        account_id: &str,
        instance_id: &str,
        bucket: &str,
        prefix: &str,
        broker_token: &str,
    ) -> tempfile::TempDir {
        let journal = tempfile::tempdir().unwrap();
        set_mode(journal.path(), "operated").unwrap();
        set_enabled(journal.path(), true).unwrap();
        generate_and_store_keys(journal.path()).unwrap();
        save_hosted_binding(
            journal.path(),
            &HostedBinding {
                broker_endpoint: broker_endpoint.into(),
                account_id: account_id.into(),
                instance_id: instance_id.into(),
                bucket: bucket.into(),
                prefix: prefix.into(),
                broker_token: broker_token.into(),
            },
        )
        .unwrap();
        journal
    }

    fn operated_journal() -> tempfile::TempDir {
        operated_journal_configured(
            "https://broker.example.invalid",
            "account",
            "instance",
            "bucket",
            "prefix",
            "token",
        )
    }

    fn byo_journal_configured(
        repository: &str,
        backend: &str,
        access_key: &str,
        secret_key: &str,
    ) -> tempfile::TempDir {
        let journal = tempfile::tempdir().unwrap();
        set_destination(
            journal.path(),
            &Destination {
                repository: repository.to_owned(),
                backend: backend.to_owned(),
                credentials: serde_json::Map::from_iter([
                    (
                        "access_key_id".to_owned(),
                        Value::String(access_key.to_owned()),
                    ),
                    (
                        "secret_access_key".to_owned(),
                        Value::String(secret_key.to_owned()),
                    ),
                ]),
            },
        )
        .unwrap();
        generate_and_store_keys(journal.path()).unwrap();
        set_enabled(journal.path(), true).unwrap();
        journal
    }

    fn byo_journal() -> tempfile::TempDir {
        byo_journal_configured("s3:bucket/prefix", "s3", "access", "secret")
    }

    fn journal_config_bytes(journal: &Path) -> Vec<u8> {
        fs::read(journal.join("config/journal.json")).expect("journal config reads")
    }

    fn assert_alias_resolved_once() {
        assert_eq!(backup_path_resolution_attempts(), 1);
        assert!(!backup_journal_resolved_hook_armed());
    }

    #[cfg(unix)]
    fn install_alias_retarget(
        alias: PathBuf,
        replacement: PathBuf,
        next_directory: PathBuf,
        after_retarget: impl FnOnce() + 'static,
    ) {
        install_backup_journal_resolved_hook(move || {
            fs::remove_file(&alias).expect("source alias removes");
            symlink(&replacement, &alias).expect("replacement alias creates");
            std::env::set_current_dir(&next_directory)
                .expect("working directory changes after admission");
            after_retarget();
        });
    }

    #[cfg(unix)]
    struct ReadOnlyDirectoryGuard {
        path: PathBuf,
        original_permissions: fs::Permissions,
    }

    #[cfg(unix)]
    impl ReadOnlyDirectoryGuard {
        fn make_read_only(path: &Path) -> Self {
            let original_permissions = fs::metadata(path)
                .expect("config directory metadata reads")
                .permissions();
            fs::set_permissions(path, fs::Permissions::from_mode(0o555))
                .expect("config directory becomes read-only");
            Self {
                path: path.to_path_buf(),
                original_permissions,
            }
        }
    }

    #[cfg(unix)]
    impl Drop for ReadOnlyDirectoryGuard {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.path, self.original_permissions.clone());
        }
    }

    fn assert_restic_unavailable_output(output: &CliRun) {
        assert_eq!(output.stdout, "backup: error reason=restic_unavailable\n");
        assert_eq!(output.stderr, "");
        assert_eq!(output.exit_code, 0);
    }

    fn assert_rclone_unavailable_output(output: &CliRun) {
        assert_eq!(output.stdout, "backup: error reason=rclone_unavailable\n");
        assert_eq!(output.stderr, "");
        assert_eq!(output.exit_code, 0);
    }

    fn assert_no_backup_execution(runner: &RecordingRunner) {
        assert!(
            runner
                .argvs
                .borrow()
                .iter()
                .all(|argv| argv.first().map(String::as_str) != Some("backup"))
        );
    }

    fn configure_offload(journal: &Path) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        record_backup_result(journal, "ok", json!(now), json!("ready"), Value::Null).unwrap();
        record_verification_result(journal, "ok", json!(now), Value::Null, json!("1/52")).unwrap();
        set_offload(
            journal,
            json!({"enabled": true, "budget_bytes": 1, "floor_bytes": 1})
                .as_object()
                .unwrap(),
        )
        .unwrap();
        let raw = journal.join("chronicle/20260101/010000_001/raw.webm");
        fs::create_dir_all(raw.parent().unwrap()).unwrap();
        fs::write(&raw, b"one").unwrap();
        let size = raw.metadata().unwrap().len();
        fs::write(
            raw.with_extension("jsonl"),
            format!(
                "{}\n",
                json!({"_solstone_processing": {
                    "schema": "solstone.processing.v1",
                    "state": "empty",
                    "reason_code": "no_decodable_frames",
                    "handler": "describe",
                    "attempted_at": "2026-01-01T00:00:00Z",
                    "input_size": size
                }})
            ),
        )
        .unwrap();
    }

    fn assert_resolved_restic(programs: &[OsString], expected: &Path, decoy: &Path) {
        assert!(
            programs
                .iter()
                .any(|program| program == expected.as_os_str()),
            "missing restic spawn at {expected:?}: {programs:?}"
        );
        assert!(programs.iter().all(|program| program != decoy.as_os_str()));
        assert!(programs.iter().all(|program| program != "restic"));
    }

    #[test]
    fn ac2_backup_run_uses_pinned_restic_not_decoy() {
        let restic_dir = tempfile::tempdir().unwrap();
        let decoy_dir = tempfile::tempdir().unwrap();
        let expected = write_ready_restic(restic_dir.path());
        let decoy = decoy_dir.path().join("restic");
        fs::write(&decoy, b"decoy").unwrap();
        let journal = byo_journal();
        let runner = RecordingRunner::new();
        run_cli_with_deps(
            &args(&["run", "backup:run"]),
            journal.path(),
            &runner,
            &PanicDownload,
            &UnusedHttp,
            dirs(restic_dir.path(), None),
        );
        assert_resolved_restic(&runner.programs.borrow(), &expected, &decoy);
    }

    #[test]
    fn ac3_backup_run_persists_restic_unavailable() {
        let restic_dir = tempfile::tempdir().unwrap();
        let journal = byo_journal();
        record_backup_result(journal.path(), "ok", json!(1), json!("prior"), Value::Null).unwrap();
        let runner = RecordingRunner::new();
        let downloader = FailingDownload {
            calls: Cell::new(0),
        };
        let output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            journal.path(),
            &runner,
            &downloader,
            &UnusedHttp,
            dirs(restic_dir.path(), None),
        );
        assert_eq!(output.exit_code, 0);
        assert!(output.stdout.contains("restic_unavailable"));
        assert_eq!(
            last_backup_reason(journal.path()).as_deref(),
            Some("restic_unavailable")
        );
        assert_eq!(last_backup_status(journal.path()).as_deref(), Some("error"));
        assert!(downloader.calls.get() > 0);
        assert!(runner.programs.borrow().is_empty());
    }

    #[test]
    fn backup_run_success_twin_byo_and_operated() {
        let restic_dir = tempfile::tempdir().unwrap();
        let expected_restic = write_ready_restic(restic_dir.path());
        let byo = byo_journal();
        let byo_runner = RecordingRunner::new();
        let byo_output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            byo.path(),
            &byo_runner,
            &PanicDownload,
            &UnusedHttp,
            dirs(restic_dir.path(), None),
        );
        assert_eq!(byo_output.stdout, "backup: ok snapshot_id=snap\n");
        assert_eq!(byo_output.exit_code, 0);
        assert!(
            byo_runner
                .programs
                .borrow()
                .iter()
                .any(|program| program == expected_restic.as_os_str())
        );
        assert!(rclone_program(&byo_runner.argvs.borrow()).is_none());

        let rclone_dir = tempfile::tempdir().unwrap();
        let expected_rclone = write_ready_rclone(rclone_dir.path());
        let operated = operated_journal();
        let operated_runner = RecordingRunner::new();
        let broker = BrokerHttp::new();
        let operated_output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            operated.path(),
            &operated_runner,
            &PanicDownload,
            &broker,
            dirs(restic_dir.path(), Some(rclone_dir.path())),
        );
        assert_eq!(operated_output.stdout, "backup: ok snapshot_id=snap\n");
        assert_eq!(operated_output.exit_code, 0);
        assert_eq!(
            rclone_program(&operated_runner.argvs.borrow()).as_deref(),
            Some(expected_rclone.to_string_lossy().as_ref())
        );
        assert!(broker.calls.get() > 0);
    }

    #[test]
    fn backup_run_pins_admitted_byo_mode_despite_config_flip_to_operated_during_resolution() {
        let restic_dir = tempfile::tempdir().unwrap();
        write_ready_restic(restic_dir.path());
        let journal = byo_journal();
        let journal_path = journal.path().to_path_buf();
        let runner = RecordingRunner::with_on_first_call(move || {
            set_mode(&journal_path, "operated").unwrap();
        });
        let output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            journal.path(),
            &runner,
            &PanicDownload,
            &UnusedHttp,
            dirs(restic_dir.path(), None),
        );
        assert_eq!(output.stdout, "backup: ok snapshot_id=snap\n");
        assert!(rclone_program(&runner.argvs.borrow()).is_none());
    }

    #[test]
    fn backup_run_pins_admitted_operated_mode_despite_config_flip_to_byo_during_resolution() {
        let restic_dir = tempfile::tempdir().unwrap();
        let rclone_dir = tempfile::tempdir().unwrap();
        write_ready_restic(restic_dir.path());
        let expected_rclone = write_ready_rclone(rclone_dir.path());
        let journal = operated_journal();
        let journal_path = journal.path().to_path_buf();
        let runner = RecordingRunner::with_on_first_call(move || {
            set_mode(&journal_path, "byo").unwrap();
        });
        let broker = BrokerHttp::new();
        let output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            journal.path(),
            &runner,
            &PanicDownload,
            &broker,
            dirs(restic_dir.path(), Some(rclone_dir.path())),
        );
        assert_eq!(output.stdout, "backup: ok snapshot_id=snap\n");
        assert_eq!(
            rclone_program(&runner.argvs.borrow()).as_deref(),
            Some(expected_rclone.to_string_lossy().as_ref())
        );
        assert!(broker.calls.get() > 0);
    }

    #[cfg(unix)]
    #[test]
    fn backup_run_admission_resolves_relative_byo_alias_once_and_keeps_admitted_mode() {
        let _lock = CURRENT_DIRECTORY
            .lock()
            .expect("working directory lock holds");
        reset_backup_path_resolution_attempts();
        reset_backup_journal_resolved_hook();

        let neutral = tempfile::tempdir().expect("neutral directory creates");
        let journal_a = byo_journal_configured(
            "s3:source-bucket/source-prefix",
            "s3",
            "source-access",
            "source-secret",
        );
        let journal_b = operated_journal_configured(
            "https://replacement-broker.example.invalid",
            "replacement-account",
            "replacement-instance",
            "replacement-bucket",
            "replacement-prefix",
            "replacement-token",
        );
        let replacement_before = journal_config_bytes(journal_b.path());
        let alias = neutral.path().join("alias");
        symlink(journal_a.path(), &alias).expect("source alias creates");
        let restic_dir = tempfile::tempdir().expect("restic directory creates");
        let expected_restic = write_ready_restic(restic_dir.path());
        let journal_a_path = journal_a.path().to_path_buf();
        let runner = RecordingRunner::with_on_first_call(move || {
            set_mode(&journal_a_path, "operated").expect("source mode flips");
        });
        let _directory = CurrentDirectoryGuard::change_to(neutral.path());
        install_alias_retarget(
            alias,
            journal_b.path().to_path_buf(),
            journal_b.path().to_path_buf(),
            || {},
        );

        let output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            Path::new("alias"),
            &runner,
            &PanicDownload,
            &UnusedHttp,
            dirs(restic_dir.path(), None),
        );

        assert_eq!(output.stdout, "backup: ok snapshot_id=snap\n");
        assert_eq!(output.stderr, "");
        assert_eq!(output.exit_code, 0);
        assert!(
            runner
                .programs
                .borrow()
                .iter()
                .all(|program| program == expected_restic.as_os_str())
        );
        assert!(rclone_program(&runner.argvs.borrow()).is_none());
        assert_eq!(last_backup_status(journal_a.path()).as_deref(), Some("ok"));
        assert_alias_resolved_once();
        assert_eq!(journal_config_bytes(journal_b.path()), replacement_before);
    }

    #[cfg(unix)]
    #[test]
    fn backup_run_admission_resolves_relative_operated_alias_once_and_keeps_admitted_mode() {
        let _lock = CURRENT_DIRECTORY
            .lock()
            .expect("working directory lock holds");
        reset_backup_path_resolution_attempts();
        reset_backup_journal_resolved_hook();

        let neutral = tempfile::tempdir().expect("neutral directory creates");
        let journal_a = operated_journal_configured(
            "https://source-broker.example.invalid",
            "source-account",
            "source-instance",
            "source-bucket",
            "source-prefix",
            "source-token",
        );
        let journal_b = byo_journal_configured(
            "s3:replacement-bucket/replacement-prefix",
            "s3",
            "replacement-access",
            "replacement-secret",
        );
        let replacement_before = journal_config_bytes(journal_b.path());
        let alias = neutral.path().join("alias");
        symlink(journal_a.path(), &alias).expect("source alias creates");
        let restic_dir = tempfile::tempdir().expect("restic directory creates");
        let rclone_dir = tempfile::tempdir().expect("rclone directory creates");
        write_ready_restic(restic_dir.path());
        let expected_rclone = write_ready_rclone(rclone_dir.path());
        let journal_a_path = journal_a.path().to_path_buf();
        let runner = RecordingRunner::with_on_first_call(move || {
            set_mode(&journal_a_path, "byo").expect("source mode flips");
        });
        let broker = BrokerHttp::new();
        let _directory = CurrentDirectoryGuard::change_to(neutral.path());
        install_alias_retarget(
            alias,
            journal_b.path().to_path_buf(),
            journal_b.path().to_path_buf(),
            || {},
        );

        let output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            Path::new("alias"),
            &runner,
            &PanicDownload,
            &broker,
            dirs(restic_dir.path(), Some(rclone_dir.path())),
        );

        assert_eq!(output.stdout, "backup: ok snapshot_id=snap\n");
        assert_eq!(output.stderr, "");
        assert_eq!(output.exit_code, 0);
        assert_eq!(
            rclone_program(&runner.argvs.borrow()).as_deref(),
            Some(expected_rclone.to_string_lossy().as_ref())
        );
        assert_eq!(broker.calls.get(), 1);
        assert_eq!(last_backup_status(journal_a.path()).as_deref(), Some("ok"));
        assert_alias_resolved_once();
        assert_eq!(journal_config_bytes(journal_b.path()), replacement_before);
    }

    #[cfg(unix)]
    #[test]
    fn backup_run_admission_records_restic_unavailable_at_resolved_alias_once() {
        let _lock = CURRENT_DIRECTORY
            .lock()
            .expect("working directory lock holds");
        reset_backup_path_resolution_attempts();
        reset_backup_journal_resolved_hook();

        let neutral = tempfile::tempdir().expect("neutral directory creates");
        let journal_a = byo_journal_configured(
            "s3:source-bucket/source-prefix",
            "s3",
            "source-access",
            "source-secret",
        );
        let journal_b = operated_journal_configured(
            "https://replacement-broker.example.invalid",
            "replacement-account",
            "replacement-instance",
            "replacement-bucket",
            "replacement-prefix",
            "replacement-token",
        );
        let replacement_before = journal_config_bytes(journal_b.path());
        let alias = neutral.path().join("alias");
        symlink(journal_a.path(), &alias).expect("source alias creates");
        let restic_dir = tempfile::tempdir().expect("restic directory creates");
        let runner = RecordingRunner::new();
        let downloader = FailingDownload {
            calls: Cell::new(0),
        };
        let _directory = CurrentDirectoryGuard::change_to(neutral.path());
        install_alias_retarget(
            alias,
            journal_b.path().to_path_buf(),
            journal_b.path().to_path_buf(),
            || {},
        );

        let output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            Path::new("alias"),
            &runner,
            &downloader,
            &UnusedHttp,
            dirs(restic_dir.path(), None),
        );

        assert_restic_unavailable_output(&output);
        assert_eq!(
            last_backup_reason(journal_a.path()).as_deref(),
            Some("restic_unavailable")
        );
        assert_eq!(
            last_backup_status(journal_a.path()).as_deref(),
            Some("error")
        );
        assert!(downloader.calls.get() > 0);
        assert!(runner.programs.borrow().is_empty());
        assert_alias_resolved_once();
        assert_eq!(journal_config_bytes(journal_b.path()), replacement_before);
    }

    #[cfg(unix)]
    #[test]
    fn backup_run_admission_records_rclone_unavailable_at_resolved_alias_once() {
        let _lock = CURRENT_DIRECTORY
            .lock()
            .expect("working directory lock holds");
        reset_backup_path_resolution_attempts();
        reset_backup_journal_resolved_hook();

        let neutral = tempfile::tempdir().expect("neutral directory creates");
        let journal_a = operated_journal_configured(
            "https://source-broker.example.invalid",
            "source-account",
            "source-instance",
            "source-bucket",
            "source-prefix",
            "source-token",
        );
        let journal_b = byo_journal_configured(
            "s3:replacement-bucket/replacement-prefix",
            "s3",
            "replacement-access",
            "replacement-secret",
        );
        let replacement_before = journal_config_bytes(journal_b.path());
        let alias = neutral.path().join("alias");
        symlink(journal_a.path(), &alias).expect("source alias creates");
        let restic_dir = tempfile::tempdir().expect("restic directory creates");
        let rclone_dir = tempfile::tempdir().expect("rclone directory creates");
        write_ready_restic(restic_dir.path());
        let runner = RecordingRunner::new();
        let downloader = FailingDownload {
            calls: Cell::new(0),
        };
        let _directory = CurrentDirectoryGuard::change_to(neutral.path());
        install_alias_retarget(
            alias,
            journal_b.path().to_path_buf(),
            journal_b.path().to_path_buf(),
            || {},
        );

        let output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            Path::new("alias"),
            &runner,
            &downloader,
            &UnusedHttp,
            dirs(restic_dir.path(), Some(rclone_dir.path())),
        );

        assert_rclone_unavailable_output(&output);
        assert_eq!(
            last_backup_reason(journal_a.path()).as_deref(),
            Some("rclone_unavailable")
        );
        assert_eq!(
            last_backup_status(journal_a.path()).as_deref(),
            Some("error")
        );
        assert!(downloader.calls.get() > 0);
        assert_no_backup_execution(&runner);
        assert_alias_resolved_once();
        assert_eq!(journal_config_bytes(journal_b.path()), replacement_before);
    }

    #[cfg(unix)]
    #[test]
    fn backup_run_admission_restic_unavailable_ignores_record_mutation_failure() {
        let _lock = CURRENT_DIRECTORY
            .lock()
            .expect("working directory lock holds");
        reset_backup_path_resolution_attempts();
        reset_backup_journal_resolved_hook();

        let neutral = tempfile::tempdir().expect("neutral directory creates");
        let journal_a = byo_journal_configured(
            "s3:source-bucket/source-prefix",
            "s3",
            "source-access",
            "source-secret",
        );
        let journal_b = operated_journal_configured(
            "https://replacement-broker.example.invalid",
            "replacement-account",
            "replacement-instance",
            "replacement-bucket",
            "replacement-prefix",
            "replacement-token",
        );
        let replacement_before = journal_config_bytes(journal_b.path());
        let alias = neutral.path().join("alias");
        symlink(journal_a.path(), &alias).expect("source alias creates");
        let restic_dir = tempfile::tempdir().expect("restic directory creates");
        let runner = RecordingRunner::new();
        let downloader = FailingDownload {
            calls: Cell::new(0),
        };
        let read_only_config = Rc::new(RefCell::new(None));
        let held_read_only_config = Rc::clone(&read_only_config);
        let config_directory = journal_a.path().join("config");
        let _directory = CurrentDirectoryGuard::change_to(neutral.path());
        install_alias_retarget(
            alias,
            journal_b.path().to_path_buf(),
            journal_b.path().to_path_buf(),
            move || {
                *held_read_only_config.borrow_mut() =
                    Some(ReadOnlyDirectoryGuard::make_read_only(&config_directory));
            },
        );

        let output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            Path::new("alias"),
            &runner,
            &downloader,
            &UnusedHttp,
            dirs(restic_dir.path(), None),
        );

        assert_restic_unavailable_output(&output);
        assert!(downloader.calls.get() > 0);
        assert!(runner.programs.borrow().is_empty());
        assert_alias_resolved_once();
        assert_eq!(journal_config_bytes(journal_b.path()), replacement_before);
        drop(read_only_config.borrow_mut().take());
    }

    #[cfg(unix)]
    #[test]
    fn backup_run_admission_rclone_unavailable_ignores_record_mutation_failure() {
        let _lock = CURRENT_DIRECTORY
            .lock()
            .expect("working directory lock holds");
        reset_backup_path_resolution_attempts();
        reset_backup_journal_resolved_hook();

        let neutral = tempfile::tempdir().expect("neutral directory creates");
        let journal_a = operated_journal_configured(
            "https://source-broker.example.invalid",
            "source-account",
            "source-instance",
            "source-bucket",
            "source-prefix",
            "source-token",
        );
        let journal_b = byo_journal_configured(
            "s3:replacement-bucket/replacement-prefix",
            "s3",
            "replacement-access",
            "replacement-secret",
        );
        let replacement_before = journal_config_bytes(journal_b.path());
        let alias = neutral.path().join("alias");
        symlink(journal_a.path(), &alias).expect("source alias creates");
        let restic_dir = tempfile::tempdir().expect("restic directory creates");
        let rclone_dir = tempfile::tempdir().expect("rclone directory creates");
        write_ready_restic(restic_dir.path());
        let runner = RecordingRunner::new();
        let downloader = FailingDownload {
            calls: Cell::new(0),
        };
        let read_only_config = Rc::new(RefCell::new(None));
        let held_read_only_config = Rc::clone(&read_only_config);
        let config_directory = journal_a.path().join("config");
        let _directory = CurrentDirectoryGuard::change_to(neutral.path());
        install_alias_retarget(
            alias,
            journal_b.path().to_path_buf(),
            journal_b.path().to_path_buf(),
            move || {
                *held_read_only_config.borrow_mut() =
                    Some(ReadOnlyDirectoryGuard::make_read_only(&config_directory));
            },
        );

        let output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            Path::new("alias"),
            &runner,
            &downloader,
            &UnusedHttp,
            dirs(restic_dir.path(), Some(rclone_dir.path())),
        );

        assert_rclone_unavailable_output(&output);
        assert!(downloader.calls.get() > 0);
        assert_no_backup_execution(&runner);
        assert_alias_resolved_once();
        assert_eq!(journal_config_bytes(journal_b.path()), replacement_before);
        drop(read_only_config.borrow_mut().take());
    }

    #[cfg(unix)]
    #[test]
    fn backup_run_admission_skip_resolves_relative_alias_once() {
        let _lock = CURRENT_DIRECTORY
            .lock()
            .expect("working directory lock holds");
        reset_backup_path_resolution_attempts();
        reset_backup_journal_resolved_hook();

        let neutral = tempfile::tempdir().expect("neutral directory creates");
        let journal_a = tempfile::tempdir().expect("unconfigured journal creates");
        let journal_b = byo_journal_configured(
            "s3:replacement-bucket/replacement-prefix",
            "s3",
            "replacement-access",
            "replacement-secret",
        );
        let replacement_before = journal_config_bytes(journal_b.path());
        let alias = neutral.path().join("alias");
        symlink(journal_a.path(), &alias).expect("source alias creates");
        let runner = RecordingRunner::new();
        let _directory = CurrentDirectoryGuard::change_to(neutral.path());
        install_alias_retarget(
            alias,
            journal_b.path().to_path_buf(),
            journal_b.path().to_path_buf(),
            || {},
        );

        let output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            Path::new("alias"),
            &runner,
            &PanicDownload,
            &UnusedHttp,
            ToolInstallDirs::default(),
        );

        assert_eq!(output.stdout, "backup: skipped\n");
        assert_eq!(output.stderr, "");
        assert_eq!(output.exit_code, 0);
        assert!(runner.programs.borrow().is_empty());
        assert_alias_resolved_once();
        assert_eq!(journal_config_bytes(journal_b.path()), replacement_before);
    }

    #[cfg(unix)]
    #[test]
    fn backup_run_admission_unresolved_relative_alias_attempts_once() {
        let _lock = CURRENT_DIRECTORY
            .lock()
            .expect("working directory lock holds");
        reset_backup_path_resolution_attempts();
        reset_backup_journal_resolved_hook();

        let neutral = tempfile::tempdir().expect("neutral directory creates");
        let journal_a = byo_journal_configured(
            "s3:source-bucket/source-prefix",
            "s3",
            "source-access",
            "source-secret",
        );
        let missing_source = journal_a.path().to_path_buf();
        journal_a.close().expect("source journal removes");
        let journal_b = operated_journal_configured(
            "https://replacement-broker.example.invalid",
            "replacement-account",
            "replacement-instance",
            "replacement-bucket",
            "replacement-prefix",
            "replacement-token",
        );
        let replacement_before = journal_config_bytes(journal_b.path());
        let alias = neutral.path().join("alias");
        symlink(&missing_source, &alias).expect("dangling source alias creates");
        let runner = RecordingRunner::new();
        let _directory = CurrentDirectoryGuard::change_to(neutral.path());

        let output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            Path::new("alias"),
            &runner,
            &PanicDownload,
            &UnusedHttp,
            ToolInstallDirs::default(),
        );

        assert_eq!(
            output.stdout,
            "backup: error reason=journal_path_unresolved\n"
        );
        assert_eq!(output.stderr, "");
        assert_eq!(output.exit_code, 0);
        assert!(runner.programs.borrow().is_empty());
        assert_alias_resolved_once();
        assert_eq!(journal_config_bytes(journal_b.path()), replacement_before);
    }

    #[cfg(unix)]
    #[test]
    fn backup_run_admission_config_error_resolves_relative_alias_once() {
        let _lock = CURRENT_DIRECTORY
            .lock()
            .expect("working directory lock holds");
        reset_backup_path_resolution_attempts();
        reset_backup_journal_resolved_hook();

        let neutral = tempfile::tempdir().expect("neutral directory creates");
        let journal_a = tempfile::tempdir().expect("malformed journal creates");
        fs::create_dir_all(journal_a.path().join("config")).expect("config directory creates");
        fs::write(journal_a.path().join("config/journal.json"), b"{")
            .expect("malformed config writes");
        let journal_b = byo_journal_configured(
            "s3:replacement-bucket/replacement-prefix",
            "s3",
            "replacement-access",
            "replacement-secret",
        );
        let replacement_before = journal_config_bytes(journal_b.path());
        let source_before = journal_config_bytes(journal_a.path());
        let alias = neutral.path().join("alias");
        symlink(journal_a.path(), &alias).expect("source alias creates");
        let runner = RecordingRunner::new();
        let _directory = CurrentDirectoryGuard::change_to(neutral.path());
        install_alias_retarget(
            alias,
            journal_b.path().to_path_buf(),
            journal_b.path().to_path_buf(),
            || {},
        );

        let output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            Path::new("alias"),
            &runner,
            &PanicDownload,
            &UnusedHttp,
            ToolInstallDirs::default(),
        );

        assert_eq!(output.stdout, "backup: error reason=broker_error\n");
        assert_eq!(output.stderr, "");
        assert_eq!(output.exit_code, 0);
        assert!(runner.programs.borrow().is_empty());
        assert_alias_resolved_once();
        assert_eq!(journal_config_bytes(journal_a.path()), source_before);
        assert_eq!(journal_config_bytes(journal_b.path()), replacement_before);
    }

    #[test]
    fn backup_run_admission_terminals_do_not_resolve_tools() {
        let runner = RecordingRunner::new();
        let skipped_journal = tempfile::tempdir().unwrap();
        let skipped = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            skipped_journal.path(),
            &runner,
            &PanicDownload,
            &UnusedHttp,
            ToolInstallDirs::default(),
        );
        assert_eq!(skipped.stdout, "backup: skipped\n");
        assert_eq!(skipped.exit_code, 0);
        assert!(runner.programs.borrow().is_empty());

        let runner = RecordingRunner::new();
        let unresolved_journal = tempfile::tempdir().unwrap();
        let unresolved = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            &unresolved_journal.path().join("missing"),
            &runner,
            &PanicDownload,
            &UnusedHttp,
            ToolInstallDirs::default(),
        );
        assert_eq!(
            unresolved.stdout,
            "backup: error reason=journal_path_unresolved\n"
        );
        assert_eq!(unresolved.exit_code, 0);
        assert!(runner.programs.borrow().is_empty());

        let runner = RecordingRunner::new();
        let config_error_journal = tempfile::tempdir().unwrap();
        fs::create_dir_all(config_error_journal.path().join("config")).unwrap();
        fs::write(
            config_error_journal.path().join("config/journal.json"),
            b"{",
        )
        .unwrap();
        let config_error = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            config_error_journal.path(),
            &runner,
            &PanicDownload,
            &UnusedHttp,
            ToolInstallDirs::default(),
        );
        assert_eq!(config_error.stdout, "backup: error reason=broker_error\n");
        assert_eq!(config_error.exit_code, 0);
        assert!(runner.programs.borrow().is_empty());
    }

    #[test]
    fn run_cli_with_deps_excludes_invalid_and_help_forms_from_capability_path() {
        let journal = tempfile::tempdir().unwrap();
        for (args, expected_exit, expected_usage) in [
            (
                args(&["run", "backup:run", "extra"]),
                2,
                "usage: journal maintenance run backup:run [-h]\n",
            ),
            (
                args(&["run", "-h"]),
                0,
                "usage: journal maintenance run ID [ARGS...]\n",
            ),
        ] {
            let runner = RecordingRunner::new();
            let clock = ProductionClock;
            let maintenance = NativeJournalMaintenance;
            let placeholder = BackupServices {
                runner: &runner,
                http: &UnusedHttp,
                clock: &clock,
                restic_path: None,
                rclone_path: None,
                version: env!("CARGO_PKG_VERSION"),
                journal_maintenance: &maintenance,
            };
            let services = MaintenanceServices::new(crate::registry::routines());
            let expected = super::run_cli_with_all_services(
                &args,
                journal.path(),
                &services,
                Some(&placeholder),
                None,
                None,
            );
            let output = run_cli_with_deps(
                &args,
                journal.path(),
                &runner,
                &PanicDownload,
                &UnusedHttp,
                ToolInstallDirs::default(),
            );
            assert_eq!(output, expected);
            assert_eq!(output.exit_code, expected_exit);
            if expected_exit == 0 {
                assert_eq!(output.stdout, expected_usage);
            } else {
                assert!(output.stderr.starts_with(expected_usage));
            }
            assert!(runner.programs.borrow().is_empty());
        }
    }

    // Fixture is pre-installed: ensure_rclone's zip verification checks RCLONE_ZIP_SHA256, a real published-release checksum no offline fixture can satisfy, so this proves the resolved path is wired into the spawn, not the install cycle itself.
    #[test]
    fn ac4_operated_run_passes_absolute_rclone_program() {
        let restic_dir = tempfile::tempdir().unwrap();
        let rclone_dir = tempfile::tempdir().unwrap();
        let decoy_dir = tempfile::tempdir().unwrap();
        write_ready_restic(restic_dir.path());
        let expected_rclone = write_ready_rclone(rclone_dir.path());
        let decoy = decoy_dir.path().join("rclone");
        fs::write(&decoy, b"decoy").unwrap();
        let journal = operated_journal();
        let runner = RecordingRunner::new();
        run_cli_with_deps(
            &args(&["run", "backup:run"]),
            journal.path(),
            &runner,
            &PanicDownload,
            &BrokerHttp::new(),
            dirs(restic_dir.path(), Some(rclone_dir.path())),
        );
        let program = rclone_program(&runner.argvs.borrow()).expect("rclone.program");
        assert_eq!(program, expected_rclone.display().to_string());
        assert_ne!(program, decoy.display().to_string());
        assert_ne!(program, "rclone");
    }

    // Fixture is pre-installed: ensure_rclone's zip verification checks RCLONE_ZIP_SHA256, a real published-release checksum no offline fixture can satisfy, so this proves the resolved path is wired into the spawn, not the install cycle itself.
    #[test]
    fn ac5_offload_passes_absolute_rclone_program() {
        let restic_dir = tempfile::tempdir().unwrap();
        let rclone_dir = tempfile::tempdir().unwrap();
        let decoy_dir = tempfile::tempdir().unwrap();
        write_ready_restic(restic_dir.path());
        let expected_rclone = write_ready_rclone(rclone_dir.path());
        let decoy = decoy_dir.path().join("rclone");
        fs::write(&decoy, b"decoy").unwrap();
        let journal = operated_journal();
        configure_offload(journal.path());
        let runner = RecordingRunner::new();
        run_cli_with_deps(
            &args(&["run", "backup:offload"]),
            journal.path(),
            &runner,
            &PanicDownload,
            &BrokerHttp::new(),
            dirs(restic_dir.path(), Some(rclone_dir.path())),
        );
        let program = rclone_program(&runner.argvs.borrow()).expect("rclone.program");
        assert_eq!(program, expected_rclone.display().to_string());
        assert_ne!(program, decoy.display().to_string());
        assert_ne!(program, "rclone");
    }

    #[test]
    fn ac6_operated_run_persists_rclone_unavailable() {
        let restic_dir = tempfile::tempdir().unwrap();
        let rclone_dir = tempfile::tempdir().unwrap();
        write_ready_restic(restic_dir.path());
        let journal = operated_journal();
        record_backup_result(journal.path(), "ok", json!(1), json!("prior"), Value::Null).unwrap();
        let runner = RecordingRunner::new();
        let downloader = FailingDownload {
            calls: Cell::new(0),
        };
        let output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            journal.path(),
            &runner,
            &downloader,
            &BrokerHttp::new(),
            dirs(restic_dir.path(), Some(rclone_dir.path())),
        );
        assert_eq!(output.exit_code, 0);
        assert!(output.stdout.contains("rclone_unavailable"));
        assert_eq!(
            last_backup_reason(journal.path()).as_deref(),
            Some("rclone_unavailable")
        );
        assert!(downloader.calls.get() > 0);
        assert!(
            runner
                .programs
                .borrow()
                .iter()
                .all(|program| program != "rclone")
        );
    }

    #[test]
    fn ac7_byo_backup_routines_resolve_restic_without_rclone() {
        let restic_dir = tempfile::tempdir().unwrap();
        let decoy_dir = tempfile::tempdir().unwrap();
        let expected = write_ready_restic(restic_dir.path());
        let decoy = decoy_dir.path().join("restic");
        fs::write(&decoy, b"decoy").unwrap();
        let journal = tempfile::tempdir().unwrap();
        for argv in [
            args(&["run", "backup:prune"]),
            args(&["run", "backup:verify"]),
            args(&["run", "backup:offload"]),
            args(&["run", "backup:offload", "--dry-run"]),
        ] {
            let runner = RecordingRunner::new();
            run_cli_with_deps(
                &argv,
                journal.path(),
                &runner,
                &PanicDownload,
                &UnusedHttp,
                dirs(restic_dir.path(), None),
            );
            assert_resolved_restic(&runner.programs.borrow(), &expected, &decoy);
        }
    }

    #[test]
    fn ac8_operated_prune_and_verify_do_not_resolve_rclone() {
        let restic_dir = tempfile::tempdir().unwrap();
        let decoy_dir = tempfile::tempdir().unwrap();
        let expected = write_ready_restic(restic_dir.path());
        let decoy = decoy_dir.path().join("restic");
        fs::write(&decoy, b"decoy").unwrap();
        let journal = operated_journal();
        for argv in [
            args(&["run", "backup:prune"]),
            args(&["run", "backup:verify"]),
        ] {
            let runner = RecordingRunner::new();
            run_cli_with_deps(
                &argv,
                journal.path(),
                &runner,
                &PanicDownload,
                &BrokerHttp::new(),
                dirs(restic_dir.path(), None),
            );
            assert_resolved_restic(&runner.programs.borrow(), &expected, &decoy);
        }
    }

    #[test]
    fn ac11_list_does_not_resolve_tools() {
        let journal = tempfile::tempdir().unwrap();
        let runner = RecordingRunner::new();
        run_cli_with_deps(
            &args(&["list"]),
            journal.path(),
            &runner,
            &PanicDownload,
            &UnusedHttp,
            ToolInstallDirs::default(),
        );
        assert!(runner.programs.borrow().is_empty());
    }

    #[test]
    fn injected_service_entry_points_execute_backup_run() {
        for entry_point in ["backup", "services", "all"] {
            let journal = byo_journal();
            let runner = RecordingRunner::new();
            let http = UnusedHttp;
            let clock = ProductionClock;
            let maintenance = NativeJournalMaintenance;
            let backup_services = BackupServices {
                runner: &runner,
                http: &http,
                clock: &clock,
                restic_path: Some(Path::new("/fixture/bin/restic")),
                rclone_path: None,
                version: "test",
                journal_maintenance: &maintenance,
            };
            let services = MaintenanceServices::new(crate::registry::routines());
            let run_args = args(&["run", "backup:run"]);
            let output = match entry_point {
                "backup" => {
                    run_cli_with_backup(&run_args, journal.path(), &services, &backup_services)
                }
                "services" => run_cli_with_services(
                    &run_args,
                    journal.path(),
                    &services,
                    Some(&backup_services),
                    None,
                ),
                "all" => run_cli_with_all_services(
                    &run_args,
                    journal.path(),
                    &services,
                    Some(&backup_services),
                    None,
                    None,
                ),
                _ => unreachable!("fixed entry point"),
            };
            assert_eq!(output.stdout, "backup: ok snapshot_id=snap\n");
            assert_eq!(output.exit_code, 0);
        }
    }

    #[test]
    fn injected_service_entry_points_render_backup_run_admission_terminals_without_tools() {
        for (terminal, expected) in [
            ("skip", "backup: skipped\n"),
            (
                "unresolved",
                "backup: error reason=journal_path_unresolved\n",
            ),
            ("config-error", "backup: error reason=broker_error\n"),
        ] {
            for entry_point in ["backup", "services", "all"] {
                let journal = tempfile::tempdir().expect("terminal journal creates");
                let journal_path = match terminal {
                    "skip" => journal.path().to_path_buf(),
                    "unresolved" => journal.path().join("missing"),
                    "config-error" => {
                        fs::create_dir_all(journal.path().join("config"))
                            .expect("config directory creates");
                        fs::write(journal.path().join("config/journal.json"), b"{")
                            .expect("malformed config writes");
                        journal.path().to_path_buf()
                    }
                    _ => unreachable!("fixed terminal"),
                };
                let runner = RecordingRunner::new();
                let http = UnusedHttp;
                let clock = ProductionClock;
                let maintenance = NativeJournalMaintenance;
                let backup_services = BackupServices {
                    runner: &runner,
                    http: &http,
                    clock: &clock,
                    restic_path: Some(Path::new("/fixture/bin/restic")),
                    rclone_path: None,
                    version: "test",
                    journal_maintenance: &maintenance,
                };
                let services = MaintenanceServices::new(crate::registry::routines());
                let run_args = args(&["run", "backup:run"]);
                let output = match entry_point {
                    "backup" => {
                        run_cli_with_backup(&run_args, &journal_path, &services, &backup_services)
                    }
                    "services" => run_cli_with_services(
                        &run_args,
                        &journal_path,
                        &services,
                        Some(&backup_services),
                        None,
                    ),
                    "all" => run_cli_with_all_services(
                        &run_args,
                        &journal_path,
                        &services,
                        Some(&backup_services),
                        None,
                        None,
                    ),
                    _ => unreachable!("fixed entry point"),
                };
                assert_eq!(output.stdout, expected, "{terminal}/{entry_point}");
                assert_eq!(output.stderr, "", "{terminal}/{entry_point}");
                assert_eq!(output.exit_code, 0, "{terminal}/{entry_point}");
                assert!(
                    runner.programs.borrow().is_empty(),
                    "{terminal}/{entry_point}"
                );
            }
        }
    }
}
