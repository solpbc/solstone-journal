// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native registry and command surface for recurring journal maintenance.

pub mod bodies;
mod parser;
pub mod registry;
pub mod schedule_sync;

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Datelike, Utc};
use registry::{RoutineDescriptor, routines};
use solstone_core_backup_runtime::{
    BackupServices, Clock, ClosedToolError, HttpTransport, NativeJournalMaintenance,
    SystemToolRunner, ToolRunner, UreqHttpTransport, prepare, resolve_operational_tools,
    resolve_tools,
};
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
    #[cfg(windows)]
    discovery_generation: Option<&'a solstone_core_system::process::ChildLaunchContext>,
}

/// Injectable time and zone for health routines: `zone` is the journal's owner
/// zone in production, the same zone removal approval re-checks a date in.
pub struct HealthServices {
    pub now: DateTime<Utc>,
    pub zone: chrono_tz::Tz,
}

impl<'a> MaintenanceServices<'a> {
    pub const fn new(routines: &'a [RoutineDescriptor]) -> Self {
        Self {
            routines,
            #[cfg(windows)]
            discovery_generation: None,
        }
    }
}

/// Run the production maintenance command parser.
pub fn run_cli(args: &[String], journal: &Path) -> CliRun {
    let executable = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            let msg = err.to_string();
            let clock = ProductionClock;
            let error = ClosedToolError::ResticUnavailable {
                detail: msg.clone(),
                guidance: msg,
            };
            if is_bare_backup_run(args)
                && let Ok(capability) = prepare(journal, &clock)
            {
                return bodies::backup::backup_run_result(
                    capability.record_tool_error(&clock, error),
                );
            }
            return format_backup_resolution_error(args, &error);
        }
    };
    let http = UreqHttpTransport;
    run_cli_with_deps(args, journal, &SystemToolRunner, &http, &executable)
}

/// Select only an actual discovery body; help and other maintenance verbs need no generation.
#[cfg(windows)]
pub fn is_discovery_run(args: &[String]) -> bool {
    let args = strip_maintenance_global_flags(args);
    if maintenance_has_help(&args) {
        return false;
    }
    let Some(rest) = args.strip_prefix(&["run".to_owned(), "speakers:discover-voices".to_owned()])
    else {
        return false;
    };
    rest.is_empty() || rest == ["--"]
}

/// The executable has acquired or authenticated and journal-validated this generation.
#[cfg(windows)]
pub fn run_cli_with_discovery_generation(
    args: &[String],
    journal: &Path,
    generation: &solstone_core_system::process::ChildLaunchContext,
) -> CliRun {
    let services = MaintenanceServices {
        routines: routines(),
        discovery_generation: Some(generation),
    };
    parser::run(args, journal, &services, None, None)
}

fn run_cli_with_deps(
    args: &[String],
    journal: &Path,
    runner: &dyn ToolRunner,
    http: &dyn HttpTransport,
    executable: &Path,
) -> CliRun {
    let clock = ProductionClock;
    let restore_hooks = NativeJournalMaintenance;
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
        return run_admitted_backup(journal, &clock, runner, executable, placeholder);
    }
    let maintenance_services = MaintenanceServices::new(routines());
    let health = HealthServices {
        now,
        zone: solstone_core_journal_config::owner_zone(journal),
    };
    match classify_maintenance_tool_resolution(args) {
        None => run_cli_with_services(
            args,
            journal,
            &maintenance_services,
            Some(&placeholder),
            Some(&health),
        ),
        Some(append_only) => {
            match resolve_operational_tools(runner, journal, append_only, executable) {
                Ok(tools) => {
                    let backup_services = BackupServices {
                        restic_path: Some(&tools.restic_path),
                        rclone_path: tools.rclone_path.as_deref(),
                        ..placeholder
                    };
                    run_cli_with_services(
                        args,
                        journal,
                        &maintenance_services,
                        Some(&backup_services),
                        Some(&health),
                    )
                }
                // A journal without backup turned on skips these bodies before
                // running any tool, so a missing tool is not its error.
                Err(_) if !solstone_core_backup_runtime::backup_enabled(journal) => {
                    run_cli_with_services(
                        args,
                        journal,
                        &maintenance_services,
                        Some(&placeholder),
                        Some(&health),
                    )
                }
                Err(error) => format_backup_resolution_error(args, &error),
            }
        }
    }
}

fn format_backup_resolution_error(args: &[String], error: &ClosedToolError) -> CliRun {
    let outer = error.to_string();
    let detail = error.detail();
    let id = classify_maintenance_routine_id(args);
    let line = match id.as_deref() {
        Some("backup:prune") => format!("backup prune: error reason={outer} detail={detail}"),
        Some("backup:verify") => format!("backup verify: error reason={outer} detail={detail}"),
        Some("backup:offload") => format_offload_result(&OffloadResult {
            status: "stalled".into(),
            reason: Some(outer),
            files_marked: 0,
            bytes_marked: 0,
            files_already_marked: 0,
            bytes_already_marked: 0,
            ran_out_of_markable_media: false,
            dry_run: false,
            reason_detail: Some(detail.to_owned()),
            details: vec![],
            audit_recording_failure: None,
            recording_failure: None,
        }),
        _ => format!("backup: error reason={outer} detail={detail}"),
    };
    CliRun {
        stdout: format!("{line}\n"),
        stderr: String::new(),
        exit_code: i32::from(id.as_deref() != Some("backup:offload")),
    }
}

