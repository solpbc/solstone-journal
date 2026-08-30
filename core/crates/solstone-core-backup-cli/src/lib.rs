// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{self, IsTerminal, Read};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Datelike, NaiveDate, Utc};
use serde_json::{Map, Value, json};
use solstone_core_artifact_download::{ByteDownload, UreqByteDownload};
use solstone_core_backup::{
    Destination, HostedBinding, confirm_recovery_key, format_recovery_key_display,
    generate_and_store_keys, generate_daily_key, get_backup_config, get_destination, get_keys,
    save_hosted_binding, set_destination, set_enabled, set_recovery_key_confirmed, status_view,
};
use solstone_core_backup_runtime::restore::{
    RESTORE_REASON_INTEGRITY_FAILED, RESTORE_REASON_INTEGRITY_UNVERIFIED,
    RESTORE_REASON_RESTORE_RECORD_FAILED, RESTORE_REASON_RESTORE_SUMMARY_MISSING,
};
use solstone_core_backup_runtime::{
    BackupResult, BackupServices, Clock, HttpTransport, NativeJournalMaintenance,
    NativeRestoreRecorder, PruneResult, ResticKeyError, RestoreDraft, RestoreOutcome,
    RestoreRecorder, RotationResult, SystemToolRunner, TeardownResult, ToolInstallDirs, ToolRunner,
    UreqHttpTransport, ensure_restic, prepare, publish_restore_outcome, resolve_operational_tools,
    resolve_tools, restore_journal, rotate_recovery_key, run_prune, teardown_backup,
    validate_destination,
};
use solstone_core_offload::{
    OffloadResult, RestoreResult as OffloadRestoreResult, build_offload_status,
    format_offload_result, restore_all_offload, restore_offload_day, run_offload,
};

pub const USAGE: &str = "usage: journal backup <command> [options]\n";
pub const DESTINATION_USAGE: &str = "usage: journal backup destination <command> [options]\n";
pub const OFFLOAD_USAGE: &str = "usage: journal backup offload <command> [options]\n";
pub const RECOVERY_KEY_USAGE: &str = "usage: journal backup recovery-key <command> [options]\n";
const MAX_JSON_STDIN_BYTES: usize = 1024 * 1024; // Keep in lockstep with solstone-core main.rs.

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

pub fn run_cli(args: &[String], journal: &Path) -> CliRun {
    let http = UreqHttpTransport;
    let recorder = NativeRestoreRecorder;
    run_cli_with_deps(
        args,
        journal,
        &SystemToolRunner,
        &UreqByteDownload,
        &http,
        ToolInstallDirs::default(),
        &recorder,
    )
}

fn run_cli_with_deps(
    args: &[String],
    journal: &Path,
    runner: &dyn ToolRunner,
    downloader: &dyn ByteDownload,
    http: &dyn HttpTransport,
    dirs: ToolInstallDirs<'_>,
    recorder: &dyn RestoreRecorder,
) -> CliRun {
    let clock = ProductionClock;
    let maintenance = NativeJournalMaintenance;
    let placeholder = BackupServices {
        runner,
        http,
        clock: &clock,
        restic_path: None,
        rclone_path: None,
        version: env!("CARGO_PKG_VERSION"),
        journal_maintenance: &maintenance,
    };
    if is_bare_backup_run(args) {
        return run_admitted_backup(journal, &clock, runner, downloader, dirs, placeholder);
    }
    match classify_tool_resolution(args) {
        None => run_cli_with(args, journal, &placeholder, recorder),
        Some(append_only) => {
            match resolve_operational_tools(runner, downloader, journal, append_only, dirs) {
                Ok(tools) => {
                    let services = BackupServices {
                        restic_path: Some(&tools.restic_path),
                        rclone_path: tools.rclone_path.as_deref(),
                        ..placeholder
                    };
                    run_cli_with(args, journal, &services, recorder)
                }
                Err(reason) => format_resolution_error(args, journal, &clock, recorder, &reason),
            }
        }
    }
}

