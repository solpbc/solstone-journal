// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Coordinator for executing and resuming non-Person speaker repair operations.

use std::fs;
use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use solstone_core_entity::{
    EncoderIdentity, VoiceprintRemoval, entity_memory_path, hold_entity_trust_lock,
    remove_voiceprints_by_key, try_load_entity_voiceprints_in_dir,
};
use solstone_core_journal_io::{LockOptions, hold_lock};
use solstone_core_speaker_id::corrections::read_corrections;
use solstone_core_speaker_id::json::write_python_compatible_json;
use solstone_core_speaker_id::labels::{
    build_repaired_label_payload, compute_bytes_sha256, compute_file_sha256, corrections_path,
    labels_path, replace_labels_if_current_hash_matches_locked,
};

use crate::layer1::Label;
use crate::repair_inventory::survey_repair_inventory;
use crate::repair_operations::{
    PreparedSegmentSnapshot, REPAIR_OPERATION_SCHEMA_VERSION, RepairEvent, SegmentTupleKey,
    acquire_repair_lock, append_repair_event, fold_repair_operation,
};
use crate::resolve::{ResolveMetadata, ResolveOutcome, resolve};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartRepairRequest {
    pub operation_id: Option<String>,
    pub commit: bool,
    pub now_ms: i64,
}

fn metadata_map(metadata: Option<&ResolveMetadata>) -> Map<String, Value> {
    let mut map = Map::new();
    if let Some(meta) = metadata {
        map.insert(
            "owner_centroid_last_refreshed_at".to_owned(),
            meta.owner_centroid_last_refreshed_at
                .as_ref()
                .map(|s| Value::String(s.clone()))
                .unwrap_or(Value::Null),
        );
        let mut vp_map = Map::new();
        for (k, v) in &meta.voiceprint_versions {
            vp_map.insert(k.clone(), Value::Number((*v).into()));
        }
        map.insert("voiceprint_versions".to_owned(), Value::Object(vp_map));
        map.insert(
            "candidate_evidence".to_owned(),
            Value::Array(
                meta.candidate_evidence
                    .iter()
                    .map(|evidence| {
                        serde_json::json!({
                            "entity_id": evidence.entity_id,
                            "sources": evidence.sources,
                        })
                    })
                    .collect(),
            ),
        );
        if let Some(gaps) = &meta.candidate_evidence_gaps {
            map.insert(
                "candidate_evidence_gaps".to_owned(),
                Value::Array(
                    gaps.iter()
                        .map(|gap| {
                            serde_json::json!({
                                "source": gap.source,
                                "reason": gap.reason,
                            })
                        })
                        .collect(),
                ),
            );
        }
    }
    map
}
fn label_to_value(label: &Label) -> Value {
    let mut map = Map::new();
    map.insert(
        "sentence_id".to_owned(),
        Value::Number(label.sentence_id.into()),
    );
    map.insert(
        "speaker".to_owned(),
        label
            .speaker
            .as_ref()
            .map(|s| Value::String(s.clone()))
            .unwrap_or(Value::Null),
    );
    if let Some(conf) = &label.confidence {
        map.insert("confidence".to_owned(), Value::String(conf.clone()));
    }
    if let Some(method) = &label.method {
        map.insert("method".to_owned(), Value::String(method.clone()));
    }
    if let Some(declined) = label.owner_margin_declined {
        map.insert("owner_margin_declined".to_owned(), Value::Bool(declined));
    }
    if let Some(declined) = label.acoustic_margin_declined {
        map.insert("acoustic_margin_declined".to_owned(), Value::Bool(declined));
    }
    Value::Object(map)
}

fn payload_bytes_newline(bytes: &mut Vec<u8>) {
    if !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
    }
}

/// Execute repair or survey based on request.
pub fn start_repair(
    journal_root: &Path,
    request: StartRepairRequest,
) -> Result<Value, String> {
    if !request.commit {
        let inv = survey_repair_inventory(journal_root)?;
        return Ok(serde_json::json!({
            "mode": "dry_run",
            "complete": inv.complete,
            "clean": inv.clean,
            "entities": inv.entities,
            "planned_removals": inv.planned_removals,
            "planned_segments": inv.planned_segments,
            "gaps": inv.gaps,
            "collision_groups": inv.collision_groups,
            "summary": inv.summary,
        }));
    }

    let operation_id = request
        .operation_id
        .unwrap_or_else(|| format!("repair_{}", Utc::now().timestamp_millis()));

    execute_repair_commit(journal_root, &operation_id, false, request.now_ms)
}

/// Query repair status for an operation ID.
pub fn query_repair_status(
    journal_root: &Path,
    operation_id: &str,
) -> Result<Value, String> {
    let state = fold_repair_operation(journal_root, operation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("operation '{operation_id}' not found in ledger"))?;

    Ok(serde_json::json!({
        "operation_id": operation_id,
        "is_accepted": state.is_accepted,
        "is_completed": state.is_completed,
        "latest_attempt_id": state.latest_attempt_id,
        "checkpointed_count": state.checkpointed_segments.len(),
        "latest_failure": state.latest_failure,
        "summary": state.summary,
    }))
}

/// Resume an existing repair operation ID.
pub fn resume_repair(
    journal_root: &Path,
    operation_id: &str,
    now_ms: i64,
) -> Result<Value, String> {
    execute_repair_commit(journal_root, operation_id, true, now_ms)
}

