// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rcgen::CertificateSigningRequestParams;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use solstone_core_journal_io::{JsonWriteOptions, LockOptions, hold_lock, write_json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use x509_parser::certification_request::X509CertificationRequest;
use x509_parser::pem::parse_x509_pem;
use x509_parser::prelude::FromDer;

use crate::client_description::current_display_label;
use crate::client_description_store::{read_descriptions, set_owner_label_locked};
use crate::committed::load_committed_identity;
use crate::ledger::{
    AuthorizationLedger, AuthorizedClientsRead, ClientEntry, read_authorized_clients,
};
use crate::pairing::attestation::mint_home_attestation;
use crate::pairing::pair_response_json;

use super::{
    Choice, DecisionRequest, MigrationReasonCode as Code, MigrationState as State, MigrationView,
    RekeyRequest, RekeyResponse, ReplacesCid,
};

const RECORD_DIR: &str = "device-migrations";
const OPERATION_DIR: &str = "operations";
const DECISION_DIR: &str = "decisions";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MigrationError {
    pub reason: Code,
}

impl MigrationError {
    fn new(reason: Code) -> Self {
        Self { reason }
    }

    pub const fn reason_code(self) -> &'static str {
        self.reason.as_wire()
    }

    pub const fn status_code(self) -> u16 {
        match self.reason {
            Code::MigrationRequestInvalid
            | Code::MigrationProtocolUnsupported
            | Code::MigrationCsrInvalid
            | Code::MigrationKeyNotFresh => 400,
            Code::MigrationForbidden | Code::MigrationReplayForbidden => 403,
            Code::PairedDeviceNotFound => 404,
            Code::MigrationOperationConflict
            | Code::MigrationProofMissing
            | Code::MigrationAlreadyDecided
            | Code::MigrationSelfReplacement
            | Code::MigrationTargetConflict => 409,
            Code::MigrationStateUnavailable => 503,
        }
    }
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.reason_code())
    }
}

impl Error for MigrationError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RekeyOutcome {
    pub response: RekeyResponse,
    pub created: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum OperationCheckpoint {
    Reserved,
    CertRecorded,
    Issued,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum DecisionCheckpoint {
    Prepared,
    AuthRetired,
    AwaitingPush,
    Complete,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OperationRecord {
    caller_cid: String,
    operation_id: String,
    raw_body: Vec<u8>,
    issued_pem: Option<String>,
    issued_cid: Option<String>,
    previous_cid: String,
    pairing: Option<Value>,
    checkpoint: OperationCheckpoint,
    generation: u64,
    order: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DecisionRecord {
    caller_cid: String,
    decision_id: String,
    raw_body: Vec<u8>,
    choice: Choice,
    replaced_cid: Option<String>,
    operation_id: Option<String>,
    continuity_plan: Option<Value>,
    checkpoint: DecisionCheckpoint,
    order: u64,
}

#[derive(Default)]
struct MigrationRecords {
    operations: Vec<OperationRecord>,
    decisions: Vec<DecisionRecord>,
}

pub type ContinuityPublisher<'a> = dyn FnMut(&Value) -> Result<(), MigrationError> + 'a;
pub type DecisionApply<'a> = dyn for<'publisher> FnMut(
        Value,
        &mut ContinuityPublisher<'publisher>,
    ) -> Result<MigrationView, MigrationError>
    + 'a;
pub type DecisionBoundary<'a> = dyn FnMut(
        &str,
        &str,
        Option<&Value>,
        &mut DecisionApply<'a>,
    ) -> Result<MigrationView, MigrationError>
    + 'a;

pub fn rekey(
    journal: &Path,
    caller_cid: &str,
    leaf_spki: &[u8],
    raw_body: &[u8],
) -> Result<RekeyOutcome, MigrationError> {
    let request: RekeyRequest = parse_request(raw_body)?;
    if request.protocol != 1 {
        return Err(MigrationError::new(Code::MigrationProtocolUnsupported));
    }
    if !valid_uuid(&request.operation_id)
        || request.device_label.trim().is_empty()
        || request.device_label.trim().len() > 80
    {
        return Err(MigrationError::new(Code::MigrationRequestInvalid));
    }
    validate_csr_spki(&request.csr, leaf_spki)?;

    let operation_id = request.operation_id.clone();
    let auth_path = authorization_path(journal);
    let generation = {
        let guard = hold_lock(&auth_path, LockOptions::default())
            .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))?;
        let mut records = read_migration_records(journal)?;
        let entries = present_entries(&auth_path)?;
        let caller = entries
            .iter()
            .find(|entry| entry.fingerprint == caller_cid)
            .ok_or_else(|| MigrationError::new(Code::MigrationForbidden))?;
        if let Some(index) = records
            .operations
            .iter()
            .position(|record| record.operation_id == operation_id)
        {
            let latest_order = next_order(&records);
            let record = &mut records.operations[index];
            if record.caller_cid != caller_cid {
                return Err(MigrationError::new(Code::MigrationReplayForbidden));
            }
            if record.raw_body != raw_body {
                return Err(MigrationError::new(Code::MigrationOperationConflict));
            }
            if record.checkpoint == OperationCheckpoint::Issued {
                return Ok(RekeyOutcome {
                    response: rekey_response(record)?,
                    created: false,
                });
            }
            if record.issued_pem.is_some() {
                let response = rekey_response(record)?;
                recover_operation_row(journal, &guard, caller, record)?;
                return Ok(RekeyOutcome {
                    response,
                    created: false,
                });
            }
            record.generation = record
                .generation
                .checked_add(1)
                .ok_or_else(|| MigrationError::new(Code::MigrationStateUnavailable))?;
            record.checkpoint = OperationCheckpoint::Reserved;
            record.order = latest_order;
            let generation = record.generation;
            write_operation(journal, record)?;
            generation
        } else {
            let generation = 1;
            let record = OperationRecord {
                caller_cid: caller_cid.to_owned(),
                operation_id: operation_id.clone(),
                raw_body: raw_body.to_vec(),
                issued_pem: None,
                issued_cid: None,
                previous_cid: caller_cid.to_owned(),
                pairing: None,
                checkpoint: OperationCheckpoint::Reserved,
                generation,
                order: next_order(&records),
            };
            write_operation(journal, &record)?;
            records.operations.push(record);
            generation
        }
    };

    // Certificate, attestation, and response construction deliberately happen
    // after the authorization sidecar has been released.
    let identity = load_committed_identity(journal)
        .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))?;
    let issued = crate::ca::sign_csr(identity.ca(), &request.csr, request.device_label.trim())
        .map_err(|_| MigrationError::new(Code::MigrationCsrInvalid))?;
    let now = unix_seconds()?;
    let attestation =
        mint_home_attestation(identity.ca(), identity.instance_id(), issued.cid(), now)
            .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))?;
    let home_label = crate::mark::mark_words_from_jid(identity.instance_id())
        .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))?;
    let response = spl_core::PairResponse {
        client_cert: issued.pem().to_owned(),
        ca_chain: vec![
            String::from_utf8(identity.certificate_pem().to_vec())
                .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))?,
        ],
        instance_id: identity.instance_id().to_owned(),
        home_label,
        fingerprint: issued.cid().to_owned(),
        home_attestation: Some(attestation),
        local_endpoints: None,
        relay_access: None,
    };
    let pairing = pair_response_json(&response, None)
        .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))?;

    let guard = hold_lock(&auth_path, LockOptions::default())
        .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))?;
    let entries = present_entries(&auth_path)?;
    let caller = entries
        .iter()
        .find(|entry| entry.fingerprint == caller_cid)
        .ok_or_else(|| MigrationError::new(Code::MigrationForbidden))?;
    let mut records = read_migration_records(journal)?;
    let record = records
        .operations
        .iter_mut()
        .find(|record| record.operation_id == operation_id)
        .ok_or_else(|| MigrationError::new(Code::MigrationStateUnavailable))?;
    if record.caller_cid != caller_cid || record.raw_body != raw_body {
        return Err(MigrationError::new(Code::MigrationReplayForbidden));
    }
    if record.generation != generation {
        if record.issued_pem.is_some() {
            return Ok(RekeyOutcome {
                response: rekey_response(record)?,
                created: false,
            });
        }
        return Err(MigrationError::new(Code::MigrationStateUnavailable));
    }
    if record.issued_pem.is_some() {
        let stored_response = rekey_response(record)?;
        recover_operation_row(journal, &guard, caller, record)?;
        return Ok(RekeyOutcome {
            response: stored_response,
            created: false,
        });
    }
    record.issued_pem = Some(issued.pem().to_owned());
    record.issued_cid = Some(issued.cid().to_owned());
    record.pairing = Some(pairing);
    record.checkpoint = OperationCheckpoint::CertRecorded;
    write_operation(journal, record)?;
    add_operation_row(journal, &guard, caller, record)?;
    record.checkpoint = OperationCheckpoint::Issued;
    write_operation(journal, record)?;
    Ok(RekeyOutcome {
        response: rekey_response(record)?,
        created: true,
    })
}