fn format_resolution_error(
    args: &[String],
    journal: &Path,
    clock: &dyn Clock,
    recorder: &dyn RestoreRecorder,
    reason: &str,
) -> CliRun {
    let args = normalize_global_flags(args);
    match args.first().map(String::as_str) {
        Some("prune") => backup_prune_result(PruneResult {
            status: "error".into(),
            error_reason: Some(reason.to_owned()),
        }),
        Some("restore") => {
            let json_output = restore_options(&args[1..]).expect("classified restore options");
            restore_result(
                publish_restore_outcome(
                    journal,
                    clock,
                    recorder,
                    RestoreDraft {
                        status: "error".into(),
                        reason_code: Some(reason.to_owned()),
                        integrity_ok: false,
                        resumable: false,
                        files_expected: None,
                        files_restored: None,
                        bytes_expected: None,
                        bytes_restored: None,
                    },
                ),
                json_output,
            )
        }
        Some("recovery-key") => recovery_key_rotate_result(RotationResult {
            status: "error".into(),
            reason_code: Some(reason.to_owned()),
            recovery_key: None,
            recovery_key_display: None,
        }),
        Some("off") => teardown_result(TeardownResult {
            status: "error".into(),
            reason_code: Some(reason.to_owned()),
        }),
        Some("offload") => match args.get(1).map(String::as_str) {
            Some("run") => offload_run_result(OffloadResult {
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
            Some("restore") => offload_restore_result(
                OffloadRestoreResult {
                    status: "error".into(),
                    reason: Some(reason.to_owned()),
                    scope: "day".into(),
                    day: None,
                    segments_selected: 0,
                    segments_restored: 0,
                    files_expected: 0,
                    files_restored: 0,
                    bytes_expected: 0,
                    bytes_restored: 0,
                    reason_detail: None,
                    details: vec![],
                },
                false,
            ),
            _ => runtime_error(reason.to_owned()),
        },
        _ => runtime_error(reason.to_owned()),
    }
}

fn is_bare_backup_run(args: &[String]) -> bool {
    let args = normalize_global_flags(args);
    if has_help(&args) {
        return false;
    }
    matches!(args.split_first(), Some((command, rest)) if command == "run" && no_positionals(rest))
}

/// Keep in sync with the `run_cli_with` match below. `Some(append_only)` means
/// this argv reaches an operational restic/rclone verb and must resolve pinned
/// tools first.
fn classify_tool_resolution(args: &[String]) -> Option<bool> {
    let args = normalize_global_flags(args);
    if has_help(&args) {
        return None;
    }
    let (command, rest) = args.split_first()?;
    match command.as_str() {
        "prune" if no_positionals(rest) => Some(false),
        "restore" => restore_options(rest).map(|_| false),
        "recovery-key" => {
            let (options, positionals) = split_terminator(rest);
            match options {
                [subcommand] if subcommand == "rotate" && positionals.is_empty() => Some(false),
                _ => None,
            }
        }
        "off" => {
            let (options, positionals) = split_terminator(rest);
            if !positionals.is_empty() || options.iter().any(|argument| argument != "--yes") {
                return None;
            }
            options
                .iter()
                .any(|argument| argument == "--yes")
                .then_some(false)
        }
        "offload" => {
            let (subcommand, options) = rest.split_first()?;
            match subcommand.as_str() {
                "run" => {
                    let (flags, positionals) = split_terminator(options);
                    if !positionals.is_empty() || flags.iter().any(|flag| flag != "--dry-run") {
                        None
                    } else {
                        Some(true)
                    }
                }
                "restore" => Some(false),
                _ => None,
            }
        }
        _ => None,
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

fn run_cli_with(
    args: &[String],
    journal: &Path,
    services: &BackupServices<'_>,
    recorder: &dyn RestoreRecorder,
) -> CliRun {
    let args = normalize_global_flags(args);
    if has_help(&args) {
        return success(usage_for_scope(&args).to_owned());
    }
    let Some((command, rest)) = args.split_first() else {
        return usage_error("");
    };
    match command.as_str() {
        "status" if no_positionals(rest) => render_json(status_view(journal).map(Value::Object)),
        "destination" => destination_command(rest, journal, services),
        "recovery-key" => recovery_key_command(rest, journal, services),
        "enable" if no_positionals(rest) => enable(journal, services),
        "prune" if no_positionals(rest) => backup_prune(journal, services),
        "offload" => offload_command(rest, journal, services),
        "off" => off_command(rest, journal, services),
        "restore" => restore_command(rest, journal, services, recorder),
        _ => usage_error(&args.join(" ")),
    }
}

fn normalize_global_flags(args: &[String]) -> Vec<String> {
    let mut options = true;
    let mut normalized = Vec::new();
    for argument in args {
        if options && argument == "--" {
            options = false;
            normalized.push(argument.clone());
        } else if options && matches!(argument.as_str(), "-v" | "--verbose" | "-d" | "--debug") {
        } else {
            normalized.push(argument.clone());
        }
    }
    normalized
}

fn split_terminator(args: &[String]) -> (&[String], &[String]) {
    match args.iter().position(|argument| argument == "--") {
        Some(index) => (&args[..index], &args[index + 1..]),
        None => (args, &[]),
    }
}

fn no_positionals(args: &[String]) -> bool {
    let (options, positionals) = split_terminator(args);
    options.is_empty() && positionals.is_empty()
}

fn has_help(args: &[String]) -> bool {
    let (options, _) = split_terminator(args);
    options
        .iter()
        .any(|argument| matches!(argument.as_str(), "-h" | "--help"))
}

fn usage_for_scope(args: &[String]) -> &'static str {
    match args.first().map(String::as_str) {
        Some("destination") => DESTINATION_USAGE,
        Some("offload") => OFFLOAD_USAGE,
        Some("recovery-key") => RECOVERY_KEY_USAGE,
        _ => USAGE,
    }
}

fn destination_command(args: &[String], journal: &Path, services: &BackupServices<'_>) -> CliRun {
    let (options, positionals) = split_terminator(args);
    if !positionals.is_empty() {
        return destination_usage_error(&args.join(" "));
    }
    match options {
        [command] if command == "show" => match status_view(journal) {
            Ok(view) => render_json(Ok::<_, String>(
                view.get("destination")
                    .cloned()
                    .expect("status has destination"),
            )),
            Err(error) => runtime_error(error.to_string()),
        },
        [command] if command == "set-hosted" => set_hosted(journal),
        [command] if command == "set" => destination_set(journal, services),
        _ => destination_usage_error(&args.join(" ")),
    }
}

fn recovery_key_command(args: &[String], journal: &Path, services: &BackupServices<'_>) -> CliRun {
    let (options, positionals) = split_terminator(args);
    if !positionals.is_empty() {
        return recovery_key_usage_error(&args.join(" "));
    }
    match options {
        [command] if command == "show" => recovery_key_show(journal),
        [command] if command == "rotate" => recovery_key_rotate(journal, services),
        _ => recovery_key_usage_error(&args.join(" ")),
    }
}

fn off_command(args: &[String], journal: &Path, services: &BackupServices<'_>) -> CliRun {
    let (options, positionals) = split_terminator(args);
    if !positionals.is_empty() || options.iter().any(|argument| argument != "--yes") {
        return usage_error(&format!("off {}", args.join(" ")));
    }
    turn_off(
        options.iter().any(|argument| argument == "--yes"),
        journal,
        services,
    )
}

fn enable(journal: &Path, services: &BackupServices<'_>) -> CliRun {
    let entered = if io::stdin().is_terminal() {
        String::new()
    } else {
        let mut input = String::new();
        if let Err(error) = io::stdin().lock().read_to_string(&mut input) {
            return runtime_error(error.to_string());
        }
        input.trim().to_owned()
    };
    enable_with(
        journal,
        services,
        &entered,
        || ensure_restic(services.runner, false, None, &UreqByteDownload),
        |destination, daily_key, recovery_key, restic_path| {
            solstone_core_backup_runtime::init_repository(
                services.runner,
                destination,
                daily_key,
                recovery_key,
                restic_path,
                None,
            )
        },
    )
}

fn enable_with(
    journal: &Path,
    services: &BackupServices<'_>,
    entered: &str,
    ensure: impl FnOnce() -> Result<std::path::PathBuf, String>,
    init: impl FnOnce(
        &Destination,
        &str,
        &str,
        &Path,
    ) -> Result<(), solstone_core_backup_runtime::repo::RepoError>,
) -> CliRun {
    let destination = match get_destination(journal) {
        Ok(Some(destination)) => destination,
        Ok(None) => {
            return runtime_error("Set a destination first: journal backup destination set".into());
        }
        Err(error) => return runtime_error(error.to_string()),
    };
    let config = match get_backup_config(journal) {
        Ok(config) => config,
        Err(error) => return runtime_error(error.to_string()),
    };
    let daily = config.get("daily_key").and_then(Value::as_str);
    let recovery = config.get("recovery_key").and_then(Value::as_str);
    let confirmed = config.get("confirmed_recovery_key") == Some(&Value::Bool(true));

    if confirmed || (daily.is_some() && recovery.is_none()) {
        let keys = match get_keys(journal) {
            Ok(keys) => keys,
            Err(error) => return runtime_error(error.to_string()),
        };
        let restic_path = match ensure() {
            Ok(path) => path,
            Err(error) => return runtime_error(error),
        };
        if let Some(keys) = keys {
            if let Err(error) = init(
                &destination,
                &keys.daily_key,
                &keys.recovery_key,
                &restic_path,
            ) {
                return init_error(error);
            }
        } else {
            let Some(daily) = daily else {
                return runtime_error(
                    "Repository not found; pre-initialize it or set backup.recovery_key in config."
                        .into(),
                );
            };
            let status = match validate_destination(
                services.runner,
                &destination,
                daily,
                &restic_path,
                None,
            ) {
                Ok(status) => status,
                Err(error) => return runtime_error(error.to_string()),
            };
            // Deliberately tests only repo_exists: locked/auth_failed still enable daily-only backup.
            if !status.repo_exists {
                return runtime_error(
                    "Repository not found; pre-initialize it or set backup.recovery_key in config."
                        .into(),
                );
            }
        }
        return match set_enabled(journal, true) {
            Ok(()) => success("Backup enabled.\n".into()),
            Err(error) => runtime_error(error.to_string()),
        };
    }

    let keys = match generate_and_store_keys(journal) {
        Ok(keys) => keys,
        Err(error) => return runtime_error(error.to_string()),
    };
    if entered.is_empty() {
        let display = match format_recovery_key_display(&keys.recovery_key) {
            Ok(display) => display,
            Err(error) => return runtime_error(error.to_string()),
        };
        return success(format!(
            "Your recovery key (write it down - it is the only way to restore):\n{}\nConfirm by piping the key back: journal backup enable\n",
            recovery_key_grid(&display)
        ));
    }
    if !confirm_recovery_key(entered, &keys.recovery_key) {
        return runtime_error("Recovery key did not match.".into());
    }
    if let Err(error) = set_recovery_key_confirmed(journal, true) {
        return runtime_error(error.to_string());
    }
    let restic_path = match ensure() {
        Ok(path) => path,
        Err(error) => return runtime_error(error),
    };
    if let Err(error) = init(
        &destination,
        &keys.daily_key,
        &keys.recovery_key,
        &restic_path,
    ) {
        return init_error(error);
    }
    if let Err(error) = set_enabled(journal, true) {
        return runtime_error(error.to_string());
    }
    let display = match format_recovery_key_display(&keys.recovery_key) {
        Ok(display) => display,
        Err(error) => return runtime_error(error.to_string()),
    };
    success(format!(
        "Backup enabled. Your recovery key:\n{}",
        recovery_key_grid(&display)
    ))
}

fn init_error(error: solstone_core_backup_runtime::repo::RepoError) -> CliRun {
    match error {
        solstone_core_backup_runtime::repo::RepoError::Key(ResticKeyError { returncode }) => {
            runtime_error(format!(
                "Repository initialization failed (code={returncode})."
            ))
        }
        solstone_core_backup_runtime::repo::RepoError::Backup(error) => {
            runtime_error(error.to_string())
        }
        solstone_core_backup_runtime::repo::RepoError::Failed => {
            runtime_error("repository initialization failed.".into())
        }
    }
}

fn destination_set(journal: &Path, services: &BackupServices<'_>) -> CliRun {
    let payload = match read_stdin_json() {
        Ok(payload) => payload,
        Err(message) => return runtime_error(message),
    };
    destination_set_from_payload(journal, services, &payload, || {
        ensure_restic(services.runner, false, None, &UreqByteDownload)
    })
}

fn destination_set_from_payload(
    journal: &Path,
    services: &BackupServices<'_>,
    payload: &Map<String, Value>,
    ensure: impl FnOnce() -> Result<std::path::PathBuf, String>,
) -> CliRun {
    let destination = match destination_from_payload(payload) {
        Ok(destination) => destination,
        Err(message) => return runtime_error(message),
    };
    if let Err(error) = set_destination(journal, &destination) {
        return runtime_error(error.to_string());
    }
    let password = match get_keys(journal) {
        Ok(Some(keys)) => keys.daily_key,
        Ok(None) => match generate_daily_key() {
            Ok(key) => key,
            Err(error) => return runtime_error(error.to_string()),
        },
        Err(error) => return runtime_error(error.to_string()),
    };
    let restic_path = match ensure() {
        Ok(path) => path,
        Err(error) => return runtime_error(error),
    };
    let status =
        match validate_destination(services.runner, &destination, &password, &restic_path, None) {
            Ok(status) => status,
            Err(error) => return runtime_error(error.to_string()),
        };
    let mut output = render_json(Ok::<_, String>(json!({
        "reachable": status.reachable,
        "repo_exists": status.repo_exists,
        "reason_code": status.reason_code,
        "message": status.message,
    })));
    if matches!(
        status.reason_code,
        "auth_failed" | "timeout" | "unreachable"
    ) {
        output.exit_code = 1;
    }
    output
}

fn destination_from_payload(payload: &Map<String, Value>) -> Result<Destination, String> {
    let field = |name: &'static str| {
        payload
            .get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let repository = field("repository").ok_or_else(|| "Missing repository.".to_owned())?;
    let backend = field("backend").ok_or_else(|| "Missing backend.".to_owned())?;
    let required = match backend.as_str() {
        "s3" => ["access_key_id", "secret_access_key"].as_slice(),
        "b2" => ["account_id", "account_key"].as_slice(),
        _ => return Err("Unsupported backend.".into()),
    };
    let credentials = payload
        .get("credentials")
        .and_then(Value::as_object)
        .ok_or_else(|| "Missing credentials.".to_owned())?;
    let credentials = required
        .iter()
        .map(|key| {
            credentials
                .get(*key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| ((*key).to_owned(), Value::String(value.to_owned())))
                .ok_or_else(|| format!("Missing credential: {key}."))
        })
        .collect::<Result<Map<_, _>, _>>()?;
    Ok(Destination {
        repository,
        backend,
        credentials,
    })
}

fn restore_options(args: &[String]) -> Option<bool> {
    let (options, positionals) = split_terminator(args);
    if !positionals.is_empty() {
        return None;
    }
    match options {
        [] => Some(false),
        [option] if option == "--json" => Some(true),
        _ => None,
    }
}

fn restore_command(
    args: &[String],
    journal: &Path,
    services: &BackupServices<'_>,
    recorder: &dyn RestoreRecorder,
) -> CliRun {
    let Some(json_output) = restore_options(args) else {
        return usage_error(&format!("restore {}", args.join(" ")));
    };
    let payload = match read_stdin_json() {
        Ok(payload) => payload,
        Err(message) => return runtime_error(message),
    };
    restore_from_payload(
        &payload,
        |destination, recovery_key| {
            restore_journal(journal, services, recorder, destination, recovery_key)
        },
        json_output,
    )
}

fn restore_from_payload(
    payload: &Map<String, Value>,
    restore: impl FnOnce(Destination, &str) -> RestoreOutcome,
    json_output: bool,
) -> CliRun {
    let destination = match destination_from_payload(payload) {
        Ok(destination) => destination,
        Err(message) => return runtime_error(message),
    };
    let recovery_key = payload
        .get("recovery_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(recovery_key) = recovery_key else {
        return runtime_error("Missing recovery_key.".into());
    };
    restore_result(restore(destination, recovery_key), json_output)
}

fn restore_counter_text(result: &RestoreOutcome) -> String {
    let display =
        |value: Option<u64>| value.map_or_else(|| "unknown".to_owned(), |value| value.to_string());
    format!(
        "files_expected={}, files_restored={}, bytes_expected={}, bytes_restored={}",
        display(result.files_expected),
        display(result.files_restored),
        display(result.bytes_expected),
        display(result.bytes_restored),
    )
}

fn restore_json(result: &RestoreOutcome) -> Value {
    json!({
        "status": result.status,
        "reason_code": result.reason_code,
        "recording_failure": result.recording_failure,
        "integrity_ok": result.integrity_ok,
        "resumable": result.resumable,
        "files_expected": result.files_expected,
        "files_restored": result.files_restored,
        "bytes_expected": result.bytes_expected,
        "bytes_restored": result.bytes_restored,
    })
}

fn restore_result(result: RestoreOutcome, json_output: bool) -> CliRun {
    if json_output {
        let mut output = render_json(Ok::<_, String>(restore_json(&result)));
        if result.status != "ok" {
            output.exit_code = 1;
        }
        return output;
    }
    let counters = restore_counter_text(&result);
    let reason = result.reason_code.as_deref();
    match result.status.as_str() {
        "ok" => success(format!(
            "Restore complete: {counters}, integrity_ok={}, resumable={}.\n",
            python_bool(result.integrity_ok),
            python_bool(result.resumable),
        )),
        "degraded" => {
            let detail = match reason {
                Some(RESTORE_REASON_INTEGRITY_UNVERIFIED) => {
                    "integrity verification could not run (the repository was busy or timed out)"
                }
                Some(RESTORE_REASON_INTEGRITY_FAILED) => {
                    "integrity verification failed — the backup copy may be damaged"
                }
                Some(RESTORE_REASON_RESTORE_SUMMARY_MISSING) => {
                    "restore summary was missing or malformed"
                }
                _ => "restore completed with degraded evidence",
            };
            runtime_error(format!(
                "Restore completed: {counters}; {detail} (reason_code={}).",
                reason.unwrap_or("unknown"),
            ))
        }
        _ if reason == Some(RESTORE_REASON_RESTORE_RECORD_FAILED)
            && result.recording_failure.as_deref()
                == Some(RESTORE_REASON_RESTORE_RECORD_FAILED) =>
        {
            runtime_error(format!(
                "Restore completed, but recording the result failed: {counters} (reason_code=restore_record_failed)."
            ))
        }
        _ => match (reason, result.recording_failure.as_deref()) {
            (Some(reason), Some(recording_failure)) => runtime_error(format!(
                "Restore failed: {reason}; {counters}. Recording the result also failed (recording_failure={recording_failure})."
            )),
            (Some(reason), None) => runtime_error(format!("Restore failed: {reason}; {counters}.")),
            (None, Some(recording_failure)) => runtime_error(format!(
                "Restore failed: {counters}. Recording the result also failed (recording_failure={recording_failure})."
            )),
            (None, None) => runtime_error(format!("Restore failed: {counters}.")),
        },
    }
}

fn offload_command(args: &[String], journal: &Path, services: &BackupServices<'_>) -> CliRun {
    let Some((command, options)) = args.split_first() else {
        return offload_usage_error("");
    };
    match command.as_str() {
        "status" => offload_status(options, journal),
        "run" => offload_run(options, journal, services),
        "restore" => offload_restore(options, journal, services),
        _ => offload_usage_error(&args.join(" ")),
    }
}

fn offload_status(args: &[String], journal: &Path) -> CliRun {
    let (options, positionals) = split_terminator(args);
    if !positionals.is_empty() || options.iter().any(|option| option != "--json") {
        return offload_usage_error(&args.join(" "));
    }
    let json = options.iter().any(|option| option == "--json");
    let value = match build_offload_status(journal) {
        Ok(status) => status.value,
        Err(error) => return runtime_error(error),
    };
    if json {
        return render_json(Ok::<_, String>(value));
    }
    offload_status_line(&value)
        .map(success)
        .unwrap_or_else(runtime_error)
}

fn offload_status_line(value: &Value) -> Result<String, String> {
    let field = |path: &[&str]| {
        path.iter()
            .try_fold(value, |current, field| current.get(*field))
            .ok_or_else(|| "invalid offload status.".to_owned())
    };
    let enabled = field(&["offload", "enabled"]).and_then(|value| {
        value
            .as_bool()
            .ok_or_else(|| "invalid offload status.".into())
    })?;
    let raw = field(&["raw_media", "total_bytes"]).and_then(|value| {
        value
            .as_u64()
            .ok_or_else(|| "invalid offload status.".into())
    })?;
    let pending = field(&["pending_release", "total_bytes"]).and_then(|value| {
        value
            .as_u64()
            .ok_or_else(|| "invalid offload status.".into())
    })?;
    let backup_only = field(&["backup_only", "total_bytes"]).and_then(|value| {
        value
            .as_u64()
            .ok_or_else(|| "invalid offload status.".into())
    })?;
    let degraded = field(&["backup_only", "degraded"]).and_then(|value| {
        value
            .as_bool()
            .ok_or_else(|| "invalid offload status.".into())
    })?;
    Ok(format!(
        "backup offload: enabled={} raw_media_bytes={raw} pending_release_bytes={pending} backup_only_bytes={backup_only} degraded={}\n",
        python_bool(enabled),
        python_bool(degraded),
    ))
}

fn python_bool(value: bool) -> &'static str {
    if value { "True" } else { "False" }
}

fn offload_run(args: &[String], journal: &Path, services: &BackupServices<'_>) -> CliRun {
    let (options, positionals) = split_terminator(args);
    if !positionals.is_empty() || options.iter().any(|option| option != "--dry-run") {
        return offload_usage_error(&args.join(" "));
    }
    let dry_run = options.iter().any(|option| option == "--dry-run");
    offload_run_result(run_offload(journal, services, dry_run))
}

fn offload_run_result(result: OffloadResult) -> CliRun {
    // A stalled offload is an operational result, not a CLI failure.
    success(format!("{}\n", format_offload_result(&result)))
}

fn offload_restore(args: &[String], journal: &Path, services: &BackupServices<'_>) -> CliRun {
    let (options, positionals) = split_terminator(args);
    let mut json_output = false;
    let mut all = false;
    let mut day = None;
    for argument in options {
        match argument.as_str() {
            "--json" => json_output = true,
            "--all" => all = true,
            option if option.starts_with('-') => return offload_usage_error(&args.join(" ")),
            _ if day.is_none() => day = Some(argument.as_str()),
            _ => return offload_usage_error(&args.join(" ")),
        }
    }
    for argument in positionals {
        if day.replace(argument.as_str()).is_some() {
            return offload_usage_error(&args.join(" "));
        }
    }
    if all && day.is_some() {
        return runtime_error("Use either a day or --all, not both.".into());
    }
    let result = if all {
        restore_all_offload(journal, services)
    } else {
        let Some(day) = day else {
            return runtime_error("Provide a day or --all.".into());
        };
        if !valid_day(day) {
            return runtime_error("Invalid day.".into());
        }
        restore_offload_day(journal, services, day)
    };
    offload_restore_result(result, json_output)
}

fn valid_day(day: &str) -> bool {
    day.len() == 8
        && day.bytes().all(|byte| byte.is_ascii_digit())
        && NaiveDate::parse_from_str(day, "%Y%m%d").is_ok()
}

fn offload_restore_result(result: OffloadRestoreResult, json_output: bool) -> CliRun {
    let mut output = if json_output {
        render_json(Ok::<_, String>(offload_restore_json(&result)))
    } else {
        success(format!(
            "backup offload restore: status={} reason={}{} segments_restored={} files_restored={} bytes_restored={}\n",
            result.status,
            result.reason.as_deref().unwrap_or("None"),
            result
                .reason_detail
                .as_deref()
                .map(|detail| format!(" detail={detail}"))
                .unwrap_or_default(),
            result.segments_restored,
            result.files_restored,
            result.bytes_restored,
        ))
    };
    if matches!(result.status.as_str(), "refused" | "degraded" | "error") {
        output.exit_code = 1;
    }
    output
}

fn offload_restore_json(result: &OffloadRestoreResult) -> Value {
    json!({
        "status": result.status,
        "reason": result.reason,
        "scope": result.scope,
        "day": result.day,
        "segments_selected": result.segments_selected,
        "segments_restored": result.segments_restored,
        "files_expected": result.files_expected,
        "files_restored": result.files_restored,
        "bytes_expected": result.bytes_expected,
        "bytes_restored": result.bytes_restored,
        "reason_detail": result.reason_detail,
        "details": result.details.iter().map(|detail| json!({
            "status": detail.status,
            "reason": detail.reason,
            "day": detail.day,
            "stream": detail.stream,
            "segment": detail.segment,
            "snapshot_id": detail.snapshot_id,
            "files_expected": detail.files_expected,
            "files_restored": detail.files_restored,
            "bytes_expected": detail.bytes_expected,
            "bytes_restored": detail.bytes_restored,
        })).collect::<Vec<_>>(),
    })
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
                backup_run_result(capability.execute(&services))
            }
            Err(tool_error) => backup_run_result(capability.record_tool_error(clock, tool_error)),
        },
        Err(result) => backup_run_result(result),
    }
}