fn execute_repair_commit(
    journal_root: &Path,
    operation_id: &str,
    is_resume: bool,
    now_ms: i64,
) -> Result<Value, String> {
    // 1. Acquire execution lock (timeout ZERO)
    let _exec_lock = acquire_repair_lock(journal_root).map_err(|e| {
        format!("failed to acquire speaker repair execution lock: {e}")
    })?;

    // 2. Hold entity trust lock
    let _trust_lock = hold_entity_trust_lock(journal_root).map_err(|e| {
        format!("failed to acquire entity trust lock: {e}")
    })?;

    // Load existing state if any
    let prior_state = fold_repair_operation(journal_root, operation_id).map_err(|e| e.to_string())?;

    if let Some(state) = &prior_state
        && state.is_completed
    {
        return Ok(serde_json::json!({
            "operation_id": operation_id,
            "status": "completed",
            "summary": state.summary,
        }));
    }

    // 3. Re-inventory under trust
    let inventory = survey_repair_inventory(journal_root)?;

    if !inventory.complete {
        if is_resume || prior_state.as_ref().map(|s| s.is_accepted).unwrap_or(false) {
            let attempt_id = format!("attempt_{}", Utc::now().timestamp_millis());
            let _ = append_repair_event(
                journal_root,
                &RepairEvent::AttemptFailed {
                    schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                    operation_id: operation_id.to_owned(),
                    attempt_id,
                    stage: "re_inventory".to_owned(),
                    detail: "re-inventory incomplete with gaps".to_owned(),
                    retryable: true,
                    timestamp: Utc::now().to_rfc3339(),
                },
            );
        }
        return Err(format!(
            "refusing repair: journal inventory is incomplete ({} gaps found)",
            inventory.gaps.len()
        ));
    }

    // If complete and nothing to repair on a new commit -> no-op
    if !is_resume
        && prior_state.is_none()
        && inventory.planned_removals.is_empty()
        && inventory.planned_segments.is_empty()
    {
        return Ok(serde_json::json!({
            "operation_id": operation_id,
            "status": "completed",
            "summary": {
                "complete": true,
                "clean": inventory.clean,
                "message": "no repairable contamination found",
            }
        }));
    }

    let attempt_id = format!("attempt_{}", Utc::now().timestamp_millis());

    // Append accepted if not already accepted
    if prior_state.as_ref().map(|s| !s.is_accepted).unwrap_or(true) {
        append_repair_event(
            journal_root,
            &RepairEvent::Accepted {
                schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                operation_id: operation_id.to_owned(),
                timestamp: Utc::now().to_rfc3339(),
            },
        )
        .map_err(|e| e.to_string())?;
    }

    append_repair_event(
        journal_root,
        &RepairEvent::AttemptStarted {
            schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
            operation_id: operation_id.to_owned(),
            attempt_id: attempt_id.clone(),
            timestamp: Utc::now().to_rfc3339(),
        },
    )
    .map_err(|e| e.to_string())?;

    // 5. Cleanup first: remove voiceprint rows from repairable non-persons
    for removal in &inventory.planned_removals {
        let memory_dir = match entity_memory_path(journal_root, &removal.entity_id, false) {
            Ok(d) => d,
            Err(err) => {
                let _ = append_repair_event(
                    journal_root,
                    &RepairEvent::AttemptFailed {
                        schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                        operation_id: operation_id.to_owned(),
                        attempt_id: attempt_id.clone(),
                        stage: "directory_proof".to_owned(),
                        detail: format!("entity_memory_path failed for {}: {}", removal.entity_id, err),
                        retryable: false,
                        timestamp: Utc::now().to_rfc3339(),
                    },
                );
                return Err(format!("directory proof failed for {}: {}", removal.entity_id, err));
            }
        };

        let expected_dir = journal_root.join("entities").join(&removal.entity_dir);
        if memory_dir != expected_dir {
            let _ = append_repair_event(
                journal_root,
                &RepairEvent::AttemptFailed {
                    schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                    operation_id: operation_id.to_owned(),
                    attempt_id: attempt_id.clone(),
                    stage: "directory_proof".to_owned(),
                    detail: format!(
                        "directory proof mismatch for {}: {:?} vs {:?}",
                        removal.entity_id, memory_dir, expected_dir
                    ),
                    retryable: false,
                    timestamp: Utc::now().to_rfc3339(),
                },
            );
            return Err(format!("directory proof mismatch for {}", removal.entity_id));
        }

        let running_encoder = match try_load_entity_voiceprints_in_dir(journal_root, &removal.entity_dir) {
            Ok(Some(archive)) => archive.envelope.encoder.unwrap_or_else(|| EncoderIdentity {
                id: "dummy".to_owned(),
                sha256: "dummy".to_owned(),
                width: 512,
            }),
            Ok(None) => EncoderIdentity {
                id: "dummy".to_owned(),
                sha256: "dummy".to_owned(),
                width: 512,
            },
            Err(err) => {
                let _ = append_repair_event(
                    journal_root,
                    &RepairEvent::AttemptFailed {
                        schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                        operation_id: operation_id.to_owned(),
                        attempt_id: attempt_id.clone(),
                        stage: "voiceprint_removal".to_owned(),
                        detail: format!(
                            "failed loading voiceprint archive for {}: {}",
                            removal.entity_id, err
                        ),
                        retryable: false,
                        timestamp: Utc::now().to_rfc3339(),
                    },
                );
                return Err(format!(
                    "failed loading voiceprint archive for {}: {}",
                    removal.entity_id, err
                ));
            }
        };

        let removals = removal
            .voiceprint_keys
            .iter()
            .map(|k| VoiceprintRemoval {
                key: k.clone(),
                expected_metadata: Some(k.clone()),
            })
            .collect::<Vec<_>>();

        match remove_voiceprints_by_key(journal_root, &removal.entity_id, &removals, &running_encoder) {
            Ok(delta) => {
                if delta.skipped_reasons.metadata_mismatch > 0
                    || delta.skipped_count > delta.skipped_reasons.missing
                {
                    let _ = append_repair_event(
                        journal_root,
                        &RepairEvent::AttemptFailed {
                            schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                            operation_id: operation_id.to_owned(),
                            attempt_id: attempt_id.clone(),
                            stage: "voiceprint_removal".to_owned(),
                            detail: format!(
                                "voiceprint removal unexpected skip/mismatch for {}: total_skipped={}, missing={}, mismatch={}",
                                removal.entity_id,
                                delta.skipped_count,
                                delta.skipped_reasons.missing,
                                delta.skipped_reasons.metadata_mismatch,
                            ),
                            retryable: false,
                            timestamp: Utc::now().to_rfc3339(),
                        },
                    );
                    return Err(format!(
                        "voiceprint removal unexpected skip for {}",
                        removal.entity_id
                    ));
                }
            }
            Err(err) => {
                let _ = append_repair_event(
                    journal_root,
                    &RepairEvent::AttemptFailed {
                        schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                        operation_id: operation_id.to_owned(),
                        attempt_id: attempt_id.clone(),
                        stage: "voiceprint_removal".to_owned(),
                        detail: format!("voiceprint removal failed for {}: {}", removal.entity_id, err),
                        retryable: false,
                        timestamp: Utc::now().to_rfc3339(),
                    },
                );
                return Err(format!(
                    "voiceprint removal failed for {}: {}",
                    removal.entity_id, err
                ));
            }
        }
    }

    // 6. Prepared snapshot
    let planned_snapshots = inventory
        .planned_segments
        .iter()
        .map(|seg| PreparedSegmentSnapshot {
            day: seg.day.clone(),
            stream_layout: seg.stream_layout,
            stream: seg.stream.clone(),
            segment_name: seg.segment_name.clone(),
            label_sha256: seg.current_label_sha256.clone(),
            corrections_sha256: seg.current_corrections_sha256.clone(),
        })
        .collect::<Vec<_>>();

    append_repair_event(
        journal_root,
        &RepairEvent::Prepared {
            schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
            operation_id: operation_id.to_owned(),
            attempt_id: attempt_id.clone(),
            planned_segments: planned_snapshots.clone(),
            timestamp: Utc::now().to_rfc3339(),
        },
    )
    .map_err(|e| e.to_string())?;

    // Refold state to get latest intents and checkpoints
    let state = fold_repair_operation(journal_root, operation_id)
        .map_err(|e| e.to_string())?
        .unwrap_or_default();

    // 7. Process each segment
    for plan in &inventory.planned_segments {
        let tuple_key = SegmentTupleKey::new(
            &plan.day,
            plan.stream_layout,
            &plan.stream,
            &plan.segment_name,
        );

        if state.checkpointed_segments.contains(&tuple_key) {
            continue;
        }

        let label_p = labels_path(&plan.segment_dir);
        let corr_p = corrections_path(&plan.segment_dir);

        // Hold correction lock THEN label lock
        let _corr_lock = hold_lock(&corr_p, LockOptions::default()).map_err(|e| e.to_string())?;
        let _label_lock = hold_lock(&label_p, LockOptions::default()).map_err(|e| e.to_string())?;

        let current_label_sha = compute_file_sha256(&label_p).map_err(|e| e.to_string())?;
        let current_corr_sha = compute_file_sha256(&corr_p).map_err(|e| e.to_string())?;

        let latest_intent = state.latest_intents.get(&tuple_key);

        if let Some(intent) = latest_intent {
            let h0 = &intent.expected_current_label_sha256;
            let ha = &intent.intended_payload_sha256;
            let c0 = &intent.expected_corrections_sha256;

            if current_label_sha == *ha && current_corr_sha == *c0 {
                // Landed: checkpoint without rewrite
                append_repair_event(
                    journal_root,
                    &RepairEvent::Checkpoint {
                        schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                        operation_id: operation_id.to_owned(),
                        attempt_id: attempt_id.clone(),
                        intent_id: intent.intent_id.clone(),
                        day: plan.day.clone(),
                        stream_layout: plan.stream_layout,
                        stream: plan.stream.clone(),
                        segment_name: plan.segment_name.clone(),
                        timestamp: Utc::now().to_rfc3339(),
                    },
                )
                .map_err(|e| e.to_string())?;
                continue;
            } else if current_label_sha == *h0 && current_corr_sha == *c0 {
                // Did not land: re-resolve
                let outcome = resolve(
                    journal_root,
                    &plan.day,
                    &plan.stream,
                    &plan.segment_name,
                    plan.stream_layout,
                    true,
                    now_ms,
                )
                .map_err(|e| e.to_string())?;

                let (resolved_labels, outcome_metadata) = match outcome {
                    ResolveOutcome::Resolved(box_out) => (box_out.labels, Some(box_out.metadata)),
                    _ => (Vec::new(), None),
                };

                let corrections = read_corrections(&plan.segment_dir).map_err(|e| e.to_string())?;
                let current_labels_val = if label_p.is_file() {
                    let b = fs::read(&label_p).map_err(|e| e.to_string())?;
                    serde_json::from_slice::<Value>(&b).map_err(|e| e.to_string())?
                } else {
                    json!({})
                };

                let meta_map_val = metadata_map(outcome_metadata.as_ref());
                let fresh_labels = resolved_labels.iter().map(label_to_value).collect::<Vec<_>>();
                let payload_val = build_repaired_label_payload(
                    current_labels_val.as_object(),
                    corrections,
                    fresh_labels,
                    &meta_map_val,
                )
                .map_err(|e| e.to_string())?;

                let mut payload_bytes = write_python_compatible_json(&payload_val, 2)
                    .map_err(|e| e.to_string())?
                    .into_bytes();
                payload_bytes_newline(&mut payload_bytes);

                let new_intended_sha = compute_bytes_sha256(&payload_bytes);

                let final_intent_id = if new_intended_sha == *ha {
                    intent.intent_id.clone()
                } else {
                    let super_intent_id = format!("{}_super_{}_{}", attempt_id, plan.day, plan.segment_name);
                    append_repair_event(
                        journal_root,
                        &RepairEvent::WriteIntent {
                            schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                            operation_id: operation_id.to_owned(),
                            attempt_id: attempt_id.clone(),
                            intent_id: super_intent_id.clone(),
                            supersedes_intent_id: Some(intent.intent_id.clone()),
                            day: plan.day.clone(),
                            stream_layout: plan.stream_layout,
                            stream: plan.stream.clone(),
                            segment_name: plan.segment_name.clone(),
                            expected_current_label_sha256: h0.clone(),
                            expected_corrections_sha256: c0.clone(),
                            intended_payload_sha256: new_intended_sha.clone(),
                            intended_payload: payload_val.clone(),
                            timestamp: Utc::now().to_rfc3339(),
                        },
                    )
                    .map_err(|e| e.to_string())?;
                    super_intent_id
                };

                replace_labels_if_current_hash_matches_locked(
                    &plan.segment_dir,
                    h0,
                    c0,
                    &payload_val,
                    &new_intended_sha,
                    &_label_lock,
                )
                .map_err(|e| e.to_string())?;

                append_repair_event(
                    journal_root,
                    &RepairEvent::Checkpoint {
                        schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                        operation_id: operation_id.to_owned(),
                        attempt_id: attempt_id.clone(),
                        intent_id: final_intent_id,
                        day: plan.day.clone(),
                        stream_layout: plan.stream_layout,
                        stream: plan.stream.clone(),
                        segment_name: plan.segment_name.clone(),
                        timestamp: Utc::now().to_rfc3339(),
                    },
                )
                .map_err(|e| e.to_string())?;
                continue;
            } else {
                let _ = append_repair_event(
                    journal_root,
                    &RepairEvent::AttemptFailed {
                        schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                        operation_id: operation_id.to_owned(),
                        attempt_id: attempt_id.clone(),
                        stage: "resume_matrix".to_owned(),
                        detail: format!(
                            "resumable conflict on segment {}: disk label sha {} vs expected ({}, {}), disk corr sha {} vs {}",
                            plan.segment_name, current_label_sha, h0, ha, current_corr_sha, c0
                        ),
                        retryable: true,
                        timestamp: Utc::now().to_rfc3339(),
                    },
                );
                return Err(format!("resumable conflict on segment {}", plan.segment_name));
            }
        } else {
            // First time processing this segment
            if current_label_sha != plan.current_label_sha256
                || current_corr_sha != plan.current_corrections_sha256
            {
                let _ = append_repair_event(
                    journal_root,
                    &RepairEvent::AttemptFailed {
                        schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                        operation_id: operation_id.to_owned(),
                        attempt_id: attempt_id.clone(),
                        stage: "prepared_validation".to_owned(),
                        detail: format!(
                            "label or correction drift on segment {} before write",
                            plan.segment_name
                        ),
                        retryable: true,
                        timestamp: Utc::now().to_rfc3339(),
                    },
                );
                return Err(format!(
                    "drift detected on segment {} before write",
                    plan.segment_name
                ));
            }

            let outcome = resolve(
                journal_root,
                &plan.day,
                &plan.stream,
                &plan.segment_name,
                plan.stream_layout,
                true,
                now_ms,
            )
            .map_err(|e| e.to_string())?;

            let (resolved_labels, outcome_metadata) = match outcome {
                ResolveOutcome::Resolved(box_out) => (box_out.labels, Some(box_out.metadata)),
                _ => (Vec::new(), None),
            };

            let corrections = read_corrections(&plan.segment_dir).map_err(|e| e.to_string())?;
            let current_labels_val = if label_p.is_file() {
                let b = fs::read(&label_p).map_err(|e| e.to_string())?;
                serde_json::from_slice::<Value>(&b).map_err(|e| e.to_string())?
            } else {
                json!({})
            };

            let meta_map_val = metadata_map(outcome_metadata.as_ref());
            let fresh_labels = resolved_labels.iter().map(label_to_value).collect::<Vec<_>>();
            let payload_val = build_repaired_label_payload(
                current_labels_val.as_object(),
                corrections,
                fresh_labels,
                &meta_map_val,
            )
            .map_err(|e| e.to_string())?;

            let mut payload_bytes = write_python_compatible_json(&payload_val, 2)
                .map_err(|e| e.to_string())?
                .into_bytes();
            payload_bytes_newline(&mut payload_bytes);

            let intended_sha = compute_bytes_sha256(&payload_bytes);
            let intent_id = format!("{}_seg_{}_{}", attempt_id, plan.day, plan.segment_name);

            append_repair_event(
                journal_root,
                &RepairEvent::WriteIntent {
                    schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                    operation_id: operation_id.to_owned(),
                    attempt_id: attempt_id.clone(),
                    intent_id: intent_id.clone(),
                    supersedes_intent_id: None,
                    day: plan.day.clone(),
                    stream_layout: plan.stream_layout,
                    stream: plan.stream.clone(),
                    segment_name: plan.segment_name.clone(),
                    expected_current_label_sha256: current_label_sha.clone(),
                    expected_corrections_sha256: current_corr_sha.clone(),
                    intended_payload_sha256: intended_sha.clone(),
                    intended_payload: payload_val.clone(),
                    timestamp: Utc::now().to_rfc3339(),
                },
            )
            .map_err(|e| e.to_string())?;

            replace_labels_if_current_hash_matches_locked(
                &plan.segment_dir,
                &current_label_sha,
                &current_corr_sha,
                &payload_val,
                &intended_sha,
                &_label_lock,
            )
            .map_err(|e| e.to_string())?;

            append_repair_event(
                journal_root,
                &RepairEvent::Checkpoint {
                    schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                    operation_id: operation_id.to_owned(),
                    attempt_id: attempt_id.clone(),
                    intent_id,
                    day: plan.day.clone(),
                    stream_layout: plan.stream_layout,
                    stream: plan.stream.clone(),
                    segment_name: plan.segment_name.clone(),
                    timestamp: Utc::now().to_rfc3339(),
                },
            )
            .map_err(|e| e.to_string())?;
        }
    }

    // 8. Re-survey final inventory to confirm complete/clean status
    let final_inv = survey_repair_inventory(journal_root)?;
    let summary = json!({
        "complete": final_inv.complete,
        "clean": final_inv.clean,
        "planned_removals_count": final_inv.planned_removals.len(),
        "planned_segments_count": final_inv.planned_segments.len(),
    });

    append_repair_event(
        journal_root,
        &RepairEvent::Completed {
            schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
            operation_id: operation_id.to_owned(),
            attempt_id,
            summary: summary.clone(),
            timestamp: Utc::now().to_rfc3339(),
        },
    )
    .map_err(|e| e.to_string())?;

    Ok(serde_json::json!({
        "operation_id": operation_id,
        "status": "completed",
        "summary": summary,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;
    use solstone_core_journal_io::SegmentLayout;
    use crate::repair_operations::ledger_path;

    fn make_owner(root: &Path, id: &str) {
        let dir = root.join("entities").join(id);
        fs::create_dir_all(&dir).unwrap();
        let entity_json = serde_json::json!({
            "id": id,
            "type": "Person",
            "blocked": false,
            "is_principal": true,
            "names": [id],
            "display_name": id
        });
        fs::write(dir.join("entity.json"), serde_json::to_vec_pretty(&entity_json).unwrap()).unwrap();

        // Write owner centroid
        let centroid_npy = solstone_core_npy::write_npy("<f4", "(256,)", &vec![0u8; 256 * 4]);
        let thresh_npy = solstone_core_npy::write_npy("<f4", "()", &0.43f32.to_le_bytes());
        let cluster_npy = solstone_core_npy::write_npy("<i4", "()", &1i32.to_le_bytes());
        let mut zip_bytes = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut zip_bytes));
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("centroid.npy", options).unwrap();
            std::io::Write::write_all(&mut zip, &centroid_npy).unwrap();
            zip.start_file("threshold.npy", options).unwrap();
            std::io::Write::write_all(&mut zip, &thresh_npy).unwrap();
            zip.start_file("cluster_size.npy", options).unwrap();
            std::io::Write::write_all(&mut zip, &cluster_npy).unwrap();
            zip.finish().unwrap();
        }
        fs::write(dir.join("owner_centroid.npz"), &zip_bytes).unwrap();
    }

    fn make_non_person_with_vp(root: &Path, id: &str, entity_type: &str, key: &str) {
        let dir = root.join("entities").join(id);
        fs::create_dir_all(&dir).unwrap();
        let entity_json = serde_json::json!({
            "id": id,
            "type": entity_type,
            "blocked": false,
            "is_principal": false,
            "names": [id],
            "display_name": id
        });
        fs::write(dir.join("entity.json"), serde_json::to_vec_pretty(&entity_json).unwrap()).unwrap();

        let items = vec![solstone_core_entity::VoiceprintItem {
            embedding: vec![1.0; 256],
            metadata: serde_json::json!({
                "day": "20260101",
                "segment_key": key,
                "source": "audio",
                "sentence_id": 1,
            }),
        }];
        let encoder = solstone_core_entity::EncoderIdentity {
            id: "test".to_string(),
            sha256: "0".repeat(64),
            width: 256,
        };
        solstone_core_entity::save_voiceprints_batch(root, id, &items, &encoder).unwrap();
    }

    fn make_segment_with_labels(root: &Path, day: &str, segment_name: &str, labels_val: Value) {
        let seg_dir = root.join("chronicle").join(day).join(segment_name);
        let talents_dir = seg_dir.join("talents");
        fs::create_dir_all(&talents_dir).unwrap();

        let mut bytes = write_python_compatible_json(&labels_val, 2).unwrap().into_bytes();
        payload_bytes_newline(&mut bytes);
        fs::write(talents_dir.join("speaker_labels.json"), bytes).unwrap();
    }

    #[test]
    fn test_repair_dry_run_and_commit_happy_path() {
        let temp = tempdir().unwrap();
        let root = temp.path();

        make_owner(root, "owner_user");
        make_non_person_with_vp(root, "coffee_shop", "Place", "120000_300");

        let initial_labels = serde_json::json!({
            "labels": [
                {"sentence_id": 1, "speaker": "coffee_shop", "method": "acoustic", "confidence": "high"}
            ]
        });
        make_segment_with_labels(root, "20260101", "120000_300", initial_labels);

        // 1. Dry run
        let req_dry = StartRepairRequest {
            operation_id: None,
            commit: false,
            now_ms: 1000,
        };
        let dry_res = start_repair(root, req_dry).unwrap();
        assert_eq!(dry_res["mode"], "dry_run");
        assert_eq!(dry_res["complete"], true);
        assert_eq!(dry_res["clean"], false);
        assert_eq!(dry_res["planned_removals"].as_array().unwrap().len(), 1);
        assert_eq!(dry_res["planned_segments"].as_array().unwrap().len(), 1);

        // Ensure ledger is untouched in dry run
        assert!(!ledger_path(root).exists());

        // Check owner centroid hash before commit
        let owner_centroid_before = fs::read(root.join("entities/owner_user/owner_centroid.npz")).unwrap();
        let owner_centroid_sha_before = compute_bytes_sha256(&owner_centroid_before);

        // 2. Commit
        let req_commit = StartRepairRequest {
            operation_id: Some("op_test_1".to_owned()),
            commit: true,
            now_ms: 1000,
        };
        let commit_res = start_repair(root, req_commit).unwrap();
        assert_eq!(commit_res["status"], "completed");
        assert_eq!(commit_res["summary"]["complete"], true);

        // Verify owner centroid was NOT touched
        let owner_centroid_after = fs::read(root.join("entities/owner_user/owner_centroid.npz")).unwrap();
        let owner_centroid_sha_after = compute_bytes_sha256(&owner_centroid_after);
        assert_eq!(owner_centroid_sha_before, owner_centroid_sha_after);

        // Verify non-Person voiceprint was removed
        let vp = try_load_entity_voiceprints_in_dir(root, "coffee_shop").unwrap();
        if let Some(archive) = vp {
            assert_eq!(archive.metadata.len(), 0);
        }

        // Verify ledger was written with proper events
        let ledger = crate::repair_operations::load_repair_ledger(root).unwrap();
        assert!(ledger.iter().any(|e| matches!(e, RepairEvent::Accepted { .. })));
        assert!(ledger.iter().any(|e| matches!(e, RepairEvent::Completed { .. })));

        // 3. Re-run repair -> should be no-op complete
        let req_rerun = StartRepairRequest {
            operation_id: Some("op_test_2".to_owned()),
            commit: true,
            now_ms: 2000,
        };
        let rerun_res = start_repair(root, req_rerun).unwrap();
        assert_eq!(rerun_res["status"], "completed");
    }

    #[test]
    fn test_repair_refusal_on_gap() {
        let temp = tempdir().unwrap();
        let root = temp.path();

        make_owner(root, "owner_user");
        make_non_person_with_vp(root, "coffee_shop", "Place", "120000_300");

        // Reference a missing entity in labels
        let bad_labels = serde_json::json!({
            "labels": [
                {"sentence_id": 1, "speaker": "ghost_entity", "method": "acoustic", "confidence": "high"}
            ]
        });
        make_segment_with_labels(root, "20260101", "120000_300", bad_labels);

        let req_commit = StartRepairRequest {
            operation_id: Some("op_gap_1".to_owned()),
            commit: true,
            now_ms: 1000,
        };
        let err = start_repair(root, req_commit).unwrap_err();
        assert!(err.contains("refusing repair: journal inventory is incomplete"));

        // Ledger must NOT contain accepted event
        assert!(!ledger_path(root).exists());
    }

    fn collect_all_files(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files = Vec::new();
        fn visit(dir: &Path, root: &Path, files: &mut Vec<(PathBuf, Vec<u8>)>) {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        visit(&path, root, files);
                    } else if path.is_file() {
                        let rel = path.strip_prefix(root).unwrap().to_path_buf();
                        if !rel.starts_with("health/locks") && !rel.to_string_lossy().ends_with(".lock") {
                            if let Ok(b) = fs::read(&path) {
                                files.push((rel, b));
                            }
                        }
                    }
                }
            }
        }
        visit(root, root, &mut files);
        files.sort_by(|a, b| a.0.cmp(&b.0));
        files
    }

    #[test]
    fn test_lock_identity_and_drift_rejection() {
        let temp = tempdir().unwrap();
        let root = temp.path();

        make_owner(root, "owner_user");
        make_non_person_with_vp(root, "coffee_shop", "Place", "120000_300");

        let initial_labels = serde_json::json!({
            "labels": [
                {"sentence_id": 1, "speaker": "coffee_shop", "method": "acoustic", "confidence": "high"}
            ]
        });
        make_segment_with_labels(root, "20260101", "120000_300", initial_labels);

        let seg_dir = root.join("chronicle/20260101/120000_300");
        let label_p = labels_path(&seg_dir);
        let corr_p = corrections_path(&seg_dir);

        // Lock path integrity check: hold_lock creates speaker_labels.json.lock without doubling
        let _lock = hold_lock(&label_p, LockOptions::default()).unwrap();
        assert!(label_p.with_file_name("speaker_labels.json.lock").exists());
        drop(_lock);

        // Prove AC12: Drift rejection after prepared snapshot
        let op_id = "op_drift_reject";
        let att_id = "att_drift";
        let label_sha = compute_file_sha256(&label_p).unwrap();
        let corr_sha = compute_file_sha256(&corr_p).unwrap();

        append_repair_event(
            root,
            &RepairEvent::Accepted {
                schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                operation_id: op_id.to_owned(),
                timestamp: Utc::now().to_rfc3339(),
            },
        )
        .unwrap();

        append_repair_event(
            root,
            &RepairEvent::AttemptStarted {
                schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                operation_id: op_id.to_owned(),
                attempt_id: att_id.to_owned(),
                timestamp: Utc::now().to_rfc3339(),
            },
        )
        .unwrap();

        append_repair_event(
            root,
            &RepairEvent::Prepared {
                schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                operation_id: op_id.to_owned(),
                attempt_id: att_id.to_owned(),
                planned_segments: vec![PreparedSegmentSnapshot {
                    day: "20260101".to_owned(),
                    stream_layout: SegmentLayout::Direct,
                    stream: "_default".to_owned(),
                    segment_name: "120000_300".to_owned(),
                    label_sha256: label_sha.clone(),
                    corrections_sha256: corr_sha.clone(),
                }],
                timestamp: Utc::now().to_rfc3339(),
            },
        )
        .unwrap();

        append_repair_event(
            root,
            &RepairEvent::WriteIntent {
                schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                operation_id: op_id.to_owned(),
                attempt_id: att_id.to_owned(),
                intent_id: "intent_drift".to_owned(),
                supersedes_intent_id: None,
                day: "20260101".to_owned(),
                stream_layout: SegmentLayout::Direct,
                stream: "_default".to_owned(),
                segment_name: "120000_300".to_owned(),
                expected_current_label_sha256: label_sha,
                expected_corrections_sha256: corr_sha,
                intended_payload_sha256: "intended_sha_placeholder".to_owned(),
                intended_payload: serde_json::json!({"labels": []}),
                timestamp: Utc::now().to_rfc3339(),
            },
        )
        .unwrap();

        // Mutate corrections before resume
        let mut corr_map = serde_json::Map::new();
        corr_map.insert("sentence_id".to_owned(), serde_json::json!(1));
        corr_map.insert("corrected_speaker".to_owned(), serde_json::json!("owner_user"));
        solstone_core_speaker_id::corrections::append_correction(&seg_dir, corr_map).unwrap();

        let labels_before_resume = fs::read(&label_p).unwrap();
        let corr_before_resume = fs::read(&corr_p).unwrap();

        // Resume must fail with resumable conflict
        let resume_res = resume_repair(root, op_id, 1000);
        assert!(resume_res.is_err());

        // Labels and corrections bytes must be preserved
        let labels_after_resume = fs::read(&label_p).unwrap();
        let corr_after_resume = fs::read(&corr_p).unwrap();
        assert_eq!(labels_before_resume, labels_after_resume);
        assert_eq!(corr_before_resume, corr_after_resume);

        let ledger = crate::repair_operations::load_repair_ledger(root).unwrap();
        assert!(ledger.iter().any(|e| match e {
            RepairEvent::AttemptFailed { stage, .. } => stage == "resume_matrix",
            _ => false,
        }));
    }

    #[test]
    fn test_twins_direct_and_named_isolation() {
        let temp = tempdir().unwrap();
        let root = temp.path();

        make_owner(root, "owner_user");
        make_non_person_with_vp(root, "coffee_shop", "Place", "093000_300");

        let labels = serde_json::json!({
            "labels": [
                {"sentence_id": 1, "speaker": "coffee_shop", "method": "acoustic", "confidence": "high"}
            ]
        });

        // 1. Direct segment: chronicle/20260101/093000_300
        make_segment_with_labels(root, "20260101", "093000_300", labels.clone());

        // 2. Named stream segment twin A: chronicle/20260101/mic/093000_300_a
        let twin_a = root.join("chronicle/20260101/mic/093000_300_a/talents");
        fs::create_dir_all(&twin_a).unwrap();
        let mut ba = write_python_compatible_json(&labels, 2).unwrap().into_bytes();
        payload_bytes_newline(&mut ba);
        fs::write(twin_a.join("speaker_labels.json"), &ba).unwrap();

        // 3. Named stream segment twin B: chronicle/20260101/mic/093000_300_b
        let twin_b = root.join("chronicle/20260101/mic/093000_300_b/talents");
        fs::create_dir_all(&twin_b).unwrap();
        let mut bb = write_python_compatible_json(&labels, 2).unwrap().into_bytes();
        payload_bytes_newline(&mut bb);
        fs::write(twin_b.join("speaker_labels.json"), &bb).unwrap();

        // Seed accepted + attempt + checkpoint for twin_a ONLY
        let op_id = "op_twins_resume";
        append_repair_event(
            root,
            &RepairEvent::Accepted {
                schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                operation_id: op_id.to_owned(),
                timestamp: Utc::now().to_rfc3339(),
            },
        )
        .unwrap();

        append_repair_event(
            root,
            &RepairEvent::AttemptStarted {
                schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                operation_id: op_id.to_owned(),
                attempt_id: "att_prev".to_owned(),
                timestamp: Utc::now().to_rfc3339(),
            },
        )
        .unwrap();

        append_repair_event(
            root,
            &RepairEvent::Checkpoint {
                schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                operation_id: op_id.to_owned(),
                attempt_id: "att_prev".to_owned(),
                intent_id: "intent_a".to_owned(),
                day: "20260101".to_owned(),
                stream_layout: SegmentLayout::Named,
                stream: "mic".to_owned(),
                segment_name: "093000_300_a".to_owned(),
                timestamp: Utc::now().to_rfc3339(),
            },
        )
        .unwrap();

        let twin_a_bytes_before = fs::read(twin_a.join("speaker_labels.json")).unwrap();

        let res = resume_repair(root, op_id, 1000).unwrap();
        assert_eq!(res["status"], "completed");

        // Twin A labels must be untouched
        let twin_a_bytes_after = fs::read(twin_a.join("speaker_labels.json")).unwrap();
        assert_eq!(twin_a_bytes_before, twin_a_bytes_after);

        // Twin B labels and direct labels must have been processed/checkpointed
        let ledger = crate::repair_operations::load_repair_ledger(root).unwrap();
        let checkpoints: Vec<_> = ledger
            .iter()
            .filter_map(|e| match e {
                RepairEvent::Checkpoint { segment_name, stream_layout, .. } => {
                    Some((segment_name.clone(), *stream_layout))
                }
                _ => None,
            })
            .collect();

        assert!(checkpoints.contains(&("093000_300_a".to_owned(), SegmentLayout::Named)));
        assert!(checkpoints.contains(&("093000_300_b".to_owned(), SegmentLayout::Named)));
        assert!(checkpoints.contains(&("093000_300".to_owned(), SegmentLayout::Direct)));
    }

    #[test]
    fn test_tree_comparison_and_no_sidecar_pollution() {
        let temp = tempdir().unwrap();
        let root = temp.path();

        make_owner(root, "owner_user");
        make_non_person_with_vp(root, "coffee_shop", "Place", "120000_300");

        let initial_labels = serde_json::json!({
            "labels": [
                {"sentence_id": 1, "speaker": "coffee_shop", "method": "acoustic", "confidence": "high"}
            ]
        });
        make_segment_with_labels(root, "20260101", "120000_300", initial_labels);

        let files_before = collect_all_files(root);

        let req_commit = StartRepairRequest {
            operation_id: Some("op_tree_check".to_owned()),
            commit: true,
            now_ms: 1000,
        };
        let res = start_repair(root, req_commit).unwrap();
        assert_eq!(res["status"], "completed");

        let files_after = collect_all_files(root);

        for (rel, bytes_after) in &files_after {
            let rel_str = rel.to_string_lossy();
            let before_match = files_before.iter().find(|(b_rel, _)| b_rel == rel);

            if let Some((_, bytes_before)) = before_match {
                if bytes_before != bytes_after {
                    // Allowed modified files: planned NP voiceprints, speaker_labels.json, repair-operations.jsonl
                    let allowed = rel_str == "entities/coffee_shop/voiceprints.npz"
                        || rel_str.ends_with("speaker_labels.json")
                        || rel_str == "speakers/repair-operations.jsonl";
                    assert!(allowed, "Unexpected modification to file: {}", rel_str);
                }
            } else {
                // Allowed new files: repair-operations.jsonl
                let allowed = rel_str == "speakers/repair-operations.jsonl";
                assert!(allowed, "Unexpected new file created: {}", rel_str);
            }
        }

        // Verify ambiguities.jsonl does not exist or was untouched
        assert!(!root.join("entities/ambiguities.jsonl").exists());
    }

    #[test]
    fn test_second_commit_is_noop() {
        let temp = tempdir().unwrap();
        let root = temp.path();

        make_owner(root, "owner_user");
        make_non_person_with_vp(root, "coffee_shop", "Place", "120000_300");

        let initial_labels = serde_json::json!({
            "labels": [
                {"sentence_id": 1, "speaker": "coffee_shop", "method": "acoustic", "confidence": "high"}
            ]
        });
        make_segment_with_labels(root, "20260101", "120000_300", initial_labels);

        // First commit
        let res1 = start_repair(
            root,
            StartRepairRequest {
                operation_id: Some("op_first".to_owned()),
                commit: true,
                now_ms: 1000,
            },
        )
        .unwrap();
        assert_eq!(res1["status"], "completed");

        let ledger_bytes_1 = fs::read(ledger_path(root)).unwrap();

        // Second commit with new operation ID
        let res2 = start_repair(
            root,
            StartRepairRequest {
                operation_id: Some("op_second".to_owned()),
                commit: true,
                now_ms: 2000,
            },
        )
        .unwrap();
        assert_eq!(res2["status"], "completed");

        // Ledger must be byte-identical: no new accepted/events appended
        let ledger_bytes_2 = fs::read(ledger_path(root)).unwrap();
        assert_eq!(ledger_bytes_1, ledger_bytes_2);
    }

    #[test]
    fn test_write_intent_resume_matrix_cases() {
        // Case 1: Landed - disk labels hash == intended, corr unchanged -> resume checkpoints without changing label bytes
        {
            let temp = tempdir().unwrap();
            let root = temp.path();

            make_owner(root, "owner_user");
            make_non_person_with_vp(root, "coffee_shop", "Place", "120000_300");

            let initial_labels = serde_json::json!({
                "labels": [
                    {"sentence_id": 1, "speaker": "coffee_shop", "method": "acoustic", "confidence": "high"}
                ]
            });
            make_segment_with_labels(root, "20260101", "120000_300", initial_labels);

            let seg_dir = root.join("chronicle/20260101/120000_300");
            let label_p = labels_path(&seg_dir);
            let corr_p = corrections_path(&seg_dir);

            let current_label_sha = compute_file_sha256(&label_p).unwrap();
            let current_corr_sha = compute_file_sha256(&corr_p).unwrap();

            let op_id = "op_resume_landed";
            let att_id = "att_1";
            append_repair_event(
                root,
                &RepairEvent::Accepted {
                    schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                    operation_id: op_id.to_owned(),
                    timestamp: Utc::now().to_rfc3339(),
                },
            )
            .unwrap();

            // Intended hash matches disk label sha
            append_repair_event(
                root,
                &RepairEvent::WriteIntent {
                    schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                    operation_id: op_id.to_owned(),
                    attempt_id: att_id.to_owned(),
                    intent_id: "intent_landed".to_owned(),
                    supersedes_intent_id: None,
                    day: "20260101".to_owned(),
                    stream_layout: SegmentLayout::Direct,
                    stream: "_default".to_owned(),
                    segment_name: "120000_300".to_owned(),
                    expected_current_label_sha256: "0".repeat(64),
                    expected_corrections_sha256: current_corr_sha.clone(),
                    intended_payload_sha256: current_label_sha.clone(),
                    intended_payload: serde_json::json!({}),
                    timestamp: Utc::now().to_rfc3339(),
                },
            )
            .unwrap();

            let bytes_before = fs::read(&label_p).unwrap();
            let resume_res = resume_repair(root, op_id, 1000).unwrap();
            assert_eq!(resume_res["status"], "completed");

            let bytes_after = fs::read(&label_p).unwrap();
            assert_eq!(bytes_before, bytes_after);

            let ledger = crate::repair_operations::load_repair_ledger(root).unwrap();
            assert!(ledger.iter().any(|e| match e {
                RepairEvent::Checkpoint { intent_id, .. } => intent_id == "intent_landed",
                _ => false,
            }));
        }

        // Case 2: Not landed - disk still original h0 -> resume writes and checkpoints
        {
            let temp = tempdir().unwrap();
            let root = temp.path();

            make_owner(root, "owner_user");
            make_non_person_with_vp(root, "coffee_shop", "Place", "120000_300");

            let initial_labels = serde_json::json!({
                "labels": [
                    {"sentence_id": 1, "speaker": "coffee_shop", "method": "acoustic", "confidence": "high"}
                ]
            });
            make_segment_with_labels(root, "20260101", "120000_300", initial_labels);

            let seg_dir = root.join("chronicle/20260101/120000_300");
            let label_p = labels_path(&seg_dir);
            let corr_p = corrections_path(&seg_dir);

            let current_label_sha = compute_file_sha256(&label_p).unwrap();
            let current_corr_sha = compute_file_sha256(&corr_p).unwrap();

            let op_id = "op_resume_not_landed";
            let att_id = "att_2";
            append_repair_event(
                root,
                &RepairEvent::Accepted {
                    schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                    operation_id: op_id.to_owned(),
                    timestamp: Utc::now().to_rfc3339(),
                },
            )
            .unwrap();

            append_repair_event(
                root,
                &RepairEvent::WriteIntent {
                    schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                    operation_id: op_id.to_owned(),
                    attempt_id: att_id.to_owned(),
                    intent_id: "intent_not_landed".to_owned(),
                    supersedes_intent_id: None,
                    day: "20260101".to_owned(),
                    stream_layout: SegmentLayout::Direct,
                    stream: "_default".to_owned(),
                    segment_name: "120000_300".to_owned(),
                    expected_current_label_sha256: current_label_sha.clone(),
                    expected_corrections_sha256: current_corr_sha.clone(),
                    intended_payload_sha256: "will_be_recalculated".to_owned(),
                    intended_payload: serde_json::json!({}),
                    timestamp: Utc::now().to_rfc3339(),
                },
            )
            .unwrap();

            let resume_res = resume_repair(root, op_id, 1000).unwrap();
            assert_eq!(resume_res["status"], "completed");

            let ledger = crate::repair_operations::load_repair_ledger(root).unwrap();
            assert!(ledger.iter().any(|e| matches!(e, RepairEvent::Checkpoint { .. })));
        }

        // Case 3: Supersede - re-resolve output differs from intent A, resumes with superseding intent
        {
            let temp = tempdir().unwrap();
            let root = temp.path();

            make_owner(root, "owner_user");
            make_non_person_with_vp(root, "coffee_shop", "Place", "120000_300");

            let initial_labels = serde_json::json!({
                "labels": [
                    {"sentence_id": 1, "speaker": "coffee_shop", "method": "acoustic", "confidence": "high"}
                ]
            });
            make_segment_with_labels(root, "20260101", "120000_300", initial_labels);

            let seg_dir = root.join("chronicle/20260101/120000_300");
            let label_p = labels_path(&seg_dir);
            let corr_p = corrections_path(&seg_dir);

            let current_label_sha = compute_file_sha256(&label_p).unwrap();
            let current_corr_sha = compute_file_sha256(&corr_p).unwrap();

            let op_id = "op_resume_supersede";
            let att_id = "att_3";
            append_repair_event(
                root,
                &RepairEvent::Accepted {
                    schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                    operation_id: op_id.to_owned(),
                    timestamp: Utc::now().to_rfc3339(),
                },
            )
            .unwrap();

            append_repair_event(
                root,
                &RepairEvent::WriteIntent {
                    schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                    operation_id: op_id.to_owned(),
                    attempt_id: att_id.to_owned(),
                    intent_id: "intent_A".to_owned(),
                    supersedes_intent_id: None,
                    day: "20260101".to_owned(),
                    stream_layout: SegmentLayout::Direct,
                    stream: "_default".to_owned(),
                    segment_name: "120000_300".to_owned(),
                    expected_current_label_sha256: current_label_sha.clone(),
                    expected_corrections_sha256: current_corr_sha.clone(),
                    intended_payload_sha256: "stale_hash_that_differs".to_owned(),
                    intended_payload: serde_json::json!({}),
                    timestamp: Utc::now().to_rfc3339(),
                },
            )
            .unwrap();

            let resume_res = resume_repair(root, op_id, 1000).unwrap();
            assert_eq!(resume_res["status"], "completed");

            let ledger = crate::repair_operations::load_repair_ledger(root).unwrap();
            assert!(ledger.iter().any(|e| match e {
                RepairEvent::WriteIntent { supersedes_intent_id, .. } => {
                    supersedes_intent_id.as_deref() == Some("intent_A")
                }
                _ => false,
            }));
        }

        // Case 4: Conflict if disk label has a third hash
        {
            let temp = tempdir().unwrap();
            let root = temp.path();

            make_owner(root, "owner_user");
            make_non_person_with_vp(root, "coffee_shop", "Place", "120000_300");

            let initial_labels = serde_json::json!({
                "labels": [
                    {"sentence_id": 1, "speaker": "coffee_shop", "method": "acoustic", "confidence": "high"}
                ]
            });
            make_segment_with_labels(root, "20260101", "120000_300", initial_labels);

            let op_id = "op_resume_conflict";
            let att_id = "att_4";
            append_repair_event(
                root,
                &RepairEvent::Accepted {
                    schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                    operation_id: op_id.to_owned(),
                    timestamp: Utc::now().to_rfc3339(),
                },
            )
            .unwrap();

            append_repair_event(
                root,
                &RepairEvent::WriteIntent {
                    schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                    operation_id: op_id.to_owned(),
                    attempt_id: att_id.to_owned(),
                    intent_id: "intent_1".to_owned(),
                    supersedes_intent_id: None,
                    day: "20260101".to_owned(),
                    stream_layout: SegmentLayout::Direct,
                    stream: "_default".to_owned(),
                    segment_name: "120000_300".to_owned(),
                    expected_current_label_sha256: "0".repeat(64),
                    expected_corrections_sha256: "0".repeat(64),
                    intended_payload_sha256: "1".repeat(64),
                    intended_payload: serde_json::json!({}),
                    timestamp: Utc::now().to_rfc3339(),
                },
            )
            .unwrap();

            let resume_res = resume_repair(root, op_id, 1000);
            assert!(resume_res.is_err());

            let ledger = crate::repair_operations::load_repair_ledger(root).unwrap();
            assert!(ledger.iter().any(|e| match e {
                RepairEvent::AttemptFailed { stage, .. } => stage == "resume_matrix",
                _ => false,
            }));
        }
    }

    #[test]
    fn test_prepare_after_cleanup_failure_stage() {
        let temp = tempdir().unwrap();
        let root = temp.path();

        make_owner(root, "owner_user");
        make_non_person_with_vp(root, "coffee_shop", "Place", "120000_300");

        let initial_labels = serde_json::json!({
            "labels": [
                {"sentence_id": 1, "speaker": "coffee_shop", "method": "acoustic", "confidence": "high"}
            ]
        });
        make_segment_with_labels(root, "20260101", "120000_300", initial_labels);

        // Pre-create an accepted event and corrupted entity directory before resume to trigger failure before Prepared
        let op_id = "op_pre_prepare_fail";
        append_repair_event(
            root,
            &RepairEvent::Accepted {
                schema_version: REPAIR_OPERATION_SCHEMA_VERSION,
                operation_id: op_id.to_owned(),
                timestamp: Utc::now().to_rfc3339(),
            },
        )
        .unwrap();

        // Corrupt coffee_shop directory so inventory survey under trust detects gap
        fs::write(root.join("entities/coffee_shop/entity.json"), b"corrupt {").unwrap();

        let resume_res = resume_repair(root, op_id, 1000);
        assert!(resume_res.is_err());

        let ledger = crate::repair_operations::load_repair_ledger(root).unwrap();
        assert!(ledger.iter().any(|e| match e {
            RepairEvent::AttemptFailed { stage, .. } => stage == "re_inventory",
            _ => false,
        }));
        // Verify NO Prepared event was appended
        assert!(!ledger.iter().any(|e| matches!(e, RepairEvent::Prepared { .. })));
    }
}