fn validate_csr_spki(csr_pem: &str, leaf_spki: &[u8]) -> Result<(), MigrationError> {
    CertificateSigningRequestParams::from_pem(csr_pem)
        .map_err(|_| MigrationError::new(Code::MigrationCsrInvalid))?;
    let (_, pem) = parse_x509_pem(csr_pem.as_bytes())
        .map_err(|_| MigrationError::new(Code::MigrationCsrInvalid))?;
    let (_, csr) = X509CertificationRequest::from_der(&pem.contents)
        .map_err(|_| MigrationError::new(Code::MigrationCsrInvalid))?;
    if csr.certification_request_info.subject_pki.raw == leaf_spki {
        return Err(MigrationError::new(Code::MigrationKeyNotFresh));
    }
    Ok(())
}

fn add_operation_row(
    journal: &Path,
    guard: &solstone_core_journal_io::FileLock,
    caller: &ClientEntry,
    record: &OperationRecord,
) -> Result<(), MigrationError> {
    let cid = record
        .issued_cid
        .as_deref()
        .ok_or_else(|| MigrationError::new(Code::MigrationStateUnavailable))?;
    let paired_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))?;
    let mut entry = ClientEntry::new(
        cid,
        record_device_label(record)?,
        paired_at,
        caller.instance_id.clone(),
        caller.role.clone(),
    );
    entry.platform = caller.platform;
    let mut ledger = AuthorizationLedger::new(journal);
    let row = read_authorized_clients(&authorization_path(journal));
    match row {
        AuthorizedClientsRead::Present(entries)
            if entries.iter().any(|entry| entry.fingerprint == cid) =>
        {
            Ok(())
        }
        AuthorizedClientsRead::Present(_) => ledger
            .add_locked(guard, entry)
            .map(|_| ())
            .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable)),
        _ => Err(MigrationError::new(Code::MigrationStateUnavailable)),
    }
}

fn record_device_label(record: &OperationRecord) -> Result<String, MigrationError> {
    let request: RekeyRequest = parse_request(&record.raw_body)?;
    Ok(request.device_label.trim().to_owned())
}

fn recover_operation_row(
    journal: &Path,
    guard: &solstone_core_journal_io::FileLock,
    caller: &ClientEntry,
    record: &mut OperationRecord,
) -> Result<(), MigrationError> {
    add_operation_row(journal, guard, caller, record)?;
    if record.checkpoint != OperationCheckpoint::Issued {
        record.checkpoint = OperationCheckpoint::Issued;
        write_operation(journal, record)?;
    }
    Ok(())
}

fn rekey_response(record: &OperationRecord) -> Result<RekeyResponse, MigrationError> {
    Ok(RekeyResponse {
        protocol: 1,
        operation_id: record.operation_id.clone(),
        state: State::Pending,
        cid: record
            .issued_cid
            .clone()
            .ok_or_else(|| MigrationError::new(Code::MigrationStateUnavailable))?,
        previous_cid: record.previous_cid.clone(),
        pairing: record
            .pairing
            .clone()
            .ok_or_else(|| MigrationError::new(Code::MigrationStateUnavailable))?,
    })
}

pub fn migration_state(journal: &Path, caller_cid: &str) -> Result<MigrationView, MigrationError> {
    let auth_path = authorization_path(journal);
    let _guard = hold_lock(&auth_path, LockOptions::default())
        .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))?;
    let records = read_migration_records(journal)?;
    let entries = present_entries(&auth_path)?;
    if !entries.iter().any(|entry| entry.fingerprint == caller_cid) {
        return Err(MigrationError::new(Code::MigrationForbidden));
    }
    view_for_caller(&records, caller_cid)
}

pub fn ingest_blocked(journal: &Path, cid: &str) -> Result<bool, MigrationError> {
    let _guard = hold_lock(authorization_path(journal), LockOptions::default())
        .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))?;
    let records = read_migration_records(journal)?;
    Ok(records.decisions.iter().any(|decision| {
        decision.checkpoint == DecisionCheckpoint::Prepared
            && (decision.caller_cid == cid || decision.replaced_cid.as_deref() == Some(cid))
    }))
}