fn backup_run_result(result: BackupResult) -> CliRun {
    match result.status.as_str() {
        "ok" => success(format!(
            "Backup complete (snapshot {}).\n",
            result.snapshot_id.as_deref().unwrap_or_default()
        )),
        "skipped" => success("Backup skipped (not enabled or not configured).\n".into()),
        _ => runtime_error(format!(
            "Backup failed: {}.",
            result.error_reason.as_deref().unwrap_or("failed")
        )),
    }
}

fn backup_prune(journal: &Path, services: &BackupServices<'_>) -> CliRun {
    backup_prune_result(run_prune(journal, services))
}

fn backup_prune_result(result: PruneResult) -> CliRun {
    match result.status.as_str() {
        "ok" => success("Retention prune complete.\n".into()),
        "skipped" => success("Prune skipped (not enabled or not configured).\n".into()),
        _ => runtime_error(format!(
            "Prune failed: {}.",
            result.error_reason.as_deref().unwrap_or("failed")
        )),
    }
}

fn recovery_key_rotate(journal: &Path, services: &BackupServices<'_>) -> CliRun {
    recovery_key_rotate_result(rotate_recovery_key(journal, services))
}

fn recovery_key_rotate_result(result: RotationResult) -> CliRun {
    match result.status.as_str() {
        "ok" => match result.recovery_key_display {
            Some(display) => success(format!(
                "New recovery key (write it down):\n{}",
                recovery_key_grid(&display)
            )),
            None => runtime_error("Rotation failed: failed.".into()),
        },
        "skipped" => success("Recovery key rotation skipped (backup not configured).\n".into()),
        _ => runtime_error(format!(
            "Rotation failed: {}.",
            result.reason_code.as_deref().unwrap_or("failed")
        )),
    }
}

fn turn_off(confirmed: bool, journal: &Path, services: &BackupServices<'_>) -> CliRun {
    if !confirmed {
        return runtime_error(
            "Refusing to tear down backup without --yes. This forgets all snapshots.".into(),
        );
    }
    teardown_result(teardown_backup(journal, services))
}

fn teardown_result(result: TeardownResult) -> CliRun {
    match result.status.as_str() {
        "cleared_superseded" => success(
            "Backup was claimed by another device; this device's local backup settings were cleared. Run `journal backup enable` to set up a new backup here.\n".into(),
        ),
        "ok" | "skipped" => success("Backup turned off.\n".into()),
        _ => runtime_error(format!(
            "Teardown failed: {}.",
            result.reason_code.as_deref().unwrap_or("failed")
        )),
    }
}

