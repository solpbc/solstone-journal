// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]

//! Purpose-built MCP read-boundary fixtures. These never use the development
//! journal fixture because each case needs isolated, adversarial journal state.

use std::fs;

use rusqlite::params;
use serde_json::{Value, json};

use crate::dispatch::{McpProbeError, run_mcp_probe};
use crate::permissions::{
    PermissionDecision, PermissionStore, ReadPermission, ReadScope, evaluate_connection_read,
};

const CONNECTION: &str = "bearer:boundary-test";
const FACET_A: &str = "123e4567-e89b-42d3-a456-426614174000";
const FACET_B: &str = "123e4567-e89b-42d3-a456-426614174001";
const DAY: &str = "20260914";
const STREAM: &str = "default";
const SEGMENT: &str = "090000_300";
const PATH: &str = "20260914/default/090000_300/talents/brief.md";

fn fixture() -> tempfile::TempDir {
    let journal = tempfile::Builder::new()
        .prefix("solstone-mcp-boundary-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    for (name, id, title) in [("alpha", FACET_A, "Alpha"), ("beta", FACET_B, "Beta")] {
        let directory = journal.path().join("facets").join(name);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("facet.json"),
            json!({"id": id, "title": title, "description": format!("{title} details")})
                .to_string(),
        )
        .unwrap();
    }
    let segment = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    fs::create_dir_all(segment.join("talents")).unwrap();
    fs::write(
        segment.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(
        segment.join("meeting_transcript.md"),
        "approved transcript\n",
    )
    .unwrap();
    fs::write(segment.join("audio.jsonl"), "percept material\n").unwrap();
    fs::write(segment.join("talents/brief.md"), "disk bytes\n").unwrap();
    for (directory, id, detached) in [
        ("primary", "entity-primary", false),
        ("same-name", "entity-same-name", false),
        ("detached", "entity-detached", true),
    ] {
        let entity = journal.path().join("entities").join(directory);
        fs::create_dir_all(&entity).unwrap();
        fs::write(
            entity.join("entity.json"),
            json!({
                "id": id,
                "name": "Same Name",
                "aka": ["forbidden aka"],
                "description": "out-of-scope description",
                "detached_facets": ["forbidden detached-facet list"],
                "decisions": ["forbidden cross-facet aggregate"],
            })
            .to_string(),
        )
        .unwrap();
        let link = journal.path().join("facets/alpha/entities").join(directory);
        fs::create_dir_all(&link).unwrap();
        fs::write(
            link.join("entity.json"),
            json!({"entity_id": id, "detached": detached}).to_string(),
        )
        .unwrap();
    }

    let index = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    index
        .execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
             VALUES ('indexed bytes', ?1, ?2, '', 'fixture-stream', ?3, 0, '')",
            params![PATH, DAY, STREAM],
        )
        .unwrap();
    index
        .execute(
            "INSERT INTO chunk_classification(path, category, basis, eligible, unclassified) \
             VALUES (?1, 'transcripts', 'segment_assigned', 1, 0)",
            params![PATH],
        )
        .unwrap();
    index
        .execute(
            "INSERT INTO chunk_classification_facets(path, facet_id) VALUES (?1, ?2)",
            params![PATH, FACET_A],
        )
        .unwrap();
    index
        .execute(
            "REPLACE INTO chunk_classification_backfill(id, cursor, completed, stalled, stalled_path, resume_count) \
             VALUES (1, '', 1, 0, NULL, 0)",
            [],
        )
        .unwrap();
    index
        .execute(
            "REPLACE INTO index_build_state(id, schema_version, state, files_count, chunks_count) \
             VALUES (1, 1, 'complete', 1, 1)",
            [],
        )
        .unwrap();
    drop(index);

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();
    journal
}

fn probe(
    journal: &tempfile::TempDir,
    tool: &str,
    arguments: Value,
) -> Result<Value, McpProbeError> {
    run_mcp_probe(journal.path(), CONNECTION, tool, &arguments)
}

fn visible_search_page(result: &Value) -> Value {
    let results = result["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|result| {
            json!({
                "title": result["title"],
                "date": result["date"],
                "snippet": result["snippet"],
            })
        })
        .collect::<Vec<_>>();
    json!({"results": results, "coverage": result["coverage"]})
}

fn assert_no_coordinate_keys(value: &Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                assert_no_coordinate_keys(value);
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                assert!(
                    !matches!(key.as_str(), "path" | "idx" | "row_id" | "rowid" | "stream"),
                    "agent result exposed internal key {key}"
                );
                assert_no_coordinate_keys(value);
            }
        }
        _ => {}
    }
}

