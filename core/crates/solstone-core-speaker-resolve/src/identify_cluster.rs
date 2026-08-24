// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Prepared-plan assembly and replay-safe identify operation orchestration.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::{Value, json};
use solstone_core_entity::{
    EncoderIdentity, VoiceprintItem, hold_entity_trust_lock, normalize_embedding,
    read_entity_identity,
};
use solstone_core_journal_io::segment_path;
use thiserror::Error;

use crate::candidate_tracker::{CandidateTracker, MERGE_THRESHOLD, best_matching_candidate};
use crate::direct_voiceprints::{
    DirectVoiceprintEntry, DirectVoiceprintKey, DirectVoiceprintsPlan,
    execute_direct_voiceprints_phase, plan_direct_voiceprints,
};
use crate::discovery_cache::{canonical_members, load_discovery_cache};
use crate::eligibility::{current_principal_id, speaker_attach_rejection_reason};
use crate::identify_forward_phases::{
    EntityPhasePlan, ForwardPhaseError, KeepSeparatePhaseEntry, LabelPlanItem,
    RetroTrackerPhasePlan, RetroVoiceprintEntry, SegmentCorrectionPlan, SegmentLabelPlan,
    SentinelPhasePlan, load_corrections, load_labels, load_resolved_clusters, phase_corrections,
    phase_entity, phase_keep_separate, phase_labels, phase_retro_tracker, phase_sentinel,
};
use crate::identify_operations::{
    EventPayload, FORWARD_PHASE_ORDER, ForwardPhase, IDENTIFY_OPERATION_SCHEMA_VERSION,
    IdentifyOperationError, IdentifyOperationEvent, MemberProvenance, OperationState,
    TerminalStatus, append_event, fold_all_operations, fold_operation, load_operations,
    operation_id_for_request, request_fingerprint,
};
use crate::identify_target::{
    IdentifyTargetOutcome, IdentifyTargetRequest, TargetResolution, resolve_identify_target,
};
use crate::keep_separate::pair_key;
use crate::owner_admission::{OWNER_IDENTITY_INVALID_REASON, OwnerAdmission, admitted_owner_id};
use crate::retroactive_confirm::plan_retroactive_confirm;

const CALLER: &str = "speaker_resolve.identify_cluster";

/// Raw inputs accepted by the native identify orchestrator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentifyClusterRequest {
    pub journal_root: PathBuf,
    pub cluster_id: i64,
    pub name: Option<String>,
    pub entity_id: Option<String>,
    pub resolve_only: bool,
    pub create_new: bool,
    pub entity_type: String,
    pub request_id: String,
    pub reviewed_near_match_entity_ids: Vec<String>,
    pub caller: String,
    pub actor: Option<String>,
}

