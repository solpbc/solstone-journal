// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Replay-safe orchestration for undoing a committed identify operation.

use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::{Value, json};
use solstone_core_entity::{EncoderIdentity, hold_entity_trust_lock};
use thiserror::Error;

use crate::identify_cluster::state_status_result;
use crate::identify_operations::{
    EventPayload, IDENTIFY_OPERATION_SCHEMA_VERSION, IdentifyOperationError,
    IdentifyOperationEvent, OperationState, TerminalStatus, UNDO_PHASE_ORDER, UndoPhase,
    append_event, fold_operation, load_operations,
};
use crate::identify_undo_phases::{
    UndoPhaseError, empty_undo_report, undo_corrections, undo_entity, undo_labels, undo_sentinel,
    undo_tracker, undo_voiceprints,
};

const CALLER: &str = "speaker_resolve.identify_cluster";

/// Failure reading the ledger or acquiring the operation-wide trust lock.
#[derive(Debug, Error)]
pub enum UndoIdentifyError {
    #[error("identify operation ledger failed: {0}")]
    Ledger(#[from] IdentifyOperationError),
    #[error("entity trust lock failed: {0}")]
    Trust(#[from] solstone_core_entity::EntityTrustLockError),
}

/// Undo one committed identify operation, resuming any durable partial undo.
pub fn undo_identify_operation(
    journal_root: &Path,
    operation_id: &str,
    encoder: &EncoderIdentity,
) -> Result<Value, UndoIdentifyError> {
    let ledger_path = identify_ledger_path(journal_root);
    let Some(state) = current_state(&ledger_path, operation_id)? else {
        return Ok(not_found(operation_id));
    };
    if let Some(result) = already_undone_result(&state) {
        return Ok(result);
    }
    if !matches!(
        state.terminal_status,
        TerminalStatus::Committed | TerminalStatus::Undoing
    ) {
        return Ok(state_status_result(&state));
    }

    let _trust = hold_entity_trust_lock(journal_root)?;
    let Some(state) = current_state(&ledger_path, operation_id)? else {
        return Ok(not_found(operation_id));
    };
    if let Some(result) = already_undone_result(&state) {
        return Ok(result);
    }
    if !matches!(
        state.terminal_status,
        TerminalStatus::Committed | TerminalStatus::Undoing
    ) {
        return Ok(state_status_result(&state));
    }

    let undo_started_at = append_undo_prepared_once(&ledger_path, &state)?;
    for phase in UNDO_PHASE_ORDER {
        let Some(state) = current_state(&ledger_path, operation_id)? else {
            return Ok(not_found(operation_id));
        };
        if state.undo_phase_checkpoints.contains_key(&phase) {
            continue;
        }
        let delta = match run_undo_phase(journal_root, &state, phase, &undo_started_at, encoder) {
            Ok(delta) => delta,
            Err(UndoPhaseError::Voiceprints(error)) => {
                return append_undo_repair_required(
                    &ledger_path,
                    &state,
                    phase,
                    "voiceprint_removal_ambiguous",
                    json!({"voiceprints": 1, "detail": error.to_string()}),
                );
            }
            Err(error) => {
                return undo_recoverable_result(&ledger_path, operation_id, error.to_string());
            }
        };
        let event = undo_event(
            &state,
            format!("{operation_id}:undo_checkpoint:{}", phase.as_str()),
            EventPayload::UndoCheckpoint {
                phase,
                undo_report_delta: delta,
            },
        );
        if let Err(error) = append_event(&ledger_path, &event) {
            return undo_recoverable_result(&ledger_path, operation_id, error.to_string());
        }
    }
    let Some(state) = current_state(&ledger_path, operation_id)? else {
        return Ok(not_found(operation_id));
    };
    let report = aggregate_undo_report(&state, "undone");
    let event = undo_event(
        &state,
        format!("{operation_id}:undo_committed"),
        EventPayload::UndoCommitted {
            undo_report: report.clone(),
        },
    );
    if let Err(error) = append_event(&ledger_path, &event) {
        return undo_recoverable_result(&ledger_path, operation_id, error.to_string());
    }
    Ok(report)
}

fn run_undo_phase(
    journal_root: &Path,
    state: &OperationState,
    phase: UndoPhase,
    undo_started_at: &str,
    encoder: &EncoderIdentity,
) -> Result<Value, UndoPhaseError> {
    match phase {
        UndoPhase::Labels => undo_labels(journal_root, state),
        UndoPhase::Corrections => undo_corrections(journal_root, state, undo_started_at),
        UndoPhase::Voiceprints => undo_voiceprints(journal_root, state, encoder),
        UndoPhase::Tracker => undo_tracker(journal_root, state),
        UndoPhase::Sentinel => undo_sentinel(journal_root, state),
        UndoPhase::Entity => undo_entity(journal_root, state),
    }
}

fn append_undo_prepared_once(
    ledger_path: &Path,
    state: &OperationState,
) -> Result<String, UndoIdentifyError> {
    if let Some(started_at) = state.undo_started_at.clone() {
        return Ok(started_at);
    }
    let undo_started_at = Utc::now().to_rfc3339();
    let event = undo_event(
        state,
        format!("{}:undo_prepared", state.operation_id),
        EventPayload::UndoPrepared {
            undo_started_at: undo_started_at.clone(),
        },
    );
    append_event(ledger_path, &event)?;
    Ok(undo_started_at)
}

fn aggregate_undo_report(state: &OperationState, status: &str) -> Value {
    let mut report = empty_undo_report(&state.operation_id, status);
    let categories = report["undo_report"]
        .as_object_mut()
        .expect("empty undo report has categories");
    for phase in UNDO_PHASE_ORDER {
        let Some(delta) = state.undo_phase_checkpoints.get(&phase) else {
            continue;
        };
        let Some(delta) = delta.as_object() else {
            continue;
        };
        if let Some(value) = delta.get(phase.as_str()).filter(|value| value.is_object()) {
            categories.insert(phase.as_str().to_owned(), value.clone());
        }
    }
    report
}

fn append_undo_repair_required(
    ledger_path: &Path,
    state: &OperationState,
    phase: UndoPhase,
    repair_code: &str,
    repair_categories: Value,
) -> Result<Value, UndoIdentifyError> {
    let Some(current) = current_state(ledger_path, &state.operation_id)? else {
        return Ok(not_found(&state.operation_id));
    };
    if current.terminal_status == TerminalStatus::UndoRepairRequired {
        return Ok(undo_repair_result(
            &current,
            phase,
            repair_code,
            repair_categories,
        ));
    }
    let report = aggregate_undo_report(&current, "undo_repair_required");
    let event = undo_event(
        &current,
        format!(
            "{}:undo_repair_required:{}",
            current.operation_id,
            phase.as_str()
        ),
        EventPayload::UndoRepairRequired {
            phase,
            repair_code: repair_code.to_owned(),
            repair_categories: repair_categories.clone(),
            undo_report: report.clone(),
        },
    );
    append_event(ledger_path, &event)?;
    Ok(json!({
        "status":"undo_repair_required",
        "operation_id":current.operation_id,
        "operation_state":"undo_repair_required",
        "phase":phase.as_str(),
        "repair_code":repair_code,
        "repair_categories":repair_categories,
        "undo_report":report["undo_report"],
    }))
}

fn undo_recoverable_result(
    ledger_path: &Path,
    operation_id: &str,
    detail: String,
) -> Result<Value, UndoIdentifyError> {
    let state = current_state(ledger_path, operation_id)?;
    Ok(json!({
        "status":"recoverable",
        "operation_id":operation_id,
        "operation_state":state.as_ref().map_or("not_found", |state| terminal_name(state.terminal_status)),
        "request_id":state.as_ref().map(|state| state.request_id.clone()),
        "detail":detail,
        "undo_report":state.as_ref().map(|state| aggregate_undo_report(state, "recoverable")["undo_report"].clone()),
    }))
}

fn current_state(
    ledger_path: &Path,
    operation_id: &str,
) -> Result<Option<OperationState>, IdentifyOperationError> {
    fold_operation(&load_operations(ledger_path)?, operation_id)
}

fn undo_event(
    state: &OperationState,
    event_id: String,
    payload: EventPayload,
) -> IdentifyOperationEvent {
    IdentifyOperationEvent {
        schema_version: IDENTIFY_OPERATION_SCHEMA_VERSION,
        event_id,
        operation_id: state.operation_id.clone(),
        request_id: state.request_id.clone(),
        ts: Utc::now().to_rfc3339(),
        caller: CALLER.to_owned(),
        actor: None,
        payload,
    }
}

fn already_undone_result(state: &OperationState) -> Option<Value> {
    (state.terminal_status == TerminalStatus::Undone)
        .then(|| state.undo_report.clone())
        .flatten()
        .map(|mut report| {
            report["status"] = Value::String("already_undone".to_owned());
            report
        })
}

fn undo_repair_result(
    state: &OperationState,
    default_phase: UndoPhase,
    default_code: &str,
    default_categories: Value,
) -> Value {
    let Some(event) = state.undo_repair_required.as_ref() else {
        return json!({"status":"undo_repair_required","operation_id":state.operation_id});
    };
    let EventPayload::UndoRepairRequired {
        phase,
        repair_code,
        repair_categories,
        undo_report,
    } = &event.payload
    else {
        return json!({"status":"undo_repair_required","operation_id":state.operation_id,"phase":default_phase.as_str(),"repair_code":default_code,"repair_categories":default_categories});
    };
    json!({
        "status":"undo_repair_required",
        "operation_id":state.operation_id,
        "operation_state":"undo_repair_required",
        "phase":phase.as_str(),
        "repair_code":repair_code,
        "repair_categories":repair_categories,
        "undo_report":undo_report["undo_report"],
    })
}

fn identify_ledger_path(root: &Path) -> PathBuf {
    root.join("speakers/identify-operations.jsonl")
}

fn not_found(operation_id: &str) -> Value {
    json!({
        "status":"not_found",
        "operation_id":operation_id,
        "list_command":"sol call speakers identify-operations",
    })
}

fn terminal_name(status: TerminalStatus) -> &'static str {
    match status {
        TerminalStatus::InProgress => "in_progress",
        TerminalStatus::Committed => "committed",
        TerminalStatus::RepairRequired => "repair_required",
        TerminalStatus::Undoing => "undoing",
        TerminalStatus::Undone => "undone",
        TerminalStatus::UndoRepairRequired => "undo_repair_required",
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;

    use super::*;
    use crate::identify_operations::{
        EventPayload, ForwardPhase, is_fully_restored_identify_operation,
    };

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-identify-undo-orchestrator-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn encoder() -> EncoderIdentity {
        EncoderIdentity {
            id: "test".to_owned(),
            sha256: "a".repeat(64),
            width: 256,
        }
    }

    fn plan(operation_id: &str) -> Value {
        json!({
            "plan_schema_version":1,
            "operation_id":operation_id,
            "request_id":"request",
            "planned_at":"2026-08-08T00:00:00Z",
            "request":{"cluster_id":1,"name":"Target","entity_id":"target","resolve_only":false,"create_new":false,"entity_type":"Person","reviewed_near_match_entity_ids":[]},
            "cluster":{"member_count":0,"members":[]},
            "target":{"entity_id":"target","entity_name":"Target","will_create":false},
            "entity_identity":{},
            "direct_voiceprints":{},
            "segments":[],
            "retro_confirm":{},
            "sentinel":{},
            "keep_separate_assertions":[],
        })
    }

    fn append(root: &Path, operation_id: &str, payload: EventPayload, suffix: &str) {
        let event = IdentifyOperationEvent {
            schema_version: IDENTIFY_OPERATION_SCHEMA_VERSION,
            event_id: format!("{operation_id}:{suffix}"),
            operation_id: operation_id.to_owned(),
            request_id: "request".to_owned(),
            ts: "2026-08-08T00:00:00Z".to_owned(),
            caller: CALLER.to_owned(),
            actor: None,
            payload,
        };
        append_event(&identify_ledger_path(root), &event).unwrap();
    }

    fn checkpoint(phase: ForwardPhase) -> Value {
        let base = json!({"phase_status":"complete","completed_at":"2026-08-08T00:00:00Z","counts":{},"skipped_reasons":{}});
        let mut object = base.as_object().unwrap().clone();
        match phase {
            ForwardPhase::Entity => object.extend(json!({"entity_id":"target","entity_created":false,"identity_after_hash":"hash","history_event_refs":[]}).as_object().unwrap().clone()),
            ForwardPhase::KeepSeparate => object.extend(json!({"pair_keys":[],"recorded_count":0,"already_present_count":0}).as_object().unwrap().clone()),
            ForwardPhase::DirectVoiceprints => object.extend(json!({"saved_keys":[],"saved_count":0,"skipped_existing_count":0}).as_object().unwrap().clone()),
            ForwardPhase::Corrections => object.extend(json!({"appended_keys":[],"appended_count":0,"skipped_existing_count":0,"segment_count":0}).as_object().unwrap().clone()),
            ForwardPhase::Labels => object.extend(json!({"patched_sentence_keys":[],"inserted_sentence_keys":[],"patched_count":0,"inserted_count":0,"skipped_already_intended_count":0,"segment_count":0}).as_object().unwrap().clone()),
            ForwardPhase::RetroTracker => object.extend(json!({"matched":false,"candidate_id":null,"saved_keys":[],"voiceprints_saved_count":0,"voiceprints_skipped_existing_count":0,"tracker_updated":false}).as_object().unwrap().clone()),
            ForwardPhase::Sentinel => object.extend(json!({"cluster_key":"1","written":false}).as_object().unwrap().clone()),
        }
        Value::Object(object)
    }

    fn committed_operation(root: &Path, operation_id: &str) {
        append(
            root,
            operation_id,
            EventPayload::Prepared {
                request_fingerprint: "a".repeat(64),
                prepared_plan: plan(operation_id),
            },
            "prepared",
        );
        for phase in crate::identify_operations::FORWARD_PHASE_ORDER {
            append(
                root,
                operation_id,
                EventPayload::Checkpoint {
                    phase,
                    checkpoint: checkpoint(phase),
                },
                &format!("checkpoint:{}", phase.as_str()),
            );
        }
        append(
            root,
            operation_id,
            EventPayload::Committed {
                result: json!({"status":"identified"}),
            },
            "committed",
        );
    }

    #[test]
    fn ac9_repair_required_undo_returns_completed_forward_phases() {
        let temporary = Temp::new();
        let operation_id = "idop_repair";
        append(
            &temporary.0,
            operation_id,
            EventPayload::Prepared {
                request_fingerprint: "a".repeat(64),
                prepared_plan: plan(operation_id),
            },
            "prepared",
        );
        append(
            &temporary.0,
            operation_id,
            EventPayload::Checkpoint {
                phase: ForwardPhase::Entity,
                checkpoint: checkpoint(ForwardPhase::Entity),
            },
            "checkpoint:entity",
        );
        append(
            &temporary.0,
            operation_id,
            EventPayload::RepairRequired {
                phase: ForwardPhase::Labels,
                repair_code: "concurrent_change".into(),
                repair_categories: json!({"labels":1}),
                partial_report: json!({"completed_phases":["entity"],"pending_phases":["labels"]}),
            },
            "repair_required:labels",
        );

        let result = undo_identify_operation(&temporary.0, operation_id, &encoder()).unwrap();
        assert_eq!(result["status"], "repair_required");
        assert_eq!(result["completed_phases"], json!(["entity"]));
        assert_eq!(result["pending_phases"], json!(["labels"]));
    }

    #[test]
    fn undo_round_trip_commits_a_fully_restored_operation() {
        let temporary = Temp::new();
        let operation_id = "idop_round_trip";
        committed_operation(&temporary.0, operation_id);

        let result = undo_identify_operation(&temporary.0, operation_id, &encoder()).unwrap();
        assert_eq!(result["status"], "undone");
        let state = current_state(&identify_ledger_path(&temporary.0), operation_id)
            .unwrap()
            .unwrap();
        assert_eq!(state.terminal_status, TerminalStatus::Undone);
        assert!(is_fully_restored_identify_operation(&state));
    }

    #[test]
    fn undo_already_undone_returns_saved_report_without_new_events() {
        let temporary = Temp::new();
        let operation_id = "idop_again";
        committed_operation(&temporary.0, operation_id);
        let first = undo_identify_operation(&temporary.0, operation_id, &encoder()).unwrap();
        let before = load_operations(&identify_ledger_path(&temporary.0))
            .unwrap()
            .len();
        let second = undo_identify_operation(&temporary.0, operation_id, &encoder()).unwrap();
        let after = load_operations(&identify_ledger_path(&temporary.0))
            .unwrap()
            .len();

        assert_eq!(second["status"], "already_undone");
        assert_eq!(second["undo_report"], first["undo_report"]);
        assert_eq!(before, after);
    }

    #[test]
    fn undo_resume_skips_already_checkpointed_phases() {
        let temporary = Temp::new();
        let operation_id = "idop_resume";
        committed_operation(&temporary.0, operation_id);
        append(
            &temporary.0,
            operation_id,
            EventPayload::UndoPrepared {
                undo_started_at: "2026-08-08T00:00:01Z".into(),
            },
            "undo_prepared",
        );
        let labels = empty_undo_report(operation_id, "undone")["undo_report"]["labels"].clone();
        append(
            &temporary.0,
            operation_id,
            EventPayload::UndoCheckpoint {
                phase: UndoPhase::Labels,
                undo_report_delta: json!({"labels":labels}),
            },
            "undo_checkpoint:labels",
        );

        let result = undo_identify_operation(&temporary.0, operation_id, &encoder()).unwrap();
        assert_eq!(result["status"], "undone");
        let rows = load_operations(&identify_ledger_path(&temporary.0)).unwrap();
        assert_eq!(rows.iter().filter(|row| row.event.event_id == format!("{operation_id}:undo_checkpoint:labels")).count(), 1);
        let state = fold_operation(&rows, operation_id).unwrap().unwrap();
        assert_eq!(state.undo_phase_checkpoints.len(), UNDO_PHASE_ORDER.len());
    }
}