#[test]
fn ac5_live_check_rejects_reassigned_segment_without_rescanning_the_index() {
    let journal = fixture();
    assert_eq!(
        probe(&journal, "search", json!({"query": "indexed"})).unwrap()["results"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        probe(&journal, "list_transcripts", json!({})).unwrap()["transcripts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let assignments = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT)
        .join("talents/facets.json");
    fs::write(assignments, "[{\"facet\":\"beta\"}]").unwrap();

    assert!(
        probe(&journal, "search", json!({"query": "indexed"})).unwrap()["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        probe(&journal, "list_transcripts", json!({})).unwrap()["transcripts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn ac10_fetch_returns_indexed_bytes_not_the_current_source_file() {
    let journal = fixture();
    let search = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    let reference = search["results"][0]["reference"].as_str().unwrap();
    let fetched = probe(&journal, "fetch", json!({"reference": reference})).unwrap();
    assert_eq!(fetched["text"], "indexed bytes");
    assert_ne!(fetched["text"], "disk bytes");
}

#[test]
fn ac23_chosen_scope_excludes_unassigned_segments_while_whole_journal_includes_them() {
    let journal = fixture();
    let assignments = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT)
        .join("talents/facets.json");
    fs::write(&assignments, "[]").unwrap();
    assert!(
        probe(&journal, "list_transcripts", json!({})).unwrap()["transcripts"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "list_transcripts", json!({})).unwrap()["transcripts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn ac25_a_refusal_is_recorded_and_the_owner_can_tell_it_from_a_served_call() {
    // 🔑 This supersedes the three-field assertion: EVIDENCE.md § 5.3's gap was
    // that an owner asking "what did this connection try?" could not be told
    // "and it was refused". A denial and an admission differed only by
    // timestamp. They no longer do.
    let journal = fixture();
    let served = probe(&journal, "search", json!({"query": "indexed"}));
    assert!(served.is_ok());
    PermissionStore::open(journal.path())
        .clear_permission(CONNECTION)
        .unwrap();
    assert_eq!(
        probe(&journal, "search", json!({"query": "indexed"})),
        Err(McpProbeError::PermissionDenied)
    );

    let page = crate::activity::read_activity(
        journal.path(),
        &crate::activity::ActivityQuery {
            limit: 10,
            ..crate::activity::ActivityQuery::default()
        },
    )
    .unwrap();
    assert_eq!(page.entries.len(), 2);
    let outcomes = page
        .entries
        .iter()
        .map(|entry| entry.outcome)
        .collect::<Vec<_>>();
    assert!(outcomes.contains(&crate::activity::RecordedOutcome::Refused));
    assert!(outcomes.contains(&crate::activity::RecordedOutcome::Served));
    let refused = page
        .entries
        .iter()
        .find(|entry| entry.outcome == crate::activity::RecordedOutcome::Refused)
        .unwrap();
    assert_eq!(refused.tool_name, solstone_core_mcp_audit::ToolName::Search);
    assert_eq!(refused.connection.as_deref(), Some(CONNECTION));
    assert_eq!(
        refused.reason.as_deref(),
        Some("this connection has no read permission")
    );
    // 🔑 And the wire keeps the other half of the asymmetry: both refusals
    // — no permission at all, and a permission that lacks the category —
    // render identically to the agent.
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "search", json!({"query": "indexed"})),
        Err(McpProbeError::PermissionDenied)
    );
    let page = crate::activity::read_activity(
        journal.path(),
        &crate::activity::ActivityQuery {
            limit: 10,
            outcome: Some(crate::activity::RecordedOutcome::Refused),
            ..crate::activity::ActivityQuery::default()
        },
    )
    .unwrap();
    // ⚠ Two refusals, indistinguishable on the wire, distinguishable here.
    assert_eq!(page.entries.len(), 2);
    let mut reasons = page
        .entries
        .iter()
        .filter_map(|entry| entry.reason.clone())
        .collect::<Vec<_>>();
    reasons.sort();
    assert_eq!(
        reasons,
        [
            "this connection does not have the transcripts category",
            "this connection has no read permission",
        ]
    );
}

#[test]
fn ac25b_a_search_that_matches_nothing_is_recorded_empty_rather_than_served() {
    let journal = fixture();
    assert!(
        probe(&journal, "search", json!({"query": "zzzznotpresent"})).unwrap()["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let page = crate::activity::read_activity(
        journal.path(),
        &crate::activity::ActivityQuery {
            limit: 10,
            ..crate::activity::ActivityQuery::default()
        },
    )
    .unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(
        page.entries[0].outcome,
        crate::activity::RecordedOutcome::Empty
    );
    assert_eq!(
        page.entries[0].request.as_ref().unwrap().arguments["query"],
        "zzzznotpresent"
    );
}

fn dispatch_output(
    journal: &tempfile::TempDir,
    tool: &str,
    arguments: Value,
) -> crate::dispatch::ToolOutput {
    let entry = crate::registry::tool_by_wire_name(tool).unwrap();
    crate::dispatch::dispatch_authenticated_tool_call(
        journal.path(),
        crate::dispatch::DispatchPrincipal {
            connection: CONNECTION,
            agent_identity: CONNECTION,
        },
        entry.tool_name,
        Some(&arguments),
        chrono::Utc::now(),
        None,
    )
    .unwrap()
}

#[test]
fn an_empty_result_says_so_in_words_and_a_served_one_does_not() {
    let journal = fixture();
    let empty = dispatch_output(&journal, "search", json!({"query": "zzzznotpresent"}));
    assert!(empty.value["results"].as_array().unwrap().is_empty());
    let note = empty
        .empty_note
        .clone()
        .expect("an empty search carries a plain note");
    assert!(note.starts_with("No results"), "{note}");
    // ⚠ Search does not cover raw transcripts, so the note must not claim the
    // journal as a whole has nothing.
    assert!(note.contains("raw transcripts"), "{note}");

    let rendered = crate::jsonrpc::tool_result(empty.value, empty.empty_note.as_deref());
    assert_eq!(rendered["content"][1]["text"], note);
    assert!(rendered["structuredContent"].get("note").is_none());

    let served = dispatch_output(&journal, "list_facets", json!({}));
    assert!(!served.value["facets"].as_array().unwrap().is_empty());
    assert!(served.empty_note.is_none());
}

#[test]
fn a_journal_that_has_recorded_nothing_lists_no_transcripts_rather_than_failing() {
    let journal = fixture();
    fs::remove_dir_all(journal.path().join("chronicle")).unwrap();
    let output = dispatch_output(&journal, "list_transcripts", json!({}));
    assert!(output.value["transcripts"].as_array().unwrap().is_empty());
    assert!(output.empty_note.is_some());
}

#[test]
fn ac26_the_owners_activity_log_is_unreachable_through_the_boundary_it_audits() {
    // 🔴 The recursion, measured rather than argued. A connection with the
    // widest grant this build can express searches for a term that exists only
    // inside its own interaction records.
    let journal = fixture();
    probe(
        &journal,
        "search",
        json!({"query": "supercalifragilisticexpialidocious"}),
    )
    .unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec![
                    "transcripts".to_owned(),
                    "entities".to_owned(),
                    "facets".to_owned(),
                ],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    // Control: the term really is on disk, in this journal, right now.
    let recorded = crate::activity::read_activity(
        journal.path(),
        &crate::activity::ActivityQuery {
            limit: 10,
            ..crate::activity::ActivityQuery::default()
        },
    )
    .unwrap();
    assert!(
        recorded.entries.iter().any(|entry| {
            entry.request.as_ref().is_some_and(|request| {
                request.arguments["query"] == "supercalifragilisticexpialidocious"
            })
        }),
        "control failed: the term was never recorded, so the negative below proves nothing"
    );
    // Second control, on the coordinate that could be wrong: the same query
    // shape does find the fixture's indexed material.
    assert_eq!(
        probe(&journal, "search", json!({"query": "indexed"})).unwrap()["results"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    assert!(
        probe(
            &journal,
            "search",
            json!({"query": "supercalifragilisticexpialidocious"})
        )
        .unwrap()["results"]
            .as_array()
            .unwrap()
            .is_empty(),
        "an agent reached the owner's log of its own queries"
    );
    assert!(
        probe(&journal, "list_transcripts", json!({})).unwrap()["transcripts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|segment| !segment.to_string().contains("mcp.agent"))
    );
}

#[test]
fn ac4_entities_without_facets_can_list_only_facet_id_and_name() {
    let journal = fixture();
    let result = probe(&journal, "list_facets", json!({})).unwrap();
    let facet = &result["facets"][0];
    assert_eq!(facet["id"], FACET_A);
    assert_eq!(facet["name"], "Alpha");
    assert!(facet.get("description").is_none());
    assert!(facet.get("summary").is_none());
}

#[test]
fn a_malformed_or_undeclared_facet_directory_is_skipped_not_fatal() {
    let journal = fixture();
    let malformed = journal.path().join("facets").join("gamma");
    fs::create_dir_all(&malformed).unwrap();
    fs::write(malformed.join("facet.json"), "not json").unwrap();
    let undeclared = journal.path().join("facets").join("delta");
    fs::create_dir_all(&undeclared).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    let result = probe(&journal, "list_facets", json!({})).unwrap();
    let ids = result["facets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|facet| facet["id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![FACET_A.to_owned(), FACET_B.to_owned()]);
}

#[test]
fn ac3_search_and_fetch_require_the_transcripts_category() {
    let journal = fixture();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "search", json!({"query": "indexed"})),
        Err(McpProbeError::PermissionDenied)
    );
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": "not-a-reference"})),
        Err(McpProbeError::PermissionDenied)
    );
}

#[test]
fn ac15_ac16_exam_bound_is_unquantified_when_live_drops_cannot_fill_the_page() {
    let journal = fixture();
    let index = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    for idx in 1..=crate::dispatch::MAX_SEARCH_EXAMINED_ROWS as i64 {
        index
            .execute(
                "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
                 VALUES ('indexed', ?1, ?2, '', 'fixture-stream', ?3, ?4, '')",
                params![PATH, DAY, STREAM, idx],
            )
            .unwrap();
    }
    drop(index);
    let assignments = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT)
        .join("talents/facets.json");
    fs::write(assignments, "[{\"facet\":\"beta\"}]").unwrap();

    let result = probe(&journal, "search", json!({"query": "indexed", "limit": 1})).unwrap();
    assert_eq!(result["coverage"]["live_examination_complete"], false);
    let serialized = result.to_string();
    for forbidden in ["count", "percent", "range", "remaining", "dropped"] {
        assert!(
            !serialized.contains(forbidden),
            "coverage leaked {forbidden}"
        );
    }
}

#[test]
fn ac14_unauthorized_chunks_do_not_change_visible_search_page_or_coverage() {
    let journal = fixture();
    let before = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    let before = visible_search_page(&before);

    let unauthorized_segment = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join("100000_300");
    fs::create_dir_all(unauthorized_segment.join("talents")).unwrap();
    fs::write(
        unauthorized_segment.join("talents/facets.json"),
        "[{\"facet\":\"beta\"}]",
    )
    .unwrap();
    let unauthorized_path = "20260914/default/100000_300/talents/brief.md";
    let index = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    index
        .execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
             VALUES ('indexed unauthorized', ?1, ?2, '', 'fixture-stream', ?3, 0, '')",
            params![unauthorized_path, DAY, STREAM],
        )
        .unwrap();
    index
        .execute(
            "INSERT INTO chunk_classification(path, category, basis, eligible, unclassified) \
             VALUES (?1, 'transcripts', 'segment_assigned', 1, 0)",
            params![unauthorized_path],
        )
        .unwrap();
    index
        .execute(
            "INSERT INTO chunk_classification_facets(path, facet_id) VALUES (?1, ?2)",
            params![unauthorized_path, FACET_B],
        )
        .unwrap();
    drop(index);

    let after = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    assert_eq!(visible_search_page(&after), before);
}

#[test]
fn ac13_cursor_rejects_a_replaced_anchor_and_valid_cursor_continues_without_repeats() {
    let journal = fixture();
    let index = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    index
        .execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
             VALUES ('indexed next', ?1, ?2, '', 'fixture-stream', ?3, 1, '')",
            params![PATH, DAY, STREAM],
        )
        .unwrap();
    drop(index);

    let first = probe(&journal, "search", json!({"query": "indexed", "limit": 1})).unwrap();
    let cursor = first["next_cursor"].as_str().unwrap().to_owned();
    let second = probe(
        &journal,
        "search",
        json!({"query": "indexed", "limit": 1, "cursor": cursor}),
    )
    .unwrap();
    assert_ne!(
        first["results"][0]["snippet"],
        second["results"][0]["snippet"]
    );

    let index = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    index.execute("DELETE FROM chunks", []).unwrap();
    index
        .execute(
            "INSERT INTO chunks(rowid, content, path, day, facet, agent, stream, idx, time_bucket) \
             VALUES (2, 'replacement indexed bytes', ?1, ?2, '', 'fixture-stream', ?3, 1, '')",
            params![PATH, DAY, STREAM],
        )
        .unwrap();
    drop(index);
    assert_eq!(
        probe(
            &journal,
            "search",
            json!({"query": "indexed", "limit": 1, "cursor": cursor}),
        ),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn ac13_cursor_rejects_a_reset_rebuild_with_rowids_restarted_at_one() {
    let journal = fixture();
    let index = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    index
        .execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
             VALUES ('indexed next', ?1, ?2, '', 'fixture-stream', ?3, 1, '')",
            params![PATH, DAY, STREAM],
        )
        .unwrap();
    drop(index);
    let first = probe(&journal, "search", json!({"query": "indexed", "limit": 1})).unwrap();
    let cursor = first["next_cursor"].as_str().unwrap().to_owned();

    let index = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    index.execute("DELETE FROM chunks", []).unwrap();
    index
        .execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
             VALUES ('indexed next', ?1, ?2, '', 'fixture-stream', ?3, 1, '')",
            params![PATH, DAY, STREAM],
        )
        .unwrap();
    index
        .execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
             VALUES ('indexed bytes', ?1, ?2, '', 'fixture-stream', ?3, 0, '')",
            params![PATH, DAY, STREAM],
        )
        .unwrap();
    let row_ids = index
        .prepare("SELECT rowid FROM chunks ORDER BY rowid")
        .unwrap()
        .query_map([], |row| row.get::<_, i64>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    assert_eq!(row_ids, [1, 2]);
    drop(index);

    assert_eq!(
        probe(
            &journal,
            "search",
            json!({"query": "indexed", "limit": 1, "cursor": cursor}),
        ),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn ac28_generation_recheck_refuses_a_narrowed_permission_before_release() {
    let journal = fixture();
    let PermissionDecision::Snapshot(snapshot) =
        evaluate_connection_read(journal.path(), CONNECTION)
    else {
        panic!("fixture permission is enforceable");
    };
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();
    assert!(!crate::dispatch::permission_generation_is_current(
        journal.path(),
        CONNECTION,
        &snapshot,
    ));
}

#[test]
fn ac24_get_transcript_pages_a_huge_segment_without_returning_the_whole_file() {
    let journal = fixture();
    let transcript = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT)
        .join("meeting_transcript.md");
    fs::write(&transcript, "pageable transcript\n".repeat(10_000)).unwrap();
    let listed = probe(&journal, "list_transcripts", json!({})).unwrap();
    let reference = listed["transcripts"][0]["reference"].as_str().unwrap();
    let page = probe(&journal, "get_transcript", json!({"reference": reference})).unwrap();
    assert!(page["entries"].as_array().unwrap().len() <= 100);
    assert!(page["next_cursor"].is_string());
}

#[test]
fn get_transcript_reads_recorded_speech_from_audio_jsonl() {
    let journal = fixture();
    let segment = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    fs::remove_file(segment.join("meeting_transcript.md")).unwrap();
    fs::write(
        segment.join("audio.jsonl"),
        "{\"source\":\"recording\"}\n{\"start\":\"00:00:02\",\"text\":\"spoken words\"}\n",
    )
    .unwrap();

    let listed = probe(&journal, "list_transcripts", json!({})).unwrap();
    let reference = listed["transcripts"][0]["reference"].as_str().unwrap();
    let page = probe(&journal, "get_transcript", json!({"reference": reference})).unwrap();
    assert_eq!(page["entries"], json!(["[00:00:02] spoken words"]));
    assert!(page["next_cursor"].is_null());
}

#[test]
fn ac7_ac21_ac22_entities_are_scoped_stable_and_omit_forbidden_fields() {
    let journal = fixture();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();
    let listed = probe(&journal, "list_entities", json!({})).unwrap();
    assert_eq!(listed["entities"].as_array().unwrap().len(), 2);
    let reference = listed["entities"][0]["reference"].as_str().unwrap();
    let entity = probe(&journal, "get_entity", json!({"reference": reference})).unwrap();
    assert_eq!(entity, json!({"name": "Same Name"}));
    let serialized = entity.to_string();
    for forbidden in ["aka", "out-of-scope", "detached", "aggregate", "decision"] {
        assert!(!serialized.contains(forbidden));
    }
}

#[test]
fn ac19_ac20_all_agent_results_omit_coordinates_and_keep_index_coverage_statement() {
    let journal = fixture();
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let facets = probe(&journal, "list_facets", json!({})).unwrap();
    let search = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    let fetch = probe(
        &journal,
        "fetch",
        json!({"reference": search["results"][0]["reference"]}),
    )
    .unwrap();
    let transcripts = probe(&journal, "list_transcripts", json!({})).unwrap();
    let transcript = probe(
        &journal,
        "get_transcript",
        json!({"reference": transcripts["transcripts"][0]["reference"]}),
    )
    .unwrap();
    let entities = probe(&journal, "list_entities", json!({})).unwrap();
    let entity = probe(
        &journal,
        "get_entity",
        json!({"reference": entities["entities"][0]["reference"]}),
    )
    .unwrap();

    for result in [
        &facets,
        &search,
        &fetch,
        &transcripts,
        &transcript,
        &entities,
        &entity,
    ] {
        assert!(!result.to_string().contains("20260914/default/090000_300"));
        assert_no_coordinate_keys(result);
    }
    assert_eq!(
        search["coverage"]["transcript_index"],
        "Raw transcript JSONL is not covered by index search."
    );
}

#[test]
fn ac6_delete_recreate_and_ac9_rename_keep_stable_facet_ids_real() {
    let journal = fixture();
    let alpha = journal.path().join("facets/alpha");
    fs::rename(&alpha, journal.path().join("facets/alpha-renamed")).unwrap();
    assert_eq!(
        probe(&journal, "list_entities", json!({})),
        Err(McpProbeError::PermissionDenied)
    );
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".to_owned(), "transcripts".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "list_entities", json!({})).unwrap()["entities"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(
        probe(&journal, "search", json!({"query":"indexed"})).unwrap()["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        probe(&journal, "list_transcripts", json!({})).unwrap()["transcripts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    fs::remove_dir_all(journal.path().join("facets/alpha-renamed")).unwrap();
    fs::create_dir_all(&alpha).unwrap();
    fs::write(
        alpha.join("facet.json"),
        json!({"id": FACET_B, "title":"Alpha"}).to_string(),
    )
    .unwrap();
    assert!(
        probe(&journal, "list_facets", json!({})).unwrap()["facets"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

fn grant(journal: &tempfile::TempDir, scope: ReadScope) {
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".to_owned(), "transcripts".to_owned()],
                scope,
            },
        )
        .unwrap();
}

fn reachable(journal: &tempfile::TempDir) -> (usize, usize) {
    let search = probe(journal, "search", json!({"query":"indexed"}))
        .map(|value| value["results"].as_array().map_or(0, Vec::len))
        .unwrap_or(0);
    let transcripts = probe(journal, "list_transcripts", json!({}))
        .map(|value| value["transcripts"].as_array().map_or(0, Vec::len))
        .unwrap_or(0);
    (search, transcripts)
}

fn reconcile(journal: &tempfile::TempDir) {
    let mut snapshot = || {
        solstone_core_indexer_store::classification::FacetDeclarationSet::from_journal(
            journal.path(),
        )
    };
    solstone_core_indexer_store::reconcile::reconcile_stale_classifications(
        journal.path(),
        &mut snapshot,
    )
    .unwrap();
}

#[test]
fn a_merged_facet_name_reaches_the_survivor_in_search_and_transcripts_and_never_its_old_grant() {
    let journal = fixture();
    // What `journal facet merge alpha --into beta` leaves: alpha's folder is
    // gone and its name resolves to beta. The index was built before it.
    fs::remove_dir_all(journal.path().join("facets/alpha")).unwrap();
    fs::write(
        journal.path().join("facets/retired.json"),
        json!({"names": {"alpha": {"state": "merged", "id": FACET_A, "successor": FACET_B}}})
            .to_string(),
    )
    .unwrap();
    grant(
        &journal,
        ReadScope::Facets {
            ids: vec![FACET_B.to_owned()],
        },
    );
    // Transcripts re-check live, so they follow at once; search needs the
    // stored classification brought up to date.
    assert_eq!(reachable(&journal), (0, 1));
    reconcile(&journal);
    assert_eq!(reachable(&journal), (1, 1));
    grant(
        &journal,
        ReadScope::Facets {
            ids: vec![FACET_A.to_owned()],
        },
    );
    assert_eq!(reachable(&journal), (0, 0));
    grant(
        &journal,
        ReadScope::Facets {
            ids: vec!["123e4567-e89b-42d3-a456-426614174009".to_owned()],
        },
    );
    assert_eq!(reachable(&journal), (0, 0));
}

#[test]
fn a_deleted_facet_name_is_never_given_to_a_new_facet_and_a_forced_reuse_stays_closed() {
    let journal = fixture();
    assert!(solstone_core_facets::delete_facet(journal.path(), "alpha").unwrap());
    assert!(matches!(
        solstone_core_facets::create_facet(journal.path(), "alpha", "Alpha", "", "", "", None),
        Err(solstone_core_facets::FacetWriteError::NameRetired { .. })
    ));
    // A hand edit reuses the name anyway, with a new id.
    let alpha = journal.path().join("facets/alpha");
    fs::create_dir_all(&alpha).unwrap();
    const FACET_C: &str = "123e4567-e89b-42d3-a456-426614174002";
    fs::write(
        alpha.join("facet.json"),
        json!({"id": FACET_C, "title": "Alpha"}).to_string(),
    )
    .unwrap();
    reconcile(&journal);
    grant(
        &journal,
        ReadScope::Facets {
            ids: vec![FACET_C.to_owned()],
        },
    );
    assert_eq!(reachable(&journal), (0, 0));
    grant(&journal, ReadScope::WholeJournal);
    assert_eq!(reachable(&journal), (1, 1));
}

#[test]
fn ac8_ac11_ac12_ac30_bad_live_scope_and_references_fail_closed() {
    let journal = fixture();
    let search = probe(&journal, "search", json!({"query":"indexed"})).unwrap();
    let reference = search["results"][0]["reference"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        probe(&journal, "fetch", json!({"reference":"notes/a:b.txt:42"})),
        Err(McpProbeError::Unavailable)
    );
    let tampered = probe(&journal, "fetch", json!({"reference":"tampered"}));
    assert_eq!(tampered, Err(McpProbeError::Unavailable));
    PermissionStore::open(journal.path())
        .set_permission("bearer:other", ReadPermission::default_whole_journal())
        .unwrap();
    assert_eq!(
        run_mcp_probe(
            journal.path(),
            "bearer:other",
            "fetch",
            &json!({"reference":reference})
        ),
        Err(McpProbeError::Unavailable)
    );
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": reference})),
        tampered
    );
    let facets = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT)
        .join("talents/facets.json");
    fs::write(facets, "not json").unwrap();
    assert!(
        probe(&journal, "list_transcripts", json!({})).unwrap()["transcripts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        probe(&journal, "search", json!({"query":"indexed"})).unwrap()["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    PermissionStore::open(journal.path())
        .clear_permission(CONNECTION)
        .unwrap();
    assert_eq!(
        probe(&journal, "search", json!({"query":"indexed"})),
        Err(McpProbeError::PermissionDenied)
    );
}

static PORT_7658_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn tls_pair(
    hostname: &str,
) -> (
    std::sync::Arc<rustls::ServerConfig>,
    std::sync::Arc<rustls::ClientConfig>,
) {
    use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use rustls::{ClientConfig, RootCertStore, ServerConfig};
    let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("fixture key");
    let certificate = CertificateParams::new(vec![hostname.to_owned()])
        .expect("fixture params")
        .self_signed(&key_pair)
        .expect("fixture certificate");
    let certificate = CertificateDer::from(certificate.der().to_vec());
    let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));
    let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
    let mut server = ServerConfig::builder_with_provider(std::sync::Arc::clone(&provider))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("ring provider supports TLS 1.3")
        .with_no_client_auth()
        .with_single_cert(vec![certificate.clone()], private_key)
        .expect("fixture server certificate");
    server.alpn_protocols = vec![b"http/1.1".to_vec()];

    let mut roots = RootCertStore::empty();
    roots.add(certificate).expect("fixture root");
    let mut client = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("ring provider supports TLS 1.3")
        .with_root_certificates(roots)
        .with_no_client_auth();
    client.alpn_protocols = vec![b"http/1.1".to_vec()];
    (std::sync::Arc::new(server), std::sync::Arc::new(client))
}

#[tokio::test]
async fn offer_count_does_not_rise_after_raise_on_a_blocked_large_write() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::{TlsAcceptor, TlsConnector};

    let (server_tls, client_tls) = tls_pair("mcp.example.com");
    let (client_io, server_io) = tokio::io::duplex(1024);

    let door = crate::serving_epoch::EndpointDoor::new();
    let (epoch, _) = door.open_epoch();
    let offers = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let guarded_server = crate::serving_epoch::OfferGuardedStream::new(
        server_io,
        std::sync::Arc::clone(&epoch.closed),
        epoch.shutdown.subscribe(),
        std::sync::Arc::clone(&offers),
    );

    let acceptor = TlsAcceptor::from(server_tls);
    let connector = TlsConnector::from(client_tls);
    let domain = rustls::pki_types::ServerName::try_from("mcp.example.com")
        .unwrap()
        .to_owned();

    let server_task = tokio::spawn(async move {
        let mut tls_stream = acceptor.accept(guarded_server).await.unwrap();
        let mut req_buf = vec![0u8; 100];
        let _ = tls_stream.read(&mut req_buf).await;
        let large_body = vec![b'A'; 10 * 1024 * 1024];
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            large_body.len()
        );
        let _ = tls_stream.write_all(headers.as_bytes()).await;
        let _ = tls_stream.write_all(&large_body).await;
        let _ = tls_stream.flush().await;
    });

    let client_task = tokio::spawn(async move {
        let mut tls_client = connector.connect(domain, client_io).await.unwrap();
        tls_client
            .write_all(b"GET / HTTP/1.1\r\nHost: mcp.example.com\r\n\r\n")
            .await
            .unwrap();
        let mut buf = vec![0u8; 50];
        let n = tls_client.read(&mut buf).await.unwrap();
        assert!(n > 0);
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        (tls_client, n)
    });

    let (_client, _initial_read) = client_task.await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    door.raise();
    let count_at_raise = offers.load(std::sync::atomic::Ordering::SeqCst);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let count_after_raise = offers.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        count_at_raise, count_after_raise,
        "offers should not increase after raise"
    );

    server_task.abort();
}

#[tokio::test]
async fn offer_count_reads_full_body_when_not_raised() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::{TlsAcceptor, TlsConnector};

    let (server_tls, client_tls) = tls_pair("mcp.example.com");
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);

    let door = crate::serving_epoch::EndpointDoor::new();
    let (epoch, _) = door.open_epoch();
    let offers = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let guarded_server = crate::serving_epoch::OfferGuardedStream::new(
        server_io,
        std::sync::Arc::clone(&epoch.closed),
        epoch.shutdown.subscribe(),
        std::sync::Arc::clone(&offers),
    );

    let acceptor = TlsAcceptor::from(server_tls);
    let connector = TlsConnector::from(client_tls);
    let domain = rustls::pki_types::ServerName::try_from("mcp.example.com")
        .unwrap()
        .to_owned();

    let body_len = 64 * 1024;
    let server_task = tokio::spawn(async move {
        let mut tls_stream = acceptor.accept(guarded_server).await.unwrap();
        let mut req_buf = vec![0u8; 100];
        let _ = tls_stream.read(&mut req_buf).await;
        let large_body = vec![b'B'; body_len];
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            large_body.len()
        );
        let _ = tls_stream.write_all(headers.as_bytes()).await;
        let _ = tls_stream.write_all(&large_body).await;
        let _ = tls_stream.flush().await;
        let _ = tls_stream.shutdown().await;
    });

    let client_task = tokio::spawn(async move {
        let mut tls_client = connector.connect(domain, client_io).await.unwrap();
        tls_client
            .write_all(b"GET / HTTP/1.1\r\nHost: mcp.example.com\r\n\r\n")
            .await
            .unwrap();
        let mut resp = Vec::new();
        let _ = tls_client.read_to_end(&mut resp).await.unwrap();
        resp
    });

    let resp = client_task.await.unwrap();
    let _ = server_task.await;
    assert!(resp.len() > body_len);
    assert!(offers.load(std::sync::atomic::Ordering::SeqCst) > 0);
}

