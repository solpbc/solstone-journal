// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Portable contracts for the Windows ordinary-owner scheduled-task rail.

use std::fmt;

use getrandom::fill as fill_random;
use quick_xml::Reader;
use quick_xml::events::Event;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const CONTROL_ID: &str = "windows-ordinary-owner-inventory.v1";
pub const NONCE_LENGTH: usize = 32;
pub const NONCE_ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";
pub const RAIL_ROOT: &str = r"C:\ProgramData\solstone\journal-win-owner-rail";
pub const LEASE_PATH: &str =
    r"C:\ProgramData\solstone\journal-win-owner-rail\ordinary-owner.lease.json";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LeaseState {
    Active,
    Launched,
    TerminalVerified,
    Cleaned,
    Held,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LeaseRecord {
    pub schema: String,
    pub nonce: String,
    pub control_id: String,
    pub state: LeaseState,
    pub expected_commit: String,
    pub expected_cargo_lock_sha256: String,
    pub expected_owner_account: String,
    pub expected_owner_sid: String,
    pub expected_session: u32,
    pub worktree: String,
    pub worker_path: String,
    pub worker_sha256: String,
    pub task_name: String,
    pub task_command: String,
    pub payload_sha256: String,
    pub result_path: String,
    pub output_path: String,
    pub target_dir: String,
    pub refs_root: String,
    pub created_at_unix_seconds: u64,
    pub launch_boundary_unix_seconds: Option<u64>,
    pub launch_observed_last_run_time: Option<String>,
    pub last_error: Option<String>,
}

impl LeaseRecord {
    pub fn new(input: LeaseInput) -> Self {
        let task_name = task_name(&input.nonce);
        let result_path = format!(r"{RAIL_ROOT}\results\{}.json", input.nonce);
        let output_path = format!(r"{RAIL_ROOT}\logs\{}.log", input.nonce);
        let target_dir = format!(r"{RAIL_ROOT}\targets\{}", input.nonce);
        let task_command = task_command_line(&input.worker_path, &input.lease_path, &input.nonce);
        let payload_sha256 = payload_sha256(&[
            &input.nonce,
            CONTROL_ID,
            &input.expected_commit,
            &input.expected_cargo_lock_sha256,
            &input.expected_owner_sid,
            &input.worktree,
            &input.worker_sha256,
            &task_name,
            &task_command,
            &result_path,
            &output_path,
            &target_dir,
            &input.refs_root,
        ]);
        Self {
            schema: "solstone.journal.win-owner-rail.lease.v1".to_owned(),
            nonce: input.nonce,
            control_id: CONTROL_ID.to_owned(),
            state: LeaseState::Active,
            expected_commit: input.expected_commit,
            expected_cargo_lock_sha256: input.expected_cargo_lock_sha256,
            expected_owner_account: input.expected_owner_account,
            expected_owner_sid: input.expected_owner_sid,
            expected_session: input.expected_session,
            worktree: input.worktree,
            worker_path: input.worker_path,
            worker_sha256: input.worker_sha256,
            task_name,
            task_command,
            payload_sha256,
            result_path,
            output_path,
            target_dir,
            refs_root: input.refs_root,
            created_at_unix_seconds: input.created_at_unix_seconds,
            launch_boundary_unix_seconds: None,
            launch_observed_last_run_time: None,
            last_error: None,
        }
    }

    pub fn transition(&mut self, next: LeaseState) -> Result<(), RailError> {
        let allowed = matches!(
            (&self.state, &next),
            (LeaseState::Active, LeaseState::Launched)
                | (LeaseState::Launched, LeaseState::TerminalVerified)
                | (LeaseState::Active | LeaseState::Launched, LeaseState::Held)
                | (LeaseState::Held, LeaseState::Cleaned)
                | (LeaseState::TerminalVerified, LeaseState::Cleaned)
        );
        if !allowed {
            return Err(RailError::new("invalid-lease-transition"));
        }
        self.state = next;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LeaseInput {
    pub nonce: String,
    pub lease_path: String,
    pub expected_commit: String,
    pub expected_cargo_lock_sha256: String,
    pub expected_owner_account: String,
    pub expected_owner_sid: String,
    pub expected_session: u32,
    pub worktree: String,
    pub worker_path: String,
    pub worker_sha256: String,
    pub refs_root: String,
    pub created_at_unix_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TokenAttestation {
    pub owner_sid: String,
    pub session: u32,
    pub elevated: bool,
    pub backup_privilege_present: bool,
    pub restore_privilege_present: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResultRecord {
    pub schema: String,
    pub nonce: String,
    pub selected_control: Option<String>,
    pub executed_control: Option<String>,
    pub payload_sha256: String,
    pub passed: bool,
    pub cargo_exit_code: i32,
    pub ordinary_owner_marker: bool,
    pub ordinary_owner_refs_marker: bool,
    pub before: TokenAttestation,
    pub after: TokenAttestation,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskDefinition {
    pub principal_sid: String,
    pub logon_type: String,
    pub run_level: TaskRunLevel,
    pub command: String,
    pub arguments: String,
    pub working_directory: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TaskRunLevel {
    ExplicitLeastPrivilege,
    ImplicitLeastPrivilege,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskRuntime {
    pub status: String,
    pub last_run_time: String,
    pub last_task_result: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalState {
    Pending,
    Verified,
    Rejected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryDecision {
    NoHeldLease,
    CleanThenReclaim,
    FailClosed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RailError {
    code: &'static str,
}

impl RailError {
    pub const fn new(code: &'static str) -> Self {
        Self { code }
    }
}

impl fmt::Display for RailError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code)
    }
}

impl std::error::Error for RailError {}

pub fn mint_nonce() -> Result<String, RailError> {
    let mut bytes = [0_u8; NONCE_LENGTH];
    fill_random(&mut bytes).map_err(|_| RailError::new("nonce-csprng-unavailable"))?;
    Ok(bytes
        .into_iter()
        .map(|byte| NONCE_ALPHABET[usize::from(byte) % NONCE_ALPHABET.len()] as char)
        .collect())
}

pub fn task_name(nonce: &str) -> String {
    format!(
        r"\solstone\journal\ordinary-owner-{}",
        nonce.to_ascii_lowercase()
    )
}

pub fn task_command_line(worker_path: &str, lease_path: &str, nonce: &str) -> String {
    format!("\"{worker_path}\" limited-child --lease \"{lease_path}\" --nonce {nonce}")
}

pub fn schtasks_create_argv(lease: &LeaseRecord) -> Vec<String> {
    vec![
        "/Create".to_owned(),
        "/TN".to_owned(),
        lease.task_name.clone(),
        "/TR".to_owned(),
        lease.task_command.clone(),
        "/SC".to_owned(),
        "ONCE".to_owned(),
        "/SD".to_owned(),
        "12/31/2099".to_owned(),
        "/ST".to_owned(),
        "23:59".to_owned(),
        "/RL".to_owned(),
        "LIMITED".to_owned(),
        "/IT".to_owned(),
        "/RU".to_owned(),
        lease.expected_owner_account.clone(),
    ]
}

pub fn schtasks_query_xml_argv(task_name: &str) -> Vec<String> {
    vec![
        "/Query".to_owned(),
        "/TN".to_owned(),
        task_name.to_owned(),
        "/XML".to_owned(),
    ]
}

pub fn schtasks_query_runtime_argv(task_name: &str) -> Vec<String> {
    vec![
        "/Query".to_owned(),
        "/TN".to_owned(),
        task_name.to_owned(),
        "/FO".to_owned(),
        "CSV".to_owned(),
        "/V".to_owned(),
    ]
}

pub fn schtasks_run_argv(task_name: &str) -> Vec<String> {
    vec!["/Run".to_owned(), "/TN".to_owned(), task_name.to_owned()]
}

pub fn schtasks_delete_argv(task_name: &str) -> Vec<String> {
    vec![
        "/Delete".to_owned(),
        "/TN".to_owned(),
        task_name.to_owned(),
        "/F".to_owned(),
    ]
}

/// An exact `/Query /TN <nonce-derived-name> /XML` is proof of task absence only for the
/// scheduler's specific missing-task diagnostics.  Other nonzero outcomes remain ambiguous.
pub fn schtasks_query_proves_task_absent(output: &str) -> bool {
    let output = output.to_ascii_lowercase();
    output.contains("cannot find the file specified")
        || output.contains("cannot find the path specified")
        || output.contains("task not found")
}

/// COM `GetTask`/`GetFolder` is proof of absence only for documented not-found
/// HRESULTs. `0x80041326` is `SCHED_E_TASK_DISABLED`, not a missing object.
pub fn com_hresult_proves_not_found(hresult: i32) -> bool {
    matches!(hresult as u32, 0x8007_0002 | 0x8007_0003 | 0x8004_130D)
}

pub fn parse_task_xml(xml: &str) -> Result<TaskDefinition, RailError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut stack = Vec::new();
    let mut values = std::collections::BTreeMap::<String, Vec<String>>::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(tag)) => {
                stack.push(String::from_utf8_lossy(tag.name().as_ref()).into_owned());
                values
                    .entry(stack.join("/"))
                    .or_default()
                    .push(String::new());
            }
            Ok(Event::Empty(tag)) => {
                let mut key = stack.clone();
                key.push(String::from_utf8_lossy(tag.name().as_ref()).into_owned());
                values.entry(key.join("/")).or_default().push(String::new());
            }
            Ok(Event::End(_)) => {
                stack.pop();
            }
            Ok(Event::Text(text)) => {
                if stack.is_empty() {
                    let value = text
                        .decode()
                        .map_err(|_| RailError::new("task-xml-invalid-text"))?;
                    if value.trim().is_empty() {
                        continue;
                    }
                    return Err(RailError::new("task-xml-text-without-element"));
                }
                let key = stack.join("/");
                let value = text
                    .decode()
                    .map_err(|_| RailError::new("task-xml-invalid-text"))?;
                values
                    .get_mut(&key)
                    .and_then(|entries| entries.last_mut())
                    .ok_or(RailError::new("task-xml-text-without-element"))?
                    .push_str(&value);
            }
            Ok(Event::GeneralRef(reference)) => {
                if stack.is_empty() {
                    return Err(RailError::new("task-xml-reference-without-element"));
                }
                let key = stack.join("/");
                let name = reference
                    .decode()
                    .map_err(|_| RailError::new("task-xml-invalid-reference"))?;
                let value = match name.as_ref() {
                    "quot" => "\"",
                    "apos" => "'",
                    "lt" => "<",
                    "gt" => ">",
                    "amp" => "&",
                    _ => return Err(RailError::new("task-xml-unknown-reference")),
                };
                values
                    .get_mut(&key)
                    .and_then(|entries| entries.last_mut())
                    .ok_or(RailError::new("task-xml-reference-without-element"))?
                    .push_str(value);
            }
            Ok(Event::Eof) => break,
            Err(_) => return Err(RailError::new("task-xml-malformed")),
            _ => {}
        }
    }
    let one = |suffix: &str, required: bool| -> Result<Option<String>, RailError> {
        let canonical = format!("Task/{suffix}");
        let field = suffix
            .rsplit('/')
            .next()
            .ok_or(RailError::new("task-xml-missing-required-field"))?;
        if values
            .iter()
            .filter(|(key, _)| key.rsplit('/').next() == Some(field))
            .any(|(key, _)| key.as_str() != canonical)
        {
            return Err(RailError::new("task-xml-security-field-location-mismatch"));
        }
        let entries = values
            .get(&canonical)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if entries.len() > 1 {
            return Err(RailError::new("task-xml-duplicate-field"));
        }
        match entries.first() {
            Some(value) => Ok(Some((*value).clone())),
            None if required => Err(RailError::new("task-xml-missing-required-field")),
            None => Ok(None),
        }
    };
    let required =
        |suffix: &str| one(suffix, true)?.ok_or(RailError::new("task-xml-missing-required-field"));
    let action_children = values
        .keys()
        .filter(|key| key.starts_with("Task/Actions/") && key.split('/').count() == 3)
        .collect::<Vec<_>>();
    if action_children.len() != 1 || action_children[0].as_str() != "Task/Actions/Exec" {
        return Err(RailError::new("task-xml-action-set-mismatch"));
    }
    let run_level = match one("Principals/Principal/RunLevel", false)? {
        None => TaskRunLevel::ImplicitLeastPrivilege,
        Some(value) if value.trim() == "LeastPrivilege" => TaskRunLevel::ExplicitLeastPrivilege,
        Some(_) => return Err(RailError::new("task-run-level-mismatch")),
    };
    Ok(TaskDefinition {
        principal_sid: required("Principals/Principal/UserId")?.trim().to_owned(),
        logon_type: required("Principals/Principal/LogonType")?
            .trim()
            .to_owned(),
        run_level,
        command: required("Actions/Exec/Command")?.trim().to_owned(),
        arguments: required("Actions/Exec/Arguments")?.trim().to_owned(),
        working_directory: one("Actions/Exec/WorkingDirectory", false)?,
    })
}

fn normalized_command(command: &str) -> Result<&str, RailError> {
    if command.starts_with('"') || command.ends_with('"') {
        if command.len() < 2 || !command.starts_with('"') || !command.ends_with('"') {
            return Err(RailError::new("task-command-quote-mismatch"));
        }
        let inner = &command[1..command.len() - 1];
        if inner.is_empty() || inner.contains('"') {
            return Err(RailError::new("task-command-quote-mismatch"));
        }
        return Ok(inner);
    }
    if command.contains('"') {
        return Err(RailError::new("task-command-quote-mismatch"));
    }
    Ok(command)
}

pub fn verify_task_definition(
    lease: &LeaseRecord,
    definition: &TaskDefinition,
) -> Result<(), RailError> {
    if definition.principal_sid != lease.expected_owner_sid {
        return Err(RailError::new("task-principal-sid-mismatch"));
    }
    if definition.logon_type != "InteractiveToken" {
        return Err(RailError::new("task-logon-type-mismatch"));
    }
    let expected_arguments = format!(
        "limited-child --lease \"{}\" --nonce {}",
        LEASE_PATH, lease.nonce
    );
    if normalized_command(&definition.command)? != lease.worker_path
        || definition.arguments != expected_arguments
    {
        return Err(RailError::new("task-command-or-nonce-mismatch"));
    }
    // `schtasks /Create` has no working-directory switch. The child binds and attests the lease
    // worktree itself; a present XML element must still bind exactly to that worktree.
    if definition
        .working_directory
        .as_deref()
        .is_some_and(|value| value != lease.worktree)
    {
        return Err(RailError::new("task-working-directory-mismatch"));
    }
    Ok(())
}

/// Parse `schtasks /Query /FO CSV /V`: XML is locale-safe for definitions but omits runtime
/// history, while CSV keeps a stable header/row layout for `Status`, `Last Run Time`, and result.
pub fn parse_task_runtime_csv(csv: &str) -> Result<TaskRuntime, RailError> {
    let rows = parse_csv_rows(csv)?;
    let (header, row) = rows
        .first()
        .zip(rows.get(1))
        .ok_or(RailError::new("task-runtime-csv-missing-row"))?;
    let column = |name: &str| {
        header
            .iter()
            .position(|value| value.eq_ignore_ascii_case(name))
            .and_then(|index| row.get(index))
            .cloned()
            .ok_or(RailError::new("task-runtime-csv-missing-column"))
    };
    Ok(TaskRuntime {
        status: column("Status")?,
        last_run_time: column("Last Run Time")?,
        last_task_result: column("Last Result")?,
    })
}

pub fn classify_terminal(
    runtime: &TaskRuntime,
    launch_observed_last_run_time: Option<&str>,
) -> TerminalState {
    if runtime.status.eq_ignore_ascii_case("running") {
        return TerminalState::Pending;
    }
    if !runtime.status.eq_ignore_ascii_case("ready") {
        return TerminalState::Rejected;
    }
    if runtime.last_run_time.is_empty()
        || runtime.last_run_time.eq_ignore_ascii_case("n/a")
        || launch_observed_last_run_time == Some(runtime.last_run_time.as_str())
    {
        return TerminalState::Pending;
    }
    if parse_scheduler_result(&runtime.last_task_result) == Some(0) {
        TerminalState::Verified
    } else {
        TerminalState::Rejected
    }
}

pub fn verify_attestation(
    lease: &LeaseRecord,
    attestation: &TokenAttestation,
) -> Result<(), RailError> {
    if attestation.owner_sid != lease.expected_owner_sid {
        return Err(RailError::new("owner-sid-mismatch"));
    }
    if attestation.session != lease.expected_session {
        return Err(RailError::new("session-mismatch"));
    }
    if attestation.elevated {
        return Err(RailError::new("token-elevated"));
    }
    if attestation.backup_privilege_present {
        return Err(RailError::new("token-backup-privilege-present"));
    }
    if attestation.restore_privilege_present {
        return Err(RailError::new("token-restore-privilege-present"));
    }
    Ok(())
}

/// Verify the nonce-bound identity and token evidence independently of the
/// controlled test's pass/fail result.  A terminal nonzero test outcome may be
/// cleaned only after this binding has held; an unbound or malformed result
/// remains a held recovery case.
pub fn verify_result_binding(lease: &LeaseRecord, result: &ResultRecord) -> Result<(), RailError> {
    if result.nonce != lease.nonce {
        return Err(RailError::new("result-nonce-mismatch"));
    }
    if result.selected_control.as_deref() != Some(lease.control_id.as_str()) {
        return Err(RailError::new("selected-alternate-control"));
    }
    if result.executed_control.as_deref() != Some(lease.control_id.as_str()) {
        return Err(RailError::new("ignored-unselected-control"));
    }
    if result.payload_sha256 != lease.payload_sha256 {
        return Err(RailError::new("result-payload-mismatch"));
    }
    verify_attestation(lease, &result.before)?;
    verify_attestation(lease, &result.after)?;
    Ok(())
}

pub fn verify_result(lease: &LeaseRecord, result: &ResultRecord) -> Result<(), RailError> {
    verify_result_binding(lease, result)?;
    if !result.passed || result.cargo_exit_code != 0 || !result.ordinary_owner_marker {
        return Err(RailError::new("ordinary-owner-control-failed"));
    }
    if !lease.refs_root.is_empty() && !result.ordinary_owner_refs_marker {
        return Err(RailError::new("ordinary-owner-refs-control-failed"));
    }
    Ok(())
}

pub fn recovery_decision(
    lease: Option<&LeaseRecord>,
    task_exists: bool,
    task_is_terminal: bool,
) -> RecoveryDecision {
    match lease {
        None => RecoveryDecision::NoHeldLease,
        Some(_) if task_exists && !task_is_terminal => RecoveryDecision::FailClosed,
        Some(_) => RecoveryDecision::CleanThenReclaim,
    }
}

pub fn require_clean_worktree(status_porcelain: &str) -> Result<(), RailError> {
    if status_porcelain.trim().is_empty() {
        Ok(())
    } else {
        Err(RailError::new("worktree-dirty"))
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn payload_sha256(parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update(part.as_bytes());
        digest.update([0]);
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub const PROBE_SCHEMA: &str = "solstone.journal.win-owner-rail.onlogon-probe.v1";
pub const PROBE_RECEIPT_SCHEMA: &str = "solstone.journal.win-owner-rail.onlogon-probe-receipt.v1";
pub const PROBE_MARKER_PREFIX: &str = "solstone-onlogon-probe:";
pub const ACE_MUTATION_MASK: u32 =
    0x0001_0000 | 0x0004_0000 | 0x0008_0000 | 0x1000_0000 | 0x4000_0000;
pub const TASK_CREATION_CREATE: i32 = 2;
pub const TASK_CREATION_CREATE_OR_UPDATE: i32 = 6;

pub fn com_register_flag() -> i32 {
    TASK_CREATION_CREATE_OR_UPDATE
}

pub fn probe_marker(nonce: &str) -> String {
    format!("{PROBE_MARKER_PREFIX}{nonce}")
}

pub fn probe_xml_task_name(nonce: &str) -> String {
    format!(
        r"\solstone\probe\onlogon-xml-{}",
        nonce.to_ascii_lowercase()
    )
}

pub fn probe_com_folder_path(nonce: &str) -> String {
    format!(
        r"\solstone\probe\onlogon-com-{}",
        nonce.to_ascii_lowercase()
    )
}

pub fn probe_com_task_name() -> &'static str {
    "probe"
}

pub fn schtasks_create_xml_argv(task_name: &str, xml_path: &str) -> Vec<String> {
    vec![
        "/Create".to_owned(),
        "/TN".to_owned(),
        task_name.to_owned(),
        "/XML".to_owned(),
        xml_path.to_owned(),
    ]
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProbeTaskDefinition {
    pub principal_sid: String,
    pub logon_type: String,
    pub run_level: TaskRunLevel,
    pub command: String,
    pub arguments: String,
    pub working_directory: Option<String>,
    pub description: Option<String>,
    pub trigger_user_id: String,
    pub priority: ProbePriority,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProbePriority {
    Present(i32),
    Unavailable,
    Malformed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProbeVerifyInput {
    pub expected_sid: String,
    pub expected_command: String,
    pub marker: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TokenElevationClass {
    Default,
    Limited,
    Full,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProbeTokenGate {
    ContinueUnelevated,
    StopElevated,
    StopFilteredAdmin,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProbeSidClass {
    Caller,
    Administrators,
    LocalSystem,
    LocalService,
    NetworkService,
    OrdinaryGroup,
    OrdinaryDistinctUser,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProbeAclEntry {
    pub sid: String,
    pub allow: bool,
    pub access_mask: u32,
    pub classification: ProbeSidClass,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProbeOutcome {
    ElevatedCreator,
    FilteredAdministratorCreator,
    RegistrationRefused,
    VerifiedRegistration,
    DefinitionUnverified,
    CleanupUncertain,
    ReceiptUnavailable,
    TerminalUpdateFailed,
}

impl fmt::Display for ProbeOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ElevatedCreator => "elevated-creator",
            Self::FilteredAdministratorCreator => "filtered-administrator-creator",
            Self::RegistrationRefused => "registration-refused",
            Self::VerifiedRegistration => "verified-registration",
            Self::DefinitionUnverified => "definition-unverified",
            Self::CleanupUncertain => "cleanup-uncertain",
            Self::ReceiptUnavailable => "receipt-unavailable",
            Self::TerminalUpdateFailed => "terminal-update-failed",
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProbeStageStatus {
    Passed,
    Failed,
    Inconclusive,
    Skipped,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProbeResolutionAction {
    UseStandardLogon,
    StopFilteredAdmin,
    TreatAsRefused,
    InspectDefinition,
    RetryFreshNonce,
    RepairSchedulerAccess,
    IsolateFolderAcl,
    ManualCleanup,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProbeApiErrorSpace {
    Win32,
    Hresult,
    Schtasks,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProbeApiError {
    pub space: ProbeApiErrorSpace,
    pub code: u32,
    pub diagnostic: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProbeStage {
    pub status: ProbeStageStatus,
    pub error: Option<ProbeApiError>,
    pub resolution_action: Option<ProbeResolutionAction>,
}

impl ProbeStage {
    pub fn passed() -> Self {
        Self {
            status: ProbeStageStatus::Passed,
            error: None,
            resolution_action: None,
        }
    }

    pub fn skipped() -> Self {
        Self {
            status: ProbeStageStatus::Skipped,
            error: None,
            resolution_action: None,
        }
    }

    pub fn failed(error: ProbeApiError, action: ProbeResolutionAction) -> Self {
        Self {
            status: ProbeStageStatus::Failed,
            error: Some(error),
            resolution_action: Some(action),
        }
    }

    pub fn inconclusive(error: ProbeApiError, action: ProbeResolutionAction) -> Self {
        Self {
            status: ProbeStageStatus::Inconclusive,
            error: Some(error),
            resolution_action: Some(action),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProbePriorityState {
    Present,
    Unavailable,
    Malformed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProbePriorityView {
    pub state: ProbePriorityState,
    pub value: Option<i32>,
}

impl From<ProbePriority> for ProbePriorityView {
    fn from(priority: ProbePriority) -> Self {
        match priority {
            ProbePriority::Present(value) => Self {
                state: ProbePriorityState::Present,
                value: Some(value),
            },
            ProbePriority::Unavailable => Self {
                state: ProbePriorityState::Unavailable,
                value: None,
            },
            ProbePriority::Malformed => Self {
                state: ProbePriorityState::Malformed,
                value: None,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProbeDescriptorSource {
    NamedInfo,
    ComSddl,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProbeTokenSection {
    pub elevation_type: TokenElevationClass,
    pub elevated: bool,
    pub session: u32,
    pub owner_sid: String,
    pub stage: ProbeStage,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProbeXmlSection {
    pub create: ProbeStage,
    pub definition: ProbeStage,
    pub priority: ProbePriorityView,
    pub cleanup: ProbeStage,
    #[serde(default)]
    pub create_argv: Vec<String>,
    #[serde(default)]
    pub query_argv: Vec<String>,
    #[serde(default)]
    pub definition_sha256: String,
}

impl ProbeXmlSection {
    pub fn skipped() -> Self {
        Self {
            create: ProbeStage::skipped(),
            definition: ProbeStage::skipped(),
            priority: ProbePriority::Unavailable.into(),
            cleanup: ProbeStage::skipped(),
            create_argv: Vec::new(),
            query_argv: Vec::new(),
            definition_sha256: String::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProbeFolderAclStage {
    pub status: ProbeStageStatus,
    pub error: Option<ProbeApiError>,
    pub resolution_action: Option<ProbeResolutionAction>,
    pub descriptor_source: Option<ProbeDescriptorSource>,
    pub sddl: Option<String>,
    pub ordinary_mutation: Option<bool>,
}

impl ProbeFolderAclStage {
    pub fn skipped() -> Self {
        Self {
            status: ProbeStageStatus::Skipped,
            error: None,
            resolution_action: None,
            descriptor_source: None,
            sddl: None,
            ordinary_mutation: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProbeComSection {
    pub folder_create: ProbeStage,
    pub folder_acl: ProbeFolderAclStage,
    pub register: ProbeStage,
    pub definition: ProbeStage,
    pub priority: ProbePriorityView,
    pub cleanup: ProbeStage,
    #[serde(default)]
    pub register_flag: i32,
    #[serde(default)]
    pub call_sequence: Vec<String>,
}

impl ProbeComSection {
    pub fn skipped() -> Self {
        Self {
            folder_create: ProbeStage::skipped(),
            folder_acl: ProbeFolderAclStage::skipped(),
            register: ProbeStage::skipped(),
            definition: ProbeStage::skipped(),
            priority: ProbePriority::Unavailable.into(),
            cleanup: ProbeStage::skipped(),
            register_flag: 0,
            call_sequence: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OnlogonProbeReport {
    pub schema: String,
    pub outcome: ProbeOutcome,
    pub nonce: String,
    pub marker: String,
    pub xml_task_name: String,
    pub com_folder_path: String,
    pub token: ProbeTokenSection,
    pub xml: ProbeXmlSection,
    pub com: ProbeComSection,
}

/// Mirrors `LeaseRecord`/`LeaseState::transition`: a schema-versioned on-disk record
/// with a guarded one-way `Incomplete -> Terminal` state transition, so a crash between
/// establishing the receipt and finalizing it leaves a durable, honestly-marked
/// "incomplete" record instead of no evidence at all.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProbeReceiptState {
    Incomplete,
    Terminal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProbeReceipt {
    pub schema: String,
    pub marker: String,
    pub nonce: String,
    pub state: ProbeReceiptState,
    pub report: Option<OnlogonProbeReport>,
}

impl ProbeReceipt {
    pub fn incomplete(nonce: &str, marker: &str) -> Self {
        Self {
            schema: PROBE_RECEIPT_SCHEMA.to_owned(),
            marker: marker.to_owned(),
            nonce: nonce.to_owned(),
            state: ProbeReceiptState::Incomplete,
            report: None,
        }
    }

    pub fn finalize(&mut self, report: OnlogonProbeReport) -> Result<(), RailError> {
        if self.state != ProbeReceiptState::Incomplete {
            return Err(RailError::new("invalid-receipt-transition"));
        }
        if report.marker != self.marker || report.nonce != self.nonce {
            return Err(RailError::new("receipt-marker-mismatch"));
        }
        self.state = ProbeReceiptState::Terminal;
        self.report = Some(report);
        Ok(())
    }
}

pub fn classify_token_elevation_type(value: i32) -> TokenElevationClass {
    match value {
        1 => TokenElevationClass::Default,
        2 => TokenElevationClass::Full,
        3 => TokenElevationClass::Limited,
        _ => TokenElevationClass::Unknown,
    }
}

pub fn probe_token_gate(
    class: TokenElevationClass,
    token_is_elevated: bool,
) -> Result<ProbeTokenGate, RailError> {
    match (class, token_is_elevated) {
        (TokenElevationClass::Full, true) => Ok(ProbeTokenGate::StopElevated),
        (TokenElevationClass::Limited, false) => Ok(ProbeTokenGate::StopFilteredAdmin),
        (TokenElevationClass::Default, false) => Ok(ProbeTokenGate::ContinueUnelevated),
        (TokenElevationClass::Unknown, _) => Ok(ProbeTokenGate::StopElevated),
        _ => Err(RailError::new("token-elevation-inconsistent")),
    }
}

pub fn classify_probe_sid(sid: &str, caller_sid: &str) -> ProbeSidClass {
    if sid.eq_ignore_ascii_case(caller_sid) {
        return ProbeSidClass::Caller;
    }
    if sid.eq_ignore_ascii_case("S-1-5-18") {
        return ProbeSidClass::LocalSystem;
    }
    if sid.eq_ignore_ascii_case("S-1-5-19") {
        return ProbeSidClass::LocalService;
    }
    if sid.eq_ignore_ascii_case("S-1-5-20") {
        return ProbeSidClass::NetworkService;
    }
    if sid.eq_ignore_ascii_case("S-1-5-32-544") {
        return ProbeSidClass::Administrators;
    }
    if sid.eq_ignore_ascii_case("S-1-5-32-545")
        || sid.eq_ignore_ascii_case("S-1-5-11")
        || sid.eq_ignore_ascii_case("S-1-1-0")
        || sid.eq_ignore_ascii_case("S-1-5-4")
        || sid.eq_ignore_ascii_case("S-1-5-32-546")
        || sid.eq_ignore_ascii_case("S-1-5-32-547")
    {
        return ProbeSidClass::OrdinaryGroup;
    }
    ProbeSidClass::OrdinaryDistinctUser
}

pub fn normalize_probe_acl_entries(entries: &[ProbeAclEntry]) -> Vec<ProbeAclEntry> {
    let mut normalized = entries
        .iter()
        .map(|entry| ProbeAclEntry {
            sid: entry.sid.to_ascii_uppercase(),
            allow: entry.allow,
            access_mask: entry.access_mask,
            classification: entry.classification,
        })
        .collect::<Vec<_>>();
    normalized.sort_by(|left, right| {
        (left.sid.as_str(), left.allow, left.access_mask).cmp(&(
            right.sid.as_str(),
            right.allow,
            right.access_mask,
        ))
    });
    normalized
}

pub fn folder_allows_ordinary_mutation(entries: &[ProbeAclEntry]) -> bool {
    entries.iter().any(|entry| {
        entry.allow
            && entry.access_mask & ACE_MUTATION_MASK != 0
            && matches!(
                entry.classification,
                ProbeSidClass::OrdinaryGroup | ProbeSidClass::OrdinaryDistinctUser
            )
    })
}

pub fn stage_requires_resolution_action(stage: &ProbeStage) -> bool {
    matches!(
        stage.status,
        ProbeStageStatus::Failed | ProbeStageStatus::Inconclusive
    )
}

pub fn probe_exits_successfully(report: &OnlogonProbeReport) -> bool {
    if report.token.stage.status == ProbeStageStatus::Failed {
        return false;
    }
    matches!(
        report.outcome,
        ProbeOutcome::VerifiedRegistration
            | ProbeOutcome::RegistrationRefused
            | ProbeOutcome::ElevatedCreator
            | ProbeOutcome::FilteredAdministratorCreator
    )
}

pub fn select_probe_outcome(report: &OnlogonProbeReport) -> ProbeOutcome {
    match report.token.elevation_type {
        TokenElevationClass::Full if report.token.stage.status != ProbeStageStatus::Failed => {
            return ProbeOutcome::ElevatedCreator;
        }
        TokenElevationClass::Limited if report.token.stage.status != ProbeStageStatus::Failed => {
            return ProbeOutcome::FilteredAdministratorCreator;
        }
        TokenElevationClass::Unknown => return ProbeOutcome::ElevatedCreator,
        _ => {}
    }
    if report.token.stage.status == ProbeStageStatus::Failed {
        return ProbeOutcome::ElevatedCreator;
    }
    if matches!(
        report.xml.cleanup.status,
        ProbeStageStatus::Failed | ProbeStageStatus::Inconclusive
    ) || matches!(
        report.com.cleanup.status,
        ProbeStageStatus::Failed | ProbeStageStatus::Inconclusive
    ) {
        return ProbeOutcome::CleanupUncertain;
    }
    let xml_created = report.xml.create.status == ProbeStageStatus::Passed;
    let com_created = report.com.register.status == ProbeStageStatus::Passed;
    if xml_created
        && matches!(
            report.xml.definition.status,
            ProbeStageStatus::Failed | ProbeStageStatus::Inconclusive
        )
        || com_created
            && matches!(
                report.com.definition.status,
                ProbeStageStatus::Failed | ProbeStageStatus::Inconclusive
            )
    {
        return ProbeOutcome::DefinitionUnverified;
    }
    if report.com.folder_acl.status == ProbeStageStatus::Inconclusive {
        return ProbeOutcome::DefinitionUnverified;
    }
    let xml_verified = xml_created && report.xml.definition.status == ProbeStageStatus::Passed;
    let com_verified = com_created && report.com.definition.status == ProbeStageStatus::Passed;
    if xml_verified && com_verified {
        return ProbeOutcome::VerifiedRegistration;
    }
    let xml_refused =
        report.xml.create.resolution_action == Some(ProbeResolutionAction::TreatAsRefused);
    let com_refused = report.com.folder_create.resolution_action
        == Some(ProbeResolutionAction::TreatAsRefused)
        || report.com.register.resolution_action == Some(ProbeResolutionAction::TreatAsRefused);
    if (xml_refused || com_refused)
        && !create_stage_blocks_refusal(&report.xml.create)
        && !create_stage_blocks_refusal(&report.com.folder_create)
        && !create_stage_blocks_refusal(&report.com.register)
    {
        return ProbeOutcome::RegistrationRefused;
    }
    ProbeOutcome::DefinitionUnverified
}

fn create_stage_blocks_refusal(stage: &ProbeStage) -> bool {
    matches!(
        stage.status,
        ProbeStageStatus::Failed | ProbeStageStatus::Inconclusive
    ) && stage.resolution_action != Some(ProbeResolutionAction::TreatAsRefused)
}

pub fn priority_from_raw_value(value: i32) -> ProbePriority {
    if (0..=10).contains(&value) {
        ProbePriority::Present(value)
    } else {
        ProbePriority::Malformed
    }
}

pub fn priority_from_xml_text(value: Option<&str>) -> ProbePriority {
    let Some(text) = value.map(str::trim).filter(|text| !text.is_empty()) else {
        return ProbePriority::Unavailable;
    };
    match text.parse::<i32>() {
        Ok(priority) => priority_from_raw_value(priority),
        _ => ProbePriority::Malformed,
    }
}

pub fn parse_probe_task_xml(xml: &str) -> Result<ProbeTaskDefinition, RailError> {
    let values = collect_xml_values(xml)?;
    let one = |suffix: &str, required: bool| -> Result<Option<String>, RailError> {
        probe_xml_field(&values, suffix, required, false)
    };
    let required =
        |suffix: &str| one(suffix, true)?.ok_or(RailError::new("task-xml-missing-required-field"));
    let action_children = values
        .keys()
        .filter(|key| key.starts_with("Task/Actions/") && key.split('/').count() == 3)
        .cloned()
        .collect::<Vec<_>>();
    if action_children.len() != 1 || action_children[0].as_str() != "Task/Actions/Exec" {
        return Err(RailError::new("task-xml-action-set-mismatch"));
    }
    let trigger_children = values
        .keys()
        .filter(|key| key.starts_with("Task/Triggers/") && key.split('/').count() == 3)
        .cloned()
        .collect::<Vec<_>>();
    if trigger_children.len() != 1 || trigger_children[0].as_str() != "Task/Triggers/LogonTrigger" {
        return Err(RailError::new("task-xml-trigger-set-mismatch"));
    }
    let userid_keys = values
        .keys()
        .filter(|key| key.rsplit('/').next() == Some("UserId"))
        .cloned()
        .collect::<Vec<_>>();
    let allowed_user_ids = [
        "Task/Principals/Principal/UserId",
        "Task/Triggers/LogonTrigger/UserId",
    ];
    if userid_keys
        .iter()
        .any(|key| !allowed_user_ids.contains(&key.as_str()))
    {
        return Err(RailError::new("task-xml-security-field-location-mismatch"));
    }
    let run_level = match one("Principals/Principal/RunLevel", false)? {
        None => TaskRunLevel::ImplicitLeastPrivilege,
        Some(value) if value.trim() == "LeastPrivilege" => TaskRunLevel::ExplicitLeastPrivilege,
        Some(_) => return Err(RailError::new("task-run-level-mismatch")),
    };
    let principal_sid = probe_xml_field(&values, "Principals/Principal/UserId", true, true)?
        .ok_or(RailError::new("task-xml-missing-required-field"))?
        .trim()
        .to_owned();
    let trigger_user_id = probe_xml_field(&values, "Triggers/LogonTrigger/UserId", true, true)?
        .ok_or(RailError::new("task-xml-missing-required-field"))?
        .trim()
        .to_owned();
    Ok(ProbeTaskDefinition {
        principal_sid,
        logon_type: required("Principals/Principal/LogonType")?
            .trim()
            .to_owned(),
        run_level,
        command: required("Actions/Exec/Command")?.trim().to_owned(),
        arguments: required("Actions/Exec/Arguments")?.trim().to_owned(),
        working_directory: one("Actions/Exec/WorkingDirectory", false)?,
        description: one("RegistrationInfo/Description", false)?,
        trigger_user_id,
        priority: priority_from_xml_text(one("Settings/Priority", false)?.as_deref()),
    })
}

pub fn render_probe_task_xml(definition: &ProbeTaskDefinition) -> String {
    let description = definition
        .description
        .as_deref()
        .map(|value| {
            format!(
                "<RegistrationInfo><Description>{}</Description></RegistrationInfo>",
                xml_escape(value)
            )
        })
        .unwrap_or_default();
    let run_level = match definition.run_level {
        TaskRunLevel::ExplicitLeastPrivilege => "<RunLevel>LeastPrivilege</RunLevel>",
        TaskRunLevel::ImplicitLeastPrivilege => "",
    };
    let working_directory = definition
        .working_directory
        .as_deref()
        .map(|value| format!("<WorkingDirectory>{}</WorkingDirectory>", xml_escape(value)))
        .unwrap_or_default();
    let priority = match definition.priority {
        ProbePriority::Present(value) => {
            format!("<Settings><Priority>{value}</Priority></Settings>")
        }
        ProbePriority::Unavailable | ProbePriority::Malformed => String::new(),
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Task xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task" version="1.2">{description}<Triggers><LogonTrigger><UserId>{}</UserId></LogonTrigger></Triggers><Principals><Principal><UserId>{}</UserId><LogonType>{}</LogonType>{run_level}</Principal></Principals>{priority}<Actions><Exec><Command>{}</Command><Arguments>{}</Arguments>{working_directory}</Exec></Actions></Task>"#,
        xml_escape(&definition.trigger_user_id),
        xml_escape(&definition.principal_sid),
        xml_escape(&definition.logon_type),
        xml_escape(&definition.command),
        xml_escape(&definition.arguments),
    )
}

pub fn verify_probe_task_definition(
    definition: &ProbeTaskDefinition,
    expected: &ProbeVerifyInput,
) -> Result<(), RailError> {
    if definition.principal_sid != expected.expected_sid {
        return Err(RailError::new("task-principal-sid-mismatch"));
    }
    if definition.trigger_user_id != expected.expected_sid {
        return Err(RailError::new("task-trigger-sid-mismatch"));
    }
    if definition.logon_type != "InteractiveToken" {
        return Err(RailError::new("task-logon-type-mismatch"));
    }
    if normalized_command(&definition.command)? != expected.expected_command {
        return Err(RailError::new("task-command-or-nonce-mismatch"));
    }
    if definition.arguments != expected.marker {
        return Err(RailError::new("task-command-or-nonce-mismatch"));
    }
    match definition.description.as_deref().map(str::trim) {
        Some(value) if value == expected.marker => Ok(()),
        None | Some("") => Err(RailError::new("marker-missing")),
        Some(_) => Err(RailError::new("marker-mismatch")),
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn collect_xml_values(
    xml: &str,
) -> Result<std::collections::BTreeMap<String, Vec<String>>, RailError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut stack = Vec::new();
    let mut values = std::collections::BTreeMap::<String, Vec<String>>::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(tag)) => {
                stack.push(String::from_utf8_lossy(tag.name().as_ref()).into_owned());
                values
                    .entry(stack.join("/"))
                    .or_default()
                    .push(String::new());
            }
            Ok(Event::Empty(tag)) => {
                let mut key = stack.clone();
                key.push(String::from_utf8_lossy(tag.name().as_ref()).into_owned());
                values.entry(key.join("/")).or_default().push(String::new());
            }
            Ok(Event::End(_)) => {
                stack.pop();
            }
            Ok(Event::Text(text)) => {
                if stack.is_empty() {
                    let value = text
                        .decode()
                        .map_err(|_| RailError::new("task-xml-invalid-text"))?;
                    if value.trim().is_empty() {
                        continue;
                    }
                    return Err(RailError::new("task-xml-text-without-element"));
                }
                let key = stack.join("/");
                let value = text
                    .decode()
                    .map_err(|_| RailError::new("task-xml-invalid-text"))?;
                values
                    .get_mut(&key)
                    .and_then(|entries| entries.last_mut())
                    .ok_or(RailError::new("task-xml-text-without-element"))?
                    .push_str(&value);
            }
            Ok(Event::GeneralRef(reference)) => {
                if stack.is_empty() {
                    return Err(RailError::new("task-xml-reference-without-element"));
                }
                let key = stack.join("/");
                let name = reference
                    .decode()
                    .map_err(|_| RailError::new("task-xml-invalid-reference"))?;
                let value = match name.as_ref() {
                    "quot" => "\"",
                    "apos" => "'",
                    "lt" => "<",
                    "gt" => ">",
                    "amp" => "&",
                    _ => return Err(RailError::new("task-xml-unknown-reference")),
                };
                values
                    .get_mut(&key)
                    .and_then(|entries| entries.last_mut())
                    .ok_or(RailError::new("task-xml-reference-without-element"))?
                    .push_str(value);
            }
            Ok(Event::Eof) => break,
            Err(_) => return Err(RailError::new("task-xml-malformed")),
            _ => {}
        }
    }
    Ok(values)
}

fn probe_xml_field(
    values: &std::collections::BTreeMap<String, Vec<String>>,
    suffix: &str,
    required: bool,
    skip_location_check: bool,
) -> Result<Option<String>, RailError> {
    let canonical = format!("Task/{suffix}");
    let field = suffix
        .rsplit('/')
        .next()
        .ok_or(RailError::new("task-xml-missing-required-field"))?;
    if !skip_location_check
        && values
            .iter()
            .filter(|(key, _)| key.rsplit('/').next() == Some(field))
            .any(|(key, _)| key.as_str() != canonical)
    {
        return Err(RailError::new("task-xml-security-field-location-mismatch"));
    }
    let entries = values
        .get(&canonical)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if entries.len() > 1 {
        return Err(RailError::new("task-xml-duplicate-field"));
    }
    match entries.first() {
        Some(value) => Ok(Some((*value).clone())),
        None if required => Err(RailError::new("task-xml-missing-required-field")),
        None => Ok(None),
    }
}

fn parse_scheduler_result(value: &str) -> Option<u32> {
    let trimmed = value.trim();
    if let Some(hex) = trimmed.strip_prefix("0x") {
        u32::from_str_radix(hex, 16).ok()
    } else {
        trimmed.parse().ok()
    }
}

fn parse_csv_rows(input: &str) -> Result<Vec<Vec<String>>, RailError> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = input.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => {
                row.push(std::mem::take(&mut cell));
            }
            '\n' if !quoted => {
                if cell.ends_with('\r') {
                    cell.pop();
                }
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            character => cell.push(character),
        }
    }
    if quoted {
        return Err(RailError::new("task-runtime-csv-unclosed-quote"));
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease() -> LeaseRecord {
        LeaseRecord::new(LeaseInput {
            nonce: "ABCDEFGH23456789ABCDEFGH23456789".to_owned(),
            lease_path: LEASE_PATH.to_owned(),
            expected_commit: "a".repeat(40),
            expected_cargo_lock_sha256: "b".repeat(64),
            expected_owner_account: "solbuild".to_owned(),
            expected_owner_sid: "S-1-5-21-100".to_owned(),
            expected_session: 1,
            worktree: r"C:\sol\solstone".to_owned(),
            worker_path: r"C:\sol\solstone\core\target\debug\solstone-core-win-owner-rail.exe"
                .to_owned(),
            worker_sha256: "c".repeat(64),
            refs_root: r"R:\refs".to_owned(),
            created_at_unix_seconds: 1,
        })
    }

    fn attestation() -> TokenAttestation {
        TokenAttestation {
            owner_sid: "S-1-5-21-100".to_owned(),
            session: 1,
            elevated: false,
            backup_privilege_present: false,
            restore_privilege_present: false,
        }
    }

    fn passed_result(lease: &LeaseRecord) -> ResultRecord {
        ResultRecord {
            schema: "solstone.journal.win-owner-rail.result.v1".to_owned(),
            nonce: lease.nonce.clone(),
            selected_control: Some(CONTROL_ID.to_owned()),
            executed_control: Some(CONTROL_ID.to_owned()),
            payload_sha256: lease.payload_sha256.clone(),
            passed: true,
            cargo_exit_code: 0,
            ordinary_owner_marker: true,
            ordinary_owner_refs_marker: true,
            before: attestation(),
            after: attestation(),
            error: None,
        }
    }

    fn definition(lease: &LeaseRecord) -> TaskDefinition {
        TaskDefinition {
            principal_sid: lease.expected_owner_sid.clone(),
            logon_type: "InteractiveToken".to_owned(),
            run_level: TaskRunLevel::ExplicitLeastPrivilege,
            command: lease.worker_path.clone(),
            arguments: format!(
                "limited-child --lease \"{LEASE_PATH}\" --nonce {}",
                lease.nonce
            ),
            working_directory: Some(lease.worktree.clone()),
        }
    }

    #[test]
    fn nonce_has_the_bounded_external_shape() {
        let nonce = mint_nonce().expect("nonce mints");
        assert_eq!(nonce.len(), NONCE_LENGTH);
        assert!(nonce.bytes().all(|byte| NONCE_ALPHABET.contains(&byte)));
    }

    #[test]
    fn lease_schema_and_state_machine_are_stable() {
        let mut lease = lease();
        assert_eq!(lease.schema, "solstone.journal.win-owner-rail.lease.v1");
        lease
            .transition(LeaseState::Launched)
            .expect("launch transition");
        lease
            .transition(LeaseState::TerminalVerified)
            .expect("verify transition");
        lease
            .transition(LeaseState::Cleaned)
            .expect("cleanup transition");
        assert_eq!(
            lease.transition(LeaseState::Launched),
            Err(RailError::new("invalid-lease-transition"))
        );
    }

    #[test]
    fn scheduled_task_argv_uses_limited_interactive_one_shot_principal() {
        let lease = lease();
        let argv = schtasks_create_argv(&lease);
        assert!(argv.windows(2).any(|pair| pair == ["/RL", "LIMITED"]));
        assert!(argv.windows(2).any(|pair| pair == ["/IT", "/RU"]));
        assert!(argv.windows(2).any(|pair| pair == ["/SC", "ONCE"]));
        assert!(argv.windows(2).any(|pair| pair == ["/RU", "solbuild"]));
        // A name collision must surface as a loud `/Create` failure, never a silent overwrite of
        // whatever task (possibly still live) already held the nonce-derived name.
        assert!(!argv.iter().any(|arg| arg == "/F"));
    }

    #[test]
    fn com_hresult_proves_absence_only_for_not_found() {
        assert!(com_hresult_proves_not_found(0x8007_0002_u32 as i32));
        assert!(com_hresult_proves_not_found(0x8007_0003_u32 as i32));
        assert!(com_hresult_proves_not_found(0x8004_130D_u32 as i32));
        assert!(!com_hresult_proves_not_found(0x8004_1326_u32 as i32));
        assert!(!com_hresult_proves_not_found(0x8007_0005_u32 as i32));
        assert!(!com_hresult_proves_not_found(0));
        assert!(!com_hresult_proves_not_found(1));
    }

    #[test]
    fn exact_scheduler_query_accepts_only_missing_task_diagnostics_as_absence() {
        assert!(schtasks_query_proves_task_absent(
            "ERROR: The system cannot find the file specified."
        ));
        assert!(schtasks_query_proves_task_absent(
            "ERROR: The system cannot find the path specified."
        ));
        assert!(schtasks_query_proves_task_absent("ERROR: Task not found."));
        assert!(!schtasks_query_proves_task_absent(
            "ERROR: Access is denied."
        ));
        assert!(!schtasks_query_proves_task_absent(
            "ERROR: The requested operation is unavailable."
        ));
    }

    #[test]
    fn xml_definition_verification_rejects_every_bound_field() {
        let lease = lease();
        let mut task = definition(&lease);
        assert_eq!(verify_task_definition(&lease, &task), Ok(()));
        task.principal_sid.push('x');
        assert_eq!(
            verify_task_definition(&lease, &task),
            Err(RailError::new("task-principal-sid-mismatch"))
        );
        task = definition(&lease);
        task.logon_type = "Password".to_owned();
        assert_eq!(
            verify_task_definition(&lease, &task),
            Err(RailError::new("task-logon-type-mismatch"))
        );
        task = definition(&lease);
        task.command.push('x');
        assert_eq!(
            verify_task_definition(&lease, &task),
            Err(RailError::new("task-command-or-nonce-mismatch"))
        );
        task = definition(&lease);
        task.arguments.push('x');
        assert_eq!(
            verify_task_definition(&lease, &task),
            Err(RailError::new("task-command-or-nonce-mismatch"))
        );
        task = definition(&lease);
        task.working_directory = Some(format!("{}x", lease.worktree));
        assert_eq!(
            verify_task_definition(&lease, &task),
            Err(RailError::new("task-working-directory-mismatch"))
        );
    }

    #[test]
    fn xml_parser_reads_a_realistic_scheduler_definition_fixture() {
        let xml = r#"<?xml version="1.0"?><Task><Principals><Principal><UserId>S-1-5-21-100</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals><Actions><Exec><Command>C:\worker.exe</Command><Arguments>limited-child --lease &quot;C:\lease.json&quot; --nonce ABC</Arguments><WorkingDirectory>C:\sol</WorkingDirectory></Exec></Actions></Task>"#;
        let parsed = parse_task_xml(xml).expect("parses task XML");
        assert_eq!(parsed.principal_sid, "S-1-5-21-100");
        assert_eq!(
            parsed.arguments,
            "limited-child --lease \"C:\\lease.json\" --nonce ABC"
        );
        assert_eq!(parsed.run_level, TaskRunLevel::ExplicitLeastPrivilege);
        assert_eq!(parsed.working_directory.as_deref(), Some(r"C:\sol"));
    }

    #[test]
    fn native_scheduler_implicit_limited_form_is_exact_and_fails_closed_on_variants() {
        let lease = lease();
        let arguments = format!(
            "limited-child --lease &quot;{LEASE_PATH}&quot; --nonce {}",
            lease.nonce
        );
        let xml = format!(
            "<Task><Principals><Principal><UserId>{}</UserId><LogonType>InteractiveToken</LogonType></Principal></Principals><Actions><Exec><Command>\"{}\"</Command><Arguments>{arguments}</Arguments></Exec></Actions></Task>",
            lease.expected_owner_sid, lease.worker_path,
        );
        let parsed = parse_task_xml(&xml).expect("captures native omitted RunLevel form");
        assert_eq!(parsed.run_level, TaskRunLevel::ImplicitLeastPrivilege);
        assert_eq!(parsed.working_directory, None);
        assert_eq!(verify_task_definition(&lease, &parsed), Ok(()));

        for rejected in [
            xml.replacen("InteractiveToken", "InteractiveTokenOrPassword", 1),
            xml.replacen(
                "</Principal>",
                "<RunLevel>HighestAvailable</RunLevel></Principal>",
                1,
            ),
            xml.replacen(
                "</Exec>",
                "</Exec><ComHandler><ClassId>x</ClassId></ComHandler>",
                1,
            ),
            xml.replacen(
                "</Exec>",
                "<Exec><Command>x</Command><Arguments>x</Arguments></Exec></Actions>",
                1,
            ),
            xml.replacen(
                "</Exec>",
                "<WorkingDirectory> </WorkingDirectory></Exec>",
                1,
            ),
            xml.replacen(
                "</Task>",
                "<Unexpected><UserId>wrong</UserId></Unexpected></Task>",
                1,
            ),
            xml.replacen(
                &format!("\"{}\"", lease.worker_path),
                &format!("\"{}\"\"", lease.worker_path),
                1,
            ),
        ] {
            let parsed = parse_task_xml(&rejected);
            assert!(
                parsed.is_err()
                    || verify_task_definition(&lease, &parsed.expect("parsed")).is_err()
            );
        }
    }

    #[test]
    fn csv_runtime_parser_and_terminal_classifier_reject_stale_false_green() {
        let csv = "\"HostName\",\"TaskName\",\"Next Run Time\",\"Status\",\"Logon Mode\",\"Last Run Time\",\"Last Result\"\r\n\"HOST\",\"\\solstone\\journal\\ordinary-owner-abc\",\"12/31/2099 23:59:00\",\"Ready\",\"Interactive only\",\"8/26/2026 09:00:00\",\"0x0\"\r\n";
        let runtime = parse_task_runtime_csv(csv).expect("parses verbose CSV fixture");
        assert_eq!(
            classify_terminal(&runtime, Some("8/26/2026 09:00:00")),
            TerminalState::Pending
        );
        assert_eq!(
            classify_terminal(&runtime, Some("8/26/2026 08:00:00")),
            TerminalState::Verified
        );
        let stale = TaskRuntime {
            status: "Ready".to_owned(),
            last_run_time: "N/A".to_owned(),
            last_task_result: "0".to_owned(),
        };
        assert_eq!(classify_terminal(&stale, None), TerminalState::Pending);
        let failed = TaskRuntime {
            status: "Ready".to_owned(),
            last_run_time: "8/26/2026 09:01:00".to_owned(),
            last_task_result: "0x1".to_owned(),
        };
        assert_eq!(classify_terminal(&failed, None), TerminalState::Rejected);
    }

    #[test]
    fn result_verification_rejects_control_identity_and_nonce_mismatches() {
        let lease = lease();
        let mut result = passed_result(&lease);
        assert_eq!(verify_result(&lease, &result), Ok(()));
        result.selected_control = Some("windows-cloud-sync.v1".to_owned());
        assert_eq!(
            verify_result(&lease, &result),
            Err(RailError::new("selected-alternate-control"))
        );
        result = passed_result(&lease);
        result.executed_control = None;
        assert_eq!(
            verify_result(&lease, &result),
            Err(RailError::new("ignored-unselected-control"))
        );
        result = passed_result(&lease);
        result.nonce.push('x');
        assert_eq!(
            verify_result(&lease, &result),
            Err(RailError::new("result-nonce-mismatch"))
        );
    }

    #[test]
    fn result_verification_rejects_token_and_payload_failures() {
        let lease = lease();
        let mut result = passed_result(&lease);
        result.before.elevated = true;
        assert_eq!(
            verify_result(&lease, &result),
            Err(RailError::new("token-elevated"))
        );
        result = passed_result(&lease);
        result.before.backup_privilege_present = true;
        assert_eq!(
            verify_result(&lease, &result),
            Err(RailError::new("token-backup-privilege-present"))
        );
        result = passed_result(&lease);
        result.before.restore_privilege_present = true;
        assert_eq!(
            verify_result(&lease, &result),
            Err(RailError::new("token-restore-privilege-present"))
        );
        result = passed_result(&lease);
        result.after.owner_sid.push('x');
        assert_eq!(
            verify_result(&lease, &result),
            Err(RailError::new("owner-sid-mismatch"))
        );
        result = passed_result(&lease);
        result.after.session = 2;
        assert_eq!(
            verify_result(&lease, &result),
            Err(RailError::new("session-mismatch"))
        );
        result = passed_result(&lease);
        result.payload_sha256.push('x');
        assert_eq!(
            verify_result(&lease, &result),
            Err(RailError::new("result-payload-mismatch"))
        );
    }

    #[test]
    fn dirty_tracked_and_untracked_trees_fail_closed() {
        assert_eq!(require_clean_worktree(""), Ok(()));
        assert_eq!(
            require_clean_worktree(" M core/Cargo.lock\n"),
            Err(RailError::new("worktree-dirty"))
        );
        assert_eq!(
            require_clean_worktree("?? scratch.txt\n"),
            Err(RailError::new("worktree-dirty"))
        );
    }

    #[test]
    fn held_lease_recovery_only_reclaims_terminal_or_missing_tasks() {
        let lease = lease();
        assert_eq!(
            recovery_decision(None, false, false),
            RecoveryDecision::NoHeldLease
        );
        assert_eq!(
            recovery_decision(Some(&lease), true, true),
            RecoveryDecision::CleanThenReclaim
        );
        assert_eq!(
            recovery_decision(Some(&lease), false, false),
            RecoveryDecision::CleanThenReclaim
        );
        assert_eq!(
            recovery_decision(Some(&lease), true, false),
            RecoveryDecision::FailClosed
        );
    }

    const PROBE_SID: &str = "S-1-5-21-100";
    const PROBE_NONCE: &str = "ABCDEFGH23456789ABCDEFGH23456789";
    const PROBE_EXE: &str = r"C:\probe.exe";

    fn probe_marker_value() -> String {
        probe_marker(PROBE_NONCE)
    }

    fn probe_definition() -> ProbeTaskDefinition {
        ProbeTaskDefinition {
            principal_sid: PROBE_SID.to_owned(),
            logon_type: "InteractiveToken".to_owned(),
            run_level: TaskRunLevel::ExplicitLeastPrivilege,
            command: PROBE_EXE.to_owned(),
            arguments: probe_marker_value(),
            working_directory: None,
            description: Some(probe_marker_value()),
            trigger_user_id: PROBE_SID.to_owned(),
            priority: ProbePriority::Present(7),
        }
    }

    fn probe_verify_input() -> ProbeVerifyInput {
        ProbeVerifyInput {
            expected_sid: PROBE_SID.to_owned(),
            expected_command: PROBE_EXE.to_owned(),
            marker: probe_marker_value(),
        }
    }

    fn probe_error(diagnostic: &str) -> ProbeApiError {
        ProbeApiError {
            space: ProbeApiErrorSpace::Win32,
            code: 5,
            diagnostic: diagnostic.to_owned(),
        }
    }

    fn base_probe_report(token: ProbeTokenSection) -> OnlogonProbeReport {
        OnlogonProbeReport {
            schema: PROBE_SCHEMA.to_owned(),
            outcome: ProbeOutcome::DefinitionUnverified,
            nonce: PROBE_NONCE.to_owned(),
            marker: probe_marker_value(),
            xml_task_name: probe_xml_task_name(PROBE_NONCE),
            com_folder_path: probe_com_folder_path(PROBE_NONCE),
            token,
            xml: ProbeXmlSection::skipped(),
            com: ProbeComSection::skipped(),
        }
    }

    fn default_token_passed() -> ProbeTokenSection {
        ProbeTokenSection {
            elevation_type: TokenElevationClass::Default,
            elevated: false,
            session: 1,
            owner_sid: PROBE_SID.to_owned(),
            stage: ProbeStage::passed(),
        }
    }

    fn serialize_outcome(report: &OnlogonProbeReport) -> serde_json::Value {
        serde_json::to_value(report).expect("report serializes")
    }

    fn assert_stage_resolution_invariants(report: &OnlogonProbeReport) {
        let stages = [
            &report.token.stage,
            &report.xml.create,
            &report.xml.definition,
            &report.xml.cleanup,
            &report.com.folder_create,
            &report.com.register,
            &report.com.definition,
            &report.com.cleanup,
        ];
        for stage in stages {
            if stage_requires_resolution_action(stage) {
                assert!(stage.resolution_action.is_some());
            } else {
                assert!(stage.resolution_action.is_none());
            }
        }
        let acl = &report.com.folder_acl;
        if matches!(
            acl.status,
            ProbeStageStatus::Failed | ProbeStageStatus::Inconclusive
        ) {
            assert!(acl.resolution_action.is_some());
        } else {
            assert!(acl.resolution_action.is_none());
        }
    }

    #[test]
    fn probe_xml_parser_accepts_onlogon_fixture() {
        let parsed = parse_probe_task_xml(&render_probe_task_xml(&probe_definition()))
            .expect("parses probe fixture");
        assert_eq!(parsed.principal_sid, PROBE_SID);
        assert_eq!(parsed.trigger_user_id, PROBE_SID);
        assert_eq!(parsed.arguments, probe_marker_value());
        assert_eq!(
            parsed.description.as_deref(),
            Some(probe_marker_value().as_str())
        );
        assert_eq!(parsed.priority, ProbePriority::Present(7));
        assert_eq!(parsed.run_level, TaskRunLevel::ExplicitLeastPrivilege);
    }

    #[test]
    fn render_probe_task_xml_emits_task_namespace_and_schema_version() {
        let xml = render_probe_task_xml(&probe_definition());
        assert!(xml.starts_with(r#"<?xml version="1.0" encoding="UTF-8"?>"#));
        assert!(xml.contains(
            r#"<Task xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task" version="1.2">"#
        ));
        assert!(xml.ends_with("</Task>"));
    }

    #[test]
    fn render_probe_task_xml_omits_priority_when_unspecified() {
        let mut definition = probe_definition();
        definition.priority = ProbePriority::Unavailable;
        let xml = render_probe_task_xml(&definition);
        assert!(!xml.contains("<Priority>"));
        assert!(!xml.contains("<Settings>"));
    }

    #[test]
    fn probe_xml_parser_rejects_non_logon_trigger() {
        let xml = render_probe_task_xml(&probe_definition())
            .replacen("<LogonTrigger>", "<TimeTrigger>", 1)
            .replacen("</LogonTrigger>", "</TimeTrigger>", 1);
        assert_eq!(
            parse_probe_task_xml(&xml),
            Err(RailError::new("task-xml-trigger-set-mismatch"))
        );
    }

    #[test]
    fn probe_xml_parser_rejects_extra_exec_or_comhandler() {
        let xml = render_probe_task_xml(&probe_definition()).replacen(
            "</Exec>",
            "</Exec><ComHandler><ClassId>x</ClassId></ComHandler>",
            1,
        );
        assert_eq!(
            parse_probe_task_xml(&xml),
            Err(RailError::new("task-xml-action-set-mismatch"))
        );
    }

    #[test]
    fn probe_xml_parser_rejects_userid_outside_principal_or_logon_trigger() {
        let xml = render_probe_task_xml(&probe_definition()).replacen(
            "</Task>",
            "<Unexpected><UserId>wrong</UserId></Unexpected></Task>",
            1,
        );
        assert_eq!(
            parse_probe_task_xml(&xml),
            Err(RailError::new("task-xml-security-field-location-mismatch"))
        );
    }

    #[test]
    fn probe_xml_verify_accepts_bound_marker_and_sids() {
        assert_eq!(
            verify_probe_task_definition(&probe_definition(), &probe_verify_input()),
            Ok(())
        );
    }

    #[test]
    fn probe_xml_verify_rejects_principal_sid_mismatch() {
        let mut definition = probe_definition();
        definition.principal_sid.push('x');
        assert_eq!(
            verify_probe_task_definition(&definition, &probe_verify_input()),
            Err(RailError::new("task-principal-sid-mismatch"))
        );
    }

    #[test]
    fn probe_xml_verify_rejects_trigger_sid_mismatch() {
        let mut definition = probe_definition();
        definition.trigger_user_id.push('x');
        assert_eq!(
            verify_probe_task_definition(&definition, &probe_verify_input()),
            Err(RailError::new("task-trigger-sid-mismatch"))
        );
    }

    #[test]
    fn probe_xml_verify_rejects_logon_type_not_interactive() {
        let mut definition = probe_definition();
        definition.logon_type = "Password".to_owned();
        assert_eq!(
            verify_probe_task_definition(&definition, &probe_verify_input()),
            Err(RailError::new("task-logon-type-mismatch"))
        );
    }

    #[test]
    fn probe_xml_verify_rejects_run_level_highest() {
        let xml = render_probe_task_xml(&probe_definition()).replacen(
            "<RunLevel>LeastPrivilege</RunLevel>",
            "<RunLevel>HighestAvailable</RunLevel>",
            1,
        );
        assert_eq!(
            parse_probe_task_xml(&xml),
            Err(RailError::new("task-run-level-mismatch"))
        );
    }

    #[test]
    fn probe_xml_verify_rejects_command_mismatch() {
        let mut definition = probe_definition();
        definition.command.push('x');
        assert_eq!(
            verify_probe_task_definition(&definition, &probe_verify_input()),
            Err(RailError::new("task-command-or-nonce-mismatch"))
        );
    }

    #[test]
    fn probe_xml_verify_classifies_missing_description_as_marker_missing() {
        let mut definition = probe_definition();
        definition.description = None;
        assert_eq!(
            verify_probe_task_definition(&definition, &probe_verify_input()),
            Err(RailError::new("marker-missing"))
        );
    }

    #[test]
    fn probe_xml_verify_classifies_wrong_description_as_marker_mismatch() {
        let mut definition = probe_definition();
        definition.description = Some("other".to_owned());
        assert_eq!(
            verify_probe_task_definition(&definition, &probe_verify_input()),
            Err(RailError::new("marker-mismatch"))
        );
    }

    #[test]
    fn probe_xml_verify_rejects_argument_marker_mismatch() {
        let mut definition = probe_definition();
        definition.arguments.push('x');
        assert_eq!(
            verify_probe_task_definition(&definition, &probe_verify_input()),
            Err(RailError::new("task-command-or-nonce-mismatch"))
        );
    }

    #[test]
    fn probe_priority_present_value() {
        assert_eq!(
            parse_probe_task_xml(&render_probe_task_xml(&probe_definition()))
                .expect("parses")
                .priority,
            ProbePriority::Present(7)
        );
    }

    #[test]
    fn probe_priority_absent_is_unavailable() {
        let mut definition = probe_definition();
        definition.priority = ProbePriority::Unavailable;
        assert_eq!(
            parse_probe_task_xml(&render_probe_task_xml(&definition))
                .expect("parses")
                .priority,
            ProbePriority::Unavailable
        );
    }

    #[test]
    fn probe_priority_malformed_text_or_range() {
        let xml = render_probe_task_xml(&probe_definition()).replacen(
            "<Priority>7</Priority>",
            "<Priority>high</Priority>",
            1,
        );
        assert_eq!(
            parse_probe_task_xml(&xml).expect("parses").priority,
            ProbePriority::Malformed
        );
        let xml = render_probe_task_xml(&probe_definition()).replacen(
            "<Priority>7</Priority>",
            "<Priority>11</Priority>",
            1,
        );
        assert_eq!(
            parse_probe_task_xml(&xml).expect("parses").priority,
            ProbePriority::Malformed
        );
    }

    #[test]
    fn priority_from_raw_value_accepts_scheduler_range() {
        assert_eq!(priority_from_raw_value(0), ProbePriority::Present(0));
        assert_eq!(priority_from_raw_value(7), ProbePriority::Present(7));
        assert_eq!(priority_from_raw_value(10), ProbePriority::Present(10));
        assert_eq!(priority_from_raw_value(-1), ProbePriority::Malformed);
        assert_eq!(priority_from_raw_value(11), ProbePriority::Malformed);
    }

    #[test]
    fn probe_priority_preserved_when_verify_fails() {
        let parsed = parse_probe_task_xml(&render_probe_task_xml(&probe_definition()))
            .expect("parses probe fixture");
        let mut expected = probe_verify_input();
        expected.expected_sid.push('x');
        assert_eq!(
            verify_probe_task_definition(&parsed, &expected),
            Err(RailError::new("task-principal-sid-mismatch"))
        );
        assert_eq!(parsed.priority, ProbePriority::Present(7));
        assert_eq!(
            ProbePriorityView::from(parsed.priority),
            ProbePriorityView {
                state: ProbePriorityState::Present,
                value: Some(7),
            }
        );
    }

    #[test]
    fn probe_priority_view_json_states() {
        let present = serde_json::to_value(ProbePriorityView::from(ProbePriority::Present(7)))
            .expect("present");
        assert_eq!(present["state"], "present");
        assert_eq!(present["value"], 7);
        let unavailable = serde_json::to_value(ProbePriorityView::from(ProbePriority::Unavailable))
            .expect("absent");
        assert_eq!(unavailable["state"], "unavailable");
        assert_eq!(unavailable["value"], serde_json::Value::Null);
        let malformed = serde_json::to_value(ProbePriorityView::from(ProbePriority::Malformed))
            .expect("malformed");
        assert_eq!(malformed["state"], "malformed");
        assert_eq!(malformed["value"], serde_json::Value::Null);
    }

    #[test]
    fn token_class_maps_microsoft_values() {
        assert_eq!(
            classify_token_elevation_type(1),
            TokenElevationClass::Default
        );
        assert_eq!(classify_token_elevation_type(2), TokenElevationClass::Full);
        assert_eq!(
            classify_token_elevation_type(3),
            TokenElevationClass::Limited
        );
        assert_eq!(
            classify_token_elevation_type(0),
            TokenElevationClass::Unknown
        );
        assert_eq!(
            classify_token_elevation_type(4),
            TokenElevationClass::Unknown
        );
    }

    #[test]
    fn token_gate_stops_full_and_limited_continues_default() {
        assert_eq!(
            probe_token_gate(TokenElevationClass::Full, true),
            Ok(ProbeTokenGate::StopElevated)
        );
        assert_eq!(
            probe_token_gate(TokenElevationClass::Limited, false),
            Ok(ProbeTokenGate::StopFilteredAdmin)
        );
        assert_eq!(
            probe_token_gate(TokenElevationClass::Default, false),
            Ok(ProbeTokenGate::ContinueUnelevated)
        );
        assert_eq!(
            probe_token_gate(TokenElevationClass::Unknown, false),
            Ok(ProbeTokenGate::StopElevated)
        );
    }

    #[test]
    fn token_gate_rejects_full_without_elevated_flag() {
        assert_eq!(
            probe_token_gate(TokenElevationClass::Full, false),
            Err(RailError::new("token-elevation-inconsistent"))
        );
        assert_eq!(
            probe_token_gate(TokenElevationClass::Limited, true),
            Err(RailError::new("token-elevation-inconsistent"))
        );
        assert_eq!(
            probe_token_gate(TokenElevationClass::Default, true),
            Err(RailError::new("token-elevation-inconsistent"))
        );
    }

    #[test]
    fn select_outcome_elevated_and_filtered_admin_from_token_stage() {
        let mut elevated = base_probe_report(ProbeTokenSection {
            elevation_type: TokenElevationClass::Full,
            elevated: true,
            session: 1,
            owner_sid: PROBE_SID.to_owned(),
            stage: ProbeStage::inconclusive(
                probe_error("token-elevation-type-full"),
                ProbeResolutionAction::UseStandardLogon,
            ),
        });
        elevated.outcome = select_probe_outcome(&elevated);
        assert_eq!(elevated.outcome, ProbeOutcome::ElevatedCreator);
        assert_eq!(elevated.token.stage.status, ProbeStageStatus::Inconclusive);
        let mut filtered = base_probe_report(ProbeTokenSection {
            elevation_type: TokenElevationClass::Limited,
            elevated: false,
            session: 1,
            owner_sid: PROBE_SID.to_owned(),
            stage: ProbeStage::inconclusive(
                probe_error("token-elevation-type-limited"),
                ProbeResolutionAction::StopFilteredAdmin,
            ),
        });
        filtered.outcome = select_probe_outcome(&filtered);
        assert_eq!(filtered.outcome, ProbeOutcome::FilteredAdministratorCreator);
        assert_eq!(filtered.token.stage.status, ProbeStageStatus::Inconclusive);
    }

    #[test]
    fn report_json_elevated_creator() {
        let mut report = base_probe_report(ProbeTokenSection {
            elevation_type: TokenElevationClass::Full,
            elevated: true,
            session: 1,
            owner_sid: PROBE_SID.to_owned(),
            stage: ProbeStage::inconclusive(
                probe_error("token-elevation-type-full"),
                ProbeResolutionAction::UseStandardLogon,
            ),
        });
        report.outcome = select_probe_outcome(&report);
        let json = serialize_outcome(&report);
        assert_eq!(json["outcome"], "elevated-creator");
        assert_eq!(json["token"]["elevation_type"], "full");
        assert_eq!(json["token"]["stage"]["status"], "inconclusive");
        assert_eq!(json["xml"]["create"]["status"], "skipped");
        assert!(probe_exits_successfully(&report));
    }

    #[test]
    fn report_json_filtered_administrator_creator() {
        let mut report = base_probe_report(ProbeTokenSection {
            elevation_type: TokenElevationClass::Limited,
            elevated: false,
            session: 1,
            owner_sid: PROBE_SID.to_owned(),
            stage: ProbeStage::inconclusive(
                probe_error("token-elevation-type-limited"),
                ProbeResolutionAction::StopFilteredAdmin,
            ),
        });
        report.outcome = select_probe_outcome(&report);
        let json = serialize_outcome(&report);
        assert_eq!(json["outcome"], "filtered-administrator-creator");
        assert_eq!(json["token"]["elevation_type"], "limited");
        assert_eq!(json["token"]["stage"]["status"], "inconclusive");
        assert!(probe_exits_successfully(&report));
    }

    #[test]
    fn report_json_registration_refused() {
        let mut report = base_probe_report(default_token_passed());
        report.xml.create = ProbeStage::failed(
            probe_error("access denied"),
            ProbeResolutionAction::TreatAsRefused,
        );
        report.com.folder_create = ProbeStage::failed(
            probe_error("access denied"),
            ProbeResolutionAction::TreatAsRefused,
        );
        report.outcome = select_probe_outcome(&report);
        let json = serialize_outcome(&report);
        assert_eq!(json["outcome"], "registration-refused");
        assert!(probe_exits_successfully(&report));
    }

    #[test]
    fn select_outcome_com_transport_uncertainty_is_not_refused() {
        let mut report = base_probe_report(default_token_passed());
        report.xml.create = ProbeStage::failed(
            probe_error("access denied"),
            ProbeResolutionAction::TreatAsRefused,
        );
        report.com.folder_create = ProbeStage::inconclusive(
            probe_error("ITaskService::Connect RPC failure"),
            ProbeResolutionAction::RepairSchedulerAccess,
        );
        report.outcome = select_probe_outcome(&report);
        assert_ne!(report.outcome, ProbeOutcome::RegistrationRefused);
        assert_eq!(report.outcome, ProbeOutcome::DefinitionUnverified);
        assert!(!probe_exits_successfully(&report));
    }

    #[test]
    fn report_json_verified_registration() {
        let mut report = base_probe_report(default_token_passed());
        report.xml.create = ProbeStage::passed();
        report.xml.definition = ProbeStage::passed();
        report.xml.cleanup = ProbeStage::passed();
        report.xml.priority = ProbePriority::Present(7).into();
        report.xml.create_argv =
            schtasks_create_xml_argv(r"\solstone\probe\onlogon-xml-abc", r"C:\a.xml");
        report.xml.definition_sha256 = "abc123".to_owned();
        report.com.folder_create = ProbeStage::passed();
        report.com.folder_acl.status = ProbeStageStatus::Passed;
        report.com.folder_acl.ordinary_mutation = Some(false);
        report.com.folder_acl.descriptor_source = Some(ProbeDescriptorSource::NamedInfo);
        report.com.register = ProbeStage::passed();
        report.com.definition = ProbeStage::passed();
        report.com.cleanup = ProbeStage::passed();
        report.com.register_flag = com_register_flag();
        report.com.call_sequence = vec![
            "ITaskService::Connect".to_owned(),
            "ITaskFolder::RegisterTaskDefinition".to_owned(),
        ];
        report.outcome = select_probe_outcome(&report);
        let json = serialize_outcome(&report);
        assert_eq!(json["outcome"], "verified-registration");
        assert_eq!(json["xml"]["priority"]["state"], "present");
        assert_eq!(json["xml"]["create_argv"][0], "/Create");
        assert_eq!(json["xml"]["definition_sha256"], "abc123");
        assert_eq!(json["com"]["register_flag"], com_register_flag());
        assert_eq!(json["com"]["call_sequence"][0], "ITaskService::Connect");
        assert!(probe_exits_successfully(&report));
    }

    #[test]
    fn report_json_definition_unverified() {
        let mut report = base_probe_report(default_token_passed());
        report.xml.create = ProbeStage::passed();
        report.xml.definition = ProbeStage::failed(
            ProbeApiError {
                space: ProbeApiErrorSpace::Schtasks,
                code: 0,
                diagnostic: "marker-mismatch".to_owned(),
            },
            ProbeResolutionAction::InspectDefinition,
        );
        report.xml.cleanup = ProbeStage::passed();
        report.outcome = select_probe_outcome(&report);
        let json = serialize_outcome(&report);
        assert_eq!(json["outcome"], "definition-unverified");
        assert!(!probe_exits_successfully(&report));
    }

    #[test]
    fn report_json_cleanup_uncertain() {
        let mut report = base_probe_report(default_token_passed());
        report.xml.create = ProbeStage::passed();
        report.xml.definition = ProbeStage::passed();
        report.xml.cleanup = ProbeStage::inconclusive(
            probe_error("delete did not prove absence"),
            ProbeResolutionAction::ManualCleanup,
        );
        report.outcome = select_probe_outcome(&report);
        let json = serialize_outcome(&report);
        assert_eq!(json["outcome"], "cleanup-uncertain");
        assert!(!probe_exits_successfully(&report));
    }

    #[test]
    fn probe_names_are_nonce_fresh_and_disjoint_from_ordinary_owner() {
        let xml = probe_xml_task_name(PROBE_NONCE);
        let com = probe_com_folder_path(PROBE_NONCE);
        let ordinary = task_name(PROBE_NONCE);
        assert!(xml.starts_with(r"\solstone\probe\onlogon-xml-"));
        assert!(com.starts_with(r"\solstone\probe\onlogon-com-"));
        assert_eq!(probe_com_task_name(), "probe");
        assert_ne!(xml, com);
        assert!(!xml.contains(r"\journal\"));
        assert!(!com.contains(r"\journal\"));
        assert!(ordinary.starts_with(r"\solstone\journal\ordinary-owner-"));
        assert_ne!(xml, ordinary);
        assert_ne!(com, ordinary);
        assert_ne!(probe_xml_task_name("OTHERNONCE23456789ABCDEFGH234567"), xml);
    }

    #[test]
    fn schtasks_create_xml_argv_has_no_force_flag() {
        let argv = schtasks_create_xml_argv(r"\solstone\probe\onlogon-xml-abc", r"C:\a.xml");
        assert!(argv.windows(2).any(|pair| pair == ["/Create", "/TN"]));
        assert!(argv.windows(2).any(|pair| pair == ["/XML", r"C:\a.xml"]));
        assert!(!argv.iter().any(|arg| arg == "/F"));
    }

    #[test]
    fn com_register_flag_is_create_or_update() {
        assert_eq!(com_register_flag(), TASK_CREATION_CREATE_OR_UPDATE);
        assert_eq!(com_register_flag(), 6);
        assert_ne!(com_register_flag(), TASK_CREATION_CREATE);
        assert_ne!(com_register_flag(), 2);
    }

    #[test]
    fn select_outcome_collision_is_not_verified() {
        let mut report = base_probe_report(default_token_passed());
        report.xml.create = ProbeStage::failed(
            probe_error("already exists"),
            ProbeResolutionAction::RetryFreshNonce,
        );
        report.com.folder_create = ProbeStage::failed(
            probe_error("already exists"),
            ProbeResolutionAction::RetryFreshNonce,
        );
        report.outcome = select_probe_outcome(&report);
        assert_ne!(report.outcome, ProbeOutcome::VerifiedRegistration);
        assert_eq!(report.outcome, ProbeOutcome::DefinitionUnverified);
        assert!(!probe_exits_successfully(&report));
    }

    #[test]
    fn select_outcome_single_route_verified_is_not_verified() {
        let mut report = base_probe_report(default_token_passed());
        report.xml.create = ProbeStage::passed();
        report.xml.definition = ProbeStage::passed();
        report.xml.cleanup = ProbeStage::passed();
        report.xml.priority = ProbePriority::Present(7).into();
        report.xml.create_argv =
            schtasks_create_xml_argv(r"\solstone\probe\onlogon-xml-abc", r"C:\a.xml");
        report.xml.definition_sha256 = "def456".to_owned();
        report.outcome = select_probe_outcome(&report);
        assert_ne!(report.outcome, ProbeOutcome::VerifiedRegistration);
        assert_eq!(report.outcome, ProbeOutcome::DefinitionUnverified);
        let json = serialize_outcome(&report);
        assert_eq!(json["xml"]["create"]["status"], "passed");
        assert_eq!(json["xml"]["definition"]["status"], "passed");
        assert_eq!(json["xml"]["priority"]["state"], "present");
        assert_eq!(json["xml"]["priority"]["value"], 7);
        assert_eq!(json["xml"]["create_argv"][0], "/Create");
        assert_eq!(json["xml"]["definition_sha256"], "def456");
        assert_eq!(json["com"]["register"]["status"], "skipped");
        assert!(!probe_exits_successfully(&report));
    }

    #[test]
    fn select_outcome_xml_inspect_failure_blocks_refusal() {
        let mut report = base_probe_report(default_token_passed());
        report.xml.create = ProbeStage::failed(
            probe_error("implementation parse failure"),
            ProbeResolutionAction::InspectDefinition,
        );
        report.com.folder_create = ProbeStage::failed(
            probe_error("access denied"),
            ProbeResolutionAction::TreatAsRefused,
        );
        report.outcome = select_probe_outcome(&report);
        assert_ne!(report.outcome, ProbeOutcome::RegistrationRefused);
        assert_eq!(report.outcome, ProbeOutcome::DefinitionUnverified);
        assert!(!probe_exits_successfully(&report));
    }

    #[test]
    fn classify_sid_exempts_caller_system_administrators() {
        assert_eq!(
            classify_probe_sid("S-1-5-21-100", "S-1-5-21-100"),
            ProbeSidClass::Caller
        );
        assert_eq!(
            classify_probe_sid("s-1-5-18", "S-1-5-21-100"),
            ProbeSidClass::LocalSystem
        );
        assert_eq!(
            classify_probe_sid("S-1-5-19", "S-1-5-21-100"),
            ProbeSidClass::LocalService
        );
        assert_eq!(
            classify_probe_sid("S-1-5-20", "S-1-5-21-100"),
            ProbeSidClass::NetworkService
        );
        assert_eq!(
            classify_probe_sid("S-1-5-32-544", "S-1-5-21-100"),
            ProbeSidClass::Administrators
        );
    }

    #[test]
    fn classify_sid_marks_users_authenticated_everyone_interactive_ordinary() {
        assert_eq!(
            classify_probe_sid("S-1-5-32-545", "S-1-5-21-100"),
            ProbeSidClass::OrdinaryGroup
        );
        assert_eq!(
            classify_probe_sid("S-1-5-11", "S-1-5-21-100"),
            ProbeSidClass::OrdinaryGroup
        );
        assert_eq!(
            classify_probe_sid("S-1-1-0", "S-1-5-21-100"),
            ProbeSidClass::OrdinaryGroup
        );
        assert_eq!(
            classify_probe_sid("S-1-5-4", "S-1-5-21-100"),
            ProbeSidClass::OrdinaryGroup
        );
        assert_eq!(
            classify_probe_sid("S-1-5-21-999", "S-1-5-21-100"),
            ProbeSidClass::OrdinaryDistinctUser
        );
    }

    #[test]
    fn folder_ordinary_allow_generic_all_is_non_isolatable() {
        let entries = [ProbeAclEntry {
            sid: "S-1-5-32-545".to_owned(),
            allow: true,
            access_mask: 0x1000_0000,
            classification: ProbeSidClass::OrdinaryGroup,
        }];
        assert!(folder_allows_ordinary_mutation(&entries));
    }

    #[test]
    fn folder_only_caller_system_admin_is_isolatable() {
        let entries = [
            ProbeAclEntry {
                sid: "S-1-5-21-100".to_owned(),
                allow: true,
                access_mask: ACE_MUTATION_MASK,
                classification: ProbeSidClass::Caller,
            },
            ProbeAclEntry {
                sid: "S-1-5-18".to_owned(),
                allow: true,
                access_mask: ACE_MUTATION_MASK,
                classification: ProbeSidClass::LocalSystem,
            },
            ProbeAclEntry {
                sid: "S-1-5-32-544".to_owned(),
                allow: true,
                access_mask: ACE_MUTATION_MASK,
                classification: ProbeSidClass::Administrators,
            },
        ];
        assert!(!folder_allows_ordinary_mutation(&entries));
    }

    #[test]
    fn normalize_acl_canonicalizes_sid_case_and_sorts() {
        let entries = [
            ProbeAclEntry {
                sid: "s-1-5-32-544".to_owned(),
                allow: true,
                access_mask: 1,
                classification: ProbeSidClass::Administrators,
            },
            ProbeAclEntry {
                sid: "S-1-5-18".to_owned(),
                allow: true,
                access_mask: 1,
                classification: ProbeSidClass::LocalSystem,
            },
        ];
        let normalized = normalize_probe_acl_entries(&entries);
        assert_eq!(normalized[0].sid, "S-1-5-18");
        assert_eq!(normalized[1].sid, "S-1-5-32-544");
        assert_eq!(normalized[1].classification, ProbeSidClass::Administrators);
    }

    #[test]
    fn select_outcome_folder_acl_inconclusive_without_descriptor() {
        let mut report = base_probe_report(default_token_passed());
        report.xml.create = ProbeStage::passed();
        report.xml.definition = ProbeStage::passed();
        report.xml.cleanup = ProbeStage::passed();
        report.com.folder_create = ProbeStage::passed();
        report.com.folder_acl = ProbeFolderAclStage {
            status: ProbeStageStatus::Inconclusive,
            error: Some(probe_error("named-info access denied")),
            resolution_action: Some(ProbeResolutionAction::RepairSchedulerAccess),
            descriptor_source: None,
            sddl: None,
            ordinary_mutation: None,
        };
        report.com.register = ProbeStage::passed();
        report.com.definition = ProbeStage::passed();
        report.com.cleanup = ProbeStage::passed();
        report.outcome = select_probe_outcome(&report);
        assert_ne!(report.outcome, ProbeOutcome::VerifiedRegistration);
        assert_eq!(report.outcome, ProbeOutcome::DefinitionUnverified);
    }

    #[test]
    fn select_outcome_ordinary_mutation_sets_isolate_folder_acl_action() {
        let mut report = base_probe_report(default_token_passed());
        report.xml.create = ProbeStage::passed();
        report.xml.definition = ProbeStage::passed();
        report.xml.cleanup = ProbeStage::passed();
        report.com.folder_create = ProbeStage::passed();
        report.com.folder_acl = ProbeFolderAclStage {
            status: ProbeStageStatus::Failed,
            error: Some(probe_error("ordinary mutation")),
            resolution_action: Some(ProbeResolutionAction::IsolateFolderAcl),
            descriptor_source: Some(ProbeDescriptorSource::ComSddl),
            sddl: Some("D:(A;;FA;;;BU)".to_owned()),
            ordinary_mutation: Some(true),
        };
        report.com.register = ProbeStage::passed();
        report.com.definition = ProbeStage::passed();
        report.com.cleanup = ProbeStage::passed();
        report.outcome = select_probe_outcome(&report);
        assert_eq!(
            report.com.folder_acl.resolution_action,
            Some(ProbeResolutionAction::IsolateFolderAcl)
        );
        assert_eq!(report.outcome, ProbeOutcome::VerifiedRegistration);
    }

    #[test]
    fn failed_or_inconclusive_stage_requires_resolution_action() {
        let mut report = base_probe_report(ProbeTokenSection {
            elevation_type: TokenElevationClass::Full,
            elevated: true,
            session: 1,
            owner_sid: PROBE_SID.to_owned(),
            stage: ProbeStage::inconclusive(
                probe_error("token-elevation-type-full"),
                ProbeResolutionAction::UseStandardLogon,
            ),
        });
        report.outcome = select_probe_outcome(&report);
        assert_stage_resolution_invariants(&report);
        let mut refused = base_probe_report(default_token_passed());
        refused.xml.create = ProbeStage::failed(
            probe_error("access denied"),
            ProbeResolutionAction::TreatAsRefused,
        );
        refused.com.folder_acl = ProbeFolderAclStage {
            status: ProbeStageStatus::Inconclusive,
            error: Some(probe_error("no descriptor")),
            resolution_action: Some(ProbeResolutionAction::RepairSchedulerAccess),
            descriptor_source: None,
            sddl: None,
            ordinary_mutation: None,
        };
        refused.outcome = select_probe_outcome(&refused);
        assert_stage_resolution_invariants(&refused);
    }

    #[test]
    fn passed_and_skipped_stages_have_null_resolution_action() {
        let mut report = base_probe_report(default_token_passed());
        report.xml.create = ProbeStage::passed();
        report.xml.definition = ProbeStage::passed();
        report.xml.cleanup = ProbeStage::passed();
        report.com.folder_create = ProbeStage::passed();
        report.com.folder_acl.status = ProbeStageStatus::Passed;
        report.com.register = ProbeStage::passed();
        report.com.definition = ProbeStage::passed();
        report.com.cleanup = ProbeStage::passed();
        report.outcome = select_probe_outcome(&report);
        assert_stage_resolution_invariants(&report);
        let skipped = base_probe_report(ProbeTokenSection {
            elevation_type: TokenElevationClass::Full,
            elevated: true,
            session: 1,
            owner_sid: PROBE_SID.to_owned(),
            stage: ProbeStage::inconclusive(
                probe_error("token-elevation-type-full"),
                ProbeResolutionAction::UseStandardLogon,
            ),
        });
        assert_stage_resolution_invariants(&skipped);
        assert!(skipped.xml.create.resolution_action.is_none());
        assert!(skipped.com.register.resolution_action.is_none());
    }

    #[test]
    fn probe_receipt_finalize_transitions_incomplete_to_terminal() {
        let mut receipt = ProbeReceipt::incomplete(PROBE_NONCE, &probe_marker_value());
        assert_eq!(receipt.state, ProbeReceiptState::Incomplete);
        assert!(receipt.report.is_none());
        let report = base_probe_report(default_token_passed());
        receipt.finalize(report.clone()).expect("finalize succeeds");
        assert_eq!(receipt.state, ProbeReceiptState::Terminal);
        assert_eq!(receipt.report, Some(report));
    }

    #[test]
    fn probe_receipt_finalize_rejects_double_finalize() {
        let mut receipt = ProbeReceipt::incomplete(PROBE_NONCE, &probe_marker_value());
        receipt
            .finalize(base_probe_report(default_token_passed()))
            .expect("first finalize succeeds");
        assert_eq!(
            receipt.finalize(base_probe_report(default_token_passed())),
            Err(RailError::new("invalid-receipt-transition"))
        );
    }

    #[test]
    fn probe_receipt_finalize_rejects_marker_mismatch() {
        let mut receipt = ProbeReceipt::incomplete(PROBE_NONCE, &probe_marker_value());
        let mut report = base_probe_report(default_token_passed());
        report.marker = "solstone-onlogon-probe:someone-else".to_owned();
        assert_eq!(
            receipt.finalize(report),
            Err(RailError::new("receipt-marker-mismatch"))
        );
    }

    #[test]
    fn probe_outcome_display_covers_new_receipt_dispositions() {
        assert_eq!(
            ProbeOutcome::ReceiptUnavailable.to_string(),
            "receipt-unavailable"
        );
        assert_eq!(
            ProbeOutcome::TerminalUpdateFailed.to_string(),
            "terminal-update-failed"
        );
    }
}