/// Record or resume an authenticated migration decision. The boundary owns
/// the segment source and registry locks; the apply callback runs inside it.
pub fn decide<'a>(
    journal: &'a Path,
    caller_cid: &'a str,
    raw_body: &'a [u8],
    boundary: &mut DecisionBoundary<'a>,
) -> Result<MigrationView, MigrationError> {
    let request: DecisionRequest = parse_request(raw_body)?;
    if request.protocol != 1 {
        return Err(MigrationError::new(Code::MigrationProtocolUnsupported));
    }
    if !valid_uuid(&request.decision_id) {
        return Err(MigrationError::new(Code::MigrationRequestInvalid));
    }
    let target = match (&request.choice, &request.replaces_cid) {
        (Choice::ReplaceDevice, ReplacesCid::Cid(cid)) if valid_cid(cid) => Some(cid.clone()),
        (Choice::ReplaceDevice, _) => {
            return Err(MigrationError::new(Code::MigrationRequestInvalid));
        }
        (_, ReplacesCid::Missing) => None,
        (_, ReplacesCid::Null | ReplacesCid::Cid(_)) => {
            return Err(MigrationError::new(Code::MigrationRequestInvalid));
        }
    };

    let auth_path = authorization_path(journal);
    let (retired_cid, operation_id, stored_plan, existing_id) = {
        let _guard = hold_lock(&auth_path, LockOptions::default())
            .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))?;
        let records = read_migration_records(journal)?;
        let entries = present_entries(&auth_path)?;
        if !entries.iter().any(|entry| entry.fingerprint == caller_cid) {
            return Err(MigrationError::new(Code::MigrationForbidden));
        }
        if let Some(existing) = records
            .decisions
            .iter()
            .find(|record| record.decision_id == request.decision_id)
        {
            if existing.caller_cid != caller_cid {
                return Err(MigrationError::new(Code::MigrationReplayForbidden));
            }
            if existing.raw_body != raw_body {
                return Err(MigrationError::new(Code::MigrationOperationConflict));
            }
            if existing.checkpoint != DecisionCheckpoint::Prepared {
                return Ok(view_from_record(
                    existing,
                    previous_cid_for(&records, caller_cid),
                ));
            }
            let retired = existing.replaced_cid.clone();
            let operation = existing.operation_id.clone();
            let plan = existing.continuity_plan.clone();
            (retired, operation, plan, true)
        } else {
            let operation = latest_operation_for_issued_cid(&records, caller_cid);
            let previous = operation.as_ref().map(|record| record.previous_cid.clone());
            if request.choice == Choice::SameDevice && previous.is_none() {
                return Err(MigrationError::new(Code::MigrationProofMissing));
            }
            let target = match request.choice {
                Choice::SameDevice => previous.clone(),
                Choice::ReplaceDevice => target.clone(),
                Choice::NewDevice => None,
            };
            if target.as_deref() == Some(caller_cid) {
                return Err(MigrationError::new(Code::MigrationSelfReplacement));
            }
            if target.as_ref().is_some_and(|target| {
                records.decisions.iter().any(|decision| {
                    decision.replaced_cid.as_deref() == Some(target)
                        && decision.checkpoint != DecisionCheckpoint::Prepared
                })
            }) {
                return Err(MigrationError::new(Code::MigrationTargetConflict));
            }
            if matches!(request.choice, Choice::ReplaceDevice)
                && !entries
                    .iter()
                    .any(|entry| Some(entry.fingerprint.as_str()) == target.as_deref())
            {
                return Err(MigrationError::new(Code::PairedDeviceNotFound));
            }
            if operation.as_ref().is_some_and(|operation| {
                records.decisions.iter().any(|decision| {
                    decision.operation_id.as_deref() == Some(&operation.operation_id)
                })
            }) && matches!(request.choice, Choice::NewDevice | Choice::SameDevice)
            {
                return Err(MigrationError::new(Code::MigrationAlreadyDecided));
            }
            let view_operation = operation.map(|record| record.operation_id.clone());
            let plan = None;
            (target, view_operation, plan, false)
        }
    };

    if request.choice == Choice::NewDevice {
        if existing_id {
            let _guard = hold_lock(&auth_path, LockOptions::default())
                .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))?;
            return view_for_decision(&read_migration_records(journal)?, &request.decision_id);
        }
        let _guard = hold_lock(&auth_path, LockOptions::default())
            .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))?;
        let mut records = read_migration_records(journal)?;
        let entries = present_entries(&auth_path)?;
        if !entries.iter().any(|entry| entry.fingerprint == caller_cid) {
            return Err(MigrationError::new(Code::MigrationForbidden));
        }
        if let Some(existing) = records
            .decisions
            .iter()
            .find(|record| record.decision_id == request.decision_id)
        {
            if existing.caller_cid != caller_cid {
                return Err(MigrationError::new(Code::MigrationReplayForbidden));
            }
            if existing.raw_body != raw_body {
                return Err(MigrationError::new(Code::MigrationOperationConflict));
            }
            return view_for_decision(&records, &request.decision_id);
        }
        let operation = latest_operation_for_issued_cid(&records, caller_cid);
        if operation.as_ref().is_some_and(|operation| {
            records
                .decisions
                .iter()
                .any(|decision| decision.operation_id.as_deref() == Some(&operation.operation_id))
        }) {
            return Err(MigrationError::new(Code::MigrationAlreadyDecided));
        }
        let order = next_order(&records);
        let record = DecisionRecord {
            caller_cid: caller_cid.to_owned(),
            decision_id: request.decision_id.clone(),
            raw_body: raw_body.to_vec(),
            choice: Choice::NewDevice,
            replaced_cid: None,
            operation_id,
            continuity_plan: None,
            checkpoint: DecisionCheckpoint::Complete,
            order,
        };
        write_decision(journal, &record)?;
        records.decisions.push(record);
        return view_for_caller(&records, caller_cid);
    }

    let retired =
        retired_cid.ok_or_else(|| MigrationError::new(Code::MigrationStateUnavailable))?;
    let apply_journal = journal.to_path_buf();
    let apply_auth_path = auth_path.clone();
    let apply_caller_cid = caller_cid.to_owned();
    let apply_raw_body = raw_body.to_vec();
    let apply_decision_id = request.decision_id.clone();
    let apply_choice = request.choice;
    let apply_retired_cid = retired.clone();
    let mut apply = move |plan: Value,
                          publish: &mut ContinuityPublisher<'_>|
          -> Result<MigrationView, MigrationError> {
        let _guard = hold_lock(&apply_auth_path, LockOptions::default())
            .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))?;
        let mut records = read_migration_records(&apply_journal)?;
        let entries = present_entries(&apply_auth_path)?;
        entries
            .iter()
            .find(|entry| entry.fingerprint == apply_caller_cid)
            .ok_or_else(|| MigrationError::new(Code::MigrationForbidden))?;
        let record_index = records
            .decisions
            .iter()
            .position(|record| record.decision_id == apply_decision_id);
        if let Some(index) = record_index {
            let record = &records.decisions[index];
            if record.caller_cid != apply_caller_cid {
                return Err(MigrationError::new(Code::MigrationReplayForbidden));
            }
            if record.raw_body != apply_raw_body {
                return Err(MigrationError::new(Code::MigrationOperationConflict));
            }
        } else {
            let operation = latest_operation_for_issued_cid(&records, &apply_caller_cid);
            let previous = operation
                .as_ref()
                .map(|record| record.previous_cid.as_str());
            if apply_choice == Choice::SameDevice && previous != Some(apply_retired_cid.as_str()) {
                return Err(MigrationError::new(Code::MigrationProofMissing));
            }
            if records.decisions.iter().any(|decision| {
                decision.replaced_cid.as_deref() == Some(apply_retired_cid.as_str())
                    && decision.checkpoint != DecisionCheckpoint::Prepared
            }) {
                return Err(MigrationError::new(Code::MigrationTargetConflict));
            }
            if !entries
                .iter()
                .any(|entry| entry.fingerprint == apply_retired_cid)
            {
                return Err(MigrationError::new(Code::PairedDeviceNotFound));
            }
            if operation.as_ref().is_some_and(|operation| {
                records.decisions.iter().any(|decision| {
                    decision.operation_id.as_deref() == Some(&operation.operation_id)
                })
            }) && matches!(apply_choice, Choice::NewDevice | Choice::SameDevice)
            {
                return Err(MigrationError::new(Code::MigrationAlreadyDecided));
            }
            let order = next_order(&records);
            let new_record = DecisionRecord {
                caller_cid: apply_caller_cid.clone(),
                decision_id: apply_decision_id.clone(),
                raw_body: apply_raw_body.clone(),
                choice: apply_choice,
                replaced_cid: Some(apply_retired_cid.clone()),
                operation_id: operation.map(|record| record.operation_id.clone()),
                continuity_plan: Some(plan.clone()),
                checkpoint: DecisionCheckpoint::Prepared,
                order,
            };
            write_decision(&apply_journal, &new_record)?;
            records.decisions.push(new_record);
        }

        let index = records
            .decisions
            .iter()
            .position(|record| record.decision_id == apply_decision_id)
            .ok_or_else(|| MigrationError::new(Code::MigrationStateUnavailable))?;
        let operation_id_for_view = records.decisions[index].operation_id.clone();
        let previous_for_view = operation_id_for_view.as_ref().and_then(|operation_id| {
            records
                .operations
                .iter()
                .find(|operation| operation.operation_id == *operation_id)
                .map(|operation| operation.previous_cid.clone())
        });
        let record = &mut records.decisions[index];
        if record.checkpoint == DecisionCheckpoint::Complete
            || record.checkpoint == DecisionCheckpoint::AwaitingPush
            || record.checkpoint == DecisionCheckpoint::AuthRetired
        {
            return Ok(view_from_record(record, previous_for_view));
        }
        record.continuity_plan = Some(plan.clone());
        write_decision(&apply_journal, record)?;

        publish(&plan)?;
        let descriptions = read_descriptions(&apply_journal)
            .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))?;
        let selected = entries
            .iter()
            .find(|entry| entry.fingerprint == apply_retired_cid);
        let Some(selected) = selected else {
            // The prepared record fixes the target; a missing row here is an
            // idempotent resume after its authorization write already landed.
            record.checkpoint = DecisionCheckpoint::AuthRetired;
            write_decision(&apply_journal, record)?;
            return Ok(view_from_record(record, previous_for_view));
        };
        let label = current_display_label(selected, descriptions.get(&apply_retired_cid));
        set_owner_label_locked(
            &apply_journal,
            &apply_caller_cid,
            &label,
            OffsetDateTime::now_utc(),
        )
        .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))?;
        let mut ledger = AuthorizationLedger::new(&apply_journal);
        ledger
            .remove_locked(&_guard, &apply_retired_cid)
            .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))?;
        record.checkpoint = DecisionCheckpoint::AuthRetired;
        write_decision(&apply_journal, record)?;
        Ok(view_from_record(record, previous_for_view))
    };

    let Some(stored_plan) = stored_plan else {
        return boundary(caller_cid, &retired, None, &mut apply);
    };
    boundary(caller_cid, &retired, Some(&stored_plan), &mut apply)
}

/// CID whose push registrations must be removed after an auth retirement.
pub fn pending_push_cid(
    journal: &Path,
    caller_cid: &str,
    decision_id: &str,
) -> Result<Option<String>, MigrationError> {
    let _guard = hold_lock(authorization_path(journal), LockOptions::default())
        .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))?;
    let records = read_migration_records(journal)?;
    let record = records
        .decisions
        .iter()
        .find(|record| record.decision_id == decision_id)
        .ok_or_else(|| MigrationError::new(Code::MigrationStateUnavailable))?;
    if record.caller_cid != caller_cid {
        return Err(MigrationError::new(Code::MigrationReplayForbidden));
    }
    Ok(matches!(
        record.checkpoint,
        DecisionCheckpoint::AuthRetired | DecisionCheckpoint::AwaitingPush
    )
    .then(|| record.replaced_cid.clone())
    .flatten())
}