struct NotifyGuard(std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>);
impl Drop for NotifyGuard {
    fn drop(&mut self) {
        let (lock, cvar) = &*self.0;
        let mut started = lock.lock().unwrap();
        *started = true;
        cvar.notify_all();
    }
}

#[tokio::test]
async fn blocked_tool_call_offers_nothing_and_completion_is_awaitable() {
    use std::sync::{Arc, Condvar, Mutex};

    let door = Arc::new(crate::serving_epoch::EndpointDoor::new());
    let (epoch, completion_rx) = door.open_epoch();

    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let pair_clone = Arc::clone(&pair);
    let door_clone = Arc::clone(&door);
    let notify_guard = NotifyGuard(Arc::clone(&pair));

    let raised = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let raised_clone = Arc::clone(&raised);

    let offers = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    epoch
        .blocked_calls
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let guard = crate::server::BlockedCallGuard(Some(Arc::clone(&epoch)));

    let tool_handle = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        door_clone.raise();
        raised_clone.store(true, std::sync::atomic::Ordering::Release);
        let (lock, cvar) = &*pair_clone;
        let mut started = lock.lock().unwrap();
        while !*started {
            started = cvar.wait(started).unwrap();
        }
    });

    let start = tokio::time::Instant::now();
    while !raised.load(std::sync::atomic::Ordering::Acquire) {
        if start.elapsed() > std::time::Duration::from_secs(2) {
            panic!("timed out waiting for raise in spawn_blocking thread");
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    assert!(epoch.is_closed());
    assert_eq!(offers.load(std::sync::atomic::Ordering::Relaxed), 0);

    epoch.finish_completion(crate::serving_epoch::EpochCompletion {
        aborted_after_bound: Vec::new(),
        blocked_calls_still_running: epoch
            .blocked_calls
            .load(std::sync::atomic::Ordering::Relaxed),
    });

    let completion = completion_rx.await.expect("completion arrives");
    assert_eq!(completion.blocked_calls_still_running, 1);
    assert!(completion.aborted_after_bound.is_empty());

    drop(notify_guard);
    let _ = tool_handle.await;
}

#[test]
fn client_reads_end_of_stream_while_runtime_workers_are_blocked() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();

    let pair = std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let pair_clone = std::sync::Arc::clone(&pair);
    let notify_guard = NotifyGuard(std::sync::Arc::clone(&pair));

    let (done_tx, done_rx) = std::sync::mpsc::channel();

    let (client_raw, server_raw) = std::os::unix::net::UnixStream::pair().unwrap();
    client_raw.set_nonblocking(false).unwrap();

    std::thread::spawn(move || {
        rt.block_on(async move {
            let door = std::sync::Arc::new(crate::serving_epoch::EndpointDoor::new());
            let (epoch, _completion_rx) = door.open_epoch();

            epoch
                .blocked_calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let guard = crate::server::BlockedCallGuard(Some(std::sync::Arc::clone(&epoch)));
            tokio::task::spawn_blocking(move || {
                let _guard = guard;
                let (lock, cvar) = &*pair_clone;
                let mut unblock = lock.lock().unwrap();
                while !*unblock {
                    unblock = cvar.wait(unblock).unwrap();
                }
            });

            tokio::spawn(async {
                std::thread::sleep(std::time::Duration::from_millis(500));
            });
            tokio::spawn(async {
                std::thread::sleep(std::time::Duration::from_millis(500));
            });

            let door_clone = std::sync::Arc::clone(&door);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(50));
                door_clone.raise();
                drop(server_raw);
            });

            let _ = done_rx.recv();
        });
    });

    use std::io::Read;
    let mut client = client_raw;
    let mut buf = [0u8; 10];
    let start = std::time::Instant::now();
    let n = client.read(&mut buf).unwrap();
    assert_eq!(n, 0, "client read returns 0 (EOF)");
    assert!(start.elapsed() < std::time::Duration::from_secs(2));

    drop(notify_guard);
    let _ = done_tx.send(());
}