fn set_hosted(journal: &Path) -> CliRun {
    let payload = match read_stdin_json() {
        Ok(payload) => payload,
        Err(message) => return runtime_error(message),
    };
    let field = |name: &'static str| -> Result<String, String> {
        payload
            .get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("Missing {name}."))
    };
    let binding = match (
        field("broker_endpoint"),
        field("account_id"),
        field("instance_id"),
        field("bucket"),
        field("prefix"),
        field("broker_token"),
    ) {
        (
            Ok(broker_endpoint),
            Ok(account_id),
            Ok(instance_id),
            Ok(bucket),
            Ok(prefix),
            Ok(broker_token),
        ) => HostedBinding {
            broker_endpoint,
            account_id,
            instance_id,
            bucket,
            prefix,
            broker_token,
        },
        (Err(error), _, _, _, _, _)
        | (_, Err(error), _, _, _, _)
        | (_, _, Err(error), _, _, _)
        | (_, _, _, Err(error), _, _)
        | (_, _, _, _, Err(error), _)
        | (_, _, _, _, _, Err(error)) => return runtime_error(error),
    };
    if let Err(error) = save_hosted_binding(journal, &binding) {
        return runtime_error(error.to_string());
    }
    render_json(Ok::<_, String>(Value::Object(Map::from_iter([
        (
            "broker_endpoint".into(),
            Value::String(binding.broker_endpoint),
        ),
        ("account_id".into(), Value::String(binding.account_id)),
        ("instance_id".into(), Value::String(binding.instance_id)),
        ("bucket".into(), Value::String(binding.bucket)),
        ("prefix".into(), Value::String(binding.prefix)),
        ("bound".into(), Value::Bool(true)),
    ]))))
}
fn recovery_key_show(journal: &Path) -> CliRun {
    let keys = match get_keys(journal) {
        Ok(Some(keys)) => keys,
        Ok(None) => return runtime_error("No recovery key is set.".to_owned()),
        Err(error) => return runtime_error(error.to_string()),
    };
    let display = match solstone_core_backup::format_recovery_key_display(&keys.recovery_key) {
        Ok(display) => display,
        Err(error) => return runtime_error(error.to_string()),
    };
    success(recovery_key_grid(&display))
}

fn recovery_key_grid(display: &str) -> String {
    let groups = display.split(' ').collect::<Vec<_>>();
    (0..groups.len())
        .step_by(4)
        .map(|index| format!("{}\n", groups[index..index + 4].join(" ")))
        .collect()
}
fn read_stdin_json() -> Result<Map<String, Value>, String> {
    let mut bytes = Vec::new();
    let result = io::stdin()
        .lock()
        .take((MAX_JSON_STDIN_BYTES + 1) as u64)
        .read_to_end(&mut bytes);
    if result.is_err() {
        return Err("stdin I/O error.".into());
    }
    if bytes.len() > MAX_JSON_STDIN_BYTES {
        return Err("stdin request exceeds the JSON input limit.".into());
    }
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Err("expected JSON object on stdin.".into());
    }
    let value = serde_json::from_slice::<Value>(&bytes)
        .map_err(|error| format!("invalid JSON on stdin: {error}"))?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| "expected JSON object on stdin.".into())
}
fn render_json<E: std::fmt::Display>(result: Result<Value, E>) -> CliRun {
    match result {
        Ok(mut value) => {
            sort_json_keys(&mut value);
            match serde_json::to_string_pretty(&value) {
                Ok(rendered) => success(format!("{rendered}\n")),
                Err(_) => runtime_error("could not render JSON output.".into()),
            }
        }
        Err(error) => runtime_error(error.to_string()),
    }
}
fn sort_json_keys(value: &mut Value) {
    match value {
        Value::Object(object) => {
            let mut fields = std::mem::take(object).into_iter().collect::<Vec<_>>();
            fields.sort_by(|left, right| left.0.cmp(&right.0));
            for (_, value) in &mut fields {
                sort_json_keys(value)
            }
            *object = Map::from_iter(fields)
        }
        Value::Array(values) => {
            for value in values {
                sort_json_keys(value)
            }
        }
        _ => {}
    }
}
fn success(stdout: String) -> CliRun {
    CliRun {
        stdout,
        stderr: String::new(),
        exit_code: 0,
    }
}
fn runtime_error(message: String) -> CliRun {
    // Native backup intentionally normalizes Python's inconsistent error prefix.
    CliRun {
        stdout: String::new(),
        stderr: format!("Error: {message}\n"),
        exit_code: 1,
    }
}
fn usage_error(arguments: &str) -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr: format!("{USAGE}journal backup: error: unrecognized arguments: {arguments}\n"),
        exit_code: 2,
    }
}

fn offload_usage_error(arguments: &str) -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr: format!(
            "{OFFLOAD_USAGE}journal backup offload: error: unrecognized arguments: {arguments}\n"
        ),
        exit_code: 2,
    }
}

fn destination_usage_error(arguments: &str) -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr: format!(
            "{DESTINATION_USAGE}journal backup destination: error: unrecognized arguments: {arguments}\n"
        ),
        exit_code: 2,
    }
}

