// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows-only process and filesystem boundary for the limited-token rail.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use solstone_core_win_owner_rail::{
    ACE_MUTATION_MASK, CONTROL_ID, LEASE_PATH, LeaseInput, LeaseRecord, LeaseState,
    OnlogonProbeReport, PROBE_SCHEMA, ProbeApiError, ProbeApiErrorSpace, ProbeComSection,
    ProbeDescriptorSource, ProbeFolderAclStage, ProbeOutcome, ProbePriority, ProbeResolutionAction,
    ProbeStage, ProbeStageStatus, ProbeTokenGate, ProbeTokenSection, ProbeVerifyInput,
    ProbeXmlSection, RAIL_ROOT, RailError, ResultRecord, TaskRuntime, TerminalState,
    TokenAttestation, TokenElevationClass, classify_probe_sid, classify_terminal,
    classify_token_elevation_type, folder_allows_ordinary_mutation, mint_nonce,
    parse_probe_task_xml, parse_task_runtime_csv, parse_task_xml, probe_com_folder_path,
    probe_com_task_name, probe_exits_successfully, probe_marker, probe_token_gate,
    probe_xml_task_name, recovery_decision, render_probe_task_xml, require_clean_worktree,
    schtasks_create_argv, schtasks_create_xml_argv, schtasks_delete_argv,
    schtasks_query_proves_task_absent, schtasks_query_runtime_argv, schtasks_query_xml_argv,
    schtasks_run_argv, select_probe_outcome, sha256_hex, verify_attestation,
    verify_probe_task_definition, verify_result, verify_result_binding, verify_task_definition,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize,
};
use windows::Win32::System::TaskScheduler::{
    IExecAction, ILogonTrigger, ITaskFolder, ITaskService, TASK_ACTION_EXEC, TASK_CREATE,
    TASK_LOGON_INTERACTIVE_TOKEN, TASK_RUNLEVEL_LUA, TASK_TRIGGER_LOGON, TaskScheduler,
};
use windows::Win32::System::Variant::VARIANT;
use windows::core::{BSTR, Interface};
use windows_sys::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, GENERIC_ALL, GetLastError, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW, SDDL_REVISION_1,
    SE_FILE_OBJECT, SetNamedSecurityInfoW,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_REVISION, AddAccessAllowedAceEx,
    CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, GetAce, GetLengthSid,
    GetSecurityDescriptorDacl, GetTokenInformation, INHERIT_ONLY_ACE, InitializeAcl,
    LUID_AND_ATTRIBUTES, LookupAccountNameW, LookupAccountSidW, LookupPrivilegeValueW,
    OBJECT_INHERIT_ACE, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SE_BACKUP_NAME,
    SE_RESTORE_NAME, TOKEN_ELEVATION, TOKEN_PRIVILEGES, TOKEN_QUERY, TOKEN_USER, TokenElevation,
    TokenElevationType, TokenPrivileges, TokenSessionId, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_NONE, FILE_WRITE_DATA,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

pub const ORDINARY_OWNER_CARGO_TEST: &str = "cargo test --manifest-path core\\Cargo.toml --locked -p solstone-core-journal-io --test windows_ordinary_owner_inventory --features test-hooks -- --nocapture";
const OWNER_ACCOUNT_ENV: &str = "SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT";
const REFS_ROOT_ENV: &str = "SOLSTONE_JOURNAL_WIN_REFS_ROOT";
const POLL_LIMIT: usize = 120;
const POLL_INTERVAL: Duration = Duration::from_secs(1);
const FAILED_CHILD_OUTPUT_TAIL_BYTES: usize = 8 * 1024;

pub fn run(arguments: Vec<std::ffi::OsString>) -> Result<(), String> {
    let command = arguments
        .first()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "missing rail subcommand".to_owned())?;
    let options = Options::parse(&arguments[1..])?;
    match command {
        "recover-held" => recover_held(&options),
        "prepare" => prepare(&options),
        "launch" => launch(&options),
        "await" => await_result(&options),
        "cleanup" => cleanup(&options),
        "limited-child" => limited_child(&options),
        "probe-onlogon" => probe_onlogon(&options),
        _ => Err(format!("unknown rail subcommand: {command}")),
    }
}

#[derive(Default)]
struct Options {
    values: std::collections::BTreeMap<String, String>,
}

impl Options {
    fn parse(arguments: &[std::ffi::OsString]) -> Result<Self, String> {
        let mut values = std::collections::BTreeMap::new();
        let mut iterator = arguments.iter();
        while let Some(key) = iterator.next() {
            let key = key
                .to_str()
                .ok_or_else(|| "non-UTF-8 argument".to_owned())?;
            if !key.starts_with("--") {
                return Err(format!("invalid argument: {key}"));
            }
            let value = iterator
                .next()
                .and_then(|value| value.to_str())
                .ok_or_else(|| format!("missing value for {key}"))?;
            if values.insert(key.to_owned(), value.to_owned()).is_some() {
                return Err(format!("duplicate option: {key}"));
            }
        }
        Ok(Self { values })
    }

    fn required(&self, name: &str) -> Result<&str, String> {
        self.values
            .get(name)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("missing required option {name}"))
    }

    fn optional(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }
}

fn prepare(options: &Options) -> Result<(), String> {
    let lease_path = options.required("--lease")?;
    if lease_path != LEASE_PATH {
        return Err("prepare only accepts the canonical rail lease path".to_owned());
    }
    let owner_account = required_owner_account(options)?;
    let owner = lookup_account_sid(owner_account)?;
    let parent = current_token_sid()?;
    ensure_rail_directories(&owner, &parent)?;
    let nonce = mint_nonce().map_err(display_rail_error)?;
    let worktree = canonical(options.required("--worktree")?)?;
    let worker_path = canonical(options.required("--worker")?)?;
    let worker_sha256 = file_sha256(&worker_path)?;
    let expected_commit = options.required("--expected-commit")?.to_owned();
    let expected_cargo_lock_sha256 = options.required("--expected-lock")?.to_owned();
    let refs_root = refs_root_from_environment(options)?;
    let input = LeaseInput {
        nonce,
        lease_path: lease_path.to_owned(),
        expected_commit,
        expected_cargo_lock_sha256,
        expected_owner_account: owner_account.to_owned(),
        expected_owner_sid: owner.text,
        expected_session: 1,
        worktree: worktree.clone(),
        worker_path: worker_path.clone(),
        worker_sha256,
        refs_root,
        created_at_unix_seconds: now_seconds()?,
    };
    let lease = LeaseRecord::new(input);
    create_lease_new(lease_path, &lease)?;
    if let Err(error) = run_schtasks(schtasks_create_argv(&lease)) {
        // A process or transport error does not establish that `/Create` did not take effect.
        // Retain the sole nonce-derived recovery handle rather than guessing the task absent.
        let mut held = lease;
        hold_lease(&mut held, lease_path, "create-ambiguous", &error)?;
        return Err(format!(
            "ordinary-owner prepare retained recovery lease nonce={}: {error}",
            held.nonce
        ));
    }
    if let Err(error) = verify_created_task(&lease) {
        // Creation is known to have succeeded, so contain only this nonce-derived task before
        // releasing the lease.  If any containment step is uncertain, retain the lease.
        if let Err(containment) = cleanup_lease(&lease, lease_path) {
            let mut held = lease;
            hold_lease(
                &mut held,
                lease_path,
                "post-create-containment",
                &containment,
            )?;
            return Err(format!(
                "ordinary-owner prepare retained recovery lease nonce={}: {}",
                held.nonce,
                conformance_diagnostic(&held, "post-create-definition", &error)
            ));
        }
        return Err(format!(
            "ordinary-owner prepare rejected {}",
            conformance_diagnostic(&lease, "post-create-definition", &error)
        ));
    }
    Ok(())
}

fn recover_held(options: &Options) -> Result<(), String> {
    let lease_path = options.required("--lease")?;
    let lease = match read_lease(lease_path) {
        Ok(lease) => lease,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("read held lease: {error}")),
    };
    let runtime = match query_runtime(&lease) {
        Ok(runtime) => Some(runtime),
        Err(runtime_error) => {
            if confirm_task_absent(&lease).is_ok() {
                return release_absent_lease(&lease, lease_path);
            }
            return Err(format!(
                "held ordinary-owner lease task state is unprovable nonce={}; supported recovery: repair local scheduler access then rerun recover-held ({})",
                lease.nonce,
                conformance_diagnostic(&lease, "held-runtime-query", &runtime_error)
            ));
        }
    };
    let task_exists = true;
    let terminal_state = runtime
        .as_ref()
        .map(|runtime| classify_terminal(runtime, lease.launch_observed_last_run_time.as_deref()));
    let terminal = matches!(
        terminal_state,
        Some(TerminalState::Verified) | Some(TerminalState::Rejected)
    );
    let safely_unlaunched = lease.launch_boundary_unix_seconds.is_none()
        && runtime.as_ref().is_some_and(|runtime| {
            runtime.status.eq_ignore_ascii_case("ready")
                && (runtime.last_run_time.is_empty()
                    || runtime.last_run_time.eq_ignore_ascii_case("n/a"))
        });
    let task_reclaimable = terminal || safely_unlaunched;
    match recovery_decision(Some(&lease), task_exists, task_reclaimable) {
        solstone_core_win_owner_rail::RecoveryDecision::CleanThenReclaim => {
            // Before deleting any still-present task, prove it is this lease's exact nonce-bound
            // task.  A task collision or scheduler substitution must stay fenced even if it is
            // inactive.  A conclusively unlaunched task has no result to bless; every terminal
            // task (including a nonzero terminal outcome) must have a bound result before cleanup.
            if task_exists {
                let definition = query_definition(&lease)?;
                verify_task_definition(&lease, &definition).map_err(display_rail_error)?;
            }
            if terminal {
                let result = read_result(&lease)
                    .map_err(|error| format!("read held-lease result: {error}"))?;
                verify_result_binding(&lease, &result).map_err(display_rail_error)?;
            }
            cleanup_lease(&lease, lease_path)
        }
        solstone_core_win_owner_rail::RecoveryDecision::FailClosed => {
            Err("held ordinary-owner lease has a non-terminal scheduled task".to_owned())
        }
        solstone_core_win_owner_rail::RecoveryDecision::NoHeldLease => Ok(()),
    }
}

