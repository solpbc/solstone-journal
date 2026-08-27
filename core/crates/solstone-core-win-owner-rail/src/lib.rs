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
}