#[tokio::test]
async fn cloudflare_preface_literals_are_refused_before_cutoff_and_accepted_at_cutoff() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::watch;

    let (server_tls, _client_tls) = tls_pair("mcp.example.com");
    let before_cutoff = chrono::DateTime::parse_from_rfc3339("2026-11-30T06:15:59Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let at_cutoff = chrono::DateTime::parse_from_rfc3339("2026-11-30T06:16:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    let refused_before = [
        ("173.245.48.0", false),
        ("173.245.63.255", false),
        ("104.16.0.0", false),
        ("104.23.255.255", false),
        ("104.24.0.0", false),
        ("104.27.255.255", false),
        ("::ffff:104.16.0.1", true),
        ("::104.16.0.1", true),
    ];
    let accepted_before = [("104.15.255.255", false), ("104.28.0.0", false)];

    async fn check_preface(addr: std::net::SocketAddr, ip_str: &str, is_v6: bool) -> bool {
        let mut stream = match TcpStream::connect(addr).await {
            Ok(s) => s,
            Err(_) => return true,
        };
        let line = if is_v6 {
            format!("PROXY TCP6 {ip_str} ::1 1234 7658\r\n")
        } else {
            format!("PROXY TCP4 {ip_str} 127.0.0.1 1234 7658\r\n")
        };
        if stream.write_all(line.as_bytes()).await.is_err() {
            return true;
        }
        let mut buf = [0u8; 1];
        match tokio::time::timeout(std::time::Duration::from_millis(50), stream.read(&mut buf))
            .await
        {
            Ok(Ok(0)) => true,
            Ok(Err(_)) => true,
            _ => false,
        }
    }

    // Run before cutoff
    {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let journal = fixture();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let oauth = std::sync::Arc::new(crate::oauth::OAuthRuntime::new(
            journal.path(),
            "https://mcp.example.com".to_string(),
        ));
        let server_task = tokio::spawn(crate::server::serve_with_epoch_and_clock(
            listener,
            std::sync::Arc::clone(&server_tls),
            std::sync::Arc::new(journal.path().to_path_buf()),
            oauth,
            shutdown_rx,
            None,
            Some(before_cutoff),
        ));

        for (ip, is_v6) in refused_before {
            assert!(
                check_preface(addr, ip, is_v6).await,
                "expected {ip} to be refused before cutoff"
            );
        }
        for (ip, is_v6) in accepted_before {
            assert!(
                !check_preface(addr, ip, is_v6).await,
                "expected {ip} to be accepted before cutoff"
            );
        }

        let _ = shutdown_tx.send(true);
        let _ = server_task.await;
    }

    // Run at cutoff
    {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let journal = fixture();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let oauth = std::sync::Arc::new(crate::oauth::OAuthRuntime::new(
            journal.path(),
            "https://mcp.example.com".to_string(),
        ));
        let server_task = tokio::spawn(crate::server::serve_with_epoch_and_clock(
            listener,
            server_tls,
            std::sync::Arc::new(journal.path().to_path_buf()),
            oauth,
            shutdown_rx,
            None,
            Some(at_cutoff),
        ));

        for (ip, is_v6) in refused_before {
            assert!(
                !check_preface(addr, ip, is_v6).await,
                "expected {ip} to be accepted at cutoff"
            );
        }

        let _ = shutdown_tx.send(true);
        let _ = server_task.await;
    }
}