fn launch(options: &Options) -> Result<(), String> {
    let lease_path = options.required("--lease")?;
    let mut lease = read_lease(lease_path).map_err(|error| format!("read lease: {error}"))?;
    if lease.state != LeaseState::Active {
        return Err("lease is not active".to_owned());
    }
    verify_worker_and_source(&lease)?;
    let definition = query_definition(&lease)?;
    verify_task_definition(&lease, &definition).map_err(display_rail_error)?;
    let before = query_runtime(&lease).ok();
    run_schtasks(schtasks_run_argv(&lease.task_name))?;
    lease.launch_boundary_unix_seconds = Some(now_seconds()?);
    lease.launch_observed_last_run_time = before.map(|runtime| runtime.last_run_time);
    lease
        .transition(LeaseState::Launched)
        .map_err(display_rail_error)?;
    write_lease(lease_path, &lease)
}

fn await_result(options: &Options) -> Result<(), String> {
    let lease_path = options.required("--lease")?;
    let mut lease = read_lease(lease_path).map_err(|error| format!("read lease: {error}"))?;
    if lease.state != LeaseState::Launched || lease.launch_boundary_unix_seconds.is_none() {
        return Err("lease was not launched".to_owned());
    }
    for _ in 0..POLL_LIMIT {
        let runtime = match query_runtime(&lease) {
            Ok(runtime) => runtime,
            Err(error) => {
                hold_lease(&mut lease, lease_path, "await-runtime-query", &error)?;
                return Err(format!(
                    "ordinary-owner scheduled task state retained nonce={}: {error}",
                    lease.nonce
                ));
            }
        };
        match classify_terminal(&runtime, lease.launch_observed_last_run_time.as_deref()) {
            TerminalState::Pending => std::thread::sleep(POLL_INTERVAL),
            TerminalState::Rejected => {
                hold_lease(
                    &mut lease,
                    lease_path,
                    "await-scheduler-rejected",
                    "scheduler-reported-terminal-rejection",
                )?;
                return Err("ordinary-owner scheduled task failed".to_owned());
            }
            TerminalState::Verified => {
                let definition = match query_definition(&lease) {
                    Ok(definition) => definition,
                    Err(error) => {
                        hold_lease(&mut lease, lease_path, "await-definition-query", &error)?;
                        return Err(format!(
                            "ordinary-owner scheduled task state retained nonce={}: {error}",
                            lease.nonce
                        ));
                    }
                };
                if let Err(error) =
                    verify_task_definition(&lease, &definition).map_err(display_rail_error)
                {
                    hold_lease(&mut lease, lease_path, "await-definition-binding", &error)?;
                    return Err(format!(
                        "ordinary-owner scheduled task state retained nonce={}: {error}",
                        lease.nonce
                    ));
                }
                let result = match read_result(&lease) {
                    Ok(result) => result,
                    Err(error) => {
                        hold_lease(&mut lease, lease_path, "await-result-read", &error)?;
                        return Err(format!(
                            "ordinary-owner scheduled task state retained nonce={}: {error}",
                            lease.nonce
                        ));
                    }
                };
                if let Err(error) =
                    verify_result_binding(&lease, &result).map_err(display_rail_error)
                {
                    hold_lease(&mut lease, lease_path, "await-result-binding", &error)?;
                    return Err(format!(
                        "ordinary-owner scheduled task state retained nonce={}: {error}",
                        lease.nonce
                    ));
                }
                lease
                    .transition(LeaseState::TerminalVerified)
                    .map_err(display_rail_error)?;
                write_lease(lease_path, &lease)?;
                // A bound terminal test failure is safe to delete, but not a pass.  The driver
                // invokes `cleanup`, whose state guard permits that exact sequence only here.
                if let Err(error) = verify_result(&lease, &result) {
                    return Err(format!(
                        "ordinary-owner bound test failed: {}; limited-child output tail:\n{}",
                        display_rail_error(error),
                        failed_child_output_tail(&lease.output_path)
                    ));
                }
                println!("JOURNAL_WIN_CI_ORDINARY_OWNER_CONTROL=passed");
                if !lease.refs_root.is_empty() {
                    println!("JOURNAL_WIN_CI_ORDINARY_OWNER_REFS=passed");
                }
                return Ok(());
            }
        }
    }
    hold_lease(
        &mut lease,
        lease_path,
        "await-timeout",
        "fresh-terminal-result-not-observed",
    )?;
    Err("ordinary-owner scheduled task timed out".to_owned())
}

fn failed_child_output_tail(path: &str) -> String {
    match File::open(path) {
        Ok(mut output) => {
            let length = match output.metadata() {
                Ok(metadata) => metadata.len(),
                Err(error) => return format!("<unable to inspect limited-child output: {error}>"),
            };
            let start = length.saturating_sub(FAILED_CHILD_OUTPUT_TAIL_BYTES as u64);
            if let Err(error) = output.seek(SeekFrom::Start(start)) {
                return format!("<unable to seek limited-child output: {error}>");
            }
            let mut tail = Vec::with_capacity(FAILED_CHILD_OUTPUT_TAIL_BYTES);
            if let Err(error) = output
                .take(FAILED_CHILD_OUTPUT_TAIL_BYTES as u64)
                .read_to_end(&mut tail)
            {
                return format!("<unable to read limited-child output: {error}>");
            }
            String::from_utf8_lossy(&tail).into_owned()
        }
        Err(error) => format!("<unable to read limited-child output: {error}>"),
    }
}

fn cleanup(options: &Options) -> Result<(), String> {
    let lease_path = options.required("--lease")?;
    let lease = read_lease(lease_path).map_err(|error| format!("read lease: {error}"))?;
    if lease.state != LeaseState::TerminalVerified {
        return Err(format!(
            "refusing ordinary-owner cleanup before verified terminal receipt; retained nonce={} state={:?}",
            lease.nonce, lease.state
        ));
    }
    cleanup_lease(&lease, lease_path)
}

fn limited_child(options: &Options) -> Result<(), String> {
    let lease_path = options.required("--lease")?;
    let nonce = options.required("--nonce")?;
    let lease = read_lease(lease_path).map_err(|error| format!("read lease: {error}"))?;
    if lease.nonce != nonce || lease.control_id != CONTROL_ID || lease.state != LeaseState::Launched
    {
        return Err("limited child lease identity is invalid".to_owned());
    }
    let before = limited_attestation(&lease)?;
    verify_worker_and_source(&lease)?;
    let output = cargo_test(&lease)?;
    let output_text = render_output(&output);
    write_output_new(&lease.output_path, output_text.as_bytes())?;
    let after = limited_attestation(&lease)?;
    let result = ResultRecord {
        schema: "solstone.journal.win-owner-rail.result.v1".to_owned(),
        nonce: lease.nonce.clone(),
        selected_control: Some(CONTROL_ID.to_owned()),
        executed_control: Some(CONTROL_ID.to_owned()),
        payload_sha256: lease.payload_sha256.clone(),
        passed: output.status.success()
            && output_text.contains("JOURNAL_WIN_CI_ORDINARY_OWNER_CONTROL=passed"),
        cargo_exit_code: output.status.code().unwrap_or(-1),
        ordinary_owner_marker: output_text.contains("JOURNAL_WIN_CI_ORDINARY_OWNER_CONTROL=passed"),
        ordinary_owner_refs_marker: output_text
            .contains("JOURNAL_WIN_CI_ORDINARY_OWNER_REFS=passed"),
        before,
        after,
        error: None,
    };
    write_result_new(&lease.result_path, &result)
}

fn cargo_test(lease: &LeaseRecord) -> Result<Output, String> {
    // The elevated rail owns core\target, so the limited child deliberately uses its ACL-granted
    // ProgramData target directory through CARGO_TARGET_DIR instead of inheriting that target tree.
    let mut command_words = ORDINARY_OWNER_CARGO_TEST.split_ascii_whitespace();
    let executable = command_words
        .next()
        .ok_or_else(|| "ordinary-owner command is empty".to_owned())?;
    Command::new(executable)
        .current_dir(&lease.worktree)
        .env("CARGO_TARGET_DIR", &lease.target_dir)
        .env(REFS_ROOT_ENV, &lease.refs_root)
        .args(command_words)
        .output()
        .map_err(|error| format!("run ordinary-owner cargo test: {error}"))
}