/// Failure from planning or running an identify operation.
#[derive(Debug, Error)]
pub enum IdentifyClusterError {
    #[error("identify operation ledger failed: {0}")]
    Ledger(#[from] IdentifyOperationError),
    #[error("target resolution failed: {0}")]
    Target(#[from] crate::identify_target::IdentifyTargetError),
    #[error("candidate tracker failed: {0}")]
    Tracker(#[from] crate::candidate_tracker::CandidateTrackerError),
    #[error("entity store failed: {0}")]
    Entity(#[from] solstone_core_entity::EntityStoreError),
    #[error("speaker attach eligibility failed: {0}")]
    Eligibility(#[from] crate::eligibility::EligibilityError),
    #[error("entity trust lock failed: {0}")]
    Trust(#[from] solstone_core_entity::EntityTrustLockError),
}

#[derive(Debug, Clone)]
struct PlannedIdentify {
    fingerprint: String,
    prepared_plan: Value,
}

#[derive(Debug)]
enum ExecuteError {
    Repair {
        phase: ForwardPhase,
        code: &'static str,
        categories: BTreeMap<String, usize>,
        partial_report: Option<Value>,
    },
    Unexpected(String),
}

/// Build per-segment label/correction snapshots for an identify prepared plan.
pub fn segment_plans(
    journal_root: &Path,
    target_id: &str,
    cluster_members: &[MemberProvenance],
    timestamp: i64,
    operation_id: &str,
) -> Vec<Value> {
    let mut grouped = BTreeMap::<(String, String, String), BTreeSet<i64>>::new();
    let mut sources = BTreeMap::<(String, String, String), BTreeSet<String>>::new();
    for member in cluster_members {
        let key = (
            member.day.clone(),
            member.stream.clone(),
            member.segment_key.clone(),
        );
        grouped
            .entry(key.clone())
            .or_default()
            .insert(member.sentence_id);
        sources
            .entry(key)
            .or_default()
            .insert(member.source.clone());
    }

    grouped
        .into_iter()
        .filter_map(|((day, stream, segment_key), sentence_ids)| {
            let directory = segment_path(journal_root, &day, &segment_key, &stream, false).ok()?;
            if !directory.is_dir() {
                return None;
            }
            let labels = load_labels(&directory);
            let corrections = load_corrections(&directory);
            let existing_keys = corrections
                .iter()
                .filter_map(|row| {
                    row.get("sentence_id").and_then(Value::as_i64).map(|sentence_id| {
                        json!({"sentence_id":sentence_id,"corrected_speaker":row.get("corrected_speaker").cloned().unwrap_or(Value::Null)})
                    })
                })
                .collect::<Vec<_>>();
            let existing = existing_keys
                .iter()
                .filter_map(|row| {
                    Some((
                        row.get("sentence_id")?.as_i64()?,
                        row.get("corrected_speaker").cloned().unwrap_or(Value::Null),
                    ))
                })
                .collect::<HashSet<_>>();
            let mut label_entries = Vec::new();
            let mut rows_to_append = Vec::new();
            for sentence_id in sentence_ids {
                let prior = labels.get(&sentence_id).cloned();
                let intended = json!({"sentence_id":sentence_id,"speaker":target_id,"confidence":"high","method":"user_identified"});
                label_entries.push(json!({
                    "sentence_id": sentence_id,
                    "prior_state": if prior.is_some() { "present" } else { "absent" },
                    "prior_label": prior,
                    "intended_label": intended,
                }));
                if !existing.contains(&(sentence_id, Value::String(target_id.to_owned()))) {
                    let original = labels.get(&sentence_id);
                    rows_to_append.push(json!({
                        "sentence_id":sentence_id,
                        "original_speaker":original.and_then(|label| label.get("speaker")).cloned().unwrap_or(Value::Null),
                        "corrected_speaker":target_id,
                        "original_method":original.and_then(|label| label.get("method")).cloned().unwrap_or(Value::Null),
                        "timestamp":timestamp,
                        "operation_id":operation_id,
                        "correction_kind":"identify",
                    }));
                }
            }
            let sources = sources.remove(&(day.clone(), stream.clone(), segment_key.clone()))?;
            let sources = sources.into_iter().collect::<Vec<_>>();
            let source = sources.first()?.clone();
            Some(json!({
                "day":day,"stream":stream,"segment_key":segment_key,"source":source,"sources":sources,
                "labels":label_entries,
                "corrections":{"existing_keys":existing_keys,"rows_to_append":rows_to_append},
            }))
        })
        .collect()
}

/// Execute an identify request, resuming its append-only operation ledger when needed.
pub fn identify_cluster(
    request: &IdentifyClusterRequest,
    encoder: &EncoderIdentity,
) -> Result<Value, IdentifyClusterError> {
    let operation_id = operation_id_for_request(&request.request_id)?;
    if request.resolve_only {
        return resolve_only_result(request);
    }

    let _trust = hold_entity_trust_lock(&request.journal_root)?;
    let ledger_path = identify_ledger_path(&request.journal_root);
    let rows = load_operations(&ledger_path)?;
    let state = fold_operation(&rows, &operation_id)?;
    let prepared_plan = if let Some(state) = state.as_ref() {
        match state.terminal_status {
            TerminalStatus::RepairRequired => {
                let Some(repair) = state.repair_required.as_ref() else {
                    return Ok(state_status_result(state));
                };
                let EventPayload::RepairRequired {
                    phase, repair_code, ..
                } = &repair.payload
                else {
                    return Ok(state_status_result(state));
                };
                if repair_code != OWNER_IDENTITY_INVALID_REASON
                    || !matches!(
                        admitted_owner_id(&request.journal_root),
                        OwnerAdmission::Admitted(_)
                    )
                {
                    return Ok(state_status_result(state));
                }
                if !request_matches_state(request, &operation_id, state)? {
                    return Ok(fingerprint_conflict_result(&operation_id, state));
                }
                let resume_event = event(
                    request,
                    &operation_id,
                    format!("{}:resumed", repair.event_id),
                    EventPayload::RepairResumed {
                        repair_event_id: repair.event_id.clone(),
                        phase: *phase,
                    },
                );
                append_event(&ledger_path, &resume_event)?;
                let Some(resumed) = fold_operation(&load_operations(&ledger_path)?, &operation_id)?
                else {
                    return recoverable_result(
                        &ledger_path,
                        &operation_id,
                        "resumed operation disappeared".to_owned(),
                    );
                };
                if resumed.terminal_status != TerminalStatus::InProgress {
                    return Ok(state_status_result(&resumed));
                }
                resumed.prepared_plan
            }
            _ => {
                if !request_matches_state(request, &operation_id, state)? {
                    return Ok(fingerprint_conflict_result(&operation_id, state));
                }
                if state.terminal_status == TerminalStatus::InProgress {
                    state.prepared_plan.clone()
                } else {
                    return Ok(state_status_result(state));
                }
            }
        }
    } else {
        let planned = match plan_identify(request, &operation_id)? {
            Ok(planned) => planned,
            Err(early) => return Ok(early),
        };
        for other in fold_all_operations(&rows)? {
            if other.operation_id == operation_id
                || other.terminal_status != TerminalStatus::Committed
            {
                continue;
            }
            if other.cluster_member_set
                != members_from_plan(&planned.prepared_plan)
                    .into_iter()
                    .collect()
            {
                continue;
            }
            if other.target_entity_id
                == planned.prepared_plan["target"]["entity_id"]
                    .as_str()
                    .map(str::to_owned)
            {
                return Ok(other.result.unwrap_or_else(|| {
                    json!({"status":"identified","operation_id":other.operation_id,"operation_state":"committed"})
                }));
            }
            return Ok(
                json!({"status":"conflict","operation_id":operation_id,"operation_state":"not_prepared","conflict_code":"member_set_target_conflict","conflicting_operation_id":other.operation_id}),
            );
        }
        append_prepared(&ledger_path, request, &operation_id, &planned)?;
        planned.prepared_plan
    };

    match execute_forward(&ledger_path, request, &prepared_plan, encoder) {
        Ok(result) => Ok(result),
        Err(ExecuteError::Repair {
            phase,
            code,
            categories,
            partial_report,
        }) => {
            let repair_categories = Value::Object(
                categories
                    .into_iter()
                    .map(|(key, value)| (key, json!(value)))
                    .collect(),
            );
            let completed_phases = current_phase_names(&ledger_path, &operation_id)?;
            let pending_phases = pending_phase_names(&completed_phases);
            let partial_report = match partial_report {
                None => json!({"pending_phases": pending_phases}),
                Some(value) => value,
            };
            let event = event(
                request,
                &operation_id,
                next_repair_event_id(&ledger_path, &operation_id, phase)?,
                EventPayload::RepairRequired {
                    phase,
                    repair_code: code.to_owned(),
                    repair_categories: repair_categories.clone(),
                    partial_report,
                },
            );
            append_event(&ledger_path, &event)?;
            Ok(json!({
                "status":"repair_required",
                "operation_id":operation_id,
                "operation_state":"repair_required",
                "phase":phase.as_str(),
                "repair_code":code,
                "repair_categories":repair_categories,
                "completed_phases":completed_phases,
                "pending_phases":pending_phases,
            }))
        }
        Err(ExecuteError::Unexpected(error)) => {
            recoverable_result(&ledger_path, &operation_id, error)
        }
    }
}

fn resolve_only_result(request: &IdentifyClusterRequest) -> Result<Value, IdentifyClusterError> {
    let outcome = resolve_identify_target(&target_request(request))?;
    Ok(target_outcome_value(outcome))
}

fn request_matches_state(
    request: &IdentifyClusterRequest,
    operation_id: &str,
    state: &OperationState,
) -> Result<bool, IdentifyClusterError> {
    if stored_request_matches_raw(&state.prepared_plan, request) {
        return Ok(true);
    }
    Ok(matches!(
        plan_identify(request, operation_id)?,
        Ok(planned) if planned.fingerprint == state.request_fingerprint
    ))
}

fn plan_identify(
    request: &IdentifyClusterRequest,
    operation_id: &str,
) -> Result<Result<PlannedIdentify, Value>, IdentifyClusterError> {
    let Some(cache) = load_discovery_cache(&request.journal_root) else {
        return Ok(Err(
            json!({"error":"Invalid discovery cache. Run scan again."}),
        ));
    };
    let Some(raw_members) = cache
        .get("clusters")
        .and_then(Value::as_object)
        .and_then(|clusters| clusters.get(&request.cluster_id.to_string()))
        .and_then(Value::as_array)
    else {
        return Ok(Err(
            json!({"error":format!("Cluster {} not found in scan results.", request.cluster_id)}),
        ));
    };
    if raw_members.is_empty() {
        return Ok(Err(
            json!({"error":format!("Cluster {} not found in scan results.", request.cluster_id)}),
        ));
    }
    let members = match canonical_members(raw_members) {
        Ok(members) => members,
        Err(_) => {
            return Ok(Err(
                json!({"error":"Invalid discovery cache. Run scan again."}),
            ));
        }
    };
    let target = match resolve_identify_target(&target_request(request))? {
        IdentifyTargetOutcome::Ready(target) => target,
        outcome => return Ok(Err(target_outcome_value(outcome))),
    };
    if !target.will_create && !request.reviewed_near_match_entity_ids.is_empty() {
        return Ok(Err(
            json!({"status":"invalid_request","error":"reviewed_near_match_entity_ids is only valid for create"}),
        ));
    }
    let planned_at = Utc::now().to_rfc3339();
    let added_at = Utc::now().timestamp_millis();
    let direct =
        match plan_direct_voiceprints(&request.journal_root, &target.entity_id, &members, added_at)
        {
            Ok(plan) => plan,
            Err(error) => {
                return Ok(Err(
                    json!({"status":"recoverable","error":error.to_string()}),
                ));
            }
        };
    let planning_owner_entity_id = match admitted_owner_id(&request.journal_root) {
        OwnerAdmission::Admitted(id) => id,
        OwnerAdmission::Invalid => {
            return Ok(Err(
                json!({"status":"recoverable","error":OWNER_IDENTITY_INVALID_REASON}),
            ));
        }
    };
    let assertions = match validate_near_matches(request, &target, operation_id)? {
        Ok(assertions) => assertions,
        Err(early) => return Ok(Err(early)),
    };
    let retro = build_retro_plan(
        &request.journal_root,
        &target.entity_id,
        &direct.items,
        added_at,
        &planning_owner_entity_id,
    )?;
    let resolved = load_resolved_clusters(&request.journal_root);
    let prior_identity = read_entity_identity(&request.journal_root, &target.entity_id)?
        .map(|identity| identity.value().clone());
    let intended_identity = if target.will_create {
        json!({"id":target.entity_id,"name":target.entity_name,"type":target.entity_type})
    } else {
        prior_identity.clone().unwrap_or_else(
            || json!({"id":target.entity_id,"name":target.entity_name,"type":target.entity_type}),
        )
    };
    let cluster_key = request.cluster_id.to_string();
    let fingerprint = request_fingerprint(
        &members,
        &target.entity_id,
        target.will_create,
        &target.entity_type,
        &request.reviewed_near_match_entity_ids,
    );
    let plan = json!({
        "plan_schema_version":1,
        "operation_id":operation_id,
        "request_id":request.request_id,
        "planned_at":planned_at,
        "request":raw_request(request),
        "cluster":{"cluster_id":request.cluster_id,"member_count":members.len(),"members":members.iter().map(member_json).collect::<Vec<_>>()},
        "target":{"entity_id":target.entity_id,"entity_name":target.entity_name,"entity_type":target.entity_type,"will_create":target.will_create},
        "entity_identity":{"prior_identity":prior_identity,"intended_identity":intended_identity,"expected_history_operation":{"operation_kind":"speaker_identify","operation_id":operation_id}},
        "direct_voiceprints":direct_plan_json(&direct.plan),
        "segments":segment_plans(&request.journal_root, &target.entity_id, &members, added_at, operation_id),
        "retro_confirm":retro,
        "sentinel":{"cluster_key":cluster_key,"prior_entry":resolved.get(&cluster_key).cloned(),"intended_entry":{"entity_id":target.entity_id,"label":target.entity_name,"ts":planned_at}},
        "keep_separate_assertions":assertions,
    });
    Ok(Ok(PlannedIdentify {
        fingerprint,
        prepared_plan: plan,
    }))
}

fn execute_forward(
    ledger_path: &Path,
    request: &IdentifyClusterRequest,
    prepared_plan: &Value,
    encoder: &EncoderIdentity,
) -> Result<Value, ExecuteError> {
    let operation_id = required_string(prepared_plan, "operation_id")?;
    let request_id = required_string(prepared_plan, "request_id")?;
    for phase in FORWARD_PHASE_ORDER {
        let rows = load_operations(ledger_path)
            .map_err(|error| ExecuteError::Unexpected(error.to_string()))?;
        if fold_operation(&rows, &operation_id)
            .map_err(|error| ExecuteError::Unexpected(error.to_string()))?
            .is_some_and(|state| state.phase_checkpoints.contains_key(&phase))
        {
            continue;
        }
        let mut checkpoint = run_phase(phase, &request.journal_root, prepared_plan, encoder)?;
        let checkpoint_object = checkpoint.as_object_mut().ok_or_else(|| {
            ExecuteError::Unexpected("phase checkpoint is not an object".to_owned())
        })?;
        checkpoint_object.insert(
            "phase_status".to_owned(),
            Value::String("complete".to_owned()),
        );
        checkpoint_object.insert(
            "completed_at".to_owned(),
            Value::String(Utc::now().to_rfc3339()),
        );
        let event = event(
            request,
            &operation_id,
            format!("{operation_id}:checkpoint:{}", phase.as_str()),
            EventPayload::Checkpoint { phase, checkpoint },
        );
        append_event(ledger_path, &event)
            .map_err(|error| ExecuteError::Unexpected(error.to_string()))?;
    }
    let rows = load_operations(ledger_path)
        .map_err(|error| ExecuteError::Unexpected(error.to_string()))?;
    let state = fold_operation(&rows, &operation_id)
        .map_err(|error| ExecuteError::Unexpected(error.to_string()))?
        .ok_or_else(|| ExecuteError::Unexpected("prepared operation disappeared".to_owned()))?;
    let result = forward_success_result(prepared_plan, &state.phase_checkpoints);
    let event = event(
        request,
        &operation_id,
        format!("{operation_id}:committed"),
        EventPayload::Committed {
            result: result.clone(),
        },
    );
    append_event(ledger_path, &event)
        .map_err(|error| ExecuteError::Unexpected(error.to_string()))?;
    let _ = request_id;
    Ok(result)
}

fn run_phase(
    phase: ForwardPhase,
    root: &Path,
    plan: &Value,
    encoder: &EncoderIdentity,
) -> Result<Value, ExecuteError> {
    if phase == ForwardPhase::DirectVoiceprints {
        return execute_direct_voiceprints_phase(root, &direct_phase_plan(plan)?, encoder)
            .map(|value| value.checkpoint_fields())
            .map_err(|error| match error {
                crate::direct_voiceprints::DirectVoiceprintsError::RepairRequired {
                    phase,
                    code,
                    categories,
                    partial_report,
                } => ExecuteError::Repair {
                    phase,
                    code,
                    categories,
                    partial_report,
                },
                other => ExecuteError::Unexpected(other.to_string()),
            });
    }
    let result = match phase {
        ForwardPhase::Entity => {
            phase_entity(root, &entity_phase_plan(plan)?).map(|value| value.fields)
        }
        ForwardPhase::KeepSeparate => phase_keep_separate(
            root,
            &required_string(plan, "operation_id")?,
            &keep_separate_entries(plan)?,
        )
        .map(|value| value.fields),
        ForwardPhase::DirectVoiceprints => unreachable!("handled before shared phase mapping"),
        ForwardPhase::Corrections => phase_corrections(
            root,
            &required_string(plan, "operation_id")?,
            &correction_plans(plan)?,
        )
        .map(|value| value.fields),
        ForwardPhase::Labels => phase_labels(root, &label_plans(plan)?).map(|value| value.fields),
        ForwardPhase::RetroTracker => {
            let mut tracker = CandidateTracker::new(root);
            phase_retro_tracker(root, &mut tracker, &retro_phase_plan(plan)?, encoder)
                .map(|value| value.fields)
        }
        ForwardPhase::Sentinel => {
            phase_sentinel(root, &sentinel_phase_plan(plan)?).map(|value| value.fields)
        }
    };
    result.map_err(map_forward_error)
}

fn append_prepared(
    path: &Path,
    request: &IdentifyClusterRequest,
    operation_id: &str,
    planned: &PlannedIdentify,
) -> Result<(), IdentifyClusterError> {
    let mut event = event(
        request,
        operation_id,
        format!("{operation_id}:prepared"),
        EventPayload::Prepared {
            request_fingerprint: planned.fingerprint.clone(),
            prepared_plan: planned.prepared_plan.clone(),
        },
    );
    event.ts = planned.prepared_plan["planned_at"]
        .as_str()
        .unwrap_or(&event.ts)
        .to_owned();
    append_event(path, &event)?;
    Ok(())
}

fn event(
    request: &IdentifyClusterRequest,
    operation_id: &str,
    event_id: String,
    payload: EventPayload,
) -> IdentifyOperationEvent {
    IdentifyOperationEvent {
        schema_version: IDENTIFY_OPERATION_SCHEMA_VERSION,
        event_id,
        operation_id: operation_id.to_owned(),
        request_id: request.request_id.clone(),
        ts: Utc::now().to_rfc3339(),
        caller: if request.caller.is_empty() {
            CALLER.to_owned()
        } else {
            request.caller.clone()
        },
        actor: request.actor.clone(),
        payload,
    }
}

fn next_repair_event_id(
    ledger_path: &Path,
    operation_id: &str,
    phase: ForwardPhase,
) -> Result<String, IdentifyClusterError> {
    let attempts = load_operations(ledger_path)?
        .iter()
        .filter(|row| {
            row.event.operation_id == operation_id
                && matches!(
                    &row.event.payload,
                    EventPayload::RepairRequired { phase: repair_phase, .. } if *repair_phase == phase
                )
        })
        .count();
    let base = format!("{operation_id}:repair_required:{}", phase.as_str());
    Ok(if attempts == 0 {
        base
    } else {
        format!("{base}:retry:{attempts}")
    })
}

fn target_request(request: &IdentifyClusterRequest) -> IdentifyTargetRequest {
    IdentifyTargetRequest {
        journal_root: request.journal_root.clone(),
        cluster_id: request.cluster_id,
        name: request.name.clone(),
        entity_id: request.entity_id.clone(),
        resolve_only: request.resolve_only,
        create_new: request.create_new,
        entity_type: request.entity_type.clone(),
        reviewed_near_match_entity_ids: request.reviewed_near_match_entity_ids.clone(),
    }
}

fn target_outcome_value(outcome: IdentifyTargetOutcome) -> Value {
    match outcome {
        IdentifyTargetOutcome::Ready(target) => {
            json!({"status":"ready","entity_id":target.entity_id,"entity_name":target.entity_name})
        }
        IdentifyTargetOutcome::Resolved {
            entity_id,
            entity_name,
            has_voice,
        } => {
            json!({"status":"resolved","entity_id":entity_id,"entity_name":entity_name,"has_voice":has_voice})
        }
        IdentifyTargetOutcome::Ambiguous {
            ambiguity_id,
            candidates,
        } => {
            json!({"status":"ambiguous","ambiguity_id":ambiguity_id,"candidates":candidates.iter().map(candidate_json).collect::<Vec<_>>() })
        }
        IdentifyTargetOutcome::NoMatch { candidates } => {
            json!({"status":"no_match","candidates":candidates.iter().map(candidate_json).collect::<Vec<_>>() })
        }
        IdentifyTargetOutcome::PrincipalMatch => {
            json!({"status":"principal_match","this_is_me":true})
        }
        IdentifyTargetOutcome::NameUnavailable => {
            json!({"status":"invalid_request","error":"name is unavailable"})
        }
        IdentifyTargetOutcome::NameRequired => json!({"error":"name is required"}),
        IdentifyTargetOutcome::DestinationOccupied { entity_id } => {
            json!({"status":"destination_occupied","entity_id":entity_id,"error":format!("Entity id '{entity_id}' already exists.")})
        }
        IdentifyTargetOutcome::EntityNotFound { entity_id } => {
            json!({"error":format!("Entity '{entity_id}' not found."),"not_found":true})
        }
        IdentifyTargetOutcome::NonPersonEntity {
            entity_id,
            entity_type,
        } => {
            json!({"status":"invalid_request","error":"target entity is not an admissible Person","entity_id":entity_id,"entity_type":entity_type})
        }
        IdentifyTargetOutcome::InvalidEntityType { entity_type } => {
            json!({"error":format!("Invalid entity type: {entity_type}"),"invalid_entity_type":true})
        }
        IdentifyTargetOutcome::NonPersonCreateType { entity_type } => {
            json!({"status":"invalid_request","error":"target entity type is not an admissible Person","entity_type":entity_type})
        }
    }
}

fn candidate_json(candidate: &crate::identify_target::IdentifyCandidateRow) -> Value {
    json!({"id":candidate.id,"name":candidate.name,"tier":candidate.tier,"score":candidate.score,"has_voice":candidate.has_voice})
}
fn member_json(member: &MemberProvenance) -> Value {
    json!({"day":member.day,"stream":member.stream,"segment_key":member.segment_key,"source":member.source,"sentence_id":member.sentence_id})
}
fn raw_request(request: &IdentifyClusterRequest) -> Value {
    let mut reviewed_ids = request.reviewed_near_match_entity_ids.clone();
    reviewed_ids.sort();
    json!({"cluster_id":request.cluster_id,"name":request.name.as_deref().map(str::trim).filter(|v| !v.is_empty()),"entity_id":request.entity_id.as_deref().map(str::trim).filter(|v| !v.is_empty()),"resolve_only":false,"create_new":request.create_new,"entity_type":request.entity_type,"reviewed_near_match_entity_ids":reviewed_ids})
}
fn identify_ledger_path(root: &Path) -> PathBuf {
    root.join("speakers/identify-operations.jsonl")
}

fn validate_near_matches(
    request: &IdentifyClusterRequest,
    target: &TargetResolution,
    operation_id: &str,
) -> Result<Result<Vec<Value>, Value>, IdentifyClusterError> {
    if request.reviewed_near_match_entity_ids.is_empty() {
        if target.visible_candidate_ids.is_empty() {
            return Ok(Ok(Vec::new()));
        }
        return Ok(Err(json!({
            "status":"invalid_request",
            "error":"reviewed_near_match_entity_ids must match shown near matches",
            "invalid_request_code":"reviewed_near_match_set_mismatch",
            "expected_reviewed_near_match_entity_ids":target.visible_candidate_ids,
            "actual_reviewed_near_match_entity_ids":[],
        })));
    }
    let entities = solstone_core_entity::load_all_journal_entities(&request.journal_root)?;
    let principal_id = current_principal_id(&request.journal_root)?;
    let shown = target
        .visible_candidate_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let invalid = request
        .reviewed_near_match_entity_ids
        .iter()
        .filter_map(|id| {
            speaker_attach_rejection_reason(
                id,
                &entities,
                &target.entity_id,
                Some(&shown),
                &principal_id,
            )
            .map(|reason| json!({"entity_id":id,"reason":reason.as_str()}))
        })
        .collect::<Vec<_>>();
    if !invalid.is_empty() {
        return Ok(Err(json!({
            "status":"invalid_request",
            "error":"invalid reviewed_near_match_entity_ids",
            "invalid_reviewed_near_match_entity_ids":invalid,
        })));
    }
    let reviewed_set = request
        .reviewed_near_match_entity_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    if reviewed_set != shown {
        return Ok(Err(json!({
            "status":"invalid_request",
            "error":"reviewed_near_match_entity_ids must match shown near matches",
            "invalid_request_code":"reviewed_near_match_set_mismatch",
            "expected_reviewed_near_match_entity_ids":target.visible_candidate_ids,
            "actual_reviewed_near_match_entity_ids":request.reviewed_near_match_entity_ids,
        })));
    }
    Ok(Ok(request.reviewed_near_match_entity_ids.iter().map(|reviewed| {
        let key = pair_key(&target.entity_id, reviewed);
        let (left, right) = key.split_once('|').expect("pair key contains separator");
        json!({"pair_key":key,"entity_id_a":left,"entity_id_b":right,"planned_target_entity_id":target.entity_id,"reviewed_id":reviewed,"prior_record":null,"intended_record":{"pair_key":key,"entity_id_a":left,"entity_id_b":right,"source_kind":"explicit_create_near_match","operation_id":operation_id,"detection_count":0},"detection_count_used":0,"source_kind":"explicit_create_near_match"})
    }).collect()))
}

fn build_retro_plan(
    root: &Path,
    target: &str,
    direct: &[VoiceprintItem],
    added_at: i64,
    planning_owner_entity_id: &str,
) -> Result<Value, IdentifyClusterError> {
    let Some(centroid) = mean_centroid(direct) else {
        return Ok(empty_retro_plan(planning_owner_entity_id));
    };
    let mut tracker = CandidateTracker::new(root);
    let candidates = tracker.snapshot_candidates_locked()?;
    let Some((candidate, score)) = best_matching_candidate(&candidates, &centroid)
        .filter(|(_, score)| *score >= MERGE_THRESHOLD)
    else {
        return Ok(empty_retro_plan(planning_owner_entity_id));
    };
    let candidate = candidate.clone();
    let planned = plan_retroactive_confirm(root, &candidate, &centroid, target, added_at);
    let mut after = candidate.clone();
    after.status = "confirmed".to_owned();
    after.confirmed_entity = Some(target.to_owned());
    Ok(
        json!({"matched":planned.matched,"match_score":score,"candidate_id":planned.candidate_id,"candidate_before":candidate.to_json(),"candidate_after":after.to_json(),"preexisting_voiceprint_keys":[],"voiceprints_to_add":planned.items.iter().map(|item| json!({"key":{"day":item.metadata["day"],"segment_key":item.metadata["segment_key"],"source":item.metadata["source"],"sentence_id":item.metadata["sentence_id"]},"metadata":item.metadata,"embedding":item.embedding})).collect::<Vec<_>>(),"planning_owner_entity_id":planning_owner_entity_id }),
    )
}
fn empty_retro_plan(planning_owner_entity_id: &str) -> Value {
    json!({"matched":false,"match_score":null,"candidate_id":null,"candidate_before":null,"candidate_after":null,"preexisting_voiceprint_keys":[],"voiceprints_to_add":[],"planning_owner_entity_id":planning_owner_entity_id})
}
fn mean_centroid(items: &[VoiceprintItem]) -> Option<Vec<f32>> {
    let first = items.first()?;
    let mut sum = vec![0.0; first.embedding.len()];
    for item in items {
        for (sum, value) in sum.iter_mut().zip(&item.embedding) {
            *sum += value;
        }
    }
    normalize_embedding(&sum)
}
fn direct_plan_json(plan: &DirectVoiceprintsPlan) -> Value {
    json!({"preexisting_keys":plan.preexisting_keys.iter().map(DirectVoiceprintKey::to_json).collect::<Vec<_>>(),"entries_to_add":plan.entries_to_add.iter().map(|entry| json!({"key":entry.key.to_json(),"metadata":entry.metadata,"source_member":member_json(&entry.source_member)})).collect::<Vec<_>>()})
}
fn required_string(value: &Value, field: &str) -> Result<String, ExecuteError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ExecuteError::Unexpected(format!("prepared plan missing {field}")))
}
fn entries<'a>(value: &'a Value, field: &str) -> Result<&'a Vec<Value>, ExecuteError> {
    value
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| ExecuteError::Unexpected(format!("prepared plan missing {field}")))
}
fn direct_phase_plan(plan: &Value) -> Result<DirectVoiceprintsPlan, ExecuteError> {
    let target = plan["target"]["entity_id"]
        .as_str()
        .ok_or_else(|| ExecuteError::Unexpected("prepared plan missing target.entity_id".into()))?
        .to_owned();
    let direct = &plan["direct_voiceprints"];
    let pre = entries(direct, "preexisting_keys")?
        .iter()
        .filter_map(direct_key_from_json)
        .collect();
    let mut add = Vec::new();
    for entry in entries(direct, "entries_to_add")? {
        let key = direct_key_from_json(&entry["key"])
            .ok_or_else(|| ExecuteError::Unexpected("invalid direct key".into()))?;
        let source = member_from_json(&entry["source_member"])
            .ok_or_else(|| ExecuteError::Unexpected("invalid direct source member".into()))?;
        add.push(DirectVoiceprintEntry {
            key,
            metadata: entry["metadata"].clone(),
            source_member: source,
        });
    }
    Ok(DirectVoiceprintsPlan {
        target_entity_id: target,
        preexisting_keys: pre,
        entries_to_add: add,
    })
}
fn direct_key_from_json(value: &Value) -> Option<DirectVoiceprintKey> {
    Some(DirectVoiceprintKey {
        day: value.get("day")?.as_str()?.to_owned(),
        segment_key: value.get("segment_key")?.as_str()?.to_owned(),
        source: value.get("source")?.as_str()?.to_owned(),
        sentence_id: value.get("sentence_id")?.as_i64()?,
    })
}
fn member_from_json(value: &Value) -> Option<MemberProvenance> {
    Some(MemberProvenance {
        day: value.get("day")?.as_str()?.to_owned(),
        stream: value.get("stream")?.as_str()?.to_owned(),
        segment_key: value.get("segment_key")?.as_str()?.to_owned(),
        source: value.get("source")?.as_str()?.to_owned(),
        sentence_id: value.get("sentence_id")?.as_i64()?,
    })
}
fn entity_phase_plan(plan: &Value) -> Result<EntityPhasePlan, ExecuteError> {
    Ok(EntityPhasePlan {
        target_entity_id: plan["target"]["entity_id"]
            .as_str()
            .ok_or_else(|| ExecuteError::Unexpected("missing target".into()))?
            .to_owned(),
        will_create: plan["target"]["will_create"].as_bool().unwrap_or(false),
        intended_identity: plan["entity_identity"]["intended_identity"].clone(),
        operation_id: required_string(plan, "operation_id")?,
    })
}
fn correction_plans(plan: &Value) -> Result<Vec<SegmentCorrectionPlan>, ExecuteError> {
    entries(plan, "segments")?
        .iter()
        .map(|s| {
            Ok(SegmentCorrectionPlan {
                day: s["day"]
                    .as_str()
                    .ok_or_else(|| ExecuteError::Unexpected("segment day".into()))?
                    .into(),
                stream: s["stream"]
                    .as_str()
                    .ok_or_else(|| ExecuteError::Unexpected("segment stream".into()))?
                    .into(),
                segment_key: s["segment_key"]
                    .as_str()
                    .ok_or_else(|| ExecuteError::Unexpected("segment key".into()))?
                    .into(),
                rows_to_append: entries(&s["corrections"], "rows_to_append")?.clone(),
            })
        })
        .collect()
}
fn label_plans(plan: &Value) -> Result<Vec<SegmentLabelPlan>, ExecuteError> {
    entries(plan, "segments")?
        .iter()
        .map(|s| {
            let labels = entries(s, "labels")?
                .iter()
                .map(|label| {
                    Ok(LabelPlanItem {
                        sentence_id: label["sentence_id"]
                            .as_i64()
                            .ok_or_else(|| ExecuteError::Unexpected("label sentence".into()))?,
                        intended_label: label["intended_label"].clone(),
                        prior_state: label["prior_state"].as_str().unwrap_or_default().into(),
                        prior_label: (!label["prior_label"].is_null())
                            .then(|| label["prior_label"].clone()),
                    })
                })
                .collect::<Result<Vec<_>, ExecuteError>>()?;
            Ok(SegmentLabelPlan {
                day: s["day"]
                    .as_str()
                    .ok_or_else(|| ExecuteError::Unexpected("segment day".into()))?
                    .into(),
                stream: s["stream"]
                    .as_str()
                    .ok_or_else(|| ExecuteError::Unexpected("segment stream".into()))?
                    .into(),
                segment_key: s["segment_key"]
                    .as_str()
                    .ok_or_else(|| ExecuteError::Unexpected("segment key".into()))?
                    .into(),
                labels,
            })
        })
        .collect()
}
fn keep_separate_entries(plan: &Value) -> Result<Vec<KeepSeparatePhaseEntry>, ExecuteError> {
    entries(plan, "keep_separate_assertions")?
        .iter()
        .map(|e| {
            Ok(KeepSeparatePhaseEntry {
                pair_key: e["pair_key"]
                    .as_str()
                    .ok_or_else(|| ExecuteError::Unexpected("pair key".into()))?
                    .into(),
                entity_id_a: e["entity_id_a"]
                    .as_str()
                    .ok_or_else(|| ExecuteError::Unexpected("entity a".into()))?
                    .into(),
                entity_id_b: e["entity_id_b"]
                    .as_str()
                    .ok_or_else(|| ExecuteError::Unexpected("entity b".into()))?
                    .into(),
                source_kind: e["source_kind"].as_str().unwrap_or_default().into(),
                detection_count_used: e["detection_count_used"].as_i64().unwrap_or_default(),
            })
        })
        .collect()
}
fn retro_phase_plan(plan: &Value) -> Result<RetroTrackerPhasePlan, ExecuteError> {
    let retro = &plan["retro_confirm"];
    let target = plan["target"]["entity_id"]
        .as_str()
        .ok_or_else(|| ExecuteError::Unexpected("target".into()))?
        .to_owned();
    let items = entries(retro, "voiceprints_to_add")?
        .iter()
        .map(|v| {
            let metadata = v["metadata"].clone();
            let key = direct_key_from_json(&metadata)
                .ok_or_else(|| ExecuteError::Unexpected("retro metadata".into()))?;
            let embedding = v["embedding"]
                .as_array()
                .ok_or_else(|| ExecuteError::Unexpected("retro embedding".into()))?
                .iter()
                .map(|x| x.as_f64().map(|x| x as f32))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| ExecuteError::Unexpected("retro embedding".into()))?;
            Ok(RetroVoiceprintEntry {
                key,
                metadata: metadata.clone(),
                item: VoiceprintItem {
                    embedding,
                    metadata,
                },
            })
        })
        .collect::<Result<Vec<_>, ExecuteError>>()?;
    Ok(RetroTrackerPhasePlan {
        matched: retro["matched"].as_bool().unwrap_or(false),
        candidate_id: retro["candidate_id"].as_i64(),
        target_entity_id: target,
        planning_owner_entity_id: retro
            .get("planning_owner_entity_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
        candidate_before: (!retro["candidate_before"].is_null())
            .then(|| retro["candidate_before"].clone()),
        candidate_after: (!retro["candidate_after"].is_null())
            .then(|| retro["candidate_after"].clone()),
        voiceprints_to_add: items,
    })
}
fn sentinel_phase_plan(plan: &Value) -> Result<SentinelPhasePlan, ExecuteError> {
    let s = &plan["sentinel"];
    Ok(SentinelPhasePlan {
        cluster_key: s["cluster_key"]
            .as_str()
            .ok_or_else(|| ExecuteError::Unexpected("sentinel".into()))?
            .into(),
        prior_entry: (!s["prior_entry"].is_null()).then(|| s["prior_entry"].clone()),
        intended_entry: s["intended_entry"].clone(),
    })
}

fn map_forward_error(error: ForwardPhaseError) -> ExecuteError {
    match error {
        ForwardPhaseError::RepairRequired {
            phase,
            code,
            categories,
            partial_report,
        } => ExecuteError::Repair {
            phase,
            code,
            categories,
            partial_report,
        },
        other => ExecuteError::Unexpected(other.to_string()),
    }
}
fn forward_success_result(plan: &Value, checks: &BTreeMap<ForwardPhase, Value>) -> Value {
    let get = |phase, field| {
        checks
            .get(&phase)
            .and_then(|c| c.get(field))
            .and_then(Value::as_i64)
            .unwrap_or(0)
    };
    json!({"status":"identified","operation_id":plan["operation_id"],"operation_state":"committed","entity_id":plan["target"]["entity_id"],"entity_name":plan["target"]["entity_name"],"entity_created":checks.get(&ForwardPhase::Entity).and_then(|c|c.get("entity_created")).and_then(Value::as_bool).unwrap_or(false),"voiceprints_saved":get(ForwardPhase::DirectVoiceprints,"saved_count"),"retro_voiceprints_saved":get(ForwardPhase::RetroTracker,"voiceprints_saved_count"),"segments_updated":get(ForwardPhase::Labels,"segment_count"),"sentences_attributed":get(ForwardPhase::Labels,"patched_count")+get(ForwardPhase::Labels,"inserted_count"),"corrections_appended":get(ForwardPhase::Corrections,"appended_count"),"keep_separate_assertions_recorded":get(ForwardPhase::KeepSeparate,"recorded_count")})
}
fn members_from_plan(plan: &Value) -> Vec<MemberProvenance> {
    plan["cluster"]["members"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(member_from_json)
        .collect()
}
fn stored_request_matches_raw(plan: &Value, request: &IdentifyClusterRequest) -> bool {
    plan.get("request") == Some(&raw_request(request))
}
fn fingerprint_conflict_result(operation_id: &str, state: &OperationState) -> Value {
    json!({"status":"conflict","operation_id":operation_id,"operation_state":terminal_name(state.terminal_status),"conflict_code":"request_fingerprint_mismatch"})
}
pub(crate) fn state_status_result(state: &OperationState) -> Value {
    if state.terminal_status == TerminalStatus::Committed {
        return state.result.clone().unwrap_or_else(|| {
            json!({"status":"identified","operation_id":state.operation_id,"operation_state":"committed"})
        });
    }
    let completed_phases = state
        .completed_phases
        .iter()
        .map(|phase| phase.as_str())
        .collect::<Vec<_>>();
    let pending_phases = state.pending_phases.clone();
    match state.terminal_status {
        TerminalStatus::RepairRequired => {
            let (phase, code, categories) = state
                .repair_required
                .as_ref()
                .and_then(|event| {
                    if let EventPayload::RepairRequired {
                        phase,
                        repair_code,
                        repair_categories,
                        ..
                    } = &event.payload
                    {
                        Some((
                            phase.as_str(),
                            repair_code.clone(),
                            repair_categories.clone(),
                        ))
                    } else {
                        None
                    }
                })
                .unwrap_or(("", String::new(), json!({})));
            json!({"status":"repair_required","operation_id":state.operation_id,"operation_state":"repair_required","phase":phase,"repair_code":code,"repair_categories":categories,"completed_phases":completed_phases,"pending_phases":pending_phases})
        }
        TerminalStatus::Undone => {
            json!({"status":"operation_already_undone","operation_id":state.operation_id,"operation_state":"undone"})
        }
        TerminalStatus::UndoRepairRequired => {
            json!({"status":"undo_repair_required","operation_id":state.operation_id,"operation_state":"undo_repair_required","undo_report":state.undo_report})
        }
        TerminalStatus::Undoing => {
            json!({"status":"undoing","operation_id":state.operation_id,"operation_state":"undoing","completed_phases":completed_phases,"pending_phases":pending_phases,"undo_report":state.undo_report})
        }
        TerminalStatus::InProgress => {
            json!({"status":"in_progress","operation_id":state.operation_id,"operation_state":"in_progress","completed_phases":completed_phases,"pending_phases":pending_phases})
        }
        TerminalStatus::Committed => unreachable!("handled above"),
    }
}

fn current_phase_names(
    ledger_path: &Path,
    operation_id: &str,
) -> Result<Vec<String>, IdentifyClusterError> {
    Ok(
        fold_operation(&load_operations(ledger_path)?, operation_id)?
            .map(|state| {
                state
                    .completed_phases
                    .iter()
                    .map(|phase| phase.as_str().to_owned())
                    .collect()
            })
            .unwrap_or_default(),
    )
}

fn pending_phase_names(completed: &[String]) -> Vec<String> {
    FORWARD_PHASE_ORDER
        .iter()
        .map(|phase| phase.as_str().to_owned())
        .filter(|phase| !completed.contains(phase))
        .collect()
}

fn recoverable_result(
    ledger_path: &Path,
    operation_id: &str,
    detail: String,
) -> Result<Value, IdentifyClusterError> {
    let state = fold_operation(&load_operations(ledger_path)?, operation_id)?;
    Ok(json!({
        "status":"recoverable",
        "operation_id":operation_id,
        "operation_state":state.as_ref().map_or("not_prepared", |state| terminal_name(state.terminal_status)),
        "request_id":state.as_ref().map(|state| state.request_id.clone()),
        "completed_phases":state.as_ref().map(|state| state.completed_phases.iter().map(|phase| phase.as_str()).collect::<Vec<_>>()).unwrap_or_default(),
        "pending_phases":state.as_ref().map(|state| state.pending_phases.clone()).unwrap_or_default(),
        "detail":detail,
    }))
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
    use std::io::{Cursor, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use solstone_core_entity::read_visible_history;
    use solstone_core_npy::write_npy;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    use super::*;
    use crate::candidate_tracker::ClusterInput;
    use crate::owner_centroid::{OwnerCentroidWriteInput, write_owner_centroid};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-identify-cluster-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            let owner = path.join("entities/owner");
            fs::create_dir_all(&owner).unwrap();
            fs::write(
                owner.join("entity.json"),
                json!({"id":"owner","type":"Person","is_principal":true}).to_string(),
            )
            .unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn encoder() -> EncoderIdentity {
        EncoderIdentity {
            id: "test".into(),
            sha256: "a".repeat(64),
            width: 256,
        }
    }
    fn vector() -> Vec<f32> {
        let mut value = vec![0.0; 256];
        value[0] = 1.0;
        value
    }
    fn floats(values: &[f32]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }
    fn ints(values: &[i32]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }
    fn member() -> MemberProvenance {
        MemberProvenance {
            day: "20260808".into(),
            stream: "mic".into(),
            segment_key: "120000_300".into(),
            source: "audio".into(),
            sentence_id: 7,
        }
    }
    fn write_embeddings(root: &Path) {
        let member = member();
        let segment =
            segment_path(root, &member.day, &member.segment_key, &member.stream, true).unwrap();
        fs::create_dir_all(segment.join("talents")).unwrap();
        let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        archive.start_file("embeddings.npy", options).unwrap();
        archive
            .write_all(&write_npy("<f4", "(1, 256)", &floats(&vector())))
            .unwrap();
        archive.start_file("statement_ids.npy", options).unwrap();
        archive
            .write_all(&write_npy("<i4", "(1,)", &ints(&[7])))
            .unwrap();
        fs::write(
            segment.join("audio.npz"),
            archive.finish().unwrap().into_inner(),
        )
        .unwrap();
    }
    fn write_cache(root: &Path) {
        fs::create_dir_all(root.join("awareness")).unwrap();
        fs::write(
            root.join("awareness/discovery_clusters.json"),
            json!({"clusters":{"1":[member_json(&member())]}}).to_string(),
        )
        .unwrap();
    }
    fn entity(root: &Path, id: &str, name: &str) {
        let path = root.join("entities").join(id);
        fs::create_dir_all(&path).unwrap();
        fs::write(
            path.join("entity.json"),
            json!({"id":id,"name":name,"type":"Person"}).to_string(),
        )
        .unwrap();
    }
    fn write_owner_centroid_for_test(root: &Path, entity_id: &str, centroid: Vec<f32>) {
        write_owner_centroid(
            root,
            entity_id,
            &OwnerCentroidWriteInput {
                centroid,
                cluster_size: 1,
                timestamp: "2026-08-08T00:00:00Z".into(),
                evidence_tier: "test".into(),
            },
        )
        .unwrap();
    }
    fn invalidate_owner(root: &Path) {
        fs::write(
            root.join("entities/owner/entity.json"),
            json!({"id":"owner","type":"Project","is_principal":true}).to_string(),
        )
        .unwrap();
    }
    fn set_principal(root: &Path, entity_id: &str, is_principal: bool) {
        fs::write(
            root.join("entities").join(entity_id).join("entity.json"),
            json!({"id":entity_id,"type":"Person","is_principal":is_principal}).to_string(),
        )
        .unwrap();
    }
    fn append_phase_checkpoint(
        path: &Path,
        request: &IdentifyClusterRequest,
        operation_id: &str,
        plan: &Value,
        phase: ForwardPhase,
    ) {
        let mut checkpoint = run_phase(phase, &request.journal_root, plan, &encoder()).unwrap();
        let checkpoint = checkpoint.as_object_mut().expect("phase checkpoint object");
        checkpoint.insert("phase_status".into(), Value::String("complete".into()));
        checkpoint.insert(
            "completed_at".into(),
            Value::String(Utc::now().to_rfc3339()),
        );
        append_event(
            path,
            &event(
                request,
                operation_id,
                format!("{operation_id}:checkpoint:{}", phase.as_str()),
                EventPayload::Checkpoint {
                    phase,
                    checkpoint: Value::Object(checkpoint.clone()),
                },
            ),
        )
        .unwrap();
    }
    fn request(root: &Path, request_id: &str, entity_id: &str) -> IdentifyClusterRequest {
        IdentifyClusterRequest {
            journal_root: root.to_path_buf(),
            cluster_id: 1,
            name: None,
            entity_id: Some(entity_id.into()),
            resolve_only: false,
            create_new: false,
            entity_type: "Person".into(),
            request_id: request_id.into(),
            reviewed_near_match_entity_ids: vec![],
            caller: String::new(),
            actor: None,
        }
    }

    #[test]
    fn identify_retro_plan_ignores_rejected_candidates() {
        let temporary = Temp::new();
        fs::create_dir_all(temporary.path().join("awareness")).unwrap();
        let rejected = crate::candidate_tracker::CandidateProfile {
            cand_id: 1,
            centroid: vector(),
            n_segments: 1,
            n_intervals: 1,
            total_duration_s: 1.0,
            source_segments: vec![],
            confirmed_entity: None,
            status: "rejected".to_owned(),
            merge_events: vec![],
        };
        fs::write(
            temporary.path().join("awareness/speaker_candidates.json"),
            json!({"next_id":2,"candidates":[rejected.to_json()]}).to_string(),
        )
        .unwrap();
        let direct = vec![VoiceprintItem {
            embedding: vector(),
            metadata: json!({}),
        }];
        assert_eq!(
            build_retro_plan(temporary.path(), "target", &direct, 1, "owner").unwrap()["matched"],
            false
        );

        let mut pending = rejected;
        pending.status = "pending".to_owned();
        fs::write(
            temporary.path().join("awareness/speaker_candidates.json"),
            json!({"next_id":2,"candidates":[pending.to_json()]}).to_string(),
        )
        .unwrap();
        let plan = build_retro_plan(temporary.path(), "target", &direct, 1, "owner").unwrap();
        assert_eq!(plan["matched"], true);
        assert_eq!(plan["candidate_id"], 1);
    }

    #[test]
    fn invalid_owner_identity_refuses_planning_without_an_operation_or_mutation() {
        let temporary = Temp::new();
        invalidate_owner(temporary.path());
        write_cache(temporary.path());
        write_embeddings(temporary.path());
        let request = IdentifyClusterRequest {
            journal_root: temporary.path().to_path_buf(),
            cluster_id: 1,
            name: Some("Target".into()),
            entity_id: None,
            resolve_only: false,
            create_new: true,
            entity_type: "Person".into(),
            request_id: "request-invalid-planning-owner".into(),
            reviewed_near_match_entity_ids: vec![],
            caller: String::new(),
            actor: None,
        };

        let result = identify_cluster(&request, &encoder()).unwrap();
        assert_eq!(result["status"], "recoverable");
        assert_eq!(result["error"], OWNER_IDENTITY_INVALID_REASON);
        assert!(!identify_ledger_path(temporary.path()).exists());
        assert!(!temporary.path().join("entities/target").exists());
    }

    #[test]
    fn direct_identity_repair_has_an_object_report_and_no_direct_checkpoint() {
        let temporary = Temp::new();
        entity(temporary.path(), "target", "Target");
        write_cache(temporary.path());
        write_embeddings(temporary.path());
        let request = request(temporary.path(), "request-direct-owner-repair", "target");
        let operation_id = operation_id_for_request(&request.request_id).unwrap();
        let planned = plan_identify(&request, &operation_id).unwrap().unwrap();
        let path = identify_ledger_path(temporary.path());
        append_prepared(&path, &request, &operation_id, &planned).unwrap();
        invalidate_owner(temporary.path());

        let result = identify_cluster(&request, &encoder()).unwrap();
        assert_eq!(result["status"], "repair_required");
        assert_eq!(result["phase"], ForwardPhase::DirectVoiceprints.as_str());
        assert_eq!(result["repair_code"], OWNER_IDENTITY_INVALID_REASON);

        let state = fold_operation(&load_operations(&path).unwrap(), &operation_id)
            .unwrap()
            .unwrap();
        assert_eq!(state.terminal_status, TerminalStatus::RepairRequired);
        assert_eq!(state.completed_phases, FORWARD_PHASE_ORDER[..2]);
        assert!(
            !state
                .phase_checkpoints
                .contains_key(&ForwardPhase::DirectVoiceprints)
        );
        let EventPayload::RepairRequired { partial_report, .. } = &state
            .repair_required
            .as_ref()
            .expect("outstanding repair")
            .payload
        else {
            panic!("repair projection contains repair payload");
        };
        assert_eq!(
            partial_report,
            &json!({"pending_phases": [
                "direct_voiceprints", "corrections", "labels", "retro_tracker", "sentinel"
            ]})
        );
        assert!(
            !temporary
                .path()
                .join("entities/target/voiceprints.npz")
                .exists()
        );
        assert!(
            !temporary
                .path()
                .join("awareness/discovery_clusters.resolved.json")
                .exists()
        );
        let repair_ledger = fs::read(&path).unwrap();
        let retry = identify_cluster(&request, &encoder()).unwrap();
        assert_eq!(retry["status"], "repair_required");
        assert_eq!(fs::read(&path).unwrap(), repair_ledger);
    }

    #[test]
    fn retro_identity_repair_keeps_prior_checkpoints_and_does_not_write_tracker_or_sentinel() {
        let temporary = Temp::new();
        entity(temporary.path(), "target", "Target");
        write_cache(temporary.path());
        write_embeddings(temporary.path());
        let mut owner = vec![0.0; 256];
        owner[1] = 1.0;
        write_owner_centroid_for_test(temporary.path(), "owner", owner);
        let mut tracker = CandidateTracker::new(temporary.path());
        tracker
            .process_segment(&[ClusterInput {
                source_segment: json!({"day":"20260808","stream":"mic","segment_key":"120000_300","source":"audio","cluster_label":1}),
                embeddings: vec![vector()],
                durations_s: vec![1.0],
            }])
            .unwrap();

        let request = request(temporary.path(), "request-retro-owner-repair", "target");
        let operation_id = operation_id_for_request(&request.request_id).unwrap();
        let planned = plan_identify(&request, &operation_id).unwrap().unwrap();
        assert_eq!(planned.prepared_plan["retro_confirm"]["matched"], true);
        let path = identify_ledger_path(temporary.path());
        append_prepared(&path, &request, &operation_id, &planned).unwrap();
        for phase in FORWARD_PHASE_ORDER[..5].iter().copied() {
            append_phase_checkpoint(
                &path,
                &request,
                &operation_id,
                &planned.prepared_plan,
                phase,
            );
        }
        let candidates_before =
            fs::read(temporary.path().join("awareness/speaker_candidates.json")).unwrap();
        invalidate_owner(temporary.path());

        let result = identify_cluster(&request, &encoder()).unwrap();
        assert_eq!(result["status"], "repair_required");
        assert_eq!(result["phase"], ForwardPhase::RetroTracker.as_str());
        assert_eq!(result["repair_code"], OWNER_IDENTITY_INVALID_REASON);
        let state = fold_operation(&load_operations(&path).unwrap(), &operation_id)
            .unwrap()
            .unwrap();
        assert_eq!(state.completed_phases, FORWARD_PHASE_ORDER[..5]);
        assert!(
            !state
                .phase_checkpoints
                .contains_key(&ForwardPhase::RetroTracker)
        );
        assert_eq!(
            fs::read(temporary.path().join("awareness/speaker_candidates.json")).unwrap(),
            candidates_before
        );
        assert!(
            !temporary
                .path()
                .join("awareness/discovery_clusters.resolved.json")
                .exists()
        );
    }

    #[test]
    fn unbound_legacy_retro_plan_stops_loudly_without_a_resume_event() {
        let temporary = Temp::new();
        entity(temporary.path(), "target", "Target");
        write_cache(temporary.path());
        write_embeddings(temporary.path());
        let request = request(temporary.path(), "request-unbound-retro-plan", "target");
        let operation_id = operation_id_for_request(&request.request_id).unwrap();
        let mut planned = plan_identify(&request, &operation_id).unwrap().unwrap();
        planned.prepared_plan["retro_confirm"]
            .as_object_mut()
            .unwrap()
            .remove("planning_owner_entity_id");
        let path = identify_ledger_path(temporary.path());
        append_prepared(&path, &request, &operation_id, &planned).unwrap();
        for phase in FORWARD_PHASE_ORDER[..5].iter().copied() {
            append_phase_checkpoint(
                &path,
                &request,
                &operation_id,
                &planned.prepared_plan,
                phase,
            );
        }

        let result = identify_cluster(&request, &encoder()).unwrap();
        assert_eq!(result["status"], "repair_required");
        assert_eq!(result["phase"], ForwardPhase::RetroTracker.as_str());
        assert_eq!(result["repair_code"], "speaker_identify_plan_owner_unbound");
        let before_retry = fs::read(&path).unwrap();
        let retry = identify_cluster(&request, &encoder()).unwrap();
        assert_eq!(retry["status"], "repair_required");
        assert_eq!(fs::read(&path).unwrap(), before_retry);
        assert!(
            load_operations(&path)
                .unwrap()
                .iter()
                .all(|row| !matches!(row.event.payload, EventPayload::RepairResumed { .. }))
        );
    }

    #[test]
    fn resumed_in_progress_operation_never_appends_a_second_resume_event() {
        let temporary = Temp::new();
        entity(temporary.path(), "target", "Target");
        write_cache(temporary.path());
        write_embeddings(temporary.path());
        let request = request(
            temporary.path(),
            "request-resumed-before-checkpoint",
            "target",
        );
        let operation_id = operation_id_for_request(&request.request_id).unwrap();
        let planned = plan_identify(&request, &operation_id).unwrap().unwrap();
        let path = identify_ledger_path(temporary.path());
        append_prepared(&path, &request, &operation_id, &planned).unwrap();
        let repair_event_id = format!("{operation_id}:repair_required:direct_voiceprints");
        append_event(
            &path,
            &event(
                &request,
                &operation_id,
                repair_event_id.clone(),
                EventPayload::RepairRequired {
                    phase: ForwardPhase::DirectVoiceprints,
                    repair_code: OWNER_IDENTITY_INVALID_REASON.into(),
                    repair_categories: json!({"owner_identity":1}),
                    partial_report: json!({"pending_phases":["direct_voiceprints"]}),
                },
            ),
        )
        .unwrap();
        append_event(
            &path,
            &event(
                &request,
                &operation_id,
                format!("{repair_event_id}:resumed"),
                EventPayload::RepairResumed {
                    repair_event_id: repair_event_id.clone(),
                    phase: ForwardPhase::DirectVoiceprints,
                },
            ),
        )
        .unwrap();

        let result = identify_cluster(&request, &encoder()).unwrap();
        assert_eq!(result["status"], "identified");
        let rows = load_operations(&path).unwrap();
        assert_eq!(
            rows.iter()
                .filter(|row| row.event.event_id == format!("{repair_event_id}:resumed"))
                .count(),
            1
        );
        assert_eq!(
            rows.iter()
                .filter(|row| {
                    matches!(
                        &row.event.payload,
                        EventPayload::Checkpoint {
                            phase: ForwardPhase::DirectVoiceprints,
                            ..
                        }
                    )
                })
                .count(),
            1
        );
    }

    #[test]
    fn identify_holds_owner_identity_stable_for_its_full_operation_and_next_plan_observes_b() {
        let temporary = Temp::new();
        entity(temporary.path(), "owner_b", "Owner B");
        write_cache(temporary.path());
        write_embeddings(temporary.path());
        entity(temporary.path(), "target", "Target");

        set_principal(temporary.path(), "owner", false);
        set_principal(temporary.path(), "owner_b", true);
        write_owner_centroid_for_test(temporary.path(), "owner_b", vector());
        set_principal(temporary.path(), "owner_b", false);
        set_principal(temporary.path(), "owner", true);
        let mut owner_a_centroid = vec![0.0; 256];
        owner_a_centroid[1] = 1.0;
        write_owner_centroid_for_test(temporary.path(), "owner", owner_a_centroid);

        let active_request = request(temporary.path(), "request-owner-a", "target");
        let operation_id = operation_id_for_request(&active_request.request_id).unwrap();
        let root = temporary.path().to_path_buf();
        let outer = hold_entity_trust_lock(temporary.path()).unwrap();
        let planned = plan_identify(&active_request, &operation_id)
            .unwrap()
            .unwrap();
        append_prepared(
            &identify_ledger_path(temporary.path()),
            &active_request,
            &operation_id,
            &planned,
        )
        .unwrap();
        let (started, receiver) = mpsc::channel();
        let worker = thread::spawn(move || {
            let _trust = hold_entity_trust_lock(&root).unwrap();
            started.send(()).unwrap();
            set_principal(&root, "owner", false);
            set_principal(&root, "owner_b", true);
        });
        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));

        let result = identify_cluster(&active_request, &encoder()).unwrap();
        assert_eq!(result["status"], "identified");
        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        let state = fold_operation(
            &load_operations(&identify_ledger_path(temporary.path())).unwrap(),
            &operation_id,
        )
        .unwrap()
        .unwrap();
        assert_eq!(state.terminal_status, TerminalStatus::Committed);
        assert_eq!(
            state.prepared_plan["retro_confirm"]["planning_owner_entity_id"],
            "owner"
        );
        assert_eq!(
            state
                .phase_checkpoints
                .get(&ForwardPhase::DirectVoiceprints)
                .unwrap()["saved_count"],
            1
        );

        drop(outer);
        receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.join().unwrap();

        let next = request(temporary.path(), "request-owner-b", "target");
        let next_operation_id = operation_id_for_request(&next.request_id).unwrap();
        let next_plan = plan_identify(&next, &next_operation_id).unwrap().unwrap();
        assert_eq!(
            next_plan.prepared_plan["retro_confirm"]["planning_owner_entity_id"],
            "owner_b"
        );
    }

    #[test]
    fn repair_event_ids_gain_retry_suffixes_without_changing_existing_rows() {
        let temporary = Temp::new();
        let path = identify_ledger_path(temporary.path());
        assert_eq!(
            next_repair_event_id(&path, "idop_test", ForwardPhase::DirectVoiceprints).unwrap(),
            "idop_test:repair_required:direct_voiceprints"
        );
        let request = request(temporary.path(), "request-repair-id", "target");
        append_event(
            &path,
            &event(
                &request,
                "idop_test",
                "idop_test:repair_required:direct_voiceprints".into(),
                EventPayload::RepairRequired {
                    phase: ForwardPhase::DirectVoiceprints,
                    repair_code: OWNER_IDENTITY_INVALID_REASON.into(),
                    repair_categories: json!({"owner_identity":1}),
                    partial_report: json!({"pending_phases":["direct_voiceprints"]}),
                },
            ),
        )
        .unwrap();
        assert_eq!(
            next_repair_event_id(&path, "idop_test", ForwardPhase::DirectVoiceprints).unwrap(),
            "idop_test:repair_required:direct_voiceprints:retry:1"
        );
    }

    #[test]
    fn identify_cluster_fresh_operation_commits_all_forward_phases() {
        let temporary = Temp::new();
        write_cache(temporary.path());
        write_embeddings(temporary.path());
        let request = IdentifyClusterRequest {
            journal_root: temporary.path().to_path_buf(),
            cluster_id: 1,
            name: Some("Target".into()),
            entity_id: None,
            resolve_only: false,
            create_new: true,
            entity_type: "Person".into(),
            request_id: "request-fresh".into(),
            reviewed_near_match_entity_ids: vec![],
            caller: String::new(),
            actor: None,
        };
        let result = identify_cluster(&request, &encoder()).unwrap();
        assert_eq!(result["status"], "identified", "{result}");
        assert_eq!(result["entity_id"], "target");
        assert_eq!(result["entity_created"], true);
        assert_eq!(result["voiceprints_saved"], 1);
        assert_eq!(result["sentences_attributed"], 1);
        assert_eq!(result["corrections_appended"], 1);
        assert!(
            temporary
                .path()
                .join("entities/target/voiceprints.npz")
                .exists()
        );
        assert_eq!(
            load_labels(
                &segment_path(temporary.path(), "20260808", "120000_300", "mic", false).unwrap()
            )[&7]["method"],
            "user_identified"
        );
        assert_eq!(
            load_resolved_clusters(temporary.path())["1"]["entity_id"],
            "target"
        );
        let rows = load_operations(&identify_ledger_path(temporary.path())).unwrap();
        let state = fold_operation(&rows, &operation_id_for_request("request-fresh").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(state.terminal_status, TerminalStatus::Committed);
        assert_eq!(state.completed_phases, FORWARD_PHASE_ORDER);
    }

    #[test]
    fn identify_cluster_resumes_only_uncheckpointed_forward_phases() {
        let temporary = Temp::new();
        entity(temporary.path(), "target", "Target");
        write_cache(temporary.path());
        write_embeddings(temporary.path());
        let request = request(temporary.path(), "request-resume", "target");
        let operation_id = operation_id_for_request(&request.request_id).unwrap();
        let planned = plan_identify(&request, &operation_id).unwrap().unwrap();
        let path = identify_ledger_path(temporary.path());
        append_prepared(&path, &request, &operation_id, &planned).unwrap();
        let mut entity_checkpoint = phase_entity(
            temporary.path(),
            &entity_phase_plan(&planned.prepared_plan).unwrap(),
        )
        .unwrap();
        entity_checkpoint
            .fields
            .as_object_mut()
            .unwrap()
            .insert("phase_status".into(), Value::String("complete".into()));
        entity_checkpoint.fields.as_object_mut().unwrap().insert(
            "completed_at".into(),
            Value::String(Utc::now().to_rfc3339()),
        );
        append_event(
            &path,
            &event(
                &request,
                &operation_id,
                format!("{operation_id}:checkpoint:entity"),
                EventPayload::Checkpoint {
                    phase: ForwardPhase::Entity,
                    checkpoint: entity_checkpoint.fields,
                },
            ),
        )
        .unwrap();
        let result = identify_cluster(&request, &encoder()).unwrap();
        assert_eq!(result["status"], "identified");
        let rows = load_operations(&path).unwrap();
        assert_eq!(
            rows.iter()
                .filter(|row| row.event.event_id == format!("{operation_id}:checkpoint:entity"))
                .count(),
            1
        );
        let state = fold_operation(&rows, &operation_id).unwrap().unwrap();
        assert_eq!(state.completed_phases, FORWARD_PHASE_ORDER);
    }

    #[test]
    fn identify_cluster_rejects_same_request_id_with_different_fingerprint() {
        let temporary = Temp::new();
        write_cache(temporary.path());
        write_embeddings(temporary.path());
        let first = IdentifyClusterRequest {
            journal_root: temporary.path().to_path_buf(),
            cluster_id: 1,
            name: Some("Alice".into()),
            entity_id: None,
            resolve_only: false,
            create_new: true,
            entity_type: "Person".into(),
            request_id: "request-conflict".into(),
            reviewed_near_match_entity_ids: vec![],
            caller: String::new(),
            actor: None,
        };
        let operation_id = operation_id_for_request(&first.request_id).unwrap();
        let planned = plan_identify(&first, &operation_id).unwrap().unwrap();
        append_prepared(
            &identify_ledger_path(temporary.path()),
            &first,
            &operation_id,
            &planned,
        )
        .unwrap();
        let second = IdentifyClusterRequest {
            name: Some("Bob".into()),
            ..first
        };
        let result = identify_cluster(&second, &encoder()).unwrap();
        assert_eq!(result["status"], "conflict");
        assert_eq!(result["conflict_code"], "request_fingerprint_mismatch");
    }

    #[test]
    fn identify_cluster_rejects_different_target_for_committed_member_set() {
        let temporary = Temp::new();
        entity(temporary.path(), "one", "One");
        entity(temporary.path(), "two", "Two");
        write_cache(temporary.path());
        write_embeddings(temporary.path());
        assert_eq!(
            identify_cluster(&request(temporary.path(), "request-one", "one"), &encoder()).unwrap()
                ["status"],
            "identified"
        );
        let result =
            identify_cluster(&request(temporary.path(), "request-two", "two"), &encoder()).unwrap();
        assert_eq!(result["status"], "conflict");
        assert_eq!(result["conflict_code"], "member_set_target_conflict");
    }

    #[test]
    fn identify_cluster_unexpected_failure_remains_resumable_without_terminal_row() {
        let temporary = Temp::new();
        entity(temporary.path(), "target", "Target");
        write_cache(temporary.path());
        write_embeddings(temporary.path());
        let request = request(temporary.path(), "request-recoverable", "target");
        let operation_id = operation_id_for_request(&request.request_id).unwrap();
        let planned = plan_identify(&request, &operation_id).unwrap().unwrap();
        let path = identify_ledger_path(temporary.path());
        append_prepared(&path, &request, &operation_id, &planned).unwrap();
        let label_path = segment_path(temporary.path(), "20260808", "120000_300", "mic", false)
            .unwrap()
            .join("talents/speaker_labels.json");
        fs::create_dir(&label_path).unwrap();
        let result = identify_cluster(&request, &encoder()).unwrap();
        assert_eq!(result["status"], "recoverable");
        let state = fold_operation(&load_operations(&path).unwrap(), &operation_id)
            .unwrap()
            .unwrap();
        assert_eq!(state.terminal_status, TerminalStatus::InProgress);
        assert!(state.result.is_none());
        assert!(state.repair_required.is_none());
    }

    #[test]
    fn identify_cluster_create_new_resumes_after_recoverable_labels_failure() {
        let temporary = Temp::new();
        write_cache(temporary.path());
        write_embeddings(temporary.path());
        let request = IdentifyClusterRequest {
            journal_root: temporary.path().to_path_buf(),
            cluster_id: 1,
            name: Some("Target".into()),
            entity_id: None,
            resolve_only: false,
            create_new: true,
            entity_type: "Person".into(),
            request_id: "request-create-crash".into(),
            reviewed_near_match_entity_ids: vec![],
            caller: String::new(),
            actor: None,
        };
        let label_path = segment_path(temporary.path(), "20260808", "120000_300", "mic", false)
            .unwrap()
            .join("talents/speaker_labels.json");
        fs::create_dir(&label_path).unwrap();

        let first = identify_cluster(&request, &encoder()).unwrap();
        assert_eq!(first["status"], "recoverable", "{first}");
        assert!(
            temporary
                .path()
                .join("entities/target/entity.json")
                .is_file()
        );
        let voiceprints_path = temporary.path().join("entities/target/voiceprints.npz");
        let voiceprints_before_resume = fs::read(&voiceprints_path).unwrap();
        let path = identify_ledger_path(temporary.path());
        let operation_id = operation_id_for_request(&request.request_id).unwrap();
        let state = fold_operation(&load_operations(&path).unwrap(), &operation_id)
            .unwrap()
            .unwrap();
        assert_eq!(state.terminal_status, TerminalStatus::InProgress);
        assert!(state.completed_phases.contains(&ForwardPhase::Entity));
        assert!(!state.completed_phases.contains(&ForwardPhase::Labels));

        fs::remove_dir(&label_path).unwrap();
        let second = identify_cluster(&request, &encoder()).unwrap();
        assert_eq!(second["status"], "identified", "{second}");
        assert_eq!(second["entity_id"], "target");

        let rows = load_operations(&path).unwrap();
        assert_eq!(
            rows.iter()
                .filter(|row| row.event.event_id == format!("{operation_id}:checkpoint:entity"))
                .count(),
            1,
            "entity phase must not re-checkpoint on resume"
        );
        assert_eq!(
            read_visible_history(temporary.path(), "target")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            fs::read(voiceprints_path).unwrap(),
            voiceprints_before_resume
        );
    }

    #[test]
    fn identify_cluster_replays_create_after_entity_write_before_checkpoint() {
        let temporary = Temp::new();
        write_cache(temporary.path());
        write_embeddings(temporary.path());
        let request = IdentifyClusterRequest {
            journal_root: temporary.path().to_path_buf(),
            cluster_id: 1,
            name: Some("Target".into()),
            entity_id: None,
            resolve_only: false,
            create_new: true,
            entity_type: "Person".into(),
            request_id: "request-create-before-entity-checkpoint".into(),
            reviewed_near_match_entity_ids: vec![],
            caller: String::new(),
            actor: None,
        };
        let operation_id = operation_id_for_request(&request.request_id).unwrap();
        let planned = plan_identify(&request, &operation_id).unwrap().unwrap();
        let path = identify_ledger_path(temporary.path());
        append_prepared(&path, &request, &operation_id, &planned).unwrap();
        phase_entity(
            temporary.path(),
            &entity_phase_plan(&planned.prepared_plan).unwrap(),
        )
        .unwrap();

        let before_resume = load_operations(&path).unwrap();
        assert!(
            before_resume
                .iter()
                .all(|row| row.event.event_id != format!("{operation_id}:checkpoint:entity"))
        );
        assert_eq!(
            read_visible_history(temporary.path(), "target")
                .unwrap()
                .len(),
            1
        );

        let result = identify_cluster(&request, &encoder()).unwrap();
        assert_eq!(result["status"], "identified", "{result}");
        assert_eq!(result["entity_id"], "target");
        let voiceprints_path = temporary.path().join("entities/target/voiceprints.npz");
        let voiceprints_after_resume = fs::read(&voiceprints_path).unwrap();
        let rows = load_operations(&path).unwrap();
        assert_eq!(
            rows.iter()
                .filter(|row| row.event.event_id == format!("{operation_id}:checkpoint:entity"))
                .count(),
            1
        );
        assert_eq!(
            read_visible_history(temporary.path(), "target")
                .unwrap()
                .len(),
            1
        );

        let replay = identify_cluster(&request, &encoder()).unwrap();
        assert_eq!(replay["status"], "identified", "{replay}");
        assert_eq!(
            fs::read(voiceprints_path).unwrap(),
            voiceprints_after_resume
        );
    }

    #[test]
    fn distinct_fresh_operations_each_refuse_an_occupied_create_destination() {
        let temporary = Temp::new();
        entity(temporary.path(), "new_person", "Someone Else");
        let identity_path = temporary.path().join("entities/new_person/entity.json");
        let identity_before = fs::read(&identity_path).unwrap();
        let voiceprints_path = temporary.path().join("entities/new_person/voiceprints.npz");
        fs::write(&voiceprints_path, b"incumbent-voiceprint-sentinel").unwrap();
        let voiceprints_before = fs::read(&voiceprints_path).unwrap();
        write_cache(temporary.path());
        write_embeddings(temporary.path());
        let first = IdentifyClusterRequest {
            journal_root: temporary.path().to_path_buf(),
            cluster_id: 1,
            name: Some("New Person".into()),
            entity_id: None,
            resolve_only: false,
            create_new: true,
            entity_type: "Person".into(),
            request_id: "request-occupied-one".into(),
            reviewed_near_match_entity_ids: vec![],
            caller: String::new(),
            actor: None,
        };
        let second = IdentifyClusterRequest {
            request_id: "request-occupied-two".into(),
            ..first.clone()
        };

        for request in [&first, &second] {
            let result = identify_cluster(request, &encoder()).unwrap();
            assert_eq!(result["status"], "destination_occupied", "{result}");
            assert_eq!(result["entity_id"], "new_person");
            assert!(!identify_ledger_path(temporary.path()).exists());
            assert_eq!(fs::read(&identity_path).unwrap(), identity_before);
            assert_eq!(fs::read(&voiceprints_path).unwrap(), voiceprints_before);
        }
    }

    #[test]
    fn fresh_create_refuses_empty_and_null_destinations_before_an_operation_row() {
        for contents in [b"".as_slice(), b"null".as_slice()] {
            let temporary = Temp::new();
            let destination = temporary.path().join("entities/new_person/entity.json");
            fs::create_dir_all(destination.parent().unwrap()).unwrap();
            fs::write(&destination, contents).unwrap();
            write_cache(temporary.path());
            write_embeddings(temporary.path());
            let request = IdentifyClusterRequest {
                journal_root: temporary.path().to_path_buf(),
                cluster_id: 1,
                name: Some("New Person".into()),
                entity_id: None,
                resolve_only: false,
                create_new: true,
                entity_type: "Person".into(),
                request_id: format!("request-occupied-{}", contents.len()),
                reviewed_near_match_entity_ids: vec![],
                caller: String::new(),
                actor: None,
            };

            let result = identify_cluster(&request, &encoder()).unwrap();
            assert_eq!(result["status"], "destination_occupied", "{result}");
            assert!(!identify_ledger_path(temporary.path()).exists());
            assert_eq!(fs::read(&destination).unwrap(), contents);
        }
    }
}