/// Complete or retain an auth-retired decision after push cleanup.
pub fn record_push_result(
    journal: &Path,
    caller_cid: &str,
    decision_id: &str,
    succeeded: bool,
) -> Result<MigrationView, MigrationError> {
    let _guard = hold_lock(authorization_path(journal), LockOptions::default())
        .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))?;
    let mut records = read_migration_records(journal)?;
    let index = records
        .decisions
        .iter()
        .position(|record| record.decision_id == decision_id)
        .ok_or_else(|| MigrationError::new(Code::MigrationStateUnavailable))?;
    let operation_id = records.decisions[index].operation_id.clone();
    let previous_cid = operation_id.as_ref().and_then(|operation_id| {
        records
            .operations
            .iter()
            .find(|operation| operation.operation_id == *operation_id)
            .map(|operation| operation.previous_cid.clone())
    });
    let record = &mut records.decisions[index];
    if record.caller_cid != caller_cid {
        return Err(MigrationError::new(Code::MigrationReplayForbidden));
    }
    if !matches!(
        record.checkpoint,
        DecisionCheckpoint::AuthRetired | DecisionCheckpoint::AwaitingPush
    ) {
        return if record.checkpoint == DecisionCheckpoint::Complete {
            Ok(view_from_record(record, previous_cid))
        } else {
            Err(MigrationError::new(Code::MigrationStateUnavailable))
        };
    }
    if succeeded {
        record.checkpoint = DecisionCheckpoint::Complete;
        write_decision(journal, record)?;
        Ok(view_from_record(record, previous_cid))
    } else {
        record.checkpoint = DecisionCheckpoint::AwaitingPush;
        write_decision(journal, record)?;
        Err(MigrationError::new(Code::MigrationStateUnavailable))
    }
}

fn parse_request<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, MigrationError> {
    serde_json::from_slice(bytes).map_err(|_| MigrationError::new(Code::MigrationRequestInvalid))
}

fn present_entries(path: &Path) -> Result<Vec<ClientEntry>, MigrationError> {
    match read_authorized_clients(path) {
        AuthorizedClientsRead::Present(entries) => Ok(entries),
        AuthorizedClientsRead::Missing => Err(MigrationError::new(Code::MigrationForbidden)),
        AuthorizedClientsRead::Unreadable
        | AuthorizedClientsRead::Malformed
        | AuthorizedClientsRead::DuplicateCid => {
            Err(MigrationError::new(Code::MigrationStateUnavailable))
        }
    }
}

fn read_migration_records(journal: &Path) -> Result<MigrationRecords, MigrationError> {
    let mut records = MigrationRecords::default();
    let operations = journal.join("link").join(RECORD_DIR).join(OPERATION_DIR);
    let decisions = journal.join("link").join(RECORD_DIR).join(DECISION_DIR);
    for path in json_record_paths(&operations)? {
        let bytes =
            fs::read(&path).map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))?;
        let record: OperationRecord = serde_json::from_slice(&bytes)
            .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))?;
        if !valid_uuid(&record.operation_id)
            || path.file_stem().and_then(|stem| stem.to_str()) != Some(&record.operation_id)
        {
            return Err(MigrationError::new(Code::MigrationStateUnavailable));
        }
        records.operations.push(record);
    }
    for path in json_record_paths(&decisions)? {
        let bytes =
            fs::read(&path).map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))?;
        let record: DecisionRecord = serde_json::from_slice(&bytes)
            .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))?;
        if !valid_uuid(&record.decision_id)
            || path.file_stem().and_then(|stem| stem.to_str()) != Some(&record.decision_id)
        {
            return Err(MigrationError::new(Code::MigrationStateUnavailable));
        }
        records.decisions.push(record);
    }
    records.operations.sort_by_key(|record| record.order);
    records.decisions.sort_by_key(|record| record.order);
    Ok(records)
}

fn json_record_paths(directory: &Path) -> Result<Vec<PathBuf>, MigrationError> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(MigrationError::new(Code::MigrationStateUnavailable)),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))?;
        let kind = entry
            .file_type()
            .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))?;
        if !kind.is_file() {
            return Err(MigrationError::new(Code::MigrationStateUnavailable));
        }
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            return Err(MigrationError::new(Code::MigrationStateUnavailable));
        }
        paths.push(path);
    }
    paths.sort();
    Ok(paths)
}

fn write_operation(journal: &Path, record: &OperationRecord) -> Result<(), MigrationError> {
    let path = journal
        .join("link")
        .join(RECORD_DIR)
        .join(OPERATION_DIR)
        .join(format!("{}.json", record.operation_id));
    write_json(
        &path,
        record,
        JsonWriteOptions {
            mode: Some(0o600),
            ..JsonWriteOptions::default()
        },
    )
    .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))
}

fn write_decision(journal: &Path, record: &DecisionRecord) -> Result<(), MigrationError> {
    let path = journal
        .join("link")
        .join(RECORD_DIR)
        .join(DECISION_DIR)
        .join(format!("{}.json", record.decision_id));
    write_json(
        &path,
        record,
        JsonWriteOptions {
            mode: Some(0o600),
            ..JsonWriteOptions::default()
        },
    )
    .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))
}

fn authorization_path(journal: &Path) -> PathBuf {
    journal.join("link").join("authorized_clients.json")
}

fn next_order(records: &MigrationRecords) -> u64 {
    records
        .operations
        .iter()
        .map(|record| record.order)
        .chain(records.decisions.iter().map(|record| record.order))
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

fn latest_operation_for_issued_cid<'a>(
    records: &'a MigrationRecords,
    cid: &str,
) -> Option<&'a OperationRecord> {
    records
        .operations
        .iter()
        .filter(|record| record.issued_cid.as_deref() == Some(cid))
        .max_by_key(|record| record.order)
}

fn previous_cid_for(records: &MigrationRecords, caller_cid: &str) -> Option<String> {
    latest_operation_for_issued_cid(records, caller_cid)
        .map(|operation| operation.previous_cid.clone())
}

fn view_for_caller(
    records: &MigrationRecords,
    caller_cid: &str,
) -> Result<MigrationView, MigrationError> {
    let operation = records
        .operations
        .iter()
        .filter(|record| {
            record.caller_cid == caller_cid || record.issued_cid.as_deref() == Some(caller_cid)
        })
        .max_by_key(|record| record.order);
    let decision = records
        .decisions
        .iter()
        .filter(|record| {
            record.caller_cid == caller_cid
                || operation.is_some_and(|operation| {
                    record.operation_id.as_deref() == Some(&operation.operation_id)
                })
        })
        .max_by_key(|record| record.order);
    if let Some(decision) = decision {
        if decision.checkpoint != DecisionCheckpoint::Complete {
            return Err(MigrationError::new(Code::MigrationStateUnavailable));
        }
        let previous = decision.operation_id.as_ref().and_then(|operation_id| {
            records
                .operations
                .iter()
                .find(|operation| operation.operation_id == *operation_id)
                .map(|operation| operation.previous_cid.clone())
        });
        return Ok(view_from_record(decision, previous));
    }
    if operation.is_some_and(|operation| operation.checkpoint != OperationCheckpoint::Issued) {
        return Err(MigrationError::new(Code::MigrationStateUnavailable));
    }
    Ok(match operation {
        Some(operation) => MigrationView {
            protocol: 1,
            state: State::Pending,
            operation_id: Some(operation.operation_id.clone()),
            decision_id: None,
            previous_cid: Some(operation.previous_cid.clone()),
            replaced_cid: None,
        },
        None => empty_view(),
    })
}

fn view_for_decision(
    records: &MigrationRecords,
    decision_id: &str,
) -> Result<MigrationView, MigrationError> {
    let decision = records
        .decisions
        .iter()
        .find(|record| record.decision_id == decision_id)
        .ok_or_else(|| MigrationError::new(Code::MigrationStateUnavailable))?;
    if decision.checkpoint != DecisionCheckpoint::Complete {
        return Err(MigrationError::new(Code::MigrationStateUnavailable));
    }
    let previous = decision.operation_id.as_ref().and_then(|operation_id| {
        records
            .operations
            .iter()
            .find(|operation| operation.operation_id == *operation_id)
            .map(|operation| operation.previous_cid.clone())
    });
    Ok(view_from_record(decision, previous))
}

fn view_from_record(record: &DecisionRecord, previous_cid: Option<String>) -> MigrationView {
    MigrationView {
        protocol: 1,
        state: match record.choice {
            Choice::NewDevice => State::NewDevice,
            Choice::SameDevice => State::SameDevice,
            Choice::ReplaceDevice => State::ReplacedDevice,
        },
        operation_id: record.operation_id.clone(),
        decision_id: Some(record.decision_id.clone()),
        previous_cid,
        replaced_cid: record.replaced_cid.clone(),
    }
}

fn empty_view() -> MigrationView {
    MigrationView {
        protocol: 1,
        state: State::None,
        operation_id: None,
        decision_id: None,
        previous_cid: None,
        replaced_cid: None,
    }
}