fn refs_root_from_environment(options: &Options) -> Result<String, String> {
    let name = options.required("--refs-root-env")?;
    if name != REFS_ROOT_ENV {
        return Err("prepare only accepts the canonical ReFS-root environment name".to_owned());
    }
    std::env::var(REFS_ROOT_ENV)
        .map_err(|_| "configured ReFS-root environment is unavailable to prepare".to_owned())
}

fn limited_attestation(lease: &LeaseRecord) -> Result<TokenAttestation, String> {
    let resolved_owner = lookup_account_sid(&lease.expected_owner_account)?;
    if resolved_owner.text != lease.expected_owner_sid {
        return Err("owner account SID changed after lease creation".to_owned());
    }
    let attestation = current_token_attestation()?;
    verify_attestation(lease, &attestation).map_err(display_rail_error)?;
    Ok(attestation)
}

fn verify_worker_and_source(lease: &LeaseRecord) -> Result<(), String> {
    if file_sha256(&lease.worker_path)? != lease.worker_sha256 {
        return Err("worker payload hash changed after lease creation".to_owned());
    }
    let head =
        command_text(Command::new("git").args(["-C", &lease.worktree, "rev-parse", "HEAD"]))?;
    if head.trim() != lease.expected_commit {
        return Err("worktree HEAD differs from lease commit".to_owned());
    }
    let lock = file_sha256(Path::new(&lease.worktree).join("core").join("Cargo.lock"))?;
    if lock != lease.expected_cargo_lock_sha256 {
        return Err("worktree Cargo.lock hash differs from lease".to_owned());
    }
    let status = command_text(Command::new("git").args([
        "-C",
        &lease.worktree,
        "status",
        "--porcelain",
        "--untracked-files=all",
    ]))?;
    require_clean_worktree(&status).map_err(display_rail_error)
}

fn verify_created_task(lease: &LeaseRecord) -> Result<(), String> {
    let definition = query_definition(lease)?;
    verify_task_definition(lease, &definition).map_err(display_rail_error)
}

fn query_definition(
    lease: &LeaseRecord,
) -> Result<solstone_core_win_owner_rail::TaskDefinition, String> {
    let output = Command::new("schtasks.exe")
        .args(schtasks_query_xml_argv(&lease.task_name))
        .output()
        .map_err(|error| format!("query scheduled-task XML: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "query scheduled-task XML failed: {}",
            render_output(&output)
        ));
    }
    let mut definition =
        parse_task_xml(&String::from_utf8_lossy(&output.stdout)).map_err(display_rail_error)?;
    if !definition.principal_sid.starts_with("S-") {
        definition.principal_sid = lookup_account_sid(&definition.principal_sid)?.text;
    }
    Ok(definition)
}

fn query_runtime(lease: &LeaseRecord) -> Result<TaskRuntime, String> {
    // `/XML` has task definition only; CSV `/V` provides the fresh runtime fields we bind to launch.
    let output = Command::new("schtasks.exe")
        .args(schtasks_query_runtime_argv(&lease.task_name))
        .output()
        .map_err(|error| format!("query scheduled-task runtime: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "query scheduled-task runtime failed: {}",
            render_output(&output)
        ));
    }
    parse_task_runtime_csv(&String::from_utf8_lossy(&output.stdout)).map_err(display_rail_error)
}

fn run_schtasks(arguments: Vec<String>) -> Result<(), String> {
    let output = Command::new("schtasks.exe")
        .args(arguments)
        .output()
        .map_err(|error| format!("run schtasks: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("schtasks failed: {}", render_output(&output)))
    }
}

fn cleanup_lease(lease: &LeaseRecord, lease_path: &str) -> Result<(), String> {
    let delete = run_schtasks(schtasks_delete_argv(&lease.task_name));
    if let Err(absence) = confirm_task_absent(lease) {
        return match delete {
            Ok(()) => Err(format!(
                "ordinary-owner delete did not establish exact task absence nonce={}: {absence}",
                lease.nonce
            )),
            Err(delete) => Err(format!(
                "ordinary-owner delete failed and exact task absence is unproved nonce={}: {delete}; {absence}",
                lease.nonce
            )),
        };
    }
    release_absent_lease(lease, lease_path)
}

fn confirm_task_absent(lease: &LeaseRecord) -> Result<(), String> {
    let output = Command::new("schtasks.exe")
        .args(schtasks_query_xml_argv(&lease.task_name))
        .output()
        .map_err(|error| format!("query exact task absence: {error}"))?;
    if output.status.success() {
        return Err(format!(
            "exact nonce task still exists nonce={}",
            lease.nonce
        ));
    }
    // The scheduler reports an absent exact task as either a missing file or missing task path.
    // Any other nonzero query (access, transport, parser, or scheduler failure) remains ambiguous
    // and retains the lease rather than treating a failing command as proof of absence.
    if schtasks_query_proves_task_absent(&render_output(&output)) {
        return Ok(());
    }
    Err(format!(
        "exact task absence query was not a task-not-found result nonce={}",
        lease.nonce
    ))
}

fn release_absent_lease(lease: &LeaseRecord, lease_path: &str) -> Result<(), String> {
    remove_nonce_artifact(&lease.result_path)?;
    remove_nonce_artifact(&lease.output_path)?;
    remove_nonce_directory(&lease.target_dir)?;
    fs::remove_file(lease_path).map_err(|error| format!("remove lease: {error}"))
}

fn hold_lease(
    lease: &mut LeaseRecord,
    lease_path: &str,
    phase: &str,
    error: &str,
) -> Result<(), String> {
    if lease.state != LeaseState::Held {
        lease
            .transition(LeaseState::Held)
            .map_err(display_rail_error)?;
    }
    lease.last_error = Some(conformance_diagnostic(lease, phase, error));
    write_lease(lease_path, lease)
}

fn conformance_diagnostic(lease: &LeaseRecord, phase: &str, error: &str) -> String {
    let classification = error
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '-')
        .find(|word| word.starts_with("task-") || word.starts_with("ordinary-owner-"))
        .unwrap_or("scheduler-or-transport-uncertainty");
    format!(
        "ordinary-owner-conformance-v1 phase={phase} nonce={} classification={classification}",
        lease.nonce
    )
}

fn remove_nonce_artifact(path: &str) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove nonce artifact {path}: {error}")),
    }
}

fn remove_nonce_directory(path: &str) -> Result<(), String> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove nonce target directory {path}: {error}")),
    }
}

fn read_lease(path: &str) -> io::Result<LeaseRecord> {
    let file = File::open(path)?;
    serde_json::from_reader(file).map_err(io::Error::other)
}

fn write_lease(path: &str, lease: &LeaseRecord) -> Result<(), String> {
    let file = File::create(path).map_err(|error| format!("write lease: {error}"))?;
    serde_json::to_writer(file, lease).map_err(|error| format!("serialize lease: {error}"))
}