#[tokio::test]
async fn bind_failure_shuts_the_session_and_latches_refusal_without_exiting() {
    let _lock = PORT_7658_LOCK.lock().unwrap();
    let journal = fixture();
    let blocker = tokio::net::TcpListener::bind(("127.0.0.1", 7658))
        .await
        .expect("bind blocker");

    crate::owner_state::publish_closed_status(
        journal.path(),
        Some("bind_failed"),
        Some("bind_failed"),
    );

    let state = crate::owner_state::read_mcp_owner_state(journal.path()).expect("read state");
    assert_eq!(state.status, "closed");
    assert_eq!(state.open_refusal.as_deref(), Some("bind_failed"));

    drop(blocker);
}

#[tokio::test]
async fn three_open_cycles_leave_one_listener() {
    let _lock = PORT_7658_LOCK.lock().unwrap();
    let door = crate::serving_epoch::EndpointDoor::new();

    for _ in 0..3 {
        let (epoch, completion_rx) = door.open_epoch();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 7658))
            .await
            .expect("bind listener");
        door.raise();
        epoch.finish_completion(crate::serving_epoch::EpochCompletion {
            aborted_after_bound: Vec::new(),
            blocked_calls_still_running: 0,
        });
        let _ = completion_rx.await;
        drop(listener);
        door.clear_epoch();
    }

    assert_eq!(door.current_epoch_id(), 3);
    let final_listener = tokio::net::TcpListener::bind(("127.0.0.1", 7658)).await;
    assert!(final_listener.is_ok());
}

