// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use serde_json::{Value, json};
use solstone_core_convey_http::identity::AccessBasis;
use tempfile::TempDir;
use tower::ServiceExt;

use crate::dispatch::{DispatchError, DispatchPrincipal, codec, dispatch_authenticated_tool_call};
use crate::jsonrpc::{McpMethod, ToolName, classify_method, parse_request};
use crate::owner_web::owner_routes;
use crate::permissions::{
    ConnectionReadSnapshot, PermissionDecision, PermissionStore, ReadPermission,
};
use crate::references::{ActivityReference, ReferenceTarget};
use crate::registry::advertised_tools_list;
use crate::suggestions::store::{read_record, update_connection_enabled};
use crate::suggestions::{
    set_force_suggestion_ask_failure, set_force_suggestion_receipt_failure,
    set_test_suggestions_ready,
};
use solstone_core_indexer_query::ConnectionScope;

fn init_test_journal() -> TempDir {
    let journal = tempfile::Builder::new()
        .prefix("solstone-suggestions-test-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    solstone_core_facets::create_facet(journal.path(), "work", "Work", "", "", "", None).unwrap();
    PermissionStore::open(journal.path())
        .set_permission("bearer:test-agent", ReadPermission::default_whole_journal())
        .unwrap();
    journal
}

fn setup_activity(journal: &Path, facet: &str, day: &str, act_id: &str, details: &str) {
    let act_dir = journal.join("facets").join(facet).join("activities");
    fs::create_dir_all(&act_dir).unwrap();
    let record = json!({
        "id": act_id,
        "schema": 1,
        "stream": "default",
        "details": details,
        "playback": false,
        "segments": ["090000_300"],
        "title": "Synthetic meeting",
        "activity": "meeting",
        "source": "cogitate",
        "start": "09:00:00",
        "end": "09:05:00",
        "created_at": format!("{day}T09:00:00Z"),
    });
    fs::write(
        act_dir.join(format!("{day}.jsonl")),
        format!("{}\n", serde_json::to_string(&record).unwrap()),
    )
    .unwrap();
}

fn setup_transcript_segment(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    lines: &[(&str, &str)],
) {
    let seg_dir = solstone_core_segment::SegmentDir::resolve(journal, day, segment, stream)
        .unwrap()
        .path()
        .to_path_buf();
    fs::create_dir_all(&seg_dir).unwrap();
    let mut content = String::new();
    for (start, text) in lines {
        content.push_str(
            &json!({
                "start": start,
                "text": text,
                "source": "audio",
            })
            .to_string(),
        );
        content.push('\n');
    }
    fs::write(seg_dir.join("audio.jsonl"), content).unwrap();
}

fn setup_entity(journal: &Path, facet: &str, entity_id: &str, name: &str, aka: &[&str]) {
    let ent_dir = journal.join("entities").join(entity_id);
    fs::create_dir_all(&ent_dir).unwrap();
    let entity_record = json!({
        "id": entity_id,
        "name": name,
        "type": "Person",
        "aka": aka,
    });
    fs::write(
        ent_dir.join("entity.json"),
        serde_json::to_string(&entity_record).unwrap(),
    )
    .unwrap();

    let facet_ent_dir = journal
        .join("facets")
        .join(facet)
        .join("entities")
        .join(entity_id);
    fs::create_dir_all(&facet_ent_dir).unwrap();
    let link_record = json!({
        "entity_id": entity_id,
        "description": "",
        "attached_at": "2026-11-14T00:00:00Z",
    });
    fs::write(
        facet_ent_dir.join("entity.json"),
        serde_json::to_string(&link_record).unwrap(),
    )
    .unwrap();
}

async fn owner_call(
    journal: &Path,
    method: &str,
    uri: &str,
    body: Option<Value>,
    is_owner: bool,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    let body_bytes = if let Some(b) = body {
        builder = builder.header("content-type", "application/json");
        Body::from(serde_json::to_vec(&b).unwrap())
    } else {
        Body::empty()
    };
    let mut req = builder.body(body_bytes).unwrap();
    if is_owner {
        req.extensions_mut().insert(AccessBasis::Localhost);
    }
    let res = owner_routes(journal.to_path_buf())
        .oneshot(req)
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let val: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, val)
}

#[tokio::test(flavor = "current_thread")]
async fn line_correction_uses_list_reference_and_owner_authority_and_undo_token() {
    for stream in ["default", solstone_core_journal_io::DEFAULT_STREAM] {
        set_test_suggestions_ready(true);
        let journal = init_test_journal();
        let day = "20261114";
        setup_activity(journal.path(), "work", day, "meeting_1", "");
        setup_entity(journal.path(), "work", "ada", "Ada", &[]);
        let mut activity =
            solstone_core_facets::get_activity_record(journal.path(), "work", day, "meeting_1")
                .unwrap()
                .unwrap();
        activity.insert("stream".to_owned(), json!(stream));
        fs::write(
            journal
                .path()
                .join(format!("facets/work/activities/{day}.jsonl")),
            format!("{}\n", json!(activity)),
        )
        .unwrap();
        setup_transcript_segment(
            journal.path(),
            day,
            stream,
            "090000_300",
            &[("09:00:10", "Good morning team")],
        );
        let segment =
            solstone_core_segment::SegmentDir::resolve(journal.path(), day, "090000_300", stream)
                .unwrap()
                .path()
                .to_path_buf();
        fs::rename(
            segment.join("audio.jsonl"),
            segment.join("system_audio.jsonl"),
        )
        .unwrap();
        fs::write(
            segment.join("unrelated.jsonl"),
            json!({"start":"09:00:10","text":"Good morning team, an unrelated file"}).to_string(),
        )
        .unwrap();
        let principal = DispatchPrincipal {
            connection: "bearer:test-agent",
            agent_identity: "bearer:test-agent",
        };
        let listed = dispatch_authenticated_tool_call(
            journal.path(),
            principal,
            ToolName::ListTranscripts,
            Some(&json!({"day": day})),
            Utc::now(),
        )
        .unwrap();
        let reference = listed.value["transcripts"][0]["reference"]
            .as_str()
            .expect("native list returned a transcript");
        let submitted = dispatch_authenticated_tool_call(journal.path(), principal, ToolName::SuggestCorrection,
        Some(&json!({
            "kind": "who_said_it", "target": {"reference": reference,
                "lines": [{"time": "09:00:10", "starts_with": "Good morning"}, {"time": "09:00:10", "starts_with": "Good"}]},
            "value": {"speaker": "ada@work"}, "basis": "owner_said", "reason": "The owner identified Ada.",
            "operation_id": "line-authority"
        })), Utc::now()).unwrap();
        assert_eq!(submitted.value["status"], "pending");
        let id = submitted.value["suggestion_id"].as_str().unwrap();
        let base = format!("/app/agents/api/suggestions/{id}");
        let (status, accepted) = owner_call(
            journal.path(),
            "POST",
            &format!("{base}/accept"),
            Some(json!({})),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{accepted}");
        assert_eq!(accepted["suggestion"]["status"], "accepted");
        assert_eq!(accepted["suggestion"]["asked"], true);
        assert_eq!(accepted["suggestion"]["change_parts"]["line_count"], 1);
        let segment =
            solstone_core_segment::SegmentDir::resolve(journal.path(), day, "090000_300", stream)
                .unwrap()
                .path()
                .to_path_buf();
        let entries =
            solstone_core_speaker_resolve::owner_line_labels::current_owner_line_entries(&segment)
                .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].speaker.as_deref(), Some("ada"));
        assert_eq!(entries[0].source, "system_audio");
        let saved = read_record(journal.path(), id).unwrap().unwrap();
        assert_eq!(
            saved
                .receipt
                .as_ref()
                .unwrap()
                .line_reverts
                .as_ref()
                .unwrap()[0]["set_at"],
            entries[0].set_at
        );
        let activity =
            solstone_core_facets::get_activity_record(journal.path(), "work", day, "meeting_1")
                .unwrap()
                .unwrap();
        let edit = activity["edits"].as_array().unwrap().last().unwrap();
        assert_eq!(edit["actor"], "owner:lines");
        assert_eq!(
            edit["suggested_by"]["source"],
            accepted["suggestion"]["agent"]["source"]
        );
        let (status, undone) = owner_call(
            journal.path(),
            "POST",
            &format!("{base}/undo"),
            Some(json!({})),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{undone}");
        assert_eq!(undone["suggestion"]["status"], "undone");
        assert_eq!(undone["suggestion"]["asked"], true);
        assert!(
            solstone_core_speaker_resolve::owner_line_labels::current_owner_line_entries(&segment)
                .unwrap()
                .is_empty()
        );
        let activity =
            solstone_core_facets::get_activity_record(journal.path(), "work", day, "meeting_1")
                .unwrap()
                .unwrap();
        assert!(solstone_core_facets::edit_removed(
            activity["edits"][0].as_object().unwrap()
        ));
    }
}

#[test]
fn test_schema_and_dark_default() {
    let journal = init_test_journal();

    // 1. Dark default omits tool from tools/list and direct JSON-RPC call returns -32601
    set_test_suggestions_ready(false);
    let perm_snapshot = PermissionDecision::Snapshot(ConnectionReadSnapshot {
        categories: [
            solstone_core_indexer_query::AdmittedCategory::Facets,
            solstone_core_indexer_query::AdmittedCategory::Entities,
            solstone_core_indexer_query::AdmittedCategory::Transcripts,
            solstone_core_indexer_query::AdmittedCategory::Body,
        ]
        .into_iter()
        .collect::<BTreeSet<_>>(),
        scope: ConnectionScope::WholeJournal,
        generation: 1,
    });
    let tools = advertised_tools_list(journal.path(), &perm_snapshot);
    let has_tool = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["name"] == "suggest_correction");
    assert!(!has_tool);

    let req = parse_request(
        br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"suggest_correction"}}"#,
    )
    .unwrap();
    let classified_err = classify_method(&req).unwrap_err();
    let classified_bytes = classified_err.to_bytes().unwrap();
    let classified_val: Value = serde_json::from_slice(&classified_bytes).unwrap();
    assert_eq!(classified_val["error"]["code"], -32601);

    let principal = DispatchPrincipal {
        connection: "bearer:test-agent",
        agent_identity: "bearer:test-agent",
    };
    assert!(matches!(
        dispatch_authenticated_tool_call(
            journal.path(),
            principal,
            ToolName::SuggestCorrection,
            Some(&json!({})),
            Utc::now()
        ),
        Err(DispatchError::InvalidInput)
    ));
    // 2. Extra field in arguments rejected by schema validation
    set_test_suggestions_ready(true);
    let tools = advertised_tools_list(journal.path(), &perm_snapshot);
    let has_tool = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["name"] == "suggest_correction");
    assert!(has_tool);

    let classified_ok = classify_method(&req);
    assert!(matches!(
        classified_ok,
        Ok(McpMethod::ToolsCall(ToolName::SuggestCorrection))
    ));

    let principal = DispatchPrincipal {
        connection: "bearer:test-agent",
        agent_identity: "bearer:test-agent",
    };
    let args_with_extra = json!({
        "kind": "recording",
        "target": {"reference": "20261114/work/meeting_1"},
        "value": {},
        "basis": "owner_said",
        "reason": "it was a recording",
        "operation_id": "op-extra-field",
        "extra_field": "disallowed"
    });
    let result = dispatch_authenticated_tool_call(
        journal.path(),
        principal,
        ToolName::SuggestCorrection,
        Some(&args_with_extra),
        Utc::now(),
    );
    assert!(matches!(result, Err(DispatchError::InvalidInput)));
}