fn create_lease_new(path: &str, lease: &LeaseRecord) -> Result<(), String> {
    let mut file = create_new(path)?;
    serde_json::to_writer(&mut file, lease)
        .map_err(|error| format!("serialize new lease: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("sync new lease: {error}"))
}

fn read_result(lease: &LeaseRecord) -> Result<ResultRecord, String> {
    let file =
        File::open(&lease.result_path).map_err(|error| format!("read limited result: {error}"))?;
    serde_json::from_reader(file).map_err(|error| format!("parse limited result: {error}"))
}

fn write_result_new(path: &str, result: &ResultRecord) -> Result<(), String> {
    let mut file = create_new(path)?;
    serde_json::to_writer(&mut file, result)
        .map_err(|error| format!("serialize limited result: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("sync limited result: {error}"))
}

fn write_output_new(path: &str, output: &[u8]) -> Result<(), String> {
    let mut file = create_new(path)?;
    file.write_all(output)
        .map_err(|error| format!("write limited output: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("sync limited output: {error}"))
}

fn create_new(path: &str) -> Result<File, String> {
    let wide = wide(OsStr::new(path));
    // SAFETY: `wide` is NUL-terminated, no sharing prevents replacement races, and CREATE_NEW
    // is the exclusive lease/result primitive rather than the non-exclusive temp-and-rename idiom.
    #[allow(unsafe_code)]
    let raw = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_WRITE_DATA,
            FILE_SHARE_NONE,
            std::ptr::null(),
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(format!(
            "CreateFileW(CREATE_NEW) {}: {}",
            path,
            io::Error::last_os_error()
        ));
    }
    // SAFETY: CreateFileW returned a valid, uniquely-owned handle.
    #[allow(unsafe_code)]
    Ok(unsafe { File::from_raw_handle(raw) })
}

#[derive(Clone)]
struct Sid {
    bytes: Vec<u8>,
    text: String,
}

fn required_owner_account(options: &Options) -> Result<&str, String> {
    options
        .optional("--owner-account")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{OWNER_ACCOUNT_ENV} is required"))
}

fn lookup_account_sid(account: &str) -> Result<Sid, String> {
    let account = wide(OsStr::new(account));
    let mut sid_length = 0_u32;
    let mut domain_length = 0_u32;
    let mut use_type = 0_i32;
    // SAFETY: documented sizing call with null output buffers.
    #[allow(unsafe_code)]
    let first = unsafe {
        LookupAccountNameW(
            std::ptr::null(),
            account.as_ptr(),
            std::ptr::null_mut(),
            &mut sid_length,
            std::ptr::null_mut(),
            &mut domain_length,
            &mut use_type,
        )
    };
    // SAFETY: this retrieves the failure from the sizing call.
    #[allow(unsafe_code)]
    let error = unsafe { GetLastError() };
    if first != 0 || error != ERROR_INSUFFICIENT_BUFFER || sid_length == 0 {
        return Err(format!(
            "size SID for account: {}",
            io::Error::last_os_error()
        ));
    }
    let mut sid = vec![0_u8; sid_length as usize];
    let mut domain = vec![0_u16; domain_length as usize + 1];
    // SAFETY: output buffers are sized by LookupAccountNameW's sizing result.
    #[allow(unsafe_code)]
    let result = unsafe {
        LookupAccountNameW(
            std::ptr::null(),
            account.as_ptr(),
            sid.as_mut_ptr().cast(),
            &mut sid_length,
            domain.as_mut_ptr(),
            &mut domain_length,
            &mut use_type,
        )
    };
    if result == 0 {
        return Err(format!(
            "resolve owner account: {}",
            io::Error::last_os_error()
        ));
    }
    sid_from_bytes(sid)
}

fn current_token_sid() -> Result<Sid, String> {
    let token = current_process_token()?;
    let bytes = token_information_bytes(&token, TokenUser)?;
    if bytes.len() < size_of::<TOKEN_USER>() {
        return Err("TokenUser buffer is too short".to_owned());
    }
    // SAFETY: successful GetTokenInformation supplied an aligned TOKEN_USER buffer.
    #[allow(unsafe_code)]
    let user = unsafe { bytes.as_ptr().cast::<TOKEN_USER>().read_unaligned() };
    if user.User.Sid.is_null() {
        return Err("TokenUser returned a null SID".to_owned());
    }
    // SAFETY: SID points into the still-live GetTokenInformation buffer; copy it before return.
    #[allow(unsafe_code)]
    let length = unsafe { GetLengthSid(user.User.Sid) } as usize;
    // SAFETY: GetLengthSid bounded the source SID allocation.
    #[allow(unsafe_code)]
    let sid = unsafe { std::slice::from_raw_parts(user.User.Sid.cast::<u8>(), length).to_vec() };
    sid_from_bytes(sid)
}

fn current_token_attestation() -> Result<TokenAttestation, String> {
    let token = current_process_token()?;
    let sid = current_token_sid()?;
    let elevated = token_elevated(&token)?;
    let (backup, restore) = token_privilege_presence(&token)?;
    let session = token_session(&token)?;
    Ok(TokenAttestation {
        owner_sid: sid.text,
        session,
        elevated,
        backup_privilege_present: backup,
        restore_privilege_present: restore,
    })
}

fn current_process_token() -> Result<OwnedHandle, String> {
    let mut raw = std::ptr::null_mut();
    // SAFETY: current process is valid and raw receives the token handle.
    #[allow(unsafe_code)]
    let result = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) };
    if result == 0 || raw.is_null() {
        return Err(format!(
            "open process token: {}",
            io::Error::last_os_error()
        ));
    }
    // SAFETY: successful OpenProcessToken transfers this owned handle.
    #[allow(unsafe_code)]
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

fn token_elevated(token: &OwnedHandle) -> Result<bool, String> {
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0_u32;
    // SAFETY: elevation has exactly the requested size and returned is writable.
    #[allow(unsafe_code)]
    let result = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    };
    if result == 0 {
        return Err(format!(
            "read TokenElevation: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(elevation.TokenIsElevated != 0)
}

fn token_session(token: &OwnedHandle) -> Result<u32, String> {
    let mut session = 0_u32;
    let mut returned = 0_u32;
    // SAFETY: session has exactly the requested size and returned is writable.
    #[allow(unsafe_code)]
    let result = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenSessionId,
            (&mut session as *mut u32).cast(),
            size_of::<u32>() as u32,
            &mut returned,
        )
    };
    if result == 0 {
        return Err(format!(
            "read TokenSessionId: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(session)
}

fn token_privilege_presence(token: &OwnedHandle) -> Result<(bool, bool), String> {
    let backup = lookup_privilege(SE_BACKUP_NAME)?;
    let restore = lookup_privilege(SE_RESTORE_NAME)?;
    let bytes = token_information_bytes(token, TokenPrivileges)?;
    if bytes.len() < size_of::<u32>() {
        return Err("TokenPrivileges buffer is too short".to_owned());
    }
    // SAFETY: successful API output has at least the privilege count DWORD.
    #[allow(unsafe_code)]
    let count = unsafe { bytes.as_ptr().cast::<u32>().read_unaligned() } as usize;
    let offset = offset_of!(TOKEN_PRIVILEGES, Privileges);
    let available = bytes.len().saturating_sub(offset) / size_of::<LUID_AND_ATTRIBUTES>();
    if count > available {
        return Err("TokenPrivileges count exceeds returned buffer".to_owned());
    }
    let mut found_backup = false;
    let mut found_restore = false;
    for index in 0..count {
        let start = offset + index * size_of::<LUID_AND_ATTRIBUTES>();
        // SAFETY: `count` was bounded by the returned buffer above.
        #[allow(unsafe_code)]
        let privilege = unsafe {
            bytes
                .as_ptr()
                .add(start)
                .cast::<LUID_AND_ATTRIBUTES>()
                .read_unaligned()
        };
        found_backup |=
            privilege.Luid.LowPart == backup.LowPart && privilege.Luid.HighPart == backup.HighPart;
        found_restore |= privilege.Luid.LowPart == restore.LowPart
            && privilege.Luid.HighPart == restore.HighPart;
    }
    Ok((found_backup, found_restore))
}

fn lookup_privilege(
    name: windows_sys::core::PCWSTR,
) -> Result<windows_sys::Win32::Foundation::LUID, String> {
    let mut luid = windows_sys::Win32::Foundation::LUID::default();
    // SAFETY: privilege name is static and LUID output is writable.
    #[allow(unsafe_code)]
    let result = unsafe { LookupPrivilegeValueW(std::ptr::null(), name, &mut luid) };
    if result == 0 {
        return Err(format!(
            "resolve privilege LUID: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(luid)
}

fn token_information_bytes(token: &OwnedHandle, class: i32) -> Result<Vec<u8>, String> {
    let mut required = 0_u32;
    // SAFETY: documented sizing call with a valid token and null output.
    #[allow(unsafe_code)]
    let first = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            class,
            std::ptr::null_mut(),
            0,
            &mut required,
        )
    };
    // SAFETY: retrieves the sizing call failure.
    #[allow(unsafe_code)]
    let error = unsafe { GetLastError() };
    if first != 0 || error != ERROR_INSUFFICIENT_BUFFER || required == 0 {
        return Err(format!(
            "size token information: {}",
            io::Error::last_os_error()
        ));
    }
    let mut bytes = vec![0_u8; required as usize];
    let mut returned = 0_u32;
    // SAFETY: bytes is exactly the requested capacity and returned is writable.
    #[allow(unsafe_code)]
    let result = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            class,
            bytes.as_mut_ptr().cast(),
            required,
            &mut returned,
        )
    };
    if result == 0 || returned as usize > bytes.len() {
        return Err(format!(
            "read token information: {}",
            io::Error::last_os_error()
        ));
    }
    bytes.truncate(returned as usize);
    Ok(bytes)
}

fn sid_from_bytes(bytes: Vec<u8>) -> Result<Sid, String> {
    let mut wide_sid = std::ptr::null_mut();
    // SAFETY: bytes contains a SID produced by Windows and wide_sid receives LocalAlloc storage.
    #[allow(unsafe_code)]
    let result = unsafe { ConvertSidToStringSidW(bytes.as_ptr().cast_mut().cast(), &mut wide_sid) };
    if result == 0 || wide_sid.is_null() {
        return Err(format!("format SID: {}", io::Error::last_os_error()));
    }
    // SAFETY: ConvertSidToStringSidW returned a NUL-terminated allocation.
    #[allow(unsafe_code)]
    let length = unsafe { (0..).take_while(|index| *wide_sid.add(*index) != 0).count() };
    // SAFETY: length scans only until the allocation's NUL terminator.
    #[allow(unsafe_code)]
    let text = unsafe { String::from_utf16_lossy(std::slice::from_raw_parts(wide_sid, length)) };
    // SAFETY: ConvertSidToStringSidW documentation requires LocalFree exactly once.
    #[allow(unsafe_code)]
    unsafe {
        LocalFree(wide_sid.cast())
    };
    Ok(Sid { bytes, text })
}

fn ensure_rail_directories(owner: &Sid, parent: &Sid) -> Result<(), String> {
    let system = lookup_account_sid("SYSTEM")?;
    let paths = [
        RAIL_ROOT.to_owned(),
        format!(r"{RAIL_ROOT}\results"),
        format!(r"{RAIL_ROOT}\logs"),
        format!(r"{RAIL_ROOT}\targets"),
    ];
    for path in &paths {
        fs::create_dir_all(path)
            .map_err(|error| format!("create rail directory {path}: {error}"))?;
        apply_rail_acl(path, &system, parent, owner)?;
    }
    Ok(())
}

fn apply_rail_acl(path: &str, system: &Sid, parent: &Sid, owner: &Sid) -> Result<(), String> {
    let sids = [&system.bytes, &parent.bytes, &owner.bytes];
    let acl_size = size_of::<ACL>()
        + sids
            .iter()
            .map(|sid| size_of::<u32>() * 2 + sid.len())
            .sum::<usize>();
    let mut storage = vec![0_u8; acl_size];
    let acl = storage.as_mut_ptr().cast::<ACL>();
    // SAFETY: storage is large enough for the ACL header and the three ACEs calculated above.
    #[allow(unsafe_code)]
    let initialized = unsafe { InitializeAcl(acl, acl_size as u32, ACL_REVISION) };
    if initialized == 0 {
        return Err(format!(
            "initialize rail ACL: {}",
            io::Error::last_os_error()
        ));
    }
    for sid in sids {
        // SAFETY: every SID comes from LookupAccountNameW/current token, and ACL capacity was bounded.
        #[allow(unsafe_code)]
        let added = unsafe {
            AddAccessAllowedAceEx(
                acl,
                ACL_REVISION,
                OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE,
                GENERIC_ALL,
                sid.as_ptr().cast_mut().cast::<core::ffi::c_void>(),
            )
        };
        if added == 0 {
            return Err(format!(
                "add rail ACL entry: {}",
                io::Error::last_os_error()
            ));
        }
    }
    let wide = wide(OsStr::new(path));
    // SAFETY: wide is NUL-terminated and ACL remains live for this synchronous call.
    #[allow(unsafe_code)]
    let result = unsafe {
        SetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            acl,
            std::ptr::null(),
        )
    };
    if result != 0 {
        return Err(format!("set rail ACL: Windows error {result}"));
    }
    Ok(())
}

fn canonical(path: &str) -> Result<String, String> {
    fs::canonicalize(path)
        .map_err(|error| format!("canonicalize {path}: {error}"))
        .map(|path| path.to_string_lossy().into_owned())
}

fn file_sha256(path: impl AsRef<Path>) -> Result<String, String> {
    let mut file = File::open(path.as_ref())
        .map_err(|error| format!("read {}: {error}", path.as_ref().display()))?;
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)
        .map_err(|error| format!("read {}: {error}", path.as_ref().display()))?;
    Ok(sha256_hex(&contents))
}

fn command_text(command: &mut Command) -> Result<String, String> {
    let output = command
        .output()
        .map_err(|error| format!("run source attestation command: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!(
            "source attestation command failed: {}",
            render_output(&output)
        ))
    }
}

fn render_output(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn now_seconds() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| format!("system clock is before Unix epoch: {error}"))
}

fn display_rail_error(error: RailError) -> String {
    error.to_string()
}

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
const ACCESS_DENIED_ACE_TYPE: u8 = 1;

struct ComScope;

impl Drop for ComScope {
    fn drop(&mut self) {
        // SAFETY: paired with a successful CoInitializeEx in init_com.
        #[allow(unsafe_code)]
        unsafe {
            CoUninitialize();
        }
    }
}

fn probe_onlogon(options: &Options) -> Result<(), String> {
    let scratch = options.required("--scratch")?;
    let exe = options.required("--exe")?;
    if !Path::new(scratch).is_dir() {
        return Err(format!("scratch directory does not exist: {scratch}"));
    }
    let exe_path = canonical(exe).unwrap_or_else(|_| exe.to_owned());
    let nonce = mint_nonce().map_err(display_rail_error)?;
    let marker = probe_marker(&nonce);
    let xml_task_name = probe_xml_task_name(&nonce);
    let com_folder_path = probe_com_folder_path(&nonce);
    let (mut token, gate) = match current_probe_token() {
        Ok(token) => token,
        Err(error) => {
            let mut report = skipped_probe_report(
                &nonce,
                &marker,
                &xml_task_name,
                &com_folder_path,
                ProbeTokenSection {
                    elevation_type: TokenElevationClass::Unknown,
                    elevated: false,
                    session: 0,
                    owner_sid: String::new(),
                    stage: ProbeStage::failed(
                        win32_probe_error(0, &error),
                        ProbeResolutionAction::RepairSchedulerAccess,
                    ),
                },
            );
            report.outcome = select_probe_outcome(&report);
            write_probe_report(&report, scratch)?;
            return Err(format!("onlogon probe outcome={}", report.outcome));
        }
    };
    let mut report = skipped_probe_report(
        &nonce,
        &marker,
        &xml_task_name,
        &com_folder_path,
        token.clone(),
    );
    match gate {
        Ok(ProbeTokenGate::ContinueUnelevated) => {
            token.stage = ProbeStage::passed();
            report.token = token.clone();
            report.xml = run_xml_onlogon_route(
                scratch,
                &nonce,
                &xml_task_name,
                &exe_path,
                &marker,
                &token.owner_sid,
            );
            report.com =
                run_com_onlogon_route(&com_folder_path, &exe_path, &marker, &token.owner_sid);
        }
        Ok(ProbeTokenGate::StopElevated) => {
            let diagnostic = if token.elevation_type == TokenElevationClass::Full {
                "token-elevation-type-full"
            } else {
                "token-elevation-type-unknown"
            };
            token.stage = ProbeStage::inconclusive(
                win32_probe_error(0, diagnostic),
                ProbeResolutionAction::UseStandardLogon,
            );
            report.token = token;
        }
        Ok(ProbeTokenGate::StopFilteredAdmin) => {
            token.stage = ProbeStage::inconclusive(
                win32_probe_error(0, "token-elevation-type-limited"),
                ProbeResolutionAction::StopFilteredAdmin,
            );
            report.token = token;
        }
        Err(error) => {
            if token.stage.status != ProbeStageStatus::Failed {
                token.stage = ProbeStage::failed(
                    win32_probe_error(0, &display_rail_error(error)),
                    ProbeResolutionAction::RepairSchedulerAccess,
                );
            }
            report.token = token;
            report.outcome = select_probe_outcome(&report);
            write_probe_report(&report, scratch)?;
            return Err(format!("onlogon probe outcome={}", report.outcome));
        }
    }
    report.outcome = select_probe_outcome(&report);
    write_probe_report(&report, scratch)?;
    if probe_exits_successfully(&report) {
        Ok(())
    } else {
        Err(format!("onlogon probe outcome={}", report.outcome))
    }
}

fn skipped_probe_report(
    nonce: &str,
    marker: &str,
    xml_task_name: &str,
    com_folder_path: &str,
    token: ProbeTokenSection,
) -> OnlogonProbeReport {
    OnlogonProbeReport {
        schema: PROBE_SCHEMA.to_owned(),
        outcome: ProbeOutcome::DefinitionUnverified,
        nonce: nonce.to_owned(),
        marker: marker.to_owned(),
        xml_task_name: xml_task_name.to_owned(),
        com_folder_path: com_folder_path.to_owned(),
        token,
        xml: ProbeXmlSection::skipped(),
        com: ProbeComSection::skipped(),
    }
}

fn current_probe_token() -> Result<(ProbeTokenSection, Result<ProbeTokenGate, RailError>), String> {
    let token = current_process_token()?;
    let sid = current_token_sid()?;
    let elevated = token_elevated(&token)?;
    let session = token_session(&token)?;
    let class = match token_elevation_type(&token) {
        Ok(value) => classify_token_elevation_type(value),
        Err(error) => {
            return Ok((
                ProbeTokenSection {
                    elevation_type: TokenElevationClass::Unknown,
                    elevated,
                    session,
                    owner_sid: sid.text,
                    stage: ProbeStage::failed(
                        win32_probe_error(0, &error),
                        ProbeResolutionAction::RepairSchedulerAccess,
                    ),
                },
                Err(RailError::new("token-elevation-inconsistent")),
            ));
        }
    };
    Ok((
        ProbeTokenSection {
            elevation_type: class,
            elevated,
            session,
            owner_sid: sid.text,
            stage: ProbeStage::skipped(),
        },
        probe_token_gate(class, elevated),
    ))
}

fn token_elevation_type(token: &OwnedHandle) -> Result<i32, String> {
    let mut elevation_type = 0_i32;
    let mut returned = 0_u32;
    // SAFETY: elevation_type has the TokenElevationType size and returned is writable.
    #[allow(unsafe_code)]
    let result = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenElevationType,
            (&mut elevation_type as *mut i32).cast(),
            size_of::<i32>() as u32,
            &mut returned,
        )
    };
    if result == 0 {
        return Err(format!(
            "read TokenElevationType: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(elevation_type)
}

fn run_xml_onlogon_route(
    scratch: &str,
    nonce: &str,
    task_name: &str,
    exe_path: &str,
    marker: &str,
    owner_sid: &str,
) -> ProbeXmlSection {
    let mut section = ProbeXmlSection::skipped();
    let xml_path = Path::new(scratch).join(format!("onlogon-xml-{nonce}.xml"));
    let xml_path_text = xml_path.to_string_lossy().into_owned();
    let definition = solstone_core_win_owner_rail::ProbeTaskDefinition {
        principal_sid: owner_sid.to_owned(),
        logon_type: "InteractiveToken".to_owned(),
        run_level: solstone_core_win_owner_rail::TaskRunLevel::ExplicitLeastPrivilege,
        command: exe_path.to_owned(),
        arguments: marker.to_owned(),
        working_directory: None,
        description: Some(marker.to_owned()),
        trigger_user_id: owner_sid.to_owned(),
        priority: ProbePriority::Unavailable,
    };
    let xml = render_probe_task_xml(&definition);
    if let Err(error) = write_probe_xml_file(&xml_path_text, xml.as_bytes()) {
        section.create = ProbeStage::failed(
            win32_probe_error(0, &error),
            ProbeResolutionAction::RepairSchedulerAccess,
        );
        return section;
    }
    match run_schtasks(schtasks_create_xml_argv(task_name, &xml_path_text)) {
        Ok(()) => section.create = ProbeStage::passed(),
        Err(error) => {
            let action = scheduler_create_resolution(&error);
            section.create = if action == ProbeResolutionAction::RepairSchedulerAccess {
                ProbeStage::inconclusive(schtasks_probe_error(&error), action)
            } else {
                ProbeStage::failed(schtasks_probe_error(&error), action)
            };
            return section;
        }
    }
    match query_probe_xml(task_name) {
        Ok(xml) => match bind_probe_xml(&xml, owner_sid, exe_path, marker) {
            Ok(parsed) => {
                section.priority = parsed.priority.into();
                section.definition = ProbeStage::passed();
            }
            Err(error) => {
                section.definition = ProbeStage::failed(
                    schtasks_probe_error(&error),
                    ProbeResolutionAction::InspectDefinition,
                );
            }
        },
        Err(error) => {
            section.definition = ProbeStage::inconclusive(
                schtasks_probe_error(&error),
                ProbeResolutionAction::InspectDefinition,
            );
        }
    }
    section.cleanup = cleanup_xml_task(task_name);
    section
}

fn run_com_onlogon_route(
    com_folder_path: &str,
    exe_path: &str,
    marker: &str,
    owner_sid: &str,
) -> ProbeComSection {
    let mut section = ProbeComSection::skipped();
    let com = match init_com() {
        Ok(com) => com,
        Err(error) => {
            section.folder_create = ProbeStage::inconclusive(
                hresult_probe_error(0, &error),
                ProbeResolutionAction::RepairSchedulerAccess,
            );
            return section;
        }
    };
    let service = match create_task_service() {
        Ok(service) => service,
        Err(error) => {
            section.folder_create =
                ProbeStage::inconclusive(error, ProbeResolutionAction::RepairSchedulerAccess);
            return section;
        }
    };
    let parent = match ensure_com_parent_folders(&service) {
        Ok(parent) => parent,
        Err(stage) => {
            section.folder_create = stage;
            return section;
        }
    };
    let leaf_name = com_folder_path
        .rsplit('\\')
        .next()
        .unwrap_or(com_folder_path);
    let leaf = match create_com_leaf_folder(&parent, leaf_name) {
        Ok(leaf) => {
            section.folder_create = ProbeStage::passed();
            leaf
        }
        Err(stage) => {
            section.folder_create = stage;
            return section;
        }
    };
    section.folder_acl = read_folder_acl(com_folder_path, &leaf, owner_sid);
    match register_com_onlogon_task(&service, &leaf, exe_path, marker, owner_sid) {
        Ok(xml) => {
            section.register = ProbeStage::passed();
            match bind_probe_xml(&xml, owner_sid, exe_path, marker) {
                Ok(parsed) => {
                    section.priority = parsed.priority.into();
                    section.definition = ProbeStage::passed();
                }
                Err(error) => {
                    section.definition = ProbeStage::failed(
                        hresult_probe_error(0, &error),
                        ProbeResolutionAction::InspectDefinition,
                    );
                }
            }
        }
        Err(stage) => section.register = stage,
    }
    section.cleanup = cleanup_com_leaf(&parent, &leaf, leaf_name);
    drop(com);
    section
}

fn init_com() -> Result<ComScope, String> {
    // SAFETY: first COM init for this thread; S_FALSE is still a successful init.
    #[allow(unsafe_code)]
    let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if result.is_err() {
        return Err(format!("CoInitializeEx failed: {result:?}"));
    }
    Ok(ComScope)
}

fn create_task_service() -> Result<ITaskService, ProbeApiError> {
    // SAFETY: Task Scheduler CLSID/IID are the generated 2.0 bindings; no outer unknown.
    #[allow(unsafe_code)]
    let service: ITaskService = unsafe {
        CoCreateInstance(
            &TaskScheduler,
            None::<&windows::core::IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
    }
    .map_err(|error| com_probe_error(&error, "CoCreateInstance ITaskService"))?;
    let empty = VARIANT::default();
    // SAFETY: empty VARIANTs are the documented local-connect arguments.
    #[allow(unsafe_code)]
    unsafe {
        service
            .Connect(&empty, &empty, &empty, &empty)
            .map_err(|error| com_probe_error(&error, "ITaskService::Connect"))?;
    }
    Ok(service)
}

fn ensure_com_parent_folders(service: &ITaskService) -> Result<ITaskFolder, ProbeStage> {
    // SAFETY: connected ITaskService; "\\" is the scheduler root.
    #[allow(unsafe_code)]
    let root = unsafe { service.GetFolder(&BSTR::from("\\")) }.map_err(|error| {
        com_stage(
            &error,
            "ITaskService::GetFolder root",
            ProbeResolutionAction::RepairSchedulerAccess,
            true,
        )
    })?;
    let solstone = get_or_create_folder(&root, "solstone")?;
    get_or_create_folder(&solstone, "probe")
}

fn get_or_create_folder(parent: &ITaskFolder, name: &str) -> Result<ITaskFolder, ProbeStage> {
    let bname = BSTR::from(name);
    // SAFETY: parent is a live ITaskFolder; name is a NUL-terminated BSTR.
    #[allow(unsafe_code)]
    match unsafe { parent.GetFolder(&bname) } {
        Ok(folder) => Ok(folder),
        Err(_) => {
            let empty = VARIANT::default();
            // SAFETY: documented CreateFolder with an empty SDDL VARIANT.
            #[allow(unsafe_code)]
            unsafe { parent.CreateFolder(&bname, &empty) }
                .map_err(|error| com_create_stage(&error, &format!("CreateFolder {name}")))
        }
    }
}

fn create_com_leaf_folder(parent: &ITaskFolder, name: &str) -> Result<ITaskFolder, ProbeStage> {
    let bname = BSTR::from(name);
    // SAFETY: parent is the probe folder we own; collision must stay fail-closed.
    #[allow(unsafe_code)]
    if unsafe { parent.GetFolder(&bname) }.is_ok() {
        return Err(ProbeStage::failed(
            win32_probe_error(183, "onlogon COM folder already exists"),
            ProbeResolutionAction::RetryFreshNonce,
        ));
    }
    let empty = VARIANT::default();
    // SAFETY: empty SDDL VARIANT requests the scheduler default descriptor.
    #[allow(unsafe_code)]
    unsafe { parent.CreateFolder(&bname, &empty) }
        .map_err(|error| com_create_stage(&error, "CreateFolder leaf"))
}

fn register_com_onlogon_task(
    service: &ITaskService,
    folder: &ITaskFolder,
    exe_path: &str,
    marker: &str,
    owner_sid: &str,
) -> Result<String, ProbeStage> {
    // SAFETY: connected service; flags 0 is the documented NewTask reserved value.
    #[allow(unsafe_code)]
    let definition = unsafe { service.NewTask(0) }
        .map_err(|error| com_create_stage(&error, "ITaskService::NewTask"))?;
    // SAFETY: NewTask returned a live definition; each call is a documented Task
    // Scheduler 2.0 vtable invocation on that object.
    #[allow(unsafe_code)]
    unsafe {
        let info = definition
            .RegistrationInfo()
            .map_err(|error| com_create_stage(&error, "RegistrationInfo"))?;
        info.SetDescription(&BSTR::from(marker))
            .map_err(|error| com_create_stage(&error, "SetDescription"))?;
        let principal = definition
            .Principal()
            .map_err(|error| com_create_stage(&error, "Principal"))?;
        principal
            .SetUserId(&BSTR::from(owner_sid))
            .map_err(|error| com_create_stage(&error, "SetUserId"))?;
        principal
            .SetLogonType(TASK_LOGON_INTERACTIVE_TOKEN)
            .map_err(|error| com_create_stage(&error, "SetLogonType"))?;
        principal
            .SetRunLevel(TASK_RUNLEVEL_LUA)
            .map_err(|error| com_create_stage(&error, "SetRunLevel"))?;
        let triggers = definition
            .Triggers()
            .map_err(|error| com_create_stage(&error, "Triggers"))?;
        triggers
            .Clear()
            .map_err(|error| com_create_stage(&error, "Triggers::Clear"))?;
        let trigger = triggers
            .Create(TASK_TRIGGER_LOGON)
            .map_err(|error| com_create_stage(&error, "Create LogonTrigger"))?;
        let logon: ILogonTrigger = trigger
            .cast()
            .map_err(|error| com_create_stage(&error, "cast ILogonTrigger"))?;
        logon
            .SetUserId(&BSTR::from(owner_sid))
            .map_err(|error| com_create_stage(&error, "LogonTrigger::SetUserId"))?;
        let actions = definition
            .Actions()
            .map_err(|error| com_create_stage(&error, "Actions"))?;
        actions
            .Clear()
            .map_err(|error| com_create_stage(&error, "Actions::Clear"))?;
        let action = actions
            .Create(TASK_ACTION_EXEC)
            .map_err(|error| com_create_stage(&error, "Create Exec"))?;
        let exec: IExecAction = action
            .cast()
            .map_err(|error| com_create_stage(&error, "cast IExecAction"))?;
        exec.SetPath(&BSTR::from(exe_path))
            .map_err(|error| com_create_stage(&error, "IExecAction::SetPath"))?;
        exec.SetArguments(&BSTR::from(marker))
            .map_err(|error| com_create_stage(&error, "IExecAction::SetArguments"))?;
    }
    let empty = VARIANT::default();
    // SAFETY: TASK_CREATE refuses overwrite; empty user/password use the caller token.
    #[allow(unsafe_code)]
    let registered = unsafe {
        folder.RegisterTaskDefinition(
            &BSTR::from(probe_com_task_name()),
            &definition,
            TASK_CREATE.0,
            &empty,
            &empty,
            TASK_LOGON_INTERACTIVE_TOKEN,
            &empty,
        )
    }
    .map_err(|error| com_create_stage(&error, "RegisterTaskDefinition"))?;
    // SAFETY: registered task is live and Xml is the scheduler-owned definition.
    #[allow(unsafe_code)]
    let xml = unsafe { registered.Xml() }.map_err(|error| {
        com_stage(
            &error,
            "IRegisteredTask::Xml",
            ProbeResolutionAction::InspectDefinition,
            true,
        )
    })?;
    Ok(xml.to_string())
}

fn cleanup_com_leaf(parent: &ITaskFolder, leaf: &ITaskFolder, leaf_name: &str) -> ProbeStage {
    let task = BSTR::from(probe_com_task_name());
    // SAFETY: leaf is the per-invocation folder this probe created.
    #[allow(unsafe_code)]
    let _ = unsafe { leaf.DeleteTask(&task, 0) };
    let bname = BSTR::from(leaf_name);
    // SAFETY: parent is \solstone\probe; only the nonce leaf is deleted.
    #[allow(unsafe_code)]
    if let Err(error) = unsafe { parent.DeleteFolder(&bname, 0) } {
        return com_stage(
            &error,
            "DeleteFolder leaf",
            ProbeResolutionAction::ManualCleanup,
            true,
        );
    }
    // SAFETY: absence of the leaf folder is proof of cleanup.
    #[allow(unsafe_code)]
    match unsafe { parent.GetFolder(&bname) } {
        Ok(_) => ProbeStage::inconclusive(
            win32_probe_error(0, "COM leaf folder still exists after delete"),
            ProbeResolutionAction::ManualCleanup,
        ),
        Err(_) => ProbeStage::passed(),
    }
}

fn read_folder_acl(
    com_folder_path: &str,
    folder: &ITaskFolder,
    caller_sid: &str,
) -> ProbeFolderAclStage {
    let fs_path = tasks_folder_fs_path(com_folder_path);
    if let Ok((entries, sddl)) = read_named_folder_acl(&fs_path, caller_sid) {
        return folder_acl_from_entries(entries, sddl, ProbeDescriptorSource::NamedInfo);
    }
    match read_com_folder_acl(folder, caller_sid) {
        Ok((entries, sddl)) => {
            folder_acl_from_entries(entries, sddl, ProbeDescriptorSource::ComSddl)
        }
        Err(error) => ProbeFolderAclStage {
            status: ProbeStageStatus::Inconclusive,
            error: Some(error),
            resolution_action: Some(ProbeResolutionAction::RepairSchedulerAccess),
            descriptor_source: None,
            sddl: None,
            ordinary_mutation: None,
        },
    }
}

fn folder_acl_from_entries(
    entries: Vec<solstone_core_win_owner_rail::ProbeAclEntry>,
    sddl: String,
    source: ProbeDescriptorSource,
) -> ProbeFolderAclStage {
    let ordinary = folder_allows_ordinary_mutation(&entries);
    if ordinary {
        ProbeFolderAclStage {
            status: ProbeStageStatus::Failed,
            error: Some(win32_probe_error(
                0,
                "ordinary mutation rights on COM folder",
            )),
            resolution_action: Some(ProbeResolutionAction::IsolateFolderAcl),
            descriptor_source: Some(source),
            sddl: Some(sddl),
            ordinary_mutation: Some(true),
        }
    } else {
        ProbeFolderAclStage {
            status: ProbeStageStatus::Passed,
            error: None,
            resolution_action: None,
            descriptor_source: Some(source),
            sddl: Some(sddl),
            ordinary_mutation: Some(false),
        }
    }
}

fn tasks_folder_fs_path(com_folder_path: &str) -> String {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
    let relative = com_folder_path.trim_start_matches('\\');
    format!(r"{root}\System32\Tasks\{relative}")
}

fn read_named_folder_acl(
    path: &str,
    caller_sid: &str,
) -> Result<(Vec<solstone_core_win_owner_rail::ProbeAclEntry>, String), ProbeApiError> {
    let wide = wide(OsStr::new(path));
    let mut owner = std::ptr::null_mut();
    let mut group = std::ptr::null_mut();
    let mut dacl = std::ptr::null_mut();
    let mut sacl = std::ptr::null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: wide is NUL-terminated; output pointers receive LocalAlloc storage.
    #[allow(unsafe_code)]
    let result = unsafe {
        GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            &mut owner,
            &mut group,
            &mut dacl,
            &mut sacl,
            &mut sd,
        )
    };
    if result != 0 || sd.is_null() {
        return Err(win32_probe_error(
            result,
            &format!("GetNamedSecurityInfoW {path}"),
        ));
    }
    let sddl = security_descriptor_sddl(sd)?;
    let entries = walk_dacl_entries(dacl, caller_sid)?;
    // SAFETY: GetNamedSecurityInfoW requires LocalFree of the returned descriptor.
    #[allow(unsafe_code)]
    unsafe {
        LocalFree(sd.cast());
    }
    Ok((entries, sddl))
}

fn read_com_folder_acl(
    folder: &ITaskFolder,
    caller_sid: &str,
) -> Result<(Vec<solstone_core_win_owner_rail::ProbeAclEntry>, String), ProbeApiError> {
    // SAFETY: folder is the live per-invocation ITaskFolder.
    #[allow(unsafe_code)]
    let sddl = unsafe { folder.GetSecurityDescriptor(DACL_SECURITY_INFORMATION as i32) }
        .map_err(|error| com_probe_error(&error, "ITaskFolder::GetSecurityDescriptor"))?;
    let sddl_text = sddl.to_string();
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let wide = wide(OsStr::new(&sddl_text));
    // SAFETY: SDDL is NUL-terminated; sd receives LocalAlloc storage.
    #[allow(unsafe_code)]
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            std::ptr::null_mut(),
        )
    };
    if converted == 0 || sd.is_null() {
        return Err(win32_probe_error(
            unsafe { GetLastError() },
            "ConvertStringSecurityDescriptorToSecurityDescriptorW",
        ));
    }
    let mut present = 0_i32;
    let mut defaulted = 0_i32;
    let mut dacl = std::ptr::null_mut();
    // SAFETY: sd is a valid self-relative descriptor from the convert call.
    #[allow(unsafe_code)]
    let got = unsafe { GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted) };
    if got == 0 {
        // SAFETY: convert allocated sd.
        #[allow(unsafe_code)]
        unsafe {
            LocalFree(sd.cast());
        }
        return Err(win32_probe_error(
            unsafe { GetLastError() },
            "GetSecurityDescriptorDacl",
        ));
    }
    let dacl = if present == 0 {
        std::ptr::null_mut()
    } else {
        dacl
    };
    let entries = walk_dacl_entries(dacl, caller_sid);
    // SAFETY: convert allocated sd.
    #[allow(unsafe_code)]
    unsafe {
        LocalFree(sd.cast());
    }
    Ok((entries?, sddl_text))
}