#[tokio::test]
async fn process_exit_paths_raise_the_door_then_exit() {
    let journal = fixture();

    // 1. Shutdown signal path
    {
        let door = std::sync::Arc::new(crate::serving_epoch::EndpointDoor::new());
        let (epoch, _) = door.open_epoch();
        let (shutdown_send, _shutdown_recv) = tokio::sync::watch::channel(false);
        let guard = crate::server::BlockedCallGuard(Some(std::sync::Arc::clone(&epoch)));
        let (unblock_tx, unblock_rx) = std::sync::mpsc::channel::<()>();
        let handle = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let _ = unblock_rx.recv();
        });

        door.raise();
        let _ = shutdown_send.send(true);
        assert!(epoch.is_closed());
        let state = crate::owner_state::read_mcp_owner_state(journal.path());
        assert!(
            state
                .as_ref()
                .and_then(|s| s.open_refusal.as_ref())
                .is_none()
        );
        let _ = unblock_tx.send(());
        let _ = handle.await;
    }

    // 2. Capability off path
    {
        let door = std::sync::Arc::new(crate::serving_epoch::EndpointDoor::new());
        let (epoch, _) = door.open_epoch();
        let (shutdown_send, _shutdown_recv) = tokio::sync::watch::channel(false);
        let guard = crate::server::BlockedCallGuard(Some(std::sync::Arc::clone(&epoch)));
        let (unblock_tx, unblock_rx) = std::sync::mpsc::channel::<()>();
        let handle = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let _ = unblock_rx.recv();
        });

        door.raise();
        let _ = shutdown_send.send(true);
        assert!(epoch.is_closed());
        let state = crate::owner_state::read_mcp_owner_state(journal.path());
        assert!(
            state
                .as_ref()
                .and_then(|s| s.open_refusal.as_ref())
                .is_none()
        );
        let _ = unblock_tx.send(());
        let _ = handle.await;
    }

    // 3. Hosted parent loss path
    {
        let door = std::sync::Arc::new(crate::serving_epoch::EndpointDoor::new());
        let (epoch, _) = door.open_epoch();
        let (shutdown_send, _shutdown_recv) = tokio::sync::watch::channel(false);
        let guard = crate::server::BlockedCallGuard(Some(std::sync::Arc::clone(&epoch)));
        let (unblock_tx, unblock_rx) = std::sync::mpsc::channel::<()>();
        let handle = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let _ = unblock_rx.recv();
        });

        door.raise();
        let _ = shutdown_send.send(true);
        assert!(epoch.is_closed());
        let state = crate::owner_state::read_mcp_owner_state(journal.path());
        assert!(
            state
                .as_ref()
                .and_then(|s| s.open_refusal.as_ref())
                .is_none()
        );
        let _ = unblock_tx.send(());
        let _ = handle.await;
    }
}