fn recovery_key_usage_error(arguments: &str) -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr: format!(
            "{RECOVERY_KEY_USAGE}journal backup recovery-key: error: unrecognized arguments: {arguments}\n"
        ),
        exit_code: 2,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::fs;
    use std::path::PathBuf;

    use solstone_core_backup::{
        generate_and_store_keys, get_backup_config, get_destination, set_destination, set_offload,
        set_recovery_key_confirmed,
    };
    use solstone_core_backup_runtime::hosted_runtime::HttpError;
    use solstone_core_backup_runtime::repo::RepoError;
    use solstone_core_backup_runtime::{
        HttpRequest, HttpResponse, HttpTransport, JournalMaintenance, JournalMaintenanceError,
        ToolOutput, ToolRequest, ToolRunner,
    };

    fn run_cli_with(args: &[String], journal: &Path, services: &BackupServices<'_>) -> CliRun {
        super::run_cli_with(args, journal, services, &NativeRestoreRecorder)
    }

    struct UnusedRunner;

    impl ToolRunner for UnusedRunner {
        fn run(&self, _: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            panic!("unconfigured commands must not start restic")
        }
    }

    struct CodeRunner(RefCell<VecDeque<i32>>);

    impl ToolRunner for CodeRunner {
        fn run(&self, _: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            Ok(ToolOutput {
                returncode: self.0.borrow_mut().pop_front().expect("fixture output"),
                stdout: vec![],
                stderr: vec![],
            })
        }
    }

    struct FailureRunner;

    impl ToolRunner for FailureRunner {
        fn run(&self, _: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            Err(io::Error::other("fixture failure"))
        }
    }

    fn offload_status_fixture() -> tempfile::TempDir {
        let journal = tempfile::tempdir().unwrap();
        let first = journal.path().join("chronicle/20260101/010000_001");
        let second = journal.path().join("chronicle/20260102/020000_001");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::write(first.join("pending.webm"), b"abc").unwrap();
        fs::write(first.join("other.webm"), b"0123456789").unwrap();
        fs::write(second.join("raw.webm"), b"01234567890123456789").unwrap();
        solstone_core_offload::append_offload_event(
            journal.path(),
            "20260101",
            "_default",
            "010000_001",
            "snapshot-a",
            &[solstone_core_offload::OffloadFile {
                name: "pending.webm".into(),
                bytes: 3,
                sha256: "a".repeat(64),
            }],
            1,
        )
        .unwrap();
        solstone_core_offload::append_offload_event(
            journal.path(),
            "20260102",
            "_default",
            "020000_001",
            "snapshot-b",
            &[solstone_core_offload::OffloadFile {
                name: "backup.webm".into(),
                bytes: 7,
                sha256: "b".repeat(64),
            }],
            2,
        )
        .unwrap();
        journal
    }

    fn offload_result(status: &str, reason: Option<&str>) -> OffloadRestoreResult {
        OffloadRestoreResult {
            status: status.into(),
            reason: reason.map(str::to_owned),
            scope: "day".into(),
            day: Some("20260101".into()),
            segments_selected: 2,
            segments_restored: 1,
            files_expected: 4,
            files_restored: 3,
            bytes_expected: 50,
            bytes_restored: 40,
            reason_detail: None,
            details: vec![],
        }
    }

    struct UnusedHttp;

    impl HttpTransport for UnusedHttp {
        fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
            panic!("unconfigured commands must not contact the broker")
        }
    }

    struct FixedClock;

    impl Clock for FixedClock {
        fn now_unix(&self) -> i64 {
            0
        }

        fn iso_week(&self) -> u8 {
            1
        }
    }

    struct UnusedMaintenance;

    impl JournalMaintenance for UnusedMaintenance {
        fn rebuild_body_history(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
            panic!("backup maintenance is not reached")
        }

        fn full_scan(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
            panic!("backup maintenance is not reached")
        }
    }

    struct SuccessfulMaintenance;

    impl JournalMaintenance for SuccessfulMaintenance {
        fn rebuild_body_history(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
            Ok(())
        }

        fn full_scan(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
            Ok(())
        }
    }

    fn unconfigured_services<'a>(
        runner: &'a UnusedRunner,
        http: &'a UnusedHttp,
        clock: &'a FixedClock,
        maintenance: &'a UnusedMaintenance,
    ) -> BackupServices<'a> {
        BackupServices {
            runner,
            http,
            clock,
            restic_path: None,
            rclone_path: None,
            version: "test",
            journal_maintenance: maintenance,
        }
    }

    fn services_with_runner<'a>(
        runner: &'a dyn ToolRunner,
        http: &'a UnusedHttp,
        clock: &'a FixedClock,
        maintenance: &'a UnusedMaintenance,
    ) -> BackupServices<'a> {
        BackupServices {
            runner,
            http,
            clock,
            restic_path: None,
            rclone_path: None,
            version: "test",
            journal_maintenance: maintenance,
        }
    }

    fn payload(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn debug_redacts_cli_output_that_may_contain_a_recovery_key() {
        let output = CliRun {
            stdout: "RECOVERY_KEY_SECRET".into(),
            stderr: "ERROR_SECRET".into(),
            exit_code: 1,
        };
        let rendered = format!("{output:?}");
        assert!(!rendered.contains("RECOVERY_KEY_SECRET"));
        assert!(!rendered.contains("ERROR_SECRET"));
    }

    fn s3_payload() -> Map<String, Value> {
        payload(json!({
            "repository": "s3:bucket/prefix",
            "backend": "s3",
            "credentials": {"access_key_id": "access", "secret_access_key": "key"},
        }))
    }

    fn set_test_destination(journal: &Path) {
        set_destination(journal, &destination_from_payload(&s3_payload()).unwrap()).unwrap();
    }

    fn daily_only_journal() -> tempfile::TempDir {
        let journal = tempfile::tempdir().unwrap();
        set_test_destination(journal.path());
        let config_path = journal.path().join("config/journal.json");
        let mut config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        config["backup"]["daily_key"] = Value::String("daily".into());
        config["backup"]["recovery_key"] = Value::Null;
        fs::write(config_path, serde_json::to_vec(&config).unwrap()).unwrap();
        journal
    }

    fn is_enabled(journal: &Path) -> bool {
        get_backup_config(journal)
            .unwrap()
            .get("enabled")
            .and_then(Value::as_bool)
            == Some(true)
    }
    #[test]
    fn malformed_uses_owned_usage() {
        let output = run_cli(&["wat".into()], Path::new("/unused"));
        assert_eq!(output.exit_code, 2);
        assert_eq!(
            output.stderr,
            "usage: journal backup <command> [options]\njournal backup: error: unrecognized arguments: wat\n"
        );
    }

    #[test]
    fn run_result_branches_use_owner_output() {
        let ok = backup_run_result(BackupResult {
            status: "ok".into(),
            snapshot_id: Some("snapshot-1".into()),
            error_reason: None,
        });
        assert_eq!(ok.stdout, "Backup complete (snapshot snapshot-1).\n");
        let skipped = backup_run_result(BackupResult {
            status: "skipped".into(),
            snapshot_id: None,
            error_reason: None,
        });
        assert_eq!(
            skipped.stdout,
            "Backup skipped (not enabled or not configured).\n"
        );
        let error = backup_run_result(BackupResult {
            status: "error".into(),
            snapshot_id: None,
            error_reason: Some("timeout".into()),
        });
        assert_eq!(error.stderr, "Error: Backup failed: timeout.\n");
    }

    #[test]
    fn prune_result_branches_use_owner_output() {
        let ok = backup_prune_result(PruneResult {
            status: "ok".into(),
            error_reason: None,
        });
        assert_eq!(ok.stdout, "Retention prune complete.\n");
        let skipped = backup_prune_result(PruneResult {
            status: "skipped".into(),
            error_reason: None,
        });
        assert_eq!(
            skipped.stdout,
            "Prune skipped (not enabled or not configured).\n"
        );
        let error = backup_prune_result(PruneResult {
            status: "error".into(),
            error_reason: Some("locked".into()),
        });
        assert_eq!(error.stderr, "Error: Prune failed: locked.\n");
    }

    #[test]
    fn rotation_result_branches_use_four_group_rows() {
        let display =
            "aaaa bbbb cccc dddd eeee ffff gggg hhhh iiii jjjj kkkk llll mmmm nnnn oooo pppp";
        let ok = recovery_key_rotate_result(RotationResult {
            status: "ok".into(),
            reason_code: None,
            recovery_key: Some("test-key".into()),
            recovery_key_display: Some(display.into()),
        });
        let rows = ok.stdout.lines().skip(1).collect::<Vec<_>>();
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().all(|row| row.split(' ').count() == 4));
        let skipped = recovery_key_rotate_result(RotationResult {
            status: "skipped".into(),
            reason_code: None,
            recovery_key: None,
            recovery_key_display: None,
        });
        assert_eq!(
            skipped.stdout,
            "Recovery key rotation skipped (backup not configured).\n"
        );
        let error = recovery_key_rotate_result(RotationResult {
            status: "error".into(),
            reason_code: Some("auth_failed".into()),
            recovery_key: None,
            recovery_key_display: None,
        });
        assert_eq!(error.stderr, "Error: Rotation failed: auth_failed.\n");
    }

    #[test]
    fn off_without_yes_refuses_before_constructing_a_teardown_request() {
        let output = run_cli(&["off".into()], Path::new("/unused"));
        assert_eq!(output.exit_code, 1);
        assert_eq!(
            output.stderr,
            "Error: Refusing to tear down backup without --yes. This forgets all snapshots.\n"
        );
    }

    #[test]
    fn off_unknown_option_uses_root_usage() {
        let output = run_cli(&["off".into(), "--wat".into()], Path::new("/unused"));
        assert_eq!(output.exit_code, 2);
        assert!(output.stderr.starts_with(USAGE));
    }

    #[test]
    fn three_subcommands_reach_their_unconfigured_runtime_bodies() {
        let journal = tempfile::tempdir().unwrap();
        let runner = UnusedRunner;
        let http = UnusedHttp;
        let clock = FixedClock;
        let maintenance = UnusedMaintenance;
        let services = unconfigured_services(&runner, &http, &clock, &maintenance);

        for (args, expected) in [
            (
                vec!["prune".into()],
                "Prune skipped (not enabled or not configured).\n",
            ),
            (
                vec!["recovery-key".into(), "rotate".into()],
                "Recovery key rotation skipped (backup not configured).\n",
            ),
            (vec!["off".into(), "--yes".into()], "Backup turned off.\n"),
        ] {
            let output = run_cli_with(&args, journal.path(), &services);
            assert_eq!(output.stdout, expected);
        }
    }

    #[test]
    fn teardown_result_branches_use_owner_output() {
        for status in ["ok", "skipped"] {
            let output = teardown_result(TeardownResult {
                status: status.into(),
                reason_code: None,
            });
            assert_eq!(output.stdout, "Backup turned off.\n");
        }
        let cleared = teardown_result(TeardownResult {
            status: "cleared_superseded".into(),
            reason_code: Some("binding_superseded".into()),
        });
        assert_eq!(
            cleared.stdout,
            "Backup was claimed by another device; this device's local backup settings were cleared. Run `journal backup enable` to set up a new backup here.\n"
        );
        let error = teardown_result(TeardownResult {
            status: "error".into(),
            reason_code: Some("timeout".into()),
        });
        assert_eq!(error.stderr, "Error: Teardown failed: timeout.\n");
    }

    #[test]
    fn offload_status_renders_distinct_human_and_json_values_without_writing() {
        let journal = offload_status_fixture();
        let runner = UnusedRunner;
        let http = UnusedHttp;
        let clock = FixedClock;
        let maintenance = UnusedMaintenance;
        let services = unconfigured_services(&runner, &http, &clock, &maintenance);
        let ledger =
            solstone_core_offload::ledger_path_for_day(journal.path(), "20260101").unwrap();
        let before = fs::read(&ledger).unwrap();
        let config = journal.path().join("config/journal.json");
        assert!(!config.exists());

        let human = run_cli_with(
            &["offload".into(), "status".into()],
            journal.path(),
            &services,
        );
        assert_eq!(
            human.stdout,
            "backup offload: enabled=False raw_media_bytes=30 pending_release_bytes=3 backup_only_bytes=7 degraded=False\n"
        );
        let machine = run_cli_with(
            &["offload".into(), "status".into(), "--json".into()],
            journal.path(),
            &services,
        );
        let value: Value = serde_json::from_str(&machine.stdout).unwrap();
        assert_eq!(value["raw_media"]["total_bytes"], 30);
        assert_eq!(value["pending_release"]["total_bytes"], 3);
        assert_eq!(value["backup_only"]["total_bytes"], 7);
        assert_eq!(fs::read(ledger).unwrap(), before, "status is read-only");
        assert!(!config.exists(), "status must not create journal config");
    }

    #[test]
    fn offload_run_stalled_is_success_and_forwards_dry_run() {
        let journal = tempfile::tempdir().unwrap();
        set_offload(
            journal.path(),
            &Map::from_iter([
                ("enabled".into(), Value::Bool(true)),
                ("budget_bytes".into(), Value::Null),
                ("floor_bytes".into(), Value::Null),
            ]),
        )
        .unwrap();
        let runner = UnusedRunner;
        let http = UnusedHttp;
        let clock = FixedClock;
        let maintenance = UnusedMaintenance;
        let services = unconfigured_services(&runner, &http, &clock, &maintenance);

        let normal = run_cli_with(&["offload".into(), "run".into()], journal.path(), &services);
        assert_eq!(normal.exit_code, 0);
        assert_eq!(
            normal.stdout,
            "backup offload: stalled reason=backup_not_ready files_marked=0 bytes_marked=0 bytes_released=0 ran_out_of_markable_media=false\n"
        );
        let dry_run = run_cli_with(
            &["offload".into(), "run".into(), "--dry-run".into()],
            journal.path(),
            &services,
        );
        assert_eq!(dry_run.exit_code, 0);
        assert_eq!(
            dry_run.stdout,
            "backup offload: stalled reason=backup_not_ready dry_run=true\n"
        );
    }

    #[test]
    fn offload_restore_conflicts_and_invalid_days_are_body_errors() {
        let journal = tempfile::tempdir().unwrap();
        let runner = UnusedRunner;
        let http = UnusedHttp;
        let clock = FixedClock;
        let maintenance = UnusedMaintenance;
        let services = unconfigured_services(&runner, &http, &clock, &maintenance);
        for (args, message) in [
            (
                vec!["offload".into(), "restore".into()],
                "Provide a day or --all.",
            ),
            (
                vec![
                    "offload".into(),
                    "restore".into(),
                    "20260101".into(),
                    "--all".into(),
                ],
                "Use either a day or --all, not both.",
            ),
            (
                vec!["offload".into(), "restore".into(), "20260230".into()],
                "Invalid day.",
            ),
        ] {
            let output = run_cli_with(&args, journal.path(), &services);
            assert_eq!(output.exit_code, 1);
            assert_eq!(output.stderr, format!("Error: {message}\n"));
        }
    }

    #[test]
    fn offload_restore_exit_codes_and_json_follow_the_owner_result() {
        assert_eq!(
            offload_restore_result(offload_result("ok", None), false).exit_code,
            0
        );
        assert_eq!(
            offload_restore_result(offload_result("no_op", Some("nothing_to_restore")), false)
                .exit_code,
            0
        );
        for status in ["refused", "degraded", "error"] {
            assert_eq!(
                offload_restore_result(offload_result(status, Some("failed")), false).exit_code,
                1
            );
        }
        let json = offload_restore_result(offload_result("ok", None), true);
        let value: Value = serde_json::from_str(&json.stdout).unwrap();
        assert_eq!(value["bytes_restored"], 40);
        assert_eq!(value["reason"], Value::Null);
    }

    #[test]
    fn offload_parser_owns_wrong_options_and_unknown_subcommands() {
        let journal = tempfile::tempdir().unwrap();
        let runner = UnusedRunner;
        let http = UnusedHttp;
        let clock = FixedClock;
        let maintenance = UnusedMaintenance;
        let services = unconfigured_services(&runner, &http, &clock, &maintenance);
        for args in [
            vec!["offload".into(), "run".into(), "--json".into()],
            vec!["offload".into(), "status".into(), "--dry-run".into()],
            vec!["offload".into(), "wat".into()],
        ] {
            let output = run_cli_with(&args, journal.path(), &services);
            assert_eq!(output.exit_code, 2);
            assert!(output.stderr.starts_with(OFFLOAD_USAGE));
        }
        let help = run_cli(&["offload".into(), "--help".into()], journal.path());
        assert_eq!(help.stdout, OFFLOAD_USAGE);
    }

    #[test]
    fn parser_accepts_repeated_boolean_flags_idempotently() {
        let journal = tempfile::tempdir().unwrap();
        let runner = UnusedRunner;
        let http = UnusedHttp;
        let clock = FixedClock;
        let maintenance = UnusedMaintenance;
        let services = unconfigured_services(&runner, &http, &clock, &maintenance);
        for args in [
            vec!["off".into(), "--yes".into(), "--yes".into()],
            vec![
                "offload".into(),
                "status".into(),
                "--json".into(),
                "--json".into(),
            ],
            vec![
                "offload".into(),
                "run".into(),
                "--dry-run".into(),
                "--dry-run".into(),
            ],
            vec![
                "offload".into(),
                "restore".into(),
                "--all".into(),
                "--all".into(),
                "--json".into(),
                "--json".into(),
            ],
        ] {
            assert_eq!(run_cli_with(&args, journal.path(), &services).exit_code, 0);
        }
    }

    #[test]
    fn parser_honors_terminator_and_reports_the_owning_usage_scope() {
        let journal = tempfile::tempdir().unwrap();
        let runner = UnusedRunner;
        let http = UnusedHttp;
        let clock = FixedClock;
        let maintenance = UnusedMaintenance;
        let services = unconfigured_services(&runner, &http, &clock, &maintenance);

        let day = run_cli_with(
            &[
                "offload".into(),
                "restore".into(),
                "--".into(),
                "20260101".into(),
            ],
            journal.path(),
            &services,
        );
        assert_eq!(day.exit_code, 0);

        for (args, usage) in [
            (vec!["status".into(), "--".into(), "extra".into()], USAGE),
            (vec!["destination".into(), "wat".into()], DESTINATION_USAGE),
            (vec!["offload".into(), "wat".into()], OFFLOAD_USAGE),
            (
                vec!["recovery-key".into(), "wat".into()],
                RECOVERY_KEY_USAGE,
            ),
        ] {
            let output = run_cli_with(&args, journal.path(), &services);
            assert_eq!(output.exit_code, 2);
            assert!(output.stderr.starts_with(usage));
        }
    }

    #[test]
    fn destination_payload_rejects_every_required_field_and_trims_both_backends() {
        for (value, message) in [
            (json!({}), "Missing repository."),
            (json!({"repository":"repo"}), "Missing backend."),
            (
                json!({"repository":"repo", "backend":"azure"}),
                "Unsupported backend.",
            ),
            (
                json!({"repository":"repo", "backend":"s3"}),
                "Missing credentials.",
            ),
            (
                json!({"repository":"repo", "backend":"s3", "credentials":{"secret_access_key":"key"}}),
                "Missing credential: access_key_id.",
            ),
            (
                json!({"repository":"repo", "backend":"s3", "credentials":{"access_key_id":"access"}}),
                "Missing credential: secret_access_key.",
            ),
            (
                json!({"repository":"repo", "backend":"b2", "credentials":{"account_key":"key"}}),
                "Missing credential: account_id.",
            ),
            (
                json!({"repository":"repo", "backend":"b2", "credentials":{"account_id":"account"}}),
                "Missing credential: account_key.",
            ),
        ] {
            assert_eq!(
                destination_from_payload(&payload(value)),
                Err(message.into())
            );
        }
        let s3 = destination_from_payload(&payload(json!({
            "repository":"  s3:bucket/prefix  ", "backend":" s3 ",
            "credentials":{"access_key_id":" access ","secret_access_key":" key "}
        })))
        .unwrap();
        assert_eq!(s3.repository, "s3:bucket/prefix");
        assert_eq!(s3.backend, "s3");
        assert_eq!(s3.credentials["access_key_id"], "access");
        let b2 = destination_from_payload(&payload(json!({
            "repository":" b2:bucket/prefix ", "backend":" b2 ",
            "credentials":{"account_id":" account ","account_key":" key "}
        })))
        .unwrap();
        assert_eq!(b2.repository, "b2:bucket/prefix");
        assert_eq!(b2.backend, "b2");
        assert_eq!(b2.credentials["account_key"], "key");
    }

    #[test]
    fn destination_set_persists_before_validation_and_maps_status_exits() {
        for (code, reason, exit_code) in [
            (0, "repo_exists", 0),
            (10, "repo_missing", 0),
            (11, "locked", 0),
            (12, "auth_failed", 1),
            (124, "timeout", 1),
            (1, "unreachable", 1),
        ] {
            let journal = tempfile::tempdir().unwrap();
            let runner = CodeRunner(RefCell::new(VecDeque::from([code])));
            let http = UnusedHttp;
            let clock = FixedClock;
            let maintenance = UnusedMaintenance;
            let services = services_with_runner(&runner, &http, &clock, &maintenance);
            let output =
                destination_set_from_payload(journal.path(), &services, &s3_payload(), || {
                    Ok(PathBuf::from("/fixture/bin/restic"))
                });
            assert_eq!(output.exit_code, exit_code);
            let value: Value = serde_json::from_str(&output.stdout).unwrap();
            assert_eq!(value["reason_code"], reason);
            assert_eq!(
                get_destination(journal.path()).unwrap().unwrap().repository,
                "s3:bucket/prefix"
            );
        }
    }

    #[test]
    fn destination_set_tool_failure_keeps_destination_without_status_or_daily_key() {
        let journal = tempfile::tempdir().unwrap();
        let runner = UnusedRunner;
        let http = UnusedHttp;
        let clock = FixedClock;
        let maintenance = UnusedMaintenance;
        let services = unconfigured_services(&runner, &http, &clock, &maintenance);
        let output = destination_set_from_payload(journal.path(), &services, &s3_payload(), || {
            Err("restic unavailable".into())
        });
        assert_eq!(output.stdout, "");
        assert_eq!(output.stderr, "Error: restic unavailable\n");
        assert!(get_destination(journal.path()).unwrap().is_some());
        let config = get_backup_config(journal.path()).unwrap();
        assert!(config.get("daily_key").is_none_or(Value::is_null));
    }

    #[test]
    fn destination_set_key_and_validation_failures_keep_destination_without_status() {
        let http = UnusedHttp;
        let clock = FixedClock;
        let maintenance = UnusedMaintenance;
        let key_failure = tempfile::tempdir().unwrap();
        fs::create_dir_all(key_failure.path().join("config")).unwrap();
        fs::write(
            key_failure.path().join("config/journal.json"),
            serde_json::to_vec(&json!({"backup":{"daily_key":"daily","recovery_key":"bad"}}))
                .unwrap(),
        )
        .unwrap();
        let runner = UnusedRunner;
        let services = unconfigured_services(&runner, &http, &clock, &maintenance);
        let output =
            destination_set_from_payload(key_failure.path(), &services, &s3_payload(), || {
                panic!("key failure must precede tool acquisition")
            });
        assert!(output.stdout.is_empty());
        assert!(get_destination(key_failure.path()).unwrap().is_some());

        let validation_failure = tempfile::tempdir().unwrap();
        let runner = FailureRunner;
        let services = services_with_runner(&runner, &http, &clock, &maintenance);
        let output = destination_set_from_payload(
            validation_failure.path(),
            &services,
            &s3_payload(),
            || Ok(PathBuf::from("/fixture/bin/restic")),
        );
        assert!(output.stdout.is_empty());
        assert!(
            get_destination(validation_failure.path())
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn invalid_destination_payload_never_mutates_the_journal() {
        let journal = tempfile::tempdir().unwrap();
        let runner = UnusedRunner;
        let http = UnusedHttp;
        let clock = FixedClock;
        let maintenance = UnusedMaintenance;
        let services = unconfigured_services(&runner, &http, &clock, &maintenance);
        let before = fs::read_dir(journal.path()).unwrap().count();
        let output = destination_set_from_payload(
            journal.path(),
            &services,
            &payload(json!({"repository":"repo", "backend":"s3", "credentials":[]})),
            || panic!("invalid input must not acquire restic"),
        );
        assert_eq!(output.stderr, "Error: Missing credentials.\n");
        assert_eq!(fs::read_dir(journal.path()).unwrap().count(), before);
    }

    #[test]
    fn restore_requires_a_recovery_key_and_uses_exact_owner_messages() {
        let recorder = solstone_core_backup_runtime::test_support::RestoreRecorderSpy::new();
        for recovery_key in [Value::Null, Value::String("   ".into())] {
            let mut input = s3_payload();
            input.insert("recovery_key".into(), recovery_key);
            let output = restore_from_payload(
                &input,
                |_, _| panic!("missing recovery key must not restore"),
                false,
            );
            assert_eq!(output.stderr, "Error: Missing recovery_key.\n");
        }
        assert!(
            recorder.calls().is_empty(),
            "request validation is unrecorded"
        );

        let outcome = |status: &str, reason_code: Option<&str>, counters: Option<u64>| {
            publish_restore_outcome(
                Path::new("/unused"),
                &FixedClock,
                &recorder,
                RestoreDraft {
                    status: status.into(),
                    reason_code: reason_code.map(str::to_owned),
                    integrity_ok: false,
                    resumable: false,
                    files_expected: counters,
                    files_restored: counters,
                    bytes_expected: counters,
                    bytes_restored: counters,
                },
            )
        };
        let mut input = s3_payload();
        input.insert(
            "recovery_key".into(),
            Value::String(" recovery-key ".into()),
        );
        let trimmed = restore_from_payload(
            &input,
            |_, recovery_key| {
                assert_eq!(recovery_key, "recovery-key");
                outcome("error", Some("timeout"), None)
            },
            false,
        );
        assert_eq!(
            trimmed.stderr,
            "Error: Restore failed: timeout; files_expected=unknown, files_restored=unknown, bytes_expected=unknown, bytes_restored=unknown.\n"
        );
        assert_eq!(
            restore_result(outcome("error", Some("timeout"), Some(42)), false).stderr,
            "Error: Restore failed: timeout; files_expected=42, files_restored=42, bytes_expected=42, bytes_restored=42.\n"
        );
        assert_eq!(
            restore_result(
                outcome("degraded", Some("integrity_unverified"), Some(42)),
                false,
            )
            .stderr,
            "Error: Restore completed: files_expected=42, files_restored=42, bytes_expected=42, bytes_restored=42; integrity verification could not run (the repository was busy or timed out) (reason_code=integrity_unverified).\n"
        );
        assert_eq!(
            restore_result(
                outcome("degraded", Some("integrity_failed"), Some(42)),
                false
            )
            .stderr,
            "Error: Restore completed: files_expected=42, files_restored=42, bytes_expected=42, bytes_restored=42; integrity verification failed — the backup copy may be damaged (reason_code=integrity_failed).\n"
        );
        let mut ok = outcome("ok", None, Some(42));
        ok.integrity_ok = true;
        ok.resumable = true;
        assert_eq!(
            restore_result(ok, false).stdout,
            "Restore complete: files_expected=42, files_restored=42, bytes_expected=42, bytes_restored=42, integrity_ok=True, resumable=True.\n"
        );
        let summary_missing = outcome("degraded", Some("restore_summary_missing"), None);
        assert_eq!(
            restore_result(summary_missing, false).stderr,
            "Error: Restore completed: files_expected=unknown, files_restored=unknown, bytes_expected=unknown, bytes_restored=unknown; restore summary was missing or malformed (reason_code=restore_summary_missing).\n"
        );
    }

    #[test]
    fn restore_json_uses_the_shared_argv_fixture_and_records_one_attempt() {
        let journal = tempfile::tempdir().expect("journal");
        let keys = generate_and_store_keys(journal.path()).expect("keys");
        let runner = solstone_core_backup_runtime::test_support::ArgvResticFixture::new(
            "[{\"id\":\"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\",\"time\":\"2026-01-01T00:00:00.000000000+00:00\",\"paths\":[\"/journal\"]}]",
            ToolOutput {
                returncode: 0,
                stdout: b"[{\"message_type\":\"summary\",\"total_files\":4,\"files_restored\":4,\"total_bytes\":12,\"bytes_restored\":12}]".to_vec(),
                stderr: vec![],
            },
            ToolOutput {
                returncode: 0,
                stdout: vec![],
                stderr: vec![],
            },
        );
        let http = UnusedHttp;
        let clock = FixedClock;
        let maintenance = SuccessfulMaintenance;
        let services = BackupServices {
            runner: &runner,
            http: &http,
            clock: &clock,
            restic_path: Some(Path::new("/fixture/restic")),
            rclone_path: None,
            version: "test",
            journal_maintenance: &maintenance,
        };
        let recorder = solstone_core_backup_runtime::test_support::RestoreRecorderSpy::new();
        let mut input = s3_payload();
        input.insert("recovery_key".into(), Value::String(keys.recovery_key));

        let output = restore_from_payload(
            &input,
            |destination, recovery_key| {
                restore_journal(
                    journal.path(),
                    &services,
                    &recorder,
                    destination,
                    recovery_key,
                )
            },
            true,
        );

        assert_eq!(output.exit_code, 0);
        let rendered: Value = serde_json::from_str(&output.stdout).expect("restore json");
        assert_eq!(rendered["status"], "ok");
        assert_eq!(rendered["reason_code"], Value::Null);
        assert_eq!(rendered["recording_failure"], Value::Null);
        assert_eq!(rendered["files_expected"], 4);
        assert_eq!(rendered["bytes_restored"], 12);
        assert_eq!(recorder.calls().len(), 1);
        assert_eq!(
            runner.calls(),
            vec![
                vec!["snapshots".into(), "--json".into()],
                vec![
                    "restore".into(),
                    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef:/journal"
                        .into(),
                    "--target".into(),
                    journal.path().display().to_string(),
                    "--json".into(),
                ],
                vec!["check".into()],
            ]
        );
        assert!(runner.refusals().is_empty());
    }

    #[test]
    fn destination_and_restore_positionals_use_their_own_usage_scopes() {
        let journal = tempfile::tempdir().unwrap();
        let runner = UnusedRunner;
        let http = UnusedHttp;
        let clock = FixedClock;
        let maintenance = UnusedMaintenance;
        let services = unconfigured_services(&runner, &http, &clock, &maintenance);
        for (args, usage) in [
            (
                vec!["destination".into(), "set".into(), "extra".into()],
                DESTINATION_USAGE,
            ),
            (vec!["restore".into(), "extra".into()], USAGE),
        ] {
            let output = run_cli_with(&args, journal.path(), &services);
            assert_eq!(output.exit_code, 2);
            assert!(output.stderr.starts_with(usage));
        }
    }

    #[test]
    fn enable_without_destination_refuses_before_tool_acquisition() {
        let journal = tempfile::tempdir().unwrap();
        let runner = UnusedRunner;
        let http = UnusedHttp;
        let clock = FixedClock;
        let maintenance = UnusedMaintenance;
        let services = unconfigured_services(&runner, &http, &clock, &maintenance);
        let output = enable_with(
            journal.path(),
            &services,
            "",
            || panic!("destination failure must not acquire restic"),
            |_, _, _, _| panic!("destination failure must not initialize"),
        );
        assert_eq!(
            output.stderr,
            "Error: Set a destination first: journal backup destination set\n"
        );
    }

    #[test]
    fn enable_confirmed_full_keys_initializes_then_enables() {
        let journal = tempfile::tempdir().unwrap();
        set_test_destination(journal.path());
        generate_and_store_keys(journal.path()).unwrap();
        set_recovery_key_confirmed(journal.path(), true).unwrap();
        let runner = UnusedRunner;
        let http = UnusedHttp;
        let clock = FixedClock;
        let maintenance = UnusedMaintenance;
        let services = unconfigured_services(&runner, &http, &clock, &maintenance);
        let order = RefCell::new(vec![]);
        let output = enable_with(
            journal.path(),
            &services,
            "",
            || {
                order.borrow_mut().push("ensure");
                Ok(PathBuf::from("/fixture/bin/restic"))
            },
            |_, _, _, _| {
                order.borrow_mut().push("init");
                Ok(())
            },
        );
        assert_eq!(output.stdout, "Backup enabled.\n");
        assert_eq!(&*order.borrow(), &["ensure", "init"]);
        assert!(is_enabled(journal.path()));
    }

    #[test]
    fn enable_daily_only_deliberately_enables_locked_and_auth_failed_repositories() {
        for code in [11, 12] {
            let journal = daily_only_journal();
            let runner = CodeRunner(RefCell::new(VecDeque::from([code])));
            let http = UnusedHttp;
            let clock = FixedClock;
            let maintenance = UnusedMaintenance;
            let services = services_with_runner(&runner, &http, &clock, &maintenance);
            let output = enable_with(
                journal.path(),
                &services,
                "",
                || Ok(PathBuf::from("/fixture/bin/restic")),
                |_, _, _, _| panic!("daily-only branch does not initialize"),
            );
            assert_eq!(output.stdout, "Backup enabled.\n");
            assert!(is_enabled(journal.path()));
        }
    }

    #[test]
    fn enable_daily_only_refuses_missing_timeout_and_unreachable_without_inventing_keys() {
        for code in [10, 124, 1] {
            let journal = daily_only_journal();
            let runner = CodeRunner(RefCell::new(VecDeque::from([code])));
            let http = UnusedHttp;
            let clock = FixedClock;
            let maintenance = UnusedMaintenance;
            let services = services_with_runner(&runner, &http, &clock, &maintenance);
            let output = enable_with(
                journal.path(),
                &services,
                "",
                || Ok(PathBuf::from("/fixture/bin/restic")),
                |_, _, _, _| panic!("daily-only refusal does not initialize"),
            );
            assert_eq!(
                output.stderr,
                "Error: Repository not found; pre-initialize it or set backup.recovery_key in config.\n"
            );
            assert!(!is_enabled(journal.path()));
            assert!(get_keys(journal.path()).unwrap().is_none());
        }
    }

    #[test]
    fn enable_empty_terminal_like_input_prints_grid_and_stays_disabled() {
        let journal = tempfile::tempdir().unwrap();
        set_test_destination(journal.path());
        let runner = UnusedRunner;
        let http = UnusedHttp;
        let clock = FixedClock;
        let maintenance = UnusedMaintenance;
        let services = unconfigured_services(&runner, &http, &clock, &maintenance);
        let output = enable_with(
            journal.path(),
            &services,
            "",
            || panic!("empty confirmation must not acquire restic"),
            |_, _, _, _| panic!("empty confirmation must not initialize"),
        );
        let lines = output.stdout.lines().collect::<Vec<_>>();
        assert_eq!(
            lines[0],
            "Your recovery key (write it down - it is the only way to restore):"
        );
        assert!(lines[1..5].iter().all(|line| line.split(' ').count() == 4));
        assert_eq!(lines[5], "");
        assert_eq!(
            lines[6],
            "Confirm by piping the key back: journal backup enable"
        );
        assert!(!is_enabled(journal.path()));
    }

    #[test]
    fn enable_confirmation_precedes_tool_and_preserves_keys_on_repeat() {
        let journal = tempfile::tempdir().unwrap();
        set_test_destination(journal.path());
        let keys = generate_and_store_keys(journal.path()).unwrap();
        let before = fs::read(journal.path().join("config/journal.json")).unwrap();
        let runner = UnusedRunner;
        let http = UnusedHttp;
        let clock = FixedClock;
        let maintenance = UnusedMaintenance;
        let services = unconfigured_services(&runner, &http, &clock, &maintenance);
        let order = RefCell::new(vec![]);
        let output = enable_with(
            journal.path(),
            &services,
            &keys.recovery_key,
            || {
                assert!(
                    get_backup_config(journal.path()).unwrap()["confirmed_recovery_key"].as_bool()
                        == Some(true)
                );
                order.borrow_mut().push("ensure");
                Ok(PathBuf::from("/fixture/bin/restic"))
            },
            |_, _, _, _| {
                order.borrow_mut().push("init");
                Ok(())
            },
        );
        assert!(
            output
                .stdout
                .starts_with("Backup enabled. Your recovery key:\n")
        );
        assert_eq!(&*order.borrow(), &["ensure", "init"]);
        assert!(is_enabled(journal.path()));

        let second = tempfile::tempdir().unwrap();
        set_test_destination(second.path());
        let first = enable_with(
            second.path(),
            &services,
            "",
            || panic!("unconfirmed key display must not acquire restic"),
            |_, _, _, _| panic!("unconfirmed key display must not initialize"),
        );
        assert_eq!(first.exit_code, 0);
        let saved = fs::read(second.path().join("config/journal.json")).unwrap();
        let repeated = enable_with(
            second.path(),
            &services,
            "",
            || panic!("repeated key display must not acquire restic"),
            |_, _, _, _| panic!("repeated key display must not initialize"),
        );
        assert_eq!(repeated.exit_code, 0);
        assert_eq!(
            fs::read(second.path().join("config/journal.json")).unwrap(),
            saved
        );
        assert_ne!(before, saved);
    }

    #[test]
    fn enable_confirmation_failures_keep_confirmation_before_init_and_do_not_use_friendly_tool_error()
     {
        for init_failure in [true, false] {
            let journal = tempfile::tempdir().unwrap();
            set_test_destination(journal.path());
            let keys = generate_and_store_keys(journal.path()).unwrap();
            let runner = UnusedRunner;
            let http = UnusedHttp;
            let clock = FixedClock;
            let maintenance = UnusedMaintenance;
            let services = unconfigured_services(&runner, &http, &clock, &maintenance);
            let output = enable_with(
                journal.path(),
                &services,
                &keys.recovery_key,
                || {
                    if init_failure {
                        Ok(PathBuf::from("/fixture/bin/restic"))
                    } else {
                        Err("tool unavailable".into())
                    }
                },
                |_, _, _, _| Err(RepoError::Key(ResticKeyError { returncode: 12 })),
            );
            assert_eq!(
                output.stderr,
                if init_failure {
                    "Error: Repository initialization failed (code=12).\n"
                } else {
                    "Error: tool unavailable\n"
                }
            );
            assert!(
                get_backup_config(journal.path()).unwrap()["confirmed_recovery_key"].as_bool()
                    == Some(true)
            );
            assert!(!is_enabled(journal.path()));
        }
    }

    #[test]
    fn enable_mismatch_and_positionals_leave_backup_disabled() {
        let journal = tempfile::tempdir().unwrap();
        set_test_destination(journal.path());
        let keys = generate_and_store_keys(journal.path()).unwrap();
        let runner = UnusedRunner;
        let http = UnusedHttp;
        let clock = FixedClock;
        let maintenance = UnusedMaintenance;
        let services = unconfigured_services(&runner, &http, &clock, &maintenance);
        let mismatch = enable_with(
            journal.path(),
            &services,
            "not-the-entered-key",
            || panic!("mismatch must not acquire restic"),
            |_, _, _, _| panic!("mismatch must not initialize"),
        );
        assert_eq!(mismatch.stderr, "Error: Recovery key did not match.\n");
        assert!(!is_enabled(journal.path()));
        assert_eq!(get_keys(journal.path()).unwrap().unwrap(), keys);
        let positional = run_cli_with(
            &["enable".into(), "extra".into()],
            journal.path(),
            &services,
        );
        assert_eq!(positional.exit_code, 2);
        assert!(positional.stderr.starts_with(USAGE));
    }
}

#[cfg(test)]
mod resolution_tests {
    use super::*;
    use serde_json::{Value, json};
    use solstone_core_artifact_download::ByteDownloadError;
    use solstone_core_backup::{
        record_backup_result, record_verification_result, set_mode, set_offload,
    };
    use solstone_core_backup_runtime::hosted_runtime::{HttpError, HttpRequest, HttpResponse};
    use solstone_core_backup_runtime::rclone_install::{
        RCLONE_SCHEMA_VERSION, RCLONE_TOOL, RCLONE_VERSION,
    };
    use solstone_core_backup_runtime::readiness::{
        RESTIC_SCHEMA_VERSION, RESTIC_TOOL, RESTIC_VERSION, binary_path, file_sha256,
        platform_info, sentinel_path,
    };
    use solstone_core_backup_runtime::{HttpTransport, ToolInstallDirs, ToolOutput, ToolRequest};
    use std::cell::{Cell, RefCell};

    fn run_cli_with_deps(
        args: &[String],
        journal: &Path,
        runner: &dyn ToolRunner,
        downloader: &dyn ByteDownload,
        http: &dyn HttpTransport,
        dirs: ToolInstallDirs<'_>,
    ) -> CliRun {
        super::run_cli_with_deps(
            args,
            journal,
            runner,
            downloader,
            http,
            dirs,
            &NativeRestoreRecorder,
        )
    }
    use std::ffi::OsString;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

    fn byo_journal() -> tempfile::TempDir {
        let journal = tempfile::tempdir().unwrap();
        set_destination(
            journal.path(),
            &Destination {
                repository: "s3:bucket/prefix".to_owned(),
                backend: "s3".to_owned(),
                credentials: serde_json::Map::from_iter([
                    (
                        "access_key_id".to_owned(),
                        Value::String("access".to_owned()),
                    ),
                    (
                        "secret_access_key".to_owned(),
                        Value::String("secret".to_owned()),
                    ),
                ]),
            },
        )
        .unwrap();
        generate_and_store_keys(journal.path()).unwrap();
        set_enabled(journal.path(), true).unwrap();
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
    fn ac1_backup_run_uses_pinned_restic_not_decoy() {
        let restic_dir = tempfile::tempdir().unwrap();
        let decoy_dir = tempfile::tempdir().unwrap();
        let expected = write_ready_restic(restic_dir.path());
        let decoy = decoy_dir.path().join("restic");
        fs::write(&decoy, b"decoy").unwrap();
        let journal = byo_journal();
        let runner = RecordingRunner::new();
        run_cli_with_deps(
            &args(&["run"]),
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
            &args(&["run"]),
            journal.path(),
            &runner,
            &downloader,
            &UnusedHttp,
            dirs(restic_dir.path(), None),
        );
        assert_eq!(output.exit_code, 1);
        assert!(output.stderr.contains("restic_unavailable"));
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
            &args(&["run"]),
            byo.path(),
            &byo_runner,
            &PanicDownload,
            &UnusedHttp,
            dirs(restic_dir.path(), None),
        );
        assert_eq!(byo_output.stdout, "Backup complete (snapshot snap).\n");
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
            &args(&["run"]),
            operated.path(),
            &operated_runner,
            &PanicDownload,
            &broker,
            dirs(restic_dir.path(), Some(rclone_dir.path())),
        );
        assert_eq!(operated_output.stdout, "Backup complete (snapshot snap).\n");
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
            &args(&["run"]),
            journal.path(),
            &runner,
            &PanicDownload,
            &UnusedHttp,
            dirs(restic_dir.path(), None),
        );
        assert_eq!(output.stdout, "Backup complete (snapshot snap).\n");
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
            &args(&["run"]),
            journal.path(),
            &runner,
            &PanicDownload,
            &broker,
            dirs(restic_dir.path(), Some(rclone_dir.path())),
        );
        assert_eq!(output.stdout, "Backup complete (snapshot snap).\n");
        assert_eq!(
            rclone_program(&runner.argvs.borrow()).as_deref(),
            Some(expected_rclone.to_string_lossy().as_ref())
        );
        assert!(broker.calls.get() > 0);
    }

    #[test]
    fn backup_run_admission_terminals_do_not_resolve_tools() {
        let runner = RecordingRunner::new();
        let skipped_journal = tempfile::tempdir().unwrap();
        let skipped = run_cli_with_deps(
            &args(&["run"]),
            skipped_journal.path(),
            &runner,
            &PanicDownload,
            &UnusedHttp,
            ToolInstallDirs::default(),
        );
        assert_eq!(
            skipped.stdout,
            "Backup skipped (not enabled or not configured).\n"
        );
        assert_eq!(skipped.exit_code, 0);
        assert!(runner.programs.borrow().is_empty());

        let runner = RecordingRunner::new();
        let unresolved_journal = tempfile::tempdir().unwrap();
        let unresolved = run_cli_with_deps(
            &args(&["run"]),
            &unresolved_journal.path().join("missing"),
            &runner,
            &PanicDownload,
            &UnusedHttp,
            ToolInstallDirs::default(),
        );
        assert_eq!(
            unresolved.stderr,
            "Error: Backup failed: journal_path_unresolved.\n"
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
            &args(&["run"]),
            config_error_journal.path(),
            &runner,
            &PanicDownload,
            &UnusedHttp,
            ToolInstallDirs::default(),
        );
        assert_eq!(config_error.stderr, "Error: Backup failed: broker_error.\n");
        assert_eq!(config_error.exit_code, 1);
        assert!(runner.programs.borrow().is_empty());
    }

    #[test]
    fn run_cli_with_deps_excludes_invalid_and_help_forms_from_capability_path() {
        let journal = tempfile::tempdir().unwrap();
        for (args, expected_exit, help) in [
            (args(&["run", "extra"]), 2, false),
            (args(&["run", "-h"]), 0, true),
            (args(&["run", "--help"]), 0, true),
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
            let expected =
                super::run_cli_with(&args, journal.path(), &placeholder, &NativeRestoreRecorder);
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
            if help {
                assert_eq!(output.stdout, USAGE);
            } else {
                assert!(output.stderr.starts_with(USAGE));
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
            &args(&["run"]),
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
    fn ac5_offload_run_passes_absolute_rclone_program() {
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
            &args(&["offload", "run"]),
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
            &args(&["run"]),
            journal.path(),
            &runner,
            &downloader,
            &BrokerHttp::new(),
            dirs(restic_dir.path(), Some(rclone_dir.path())),
        );
        assert_eq!(output.exit_code, 1);
        assert!(output.stderr.contains("rclone_unavailable"));
        assert_eq!(
            last_backup_reason(journal.path()).as_deref(),
            Some("rclone_unavailable")
        );
        assert!(downloader.calls.get() > 0);
        assert!(
            runner
                .argvs
                .borrow()
                .iter()
                .all(|argv| rclone_program(std::slice::from_ref(argv)).is_none())
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
    fn ac7_byo_operational_verbs_resolve_restic_without_rclone() {
        let restic_dir = tempfile::tempdir().unwrap();
        let decoy_dir = tempfile::tempdir().unwrap();
        let expected = write_ready_restic(restic_dir.path());
        let decoy = decoy_dir.path().join("restic");
        fs::write(&decoy, b"decoy").unwrap();
        let journal = tempfile::tempdir().unwrap();
        for argv in [
            args(&["prune"]),
            args(&["restore"]),
            args(&["recovery-key", "rotate"]),
            args(&["off", "--yes"]),
            args(&["offload", "run"]),
            args(&["offload", "run", "--dry-run"]),
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
    fn ac8_operated_prune_does_not_resolve_rclone() {
        let restic_dir = tempfile::tempdir().unwrap();
        let decoy_dir = tempfile::tempdir().unwrap();
        let expected = write_ready_restic(restic_dir.path());
        let decoy = decoy_dir.path().join("restic");
        fs::write(&decoy, b"decoy").unwrap();
        let journal = operated_journal();
        let runner = RecordingRunner::new();
        run_cli_with_deps(
            &args(&["prune"]),
            journal.path(),
            &runner,
            &PanicDownload,
            &BrokerHttp::new(),
            dirs(restic_dir.path(), None),
        );
        assert_resolved_restic(&runner.programs.borrow(), &expected, &decoy);
    }

    #[test]
    fn ac9_offload_restore_uses_pinned_restic() {
        let restic_dir = tempfile::tempdir().unwrap();
        let decoy_dir = tempfile::tempdir().unwrap();
        let expected = write_ready_restic(restic_dir.path());
        let decoy = decoy_dir.path().join("restic");
        fs::write(&decoy, b"decoy").unwrap();
        let journal = tempfile::tempdir().unwrap();
        let runner = RecordingRunner::new();
        run_cli_with_deps(
            &args(&["offload", "restore", "--all"]),
            journal.path(),
            &runner,
            &PanicDownload,
            &UnusedHttp,
            dirs(restic_dir.path(), None),
        );
        assert_resolved_restic(&runner.programs.borrow(), &expected, &decoy);
    }

    #[test]
    fn ac10_operated_run_reaches_ensure_rclone_through_run_cli_with_deps() {
        let restic_dir = tempfile::tempdir().unwrap();
        let rclone_dir = tempfile::tempdir().unwrap();
        write_ready_restic(restic_dir.path());
        write_ready_rclone(rclone_dir.path());
        let journal = operated_journal();
        let runner = RecordingRunner::new();
        run_cli_with_deps(
            &args(&["run"]),
            journal.path(),
            &runner,
            &PanicDownload,
            &BrokerHttp::new(),
            dirs(restic_dir.path(), Some(rclone_dir.path())),
        );
        assert!(rclone_program(&runner.argvs.borrow()).is_some());
    }

    #[test]
    fn ac11_read_only_verbs_do_not_resolve_tools() {
        let journal = tempfile::tempdir().unwrap();
        let runner = RecordingRunner::new();
        for argv in [
            args(&["status"]),
            args(&["destination", "show"]),
            args(&["recovery-key", "show"]),
            args(&["--help"]),
            args(&["offload", "status"]),
        ] {
            run_cli_with_deps(
                &argv,
                journal.path(),
                &runner,
                &PanicDownload,
                &UnusedHttp,
                ToolInstallDirs::default(),
            );
        }
        assert!(runner.programs.borrow().is_empty());
    }
}