fn security_descriptor_sddl(sd: PSECURITY_DESCRIPTOR) -> Result<String, ProbeApiError> {
    let mut wide_sddl = std::ptr::null_mut();
    // SAFETY: sd is live; output is a LocalAlloc string.
    #[allow(unsafe_code)]
    let result = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            sd,
            SDDL_REVISION_1,
            DACL_SECURITY_INFORMATION,
            &mut wide_sddl,
            std::ptr::null_mut(),
        )
    };
    if result == 0 || wide_sddl.is_null() {
        return Err(win32_probe_error(
            unsafe { GetLastError() },
            "ConvertSecurityDescriptorToStringSecurityDescriptorW",
        ));
    }
    // SAFETY: convert returned a NUL-terminated allocation.
    #[allow(unsafe_code)]
    let length = unsafe {
        (0..)
            .take_while(|index| *wide_sddl.add(*index) != 0)
            .count()
    };
    // SAFETY: length stops at the terminator.
    #[allow(unsafe_code)]
    let text = unsafe { String::from_utf16_lossy(std::slice::from_raw_parts(wide_sddl, length)) };
    // SAFETY: LocalFree once for the convert allocation.
    #[allow(unsafe_code)]
    unsafe {
        LocalFree(wide_sddl.cast());
    }
    Ok(text)
}

