// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{self, IsTerminal, Read};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Datelike, NaiveDate, Utc};
use serde_json::{Map, Value, json};
use solstone_core_artifact_download::UreqByteDownload;
use solstone_core_backup::{
    Destination, HostedBinding, confirm_recovery_key, format_recovery_key_display,
    generate_and_store_keys, generate_daily_key, get_backup_config, get_destination, get_keys,
    save_hosted_binding, set_destination, set_enabled, set_recovery_key_confirmed, status_view,
};
use solstone_core_backup_runtime::{
    BackupResult, BackupServices, Clock, NativeJournalMaintenance, PruneResult, ResticKeyError,
    RotationResult, SystemToolRunner, TeardownResult, UreqHttpTransport, ensure_restic,
    restore_journal, rotate_recovery_key, run_backup, run_prune, teardown_backup,
    validate_destination,
};
use solstone_core_offload::{
    OffloadResult, RestoreResult as OffloadRestoreResult, build_offload_status,
    format_offload_result, restore_all_offload, restore_offload_day, run_offload,
};

pub const USAGE: &str = "usage: journal backup <command> [options]\n";
pub const OFFLOAD_USAGE: &str = "usage: journal backup offload <command> [options]\n";
const MAX_JSON_STDIN_BYTES: usize = 1024 * 1024; // Keep in lockstep with solstone-core main.rs.