#[test]
fn test_suggestions_off_and_unknown_target_return_message() {
    let journal = init_test_journal();
    set_test_suggestions_ready(true);

    let principal = DispatchPrincipal {
        connection: "bearer:test-agent",
        agent_identity: "bearer:test-agent",
    };

    // 1. Unknown or hidden name returns status refused with non-empty message, schema, operation_id
    let args_unknown = json!({
        "kind": "what_happened",
        "target": {"reference": "20261114/work/nonexistent_act"},
        "value": {"text": "something happened"},
        "basis": "owner_said",
        "reason": "correction reason",
        "operation_id": "op-unknown-1"
    });
    let out = dispatch_authenticated_tool_call(
        journal.path(),
        principal,
        ToolName::SuggestCorrection,
        Some(&args_unknown),
        Utc::now(),
    )
    .unwrap();
    assert_eq!(out.value["status"], "refused");
    assert_eq!(out.value["schema"], 1);
    assert_eq!(out.value["operation_id"], "op-unknown-1");
    let msg = out.value["message"].as_str().unwrap();
    assert!(!msg.is_empty());
    assert!(out.value.get("next").is_none());

    // 2. Suggestions disabled for connection returns status suggestions_off with message
    update_connection_enabled(journal.path(), "bearer:test-agent", false, None).unwrap();
    let out_off = dispatch_authenticated_tool_call(
        journal.path(),
        principal,
        ToolName::SuggestCorrection,
        Some(&args_unknown),
        Utc::now(),
    )
    .unwrap();
    assert_eq!(out_off.value["status"], "suggestions_off");
    assert_eq!(out_off.value["schema"], 1);
    assert_eq!(out_off.value["operation_id"], "op-unknown-1");
    let msg_off = out_off.value["message"].as_str().unwrap();
    assert!(!msg_off.is_empty());
}

#[test]
fn test_already_true_and_dedup_and_cross_grant() {
    let journal = init_test_journal();
    set_test_suggestions_ready(true);

    setup_entity(journal.path(), "work", "dana", "Dana Test", &["D", "Dan"]);
    let entity_name = crate::names::entity_name("work", "dana").unwrap();

    let principal_a = DispatchPrincipal {
        connection: "bearer:test-agent-a",
        agent_identity: "bearer:test-agent-a",
    };
    PermissionStore::open(journal.path())
        .set_permission(
            "bearer:test-agent-a",
            ReadPermission::default_whole_journal(),
        )
        .unwrap();

    let principal_b = DispatchPrincipal {
        connection: "bearer:test-agent-b",
        agent_identity: "bearer:test-agent-b",
    };
    PermissionStore::open(journal.path())
        .set_permission(
            "bearer:test-agent-b",
            ReadPermission::default_whole_journal(),
        )
        .unwrap();

    // 1. Suggesting an already-true alias returns status already_true with message
    let args_already_true = json!({
        "kind": "name",
        "target": {"reference": entity_name},
        "value": {"text": "Dan", "mode": "also_known_as"},
        "basis": "owner_said",
        "reason": "alias exists",
        "operation_id": "op-aka-1"
    });
    let out = dispatch_authenticated_tool_call(
        journal.path(),
        principal_a,
        ToolName::SuggestCorrection,
        Some(&args_already_true),
        Utc::now(),
    )
    .unwrap();
    assert_eq!(out.value["status"], "already_true");
    assert!(!out.value["message"].as_str().unwrap().is_empty());

    // 2. Clearly different name creates a second card
    let args_new_alias_1 = json!({
        "kind": "name",
        "target": {"reference": entity_name},
        "value": {"text": "Danny", "mode": "also_known_as"},
        "basis": "owner_said",
        "reason": "new alias Danny",
        "operation_id": "op-aka-danny"
    });
    let out_1 = dispatch_authenticated_tool_call(
        journal.path(),
        principal_a,
        ToolName::SuggestCorrection,
        Some(&args_new_alias_1),
        Utc::now(),
    )
    .unwrap();
    assert_eq!(out_1.value["status"], "pending");
    let id_1 = out_1.value["suggestion_id"].as_str().unwrap().to_string();

    let args_new_alias_2 = json!({
        "kind": "name",
        "target": {"reference": entity_name},
        "value": {"text": "DeeDee", "mode": "also_known_as"},
        "basis": "owner_said",
        "reason": "new alias DeeDee",
        "operation_id": "op-aka-deedee"
    });
    let out_2 = dispatch_authenticated_tool_call(
        journal.path(),
        principal_a,
        ToolName::SuggestCorrection,
        Some(&args_new_alias_2),
        Utc::now(),
    )
    .unwrap();
    assert_eq!(out_2.value["status"], "pending");
    let id_2 = out_2.value["suggestion_id"].as_str().unwrap().to_string();
    assert_ne!(id_1, id_2);

    // 3. Cross-grant duplicate suggestion omits suggestion_id for agent B
    let out_b = dispatch_authenticated_tool_call(
        journal.path(),
        principal_b,
        ToolName::SuggestCorrection,
        Some(&args_new_alias_1),
        Utc::now(),
    )
    .unwrap();
    assert_eq!(out_b.value["status"], "already_exists");
    assert_eq!(out_b.value["existing_status"], "pending");
    assert!(out_b.value.get("suggestion_id").is_none());
}

#[test]
fn test_line_matching_and_narrow_lines() {
    let journal = init_test_journal();
    set_test_suggestions_ready(true);

    let day = "20261114";
    let stream = "default";
    let seg = "090000_300";

    // Create transcript with lines: one unique, and two duplicates at the same timestamp
    setup_transcript_segment(
        journal.path(),
        day,
        stream,
        seg,
        &[
            ("09:00:10", "Good morning team"),
            ("09:00:20", "Let us start the meeting"),
            ("09:00:20", "Let us start by reviewing numbers"),
        ],
    );

    let seg_name = crate::names::segment_name(journal.path(), day, stream, seg).unwrap();
    let principal = DispatchPrincipal {
        connection: "bearer:test-agent",
        agent_identity: "bearer:test-agent",
    };

    // 1. Two matches for selector at 09:00:20 with prefix "Let us start" returns next: "narrow_lines"
    let args_ambiguous = json!({
        "kind": "who_said_it",
        "target": {
            "reference": seg_name,
            "lines": [{"time": "09:00:20", "starts_with": "Let us start"}]
        },
        "value": {"speaker": "owner"},
        "basis": "owner_said",
        "reason": "owner spoke here",
        "operation_id": "op-lines-ambiguous"
    });
    let out_ambiguous = dispatch_authenticated_tool_call(
        journal.path(),
        principal,
        ToolName::SuggestCorrection,
        Some(&args_ambiguous),
        Utc::now(),
    )
    .unwrap();
    assert_eq!(out_ambiguous.value["status"], "refused");
    assert_eq!(out_ambiguous.value["next"], "narrow_lines");
    assert!(!out_ambiguous.value["message"].as_str().unwrap().is_empty());

    // 2. One unique match returns pending
    let args_unique = json!({
        "kind": "who_said_it",
        "target": {
            "reference": seg_name,
            "lines": [{"time": "09:00:10", "starts_with": "Good morning"}]
        },
        "value": {"speaker": "owner"},
        "basis": "owner_said",
        "reason": "owner spoke here",
        "operation_id": "op-lines-unique"
    });
    let out_unique = dispatch_authenticated_tool_call(
        journal.path(),
        principal,
        ToolName::SuggestCorrection,
        Some(&args_unique),
        Utc::now(),
    )
    .unwrap();
    assert_eq!(out_unique.value["status"], "pending");
    assert!(out_unique.value["suggestion_id"].is_string());
}