fn walk_dacl_entries(
    acl: *mut ACL,
    caller_sid: &str,
) -> Result<Vec<solstone_core_win_owner_rail::ProbeAclEntry>, ProbeApiError> {
    if acl.is_null() {
        return Ok(vec![solstone_core_win_owner_rail::ProbeAclEntry {
            sid: "S-1-1-0".to_owned(),
            allow: true,
            access_mask: ACE_MUTATION_MASK,
            classification: classify_probe_sid("S-1-1-0", caller_sid),
        }]);
    }
    // SAFETY: ACL header is the documented prefix of a valid ACL.
    #[allow(unsafe_code)]
    let count = unsafe { (*acl).AceCount } as u32;
    let mut entries = Vec::new();
    for index in 0..count {
        let mut ace = std::ptr::null_mut();
        // SAFETY: index is bounded by AceCount.
        #[allow(unsafe_code)]
        if unsafe { GetAce(acl, index, &mut ace) } == 0 || ace.is_null() {
            return Err(win32_probe_error(unsafe { GetLastError() }, "GetAce"));
        }
        // SAFETY: GetAce returned an ACE whose header is at the pointer.
        #[allow(unsafe_code)]
        let header = unsafe { ace.cast::<ACE_HEADER>().read_unaligned() };
        if u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0 {
            continue;
        }
        let allow = match header.AceType {
            ACCESS_ALLOWED_ACE_TYPE => true,
            ACCESS_DENIED_ACE_TYPE => false,
            _ => continue,
        };
        let allowed = ace.cast::<ACCESS_ALLOWED_ACE>();
        // SAFETY: allow/deny ACE layout starts with ACCESS_ALLOWED_ACE.
        #[allow(unsafe_code)]
        let mask = unsafe { (*allowed).Mask };
        // SAFETY: SidStart is the first DWORD of the in-place SID.
        #[allow(unsafe_code)]
        let sid_ptr = unsafe { std::ptr::addr_of_mut!((*allowed).SidStart) }.cast();
        // SAFETY: GetLengthSid bounds the in-place SID.
        #[allow(unsafe_code)]
        let length = unsafe { GetLengthSid(sid_ptr) } as usize;
        // SAFETY: length bytes starting at SidStart are the SID.
        #[allow(unsafe_code)]
        let bytes = unsafe { std::slice::from_raw_parts(sid_ptr.cast::<u8>(), length).to_vec() };
        let sid = sid_from_bytes(bytes).map_err(|error| win32_probe_error(0, &error))?;
        let _ = lookup_sid_account(&sid.bytes);
        entries.push(solstone_core_win_owner_rail::ProbeAclEntry {
            classification: classify_probe_sid(&sid.text, caller_sid),
            sid: sid.text,
            allow,
            access_mask: mask,
        });
    }
    Ok(solstone_core_win_owner_rail::normalize_probe_acl_entries(
        &entries,
    ))
}

