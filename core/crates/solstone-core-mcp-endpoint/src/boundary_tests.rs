// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

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
        .tempdir_in("/var/tmp")
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