#[tokio::test]
async fn test_accept_and_undo_recording_or_activity() {
    let journal = init_test_journal();
    set_test_suggestions_ready(true);

    let day = "20261114";
    let act_id = "act_meeting_1";
    setup_activity(
        journal.path(),
        "work",
        day,
        act_id,
        "Original activity details",
    );

    let act_name = crate::names::activity_name(day, "work", act_id).unwrap();
    let principal = DispatchPrincipal {
        connection: "bearer:test-agent",
        agent_identity: "bearer:test-agent",
    };

    // 1. Submit recording suggestion
    let args = json!({
        "kind": "recording",
        "target": {"reference": act_name},
        "value": {},
        "basis": "owner_said",
        "reason": "this was a recording",
        "operation_id": "op-rec-1"
    });
    let out = dispatch_authenticated_tool_call(
        journal.path(),
        principal,
        ToolName::SuggestCorrection,
        Some(&args),
        Utc::now(),
    )
    .unwrap();
    assert_eq!(out.value["status"], "pending");
    let sugg_id = out.value["suggestion_id"].as_str().unwrap();

    // 2. Accept suggestion via owner API
    let (status, res) = owner_call(
        journal.path(),
        "POST",
        &format!("/app/agents/api/suggestions/{sugg_id}/accept"),
        Some(json!({})),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(res["suggestion"]["status"], "accepted");

    // Verify activity record updated with actor owner:update and suggested_by
    let act_rec = solstone_core_facets::get_activity_record(journal.path(), "work", day, act_id)
        .unwrap()
        .unwrap();
    assert_eq!(act_rec["playback"], true);
    let edits = act_rec["edits"].as_array().unwrap();
    let last_edit = edits.last().unwrap();
    assert_eq!(last_edit["actor"], "owner:update");
    assert!(last_edit.get("suggested_by").is_some());

    // 3. Undo suggestion via owner API
    let (status_undo, res_undo) = owner_call(
        journal.path(),
        "POST",
        &format!("/app/agents/api/suggestions/{sugg_id}/undo"),
        None,
        true,
    )
    .await;
    assert_eq!(status_undo, StatusCode::OK);
    assert_eq!(res_undo["suggestion"]["status"], "undone");

    // Verify edit removed
    let act_rec_undone =
        solstone_core_facets::get_activity_record(journal.path(), "work", day, act_id)
            .unwrap()
            .unwrap();
    let edits_undone = act_rec_undone["edits"].as_array().unwrap();
    assert!(edits_undone.last().unwrap().get("removed").is_some());
    assert_eq!(act_rec_undone["playback"], false);
}

#[tokio::test]
async fn test_name_undo_without_history_and_later_rename_conflict() {
    let journal = init_test_journal();
    set_test_suggestions_ready(true);

    let entity_id = "dana";
    setup_entity(
        journal.path(),
        "work",
        entity_id,
        "Dana Original",
        &["D", "Dan"],
    );

    // Verify history directory does not exist
    assert!(!journal.path().join("entities/dana/history").exists());

    let entity_name = crate::names::entity_name("work", entity_id).unwrap();
    let principal = DispatchPrincipal {
        connection: "bearer:test-agent",
        agent_identity: "bearer:test-agent",
    };

    // 1. Submit name suggestion
    let args = json!({
        "kind": "name",
        "target": {"reference": entity_name},
        "value": {"text": "Dana Proposed", "mode": "replace"},
        "basis": "owner_said",
        "reason": "name correction",
        "operation_id": "op-name-1"
    });
    let out = dispatch_authenticated_tool_call(
        journal.path(),
        principal,
        ToolName::SuggestCorrection,
        Some(&args),
        Utc::now(),
    )
    .unwrap();
    assert_eq!(out.value["status"], "pending");
    let sugg_id = out.value["suggestion_id"].as_str().unwrap();

    // 2. Accept suggestion with edit body
    let (status_accept, res_accept) = owner_call(
        journal.path(),
        "POST",
        &format!("/app/agents/api/suggestions/{sugg_id}/accept"),
        Some(json!({
            "value": {
                "text": "Dana Accepted",
                "mode": "replace"
            }
        })),
        true,
    )
    .await;
    assert_eq!(status_accept, StatusCode::OK);
    assert_eq!(res_accept["suggestion"]["status"], "accepted");

    // Check entity identity on disk has the accepted name
    let live = solstone_core_entity::read_entity_identity(journal.path(), entity_id)
        .unwrap()
        .unwrap();
    assert_eq!(live.value()["name"], "Dana Accepted");

    // Check receipt stored identity_before with original name and aliases
    let rec = read_record(journal.path(), sugg_id).unwrap().unwrap();
    let receipt = rec.receipt.unwrap();
    let before = receipt.identity_before.unwrap();
    assert_eq!(before["name"], "Dana Original");
    assert_eq!(before["aka"], json!(["D", "Dan"]));

    // 3. Undo restores original name and aliases
    let (status_undo, res_undo) = owner_call(
        journal.path(),
        "POST",
        &format!("/app/agents/api/suggestions/{sugg_id}/undo"),
        None,
        true,
    )
    .await;
    assert_eq!(status_undo, StatusCode::OK);
    assert_eq!(res_undo["suggestion"]["status"], "undone");

    let live_restored = solstone_core_entity::read_entity_identity(journal.path(), entity_id)
        .unwrap()
        .unwrap();
    assert_eq!(live_restored.value()["name"], "Dana Original");
    assert_eq!(live_restored.value()["aka"], json!(["D", "Dan"]));

    // 4. Test conflict: accept a second suggestion, then owner changes name -> undo gives 409 undo_refused
    let entity_id = "dana_second";
    setup_entity(
        journal.path(),
        "work",
        entity_id,
        "Dana Second Original",
        &[],
    );
    let entity_name = crate::names::entity_name("work", entity_id).unwrap();
    let args_2 = json!({
        "kind": "name",
        "target": {"reference": entity_name},
        "value": {"text": "Dana Second", "mode": "replace"},
        "basis": "owner_said",
        "reason": "second change",
        "operation_id": "op-name-2"
    });
    let out_2 = dispatch_authenticated_tool_call(
        journal.path(),
        principal,
        ToolName::SuggestCorrection,
        Some(&args_2),
        Utc::now(),
    )
    .unwrap();
    let sugg_id_2 = out_2.value["suggestion_id"].as_str().unwrap();

    let (status_accept_2, _) = owner_call(
        journal.path(),
        "POST",
        &format!("/app/agents/api/suggestions/{sugg_id_2}/accept"),
        None,
        true,
    )
    .await;
    assert_eq!(status_accept_2, StatusCode::OK);

    // Simulate an owner rename on the entity
    let _ = solstone_core_facets::update_facet_entity_identity(
        journal.path(),
        "work",
        "Dana Second",
        "Dana Owner Changed",
        "Person",
        &[],
        Some(entity_id),
        json!({}),
        |_| Ok(()),
    )
    .unwrap();

    // Now undo should return 409 undo_refused with can_undo: false
    let (status_conflict, res_conflict) = owner_call(
        journal.path(),
        "POST",
        &format!("/app/agents/api/suggestions/{sugg_id_2}/undo"),
        None,
        true,
    )
    .await;
    assert_eq!(status_conflict, StatusCode::CONFLICT);
    assert_eq!(res_conflict["code"], "undo_refused");
    assert_eq!(res_conflict["reason_code"], "undo_refused");
    assert_eq!(res_conflict["suggestion"]["status"], "accepted");
    assert_eq!(res_conflict["suggestion"]["can_undo"], false);
}

#[tokio::test]
async fn test_seams_receipt_failure_retry_and_ask_failure() {
    let journal = init_test_journal();
    set_test_suggestions_ready(true);

    let day = "20261114";
    let act_id = "act_seam_test";
    setup_activity(
        journal.path(),
        "work",
        day,
        act_id,
        "Original seam activity text",
    );
    let act_name = crate::names::activity_name(day, "work", act_id).unwrap();

    let principal = DispatchPrincipal {
        connection: "bearer:test-agent",
        agent_identity: "bearer:test-agent",
    };

    let args = json!({
        "kind": "what_happened",
        "target": {"reference": act_name},
        "value": {"text": "Updated seam text"},
        "basis": "owner_said",
        "reason": "seam test",
        "operation_id": "op-seam-1"
    });
    let out = dispatch_authenticated_tool_call(
        journal.path(),
        principal,
        ToolName::SuggestCorrection,
        Some(&args),
        Utc::now(),
    )
    .unwrap();
    let sugg_id = out.value["suggestion_id"].as_str().unwrap();

    // 1. Injected receipt failure causes 500 error
    set_force_suggestion_receipt_failure(true);
    let (status_err, res_err) = owner_call(
        journal.path(),
        "POST",
        &format!("/app/agents/api/suggestions/{sugg_id}/accept"),
        None,
        true,
    )
    .await;
    assert_eq!(status_err, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(res_err["code"], "receipt_failed");

    let ask_ids_before_retry: BTreeSet<_> = solstone_core_system::owner_ask::read(journal.path())
        .unwrap()
        .into_keys()
        .collect();
    assert_eq!(ask_ids_before_retry.len(), 1);
    // 2. Retrying with seam off finds existing effect and does not double-apply
    set_force_suggestion_receipt_failure(false);
    let (status_ok, res_ok) = owner_call(
        journal.path(),
        "POST",
        &format!("/app/agents/api/suggestions/{sugg_id}/accept"),
        None,
        true,
    )
    .await;
    assert_eq!(status_ok, StatusCode::OK);
    assert_eq!(res_ok["suggestion"]["status"], "accepted");

    assert_eq!(
        solstone_core_system::owner_ask::read(journal.path())
            .unwrap()
            .into_keys()
            .collect::<BTreeSet<_>>(),
        ask_ids_before_retry
    );
    // Check activity edits length is exactly 1 (not double-applied)
    let act_rec = solstone_core_facets::get_activity_record(journal.path(), "work", day, act_id)
        .unwrap()
        .unwrap();
    let edits = act_rec["edits"].as_array().unwrap();
    let matching_edits = edits
        .iter()
        .filter(|e| e.get("suggestion_id").and_then(Value::as_str) == Some(sugg_id))
        .count();
    assert_eq!(matching_edits, 1);

    // 3. Ask failure seam test
    let act_id_2 = "act_ask_test";
    setup_activity(
        journal.path(),
        "work",
        day,
        act_id_2,
        "Original ask activity text",
    );
    let act_name_2 = crate::names::activity_name(day, "work", act_id_2).unwrap();
    let args_2 = json!({
        "kind": "what_happened",
        "target": {"reference": act_name_2},
        "value": {"text": "Updated ask text"},
        "basis": "owner_said",
        "reason": "ask failure test",
        "operation_id": "op-ask-fail-1"
    });
    let out_2 = dispatch_authenticated_tool_call(
        journal.path(),
        principal,
        ToolName::SuggestCorrection,
        Some(&args_2),
        Utc::now(),
    )
    .unwrap();
    let sugg_id_2 = out_2.value["suggestion_id"].as_str().unwrap();

    let asks_before = solstone_core_system::owner_ask::read(journal.path())
        .unwrap()
        .len();

    set_force_suggestion_ask_failure(true);
    let (status_ask_fail, res_ask_fail) = owner_call(
        journal.path(),
        "POST",
        &format!("/app/agents/api/suggestions/{sugg_id_2}/accept"),
        None,
        true,
    )
    .await;
    assert_eq!(status_ask_fail, StatusCode::OK);
    assert_eq!(res_ask_fail["suggestion"]["status"], "accepted");

    // Check asked: false, ask_reason is set, and no ask written to store
    let rec_2 = read_record(journal.path(), sugg_id_2).unwrap().unwrap();
    assert_eq!(rec_2.asked, Some(false));
    assert!(rec_2.ask_reason.is_some());
    let asks = solstone_core_system::owner_ask::read(journal.path()).unwrap();
    assert_eq!(asks.len(), asks_before);
    assert!(
        !asks
            .values()
            .any(|a| a.to_seed.iter().any(|act| act.activity == act_id_2))
    );

    // 4. Seam off writes an ask via ask_corrected and sets asked: true
    set_force_suggestion_ask_failure(false);
    let act_id_3 = "act_ask_success";
    setup_activity(
        journal.path(),
        "work",
        day,
        act_id_3,
        "Original ask success activity text",
    );
    let act_name_3 = crate::names::activity_name(day, "work", act_id_3).unwrap();
    let args_3 = json!({
        "kind": "what_happened",
        "target": {"reference": act_name_3},
        "value": {"text": "Updated ask success text"},
        "basis": "owner_said",
        "reason": "ask success test",
        "operation_id": "op-ask-success-1"
    });
    let out_3 = dispatch_authenticated_tool_call(
        journal.path(),
        principal,
        ToolName::SuggestCorrection,
        Some(&args_3),
        Utc::now(),
    )
    .unwrap();
    let sugg_id_3 = out_3.value["suggestion_id"].as_str().unwrap();

    let (status_ask_ok, res_ask_ok) = owner_call(
        journal.path(),
        "POST",
        &format!("/app/agents/api/suggestions/{sugg_id_3}/accept"),
        None,
        true,
    )
    .await;
    assert_eq!(status_ask_ok, StatusCode::OK);
    assert_eq!(res_ask_ok["suggestion"]["status"], "accepted");

    let rec_3 = read_record(journal.path(), sugg_id_3).unwrap().unwrap();
    assert_eq!(rec_3.asked, Some(true));
    assert_eq!(rec_3.ask_reason, None);
    let asks_after = solstone_core_system::owner_ask::read(journal.path()).unwrap();
    assert_eq!(asks_after.len(), asks_before + 1);
    assert!(
        asks_after
            .values()
            .any(|a| a.to_seed.iter().any(|act| act.activity == act_id_3))
    );
}

#[test]
fn test_content_classifier() {
    assert_eq!(
        solstone_core_format::content::classify(
            "mcp-endpoint/correction-suggestions/records/sc_x.json"
        ),
        solstone_core_format::content::ContentResolution::Unrecognized
    );
}

#[tokio::test]
async fn test_owner_routes_and_access_control() {
    let journal = init_test_journal();

    // 1. Missing suggestion on accept returns 404 suggestion_not_found
    let (status, res) = owner_call(
        journal.path(),
        "POST",
        "/app/agents/api/suggestions/sc_missing_suggestion_id/accept",
        Some(json!({})),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(res["code"], "suggestion_not_found");
    assert_eq!(res["reason_code"], "suggestion_not_found");

    // 2. Missing suggestion on dismiss returns 404 suggestion_not_found
    let (status_dis, res_dis) = owner_call(
        journal.path(),
        "POST",
        "/app/agents/api/suggestions/sc_missing_suggestion_id/dismiss",
        None,
        true,
    )
    .await;
    assert_eq!(status_dis, StatusCode::NOT_FOUND);
    assert_eq!(res_dis["code"], "suggestion_not_found");

    // 3. Missing suggestion on undo returns 404 suggestion_not_found
    let (status_undo, res_undo) = owner_call(
        journal.path(),
        "POST",
        "/app/agents/api/suggestions/sc_missing_suggestion_id/undo",
        None,
        true,
    )
    .await;
    assert_eq!(status_undo, StatusCode::NOT_FOUND);
    assert_eq!(res_undo["code"], "suggestion_not_found");

    // 4. Non-owner agent route returns 404
    let (status_no_owner, _) = owner_call(
        journal.path(),
        "GET",
        "/app/agents/api/suggestions",
        None,
        false,
    )
    .await;
    assert_eq!(status_no_owner, StatusCode::NOT_FOUND);

    // 5. Invalid body on accept returns 400 invalid_body
    setup_activity(
        journal.path(),
        "work",
        "20261114",
        "act_body_check",
        "Original",
    );
    let act_name = crate::names::activity_name("20261114", "work", "act_body_check").unwrap();
    let principal = DispatchPrincipal {
        connection: "bearer:test-agent",
        agent_identity: "bearer:test-agent",
    };
    set_test_suggestions_ready(true);
    let out = dispatch_authenticated_tool_call(
        journal.path(),
        principal,
        ToolName::SuggestCorrection,
        Some(&json!({
            "kind": "recording",
            "target": {"reference": act_name},
            "value": {},
            "basis": "owner_said",
            "reason": "body check",
            "operation_id": "op-rec-body-check"
        })),
        Utc::now(),
    )
    .unwrap();
    let rec_id = out.value["suggestion_id"].as_str().unwrap();

    // Trying to edit recording (which cannot be edited) returns 400 invalid_body
    let (status_bad_body, res_bad_body) = owner_call(
        journal.path(),
        "POST",
        &format!("/app/agents/api/suggestions/{rec_id}/accept"),
        Some(json!({"value": {"text": "edited text"}})),
        true,
    )
    .await;
    assert_eq!(status_bad_body, StatusCode::BAD_REQUEST);
    assert_eq!(res_bad_body["code"], "invalid_body");

    // 6. Dismissing changes status to dismissed, accept then returns 409 not_pending
    let (status_dismiss_ok, _) = owner_call(
        journal.path(),
        "POST",
        &format!("/app/agents/api/suggestions/{rec_id}/dismiss"),
        None,
        true,
    )
    .await;
    assert_eq!(status_dismiss_ok, StatusCode::OK);

    let (status_conflict, res_conflict) = owner_call(
        journal.path(),
        "POST",
        &format!("/app/agents/api/suggestions/{rec_id}/accept"),
        None,
        true,
    )
    .await;
    assert_eq!(status_conflict, StatusCode::CONFLICT);
    assert_eq!(res_conflict["code"], "not_pending");

    // Undo on dismissed (not accepted) returns 409 not_accepted
    let (status_undo_not_acc, res_undo_not_acc) = owner_call(
        journal.path(),
        "POST",
        &format!("/app/agents/api/suggestions/{rec_id}/undo"),
        None,
        true,
    )
    .await;
    assert_eq!(status_undo_not_acc, StatusCode::CONFLICT);
    assert_eq!(res_undo_not_acc["code"], "not_accepted");
}

#[test]
fn test_sealed_token_still_resolves() {
    let journal = init_test_journal();
    set_test_suggestions_ready(true);

    let day = "20261114";
    let act_id = "act_sealed_test";
    setup_activity(
        journal.path(),
        "work",
        day,
        act_id,
        "Original sealed test details",
    );

    // Mint a sealed token for the activity
    let act_ref = ActivityReference {
        day: day.to_string(),
        facet_id: "work".to_string(),
        facet_dir: "work".to_string(),
        record_identity: act_id.to_string(),
        file_digest: "digest".to_string(),
        unit_index: None,
        source_binding: None,
    };
    let sealed_token = codec()
        .mint("bearer:test-agent", 1, ReferenceTarget::Activity(act_ref))
        .unwrap();

    let principal = DispatchPrincipal {
        connection: "bearer:test-agent",
        agent_identity: "bearer:test-agent",
    };
    let args = json!({
        "kind": "recording",
        "target": {"reference": sealed_token},
        "value": {},
        "basis": "owner_said",
        "reason": "sealed token test",
        "operation_id": "op-sealed-1"
    });
    let out = dispatch_authenticated_tool_call(
        journal.path(),
        principal,
        ToolName::SuggestCorrection,
        Some(&args),
        Utc::now(),
    )
    .unwrap();
    assert_eq!(out.value["status"], "pending");
    assert!(out.value["suggestion_id"].is_string());
}

fn suggest(root: &Path, kind: &str, target: &str, value: Value, operation: &str) -> Value {
    dispatch_authenticated_tool_call(root, DispatchPrincipal {connection:"bearer:test-agent",agent_identity:"bearer:test-agent"},
        ToolName::SuggestCorrection,Some(&json!({"kind":kind,"target":{"reference":target},"value":value,
            "basis":"owner_said","reason":"The owner corrected the record.","operation_id":operation})),Utc::now()).unwrap().value
}

#[tokio::test]
async fn owner_answers_do_not_validate_new_suggestion_guesses() {
    set_test_suggestions_ready(true);
    let mut failures = Vec::new();
    for (kind, mode) in [
        ("recording", ""),
        ("what_happened", ""),
        ("name", "replace"),
        ("name", "also_known_as"),
    ] {
        let journal = init_test_journal();
        let root = journal.path();
        let (target, value, guesses) = if kind == "name" {
            setup_entity(root, "work", "dana", "Dana Original", &[]);
            (
                crate::names::entity_name("work", "dana").unwrap(),
                json!({"text":"Dana Accepted","mode":mode}),
                vec![
                    json!({"text":"Dana Accepted","mode":mode}),
                    json!({"text":"Another name","mode":mode}),
                ],
            )
        } else {
            setup_activity(root, "work", "20261114", "answer", "Original");
            let value = if kind == "recording" {
                json!({})
            } else {
                json!({"text":"Accepted account"})
            };
            let guesses = if kind == "recording" {
                vec![json!({})]
            } else {
                vec![value.clone(), json!({"text":"Another account"})]
            };
            (
                crate::names::activity_name("20261114", "work", "answer").unwrap(),
                value,
                guesses,
            )
        };
        let queued = suggest(root, kind, &target, value.clone(), "original");
        let id = queued["suggestion_id"].as_str().unwrap();
        let (status, accepted) = owner_call(
            root,
            "POST",
            &format!("/app/agents/api/suggestions/{id}/accept"),
            None,
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{accepted}");
        let replay = suggest(root, kind, &target, value, "original");
        assert_eq!(replay["status"], "already_exists");
        assert_eq!(replay["existing_status"], "accepted");
        for (i, guess) in guesses.into_iter().enumerate() {
            let result = suggest(root, kind, &target, guess, &format!("guess-{i}"));
            if result["status"] != "owner_answered" {
                failures.push((kind, result));
            }
        }
        if kind == "recording" {
            solstone_core_facets::update_activity_record(
                root,
                "work",
                "20261114",
                "answer",
                json!({"playback":false}).as_object().unwrap(),
                "owner:update",
                "owner:update",
                &Utc::now().to_rfc3339(),
            )
            .unwrap();
            let result = suggest(root, kind, &target, json!({}), "guess-owner-false");
            if result["status"] != "owner_answered" {
                failures.push((kind, result));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[test]
fn speaker_suggestion_status_uses_only_admitted_voice_evidence() {
    use solstone_core_speaker_resolve::owner_line_labels::{
        LineSpeaker, OwnerLine, set_owner_line_labels,
    };
    set_test_suggestions_ready(true);
    let journal = init_test_journal();
    let root = journal.path();
    setup_activity(root, "work", "20261114", "meeting_1", "");
    setup_entity(root, "work", "ada", "Ada", &[]);
    setup_entity(root, "work", "owner", "Synthetic Owner", &[]);
    let owner_path = root.join("entities/owner/entity.json");
    let mut owner: Value = serde_json::from_slice(&fs::read(&owner_path).unwrap()).unwrap();
    owner["is_principal"] = json!(true);
    fs::write(owner_path, owner.to_string()).unwrap();
    setup_transcript_segment(
        root,
        "20261114",
        "default",
        "090000_300",
        &[("09:00:10", "A sentence from the journal")],
    );
    let segment =
        solstone_core_segment::SegmentDir::resolve(root, "20261114", "090000_300", "default")
            .unwrap();
    let target = crate::names::segment_name(root, "20261114", "default", "090000_300").unwrap();
    fs::create_dir_all(segment.path().join("talents")).unwrap();
    let labels = segment.path().join("talents/speaker_labels.json");
    let call = |speaker: &str, operation: &str| {
        dispatch_authenticated_tool_call(root,
        DispatchPrincipal {connection:"bearer:test-agent",agent_identity:"bearer:test-agent"}, ToolName::SuggestCorrection,
        Some(&json!({"kind":"who_said_it","target":{"reference":target,"lines":[{"time":"09:00:10","starts_with":"A sentence"}]},"value":{"speaker":speaker},"basis":"owner_said","reason":"A synthetic correction.","operation_id":operation})), Utc::now()).unwrap().value
    };
    let mut failures = Vec::new();
    for (i, (speaker, method, proposed, expected)) in [
        ("ada", "contextual", "ada@work", "pending"),
        ("ada", "acoustic", "ada@work", "already_true"),
        ("owner", "owner_centroid", "owner", "pending"),
        ("ada", "contextual", "someone_else", "pending"),
    ]
    .into_iter()
    .enumerate()
    {
        fs::write(&labels, json!({"labels":[{"sentence_id":1,"speaker":speaker,"method":method,"owner_margin_declined":true}]}).to_string()).unwrap();
        let result = call(proposed, &format!("voice-{i}"));
        if result["status"] != expected {
            failures.push((method, proposed, result));
        }
    }
    set_owner_line_labels(
        root,
        &[OwnerLine {
            segment_dir: segment.path().to_path_buf(),
            source: "audio".to_owned(),
            sentence_id: 1,
        }],
        &LineSpeaker::Person("ada".to_owned()),
    )
    .unwrap();
    for (i, speaker) in ["ada@work", "owner", "someone_else"]
        .into_iter()
        .enumerate()
    {
        let result = call(speaker, &format!("owner-answer-{i}"));
        if result["status"] != "owner_answered" {
            failures.push(("owner answer", speaker, result));
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[test]
fn anonymous_speaker_label_shapes_do_not_validate_guesses() {
    use solstone_core_journal_io::paths::{PathOrDay, iter_segments_skipping_unreadable_streams};
    set_test_suggestions_ready(true);
    for shape in [
        "absent",
        "null",
        "contextual",
        "oversized",
        "declined",
        "acoustic",
        "owner",
    ] {
        let journal = init_test_journal();
        let root = journal.path();
        setup_activity(root, "work", "20261114", "meeting_1", "");
        setup_entity(root, "work", "ada", "Ada", &[]);
        setup_entity(root, "work", "owner", "Synthetic Owner", &[]);
        let owner_path = root.join("entities/owner/entity.json");
        let mut owner: Value = serde_json::from_slice(&fs::read(&owner_path).unwrap()).unwrap();
        owner["is_principal"] = json!(true);
        fs::write(owner_path, owner.to_string()).unwrap();
        solstone_core_speaker_resolve::owner_centroid::write_owner_centroid(
            root,
            "owner",
            &solstone_core_speaker_resolve::owner_centroid::OwnerCentroidWriteInput {
                centroid: vec![1.0, 0.0],
                cluster_size: 8,
                timestamp: Utc::now().to_rfc3339(),
                evidence_tier: "synthetic".to_owned(),
            },
        )
        .unwrap();
        setup_transcript_segment(
            root,
            "20261114",
            "default",
            "090000_300",
            &[("09:00:10", "A synthetic sentence")],
        );
        let segment = iter_segments_skipping_unreadable_streams(root, PathOrDay::Day("20261114"))
            .unwrap()
            .pop()
            .unwrap();
        fs::create_dir_all(segment.path().join("talents")).unwrap();
        let mut row = json!({"sentence_id":1,"speaker":"ada","method":"acoustic"});
        match shape {
            "null" => row["speaker"] = Value::Null,
            "contextual" => row["method"] = json!("contextual"),
            "declined" | "owner" => {
                row["speaker"] = json!("owner");
                row["method"] = json!("owner_centroid");
                row["owner_margin_declined"] = json!(shape == "declined");
            }
            _ => {}
        }
        if shape != "absent" {
            let mut labels = json!({"labels":[row]});
            if shape == "oversized" {
                labels["padding"] =
                    json!("x".repeat(solstone_core_transcripts::MAX_TRANSCRIPT_PAGE_BYTES));
            }
            fs::write(
                segment.path().join("talents/speaker_labels.json"),
                labels.to_string(),
            )
            .unwrap();
        }
        let speakers = solstone_core_talent_runtime::agent_speakers(root).unwrap();
        assert!(speakers.voices.owner_voice_confirmed);
        let page = solstone_core_transcripts::read_segment_transcript_page(
            &segment,
            None,
            Some(&speakers),
            None,
        )
        .unwrap();
        assert!(!page.entries.is_empty());
        assert_eq!(
            page.entries.iter().any(|entry| entry.text.contains("Ada:")),
            shape == "acoustic",
            "{shape}: {page:?}"
        );
        assert_eq!(
            page.entries
                .iter()
                .any(|entry| entry.text.contains("Synthetic Owner")),
            shape == "owner",
            "{shape}: {page:?}"
        );
        let target = crate::names::segment_name(root, "20261114", "default", "090000_300").unwrap();
        for (i, proposed) in ["ada@work", "someone_else", "owner"]
            .into_iter()
            .enumerate()
        {
            let result = dispatch_authenticated_tool_call(root,
                DispatchPrincipal {connection:"bearer:test-agent",agent_identity:"bearer:test-agent"}, ToolName::SuggestCorrection,
                Some(&json!({"kind":"who_said_it","target":{"reference":target,"lines":[{"time":"09:00:10","starts_with":"A synthetic"}]},"value":{"speaker":proposed},"basis":"owner_said","reason":"A synthetic correction.","operation_id":format!("shape-{i}")})), Utc::now()).unwrap().value;
            assert_eq!(
                result["status"],
                if (shape == "acoustic" && proposed == "ada@work")
                    || (shape == "owner" && proposed == "owner")
                {
                    "already_true"
                } else {
                    "pending"
                },
                "{shape}: {result}"
            );
        }
    }
}

#[tokio::test]
async fn edited_accept_replays_current_decision_and_hidden_target_discloses_nothing() {
    set_test_suggestions_ready(true);
    let journal = init_test_journal();
    let root = journal.path();
    let day = "20261114";
    setup_activity(root, "work", day, "replay", "Original account");
    let target = crate::names::activity_name(day, "work", "replay").unwrap();
    let queued = suggest(
        root,
        "what_happened",
        &target,
        json!({"text":"Proposed account"}),
        "replay",
    );
    let id = queued["suggestion_id"].as_str().unwrap();
    let path = format!("/app/agents/api/suggestions/{id}");
    let hash = read_record(root, id).unwrap().unwrap().request_sha256;
    let (status, accepted) = owner_call(
        root,
        "POST",
        &format!("{path}/accept"),
        Some(json!({"value":{"text":"The owner's edited account"}})),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    assert_eq!(
        accepted["suggestion"]["change_parts"]["text"],
        "The owner's edited account"
    );
    let retried = suggest(
        root,
        "what_happened",
        &target,
        json!({"text":"Proposed account"}),
        "replay",
    );
    assert_eq!(retried["status"], "already_exists");
    assert_eq!(retried["existing_status"], "accepted");
    assert!(!retried.to_string().contains("edited account"));
    assert_eq!(read_record(root, id).unwrap().unwrap().request_sha256, hash);
    let (status, replay) = owner_call(root, "POST", &format!("{path}/accept"), None, true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay, accepted);
    let (status, _) = owner_call(root, "POST", &format!("{path}/undo"), None, true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        suggest(
            root,
            "what_happened",
            &target,
            json!({"text":"Proposed account"}),
            "replay"
        )["status"],
        "owner_answered"
    );
    solstone_core_facets::set_activity_hidden(
        root,
        "work",
        day,
        "replay",
        true,
        "owner:hide",
        None,
        &Utc::now().to_rfc3339(),
    )
    .unwrap();
    let refused = suggest(
        root,
        "what_happened",
        &target,
        json!({"text":"Proposed account"}),
        "replay",
    );
    assert_eq!(refused["status"], "refused");
    assert!(refused.get("suggestion_id").is_none());
    assert!(refused.get("existing_status").is_none());
}

#[tokio::test]
async fn same_value_owner_answer_stales_card_and_later_account_blocks_undo() {
    set_test_suggestions_ready(true);
    let journal = init_test_journal();
    let root = journal.path();
    let day = "20261114";
    setup_activity(root, "work", day, "stale", "Original account");
    let target = crate::names::activity_name(day, "work", "stale").unwrap();
    let queued = suggest(root, "recording", &target, json!({}), "same-value");
    let id = queued["suggestion_id"].as_str().unwrap();
    solstone_core_facets::update_activity_record(
        root,
        "work",
        day,
        "stale",
        json!({"playback":false}).as_object().unwrap(),
        "owner:update",
        "owner:update",
        &Utc::now().to_rfc3339(),
    )
    .unwrap();
    let (status, stale) = owner_call(
        root,
        "POST",
        &format!("/app/agents/api/suggestions/{id}/accept"),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(stale["code"], "already_changed");
    let (_, listed) = owner_call(root, "GET", "/app/agents/api/suggestions", None, true).await;
    assert_eq!(listed["items"][0]["status"], "stale");
    assert_eq!(listed["agents"][0]["pending_count"], 0);
    let queued = suggest(
        root,
        "what_happened",
        &target,
        json!({"text":"Agent account"}),
        "later-answer",
    );
    let id = queued["suggestion_id"].as_str().unwrap();
    let path = format!("/app/agents/api/suggestions/{id}");
    assert_eq!(
        owner_call(root, "POST", &format!("{path}/accept"), None, true)
            .await
            .0,
        StatusCode::OK
    );
    solstone_core_facets::update_activity_record(
        root,
        "work",
        day,
        "stale",
        json!({"details":"Later owner account"})
            .as_object()
            .unwrap(),
        "owner:update",
        "owner:update",
        &Utc::now().to_rfc3339(),
    )
    .unwrap();
    let before = fs::read(root.join(format!("facets/work/activities/{day}.jsonl"))).unwrap();
    let (status, refused) = owner_call(root, "POST", &format!("{path}/undo"), None, true).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(refused["code"], "undo_refused");
    assert_eq!(refused["suggestion"]["can_undo"], false);
    assert_eq!(
        fs::read(root.join(format!("facets/work/activities/{day}.jsonl"))).unwrap(),
        before
    );
}

#[tokio::test]
async fn combined_ask_and_receipt_failures_recover_without_losing_receipts_or_dismissing_effect() {
    set_test_suggestions_ready(true);
    let journal = init_test_journal();
    let root = journal.path();
    let day = "20261114";
    setup_activity(root, "work", day, "combined", "Original account");
    let target = crate::names::activity_name(day, "work", "combined").unwrap();
    let queued = suggest(
        root,
        "what_happened",
        &target,
        json!({"text":"Corrected account"}),
        "combined",
    );
    let id = queued["suggestion_id"].as_str().unwrap();
    let path = format!("/app/agents/api/suggestions/{id}");
    set_force_suggestion_ask_failure(true);
    set_force_suggestion_receipt_failure(true);
    assert_eq!(
        owner_call(root, "POST", &format!("{path}/accept"), None, true)
            .await
            .0,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    set_force_suggestion_ask_failure(false);
    set_force_suggestion_receipt_failure(false);
    assert_eq!(
        owner_call(root, "POST", &format!("{path}/dismiss"), None, true)
            .await
            .0,
        StatusCode::CONFLICT
    );
    let (status, recovered) = owner_call(root, "POST", &format!("{path}/accept"), None, true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recovered["suggestion"]["asked"], false);
    assert!(recovered["suggestion"]["ask_reason"].is_string());
    let reason = recovered["suggestion"]["ask_reason"].clone();
    set_force_suggestion_ask_failure(true);
    set_force_suggestion_receipt_failure(true);
    assert_eq!(
        owner_call(root, "POST", &format!("{path}/undo"), None, true)
            .await
            .0,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    set_force_suggestion_ask_failure(false);
    set_force_suggestion_receipt_failure(false);
    let (status, undone) = owner_call(root, "POST", &format!("{path}/undo"), None, true).await;
    assert_eq!(status, StatusCode::OK, "{undone}");
    assert_eq!(undone["suggestion"]["asked"], false);
    assert_eq!(undone["suggestion"]["ask_reason"], reason);
    let activity = solstone_core_facets::get_activity_record(root, "work", day, "combined")
        .unwrap()
        .unwrap();
    assert_eq!(activity["details"], "Original account");
    assert_eq!(activity["edits"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn name_inverse_preserves_unrelated_owner_identity_fields() {
    set_test_suggestions_ready(true);
    let journal = init_test_journal();
    let root = journal.path();
    setup_entity(root, "work", "dana", "Dana Original", &["D"]);
    let target = crate::names::entity_name("work", "dana").unwrap();
    let queued = suggest(
        root,
        "name",
        &target,
        json!({"text":"Dana Proposed","mode":"also_known_as"}),
        "name-mode-edit",
    );
    let id = queued["suggestion_id"].as_str().unwrap();
    let path = format!("/app/agents/api/suggestions/{id}");
    let (status, accepted) = owner_call(
        root,
        "POST",
        &format!("{path}/accept"),
        Some(json!({"value":{"text":"Dana Accepted","mode":"replace"}})),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    assert_eq!(
        accepted["suggestion"]["change_parts"]["from"],
        "Dana Original"
    );
    assert_eq!(
        accepted["suggestion"]["change_parts"]["to"],
        "Dana Accepted"
    );
    let receipt = read_record(root, id).unwrap().unwrap().receipt.unwrap();
    assert!(receipt.version_id.unwrap().starts_with("vh_"));
    let mut identity = solstone_core_entity::read_entity_identity(root, "dana")
        .unwrap()
        .unwrap()
        .value()
        .clone();
    identity["type"] = json!("Organization");
    identity["custom_owner_note"] = json!("keep this");
    solstone_core_entity::save_entity_identity(root, "dana", &identity, None).unwrap();
    let (status, undone) = owner_call(root, "POST", &format!("{path}/undo"), None, true).await;
    assert_eq!(status, StatusCode::OK, "{undone}");
    let identity = solstone_core_entity::read_entity_identity(root, "dana")
        .unwrap()
        .unwrap();
    assert_eq!(identity.value()["name"], "Dana Original");
    assert_eq!(identity.value()["aka"], json!(["D"]));
    assert_eq!(identity.value()["type"], "Organization");
    assert_eq!(identity.value()["custom_owner_note"], "keep this");
}

#[tokio::test]
async fn oauth_switch_uses_grant_id_and_queue_only_agents_survive_revocation() {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use sha2::{Digest, Sha256};
    use std::net::{IpAddr, Ipv4Addr};
    set_test_suggestions_ready(true);
    let journal = init_test_journal();
    let root = journal.path();
    let runtime = crate::oauth::OAuthRuntime::new(root, "https://mcp.test".to_owned());
    let binding = runtime.binding();
    let client = runtime
        .store
        .register_client(
            "https://client.example/cimd.json",
            vec!["http://127.0.0.1/callback".to_owned()],
            Some("OAuth Agent".to_owned()),
            &runtime.source_cohort(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))),
        )
        .unwrap();
    let pairing = runtime
        .store
        .generate_pairing_code_with_door("relay")
        .unwrap();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(b"suggestions-verifier"));
    let transaction = runtime
        .store
        .create_transaction_with_generation(
            &client.id,
            "http://127.0.0.1/callback",
            binding.canonical(),
            "https://mcp.test",
            &challenge,
            "S256",
            None,
            "192.0.2.1",
            binding.stored_grant_generation(),
        )
        .unwrap();
    let authorization = runtime
        .store
        .complete_pairing_with_permission(
            &transaction,
            &pairing.code,
            Some(ReadPermission::default_whole_journal()),
            &binding,
        )
        .unwrap();
    let issued = runtime
        .store
        .redeem_authorization_code(
            &authorization.code,
            &client.client_id,
            "http://127.0.0.1/callback",
            binding.canonical(),
            "suggestions-verifier",
            &binding,
        )
        .unwrap();
    let connection = format!("oauth:{}", issued.token_id);
    setup_activity(root, "work", "20261114", "oauth", "Original account");
    let target = crate::names::activity_name("20261114", "work", "oauth").unwrap();
    let args = json!({"kind":"what_happened","target":{"reference":target},"value":{"text":"Corrected OAuth account"},"basis":"owner_said","reason":"The owner corrected it.","operation_id":"oauth-operation"});
    let principal = DispatchPrincipal {
        connection: &connection,
        agent_identity: &connection,
    };
    let queued = dispatch_authenticated_tool_call(
        root,
        principal,
        ToolName::SuggestCorrection,
        Some(&args),
        Utc::now(),
    )
    .unwrap()
    .value;
    assert_eq!(queued["status"], "pending");
    let (status, _) = owner_call(
        root,
        "PUT",
        &format!(
            "/app/agents/api/connections/oauth/{}/suggestions",
            issued.token_id
        ),
        Some(json!({"enabled":false})),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mut new_args = args.clone();
    new_args["operation_id"] = json!("oauth-disabled");
    assert_eq!(
        dispatch_authenticated_tool_call(
            root,
            principal,
            ToolName::SuggestCorrection,
            Some(&new_args),
            Utc::now()
        )
        .unwrap()
        .value["status"],
        "suggestions_off"
    );
    assert_eq!(
        dispatch_authenticated_tool_call(
            root,
            principal,
            ToolName::SuggestCorrection,
            Some(&args),
            Utc::now()
        )
        .unwrap()
        .value["status"],
        "pending"
    );
    let (_, listed) = owner_call(root, "GET", "/app/agents/api/suggestions", None, true).await;
    assert_eq!(listed["agents"][0]["label"], "OAuth Agent");
    assert_eq!(listed["agents"][0]["connected"], true);
    assert_eq!(listed["agents"][0]["pending_count"], 1);
    runtime.store.revoke_grant_by_id(&issued.token_id).unwrap();
    let (_, listed) = owner_call(root, "GET", "/app/agents/api/suggestions", None, true).await;
    assert_eq!(listed["agents"][0]["label"], "OAuth Agent");
    assert_eq!(listed["agents"][0]["connected"], false);
    assert_eq!(listed["agents"][0]["pending_count"], 1);
    let id = queued["suggestion_id"].as_str().unwrap();
    assert_eq!(
        owner_call(
            root,
            "POST",
            &format!("/app/agents/api/suggestions/{id}/dismiss"),
            None,
            true
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn pending_cap_excludes_stale_cards_but_daily_cap_counts_every_new_suggestion() {
    set_test_suggestions_ready(true);
    let journal = init_test_journal();
    let root = journal.path();
    let first = chrono::NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
    let mut ids = Vec::new();
    for n in 0..20 {
        let day = (first + chrono::Duration::days(n))
            .format("%Y%m%d")
            .to_string();
        setup_activity(root, "work", &day, "capacity", "Original");
        let target = crate::names::activity_name(&day, "work", "capacity").unwrap();
        let queued = suggest(
            root,
            "recording",
            &target,
            json!({}),
            &format!("capacity-{n}"),
        );
        assert_eq!(queued["status"], "pending", "{queued}");
        ids.push(queued["suggestion_id"].as_str().unwrap().to_owned());
    }
    let day = (first + chrono::Duration::days(20))
        .format("%Y%m%d")
        .to_string();
    setup_activity(root, "work", &day, "capacity", "Original");
    let target = crate::names::activity_name(&day, "work", "capacity").unwrap();
    assert_eq!(
        suggest(root, "recording", &target, json!({}), "capacity-20")["next"],
        "wait_for_review"
    );
    solstone_core_facets::update_activity_record(
        root,
        "work",
        "20260901",
        "capacity",
        json!({"playback":false}).as_object().unwrap(),
        "owner:update",
        "owner:update",
        &Utc::now().to_rfc3339(),
    )
    .unwrap();
    let queued = suggest(root, "recording", &target, json!({}), "capacity-20");
    assert_eq!(queued["status"], "pending");
    ids.push(queued["suggestion_id"].as_str().unwrap().to_owned());
    for id in ids {
        assert_eq!(
            owner_call(
                root,
                "POST",
                &format!("/app/agents/api/suggestions/{id}/dismiss"),
                None,
                true
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    for n in 21..50 {
        let day = (first + chrono::Duration::days(n))
            .format("%Y%m%d")
            .to_string();
        setup_activity(root, "work", &day, "capacity", "Original");
        let target = crate::names::activity_name(&day, "work", "capacity").unwrap();
        let queued = suggest(
            root,
            "recording",
            &target,
            json!({}),
            &format!("capacity-{n}"),
        );
        assert_eq!(queued["status"], "pending", "{queued}");
        let id = queued["suggestion_id"].as_str().unwrap();
        assert_eq!(
            owner_call(
                root,
                "POST",
                &format!("/app/agents/api/suggestions/{id}/dismiss"),
                None,
                true
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    let day = (first + chrono::Duration::days(50))
        .format("%Y%m%d")
        .to_string();
    setup_activity(root, "work", &day, "capacity", "Original");
    let target = crate::names::activity_name(&day, "work", "capacity").unwrap();
    assert_eq!(
        suggest(root, "recording", &target, json!({}), "capacity-50")["next"],
        "retry_tomorrow"
    );
}

#[tokio::test]
async fn interrupted_stretch_accept_stays_reviewable_and_undo_preserves_failed_copy_owner_answer() {
    set_test_suggestions_ready(true);
    let journal = init_test_journal();
    let root = journal.path();
    let day = "20261114";
    for facet in ["work", "other"] {
        if !root.join("facets").join(facet).join("facet.json").exists() {
            solstone_core_facets::create_facet(root, facet, facet, "", "#123456", "", None)
                .unwrap();
        }
        setup_activity(root, facet, day, "stretch", "Original");
    }
    let path = root.join(format!("facets/other/activities/{day}.jsonl"));
    let mut copy: Value = serde_json::from_str(fs::read_to_string(&path).unwrap().trim()).unwrap();
    copy["destination_id"] = json!("stale-destination");
    fs::write(&path, format!("{copy}\n")).unwrap();
    let target = crate::names::activity_name(day, "work", "stretch").unwrap();
    let queued = suggest(
        root,
        "what_happened",
        &target,
        json!({"text":"The corrected stretch"}),
        "stretch-recovery",
    );
    let id = queued["suggestion_id"].as_str().unwrap();
    let base = format!("/app/agents/api/suggestions/{id}");
    let (status, accepted) = owner_call(root, "POST", &format!("{base}/accept"), None, true).await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    assert_eq!(accepted["suggestion"]["status"], "accepted");
    assert_eq!(accepted["suggestion"]["incomplete"], true);
    assert_eq!(accepted["suggestion"]["ask_reason"], "stretch_incomplete");
    assert_eq!(accepted["suggestion"]["can_undo"], true);
    assert_eq!(
        accepted["suggestion"]["impact"]["activities"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    copy.as_object_mut().unwrap().remove("destination_id");
    fs::write(&path, format!("{copy}\n")).unwrap();
    solstone_core_facets::update_activity_record(
        root,
        "other",
        day,
        "stretch",
        json!({"details":"Later owner answer"}).as_object().unwrap(),
        "owner:update",
        "owner:update",
        &Utc::now().to_rfc3339(),
    )
    .unwrap();
    assert_eq!(
        owner_call(root, "GET", "/app/agents/api/suggestions", None, true)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        owner_call(root, "POST", &format!("{base}/accept"), None, true)
            .await
            .1["suggestion"]["incomplete"],
        true
    );
    let (status, undone) = owner_call(root, "POST", &format!("{base}/undo"), None, true).await;
    assert_eq!(status, StatusCode::OK, "{undone}");
    assert_eq!(
        solstone_core_facets::get_activity_record(root, "work", day, "stretch")
            .unwrap()
            .unwrap()["details"],
        "Original"
    );
    assert_eq!(
        solstone_core_facets::get_activity_record(root, "other", day, "stretch")
            .unwrap()
            .unwrap()["details"],
        "Later owner answer"
    );
    let asks = solstone_core_system::owner_ask::read(root).unwrap();
    assert!(
        asks.values()
            .all(|ask| ask.to_seed.iter().all(|asked| asked.facet == "work"))
    );
}

#[test]
fn deleted_or_staged_recordings_refuse_new_suggestions_and_exact_retry() {
    set_test_suggestions_ready(true);
    let journal = init_test_journal();
    let root = journal.path();
    let day = "20261114";
    setup_activity(root, "work", day, "deleted", "Original");
    setup_entity(root, "work", "ada", "Ada", &[]);
    setup_transcript_segment(
        root,
        day,
        "default",
        "090000_300",
        &[("09:00:10", "A unique sentence")],
    );
    let principal = DispatchPrincipal {
        connection: "bearer:test-agent",
        agent_identity: "bearer:test-agent",
    };
    let listed = dispatch_authenticated_tool_call(
        root,
        principal,
        ToolName::ListTranscripts,
        Some(&json!({"day":day})),
        Utc::now(),
    )
    .unwrap()
    .value;
    let reference = listed["transcripts"][0]["reference"].as_str().unwrap();
    let args = json!({"kind":"who_said_it","target":{"reference":reference,"lines":[{"time":"09:00:10","starts_with":"A unique"}]},"value":{"speaker":"ada@work"},"basis":"owner_said","reason":"The owner identified Ada.","operation_id":"deleted-retry"});
    assert_eq!(
        dispatch_authenticated_tool_call(
            root,
            principal,
            ToolName::SuggestCorrection,
            Some(&args),
            Utc::now()
        )
        .unwrap()
        .value["status"],
        "pending"
    );
    let segment =
        solstone_core_segment::SegmentDir::resolve(root, day, "090000_300", "default").unwrap();
    let staged = segment
        .path()
        .parent()
        .unwrap()
        .join(".removing_090000_300");
    for marker in [segment.path().join("tombstone.json"), staged] {
        fs::write(&marker, "{}").unwrap();
        assert!(!crate::source_release::check_segment_transcript_release(
            root,
            day,
            "default",
            "090000_300"
        ));
        for operation in ["deleted-retry", "deleted-new"] {
            let mut input = args.clone();
            input["operation_id"] = json!(operation);
            let refused = dispatch_authenticated_tool_call(
                root,
                principal,
                ToolName::SuggestCorrection,
                Some(&input),
                Utc::now(),
            )
            .unwrap()
            .value;
            assert_eq!(refused["status"], "refused");
            assert!(refused.get("suggestion_id").is_none());
            assert!(refused.get("existing_status").is_none());
        }
        fs::remove_file(marker).unwrap();
    }
    assert_eq!(
        crate::suggestions::store::list_records(root).unwrap().len(),
        1
    );
}

#[tokio::test]
async fn name_round_trip_answer_blocks_undo_with_current_card_and_blocks_new_rename() {
    set_test_suggestions_ready(true);
    let journal = init_test_journal();
    let root = journal.path();
    setup_entity(root, "work", "dana", "Dana Original", &[]);
    let target = crate::names::entity_name("work", "dana").unwrap();
    let queued = suggest(
        root,
        "name",
        &target,
        json!({"text":"Dana Accepted","mode":"replace"}),
        "roundtrip",
    );
    let id = queued["suggestion_id"].as_str().unwrap();
    let path = format!("/app/agents/api/suggestions/{id}");
    assert_eq!(
        owner_call(root, "POST", &format!("{path}/accept"), None, true)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        suggest(
            root,
            "name",
            &target,
            json!({"text":"Another name","mode":"replace"}),
            "after-owner-accept"
        )["status"],
        "owner_answered"
    );
    for (old, new) in [
        ("Dana Accepted", "Dana Other"),
        ("Dana Other", "Dana Accepted"),
    ] {
        solstone_core_facets::update_facet_entity_identity(
            root,
            "work",
            old,
            new,
            "",
            &[],
            Some("dana"),
            json!({"owner_answer":true}),
            |_| Ok(()),
        )
        .unwrap();
    }
    let (_, listed) = owner_call(root, "GET", "/app/agents/api/suggestions", None, true).await;
    assert_eq!(listed["items"][0]["can_undo"], false);
    let before = fs::read(root.join("entities/dana/entity.json")).unwrap();
    let (status, refused) = owner_call(root, "POST", &format!("{path}/undo"), None, true).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["code"], "undo_refused");
    assert_eq!(refused["suggestion"]["can_undo"], false);
    assert_eq!(
        fs::read(root.join("entities/dana/entity.json")).unwrap(),
        before
    );
}

#[test]
fn unrelated_unsegmented_activities_do_not_dedup_and_overlapping_stretches_do() {
    set_test_suggestions_ready(true);
    let journal = init_test_journal();
    let root = journal.path();
    let day = "20261114";
    setup_activity(root, "work", day, "one", "Original");
    let mut first: Value = json!(
        solstone_core_facets::get_activity_record(root, "work", day, "one")
            .unwrap()
            .unwrap()
    );
    first["segments"] = json!([]);
    let mut second = first.clone();
    second["id"] = json!("two");
    let path = root.join(format!("facets/work/activities/{day}.jsonl"));
    fs::write(&path, format!("{first}\n{second}\n")).unwrap();
    let one = crate::names::activity_name(day, "work", "one").unwrap();
    let two = crate::names::activity_name(day, "work", "two").unwrap();
    let a = suggest(root, "recording", &one, json!({}), "unsegmented-one");
    let b = suggest(root, "recording", &two, json!({}), "unsegmented-two");
    assert_eq!(a["status"], "pending");
    assert_eq!(b["status"], "pending");
    assert_ne!(a["suggestion_id"], b["suggestion_id"]);
    first["segments"] = json!(["090000_300", "090500_300"]);
    second["segments"] = json!(["090500_300", "091000_300"]);
    fs::write(&path, format!("{first}\n{second}\n")).unwrap();
    let a = suggest(
        root,
        "what_happened",
        &one,
        json!({"text":"The shared stretch account"}),
        "overlap-one",
    );
    let b = suggest(
        root,
        "what_happened",
        &two,
        json!({"text":"The shared stretch account"}),
        "overlap-two",
    );
    assert_eq!(a["status"], "pending");
    assert_eq!(b["status"], "already_exists");
    assert_eq!(a["suggestion_id"], b["suggestion_id"]);
}

#[tokio::test]
async fn derived_speaker_change_stales_a_pending_line_without_changing_its_words() {
    set_test_suggestions_ready(true);
    let journal = init_test_journal();
    let root = journal.path();
    let day = "20261114";
    setup_activity(root, "work", day, "speaker-state", "Original");
    setup_entity(root, "work", "ada", "Ada", &[]);
    setup_transcript_segment(
        root,
        day,
        "default",
        "090000_300",
        &[("09:00:10", "A unique sentence")],
    );
    let principal = DispatchPrincipal {
        connection: "bearer:test-agent",
        agent_identity: "bearer:test-agent",
    };
    let listed = dispatch_authenticated_tool_call(
        root,
        principal,
        ToolName::ListTranscripts,
        Some(&json!({"day":day})),
        Utc::now(),
    )
    .unwrap()
    .value;
    let reference = listed["transcripts"][0]["reference"].as_str().unwrap();
    let input = json!({"kind":"who_said_it","target":{"reference":reference,"lines":[{"time":"09:00:10","starts_with":"A unique"}]},"value":{"speaker":"ada@work"},"basis":"owner_said","reason":"The owner identified Ada.","operation_id":"speaker-state"});
    let queued = dispatch_authenticated_tool_call(
        root,
        principal,
        ToolName::SuggestCorrection,
        Some(&input),
        Utc::now(),
    )
    .unwrap()
    .value;
    assert_eq!(queued["status"], "pending");
    let id = queued["suggestion_id"].as_str().unwrap();
    let directory =
        solstone_core_segment::SegmentDir::resolve(root, day, "090000_300", "default").unwrap();
    fs::create_dir_all(directory.path().join("talents")).unwrap();
    fs::write(
        directory.path().join("talents/speaker_labels.json"),
        json!({"labels":[{"sentence_id":1,"speaker":"ada"}]}).to_string(),
    )
    .unwrap();
    assert_eq!(
        solstone_core_transcripts::load_who_spoke(directory.path(), Some("audio"), None).rows[0]["speaker"],
        "ada"
    );
    let (status, stale) = owner_call(
        root,
        "POST",
        &format!("/app/agents/api/suggestions/{id}/accept"),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{stale}");
    assert_eq!(stale["code"], "already_changed");
    assert!(
        solstone_core_speaker_resolve::owner_line_labels::current_owner_line_entries(
            directory.path()
        )
        .unwrap()
        .is_empty()
    );
}

#[tokio::test]
async fn activity_disappearing_at_mutation_boundary_never_becomes_accepted() {
    set_test_suggestions_ready(true);
    let journal = init_test_journal();
    let root = journal.path();
    let day = "20261114";
    setup_activity(root, "work", day, "vanishing", "Original");
    let target = crate::names::activity_name(day, "work", "vanishing").unwrap();
    let queued = suggest(root, "recording", &target, json!({}), "vanishing");
    let id = queued["suggestion_id"].as_str().unwrap();
    let path = root.join(format!("facets/work/activities/{day}.jsonl"));
    crate::suggestions::before_activity_apply(move || fs::write(path, "").unwrap());
    let (status, refused) = owner_call(
        root,
        "POST",
        &format!("/app/agents/api/suggestions/{id}/accept"),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["code"], "already_changed");
    let record = read_record(root, id).unwrap().unwrap();
    assert_eq!(record.status, "pending");
    assert!(record.receipt.is_none());
    assert!(
        solstone_core_system::owner_ask::read(root)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn completed_undo_replay_preserves_a_later_account_or_hidden_person_after_lost_receipt() {
    set_test_suggestions_ready(true);
    for kind in ["what_happened", "name"] {
        let journal = init_test_journal();
        let root = journal.path();
        let day = "20261114";
        let (target, value, path) = if kind == "name" {
            setup_entity(root, "work", "dana", "Dana Original", &[]);
            (
                crate::names::entity_name("work", "dana").unwrap(),
                json!({"text":"Dana Accepted","mode":"replace"}),
                root.join("entities/dana/entity.json"),
            )
        } else {
            setup_activity(root, "work", day, "replayed-undo", "Original");
            (
                crate::names::activity_name(day, "work", "replayed-undo").unwrap(),
                json!({"text":"Accepted account"}),
                root.join(format!("facets/work/activities/{day}.jsonl")),
            )
        };
        let queued = suggest(root, kind, &target, value, "replayed-undo");
        let id = queued["suggestion_id"].as_str().unwrap();
        let base = format!("/app/agents/api/suggestions/{id}");
        assert_eq!(
            owner_call(root, "POST", &format!("{base}/accept"), None, true)
                .await
                .0,
            StatusCode::OK
        );
        set_force_suggestion_receipt_failure(true);
        assert_eq!(
            owner_call(root, "POST", &format!("{base}/undo"), None, true)
                .await
                .0,
            StatusCode::INTERNAL_SERVER_ERROR
        );
        set_force_suggestion_receipt_failure(false);
        if kind == "name" {
            solstone_core_facets::block_journal_entity(root, "dana").unwrap();
        } else {
            solstone_core_facets::update_activity_record(
                root,
                "work",
                day,
                "replayed-undo",
                json!({"details":"A later owner answer"})
                    .as_object()
                    .unwrap(),
                "owner:update",
                "owner:update",
                &Utc::now().to_rfc3339(),
            )
            .unwrap();
        }
        let before = fs::read(&path).unwrap();
        let asks = solstone_core_system::owner_ask::read(root).unwrap();
        let (status, undone) = owner_call(root, "POST", &format!("{base}/undo"), None, true).await;
        assert_eq!(status, StatusCode::OK, "{undone}");
        assert_eq!(undone["suggestion"]["status"], "undone");
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(solstone_core_system::owner_ask::read(root).unwrap(), asks);
    }
}

#[tokio::test]
async fn omitted_activity_fields_use_the_same_baseline_as_the_owner_writer() {
    set_test_suggestions_ready(true);
    let journal = init_test_journal();
    let root = journal.path();
    let day = "20261114";
    setup_activity(root, "work", day, "minimal", "");
    let path = root.join(format!("facets/work/activities/{day}.jsonl"));
    let mut activity: Value =
        serde_json::from_str(fs::read_to_string(&path).unwrap().trim()).unwrap();
    activity.as_object_mut().unwrap().remove("details");
    activity.as_object_mut().unwrap().remove("playback");
    fs::write(path, format!("{activity}\n")).unwrap();
    let target = crate::names::activity_name(day, "work", "minimal").unwrap();
    let queued = suggest(
        root,
        "what_happened",
        &target,
        json!({"text":"The owner's account"}),
        "minimal",
    );
    let id = queued["suggestion_id"].as_str().unwrap();
    let (status, accepted) = owner_call(
        root,
        "POST",
        &format!("/app/agents/api/suggestions/{id}/accept"),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    assert_eq!(accepted["suggestion"]["status"], "accepted");
}

#[tokio::test]
async fn account_card_shows_the_journals_story_and_regeneration_stales_it() {
    set_test_suggestions_ready(true);
    for omitted in [false, true] {
        let journal = init_test_journal();
        let root = journal.path();
        let day = "20261114";
        setup_activity(root, "work", day, "story", "");
        let path = root.join(format!("facets/work/activities/{day}.jsonl"));
        let mut activity: Value =
            serde_json::from_str(fs::read_to_string(&path).unwrap().trim()).unwrap();
        activity["description"] = json!("The journal's generated account");
        if omitted {
            activity.as_object_mut().unwrap().remove("details");
        }
        fs::write(&path, format!("{activity}\n")).unwrap();
        let target = crate::names::activity_name(day, "work", "story").unwrap();
        let current = suggest(
            root,
            "what_happened",
            &target,
            json!({"text":"The journal's generated account"}),
            "same-story",
        );
        assert_eq!(current["status"], "already_true", "{current}");
        let queued = suggest(
            root,
            "what_happened",
            &target,
            json!({"text":"The owner's corrected account"}),
            "new-story",
        );
        assert_eq!(queued["status"], "pending", "{queued}");
        let id = queued["suggestion_id"].as_str().unwrap();
        let record = read_record(root, id).unwrap().unwrap();
        assert_eq!(
            super::owner::project_card(root, &record)["now"],
            "The journal's generated account"
        );
        activity["description"] = json!("A newly generated journal account");
        fs::write(&path, format!("{activity}\n")).unwrap();
        let (status, refused) = owner_call(
            root,
            "POST",
            &format!("/app/agents/api/suggestions/{id}/accept"),
            None,
            true,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{refused}");
        assert_eq!(refused["code"], "already_changed");
        assert_eq!(
            refused["suggestion"]["now"],
            "A newly generated journal account"
        );
        assert_eq!(fs::read_to_string(path).unwrap(), format!("{activity}\n"));
    }
}

#[tokio::test]
async fn line_card_uses_canonical_numbers_and_only_voice_or_owner_names() {
    use solstone_core_speaker_resolve::owner_line_labels::{
        LineSpeaker, OwnerLine, set_owner_line_labels,
    };
    set_test_suggestions_ready(true);
    let journal = init_test_journal();
    let root = journal.path();
    let day = "20261114";
    setup_activity(root, "work", day, "meeting_1", "");
    setup_entity(root, "work", "ada", "Ada", &[]);
    setup_entity(root, "work", "bob", "Bob", &[]);
    setup_entity(root, "work", "owner", "Synthetic Owner", &[]);
    let owner_path = root.join("entities/owner/entity.json");
    let mut owner: Value = serde_json::from_slice(&fs::read(&owner_path).unwrap()).unwrap();
    owner["is_principal"] = json!(true);
    fs::write(owner_path, owner.to_string()).unwrap();
    setup_transcript_segment(
        root,
        day,
        "default",
        "090000_300",
        &[("09:00:10", "A sentence from the journal")],
    );
    let directory =
        solstone_core_segment::SegmentDir::resolve(root, day, "090000_300", "default").unwrap();
    fs::write(directory.path().join("audio.jsonl"), format!("{}\n{}\n", json!({"source":"recording"}), json!({"start":"09:00:10","speaker":2,"sentence_id":1,"text":"A sentence from the journal"}))).unwrap();
    let target = crate::names::segment_name(root, day, "default", "090000_300").unwrap();
    let principal = DispatchPrincipal {
        connection: "bearer:test-agent",
        agent_identity: "bearer:test-agent",
    };
    let queued = dispatch_authenticated_tool_call(root, principal, ToolName::SuggestCorrection,
        Some(&json!({"kind":"who_said_it","target":{"reference":target,"lines":[{"time":"09:00:10","starts_with":"A sentence"}]},"value":{"speaker":"ada@work"},"basis":"owner_said","reason":"The owner identified Ada.","operation_id":"displayed-speaker"})), Utc::now()).unwrap().value;
    assert_eq!(queued["status"], "pending", "{queued}");
    let record = read_record(root, queued["suggestion_id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    let card = || super::owner::project_card(root, &record);
    assert_eq!(card()["now_lines"][0]["speaker"], "Speaker 2");
    assert_eq!(card()["now"], "A sentence from the journal");
    fs::create_dir_all(directory.path().join("talents")).unwrap();
    let labels = directory.path().join("talents/speaker_labels.json");
    fs::write(
        &labels,
        json!({"labels":[{"sentence_id":1,"speaker":"bob","method":"contextual"}]}).to_string(),
    )
    .unwrap();
    assert_eq!(card()["now_lines"][0]["speaker"], "Speaker 2");
    fs::write(
        labels,
        json!({"labels":[{"sentence_id":1,"speaker":"bob","method":"acoustic"}]}).to_string(),
    )
    .unwrap();
    assert_eq!(card()["now_lines"][0]["speaker"], "Bob");
    set_owner_line_labels(
        root,
        &[OwnerLine {
            segment_dir: directory.path().to_path_buf(),
            source: "audio".to_owned(),
            sentence_id: 1,
        }],
        &LineSpeaker::Person("ada".to_owned()),
    )
    .unwrap();
    assert_eq!(card()["now_lines"][0]["speaker"], "Ada");
}

fn suggest_same_person(root: &Path, first: &str, second: &str, operation: &str) -> Value {
    dispatch_authenticated_tool_call(
        root,
        DispatchPrincipal {
            connection: "bearer:test-agent",
            agent_identity: "bearer:test-agent",
        },
        ToolName::SuggestCorrection,
        Some(&json!({
            "kind":"same_person", "target":{"reference":first,"other_reference":second}, "value":{},
            "basis":"owner_said", "reason":"Use the person's name from the journal.",
            "operation_id":operation,
        })),
        Utc::now(),
    )
    .unwrap()
    .value
}

#[tokio::test(flavor = "current_thread")]
async fn same_person_only_owner_can_choose_and_merge_with_durable_history() {
    set_test_suggestions_ready(true);
    super::TEST_SAME_PERSON_READY.set(true);
    let journal = init_test_journal();
    let root = journal.path();
    setup_entity(root, "work", "ada", "Ada", &[]);
    fs::rename(root.join("entities/ada"), root.join("entities/legacy-ada")).unwrap();
    setup_entity(root, "work", "a-lovelace", "Ada Lovelace", &[]);
    setup_entity(root, "private", "hidden", "Hidden", &[]);

    assert_eq!(
        suggest_same_person(root, "ada@work", "hidden@work", "hidden")["status"],
        "refused"
    );
    assert_eq!(
        suggest_same_person(root, "ada@work", "ada@work", "same")["status"],
        "already_true"
    );
    let queued = suggest_same_person(root, "ada@work", "a-lovelace@work", "ada");
    assert_eq!(queued["status"], "pending");
    assert!(root.join("entities/legacy-ada/entity.json").exists());
    let duplicate = suggest_same_person(root, "a-lovelace@work", "ada@work", "reverse");
    assert_eq!(duplicate["status"], "already_exists");
    assert_eq!(duplicate["suggestion_id"], queued["suggestion_id"]);
    let id = queued["suggestion_id"].as_str().unwrap();
    let url = format!("/app/agents/api/suggestions/{id}/accept");
    let (_, list) = owner_call(
        root,
        "GET",
        "/app/agents/api/suggestions?status=pending",
        None,
        true,
    )
    .await;
    let card = &list["items"][0];
    assert_eq!(card["change_parts"]["people"][0]["name"], "Ada");
    assert_eq!(card["change_parts"]["people"][1]["name"], "Ada Lovelace");
    assert_eq!(
        card["change_parts"]["people"][1]["attachments"][0]["facet"],
        "work"
    );
    assert_eq!(card["change_parts"]["keep_id"], "ada");
    assert_eq!(card["can_undo"], false);
    let switched = json!({"value":{"keep_id":"a-lovelace"}});
    let (status, _) = owner_call(root, "POST", &url, Some(switched.clone()), false).await;
    assert!(!status.is_success());
    assert!(root.join("entities/legacy-ada/entity.json").exists());

    set_force_suggestion_receipt_failure(true);
    let (status, body) = owner_call(root, "POST", &url, Some(switched), true).await;
    set_force_suggestion_receipt_failure(false);
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    let (status, body) = owner_call(root, "POST", &url, None, true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let card = &body["suggestion"];
    assert_eq!(card["status"], "accepted");
    assert_eq!(card["value"]["keep_id"], "a-lovelace");
    assert_eq!(card["change_parts"]["people"][0]["name"], "Ada");
    assert_eq!(card["asked"], false);
    assert_eq!(card["can_undo"], false);
    let identity = solstone_core_entity::read_entity_identity(root, "a-lovelace")
        .unwrap()
        .unwrap();
    assert_eq!(identity.value()["name"], "Ada Lovelace");
    assert_eq!(identity.value()["aka"], json!(["Ada"]));
    assert_eq!(
        solstone_core_entity::merged_away(root, "ada")
            .unwrap()
            .as_deref(),
        Some("a-lovelace")
    );
    let merges = solstone_core_entity::read_visible_history(root, "a-lovelace").unwrap();
    let merges = merges
        .iter()
        .filter(|event| event.value()["kind"] == "merge")
        .collect::<Vec<_>>();
    assert_eq!(merges.len(), 1);
    let event = merges[0].value();
    assert_eq!(event["actor"]["kind"], "owner");
    assert_eq!(event["operation"]["suggestion_id"], id);
    assert!(event["operation"]["suggested_by"]["label"].is_string());
    assert_eq!(
        suggest_same_person(root, "ada@work", "a-lovelace@work", "ada")["existing_status"],
        "accepted"
    );
    assert_eq!(
        suggest_same_person(root, "a-lovelace@work", "ada@work", "merged")["status"],
        "already_true"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn same_person_journal_candidate_and_owner_answers_take_precedence() {
    set_test_suggestions_ready(true);
    super::TEST_SAME_PERSON_READY.set(true);
    let journal = init_test_journal();
    let root = journal.path();
    setup_entity(root, "work", "ada", "Ada", &[]);
    setup_entity(root, "work", "a-lovelace", "Ada Lovelace", &[]);
    let queued = suggest_same_person(root, "ada@work", "a-lovelace@work", "ada");
    let id = queued["suggestion_id"].as_str().unwrap();
    solstone_core_entity::record_merge_candidate(
        root,
        "work",
        "20261114",
        "Ada Lovelace",
        "a-lovelace",
        "Ada",
        "ada",
        "The results look similar.",
        None,
        Some(2),
        Some(2),
    )
    .unwrap();
    let (_, list) = owner_call(
        root,
        "GET",
        "/app/agents/api/suggestions?status=pending",
        None,
        true,
    )
    .await;
    assert_eq!(list["items"], json!([]));
    assert_eq!(
        suggest_same_person(root, "a-lovelace@work", "ada@work", "open")["status"],
        "already_exists"
    );
    let (status, body) = owner_call(
        root,
        "POST",
        &format!("/app/agents/api/suggestions/{id}/accept"),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["suggestion"]["status"], "stale");
    assert_eq!(body["suggestion"]["stale_reason"], "journal_candidate");
    solstone_core_entity::dismiss_merge_candidate(root, "work", "a-lovelace", "ada").unwrap();
    let (_, earlier) = owner_call(
        root,
        "GET",
        "/app/agents/api/suggestions?status=stale",
        None,
        true,
    )
    .await;
    assert_eq!(earlier["items"][0]["stale_reason"], "owner_answered");
    assert_eq!(
        suggest_same_person(root, "ada@work", "a-lovelace@work", "answered")["status"],
        "owner_answered"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn same_person_rechecks_speaker_candidates_and_recovers_a_chained_merge() {
    use solstone_core_speaker_resolve::{
        name_variant_scan::NameVariantCandidate, speaker_review_candidates,
    };
    set_test_suggestions_ready(true);
    super::TEST_SAME_PERSON_READY.set(true);
    let journal = init_test_journal();
    let root = journal.path();
    for (id, name) in [
        ("ada", "Ada"),
        ("al", "Ada Lovelace"),
        ("ada-l", "Ada L"),
        ("bea", "Bea"),
        ("bee", "Bee"),
        ("jon", "Jon"),
        ("john", "John"),
    ] {
        setup_entity(root, "work", id, name, &[]);
    }
    let candidate = NameVariantCandidate {
        source_id: "bea".to_owned(),
        source_label: "Bea".to_owned(),
        target_id: "bee".to_owned(),
        target_label: "Bee".to_owned(),
        similarity: 0.98,
        readiness: "ready".to_owned(),
    };
    let queued = suggest_same_person(root, "bea@work", "bee@work", "agent-first");
    assert_eq!(queued["status"], "pending");
    speaker_review_candidates::record_name_variant_candidate(root, &candidate).unwrap();
    let (_, list) = owner_call(
        root,
        "GET",
        "/app/agents/api/suggestions?status=pending",
        None,
        true,
    )
    .await;
    assert_eq!(list["items"], json!([]));
    let (_, stale) = owner_call(
        root,
        "GET",
        "/app/agents/api/suggestions?status=stale",
        None,
        true,
    )
    .await;
    // A speaker candidate is not in possible duplicates, so the card does not point there.
    assert_eq!(stale["items"][0]["stale_reason"], "journal_question");
    assert_eq!(
        suggest_same_person(root, "bee@work", "bea@work", "speaker-first")["status"],
        "already_exists"
    );
    speaker_review_candidates::dismiss_candidate(root, "bea", "bee").unwrap();
    assert_eq!(
        suggest_same_person(root, "bee@work", "bea@work", "speaker-dismissed")["status"],
        "owner_answered"
    );

    let queued = suggest_same_person(root, "ada@work", "al@work", "chain");
    assert_eq!(queued["status"], "pending");
    let id = queued["suggestion_id"].as_str().unwrap();
    let url = format!("/app/agents/api/suggestions/{id}/accept");
    set_force_suggestion_receipt_failure(true);
    let (status, body) = owner_call(
        root,
        "POST",
        &url,
        Some(json!({"value":{"keep_id":"al"}})),
        true,
    )
    .await;
    set_force_suggestion_receipt_failure(false);
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    solstone_core_entity::commit_entity_merge(
        root,
        "al",
        "ada-l",
        solstone_core_entity::EntityMergeOptions {
            keep_source_as_aka: true,
        },
        &solstone_core_entity::EncoderIdentity {
            id: "unresolved".to_owned(),
            sha256: "0".repeat(64),
            width: 256,
        },
    )
    .unwrap();
    let (status, body) = owner_call(root, "POST", &url, None, true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["suggestion"]["status"], "accepted");
    assert_eq!(body["suggestion"]["value"]["keep_id"], "al");
    assert_eq!(
        solstone_core_entity::read_merge_log(root).unwrap().0.len(),
        2
    );
    // This pair has never had an agent proposal. A manual merge records
    // the missing source's original membership in the exact facet.
    solstone_core_entity::commit_entity_merge(
        root,
        "jon",
        "john",
        solstone_core_entity::EntityMergeOptions {
            keep_source_as_aka: true,
        },
        &solstone_core_entity::EncoderIdentity {
            id: "unresolved".to_owned(),
            sha256: "0".repeat(64),
            width: 256,
        },
    )
    .unwrap();
    assert_eq!(
        suggest_same_person(root, "jon@work", "john@work", "manual")["status"],
        "already_true"
    );
    solstone_core_facets::create_facet(root, "other", "Other", "", "", "", None).unwrap();
    let linked = root.join("facets/other/entities/john");
    fs::create_dir_all(&linked).unwrap();
    fs::write(
        linked.join("entity.json"),
        json!({"entity_id":"john"}).to_string(),
    )
    .unwrap();
    assert_eq!(
        suggest_same_person(root, "jon@other", "john@other", "guessed")["status"],
        "refused"
    );
}