fn lookup_sid_account(bytes: &[u8]) -> Result<(), ()> {
    let mut name_length = 0_u32;
    let mut domain_length = 0_u32;
    let mut use_type = 0_i32;
    // SAFETY: documented sizing call with a live SID and null output buffers.
    #[allow(unsafe_code)]
    let first = unsafe {
        LookupAccountSidW(
            std::ptr::null(),
            bytes.as_ptr().cast_mut().cast(),
            std::ptr::null_mut(),
            &mut name_length,
            std::ptr::null_mut(),
            &mut domain_length,
            &mut use_type,
        )
    };
    // SAFETY: retrieves the sizing-call failure.
    #[allow(unsafe_code)]
    let error = unsafe { GetLastError() };
    if first != 0 || error != ERROR_INSUFFICIENT_BUFFER || name_length == 0 {
        return Err(());
    }
    let mut name = vec![0_u16; name_length as usize + 1];
    let mut domain = vec![0_u16; domain_length as usize + 1];
    // SAFETY: buffers are sized by the previous LookupAccountSidW call.
    #[allow(unsafe_code)]
    let result = unsafe {
        LookupAccountSidW(
            std::ptr::null(),
            bytes.as_ptr().cast_mut().cast(),
            name.as_mut_ptr(),
            &mut name_length,
            domain.as_mut_ptr(),
            &mut domain_length,
            &mut use_type,
        )
    };
    if result == 0 { Err(()) } else { Ok(()) }
}

