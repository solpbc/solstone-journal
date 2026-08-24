// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use solstone_core_speaker_resolve::OWNER_IDENTITY_INVALID_REASON;
use solstone_core_speaker_resolve::identify_operations::{
    FORWARD_PHASE_ORDER, ForwardPhase, IDENTIFY_OPERATION_SCHEMA_VERSION, IdentifyOperationError,
    MemberProvenance, OperationState, TerminalStatus, UNDO_PHASE_ORDER, UndoPhase, append_event,
    expected_restored_correction_artifact_signatures, fold_operation,
    identify_correction_artifact_signature, is_fully_restored_identify_operation, load_operations,
    operation_id_for_request, request_fingerprint, validate_row,
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-identify-operations-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn ledger(&self) -> PathBuf {
        self.0.join("speakers/identify-operations.jsonl")
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn base(event_id: &str, kind: &str) -> serde_json::Map<String, Value> {
    let mut value = serde_json::Map::new();
    value.insert("schema_version".into(), json!(1));
    value.insert("event_id".into(), json!(event_id));
    value.insert("operation_id".into(), json!("idop_test"));
    value.insert("request_id".into(), json!("request"));
    value.insert("event_kind".into(), json!(kind));
    value.insert("ts".into(), json!("2026-08-08T00:00:00Z"));
    value.insert("caller".into(), json!("test"));
    value.insert("actor".into(), Value::Null);
    value
}

fn prepared(event_id: &str) -> Value {
    let mut value = base(event_id, "prepared");
    value.insert("request_fingerprint".into(), json!("a".repeat(64)));
    value.insert("prepared_plan".into(), json!({
        "plan_schema_version": 1,
        "operation_id": "idop_test",
        "request_id": "request",
        "planned_at": "2026-08-08T00:00:00Z",
        "request": {"cluster_id": 1, "name": null, "entity_id": "alice", "resolve_only": false, "create_new": false, "entity_type": "Person", "reviewed_near_match_entity_ids": []},
        "cluster": {"member_count": 0, "members": []},
        "target": {"entity_id": "alice", "entity_name": "Alice", "will_create": false},
        "entity_identity": {}, "direct_voiceprints": {}, "segments": [], "retro_confirm": {}, "sentinel": {}, "keep_separate_assertions": []
    }));
    Value::Object(value)
}

fn committed(event_id: &str) -> Value {
    let mut value = base(event_id, "committed");
    value.insert("result".into(), json!({"status": "identified"}));
    Value::Object(value)
}
fn repair(event_id: &str, kind: &str) -> Value {
    let mut value = base(event_id, kind);
    value.insert(
        "phase".into(),
        json!(if kind == "undo_repair_required" {
            "voiceprints"
        } else {
            "labels"
        }),
    );
    value.insert("repair_code".into(), json!("concurrent_change"));
    value.insert("repair_categories".into(), json!({}));
    value.insert(
        if kind == "undo_repair_required" {
            "undo_report"
        } else {
            "partial_report"
        }
        .into(),
        json!({}),
    );
    Value::Object(value)
}
fn identity_repair(event_id: &str, phase: ForwardPhase, pending_phases: &[&str]) -> Value {
    let mut value = base(event_id, "repair_required");
    value.insert("phase".into(), json!(phase.as_str()));
    value.insert("repair_code".into(), json!(OWNER_IDENTITY_INVALID_REASON));
    value.insert("repair_categories".into(), json!({"owner_identity": 1}));
    value.insert(
        "partial_report".into(),
        json!({"pending_phases": pending_phases}),
    );
    Value::Object(value)
}
fn resumed(event_id: &str, repair_event_id: &str, phase: ForwardPhase) -> Value {
    let mut value = base(event_id, "repair_resumed");
    value.insert(
        "schema_version".into(),
        json!(IDENTIFY_OPERATION_SCHEMA_VERSION),
    );
    value.insert("repair_event_id".into(), json!(repair_event_id));
    value.insert("phase".into(), json!(phase.as_str()));
    Value::Object(value)
}
fn checkpoint(event_id: &str, phase: ForwardPhase) -> Value {
    let details = match phase {
        ForwardPhase::Entity => {
            json!({"entity_id":"alice","entity_created":false,"identity_after_hash":"hash","history_event_refs":[]})
        }
        ForwardPhase::KeepSeparate => {
            json!({"pair_keys":[],"recorded_count":0,"already_present_count":0})
        }
        ForwardPhase::DirectVoiceprints => {
            json!({"saved_keys":[],"saved_count":0,"skipped_existing_count":0})
        }
        ForwardPhase::Corrections => {
            json!({"appended_keys":[],"appended_count":0,"skipped_existing_count":0,"segment_count":0})
        }
        ForwardPhase::Labels => {
            json!({"patched_sentence_keys":[],"inserted_sentence_keys":[],"patched_count":0,"inserted_count":0,"skipped_already_intended_count":0,"segment_count":0})
        }
        ForwardPhase::RetroTracker => {
            json!({"matched":false,"candidate_id":null,"saved_keys":[],"voiceprints_saved_count":0,"voiceprints_skipped_existing_count":0,"tracker_updated":false})
        }
        ForwardPhase::Sentinel => json!({"cluster_key":"cluster","written":true}),
    };
    let mut value = base(event_id, "checkpoint");
    value.insert("phase".into(), json!(phase.as_str()));
    value.insert(
        "checkpoint".into(),
        Value::Object(
            json!({"phase_status":"complete","completed_at":"2026-08-08T00:00:00Z","counts":{},"skipped_reasons":{}})
                .as_object()
                .expect("object")
                .clone()
                .into_iter()
                .chain(details.as_object().expect("object").clone())
                .collect(),
        ),
    );
    Value::Object(value)
}
fn undo_prepared(event_id: &str) -> Value {
    let mut value = base(event_id, "undo_prepared");
    value.insert("undo_started_at".into(), json!("2026-08-08T01:00:00Z"));
    Value::Object(value)
}
fn undo_committed(event_id: &str) -> Value {
    let mut value = base(event_id, "undo_committed");
    value.insert("undo_report".into(), json!({}));
    Value::Object(value)
}

fn write_rows(path: &Path, rows: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        rows.iter()
            .map(|row| serde_json::to_string(row).unwrap() + "\n")
            .collect::<String>(),
    )
    .unwrap();
}

fn fully_restored_operation_state() -> OperationState {
    let correction_key = json!({
        "day": "20260101",
        "stream": "mic",
        "segment_key": "seg-a",
        "sentence_id": 7,
    });
    let direct_voiceprint_key = json!({
        "day": "20260101",
        "segment_key": "seg-a",
        "source": "audio",
        "sentence_id": 1,
    });
    let retro_voiceprint_key = json!({
        "day": "20260101",
        "segment_key": "seg-a",
        "source": "audio",
        "sentence_id": 2,
    });
    let pair_key = json!({"left": "ent-alice", "right": "ent-bob"});
    let mut phase_checkpoints = BTreeMap::new();
    phase_checkpoints.insert(
        ForwardPhase::Entity,
        json!({
            "phase_status": "complete", "completed_at": "2026-08-08T00:01:00Z",
            "counts": {}, "skipped_reasons": {}, "entity_id": "ent-alice",
            "entity_created": true, "identity_after_hash": "identity-hash",
            "history_event_refs": [],
        }),
    );
    phase_checkpoints.insert(
        ForwardPhase::KeepSeparate,
        json!({
            "phase_status": "complete", "completed_at": "2026-08-08T00:02:00Z",
            "counts": {}, "skipped_reasons": {}, "pair_keys": [pair_key],
            "recorded_count": 1, "already_present_count": 0,
        }),
    );
    phase_checkpoints.insert(
        ForwardPhase::DirectVoiceprints,
        json!({
            "phase_status": "complete", "completed_at": "2026-08-08T00:03:00Z",
            "counts": {}, "skipped_reasons": {}, "saved_keys": [direct_voiceprint_key],
            "saved_count": 1, "skipped_existing_count": 0,
        }),
    );
    phase_checkpoints.insert(
        ForwardPhase::Corrections,
        json!({
            "phase_status": "complete", "completed_at": "2026-08-08T00:04:00Z",
            "counts": {}, "skipped_reasons": {}, "appended_keys": [correction_key],
            "appended_count": 1, "skipped_existing_count": 0, "segment_count": 1,
        }),
    );
    phase_checkpoints.insert(
        ForwardPhase::Labels,
        json!({
            "phase_status": "complete", "completed_at": "2026-08-08T00:05:00Z",
            "counts": {}, "skipped_reasons": {},
            "patched_sentence_keys": [json!({"sentence_id": 1})],
            "inserted_sentence_keys": [json!({"sentence_id": 2})],
            "patched_count": 1, "inserted_count": 1,
            "skipped_already_intended_count": 0, "segment_count": 1,
        }),
    );
    phase_checkpoints.insert(
        ForwardPhase::RetroTracker,
        json!({
            "phase_status": "complete", "completed_at": "2026-08-08T00:06:00Z",
            "counts": {}, "skipped_reasons": {}, "matched": true, "candidate_id": 42,
            "saved_keys": [retro_voiceprint_key], "voiceprints_saved_count": 1,
            "voiceprints_skipped_existing_count": 0, "tracker_updated": true,
        }),
    );
    phase_checkpoints.insert(
        ForwardPhase::Sentinel,
        json!({
            "phase_status": "complete", "completed_at": "2026-08-08T00:07:00Z",
            "counts": {}, "skipped_reasons": {}, "cluster_key": "cluster-a", "written": true,
        }),
    );

    let categories = json!({
        "labels": {
            "restored_count": 2, "skipped_count": 0, "skipped_reasons": {},
            "removed_inserted_count": 1, "patched_existing_count": 1,
        },
        "corrections": {
            "restored_count": 1, "skipped_count": 0, "skipped_reasons": {},
            "appended_count": 1, "already_present_count": 0,
        },
        "voiceprints": {
            "restored_count": 2, "skipped_count": 0, "skipped_reasons": {},
            "removed_count": 2, "missing_count": 0, "metadata_mismatch_count": 0,
        },
        "tracker": {
            "restored_count": 1, "skipped_count": 0, "skipped_reasons": {},
            "restored_candidate_count": 1,
        },
        "sentinel": {
            "restored_count": 1, "skipped_count": 0, "skipped_reasons": {},
            "removed_count": 1, "restored_prior_count": 0,
        },
        "entity": {
            "restored_count": 1, "skipped_count": 0, "skipped_reasons": {},
            "deleted": true, "blocked_categories": [], "keep_separate_sources_removed_count": 1,
        },
    });
    let mut undo_phase_checkpoints = BTreeMap::new();
    for phase in UNDO_PHASE_ORDER {
        undo_phase_checkpoints.insert(
            phase,
            json!({phase.as_str(): categories[phase.as_str()].clone()}),
        );
    }

    OperationState {
        operation_id: "idop_fixture".into(),
        request_id: "request-fixture".into(),
        request_fingerprint: "f".repeat(64),
        cluster_member_set: BTreeSet::from([MemberProvenance {
            day: "20260101".into(),
            stream: "mic".into(),
            segment_key: "seg-a".into(),
            source: "audio".into(),
            sentence_id: 7,
        }]),
        target_entity_id: Some("ent-alice".into()),
        target_entity_name: Some("Alice".into()),
        will_create: true,
        entity_type: Some("Person".into()),
        reviewed_near_match_entity_ids: vec!["ent-bob".into()],
        completed_phases: FORWARD_PHASE_ORDER.to_vec(),
        pending_phases: Vec::new(),
        terminal_status: TerminalStatus::Undone,
        result: Some(json!({"status": "identified"})),
        undo_report: Some(json!({
            "status": "undone", "operation_id": "idop_fixture", "undo_report": categories,
        })),
        undo_started_at: Some("2026-08-08T01:00:00Z".into()),
        undo_committed_count: 1,
        phase_checkpoints,
        prepared_plan: json!({
            "segments": [{
                "day": "20260101", "stream": "mic", "segment_key": "seg-a",
                "corrections": {"rows_to_append": [{
                    "sentence_id": 7, "original_speaker": "Unknown",
                    "corrected_speaker": "ent-alice", "original_method": "cluster",
                    "timestamp": "2026-08-08T00:04:00Z",
                }]},
                "labels": [{
                    "sentence_id": 7, "prior_state": "present",
                    "prior_label": {"speaker": "Unknown"},
                }],
            }],
        }),
        repair_required: None,
        undo_repair_required: None,
        undo_phase_checkpoints,
    }
}

fn replace_undo_category(state: &mut OperationState, phase: UndoPhase, category: Value) {
    state.undo_report.as_mut().unwrap()["undo_report"][phase.as_str()] = category.clone();
    state
        .undo_phase_checkpoints
        .insert(phase, json!({phase.as_str(): category}));
}

#[test]
fn ac7_malformed_row_fails_loudly_without_a_partial_append() {
    let temporary = TempDir::new();
    let path = temporary.ledger();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"{not json}\n").unwrap();
    let before = fs::read(&path).unwrap();
    assert!(matches!(
        load_operations(&path),
        Err(IdentifyOperationError::MalformedJson { line: 1, .. })
    ));
    let mut invalid_event = validate_row(&prepared("invalid-append")).unwrap();
    invalid_event.schema_version = 3;
    assert!(matches!(
        append_event(&path, &invalid_event),
        Err(IdentifyOperationError::InvalidSchemaVersion)
    ));
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn repair_resume_reads_a_v1_prefix_without_rewriting_it() {
    let temporary = TempDir::new();
    let path = temporary.ledger();
    let prefix = vec![
        prepared("prepared"),
        identity_repair(
            "idop_test:repair_required:direct_voiceprints",
            ForwardPhase::DirectVoiceprints,
            &["direct_voiceprints"],
        ),
    ];
    write_rows(&path, &prefix);
    let before = fs::read(&path).unwrap();

    let resume = validate_row(&resumed(
        "idop_test:repair_required:direct_voiceprints:resumed",
        "idop_test:repair_required:direct_voiceprints",
        ForwardPhase::DirectVoiceprints,
    ))
    .unwrap();
    append_event(&path, &resume).unwrap();

    let after = fs::read(&path).unwrap();
    assert_eq!(&after[..before.len()], before.as_slice());
    let state = fold_operation(&load_operations(&path).unwrap(), "idop_test")
        .unwrap()
        .unwrap();
    assert_eq!(state.terminal_status, TerminalStatus::InProgress);
    assert!(state.repair_required.is_none());
}

#[test]
fn repair_resume_requires_v2_and_the_latest_outstanding_identity_repair() {
    let mut v1_resume = resumed("repair:resumed", "repair", ForwardPhase::DirectVoiceprints);
    v1_resume["schema_version"] = json!(1);
    assert!(matches!(
        validate_row(&v1_resume),
        Err(IdentifyOperationError::RepairResumeRequiresSchemaVersion2)
    ));

    let cases = [
        (
            "non_latest",
            vec![
                prepared("prepared"),
                identity_repair("repair-a", ForwardPhase::DirectVoiceprints, &[]),
                identity_repair("repair-b", ForwardPhase::Labels, &[]),
                resumed(
                    "repair-a:resumed",
                    "repair-a",
                    ForwardPhase::DirectVoiceprints,
                ),
            ],
        ),
        (
            "wrong_phase",
            vec![
                prepared("prepared"),
                identity_repair("repair", ForwardPhase::DirectVoiceprints, &[]),
                resumed("repair:resumed", "repair", ForwardPhase::Labels),
            ],
        ),
        (
            "not_outstanding",
            vec![
                prepared("prepared"),
                identity_repair("repair", ForwardPhase::DirectVoiceprints, &[]),
                committed("committed"),
                resumed("repair:resumed", "repair", ForwardPhase::DirectVoiceprints),
            ],
        ),
    ];
    for (name, rows) in cases {
        let temporary = TempDir::new();
        write_rows(&temporary.ledger(), &rows);
        assert!(
            matches!(
                fold_operation(&load_operations(&temporary.ledger()).unwrap(), "idop_test"),
                Err(IdentifyOperationError::InvalidRepairResume { .. })
            ),
            "{name}"
        );
    }
}

#[test]
fn repair_resume_cannot_reopen_a_committed_or_undo_lifecycle() {
    let lifecycle_events = [
        ("committed", vec![committed("committed")]),
        (
            "undoing",
            vec![committed("committed"), undo_prepared("undo_prepared")],
        ),
        (
            "undone",
            vec![
                committed("committed"),
                undo_prepared("undo_prepared"),
                undo_committed("undo_committed"),
            ],
        ),
        (
            "undo_repair_required",
            vec![
                committed("committed"),
                undo_prepared("undo_prepared"),
                repair("undo_repair", "undo_repair_required"),
            ],
        ),
    ];
    for (name, mut suffix) in lifecycle_events {
        let temporary = TempDir::new();
        let mut rows = vec![
            prepared("prepared"),
            identity_repair("repair", ForwardPhase::DirectVoiceprints, &[]),
        ];
        rows.append(&mut suffix);
        rows.push(resumed(
            "repair:resumed",
            "repair",
            ForwardPhase::DirectVoiceprints,
        ));
        write_rows(&temporary.ledger(), &rows);
        assert!(
            matches!(
                fold_operation(&load_operations(&temporary.ledger()).unwrap(), "idop_test"),
                Err(IdentifyOperationError::InvalidRepairResume { .. })
            ),
            "{name}"
        );
    }
}

#[test]
fn ordered_repair_resume_state_machine_uses_checkpoints_for_pending_phases() {
    let temporary = TempDir::new();
    let path = temporary.ledger();
    let mut rows = vec![
        prepared("prepared"),
        checkpoint("entity", ForwardPhase::Entity),
        identity_repair("repair", ForwardPhase::DirectVoiceprints, &["sentinel"]),
        resumed("repair:resumed", "repair", ForwardPhase::DirectVoiceprints),
    ];
    write_rows(&path, &rows);
    let resumed_state = fold_operation(&load_operations(&path).unwrap(), "idop_test")
        .unwrap()
        .unwrap();
    assert_eq!(resumed_state.terminal_status, TerminalStatus::InProgress);
    assert_eq!(
        resumed_state.pending_phases,
        FORWARD_PHASE_ORDER[1..]
            .iter()
            .map(|phase| phase.as_str().to_owned())
            .collect::<Vec<_>>()
    );

    rows.extend([
        checkpoint("keep_separate", ForwardPhase::KeepSeparate),
        checkpoint("direct_voiceprints", ForwardPhase::DirectVoiceprints),
        checkpoint("corrections", ForwardPhase::Corrections),
        identity_repair("repair:retry:1", ForwardPhase::Labels, &["labels"]),
        resumed(
            "repair:retry:1:resumed",
            "repair:retry:1",
            ForwardPhase::Labels,
        ),
        checkpoint("labels", ForwardPhase::Labels),
        checkpoint("retro_tracker", ForwardPhase::RetroTracker),
        checkpoint("sentinel", ForwardPhase::Sentinel),
        committed("committed"),
    ]);
    write_rows(&path, &rows);
    let committed_state = fold_operation(&load_operations(&path).unwrap(), "idop_test")
        .unwrap()
        .unwrap();
    assert_eq!(committed_state.terminal_status, TerminalStatus::Committed);
    assert!(committed_state.repair_required.is_none());
    assert_eq!(
        rows.iter()
            .filter(|row| row["event_kind"] == "repair_required")
            .map(|row| row["event_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["repair", "repair:retry:1"]
    );
}

#[test]
fn resumed_repair_is_not_projected_after_commit_and_undo() {
    let temporary = TempDir::new();
    let path = temporary.ledger();
    let mut rows = vec![
        prepared("prepared"),
        identity_repair("repair", ForwardPhase::DirectVoiceprints, &[]),
        resumed("repair:resumed", "repair", ForwardPhase::DirectVoiceprints),
    ];
    rows.extend(
        FORWARD_PHASE_ORDER
            .iter()
            .map(|phase| checkpoint(phase.as_str(), *phase)),
    );
    rows.extend([
        committed("committed"),
        undo_prepared("undo_prepared"),
        undo_committed("undone"),
    ]);
    write_rows(&path, &rows);
    let state = fold_operation(&load_operations(&path).unwrap(), "idop_test")
        .unwrap()
        .unwrap();
    assert_eq!(state.terminal_status, TerminalStatus::Undone);
    assert!(state.repair_required.is_none());

    let mut restored = fully_restored_operation_state();
    restored.repair_required = state.repair_required;
    assert!(is_fully_restored_identify_operation(&restored));
}

#[test]
fn ac10_append_only_no_rewrite() {
    let temporary = TempDir::new();
    let path = temporary.ledger();
    append_event(&path, &validate_row(&prepared("prepared")).unwrap()).unwrap();
    let before = fs::read(&path).unwrap();
    let mut corruptible_model = load_operations(&path).unwrap();
    corruptible_model.clear();
    append_event(&path, &validate_row(&committed("committed")).unwrap()).unwrap();
    let after = fs::read(&path).unwrap();
    assert!(after.starts_with(&before));
    assert_eq!(after[..before.len()], before);
}

#[test]
fn byte_identical_duplicate_event_ids_dedupe_cleanly() {
    let temporary = TempDir::new();
    let path = temporary.ledger();
    write_rows(&path, &[prepared("prepared"), prepared("prepared")]);
    let rows = load_operations(&path).unwrap();
    let state = fold_operation(&rows, "idop_test").unwrap().unwrap();
    assert_eq!(state.terminal_status, TerminalStatus::InProgress);
}

#[test]
fn conflicting_duplicate_event_ids_fail() {
    let temporary = TempDir::new();
    let path = temporary.ledger();
    let first = prepared("prepared");
    let mut second = prepared("prepared");
    second["caller"] = json!("different");
    write_rows(&path, &[first, second]);
    let rows = load_operations(&path).unwrap();
    assert!(matches!(
        fold_operation(&rows, "idop_test"),
        Err(IdentifyOperationError::ConflictingDuplicateEventId { .. })
    ));
}

#[test]
fn terminal_precedence_is_undo_repair_then_undone_then_undoing_then_repair_then_committed() {
    let temporary = TempDir::new();
    let path = temporary.ledger();
    let mut events = vec![prepared("prepared")];
    for (event, expected) in [
        (committed("committed"), TerminalStatus::Committed),
        (
            repair("repair", "repair_required"),
            TerminalStatus::RepairRequired,
        ),
        (undo_prepared("undo_prepared"), TerminalStatus::Undoing),
        (undo_committed("undo_committed"), TerminalStatus::Undone),
        (
            repair("undo_repair", "undo_repair_required"),
            TerminalStatus::UndoRepairRequired,
        ),
    ] {
        events.push(event);
        write_rows(&path, &events);
        let rows = load_operations(&path).unwrap();
        assert_eq!(
            fold_operation(&rows, "idop_test")
                .unwrap()
                .unwrap()
                .terminal_status,
            expected
        );
    }
}

#[test]
fn phase_orders_match_the_durable_python_ledger_contract() {
    assert_eq!(
        FORWARD_PHASE_ORDER.map(|phase| phase.as_str()),
        [
            "entity",
            "keep_separate",
            "direct_voiceprints",
            "corrections",
            "labels",
            "retro_tracker",
            "sentinel"
        ],
    );
    assert_eq!(
        UNDO_PHASE_ORDER.map(|phase| phase.as_str()),
        [
            "labels",
            "corrections",
            "voiceprints",
            "tracker",
            "sentinel",
            "entity"
        ],
    );
}

#[test]
fn operation_id_matches_python_sha256_truncation() {
    assert_eq!(
        operation_id_for_request("request-fixture-1").unwrap(),
        "idop_cae303413dae3848a724b3a6"
    );
}

#[test]
fn request_fingerprint_matches_python_canonical_json_hash() {
    let members = [
        MemberProvenance {
            day: "20260101".into(),
            stream: "mic".into(),
            segment_key: "seg-b".into(),
            source: "audio".into(),
            sentence_id: 2,
        },
        MemberProvenance {
            day: "20260101".into(),
            stream: "mic".into(),
            segment_key: "seg-a".into(),
            source: "audio".into(),
            sentence_id: 1,
        },
    ];
    assert_eq!(
        request_fingerprint(
            &members,
            "ent-alice",
            false,
            "Person",
            &["ent-bob".into(), "ent-carol".into()],
        ),
        "6ba95061df6ae83b04036eef7ce23dc7ab6de4b1bf272f87c4278429954e7628"
    );
}

#[test]
fn restoration_proof_accepts_a_complete_matching_ledger() {
    assert!(is_fully_restored_identify_operation(
        &fully_restored_operation_state()
    ));
}

#[test]
fn restoration_proof_rejects_each_independent_incomplete_or_inconsistent_state() {
    let state = fully_restored_operation_state();

    let mut undoing = state.clone();
    undoing.terminal_status = TerminalStatus::Undoing;
    assert!(!is_fully_restored_identify_operation(&undoing));

    let mut repair_required = state.clone();
    repair_required.repair_required = Some(validate_row(&prepared("repair-marker")).unwrap());
    assert!(!is_fully_restored_identify_operation(&repair_required));

    let mut duplicate_undo_commit = state.clone();
    duplicate_undo_commit.undo_committed_count = 2;
    assert!(!is_fully_restored_identify_operation(
        &duplicate_undo_commit
    ));

    let mut missing_checkpoint = state.clone();
    missing_checkpoint
        .undo_phase_checkpoints
        .remove(&UndoPhase::Labels);
    assert!(!is_fully_restored_identify_operation(&missing_checkpoint));

    let mut mismatched_count = state.clone();
    let mut labels =
        mismatched_count.undo_report.as_ref().unwrap()["undo_report"]["labels"].clone();
    labels["restored_count"] = json!(99);
    replace_undo_category(&mut mismatched_count, UndoPhase::Labels, labels);
    assert!(!is_fully_restored_identify_operation(&mismatched_count));

    let mut skipped_work = state;
    let mut corrections =
        skipped_work.undo_report.as_ref().unwrap()["undo_report"]["corrections"].clone();
    corrections["skipped_count"] = json!(1);
    replace_undo_category(&mut skipped_work, UndoPhase::Corrections, corrections);
    assert!(!is_fully_restored_identify_operation(&skipped_work));
}

#[test]
fn correction_artifact_signatures_match_the_restored_forward_and_undo_pair() {
    let row = json!({
        "sentence_id": 7,
        "correction_kind": "identify",
        "operation_id": "idop_fixture",
        "undo_of_operation_id": null,
        "original_speaker": "Unknown",
        "corrected_speaker": "ent-alice",
        "original_method": "cluster",
        "timestamp": "2026-08-08T00:04:00Z",
    });
    assert_eq!(
        identify_correction_artifact_signature(&row, "20260101", "mic", "seg-a"),
        vec![
            json!("20260101"),
            json!("mic"),
            json!("seg-a"),
            json!(7),
            json!("identify"),
            json!("idop_fixture"),
            Value::Null,
            json!("Unknown"),
            json!("ent-alice"),
            json!("cluster"),
            json!("2026-08-08T00:04:00Z"),
        ],
    );

    assert_eq!(
        expected_restored_correction_artifact_signatures(
            &fully_restored_operation_state(),
            "20260101",
            "mic",
            "seg-a",
        ),
        vec![
            vec![
                json!("20260101"),
                json!("mic"),
                json!("seg-a"),
                json!(7),
                json!("identify"),
                json!("idop_fixture"),
                Value::Null,
                json!("Unknown"),
                json!("ent-alice"),
                json!("cluster"),
                json!("2026-08-08T00:04:00Z"),
            ],
            vec![
                json!("20260101"),
                json!("mic"),
                json!("seg-a"),
                json!(7),
                json!("identify_undo"),
                json!("idop_fixture"),
                json!("idop_fixture"),
                json!("ent-alice"),
                json!("Unknown"),
                json!("user_identified"),
                json!("2026-08-08T01:00:00Z"),
            ],
        ],
    );
}