#[derive(Debug, PartialEq, Eq)]
pub struct CliRun {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

pub fn run_cli(args: &[String], journal: &Path) -> CliRun {
    let args = args
        .iter()
        .filter(|arg| !matches!(arg.as_str(), "-v" | "--verbose" | "-d" | "--debug"))
        .cloned()
        .collect::<Vec<_>>();
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        return success(
            if args.first().is_some_and(|arg| arg == "offload") {
                OFFLOAD_USAGE
            } else {
                USAGE
            }
            .to_owned(),
        );
    }
    let runner = SystemToolRunner;
    let http = UreqHttpTransport;
    let clock = ProductionClock;
    let maintenance = NativeJournalMaintenance;
    let services = BackupServices {
        runner: &runner,
        http: &http,
        clock: &clock,
        restic_path: Path::new("restic"),
        rclone_path: None,
        version: env!("CARGO_PKG_VERSION"),
        journal_maintenance: &maintenance,
    };
    run_cli_with(&args, journal, &services)
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

fn run_cli_with(args: &[String], journal: &Path, services: &BackupServices<'_>) -> CliRun {
    match args {
        [command] if command == "status" => render_json(status_view(journal).map(Value::Object)),
        [destination, command] if destination == "destination" && command == "show" => {
            match status_view(journal) {
                Ok(view) => render_json(Ok::<_, String>(
                    view.get("destination")
                        .cloned()
                        .expect("status has destination"),
                )),
                Err(error) => runtime_error(error.to_string()),
            }
        }
        [destination, command] if destination == "destination" && command == "set-hosted" => {
            set_hosted(journal)
        }
        [destination, command] if destination == "destination" && command == "set" => {
            destination_set(journal, services)
        }
        [key, command] if key == "recovery-key" && command == "show" => recovery_key_show(journal),
        [command] if command == "enable" => enable(journal, services),
        [command] if command == "run" => backup_run(journal, services),
        [command] if command == "prune" => backup_prune(journal, services),
        [key, command] if key == "recovery-key" && command == "rotate" => {
            recovery_key_rotate(journal, services)
        }
        [offload, rest @ ..] if offload == "offload" => offload_command(rest, journal, services),
        [command] if command == "off" => turn_off(false, journal, services),
        [command, yes] if command == "off" && yes == "--yes" => turn_off(true, journal, services),
        [command] if command == "restore" => restore(journal, services),
        _ => usage_error(&args.join(" ")),
    }
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

fn restore(journal: &Path, services: &BackupServices<'_>) -> CliRun {
    let payload = match read_stdin_json() {
        Ok(payload) => payload,
        Err(message) => return runtime_error(message),
    };
    restore_from_payload(&payload, |destination, recovery_key| {
        restore_journal(journal, services, destination, recovery_key)
    })
}

fn restore_from_payload(
    payload: &Map<String, Value>,
    restore: impl FnOnce(Destination, &str) -> solstone_core_backup_runtime::RestoreResult,
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
    restore_result(restore(destination, recovery_key))
}

fn restore_result(result: solstone_core_backup_runtime::RestoreResult) -> CliRun {
    let reason = result.reason_code.as_deref().unwrap_or("failed");
    match result.status.as_str() {
        "ok" => success(format!(
            "Restore complete: {} bytes, integrity_ok={}, resumable={}.\n",
            result.bytes_restored.unwrap_or(0),
            python_bool(result.integrity_ok),
            python_bool(result.resumable),
        )),
        "degraded" => {
            let detail = if reason == "integrity_unverified" {
                "integrity verification could not run (the repository was busy or timed out)"
            } else {
                "integrity verification failed — the backup copy may be damaged"
            };
            runtime_error(format!(
                "Restored {} bytes and saved the recovery key, but {detail} (reason_code={reason}).",
                result.bytes_restored.unwrap_or(0),
            ))
        }
        _ => runtime_error(format!("Restore failed: {reason}.")),
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
    let json = match args {
        [] => false,
        [option] if option == "--json" => true,
        _ => return offload_usage_error(&args.join(" ")),
    };
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
    let dry_run = match args {
        [] => false,
        [option] if option == "--dry-run" => true,
        _ => return offload_usage_error(&args.join(" ")),
    };
    offload_run_result(run_offload(journal, services, dry_run))
}

fn offload_run_result(result: OffloadResult) -> CliRun {
    // A stalled offload is an operational result, not a CLI failure.
    success(format!("{}\n", format_offload_result(&result)))
}

fn offload_restore(args: &[String], journal: &Path, services: &BackupServices<'_>) -> CliRun {
    let mut json_output = false;
    let mut all = false;
    let mut day = None;
    for argument in args {
        match argument.as_str() {
            "--json" if !json_output => json_output = true,
            "--all" if !all => all = true,
            option if option.starts_with('-') => return offload_usage_error(&args.join(" ")),
            _ if day.is_none() => day = Some(argument.as_str()),
            _ => return offload_usage_error(&args.join(" ")),
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
            "backup offload restore: status={} reason={} segments_restored={} files_restored={} bytes_restored={}\n",
            result.status,
            result.reason.as_deref().unwrap_or("None"),
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

fn backup_run(journal: &Path, services: &BackupServices<'_>) -> CliRun {
    backup_run_result(run_backup(journal, services))
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
            restic_path: Path::new("restic"),
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
            restic_path: Path::new("restic"),
            rclone_path: None,
            version: "test",
            journal_maintenance: maintenance,
        }
    }

    fn payload(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
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
    fn four_subcommands_reach_their_unconfigured_runtime_bodies() {
        let journal = tempfile::tempdir().unwrap();
        let runner = UnusedRunner;
        let http = UnusedHttp;
        let clock = FixedClock;
        let maintenance = UnusedMaintenance;
        let services = unconfigured_services(&runner, &http, &clock, &maintenance);

        for (args, expected) in [
            (
                vec!["run".into()],
                "Backup skipped (not enabled or not configured).\n",
            ),
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
                    Ok(PathBuf::from("restic"))
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
            || Ok(PathBuf::from("restic")),
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
        for recovery_key in [Value::Null, Value::String("   ".into())] {
            let mut input = s3_payload();
            input.insert("recovery_key".into(), recovery_key);
            let output = restore_from_payload(&input, |_, _| {
                panic!("missing recovery key must not restore")
            });
            assert_eq!(output.stderr, "Error: Missing recovery_key.\n");
        }
        let mut input = s3_payload();
        input.insert(
            "recovery_key".into(),
            Value::String(" recovery-key ".into()),
        );
        let trimmed = restore_from_payload(&input, |_, recovery_key| {
            assert_eq!(recovery_key, "recovery-key");
            solstone_core_backup_runtime::RestoreResult {
                status: "error".into(),
                reason_code: Some("failed".into()),
                integrity_ok: false,
                resumable: false,
                bytes_restored: None,
            }
        });
        assert_eq!(trimmed.stderr, "Error: Restore failed: failed.\n");
        let base =
            |status: &str, reason_code: Option<&str>| solstone_core_backup_runtime::RestoreResult {
                status: status.into(),
                reason_code: reason_code.map(str::to_owned),
                integrity_ok: false,
                resumable: false,
                bytes_restored: Some(42),
            };
        assert_eq!(
            restore_result(base("error", Some("timeout"))).stderr,
            "Error: Restore failed: timeout.\n"
        );
        assert_eq!(
            restore_result(base("degraded", Some("integrity_unverified"))).stderr,
            "Error: Restored 42 bytes and saved the recovery key, but integrity verification could not run (the repository was busy or timed out) (reason_code=integrity_unverified).\n"
        );
        assert_eq!(
            restore_result(base("degraded", Some("integrity_failed"))).stderr,
            "Error: Restored 42 bytes and saved the recovery key, but integrity verification failed — the backup copy may be damaged (reason_code=integrity_failed).\n"
        );
        let mut ok = base("ok", None);
        ok.integrity_ok = true;
        ok.resumable = true;
        assert_eq!(
            restore_result(ok).stdout,
            "Restore complete: 42 bytes, integrity_ok=True, resumable=True.\n"
        );
    }

    #[test]
    fn destination_set_and_restore_positionals_use_root_usage() {
        let journal = tempfile::tempdir().unwrap();
        let runner = UnusedRunner;
        let http = UnusedHttp;
        let clock = FixedClock;
        let maintenance = UnusedMaintenance;
        let services = unconfigured_services(&runner, &http, &clock, &maintenance);
        for args in [
            vec!["destination".into(), "set".into(), "extra".into()],
            vec!["restore".into(), "extra".into()],
        ] {
            let output = run_cli_with(&args, journal.path(), &services);
            assert_eq!(output.exit_code, 2);
            assert!(output.stderr.starts_with(USAGE));
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
                Ok(PathBuf::from("restic"))
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
                || Ok(PathBuf::from("restic")),
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
                || Ok(PathBuf::from("restic")),
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
                Ok(PathBuf::from("restic"))
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
                        Ok(PathBuf::from("restic"))
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