fn write_probe_xml_file(path: &str, xml: &[u8]) -> Result<(), String> {
    let mut file = create_new(path)?;
    file.write_all(xml)
        .map_err(|error| format!("write probe xml: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("sync probe xml: {error}"))
}

fn query_probe_xml(task_name: &str) -> Result<String, String> {
    let output = Command::new("schtasks.exe")
        .args(schtasks_query_xml_argv(task_name))
        .output()
        .map_err(|error| format!("query probe XML: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "query probe XML failed: {}",
            render_output(&output)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn bind_probe_xml(
    xml: &str,
    owner_sid: &str,
    exe_path: &str,
    marker: &str,
) -> Result<solstone_core_win_owner_rail::ProbeTaskDefinition, String> {
    let mut definition = parse_probe_task_xml(xml).map_err(display_rail_error)?;
    if !definition.principal_sid.starts_with("S-") {
        definition.principal_sid = lookup_account_sid(&definition.principal_sid)?.text;
    }
    if !definition.trigger_user_id.starts_with("S-") {
        definition.trigger_user_id = lookup_account_sid(&definition.trigger_user_id)?.text;
    }
    verify_probe_task_definition(
        &definition,
        &ProbeVerifyInput {
            expected_sid: owner_sid.to_owned(),
            expected_command: exe_path.to_owned(),
            marker: marker.to_owned(),
        },
    )
    .map_err(display_rail_error)?;
    Ok(definition)
}

fn cleanup_xml_task(task_name: &str) -> ProbeStage {
    let delete = run_schtasks(schtasks_delete_argv(task_name));
    match confirm_task_name_absent(task_name) {
        Ok(()) => ProbeStage::passed(),
        Err(absence) => match delete {
            Ok(()) => ProbeStage::inconclusive(
                schtasks_probe_error(&absence),
                ProbeResolutionAction::ManualCleanup,
            ),
            Err(delete) => ProbeStage::inconclusive(
                schtasks_probe_error(&format!("{delete}; {absence}")),
                ProbeResolutionAction::ManualCleanup,
            ),
        },
    }
}

fn confirm_task_name_absent(task_name: &str) -> Result<(), String> {
    let output = Command::new("schtasks.exe")
        .args(schtasks_query_xml_argv(task_name))
        .output()
        .map_err(|error| format!("query exact task absence: {error}"))?;
    if output.status.success() {
        return Err(format!("exact probe task still exists {task_name}"));
    }
    if schtasks_query_proves_task_absent(&render_output(&output)) {
        return Ok(());
    }
    Err(format!(
        "exact task absence query was not a task-not-found result {task_name}"
    ))
}

fn write_probe_report(report: &OnlogonProbeReport, scratch: &str) -> Result<(), String> {
    let json = serde_json::to_string_pretty(report)
        .map_err(|error| format!("serialize onlogon probe report: {error}"))?;
    println!("{json}");
    let path = Path::new(scratch).join(format!("onlogon-probe-{}.json", report.nonce));
    let mut file = create_new(&path.to_string_lossy())?;
    file.write_all(json.as_bytes())
        .map_err(|error| format!("write onlogon probe report: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("sync onlogon probe report: {error}"))
}

fn scheduler_create_resolution(error: &str) -> ProbeResolutionAction {
    let lower = error.to_ascii_lowercase();
    if lower.contains("access is denied") || lower.contains("access denied") {
        ProbeResolutionAction::TreatAsRefused
    } else if lower.contains("already exists") {
        ProbeResolutionAction::RetryFreshNonce
    } else {
        ProbeResolutionAction::RepairSchedulerAccess
    }
}

fn com_create_stage(error: &windows::core::Error, diagnostic: &str) -> ProbeStage {
    let api = com_probe_error(error, diagnostic);
    let action = hresult_create_resolution(api.code, diagnostic);
    if action == ProbeResolutionAction::RepairSchedulerAccess {
        ProbeStage::inconclusive(api, action)
    } else {
        ProbeStage::failed(api, action)
    }
}

fn com_stage(
    error: &windows::core::Error,
    diagnostic: &str,
    action: ProbeResolutionAction,
    inconclusive: bool,
) -> ProbeStage {
    let api = com_probe_error(error, diagnostic);
    if inconclusive {
        ProbeStage::inconclusive(api, action)
    } else {
        ProbeStage::failed(api, action)
    }
}

fn hresult_create_resolution(code: u32, diagnostic: &str) -> ProbeResolutionAction {
    if code == 0x8007_0005 || diagnostic.to_ascii_lowercase().contains("access denied") {
        ProbeResolutionAction::TreatAsRefused
    } else if code == 0x8007_00B7 || diagnostic.to_ascii_lowercase().contains("already exists") {
        ProbeResolutionAction::RetryFreshNonce
    } else {
        ProbeResolutionAction::RepairSchedulerAccess
    }
}

fn win32_probe_error(code: u32, diagnostic: &str) -> ProbeApiError {
    ProbeApiError {
        space: ProbeApiErrorSpace::Win32,
        code,
        diagnostic: diagnostic.to_owned(),
    }
}

fn schtasks_probe_error(diagnostic: &str) -> ProbeApiError {
    ProbeApiError {
        space: ProbeApiErrorSpace::Schtasks,
        code: 1,
        diagnostic: diagnostic.to_owned(),
    }
}

fn hresult_probe_error(code: u32, diagnostic: &str) -> ProbeApiError {
    ProbeApiError {
        space: ProbeApiErrorSpace::Hresult,
        code,
        diagnostic: diagnostic.to_owned(),
    }
}

fn com_probe_error(error: &windows::core::Error, diagnostic: &str) -> ProbeApiError {
    ProbeApiError {
        space: ProbeApiErrorSpace::Hresult,
        code: error.code().0 as u32,
        diagnostic: format!("{diagnostic}: {error}"),
    }
}