fn valid_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && [8, 13, 18, 23]
            .into_iter()
            .all(|index| bytes[index] == b'-')
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| [8, 13, 18, 23].contains(&index) || byte.is_ascii_hexdigit())
}

fn valid_cid(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn unix_seconds() -> Result<i64, MigrationError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .map_err(|_| MigrationError::new(Code::MigrationStateUnavailable))
}

pub fn contract_pairing_value() -> Value {
    let response = spl_core::PairResponse {
        client_cert: "-----BEGIN CERTIFICATE-----\nfixture\n-----END CERTIFICATE-----".to_owned(),
        ca_chain: vec![
            "-----BEGIN CERTIFICATE-----\nca-fixture\n-----END CERTIFICATE-----".to_owned(),
        ],
        instance_id: "fixture-instance".to_owned(),
        home_label: "fixture-home".to_owned(),
        fingerprint: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .to_owned(),
        home_attestation: None,
        local_endpoints: None,
        relay_access: None,
    };
    pair_response_json(&response, None).expect("pairing contract fixture serializes")
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
    use serde_json::{Value, json};

    use super::*;
    use crate::ca::{LocalCa, generate_ca, jid_from_spki, sign_csr};
    use crate::client_description::{
        JournalIdentityMeta, PutSelfDescriptionRequest, ReportedDescription,
    };
    use crate::client_description_store::put_self_description;
    use crate::ledger::{ClientRole, DeviceActivityRead};
    use crate::pairing_identity::Platform;

    static NEXT: AtomicU64 = AtomicU64::new(0);
    const OLD: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const ADOPTED: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const OTHER: &str = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

    struct Journal(PathBuf);

    impl Journal {
        fn new() -> Self {
            let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path = PathBuf::from("/var/tmp").join(format!(
                "solstone-device-migration-{}-{nanos}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("journal root creates");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Journal {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn identity(journal: &Path) -> (LocalCa, String) {
        let ca = generate_ca().expect("test CA generates");
        let instance_id = jid_from_spki(ca.spki_der()).expect("instance id derives");
        let ca_dir = journal.join("link/ca");
        fs::create_dir_all(&ca_dir).expect("CA path creates");
        fs::write(ca_dir.join("cert.pem"), ca.certificate_pem()).expect("CA cert writes");
        fs::write(ca_dir.join("private.pem"), ca.private_key_pem()).expect("CA key writes");
        fs::write(
            journal.join("link/state.json"),
            serde_json::to_vec(&json!({"instance_id": instance_id, "home_label": "Test Home"}))
                .unwrap(),
        )
        .expect("link state writes");
        (ca, instance_id)
    }

    fn csr() -> (KeyPair, String) {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("test key generates");
        let pem = CertificateParams::default()
            .serialize_request(&key)
            .expect("CSR serializes")
            .pem()
            .expect("CSR PEM serializes");
        (key, pem)
    }

    fn csr_spki(pem: &str) -> Vec<u8> {
        let (_, pem) = parse_x509_pem(pem.as_bytes()).expect("CSR PEM parses");
        let (_, csr) = X509CertificationRequest::from_der(&pem.contents).expect("CSR parses");
        csr.certification_request_info.subject_pki.raw.to_vec()
    }

    fn leaf_spki(pem: &str) -> Vec<u8> {
        let (_, pem) = parse_x509_pem(pem.as_bytes()).expect("certificate PEM parses");
        let (_, leaf) = x509_parser::parse_x509_certificate(&pem.contents).expect("leaf parses");
        leaf.tbs_certificate.subject_pki.raw.to_vec()
    }

    fn add_client(
        journal: &Path,
        cid: &str,
        label: &str,
        role: ClientRole,
        platform: Option<Platform>,
    ) {
        let mut entry =
            ClientEntry::new(cid, label, "2026-09-01T00:00:00Z", "fixture-instance", role);
        entry.platform = platform;
        AuthorizationLedger::new(journal)
            .add(entry)
            .expect("authorization row added");
    }

    fn add_operation(journal: &Path, old: &str, adopted: &str, order: u64) {
        let request = RekeyRequest {
            protocol: 1,
            operation_id: "123e4567-e89b-42d3-a456-426614174000".to_owned(),
            csr: "stored-csr".to_owned(),
            device_label: "Phone".to_owned(),
        };
        let record = OperationRecord {
            caller_cid: old.to_owned(),
            operation_id: request.operation_id.clone(),
            raw_body: serde_json::to_vec(&request).unwrap(),
            issued_pem: Some("stored-certificate".to_owned()),
            issued_cid: Some(adopted.to_owned()),
            previous_cid: old.to_owned(),
            pairing: Some(json!({"client_cert": "stored-certificate"})),
            checkpoint: OperationCheckpoint::Issued,
            generation: 1,
            order,
        };
        let guard = hold_lock(&authorization_path(journal), LockOptions::default()).unwrap();
        write_operation(journal, &record).unwrap();
        drop(guard);
    }

    fn body(choice: Choice, id: &str, target: Option<&str>) -> Vec<u8> {
        let request = DecisionRequest {
            protocol: 1,
            decision_id: id.to_owned(),
            choice,
            replaces_cid: target
                .map_or(ReplacesCid::Missing, |cid| ReplacesCid::Cid(cid.to_owned())),
        };
        serde_json::to_vec(&request).unwrap()
    }

    fn direct_boundary(
        adopted: &str,
        retired: &str,
        stored_plan: Option<&Value>,
        apply: &mut DecisionApply<'_>,
    ) -> Result<MigrationView, MigrationError> {
        let plan = stored_plan.cloned().unwrap_or_else(
            || json!({"adopted_cid": adopted, "retired_cid": retired, "sources": []}),
        );
        let mut publish = |_plan: &Value| Ok(());
        apply(plan, &mut publish)
    }

    fn entries(journal: &Path) -> Vec<ClientEntry> {
        match read_authorized_clients(&authorization_path(journal)) {
            AuthorizedClientsRead::Present(entries) => entries,
            other => panic!("unexpected authorization posture: {other:?}"),
        }
    }

    #[test]
    fn migration_csr_freshness_compares_full_spki_der() {
        let (_, csr) = csr();
        let spki = csr_spki(&csr);
        assert_eq!(
            validate_csr_spki(&csr, &spki).unwrap_err().reason,
            Code::MigrationKeyNotFresh
        );
        validate_csr_spki(&csr, &[0x30, 0x00]).expect("different SPKI is fresh");
    }

    #[test]
    fn migration_operation_replay_returns_stored_pairing_without_signing() {
        let journal = Journal::new();
        let (ca, instance_id) = identity(journal.path());
        let (old_key, old_csr) = csr();
        let old_certificate = sign_csr(&ca, &old_csr, "Old phone").unwrap();
        let old_cid = old_certificate.cid().to_owned();
        let old_spki = leaf_spki(old_certificate.pem());
        let mut old_entry = ClientEntry::new(
            old_cid.clone(),
            "Old phone",
            "2026-09-01T00:00:00Z",
            &instance_id,
            ClientRole::Unknown("custom-role".to_owned()),
        );
        old_entry.platform = Some(Platform::Ios);
        AuthorizationLedger::new(journal.path())
            .add(old_entry)
            .unwrap();
        let (_, new_csr) = csr();
        let body = serde_json::to_vec(&RekeyRequest {
            protocol: 1,
            operation_id: "123e4567-e89b-42d3-a456-426614174010".to_owned(),
            csr: new_csr,
            device_label: "Phone".to_owned(),
        })
        .unwrap();

        let first = rekey(journal.path(), &old_cid, &old_spki, &body).unwrap();
        let second = rekey(journal.path(), &old_cid, &old_spki, &body).unwrap();

        assert!(first.created);
        assert!(!second.created);
        assert_eq!(first.response, second.response);
        let rows = entries(journal.path());
        assert_eq!(rows.len(), 2);
        let adopted = rows
            .iter()
            .find(|entry| entry.fingerprint == first.response.cid)
            .unwrap();
        assert_eq!(adopted.role.as_wire(), "custom-role");
        assert_eq!(adopted.platform, Some(Platform::Ios));
        assert_eq!(old_key.public_key_der(), csr_spki(&old_csr));
    }

    #[test]
    fn migration_operation_cross_cid_replay_is_forbidden() {
        let journal = Journal::new();
        let (ca, _) = identity(journal.path());
        let (old_key, old_csr) = csr();
        let old_certificate = sign_csr(&ca, &old_csr, "Old").unwrap();
        let old_cid = old_certificate.cid().to_owned();
        let old_spki = leaf_spki(old_certificate.pem());
        add_client(journal.path(), &old_cid, "Old", ClientRole::Roleless, None);
        let (other_key, other_csr) = csr();
        let other_certificate = sign_csr(&ca, &other_csr, "Other").unwrap();
        let other_cid = other_certificate.cid().to_owned();
        let other_spki = leaf_spki(other_certificate.pem());
        add_client(
            journal.path(),
            &other_cid,
            "Other",
            ClientRole::Roleless,
            None,
        );
        let (_, new_csr) = csr();
        let body = serde_json::to_vec(&RekeyRequest {
            protocol: 1,
            operation_id: "123e4567-e89b-42d3-a456-426614174011".to_owned(),
            csr: new_csr,
            device_label: "New".to_owned(),
        })
        .unwrap();

        rekey(journal.path(), &old_cid, &old_spki, &body).unwrap();
        let error = rekey(journal.path(), &other_cid, &other_spki, &body).unwrap_err();

        assert_eq!(error.reason, Code::MigrationReplayForbidden);
        assert_ne!(old_key.public_key_der(), other_key.public_key_der());
    }

    #[test]
    fn migration_new_device_leaves_the_previous_row_and_exact_replay_is_stable() {
        let journal = Journal::new();
        add_client(
            journal.path(),
            OLD,
            "Old phone",
            ClientRole::Unknown("x-role".into()),
            None,
        );
        add_client(
            journal.path(),
            ADOPTED,
            "New phone",
            ClientRole::Roleless,
            None,
        );
        add_operation(journal.path(), OLD, ADOPTED, 1);
        let body = body(
            Choice::NewDevice,
            "123e4567-e89b-42d3-a456-426614174020",
            None,
        );

        let first = decide(journal.path(), ADOPTED, &body, &mut direct_boundary).unwrap();
        let replay = decide(journal.path(), ADOPTED, &body, &mut direct_boundary).unwrap();

        assert_eq!(first.state, State::NewDevice);
        assert_eq!(first, replay);
        assert_eq!(entries(journal.path()).len(), 2);
        assert_eq!(
            migration_state(journal.path(), OLD).unwrap().state,
            State::NewDevice
        );
    }

    #[test]
    fn migration_same_device_retires_only_the_stored_previous_cid() {
        let journal = Journal::new();
        add_client(journal.path(), OLD, "Old phone", ClientRole::Roleless, None);
        add_client(
            journal.path(),
            ADOPTED,
            "New phone",
            ClientRole::Roleless,
            None,
        );
        add_client(journal.path(), OTHER, "Other", ClientRole::Roleless, None);
        add_operation(journal.path(), OLD, ADOPTED, 1);
        let body = body(
            Choice::SameDevice,
            "123e4567-e89b-42d3-a456-426614174021",
            None,
        );

        let view = decide(journal.path(), ADOPTED, &body, &mut direct_boundary).unwrap();
        let replay = decide(journal.path(), ADOPTED, &body, &mut direct_boundary).unwrap();

        assert_eq!(view.state, State::SameDevice);
        assert_eq!(view, replay);
        assert_eq!(view.replaced_cid.as_deref(), Some(OLD));
        let rows = entries(journal.path());
        assert!(!rows.iter().any(|entry| entry.fingerprint == OLD));
        assert!(rows.iter().any(|entry| entry.fingerprint == ADOPTED));
        assert!(rows.iter().any(|entry| entry.fingerprint == OTHER));
    }

    #[test]
    fn migration_explicit_replace_checks_self_missing_and_retired_targets() {
        let journal = Journal::new();
        add_client(journal.path(), OLD, "Old phone", ClientRole::Roleless, None);
        add_client(
            journal.path(),
            ADOPTED,
            "New phone",
            ClientRole::Roleless,
            None,
        );
        add_client(journal.path(), OTHER, "Other", ClientRole::Roleless, None);

        let self_body = body(
            Choice::ReplaceDevice,
            "123e4567-e89b-42d3-a456-426614174030",
            Some(ADOPTED),
        );
        assert_eq!(
            decide(journal.path(), ADOPTED, &self_body, &mut direct_boundary)
                .unwrap_err()
                .reason,
            Code::MigrationSelfReplacement
        );
        let missing_body = body(
            Choice::ReplaceDevice,
            "123e4567-e89b-42d3-a456-426614174031",
            Some("sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"),
        );
        assert_eq!(
            decide(journal.path(), ADOPTED, &missing_body, &mut direct_boundary)
                .unwrap_err()
                .reason,
            Code::PairedDeviceNotFound
        );

        let replace_body = body(
            Choice::ReplaceDevice,
            "123e4567-e89b-42d3-a456-426614174032",
            Some(OLD),
        );
        decide(journal.path(), ADOPTED, &replace_body, &mut direct_boundary).unwrap();
        let conflict_body = body(
            Choice::ReplaceDevice,
            "123e4567-e89b-42d3-a456-426614174033",
            Some(OLD),
        );
        assert_eq!(
            decide(
                journal.path(),
                ADOPTED,
                &conflict_body,
                &mut direct_boundary
            )
            .unwrap_err()
            .reason,
            Code::MigrationTargetConflict
        );
    }

    #[test]
    fn replacement_preserves_unrelated_ordinals_roles_and_malformed_activity() {
        let journal = Journal::new();
        let mut old = ClientEntry::new(
            OLD,
            "Phone",
            "2026-09-01T00:00:00Z",
            "id",
            ClientRole::Roleless,
        );
        old.label_ordinal = 7;
        AuthorizationLedger::new(journal.path()).add(old).unwrap();
        let mut selected = ClientEntry::new(
            ADOPTED,
            "adopted",
            "2026-09-01T00:00:00Z",
            "id",
            ClientRole::Unknown("custom-role".into()),
        );
        selected.platform = Some(Platform::Ios);
        AuthorizationLedger::new(journal.path())
            .add(selected.clone())
            .unwrap();
        let mut other = ClientEntry::new(
            OTHER,
            "Phone",
            "2026-09-01T00:00:00Z",
            "id",
            ClientRole::Unknown("other-role".into()),
        );
        other.label_ordinal = 11;
        AuthorizationLedger::new(journal.path())
            .add(other.clone())
            .unwrap();
        let auth_path = authorization_path(journal.path());
        let mut auth_json: Value = serde_json::from_slice(&fs::read(&auth_path).unwrap()).unwrap();
        for row in auth_json.as_array_mut().unwrap() {
            if row["fingerprint"] == OLD {
                row["label_ordinal"] = json!(7);
            } else if row["fingerprint"] == OTHER {
                row["label_ordinal"] = json!(11);
            }
        }
        fs::write(&auth_path, serde_json::to_vec(&auth_json).unwrap()).unwrap();
        let request = RekeyRequest {
            protocol: 1,
            operation_id: "123e4567-e89b-42d3-a456-426614174040".to_owned(),
            csr: "fixture-csr".to_owned(),
            device_label: "Phone".to_owned(),
        };
        let record = OperationRecord {
            caller_cid: ADOPTED.to_owned(),
            operation_id: request.operation_id.clone(),
            raw_body: serde_json::to_vec(&request).unwrap(),
            issued_pem: Some("fixture-cert".to_owned()),
            issued_cid: Some(
                "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
                    .to_owned(),
            ),
            previous_cid: ADOPTED.to_owned(),
            pairing: Some(json!({})),
            checkpoint: OperationCheckpoint::CertRecorded,
            generation: 1,
            order: 1,
        };
        let guard = hold_lock(&authorization_path(journal.path()), LockOptions::default()).unwrap();
        add_operation_row(journal.path(), &guard, &selected, &record).unwrap();
        let activity_path = journal.path().join("link/devices.json");
        fs::write(&activity_path, json!({
            OTHER: {"last_seen_at": "2026-09-01T00:00:00Z", "sources": {"audio": {"unexpected": true}}}
        }).to_string()).unwrap();
        let mut ledger = AuthorizationLedger::new(journal.path());
        ledger.remove_locked(&guard, OLD).unwrap();
        drop(guard);
        let rows = entries(journal.path());
        let added = rows
            .iter()
            .find(|entry| entry.fingerprint == record.issued_cid.as_deref().unwrap())
            .unwrap();
        assert_eq!(added.role.as_wire(), "custom-role");
        assert_eq!(added.platform, Some(Platform::Ios));
        assert_eq!(added.label_ordinal, 1);
        assert_eq!(
            rows.iter()
                .find(|entry| entry.fingerprint == OTHER)
                .unwrap()
                .label_ordinal,
            11
        );
        assert_eq!(
            rows.iter()
                .find(|entry| entry.fingerprint == OTHER)
                .unwrap()
                .role,
            other.role
        );
        assert!(matches!(
            crate::ledger::read_device_activity(&activity_path),
            DeviceActivityRead::Present(_)
        ));
        let activity: Value = serde_json::from_slice(&fs::read(&activity_path).unwrap()).unwrap();
        assert_eq!(
            activity[OTHER]["sources"]["audio"],
            json!({"unexpected": true})
        );
    }

    #[test]
    fn malformed_migration_record_is_left_untouched_and_unavailable() {
        let journal = Journal::new();
        add_client(journal.path(), OLD, "Old", ClientRole::Roleless, None);
        let dir = journal.path().join("link/device-migrations/operations");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("123e4567-e89b-42d3-a456-426614174050.json");
        fs::write(&path, b"{ malformed").unwrap();
        let before = fs::read(&path).unwrap();

        assert_eq!(
            migration_state(journal.path(), OLD).unwrap_err().reason,
            Code::MigrationStateUnavailable
        );
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn migration_state_refuses_unfinished_rekey_records_without_writing() {
        let journal = Journal::new();
        add_client(journal.path(), OLD, "Old", ClientRole::Roleless, None);

        for (operation_id, checkpoint, order) in [
            (
                "123e4567-e89b-42d3-a456-426614174051",
                OperationCheckpoint::Reserved,
                1,
            ),
            (
                "123e4567-e89b-42d3-a456-426614174052",
                OperationCheckpoint::CertRecorded,
                2,
            ),
        ] {
            let request = RekeyRequest {
                protocol: 1,
                operation_id: operation_id.to_owned(),
                csr: "fixture-csr".to_owned(),
                device_label: "Phone".to_owned(),
            };
            let record = OperationRecord {
                caller_cid: OLD.to_owned(),
                operation_id: operation_id.to_owned(),
                raw_body: serde_json::to_vec(&request).unwrap(),
                issued_pem: (checkpoint == OperationCheckpoint::CertRecorded)
                    .then(|| "fixture-certificate".to_owned()),
                issued_cid: (checkpoint == OperationCheckpoint::CertRecorded)
                    .then(|| ADOPTED.to_owned()),
                previous_cid: OLD.to_owned(),
                pairing: (checkpoint == OperationCheckpoint::CertRecorded)
                    .then(|| json!({"fixture": true})),
                checkpoint,
                generation: 1,
                order,
            };
            let guard =
                hold_lock(&authorization_path(journal.path()), LockOptions::default()).unwrap();
            write_operation(journal.path(), &record).unwrap();
            drop(guard);

            let path = journal
                .path()
                .join("link/device-migrations/operations")
                .join(format!("{operation_id}.json"));
            let before = fs::read(&path).unwrap();
            assert_eq!(
                migration_state(journal.path(), OLD).unwrap_err().reason,
                Code::MigrationStateUnavailable
            );
            assert_eq!(fs::read(path).unwrap(), before);
        }
    }

    #[test]
    fn migration_operation_id_with_different_raw_request_conflicts() {
        let journal = Journal::new();
        let (ca, instance_id) = identity(journal.path());
        let (_, old_csr) = csr();
        let old_certificate = sign_csr(&ca, &old_csr, "Old").unwrap();
        let old_cid = old_certificate.cid().to_owned();
        let old_spki = leaf_spki(old_certificate.pem());
        let old_entry = ClientEntry::new(
            old_cid.clone(),
            "Old",
            "2026-09-01T00:00:00Z",
            &instance_id,
            ClientRole::Roleless,
        );
        AuthorizationLedger::new(journal.path())
            .add(old_entry)
            .unwrap();
        let (_, new_csr) = csr();
        let request = RekeyRequest {
            protocol: 1,
            operation_id: "123e4567-e89b-42d3-a456-426614174060".to_owned(),
            csr: new_csr,
            device_label: "Phone".to_owned(),
        };
        let raw = serde_json::to_vec(&request).unwrap();
        rekey(journal.path(), &old_cid, &old_spki, &raw).unwrap();

        let changed = serde_json::to_vec(&RekeyRequest {
            device_label: "Other label".to_owned(),
            ..request
        })
        .unwrap();
        assert_eq!(
            rekey(journal.path(), &old_cid, &old_spki, &changed)
                .unwrap_err()
                .reason,
            Code::MigrationOperationConflict
        );
    }

    #[test]
    fn migration_rekey_label_obeys_existing_nonempty_80_byte_limit() {
        let journal = Journal::new();
        let (ca, instance_id) = identity(journal.path());
        let (_, old_csr) = csr();
        let old_certificate = sign_csr(&ca, &old_csr, "Old").unwrap();
        let old_cid = old_certificate.cid().to_owned();
        let old_spki = leaf_spki(old_certificate.pem());
        AuthorizationLedger::new(journal.path())
            .add(ClientEntry::new(
                old_cid.clone(),
                "Old",
                "2026-09-01T00:00:00Z",
                &instance_id,
                ClientRole::Roleless,
            ))
            .unwrap();
        let (_, new_csr) = csr();
        for (operation_id, device_label) in [
            ("123e4567-e89b-42d3-a456-426614174072", " \t ".to_owned()),
            ("123e4567-e89b-42d3-a456-426614174073", "x".repeat(81)),
            ("123e4567-e89b-42d3-a456-426614174074", "é".repeat(41)),
        ] {
            let raw = serde_json::to_vec(&RekeyRequest {
                protocol: 1,
                operation_id: operation_id.to_owned(),
                csr: new_csr.clone(),
                device_label,
            })
            .unwrap();
            assert_eq!(
                rekey(journal.path(), &old_cid, &old_spki, &raw)
                    .unwrap_err()
                    .reason,
                Code::MigrationRequestInvalid
            );
        }
        assert!(
            read_migration_records(journal.path())
                .unwrap()
                .operations
                .is_empty()
        );
    }

    #[test]
    fn migration_recovers_reserved_and_certificate_recorded_operations() {
        let journal = Journal::new();
        let (ca, _instance_id) = identity(journal.path());
        let (_, old_csr) = csr();
        let old_certificate = sign_csr(&ca, &old_csr, "Old").unwrap();
        let old_cid = old_certificate.cid().to_owned();
        let old_spki = leaf_spki(old_certificate.pem());
        add_client(journal.path(), &old_cid, "Old", ClientRole::Roleless, None);
        let (_, new_csr) = csr();
        let reserved_request = RekeyRequest {
            protocol: 1,
            operation_id: "123e4567-e89b-42d3-a456-426614174061".to_owned(),
            csr: new_csr,
            device_label: "Reserved phone".to_owned(),
        };
        let reserved_raw = serde_json::to_vec(&reserved_request).unwrap();
        let reserved = OperationRecord {
            caller_cid: old_cid.clone(),
            operation_id: reserved_request.operation_id.clone(),
            raw_body: reserved_raw.clone(),
            issued_pem: None,
            issued_cid: None,
            previous_cid: old_cid.clone(),
            pairing: None,
            checkpoint: OperationCheckpoint::Reserved,
            generation: 1,
            order: 1,
        };
        let guard = hold_lock(&authorization_path(journal.path()), LockOptions::default()).unwrap();
        write_operation(journal.path(), &reserved).unwrap();
        drop(guard);

        let first = rekey(journal.path(), &old_cid, &old_spki, &reserved_raw).unwrap();
        assert!(first.created);
        assert_eq!(entries(journal.path()).len(), 2);
        assert_eq!(
            rekey(journal.path(), &old_cid, &old_spki, &reserved_raw)
                .unwrap()
                .response,
            first.response
        );

        let (_, recorded_csr) = csr();
        let recorded_request = RekeyRequest {
            protocol: 1,
            operation_id: "123e4567-e89b-42d3-a456-426614174062".to_owned(),
            csr: recorded_csr.clone(),
            device_label: "Recorded phone".to_owned(),
        };
        let recorded_raw = serde_json::to_vec(&recorded_request).unwrap();
        let recorded_certificate = sign_csr(&ca, &recorded_csr, "Recorded phone").unwrap();
        let stored_pairing = json!({"persisted": "pairing response"});
        let recorded = OperationRecord {
            caller_cid: old_cid.clone(),
            operation_id: recorded_request.operation_id.clone(),
            raw_body: recorded_raw.clone(),
            issued_pem: Some(recorded_certificate.pem().to_owned()),
            issued_cid: Some(recorded_certificate.cid().to_owned()),
            previous_cid: old_cid.clone(),
            pairing: Some(stored_pairing.clone()),
            checkpoint: OperationCheckpoint::CertRecorded,
            generation: 3,
            order: 2,
        };
        let guard = hold_lock(&authorization_path(journal.path()), LockOptions::default()).unwrap();
        write_operation(journal.path(), &recorded).unwrap();
        drop(guard);

        let recovered = rekey(journal.path(), &old_cid, &old_spki, &recorded_raw).unwrap();
        assert!(!recovered.created);
        assert_eq!(recovered.response.cid, recorded_certificate.cid());
        assert_eq!(recovered.response.pairing, stored_pairing);
        assert!(
            entries(journal.path())
                .iter()
                .any(|entry| entry.fingerprint == recorded_certificate.cid())
        );
    }

    #[test]
    fn migration_decision_proof_already_decided_and_replacement_rules() {
        let journal = Journal::new();
        add_client(journal.path(), OLD, "Old", ClientRole::Roleless, None);
        add_client(
            journal.path(),
            ADOPTED,
            "Adopted",
            ClientRole::Roleless,
            None,
        );
        assert_eq!(
            decide(
                journal.path(),
                ADOPTED,
                &body(
                    Choice::SameDevice,
                    "123e4567-e89b-42d3-a456-426614174063",
                    None,
                ),
                &mut direct_boundary,
            )
            .unwrap_err()
            .reason,
            Code::MigrationProofMissing
        );

        add_operation(journal.path(), OLD, ADOPTED, 1);
        decide(
            journal.path(),
            ADOPTED,
            &body(
                Choice::NewDevice,
                "123e4567-e89b-42d3-a456-426614174064",
                None,
            ),
            &mut direct_boundary,
        )
        .unwrap();
        assert_eq!(
            decide(
                journal.path(),
                ADOPTED,
                &body(
                    Choice::SameDevice,
                    "123e4567-e89b-42d3-a456-426614174065",
                    None,
                ),
                &mut direct_boundary,
            )
            .unwrap_err()
            .reason,
            Code::MigrationAlreadyDecided
        );

        let takeover = body(
            Choice::ReplaceDevice,
            "123e4567-e89b-42d3-a456-426614174066",
            Some(OLD),
        );
        assert_eq!(
            decide(journal.path(), ADOPTED, &takeover, &mut direct_boundary)
                .unwrap()
                .state,
            State::ReplacedDevice
        );
    }

    #[test]
    fn migration_decision_id_conflicts_on_bytes_and_forbids_cross_cid_replay() {
        let journal = Journal::new();
        for cid in [OLD, ADOPTED, OTHER] {
            add_client(journal.path(), cid, "phone", ClientRole::Roleless, None);
        }
        let id = "123e4567-e89b-42d3-a456-426614174069";
        let raw = body(Choice::NewDevice, id, None);
        decide(journal.path(), ADOPTED, &raw, &mut direct_boundary).unwrap();

        let changed = body(Choice::ReplaceDevice, id, Some(OLD));
        assert_eq!(
            decide(journal.path(), ADOPTED, &changed, &mut direct_boundary)
                .unwrap_err()
                .reason,
            Code::MigrationOperationConflict
        );
        assert_eq!(
            decide(journal.path(), OTHER, &raw, &mut direct_boundary)
                .unwrap_err()
                .reason,
            Code::MigrationReplayForbidden
        );
        let null_replaces = serde_json::to_vec(&json!({
            "protocol": 1,
            "decision_id": "123e4567-e89b-42d3-a456-426614174070",
            "choice": "new_device",
            "replaces_cid": null
        }))
        .unwrap();
        assert_eq!(
            decide(
                journal.path(),
                ADOPTED,
                &null_replaces,
                &mut direct_boundary
            )
            .unwrap_err()
            .reason,
            Code::MigrationRequestInvalid
        );
    }

    #[test]
    fn migration_prepared_decision_get_is_read_only_and_put_resumes_stored_plan() {
        let journal = Journal::new();
        add_client(
            journal.path(),
            OLD,
            "Selected phone",
            ClientRole::Roleless,
            None,
        );
        add_client(
            journal.path(),
            ADOPTED,
            "Adopted phone",
            ClientRole::Roleless,
            None,
        );
        add_operation(journal.path(), OLD, ADOPTED, 1);
        let raw_body = body(
            Choice::SameDevice,
            "123e4567-e89b-42d3-a456-426614174067",
            None,
        );
        let record = DecisionRecord {
            caller_cid: ADOPTED.to_owned(),
            decision_id: "123e4567-e89b-42d3-a456-426614174067".to_owned(),
            raw_body: raw_body.clone(),
            choice: Choice::SameDevice,
            replaced_cid: Some(OLD.to_owned()),
            operation_id: Some("123e4567-e89b-42d3-a456-426614174000".to_owned()),
            continuity_plan: Some(json!({"stored": "plan"})),
            checkpoint: DecisionCheckpoint::Prepared,
            order: 2,
        };
        let guard = hold_lock(&authorization_path(journal.path()), LockOptions::default()).unwrap();
        write_decision(journal.path(), &record).unwrap();
        drop(guard);
        let path = journal
            .path()
            .join("link/device-migrations/decisions/123e4567-e89b-42d3-a456-426614174067.json");
        let before = fs::read(&path).unwrap();

        assert_eq!(
            decide(
                journal.path(),
                ADOPTED,
                &body(
                    Choice::SameDevice,
                    "123e4567-e89b-42d3-a456-426614174071",
                    None,
                ),
                &mut direct_boundary,
            )
            .unwrap_err()
            .reason,
            Code::MigrationAlreadyDecided
        );
        assert_eq!(
            migration_state(journal.path(), ADOPTED).unwrap_err().reason,
            Code::MigrationStateUnavailable
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        let auth_path = authorization_path(journal.path());
        let auth_before = fs::read(&auth_path).unwrap();
        fs::write(&auth_path, b"{ malformed").unwrap();
        assert_eq!(
            decide(journal.path(), ADOPTED, &raw_body, &mut direct_boundary)
                .unwrap_err()
                .reason,
            Code::MigrationStateUnavailable
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        fs::write(&auth_path, auth_before).unwrap();
        assert_eq!(
            decide(journal.path(), ADOPTED, &raw_body, &mut direct_boundary)
                .unwrap()
                .state,
            State::SameDevice
        );
        assert!(
            !entries(journal.path())
                .iter()
                .any(|entry| entry.fingerprint == OLD)
        );
        let record: DecisionRecord = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(record.continuity_plan, Some(json!({"stored": "plan"})));
        assert_eq!(record.checkpoint, DecisionCheckpoint::AuthRetired);
    }

    #[test]
    fn same_device_name_transfer_survives_a_later_self_report() {
        let journal = Journal::new();
        add_client(
            journal.path(),
            OLD,
            "Selected device",
            ClientRole::Roleless,
            None,
        );
        add_client(
            journal.path(),
            ADOPTED,
            "Adopted device",
            ClientRole::Roleless,
            None,
        );
        add_operation(journal.path(), OLD, ADOPTED, 1);
        let meta = JournalIdentityMeta {
            name: Some("migration-test".to_owned()),
            version: "1".to_owned(),
        };
        put_self_description(
            journal.path(),
            OLD,
            PutSelfDescriptionRequest {
                protocol_version: 1,
                expected_revision: 0,
                reported: Some(ReportedDescription {
                    name: Some("Selected phone name".to_owned()),
                    platform: None,
                    device_type: None,
                    app_id: None,
                    app_version: None,
                }),
            },
            OffsetDateTime::now_utc(),
            meta.clone(),
        )
        .unwrap();
        decide(
            journal.path(),
            ADOPTED,
            &body(
                Choice::SameDevice,
                "123e4567-e89b-42d3-a456-426614174068",
                None,
            ),
            &mut direct_boundary,
        )
        .unwrap();

        let response = put_self_description(
            journal.path(),
            ADOPTED,
            PutSelfDescriptionRequest {
                protocol_version: 1,
                expected_revision: 1,
                reported: Some(ReportedDescription {
                    name: Some("Later self report".to_owned()),
                    platform: Some("ios".to_owned()),
                    device_type: None,
                    app_id: None,
                    app_version: None,
                }),
            },
            OffsetDateTime::now_utc(),
            meta,
        )
        .unwrap();

        assert_eq!(
            response.reported.unwrap().name.as_deref(),
            Some("Later self report")
        );
        assert_eq!(response.owner_label.as_deref(), Some("Selected phone name"));
        assert_eq!(response.display_label, "Selected phone name");
    }
}
