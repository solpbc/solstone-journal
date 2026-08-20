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
    ToolInstallDirs, ToolRunner, UreqHttpTransport, record_backup_error, resolve_operational_tools,
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
        restic_path: Path::new("restic"),
        rclone_path: None,
        version: env!("CARGO_PKG_VERSION"),
        journal_maintenance: &restore_hooks,
    };
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
                        restic_path: &tools.restic_path,
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
                Err(reason) => format_backup_resolution_error(args, journal, &clock, &reason),
            }
        }
    }
}

fn format_backup_resolution_error(
    args: &[String],
    journal: &Path,
    clock: &dyn Clock,
    reason: &str,
) -> CliRun {
    let id = classify_maintenance_routine_id(args);
    let line = match id.as_deref() {
        Some("backup:run") => {
            let result = record_backup_error(journal, clock, reason);
            return backup_routine_line(match result.status.as_str() {
                "ok" => format!(
                    "backup: ok snapshot_id={}",
                    result.snapshot_id.as_deref().unwrap_or("None")
                ),
                "skipped" => "backup: skipped".to_owned(),
                _ => format!(
                    "backup: error reason={}",
                    result.error_reason.as_deref().unwrap_or("None")
                ),
            });
        }
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
            details: vec![],
        }),
        _ => format!("backup: error reason={reason}"),
    };
    backup_routine_line(line)
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
        "backup:run" if forwarded.is_empty() => Some(true),
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
            restic_path: Path::new("restic"),
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
            ("backup:run", "backup:", 0),
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
        HostedBinding, generate_and_store_keys, get_backup_config, record_backup_result,
        record_verification_result, save_hosted_binding, set_enabled, set_mode, set_offload,
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
    };
    use std::cell::{Cell, RefCell};
    use std::ffi::OsString;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    struct RecordingRunner {
        programs: RefCell<Vec<OsString>>,
        argvs: RefCell<Vec<Vec<String>>>,
    }

    impl RecordingRunner {
        fn new() -> Self {
            Self {
                programs: RefCell::new(vec![]),
                argvs: RefCell::new(vec![]),
            }
        }
    }

    impl ToolRunner for RecordingRunner {
        fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
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

    struct BrokerHttp;

    impl HttpTransport for BrokerHttp {
        fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
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

    fn operated_journal() -> tempfile::TempDir {
        let journal = tempfile::tempdir().unwrap();
        set_mode(journal.path(), "operated").unwrap();
        set_enabled(journal.path(), true).unwrap();
        generate_and_store_keys(journal.path()).unwrap();
        save_hosted_binding(
            journal.path(),
            &HostedBinding {
                broker_endpoint: "https://broker.example.invalid".into(),
                account_id: "account".into(),
                instance_id: "instance".into(),
                bucket: "bucket".into(),
                prefix: "prefix".into(),
                broker_token: "token".into(),
            },
        )
        .unwrap();
        journal
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
        let journal = tempfile::tempdir().unwrap();
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
        let journal = tempfile::tempdir().unwrap();
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
            &BrokerHttp,
            dirs(restic_dir.path(), Some(rclone_dir.path())),
        );
        let program = rclone_program(&runner.argvs.borrow()).expect("rclone.program");
        assert_eq!(program, expected_rclone.display().to_string());
        assert_ne!(program, decoy.display().to_string());
        assert_ne!(program, "rclone");
    }

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
            &BrokerHttp,
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
            &BrokerHttp,
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
            args(&["run", "backup:run"]),
            args(&["run", "backup:prune"]),
            args(&["run", "backup:verify"]),
            args(&["run", "backup:offload"]),
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
                &BrokerHttp,
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
}