fn run_admitted_backup(
    journal: &Path,
    clock: &dyn Clock,
    runner: &dyn ToolRunner,
    executable: &Path,
    placeholder: BackupServices<'_>,
) -> CliRun {
    match prepare(journal, clock) {
        Ok(capability) => match resolve_tools(&capability, runner, executable) {
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
    parser::run(args, journal, services, None, None)
}

/// Run the maintenance parser with an injected backup runtime service set.
pub fn run_cli_with_backup(
    args: &[String],
    journal: &Path,
    services: &MaintenanceServices<'_>,
    backup_services: &BackupServices<'_>,
) -> CliRun {
    parser::run(args, journal, services, Some(backup_services), None)
}

/// Run the maintenance parser with injected backup and health routine services.
pub fn run_cli_with_services(
    args: &[String],
    journal: &Path,
    services: &MaintenanceServices<'_>,
    backup_services: Option<&BackupServices<'_>>,
    health_services: Option<&HealthServices>,
) -> CliRun {
    parser::run(args, journal, services, backup_services, health_services)
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

    use super::{HealthServices, MaintenanceServices, registry, run_cli_with_services};
    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use solstone_core_backup_runtime::hosted_runtime::HttpError;
    use solstone_core_backup_runtime::{
        BackupServices, Clock, HttpRequest, HttpResponse, HttpTransport, JournalMaintenance,
        JournalMaintenanceError, ToolOutput, ToolRequest, ToolRunner,
    };

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
            zone: chrono_tz::Tz::UTC,
        };
        let services = MaintenanceServices::new(registry::routines());
        let run = |arguments: &[&str]| {
            run_cli_with_services(
                &arguments
                    .iter()
                    .map(|argument| (*argument).to_owned())
                    .collect::<Vec<_>>(),
                journal.path(),
                &services,
                Some(&backup),
                Some(&health),
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
        ] {
            let mut args = vec!["run", id];
            if id == "backup:offload" {
                args.push("--dry-run");
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
    use solstone_core_backup::{
        Destination, HostedBinding, generate_and_store_keys, get_backup_config,
        record_backup_result, record_verification_result, save_hosted_binding, set_destination,
        set_enabled, set_mode, set_offload,
    };
    use solstone_core_backup_runtime::hosted_runtime::{HttpError, HttpRequest, HttpResponse};
    #[cfg(all(test, feature = "full-tests"))]
    use solstone_core_backup_runtime::install_backup_tool_resolution_started_hook;
    use solstone_core_backup_runtime::{
        HttpTransport, ToolOutput, ToolRequest, ToolRunner, backup_journal_resolved_hook_armed,
        backup_path_resolution_attempts, install_backup_journal_resolved_hook,
        reset_backup_journal_resolved_hook, reset_backup_path_resolution_attempts,
    };
    #[cfg(all(test, feature = "full-tests"))]
    use solstone_core_backup_runtime::{
        backup_record_failure_hook_armed, backup_record_failure_hook_consumed_target,
        backup_tool_resolution_started_hook_armed, install_backup_record_failure_hook,
        reset_backup_record_failure_hook, reset_backup_tool_resolution_started_hook,
    };
    use solstone_core_installed_payload::code;
    use std::cell::{Cell, RefCell};
    use std::ffi::OsString;
    use std::fs;
    use std::io;
    #[cfg(unix)]
    #[cfg(all(test, feature = "full-tests"))]
    use std::os::unix::fs::MetadataExt;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};
    #[cfg(unix)]
    #[cfg(all(test, feature = "full-tests"))]
    use std::rc::Rc;
    use std::sync::{LazyLock, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

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
            let stdout = if version && name == "rclone" {
                "rclone v0.19.0\n".to_owned()
            } else if version {
                "restic 0.19.0\n".to_owned()
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

    fn write_file(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
    }

    fn build_fixture_tree(root: &Path, restic_bytes: &[u8], rclone_bytes: &[u8]) -> PathBuf {
        use solstone_core_installed_payload::{
            COMPILED_VERSION, INSTALLED_PAYLOAD_MANIFEST, PRODUCT, compiled_target,
            render_installed_payload,
        };
        let exe = root.join("bin/solstone");
        write_file(root, "bin/solstone", b"launcher");
        write_file(root, "lib/solstone-restic/restic", restic_bytes);
        write_file(root, "lib/solstone-rclone/rclone", rclone_bytes);
        let manifest = render_installed_payload(
            root,
            PRODUCT,
            COMPILED_VERSION,
            compiled_target(),
            "fixture-commit",
        )
        .unwrap();
        write_file(root, INSTALLED_PAYLOAD_MANIFEST, &manifest);
        exe
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

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    fn install_tool_resolution_alias_retarget(
        alias: PathBuf,
        replacement: PathBuf,
        next_directory: PathBuf,
        after_retarget: impl FnOnce(&Path) + 'static,
    ) {
        install_backup_tool_resolution_started_hook(move |resolved_journal| {
            fs::remove_file(&alias).expect("source alias removes");
            symlink(&replacement, &alias).expect("replacement alias creates");
            std::env::set_current_dir(&next_directory)
                .expect("working directory changes after tool resolution starts");
            after_retarget(resolved_journal);
        });
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[derive(Debug, PartialEq, Eq)]
    struct FileMetadataSnapshot {
        device: u64,
        inode: u64,
        file_type: fs::FileType,
        mode: u32,
        len: u64,
        mtime: i64,
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[derive(Debug, PartialEq, Eq)]
    struct ConfigStateSnapshot {
        journal_config_bytes: Vec<u8>,
        directory: FileMetadataSnapshot,
        entries: Vec<(String, FileMetadataSnapshot)>,
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    fn file_metadata_snapshot(path: &Path) -> FileMetadataSnapshot {
        let metadata = fs::symlink_metadata(path).expect("config metadata reads");
        FileMetadataSnapshot {
            device: metadata.dev(),
            inode: metadata.ino(),
            file_type: metadata.file_type(),
            mode: metadata.mode(),
            len: metadata.len(),
            mtime: metadata.mtime(),
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    fn config_state_snapshot(journal: &Path) -> ConfigStateSnapshot {
        let config = journal.join("config");
        let mut entries: Vec<_> = fs::read_dir(&config)
            .expect("config directory reads")
            .map(|entry| {
                let entry = entry.expect("config entry reads");
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    file_metadata_snapshot(&entry.path()),
                )
            })
            .collect();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        ConfigStateSnapshot {
            journal_config_bytes: journal_config_bytes(journal),
            directory: file_metadata_snapshot(&config),
            entries,
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn assert_restic_unavailable_output(output: &CliRun) {
        assert_eq!(
            output.stdout,
            format!(
                "backup: error reason=restic_unavailable detail={}\n",
                code::MANIFEST_MISSING
            )
        );
        assert_eq!(output.stderr, "");
        assert_eq!(output.exit_code, 1);
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn assert_rclone_unavailable_output(output: &CliRun) {
        assert_eq!(
            output.stdout,
            format!(
                "backup: error reason=rclone_unavailable detail={}\n",
                code::MEMBER_MISSING
            )
        );
        assert_eq!(output.stderr, "");
        assert_eq!(output.exit_code, 1);
    }

    #[cfg(all(test, feature = "full-tests"))]
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
        record_backup_result(journal, "ok", json!(now), json!("ready"), Value::Null, None).unwrap();
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
        let package_dir = tempfile::tempdir().unwrap();
        let decoy_dir = tempfile::tempdir().unwrap();
        let exe = build_fixture_tree(package_dir.path(), b"restic", b"rclone");
        let expected = package_dir.path().join("lib/solstone-restic/restic");
        let decoy = decoy_dir.path().join("restic");
        fs::write(&decoy, b"decoy").unwrap();
        let journal = byo_journal();
        let runner = RecordingRunner::new();
        run_cli_with_deps(
            &args(&["run", "backup:run"]),
            journal.path(),
            &runner,
            &UnusedHttp,
            &exe,
        );
        assert_resolved_restic(&runner.programs.borrow(), &expected, &decoy);
    }

    #[test]
    fn ac3_backup_run_persists_restic_unavailable() {
        let temp_dir = tempfile::tempdir().unwrap();
        let bin_dir = temp_dir.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let temp_exe = bin_dir.join("solstone");
        fs::write(&temp_exe, b"launcher").unwrap();
        let journal = byo_journal();
        record_backup_result(
            journal.path(),
            "ok",
            json!(1),
            json!("prior"),
            Value::Null,
            None,
        )
        .unwrap();
        let runner = RecordingRunner::new();
        let output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            journal.path(),
            &runner,
            &UnusedHttp,
            &temp_exe,
        );
        assert_eq!(output.exit_code, 1);
        assert!(output.stdout.contains("restic_unavailable"));
        assert_eq!(
            last_backup_reason(journal.path()).as_deref(),
            Some("restic_unavailable")
        );
        assert_eq!(last_backup_status(journal.path()).as_deref(), Some("error"));
        let config = get_backup_config(journal.path()).unwrap();
        assert_eq!(
            config["last_backup"]["detail"].as_str(),
            Some(code::MANIFEST_MISSING)
        );
        assert!(runner.programs.borrow().is_empty());
    }

    #[test]
    fn backup_run_success_twin_byo_and_operated() {
        let package_dir = tempfile::tempdir().unwrap();
        let exe = build_fixture_tree(package_dir.path(), b"restic", b"rclone");
        let expected_restic = package_dir.path().join("lib/solstone-restic/restic");
        let expected_rclone = package_dir.path().join("lib/solstone-rclone/rclone");
        let byo = byo_journal();
        let byo_runner = RecordingRunner::new();
        let byo_output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            byo.path(),
            &byo_runner,
            &UnusedHttp,
            &exe,
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

        let operated = operated_journal();
        let operated_runner = RecordingRunner::new();
        let broker = BrokerHttp::new();
        let operated_output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            operated.path(),
            &operated_runner,
            &broker,
            &exe,
        );
        assert_eq!(operated_output.stdout, "backup: ok snapshot_id=snap\n");
        assert_eq!(operated_output.exit_code, 0);
        assert_eq!(
            rclone_program(&operated_runner.argvs.borrow()),
            Some(format!("\"{}\"", expected_rclone.display()))
        );
        assert!(broker.calls.get() > 0);
    }

    #[test]
    fn backup_run_pins_admitted_byo_mode_despite_config_flip_to_operated_during_resolution() {
        let package_dir = tempfile::tempdir().unwrap();
        let exe = build_fixture_tree(package_dir.path(), b"restic", b"rclone");
        let journal = byo_journal();
        let journal_path = journal.path().to_path_buf();
        let runner = RecordingRunner::with_on_first_call(move || {
            set_mode(&journal_path, "operated").unwrap();
        });
        let output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            journal.path(),
            &runner,
            &UnusedHttp,
            &exe,
        );
        assert_eq!(output.stdout, "backup: ok snapshot_id=snap\n");
        assert!(rclone_program(&runner.argvs.borrow()).is_none());
    }

    #[test]
    fn backup_run_pins_admitted_operated_mode_despite_config_flip_to_byo_during_resolution() {
        let package_dir = tempfile::tempdir().unwrap();
        let exe = build_fixture_tree(package_dir.path(), b"restic", b"rclone");
        let expected_rclone = package_dir.path().join("lib/solstone-rclone/rclone");
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
            &broker,
            &exe,
        );
        assert_eq!(output.stdout, "backup: ok snapshot_id=snap\n");
        assert_eq!(
            rclone_program(&runner.argvs.borrow()),
            Some(format!("\"{}\"", expected_rclone.display()))
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
        let package_dir = tempfile::tempdir().expect("package directory creates");
        let exe = build_fixture_tree(package_dir.path(), b"restic", b"rclone");
        let expected_restic = package_dir.path().join("lib/solstone-restic/restic");
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
            &UnusedHttp,
            &exe,
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
        let package_dir = tempfile::tempdir().expect("package directory creates");
        let exe = build_fixture_tree(package_dir.path(), b"restic", b"rclone");
        let expected_rclone = package_dir.path().join("lib/solstone-rclone/rclone");
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
            &broker,
            &exe,
        );

        assert_eq!(output.stdout, "backup: ok snapshot_id=snap\n");
        assert_eq!(output.stderr, "");
        assert_eq!(output.exit_code, 0);
        assert_eq!(
            rclone_program(&runner.argvs.borrow()),
            Some(format!("\"{}\"", expected_rclone.display()))
        );
        assert_eq!(broker.calls.get(), 1);
        assert_eq!(last_backup_status(journal_a.path()).as_deref(), Some("ok"));
        assert_alias_resolved_once();
        assert_eq!(journal_config_bytes(journal_b.path()), replacement_before);
    }

    #[cfg(all(test, feature = "full-tests"))]
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
        let temp_dir = tempfile::tempdir().expect("temp dir creates");
        let bin_dir = temp_dir.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let temp_exe = bin_dir.join("solstone");
        fs::write(&temp_exe, b"launcher").unwrap();
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
            &UnusedHttp,
            &temp_exe,
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
        assert_eq!(
            get_backup_config(journal_a.path()).unwrap()["last_backup"]["detail"].as_str(),
            Some(code::MANIFEST_MISSING)
        );
        assert!(runner.programs.borrow().is_empty());
        assert_alias_resolved_once();
        assert_eq!(journal_config_bytes(journal_b.path()), replacement_before);
    }

    #[cfg(all(test, feature = "full-tests"))]
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
        let package_dir = tempfile::tempdir().expect("package directory creates");
        let exe = build_fixture_tree(package_dir.path(), b"restic", b"rclone");
        fs::remove_file(package_dir.path().join("lib/solstone-rclone/rclone")).unwrap();
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
            &UnusedHttp,
            &exe,
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
        assert_eq!(
            get_backup_config(journal_a.path()).unwrap()["last_backup"]["detail"].as_str(),
            Some(code::MEMBER_MISSING)
        );
        assert_no_backup_execution(&runner);
        assert_alias_resolved_once();
        assert_eq!(journal_config_bytes(journal_b.path()), replacement_before);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn backup_run_admission_restic_unavailable_ignores_record_mutation_failure() {
        let _lock = CURRENT_DIRECTORY
            .lock()
            .expect("working directory lock holds");
        reset_backup_path_resolution_attempts();
        reset_backup_journal_resolved_hook();
        reset_backup_tool_resolution_started_hook();
        reset_backup_record_failure_hook();

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
        let canonical_a = fs::canonicalize(journal_a.path()).expect("source journal resolves");
        let alias = neutral.path().join("alias");
        symlink(journal_a.path(), &alias).expect("source alias creates");
        let temp_dir = tempfile::tempdir().expect("temp dir creates");
        let bin_dir = temp_dir.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let temp_exe = bin_dir.join("solstone");
        fs::write(&temp_exe, b"launcher").unwrap();
        let runner = RecordingRunner::new();
        let checkpoint_calls = Rc::new(Cell::new(0));
        let hook_calls = Rc::clone(&checkpoint_calls);
        let snapshots = Rc::new(RefCell::new(None));
        let hook_snapshots = Rc::clone(&snapshots);
        let source_path = journal_a.path().to_path_buf();
        let replacement_path = journal_b.path().to_path_buf();
        let hook_canonical_a = canonical_a.clone();
        let _directory = CurrentDirectoryGuard::change_to(neutral.path());
        install_backup_record_failure_hook(canonical_a.clone());
        install_tool_resolution_alias_retarget(
            alias,
            journal_b.path().to_path_buf(),
            journal_b.path().to_path_buf(),
            move |resolved_journal| {
                assert_eq!(resolved_journal, hook_canonical_a.as_path());
                hook_calls.set(hook_calls.get() + 1);
                *hook_snapshots.borrow_mut() = Some((
                    config_state_snapshot(&source_path),
                    config_state_snapshot(&replacement_path),
                ));
            },
        );

        let output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            Path::new("alias"),
            &runner,
            &UnusedHttp,
            &temp_exe,
        );

        assert_restic_unavailable_output(&output);
        assert!(runner.programs.borrow().is_empty());
        assert_alias_resolved_once();
        assert_eq!(checkpoint_calls.get(), 1);
        assert!(!backup_tool_resolution_started_hook_armed());
        assert!(!backup_record_failure_hook_armed());
        assert_eq!(
            backup_record_failure_hook_consumed_target().as_deref(),
            Some(canonical_a.as_path())
        );
        let (source_before, replacement_before) = snapshots
            .borrow_mut()
            .take()
            .expect("tool-resolution snapshots capture");
        assert_eq!(config_state_snapshot(journal_a.path()), source_before);
        assert_eq!(config_state_snapshot(journal_b.path()), replacement_before);
        reset_backup_record_failure_hook();
        reset_backup_tool_resolution_started_hook();
        reset_backup_journal_resolved_hook();
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn backup_run_admission_rclone_unavailable_ignores_record_mutation_failure() {
        let _lock = CURRENT_DIRECTORY
            .lock()
            .expect("working directory lock holds");
        reset_backup_path_resolution_attempts();
        reset_backup_journal_resolved_hook();
        reset_backup_tool_resolution_started_hook();
        reset_backup_record_failure_hook();

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
        let canonical_a = fs::canonicalize(journal_a.path()).expect("source journal resolves");
        let alias = neutral.path().join("alias");
        symlink(journal_a.path(), &alias).expect("source alias creates");
        let package_dir = tempfile::tempdir().expect("package directory creates");
        let exe = build_fixture_tree(package_dir.path(), b"restic", b"rclone");
        fs::remove_file(package_dir.path().join("lib/solstone-rclone/rclone")).unwrap();
        let runner = RecordingRunner::new();
        let checkpoint_calls = Rc::new(Cell::new(0));
        let hook_calls = Rc::clone(&checkpoint_calls);
        let snapshots = Rc::new(RefCell::new(None));
        let hook_snapshots = Rc::clone(&snapshots);
        let source_path = journal_a.path().to_path_buf();
        let replacement_path = journal_b.path().to_path_buf();
        let hook_canonical_a = canonical_a.clone();
        let _directory = CurrentDirectoryGuard::change_to(neutral.path());
        install_backup_record_failure_hook(canonical_a.clone());
        install_tool_resolution_alias_retarget(
            alias,
            journal_b.path().to_path_buf(),
            journal_b.path().to_path_buf(),
            move |resolved_journal| {
                assert_eq!(resolved_journal, hook_canonical_a.as_path());
                hook_calls.set(hook_calls.get() + 1);
                *hook_snapshots.borrow_mut() = Some((
                    config_state_snapshot(&source_path),
                    config_state_snapshot(&replacement_path),
                ));
            },
        );

        let output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            Path::new("alias"),
            &runner,
            &UnusedHttp,
            &exe,
        );

        assert_rclone_unavailable_output(&output);
        assert_no_backup_execution(&runner);
        assert_alias_resolved_once();
        assert_eq!(checkpoint_calls.get(), 1);
        assert!(!backup_tool_resolution_started_hook_armed());
        assert!(!backup_record_failure_hook_armed());
        assert_eq!(
            backup_record_failure_hook_consumed_target().as_deref(),
            Some(canonical_a.as_path())
        );
        let (source_before, replacement_before) = snapshots
            .borrow_mut()
            .take()
            .expect("tool-resolution snapshots capture");
        assert_eq!(config_state_snapshot(journal_a.path()), source_before);
        assert_eq!(config_state_snapshot(journal_b.path()), replacement_before);
        reset_backup_record_failure_hook();
        reset_backup_tool_resolution_started_hook();
        reset_backup_journal_resolved_hook();
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
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_exe = temp_dir.path().join("fake_exe");
        fs::write(&temp_exe, b"placeholder").unwrap();
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
            &UnusedHttp,
            &temp_exe,
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
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_exe = temp_dir.path().join("fake_exe");
        fs::write(&temp_exe, b"placeholder").unwrap();
        let _directory = CurrentDirectoryGuard::change_to(neutral.path());

        let output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            Path::new("alias"),
            &runner,
            &UnusedHttp,
            &temp_exe,
        );

        assert_eq!(
            output.stdout,
            "backup: error reason=journal_path_unresolved\n"
        );
        assert_eq!(output.stderr, "");
        assert_eq!(output.exit_code, 1);
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
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_exe = temp_dir.path().join("fake_exe");
        fs::write(&temp_exe, b"placeholder").unwrap();
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
            &UnusedHttp,
            &temp_exe,
        );

        assert_eq!(output.stdout, "backup: error reason=broker_error\n");
        assert_eq!(output.stderr, "");
        assert_eq!(output.exit_code, 1);
        assert!(runner.programs.borrow().is_empty());
        assert_alias_resolved_once();
        assert_eq!(journal_config_bytes(journal_a.path()), source_before);
        assert_eq!(journal_config_bytes(journal_b.path()), replacement_before);
    }

    #[test]
    fn backup_run_admission_terminals_do_not_resolve_tools() {
        let runner = RecordingRunner::new();
        let skipped_journal = tempfile::tempdir().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_exe = temp_dir.path().join("fake_exe");
        fs::write(&temp_exe, b"placeholder").unwrap();
        let skipped = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            skipped_journal.path(),
            &runner,
            &UnusedHttp,
            &temp_exe,
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
            &UnusedHttp,
            &temp_exe,
        );
        assert_eq!(
            unresolved.stdout,
            "backup: error reason=journal_path_unresolved\n"
        );
        assert_eq!(unresolved.exit_code, 1);
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
            &UnusedHttp,
            &temp_exe,
        );
        assert_eq!(config_error.stdout, "backup: error reason=broker_error\n");
        assert_eq!(config_error.exit_code, 1);
        assert!(runner.programs.borrow().is_empty());
    }

    #[test]
    fn run_cli_with_deps_excludes_invalid_and_help_forms_from_capability_path() {
        let journal = tempfile::tempdir().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_exe = temp_dir.path().join("fake_exe");
        fs::write(&temp_exe, b"placeholder").unwrap();
        for (args, expected_exit, expected_usage) in [
            (
                args(&["run", "backup:run", "extra"]),
                2,
                "usage: solstone journal maintenance run backup:run [-h]\n",
            ),
            (
                args(&["run", "-h"]),
                0,
                "usage: solstone journal maintenance run ID [ARGS...]\n",
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
            let expected = super::run_cli_with_services(
                &args,
                journal.path(),
                &services,
                Some(&placeholder),
                None,
            );
            let output = run_cli_with_deps(&args, journal.path(), &runner, &UnusedHttp, &temp_exe);
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

    // The operated run passes the verified rclone member path.
    #[test]
    fn ac4_operated_run_passes_absolute_rclone_program() {
        let package_dir = tempfile::tempdir().unwrap();
        let decoy_dir = tempfile::tempdir().unwrap();
        let exe = build_fixture_tree(package_dir.path(), b"restic", b"rclone");
        let expected_rclone = package_dir.path().join("lib/solstone-rclone/rclone");
        let decoy = decoy_dir.path().join("rclone");
        fs::write(&decoy, b"decoy").unwrap();
        let journal = operated_journal();
        let runner = RecordingRunner::new();
        run_cli_with_deps(
            &args(&["run", "backup:run"]),
            journal.path(),
            &runner,
            &BrokerHttp::new(),
            &exe,
        );
        let program = rclone_program(&runner.argvs.borrow()).expect("rclone.program");
        assert_eq!(program, format!("\"{}\"", expected_rclone.display()));
        assert_ne!(program, decoy.display().to_string());
        assert_ne!(program, "rclone");
    }

    // The operated run passes the verified rclone member path.
    #[test]
    fn ac5_offload_passes_absolute_rclone_program() {
        let package_dir = tempfile::tempdir().unwrap();
        let decoy_dir = tempfile::tempdir().unwrap();
        let exe = build_fixture_tree(package_dir.path(), b"restic", b"rclone");
        let expected_rclone = package_dir.path().join("lib/solstone-rclone/rclone");
        let decoy = decoy_dir.path().join("rclone");
        fs::write(&decoy, b"decoy").unwrap();
        let journal = operated_journal();
        configure_offload(journal.path());
        let runner = RecordingRunner::new();
        run_cli_with_deps(
            &args(&["run", "backup:offload"]),
            journal.path(),
            &runner,
            &BrokerHttp::new(),
            &exe,
        );
        let program = rclone_program(&runner.argvs.borrow()).expect("rclone.program");
        assert_eq!(program, format!("\"{}\"", expected_rclone.display()));
        assert_ne!(program, decoy.display().to_string());
        assert_ne!(program, "rclone");
    }

    #[test]
    fn ac6_operated_run_persists_rclone_unavailable() {
        let package_dir = tempfile::tempdir().unwrap();
        let exe = build_fixture_tree(package_dir.path(), b"restic", b"rclone");
        fs::remove_file(package_dir.path().join("lib/solstone-rclone/rclone")).unwrap();
        let journal = operated_journal();
        record_backup_result(
            journal.path(),
            "ok",
            json!(1),
            json!("prior"),
            Value::Null,
            None,
        )
        .unwrap();
        let runner = RecordingRunner::new();
        let output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            journal.path(),
            &runner,
            &BrokerHttp::new(),
            &exe,
        );
        assert_eq!(output.exit_code, 1);
        assert!(output.stdout.contains("rclone_unavailable"));
        assert_eq!(
            last_backup_reason(journal.path()).as_deref(),
            Some("rclone_unavailable")
        );
        assert_eq!(
            get_backup_config(journal.path()).unwrap()["last_backup"]["detail"].as_str(),
            Some(code::MEMBER_MISSING)
        );
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
        let package_dir = tempfile::tempdir().unwrap();
        let decoy_dir = tempfile::tempdir().unwrap();
        let exe = build_fixture_tree(package_dir.path(), b"restic", b"rclone");
        let expected = package_dir.path().join("lib/solstone-restic/restic");
        let decoy = decoy_dir.path().join("restic");
        fs::write(&decoy, b"decoy").unwrap();
        for argv in [
            args(&["run", "backup:prune"]),
            args(&["run", "backup:verify"]),
        ] {
            let journal = byo_journal();
            let runner = RecordingRunner::new();
            run_cli_with_deps(&argv, journal.path(), &runner, &UnusedHttp, &exe);
            assert_resolved_restic(&runner.programs.borrow(), &expected, &decoy);
        }

        let journal = byo_journal();
        configure_offload(journal.path());
        for argv in [
            args(&["run", "backup:offload"]),
            args(&["run", "backup:offload", "--dry-run"]),
        ] {
            assert_eq!(classify_maintenance_tool_resolution(&argv), Some(true));
            let runner = RecordingRunner::new();
            let tools = resolve_operational_tools(&runner, journal.path(), true, &exe)
                .expect("offload resolves its pinned restic dependency");
            assert_eq!(tools.restic_path, expected);
            assert!(tools.rclone_path.is_none());
        }
    }

    #[test]
    fn ac8_operated_prune_and_verify_do_not_resolve_rclone() {
        let package_dir = tempfile::tempdir().unwrap();
        let decoy_dir = tempfile::tempdir().unwrap();
        let exe = build_fixture_tree(package_dir.path(), b"restic", b"rclone");
        let expected = package_dir.path().join("lib/solstone-restic/restic");
        let decoy = decoy_dir.path().join("restic");
        fs::write(&decoy, b"decoy").unwrap();
        let journal = operated_journal();
        for argv in [
            args(&["run", "backup:prune"]),
            args(&["run", "backup:verify"]),
        ] {
            let runner = RecordingRunner::new();
            run_cli_with_deps(&argv, journal.path(), &runner, &BrokerHttp::new(), &exe);
            assert_resolved_restic(&runner.programs.borrow(), &expected, &decoy);
        }
    }

    #[test]
    fn ac11_list_does_not_resolve_tools() {
        let journal = tempfile::tempdir().unwrap();
        let runner = RecordingRunner::new();
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_exe = temp_dir.path().join("fake_exe");
        fs::write(&temp_exe, b"placeholder").unwrap();
        run_cli_with_deps(
            &args(&["list"]),
            journal.path(),
            &runner,
            &UnusedHttp,
            &temp_exe,
        );
        assert!(runner.programs.borrow().is_empty());
    }

    #[test]
    fn injected_service_entry_points_execute_backup_run() {
        for entry_point in ["backup", "services"] {
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
            for entry_point in ["backup", "services"] {
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
                    _ => unreachable!("fixed entry point"),
                };
                assert_eq!(output.stdout, expected, "{terminal}/{entry_point}");
                assert_eq!(output.stderr, "", "{terminal}/{entry_point}");
                assert_eq!(
                    output.exit_code,
                    i32::from(terminal != "skip"),
                    "{terminal}/{entry_point}"
                );
                assert!(
                    runner.programs.borrow().is_empty(),
                    "{terminal}/{entry_point}"
                );
            }
        }
    }

    struct PlantedFallbacks {
        _home: tempfile::TempDir,
        _path_dir: tempfile::TempDir,
        _bundle_dir: tempfile::TempDir,
        script_dirs: Vec<PathBuf>,
        recorded_files: Vec<(PathBuf, Vec<u8>)>,
        prev_home: Option<std::ffi::OsString>,
        prev_path: Option<std::ffi::OsString>,
        prev_restic_bundle: Option<std::ffi::OsString>,
        prev_rclone_bundle: Option<std::ffi::OsString>,
    }

    #[allow(unsafe_code)]
    impl Drop for PlantedFallbacks {
        fn drop(&mut self) {
            unsafe {
                if let Some(ref val) = self.prev_home {
                    std::env::set_var("HOME", val);
                } else {
                    std::env::remove_var("HOME");
                }
                if let Some(ref val) = self.prev_path {
                    std::env::set_var("PATH", val);
                } else {
                    std::env::remove_var("PATH");
                }
                if let Some(ref val) = self.prev_restic_bundle {
                    std::env::set_var("SOLSTONE_RESTIC_BUNDLE", val);
                } else {
                    std::env::remove_var("SOLSTONE_RESTIC_BUNDLE");
                }
                if let Some(ref val) = self.prev_rclone_bundle {
                    std::env::set_var("SOLSTONE_RCLONE_BUNDLE", val);
                } else {
                    std::env::remove_var("SOLSTONE_RCLONE_BUNDLE");
                }
            }
        }
    }

    impl PlantedFallbacks {
        fn verify(&self) {
            for dir in &self.script_dirs {
                assert!(
                    !dir.join("marker").exists(),
                    "marker file was created in {:?}",
                    dir
                );
            }
            for (path, expected_bytes) in &self.recorded_files {
                let actual = fs::read(path).unwrap_or_else(|_| panic!("read {:?}", path));
                assert_eq!(&actual, expected_bytes, "file {:?} was modified", path);
            }
        }
    }

    #[allow(unsafe_code)]
    fn plant_fallbacks(pkg_root: &Path) -> PlantedFallbacks {
        let home = tempfile::tempdir().unwrap();
        let path_dir = tempfile::tempdir().unwrap();
        let bundle_dir = tempfile::tempdir().unwrap();

        let prev_home = std::env::var_os("HOME");
        let prev_path = std::env::var_os("PATH");
        let prev_restic_bundle = std::env::var_os("SOLSTONE_RESTIC_BUNDLE");
        let prev_rclone_bundle = std::env::var_os("SOLSTONE_RCLONE_BUNDLE");

        let script_content = b"#!/bin/sh\ntouch \"$(dirname \"$0\")/marker\"\n";
        let sentinel_content = b"sentinel-complete";
        let license_content = b"LICENSE-TEXT";

        let mut recorded_files = Vec::new();
        let mut script_dirs = Vec::new();

        let mut plant = |dir: &Path, name: &str| -> PathBuf {
            fs::create_dir_all(dir).unwrap();
            let script = dir.join(name);
            fs::write(&script, script_content).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
            }
            recorded_files.push((script.clone(), script_content.to_vec()));

            let sentinel = dir.join(".install-complete");
            fs::write(&sentinel, sentinel_content).unwrap();
            recorded_files.push((sentinel, sentinel_content.to_vec()));

            let license = dir.join("LICENSE");
            fs::write(&license, license_content).unwrap();
            recorded_files.push((license, license_content.to_vec()));

            if !script_dirs.contains(&dir.to_path_buf()) {
                script_dirs.push(dir.to_path_buf());
            }
            script
        };

        let cache_dir = home.path().join(".cache/solstone");
        plant(&cache_dir, "restic");
        plant(&cache_dir, "rclone");

        let app_support_dir = home.path().join("Library/Application Support/solstone");
        plant(&app_support_dir, "restic");
        plant(&app_support_dir, "rclone");

        let pkg_bin_dir = pkg_root.join("_bin");
        plant(&pkg_bin_dir, "restic");
        plant(&pkg_bin_dir, "rclone");

        let restic_bundle = plant(bundle_dir.path(), "bundle-restic");
        let rclone_bundle = plant(bundle_dir.path(), "bundle-rclone");

        plant(path_dir.path(), "restic");
        plant(path_dir.path(), "rclone");

        unsafe {
            std::env::set_var("HOME", home.path());
            std::env::set_var("SOLSTONE_RESTIC_BUNDLE", &restic_bundle);
            std::env::set_var("SOLSTONE_RCLONE_BUNDLE", &rclone_bundle);
        }

        let new_path = if let Some(ref old) = prev_path {
            let mut p = path_dir.path().as_os_str().to_os_string();
            p.push(":");
            p.push(old);
            p
        } else {
            path_dir.path().as_os_str().to_os_string()
        };
        unsafe {
            std::env::set_var("PATH", new_path);
        }

        PlantedFallbacks {
            _home: home,
            _path_dir: path_dir,
            _bundle_dir: bundle_dir,
            script_dirs,
            recorded_files,
            prev_home,
            prev_path,
            prev_restic_bundle,
            prev_rclone_bundle,
        }
    }

    #[test]
    fn fallback_negative_ignores_markers_and_env() {
        let _lock = CURRENT_DIRECTORY.lock().unwrap();

        let pkg = tempfile::tempdir().unwrap();
        let exe = pkg.path().join("bin/solstone");
        write_file(pkg.path(), "bin/solstone", b"launcher");

        let fallbacks = plant_fallbacks(pkg.path());

        let journal = byo_journal();
        let runner = RecordingRunner::new();
        let output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            journal.path(),
            &runner,
            &UnusedHttp,
            &exe,
        );

        assert_eq!(output.exit_code, 1);
        assert!(output.stdout.contains("reason=restic_unavailable"));
        assert!(output.stdout.contains("detail=manifest-missing"));
        assert!(runner.programs.borrow().is_empty());
        fallbacks.verify();
    }

    #[test]
    fn altered_member_refuses_backup_run() {
        use solstone_core_installed_payload::code;

        let pkg = tempfile::tempdir().unwrap();
        let exe = build_fixture_tree(pkg.path(), b"restic", b"rclone");

        // Flip one byte of lib/solstone-restic/restic and keep same length
        let restic_path = pkg.path().join("lib/solstone-restic/restic");
        let mut bytes = fs::read(&restic_path).unwrap();
        bytes[0] ^= 0xFF;
        fs::write(&restic_path, bytes).unwrap();

        let journal = byo_journal();
        let runner = RecordingRunner::new();
        let output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            journal.path(),
            &runner,
            &UnusedHttp,
            &exe,
        );

        assert_eq!(output.exit_code, 1);
        assert!(output.stdout.contains("reason=restic_unavailable"));
        assert!(
            output
                .stdout
                .contains(&format!("detail={}", code::MEMBER_CHANGED))
        );
        assert_eq!(
            last_backup_reason(journal.path()).as_deref(),
            Some("restic_unavailable")
        );
        let config = get_backup_config(journal.path()).unwrap();
        assert_eq!(
            config["last_backup"]["detail"].as_str(),
            Some(code::MEMBER_CHANGED)
        );
        assert!(runner.programs.borrow().is_empty());
    }

    #[test]
    fn scheduled_restart_record_and_output() {
        use solstone_core_installed_payload::{
            INSTALLED_PAYLOAD_MANIFEST, PRODUCT, code, compiled_target, render_installed_payload,
        };

        let pkg = tempfile::tempdir().unwrap();
        let exe = pkg.path().join("bin/solstone");
        write_file(pkg.path(), "bin/solstone", b"launcher");
        write_file(pkg.path(), "lib/solstone-restic/restic", b"restic");
        write_file(pkg.path(), "lib/solstone-rclone/rclone", b"rclone");
        let manifest = render_installed_payload(
            pkg.path(),
            PRODUCT,
            "999.0.0",
            compiled_target(),
            "fixture-commit",
        )
        .unwrap();
        write_file(pkg.path(), INSTALLED_PAYLOAD_MANIFEST, &manifest);

        let journal = byo_journal();
        let runner = RecordingRunner::new();
        let output = run_cli_with_deps(
            &args(&["run", "backup:run"]),
            journal.path(),
            &runner,
            &UnusedHttp,
            &exe,
        );

        assert_eq!(output.exit_code, 1);
        assert!(output.stdout.contains("reason=restic_unavailable"));
        assert!(
            output
                .stdout
                .contains(&format!("detail={}", code::RESTART_TO_FINISH_UPDATE))
        );
        assert_eq!(
            last_backup_reason(journal.path()).as_deref(),
            Some("restic_unavailable")
        );
        let config = get_backup_config(journal.path()).unwrap();
        assert_eq!(
            config["last_backup"]["detail"].as_str(),
            Some(code::RESTART_TO_FINISH_UPDATE)
        );
        assert!(runner.programs.borrow().is_empty());
    }
}
